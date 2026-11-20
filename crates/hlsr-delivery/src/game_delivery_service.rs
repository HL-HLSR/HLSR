//! Оркестратор доставки игр (`GameDeliveryService.js`): статус каждой игры, установка,
//! установка из депо Steam, проверка и ремонт файлов, обновления (патчи, полная сборка, сборка из
//! источника), восстановление после сбоя, удаление, экспорт и импорт `.hlsrpack`. Загрузка,
//! установка, патчи, проверка и ремонт — свои сервисы; здесь — порядок их вызовов, слот
//! операции, статусы и событие `status`.
//!
//! Операции (`install`, `verify_files`, `repair`, `apply_update`, `uninstall_game`,
//! `export_pack`, `import_pack`) асинхронные. Их будущее нельзя бросать посреди работы: блоки
//! `finally` 0.4.7 выполняются после `await` (снять пометку операции, вернуть статус). Отмена —
//! [`GameDeliveryService::cancel`] и [`GameDeliveryService::pause`].

use crate::archive_service::{
    ArchiveService, CancelHandle, CreateOptions, ExtractOptions, FractionFn, InspectArgs, PercentFn,
};
use crate::build_metadata_trust::{
    self as trust, Reason as TrustReason, State as TrustState, VerifyAgainstSnapshot,
    VerifyForInstall,
};
use crate::channel_service::{ChannelOptions, ChannelService};
use crate::download_rate_limiter::DownloadRateLimiter;
use crate::environment::Environment;
use crate::errors::{Code, DeliveryError, Failure, RawError, to_delivery_error};
use crate::feature_flag;
use crate::files_manifest::{self, ValidateOptions};
use crate::files_manifest_store::{self, FilesManifestStore};
use crate::game_download_manager::{
    DownloadEvent, DownloadOptions, DownloadOutcome, DownloadRequest, GameDownloadManager,
};
use crate::game_pack::{self, PackMeta};
use crate::game_removal;
use crate::http_client::{HttpClient, RequestOptions, read_body};
use crate::install_service::{
    FaultInjector, FullUpdate, InstallEvent, InstallOptions, InstallRequest, InstallService,
    InstallTarget, IntegrityCheck, IntegrityRequest, OrphanOptions, Recovery,
};
use crate::integrity_service::{IntegrityOptions, IntegrityResult, IntegrityService};
use crate::js_value as jv;
use crate::main_button;
use crate::manifest_service::{InstallableOptions, LoadedManifest, ManifestService};
use crate::manifests;
use crate::mirror_runner::{AttemptRecord, MirrorRequest, MirrorRunner};
use crate::origin_updates::{OriginOptions, OriginUpdates};
use crate::patch_chain::{self, PlanKind, PlanOptions, UpdatePlan};
use crate::patch_service::{PatchEvent, PatchOptions, PatchRequest, PatchService};
use crate::paths::{
    LibraryPaths, assert_inside, ensure_layout, game_install_path, remove_within_zone,
    resolve_library,
};
use crate::ports::{
    DepotProgress, DepotProgressFn, FreeSpace, Logger, NoGameProcesses, Notifier, ProcessProbe,
    ProcessStopper, SteamDepot, SystemFreeSpace, TracingLogger, VersionEventInfo,
};
use crate::providers::{self, ProviderOptions};
use crate::release_notes::ReleaseNotesStore;
use crate::repair_mirror::{RepairMirrorClient, RepairMirrorOptions};
use crate::repair_service::{RepairEvent, RepairOptions, RepairService};
use crate::timing::{Clock, iso_from_ms, system_clock};
use crate::unavailable_reasons;
use crate::update_checker::{
    CheckOptions, CheckerOptions, UpdateChecker, UpdateEvent, UpdateResult,
};
use crate::update_target;
use hlsr_core::js;
use hlsr_core::js::path::NodePath;
use hlsr_core::json_store::read_json_safe;
use hlsr_core::json_text::{js_number, json_parse, stringify};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// Потоки доставки 0.4.9 — свои `impl GameDeliveryService` в своих файлах (каркас T0):
// ремонт по файлам (B), сборки .hlsrpack (C), эпизоды Ghosting (E), обновления и доверие (G).
mod episodes;
mod pack;
mod repair;
mod update;

/// Хук `renderActive(gameId)`: идёт ли рендер демо этой игры (ставит фасад, инструмент «Демо»).
pub type RenderActive = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// `STATES`: состояния статуса доставки.
pub const STATES: [&str; 18] = [
    "not-installed",
    "preparing",
    "downloading",
    "paused",
    "verifying",
    "extracting",
    "finalizing",
    "applying-update",
    "checking-files",
    "files-valid",
    "files-damaged",
    "repairing",
    "repair-complete",
    "exporting",
    "importing",
    "ready",
    "cancelled",
    "error",
];

/// Проверка обновлений при фокусе окна — не чаще раза в 5 минут (`FOCUS_CHECK_AGE_MS`).
pub const FOCUS_CHECK_AGE_MS: i64 = 5 * 60 * 1000;

/// Запас места при обновлении полной сборкой (`checkFullUpdateSpace`).
pub const FULL_UPDATE_SPACE_MARGIN: u64 = 512 * 1024 * 1024;

/// Манифест патча скачивается не больше 8 МБ.
pub const PATCH_MANIFEST_LIMIT: u64 = 8 * 1024 * 1024;

/// Сведения из `.hlsrpack` читаются не больше 4 МБ.
pub const PACK_META_LIMIT: u64 = 4 * 1024 * 1024;

/// Слушатель события `status`.
pub type Listener = Arc<dyn Fn(&Value) + Send + Sync>;

// ---------------------------------------------------------------- реестр и параметры

/// Манифесты, которыми пользуется сервис (`options.manifests`, `options.filesManifests`): игра →
/// манифест; игра → (версия → файловый манифест). Порядок — как у ключей объекта 0.4.7.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    pub manifests: Vec<(String, Value)>,
    pub files: Vec<(String, Vec<(String, Value)>)>,
}

impl Registry {
    /// Встроенные манифесты (`MANIFESTS`, `FILES_MANIFESTS`).
    pub fn bundled() -> Registry {
        Registry {
            manifests: manifests::manifests()
                .into_iter()
                .map(|(id, manifest)| (id.to_owned(), manifest.clone()))
                .collect(),
            files: manifests::game_ids()
                .into_iter()
                .map(|id| {
                    let versions = manifests::files_manifests_of(id)
                        .iter()
                        .map(|(version, embedded)| {
                            ((*version).to_owned(), embedded.value().clone())
                        })
                        .collect();
                    (id.to_owned(), versions)
                })
                .collect(),
        }
    }

    fn manifest(&self, game_id: &str) -> Option<&Value> {
        self.manifests
            .iter()
            .find(|(id, _)| id == game_id)
            .map(|(_, manifest)| manifest)
    }

    fn files_of(&self, game_id: &str) -> Option<&Vec<(String, Value)>> {
        self.files
            .iter()
            .find(|(id, _)| id == game_id)
            .map(|(_, versions)| versions)
    }
}

/// Сервис по умолчанию, выключен (`null` в 0.4.7) или задан.
#[derive(Default)]
pub enum Provided<T> {
    #[default]
    Default,
    Off,
    Given(Arc<T>),
}

impl<T> Clone for Provided<T> {
    fn clone(&self) -> Self {
        match self {
            Provided::Default => Provided::Default,
            Provided::Off => Provided::Off,
            Provided::Given(service) => Provided::Given(service.clone()),
        }
    }
}

/// Параметры конструктора (`options`).
#[derive(Clone, Default)]
pub struct ServiceOptions {
    /// `libraryPath`.
    pub library_path: String,
    pub logger: Option<Arc<dyn Logger>>,
    /// Переменные окружения; по умолчанию — процесса.
    pub env: Option<Environment>,
    /// `launcherVersion`: пустая — `0.0.0`.
    pub launcher_version: Option<String>,
    /// `manifests` и `filesManifests`; по умолчанию — встроенные.
    pub registry: Option<Registry>,
    /// `depotService`: нет — проверка лицензий и депо недоступны.
    pub depot: Option<Arc<dyn SteamDepot>>,
    /// Процессы игр; по умолчанию — ни одна игра не запущена.
    pub processes: Option<Arc<dyn ProcessProbe>>,
    pub process_stopper: Option<Arc<dyn ProcessStopper>>,
    pub channel_service: Provided<ChannelService>,
    pub origin_updates: Provided<OriginUpdates>,
    /// `versionEvents`: нет — события версий не создаются (и заметки к ним не сохраняются).
    pub notifier: Option<Arc<dyn Notifier>>,
    pub release_notes: Option<Arc<ReleaseNotesStore>>,
    pub http: Option<HttpClient>,
    pub archive_service: Option<ArchiveService>,
    /// `timeout`, `maxAttempts`, `backoffBaseMs` загрузки.
    pub timeout: Option<Duration>,
    pub max_attempts: Option<u32>,
    pub backoff_base_ms: Option<u64>,
    /// `freeSpaceProvider` загрузки (установка и патчи спрашивают ОС, как в 0.4.7); им же —
    /// место для обновления полной сборкой (`fs.statfsSync` в 0.4.7), чтобы его можно было
    /// подменить в тестах.
    pub free_space: Option<Arc<dyn FreeSpace>>,
    pub rate_limiter: Option<Arc<DownloadRateLimiter>>,
    /// `updateCheckIntervalMs`.
    pub update_check_interval_ms: Option<i64>,
    /// Часы (`Date.now()`, `new Date()`) — для всех сервисов; для тестов.
    pub clock: Option<Clock>,
    /// Точки отказа установки (`UPDATE_FAULTS`); для тестов.
    pub install_fault_injector: Option<FaultInjector>,
}

// ---------------------------------------------------------------- состояние

/// Слот операции: игра, операция и жетон, которым его освобождают.
struct Slot {
    token: u64,
    game_id: String,
    operation: &'static str,
}

/// Отмена идущего экспорта или импорта сборки (`packCancel`).
#[derive(Clone)]
enum PackCancel {
    Archive(CancelHandle),
    Integrity,
}

#[derive(Default)]
struct State {
    statuses: HashMap<String, Map<String, Value>>,
    active_game_id: Option<String>,
    slot: Option<Slot>,
    last_integrity: HashMap<String, IntegrityResult>,
    pack_cancel: Option<PackCancel>,
    silent_version_events: HashSet<String>,
    operations: HashMap<String, String>,
    published_at_backfilled: HashSet<String>,
    last_mirror_attempts: Vec<AttemptRecord>,
    /// Отмена загрузки файлов с зеркала ремонта (`repairAbort`, `fetchFromMirror`).
    repair_abort: Option<CancelHandle>,
    /// Последние принятые манифесты эпизодов (`episodeManifests`): id → `{manifest, url, fetchedAt}`.
    episode_manifests: HashMap<String, Value>,
    /// Отказы манифестов эпизодов (`episodeManifestFailures`): id → ошибка и когда (мс).
    episode_manifest_failures: HashMap<String, (DeliveryError, f64)>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Занятый слот: освобождается при выходе из операции, если его никто не перехватил.
struct SlotGuard<'a> {
    service: &'a GameDeliveryService,
    token: u64,
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        let mut state = lock(&self.service.state);
        if state
            .slot
            .as_ref()
            .is_some_and(|slot| slot.token == self.token)
        {
            state.slot = None;
        }
    }
}

/// `GameDeliveryService`.
pub struct GameDeliveryService {
    me: Weak<GameDeliveryService>,
    logger: Arc<dyn Logger>,
    env: Environment,
    launcher_version: String,
    paths: LibraryPaths,
    registry: Arc<Registry>,
    depot: Option<Arc<dyn SteamDepot>>,
    files_store: FilesManifestStore,
    manifest_service: ManifestService,
    mirrors: MirrorRunner,
    archive_service: ArchiveService,
    download_manager: GameDownloadManager,
    install_service: Arc<InstallService>,
    patch_service: PatchService,
    integrity_service: Arc<IntegrityService>,
    repair_service: RepairService,
    repair_mirror: RepairMirrorClient,
    processes: Arc<dyn ProcessProbe>,
    process_stopper: Arc<dyn ProcessStopper>,
    channel_service: Option<Arc<ChannelService>>,
    update_checker: UpdateChecker,
    notifier: Option<Arc<dyn Notifier>>,
    release_notes: Option<Arc<ReleaseNotesStore>>,
    http: HttpClient,
    /// Свободное место для `checkFullUpdateSpace` (`fs.statfsSync` в 0.4.7).
    free_space: Arc<dyn FreeSpace>,
    clock: Clock,
    state: Mutex<State>,
    next_token: AtomicU64,
    listeners: Mutex<Vec<Listener>>,
    /// `renderActive` (ставит фасад): нет — рендера нет.
    render_active: Mutex<Option<RenderActive>>,
}

/// `manifestFor(gameId)` над реестром: встроенный манифест игры, проверенный схемой.
fn manifest_from(
    registry: &Registry,
    service: &ManifestService,
    game_id: &str,
) -> Result<LoadedManifest, DeliveryError> {
    let Some(manifest) = registry.manifest(game_id) else {
        return Err(DeliveryError::new(
            Code::ManifestNotFound,
            format!("Нет манифеста для {game_id}"),
        ));
    };
    service.validate(manifest, Some(&format!("bundled:{game_id}")))
}

