//! Импорт конфига с цепочкой `exec` (`shared/cfg-import.js`, вкладка «Клавиатура»).
//!
//! Конфиг игрока часто вызывает другие cfg: сразу (exec на верхнем уровне) или по клавише
//! (exec внутри alias и bind). Лаунчер просит выбрать каждый такой файл и подключает их
//! вместе с конфигом. В HL1 файлы ложатся в папку мода под теми же именами и по тем же
//! путям, что в exec: так конфиг работает как у игрока, без переименований. Файлы, которые
//! импорт заменил, лаунчер сохраняет, и «Убрать конфиг» возвращает их (см.
//! `game_binds_install`).
//!
//! Проверки: файл, который выполняется сам из себя (цикл exec), игра крутила бы бесконечно —
//! такой exec закомментирован. Один и тот же файл, выбранный под двумя именами, попадает в
//! отчёт. Бинды, alias и переменные, которые цепочка задаёт несколько раз по-разному, тоже.
//!
//! Тексты — строки как их видит JS: файл HL1 читается побайтно (latin1). Размеры, которые
//! 0.4.7 считает по `String.length`, считаются в единицах UTF-16. Регулярные выражения
//! 0.4.7 с флагом `i` без `u` сводят регистр только у ASCII-букв (`ſ` и знак Кельвина с
//! ними не совпадают), поэтому такие проверки записаны вручную; остальные — через `regress`.

use crate::game_binds::{self, Rewrite};
use crate::game_config;
use hlsr_core::js;
use hlsr_core::js::object::JsObject;
use regress::Regex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::LazyLock;

/// Сколько файлов цепочки принимает импорт.
pub const MAX_FILES: usize = 32;
/// Сколько команд в плоском тексте раскрытия.
const MAX_FLAT_COMMANDS: usize = 20000;
/// Бюджет текста `flatten_import` (единицы UTF-16) на весь проход — корень и все файлы скриптов:
/// ромб exec (файл зовёт следующий дважды) растёт как 2^n, у 0.4.9 предела нет. Всё больше `MAX_IMPORT_BYTES` импорт и так отклоняет
/// (BINDS_FILE_TOO_LARGE); запас ×16 — чтобы фикстура «раскрытие до предела команд»
/// (2,9 млн) совпадала с 0.4.9 байт в байт.
const FLATTEN_BUDGET: usize = 16 * game_binds::MAX_IMPORT_BYTES;

/// Куда ведёт `exec`: ключ (без регистра, прямые слэши) и имя для показа; `system` —
/// файл, который выполняет сама игра или лаунчер; `auto` — файл игрока, который
/// выполняет движок (userconfig.cfg, autoexec.cfg).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecTarget {
    pub key: String,
    pub name: String,
    pub system: bool,
    pub auto: bool,
}

/// `exec` из текста: `runtime` — внутри alias или bind (выполнится по нажатию клавиши).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exec {
    pub runtime: bool,
    pub target: ExecTarget,
}

/// Файл цепочки, выбранный игроком.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportFile {
    /// Имя, как его показал игрок; пустое — не задано (берётся ключ).
    pub name: String,
    pub text: String,
    /// Имя в папке мода, если игрок выбрал несколько файлов сразу ([`root_of_picked`]).
    pub target: Option<String>,
}

/// Выбранные файлы по ключам (имя в нижнем регистре); порядок — как у `Object.entries`.
pub type ImportFiles = JsObject<ImportFile>;

/// Корень импорта: выбранный конфиг или, при нескольких файлах, «виртуальный» корень,
/// у которого на диске файла нет, а `exec` его файлов идут в userconfig.cfg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub name: String,
    pub text: String,
    pub virtual_root: bool,
}

/// Следующий файл цепочки, который нужно выбрать.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub key: String,
    pub name: String,
    /// Файл, из которого вызван этот.
    pub from: String,
}

/// Файл для папки игры.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub file: String,
    pub text: String,
    /// Это корень импорта.
    pub root: bool,
}

/// Бинд, alias или переменная, которые цепочка задаёт несколько раз по-разному.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Duplicate {
    /// `bind`, `alias` или `cvar`.
    pub kind: String,
    pub name: String,
    pub files: Vec<String>,
    /// Действует последнее значение.
    pub value: String,
}

/// Отчёт о проверках импорта.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Файлы цепочки, кроме корня.
    pub files: Vec<String>,
    pub missing: Vec<String>,
    /// Цикл exec: (файл, файл, на который он ссылается).
    pub cycles: Vec<(String, String)>,
    /// Один файл под двумя именами: (первое имя, второе имя).
    pub duplicate_files: Vec<(String, String)>,
    pub duplicates: Vec<Duplicate>,
}

/// Собранный импорт.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    pub outputs: Vec<Output>,
    /// Что подключить из userconfig.cfg.
    pub execs: Vec<String>,
    /// Команды в порядке выполнения (для предпросмотра и импорта биндов).
    pub flat_text: String,
    /// Размер файлов, единиц UTF-16.
    pub bytes: usize,
    pub report: Report,
}

