//! Бинды игр (`shared/game-binds.js`): имена клавиш движков, разбор cfg и переназначения.
//!
//! GoldSrc (HL1) выполняет config.cfg, а он последней строкой — userconfig.cfg.
//! Source (HL2, Ghosting) выполняет cfg/config.cfg, потом cfg/autoexec.cfg.
//! Оба движка перезаписывают config.cfg при выходе: в начале unbindall, потом все
//! бинды. Переназначения лаунчера пишутся в hlsr-binds.cfg и подключаются
//! последней строкой userconfig.cfg или autoexec.cfg, поэтому всегда главнее.
//!
//! После первого запуска игра сохранит переназначенный бинд в config.cfg, и
//! прежнее значение оттуда пропадёт. Поэтому у каждого переназначения хранится
//! `base` — что было на клавише до лаунчера. Отмена переназначения не удаляет
//! запись, а превращает её в «возврат»: `command = base`.
//!
//! Тексты cfg здесь — строки, как их видит JS: для GoldSrc файл читается
//! побайтно (latin1: байт → символ U+00XX), для Source — как UTF-8. Длины,
//! которые 0.4.7 считает по `String.length`, считаются в единицах UTF-16.
//! Объекты JS с произвольными ключами — [`JsObject`]: порядок ключей у них
//! тот же, что у `Object.keys`.

use hlsr_core::js::object::JsObject;
use hlsr_core::js::{self, cmp_utf16};
use regress::Regex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::LazyLock;

/// Файл переназначений лаунчера.
pub const BINDS_FILE: &str = "hlsr-binds.cfg";
pub const BINDS_EXEC_LINE: &str = "exec hlsr-binds.cfg";
/// Файл импортированного конфига (так его подключали прошлые версии лаунчера;
/// в предпросмотре ещё не сохранённого импорта это метка его биндов).
pub const IMPORT_FILE: &str = "hlsr-import.cfg";
pub const IMPORT_EXEC_LINE: &str = "exec hlsr-import.cfg";
pub const MAX_IMPORT_BYTES: usize = 256 * 1024;
/// Самая длинная команда бинда, единиц UTF-16.
pub const MAX_COMMAND: usize = 255;
/// Сколько переназначений лаунчер хранит.
pub const MAX_OVERRIDES: usize = 300;

/// Имена из gfx/shell/kb_keys.lst HL1, без джойстика и AUX. Старый Source
/// (HL2 2707) называет клавиши так же.
pub const GOLDSRC_KEYS: &[&str] = &[
    "TAB",
    "ENTER",
    "ESCAPE",
    "SPACE",
    "BACKSPACE",
    "UPARROW",
    "DOWNARROW",
    "LEFTARROW",
    "RIGHTARROW",
    "ALT",
    "CTRL",
    "SHIFT",
    "CAPSLOCK",
    "PAUSE",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "INS",
    "DEL",
    "PGDN",
    "PGUP",
    "HOME",
    "END",
    "KP_HOME",
    "KP_UPARROW",
    "KP_PGUP",
    "KP_LEFTARROW",
    "KP_5",
    "KP_RIGHTARROW",
    "KP_END",
    "KP_DOWNARROW",
    "KP_PGDN",
    "KP_ENTER",
    "KP_INS",
    "KP_DEL",
    "KP_SLASH",
    "KP_MINUS",
    "KP_PLUS",
    "MOUSE1",
    "MOUSE2",
    "MOUSE3",
    "MOUSE4",
    "MOUSE5",
    "MWHEELUP",
    "MWHEELDOWN",
];

/// Source с inputsystem (HL2 4260, Ghosting) различает правые модификаторы,
/// Win, Num Lock и Scroll Lock, а Num * зовёт KP_MULTIPLY.
pub const SOURCE_KEYS: &[&str] = &[
    "TAB",
    "ENTER",
    "ESCAPE",
    "SPACE",
    "BACKSPACE",
    "UPARROW",
    "DOWNARROW",
    "LEFTARROW",
    "RIGHTARROW",
    "ALT",
    "CTRL",
    "SHIFT",
    "CAPSLOCK",
    "PAUSE",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "INS",
    "DEL",
    "PGDN",
    "PGUP",
    "HOME",
    "END",
    "KP_HOME",
    "KP_UPARROW",
    "KP_PGUP",
    "KP_LEFTARROW",
    "KP_5",
    "KP_RIGHTARROW",
    "KP_END",
    "KP_DOWNARROW",
    "KP_PGDN",
    "KP_ENTER",
    "KP_INS",
    "KP_DEL",
    "KP_SLASH",
    "KP_MINUS",
    "KP_PLUS",
    "MOUSE1",
    "MOUSE2",
    "MOUSE3",
    "MOUSE4",
    "MOUSE5",
    "MWHEELUP",
    "MWHEELDOWN",
    "RSHIFT",
    "RCTRL",
    "RALT",
    "LWIN",
    "RWIN",
    "APP",
    "NUMLOCK",
    "SCROLLLOCK",
    "KP_MULTIPLY",
];

static NAMED: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| SOURCE_KEYS.iter().copied().collect());

/// Как движок читает и пишет cfg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Latin1,
    Utf8,
}

impl Encoding {
    /// Название кодировки в `fs` Node.
    pub fn as_str(self) -> &'static str {
        match self {
            Encoding::Latin1 => "latin1",
            Encoding::Utf8 => "utf8",
        }
    }
}

/// Движок игры: где лежат cfg, с чего он начинает их выполнять и в какой файл
/// лаунчер дописывает exec своего hlsr-binds.cfg.
#[derive(Debug)]
pub struct Engine {
    pub name: &'static str,
    pub keys: &'static [&'static str],
    /// Папка cfg относительно папки игры.
    pub cfg_dir: &'static str,
    /// С чего движок начинает выполнять cfg.
    pub roots: &'static [&'static str],
    pub exec_file: &'static str,
    pub encoding: Encoding,
}