/// `x || null`.
fn or_null(value: Option<&Value>) -> Value {
    value
        .filter(|value| js::truthy(value))
        .cloned()
        .unwrap_or(Value::Null)
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn native() -> NodePath {
    NodePath::native()
}

/// `path.join(…)` на этой ОС.
fn join(parts: &[&str]) -> PathBuf {
    PathBuf::from(native().join(parts))
}

/// `path.basename(name)` на этой ОС.
fn basename(name: &str) -> String {
    native().basename(name)
}

/// `fs.existsSync(path)`.
fn exists(path: &Path) -> bool {
    std::fs::metadata(path).is_ok()
}

/// Ошибка как `Error` в catch: `DeliveryError` или текст чужой ошибки.
fn message_of(failure: &Failure) -> String {
    match failure {
        Failure::Delivery(error) => error.message.clone(),
        Failure::Raw(error) => error.message.clone(),
    }
}

/// `error.code` ошибки (у чужой — её код `fs` или нет).
fn code_of(failure: &Failure) -> Option<String> {
    match failure {
        Failure::Delivery(error) => Some(error.code.as_str().to_owned()),
        Failure::Raw(error) => error.code.clone(),
    }
}

impl GameDeliveryService {
    /// `new GameDeliveryService(options)`. Путь библиотеки недопустим — `PATH_INVALID`.
    pub fn new(options: ServiceOptions) -> Result<Arc<GameDeliveryService>, DeliveryError> {
        let logger: Arc<dyn Logger> = options
            .logger
            .clone()
            .unwrap_or_else(|| Arc::new(TracingLogger));
        let env = options.env.clone().unwrap_or_else(Environment::process);
        let launcher_version = options
            .launcher_version
            .clone()
            .filter(|version| !version.is_empty())
            .unwrap_or_else(|| "0.0.0".to_owned());
        let paths = resolve_library(&options.library_path)?;
        let registry = Arc::new(options.registry.clone().unwrap_or_else(Registry::bundled));
        let clock = options.clock.clone().unwrap_or_else(system_clock);
        let http = options.http.clone().unwrap_or_default();

        let files_store = FilesManifestStore::new(
            &paths,
            files_manifest_store::StoreOptions {
                logger: Some(logger.clone()),
                env: Some(env.clone()),
                http: Some(http.clone()),
                clock: Some(clock.clone()),
                ..files_manifest_store::StoreOptions::default()
            },
        );
        let manifest_service = ManifestService::new(Some(&launcher_version));
        let mirrors = MirrorRunner::with_clock(logger.clone(), clock.clone());
        let archive_service = options
            .archive_service
            .clone()
            .unwrap_or_else(|| ArchiveService::with_default_bin(logger.clone()));

        let download_manager = GameDownloadManager::new(
            paths.downloads.clone(),
            DownloadOptions {
                http: Some(http.clone()),
                logger: Some(logger.clone()),
                timeout: options.timeout,
                max_attempts: options.max_attempts,
                backoff_base_ms: options.backoff_base_ms,
                free_space: options.free_space.clone(),
                rate_limiter: options.rate_limiter.clone(),
                clock: Some(clock.clone()),
                ..DownloadOptions::default()
            },
        );
        let install_service = Arc::new(InstallService::new(
            paths.clone(),
            InstallOptions {
                archive_service: Some(archive_service.clone()),
                logger: Some(logger.clone()),
                launcher_version: Some(launcher_version.clone()),
                fault_injector: options.install_fault_injector.clone(),
                clock: Some(clock.clone()),
                ..InstallOptions::default()
            },
        ));
        let patch_service = PatchService::new(
            paths.clone(),
            PatchOptions {
                archive_service: Some(archive_service.clone()),
                logger: Some(logger.clone()),
                launcher_version: Some(launcher_version.clone()),
                clock: Some(clock.clone()),
                ..PatchOptions::default()
            },
        );
        let integrity_service = Arc::new(IntegrityService::new(IntegrityOptions {
            logger: Some(logger.clone()),
        }));
        let repair_service = RepairService::new(
            paths.clone(),
            RepairOptions {
                logger: Some(logger.clone()),
                clock: Some(clock.clone()),
                ..RepairOptions::default()
            },
        );
        let repair_mirror = RepairMirrorClient::new(RepairMirrorOptions {
            http: Some(http.clone()),
            rate_limiter: options.rate_limiter.clone(),
            ..RepairMirrorOptions::default()
        });
        let no_processes = Arc::new(NoGameProcesses);
        let processes = options
            .processes
            .clone()
            .unwrap_or_else(|| no_processes.clone() as Arc<dyn ProcessProbe>);
        let process_stopper = options
            .process_stopper
            .clone()
            .unwrap_or(no_processes as Arc<dyn ProcessStopper>);

        let channel_service = match &options.channel_service {
            Provided::Off => None,
            Provided::Given(service) => Some(service.clone()),
            Provided::Default => Some(Arc::new(ChannelService::new(ChannelOptions {
                logger: Some(logger.clone()),
                env: Some(env.clone()),
                http: Some(http.clone()),
                clock: Some(clock.clone()),
                ..ChannelOptions::default()
            }))),
        };
        let origin_updates = match &options.origin_updates {
            Provided::Off => None,
            Provided::Given(service) => Some(service.clone()),
            Provided::Default => Some(Arc::new(OriginUpdates::new(OriginOptions {
                logger: Some(logger.clone()),
                http: Some(http.clone()),
                origins: None,
            }))),
        };

        let installed_of = install_service.clone();
        let manifests_of = registry.clone();
        let manifest_check = manifest_service.clone();
        let update_checker = UpdateChecker::new(CheckerOptions {
            logger: Some(logger.clone()),
            channel_service: channel_service.clone(),
            origin_updates: origin_updates.clone(),
            installed_version_of: Some(Arc::new(move |game_id: &str| {
                installed_version(&installed_of, game_id)
            })),
            local_manifest_of: Some(Arc::new(move |game_id: &str| {
                manifest_from(&manifests_of, &manifest_check, game_id)
                    .ok()
                    .map(|loaded| loaded.manifest)
            })),
            launcher_version: Some(launcher_version.clone()),
            min_interval_ms: options.update_check_interval_ms,
            clock: Some(clock.clone()),
        });

        let service = Arc::new_cyclic(|me| GameDeliveryService {
            me: me.clone(),
            logger,
            env,
            launcher_version,
            paths,
            registry,
            depot: options.depot.clone(),
            files_store,
            manifest_service,
            mirrors,
            archive_service,
            download_manager,
            install_service,
            patch_service,
            integrity_service,
            repair_service,
            repair_mirror,
            processes,
            process_stopper,
            channel_service,
            update_checker,
            notifier: options.notifier.clone(),
            release_notes: options.release_notes.clone(),
            http,
            free_space: options
                .free_space
                .clone()
                .unwrap_or_else(|| Arc::new(SystemFreeSpace)),
            clock,
            state: Mutex::new(State::default()),
            next_token: AtomicU64::new(1),
            listeners: Mutex::new(Vec::new()),
            render_active: Mutex::new(None),
        });
        service.subscribe();
        Ok(service)
    }

    /// Подписки конструктора: события сервисов становятся статусами.
    fn subscribe(&self) {
        let me = self.me.clone();
        self.update_checker.on_event(move |event| {
            let Some(service) = me.upgrade() else {
                return;
            };
            match event {
                UpdateEvent::Checking { game_id, .. } => {
                    let _ = service.refresh(game_id);
                }
                UpdateEvent::Result(result) => {
                    let _ = service.refresh(result.game_id());
                    service.announce_update_result(result);
                }
            }
        });

        let me = self.me.clone();
        self.download_manager.on_event(move |event| {
            if let Some(service) = me.upgrade() {
                match event {
                    DownloadEvent::Progress(progress) => {
                        service.on_download_progress(&progress.to_json())
                    }
                    DownloadEvent::State(state) => service.on_download_state(&state.to_json()),
                }
            }
        });
        let me = self.me.clone();
        self.install_service.on_event(move |event: &InstallEvent| {
            if let Some(service) = me.upgrade() {
                service.on_install_state(&event.to_json());
            }
        });
        let me = self.me.clone();
        self.patch_service.on_event(move |event: &PatchEvent| {
            if let Some(service) = me.upgrade() {
                service.on_patch_state(&event.to_json());
            }
        });
        let me = self.me.clone();
        self.integrity_service.on_progress(move |event| {
            if let Some(service) = me.upgrade() {
                service.on_integrity_progress(&event.to_json());
            }
        });
        let me = self.me.clone();
        self.repair_service.on_event(move |event: &RepairEvent| {
            if let Some(service) = me.upgrade() {
                match event {
                    RepairEvent::State { .. } => service.on_repair_state(&event.to_json()),
                    RepairEvent::Progress { .. } => service.on_integrity_progress(&event.to_json()),
                }
            }
        });
    }

    /// `on("status", listener)`: слушатель зовётся сразу, в той же точке кода, где меняется статус.
    pub fn on_status(&self, listener: impl Fn(&Value) + Send + Sync + 'static) {
        lock(&self.listeners).push(Arc::new(listener));
    }

    fn emit(&self, status: &Value) {
        let listeners: Vec<Listener> = lock(&self.listeners).clone();
        for listener in listeners {
            listener(status);
        }
    }

    // ------------------------------------------------------------ доступ к частям

    /// `paths`.
    pub fn paths(&self) -> &LibraryPaths {
        &self.paths
    }

    pub fn install_service(&self) -> &InstallService {
        &self.install_service
    }

    pub fn archive_service(&self) -> &ArchiveService {
        &self.archive_service
    }

    pub fn download_manager(&self) -> &GameDownloadManager {
        &self.download_manager
    }

    pub fn update_checker(&self) -> &UpdateChecker {
        &self.update_checker
    }

    pub fn files_store(&self) -> &FilesManifestStore {
        &self.files_store
    }

    pub fn patch_service(&self) -> &PatchService {
        &self.patch_service
    }

    pub fn repair_service(&self) -> &RepairService {
        &self.repair_service
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn launcher_version(&self) -> &str {
        &self.launcher_version
    }

    pub fn env(&self) -> &Environment {
        &self.env
    }

    fn now_ms(&self) -> i64 {
        (self.clock)()
    }

    fn now_iso(&self) -> String {
        iso_from_ms(self.now_ms())
    }

    fn system_now(&self) -> SystemTime {
        let ms = self.now_ms();
        if ms >= 0 {
            UNIX_EPOCH + Duration::from_millis(ms as u64)
        } else {
            UNIX_EPOCH - Duration::from_millis(ms.unsigned_abs())
        }
    }

    fn allowed_provider_types(&self) -> Vec<String> {
        feature_flag::allowed_provider_types(&self.env)
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    fn ensure_layout(&self) -> Result<(), DeliveryError> {
        ensure_layout(&self.paths, self.system_now())
    }

    // ------------------------------------------------------------ файловые манифесты

    /// `filesManifestFor(gameId, version)`: встроенный файловый манифест версии или из кеша.
    pub fn files_manifest_for(
        &self,
        game_id: &str,
        version: Option<&str>,
    ) -> Result<Value, DeliveryError> {
        let Some(version) = version.filter(|version| !version.is_empty()) else {
            return Err(DeliveryError::new(
                Code::FilesManifestMissing,
                format!(
                    "Не задана версия сборки {game_id} — неизвестно, каким манифестом проверять"
                ),
            )
            .with_details(json!({"gameId": game_id})));
        };

        if let Some(bundled) = self.bundled_files_manifest(game_id, version)? {
            return Ok(bundled);
        }

        if let Some(cached) = self.files_store.load(game_id, version) {
            return Ok(cached);
        }

        let bundled: Vec<&str> = self
            .registry
            .files_of(game_id)
            .map(|versions| {
                versions
                    .iter()
                    .map(|(version, _)| version.as_str())
                    .collect()
            })
            .unwrap_or_default();
        Err(DeliveryError::new(
            Code::FilesManifestMissing,
            format!("Нет файлового манифеста версии {version} для {game_id}"),
        )
        .with_details(json!({
            "gameId": game_id,
            "version": version,
            "bundled": bundled,
            "cached": self.files_store.list(game_id),
        })))
    }

    /// `bundledFilesManifest(gameId, version)`.
    fn bundled_files_manifest(
        &self,
        game_id: &str,
        version: &str,
    ) -> Result<Option<Value>, DeliveryError> {
        let Some(manifest) = self
            .registry
            .files_of(game_id)
            .and_then(|versions| versions.iter().find(|(v, _)| v == version))
            .map(|(_, manifest)| manifest)
            .filter(|manifest| js::truthy(manifest))
        else {
            return Ok(None);
        };
        files_manifest::validate(
            manifest,
            Some(&format!("bundled:{game_id}-{version}.files.json")),
            Some(&ValidateOptions {
                game_id: Some(game_id.to_owned()),
                version: Some(version.to_owned()),
                ..ValidateOptions::default()
            }),
        )?;
        Ok(Some(manifest.clone()))
    }

    /// `hasFilesManifest(gameId, version)`.
    pub fn has_files_manifest(&self, game_id: &str, version: Option<&str>) -> bool {
        self.files_manifest_for(game_id, version).is_ok()
    }

    /// `ensureFilesManifest(gameId, version, options)`: файловый манифест версии — из канала,
    /// если его ещё нет.
    pub async fn ensure_files_manifest(
        &self,
        game_id: &str,
        version: Option<&str>,
        force: bool,
        ignore_cache: bool,
    ) -> Result<Value, DeliveryError> {
        let Some(version) = version.filter(|version| !version.is_empty()) else {
            return Err(
                DeliveryError::new(Code::FilesManifestMissing, "Не задана версия сборки")
                    .with_details(json!({"gameId": game_id})),
            );
        };

        if !force && self.has_files_manifest(game_id, Some(version)) {
            return Ok(json!({"status": "already-available", "version": version}));
        }

        let Some(channel) = &self.channel_service else {
            return Err(
                DeliveryError::new(Code::ChannelDisabled, "Канал обновлений выключен")
                    .with_details(json!({"gameId": game_id})),
            );
        };

        let fetched = channel.fetch(game_id, ignore_cache).await?;
        let raw = jv::prop(&fetched.manifest, "filesManifest");
        let descriptor = raw.filter(|d| js::truthy(d));

        if descriptor.is_none_or(|descriptor| !jv::is_str(jv::prop(descriptor, "version"), version))
        {
            // `descriptor && descriptor.version`.
            let declared = match descriptor {
                Some(descriptor) => jv::template(jv::prop(descriptor, "version")),
                None => jv::template(raw),
            };
            return Err(DeliveryError::new(
                Code::FilesManifestMissing,
                format!("Канал объявляет файловый манифест версии {declared}, а нужен {version}"),
            )
            .with_details(json!({"gameId": game_id, "version": version})));
        }
        let descriptor = descriptor.expect("проверено выше");

        let manifest = self
            .files_store
            .fetch(
                game_id,
                descriptor,
                &files_manifest_store::FetchOptions {
                    channel_sha256: Some(ManifestService::hash(&fetched.manifest)),
                    source: None,
                },
            )
            .await?;

        Ok(json!({
            "status": "stored",
            "version": version,
            "totalFiles": manifest.get("totalFiles").cloned().unwrap_or(Value::Null),
        }))
    }

    /// `tryEnsureFilesManifest(gameId, version)`: неудача — в журнал и `{status: "failed"}`.
    pub async fn try_ensure_files_manifest(&self, game_id: &str, version: Option<&str>) -> Value {
        match self
            .ensure_files_manifest(game_id, version, false, false)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                let delivery = to_delivery_error(error.into(), Some(Code::FilesManifestMissing));
                self.logger.warn(&format!(
                    "[game-delivery] файловый манифест {game_id} {} не получен: {} {}",
                    version.unwrap_or("undefined"),
                    delivery.code,
                    delivery.message
                ));
                json!({
                    "status": "failed",
                    "version": version,
                    "error": delivery.to_public(),
                })
            }
        }
    }

    /// `resolveFilesManifest(gameId, version)`.
    pub async fn resolve_files_manifest(
        &self,
        game_id: &str,
        version: Option<&str>,
    ) -> Result<Value, DeliveryError> {
        if let Ok(manifest) = self.files_manifest_for(game_id, version) {
            return Ok(manifest);
        }
        self.try_ensure_files_manifest(game_id, version).await;
        self.files_manifest_for(game_id, version)
    }

    /// `installedVersionOf(gameId)`.
    pub fn installed_version_of(&self, game_id: &str) -> Option<String> {
        installed_version(&self.install_service, game_id)
    }

    fn read_installed(&self, game_id: &str) -> Result<Option<Value>, DeliveryError> {
        self.install_service.read_installed(game_id)
    }

    // ------------------------------------------------------------ метаданные сборки

    /// `buildDescriptorFor(gameId, version)`: описание метаданных сборки версии — из канала (если
    /// он объявляет ту же версию) или из встроенного манифеста.
    fn build_descriptor_for(&self, game_id: &str, version: Option<&str>) -> Option<(Value, Value)> {
        let bundled = self.manifest_for(game_id).ok()?.manifest;
        let target = version.filter(|version| !version.is_empty())?;

        let cached = self
            .channel_service
            .as_ref()
            .and_then(|channel| channel.cached_for(game_id));
        let channel = cached
            .as_ref()
            .map(|entry| &entry.manifest)
            .filter(|manifest| js::truthy(manifest));

        if let Some(channel) = channel {
            let meta = jv::prop(channel, "buildMetadata").filter(|meta| js::truthy(meta));
            if let Some(meta) = meta
                && jv::prop(meta, "buildId").is_some_and(|id| !id.is_null())
                && jv::prop(channel, "gameId") == jv::prop(&bundled, "gameId")
                && jv::template(jv::prop(channel, "version")) == target
            {
                let mut manifest = bundled.as_object().cloned().unwrap_or_default();
                manifest.insert(
                    "version".to_owned(),
                    or_undefined(jv::prop(channel, "version")),
                );
                let published = [
                    jv::prop(channel, "publishedAt"),
                    jv::prop(&bundled, "publishedAt"),
                ]
                .into_iter()
                .flatten()
                .find(|value| js::truthy(value))
                .cloned()
                .unwrap_or(Value::Null);
                manifest.insert("publishedAt".to_owned(), published);
                manifest.insert("buildMetadata".to_owned(), meta.clone());
                return Some((meta.clone(), Value::Object(manifest)));
            }
        }

        let meta = jv::prop(&bundled, "buildMetadata").filter(|meta| js::truthy(meta));
        if let Some(meta) = meta
            && jv::prop(meta, "buildId").is_some_and(|id| !id.is_null())
            && jv::is_str(jv::prop(&bundled, "version"), target)
        {
            return Some((meta.clone(), bundled.clone()));
        }

        None
    }

    /// `refreshBuildMetadataSnapshot(installed)`: снимок метаданных в `installed.json` отстал от
    /// установленной версии — записать новый. `installed` дополняется новыми полями.
    fn refresh_build_metadata_snapshot(
        &self,
        installed: &mut Value,
    ) -> Option<(TrustState, Option<TrustReason>)> {
        let game_id = jv::template(jv::prop(installed, "gameId"));
        let version = jv::prop(installed, "version").map(|v| jv::template(Some(v)));
        let (descriptor, manifest) = self.build_descriptor_for(&game_id, version.as_deref())?;

        let install_root = PathBuf::from(jv::template(jv::prop(installed, "installPath")));
        let verified = match trust::verify_for_install(&VerifyForInstall {
            install_root: &install_root,
            descriptor: &descriptor,
            manifest: Some(&manifest),
            files_manifest: None,
            launcher_version: Some(&self.launcher_version),
            expected: None,
            now: Some(self.now_ms() as f64),
        }) {
            Ok(verified) => verified,
            Err(error) => {
                self.logger.info(&format!(
                    "[game-delivery] снимок сведений о сборке {game_id} освежить не вышло ({}): {}",
                    error.code.as_str(),
                    error.message
                ));
                return None;
            }
        };

        let snapshot = trust::snapshot_from_verified(&verified).to_value();

        if let Err(error) = self
            .install_service
            .replace_build_metadata_snapshot(&game_id, &snapshot)
        {
            self.logger.warn(&format!(
                "[game-delivery] снимок сведений о сборке не записан: {}",
                message_of(&error)
            ));
            return None;
        }

        let meta = &snapshot["buildMetadata"];
        self.logger.info(&format!(
            "[game-delivery] снимок сведений о сборке {game_id} обновлён до {} ({}) — запись отставала от установки",
            jv::template(jv::prop(meta, "version")),
            jv::template(jv::prop(meta, "buildId"))
        ));

        installed["buildMetadata"] = snapshot["buildMetadata"].clone();
        installed["buildMetadataSha256"] = snapshot["buildMetadataSha256"].clone();
        installed["buildMetadataVerifiedAt"] = snapshot["buildMetadataVerifiedAt"].clone();

        Some((TrustState::Trusted, None))
    }

    /// `computeBuildMetadataView(installed)`: что показать о сборке и не нужен ли ремонт.
    fn compute_build_metadata_view(&self, installed: Option<&mut Value>) -> Map<String, Value> {
        let mut view = Map::new();
        let Some(installed) = installed else {
            view.insert("buildMetadata".to_owned(), Value::Null);
            view.insert("buildDate".to_owned(), Value::Null);
            view.insert("buildDateSource".to_owned(), Value::Null);
            view.insert("metadataState".to_owned(), json!("not-installed"));
            view.insert("metadataIssue".to_owned(), Value::Null);
            return view;
        };

        let mut snapshot_meta = jv::prop(installed, "buildMetadata")
            .filter(|meta| js::truthy(meta))
            .cloned();

        let mut build_date = Value::Null;
        let mut build_date_source = Value::Null;
        if let Some(built) = snapshot_meta
            .as_ref()
            .and_then(|meta| jv::prop(meta, "builtAt"))
            .filter(|built| js::truthy(built))
        {
            build_date = built.clone();
            build_date_source = json!("builtAt");
        } else if let Some(published) = jv::prop(installed, "publishedAt").filter(|p| js::truthy(p))
        {
            build_date = published.clone();
            build_date_source = json!("publishedAt");
        }

        let mut metadata_state = json!(if snapshot_meta.is_some() {
            "trusted"
        } else {
            "legacy"
        });
        let mut metadata_issue = Value::Null;

        if let Some(meta) = snapshot_meta.clone() {
            let install_root = PathBuf::from(jv::template(jv::prop(installed, "installPath")));
            let verdict = trust::verify_against_snapshot(&VerifyAgainstSnapshot {
                install_root: &install_root,
                snapshot: Some(installed),
                launcher_version: Some(&self.launcher_version),
                expected: None,
                now: Some(self.now_ms() as f64),
            });
            let mut state = verdict.state;
            let mut reason = verdict.reason;
            let mut detail = verdict.detail.clone();

            let meta_version = jv::prop(&meta, "version").filter(|v| js::truthy(v));
            let installed_version = jv::prop(installed, "version").filter(|v| js::truthy(v));
            if state == TrustState::RepairRequired
                && let (Some(meta_version), Some(installed_version)) =
                    (meta_version, installed_version)
                && meta_version != installed_version
                && let Some((refreshed_state, refreshed_reason)) =
                    self.refresh_build_metadata_snapshot(installed)
            {
                state = refreshed_state;
                reason = refreshed_reason;
                detail = None;
                snapshot_meta = jv::prop(installed, "buildMetadata")
                    .filter(|meta| js::truthy(meta))
                    .cloned()
                    .or(snapshot_meta);
                if let Some(built) = snapshot_meta
                    .as_ref()
                    .and_then(|meta| jv::prop(meta, "builtAt"))
                    .filter(|built| js::truthy(built))
                {
                    build_date = built.clone();
                    build_date_source = json!("builtAt");
                }
            }

            metadata_state = json!(state.as_str());
            if let Some(reason) = reason {
                metadata_issue = json!({
                    "code": reason.as_str(),
                    "detail": detail.filter(js::truthy).unwrap_or(Value::Null),
                });
            }
        }

        view.insert(
            "buildMetadata".to_owned(),
            snapshot_meta.as_ref().map_or(Value::Null, |meta| {
                let mut out = Map::new();
                for key in [
                    "version",
                    "buildId",
                    "buildName",
                    "gitCommit",
                    "builtAt",
                    "architecture",
                    "minimumLauncherVersion",
                ] {
                    if let Some(value) = jv::prop(meta, key) {
                        out.insert(key.to_owned(), value.clone());
                    }
                }
                Value::Object(out)
            }),
        );
        view.insert("buildDate".to_owned(), build_date);
        view.insert("buildDateSource".to_owned(), build_date_source);
        view.insert("metadataState".to_owned(), metadata_state);
        view.insert("metadataIssue".to_owned(), metadata_issue);
        view
    }

    /// `manifestFor(gameId)`: встроенный манифест игры, проверенный схемой.
    pub fn manifest_for(&self, game_id: &str) -> Result<LoadedManifest, DeliveryError> {
        manifest_from(&self.registry, &self.manifest_service, game_id)
    }

    // ------------------------------------------------------------ статусы

    /// `defaultStatus(gameId)`.
    pub fn default_status(&self, game_id: &str) -> Map<String, Value> {
        self.default_status_with(game_id, self.processes.process_state(game_id))
    }

    fn default_status_with(&self, game_id: &str, process: Value) -> Map<String, Value> {
        let value = json!({
            "gameId": game_id,
            "launcherId": manifests::launcher_id_of(game_id),
            // Окно «Почему недоступно» предлагает ремонт только таким играм.
            "fileRepair": manifests::is_file_repair_game(game_id),
            "state": "not-installed",
            "percent": 0,
            "bytesDownloaded": 0,
            "totalBytes": 0,
            "speedBps": 0,
            "etaSeconds": null,
            "version": null,
            "installedVersion": null,
            "installedAt": null,
            "buildMetadata": null,
            "buildDate": null,
            "buildDateSource": null,
            "metadataState": null,
            "metadataIssue": null,
            "installable": false,
            "reasons": [],
            "unavailableReason": null,
            "error": null,
            "pending": null,
            "phase": null,
            "checked": 0,
            "totalChecked": 0,
            "integrity": null,
            "updatePlan": null,
            "source": null,
            "operation": null,
            "chain": null,
            "process": process,
            "update": null,
            "mainButton": null,
        });
        match value {
            Value::Object(map) => map,
            _ => unreachable!("литерал объекта"),
        }
    }

    /// `statusOf(gameId)`: статус игры (умолчание запоминается).
    pub fn status_of(&self, game_id: &str) -> Value {
        if let Some(status) = lock(&self.state).statuses.get(game_id) {
            return Value::Object(status.clone());
        }
        let status = self.default_status(game_id);
        let mut state = lock(&self.state);
        let status = state.statuses.entry(game_id.to_owned()).or_insert(status);
        Value::Object(status.clone())
    }

    fn state_name_of(&self, game_id: &str) -> String {
        jv::template(self.status_of(game_id).get("state"))
    }

    /// `setStatus(gameId, patch)`: дополнить статус, пересчитать операцию, процесс, обновление,
    /// причину недоступности и главную кнопку; событие `status`.
    pub fn set_status(&self, game_id: &str, patch: Value) -> Value {
        // Процесс и проверка обновлений — до блокировки статусов: их службы не должны ждать её.
        let process = self.processes.process_state(game_id);
        let update = self.public_update(game_id);
        let checking_updates = self.update_checker.is_checking(game_id);
        let status = {
            let mut state = lock(&self.state);
            let mut status = match state.statuses.remove(game_id) {
                Some(status) => status,
                None => self.default_status_with(game_id, process.clone()),
            };
            if let Value::Object(patch) = patch {
                for (key, value) in patch {
                    status.insert(key, value);
                }
            }
            status.insert(
                "operation".to_owned(),
                state
                    .operations
                    .get(game_id)
                    .map_or(Value::Null, |operation| json!(operation)),
            );
            status.insert("process".to_owned(), process.clone());
            status.insert("update".to_owned(), update.clone());
            let as_value = Value::Object(status.clone());
            let reason = unavailable_reasons::from_delivery_status(Some(&as_value));
            status.insert("unavailableReason".to_owned(), reason);
            let as_value = Value::Object(status.clone());
            let button = main_button::resolve(&main_button::Input {
                delivery: Some(&as_value),
                process: Some(&process),
                update: Some(&update),
                checking_updates,
            });
            status.insert("mainButton".to_owned(), button);
            state.statuses.insert(game_id.to_owned(), status.clone());
            Value::Object(status)
        };
        self.emit(&status);
        status
    }

    /// `publicUpdate(gameId)`: итог последней проверки обновлений для статуса.
    pub fn public_update(&self, game_id: &str) -> Value {
        let Some(result) = self.update_checker.last_result(game_id) else {
            return Value::Null;
        };
        let get = |key: &str| result.get(key);
        let steps: Vec<Value> = get("steps")
            .and_then(Value::as_array)
            .map(|steps| {
                steps
                    .iter()
                    .map(|step| {
                        json!(format!(
                            "{} -> {}",
                            jv::template(jv::prop(step, "fromVersion")),
                            jv::template(jv::prop(step, "toVersion"))
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut out = Map::new();
        out.insert(
            "installedVersion".to_owned(),
            or_undefined(get("installedVersion")),
        );
        out.insert(
            "availableVersion".to_owned(),
            or_undefined(get("availableVersion")),
        );
        out.insert("upToDate".to_owned(), json!(result.flag("upToDate")));
        out.insert(
            "updateAvailable".to_owned(),
            json!(result.flag("updateAvailable")),
        );
        out.insert(
            "updateRequired".to_owned(),
            json!(result.flag("updateRequired")),
        );
        out.insert("kind".to_owned(), or_null(get("kind")));
        out.insert(
            "totalSize".to_owned(),
            get("totalSize")
                .filter(|size| js::truthy(size))
                .cloned()
                .unwrap_or(json!(0)),
        );
        out.insert("steps".to_owned(), Value::Array(steps));
        out.insert("source".to_owned(), or_null(get("source")));
        out.insert("checkedAt".to_owned(), or_null(get("checkedAt")));
        out.insert("stale".to_owned(), json!(result.flag("stale")));
        out.insert("unknown".to_owned(), json!(result.flag("unknown")));
        out.insert("error".to_owned(), or_null(get("error")));
        Value::Object(out)
    }

    /// `downloadWithFallback(request)`: загрузка через источники по порядку.
    async fn download_with_fallback(
        &self,
        game_id: &str,
        descriptors: &[Value],
        filename: &str,
        expected_size: Option<&Value>,
        expected_sha256: Option<&Value>,
        disabled_message: &str,
    ) -> Result<Value, Failure> {
        let allowed = self.allowed_provider_types();
        let usable: Vec<Value> = descriptors
            .iter()
            .filter(|descriptor| {
                descriptor
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| allowed.iter().any(|a| a == kind))
            })
            .cloned()
            .collect();

        if usable.is_empty() {
            return Err(DeliveryError::new(Code::RemoteDisabled, disabled_message).into());
        }

        let ordered = ManifestService::order_providers(&usable);
        let artifact = format!("{game_id}:{filename}");
        let is_terminal =
            |value: &DownloadOutcome| !matches!(value, DownloadOutcome::Completed { .. });
        let discard = |_descriptor: &Value, _code: Code| -> Result<(), Failure> {
            self.download_manager.discard(game_id, filename)
        };
        let provider_options =
            ProviderOptions::with_allowed_provider_types(self.http.clone(), allowed.clone());
        let expected_size = expected_size
            .and_then(jv::number)
            .filter(|size| *size > 0.0);
        let expected_sha256 = expected_sha256
            .filter(|sha| js::truthy(sha))
            .map(|sha| jv::template(Some(sha)));

        let outcome = self
            .mirrors
            .run(
                MirrorRequest {
                    artifact: &artifact,
                    descriptors: &ordered,
                    is_terminal_result: Some(&is_terminal),
                    on_discard_partial: Some(&discard),
                },
                |attempt| {
                    let provider_options = &provider_options;
                    let expected_sha256 = expected_sha256.clone();
                    async move {
                        let provider = providers::create(&attempt.descriptor, provider_options)?;
                        self.download_manager
                            .start(DownloadRequest {
                                game_id: game_id.to_owned(),
                                provider: Arc::from(provider),
                                filename: filename.to_owned(),
                                expected_size: expected_size.map(|size| size as u64),
                                expected_sha256,
                            })
                            .await
                    }
                },
            )
            .await;

        let outcome = outcome?;
        lock(&self.state).last_mirror_attempts = outcome.attempts.clone();
        if outcome.used_fallback {
            self.logger.warn(&format!(
                "[game-delivery] {filename}: использован резервный источник {}",
                outcome.provider_id.as_deref().unwrap_or("undefined")
            ));
        }

        Ok(outcome.value.to_json())
    }

    /// `mirrorDiagnostics()`: журнал попыток последней загрузки и паузы хостов.
    pub fn mirror_diagnostics(&self) -> Value {
        let attempts: Vec<Value> = lock(&self.state)
            .last_mirror_attempts
            .iter()
            .map(AttemptRecord::to_json)
            .collect();
        let cooldown: Map<String, Value> = self
            .mirrors
            .cooldown()
            .snapshot()
            .into_iter()
            .map(|(host, ms)| (host, json!(ms)))
            .collect();
        json!({"attempts": attempts, "cooldown": cooldown})
    }

    fn set_operation(&self, game_id: &str, operation: Option<&str>) {
        let mut state = lock(&self.state);
        match operation {
            Some(operation) => {
                state
                    .operations
                    .insert(game_id.to_owned(), operation.to_owned());
            }
            None => {
                state.operations.remove(game_id);
            }
        }
    }

    /// `silentVersionEvents.size`: установка «база + патчи» между шагами (слот свободен).
    pub fn has_silent_version_events(&self) -> bool {
        !lock(&self.state).silent_version_events.is_empty()
    }

    fn set_active(&self, game_id: Option<&str>) {
        lock(&self.state).active_game_id = game_id.map(str::to_owned);
    }

    fn active_game_id(&self) -> Option<String> {
        lock(&self.state).active_game_id.clone()
    }

    /// `onProcessState(event)`: процесс игры сменил состояние — статус пересчитывается.
    pub fn process_state_changed(&self, game_id: &str) {
        if !game_id.is_empty() {
            let _ = self.refresh(game_id);
        }
    }

    // ------------------------------------------------------------ события версий

    fn version_events_silenced(&self, game_id: &str) -> bool {
        lock(&self.state).silent_version_events.contains(game_id)
    }

    fn version_info(&self, game_id: &str, has_release_notes: bool) -> VersionEventInfo {
        VersionEventInfo {
            has_release_notes,
            launcher_id: manifests::launcher_id_of(game_id).map(str::to_owned),
        }
    }

    /// `resolveUpdateOffers(gameId, installedVersion)`.
    fn resolve_update_offers(&self, game_id: &str, installed_version: Option<&str>) {
        if let Some(notifier) = &self.notifier
            && let Err(message) = notifier.resolve_update_offers("game", installed_version, game_id)
        {
            self.logger.warn(&format!(
                "[game-delivery] приглашения не пересмотрены: {message}"
            ));
        }
    }

    fn capture_notes(&self, game_id: &str, version: &str, notes: &Value) -> Result<(), String> {
        if let Some(store) = &self.release_notes {
            store
                .capture(game_id, version, notes)
                .map_err(|error| error.message)?;
        }
        Ok(())
    }

    /// `announceUpdateResult(result)`: обновление доступно — заметка о выпуске и событие версии.
    fn announce_update_result(&self, result: &UpdateResult) {
        let Some(notifier) = self.notifier.clone() else {
            return;
        };
        let game_id = result.game_id();
        if game_id.is_empty() || self.version_events_silenced(game_id) {
            return;
        }
        if result.flag("stale") || result.flag("unknown") {
            return;
        }

        self.resolve_update_offers(game_id, result.installed_version());

        if !result.flag("updateAvailable") {
            return;
        }
        let Some(available) = result
            .available_version()
            .filter(|version| !version.is_empty())
        else {
            return;
        };
        if !result.flag("installed") {
            return;
        }

        let mut has_notes = false;
        if let Some(store) = &self.release_notes {
            let notes = result
                .target()
                .and_then(|target| jv::prop(target, "notes"))
                .filter(|notes| js::truthy(notes));
            let captured = match notes {
                Some(notes) => self.capture_notes(game_id, available, notes),
                None => Ok(()),
            };
            match captured {
                Ok(()) => has_notes = store.has(game_id, available),
                Err(message) => self
                    .logger
                    .warn(&format!("[game-delivery] патчноут не сохранён: {message}")),
            }
        }

        if let Err(message) = notifier.game_update_available(
            available,
            result.installed_version(),
            game_id,
            &self.version_info(game_id, has_notes),
        ) {
            self.logger.warn(&format!(
                "[game-delivery] событие версии не создано: {message}"
            ));
        }
    }

    /// `announceInstalled(gameId, installedVersion, manifestNotes)`.
    fn announce_installed(
        &self,
        game_id: &str,
        installed_version: Option<&str>,
        notes: Option<&Value>,
    ) {
        let Some(notifier) = self.notifier.clone() else {
            return;
        };
        let Some(installed_version) = installed_version.filter(|v| !v.is_empty()) else {
            return;
        };
        if self.version_events_silenced(game_id) {
            return;
        }
        if self.installed_version_of(game_id).as_deref() != Some(installed_version) {
            return;
        }

        let mut has_notes = false;
        if let Some(store) = &self.release_notes {
            let captured = match notes.filter(|notes| js::truthy(notes)) {
                Some(notes) => self.capture_notes(game_id, installed_version, notes),
                None => Ok(()),
            };
            match captured {
                Ok(()) => has_notes = store.has(game_id, installed_version),
                Err(message) => self
                    .logger
                    .warn(&format!("[game-delivery] патчноут не сохранён: {message}")),
            }
        }

        self.resolve_update_offers(game_id, Some(installed_version));

        if let Err(message) = notifier.game_installed(
            installed_version,
            game_id,
            &self.version_info(game_id, has_notes),
        ) {
            self.logger.warn(&format!(
                "[game-delivery] событие версии не создано: {message}"
            ));
        }
    }

    /// `announceUpdated(gameId, fromVersion, toVersion)`.
    fn announce_updated(
        &self,
        game_id: &str,
        from_version: Option<&str>,
        to_version: Option<&str>,
    ) {
        let Some(notifier) = self.notifier.clone() else {
            return;
        };
        let Some(to_version) = to_version.filter(|v| !v.is_empty()) else {
            return;
        };
        if self.version_events_silenced(game_id) {
            return;
        }
        if self.installed_version_of(game_id).as_deref() != Some(to_version) {
            return;
        }

        let has_notes = self
            .release_notes
            .as_ref()
            .is_some_and(|store| store.has(game_id, to_version));

        self.resolve_update_offers(game_id, Some(to_version));

        if let Err(message) = notifier.game_updated(
            from_version,
            to_version,
            game_id,
            &self.version_info(game_id, has_notes),
        ) {
            self.logger.warn(&format!(
                "[game-delivery] событие версии не создано: {message}"
            ));
        }
    }

    /// `ensureNotesForInstalled(gameId)`: заметка к установленной версии из встроенного манифеста.
    fn ensure_notes_for_installed(&self, game_id: &str) {
        if self.release_notes.is_none() {
            return;
        }
        let result: Result<(), String> = (|| {
            let Some(installed) = self.read_installed(game_id).map_err(|e| e.message)? else {
                return Ok(());
            };
            if !jv::is_str(jv::prop(&installed, "state"), "ready") {
                return Ok(());
            }
            let Some(version) = jv::prop(&installed, "version").filter(|v| js::truthy(v)) else {
                return Ok(());
            };
            let declared = self.manifest_for(game_id).map_err(|e| e.message)?.manifest;
            let notes = jv::prop(&declared, "notes").filter(|notes| js::truthy(notes));
            if jv::prop(&declared, "version") != Some(version) || notes.is_none() {
                return Ok(());
            }
            self.capture_notes(
                game_id,
                &jv::template(Some(version)),
                notes.expect("проверено"),
            )
        })();
        if let Err(message) = result {
            self.logger
                .warn(&format!("[release-notes] заметка не обновлена: {message}"));
        }
    }
}

/// `undefined` в объекте JS — отсутствие; здесь — `null` (так значение уходит в JSON окна).
fn or_undefined(value: Option<&Value>) -> Value {
    value.cloned().unwrap_or(Value::Null)
}

/// `installService.readInstalled(gameId)?.version`; ошибка чтения — как отсутствие.
fn installed_version(install_service: &InstallService, game_id: &str) -> Option<String> {
    install_service
        .read_installed(game_id)
        .ok()
        .flatten()
        .and_then(|installed| installed.get("version").cloned())
        .filter(|version| !version.is_null())
        .map(|version| jv::template(Some(&version)))
}

// ---------------------------------------------------------------- параметры операций

/// `repair(gameId, options)`: подтверждение, без повторной проверки (`recheck === false`),
/// выбор галочками (`files`; `None` — всё повреждённое, изменённые cfg не трогаются).
#[derive(Debug, Clone, Default)]
pub struct RepairCall {
    pub confirmed: bool,
    pub skip_recheck: bool,
    pub files: Option<Vec<String>>,
}

/// `extra` экспорта `.hlsrpack`: то, что знает фасад.
#[derive(Debug, Clone, Default)]
pub struct PackExtra {
    pub launcher_id: Option<Value>,
    pub build: Option<Value>,
    pub launcher_version: Option<Value>,
    pub settings: Option<Value>,
    /// `allowModified`: игрок подтвердил экспорт с изменёнными файлами сборки (`PACK_MODIFIED`).
    pub allow_modified: bool,
}

/// Идущая операция: игра активна, у неё пометка операции; при выходе из операции (`finally`)
/// обе снимаются.
struct Activity<'a> {
    service: &'a GameDeliveryService,
    game_id: String,
}

impl Drop for Activity<'_> {
    fn drop(&mut self) {
        self.service.set_active(None);
        self.service.set_operation(&self.game_id, None);
    }
}

/// Поле события как значение (`undefined` — `null`).
fn field(event: &Value, name: &str) -> Value {
    jv::prop(event, name).cloned().unwrap_or(Value::Null)
}

/// `x || null` над строкой: ложное значение — `None`.
fn truthy_text(value: Option<&Value>) -> Option<String> {
    value
        .filter(|value| js::truthy(value))
        .map(|value| jv::template(Some(value)))
}

/// `a === b` для значений JSON.
fn strict_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Value::Number(x)), Some(Value::Number(y))) => {
            jv::number(&Value::Number(x.clone())) == jv::number(&Value::Number(y.clone()))
        }
        (Some(Value::Object(_) | Value::Array(_)), _)
        | (_, Some(Value::Object(_) | Value::Array(_))) => false,
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Число JS (`undefined` — `NaN`).
fn number_of(value: Option<&Value>) -> f64 {
    value.and_then(jv::number).unwrap_or(f64::NAN)
}

impl GameDeliveryService {
    fn activate(&self, game_id: &str, operation: &str) -> Activity<'_> {
        self.set_active(Some(game_id));
        self.set_operation(game_id, Some(operation));
        Activity {
            service: self,
            game_id: game_id.to_owned(),
        }
    }

    fn installable_options(&self) -> InstallableOptions {
        InstallableOptions {
            launcher_version: Some(self.launcher_version.clone()),
            allowed_provider_types: self.allowed_provider_types(),
        }
    }

    /// Источники типов, разрешённых флагами.
    fn usable_providers(&self, providers: Option<&Value>) -> Vec<Value> {
        let allowed = self.allowed_provider_types();
        providers
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|provider| {
                jv::prop(provider, "type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| allowed.iter().any(|a| a == kind))
            })
            .cloned()
            .collect()
    }

    // ------------------------------------------------------------ обновление статусов

    /// `refresh(gameId)`: статус игры по манифесту, записи об установке, незаконченной загрузке и
    /// журналам операций.
    pub fn refresh(&self, game_id: &str) -> Result<Value, Failure> {
        if manifests::is_depot_build(game_id) {
            return self.refresh_depot(game_id);
        }

        self.ensure_notes_for_installed(game_id);

        let loaded = match self.manifest_for(game_id) {
            Ok(loaded) => loaded,
            Err(error) => {
                let delivery = to_delivery_error(error.into(), Some(Code::ManifestInvalid));
                return Ok(self.set_status(
                    game_id,
                    json!({"state": "error", "error": delivery.to_public()}),
                ));
            }
        };

        let manifest = &loaded.manifest;
        let check = self
            .manifest_service
            .check_installable(manifest, Some(&self.installable_options()))?;

        let installed = self.read_installed(game_id)?;
        let mut installed = self.backfill_published_at(game_id, installed)?;
        let base = jv::prop(manifest, "baseArchive");
        let filename = jv::template(base.and_then(|base| jv::prop(base, "filename")));
        let resumable = self.download_manager.find_resumable(game_id, &filename)?;
        let pending = self
            .install_service
            .list_pending_operations()?
            .into_iter()
            .find(|item| jv::is_str(jv::prop(item, "gameId"), game_id));

        let view = self.compute_build_metadata_view(installed.as_mut());
        let size = base.and_then(|base| jv::prop(base, "size"));

        let mut patch = Map::new();
        patch.insert("version".to_owned(), field(manifest, "version"));
        patch.insert(
            "installedVersion".to_owned(),
            installed
                .as_ref()
                .map_or(Value::Null, |i| field(i, "version")),
        );
        patch.insert(
            "installedAt".to_owned(),
            installed
                .as_ref()
                .map_or(Value::Null, |i| or_null(jv::prop(i, "installedAt"))),
        );
        for (key, value) in view.iter() {
            patch.insert(key.clone(), value.clone());
        }
        patch.insert("installable".to_owned(), json!(check.ok));
        patch.insert(
            "requiresLicense".to_owned(),
            json!(self.requires_valve_license(game_id)),
        );
        patch.insert("source".to_owned(), json!("http"));
        patch.insert(
            "reasons".to_owned(),
            Value::Array(
                check
                    .reasons
                    .iter()
                    .map(|reason| json!(reason.code.as_str()))
                    .collect(),
            ),
        );
        patch.insert(
            "totalBytes".to_owned(),
            size.cloned().unwrap_or(Value::Null),
        );
        patch.insert("pending".to_owned(), pending.unwrap_or(Value::Null));
        self.patch_episodes(game_id, installed.as_ref(), &mut patch);

        if self.active_game_id().as_deref() == Some(game_id) {
            return Ok(self.set_status(game_id, Value::Object(patch)));
        }

        if installed.is_some() {
            let damaged = jv::is_str(view.get("metadataState"), "repair-required");
            patch.insert(
                "state".to_owned(),
                json!(if damaged { "files-damaged" } else { "ready" }),
            );
            patch.insert("percent".to_owned(), json!(100));
            return Ok(self.set_status(game_id, Value::Object(patch)));
        }

        if let Some(resumable) = resumable.filter(|r| r.bytes_downloaded > 0) {
            let total = size.map(|size| number_of(Some(size))).unwrap_or(f64::NAN);
            let percent = if total != 0.0 && !total.is_nan() {
                resumable.bytes_downloaded as f64 / total * 100.0
            } else {
                0.0
            };
            patch.insert("state".to_owned(), json!("paused"));
            patch.insert(
                "bytesDownloaded".to_owned(),
                json!(resumable.bytes_downloaded),
            );
            patch.insert("percent".to_owned(), js_number(percent));
            return Ok(self.set_status(game_id, Value::Object(patch)));
        }

        patch.insert("state".to_owned(), json!("not-installed"));
        patch.insert("percent".to_owned(), json!(0));
        Ok(self.set_status(game_id, Value::Object(patch)))
    }

    /// `refreshDepot(gameId)`: сборка из депо Steam — без манифеста доставки.
    fn refresh_depot(&self, game_id: &str) -> Result<Value, Failure> {
        let installed = self.read_installed(game_id)?;
        let version = installed
            .as_ref()
            .map_or(Value::Null, |i| field(i, "version"));
        let mut patch = json!({
            "version": version,
            "installedVersion": version,
            "installedAt": installed
                .as_ref()
                .map_or(Value::Null, |i| or_null(jv::prop(i, "installedAt"))),
            "buildMetadata": null,
            "buildDate": null,
            "buildDateSource": null,
            "metadataState": "legacy",
            "metadataIssue": null,
            "installable": true,
            "requiresLicense": self.requires_valve_license(game_id),
            "reasons": [],
            "totalBytes": 0,
            "pending": null,
            "source": "steam-depot",
        });

        if self.active_game_id().as_deref() == Some(game_id) {
            return Ok(self.set_status(game_id, patch));
        }

        if installed.is_some() {
            patch["state"] = json!("ready");
            patch["percent"] = json!(100);
        } else {
            patch["state"] = json!("not-installed");
            patch["percent"] = json!(0);
        }
        Ok(self.set_status(game_id, patch))
    }

    /// `refreshAll()`: игры реестра и сборки из депо.
    pub fn refresh_all(&self) -> Result<Vec<Value>, Failure> {
        let mut ids: Vec<String> = self
            .registry
            .manifests
            .iter()
            .map(|(id, _)| id.clone())
            .collect();
        ids.extend(manifests::depot_builds().into_iter().map(str::to_owned));
        ids.iter().map(|id| self.refresh(id)).collect()
    }

    fn on_download_progress(&self, progress: &Value) {
        let game_id = jv::template(jv::prop(progress, "gameId"));
        self.set_status(
            &game_id,
            json!({
                "state": "downloading",
                "percent": field(progress, "percent"),
                "bytesDownloaded": field(progress, "bytesDownloaded"),
                "totalBytes": field(progress, "totalBytes"),
                "speedBps": field(progress, "speedBps"),
                "etaSeconds": field(progress, "etaSeconds"),
            }),
        );
    }

    fn on_download_state(&self, event: &Value) {
        let state = match jv::prop(event, "state").and_then(Value::as_str) {
            Some("preparing") => "preparing",
            Some("resuming" | "downloading" | "retrying") => "downloading",
            Some("paused") => "paused",
            Some("cancelled") => "cancelled",
            Some("downloaded") => "verifying",
            Some("error") => "error",
            _ => return,
        };
        let game_id = jv::template(jv::prop(event, "gameId"));
        self.set_status(
            &game_id,
            json!({
                "state": state,
                "percent": field(event, "percent"),
                "bytesDownloaded": field(event, "bytesDownloaded"),
                "totalBytes": field(event, "totalBytes"),
                "speedBps": field(event, "speedBps"),
                "etaSeconds": field(event, "etaSeconds"),
                "error": or_null(jv::prop(event, "error")),
            }),
        );
    }

    fn on_install_state(&self, event: &Value) {
        let state = field(event, "state");
        let mut patch = Map::new();
        patch.insert("state".to_owned(), state.clone());
        patch.insert("error".to_owned(), or_null(jv::prop(event, "error")));
        if state == "extracting"
            && let Some(percent @ Value::Number(_)) = jv::prop(event, "percent")
        {
            patch.insert("percent".to_owned(), percent.clone());
        }
        if state == "ready" {
            patch.insert("percent".to_owned(), json!(100));
        }
        let game_id = jv::template(jv::prop(event, "gameId"));
        self.set_status(&game_id, Value::Object(patch));
    }

    /// Игра события или активная (`event.gameId || this.activeGameId`).
    fn event_game_id(&self, event: &Value) -> Option<String> {
        truthy_text(jv::prop(event, "gameId")).or_else(|| self.active_game_id())
    }

    fn on_patch_state(&self, event: &Value) {
        let Some(game_id) = self.event_game_id(event) else {
            return;
        };
        let mut patch = Map::new();
        patch.insert("state".to_owned(), field(event, "state"));
        patch.insert("error".to_owned(), or_null(jv::prop(event, "error")));
        if let Some(percent @ Value::Number(_)) = jv::prop(event, "percent") {
            patch.insert("percent".to_owned(), percent.clone());
        }
        if let Some(phase) = jv::prop(event, "phase").filter(|phase| js::truthy(phase)) {
            patch.insert("phase".to_owned(), phase.clone());
        }
        self.set_status(&game_id, Value::Object(patch));
    }

    fn on_integrity_progress(&self, event: &Value) {
        let Some(game_id) = self.event_game_id(event) else {
            return;
        };
        let percent = match jv::prop(event, "percent") {
            Some(percent @ Value::Number(_)) => percent.clone(),
            _ => json!(0),
        };
        self.set_status(
            &game_id,
            json!({
                "checked": field(event, "checked"),
                "totalChecked": field(event, "total"),
                "percent": percent,
            }),
        );
    }

    fn on_repair_state(&self, event: &Value) {
        let Some(game_id) = self.event_game_id(event) else {
            return;
        };
        self.set_status(
            &game_id,
            json!({
                "state": field(event, "state"),
                "phase": or_null(jv::prop(event, "phase")),
                "error": or_null(jv::prop(event, "error")),
            }),
        );
    }

    // ------------------------------------------------------------ слот операции

    /// `isBusy()`: идёт операция сервиса, загрузка, установка, патч, проверка или ремонт.
    pub fn is_busy(&self) -> bool {
        lock(&self.state).slot.is_some() || self.services_busy()
    }

    fn services_busy(&self) -> bool {
        self.download_manager.is_busy()
            || self.install_service.is_busy()
            || self.patch_service.is_busy()
            || self.integrity_service.is_busy()
            || self.repair_service.is_busy()
    }

    /// `reserveSlot(gameId, operation)`: занять слот синхронно, до первого `await`; занято —
    /// `ALREADY_RUNNING`. Слот освобождается при выходе из операции, если его никто не
    /// перехватил.
    fn reserve_slot(
        &self,
        game_id: &str,
        operation: &'static str,
    ) -> Result<SlotGuard<'_>, DeliveryError> {
        self.assert_not_rendering(game_id)?;
        let services_busy = self.services_busy();
        let mut state = lock(&self.state);
        if state.slot.is_some() || services_busy {
            let slot_game = state.slot.as_ref().map(|slot| slot.game_id.clone());
            let details = json!({
                "gameId": slot_game.or_else(|| state.active_game_id.clone()),
                "operation": state.slot.as_ref().map(|slot| slot.operation),
            });
            return Err(
                DeliveryError::new(Code::AlreadyRunning, "Уже идёт другая операция")
                    .with_details(details),
            );
        }
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        state.slot = Some(Slot {
            token,
            game_id: game_id.to_owned(),
            operation,
        });
        Ok(SlotGuard {
            service: self,
            token,
        })
    }

    /// Слот на время работы с модами игры (`custom-packs-*`): установка, проверка, обновление и
    /// запуск игры в это время отказывают. В 0.4.9 `customPacks` синхронный, и между проверкой
    /// занятости и копиями ничего не вклинивалось. Занято — `None`.
    pub fn reserve_custom_packs(&self, game_id: &str) -> Option<impl Sized + '_> {
        self.reserve_slot(game_id, "custom-packs").ok()
    }

    /// `assertGameNotRunning(gameId)`.
    fn assert_game_not_running(&self, game_id: &str) -> Result<(), DeliveryError> {
        self.assert_not_rendering(game_id)?;
        if !self.processes.is_running(game_id) {
            return Ok(());
        }
        Err(DeliveryError::new(
            Code::GameRunning,
            "Игра запущена. Закройте её и повторите операцию.",
        )
        .with_details(json!({"gameId": game_id})))
    }

    // ------------------------------------------------------------ установка

    /// `publishedAtFor(gameId, version)`: дата выпуска версии — из цели последней проверки или
    /// встроенного манифеста.
    fn published_at_for(
        &self,
        game_id: &str,
        version: Option<&Value>,
    ) -> Result<Value, DeliveryError> {
        if let Some(last) = self.update_checker.last_result(game_id)
            && let Some(target) = last.target()
            && strict_eq(jv::prop(target, "version"), version)
            && let Some(published) = jv::prop(target, "publishedAt").filter(|p| js::truthy(p))
        {
            return Ok(published.clone());
        }
        let manifest = self.manifest_for(game_id)?.manifest;
        Ok(if strict_eq(jv::prop(&manifest, "version"), version) {
            or_null(jv::prop(&manifest, "publishedAt"))
        } else {
            Value::Null
        })
    }

    /// `backfillPublishedAt(gameId, installed)`: дописать дату выпуска в старую запись об
    /// установке (раз за запуск).
    fn backfill_published_at(
        &self,
        game_id: &str,
        installed: Option<Value>,
    ) -> Result<Option<Value>, Failure> {
        let Some(installed) = installed else {
            return Ok(None);
        };
        if jv::truthy(jv::prop(&installed, "publishedAt"))
            || lock(&self.state).published_at_backfilled.contains(game_id)
        {
            return Ok(Some(installed));
        }

        let published_at = self.published_at_for(game_id, jv::prop(&installed, "version"))?;
        if !js::truthy(&published_at) {
            return Ok(Some(installed));
        }

        lock(&self.state)
            .published_at_backfilled
            .insert(game_id.to_owned());

        match self
            .install_service
            .fill_installed(game_id, &json!({"publishedAt": published_at}))
        {
            Ok(Some(updated)) => {
                self.logger.info(&format!(
                    "[game-delivery] дата выпуска {game_id} дозаписана: {}",
                    jv::template(Some(&published_at))
                ));
                Ok(Some(updated))
            }
            Ok(None) => Ok(Some(installed)),
            Err(error) => {
                self.logger.warn(&format!(
                    "[game-delivery] дата выпуска не дозаписана: {}",
                    message_of(&error)
                ));
                Ok(Some(installed))
            }
        }
    }

    /// `baseVersionOf(manifest)`: версия базового (или полного) архива, иначе версия манифеста.
    fn base_version_of(manifest: &Value) -> Value {
        let archive = [
            jv::prop(manifest, "baseArchive"),
            jv::prop(manifest, "fullArchive"),
        ]
        .into_iter()
        .flatten()
        .find(|archive| js::truthy(archive));
        archive
            .and_then(|archive| jv::prop(archive, "version"))
            .filter(|version| js::truthy(version))
            .cloned()
            .unwrap_or_else(|| field(manifest, "version"))
    }

    /// `baseInstallManifest(gameId, manifest)`: манифест базовой версии, если базовый архив старше
    /// версии манифеста (дальше — патчи).
    fn base_install_manifest(&self, game_id: &str, manifest: &Value) -> Result<Value, Failure> {
        let base_version = Self::base_version_of(manifest);
        if strict_eq(Some(&base_version), jv::prop(manifest, "version")) {
            return Ok(manifest.clone());
        }

        let version = truthy_text(Some(&base_version));
        let files = self.files_manifest_for(game_id, version.as_deref())?;

        let mut out = manifest.as_object().cloned().unwrap_or_default();
        out.insert("version".to_owned(), base_version.clone());
        let published = self.published_at_for(game_id, Some(&base_version))?;
        out.insert("publishedAt".to_owned(), or_null(Some(&published)));
        out.insert(
            "expectedInstalledSize".to_owned(),
            field(&files, "totalSize"),
        );
        out.insert("buildMetadata".to_owned(), Value::Null);
        Ok(Value::Object(out))
    }

    /// `install(gameId)`: установка сборки (базовой версии, если она старше манифеста).
    pub async fn install(&self, game_id: &str) -> Result<Value, Failure> {
        let _slot = self.reserve_slot(game_id, "install")?;
        self.run_install(game_id).await
    }

    async fn run_install(&self, game_id: &str) -> Result<Value, Failure> {
        self.assert_game_not_running(game_id)?;
        self.assert_steam_requirements(game_id).await?;

        if manifests::is_depot_build(game_id) {
            return self.install_from_depot(game_id).await;
        }

        let loaded = self.manifest_for(game_id)?;
        let declared = &loaded.manifest;
        let manifest = self.base_install_manifest(game_id, declared)?;

        self.ensure_layout()?;

        let check = self
            .manifest_service
            .assert_installable(&manifest, Some(&self.installable_options()))?;
        let archive_providers = check.providers;

        let _activity = self.activate(game_id, "install");
        self.set_status(game_id, json!({"state": "preparing", "error": null}));

        let result: Result<Value, Failure> = async {
            let base = jv::prop(&manifest, "baseArchive")
                .cloned()
                .unwrap_or(Value::Null);
            let filename = jv::template(jv::prop(&base, "filename"));
            let download = self
                .download_with_fallback(
                    game_id,
                    &archive_providers,
                    &filename,
                    jv::prop(&base, "size"),
                    jv::prop(&base, "sha256"),
                    "Источник полного архива выключен feature flag'ом",
                )
                .await?;

            if !jv::is_str(jv::prop(&download, "status"), "completed") {
                self.refresh(game_id)?;
                return Ok(download);
            }

            let published_at = self.published_at_for(game_id, jv::prop(&manifest, "version"))?;
            let installed = self
                .install_service
                .install(InstallRequest {
                    manifest: manifest.clone(),
                    manifest_sha256: Some(loaded.sha256.clone()),
                    archive_path: PathBuf::from(jv::template(jv::prop(&download, "filePath"))),
                    files_manifest: None,
                    published_at: truthy_text(Some(&published_at)),
                    update: None,
                })
                .await?;

            self.download_manager.discard(game_id, &filename)?;
            self.remove_archive(game_id, &filename);

            self.set_active(None);
            self.set_operation(game_id, None);
            self.refresh(game_id)?;

            let same = strict_eq(
                jv::prop(&installed, "version"),
                jv::prop(declared, "version"),
            );
            let version = truthy_text(jv::prop(&installed, "version"));
            self.announce_installed(
                game_id,
                version.as_deref(),
                if same {
                    jv::prop(declared, "notes")
                } else {
                    None
                },
            );

            Ok(json!({"status": "installed", "installed": installed, "baseOnly": !same}))
        }
        .await;

        result.map_err(|error| self.fail_with(game_id, error, Code::InstallFailed, true))
    }

    /// `catch` операций: ошибка — `DeliveryError` (`fallback` для чужой), статус `error` (или
    /// `cancelled`, если `cancelled_state` и код `CANCELLED`).
    fn fail_with(
        &self,
        game_id: &str,
        error: Failure,
        fallback: Code,
        cancelled_state: bool,
    ) -> Failure {
        let delivery = to_delivery_error(error, Some(fallback));
        let state = if cancelled_state && delivery.code == Code::Cancelled {
            "cancelled"
        } else {
            "error"
        };
        self.set_status(
            game_id,
            json!({"state": state, "error": delivery.to_public()}),
        );
        delivery.into()
    }

    /// `requiresValveLicense(gameId)`: сборке нужны игры Valve на аккаунте Steam.
    pub fn requires_valve_license(&self, game_id: &str) -> bool {
        manifests::launch_config_of(game_id)
            .is_some_and(|config| !config.requires_owned_app_ids.is_empty())
    }

    /// `assertSteamRequirements(gameId)`: игры Valve на аккаунте (через службу Steam).
    async fn assert_steam_requirements(&self, game_id: &str) -> Result<Option<Value>, Failure> {
        let app_ids = manifests::launch_config_of(game_id)
            .map(|config| config.requires_owned_app_ids)
            .unwrap_or(&[]);
        if app_ids.is_empty() {
            return Ok(None);
        }
        let Some(depot) = &self.depot else {
            return Err(DeliveryError::new(
                Code::LicenseCheckUnavailable,
                "Проверка лицензии Steam недоступна в этой сборке лаунчера",
            )
            .with_details(json!({"gameId": game_id}))
            .into());
        };
        depot.assert_owns(app_ids).await?;
        Ok(Some(json!({"owned": true})))
    }

    /// `installFromDepot(gameId)`: сборка из депо Steam.
    async fn install_from_depot(&self, game_id: &str) -> Result<Value, Failure> {
        let Some(depot) = self.depot.clone() else {
            return Err(DeliveryError::new(
                Code::ProviderUnsupported,
                "Установка через депоты Steam в этой сборке лаунчера недоступна",
            )
            .with_details(json!({"gameId": game_id}))
            .into());
        };

        self.ensure_layout()?;

        let _activity = self.activate(game_id, "install");
        self.set_status(
            game_id,
            json!({
                "state": "preparing",
                "percent": 0,
                "error": null,
                "source": "steam-depot",
                "filesDone": 0,
                "filesTotal": 0,
            }),
        );

        let me = self.me.clone();
        let id = game_id.to_owned();
        let on_progress: DepotProgressFn = Arc::new(move |progress: &DepotProgress| {
            let Some(service) = me.upgrade() else {
                return;
            };
            let total = progress.expected_files.unwrap_or(0);
            let percent = if total != 0 {
                99f64.min(js::math_round(progress.files as f64 / total as f64 * 100.0))
            } else {
                0.0
            };
            service.set_status(
                &id,
                json!({
                    "state": "downloading",
                    "percent": js_number(percent),
                    "source": "steam-depot",
                    "filesDone": progress.files,
                    "filesTotal": total,
                    "depotStep": progress.index + 1,
                    "depotSteps": progress.total,
                }),
            );
        });

        let result: Result<Value, Failure> = async {
            let installed = depot.install(&self.paths, game_id, on_progress).await?;
            self.set_active(None);
            self.set_operation(game_id, None);
            self.set_status(
                game_id,
                json!({"state": "ready", "percent": 100, "chain": null, "error": null}),
            );
            self.refresh(game_id)?;
            let version = truthy_text(jv::prop(&installed, "version"));
            self.announce_installed(game_id, version.as_deref(), None);
            Ok(json!({"status": "installed", "installed": installed}))
        }
        .await;

        result.map_err(|error| {
            let delivery = to_delivery_error(error, Some(Code::InstallFailed));
            let state = if delivery.code == Code::Cancelled {
                "cancelled"
            } else {
                "error"
            };
            self.set_status(
                game_id,
                json!({"state": state, "error": delivery.to_public(), "chain": null}),
            );
            delivery.into()
        })
    }

    /// `installToLatest(gameId)`: установка и патчи до версии манифеста; события версий — одно,
    /// в конце.
    pub async fn install_to_latest(&self, game_id: &str) -> Result<Value, Failure> {
        lock(&self.state)
            .silent_version_events
            .insert(game_id.to_owned());
        let result = self.install_to_latest_silently(game_id).await;
        lock(&self.state).silent_version_events.remove(game_id);
        self.announce_installed_final(game_id)?;
        result
    }

    /// `announceInstalledFinal(gameId)`.
    fn announce_installed_final(&self, game_id: &str) -> Result<(), Failure> {
        let Some(installed) = self.read_installed(game_id)? else {
            return Ok(());
        };
        let Some(version) = truthy_text(jv::prop(&installed, "version")) else {
            return Ok(());
        };
        if !jv::is_str(jv::prop(&installed, "state"), "ready") {
            return Ok(());
        }
        let notes = match self.manifest_for(game_id) {
            Ok(loaded)
                if strict_eq(
                    jv::prop(&loaded.manifest, "version"),
                    jv::prop(&installed, "version"),
                ) =>
            {
                or_null(jv::prop(&loaded.manifest, "notes"))
            }
            _ => Value::Null,
        };
        self.announce_installed(game_id, Some(&version), Some(&notes));
        Ok(())
    }

    async fn install_to_latest_silently(&self, game_id: &str) -> Result<Value, Failure> {
        let result = self.install(game_id).await?;

        if !jv::is_str(jv::prop(&result, "status"), "installed")
            || !jv::truthy(jv::prop(&result, "baseOnly"))
        {
            return Ok(result);
        }

        let declared = field(&self.manifest_for(game_id)?.manifest, "version");
        let pending = |result: &Value| {
            let mut out = result.as_object().cloned().unwrap_or_default();
            out.insert("pendingVersion".to_owned(), declared.clone());
            Value::Object(out)
        };

        self.update_checker.invalidate(game_id);

        if let Err(error) = self.check_for_updates(game_id, true, None).await {
            self.logger.warn(&format!(
                "[game-delivery] база {game_id} установлена, но проверка обновлений не удалась: {}",
                message_of(&error)
            ));
            return Ok(pending(&result));
        }

        let plan = self.plan_update(game_id)?;

        if plan.kind == PlanKind::UpToDate {
            return Ok(result);
        }

        if plan.kind != PlanKind::Patches {
            self.logger.warn(&format!(
                "[game-delivery] от базы {game_id} нет патча до {} ({}) — остаёмся на базе, полный архив второй раз не качаем",
                jv::template(Some(&declared)),
                plan.reason.unwrap_or(plan.kind.as_str())
            ));
            return Ok(pending(&result));
        }

        let updated = Box::pin(self.apply_update(game_id)).await?;

        Ok(json!({
            "status": "installed",
            "installed": self.read_installed(game_id)?,
            "base": field(&result, "installed"),
            "updated": updated,
        }))
    }

    /// `removeArchive(gameId, filename)`: архив из `Downloads/<gameId>`; ошибка — в журнал.
    fn remove_archive(&self, game_id: &str, filename: &str) {
        let target = join(&[&text(&self.paths.downloads), game_id, &basename(filename)]);
        if exists(&target)
            && let Err(error) = remove_within_zone(&self.paths.downloads, &target)
        {
            self.logger.warn(&format!(
                "[game-delivery] не удалось удалить архив: {}",
                message_of(&error)
            ));
        }
    }

    /// `installedOf(gameId)`: запись об установке; нет — `NOT_INSTALLED`.
    pub fn installed_of(&self, game_id: &str) -> Result<Value, DeliveryError> {
        self.read_installed(game_id)?.ok_or_else(|| {
            DeliveryError::new(Code::NotInstalled, "Игра не установлена")
                .with_details(json!({"gameId": game_id}))
        })
    }

    /// `assertNotDepotBuild(gameId, action)`.
    fn assert_not_depot_build(&self, game_id: &str, action: &str) -> Result<(), DeliveryError> {
        if !manifests::is_depot_build(game_id) {
            return Ok(());
        }
        Err(DeliveryError::new(
            Code::SteamManagedBuild,
            format!("Сборка {game_id} доставляется Steam: {action} для неё не выполняется"),
        )
        .with_details(json!({"gameId": game_id})))
    }

    // ------------------------------------------------------------ проверка файлов

    /// `verifyFiles(gameId)`: сверка установленной сборки с файловым манифестом её версии.
    pub async fn verify_files(&self, game_id: &str) -> Result<IntegrityResult, Failure> {
        self.assert_not_depot_build(game_id, "проверка файлов")?;
        Self::assert_file_repair_game(game_id)?;
        let _slot = self.reserve_slot(game_id, "verify")?;
        self.run_verify_files(game_id).await
    }

    async fn run_verify_files(&self, game_id: &str) -> Result<IntegrityResult, Failure> {
        self.assert_game_not_running(game_id)?;

        let installed = self.installed_of(game_id)?;
        let manifest = self.manifest_for(game_id)?.manifest;
        let version = truthy_text(jv::prop(&installed, "version"));
        let files = self
            .resolve_files_manifest(game_id, version.as_deref())
            .await?;

        let _activity = self.activate(game_id, "verify");
        self.set_status(
            game_id,
            json!({"state": "checking-files", "percent": 0, "error": null}),
        );

        let install_path = PathBuf::from(jv::template(jv::prop(&installed, "installPath")));
        let preserve = preserve_paths(&manifest);
        let result = self
            .integrity_service
            .check(IntegrityRequest {
                game_id,
                install_path: &install_path,
                files_manifest: &files,
                preserve_paths: &preserve,
                stock_configs: true,
            })
            .await;

        match result {
            Ok(result) => {
                lock(&self.state)
                    .last_integrity
                    .insert(game_id.to_owned(), result.clone());
                self.set_status(
                    game_id,
                    json!({
                        "state": if result.ok { "files-valid" } else { "files-damaged" },
                        "percent": 100,
                        "integrity": Self::summarize_integrity(&result),
                    }),
                );
                Ok(result)
            }
            Err(error) => Err(self.fail_with(game_id, error, Code::VerifyFailed, false)),
        }
    }

    /// `summarizeIntegrity(result)`: счёт проверки и первые десять повреждённых путей.
    pub fn summarize_integrity(result: &IntegrityResult) -> Value {
        let samples: Vec<&String> = Self::damaged_paths_of(result)
            .into_iter()
            .take(10)
            .collect();
        json!({
            "total": result.total,
            "checked": result.checked,
            "valid": result.valid.len(),
            "missing": result.missing.len(),
            "corrupted": result.corrupted.len(),
            "sizeMismatch": result.size_mismatch.len(),
            "configMissing": result.config_missing.len(),
            "configChanged": result.config_changed.len(),
            "stockMissing": result.stock_missing,
            "extra": result.extra.len(),
            "damaged": result.damaged,
            "ok": result.ok,
            "samples": samples,
        })
    }

    /// `damagedPathsOf(integrity)`: пропавшие, повреждённые, другого размера и пропавшие cfg.
    fn damaged_paths_of(result: &IntegrityResult) -> Vec<&String> {
        result
            .missing
            .iter()
            .chain(result.corrupted.iter().map(|item| &item.path))
            .chain(result.size_mismatch.iter().map(|item| &item.path))
            .chain(&result.config_missing)
            .collect()
    }

    // ------------------------------------------------------------ проверка обновлений

    /// `targetFor(gameId)`: цель последней проверки или встроенного манифеста.
    pub fn target_for(&self, game_id: &str) -> Result<Option<Value>, DeliveryError> {
        if let Some(last) = self.update_checker.last_result(game_id)
            && let Some(target) = last.target()
        {
            return Ok(Some(target.clone()));
        }
        let loaded = self.manifest_for(game_id)?;
        Ok(update_target::from_local(&loaded.manifest).map(|target| {
            let mut value = target.to_json();
            value["manifestSha256"] = json!(loaded.sha256);
            value
        }))
    }

    /// `planUpdate(gameId)`: патчи, полная сборка или «обновлять нечего».
    pub fn plan_update(&self, game_id: &str) -> Result<UpdatePlan, Failure> {
        let target = self.target_for(game_id)?;
        let installed = self.read_installed(game_id)?;

        let Some(target) = target else {
            return Ok(UpdatePlan {
                kind: PlanKind::UpToDate,
                reason: Some("no-target"),
                steps: Vec::new(),
                total_size: 0.0,
                full_size: None,
                message: None,
            });
        };

        let patches = jv::prop(&target, "patches")
            .filter(|patches| js::truthy(patches))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let from_version = installed
            .as_ref()
            .and_then(|installed| jv::prop(installed, "version"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let full_size = jv::prop(&target, "fullArchive")
            .filter(|archive| js::truthy(archive))
            .map_or(0.0, |archive| number_of(jv::prop(archive, "size")));
        Ok(patch_chain::plan_update(&PlanOptions {
            patches: &patches,
            from_version: from_version.as_deref(),
            to_version: jv::prop(&target, "version").and_then(Value::as_str),
            full_size: Some(full_size),
        }))
    }

    /// `checkForUpdates(gameId, options)`: проверка (по умолчанию принудительная), план и статус.
    pub async fn check_for_updates(
        &self,
        game_id: &str,
        force: bool,
        reason: Option<&str>,
    ) -> Result<Value, Failure> {
        self.assert_not_depot_build(game_id, "проверка обновлений")?;

        let result = self
            .update_checker
            .check(
                game_id,
                &CheckOptions {
                    force,
                    reason: Some(
                        reason
                            .filter(|r| !r.is_empty())
                            .unwrap_or("manual")
                            .to_owned(),
                    ),
                    ignore_cache: false,
                },
            )
            .await;

        let plan = self.plan_update(game_id)?;
        let steps: Vec<Value> = plan
            .steps
            .iter()
            .map(|step| {
                json!(format!(
                    "{} -> {}",
                    jv::template(jv::prop(step, "fromVersion")),
                    jv::template(jv::prop(step, "toVersion"))
                ))
            })
            .collect();

        let status = self.set_status(
            game_id,
            json!({
                "updatePlan": {
                    "kind": plan.kind.as_str(),
                    "reason": plan.reason,
                    "steps": steps,
                    "totalSize": js_number(plan.total_size),
                },
            }),
        );

        Ok(json!({
            "status": status,
            "plan": plan.to_json(),
            "update": self.public_update(game_id),
            "raw": result.to_json(),
        }))
    }

    /// `checkForUpdatesIfDue(gameId, reason)`: фоновая проверка, если интервал прошёл.
    pub async fn check_for_updates_if_due(
        &self,
        game_id: &str,
        reason: Option<&str>,
    ) -> Option<Value> {
        if manifests::is_depot_build(game_id) {
            return None;
        }
        let result = self
            .update_checker
            .check(
                game_id,
                &CheckOptions {
                    force: false,
                    reason: Some(
                        reason
                            .filter(|r| !r.is_empty())
                            .unwrap_or("scheduled")
                            .to_owned(),
                    ),
                    ignore_cache: false,
                },
            )
            .await;
        Some(result.to_json())
    }

    /// `checkForUpdatesOnFocus(gameId)`: проверка при фокусе окна, если прошлой больше 5 минут.
    pub async fn check_for_updates_on_focus(&self, game_id: &str) -> Option<Value> {
        if manifests::is_depot_build(game_id) {
            return None;
        }
        self.update_checker
            .check_if_older_than(game_id, FOCUS_CHECK_AGE_MS, Some("focus"))
            .await
            .map(|result| result.to_json())
    }
}

/// `manifest.preservePaths` как список строк.
fn preserve_paths(manifest: &Value) -> Vec<String> {
    jv::prop(manifest, "preservePaths")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| jv::template(Some(item)))
        .collect()
}

