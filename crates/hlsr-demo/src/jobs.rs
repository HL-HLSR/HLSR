//! Задание экспорта или предпросмотра (`jobs.js`): адаптер рендерит сырьё в папку задания,
//! FFmpeg кодирует MP4 (H.264 + AAC, `+faststart`), итог проверяется (размер для Discord,
//! `ffprobe`) и сохраняется рядом с `timeline.json`. Папка задания удаляется всегда, кроме
//! готового предпросмотра.
//!
//! Корень заданий — в данных пользователя лаунчера (Windows — `%LOCALAPPDATA%\hlsr\demo-jobs`,
//! Linux — `~/.config/hlsr/demo-jobs`), не в общем `/tmp`: там чужой пользователь мог бы
//! подложить папку или ссылку. Задание синхронное (процессы ждутся опросом): фасад зовёт его
//! не в потоке окна.

use crate::errors::DemoError;
use crate::estimate::Stats;
use crate::ffmpeg::{self, Located, RunOptions, RunOutput};
use crate::library::Described;
use crate::planner::{
    self, Clip, EncodeMaterial, Plan, PlanOptions, Settings, Source, Timing, Trim, map_timeline,
    material_time, normalize_cuts, range_value,
};
use crate::source_render::{Cut, Range};
use crate::{Marker, round3};
use hlsr_core::js::errno;
use hlsr_core::js::number::number_to_string;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::value::{number, to_number};
use hlsr_core::js::{is_whitespace, math_round, slice_utf16, to_template_string};
use hlsr_core::json_text::{JsonFormat, js_number, to_js_json};
use hlsr_core::logger::Logger;
use hlsr_delivery::paths::{is_inside_with, remove_within_zone};
use icu_normalizer::ComposingNormalizerBorrowed;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Папка заданий в данных лаунчера.
pub const JOBS_DIR: &str = "demo-jobs";
/// Остатки прошлых запусков старше суток убираются.
pub const STALE: Duration = Duration::from_secs(24 * 60 * 60);
const OUTPUT: &str = "output.mp4";
const THROTTLE: Duration = Duration::from_millis(250);

/// Доля шкалы прогресса у фазы задания.
fn phase_range(phase: &str) -> (f64, f64) {
    match phase {
        "prepare" => (0.0, 2.0),
        "render" => (2.0, 50.0),
        "encode" => (50.0, 95.0),
        "verify" => (95.0, 100.0),
        _ => (100.0, 100.0),
    }
}

fn with_message(code: &str, message: impl Into<String>) -> DemoError {
    DemoError {
        message: message.into(),
        ..DemoError::new(code)
    }
}

fn is_enospc(error: &io::Error) -> bool {
    errno::code(error).as_deref() == Some("ENOSPC")
}

/// Ошибка ОС посреди задания: `NO_SPACE` или `fallback` (как `publicError`).
fn io_failure(error: &io::Error, fallback: &str) -> DemoError {
    with_message(
        if is_enospc(error) {
            "NO_SPACE"
        } else {
            fallback
        },
        error.to_string(),
    )
}

// ---------------------------------------------------------------- корень заданий

