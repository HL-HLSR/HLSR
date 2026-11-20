//! Служба обновления Windows (`UpdateManager.js` 0.4.7) без electron-updater: проверка по
//! `latest.yml` (основной адрес, запасной GitHub) вместе с подписанным каналом (шлюз,
//! `latestYmlSha256`), фоновая проверка раз в 30 мин + до 2 мин, при фокусе не чаще 5 мин,
//! откаты при ошибках и паузы при лимите; экран загрузки (путь А, Р5): скачать в процессе с
//! прогрессом, проверить по каналу и запустить `HLSR-Setup-<v>.exe --updated /S --force-run`;
//! «Обновить» из открытого лаунчера (путь Б) — внешний апдейтер, при игре — отказ `GAME_RUNNING`.
//!
//! Устройство повторяет 0.4.7: обработчики событий electron-updater (`checking-for-update`,
//! `update-available`, `update-not-available`, `download-progress`, `update-downloaded`, `error`,
//! `update-cancelled`) — методы службы; то, что делал сам electron-updater (лента, загрузка,
//! «проверка уже идёт», установка при выходе, `quitAndInstall`), — небольшая эмуляция поверх портов
//! [`UpdateFeed`], [`InstallerDownload`] и [`SetupRunner`]. Снимок `updater-state` и порядок
//! записей — как у 0.4.7 (сверка — трассы `tools/reference/update/update-manager-win.js`).
//!
//! Отступления: нет `app-update.yml` (настройки ленты — константы, ветки
//! `LU_UPDATE_CONFIG_MISSING` нет); строки журнала самого electron-updater не пишутся; установщик
//! для пути А качается по подписанным адресам канала (как апдейтер), без канала — по адресу файла
//! из `latest.yml`; не запустившийся установщик — ошибка обновления, а не выход лаунчера.

use crate::boot_gate::{self, BootAction, BootDecision, BootGateHost, Subscriber, UpdateState};
use crate::cancel::CancelToken;
use crate::channel::{self, VerifyOutcome};
use crate::gate::{LauncherUpdateGate, Outcome, VerifyInstallerOptions};
use crate::ports::{BoxFuture, Clock, Logger};
use hlsr_core::js;
use hlsr_core::json_text::js_number;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::AbortHandle;

/// `CHECK_TIMEOUT_MS`.
pub const CHECK_TIMEOUT: Duration = Duration::from_millis(30_000);
/// `LAUNCHER_TICK_MS` и `LAUNCHER_TICK_JITTER_MS`.
pub const LAUNCHER_TICK_MS: u64 = 30 * 60 * 1000;
pub const LAUNCHER_TICK_JITTER_MS: u64 = 2 * 60 * 1000;
/// `MIN_CHECK_AGE_MS`, `FOCUS_CHECK_AGE_MS`.
pub const MIN_CHECK_AGE_MS: i64 = 10 * 60 * 1000;
pub const FOCUS_CHECK_AGE_MS: i64 = 5 * 60 * 1000;
/// `RATE_LIMIT_PAUSE_MS`, `BACKOFF_BASE_MS`, `BACKOFF_MAX_MS`.
pub const RATE_LIMIT_PAUSE_MS: i64 = 15 * 60 * 1000;
pub const BACKOFF_BASE_MS: i64 = 60 * 1000;
pub const BACKOFF_MAX_MS: i64 = 30 * 60 * 1000;
/// `BOOT_CHECK_TIMEOUT_MS`.
pub const BOOT_CHECK_TIMEOUT: Duration = Duration::from_millis(10_000);
/// Повторная проверка после экрана загрузки не раньше чем через минуту.
const BOOT_RECHECK_MS: i64 = 60 * 1000;

/// Признак QA-сборки (`HLSR_BUILD_CHANNEL`).
pub const QA_BUILD_CHANNEL: &str = "qa";

// ------------------------------------------------------------------- чистые функции

fn regex(pattern: &str, flags: &str) -> regress::Regex {
    regress::Regex::with_flags(pattern, flags).expect("выражение 0.4.7 разбирается")
}

static RATE_LIMIT: LazyLock<regress::Regex> = LazyLock::new(|| regex(r"\b429\b|rate ?limit", "i"));
static NETWORK: LazyLock<regress::Regex> = LazyLock::new(|| {
    regex(
        r"ENOTFOUND|EAI_AGAIN|ECONNRESET|ECONNREFUSED|ETIMEDOUT|ERR_INTERNET_DISCONNECTED|network",
        "i",
    )
});
static FEED: LazyLock<regress::Regex> =
    LazyLock::new(|| regex(r"\b(404|403|5\d\d)\b|HttpError", "i"));
static NO_SPACE: LazyLock<regress::Regex> = LazyLock::new(|| regex(r"ENOSPC|no space", "i"));
static ACCESS: LazyLock<regress::Regex> = LazyLock::new(|| regex(r"EPERM|EACCES|denied", "i"));
static VERIFY: LazyLock<regress::Regex> =
    LazyLock::new(|| regex(r"sha512|checksum|signature|integrity", "i"));

/// `classifyUpdateError(error)`: класс ошибки для `errorCode` окна.
pub fn classify_update_error(text: &str) -> &'static str {
    if RATE_LIMIT.find(text).is_some() {
        "RATE_LIMIT"
    } else if NETWORK.find(text).is_some() {
        "NETWORK"
    } else if FEED.find(text).is_some() {
        "FEED"
    } else if NO_SPACE.find(text).is_some() {
        "NO_SPACE"
    } else if ACCESS.find(text).is_some() {
        "ACCESS"
    } else if VERIFY.find(text).is_some() {
        "VERIFY"
    } else {
        "UNKNOWN"
    }
}

static SCRIPT_STYLE: LazyLock<regress::Regex> =
    LazyLock::new(|| regex(r"<\s*(script|style)[^>]*>[\s\S]*?<\s*\/\s*\1\s*>", "gi"));
static TAG: LazyLock<regress::Regex> = LazyLock::new(|| regex(r"<[^>]+>", "g"));
static SPACES: LazyLock<regress::Regex> = LazyLock::new(|| regex(r"[ \t]+", "g"));
static BLANK_LINES: LazyLock<regress::Regex> = LazyLock::new(|| regex(r"\n{3,}", "g"));

fn replace_all(pattern: &regress::Regex, text: &str, with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for found in pattern.find_iter(text) {
        out.push_str(&text[last..found.start()]);
        out.push_str(with);
        last = found.end();
    }
    out.push_str(&text[last..]);
    out
}

/// `sanitizeReleaseNotes(notes)`: заметки без разметки, не длиннее 4000 единиц UTF-16.
pub fn sanitize_release_notes(notes: Option<&Value>) -> String {
    let Some(notes) = notes.filter(|notes| js::truthy(notes)) else {
        return String::new();
    };
    let text = match notes {
        Value::Array(items) => items
            .iter()
            .map(|item| match item.get("note") {
                Some(note) if js::truthy(item) && js::truthy(note) => js::to_string(note),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        Value::Object(_) => "[object Object]".to_owned(),
        other => js::to_string(other),
    };
    let text = replace_all(&SCRIPT_STYLE, &text, "");
    let text = replace_all(&TAG, &text, " ");
    let text = text
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    let text = replace_all(&SPACES, &text, " ");
    let text = replace_all(&BLANK_LINES, &text, "\n\n");
    js::slice_utf16(js::trim(&text), 4000)
}

/// `/^https?:\/\/(127\.0\.0\.1|localhost)(:\d+)?(\/|$)/`.
pub fn is_loopback_feed(value: &str) -> bool {
    let Some(rest) = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"))
    else {
        return false;
    };
    let Some(rest) = rest
        .strip_prefix("127.0.0.1")
        .or_else(|| rest.strip_prefix("localhost"))
    else {
        return false;
    };
    let rest = match rest.strip_prefix(':') {
        Some(port) => {
            let digits = port.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return false;
            }
            &port[digits..]
        }
        None => rest,
    };
    rest.is_empty() || rest.starts_with('/')
}

// ------------------------------------------------------------------- порты

/// Сведения `latest.yml` (`UpdateInfo` electron-updater).
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateInfo {
    pub version: String,
    /// `releaseDate`; `None` — поля нет (`undefined`).
    pub release_date: Option<Value>,
    pub release_notes: Option<Value>,
    /// Первый файл (`files[0]` или `path`): адрес относительно ленты, sha512, размер.
    pub file: Option<FeedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedFile {
    pub url: String,
    pub sha512: Option<String>,
    pub size: Option<u64>,
}

/// Лента обновлений (`autoUpdater.setFeedURL`, проверка `latest.yml`).
pub trait UpdateFeed: Send + Sync {
    fn set_feed_url(&self, url: &str);
    /// `latest.yml` с текущего адреса; ошибка — текст (по нему `classifyUpdateError`).
    fn check(&self) -> BoxFuture<'static, Result<UpdateInfo, String>>;
}

/// Шлюз канала с точки зрения службы (`LauncherUpdateGate`); ошибка — текст исключения.
pub trait GatePort: Send + Sync {
    fn evaluate<'a>(&'a self, version: &'a str) -> BoxFuture<'a, Result<Outcome, String>>;
    fn probe_latest(&self) -> BoxFuture<'_, Result<Option<Outcome>, String>>;
    fn verify_installer<'a>(
        &'a self,
        file: &'a Path,
        expected_version: &'a str,
    ) -> BoxFuture<'a, Result<VerifyOutcome, String>>;
}

impl GatePort for LauncherUpdateGate {
    fn evaluate<'a>(&'a self, version: &'a str) -> BoxFuture<'a, Result<Outcome, String>> {
        Box::pin(async move {
            LauncherUpdateGate::evaluate(self, Some(version))
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn probe_latest(&self) -> BoxFuture<'_, Result<Option<Outcome>, String>> {
        Box::pin(async move { Ok(LauncherUpdateGate::probe_latest(self).await) })
    }

    fn verify_installer<'a>(
        &'a self,
        file: &'a Path,
        expected_version: &'a str,
    ) -> BoxFuture<'a, Result<VerifyOutcome, String>> {
        Box::pin(async move {
            Ok(LauncherUpdateGate::verify_installer(
                self,
                file,
                &VerifyInstallerOptions {
                    expected_version: Some(expected_version.to_owned()),
                    ..VerifyInstallerOptions::default()
                },
            )
            .await)
        })
    }
}

/// Ход загрузки, как `download-progress` electron-updater.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DownloadProgressInfo {
    pub percent: f64,
    pub transferred: f64,
    pub total: f64,
    pub bytes_per_second: f64,
}

