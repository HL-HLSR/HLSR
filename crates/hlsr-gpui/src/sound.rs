//! Звук лаунчера через cpal (решение владельца 3): WAV из памяти — прослушивание модов
//! (`new Audio(dataUrl).play()` 0.4.9). Звук идёт, пока жив [`Playback`]; доигран — зовётся
//! `ended` (из потока звука).

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, SampleFormat, SizedSample, StreamConfig};
use std::sync::{Arc, Mutex, mpsc};

/// Разобранный WAV: отсчёты −1…1 подряд по каналам.
#[derive(Debug, Clone, PartialEq)]
pub struct Pcm {
    pub rate: u32,
    pub channels: usize,
    pub samples: Vec<f32>,
}

/// RIFF/WAVE: PCM 8/16/24/32 бит или float 32 (и `WAVE_FORMAT_EXTENSIBLE` с ними); прочие
/// куски (`cue `, `LIST`) пропускаются, кусок длиннее файла обрезается (так пишут старые WAV HL1).
pub fn decode_wav(bytes: &[u8]) -> Option<Pcm> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let u16_at =
        |b: &[u8], at: usize| Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?));
    let u32_at =
        |b: &[u8], at: usize| Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?));
    let (mut format, mut data) = (None, None);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32_at(bytes, at + 4)? as usize;
        let body = &bytes[at + 8..(at + 8).saturating_add(size).min(bytes.len())];
        match &bytes[at..at + 4] {
            b"fmt " => format = Some(body),
            b"data" => data = Some(body),
            _ => {}
        }
        at = (at + 8).saturating_add(size).saturating_add(size & 1);
    }
    let (fmt, data) = (format?, data?);
    let mut tag = u16_at(fmt, 0)?;
    if tag == 0xfffe {
        tag = u16_at(fmt, 24)?;
    }
    let channels = usize::from(u16_at(fmt, 2)?);
    let rate = u32_at(fmt, 4)?;
    let bits = u16_at(fmt, 14)?;
    if !(1..=8).contains(&channels) || !(1000..=384_000).contains(&rate) {
        return None;
    }
    let samples: Vec<f32> = match (tag, bits) {
        (1, 8) => data.iter().map(|&b| (f32::from(b) - 128.) / 128.).collect(),
        (1, 16) => data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| f32::from(i16::from_le_bytes(b)) / 32_768.)
            .collect(),
        (1, 24) => data
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&[a, b, c]| (i32::from_le_bytes([0, a, b, c]) >> 8) as f32 / 8_388_608.)
            .collect(),
        (1, 32) => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| i32::from_le_bytes(b) as f32 / 2_147_483_648.)
            .collect(),
        (3, 32) => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| f32::from_le_bytes(b).clamp(-1., 1.))
            .collect(),
        _ => return None,
    };
    Some(Pcm {
        rate,
        channels,
        samples,
    })
}

/// Звук в поток устройства: линейная передискретизация, каналы по кругу (моно — во все).
struct Mixer {
    pcm: Pcm,
    out_channels: usize,
    /// Позиция в кадрах источника и шаг на кадр устройства.
    pos: f64,
    step: f64,
    ended: Option<Box<dyn FnOnce() + Send>>,
}

impl Mixer {
    fn frames(&self) -> usize {
        self.pcm.samples.len() / self.pcm.channels
    }

    fn fill<T: SizedSample + FromSample<f32>>(&mut self, out: &mut [T]) {
        let (frames, channels) = (self.frames(), self.pcm.channels);
        for frame in out.chunks_mut(self.out_channels) {
            let i = self.pos as usize;
            if i >= frames {
                frame.fill(T::from_sample(0.));
                if let Some(ended) = self.ended.take() {
                    ended();
                }
                continue;
            }
            let t = (self.pos - i as f64) as f32;
            let next = (i + 1).min(frames - 1);
            for (c, sample) in frame.iter_mut().enumerate() {
                let c = c % channels;
                let a = self.pcm.samples[i * channels + c];
                let b = self.pcm.samples[next * channels + c];
                *sample = T::from_sample(a + (b - a) * t);
            }
            self.pos += self.step;
        }
    }
}

/// Сколько поток окна ждёт открытия устройства вывода.
const OPEN_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// Звук играет, пока жив: поток `hlsr-sound` держит `cpal::Stream`, пока жив отправитель.
pub struct Playback {
    _stop: mpsc::Sender<()>,
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: StreamConfig,
    mut mixer: Mixer,
    ended: impl Fn() + Send + 'static,
) -> Option<cpal::Stream> {
    device
        .build_output_stream(
            config,
            move |out: &mut [T], _: &cpal::OutputCallbackInfo| mixer.fill(out),
            move |error: cpal::Error| {
                tracing::warn!("[sound] поток вывода: {error}");
                // Устройство пропало или поток сломан — звука больше не будет: кнопка не «играет».
                if !matches!(
                    error.kind(),
                    ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied
                ) {
                    ended();
                }
            },
            None,
        )
        .map_err(|error| tracing::warn!("[sound] поток вывода не открылся: {error}"))
        .ok()
}

