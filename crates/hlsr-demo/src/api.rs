//! Фасад «Демо» для окна (`DemoTool` и `bind` из `index.js` 0.4.9): 30 каналов `demo-*` и 6
//! событий ([`JOB_PROGRESS`] …). Ответ канала — `{status: "ok", …}` или `{status: "error", code,
//! …подробности}`; `demo-player-fullscreen` отвечает просто состоянием (`true`/`false`).
//!
//! Список и приём демок, разбор, FFmpeg и bxt-rs как скачиваемые зависимости (тот же
//! `ToolService`), пробы кодировщиков, план и задания экспорта и предпросмотра, «Отрендеренные»
//! (видео папки вывода), адреса `hlsr-media://` для плеера. Store — `demo-tool.json`. Рендер игр —
//! адаптеры Ghosting и HL1 ([`RenderGame`]), по одному на лаунчер; при старте — их восстановление
//! после сбоя и уборка корня заданий, через 15 с — папки рендера в играх и фоновый сброс mute раз в
//! минуту ([`DemoApi::start_background`]).
//!
//! Строже 0.4.9 (решение владельца 5): «Показать» и удаление в «Отрендеренных» — только у видео со
//! своим `timeline.json`; `ffmpeg.exe` инструмента сверяется с манифестом до `-version`.
//!
//! Тяжёлое (разбор, пробы FFmpeg, задание) идёт в `spawn_blocking` среды tokio.

use crate::errors::{DemoError, is_enoent, public_code};
use crate::estimate::{self, Input};
use crate::ffmpeg::{self, EncoderProbe, Located, RunOptions, Summary, Version};
use crate::ghosting::{EstimateProfile, GhostingAdapter};
use crate::goldsrc::Hl1Adapter;
use crate::goldsrc_demo::{self, GoldsrcScan};
use crate::jobs::{
    self, Adapter, JobKind, JobOptions, JobResult, Material, RenderProgress, RenderRequest,
};
use crate::library::{self, Described, Entry, GameContext, Record};
use crate::planner::{
    self, CPU_ENCODER, Capabilities, Clip, ENCODER_MODES, MARKER_TYPES, PlanOptions, RESOLUTIONS,
    Resolution, Settings, Source, VIEW_KEYS,
};
use crate::source_demo::{self, FinishOptions, FinishScan};
use crate::source_render::{self, Range};
use crate::{Marker, goldsrc_render, markers_value};
use hlsr_core::js::collate::{Collator, Sensitivity, default_locale};
use hlsr_core::js::number::number_to_string;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::text::utf16_len;
use hlsr_core::js::{Platform, math_round, slice_utf16, truthy};
use hlsr_core::json_text::js_number;
use hlsr_core::logger::Logger;
use hlsr_core::store::Store;
use hlsr_delivery::api::DeliveryEvent;
use hlsr_delivery::errors::Failure;
use hlsr_delivery::ports::{Dialogs, FileFilter, OpenDialog, Shell};
use hlsr_delivery::providers::BoxFuture;
use hlsr_delivery::tool_service::ToolService;
use hlsr_delivery::tools::{BXTRS, FFMPEG, dependency_of};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

/// События окну: канал и данные — как у фасадов доставки и платформы.
pub type DemoEvent = DeliveryEvent;

/// Ход задания: `{jobId, kind, phase, percent, segment, message}`.
pub const JOB_PROGRESS: &str = "demo-job-progress";
/// Задание готово: `{jobId, kind, result}`.
pub const JOB_DONE: &str = "demo-job-done";
/// Задание не удалось или отменено: `{jobId, kind, error}`.
pub const JOB_ERROR: &str = "demo-job-error";
/// Ход скачивания FFmpeg: `{percent}`.
pub const FFMPEG_PROGRESS: &str = "demo-ffmpeg-progress";
/// Ход скачивания bxt-rs: `{percent}`.
pub const BXTRS_PROGRESS: &str = "demo-bxtrs-progress";
/// Полный экран плеера снят не по команде окна (`false`); шлёт окно лаунчера (`window_ops`).
pub const PLAYER_FULLSCREEN_CHANGED: &str = "demo-player-fullscreen-changed";

/// Каналы окна (`bind` 0.4.9).
pub const CHANNELS: [&str; 30] = [
    "demo-list",
    "demo-add-paths",
    "demo-pick-files",
    "demo-remove",
    "demo-reorder",
    "demo-inspect",
    "demo-ffmpeg-status",
    "demo-ffmpeg-install",
    "demo-ffmpeg-cancel",
    "demo-bxtrs-status",
    "demo-bxtrs-install",
    "demo-bxtrs-cancel",
    "demo-encoders",
    "demo-options",
    "demo-plan",
    "demo-estimate",
    "demo-export-start",
    "demo-preview-start",
    "demo-cancel",
    "demo-preview-clear",
    "demo-settings-get",
    "demo-output-pick",
    "demo-reveal",
    "demo-media-url",
    "demo-timeline-read",
    "demo-rendered-list",
    "demo-rendered-open",
    "demo-rendered-delete",
    "demo-rendered-reveal",
    "demo-player-fullscreen",
];

/// Store окна: `demo-tool.json`.
pub const STORE_FILE: &str = "demo-tool.json";
/// Папки рендера в играх и первый фоновый сброс mute — чуть позже старта.
pub const BACKGROUND_DELAY: Duration = Duration::from_secs(15);
/// Фоновый сброс mute, запомненного Windows после рендера.
pub const MUTE_RESET_INTERVAL: Duration = Duration::from_secs(60);

const MAX_TIMELINE_BYTES: u64 = 5 * 1024 * 1024;
/// «Отрендеренные»: видео в папке вывода, у которых есть свой `timeline.json`.
const MAX_RENDERED: usize = 500;
const MAX_TIMELINE_MARKERS: usize = 5000;
const MAX_ADD_PATHS: usize = 512;
const MAX_SCANS: usize = 256;
const SAVED_KEYS: [&str; 11] = [
    "preset",
    "height",
    "fps",
    "audio",
    "keyhud",
    "integrity",
    "encoder",
    "speed",
    "hud",
    "ownConfig",
    "ownHud",
];
const MEDIA_SCHEME: &str = "hlsr-media";
const MEDIA_HOST: &str = "media";
/// Подставной рендер (`HLSR_DEMO_FAKE_RENDER=1`) — только отладка и тесты.
pub const FAKE_FLAG: &str = "HLSR_DEMO_FAKE_RENDER";

/// `StoreDefaults.demoTool`.
pub fn store_defaults() -> Value {
    json!({
        "manual": [],
        "export": {"preset": "quality", "height": 1080, "fps": 60, "audio": true, "keyhud": false,
            "encoder": "auto", "speed": 1},
        "outputDir": null,
    })
}

// ---------------------------------------------------------------- адаптеры

/// Возможности адаптера (`capabilities`): предел высоты, частоты и включённые флаги
/// (`keyhud`, `audio`, `integrity`, `hud`, `ownConfig`, `ownHud`).
#[derive(Debug, Clone, PartialEq)]
pub struct Caps {
    pub max_height: u32,
    pub fps: Vec<u32>,
    pub flags: Vec<&'static str>,
}

impl Caps {
    pub fn has(&self, flag: &str) -> bool {
        self.flags.contains(&flag)
    }

    fn planner(&self) -> Capabilities {
        Capabilities {
            max_height: self.max_height,
            fps: self.fps.clone(),
        }
    }
}

/// `PENDING_CAPS`: возможности, пока адаптера нет.
fn pending_caps() -> Caps {
    Caps {
        max_height: 2160,
        fps: planner::FPS.to_vec(),
        flags: vec!["audio"],
    }
}

/// `viewCaps(caps)`: переключатели «Дополнительных настроек», которые умеет адаптер.
fn view_caps(caps: Option<&Caps>) -> Map<String, Value> {
    VIEW_KEYS
        .iter()
        .map(|key| ((*key).to_owned(), json!(caps.is_some_and(|c| c.has(key)))))
        .collect()
}

/// Адаптер рендера для фасада: рендер (`jobs::Adapter`), проверка, оценка и жизнь рендера игры
/// (`isActive`, сброс mute, восстановление, уборка, выход лаунчера).
pub trait RenderGame: Adapter + Send + Sync {
    /// Игра лаунчера, которую рендерит адаптер (`None` — подставной).
    fn game_id(&self) -> Option<&'static str>;
    fn caps(&self) -> Caps;
    fn can_render(&self, item: &Described) -> Result<(), DemoError>;
    /// `estimateProfile(item, range, fps)`; `None` — профиля нет (`other`).
    fn estimate_profile(
        &self,
        _item: &Described,
        _range: Range,
        _fps: u32,
    ) -> Option<EstimateProfile> {
        None
    }
    fn is_active(&self) -> bool {
        false
    }
    /// `None` — mute можно снять сейчас.
    fn mute_reset_blocker(&self) -> Option<String> {
        Some("no-flag".to_owned())
    }
    fn reset_mute(&self) -> bool {
        false
    }
    /// Старт лаунчера: реестр (и журналы HL1) рендера, прерванного сбоем.
    fn recover(&self) {}
    /// Папки рендера, оставшиеся в игре после сбоя.
    fn cleanup_stale(&self) -> usize {
        0
    }
    /// `disposeSync()`: выход лаунчера посреди рендера.
    fn dispose(&self) {}
}

fn game_caps(game: &crate::game_render::GameRender) -> Caps {
    let base = game.capabilities();
    Caps {
        max_height: base.max_height,
        fps: base.fps,
        flags: game.spec.caps.to_vec(),
    }
}

impl RenderGame for GhostingAdapter {
    fn game_id(&self) -> Option<&'static str> {
        Some(crate::ghosting::GAME_ID)
    }
    fn caps(&self) -> Caps {
        game_caps(self.game())
    }
    fn can_render(&self, item: &Described) -> Result<(), DemoError> {
        GhostingAdapter::can_render(self, item)
    }
    fn estimate_profile(
        &self,
        item: &Described,
        range: Range,
        fps: u32,
    ) -> Option<EstimateProfile> {
        Some(GhostingAdapter::estimate_profile(
            self,
            item,
            Some(range),
            f64::from(fps),
        ))
    }
    fn is_active(&self) -> bool {
        self.game().is_active()
    }
    fn mute_reset_blocker(&self) -> Option<String> {
        self.game().mute_reset_blocker()
    }
    fn reset_mute(&self) -> bool {
        GhostingAdapter::reset_mute(self)
    }
    fn recover(&self) {
        self.game().recover();
    }
    fn cleanup_stale(&self) -> usize {
        self.game().cleanup_stale()
    }
    fn dispose(&self) {
        self.game().dispose();
    }
}

impl RenderGame for Hl1Adapter {
    fn game_id(&self) -> Option<&'static str> {
        Some(crate::goldsrc::GAME_ID)
    }
    fn caps(&self) -> Caps {
        game_caps(self.game())
    }
    fn can_render(&self, item: &Described) -> Result<(), DemoError> {
        Hl1Adapter::can_render(self, item)
    }
    fn estimate_profile(
        &self,
        item: &Described,
        range: Range,
        _fps: u32,
    ) -> Option<EstimateProfile> {
        Some(Hl1Adapter::estimate_profile(self, item, Some(range)))
    }
    fn is_active(&self) -> bool {
        self.game().is_active()
    }
    fn mute_reset_blocker(&self) -> Option<String> {
        self.game().mute_reset_blocker()
    }
    fn reset_mute(&self) -> bool {
        Hl1Adapter::reset_mute(self)
    }
    fn recover(&self) {
        // Только он вернёт и журналы игрока.
        Hl1Adapter::recover(self);
    }
    fn cleanup_stale(&self) -> usize {
        self.game().cleanup_stale()
    }
    fn dispose(&self) {
        self.game().dispose();
    }
}

