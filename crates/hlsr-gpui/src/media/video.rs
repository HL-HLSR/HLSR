//! Видео плеера «Демо». Кадры: `ffmpeg -ss <t> -i <файл> -vf [setpts=PTS/<скорость>,]fps=<fps>,
//! scale=<w>:<h> -f rawvideo -pix_fmt bgra pipe:1` — BGRA под размер рамки (не больше исходного:
//! крупнее растягивает видеокарта), ровным шагом `скорость / fps` секунд видео. Кадр
//! показывается, когда до него дошли часы; опоздавшие пропускаются без перевода в картинку.
//! Часы — по звуку (что уже слышно), без звука или когда он кончился — системные. Перемотка,
//! скорость и новый размер перезапускают FFmpeg с `-ss`; прежний процесс гасится по своему
//! дескриптору, как и при закрытии плеера.

use super::audio::Audio;
use super::{MediaError, command, spawn, stop};
use crate::image::Raster;
use gpui::{App, RenderImage, Window};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::time::{Duration, Instant};

/// Новый размер рамки применяется, когда он не менялся столько (анимация разворота не
/// перезапускает FFmpeg на каждом кадре).
const RESIZE_SETTLE: Duration = Duration::from_millis(150);
/// Кадров в очереди после трубы (у 1080p ~8 МБ каждый).
const QUEUE: usize = 2;
/// FFmpeg молчит столько (нет первого кадра или не выходит после конца трубы) — ошибка:
/// зависший процесс не держит окно в вечной перерисовке.
const STALL: Duration = Duration::from_secs(10);

/// Кадр: размер и байты BGRA.
pub type Bgra = ((u32, u32), Vec<u8>);

/// Что известно о файле до воспроизведения.
#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    /// Секунды (`format.duration`).
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    /// Кадров в секунду (`avg_frame_rate`; неизвестна — 30).
    pub fps: f64,
    /// Есть звуковая дорожка.
    pub audio: bool,
}