impl DownloadProgressInfo {
    pub fn to_json(&self) -> Value {
        json!({
            "percent": js_number(self.percent),
            "transferred": js_number(self.transferred),
            "total": js_number(self.total),
            "bytesPerSecond": js_number(self.bytes_per_second),
        })
    }
}

/// Почему загрузка не удалась.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadFailure {
    /// Отмена пользователем (`CancellationError`: «cancelled»).
    Cancelled,
    Failed(String),
}

/// Что качать.
#[derive(Debug, Clone, PartialEq)]
pub struct InstallerRequest {
    pub info: UpdateInfo,
    /// Адрес ленты на момент загрузки.
    pub feed_url: String,
    /// Подписанный канал (если шлюз его принял).
    pub manifest: Option<Value>,
}

pub type ProgressSink = Arc<dyn Fn(DownloadProgressInfo) + Send + Sync>;

/// Загрузка установщика (`downloadUpdate`); `Ok(None)` — файла нет (`downloadedFile` пуст).
pub trait InstallerDownload: Send + Sync {
    fn download(
        &self,
        request: InstallerRequest,
        cancel: CancelToken,
        on_progress: ProgressSink,
    ) -> BoxFuture<'static, Result<Option<PathBuf>, DownloadFailure>>;
}

/// Запуск скачанного установщика (`doInstall`): `--updated [/S] [--force-run]`, отвязанно.
pub trait SetupRunner: Send + Sync {
    fn run(&self, file: &Path, silent: bool, force_run: bool) -> Result<(), String>;
}

/// Внешний апдейтер (`updater-launch.js`): запуск копии с планом и уборка старых копий.
pub trait ExternalUpdater: Send + Sync {
    /// `canUseExternalUpdater`: апдейтер есть в этой сборке.
    fn available(&self) -> bool;
    fn launch(&self, plan: Value) -> BoxFuture<'static, Result<u32, String>>;
    fn cleanup_old_stages(&self) -> Result<usize, String>;
}

/// Центр уведомлений (`notifications.add({level, code, detail})`).
pub trait UpdateNotifier: Send + Sync {
    fn add(&self, level: &str, code: &str, detail: &str) -> Result<(), String>;
}

/// Телеметрия обновления (`launcher-update-events.js`, подпроект 4).
pub trait UpdateTelemetry: Send + Sync {
    fn available(&self, current: &str, next: Option<&str>) -> Result<(), String>;
    fn download_started(&self, current: &str, next: Option<&str>) -> Result<(), String>;
    fn download_completed(
        &self,
        current: &str,
        next: &str,
        bytes: Option<f64>,
        duration_ms: Option<i64>,
    ) -> Result<(), String>;
    fn failed(&self, current: &str, next: Option<&str>, code: Option<&str>) -> Result<(), String>;
}

/// События версий (`VersionEvents`, подпроект 4).
pub trait LauncherVersionEvents: Send + Sync {
    fn launcher_update_available(
        &self,
        version: &str,
        current: &str,
        has_release_notes: bool,
    ) -> Result<(), String>;
    fn write_pending_receipt(&self, from: &str, to: &str) -> Result<(), String>;
}

/// `InstallLocation` в реестре перед установкой (`install-location.js`).
pub trait InstallLocationFix: Send + Sync {
    fn ensure(&self, install_dir: &str) -> Result<(), String>;
}

/// Окно лаунчера: `updater-state` и прогресс на панели задач (`ITaskbarList3`).
pub trait UpdateWindow: Send + Sync {
    fn send_state(&self, payload: Value);
    fn set_progress_bar(&self, value: f64);
}

/// Выход из приложения (`app.quit()`).
pub trait AppControl: Send + Sync {
    fn quit(&self);
}

/// Адреса и ключи QA-канала для шлюза.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QaGate {
    pub channel_urls: Vec<String>,
    pub latest_yml_urls: Vec<String>,
    pub public_keys: Option<Vec<String>>,
}

/// Шлюз службы: `None` — боевой, `Some` — QA-канала.
pub type GateFactory = Box<dyn Fn(Option<&QaGate>) -> Arc<dyn GatePort> + Send + Sync>;

/// Настройки (то, что 0.4.7 брала у `app` и `process.env`).
#[derive(Debug, Clone)]
pub struct ManagerConfig {
    /// `app.getVersion()`.
    pub current_version: String,
    /// `app.isPackaged`.
    pub packaged: bool,
    /// `NODE_ENV === "production"`.
    pub production: bool,
    /// `HLSR_BUILD_CHANNEL` (по умолчанию `development`).
    pub build_channel: String,
    /// `HLSR_UPDATE_FEED`; в выпуске без `dev-hooks` не действует.
    pub update_feed: Option<String>,
    /// `HLSR_UPDATE_CHANNEL_KEY`.
    pub channel_key: Option<String>,
    /// `app.getPath("exe")`.
    pub exe_path: String,
    /// `app.getPath("userData")`.
    pub user_data_dir: String,
}

/// Порты службы. Необязательные (`set*` 0.4.7) можно задать и позже.
pub struct ManagerPorts {
    pub feed: Arc<dyn UpdateFeed>,
    pub make_gate: GateFactory,
    pub download: Arc<dyn InstallerDownload>,
    pub setup: Arc<dyn SetupRunner>,
    pub external: Arc<dyn ExternalUpdater>,
    pub install_location: Arc<dyn InstallLocationFix>,
    pub app: Arc<dyn AppControl>,
    pub logger: Arc<dyn Logger>,
    pub clock: Arc<dyn Clock>,
    /// `Math.random()`.
    pub random: Arc<dyn Fn() -> f64 + Send + Sync>,
    pub window: Option<Arc<dyn UpdateWindow>>,
    pub notifier: Option<Arc<dyn UpdateNotifier>>,
    pub telemetry: Option<Arc<dyn UpdateTelemetry>>,
    pub version_events: Option<Arc<dyn LauncherVersionEvents>>,
}

/// Ответ `updater-download`: `true`, `"external"` или `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadReply {
    Started,
    External,
    Refused,
}

impl DownloadReply {
    pub fn to_json(self) -> Value {
        match self {
            DownloadReply::Started => json!(true),
            DownloadReply::External => json!("external"),
            DownloadReply::Refused => json!(false),
        }
    }
}

// ------------------------------------------------------------------- состояние

#[derive(Debug, Clone, PartialEq)]
struct Info {
    version: String,
    release_date: Option<Value>,
    release_notes: String,
}

/// Поля `this` 0.4.7 и состояние эмуляции electron-updater.
struct Fields {
    state: UpdateState,
    info: Option<Info>,
    progress: Option<DownloadProgressInfo>,
    error: Option<String>,
    error_code: Option<String>,
    user_accepted_download: bool,
    check_timeout: Option<AbortHandle>,
    using_fallback_feed: bool,
    feed_freshness_checked: bool,
    verified_version: Option<String>,
    last_actual_check_at: i64,
    backoff_ms: i64,
    backoff_until: i64,
    rate_limited_until: i64,
    interval_timer: Option<AbortHandle>,
    install_requested: bool,
    install_location_checked: bool,
    gate: Arc<dyn GatePort>,
    gate_result: Option<Outcome>,
    boot_run: Option<Arc<BootRun>>,
    boot_decision: Option<BootDecision>,
    boot_completed_at: i64,
    pending_version: Option<String>,
    download_started_at: i64,
    last_transferred: f64,
    language: String,
    check_source: &'static str,
    external_updater_disabled: bool,
    enabled: bool,
    download_cancellation: Option<CancelToken>,
    game_running: Arc<dyn Fn() -> bool + Send + Sync>,
    // electron-updater
    feed_url: String,
    auto_install_on_app_quit: bool,
    quit_handler_added: bool,
    quit_and_install_called: bool,
    downloaded_file: Option<PathBuf>,
    update_info: Option<UpdateInfo>,
    /// Проверка идёт: число ожидающих её `.catch`.
    check_waiters: Option<usize>,
    /// Загрузка идёт: `.catch` ожидающих.
    download_waiters: Option<Vec<Rejection>>,
}

type Rejection = Box<dyn FnOnce(&str) + Send>;

/// Один прогон экрана загрузки (`bootPromise`).
struct BootRun {
    result: Mutex<Option<BootDecision>>,
    done: Notify,
}