/// Подставной адаптер (`fakeAdapter`, тесты и `HLSR_DEMO_FAKE_RENDER`): испытательная таблица FFmpeg
/// с номером кадра и синус в WAV длительностью отрезка, метки — из источника.
pub struct FakeAdapter;

impl Adapter for FakeAdapter {
    fn render(&self, request: &RenderRequest) -> Result<Material, DemoError> {
        let range = request.range;
        let length = planner::clip_duration(&Clip {
            from: range.from,
            to: range.to,
            speed: 1.0,
            offset: None,
            cuts: Vec::new(),
        });
        let path = |name: &str| request.job_dir.join(name).to_string_lossy().into_owned();
        let video = path("fake.mkv");
        let wav = request.audio.then(|| path("fake.wav"));
        let length_text = number_to_string(length);
        let mut args: Vec<String> = [
            "-hide_banner",
            "-nostdin",
            "-y",
            "-loglevel",
            "error",
            "-progress",
            "pipe:1",
            "-nostats",
            "-f",
            "lavfi",
            "-i",
        ]
        .map(str::to_owned)
        .to_vec();
        args.push(format!(
            "testsrc=size={}x{}:rate={}:duration={length_text}",
            request.width, request.height, request.fps
        ));
        if wav.is_some() {
            args.extend(["-f", "lavfi", "-i"].map(str::to_owned));
            args.push(format!(
                "sine=frequency=440:sample_rate=48000:duration={length_text}"
            ));
        }
        args.extend(
            [
                "-map",
                "0:v",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-crf",
                "12",
                "-pix_fmt",
                "yuv420p",
            ]
            .map(str::to_owned),
        );
        args.push(video.clone());
        if let Some(wav) = &wav {
            args.extend(["-map", "1:a", "-c:a", "pcm_s16le"].map(str::to_owned));
            args.push(wav.clone());
        }
        let on_progress = |seconds: f64| {
            (request.on_progress)(&RenderProgress {
                fraction: (seconds / length).min(1.0),
                segment: None,
                message: None,
            })
        };
        ffmpeg::run(
            &request.ffmpeg.ffmpeg,
            &args,
            &RunOptions {
                cancel: Some(request.cancel),
                on_progress: Some(&on_progress),
                ..RunOptions::default()
            },
        )?;
        let item = request.item;
        let mut timeline = vec![Marker {
            t: range.from,
            kind: "map",
            label: item.map.clone().unwrap_or_else(|| "fake".to_owned()),
        }];
        timeline.extend(item.markers.iter().cloned());
        timeline.push(Marker {
            t: range.from + (range.to - range.from) / 2.0,
            kind: "timer",
            label: "fake".to_owned(),
        });
        let mut material = Material::new(Source::Video(video));
        material.audio = wav;
        material.offset = Some(range.from);
        material.timeline = timeline;
        Ok(material)
    }
}

impl RenderGame for FakeAdapter {
    fn game_id(&self) -> Option<&'static str> {
        None
    }
    fn caps(&self) -> Caps {
        pending_caps()
    }
    fn can_render(&self, item: &Described) -> Result<(), DemoError> {
        if item.duration.is_some_and(|d| d > 0.0) {
            Ok(())
        } else {
            Err(DemoError::new("DURATION_UNKNOWN"))
        }
    }
}

/// `pickAdapter(item, adapters)`: первый готовый рендерить; иначе самый понятный отказ.
struct Verdict {
    adapter: Option<Arc<dyn RenderGame>>,
    ok: bool,
    code: Option<String>,
}

impl Verdict {
    fn caps(&self) -> Caps {
        self.adapter
            .as_ref()
            .map_or_else(pending_caps, |adapter| adapter.caps())
    }

    fn render_value(&self) -> Value {
        json!({"ok": self.ok, "code": self.code})
    }
}

fn pick_adapter(item: &Described, adapters: &[Arc<dyn RenderGame>]) -> Verdict {
    let mut refusal: Option<Verdict> = None;
    for adapter in adapters {
        let code = match adapter.can_render(item) {
            Ok(()) => {
                return Verdict {
                    adapter: Some(adapter.clone()),
                    ok: true,
                    code: None,
                };
            }
            Err(error) => error.code,
        };
        let better = refusal.as_ref().is_none_or(|current| {
            current.code.as_deref() == Some("DEMO_UNSUPPORTED") && code != "DEMO_UNSUPPORTED"
        });
        if better {
            refusal = Some(Verdict {
                adapter: Some(adapter.clone()),
                ok: false,
                code: Some(code),
            });
        }
    }
    refusal.unwrap_or(Verdict {
        adapter: None,
        ok: false,
        code: Some("DEMO_UNSUPPORTED".to_owned()),
    })
}

// ---------------------------------------------------------------- bxt-rs

/// `bxtrsLocated()`: подмена разработчика (`HLSR_DEMO_BXTRS`: файл или папка с `bxt_rs.dll`, без
/// sha256) или установленный инструмент закреплённой версии. Ответ — где он и откуда (`env`,
/// `tool`); sha256 сверяет адаптер перед рендером.
pub fn bxtrs_located(
    platform: Platform,
    override_path: Option<&Path>,
    tools: Option<&ToolService>,
) -> Option<(crate::goldsrc::Bxtrs, &'static str)> {
    if let Some(given) = override_path {
        let meta = fs::metadata(given).ok()?;
        let path = if meta.is_dir() {
            given.join("bxt_rs.dll")
        } else {
            given.to_path_buf()
        };
        return Some((crate::goldsrc::Bxtrs { path, sha256: None }, "env"));
    }
    if platform != Platform::Win32 {
        return None;
    }
    let tools = tools?;
    let manifest = dependency_of(BXTRS)?;
    let installed = tools.read_installed(BXTRS)?;
    if installed.get("version") != manifest.get("version") {
        return None;
    }
    let executable = manifest.get("executable")?.as_str()?;
    let path = tools.install_path_of(BXTRS).ok()?.join(executable);
    let sha256 = manifest["files"][executable].as_str().map(str::to_owned);
    path.exists()
        .then_some((crate::goldsrc::Bxtrs { path, sha256 }, "tool"))
}

/// `originOf(provider)`: «github.com/GyanD/codexffmpeg» и релиз — у источника на GitHub, иначе
/// только сайт.
fn origin_of(provider: &Value) -> Value {
    let url = provider
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return Value::Null;
    };
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let host = host
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if host.is_empty() {
        return Value::Null;
    }
    // `/<владелец>/<репозиторий>/releases/download/<релиз>/…`.
    let parts: Vec<&str> = path.split('/').collect();
    if host == "github.com"
        && parts.len() > 6
        && parts[3] == "releases"
        && parts[4] == "download"
        && parts[1..=5].iter().all(|part| !part.is_empty())
    {
        // shortcut: релиз без decodeURIComponent — в манифестах только «9.0.2»; иначе декодировать.
        return json!({"site": format!("github.com/{}/{}", parts[1], parts[2]), "release": parts[5]});
    }
    json!({"site": host, "release": null})
}

// ---------------------------------------------------------------- media

/// Адреса `hlsr-media://media/<токен>` для плеера: только файлы, которые фасад сам выдал (итоги
/// заданий, предпросмотр, видео «Отрендеренных»).
#[derive(Default)]
struct MediaRegistry {
    files: Mutex<HashMap<String, PathBuf>>,
}

impl MediaRegistry {
    fn register(&self, file: &Path) -> (String, String) {
        let token = random_hex(16);
        let resolved = PathBuf::from(NodePath::native().resolve(&[&*file.to_string_lossy()]));
        lock(&self.files).insert(token.clone(), resolved);
        let url = format!("{MEDIA_SCHEME}://{MEDIA_HOST}/{token}");
        (token, url)
    }

    fn unregister(&self, token: &str) {
        lock(&self.files).remove(token);
    }

    fn clear(&self) {
        lock(&self.files).clear();
    }

    fn resolve(&self, url: &str) -> Option<PathBuf> {
        let token = url
            .strip_prefix(&format!("{MEDIA_SCHEME}://{MEDIA_HOST}/"))?
            .trim_start_matches('/');
        if token.len() != 32
            || !token
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return None;
        }
        let file = lock(&self.files).get(token).cloned()?;
        file.is_file().then_some(file)
    }
}

/// Случайные байты ОС в hex (`crypto.randomBytes(n).toString("hex")`): id задания и токены
/// `hlsr-media://` (токен открывает плееру локальный файл — угадать его нельзя).
fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).expect("системный генератор случайных чисел недоступен");
    buffer.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Сброс на выходе из области — и когда future отменён, и при панике.
struct OnDrop<F: FnMut()>(F);

impl<F: FnMut()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------- timeline.json

/// `readTimelineFile(file)`: только настоящий файл (не ссылка) не больше 5 МиБ, версия 1, метки и
/// видео. Нет файла — `RESULT_NOT_FOUND`, иначе — `TIMELINE_INVALID`.
fn read_timeline_file(file: &Path) -> Result<Value, DemoError> {
    let io = |error: std::io::Error| {
        DemoError::new(if is_enoent(&error) {
            "RESULT_NOT_FOUND"
        } else {
            "TIMELINE_INVALID"
        })
    };
    let meta = fs::symlink_metadata(file).map_err(io)?;
    if !meta.is_file() || meta.len() > MAX_TIMELINE_BYTES {
        return Err(DemoError::new("TIMELINE_INVALID"));
    }
    let bytes = fs::read(file).map_err(io)?;
    let timeline: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes))
        .map_err(|_| DemoError::new("TIMELINE_INVALID"))?;
    let valid = timeline.is_object()
        && timeline["version"].as_f64() == Some(1.0)
        && timeline["markers"].is_array()
        && timeline["video"].is_object();
    if !valid {
        return Err(DemoError::new("TIMELINE_INVALID"));
    }
    Ok(timeline)
}

/// Число JS, конечное, — как есть; иначе `null`.
fn finite(value: Option<&Value>) -> Value {
    match value {
        Some(number @ Value::Number(_)) if number.as_f64().is_some_and(f64::is_finite) => {
            number.clone()
        }
        _ => Value::Null,
    }
}

/// `typeof value === "object" && value` (массив тоже объект).
fn object_like(value: Option<&Value>) -> Option<&Value> {
    value.filter(|value| value.is_object() || value.is_array())
}

/// `publicTimeline(timeline)`: `timeline.json` с диска — в окно только известные поля и метки.
fn public_timeline(timeline: &Value) -> Value {
    let text = |value: Option<&Value>| match value {
        Some(Value::String(text)) => json!(slice_utf16(text, 64)),
        _ => Value::Null,
    };
    let video = &timeline["video"];
    let range = object_like(video.get("range")).and_then(|range| {
        let (from, to) = (finite(range.get("from")), finite(range.get("to")));
        (!from.is_null() && !to.is_null()).then(|| json!({"from": from, "to": to}))
    });
    let speed = finite(video.get("speed"));
    let speed = if speed.as_f64().is_some_and(|s| s != 0.0) {
        speed
    } else {
        json!(1)
    };
    let source = object_like(timeline.get("source"));
    let field = |key: &str| source.and_then(|source| source.get(key));
    let files = field("files").and_then(Value::as_array).map_or(0, Vec::len);
    let markers: Vec<Value> = timeline["markers"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|marker| {
            let kind = marker.get("type").and_then(Value::as_str);
            let t = finite(marker.get("t")).as_f64();
            kind.is_some_and(|kind| MARKER_TYPES.contains(&kind)) && t.is_some_and(|t| t >= 0.0)
        })
        .take(MAX_TIMELINE_MARKERS)
        .map(|marker| {
            let label = match marker.get("label") {
                None | Some(Value::Null) => String::new(),
                Some(label) => hlsr_core::js::to_string(label),
            };
            json!({
                "t": marker["t"],
                "type": marker["type"],
                "label": slice_utf16(&label, 64),
                "sourceTime": finite(marker.get("sourceTime")),
            })
        })
        .collect();
    json!({
        "video": {
            "duration": finite(video.get("duration")),
            "fps": finite(video.get("fps")),
            "width": finite(video.get("width")),
            "height": finite(video.get("height")),
            "speed": speed,
            "range": range,
        },
        "source": {"game": text(field("game")), "map": text(field("map")), "files": files},
        "markers": markers,
    })
}

