//! FFmpeg только кодирует и собирает MP4 (`ffmpeg.js`). Windows: закреплённая сборка gyan.dev,
//! скачанная как инструмент лаунчера (`config/tools/ffmpeg.manifest.json`); Linux, первая
//! версия: ffmpeg из PATH, без скачивания.
//!
//! Строже 0.4.9 (решение владельца 5): `ffmpeg.exe` и `ffprobe.exe` инструмента сверяются с
//! sha256 манифеста перед запуском ([`verify`]), как `bxt_rs.dll`.

use crate::errors::{DemoError, is_enoent};
use crate::planner::{CPU_ENCODER, GPU_ENCODERS};
use hlsr_core::js::{Platform, is_whitespace, trim};
use hlsr_core::logger::Logger;
use hlsr_delivery::files_manifest::hash_file_sync;
use hlsr_delivery::tools::{FFMPEG, dependency_of};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

/// Подмена пути к FFmpeg (папка bin или сам ffmpeg) — только отладка и `dev-hooks`.
pub const ENV_OVERRIDE: &str = "HLSR_DEMO_FFMPEG";
pub const PINNED_VERSION: &str = "9.0.2";
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(50);
const STDOUT_LIMIT: usize = 1024 * 1024;
const STDERR_TAIL: usize = 4000;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn exe_name(base: &str, platform: Platform) -> String {
    match platform {
        Platform::Win32 => format!("{base}.exe"),
        Platform::Linux => base.to_owned(),
    }
}

/// Где FFmpeg и откуда он: `env` (подмена), `tool` (инструмент), `path` (Linux, из PATH).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub source: &'static str,
}

/// Подмена из окружения: в выпуске без `dev-hooks` не читается.
pub fn env_override() -> Option<PathBuf> {
    if cfg!(any(debug_assertions, feature = "dev-hooks")) {
        std::env::var_os(ENV_OVERRIDE)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    } else {
        None
    }
}

/// `locate({ platform, env, production, toolDir })`: `override_path` — [`env_override`],
/// `tool_dir` — папка установленного инструмента `ffmpeg` (только Windows).
pub fn locate(
    platform: Platform,
    override_path: Option<&Path>,
    tool_dir: Option<&Path>,
) -> Option<Located> {
    if let Some(given) = override_path {
        let meta = std::fs::metadata(given).ok()?;
        let (dir, ffmpeg) = if meta.is_dir() {
            (
                given.to_path_buf(),
                given.join(exe_name("ffmpeg", platform)),
            )
        } else {
            (
                given.parent().unwrap_or(Path::new("")).to_path_buf(),
                given.to_path_buf(),
            )
        };
        return Some(Located {
            ffmpeg,
            ffprobe: dir.join(exe_name("ffprobe", platform)),
            source: "env",
        });
    }
    match platform {
        Platform::Win32 => {
            let dir = tool_dir?;
            let ffmpeg = dir.join("ffmpeg.exe");
            ffmpeg.exists().then(|| Located {
                ffmpeg,
                ffprobe: dir.join("ffprobe.exe"),
                source: "tool",
            })
        }
        Platform::Linux => Some(Located {
            ffmpeg: PathBuf::from("ffmpeg"),
            ffprobe: PathBuf::from("ffprobe"),
            source: "path",
        }),
    }
}

/// Сверка `ffmpeg.exe` и `ffprobe.exe` инструмента с sha256 манифеста перед запуском.
/// Подмена и PATH не сверяются (своих хешей нет). Не совпало или не читается — `FFMPEG_BROKEN`.
pub fn verify(located: &Located) -> Result<(), DemoError> {
    if located.source != "tool" {
        return Ok(());
    }
    let files = dependency_of(FFMPEG)
        .and_then(|manifest| manifest.get("files"))
        .and_then(Value::as_object)
        .expect("вшитый манифест FFmpeg с files");
    for file in [&located.ffmpeg, &located.ffprobe] {
        let name = file
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let expected = files.get(name).and_then(Value::as_str);
        let actual =
            hash_file_sync(file).map_err(|error| broken(format!("{name}: {}", error.message)))?;
        if expected != Some(actual.as_str()) {
            return Err(broken(format!(
                "{name}: sha256 {actual} не совпадает с манифестом"
            )));
        }
    }
    Ok(())
}