/// Изменение снимка (`send(patch)`).
#[derive(Default)]
struct Patch {
    state: Option<UpdateState>,
    error: Option<Option<String>>,
    error_code: Option<Option<String>>,
    progress: Option<Option<DownloadProgressInfo>>,
    info: Option<Option<Info>>,
}

impl Patch {
    fn state(state: UpdateState) -> Patch {
        Patch {
            state: Some(state),
            ..Patch::default()
        }
    }

    fn error(mut self, error: Option<String>) -> Patch {
        self.error = Some(error);
        self
    }

    fn error_code(mut self, code: Option<&str>) -> Patch {
        self.error_code = Some(code.map(str::to_owned));
        self
    }

    fn progress(mut self, progress: Option<DownloadProgressInfo>) -> Patch {
        self.progress = Some(progress);
        self
    }

    fn info(mut self, info: Option<Info>) -> Patch {
        self.info = Some(info);
        self
    }
}

struct Inner {
    config: ManagerConfig,
    is_qa_build: bool,
    feed: Arc<dyn UpdateFeed>,
    download: Arc<dyn InstallerDownload>,
    setup: Arc<dyn SetupRunner>,
    external: Arc<dyn ExternalUpdater>,
    install_location: Arc<dyn InstallLocationFix>,
    app: Arc<dyn AppControl>,
    log: Arc<dyn Logger>,
    clock: Arc<dyn Clock>,
    random: Arc<dyn Fn() -> f64 + Send + Sync>,
    window: Mutex<Option<Arc<dyn UpdateWindow>>>,
    notifier: Mutex<Option<Arc<dyn UpdateNotifier>>>,
    telemetry: Mutex<Option<Arc<dyn UpdateTelemetry>>>,
    version_events: Mutex<Option<Arc<dyn LauncherVersionEvents>>>,
    on_boot_state: Mutex<Option<(u64, Arc<Subscriber>)>>,
    fields: Mutex<Fields>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `String(x).slice(0, 500)`.
fn first_500(text: &str) -> String {
    js::slice_utf16(text, 500)
}

/// `setImmediate(fn)`: после текущей синхронной части и уже начатой асинхронной работы.
fn immediate(work: impl FnOnce() + Send + 'static) {
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        work();
    });
}

/// `UpdateManager` (Windows).
#[derive(Clone)]
pub struct UpdateManager {
    inner: Arc<Inner>,
}

impl UpdateManager {
    /// `new UpdateManager(window)`: шлюз, лента (основная или QA) и журнал как в конструкторе 0.4.7.
    pub fn new(config: ManagerConfig, ports: ManagerPorts) -> UpdateManager {
        let log = ports.logger.clone();
        let gate = (ports.make_gate)(None);
        let is_qa_build = config.build_channel == QA_BUILD_CHANNEL;
        let mut enabled = config.packaged && config.production;
        let endpoints = channel::launcher_for(js::Platform::Win32);
        let primary = endpoints.feed_urls[0].clone();
        let fallback = endpoints.feed_urls[1].clone();

        // Подмена ленты и ключа — только отладочная сборка и `dev-hooks` (у 0.4.7 webpack
        // заменял признак QA-сборки константой в выпуске).
        let overrides = cfg!(any(debug_assertions, feature = "dev-hooks"));
        let env_feed = config
            .update_feed
            .clone()
            .filter(|value| overrides && !value.is_empty());
        let qa_feed = if is_qa_build {
            env_feed.clone().filter(|value| {
                let ok = is_loopback_feed(value);
                if !ok {
                    log.warn(&format!(
                        "Update feed override rejected (not loopback): {value}"
                    ));
                }
                ok
            })
        } else {
            None
        };
        if env_feed.is_some() && !is_qa_build {
            log.warn(&format!(
                "HLSR_UPDATE_FEED ignored: build channel is \"{}\", not \"qa\"",
                config.build_channel
            ));
        }

        let mut feed_url = String::new();
        let mut gate = gate;
        if let Some(qa_feed) = &qa_feed {
            log.warn(&format!("QA channel: using loopback update feed {qa_feed}"));
            ports.feed.set_feed_url(qa_feed);
            feed_url = qa_feed.clone();
            enabled = true;
            let base = qa_feed.trim_end_matches('/');
            gate = (ports.make_gate)(Some(&QaGate {
                channel_urls: vec![format!("{base}/launcher-channel.json")],
                latest_yml_urls: vec![format!("{base}/latest.yml")],
                public_keys: config
                    .channel_key
                    .clone()
                    .filter(|key| !key.is_empty())
                    .map(|key| vec![key.replace("\\n", "\n")]),
            }));
        } else if enabled {
            ports.feed.set_feed_url(&primary);
            feed_url = primary.clone();
            log.info(&format!("Update feed: {primary} (fallback: {fallback})"));
        }

        let fields = Fields {
            state: UpdateState::Idle,
            info: None,
            progress: None,
            error: None,
            error_code: None,
            user_accepted_download: false,
            check_timeout: None,
            using_fallback_feed: false,
            feed_freshness_checked: false,
            verified_version: None,
            last_actual_check_at: 0,
            backoff_ms: 0,
            backoff_until: 0,
            rate_limited_until: 0,
            interval_timer: None,
            install_requested: false,
            install_location_checked: false,
            gate,
            gate_result: None,
            boot_run: None,
            boot_decision: None,
            boot_completed_at: 0,
            pending_version: None,
            download_started_at: 0,
            last_transferred: 0.0,
            language: "ru".to_owned(),
            check_source: "automatic",
            external_updater_disabled: false,
            enabled,
            download_cancellation: None,
            game_running: Arc::new(|| false),
            feed_url,
            auto_install_on_app_quit: false,
            quit_handler_added: false,
            quit_and_install_called: false,
            downloaded_file: None,
            update_info: None,
            check_waiters: None,
            download_waiters: None,
        };
        UpdateManager {
            inner: Arc::new(Inner {
                config,
                is_qa_build,
                feed: ports.feed,
                download: ports.download,
                setup: ports.setup,
                external: ports.external,
                install_location: ports.install_location,
                app: ports.app,
                log: ports.logger,
                clock: ports.clock,
                random: ports.random,
                window: Mutex::new(ports.window),
                notifier: Mutex::new(ports.notifier),
                telemetry: Mutex::new(ports.telemetry),
                version_events: Mutex::new(ports.version_events),
                on_boot_state: Mutex::new(None),
                fields: Mutex::new(fields),
            }),
        }
    }

    /// `initialize()`: строка о запуске (обработчики каналов — методы службы).
    pub fn initialize(&self) {
        let inner = &self.inner;
        inner.log.info(&format!(
            "App start: HLSR {} | packaged={} | updates={}",
            inner.config.current_version,
            inner.config.packaged,
            if lock(&inner.fields).enabled {
                "enabled"
            } else {
                "disabled"
            }
        ));
    }

    pub fn set_game_running_check(&self, check: impl Fn() -> bool + Send + Sync + 'static) {
        lock(&self.inner.fields).game_running = Arc::new(check);
    }

    pub fn set_notifications(&self, notifier: Arc<dyn UpdateNotifier>) {
        *lock(&self.inner.notifier) = Some(notifier);
    }

    pub fn set_version_events(&self, events: Arc<dyn LauncherVersionEvents>) {
        *lock(&self.inner.version_events) = Some(events);
    }

    pub fn set_update_telemetry(&self, telemetry: Arc<dyn UpdateTelemetry>) {
        *lock(&self.inner.telemetry) = Some(telemetry);
    }

    pub fn set_window(&self, window: Option<Arc<dyn UpdateWindow>>) {
        *lock(&self.inner.window) = window;
    }

    /// Обновления включены (сборка, режим, QA-лента).
    pub fn enabled(&self) -> bool {
        lock(&self.inner.fields).enabled
    }

    pub fn state(&self) -> UpdateState {
        lock(&self.inner.fields).state
    }

    /// `updater-check`.
    pub fn check(&self) -> bool {
        self.inner.check_updates(true, "manual")
    }

    /// `checkUpdates({manual, reason})`.
    pub fn check_updates(&self, manual: bool, reason: &str) -> bool {
        self.inner.check_updates(manual, reason)
    }

    /// `updater-cancel`.
    pub fn cancel(&self) -> bool {
        let token = {
            let fields = lock(&self.inner.fields);
            if fields.state != UpdateState::Downloading {
                return false;
            }
            fields.download_cancellation.clone()
        };
        let Some(token) = token else {
            return false;
        };
        token.cancel();
        true
    }

    /// `updater-download`.
    pub async fn download(&self) -> DownloadReply {
        self.inner.updater_download().await
    }

    /// `boot-gate-run`.
    pub async fn boot_gate_run(&self, retry: bool) -> BootDecision {
        self.inner.run_boot_gate(retry).await
    }

    /// `updater-install-now`.
    pub fn install_now(&self) -> bool {
        self.inner.install_now()
    }

    /// `updater-install-on-quit`.
    pub fn install_on_quit(&self) -> bool {
        let inner = &self.inner;
        {
            let fields = lock(&inner.fields);
            if fields.state != UpdateState::Downloaded || fields.verified_version.is_none() {
                return false;
            }
        }
        inner.log.info("Update will be installed on app quit");
        inner.prepare_install_location();
        inner.arm_install_on_quit(true)
    }

    /// `updater-set-language`.
    pub fn set_language(&self, value: &str) -> String {
        let language = if value == "en" { "en" } else { "ru" };
        lock(&self.inner.fields).language = language.to_owned();
        language.to_owned()
    }