/// `ffprobe` файла. Блокирует (десятки мс) — звать в фоне.
pub fn probe(ffprobe: &Path, file: &Path) -> Result<Info, MediaError> {
    let output = command(ffprobe)
        .args(["-v", "error", "-show_entries"])
        .arg("stream=codec_type,width,height,avg_frame_rate:format=duration")
        .args(["-of", "json"])
        .arg(file)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| MediaError(format!("{}: {error}", ffprobe.display())))?;
    if !output.status.success() {
        return Err(MediaError(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    parse_probe(&output.stdout).ok_or_else(|| MediaError("в файле нет видео".into()))
}

fn parse_probe(json: &[u8]) -> Option<Info> {
    let root: Value = serde_json::from_slice(json).ok()?;
    let streams = root.get("streams")?.as_array()?;
    fn kind(stream: &Value) -> Option<&str> {
        stream.get("codec_type").and_then(Value::as_str)
    }
    let video = streams
        .iter()
        .find(|stream| kind(stream) == Some("video"))?;
    let side = |key: &str| {
        video
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|side| u32::try_from(side).ok())
            .filter(|&side| side > 0)
    };
    let fps = video
        .get("avg_frame_rate")
        .and_then(Value::as_str)
        .and_then(|rate| rate.split_once('/'))
        .and_then(|(num, den)| Some(num.parse::<f64>().ok()? / den.parse::<f64>().ok()?))
        .filter(|fps| fps.is_finite() && *fps > 0.)
        .unwrap_or(30.);
    let duration = root
        .get("format")?
        .get("duration")?
        .as_str()?
        .parse::<f64>()
        .ok()
        .filter(|duration| duration.is_finite() && *duration > 0.)?;
    Some(Info {
        duration,
        width: side("width")?,
        height: side("height")?,
        fps,
        audio: streams.iter().any(|stream| kind(stream) == Some("audio")),
    })
}

/// Размер кадра в рамке `size` (физические px): целиком, с пропорциями (`object-fit: contain`),
/// но не больше исходного.
pub fn fit(info: &Info, (width, height): (u32, u32)) -> (u32, u32) {
    let (w, h) = (f64::from(info.width), f64::from(info.height));
    let scale = (f64::from(width) / w).min(f64::from(height) / h).min(1.);
    (
        ((w * scale).round() as u32).max(1),
        ((h * scale).round() as u32).max(1),
    )
}

fn args(file: &Path, at: f64, speed: f64, fps: f64, (width, height): (u32, u32)) -> Vec<String> {
    let mut filter = String::new();
    if speed != 1. {
        filter.push_str(&format!("setpts=PTS/{speed},"));
    }
    filter.push_str(&format!("fps={fps},scale={width}:{height}"));
    // Потоков декодера 4, не по числу ядер: у 1080p60 ~100 МБ вместо ~200, запас скорости ×6.
    let mut args: Vec<String> = ["-v", "error", "-nostdin", "-threads", "4", "-ss"]
        .map(String::from)
        .into();
    args.extend([
        format!("{at:.6}"),
        "-i".into(),
        file.to_string_lossy().into(),
    ]);
    args.extend(["-an", "-sn", "-dn", "-vf"].map(String::from));
    args.push(filter);
    args.extend(["-f", "rawvideo", "-pix_fmt", "bgra", "pipe:1"].map(String::from));
    args
}

/// Один запуск FFmpeg с кадрами: кадр `k` — время видео `start + k × step`.
struct Frames {
    /// Есть до `drop` (гасится там).
    child: Option<Child>,
    queue: Receiver<Vec<u8>>,
    size: (u32, u32),
    start: f64,
    step: f64,
    /// Номер следующего кадра.
    next: u64,
    /// Пришёл, но часы до него не дошли.
    pending: Option<Vec<u8>>,
    /// Труба кончилась.
    done: bool,
    /// Процесс вышел (код проверен).
    exited: bool,
    /// Когда FFmpeg последний раз подал знак: запуск, кадр, конец трубы.
    progress: Instant,
}

impl Frames {
    fn spawn(
        ffmpeg: &Path,
        file: &Path,
        at: f64,
        speed: f64,
        fps: f64,
        size: (u32, u32),
    ) -> Result<Frames, MediaError> {
        let mut child = spawn(ffmpeg, &args(file, at, speed, fps, size))?;
        let mut out = child.stdout.take().expect("stdout в трубе");
        let (sender, queue) = sync_channel(QUEUE);
        let len = size.0 as usize * size.1 as usize * 4;
        let reader = std::thread::Builder::new()
            .name("hlsr-video".into())
            .spawn(move || {
                loop {
                    let mut frame = vec![0u8; len];
                    if out.read_exact(&mut frame).is_err() || sender.send(frame).is_err() {
                        break;
                    }
                }
            });
        if let Err(error) = reader {
            stop(child);
            return Err(MediaError(format!("поток чтения кадров: {error}")));
        }
        Ok(Frames {
            child: Some(child),
            queue,
            size,
            start: at,
            step: speed / fps,
            next: 0,
            pending: None,
            done: false,
            exited: false,
            progress: Instant::now(),
        })
    }

    /// Последний кадр, до которого дошли часы `now`; более ранние пропускаются.
    fn due(&mut self, now: f64) -> Option<Vec<u8>> {
        let mut due = None;
        loop {
            if self.pending.is_none() {
                match self.queue.try_recv() {
                    Ok(frame) => (self.pending, self.progress) = (Some(frame), Instant::now()),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        (self.done, self.progress) = (true, Instant::now());
                        break;
                    }
                }
            }
            if self.start + self.next as f64 * self.step > now + 1e-6 {
                break;
            }
            due = self.pending.take();
            self.next += 1;
        }
        due
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            stop(child);
        }
    }
}

/// Плеер видео: вид держит его полем, в `render` зовёт [`Video::frame`]; когда вид закрыт —
/// [`Video::release`] (картинку из атласа), процессы гасит `Drop`.
pub struct Video {
    ffmpeg: PathBuf,
    file: PathBuf,
    info: Info,
    speed: f64,
    volume: f32,
    playing: bool,
    ended: bool,
    /// Часы без звука: время видео `at`, с `since` идут со скоростью `speed` (на паузе — нет).
    at: f64,
    since: Option<Instant>,
    /// Нужный размер кадра и с какого момента он такой.
    want: Option<((u32, u32), Instant)>,
    frames: Option<Frames>,
    audio: Option<Audio>,
    raster: Raster,
    error: Option<MediaError>,
    /// Файл отпущен ([`Video::park`]): FFmpeg не запущен до `play` или `seek`.
    parked: bool,
}