// ---------------------------------------------------------------- фасад

/// `shell.trashItem(path)`: ошибка — текст для журнала.
pub type Trash = dyn Fn(&Path) -> Result<(), String> + Send + Sync;

/// Связи фасада с лаунчером (`options` конструктора `DemoTool`).
pub struct DemoOptions {
    /// Store `demo-tool.json` (общий с адаптерами: флаги `*UnmutePending`).
    pub store: Arc<Mutex<Store>>,
    /// Установленные игры с папками демо (`gameDelivery.demoContexts()`).
    pub games: Box<dyn Fn() -> Vec<GameContext<'static>> + Send + Sync>,
    /// `ToolService` библиотеки, если служба доставки есть.
    pub tools: Box<dyn Fn() -> Option<Arc<ToolService>> + Send + Sync>,
    /// Достижения профиля: «Монтажёр» за готовый экспорт.
    pub achieve: Box<dyn Fn(&str) + Send + Sync>,
    pub dialogs: Arc<dyn Dialogs>,
    pub shell: Arc<dyn Shell>,
    /// `shell.trashItem(path)`: в Корзину.
    pub trash: Box<Trash>,
    /// Полный экран плеера в окне лаунчера; ответ — применённое состояние.
    pub fullscreen: Box<dyn Fn(bool) -> BoxFuture<'static, bool> + Send + Sync>,
    /// Папка «Видео» пользователя.
    pub videos_dir: PathBuf,
    /// Корень заданий в данных пользователя; без него задания недоступны.
    pub jobs_root: Option<PathBuf>,
    pub platform: Platform,
    /// Подмена FFmpeg (`HLSR_DEMO_FFMPEG`, только отладка и `dev-hooks`).
    pub ffmpeg_override: Option<PathBuf>,
    /// Подмена `bxt_rs.dll` (`HLSR_DEMO_BXTRS`, только отладка и `dev-hooks`).
    pub bxtrs_override: Option<PathBuf>,
    /// Подставной рендер первым в списке адаптеров (`HLSR_DEMO_FAKE_RENDER`).
    pub fake_render: bool,
    /// Адаптеры рендера игр по порядку (HL1, затем Ghosting).
    pub renderers: Vec<Arc<dyn RenderGame>>,
    pub logger: Arc<dyn Logger>,
    /// Только тесты: маленький лимит Discord.
    pub limit_bytes: Option<f64>,
}

struct Job {
    id: Option<String>,
    kind: JobKind,
    cancel: Arc<AtomicBool>,
}

/// Итог задания для `demo-reveal`, `demo-media-url`, `demo-timeline-read`.
struct Stored {
    kind: JobKind,
    file: PathBuf,
    timeline_file: Option<PathBuf>,
    timeline: Value,
    token: String,
    url: String,
}

struct Preview {
    job_id: String,
    dir: Option<PathBuf>,
    token: String,
}