// ---------------------------------------------------------------- обновления

impl GameDeliveryService {
    /// `applyUpdate(gameId)`: патчи по цепочке или полная сборка.
    pub async fn apply_update(&self, game_id: &str) -> Result<Value, Failure> {
        let _slot = self.reserve_slot(game_id, "update")?;
        self.run_apply_update(game_id).await
    }

    async fn run_apply_update(&self, game_id: &str) -> Result<Value, Failure> {
        self.assert_game_not_running(game_id)?;
        self.assert_steam_requirements(game_id).await?;

        let manifest = self.manifest_for(game_id)?.manifest;
        let plan = self.plan_update(game_id)?;

        if plan.kind == PlanKind::UpToDate {
            return Ok(json!({"status": "up-to-date"}));
        }
        if plan.kind == PlanKind::Full && plan.reason == Some("downgrade") {
            return Ok(json!({"status": "up-to-date", "reason": "downgrade"}));
        }
        if plan.kind == PlanKind::Full {
            return self.apply_full_update(game_id, &plan).await;
        }

        self.ensure_layout()?;

        self.set_active(Some(game_id));
        self.set_operation(game_id, Some("update"));

        let steps = &plan.steps;
        let chain_total: f64 = steps
            .iter()
            .map(
                |step| match jv::prop(step, "archive").filter(|a| js::truthy(a)) {
                    Some(archive) => number_of(jv::prop(archive, "size")),
                    None => 0.0,
                },
            )
            .sum();
        let last_step = steps.last().cloned().unwrap_or(Value::Null);
        let target_version = field(&last_step, "toVersion");
        let target_text = truthy_text(Some(&target_version));

        self.try_ensure_files_manifest(game_id, target_text.as_deref())
            .await;

        let result: Result<Value, Failure> = async {
            let version_before = self.installed_version_of(game_id);
            let mut chain_done = 0.0;

            for (index, step) in steps.iter().enumerate() {
                let installed = self.installed_of(game_id)?;

                self.set_status(
                    game_id,
                    json!({
                        "chain": {
                            "step": index + 1,
                            "steps": steps.len(),
                            "fromVersion": field(step, "fromVersion"),
                            "toVersion": field(step, "toVersion"),
                            "targetVersion": target_version,
                            "doneBytes": js_number(chain_done),
                            "totalBytes": js_number(chain_total),
                        },
                    }),
                );

                let archive = field(step, "archive");
                let usable = self.usable_providers(jv::prop(&archive, "providers"));
                let disabled = "Источник патча выключен feature flag'ом";
                if usable.is_empty() {
                    return Err(DeliveryError::new(Code::RemoteDisabled, disabled).into());
                }

                self.set_status(
                    game_id,
                    json!({"state": "preparing", "percent": 0, "error": null}),
                );

                let filename = jv::template(jv::prop(&archive, "filename"));
                let download = self
                    .download_with_fallback(
                        game_id,
                        &usable,
                        &filename,
                        jv::prop(&archive, "size"),
                        jv::prop(&archive, "sha256"),
                        disabled,
                    )
                    .await?;

                if !jv::is_str(jv::prop(&download, "status"), "completed") {
                    self.refresh(game_id)?;
                    return Ok(download);
                }

                let archive_path = jv::template(jv::prop(&download, "filePath"));
                let patch_manifest = self
                    .load_patch_manifest(game_id, step, &archive_path, &usable)
                    .await?;

                let to_version = truthy_text(jv::prop(step, "toVersion"));
                let game_manifest = self
                    .build_descriptor_for(game_id, to_version.as_deref())
                    .map(|(_, manifest)| manifest)
                    .filter(js::truthy)
                    .unwrap_or_else(|| manifest.clone());

                let applied = self
                    .patch_service
                    .apply(PatchRequest {
                        game_manifest,
                        patch_manifest,
                        installed_version: jv::prop(&installed, "version")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        archive_path: PathBuf::from(&archive_path),
                    })
                    .await;
                if applied.is_err() {
                    // Манифест мог быть подменён (files[]): повтор скачает его с зеркала заново.
                    self.drop_patch_manifest(&archive_path);
                }
                applied?;

                self.remove_archive(game_id, &filename);
                chain_done += number_of(jv::prop(&archive, "size"));
            }

            self.update_checker.invalidate(game_id);
            self.update_checker
                .check(
                    game_id,
                    &CheckOptions {
                        force: true,
                        reason: Some("after-update".to_owned()),
                        ignore_cache: false,
                    },
                )
                .await;

            let status = self.refresh(game_id)?;
            let target = self.target_for(game_id)?;

            self.announce_updated(game_id, version_before.as_deref(), target_text.as_deref());

            Ok(json!({
                "status": "updated",
                "version": match &target {
                    Some(target) => field(target, "version"),
                    None => field(&manifest, "version"),
                },
                "delivery": status,
            }))
        }
        .await;

        let result =
            result.map_err(|error| self.fail_with(game_id, error, Code::PatchFailed, false));
        self.finish_update(game_id);
        self.with_live_delivery(game_id, result)
    }