/// Файл, выбранный игроком вместе с другими.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    pub name: String,
    pub text: String,
}

/// Корень и файлы нескольких выбранных конфигов.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootOfPicked {
    pub root: Root,
    pub files: ImportFiles,
}

/// Файл игрока, который движок выполнил бы сам рядом с выбранными.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Companion {
    pub name: &'static str,
    pub from: &'static str,
    pub before: bool,
}

// ------------------------------------------------------------- имена файлов

/// Совпадение с хвостом без учёта регистра ASCII-букв.
fn ends_with_ascii_ci(text: &str, suffix: &str) -> bool {
    let (text, suffix) = (text.as_bytes(), suffix.as_bytes());
    text.len() >= suffix.len() && text[text.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

/// Имя без `.cfg` или `.rc` на конце (регистр ASCII не важен): `/\.(cfg|rc)$/i`.
fn strip_cfg_rc_ci(name: &str) -> Option<&str> {
    for suffix in [".cfg", ".rc"] {
        if ends_with_ascii_ci(name, suffix) {
            return Some(&name[..name.len() - suffix.len()]);
        }
    }
    None
}

/// `ENGINE_RUN = /^(config|userconfig|autoexec)\.cfg$/i`: cfg, которые движок выполняет
/// сам, — импортированный под этим именем файл строкой exec не подключается.
pub fn is_engine_run(name: &str) -> bool {
    name.get(..name.len().saturating_sub(4))
        .filter(|_| ends_with_ascii_ci(name, ".cfg"))
        .is_some_and(|stem| {
            ["config", "userconfig", "autoexec"]
                .iter()
                .any(|known| stem.eq_ignore_ascii_case(known))
        })
}

/// `SYSTEM_NAMES`: файлы, которые выполняет сама игра или лаунчер (имя уже в нижнем
/// регистре): `^(config|valve|game|language|joystick|skill|listenserver|server)\.(cfg|rc)$`
/// или `^hlsr-(?!import)`.
fn is_system_name(base: &str) -> bool {
    if let Some(rest) = base.strip_prefix("hlsr-")
        && !rest.starts_with("import")
    {
        return true;
    }
    let stem = base
        .strip_suffix(".cfg")
        .or_else(|| base.strip_suffix(".rc"));
    matches!(
        stem,
        Some(
            "config"
                | "valve"
                | "game"
                | "language"
                | "joystick"
                | "skill"
                | "listenserver"
                | "server"
        )
    )
}

/// `PLAYER_AUTO = /^(userconfig|autoexec)\.cfg$/`: файлы игрока, которые движок
/// выполняет сам (имя уже в нижнем регистре).
fn is_player_auto(base: &str) -> bool {
    base == "userconfig.cfg" || base == "autoexec.cfg"
}

/// Имя файла из аргумента exec, как его найдёт движок. `None` — движок такой exec не
/// выполнит (пусто, `..`, `:`, у GoldSrc нет `.cfg` и `.rc`).
pub fn exec_target(arg: &str, engine: &str) -> Option<ExecTarget> {
    let mut name = js::trim(arg)
        .replace('\\', "/")
        .trim_start_matches('/')
        .to_owned();
    if name.is_empty() || name.contains("..") || name.contains(':') {
        return None;
    }
    if strip_cfg_rc_ci(&name).is_none() {
        // GoldSrc выполняет только .cfg и .rc, Source сам дописывает .cfg.
        if engine == "goldsrc" {
            return None;
        }
        name.push_str(".cfg");
    }
    let key = name.to_lowercase();
    let base = key.rsplit('/').next().unwrap_or_default();
    let (system, auto) = (is_system_name(base), is_player_auto(base));
    Some(ExecTarget {
        key,
        name,
        system,
        auto,
    })
}

/// exec в командах: верхний уровень и внутри значений alias и bind.
pub fn execs_of(text: &str, engine: &str) -> Vec<Exec> {
    fn visit(args: &[String], runtime: bool, depth: usize, engine: &str, out: &mut Vec<Exec>) {
        if args.is_empty() || depth > 4 {
            return;
        }
        let verb = args[0].to_lowercase();
        if verb == "exec" && args.len() >= 2 {
            if let Some(target) = exec_target(&args[1], engine) {
                out.push(Exec { runtime, target });
            }
        } else if (verb == "alias" || verb == "bind") && args.len() >= 3 {
            for inner in game_binds::parse_cfg(&args[2..].join(" ")) {
                visit(&inner, true, depth + 1, engine, out);
            }
        }
    }
    let mut result = Vec::new();
    for args in game_binds::parse_cfg(text) {
        visit(&args, false, 0, engine, &mut result);
    }
    result
}

fn root_key(root: &Root, engine: &str) -> String {
    exec_target(&root.name, engine)
        .map(|target| target.key)
        .unwrap_or_default()
}

/// Следующий файл цепочки, который нужно выбрать. `files` — уже выбранные, `missing` —
/// ключи, от которых игрок отказался, `top_level_only` — только exec, которые выполняются
/// сразу, без alias и bind.
pub fn next_wanted(
    engine: &str,
    root: &Root,
    files: &ImportFiles,
    missing: &[String],
    top_level_only: bool,
) -> Option<Wanted> {
    let mut skip: HashSet<String> = missing.iter().map(|key| key.to_lowercase()).collect();
    skip.insert(root_key(root, engine));
    let mut queue: VecDeque<(String, String)> = VecDeque::new();
    queue.push_back((root.name.clone(), root.text.clone()));
    let mut seen: HashSet<String> = HashSet::new();
    while let Some((name, text)) = queue.pop_front() {
        for exec in execs_of(&text, engine) {
            let target = exec.target;
            if seen.contains(&target.key) {
                continue;
            }
            // Выбранный игроком файл (несколько файлов сразу, root_of_picked)
            // выполняется, даже если зовётся как файл игры (config.cfg).
            if let Some(file) = files.get(&target.key) {
                seen.insert(target.key);
                queue.push_back((file.name.clone(), file.text.clone()));
                continue;
            }
            if target.system || (top_level_only && exec.runtime) {
                continue;
            }
            seen.insert(target.key.clone());
            if !skip.contains(&target.key) {
                return Some(Wanted {
                    key: target.key,
                    name: target.name,
                    from: name,
                });
            }
        }
    }
    None
}

/// `SAFE_NAME`: имя в папке мода — до двух подпапок, без «..», не файл лаунчера и не
/// файл движка. `/^[a-z0-9_][a-z0-9_. -]*(\/[a-z0-9_][a-z0-9_. -]*){0,2}\.(cfg|rc)$/i`.
fn matches_safe_name(value: &str) -> bool {
    let Some(stem) = strip_cfg_rc_ci(value) else {
        return false;
    };
    let mut parts = 0;
    for part in stem.split('/') {
        parts += 1;
        let mut chars = part.chars();
        let first_ok = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        if parts > 3
            || !first_ok
            || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ' ' | '-'))
        {
            return false;
        }
    }
    true
}

