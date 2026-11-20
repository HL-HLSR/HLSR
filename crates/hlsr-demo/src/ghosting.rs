//! Адаптер рендера Ghosting Mod: Renewed (Source 2013, мод `ghostingmod`), `ghosting.js` 0.4.9 в
//! режиме мода `-ghrender` ([`crate::ghrender`]): игра на скрытом рабочем столе
//! ([`crate::render_host`]) одним запуском проигрывает плейлист игрока — весь или записи,
//! которые задевает интервал (`-ghrender_segments`); кадры `f_NNNN.tga` (появляются атомарно)
//! идут в FFmpeg (промежуточный H.264 без звука) и сразу удаляются, `audio.wav` — после `DONE`,
//! без сдвига; таймлайн и вырезанное — из журнала `ghrender.log`. Интервал и скорость режет
//! общий слой ([`crate::jobs`]).
//!
//! Строже 0.4.9 (решение владельца 5): старого пути (`+startdemos` пачками, сборки < 1.0.13)
//! нет — он не проверяет команды демки. Нужен Ghosting 1.0.13 или новее: без `-ghrender` в
//! `client.dll` — `GHRENDER_UNSUPPORTED`, мод не ответил журналом — тоже, без отката. Одиночная
//! демка и плейлист не из `demos/` мода — `RENDER_NOT_READY`, переименованная папка рана —
//! `PLAYLIST_PATH_ESCAPE`.
//!
//! Что адаптер возвращает игроку как было, — общее с HL1 ([`crate::game_render`]): реестр
//! `HKCU\Software\Valve\Source\ghostingmod\Settings`, mute `hl2.exe`, папка рендера в моде;
//! `cfg/config.cfg` и `console.log` игрока не трогаются: игра закрывается только заданием
//! помощника, журнал консоли (`+con_logfile`) — в папке задания.

use crate::errors::DemoError;
use crate::game_render::{self, Active, FPS, GameRender, Launch, Ports, Spec};
use crate::ghrender::{self, Fields, GameArgs, Line};
use crate::jobs::{Adapter, Material, RenderProgress, RenderRequest, SegmentProgress};
use crate::library::{Described, Segment};
use crate::planner::Source;
use crate::source_render::{self, Picked, TgaInfo, command_line, plan_segments};
use hlsr_core::js::latin1_string;
use hlsr_core::js::math_round;
use hlsr_core::js::number::number_to_string;
use hlsr_core::json_text::js_number;
use hlsr_delivery::files_manifest::hash_file_sync;
use hlsr_delivery::ports::FreeSpace;
use serde_json::{Map, json};
use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::BuildHasher;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

pub const GAME_ID: &str = "ghosting-renewed";
pub const MOD: &str = "ghostingmod";
pub const EXE: &str = "hl2.exe";
pub const RENDER_DIR: &str = "hlsr-render";
pub const REGISTRY_KEY: &str = "HKCU\\Software\\Valve\\Source\\ghostingmod\\Settings";
pub const MARKER: &str = "ghosting-render.restore.json";
pub const REG_FILE: &str = "ghosting-render.settings.reg";
pub const UNMUTE_PENDING: &str = "ghostingUnmutePending";
/// Профиль оценки времени ([`crate::estimate::profile`]).
pub const PROFILE: &str = "ghosting-ghrender";

/// Что у Ghosting своё (`SPEC`). `integrity` — «GH DEMO DATA: INTACT» по выбору игрока;
/// `ownConfig`, `ownHud` — стандартный конфиг или HUD ([`source_render::config_arguments`]).
pub const SPEC: Spec = Spec {
    game_id: GAME_ID,
    image: EXE,
    registry_key: REGISTRY_KEY,
    marker: MARKER,
    reg_file: REG_FILE,
    unmute_pending: UNMUTE_PENDING,
    label: "Ghosting",
    other_running: None,
    required: &[EXE, MOD],
    render_root,
    caps: &["keyhud", "audio", "integrity", "ownConfig", "ownHud"],
};

fn render_root(install: &Path) -> PathBuf {
    install.join(MOD).join(RENDER_DIR)
}

/// Первая строка журнала приходит через ~2,5 с после запуска игры; мод без режима молча висит
/// в меню.
const FIRST_LINE: Duration = Duration::from_secs(45);
/// Застой: дольше собственного таймаута мода на открытие демки (60 с, `FAIL DEMO_FAILED`).
const STALL: Duration = Duration::from_secs(75);
const MAX_JOURNAL_LINE: usize = 4096;
/// После `DONE` игра сама выходит через 1 с; дольше — закрывает задание.
const EXIT_WAIT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(10);
const RETRY: Duration = Duration::from_millis(25);
const REPORT: Duration = Duration::from_millis(200);
/// Оценка промежуточного H.264 (ultrafast, CRF 14) для проверки места.
const INTERMEDIATE_BITS_PER_PIXEL: f64 = 0.35;
const RESERVE_BYTES: f64 = 512.0 * 1024.0 * 1024.0;
const STDERR_TAIL: usize = 4000;