    /// `delivery` в итоге обновления — в 0.4.7 это тот же объект статуса, что меняет `finally`:
    /// вызывающий видит статус после снятия пометки операции.
    fn with_live_delivery(
        &self,
        game_id: &str,
        result: Result<Value, Failure>,
    ) -> Result<Value, Failure> {
        result.map(|mut value| {
            if value.get("delivery").is_some() {
                value["delivery"] = self.status_of(game_id);
            }
            value
        })
    }

    /// `finally` обновления: снять активную игру; пауза загрузки оставляет пометку операции.
    fn finish_update(&self, game_id: &str) {
        self.set_active(None);
        if self.state_name_of(game_id) != "paused" {
            self.set_operation(game_id, None);
            self.set_status(game_id, json!({"chain": null}));
        } else {
            self.set_status(game_id, json!({}));
        }
    }

    /// `applyFullUpdate(gameId, plan)`: новая версия полной сборкой поверх установленной.
    async fn apply_full_update(&self, game_id: &str, plan: &UpdatePlan) -> Result<Value, Failure> {
        let Some(installed) = self.read_installed(game_id)? else {
            return Box::pin(self.install_to_latest(game_id)).await;
        };

        let loaded = self.manifest_for(game_id)?;
        let target = self.target_for(game_id)?;
        let descriptor = target
            .as_ref()
            .and_then(|target| jv::prop(target, "fullArchive"))
            .filter(|archive| js::truthy(archive))
            .cloned();

        let (Some(target), Some(descriptor)) = (target, descriptor) else {
            return Err(DeliveryError::new(
                Code::ManifestInvalid,
                "Источник не объявил полный архив целевой версии — обновляться нечем",
            )
            .with_details(json!({"gameId": game_id, "reason": plan.reason}))
            .into());
        };

        let target_version = field(&target, "version");
        let archive_version = jv::prop(&descriptor, "version")
            .filter(|version| js::truthy(version))
            .cloned()
            .unwrap_or_else(|| target_version.clone());

        if !strict_eq(Some(&archive_version), Some(&target_version)) {
            let (from, to) = (
                jv::template(Some(&archive_version)),
                jv::template(Some(&target_version)),
            );
            return Err(DeliveryError::new(
                Code::ManifestInvalid,
                format!(
                    "Полный архив содержит версию {from}, а обновление ведёт к {to}: нужен патч {from} -> {to}, но канал его не объявил"
                ),
            )
            .with_details(json!({
                "gameId": game_id,
                "archiveVersion": archive_version,
                "targetVersion": target_version,
            }))
            .into());
        }

        let usable = self.usable_providers(jv::prop(&descriptor, "providers"));

        self.ensure_layout()?;

        self.set_active(Some(game_id));
        self.set_operation(game_id, Some("update"));

        let target_text = truthy_text(Some(&target_version));
        let result: Result<Value, Failure> = async {
            self.set_status(
                game_id,
                json!({
                    "state": "preparing",
                    "percent": 0,
                    "error": null,
                    "chain": {
                        "step": 1,
                        "steps": 1,
                        "fromVersion": field(&installed, "version"),
                        "toVersion": target_version,
                        "targetVersion": target_version,
                        "doneBytes": 0,
                        "totalBytes": field(&descriptor, "size"),
                    },
                }),
            );

            // Полный архив ставится только с SHA-256 из подписанного канала или встроенного
            // манифеста: сборку без закреплённого хеша не ставим.
            Self::assert_pinned_archive(game_id, &descriptor, &target_version)?;

            let files = Some(
                self.resolve_files_manifest(game_id, target_text.as_deref())
                    .await?,
            );

            let local_archive = self.find_local_archive(game_id, &descriptor)?;
            let expected = json!({"totalSize": field(&loaded.manifest, "expectedInstalledSize")});
            let user_data = self.install_service.user_data_bytes(
                game_id,
                &game_install_path(&self.paths, game_id)?,
                &loaded.manifest,
            );
            self.check_full_update_space(
                &descriptor,
                files.as_ref().unwrap_or(&expected),
                local_archive.is_some(),
                user_data,
            )?;

            let archive_path = match local_archive {
                Some(local) => local,
                None => {
                    let filename = jv::template(jv::prop(&descriptor, "filename"));
                    let download = self
                        .download_with_fallback(
                            game_id,
                            &usable,
                            &filename,
                            jv::prop(&descriptor, "size"),
                            jv::prop(&descriptor, "sha256"),
                            "Источник полного архива выключен feature flag'ом",
                        )
                        .await?;

                    if !jv::is_str(jv::prop(&download, "status"), "completed") {
                        self.refresh(game_id)?;
                        return Ok(download);
                    }

                    let path = PathBuf::from(jv::template(jv::prop(&download, "filePath")));
                    self.mark_archive_origin(game_id, &descriptor);
                    path
                }
            };

            let manifest_sha256 = truthy_text(jv::prop(&target, "manifestSha256"))
                .unwrap_or_else(|| loaded.sha256.clone());
            let result = self
                .install_service
                .install(InstallRequest {
                    manifest: Self::full_update_manifest(
                        &loaded.manifest,
                        &target,
                        &descriptor,
                        files.as_ref(),
                    ),
                    manifest_sha256: Some(manifest_sha256),
                    archive_path,
                    files_manifest: None,
                    published_at: None,
                    update: Some(FullUpdate {
                        from_version: jv::template(jv::prop(&installed, "version")),
                        files_manifest: files.clone(),
                        integrity: Some(self.integrity_service.clone() as Arc<dyn IntegrityCheck>),
                    }),
                })
                .await?;

            self.cleanup_full_update_archive(game_id, &descriptor)?;

            self.update_checker.invalidate(game_id);
            self.update_checker
                .check(
                    game_id,
                    &CheckOptions {
                        force: true,
                        reason: Some("after-update".to_owned()),
                        ignore_cache: false,
                    },
                )
                .await;

            let status = self.refresh(game_id)?;
            let from_version = truthy_text(jv::prop(&installed, "version"));
            let to_version = truthy_text(jv::prop(&result, "version"));
            self.announce_updated(game_id, from_version.as_deref(), to_version.as_deref());

            Ok(json!({
                "status": "updated",
                "version": field(&result, "version"),
                "kind": "full",
                "delivery": status,
            }))
        }
        .await;

        let result =
            result.map_err(|error| self.fail_with(game_id, error, Code::InstallFailed, true));
        self.finish_update(game_id);
        self.with_live_delivery(game_id, result)
    }

