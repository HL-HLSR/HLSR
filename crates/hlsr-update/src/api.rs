//! `UpdateApi` — фасад обновления для окна лаунчера (карта §4.1), по образцу `DeliveryApi`: метод
//! на канал и [`UpdateApi::call`] по имени канала, события окну — [`UpdateEvent`].
//!
//! Каналы: `boot-gate-run`, `updater-check`, `updater-download`, `updater-cancel`,
//! `updater-install-now`, `updater-install-on-quit`, `updater-set-language`, `updater-get-state`
//! (`UpdateManager.js:533-651` и `linux/UpdateManager.js:87-94`), `diagnostics-run`,
//! `diagnostics-cancel`, `export-diagnostics-zip` (`AppManager.js:846-987`). События:
//! `updater-state` (снимок службы), `diagnostics-progress` (`{runId, …строка проверки}`). Окна
//! установщика и апдейтера (§4.2–4.3: `installer-state`, `updater:init/stage/progress`) живут в
//! своих режимах запуска, их каналы — [`channels`]. `open-logs-folder` и `app-version` — ПП1/ПП5.
//!
//! Служба обновления — Windows ([`UpdateManager`]) или Linux ([`LinuxUpdateManager`]) за трейтом
//! [`UpdateService`]; что даёт ОС (сеть, реестр, процессы, диалоги) — [`system`]. Не переносится
//! (решение 19): канал `export-diagnostics` недостижим в 0.4.7.

pub mod system;

use crate::boot_gate::{BootDecision, UpdateState};
use crate::diagnostics::runner::{Check, DiagnosticsRunner};
use crate::diagnostics::service::{DiagnosticsEvents, DiagnosticsService};
use crate::diagnostics::zip::{ZipData, ZipExporter};
use crate::linux::update_manager::LinuxUpdateManager;
use crate::ports::{BoxFuture, Logger};
use crate::update_manager::{UpdateManager, UpdateNotifier, UpdateWindow};
use hlsr_core::js;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Имена каналов и событий (карта §4.1–4.3).
pub mod channels {
    pub const BOOT_GATE_RUN: &str = "boot-gate-run";
    pub const UPDATER_CHECK: &str = "updater-check";
    pub const UPDATER_DOWNLOAD: &str = "updater-download";
    pub const UPDATER_CANCEL: &str = "updater-cancel";
    pub const UPDATER_INSTALL_NOW: &str = "updater-install-now";
    pub const UPDATER_INSTALL_ON_QUIT: &str = "updater-install-on-quit";
    pub const UPDATER_SET_LANGUAGE: &str = "updater-set-language";
    pub const UPDATER_GET_STATE: &str = "updater-get-state";
    pub const DIAGNOSTICS_RUN: &str = "diagnostics-run";
    pub const DIAGNOSTICS_CANCEL: &str = "diagnostics-cancel";
    pub const EXPORT_DIAGNOSTICS_ZIP: &str = "export-diagnostics-zip";
    /// Событие: снимок службы обновления.
    pub const UPDATER_STATE: &str = "updater-state";
    /// Событие: строка проверки диагностики с номером прогона.
    pub const DIAGNOSTICS_PROGRESS: &str = "diagnostics-progress";
    /// Окно установщика (§4.2): состояние.
    pub const INSTALLER_STATE: &str = "installer-state";
    /// Окно апдейтера (§4.3).
    pub const UPDATER_INIT: &str = "updater:init";
    pub const UPDATER_STAGE: &str = "updater:stage";
    pub const UPDATER_PROGRESS: &str = "updater:progress";
}

/// Событие окну (`webContents.send(channel, payload)`).
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateEvent {
    pub channel: &'static str,
    pub payload: Value,
}

/// Сколько событий окно может не забрать, прежде чем старые потеряются.
const EVENT_CAPACITY: usize = 256;