pub static GOLDSRC: Engine = Engine {
    name: "goldsrc",
    keys: GOLDSRC_KEYS,
    cfg_dir: "",
    roots: &["config.cfg"],
    exec_file: "userconfig.cfg",
    encoding: Encoding::Latin1,
};

pub static SOURCE: Engine = Engine {
    name: "source",
    keys: SOURCE_KEYS,
    cfg_dir: "cfg",
    roots: &["config.cfg", "autoexec.cfg"],
    exec_file: "autoexec.cfg",
    encoding: Encoding::Utf8,
};

pub static SOURCE_OLD: Engine = Engine {
    name: "source-old",
    keys: GOLDSRC_KEYS,
    cfg_dir: "cfg",
    roots: &["config.cfg", "autoexec.cfg"],
    exec_file: "autoexec.cfg",
    encoding: Encoding::Utf8,
};

/// `engineOf`: незнакомое имя — GoldSrc.
pub fn engine_of(name: &str) -> &'static Engine {
    match name {
        "source" => &SOURCE,
        "source-old" => &SOURCE_OLD,
        _ => &GOLDSRC,
    }
}

/// Бинд из cfg: команда и файл, из которого он взят.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bind {
    pub command: String,
    pub source: String,
}

/// Итог выполнения цепочки cfg: бинды по клавишам, alias, нужные переменные и
/// файлы, которые удалось прочитать.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BindRead {
    pub binds: JsObject<Bind>,
    pub aliases: JsObject<String>,
    pub cvars: JsObject<String>,
    pub files: Vec<String>,
    /// Файлы подключённого импорта (пути в нижнем регистре); `read_binds` их не
    /// знает, заполняет вызывающий.
    pub imported: Vec<String>,
    /// Бинды, которые появляются только по нажатию клавиши ([`runtime_binds`]).
    pub runtime: RuntimeBinds,
}

/// Бинд, который ставит cfg по нажатию другой клавиши: `trigger` — клавиша, которая его
/// ставит, `file` — файл с этим bind (`None` — bind прямо в alias или команде клавиши).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBind {
    pub command: String,
    pub file: Option<String>,
    pub trigger: String,
}

/// Бинды по нажатию по клавишам (ключ — имя клавиши после [`normalize_key`]), до 4 на клавишу.
pub type RuntimeBinds = JsObject<Vec<RuntimeBind>>;

/// `runtime` в виде JSON, как его отдаёт 0.4.9: `{ key: [{ command, file, trigger }] }`.
/// Пока не подключено: в `keyBinds` ответа `game-settings-get` поле добавит задача A (каналы
/// `game-settings-*`), разбор в окне (`key_binds_from_value`) и показ — задача G7.
pub fn runtime_to_value(runtime: &RuntimeBinds) -> Value {
    Value::Object(
        runtime
            .iter()
            .map(|(key, list)| {
                let items = list
                    .iter()
                    .map(|item| {
                        serde_json::json!({"command": item.command, "file": item.file, "trigger": item.trigger})
                    })
                    .collect();
                (key.to_owned(), Value::Array(items))
            })
            .collect(),
    )
}

/// Переназначение клавиши лаунчера: что назначить и что было до лаунчера.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    pub command: String,
    pub base: String,
}

impl Override {
    pub fn to_value(&self) -> Value {
        serde_json::json!({"command": self.command, "base": self.base})
    }
}

/// Переназначения по клавишам (ключ — имя клавиши после [`normalize_key`]).
pub type Overrides = JsObject<Override>;

/// Переназначения в виде JSON объекта: так они хранятся в настройках игры.
pub fn overrides_to_value(overrides: &Overrides) -> Value {
    Value::Object(
        overrides
            .iter()
            .map(|(key, entry)| (key.to_owned(), entry.to_value()))
            .collect(),
    )
}

/// Почему команда не годится для бинда.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandError {
    /// Не строка.
    Invalid,
    TooLong,
    ForbiddenChars,
}

impl CommandError {
    /// Код, как у `commandError` 0.4.7.
    pub fn code(self) -> &'static str {
        match self {
            CommandError::Invalid => "INVALID",
            CommandError::TooLong => "TOO_LONG",
            CommandError::ForbiddenChars => "FORBIDDEN_CHARS",
        }
    }
}

/// Имя клавиши как его понимает движок: одиночный символ без учёта регистра
/// (буквы в нижнем), длинные имена — в верхнем. Кавычку забиндить нельзя: cfg не
/// умеет её экранировать. `SEMICOLON` — это «;».
pub fn normalize_key(name: &str) -> Option<String> {
    let raw = js::trim(name);
    let mut units = raw.encode_utf16();
    if let (Some(code), None) = (units.next(), units.clone().next()) {
        if !(33..=126).contains(&code) || raw == "\"" {
            return None;
        }
        return Some(raw.to_lowercase());
    }
    let upper = raw.to_uppercase();
    if upper == "SEMICOLON" {
        return Some(";".to_owned());
    }
    NAMED.contains(upper.as_str()).then_some(upper)
}

/// `commandError` для строки. Кавычки cfg не экранирует, управляющие символы
/// рвут строку.
pub fn command_error(command: &str) -> Option<CommandError> {
    if command.encode_utf16().count() > MAX_COMMAND {
        return Some(CommandError::TooLong);
    }
    let forbidden = command
        .chars()
        .any(|c| c == '"' || u32::from(c) < 32 || c == '\u{7f}');
    forbidden.then_some(CommandError::ForbiddenChars)
}

/// `commandError` для значения из JSON: не строка — `INVALID`.
pub fn command_error_value(command: &Value) -> Option<CommandError> {
    match command {
        Value::String(text) => command_error(text),
        _ => Some(CommandError::Invalid),
    }
}

/// `cleanCommand`: пробелы по краям убраны, середина — по одному пробелу.
pub fn clean_command(command: &str) -> String {
    collapse_whitespace(js::trim(command))
}

