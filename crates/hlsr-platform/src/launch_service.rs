//! Запуск игр на Windows (`game-delivery/LaunchService.js` 0.4.9): проверки (метаданные сборки,
//! Steam или Source SDK, лицензии в депо), Source (файлы и `gameinfo.txt`, `steam_appid.txt`, язык,
//! подготовка файла игрока, Vulkan, `spawn` без переменных Steam и окна лаунчера, ожидание
//! старта), эпизоды Ghosting (`-game episodic|ep2`, config.cfg туда и обратно), GoldSrc
//! (`goldsrc-launch.js`: инжектор Bunnymod XT и поиск самого `hl.exe`, DEP, RInput и патч курсора
//! в меню), маска ЦП без нулевого процессора, прокси оверлея LiveSplit до запуска.
//!
//! Внешний мир — порты: процессы ([`LaunchSpawner`], инспектор), Steam ([`LaunchLocator`]),
//! депо (`ports::SteamDepot`), DEP ([`DepCompat`]), маска ЦП ([`CpuAffinity`]), сессия патча
//! RInput. Порты доставки, службы процессов и настроек ([`LaunchDelivery`], [`LaunchProcesses`],
//! [`LaunchSettings`]) общие с Linux-`LaunchService`.
//!
//! В 0.4.7 маску ЦП ставил PowerShell (`ProcessorAffinity`); здесь — `SetProcessAffinityMask`,
//! число процессоров — `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)` (решение 2 спецификации).

use crate::child_env::{self, Env};
use crate::dep_compat::{self, DepState, Exemption, SystemDepRegistry};
use crate::game_process_service::{GameProcessService, StartedProcess};
use crate::goldsrc_launch::{self, is_goldsrc};
use crate::livesplit_overlay::{LiveSplitOverlay, Prepared, Target};
use crate::process_inspect::{ProcessInspector, system_inspector};
use crate::rinput_menu_fix::{MenuSession, PatchStatus};
use crate::spawn::{Child, ChildExit, SpawnOptions, Stdio};
use crate::steam_locator::{LocatorOptions, SteamLocating, SteamLocator, steam_summary};
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform, date_parse};
use hlsr_delivery::build_metadata_trust::{self, State, VerifyAgainstSnapshot};
use hlsr_delivery::cfg_search::{CfgSearch, LanguageOption, SteamLanguage};
use hlsr_delivery::errors::{Code, DeliveryError, Failure, RawError, to_delivery_error};
use hlsr_delivery::game_binds::Encoding;
use hlsr_delivery::game_language::{self, ApplyOptions, EngineOptions, Reason};
use hlsr_delivery::game_settings_store::GameSettingsStore;
use hlsr_delivery::manifests::{LaunchConfig, launch_config_of};
use hlsr_delivery::paths::assert_inside_with;
use hlsr_delivery::player_config::PrepareOptions;
use hlsr_delivery::ports::{Logger, Registry, SteamDepot, SystemRegistry, TracingLogger};
use hlsr_delivery::providers::BoxFuture;
use hlsr_delivery::{
    default_configs, episode_install, episodes, launch_args, node_fs, vulkan_backend,
};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;

// ---------------------------------------------------------------- постоянные 0.4.7

/// `EARLY_EXIT_WINDOW_MS`: сколько ждать выхода игры сразу после запуска.
pub const EARLY_EXIT_WINDOW: Duration = Duration::from_millis(1500);
/// `INJECTED_GAME_TIMEOUT_MS`: инжектор Bunnymod XT запускает `hl.exe`, внедряет DLL и
/// завершается — столько ждём появления игры.
pub const INJECTED_GAME_TIMEOUT: Duration = Duration::from_millis(30_000);
/// `INJECTED_GAME_POLL_MS`.
pub const INJECTED_GAME_POLL: Duration = Duration::from_millis(500);
/// `INJECTOR_EXIT_GRACE_MS`: инжектор завершился, а игры всё нет.
pub const INJECTOR_EXIT_GRACE: Duration = Duration::from_millis(5000);
/// `RINPUT_PATCH_WAIT_MS`: сколько ждать, пока RInput поставит перехваты.
pub const RINPUT_PATCH_WAIT: Duration = Duration::from_millis(30_000);
/// `RINPUT_PATCH_POLL_MS`.
pub const RINPUT_PATCH_POLL: Duration = Duration::from_millis(500);

/// `STRIPPED_ENV`: переменные Steam, которые игре не передаются (без учёта регистра).
pub const STRIPPED_ENV: [&str; 4] = [
    "SteamAppId",
    "SteamGameId",
    "SteamOverlayGameId",
    "SteamAppUser",
];

/// `DEFINITE_STEAM_CODES`: ответ депо, после которого запуск не продолжается.
const DEFINITE_STEAM_CODES: [&str; 5] = [
    "STEAM_APP_NOT_OWNED",
    "STEAM_NOT_RUNNING",
    "STEAM_NOT_INSTALLED",
    "STEAM_PROCESS_MISMATCH",
    "STEAM_ELEVATED",
];

/// `UNCERTAIN_LAUNCH_CODES`: причину уточняет состояние Steam.
const UNCERTAIN_LAUNCH_CODES: [Code; 2] = [Code::LaunchFailed, Code::SteamNotRunning];

/// `normalizeAppIdText(raw)`: без BOM в начале, без NUL (движок дописывает `\0`), без пробелов.
pub fn normalize_app_id_text(raw: &str) -> String {
    let text = raw
        .strip_prefix('\u{feff}')
        .unwrap_or(raw)
        .replace('\0', "");
    js::trim(&text).to_owned()
}

// ---------------------------------------------------------------- порты, общие с Linux

/// Служба процессов игр (`delivery.processService`).
pub trait LaunchProcesses: Send + Sync {
    fn is_running(&self, game_id: &str) -> bool;
    fn get_process_state(&self, game_id: &str) -> Value;
    fn mark_starting(&self, game_id: &str);
    fn cancel_starting(&self, game_id: &str, reason: &str);
    /// `markExited(gameId, pid)`: игра закрылась, а сессия Proton жива.
    fn mark_exited(&self, game_id: &str, pid: u32) -> bool;
}

impl LaunchProcesses for GameProcessService {
    fn is_running(&self, game_id: &str) -> bool {
        GameProcessService::is_running(self, game_id)
    }
    fn get_process_state(&self, game_id: &str) -> Value {
        GameProcessService::get_process_state(self, game_id)
    }
    fn mark_starting(&self, game_id: &str) {
        GameProcessService::mark_starting(self, game_id);
    }
    fn cancel_starting(&self, game_id: &str, reason: &str) {
        GameProcessService::cancel_starting(self, game_id, Some(reason));
    }
    fn mark_exited(&self, game_id: &str, pid: u32) -> bool {
        GameProcessService::mark_exited(self, game_id, pid)
    }
}

/// Доставка (`delivery`): занятость, установленная сборка, регистрация запущенной игры.
pub trait LaunchDelivery: Send + Sync {
    /// `isBusy()`.
    fn is_busy(&self) -> bool;
    /// `installedOf(gameId)`: запись `installed.json` (`installPath`, `buildMetadata`…).
    fn installed_of(&self, game_id: &str) -> Result<Value, DeliveryError>;
    /// `registerStartedProcess(gameId, info)`: папка установки, служба процессов и обновление
    /// статуса игры.
    fn register_started_process(
        &self,
        game_id: &str,
        info: StartedProcess,
    ) -> Result<Value, DeliveryError>;
    /// `filesManifestFor(gameId, version)`: файловый манифест установленной версии (cfg сборки
    /// Ghosting для эпизода). Нет — ошибка, эпизод получает только autoexec и config.
    fn files_manifest_for(
        &self,
        game_id: &str,
        _version: Option<&str>,
    ) -> Result<Value, DeliveryError> {
        Err(DeliveryError::new(
            Code::FilesManifestMissing,
            format!("Файловый манифест {game_id} недоступен"),
        ))
    }
}

/// Настройки игр (`settingsStore`).
pub trait LaunchSettings: Send + Sync {
    /// `get(gameId)`: настройки после проверки.
    fn get(&self, game_id: &str) -> Value;
    /// `save(gameId, settings)`: `true` — записано.
    fn save(&self, game_id: &str, settings: &Value) -> bool;
}

impl LaunchSettings for GameSettingsStore {
    fn get(&self, game_id: &str) -> Value {
        GameSettingsStore::get(self, game_id).to_value()
    }
    fn save(&self, game_id: &str, settings: &Value) -> bool {
        GameSettingsStore::save(self, game_id, settings).is_ok()
    }
}