fn with_message(code: &str, message: impl Into<String>) -> DemoError {
    DemoError {
        message: message.into(),
        ..DemoError::new(code)
    }
}

fn frame_name(dir: &Path, index: u32) -> PathBuf {
    dir.join(format!("f_{index:04}.tga"))
}

/// Восемь случайных шестнадцатеричных знаков.
fn random_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{:08x}", RandomState::new().hash_one(nanos) as u32)
}

/// `jobId8(jobDir)`: папка задания в моде — первые 8 знаков id задания.
pub(crate) fn job_id8(job_dir: &Path) -> String {
    let name = job_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let id: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(8)
        .collect::<String>()
        .to_ascii_lowercase();
    if id.is_empty() { random_id() } else { id }
}

/// `resetArguments(logFile)`: сброс mute после закрытия заглушённой игры (помощник коротко
/// перезапустит её и снимет глушение); в самой игре звука нет.
fn reset_arguments(log_file: &str) -> Vec<String> {
    [
        "-steam",
        "-game",
        MOD,
        "-windowed",
        "-noborder",
        "-w",
        "640",
        "-h",
        "480",
        "-novid",
        "-multirun",
        "+con_logfile",
        log_file,
        "+developer",
        "1",
        "+volume",
        "0",
        "+snd_mute_losefocus",
        "0",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// `configFor(request, install, modJobDir)`: cfg стандартного конфига или HUD в папке задания в
/// моде (`std`), аргументы игры для них.
fn config_for(
    request: &RenderRequest,
    mod_job_dir: &Path,
    id8: &str,
) -> Result<Vec<String>, DemoError> {
    let dir = format!("{MOD}/{RENDER_DIR}/{id8}/std");
    let plan = source_render::config_arguments(request.own_config, request.own_hud, &dir);
    let std = mod_job_dir.join("std");
    for (name, text) in &plan.files {
        let file = name.split('/').fold(std.clone(), |p, part| p.join(part));
        let write = || -> io::Result<()> {
            fs::create_dir_all(file.parent().unwrap_or(&std))?;
            fs::write(&file, text)
        };
        write().map_err(|error| with_message("JOB_FAILED", error.to_string()))?;
    }
    Ok(plan.args)
}

/// `moveFile(from, to)`: игра и папка заданий бывают на разных дисках.
fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    let mut source = fs::File::open(from)?;
    let mut target = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)?;
    io::copy(&mut source, &mut target)?;
    drop(source);
    let _ = fs::remove_file(from);
    Ok(())
}

/// Корень пути (`path.parse(path.resolve(p)).root`) без учёта регистра.
pub(crate) fn root_of(path: &Path) -> String {
    let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    full.components()
        .take_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

// ---------------------------------------------------------------- FFmpeg

/// Промежуточный H.264 из кадров игры: FFmpeg читает кадры из stdin.
pub trait Encoder: Send {
    /// Ещё один кадр (пиксели TGA). Ошибка — `FFMPEG_FAILED`.
    fn write(&mut self, pixels: &[u8]) -> Result<(), DemoError>;
    /// Конец кадров: дождаться FFmpeg; код не 0 — `FFMPEG_FAILED` (`exitCode`).
    fn finish(self: Box<Self>) -> Result<(), DemoError>;
}

/// Запуск FFmpeg с аргументами (подмена в тестах).
pub type SpawnEncoder =
    dyn Fn(&Path, &[String]) -> Result<Box<dyn Encoder>, DemoError> + Send + Sync;

/// Свой процесс FFmpeg: гасится по своему дескриптору, если кадры не дописаны.
struct FfmpegPump {
    child: Child,
    stdin: Option<ChildStdin>,
    stderr: Option<JoinHandle<String>>,
}

/// Хвост `stderr.trim().slice(-500)`.
fn stderr_tail(text: &str) -> String {
    let text = text.trim();
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(500)).collect()
}

impl FfmpegPump {
    fn spawn(exe: &Path, args: &[String]) -> Result<Box<dyn Encoder>, DemoError> {
        let mut command = Command::new(exe);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = command.spawn().map_err(|error| {
            let code = if error.kind() == io::ErrorKind::NotFound {
                "FFMPEG_MISSING"
            } else {
                "FFMPEG_START_FAILED"
            };
            with_message(code, error.to_string())
        })?;
        let mut stderr = child.stderr.take().expect("stderr в трубе");
        let reader = thread::spawn(move || {
            let mut text = String::new();
            let mut buffer = [0u8; 4096];
            while let Ok(read) = stderr.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                text.push_str(&String::from_utf8_lossy(&buffer[..read]));
                if text.len() > STDERR_TAIL * 2 {
                    let cut = text.len() - STDERR_TAIL;
                    let at = (cut..text.len())
                        .find(|at| text.is_char_boundary(*at))
                        .unwrap_or(cut);
                    text.drain(..at);
                }
            }
            text
        });
        Ok(Box::new(FfmpegPump {
            stdin: child.stdin.take(),
            child,
            stderr: Some(reader),
        }))
    }

    fn stderr_text(&mut self) -> String {
        self.stderr
            .take()
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default()
    }
}