/// `RESERVED_NAME`: `hlsr-…` или файл игры по имени, без учёта регистра ASCII:
/// `/^(hlsr-.*|(valve|game|language|joystick|skill|listenserver|server)\.(cfg|rc))$/i`.
fn is_reserved_name(base: &str) -> bool {
    let line_break = |c: char| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
    if base
        .get(..5)
        .is_some_and(|head| head.eq_ignore_ascii_case("hlsr-"))
        && !base[5..].contains(line_break)
    {
        return true;
    }
    strip_cfg_rc_ci(base).is_some_and(|stem| {
        [
            "valve",
            "game",
            "language",
            "joystick",
            "skill",
            "listenserver",
            "server",
        ]
        .iter()
        .any(|known| stem.eq_ignore_ascii_case(known))
    })
}

/// Имя файла импорта в папке мода безопасно: как в exec, если оно безопасно (до двух
/// подпапок, без «..»), и это не файл лаунчера или движка.
pub fn is_safe_import_name(name: &str) -> bool {
    matches_safe_name(name)
        && !name.split('/').any(|part| {
            let part = js::trim(part);
            !part.is_empty() && part.chars().all(|c| c == '.')
        })
        && !is_reserved_name(name.rsplit('/').next().unwrap_or_default())
}

/// Имя в папке мода: как у игрока, а небезопасное — очищенное. Повтор имени (два
/// config.cfg из разных папок) получает номер.
fn output_name(name: &str, used: &mut HashSet<String>) -> String {
    let mut candidate = js::trim(name)
        .replace('\\', "/")
        .trim_start_matches('/')
        .to_owned();
    if !is_safe_import_name(&candidate) {
        // Импорт прошлых версий (hlsr-import-jump.cfg) возвращает себе имя игрока.
        let lower = candidate
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_lowercase();
        let lower = lower
            .strip_suffix(".cfg")
            .or_else(|| lower.strip_suffix(".rc"))
            .unwrap_or(&lower);
        let lower = match lower.strip_prefix("hlsr-import") {
            Some("") => "",
            Some(rest) if rest.starts_with('-') => &rest[1..],
            _ => lower,
        };
        let mut stem = String::new();
        let mut in_run = false;
        for c in lower.chars() {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
                stem.push(c);
                in_run = false;
            } else if !in_run {
                stem.push('-');
                in_run = true;
            }
        }
        let stem = stem.trim_matches('-');
        let stem = if stem.is_empty() { "player" } else { stem };
        candidate = format!("{stem}.cfg");
        if !is_safe_import_name(&candidate) {
            candidate = format!("cfg-{stem}.cfg");
        }
    }
    let dot = candidate.rfind('.').unwrap_or(candidate.len());
    let mut result = candidate.clone();
    let mut n = 2;
    while used.contains(&result.to_lowercase()) {
        result = format!("{}-{n}{}", &candidate[..dot], &candidate[dot..]);
        n += 1;
    }
    used.insert(result.to_lowercase());
    result
}

