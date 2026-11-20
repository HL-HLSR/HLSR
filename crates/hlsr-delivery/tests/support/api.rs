//! Фасад доставки в тестах: фейки окна (диалоги, оболочка ОС, уведомления, телеметрия,
//! достижения, presence, оверлей LiveSplit), процессы игр, журнал и `DeliveryApi` с готовой службой
//! доставки без канала и сети. Зеркало фейков `tools/reference/delivery/delivery-api.js`.

use hlsr_delivery::api::{ApiOptions, ApiPorts, DeliveryApi, DeliveryEvent, LibraryStore, Parts};
use hlsr_delivery::archive_service::ArchiveService;
use hlsr_delivery::download_settings::DownloadSettings;
use hlsr_delivery::environment::Environment;
use hlsr_delivery::game_actions::LauncherIdentity;
use hlsr_delivery::game_delivery_service::{
    GameDeliveryService, Provided, Registry, ServiceOptions,
};
use hlsr_delivery::ports::{
    Achievements, Dialogs, FakeFreeSpace, FakeRegistry, FileFilter, Logger, Notifier, OpenDialog,
    OverlayConfig, PresenceSink, ProcessProbe, SaveDialog, Shell, ShortcutLink, SilentNotifier,
    Telemetry, VersionEventInfo, process_not_running,
};
use hlsr_delivery::providers::BoxFuture;
use hlsr_delivery::tool_service::{ToolOptions, ToolService};
use serde_json::{Map, Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast::Receiver;

use super::str_of;

fn iso(ms: i64) -> String {
    hlsr_delivery::timing::iso_from_ms(ms)
}

/// Вызовы фейков шага.
pub type Effects = Arc<Mutex<Vec<Value>>>;

pub fn record(effects: &Effects, value: Value) {
    effects.lock().unwrap().push(value);
}

/// Журнал: `{level, message}`.
pub struct ListLogger {
    pub entries: Arc<Mutex<Vec<(String, String)>>>,
}

impl ListLogger {
    fn push(&self, level: &str, message: &str) {
        self.entries
            .lock()
            .unwrap()
            .push((level.to_owned(), message.to_owned()));
    }
}

impl Logger for ListLogger {
    fn info(&self, message: &str) {
        self.push("info", message);
    }
    fn warn(&self, message: &str) {
        self.push("warn", message);
    }
    fn error(&self, message: &str) {
        self.push("error", message);
    }
}

/// Процессы игр: запущены игры из списка (по порядку).
#[derive(Default)]
pub struct FakeProcesses {
    pub running: Mutex<Vec<String>>,
    /// Время запуска игр, мс.
    pub now: i64,
}

impl ProcessProbe for FakeProcesses {
    fn process_state(&self, game_id: &str) -> Value {
        if !self.is_running(game_id) {
            return process_not_running(game_id);
        }
        json!({
            "gameId": game_id,
            "state": "running",
            "running": true,
            "since": iso(self.now),
            "startedAt": iso(self.now),
            "exitCode": null,
            "lastExit": null,
        })
    }

    fn is_running(&self, game_id: &str) -> bool {
        self.running.lock().unwrap().iter().any(|id| id == game_id)
    }

    fn any_running(&self) -> Option<String> {
        self.running.lock().unwrap().first().cloned()
    }
}

/// Диалоги: параметры — в вызовы, ответы — по очереди (`<root>` → временная папка).
pub struct FakeDialogs {
    pub effects: Effects,
    pub answers: Mutex<VecDeque<Value>>,
    pub root: PathBuf,
}

pub fn filters_json(filters: &[FileFilter]) -> Value {
    Value::Array(
        filters
            .iter()
            .map(|filter| json!({"name": filter.name, "extensions": filter.extensions}))
            .collect(),
    )
}

/// Параметры диалога открытия в порядке ключей 0.4.7.
pub fn open_json(options: &OpenDialog) -> Value {
    let mut out = Map::new();
    if let Some(title) = &options.title {
        out.insert("title".into(), json!(title));
    }
    out.insert("properties".into(), json!(options.properties));
    if let Some(path) = &options.default_path {
        out.insert("defaultPath".into(), json!(path));
    }
    if !options.filters.is_empty() {
        out.insert("filters".into(), filters_json(&options.filters));
    }
    Value::Object(out)
}

pub fn save_json(options: &SaveDialog) -> Value {
    let mut out = Map::new();
    if let Some(path) = &options.default_path {
        out.insert("defaultPath".into(), json!(path));
    }
    out.insert("filters".into(), filters_json(&options.filters));
    Value::Object(out)
}

impl FakeDialogs {
    fn fill(&self, text: &str) -> PathBuf {
        PathBuf::from(text.replace("<root>", &self.root.to_string_lossy()))
    }
}

impl Dialogs for FakeDialogs {
    fn show_open_dialog(&self, options: OpenDialog) -> BoxFuture<'_, Vec<PathBuf>> {
        record(
            &self.effects,
            json!({"dialog": "open", "options": open_json(&options)}),
        );
        let answer = self.answers.lock().unwrap().pop_front();
        let files: Vec<PathBuf> = answer
            .as_ref()
            .and_then(|answer| answer["open"].as_array())
            .map(|list| list.iter().map(|file| self.fill(str_of(file))).collect())
            .unwrap_or_default();
        Box::pin(async move { files })
    }

    fn show_save_dialog(&self, options: SaveDialog) -> BoxFuture<'_, Option<PathBuf>> {
        record(
            &self.effects,
            json!({"dialog": "save", "options": save_json(&options)}),
        );
        let answer = self.answers.lock().unwrap().pop_front();
        let file = answer
            .as_ref()
            .and_then(|answer| answer["save"].as_str())
            .filter(|file| !file.is_empty())
            .map(|file| self.fill(file));
        Box::pin(async move { file })
    }
}

