//! Платформа этой ОС для фасада: Steam, депо, служба процессов, инструменты, служба доставки и
//! запуск — как их создавал `ensureService` 0.4.7 (`SteamLocator`/`SteamAccount` Windows или
//! Linux, `SteamDepotService`, `GameDeliveryService`, `ToolService`, `PracticeSaves`,
//! `LaunchService` с `goldsrc-dependencies`, паками и персонализацией GoldSrc).

use super::{
    DeliveryPorts, GameLauncher, LaunchContext, PlatformFactory, ProtonPorts, SteamLocatorRef,
    SteamParts, SteamPort,
};
use crate::depot_install::{SteamDepotBridge, SystemRegistryWriter};
use crate::game_process_service::{self, GameProcessService};
use crate::goldsrc_launch::GoldSrcLaunch;
use crate::launch_service::{DependencyUpdater, LaunchServiceOptions, PackApplier};
use crate::steam_account::{AccountOptions, SteamAccount};
use crate::steam_depot::{DepotOptions, DepotSteam, SteamDepotService};
use crate::steam_locator::{LocatorOptions, SteamLocating, SteamLocator};
use crate::tool_runner::{PrefixLauncher, ToolProcessRunner, ToolRunnerOptions};
use hlsr_core::js::Platform;
use hlsr_delivery::api::{Parts, ServiceRequest};
use hlsr_delivery::cfg_search::{CfgSearch, SteamLanguage};
use hlsr_delivery::download_rate_limiter::DownloadRateLimiter;
use hlsr_delivery::environment::Environment;
use hlsr_delivery::errors::{DeliveryError, Failure};
use hlsr_delivery::game_delivery_service::{GameDeliveryService, ServiceOptions};
use hlsr_delivery::goldsrc_dependencies::{self, UpdateOptions};
use hlsr_delivery::goldsrc_packs::{self, PackOptions};
use hlsr_delivery::manifests::LaunchConfig;
use hlsr_delivery::ports::{Logger, Notifier, Registry, SteamDepot};
use hlsr_delivery::practice_saves::PracticeSaves;
use hlsr_delivery::providers::BoxFuture;
use hlsr_delivery::tool_service::{ToolOptions, ToolService};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Параметры платформы этой ОС (то, что `GameDelivery` получал от `AppManager`).
#[derive(Clone)]
pub struct SystemOptions {
    pub logger: Arc<dyn Logger>,
    pub launcher_version: String,
    pub env: Environment,
    pub registry: Arc<dyn Registry>,
    /// `versionEvents` службы доставки.
    pub notifier: Option<Arc<dyn Notifier>>,
    /// Общий лимит скорости загрузок.
    pub rate_limiter: Option<Arc<DownloadRateLimiter>>,
    /// Папка журналов лаунчера: туда же пишется лог Bunnymod XT при запуске HL1.
    pub logs_dir: Option<PathBuf>,
    /// Папка данных: мост присутствия под Proton.
    pub user_data_path: Option<PathBuf>,
    /// Linux: Proton для игр и инструментов.
    pub proton: Option<ProtonPorts>,
}

/// Платформа этой ОС.
pub struct SystemFactory {
    options: SystemOptions,
}

impl SystemFactory {
    pub fn new(options: SystemOptions) -> SystemFactory {
        SystemFactory { options }
    }
}

// ---------------------------------------------------------------- Steam

/// Steam Windows: поиск через реестр, аккаунт `ActiveUser`.
struct WindowsSteam {
    locator: Arc<SteamLocator>,
    account: SteamAccount,
}

impl SteamPort for WindowsSteam {
    fn refresh_connection(&self) -> String {
        self.account.refresh_connection()
    }

    fn describe(&self) -> Value {
        self.account.describe().to_value()
    }

    fn install_path(&self) -> Option<String> {
        self.locator
            .locate_steam()
            .ok()
            .map(|steam| steam.install_path)
    }

    fn active_account_id(&self) -> Option<u64> {
        self.account.active_account_id()
    }

    fn describe_steam_only(&self) -> Value {
        SteamLocating::describe_steam_only(self.locator.as_ref())
    }
}

