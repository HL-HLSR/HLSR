//! Служба обновления Linux (`linux/UpdateManager.js` 0.4.7): проверка подписанного канала, загрузка
//! рядом с установкой (`<корень>/.downloads/update-*`, 0700), сначала чанками (если в канале есть
//! `delta`), при сбое — полный AppImage; из AppImage — вшитый архив, его версия должна совпасть с
//! каналом; подготовка версии (`ArchiveInstall`), установка сейчас (с перезапуском `<версия>/hlsr
//! --no-sandbox <прежние аргументы>`) или при выходе; экран загрузки (проверка до 10 с, затем
//! скачать и установить); фоновая проверка раз в 30 мин + до 2 мин, при фокусе не чаще 5 мин, иначе
//! не чаще 10 мин; откат по ошибкам (1, 2, 4… мин, не больше 30; при лимите 429 — 15 мин).
//!
//! Включена, только если сборка упакована и бинарник из управляемой установки (`managed_root`).
//! Снимок `updater-state` — как у 0.4.7 (с `info` и `cancellable`). Порты: шлюз канала, загрузка
//! AppImage, извлечение payload, манифест чанков, установщик версий, перезапуск и выход, окно,
//! телеметрия и события версий (общие с Windows, `crate::update_manager`).

use super::archive_download::{ArchiveDownload, DownloadOptions, Rewrite};
use super::archive_install::{ArchiveInstall, ChunkStage};
use super::embedded_payload::{self, Payload};
use super::{EnvVars, SyncRunner};
use crate::boot_gate::{BootAction, BootDecision, UpdateState};
use crate::cancel::CancelToken;
use crate::channel;
use crate::chunks::{self, AllowUrl, ChunkFetch, Delta, Expected, FetchManifestOptions, Manifest};
use crate::gate::{LauncherUpdateGate, Outcome};
use crate::ports::{BoxFuture, Clock, Logger};
use crate::update_manager::{AppControl, LauncherVersionEvents, UpdateTelemetry, UpdateWindow};
use hlsr_core::js;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::AbortHandle;

/// `CHECK_INTERVAL_MS`.
pub const CHECK_INTERVAL_MS: i64 = 30 * 60 * 1000;
/// Случайная добавка к интервалу (до 2 мин).
pub const CHECK_JITTER_MS: f64 = 120_000.0;
/// `BOOT_TIMEOUT_MS`: сколько экран загрузки ждёт проверку.
pub const BOOT_TIMEOUT: Duration = Duration::from_millis(10_000);
/// Пауза после ответа 429.
pub const RATE_LIMIT_PAUSE_MS: i64 = 15 * 60 * 1000;
/// `AbortError` Node (`signal.throwIfAborted()`).
pub const ABORTED: &str = "This operation was aborted";

// ------------------------------------------------------------------- чистые функции

fn regex(pattern: &str) -> regress::Regex {
    regress::Regex::with_flags(pattern, "i").expect("выражение 0.4.7 разбирается")
}

static RATE_LIMIT: LazyLock<regress::Regex> = LazyLock::new(|| regex(r"429|rate.?limit"));
static NO_SPACE: LazyLock<regress::Regex> = LazyLock::new(|| regex("ENOSPC"));
static ACCESS: LazyLock<regress::Regex> = LazyLock::new(|| regex("EACCES|EPERM|EROFS"));
static VERIFY: LazyLock<regress::Regex> =
    LazyLock::new(|| regex("sha|подпис|метадан|AppImage изменился"));
static FEED: LazyLock<regress::Regex> = LazyLock::new(|| regex("HTTP|feed"));
static NETWORK: LazyLock<regress::Regex> =
    LazyLock::new(|| regex("timeout|таймаут|ENOTFOUND|ECONN|ETIMEDOUT"));

/// `errorCode(error)`: класс ошибки для окна (`errorCode` снимка).
pub fn error_code(text: &str) -> &'static str {
    let matches = |re: &regress::Regex| re.find(text).is_some();
    if matches(&RATE_LIMIT) {
        "RATE_LIMIT"
    } else if matches(&NO_SPACE) {
        "NO_SPACE"
    } else if matches(&ACCESS) {
        "ACCESS"
    } else if matches(&VERIFY) {
        "VERIFY"
    } else if matches(&FEED) {
        "FEED"
    } else if matches(&NETWORK) {
        "NETWORK"
    } else {
        "UNKNOWN"
    }
}

/// `relaunchArguments(argv)`: `--no-sandbox` и прежние аргументы (параметры окна сохраняются).
pub fn relaunch_arguments(argv: &[String]) -> Vec<String> {
    std::iter::once("--no-sandbox".to_owned())
        .chain(
            argv.iter()
                .skip(1)
                .filter(|arg| *arg != "--no-sandbox")
                .cloned(),
        )
        .collect()
}

/// `releaseNotes`: строка без тегов (`<…>` → пробел), не длиннее 4000 (UTF-16); не строка — `""`.
fn release_notes(manifest: &Value) -> String {
    let Some(text) = manifest.get("releaseNotes").and_then(Value::as_str) else {
        return String::new();
    };
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            let rest: String = chars.clone().collect();
            if let Some(end) = rest.find('>') {
                out.push(' ');
                for _ in 0..rest[..end].chars().count() + 1 {
                    chars.next();
                }
                continue;
            }
        }
        out.push(c);
    }
    js::slice_utf16(&out, 4000)
}

// ------------------------------------------------------------------- порты

/// Шлюз канала (`gate.evaluate()`); ошибка — текст исключения.
pub trait LinuxGate: Send + Sync {
    fn evaluate(&self) -> BoxFuture<'static, Result<Outcome, String>>;
}