fn quote(token: &str) -> String {
    format!("\"{token}\"")
}

// ------------------------------------------------------------------ сборка

struct Entry {
    key: String,
    name: String,
    text: String,
    /// Имя в папке мода; у виртуального корня его нет.
    output: Option<String>,
    virtual_root: bool,
}

impl Entry {
    /// `idOf(entry)`: имя файла для пометки обрезанного exec.
    fn id(&self) -> &str {
        self.output.as_deref().unwrap_or("")
    }
}

/// `exec` как имя файла: `(\bexec\s+)("?)([^\s";]+)\2`, без учёта регистра ASCII-букв.
static EXEC_ARGUMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(\b[eE][xX][eE][cC]\s+)("?)([^\s";]+)\2"#).expect("выражение 0.4.7 разбирается")
});

struct Flat {
    args: Vec<String>,
    file: String,
}

/// Собирает импорт: файлы для папки игры, строки exec для userconfig.cfg, плоский текст в
/// порядке выполнения (для предпросмотра и импорта биндов) и отчёт о проверках.
/// `root.virtual_root` — несколько выбранных файлов: самого корня на диске нет, его exec
/// идут в userconfig.cfg.
pub fn build_import(engine: &str, root: &Root, files: &ImportFiles, missing: &[String]) -> Built {
    let mut entries: Vec<Entry> = Vec::new();
    // `sources`: Map по ключу в порядке первой вставки; значение при повторе заменяется.
    let mut sources: Vec<(String, usize)> = Vec::new();
    let mut used: HashSet<String> = HashSet::new();
    let mut by_text: HashMap<String, String> = HashMap::new();
    let mut duplicate_files: Vec<(String, String)> = Vec::new();

    let root_entry = Entry {
        key: if root.virtual_root {
            String::new()
        } else {
            root_key(root, engine)
        },
        name: root.name.clone(),
        // exec выбранных файлов, даже config.cfg: их строит root_of_picked.
        text: if root.virtual_root {
            root.text.clone()
        } else {
            game_binds::import_file_text(&root.text, true)
        },
        output: if root.virtual_root {
            None
        } else {
            Some(output_name(&root.name, &mut used))
        },
        virtual_root: root.virtual_root,
    };
    if !root_entry.virtual_root {
        by_text.insert(root_entry.text.clone(), root_entry.name.clone());
    }
    entries.push(root_entry);
    const ROOT: usize = 0;

    for (key, file) in files.iter() {
        let lower = key.to_lowercase();
        let text = game_binds::import_file_text(&file.text, true);
        let name = if file.name.is_empty() {
            key.to_owned()
        } else {
            file.name.clone()
        };
        let same = by_text.get(&text).cloned();
        if let Some(same) = &same {
            duplicate_files.push((same.clone(), name.clone()));
        }
        let output = output_name(
            file.target
                .as_deref()
                .filter(|t| !t.is_empty())
                .unwrap_or(key),
            &mut used,
        );
        if same.is_none() {
            by_text.insert(text.clone(), name.clone());
        }
        entries.push(Entry {
            key: lower.clone(),
            name,
            text,
            output: Some(output),
            virtual_root: false,
        });
        let index = entries.len() - 1;
        match sources
            .iter_mut()
            .find(|(source_key, _)| *source_key == lower)
        {
            Some(slot) => slot.1 = index,
            None => sources.push((lower, index)),
        }
    }
    if !entries[ROOT].key.is_empty() && !sources.iter().any(|(key, _)| *key == entries[ROOT].key) {
        sources.push((entries[ROOT].key.clone(), ROOT));
    }
    let source_of = |key: &str| {
        sources
            .iter()
            .find(|(source_key, _)| source_key == key)
            .map(|&(_, index)| index)
    };
    let child_of = |target: Option<&ExecTarget>| {
        target
            .filter(|t| !t.system || files.contains_key(&t.key))
            .and_then(|t| source_of(&t.key))
    };

    // Порядок выполнения: exec верхнего уровня раскрывается на месте. Exec на файл, который
    // уже выполняется выше по цепочке, — цикл.
    let mut flat: Vec<Flat> = Vec::new();
    let mut cycles: Vec<(String, String)> = Vec::new();
    let mut cut: HashSet<String> = HashSet::new();
    struct Run<'a, F: Fn(Option<&ExecTarget>) -> Option<usize>> {
        entries: &'a [Entry],
        engine: &'a str,
        child_of: F,
        flat: &'a mut Vec<Flat>,
        cycles: &'a mut Vec<(String, String)>,
        cut: &'a mut HashSet<String>,
    }
    impl<F: Fn(Option<&ExecTarget>) -> Option<usize>> Run<'_, F> {
        fn run(&mut self, index: usize, stack: &mut Vec<usize>) {
            let entries = self.entries;
            let entry = &entries[index];
            for args in game_binds::parse_cfg(&entry.text) {
                if self.flat.len() >= MAX_FLAT_COMMANDS {
                    return;
                }
                if args[0].to_lowercase() == "exec" && args.len() >= 2 {
                    let target = exec_target(&args[1], self.engine);
                    let child = (self.child_of)(target.as_ref());
                    if let (Some(child), Some(target)) = (child, &target) {
                        if stack.contains(&child) {
                            let id = format!("{}\n{}", entry.id(), target.key);
                            if !self.cut.contains(&id) {
                                self.cycles
                                    .push((entry.name.clone(), entries[child].name.clone()));
                            }
                            self.cut.insert(id);
                            continue;
                        }
                        stack.push(child);
                        self.run(child, stack);
                        stack.pop();
                        continue;
                    }
                }
                self.flat.push(Flat {
                    args,
                    file: entry.name.clone(),
                });
            }
        }
    }
    Run {
        entries: &entries,
        engine,
        child_of: &child_of,
        flat: &mut flat,
        cycles: &mut cycles,
        cut: &mut cut,
    }
    .run(ROOT, &mut vec![ROOT]);

    // Тексты для папки игры: exec остаются как у игрока, меняется только имя файла, которое
    // пришлось очистить. exec цикла и невыбранного файла на верхнем уровне закомментированы.
    let skipped: Vec<String> = {
        let mut list: Vec<String> = Vec::new();
        for key in missing {
            let lower = key.to_lowercase();
            if !list.contains(&lower) {
                list.push(lower);
            }
        }
        list
    };
    let renamed = |text: &str| -> String {
        let mut out = String::new();
        let mut last = 0;
        for found in EXEC_ARGUMENT.find_iter(text) {
            let range = found.range();
            out.push_str(&text[last..range.start]);
            last = range.end;
            let group = |index: usize| found.group(index).map(|r| &text[r]).unwrap_or_default();
            let (verb, q, arg) = (group(1), group(2), group(3));
            let target = exec_target(arg, engine);
            let renamed_to = child_of(target.as_ref()).and_then(|child| {
                let output = entries[child].output.as_deref()?;
                let target = target.as_ref()?;
                (output.to_lowercase() != target.key).then_some(output)
            });
            match renamed_to {
                Some(output) => out.push_str(&format!("{verb}{q}{output}{q}")),
                None => out.push_str(&text[range]),
            }
        }
        out.push_str(&text[last..]);
        out
    };
    let rewrite = |entry: &Entry| -> String {
        entry
            .text
            .split('\n')
            .map(|line| {
                game_binds::rewrite_top_level(
                    line,
                    |args, _| {
                        if args[0].to_lowercase() != "exec" || args.len() < 2 {
                            return Rewrite::Keep;
                        }
                        let Some(target) = exec_target(&args[1], engine) else {
                            return Rewrite::Keep;
                        };
                        if target.system && !files.contains_key(&target.key) {
                            return Rewrite::Keep;
                        }
                        if cut.contains(&format!("{}\n{}", entry.id(), target.key)) {
                            return Rewrite::Drop("exec loop".to_owned());
                        }
                        // userconfig.cfg и autoexec.cfg без файла игрока выполнили бы файлы
                        // лаунчера, а те снова вызывают импорт.
                        if source_of(&target.key).is_none()
                            && (skipped.contains(&target.key) || target.auto)
                        {
                            return Rewrite::Drop("not imported".to_owned());
                        }
                        Rewrite::Keep
                    },
                    renamed,
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let mut outputs: Vec<Output> = Vec::new();
    if !entries[ROOT].virtual_root {
        outputs.push(Output {
            file: entries[ROOT].output.clone().unwrap_or_default(),
            text: rewrite(&entries[ROOT]),
            root: true,
        });
    }
    // `new Set(sources.values())`: каждый файл один раз, в порядке ключей.
    let mut shown: Vec<usize> = Vec::new();
    for &(_, index) in &sources {
        if index != ROOT && !shown.contains(&index) {
            shown.push(index);
            outputs.push(Output {
                file: entries[index].output.clone().unwrap_or_default(),
                text: rewrite(&entries[index]),
                root: false,
            });
        }
    }

    // Что подключить из userconfig.cfg: корень или выбранные файлы по порядку. Файлы, которые
    // движок выполняет сам, строки не получают.
    let mut execs: Vec<String> = Vec::new();
    let exec_lines = if entries[ROOT].virtual_root {
        entries[ROOT].text.clone()
    } else {
        format!("exec \"{}\"", entries[ROOT].id())
    };
    for args in game_binds::parse_cfg(&exec_lines) {
        if args[0].to_lowercase() != "exec" || args.len() < 2 {
            continue;
        }
        let target = exec_target(&args[1], engine);
        let entry = if entries[ROOT].virtual_root {
            child_of(target.as_ref())
        } else {
            Some(ROOT)
        };
        if let Some(entry) = entry {
            let output = entries[entry].id();
            if !is_engine_run(output) && !execs.iter().any(|known| known == output) {
                execs.push(output.to_owned());
            }
        }
    }

    let bytes = outputs
        .iter()
        .map(|item| item.text.encode_utf16().count())
        .sum();
    let report_files = outputs
        .iter()
        .filter(|item| !item.root)
        .map(|item| item.file.clone())
        .collect();
    let flat_text = flat
        .iter()
        .map(|item| {
            item.args
                .iter()
                .map(|token| quote(token))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let duplicates = duplicates_of(&flat);
    Built {
        outputs,
        execs,
        flat_text,
        bytes,
        report: Report {
            files: report_files,
            missing: skipped,
            cycles,
            duplicate_files,
            duplicates,
        },
    }
}

/// Файл, который зовут alias или bind (выполнится по нажатию клавиши): остаётся файлом.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeFile {
    /// Имя из exec: путь от папки мода.
    pub file: String,
    pub text: String,
}

/// Сколько чего задаёт текст импорта. `unknown` — команды без значения (кроме unbindall и
/// exec): скрипты вида +jump, echo, жесты. `files` — сколько файлов вошло в импорт.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub binds: usize,
    pub cvars: usize,
    pub aliases: usize,
    pub unknown: usize,
    pub files: usize,
}

/// Импорт одним файлом ([`flatten_import`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flattened {
    /// Текст файла игрока: exec верхнего уровня раскрыты на месте, переводы строк — `\n`.
    pub text: String,
    pub runtime_files: Vec<RuntimeFile>,
    /// Отчёт как у [`build_import`]; `files` — выбранные файлы, которые вошли в итог.
    pub report: Report,
    /// Файлы скриптов с небезопасным именем: в папку мода они не ложатся.
    pub unsafe_names: Vec<String>,
    /// Размер текста и файлов скриптов, единиц UTF-16 (`Buffer.byteLength(…, "latin1")`).
    pub bytes: usize,
    pub summary: Summary,
}

impl Summary {
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::json!({
            "binds": self.binds, "cvars": self.cvars, "aliases": self.aliases,
            "unknown": self.unknown, "files": self.files,
        })
    }
}

impl Report {
    /// Отчёт в виде JSON, как у 0.4.7 и 0.4.9 (`cycles` и `duplicateFiles` — пары).
    pub fn to_value(&self) -> serde_json::Value {
        let pairs = |list: &[(String, String)]| -> Vec<[String; 2]> {
            list.iter().map(|(a, b)| [a.clone(), b.clone()]).collect()
        };
        let duplicates: Vec<serde_json::Value> = self
            .duplicates
            .iter()
            .map(|d| serde_json::json!({"kind": d.kind, "name": d.name, "files": d.files, "value": d.value}))
            .collect();
        serde_json::json!({
            "files": self.files,
            "missing": self.missing,
            "cycles": pairs(&self.cycles),
            "duplicateFiles": pairs(&self.duplicate_files),
            "duplicates": duplicates,
        })
    }
}

impl Flattened {
    /// `report` ответа `flattenImport` 0.4.9: отчёт цепочки и `unsafe`.
    pub fn report_value(&self) -> serde_json::Value {
        let mut report = self.report.to_value();
        report["unsafe"] = serde_json::json!(self.unsafe_names);
        report
    }

    /// Ответ `flattenImport` 0.4.9: `{ text, runtimeFiles, report, bytes, summary }`.
    pub fn to_value(&self) -> serde_json::Value {
        let report = self.report_value();
        let runtime_files: Vec<serde_json::Value> = self
            .runtime_files
            .iter()
            .map(|item| serde_json::json!({"file": item.file, "text": item.text}))
            .collect();
        serde_json::json!({
            "text": self.text,
            "runtimeFiles": runtime_files,
            "report": report,
            "bytes": self.bytes,
            "summary": self.summary.to_value(),
        })
    }
}

/// Строка, в которой только exec одного файла: его цель или `None`.
fn single_exec(line: &str, engine: &str) -> Option<ExecTarget> {
    let commands = game_config::line_commands(line);
    let [command] = commands.as_slice() else {
        return None;
    };
    let [verb, arg] = command.args.as_slice() else {
        return None;
    };
    if verb.value.to_lowercase() == "exec" {
        exec_target(&arg.value, engine)
    } else {
        None
    }
}

/// Сколько чего задаёт текст (`summaryOf`); `files` — 0, его считает [`flatten_import`].
pub fn summary_of(text: &str) -> Summary {
    let mut summary = Summary::default();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        for command in game_config::line_commands(line) {
            let args = &command.args;
            let verb = args[0].value.to_lowercase();
            if verb == "bind" && args.len() >= 3 {
                summary.binds += 1;
            } else if verb == "alias" && args.len() >= 2 {
                summary.aliases += 1;
            } else if verb == "exec" || verb == "unbindall" || verb == "unbind" {
                continue;
            } else if args.len() >= 2 {
                summary.cvars += 1;
            } else {
                summary.unknown += 1;
            }
        }
    }
    summary
}