fn broken(message: String) -> DemoError {
    DemoError {
        message,
        ..DemoError::new("FFMPEG_BROKEN")
    }
}

/// Параметры запуска FFmpeg: папка, предел времени, сбор stdout, отмена, прогресс (секунды
/// выхода из `-progress pipe:1`) и PID запущенного процесса.
#[derive(Default)]
pub struct RunOptions<'a> {
    pub cwd: Option<&'a Path>,
    pub timeout: Option<Duration>,
    pub collect: bool,
    pub cancel: Option<&'a dyn Fn() -> bool>,
    pub on_progress: Option<&'a dyn Fn(f64)>,
    pub on_spawn: Option<&'a dyn Fn(u32)>,
}

/// Вывод завершившегося процесса (stdout — если собирался, не больше 1 МиБ; stderr — хвост).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutput {
    pub stdout: String,
    pub stderr: String,
}

/// Хвост строки: не больше `limit` байт, по границе символа.
fn tail(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut at = text.len() - limit;
    while !text.is_char_boundary(at) {
        at += 1;
    }
    &text[at..]
}

/// `run(exe, args, options)`: процесс FFmpeg без окна и без оболочки. Отмена и предел времени
/// гасят именно этот процесс (по его дескриптору). Ошибки: `CANCELLED`, `FFMPEG_TIMEOUT`,
/// `FFMPEG_FAILED` (`exitCode`), `FFMPEG_MISSING` (нет файла), `FFMPEG_START_FAILED`.
pub fn run(exe: &Path, args: &[String], options: &RunOptions) -> Result<RunOutput, DemoError> {
    let cancelled = || options.cancel.is_some_and(|cancel| cancel());
    if cancelled() {
        return Err(DemoError::new("CANCELLED"));
    }
    let mut command = Command::new(exe);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = options.cwd {
        command.current_dir(cwd);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|error| DemoError {
        message: error.to_string(),
        ..DemoError::new(if is_enoent(&error) {
            "FFMPEG_MISSING"
        } else {
            "FFMPEG_START_FAILED"
        })
    })?;
    if let Some(on_spawn) = options.on_spawn {
        on_spawn(child.id());
    }

    let stdout = child.stdout.take().expect("stdout в трубе");
    let mut stderr = child.stderr.take().expect("stderr в трубе");
    let (lines, received) = mpsc::channel::<Vec<u8>>();
    let out_reader = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = Vec::new();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if lines.send(line).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let err_reader = thread::spawn(move || {
        let mut tail_text = String::new();
        let mut buffer = [0u8; 8192];
        while let Ok(read) = stderr.read(&mut buffer) {
            if read == 0 {
                break;
            }
            tail_text.push_str(&String::from_utf8_lossy(&buffer[..read]));
            if tail_text.len() > STDERR_TAIL * 2 {
                tail_text = tail(&tail_text, STDERR_TAIL).to_owned();
            }
        }
        tail(&tail_text, STDERR_TAIL).to_owned()
    });

    let deadline = options.timeout.map(|timeout| Instant::now() + timeout);
    let mut collected: Vec<u8> = Vec::new();
    let mut handle_line = |line: Vec<u8>| {
        if options.collect && collected.len() < STDOUT_LIMIT {
            collected.extend_from_slice(&line);
        }
        if let Some(on_progress) = options.on_progress {
            let text = String::from_utf8_lossy(&line);
            if let Some(us) = text.trim().strip_prefix("out_time_us=")
                && let Ok(us) = us.parse::<u64>()
            {
                on_progress(us as f64 / 1e6);
            }
        }
    };
    let (mut aborted, mut timed_out, mut open) = (false, false, true);
    let status = loop {
        if open {
            match received.recv_timeout(POLL) {
                Ok(line) => handle_line(line),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => open = false,
            }
        } else {
            thread::sleep(POLL);
        }
        if !aborted && !timed_out && cancelled() {
            aborted = true;
            let _ = child.kill();
        }
        if !aborted && !timed_out && deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            timed_out = true;
            let _ = child.kill();
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
    };
    let _ = out_reader.join();
    for line in received.try_iter() {
        handle_line(line);
    }
    let stderr_text = err_reader.join().unwrap_or_default();
    if aborted {
        return Err(DemoError::new("CANCELLED"));
    }
    let message = || tail(stderr_text.trim(), 1000).to_owned();
    if timed_out {
        return Err(DemoError {
            message: message(),
            ..DemoError::new("FFMPEG_TIMEOUT")
        });
    }
    let code = status.and_then(|status| status.code());
    if code != Some(0) {
        return Err(DemoError {
            message: message(),
            ..DemoError::new("FFMPEG_FAILED").with_details(json!({"exitCode": code}))
        });
    }
    Ok(RunOutput {
        stdout: String::from_utf8_lossy(&collected).into_owned(),
        stderr: stderr_text,
    })
}