/// `replace(/\s+/g, " ")`.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_run = false;
    for c in text.chars() {
        if js::is_whitespace(c) {
            if !in_run {
                out.push(' ');
            }
            in_run = true;
        } else {
            out.push(c);
            in_run = false;
        }
    }
    out
}

/// Разбор cfg так же, как это делает движок: команды разделяются переводом
/// строки или «;» вне кавычек, «//» вне кавычек — комментарий до конца
/// строки, кавычки не экранируются. Каждая команда — список аргументов.
pub fn parse_cfg(text: &str) -> Vec<Vec<String>> {
    #[derive(Default)]
    struct State {
        commands: Vec<Vec<String>>,
        args: Vec<String>,
        token: Option<String>,
    }
    impl State {
        fn end_token(&mut self) {
            if let Some(token) = self.token.take() {
                self.args.push(token);
            }
        }

        fn end_command(&mut self) {
            self.end_token();
            if !self.args.is_empty() {
                self.commands.push(std::mem::take(&mut self.args));
            }
        }
    }

    let chars: Vec<char> = text.chars().collect();
    let mut state = State::default();
    let mut quoted = false;
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        i += 1;
        if quoted {
            if ch == '"' {
                quoted = false;
                let token = state.token.take().unwrap_or_default();
                state.args.push(token);
            } else if ch == '\n' {
                quoted = false;
                state.end_command();
            } else {
                state.token.get_or_insert_with(String::new).push(ch);
            }
            continue;
        }
        match ch {
            '"' => {
                state.end_token();
                quoted = true;
                state.token = Some(String::new());
            }
            '\n' | ';' => state.end_command(),
            '/' if chars.get(i) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                state.end_command();
                // Перевод строки, на котором остановились, пропускает сам цикл.
                i += 1;
            }
            ' ' | '\t' | '\r' => state.end_token(),
            _ => state.token.get_or_insert_with(String::new).push(ch),
        }
    }
    if quoted {
        let token = state.token.take().unwrap_or_default();
        state.args.push(token);
    }
    state.end_command();
    state.commands
}

/// Переменные, значения которых нужны лаунчеру из cfg игрока.
const READ_CVARS: [&str; 2] = ["sensitivity", "m_pitch"];

/// Имя файла, который лаунчер соглашается читать по `exec`:
/// `^[A-Za-z0-9_-][A-Za-z0-9._ -]*(\/[A-Za-z0-9_-][A-Za-z0-9._ -]*){0,2}\.cfg$`.
fn is_readable_cfg_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".cfg") else {
        return false;
    };
    let mut parts = 0;
    for part in stem.split('/') {
        parts += 1;
        let mut chars = part.chars();
        let first_ok = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if parts > 3 || !first_ok || !chars.all(|c| c.is_ascii_alphanumeric() || "._ -".contains(c))
        {
            return false;
        }
    }
    true
}

/// Выполняет цепочку cfg движка (`roots` по порядку) и собирает итоговые бинды,
/// alias и нужные переменные. Свой hlsr-binds.cfg лаунчер пропускает:
/// переназначения он знает и так. `read_file(имя)` возвращает текст файла или
/// `None`, если файла нет.
pub fn read_binds(mut read_file: impl FnMut(&str) -> Option<String>, roots: &[&str]) -> BindRead {
    struct Run<'a> {
        read_file: &'a mut dyn FnMut(&str) -> Option<String>,
        read: BindRead,
        visited: HashSet<String>,
    }

    fn run(state: &mut Run, raw: &str, depth: usize) {
        // Файлы в подпапках мода тоже выполняются (exec scripts/jump.cfg).
        let name = raw.replace('\\', "/");
        let lower = name.to_lowercase();
        if depth > 8 || state.visited.contains(&lower) || lower == BINDS_FILE {
            return;
        }
        if !is_readable_cfg_name(&name) {
            return;
        }
        let Some(text) = (state.read_file)(&name) else {
            return;
        };
        state.visited.insert(lower);
        state.read.files.push(name.clone());
        for args in parse_cfg(&text) {
            let verb = args[0].to_lowercase();
            if verb == "unbindall" {
                state.read.binds.clear();
            } else if verb == "bind" && args.len() >= 3 {
                if let Some(key) = normalize_key(&args[1]) {
                    let command = clean_command(&args[2..].join(" "));
                    state.read.binds.insert(
                        key,
                        Bind {
                            command,
                            source: name.clone(),
                        },
                    );
                }
            } else if verb == "unbind" && args.len() >= 2 {
                if let Some(key) = normalize_key(&args[1]) {
                    state.read.binds.remove(&key);
                }
            } else if verb == "alias" && args.len() >= 2 {
                state
                    .read
                    .aliases
                    .insert(args[1].clone(), args[2..].join(" "));
            } else if verb == "exec" && args.len() >= 2 {
                run(state, &args[1], depth + 1);
            } else if READ_CVARS.contains(&verb.as_str()) && args.len() >= 2 {
                state.read.cvars.insert(verb, args[1].clone());
            }
        }
    }

    let mut state = Run {
        read_file: &mut read_file,
        read: BindRead::default(),
        visited: HashSet::new(),
    };
    for root in roots {
        run(&mut state, root, 0);
    }
    let mut read = state.read;
    read.runtime = runtime_binds(&read.binds, &read.aliases, read_file, &read.files);
    read
}

/// Сколько команд разбирает [`runtime_binds`] на все клавиши: огромный или зацикленный
/// конфиг не вешает окно.
const RUNTIME_BUDGET: i64 = 5000;