impl Video {
    /// Плеер на паузе в начале. Кадры пойдут с первой [`Video::frame`] (нужен размер рамки).
    /// Блокирует (устройство звука, запуск FFmpeg) — строить в фоне.
    pub fn new(ffmpeg: PathBuf, file: PathBuf, info: Info) -> Video {
        let audio = info.audio.then(|| Audio::open(1.)).and_then(|audio| {
            audio
                .map_err(|error| tracing::warn!("[media] без звука: {}", error.0))
                .ok()
        });
        let mut video = Video {
            ffmpeg,
            file,
            info,
            speed: 1.,
            volume: 1.,
            playing: false,
            ended: false,
            at: 0.,
            since: None,
            want: None,
            frames: None,
            audio,
            raster: Raster::default(),
            error: None,
            parked: false,
        };
        video.load_audio(0.);
        video
    }

    pub fn info(&self) -> &Info {
        &self.info
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    pub fn ended(&self) -> bool {
        self.ended
    }

    pub fn speed(&self) -> f64 {
        self.speed
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    /// FFmpeg не запустился или файл не декодируется (`@error` плеера).
    pub fn error(&self) -> Option<&MediaError> {
        self.error.as_ref()
    }

    /// Текущее время видео (`currentTime`).
    pub fn position(&self) -> f64 {
        self.clock(Instant::now())
    }

    fn clock(&self, now: Instant) -> f64 {
        let heard = if self.playing {
            self.audio.as_ref().and_then(Audio::time)
        } else {
            None
        };
        let t = heard.unwrap_or_else(|| {
            let run = self.since.map_or(0., |since| {
                now.saturating_duration_since(since).as_secs_f64() * self.speed
            });
            self.at + run
        });
        t.clamp(0., self.info.duration)
    }

    pub fn play(&mut self) {
        if self.playing || self.error.is_some() {
            return;
        }
        // Как `video.play()` после конца — с начала.
        if self.ended {
            self.seek(0.);
        } else if self.parked {
            self.seek(self.at);
        }
        self.playing = true;
        self.since = Some(Instant::now());
        if let Some(audio) = &self.audio {
            audio.play();
        }
    }

    pub fn pause(&mut self) {
        if !self.playing {
            return;
        }
        self.at = self.clock(Instant::now());
        self.since = None;
        self.playing = false;
        if let Some(audio) = &self.audio {
            audio.pause();
        }
    }

    /// Перемотка (`currentTime = t`): FFmpeg заново с `-ss`; до нового кадра виден прежний.
    pub fn seek(&mut self, t: f64) {
        let t = t.clamp(0., self.info.duration);
        self.at = t;
        self.since = self.playing.then(Instant::now);
        self.ended = false;
        self.parked = false;
        self.frames = None;
        if self.error.is_none()
            && let Some((size, _)) = self.want
        {
            self.start_frames(t, size);
        }
        self.load_audio(t);
    }

    /// Скорость (`playbackRate`): тон звука сохраняется (`atempo`).
    pub fn set_speed(&mut self, speed: f64) {
        if speed == self.speed || !(speed.is_finite() && speed > 0.) {
            return;
        }
        let t = self.clock(Instant::now());
        self.speed = speed;
        // Отпущенный файл не открывается: новую скорость возьмёт `play` или `seek`.
        if !self.ended && !self.parked {
            self.seek(t);
        }
    }

    /// Громкость 0…1 (`volume`; без звука — 0).
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0., 1.);
        if let Some(audio) = &self.audio {
            audio.set_volume(self.volume);
        }
    }

    /// Отпустить файл (уход со страницы, видео — в Корзину): пауза, процессы FFmpeg гасятся,
    /// последний кадр остаётся на экране. `play` и `seek` запускают FFmpeg заново.
    pub fn park(&mut self) {
        self.pause();
        self.parked = true;
        self.frames = None;
        if let Some(audio) = &mut self.audio {
            audio.unload();
        }
    }

