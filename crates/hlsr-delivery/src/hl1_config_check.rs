//! Проверка конфига HL1 по таблице разрешённых команд speedrun.com (`shared/hl1-config-check.js`
//! 0.4.9; таблица — `data/hl1-whitelist.json`, её собирает `scripts/build-hl1-whitelist.js` 0.4.9).
//!
//! Чистая функция: тексты файлов приходят снаружи. Проход как у движка: команды корневого
//! файла по порядку, exec уходит в файл сразу. Плюс всё, что может выполниться позже: тела
//! alias, команды биндов, bind внутри bind и alias, exec внутри них (эти файлы проверяются тоже).
//!
//! Уровни: error — запрещено или нет в таблице; warning — зависит от категории или значение не
//! то; info — только для старта рана и файлы, которых нет.
//!
//! Отступление от 0.4.9: таблица там — объект JSON, и имя свойства `Object.prototype`
//! (`constructor`, `toString`…) находилось в ней как команда без правила — без находки. Здесь
//! такая команда «нет в таблице», как любая другая.

use crate::game_binds::parse_cfg;
use crate::game_config::line_commands;
use hlsr_core::js;
use hlsr_core::js::object::JsObject;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Таблица разрешённых команд HL1 (байты `src/shared/hl1-whitelist.json` 0.4.9).
pub const WHITELIST_JSON: &str = include_str!("../data/hl1-whitelist.json");

const RUN_START_ANCHOR: &str = "map c1a0";
/// Предел вложенности тел alias/bind: 0.4.9 на строке `alias x alias x …` падает
/// перехватываемым RangeError, здесь кончился бы стек (abort). Живые конфиги — 3–4 уровня.
const MAX_BODY_DEPTH: usize = 32;
const WARN_RULES: [&str; 4] = ["scripted", "category", "thirdperson", "steam"];

fn rule_text(rule: &str) -> &'static str {
    match rule {
        "scripted" => "только в scripted-категориях",
        "category" => "зависит от категории",
        "thirdperson" => "только в категории с thirdperson",
        "steam" => "только в Steam-версии",
        _ => "",
    }
}

/// Запись таблицы: правило, пояснение, значение по умолчанию (пустое в таблице — нет) и
/// допустимые значения.
struct Entry {
    rule: String,
    note: String,
    default: Option<String>,
    values: Vec<String>,
}

struct Whitelist {
    commands: HashMap<String, Entry>,
    cvars: HashMap<String, Entry>,
    /// Префикс, это переменные (`kind: "cvars"`), запись.
    patterns: Vec<(String, bool, Entry)>,
    skill: HashMap<String, String>,
}

fn entry_of(value: &Value) -> Entry {
    let text = |key: &str| value[key].as_str().unwrap_or_default().to_owned();
    Entry {
        rule: text("rule"),
        note: text("note"),
        default: value["default"]
            .as_str()
            .filter(|d| !d.is_empty())
            .map(str::to_owned),
        values: value["values"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

static WHITELIST: LazyLock<Whitelist> = LazyLock::new(|| {
    let data: Value =
        serde_json::from_str(WHITELIST_JSON).expect("таблица разрешённых команд — JSON");
    let section = |name: &str| -> HashMap<String, Entry> {
        data[name]
            .as_object()
            .expect("раздел таблицы")
            .iter()
            .map(|(key, value)| (key.clone(), entry_of(value)))
            .collect()
    };
    Whitelist {
        commands: section("commands"),
        cvars: section("cvars"),
        patterns: data["patterns"]
            .as_array()
            .expect("шаблоны таблицы")
            .iter()
            .map(|p| {
                let prefix = p["prefix"].as_str().unwrap_or_default().to_owned();
                (prefix, p["kind"] == "cvars", entry_of(p))
            })
            .collect(),
        skill: data["skill"]
            .as_object()
            .expect("раздел skill")
            .iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_owned()))
            .collect(),
    }
});

/// Категория рана: всё, кроме `scripted`, — Scriptless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Category {
    #[default]
    Scriptless,
    Scripted,
}

impl Category {
    pub fn parse(value: &str) -> Category {
        if value == "scripted" {
            Category::Scripted
        } else {
            Category::Scriptless
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Category::Scriptless => "scriptless",
            Category::Scripted => "scripted",
        }
    }
}