#[derive(Default)]
struct State {
    job: Option<Job>,
    preview: Option<Preview>,
    results: HashMap<String, Stored>,
    /// Идущая установка инструмента и её процент.
    install: Option<(&'static str, f64)>,
    versions: HashMap<PathBuf, Version>,
    probe: Option<(PathBuf, Arc<EncoderProbe>)>,
    /// Видео, открытое во вкладке «Отрендеренные»: токен и файл.
    rendered: Option<(String, String)>,
}

#[derive(Default)]
struct Scans {
    goldsrc: HashMap<String, Option<GoldsrcScan>>,
    source: HashMap<String, Option<FinishScan>>,
}

/// Подготовленный план задания.
struct Prepared {
    settings: Settings,
    plan: planner::Plan,
    verdict: Verdict,
    located: Located,
}

/// Пути видео «Отрендеренных»: файл и его `timeline.json`.
struct RenderedPaths {
    file: String,
    timeline: String,
}

/// Фасад «Демо».
pub struct DemoApi {
    me: Weak<DemoApi>,
    options: DemoOptions,
    adapters: Vec<Arc<dyn RenderGame>>,
    state: Mutex<State>,
    scans: Mutex<Scans>,
    /// Фоновый сброс mute: игры, у которых он не удался (до перезапуска лаунчера); замок — один
    /// сброс за раз.
    mute: Mutex<HashSet<&'static str>>,
    /// Правка записей Store `manual` (добавить, убрать, порядок) — одна за раз: каналы идут
    /// параллельно в `spawn_blocking`, чтение и запись иначе затирали бы друг друга.
    manual_edit: Mutex<()>,
    media: MediaRegistry,
    events: broadcast::Sender<DemoEvent>,
    /// Выход лаунчера: пробы, проверки версии и разбор гасятся.
    disposed: AtomicBool,
}

type Reply = Result<Value, DemoError>;

impl DemoApi {
    /// Фасад и старт: восстановление после рендера, прерванного сбоем (раньше чистки), и уборка
    /// корня заданий — заданий ещё нет, всё оставшееся — мусор.
    pub fn new(options: DemoOptions) -> Arc<DemoApi> {
        let mut adapters: Vec<Arc<dyn RenderGame>> = Vec::new();
        if options.fake_render {
            adapters.push(Arc::new(FakeAdapter));
        }
        adapters.extend(options.renderers.iter().cloned());
        let (events, _) = broadcast::channel(256);
        let api = Arc::new_cyclic(|me| DemoApi {
            me: me.clone(),
            options,
            adapters,
            state: Mutex::new(State::default()),
            scans: Mutex::new(Scans::default()),
            mute: Mutex::new(HashSet::new()),
            manual_edit: Mutex::new(()),
            media: MediaRegistry::default(),
            events,
            disposed: AtomicBool::new(false),
        });
        for renderer in &api.options.renderers {
            renderer.recover();
        }
        if let Some(root) = &api.options.jobs_root {
            match jobs::cleanup_stale(root, SystemTime::now(), Duration::ZERO) {
                Ok(0) => {}
                Ok(removed) => api
                    .log()
                    .info(&format!("[demo] удалены старые папки заданий: {removed}")),
                Err(error) => api.log().warn(&format!(
                    "[demo] старые папки заданий не убраны: {}",
                    error.message
                )),
            }
        }
        api
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DemoEvent> {
        self.events.subscribe()
    }

    fn log(&self) -> &dyn Logger {
        self.options.logger.as_ref()
    }

    fn emit(&self, channel: &'static str, payload: Value) {
        let _ = self.events.send(DemoEvent { channel, payload });
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    fn is_disposed(&self) -> bool {
        self.disposed.load(Ordering::SeqCst)
    }

    fn store_get(&self, key: &str) -> Option<Value> {
        lock(&self.options.store).get(key).cloned()
    }

    fn store_set(&self, key: &str, value: Value) {
        lock(&self.options.store).set(key, value);
    }

    /// Файл адреса `hlsr-media://media/<токен>`, который фасад выдал (плеер).
    pub fn media_file(&self, url: &str) -> Option<PathBuf> {
        self.media.resolve(url)
    }

    /// Плеер (G12b): файл адреса `hlsr-media://` и FFmpeg с ffprobe, сверенные с манифестом перед
    /// каждым открытием (решение 5). Блокирует (хеш файлов) — звать в фоне.
    pub fn player_media(&self, url: &str) -> Result<(Located, PathBuf), DemoError> {
        let file = self
            .media_file(url)
            .ok_or_else(|| DemoError::new("RESULT_NOT_FOUND"))?;
        let located = self
            .located()
            .ok_or_else(|| DemoError::new("FFMPEG_MISSING"))?;
        ffmpeg::verify(&located)?;
        Ok((located, file))
    }

    // ------------------------------------------------------------ каналы

    /// Канал окна; `None` — не канал «Демо». Ошибка — в ответе, отказа промиса нет.
    pub async fn call(&self, channel: &str, args: &[Value]) -> Option<Result<Value, String>> {
        let arg = |index: usize| args.get(index).cloned().unwrap_or(Value::Null);
        let reply: Reply = match channel {
            "demo-list" => self.blocking(|api| Ok(api.list())).await,
            "demo-add-paths" => {
                let paths = arg(0);
                self.blocking(move |api| api.add_paths(&paths)).await
            }
            "demo-pick-files" => self.pick_files().await,
            "demo-remove" => {
                let id = arg(0);
                self.blocking(move |api| api.remove(&id)).await
            }
            "demo-reorder" => {
                let (id, order) = (arg(0), arg(1));
                self.blocking(move |api| api.reorder(&id, &order)).await
            }
            "demo-inspect" => {
                let id = arg(0);
                self.blocking(move |api| api.inspect(&id)).await
            }
            "demo-ffmpeg-status" => {
                self.blocking(|api| Ok(json!({"status": "ok", "ffmpeg": api.ffmpeg_status()})))
                    .await
            }
            "demo-ffmpeg-install" => self.install_ffmpeg().await,
            "demo-ffmpeg-cancel" => Ok(self.cancel_install(FFMPEG)),
            "demo-bxtrs-status" => {
                self.blocking(|api| Ok(json!({"status": "ok", "bxtrs": api.bxtrs_status()})))
                    .await
            }
            "demo-bxtrs-install" => self.install_bxtrs().await,
            "demo-bxtrs-cancel" => Ok(self.cancel_install(BXTRS)),
            "demo-encoders" => self.blocking(|api| api.encoders()).await,
            "demo-options" => {
                let id = arg(0);
                self.blocking(move |api| api.options(&id)).await
            }
            "demo-plan" => {
                let (id, settings) = (arg(0), arg(1));
                self.blocking(move |api| api.plan(&id, &settings)).await
            }
            "demo-estimate" => {
                let (id, list) = (arg(0), arg(1));
                self.blocking(move |api| api.estimate(&id, &list)).await
            }
            "demo-export-start" => self.start(JobKind::Export, arg(0), arg(1)).await,
            "demo-preview-start" => {
                let raw = arg(1);
                match self.blocking(move |api| Ok(api.preview_input(&raw))).await {
                    Ok(input) => self.start(JobKind::Preview, arg(0), input).await,
                    Err(error) => Err(error),
                }
            }
            "demo-cancel" => Ok(self.cancel(&arg(0))),
            "demo-preview-clear" => self.blocking(|api| Ok(api.clear_preview())).await,
            "demo-settings-get" => Ok(self.settings()),
            "demo-output-pick" => self.pick_output_dir().await,
            "demo-reveal" => {
                let id = arg(0);
                self.blocking(move |api| api.reveal(&id)).await
            }
            "demo-media-url" => self.media_url(&arg(0)),
            "demo-timeline-read" => {
                let id = arg(0);
                self.blocking(move |api| api.read_timeline(&id)).await
            }
            "demo-rendered-list" => self.blocking(|api| Ok(api.rendered_list())).await,
            "demo-rendered-open" => {
                let name = arg(0);
                self.blocking(move |api| api.rendered_open(&name)).await
            }
            "demo-rendered-delete" => {
                let name = arg(0);
                self.blocking(move |api| api.rendered_delete(&name)).await
            }
            "demo-rendered-reveal" => {
                let name = arg(0);
                self.blocking(move |api| api.rendered_reveal(&name)).await
            }
            "demo-player-fullscreen" => {
                let on = arg(0) == Value::Bool(true);
                return Some(Ok(Value::Bool((self.options.fullscreen)(on).await)));
            }
            _ => return None,
        };
        Some(Ok(match reply {
            Ok(value) => value,
            Err(error) => {
                let mut out = Map::new();
                out.insert("status".to_owned(), json!("error"));
                if let Value::Object(public) = error.to_public() {
                    out.extend(public);
                }
                Value::Object(out)
            }
        }))
    }

    /// Синхронная работа фасада в `spawn_blocking`.
    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(&DemoApi) -> Result<T, DemoError> + Send + 'static,
    ) -> Result<T, DemoError> {
        let Some(api) = self.me.upgrade() else {
            return Err(DemoError::new("DEMO_FAILED"));
        };
        tokio::task::spawn_blocking(move || work(&api))
            .await
            .unwrap_or_else(|error| {
                Err(DemoError {
                    message: error.to_string(),
                    ..DemoError::new("DEMO_FAILED")
                })
            })
    }

    // ------------------------------------------------------------ список и приём

    /// `manual()`: записи Store; удалённые файлы отбрасываются и из Store, записи с недоступного
    /// диска (USB, сеть) остаются и показываются с `DEMO_NOT_FOUND`.
    fn manual(&self) -> (Vec<Entry>, Vec<Record>) {
        let stored = self.store_get("manual").unwrap_or(Value::Null);
        let (entries, cleaned) = library::manual_entries(&stored);
        let cleaned_value = records_value(&cleaned);
        if stored.is_array() && cleaned_value != stored {
            // Чистка не затирает правку, записанную после чтения (`manual_edit` она не держит).
            let mut store = lock(&self.options.store);
            if store.get("manual") == Some(&stored) {
                store.set("manual", cleaned_value);
            }
        }
        (entries, cleaned)
    }

    fn entries(&self) -> Vec<Entry> {
        let mut entries = library::game_entries(&(self.options.games)());
        entries.extend(self.manual().0);
        entries
    }

    fn resolve(&self, id: &Value) -> Result<Described, DemoError> {
        let id = id
            .as_str()
            .ok_or_else(|| DemoError::new("DEMO_NOT_FOUND"))?;
        self.entries()
            .iter()
            .find(|entry| entry.id == id)
            .map(library::describe)
            .ok_or_else(|| DemoError::new("DEMO_NOT_FOUND"))
    }

    fn list_items(&self) -> Vec<Value> {
        self.entries()
            .iter()
            .map(|entry| library::describe(entry).summary())
            .collect()
    }

    fn list(&self) -> Value {
        json!({"status": "ok", "items": self.list_items()})
    }

    fn add_paths(&self, paths: &Value) -> Reply {
        if paths
            .as_array()
            .is_none_or(|list| list.len() > MAX_ADD_PATHS)
        {
            return Err(DemoError::new("PATH_INVALID"));
        }
        let edit = lock(&self.manual_edit);
        let added = library::add_paths(&self.manual().1, paths, self.options.platform);
        if !added.added.is_empty() {
            self.store_set("manual", records_value(&added.stored));
        }
        drop(edit);
        for rejected in &added.rejected {
            self.log().info(&format!(
                "[demo] не добавлен {}: {}",
                rejected.name, rejected.code
            ));
        }
        let ids: Vec<String> = library::manual_entries(&records_value(&added.added))
            .0
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        let rejected: Vec<Value> = added.rejected.iter().map(|r| r.to_value()).collect();
        Ok(json!({"status": "ok", "added": ids, "rejected": rejected, "items": self.list_items()}))
    }

    async fn pick_files(&self) -> Reply {
        let picked = self
            .options
            .dialogs
            .show_open_dialog(OpenDialog {
                title: None,
                default_path: None,
                properties: vec!["openFile", "multiSelections"],
                filters: vec![FileFilter::new("Demo", &["dem", "ghplaylist"])],
            })
            .await;
        if picked.is_empty() {
            return Ok(json!({"status": "cancelled"}));
        }
        let paths = Value::Array(
            picked
                .iter()
                .map(|path| json!(path.to_string_lossy()))
                .collect(),
        );
        self.blocking(move |api| api.add_paths(&paths)).await
    }

    /// `manualIndex(id)`: запись Store, её место и элемент.
    fn manual_index(&self, id: &Value) -> Result<(usize, Vec<Record>, Entry), DemoError> {
        let (entries, cleaned) = self.manual();
        let index = entries
            .iter()
            .position(|entry| id.as_str() == Some(entry.id.as_str()))
            .ok_or_else(|| DemoError::new("DEMO_NOT_FOUND"))?;
        let entry = entries[index].clone();
        Ok((index, cleaned, entry))
    }

    fn remove(&self, id: &Value) -> Reply {
        let edit = lock(&self.manual_edit);
        let (index, mut cleaned, _) = self.manual_index(id)?;
        cleaned.remove(index);
        self.store_set("manual", records_value(&cleaned));
        drop(edit);
        Ok(self.list())
    }

    /// Порядок сегментов набора HL1: `order` — перестановка индексов `0..n-1`.
    fn reorder(&self, id: &Value, order: &Value) -> Reply {
        let edit = lock(&self.manual_edit);
        let (index, mut cleaned, entry) = self.manual_index(id)?;
        let files = &entry.files;
        let order: Option<Vec<usize>> = order.as_array().and_then(|list| {
            list.iter()
                .map(|value| value.as_f64().filter(|n| n.fract() == 0.0 && *n >= 0.0))
                .map(|n| n.map(|n| n as usize).filter(|n| *n < files.len()))
                .collect()
        });
        let valid = order.as_ref().is_some_and(|order| {
            order.len() == files.len() && order.iter().collect::<HashSet<_>>().len() == files.len()
        });
        let (true, Some(order)) = (entry.kind == "segments" && valid, order) else {
            return Err(DemoError::new("ORDER_INVALID"));
        };
        cleaned[index] = Record::Segments(order.iter().map(|&n| files[n].clone()).collect());
        self.store_set("manual", records_value(&cleaned));
        drop(edit);
        self.inspect(id)
    }

    fn inspect(&self, id: &Value) -> Reply {
        let item = self.resolve(id)?;
        let verdict = self.verdict(&item);
        let segments: Vec<Value> = item
            .segments
            .iter()
            .map(|segment| {
                let mut value = segment.to_value();
                if let Some(fields) = value.as_object_mut() {
                    fields.shift_remove("path");
                    fields.shift_remove("sha256");
                }
                value
            })
            .collect();
        let caps = verdict.adapter.as_ref().map(|adapter| adapter.caps());
        Ok(json!({
            "status": "ok",
            "item": item.summary(),
            "segments": segments,
            "markers": markers_value(&self.source_markers(&item)),
            // view — что из «Дополнительных настроек» умеет адаптер (keyhud, hud, ownConfig, ownHud).
            "render": {"ok": verdict.ok, "code": verdict.code, "view": view_caps(caps.as_ref())},
        }))
    }

    /// Метки до рендера. HL1 — те же, что даст рендер (стыки сегментов и смены карт из самих
    /// демок); Source — метки описания (с главами) и финиш из самих демок.
    fn source_markers(&self, item: &Described) -> Vec<Marker> {
        if item.engine == Some(crate::headers::ENGINE_SOURCE) {
            let mut markers = item.markers.clone();
            markers.extend(self.finish_markers(item));
            markers.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
            return markers;
        }
        if item.engine != Some(crate::headers::ENGINE_GOLDSRC) || !item.supported {
            return item.markers.clone();
        }
        let (picked, _) = goldsrc_render::plan_segments(&item.segments, None);
        let scans: Vec<Option<GoldsrcScan>> = picked
            .iter()
            .map(|entry| self.goldsrc_scan(entry.segment))
            .collect();
        goldsrc_render::timeline_of(&picked, &scans, None)
    }

    /// Финиш рана Source (запись мода в демке) — во времени демо, по тем же сегментам, что метки.
    fn finish_markers(&self, item: &Described) -> Vec<Marker> {
        if item.engine != Some(crate::headers::ENGINE_SOURCE)
            || item.incomplete
            || item.segments.iter().any(|segment| !segment.ok())
        {
            return Vec::new();
        }
        let picked = source_render::plan_segments(&item.segments, None, 60.0).picked;
        let scans: Vec<Option<FinishScan>> = picked
            .iter()
            .map(|entry| {
                let segment = entry.segment;
                let key = format!(
                    "source|{}|{}|{:?}",
                    segment.path,
                    segment.size,
                    segment.duration()
                );
                if let Some(found) = lock(&self.scans).source.get(&key) {
                    return *found;
                }
                let scan =
                    source_demo::scan_file(Path::new(&segment.path), &|| self.is_disposed()).ok();
                let mut scans = lock(&self.scans);
                if scans.source.len() >= MAX_SCANS {
                    scans.source.clear();
                }
                scans.source.insert(key, scan);
                scan
            })
            .collect();
        source_demo::finish_markers(
            &picked,
            &scans,
            FinishOptions {
                incomplete: item.incomplete,
                run_time: item.run_time,
            },
        )
    }

    /// Метки рана, которых нет у адаптера рендера: главы HL2 и финиш (время демо).
    fn run_markers(&self, item: &Described) -> Vec<Marker> {
        if item.engine != Some(crate::headers::ENGINE_SOURCE) {
            return Vec::new();
        }
        let picked = source_render::plan_segments(&item.segments, None, 60.0).picked;
        let mut markers = source_demo::chapter_markers(&picked);
        markers.extend(self.finish_markers(item));
        markers
    }

    /// Разбор демки HL1 для меток (кэш по пути, размеру и длительности).
    fn goldsrc_scan(&self, segment: &library::Segment) -> Option<GoldsrcScan> {
        let key = format!("{}|{}|{:?}", segment.path, segment.size, segment.duration());
        if let Some(found) = lock(&self.scans).goldsrc.get(&key) {
            return found.clone();
        }
        let scan = goldsrc_demo::scan_demo(Path::new(&segment.path), &|| self.is_disposed()).ok();
        let mut scans = lock(&self.scans);
        if scans.goldsrc.len() >= MAX_SCANS {
            scans.goldsrc.clear();
        }
        scans.goldsrc.insert(key, scan.clone());
        scan
    }

    // ------------------------------------------------------------ FFmpeg

    fn tools(&self) -> Option<Arc<ToolService>> {
        (self.options.tools)()
    }

    fn tool_dir(&self) -> Option<PathBuf> {
        let tools = self.tools()?;
        tools.read_installed(FFMPEG)?;
        tools.install_path_of(FFMPEG).ok()
    }

    fn located(&self) -> Option<Located> {
        let tool_dir = (self.options.platform == Platform::Win32)
            .then(|| self.tool_dir())
            .flatten();
        ffmpeg::locate(
            self.options.platform,
            self.options.ffmpeg_override.as_deref(),
            tool_dir.as_deref(),
        )
    }

    /// `ffmpeg -version` на сессию (сбой не запоминается). Строже 0.4.9: файлы инструмента
    /// сверяются с манифестом до запуска.
    fn version_of(&self, located: &Located) -> Result<Version, DemoError> {
        if let Some(known) = self.state().versions.get(&located.ffmpeg) {
            return Ok(known.clone());
        }
        ffmpeg::verify(located)?;
        let version = ffmpeg::version(&located.ffmpeg, Some(&|| self.is_disposed()))?;
        self.state()
            .versions
            .insert(located.ffmpeg.clone(), version.clone());
        Ok(version)
    }

    fn installing(&self, tool: &str) -> Option<f64> {
        self.state()
            .install
            .filter(|(current, _)| *current == tool)
            .map(|(_, percent)| percent)
    }

    /// `ffmpegStatus()`.
    fn ffmpeg_status(&self) -> Value {
        let manifest = dependency_of(FFMPEG).expect("вшитый манифест FFmpeg");
        let installing = self.installing(FFMPEG);
        let origins: Vec<Value> = manifest["archive"]["providers"]
            .as_array()
            .map(|list| list.iter().map(origin_of).collect())
            .unwrap_or_default();
        let mut base = json!({
            "state": "not-installed",
            "code": null,
            "source": null,
            "version": null,
            "license": null,
            "pinnedVersion": manifest["version"],
            "build": manifest["build"],
            "pinnedLicense": manifest["license"],
            "installable": self.options.platform == Platform::Win32,
            "sizeBytes": manifest["archive"]["size"],
            // Откуда качает ToolService — источники манифеста по порядку.
            "origins": origins,
            "percent": js_number(installing.unwrap_or(0.0)),
        });
        let mut set = |fields: Value| {
            for (key, value) in fields.as_object().into_iter().flatten() {
                base[key] = value.clone();
            }
        };
        if installing.is_some() {
            set(json!({"state": "installing"}));
            return base;
        }
        let Some(located) = self.located() else {
            return base;
        };
        match self.version_of(&located) {
            Err(_) if located.source == "path" => set(json!({"code": "FFMPEG_NOT_IN_PATH"})),
            Err(_) => {
                set(json!({"state": "error", "code": "FFMPEG_BROKEN", "source": located.source}))
            }
            Ok(info) if located.source == "tool" && !info.pinned => set(json!({
                "state": "error", "code": "FFMPEG_VERSION_MISMATCH", "source": located.source,
                "version": info.version,
            })),
            Ok(info) => set(json!({
                "state": "ready", "source": located.source, "version": info.version,
                "license": info.license,
            })),
        }
        base
    }

    /// `requireFfmpeg()`.
    fn require_ffmpeg(&self) -> Result<Located, DemoError> {
        let status = self.ffmpeg_status();
        if status["state"] != "ready" {
            let code = status["code"]
                .as_str()
                .unwrap_or(if status["state"] == "installing" {
                    "FFMPEG_INSTALLING"
                } else {
                    "FFMPEG_MISSING"
                });
            return Err(DemoError::new(code));
        }
        self.located()
            .ok_or_else(|| DemoError::new("FFMPEG_MISSING"))
    }

    /// Скачать зависимость тем же `ToolService` (закреплённый sha256); прогресс — событием
    /// `channel`. `None` — установлено, `Some` — отменено (`{status: "cancelled"}`).
    async fn install_tool(
        &self,
        tool: &'static str,
        channel: &'static str,
        unsupported: &str,
    ) -> Result<Option<Value>, DemoError> {
        if self.options.platform != Platform::Win32 {
            return Err(DemoError::new(unsupported));
        }
        let tools = self
            .tools()
            .ok_or_else(|| DemoError::new("TOOLS_UNAVAILABLE"))?;
        {
            let mut state = self.state();
            if state.install.is_some() {
                return Err(DemoError::new("ALREADY_RUNNING"));
            }
            // Посреди экспорта ffmpeg.exe и bxt_rs.dll заняты: переустановка сломала бы задание.
            if state.job.is_some() {
                return Err(DemoError::new("JOB_BUSY"));
            }
            state.install = Some((tool, 0.0));
            // Новый FFmpeg — пробы кодировщиков и версии заново.
            if tool == FFMPEG {
                state.versions.clear();
                state.probe = None;
            }
        }
        // Окно закрыто посреди скачивания (future отменён) — иначе ALREADY_RUNNING до перезапуска.
        let installing = OnDrop(|| self.state().install = None);
        let me = self.me.clone();
        let progress = move |percent: f64, _bytes: u64| {
            let Some(api) = me.upgrade() else { return };
            {
                let mut state = api.state();
                match &mut state.install {
                    Some((_, current)) if *current != percent => *current = percent,
                    _ => return,
                }
            }
            api.emit(channel, json!({"percent": js_number(percent)}));
        };
        let installed = tools.install(tool, Some(&progress)).await;
        drop(installing);
        match installed {
            Ok(_) => Ok(None),
            Err(failure) => {
                let code = failure_code(&failure);
                if code.as_deref() == Some("CANCELLED") {
                    return Ok(Some(json!({"status": "cancelled"})));
                }
                self.log()
                    .warn(&format!("[demo] установка {tool}: {failure}"));
                let public = public_code(code.as_deref(), Some("DEMO_FAILED"));
                Err(DemoError::new(
                    public["code"].as_str().unwrap_or("DEMO_FAILED"),
                ))
            }
        }
    }

    async fn install_ffmpeg(&self) -> Reply {
        if let Some(cancelled) = self
            .install_tool(FFMPEG, FFMPEG_PROGRESS, "FFMPEG_INSTALL_UNSUPPORTED")
            .await?
        {
            return Ok(cancelled);
        }
        // Закреплённая сборка: первая строка ffmpeg -version содержит 9.0.2.
        self.blocking(|api| {
            let status = api.ffmpeg_status();
            if status["state"] != "ready" {
                api.log().warn(&format!(
                    "[demo] FFmpeg после установки не прошёл проверку: {}",
                    status["code"]
                ));
                if let Some(tools) = api.tools()
                    && let Err(error) = tools.remove(FFMPEG)
                {
                    api.log().warn(&format!("[demo] FFmpeg не удалён: {error}"));
                }
                return Err(DemoError::new(
                    status["code"].as_str().unwrap_or("FFMPEG_BROKEN"),
                ));
            }
            api.log()
                .info(&format!("[demo] FFmpeg {} установлен", status["version"]));
            Ok(json!({"status": "ok", "ffmpeg": status}))
        })
        .await
    }

    fn cancel_install(&self, tool: &str) -> Value {
        let cancelled = self.tools().is_some_and(|tools| tools.cancel(tool));
        json!({"status": "ok", "cancelled": cancelled})
    }

    fn bxtrs_located(&self) -> Option<(crate::goldsrc::Bxtrs, &'static str)> {
        let tools = self.tools();
        bxtrs_located(
            self.options.platform,
            self.options.bxtrs_override.as_deref(),
            tools.as_deref(),
        )
    }

    /// `bxtrsStatus()`.
    fn bxtrs_status(&self) -> Value {
        let manifest = dependency_of(BXTRS).expect("вшитый манифест bxt-rs");
        let installing = self.installing(BXTRS);
        let found = installing.is_none().then(|| self.bxtrs_located()).flatten();
        let extras: f64 = manifest["extras"]
            .as_array()
            .map(|list| list.iter().filter_map(|extra| extra["size"].as_f64()).sum())
            .unwrap_or(0.0);
        let size = extras + manifest["archive"]["size"].as_f64().unwrap_or(0.0);
        json!({
            "state": if installing.is_some() { "installing" } else if found.is_some() { "ready" } else { "not-installed" },
            "source": found.as_ref().map(|(_, source)| *source),
            "version": manifest["version"],
            "license": manifest["license"],
            "installable": self.options.platform == Platform::Win32,
            // DLL и рядом с ней текст лицензии и уведомления крейтов (manifest.extras).
            "sizeBytes": js_number(size),
            "percent": js_number(installing.unwrap_or(0.0)),
            // Ключи ссылок на исходник и лицензию (open-external-link).
            "sourceLink": "bxtrsSource",
            "licenseLink": "bxtrsLicense",
        })
    }

    async fn install_bxtrs(&self) -> Reply {
        if let Some(cancelled) = self
            .install_tool(BXTRS, BXTRS_PROGRESS, "BXTRS_INSTALL_UNSUPPORTED")
            .await?
        {
            return Ok(cancelled);
        }
        let status = self.bxtrs_status();
        if status["state"] != "ready" {
            return Err(DemoError::new("BXTRS_BROKEN"));
        }
        self.log()
            .info(&format!("[demo] bxt-rs {} установлен", status["version"]));
        Ok(json!({"status": "ok", "bxtrs": status}))
    }

    fn encoder_probe(&self, exe: &Path) -> Arc<EncoderProbe> {
        let mut state = self.state();
        if let Some((known, probe)) = &state.probe
            && known == exe
        {
            return probe.clone();
        }
        let me = self.me.clone();
        let cancel: Arc<dyn Fn() -> bool + Send + Sync> =
            Arc::new(move || me.upgrade().is_none_or(|api| api.is_disposed()));
        let probe = Arc::new(
            EncoderProbe::new(exe.to_path_buf(), Some(cancel))
                .with_logger(self.options.logger.clone()),
        );
        state.probe = Some((exe.to_path_buf(), probe.clone()));
        probe
    }

    fn encoders(&self) -> Reply {
        let located = self.require_ffmpeg()?;
        let summary = self.encoder_probe(&located.ffmpeg).summary();
        let mut out = Map::new();
        out.insert("status".to_owned(), json!("ok"));
        if let Value::Object(fields) = summary.to_value() {
            out.extend(fields);
        }
        Ok(Value::Object(out))
    }

    /// FPS, которые кодировщик тянет на этом разрешении: проба на максимальном FPS, при неудаче —
    /// на следующем ниже.
    fn supported_fps(
        probe: &EncoderProbe,
        encoder: &str,
        resolution: Resolution,
        fps_list: &[u32],
    ) -> Vec<u32> {
        let mut sorted = fps_list.to_vec();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        for fps in sorted {
            if probe.works(encoder, resolution.width, resolution.height, fps) {
                return fps_list
                    .iter()
                    .copied()
                    .filter(|value| *value <= fps)
                    .collect();
            }
        }
        Vec::new()
    }

    fn combos(probe: &EncoderProbe, encoder: &str, caps: &Caps) -> Vec<Value> {
        let fps_list: Vec<u32> = planner::FPS
            .into_iter()
            .filter(|fps| caps.fps.contains(fps))
            .collect();
        let allowed: HashMap<u32, Vec<u32>> = RESOLUTIONS
            .into_iter()
            .filter(|resolution| resolution.height <= caps.max_height)
            .map(|resolution| {
                let fps = Self::supported_fps(probe, encoder, resolution, &fps_list);
                (resolution.height, fps)
            })
            .collect();
        let supports =
            |height: u32, fps: u32| allowed.get(&height).is_some_and(|list| list.contains(&fps));
        planner::allowed_combos(&caps.planner(), Some(&supports))
            .into_iter()
            .map(|combo| json!({"height": combo.height, "width": combo.width, "fps": combo.fps}))
            .collect()
    }

    fn encoder_of(mode: &str, summary: &Summary) -> Option<&'static str> {
        match mode {
            "gpu" => summary.gpu,
            "cpu" => summary.cpu,
            _ => summary.auto,
        }
    }