/// Служба обновления лаунчера с точки зрения окна: Windows или Linux.
pub trait UpdateService: Send + Sync {
    /// Обновления включены (сборка, установка, QA-лента).
    fn enabled(&self) -> bool;
    fn state(&self) -> UpdateState;
    /// `updater-get-state`.
    fn get_state(&self) -> Value;
    /// `updater-check`.
    fn check(&self) -> bool;
    /// `updater-download`: `true`, `"external"` или `false`.
    fn download(&self) -> BoxFuture<'_, Value>;
    /// `updater-cancel`.
    fn cancel(&self) -> bool;
    /// `updater-install-now`.
    fn install_now(&self) -> bool;
    /// `updater-install-on-quit`.
    fn install_on_quit(&self) -> bool;
    /// `updater-set-language`.
    fn set_language(&self, language: &str) -> String;
    /// `boot-gate-run`.
    fn boot_gate_run(&self, retry: bool) -> BoxFuture<'_, BootDecision>;
    /// `initialize()`: строка журнала о запуске (Windows).
    fn initialize(&self) {}
    /// `startCheckingInterval()` — когда окно загрузилось.
    fn start_checking_interval(&self);
    /// `checkOnFocus()`.
    fn check_on_focus(&self);
    fn set_window(&self, window: Option<Arc<dyn UpdateWindow>>);
    fn set_game_running_check(&self, check: Arc<dyn Fn() -> bool + Send + Sync>);
    /// `before-quit` (Linux): `true` — выход отложить, служба сама вызовет выход позже.
    fn before_quit(&self) -> bool {
        false
    }
    /// Выход с кодом (Windows): установка при выходе (`addQuitHandler` electron-updater).
    fn on_app_quit(&self, _exit_code: i32) {}
    /// `dispose()`.
    fn dispose(&self);
}

impl UpdateService for UpdateManager {
    fn enabled(&self) -> bool {
        UpdateManager::enabled(self)
    }

    fn state(&self) -> UpdateState {
        UpdateManager::state(self)
    }

    fn get_state(&self) -> Value {
        UpdateManager::get_state(self)
    }

    fn check(&self) -> bool {
        UpdateManager::check(self)
    }

    fn download(&self) -> BoxFuture<'_, Value> {
        Box::pin(async move { UpdateManager::download(self).await.to_json() })
    }

    fn cancel(&self) -> bool {
        UpdateManager::cancel(self)
    }

    fn install_now(&self) -> bool {
        UpdateManager::install_now(self)
    }

    fn install_on_quit(&self) -> bool {
        UpdateManager::install_on_quit(self)
    }

    fn set_language(&self, language: &str) -> String {
        UpdateManager::set_language(self, language)
    }

    fn boot_gate_run(&self, retry: bool) -> BoxFuture<'_, BootDecision> {
        Box::pin(UpdateManager::boot_gate_run(self, retry))
    }

    fn initialize(&self) {
        UpdateManager::initialize(self);
    }

    fn start_checking_interval(&self) {
        UpdateManager::start_checking_interval(self);
    }

    fn check_on_focus(&self) {
        UpdateManager::check_on_focus(self);
    }

    fn set_window(&self, window: Option<Arc<dyn UpdateWindow>>) {
        UpdateManager::set_window(self, window);
    }

    fn set_game_running_check(&self, check: Arc<dyn Fn() -> bool + Send + Sync>) {
        UpdateManager::set_game_running_check(self, move || check());
    }

    fn on_app_quit(&self, exit_code: i32) {
        UpdateManager::on_app_quit(self, exit_code);
    }

    fn dispose(&self) {
        UpdateManager::dispose(self);
    }
}

impl UpdateService for LinuxUpdateManager {
    fn enabled(&self) -> bool {
        LinuxUpdateManager::enabled(self)
    }

    fn state(&self) -> UpdateState {
        LinuxUpdateManager::state(self)
    }

    fn get_state(&self) -> Value {
        LinuxUpdateManager::get_state(self)
    }

    fn check(&self) -> bool {
        LinuxUpdateManager::check(self)
    }

    fn download(&self) -> BoxFuture<'_, Value> {
        let started = LinuxUpdateManager::download(self);
        Box::pin(async move { json!(started) })
    }

    fn cancel(&self) -> bool {
        LinuxUpdateManager::cancel(self)
    }

    fn install_now(&self) -> bool {
        LinuxUpdateManager::install_now(self)
    }

    fn install_on_quit(&self) -> bool {
        LinuxUpdateManager::install_on_quit(self)
    }

    fn set_language(&self, language: &str) -> String {
        LinuxUpdateManager::set_language(self, language).to_owned()
    }

    fn boot_gate_run(&self, retry: bool) -> BoxFuture<'_, BootDecision> {
        Box::pin(LinuxUpdateManager::boot_gate_run(self, retry))
    }

    fn start_checking_interval(&self) {
        LinuxUpdateManager::start_checking_interval(self);
    }

    fn check_on_focus(&self) {
        LinuxUpdateManager::check_on_focus(self);
    }

    fn set_window(&self, window: Option<Arc<dyn UpdateWindow>>) {
        LinuxUpdateManager::set_window(self, window);
    }

    fn set_game_running_check(&self, check: Arc<dyn Fn() -> bool + Send + Sync>) {
        LinuxUpdateManager::set_game_running_check(self, move || check());
    }

    fn before_quit(&self) -> bool {
        LinuxUpdateManager::before_quit(self)
    }

    fn dispose(&self) {
        LinuxUpdateManager::dispose(self);
    }
}