/// Обновление зависимостей GoldSrc (`dependencyUpdater(installPath, config)`).
pub type DependencyUpdater =
    Arc<dyn Fn(&Path, &'static LaunchConfig) -> BoxFuture<'static, Value> + Send + Sync>;
/// Паки GoldSrc (`packApplier(installPath, config, settings)`).
pub type PackApplier =
    Arc<dyn Fn(&Path, &'static LaunchConfig, &Value) -> BoxFuture<'static, Value> + Send + Sync>;
/// `extraEnvProvider()`: окружение моста presence лаунчера (`HLSR_*`).
pub type ExtraEnvProvider = Arc<dyn Fn() -> Option<Env> + Send + Sync>;

/// `prepareConfigs(gameId, installed, config, settings)` обоих `LaunchService`: файл игрока на
/// запуске не переписывается — только переход с 0.4.8 или создание отсутствующего
/// (`default_configs::prepare_for_launch`). Сбой игру не останавливает.
pub(crate) fn prepare_configs(
    search: &CfgSearch,
    logger: &dyn Logger,
    game_id: &str,
    install_path: &Path,
    config: &LaunchConfig,
    settings: Option<&Value>,
    launcher_version: &str,
) {
    let options = PrepareOptions {
        launcher_version: Some(launcher_version),
        ..PrepareOptions::default()
    };
    if let Err(error) = default_configs::prepare_for_launch(
        search,
        game_id,
        install_path,
        Some(config),
        settings,
        Platform::CURRENT,
        &options,
    ) {
        logger.warn(&format!("[launch] конфиг игрока не подготовлен: {error}"));
    }
}

// ---------------------------------------------------------------- порты Windows

/// Steam (`steamLocator`): что `LaunchService` у него спрашивает.
pub trait LaunchLocator: Send + Sync {
    /// `preflight(appId)`: `{steam: {installPath, steamExe, pid, source}, app}` или ошибка.
    fn preflight(&self, app_id: u32) -> Result<Value, DeliveryError>;
    /// `describeSteamOnly()`: `{ok, code, steam, app}` или `{ok: false, code, message}`.
    fn describe_steam_only(&self) -> Value;
    /// `describeAvailability(appId)`.
    fn describe_availability(&self, app_id: u32) -> Value;
}

impl LaunchLocator for SteamLocator {
    fn preflight(&self, app_id: u32) -> Result<Value, DeliveryError> {
        let result = SteamLocator::preflight(self, Some(app_id))?;
        Ok(json!({
            "steam": steam_summary(&result.steam, &result.running),
            "app": result.app.to_value(),
        }))
    }
    fn describe_steam_only(&self) -> Value {
        SteamLocating::describe_steam_only(self)
    }
    fn describe_availability(&self, app_id: u32) -> Value {
        SteamLocator::describe_availability(self, Some(app_id))
    }
}

/// Параметры `spawn` (`cwd`, `env`, `windowsHide`, `detached`; `stdio: "ignore"`, без оболочки).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpawnRequest {
    pub cwd: PathBuf,
    /// `env`; `None` — окружение лаунчера (без переменных окна, [`crate::child_env`]).
    pub env: Option<Env>,
    /// `windowsHide`: без окна консоли.
    pub windows_hide: bool,
    /// `detached`: своя группа процессов, переживает лаунчер.
    pub detached: bool,
}

/// `spawnFn(file, args, options)`. `Err` — `spawn` бросил исключение сразу (у Node так со всеми
/// ошибками, кроме EACCES, EAGAIN, EMFILE, ENFILE и ENOENT); они приходят событием `error` у
/// [`Child`].
pub trait LaunchSpawner: Send + Sync {
    fn spawn(&self, file: &Path, args: &[String], request: SpawnRequest)
    -> Result<Child, RawError>;
}

/// Настоящие процессы: общий [`spawn::SystemSpawner`](crate::spawn::SystemSpawner) (окружение без
/// переменных окна, флаги создания как у libuv: `detached` — `DETACHED_PROCESS |
/// CREATE_NEW_PROCESS_GROUP`, `windowsHide` — `CREATE_NO_WINDOW`, `stdio: "ignore"`); здесь — только
/// различие в том, как приходит ошибка запуска: `Err` сразу или событием `error` у [`Child`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemSpawner;

/// Ошибки `spawn`, которые Node отдаёт событием `error`, а не исключением.
const ASYNC_SPAWN_ERRORS: [&str; 5] = ["EACCES", "EAGAIN", "EMFILE", "ENFILE", "ENOENT"];

impl LaunchSpawner for SystemSpawner {
    fn spawn(
        &self,
        file: &Path,
        args: &[String],
        request: SpawnRequest,
    ) -> Result<Child, RawError> {
        let child = crate::spawn::Spawner::spawn(
            &crate::spawn::SystemSpawner,
            &file.to_string_lossy(),
            args,
            SpawnOptions {
                env: request.env.unwrap_or_else(child_env::process_env),
                cwd: Some(request.cwd),
                stdio: Stdio::Ignore,
                detached: request.detached,
                windows_hide: Some(request.windows_hide),
            },
        );
        // Процесс не запустился: у него нет PID, а ошибка уже в `exit_state`.
        if child.pid().is_none()
            && let Some(ChildExit::Error(raw)) = child.exit_state()
        {
            let code = raw.code.clone().unwrap_or_default();
            if ASYNC_SPAWN_ERRORS.contains(&code.as_str()) {
                return Ok(child);
            }
            // `new ErrnoException(err, "spawn")`: без пути.
            return Err(RawError {
                message: format!("spawn {code}"),
                path: None,
                ..raw
            });
        }
        Ok(child)
    }
}

/// Маска процессоров игры (`applyAffinity`).
pub trait CpuAffinity: Send + Sync {
    /// Число логических процессоров (`os.cpus().length`).
    fn cpu_count(&self) -> u32;
    /// `(Get-Process -Id <pid>).ProcessorAffinity = <mask>`.
    fn set_affinity(&self, pid: u32, mask: u64) -> Result<(), String>;
}

/// `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)` и `SetProcessAffinityMask`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemAffinity;

#[cfg(windows)]
impl CpuAffinity for SystemAffinity {
    fn cpu_count(&self) -> u32 {
        use windows_sys::Win32::System::Threading::{
            ALL_PROCESSOR_GROUPS, GetActiveProcessorCount,
        };
        // SAFETY: функция без указателей.
        unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) }
    }

    fn set_affinity(&self, pid: u32, mask: u64) -> Result<(), String> {
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_SET_INFORMATION, SetProcessAffinityMask,
        };
        let mask = usize::try_from(mask).map_err(|_| format!("маска {mask} не умещается"))?;
        // SAFETY: открываем процесс по PID; дескриптор закрывается ниже.
        let handle =
            unsafe { OpenProcess(PROCESS_SET_INFORMATION | PROCESS_QUERY_INFORMATION, 0, pid) };
        if handle.is_null() {
            // SAFETY: код ошибки текущего потока.
            return Err(format!("OpenProcess: код {}", unsafe { GetLastError() }));
        }
        // SAFETY: дескриптор открыт с PROCESS_SET_INFORMATION.
        let set = unsafe { SetProcessAffinityMask(handle, mask) };
        // SAFETY: код ошибки до закрытия дескриптора.
        let error = unsafe { GetLastError() };
        // SAFETY: дескриптор получен от OpenProcess и закрывается один раз.
        unsafe { CloseHandle(handle) };
        if set == 0 {
            return Err(format!("SetProcessAffinityMask: код {error}"));
        }
        Ok(())
    }
}

#[cfg(not(windows))]
impl CpuAffinity for SystemAffinity {
    fn cpu_count(&self) -> u32 {
        std::thread::available_parallelism().map_or(1, |count| count.get() as u32)
    }

    fn set_affinity(&self, _pid: u32, _mask: u64) -> Result<(), String> {
        Err("маска процессоров ставится только на Windows".to_owned())
    }
}

/// DEP для `hl.exe` (`depCompat`): слой совместимости и режим системы.
pub trait DepCompat: Send + Sync {
    /// `ensureExemption(exe, {logger})`.
    fn ensure_exemption(&self, executable: &str, logger: &dyn Logger) -> Exemption;
    /// `queryPolicy()`: режим DEP 0..3 или `None`.
    fn query_policy(&self) -> Option<i64>;
}

/// Слой в `HKCU` (`RegSetValueExW`) и `GetSystemDEPPolicy`.
pub struct SystemDepCompat {
    registry: SystemDepRegistry,
}

impl SystemDepCompat {
    pub fn new(registry: Arc<dyn Registry>) -> SystemDepCompat {
        SystemDepCompat {
            registry: SystemDepRegistry::new(registry),
        }
    }
}

impl DepCompat for SystemDepCompat {
    fn ensure_exemption(&self, executable: &str, logger: &dyn Logger) -> Exemption {
        dep_compat::ensure_exemption(&self.registry, executable, logger)
    }
    fn query_policy(&self) -> Option<i64> {
        dep_compat::query_policy()
    }
}

/// Сессия патча курсора RInput (`rinputMenuSession()`).
pub type MenuSessionFactory = Arc<dyn Fn() -> Box<dyn MenuSession> + Send + Sync>;
/// Настройки запуска игр (`launchRegistry[gameId]`).
pub type LaunchRegistry = Arc<dyn Fn(&str) -> Option<&'static LaunchConfig> + Send + Sync>;

/// Сессия патча этой ОС: на Windows — память процесса игры.
fn system_menu_session() -> Box<dyn MenuSession> {
    #[cfg(windows)]
    {
        Box::new(crate::rinput_menu_fix_win::open_session())
    }
    #[cfg(not(windows))]
    {
        struct Unavailable;
        impl MenuSession for Unavailable {
            fn patch(&mut self, _process_name: &str) -> Result<PatchStatus, String> {
                Err("память процесса игры доступна только на Windows".to_owned())
            }
            fn close(&mut self) {}
        }
        Box::new(Unavailable)
    }
}

// ---------------------------------------------------------------- служба