/// Боевой шлюз.
pub struct GateAdapter(pub Arc<LauncherUpdateGate>);

impl LinuxGate for GateAdapter {
    fn evaluate(&self) -> BoxFuture<'static, Result<Outcome, String>> {
        let gate = self.0.clone();
        Box::pin(async move { gate.evaluate(None).await.map_err(|error| error.to_string()) })
    }
}

/// Ход загрузки — как его прислал источник (`{transferred, total, percent, bytesPerSecond}` у
/// AppImage, `{done, total, bytesPerSecond}` у чанков); попадает в снимок как есть.
pub type ProgressSink = Arc<dyn Fn(Value) + Send + Sync>;

/// Загрузка AppImage (`download(manifest, directory, {signal, onProgress})`).
pub trait AppImageDownloader: Send + Sync {
    fn download(
        &self,
        manifest: Value,
        directory: PathBuf,
        cancel: CancelToken,
        on_progress: ProgressSink,
    ) -> BoxFuture<'static, Result<PathBuf, String>>;
}

/// Вшитый архив из AppImage (`extractPayload(file, directory, {signal})`).
pub trait PayloadExtractor: Send + Sync {
    fn extract(
        &self,
        file: PathBuf,
        directory: PathBuf,
        cancel: CancelToken,
    ) -> BoxFuture<'static, Result<Payload, String>>;
}

/// Манифест чанков версии (`chunks.fetchManifest(delta, {version, platform: "linux"})`).
pub trait ChunkManifests: Send + Sync {
    fn fetch(
        &self,
        delta: Value,
        version: String,
        cancel: CancelToken,
    ) -> BoxFuture<'static, Result<Manifest, String>>;
}

/// Параметры сборки из чанков для установщика.
pub struct ChunkPrepare {
    pub cache_dir: PathBuf,
    pub chunk_base_urls: Vec<String>,
    pub cancel: CancelToken,
    pub on_progress: ProgressSink,
}

/// Итог сборки из чанков (`chunkResult`): скачано и взято с диска.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkResult {
    pub downloaded_bytes: u64,
    pub reused_bytes: u64,
}

/// Установка версии (`ArchiveInstall`, `createInstaller()`).
pub trait VersionInstaller: Send + Sync {
    fn prepare(
        &self,
        archive: PathBuf,
        manifest: Value,
        cancel: CancelToken,
    ) -> BoxFuture<'_, Result<(), String>>;
    fn prepare_from_chunks(
        &self,
        manifest: Manifest,
        channel_manifest: Value,
        options: ChunkPrepare,
    ) -> BoxFuture<'_, Result<Option<ChunkResult>, String>>;
    /// Путь к `hlsr` новой версии.
    fn commit(&self) -> Result<PathBuf, String>;
    fn rollback(&self) -> Result<(), String>;
    fn discard(&self);
}

pub type InstallerFactory = Arc<dyn Fn() -> Arc<dyn VersionInstaller> + Send + Sync>;

/// Перезапуск после выхода (`app.relaunch({execPath, args})`).
pub trait Relauncher: Send + Sync {
    fn relaunch(&self, exec_path: &Path, args: &[String]) -> Result<(), String>;
}

/// Настройки (то, что 0.4.7 брала у `app` и `process`).
#[derive(Debug, Clone)]
pub struct LinuxManagerConfig {
    /// `app.getVersion()`.
    pub current_version: String,
    /// `app.isPackaged`.
    pub packaged: bool,
    /// Корень управляемой установки (`managedRoot(process.execPath)`); `None` — обновления выключены.
    pub install_root: Option<PathBuf>,
    /// `process.argv` (нулевой — сам бинарник).
    pub argv: Vec<String>,
}

/// Порты службы.
pub struct LinuxManagerPorts {
    pub gate: Arc<dyn LinuxGate>,
    pub download: Arc<dyn AppImageDownloader>,
    pub extract: Arc<dyn PayloadExtractor>,
    pub chunk_manifests: Arc<dyn ChunkManifests>,
    pub create_installer: InstallerFactory,
    pub app: Arc<dyn AppControl>,
    pub relauncher: Arc<dyn Relauncher>,
    pub logger: Arc<dyn Logger>,
    pub clock: Arc<dyn Clock>,
    /// `Math.random()`.
    pub random: Arc<dyn Fn() -> f64 + Send + Sync>,
    pub window: Option<Arc<dyn UpdateWindow>>,
    pub telemetry: Option<Arc<dyn UpdateTelemetry>>,
    pub version_events: Option<Arc<dyn LauncherVersionEvents>>,
}

// ------------------------------------------------------------------- состояние

/// Обещание 0.4.7 (`checkPromise`, `downloadPromise`): ждать можно, пока не исполнится.
struct Completion {
    done: AtomicBool,
    notify: Notify,
}

impl Completion {
    fn new() -> Arc<Completion> {
        Arc::new(Completion {
            done: AtomicBool::new(false),
            notify: Notify::new(),
        })
    }

    fn complete(&self) {
        self.done.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            if self.done.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
}

/// Один прогон экрана загрузки (`bootPromise`).
struct BootRun {
    result: Mutex<Option<BootDecision>>,
    done: Arc<Completion>,
}

#[derive(Debug, Clone, PartialEq)]
struct Info {
    version: String,
    release_date: Option<Value>,
    release_notes: String,
}

impl Info {
    fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("version".into(), self.version.clone().into());
        if let Some(date) = &self.release_date {
            map.insert("releaseDate".into(), date.clone());
        }
        map.insert("releaseNotes".into(), self.release_notes.clone().into());
        Value::Object(map)
    }
}

