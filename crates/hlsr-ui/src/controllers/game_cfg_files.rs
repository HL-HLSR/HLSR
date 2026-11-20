//! Редактор cfg-файлов сборки (`GameCfgFiles.vue` 0.4.9, строка 5b.5 карты ПП5): список cfg
//! с поиском и усечением (основной файл игрока — первым, с меткой), текст с номерами строк, «не сохранено», поиск и замена (Aa, W, .*,
//! ошибка выражения, «≥N»), переход к строке, памятка клавиш, операции строк
//! (`hlsr_delivery::cfg_text_ops` — перенос `shared/cfg-text-ops.js`), своя отмена и повтор
//! ([`super::cfg_history`]), позиция курсора, подвал, сохранение.
//!
//! Без ввода-вывода и без Slint: вызовы каналов — [`Request`], ответы приходят в
//! `finish_*`; что сделать с полем (текст, выделение, фокус, прокрутка) — [`Effect`]. Смещения
//! в тексте и выделение — единицы UTF-16, как у `textarea`; поле Slint считает байтами
//! UTF-8 — перевод [`utf16_at`] и [`byte_at`].

use super::cfg_history::{EditKind, History, Range16};
use super::jsv;
use crate::tr;
use hlsr_core::language::Language;
use hlsr_delivery::cfg_text_ops::{self as ops, Edit, FindAll, Match, Options};
use serde_json::{Value, json};

/// Модификатор в памятке: `⌘` только на macOS (`platform === "darwin"`), её у лаунчера нет.
pub const MOD: &str = "Ctrl";

/// Строка памятки горячих клавиш: клавиши и ключ подписи.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    pub keys: &'static [&'static str],
    pub label: &'static str,
}

/// Памятка горячих клавиш редактора (`SHORTCUTS`).
pub const SHORTCUTS: [Shortcut; 13] = [
    Shortcut {
        keys: &[MOD, "S"],
        label: "#L_CF_KEY_SAVE",
    },
    Shortcut {
        keys: &[MOD, "F"],
        label: "#L_CF_KEY_FIND",
    },
    Shortcut {
        keys: &[MOD, "H"],
        label: "#L_CF_KEY_REPLACE",
    },
    Shortcut {
        keys: &["F3"],
        label: "#L_CF_KEY_NEXT",
    },
    Shortcut {
        keys: &["Shift", "F3"],
        label: "#L_CF_KEY_PREV",
    },
    Shortcut {
        keys: &[MOD, "G"],
        label: "#L_CF_KEY_GOTO",
    },
    Shortcut {
        keys: &[MOD, "/"],
        label: "#L_CF_KEY_COMMENT",
    },
    Shortcut {
        keys: &[MOD, "D"],
        label: "#L_CF_KEY_DUPLICATE",
    },
    Shortcut {
        keys: &[MOD, "Shift", "K"],
        label: "#L_CF_KEY_DELETE",
    },
    Shortcut {
        keys: &["Alt", "↑", "↓"],
        label: "#L_CF_KEY_MOVE",
    },
    Shortcut {
        keys: &["Tab", "Shift", "Tab"],
        label: "#L_CF_KEY_INDENT",
    },
    Shortcut {
        keys: &[MOD, "Z"],
        label: "#L_CF_KEY_UNDO",
    },
    Shortcut {
        keys: &["Esc"],
        label: "#L_CF_KEY_CLOSE",
    },
];

/// Флаг поиска: подпись на кнопке и ключ подсказки (`FIND_FLAGS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindFlag {
    CaseSensitive,
    WholeWord,
    Regex,
}

impl FindFlag {
    pub const ALL: [FindFlag; 3] = [
        FindFlag::CaseSensitive,
        FindFlag::WholeWord,
        FindFlag::Regex,
    ];

    pub fn text(self) -> &'static str {
        match self {
            FindFlag::CaseSensitive => "Aa",
            FindFlag::WholeWord => "W",
            FindFlag::Regex => ".*",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FindFlag::CaseSensitive => "#L_CF_MATCH_CASE",
            FindFlag::WholeWord => "#L_CF_WHOLE_WORD",
            FindFlag::Regex => "#L_CF_REGEX",
        }
    }
}

/// Коды ошибок main, у которых есть свой текст (`errorText`); прочие — `#L_CF_ERR_IO`.
const KNOWN_ERRORS: [&str; 6] = [
    "NOT_INSTALLED",
    "NOT_FOUND",
    "CHANGED",
    "TOO_LARGE",
    "ENCODING",
    "PATH_INVALID",
];

/// Ключ текста ошибки по коду ответа main.
pub fn error_key(code: Option<&str>) -> String {
    match code {
        Some(code) if KNOWN_ERRORS.contains(&code) => format!("#L_CF_ERR_{code}"),
        _ => "#L_CF_ERR_IO".to_owned(),
    }
}

/// Имя файла без папки (`baseName`).
pub fn base_name(path: &str) -> &str {
    path.rfind('/').map_or(path, |at| &path[at + 1..])
}

/// Папка файла (`dirName`): пусто, если файл в корне сборки (или путь начинается с «/»).
pub fn dir_name(path: &str) -> &str {
    match path.rfind('/') {
        Some(at) if at > 0 => &path[..at],
        _ => "",
    }
}

/// `/(^|\/)config\.cfg$/i`: config.cfg игры (его игра перепишет при выходе).
pub fn is_game_config(path: &str) -> bool {
    let name = base_name(path);
    name.eq_ignore_ascii_case("config.cfg")
}

/// Номера строк колонки слева: «1\n2\n…\nN».
pub fn line_numbers(lines: usize) -> String {
    let mut out = String::new();
    for n in 1..=lines.max(1) {
        if n > 1 {
            out.push('\n');
        }
        out.push_str(&n.to_string());
    }
    out
}