/// Steam Linux: домашний каталог и `/proc`, аккаунт `MostRecent`.
struct LinuxSteam {
    locator: Arc<crate::linux::steam_locator::SteamLocator>,
    account: crate::linux::steam_account::SteamAccount,
}

impl SteamPort for LinuxSteam {
    fn refresh_connection(&self) -> String {
        self.account.refresh_connection()
    }

    fn describe(&self) -> Value {
        self.account.describe().to_value()
    }

    fn install_path(&self) -> Option<String> {
        self.locator
            .locate_steam()
            .ok()
            .map(|steam| steam.install_path)
    }

    fn active_account_id(&self) -> Option<u64> {
        self.account
            .active_account_id()
            .and_then(|id| u64::try_from(id).ok())
    }

    fn describe_steam_only(&self) -> Value {
        SteamLocating::describe_steam_only(self.locator.as_ref())
    }
}

// ---------------------------------------------------------------- запуск

struct WindowsLauncher(crate::launch_service::LaunchService);

impl GameLauncher for WindowsLauncher {
    fn launch<'a>(
        &'a self,
        game_id: &'a str,
        episode: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Value, DeliveryError>> {
        Box::pin(self.0.launch_episode(game_id, episode))
    }

    fn is_launching(&self) -> bool {
        self.0.is_launching()
    }

    fn describe_availability(&self, game_id: &str) -> Value {
        self.0.describe_availability(game_id)
    }

    fn dep_crash_hint(&self, exit_code: Option<i64>) -> Option<String> {
        self.0.dep_crash_hint(exit_code).map(str::to_owned)
    }
}

struct LinuxLauncher(crate::linux::launch_service::LaunchService);

impl GameLauncher for LinuxLauncher {
    fn launch<'a>(
        &'a self,
        game_id: &'a str,
        episode: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Value, DeliveryError>> {
        Box::pin(self.0.launch_episode(game_id, episode))
    }

    fn is_launching(&self) -> bool {
        self.0.is_launching()
    }

    fn describe_availability(&self, game_id: &str) -> Value {
        self.0.describe_availability(game_id)
    }
}

/// `dependencyUpdater(installPath, config)`: Bunnymod XT и инжектор с GitHub перед запуском HL1.
fn dependency_updater(
    service: Arc<GameDeliveryService>,
    logger: Arc<dyn Logger>,
) -> DependencyUpdater {
    Arc::new(move |install: &Path, config: &'static LaunchConfig| {
        let (service, logger, install) = (service.clone(), logger.clone(), install.to_path_buf());
        Box::pin(async move {
            let options = UpdateOptions {
                archive: Some(service.archive_service()),
                logger: Some(logger.as_ref()),
                ..UpdateOptions::default()
            };
            match goldsrc_dependencies::update_dependencies(&install, config.dependencies, &options)
                .await
            {
                Ok(results) => {
                    Value::Array(results.iter().map(|result| result.to_value()).collect())
                }
                Err(error) => {
                    logger.warn(&format!(
                        "[launch] зависимости не обновлены: {} {}",
                        error.code.as_str(),
                        error.message
                    ));
                    json!([])
                }
            }
        })
    })
}

/// `packApplier(installPath, config, settings)`: паки HUD и прицелов. Настройки BXT 0.4.9 живут в
/// файле игрока — его готовит `prepare_configs` запуска, здесь cfg не пишутся.
fn pack_applier(service: Arc<GameDeliveryService>, logger: Arc<dyn Logger>) -> PackApplier {
    Arc::new(
        move |install: &Path, config: &'static LaunchConfig, settings: &Value| {
            let (service, logger) = (service.clone(), logger.clone());
            let (install, settings) = (install.to_path_buf(), settings.clone());
            Box::pin(async move {
                let options = PackOptions {
                    archive: Some(service.archive_service()),
                    logger: Some(logger.as_ref()),
                    ..PackOptions::default()
                };
                match goldsrc_packs::apply_packs(&install, config, &settings, &options).await {
                    Ok(outcome) => outcome.to_value(),
                    Err(failure) => {
                        logger.warn(&format!(
                            "[launch] паки не применены: {}",
                            hlsr_delivery::errors::to_delivery_error(failure, None).message
                        ));
                        Value::Null
                    }
                }
            })
        },
    )
}