/// Имя файла в строке-комментарии лаунчера: только ASCII и без «;» (`/[^\x20-\x7e]|;/g` по
/// единицам UTF-16).
fn ascii_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if (' '..='~').contains(&c) && c != ';' {
            out.push(c);
        } else {
            out.extend(std::iter::repeat_n('_', c.len_utf16()));
        }
    }
    out
}

/// Импорт одним файлом (`flattenImport` 0.4.9): корень, где каждый exec верхнего уровня заменён
/// текстом своего файла на том же месте, в порядке выполнения. Файлы, которые зовут только
/// alias и bind (во время игры), остаются файлами: `runtime_files` под их именами из exec.
/// Цикл exec обрывается комментарием, exec файла игрока (userconfig.cfg, autoexec.cfg) и
/// отказанных файлов закомментирован, exec лаунчера убирает `import_file_text`.
/// Исходная кодировка строк сохраняется, переводы строк — `\n`.
pub fn flatten_import(
    engine: &str,
    root: &Root,
    files: &ImportFiles,
    missing: &[String],
) -> Flattened {
    struct Expand<'a> {
        engine: &'a str,
        files: &'a ImportFiles,
        skipped: HashSet<String>,
        root_key: String,
    }
    impl Expand<'_> {
        /// `size` — длина `out` в единицах UTF-16 без переводов строк (как `bytes`).
        fn expand(
            &self,
            text: &str,
            stack: &mut Vec<String>,
            out: &mut Vec<String>,
            size: &mut usize,
        ) {
            let push = |out: &mut Vec<String>, size: &mut usize, line: String| {
                *size += line.encode_utf16().count();
                out.push(line);
            };
            for raw in text.split('\n') {
                let line = raw.strip_suffix('\r').unwrap_or(raw);
                let Some(target) = single_exec(line, self.engine) else {
                    push(out, size, line.to_owned());
                    continue;
                };
                let child = self.files.get(&target.key);
                if child.is_some() && stack.contains(&target.key) {
                    let note =
                        format!("// HLSR: exec {} skipped (cycle)", ascii_name(&target.name));
                    push(out, size, note);
                } else if let Some(child) = child {
                    // Сверх бюджета `bytes` уже больше предела импорта: дальше не раскрываем.
                    if *size > FLATTEN_BUDGET {
                        push(out, size, line.to_owned());
                        continue;
                    }
                    let header = format!("// ===== from {} =====", ascii_name(&target.name));
                    push(out, size, header);
                    stack.push(target.key);
                    let text = game_binds::import_file_text(&child.text, true);
                    self.expand(&text, stack, out, size);
                    stack.pop();
                } else if target.key == self.root_key && stack.contains(&self.root_key) {
                    let note =
                        format!("// HLSR: exec {} skipped (cycle)", ascii_name(&target.name));
                    push(out, size, note);
                } else if target.auto || self.skipped.contains(&target.key) {
                    let note = format!(
                        "// HLSR: exec {} skipped (not imported)",
                        ascii_name(&target.name)
                    );
                    push(out, size, note);
                } else {
                    push(out, size, line.to_owned());
                }
            }
        }

        fn expanded(&self, text: &str, mut stack: Vec<String>, size: &mut usize) -> String {
            let mut out = Vec::new();
            self.expand(text, &mut stack, &mut out, size);
            out.join("\n")
        }
    }

    let root_key = if root.virtual_root {
        String::new()
    } else {
        root_key(root, engine)
    };
    let flat = Expand {
        engine,
        files,
        skipped: missing.iter().map(|key| key.to_lowercase()).collect(),
        root_key: root_key.clone(),
    };
    let source_of = |key: &str| -> Option<&str> {
        match files.get(key) {
            Some(file) => Some(&file.text),
            None => (!key.is_empty() && key == root_key).then_some(root.text.as_str()),
        }
    };

    let root_text = if root.virtual_root {
        root.text.clone()
    } else {
        game_binds::import_file_text(&root.text, true)
    };
    let stack = if root_key.is_empty() {
        Vec::new()
    } else {
        vec![root_key.clone()]
    };
    // Один счётчик на корень и файлы скриптов: иначе каждый из них раскрывался бы до бюджета.
    let mut size = 0;
    let text = flat.expanded(&root_text, stack, &mut size);

    let mut runtime_files: Vec<RuntimeFile> = Vec::new();
    let mut unsafe_names: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: Vec<String> = vec![text.clone()];
    while let Some(next) = queue.pop() {
        for exec in execs_of(&next, engine) {
            let target = exec.target;
            let source = if exec.runtime && !seen.contains(&target.key) {
                source_of(&target.key)
            } else {
                None
            };
            let Some(source) = source else {
                continue;
            };
            if is_engine_run(target.key.rsplit('/').next().unwrap_or_default()) {
                continue;
            }
            seen.insert(target.key.clone());
            if !is_safe_import_name(&target.name) {
                unsafe_names.push(target.name);
                continue;
            }
            let body = flat.expanded(
                &game_binds::import_file_text(source, true),
                vec![target.key],
                &mut size,
            );
            queue.push(body.clone());
            runtime_files.push(RuntimeFile {
                file: target.name,
                text: body,
            });
        }
    }

    // Отчёт — как у build_import (цепочка, цикл, повторы); files — что вошло в итог.
    let mut report = build_import(engine, root, files, missing).report;
    let inlined: Vec<&str> = files.values().map(|file| file.name.as_str()).collect();
    report.files = inlined
        .iter()
        .filter(|name| !name.is_empty())
        .map(|name| (*name).to_owned())
        .collect();
    let bytes = std::iter::once(&text)
        .chain(runtime_files.iter().map(|item| &item.text))
        .map(|item| item.encode_utf16().count())
        .sum();
    let summary = Summary {
        files: runtime_files.len() + inlined.len(),
        ..summary_of(&text)
    };
    Flattened {
        text,
        runtime_files,
        report,
        unsafe_names,
        bytes,
        summary,
    }
}