/// `picked.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")`.
pub fn escape_regexp(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if ".*+?^${}()|[]\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `parseInt(text, 10)`: пробелы слева, знак, цифры до первой не цифры; `None` — `NaN`.
pub fn parse_int(text: &str) -> Option<i64> {
    let rest = text.trim_start_matches(|c: char| hlsr_core::js::is_whitespace(c));
    let (negative, rest) = match rest.as_bytes().first() {
        Some(b'-') => (true, &rest[1..]),
        Some(b'+') => (false, &rest[1..]),
        _ => (false, rest),
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let value: i64 = digits.parse().unwrap_or(i64::MAX);
    Some(if negative { -value } else { value })
}

/// Байтовое смещение UTF-8 → единицы UTF-16 (смещение внутри символа — к его началу).
pub fn utf16_at(text: &str, byte: usize) -> usize {
    let mut units = 0;
    for (at, c) in text.char_indices() {
        if at >= byte {
            break;
        }
        if at + c.len_utf8() > byte {
            break;
        }
        units += c.len_utf16();
    }
    units
}

/// Единицы UTF-16 → байтовое смещение UTF-8; середина суррогатной пары — к началу символа,
/// за концом текста — конец.
pub fn byte_at(text: &str, unit: usize) -> usize {
    let mut units = 0;
    for (at, c) in text.char_indices() {
        if units + c.len_utf16() > unit {
            return at;
        }
        units += c.len_utf16();
    }
    text.len()
}

/// Длина в единицах UTF-16 (`String.length`).
pub fn len16(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `setSelectionRange(start, end)` у `textarea`: аргументы — `unsigned long` (−1 →
/// 4294967295), обрезка по длине, начало после конца — к концу.
pub fn selection_range(start: i64, end: i64, len: usize) -> Range16 {
    let clamp = |value: i64| -> usize {
        let unsigned = value.rem_euclid(1 << 32) as usize;
        unsigned.min(len)
    };
    let (start, end) = (clamp(start), clamp(end));
    (start.min(end), end)
}

/// Открытый файл (`current`).
#[derive(Debug, Clone, PartialEq)]
pub struct OpenFile {
    pub path: String,
    pub content: String,
    /// `encoding`, `eol`, `mtime` ответа чтения — как есть уходят в запись (`meta`).
    pub encoding: Value,
    pub eol: Value,
    pub mtime: Value,
}

/// Строка подвала: итог сохранения или ошибка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub error: bool,
    pub text: String,
}

/// Поиск и замена (`find`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Find {
    pub open: bool,
    pub replace: bool,
    pub query: String,
    pub replacement: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

impl Find {
    pub fn options(&self) -> Options {
        Options {
            regex: self.regex,
            whole_word: self.whole_word,
            case_sensitive: self.case_sensitive,
        }
    }

    pub fn flag(&self, flag: FindFlag) -> bool {
        match flag {
            FindFlag::CaseSensitive => self.case_sensitive,
            FindFlag::WholeWord => self.whole_word,
            FindFlag::Regex => self.regex,
        }
    }
}

/// Вызов канала main.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// `game-cfg-files-list(id, build)`.
    List { game_id: String, build: Value },
    /// `game-cfg-file-read(id, build, path)`.
    Read {
        game_id: String,
        build: Value,
        path: String,
    },
    /// `game-cfg-file-write(id, build, path, content, meta)`.
    Write {
        game_id: String,
        build: Value,
        path: String,
        content: String,
        meta: Value,
    },
}

impl Request {
    /// Канал и аргументы `ipcRenderer.invoke`.
    pub fn invoke(&self) -> (&'static str, Vec<Value>) {
        match self {
            Request::List { game_id, build } => {
                ("game-cfg-files-list", vec![json!(game_id), build.clone()])
            }
            Request::Read {
                game_id,
                build,
                path,
            } => (
                "game-cfg-file-read",
                vec![json!(game_id), build.clone(), json!(path)],
            ),
            Request::Write {
                game_id,
                build,
                path,
                content,
                meta,
            } => (
                "game-cfg-file-write",
                vec![
                    json!(game_id),
                    build.clone(),
                    json!(path),
                    json!(content),
                    meta.clone(),
                ],
            ),
        }
    }
}

/// Нажатие файла в списке (`open`).
#[derive(Debug, Clone, PartialEq)]
pub enum OpenStep {
    /// Этот файл уже открыт.
    Nothing,
    /// Есть несохранённые правки: сначала спросить (`confirmLeave`), затем [`CfgFiles::read`].
    Confirm,
    /// Читать файл: [`CfgFiles::read`].
    Read,
}

/// Куда перевести фокус.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    None,
    /// Текст (без прокрутки: `focus({preventScroll: true})`).
    Text,
    /// Поле поиска; `select` — выделить в нём всё.
    Find,
    Replace,
    Goto,
    /// Кнопка памятки.
    KeysToggle,
}

/// Что сделать с полем после действия.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Effect {
    /// Текст и выделение поля сменились: поставить [`CfgFiles::text`] и
    /// [`CfgFiles::selection`].
    pub apply: bool,
    /// Прокрутить к каретке, как Chromium после правки (`RevealSelectionAfterEditingOperation`).
    pub follow: bool,
    /// Прокрутка текста (`scrollTop`, `scrollLeft`), как у `reveal`; `None` — не трогать.
    pub scroll: Option<(f32, f32)>,
    /// Прокрутку текста — в начало (файл открыт).
    pub scroll_home: bool,
    pub focus: Focus,
    /// Выделить всё в поле фокуса (`input.select()`).
    pub select: bool,
    /// Начать сохранение ([`CfgFiles::begin_save`]).
    pub save: bool,
}

impl Effect {
    fn merge(mut self, other: Effect) -> Effect {
        self.apply |= other.apply;
        self.follow |= other.follow;
        if other.scroll.is_some() {
            self.scroll = other.scroll;
        }
        self.scroll_home |= other.scroll_home;
        if other.focus != Focus::None {
            self.focus = other.focus;
            self.select = other.select;
        }
        self.save |= other.save;
        self
    }
}

