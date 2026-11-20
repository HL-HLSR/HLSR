//! Настройки игры на странице «Персонализация» (`GameSettingsModal.vue` embedded 0.4.9):
//! состояние, загрузка, сохранение, сброс и отмена правок, записи «Призраков», раздел «Конфиг»
//! и импорт cfg, общий подвал. Модального окна (крестик, окно клавиатуры поверх) в 0.4.9 нет —
//! страница не закрывается. Живёт в потоке интерфейса; вида не знает — вид читает [`View`],
//! события идут в [`Event`].
//!
//! Вызовы главного процесса — `async`-сценарии на очереди [`Tasks`]: порядок вызовов каналов
//! тот же, что у `await ipcRenderer.invoke(…)` компонента (журнал `ipc-log.json`).

use super::super::jsv;
pub use super::config::EditorView;
use super::config::{self, ConfigEditor, Edits};
use super::draft::{self, Draft, Item, SaveError, Toggle, Tr};
use super::import::{self, Note};
use super::records::{self, Fact, Run, Runs};
use super::sections::{self, Context, Direction, Section};
use super::tasks::{Tasks, yield_now};
use crate::controllers::boot::format_size;
use crate::controllers::cfg_check::{self, RuleCounts};
use crate::controllers::game_keyboard::{Keyboard, KeyboardPrefs, KeyboardView, MemoryPrefs};
use crate::controllers::game_scripts::{Scripts, ScriptsView};
use crate::controllers::shell::Translator;
use crate::env::Env as UiEnv;
use crate::services::{Reply, Services};
use chrono::{FixedOffset, Local};
use hlsr_core::js::{self, Platform};
use hlsr_core::language::Language;
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Часовой пояс дат окна.
#[derive(Debug, Clone, Copy)]
pub enum Zone {
    /// Местное время ОС.
    Local,
    /// Свой пояс (сцены снимков, тесты).
    Fixed(FixedOffset),
}

impl Zone {
    /// Всемирное время (сцены снимков, тесты).
    pub fn utc() -> Zone {
        Zone::Fixed(FixedOffset::east_opt(0).expect("нулевой сдвиг пояса"))
    }
}

/// Что окно сообщает странице.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// `$emit("saved", settings)`.
    Saved(Value),
    /// «Скрипты» больше не ждут клавишу (`stopCapture`): снять перехват нажатий.
    StopCapture,
    /// Встроенный плеер: адрес встраивания — открыть, `None` — закрыть.
    Video(Option<String>),
}

/// Загрузка настроек (`loadState`): страница ждёт её перед импортом из опроса.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadState {
    Loading,
    Ready,
    Failed,
}

/// Игра и окружение окна.
pub struct Env {
    /// Идентификатор в лаунчере («70», «220», «218»).
    pub game_id: String,
    /// Название игры (`gameTitle`).
    pub game_title: String,
    /// Раздел, открытый сразу (`initialSection`); пусто — первый в списке.
    pub initial_section: String,
    /// `window.hlsrElectron.platform === "linux"`.
    pub linux: bool,
    pub language: Language,
    pub translator: Translator,
    pub zone: Zone,
}

type Render = Rc<dyn Fn()>;
type Sink = Rc<dyn Fn(Event)>;

/// Вкладки и страницы окна.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigMode {
    /// `GameConfigEditor` (`configState.status === "ok"`).
    Editor,
    /// Только список файлов: у игры нет понятных настроек, но cfg править можно.
    FilesOnly,
    /// Пустое состояние.
    Empty,
}

/// Место редактора файлов cfg (задача 28 заменяет его настоящим `GameCfgFiles`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesSlot {
    /// `true` — внутри редактора настроек (слот `files`), `false` — вместо него.
    pub in_editor: bool,
    pub game_id: String,
    pub build: Option<String>,
}

#[derive(Debug)]
struct State {
    draft: Draft,
    args_error: Option<String>,
    proton_error: Option<String>,
    saving: bool,
    section: String,
    /// Видимая вкладка (`activeSection`), какой её видел последний проход `watch`; `None` — до
    /// первой отрисовки.
    seen_section: Option<&'static str>,
    slide: Direction,
    /// Загрузка настроек (`loadState`).
    load_state: LoadState,
    /// Черновик, каким он загружен или сохранён (`draftSaved`): по нему видно несохранённое;
    /// `None` — ещё читается.
    draft_saved: Option<Draft>,
    /// Переключатель Vulkan: `None` — у игры его нет, `Some(false)` — в сборке нет DXVK.
    vulkan: Option<bool>,
    /// Отпечаток файла игрока, по которому построено окно (`configSha256`); `None` — `undefined`.
    config_sha256: Option<Value>,
    /// Сохранение отклонено: файл игрока изменён вне лаунчера (`CONFIG_CHANGED`).
    file_changed: bool,
    /// Блоки скриптов, правленные руками: ждут ответа «Удалить?» (`editedPending`).
    edited_pending: Option<Vec<String>>,
    /// Последнее перечитывание по фокусу (`focusToken`).
    focus_token: u64,
    /// Проверка конфига HL1 по правилам speedrun.com (`ruleCheck`): ответ `player-config-check`
    /// со `status: "ok"`; прежний итог переживает сбой перепроверки.
    rule_check: Option<Value>,
    rule_check_loading: bool,
    rule_check_failed: bool,
    rule_check_token: u64,
    /// Категория проверки (`ruleCategory`, launcher-prefs); вид «Проверки» — G5.
    rule_category: String,
    languages: Vec<String>,
    key_binds: Option<Value>,
    binds_game_id: Option<String>,
    config_info: Option<Value>,
    config_build: Option<String>,
    config_state: Option<Value>,
    editor: Option<ConfigEditor>,
    edits: Edits,
    importing_config: bool,
    config_import: Option<Note>,
    config_error: Option<String>,
    runs: Runs,
    delivery: Option<Value>,
    files: Option<FilesSlot>,
    keyboard: Keyboard,
    scripts: Scripts,
    importing_binds: bool,
    /// Итог импорта биндов в подвале окна клавиатуры (`bindsImport`).
    binds_import: Option<Note>,
    pending_cfg_import: Option<binds::PendingImport>,
    /// В редакторе текста cfg несохранённая правка (`cfgFilesDirty`).
    cfg_files_dirty: bool,
    /// `everyKeyBound` клавиатуры на прошлом показе («Пианино», `watch` с `immediate`).
    piano: Option<bool>,
}

impl State {
    fn new(section: &str) -> State {
        State {
            draft: Draft::empty(),
            args_error: None,
            proton_error: None,
            saving: false,
            section: section.to_owned(),
            seen_section: None,
            slide: Direction::Next,
            load_state: LoadState::Loading,
            draft_saved: None,
            vulkan: None,
            config_sha256: None,
            file_changed: false,
            edited_pending: None,
            focus_token: 0,
            rule_check: None,
            rule_check_loading: false,
            rule_check_failed: false,
            rule_check_token: 0,
            rule_category: "scriptless".to_owned(),
            languages: Vec::new(),
            key_binds: None,
            binds_game_id: None,
            config_info: None,
            config_build: None,
            config_state: None,
            editor: None,
            edits: Edits::new(),
            importing_config: false,
            config_import: None,
            config_error: None,
            runs: Runs::default(),
            delivery: None,
            files: None,
            keyboard: Keyboard::default(),
            scripts: Scripts::default(),
            importing_binds: false,
            binds_import: None,
            pending_cfg_import: None,
            cfg_files_dirty: false,
            piano: None,
        }
    }

    /// `ruleCounts`: значок и вкладка «Проверка».
    fn rule_counts(&self) -> Option<RuleCounts> {
        cfg_check::rule_counts(self.rule_check.as_ref(), self.rule_check_failed)
    }
}