    /// `updater-get-state` (без `releaseDate`, как в 0.4.7).
    pub fn get_state(&self) -> Value {
        let inner = &self.inner;
        let fields = lock(&inner.fields);
        json!({
            "state": fields.state.as_str(),
            "enabled": fields.enabled,
            "currentVersion": inner.config.current_version,
            "version": fields.info.as_ref().map(|info| info.version.clone()),
            "releaseNotes": fields.info.as_ref().map(|info| info.release_notes.clone()),
            "progress": fields.progress.map(|p| p.to_json()),
            "error": fields.error,
            "errorCode": fields.error_code.clone().filter(|code| !code.is_empty()),
            "checkSource": fields.check_source,
        })
    }

    /// `startCheckingInterval()`.
    pub fn start_checking_interval(&self) {
        self.inner.start_checking_interval();
    }

    /// `checkOnFocus()`.
    pub fn check_on_focus(&self) {
        self.inner.check_updates(false, "focus");
    }

    /// `dispose()`.
    pub fn dispose(&self) {
        let mut fields = lock(&self.inner.fields);
        if let Some(timer) = fields.interval_timer.take() {
            timer.abort();
        }
        if let Some(timer) = fields.check_timeout.take() {
            timer.abort();
        }
    }

    /// Выход приложения с кодом `exit_code`: установка при выходе (`addQuitHandler` electron-updater).
    pub fn on_app_quit(&self, exit_code: i32) {
        let inner = &self.inner;
        {
            let fields = lock(&inner.fields);
            if !fields.quit_handler_added
                || fields.quit_and_install_called
                || !fields.auto_install_on_app_quit
                || exit_code != 0
            {
                return;
            }
        }
        inner.install(true, false);
    }

    /// `isBootMandatory()`.
    pub fn is_boot_mandatory(&self) -> bool {
        self.inner.is_boot_mandatory()
    }
}

impl Inner {
    fn now(&self) -> i64 {
        self.clock.now_ms()
    }

    fn current(&self) -> &str {
        &self.config.current_version
    }

    fn is_game_running(&self) -> bool {
        let check = lock(&self.fields).game_running.clone();
        check()
    }

    // --------------------------------------------------------------- send, notify, telemetry

    /// `send(patch)`: поля, снимок окну, состояние экрану загрузки.
    fn send(&self, patch: Patch) {
        let (payload, state) = {
            let mut fields = lock(&self.fields);
            if let Some(state) = patch.state {
                fields.state = state;
            }
            if let Some(error) = patch.error {
                fields.error = error;
            }
            if let Some(code) = patch.error_code {
                fields.error_code = code;
            }
            if let Some(progress) = patch.progress {
                fields.progress = progress;
            }
            if let Some(info) = patch.info {
                fields.info = info;
            }
            let mut payload = Map::new();
            payload.insert("state".into(), json!(fields.state.as_str()));
            payload.insert("enabled".into(), json!(fields.enabled));
            payload.insert("currentVersion".into(), json!(self.config.current_version));
            match &fields.info {
                Some(info) => {
                    payload.insert("version".into(), json!(info.version));
                    if let Some(date) = &info.release_date {
                        payload.insert("releaseDate".into(), date.clone());
                    }
                    payload.insert("releaseNotes".into(), json!(info.release_notes));
                }
                None => {
                    payload.insert("version".into(), Value::Null);
                    payload.insert("releaseDate".into(), Value::Null);
                    payload.insert("releaseNotes".into(), Value::Null);
                }
            }
            payload.insert(
                "progress".into(),
                fields.progress.map_or(Value::Null, |p| p.to_json()),
            );
            payload.insert("error".into(), json!(fields.error));
            payload.insert(
                "errorCode".into(),
                json!(fields.error_code.clone().filter(|code| !code.is_empty())),
            );
            payload.insert("checkSource".into(), json!(fields.check_source));
            (Value::Object(payload), fields.state)
        };
        let window = lock(&self.window).clone();
        if let Some(window) = window {
            window.send_state(payload);
        }
        let subscriber = lock(&self.on_boot_state)
            .as_ref()
            .map(|(_, subscriber)| subscriber.clone());
        if let Some(subscriber) = subscriber {
            subscriber(state);
        }
    }

    fn progress_bar(&self, value: f64) {
        let window = lock(&self.window).clone();
        if let Some(window) = window {
            window.set_progress_bar(value);
        }
    }

    fn notify(&self, level: &str, code: &str, detail: &str) {
        let notifier = lock(&self.notifier).clone();
        if let Some(notifier) = notifier {
            let _ = notifier.add(level, code, detail);
        }
    }

    /// `telemetry(method, ...args)`: сбой — строка журнала.
    fn telemetry(
        &self,
        method: &str,
        call: impl FnOnce(&dyn UpdateTelemetry) -> Result<(), String>,
    ) {
        let telemetry = lock(&self.telemetry).clone();
        let Some(telemetry) = telemetry else {
            return;
        };
        if let Err(message) = call(telemetry.as_ref()) {
            self.log.warn(&format!(
                "[telemetry] событие обновления {method} не записано: {message}"
            ));
        }
    }

    fn clear_check_timeout(&self) {
        if let Some(timer) = lock(&self.fields).check_timeout.take() {
            timer.abort();
        }
    }

    // --------------------------------------------------------------- лента

    fn set_feed(&self, url: &str) {
        lock(&self.fields).feed_url = url.to_owned();
        self.feed.set_feed_url(url);
    }

    /// `maybeUseFallbackFeed()`.
    fn maybe_use_fallback_feed(&self) -> bool {
        {
            let mut fields = lock(&self.fields);
            if fields.using_fallback_feed || !fields.enabled || self.is_qa_build {
                return false;
            }
            fields.using_fallback_feed = true;
        }
        let fallback = channel::launcher_for(js::Platform::Win32).feed_urls[1].clone();
        self.log.warn(&format!(
            "Switching launcher update feed to fallback: {fallback}"
        ));
        self.set_feed(&fallback);
        true
    }

    /// `restorePrimaryFeed()`.
    fn restore_primary_feed(&self) -> bool {
        {
            let fields = lock(&self.fields);
            if !fields.using_fallback_feed || !fields.enabled || self.is_qa_build {
                return false;
            }
        }
        let primary = channel::launcher_for(js::Platform::Win32).feed_urls[0].clone();
        self.log.warn(&format!(
            "Fallback feed failed too — back to primary: {primary}"
        ));
        self.set_feed(&primary);
        true
    }

    /// `checkFeedFreshness()`.
    fn check_feed_freshness(self: &Arc<Self>) {
        let gate = {
            let mut fields = lock(&self.fields);
            if fields.feed_freshness_checked || !fields.enabled {
                return;
            }
            fields.feed_freshness_checked = true;
            fields.gate.clone()
        };
        let inner = self.clone();
        tokio::spawn(async move {
            match gate.probe_latest().await {
                Ok(Some(outcome)) if outcome.ok && outcome.update_available => {
                    let Some(version) = outcome
                        .manifest
                        .as_ref()
                        .and_then(|manifest| manifest.get("version"))
                        .filter(|version| js::truthy(version))
                        .map(js::to_string)
                    else {
                        return;
                    };
                    inner.log.warn(&format!(
                        "Feed says no update, but the signed channel offers {version} (installed {}) — latest.yml on the host is stale",
                        inner.current()
                    ));
                    inner.notify("warning", "LU_FEED_STALE", &version);
                }
                Ok(_) => {}
                Err(message) => inner
                    .log
                    .warn(&format!("Feed freshness probe failed: {message}")),
            }
        });
    }

    // --------------------------------------------------------------- проверка