/// Замеры поля для `reveal`: высота строки, клиентская область, прокрутка, ширина символа
/// (`measureChar`, замер в Rust), `padding-left`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub line_height: f32,
    pub client_width: f32,
    pub client_height: f32,
    pub scroll_top: f32,
    pub scroll_left: f32,
    pub char_width: f32,
    pub pad_left: f32,
}

impl Default for View {
    fn default() -> View {
        View {
            line_height: LINE_HEIGHT,
            client_width: 0.0,
            client_height: 0.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            char_width: 7.5,
            pad_left: 12.0,
        }
    }
}

/// `line-height: 1.55` при 12.5px (`.cf__text`, `.cf__numbers`).
pub const LINE_HEIGHT: f32 = 12.5 * 1.55;

/// Высота строки перехода к строке (`.cf__find`: padding 7px сверху и снизу, строка 30px — высота
/// `.gx-btn`, рамка снизу 1px).
pub const GOTO_BAR_HEIGHT: f32 = 7.0 + 30.0 + 7.0 + 1.0;

/// Где нажата клавиша.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// Текст файла (`onKey`, затем `onEditorKey`).
    Text,
    /// Поле «Найти» (`onFindKey`, затем `onEditorKey`).
    Find,
    /// Поле «Заменить на» (`onReplaceKey`, затем `onEditorKey`).
    Replace,
    /// Поле «Строка» (`onGotoKey`, затем `onEditorKey`).
    Goto,
    /// Прочее внутри редактора (кнопки): только `onEditorKey`.
    Other,
}

/// Нажатая клавиша в виде DOM: `key` — «Tab», «Enter», «Escape», «F3», «ArrowUp»,
/// «ArrowDown», «Backspace», «Delete» или символ; `code` — физическая клавиша
/// (`KeyboardEvent.code`, пусто — неизвестна).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Key {
    pub key: String,
    pub code: String,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Key {
    pub fn new(key: &str) -> Key {
        Key {
            key: key.to_owned(),
            ..Key::default()
        }
    }

    pub fn ctrl(mut self) -> Key {
        self.ctrl = true;
        self
    }

    pub fn shift(mut self) -> Key {
        self.shift = true;
        self
    }

    pub fn alt(mut self) -> Key {
        self.alt = true;
        self
    }

    pub fn code(mut self, code: &str) -> Key {
        code.clone_into(&mut self.code);
        self
    }

    /// `event.ctrlKey || event.metaKey`.
    fn command(&self) -> bool {
        self.ctrl || self.meta
    }

    /// `event.key.toLowerCase()`.
    fn lower(&self) -> String {
        self.key.to_lowercase()
    }
}

/// Итог нажатия: `handled` — `preventDefault` (поле клавишу не обрабатывает),
/// `stop` — `stopPropagation` (Esc не закрывает окно).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct KeyOutcome {
    pub handled: bool,
    pub stop: bool,
    pub effect: Effect,
}

/// Состояние совпадений для `findState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindState {
    pub matches: Vec<Match>,
    pub error: bool,
    pub capped: bool,
}

/// Редактор cfg одной сборки игры.
pub struct CfgFiles {
    language: Language,
    game_id: String,
    build: Value,
    mounted: bool,
    pub files: Vec<String>,
    pub truncated: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub filter: String,
    /// Основной файл игрока (путь как в списке, `playerFile`): метка и первое место.
    pub player: Option<String>,
    pub current: Option<OpenFile>,
    text: String,
    pub reading: bool,
    pub saving: bool,
    pub running: bool,
    pub notice: Option<Notice>,
    pub find: Find,
    pub goto_open: bool,
    pub goto_line: String,
    pub keys_open: bool,
    selection: Range16,
    history: History,
    /// Чем будет следующая правка поля (по нажатой клавише).
    pending: Option<EditKind>,
    found: Option<(FindKey, FindState)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FindKey {
    text_len: usize,
    text_hash: u64,
    query: String,
    options: (bool, bool, bool),
}

fn hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

impl CfgFiles {
    pub fn new(language: Language) -> CfgFiles {
        CfgFiles {
            language,
            game_id: String::new(),
            build: Value::Null,
            mounted: false,
            files: Vec::new(),
            truncated: false,
            loading: false,
            error: None,
            filter: String::new(),
            player: None,
            current: None,
            text: String::new(),
            reading: false,
            saving: false,
            running: false,
            notice: None,
            find: Find::default(),
            goto_open: false,
            goto_line: String::new(),
            keys_open: false,
            selection: (0, 0),
            history: History::new(),
            pending: None,
            found: None,
        }
    }

    pub fn set_language(&mut self, language: Language) {
        self.language = language;
    }

    /// Язык текстов состояния.
    pub fn language(&self) -> Language {
        self.language
    }

    fn t(&self, key: &str, args: &[&str]) -> String {
        tr(self.language, key, args)
    }

    pub fn game_id(&self) -> &str {
        &self.game_id
    }

    pub fn build(&self) -> &Value {
        &self.build
    }

    // ---- Список ----------------------------------------------------------------------

    /// Игра или сборка окна (`props`): при первом показе — `load()` (`mounted`), при смене —
    /// `reset()` (`watch gameId/build`). `None` — ничего не сменилось.
    pub fn set_target(&mut self, game_id: &str, build: Value) -> Option<Request> {
        let changed = self.game_id != game_id || self.build != build;
        self.game_id = game_id.to_owned();
        self.build = build;
        if !self.mounted {
            self.mounted = true;
            return Some(self.begin_load());
        }
        if !changed {
            return None;
        }
        Some(self.reset())
    }

