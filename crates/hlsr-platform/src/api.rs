//! Фасад платформы (`PlatformApi`): каналы окна для запуска, процессов, Steam, дампов падений,
//! трея во время игры и Proton (§4.4 карты) — то, что в 0.4.7 делали части
//! `game-delivery/index.js` подпроекта 3 (`attachProcessEvents`, `launch`, `handleCrash`,
//! `crashInfo`, `openCrashDump`, наблюдение за Steam, `steamHealthCheck`, `steamOwnership`,
//! друзья, `startTimerWithGame`, `captureLiveSplit`, `game-launch-*`) и `AppManager.js`
//! (`proton-*`, `native-available`, `umu-available`).
//!
//! Службу доставки по-прежнему создаёт фасад доставки ([`DeliveryApi`], `ensureService`), но
//! фабрикой платформы ([`PlatformApi::service_factory`]): вместе со службой для новой библиотеки
//! создаются Steam, депо, служба процессов игр, запуск и инструменты, и платформа подписывается
//! на события процессов. Смена библиотеки — новый набор, прежняя служба процессов
//! останавливается (`DeliveryApi::set_library_path`). События окна — [`DeliveryEvent`] через
//! `broadcast`. Каналы `profile-get` и `friends-list` 0.4.7 отвечали через `onlineProfiles`
//! (он есть всегда) — они в `OnlineApi`; Steam для них даёт [`PlatformApi::steam_account_state`]
//! и [`PlatformApi::steam_friends`].

mod launch;
mod process_events;
mod proton;
mod steam;
pub mod system;

pub use process_events::SESSION_GOALS;

use crate::crash_dumps::DumpContext;
use crate::elevation::{ELEVATION_QUIT_DELAY, ELEVATION_WAIT, Elevator, InstanceLock};
use crate::game_process_service::{GameProcessService, ListenerId, StartedProcess};
use crate::game_stats::game_stats_store::{GameStatsStore, StatsOptions};
use crate::launch_service::{ExtraEnvProvider, LaunchDelivery};
use crate::linux::proton_manager::ProtonManager;
use crate::linux::proton_runner::ProtonRunner;
use crate::livesplit_overlay::LiveSplitOverlay;
use crate::profile::steam_avatars::HttpGet;
use crate::steam_depot::SteamDepotService;
use crate::steam_fs::{RealFs, SteamFs};
use crate::tool_runner::ToolProcessRunner;
use crate::tray_controller::{TrayController, TrayOptions, TrayWindow};
use hlsr_core::js::Platform;
use hlsr_core::notifications::NotificationStore;
use hlsr_delivery::api::{DeliveryApi, DeliveryEvent, Parts, ServiceFactory, ServiceRequest};
use hlsr_delivery::environment::Environment;
use hlsr_delivery::errors::{DeliveryError, Failure};
use hlsr_delivery::game_delivery_service::GameDeliveryService;
use hlsr_delivery::game_settings_store::GameSettingsStore;
use hlsr_delivery::ports::{
    Achievements, Dialogs, Logger, NoDialogs, NoShell, PresenceSink, ProcessProbe, ProcessStopper,
    Shell, ToolRunner, TracingLogger,
};
use hlsr_delivery::providers::BoxFuture;
use hlsr_delivery::timing::{Clock, system_clock};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// Каналы событий окна.
pub mod channels {
    pub const PROCESS_RUNNING: &str = "game-process-running";
    pub const PROCESS_EXITED: &str = "game-process-exited";
    pub const PROCESS_CRASHED: &str = "game-process-crashed";
    pub const CRASH_REPORT: &str = "game-crash-report";
    pub const STEAM_ACCOUNT: &str = "steam-account";
    pub const STEAM_HEALTH: &str = "steam-health";
    pub const PROTON_STATE: &str = "proton-state";
}

/// Сколько событий окна ждёт медленного получателя.
const EVENT_CAPACITY: usize = 1024;

// ---------------------------------------------------------------- порты