struct Fields {
    state: UpdateState,
    info: Option<Info>,
    progress: Option<Value>,
    error: Option<String>,
    error_code: Option<&'static str>,
    check_source: &'static str,
    install_on_quit: bool,
    install_requested: bool,
    last_check_at: i64,
    backoff_ms: i64,
    backoff_until: i64,
    disposed: bool,
    game_running: Arc<dyn Fn() -> bool + Send + Sync>,
    gate_result: Option<Outcome>,
    check: Option<Arc<Completion>>,
    download: Option<Arc<Completion>>,
    abort: Option<CancelToken>,
    cancel_requested: bool,
    quit_pending: bool,
    installer: Option<Arc<dyn VersionInstaller>>,
    boot: Option<Arc<BootRun>>,
    boot_decision: Option<BootDecision>,
    timer: Option<AbortHandle>,
}

/// Изменение снимка (`send(state, extra)`).
#[derive(Default)]
struct Patch {
    error: Option<Option<String>>,
    error_code: Option<Option<&'static str>>,
    progress: Option<Option<Value>>,
    info: Option<Option<Info>>,
}

struct Inner {
    config: LinuxManagerConfig,
    enabled: bool,
    ports: LinuxManagerPorts,
    window: Mutex<Option<Arc<dyn UpdateWindow>>>,
    telemetry: Mutex<Option<Arc<dyn UpdateTelemetry>>>,
    version_events: Mutex<Option<Arc<dyn LauncherVersionEvents>>>,
    fields: Mutex<Fields>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Служба обновления Linux.
#[derive(Clone)]
pub struct LinuxUpdateManager {
    inner: Arc<Inner>,
}

enum Telemetry<'a> {
    Available(&'a str),
    DownloadStarted(&'a str),
    DownloadCompleted(&'a str, f64, i64),
    Failed(Option<&'a str>, &'a str),
}

impl LinuxUpdateManager {
    /// `new UpdateManager(window, options)`: включена у упакованной сборки с управляемым корнем.
    pub fn new(config: LinuxManagerConfig, ports: LinuxManagerPorts) -> LinuxUpdateManager {
        let enabled = config.packaged && config.install_root.is_some();
        let window = ports.window.clone();
        let telemetry = ports.telemetry.clone();
        let version_events = ports.version_events.clone();
        LinuxUpdateManager {
            inner: Arc::new(Inner {
                config,
                enabled,
                ports,
                window: Mutex::new(window),
                telemetry: Mutex::new(telemetry),
                version_events: Mutex::new(version_events),
                fields: Mutex::new(Fields {
                    state: UpdateState::Idle,
                    info: None,
                    progress: None,
                    error: None,
                    error_code: None,
                    check_source: "automatic",
                    install_on_quit: false,
                    install_requested: false,
                    last_check_at: 0,
                    backoff_ms: 0,
                    backoff_until: 0,
                    disposed: false,
                    game_running: Arc::new(|| false),
                    gate_result: None,
                    check: None,
                    download: None,
                    abort: None,
                    cancel_requested: false,
                    quit_pending: false,
                    installer: None,
                    boot: None,
                    boot_decision: None,
                    timer: None,
                }),
            }),
        }
    }

    pub fn enabled(&self) -> bool {
        self.inner.enabled
    }

    pub fn state(&self) -> UpdateState {
        lock(&self.inner.fields).state
    }

    pub fn set_game_running_check(&self, check: impl Fn() -> bool + Send + Sync + 'static) {
        lock(&self.inner.fields).game_running = Arc::new(check);
    }

    pub fn set_version_events(&self, events: Option<Arc<dyn LauncherVersionEvents>>) {
        *lock(&self.inner.version_events) = events;
    }

    pub fn set_update_telemetry(&self, telemetry: Option<Arc<dyn UpdateTelemetry>>) {
        *lock(&self.inner.telemetry) = telemetry;
    }

    pub fn set_window(&self, window: Option<Arc<dyn UpdateWindow>>) {
        *lock(&self.inner.window) = window;
    }

    /// `updater-get-state`.
    pub fn get_state(&self) -> Value {
        self.inner.snapshot(&lock(&self.inner.fields))
    }

    /// `updater-check`.
    pub fn check(&self) -> bool {
        self.inner.check_updates("manual")
    }

    /// `checkOnFocus()`.
    pub fn check_on_focus(&self) {
        self.inner.check_updates("focus");
    }

    /// `updater-download`.
    pub fn download(&self) -> bool {
        self.inner.download()
    }

    /// `updater-cancel`.
    pub fn cancel(&self) -> bool {
        let mut fields = lock(&self.inner.fields);
        let Some(abort) = fields.abort.clone() else {
            return false;
        };
        if !matches!(
            fields.state,
            UpdateState::Downloading | UpdateState::Verifying
        ) {
            return false;
        }
        fields.cancel_requested = true;
        drop(fields);
        abort.cancel();
        true
    }

    /// `updater-install-now`: установить, перезапустить новую версию и выйти.
    pub fn install_now(&self) -> bool {
        self.inner.install_now()
    }

    /// `updater-install-on-quit`.
    pub fn install_on_quit(&self) -> bool {
        let mut fields = lock(&self.inner.fields);
        if !self.inner.enabled
            || fields.state != UpdateState::Downloaded
            || fields.installer.is_none()
        {
            return false;
        }
        fields.install_on_quit = true;
        true
    }

    /// `updater-set-language`: на Linux язык не хранится.
    pub fn set_language(&self, language: &str) -> &'static str {
        if language == "en" { "en" } else { "ru" }
    }

    /// `boot-gate-run`.
    pub async fn boot_gate_run(&self, retry: bool) -> BootDecision {
        let run = {
            let mut fields = lock(&self.inner.fields);
            if retry
                && fields
                    .boot_decision
                    .as_ref()
                    .is_some_and(|decision| decision.action == BootAction::Blocked)
            {
                fields.boot = None;
            }
            match &fields.boot {
                Some(run) => run.clone(),
                None => {
                    let run = Arc::new(BootRun {
                        result: Mutex::new(None),
                        done: Completion::new(),
                    });
                    fields.boot = Some(run.clone());
                    drop(fields);
                    let (inner, task) = (self.inner.clone(), run.clone());
                    tokio::spawn(async move {
                        let decision = inner.execute_boot_gate().await;
                        lock(&inner.fields).boot_decision = Some(decision.clone());
                        *lock(&task.result) = Some(decision);
                        task.done.complete();
                    });
                    run
                }
            }
        };
        self.wait_boot(&run).await
    }

    async fn wait_boot(&self, run: &Arc<BootRun>) -> BootDecision {
        run.done.wait().await;
        lock(&run.result).clone().expect("решение экрана загрузки")
    }

    /// `startCheckingInterval()`. После `dispose()` цикл не заводится: у 0.4.7 `this.timer` остаётся
    /// взведённым и повторный вызов выходит сразу, здесь `dispose` снимает таймер, поэтому
    /// `disposed` проверяется отдельно.
    pub fn start_checking_interval(&self) {
        let inner = &self.inner;
        let boot_ran = {
            let fields = lock(&inner.fields);
            if !inner.enabled || fields.disposed || fields.timer.is_some() {
                return;
            }
            fields.boot.is_some()
        };
        let worker = inner.clone();
        let handle = tokio::spawn(async move {
            loop {
                let jitter = ((worker.ports.random)() * CHECK_JITTER_MS).floor() as u64;
                tokio::time::sleep(Duration::from_millis(CHECK_INTERVAL_MS as u64 + jitter)).await;
                worker.check_updates("automatic");
            }
        });
        lock(&inner.fields).timer = Some(handle.abort_handle());
        if !boot_ran {
            inner.check_updates("automatic");
        }
    }

    /// `before-quit`: `true` — выход отложить (`preventDefault`). Идёт загрузка — отменить и выйти после
    /// неё; выбрана установка при выходе — поставить версию без перезапуска.
    pub fn before_quit(&self) -> bool {
        let inner = &self.inner;
        let mut fields = lock(&inner.fields);
        if fields.disposed {
            return false;
        }
        if let Some(abort) = fields.abort.clone() {
            let first = !fields.quit_pending;
            fields.quit_pending = true;
            let download = fields.download.clone();
            drop(fields);
            abort.cancel();
            if first {
                let app = inner.ports.app.clone();
                tokio::spawn(async move {
                    if let Some(download) = download {
                        download.wait().await;
                    }
                    app.quit();
                });
            }
            return true;
        }
        if !fields.install_on_quit || fields.install_requested || (fields.game_running)() {
            return false;
        }
        drop(fields);
        !inner.apply_update(false)
    }

    /// `dispose()`.
    pub fn dispose(&self) {
        let (abort, installer, timer) = {
            let mut fields = lock(&self.inner.fields);
            fields.disposed = true;
            (
                fields.abort.clone(),
                fields.installer.clone(),
                fields.timer.take(),
            )
        };
        if let Some(timer) = timer {
            timer.abort();
        }
        if let Some(abort) = &abort {
            abort.cancel();
        }
        if let (Some(installer), None) = (installer, abort) {
            installer.discard();
        }
    }
}

impl Inner {
    fn now(&self) -> i64 {
        self.ports.clock.now_ms()
    }

    fn current(&self) -> &str {
        &self.config.current_version
    }

    fn snapshot(&self, fields: &Fields) -> Value {
        let mut map = Map::new();
        map.insert("state".into(), fields.state.as_str().into());
        map.insert("enabled".into(), self.enabled.into());
        map.insert(
            "currentVersion".into(),
            self.config.current_version.clone().into(),
        );
        map.insert(
            "version".into(),
            fields
                .info
                .as_ref()
                .map_or(Value::Null, |info| info.version.clone().into()),
        );
        map.insert(
            "releaseNotes".into(),
            fields
                .info
                .as_ref()
                .map_or(Value::Null, |info| info.release_notes.clone().into()),
        );
        map.insert(
            "info".into(),
            fields.info.as_ref().map_or(Value::Null, Info::to_value),
        );
        map.insert(
            "progress".into(),
            fields.progress.clone().unwrap_or(Value::Null),
        );
        map.insert(
            "error".into(),
            fields.error.clone().map_or(Value::Null, Value::from),
        );
        map.insert(
            "errorCode".into(),
            fields.error_code.map_or(Value::Null, Value::from),
        );
        map.insert("checkSource".into(), fields.check_source.into());
        map.insert(
            "cancellable".into(),
            (fields.abort.is_some()
                && matches!(
                    fields.state,
                    UpdateState::Downloading | UpdateState::Verifying
                ))
            .into(),
        );
        Value::Object(map)
    }

    /// `send(state, extra)`: поля заменяются, снимок уходит в окно (если служба не закрыта).
    fn send(&self, state: UpdateState, patch: Patch) -> Value {
        let (snapshot, disposed) = {
            let mut fields = lock(&self.fields);
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
            fields.state = state;
            (self.snapshot(&fields), fields.disposed)
        };
        if !disposed && let Some(window) = lock(&self.window).clone() {
            window.send_state(snapshot.clone());
        }
        snapshot
    }

    fn telemetry(&self, event: Telemetry<'_>) {
        let Some(telemetry) = lock(&self.telemetry).clone() else {
            return;
        };
        let current = self.current();
        let result = match event {
            Telemetry::Available(next) => telemetry.available(current, Some(next)),
            Telemetry::DownloadStarted(next) => telemetry.download_started(current, Some(next)),
            Telemetry::DownloadCompleted(next, bytes, duration) => {
                telemetry.download_completed(current, next, Some(bytes), Some(duration))
            }
            Telemetry::Failed(next, code) => telemetry.failed(current, next, Some(code)),
        };
        if let Err(error) = result {
            self.ports
                .logger
                .warn(&format!("[updater] telemetry: {error}"));
        }
    }

    fn game_running(&self) -> bool {
        let check = lock(&self.fields).game_running.clone();
        check()
    }

    /// `handleError(error)`.
    fn handle_error(&self, text: &str) {
        let code = error_code(text);
        let next = {
            let mut fields = lock(&self.fields);
            fields.backoff_ms = if fields.backoff_ms != 0 {
                (fields.backoff_ms * 2).min(CHECK_INTERVAL_MS)
            } else {
                60_000.min(CHECK_INTERVAL_MS)
            };
            fields.backoff_until = self.now()
                + if code == "RATE_LIMIT" {
                    RATE_LIMIT_PAUSE_MS
                } else {
                    fields.backoff_ms
                };
            fields.install_on_quit = false;
            fields.info.as_ref().map(|info| info.version.clone())
        };
        self.send(
            UpdateState::Error,
            Patch {
                error: Some(Some(js::slice_utf16(text, 500))),
                error_code: Some(Some(code)),
                progress: Some(None),
                ..Patch::default()
            },
        );
        self.telemetry(Telemetry::Failed(next.as_deref(), code));
    }

    /// `checkUpdates(source)`.
    fn check_updates(self: &Arc<Self>, source: &str) -> bool {
        let done = {
            let mut fields = lock(&self.fields);
            if !self.enabled
                || fields.disposed
                || fields.check.is_some()
                || matches!(
                    fields.state,
                    UpdateState::Downloading | UpdateState::Verifying | UpdateState::Downloaded
                )
            {
                return false;
            }
            let now = self.now();
            let age = if source == "focus" { 5 } else { 10 } * 60_000;
            if source != "manual"
                && (now < fields.backoff_until || now - fields.last_check_at < age)
            {
                return false;
            }
            fields.check_source = if source == "manual" {
                "manual"
            } else {
                "automatic"
            };
            fields.last_check_at = now;
            let done = Completion::new();
            fields.check = Some(done.clone());
            done
        };
        self.send(
            UpdateState::Checking,
            Patch {
                error: Some(None),
                error_code: Some(None),
                info: Some(None),
                ..Patch::default()
            },
        );
        lock(&self.fields).gate_result = None;
        let evaluation = self.ports.gate.evaluate();
        let this = self.clone();
        tokio::spawn(async move {
            let result = evaluation.await;
            if let Err(error) = this.after_check(result) {
                this.handle_error(&error);
            }
            lock(&this.fields).check = None;
            done.complete();
        });
        true
    }

    /// Остаток `performCheck` после ответа шлюза.
    fn after_check(&self, result: Result<Outcome, String>) -> Result<(), String> {
        let outcome = result?;
        lock(&self.fields).gate_result = Some(outcome.clone());
        if !outcome.ok {
            return Err(format!("метаданные: {}", outcome.problems.join("; ")));
        }
        {
            let mut fields = lock(&self.fields);
            if fields.disposed {
                return Ok(());
            }
            fields.backoff_ms = 0;
            fields.backoff_until = 0;
        }
        if !outcome.update_available {
            self.send(UpdateState::NotAvailable, Patch::default());
            return Ok(());
        }
        let manifest = outcome.manifest.clone().unwrap_or(Value::Null);
        let info = Info {
            version: js::to_string(&manifest["version"]),
            release_date: manifest.get("publishedAt").cloned(),
            release_notes: release_notes(&manifest),
        };
        let has_notes = !info.release_notes.is_empty();
        let version = info.version.clone();
        if lock(&self.fields).disposed {
            return Ok(());
        }
        self.send(
            UpdateState::Available,
            Patch {
                info: Some(Some(info)),
                ..Patch::default()
            },
        );
        self.telemetry(Telemetry::Available(&version));
        if let Some(events) = lock(&self.version_events).clone()
            && let Err(error) =
                events.launcher_update_available(&version, self.current(), has_notes)
        {
            self.ports
                .logger
                .warn(&format!("[updater] notification: {error}"));
        }
        Ok(())
    }

    /// `download()`.
    fn download(self: &Arc<Self>) -> bool {
        let (done, manifest) = {
            let mut fields = lock(&self.fields);
            let ok_gate = fields.gate_result.as_ref().is_some_and(|gate| gate.ok);
            if !self.enabled
                || fields.disposed
                || fields.state != UpdateState::Available
                || !ok_gate
            {
                return false;
            }
            fields.check_source = "manual";
            let done = Completion::new();
            fields.download = Some(done.clone());
            fields.cancel_requested = false;
            let manifest = fields
                .gate_result
                .as_ref()
                .and_then(|gate| gate.manifest.clone())
                .unwrap_or(Value::Null);
            (done, manifest)
        };
        let version = js::to_string(&manifest["version"]);
        self.send(
            UpdateState::Downloading,
            Patch {
                progress: Some(None),
                ..Patch::default()
            },
        );
        self.telemetry(Telemetry::DownloadStarted(&version));
        let started = self.now();
        let directory = match self.download_directory() {
            Ok(directory) => directory,
            Err(error) => {
                let this = self.clone();
                tokio::spawn(async move {
                    this.handle_error(&error);
                    done.complete();
                });
                return true;
            }
        };
        let token = CancelToken::new();
        lock(&self.fields).abort = Some(token.clone());
        let this = self.clone();
        let work = this
            .clone()
            .perform_download(manifest, directory.clone(), token, started);
        tokio::spawn(async move {
            let result = work.await;
            let failure = match result {
                Ok(()) => None,
                Err(error) => {
                    if lock(&this.fields).cancel_requested {
                        if let Some(installer) = lock(&this.fields).installer.clone() {
                            installer.discard();
                        }
                        this.send(
                            UpdateState::Available,
                            Patch {
                                progress: Some(None),
                                error: Some(None),
                                error_code: Some(None),
                                ..Patch::default()
                            },
                        );
                        None
                    } else {
                        Some(error)
                    }
                }
            };
            let _ = super::remove_all(&directory);
            lock(&this.fields).abort = None;
            if let Some(error) = failure {
                this.handle_error(&error);
            }
            done.complete();
        });
        true
    }

    /// `downloadDirectory()`: `.downloads` целиком заново (0700), внутри `update-XXXXXX`.
    fn download_directory(&self) -> Result<PathBuf, String> {
        let root = self.config.install_root.clone().unwrap_or_default();
        let base = root.join(".downloads");
        super::remove_all(&base).map_err(|error| super::fs_error(&error, "rm", &base))?;
        super::make_dirs(&base, 0o700).map_err(|error| super::fs_error(&error, "mkdir", &base))?;
        let prefix = base.join("update-");
        super::mkdtemp(&prefix).map_err(|error| super::fs_error(&error, "mkdtemp", &prefix))
    }

    fn installer(&self) -> Option<Arc<dyn VersionInstaller>> {
        lock(&self.fields).installer.clone()
    }

    fn new_installer(&self) -> Arc<dyn VersionInstaller> {
        if let Some(previous) = self.installer() {
            previous.discard();
        }
        let installer = (self.ports.create_installer)();
        lock(&self.fields).installer = Some(installer.clone());
        installer
    }

    fn disposed(&self) -> bool {
        lock(&self.fields).disposed
    }

    fn progress_sink(self: &Arc<Self>) -> ProgressSink {
        let this = self.clone();
        Arc::new(move |progress: Value| {
            if !this.disposed() {
                this.send(
                    UpdateState::Downloading,
                    Patch {
                        progress: Some(Some(progress)),
                        ..Patch::default()
                    },
                );
            }
        })
    }

    /// `performDownload()` после синхронного начала.
    async fn perform_download(
        self: Arc<Self>,
        manifest: Value,
        directory: PathBuf,
        token: CancelToken,
        started: i64,
    ) -> Result<(), String> {
        let on_progress = self.progress_sink();
        let mut bytes = manifest["installer"]["size"].as_f64().unwrap_or(f64::NAN);
        let mut staged = false;
        if let Some(delta) = channel::delta_of(&manifest).cloned() {
            match self
                .download_chunks(&manifest, delta, &directory, &token, on_progress.clone())
                .await
            {
                Ok(downloaded) => {
                    bytes = downloaded as f64;
                    staged = true;
                }
                Err(error) => {
                    if token.is_cancelled() || self.disposed() {
                        return Err(error);
                    }
                    if let Some(installer) = self.installer() {
                        installer.discard();
                    }
                    self.ports.logger.warn(&format!(
                        "[updater] чанки не сработали, качаю установщик целиком: {error}"
                    ));
                    self.send(
                        UpdateState::Downloading,
                        Patch {
                            progress: Some(None),
                            ..Patch::default()
                        },
                    );
                }
            }
        }
        if !staged {
            self.download_full(&manifest, &directory, &token, on_progress)
                .await?;
        }
        if self.disposed() {
            if let Some(installer) = self.installer() {
                installer.discard();
            }
            return Ok(());
        }
        if token.is_cancelled() {
            return Err(ABORTED.to_owned());
        }
        self.send(UpdateState::Downloaded, Patch::default());
        let version = js::to_string(&manifest["version"]);
        self.telemetry(Telemetry::DownloadCompleted(
            &version,
            bytes,
            self.now() - started,
        ));
        Ok(())
    }

    /// `downloadChunks(...)`: скачанные байты.
    async fn download_chunks(
        &self,
        manifest: &Value,
        delta: Value,
        directory: &Path,
        token: &CancelToken,
        on_progress: ProgressSink,
    ) -> Result<u64, String> {
        let version = js::to_string(&manifest["version"]);
        let chunk_base_urls: Vec<String> = delta["chunkBaseUrls"]
            .as_array()
            .map(|urls| urls.iter().map(js::to_string).collect())
            .unwrap_or_default();
        let chunk_manifest = self
            .ports
            .chunk_manifests
            .fetch(delta, version.clone(), token.clone())
            .await?;
        let installer = self.new_installer();
        let result = installer
            .prepare_from_chunks(
                chunk_manifest,
                manifest.clone(),
                ChunkPrepare {
                    cache_dir: directory.join("chunks"),
                    chunk_base_urls,
                    cancel: token.clone(),
                    on_progress,
                },
            )
            .await?;
        let text = |value: Option<u64>| {
            value.map_or_else(|| "undefined".to_owned(), |value| value.to_string())
        };
        self.ports.logger.info(&format!(
            "[updater] {version} собрана из чанков: скачано {} байт, с диска {}",
            text(result.map(|result| result.downloaded_bytes)),
            text(result.map(|result| result.reused_bytes)),
        ));
        Ok(result.map_or(0, |result| result.downloaded_bytes))
    }

    /// `downloadFull(...)`: AppImage, вшитый архив, его версия == версии канала, подготовка.
    async fn download_full(
        &self,
        manifest: &Value,
        directory: &Path,
        token: &CancelToken,
        on_progress: ProgressSink,
    ) -> Result<(), String> {
        let file = self
            .ports
            .download
            .download(
                manifest.clone(),
                directory.to_owned(),
                token.clone(),
                on_progress,
            )
            .await?;
        if self.disposed() {
            return Ok(());
        }
        self.send(
            UpdateState::Verifying,
            Patch {
                progress: Some(None),
                ..Patch::default()
            },
        );
        let payload = self
            .ports
            .extract
            .extract(file.clone(), directory.to_owned(), token.clone())
            .await?;
        let _ = super::remove_all(&file);
        if payload.manifest["version"] != manifest["version"] {
            return Err(format!(
                "Установщик содержит HLSR {}, а подписанный канал — {}",
                js::to_string(&payload.manifest["version"]),
                js::to_string(&manifest["version"])
            ));
        }
        if token.is_cancelled() {
            return Err(ABORTED.to_owned());
        }
        let installer = self.new_installer();
        installer
            .prepare(
                payload.archive.clone(),
                payload.manifest.clone(),
                token.clone(),
            )
            .await
    }

    /// `applyUpdate(relaunch)`.
    fn apply_update(self: &Arc<Self>, relaunch: bool) -> bool {
        let (installer, next) = {
            let fields = lock(&self.fields);
            let allowed = self.enabled
                && fields.state == UpdateState::Downloaded
                && fields.installer.is_some()
                && !fields.install_requested;
            if !allowed {
                return false;
            }
            (
                fields.installer.clone().expect("установщик проверен"),
                fields
                    .info
                    .as_ref()
                    .map(|info| info.version.clone())
                    .unwrap_or_default(),
            )
        };
        if self.game_running() {
            return false;
        }
        let target = match installer.commit() {
            Ok(target) => target,
            Err(error) => {
                self.handle_error(&error);
                return false;
            }
        };
        if relaunch
            && let Err(error) = self
                .ports
                .relauncher
                .relaunch(&target, &relaunch_arguments(&self.config.argv))
        {
            if let Err(rollback) = installer.rollback() {
                self.ports
                    .logger
                    .error(&format!("[updater] rollback: {rollback}"));
            }
            self.handle_error(&error);
            return false;
        }
        lock(&self.fields).install_requested = true;
        if let Some(events) = lock(&self.version_events).clone()
            && let Err(error) = events.write_pending_receipt(self.current(), &next)
        {
            self.ports
                .logger
                .warn(&format!("[updater] receipt: {error}"));
        }
        true
    }

    /// `installNow()`.
    fn install_now(self: &Arc<Self>) -> bool {
        if !self.apply_update(true) {
            return false;
        }
        let app = self.ports.app.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            app.quit();
        });
        true
    }