/// Сессия окна настроек одной игры.
pub struct Session {
    services: Services,
    env: Env,
    tasks: Tasks,
    state: RefCell<State>,
    render: RefCell<Render>,
    sink: RefCell<Sink>,
    flush_queued: Cell<bool>,
    /// Окно другой игры заменило это: компонент уничтожен (`isUnmounted`), экран и страница
    /// этой сессии больше не слышат.
    detached: Cell<bool>,
    /// Где хранится режим рана клавиатуры.
    keyboard_prefs: RefCell<Rc<dyn KeyboardPrefs>>,
    /// Редактор файлов cfg окна (`GameCfgFiles`): ему — игра и сборка места; без него список
    /// просит сама сессия.
    files_hook: RefCell<Option<FilesHook>>,
    /// «Отмена» и «Не сохранять» откатывают текст редактора cfg (`$refs.cfgFiles.revert()`).
    cfg_files_revert: RefCell<Option<Rc<dyn Fn()>>>,
}

/// Место редактора cfg появилось, сменило игру или сборку (`Some`) или ушло (`None`).
pub type FilesHook = Rc<dyn Fn(Option<&FilesSlot>)>;

/// Раздел в списке слева.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabView {
    pub id: &'static str,
    pub label: String,
    pub icon: &'static str,
}

/// Переключатель Vulkan (DXVK) у Ghosting на Windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VulkanView {
    /// В сборке есть DXVK: переключатель доступен.
    pub available: bool,
    pub checked: bool,
    pub hint: String,
}

/// Общий подвал (`settings-modal__footer`): заметка справа и кнопки.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FooterView {
    /// Нет подвала: в «Персонализации» свои кнопки.
    pub hidden: bool,
    /// «Сбросить», «Отмена», «Сохранить» (`footerSaves`).
    pub saves: bool,
    pub save_enabled: bool,
    pub note: String,
    pub note_error: bool,
    /// Подсказка заметки (`title`).
    pub note_details: String,
    /// «Файл изменён снаружи»: кнопка «Перечитать».
    pub file_changed: bool,
    /// Правленые руками скрипты: «Удалить» и крестик.
    pub edited: bool,
    /// Значок «Убрать импортированный cfg» (раздел «Клавиатура»).
    pub remove_imported: bool,
    pub remove_imported_busy: bool,
}

/// Переключатель вкладки «Запуск».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToggleView {
    pub key: &'static str,
    pub label: String,
    pub hint: String,
    pub checked: bool,
}

/// Список выбора: пункты, выбранный и подпись кнопки.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectView {
    pub items: Vec<Item>,
    pub value: String,
    pub label: String,
}

/// Запись «Призраков» в строке списка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
    pub id: String,
    pub name: String,
    pub size: String,
    pub date: String,
    pub confirm: bool,
    pub deleting: bool,
}

/// Что показано вместо списка записей.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunsBody {
    Loading,
    Error(String),
    Empty,
    List,
}

/// Всё, что окно показывает; пересчитывается после каждого изменения состояния.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub game_title: String,
    pub tabs: Vec<TabView>,
    /// Номер открытого раздела в списке.
    pub active: usize,
    pub active_id: &'static str,
    pub slide_next: bool,
    pub load_state: LoadState,
    pub goldsrc: bool,
    pub toggles: Vec<ToggleView>,
    /// Vulkan: `None` — переключателя нет (не Ghosting или Linux).
    pub vulkan: Option<VulkanView>,
    pub resolution: SelectView,
    pub refresh: SelectView,
    pub cpu0: bool,
    pub language_available: bool,
    pub language: SelectView,
    pub timer: bool,
    pub args: String,
    pub args_error: String,
    pub args_hint: String,
    pub args_placeholder: &'static str,
    pub linux: bool,
    pub proton: String,
    pub proton_error: String,
    pub config_mode: ConfigMode,
    pub build: SelectView,
    pub build_select: bool,
    pub path_text: String,
    pub open_folder_enabled: bool,
    /// Предупреждение над редактором: ошибка записи или «игра запущена».
    pub alert: String,
    pub alert_error: bool,
    pub editor: Option<EditorView>,
    pub empty_text: String,
    pub files: Option<FilesSlot>,
    pub runs_summary: String,
    pub runs_body: RunsBody,
    pub runs: Vec<RunRow>,
    pub runs_error: String,
    pub runs_refresh_enabled: bool,
    pub runs_folder_enabled: bool,
    pub facts: Vec<Fact>,
    pub import_busy: bool,
    pub saving: bool,
    /// Раздел «Клавиатура»; `None` — у игры его нет.
    pub keyboard: Option<KeyboardView>,
    /// Раздел «Скрипты» (только Half-Life).
    pub scripts: Option<ScriptsView>,
    pub footer: FooterView,
}

mod binds;

fn js_str(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => js::to_string(other),
    }
}

impl Session {
    /// Сессия; `ui` — цикл событий потока интерфейса (задачи).
    pub fn new(services: Services, env: Env, ui: Rc<dyn UiEnv>) -> Rc<Session> {
        Rc::new(Session {
            services,
            state: RefCell::new(State::new(&env.initial_section)),
            env,
            tasks: Tasks::new(crate::env::dispatch(&*ui)),
            render: RefCell::new(Rc::new(|| {})),
            sink: RefCell::new(Rc::new(|_| {})),
            flush_queued: Cell::new(false),
            detached: Cell::new(false),
            keyboard_prefs: RefCell::new(Rc::new(MemoryPrefs::default())),
            files_hook: RefCell::new(None),
            cfg_files_revert: RefCell::new(None),
        })
    }

    /// Редактор текста cfg: откат его правок при «Отмене» и «Не сохранять».
    pub fn set_cfg_files_revert(&self, revert: Rc<dyn Fn()>) {
        *self.cfg_files_revert.borrow_mut() = Some(revert);
    }

    /// `@dirty` редактора текста cfg (`cfgFilesDirty`).
    pub fn set_cfg_files_dirty(self: &Rc<Self>, dirty: bool) {
        if self.state.borrow().cfg_files_dirty != dirty {
            self.state.borrow_mut().cfg_files_dirty = dirty;
            self.dirty();
            self.pump();
        }
    }

    pub fn cfg_files_dirty(&self) -> bool {
        self.state.borrow().cfg_files_dirty
    }

    /// `onCfgFileSaved`: текст cfg сохранён — проверка заново, файл игрока перечитывается
    /// (его могли править в этом редакторе).
    pub fn on_cfg_file_saved(self: &Rc<Self>) {
        self.state.borrow_mut().cfg_files_dirty = false;
        self.load_rule_check();
        self.on_window_focus();
        self.dirty();
        self.pump();
    }

    /// `showCheck` (строка итога импорта на странице): раздел «Конфиг», вкладка «Проверка».
    /// Вопрос о несохранённом задаёт страница до вызова.
    pub fn show_check(self: &Rc<Self>) {
        self.select_section("configs");
        if self.state.borrow().rule_counts().is_none() {
            self.load_rule_check();
        }
        self.config_select_section(config::CHECK_ID);
    }

    /// Редактор файлов cfg окна: место (игра, сборка) уходит ему, `game-cfg-files-list` просит он.
    pub fn set_files_hook(&self, hook: FilesHook) {
        *self.files_hook.borrow_mut() = Some(hook);
    }

    /// Куда показывать состояние (адаптер): вызывается после изменений, один раз за круг очереди.
    pub fn set_render(&self, render: impl Fn() + 'static) {
        *self.render.borrow_mut() = Rc::new(render);
    }

