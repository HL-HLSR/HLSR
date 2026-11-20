//! Службы обновления на настоящей ОС: лента и канал по HTTPS (allowlist, rustls), загрузка с
//! докачкой, установщик и внешний апдейтер ([`ProcessLauncher`]), `InstallLocation` в реестре
//! ([`Registry`]), AppImage и архивная установка на Linux, проверки диагностики по сети — то, что
//! 0.4.7 брала у Electron, Node и electron-updater. Окно ОС, выход и перезапуск приложения,
//! уведомления, телеметрию и события версий подставляет приложение (`hlsr-app`, порты ПП4).
//!
//! Подмена ленты, канала и ключей (QA-сборка, `HLSR_UPDATE_FEED`/`HLSR_UPDATE_CHANNEL_KEY`, локальный
//! канал Linux) действует только в отладочной сборке и с `dev-hooks`.

use crate::channel;
use crate::chunks::{self, AllowUrl, HttpsFetch};
use crate::diagnostics::context::DiagnosticsContext;
use crate::diagnostics::net::{ReqwestRequest, SystemDns};
use crate::diagnostics::runner::DiagnosticsRunner;
use crate::gate::{GateOptions, LauncherUpdateGate};
use crate::http::ReqwestGet;
use crate::linux::archive_download::{ArchiveDownload, Rewrite};
use crate::linux::archive_install;
use crate::linux::update_manager::{
    AppImageExtractor, ArchiveInstaller, GateAdapter, HttpAppImageDownload, HttpChunkManifests,
    LinuxGate, LinuxManagerConfig, LinuxManagerPorts, LinuxUpdateManager, Relauncher,
    VersionInstaller,
};
use crate::linux::{EnvVars, SystemRunner};
use crate::ports::{Clock, HttpGet, Logger, SystemClock};
use crate::update_manager::{
    AppControl, GatePort, HttpFeed, HttpInstallerDownload, LauncherVersionEvents, ManagerConfig,
    ManagerPorts, ProcessSetupRunner, QaGate, RegistryInstallLocation, StagedExternalUpdater,
    UpdateManager, UpdateNotifier, UpdateTelemetry,
};
use crate::updater::download::SystemFreeSpace;
use crate::updater::processes::ProcessLauncher;
use crate::windows::registry::Registry;
use hlsr_core::js::Platform;
use std::path::PathBuf;
use std::sync::Arc;

/// `Math.random()`: равномерно в `[0, 1)`.
pub fn random_f64() -> f64 {
    let mut bytes = [0u8; 8];
    if getrandom::fill(&mut bytes).is_err() {
        return 0.5;
    }
    // 53 бита мантиссы.
    (u64::from_le_bytes(bytes) >> 11) as f64 / (1u64 << 53) as f64
}

/// Канал сборки (`HLSR_BUILD_CHANNEL`): у 0.4.7 webpack вписывал его при сборке, здесь — `cargo
/// build` с этой переменной (QA-сборка, `qa-launcher-build.js`); в отладочной сборке и с `dev-hooks`
/// годится и переменная окружения запуска. По умолчанию — `development`.
pub fn build_channel() -> String {
    if let Some(channel) = option_env!("HLSR_BUILD_CHANNEL").filter(|value| !value.is_empty()) {
        return channel.to_owned();
    }
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    if let Some(channel) = std::env::var(crate::local_qa::BUILD_CHANNEL_VAR)
        .ok()
        .filter(|value| !value.is_empty())
    {
        return channel;
    }
    "development".to_owned()
}

/// Сборка: что 0.4.7 брала у `app` и `process.env`.
#[derive(Debug, Clone)]
pub struct BuildInfo {
    /// `app.getVersion()`.
    pub current_version: String,
    /// `app.isPackaged`: сборка выпуска (не отладочная).
    pub packaged: bool,
    /// `NODE_ENV === "production"`: выпуск без `dev-hooks`. Сборка с `dev-hooks` обновляется только
    /// по локальной QA-ленте — смоук и запуск разработчика не ходят на настоящий канал.
    pub production: bool,
    /// [`build_channel`].
    pub build_channel: String,
}

impl BuildInfo {
    /// Эта сборка.
    pub fn current(current_version: impl Into<String>) -> BuildInfo {
        BuildInfo {
            current_version: current_version.into(),
            packaged: cfg!(not(debug_assertions)),
            production: cfg!(not(any(debug_assertions, feature = "dev-hooks"))),
            build_channel: build_channel(),
        }
    }
}