/// Параметры конструктора (`options`).
#[derive(Clone, Default)]
pub struct LaunchServiceOptions {
    pub delivery: Option<Arc<dyn LaunchDelivery>>,
    /// `delivery.processService`.
    pub processes: Option<Arc<dyn LaunchProcesses>>,
    pub logger: Option<Arc<dyn Logger>>,
    /// `env`: окружение, от которого строится окружение игры; по умолчанию — лаунчера.
    pub env: Option<Env>,
    pub inspector: Option<Arc<dyn ProcessInspector>>,
    /// `spawnFn`.
    pub spawner: Option<Arc<dyn LaunchSpawner>>,
    /// `launchRegistry`; по умолчанию — `LAUNCH_CONFIG`.
    pub launch_registry: Option<LaunchRegistry>,
    pub settings_store: Option<Arc<dyn LaunchSettings>>,
    pub logs_dir: Option<PathBuf>,
    pub dependency_updater: Option<DependencyUpdater>,
    pub pack_applier: Option<PackApplier>,
    pub extra_env_provider: Option<ExtraEnvProvider>,
    /// `launcherVersion`; по умолчанию `0.0.0`.
    pub launcher_version: Option<String>,
    pub steam_locator: Option<Arc<dyn LaunchLocator>>,
    pub depot_service: Option<Arc<dyn SteamDepot>>,
    pub early_exit_window: Option<Duration>,
    pub injected_game_timeout: Option<Duration>,
    pub injected_game_poll: Option<Duration>,
    /// Постоянная `INJECTOR_EXIT_GRACE_MS` 0.4.7; меняется только в проверках.
    pub injector_exit_grace: Option<Duration>,
    pub rinput_menu_session: Option<MenuSessionFactory>,
    pub rinput_patch_wait: Option<Duration>,
    pub rinput_patch_poll: Option<Duration>,
    pub live_split_overlay: Option<Arc<LiveSplitOverlay>>,
    pub dep_compat: Option<Arc<dyn DepCompat>>,
    /// `platform`: DEP ставится только на Windows; по умолчанию — эта ОС.
    pub platform: Option<Platform>,
    /// Реестр Windows: язык движка и язык Steam (поиск cfg); по умолчанию — системный.
    pub registry: Option<Arc<dyn Registry>>,
    /// Поиск cfg (бинды, стандартный cfg); по умолчанию — с языком Steam из `registry`.
    pub cfg_search: Option<Arc<CfgSearch>>,
    pub affinity: Option<Arc<dyn CpuAffinity>>,
    /// Часы (`Date.now()`, `new Date().toISOString()`).
    pub now: Option<Arc<dyn Fn() -> SystemTime + Send + Sync>>,
}

/// Итог `launch(gameId)`: `{status: "started" | "already-running", process, dependencies?,
/// packs?}`.
pub type LaunchResult = Value;

/// Запущенная игра (`waitForStart`, `waitForInjectedGame`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedGame {
    pub pid: u32,
    pub executable_path: String,
    pub started_at: String,
}

/// Файлы Source (`resolveGameFiles`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameFiles {
    pub executable: String,
    pub game_directory: String,
    pub gameinfo: String,
}

struct Inner {
    delivery: Option<Arc<dyn LaunchDelivery>>,
    processes: Option<Arc<dyn LaunchProcesses>>,
    logger: Arc<dyn Logger>,
    env: Option<Env>,
    inspector: Arc<dyn ProcessInspector>,
    spawner: Arc<dyn LaunchSpawner>,
    registry_of: LaunchRegistry,
    settings_store: Option<Arc<dyn LaunchSettings>>,
    logs_dir: Option<PathBuf>,
    dependency_updater: Option<DependencyUpdater>,
    pack_applier: Option<PackApplier>,
    extra_env_provider: Option<ExtraEnvProvider>,
    launcher_version: String,
    locator: Arc<dyn LaunchLocator>,
    depot: Option<Arc<dyn SteamDepot>>,
    early_exit_window: Duration,
    injected_game_timeout: Duration,
    injected_game_poll: Duration,
    injector_exit_grace: Duration,
    rinput_menu_session: MenuSessionFactory,
    rinput_patch_wait: Duration,
    rinput_patch_poll: Duration,
    live_split_overlay: Option<Arc<LiveSplitOverlay>>,
    dep: Arc<dyn DepCompat>,
    platform: Platform,
    registry: Arc<dyn Registry>,
    cfg_search: Arc<CfgSearch>,
    affinity: Arc<dyn CpuAffinity>,
    now: Arc<dyn Fn() -> SystemTime + Send + Sync>,
    path: NodePath,
    launching: Mutex<HashSet<String>>,
    dep_state: Mutex<Option<Arc<Mutex<DepState>>>>,
    rinput_patch: Mutex<Option<JoinHandle<()>>>,
    background: Mutex<Vec<JoinHandle<()>>>,
}

/// `LaunchService` (Windows). Копии — та же служба.
#[derive(Clone)]
pub struct LaunchService {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for LaunchService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchService").finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `error.message`.
fn failure_message(failure: &Failure) -> String {
    match failure {
        Failure::Delivery(error) => error.message.clone(),
        Failure::Raw(error) => error.message.clone(),
    }
}

/// `${code == null ? "?" : code}`.
fn code_text(code: Option<i32>) -> String {
    code.map_or_else(|| "?".to_owned(), |code| code.to_string())
}

/// `prepareOverlay` обоих `LaunchService`: прокси оверлея до запуска и строка журнала о том, куда он
/// встал. Сбой оверлея игру не останавливает.
pub(crate) fn prepare_overlay_logged(
    overlay: &LiveSplitOverlay,
    logger: &dyn Logger,
    game_id: &str,
    target: &Target,
    settings: Option<&Value>,
    vulkan: bool,
) -> Option<Prepared> {
    match overlay.prepare(
        game_id,
        settings.unwrap_or(&Value::Null),
        Some(target),
        vulkan,
    ) {
        Ok(Some(prepared)) => {
            if prepared.placed.is_empty() {
                logger.warn(&format!(
                    "[launch] оверлей LiveSplit: {} в папку игры не положен (чужой файл или нет папки)",
                    prepared.proxy
                ));
            } else {
                let placed: Vec<String> = prepared
                    .placed
                    .iter()
                    .map(|folder| folder.to_string_lossy().into_owned())
                    .collect();
                logger.info(&format!(
                    "[launch] оверлей LiveSplit: {} в {}, журнал: {}",
                    prepared.proxy,
                    placed.join(", "),
                    prepared.log.to_string_lossy()
                ));
            }
            Some(prepared)
        }
        Ok(None) => None,
        Err(error) => {
            logger.warn(&format!(
                "[launch] оверлей LiveSplit не подготовлен: {}",
                error.message
            ));
            None
        }
    }
}

/// `${code}` в шаблоне JS: `null` — `null`.
fn js_code(code: Option<i32>) -> String {
    code.map_or_else(|| "null".to_owned(), |code| code.to_string())
}

/// Запуск фоновой задачи в текущей среде tokio. Среды нет — строка в журнал и `None`, а не
/// молчание: слежение за процессом без неё не заработает.
fn spawn_background<F>(logger: &dyn Logger, what: &str, future: F) -> Option<JoinHandle<()>>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        return Some(runtime.spawn(future));
    }
    logger.warn(&format!(
        "[launch] {what}: фоновая задача не запущена — нет среды tokio"
    ));
    None
}

/// `/^HLSR_[A-Z0-9_]+$/`.
fn is_hlsr_env_name(name: &str) -> bool {
    name.strip_prefix("HLSR_").is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
    })
}

/// Строки текста по разделителям строк JS (`\n`, `\r`, U+2028, U+2029) — для флага `m`.
fn js_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split(['\n', '\r', '\u{2028}', '\u{2029}'])
}

/// `/^[ \t]*SteamAppId[ \t]+(\d+)[ \t]*(?:\/\/.*)?$/im` на одной строке: число после `SteamAppId`.
fn steam_app_id_line(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches([' ', '\t']);
    const KEY: &str = "steamappid";
    if rest.len() < KEY.len() || !rest.as_bytes()[..KEY.len()].eq_ignore_ascii_case(KEY.as_bytes())
    {
        return None;
    }
    let rest = &rest[KEY.len()..];
    let after_space = rest.trim_start_matches([' ', '\t']);
    if after_space.len() == rest.len() {
        return None;
    }
    let digits_end = after_space
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after_space.len());
    if digits_end == 0 {
        return None;
    }
    let tail = after_space[digits_end..].trim_start_matches([' ', '\t']);
    (tail.is_empty() || tail.starts_with("//")).then(|| &after_space[..digits_end])
}

/// Число из десятичных цифр (`Number(text)`) как значение JSON.
fn number_value(digits: &str) -> Value {
    match digits.parse::<u64>() {
        Ok(number) if number <= (1 << 53) => json!(number),
        _ => json!(js::to_number(digits)),
    }
}