/// Бинды, которые появляются только по нажатию: клавиша зовёт exec (`bind k "exec tc_n.cfg"`,
/// в том числе через alias), а файл назначает другие клавиши. Стартовые файлы
/// (`startup_files`) уже учтены в `binds` и не читаются. Бинд, совпадающий со стартовым,
/// не считается (`runtimeBinds` 0.4.9).
pub fn runtime_binds(
    binds: &JsObject<Bind>,
    aliases: &JsObject<String>,
    mut read_file: impl FnMut(&str) -> Option<String>,
    startup_files: &[String],
) -> RuntimeBinds {
    #[derive(Clone)]
    struct Ctx<'a> {
        trigger: &'a str,
        file: Option<String>,
        called: HashSet<String>,
    }
    struct Walk<'a, F: FnMut(&str) -> Option<String>> {
        binds: &'a JsObject<Bind>,
        read_file: F,
        startup: HashSet<String>,
        // Каждый файл читается и разбирается один раз, сколько бы клавиш его ни звали; `Rc` —
        // чтобы exec не копировал весь разобранный файл.
        parsed: HashMap<String, Option<Rc<Vec<Vec<String>>>>>,
        budget: i64,
        runtime: RuntimeBinds,
        // `ctx.files` и `ctx.aliases` у 0.4.9 общие на всю клавишу, `called` — свой у файла.
        files: HashSet<String>,
        aliases: HashMap<String, String>,
    }
    impl<F: FnMut(&str) -> Option<String>> Walk<'_, F> {
        fn record(&mut self, raw_key: &str, command: String, ctx: &Ctx) {
            let Some(key) = normalize_key(raw_key) else {
                return;
            };
            if self
                .binds
                .get(&key)
                .is_some_and(|bind| bind.command == command)
            {
                return;
            }
            if self.runtime.get(&key).is_none() {
                self.runtime.insert(key.clone(), Vec::new());
            }
            let list = self.runtime.get_mut(&key).expect("список вставлен выше");
            if list.len() < 4
                && !list
                    .iter()
                    .any(|entry| entry.command == command && entry.trigger == ctx.trigger)
            {
                list.push(RuntimeBind {
                    command,
                    file: ctx.file.clone(),
                    trigger: ctx.trigger.to_owned(),
                });
            }
        }

        fn walk(&mut self, commands: &[Vec<String>], ctx: &mut Ctx, depth: usize) {
            if depth > 8 {
                return;
            }
            for args in commands {
                self.budget -= 1;
                if self.budget < 0 {
                    return;
                }
                let verb = args[0].to_lowercase();
                if verb == "bind" && args.len() >= 3 {
                    self.record(&args[1], clean_command(&args[2..].join(" ")), ctx);
                } else if verb == "exec" && args.len() >= 2 {
                    let name = args[1].replace('\\', "/");
                    let lower = name.to_lowercase();
                    if self.startup.contains(&lower)
                        || self.files.contains(&lower)
                        || lower == BINDS_FILE
                        || !is_readable_cfg_name(&name)
                    {
                        continue;
                    }
                    self.files.insert(lower);
                    if !self.parsed.contains_key(&name) {
                        let commands =
                            (self.read_file)(&name).map(|text| Rc::new(parse_cfg(&text)));
                        self.parsed.insert(name.clone(), commands);
                    }
                    if let Some(Some(commands)) = self.parsed.get(&name).cloned() {
                        // `called` — свой у файла: alias, вызванный из двух файлов, назовёт оба.
                        let mut inner = Ctx {
                            file: Some(name),
                            ..ctx.clone()
                        };
                        self.walk(&commands, &mut inner, depth + 1);
                    }
                } else if verb == "alias" && args.len() >= 2 {
                    self.aliases
                        .insert(args[1].to_lowercase(), args[2..].join(" "));
                } else if let Some(body) = self.aliases.get(&verb).cloned()
                    && !ctx.called.contains(&verb)
                {
                    ctx.called.insert(verb);
                    self.walk(&parse_cfg(&body), ctx, depth + 1);
                }
            }
        }
    }

    let mut known: HashMap<String, String> = HashMap::new();
    for (name, value) in aliases.iter() {
        known.insert(name.to_lowercase(), value.clone());
    }
    let mut state = Walk {
        binds,
        read_file: &mut read_file,
        startup: startup_files
            .iter()
            .map(|name| name.to_lowercase())
            .collect(),
        parsed: HashMap::new(),
        budget: RUNTIME_BUDGET,
        runtime: RuntimeBinds::new(),
        files: HashSet::new(),
        aliases: HashMap::new(),
    };
    for (trigger, bound) in binds.iter() {
        state.files = HashSet::new();
        state.aliases = known.clone();
        let mut ctx = Ctx {
            trigger,
            file: None,
            called: HashSet::new(),
        };
        state.walk(&parse_cfg(&bound.command), &mut ctx, 0);
    }
    state.runtime
}

/// Одна запись переназначений. `command` — `None`, если в JSON не строка;
/// `base` — `None`, если его нет, `null` или не строка. `true`, если запись принята.
fn accept_override(
    result: &mut Overrides,
    raw_key: &str,
    command: Option<&str>,
    base: Option<&str>,
) -> bool {
    let Some(key) = normalize_key(raw_key) else {
        return false;
    };
    let Some(command) = command else {
        return false;
    };
    let command = clean_command(command);
    if command_error(&command).is_some() {
        return false;
    }
    let mut base = base.map(clean_command).unwrap_or_default();
    if command_error(&base).is_some() {
        base.clear();
    }
    result.insert(key, Override { command, base });
    true
}

/// Очистка списка переназначений: только годные клавиши и команды, не больше
/// [`MAX_OVERRIDES`] записей.
fn clean_overrides<'a>(entries: impl Iterator<Item = (&'a str, &'a Override)>) -> Overrides {
    let mut result = Overrides::new();
    let mut count = 0;
    for (raw_key, entry) in entries {
        if count >= MAX_OVERRIDES {
            break;
        }
        if accept_override(
            &mut result,
            raw_key,
            Some(&entry.command),
            Some(&entry.base),
        ) {
            count += 1;
        }
    }
    result
}