/// Онлайн-профиль и телеметрия (подпроект 4): что платформа сообщает об играх.
pub trait OnlineHooks: Send + Sync {
    /// `telemetryGameStarting(gameId)`: начало игровой сессии телеметрии.
    fn game_starting(&self, _game_id: &str, _installed: Option<&Value>) {}
    /// `telemetryGameLaunchFailed(gameId, error)`: ошибка — `{code, message}`.
    fn game_launch_failed(&self, _game_id: &str, _error: &Value) {}
    /// `onlineProfiles.gameStart` и `telemetryGameRunning` (процесс `running`).
    fn game_running(&self, _game_id: &str) {}
    /// `onlineProfiles.gameEnd` и `telemetryGameFinished` (процесс `finished`).
    fn game_finished(&self, _game_id: &str, _last_exit: Option<&Value>, _crashed: bool) {}
    /// Онлайн-ветка `game-stats-get`: при входе — игра из снимка сервера.
    fn game_stats_from_server(&self, _game_id: &str) -> Option<Value> {
        None
    }
}

/// Без онлайна (тесты, отладка).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOnline;

impl OnlineHooks for NoOnline {}

/// Запуск игр (`launchService`): Windows — [`crate::launch_service::LaunchService`], Linux —
/// [`crate::linux::launch_service::LaunchService`].
pub trait GameLauncher: Send + Sync {
    /// `launch(gameId, {episode})`: `{status: "started", process, …}` или ошибка.
    fn launch<'a>(
        &'a self,
        game_id: &'a str,
        episode: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Value, DeliveryError>>;
    /// `launching.size`: идёт запуск какой-то игры.
    fn is_launching(&self) -> bool {
        false
    }
    /// `describeAvailability(gameId)`: `{ok, code, steam?, message?, …}`.
    fn describe_availability(&self, game_id: &str) -> Value;
    /// `depCrashHint(exitCode)`: только у Windows-запуска.
    fn dep_crash_hint(&self, _exit_code: Option<i64>) -> Option<String> {
        None
    }
}

/// Steam одного `ensureService`: `steamAccount` (и его `locator`).
pub trait SteamPort: Send + Sync {
    /// `refreshConnection()`: `connected`, `offline`, `elevated` или `unknown`.
    fn refresh_connection(&self) -> String;
    /// `describe()`: `{signedIn, steamId64, steamId2, personaName, avatarDataUri, connection}`.
    fn describe(&self) -> Value;
    /// `locator.locateSteam().installPath`.
    fn install_path(&self) -> Option<String>;
    /// `activeAccountId()`.
    fn active_account_id(&self) -> Option<u64>;
    /// `steamLocator.describeSteamOnly()` (здоровье Steam в Linux).
    fn describe_steam_only(&self) -> Value;
}

/// Поиск Steam, созданный для запуска (`steamLocator`).
#[derive(Clone, Default)]
pub enum SteamLocatorRef {
    #[default]
    None,
    Windows(Arc<crate::steam_locator::SteamLocator>),
    Linux(Arc<crate::linux::steam_locator::SteamLocator>),
}

/// Steam, депо и поиск Steam одного `ensureService`.
#[derive(Clone, Default)]
pub struct SteamParts {
    pub account: Option<Arc<dyn SteamPort>>,
    /// `depotService`: лицензии (`ownershipOf`) и депо.
    pub depot: Option<Arc<SteamDepotService>>,
    pub locator: SteamLocatorRef,
}

/// Порты платформы для службы доставки новой библиотеки.
pub struct DeliveryPorts {
    pub processes: GameProcessService,
    /// Steam этой службы: депо (`depotService`) для лицензий и установки сборки из депо.
    pub steam: SteamParts,
    pub tool_runner: Arc<dyn ToolRunner>,
}

impl DeliveryPorts {
    /// Служба процессов как порты доставки.
    pub fn process_probe(&self) -> (Arc<dyn ProcessProbe>, Arc<dyn ProcessStopper>) {
        let processes = Arc::new(self.processes.clone());
        (processes.clone(), processes)
    }
}

/// Что нужно запуску игр (`new LaunchService({...})` в `ensureService`).
pub struct LaunchContext {
    pub service: Arc<GameDeliveryService>,
    pub processes: GameProcessService,
    /// `delivery`: служба доставки и регистрация запущенной игры.
    pub delivery: Arc<dyn LaunchDelivery>,
    pub settings_store: Option<Arc<GameSettingsStore>>,
    pub steam: SteamParts,
    pub overlay: Option<Arc<LiveSplitOverlay>>,
    /// `extraEnvProvider`: окружение моста присутствия.
    pub extra_env: ExtraEnvProvider,
}