    /// `executeBootGate()`.
    async fn execute_boot_gate(self: &Arc<Self>) -> BootDecision {
        let decision = |action, reason| BootDecision {
            action,
            reason,
            version: None,
            error: None,
            warn: None,
            mandatory: None,
        };
        if !self.enabled {
            return decision(BootAction::Proceed, "updates-disabled");
        }
        self.check_updates("manual");
        let check = lock(&self.fields).check.clone();
        if let Some(check) = check {
            let finished = tokio::select! {
                () = check.wait() => true,
                () = tokio::time::sleep(BOOT_TIMEOUT) => false,
            };
            if !finished {
                return decision(BootAction::Proceed, "check-timeout");
            }
        }
        if self.game_running() {
            return decision(BootAction::Proceed, "game-running");
        }
        if lock(&self.fields).state == UpdateState::Available {
            self.download();
            let download = lock(&self.fields).download.clone();
            if let Some(download) = download {
                download.wait().await;
            }
        }
        if lock(&self.fields).state == UpdateState::Downloaded && self.install_now() {
            let version = lock(&self.fields)
                .info
                .as_ref()
                .map(|info| info.version.clone());
            return BootDecision {
                version,
                ..decision(BootAction::Installing, "installing")
            };
        }
        let fields = lock(&self.fields);
        let mandatory = fields
            .gate_result
            .as_ref()
            .is_some_and(|gate| gate.update_required || gate.unsupported);
        if fields.state == UpdateState::Error && mandatory {
            return BootDecision {
                error: fields.error.clone(),
                ..decision(BootAction::Blocked, "mandatory-update-failed")
            };
        }
        decision(BootAction::Proceed, fields.state.as_str())
    }
}