/// `ffmpeg -version`: первая строка, версия, закреплённая ли сборка, лицензия.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub first_line: String,
    pub version: Option<String>,
    pub pinned: bool,
    /// `GPL` или `LGPL`.
    pub license: &'static str,
}

/// Разбор вывода `ffmpeg -version`.
pub fn parse_version(stdout: &str) -> Version {
    let first_line = trim(
        stdout
            .split('\n')
            .next()
            .unwrap_or_default()
            .trim_end_matches('\r'),
    )
    .to_owned();
    // `/^ffmpeg version (\S+)/`.
    let version = first_line
        .strip_prefix("ffmpeg version ")
        .and_then(|rest| rest.split(is_whitespace).next())
        .filter(|word| !word.is_empty())
        .map(str::to_owned);
    let gpl = stdout.match_indices("--enable-gpl").any(|(at, word)| {
        stdout[at + word.len()..]
            .chars()
            .next()
            .is_none_or(|next| !(next.is_ascii_alphanumeric() || next == '_'))
    });
    Version {
        pinned: first_line.starts_with(&format!("ffmpeg version {PINNED_VERSION}")),
        license: if gpl { "GPL" } else { "LGPL" },
        version,
        first_line,
    }
}

/// `version(exe)`.
pub fn version(exe: &Path, cancel: Option<&dyn Fn() -> bool>) -> Result<Version, DemoError> {
    let options = RunOptions {
        collect: true,
        timeout: Some(VERSION_TIMEOUT),
        cancel,
        ..RunOptions::default()
    };
    run(exe, &["-version".to_owned()], &options).map(|output| parse_version(&output.stdout))
}

/// Разбор `ffmpeg -encoders`: строки `/^\s*[VAS][A-Z.]{5}\s+(\S+)/`.
pub fn parse_encoders(stdout: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in stdout.lines() {
        let line = line.trim_start();
        let mut chars = line.char_indices();
        let Some((_, kind)) = chars.next() else {
            continue;
        };
        if !matches!(kind, 'V' | 'A' | 'S') {
            continue;
        }
        let flags: String = line.chars().skip(1).take(5).collect();
        if flags.chars().count() != 5 || !flags.chars().all(|c| c.is_ascii_uppercase() || c == '.')
        {
            continue;
        }
        let rest = &line[1 + flags.len()..];
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        if let Some(name) = rest.split_whitespace().next() {
            names.insert(name.to_owned());
        }
    }
    names
}

/// `listEncoders(exe)`.
pub fn list_encoders(
    exe: &Path,
    cancel: Option<&dyn Fn() -> bool>,
) -> Result<BTreeSet<String>, DemoError> {
    let options = RunOptions {
        collect: true,
        timeout: Some(VERSION_TIMEOUT),
        cancel,
        ..RunOptions::default()
    };
    run(
        exe,
        &["-hide_banner".to_owned(), "-encoders".to_owned()],
        &options,
    )
    .map(|output| parse_encoders(&output.stdout))
}

/// `probeArgs(encoder, width, height, fps)`: секунда испытательной таблицы в `null`.
pub fn probe_args(encoder: &str, width: u32, height: u32, fps: u32) -> Vec<String> {
    let pix = match encoder {
        "h264_qsv" | "h264_amf" => "nv12",
        _ => "yuv420p",
    };
    [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        &format!("testsrc2=size={width}x{height}:rate={fps}"),
        "-t",
        "1",
        "-c:v",
        encoder,
        "-pix_fmt",
        pix,
        "-f",
        "null",
        "-",
    ]
    .iter()
    .map(|item| (*item).to_owned())
    .collect()
}

type ListFn = Box<dyn Fn(&Path) -> Result<BTreeSet<String>, DemoError> + Send + Sync>;
type ProbeFn = Box<dyn Fn(&Path, &[String]) -> Result<(), DemoError> + Send + Sync>;