/// `sanitizeOverrides` для значения из JSON (настройки игры, ответ окна).
/// Всё, что не объект, даёт пустой набор.
pub fn sanitize_overrides(input: &Value) -> Overrides {
    let mut result = Overrides::new();
    let Value::Object(map) = input else {
        return result;
    };
    // Объект JS перебирает ключи-индексы первыми: от порядка зависит, какие записи
    // попадут под предел.
    let object: JsObject<&Value> = map
        .iter()
        .map(|(key, entry)| (key.as_str(), entry))
        .collect();
    let mut count = 0;
    for (raw_key, entry) in object.iter() {
        if count >= MAX_OVERRIDES {
            break;
        }
        let Value::Object(entry) = entry else {
            continue;
        };
        let command = entry.get("command").and_then(Value::as_str);
        let base = entry.get("base").and_then(Value::as_str);
        if accept_override(&mut result, raw_key, command, base) {
            count += 1;
        }
    }
    result
}

/// Возврат (`command = base`) больше не нужен, когда игра уже хранит `base` сама.
pub fn prune_overrides(overrides: &Overrides, file_binds: &JsObject<Bind>) -> Overrides {
    let mut result = Overrides::new();
    for (key, entry) in clean_overrides(overrides.iter()).iter() {
        let current = file_binds.get(key).map_or("", |bind| bind.command.as_str());
        if entry.command == entry.base && current == entry.base {
            continue;
        }
        result.insert(key, entry.clone());
    }
    result
}

/// «Сбросить» в окне настроек: все переназначения становятся возвратами.
pub fn reset_overrides(overrides: &Overrides) -> Overrides {
    clean_overrides(overrides.iter())
        .iter()
        .map(|(key, entry)| {
            (
                key.to_owned(),
                Override {
                    command: entry.base.clone(),
                    base: entry.base.clone(),
                },
            )
        })
        .collect()
}

/// Source не принимает «;» в имени клавиши даже в кавычках.
fn quote_key(key: &str, engine: &str) -> String {
    let name = if key == ";" && engine != "goldsrc" {
        "SEMICOLON"
    } else {
        key
    };
    format!("\"{name}\"")
}

/// Текст hlsr-binds.cfg. `aliases` — пары «имя, значение» скриптов из заготовок,
/// которые нужны назначенным командам: пишутся до биндов, потому что alias должен
/// существовать к нажатию клавиши. Пустое значение — пустой alias. `engine` —
/// `None` или пустая строка значит GoldSrc.
pub fn to_cfg_lines(
    overrides: &Overrides,
    engine: Option<&str>,
    aliases: &[(String, String)],
) -> String {
    let engine = engine.filter(|name| !name.is_empty()).unwrap_or("goldsrc");
    let clean = clean_overrides(overrides.iter());
    let mut lines = vec![
        "// Generated by HLSR Launcher. Do not edit: overwritten before every launch.".to_owned(),
        format!(
            "// Executed from {}. Change binds in the launcher: game settings, Keyboard.",
            engine_of(engine).exec_file
        ),
    ];
    for (name, value) in aliases {
        lines.push(if value.is_empty() {
            format!("alias {name}")
        } else {
            format!("alias {name} \"{value}\"")
        });
    }
    let mut keys: Vec<&str> = clean.keys().collect();
    keys.sort_by(|a, b| cmp_utf16(a, b));
    for key in keys {
        let command = &clean.get(key).expect("ключ из набора").command;
        let quoted = quote_key(key, engine);
        lines.push(if command.is_empty() {
            format!("unbind {quoted}")
        } else {
            format!("bind {quoted} \"{command}\"")
        });
    }
    lines.push(String::new());
    lines.join("\r\n")
}

/// Что знает импорт о состоянии игры.
#[derive(Default)]
pub struct ImportOptions<'a> {
    /// Бинды из cfg игрока.
    pub file_binds: Option<&'a JsObject<Bind>>,
    /// Текущие переназначения лаунчера.
    pub overrides: Option<&'a Overrides>,
    /// Команда из заготовок лаунчера: их alias он пишет сам.
    pub known_command: Option<&'a dyn Fn(&str) -> bool>,
    /// Alias, которые уже есть в cfg игры.
    pub aliases: Option<&'a JsObject<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportResult {
    pub overrides: Overrides,
    pub changed: usize,
    pub unbound: usize,
    pub unchanged: usize,
    pub skipped_alias: usize,
    pub skipped_invalid: usize,
    pub skipped_key: usize,
    pub found: usize,
}