    fn resolve_encoder(mode: &str, summary: &Summary) -> Result<&'static str, DemoError> {
        Self::encoder_of(mode, summary).ok_or_else(|| {
            DemoError::new(if mode == "gpu" {
                "ENCODER_GPU_UNAVAILABLE"
            } else {
                "ENCODER_UNAVAILABLE"
            })
        })
    }

    // ------------------------------------------------------------ адаптеры, план

    fn verdict(&self, item: &Described) -> Verdict {
        if !item.supported {
            return Verdict {
                adapter: None,
                ok: false,
                code: Some(item.reason.unwrap_or("DEMO_UNSUPPORTED").to_owned()),
            };
        }
        pick_adapter(item, &self.adapters)
    }

    /// Настройки экспорта из Store поверх умолчаний; негодные — умолчания.
    fn saved_settings(&self) -> Value {
        let saved = self.store_get("export");
        let mut out = planner::defaults(false);
        for key in SAVED_KEYS {
            if let Some(value) = saved.as_ref().and_then(|saved| saved.get(key)) {
                out[key] = value.clone();
            }
        }
        if planner::normalize_settings(&out, Some(1.0), false).is_ok() {
            out
        } else {
            planner::defaults(false)
        }
    }

    fn options(&self, id: &Value) -> Reply {
        let item = self.resolve(id)?;
        let verdict = self.verdict(&item);
        let caps = verdict.caps();
        let status = self.ffmpeg_status();
        let mut modes = Value::Null;
        if status["state"] == "ready"
            && let Some(located) = self.located()
        {
            let probe = self.encoder_probe(&located.ffmpeg);
            let summary = probe.summary();
            let mut out = Map::new();
            for mode in ENCODER_MODES {
                let value = Self::encoder_of(mode, &summary).map_or(Value::Null, |encoder| {
                    json!({"encoder": encoder, "combos": Self::combos(&probe, encoder, &caps)})
                });
                out.insert(mode.to_owned(), value);
            }
            modes = Value::Object(out);
        }
        let mut capabilities = Map::new();
        capabilities.insert("maxHeight".to_owned(), json!(caps.max_height));
        capabilities.insert("fps".to_owned(), json!(caps.fps));
        capabilities.insert("audio".to_owned(), json!(caps.has("audio")));
        capabilities.insert("integrity".to_owned(), json!(caps.has("integrity")));
        capabilities.extend(view_caps(Some(&caps)));
        let resolutions: Vec<Value> = RESOLUTIONS
            .iter()
            .filter(|resolution| resolution.height <= caps.max_height)
            .map(|r| json!({"height": r.height, "width": r.width, "label": r.label}))
            .collect();
        Ok(json!({
            "status": "ok",
            "id": id,
            "duration": item.duration.map_or(Value::Null, js_number),
            "render": verdict.render_value(),
            "ffmpeg": {"state": status["state"], "code": status["code"]},
            "capabilities": capabilities,
            "keyhud": if caps.has("keyhud") {
                json!({"available": true, "code": null})
            } else {
                json!({"available": false, "code": "KEYHUD_NOT_AVAILABLE"})
            },
            "resolutions": resolutions,
            "speed": {"min": js_number(planner::SPEED_MIN), "max": js_number(planner::SPEED_MAX)},
            "presets": planner::PRESETS,
            "discordLimitBytes": js_number(self.options.limit_bytes.unwrap_or(planner::DISCORD_LIMIT_BYTES)),
            "modes": modes,
            "defaults": self.saved_settings(),
        }))
    }

