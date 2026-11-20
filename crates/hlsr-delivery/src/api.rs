//! Фасад доставки (`game-delivery/index.js` — каналы подпроекта 2, `GameActions.js`, части
//! `AppManager.js` о библиотеке и загрузках): по методу на канал IPC окна, события окна —
//! [`DeliveryEvent`] через `broadcast`.
//!
//! Аргументы каналов — значения JSON, как их присылает окно (`undefined` — `None` или `null`);
//! ответы — те же объекты, что у 0.4.7. Служба доставки создаётся при первом обращении
//! (`ensureService`) фабрикой [`ServiceFactory`]; ошибка создания запоминается (`initError`) и
//! возвращается каналами. Фоновая работа (проверки обновлений, тренировочные сейвы) идёт задачами
//! tokio; [`DeliveryApi::settle`] ждёт их завершения.
//!
//! Запуск игр, процессы, Steam, профиль, статистика, трей и снимок LiveSplit — подпроекты 3–5:
//! их каналов здесь нет, а события процессов приходят через [`DeliveryApi::process_state_changed`]
//! и [`DeliveryApi::tool_state_changed`].

mod custom_packs;
mod demos;
mod episodes;
mod files;
mod library;
mod player_config;
mod settings;
mod tools;
mod transfer;

pub use library::{LIBRARY_FOLDER_NAME, LibraryStore, with_hlsr_folder};

use crate::cfg_search::{CfgSearch, SteamLanguage};
use crate::download_settings::DownloadSettings;
use crate::environment::Environment;
use crate::error_map;
use crate::errors::{Code, DeliveryError, Failure, RawError, to_delivery_error};
use crate::feature_flag;
use crate::game_actions::{GameActions, LauncherIdentity};
use crate::game_delivery_service::{GameDeliveryService, RepairCall};
use crate::game_settings_store::GameSettingsStore;
use crate::js_value as jv;
use crate::library_migration::{self, LegacyGame};
use crate::library_recovery;
use crate::manifests;
use crate::mirror_runner::Clock;
use crate::ports::{
    Achievements, Dialogs, FreeSpace, Logger, Notifier, OverlayConfig, PresenceSink,
    Registry as WinRegistry, Shell, Telemetry,
};
use crate::practice_saves::PracticeSaves;
use crate::progress_states;
use crate::qa_launch;
use crate::qa_mode;
use crate::release_notes::{ReleaseNotesStore, StoreOptions};
use crate::tool_service::ToolService;
use hlsr_core::js::Platform;
use hlsr_core::json_text::{js_number, stringify};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// Каналы событий окна.
pub mod channels {
    pub const STATUS: &str = "game-delivery-status";
    pub const SELECT_GAME: &str = "select-game";
    pub const TOOL_STATUS: &str = "tool-status";
    pub const TOOL_PROGRESS: &str = "tool-progress";
    pub const BUILD_IMPORT_APPLIED: &str = "game-build-import-applied";
}

/// `UPDATE_TICK_MS`: плановая проверка обновлений.
pub const UPDATE_TICK: Duration = Duration::from_millis(5 * 60 * 1000);
/// `TICK_JITTER_MS`.
pub const UPDATE_TICK_JITTER_MS: u64 = 30 * 1000;
/// Сколько событий окна ждёт медленного получателя.
const EVENT_CAPACITY: usize = 4096;

/// Событие окна: канал и данные, как `webContents.send(channel, payload)`.
#[derive(Debug, Clone, PartialEq)]
pub struct DeliveryEvent {
    pub channel: &'static str,
    pub payload: Value,
}

/// Подсистемы, с которыми говорит фасад; у каждой есть заглушка в [`crate::ports`].
#[derive(Clone)]
pub struct ApiPorts {
    pub logger: Arc<dyn Logger>,
    /// `notify(code, detail)`: уровень по каталогу уведомлений выбирает получатель.
    pub notifier: Arc<dyn Notifier>,
    /// `telemetry`: нет — события не пишутся.
    pub telemetry: Option<Arc<dyn Telemetry>>,
    /// `onlineProfiles.achieve`.
    pub achievements: Option<Arc<dyn Achievements>>,
    /// `onPresenceSignal`.
    pub presence: Option<Arc<dyn PresenceSink>>,
    /// `liveSplitOverlay`: нет каталога данных — нет оверлея.
    pub overlay: Option<Arc<dyn OverlayConfig>>,
    pub dialogs: Arc<dyn Dialogs>,
    pub shell: Arc<dyn Shell>,
    /// Реестр Windows: язык Steam при поиске cfg, разрешение Source.
    pub registry: Arc<dyn WinRegistry>,
    /// Свободное место (`getFreeSpaceMb` окна выбора библиотеки).
    pub free_space: Arc<dyn FreeSpace>,
    /// `protonRunner.prefixFor(gameId)`: префикс Proton игры (Linux).
    pub proton_prefix: Option<ProtonPrefix>,
}

/// `protonRunner.prefixFor(gameId)`: папка префикса Proton игры.
pub type ProtonPrefix = Arc<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;

/// Что фасад знает о лаунчере (`options` `new GameDelivery`, `app.getPath`).
pub struct ApiOptions {
    /// `libraryPath` из `library.json`.
    pub library_path: String,
    pub launcher_version: String,
    /// `userDataPath`: нет — нет настроек игр, заметок и отложенного импорта.
    pub user_data_path: Option<PathBuf>,
    pub env: Environment,
    /// `app.getPath("desktop")`.
    pub desktop: PathBuf,
    pub launcher: LauncherIdentity,
    pub ports: ApiPorts,
    /// Часы (`Date.now()`), мс; по умолчанию — системные.
    pub clock: Option<Clock>,
    /// Создание службы доставки (`ensureService`); `None` — служба задаётся
    /// [`DeliveryApi::with_parts`].
    pub factory: Option<ServiceFactory>,
    /// `download-settings.json` и общий лимит скорости.
    pub download_settings: Option<Arc<Mutex<DownloadSettings>>>,
    /// `library.json` (`Store` окна); нет — каналы библиотеки отвечают ошибкой.
    pub library_store: Option<Arc<dyn LibraryStore>>,
    /// Ограничение запуска плановых проверок (тесты).
    pub schedule_updates: bool,
}

/// Служба доставки и то, что создаётся вместе с ней.
#[derive(Clone)]
pub struct Parts {
    pub service: Arc<GameDeliveryService>,
    /// `tools`: подсистема инструментов не поднялась — `None`.
    pub tools: Option<Arc<ToolService>>,
    pub practice_saves: Option<Arc<PracticeSaves>>,
}

/// Что нужно фабрике службы.
pub struct ServiceRequest {
    /// Библиотека после `recoverLibraryPath` (или библиотека QA).
    pub library_path: String,
    /// QA-режим: копии манифестов и файловые манифесты QA.
    pub registry: Option<crate::game_delivery_service::Registry>,
    pub release_notes: Option<Arc<ReleaseNotesStore>>,
}

/// `ensureService`: служба доставки, инструменты и сейвы для библиотеки.
pub type ServiceFactory = Box<dyn Fn(ServiceRequest) -> Result<Parts, Failure> + Send + Sync>;

/// Аргументы канала: отсутствующий — `undefined`.
#[derive(Debug, Clone, Copy)]
pub struct Args<'a>(pub &'a [Value]);

impl<'a> Args<'a> {
    /// Аргумент или `null`, если его нет.
    pub fn get(&self, index: usize) -> &'a Value {
        self.0.get(index).unwrap_or(&Value::Null)
    }

    /// Аргумент; нет — `None` (`undefined`).
    pub fn opt(&self, index: usize) -> Option<&'a Value> {
        self.0.get(index)
    }
}

/// Ожидающий подтверждения импорт `.hlsrpack`: путь к архиву остаётся здесь.
struct PendingPack {
    token: String,
    file: PathBuf,
}

#[derive(Default)]
struct State {
    parts: Option<Parts>,
    actions: Option<Arc<GameActions>>,
    init_error: Option<DeliveryError>,
    library_path: String,
    legacy_layout: Vec<LegacyGame>,
    selected_builds: HashMap<String, String>,
    last_selected_game_id: String,
    /// Сплиты LiveSplit по играм (`gameId` → id набора) из окна запуска.
    live_split_choices: Vec<(String, String)>,
    pending_pack: Option<PendingPack>,
    /// `pendingPackExport`: экспорт с изменёнными файлами ждёт подтверждения (C).
    pending_pack_export: Option<transfer::PendingPackExport>,
    full_set_reached: bool,
    update_timer: Option<JoinHandle<()>>,
    /// `launchService.launching.size`: идёт ли запуск игры (ставит тот, кто держит запуск).
    launching: Option<LaunchingProbe>,
    /// `editedPrevious`: прежние бинды игр, пока в файле игрока остаются правленые блоки (A).
    edited_previous: HashMap<String, Value>,
    /// `demoPackBusy`: идёт упаковка демо (D).
    demo_pack_busy: bool,
}