/// Что создаётся вместе со службой доставки для каждой библиотеки.
pub trait PlatformFactory: Send + Sync {
    /// `new SteamLocator`, `new SteamAccount`, `new SteamDepotService`.
    fn steam(&self) -> SteamParts;
    /// `new GameProcessService({paths})`: записи процессов — в `state_dir`.
    fn processes(&self, state_dir: &Path) -> GameProcessService;
    /// Запуск и остановка инструментов (LiveSplit).
    fn tool_runner(&self) -> ToolProcessRunner;
    /// `new GameDeliveryService`, `new ToolService`, `new PracticeSaves`.
    fn delivery(&self, request: ServiceRequest, ports: DeliveryPorts) -> Result<Parts, Failure>;
    /// `new LaunchService({...})`.
    fn launcher(&self, context: LaunchContext) -> Arc<dyn GameLauncher>;
}

/// Перезапуск от администратора: запуск с UAC, замок экземпляра и выход (`app.quit()`).
#[derive(Clone)]
pub struct ElevationPorts {
    pub elevator: Arc<dyn Elevator>,
    pub lock: Arc<dyn InstanceLock>,
    pub quit: Arc<dyn Fn() + Send + Sync>,
}

/// Proton (Linux): менеджер сборок и запуск в префиксе.
#[derive(Clone)]
pub struct ProtonPorts {
    pub manager: ProtonManager,
    pub runner: ProtonRunner,
}

/// Задержки и сроки (у тестов — короче).
#[derive(Debug, Clone)]
pub struct Timings {
    /// `STEAM_CONNECTION_FIRST_MS`.
    pub steam_first: Duration,
    /// `STEAM_CONNECTION_TICK_MS`.
    pub steam_tick: Duration,
    /// `STEAM_SIGNED_OUT_TICK_MS`.
    pub steam_signed_out_tick: Duration,
    /// `STEAM_SIGNED_OUT_FAST_TICKS`.
    pub steam_signed_out_fast_ticks: u32,
    /// Кеш `steamHealthCheck`, мс.
    pub steam_health_cache_ms: i64,
    /// `waitForDump`: `settleMs` и `pollMs`.
    pub dump_settle_ms: Option<u64>,
    pub dump_poll_ms: Option<u64>,
    /// `ELEVATION_WAIT_MS`, `ELEVATION_QUIT_DELAY_MS`.
    pub elevation_wait: Duration,
    pub elevation_quit_delay: Duration,
    /// Снимок окна LiveSplit: Windows — `execFile` с тайм-аутом, Linux — файл статуса.
    pub capture_timeout: Duration,
    pub linux_capture_wait: Duration,
    /// Трей: возврат окна после игры и страховка.
    pub tray_settle: Option<Duration>,
    pub tray_safety: Option<Duration>,
}

impl Default for Timings {
    fn default() -> Self {
        Timings {
            steam_first: Duration::from_millis(150),
            steam_tick: Duration::from_secs(30),
            steam_signed_out_tick: Duration::from_secs(3),
            steam_signed_out_fast_ticks: 10,
            steam_health_cache_ms: 60_000,
            dump_settle_ms: None,
            dump_poll_ms: None,
            elevation_wait: ELEVATION_WAIT,
            elevation_quit_delay: ELEVATION_QUIT_DELAY,
            capture_timeout: Duration::from_secs(15),
            linux_capture_wait: Duration::from_secs(20),
            tray_settle: None,
            tray_safety: None,
        }
    }
}