    /// `fullUpdateManifest(localManifest, target, descriptor, files)`.
    fn full_update_manifest(
        local: &Value,
        target: &Value,
        descriptor: &Value,
        files: Option<&Value>,
    ) -> Value {
        let mut out = local.as_object().cloned().unwrap_or_default();
        out.insert("version".to_owned(), field(target, "version"));
        out.insert(
            "publishedAt".to_owned(),
            or_null(jv::prop(target, "publishedAt")),
        );
        let size = jv::prop(target, "expectedInstalledSize")
            .filter(|size| js::truthy(size))
            .cloned()
            .unwrap_or_else(|| files.map_or(Value::Null, |files| field(files, "totalSize")));
        out.insert("expectedInstalledSize".to_owned(), size);
        let build_metadata = jv::prop(target, "buildMetadata")
            .filter(|meta| js::truthy(meta))
            .cloned()
            .or_else(|| {
                strict_eq(jv::prop(local, "version"), jv::prop(target, "version"))
                    .then(|| jv::prop(local, "buildMetadata").cloned())
                    .flatten()
                    .filter(js::truthy)
            })
            .unwrap_or(Value::Null);
        out.insert("buildMetadata".to_owned(), build_metadata);
        let mut base = Map::new();
        base.insert("filename".to_owned(), field(descriptor, "filename"));
        base.insert("size".to_owned(), field(descriptor, "size"));
        base.insert("sha256".to_owned(), field(descriptor, "sha256"));
        base.insert(
            "rootPrefix".to_owned(),
            or_null(jv::prop(descriptor, "rootPrefix")),
        );
        base.insert(
            "providers".to_owned(),
            jv::prop(descriptor, "providers")
                .filter(|providers| js::truthy(providers))
                .cloned()
                .unwrap_or_else(|| json!([])),
        );
        out.insert("baseArchive".to_owned(), Value::Object(base));
        Value::Object(out)
    }