impl Encoder for FfmpegPump {
    fn write(&mut self, pixels: &[u8]) -> Result<(), DemoError> {
        let written = self
            .stdin
            .as_mut()
            .map_or(Err(io::ErrorKind::BrokenPipe.into()), |stdin| {
                stdin.write_all(pixels)
            });
        if written.is_ok() {
            return Ok(());
        }
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
        Err(with_message(
            "FFMPEG_FAILED",
            stderr_tail(&self.stderr_text()),
        ))
    }

    fn finish(mut self: Box<Self>) -> Result<(), DemoError> {
        self.stdin = None;
        let status = self.child.wait();
        let stderr = self.stderr_text();
        let code = status.ok().and_then(|status| status.code());
        if code == Some(0) {
            return Ok(());
        }
        Err(DemoError {
            message: stderr_tail(&stderr),
            ..DemoError::new("FFMPEG_FAILED").with_details(json!({"exitCode": code}))
        })
    }
}

impl Drop for FfmpegPump {
    fn drop(&mut self) {
        if self.stdin.take().is_some() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Насос кадров: FFmpeg запускается по первому кадру (его размер и формат).
struct Pump<'a> {
    spawn: &'a SpawnEncoder,
    encoder: Option<Box<dyn Encoder>>,
    expected: (u32, u32),
    format: Option<(u16, u16, &'static str)>,
    written: u32,
}

impl Pump<'_> {
    fn start_args(info: &TgaInfo, fps: u32, video: &Path) -> Vec<String> {
        let mut args: Vec<String> = [
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            info.pix_fmt,
            "-s",
        ]
        .map(str::to_owned)
        .to_vec();
        args.push(format!("{}x{}", info.width, info.height));
        args.extend([
            "-framerate".into(),
            fps.to_string(),
            "-i".into(),
            "pipe:0".into(),
        ]);
        // TGA игры — снизу вверх.
        if !info.top_down {
            args.extend(["-vf".into(), "vflip".into()]);
        }
        for item in [
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "14",
            "-pix_fmt",
            "yuv420p",
            "-y",
        ] {
            args.push(item.to_owned());
        }
        args.push(video.to_string_lossy().into_owned());
        args
    }

    /// `pumpFrame`: кадр игры → FFmpeg. Окно больше рабочего стола игра обрезает (масштаб
    /// экрана, 4K на меньшем мониторе): растягивать молча не будем — понятная ошибка.
    fn frame(
        &mut self,
        buffer: &[u8],
        info: &TgaInfo,
        fps: u32,
        video: &Path,
        ffmpeg: &Path,
    ) -> Result<(), DemoError> {
        let size = (u32::from(info.width), u32::from(info.height));
        match self.format {
            None => {
                if size != self.expected {
                    return Err(DemoError {
                        message: format!("Игра дала кадр {}x{}", size.0, size.1),
                        ..DemoError::new("RESOLUTION_LIMITED")
                            .with_details(json!({"width": size.0, "height": size.1}))
                    });
                }
                self.encoder = Some((self.spawn)(ffmpeg, &Self::start_args(info, fps, video))?);
                self.format = Some((info.width, info.height, info.pix_fmt));
            }
            Some((width, height, pix_fmt)) => {
                if (width, height, pix_fmt) != (info.width, info.height, info.pix_fmt) {
                    return Err(with_message(
                        "RENDER_FAILED",
                        format!(
                            "Кадр {}x{} вместо {width}x{height}",
                            info.width, info.height
                        ),
                    ));
                }
            }
        }
        let encoder = self.encoder.as_mut().expect("FFmpeg запущен");
        encoder.write(&buffer[info.offset..info.offset + info.bytes])?;
        self.written += 1;
        Ok(())
    }
}

// ---------------------------------------------------------------- журнал

/// Журнал `-ghrender`: первая строка, `DONE`, строки `SEGMENT` и `TIMER`.
#[derive(Default)]
struct Journal {
    first: Option<String>,
    done: Option<Fields>,
    segments: Vec<Fields>,
    timers: Vec<Fields>,
}

/// Чтение `ghrender.log` с места, где остановились (строки — latin1).
struct LogReader {
    file: PathBuf,
    pos: u64,
    rest: Vec<u8>,
}

impl LogReader {
    /// Новые целые строки; `None` — файла нет или ничего не прибавилось.
    fn lines(&mut self) -> Option<Vec<Vec<u8>>> {
        let mut handle = fs::File::open(&self.file).ok()?;
        let size = handle.metadata().ok()?.len();
        if size <= self.pos {
            return None;
        }
        let mut chunk = vec![0u8; (size - self.pos).min(1024 * 1024) as usize];
        handle.seek(SeekFrom::Start(self.pos)).ok()?;
        let read = handle.read(&mut chunk).ok()?;
        self.pos += read as u64;
        self.rest.extend_from_slice(&chunk[..read]);
        let mut lines: Vec<Vec<u8>> = self
            .rest
            .split(|b| *b == b'\n')
            .map(<[u8]>::to_vec)
            .collect();
        self.rest = lines.pop().unwrap_or_default();
        // Строка без конца не копится: журнал короткими строками пишет мод, длинная — мусор.
        if self.rest.len() > MAX_JOURNAL_LINE {
            self.rest.clear();
        }
        Some(lines)
    }
}

// ---------------------------------------------------------------- адаптер

/// Профиль для оценки времени ([`crate::estimate::Input`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EstimateProfile {
    pub key: &'static str,
    pub launches: f64,
    pub segments: f64,
}