    /// `checkUpdates({manual, reason})`.
    fn check_updates(self: &Arc<Self>, manual: bool, reason: &str) -> bool {
        let enabled = {
            let mut fields = lock(&self.fields);
            if reason != "fallback" {
                fields.check_source = if reason == "manual" {
                    "manual"
                } else {
                    "automatic"
                };
            }
            fields.enabled
        };
        if !enabled {
            self.log.info("Update check skipped (development build)");
            self.send(Patch::state(UpdateState::NotAvailable));
            return false;
        }
        let now = self.now();
        {
            let mut fields = lock(&self.fields);
            if matches!(
                fields.state,
                UpdateState::Checking | UpdateState::Downloading | UpdateState::Downloaded
            ) {
                return false;
            }
            if now < fields.rate_limited_until {
                drop(fields);
                self.log.info("Update check skipped: rate limited");
                return false;
            }
            if !manual {
                if now < fields.backoff_until {
                    return false;
                }
                let min_age = if reason == "focus" {
                    FOCUS_CHECK_AGE_MS
                } else {
                    MIN_CHECK_AGE_MS
                };
                if fields.last_actual_check_at != 0 && now - fields.last_actual_check_at < min_age {
                    return false;
                }
            }
            fields.last_actual_check_at = now;
        }

        self.clear_check_timeout();
        let inner = self.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(CHECK_TIMEOUT).await;
            if lock(&inner.fields).state == UpdateState::Checking {
                inner.log.warn("Update check timed out");
                inner.notify(
                    "warning",
                    "UPDATE_CHECK_TIMEOUT",
                    "проверка обновления лаунчера",
                );
                inner.send(
                    Patch::state(UpdateState::Error)
                        .error(Some("timeout".to_owned()))
                        .error_code(Some("TIMEOUT")),
                );
            }
        });
        lock(&self.fields).check_timeout = Some(timer.abort_handle());

        self.au_check_for_updates();
        true
    }

    /// `autoUpdater.checkForUpdates()` и `.catch` службы: проверка уже идёт — её итог получат все
    /// ожидающие.
    fn au_check_for_updates(self: &Arc<Self>) {
        {
            let mut fields = lock(&self.fields);
            if let Some(waiters) = &mut fields.check_waiters {
                *waiters += 1;
                return;
            }
            fields.check_waiters = Some(1);
        }
        let check = self.feed.check();
        self.on_checking_for_update();
        let inner = self.clone();
        tokio::spawn(async move {
            let result = check.await.and_then(|info| {
                inner
                    .is_update_available(&info)
                    .map(|available| (info, available))
            });
            match result {
                Ok((info, true)) => {
                    {
                        let mut fields = lock(&inner.fields);
                        fields.update_info = Some(info.clone());
                        fields.check_waiters = None;
                    }
                    inner.on_update_available(info).await;
                }
                Ok((_, false)) => {
                    lock(&inner.fields).check_waiters = None;
                    inner.on_update_not_available();
                }
                Err(message) => {
                    let waiters = lock(&inner.fields).check_waiters.take().unwrap_or(0);
                    inner.on_error(&message);
                    for _ in 0..waiters {
                        inner.clear_check_timeout();
                        inner
                            .log
                            .error(&format!("checkForUpdates failed: {message}"));
                        inner.send(
                            Patch::default()
                                .error(Some(first_500(&message)))
                                .with_error_state(),
                        );
                    }
                }
            }
        });
    }

    /// `isUpdateAvailable(updateInfo)`: новее установленной (без понижения).
    fn is_update_available(&self, info: &UpdateInfo) -> Result<bool, String> {
        match channel::compare_semver(&info.version, self.current()) {
            Ok(order) => Ok(order == std::cmp::Ordering::Greater),
            Err(_) => Err(format!(
                "This file could not be downloaded, or the latest version (from update server) does not have a valid semver version: \"{}\"",
                info.version
            )),
        }
    }

    /// `checking-for-update`.
    fn on_checking_for_update(&self) {
        self.log.info("Update check started");
        self.send(Patch::state(UpdateState::Checking).error(None));
    }

    /// `update-available`.
    async fn on_update_available(self: &Arc<Self>, info: UpdateInfo) {
        self.clear_check_timeout();
        self.log
            .info(&format!("Update available: {}", info.version));
        let gate = {
            let mut fields = lock(&self.fields);
            fields.backoff_ms = 0;
            fields.backoff_until = 0;
            fields.gate.clone()
        };
        let gate_result = match gate.evaluate(&info.version).await {
            Ok(outcome) => outcome,
            Err(message) => Outcome {
                ok: false,
                problems: vec![message],
                update_available: false,
                downgrade: false,
                unsupported: false,
                code: None,
                update_required: false,
                manifest: None,
                source_url: String::new(),
            },
        };
        {
            let mut fields = lock(&self.fields);
            fields.gate_result = Some(gate_result.clone());
            fields.pending_version = Some(info.version.clone());
            fields.download_started_at = 0;
            fields.last_transferred = 0.0;
        }

        if !gate_result.ok {
            let first = gate_result
                .problems
                .first()
                .cloned()
                .unwrap_or_else(|| "rejected".to_owned());
            self.log.warn(&format!(
                "Launcher update rejected by signed metadata: {}",
                gate_result.problems.join("; ")
            ));
            let code = gate_result.code.map(|code| code.as_str());
            self.notify("error", code.unwrap_or("LAUNCHER_UPDATE_ERROR"), &first);
            self.send(
                Patch::state(UpdateState::Error)
                    .error(Some(first_500(&format!("metadata: {first}"))))
                    .error_code(Some("METADATA")),
            );
            self.telemetry("failed", |t| {
                t.failed(self.current(), Some(&info.version), code)
            });
            return;
        }

        let notes = sanitize_release_notes(info.release_notes.as_ref());
        self.send(Patch::state(UpdateState::Available).info(Some(Info {
            version: info.version.clone(),
            release_date: info.release_date.clone(),
            release_notes: notes.clone(),
        })));
        self.telemetry("available", |t| {
            t.available(self.current(), Some(&info.version))
        });
        let events = lock(&self.version_events).clone();
        if let Some(events) = events
            && let Err(message) =
                events.launcher_update_available(&info.version, self.current(), !notes.is_empty())
        {
            self.log.warn(&format!(
                "[versions] уведомление о версии не создано: {message}"
            ));
        }
    }

    /// `update-not-available`.
    fn on_update_not_available(self: &Arc<Self>) {
        self.clear_check_timeout();
        self.log.info("No update available");
        {
            let mut fields = lock(&self.fields);
            fields.backoff_ms = 0;
            fields.backoff_until = 0;
        }
        self.send(Patch::state(UpdateState::NotAvailable).info(None));
        self.check_feed_freshness();
    }

    /// `download-progress`.
    fn on_download_progress(&self, progress: DownloadProgressInfo) {
        self.send(Patch::state(UpdateState::Downloading).progress(Some(progress)));
        let (started, pending) = {
            let fields = lock(&self.fields);
            (
                fields.gate_result.as_ref().is_some_and(|gate| gate.ok)
                    && progress.transferred > 0.0,
                fields.pending_version.clone(),
            )
        };
        if started {
            self.telemetry("downloadStarted", |t| {
                t.download_started(self.current(), pending.as_deref())
            });
            let mut fields = lock(&self.fields);
            if fields.download_started_at == 0 {
                fields.download_started_at = self.now();
            }
        }
        lock(&self.fields).last_transferred = progress.transferred;
        self.progress_bar(progress.percent / 100.0);
    }

    /// `update-downloaded`.
    async fn on_update_downloaded(self: &Arc<Self>, version: String, file: Option<PathBuf>) {
        self.log.info(&format!("Update downloaded: {version}"));
        self.progress_bar(-1.0);
        let gate = {
            let mut fields = lock(&self.fields);
            fields.auto_install_on_app_quit = false;
            fields.gate.clone()
        };
        self.send(Patch::state(UpdateState::Verifying));

        let verdict = match &file {
            Some(file) => match gate.verify_installer(file, &version).await {
                Ok(outcome) => (
                    outcome.ok,
                    outcome.problems,
                    outcome.code.map(|code| code.as_str().to_owned()),
                ),
                Err(message) => (false, vec![message], Some("LU_VERIFY_FAILED".to_owned())),
            },
            None => (
                false,
                vec!["нет пути к скачанному файлу".to_owned()],
                Some("LU_NO_FILE".to_owned()),
            ),
        };
        let (ok, problems, code) = verdict;
        if !ok {
            {
                let mut fields = lock(&self.fields);
                fields.auto_install_on_app_quit = false;
                fields.verified_version = None;
            }
            let first = problems
                .first()
                .cloned()
                .unwrap_or_else(|| "rejected".to_owned());
            self.log.error(&format!(
                "Downloaded installer rejected [{}]: {}",
                code.as_deref().unwrap_or("?"),
                problems.join("; ")
            ));
            self.notify(
                "error",
                code.as_deref().unwrap_or("LAUNCHER_UPDATE_ERROR"),
                &first,
            );
            self.send(
                Patch::state(UpdateState::Error)
                    .error(Some(first_500(&format!(
                        "{}: {first}",
                        code.as_deref().unwrap_or("verify")
                    ))))
                    .error_code(Some("VERIFY")),
            );
            self.telemetry("failed", |t| {
                t.failed(self.current(), Some(&version), code.as_deref())
            });
            return;
        }

        self.log
            .info("Downloaded installer verified against signed metadata");
        let (bytes, duration, accepted) = {
            let mut fields = lock(&self.fields);
            fields.verified_version = Some(version.clone());
            let bytes = (fields.last_transferred != 0.0).then_some(fields.last_transferred);
            let duration =
                (fields.download_started_at != 0).then(|| self.now() - fields.download_started_at);
            (bytes, duration, fields.user_accepted_download)
        };
        self.telemetry("downloadCompleted", |t| {
            t.download_completed(self.current(), &version, bytes, duration)
        });
        self.arm_install_on_quit(accepted);
        self.send(Patch::state(UpdateState::Downloaded));
    }

    /// `error`.
    fn on_error(self: &Arc<Self>, message: &str) {
        self.clear_check_timeout();
        self.log.error(&format!("Updater error: {message}"));
        self.progress_bar(-1.0);

        let checking = lock(&self.fields).state == UpdateState::Checking;
        if checking && self.maybe_use_fallback_feed() {
            self.check_updates(true, "fallback");
            return;
        }
        if checking {
            self.restore_primary_feed();
        }

        if RATE_LIMIT.find(message).is_some() {
            lock(&self.fields).rate_limited_until = self.now() + RATE_LIMIT_PAUSE_MS;
            self.log.warn("Update feed rate limited — checks paused");
        } else if checking {
            let now = self.now();
            let mut fields = lock(&self.fields);
            fields.backoff_ms = if fields.backoff_ms != 0 {
                (fields.backoff_ms * 2).min(BACKOFF_MAX_MS)
            } else {
                BACKOFF_BASE_MS.min(BACKOFF_MAX_MS)
            };
            fields.backoff_until = now + fields.backoff_ms;
        }

        let shown = first_500(message);
        if checking {
            self.notify("warning", "LU_FEED_UNAVAILABLE", &shown);
        } else {
            self.notify("error", "LAUNCHER_UPDATE_ERROR", &shown);
            let pending = lock(&self.fields).pending_version.clone();
            self.telemetry("failed", |t| {
                t.failed(
                    self.current(),
                    pending.as_deref(),
                    Some("LAUNCHER_UPDATE_ERROR"),
                )
            });
        }
        self.send(
            Patch::state(UpdateState::Error)
                .error(Some(shown))
                .error_code(Some(classify_update_error(message))),
        );
    }

    /// `update-cancelled`.
    fn on_update_cancelled(&self) {
        {
            let mut fields = lock(&self.fields);
            fields.user_accepted_download = false;
            fields.download_cancellation = None;
        }
        self.progress_bar(-1.0);
        self.send(
            Patch::state(UpdateState::Available)
                .progress(None)
                .error(None)
                .error_code(None),
        );
    }

    // --------------------------------------------------------------- загрузка

    /// `autoUpdater.downloadUpdate(token)` и `.catch` вызывающего: загрузка уже идёт — её итог
    /// получат все.
    fn au_download_update(self: &Arc<Self>, cancel: CancelToken, on_rejected: Rejection) {
        let request = {
            let mut fields = lock(&self.fields);
            if let Some(waiters) = &mut fields.download_waiters {
                waiters.push(on_rejected);
                return;
            }
            fields.download_waiters = Some(vec![on_rejected]);
            InstallerRequest {
                info: fields.update_info.clone().unwrap_or(UpdateInfo {
                    version: String::new(),
                    release_date: None,
                    release_notes: None,
                    file: None,
                }),
                feed_url: fields.feed_url.clone(),
                manifest: fields
                    .gate_result
                    .as_ref()
                    .and_then(|gate| gate.manifest.clone()),
            }
        };
        let version = request.info.version.clone();
        let progress_target = Arc::downgrade(self);
        let download = self.download.download(
            request,
            cancel,
            Arc::new(move |progress| {
                if let Some(inner) = progress_target.upgrade() {
                    inner.on_download_progress(progress);
                }
            }),
        );
        let inner = self.clone();
        tokio::spawn(async move {
            match download.await {
                Ok(file) => {
                    {
                        let mut fields = lock(&inner.fields);
                        fields.downloaded_file.clone_from(&file);
                        fields.download_waiters = None;
                    }
                    inner.on_update_downloaded(version, file).await;
                }
                Err(DownloadFailure::Cancelled) => {
                    inner.on_update_cancelled();
                    let waiters = lock(&inner.fields).download_waiters.take();
                    for waiter in waiters.into_iter().flatten() {
                        waiter("cancelled");
                    }
                }
                Err(DownloadFailure::Failed(message)) => {
                    inner.on_error(&message);
                    let waiters = lock(&inner.fields).download_waiters.take();
                    for waiter in waiters.into_iter().flatten() {
                        waiter(&message);
                    }
                }
            }
        });
    }

    /// Загрузка, начатая службой: состояние `downloading` и токен отмены.
    fn begin_download(self: &Arc<Self>, log_prefix: &'static str) {
        let token = CancelToken::new();
        {
            let mut fields = lock(&self.fields);
            fields.user_accepted_download = true;
        }
        self.send(Patch::state(UpdateState::Downloading).progress(None));
        lock(&self.fields).download_cancellation = Some(token.clone());
        let log = self.log.clone();
        self.au_download_update(
            token,
            Box::new(move |message| {
                log.error(&format!("{log_prefix}downloadUpdate failed: {message}"));
            }),
        );
    }

    /// `canUseExternalUpdater()`.
    fn can_use_external_updater(&self) -> bool {
        if !self.config.packaged {
            return false;
        }
        {
            let fields = lock(&self.fields);
            if fields.external_updater_disabled {
                return false;
            }
            let usable = fields
                .gate_result
                .as_ref()
                .is_some_and(|gate| gate.ok && gate.manifest.is_some());
            if !usable {
                return false;
            }
        }
        self.external.available()
    }

    /// `buildUpdatePlan()`.
    fn build_update_plan(&self) -> Value {
        let fields = lock(&self.fields);
        let manifest = fields
            .gate_result
            .as_ref()
            .and_then(|gate| gate.manifest.clone())
            .unwrap_or(Value::Null);
        let path = js::path::NodePath::new(js::Platform::Win32, "C:\\");
        json!({
            "schema": 1,
            "manifest": manifest,
            "currentVersion": self.config.current_version,
            "targetVersion": manifest.get("version").cloned().unwrap_or(Value::Null),
            "installDir": path.dirname(&self.config.exe_path),
            "launcherExe": self.config.exe_path,
            "receiptFile": path.join(&[&self.config.user_data_dir, "version-state.json"]),
            "language": if fields.language.is_empty() { "ru" } else { fields.language.as_str() },
        })
    }

    /// `startExternalUpdate()`.
    async fn start_external_update(self: &Arc<Self>) -> bool {
        if self.is_game_running() {
            self.log.info("External updater deferred: game is running");
            self.notify("warning", "GAME_RUNNING", "обновление лаунчера отложено");
            return false;
        }
        let plan = self.build_update_plan();
        match self.external.launch(plan).await {
            Ok(pid) => {
                self.log.info(&format!(
                    "External updater started (PID {pid}); quitting launcher"
                ));
                let app = self.app.clone();
                immediate(move || app.quit());
                true
            }
            Err(message) => {
                self.log
                    .error(&format!("External updater failed to start: {message}"));
                self.notify(
                    "error",
                    "LAUNCHER_UPDATE_ERROR",
                    &format!("окно обновления не запустилось: {message}"),
                );
                lock(&self.fields).external_updater_disabled = true;
                false
            }
        }
    }

    /// `updater-download`.
    async fn updater_download(self: &Arc<Self>) -> DownloadReply {
        {
            let fields = lock(&self.fields);
            if !fields.enabled {
                return DownloadReply::Refused;
            }
            if matches!(
                fields.state,
                UpdateState::Downloading | UpdateState::Verifying | UpdateState::Downloaded
            ) {
                return DownloadReply::Refused;
            }
            let retry_after_error =
                fields.state == UpdateState::Error && fields.pending_version.is_some();
            if fields.state != UpdateState::Available && !retry_after_error {
                let state = fields.state.as_str();
                drop(fields);
                self.log.info(&format!(
                    "updater-download отклонён: состояние {state}, качать нечего"
                ));
                return DownloadReply::Refused;
            }
        }
        self.log.info("User accepted update download");
        lock(&self.fields).check_source = "manual";

        if self.can_use_external_updater() {
            if self.start_external_update().await {
                return DownloadReply::External;
            }
            self.log
                .warn("External updater unavailable, falling back to electron-updater");
        }
        self.begin_download("");
        DownloadReply::Started
    }

    // --------------------------------------------------------------- установка

    /// `armInstallOnQuit(enabled)`.
    fn arm_install_on_quit(&self, enabled: bool) -> bool {
        let mut fields = lock(&self.fields);
        fields.auto_install_on_app_quit = enabled;
        if !enabled {
            return false;
        }
        // addQuitHandler: один раз и только при включённой установке при выходе.
        if !fields.quit_handler_added {
            fields.quit_handler_added = true;
        }
        true
    }

    /// `prepareInstallLocation()`.
    fn prepare_install_location(&self) {
        {
            let mut fields = lock(&self.fields);
            if fields.install_location_checked {
                return;
            }
            fields.install_location_checked = true;
        }
        let path = js::path::NodePath::new(js::Platform::Win32, "C:\\");
        if let Err(message) = self
            .install_location
            .ensure(&path.dirname(&self.config.exe_path))
        {
            self.log.warn(&format!(
                "[update] проверка InstallLocation не удалась: {message}"
            ));
        }
    }

    fn write_pending_receipt(&self, to: &str) {
        let events = lock(&self.version_events).clone();
        if let Some(events) = events
            && let Err(message) = events.write_pending_receipt(self.current(), to)
        {
            self.log
                .warn(&format!("[versions] receipt не сохранён: {message}"));
        }
    }

    /// `updater-install-now`.
    fn install_now(self: &Arc<Self>) -> bool {
        let verified = {
            let fields = lock(&self.fields);
            if fields.install_requested {
                drop(fields);
                self.log
                    .info("quitAndInstall уже запрошен — повторное нажатие проигнорировано");
                return false;
            }
            if fields.state != UpdateState::Downloaded {
                return false;
            }
            fields.verified_version.clone()
        };
        let Some(verified) = verified else {
            self.log
                .warn("quitAndInstall refused: installer was not verified");
            return false;
        };
        if self.is_game_running() {
            self.log.info("quitAndInstall deferred: game is running");
            return false;
        }
        self.log.info("quitAndInstall requested by user");
        self.write_pending_receipt(&verified);
        lock(&self.fields).install_requested = true;
        self.prepare_install_location();
        lock(&self.fields).auto_install_on_app_quit = true;
        let inner = self.clone();
        immediate(move || inner.quit_and_install(true, true));
        true
    }

    /// `autoUpdater.quitAndInstall(isSilent, isForceRunAfter)`.
    fn quit_and_install(self: &Arc<Self>, silent: bool, force_run: bool) {
        if self.install(silent, force_run) {
            let app = self.app.clone();
            immediate(move || app.quit());
        } else {
            lock(&self.fields).quit_and_install_called = false;
        }
    }

    /// `install(isSilent, isForceRunAfter)` electron-updater: установщик отвязанным процессом.
    fn install(self: &Arc<Self>, silent: bool, force_run: bool) -> bool {
        let file = {
            let mut fields = lock(&self.fields);
            if fields.quit_and_install_called {
                return false;
            }
            let Some(file) = fields.downloaded_file.clone() else {
                drop(fields);
                self.on_error("No update filepath provided, can't quit and install");
                return false;
            };
            fields.quit_and_install_called = true;
            file
        };
        match self.setup.run(&file, silent, force_run) {
            Ok(()) => true,
            Err(message) => {
                self.on_error(&message);
                false
            }
        }
    }

    // --------------------------------------------------------------- интервал

    /// `startCheckingInterval()`.
    fn start_checking_interval(self: &Arc<Self>) {
        self.start_background_timer();
        let skip = {
            let fields = lock(&self.fields);
            (fields.boot_run.is_some() && fields.boot_decision.is_none())
                || (fields.boot_completed_at != 0
                    && self.now() - fields.boot_completed_at < BOOT_RECHECK_MS)
        };
        if !skip {
            self.check_updates(false, "startup");
        }
        let inner = self.clone();
        immediate(move || match inner.external.cleanup_old_stages() {
            Ok(0) => {}
            Ok(removed) => inner
                .log
                .info(&format!("[updater] убрано старых копий в TEMP: {removed}")),
            Err(message) => inner
                .log
                .warn(&format!("[updater] уборка TEMP не выполнена: {message}")),
        });
    }

    /// `startBackgroundTimer()`.
    fn start_background_timer(self: &Arc<Self>) {
        if lock(&self.fields).interval_timer.is_some() {
            return;
        }
        self.schedule_tick();
    }

    fn schedule_tick(self: &Arc<Self>) {
        let jitter = ((self.random)() * LAUNCHER_TICK_JITTER_MS as f64).floor() as u64;
        let delay = Duration::from_millis(LAUNCHER_TICK_MS + jitter);
        let weak = Arc::downgrade(self);
        let timer = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let Some(inner) = weak.upgrade() else { return };
            inner.check_updates(false, "scheduled");
            inner.schedule_tick();
        });
        lock(&self.fields).interval_timer = Some(timer.abort_handle());
    }

    // --------------------------------------------------------------- экран загрузки

    /// `isBootMandatory()`.
    fn is_boot_mandatory(&self) -> bool {
        lock(&self.fields)
            .gate_result
            .as_ref()
            .is_some_and(|gate| gate.update_required || gate.unsupported)
    }

    /// `runBootGate(options)`.
    async fn run_boot_gate(self: &Arc<Self>, retry: bool) -> BootDecision {
        let reset = {
            let mut fields = lock(&self.fields);
            let blocked = fields
                .boot_decision
                .as_ref()
                .is_some_and(|decision| decision.action == BootAction::Blocked);
            if retry && blocked {
                fields.boot_run = None;
                fields.boot_decision = None;
            }
            retry && blocked
        };
        if reset {
            self.send(Patch::state(UpdateState::Idle).error(None).progress(None));
        }
        let (run, start) = {
            let mut fields = lock(&self.fields);
            match &fields.boot_run {
                Some(run) => (run.clone(), false),
                None => {
                    let run = Arc::new(BootRun {
                        result: Mutex::new(None),
                        done: Notify::new(),
                    });
                    fields.boot_run = Some(run.clone());
                    (run, true)
                }
            }
        };
        if start {
            let inner = self.clone();
            let target = run.clone();
            tokio::spawn(async move {
                let decision = inner.execute_boot_gate().await;
                *lock(&target.result) = Some(decision);
                target.done.notify_waiters();
            });
        }
        loop {
            let notified = run.done.notified();
            if let Some(decision) = lock(&run.result).clone() {
                return decision;
            }
            notified.await;
        }
    }

    /// `executeBootGate()`.
    async fn execute_boot_gate(self: &Arc<Self>) -> BootDecision {
        let host: Arc<dyn BootGateHost> = Arc::new(BootHost {
            inner: self.clone(),
        });
        let decision = boot_gate::run_boot_gate(host, self.log.clone(), BOOT_CHECK_TIMEOUT).await;
        self.finish_boot(decision)
    }

    /// `finishBoot(decision)`.
    fn finish_boot(&self, decision: BootDecision) -> BootDecision {
        {
            let mut fields = lock(&self.fields);
            fields.boot_completed_at = self.now();
            fields.boot_decision = Some(decision.clone());
        }
        if let Some(warn) = decision.warn {
            let detail = decision
                .error
                .clone()
                .filter(|error| !error.is_empty())
                .unwrap_or_else(|| decision.reason.to_owned());
            self.notify("warning", warn, &detail);
        }
        self.log.info(&format!(
            "[boot] decision: {} ({}){}",
            decision.action.as_str(),
            decision.reason,
            decision
                .error
                .as_ref()
                .filter(|error| !error.is_empty())
                .map(|error| format!(" — {error}"))
                .unwrap_or_default()
        ));
        decision
    }
}