/// Шаг пути к находке: alias, bind или exec из тела alias/bind (`via` — путь до этого exec).
/// Текст пути строит окно.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Alias(String),
    Bind(String),
    Exec(Vec<Step>),
}

impl Step {
    pub fn to_value(&self) -> Value {
        match self {
            Step::Alias(name) => json!({"kind": "alias", "name": name}),
            Step::Bind(name) => json!({"kind": "bind", "name": name}),
            Step::Exec(via) => json!({"kind": "exec", "via": steps_value(via)}),
        }
    }
}

fn steps_value(steps: &[Step]) -> Value {
    Value::Array(steps.iter().map(Step::to_value).collect())
}

/// Находка проверки. `level` — `error`, `warning` или `info`; `context` — путь текстом
/// («alias y > bind j» или «top level»), `trail` — он же структурой.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub file: String,
    pub line: usize,
    pub command: String,
    pub level: &'static str,
    pub rule: String,
    pub note: String,
    pub context: String,
    pub trail: Vec<Step>,
}

impl Finding {
    pub fn to_value(&self) -> Value {
        json!({
            "file": self.file, "line": self.line, "command": self.command, "level": self.level,
            "rule": self.rule, "note": self.note, "context": self.context, "trail": steps_value(&self.trail),
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub category: Category,
    pub files: usize,
    pub commands: usize,
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
}

impl Stats {
    pub fn to_value(&self) -> Value {
        json!({
            "category": self.category.as_str(), "files": self.files, "commands": self.commands,
            "errors": self.errors, "warnings": self.warnings, "infos": self.infos,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckResult {
    pub findings: Vec<Finding>,
    pub stats: Stats,
}

impl CheckResult {
    /// `{ findings, stats }` как у `checkConfig` 0.4.9.
    pub fn to_value(&self) -> Value {
        json!({
            "findings": self.findings.iter().map(Finding::to_value).collect::<Vec<_>>(),
            "stats": self.stats.to_value(),
        })
    }
}

/// Имя файла в exec как его ищет движок: обратные слэши — прямые, нижний регистр, без
/// `./` в начале, `.cfg` по умолчанию.
fn exec_name(raw: &str) -> String {
    let lower = raw.to_lowercase().replace('\\', "/");
    let mut name = lower.strip_prefix("./").unwrap_or(&lower).to_owned();
    // `/\.[a-z0-9]+$/`: после последней точки — только латиница в нижнем регистре и цифры.
    let has_extension = name.rsplit_once('.').is_some_and(|(_, ext)| {
        !ext.is_empty()
            && ext
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    });
    if !has_extension {
        name.push_str(".cfg");
    }
    name
}

/// `sameValue`: числа сравниваются как числа, остальное — строкой без учёта регистра.
fn same_value(a: &str, b: &str) -> bool {
    let (x, y) = (js::to_number(a), js::to_number(b));
    if x.is_nan() || y.is_nan() || a.is_empty() || b.is_empty() {
        a.to_lowercase() == b.to_lowercase()
    } else {
        x == y
    }
}

struct Found<'a> {
    entry: &'a Entry,
    key: &'a str,
    is_cvar: bool,
}

/// Запись таблицы для команды: точное имя команды с аргументом («map c1a0» раньше «map»),
/// переменная, команда, потом самый длинный префикс.
fn lookup(args: &[String]) -> Option<Found<'static>> {
    let list = &*WHITELIST;
    let verb = args[0].to_lowercase();
    if args.len() > 1 {
        let with_arg = format!("{verb} {}", args[1].to_lowercase());
        if let Some((key, entry)) = list.commands.get_key_value(&with_arg) {
            return Some(Found {
                entry,
                key,
                is_cvar: false,
            });
        }
    }
    if let Some((key, entry)) = list.cvars.get_key_value(&verb) {
        return Some(Found {
            entry,
            key,
            is_cvar: true,
        });
    }
    if let Some((key, entry)) = list.commands.get_key_value(&verb) {
        return Some(Found {
            entry,
            key,
            is_cvar: false,
        });
    }
    let mut best: Option<&(String, bool, Entry)> = None;
    for pattern in &list.patterns {
        if verb.starts_with(&pattern.0) && best.is_none_or(|b| pattern.0.len() > b.0.len()) {
            best = Some(pattern);
        }
    }
    best.map(|(prefix, is_cvar, entry)| Found {
        entry,
        key: prefix,
        is_cvar: *is_cvar,
    })
}

/// Место команды: файл, строка, путь текстом и структурой, тело alias/bind или нет.
#[derive(Clone)]
struct Site {
    file: String,
    line: usize,
    path: Vec<String>,
    trail: Vec<Step>,
    body: bool,
}

struct Check<'a> {
    files: &'a JsObject<String>,
    category: Category,
    findings: Vec<Finding>,
    aliases: HashSet<String>,
    stats: Stats,
    collecting: bool,
    visited: HashSet<String>,
}

impl Check<'_> {
    fn is_own_name(&self, name: &str) -> bool {
        let base = name
            .strip_prefix('+')
            .or_else(|| name.strip_prefix('-'))
            .unwrap_or(name);
        self.aliases.contains(name)
            || self.aliases.contains(base)
            || self.aliases.contains(&format!("+{base}"))
            || self.aliases.contains(&format!("-{base}"))
    }

    fn report(
        &mut self,
        site: &Site,
        args: &[String],
        level: &'static str,
        rule: &str,
        note: String,
    ) {
        if self.collecting {
            return;
        }
        self.findings.push(Finding {
            file: site.file.clone(),
            line: site.line,
            command: args.join(" "),
            level,
            rule: rule.to_owned(),
            note,
            context: if site.path.is_empty() {
                "top level".to_owned()
            } else {
                site.path.join(" > ")
            },
            trail: site.trail.clone(),
        });
        match level {
            "error" => self.stats.errors += 1,
            "warning" => self.stats.warnings += 1,
            _ => self.stats.infos += 1,
        }
    }

    /// Проверка одной команды; `sequence` — команды той же строки или того же тела alias/bind.
    fn check(&mut self, args: &[String], site: &Site, sequence: &[Vec<String>]) {
        if !self.collecting {
            self.stats.commands += 1;
        }
        let verb = args[0].to_lowercase();
        let value = args.get(1).map(String::as_str);

        if verb.starts_with("sk_") {
            match WHITELIST.skill.get(&verb) {
                None => self.report(
                    site,
                    args,
                    "error",
                    "unknown",
                    "нет в таблице разрешённых команд (раздел HL1 вкладки Skill)".to_owned(),
                ),
                Some(default) if value.is_some_and(|v| !same_value(v, default)) => self.report(
                    site,
                    args,
                    "warning",
                    "skill",
                    format!("значение должно быть по умолчанию: {default}"),
                ),
                Some(_) => {}
            }
            return;
        }

        let Some(found) = lookup(args) else {
            if !self.is_own_name(&verb) {
                self.report(
                    site,
                    args,
                    "error",
                    "unknown",
                    "нет в таблице разрешённых команд".to_owned(),
                );
            }
            return;
        };
        let entry = found.entry;
        let note = entry.note.clone();
        // Запрещённая переменная или переменная с набором значений, у которой значение по
        // умолчанию таблицы (bxt_hud_viewangles 0 — выключено), не нарушение.
        let is_default = found.is_cvar
            && entry
                .default
                .as_deref()
                .is_some_and(|default| value.is_some_and(|v| same_value(v, default)));
        match entry.rule.as_str() {
            "banned" => {
                if !is_default {
                    let note = if note.is_empty() {
                        "запрещено правилами".to_owned()
                    } else {
                        note
                    };
                    self.report(site, args, "error", "banned", note);
                }
            }
            "scripted-only" => {
                if self.category != Category::Scripted {
                    let note = "В Scriptless нельзя (только Scripted)".to_owned();
                    self.report(site, args, "error", "scripted-only", note);
                }
            }
            "no-script-seq" => {
                // Одиночная команда в теле alias/bind и верхний уровень не флагуются: запрещена
                // только связка из нескольких команд.
                if site.body && sequence.len() >= 2 {
                    let note = "Нельзя в последовательности команд (alias, bind)".to_owned();
                    self.report(site, args, "error", "no-script-seq", note);
                }
            }
            rule if WARN_RULES.contains(&rule) => {
                let note = if note.is_empty() {
                    rule_text(rule).to_owned()
                } else {
                    note
                };
                self.report(site, args, "warning", rule, note);
            }
            "values" => {
                if let Some(v) = value
                    && !is_default
                    && !entry.values.iter().any(|allowed| same_value(v, allowed))
                {
                    let note = format!("допустимые значения: {}", entry.values.join(", "));
                    self.report(site, args, "warning", "values", note);
                }
            }
            "run-start" => {
                // Старт рана: bind/alias, где есть map c1a0, — так и задумано.
                let is_anchor = found.key == RUN_START_ANCHOR;
                let with_anchor = sequence.iter().any(|other| {
                    !other.is_empty()
                        && format!(
                            "{} {}",
                            other[0].to_lowercase(),
                            other.get(1).map(|s| s.to_lowercase()).unwrap_or_default()
                        ) == RUN_START_ANCHOR
                });
                if !(site.body && (is_anchor || with_anchor)) {
                    self.report(site, args, "info", "run-start", note);
                }
            }
            _ => {}
        }
    }

    fn run_sequence(&mut self, sequence: &[Vec<String>], site: &Site) {
        for args in sequence {
            let verb = args[0].to_lowercase();
            if verb == "alias" && args.len() >= 2 {
                self.aliases.insert(args[1].to_lowercase());
                let body = args[2..].join(" ");
                if !body.is_empty() && site.path.len() < MAX_BODY_DEPTH {
                    let inner = self.nested(site, "alias", &args[1], Step::Alias(args[1].clone()));
                    self.run_sequence(&parse_cfg(&body), &inner);
                }
            } else if verb == "bind" && args.len() >= 3 {
                if site.path.len() < MAX_BODY_DEPTH {
                    let inner = self.nested(site, "bind", &args[1], Step::Bind(args[1].clone()));
                    self.run_sequence(&parse_cfg(&args[2..].join(" ")), &inner);
                }
            } else if verb == "exec" && args.len() >= 2 {
                self.run_file(&args[1], Some(site), Some(args));
            }
            self.check(args, site, sequence);
        }
    }

    fn nested(&self, site: &Site, kind: &str, name: &str, step: Step) -> Site {
        let mut inner = site.clone();
        inner.body = true;
        inner.path.push(format!("{kind} {name}"));
        inner.trail.push(step);
        inner
    }

    fn run_file(&mut self, raw_name: &str, from: Option<&Site>, exec_args: Option<&[String]>) {
        let name = exec_name(raw_name);
        if !self.visited.insert(name.clone()) {
            return;
        }
        let Some(text) = self.files.get(&name) else {
            let site = from.cloned().unwrap_or_else(|| Site {
                file: name.clone(),
                line: 0,
                path: Vec::new(),
                trail: Vec::new(),
                body: false,
            });
            let fallback = ["exec".to_owned(), raw_name.to_owned()];
            let args = exec_args.unwrap_or(&fallback);
            self.report(&site, args, "info", "missing", "файл не найден".to_owned());
            return;
        };
        if !self.collecting {
            self.stats.files += 1;
        }
        // Файл из exec внутри bind/alias выполнится позже, но его команды — верхний уровень
        // этого файла.
        let via: Vec<String> = from
            .filter(|from| !from.path.is_empty())
            .map(|from| vec![format!("exec из {}", from.path.join(" > "))])
            .unwrap_or_default();
        let via_trail: Vec<Step> = from
            .filter(|from| !from.trail.is_empty())
            .map(|from| vec![Step::Exec(from.trail.clone())])
            .unwrap_or_default();
        for (index, raw_line) in text.split('\n').enumerate() {
            let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            let sequence: Vec<Vec<String>> = line_commands(line)
                .into_iter()
                .map(|command| command.args.into_iter().map(|token| token.value).collect())
                .collect();
            if !sequence.is_empty() {
                let site = Site {
                    file: name.clone(),
                    line: index + 1,
                    path: via.clone(),
                    trail: via_trail.clone(),
                    body: false,
                };
                self.run_sequence(&sequence, &site);
            }
        }
    }
}

/// `checkConfig` 0.4.9: `files` — тексты по именам файлов в нижнем регистре (как их ищет
/// exec), `root` — корневой файл.
pub fn check_config(files: &JsObject<String>, root: &str, category: Category) -> CheckResult {
    let mut check = Check {
        files,
        category,
        findings: Vec::new(),
        aliases: HashSet::new(),
        stats: Stats {
            category,
            ..Stats::default()
        },
        collecting: true,
        visited: HashSet::new(),
    };
    // Первый проход собирает имена alias игрока, второй проверяет.
    check.run_file(root, None, None);
    check.collecting = false;
    check.visited.clear();
    check.run_file(root, None, None);
    CheckResult {
        findings: check.findings,
        stats: check.stats,
    }
}