    /// Кому сообщать о закрытии и сохранении окна.
    pub fn set_sink(&self, sink: impl Fn(Event) + 'static) {
        *self.sink.borrow_mut() = Rc::new(sink);
    }

    /// Окно другой игры заменило это (`:key="gameId"`): как уничтожение компонента Vue. `emit`
    /// уничтоженного компонента — пустая операция (`instance.isUnmounted`), и экрана у него нет,
    /// а его `async`-методы доходят до конца: начатые записи и сохранение не обрываются, очередь
    /// не сбрасывается. Обработчики экрана и страницы отпускаются (иначе сессия держала бы вид,
    /// а вид — её).
    pub fn detach(&self) {
        self.detached.set(true);
        *self.render.borrow_mut() = Rc::new(|| {});
        *self.sink.borrow_mut() = Rc::new(|_| {});
    }

    pub fn game_id(&self) -> &str {
        &self.env.game_id
    }

    /// Название игры (`gameTitle`).
    pub fn game_title(&self) -> &str {
        &self.env.game_title
    }

    /// Игра биндов и файла игрока (`bindsGameId`: у HL2 — по сборке); `None` — ещё не узнана.
    pub fn binds_game_id(&self) -> Option<String> {
        self.state.borrow().binds_game_id.clone()
    }

    /// Очередь задач (сцены снимков и тесты ждут ответов каналов).
    pub fn tasks(&self) -> &Tasks {
        &self.tasks
    }

    fn t(&self, key: &str, args: &[&str]) -> String {
        (self.env.translator)(self.env.language, key, args)
    }

    fn emit(&self, event: Event) {
        if self.detached.get() {
            return;
        }
        let sink = self.sink.borrow().clone();
        sink(event);
    }

    /// Состояние изменилось: показать его после продолжений, уже стоящих в очереди (обновление
    /// Vue идёт микрозадачей).
    fn dirty(self: &Rc<Self>) {
        if self.flush_queued.replace(true) {
            return;
        }
        let session = self.clone();
        self.tasks.defer(move || {
            session.flush_queued.set(false);
            // У уничтоженного компонента нет ни экрана, ни вложенных компонентов (список файлов
            // cfg не запрашивается).
            if session.detached.get() {
                return;
            }
            session.watch_active_section();
            session.watch_piano();
            session.sync_files_slot();
            let render = session.render.borrow().clone();
            render();
        });
    }

    /// `await ipcRenderer.invoke(channel, …args)`: вызов состоялся сразу, ответ — на следующем
    /// круге очереди.
    async fn call(&self, channel: &str, args: Vec<Value>) -> Reply {
        let reply = self.services.invoke(channel, args);
        yield_now().await;
        reply.await
    }

    /// `ipcRenderer.invoke(…)` без `await`: вызов состоялся сразу, ответ никому не нужен, но
    /// обещание доводится до конца на очереди окна.
    fn fire(&self, channel: &str, args: Vec<Value>) {
        let reply = self.services.invoke(channel, args);
        self.tasks.spawn(async move {
            let _ = reply.await;
        });
    }

    /// Выполнить готовые задачи и показать состояние (после обработчика нажатия).
    pub fn pump(self: &Rc<Self>) {
        self.tasks.run();
    }

    // ---- загрузка ---------------------------------------------------------------------------

    /// `mounted()`: `load()` (затем `loadState`), `loadConfigInfo()`, `loadRuns()` (только у
    /// игры с «Призраками»), `loadDelivery()` друг за другом.
    pub fn mounted(self: &Rc<Self>) {
        let session = self.clone();
        self.tasks.spawn(async move {
            let ok = session.load(None).await;
            session.state.borrow_mut().load_state = if ok {
                LoadState::Ready
            } else {
                LoadState::Failed
            };
            session.dirty();
        });
        let session = self.clone();
        self.tasks
            .spawn(async move { session.load_config_info().await });
        if self.env.game_id == draft::GHOSTING_GAME_ID {
            let session = self.clone();
            self.tasks.spawn(async move { session.load_runs().await });
        }
        let session = self.clone();
        self.tasks
            .spawn(async move { session.load_delivery().await });
        self.dirty();
        self.pump();
    }

    /// `loadState`: страница ждёт её перед импортом.
    pub fn load_state(&self) -> LoadState {
        self.state.borrow().load_state
    }

    /// `load(accept)`: `false` — канал не ответил или ответ устарел (`accept`).
    async fn load(self: &Rc<Self>, accept: Option<&dyn Fn() -> bool>) -> bool {
        let id = json!(self.env.game_id);
        let Ok(result) = self.call("game-settings-get", vec![id]).await else {
            return false;
        };
        if accept.is_some_and(|accept| !accept()) {
            return false;
        }
        // 0.4.9 брал пустой черновик за сохранённый: «Сохранить» затёр бы настройки игрока
        // умолчаниями. Здесь ошибка — как сбой канала: черновик не загружен, сохранять нечего.
        if !jsv::is_str(jsv::prop(&result, "status"), "ok") {
            return false;
        }
        {
            let mut state = self.state.borrow_mut();
            let settings = jsv::prop(&result, "settings").filter(|s| js::truthy(s));
            state.draft = match settings {
                Some(settings) => Draft::from_settings(Some(settings)),
                None => Draft::empty(),
            };
            state.draft_saved = Some(state.draft.clone());
            state.config_sha256 = jsv::prop(&result, "configSha256").cloned();
            state.vulkan = jsv::prop(&result, "vulkan").and_then(Value::as_bool);
            state.languages = jsv::prop(&result, "languages")
                .and_then(Value::as_array)
                .map(|list| list.iter().map(|item| js_str(Some(item))).collect())
                .unwrap_or_default();
            state.key_binds = jsv::prop(&result, "keyBinds")
                .filter(|binds| js::truthy(binds))
                .cloned();
            let binds_game_id = jsv::str_of(jsv::prop(&result, "bindsGameId"))
                .filter(|id| !id.is_empty())
                .map(str::to_owned);
            // watch gameId клавиатуры: выбор, наведение, категория и поиск сбрасываются.
            if binds_game_id != state.binds_game_id {
                state.keyboard.reset_for_game();
            }
            state.binds_game_id = binds_game_id;
        }
        self.dirty();
        true
    }

    async fn load_config_info(self: &Rc<Self>) {
        let (id, build) = (json!(self.env.game_id), self.build_value());
        let Ok(info) = self.call("game-config-info", vec![id, build]).await else {
            return;
        };
        {
            let mut state = self.state.borrow_mut();
            if let Some(build) = jsv::str_of(jsv::prop(&info, "build")).filter(|b| !b.is_empty()) {
                state.config_build = Some(build.to_owned());
            }
            state.config_info = Some(info);
        }
        self.dirty();
        self.load_config().await;
    }

    fn build_value(&self) -> Value {
        self.state
            .borrow()
            .config_build
            .as_ref()
            .map_or(Value::Null, |build| json!(build))
    }

    async fn load_config(self: &Rc<Self>) {
        self.load_config_if(None).await;
    }

    /// `loadConfig(accept)`: ответ устарел (`accept`) — не применяется.
    async fn load_config_if(self: &Rc<Self>, accept: Option<&dyn Fn() -> bool>) {
        let (id, build) = (json!(self.env.game_id), self.build_value());
        let reply = self.call("game-config-read", vec![id, build]).await;
        if accept.is_some_and(|accept| !accept()) {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            let value = reply.unwrap_or_else(|_| json!({"status": "error"}));
            let ok = jsv::is_str(jsv::prop(&value, "status"), "ok");
            // Редактор получает слот файлов cfg (в 0.4.7 — всегда: `#files` у окна есть).
            let mut editor = ok.then(|| ConfigEditor::from_state(&value, true)).flatten();
            if let (Some(editor), Some(previous)) = (editor.as_mut(), state.editor.take()) {
                editor.keep_view(previous);
            }
            state.editor = editor;
            state.config_state = Some(value);
        }
        self.load_rule_check();
        self.dirty();
    }