/// Кодировщик в сводке проб.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderInfo {
    pub id: &'static str,
    /// `gpu` или `cpu`.
    pub kind: &'static str,
    pub available: bool,
}

/// Сводка проб для окна экспорта: все кодировщики, первый рабочий GPU и процессор, «Авто».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub encoders: Vec<EncoderInfo>,
    pub gpu: Option<&'static str>,
    pub cpu: Option<&'static str>,
    pub auto: Option<&'static str>,
}

impl Summary {
    pub fn to_value(&self) -> Value {
        json!({
            "encoders": self.encoders.iter().map(|item| json!({"id": item.id, "kind": item.kind, "available": item.available})).collect::<Vec<_>>(),
            "gpu": self.gpu,
            "cpu": self.cpu,
            "auto": self.auto,
        })
    }
}

/// Пробы кодировщиков на сессию (`EncoderProbe`): GPU-кодировщик считается рабочим, только
/// если короткое пробное кодирование нужного размера прошло. Ответы запоминаются.
pub struct EncoderProbe {
    exe: PathBuf,
    list_fn: ListFn,
    probe_fn: ProbeFn,
    logger: Option<Arc<dyn Logger>>,
    listed: OnceLock<BTreeSet<String>>,
    cache: Mutex<HashMap<String, bool>>,
}

impl EncoderProbe {
    /// Настоящие `-encoders` и пробы этим FFmpeg (предел 30 с на пробу). `cancel` — отмена
    /// службы (`signal` 0.4.9: выход гасит пробы).
    pub fn new(exe: PathBuf, cancel: Option<Arc<dyn Fn() -> bool + Send + Sync>>) -> EncoderProbe {
        let list_cancel = cancel.clone();
        EncoderProbe::with_fns(
            exe,
            Box::new(move |exe| list_encoders(exe, list_cancel.as_deref().map(|c| c as _))),
            Box::new(move |exe, args| {
                let options = RunOptions {
                    timeout: Some(PROBE_TIMEOUT),
                    cancel: cancel.as_deref().map(|c| c as _),
                    ..RunOptions::default()
                };
                run(exe, args, &options).map(drop)
            }),
        )
    }

