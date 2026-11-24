//! Запуск игр на Linux через Proton (`linux/LaunchService.js`): проверки (Steam запущен, папка-
//! двойник по регистру, метаданные сборки), шрифты LiveSplit до игры при «таймер вместе с игрой»,
//! Source (язык, подготовка файла игрока, мост presence, мост к Steam; эпизоды Ghosting — отказ) и
//! GoldSrc (инжектор Bunnymod XT, мост к Steam, RInput после BXT, патч курсора RInput), оверлей
//! LiveSplit, слежение за процессом игры внутри живой сессии Proton.
//!
//! Запуск в префиксе — [`ProtonLauncher`] (`ProtonRunner`), процессы — [`LaunchProcesses`]
//! (`GameProcessService`), установка — [`LaunchDelivery`]. Части других задач подключаются
//! портами: [`GoldSrc`] — `goldsrc-launch.js` (задача 6), `rinput_menu_patcher` —
//! `linux/rinput-menu-fix.js` (задача 5).

use crate::game_process_service::StartedProcess;
use crate::linux::path_case::find_case_conflict;
use crate::linux::presence_relay::{self, PresenceRelay, PresenceTarget};
use crate::linux::proton_runner::{
    LaunchOptions, ProtonLaunch, ProtonLauncher, SteamBridge, windows_path_of,
};
use crate::livesplit_overlay::{
    LiveSplitOverlay, Prepared, Target, WaitOptions, merge_overrides, process_name_of,
};
use crate::spawn::{Child, ChildExit, Env};
use crate::steam_fs::RealFs;
use crate::steam_locator::SteamInstall;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform};
use hlsr_delivery::build_metadata_trust::{self, State, VerifyAgainstSnapshot};
use hlsr_delivery::cfg_search::{CfgSearch, SteamLanguage};
use hlsr_delivery::errors::{Code, DeliveryError, Failure, to_delivery_error};
use hlsr_delivery::manifests::{LaunchConfig, launch_config_of};
use hlsr_delivery::paths::assert_inside;
use hlsr_delivery::ports::{Logger, SystemRegistry, TracingLogger};
use hlsr_delivery::providers::BoxFuture;
use hlsr_delivery::{episode_install, episodes, game_language, launch_args};
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

// ---------------------------------------------------------------- порты

// Порты доставки, службы процессов и настроек — общие с Windows-`LaunchService`; запуск GoldSrc —
// `goldsrc-launch.js` ([`crate::goldsrc_launch`]).
pub use crate::goldsrc_launch::{GoldSrc, GoldSrcFiles, RInputPlan, is_goldsrc};
pub use crate::launch_service::{
    DependencyUpdater, LaunchDelivery, LaunchProcesses, LaunchSettings, PackApplier,
};

/// Steam (`steamLocator`).
pub trait LaunchSteam: Send + Sync {
    fn locate_steam(&self) -> Result<SteamInstall, DeliveryError>;
    fn assert_steam_running(&self, steam: &SteamInstall) -> Result<(), DeliveryError>;
}

impl LaunchSteam for crate::linux::steam_locator::SteamLocator {
    fn locate_steam(&self) -> Result<SteamInstall, DeliveryError> {
        crate::linux::steam_locator::SteamLocator::locate_steam(self)
    }
    fn assert_steam_running(&self, _steam: &SteamInstall) -> Result<(), DeliveryError> {
        crate::linux::steam_locator::SteamLocator::assert_steam_running(self).map(drop)
    }
}

/// Есть ли процесс игры (`processRunning(name)`).
pub type ProcessRunning = Arc<dyn Fn(&str) -> bool + Send + Sync>;
/// Есть ли процесс игры в её префиксе (`gameProcessRunning(name, gameId)`).
pub type GameProcessRunning = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;

/// Патч курсора RInput в меню (`rinputMenuPatcher(processName)`): `patched`, `already`,
/// `unsupported` или «ещё рано» (любое другое); `Err` — сбой.
pub type RInputMenuPatcher =
    Arc<dyn Fn(&str) -> BoxFuture<'static, Result<String, String>> + Send + Sync>;

// ---------------------------------------------------------------- /proc

/// Числовые каталоги `/proc`.
fn proc_pids(proc: &Path) -> Vec<String> {
    js::fs::read_dir_names(proc)
        .unwrap_or_default()
        .into_iter()
        .filter(|name| !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()))
        .collect()
}

fn comm_of(proc: &Path, pid: &str) -> Option<String> {
    std::fs::read(proc.join(pid).join("comm"))
        .ok()
        .map(|bytes| js::trim(&js::utf8_string(&bytes)).to_lowercase())
}

/// `bxtInjected()`: Bunnymod XT уже внутри `hl.exe` — Wine называет процесс по имени exe, а
/// внедрённая DLL видна в его карте памяти.
pub fn bxt_injected_at(proc: &Path) -> bool {
    proc_pids(proc).iter().any(|pid| {
        comm_of(proc, pid).as_deref() == Some("hl.exe")
            && std::fs::read(proc.join(pid).join("maps")).is_ok_and(|maps| {
                js::utf8_string(&maps)
                    .to_lowercase()
                    .contains("bunnymodxt.dll")
            })
    })
}

/// `processRunning(name)`: процесс игры есть (имя `comm` — до 15 знаков).
pub fn process_running_at(proc: &Path, name: &str) -> bool {
    let wanted = js::slice_utf16(&name.to_lowercase(), 15);
    proc_pids(proc)
        .iter()
        .any(|pid| comm_of(proc, pid).as_deref() == Some(wanted.as_str()))
}

/// `gameProcessRunning(name, gameId)`: процесс игры в префиксе `gameId` — имя exe и префикс в
/// окружении (`STEAM_COMPAT_DATA_PATH` или `WINEPREFIX` ведут в `compatdata/<gameId>`).
/// Окружение без доступа не мешает: тогда хватает имени.
pub fn game_process_running_at(proc: &Path, name: &str, game_id: &str) -> bool {
    let wanted = js::slice_utf16(&name.to_lowercase(), 15);
    let prefix = format!("/compatdata/{game_id}/");
    proc_pids(proc).iter().any(|pid| {
        if comm_of(proc, pid).as_deref() != Some(wanted.as_str()) {
            return false;
        }
        match std::fs::read(proc.join(pid).join("environ")) {
            Ok(environ) => js::utf8_string(&environ).contains(&prefix),
            Err(_) => true,
        }
    })
}