/// Параметры фасада.
pub struct PlatformOptions {
    pub logger: Arc<dyn Logger>,
    /// `userDataPath`: `game-stats.json`, `avatar-cache`.
    pub user_data_path: Option<PathBuf>,
    /// Переменные окружения (QA, `LOCALAPPDATA`, `XDG_DATA_HOME`).
    pub env: Environment,
    /// `process.platform`: `win32` или `linux`.
    pub platform: Platform,
    /// Центр уведомлений (`GAME_CRASHED`, `GAME_STATS_CORRUPT`, `PROTON_*`).
    pub notifications: Option<Arc<NotificationStore>>,
    pub online: Arc<dyn OnlineHooks>,
    pub achievements: Option<Arc<dyn Achievements>>,
    /// `onPresenceSignal`: сигналы `running`/`stopped` для Discord.
    pub presence: Option<Arc<dyn PresenceSink>>,
    pub shell: Arc<dyn Shell>,
    pub dialogs: Arc<dyn Dialogs>,
    /// Окно и значок трея: нет окна — трея во время игры нет.
    pub tray_window: Option<Arc<dyn TrayWindow>>,
    /// `isWindowVisible()`: Steam опрашивается только при видимом окне.
    pub window_visible: Arc<dyn Fn() -> bool + Send + Sync>,
    pub elevation: Option<ElevationPorts>,
    pub factory: Arc<dyn PlatformFactory>,
    pub overlay: Option<Arc<LiveSplitOverlay>>,
    /// Linux: Proton; нет — каналов `proton-*` нет.
    pub proton: Option<ProtonPorts>,
    /// Файлы Steam (друзья и аватары из кеша Steam).
    pub steam_fs: Arc<dyn SteamFs>,
    /// Запросы аватаров; по умолчанию — настоящие.
    pub avatar_get: Option<Arc<dyn HttpGet>>,
    /// Часы (`Date.now()`), мс.
    pub clock: Option<Clock>,
    pub timings: Timings,
    /// Среда tokio для таймеров и фоновой работы; по умолчанию — текущая.
    pub runtime: Option<Handle>,
}

impl PlatformOptions {
    /// Параметры с портами по умолчанию (без окна, онлайна и уведомлений).
    pub fn new(factory: Arc<dyn PlatformFactory>) -> PlatformOptions {
        PlatformOptions {
            logger: Arc::new(TracingLogger),
            user_data_path: None,
            env: Environment::process(),
            platform: Platform::CURRENT,
            notifications: None,
            online: Arc::new(NoOnline),
            achievements: None,
            presence: None,
            shell: Arc::new(NoShell),
            dialogs: Arc::new(NoDialogs),
            tray_window: None,
            window_visible: Arc::new(|| true),
            elevation: None,
            factory,
            overlay: None,
            proton: None,
            steam_fs: Arc::new(RealFs),
            avatar_get: None,
            clock: None,
            timings: Timings::default(),
            runtime: None,
        }
    }
}

// ---------------------------------------------------------------- доставка для запуска

/// `delivery` для `LaunchService`: служба доставки и служба процессов её библиотеки
/// (`GameDeliveryService.registerStartedProcess`).
pub struct DeliveryLink {
    pub service: Arc<GameDeliveryService>,
    pub processes: GameProcessService,
}

impl LaunchDelivery for DeliveryLink {
    fn is_busy(&self) -> bool {
        self.service.is_busy()
    }

    fn installed_of(&self, game_id: &str) -> Result<Value, DeliveryError> {
        self.service.installed_of(game_id)
    }