    /// Общая подготовка плана: проверка возможностей адаптера и кодировщика.
    fn prepare(
        &self,
        item: &Described,
        raw: &Value,
        allow_preview: bool,
    ) -> Result<Prepared, DemoError> {
        let mut settings = planner::normalize_settings(raw, item.duration, allow_preview)?;
        let located = self.require_ffmpeg()?;
        let verdict = self.verdict(item);
        let caps = verdict.caps();
        let mut warnings = Vec::new();
        if settings.keyhud && !caps.has("keyhud") {
            settings.keyhud = false;
            warnings.push("KEYHUD_UNAVAILABLE".to_owned());
        }
        // Надпись проверки данных — только у адаптеров, которые её умеют (Ghosting).
        if settings.integrity && !caps.has("integrity") {
            settings.integrity = false;
        }
        // Выключить HUD, взять стандартный конфиг или HUD — только где игра это умеет.
        if !settings.hud && !caps.has("hud") {
            settings.hud = true;
        }
        if !settings.own_config && !caps.has("ownConfig") {
            settings.own_config = true;
        }
        if !settings.own_hud && !caps.has("ownHud") {
            settings.own_hud = true;
        }
        if settings.audio && !caps.has("audio") {
            settings.audio = false;
            warnings.push("AUDIO_UNAVAILABLE".to_owned());
        }
        if settings.height > caps.max_height || !caps.fps.contains(&settings.fps) {
            return Err(DemoError::new("COMBO_UNSUPPORTED"));
        }
        let probe = self.encoder_probe(&located.ffmpeg);
        let encoder = Self::resolve_encoder(&settings.encoder, &probe.summary())?;
        let resolution = planner::resolution_of(settings.height).expect("высота из списка");
        if !Self::supported_fps(&probe, encoder, resolution, &caps.fps).contains(&settings.fps) {
            return Err(
                DemoError::new("COMBO_UNSUPPORTED").with_details(json!({"encoder": encoder}))
            );
        }
        let output_duration = planner::clip_duration(&Clip {
            from: settings.range.from,
            to: settings.range.to,
            speed: settings.speed,
            offset: None,
            cuts: Vec::new(),
        });
        let plan = planner::plan_export(
            &settings,
            &PlanOptions {
                encoder: Some(encoder.to_owned()),
                output_duration,
                limit_bytes: self.options.limit_bytes,
                warnings,
            },
        )?;
        Ok(Prepared {
            settings,
            plan,
            verdict,
            located,
        })
    }

    fn plan(&self, id: &Value, raw: &Value) -> Reply {
        let item = self.resolve(id)?;
        let prepared = self.prepare(&item, raw, false)?;
        Ok(json!({
            "status": "ok",
            "plan": prepared.plan.to_value(),
            "render": prepared.verdict.render_value(),
        }))
    }

    // ------------------------------------------------------------ оценка времени