/// Данные отчёта ZIP по строкам последнего прогона проверок (`lastDiagnostics`, пусто — прогона
/// не было). Собираются после выбора файла, в потоке выгрузки.
pub type ZipDataSource = Arc<dyn Fn(Vec<Check>) -> Result<ZipData, String> + Send + Sync>;

/// Диагностика фасада: проверки, выгрузка ZIP и его данные.
pub struct DiagnosticsOptions {
    pub runner: DiagnosticsRunner,
    pub exporter: ZipExporter,
    pub zip_data: ZipDataSource,
}

/// Что фасаду нужно снаружи.
pub struct UpdateApiOptions {
    pub updates: Arc<dyn UpdateService>,
    /// Окно ОС: прогресс на панели задач (`setProgressBar`); снимки `updater-state` оно тоже
    /// получает (окну лаунчера они идут событиями фасада).
    pub window: Option<Arc<dyn UpdateWindow>>,
    /// Центр уведомлений: `DIAGNOSTICS_NETWORK`.
    pub notifier: Option<Arc<dyn UpdateNotifier>>,
    /// `None` — каналы диагностики не отвечают (как будто их нет).
    pub diagnostics: Option<DiagnosticsOptions>,
    pub logger: Arc<dyn Logger>,
}

/// Окно службы обновления: снимок — событием фасада и окну ОС, прогресс — окну ОС.
struct ApiWindow {
    events: broadcast::Sender<UpdateEvent>,
    os: Option<Arc<dyn UpdateWindow>>,
}

impl UpdateWindow for ApiWindow {
    fn send_state(&self, payload: Value) {
        if let Some(os) = &self.os {
            os.send_state(payload.clone());
        }
        let _ = self.events.send(UpdateEvent {
            channel: channels::UPDATER_STATE,
            payload,
        });
    }

    fn set_progress_bar(&self, value: f64) {
        if let Some(os) = &self.os {
            os.set_progress_bar(value);
        }
    }
}

/// Диагностика → окно и центр уведомлений (`AppManager.js:890-910`).
struct ApiDiagnosticsEvents {
    events: broadcast::Sender<UpdateEvent>,
    notifier: Option<Arc<dyn UpdateNotifier>>,
    logger: Arc<dyn Logger>,
}

impl DiagnosticsEvents for ApiDiagnosticsEvents {
    fn progress(&self, run_id: u64, check: &Check) {
        // `Object.assign({ runId }, item)`.
        let mut payload = Map::new();
        payload.insert("runId".into(), json!(run_id));
        if let Value::Object(fields) = check.to_value() {
            payload.extend(fields);
        }
        let _ = self.events.send(UpdateEvent {
            channel: channels::DIAGNOSTICS_PROGRESS,
            payload: Value::Object(payload),
        });
    }

    fn network_failed(&self, detail: &str) {
        if let Some(notifier) = &self.notifier
            && let Err(error) = notifier.add("warning", "DIAGNOSTICS_NETWORK", detail)
        {
            self.logger.warn(&format!(
                "[diagnostics] уведомление DIAGNOSTICS_NETWORK не записано: {error}"
            ));
        }
    }
}

/// Фасад обновления.
pub struct UpdateApi {
    updates: Arc<dyn UpdateService>,
    diagnostics: Option<Arc<DiagnosticsService>>,
    exporter: Option<ZipExporter>,
    zip_data: Option<ZipDataSource>,
    events: broadcast::Sender<UpdateEvent>,
}

impl UpdateApi {
    /// Фасад над службой: окно службы — события фасада (и окно ОС).
    pub fn new(options: UpdateApiOptions) -> Arc<UpdateApi> {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        options.updates.set_window(Some(Arc::new(ApiWindow {
            events: events.clone(),
            os: options.window,
        })));
        let (diagnostics, exporter, zip_data) = match options.diagnostics {
            Some(parts) => {
                let service = DiagnosticsService::new(
                    parts.runner,
                    Arc::new(ApiDiagnosticsEvents {
                        events: events.clone(),
                        notifier: options.notifier,
                        logger: options.logger.clone(),
                    }),
                    options.logger.clone(),
                );
                (Some(service), Some(parts.exporter), Some(parts.zip_data))
            }
            None => (None, None, None),
        };
        Arc::new(UpdateApi {
            updates: options.updates,
            diagnostics,
            exporter,
            zip_data,
            events,
        })
    }