/// Импорт cfg на вкладке «Клавиатура»: переназначения, после которых клавиши
/// станут такими же, как в файле. Файл разбирается сам по себе, его `exec` не
/// выполняются. `unbindall` в файле значит, что клавиш вне файла быть не должно.
/// Бинд на alias, объявленный только в этом файле, пропускается: в игре такого
/// alias нет.
pub fn import_overrides(text: &str, options: &ImportOptions) -> ImportResult {
    let no_binds = JsObject::new();
    let file_binds = options.file_binds.unwrap_or(&no_binds);
    let current = options.overrides.map_or_else(Overrides::new, |overrides| {
        clean_overrides(overrides.iter())
    });
    let game_aliases: HashSet<String> = options
        .aliases
        .map(|aliases| aliases.keys().map(str::to_lowercase).collect())
        .unwrap_or_default();
    let known = |command: &str| options.known_command.is_some_and(|f| f(command));

    // Map JS: порядок вставки, повторная вставка место не меняет.
    let mut imported: Vec<(String, String)> = Vec::new();
    let mut file_aliases: HashSet<String> = HashSet::new();
    let mut stats = ImportResult {
        overrides: Overrides::new(),
        changed: 0,
        unbound: 0,
        unchanged: 0,
        skipped_alias: 0,
        skipped_invalid: 0,
        skipped_key: 0,
        found: 0,
    };
    let mut clear_all = false;

    for args in parse_cfg(text) {
        let verb = args[0].to_lowercase();
        if verb == "unbindall" {
            clear_all = true;
            imported.clear();
        } else if (verb == "bind" && args.len() >= 3) || (verb == "unbind" && args.len() >= 2) {
            let Some(key) = normalize_key(&args[1]) else {
                stats.skipped_key += 1;
                continue;
            };
            let command = if verb == "bind" {
                clean_command(&args[2..].join(" "))
            } else {
                String::new()
            };
            match imported.iter_mut().find(|(known_key, _)| *known_key == key) {
                Some(slot) => slot.1 = command,
                None => imported.push((key, command)),
            }
        } else if verb == "alias" && args.len() >= 2 {
            file_aliases.insert(args[1].to_lowercase());
        }
    }

    let command_of = |key: &str| -> String {
        match (current.get(key), file_binds.get(key)) {
            (Some(entry), _) => entry.command.clone(),
            (None, Some(bind)) => bind.command.clone(),
            (None, None) => String::new(),
        }
    };
    let base_of = |key: &str| -> String {
        match (current.get(key), file_binds.get(key)) {
            (Some(entry), _) => entry.base.clone(),
            (None, Some(bind)) => bind.command.clone(),
            (None, None) => String::new(),
        }
    };
    let uses_file_alias = |command: &str| {
        command.split(';').any(|part| {
            let trimmed = js::trim(part);
            let name = trimmed
                .split(js::is_whitespace)
                .next()
                .unwrap_or_default()
                .to_lowercase();
            !name.is_empty() && file_aliases.contains(&name) && !game_aliases.contains(&name)
        })
    };

    let mut next = current.clone();
    for (key, command) in &imported {
        stats.found += 1;
        if command_error(command).is_some() {
            stats.skipped_invalid += 1;
        } else if !command.is_empty() && uses_file_alias(command) && !known(command) {
            stats.skipped_alias += 1;
        } else if *command == command_of(key) {
            stats.unchanged += 1;
        } else {
            next.insert(
                key.clone(),
                Override {
                    command: command.clone(),
                    base: base_of(key),
                },
            );
            if command.is_empty() {
                stats.unbound += 1;
            } else {
                stats.changed += 1;
            }
        }
    }
    if clear_all {
        let mut seen: HashSet<&str> = HashSet::new();
        let keys: Vec<&str> = file_binds
            .keys()
            .chain(current.keys())
            .filter(|key| seen.insert(*key))
            .collect();
        for key in keys {
            if imported.iter().any(|(k, _)| k == key) || command_of(key).is_empty() {
                continue;
            }
            next.insert(
                key,
                Override {
                    command: String::new(),
                    base: base_of(key),
                },
            );
            stats.unbound += 1;
        }
    }
    stats.overrides = clean_overrides(next.iter());
    stats
}

/// Текст cfg, прочитанный побайтно (latin1): BOM UTF-8 убирается, UTF-16
/// (Блокнот «Юникод») перекодируется. Символы вне latin1 после UTF-16 становятся
/// байтами UTF-8, как если бы файл был сохранён в UTF-8.
pub fn cfg_text(raw: &str) -> String {
    let units: Vec<u16> = raw.encode_utf16().collect();
    if units.starts_with(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf16_lossy(&units[3..]);
    }
    let mut body: &[u16] = &units;
    let little = if units.starts_with(&[0xff, 0xfe]) {
        body = &units[2..];
        Some(true)
    } else if units.starts_with(&[0xfe, 0xff]) {
        body = &units[2..];
        Some(false)
    } else if units.len() >= 4 {
        // Без BOM: в ASCII-тексте UTF-16 каждый второй байт нулевой.
        let n = units.len().min(4096) & !1;
        let (mut even, mut odd) = (0usize, 0usize);
        for pair in units[..n].as_chunks::<2>().0 {
            if pair[0] == 0 {
                even += 1;
            }
            if pair[1] == 0 {
                odd += 1;
            }
        }
        let threshold = n as f64 / 2.0 * 0.6;
        if odd as f64 > threshold && even == 0 {
            Some(true)
        } else if even as f64 > threshold && odd == 0 {
            Some(false)
        } else {
            None
        }
    } else {
        None
    };
    let Some(little) = little else {
        return raw.to_owned();
    };
    let decoded: Vec<u16> = body
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let (lo, hi) = if little {
                (pair[0], pair[1])
            } else {
                (pair[1], pair[0])
            };
            (u32::from(lo) | (u32::from(hi) << 8)) as u16
        })
        .collect();
    if decoded.iter().all(|&unit| unit <= 0xff) {
        return decoded.iter().map(|&unit| char::from(unit as u8)).collect();
    }
    String::from_utf16_lossy(&decoded)
        .bytes()
        .map(char::from)
        .collect()
}

/// Команды строки верхнего уровня с их исходным текстом: «;» и «//» внутри
/// кавычек не делят строку. Возвращает тексты команд и комментарий (с «//»).
pub fn split_line(line: &str) -> (Vec<String>, String) {
    let bytes = line.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut end = line.len();
    let mut comment = String::new();
    let mut quoted = false;
    for (i, &byte) in bytes.iter().enumerate() {
        if byte == b'"' {
            quoted = !quoted;
        } else if !quoted && byte == b';' {
            parts.push(line[start..i].to_owned());
            start = i + 1;
        } else if !quoted && byte == b'/' && bytes.get(i + 1) == Some(&b'/') {
            comment = line[i..].to_owned();
            end = i;
            break;
        }
    }
    parts.push(line[start..end].to_owned());
    (parts, comment)
}

/// Что сделать с командой верхнего уровня строки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rewrite {
    /// Оставить как есть.
    Keep,
    /// Заменить текстом.
    Replace(String),
    /// Убрать; команда уходит в комментарий «// HLSR: причина: команда» в конце
    /// строки (причина может быть пустой).
    Drop(String),
}