/// `checkRoot(root)`: настоящая папка (не ссылка и не junction), на Linux — наша и только для
/// нас (0700). Иначе — `JOBS_DIR_UNSAFE`.
pub fn check_root(root: &Path) -> Result<PathBuf, DemoError> {
    let meta = fs::symlink_metadata(root)
        .map_err(|error| with_message("JOBS_DIR_UNAVAILABLE", error.to_string()))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(with_message(
            "JOBS_DIR_UNSAFE",
            format!("{}: ссылка или не папка", root.display()),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let uid = rustix::process::getuid().as_raw();
        if meta.uid() != uid {
            return Err(with_message(
                "JOBS_DIR_UNSAFE",
                format!("{}: владелец {}, а не {uid}", root.display(), meta.uid()),
            ));
        }
        if meta.mode() & 0o077 != 0 {
            let _ = fs::set_permissions(root, fs::Permissions::from_mode(0o700));
        }
    }
    Ok(root.to_path_buf())
}

fn private_dir(recursive: bool) -> fs::DirBuilder {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(recursive);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
}

/// `prepareRoot(root)`: создать корень (0700) и проверить его.
pub fn prepare_root(root: Option<&Path>) -> Result<PathBuf, DemoError> {
    let root =
        root.ok_or_else(|| with_message("JOBS_DIR_UNAVAILABLE", "Корень заданий не задан"))?;
    private_dir(true)
        .create(root)
        .map_err(|error| io_failure(&error, "JOBS_DIR_UNAVAILABLE"))?;
    check_root(root)
}

/// `cleanupStale(root, now, maxAge)`: папки прошлых запусков старше `max_age` — только внутри
/// корня и без перехода по ссылкам (ссылка снимается сама). Файлы корня — маркер и снимок
/// реестра рендера Ghosting: их удаляет только удачный возврат реестра. Занятое уберётся в
/// следующий раз. Ответ — сколько убрано. Как в 0.4.9, зовётся только при старте службы
/// (`max_age` 0): заданий и открытого предпросмотра тогда нет.
pub fn cleanup_stale(root: &Path, now: SystemTime, max_age: Duration) -> Result<usize, DemoError> {
    if !root.exists() {
        return Ok(0);
    }
    check_root(root)?;
    let Ok(entries) = fs::read_dir(root) else {
        return Ok(0);
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let full = entry.path();
        let Ok(meta) = fs::symlink_metadata(&full) else {
            continue;
        };
        if meta.is_file() {
            continue;
        }
        let old = meta
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= max_age);
        if !old {
            continue;
        }
        let done = if meta.file_type().is_symlink() {
            fs::remove_file(&full)
                .or_else(|_| fs::remove_dir(&full))
                .is_ok()
        } else {
            remove_within_zone(root, &full).is_ok()
        };
        if done {
            removed += 1;
        }
    }
    Ok(removed)
}

/// `removeJobDir(root, dir)`: не вышло — уберётся чисткой старых папок.
pub fn remove_job_dir(root: &Path, dir: &Path) {
    let _ = remove_within_zone(root, dir);
}

// ---------------------------------------------------------------- имя итога

/// `safeBaseName(name)`: имя файла без запрещённых символов, имён устройств и хвостовых точек,
/// не длиннее 120 единиц UTF-16.
pub fn safe_base_name(name: &str) -> String {
    let normalized = ComposingNormalizerBorrowed::new_nfc().normalize(name);
    let mut base = String::with_capacity(normalized.len());
    let mut space = false;
    for c in normalized.chars() {
        let c = if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
            || c <= '\u{1f}'
            || c == '\u{7f}'
        {
            '_'
        } else {
            c
        };
        if is_whitespace(c) {
            if !space {
                base.push(' ');
            }
            space = true;
        } else {
            base.push(c);
            space = false;
        }
    }
    let trailing = |text: &str| text.trim_end_matches(['.', ' ']).to_owned();
    let mut base = trailing(hlsr_core::js::trim(&base));
    if base.is_empty() {
        base = "demo".to_owned();
    }
    if is_device_name(&base) {
        base.insert(0, '_');
    }
    let base = trailing(&slice_utf16(&base, 120));
    if base.is_empty() {
        "demo".to_owned()
    } else {
        base
    }
}

/// `/^(con|prn|aux|nul|conin\$|conout\$|com[0-9¹²³]|lpt[0-9¹²³])(\..*)?$/i`: имя устройства
/// Windows с любым расширением.
fn is_device_name(base: &str) -> bool {
    let stem = base
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix).is_some_and(|rest| {
            let mut chars = rest.chars();
            matches!(chars.next(), Some('0'..='9' | '¹' | '²' | '³')) && chars.next().is_none()
        })
    };
    matches!(
        stem.as_str(),
        "con" | "prn" | "aux" | "nul" | "conin$" | "conout$"
    ) || numbered("com")
        || numbered("lpt")
}

/// `clock(seconds)`: `ММ-СС`.
fn clock(seconds: f64) -> String {
    let whole = seconds.floor();
    format!("{:02}-{:02}", (whole / 60.0).floor(), whole % 60.0)
}

/// `outputBaseName(item, settings)`: имя демки, отрезок (если не весь), скорость, Discord.
pub fn output_base_name(name: &str, duration: Option<f64>, settings: &Settings) -> String {
    let name = if name.is_empty() { "demo" } else { name };
    let lower = name.to_ascii_lowercase();
    let stem = [".dem", ".ghplaylist"]
        .iter()
        .find(|ext| lower.ends_with(*ext))
        .map_or(name, |ext| &name[..name.len() - ext.len()]);
    let mut base = stem.to_owned();
    let range = settings.range;
    let full = duration
        .is_some_and(|d| d != 0.0 && !d.is_nan() && range.from <= 0.001 && range.to >= d - 0.001);
    if !full {
        base += &format!("_{}_{}", clock(range.from), clock(range.to));
    }
    if settings.speed != 1.0 {
        base += &format!("_x{}", number_to_string(settings.speed));
    }
    if settings.preset == "discord" {
        base += "_discord";
    }
    safe_base_name(&base)
}