/// `process.kill(-pid, "SIGTERM")`.
fn kill_process_group(pid: u32) {
    #[cfg(unix)]
    if let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::TERM);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

// ---------------------------------------------------------------- служба

/// Параметры конструктора (`options`).
#[derive(Clone, Default)]
pub struct LaunchServiceOptions {
    pub delivery: Option<Arc<dyn LaunchDelivery>>,
    /// `delivery.processService`.
    pub processes: Option<Arc<dyn LaunchProcesses>>,
    /// `protonRunner`.
    pub runner: Option<Arc<dyn ProtonLauncher>>,
    pub logger: Option<Arc<dyn Logger>>,
    pub settings_store: Option<Arc<dyn LaunchSettings>>,
    pub logs_dir: Option<PathBuf>,
    pub dependency_updater: Option<DependencyUpdater>,
    pub pack_applier: Option<PackApplier>,
    /// `launcherVersion`; по умолчанию `0.0.0`.
    pub launcher_version: Option<String>,
    pub steam_locator: Option<Arc<dyn LaunchSteam>>,
    pub goldsrc: Option<Arc<dyn GoldSrc>>,
    /// Поиск cfg (бинды, стандартный cfg); по умолчанию — язык Steam из системного реестра.
    pub cfg_search: Option<Arc<CfgSearch>>,
    pub kill_process_group: Option<Arc<dyn Fn(u32) + Send + Sync>>,
    pub bxt_injected: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    pub bxt_wait: Option<Duration>,
    pub bxt_poll: Option<Duration>,
    pub rinput_menu_patcher: Option<RInputMenuPatcher>,
    pub rinput_patch_wait: Option<Duration>,
    pub live_split_overlay: Option<Arc<LiveSplitOverlay>>,
    pub presence_relay: Option<Arc<PresenceRelay>>,
    /// `extraEnvProvider()`: окружение моста presence лаунчера.
    pub extra_env_provider: Option<Arc<dyn Fn() -> Option<Env> + Send + Sync>>,
    pub presence_wait: Option<Duration>,
    pub process_running: Option<ProcessRunning>,
    pub game_process_running: Option<GameProcessRunning>,
    pub game_watch: Option<Duration>,
    pub presence_start_wait: Option<Duration>,
    pub presence_settle: Option<Duration>,
    /// Сколько ждать выхода Proton сразу после запуска (1,5 с).
    pub start_check: Option<Duration>,
    /// Часы (`startedAt`).
    pub now: Option<Arc<dyn Fn() -> SystemTime + Send + Sync>>,
}

/// Итог `launch(gameId)`: `{status: "started" | "already-running", process, dependencies?,
/// packs?}`.
pub type LaunchResult = Value;

/// Что запущено (`startSource`/`startGoldSrc`).
pub struct Started {
    pub launch: ProtonLaunch,
    pub executable_path: PathBuf,
    pub dependencies: Option<Value>,
    pub packs: Option<Value>,
}

/// План запуска GoldSrc (`goldSrcLaunch`).
#[derive(Debug, Clone)]
pub struct GoldSrcPlan {
    pub files: GoldSrcFiles,
    /// Инжектор Bunnymod XT.
    pub executable: PathBuf,
    pub argv: Vec<String>,
    pub options: LaunchOptions,
}

struct Inner {
    delivery: Option<Arc<dyn LaunchDelivery>>,
    processes: Option<Arc<dyn LaunchProcesses>>,
    runner: Option<Arc<dyn ProtonLauncher>>,
    logger: Arc<dyn Logger>,
    settings_store: Option<Arc<dyn LaunchSettings>>,
    logs_dir: Option<PathBuf>,
    dependency_updater: Option<DependencyUpdater>,
    pack_applier: Option<PackApplier>,
    launcher_version: String,
    steam: Option<Arc<dyn LaunchSteam>>,
    goldsrc: Option<Arc<dyn GoldSrc>>,
    cfg_search: Arc<CfgSearch>,
    kill_process_group: Arc<dyn Fn(u32) + Send + Sync>,
    bxt_injected: Arc<dyn Fn() -> bool + Send + Sync>,
    bxt_wait: Duration,
    bxt_poll: Duration,
    rinput_menu_patcher: Option<RInputMenuPatcher>,
    rinput_patch_wait: Duration,
    live_split_overlay: Option<Arc<LiveSplitOverlay>>,
    presence_relay: Option<Arc<PresenceRelay>>,
    extra_env_provider: Option<Arc<dyn Fn() -> Option<Env> + Send + Sync>>,
    presence_wait: Duration,
    process_running: ProcessRunning,
    game_process_running: GameProcessRunning,
    game_watch: Duration,
    presence_start_wait: Duration,
    presence_settle: Duration,
    start_check: Duration,
    now: Arc<dyn Fn() -> SystemTime + Send + Sync>,
    path: NodePath,
    launching: Mutex<HashSet<String>>,
    rinput_start: Mutex<Option<tokio::task::JoinHandle<()>>>,
    presence_ready: Mutex<Option<tokio::task::JoinHandle<bool>>>,
}

/// `LaunchService` (Linux). Копии — та же служба.
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

/// Процесс вышел (`once("exit")`): у ошибки запуска выхода нет.
async fn exited(child: &Child) {
    loop {
        if let ChildExit::Exit { .. } = child.ended().await {
            return;
        }
        // Ошибка без выхода: события `exit` не будет.
        std::future::pending::<()>().await;
    }
}

/// Значение `settings.<key> === true`.
fn flag(settings: Option<&Value>, key: &str) -> bool {
    settings
        .and_then(|settings| settings.get(key))
        .is_some_and(|value| value == &Value::Bool(true))
}