/// `GhostingAdapter`.
pub struct GhostingAdapter {
    game: GameRender,
    free_space: Arc<dyn FreeSpace>,
    spawn: Box<SpawnEncoder>,
    first_line: Duration,
    stall: Duration,
}

/// Что видит цикл журнала.
struct Watch<'a> {
    mod_job_dir: &'a Path,
    video: &'a Path,
    frames_total: f64,
    count: usize,
    started: Instant,
}

impl GhostingAdapter {
    pub fn new(ports: Ports, free_space: Arc<dyn FreeSpace>) -> GhostingAdapter {
        GhostingAdapter {
            game: GameRender::new(SPEC, ports),
            free_space,
            spawn: Box::new(FfmpegPump::spawn),
            first_line: FIRST_LINE,
            stall: STALL,
        }
    }

    /// Подмена FFmpeg (тесты).
    pub fn with_encoder(mut self, spawn: Box<SpawnEncoder>) -> GhostingAdapter {
        self.spawn = spawn;
        self
    }

    /// Подмена ожидания первой строки журнала и застоя (тесты).
    pub fn with_timeouts(mut self, first_line: Duration, stall: Duration) -> GhostingAdapter {
        self.first_line = first_line;
        self.stall = stall;
        self
    }

    /// Общее рендера игр: восстановление после сбоя, сброс mute, возможности, выход лаунчера.
    pub fn game(&self) -> &GameRender {
        &self.game
    }

    /// `resetLaunch(install, active)`: беззвучный сброс mute без рендера; консоль — в своей папке
    /// в моде.
    pub fn reset_launch(install: &Path, active: &mut Active) -> Result<Launch, DemoError> {
        let id = random_id();
        active.mod_job_dir = Some(game_render::make_render_dir(&render_root(install), &id)?);
        let exe = install.join(EXE);
        let args = reset_arguments(&format!("{RENDER_DIR}/{id}/reset.log"));
        Ok(Launch {
            command_line: command_line(&exe.to_string_lossy(), &args),
            exe,
            cwd: install.to_path_buf(),
        })
    }

    /// `resetMute()`: mute после рендера снимается без экспорта. Ответ — снят.
    pub fn reset_mute(&self) -> bool {
        self.game.reset_mute(&Self::reset_launch)
    }

    /// `estimateProfile(item, range, fps)`: один запуск игры; без `-ghrender` рендера нет.
    pub fn estimate_profile(
        &self,
        item: &Described,
        range: Option<source_render::Range>,
        fps: f64,
    ) -> EstimateProfile {
        EstimateProfile {
            key: PROFILE,
            launches: 1.0,
            segments: plan_segments(&item.segments, range, fps).picked.len() as f64,
        }
    }

    /// `canRender(item)`: `Err` — код отказа. Строже 0.4.9: без `-ghrender` не рендерим.
    pub fn can_render(&self, item: &Described) -> Result<(), DemoError> {
        if item.engine != Some("source") || item.game != Some("ghosting") {
            return Err(DemoError::new("DEMO_UNSUPPORTED"));
        }
        if !cfg!(windows) {
            return Err(DemoError::new("RENDER_NOT_READY"));
        }
        let install = self
            .game
            .install_path()
            .ok_or_else(|| DemoError::new("GAME_NOT_INSTALLED"))?;
        if !item.segments.iter().any(|segment| segment.ticks() > 1) {
            return Err(DemoError::new("DEMO_EMPTY"));
        }
        self.ghrender_playlist(&install, item, true).map(drop)
    }

