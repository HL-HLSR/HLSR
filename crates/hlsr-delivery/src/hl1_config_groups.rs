//! Находки проверки конфига HL1 ([`crate::hl1_config_check`]) для окна
//! (`shared/hl1-config-groups.js` 0.4.9): одна группа на имя команды и правило, внутри —
//! места. Модуль лёгкий: таблицу разрешённых команд он не трогает. На входе — находки, как их
//! отдаёт канал `player-config-check` (JSON, у каждой ещё `path`).

use hlsr_core::js;
use serde_json::{Value, json};

/// Нормализованное правило → строка локали.
pub const RULE_KEYS: [(&str, &str); 12] = [
    ("banned", "#L_HC_RULE_BANNED"),
    ("unknown", "#L_HC_RULE_UNKNOWN"),
    ("scripted", "#L_HC_RULE_SCRIPTED"),
    ("scripted-only", "#L_HC_RULE_SCRIPTED_ONLY"),
    ("no-script-seq", "#L_HC_RULE_NO_SEQ"),
    ("category", "#L_HC_RULE_CATEGORY"),
    ("thirdperson", "#L_HC_RULE_THIRDPERSON"),
    ("steam", "#L_HC_RULE_STEAM"),
    ("values", "#L_HC_RULE_VALUES"),
    ("skill", "#L_HC_RULE_SKILL"),
    ("run-start", "#L_HC_RULE_RUN_START"),
    ("missing", "#L_HC_RULE_MISSING"),
];

/// Строка локали правила (`RULE_KEYS[rule]`).
pub fn rule_key(rule: &str) -> Option<&'static str> {
    RULE_KEYS
        .iter()
        .find(|(name, _)| *name == rule)
        .map(|(_, key)| *key)
}

/// Неизвестный уровень — последним: у 0.4.9 он `NaN` и «равен» любому, а такой компаратор не
/// даёт полного порядка, и `sort_by` Rust 1.81+ вправе паниковать.
fn level_order(level: &str) -> u8 {
    match level {
        "error" => 0,
        "warning" => 1,
        "info" => 2,
        _ => 3,
    }
}

/// Место находки в группе. `trail` — путь структурой, как в находке (текст строит окно).
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub file: Value,
    pub path: Option<String>,
    pub line: Value,
    pub command: Value,
    pub context: Value,
    pub trail: Value,
}

/// Группа находок одной команды по одному правилу.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    /// «имя правило».
    pub key: String,
    pub name: String,
    pub rule: String,
    pub level: String,
    pub note: String,
    pub places: Vec<Place>,
}

impl Group {
    pub fn to_value(&self) -> Value {
        let places: Vec<Value> = self
            .places
            .iter()
            .map(|place| {
                json!({
                    "file": place.file, "path": place.path, "line": place.line, "command": place.command,
                    "context": place.context, "trail": place.trail,
                })
            })
            .collect();
        json!({
            "key": self.key, "name": self.name, "rule": self.rule, "level": self.level,
            "note": self.note, "places": places,
        })
    }
}

/// `String(value)` для строки и числа из JSON; прочее — как пустая строка.
fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => js::to_string(other),
    }
}

/// `groupFindings`: сначала ошибки, затем предупреждения и справки; внутри уровня больше мест —
/// выше, потом по ключу.
pub fn group_findings(findings: &[Value]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for finding in findings {
        let command = text_of(&finding["command"]);
        let name = command
            .split(js::is_whitespace)
            .next()
            .unwrap_or_default()
            .to_lowercase();
        let rule = text_of(&finding["rule"]);
        let key = format!("{name} {rule}");
        let at = match groups.iter().position(|group| group.key == key) {
            Some(at) => at,
            None => {
                groups.push(Group {
                    key,
                    name,
                    rule,
                    level: text_of(&finding["level"]),
                    note: finding["note"].as_str().unwrap_or_default().to_owned(),
                    places: Vec::new(),
                });
                groups.len() - 1
            }
        };
        groups[at].places.push(Place {
            file: finding["file"].clone(),
            path: finding["path"]
                .as_str()
                .filter(|path| !path.is_empty())
                .map(str::to_owned),
            line: finding["line"].clone(),
            command: finding["command"].clone(),
            context: finding["context"].clone(),
            trail: if js::truthy(&finding["trail"]) {
                finding["trail"].clone()
            } else {
                Value::Array(Vec::new())
            },
        });
    }
    groups.sort_by(|a, b| {
        level_order(&a.level)
            .cmp(&level_order(&b.level))
            .then(b.places.len().cmp(&a.places.len()))
            .then_with(|| js::cmp_utf16(&a.key, &b.key))
    });
    groups
}

/// `ruleDetail`: хвост пояснения после последнего «: » у правил values и skill — допустимые
/// значения или значение по умолчанию (данные, не текст; их показывают рядом с переводом).
pub fn rule_detail(group: &Group) -> &str {
    if group.rule != "values" && group.rule != "skill" {
        return "";
    }
    group
        .note
        .rfind(": ")
        .map_or("", |at| &group.note[at + 2..])
}