/// Оболочка ОС: вызовы — в список, `openPath` отвечает заданной ошибкой.
pub struct FakeShell {
    pub effects: Effects,
    pub open_path_failure: Mutex<String>,
    /// Непустая строка — `openExternal` отказывает с этим текстом.
    pub open_external_failure: Mutex<String>,
}

impl Shell for FakeShell {
    fn open_path<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, String> {
        record(
            &self.effects,
            json!({"shell": "openPath", "path": path.to_string_lossy()}),
        );
        let failure = self.open_path_failure.lock().unwrap().clone();
        Box::pin(async move { failure })
    }

    fn open_external(&self, url: &str) -> Result<(), String> {
        record(&self.effects, json!({"shell": "openExternal", "url": url}));
        match self.open_external_failure.lock().unwrap().as_str() {
            "" => Ok(()),
            failure => Err(failure.to_owned()),
        }
    }

    fn show_item_in_folder(&self, path: &Path) {
        record(
            &self.effects,
            json!({"shell": "showItemInFolder", "path": path.to_string_lossy()}),
        );
    }

    fn write_shortcut_link(&self, _path: &Path, _link: &ShortcutLink) -> bool {
        false
    }

    fn read_shortcut_link(&self, _path: &Path) -> Result<ShortcutLink, String> {
        Err("нет ярлыка".to_owned())
    }
}

/// Уведомления, телеметрия, достижения, presence и оверлей — в список вызовов.
pub struct Recorder {
    pub effects: Effects,
}

impl Notifier for Recorder {
    fn notify(&self, code: &str, detail: &str) {
        record(&self.effects, json!({"notify": code, "detail": detail}));
    }

    fn resolve_update_offers(&self, _: &str, _: Option<&str>, _: &str) -> Result<(), String> {
        Ok(())
    }

    fn game_update_available(
        &self,
        _: &str,
        _: Option<&str>,
        _: &str,
        _: &VersionEventInfo,
    ) -> Result<(), String> {
        Ok(())
    }