    /// `findLocalArchive(gameId, descriptor)`: архив той же версии уже лежит в `Downloads`.
    fn find_local_archive(
        &self,
        game_id: &str,
        descriptor: &Value,
    ) -> Result<Option<PathBuf>, Failure> {
        let candidate = join(&[
            &text(&self.paths.downloads),
            game_id,
            &basename(&jv::template(jv::prop(descriptor, "filename"))),
        ]);
        let Ok(stat) = std::fs::metadata(&candidate) else {
            return Ok(None);
        };
        if !stat.is_file()
            || Some(stat.len() as f64) != jv::prop(descriptor, "size").and_then(jv::number)
        {
            return Ok(None);
        }
        let digest = files_manifest::hash_file_sync(&candidate)?;
        if !jv::is_str(jv::prop(descriptor, "sha256"), &digest) {
            self.logger.warn(&format!(
                "[game-delivery] архив в Downloads не подходит для версии: SHA-256 {digest} вместо {} — будет скачан заново",
                jv::template(jv::prop(descriptor, "sha256"))
            ));
            return Ok(None);
        }
        Ok(Some(candidate))
    }

    /// `checkFullUpdateSpace(descriptor, files, options)`: место на архив, сборку, копии файлов
    /// игрока в новой сборке (`userDataBytes`, как `InstallService.checkSpace`) и 512 МБ.
    fn check_full_update_space(
        &self,
        descriptor: &Value,
        files: &Value,
        archive_local: bool,
        user_data_bytes: u64,
    ) -> Result<(), DeliveryError> {
        let Some(free) = self.free_space.free_bytes(&self.paths.root) else {
            return Ok(());
        };
        let archive = if archive_local {
            0.0
        } else {
            number_of(jv::prop(descriptor, "size"))
        };
        let installed = jv::prop(files, "totalSize")
            .filter(|size| js::truthy(size))
            .map_or(0.0, |size| number_of(Some(size)));
        let required =
            archive + installed + user_data_bytes as f64 + FULL_UPDATE_SPACE_MARGIN as f64;
        if (free as f64) < required {
            return Err(DeliveryError::new(
                Code::NoSpace,
                format!(
                    "Недостаточно места для обновления: нужно {}, свободно {free}",
                    js::number::number_to_string(required)
                ),
            )
            .with_details(json!({"requiredBytes": js_number(required), "freeBytes": free})));
        }
        Ok(())
    }