    fn start_frames(&mut self, t: f64, size: (u32, u32)) {
        let frames = Frames::spawn(&self.ffmpeg, &self.file, t, self.speed, self.info.fps, size);
        match frames {
            Ok(frames) => self.frames = Some(frames),
            Err(error) => self.fail(error),
        }
    }

    fn load_audio(&mut self, t: f64) {
        let Some(audio) = &mut self.audio else {
            return;
        };
        if let Err(error) = audio.load(&self.ffmpeg, &self.file, t, self.speed) {
            tracing::warn!("[media] без звука: {}", error.0);
            self.audio = None;
        }
    }

    fn fail(&mut self, error: MediaError) {
        tracing::warn!("[media] видео: {}", error.0);
        self.pause();
        self.error = Some(error);
        self.frames = None;
    }

    /// Шаг часов для рамки `size` (физические px): новый кадр (BGRA, его размер), до которого
    /// они дошли, и нужен ли следующий кадр окна (идёт воспроизведение, ждём кадр после
    /// перемотки или новый размер).
    pub fn advance(&mut self, size: (u32, u32)) -> (Option<Bgra>, bool) {
        if self.parked {
            return (None, false);
        }
        let now = Instant::now();
        let mut again = false;
        let want = fit(&self.info, size);
        if self.error.is_none() && self.frames.as_ref().map(|frames| frames.size) != Some(want) {
            let since = match self.want {
                Some((size, since)) if size == want => since,
                _ => now,
            };
            self.want = Some((want, since));
            if self.frames.is_none() || now.duration_since(since) >= RESIZE_SETTLE {
                self.start_frames(self.clock(now), want);
            } else {
                again = true;
            }
        }
        let t = self.clock(now);
        if self.playing {
            (self.at, self.since) = (t, Some(now));
        }
        let mut frame = None;
        let mut failed = None;
        if let Some(frames) = &mut self.frames {
            frame = frames.due(t).map(|bytes| (frames.size, bytes));
            // Ждём FFmpeg: первый кадр или выход после конца трубы.
            let mut waiting = frames.next == 0 && !frames.done;
            if frames.done && frames.pending.is_none() && !frames.exited {
                match frames.child.as_mut().map_or(Ok(None), Child::try_wait) {
                    Ok(None) => waiting = true,
                    Ok(Some(status)) => {
                        frames.exited = true;
                        if !status.success() {
                            failed = Some(MediaError(format!("FFmpeg вышел: {status}")));
                        }
                    }
                    Err(_) => frames.exited = true,
                }
            }
            if waiting && frames.progress.elapsed() >= STALL {
                failed = Some(MediaError(format!(
                    "FFmpeg не отвечает {} с",
                    STALL.as_secs()
                )));
            } else if waiting {
                again = true;
            }
        }
        if let Some(error) = failed {
            self.fail(error);
        }
        let finished = self.frames.as_ref().is_some_and(|frames| frames.exited);
        let heard = self.audio.as_ref().and_then(Audio::time);
        if self.playing && finished && heard.is_none() {
            self.pause();
            (self.ended, self.at) = (true, self.info.duration);
        }
        (frame, again || self.playing)
    }