impl LaunchService {
    pub fn new(options: LaunchServiceOptions) -> LaunchService {
        let logger = options.logger.unwrap_or_else(|| Arc::new(TracingLogger));
        let registry: Arc<dyn Registry> =
            options.registry.unwrap_or_else(|| Arc::new(SystemRegistry));
        let platform = options.platform.unwrap_or(Platform::CURRENT);
        let cfg_search = options
            .cfg_search
            .unwrap_or_else(|| Arc::new(CfgSearch::new(SteamLanguage::new(registry.clone()))));
        let locator = options.steam_locator.unwrap_or_else(|| {
            Arc::new(SteamLocator::new(LocatorOptions {
                registry: Some(registry.clone()),
                logger: Some(logger.clone()),
                ..LocatorOptions::default()
            }))
        });
        let dep = options
            .dep_compat
            .unwrap_or_else(|| Arc::new(SystemDepCompat::new(registry.clone())));
        LaunchService {
            inner: Arc::new(Inner {
                delivery: options.delivery,
                processes: options.processes,
                env: options.env,
                inspector: options.inspector.unwrap_or_else(system_inspector),
                spawner: options.spawner.unwrap_or_else(|| Arc::new(SystemSpawner)),
                registry_of: options
                    .launch_registry
                    .unwrap_or_else(|| Arc::new(launch_config_of)),
                settings_store: options.settings_store,
                logs_dir: options.logs_dir,
                dependency_updater: options.dependency_updater,
                pack_applier: options.pack_applier,
                extra_env_provider: options.extra_env_provider,
                launcher_version: options
                    .launcher_version
                    .unwrap_or_else(|| "0.0.0".to_owned()),
                locator,
                depot: options.depot_service,
                early_exit_window: options.early_exit_window.unwrap_or(EARLY_EXIT_WINDOW),
                injected_game_timeout: options
                    .injected_game_timeout
                    .unwrap_or(INJECTED_GAME_TIMEOUT),
                injected_game_poll: options.injected_game_poll.unwrap_or(INJECTED_GAME_POLL),
                injector_exit_grace: options.injector_exit_grace.unwrap_or(INJECTOR_EXIT_GRACE),
                rinput_menu_session: options
                    .rinput_menu_session
                    .unwrap_or_else(|| Arc::new(system_menu_session)),
                rinput_patch_wait: options.rinput_patch_wait.unwrap_or(RINPUT_PATCH_WAIT),
                rinput_patch_poll: options.rinput_patch_poll.unwrap_or(RINPUT_PATCH_POLL),
                live_split_overlay: options.live_split_overlay,
                dep,
                platform,
                registry,
                cfg_search,
                affinity: options.affinity.unwrap_or_else(|| Arc::new(SystemAffinity)),
                now: options.now.unwrap_or_else(|| Arc::new(SystemTime::now)),
                logger,
                path: NodePath::native(),
                launching: Mutex::new(HashSet::new()),
                dep_state: Mutex::new(None),
                rinput_patch: Mutex::new(None),
                background: Mutex::new(Vec::new()),
            }),
        }
    }

    fn logger(&self) -> &dyn Logger {
        self.inner.logger.as_ref()
    }