    /// `registerStartedProcess(gameId, processInfo)`: папка установки из `installed.json`
    /// (если её не передали), регистрация и статус игры.
    fn register_started_process(
        &self,
        game_id: &str,
        mut info: StartedProcess,
    ) -> Result<Value, DeliveryError> {
        let installed = self.service.installed_of(game_id)?;
        if info.install_path.is_none() {
            info.install_path = installed
                .get("installPath")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        let registered = self.processes.register_started_process(game_id, info)?;
        let _ = self.service.refresh(game_id);
        Ok(registered)
    }

    fn files_manifest_for(
        &self,
        game_id: &str,
        version: Option<&str>,
    ) -> Result<Value, DeliveryError> {
        self.service.files_manifest_for(game_id, version)
    }
}

// ---------------------------------------------------------------- состояние

/// То, что создаётся вместе со службой доставки (`ensureService`).
#[derive(Clone)]
pub(crate) struct Session {
    pub(crate) service: Arc<GameDeliveryService>,
    pub(crate) processes: GameProcessService,
    pub(crate) listener: ListenerId,
    pub(crate) steam: SteamParts,
    pub(crate) launcher: Arc<dyn GameLauncher>,
}

struct State {
    session: Option<Session>,
    /// Steam последней попытки создать службу: поднимается раньше службы (`ensureService`) и
    /// остаётся наблюдаемым, даже если служба не создалась.
    steam: Option<SteamParts>,
    /// `dumpSnapshots`.
    dump_snapshots: Vec<(String, DumpContext)>,
    /// `lastCrash` (`Map` в порядке вставки).
    last_crash: Vec<(String, Value)>,
    /// `processStarts`: время старта процесса игры (мс) — для «Марафона» и «24/7» после
    /// перезапуска лаунчера.
    process_starts: Vec<(String, i64)>,
    /// `steamHealth`.
    steam_health: Value,
    /// `steamMissWindow`.
    steam_miss_window: u32,
    steam_timer: Option<JoinHandle<()>>,
    /// `statsStore` (создаётся при первом обращении).
    stats: Option<Arc<GameStatsStore>>,
    /// `gameEnvironmentProvider`.
    game_environment: Option<ExtraEnvProvider>,
    /// `liveSplitCapture`: идущий снимок окна LiveSplit.
    capture: Option<tokio::sync::watch::Receiver<Option<Value>>>,
    proton_failed: bool,
    /// `renderActive` и `beforeLaunch` («Демо»).
    render_active: Option<RenderActive>,
    before_launch: Option<BeforeLaunch>,
}

/// `renderActive(gameId)` («Демо», `AppManager`): рендер этой игры идёт — запуск отказывает
/// `RENDER_ACTIVE`, доставка — тоже (`assertNotRendering`).
pub type RenderActive = hlsr_delivery::game_delivery_service::RenderActive;
/// `beforeLaunch(gameId)` («Демо»): снять mute, оставшийся после рендера. `Err` — строка журнала,
/// игра всё равно запускается.
pub type BeforeLaunch = Arc<dyn Fn(&str) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

/// Фасад платформы.
pub struct PlatformApi {
    me: Weak<PlatformApi>,
    options: PlatformOptions,
    delivery: Mutex<Weak<DeliveryApi>>,
    tray: Option<Arc<TrayController>>,
    state: Mutex<State>,
    events: broadcast::Sender<DeliveryEvent>,
    background: Mutex<Vec<JoinHandle<()>>>,
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Ответ `{status: "error", error: {code}}`.
pub(crate) fn error_code(code: &str) -> Value {
    json!({"status": "error", "error": {"code": code}})
}

impl PlatformApi {
    pub fn new(mut options: PlatformOptions) -> Arc<PlatformApi> {
        // Один клиент на все запросы аватаров (`friends-avatars`).
        if options.avatar_get.is_none() {
            options.avatar_get = Some(Arc::new(crate::profile::steam_avatars::WebGet::new()));
        }
        let api = Arc::new_cyclic(|me: &Weak<PlatformApi>| {
            let tray = options.tray_window.clone().map(|window| {
                let me = me.clone();
                TrayController::new(TrayOptions {
                    window: Some(window),
                    logger: Some(options.logger.clone()),
                    is_game_running: Some(Arc::new(move || {
                        me.upgrade().is_some_and(|api| api.is_any_game_running())
                    })),
                    settle: options.timings.tray_settle,
                    safety: options.timings.tray_safety,
                    runtime: options.runtime.clone(),
                })
            });
            PlatformApi {
                me: me.clone(),
                delivery: Mutex::new(Weak::new()),
                tray,
                state: Mutex::new(State {
                    session: None,
                    steam: None,
                    dump_snapshots: Vec::new(),
                    last_crash: Vec::new(),
                    process_starts: Vec::new(),
                    steam_health: json!({"status": "unknown", "code": null, "checkedAt": 0}),
                    steam_miss_window: 0,
                    steam_timer: None,
                    stats: None,
                    game_environment: None,
                    capture: None,
                    proton_failed: false,
                    render_active: None,
                    before_launch: None,
                }),
                events: broadcast::channel(EVENT_CAPACITY).0,
                background: Mutex::new(Vec::new()),
                options,
            }
        });
        api.attach_proton_events();
        api
    }

    /// Фасад доставки, через который создаются службы (`this` в `index.js`).
    pub fn bind_delivery(&self, delivery: &Arc<DeliveryApi>) {
        *lock(&self.delivery) = Arc::downgrade(delivery);
    }