/// Идёт ли запуск какой-нибудь игры (`launchService.launching`).
pub type LaunchingProbe = Arc<dyn Fn() -> bool + Send + Sync>;

/// Фасад доставки.
pub struct DeliveryApi {
    weak: Weak<DeliveryApi>,
    options: ApiOptions,
    state: Mutex<State>,
    settings_store: Option<Arc<GameSettingsStore>>,
    /// Поиск cfg (язык Steam из реестра).
    cfg_search: CfgSearch,
    /// Чтение-запись `game-settings.json` идут по одному (в 0.4.7 — один поток).
    settings_lock: Mutex<()>,
    release_notes: Option<Arc<ReleaseNotesStore>>,
    events: broadcast::Sender<DeliveryEvent>,
    background: Mutex<Vec<JoinHandle<()>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `Object.assign({status}, result)`: `status` первым, поля ответа — после (свой `status` ответа
/// заменяет значение на том же месте).
pub(crate) fn with_status(status: &str, result: Value) -> Value {
    let mut out = Map::new();
    out.insert("status".into(), json!(status));
    if let Value::Object(fields) = result {
        for (key, value) in fields {
            out.insert(key, value);
        }
    }
    Value::Object(out)
}

/// `{status: "error", code}`.
pub(crate) fn error_code(code: &str) -> Value {
    json!({"status": "error", "code": code})
}

/// Опрос будущего один раз сейчас: синхронная часть `async`-функции 0.4.7 выполняется до
/// возврата из обработчика.
fn poll_now<F: Future + ?Sized>(future: Pin<&mut F>) -> Poll<F::Output> {
    let mut context = Context::from_waker(Waker::noop());
    future.poll(&mut context)
}

impl DeliveryApi {
    /// Фасад без службы: она создаётся фабрикой при первом обращении.
    pub fn new(options: ApiOptions) -> Arc<DeliveryApi> {
        let library_path = options.library_path.clone();
        let release_notes = options.user_data_path.as_ref().and_then(|dir| {
            ReleaseNotesStore::new(StoreOptions {
                dir: dir.clone(),
                logger: Some(options.ports.logger.clone()),
                clock: options.clock.clone(),
            })
            .ok()
            .map(Arc::new)
        });
        let settings_store = options.user_data_path.as_ref().map(|dir| {
            Arc::new(GameSettingsStore::new(
                GameSettingsStore::default_file(dir),
                options.ports.logger.clone(),
            ))
        });
        Arc::new_cyclic(|weak| DeliveryApi {
            weak: weak.clone(),
            state: Mutex::new(State {
                library_path,
                last_selected_game_id: "ghosting-renewed".to_owned(),
                ..State::default()
            }),
            settings_store,
            cfg_search: CfgSearch::new(SteamLanguage::new(options.ports.registry.clone())),
            settings_lock: Mutex::new(()),
            release_notes,
            events: broadcast::channel(EVENT_CAPACITY).0,
            background: Mutex::new(Vec::new()),
            options,
        })
    }

    /// Фасад с готовой службой (как `controller.service = …` в сверке): без шагов
    /// [`DeliveryApi::initialize_service`].
    pub fn with_parts(options: ApiOptions, parts: Parts) -> Arc<DeliveryApi> {
        let api = DeliveryApi::new(options);
        api.adopt(parts);
        api
    }

    /// Служба создана снаружи (`controller.service = …`): шаги
    /// [`DeliveryApi::initialize_service`] — отдельно.
    pub fn set_parts(&self, parts: Parts) {
        self.adopt(parts);
    }

    /// Заметки о выпуске (`ensureReleaseNotes`): ими же пользуется служба.
    pub fn release_notes(&self) -> Option<Arc<ReleaseNotesStore>> {
        self.release_notes.clone()
    }

    /// Получатель событий окна.
    pub fn subscribe(&self) -> broadcast::Receiver<DeliveryEvent> {
        self.events.subscribe()
    }

    pub(crate) fn logger(&self) -> &dyn Logger {
        self.options.ports.logger.as_ref()
    }

    pub(crate) fn ports(&self) -> &ApiPorts {
        &self.options.ports
    }

    pub(crate) fn now_ms(&self) -> i64 {
        match &self.options.clock {
            Some(clock) => clock(),
            None => std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_millis() as i64),
        }
    }

    pub fn settings_store(&self) -> Option<&Arc<GameSettingsStore>> {
        self.settings_store.as_ref()
    }

    pub(crate) fn settings_guard(&self) -> MutexGuard<'_, ()> {
        lock(&self.settings_lock)
    }

    pub(crate) fn cfg_search(&self) -> &CfgSearch {
        &self.cfg_search
    }

    pub(crate) fn live_split_choices(&self) -> Vec<(String, String)> {
        lock(&self.state).live_split_choices.clone()
    }

    pub(crate) fn set_live_split_choices(&self, choices: Vec<(String, String)>) {
        lock(&self.state).live_split_choices = choices;
    }

    pub(crate) fn set_pending_pack(&self, token: String, file: PathBuf) {
        lock(&self.state).pending_pack = Some(PendingPack { token, file });
    }

    /// Ожидающий импорт забирается при любом ответе (одноразовый токен).
    pub(crate) fn take_pending_pack(&self) -> Option<(String, PathBuf)> {
        lock(&self.state)
            .pending_pack
            .take()
            .map(|pending| (pending.token, pending.file))
    }

    /// `processService.anyRunning()`: запущенная игра.
    pub(crate) fn running_game(&self, service: &GameDeliveryService) -> Option<String> {
        service.any_running()
    }

    pub(crate) fn user_data_path(&self) -> Option<&PathBuf> {
        self.options.user_data_path.as_ref()
    }

    // ------------------------------------------------------------ события и уведомления

    /// `emitTo(channel, payload)`.
    pub(crate) fn emit_to(&self, channel: &'static str, payload: Value) {
        // Нет получателей — окно закрыто: событие никому не нужно.
        let _ = self.events.send(DeliveryEvent { channel, payload });
    }

    /// `emitPresence(signal)`.
    pub(crate) fn emit_presence(&self, signal: Value) {
        let Some(presence) = &self.options.ports.presence else {
            return;
        };
        if let Err(message) = presence.signal(&signal) {
            self.logger().warn(&format!(
                "[discord-rpc] сигнал presence не обработан: {message}"
            ));
        }
    }

    /// `notify(code, detail)`.
    pub(crate) fn notify(&self, code: &str, detail: &str) {
        self.options.ports.notifier.notify(code, detail);
    }

    /// `telemetryEmit(type, fields)`.
    pub(crate) fn telemetry_emit(&self, event_type: &str, fields: Value) {
        let Some(telemetry) = &self.options.ports.telemetry else {
            return;
        };
        if let Err(message) = telemetry.emit(event_type, &fields) {
            self.logger().warn(&format!(
                "[telemetry] событие {event_type} не записано: {message}"
            ));
        }
    }

    /// `achieve(id)`.
    pub(crate) fn achieve(&self, id: &str) {
        if let Some(achievements) = &self.options.ports.achievements {
            achievements.achieve(id);
        }
    }

    // ------------------------------------------------------------ фоновые задачи

    /// Запуск фоновой работы так, как вызов `async`-функции без `await` в 0.4.7: синхронная
    /// часть — сейчас, остальное — задачей tokio.
    pub(crate) fn start_background(&self, future: impl Future<Output = ()> + Send + 'static) {
        let mut future = Box::pin(future);
        if poll_now(future.as_mut()).is_ready() {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            self.logger()
                .warn("[game-delivery] фоновая задача не запущена: нет среды tokio");
            return;
        };
        let task = handle.spawn(future);
        let mut background = lock(&self.background);
        background.retain(|task| !task.is_finished());
        background.push(task);
    }

    /// Дождаться фоновых задач (и тех, что они запустили).
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