// ------------------------------------------------------------------- реализации портов

/// QA-сборка (`local-qa.js`, `HLSR_BUILD_CHANNEL=qa`): шлюз локального канала и адрес запроса
/// установщика с loopback (подписанные адреса и allowlist остаются боевыми). `None` — обычная сборка или
/// лента не на loopback. В выпуске без `dev-hooks` функции нет.
#[cfg(any(debug_assertions, feature = "dev-hooks"))]
pub fn qa_overrides(
    current_version: &str,
    logger: Arc<dyn Logger>,
) -> Option<(Arc<dyn LinuxGate>, Rewrite)> {
    let qa = crate::local_qa::configuration_from_process(
        hlsr_core::js::Platform::Linux,
        current_version,
        logger,
    )?;
    let feed = qa.feed.clone();
    let rewrite: Rewrite = Arc::new(move |url: &str| {
        crate::local_qa::feed_file_url(&feed, url).unwrap_or_else(|| url.to_owned())
    });
    Some((Arc::new(GateAdapter(Arc::new(qa.gate))), rewrite))
}

/// Загрузка AppImage по HTTP ([`ArchiveDownload`]); у QA-сборки адрес запроса — с loopback.
pub struct HttpAppImageDownload {
    pub client: ArchiveDownload,
    pub rewrite: Option<Rewrite>,
    pub is_allowed: AllowUrl,
    pub clock: Arc<dyn Clock>,
}