/// Порты приложения, общие для обеих ОС.
#[derive(Clone)]
pub struct ServiceLinks {
    /// `app.quit()`.
    pub app: Arc<dyn AppControl>,
    pub logger: Arc<dyn Logger>,
    pub notifier: Option<Arc<dyn UpdateNotifier>>,
    pub telemetry: Option<Arc<dyn UpdateTelemetry>>,
    pub version_events: Option<Arc<dyn LauncherVersionEvents>>,
}

/// Служба обновления Windows на настоящей ОС.
pub struct WindowsServiceOptions {
    pub build: BuildInfo,
    /// `app.getPath("exe")`.
    pub exe: PathBuf,
    /// `resources` рядом с exe (`QA_TRUST` для копии апдейтера).
    pub resources: PathBuf,
    /// `app.getPath("userData")`.
    pub user_data_dir: PathBuf,
    /// `%TEMP%`: копии внешнего апдейтера.
    pub temp_dir: PathBuf,
    /// Куда путь А качает установщик (`%LOCALAPPDATA%\<имя>-updater\pending`, как electron-updater:
    /// `hlsr-updater`, у QA-сборки `hlsr-qa-updater`).
    pub download_dir: PathBuf,
    pub registry: Arc<dyn Registry>,
    pub launcher: Arc<dyn ProcessLauncher>,
    /// Окружение установщика и апдейтера (без переменных окна, `PORTABLE_EXECUTABLE_*`, AppImage).
    pub env: Vec<(String, String)>,
    pub links: ServiceLinks,
}

/// HTTP ленты и шлюза: в отладочной сборке и с `dev-hooks` — `qaHttpGet` (loopback для QA-ленты,
/// остальное — боевой клиент), иначе — боевой клиент.
fn feed_http() -> Arc<dyn HttpGet> {
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    {
        Arc::new(crate::http::QaHttpGet::new())
    }
    #[cfg(not(any(debug_assertions, feature = "dev-hooks")))]
    {
        Arc::new(ReqwestGet::production())
    }
}

/// `process.env.<name>`: только подмены отладочной сборки и `dev-hooks` (их служба и так не берёт
/// в выпуске). Своя копия, не `hlsr-app` и не `hlsr-core`: `cfg!` читает признак `dev-hooks` того
/// крейта, где написан, — адрес канала обновлений (`HLSR_UPDATE_FEED`) открывает только признак
/// самого `hlsr-update`.
fn dev_env(name: &str) -> Option<String> {
    if cfg!(any(debug_assertions, feature = "dev-hooks")) {
        std::env::var(name).ok().filter(|value| !value.is_empty())
    } else {
        None
    }
}

/// `new UpdateManager(window)` (Windows).
pub fn windows_service(options: WindowsServiceOptions) -> UpdateManager {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let links = options.links;
    let gate_logger = links.logger.clone();
    let current = options.build.current_version.clone();
    let make_gate = Box::new(move |qa: Option<&QaGate>| -> Arc<dyn GatePort> {
        let mut gate = GateOptions::new(current.clone(), Arc::new(ReqwestGet::production()))
            .for_platform(Platform::Win32);
        gate.logger = gate_logger.clone();
        if let Some(qa) = qa {
            gate.http_get = feed_http();
            gate.channel_urls = qa.channel_urls.clone();
            gate.latest_yml_urls = qa.latest_yml_urls.clone();
            if let Some(keys) = &qa.public_keys {
                gate.public_keys = keys.clone();
            }
        }
        Arc::new(LauncherUpdateGate::new(gate))
    });
    let config = ManagerConfig {
        current_version: options.build.current_version.clone(),
        packaged: options.build.packaged,
        production: options.build.production,
        build_channel: options.build.build_channel.clone(),
        update_feed: dev_env("HLSR_UPDATE_FEED"),
        channel_key: dev_env("HLSR_UPDATE_CHANNEL_KEY"),
        exe_path: options.exe.to_string_lossy().into_owned(),
        user_data_dir: options.user_data_dir.to_string_lossy().into_owned(),
    };
    let ports = ManagerPorts {
        feed: Arc::new(HttpFeed::new(feed_http(), clock.clone())),
        make_gate,
        download: Arc::new(HttpInstallerDownload {
            dir: options.download_dir,
            free_space: Arc::new(SystemFreeSpace),
            clock: clock.clone(),
        }),
        setup: Arc::new(ProcessSetupRunner {
            launcher: options.launcher.clone(),
            env: options.env.clone(),
        }),
        external: Arc::new(StagedExternalUpdater {
            app_exe: options.exe,
            resources: options.resources,
            temp_root: options.temp_dir,
            launcher: options.launcher,
            env: options.env,
            clock: clock.clone(),
            logger: links.logger.clone(),
        }),
        install_location: Arc::new(RegistryInstallLocation {
            registry: options.registry,
            logger: links.logger.clone(),
        }),
        app: links.app,
        logger: links.logger,
        clock,
        random: Arc::new(random_f64),
        window: None,
        notifier: links.notifier,
        telemetry: links.telemetry,
        version_events: links.version_events,
    };
    UpdateManager::new(config, ports)
}