impl LaunchService {
    pub fn new(options: LaunchServiceOptions) -> LaunchService {
        let ms = Duration::from_millis;
        LaunchService {
            inner: Arc::new(Inner {
                delivery: options.delivery,
                processes: options.processes,
                runner: options.runner,
                logger: options.logger.unwrap_or_else(|| Arc::new(TracingLogger)),
                settings_store: options.settings_store,
                logs_dir: options.logs_dir,
                dependency_updater: options.dependency_updater,
                pack_applier: options.pack_applier,
                launcher_version: options
                    .launcher_version
                    .unwrap_or_else(|| "0.0.0".to_owned()),
                steam: options.steam_locator,
                goldsrc: options.goldsrc,
                // Язык Steam под Proton из реестра не читается (`process.platform` — linux).
                cfg_search: options.cfg_search.unwrap_or_else(|| {
                    Arc::new(CfgSearch::new(SteamLanguage::with(
                        Arc::new(SystemRegistry),
                        Platform::Linux,
                        Box::new(Instant::now),
                    )))
                }),
                kill_process_group: options
                    .kill_process_group
                    .unwrap_or_else(|| Arc::new(kill_process_group)),
                bxt_injected: options
                    .bxt_injected
                    .unwrap_or_else(|| Arc::new(|| bxt_injected_at(Path::new("/proc")))),
                bxt_wait: options.bxt_wait.unwrap_or(ms(30_000)),
                bxt_poll: options.bxt_poll.unwrap_or(ms(500)),
                rinput_menu_patcher: options.rinput_menu_patcher,
                rinput_patch_wait: options.rinput_patch_wait.unwrap_or(ms(30_000)),
                live_split_overlay: options.live_split_overlay,
                presence_relay: options.presence_relay,
                extra_env_provider: options.extra_env_provider,
                presence_wait: options.presence_wait.unwrap_or(ms(20_000)),
                process_running: options.process_running.unwrap_or_else(|| {
                    Arc::new(|name: &str| process_running_at(Path::new("/proc"), name))
                }),
                game_process_running: options.game_process_running.unwrap_or_else(|| {
                    Arc::new(|name: &str, game_id: &str| {
                        game_process_running_at(Path::new("/proc"), name, game_id)
                    })
                }),
                game_watch: options.game_watch.unwrap_or(ms(2000)),
                presence_start_wait: options.presence_start_wait.unwrap_or(ms(60_000)),
                presence_settle: options.presence_settle.unwrap_or(ms(5000)),
                start_check: options.start_check.unwrap_or(ms(1500)),
                now: options.now.unwrap_or_else(|| Arc::new(SystemTime::now)),
                path: NodePath::native(),
                launching: Mutex::new(HashSet::new()),
                rinput_start: Mutex::new(None),
                presence_ready: Mutex::new(None),
            }),
        }
    }

    fn runner(&self) -> Result<&Arc<dyn ProtonLauncher>, DeliveryError> {
        self.inner
            .runner
            .as_ref()
            .ok_or_else(|| DeliveryError::new(Code::LaunchFailed, "Proton не настроен"))
    }

    fn processes(&self) -> Result<&Arc<dyn LaunchProcesses>, DeliveryError> {
        self.inner
            .processes
            .as_ref()
            .ok_or_else(|| DeliveryError::new(Code::LaunchFailed, "Служба процессов не настроена"))
    }

    fn delivery(&self) -> Result<&Arc<dyn LaunchDelivery>, DeliveryError> {
        self.inner
            .delivery
            .as_ref()
            .ok_or_else(|| DeliveryError::new(Code::LaunchFailed, "Доставка не настроена"))
    }

    fn steam(&self) -> Result<&Arc<dyn LaunchSteam>, DeliveryError> {
        self.inner
            .steam
            .as_ref()
            .ok_or_else(|| DeliveryError::new(Code::SteamNotInstalled, "Поиск Steam не настроен"))
    }

    fn goldsrc(&self) -> Result<&Arc<dyn GoldSrc>, DeliveryError> {
        self.inner
            .goldsrc
            .as_ref()
            .ok_or_else(|| DeliveryError::new(Code::LaunchFailed, "Запуск GoldSrc не настроен"))
    }

    /// Фоновый запуск RInput последней игры (`this.rinputStart`): дождаться.
    pub async fn rinput_started(&self) {
        let handle = lock(&self.inner.rinput_start).take();
        if let Some(handle) = handle {
            let _ = handle.await;
        }
    }

    /// Мост presence (`this.presenceReady`): готов ли.
    pub async fn presence_ready(&self) -> Option<bool> {
        let handle = lock(&self.inner.presence_ready).take();
        match handle {
            Some(handle) => handle.await.ok(),
            None => None,
        }
    }