    /// `ghrenderPlaylist(install, item, quiet)`: имя рана для `-ghrender` — `client.dll` мода
    /// знает режим и диапазон записей (протокол 2), ран — плейлист из `demos/` мода в своей
    /// папке. `quiet` — без записи в журнал (оценка и список зовут это при каждом выборе).
    pub fn ghrender_playlist(
        &self,
        install: &Path,
        item: &Described,
        quiet: bool,
    ) -> Result<String, DemoError> {
        let mod_dir = install.join(MOD);
        let refuse = |code: &str, why: &str| {
            if !quiet {
                self.game.logger().warn(&format!(
                    "[demo] Ghosting: {why} — экспорт только через -ghrender (Ghosting 1.0.13 или новее)"
                ));
            }
            Err(with_message(code, why))
        };
        let caps = match ghrender::client_caps(&mod_dir) {
            Ok(caps) => caps,
            Err(error) => {
                let code = hlsr_core::js::errno::code(&error).unwrap_or_default();
                return refuse(
                    "GHRENDER_UNSUPPORTED",
                    &format!("client.dll не читается: {code}"),
                );
            }
        };
        if !caps.ghrender || !caps.segments {
            let why = if caps.ghrender {
                "client.dll без -ghrender_segments (протокол 1)"
            } else {
                "client.dll не знает -ghrender"
            };
            return refuse("GHRENDER_UNSUPPORTED", why);
        }
        let file = match item.entry.files.first() {
            Some(file) if item.entry.kind == "playlist" => file,
            _ => return refuse("RENDER_NOT_READY", "это не плейлист рана"),
        };
        let Some(playlist) = ghrender::playlist_arg(&mod_dir, file) else {
            return refuse("RENDER_NOT_READY", "плейлист не из demos/ мода");
        };
        let entries = item.segments.iter().map(|segment: &Segment| {
            (
                segment.name.as_str(),
                segment.playlist.as_ref().map(|p| p.file.as_str()),
            )
        });
        if !ghrender::entries_in_place(&playlist, entries) {
            return refuse(
                "PLAYLIST_PATH_ESCAPE",
                "папку рана переименовали или скопировали (пути в плейлисте другие)",
            );
        }
        if !quiet {
            self.game
                .logger()
                .info(&format!("[demo] Ghosting: режим -ghrender, {playlist}"));
        }
        Ok(playlist)
    }

    /// `checkSegments(picked)`: плейлист хранит размер и SHA-256 каждого сегмента. Отмена — между
    /// сегментами: хэш всего плейлиста бывает долгим.
    fn check_segments(
        &self,
        picked: &[Picked],
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), DemoError> {
        for item in picked {
            if self.game.cancelled(cancel) {
                return Err(DemoError::new("CANCELLED"));
            }
            let segment = item.segment;
            let number = json!({"segment": item.index + 1});
            let is_file = fs::symlink_metadata(&segment.path).is_ok_and(|meta| meta.is_file());
            if !is_file {
                return Err(DemoError::new("SEGMENT_MISSING").with_details(number));
            }
            if let Some(playlist) = &segment.playlist {
                let changed = !playlist.size_matches
                    || (!playlist.sha256.is_empty()
                        && hash_file_sync(Path::new(&segment.path))
                            .map_err(|error| with_message("JOB_FAILED", error.message))?
                            != playlist.sha256);
                if changed {
                    return Err(DemoError::new("SEGMENT_CHANGED").with_details(number));
                }
            }
        }
        Ok(())
    }

    /// `checkSpace(...)`: кадры в моде (≤ 90 на диске) и два WAV на весь ран (`raw_` и
    /// `audio.wav`), промежуточное видео и звук в папке задания.
    fn check_space(
        &self,
        install: &Path,
        request: &RenderRequest,
        frames_total: f64,
    ) -> Result<(), DemoError> {
        let seconds = frames_total / f64::from(request.fps);
        let (width, height) = (f64::from(request.width), f64::from(request.height));
        let wav = (seconds * 176_400.0).ceil();
        let wav_bytes = if request.audio { wav } else { 0.0 };
        let video_bytes =
            (width * height * f64::from(request.fps) * INTERMEDIATE_BITS_PER_PIXEL * seconds / 8.0)
                .ceil();
        let game_need = width * height * 3.0 * 90.0 + wav * 2.0 + RESERVE_BYTES;
        let job_need = video_bytes * 2.0 + wav_bytes + RESERVE_BYTES;
        let free = |dir: &Path| self.free_space.free_bytes(dir).map(|bytes| bytes as f64);
        let (game_free, job_free) = (free(install), free(request.job_dir));
        let short = if root_of(install) == root_of(request.job_dir) {
            game_free.is_some_and(|free| free < game_need + job_need)
        } else {
            game_free.is_some_and(|free| free < game_need)
                || job_free.is_some_and(|free| free < job_need)
        };
        if short {
            return Err(DemoError::new("NO_SPACE"));
        }
        Ok(())
    }