    fn now_ms(&self) -> f64 {
        (self.inner.now)()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |elapsed| elapsed.as_millis() as f64)
    }

    fn now_iso(&self) -> String {
        js::iso_string((self.inner.now)())
    }

    fn processes(&self) -> Result<Arc<dyn LaunchProcesses>, DeliveryError> {
        self.inner
            .processes
            .clone()
            .ok_or_else(|| DeliveryError::new(Code::LaunchFailed, "Служба процессов не настроена"))
    }

    fn delivery(&self) -> Result<&Arc<dyn LaunchDelivery>, DeliveryError> {
        self.inner
            .delivery
            .as_ref()
            .ok_or_else(|| DeliveryError::new(Code::LaunchFailed, "Доставка не настроена"))
    }

    /// `settingsStore.get(gameId)`; хранилища нет — `None`.
    fn settings_of(&self, game_id: &str) -> Option<Value> {
        self.inner
            .settings_store
            .as_ref()
            .map(|store| store.get(game_id))
    }

    fn keep(&self, handle: Option<JoinHandle<()>>) {
        if let Some(handle) = handle {
            let mut background = lock(&self.inner.background);
            background.retain(|task| !task.is_finished());
            background.push(handle);
        }
    }

    /// Дождаться фоновых дел последнего запуска: событий процессов оверлея и RInput, патча
    /// курсора, режима DEP. Ничего не останавливает.
    pub async fn settled(&self) {
        self.rinput_patched().await;
        loop {
            let tasks: Vec<JoinHandle<()>> = lock(&self.inner.background).drain(..).collect();
            if tasks.is_empty() {
                return;
            }
            for task in tasks {
                let _ = task.await;
            }
        }
    }

    /// Патч курсора RInput последнего запуска (`this.rinputPatch`): дождаться.
    pub async fn rinput_patched(&self) {
        let handle = lock(&self.inner.rinput_patch).take();
        if let Some(handle) = handle {
            let _ = handle.await;
        }
    }

    /// `configFor(gameId)`: доверенные настройки запуска игры.
    pub fn config_for(&self, game_id: &str) -> Result<&'static LaunchConfig, DeliveryError> {
        let Some(config) = (self.inner.registry_of)(game_id) else {
            return Err(DeliveryError::new(
                Code::LaunchConfigInvalid,
                format!("Для игры {game_id} нет доверенной конфигурации запуска"),
            )
            .with_details(json!({"gameId": game_id})));
        };
        if config.steam_app_id == 0 {
            return Err(DeliveryError::new(
                Code::LaunchConfigInvalid,
                "Некорректный steamAppId в конфигурации",
            ));
        }
        if config.executable.is_empty() || self.inner.path.is_absolute(config.executable) {
            return Err(DeliveryError::new(
                Code::LaunchConfigInvalid,
                "Исполняемый файл задаётся относительным путём внутри сборки",
            ));
        }
        if config.game_directory.is_empty() {
            return Err(DeliveryError::new(
                Code::LaunchConfigInvalid,
                "Не задан каталог мода",
            ));
        }
        Ok(config)
    }

    /// `path.join(installPath, relative.replace(/\//g, path.sep))` внутри папки установки.
    fn inside(
        &self,
        install_path: &str,
        relative: &str,
        what: &str,
    ) -> Result<String, DeliveryError> {
        let path = &self.inner.path;
        let joined = path.join(&[install_path, &relative.replace('/', path.sep())]);
        assert_inside_with(path, install_path, &joined, Some(what))
    }

    /// `resolveGameFiles(installPath, config)`: exe, папка мода и `gameinfo.txt` на месте.
    pub fn resolve_game_files(
        &self,
        install_path: &str,
        config: &LaunchConfig,
    ) -> Result<GameFiles, DeliveryError> {
        let executable = self.inside(install_path, config.executable, "Исполняемый файл игры")?;
        let game_directory = self.inside(install_path, config.game_directory, "Каталог мода")?;
        if !Path::new(&executable).exists() {
            return Err(DeliveryError::new(
                Code::GameExecutableMissing,
                format!("В установленной сборке нет {}", config.executable),
            )
            .with_details(json!({"executable": executable})));
        }
        if !std::fs::metadata(&game_directory).is_ok_and(|meta| meta.is_dir()) {
            return Err(DeliveryError::new(
                Code::GameDirectoryMissing,
                format!(
                    "В установленной сборке нет каталога {}",
                    config.game_directory
                ),
            )
            .with_details(json!({"gameDirectory": game_directory})));
        }
        let gameinfo = self.inner.path.join(&[&game_directory, "gameinfo.txt"]);
        if !Path::new(&gameinfo).exists() {
            return Err(DeliveryError::new(
                Code::GameDirectoryMissing,
                "В каталоге мода нет gameinfo.txt",
            )
            .with_details(json!({"gameinfo": gameinfo})));
        }
        Ok(GameFiles {
            executable,
            game_directory,
            gameinfo,
        })
    }

    fn app_id_file(&self, install_path: &str) -> Result<String, DeliveryError> {
        let joined = self.inner.path.join(&[install_path, "steam_appid.txt"]);
        assert_inside_with(
            &self.inner.path,
            install_path,
            &joined,
            Some("steam_appid.txt"),
        )
    }

    /// `enforceSteamAppIdFile(installPath, config)`: файл приводится к AppID игры (движок
    /// переписывает его значением из `gameinfo.txt` мода, с которым запущен).
    pub fn enforce_steam_app_id_file(
        &self,
        install_path: &str,
        config: &LaunchConfig,
    ) -> Result<(), DeliveryError> {
        let file = self.app_id_file(install_path)?;
        let expected = config
            .steam_app_id_file_text
            .map_or_else(|| format!("{}\n", config.steam_app_id), str::to_owned);
        let current = node_fs::read_text(Path::new(&file), Encoding::Utf8).ok();
        if current.as_deref().is_some_and(|current| {
            normalize_app_id_text(current) == config.steam_app_id.to_string()
        }) {
            return Ok(());
        }
        if let Err(error) = node_fs::write_bytes(Path::new(&file), expected.as_bytes()) {
            return Err(DeliveryError::new(
                Code::SteamAppIdMismatch,
                format!("Не удалось записать steam_appid.txt: {}", error.message),
            )
            .with_details(json!({"file": file})));
        }
        let was = current
            .map(|current| format!(" (было \"{}\")", normalize_app_id_text(&current)))
            .unwrap_or_default();
        self.logger().info(&format!(
            "[launch] steam_appid.txt приведён к {}{was}",
            config.steam_app_id
        ));
        Ok(())
    }

    /// `assertSteamAppIdFile(installPath, config)`: файл сборки объявляет AppID игры.
    pub fn assert_steam_app_id_file(
        &self,
        install_path: &str,
        config: &LaunchConfig,
    ) -> Result<String, DeliveryError> {
        let file = self.app_id_file(install_path)?;
        let Ok(raw) = node_fs::read_text(Path::new(&file), Encoding::Utf8) else {
            return Err(DeliveryError::new(
                Code::SteamAppIdMismatch,
                "В установленной сборке нет steam_appid.txt",
            )
            .with_details(json!({"file": file})));
        };
        let value = normalize_app_id_text(&raw);
        let expected = config.steam_app_id.to_string();
        if value != expected {
            return Err(DeliveryError::new(
                Code::SteamAppIdMismatch,
                format!("steam_appid.txt объявляет \"{value}\", ожидался {expected}"),
            )
            .with_details(json!({"file": file, "expected": expected, "actual": value})));
        }
        Ok(file)
    }

    /// `assertGameInfoAppId(gameinfo, config)`: `SteamAppId` в `gameinfo.txt` мода.
    pub fn assert_game_info_app_id(
        &self,
        gameinfo: &str,
        config: &LaunchConfig,
    ) -> Result<(), DeliveryError> {
        let text = node_fs::read_text(Path::new(gameinfo), Encoding::Utf8).map_err(|error| {
            DeliveryError::new(
                Code::GameDirectoryMissing,
                format!("gameinfo.txt не читается: {}", error.message),
            )
        })?;
        let Some(digits) = js_lines(&text).find_map(steam_app_id_line) else {
            return Err(DeliveryError::new(
                Code::SteamAppIdMismatch,
                "В gameinfo.txt не найден SteamAppId",
            )
            .with_details(json!({"gameinfo": gameinfo})));
        };
        if js::to_number(digits) != f64::from(config.steam_app_id) {
            return Err(DeliveryError::new(
                Code::SteamAppIdMismatch,
                format!(
                    "gameinfo.txt использует SteamAppId {digits}, ожидался {}",
                    config.steam_app_id
                ),
            )
            .with_details(json!({
                "gameinfo": gameinfo,
                "expected": config.steam_app_id,
                "actual": number_value(digits),
            })));
        }
        Ok(())
    }

    /// `buildEnvironment()`: окружение лаунчера без переменных Steam (и без переменных окна
    /// лаунчера) плюс `HLSR_*` моста presence.
    pub fn build_environment(&self) -> Env {
        let mut env = match &self.inner.env {
            Some(env) => child_env::without_ui_vars(env.clone()),
            None => child_env::process_env(),
        };
        env.retain(|key, _| {
            let lower = key.to_lowercase();
            !STRIPPED_ENV.iter().any(|name| name.to_lowercase() == lower)
        });
        if let Some(extra) = self
            .inner
            .extra_env_provider
            .as_ref()
            .and_then(|provider| provider())
        {
            for (key, value) in extra {
                if is_hlsr_env_name(&key) && !value.is_empty() {
                    env.insert(key, value);
                }
            }
        }
        env
    }

    /// `assertBuildMetadata(installed)`: сборка с метаданными должна им соответствовать.
    pub fn assert_build_metadata(&self, installed: &Value) -> Result<(), DeliveryError> {
        if !installed.get("buildMetadata").is_some_and(js::truthy) {
            return Ok(());
        }
        let install = installed
            .get("installPath")
            .map(js::to_string)
            .unwrap_or_default();
        let verdict = build_metadata_trust::verify_against_snapshot(&VerifyAgainstSnapshot {
            install_root: Path::new(&install),
            snapshot: Some(installed),
            launcher_version: Some(&self.inner.launcher_version),
            expected: None,
            now: None,
        });
        if matches!(verdict.state, State::Trusted | State::Legacy) {
            return Ok(());
        }
        let code = verdict
            .reason
            .map_or("BUILD_METADATA_MISMATCH", |reason| reason.as_str());
        Err(DeliveryError::new(
            Code::coerce(code),
            format!("Сборка не подтверждена метаданными: {code}"),
        )
        .with_details(verdict.detail.unwrap_or(Value::Null)))
    }

    /// `launch(gameId)`.
    pub async fn launch(&self, game_id: &str) -> Result<LaunchResult, DeliveryError> {
        self.launch_episode(game_id, None).await
    }

    /// `launching.size > 0`: идёт запуск какой-то игры (отказ `set-library-path`).
    pub fn is_launching(&self) -> bool {
        !lock(&self.inner.launching).is_empty()
    }

    /// `launch(gameId, {episode})`: `episode` (`ep1` | `ep2`) — эпизод Ghosting, тот же `hl2.exe`
    /// с `-game episodic|ep2`.
    pub async fn launch_episode(
        &self,
        game_id: &str,
        episode: Option<&str>,
    ) -> Result<LaunchResult, DeliveryError> {
        let base = self.config_for(game_id)?;
        if let Some(episode) = episode
            && (game_id != episode_install::GAME_ID || !episodes::is_episode(episode))
        {
            return Err(DeliveryError::new(
                Code::LaunchConfigInvalid,
                format!("Эпизод {episode} не запускается для {game_id}"),
            )
            .with_details(json!({"gameId": game_id, "episode": episode})));
        }
        // Каталог мода, gameinfo.txt и -game — эпизода; конфиг игрока и язык — Ghosting (base).
        let config = match episode {
            Some(episode) => episodes::launch_config(base, episode)?,
            None => base.clone(),
        };
        if lock(&self.inner.launching).contains(game_id) {
            return Err(
                DeliveryError::new(Code::AlreadyRunning, "Запуск уже выполняется")
                    .with_details(json!({"gameId": game_id})),
            );
        }
        let processes = self.processes()?;
        if processes.is_running(game_id) {
            return Ok(json!({
                "status": "already-running",
                "process": processes.get_process_state(game_id),
            }));
        }
        let delivery = self.delivery()?.clone();
        if delivery.is_busy() {
            return Err(DeliveryError::new(
                Code::AlreadyRunning,
                "Дождитесь завершения текущей операции",
            )
            .with_details(json!({"gameId": game_id})));
        }
        let installed = delivery.installed_of(game_id)?;
        let install_path = installed
            .get("installPath")
            .map(js::to_string)
            .unwrap_or_else(|| "undefined".to_owned());

        if let Some(episode) = episode
            && !episodes::status(Some(Path::new(&install_path)))[episode]["installed"]
                .as_bool()
                .unwrap_or(false)
        {
            return Err(DeliveryError::new(
                Code::NotInstalled,
                format!("Эпизод {episode} не установлен"),
            )
            .with_details(json!({"gameId": game_id, "episode": episode})));
        }

        lock(&self.inner.launching).insert(game_id.to_owned());
        processes.mark_starting(game_id);
        let result = self
            .launch_started(game_id, base, &config, episode, &installed)
            .await;
        let result = result.map_err(|failure| {
            let error =
                self.explain_with_steam_state(to_delivery_error(failure, Some(Code::LaunchFailed)));
            processes.cancel_starting(
                game_id,
                &format!("{}: {}", error.code.as_str(), error.message),
            );
            self.logger().error(&format!(
                "[launch] запуск {game_id} не удался: {} {}",
                error.code.as_str(),
                error.message
            ));
            error
        });
        lock(&self.inner.launching).remove(game_id);
        result
    }

    async fn launch_started(
        &self,
        game_id: &str,
        base: &'static LaunchConfig,
        config: &LaunchConfig,
        episode: Option<&str>,
        installed: &Value,
    ) -> Result<LaunchResult, Failure> {
        let install_path = installed
            .get("installPath")
            .map(js::to_string)
            .unwrap_or_else(|| "undefined".to_owned());
        // config.cfg эпизода, не вернувшийся после краха игры или лаунчера, — в Ghosting до её запуска.
        if game_id == episode_install::GAME_ID {
            episodes::return_pending(Path::new(&install_path), self.logger(), None);
        }

        self.assert_build_metadata(installed)?;
        // Все сборки идут с -steam, а engine.dll без клиента Steam не стартует: отказываем
        // заранее. sourceSdkRequired: false снимает только проверку Source SDK Base.
        let preflight = if config.source_sdk_required {
            self.inner.locator.preflight(config.steam_app_id)?
        } else {
            self.assert_steam_running()?
        };
        self.assert_owned_apps(game_id, config).await?;

        if is_goldsrc(base) {
            return self.launch_goldsrc(game_id, &install_path, base).await;
        }

        let files = self.resolve_game_files(&install_path, config)?;
        if config.owns_steam_app_id_file {
            self.enforce_steam_app_id_file(&install_path, config)?;
        } else {
            self.assert_steam_app_id_file(&install_path, config)?;
        }
        self.assert_game_info_app_id(&files.gameinfo, config)?;

        let settings = self.settings_of(game_id);
        let trusted: Vec<String> = config
            .arguments
            .iter()
            .map(|arg| (*arg).to_owned())
            .collect();
        let argv = launch_args::merge_launch_arguments(
            &trusted,
            settings.as_ref().unwrap_or(&Value::Null),
        );

        let install = Path::new(&install_path);
        // Языковая подмена — в ghostingmod/custom: эпизоды монтируют ../ghostingmod/custom/*.
        self.apply_language(game_id, install, base, settings.as_ref());

        // Файл игрока один — в ghostingmod/cfg; в эпизод его копирует sync_config_in ниже.
        self.prepare_configs(game_id, install, base, settings.as_ref());

        let dxvk = self.prepare_vulkan(&install_path, config, settings.as_ref());
        let prepared =
            self.prepare_overlay(game_id, &install_path, config, settings.as_ref(), dxvk);
        // DXVK грузит только прокси d3d9.dll: не встал — игра на системной d3d9.dll.
        let vulkan = dxvk && prepared.is_some_and(|prepared| !prepared.placed.is_empty());
        if dxvk && !vulkan {
            self.logger()
                .warn("[launch] Vulkan: прокси d3d9.dll в папке игры нет: запуск на DirectX 9");
        }

        if let Some(episode) = episode {
            let stock = self.ghosting_stock_of(game_id, installed);
            episodes::sync_config_in(install, episode, stock.as_deref())?;
        }

        let mut env = self.build_environment();
        vulkan_backend::apply_environment(&mut env, &install_path, vulkan);
        let child = self.inner.spawner.spawn(
            Path::new(&files.executable),
            &argv,
            SpawnRequest {
                cwd: PathBuf::from(&install_path),
                env: Some(env),
                windows_hide: false,
                detached: true,
            },
        )?;
        let started = self.wait_for_start(&child, &files.executable).await?;

        self.apply_affinity(game_id, started.pid);

        let state = self.delivery()?.register_started_process(
            game_id,
            StartedProcess {
                pid: Some(i64::from(started.pid)),
                executable_path: Some(started.executable_path.clone()),
                started_at: Some(started.started_at.clone()),
                child: child.handle(),
                episode: episode.map(str::to_owned),
                ..StartedProcess::default()
            },
        )?;

        self.logger().info(&format!(
            "[launch] {game_id}{} запущена: PID {}, {}, аргументы [{}], AppID {}, Steam PID {}",
            episode
                .map(|episode| format!(" (эпизод {episode})"))
                .unwrap_or_default(),
            started.pid,
            files.executable,
            argv.join(" "),
            config.steam_app_id,
            js::to_template_string(preflight.get("steam").and_then(|steam| steam.get("pid"))),
        ));

        Ok(json!({"status": "started", "process": state}))
    }

    /// `launchGoldSrc(gameId, installed, config)`: HL1 WON — `hl.exe` стартует через инжектор
    /// Bunnymod XT; отслеживается сам `hl.exe`.
    async fn launch_goldsrc(
        &self,
        game_id: &str,
        install_path: &str,
        config: &'static LaunchConfig,
    ) -> Result<LaunchResult, Failure> {
        let install = Path::new(install_path);
        let dependencies = match &self.inner.dependency_updater {
            Some(update) => update(install, config).await,
            None => json!([]),
        };
        let files = goldsrc_launch::resolve_files(install, config)?;
        goldsrc_launch::enforce_steam_app_id_file(install, config, self.logger())?;

        let settings = self.settings_of(game_id);
        self.prepare_configs(game_id, install, config, settings.as_ref());
        let packs = match &self.inner.pack_applier {
            Some(apply) => apply(install, config, settings.as_ref().unwrap_or(&Value::Null)).await,
            None => Value::Null,
        };
        let argv =
            goldsrc_launch::merge_arguments(config, settings.as_ref().unwrap_or(&Value::Null));
        let mut env = self.build_environment();
        env.extend(goldsrc_launch::environment_of(config));
        if let Some(bxt_log) =
            goldsrc_launch::prepare_bxt_log(self.inner.logs_dir.as_deref(), self.logger())
        {
            env.insert(
                "BXT_LOGFILE".to_owned(),
                bxt_log.to_string_lossy().into_owned(),
            );
        }
        let executable = files.executable.to_string_lossy().into_owned();
        self.prepare_dep(&executable);
        self.prepare_overlay(game_id, install_path, config, settings.as_ref(), false);
        let spawned_at = self.now_ms();

        let mut args = vec![executable.clone()];
        args.extend(argv.iter().cloned());
        let child = self.inner.spawner.spawn(
            &files.injector,
            &args,
            SpawnRequest {
                cwd: PathBuf::from(self.inner.path.dirname(&executable)),
                env: Some(env),
                windows_hide: false,
                detached: true,
            },
        )?;
        let started = self
            .wait_for_injected_game(&child, &executable, spawned_at)
            .await?;
        // `await` 0.4.7: накопившиеся фоновые дела (режим DEP) идут раньше продолжения.
        tokio::task::yield_now().await;

        self.apply_affinity(game_id, started.pid);

        let state = self.delivery()?.register_started_process(
            game_id,
            StartedProcess {
                pid: Some(i64::from(started.pid)),
                executable_path: Some(started.executable_path.clone()),
                started_at: Some(started.started_at.clone()),
                ..StartedProcess::default()
            },
        )?;

        self.start_rinput(install_path, config, settings.as_ref(), Some(started.pid))?;

        self.logger().info(&format!(
            "[launch] {game_id} запущена через Bunnymod XT: PID {}, {executable}, аргументы [{}], AppID {}",
            started.pid,
            argv.join(" "),
            config.steam_app_id
        ));

        Ok(json!({
            "status": "started",
            "process": state,
            "dependencies": dependencies,
            "packs": packs,
        }))
    }

    /// `prepareDep(executable)`: `hl.exe` из WON падает при DEP в режиме OptOut — перед запуском
    /// DEP снимается с него слоем совместимости, а режим системы узнаётся в фоне, чтобы при
    /// падении подсказать, что делать. Не Windows — `None`.
    pub fn prepare_dep(&self, executable: &str) -> Option<DepState> {
        if self.inner.platform != Platform::Win32 {
            return None;
        }
        let exemption = self.inner.dep.ensure_exemption(executable, self.logger());
        let policy = lock(&self.inner.dep_state)
            .as_ref()
            .and_then(|state| lock(state).policy);
        let state = Arc::new(Mutex::new(DepState { exemption, policy }));
        *lock(&self.inner.dep_state) = Some(state.clone());
        if policy.is_none() {
            let queried = self.inner.dep.query_policy();
            let logger = self.inner.logger.clone();
            let shared = state.clone();
            let record = move || {
                lock(&shared).policy = queried;
                let name = queried.map_or("не определён", |policy| {
                    dep_compat::policy_name(policy).unwrap_or("undefined")
                });
                logger.info(&format!(
                    "[dep] режим DEP: {name}, слой для hl.exe: {}",
                    exemption.as_str()
                ));
            };
            match tokio::runtime::Handle::try_current() {
                Ok(runtime) => self.keep(Some(runtime.spawn(async move { record() }))),
                Err(_) => record(),
            }
        }
        let current = *lock(&state);
        Some(current)
    }

    /// `depCrashHint(exitCode)`: `DEP_ALWAYS_ON`, `DEP_EXEMPTION_FAILED` или `None`.
    pub fn dep_crash_hint(&self, exit_code: Option<i64>) -> Option<&'static str> {
        let state = lock(&self.inner.dep_state)
            .as_ref()
            .map(|state| *lock(state));
        dep_compat::crash_hint(state.as_ref(), exit_code)
    }

    /// `prepareConfigs(gameId, installed, config, settings)`.
    fn prepare_configs(
        &self,
        game_id: &str,
        install: &Path,
        config: &LaunchConfig,
        settings: Option<&Value>,
    ) {
        prepare_configs(
            &self.inner.cfg_search,
            self.logger(),
            game_id,
            install,
            config,
            settings,
            &self.inner.launcher_version,
        );
    }

    /// `ghostingStockOf(gameId, installed)`: имена cfg сборки Ghosting (`ghostingmod/cfg` из
    /// files.json установленной версии) — их `sync_config_in` в эпизод не берёт. `None` — манифест
    /// недоступен, идут только autoexec и config.
    fn ghosting_stock_of(&self, game_id: &str, installed: &Value) -> Option<Vec<String>> {
        let version = installed.get("version").map(js::to_string);
        let manifest = self
            .delivery()
            .and_then(|delivery| delivery.files_manifest_for(game_id, version.as_deref()));
        match manifest {
            Ok(manifest) => Some(
                manifest
                    .get("files")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|file| {
                        let path = js::to_string(file.get("path").unwrap_or(&Value::Null));
                        let name = path.to_lowercase();
                        let name = name.strip_prefix("ghostingmod/cfg/")?;
                        (!name.is_empty() && !name.contains('/')).then(|| name.to_owned())
                    })
                    .collect(),
            ),
            Err(error) => {
                self.logger().warn(&format!(
                    "[launch] список cfg сборки Ghosting недоступен: {}",
                    error.message
                ));
                None
            }
        }
    }

    /// `prepareVulkan(installed, config, settings)`: Vulkan (`vulkan_backend`) — включён в
    /// настройках и DXVK есть в папке игры. Нет DXVK — игра идёт на DirectX 9, как без
    /// переключателя.
    pub fn prepare_vulkan(
        &self,
        install_path: &str,
        config: &LaunchConfig,
        settings: Option<&Value>,
    ) -> bool {
        if settings.and_then(|settings| settings.get("vulkan")) != Some(&Value::Bool(true)) {
            return false;
        }
        let install = Path::new(install_path);
        match vulkan_backend::availability(Some(install), Some(config), self.inner.platform) {
            None => return false,
            Some(false) => {
                self.logger().warn(&format!(
                    "[launch] Vulkan включён, но в папке игры нет {}: запуск на DirectX 9",
                    vulkan_backend::BACKEND
                ));
                return false;
            }
            Some(true) => {}
        }
        for dir in [vulkan_backend::LOG_DIR, vulkan_backend::CACHE_DIR] {
            if let Err(error) = std::fs::create_dir_all(install.join(dir)) {
                self.logger().warn(&format!(
                    "[launch] Vulkan: папка {dir} не создана: {}",
                    RawError::fs(&error, "mkdir", &install.join(dir)).message
                ));
            }
        }
        self.logger().info(&format!(
            "[launch] Vulkan (DXVK): журнал в {}",
            self.inner
                .path
                .join(&[install_path, vulkan_backend::LOG_DIR])
        ));
        true
    }

    /// `prepareOverlay(gameId, installed, config, settings, vulkan)`: оверлей LiveSplit — прокси
    /// `d3d9.dll` или `opengl32.dll` кладётся в папку игры до запуска, игра грузит его сама. Ничего
    /// не внедряется. Сбой оверлея игру не останавливает. `vulkan` — прокси нужен и с выключенным
    /// оверлеем: через него игра грузит DXVK.
    pub fn prepare_overlay(
        &self,
        game_id: &str,
        install_path: &str,
        config: &LaunchConfig,
        settings: Option<&Value>,
        vulkan: bool,
    ) -> Option<Prepared> {
        prepare_overlay_logged(
            self.inner.live_split_overlay.as_deref()?,
            self.logger(),
            game_id,
            &Target::of(Path::new(install_path), config),
            settings,
            vulkan,
        )
    }

    /// `startRInput(installed, config, settings, gamePid)`: `hl.exe` уже найден, так что RInput
    /// внедрится сразу и завершится сам. Сбой RInput игру не останавливает.
    pub fn start_rinput(
        &self,
        install_path: &str,
        config: &LaunchConfig,
        settings: Option<&Value>,
        game_pid: Option<u32>,
    ) -> Result<(), DeliveryError> {
        let settings = settings.unwrap_or(&Value::Null);
        let Some(rinput) =
            goldsrc_launch::resolve_rinput(Path::new(install_path), config, settings)?
        else {
            return Ok(());
        };
        if rinput.missing {
            self.logger().warn(&format!(
                "[launch] RInput не найден: {}",
                rinput.executable.to_string_lossy()
            ));
            return Ok(());
        }
        let executable = rinput.executable.to_string_lossy().into_owned();
        let child = match self.inner.spawner.spawn(
            &rinput.executable,
            std::slice::from_ref(&rinput.process_name),
            SpawnRequest {
                cwd: PathBuf::from(self.inner.path.dirname(&executable)),
                env: None,
                windows_hide: true,
                detached: true,
            },
        ) {
            Ok(child) => child,
            Err(error) => {
                self.logger()
                    .warn(&format!("[launch] RInput не запущен: {}", error.message));
                return Ok(());
            }
        };
        // Первая попытка патча идёт сразу (у 0.4.7 — до первого `await`), её итог — раньше
        // событий процесса RInput.
        if launch_args::sanitize_settings(settings)
            .settings
            .rinput_menu_fix
        {
            let patch = self.patch_rinput_menu(&rinput.process_name, game_pid);
            *lock(&self.inner.rinput_patch) = patch;
        }
        let logger = self.inner.logger.clone();
        self.keep(spawn_background(
            self.inner.logger.as_ref(),
            "слежение за RInput",
            async move {
                match child.ended().await {
                    ChildExit::Error(error) => {
                        logger.warn(&format!("[launch] RInput не запущен: {}", error.message));
                    }
                    ChildExit::Exit { code, .. } => {
                        logger.info(&format!(
                            "[launch] RInput завершился (код {})",
                            js_code(code)
                        ));
                    }
                }
            },
        ));
        Ok(())
    }

    /// `patchRInputMenu(processName, gamePid)`: когда RInput поставит свои перехваты в `hl.exe`,
    /// они патчатся в памяти (курсор в меню). Сбой патча игру не трогает. Первая попытка — сразу,
    /// остальные — в фоне раз в `rinputPatchPollMs`, пока игра жива.
    pub fn patch_rinput_menu(
        &self,
        process_name: &str,
        game_pid: Option<u32>,
    ) -> Option<JoinHandle<()>> {
        let inner = self.inner.clone();
        let alive = move |inner: &Inner| game_pid.is_none_or(|pid| inner.inspector.is_alive(pid));
        let mut session = (inner.rinput_menu_session)();
        let deadline = Instant::now() + inner.rinput_patch_wait;
        let process_name = process_name.to_owned();
        let first = alive(&inner).then(|| session.patch(&process_name));
        let task = async move {
            let mut attempt = first;
            while let Some(status) = attempt {
                let logger = inner.logger.as_ref();
                match status {
                    Err(message) => {
                        logger.warn(&format!(
                            "[launch] курсор RInput в меню не исправлен: {message}"
                        ));
                        break;
                    }
                    Ok(PatchStatus::Patched | PatchStatus::Already) => {
                        logger.info("[launch] курсор RInput в меню исправлен");
                        break;
                    }
                    Ok(PatchStatus::Unsupported) => {
                        logger.warn(
                            "[launch] курсор RInput в меню не исправлен: незнакомая версия RInput",
                        );
                        break;
                    }
                    Ok(PatchStatus::Waiting) => {}
                }
                if Instant::now() >= deadline {
                    logger.warn(&format!(
                        "[launch] курсор RInput в меню не исправлен: RInput не перехватил курсор за {} с",
                        js::math_round(inner.rinput_patch_wait.as_millis() as f64 / 1000.0)
                    ));
                    break;
                }
                tokio::time::sleep(inner.rinput_patch_poll).await;
                attempt = alive(&inner).then(|| session.patch(&process_name));
            }
            session.close();
        };
        spawn_background(self.inner.logger.as_ref(), "патч курсора RInput", task)
    }

    /// `waitForInjectedGame(child, expectedExecutable, spawnedAt)`: ищется сам `hl.exe` (тот же
    /// файл, запущен не раньше чем за 2 с до инжектора, жив), самый свежий.
    pub async fn wait_for_injected_game(
        &self,
        child: &Child,
        expected_executable: &str,
        spawned_at: f64,
    ) -> Result<StartedGame, DeliveryError> {
        // `once("exit")`/`once("error")`: событие и момент выхода.
        let ended: Arc<Mutex<Option<(ChildExit, Instant)>>> = Arc::new(Mutex::new(None));
        let watcher = {
            let ended = ended.clone();
            let child = child.clone();
            spawn_background(
                self.inner.logger.as_ref(),
                "ожидание выхода инжектора",
                async move {
                    let end = child.ended().await;
                    *lock(&ended) = Some((end, Instant::now()));
                },
            )
        };
        let result = self
            .poll_injected_game(&ended, expected_executable, spawned_at)
            .await;
        if let Some(watcher) = watcher {
            watcher.abort();
        }
        result
    }

    async fn poll_injected_game(
        &self,
        ended: &Mutex<Option<(ChildExit, Instant)>>,
        expected_executable: &str,
        spawned_at: f64,
    ) -> Result<StartedGame, DeliveryError> {
        let inspector = &self.inner.inspector;
        let image_name = self.inner.path.basename(expected_executable);
        let deadline = Instant::now() + self.inner.injected_game_timeout;
        while Instant::now() < deadline {
            let end = lock(ended).clone();
            if let Some((ChildExit::Error(error), _)) = &end {
                return Err(DeliveryError::new(
                    Code::LaunchFailed,
                    format!("Инжектор Bunnymod XT не запустился: {}", error.message),
                ));
            }

            let mut found: Vec<_> = inspector
                .find_by_image_name(&image_name)
                .into_iter()
                .filter(|item| {
                    item.executable_path.as_deref().is_some_and(|path| {
                        !path.is_empty()
                            && inspector.same_path(Some(path), Some(expected_executable))
                    })
                })
                .filter(
                    |item| match item.started_at.as_deref().filter(|at| !at.is_empty()) {
                        None => true,
                        Some(at) => {
                            date_parse(at).is_some_and(|at| at as f64 >= spawned_at - 2000.0)
                        }
                    },
                )
                .filter(|item| inspector.is_alive(item.pid))
                .collect();
            let started_of = |at: Option<&str>| {
                at.filter(|at| !at.is_empty())
                    .and_then(date_parse)
                    .map_or(f64::NEG_INFINITY, |at| at as f64)
            };
            found.sort_by(|a, b| {
                started_of(b.started_at.as_deref())
                    .partial_cmp(&started_of(a.started_at.as_deref()))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            if let Some(first) = found.into_iter().next() {
                return Ok(StartedGame {
                    pid: first.pid,
                    executable_path: first.executable_path.unwrap_or_default(),
                    started_at: first
                        .started_at
                        .filter(|at| !at.is_empty())
                        .unwrap_or_else(|| self.now_iso()),
                });
            }

            if let Some((ChildExit::Exit { code, .. }, at)) = &end {
                if *code != Some(0) {
                    return Err(DeliveryError::new(
                        Code::LaunchFailed,
                        format!(
                            "Инжектор Bunnymod XT завершился с кодом {}",
                            code_text(*code)
                        ),
                    )
                    .with_details(json!({"exitCode": code})));
                }
                if at.elapsed() > self.inner.injector_exit_grace {
                    return Err(DeliveryError::new(
                        Code::LaunchFailed,
                        "Инжектор Bunnymod XT завершился, но игра не запустилась",
                    ));
                }
            }

            tokio::time::sleep(self.inner.injected_game_poll).await;
        }
        Err(DeliveryError::new(
            Code::LaunchFailed,
            "Игра не запустилась через Bunnymod XT за отведённое время",
        ))
    }

    /// `assertSteamRunning()`: `describeSteamOnly()` без ошибки.
    pub fn assert_steam_running(&self) -> Result<Value, DeliveryError> {
        let steam = self.inner.locator.describe_steam_only();
        if !steam.get("ok").is_some_and(js::truthy) {
            let code = steam
                .get("code")
                .and_then(Value::as_str)
                .filter(|code| !code.is_empty())
                .unwrap_or("STEAM_NOT_RUNNING");
            let message = steam
                .get("message")
                .and_then(Value::as_str)
                .filter(|message| !message.is_empty())
                .unwrap_or("Steam не запущен");
            return Err(DeliveryError::new(Code::coerce(code), message));
        }
        Ok(steam)
    }

    /// `explainWithSteamState(delivery)`: неясный отказ запуска при Steam, запущенном с другими
    /// правами, — `STEAM_ELEVATED`.
    pub fn explain_with_steam_state(&self, delivery: DeliveryError) -> DeliveryError {
        if !UNCERTAIN_LAUNCH_CODES.contains(&delivery.code) {
            return delivery;
        }
        let state = self.inner.locator.describe_steam_only();
        if state.get("code").and_then(Value::as_str) != Some("STEAM_ELEVATED") {
            return delivery;
        }
        let message = state
            .get("message")
            .and_then(Value::as_str)
            .filter(|message| !message.is_empty());
        self.logger().info(&format!(
            "[launch] причина уточнена: {} -> STEAM_ELEVATED ({})",
            delivery.code.as_str(),
            message.unwrap_or("права Steam и лаунчера разошлись")
        ));
        DeliveryError::new(
            Code::SteamElevated,
            message.map_or_else(|| delivery.message.clone(), str::to_owned),
        )
        .with_details(json!({
            "originalCode": delivery.code.as_str(),
            "originalMessage": delivery.message,
        }))
    }

    /// `assertOwnedApps(gameId, config)`: лицензии из депо. Неясный ответ Steam запуск не
    /// останавливает — решает игра.
    pub async fn assert_owned_apps(
        &self,
        game_id: &str,
        config: &LaunchConfig,
    ) -> Result<Option<bool>, Failure> {
        let app_ids = config.requires_owned_app_ids;
        let Some(depot) = self.inner.depot.as_ref().filter(|_| !app_ids.is_empty()) else {
            return Ok(None);
        };
        match depot.assert_owns(app_ids).await {
            Ok(()) => Ok(Some(true)),
            Err(failure) => {
                let code = match &failure {
                    Failure::Delivery(error) => Some(error.code.as_str().to_owned()),
                    Failure::Raw(error) => error.code.clone(),
                };
                if code
                    .as_deref()
                    .is_some_and(|code| DEFINITE_STEAM_CODES.contains(&code))
                {
                    return Err(failure);
                }
                self.logger().warn(&format!(
                    "[launch] {game_id}: лицензию подтвердить не удалось ({}: {}) — запуск продолжается, решение за игрой",
                    code.as_deref().filter(|code| !code.is_empty()).unwrap_or("?"),
                    failure_message(&failure)
                ));
                Ok(None)
            }
        }
    }

    /// `applyLanguage(gameId, installPath, config, settings)`: язык игры подменой файлов.
    pub fn apply_language(
        &self,
        game_id: &str,
        install_path: &Path,
        config: &LaunchConfig,
        settings: Option<&Value>,
    ) -> Option<game_language::Applied> {
        let empty = json!({});
        let choice = launch_args::language_of(settings.unwrap_or(&empty));
        let options = ApplyOptions {
            engine_language: LanguageOption::Detect,
            engine: EngineOptions {
                platform: Platform::CURRENT,
                default_language: None,
                registry: self.inner.registry.as_ref(),
            },
        };
        let report = match game_language::apply(
            install_path,
            config.game_directory,
            Some(&choice),
            &options,
        ) {
            Ok(report) => report,
            Err(error) => {
                self.logger().warn(&format!(
                    "[launch] {game_id}: язык не применён ({})",
                    error.message
                ));
                return None;
            }
        };
        let reason = report.reason.map_or("undefined", Reason::as_str);
        if report.applied {
            self.logger().info(&format!(
                "[launch] {game_id}: язык {} подменой {} файлов (движок подставит {})",
                report.language,
                report.files.len(),
                report.engine_language.as_deref().unwrap_or("null")
            ));
        } else if !report.ok {
            let message = report
                .message
                .as_deref()
                .filter(|message| !message.is_empty())
                .map(|message| format!(": {message}"))
                .unwrap_or_default();
            self.logger().warn(&format!(
                "[launch] {game_id}: язык не применён ({reason}){message}"
            ));
        } else if !matches!(report.reason, Some(Reason::Auto | Reason::SameAsEngine)) {
            self.logger().info(&format!(
                "[launch] {game_id}: язык остаётся как в Steam ({reason})"
            ));
        }
        Some(report)
    }

    /// `applyAffinity(gameId, pid)`: «Без ЦП 0» — маска процессоров игры без нулевого. Только
    /// Windows; сбой игру не останавливает. Итог — поставленная маска.
    pub fn apply_affinity(&self, game_id: &str, pid: u32) -> Option<u64> {
        let settings = self.settings_of(game_id)?;
        if settings.get("excludeCpu0") != Some(&Value::Bool(true)) {
            return None;
        }
        if Platform::CURRENT != Platform::Win32 || pid == 0 {
            return None;
        }
        let count = self.inner.affinity.cpu_count();
        let Some(mask) = launch_args::affinity_mask_without_cpu0(f64::from(count))
            .and_then(|mask| mask.parse::<u64>().ok())
        else {
            self.logger().warn(&format!(
                "[launch] {game_id}: процессоров меньше двух, оставить игру без ЦП 0 невозможно"
            ));
            return None;
        };
        match self.inner.affinity.set_affinity(pid, mask) {
            Ok(()) => {
                self.logger().info(&format!(
                    "[launch] {game_id}: маска процессоров 0x{mask:X} ({count} ЦП, нулевой исключён)"
                ));
                Some(mask)
            }
            Err(message) => {
                self.logger().warn(&format!(
                    "[launch] {game_id}: маску процессоров применить не удалось: {message}"
                ));
                None
            }
        }
    }

    /// `waitForStart(gameId, child, expectedExecutable)`: процесс жив через `earlyExitWindowMs`
    /// и это тот самый файл.
    pub async fn wait_for_start(
        &self,
        child: &Child,
        expected_executable: &str,
    ) -> Result<StartedGame, DeliveryError> {
        tokio::time::sleep(self.inner.early_exit_window).await;
        match child.exit_state() {
            Some(ChildExit::Error(error)) => {
                return Err(DeliveryError::new(
                    Code::LaunchFailed,
                    format!("Процесс игры не запустился: {}", error.message),
                ));
            }
            Some(ChildExit::Exit { code, signal }) => {
                return Err(DeliveryError::new(
                    Code::LaunchFailed,
                    format!(
                        "Игра завершилась сразу после запуска (код {}). Обычно это означает, что Steam отклонил запуск.",
                        code_text(code)
                    ),
                )
                .with_details(json!({"exitCode": code, "signal": signal})));
            }
            None => {}
        }
        let inspector = &self.inner.inspector;
        let Some(pid) = child.pid().filter(|pid| inspector.is_alive(*pid)) else {
            return Err(DeliveryError::new(
                Code::LaunchFailed,
                "Процесс игры не найден после запуска",
            ));
        };
        let actual = inspector.inspect(pid);
        let mut started_at = self.now_iso();
        let mut executable_path = expected_executable.to_owned();
        match actual {
            Some(actual) => {
                if let Some(path) = actual.executable_path.as_deref().filter(|p| !p.is_empty()) {
                    if !inspector.same_path(Some(path), Some(expected_executable)) {
                        return Err(DeliveryError::new(
                            Code::LaunchFailed,
                            format!("PID {pid} принадлежит другому файлу: {path}"),
                        ));
                    }
                    executable_path = path.to_owned();
                }
                if let Some(at) = actual.started_at.filter(|at| !at.is_empty()) {
                    started_at = at;
                }
            }
            None => self.logger().warn(&format!(
                "[launch] система не отдала данные о PID {pid} — использую данные spawn"
            )),
        }
        Ok(StartedGame {
            pid,
            executable_path,
            started_at,
        })
    }

    /// `describeAvailability(gameId)`: Steam (и Source SDK, если нужен) для кнопки «Играть».
    pub fn describe_availability(&self, game_id: &str) -> Value {
        let Ok(config) = self.config_for(game_id) else {
            return json!({"ok": false, "code": "LAUNCH_CONFIG_INVALID"});
        };
        if !config.source_sdk_required {
            return self.inner.locator.describe_steam_only();
        }
        self.inner
            .locator
            .describe_availability(config.steam_app_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hlsr_delivery::ports::MemoryLogger;

    #[test]
    fn a_missing_tokio_runtime_is_reported_not_silent() {
        let logger = MemoryLogger::default();
        assert!(spawn_background(&logger, "слежение за RInput", async {}).is_none());
        assert_eq!(
            logger.take(),
            [(
                "warn",
                "[launch] слежение за RInput: фоновая задача не запущена — нет среды tokio"
                    .to_owned()
            )]
        );
    }

    #[tokio::test]
    async fn inside_tokio_the_task_runs() {
        let logger = MemoryLogger::default();
        let task = spawn_background(&logger, "задача", async {}).expect("среда есть");
        task.await.unwrap();
        assert!(logger.take().is_empty());
    }
}