/// Служба обновления Linux на настоящей ОС.
pub struct LinuxServiceOptions {
    pub build: BuildInfo,
    /// `process.execPath`: корень управляемой установки — `managedRoot` от него.
    pub exe: PathBuf,
    /// `process.argv` (нулевой — сам бинарник): аргументы перезапуска.
    pub argv: Vec<String>,
    /// Окружение рантайма AppImage (без переменных окна).
    pub env: EnvVars,
    pub relauncher: Arc<dyn Relauncher>,
    pub links: ServiceLinks,
}

/// `new UpdateManager(window, options)` (Linux).
pub fn linux_service(options: LinuxServiceOptions) -> LinuxUpdateManager {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let links = options.links;
    let logger = links.logger.clone();
    let install_root = archive_install::managed_root(&options.exe);
    let allow: AllowUrl = chunks::production_allow();
    let fetch: Arc<dyn chunks::ChunkFetch> = Arc::new(HttpsFetch::new());

    let production_gate = || -> Arc<dyn LinuxGate> {
        let mut gate = GateOptions::new(
            options.build.current_version.clone(),
            Arc::new(ReqwestGet::production()),
        )
        .for_platform(Platform::Linux);
        gate.logger = logger.clone();
        Arc::new(GateAdapter(Arc::new(LauncherUpdateGate::new(gate))))
    };
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    let (gate, rewrite): (Arc<dyn LinuxGate>, Option<Rewrite>) =
        match crate::linux::update_manager::qa_overrides(
            &options.build.current_version,
            logger.clone(),
        ) {
            Some((gate, rewrite)) => (gate, Some(rewrite)),
            None => (production_gate(), None),
        };
    #[cfg(not(any(debug_assertions, feature = "dev-hooks")))]
    let (gate, rewrite): (Arc<dyn LinuxGate>, Option<Rewrite>) = (production_gate(), None);

    let create_installer = {
        let root = install_root.clone().unwrap_or_default();
        let (fetch, allow, logger, clock) =
            (fetch.clone(), allow.clone(), logger.clone(), clock.clone());
        Arc::new(move || -> Arc<dyn VersionInstaller> {
            Arc::new(ArchiveInstaller::new(
                &root,
                fetch.clone(),
                allow.clone(),
                logger.clone(),
                clock.clone(),
            ))
        })
    };
    let config = LinuxManagerConfig {
        current_version: options.build.current_version.clone(),
        packaged: options.build.packaged,
        install_root,
        argv: options.argv,
    };
    let ports = LinuxManagerPorts {
        gate,
        download: Arc::new(HttpAppImageDownload {
            client: ArchiveDownload::new(),
            rewrite,
            is_allowed: allow.clone(),
            clock: clock.clone(),
        }),
        extract: Arc::new(AppImageExtractor {
            runner: Arc::new(SystemRunner),
            env: options.env,
        }),
        chunk_manifests: Arc::new(HttpChunkManifests {
            fetch,
            is_allowed: allow,
        }),
        create_installer,
        app: links.app,
        relauncher: options.relauncher,
        logger: links.logger,
        clock,
        random: Arc::new(random_f64),
        window: None,
        telemetry: links.telemetry,
        version_events: links.version_events,
    };
    LinuxUpdateManager::new(config, ports)
}

/// `new DiagnosticsRunner({context})` с настоящей сетью и DNS: боевые адреса и ключи канала.
pub fn diagnostics_runner(context: Arc<dyn DiagnosticsContext>) -> DiagnosticsRunner {
    DiagnosticsRunner::new(
        context,
        Arc::new(ReqwestRequest::new()),
        Arc::new(SystemDns),
        Arc::new(SystemClock),
    )
    .with_launcher_public_keys(channel::default_public_keys())
}