/// Патч курсора RInput через `/proc/<pid>/mem` (Linux).
#[cfg(target_os = "linux")]
fn linux_rinput_patcher() -> Option<crate::linux::launch_service::RInputMenuPatcher> {
    Some(Arc::new(|process_name: &str| {
        let process_name = process_name.to_owned();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                crate::linux::rinput_menu_fix::patch_running_game(
                    &process_name,
                    &crate::linux::rinput_menu_fix::SystemProcFs,
                )
                .map(|status| status.as_str().to_owned())
            })
            .await
            .map_err(|error| error.to_string())?
        })
    }))
}

#[cfg(not(target_os = "linux"))]
fn linux_rinput_patcher() -> Option<crate::linux::launch_service::RInputMenuPatcher> {
    None
}

impl PlatformFactory for SystemFactory {
    fn steam(&self) -> SteamParts {
        let logger = self.options.logger.clone();
        if Platform::CURRENT == Platform::Linux {
            let locator = Arc::new(crate::linux::steam_locator::SteamLocator::new(
                crate::linux::steam_locator::LocatorOptions {
                    logger: Some(logger.clone()),
                    ..Default::default()
                },
            ));
            let account = crate::linux::steam_account::SteamAccount::new(
                locator.clone() as Arc<dyn SteamLocating>,
                None,
            );
            let depot = SteamDepotService::new(DepotOptions {
                locator: Some(locator.clone() as Arc<dyn DepotSteam>),
                logger: Some(logger),
                ..DepotOptions::default()
            });
            return SteamParts {
                account: Some(Arc::new(LinuxSteam {
                    locator: locator.clone(),
                    account,
                })),
                depot: Some(Arc::new(depot)),
                locator: SteamLocatorRef::Linux(locator),
            };
        }
        let locator = Arc::new(SteamLocator::new(LocatorOptions {
            registry: Some(self.options.registry.clone()),
            logger: Some(logger.clone()),
            ..LocatorOptions::default()
        }));
        let account = SteamAccount::new(
            locator.clone() as Arc<dyn SteamLocating>,
            AccountOptions {
                registry: Some(self.options.registry.clone()),
                logger: Some(logger.clone()),
                ..AccountOptions::default()
            },
        );
        let depot = SteamDepotService::new(DepotOptions {
            locator: Some(locator.clone() as Arc<dyn DepotSteam>),
            logger: Some(logger),
            ..DepotOptions::default()
        });
        SteamParts {
            account: Some(Arc::new(WindowsSteam {
                locator: locator.clone(),
                account,
            })),
            depot: Some(Arc::new(depot)),
            locator: SteamLocatorRef::Windows(locator),
        }
    }

    fn processes(&self, state_dir: &Path) -> GameProcessService {
        GameProcessService::new(game_process_service::ServiceOptions {
            state_dir: state_dir.to_path_buf(),
            logger: Some(self.options.logger.clone()),
            ..game_process_service::ServiceOptions::default()
        })
    }

    fn tool_runner(&self) -> ToolProcessRunner {
        ToolProcessRunner::new(ToolRunnerOptions {
            logger: Some(self.options.logger.clone()),
            proton: self
                .options
                .proton
                .as_ref()
                .map(|proton| Arc::new(proton.runner.clone()) as Arc<dyn PrefixLauncher>),
            ..ToolRunnerOptions::default()
        })
    }

    fn delivery(&self, request: ServiceRequest, ports: DeliveryPorts) -> Result<Parts, Failure> {
        let logger = self.options.logger.clone();
        let depot = ports.steam.depot.clone().map(|service| {
            Arc::new(SteamDepotBridge::new(
                service,
                Some(logger.clone()),
                Some(Arc::new(SystemRegistryWriter)),
            )) as Arc<dyn SteamDepot>
        });
        let (probe, stopper) = ports.process_probe();
        let service = GameDeliveryService::new(ServiceOptions {
            library_path: request.library_path,
            logger: Some(logger.clone()),
            env: Some(self.options.env.clone()),
            launcher_version: Some(self.options.launcher_version.clone()),
            registry: request.registry,
            depot,
            processes: Some(probe),
            process_stopper: Some(stopper),
            notifier: self.options.notifier.clone(),
            release_notes: request.release_notes,
            rate_limiter: self.options.rate_limiter.clone(),
            ..ServiceOptions::default()
        })?;
        let tools = ToolService::new(
            service.paths().clone(),
            ToolOptions {
                archive: Some(service.archive_service().clone()),
                logger: Some(logger.clone()),
                rate_limiter: self.options.rate_limiter.clone(),
                runner: Some(ports.tool_runner),
                ..ToolOptions::default()
            },
        );
        let mut saves = PracticeSaves::new(service.paths())
            .with_archive(service.archive_service().clone())
            .with_logger(logger);
        if let Some(limiter) = &self.options.rate_limiter {
            saves = saves.with_rate_limiter(limiter.clone());
        }
        Ok(Parts {
            service,
            tools: Some(Arc::new(tools)),
            practice_saves: Some(Arc::new(saves)),
        })
    }