    pub fn check_for_updates_if_due(
        &self,
        service: &Arc<GameDeliveryService>,
        game_id: &str,
        reason: &str,
    ) {
        let service = service.clone();
        let game_id = game_id.to_owned();
        let reason = reason.to_owned();
        self.start_background(async move {
            service
                .check_for_updates_if_due(&game_id, Some(&reason))
                .await;
        });
    }

    // ------------------------------------------------------------ служба

    /// Служба, если создана.
    pub(crate) fn parts(&self) -> Option<Parts> {
        lock(&self.state).parts.clone()
    }

    pub fn service(&self) -> Option<Arc<GameDeliveryService>> {
        lock(&self.state)
            .parts
            .as_ref()
            .map(|parts| parts.service.clone())
    }

    pub fn tools(&self) -> Option<Arc<ToolService>> {
        lock(&self.state)
            .parts
            .as_ref()
            .and_then(|parts| parts.tools.clone())
    }

    fn actions(&self) -> Option<Arc<GameActions>> {
        lock(&self.state).actions.clone()
    }

    fn init_error(&self) -> Option<DeliveryError> {
        lock(&self.state).init_error.clone()
    }

    /// Служба и события: `service.on("status", send)`, действия меню.
    fn adopt(&self, parts: Parts) {
        let weak = self.weak.clone();
        parts.service.on_status(move |status| {
            if let Some(api) = weak.upgrade() {
                api.send(status);
            }
        });
        let actions = GameActions::new(
            parts.service.paths().clone(),
            self.options.ports.shell.clone(),
            self.options.desktop.clone(),
            self.options.launcher.clone(),
            self.options.ports.logger.clone(),
        );
        let mut state = lock(&self.state);
        state.actions = Some(Arc::new(actions));
        state.parts = Some(parts);
    }

    /// `ensureService()`: служба или `None` (тогда есть `initError`).
    pub fn ensure_service(&self) -> Option<Arc<GameDeliveryService>> {
        {
            let state = lock(&self.state);
            if let Some(parts) = &state.parts {
                return Some(parts.service.clone());
            }
            if state.init_error.is_some() {
                return None;
            }
        }
        let Some(factory) = &self.options.factory else {
            return None;
        };
        let env = &self.options.env;
        let qa = qa_mode::is_enabled(env);
        let created = (|| -> Result<Parts, Failure> {
            let current = lock(&self.state).library_path.clone();
            let (library_path, registry) = if qa {
                let bundled = crate::game_delivery_service::Registry::bundled();
                let now = hlsr_core::js::iso_string(
                    std::time::UNIX_EPOCH + Duration::from_millis(self.now_ms().max(0) as u64),
                );
                let registry = crate::game_delivery_service::Registry {
                    manifests: qa_mode::manifests_for(&bundled.manifests, &now, env)?,
                    files: qa_mode::files_manifests_for(&bundled.files, env)?,
                };
                (qa_mode::library_path(env)?, Some(registry))
            } else {
                let fallback = crate::library_path::default_library_path();
                let recovered = library_recovery::recover_library_path(
                    Some(&current),
                    Some(&fallback.to_string_lossy()),
                    &manifests::game_ids(),
                    self.logger(),
                );
                (recovered.unwrap_or(current), None)
            };
            if qa {
                self.logger().warn(&format!(
                    "[game-delivery] ВКЛЮЧЁН QA-РЕЖИМ ({}). Библиотека: {library_path}, версия \
                     сборки: {}. Это режим для локальной проверки, в production он недоступен.",
                    qa_mode::QA_FLAG,
                    qa_mode::QA_VERSION
                ));
            }
            factory(ServiceRequest {
                library_path,
                registry,
                release_notes: self.release_notes.clone(),
            })
        })();
        match created {
            Ok(parts) => {
                let service = parts.service.clone();
                self.adopt(parts);
                self.initialize_service();
                Some(service)
            }
            Err(failure) => {
                let error = to_delivery_error(failure, Some(Code::PathInvalid));
                self.logger().error(&format!(
                    "[game-delivery] сервис не инициализирован: {}",
                    error.message
                ));
                lock(&self.state).init_error = Some(error);
                None
            }
        }
    }

    /// Шаги `ensureService` после создания службы: старая раскладка библиотеки, статусы,
    /// восстановление незавершённых операций, проверки обновлений при запуске и по таймеру.
    pub fn initialize_service(&self) {
        let Some(service) = self.service() else {
            return;
        };
        // G: свежесть канала игр — в userData, не в библиотеке (`channelFreshnessFile`).
        if let Some(dir) = self.user_data_path() {
            service.set_channel_freshness_file(dir.join("game-channel-freshness.json"));
        }
        let root = service.paths().root.to_string_lossy().into_owned();
        let legacy = library_migration::detect_legacy_layout(&root, &manifests::game_ids())
            .unwrap_or_default();
        if !legacy.is_empty() {
            let from: Vec<String> = legacy
                .iter()
                .map(|game| game.from.to_string_lossy().into_owned())
                .collect();
            self.logger().warn(&format!(
                "[game-delivery] обнаружена старая раскладка библиотеки: {}",
                from.join(", ")
            ));
            let ids: Vec<&str> = legacy.iter().map(|game| game.game_id.as_str()).collect();
            self.notify("LIBRARY_LEGACY_LAYOUT", &ids.join(", "));
        }
        lock(&self.state).legacy_layout = legacy;

        if let Err(error) = service.refresh_all() {
            self.logger().warn(&format!(
                "[game-delivery] статусы не обновлены: {}",
                to_delivery_error(error, None).message
            ));
        }
        self.recover_pending(&service);
        service.reconcile_processes(&[]);
        self.schedule_update_checks(&service);
    }