/// Сохранённый итог: видео, `timeline.json` и его содержимое.
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    pub file: PathBuf,
    pub timeline_file: PathBuf,
    pub timeline: Value,
}

/// `saveOutput(outputDir, base, file, timeline)`: имя без перезаписи — «имя», «имя (2)», … —
/// и для MP4, и для `timeline.json`.
pub fn save_output(
    output_dir: &Path,
    base: &str,
    file: &Path,
    timeline: &Value,
) -> Result<Saved, DemoError> {
    fs::create_dir_all(output_dir)
        .map_err(|error| with_message("OUTPUT_DIR_UNAVAILABLE", error.to_string()))?;
    let failed = |error: &io::Error| io_failure(error, "OUTPUT_WRITE_FAILED");
    for n in 1..1000 {
        let name = if n == 1 {
            base.to_owned()
        } else {
            format!("{base} ({n})")
        };
        let video = output_dir.join(format!("{name}.mp4"));
        let json = output_dir.join(format!("{name}.timeline.json"));
        if json.exists() {
            continue;
        }
        match copy_exclusive(file, &video) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(failed(&error)),
            Ok(()) => {}
        }
        let mut data = timeline.clone();
        data["video"]["file"] = json!(format!("{name}.mp4"));
        let written = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&json)
            .and_then(|mut out| {
                io::Write::write_all(&mut out, to_js_json(&data, JsonFormat::Atomic).as_bytes())
            });
        if let Err(error) = written {
            let _ = fs::remove_file(&video);
            if error.kind() == io::ErrorKind::AlreadyExists {
                continue;
            }
            // Недописанный timeline.json занял бы имя навсегда (оно пропускается по `json.exists()`).
            let _ = fs::remove_file(&json);
            return Err(failed(&error));
        }
        return Ok(Saved {
            file: video,
            timeline_file: json,
            timeline: data,
        });
    }
    Err(with_message(
        "OUTPUT_WRITE_FAILED",
        "Свободное имя не найдено",
    ))
}

/// `copyFileSync(from, to, COPYFILE_EXCL)`: занятое имя — `AlreadyExists`; недописанная копия
/// удаляется.
fn copy_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    let mut source = fs::File::open(from)?;
    let mut target = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)?;
    let copied = io::copy(&mut source, &mut target);
    drop(target);
    if copied.is_err() {
        let _ = fs::remove_file(to);
    }
    copied.map(drop)
}

// ---------------------------------------------------------------- прогресс

/// Сегмент рендера: номер и сколько всего.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentProgress {
    pub index: u32,
    pub count: u32,
}

/// Прогресс адаптера: доля 0..1, сегмент, сообщение.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderProgress {
    pub fraction: f64,
    pub segment: Option<SegmentProgress>,
    pub message: Option<String>,
}

/// Прогресс задания для окна: фаза, проценты, сегмент, сообщение.
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    /// `prepare`, `render`, `encode`, `verify`, `done`.
    pub phase: &'static str,
    pub percent: f64,
    pub segment: Option<SegmentProgress>,
    pub message: Option<String>,
}

impl Progress {
    pub fn to_value(&self) -> Value {
        json!({
            "phase": self.phase,
            "percent": js_number(self.percent),
            "segment": self.segment.map(|s| json!({"index": s.index, "count": s.count})),
            "message": self.message,
        })
    }
}

/// `reporter(onProgress)`: одна фаза и сегмент — не чаще раза в 250 мс (кроме доли 1).
struct Reporter<'a> {
    sink: Option<&'a dyn Fn(&Progress)>,
    last: RefCell<(Option<Instant>, String)>,
}

