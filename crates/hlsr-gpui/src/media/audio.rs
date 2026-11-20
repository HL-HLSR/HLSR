//! Звук видео: `ffmpeg -f s16le` под частоту и каналы устройства → cpal. Поток устройства один
//! на плеер ([`Audio::open`]); перемотка и скорость меняют только источник ([`Audio::load`]),
//! поток WASAPI не пересоздаётся. Часы плеера идут по тому, что уже слышно ([`Audio::time`]).

use super::{MediaError, spawn, stop};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, SampleFormat, SizedSample, StreamConfig};
use std::io::{self, Read};
use std::path::Path;
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Кусок из трубы — 1024 кадра (~21 мс при 48 кГц); в очереди до 16 кусков.
const CHUNK_FRAMES: usize = 1024;
const QUEUE: usize = 16;

/// Последний вызов устройства: кадров источника до него, в нём, когда он был и через сколько
/// его начало прозвучит.
#[derive(Clone, Copy)]
struct Heard {
    before: u64,
    len: u64,
    at: Instant,
    latency: Duration,
}

/// Источник глазами потока устройства.
#[derive(Default)]
struct Feed {
    queue: Option<Receiver<Vec<f32>>>,
    chunk: Vec<f32>,
    at: usize,
    /// Кадров источника отдано устройству.
    written: u64,
    heard: Option<Heard>,
    /// Источник кончился, всё отдано.
    drained: bool,
}

struct Shared {
    feed: Mutex<Feed>,
    volume: AtomicU32,
    /// Поток устройства сломан: звука больше не будет.
    broken: AtomicBool,
}

impl Shared {
    /// Вызов устройства: `channels` каналов источника в первые каналы устройства, прочие — тишина;
    /// источник не успел — тишина без хода часов.
    fn fill<T: SizedSample + FromSample<f32>>(
        &self,
        out: &mut [T],
        channels: usize,
        out_channels: usize,
        latency: Duration,
    ) {
        let silence = T::from_sample(0.);
        // Захваты UI короткие (копия часов, замена источника): ждать их дешевле, чем молчать весь
        // вызов.
        let Ok(mut feed) = self.feed.lock() else {
            out.fill(silence);
            return;
        };
        let volume = f32::from_bits(self.volume.load(Ordering::Relaxed));
        let before = feed.written;
        for frame in out.chunks_mut(out_channels) {
            if feed.at >= feed.chunk.len() {
                match feed.queue.as_ref().map(Receiver::try_recv) {
                    Some(Ok(chunk)) => (feed.chunk, feed.at) = (chunk, 0),
                    Some(Err(TryRecvError::Disconnected)) => feed.drained = true,
                    _ => {}
                }
                if feed.at >= feed.chunk.len() {
                    frame.fill(silence);
                    continue;
                }
            }
            let at = feed.at;
            for (c, sample) in frame.iter_mut().enumerate() {
                *sample = if c < channels {
                    T::from_sample(feed.chunk[at + c] * volume)
                } else {
                    silence
                };
            }
            feed.at += channels;
            feed.written += 1;
        }
        let len = feed.written - before;
        feed.heard = Some(Heard {
            before,
            len,
            at: Instant::now(),
            latency,
        });
    }
}

/// Звук плеера: поток устройства вывода по умолчанию и процесс FFmpeg текущего источника.
pub struct Audio {
    stream: cpal::Stream,
    shared: Arc<Shared>,
    child: Option<Child>,
    rate: u32,
    channels: usize,
    /// Время видео начала источника и скорость.
    start: f64,
    speed: f64,
}

/// `atempo` в пределах 0,5…2 на звено (скорость 0,25 — два звена), высота тона не меняется,
/// как `playbackRate` у `<video>`.
pub(super) fn atempo(mut speed: f64) -> String {
    let mut chain = Vec::new();
    while speed < 0.5 {
        chain.push("atempo=0.5".to_owned());
        speed /= 0.5;
    }
    while speed > 2. {
        chain.push("atempo=2".to_owned());
        speed /= 2.;
    }
    chain.push(format!("atempo={speed}"));
    chain.join(",")
}