    /// Кадр для отрисовки в рамке `size` (физические px, [`crate::image::device_size`]); пока
    /// видео идёт или ждёт кадр — просит следующий кадр окна. Прежние кадры уходят из атласа.
    pub fn frame(
        &mut self,
        size: (u32, u32),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Arc<RenderImage>> {
        let (frame, again) = self.advance(size);
        if let Some(((width, height), bytes)) = frame
            && let Some(buffer) = ::image::RgbaImage::from_raw(width, height, bytes)
        {
            // Байты уже BGRA, как ждёт GPUI.
            let image = RenderImage::new([::image::Frame::new(buffer)]);
            self.raster.set_render(Arc::new(image));
        }
        if again {
            crate::motion::monitor_frame(window);
        }
        self.raster.image(window, cx)
    }

    /// Вид закрыт: картинку — из атласа (звать вне отрисовки окна).
    pub fn release(&mut self, cx: &mut App) {
        self.raster.release(cx);
    }

    /// PID своих процессов FFmpeg (кадры, звук).
    pub fn pids(&self) -> Vec<u32> {
        let frames = self
            .frames
            .as_ref()
            .and_then(|frames| frames.child.as_ref())
            .map(Child::id);
        frames
            .into_iter()
            .chain(self.audio.as_ref().and_then(Audio::pid))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn probe_json_fit_and_arguments() {
        let json = br#"{"streams":[{"codec_type":"video","width":1920,"height":1080,"avg_frame_rate":"60/1"},{"codec_type":"audio","avg_frame_rate":"0/0"}],"format":{"duration":"5.000000"}}"#;
        let info = parse_probe(json).unwrap();
        let expected = Info {
            duration: 5.,
            width: 1920,
            height: 1080,
            fps: 60.,
            audio: true,
        };
        assert_eq!(info, expected);
        // Частота неизвестна — 30; без видео — нет.
        let unknown = br#"{"streams":[{"codec_type":"video","width":4,"height":2,"avg_frame_rate":"0/0"}],"format":{"duration":"1"}}"#;
        assert_eq!(parse_probe(unknown).unwrap().fps, 30.);
        let sound = br#"{"streams":[{"codec_type":"audio"}],"format":{"duration":"1"}}"#;
        assert_eq!(parse_probe(sound), None);
        assert_eq!(fit(&info, (1200, 1200)), (1200, 675));
        assert_eq!(fit(&info, (4000, 3000)), (1920, 1080));
        assert_eq!(fit(&info, (100, 50)), (89, 50));
        assert_eq!(
            args(Path::new("v.mp4"), 1.5, 2., 60., (640, 360)).join(" "),
            "-v error -nostdin -threads 4 -ss 1.500000 -i v.mp4 -an -sn -dn -vf setpts=PTS/2,fps=60,scale=640:360 -f rawvideo -pix_fmt bgra pipe:1"
        );
    }

    /// FFmpeg для проверки: `HLSR_DEMO_FFMPEG` или из PATH, ffprobe рядом.
    fn tools() -> Option<(PathBuf, PathBuf)> {
        let ffmpeg = std::env::var_os("HLSR_DEMO_FFMPEG").map_or("ffmpeg".into(), PathBuf::from);
        let ffprobe = ffmpeg.with_file_name(format!("ffprobe{}", std::env::consts::EXE_SUFFIX));
        let works = |exe: &Path| {
            command(exe)
                .arg("-version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        };
        (works(&ffmpeg) && works(&ffprobe)).then_some((ffmpeg, ffprobe))
    }

    /// Образец: `ffmpeg -ss t -frames:v 1` в размере `size`.
    fn reference(ffmpeg: &Path, file: &Path, t: f64, (w, h): (u32, u32)) -> Vec<u8> {
        let output = command(ffmpeg)
            .args(["-v", "error", "-ss", &t.to_string(), "-i"])
            .arg(file)
            .args(["-frames:v", "1", "-vf", &format!("scale={w}:{h}")])
            .args(["-f", "rawvideo", "-pix_fmt", "bgra", "pipe:1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(output.stdout.len(), w as usize * h as usize * 4);
        output.stdout
    }

    fn alive(pid: u32) -> bool {
        #[cfg(windows)]
        {
            let list = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&list.stdout).contains(&format!("\"{pid}\""))
        }
        #[cfg(not(windows))]
        {
            Path::new(&format!("/proc/{pid}")).exists()
        }
    }

    /// Процессы вышли (выхода дожидаются в фоне — не дольше 2 с).
    fn gone(pids: &[u32]) -> bool {
        let begun = Instant::now();
        while pids.iter().any(|&pid| alive(pid)) {
            if begun.elapsed() > Duration::from_secs(2) {
                return false;
            }
            sleep(Duration::from_millis(20));
        }
        true
    }

    /// Шаги движка, пока не придёт кадр (не дольше 3 с).
    fn next_frame(video: &mut Video, size: (u32, u32)) -> Vec<u8> {
        let begun = Instant::now();
        while begun.elapsed() < Duration::from_secs(3) {
            if let (Some((_, frame)), _) = video.advance(size) {
                return frame;
            }
            sleep(Duration::from_millis(4));
        }
        panic!("кадра нет 3 с");
    }

    /// Клип в своей папке `name`: 5 с 320×180, 25 к/с, ключевой кадр раз в секунду; звук AAC —
    /// тишина.
    fn clip(ffmpeg: &Path, ffprobe: &Path, name: &str) -> (PathBuf, PathBuf, Info) {
        let dir = std::env::temp_dir().join(format!("hlsr-media-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("clip.mp4");
        let made = command(ffmpeg)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg("testsrc2=size=320x180:rate=25:duration=5")
            .args(["-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo", "-t", "5"])
            .args([
                "-c:v", "libx264", "-pix_fmt", "yuv420p", "-g", "25", "-c:a", "aac",
            ])
            .arg(&file)
            .status()
            .unwrap();
        assert!(made.success());
        let info = probe(ffprobe, &file).unwrap();
        assert!((info.duration - 5.).abs() < 0.1, "{info:?}");
        assert_eq!(
            (info.width, info.height, info.fps, info.audio),
            (320, 180, 25., true)
        );
        (dir, file, info)
    }

    #[test]
    fn a_5_second_mp4_seeks_to_exact_frames_plays_in_sync_and_cleans_up() {
        let Some((ffmpeg, ffprobe)) = tools() else {
            eprintln!("FFmpeg нет — проверка пропущена");
            return;
        };
        let (dir, file, info) = clip(&ffmpeg, &ffprobe, "engine");

        // Кадр N после перемотки на 1 с совпадает с `ffmpeg -ss 1.48` побайтно: на ×1 — 12-й,
        // на ×2 — 6-й, на ×0,5 — 24-й.
        let full = (320, 180);
        let target = reference(&ffmpeg, &file, 1.48, full);
        for (speed, n) in [(1., 12), (2., 6), (0.5, 24)] {
            let frames = Frames::spawn(&ffmpeg, &file, 1., speed, info.fps, full).unwrap();
            let frame = frames.queue.iter().nth(n).unwrap();
            assert!((frames.start + n as f64 * frames.step - 1.48).abs() < 1e-9);
            assert!(frame == target, "кадр {n} на ×{speed}");
        }
        // Уменьшенный под рамку кадр — как `-vf scale` образца.
        let half = fit(&info, (160, 120));
        let frames = Frames::spawn(&ffmpeg, &file, 2., 1., info.fps, half).unwrap();
        assert!(frames.queue.recv().unwrap() == reference(&ffmpeg, &file, 2., half));
        // Иначе его FFmpeg держит clip.mp4 до конца проверки и папка не удаляется.
        drop(frames);

        // Плеер: первый кадр на паузе — начало видео.
        let mut video = Video::new(ffmpeg.clone(), file.clone(), info.clone());
        video.set_volume(0.);
        assert!(next_frame(&mut video, full) == reference(&ffmpeg, &file, 0., full));

        // Секунда воспроизведения: часы (по звуку, если есть устройство) идут вровень.
        video.play();
        let (begun, mut shown) = (Instant::now(), 0);
        while begun.elapsed() < Duration::from_secs(1) {
            shown += usize::from(video.advance(full).0.is_some());
            sleep(Duration::from_millis(4));
        }
        let position = video.position();
        eprintln!("за 1 с: время видео {position:.3} с, кадров {shown}");
        assert!((position - 1.).abs() < 0.2, "{position}");
        assert!(shown >= 20, "{shown}");

        // Пауза и перемотка на 3 с: виден ровно этот кадр, прежние процессы погашены.
        video.pause();
        let old = video.pids();
        video.seek(3.);
        assert!(next_frame(&mut video, full) == reference(&ffmpeg, &file, 3., full));
        assert_eq!(video.position(), 3.);
        assert!(gone(&old), "{old:?}");

        // ×2 с 4 с до конца (за сколько — в `timings_…`).
        let took = to_the_end_at_double_speed(&mut video);
        eprintln!("секунда видео на ×2: {took:?}");
        assert!(video.ended() && !video.playing());
        assert_eq!(video.position(), info.duration);

        // Файл отпущен: процессы погашены, новых нет, пока не нажали «Играть»; потом — с того же места.
        video.seek(2.);
        next_frame(&mut video, full);
        let pids = video.pids();
        video.park();
        assert!(video.pids().is_empty() && gone(&pids));
        assert_eq!(video.advance(full), (None, false));
        // Скорость на отпущенном файле его не открывает.
        video.set_speed(1.);
        assert!(video.pids().is_empty());
        assert!(video.pids().is_empty() && video.position() == 2.);
        video.play();
        video.pause();
        assert!(!video.pids().is_empty());
        assert!(next_frame(&mut video, full) == reference(&ffmpeg, &file, 2., full));

        // Закрытие посреди видео: свои процессы FFmpeg гаснут.
        video.seek(1.);
        next_frame(&mut video, full);
        let pids = video.pids();
        assert!(
            !pids.is_empty() && pids.iter().all(|&pid| alive(pid)),
            "{pids:?}"
        );
        drop(video);
        assert!(gone(&pids), "{pids:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// ×2 с 4 с: играть до конца (не дольше 3 с), сколько заняло.
    fn to_the_end_at_double_speed(video: &mut Video) -> Duration {
        video.set_speed(2.);
        video.seek(4.);
        video.play();
        let begun = Instant::now();
        while !video.ended() && begun.elapsed() < Duration::from_secs(3) {
            video.advance((320, 180));
            sleep(Duration::from_millis(4));
        }
        begun.elapsed()
    }

    /// Тайминги на свободной машине (под нагрузкой плавают):
    /// `cargo test -p hlsr-gpui media::video -- --ignored`.
    #[test]
    #[ignore = "тайминги — на свободной машине"]
    fn timings_seek_within_300_ms_and_a_second_at_double_speed_within_900_ms() {
        let Some((ffmpeg, ffprobe)) = tools() else {
            eprintln!("FFmpeg нет — проверка пропущена");
            return;
        };
        let (dir, file, info) = clip(&ffmpeg, &ffprobe, "timings");
        // Перемотка: от запуска FFmpeg до первого кадра.
        let mut worst = Duration::ZERO;
        for at in [0.7, 2.3, 3.9, 4.6] {
            let begun = Instant::now();
            let frames = Frames::spawn(&ffmpeg, &file, at, 1., info.fps, (320, 180)).unwrap();
            frames.queue.recv().unwrap();
            worst = worst.max(begun.elapsed());
        }
        eprintln!("перемотка до первого кадра: не дольше {worst:?}");
        assert!(worst <= Duration::from_millis(300), "{worst:?}");
        // Секунда видео на ×2 — около 0,5 с.
        let mut video = Video::new(ffmpeg, file, info);
        video.set_volume(0.);
        next_frame(&mut video, (320, 180));
        let took = to_the_end_at_double_speed(&mut video);
        eprintln!("секунда видео на ×2: {took:?}");
        assert!(
            video.ended() && took < Duration::from_millis(900),
            "{took:?}"
        );
        drop(video);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_silent_ffmpeg_is_an_error_not_an_endless_redraw() {
        let Some((ffmpeg, _)) = tools() else {
            eprintln!("FFmpeg нет — проверка пропущена");
            return;
        };
        // Сервер на петле принимает и молчит: FFmpeg ждёт данных вечно.
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let file = PathBuf::from(format!("tcp://{}", server.local_addr().unwrap()));
        let info = Info {
            duration: 5.,
            width: 320,
            height: 180,
            fps: 25.,
            audio: false,
        };
        let mut video = Video::new(ffmpeg, file, info);
        assert!(video.advance((320, 180)).1 && video.error().is_none());
        let pids = video.pids();
        // Знака нет дольше `STALL`.
        video.frames.as_mut().unwrap().progress = Instant::now().checked_sub(STALL).unwrap();
        let (frame, again) = video.advance((320, 180));
        assert!(frame.is_none() && !again);
        assert_eq!(
            video.error().map(|error| error.0.as_str()),
            Some("FFmpeg не отвечает 10 с")
        );
        assert!(!pids.is_empty() && gone(&pids), "{pids:?}");
    }
}