    /// Скорости прошлых заданий: `{render: {"<профиль>@<высота>": {rate, frames}}, encode: {…}}`.
    fn learned_speed(&self) -> Value {
        self.store_get("speed")
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}))
    }

    /// Кодировщик, которым пойдёт задание: из проб FFmpeg; без FFmpeg — процессор.
    fn encoder_guess(&self, mode: &str) -> &'static str {
        let status = self.ffmpeg_status();
        if status["state"] != "ready" {
            return CPU_ENCODER;
        }
        self.located()
            .and_then(|located| {
                Self::resolve_encoder(mode, &self.encoder_probe(&located.ffmpeg).summary()).ok()
            })
            .unwrap_or(CPU_ENCODER)
    }

    /// Оценка одного задания: `raw` — настройки экспорта или `{preset: "preview", range}`.
    fn estimate_for(&self, item: &Described, raw: &Value) -> Reply {
        let preview = raw["preset"] == "preview";
        let input = if preview {
            let mut input = planner::defaults(true);
            input["encoder"] = json!("auto");
            input["range"] = raw
                .get("range")
                .filter(|range| truthy(range))
                .cloned()
                .unwrap_or(Value::Null);
            input
        } else {
            raw.clone()
        };
        let settings = planner::normalize_settings(&input, item.duration, preview)?;
        let verdict = self.verdict(item);
        let profile = verdict
            .adapter
            .as_ref()
            .and_then(|adapter| adapter.estimate_profile(item, settings.range, settings.fps))
            .unwrap_or(EstimateProfile {
                key: "other",
                launches: 1.0,
                segments: item.segments.len() as f64,
            });
        let seconds = settings.range.to - settings.range.from;
        let encoder = self.encoder_guess(&settings.encoder);
        let result = estimate::estimate(
            &Input {
                profile: profile.key.to_owned(),
                launches: Some(profile.launches),
                segments: Some(profile.segments),
                seconds,
                fps: f64::from(settings.fps),
                width: f64::from(settings.width),
                height: f64::from(settings.height),
                output_seconds: seconds / settings.speed,
                preset: settings.preset.clone(),
                gpu: encoder != CPU_ENCODER,
            },
            &self.learned_speed(),
        );
        let mut out = result.to_value();
        out["range"] = js_number(math_round(seconds * 10.0) / 10.0);
        out["segments"] = js_number(if profile.segments == 0.0 || profile.segments.is_nan() {
            1.0
        } else {
            profile.segments
        });
        Ok(out)
    }

    /// Окно спрашивает несколько вариантов сразу (предпросмотр, «Лучшее качество», Discord).
    fn estimate(&self, id: &Value, list: &Value) -> Reply {
        let item = self.resolve(id)?;
        let Some(list) = list.as_array().filter(|list| (1..=8).contains(&list.len())) else {
            return Err(
                DemoError::new("SETTINGS_INVALID").with_details(json!({"field": "estimate"}))
            );
        };
        let items = list
            .iter()
            .map(|raw| self.estimate_for(&item, raw))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({
            "status": "ok",
            "items": items,
            "threshold": js_number(estimate::PREVIEW_PROMPT_SECONDS),
        }))
    }

    // ------------------------------------------------------------ задания

    fn default_output_dir(&self) -> String {
        NodePath::native().join(&[&*self.options.videos_dir.to_string_lossy(), "HLSR"])
    }

    fn output_dir(&self) -> String {
        match self.store_get("outputDir") {
            Some(Value::String(saved))
                if !saved.is_empty() && NodePath::native().is_absolute(&saved) =>
            {
                saved
            }
            _ => self.default_output_dir(),
        }
    }

    fn settings(&self) -> Value {
        json!({
            "status": "ok",
            "settings": self.saved_settings(),
            "outputDir": self.output_dir(),
            "defaultOutputDir": self.default_output_dir(),
            "preview": self.preview_view(),
        })
    }

    async fn pick_output_dir(&self) -> Reply {
        let picked = self
            .options
            .dialogs
            .show_open_dialog(OpenDialog {
                title: None,
                default_path: Some(self.output_dir()),
                properties: vec!["openDirectory", "createDirectory"],
                filters: Vec::new(),
            })
            .await;
        let Some(dir) = picked.first() else {
            return Ok(json!({"status": "cancelled"}));
        };
        let resolved = NodePath::native().resolve(&[&*dir.to_string_lossy()]);
        self.store_set("outputDir", json!(resolved));
        Ok(json!({"status": "ok", "outputDir": self.output_dir()}))
    }

    /// Вид предпросмотра (KeyHUD, HUD, конфиг и HUD игрока): по умолчанию всё включено, выбор
    /// запоминается.
    fn preview_view(&self) -> Map<String, Value> {
        let saved = self.store_get("preview").unwrap_or(Value::Null);
        VIEW_KEYS
            .iter()
            .map(|key| {
                let value = saved.get(key).filter(|v| v.is_boolean()).cloned();
                ((*key).to_owned(), value.unwrap_or(Value::Bool(true)))
            })
            .collect()
    }

    /// Предпросмотр: 720p30, быстрое кодирование, со звуком; из окна — отрезок и вид в кадре
    /// (выбор запоминается), из настроек экспорта — надпись проверки данных.
    fn preview_input(&self, raw: &Value) -> Value {
        let range = raw
            .get("range")
            .filter(|range| truthy(range))
            .cloned()
            .unwrap_or(Value::Null);
        let mut view = self.preview_view();
        let mut chosen = false;
        for key in VIEW_KEYS {
            if let Some(value) = raw.get(key).filter(|value| value.is_boolean()) {
                view.insert(key.to_owned(), value.clone());
                chosen = true;
            }
        }
        if chosen {
            self.store_set("preview", Value::Object(view.clone()));
        }
        let mut input = planner::defaults(true);
        for (key, value) in view {
            input[key] = value;
        }
        input["encoder"] = json!("auto");
        input["range"] = range;
        input["integrity"] = self.saved_settings()["integrity"].clone();
        input
    }

    /// `start(kind, id, raw)`: план, затем задание в фоне; ответ — `{status, jobId, plan}`.
    async fn start(&self, kind: JobKind, id: Value, raw: Value) -> Reply {
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut state = self.state();
            if state.job.is_some() {
                return Err(DemoError::new("JOB_BUSY"));
            }
            if matches!(state.install, Some((FFMPEG, _))) {
                return Err(DemoError::new("FFMPEG_INSTALLING"));
            }
            state.job = Some(Job {
                id: None,
                kind,
                cancel: cancel.clone(),
            });
        }
        // Задание снимается и когда этот future отменён, и при панике рендера: иначе JOB_BUSY до
        // перезапуска лаунчера.
        let me = self.me.clone();
        let slot = OnDrop(move || {
            if let Some(api) = me.upgrade() {
                api.state().job = None;
            }
        });
        let job_cancel = cancel.clone();
        let prepared = self
            .blocking(move |api| {
                let cancelled = || {
                    if job_cancel.load(Ordering::SeqCst) {
                        Err(DemoError::new("CANCELLED"))
                    } else {
                        Ok(())
                    }
                };
                jobs::prepare_root(api.options.jobs_root.as_deref())?;
                let item = api.resolve(&id)?;
                let prepared = api.prepare(&item, &raw, kind == JobKind::Preview)?;
                if !prepared.verdict.ok {
                    return Err(DemoError::new(
                        prepared
                            .verdict
                            .code
                            .as_deref()
                            .unwrap_or("DEMO_UNSUPPORTED"),
                    ));
                }
                // Установка bxt-rs переписывает bxt_rs.dll, которую грузит рендер HL1 (строже 0.4.9).
                let hl1 = prepared.verdict.adapter.as_ref().and_then(|a| a.game_id())
                    == Some(crate::goldsrc::GAME_ID);
                if hl1 && api.installing(BXTRS).is_some() {
                    return Err(DemoError::new("JOB_BUSY"));
                }
                cancelled()?;
                let output_dir = (kind == JobKind::Export).then(|| api.output_dir());
                if let Some(dir) = &output_dir {
                    // Без домашней папки умолчание «Videos/HLSR» относительное: не в текущую папку.
                    if !NodePath::native().is_absolute(dir) || fs::create_dir_all(dir).is_err() {
                        return Err(DemoError::new("OUTPUT_DIR_UNAVAILABLE"));
                    }
                    let settings = prepared.settings.to_value();
                    let remembered: Map<String, Value> = SAVED_KEYS
                        .iter()
                        .map(|key| ((*key).to_owned(), settings[key].clone()))
                        .collect();
                    api.store_set("export", Value::Object(remembered));
                } else {
                    api.drop_preview();
                }
                // Главы и финиш Source — из карт и самих демок, рендер их не знает.
                let extra = api.run_markers(&item);
                cancelled()?;
                Ok((item, prepared, output_dir, extra))
            })
            .await;
        let (item, prepared, output_dir, extra) = prepared?;
        let job_id = random_hex(8);
        if let Some(job) = &mut self.state().job {
            job.id = Some(job_id.clone());
        }
        let plan = &prepared.plan;
        self.log().info(&format!(
            "[demo] задание {job_id} ({}): {} {}, {}x{}@{}, {}, {}",
            kind.as_str(),
            item.entry.kind,
            item.entry.name,
            plan.width,
            plan.height,
            plan.fps,
            plan.encoder,
            plan.preset
        ));
        let reply = json!({"status": "ok", "jobId": job_id, "plan": plan.to_value()});
        let Some(api) = self.me.upgrade() else {
            return Err(DemoError::new("DEMO_FAILED"));
        };
        tokio::task::spawn_blocking(move || {
            let run = || {
                api.run(
                    &job_id,
                    kind,
                    &item,
                    &prepared,
                    output_dir.as_deref(),
                    &extra,
                    &cancel,
                )
            };
            // Паника рендера — тоже JOB_ERROR: окно не висит в «идёт».
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
                .unwrap_or_else(|_| Err(DemoError::new("DEMO_FAILED")));
            drop(slot);
            match result {
                Ok(result) => api.finished(&job_id, kind, result),
                Err(error) => api.failed(&job_id, kind, &error),
            }
        });
        Ok(reply)
    }

    #[allow(clippy::too_many_arguments)]
    fn run(
        &self,
        job_id: &str,
        kind: JobKind,
        item: &Described,
        prepared: &Prepared,
        output_dir: Option<&str>,
        extra: &[Marker],
        cancel: &AtomicBool,
    ) -> Result<JobResult, DemoError> {
        let root = self
            .options
            .jobs_root
            .as_deref()
            .ok_or_else(|| DemoError::new("JOBS_DIR_UNAVAILABLE"))?;
        let adapter: Arc<dyn RenderGame> = prepared
            .verdict
            .adapter
            .clone()
            .ok_or_else(|| DemoError::new("DEMO_UNSUPPORTED"))?;
        let stop = || cancel.load(Ordering::SeqCst) || self.is_disposed();
        let progress = |progress: &jobs::Progress| {
            let mut payload = json!({"jobId": job_id, "kind": kind.as_str()});
            if let Value::Object(fields) = progress.to_value() {
                payload.as_object_mut().expect("объект").extend(fields);
            }
            self.emit(JOB_PROGRESS, payload);
        };
        let output_dir = output_dir.map(Path::new);
        jobs::run_job(&JobOptions {
            id: job_id,
            kind,
            item,
            adapter: adapter.as_ref() as &dyn Adapter,
            settings: &prepared.settings,
            plan: &prepared.plan,
            root,
            output_dir,
            extra_markers: extra,
            ffmpeg: &prepared.located,
            cancel: &stop,
            on_progress: Some(&progress),
            on_spawn: None,
            logger: Some(self.log()),
            run_fn: None,
            ffprobe_fn: None,
        })
    }

    fn finished(&self, job_id: &str, kind: JobKind, result: JobResult) {
        if result.stats.profile.is_some() {
            let learned = estimate::learn(&self.learned_speed(), Some(&result.stats));
            self.store_set("speed", learned);
        }
        let (token, url) = self.media.register(&result.file);
        let export = kind == JobKind::Export;
        let file_name = result
            .file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        let payload = json!({
            "jobId": job_id,
            "kind": kind.as_str(),
            "result": {
                "id": job_id,
                "kind": kind.as_str(),
                "file": export.then(|| result.file.to_string_lossy().into_owned()),
                "fileName": if export { file_name } else { None },
                "timelineFile": result.timeline_file.as_ref()
                    .and_then(|file| file.file_name())
                    .map(|name| name.to_string_lossy().into_owned()),
                "size": result.size,
                "attempts": result.attempts,
                "encoder": result.encoder,
                "warnings": result.warnings,
                "video": result.timeline["video"],
                "markers": result.timeline["markers"],
                "mediaUrl": url,
            },
        });
        {
            let mut state = self.state();
            if kind == JobKind::Preview {
                state.preview = Some(Preview {
                    job_id: job_id.to_owned(),
                    dir: result.dir.clone(),
                    token: token.clone(),
                });
            }
            state.results.insert(
                job_id.to_owned(),
                Stored {
                    kind,
                    file: result.file.clone(),
                    timeline_file: result.timeline_file.clone(),
                    timeline: result.timeline.clone(),
                    token,
                    url,
                },
            );
        }
        self.log().info(&format!(
            "[demo] задание {job_id} готово: {} байт, {}, попыток {}",
            result.size, result.encoder, result.attempts
        ));
        self.emit(JOB_DONE, payload);
        if export {
            (self.options.achieve)("video-editor");
        }
    }

    fn failed(&self, job_id: &str, kind: JobKind, error: &DemoError) {
        if error.code == "CANCELLED" {
            self.log()
                .info(&format!("[demo] задание {job_id} отменено"));
        } else {
            self.log().warn(&format!(
                "[demo] задание {job_id} не удалось: {} {}",
                error.code, error.message
            ));
        }
        self.emit(
            JOB_ERROR,
            json!({"jobId": job_id, "kind": kind.as_str(), "error": error.to_public()}),
        );
    }

    /// С `jobId` — только это задание (поздняя отмена старого не гасит новое, пока у того ещё нет
    /// id); без него — текущее, в том числе готовящееся.
    fn cancel(&self, job_id: &Value) -> Value {
        let state = self.state();
        let Some(job) = &state.job else {
            return json!({"status": "ok", "cancelled": false});
        };
        if truthy(job_id) && (job_id.as_str().is_none() || job_id.as_str() != job.id.as_deref()) {
            return json!({"status": "ok", "cancelled": false});
        }
        job.cancel.store(true, Ordering::SeqCst);
        json!({"status": "ok", "cancelled": true})
    }

    /// Предпросмотр хранится один: при новом, при уходе со страницы и при выходе удаляется.
    fn clear_preview(&self) -> Value {
        if let Some(job) = &self.state().job
            && job.kind == JobKind::Preview
        {
            job.cancel.store(true, Ordering::SeqCst);
        }
        self.drop_preview()
    }

    fn drop_preview(&self) -> Value {
        let preview = {
            let mut state = self.state();
            let preview = state.preview.take();
            if let Some(preview) = &preview {
                state.results.remove(&preview.job_id);
            }
            preview
        };
        let Some(preview) = preview else {
            return json!({"status": "ok", "cleared": false});
        };
        self.media.unregister(&preview.token);
        if let (Some(dir), Some(root)) = (&preview.dir, &self.options.jobs_root) {
            jobs::remove_job_dir(root, dir);
        }
        json!({"status": "ok", "cleared": true})
    }

    /// Итог задания по id: файл, вид и `timeline.json`.
    fn result<T>(&self, id: &Value, read: impl FnOnce(&Stored) -> T) -> Result<T, DemoError> {
        let state = self.state();
        id.as_str()
            .and_then(|id| state.results.get(id))
            .map(read)
            .ok_or_else(|| DemoError::new("RESULT_NOT_FOUND"))
    }

    fn media_url(&self, id: &Value) -> Reply {
        let url = self.result(id, |found| found.url.clone())?;
        Ok(json!({"status": "ok", "url": url}))
    }

    fn reveal(&self, id: &Value) -> Reply {
        let (kind, file) = self.result(id, |found| (found.kind, found.file.clone()))?;
        if kind != JobKind::Export || !file.exists() {
            return Err(DemoError::new("RESULT_NOT_FOUND"));
        }
        self.options.shell.show_item_in_folder(&file);
        Ok(json!({"status": "ok"}))
    }

    fn read_timeline(&self, id: &Value) -> Reply {
        let (file, timeline) = self.result(id, |found| {
            (found.timeline_file.clone(), found.timeline.clone())
        })?;
        let timeline = match file {
            None => timeline,
            Some(file) => read_timeline_file(&file)?,
        };
        Ok(json!({"status": "ok", "timeline": timeline}))
    }

    // ------------------------------------------------------------ отрендеренные

    /// Имя из окна — только имя файла `.mp4` (без папок, «..», «:» потоков NTFS), а файл — прямо
    /// в папке вывода; рядом его `<имя>.timeline.json`.
    fn rendered_paths(&self, name: &Value) -> Result<RenderedPaths, DemoError> {
        let invalid = || DemoError::new("PATH_INVALID");
        let name = name.as_str().ok_or_else(invalid)?;
        let path = NodePath::native();
        let forbidden = |c: char| "<>:\"/\\|?*".contains(c) || u32::from(c) < 0x20;
        if utf16_len(name) > 255
            || !name.to_ascii_lowercase().ends_with(".mp4")
            || name.chars().any(forbidden)
            || path.basename(name) != name
        {
            return Err(invalid());
        }
        let dir = path.resolve(&[&self.output_dir()]);
        let file = path.join(&[&dir, name]);
        if path.dirname(&file) != dir {
            return Err(invalid());
        }
        let timeline = path.join(&[&dir, &format!("{}.timeline.json", &name[..name.len() - 4])]);
        Ok(RenderedPaths { file, timeline })
    }

    /// Список вкладки: только видео со своим `timeline.json` (наши итоги), новые сверху.
    fn rendered_list(&self) -> Value {
        let dir = self.output_dir();
        let mut videos: Vec<(String, RenderedPaths, u64, f64)> = Vec::new();
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
            if !is_file || !name.to_ascii_lowercase().ends_with(".mp4") {
                continue;
            }
            let Ok(paths) = self.rendered_paths(&json!(name)) else {
                continue;
            };
            let Ok(meta) = fs::metadata(&paths.file) else {
                continue;
            };
            videos.push((name, paths, meta.len(), mtime_ms(&meta)));
        }
        let collator = Collator::new(&default_locale(), Sensitivity::Variant);
        videos.sort_by(|a, b| {
            b.3.partial_cmp(&a.3)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| collator.compare(&a.0, &b.0))
        });
        let items: Vec<Value> = videos
            .iter()
            .filter_map(|(name, paths, size, mtime)| {
                let timeline =
                    public_timeline(&read_timeline_file(Path::new(&paths.timeline)).ok()?);
                Some(json!({
                    "name": name,
                    "size": size,
                    "mtimeMs": js_number(*mtime),
                    "video": timeline["video"],
                    "source": timeline["source"],
                    "markerCount": timeline["markers"].as_array().map_or(0, Vec::len),
                }))
            })
            .take(MAX_RENDERED)
            .collect();
        json!({"status": "ok", "outputDir": dir, "items": items})
    }

    /// Файл видео прямо в папке вывода (не ссылка) и его `timeline.json`.
    fn rendered_timeline(paths: &RenderedPaths) -> Result<Value, DemoError> {
        match fs::symlink_metadata(&paths.file) {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => return Err(DemoError::new("RESULT_NOT_FOUND")),
            Err(error) if is_enoent(&error) => return Err(DemoError::new("RESULT_NOT_FOUND")),
            Err(_) => return Err(DemoError::new("TIMELINE_INVALID")),
        }
        read_timeline_file(Path::new(&paths.timeline))
    }

    /// Видео для плеера вкладки: адрес `hlsr-media` (один на вкладку) и метки из `timeline.json`.
    fn rendered_open(&self, name: &Value) -> Reply {
        let paths = self.rendered_paths(name)?;
        let raw = Self::rendered_timeline(&paths)?;
        let timeline = public_timeline(&raw);
        self.drop_rendered();
        let (token, url) = self.media.register(Path::new(&paths.file));
        self.state().rendered = Some((token, paths.file.clone()));
        Ok(json!({
            "status": "ok",
            "name": name,
            "url": url,
            "video": timeline["video"],
            "source": timeline["source"],
            "markers": timeline["markers"],
            "sourceId": self.rendered_source_id(&raw),
        }))
    }

    /// Исходная демка видео (кнопка «Экспорт» во вкладке): по id из `timeline.json`, а у видео
    /// старых сборок — по именам файлов, игре и карте, если такая запись в списке одна.
    fn rendered_source_id(&self, timeline: &Value) -> Value {
        let empty = Value::Null;
        let source = object_like(timeline.get("source")).unwrap_or(&empty);
        let entries = self.entries();
        if let Some(id) = source.get("id").and_then(Value::as_str)
            && entries.iter().any(|entry| entry.id == id)
        {
            return json!(id);
        }
        let Some(files) = source
            .get("files")
            .and_then(Value::as_array)
            .filter(|files| !files.is_empty())
            .map(|files| {
                files
                    .iter()
                    .map(hlsr_core::js::to_string)
                    .collect::<Vec<_>>()
            })
        else {
            return Value::Null;
        };
        let falsy_or = |key: &str| {
            source
                .get(key)
                .filter(|value| truthy(value))
                .map(hlsr_core::js::to_string)
        };
        let (game, map) = (falsy_or("game"), falsy_or("map"));
        let path = NodePath::native();
        let matches: Vec<&Entry> = entries
            .iter()
            .filter(|entry| {
                entry
                    .files
                    .first()
                    .is_some_and(|first| path.basename(first) == files[0])
            })
            .filter(|entry| {
                let item = library::describe(entry);
                item.game.map(str::to_owned) == game
                    && item.map == map
                    && item.source_files.as_ref() == Some(&files)
            })
            .collect();
        match matches.as_slice() {
            [only] => json!(only.id),
            _ => Value::Null,
        }
    }

    /// «Открыть папку»: Проводник с выделенным видео. Строже 0.4.9 — только у видео со своим
    /// `timeline.json`.
    fn rendered_reveal(&self, name: &Value) -> Reply {
        let paths = self.rendered_paths(name)?;
        Self::rendered_timeline(&paths)?;
        self.options
            .shell
            .show_item_in_folder(Path::new(&paths.file));
        Ok(json!({"status": "ok"}))
    }

    fn drop_rendered(&self) {
        if let Some((token, _)) = self.state().rendered.take() {
            self.media.unregister(&token);
        }
    }

    /// В Корзину видео и его `timeline.json` (подтверждает окно). Строже 0.4.9 — только видео со
    /// своим `timeline.json`. Ответ — новый список.
    fn rendered_delete(&self, name: &Value) -> Reply {
        let paths = self.rendered_paths(name)?;
        Self::rendered_timeline(&paths)?;
        let is_file = |file: &str| fs::symlink_metadata(file).is_ok_and(|meta| meta.is_file());
        let open = self
            .state()
            .rendered
            .as_ref()
            .is_some_and(|(_, file)| *file == paths.file);
        if open {
            self.drop_rendered();
        }
        let trashed = (self.options.trash)(Path::new(&paths.file)).and_then(|()| {
            if is_file(&paths.timeline) {
                (self.options.trash)(Path::new(&paths.timeline))
            } else {
                Ok(())
            }
        });
        if let Err(message) = trashed {
            self.log().warn(&format!(
                "[demo] {} не убран в Корзину: {message}",
                hlsr_core::js::to_string(name)
            ));
            return Err(DemoError {
                message,
                ..DemoError::new("TRASH_FAILED")
            });
        }
        self.log().info(&format!(
            "[demo] {} и его метки убраны в Корзину",
            hlsr_core::js::to_string(name)
        ));
        // Итог экспорта с этим файлом больше не отдаётся (иначе плеер «Демок» открыл бы удалённое).
        let path = NodePath::native();
        let gone: Vec<String> = {
            let mut state = self.state();
            let ids: Vec<String> = state
                .results
                .iter()
                .filter(|(_, result)| {
                    path.resolve(&[&*result.file.to_string_lossy()]) == paths.file
                })
                .map(|(id, _)| id.clone())
                .collect();
            ids.iter()
                .filter_map(|id| state.results.remove(id))
                .map(|result| result.token)
                .collect()
        };
        for token in gone {
            self.media.unregister(&token);
        }
        let mut out = self.rendered_list();
        out["deleted"] = json!(paths.file);
        Ok(out)
    }

    // ------------------------------------------------------------ рендер игр

    fn renderer(&self, game_id: &str) -> Option<&Arc<dyn RenderGame>> {
        self.options
            .renderers
            .iter()
            .find(|renderer| renderer.game_id() == Some(game_id))
    }

    /// Идёт рендер этой игры: её запуск из лаунчера отказывает (`RENDER_ACTIVE`).
    pub fn render_active(&self, game_id: &str) -> bool {
        self.renderer(game_id)
            .is_some_and(|renderer| renderer.is_active())
    }

    /// Mute игры, запомненный Windows после рендера, снимается в фоне по одной игре, пока флаг
    /// стоит и ничто не мешает. Неудачный сброс до перезапуска лаунчера не повторяется: игра не
    /// стартует на скрытом столе каждую минуту.
    pub fn reset_pending_mutes(&self) {
        let mut gave_up = lock(&self.mute);
        for renderer in &self.options.renderers {
            let Some(game_id) = renderer.game_id() else {
                continue;
            };
            let busy = self.state().job.is_some()
                || self.options.renderers.iter().any(|other| other.is_active());
            if busy || gave_up.contains(game_id) || renderer.mute_reset_blocker().is_some() {
                continue;
            }
            if !renderer.reset_mute() {
                gave_up.insert(game_id);
            }
        }
    }

    /// Перед запуском игры из лаунчера: флаг стоит — короткий сброс (фоновый сначала доходит до
    /// конца), иначе игрок остался бы без звука. Ответ — mute снят.
    pub fn reset_mute_before_launch(&self, game_id: &str) -> bool {
        let Some(renderer) = self.renderer(game_id) else {
            return false;
        };
        let _one_at_a_time = lock(&self.mute);
        renderer.mute_reset_blocker().is_none() && renderer.reset_mute()
    }

    /// Через [`BACKGROUND_DELAY`] — папки рендера, оставшиеся в играх после сбоя (путь к игре —
    /// из библиотеки), и фоновый сброс mute раз в [`MUTE_RESET_INTERVAL`].
    pub fn start_background(&self, runtime: &tokio::runtime::Handle) {
        let me = self.me.clone();
        runtime.spawn(async move {
            tokio::time::sleep(BACKGROUND_DELAY).await;
            let step = |work: fn(&DemoApi)| {
                let me = me.clone();
                async move {
                    let Some(api) = me.upgrade().filter(|api| !api.is_disposed()) else {
                        return false;
                    };
                    tokio::task::spawn_blocking(move || work(&api))
                        .await
                        .is_ok()
                }
            };
            if !step(DemoApi::cleanup_render_dirs).await {
                return;
            }
            while step(DemoApi::reset_pending_mutes).await {
                tokio::time::sleep(MUTE_RESET_INTERVAL).await;
            }
        });
    }

    fn cleanup_render_dirs(&self) {
        for renderer in &self.options.renderers {
            let removed = renderer.cleanup_stale();
            if removed > 0 {
                self.log()
                    .info(&format!("[demo] удалены папки рендера в игре: {removed}"));
            }
        }
    }

    /// Выход лаунчера: задание отменяется, рендер игры закрывает её и возвращает реестр
    /// (синхронно, до 20 с), предпросмотр удаляется. Полный экран плеера снимает само окно.
    pub fn dispose(&self) {
        self.disposed.store(true, Ordering::SeqCst);
        if let Some(job) = &self.state().job {
            job.cancel.store(true, Ordering::SeqCst);
        }
        for renderer in &self.options.renderers {
            renderer.dispose();
        }
        self.clear_preview();
        self.drop_rendered();
        self.media.clear();
    }
}