    fn game_installed(&self, _: &str, _: &str, _: &VersionEventInfo) -> Result<(), String> {
        Ok(())
    }

    fn game_updated(
        &self,
        _: Option<&str>,
        _: &str,
        _: &str,
        _: &VersionEventInfo,
    ) -> Result<(), String> {
        Ok(())
    }
}

impl Telemetry for Recorder {
    fn emit(&self, event_type: &str, fields: &Value) -> Result<(), String> {
        record(
            &self.effects,
            json!({"telemetry": event_type, "fields": fields}),
        );
        Ok(())
    }
}

impl Achievements for Recorder {
    fn achieve(&self, id: &str) {
        record(&self.effects, json!({"achieve": id}));
    }
}

impl PresenceSink for Recorder {
    fn signal(&self, signal: &Value) -> Result<(), String> {
        record(&self.effects, json!({"presence": signal}));
        Ok(())
    }
}

impl OverlayConfig for Recorder {
    fn write_config(&self, game_id: &str, _settings: &Value) -> Result<bool, String> {
        record(
            &self.effects,
            json!({"overlay": "writeConfig", "gameId": game_id}),
        );
        Ok(true)
    }

    fn bind_to(&self, game_id: Option<&str>, _: &dyn Fn(&str) -> Option<Value>) -> bool {
        record(
            &self.effects,
            json!({"overlay": "bindTo", "gameId": game_id}),
        );
        false
    }
}

/// `library.json` в памяти: каналы библиотеки в сценариях скрипта не вызываются.
#[derive(Default)]
pub struct MemoryLibrary(pub Mutex<Map<String, Value>>);

impl LibraryStore for MemoryLibrary {
    fn get(&self, key: &str) -> Option<Value> {
        self.0.lock().unwrap().get(key).cloned()
    }

    fn set(&self, key: &str, value: Value) -> bool {
        self.0.lock().unwrap().insert(key.to_owned(), value);
        true
    }
}

impl MemoryLibrary {
    /// Значение ключа или `null`.
    pub fn get_value(&self, key: &str) -> Value {
        self.0
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .unwrap_or(Value::Null)
    }

    pub fn set_value(&self, key: &str, value: Value) {
        self.0.lock().unwrap().insert(key.to_owned(), value);
    }
}

/// Фасад и фейки сценария.
pub struct Harness {
    pub api: Arc<DeliveryApi>,
    pub events: Receiver<DeliveryEvent>,
    pub effects: Effects,
    pub log: Arc<Mutex<Vec<(String, String)>>>,
    pub processes: Arc<FakeProcesses>,
    pub dialogs: Arc<FakeDialogs>,
    pub shell: Arc<FakeShell>,
    pub free_space: Arc<FakeFreeSpace>,
    pub library: Arc<MemoryLibrary>,
}

impl Harness {
    /// События окна, пришедшие с прошлого вызова.
    pub fn take_events(&mut self) -> Vec<DeliveryEvent> {
        let mut out = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            out.push(event);
        }
        out
    }

    /// Вызовы фейков с прошлого вызова.
    pub fn take_effects(&self) -> Vec<Value> {
        std::mem::take(&mut *self.effects.lock().unwrap())
    }

    /// Строки журнала с прошлого вызова: `(уровень, текст)`.
    pub fn take_log(&self) -> Vec<(String, String)> {
        std::mem::take(&mut *self.log.lock().unwrap())
    }
}

/// Что задаёт сценарий.
pub struct Setup {
    /// Папка сценария: библиотека — `lib`, данные лаунчера — `data`, рабочий стол — `Desktop`.
    pub root: PathBuf,
    /// Манифесты; `None` — встроенные.
    pub registry: Option<Registry>,
    /// Часы, мс.
    pub now: i64,
    pub launcher: LauncherIdentity,
}

/// 7za из `resources/7zip`.
pub fn real_7za() -> PathBuf {
    hlsr_core::paths::Resources::detect().sevenzip()
}