/// Что цепочка задаёт несколько раз с разными значениями: в игре действует последнее.
/// `unbindall` сбрасывает историю биндов.
fn duplicates_of(flat: &[Flat]) -> Vec<Duplicate> {
    struct Seen {
        id: String,
        list: Vec<(String, String)>,
    }
    fn note(seen: &mut Vec<Seen>, kind: &str, name: &str, value: String, file: &str) {
        let id = format!("{kind}\n{name}");
        match seen.iter_mut().find(|slot| slot.id == id) {
            Some(slot) => slot.list.push((value, file.to_owned())),
            None => seen.push(Seen {
                id,
                list: vec![(value, file.to_owned())],
            }),
        }
    }
    let mut seen: Vec<Seen> = Vec::new();
    for item in flat {
        let args = &item.args;
        let verb = args[0].to_lowercase();
        if verb == "unbindall" {
            seen.retain(|slot| !slot.id.starts_with("bind\n"));
        } else if verb == "bind" && args.len() >= 3 {
            if let Some(key) = game_binds::normalize_key(&args[1]) {
                note(
                    &mut seen,
                    "bind",
                    &key,
                    game_binds::clean_command(&args[2..].join(" ")),
                    &item.file,
                );
            }
        } else if verb == "alias" && args.len() >= 2 {
            note(
                &mut seen,
                "alias",
                &args[1].to_lowercase(),
                args[2..].join(" "),
                &item.file,
            );
        } else if args.len() == 2 && is_identifier(&verb) && verb != "exec" && verb != "unbind" {
            note(&mut seen, "cvar", &verb, args[1].clone(), &item.file);
        }
    }
    let mut result = Vec::new();
    for slot in &seen {
        let distinct: HashSet<&str> = slot.list.iter().map(|(value, _)| value.as_str()).collect();
        if slot.list.len() < 2 || distinct.len() < 2 {
            continue;
        }
        let (kind, name) = slot.id.split_once('\n').unwrap_or((&slot.id, ""));
        result.push(Duplicate {
            kind: kind.to_owned(),
            name: name.to_owned(),
            files: slot.list.iter().map(|(_, file)| file.clone()).collect(),
            value: slot.list[slot.list.len() - 1].0.clone(),
        });
    }
    result
}