/// Играть WAV на устройстве вывода по умолчанию. `None` — не разобрался или устройства нет
/// (как `audio.play()` с отказом: `ended` не зовётся). `ended` — один раз: доигран или сбой потока.
///
/// WASAPI (COM) живёт в своём потоке от открытия до закрытия: в потоке окна вызовы COM крутят
/// его сообщения посреди обновления App, и GPUI теряет кадр («RefCell already borrowed»).
/// Поток окна ждёт только ответа об открытии.
pub fn play_wav(bytes: &[u8], ended: impl FnOnce() + Send + 'static) -> Option<Playback> {
    let pcm = decode_wav(bytes).filter(|pcm| pcm.samples.len() >= pcm.channels)?;
    let (stop, stopped) = mpsc::channel::<()>();
    let (ready, opened) = mpsc::sync_channel::<bool>(1);
    std::thread::Builder::new()
        .name("hlsr-sound".into())
        .spawn(move || {
            let stream = open(pcm, ended);
            // Окно не дождалось (`OPEN_WAIT`) — звук уже не нужен.
            if ready.send(stream.is_some()).is_ok() && stream.is_some() {
                // Ждёт, пока жив `Playback`: отправитель ушёл — поток звука закрывается.
                let _ = stopped.recv();
            }
        })
        .map_err(|error| tracing::warn!("[sound] поток звука не запустился: {error}"))
        .ok()?;
    // Зависший драйвер не морозит окно: не открылось за `OPEN_WAIT` — как нет устройства.
    opened
        .recv_timeout(OPEN_WAIT)
        .map_err(|error| tracing::warn!("[sound] устройство не открылось: {error}"))
        .unwrap_or(false)
        .then_some(Playback { _stop: stop })
}

/// Открыть поток вывода и пустить звук (в потоке `hlsr-sound`).
fn open(pcm: Pcm, ended: impl FnOnce() + Send + 'static) -> Option<cpal::Stream> {
    let once = Arc::new(Mutex::new(Some(ended)));
    let ended = move || {
        if let Some(ended) = once.lock().ok().and_then(|mut once| once.take()) {
            ended();
        }
    };
    let device = cpal::default_host().default_output_device()?;
    let supported = device
        .default_output_config()
        .map_err(|error| tracing::warn!("[sound] нет формата вывода: {error}"))
        .ok()?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let mixer = Mixer {
        step: f64::from(pcm.rate) / f64::from(config.sample_rate),
        pcm,
        out_channels: usize::from(config.channels.max(1)),
        pos: 0.,
        ended: Some(Box::new(ended.clone())),
    };
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, config, mixer, ended),
        SampleFormat::I16 => build::<i16>(&device, config, mixer, ended),
        SampleFormat::U16 => build::<u16>(&device, config, mixer, ended),
        SampleFormat::I32 => build::<i32>(&device, config, mixer, ended),
        other => {
            tracing::warn!("[sound] формат вывода {other:?} не поддержан");
            None
        }
    }?;
    stream
        .play()
        .map_err(|error| tracing::warn!("[sound] не играет: {error}"))
        .ok()?;
    Some(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn wav(fmt: &[u8], data: &[u8], extra: &[u8]) -> Vec<u8> {
        let mut out = b"RIFF\0\0\0\0WAVE".to_vec();
        out.extend(b"fmt ");
        out.extend((fmt.len() as u32).to_le_bytes());
        out.extend(fmt);
        out.extend(extra);
        out.extend(b"data");
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        out
    }

    fn fmt(tag: u16, channels: u16, rate: u32, bits: u16) -> Vec<u8> {
        let mut out = tag.to_le_bytes().to_vec();
        out.extend(channels.to_le_bytes());
        out.extend(rate.to_le_bytes());
        out.extend([0; 6]);
        out.extend(bits.to_le_bytes());
        out
    }

    #[test]
    fn wav_of_hl1_and_16_bit_stereo_decode() {
        // 8 бит моно 11025 Гц, перед данными — нечётный кусок `cue ` с байтом выравнивания.
        let cue = b"cue \x03\0\0\0abc\0";
        let pcm = decode_wav(&wav(&fmt(1, 1, 11_025, 8), &[128, 255, 0], cue)).unwrap();
        assert_eq!((pcm.rate, pcm.channels), (11_025, 1));
        assert_eq!(pcm.samples, vec![0., 127. / 128., -1.]);
        let data: Vec<u8> = [i16::MIN, 16_384]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let pcm = decode_wav(&wav(&fmt(1, 2, 44_100, 16), &data, &[])).unwrap();
        assert_eq!(pcm.samples, vec![-1., 0.5]);
        // Кусок данных длиннее файла — обрезается.
        let mut cut = wav(&fmt(1, 1, 22_050, 8), &[128, 128], &[]);
        cut.truncate(cut.len() - 1);
        assert_eq!(decode_wav(&cut).unwrap().samples.len(), 1);
        assert_eq!(decode_wav(&wav(&fmt(2, 1, 22_050, 4), &[0], &[])), None);
        assert_eq!(decode_wav(b"RIFF\0\0\0\0WAVE"), None);
    }

    #[test]
    fn the_mixer_resamples_spreads_mono_and_ends_once() {
        let ended = Arc::new(AtomicBool::new(false));
        let flag = ended.clone();
        let mut mixer = Mixer {
            pcm: Pcm {
                rate: 1,
                channels: 1,
                samples: vec![0., 1.],
            },
            out_channels: 2,
            pos: 0.,
            step: 0.5,
            ended: Some(Box::new(move || flag.store(true, Ordering::SeqCst))),
        };
        let mut out = [9f32; 10];
        mixer.fill(&mut out);
        assert_eq!(out, [0., 0., 0.5, 0.5, 1., 1., 1., 1., 0., 0.]);
        assert!(ended.load(Ordering::SeqCst));
        assert!(mixer.ended.is_none());
    }
}