impl Reporter<'_> {
    fn report(
        &self,
        phase: &'static str,
        fraction: f64,
        segment: Option<SegmentProgress>,
        message: Option<String>,
    ) {
        let Some(sink) = self.sink else { return };
        let (low, high) = phase_range(phase);
        let share = if fraction.is_nan() {
            0.0
        } else {
            fraction.clamp(0.0, 1.0)
        };
        let key = format!(
            "{phase}:{}",
            segment.map(|s| s.index.to_string()).unwrap_or_default()
        );
        let now = Instant::now();
        let mut last = self.last.borrow_mut();
        if key == last.1
            && share < 1.0
            && last.0.is_some_and(|at| now.duration_since(at) < THROTTLE)
        {
            return;
        }
        *last = (Some(now), key);
        drop(last);
        sink(&Progress {
            phase,
            percent: math_round(low + (high - low) * share),
            segment,
            message: message.filter(|m| !m.is_empty()),
        });
    }
}

// ---------------------------------------------------------------- адаптер

/// Сырьё адаптера в папке задания.
#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    /// Видео (файл) или кадры по шаблону.
    pub video: Source,
    /// Файл со звуком или нет.
    pub audio: Option<String>,
    /// Время демо первого кадра; нет — начало отрезка.
    pub offset: Option<f64>,
    /// Метки во времени демо.
    pub timeline: Vec<Marker>,
    /// Вырезанные из материала участки (загрузки), время демо.
    pub cuts: Vec<Cut>,
    /// Имя ASS в папке задания (KeyHUD, который рисует лаунчер).
    pub overlay: Option<String>,
    /// Коды предупреждений (`KEYHUD_PARTIAL`…).
    pub warnings: Vec<String>,
    /// Замер для оценки времени: профиль, запуски игры, сегменты, кадры.
    pub profile: Option<String>,
    pub launches: Option<f64>,
    pub segments: Option<f64>,
    pub frames: Option<f64>,
}

impl Material {
    /// Только видео, остальное пусто.
    pub fn new(video: Source) -> Material {
        Material {
            video,
            audio: None,
            offset: None,
            timeline: Vec::new(),
            cuts: Vec::new(),
            overlay: None,
            warnings: Vec::new(),
            profile: None,
            launches: None,
            segments: None,
            frames: None,
        }
    }
}

/// Что рендерить: демка, отрезок, кадр, вид в кадре, папка задания, FFmpeg, отмена, прогресс.
pub struct RenderRequest<'a> {
    pub item: &'a Described,
    pub range: Range,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub audio: bool,
    pub keyhud: bool,
    pub integrity: bool,
    pub hud: bool,
    pub own_config: bool,
    pub own_hud: bool,
    pub job_dir: &'a Path,
    pub ffmpeg: &'a Located,
    pub cancel: &'a dyn Fn() -> bool,
    pub on_progress: &'a dyn Fn(&RenderProgress),
}

/// Адаптер рендера (`adapters.js`): сырьё в папке задания, кодирование общее. Свои процессы
/// адаптер гасит по своим PID, когда `cancel()` истинно.
pub trait Adapter {
    fn render(&self, request: &RenderRequest) -> Result<Material, DemoError>;
}

/// Запуск FFmpeg (подмена в тестах).
pub type RunFn<'a> = dyn Fn(&Path, &[String], &RunOptions) -> Result<RunOutput, DemoError> + 'a;
/// `ffprobe` итога (подмена в тестах).
pub type FfprobeFn<'a> =
    dyn Fn(&Path, &Path, Option<&dyn Fn() -> bool>) -> Result<Value, DemoError> + 'a;

/// `export` или `preview`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Export,
    Preview,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            JobKind::Export => "export",
            JobKind::Preview => "preview",
        }
    }
}

/// Параметры задания (`runJob(options)`).
pub struct JobOptions<'a> {
    pub id: &'a str,
    pub kind: JobKind,
    pub item: &'a Described,
    pub adapter: &'a dyn Adapter,
    pub settings: &'a Settings,
    pub plan: &'a Plan,
    pub root: &'a Path,
    /// Папка итога экспорта (у предпросмотра нет).
    pub output_dir: Option<&'a Path>,
    /// Метки поверх меток адаптера (главы и финиш Source), во времени демо.
    pub extra_markers: &'a [Marker],
    pub ffmpeg: &'a Located,
    pub cancel: &'a dyn Fn() -> bool,
    pub on_progress: Option<&'a dyn Fn(&Progress)>,
    pub on_spawn: Option<&'a dyn Fn(u32)>,
    pub logger: Option<&'a dyn Logger>,
    pub run_fn: Option<&'a RunFn<'a>>,
    pub ffprobe_fn: Option<&'a FfprobeFn<'a>>,
}