    /// `loadRuleCheck()`: только чтение файлов; вызов сразу, ответ устаревшего запроса
    /// отбрасывается. Сбой не стирает прежний итог; `nothing` и `unsupported` — проверять нечего.
    pub fn load_rule_check(self: &Rc<Self>) {
        let (token, category) = {
            let mut state = self.state.borrow_mut();
            state.rule_check_token += 1;
            state.rule_check_loading = true;
            (state.rule_check_token, state.rule_category.clone())
        };
        let reply = self.services.invoke(
            "player-config-check",
            vec![json!(self.env.game_id), json!(category)],
        );
        let session = self.clone();
        self.tasks.spawn(async move {
            yield_now().await;
            let result = reply.await.ok();
            {
                let mut state = session.state.borrow_mut();
                if token != state.rule_check_token {
                    return;
                }
                state.rule_check_loading = false;
                let status = result
                    .as_ref()
                    .and_then(|result| jsv::str_of(jsv::prop(result, "status")))
                    .map(str::to_owned);
                let failed = result.is_none() || status.as_deref() == Some("error");
                state.rule_check_failed = failed;
                if !failed {
                    state.rule_check = result.filter(|_| status.as_deref() == Some("ok"));
                }
            }
            session.dirty();
        });
    }

    /// Итог проверки HL1: ответ `ok` (или `None`), идёт ли проверка, не удалась ли перепроверка.
    pub fn rule_check(&self) -> (Option<Value>, bool, bool) {
        let state = self.state.borrow();
        (
            state.rule_check.clone(),
            state.rule_check_loading,
            state.rule_check_failed,
        )
    }

    /// Категория из prefs до первой проверки (`ruleCategory: prefsStore.ruleCategoryOf()`).
    pub fn init_rule_category(&self, category: &str) {
        let category = if category == "scripted" {
            "scripted"
        } else {
            "scriptless"
        };
        category.clone_into(&mut self.state.borrow_mut().rule_category);
    }

    pub fn rule_category(&self) -> String {
        self.state.borrow().rule_category.clone()
    }

    /// Выбрана категория проверки (`setRuleCategory`): проверить заново.
    pub fn set_rule_category(self: &Rc<Self>, category: &str) {
        let category = if category == "scripted" {
            "scripted"
        } else {
            "scriptless"
        };
        if self.state.borrow().rule_category == category {
            return;
        }
        self.state.borrow_mut().rule_category = category.to_owned();
        self.load_rule_check();
        self.pump();
    }

    async fn load_runs(self: &Rc<Self>) {
        {
            let mut state = self.state.borrow_mut();
            if state.runs.loading {
                return;
            }
            state.runs.pending_delete = None;
            state.runs.loading = true;
            state.runs.error = None;
        }
        self.dirty();
        let reply = self
            .call("game-runs-list", vec![json!(self.env.game_id)])
            .await;
        {
            let mut state = self.state.borrow_mut();
            // Отказ канала: `finally` снимает загрузку, список прежний (исключение уходит наверх).
            if let Ok(result) = &reply {
                let t = |key: &str, args: &[&str]| self.t(key, args);
                state.runs.apply_list(Some(result), &t);
            }
            state.runs.loading = false;
        }
        self.dirty();
    }

    async fn load_delivery(self: &Rc<Self>) {
        let reply = self
            .call("game-delivery-get-status", vec![json!(self.env.game_id)])
            .await;
        self.state.borrow_mut().delivery = reply.ok();
        self.dirty();
    }

    // ---- вкладки ----------------------------------------------------------------------------

    fn section_context(&self, state: &State) -> Vec<Section> {
        let has_keyboard = state.binds_game_id.as_deref().is_some_and(|id| {
            hlsr_delivery::bind_presets::catalog_for(id, self.platform()).is_some()
        });
        sections::sections(&Context {
            game_id: &self.env.game_id,
            has_keyboard,
            has_scripts: state.binds_game_id.as_deref() == Some("hl1-won"),
        })
    }

    fn platform(&self) -> Platform {
        if self.env.linux {
            Platform::Linux
        } else {
            Platform::Win32
        }
    }

    /// Открыть раздел (`section = id`). Вопрос о несохранённом при переходе через
    /// «Персонализацию» задаёт страница до вызова ([`sections::leave_part`]).
    pub fn select_section(self: &Rc<Self>, id: &str) {
        let was_scripts = self.scripts_active();
        // Свайп и сброс вопроса об удалении — `watch activeSection` (watch_active_section).
        self.state.borrow_mut().section = id.to_owned();
        if was_scripts && !self.scripts_active() {
            self.scripts_left();
        }
        self.dirty();
        self.pump();
    }