    /// `renderJob(request, active)`.
    fn render_job(
        &self,
        request: &RenderRequest,
        active: &mut Active,
    ) -> Result<Material, DemoError> {
        if !cfg!(windows) {
            return Err(DemoError::new("RENDER_NOT_READY"));
        }
        let install = self
            .game
            .install_path()
            .ok_or_else(|| DemoError::new("GAME_NOT_INSTALLED"))?;
        let exe = install.join(EXE);
        if !FPS.contains(&request.fps) {
            return Err(DemoError::new("COMBO_UNSUPPORTED"));
        }
        self.game.preflight(&exe)?;
        let playlist = self.ghrender_playlist(&install, request.item, false)?;
        let fps = f64::from(request.fps);
        let plan = plan_segments(&request.item.segments, Some(request.range), fps);
        if plan.picked.is_empty() {
            return Err(DemoError::new("DEMO_EMPTY"));
        }
        self.check_segments(&plan.picked, request.cancel)?;
        self.check_space(&install, request, plan.frames_total)?;
        if self.game.cancelled(request.cancel) {
            return Err(DemoError::new("CANCELLED"));
        }
        self.game.restore_previous()?;
        self.render_ghrender(
            request,
            active,
            &install,
            &exe,
            &playlist,
            &plan.picked,
            plan.frames_total,
        )
    }

    /// `renderGhrender(...)`: одним запуском — мод проигрывает плейлист игрока сам.
    #[allow(clippy::too_many_arguments)]
    fn render_ghrender(
        &self,
        request: &RenderRequest,
        active: &mut Active,
        install: &Path,
        exe: &Path,
        playlist: &str,
        picked: &[Picked],
        frames_total: f64,
    ) -> Result<Material, DemoError> {
        let (fps, width, height) = (request.fps, request.width, request.height);
        let id8 = job_id8(request.job_dir);
        let out = format!("{RENDER_DIR}/{id8}");
        let whole = plan_segments(&request.item.segments, None, f64::from(fps))
            .picked
            .len();
        let indexes: Vec<usize> = picked.iter().map(|item| item.index).collect();
        let segments = ghrender::segments_arg(&indexes, whole);
        let video = request.job_dir.join("video.mkv");
        let mut pump = Pump {
            spawn: &*self.spawn,
            encoder: None,
            expected: (width, height),
            format: None,
            written: 0,
        };

        self.game.snapshot(active, Map::new())?;
        let mod_job_dir = game_render::make_render_dir(&render_root(install), &id8)?;
        active.mod_job_dir = Some(mod_job_dir.clone());
        let config = config_for(request, &mod_job_dir, &id8)?;
        let unmute_all = active.muted;
        let exe_text = exe.to_string_lossy();
        let args = ghrender::game_arguments(&GameArgs {
            width,
            height,
            fps,
            keyhud: request.keyhud,
            integrity: request.integrity,
            playlist,
            out: &out,
            segments: segments.as_deref(),
            config: &config,
        });
        let pid = self.game.start_host(
            active,
            &crate::render_host::StartRequest {
                exe: exe.to_path_buf(),
                cwd: install.to_path_buf(),
                command_line: command_line(&exe_text, &args),
                mute: true,
                unmute_all,
                reset_command_line: command_line(
                    &exe_text,
                    &reset_arguments(&format!("{out}/reset.log")),
                ),
                ..Default::default()
            },
            unmute_all,
        )?;
        // Таймауты — от запуска игры.
        let started = Instant::now();
        self.game.logger().info(&format!(
            "[demo] Ghosting -ghrender: PID {pid}, {width}x{height}@{fps}, записи {}",
            segments.as_deref().unwrap_or("все")
        ));
        let watch = Watch {
            mod_job_dir: &mod_job_dir,
            video: &video,
            frames_total,
            count: picked.len(),
            started,
        };
        let journal = self.watch_ghrender(request, active, &mut pump, &watch)?;
        // Игра после DONE выходит сама; не вышла — её закроет задание. Mute снимает перезапуск.
        let until = Instant::now() + EXIT_WAIT;
        while active.exit_code().is_none() && !active.host_closed() && Instant::now() < until {
            self.game.poll(active);
            thread::sleep(Duration::from_millis(50));
        }
        self.game.stop_host(active, true);
        let written = pump.written;
        let encoder = pump
            .encoder
            .take()
            .ok_or_else(|| with_message("RENDER_FAILED", "Игра не дала ни одного кадра"))?;
        encoder.finish()?;
        let done = journal.done.unwrap_or_default();
        let field = |key: &str| done.get(key).unwrap_or("undefined").to_owned();
        let logger = self.game.logger();
        let wav = mod_job_dir.join("audio.wav");
        let mut audio = None;
        if request.audio && done.get("audio") == Some("audio.wav") && wav.exists() {
            let target = request.job_dir.join("audio.wav");
            move_file(&wav, &target)
                .map_err(|error| with_message("JOB_FAILED", error.to_string()))?;
            audio = Some(target.to_string_lossy().into_owned());
        } else if request.audio {
            logger.warn(&format!(
                "[demo] Ghosting -ghrender: звука нет (audio={}) — экспорт без звука",
                field("audio")
            ));
        }
        if done.number("frames") != f64::from(written) {
            logger.warn(&format!(
                "[demo] Ghosting -ghrender: кадров {written}, в журнале {}",
                field("frames")
            ));
        }
        logger.info(&format!(
            "[demo] Ghosting -ghrender: {written} кадров, сегментов {}, data={}, {} с",
            field("segments"),
            field("data"),
            number_to_string(math_round(started.elapsed().as_secs_f64()))
        ));
        let mut t = 0.0;
        let starts: Vec<f64> = request
            .item
            .segments
            .iter()
            .map(|segment| {
                let start = t;
                t += segment.duration().filter(|d| *d > 0.0).unwrap_or(0.0);
                start
            })
            .collect();
        // Хвост после последнего кадра — до конца последней записи диапазона.
        let total = picked.last().map_or(0.0, |item| item.end);
        let timeline = ghrender::timeline_of(
            &journal.segments,
            &journal.timers,
            f64::from(written),
            f64::from(fps),
            &starts,
            total,
        );
        // Без якорей SEGMENT материал диапазона считался бы с начала рана — клип не из того места.
        if let Some(segments) = &segments
            && timeline.markers.is_empty()
        {
            return Err(with_message(
                "RENDER_FAILED",
                format!("В журнале -ghrender нет строк SEGMENT для записей {segments}"),
            ));
        }
        Ok(Material {
            audio,
            offset: Some(timeline.offset),
            cuts: timeline.cuts,
            timeline: ghrender::with_range_start(timeline.markers, Some(request.range)),
            profile: Some(PROFILE.to_owned()),
            launches: Some(1.0),
            segments: Some(picked.len() as f64),
            frames: Some(f64::from(written)),
            ..Material::new(Source::Video(video.to_string_lossy().into_owned()))
        })
    }