    fn launcher(&self, context: LaunchContext) -> Arc<dyn GameLauncher> {
        let logger = self.options.logger.clone();
        let search = Arc::new(CfgSearch::new(SteamLanguage::new(
            self.options.registry.clone(),
        )));
        let settings_store = context
            .settings_store
            .clone()
            .map(|store| store as Arc<dyn crate::launch_service::LaunchSettings>);
        let processes =
            Arc::new(context.processes.clone()) as Arc<dyn crate::launch_service::LaunchProcesses>;
        let updater = dependency_updater(context.service.clone(), logger.clone());
        let packs = pack_applier(context.service.clone(), logger.clone());
        match &context.steam.locator {
            SteamLocatorRef::Linux(locator) => {
                let options = crate::linux::launch_service::LaunchServiceOptions {
                    delivery: Some(context.delivery.clone()),
                    processes: Some(processes),
                    runner: self.options.proton.as_ref().map(|proton| {
                        Arc::new(proton.runner.clone())
                            as Arc<dyn crate::linux::proton_runner::ProtonLauncher>
                    }),
                    logger: Some(logger),
                    settings_store,
                    logs_dir: self.options.logs_dir.clone(),
                    dependency_updater: Some(updater),
                    pack_applier: Some(packs),
                    launcher_version: Some(self.options.launcher_version.clone()),
                    steam_locator: Some(
                        locator.clone() as Arc<dyn crate::linux::launch_service::LaunchSteam>
                    ),
                    goldsrc: Some(Arc::new(GoldSrcLaunch)),
                    rinput_menu_patcher: linux_rinput_patcher(),
                    live_split_overlay: context.overlay.clone(),
                    presence_relay: Some(Arc::new(
                        crate::linux::presence_relay::PresenceRelay::new(
                            self.options.user_data_path.clone(),
                            None,
                        ),
                    )),
                    extra_env_provider: Some(context.extra_env.clone()),
                    ..Default::default()
                };
                Arc::new(LinuxLauncher(
                    crate::linux::launch_service::LaunchService::new(options),
                ))
            }
            locator => {
                let depot = context.steam.depot.clone().map(|service| {
                    Arc::new(SteamDepotBridge::new(
                        service,
                        Some(logger.clone()),
                        Some(Arc::new(SystemRegistryWriter)),
                    )) as Arc<dyn SteamDepot>
                });
                let steam_locator = match locator {
                    SteamLocatorRef::Windows(locator) => {
                        Some(locator.clone() as Arc<dyn crate::launch_service::LaunchLocator>)
                    }
                    _ => None,
                };
                let options = LaunchServiceOptions {
                    delivery: Some(context.delivery.clone()),
                    processes: Some(processes),
                    logger: Some(logger),
                    settings_store,
                    logs_dir: self.options.logs_dir.clone(),
                    dependency_updater: Some(updater),
                    pack_applier: Some(packs),
                    extra_env_provider: Some(context.extra_env.clone()),
                    launcher_version: Some(self.options.launcher_version.clone()),
                    steam_locator,
                    depot_service: depot,
                    live_split_overlay: context.overlay.clone(),
                    registry: Some(self.options.registry.clone()),
                    cfg_search: Some(search),
                    ..LaunchServiceOptions::default()
                };
                Arc::new(WindowsLauncher(crate::launch_service::LaunchService::new(
                    options,
                )))
            }
        }
    }
}