    /// Открытый раздел (`activeSection`).
    pub fn active_section(&self) -> &'static str {
        let state = self.state.borrow();
        let list = self.section_context(&state);
        sections::active_section(&list, &state.section)
    }

    /// `watch: activeSection(id, previous)` — до отрисовки (flush `pre`): раздел сменился
    /// нажатием, `section = "configs"` (ошибка конфига) или новым списком разделов (сборка
    /// узнана) — направление свайпа, сброс вопроса об удалении записи, итог импорта биндов
    /// уходит с «Клавиатурой». Тот же раздел — ничего.
    fn watch_active_section(&self) {
        let mut state = self.state.borrow_mut();
        let list = self.section_context(&state);
        let active = sections::active_section(&list, &state.section);
        let Some(previous) = state.seen_section.replace(active) else {
            return;
        };
        if previous == active {
            return;
        }
        let ids: Vec<&str> = list.iter().map(|s| s.id).collect();
        state.slide = sections::direction(&ids, previous, active);
        state.runs.pending_delete = None;
        state.runs.delete_error = None;
        if previous == "keyboard" {
            state.binds_import = None;
        }
    }

    /// Раздел «Скрипты» сейчас виден (`active` у `GameScripts`).
    fn scripts_active(&self) -> bool {
        self.active_section() == "scripts"
    }

    // ---- вкладка «Запуск» -------------------------------------------------------------------

    pub fn set_flag(self: &Rc<Self>, key: &str, on: bool) {
        self.state.borrow_mut().draft.set(key, json!(on));
        self.dirty();
        self.pump();
    }

    pub fn set_resolution(self: &Rc<Self>, id: &str) {
        draft::set_resolution(&mut self.state.borrow_mut().draft, id);
        self.dirty();
        self.pump();
    }

    pub fn set_refresh_rate(self: &Rc<Self>, id: &str) {
        let rate = js::to_number(id);
        self.state.borrow_mut().draft.set(
            "refreshRate",
            json!(if rate.is_finite() { rate as i64 } else { 0 }),
        );
        self.dirty();
        self.pump();
    }

    pub fn set_language(self: &Rc<Self>, id: &str) {
        self.state.borrow_mut().draft.set("language", json!(id));
        self.dirty();
        self.pump();
    }

    /// Ввод в поле параметров запуска: ошибка сохранения гаснет (`@input="argsError = null"`).
    pub fn set_args(self: &Rc<Self>, text: &str) {
        {
            let mut state = self.state.borrow_mut();
            state.draft.set("launchArgs", json!(text));
            state.args_error = None;
        }
        self.dirty();
        self.pump();
    }

    pub fn set_proton(self: &Rc<Self>, text: &str) {
        {
            let mut state = self.state.borrow_mut();
            state.draft.set("protonOptions", json!(text));
            state.proton_error = None;
        }
        self.dirty();
        self.pump();
    }

    // ---- вкладка «Конфиги» ------------------------------------------------------------------

    /// Другая сборка: правки прежней к ней не относятся (`onConfigBuild`).
    pub fn select_build(self: &Rc<Self>, build: &str) {
        {
            let mut state = self.state.borrow_mut();
            state.config_build = Some(build.to_owned());
            state.edits = Edits::new();
            state.config_import = None;
            state.config_error = None;
        }
        let session = self.clone();
        self.tasks
            .spawn(async move { session.load_config_info().await });
        self.dirty();
        self.pump();
    }

    fn with_editor<R>(&self, f: impl FnOnce(&ConfigEditor, &mut Edits) -> R) -> Option<R> {
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        let editor = state.editor.as_ref()?;
        Some(f(editor, &mut state.edits))
    }

    fn field(&self, cvar: &str) -> Option<&'static hlsr_delivery::game_config::Field> {
        self.state.borrow().editor.as_ref()?.field_by_cvar(cvar)
    }

    /// Вкладка редактора (`selectSection`): уход из «Файлов cfg» с несохранённым текстом
    /// спрашивает место до вызова.
    pub fn config_select_section(self: &Rc<Self>, id: &str) {
        if let Some(editor) = self.state.borrow_mut().editor.as_mut() {
            editor.select_section(id);
        }
        self.dirty();
        self.pump();
    }

    pub fn config_set_switch(self: &Rc<Self>, cvar: &str, on: bool) {
        if let Some(field) = self.field(cvar) {
            self.with_editor(|editor, edits| {
                editor.set(
                    edits,
                    field,
                    hlsr_delivery::game_config::WindowValue::Bool(on),
                );
            });
        }
        self.dirty();
        self.pump();
    }

    /// Поле числа потеряло фокус или нажат Enter: что показать в поле (`setNumber`).
    pub fn config_set_number(self: &Rc<Self>, cvar: &str, text: &str) -> String {
        let shown = self
            .field(cvar)
            .and_then(|field| {
                self.with_editor(|editor, edits| editor.set_number(edits, field, text))
            })
            .unwrap_or_default();
        self.dirty();
        self.pump();
        shown
    }

    /// Ползунок тянут (`slide`): значение встаёт на ближайший шаг, метка умолчания притягивает.
    /// `width` — ширина ползунка, px.
    pub fn config_slide(self: &Rc<Self>, cvar: &str, raw: f64, width: f64) {
        if let Some(field) = self.field(cvar) {
            let raw = ConfigEditor::detent(field, raw, width);
            let snapped = ConfigEditor::snap(field, raw);
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            if let Some(editor) = state.editor.as_mut() {
                editor.slide(field, raw);
                editor.set(
                    &mut state.edits,
                    field,
                    hlsr_delivery::game_config::WindowValue::Num(snapped),
                );
            }
        }
        self.dirty();
        self.pump();
    }

    /// Ползунок отпущен (`endSlide`).
    pub fn config_end_slide(self: &Rc<Self>, cvar: &str, raw: f64, width: f64) {
        if let Some(field) = self.field(cvar) {
            let raw = ConfigEditor::detent(field, raw, width);
            let snapped = ConfigEditor::snap(field, raw);
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            if let Some(editor) = state.editor.as_mut() {
                editor.end_slide(field);
                editor.set(
                    &mut state.edits,
                    field,
                    hlsr_delivery::game_config::WindowValue::Num(snapped),
                );
            }
        }
        self.dirty();
        self.pump();
    }

    /// Двойной щелчок по ползунку: значение по умолчанию.
    pub fn config_reset_value(self: &Rc<Self>, cvar: &str) {
        if let Some(field) = self.field(cvar) {
            self.with_editor(|editor, edits| {
                editor.set(
                    edits,
                    field,
                    hlsr_delivery::game_config::default_value(field),
                );
            });
        }
        self.dirty();
        self.pump();
    }

    pub fn config_select_option(self: &Rc<Self>, cvar: &str, id: &str) {
        if let Some(field) = self.field(cvar) {
            let value = js::to_number(id);
            self.with_editor(|editor, edits| {
                editor.set(
                    edits,
                    field,
                    hlsr_delivery::game_config::WindowValue::Num(value),
                );
            });
        }
        self.dirty();
        self.pump();
    }

    /// Кнопка «Вернуть значения по умолчанию в этом разделе» (`resetSection`).
    pub fn config_reset_section(self: &Rc<Self>, section_id: &str) {
        {
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            if let Some(editor) = state.editor.as_ref() {
                editor.reset_section(&mut state.edits, section_id);
            }
        }
        self.dirty();
        self.pump();
    }

    pub fn open_config_folder(&self) {
        let (build, id) = (self.build_value(), json!(self.env.game_id));
        // `ipcRenderer.invoke("game-config-open-folder", gameId, build)` без ожидания ответа.
        self.fire("game-config-open-folder", vec![id, build]);
    }

    /// Кнопка «Импорт из cfg» (`importConfig`).
    pub fn import_config(self: &Rc<Self>) {
        {
            let state = self.state.borrow();
            let ready = state.editor.as_ref().is_some_and(|editor| !editor.disabled);
            if state.importing_config || !ready {
                return;
            }
        }
        self.state.borrow_mut().importing_config = true;
        let session = self.clone();
        self.tasks.spawn(async move {
            // Редактор для разбора берётся заново из ответа `game-config-read`: пока идут окна
            // выбора, окно показывает прежний.
            let editor = session
                .state
                .borrow()
                .config_state
                .as_ref()
                .and_then(|value| ConfigEditor::from_state(value, true));
            let Some(editor) = editor else {
                session.state.borrow_mut().importing_config = false;
                return;
            };
            let t = |key: &str, args: &[&str]| session.t(key, args);
            let edits = || session.state.borrow().edits.clone();
            let result =
                import::import_config(&session.services, &session.env.game_id, &editor, &edits, &t)
                    .await;
            {
                let mut state = session.state.borrow_mut();
                if let Some(edits) = result.edits {
                    state.edits = edits;
                }
                if let Some(note) = result.note {
                    state.config_import = Some(note);
                }
                state.importing_config = false;
            }
            session.dirty();
        });
        self.dirty();
        self.pump();
    }

    // ---- записи -----------------------------------------------------------------------------

    pub fn refresh_runs(self: &Rc<Self>) {
        let session = self.clone();
        self.tasks.spawn(async move { session.load_runs().await });
        self.dirty();
        self.pump();
    }

    pub fn confirm_run_delete(self: &Rc<Self>, id: &str) {
        self.state.borrow_mut().runs.confirm_delete(id);
        self.dirty();
        self.pump();
    }

    pub fn cancel_run_delete(self: &Rc<Self>) {
        {
            let mut state = self.state.borrow_mut();
            if state.runs.deleting.is_none() {
                state.runs.pending_delete = None;
            }
        }
        self.dirty();
        self.pump();
    }

    pub fn delete_run(self: &Rc<Self>, id: &str) {
        let name = {
            let mut state = self.state.borrow_mut();
            if !state.runs.may_delete(id) {
                return;
            }
            state.runs.deleting = Some(id.to_owned());
            state.runs.delete_error = None;
            state
                .runs
                .items
                .iter()
                .find(|run| run.id == id)
                .map(|run| run.name.clone())
                .unwrap_or_default()
        };
        let (session, id) = (self.clone(), id.to_owned());
        self.tasks.spawn(async move {
            let reply = session
                .call(
                    "game-runs-delete",
                    vec![json!(session.env.game_id), json!(name)],
                )
                .await;
            let reload = {
                let mut state = session.state.borrow_mut();
                let t = |key: &str, args: &[&str]| session.t(key, args);
                state
                    .runs
                    .delete_done(&id, reply.as_ref().map_err(|_| ()), false, &t)
            };
            session.dirty();
            if reload {
                session.load_runs().await;
            }
        });
        self.dirty();
        self.pump();
    }

    fn run_name(&self, id: &str) -> Option<String> {
        self.state
            .borrow()
            .runs
            .items
            .iter()
            .find(|run| run.id == id)
            .map(|run| run.name.clone())
    }

    pub fn open_runs_folder(&self) {
        self.fire("game-runs-open-folder", vec![json!(self.env.game_id)]);
    }

    pub fn reveal_run(&self, id: &str) {
        if let Some(name) = self.run_name(id) {
            self.fire(
                "game-runs-reveal",
                vec![json!(self.env.game_id), json!(name)],
            );
        }
    }

    // ---- подвал: сброс, сохранение ----------------------------------------------------------

    /// «Сбросить» (`resetDraft`): конфиги не сбрасываются к умолчанию — отменяются несохранённые
    /// правки.
    pub fn reset(self: &Rc<Self>) {
        {
            let mut state = self.state.borrow_mut();
            // Идёт сохранение: оно ещё прочтёт черновик и импорт.
            if state.saving {
                return;
            }
            state.draft.reset();
            state.args_error = None;
            state.proton_error = None;
            state.edits = Edits::new();
            state.config_import = None;
            state.config_error = None;
            state.pending_cfg_import = None;
            state.binds_import = None;
        }
        self.dirty();
        self.pump();
    }

    /// `saveConfig()`: конфиги пишутся до остальных настроек; если игра запущена, не
    /// сохраняется ничего. Запись несёт отпечаток файла игрока (`expectedSha256`): файл
    /// изменили снаружи — `CONFIG_CHANGED`, подвал предлагает перечитать.
    async fn save_config(self: &Rc<Self>) -> bool {
        let args = {
            let mut state = self.state.borrow_mut();
            if state.edits.is_empty() {
                return true;
            }
            state.config_error = None;
            let mut args = vec![
                json!(self.env.game_id),
                state
                    .config_build
                    .as_ref()
                    .map_or(Value::Null, |build| json!(build)),
                Value::Object(state.edits.clone()),
            ];
            if let Some(sha) = state
                .config_state
                .as_ref()
                .and_then(|value| jsv::prop(value, "sha256"))
                .filter(|sha| sha.is_string())
            {
                args.push(json!({"expectedSha256": sha}));
            }
            args
        };
        let reply = self.call("game-config-write", args).await.ok();
        if let Some(reply) = reply
            .as_ref()
            .filter(|reply| jsv::is_str(jsv::prop(reply, "status"), "ok"))
        {
            // Файл игрока один: новый отпечаток нужен и остальным настройкам окна.
            if let Some(sha) = jsv::prop(reply, "sha256").filter(|sha| js::truthy(sha)) {
                let mut state = self.state.borrow_mut();
                if let Some(Value::Object(config)) = state.config_state.as_mut() {
                    config.insert("sha256".into(), sha.clone());
                }
                if state.config_sha256.is_some() {
                    state.config_sha256 = Some(sha.clone());
                }
            }
            return true;
        }
        let code = reply
            .as_ref()
            .and_then(|reply| jsv::str_of(jsv::prop(reply, "code")));
        let was_scripts = self.scripts_active();
        {
            let mut state = self.state.borrow_mut();
            state.section = "configs".to_owned();
            if code == Some("CONFIG_CHANGED") {
                state.file_changed = true;
            } else {
                let t = |key: &str, args: &[&str]| self.t(key, args);
                state.config_error = Some(config::error_text(reply.as_ref(), &t));
            }
        }
        if was_scripts && !self.scripts_active() {
            self.scripts_left();
        }
        self.dirty();
        if code == Some("GAME_RUNNING") {
            self.load_config().await;
        }
        false
    }

    /// Кнопка «Сохранить» (`submit()`): страница остаётся открытой, черновик берётся из
    /// сохранённых настроек.
    pub fn submit(self: &Rc<Self>) {
        self.submit_with(false);
    }

    /// «Удалить» у правленых руками скриптов (`deleteEdited`): то же сохранение с `removeEdited`.
    pub fn delete_edited(self: &Rc<Self>) {
        self.state.borrow_mut().edited_pending = None;
        self.submit_with(true);
    }

    /// Крестик у вопроса о правленых скриптах.
    pub fn dismiss_edited(self: &Rc<Self>) {
        self.state.borrow_mut().edited_pending = None;
        self.dirty();
        self.pump();
    }

    fn submit_with(self: &Rc<Self>, remove_edited: bool) {
        {
            let mut state = self.state.borrow_mut();
            // Черновик не загружен: в настройки ушли бы умолчания.
            if state.saving || state.draft_saved.is_none() {
                return;
            }
            state.saving = true;
            // Устаревший ответ перечитывания по фокусу отбрасывается.
            state.focus_token += 1;
            state.edited_pending = None;
            state.file_changed = false;
        }
        self.dirty();
        let session = self.clone();
        self.tasks.spawn(async move {
            session.submit_steps(remove_edited).await;
            session.state.borrow_mut().saving = false;
            session.dirty();
        });
        self.pump();
    }

    async fn submit_steps(self: &Rc<Self>, remove_edited: bool) {
        if !self.save_config().await {
            return;
        }
        if !self.apply_pending_import().await {
            return;
        }
        let mut args = vec![json!(self.env.game_id), self.state.borrow().draft.payload()];
        let mut options = serde_json::Map::new();
        if remove_edited {
            options.insert("removeEdited".into(), json!(true));
        }
        if let Some(sha) = self.state.borrow().config_sha256.clone() {
            options.insert("configSha256".into(), sha);
        }
        if !options.is_empty() {
            args.push(Value::Null);
            args.push(Value::Object(options));
        }
        let reply = self.call("game-settings-save", args).await;
        let t = |key: &str, args: &[&str]| self.t(key, args);
        let result = match reply {
            Ok(result) => result,
            Err(_) => {
                // catch: любой сбой IPC — общий текст.
                self.state.borrow_mut().args_error =
                    Some(draft::error_text(&SaveError::default(), &t));
                return;
            }
        };
        if !jsv::is_str(jsv::prop(&result, "status"), "ok") {
            let error = SaveError::from_value(jsv::prop(&result, "error"));
            let mut state = self.state.borrow_mut();
            if error.code.as_deref() == Some("CONFIG_CHANGED") {
                state.file_changed = true;
            } else if error.field.as_deref() == Some("protonOptions") {
                state.proton_error = Some(draft::error_text(&error, &t));
            } else {
                state.args_error = Some(draft::error_text(&error, &t));
            }
            return;
        }
        let settings = jsv::prop(&result, "settings")
            .cloned()
            .unwrap_or(Value::Null);
        let reload = {
            let mut state = self.state.borrow_mut();
            if let Some(sha) = jsv::prop(&result, "configSha256") {
                state.config_sha256 = Some(sha.clone());
                if let Some(Value::Object(config)) = state
                    .config_state
                    .as_mut()
                    .filter(|value| jsv::is_str(jsv::prop(value, "status"), "ok"))
                {
                    config.insert("sha256".into(), sha.clone());
                }
            }
            drop(state);
            self.emit(Event::Saved(settings.clone()));
            let mut state = self.state.borrow_mut();
            if js::truthy(&settings) {
                state.draft = Draft::from_settings(Some(&settings));
                state.draft_saved = Some(state.draft.clone());
            }
            // Скрипты, которые больше не нужны, но правлены руками, остались в файле игрока.
            state.edited_pending = jsv::prop(&result, "keptEdited")
                .and_then(Value::as_array)
                .filter(|list| !list.is_empty())
                .map(|list| list.iter().map(js::to_string).collect());
            // Бинды уже в файле игрока: закрывать игру не нужно, игрок выполняет его сам.
            let running = state
                .config_state
                .as_ref()
                .is_some_and(|value| jsv::truthy(jsv::prop(value, "running")));
            state.binds_import = running.then(|| Note {
                error: false,
                text: self.t(
                    "#L_KB_SAVED_RUNNING",
                    &[hlsr_delivery::game_binds::BINDS_FILE],
                ),
                details: None,
            });
            state.args_error = None;
            state.proton_error = None;
            drop(state);
            self.load_rule_check();
            let mut state = self.state.borrow_mut();
            if state.edits.is_empty() {
                false
            } else {
                state.edits = Edits::new();
                state.config_import = None;
                true
            }
        };
        if reload {
            self.load_config().await;
        }
    }

    /// `hasUnsaved()`: черновик отличается от загруженного, есть правки «Конфига», импорт cfg
    /// или текст в редакторе cfg.
    pub fn has_unsaved(&self) -> bool {
        let state = self.state.borrow();
        state.draft_saved.as_ref() != Some(&state.draft)
            || !state.edits.is_empty()
            || state.pending_cfg_import.is_some()
            || state.cfg_files_dirty
    }

    /// Настройки ещё читаются (`draftSaved === null`): править пока нечему.
    pub fn draft_loaded(&self) -> bool {
        self.state.borrow().draft_saved.is_some()
    }

    /// Сохранение отклонено — файл игрока изменён снаружи.
    pub fn file_changed(&self) -> bool {
        self.state.borrow().file_changed
    }

    /// «Отмена» на странице и «Не сохранять» в вопросе (`discardChanges`): несохранённое
    /// отброшено, настройки и файл игрока читаются заново.
    pub fn discard_changes(self: &Rc<Self>) {
        {
            let mut state = self.state.borrow_mut();
            // Идёт сохранение: импорт и черновик ему ещё нужны, перечитывание его перетёрло бы.
            if state.saving {
                return;
            }
            state.pending_cfg_import = None;
            state.binds_import = None;
            state.config_import = None;
            state.args_error = None;
            state.proton_error = None;
            state.config_error = None;
            state.edited_pending = None;
        }
        // `$refs.cfgFiles.revert()`: текст редактора cfg — к файлу на диске.
        let revert = self.cfg_files_revert.borrow().clone();
        if let Some(revert) = revert {
            revert();
        }
        self.state.borrow_mut().cfg_files_dirty = false;
        self.reload_after_change();
    }

    /// «Перечитать» (`reloadAfterChange`): настройки и файл игрока заново, правки «Конфига»
    /// отброшены.
    pub fn reload_after_change(self: &Rc<Self>) {
        self.state.borrow_mut().file_changed = false;
        let session = self.clone();
        self.tasks.spawn(async move {
            session.load(None).await;
            let had_edits = !session.state.borrow().edits.is_empty();
            if had_edits {
                session.state.borrow_mut().edits = Edits::new();
            }
            session.load_config().await;
        });
        self.dirty();
        self.pump();
    }

    /// Вернулись на страницу или в окно (`onWindowFocus`): файл игрока могли править руками.
    /// Перечитывается, только если правок нет; ответ применяется, только если за время чтения
    /// правок и сохранения не было и запрос последний.
    pub fn on_window_focus(self: &Rc<Self>) {
        if !self.idle() {
            return;
        }
        let token = {
            let mut state = self.state.borrow_mut();
            state.focus_token += 1;
            state.focus_token
        };
        let session = self.clone();
        self.tasks.spawn(async move {
            let accept = || session.state.borrow().focus_token == token && session.idle();
            if session.load(Some(&accept)).await {
                session.load_config_if(Some(&accept)).await;
            }
        });
        self.pump();
    }

    fn idle(&self) -> bool {
        let busy = {
            let state = self.state.borrow();
            state.saving || state.file_changed
        };
        !busy && !self.has_unsaved()
    }

    // ---- слот редактора файлов cfg ----------------------------------------------------------

    fn config_mode(state: &State) -> ConfigMode {
        if state.editor.is_some() {
            ConfigMode::Editor
        } else if jsv::is_str(
            state
                .config_info
                .as_ref()
                .and_then(|i| jsv::prop(i, "status")),
            "ok",
        ) && jsv::truthy(
            state
                .config_info
                .as_ref()
                .and_then(|i| jsv::prop(i, "exists")),
        ) && !jsv::is_str(
            state
                .config_state
                .as_ref()
                .and_then(|s| jsv::prop(s, "status")),
            "ok",
        ) {
            ConfigMode::FilesOnly
        } else {
            ConfigMode::Empty
        }
    }

    fn files_slot(&self, state: &State) -> Option<FilesSlot> {
        match Self::config_mode(state) {
            ConfigMode::Editor => {
                let editor = state.editor.as_ref()?;
                let check = state.rule_counts().is_some();
                editor.files_active(check).then(|| FilesSlot {
                    in_editor: true,
                    game_id: editor.game_id.clone(),
                    build: state.config_build.clone(),
                })
            }
            ConfigMode::FilesOnly => Some(FilesSlot {
                in_editor: false,
                game_id: self.env.game_id.clone(),
                build: state.config_build.clone(),
            }),
            ConfigMode::Empty => None,
        }
    }

    /// Редактор файлов cfg появился или сменил игру/сборку: `GameCfgFiles` просит список
    /// (`mounted()`, `watch gameId/build`) — редактор окна ([`Session::set_files_hook`]); без
    /// редактора (служебные окна сцен) вызов делает сама сессия.
    fn sync_files_slot(self: &Rc<Self>) {
        let (wanted, previous) = {
            let state = self.state.borrow();
            (self.files_slot(&state), state.files.clone())
        };
        if wanted == previous {
            return;
        }
        self.state.borrow_mut().files = wanted.clone();
        let hook = self.files_hook.borrow().clone();
        if let Some(hook) = hook {
            if !self.detached.get() {
                hook(wanted.as_ref());
            }
            return;
        }
        if let Some(slot) = wanted {
            let build = slot
                .build
                .as_ref()
                .map_or(Value::Null, |build| json!(build));
            self.fire("game-cfg-files-list", vec![json!(slot.game_id), build]);
        }
    }

    // ---- показ ------------------------------------------------------------------------------

    /// Всё, что окно показывает сейчас.
    pub fn view(&self) -> View {
        let state = self.state.borrow();
        let lang = self.env.language;
        let t = |key: &str, args: &[&str]| self.t(key, args);
        let list = self.section_context(&state);
        let active_id = sections::active_section(&list, &state.section);
        let active = list.iter().position(|s| s.id == active_id).unwrap_or(0);
        let goldsrc = draft::GOLDSRC_GAME_IDS.contains(&self.env.game_id.as_str());
        let toggles = draft::visible_toggles(goldsrc, &state.draft)
            .into_iter()
            .map(|toggle: &Toggle| ToggleView {
                key: toggle.key,
                label: t(toggle.label, &[]),
                hint: match toggle.hint {
                    draft::Hint::Raw(text) => text.to_owned(),
                    draft::Hint::Key(key) => t(key, &[]),
                },
                checked: state.draft.flag(toggle.key),
            })
            .collect();
        let resolution = SelectView {
            items: draft::resolution_items(&state.draft, &t),
            value: draft::resolution_value(&state.draft),
            label: draft::resolution_label(&state.draft, &t),
        };
        let refresh = SelectView {
            items: draft::refresh_items(&state.draft, &t),
            value: js::number::number_to_string(state.draft.number("refreshRate")),
            label: draft::refresh_label(&state.draft, &t),
        };
        let language_items = draft::language_items(&state.languages, &t);
        let language = SelectView {
            value: state.draft.text("language"),
            label: draft::language_label(&state.draft, &state.languages, &t),
            items: language_items,
        };
        let build_items = config::build_items(state.config_info.as_ref(), &t);
        let build = SelectView {
            label: config::build_label(&build_items, state.config_build.as_deref(), &t),
            value: state.config_build.clone().unwrap_or_default(),
            items: build_items,
        };
        let mode = Self::config_mode(&state);
        let info_ok = state.config_info.as_ref().is_some_and(|info| {
            jsv::is_str(jsv::prop(info, "status"), "ok") && jsv::truthy(jsv::prop(info, "exists"))
        });
        let running = state.editor.as_ref().is_some_and(|editor| editor.disabled);
        let player_file = state
            .editor
            .as_ref()
            .map(|editor| editor.player_file.clone())
            .unwrap_or_default();
        let (alert, alert_error) = match (&state.config_error, running) {
            (Some(text), _) => (text.clone(), true),
            (None, true) => (t("#L_GS_CFG_RUNNING", &[&player_file]), false),
            (None, false) => (String::new(), false),
        };
        let editor = state
            .editor
            .as_ref()
            .map(|editor| editor.editor_view(&state.edits, lang, &t, state.rule_counts()));
        let runs: Vec<RunRow> = state
            .runs
            .items
            .iter()
            .map(|run: &Run| RunRow {
                id: run.id.clone(),
                name: if run.display_name.is_empty() {
                    run.name.clone()
                } else {
                    run.display_name.clone()
                },
                size: format_size(run.size, &t),
                date: self.format_date(&run.modified_at),
                confirm: state.runs.pending_delete.as_deref() == Some(run.id.as_str()),
                deleting: state.runs.deleting.as_deref() == Some(run.id.as_str()),
            })
            .collect();
        let runs_body = if state.runs.loading {
            RunsBody::Loading
        } else if let Some(error) = &state.runs.error {
            RunsBody::Error(error.clone())
        } else if runs.is_empty() {
            RunsBody::Empty
        } else {
            RunsBody::List
        };
        let facts = self.facts(&state, &t);
        let footer = self.footer(&state, active_id, &t);
        let vulkan = state
            .vulkan
            .filter(|_| !self.env.linux)
            .map(|available| VulkanView {
                available,
                checked: available && state.draft.flag("vulkan"),
                hint: t(
                    if available {
                        "#L_GS_VULKAN_HINT"
                    } else {
                        "#L_GS_VULKAN_MISSING"
                    },
                    &[],
                ),
            });
        View {
            game_title: self.env.game_title.clone(),
            tabs: list
                .iter()
                .map(|section| TabView {
                    id: section.id,
                    label: t(section.title, &[]),
                    icon: section.icon,
                })
                .collect(),
            active,
            active_id,
            slide_next: state.slide == Direction::Next,
            load_state: state.load_state,
            goldsrc,
            toggles,
            vulkan,
            resolution,
            refresh,
            cpu0: state.draft.flag("excludeCpu0"),
            language_available: language.items.len() > 1,
            language,
            timer: state.draft.flag("startTimer"),
            args: state.draft.text("launchArgs"),
            args_error: state.args_error.clone().unwrap_or_default(),
            args_hint: t(
                if goldsrc {
                    "#L_GS_ARGS_HINT_GOLDSRC"
                } else {
                    "#L_GS_ARGS_HINT"
                },
                &[],
            ),
            args_placeholder: if goldsrc {
                "-heapsize 524288 +fps_max 100"
            } else {
                "+fps_max 300 -high"
            },
            linux: self.env.linux,
            proton: state.draft.text("protonOptions"),
            proton_error: state.proton_error.clone().unwrap_or_default(),
            config_mode: mode,
            build_select: build.items.len() > 1,
            build,
            path_text: config::path_text(state.config_info.as_ref(), &t),
            open_folder_enabled: info_ok,
            alert,
            alert_error,
            editor,
            empty_text: config::empty_text(state.config_state.as_ref(), &t),
            files: state.files.clone(),
            runs_summary: state.runs.summary(false, &t),
            runs_body,
            runs_error: state.runs.delete_error.clone().unwrap_or_default(),
            runs,
            runs_refresh_enabled: !state.runs.loading,
            runs_folder_enabled: state.runs.available(),
            facts,
            import_busy: state.importing_config,
            saving: state.saving,
            keyboard: list
                .iter()
                .any(|section| section.id == "keyboard")
                .then(|| self.keyboard_view(&state))
                .flatten(),
            scripts: list
                .iter()
                .any(|section| section.id == "scripts")
                .then(|| self.scripts_view(&state)),
            footer,
        }
    }

    /// Общий подвал: заметка справа (файл изменён, правленые скрипты, «применится после
    /// сохранения», итог импорта) и кнопки; в «Персонализации» подвала нет.
    fn footer(&self, state: &State, active_id: &str, t: Tr<'_>) -> FooterView {
        if active_id == "personal" {
            return FooterView {
                hidden: true,
                ..FooterView::default()
            };
        }
        let note_of = |note: &Note| {
            (
                note.text.clone(),
                note.error,
                note.details.clone().unwrap_or_default(),
            )
        };
        let (note, note_error, note_details) = if state.file_changed {
            (t("#L_GS_CFG_CHANGED", &[]), true, String::new())
        } else if let Some(edited) = &state.edited_pending {
            (
                t("#L_GS_SCRIPT_EDITED_TEXT", &[&edited.join(", ")]),
                true,
                t("#L_GS_SCRIPT_EDITED_TITLE", &[]),
            )
        } else if active_id == "scripts" {
            (t("#L_GS_X_APPLY_NOTE", &[]), false, String::new())
        } else if active_id == "keyboard" {
            match &state.binds_import {
                Some(note) => note_of(note),
                None => (t("#L_GS_X_APPLY_NOTE", &[]), false, String::new()),
            }
        } else if let ("configs", Some(note)) = (active_id, &state.config_import) {
            note_of(note)
        } else {
            (String::new(), false, String::new())
        };
        let draft_dirty = state.draft_saved.as_ref() != Some(&state.draft)
            || !state.edits.is_empty()
            || state.pending_cfg_import.is_some()
            || state.cfg_files_dirty;
        FooterView {
            hidden: false,
            saves: sections::footer_saves(active_id),
            save_enabled: !state.saving && state.draft_saved.is_some() && draft_dirty,
            note,
            note_error,
            note_details,
            file_changed: state.file_changed,
            edited: !state.file_changed && state.edited_pending.is_some(),
            remove_imported: active_id == "keyboard" && Self::imported_cfg_active(state),
            remove_imported_busy: state.importing_binds,
        }
    }

    fn format_date(&self, iso: &str) -> String {
        match self.env.zone {
            Zone::Local => records::format_date(iso, &Local),
            Zone::Fixed(offset) => records::format_date(iso, &offset),
        }
    }

    fn facts(&self, state: &State, t: Tr<'_>) -> Vec<Fact> {
        match self.env.zone {
            Zone::Local => records::version_facts(state.delivery.as_ref(), &Local, t),
            Zone::Fixed(offset) => records::version_facts(state.delivery.as_ref(), &offset, t),
        }
    }

    /// Правки вкладки «Конфиги» (тесты).
    pub fn edits(&self) -> Edits {
        self.state.borrow().edits.clone()
    }

    /// Черновик (тесты).
    pub fn draft(&self) -> Draft {
        self.state.borrow().draft.clone()
    }

    /// Выбранная сборка конфигов (тесты).
    pub fn config_build(&self) -> Option<String> {
        self.state.borrow().config_build.clone()
    }
}