impl Patch {
    /// `send({state: ERROR, error})` из `.catch` проверки: код ошибки не меняется.
    fn with_error_state(mut self) -> Patch {
        self.state = Some(UpdateState::Error);
        self
    }
}

/// Служба с точки зрения экрана загрузки (`host` в `executeBootGate`).
struct BootHost {
    inner: Arc<Inner>,
}

impl BootGateHost for BootHost {
    fn enabled(&self) -> bool {
        lock(&self.inner.fields).enabled
    }

    fn state(&self) -> UpdateState {
        lock(&self.inner.fields).state
    }

    fn subscribe(&self, subscriber: Subscriber) -> Box<dyn FnOnce() + Send> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        *lock(&self.inner.on_boot_state) = Some((id, Arc::new(subscriber)));
        let inner = Arc::downgrade(&self.inner);
        Box::new(move || {
            if let Some(inner) = inner.upgrade() {
                let mut slot = lock(&inner.on_boot_state);
                if slot.as_ref().is_some_and(|(current, _)| *current == id) {
                    *slot = None;
                }
            }
        })
    }

    fn check(&self) {
        self.inner.check_updates(true, "boot");
    }

    fn download(&self) {
        self.inner.begin_download("[boot] ");
    }

    fn install(&self) {
        let inner = &self.inner;
        inner.prepare_install_location();
        let verified = lock(&inner.fields).verified_version.clone();
        if let Some(verified) = verified {
            inner.write_pending_receipt(&verified);
        }
        lock(&inner.fields).auto_install_on_app_quit = true;
        let target = inner.clone();
        immediate(move || target.quit_and_install(true, true));
    }

    fn is_game_running(&self) -> bool {
        self.inner.is_game_running()
    }

    fn version(&self) -> Option<String> {
        lock(&self.inner.fields)
            .info
            .as_ref()
            .map(|info| info.version.clone())
    }

    fn error(&self) -> Option<String> {
        lock(&self.inner.fields).error.clone()
    }

    fn is_mandatory(&self) -> bool {
        self.inner.is_boot_mandatory()
    }
}