/// `/^[a-z_][a-z0-9_]*$/i`: имя переменной.
fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Файлы игрока, которые движок выполнил бы сам рядом с выбранными: к config.cfg —
/// autoexec.cfg (valve.rc выполняет его раньше config.cfg). userconfig.cfg отдельно не
/// нужен: его вызывает последняя строка config.cfg, и цепочка спросит его сама.
pub fn companions_of(names: &[String]) -> Vec<Companion> {
    let lower: HashSet<String> = names.iter().map(|name| name.to_lowercase()).collect();
    if !lower.contains("config.cfg") || lower.contains("autoexec.cfg") {
        return Vec::new();
    }
    vec![Companion {
        name: "autoexec.cfg",
        from: "config.cfg",
        before: true,
    }]
}

/// Несколько выбранных файлов — один импорт: userconfig.cfg выполняет их в порядке выбора,
/// каждый под своим именем. Второй файл с тем же именем (два config.cfg из разных папок)
/// получает номер.
pub fn root_of_picked(picked: &[Picked]) -> RootOfPicked {
    if let [only] = picked {
        return RootOfPicked {
            root: Root {
                name: only.name.clone(),
                text: only.text.clone(),
                virtual_root: false,
            },
            files: ImportFiles::new(),
        };
    }
    let mut files = ImportFiles::new();
    let mut lines: Vec<String> = Vec::new();
    let mut used: HashSet<String> = HashSet::new();
    for file in picked {
        let name = output_name(&file.name, &mut used);
        files.insert(
            name.to_lowercase(),
            ImportFile {
                name: file.name.clone(),
                text: file.text.clone(),
                target: Some(name.clone()),
            },
        );
        lines.push(format!("exec \"{name}\""));
    }
    RootOfPicked {
        root: Root {
            name: picked
                .iter()
                .map(|file| file.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            text: lines.join("\n"),
            virtual_root: true,
        },
        files,
    }
}