/// Итог задания: MP4 (экспорт — в папке вывода, предпросмотр — в папке задания), метки, замер.
#[derive(Debug, Clone, PartialEq)]
pub struct JobResult {
    pub kind: JobKind,
    pub size: u64,
    pub attempts: u32,
    pub encoder: String,
    pub video_bps: Option<f64>,
    pub warnings: Vec<String>,
    /// Замер для оценки времени следующих заданий ([`crate::estimate::learn`]).
    pub stats: Stats,
    pub file: PathBuf,
    /// Папка предпросмотра (живёт, пока он открыт).
    pub dir: Option<PathBuf>,
    pub timeline_file: Option<PathBuf>,
    pub timeline: Value,
}

fn render_failed(message: &str) -> DemoError {
    with_message("RENDER_FAILED", message)
}

/// `checkMaterial(material, jobDir)`: сырьё адаптера — только файлы внутри папки задания.
fn check_material(material: &Material, job_dir: &Path) -> Result<(), DemoError> {
    let path = NodePath::native();
    let dir = job_dir.to_string_lossy();
    let inside = |file: &str| {
        is_inside_with(&path, &dir, file) && path.resolve(&[file]) != path.resolve(&[&dir])
    };
    let exists = |file: &str| Path::new(file).exists();
    match &material.video {
        Source::Frames { pattern, fps, .. } => {
            if !inside(pattern) || fps.is_nan() || *fps <= 0.0 {
                return Err(render_failed("Кадры вне папки задания"));
            }
        }
        Source::Video(video) => {
            if !inside(video) || !exists(video) {
                return Err(render_failed("Нет видео адаптера"));
            }
        }
    }
    if let Some(audio) = &material.audio
        && (!inside(audio) || !exists(audio))
    {
        return Err(render_failed("Нет звука адаптера"));
    }
    // Оверлей — имя файла в папке задания (FFmpeg запускается с cwd в ней).
    if let Some(overlay) = &material.overlay {
        // `/^[\w.-]+\.ass$/i`.
        let named = overlay.len() > 4
            && overlay
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
            && overlay[overlay.len() - 4..].eq_ignore_ascii_case(".ass");
        if !named || !job_dir.join(overlay).exists() {
            return Err(render_failed("Нет оверлея адаптера"));
        }
    }
    Ok(())
}

