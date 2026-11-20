//! Общие помощники тестов онлайна: фикстуры сверки с 0.4.7 (скрипты `tools/reference/online/`),
//! двойник сервера, сравнение JSON с понятной разницей. Устройство — как у
//! `crates/hlsr-delivery/tests/support`.

#![allow(dead_code)]

pub mod chat;
pub mod online_profile;
pub mod recipes;
pub mod server;

use hlsr_core::json_text::{parse_js_json, stringify};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// Поля карточки и снимка 0.4.9 (подписка, буст, права, привязки): в фикстурах 0.4.7 их нет.
/// Сверяет их `profile_model.rs` (`profile-state-049.json`) и `online_profile_service.rs`
/// (`online-profile-service-049.json`).
pub const KEYS_049: [&str; 7] = [
    "subscriber",
    "boosterSince",
    "supporterSince",
    "perks",
    "stylePreset",
    "telegram",
    "discordLink",
];

/// Карточка или снимок без полей [`KEYS_049`]; не объект — как есть.
pub fn without_049(value: &Value) -> Value {
    let mut value = value.clone();
    if let Some(map) = value.as_object_mut() {
        for key in KEYS_049 {
            map.shift_remove(key);
        }
    }
    value
}

/// Снимок или состояние профиля 0.4.9 (`achievements` и `summary`) в вид 0.4.7 для сверки со
/// старыми фикстурами: 17 достижений 0.4.7 в порядке таблицы, без `tier`, времени и замков
/// (секретные — только открытые), сводка по ним; без `stats.launches`, `streak` и полей
/// [`KEYS_049`]. Не состояние — `None`. Достижения 0.4.9 сверяет `achievements_049.rs`.
pub fn profile_as_047(value: &Value) -> Option<Value> {
    use hlsr_online::profile::achievements::{ACHIEVEMENTS, summarize};
    let map = value.as_object()?;
    let list = map.get("achievements")?.as_array()?;
    map.get("summary")?;
    let mut old: Vec<(usize, Value)> = list
        .iter()
        .filter(|item| item.get("hidden") != Some(&json!(true)))
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?;
            let index = ACHIEVEMENTS[..17].iter().position(|a| a.id == id)?;
            let mut out = Map::new();
            for key in ["id", "title", "description", "icon"] {
                out.insert(key.to_owned(), item[key].clone());
            }
            if item.get("secret").is_some() {
                out.insert("secret".to_owned(), json!(true));
            }
            for (key, value) in item.as_object()? {
                if !["tier", "secret", "hidden"].contains(&key.as_str()) && !out.contains_key(key) {
                    out.insert(key.clone(), value.clone());
                }
            }
            if index != 0 {
                out.insert("at".to_owned(), Value::Null);
            }
            Some((index, Value::Object(out)))
        })
        .collect();
    old.sort_by_key(|(index, _)| *index);
    let old: Vec<Value> = old.into_iter().map(|(_, item)| item).collect();
    let summary = if old.is_empty() {
        json!({"unlocked": 0, "total": 12, "percent": 0})
    } else {
        summarize(&old)
    };
    let mut out = without_049(value).as_object()?.clone();
    out.insert("achievements".to_owned(), Value::Array(old));
    out.insert("summary".to_owned(), summary);
    out.shift_remove("streak");
    if let Some(stats) = out.get_mut("stats").and_then(Value::as_object_mut) {
        stats.shift_remove("launches");
    }
    Some(Value::Object(out))
}

/// Путь к файлу из `tests/fixtures`.
pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Фикстура, записанная скриптом `tools/reference/online/<тема>.js`. Разбор — как `JSON.parse`
/// 0.4.7: одиночные суррогаты, которые пишет Node, становятся U+FFFD.
pub fn fixture(name: &str) -> Value {
    let path = fixture_path(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("фикстура {} не прочитана: {e}", path.display()));
    parse_js_json(&text).unwrap_or_else(|e| panic!("фикстура {} — не JSON: {e}", path.display()))
}

/// Строка из значения фикстуры.
pub fn str_of(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("ожидалась строка, а не {value}"))
}

/// Массив фикстуры.
pub fn items(value: &Value) -> &Vec<Value> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("ожидался массив, а не {value}"))
}

/// Собрать расхождения нескольких случаев и упасть один раз со всеми.
#[derive(Default)]
pub struct Mismatches(Vec<String>);

impl Mismatches {
    /// Порядок ключей тоже сверяется: текст в порядке JS, как у `JSON.stringify`.
    pub fn check(&mut self, actual: &Value, expected: &Value, context: impl std::fmt::Display) {
        if actual != expected || stringify(actual, "") != stringify(expected, "") {
            self.0
                .push(format!("{context}\n{}", diff(expected, actual)));
        }
    }