/// Переписывает команды верхнего уровня строки. `rewrite(args, текст)` решает
/// судьбу каждой команды; `map_kept(текст)` применяется к оставшимся.
pub fn rewrite_top_level(
    line: &str,
    mut rewrite: impl FnMut(&[String], &str) -> Rewrite,
    map_kept: impl Fn(&str) -> String,
) -> String {
    let (parts, comment) = split_line(line);
    let mut changed = false;
    let mut kept: Vec<String> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();
    for part in &parts {
        let decision = match parse_cfg(part).first() {
            Some(args) => rewrite(args, part),
            None => Rewrite::Keep,
        };
        let text = match decision {
            Rewrite::Drop(reason) => {
                changed = true;
                let prefix = if reason.is_empty() {
                    String::new()
                } else {
                    format!("{reason}: ")
                };
                dropped.push(format!("{prefix}{}", js::trim(part)));
                continue;
            }
            Rewrite::Keep => map_kept(part),
            Rewrite::Replace(text) => map_kept(&text),
        };
        if text != *part {
            changed = true;
        }
        kept.push(text);
    }
    if !changed {
        return line.to_owned();
    }
    let body = kept
        .iter()
        .filter(|part| !js::trim(part).is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(";");
    let note = if dropped.is_empty() {
        String::new()
    } else {
        format!("// HLSR: {}", dropped.join("; "))
    };
    [body, note, comment]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn js_regex(pattern: &str, flags: &str) -> Regex {
    Regex::with_flags(pattern, flags).expect("выражение 0.4.7 разбирается")
}

/// Имя из `exec` без обрамления: `^"?(?:\.\/)?…"?$`.
fn exec_name(name: &str) -> &str {
    let name = name.strip_prefix('"').unwrap_or(name);
    let name = name.strip_prefix("./").unwrap_or(name);
    name.strip_suffix('"').unwrap_or(name)
}

/// Имя без расширения `.ext`; регистр сводится только у ASCII-букв.
fn strip_extension<'a>(name: &'a str, ext: &str) -> Option<&'a str> {
    let cut = name.len().checked_sub(ext.len() + 1)?;
    let tail = name.get(cut..)?.strip_prefix('.')?;
    tail.eq_ignore_ascii_case(ext).then(|| &name[..cut])
}

/// `hlsr-`, дальше буквы и «-», но не `import…`: файлы лаунчера, кроме импорта.
fn is_launcher_file(stem: &str) -> bool {
    if !stem
        .get(..5)
        .is_some_and(|head| head.eq_ignore_ascii_case("hlsr-"))
    {
        return false;
    }
    let rest = &stem[5..];
    !rest.is_empty()
        && rest.bytes().all(|b| b.is_ascii_alphabetic() || b == b'-')
        && !rest
            .get(..6)
            .is_some_and(|head| head.eq_ignore_ascii_case("import"))
}

/// exec файлов, которые и так выполняет игра или лаунчер: из импорта они
/// зациклили бы выполнение (config.cfg снова выполнит userconfig.cfg лаунчера, а
/// тот — импорт) или выполнили бы файлы лаунчера раньше времени. В 0.4.7 это
/// `/^"?(?:\.\/)?(config|valve|game|language|joystick|hlsr-(?!import)[a-z-]+)\.(cfg|rc)"?$/i`.
/// Флаг `i` без `u` сводит регистр только у ASCII-букв (`ı`, `ſ` и знак Кельвина
/// с ними не совпадают), поэтому выражение записано вручную.
fn is_engine_or_launcher_exec(name: &str) -> bool {
    let name = exec_name(name);
    let Some(stem) = strip_extension(name, "cfg").or_else(|| strip_extension(name, "rc")) else {
        return false;
    };
    ["config", "valve", "game", "language", "joystick"]
        .iter()
        .any(|known| stem.eq_ignore_ascii_case(known))
        || is_launcher_file(stem)
}

/// userconfig.cfg и autoexec.cfg цепочка импорта просит как файлы игрока
/// (`keepPlayerFiles`) и пропускает здесь:
/// `/^"?(?:\.\/)?(userconfig|autoexec)\.cfg"?$/i`.
fn is_player_file_exec(name: &str) -> bool {
    strip_extension(exec_name(name), "cfg").is_some_and(|stem| {
        stem.eq_ignore_ascii_case("userconfig") || stem.eq_ignore_ascii_case("autoexec")
    })
}

static IMPORT_NAME: LazyLock<Regex> =
    LazyLock::new(|| js_regex(r"^hlsr-import(-[a-z0-9_-]+)?\.cfg$", ""));