/// Проверка итога по самому видеопотоку (`verifyOutput`): кодек, длительность, FPS и размер.
/// Длительность файла не годится — `apad` дотягивает звук, и обрезанное видео прошло бы с
/// застывшим кадром. Ответ — длительность видео; `None` — проверить нечем (нет ffprobe).
fn verify_output(
    ffprobe_path: &Path,
    file: &Path,
    plan: &Plan,
    with_audio: bool,
    ffprobe_fn: &FfprobeFn<'_>,
    cancel: &dyn Fn() -> bool,
) -> Result<Option<f64>, DemoError> {
    let info = match ffprobe_fn(ffprobe_path, file, Some(cancel)) {
        Ok(info) => info,
        Err(error) if error.code == "CANCELLED" => return Err(error),
        // Linux: ffprobe может не быть в PATH — тогда проверка только по размеру.
        Err(error) if error.code == "FFMPEG_MISSING" => return Ok(None),
        Err(error) => return Err(with_message("VERIFY_FAILED", error.message)),
    };
    let streams = info
        .get("streams")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let stream = |kind: &str| {
        streams
            .iter()
            .find(|s| s.get("codec_type").and_then(Value::as_str) == Some(kind))
    };
    let (video, audio) = (stream("video"), stream("audio"));
    let codec = |s: Option<&Value>| {
        s.and_then(|s| s.get("codec_name"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let duration = to_number(video.and_then(|v| v.get("duration")));
    let tolerance = hlsr_core::js::math::max(0.5, plan.output_duration * 0.02);
    let mut problems = Vec::new();
    if codec(video).as_deref() != Some("h264") {
        problems.push("video-codec".to_owned());
    }
    if with_audio && codec(audio).as_deref() != Some("aac") {
        problems.push("audio-codec".to_owned());
    }
    if let Some(video) = video {
        let off = (duration - plan.output_duration).abs();
        if off.is_nan() || off > tolerance {
            problems.push(format!("video-duration {}", number_to_string(duration)));
        }
        let rate = video.get("avg_frame_rate");
        if rate.and_then(Value::as_str) != Some(format!("{}/1", plan.fps).as_str()) {
            problems.push(format!("fps {}", to_template_string(rate)));
        }
        let (width, height) = (video.get("width"), video.get("height"));
        let same = |value: Option<&Value>, expected: u32| {
            value.and_then(number) == Some(f64::from(expected))
        };
        if !same(width, plan.width) || !same(height, plan.height) {
            problems.push(format!(
                "size {}x{}",
                to_template_string(width),
                to_template_string(height)
            ));
        }
    }
    if !problems.is_empty() {
        return Err(with_message(
            "VERIFY_FAILED",
            format!("ffprobe: {}", problems.join(", ")),
        ));
    }
    Ok(Some(round3(duration)))
}

/// `/^[A-Z_]{1,40}$/`.
fn is_warning_code(code: &str) -> bool {
    (1..=40).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')
}

/// Папка задания. `id` — одно имя папки (иначе папка вне корня и не удалится); путь — UTF-8:
/// сырьё адаптера и аргументы FFmpeg — строки, и без этого задание упало бы только после рендера.
fn job_dir_of(root: &Path, id: &str) -> Result<PathBuf, DemoError> {
    let mut parts = Path::new(id).components();
    let plain = matches!(
        (parts.next(), parts.next()),
        (Some(Component::Normal(_)), None)
    );
    let dir = root.join(id);
    if !plain || dir.to_str().is_none() {
        return Err(with_message(
            "JOBS_DIR_UNAVAILABLE",
            format!("{}: не имя папки или не UTF-8", dir.display()),
        ));
    }
    Ok(dir)
}

/// `runJob(options)`: рендер, кодирование (Discord — с повторами по размеру), проверка,
/// сохранение. Папка задания удаляется всегда, кроме готового предпросмотра. Перед рендером
/// сверяется FFmpeg инструмента ([`ffmpeg::verify`]).
pub fn run_job(options: &JobOptions) -> Result<JobResult, DemoError> {
    prepare_root(Some(options.root))?;
    let job_dir = job_dir_of(options.root, options.id)?;
    // Не рекурсивно: уже существующая папка с этим именем — не наша.
    private_dir(false)
        .create(&job_dir)
        .map_err(|error| io_failure(&error, "JOBS_DIR_UNAVAILABLE"))?;
    let outcome = run_in(options, &job_dir);
    let keep = matches!(&outcome, Ok(result) if result.kind == JobKind::Preview);
    if !keep {
        remove_job_dir(options.root, &job_dir);
    }
    outcome
}

fn run_in(options: &JobOptions, job_dir: &Path) -> Result<JobResult, DemoError> {
    let settings = options.settings;
    let cancel = options.cancel;
    let aborted = || {
        if cancel() {
            Err(DemoError::new("CANCELLED"))
        } else {
            Ok(())
        }
    };
    let reporter = Reporter {
        sink: options.on_progress,
        last: RefCell::new((None, String::new())),
    };
    let run_fn: &RunFn<'_> = options.run_fn.unwrap_or(&ffmpeg::run);
    let ffprobe_fn: &FfprobeFn<'_> = options.ffprobe_fn.unwrap_or(&ffmpeg::ffprobe);

    reporter.report("prepare", 0.0, None, None);
    ffmpeg::verify(options.ffmpeg)?;
    let range = settings.range;
    let render_started = Instant::now();
    let on_render = |info: &RenderProgress| {
        reporter.report("render", info.fraction, info.segment, info.message.clone())
    };
    let material = options.adapter.render(&RenderRequest {
        item: options.item,
        range,
        fps: settings.fps,
        width: settings.width,
        height: settings.height,
        audio: settings.audio,
        keyhud: settings.keyhud,
        integrity: settings.integrity,
        hud: settings.hud,
        own_config: settings.own_config,
        own_hud: settings.own_hud,
        job_dir,
        ffmpeg: options.ffmpeg,
        cancel,
        on_progress: &on_render,
    })?;
    let render_seconds = render_started.elapsed().as_secs_f64();
    aborted()?;
    check_material(&material, job_dir)?;

    let cuts = normalize_cuts(&material.cuts);
    let timing = Timing {
        offset: material
            .offset
            .filter(|offset| offset.is_finite())
            .unwrap_or(range.from),
        cuts,
    };
    let start = material_time(range.from, &timing);
    let length = material_time(range.to, &timing) - start;
    // Вырезанное адаптером накрыло весь отрезок — кодировать нечего.
    let seconds = length / settings.speed;
    if seconds.is_nan() || seconds < 0.1 {
        return Err(render_failed("Материал короче 0,1 с после вырезанного"));
    }
    let with_audio = settings.audio && material.audio.is_some();
    let mut warnings = options.plan.warnings.clone();
    if settings.audio && material.audio.is_none() {
        warnings.push("AUDIO_UNAVAILABLE".to_owned());
    }
    // Предупреждения адаптера (HL1: KeyHUD без данных в части демки).
    for code in &material.warnings {
        if is_warning_code(code) && !warnings.contains(code) {
            warnings.push(code.clone());
        }
    }
    let mut plan = planner::plan_export(
        &Settings {
            audio: with_audio,
            ..settings.clone()
        },
        &PlanOptions {
            encoder: Some(options.plan.encoder.clone()),
            output_duration: round3(length / settings.speed),
            limit_bytes: options.plan.limit_bytes.filter(|limit| *limit != 0.0),
            warnings,
        },
    )?;

    let output = job_dir.join(OUTPUT);
    let output_text = output.to_string_lossy().into_owned();
    let passlog = job_dir.join("pass").to_string_lossy().into_owned();
    let encode_material = EncodeMaterial {
        source: material.video.clone(),
        audio: material.audio.clone(),
        overlay: material.overlay.clone(),
    };
    let encode_started = Instant::now();
    let passes: &[u8] = if plan.two_pass { &[1, 2] } else { &[0] };
    let null_target = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let mut attempt = 0;
    let (size, encode_seconds) = loop {
        for (i, pass) in passes.iter().enumerate() {
            let args = planner::encode_args(
                &encode_material,
                Trim {
                    start,
                    duration: length,
                },
                &plan,
                &output_text,
                *pass,
                &passlog,
                null_target,
            )?;
            let message = (attempt > 0).then(|| format!("retry-{attempt}"));
            let output_duration = plan.output_duration;
            let on_encode = |seconds: f64| {
                let share = (i as f64 + (seconds / output_duration).min(1.0)) / passes.len() as f64;
                reporter.report("encode", share, None, message.clone());
            };
            // Без предела времени, как в 0.4.9: длинная демка на процессоре кодируется часами;
            // зависший кодировщик снимает отмена.
            run_fn(
                &options.ffmpeg.ffmpeg,
                &args,
                &RunOptions {
                    cwd: Some(job_dir),
                    cancel: Some(cancel),
                    on_progress: Some(&on_encode),
                    on_spawn: options.on_spawn,
                    ..RunOptions::default()
                },
            )?;
        }
        aborted()?;
        let size = fs::metadata(&output)
            .map_err(|error| io_failure(&error, "JOB_FAILED"))?
            .len();
        let encode_seconds = encode_started.elapsed().as_secs_f64();
        if plan.preset != "discord" || plan.limit_bytes.is_some_and(|limit| size as f64 <= limit) {
            break (size, encode_seconds);
        }
        attempt += 1;
        let video_bps = planner::discord_retry(&plan, size as f64, attempt)?;
        if let Some(logger) = options.logger {
            logger.info(&format!(
                "[demo] {}: {size} байт больше лимита {}, повтор {attempt} с {} бит/с",
                options.id,
                number_to_string(plan.limit_bytes.unwrap_or_default()),
                number_to_string(video_bps)
            ));
        }
        plan.video_bps = Some(video_bps);
    };

    // Итог закодирован окончательно (повторов Discord больше не будет): сырьё адаптера (сотни МБ
    // и гигабайты) и журналы проходов больше не нужны, а предпросмотр держит папку, пока открыт.
    for entry in fs::read_dir(job_dir)
        .map_err(|error| io_failure(&error, "JOB_FAILED"))?
        .flatten()
    {
        if entry.file_name() != OUTPUT {
            remove_within_zone(job_dir, &entry.path())
                .map_err(|error| with_message("JOB_FAILED", error.to_string()))?;
        }
    }

    reporter.report("verify", 0.0, None, None);
    let probed = verify_output(
        &options.ffmpeg.ffprobe,
        &output,
        &plan,
        with_audio,
        ffprobe_fn,
        cancel,
    )?;
    aborted()?;

    let item = options.item;
    let files = item.source_files.clone().unwrap_or_else(|| {
        item.segments
            .iter()
            .map(|segment| segment.name.clone())
            .collect()
    });
    let mut markers = material.timeline.clone();
    markers.extend(options.extra_markers.iter().cloned());
    let clip = Clip {
        from: range.from,
        to: range.to,
        speed: settings.speed,
        offset: Some(timing.offset),
        cuts: timing.cuts.clone(),
    };
    let timeline = json!({
        "version": 1,
        "source": {
            // id записи в списке «Демо»: по нему вкладка «Отрендеренные» находит исходную демку.
            "id": item.entry.id,
            "files": files,
            "engine": item.engine,
            "game": item.game,
            "map": item.map.as_deref().filter(|map| !map.is_empty()),
        },
        "video": {
            "file": OUTPUT,
            "duration": js_number(probed.unwrap_or(plan.output_duration)),
            "fps": plan.fps,
            "width": plan.width,
            "height": plan.height,
            "speed": js_number(plan.speed),
            "range": range_value(range),
            "encoder": plan.encoder,
        },
        "markers": map_timeline(&markers, &clip).iter().map(|m| m.to_value()).collect::<Vec<_>>(),
    });
    let attempts = attempt + 1;
    let stats = Stats {
        profile: material.profile.clone(),
        launches: material
            .launches
            .filter(|n| *n != 0.0 && !n.is_nan())
            .unwrap_or(1.0),
        segments: material.segments.filter(|n| *n != 0.0 && !n.is_nan()),
        frames: material
            .frames
            .filter(|frames| frames.is_finite())
            .unwrap_or_else(|| math_round(length * f64::from(settings.fps))),
        render_seconds,
        height: f64::from(settings.height),
        preset: plan.preset.clone(),
        gpu: plan.encoder != planner::CPU_ENCODER,
        output_frames: math_round(plan.output_duration * f64::from(plan.fps)) * f64::from(attempts),
        encode_seconds,
    };
    let (file, dir, timeline_file, timeline) = match options.kind {
        JobKind::Preview => (output.clone(), Some(job_dir.to_path_buf()), None, timeline),
        JobKind::Export => {
            let output_dir = options
                .output_dir
                .ok_or_else(|| with_message("OUTPUT_DIR_UNAVAILABLE", "Папка итога не задана"))?;
            let base = output_base_name(&item.entry.name, item.duration, settings);
            let saved = save_output(output_dir, &base, &output, &timeline)?;
            (saved.file, None, Some(saved.timeline_file), saved.timeline)
        }
    };
    reporter.report("done", 1.0, None, None);
    Ok(JobResult {
        kind: options.kind,
        size,
        attempts,
        encoder: plan.encoder.clone(),
        video_bps: plan.video_bps,
        warnings: plan.warnings.clone(),
        stats,
        file,
        dir,
        timeline_file,
        timeline,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names() {
        for name in ["con", "CON.txt", "lpt³", "Com0.a.b", "conin$", "nul"] {
            assert!(is_device_name(name), "{name}");
        }
        for name in ["conx", "com", "com10", "lpt", "x.con", "co"] {
            assert!(!is_device_name(name), "{name}");
        }
    }

    #[test]
    fn job_dir_is_one_name() {
        let root = Path::new("root");
        assert_eq!(job_dir_of(root, "a1b2").unwrap(), root.join("a1b2"));
        for id in ["", ".", "..", "../x", "a/b", "/x"] {
            assert!(job_dir_of(root, id).is_err(), "{id}");
        }
    }

    #[test]
    fn reporter_throttles_same_key() {
        let seen = RefCell::new(Vec::new());
        let sink = |p: &Progress| seen.borrow_mut().push((p.phase, p.percent));
        let reporter = Reporter {
            sink: Some(&sink),
            last: RefCell::new((None, String::new())),
        };
        reporter.report("render", 0.1, None, None);
        reporter.report("render", 0.2, None, None);
        reporter.report("render", 1.0, None, None);
        reporter.report("encode", f64::NAN, None, None);
        reporter.report("encode", 7.0, None, None);
        assert_eq!(
            *seen.borrow(),
            [
                ("render", 7.0),
                ("render", 50.0),
                ("encode", 50.0),
                ("encode", 95.0)
            ]
        );
    }
}