    #[track_caller]
    pub fn finish(self, what: &str) {
        if !self.0.is_empty() {
            let shown: Vec<_> = self.0.iter().take(20).cloned().collect();
            panic!(
                "{what}: {} расхождений с 0.4.7 (показаны первые {}):\n\n{}",
                self.0.len(),
                shown.len(),
                shown.join("\n\n")
            );
        }
    }
}

/// Строки, которые отличаются (`-` ожидание 0.4.7, `+` Rust), с двумя строками контекста.
pub fn diff(expected: &Value, actual: &Value) -> String {
    let expected = stringify(expected, "  ");
    let actual = stringify(actual, "  ");
    let left: Vec<&str> = expected.lines().collect();
    let right: Vec<&str> = actual.lines().collect();
    let prefix = left.iter().zip(&right).take_while(|(a, b)| a == b).count();
    let suffix = left[prefix..]
        .iter()
        .rev()
        .zip(right[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let from = prefix.saturating_sub(2);
    let mut out = Vec::new();
    for line in &left[from..prefix] {
        out.push(format!("  {line}"));
    }
    for line in left[prefix..left.len() - suffix].iter().take(40) {
        out.push(format!("- {line}"));
    }
    for line in right[prefix..right.len() - suffix].iter().take(40) {
        out.push(format!("+ {line}"));
    }
    for line in left[left.len() - suffix..].iter().take(2) {
        out.push(format!("  {line}"));
    }
    out.join("\n")
}

/// Путь из «/»-пути сценария.
pub fn path_in(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |acc, part| acc.join(part))
}

/// Байты из шестнадцатеричной записи.
pub fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
        .collect()
}

/// Снимок папки как `files(root)` из `tools/reference/online/lib/files.js`: путь → текст latin1.
pub fn files(root: &Path) -> Value {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, String)>) {
        for item in std::fs::read_dir(dir).unwrap() {
            let item = item.unwrap();
            let name = item.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if item.file_type().unwrap().is_dir() {
                walk(&item.path(), &rel, out);
            } else {
                let bytes = std::fs::read(item.path()).unwrap();
                out.push((rel, hlsr_core::js::latin1_string(&bytes)));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, "", &mut out);
    out.sort_by(|a, b| hlsr_core::js::cmp_utf16(&a.0, &b.0));
    Value::Object(
        out.into_iter()
            .map(|(path, text)| (path, json!(text)))
            .collect::<Map<_, _>>(),
    )
}

/// Разложить файлы `setup` сценария: `{file, utf8 | hex | latin1}`, `{dir}`.
pub fn setup(root: &Path, items_: &Value) {
    for item in items_.as_array().into_iter().flatten() {
        if let Some(dir) = item["dir"].as_str() {
            std::fs::create_dir_all(path_in(root, dir)).unwrap();
            continue;
        }
        let file = path_in(root, str_of(&item["file"]));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let bytes = if let Some(text) = item["utf8"].as_str() {
            text.as_bytes().to_vec()
        } else if let Some(hex) = item["hex"].as_str() {
            hex_bytes(hex)
        } else if let Some(text) = item["latin1"].as_str() {
            hlsr_core::js::latin1_bytes(text)
        } else {
            Vec::new()
        };
        std::fs::write(file, bytes).unwrap();
    }
}

/// Строки журнала с прошлого вызова как в фикстуре: `[[уровень, текст], …]`.
pub fn log_lines(logger: &hlsr_delivery::ports::MemoryLogger) -> Value {
    Value::Array(
        logger
            .take()
            .into_iter()
            .map(|(level, text)| json!([level, text]))
            .collect(),
    )
}

/// Поддельные часы: общее значение, которое тест двигает.
#[derive(Clone)]
pub struct FakeClock(pub std::sync::Arc<std::sync::atomic::AtomicI64>);

impl FakeClock {
    pub fn new(ms: i64) -> FakeClock {
        FakeClock(std::sync::Arc::new(std::sync::atomic::AtomicI64::new(ms)))
    }
    pub fn set(&self, ms: i64) {
        self.0.store(ms, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn get(&self) -> i64 {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
    pub fn clock(&self) -> hlsr_delivery::timing::Clock {
        let value = self.0.clone();
        std::sync::Arc::new(move || value.load(std::sync::atomic::Ordering::SeqCst))
    }
}

/// `Math.random`, подменённый последовательностью по кругу.
pub fn sequence_random(values: Vec<f64>) -> hlsr_delivery::timing::Random {
    let index = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    std::sync::Arc::new(move || {
        let at = index.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        values[at % values.len()]
    })
}