    /// `scheduleUpdateChecks()`: проверка при запуске, затем каждые 5 минут (+ до 30 с).
    fn schedule_update_checks(&self, service: &Arc<GameDeliveryService>) {
        for game_id in manifests::game_ids() {
            self.check_for_updates_if_due(service, game_id, "launcher-start");
        }
        if !self.options.schedule_updates {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let weak = self.weak.clone();
        let timer = handle.spawn(async move {
            loop {
                let jitter = rand_jitter_ms();
                tokio::time::sleep(UPDATE_TICK + Duration::from_millis(jitter)).await;
                let Some(api) = weak.upgrade() else {
                    return;
                };
                let Some(service) = api.service() else {
                    return;
                };
                for game_id in manifests::game_ids() {
                    api.check_for_updates_if_due(&service, game_id, "scheduled");
                }
            }
        });
        if let Some(previous) = lock(&self.state).update_timer.replace(timer) {
            previous.abort();
        }
    }

    /// `recoverPending()`: незавершённые обновления, патчи и ремонт.
    fn recover_pending(&self, service: &GameDeliveryService) {
        let log = self.logger();
        for game_id in manifests::game_ids() {
            let full_update = service.pending_full_update(game_id);
            if let Some(pending) = &full_update {
                log.warn(&format!(
                    "[game-delivery] найдена незавершённая транзакция обновления {game_id} \
                     ({} -> {}, фаза {})",
                    jv::template(pending.get("fromVersion")),
                    jv::template(pending.get("toVersion")),
                    jv::template(pending.get("phase"))
                ));
                match service.recover_full_update(game_id) {
                    Ok(result) => log.info(&format!(
                        "[game-delivery] восстановление обновления: {}",
                        stringify(&result.to_json(), "")
                    )),
                    Err(error) => log.error(&format!(
                        "[game-delivery] восстановление обновления не удалось: {error}"
                    )),
                }
            }

            let patch = service.pending_patch(game_id);
            if let Some(pending) = &patch {
                let detail = if jv::truthy(pending.get("corrupt")) {
                    "с повреждённым журналом".to_owned()
                } else {
                    format!(
                        "({} -> {}, фаза {})",
                        jv::template(pending.get("fromVersion")),
                        jv::template(pending.get("toVersion")),
                        jv::template(pending.get("phase"))
                    )
                };
                log.warn(&format!(
                    "[game-delivery] найдена незавершённая транзакция патча {game_id} {detail}"
                ));
                match service.recover_patch(game_id) {
                    Ok(result) => log.info(&format!(
                        "[game-delivery] восстановление патча: {}",
                        stringify(&result.to_json(), "")
                    )),
                    Err(error) => log.error(&format!(
                        "[game-delivery] восстановление патча не удалось: {error}"
                    )),
                }
            }

            let repair = service.pending_repair(game_id).ok().flatten();
            if let Some(pending) = &repair {
                log.warn(&format!(
                    "[game-delivery] найден незавершённый ремонт {game_id} (фаза {})",
                    jv::template(pending.get("phase"))
                ));
                match service.recover_repair(game_id) {
                    Ok(result) => log.info(&format!(
                        "[game-delivery] восстановление после ремонта: {}",
                        stringify(&result.to_json(), "")
                    )),
                    Err(error) => {
                        log.error(&format!("[game-delivery] откат ремонта не удался: {error}"))
                    }
                }
            }

            // Эпизоды, отложенные подменой сборки, которую прервал крах (`park_episodes`).
            match crate::paths::game_install_path(service.paths(), game_id) {
                Ok(install_path) => service
                    .install_service()
                    .unpark_episodes(game_id, &install_path),
                Err(error) => log.error(&format!(
                    "[game-delivery] эпизоды {game_id} не вернулись: {}",
                    error.message
                )),
            }

            if full_update.is_some() || patch.is_some() || repair.is_some() {
                let _ = service.refresh(game_id);
            }
        }
        service.cleanup_orphan_staging();
    }

    // ------------------------------------------------------------ статусы

    /// `isStatusVisible(status)`: у игры с несколькими сборками видна только выбранная.
    fn is_status_visible(&self, status: &Value) -> bool {
        let Some(game_id) = status.get("gameId").filter(|id| jv::truthy(Some(id))) else {
            return true;
        };
        let game_id = jv::template(Some(game_id));
        let Some(launcher_id) = manifests::launcher_id_of(&game_id) else {
            return true;
        };
        let builds = manifests::builds_of(launcher_id);
        if builds.len() < 2 {
            return true;
        }
        self.resolve_game_id(&json!(launcher_id)).as_deref() == Some(game_id.as_str())
    }

    /// `send(status)`: событие статуса для окна и presence.
    fn send(&self, status: &Value) {
        let state = status.get("state").map(|state| jv::template(Some(state)));
        if state.as_deref().is_some_and(progress_states::is_success) {
            self.check_full_set();
            if let Some(game_id) = status.get("gameId").and_then(Value::as_str) {
                self.apply_pending_build_import(game_id);
            }
        }
        if !self.is_status_visible(status) {
            return;
        }
        self.emit_to(channels::STATUS, status.clone());
        if status.is_object() {
            let mut signal = Map::new();
            signal.insert("type".into(), json!("status"));
            for key in ["gameId", "state"] {
                if let Some(value) = status.get(key) {
                    signal.insert(key.into(), value.clone());
                }
            }
            self.emit_presence(Value::Object(signal));
        }
    }

    /// `checkFullSet()`: установлены все игры с манифестом — достижение «Полный комплект».
    fn check_full_set(&self) {
        let Some(service) = self.service() else {
            return;
        };
        if lock(&self.state).full_set_reached {
            return;
        }
        let mut all = true;
        for game_id in manifests::game_ids() {
            match service.install_service().read_installed(game_id) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    all = false;
                    break;
                }
                Err(error) => {
                    self.logger().warn(&format!(
                        "[profile] не удалось проверить установленные игры: {}",
                        error.message
                    ));
                    return;
                }
            }
        }
        if !all {
            return;
        }
        lock(&self.state).full_set_reached = true;
        self.achieve("full-set");
    }

    /// Событие процесса игры (подсистема запуска): статус пересчитывается.
    pub fn process_state_changed(&self, game_id: &str) {
        if let Some(service) = self.service() {
            service.process_state_changed(game_id);
        }
    }

    // ------------------------------------------------------------ игры и сборки

    /// `resolveGameId(id)`: игра, выбранная сборка игры лаунчера или игра по id лаунчера.
    pub fn resolve_game_id(&self, id: &Value) -> Option<String> {
        let value = jv::template(Some(id));
        if manifests::is_known_game(&value) {
            return Some(value);
        }
        if let Some(selected) = lock(&self.state).selected_builds.get(&value) {
            return Some(selected.clone());
        }
        manifests::BY_LAUNCHER_ID
            .iter()
            .find(|(launcher_id, _)| *launcher_id == value)
            .map(|(_, game_id)| (*game_id).to_owned())
    }

    /// `requireGameId(id)`.
    fn require_game_id(&self, id: &Value) -> Result<String, Failure> {
        self.resolve_game_id(id).ok_or_else(|| {
            RawError::new(format!("Неизвестная игра: {}", jv::template(Some(id)))).into()
        })
    }

    pub(crate) fn select_build_of(&self, launcher_id: &str, game_id: &str) {
        lock(&self.state)
            .selected_builds
            .insert(launcher_id.to_owned(), game_id.to_owned());
    }

    /// `selectBuild(launcherId, build)`.
    fn select_build(&self, launcher_id: &Value, build: &Value) -> Value {
        let key = jv::template(Some(launcher_id));
        let build_text = jv::template(Some(build));
        let Some(game_id) = manifests::build_game_id_of(&key, &build_text) else {
            return json!({
                "status": "unknown-build",
                "launcherId": key,
                "build": build_text,
                "builds": manifests::builds_of(&key),
            });
        };
        self.select_build_of(&key, game_id);
        self.logger().info(&format!(
            "[game-delivery] выбрана сборка {build_text} игры {key} -> {game_id}"
        ));
        json!({"status": "ok", "launcherId": key, "build": build_text, "gameId": game_id})
    }

    /// `installedPathOf(gameId)`.
    pub(crate) fn installed_path_of(&self, game_id: &str) -> Option<PathBuf> {
        let service = self.service()?;
        let installed = service.install_service().read_installed(game_id).ok()??;
        installed
            .get("installPath")
            .filter(|path| jv::truthy(Some(path)))
            .map(|path| PathBuf::from(jv::template(Some(path))))
    }

    /// `readInstalled(gameId)` службы.
    pub(crate) fn read_installed(&self, game_id: &str) -> Option<Value> {
        self.service()?
            .install_service()
            .read_installed(game_id)
            .ok()
            .flatten()
    }

    pub(crate) fn is_game_running(&self, game_id: &str) -> bool {
        self.service()
            .is_some_and(|service| service.is_game_running(game_id))
    }

    // ------------------------------------------------------------ каналы: статусы

    /// `game-delivery-info`.
    pub fn game_delivery_info(&self) -> Value {
        self.ensure_service();
        let env = &self.options.env;
        let service = self.service();
        let flag = feature_flag::describe(env);
        let qa = match qa_mode::describe(env) {
            Ok(value) => value,
            Err(error) => error.to_public(),
        };
        let library_path = match &service {
            Some(service) => service.paths().root.to_string_lossy().into_owned(),
            None => lock(&self.state).library_path.clone(),
        };
        let games: Vec<Value> = manifests::game_ids()
            .into_iter()
            .map(|game_id| {
                let channel = crate::channel_config::describe(game_id, env);
                json!({
                    "gameId": game_id,
                    "launcherId": manifests::launcher_id_of(game_id),
                    "channel": {
                        "gameId": channel.game_id,
                        "configured": channel.configured,
                        "source": channel.source.map(|source| source.as_str()),
                        "requireSignature": channel.require_signature,
                        "trustedKeys": channel.trusted_keys,
                    },
                })
            })
            .collect();
        let pending = service
            .as_ref()
            .and_then(|service| service.list_pending_operations().ok())
            .unwrap_or_default();
        let state = lock(&self.state);
        json!({
            "flag": {
                "flag": flag.flag,
                "remoteEnabled": flag.remote_enabled,
                "releaseProfileActive": flag.release_profile_active,
                "localHttpAllowed": flag.local_http_allowed,
                "allowedProviderTypes": flag.allowed_provider_types,
            },
            "qa": qa,
            "qaLaunch": qa_launch::describe(env),
            "libraryPath": library_path,
            "games": games,
            "pending": pending,
            "legacyLayout": state.legacy_layout.iter()
                .map(|game| json!({"gameId": game.game_id, "version": game.version}))
                .collect::<Vec<_>>(),
            "error": state.init_error.as_ref().map(DeliveryError::to_public),
        })
    }

    /// `game-delivery-get-status`.
    pub fn game_delivery_get_status(&self, id: Option<&Value>) -> Value {
        let Some(service) = self.ensure_service() else {
            return self
                .init_error()
                .map_or(Value::Null, |error| json!({"error": error.to_public()}));
        };
        match id.filter(|id| !id.is_null()) {
            None => service.refresh_all().map_or(Value::Null, Value::from),
            Some(id) => match self.resolve_game_id(id) {
                Some(game_id) => service.refresh(&game_id).unwrap_or(Value::Null),
                None => Value::Null,
            },
        }
    }

    /// `game-delivery-install`.
    pub async fn game_delivery_install(&self, id: &Value) -> Value {
        let Some(service) = self.ensure_service() else {
            let error = self.init_error().unwrap_or_else(|| {
                to_delivery_error(RawError::new("").into(), Some(Code::PathInvalid))
            });
            self.logger().error(&format!(
                "[game-delivery] установка невозможна: {} {}",
                error.code, error.message
            ));
            self.notify(error.code.as_str(), &error.message);
            return json!({"status": "error", "error": error.to_public()});
        };
        let Some(game_id) = self.resolve_game_id(id) else {
            return json!({"status": "unknown-game"});
        };
        match service.install_to_latest(&game_id).await {
            Ok(result) => {
                if result.get("status").and_then(Value::as_str) == Some("installed") {
                    self.ensure_practice_saves(&game_id);
                }
                result
            }
            Err(failure) => {
                let error = to_delivery_error(failure, Some(Code::InstallFailed));
                self.logger().error(&format!(
                    "[game-delivery] установка {game_id} не удалась: {} {}",
                    error.code, error.message
                ));
                self.notify(error.code.as_str(), &error.message);
                json!({"status": "error", "error": error.to_public()})
            }
        }
    }

    /// `ensurePracticeSaves(gameId)`: тренировочные сейвы HL1 докачиваются в фоне.
    pub fn ensure_practice_saves(&self, game_id: &str) {
        let Some(parts) = self.parts() else {
            return;
        };
        let Some(saves) = parts.practice_saves.clone() else {
            return;
        };
        if saves.pack_for(game_id).is_none() {
            return;
        }
        let Ok(installed) = parts.service.installed_of(game_id) else {
            return;
        };
        let install_path = PathBuf::from(jv::template(installed.get("installPath")));
        let logger = self.options.ports.logger.clone();
        let game_id = game_id.to_owned();
        self.start_background(async move {
            if let Err(error) = saves.ensure(&game_id, &install_path).await {
                logger.warn(&format!(
                    "[saves] тренировочные сейвы {game_id} не скачались: {}",
                    to_delivery_error(error, None).message
                ));
            }
        });
    }

    /// `game-delivery-pause`.
    pub fn game_delivery_pause(&self) -> Value {
        json!(self.service().is_some_and(|service| service.pause()))
    }

    /// `game-delivery-cancel`: отмена; почти скачанная игра — достижение «Почти». `options` —
    /// `{episode}` из карточки эпизода на паузе: убрать архив только его.
    pub fn game_delivery_cancel(&self, id: &Value, options: &Value) -> Value {
        let Some(service) = self.service() else {
            return json!(false);
        };
        let game_id = jv::template(Some(id));
        let before = service.status_of(&game_id);
        let episode = options.get("episode").and_then(Value::as_str);
        let cancelled = service
            .cancel_with(Some(&game_id), episode)
            .unwrap_or(false);
        let state = before.get("state").and_then(Value::as_str);
        let percent = before.get("percent").map_or(f64::NAN, |percent| {
            hlsr_core::js::to_number(&jv::template(Some(percent)))
        });
        if cancelled && matches!(state, Some("downloading" | "paused")) && percent >= 99.0 {
            self.achieve("almost");
        }
        json!(cancelled)
    }

    /// `game-delivery-cleanup`.
    pub fn game_delivery_cleanup(&self, id: &Value) -> Value {
        let Some(service) = self.ensure_service() else {
            return json!({"removed": false});
        };
        match self.resolve_game_id(id) {
            Some(game_id) => service
                .cleanup_pending(&game_id)
                .unwrap_or_else(|_| json!({"removed": false})),
            None => json!({"removed": false}),
        }
    }

    /// `game-select-build`.
    pub fn game_select_build(&self, id: &Value, build: &Value) -> Value {
        if self
            .ensure_service()
            .is_some_and(|service| service.is_busy())
        {
            return json!({"status": "busy", "gameId": self.resolve_game_id(id)});
        }
        let mut result = self.select_build(id, build);
        if result.get("status").and_then(Value::as_str) == Some("ok") {
            // Discord (0.4.9): смена сборки — новый `selected` с id сборки.
            self.emit_presence(json!({"type": "selected", "gameId": result.get("gameId")}));
        }
        if result.get("status").and_then(Value::as_str) == Some("ok")
            && let Some(service) = self.ensure_service()
        {
            let game_id = jv::template(result.get("gameId"));
            self.check_for_updates_if_due(&service, &game_id, "build-selected");
            let delivery = service.refresh(&game_id).unwrap_or(Value::Null);
            result["delivery"] = delivery;
        }
        result
    }

    /// `game-delivery-selected`: игра выбрана в окне.
    pub fn game_delivery_selected(&self, id: &Value) -> Value {
        let game_id = self.resolve_game_id(id);
        // `gameId || id` (0.4.9): id сборки приводится к игре.
        let signal = game_id
            .as_ref()
            .map_or_else(|| id.clone(), |game_id| json!(game_id));
        self.emit_presence(json!({"type": "selected", "gameId": signal}));
        let Some(game_id) = game_id else {
            return Value::Null;
        };
        let Some(service) = self.ensure_service() else {
            return Value::Null;
        };
        lock(&self.state).last_selected_game_id = game_id.clone();
        self.check_for_updates_if_due(&service, &game_id, "game-selected");
        service.refresh(&game_id).unwrap_or(Value::Null)
    }

    /// Последняя выбранная игра (`lastSelectedGameId`).
    pub fn last_selected_game_id(&self) -> String {
        lock(&self.state).last_selected_game_id.clone()
    }

    // ------------------------------------------------------------ каналы: меню игры

    /// `run(action)`: ответ `{status: "ok", …}` или ошибка с уведомлением.
    pub async fn run<F>(&self, action: impl FnOnce(Arc<GameDeliveryService>) -> F) -> Value
    where
        F: Future<Output = Result<Value, Failure>>,
    {
        let Some(service) = self.ensure_service() else {
            return json!({
                "status": "error",
                "error": self.init_error().map(|error| error.to_public()),
            });
        };
        match action(service).await {
            Ok(result) => with_status("ok", result),
            Err(failure) => {
                let error = to_delivery_error(failure, Some(Code::InstallFailed));
                self.logger().warn(&format!(
                    "[game-delivery] действие меню не выполнено: {} {}",
                    error.code, error.message
                ));
                self.notify(error.code.as_str(), &error.message);
                json!({"status": "error", "error": error.to_public()})
            }
        }
    }

    fn actions_or_fail(&self) -> Result<Arc<GameActions>, Failure> {
        self.actions()
            .ok_or_else(|| RawError::new("Cannot read properties of null").into())
    }

    /// `game-menu-state`.
    pub fn game_menu_state(&self, id: &Value) -> Value {
        let service = self.ensure_service();
        let game_id = self.resolve_game_id(id);
        let (Some(service), Some(game_id)) = (service, game_id) else {
            return json!({"available": false, "installed": false, "busy": false,
                          "running": false, "items": {}});
        };
        let installed = matches!(
            service.install_service().read_installed(&game_id),
            Ok(Some(_))
        );
        let busy = service.is_busy();
        let running = service.is_game_running(&game_id);
        let item =
            |enabled: bool, reason: Option<&str>| json!({"enabled": enabled, "reason": reason});
        let file_action = |needs_install: bool| {
            if running {
                item(false, Some("game-running"))
            } else if needs_install && !installed {
                item(false, Some("not-installed"))
            } else if busy {
                item(false, Some("busy"))
            } else {
                item(true, None)
            }
        };
        let steam_managed = manifests::is_depot_build(&game_id);
        let manifest_action = |needs_install: bool| {
            if steam_managed {
                item(false, Some("steam-managed"))
            } else {
                file_action(needs_install)
            }
        };
        // ---- B: проверка и ремонт по файлам включены по списку: у остальных игр пока нет зеркала.
        let file_repair = if steam_managed {
            manifest_action(true)
        } else if manifests::is_file_repair_game(&game_id) {
            file_action(true)
        } else {
            item(false, Some("in-development"))
        };
        let when_installed = || {
            if installed {
                item(true, None)
            } else {
                item(false, Some("not-installed"))
            }
        };
        json!({
            "available": true,
            "gameId": game_id,
            "installed": installed,
            "busy": busy,
            "running": running,
            "items": {
                // Без установки — выключены: «Персонализация» показывает только установленные
                // игры и открыла бы вместо этой прошлую.
                "settings": if !installed {
                    item(false, Some("not-installed"))
                } else if busy {
                    item(false, Some("busy"))
                } else {
                    item(true, None)
                },
                "shortcut": when_installed(),
                "screenshots": when_installed(),
                "folder": when_installed(),
                "updates": manifest_action(false),
                "verify": file_repair.clone(),
                "repair": file_repair,
                "exportBuild": if steam_managed { manifest_action(true) } else { file_action(true) },
                "exportSettings": when_installed(),
                "importBuild": if busy {
                    item(false, Some("busy"))
                } else if running {
                    item(false, Some("game-running"))
                } else {
                    item(true, None)
                },
                "uninstall": file_action(true),
            },
        })
    }

    /// `game-menu-open-folder`.
    pub async fn game_menu_open_folder(&self, id: &Value) -> Value {
        self.run(|_| async {
            let game_id = self.require_game_id(id)?;
            let actions = self.actions_or_fail()?;
            actions.open_game_folder(&json!(game_id)).await
        })
        .await
    }

    /// `game-menu-open-screenshots`.
    pub async fn game_menu_open_screenshots(&self, id: &Value) -> Value {
        self.run(|_| async {
            let game_id = self.require_game_id(id)?;
            let actions = self.actions_or_fail()?;
            actions.open_screenshots_folder(&json!(game_id)).await
        })
        .await
    }

    /// `game-menu-create-shortcut`.
    pub async fn game_menu_create_shortcut(&self, id: &Value) -> Value {
        self.run(|_| async {
            let game_id = self.require_game_id(id)?;
            let actions = self.actions_or_fail()?;
            Ok(actions.create_desktop_shortcut(&json!(game_id))?)
        })
        .await
    }

    /// `game-menu-check-updates`.
    pub async fn game_menu_check_updates(&self, id: &Value) -> Value {
        self.run(|service| async move {
            let game_id = self.require_game_id(id)?;
            service.check_for_updates(&game_id, true, None).await
        })
        .await
    }

    /// `game-menu-apply-update`.
    pub async fn game_menu_apply_update(&self, id: &Value) -> Value {
        self.run(|service| async move {
            let game_id = self.require_game_id(id)?;
            self.apply_update_with_telemetry(&service, &game_id).await
        })
        .await
    }

    /// `game-menu-verify-files`.
    pub async fn game_menu_verify_files(&self, id: &Value) -> Value {
        self.run(|service| async move {
            let game_id = self.require_game_id(id)?;
            let result = service.verify_files(&game_id).await?;
            Ok(GameDeliveryService::summarize_integrity(&result))
        })
        .await
    }

    /// `game-menu-repair`.
    pub async fn game_menu_repair(&self, id: &Value, options: &Value) -> Value {
        let confirmed = jv::truthy(options.get("confirmed"));
        let skip_recheck = options.get("recheck") == Some(&json!(false));
        // ---- B: выбор галочками; сервис берёт из них только то, что сам нашёл при проверке.
        let files = options.get("files").and_then(Value::as_array).map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        });
        self.run(|service| async move {
            let game_id = self.require_game_id(id)?;
            self.repair_with_telemetry(
                &service,
                &game_id,
                RepairCall {
                    confirmed,
                    skip_recheck,
                    files,
                },
            )
            .await
        })
        .await
    }

    /// `game-menu-uninstall`.
    pub async fn game_menu_uninstall(&self, id: &Value, options: &Value) -> Value {
        let confirmed = options.get("confirmed") == Some(&json!(true));
        self.run(|service| async move {
            let game_id = self.require_game_id(id)?;
            let result = service.uninstall_game(&game_id, confirmed).await?;
            if result.get("status").and_then(Value::as_str) == Some("confirmation-required") {
                return Ok(result);
            }
            let shortcut = match self
                .actions_or_fail()
                .and_then(|actions| Ok(actions.remove_desktop_shortcut(&json!(game_id))?))
            {
                Ok(shortcut) => shortcut,
                Err(failure) => {
                    self.logger().warn(&format!(
                        "[game-delivery] ярлык {game_id} не удалён: {}",
                        to_delivery_error(failure, None).message
                    ));
                    json!({"removed": false, "reason": "skipped"})
                }
            };
            let mut out = result;
            if let Value::Object(fields) = &mut out {
                fields.insert("shortcut".into(), shortcut);
            }
            Ok(out)
        })
        .await
    }

    /// `telemetryBuildMeta(gameId)`: версия и сборка установленной игры.
    fn telemetry_build_meta(&self, service: &GameDeliveryService, game_id: &str) -> (Value, Value) {
        let installed = service
            .install_service()
            .read_installed(game_id)
            .ok()
            .flatten();
        let field = |key: &str, fallback: &str| {
            installed
                .as_ref()
                .and_then(|installed| installed.get(key))
                .filter(|value| jv::truthy(Some(value)))
                .cloned()
                .unwrap_or_else(|| json!(fallback))
        };
        (field("version", "0.0.0"), field("buildId", "unknown"))
    }

    /// `applyUpdateWithTelemetry(gameId)`. План в 0.4.7 не читается (`service.updatePlan` нет):
    /// вид — «full», целевая версия — текущая.
    async fn apply_update_with_telemetry(
        &self,
        service: &GameDeliveryService,
        game_id: &str,
    ) -> Result<Value, Failure> {
        let (from_version, _) = self.telemetry_build_meta(service, game_id);
        let plan_kind = "full";
        let started = self.now_ms();
        self.telemetry_emit(
            "game_download_started",
            json!({"gameId": game_id, "fromVersion": from_version, "toVersion": from_version,
                   "planKind": plan_kind}),
        );
        match service.apply_update(game_id).await {
            Ok(result) => {
                let (to_version, _) = self.telemetry_build_meta(service, game_id);
                let bytes = to_byte_count(result.get("networkBytes"), None);
                let elapsed = json!((self.now_ms() - started).max(0));
                let duration = to_byte_count(result.get("durationMs"), Some(&elapsed));
                let patch_bytes = to_byte_count(result.get("patchBytes"), None);
                let full_bytes = to_byte_count(result.get("fullArchiveBytes"), None);
                let average = match (bytes, duration) {
                    (Some(bytes), Some(duration)) if duration > 0.0 => {
                        js_number(((bytes * 1000.0) / duration).floor())
                    }
                    _ => Value::Null,
                };
                let saved = match (patch_bytes, full_bytes) {
                    (Some(patch), Some(full)) => js_number((full - patch).max(0.0)),
                    _ => Value::Null,
                };
                self.telemetry_emit(
                    "game_download_completed",
                    json!({
                        "gameId": game_id,
                        "fromVersion": from_version,
                        "toVersion": to_version,
                        "planKind": plan_kind,
                        "downloadBytes": number_or_null(bytes),
                        "downloadDurationMilliseconds": number_or_null(duration),
                        "averageBytesPerSecond": average,
                        "patchBytes": number_or_null(patch_bytes),
                        "fullArchiveBytes": number_or_null(full_bytes),
                        "savedBytes": saved,
                    }),
                );
                self.telemetry_emit(
                    "game_update_installed",
                    json!({"gameId": game_id, "fromVersion": from_version,
                           "toVersion": to_version, "planKind": plan_kind}),
                );
                Ok(result)
            }
            Err(failure) => {
                let cancelled =
                    matches!(&failure, Failure::Delivery(error) if error.code == Code::Cancelled);
                let mut fields = json!({"gameId": game_id, "fromVersion": from_version,
                                        "toVersion": from_version, "planKind": plan_kind});
                let error_value = failure_value(&failure);
                if cancelled {
                    fields["downloadBytes"] =
                        number_or_null(to_byte_count(error_value.get("networkBytes"), None));
                } else {
                    fields["errorCode"] = json!(error_map::to_telemetry_code(&error_value));
                    fields["fallbackReason"] = json!(error_map::to_fallback_reason(&error_value));
                }
                self.telemetry_emit(
                    if cancelled {
                        "game_download_cancelled"
                    } else {
                        "game_update_failed"
                    },
                    fields,
                );
                Err(failure)
            }
        }
    }

    /// `repairWithTelemetry(gameId, options)`.
    async fn repair_with_telemetry(
        &self,
        service: &GameDeliveryService,
        game_id: &str,
        options: RepairCall,
    ) -> Result<Value, Failure> {
        let (game_version, _) = self.telemetry_build_meta(service, game_id);
        let started = self.now_ms();
        self.telemetry_emit(
            "game_repair_started",
            json!({"gameId": game_id, "gameVersion": game_version}),
        );
        match service.repair(game_id, options).await {
            Ok(result) => {
                let seconds = (self.now_ms() - started).max(0) / 1000;
                self.telemetry_emit(
                    "game_repair_completed",
                    json!({"gameId": game_id, "gameVersion": game_version,
                           "durationSeconds": seconds}),
                );
                Ok(result)
            }
            Err(failure) => {
                self.telemetry_emit(
                    "game_repair_failed",
                    json!({"gameId": game_id, "gameVersion": game_version,
                           "errorCode": error_map::to_telemetry_code(&failure_value(&failure))}),
                );
                Err(failure)
            }
        }
    }

    // ------------------------------------------------------------ каналы: заметки, библиотека

    /// `game-release-notes`.
    pub fn game_release_notes(&self, id: &Value, version: &Value) -> Value {
        if self.resolve_game_id(id).is_none() {
            return Value::Null;
        }
        let game_id = self.resolve_game_id(id).unwrap_or_default();
        let Some(version) = version.as_str().filter(|version| is_notes_version(version)) else {
            return Value::Null;
        };
        let Some(store) = &self.release_notes else {
            return Value::Null;
        };
        store.get(&game_id, version).map_or(Value::Null, |notes| {
            json!({"gameId": notes.game_id, "version": notes.version, "items": notes.items})
        })
    }

    /// `game-release-notes-all`.
    pub fn game_release_notes_all(&self) -> Value {
        let Some(store) = &self.release_notes else {
            return json!([]);
        };
        let games: Vec<Value> = store
            .list()
            .into_iter()
            .filter(|game| !game.versions.is_empty())
            .map(|game| {
                json!({
                    "gameId": game.game_id,
                    "launcherId": manifests::launcher_id_of(&game.game_id),
                    "versions": game.versions.iter()
                        .map(|entry| json!({"version": entry.version, "items": entry.items}))
                        .collect::<Vec<_>>(),
                })
            })
            .collect();
        Value::Array(games)
    }

    /// `game-library-migrate`: перенос игр из старой раскладки библиотеки.
    pub fn game_library_migrate(&self) -> Value {
        let Some(service) = self.ensure_service() else {
            return json!({"status": "error", "error": {"code": "PATH_INVALID"}});
        };
        let (legacy, library_path) = {
            let state = lock(&self.state);
            (state.legacy_layout.clone(), state.library_path.clone())
        };
        if legacy.is_empty() {
            return json!({"status": "ok", "migrated": []});
        }
        if service.is_busy() || self.any_game_running(&service) {
            return json!({"status": "error", "error": {"code": "GAME_RUNNING"}});
        }
        let mut migrated = Vec::new();
        for entry in legacy {
            match library_migration::migrate_legacy_game(&library_path, &entry.game_id) {
                Ok(result) => {
                    lock(&self.state)
                        .legacy_layout
                        .retain(|game| game.game_id != entry.game_id);
                    self.logger().info(&format!(
                        "[game-delivery] миграция {}: {} -> {}",
                        entry.game_id,
                        result.from.display(),
                        result.to.display()
                    ));
                    migrated.push(result.to_json());
                }
                Err(failure) => {
                    let error = to_delivery_error(failure, Some(Code::InstallFailed));
                    self.logger().error(&format!(
                        "[game-delivery] миграция {} не удалась: {}",
                        entry.game_id, error.message
                    ));
                    self.notify(
                        error.code.as_str(),
                        &format!("миграция {}: {}", entry.game_id, error.message),
                    );
                    return json!({"status": "error", "error": error.to_public(),
                                  "migrated": migrated});
                }
            }
        }
        let _ = service.refresh_all();
        json!({"status": "ok", "migrated": migrated})
    }

    pub(crate) fn any_game_running(&self, service: &GameDeliveryService) -> bool {
        service.any_running().is_some()
    }

    /// `selectGameFromArgs(argv)`: игра из аргумента `--select-game=` (ярлык на рабочем столе).
    pub fn select_game_from_args(&self, argv: &[String]) -> Value {
        let Some(game_id) = crate::game_actions::parse_select_game_arg(argv) else {
            return Value::Null;
        };
        let Some(launcher_id) = manifests::launcher_id_of(&game_id) else {
            return Value::Null;
        };
        let selected = json!({"gameId": game_id, "launcherId": launcher_id});
        self.emit_to(channels::SELECT_GAME, selected.clone());
        self.logger().info(&format!(
            "[game-delivery] выбрана игра из аргумента запуска: {game_id}"
        ));
        selected
    }

    /// `checkOnFocus()`: окно получило фокус — проверки обновлений, если прошлой больше 5 минут.
    pub fn check_on_focus(&self) {
        let Some(service) = self.service() else {
            return;
        };
        for game_id in manifests::game_ids() {
            let service = service.clone();
            self.start_background(async move {
                service.check_for_updates_on_focus(game_id).await;
            });
        }
    }

    /// `setLibraryPath(libraryPath)`: новая библиотека — новая служба.
    pub fn set_library_path(&self, library_path: &str) -> Option<Value> {
        if qa_mode::is_enabled(&self.options.env) {
            lock(&self.state).library_path = library_path.to_owned();
            return None;
        }
        if lock(&self.state).library_path == library_path {
            return None;
        }
        // Новая служба не видит работы старой: загрузка писала бы в прежнюю папку, «Пауза» и
        // «Отмена» уходили бы в пустую службу, игра пропала бы из виду. Проверка и замена —
        // под одной блокировкой: проверки служб берут только свои короткие блокировки.
        let previous = {
            let mut state = lock(&self.state);
            if Self::library_busy(&state) {
                return Some(json!({"ok": false, "error": "busy"}));
            }
            if let Some(timer) = state.update_timer.take() {
                timer.abort();
            }
            state.library_path = library_path.to_owned();
            state.actions = None;
            state.init_error = None;
            state.parts.take()
        };
        // Прежняя служба больше не следит за процессами игр прежней библиотеки.
        if let Some(previous) = previous {
            previous.service.dispose();
        }
        self.ensure_service();
        None
    }

    /// Идёт операция доставки, игра, установка базы без патчей (`silentVersionEvents`),
    /// инструмент или запуск игры: папку библиотеки менять нельзя.
    fn library_busy(state: &State) -> bool {
        let (parts, launching) = (&state.parts, &state.launching);
        let service_busy = parts.as_ref().is_some_and(|parts| {
            parts.service.is_busy()
                || parts.service.any_running().is_some()
                || parts.service.has_silent_version_events()
        });
        // shortcut: закрывающийся инструмент (stopping) не считается — `running` ToolService
        // снаружи не виден; станет важно, если закрытие LiveSplit начнёт тянуться.
        let tools_busy = parts
            .as_ref()
            .and_then(|parts| parts.tools.as_ref())
            .is_some_and(|tools| {
                tools.busy_with().is_some()
                    || crate::tools::ids()
                        .iter()
                        .any(|tool_id| tools.running_info(tool_id).is_some())
            });
        service_busy || tools_busy || launching.as_ref().is_some_and(|launching| launching())
    }

    /// `launchService` для отказа `set-library-path`: идёт ли запуск игры.
    pub fn set_launching_probe(&self, probe: LaunchingProbe) {
        lock(&self.state).launching = Some(probe);
    }

    /// `dispose()`: таймер проверок обновлений и служба (процессы игр).
    pub fn dispose(&self) {
        let service = {
            let mut state = lock(&self.state);
            if let Some(timer) = state.update_timer.take() {
                timer.abort();
            }
            state.parts.as_ref().map(|parts| parts.service.clone())
        };
        if let Some(service) = service {
            service.dispose();
        }
    }

    // ------------------------------------------------------------ каналы по имени

    /// Вызов канала окна по имени; неизвестный канал — `None`.
    pub async fn call(&self, channel: &str, args: &[Value]) -> Option<Value> {
        let a = Args(args);
        let value = match channel {
            "game-delivery-info" => self.game_delivery_info(),
            "game-delivery-get-status" => self.game_delivery_get_status(a.opt(0)),
            "game-delivery-install" => self.game_delivery_install(a.get(0)).await,
            "game-delivery-pause" => self.game_delivery_pause(),
            "game-delivery-cancel" => self.game_delivery_cancel(a.get(0), a.get(1)),
            "game-delivery-cleanup" => self.game_delivery_cleanup(a.get(0)),
            "game-menu-state" => self.game_menu_state(a.get(0)),
            "game-menu-open-folder" => self.game_menu_open_folder(a.get(0)).await,
            "game-menu-open-screenshots" => self.game_menu_open_screenshots(a.get(0)).await,
            "game-menu-create-shortcut" => self.game_menu_create_shortcut(a.get(0)).await,
            "game-menu-check-updates" => self.game_menu_check_updates(a.get(0)).await,
            "game-menu-apply-update" => self.game_menu_apply_update(a.get(0)).await,
            "game-menu-verify-files" => self.game_menu_verify_files(a.get(0)).await,
            "game-menu-repair" => self.game_menu_repair(a.get(0), a.get(1)).await,
            "game-menu-uninstall" => self.game_menu_uninstall(a.get(0), a.get(1)).await,
            "game-select-build" => self.game_select_build(a.get(0), a.get(1)),
            "game-delivery-selected" => self.game_delivery_selected(a.get(0)),
            "game-release-notes" => self.game_release_notes(a.get(0), a.get(1)),
            "game-release-notes-all" => self.game_release_notes_all(),
            "game-library-migrate" => self.game_library_migrate(),
            "game-settings-get" => self.game_settings_get(a.get(0)),
            // У окна на Rust нет `undefined` посреди аргументов: `null` вместо scope — «без scope».
            "game-settings-save" => self.game_settings_save(
                a.get(0),
                a.get(1),
                a.opt(2).filter(|scope| !scope.is_null()),
                a.get(3),
            ),
            "game-settings-reset" => self.game_settings_reset(a.get(0), a.get(1)),
            "game-personalization-pick-asset" => {
                self.game_personalization_pick_asset(a.get(0), a.get(1))
                    .await
            }
            "game-personalization-preview-asset" => {
                self.game_personalization_preview_asset(a.get(0), a.get(1))
                    .await
            }
            "game-personalization-code-export" => {
                self.game_personalization_code_export(a.get(0), a.get(1))
            }
            "game-personalization-code-import" => {
                self.game_personalization_code_import(a.get(0), a.get(1))
            }
            "ghosting-screen-get" => self.ghosting_screen_get(a.get(0)),
            // ---- A: единый конфиг игрока (0.4.9)
            "player-config-import-pick" => self.player_config_import_pick(a.get(0), a.get(1)).await,
            "player-config-import-apply" => self.player_config_import_apply(a.get(0), a.opt(1)),
            "player-config-import-undo" => self.player_config_import_undo(a.get(0)),
            "player-config-setup-state" => self.player_config_setup_state(a.get(0)),
            "player-config-setup-done" => self.player_config_setup_done(a.get(0), a.get(1)),
            "player-config-export" => self.player_config_export(a.get(0)).await,
            "player-config-check" => self.player_config_check(a.get(0), a.opt(1)),
            // ---- конец A
            // ---- E: эпизоды Ghosting (0.4.9)
            "game-episodes-status" => self.game_episodes_status().await,
            "game-episode-install" => self.game_episode_install(a.get(0)).await,
            "game-episode-remove" => self.game_episode_remove(a.get(0)).await,
            // ---- конец E
            // ---- D: архив демо (0.4.9)
            "game-demos-list" => self.game_demos_list(a.get(0)),
            "game-demos-pack" => self.game_demos_pack(a.get(0), a.get(1)).await,
            "game-demos-archive-reveal" => self.game_demos_archive_reveal(a.get(0)),
            // ---- конец D
            // ---- H: моды игр (0.4.9)
            "custom-packs-get" => self.custom_packs_get(a.get(0)).await,
            "custom-packs-set" => self.custom_packs_set(a.get(0), a.get(1), a.get(2)).await,
            "custom-packs-sound" => self.custom_packs_sound(a.get(0), a.get(1), a.get(2), a.get(3)),
            // ---- конец H
            "open-discord" => self.open_discord(),
            "open-external-link" => self.open_external_link(a.get(0)),
            "hl1-guide-open-link" => self.hl1_guide_open_link(a.get(0)),
            "game-runs-list" => self.game_runs_list(a.get(0)),
            "game-runs-delete" => self.game_runs_delete(a.get(0), a.get(1)),
            "game-runs-open-folder" => self.game_runs_open_folder(a.get(0)).await,
            "game-runs-reveal" => self.game_runs_reveal(a.get(0), a.get(1)),
            "game-config-info" => self.game_config_info(a.get(0), a.get(1)),
            "game-config-read" => self.game_config_read(a.get(0), a.get(1)),
            "game-config-write" => self.game_config_write(a.get(0), a.get(1), a.get(2), a.get(3)),
            "game-config-open-folder" => self.game_config_open_folder(a.get(0), a.get(1)).await,
            "game-cfg-files-list" => self.game_cfg_files_list(a.get(0), a.get(1)),
            "game-cfg-file-read" => self.game_cfg_file_read(a.get(0), a.get(1), a.get(2)),
            "game-cfg-file-write" => {
                self.game_cfg_file_write(a.get(0), a.get(1), a.get(2), a.get(3), a.get(4))
            }
            "game-build-export" => self.game_build_export(a.get(0)).await,
            "game-build-export-confirm" => self.game_build_export_confirm(a.get(0)).await,
            "game-build-export-settings" => self.game_build_export_settings(a.get(0)).await,
            "game-build-import" => self.game_build_import().await,
            "game-build-import-confirm" => self.game_build_import_confirm(a.get(0)).await,
            "tool-list" => self.tool_list(),
            "tool-status" => self.tool_status(a.get(0)),
            "tool-cancel" => self.tool_cancel(a.get(0)),
            "tool-install" => self.tool_install(a.get(0)).await,
            "tool-launch" => {
                self.tool_launch(a.get(0), a.get(1), a.get(2), a.get(3))
                    .await
            }
            "tool-stop" => self.tool_stop(a.get(0), a.get(1)).await,
            "tool-remove" => self.tool_remove(a.get(0)),
            "tool-splits-set" => self.tool_splits_set(a.get(0), a.get(1)),
            "tool-splits-presets" => self.tool_splits_presets(a.get(0)),
            "tool-open-folder" => self.tool_open_folder(a.get(0)).await,
            "tool-open-help" => self.tool_open_help(a.get(0)),
            "select-install-folder" => self.select_install_folder(a.get(0)).await,
            "download-settings-get" => self.download_settings_get(),
            "download-settings-set" => self.download_settings_set(a.get(0)),
            "validate-install-path" => self.validate_install_path(a.get(0)),
            "open-games-folder" => self.open_games_folder().await,
            "get-library-path" => self.get_library_path(),
            "set-library-path" => self.set_library_path_channel(a.get(0)),
            _ => return None,
        };
        Some(value)
    }
}