// ------------------------------------------------------------------- реализации для приложения

/// `Number.prototype.toString(32)` для неотрицательного целого.
fn base32(mut n: u64) -> String {
    const DIGITS: &[u8; 32] = b"0123456789abcdefghijklmnopqrstuv";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(n % 32) as usize]);
        n /= 32;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).expect("цифры — ASCII")
}

/// Адрес `latest.yml` ленты (`newBaseUrl` + `newUrlFromBase` electron-updater): база с `/` на
/// конце, свой запрос базы сохраняется, иначе `noCache=<Date.now() в base32>`.
pub fn latest_yml_url(feed: &str, now_ms: i64) -> Result<String, String> {
    let mut base = url::Url::parse(feed).map_err(|error| format!("Invalid URL: {error}"))?;
    if !base.path().ends_with('/') {
        let path = format!("{}/", base.path());
        base.set_path(&path);
    }
    let mut target = base
        .join("latest.yml")
        .map_err(|error| format!("Invalid URL: {error}"))?;
    match base.query().filter(|query| !query.is_empty()) {
        Some(query) => target.set_query(Some(query)),
        None => target.set_query(Some(&format!("noCache={}", base32(now_ms.max(0) as u64)))),
    }
    Ok(target.into())
}

/// Скаляр YAML верхнего уровня: строка в кавычках, `null`/`~`/пусто — `null`, прочее — строкой.
fn yaml_scalar(raw: &str) -> Value {
    let raw = raw.trim();
    if let Some(inner) = raw
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return Value::String(inner.replace("''", "'"));
    }
    if let Some(inner) = raw
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        return Value::String(inner.replace("\\\"", "\"").replace("\\\\", "\\"));
    }
    let raw = raw.split(" #").next().unwrap_or(raw).trim();
    match raw {
        "" | "~" | "null" | "Null" | "NULL" => Value::Null,
        other => Value::String(other.to_owned()),
    }
}