    /// `watchGhrender(...)`: журнал и кадры. Первая строка — [`ghrender::PROTOCOL`], конец —
    /// `DONE` и все его кадры; `FAIL` — код лаунчера.
    fn watch_ghrender(
        &self,
        request: &RenderRequest,
        active: &mut Active,
        pump: &mut Pump,
        watch: &Watch,
    ) -> Result<Journal, DemoError> {
        let mut log = LogReader {
            file: watch.mod_job_dir.join("ghrender.log"),
            pos: 0,
            rest: Vec::new(),
        };
        let mut journal = Journal::default();
        let mut failure: Option<DemoError> = None;
        let mut frame = 0u32;
        let mut segment: Option<SegmentProgress> = None;
        let mut last_activity = watch.started;
        let mut last_report: Option<Instant> = None;
        let count = watch.count;
        let mut read_log = |journal: &mut Journal,
                            failure: &mut Option<DemoError>,
                            segment: &mut Option<SegmentProgress>,
                            last_activity: &mut Instant| {
            let Some(lines) = log.lines() else {
                return;
            };
            *last_activity = Instant::now();
            for bytes in lines {
                let parsed = (bytes.len() <= MAX_JOURNAL_LINE)
                    .then(|| ghrender::parse_line(&latin1_string(&bytes)))
                    .flatten();
                let Some(line) = parsed else {
                    continue;
                };
                self.journal_line(&line, journal, failure, segment, count);
            }
        };
        let report = |force: bool,
                      written: u32,
                      segment: Option<SegmentProgress>,
                      last: &mut Option<Instant>| {
            let now = Instant::now();
            if !force && last.is_some_and(|at| now.duration_since(at) < REPORT) {
                return;
            }
            *last = Some(now);
            (request.on_progress)(&RenderProgress {
                fraction: (f64::from(written) / watch.frames_total.max(1.0)).min(0.999),
                segment,
                message: None,
            });
        };
        let ffmpeg = &request.ffmpeg.ffmpeg;
        loop {
            if self.game.cancelled(request.cancel) {
                return Err(DemoError::new("CANCELLED"));
            }
            read_log(&mut journal, &mut failure, &mut segment, &mut last_activity);
            if let Some(error) = failure.take() {
                return Err(error);
            }
            self.game.poll(active);
            let mut consumed = false;
            while frame_name(watch.mod_job_dir, frame).exists() {
                Self::consume_ready(
                    pump,
                    &frame_name(watch.mod_job_dir, frame),
                    request.fps,
                    watch.video,
                    ffmpeg,
                )?;
                frame += 1;
                consumed = true;
                last_activity = Instant::now();
                report(false, pump.written, segment, &mut last_report);
                if self.game.cancelled(request.cancel) {
                    return Err(DemoError::new("CANCELLED"));
                }
            }
            let now = Instant::now();
            if let Some(done) = &journal.done {
                // `!(frame < frames)`: и при frames=NaN.
                if f64::from(frame).partial_cmp(&done.number("frames"))
                    != Some(std::cmp::Ordering::Less)
                {
                    break;
                }
                // Кадры появляются атомарно; DONE раньше последнего кадра — подождать недолго.
                if now.duration_since(last_activity) > EXIT_WAIT {
                    self.game.logger().warn(&format!(
                        "[demo] Ghosting -ghrender: DONE frames={}, а кадров {frame}",
                        done.get("frames").unwrap_or("undefined")
                    ));
                    break;
                }
            } else {
                if journal.first.is_none() && now.duration_since(watch.started) > self.first_line {
                    return Err(with_message(
                        "GHRENDER_UNSUPPORTED",
                        "Нет журнала ghrender.log",
                    ));
                }
                if let Some(code) = active.exit_code() {
                    read_log(&mut journal, &mut failure, &mut segment, &mut last_activity);
                    if let Some(error) = failure.take() {
                        return Err(error);
                    }
                    // Вышла, не написав ни строки (у мода код 2: журнал не создать).
                    if journal.first.is_none() {
                        return Err(with_message(
                            "GHRENDER_UNSUPPORTED",
                            format!("Игра вышла без журнала (код {code})"),
                        ));
                    }
                    if journal.done.is_none() {
                        return Err(
                            DemoError::new("GAME_EXITED").with_details(json!({"exitCode": code}))
                        );
                    }
                    continue;
                }
                if active.host_closed() {
                    return Err(with_message("RENDER_HOST_FAILED", "Помощник вышел"));
                }
                if now.duration_since(last_activity) > self.stall {
                    return Err(DemoError::new("RENDER_TIMEOUT").with_details(json!({
                        "framesDone": pump.written,
                        "framesTotal": js_number(watch.frames_total),
                    })));
                }
            }
            if !consumed {
                thread::sleep(POLL);
            }
        }
        report(true, pump.written, segment, &mut last_report);
        Ok(journal)
    }