    /// События окну.
    pub fn subscribe(&self) -> broadcast::Receiver<UpdateEvent> {
        self.events.subscribe()
    }

    /// Служба обновления.
    pub fn updates(&self) -> &Arc<dyn UpdateService> {
        &self.updates
    }

    // ------------------------------------------------------------ жизненный цикл

    /// `updateManager.initialize()`.
    pub fn initialize(&self) {
        self.updates.initialize();
    }

    /// `setGameRunningCheck(() => gameDelivery.isAnyGameRunning())`.
    pub fn set_game_running_check(&self, check: Arc<dyn Fn() -> bool + Send + Sync>) {
        self.updates.set_game_running_check(check);
    }

    /// Окно загрузилось (`ready`): фоновая проверка.
    pub fn start_checking_interval(&self) {
        self.updates.start_checking_interval();
    }

    /// Окно получило фокус.
    pub fn check_on_focus(&self) {
        self.updates.check_on_focus();
    }

    /// `before-quit` (Linux): `true` — выход отложен.
    pub fn before_quit(&self) -> bool {
        self.updates.before_quit()
    }

    /// Выход с кодом (Windows: установка при выходе).
    pub fn on_app_quit(&self, exit_code: i32) {
        self.updates.on_app_quit(exit_code);
    }

    /// `updateManager.dispose()`.
    pub fn dispose(&self) {
        self.updates.dispose();
    }

    // ------------------------------------------------------------ каналы

    /// `boot-gate-run` (`{retry?}`).
    pub async fn boot_gate_run(&self, options: Option<&Value>) -> Value {
        let retry = options
            .and_then(|options| options.get("retry"))
            .is_some_and(js::truthy);
        self.updates.boot_gate_run(retry).await.to_json()
    }

    /// `updater-set-language`.
    pub fn updater_set_language(&self, value: Option<&Value>) -> Value {
        let language = value.and_then(Value::as_str).unwrap_or("");
        json!(self.updates.set_language(language))
    }

    /// `diagnostics-run`: `{runId, status, checks}`.
    pub async fn diagnostics_run(&self) -> Option<Value> {
        let service = self.diagnostics.as_ref()?;
        Some(service.run().await.to_value())
    }

    /// `diagnostics-cancel`.
    pub fn diagnostics_cancel(&self) -> Option<Value> {
        Some(json!(self.diagnostics.as_ref()?.cancel()))
    }

    /// `export-diagnostics-zip` (`{includeDump}`): `{ok}`, `{ok:false, cancelled}` или
    /// `{ok:false, error:"EXPORT_FAILED"}`.
    pub async fn export_diagnostics_zip(&self, options: Option<&Value>) -> Option<Value> {
        let exporter = self.exporter.as_ref()?;
        let source = self.zip_data.clone()?;
        // `!!(options && options.includeDump === true)`.
        let include_dump =
            options.and_then(|options| options.get("includeDump")) == Some(&json!(true));
        let checks = self
            .diagnostics
            .as_ref()
            .and_then(|service| service.last_checks())
            .unwrap_or_default();
        let outcome = exporter.export(include_dump, move || source(checks)).await;
        Some(outcome.to_value())
    }

    /// Канал по имени: `None` — канал не этого фасада.
    pub async fn call(&self, channel: &str, args: &[Value]) -> Option<Result<Value, String>> {
        let updates = &self.updates;
        let value = match channel {
            channels::BOOT_GATE_RUN => self.boot_gate_run(args.first()).await,
            channels::UPDATER_CHECK => json!(updates.check()),
            channels::UPDATER_DOWNLOAD => updates.download().await,
            channels::UPDATER_CANCEL => json!(updates.cancel()),
            channels::UPDATER_INSTALL_NOW => json!(updates.install_now()),
            channels::UPDATER_INSTALL_ON_QUIT => json!(updates.install_on_quit()),
            channels::UPDATER_SET_LANGUAGE => self.updater_set_language(args.first()),
            channels::UPDATER_GET_STATE => updates.get_state(),
            channels::DIAGNOSTICS_RUN => self.diagnostics_run().await?,
            channels::DIAGNOSTICS_CANCEL => self.diagnostics_cancel()?,
            channels::EXPORT_DIAGNOSTICS_ZIP => self.export_diagnostics_zip(args.first()).await?,
            _ => return None,
        };
        Some(Ok(value))
    }
}