impl AppImageDownloader for HttpAppImageDownload {
    fn download(
        &self,
        manifest: Value,
        directory: PathBuf,
        cancel: CancelToken,
        on_progress: ProgressSink,
    ) -> BoxFuture<'static, Result<PathBuf, String>> {
        let client = self.client.clone();
        let options = DownloadOptions {
            is_allowed: self.is_allowed.clone(),
            cancel: Some(cancel),
            on_progress: Some(Arc::new(
                move |progress: super::archive_download::DownloadProgress| {
                    on_progress(progress.to_json())
                },
            )),
            rewrite: self.rewrite.clone(),
            clock: self.clock.clone(),
            ..DownloadOptions::default()
        };
        Box::pin(async move { client.download(&manifest, &directory, &options).await })
    }
}

/// Извлечение рантаймом AppImage (`--appimage-extract resources/payload`).
pub struct AppImageExtractor {
    pub runner: Arc<dyn SyncRunner>,
    /// Окружение для рантайма (без `SLINT_*`/`WINIT_*`).
    pub env: EnvVars,
}

impl PayloadExtractor for AppImageExtractor {
    fn extract(
        &self,
        file: PathBuf,
        directory: PathBuf,
        cancel: CancelToken,
    ) -> BoxFuture<'static, Result<Payload, String>> {
        let (runner, env) = (self.runner.clone(), self.env.clone());
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                embedded_payload::extract_from(
                    &file,
                    &directory,
                    &env,
                    runner.as_ref(),
                    Some(&cancel),
                )
            })
            .await
            .map_err(|error| error.to_string())?
        })
    }
}