    /// `configFor(gameId)`: настройки запуска игры.
    pub fn config_for(&self, game_id: &str) -> Result<&'static LaunchConfig, DeliveryError> {
        launch_config_of(game_id)
            .filter(|config| {
                !config.executable.is_empty()
                    && !self.inner.path.is_absolute(config.executable)
                    && !config.game_directory.is_empty()
            })
            .ok_or_else(|| {
                DeliveryError::new(
                    Code::LaunchConfigInvalid,
                    format!("Некорректная конфигурация запуска {game_id}"),
                )
            })
    }

    /// `filesFor(installPath, config)`: exe и `gameinfo.txt` на месте.
    pub fn files_for(
        &self,
        install_path: &Path,
        config: &LaunchConfig,
    ) -> Result<(PathBuf, PathBuf), DeliveryError> {
        let install = install_path.to_string_lossy();
        let executable = assert_inside(
            install_path,
            Path::new(&self.inner.path.join(&[&install, config.executable])),
            None,
        )?;
        let game_directory = assert_inside(
            install_path,
            Path::new(&self.inner.path.join(&[&install, config.game_directory])),
            None,
        )?;
        if !executable.exists() {
            return Err(DeliveryError::new(
                Code::GameExecutableMissing,
                format!("Игра не найдена: {}", config.executable),
            ));
        }
        if !episodes::has_gameinfo(&game_directory) {
            return Err(DeliveryError::new(
                Code::GameDirectoryMissing,
                format!("Не найден gameinfo.txt в {}", config.game_directory),
            ));
        }
        Ok((executable, game_directory))
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
            .and_then(|reason| Code::from_name(reason.as_str()))
            .unwrap_or(Code::BuildMetadataMismatch);
        Err(DeliveryError::new(
            code,
            "Метаданные сборки не подтверждены",
        ))
    }

    /// `steamBridgeOf(installed, config)`: мост к клиенту Steam — игра видна в Steam под своим
    /// AppID, как на Windows.
    pub fn steam_bridge_of(
        &self,
        install_path: &str,
        config: &LaunchConfig,
    ) -> Result<Option<SteamBridge>, DeliveryError> {
        if config.steam_bridge != Some(true) {
            return Ok(None);
        }
        let steam = self.steam()?;
        let install = steam.locate_steam()?;
        steam.assert_steam_running(&install)?;
        Ok(Some(SteamBridge {
            app_id: i64::from(config.steam_app_id),
            steam_path: install.install_path,
            install_path: Some(install_path.to_owned()),
        }))
    }

    /// `protonOptionsOf(settings)`: параметры Proton игрока; испорченная строка не применяется.
    fn proton_options_of(settings: Option<&Value>) -> (Env, Vec<String>, Vec<String>) {
        let value = launch_args::sanitize_settings(settings.unwrap_or(&Value::Null))
            .settings
            .proton_options;
        match launch_args::parse_proton_options(&Value::String(value)) {
            Ok(parsed) => (
                parsed
                    .env
                    .iter()
                    .map(|(k, v)| (k.to_owned(), v.clone()))
                    .collect(),
                parsed.wrapper,
                parsed.args,
            ),
            Err(_) => (Env::new(), Vec::new(), Vec::new()),
        }
    }

    /// `goldSrcLaunch(installed, config, settings)`: HL1 WON — `hl.exe` через инжектор Bunnymod XT,
    /// старому `steam.dll` нужен запущенный клиент Steam (Proton в режиме моста к Steam).
    pub fn goldsrc_launch(
        &self,
        install_path: &Path,
        config: &LaunchConfig,
        settings: Option<&Value>,
    ) -> Result<GoldSrcPlan, DeliveryError> {
        let goldsrc = self.goldsrc()?;
        let files = goldsrc.resolve_files(install_path, config)?;
        goldsrc.enforce_steam_app_id_file(install_path, config, self.inner.logger.as_ref())?;
        let (proton_env, wrapper, proton_args) = Self::proton_options_of(settings);
        let null = Value::Null;
        let mut argv = goldsrc.merge_arguments(config, settings.unwrap_or(&null));
        argv.extend(proton_args);
        let mut env = goldsrc.environment_of(config);
        env.extend(proton_env);
        if let Some(bxt_log) =
            goldsrc.prepare_bxt_log(self.inner.logs_dir.as_deref(), self.inner.logger.as_ref())
        {
            env.insert(
                "BXT_LOGFILE".to_owned(),
                windows_path_of(&bxt_log.to_string_lossy()),
            );
        }
        let cwd = PathBuf::from(self.inner.path.dirname(&files.executable.to_string_lossy()));
        let mut options = LaunchOptions {
            cwd: Some(cwd),
            env,
            wrapper,
            exclude_cpu0: flag(settings, "excludeCpu0"),
            ..LaunchOptions::default()
        };
        options.steam_bridge = self.steam_bridge_of(&install_path.to_string_lossy(), config)?;
        let mut full = vec![windows_path_of(&files.executable.to_string_lossy())];
        full.extend(argv);
        Ok(GoldSrcPlan {
            executable: files.injector.clone(),
            files,
            argv: full,
            options,
        })
    }

    /// `launching.size > 0`: идёт запуск какой-то игры (отказ `set-library-path`).
    pub fn is_launching(&self) -> bool {
        !lock(&self.inner.launching).is_empty()
    }

    /// `launch(gameId)`.
    pub async fn launch(&self, game_id: &str) -> Result<LaunchResult, DeliveryError> {
        self.launch_episode(game_id, None).await
    }

    /// `launch(gameId, {episode})`: `episode` (`ep1` | `ep2`) — эпизод Ghosting, тот же `hl2.exe`
    /// с `-game episodic|ep2` под Proton.
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
        let processes = self.processes()?.clone();
        if lock(&self.inner.launching).contains(game_id) {
            return Err(DeliveryError::new(
                Code::AlreadyRunning,
                "Запуск игры уже выполняется",
            ));
        }
        if processes.is_running(game_id) {
            return Ok(json!({
                "status": "already-running",
                "process": processes.get_process_state(game_id),
            }));
        }
        if self.delivery()?.is_busy() {
            return Err(DeliveryError::new(
                Code::AlreadyRunning,
                "Дождитесь завершения текущей операции",
            ));
        }
        if let Some(episode) = episode {
            let installed = self.delivery()?.installed_of(game_id)?;
            let install_path = installed
                .get("installPath")
                .map(js::to_string)
                .unwrap_or_default();
            if !episodes::status(Some(Path::new(&install_path)))[episode]["installed"]
                .as_bool()
                .unwrap_or(false)
            {
                return Err(DeliveryError::new(
                    Code::NotInstalled,
                    format!("Эпизод {episode} не установлен"),
                )
                .with_details(json!({"gameId": game_id, "episode": episode})));
            }
        }
        lock(&self.inner.launching).insert(game_id.to_owned());
        processes.mark_starting(game_id);
        let result = self.launch_started(game_id, base, &config, episode).await;
        lock(&self.inner.launching).remove(game_id);
        result.map_err(|failure| {
            let error = to_delivery_error(failure, Some(Code::LaunchFailed));
            processes.cancel_starting(
                game_id,
                &format!("{}: {}", error.code.as_str(), error.message),
            );
            error
        })
    }

    async fn launch_started(
        &self,
        game_id: &str,
        base: &'static LaunchConfig,
        config: &LaunchConfig,
        episode: Option<&str>,
    ) -> Result<LaunchResult, Failure> {
        // Все сборки идут с -steam: без клиента Steam игра падает на старте.
        let steam = self.steam()?;
        let install = steam.locate_steam()?;
        steam.assert_steam_running(&install)?;
        let installed = self.delivery()?.installed_of(game_id)?;
        let install_path = installed
            .get("installPath")
            .map(js::to_string)
            .unwrap_or_default();
        // config.cfg эпизода, не вернувшийся после краха игры или лаунчера, — в Ghosting до запуска.
        if game_id == episode_install::GAME_ID {
            episodes::return_pending(Path::new(&install_path), self.inner.logger.as_ref(), None);
        }
        if let Some(conflict) = find_case_conflict(&install_path, &RealFs) {
            return Err(DeliveryError::new(
                Code::LibraryPathCaseConflict,
                format!(
                    "Рядом с {} лежит {}: Wine путает папки, имена которых различаются только регистром",
                    conflict.folder, conflict.twin
                ),
            )
            .with_details(json!({"folder": conflict.folder, "twin": conflict.twin}))
            .into());
        }
        self.assert_build_metadata(&installed)?;
        let settings = self
            .inner
            .settings_store
            .as_ref()
            .map(|store| store.get(game_id));
        self.prepare_timer_fonts(game_id, settings.as_ref()).await;
        let started = if is_goldsrc(base) {
            self.start_goldsrc(game_id, Path::new(&install_path), base, settings.as_ref())
                .await?
        } else {
            self.start_source_episode(
                game_id,
                Path::new(&install_path),
                base,
                config,
                episode,
                &installed,
                settings,
            )
            .await?
        };
        let child = started.launch.child.clone();
        tokio::time::sleep(self.inner.start_check).await;
        let early = match child.exit_state() {
            Some(ChildExit::Error(error)) => Some(error.message),
            Some(ChildExit::Exit {
                code: Some(code), ..
            }) => Some(format!(
                "Proton завершился сразу после запуска (код {code})"
            )),
            _ => None,
        };
        let pid = child.pid();
        if let Some(message) = early.or_else(|| {
            pid.is_none()
                .then(|| "Proton завершился сразу после запуска (код null)".to_owned())
        }) {
            return Err(DeliveryError::new(Code::LaunchFailed, message).into());
        }
        let state = self.delivery()?.register_started_process(
            game_id,
            StartedProcess {
                pid: pid.map(i64::from),
                executable_path: Some(started.executable_path.to_string_lossy().into_owned()),
                host_process: true,
                started_at: Some(js::iso_string((self.inner.now)())),
                child: child.handle(),
                episode: episode.map(str::to_owned),
                ..StartedProcess::default()
            },
        )?;
        self.watch_game_process(game_id, config, child);
        self.inner.logger.info(&format!(
            "[launch] {game_id}{}: Proton {}, префикс {}",
            episode
                .map(|episode| format!(" (эпизод {episode})"))
                .unwrap_or_default(),
            started.launch.proton.name,
            started.launch.prefix
        ));
        let mut result = Map::new();
        result.insert("status".to_owned(), json!("started"));
        result.insert("process".to_owned(), state);
        if let Some(dependencies) = started.dependencies.filter(js::truthy) {
            result.insert("dependencies".to_owned(), dependencies);
        }
        if let Some(packs) = started.packs.filter(js::truthy) {
            result.insert("packs".to_owned(), packs);
        }
        Ok(Value::Object(result))
    }

    /// `watchGameProcess(gameId, config, gameChild)`: сессия umu живёт, пока в префиксе есть
    /// программы (открытый LiveSplit держит её), поэтому за игрой следят по её процессу: он
    /// появился, а потом пропал на три опроса подряд — игра закрыта. Три, а не один: GoldSrc
    /// перезапускает себя (смена видеорежима).
    pub fn watch_game_process(&self, game_id: &str, config: &LaunchConfig, game_child: Child) {
        let Ok(processes) = self.processes().cloned() else {
            return;
        };
        let process_name = process_name_of(config.executable);
        let inner = self.inner.clone();
        let game_id = game_id.to_owned();
        tokio::spawn(async move {
            let mut seen = false;
            let mut missing = 0;
            let watch = async {
                loop {
                    tokio::time::sleep(inner.game_watch).await;
                    // Игра уже закрыта; следующий запуск отсекает pid в markExited.
                    if !processes.is_running(&game_id) {
                        return;
                    }
                    if (inner.game_process_running)(&process_name, &game_id) {
                        seen = true;
                        missing = 0;
                        continue;
                    }
                    if !seen {
                        continue;
                    }
                    missing += 1;
                    if missing < 3 {
                        continue;
                    }
                    inner.logger.info(&format!(
                        "[launch] {game_id}: {process_name} закрылся, сессия Proton ещё жива (в префиксе остались программы, например LiveSplit)"
                    ));
                    if let Some(pid) = game_child.pid() {
                        processes.mark_exited(&game_id, pid);
                    }
                    return;
                }
            };
            tokio::select! {
                () = watch => {}
                () = exited(&game_child) => {}
            }
        });
    }

    /// `prepareTimerFonts(gameId, settings)`: таймер стартует вместе с игрой, а первый запуск
    /// LiveSplit ставит свои шрифты в тот же префикс (`regedit` параллельно со стартом ронял
    /// игру) — шрифты ставятся заранее; сбой игру не останавливает.
    async fn prepare_timer_fonts(&self, game_id: &str, settings: Option<&Value>) {
        if !flag(settings, "startTimer") {
            return;
        }
        let Some(runner) = self.inner.runner.as_ref() else {
            return;
        };
        if let Err(error) = runner.ensure_live_split_fonts(game_id).await {
            self.inner.logger.warn(&format!(
                "[launch] шрифты LiveSplit не подготовлены до запуска: {}",
                error.message()
            ));
        }
    }

    /// `gameLogFile(gameId)`: вывод umu и Proton последнего запуска (уходит в ZIP диагностики).
    pub fn game_log_file(&self, game_id: &str) -> Option<PathBuf> {
        self.inner
            .logs_dir
            .as_ref()
            .map(|dir| dir.join(format!("proton-{game_id}.log")))
    }

    /// `startGoldSrc(gameId, installed, config, settings)`.
    pub async fn start_goldsrc(
        &self,
        game_id: &str,
        install_path: &Path,
        config: &'static LaunchConfig,
        settings: Option<&Value>,
    ) -> Result<Started, Failure> {
        let dependencies = match &self.inner.dependency_updater {
            Some(update) => update(install_path, config).await,
            None => json!([]),
        };
        self.prepare_configs(game_id, install_path, config, settings);
        let packs = match &self.inner.pack_applier {
            Some(apply) => {
                Some(apply(install_path, config, settings.unwrap_or(&Value::Null)).await)
            }
            None => None,
        };
        let mut plan = self.goldsrc_launch(install_path, config, settings)?;
        plan.options.log_file = self.game_log_file(game_id);
        Self::apply_overlay(
            &mut plan.options,
            self.prepare_overlay(game_id, install_path, config, settings)
                .as_ref(),
        );
        let launch = self
            .runner()?
            .launch(game_id, &plan.executable, &plan.argv, plan.options.clone())
            .await
            .map_err(Failure::from)?;
        // RInput стартует в фоне, когда BXT уже внедрён: запуск игры его не ждёт.
        let this = self.clone();
        let game = game_id.to_owned();
        let install = install_path.to_path_buf();
        let rinput_settings = settings.cloned();
        let rinput_plan = plan.clone();
        let game_child = launch.child.clone();
        let rinput = tokio::spawn(async move {
            if let Err(message) = this
                .start_rinput(
                    &game,
                    &install,
                    config,
                    rinput_settings.as_ref(),
                    &rinput_plan,
                    game_child,
                )
                .await
            {
                this.inner
                    .logger
                    .warn(&format!("[launch] RInput не запущен: {message}"));
            }
        });
        *lock(&self.inner.rinput_start) = Some(rinput);
        Ok(Started {
            executable_path: plan.files.executable,
            dependencies: Some(dependencies),
            packs,
            launch,
        })
    }

    /// `waitForBxt(gameChild)`: инжектор BXT держит игру приостановленной, пока внедряется; то,
    /// что влезет в неё раньше, ломает ему внедрение. `None` — игра уже закрылась.
    pub async fn wait_for_bxt(&self, game_child: &Child) -> Option<bool> {
        if game_child.has_exited() {
            return None;
        }
        let deadline = Instant::now() + self.inner.bxt_wait;
        let poll = async {
            loop {
                if (self.inner.bxt_injected)() {
                    return true;
                }
                if Instant::now() >= deadline {
                    return false;
                }
                tokio::time::sleep(self.inner.bxt_poll).await;
            }
        };
        tokio::select! {
            ready = poll => Some(ready),
            () = exited(game_child) => None,
        }
    }

    /// `startRInput(...)`: RInput — отдельная сессия Proton в тот же префикс, сам ждёт `hl.exe`;
    /// игра закрылась — его группа процессов гасится. Сбой RInput игру не останавливает.
    async fn start_rinput(
        &self,
        game_id: &str,
        install_path: &Path,
        config: &LaunchConfig,
        settings: Option<&Value>,
        plan: &GoldSrcPlan,
        game_child: Child,
    ) -> Result<(), String> {
        let goldsrc = self.goldsrc().map_err(|error| error.message)?;
        let Some(rinput) = goldsrc
            .resolve_rinput(install_path, config, settings.unwrap_or(&Value::Null))
            .map_err(|error| error.message)?
        else {
            return Ok(());
        };
        if rinput.missing {
            self.inner.logger.warn(&format!(
                "[launch] RInput не найден: {}",
                rinput.executable.to_string_lossy()
            ));
            return Ok(());
        }
        let Some(ready) = self.wait_for_bxt(&game_child).await else {
            return Ok(());
        };
        if !ready {
            self.inner.logger.warn(&format!(
                "[launch] Bunnymod XT не появился в {} за {} с, RInput запускается без него",
                rinput.process_name,
                js::math_round(self.inner.bxt_wait.as_millis() as f64 / 1000.0)
            ));
        }
        let runner = self.runner().map_err(|error| error.message)?;
        let mut env = Env::new();
        // Консольная программа: мост steam.exe открыл бы ей окно, и оно отбирало бы фокус у игры.
        env.insert("PROTON_HIDE_PROCESS_WINDOW".to_owned(), "1".to_owned());
        let options = LaunchOptions {
            cwd: Some(PathBuf::from(
                self.inner
                    .path
                    .dirname(&rinput.executable.to_string_lossy()),
            )),
            skip_fonts: true,
            steam_bridge: plan.options.steam_bridge.clone(),
            env,
            ..LaunchOptions::default()
        };
        let launched = match runner
            .launch(
                game_id,
                &rinput.executable,
                std::slice::from_ref(&rinput.process_name),
                options,
            )
            .await
        {
            Ok(launched) => launched,
            Err(error) => {
                self.inner
                    .logger
                    .warn(&format!("[launch] RInput не запущен: {}", error.message()));
                return Ok(());
            }
        };
        let child = launched.child;
        // RInput вышел сам — в журнал; игра закрылась раньше него — его группа гасится.
        let logger = self.inner.logger.clone();
        let session = child.clone();
        tokio::spawn(async move {
            if let ChildExit::Exit { code, .. } = session.ended().await {
                let code = code.map_or_else(|| "null".to_owned(), |c| c.to_string());
                logger.info(&format!("[launch] RInput завершился (код {code})"));
            }
        });
        let kill = self.inner.kill_process_group.clone();
        let session = child.clone();
        let game = game_child.clone();
        tokio::spawn(async move {
            exited(&game).await;
            if !session.has_exited()
                && let Some(pid) = session.pid()
            {
                kill(pid);
            }
        });
        self.inner
            .logger
            .info(&format!("[launch] RInput ждёт {}", rinput.process_name));
        let menu_fix = launch_args::sanitize_settings(settings.unwrap_or(&Value::Null))
            .settings
            .rinput_menu_fix;
        if menu_fix {
            self.patch_rinput_menu(&rinput.process_name, &game_child)
                .await;
        }
        Ok(())
    }

    /// `prepareOverlay(gameId, installed, config, settings)`: оверлей LiveSplit — прокси
    /// `d3d9.dll` или `opengl32.dll` кладётся в папку игры до запуска, игра грузит его сама, а
    /// `WINEDLLOVERRIDES` (`apply_overlay`) велит Wine взять его из папки игры, а не свою DLL. Ничего
    /// не внедряется. Окно LiveSplit прокси ищет в своём префиксе, поэтому LiveSplit должен быть
    /// запущен для этой же игры. Сбой оверлея игру не останавливает.
    pub fn prepare_overlay(
        &self,
        game_id: &str,
        install_path: &Path,
        config: &LaunchConfig,
        settings: Option<&Value>,
    ) -> Option<Prepared> {
        crate::launch_service::prepare_overlay_logged(
            self.inner.live_split_overlay.as_deref()?,
            self.inner.logger.as_ref(),
            game_id,
            &Target::of(install_path, config),
            settings,
            false,
        )
    }
    /// `applyOverlay(options, prepared)`: замена DLL для Wine в окружение игры; пользовательские
    /// замены остаются.
    pub fn apply_overlay(options: &mut LaunchOptions, prepared: Option<&Prepared>) {
        let Some(addition) = prepared.and_then(|prepared| prepared.dll_override.as_deref()) else {
            return;
        };
        let merged = merge_overrides(
            options.env.get("WINEDLLOVERRIDES").map(String::as_str),
            addition,
        );
        options.env.insert("WINEDLLOVERRIDES".to_owned(), merged);
    }
    /// `presenceTarget(config)`: канал и токен моста для игры; `None` — мост этой игре не нужен
    /// или на хосте не поднят.
    pub fn presence_target(&self, config: &LaunchConfig) -> Option<PresenceTarget> {
        self.inner.presence_relay.as_ref()?;
        if config.presence_bridge != Some(true) {
            return None;
        }
        let provider = self.inner.extra_env_provider.as_ref()?;
        presence_relay::target_of(provider().as_ref())
    }

    /// `startPresenceRelay(gameId, config, target, gameChild)`: помощник стартует только после
    /// игры (первая программа префикса поднимает его общие процессы в своём контейнере); игру
    /// мост не задерживает (`client.dll` повторяет подключение каждые 5 с). Сбой моста игру не
    /// останавливает.
    pub async fn start_presence_relay(
        &self,
        game_id: &str,
        config: &LaunchConfig,
        target: &PresenceTarget,
        game_child: &Child,
    ) -> bool {
        let process_name = process_name_of(config.executable);
        let deadline = Instant::now() + self.inner.presence_start_wait;
        while !game_child.has_exited() && !(self.inner.process_running)(&process_name) {
            if Instant::now() >= deadline {
                self.inner.logger.warn(&format!(
                    "[launch] мост статуса для Discord не запущен: {process_name} не появился за {} с",
                    js::math_round(self.inner.presence_start_wait.as_millis() as f64 / 1000.0)
                ));
                return false;
            }
            tokio::select! {
                () = tokio::time::sleep(self.inner.bxt_poll) => {}
                () = exited(game_child) => {}
            }
        }
        if !game_child.has_exited() && !self.inner.presence_settle.is_zero() {
            tokio::select! {
                () = tokio::time::sleep(self.inner.presence_settle) => {}
                () = exited(game_child) => {}
            }
        }
        if game_child.has_exited() {
            return false;
        }
        let Some(relay) = self.inner.presence_relay.clone() else {
            return false;
        };
        let prepared = match relay.prepare() {
            Ok(prepared) => prepared,
            Err(error) => {
                self.inner.logger.warn(&format!(
                    "[launch] мост статуса для Discord не подготовлен: {}",
                    error.message
                ));
                return false;
            }
        };
        let args = presence_relay::relay_arguments(
            target,
            &process_name,
            &windows_path_of(&prepared.status.to_string_lossy()),
        );
        let Ok(runner) = self.runner() else {
            return false;
        };
        let mut env = Env::new();
        env.insert("PROTON_HIDE_PROCESS_WINDOW".to_owned(), "1".to_owned());
        let options = LaunchOptions {
            cwd: Some(prepared.dir.clone()),
            skip_fonts: true,
            env,
            ..LaunchOptions::default()
        };
        let child = match runner
            .launch(game_id, &prepared.executable, &args, options)
            .await
        {
            Ok(launched) => launched.child,
            Err(error) => {
                self.inner.logger.warn(&format!(
                    "[launch] мост статуса для Discord не запущен: {}",
                    error.message()
                ));
                return false;
            }
        };
        let kill = self.inner.kill_process_group.clone();
        let stop = {
            let child = child.clone();
            let kill = kill.clone();
            move || {
                if !child.has_exited()
                    && let Some(pid) = child.pid()
                {
                    kill(pid);
                }
            }
        };
        if game_child.has_exited() {
            stop();
        } else {
            let stop = stop.clone();
            let game = game_child.clone();
            tokio::spawn(async move {
                exited(&game).await;
                stop();
            });
        }
        let session = child.clone();
        let cancelled = move || session.has_exited();
        let code = relay
            .wait_for_status(
                &prepared.status,
                self.inner.presence_wait,
                &WaitOptions {
                    interval: None,
                    cancelled: Some(&cancelled),
                },
            )
            .await;
        if code == Some(0) {
            self.inner
                .logger
                .info("[launch] мост статуса для Discord готов");
            return true;
        }
        let code = code.map_or_else(|| "нет ответа".to_owned(), |code| format!("код {code}"));
        self.inner.logger.warn(&format!(
            "[launch] мост статуса для Discord не готов ({code}), игра идёт без него"
        ));
        stop();
        false
    }

    /// `patchRInputMenu(processName, gameChild)`: когда RInput поставит свои перехваты в
    /// `hl.exe`, они патчатся в памяти (курсор в меню). Сбой патча игру не трогает.
    pub async fn patch_rinput_menu(&self, process_name: &str, game_child: &Child) {
        let deadline = Instant::now() + self.inner.rinput_patch_wait;
        while !game_child.has_exited() {
            let status = match &self.inner.rinput_menu_patcher {
                Some(patch) => patch(process_name).await,
                None => Err("патч курсора RInput не подключён".to_owned()),
            };
            let status = match status {
                Ok(status) => status,
                Err(message) => {
                    self.inner.logger.warn(&format!(
                        "[launch] курсор RInput в меню не исправлен: {message}"
                    ));
                    return;
                }
            };
            if status == "patched" || status == "already" {
                self.inner
                    .logger
                    .info("[launch] курсор RInput в меню исправлен");
                return;
            }
            if status == "unsupported" {
                self.inner
                    .logger
                    .warn("[launch] курсор RInput в меню не исправлен: незнакомая версия RInput");
                return;
            }
            if Instant::now() >= deadline {
                self.inner.logger.warn(&format!(
                    "[launch] курсор RInput в меню не исправлен: RInput не перехватил курсор за {} с",
                    js::math_round(self.inner.rinput_patch_wait.as_millis() as f64 / 1000.0)
                ));
                return;
            }
            tokio::select! {
                () = tokio::time::sleep(self.inner.bxt_poll) => {}
                () = exited(game_child) => {}
            }
        }
    }

    /// `startSource(gameId, installed, config, settings)`: язык, подготовка файла игрока, мост
    /// presence, мост к Steam, журнал запуска.
    pub async fn start_source(
        &self,
        game_id: &str,
        install_path: &Path,
        config: &'static LaunchConfig,
        settings: Option<Value>,
    ) -> Result<Started, Failure> {
        self.start_source_episode(
            game_id,
            install_path,
            config,
            config,
            None,
            &Value::Null,
            settings,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_source_episode(
        &self,
        game_id: &str,
        install_path: &Path,
        base: &'static LaunchConfig,
        config: &LaunchConfig,
        episode: Option<&str>,
        installed: &Value,
        settings: Option<Value>,
    ) -> Result<Started, Failure> {
        let (executable, _) = self.files_for(install_path, config)?;
        let (proton_env, wrapper, proton_args) = Self::proton_options_of(settings.as_ref());
        let trusted: Vec<String> = config
            .arguments
            .iter()
            .map(|arg| (*arg).to_owned())
            .collect();
        let null = Value::Null;
        let mut argv =
            launch_args::merge_launch_arguments(&trusted, settings.as_ref().unwrap_or(&null));
        argv.extend(proton_args);
        let empty = json!({});
        let language = launch_args::language_of(settings.as_ref().unwrap_or(&empty));
        // Под Proton языка Steam в реестре нет: движок берёт язык по умолчанию.
        // Языковая подмена — в ghostingmod/custom: эпизоды монтируют ../ghostingmod/custom/*.
        let language_options = game_language::ApplyOptions {
            engine_language: hlsr_delivery::cfg_search::LanguageOption::Detect,
            engine: game_language::EngineOptions {
                platform: Platform::Linux,
                default_language: None,
                registry: &SystemRegistry,
            },
        };
        if let Err(error) = game_language::apply(
            install_path,
            base.game_directory,
            Some(&language),
            &language_options,
        ) {
            self.inner
                .logger
                .warn(&format!("[launch] язык не применён: {}", error.message));
        }
        // Файл игрока один — в ghostingmod/cfg; в эпизод его копирует sync_config_in ниже.
        self.prepare_configs(game_id, install_path, base, settings.as_ref());
        if let Some(episode) = episode {
            let stock = self.ghosting_stock_of(game_id, installed);
            episodes::sync_config_in(install_path, episode, stock.as_deref())?;
        }
        let presence = self.presence_target(config);
        let mut env = proton_env;
        if let Some(presence) = &presence {
            env.extend(presence.game_env.clone());
        }
        let mut options = LaunchOptions {
            cwd: Some(install_path.to_path_buf()),
            env,
            wrapper,
            exclude_cpu0: flag(settings.as_ref(), "excludeCpu0"),
            log_file: self.game_log_file(game_id),
            ..LaunchOptions::default()
        };
        options.steam_bridge = self.steam_bridge_of(&install_path.to_string_lossy(), config)?;
        Self::apply_overlay(
            &mut options,
            self.prepare_overlay(game_id, install_path, config, settings.as_ref())
                .as_ref(),
        );
        let launch = self
            .runner()?
            .launch(game_id, &executable, &argv, options)
            .await
            .map_err(Failure::from)?;
        if let Some(presence) = presence {
            let this = self.clone();
            let game = game_id.to_owned();
            let config = config.clone();
            let child = launch.child.clone();
            let ready = tokio::spawn(async move {
                this.start_presence_relay(&game, &config, &presence, &child)
                    .await
            });
            *lock(&self.inner.presence_ready) = Some(ready);
        }

        Ok(Started {
            launch,
            executable_path: executable,
            dependencies: None,
            packs: None,
        })
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
                self.inner.logger.warn(&format!(
                    "[launch] список cfg сборки Ghosting недоступен: {}",
                    error.message
                ));
                None
            }
        }
    }

    /// `prepareConfigs(gameId, installed, config, settings)`: файл игрока на запуске не
    /// переписывается — только переход с 0.4.8 или создание отсутствующего.
    fn prepare_configs(
        &self,
        game_id: &str,
        install_path: &Path,
        config: &LaunchConfig,
        settings: Option<&Value>,
    ) {
        crate::launch_service::prepare_configs(
            &self.inner.cfg_search,
            self.inner.logger.as_ref(),
            game_id,
            install_path,
            config,
            settings,
            &self.inner.launcher_version,
        );
    }

    /// `describeAvailability(gameId)`: `{ok, code}` — настроен ли Proton и найден ли umu.
    pub fn describe_availability(&self, game_id: &str) -> Value {
        if self.config_for(game_id).is_err() {
            return json!({"ok": false, "code": "LAUNCH_CONFIG_INVALID"});
        }
        let Some(runner) = self
            .inner
            .runner
            .as_ref()
            .filter(|runner| runner.selected().is_some())
        else {
            return json!({"ok": false, "code": "PROTON_NOT_INSTALLED"});
        };
        if runner.umu_path().is_none() {
            return json!({"ok": false, "code": "UMU_NOT_FOUND"});
        }
        json!({"ok": true, "code": null})
    }
}