/// Сведения `latest.yml`: `version`, `releaseDate`, `releaseNotes` и первый файл (`files[0]`, иначе
/// `path` и `sha512`). Разбор — только этих полей формы electron-builder; блочные скаляры YAML не
/// поддерживаются (их в `latest.yml` выпуска нет).
pub fn parse_latest_yml(text: &str) -> UpdateInfo {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut top: Map<String, Value> = Map::new();
    let mut first_file: Map<String, Value> = Map::new();
    let mut in_files = false;
    let mut items = 0usize;
    for line in text.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indented = line.starts_with([' ', '\t']);
        if !indented {
            in_files = false;
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_owned();
                in_files = key == "files";
                top.insert(key, yaml_scalar(value));
            }
            continue;
        }
        if !in_files {
            continue;
        }
        let trimmed = line.trim_start();
        let entry = if let Some(rest) = trimmed.strip_prefix("- ") {
            items += 1;
            rest
        } else {
            trimmed
        };
        if items != 1 {
            continue;
        }
        if let Some((key, value)) = entry.split_once(':') {
            first_file.insert(key.trim().to_owned(), yaml_scalar(value));
        }
    }
    let text_of = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_owned);
    let file = if items > 0 {
        text_of(first_file.get("url")).map(|url| FeedFile {
            url,
            sha512: text_of(first_file.get("sha512")),
            size: text_of(first_file.get("size")).and_then(|size| size.parse().ok()),
        })
    } else {
        text_of(top.get("path")).map(|url| FeedFile {
            url,
            sha512: text_of(top.get("sha512")),
            size: None,
        })
    };
    UpdateInfo {
        version: text_of(top.get("version")).unwrap_or_default(),
        release_date: top.get("releaseDate").cloned(),
        release_notes: top.get("releaseNotes").cloned(),
        file,
    }
}

/// Текст ошибки ленты: по нему `classifyUpdateError` даёт тот же класс, что у electron-updater
/// (`HttpError: 404` — `FEED`, сеть — `NETWORK`, 429 — `RATE_LIMIT`).
pub fn feed_error_text(error: &crate::ports::GetError) -> String {
    use crate::ports::GetError;
    match error {
        GetError::Status(status) => format!("HttpError: {status}"),
        GetError::Timeout => "ETIMEDOUT: таймаут".to_owned(),
        GetError::Network(message) => format!("network error: {message}"),
        other => other.to_string(),
    }
}

/// Лента `latest.yml` через HTTP-клиент шлюза (allowlist, 20 с, 256 КиБ).
pub struct HttpFeed {
    url: Mutex<String>,
    http: Arc<dyn crate::ports::HttpGet>,
    clock: Arc<dyn Clock>,
}

impl HttpFeed {
    pub fn new(http: Arc<dyn crate::ports::HttpGet>, clock: Arc<dyn Clock>) -> HttpFeed {
        HttpFeed {
            url: Mutex::new(String::new()),
            http,
            clock,
        }
    }
}

impl UpdateFeed for HttpFeed {
    fn set_feed_url(&self, url: &str) {
        *lock(&self.url) = url.to_owned();
    }

    fn check(&self) -> BoxFuture<'static, Result<UpdateInfo, String>> {
        let feed = lock(&self.url).clone();
        let http = self.http.clone();
        let now = self.clock.now_ms();
        Box::pin(async move {
            let url = latest_yml_url(&feed, now)?;
            let body = http
                .get(&url, &crate::ports::GetOptions::default())
                .await
                .map_err(|error| feed_error_text(&error))?;
            Ok(parse_latest_yml(&String::from_utf8_lossy(&body)))
        })
    }
}

/// Загрузка установщика в процессе (путь А): по подписанным адресам канала, как апдейтер
/// (`updater/core/download.js`: `.part`, докачка, запас места); без принятого канала или с
/// QA-лентой на loopback — по адресу файла из `latest.yml`.
pub struct HttpInstallerDownload {
    pub dir: PathBuf,
    pub free_space: Arc<dyn crate::updater::download::FreeSpace>,
    pub clock: Arc<dyn Clock>,
}

impl HttpInstallerDownload {
    /// Адреса, имя файла и размер установщика.
    pub fn source(request: &InstallerRequest) -> Result<(Vec<String>, String, u64), String> {
        if let Some(manifest) = request
            .manifest
            .as_ref()
            .filter(|_| !is_loopback_feed(&request.feed_url))
        {
            let installer = &manifest["installer"];
            let urls = installer["urls"]
                .as_array()
                .map(|urls| urls.iter().map(js::to_string).collect())
                .unwrap_or_default();
            let filename = js::to_string(&installer["filename"]);
            let size = installer["size"]
                .as_u64()
                .ok_or("в канале нет размера установщика")?;
            return Ok((urls, filename, size));
        }
        let file = request
            .info
            .file
            .as_ref()
            .ok_or("в latest.yml нет файла установщика")?;
        let base = latest_yml_url(&request.feed_url, 0)?;
        let url = url::Url::parse(&base)
            .and_then(|base| base.join(&file.url))
            .map_err(|error| format!("Invalid URL: {error}"))?;
        let filename = url.path().rsplit('/').next().unwrap_or_default().to_owned();
        let size = file.size.ok_or("в latest.yml нет размера установщика")?;
        Ok((vec![url.to_string()], filename, size))
    }

    fn allowed(url: &str) -> bool {
        #[cfg(any(debug_assertions, feature = "dev-hooks"))]
        if crate::http::is_qa_loopback_url(url) {
            return true;
        }
        channel::is_allowed_url(url)
    }
}

impl InstallerDownload for HttpInstallerDownload {
    fn download(
        &self,
        request: InstallerRequest,
        cancel: CancelToken,
        on_progress: ProgressSink,
    ) -> BoxFuture<'static, Result<Option<PathBuf>, DownloadFailure>> {
        let dir = self.dir.clone();
        let free_space = self.free_space.clone();
        let clock = self.clock.clone();
        Box::pin(async move {
            let (urls, filename, size) = Self::source(&request).map_err(DownloadFailure::Failed)?;
            let mut options = crate::updater::download::DownloadOptions::new(
                urls,
                dir,
                filename,
                size,
                Arc::new(Self::allowed),
            );
            options.cancel = cancel;
            options.free_space = free_space;
            options.clock = clock;
            options.on_progress = Some(Arc::new(move |progress| {
                let total = progress.total as f64;
                on_progress(DownloadProgressInfo {
                    percent: if total > 0.0 {
                        progress.done as f64 / total * 100.0
                    } else {
                        0.0
                    },
                    transferred: progress.done as f64,
                    total,
                    bytes_per_second: progress.bytes_per_second,
                });
            }));
            match crate::updater::download::download(options).await {
                Ok(file) => Ok(Some(file)),
                Err(error) if error.code == crate::errors::UpdCode::Cancelled => {
                    Err(DownloadFailure::Cancelled)
                }
                Err(error) => Err(DownloadFailure::Failed(error.message)),
            }
        })
    }
}

/// Запуск скачанного `HLSR-Setup-<v>.exe --updated [/S] [--force-run]` отвязанным процессом
/// (`NsisUpdater.doInstall`).
pub struct ProcessSetupRunner {
    pub launcher: Arc<dyn crate::updater::processes::ProcessLauncher>,
    /// Окружение установщика (без переменных окна и распаковщика).
    pub env: Vec<(String, String)>,
}

impl SetupRunner for ProcessSetupRunner {
    fn run(&self, file: &Path, silent: bool, force_run: bool) -> Result<(), String> {
        let mut args = vec!["--updated".to_owned()];
        if silent {
            args.push("/S".to_owned());
        }
        if force_run {
            args.push("--force-run".to_owned());
        }
        self.launcher
            .spawn(&crate::updater::processes::Spawn {
                file: file.to_owned(),
                args,
                cwd: None,
                env: self.env.clone(),
                hidden: false,
                detached: true,
            })
            .map(drop)
    }
}

/// Внешний апдейтер: копия `HLSR.exe` во временном каталоге (`updater::launch`).
pub struct StagedExternalUpdater {
    pub app_exe: PathBuf,
    pub resources: PathBuf,
    pub temp_root: PathBuf,
    pub launcher: Arc<dyn crate::updater::processes::ProcessLauncher>,
    pub env: Vec<(String, String)>,
    pub clock: Arc<dyn Clock>,
    pub logger: Arc<dyn Logger>,
}

impl ExternalUpdater for StagedExternalUpdater {
    fn available(&self) -> bool {
        // Апдейтер — режим того же exe: он есть, пока есть сам лаунчер.
        self.app_exe.is_file()
    }

    fn launch(&self, plan: Value) -> BoxFuture<'static, Result<u32, String>> {
        let app_exe = self.app_exe.clone();
        let resources = self.resources.clone();
        let temp_root = self.temp_root.clone();
        let launcher = self.launcher.clone();
        let env = self.env.clone();
        let clock = self.clock.clone();
        let logger = self.logger.clone();
        Box::pin(async move {
            crate::updater::launch::launch(crate::updater::launch::LaunchOptions {
                app_exe: &app_exe,
                resources: &resources,
                temp_root: &temp_root,
                dir: None,
                plan: &plan,
                launcher: launcher.as_ref(),
                env,
                clock: clock.as_ref(),
                sleep: &crate::updater::process_wait::tokio_sleep,
                logger: logger.as_ref(),
            })
            .await
            .map(|launched| launched.pid)
        })
    }

    fn cleanup_old_stages(&self) -> Result<usize, String> {
        Ok(crate::updater::launch::cleanup_old_stages(
            &self.temp_root,
            crate::updater::launch::STAGE_MAX_AGE_MS,
            self.clock.now_ms(),
        ))
    }
}

/// `InstallLocation` в реестре (`install-location.js` через порт [`crate::ports::Registry`]).
pub struct RegistryInstallLocation {
    pub registry: Arc<dyn crate::ports::Registry>,
    pub logger: Arc<dyn Logger>,
}

impl InstallLocationFix for RegistryInstallLocation {
    fn ensure(&self, install_dir: &str) -> Result<(), String> {
        crate::windows::install_location::ensure_install_location(
            self.registry.as_ref(),
            &crate::windows::install_location::Options {
                install_dir,
                ..Default::default()
            },
            self.logger.as_ref(),
        );
        Ok(())
    }
}