    pub(crate) fn delivery(&self) -> Option<Arc<DeliveryApi>> {
        lock(&self.delivery).upgrade()
    }

    /// Получатель событий окна.
    pub fn subscribe(&self) -> broadcast::Receiver<DeliveryEvent> {
        self.events.subscribe()
    }

    /// Трей во время игры.
    pub fn tray(&self) -> Option<&Arc<TrayController>> {
        self.tray.as_ref()
    }

    pub(crate) fn logger(&self) -> &dyn Logger {
        self.options.logger.as_ref()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    pub(crate) fn now_ms(&self) -> i64 {
        match &self.options.clock {
            Some(clock) => clock(),
            None => system_clock()(),
        }
    }

    pub(crate) fn is_linux(&self) -> bool {
        self.options.platform == Platform::Linux
    }

    /// `emitTo(channel, payload)`.
    pub(crate) fn emit_to(&self, channel: &'static str, payload: Value) {
        let _ = self.events.send(DeliveryEvent { channel, payload });
    }

    /// `emitPresence(signal)`.
    pub(crate) fn emit_presence(&self, signal: Value) {
        let Some(presence) = &self.options.presence else {
            return;
        };
        if let Err(message) = presence.signal(&signal) {
            self.logger().warn(&format!(
                "[discord-rpc] сигнал presence не обработан: {message}"
            ));
        }
    }

    /// `achieve(id)`.
    pub(crate) fn achieve(&self, id: &str) {
        if let Some(achievements) = &self.options.achievements {
            achievements.achieve(id);
        }
    }

    /// Среда tokio для фоновой работы.
    pub(crate) fn runtime(&self) -> Option<Handle> {
        self.options
            .runtime
            .clone()
            .or_else(|| Handle::try_current().ok())
    }

    /// Фоновая работа (вызов `async`-функции без `await` в 0.4.7).
    pub(crate) fn spawn(&self, future: impl std::future::Future<Output = ()> + Send + 'static) {
        let Some(runtime) = self.runtime() else {
            self.logger()
                .warn("[game-delivery] фоновая задача не запущена: нет среды tokio");
            return;
        };
        let task = runtime.spawn(future);
        let mut background = lock(&self.background);
        background.retain(|task| !task.is_finished());
        background.push(task);
    }

    /// Дождаться фоновой работы (тесты).
    pub async fn settle(&self) {
        loop {
            let tasks: Vec<JoinHandle<()>> = std::mem::take(&mut *lock(&self.background));
            if tasks.is_empty() {
                return;
            }
            for task in tasks {
                let _ = task.await;
            }
        }
    }

    // ------------------------------------------------------------ служба и сессия

    pub(crate) fn session(&self) -> Option<Session> {
        self.state().session.clone()
    }

    /// Аккаунт Steam: он есть и тогда, когда службы доставки нет.
    pub(crate) fn steam_account(&self) -> Option<Arc<dyn SteamPort>> {
        self.state().steam.as_ref()?.account.clone()
    }

    /// Служба процессов текущей библиотеки.
    pub fn processes(&self) -> Option<GameProcessService> {
        self.session().map(|session| session.processes)
    }

    /// `ensureService()` фасада доставки; служба есть — и сессия платформы.
    pub(crate) fn ensure_session(&self) -> Option<Session> {
        let delivery = self.delivery()?;
        delivery.ensure_service()?;
        self.session()
    }

    /// `isAnyGameRunning()`.
    pub fn is_any_game_running(&self) -> bool {
        self.session()
            .is_some_and(|session| session.processes.any_running().is_some())
    }

    /// `ensureStatsStore()`.
    pub(crate) fn stats_store(&self) -> Option<Arc<GameStatsStore>> {
        let mut state = self.state();
        if let Some(stats) = &state.stats {
            return Some(stats.clone());
        }
        let dir = self.options.user_data_path.as_ref()?;
        let notifications =
            self.options.notifications.clone().map(|store| {
                store as Arc<dyn crate::game_stats::game_stats_store::StatsNotifications>
            });
        match GameStatsStore::new(
            dir.join(crate::game_stats::game_stats_store::FILE_NAME),
            StatsOptions {
                logger: Some(self.options.logger.clone()),
                notifications,
                runtime: self.options.runtime.clone(),
                ..StatsOptions::default()
            },
        ) {
            Ok(stats) => {
                state.stats = Some(stats.clone());
                Some(stats)
            }
            Err(error) => {
                self.logger().warn(&format!(
                    "[game-stats] статистика не открыта: {}",
                    error.message
                ));
                None
            }
        }
    }

    /// `setGameEnvironmentProvider(provider)`: окружение моста присутствия для игры.
    pub fn set_game_environment_provider(&self, provider: Option<ExtraEnvProvider>) {
        self.state().game_environment = provider;
    }

    /// `this.renderActive = …` (`AppManager`, «Демо»).
    pub fn set_render_active(&self, hook: Option<RenderActive>) {
        self.state().render_active = hook;
    }

    /// `this.beforeLaunch = …` (`AppManager`, «Демо»).
    pub fn set_before_launch(&self, hook: Option<BeforeLaunch>) {
        self.state().before_launch = hook;
    }

    /// `renderActive(gameId)`: нет хука — рендера нет.
    pub(crate) fn render_active(&self, game_id: &str) -> bool {
        let hook = self.state().render_active.clone();
        hook.is_some_and(|hook| hook(game_id))
    }

    pub(crate) fn before_launch_hook(&self) -> Option<BeforeLaunch> {
        self.state().before_launch.clone()
    }

    /// `gameEnvironment()`.
    pub(crate) fn game_environment(&self) -> Option<crate::child_env::Env> {
        let provider = self.state().game_environment.clone()?;
        provider()
    }

    /// Фабрика службы доставки (`ensureService`): служба, Steam, депо, процессы, запуск,
    /// инструменты и подписка на события процессов.
    pub fn service_factory(&self) -> ServiceFactory {
        let me = self.me.clone();
        Box::new(move |request: ServiceRequest| {
            let Some(api) = me.upgrade() else {
                return Err(hlsr_delivery::errors::RawError::new("платформа остановлена").into());
            };
            api.create_session(request)
        })
    }

    fn create_session(&self, request: ServiceRequest) -> Result<Parts, Failure> {
        let factory = self.options.factory.clone();
        let paths = hlsr_delivery::paths::resolve_library(&request.library_path)?;

        let steam = factory.steam();
        if let Some(account) = &steam.account {
            // Подключение — до первого запроса аккаунта из окна; дальше — таймер.
            account.refresh_connection();
        }
        // Как в 0.4.7: Steam и его опрос поднимаются раньше службы доставки; если служба ниже не
        // создастся (библиотека недоступна), аккаунт остаётся виден окну и опрашивается.
        self.state().steam = Some(steam.clone());
        self.start_steam_connection_watch();
        // Прежняя сессия (другая библиотека) больше не нужна.
        if let Some(previous) = self.state().session.take() {
            previous.processes.unsubscribe(previous.listener);
        }
        let processes = factory.processes(&paths.state);
        let tool_runner = factory.tool_runner();
        let parts = factory.delivery(
            request,
            DeliveryPorts {
                processes: processes.clone(),
                steam: steam.clone(),
                tool_runner: Arc::new(tool_runner.clone()),
            },
        )?;
        let me = self.me.clone();
        tool_runner.subscribe(Arc::new(move |tool_id: &str| {
            if let Some(delivery) = me.upgrade().and_then(|api| api.delivery()) {
                delivery.tool_state_changed(tool_id);
            }
        }));

        let me = self.me.clone();
        let launcher = factory.launcher(LaunchContext {
            service: parts.service.clone(),
            processes: processes.clone(),
            delivery: Arc::new(DeliveryLink {
                service: parts.service.clone(),
                processes: processes.clone(),
            }),
            settings_store: self
                .delivery()
                .and_then(|delivery| delivery.settings_store().cloned()),
            steam: steam.clone(),
            overlay: self.options.overlay.clone(),
            extra_env: Arc::new(move || me.upgrade().and_then(|api| api.game_environment())),
        });
        // `service.renderActive` (RENDER_ACTIVE доставки) и `launchService.launching` для отказа
        // `set-library-path`.
        let me = self.me.clone();
        parts
            .service
            .set_render_active(Some(Arc::new(move |game_id: &str| {
                me.upgrade().is_some_and(|api| api.render_active(game_id))
            })));
        if let Some(delivery) = self.delivery() {
            let launching = launcher.clone();
            delivery.set_launching_probe(Arc::new(move || launching.is_launching()));
        }
        let listener = self.attach_process_events(&processes);
        self.state().session = Some(Session {
            service: parts.service.clone(),
            processes,
            listener,
            steam,
            launcher,
        });
        Ok(parts)
    }

    /// `dispose()`: таймер Steam, трей, статистика (службу останавливает фасад доставки).
    pub fn dispose(&self) {
        let (timer, stats) = {
            let mut state = self.state();
            (state.steam_timer.take(), state.stats.clone())
        };
        if let Some(timer) = timer {
            timer.abort();
        }
        if let Some(tray) = &self.tray {
            tray.dispose();
        }
        if let Some(stats) = stats {
            stats.dispose();
        }
    }

    /// Окно получило фокус (`checkOnFocus`): Steam снова опрашивается часто.
    pub fn check_on_focus(&self) {
        self.state().steam_miss_window = 0;
    }

    // ------------------------------------------------------------ каналы без службы

    /// `native-available` (sendSync): `hlsr-native` в 0.5 не нужен — на Windows его замена
    /// (место на диске, WinAPI) всегда есть, на Linux модуль не грузился.
    pub fn native_available(&self) -> bool {
        !self.is_linux()
    }

    /// `umu-available` (sendSync).
    pub fn umu_available(&self) -> bool {
        self.options
            .proton
            .as_ref()
            .is_some_and(|proton| proton.runner.umu_path().is_some())
    }

    /// `protonManager.selected().name` (диагностика, Linux): имя выбранного Proton; `None` — Proton
    /// не выбран или это не Linux.
    pub fn proton_selected_name(&self) -> Option<String> {
        self.options
            .proton
            .as_ref()?
            .manager
            .selected()
            .map(|entry| entry.name)
    }

    /// `tray-hide-while-playing-set`.
    pub fn tray_hide_while_playing_set(&self, enabled: &Value) -> Value {
        if let Some(tray) = &self.tray {
            tray.set_enabled(enabled != &json!(false));
        }
        json!({"status": "ok"})
    }

    /// `resolveGameId(id)` фасада доставки.
    pub(crate) fn resolve_game_id(&self, id: &Value) -> Option<String> {
        self.delivery()?.resolve_game_id(id)
    }

    // ------------------------------------------------------------ каналы по имени

    /// Вызов канала окна по имени; неизвестный канал — `None`. `Err` — отклонённый промис
    /// (`proton-select`, `proton-add`, `proton-download`, `proton-update`).
    pub async fn call(&self, channel: &str, args: &[Value]) -> Option<Result<Value, String>> {
        let a = |index: usize| args.get(index).unwrap_or(&Value::Null);
        let value = match channel {
            "game-process-launch" => self.game_process_launch(a(0), a(1)).await,
            "game-process-stop" => self.game_process_stop(a(0), a(1)).await,
            "game-launch-steam-status" => self.game_launch_steam_status(a(0)),
            "game-launch-open-steam" => self.game_launch_open_steam(),
            "game-launch-install-sdk" => self.game_launch_install_sdk(),
            "game-launch-restart-elevated" => self.game_launch_restart_elevated().await,
            "game-stats-get" => self.game_stats_get(a(0)),
            "game-crash-info" => self.game_crash_info(args.first()),
            "game-crash-open-dump" => self.game_crash_open_dump(args.first()),
            "steam-health-get" => self.steam_health_get(a(0)),
            "steam-account-get" => self.steam_account_state(),
            "steam-open-profile" => self.steam_open_profile(),
            "friends-avatars" => self.friends_avatars(a(0)).await,
            "tray-hide-while-playing-set" => self.tray_hide_while_playing_set(a(0)),
            "livesplit-overlay-capture" => self.capture_live_split(a(0)).await,
            "native-available" => json!(self.native_available()),
            "umu-available" => json!(self.umu_available()),
            _ => return self.proton_call(channel, args).await,
        };
        Some(Ok(value))
    }
}