/// Манифест чанков с сервера (`chunks::fetch_manifest`).
pub struct HttpChunkManifests {
    pub fetch: Arc<dyn ChunkFetch>,
    pub is_allowed: AllowUrl,
}

impl ChunkManifests for HttpChunkManifests {
    fn fetch(
        &self,
        delta: Value,
        version: String,
        cancel: CancelToken,
    ) -> BoxFuture<'static, Result<Manifest, String>> {
        let (fetch, is_allowed) = (self.fetch.clone(), self.is_allowed.clone());
        Box::pin(async move {
            let delta = Delta::from_value(&delta, |url| is_allowed(url))
                .map_err(|problems| format!("Манифест чанков: {}", problems.join("; ")))?;
            chunks::fetch_manifest(
                &delta,
                &FetchManifestOptions {
                    fetch: fetch.as_ref(),
                    is_allowed,
                    expected: Expected {
                        version: Some(&version),
                        platform: Some("linux"),
                    },
                    cancel: Some(cancel),
                },
            )
            .await
            .map_err(|error| error.to_string())
        })
    }
}

/// [`ArchiveInstall`] как установщик службы.
pub struct ArchiveInstaller {
    install: tokio::sync::Mutex<ArchiveInstall>,
    fetch: Arc<dyn ChunkFetch>,
    is_allowed: AllowUrl,
    log: Arc<dyn Logger>,
    clock: Arc<dyn Clock>,
}