    /// `archiveOriginPath(gameId)`: пометка «архив скачал лаунчер».
    fn archive_origin_path(&self, game_id: &str) -> PathBuf {
        join(&[
            &text(&self.paths.state),
            &format!("{game_id}.archive-origin.json"),
        ])
    }

    /// `markArchiveOrigin(gameId, descriptor)`.
    fn mark_archive_origin(&self, game_id: &str, descriptor: &Value) {
        let mark = json!({
            "filename": basename(&jv::template(jv::prop(descriptor, "filename"))),
            "sha256": field(descriptor, "sha256"),
            "downloadedByLauncher": true,
            "at": self.now_iso(),
        });
        if let Err(error) =
            crate::atomic::write_json_atomic(&self.archive_origin_path(game_id), &mark)
        {
            self.logger.warn(&format!(
                "[game-delivery] пометка происхождения архива не записана: {}",
                error.message
            ));
        }
    }

    fn read_archive_origin(&self, game_id: &str) -> Value {
        read_json_safe(&self.archive_origin_path(game_id))
    }

    fn clear_archive_origin(&self, game_id: &str) {
        let file = self.archive_origin_path(game_id);
        if let Err(error) = crate::file_ops::rm_sync(&file, false, 0, Duration::from_millis(100)) {
            self.logger.warn(&format!(
                "[game-delivery] пометка происхождения архива не удалена: {}",
                error.message
            ));
        }
    }

    /// `cleanupFullUpdateArchive(gameId, descriptor)`: архив, скачанный лаунчером, удаляется;
    /// положенный игроком — остаётся.
    fn cleanup_full_update_archive(
        &self,
        game_id: &str,
        descriptor: &Value,
    ) -> Result<(), Failure> {
        let origin = self.read_archive_origin(game_id);
        let filename = jv::template(jv::prop(descriptor, "filename"));
        let ours = js::truthy(&origin)
            && jv::truthy(jv::prop(&origin, "downloadedByLauncher"))
            && jv::is_str(jv::prop(&origin, "filename"), &basename(&filename))
            && strict_eq(jv::prop(&origin, "sha256"), jv::prop(descriptor, "sha256"));

        if !ours {
            self.logger.info(&format!(
                "[game-delivery] архив {filename} предоставлен не лаунчером — оставляю"
            ));
            return Ok(());
        }

        self.download_manager.discard(game_id, &filename)?;
        self.remove_archive(game_id, &filename);
        self.clear_archive_origin(game_id);
        Ok(())
    }

    /// `pendingFullUpdate(gameId)`.
    pub fn pending_full_update(&self, game_id: &str) -> Option<Value> {
        self.install_service.pending_full_update(game_id)
    }

    /// `cleanupOrphanStaging()`.
    pub fn cleanup_orphan_staging(&self) -> usize {
        self.install_service
            .cleanup_orphan_staging(OrphanOptions::default())
    }

    /// `recoverFullUpdate(gameId)`: восстановление после сбоя; доведённое вперёд обновление
    /// убирает свой архив.
    pub fn recover_full_update(&self, game_id: &str) -> Result<Recovery, Failure> {
        let pending = self.install_service.pending_full_update(game_id);
        let result = self.install_service.recover_full_update(game_id)?;

        if result.recovered && result.direction == Some("forward") && pending.is_some() {
            let origin = self.read_archive_origin(game_id);
            if js::truthy(&origin)
                && jv::truthy(jv::prop(&origin, "downloadedByLauncher"))
                && let Some(filename) = truthy_text(jv::prop(&origin, "filename"))
            {
                self.download_manager.discard(game_id, &filename)?;
                self.remove_archive(game_id, &filename);
                self.clear_archive_origin(game_id);
            }
            self.update_checker.invalidate(game_id);
        }

        Ok(result)
    }

    /// `loadPatchManifest(gameId, step, archivePath, descriptors)`: манифест патча рядом с
    /// архивом (`.json` вместо `.7z`) или с того же источника; сверка с шагом цепочки
    /// (`checkPatchManifest`). Отвергнутый манифест не остаётся в Downloads.
    pub async fn load_patch_manifest(
        &self,
        game_id: &str,
        step: &Value,
        archive_path: &str,
        descriptors: &[Value],
    ) -> Result<Value, Failure> {
        let candidate = patch_manifest_path(archive_path);

        if !exists(Path::new(&candidate)) {
            let allowed = self.allowed_provider_types();
            let usable: Vec<Value> = descriptors
                .iter()
                .filter(|entry| {
                    jv::prop(entry, "type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| allowed.iter().any(|a| a == kind))
                })
                .cloned()
                .collect();

            if usable.is_empty() {
                return Err(DeliveryError::new(
                    Code::ManifestNotFound,
                    "Нет манифеста патча и нет источника",
                )
                .into());
            }

            let name = basename(&candidate);
            let artifact = format!("{game_id}:patch-manifest:{name}");
            let ordered = ManifestService::order_providers(&usable);
            let provider_options =
                ProviderOptions::with_allowed_provider_types(self.http.clone(), allowed.clone());
            let outcome = self
                .mirrors
                .run(
                    MirrorRequest {
                        artifact: &artifact,
                        descriptors: &ordered,
                        is_terminal_result: None,
                        on_discard_partial: None,
                    },
                    |attempt| {
                        let provider_options = &provider_options;
                        let name = &name;
                        async move {
                            let provider =
                                providers::create(&attempt.descriptor, provider_options)?;
                            let sibling = provider.resolve_sibling(name)?;
                            let response = self
                                .http
                                .request(
                                    &sibling.url,
                                    &RequestOptions {
                                        headers: sibling.headers.clone(),
                                        ..RequestOptions::default()
                                    },
                                )
                                .await?;
                            if response.status() != 200 {
                                let status = response.status();
                                let retry_after = response.header("retry-after");
                                drop(response);
                                return Err(DeliveryError::new(
                                    Code::ManifestNotFound,
                                    format!("Манифест патча недоступен: HTTP {status}"),
                                )
                                .with_details(json!({
                                    "status": status,
                                    "retryAfter": retry_after.filter(|r| !r.is_empty()),
                                }))
                                .into());
                            }
                            Ok(
                                read_body(response, Some(PATCH_MANIFEST_LIMIT as usize), None)
                                    .await?,
                            )
                        }
                    },
                )
                .await?;

            lock(&self.state).last_mirror_attempts = outcome.attempts.clone();
            crate::node_fs::write_bytes(Path::new(&candidate), &outcome.value)?;
        }

        let checked =
            crate::node_fs::read_text(Path::new(&candidate), crate::game_binds::Encoding::Utf8)
                .map_err(|error| error.message)
                .and_then(|raw| json_parse(&raw).map_err(|error| error.to_string()))
                .map_err(|message| {
                    DeliveryError::new(
                        Code::ManifestInvalid,
                        format!("Манифест патча не читается: {message}"),
                    )
                    .into()
                })
                .and_then(|data| self.check_patch_manifest(game_id, step, &data));
        if checked.is_err() {
            self.drop_patch_manifest(archive_path);
        }
        checked
    }
    /// `recoverPatch(gameId)`.
    pub fn recover_patch(
        &self,
        game_id: &str,
    ) -> Result<crate::patch_service::PatchRecovery, Failure> {
        self.patch_service.recover(game_id)
    }

    /// `pendingPatch(gameId)`.
    pub fn pending_patch(&self, game_id: &str) -> Option<Value> {
        self.patch_service.find_pending(game_id)
    }

    /// `recoverRepair(gameId)`.
    pub fn recover_repair(
        &self,
        game_id: &str,
    ) -> Result<crate::repair_service::RepairRecovery, Failure> {
        self.repair_service.recover(game_id)
    }

    /// `pendingRepair(gameId)`.
    pub fn pending_repair(&self, game_id: &str) -> Result<Option<Value>, DeliveryError> {
        self.repair_service.find_pending(game_id)
    }

    // ------------------------------------------------------------ пауза и отмена

    /// `pause()`: пауза идущей загрузки.
    pub fn pause(&self) -> bool {
        self.download_manager.pause()
    }

    /// `cancel(gameId)`: отмена приостановленной загрузки этой игры, идущего экспорта или импорта,
    /// загрузки или установки.
    pub fn cancel(&self, game_id: Option<&str>) -> Result<bool, Failure> {
        self.cancel_with(game_id, None)
    }

    /// `cancel(gameId, {episode})`: `episode` — из карточки эпизода Ghosting на паузе, убрать
    /// архив только его.
    pub fn cancel_with(
        &self,
        game_id: Option<&str>,
        episode: Option<&str>,
    ) -> Result<bool, Failure> {
        let game_id = game_id.filter(|id| !id.is_empty());
        let active_id = {
            let state = lock(&self.state);
            state
                .slot
                .as_ref()
                .map(|slot| slot.game_id.clone())
                .or_else(|| state.active_game_id.clone())
        };

        if let Some(id) = game_id
            && self.state_name_of(id) == "paused"
            && active_id.as_deref() != Some(id)
        {
            let episode_archive =
                |file: &str| crate::episode_install::episode_of_archive(file).is_some();
            self.download_manager
                .discard_paused(id, Some(&episode_archive))?;
            self.set_operation(id, None);
            self.set_status(
                id,
                json!({
                    "state": "cancelled",
                    "percent": 0,
                    "bytesDownloaded": 0,
                    "chain": null,
                    "error": null,
                }),
            );
            self.refresh(id)?;
            return Ok(true);
        }

        if let Some(id) = game_id
            && active_id.as_deref() != Some(id)
            && self.discard_episode_archives(id, episode)?
        {
            return Ok(true);
        }

        if let (Some(id), Some(active)) = (game_id, active_id.as_deref())
            && id != active
        {
            return Ok(false);
        }

        let pack_cancel = lock(&self.state).pack_cancel.clone();
        if let Some(cancel) = pack_cancel {
            match cancel {
                PackCancel::Archive(handle) => handle.cancel(),
                PackCancel::Integrity => {
                    self.integrity_service.cancel();
                }
            }
            return Ok(true);
        }

        let repair_abort = lock(&self.state).repair_abort.clone();
        if let Some(abort) = repair_abort {
            abort.cancel();
            return Ok(true);
        }

        Ok(self.download_manager.cancel() || self.install_service.cancel())
    }

    /// `listPendingOperations()`.
    pub fn list_pending_operations(&self) -> Result<Vec<Value>, Failure> {
        self.install_service.list_pending_operations()
    }

    /// `cleanupPending(gameId)`.
    pub fn cleanup_pending(&self, game_id: &str) -> Result<Value, Failure> {
        let result = self.install_service.cleanup_pending(game_id)?;
        self.refresh(game_id)?;
        Ok(result.to_json())
    }
}

/// `archivePath.replace(/\.7z$/i, ".json")`.
fn patch_manifest_path(archive_path: &str) -> String {
    let len = archive_path.len();
    if len >= 3
        && archive_path.is_char_boundary(len - 3)
        && archive_path[len - 3..].eq_ignore_ascii_case(".7z")
    {
        format!("{}.json", &archive_path[..len - 3])
    } else {
        archive_path.to_owned()
    }
}

// ---------------------------------------------------------------- удаление и .hlsrpack

impl GameDeliveryService {
    /// `uninstallGame(gameId, options)`: без подтверждения — что будет удалено.
    pub async fn uninstall_game(&self, game_id: &str, confirmed: bool) -> Result<Value, Failure> {
        let _slot = self.reserve_slot(game_id, "uninstall")?;
        self.run_uninstall_game(game_id, confirmed)
    }

    fn run_uninstall_game(&self, game_id: &str, confirmed: bool) -> Result<Value, Failure> {
        self.assert_game_not_running(game_id)?;

        let plan = game_removal::plan_removal(&self.paths, game_id)?;

        if !confirmed {
            let targets: Vec<Value> = plan
                .targets
                .iter()
                .map(|target| json!({"kind": target.kind, "bytes": target.bytes}))
                .collect();
            return Ok(json!({
                "status": "confirmation-required",
                "gameId": plan.game_id,
                "installPath": text(&plan.install_path),
                "version": self.installed_version_of(&plan.game_id),
                "totalBytes": plan.total_bytes,
                "targets": targets,
            }));
        }

        let result = {
            let _activity = self.activate(&plan.game_id, "uninstall");
            game_removal::remove_game(&self.paths, &plan.game_id)
        }?;

        self.update_checker.invalidate(&plan.game_id);
        lock(&self.state).last_integrity.remove(&plan.game_id);

        if result.partial {
            self.logger.warn(&format!(
                "[game-delivery] удаление {}: не удалось удалить {} путей, первый — {}",
                plan.game_id,
                result.failed.len(),
                result.failed[0].path
            ));
        } else {
            self.logger.info(&format!(
                "[game-delivery] игра {} удалена, освобождено {} байт",
                plan.game_id, plan.total_bytes
            ));
        }

        self.set_status(
            &plan.game_id,
            json!({
                "state": "not-installed",
                "percent": 0,
                "error": null,
                "integrity": null,
                "chain": null,
            }),
        );

        let status = self.refresh(&plan.game_id)?;
        let leftovers: Vec<&String> = result
            .failed
            .iter()
            .take(10)
            .map(|item| &item.path)
            .collect();

        Ok(json!({
            "status": if result.partial { "partial" } else { "removed" },
            "gameId": plan.game_id,
            "freedBytes": result.freed_bytes,
            "removed": result.removed,
            "leftovers": leftovers,
            "installPath": text(&plan.install_path),
            "delivery": status,
        }))
    }