/// Сообщения 7za зависят от локали, время записей — от пояса.
pub fn pinned_env() -> [(&'static str, &'static str); 3] {
    [("LC_ALL", "C.UTF-8"), ("LANG", "C.UTF-8"), ("TZ", "UTC")]
}

/// Фасад с готовой службой доставки (`controller.service = …` скрипта): канал и сеть выключены,
/// одна попытка загрузки, 7za из ресурсов, инструменты без запуска.
pub fn harness(setup: Setup) -> Harness {
    let Setup {
        root,
        registry,
        now,
        launcher,
    } = setup;
    let lib = root.join("lib");
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let effects: Effects = Arc::default();
    let log: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let logger: Arc<dyn Logger> = Arc::new(ListLogger {
        entries: log.clone(),
    });
    let recorder = Arc::new(Recorder {
        effects: effects.clone(),
    });
    let processes = Arc::new(FakeProcesses {
        running: Mutex::default(),
        now,
    });
    let dialogs = Arc::new(FakeDialogs {
        effects: effects.clone(),
        answers: Mutex::default(),
        root: root.clone(),
    });
    let shell = Arc::new(FakeShell {
        effects: effects.clone(),
        open_path_failure: Mutex::default(),
        open_external_failure: Mutex::default(),
    });
    let free_space = Arc::new(FakeFreeSpace::default());
    let library = Arc::new(MemoryLibrary::default());
    let archive = ArchiveService::new(real_7za(), logger.clone()).with_env(pinned_env());
    let library_path = lib.to_string_lossy().into_owned();
    let api = DeliveryApi::new(ApiOptions {
        library_path: library_path.clone(),
        launcher_version: "0.4.7".to_owned(),
        user_data_path: Some(data.clone()),
        env: Environment::new(Vec::<(String, String)>::new(), false),
        desktop: root.join("Desktop"),
        launcher,
        ports: ApiPorts {
            logger: logger.clone(),
            notifier: recorder.clone(),
            telemetry: Some(recorder.clone()),
            achievements: Some(recorder.clone()),
            presence: Some(recorder.clone()),
            overlay: Some(recorder.clone()),
            dialogs: dialogs.clone(),
            shell: shell.clone(),
            registry: Arc::new(FakeRegistry::default()),
            free_space: free_space.clone(),
            proton_prefix: None,
        },
        clock: Some(Arc::new(move || now)),
        factory: None,
        download_settings: Some(Arc::new(Mutex::new(DownloadSettings::new(
            data.join("download-settings.json"),
        )))),
        library_store: Some(library.clone() as Arc<dyn LibraryStore>),
        schedule_updates: false,
    });
    // Заметки о выпуске службы — те же, что у фасада (`ensureReleaseNotes`).
    let service = GameDeliveryService::new(ServiceOptions {
        library_path,
        logger: Some(logger.clone()),
        env: Some(Environment::new(Vec::<(String, String)>::new(), false)),
        launcher_version: Some("0.4.7".to_owned()),
        registry,
        processes: Some(processes.clone()),
        channel_service: Provided::Off,
        origin_updates: Provided::Off,
        notifier: Some(Arc::new(SilentNotifier)),
        release_notes: api.release_notes(),
        archive_service: Some(archive.clone()),
        free_space: Some(Arc::new(FakeFreeSpace::default())),
        max_attempts: Some(1),
        backoff_base_ms: Some(1),
        clock: Some(Arc::new(move || now)),
        ..ServiceOptions::default()
    })
    .unwrap();
    let tools = ToolService::new(
        service.paths().clone(),
        ToolOptions {
            archive: Some(archive),
            logger: Some(logger),
            clock: Some(Arc::new(move || now)),
            ..ToolOptions::default()
        },
    );
    api.set_parts(Parts {
        service,
        tools: Some(Arc::new(tools)),
        practice_saves: None,
    });
    Harness {
        events: api.subscribe(),
        api,
        effects,
        log,
        processes,
        dialogs,
        shell,
        free_space,
        library,
    }
}