impl ArchiveInstaller {
    pub fn new(
        root: &Path,
        fetch: Arc<dyn ChunkFetch>,
        is_allowed: AllowUrl,
        log: Arc<dyn Logger>,
        clock: Arc<dyn Clock>,
    ) -> ArchiveInstaller {
        ArchiveInstaller {
            install: tokio::sync::Mutex::new(ArchiveInstall::new(root)),
            fetch,
            is_allowed,
            log,
            clock,
        }
    }
}

impl VersionInstaller for ArchiveInstaller {
    fn prepare(
        &self,
        archive: PathBuf,
        manifest: Value,
        cancel: CancelToken,
    ) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let mut install = self.install.lock().await;
            install.prepare(&archive, &manifest, Some(&cancel)).await
        })
    }

    fn prepare_from_chunks(
        &self,
        manifest: Manifest,
        channel_manifest: Value,
        options: ChunkPrepare,
    ) -> BoxFuture<'_, Result<Option<ChunkResult>, String>> {
        Box::pin(async move {
            let mut install = self.install.lock().await;
            let on_progress = options.on_progress.clone();
            let mut stage = ChunkStage::new(
                options.cache_dir,
                options.chunk_base_urls,
                self.fetch.clone(),
            );
            stage.is_allowed = self.is_allowed.clone();
            stage.cancel = Some(options.cancel);
            stage.on_progress = Some(Arc::new(move |progress: chunks::Progress| {
                on_progress(serde_json::json!({
                    "done": progress.done,
                    "total": progress.total,
                    "bytesPerSecond": progress.bytes_per_second,
                }))
            }));
            stage.log = Some(self.log.clone());
            stage.clock = self.clock.clone();
            install
                .prepare_from_chunks(manifest, &channel_manifest, stage)
                .await?;
            Ok(install.chunk_result().map(|result| ChunkResult {
                downloaded_bytes: result.downloaded_bytes,
                reused_bytes: result.reused_bytes,
            }))
        })
    }

    fn commit(&self) -> Result<PathBuf, String> {
        match self.install.try_lock() {
            Ok(mut install) => install.commit(),
            Err(_) => Err("Нет подготовленной версии".to_owned()),
        }
    }

    fn rollback(&self) -> Result<(), String> {
        match self.install.try_lock() {
            Ok(mut install) => install.rollback(),
            Err(_) => Err("Нет подготовленной версии".to_owned()),
        }
    }

    fn discard(&self) {
        if let Ok(mut install) = self.install.try_lock() {
            install.discard();
        }
    }
}