/// Версия в запросе заметок: `/^[0-9A-Za-z.\-+]{1,32}$/`.
fn is_notes_version(version: &str) -> bool {
    (1..=32).contains(&version.len())
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
}

/// `toByteCount(value, fallback)`: целое неотрицательное число или `null`.
fn to_byte_count(value: Option<&Value>, fallback: Option<&Value>) -> Option<f64> {
    let candidate = value.filter(|value| !value.is_null()).or(fallback)?;
    let number = candidate.as_f64()?;
    (number.is_finite() && number.fract() == 0.0 && number >= 0.0).then_some(number)
}

/// Число JS или `null`.
fn number_or_null(value: Option<f64>) -> Value {
    value.map_or(Value::Null, js_number)
}

/// Исключение как объект для `error-map.js` (`code`, `details`). `networkBytes` у ошибок 0.4.7
/// не бывает: у отменённой загрузки объём — `null`.
fn failure_value(failure: &Failure) -> Value {
    match failure {
        Failure::Delivery(error) => json!({
            "code": error.code.as_str(),
            "message": error.message,
            "details": error.details,
        }),
        Failure::Raw(error) => json!({"code": error.code, "message": error.message}),
    }
}

/// Случайная добавка к интервалу проверок (`Math.random() * TICK_JITTER_MS`).
fn rand_jitter_ms() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| u64::from(since.subsec_nanos()));
    nanos % UPDATE_TICK_JITTER_MS
}

/// Платформа фасада: эта ОС.
pub(crate) const PLATFORM: Platform = Platform::CURRENT;