/// Текст файла для записи: exec файлов игры и лаунчера на верхнем уровне убраны в
/// комментарий (остальные команды строки остаются), NUL и прочие управляющие
/// символы (кроме табуляции и переводов строк) убраны.
pub fn import_file_text(text: &str, keep_player_files: bool) -> String {
    let skipped = |name: &str| {
        is_engine_or_launcher_exec(name) || (!keep_player_files && is_player_file_exec(name))
    };
    let stripped: String = cfg_text(text)
        .chars()
        .filter(
            |&c| !matches!(c, '\0'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{7f}'),
        )
        .collect();
    stripped
        .split('\n')
        .map(|line| {
            let (cr, body) = match line.strip_suffix('\r') {
                Some(body) => ("\r", body),
                None => ("", line),
            };
            let rewritten = rewrite_top_level(
                body,
                |args, _| {
                    if args[0].to_lowercase() == "exec" && args.len() >= 2 && skipped(&args[1]) {
                        Rewrite::Drop(String::new())
                    } else {
                        Rewrite::Keep
                    }
                },
                str::to_owned,
            );
            rewritten + cr
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Бинд из подключённого импорта: из его файлов (`imported` — пути в нижнем
/// регистре) или из hlsr-import*.cfg прошлых версий лаунчера.
pub fn is_import_source(source: &str, imported: &[String]) -> bool {
    let lower = source.replace('\\', "/").to_lowercase();
    if lower.is_empty() {
        return false;
    }
    IMPORT_NAME.find(&lower).is_some() || imported.contains(&lower)
}

/// Что увидит игра после импорта: бинды, alias и cvar из `read_binds` плюс
/// команды файла поверх них (файл выполняется последним из cfg игрока).
pub fn with_import(read: Option<&BindRead>, text: &str) -> BindRead {
    let mut result = BindRead::default();
    let imported = read.map(|r| r.imported.as_slice()).unwrap_or_default();
    if let Some(read) = read {
        result.binds = read.binds.clone();
        result.aliases = read.aliases.clone();
        result.cvars = read.cvars.clone();
    }
    // Бинды прошлого импорта уходят: новый импорт его заменяет.
    let from_import: Vec<String> = result
        .binds
        .iter()
        .filter(|(_, bind)| is_import_source(&bind.source, imported))
        .map(|(key, _)| key.to_owned())
        .collect();
    for key in from_import {
        result.binds.remove(&key);
    }
    for args in parse_cfg(&import_file_text(text, false)) {
        let verb = args[0].to_lowercase();
        if verb == "unbindall" {
            result.binds.clear();
        } else if verb == "bind" && args.len() >= 3 {
            if let Some(key) = normalize_key(&args[1]) {
                let command = clean_command(&args[2..].join(" "));
                result.binds.insert(
                    key,
                    Bind {
                        command,
                        source: IMPORT_FILE.to_owned(),
                    },
                );
            }
        } else if verb == "unbind" && args.len() >= 2 {
            if let Some(key) = normalize_key(&args[1]) {
                result.binds.remove(&key);
            }
        } else if verb == "alias" && args.len() >= 2 {
            result.aliases.insert(args[1].clone(), args[2..].join(" "));
        } else if READ_CVARS.contains(&verb.as_str()) && args.len() >= 2 {
            result.cvars.insert(verb, args[1].clone());
        }
    }
    if let Some(read) = read {
        result.files = read
            .files
            .iter()
            .filter(|name| !is_import_source(name, imported))
            .cloned()
            .collect();
    }
    result.files.push(IMPORT_FILE.to_owned());
    // Файлы импорта, которые зовут клавиши, ещё не на диске: что они назначат — видно после
    // «Сохранить». С диска остаются только бинды по нажатию клавиш, которые импорт не тронул.
    if let Some(read) = read {
        for (key, list) in read.runtime.iter() {
            let kept: Vec<RuntimeBind> = list
                .iter()
                .filter(|item| {
                    read.binds.get(&item.trigger).is_some_and(|before| {
                        result
                            .binds
                            .get(&item.trigger)
                            .is_some_and(|now| now.command == before.command)
                    })
                })
                .cloned()
                .collect();
            if !kept.is_empty() {
                result.runtime.insert(key, kept);
            }
        }
    }
    result
}

/// Клавиши, которые импорт назначает или снимает сам (`bind` и `unbind` верхнего
/// уровня), в порядке первого появления. `unbindall` сюда не входит: он в начале
/// каждого config.cfg и не значит, что игрок отказывается от переназначений
/// лаунчера.
pub fn import_touched_keys(text: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for args in parse_cfg(text) {
        let verb = args[0].to_lowercase();
        if ((verb == "bind" && args.len() >= 3) || (verb == "unbind" && args.len() >= 2))
            && let Some(key) = normalize_key(&args[1])
            && !keys.contains(&key)
        {
            keys.push(key);
        }
    }
    keys
}

/// Отдельные команды бинда, в том числе перебинды внутри него:
/// «save x; bind mouse4 +tau; say TAO» → «save x», «bind mouse4 +tau», «say TAO»,
/// «+tau». По ним вкладка «Скрипты» находит скрипты в биндах импортированного
/// конфига.
pub fn commands_of(command: &str) -> Vec<String> {
    let mut result = Vec::new();
    for args in parse_cfg(command) {
        result.push(args.join(" "));
        if args[0].to_lowercase() == "bind" && args.len() >= 3 {
            result.extend(commands_of(&args[2..].join(" ")));
        }
    }
    result
}

/// Alias по имени без учёта регистра: так их ищет движок.
pub type AliasMap = HashMap<String, String>;

pub fn alias_map(aliases: &JsObject<String>) -> AliasMap {
    aliases
        .iter()
        .map(|(name, value)| (name.to_lowercase(), value.clone()))
        .collect()
}

/// Alias, которые переопределяются внутри других alias («alias sst sst2»): это
/// шаги и переключатели скриптов, их значение в cfg — только начальное.
pub fn reassigned_aliases(map: &AliasMap) -> HashSet<String> {
    let mut names = HashSet::new();
    for value in map.values() {
        for args in parse_cfg(value) {
            if args[0].to_lowercase() == "alias" && args.len() >= 2 {
                names.insert(args[1].to_lowercase());
            }
        }
    }
    names
}

const EXPAND_DEPTH: usize = 16;
const EXPAND_LIMIT: usize = 400;

/// Всё, что может выполнить бинд: [`commands_of`], а ещё тела alias, которые он
/// вызывает или назначает («alias re4 re42»), и парный «-x» для «+x». Так вкладка
/// «Скрипты» находит скрипт, назначенный цепочкой alias: «re4» → «re41» →
/// «alias re4 re42» → «bind f obbo1000».
pub fn expand_command(command: &str, aliases: &AliasMap) -> Vec<String> {
    fn walk(
        text: &str,
        depth: usize,
        aliases: &AliasMap,
        visited: &mut HashSet<String>,
        result: &mut Vec<String>,
    ) {
        if depth > EXPAND_DEPTH {
            return;
        }
        for args in parse_cfg(text) {
            if result.len() >= EXPAND_LIMIT {
                return;
            }
            result.push(args.join(" "));
            let verb = args[0].to_lowercase();
            if (verb == "bind" || verb == "alias") && args.len() >= 3 {
                walk(&args[2..].join(" "), depth + 1, aliases, visited, result);
                continue;
            }
            let names = match verb.strip_prefix('+') {
                Some(rest) => vec![verb.clone(), format!("-{rest}")],
                None => vec![verb],
            };
            for name in names {
                let Some(body) = aliases.get(&name) else {
                    continue;
                };
                if !visited.insert(name) {
                    continue;
                }
                walk(body, depth + 1, aliases, visited, result);
            }
        }
    }

    let mut result = Vec::new();
    walk(command, 0, aliases, &mut HashSet::new(), &mut result);
    result
}