    /// `reset()`: файл закрыт, список заново.
    pub fn reset(&mut self) -> Request {
        self.current = None;
        self.text.clear();
        self.notice = None;
        self.history.clear();
        self.selection = (0, 0);
        self.begin_load()
    }

    /// `load()`: кнопка «Обновить список» и первый показ.
    pub fn begin_load(&mut self) -> Request {
        self.loading = true;
        self.error = None;
        Request::List {
            game_id: self.game_id.clone(),
            build: self.build.clone(),
        }
    }

    /// Ответ `game-cfg-files-list`.
    pub fn finish_load(&mut self, reply: Result<Value, String>) {
        self.loading = false;
        let result = match reply {
            Ok(result) => result,
            Err(_) => {
                self.error = Some(self.t(&error_key(None), &[]));
                return;
            }
        };
        if !jsv::is_str(jsv::prop(&result, "status"), "ok") {
            self.files.clear();
            let code = jsv::str_of(jsv::prop(&result, "code"));
            self.error = Some(self.t(&error_key(code), &[]));
            return;
        }
        self.files = jsv::prop(&result, "files")
            .and_then(Value::as_array)
            .map(|files| {
                files
                    .iter()
                    .filter_map(|file| jsv::str_of(jsv::prop(file, "path")).map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        self.player = jsv::str_of(jsv::prop(&result, "player")).map(str::to_owned);
        self.truncated = jsv::truthy(jsv::prop(&result, "truncated"));
        self.running = jsv::truthy(jsv::prop(&result, "running"));
    }

    /// `isPlayerFile`: основной файл игрока (без регистра).
    pub fn is_player_file(&self, path: &str) -> bool {
        self.player
            .as_ref()
            .is_some_and(|player| player.to_lowercase() == path.to_lowercase())
    }

    /// `shownFiles`: файлы, в пути которых есть строка поиска (без регистра, без пробелов по
    /// краям); основной файл игрока — первым, остальные в прежнем порядке.
    pub fn shown_files(&self) -> Vec<&str> {
        let query = self.filter.trim().to_lowercase();
        let mut list: Vec<&str> = self
            .files
            .iter()
            .map(String::as_str)
            .filter(|path| query.is_empty() || path.to_lowercase().contains(&query))
            .collect();
        list.sort_by_key(|path| !self.is_player_file(path));
        list
    }

    /// `openAt`: строка `line` (с 1) открытого файла — начало и конец без перевода строки
    /// (UTF-16); `None` — строки нет.
    pub fn line_span(&self, line: usize) -> Option<Range16> {
        if line == 0 || line > self.lines() {
            return None;
        }
        let start = ops::line_start(&self.text, line);
        let units: Vec<u16> = self.text.encode_utf16().collect();
        let end = units[start.min(units.len())..]
            .iter()
            .position(|unit| *unit == 0x0a)
            .map_or(units.len(), |at| start + at);
        Some((start, end))
    }

    /// Строка состояния списка вместо файлов: ошибка, «Ищем cfg…», «Ничего не нашлось» или
    /// «нет cfg»; `None` — список есть.
    pub fn list_state(&self) -> Option<(bool, String)> {
        if let Some(error) = &self.error {
            return Some((true, error.clone()));
        }
        if self.loading && self.files.is_empty() {
            return Some((false, self.t("#L_CF_LOADING", &[])));
        }
        if self.shown_files().is_empty() {
            let key = if self.filter.is_empty() {
                "#L_CF_EMPTY"
            } else {
                "#L_CF_NOTHING_FOUND"
            };
            return Some((false, self.t(key, &[])));
        }
        None
    }

    // ---- Файл ------------------------------------------------------------------------

    /// Текст в поле (`text`).
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Выделение поля `[selectionStart, selectionEnd]`, UTF-16.
    pub fn selection(&self) -> Range16 {
        self.selection
    }

    /// Строка и столбец каретки (`caret`: по `selectionEnd`).
    pub fn caret(&self) -> ops::LineCol {
        ops::line_col(&self.text, self.selection.1)
    }

    /// Правки не сохранены (`dirty`).
    pub fn dirty(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|current| self.text != current.content)
    }

    /// Строк в тексте (`lines`).
    pub fn lines(&self) -> usize {
        ops::line_count(&self.text)
    }

    /// Нажат файл списка (`open`).
    pub fn open(&self, path: &str) -> OpenStep {
        if self.current.as_ref().is_some_and(|c| c.path == path) {
            return OpenStep::Nothing;
        }
        if self.dirty() {
            return OpenStep::Confirm;
        }
        OpenStep::Read
    }

    fn read_request(&self, path: &str) -> Request {
        Request::Read {
            game_id: self.game_id.clone(),
            build: self.build.clone(),
            path: path.to_owned(),
        }
    }

    /// Читать файл (после [`OpenStep::Read`] или подтверждения).
    pub fn read(&mut self, path: &str) -> Request {
        self.reading = true;
        self.notice = None;
        self.read_request(path)
    }

    /// Ответ `game-cfg-file-read`. Файл открыт — поле получает текст, выделение в начале,
    /// прокрутка в начале, фокус в тексте, если поиск закрыт.
    pub fn finish_read(&mut self, reply: Result<Value, String>) -> Effect {
        self.reading = false;
        let result = match reply {
            Ok(result) => result,
            Err(_) => {
                self.notice = Some(self.error_notice(None));
                return Effect::default();
            }
        };
        if !jsv::is_str(jsv::prop(&result, "status"), "ok") {
            let code = jsv::str_of(jsv::prop(&result, "code"));
            self.notice = Some(self.error_notice(code));
            return Effect::default();
        }
        self.running = jsv::truthy(jsv::prop(&result, "running"));
        let field = |name: &str| jsv::prop(&result, name).cloned().unwrap_or(Value::Null);
        let content = jsv::str_of(jsv::prop(&result, "content"))
            .unwrap_or_default()
            .to_owned();
        self.current = Some(OpenFile {
            path: jsv::str_of(jsv::prop(&result, "path"))
                .unwrap_or_default()
                .to_owned(),
            content: content.clone(),
            encoding: field("encoding"),
            eol: field("eol"),
            mtime: field("mtime"),
        });
        self.text = content;
        self.selection = (0, 0);
        self.history.clear();
        Effect {
            apply: true,
            scroll_home: true,
            focus: if self.find.open {
                Focus::None
            } else {
                Focus::Text
            },
            ..Effect::default()
        }
    }

    fn error_notice(&self, code: Option<&str>) -> Notice {
        Notice {
            error: true,
            text: self.t(&error_key(code), &[]),
        }
    }

    /// «Вернуть» (`revert`): текст файла, подвал без итога. Новое значение поля ставит
    /// выделение в конец текста (`textarea.value =`).
    pub fn revert(&mut self) -> Effect {
        let Some(current) = &self.current else {
            return Effect::default();
        };
        if self.text != current.content {
            self.text = current.content.clone();
            let end = len16(&self.text);
            self.selection = (end, end);
        }
        self.history.clear();
        self.notice = None;
        Effect {
            apply: true,
            ..Effect::default()
        }
    }

    /// «Сохранить», Ctrl+S (`save`): `None` — нечего сохранять или уже сохраняется.
    pub fn begin_save(&mut self) -> Option<Request> {
        if !self.dirty() || self.saving {
            return None;
        }
        let current = self.current.as_ref()?;
        let request = Request::Write {
            game_id: self.game_id.clone(),
            build: self.build.clone(),
            path: current.path.clone(),
            content: self.text.clone(),
            meta: json!({
                "encoding": current.encoding,
                "eol": current.eol,
                "mtime": current.mtime,
            }),
        };
        self.saving = true;
        self.notice = None;
        Some(request)
    }

    /// Ответ `game-cfg-file-write`; `saved` — сохранённый текст (с начала записи).
    pub fn finish_save(&mut self, reply: Result<Value, String>, saved: &str) {
        self.saving = false;
        let result = match reply {
            Ok(result) => result,
            Err(_) => {
                self.notice = Some(self.error_notice(None));
                return;
            }
        };
        if !jsv::is_str(jsv::prop(&result, "status"), "ok") {
            let code = jsv::str_of(jsv::prop(&result, "code"));
            self.notice = Some(self.error_notice(code));
            return;
        }
        self.running = jsv::truthy(jsv::prop(&result, "running"));
        let mtime = jsv::prop(&result, "mtime").cloned().unwrap_or(Value::Null);
        let Some(current) = self.current.as_mut() else {
            return;
        };
        current.content = saved.to_owned();
        current.mtime = mtime;
        let name = base_name(&current.path).to_owned();
        let key = if self.running {
            "#L_CF_SAVED_RUNNING"
        } else {
            "#L_CF_SAVED"
        };
        self.notice = Some(Notice {
            error: false,
            text: self.t(key, &[&name, &name]),
        });
    }

    /// Строка подвала (`footNote`).
    pub fn foot_note(&self) -> String {
        if let Some(notice) = &self.notice {
            return notice.text.clone();
        }
        if self.running
            && self
                .current
                .as_ref()
                .is_some_and(|c| is_game_config(&c.path))
        {
            return self.t("#L_CF_RUNNING_CONFIG", &[]);
        }
        self.t("#L_CF_EXEC_HINT", &[])
    }

    // ---- Правки поля ------------------------------------------------------------------

    /// Поле само сменило текст (ввод, удаление, вставка, вырезание): `selection` — новое
    /// выделение (UTF-16).
    pub fn user_edit(&mut self, text: &str, selection: Range16) {
        if text == self.text {
            self.user_select(selection);
            return;
        }
        let kind = self.pending.take().unwrap_or(EditKind::Command);
        let before = std::mem::replace(&mut self.text, text.to_owned());
        self.history
            .record(kind, &before, &self.text, self.selection, selection);
        self.selection = selection;
    }

    /// Выделение сменилось не правкой (стрелки, щелчок, перетаскивание).
    pub fn user_select(&mut self, selection: Range16) {
        if selection != self.selection {
            self.history.close_typing();
        }
        self.selection = selection;
    }

    /// `applyEdit`: правка программы как `execCommand('insertText'|'delete')` — свой шаг
    /// отмены; затем выделение `selStart…selEnd`.
    pub fn apply_edit(&mut self, edit: &Edit) -> Effect {
        let len = len16(&self.text);
        let (start, end) = selection_range(edit.start as i64, edit.end as i64, len);
        let before_selection = (start, end);
        let (from, to, caret) = if !edit.insert.is_empty() {
            (start, end, start + len16(&edit.insert))
        } else if edit.start != edit.end && start == end {
            // `execCommand("delete")` при пустом выделении (начало правки после конца) стирает
            // символ перед кареткой.
            let back = self.previous_char_start(start);
            (back, start, back)
        } else {
            (start, end, start)
        };
        if from != to || !edit.insert.is_empty() {
            let units: Vec<u16> = self.text.encode_utf16().collect();
            let mut out = String::from_utf16_lossy(&units[..from]);
            out.push_str(&edit.insert);
            out.push_str(&String::from_utf16_lossy(&units[to..]));
            let before = std::mem::replace(&mut self.text, out);
            self.history.record(
                EditKind::Command,
                &before,
                &self.text,
                before_selection,
                (caret, caret),
            );
        }
        let len = len16(&self.text);
        self.selection = selection_range(edit.sel_start, edit.sel_end, len);
        self.history.close_typing();
        Effect {
            apply: true,
            follow: true,
            ..Effect::default()
        }
    }

    /// Начало символа перед `at` (UTF-16), с учётом суррогатной пары.
    fn previous_char_start(&self, at: usize) -> usize {
        if at == 0 {
            return 0;
        }
        let units: Vec<u16> = self.text.encode_utf16().collect();
        let low = units
            .get(at - 1)
            .is_some_and(|u| (0xdc00..0xe000).contains(u));
        let high = at >= 2
            && units
                .get(at - 2)
                .is_some_and(|u| (0xd800..0xdc00).contains(u));
        if low && high { at - 2 } else { at - 1 }
    }

    /// Ctrl+Z.
    pub fn undo(&mut self) -> Effect {
        match self.history.undo(&self.text) {
            Some(restored) => {
                self.text = restored.text;
                self.selection = restored.selection;
                Effect {
                    apply: true,
                    follow: true,
                    ..Effect::default()
                }
            }
            None => Effect::default(),
        }
    }

    /// Ctrl+Y, Ctrl+Shift+Z.
    pub fn redo(&mut self) -> Effect {
        match self.history.redo(&self.text) {
            Some(restored) => {
                self.text = restored.text;
                self.selection = restored.selection;
                Effect {
                    apply: true,
                    follow: true,
                    ..Effect::default()
                }
            }
            None => Effect::default(),
        }
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    // ---- Поиск и замена -------------------------------------------------------------

    /// `findState`: совпадения запроса (пусто, пока поиск закрыт или запрос пуст).
    pub fn find_state(&mut self) -> FindState {
        if !self.find.open || self.find.query.is_empty() {
            return FindState {
                matches: Vec::new(),
                error: false,
                capped: false,
            };
        }
        let key = FindKey {
            text_len: self.text.len(),
            text_hash: hash(&self.text),
            query: self.find.query.clone(),
            options: (
                self.find.regex,
                self.find.whole_word,
                self.find.case_sensitive,
            ),
        };
        if let Some((cached, state)) = &self.found
            && *cached == key
        {
            return state.clone();
        }
        let FindAll {
            matches,
            bad_regex,
            capped,
        } = ops::find_all(&self.text, &self.find.query, &self.find.options());
        let state = FindState {
            matches,
            error: bad_regex,
            capped,
        };
        self.found = Some((key, state.clone()));
        state
    }

    /// `findIndex`: выделенное совпадение, иначе ближайшее после курсора; −1 — нет.
    pub fn find_index(&mut self) -> i64 {
        let state = self.find_state();
        let (start, end) = self.selection;
        if let Some(exact) = state
            .matches
            .iter()
            .position(|m| m.start == start && m.end == end)
        {
            return exact as i64;
        }
        ops::nearest_match(&state.matches, start, false).map_or(-1, |at| at as i64)
    }

    /// `findCountText`: «N из M», «≥», «Нет совпадений», «Ошибка в выражении».
    pub fn find_count_text(&mut self) -> String {
        let state = self.find_state();
        if state.error {
            return self.t("#L_CF_BAD_REGEX", &[]);
        }
        if self.find.query.is_empty() {
            return String::new();
        }
        if state.matches.is_empty() {
            return self.t("#L_CF_NO_MATCHES", &[]);
        }
        let index = (self.find_index() + 1).to_string();
        let total = format!(
            "{}{}",
            if state.capped { "≥" } else { "" },
            state.matches.len()
        );
        self.t("#L_CF_MATCHES", &[&index, &total])
    }

    /// Выделение в одну строку не длиннее 200 — запрос поиска (`selectedSingleLine`).
    fn selected_single_line(&self) -> Option<String> {
        let (start, end) = self.selection;
        let units: Vec<u16> = self.text.encode_utf16().collect();
        let end = end.min(units.len());
        let start = start.min(end);
        let picked = String::from_utf16_lossy(&units[start..end]);
        (!picked.is_empty() && !picked.contains('\n') && end - start <= 200).then_some(picked)
    }

    /// Ctrl+F, Ctrl+H, кнопка поиска (`openFind`).
    pub fn open_find(&mut self, replace: bool) -> Effect {
        self.goto_open = false;
        if let Some(picked) = self.selected_single_line() {
            self.find.query = if self.find.regex {
                escape_regexp(&picked)
            } else {
                picked
            };
        }
        self.find.open = true;
        if replace {
            self.find.replace = true;
        }
        let focus = if replace && !self.find.query.is_empty() {
            Focus::Replace
        } else {
            Focus::Find
        };
        Effect {
            focus,
            select: true,
            ..Effect::default()
        }
    }

    /// `closeFind`.
    pub fn close_find(&mut self) -> Effect {
        self.find.open = false;
        self.find.replace = false;
        Effect {
            focus: Focus::Text,
            ..Effect::default()
        }
    }

    /// Стрелка у поля поиска (`toggleReplace`).
    pub fn toggle_replace(&mut self) -> Effect {
        self.find.replace = !self.find.replace;
        Effect {
            focus: if self.find.replace {
                Focus::Replace
            } else {
                Focus::None
            },
            ..Effect::default()
        }
    }

    pub fn toggle_flag(&mut self, flag: FindFlag) {
        match flag {
            FindFlag::CaseSensitive => self.find.case_sensitive = !self.find.case_sensitive,
            FindFlag::WholeWord => self.find.whole_word = !self.find.whole_word,
            FindFlag::Regex => self.find.regex = !self.find.regex,
        }
    }

    /// `reveal(start, end)`: выделить и прокрутить к участку, не забирая фокус у поиска.
    pub fn reveal(&mut self, start: usize, end: usize, view: &View) -> Effect {
        self.selection = (start, end);
        self.history.close_typing();
        let caret = ops::line_col(&self.text, end);
        let line_height = view.line_height;
        let top = (caret.line as f32 - 1.0) * line_height;
        let mut scroll_top = view.scroll_top;
        if top < view.scroll_top || top + line_height > view.scroll_top + view.client_height {
            scroll_top = (top - view.client_height / 2.0).max(0.0);
        }
        let from = ops::line_col(&self.text, start).col as f32 - 1.0;
        let left = from * view.char_width;
        let mut scroll_left = view.scroll_left;
        if left < view.scroll_left
            || left + view.pad_left * 2.0 > view.scroll_left + view.client_width
        {
            scroll_left = (left - view.client_width / 3.0).max(0.0);
        }
        Effect {
            apply: true,
            scroll: Some((scroll_top, scroll_left)),
            ..Effect::default()
        }
    }

    /// Стрелки, Enter, F3 (`step`): к следующему или предыдущему совпадению по кругу.
    pub fn step(&mut self, direction: i32, view: &View) -> Effect {
        let state = self.find_state();
        let (start, end) = self.selection;
        let index = if direction < 0 {
            ops::nearest_match(&state.matches, start, true)
        } else {
            ops::nearest_match(&state.matches, if end > start { end } else { start }, false)
        };
        match index {
            Some(index) => {
                let found = state.matches[index];
                self.reveal(found.start, found.end, view)
            }
            None => Effect::default(),
        }
    }

    /// «Заменить» (`replaceCurrent`): выделенное совпадение и переход к следующему; не выделено
    /// — просто к следующему.
    pub fn replace_current(&mut self, view: &View) -> Effect {
        let state = self.find_state();
        let (start, end) = self.selection;
        let Some(current) = state
            .matches
            .iter()
            .copied()
            .find(|m| m.start == start && m.end == end)
        else {
            return self.step(1, view);
        };
        let mut edit = ops::replace_one(
            &self.text,
            current,
            &self.find.query,
            &self.find.replacement,
            &self.find.options(),
        );
        edit.sel_start = edit.sel_end;
        let applied = self.apply_edit(&edit);
        applied.merge(self.step(1, view))
    }

    /// «Заменить всё» (`replaceEverything`): одна правка, каретка на прежнем месте (не дальше
    /// конца), прокрутка по вертикали не меняется.
    pub fn replace_everything(&mut self, view: &View) -> Effect {
        let result = ops::replace_all(
            &self.text,
            &self.find.query,
            &self.find.replacement,
            &self.find.options(),
        );
        let Some(mut edit) = result.edit.filter(|_| result.count > 0) else {
            return Effect::default();
        };
        let at = self.selection.0.min(len16(&edit.insert)) as i64;
        edit.sel_start = at;
        edit.sel_end = at;
        let mut effect = self.apply_edit(&edit);
        effect.follow = false;
        effect.scroll = Some((view.scroll_top, view.scroll_left));
        self.notice = Some(Notice {
            error: false,
            text: self.t("#L_CF_REPLACED", &[&result.count.to_string()]),
        });
        effect
    }

    // ---- Переход к строке ---------------------------------------------------------------

    /// Ctrl+G (`openGoto`).
    pub fn open_goto(&mut self) -> Effect {
        self.find.open = false;
        self.goto_open = true;
        self.goto_line = self.caret().line.to_string();
        Effect {
            focus: Focus::Goto,
            select: true,
            ..Effect::default()
        }
    }

    /// `closeGoto`.
    pub fn close_goto(&mut self) -> Effect {
        self.goto_open = false;
        Effect {
            focus: Focus::Text,
            ..Effect::default()
        }
    }

    /// «Перейти» (`goToLine`): номер в пределах `1…lines`, каретка в начале строки. Строка
    /// перехода закрывается до `reveal` (он идёт в `$nextTick`): поле к тому времени выше на
    /// [`GOTO_BAR_HEIGHT`].
    pub fn go_to_line(&mut self, view: &View) -> Effect {
        let line = parse_int(&self.goto_line)
            .filter(|&n| n != 0)
            .unwrap_or(1)
            .max(1)
            .min(self.lines() as i64) as usize;
        let at = ops::line_start(&self.text, line);
        let grown = if self.goto_open { GOTO_BAR_HEIGHT } else { 0.0 };
        self.goto_open = false;
        let view = View {
            client_height: view.client_height + grown,
            ..*view
        };
        let mut effect = self.reveal(at, at, &view);
        effect.focus = Focus::Text;
        effect
    }

    // ---- Памятка ---------------------------------------------------------------------

    pub fn toggle_keys(&mut self) {
        self.keys_open = !self.keys_open;
    }

    /// Нажатие мимо памятки и её кнопки (`onOutsidePointer`).
    pub fn close_keys(&mut self) {
        self.keys_open = false;
    }

    // ---- Клавиши ---------------------------------------------------------------------

    /// Нажатие в редакторе: обработчик поля, затем `onEditorKey` (всплытие).
    pub fn key(&mut self, field: Field, key: &Key, view: &View) -> KeyOutcome {
        self.pending = None;
        let own = match field {
            Field::Text => self.on_text_key(key, view),
            Field::Find => self.on_find_key(key, view),
            Field::Replace => self.on_replace_key(key, view),
            Field::Goto => self.on_goto_key(key, view),
            Field::Other => KeyOutcome::default(),
        };
        let editor = self.on_editor_key(key, view);
        KeyOutcome {
            handled: own.handled || editor.handled,
            stop: own.stop || editor.stop,
            effect: own.effect.merge(editor.effect),
        }
    }

    /// `onKey` текста: сохранение, строки, отступы, своя отмена; прочие клавиши — полю, с
    /// пометкой, чем будет правка.
    fn on_text_key(&mut self, key: &Key, view: &View) -> KeyOutcome {
        let command = key.command();
        let lower = key.lower();
        let (start, end) = self.selection;
        let handled = |effect: Effect| KeyOutcome {
            handled: true,
            stop: false,
            effect,
        };
        if command && !key.shift && !key.alt && lower == "s" {
            return handled(Effect {
                save: true,
                ..Effect::default()
            });
        }
        // Своя отмена (решение 23): встроенная у поля Slint не знает правок программы.
        if command && !key.alt && lower == "z" {
            return handled(if key.shift { self.redo() } else { self.undo() });
        }
        if command && !key.shift && !key.alt && lower == "y" {
            return handled(self.redo());
        }
        // Ctrl+/ — и по физической клавише: в русской раскладке на ней «.».
        let edit = if command && !key.alt && (key.key == "/" || key.code == "Slash") {
            Some(ops::toggle_comment(&self.text, start, end))
        } else if command && !key.shift && !key.alt && lower == "d" {
            Some(ops::duplicate_lines(&self.text, start, end))
        } else if command && key.shift && !key.alt && lower == "k" {
            Some(ops::delete_lines(&self.text, start, end))
        } else if key.alt && !command && (key.key == "ArrowUp" || key.key == "ArrowDown") {
            let direction = if key.key == "ArrowUp" { -1 } else { 1 };
            match ops::move_lines(&self.text, start, end, direction) {
                Some(edit) => Some(edit),
                // preventDefault и без правки.
                None => return handled(Effect::default()),
            }
        } else if key.key == "Tab" && !command && !key.alt {
            let units: Vec<u16> = self.text.encode_utf16().collect();
            let multiline = units[start.min(units.len())..end.min(units.len())].contains(&0x0a);
            if key.shift || multiline {
                Some(ops::indent_lines(&self.text, start, end, key.shift))
            } else {
                let at = start as i64 + 1;
                Some(Edit {
                    start,
                    end,
                    insert: "\t".to_owned(),
                    sel_start: at,
                    sel_end: at,
                })
            }
        } else {
            None
        };
        if let Some(edit) = edit {
            return handled(self.apply_edit(&edit));
        }
        let _ = view;
        self.pending = Some(match key.key.as_str() {
            "Backspace" => EditKind::DeleteBackward,
            "Delete" => EditKind::DeleteForward,
            _ if key.ctrl && !key.shift && lower == "v" => EditKind::Paste,
            _ if key.ctrl && !key.shift && lower == "x" => EditKind::Cut,
            _ => EditKind::Insert,
        });
        KeyOutcome::default()
    }

    /// `onFindKey`: Enter — следующее, Shift+Enter — предыдущее.
    fn on_find_key(&mut self, key: &Key, view: &View) -> KeyOutcome {
        if key.key != "Enter" {
            return KeyOutcome::default();
        }
        KeyOutcome {
            handled: true,
            stop: false,
            effect: self.step(if key.shift { -1 } else { 1 }, view),
        }
    }

    /// `onReplaceKey`: Enter — заменить, Ctrl+Alt+Enter — заменить всё.
    fn on_replace_key(&mut self, key: &Key, view: &View) -> KeyOutcome {
        if key.key != "Enter" {
            return KeyOutcome::default();
        }
        let effect = if key.command() && key.alt {
            self.replace_everything(view)
        } else {
            self.replace_current(view)
        };
        KeyOutcome {
            handled: true,
            stop: false,
            effect,
        }
    }

    /// `onGotoKey`: Enter — перейти.
    fn on_goto_key(&mut self, key: &Key, view: &View) -> KeyOutcome {
        if key.key != "Enter" {
            return KeyOutcome::default();
        }
        KeyOutcome {
            handled: true,
            stop: false,
            effect: self.go_to_line(view),
        }
    }

    /// `onEditorKey`: Ctrl+F/H/G, F3, Esc — и в тексте, и в полях поиска.
    fn on_editor_key(&mut self, key: &Key, view: &View) -> KeyOutcome {
        let command = key.command();
        let lower = key.lower();
        let handled = |effect: Effect| KeyOutcome {
            handled: true,
            stop: false,
            effect,
        };
        if command && !key.alt && (lower == "f" || lower == "h") {
            return handled(self.open_find(lower == "h"));
        }
        if command && !key.alt && lower == "g" {
            return handled(self.open_goto());
        }
        if key.key == "F3" {
            return handled(if !self.find.open {
                self.open_find(false)
            } else {
                self.step(if key.shift { -1 } else { 1 }, view)
            });
        }
        if key.key == "Escape" && (self.find.open || self.goto_open || self.keys_open) {
            let effect = if self.keys_open {
                self.keys_open = false;
                Effect {
                    focus: Focus::KeysToggle,
                    ..Effect::default()
                }
            } else if self.goto_open {
                self.close_goto()
            } else {
                self.close_find()
            };
            return KeyOutcome {
                handled: true,
                stop: true,
                effect,
            };
        }
        KeyOutcome::default()
    }
}

/// Где встаёт памятка (`placeKeys`): под кнопкой, если там хватает места, иначе туда, где
/// места больше; список прокручивается в пределах окна. Координаты — окна, CSS px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeysPlace {
    /// `top` (под кнопкой) или `bottom` (над ней) — расстояние от края окна.
    pub below: bool,
    pub top: f32,
    pub bottom: f32,
    pub right: f32,
    pub max_height: f32,
}

/// `placeKeys`: `toggle` — прямоугольник кнопки (x, y, ширина, высота), `height` —
/// `scrollHeight` памятки.
pub fn place_keys(toggle: (f32, f32, f32, f32), window: (f32, f32), height: f32) -> KeysPlace {
    const MARGIN: f32 = 12.0;
    const GAP: f32 = 6.0;
    let (x, y, width, h) = toggle;
    let (window_width, window_height) = window;
    let rect_bottom = y + h;
    let rect_right = x + width;
    let below = window_height - rect_bottom - GAP - MARGIN;
    let above = y - GAP - MARGIN;
    let right = MARGIN.max(window_width - rect_right);
    if height <= below || below >= above {
        KeysPlace {
            below: true,
            top: rect_bottom + GAP,
            bottom: 0.0,
            right,
            max_height: below,
        }
    } else {
        KeysPlace {
            below: false,
            top: 0.0,
            bottom: window_height - y + GAP,
            right,
            max_height: above,
        }
    }
}