fn records_value(records: &[Record]) -> Value {
    Value::Array(records.iter().map(Record::to_value).collect())
}

fn mtime_ms(meta: &fs::Metadata) -> f64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0.0, |since| since.as_secs_f64() * 1000.0)
}

/// Код ошибки доставки (`error.code`).
fn failure_code(failure: &Failure) -> Option<String> {
    match failure {
        Failure::Delivery(error) => Some(error.code.as_str().to_owned()),
        Failure::Raw(raw) => raw.code.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_of_github_and_site() {
        assert_eq!(
            origin_of(
                &json!({"url": "https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg.zip"})
            ),
            json!({"site": "github.com/GyanD/codexffmpeg", "release": "9.0.2"})
        );
        assert_eq!(
            origin_of(&json!({"url": "https://www.gyan.dev/ffmpeg/builds/x.zip"})),
            json!({"site": "www.gyan.dev", "release": null})
        );
        assert_eq!(origin_of(&json!({"url": "nope"})), Value::Null);
    }

    #[test]
    fn media_tokens_resolve_only_registered_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.mp4");
        fs::write(&file, b"x").unwrap();
        let media = MediaRegistry::default();
        let (token, url) = media.register(&file);
        assert_eq!(token.len(), 32);
        assert_eq!(url, format!("hlsr-media://media/{token}"));
        assert_eq!(media.resolve(&url), Some(file.clone()));
        assert_eq!(media.resolve("hlsr-media://media/../a.mp4"), None);
        media.unregister(&token);
        assert_eq!(media.resolve(&url), None);
        let id = random_hex(8);
        assert!(id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(id, random_hex(8));
    }

    #[test]
    fn public_timeline_keeps_known_fields_only() {
        let raw = json!({
            "version": 1,
            "video": {"duration": 20, "fps": 60, "width": "1920", "height": 1080, "speed": 0,
                "range": {"from": 1, "to": null}},
            "source": {"game": "ghosting", "map": "x".repeat(70), "files": ["a", "b"], "id": "q"},
            "markers": [{"t": 1, "type": "map", "label": 5, "sourceTime": 2},
                {"t": -1, "type": "map"}, {"t": 2, "type": "bogus"}, "junk",
                {"t": 3, "type": "finish"}],
        });
        assert_eq!(
            public_timeline(&raw),
            json!({
                "video": {"duration": 20, "fps": 60, "width": null, "height": 1080, "speed": 1, "range": null},
                "source": {"game": "ghosting", "map": "x".repeat(64), "files": 2},
                "markers": [{"t": 1, "type": "map", "label": "5", "sourceTime": 2},
                    {"t": 3, "type": "finish", "label": "", "sourceTime": null}],
            })
        );
    }
}