    /// Свои список и проба (тесты).
    pub fn with_fns(exe: PathBuf, list_fn: ListFn, probe_fn: ProbeFn) -> EncoderProbe {
        EncoderProbe {
            exe,
            list_fn,
            probe_fn,
            logger: None,
            listed: OnceLock::new(),
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_logger(mut self, logger: Arc<dyn Logger>) -> EncoderProbe {
        self.logger = Some(logger);
        self
    }

    /// Список кодировщиков сборки; сбой — пустой.
    pub fn names(&self) -> &BTreeSet<String> {
        self.listed
            .get_or_init(|| (self.list_fn)(&self.exe).unwrap_or_default())
    }

    /// `works(encoder, width, height, fps)`.
    pub fn works(&self, encoder: &str, width: u32, height: u32, fps: u32) -> bool {
        let key = format!("{encoder}@{width}x{height}@{fps}");
        if let Some(known) = self.cache.lock().expect("кеш проб").get(&key) {
            return *known;
        }
        let works = if !self.names().contains(encoder) {
            false
        } else if encoder == CPU_ENCODER {
            true
        } else {
            let result = (self.probe_fn)(&self.exe, &probe_args(encoder, width, height, fps));
            if let Some(logger) = &self.logger {
                match &result {
                    Ok(()) => logger.info(&format!("[demo] проба {key}: работает")),
                    Err(error) => {
                        logger.info(&format!("[demo] проба {key}: не работает ({})", error.code))
                    }
                }
            }
            result.is_ok()
        };
        self.cache.lock().expect("кеш проб").insert(key, works);
        works
    }

    /// Базовая проба (1080p60) всех кодировщиков: список для окна экспорта.
    pub fn summary(&self) -> Summary {
        let encoders: Vec<EncoderInfo> = GPU_ENCODERS
            .into_iter()
            .chain([CPU_ENCODER])
            .map(|id| EncoderInfo {
                id,
                kind: if id == CPU_ENCODER { "cpu" } else { "gpu" },
                available: self.works(id, 1920, 1080, 60),
            })
            .collect();
        let first = |kind: &str| {
            encoders
                .iter()
                .find(|item| item.kind == kind && item.available)
                .map(|item| item.id)
        };
        let (gpu, cpu) = (first("gpu"), first("cpu"));
        Summary {
            auto: gpu.or(cpu),
            gpu,
            cpu,
            encoders,
        }
    }
}

/// `ffprobe(exe, file)`: длительность каждого потока — `format.duration` это максимум по
/// потокам, и дотянутый `apad` звук скрыл бы обрезанное видео.
pub fn ffprobe(
    exe: &Path,
    file: &Path,
    cancel: Option<&dyn Fn() -> bool>,
) -> Result<Value, DemoError> {
    let args = [
        "-v".to_owned(),
        "error".to_owned(),
        "-show_entries".to_owned(),
        "stream=codec_type,codec_name,avg_frame_rate,width,height,duration,nb_frames:format=duration".to_owned(),
        "-of".to_owned(),
        "json".to_owned(),
        file.to_string_lossy().into_owned(),
    ];
    let options = RunOptions {
        collect: true,
        timeout: Some(VERSION_TIMEOUT),
        cancel,
        ..RunOptions::default()
    };
    let output = run(exe, &args, &options)?;
    serde_json::from_str(&output.stdout).map_err(|_| DemoError {
        message: "ffprobe вернул не JSON".to_owned(),
        ..DemoError::new("VERIFY_FAILED")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line() {
        let text = "ffmpeg version 9.0.2-essentials_build-www.gyan.dev Copyright (c) 2000-2026\r\nbuilt with gcc\r\nconfiguration: --enable-gpl --enable-version3\r\n";
        let info = parse_version(text);
        assert_eq!(
            info.version.as_deref(),
            Some("9.0.2-essentials_build-www.gyan.dev")
        );
        assert!(info.pinned);
        assert_eq!(info.license, "GPL");
        let lgpl = parse_version(
            "ffmpeg version 6.1.1 Copyright\nconfiguration: --enable-gpls --disable-gpl\n",
        );
        assert_eq!(
            (lgpl.version.as_deref(), lgpl.pinned, lgpl.license),
            (Some("6.1.1"), false, "LGPL")
        );
        assert_eq!(parse_version("ffmpeg  version x").version, None);
        assert_eq!(parse_version("").first_line, "");
    }

    #[test]
    fn encoder_list() {
        let text = "Encoders:\n V..... = Video\n ------\n V....D libx264              libx264 H.264\n V....D h264_nvenc           NVIDIA\n A....D aac                  AAC\n S..... ass                  ASS\n X..... bogus  x\n V.... short\n";
        let names = parse_encoders(text);
        let expected: BTreeSet<String> = ["=", "libx264", "h264_nvenc", "aac", "ass"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(names, expected);
    }

    #[test]
    fn probe_order_and_cache() {
        use std::sync::Mutex as StdMutex;
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let seen = Arc::clone(&calls);
        let probe = EncoderProbe::with_fns(
            PathBuf::from("ffmpeg"),
            Box::new(|_| {
                Ok(["libx264", "h264_nvenc", "h264_amf", "h264_qsv", "aac"]
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect())
            }),
            Box::new(move |_, args| {
                let encoder = args[args.iter().position(|a| a == "-c:v").unwrap() + 1].clone();
                let input = args[args.iter().position(|a| a == "-i").unwrap() + 1].clone();
                seen.lock().unwrap().push(format!("{encoder} {input}"));
                if encoder == "h264_qsv" {
                    Ok(())
                } else {
                    Err(DemoError::new("FFMPEG_FAILED"))
                }
            }),
        );
        let summary = probe.summary();
        assert_eq!(
            (summary.gpu, summary.cpu, summary.auto),
            (Some("h264_qsv"), Some("libx264"), Some("h264_qsv"))
        );
        probe.summary();
        assert_eq!(
            *calls.lock().unwrap(),
            [
                "h264_nvenc testsrc2=size=1920x1080:rate=60",
                "h264_amf testsrc2=size=1920x1080:rate=60",
                "h264_qsv testsrc2=size=1920x1080:rate=60",
            ]
        );
        let none = EncoderProbe::with_fns(
            PathBuf::from("ffmpeg"),
            Box::new(|_| Ok(["libx264".to_owned()].into_iter().collect())),
            Box::new(|_, _| panic!("без пробы")),
        );
        let summary = none.summary();
        assert_eq!((summary.gpu, summary.auto), (None, Some("libx264")));
    }
}