fn args(file: &Path, at: f64, speed: f64, rate: u32, channels: usize) -> Vec<String> {
    let mut args: Vec<String> = ["-v", "error", "-nostdin", "-ss"].map(String::from).into();
    args.extend([
        format!("{at:.6}"),
        "-i".into(),
        file.to_string_lossy().into(),
    ]);
    args.extend(["-vn", "-sn", "-dn"].map(String::from));
    if speed != 1. {
        args.extend(["-af".into(), atempo(speed)]);
    }
    args.extend([
        "-f".into(),
        "s16le".into(),
        "-ac".into(),
        channels.to_string(),
        "-ar".into(),
        rate.to_string(),
        "pipe:1".into(),
    ]);
    args
}

/// Сколько влезло до конца буфера или трубы.
fn read_full(from: &mut impl Read, buffer: &mut [u8]) -> usize {
    let mut filled = 0;
    while filled < buffer.len() {
        match from.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    filled
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: StreamConfig,
    shared: &Arc<Shared>,
    channels: usize,
) -> Result<cpal::Stream, MediaError> {
    let out_channels = usize::from(config.channels.max(1));
    let (feed, broken) = (shared.clone(), shared.clone());
    device
        .build_output_stream(
            config,
            move |out: &mut [T], info: &cpal::OutputCallbackInfo| {
                let stamp = info.timestamp();
                let latency = stamp.playback.saturating_duration_since(stamp.callback);
                feed.fill(out, channels, out_channels, latency);
            },
            move |error: cpal::Error| {
                tracing::warn!("[media] поток звука: {error}");
                // Звука больше не будет — часы плеера идут дальше сами.
                if !matches!(
                    error.kind(),
                    ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied
                ) {
                    broken.broken.store(true, Ordering::Relaxed);
                }
            },
            None,
        )
        .map_err(|error| MediaError(format!("поток звука не открылся: {error}")))
}

impl Audio {
    /// Поток устройства вывода по умолчанию, на паузе и без источника. Нет устройства — `Err`
    /// (видео идёт без звука).
    pub fn open(volume: f32) -> Result<Audio, MediaError> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| MediaError("нет устройства вывода звука".into()))?;
        let supported = device
            .default_output_config()
            .map_err(|error| MediaError(format!("нет формата вывода: {error}")))?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let rate = config.sample_rate;
        let channels = usize::from(config.channels.clamp(1, 2));
        let shared = Arc::new(Shared {
            feed: Mutex::default(),
            volume: AtomicU32::new(volume.to_bits()),
            broken: AtomicBool::new(false),
        });
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, config, &shared, channels),
            SampleFormat::I16 => build::<i16>(&device, config, &shared, channels),
            SampleFormat::U16 => build::<u16>(&device, config, &shared, channels),
            SampleFormat::I32 => build::<i32>(&device, config, &shared, channels),
            other => Err(MediaError(format!("формат вывода {other:?} не поддержан"))),
        }?;
        let _ = stream.pause();
        Ok(Audio {
            stream,
            shared,
            child: None,
            rate,
            channels,
            start: 0.,
            speed: 1.,
        })
    }

    /// Источник с `at` (время видео) на скорости `speed`; прежний процесс гасится.
    pub fn load(
        &mut self,
        ffmpeg: &Path,
        file: &Path,
        at: f64,
        speed: f64,
    ) -> Result<(), MediaError> {
        self.unload();
        let mut child = spawn(ffmpeg, &args(file, at, speed, self.rate, self.channels))?;
        let mut out = child.stdout.take().expect("stdout в трубе");
        let (sender, queue) = sync_channel(QUEUE);
        let frame = 2 * self.channels;
        let reader = std::thread::Builder::new()
            .name("hlsr-audio".into())
            .spawn(move || {
                let mut buffer = vec![0u8; CHUNK_FRAMES * frame];
                loop {
                    let read = read_full(&mut out, &mut buffer);
                    let whole = read - read % frame;
                    let samples: Vec<f32> = buffer[..whole]
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|&bytes| f32::from(i16::from_le_bytes(bytes)) / 32_768.)
                        .collect();
                    if (!samples.is_empty() && sender.send(samples).is_err()) || read < buffer.len()
                    {
                        break;
                    }
                }
            });
        if let Err(error) = reader {
            stop(child);
            return Err(MediaError(format!("поток чтения звука: {error}")));
        }
        self.child = Some(child);
        let new = Feed {
            queue: Some(queue),
            ..Feed::default()
        };
        let old = self
            .shared
            .feed
            .lock()
            .ok()
            .map(|mut feed| std::mem::replace(&mut *feed, new));
        // Очередь прежнего источника освобождается вне захвата.
        drop(old);
        (self.start, self.speed) = (at, speed);
        Ok(())
    }

    pub(super) fn unload(&mut self) {
        if let Some(child) = self.child.take() {
            stop(child);
        }
    }

    pub fn play(&self) {
        if let Err(error) = self.stream.play() {
            tracing::warn!("[media] звук не играет: {error}");
            // Вызовов устройства не будет — часы плеера идут сами.
            self.shared.broken.store(true, Ordering::Relaxed);
        }
    }

    pub fn pause(&self) {
        let _ = self.stream.pause();
    }

    pub fn set_volume(&self, volume: f32) {
        self.shared
            .volume
            .store(volume.clamp(0., 1.).to_bits(), Ordering::Relaxed);
    }

    /// Время видео, которое слышно сейчас (между вызовами устройства — по системным часам, не
    /// дальше отданного). `None` — источника нет, он кончился или поток сломан: часы плеера
    /// идут сами.
    pub fn time(&self) -> Option<f64> {
        if self.shared.broken.load(Ordering::Relaxed) {
            return None;
        }
        let heard = {
            let feed = self.shared.feed.lock().ok()?;
            if feed.drained || feed.queue.is_none() {
                return None;
            }
            feed.heard
        };
        let heard = heard.map_or(0., |heard| {
            let rate = f64::from(self.rate);
            let now = heard.before as f64 / rate + heard.at.elapsed().as_secs_f64()
                - heard.latency.as_secs_f64();
            now.clamp(0., (heard.before + heard.len) as f64 / rate)
        });
        Some(self.start + heard * self.speed)
    }

    /// PID процесса FFmpeg источника.
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.unload();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atempo_keeps_each_link_within_half_to_double() {
        assert_eq!(atempo(0.25), "atempo=0.5,atempo=0.5");
        assert_eq!(atempo(0.75), "atempo=0.75");
        assert_eq!(atempo(2.), "atempo=2");
        assert_eq!(atempo(4.), "atempo=2,atempo=2");
    }

    #[test]
    fn the_feed_spreads_channels_counts_only_real_frames_and_drains() {
        let shared = Shared {
            feed: Mutex::default(),
            volume: AtomicU32::new(0.5f32.to_bits()),
            broken: AtomicBool::new(false),
        };
        let (sender, queue) = sync_channel(4);
        sender.send(vec![1., -1., 0.5, -0.5]).unwrap();
        drop(sender);
        shared.feed.lock().unwrap().queue = Some(queue);
        // Стерео источник в 3 канала устройства, 3 кадра: 2 настоящих, затем конец — тишина.
        let mut out = [9f32; 9];
        shared.fill(&mut out, 2, 3, Duration::ZERO);
        assert_eq!(out, [0.5, -0.5, 0., 0.25, -0.25, 0., 0., 0., 0.]);
        let feed = shared.feed.lock().unwrap();
        assert_eq!((feed.written, feed.drained), (2, true));
        assert_eq!(
            feed.heard.map(|heard| (heard.before, heard.len)),
            Some((0, 2))
        );
    }
}