    /// Одна строка журнала.
    fn journal_line(
        &self,
        line: &Line,
        journal: &mut Journal,
        failure: &mut Option<DemoError>,
        segment: &mut Option<SegmentProgress>,
        count: usize,
    ) {
        if journal.first.is_none() {
            journal.first = Some(line.text.clone());
            // Другой протокол — не наш формат журнала.
            if line.text != ghrender::PROTOCOL && failure.is_none() {
                *failure = Some(with_message("GHRENDER_UNSUPPORTED", line.text.clone()));
            }
            return;
        }
        match line.key.as_str() {
            "SEGMENT" => {
                // Записей не больше, чем сегментов в плане: мод показывает каждый один раз.
                if journal.segments.len() >= count {
                    return;
                }
                journal.segments.push(line.fields.clone());
                let n = line.fields.number("n");
                let index = if n.fract() == 0.0 && n >= 1.0 && n <= count as f64 {
                    n as u32
                } else {
                    journal.segments.len() as u32
                };
                *segment = Some(SegmentProgress {
                    index,
                    count: count as u32,
                });
            }
            "TIMER" if journal.timers.len() < count * 4 + 16 => {
                journal.timers.push(line.fields.clone())
            }
            "WARN" => self
                .game
                .logger()
                .warn(&format!("[demo] Ghosting -ghrender: {}", line.text)),
            "DONE" => journal.done = Some(line.fields.clone()),
            "FAIL" if failure.is_none() => *failure = Some(ghrender::fail_error(line)),
            _ => {}
        }
    }

    /// `consumeReady(...)`: готовый кадр (мод кладёт его переименованием) — в насос и удалить.
    fn consume_ready(
        pump: &mut Pump,
        file: &Path,
        fps: u32,
        video: &Path,
        ffmpeg: &Path,
    ) -> Result<(), DemoError> {
        let base = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Кадр может быть ненадолго занят (антивирус проверяет новый файл).
        let mut attempt = 0;
        let buffer = loop {
            match fs::read(file) {
                Ok(buffer) => break buffer,
                Err(error) if attempt >= 40 => {
                    let code = hlsr_core::js::errno::code(&error).unwrap_or_default();
                    return Err(with_message(
                        "RENDER_FAILED",
                        format!("Кадр {base} не читается: {code}"),
                    ));
                }
                Err(_) => {
                    attempt += 1;
                    thread::sleep(RETRY);
                }
            }
        };
        let info = source_render::tga_info(&buffer)
            .filter(|info| info.complete)
            .ok_or_else(|| with_message("RENDER_FAILED", format!("Кадр {base} не читается")))?;
        pump.frame(&buffer, &info, fps, video, ffmpeg)?;
        drop(buffer);
        // 0.4.9 после 20 попыток оставляет кадр молча; оставшиеся кадры копятся сверх запаса
        // `check_space` (≤ 90 на диске) — стоп с ошибкой.
        let mut attempt = 0;
        loop {
            match fs::remove_file(file) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) if attempt >= 40 => {
                    let code = hlsr_core::js::errno::code(&error).unwrap_or_default();
                    return Err(with_message(
                        "RENDER_FAILED",
                        format!("Кадр {base} не удаляется: {code}"),
                    ));
                }
                Err(_) => {
                    attempt += 1;
                    thread::sleep(RETRY);
                }
            }
        }
    }
}

impl Adapter for GhostingAdapter {
    fn render(&self, request: &RenderRequest) -> Result<Material, DemoError> {
        self.game
            .render(request.cancel, |active| self.render_job(request, active))
    }
}