    /// `exportPack(gameId, targetPath, extra)`: полная сборка в архив `.hlsrpack`. Игра должна
    /// быть закрыта: иначе в архив попали бы файлы посреди записи.
    pub async fn export_pack(
        &self,
        game_id: &str,
        target_path: &Path,
        extra: PackExtra,
    ) -> Result<Value, Failure> {
        let slot = self.reserve_slot(game_id, "export")?;
        let work = assert_inside(
            &self.paths.staging,
            &join(&[
                &text(&self.paths.staging),
                &format!("{game_id}-export-{}", crate::install_service::random_hex4()),
            ]),
            Some("Папка экспорта"),
        )?;
        let partial = PathBuf::from(format!("{}.hlsr-partial", text(target_path)));

        let result: Result<Value, Failure> = async {
            self.assert_game_not_running(game_id)?;
            if manifests::is_depot_build(game_id) {
                return Err(DeliveryError::new(
                    Code::PackDepotBuild,
                    "Сборку из Steam целиком не перенести",
                )
                .into());
            }
            let Some(installed) = self.read_installed(game_id)? else {
                return Err(
                    DeliveryError::new(Code::NotInstalled, "Сборка не установлена")
                        .with_details(json!({"gameId": game_id}))
                        .into(),
                );
            };

            // Изменённые файлы сборки (кастомный HUD поверх файлов игры) едут как есть, но сперва
            // игрок узнаёт, что при импорте они переедут (restorePackOriginals).
            if !extra.allow_modified {
                let modified = self.pack_modified_count(game_id, &installed).await?;
                if modified != 0 {
                    return Err(DeliveryError::new(
                        Code::PackModified,
                        format!("В сборке изменено файлов: {modified}"),
                    )
                    .with_details(json!({
                        "modified": modified,
                        "custom": game_pack::custom_layer_of(game_id).is_some(),
                    }))
                    .into());
                }
            }

            let install_path = PathBuf::from(jv::template(jv::prop(&installed, "installPath")));
            let collected =
                game_pack::collect(&install_path).map_err(game_pack::CollectError::into_failure)?;
            let files = collected.files.len();
            let bytes = collected.bytes;
            crate::node_fs::mkdir_recursive(&work)?;
            let meta_file = join(&[&text(&work), game_pack::META_FILE]);
            let meta = game_pack::meta(
                &game_pack::MetaInput {
                    launcher_id: extra.launcher_id.clone(),
                    build: extra.build.clone(),
                    launcher_version: extra.launcher_version.clone(),
                    settings: extra.settings.clone(),
                    installed: Some(installed.clone()),
                    files,
                    bytes,
                    exported_at: None,
                },
                self.now_ms(),
            );
            crate::node_fs::write_bytes(&meta_file, stringify(&meta, " ").as_bytes())?;

            self.set_active(Some(game_id));
            self.set_operation(game_id, Some("export"));
            self.set_status(
                game_id,
                json!({
                    "state": "exporting",
                    "percent": 0,
                    "bytesDownloaded": 0,
                    "totalBytes": bytes,
                    "chain": null,
                    "error": null,
                }),
            );

            crate::file_ops::rm_sync(&partial, false, 0, Duration::from_millis(100))?;
            let me = self.me.clone();
            let id = game_id.to_owned();
            let on_progress: PercentFn = Arc::new(move |percent: u32| {
                if let Some(service) = me.upgrade() {
                    let done = js::math_round(bytes as f64 * f64::from(percent) / 100.0);
                    service.set_status(
                        &id,
                        json!({"percent": percent, "bytesDownloaded": js_number(done)}),
                    );
                }
            });
            let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
            let job = self.archive_service.create(
                &partial,
                CreateOptions {
                    cwd: Some(install_path.clone()),
                    files: collected.files.clone(),
                    head_files: vec![text(&meta_file)],
                    list_dir: Some(work.clone()),
                    threads: Some(threads as f64),
                    on_progress: Some(on_progress),
                },
            );
            lock(&self.state).pack_cancel = Some(PackCancel::Archive(job.cancel_handle()));
            job.await?;

            crate::file_ops::rm_sync(target_path, false, 0, Duration::from_millis(100))?;
            std::fs::rename(&partial, target_path).map_err(|error| {
                RawError::fs_with_dest(&error, "rename", &partial, Some(target_path))
            })?;
            let size = std::fs::metadata(target_path)
                .map_err(|error| RawError::fs(&error, "stat", target_path))?
                .len();
            self.logger.info(&format!(
                "[game-delivery] {game_id}: сборка сохранена в {} ({files} файлов, {size} байт)",
                text(target_path)
            ));
            Ok(json!({
                "status": "ok",
                "gameId": game_id,
                "files": files,
                "bytes": bytes,
                "size": size,
                "path": text(target_path),
            }))
        }
        .await;

        let result = result.inspect_err(|error| {
            if let Err(cleanup) =
                crate::file_ops::rm_sync(&partial, false, 0, Duration::from_millis(100))
            {
                self.logger.warn(&format!(
                    "[game-delivery] не удалось удалить {}: {}",
                    text(&partial),
                    cleanup.message
                ));
            }
            if code_of(error).as_deref() == Some("CANCELLED") {
                self.set_status(
                    game_id,
                    json!({"state": "cancelled", "percent": 0, "error": null}),
                );
            }
        });

        self.finish_pack(game_id, &work, slot)?;
        result
    }

    /// `finally` экспорта и импорта: снять отмену, убрать рабочую папку, освободить слот и
    /// обновить статус.
    fn finish_pack(&self, game_id: &str, work: &Path, slot: SlotGuard<'_>) -> Result<(), Failure> {
        lock(&self.state).pack_cancel = None;
        if exists(work)
            && let Err(error) = remove_within_zone(&self.paths.staging, work)
        {
            self.logger.warn(&format!(
                "[game-delivery] не удалось удалить {}: {}",
                text(work),
                message_of(&error)
            ));
        }
        {
            let mut state = lock(&self.state);
            if state.active_game_id.as_deref() == Some(game_id) {
                state.active_game_id = None;
            }
        }
        self.set_operation(game_id, None);
        drop(slot);
        self.refresh(game_id)?;
        Ok(())
    }

    /// `readPackMeta(archivePath)`: сведения из архива `.hlsrpack` — до вопроса о замене
    /// установленной сборки.
    pub async fn read_pack_meta(&self, archive_path: &Path) -> Result<PackMeta, Failure> {
        let text = match self
            .archive_service
            .read_text(archive_path, game_pack::META_FILE, Some(PACK_META_LIMIT))
            .await
        {
            Ok(text) => text,
            Err(error) if error.code == Code::AntivirusBlocked => return Err(error.into()),
            Err(error) => {
                return Err(DeliveryError::new(
                    Code::PackBadFormat,
                    format!("Не архив сборки HLSR: {}", error.message),
                )
                .into());
            }
        };
        let is_known = |game_id: &str| manifests::is_known_game(game_id);
        let is_depot = |game_id: &str| manifests::is_depot_build(game_id);
        let mut meta = game_pack::parse_meta(
            &text,
            &game_pack::ParseOptions {
                is_known_game: Some(&is_known),
                is_depot_build: Some(&is_depot),
            },
        )
        .map_err(|error| error.to_delivery())?;
        meta.installed_version = Some(self.installed_version_of(&meta.game_id));
        Ok(meta)
    }

    /// `importPack(archivePath, meta)`: установка сборки из архива. Файлы сборки сверяются с
    /// подписанным списком версии; изменённые заменяются оригиналами с сервера
    /// (`restore_pack_originals`); посторонние и неподтверждённые исполняемые файлы
    /// отбрасываются. Установленная сборка заменяется целиком, только после проверки.
    pub async fn import_pack(
        &self,
        archive_path: &Path,
        meta: &PackMeta,
    ) -> Result<Value, Failure> {
        let game_id = meta.game_id.as_str();
        let slot = self.reserve_slot(game_id, "import")?;
        let staging_path = assert_inside(
            &self.paths.staging,
            &join(&[
                &text(&self.paths.staging),
                &format!("{game_id}-import-{}", crate::install_service::random_hex4()),
            ]),
            Some("Папка импорта"),
        )?;
        let mut locked = false;

        let result: Result<Value, Failure> = async {
            self.assert_game_not_running(game_id)?;
            let manifest = self.manifest_for(game_id)?.manifest;
            let files_manifest = self
                .resolve_files_manifest(game_id, Some(&meta.version))
                .await?;
            let Some(listed) = jv::prop(&files_manifest, "files").and_then(Value::as_array) else {
                return Err(DeliveryError::new(
                    Code::PackVersionUnknown,
                    format!("Нет списка файлов версии {}", meta.version),
                )
                .with_details(json!({"version": meta.version}))
                .into());
            };
            let listed = listed.len();

            self.set_active(Some(game_id));
            self.set_operation(game_id, Some("import"));
            let meta_bytes = meta.bytes.filter(|b| *b != 0.0).unwrap_or(0.0);
            self.set_status(
                game_id,
                json!({
                    "state": "importing",
                    "percent": 0,
                    "bytesDownloaded": 0,
                    "totalBytes": js_number(meta_bytes),
                    "chain": null,
                    "error": null,
                }),
            );

            self.install_service.check_writable()?;
            let inspection = self
                .archive_service
                .inspect(
                    archive_path,
                    &InspectArgs {
                        max_entries: Some(game_pack::MAX_ENTRIES as f64),
                        max_uncompressed_size: Some(game_pack::MAX_UNCOMPRESSED_BYTES as f64),
                        root_prefix: None,
                    },
                )
                .await?;
            let uncompressed = inspection.report.uncompressed_size;
            let total = if uncompressed != 0.0 && !uncompressed.is_nan() {
                uncompressed
            } else {
                meta_bytes
            };
            self.set_status(game_id, json!({"totalBytes": js_number(total)}));
            self.install_service.check_space(if uncompressed != 0.0 && !uncompressed.is_nan() {
                uncompressed
            } else {
                1.0
            })?;
            self.install_service.acquire_lock(game_id)?;
            locked = true;

            crate::node_fs::mkdir_recursive(&staging_path)?;
            let me = self.me.clone();
            let id = game_id.to_owned();
            let on_progress: FractionFn = Arc::new(move |fraction: f64| {
                if let Some(service) = me.upgrade() {
                    service.set_status(
                        &id,
                        json!({
                            "percent": js_number(js::math_round(fraction * 100.0)),
                            "bytesDownloaded": js_number(js::math_round(total * fraction)),
                        }),
                    );
                }
            });
            let job = self.archive_service.extract(
                archive_path,
                &staging_path,
                ExtractOptions {
                    on_progress: Some(on_progress),
                },
            );
            lock(&self.state).pack_cancel = Some(PackCancel::Archive(job.cancel_handle()));
            job.await?;
            // Служебный файл архива — не часть сборки: в проверке он был бы «лишним».
            crate::file_ops::rm_sync(
                &join(&[&text(&staging_path), game_pack::META_FILE]),
                false,
                0,
                Duration::from_millis(100),
            )?;

            lock(&self.state).pack_cancel = Some(PackCancel::Integrity);
            self.set_status(
                game_id,
                json!({"state": "checking-files", "percent": 0, "checked": 0, "totalChecked": listed}),
            );
            let preserve = preserve_paths(&manifest);
            let check = self
                .integrity_service
                .check(IntegrityRequest {
                    game_id,
                    install_path: &staging_path,
                    files_manifest: &files_manifest,
                    preserve_paths: &preserve,
                    stock_configs: false,
                })
                .await?;
            lock(&self.state).pack_cancel = None;
            let modified = match check.ok {
                true => None,
                false => Some(
                    self.restore_pack_originals(
                        game_id,
                        &meta.version,
                        &staging_path,
                        &files_manifest,
                        &check,
                    )
                    .await?,
                ),
            };

            self.set_status(game_id, json!({"state": "finalizing"}));
            let dependencies: Vec<game_pack::DependencyFiles> = manifests::launch_config_of(game_id)
                .map(|config| config.dependencies)
                .unwrap_or(&[])
                .iter()
                .map(|dependency| game_pack::DependencyFiles {
                    target: dependency.target.to_owned(),
                    version_file: dependency.version_file.to_owned(),
                })
                .collect();
            // Скачанные оригиналы подтверждены списком версии, как и прошедшие проверку файлы.
            let mut valid = check.valid.clone();
            if let Some(modified) = &modified {
                valid.extend(modified.restored_paths.iter().cloned());
            }
            let dropped = game_pack::sanitize(
                &staging_path,
                &game_pack::SanitizeCheck {
                    extra: check.extra.clone(),
                    valid,
                },
                &dependencies,
            )?;
            if !dropped.is_empty() {
                self.logger.warn(&format!(
                    "[game-delivery] {game_id}: из архива отброшено файлов — {}: {}",
                    dropped.len(),
                    dropped.iter().take(10).cloned().collect::<Vec<_>>().join(", ")
                ));
            }

            self.assert_game_not_running(game_id)?;
            let install_path = game_install_path(&self.paths, game_id)?;
            let target = InstallTarget {
                game_id: game_id.to_owned(),
                version: json!(meta.version),
                install_path,
                staging_path: staging_path.clone(),
                published_at: meta.published_at.clone(),
                manifest_sha256: meta.manifest_sha256.clone(),
            };
            self.install_service.swap_into_library(&target, &staging_path)?;
            let mut installed = self.install_service.write_installed(
                &target,
                meta.archive_sha256.as_ref().map_or(Value::Null, |sha| json!(sha)),
                None,
            )?;
            self.refresh_build_metadata_snapshot(&mut installed);

            self.update_checker.invalidate(game_id);
            lock(&self.state).last_integrity.remove(game_id);
            self.logger.info(&format!(
                "[game-delivery] {game_id}: сборка {} импортирована из {}, файлов сборки проверено {}, отброшено {}{}",
                meta.version,
                text(archive_path),
                check.checked,
                dropped.len(),
                modified.as_ref().map_or(String::new(), |m| format!(
                    ", изменённых {}, оригиналов скачано {}",
                    m.changed, m.restored
                ))
            ));
            let install_text = text(&target.install_path);
            let where_ = |relative: &str| text(&join(&[&install_text, relative]));
            let modified = modified.map(|m| {
                json!({
                    "changed": m.changed,
                    "restored": m.restored,
                    "custom": m.custom,
                    "customDir": match (m.custom, game_pack::custom_layer_of(game_id)) {
                        (0, _) | (_, None) => Value::Null,
                        (_, Some(dir)) => json!(where_(dir)),
                    },
                    "backup": m.backup,
                    "backupDir": if m.backup == 0 { Value::Null } else { json!(where_(game_pack::IMPORTED_BACKUP)) },
                    "rejected": m.rejected,
                })
            });
            Ok(json!({
                "status": "ok",
                "gameId": game_id,
                "version": meta.version,
                "dropped": dropped.len(),
                "modified": modified,
            }))
        }
        .await;

        let result = result.inspect_err(|error| {
            if exists(&staging_path)
                && let Err(cleanup) = remove_within_zone(&self.paths.staging, &staging_path)
            {
                self.logger.warn(&format!(
                    "[game-delivery] не удалось убрать {}: {}",
                    text(&staging_path),
                    message_of(&cleanup)
                ));
            }
            if code_of(error).as_deref() == Some("CANCELLED") {
                self.set_status(
                    game_id,
                    json!({"state": "cancelled", "percent": 0, "error": null}),
                );
            }
        });

        lock(&self.state).pack_cancel = None;
        if locked {
            self.install_service.release_lock();
        }
        {
            let mut state = lock(&self.state);
            if state.active_game_id.as_deref() == Some(game_id) {
                state.active_game_id = None;
            }
        }
        self.set_operation(game_id, None);
        drop(slot);
        self.refresh(game_id)?;
        result
    }

    // ------------------------------------------------------------ процессы игр

    /// `processState(gameId)`.
    pub fn process_state(&self, game_id: &str) -> Value {
        self.processes.process_state(game_id)
    }

    /// `isGameRunning(gameId)`.
    pub fn is_game_running(&self, game_id: &str) -> bool {
        self.processes.is_running(game_id)
    }

    /// `processService.anyRunning()`: запущенная игра.
    pub fn any_running(&self) -> Option<String> {
        self.processes.any_running()
    }

    /// `stopGame(gameId, options)`: остановка игры и статус.
    pub async fn stop_game(&self, game_id: &str, force: bool) -> Result<Value, Failure> {
        let result = self.process_stopper.request_stop(game_id, force).await?;
        self.refresh(game_id)?;
        Ok(result)
    }

    /// `reconcileProcesses(gameIds)`: игры, пережившие перезапуск лаунчера, и их статусы.
    pub fn reconcile_processes(&self, game_ids: &[String]) -> Vec<Value> {
        let results = self.processes.reconcile_after_launcher_restart(game_ids);
        for item in &results {
            if let Some(game_id) = item.get("gameId").and_then(Value::as_str) {
                let _ = self.refresh(game_id);
            }
        }
        // После сверки процессов известно, идёт ли игра: висящий config.cfg эпизода (крах игры
        // или лаунчера) возвращается, только если она не запущена.
        self.return_episode_configs(crate::episode_install::GAME_ID, None);
        results
    }

    /// Хук `renderActive`: ставит фасад, когда инструмент «Демо» поднят.
    pub fn set_render_active(&self, hook: Option<RenderActive>) {
        *lock(&self.render_active) = hook;
    }

    /// `dispose()`: служба процессов останавливается.
    pub fn dispose(&self) {
        self.processes.dispose();
    }

    /// `describeFlag()`.
    pub fn describe_flag(&self) -> feature_flag::Description {
        feature_flag::describe(&self.env)
    }

    /// `describeChannel(gameId)`.
    pub fn describe_channel(&self, game_id: &str) -> crate::channel_config::Description {
        crate::channel_config::describe(game_id, &self.env)
    }
}
