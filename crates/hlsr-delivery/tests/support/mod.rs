//! Общие помощники тестов доставки: фикстуры сверки с 0.4.7, снимок дерева
//! файлов, сравнение JSON с понятной разницей.

#![allow(dead_code)]

pub mod api;
pub mod http;
pub mod library;
pub mod scenario;
pub mod tree;
pub mod zip;

use hlsr_core::json_text::{parse_js_json, stringify};
use serde_json::Value;
use std::path::PathBuf;

/// Путь к файлу из `tests/fixtures`.
pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Фикстура, записанная скриптом `tools/reference/delivery/<тема>.js`. Разбор —
/// как `JSON.parse` 0.4.7: одиночные суррогаты, которые пишет Node, становятся U+FFFD.
pub fn fixture(name: &str) -> Value {
    let path = fixture_path(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("фикстура {} не прочитана: {e}", path.display()));
    parse_js_json(&text).unwrap_or_else(|e| panic!("фикстура {} — не JSON: {e}", path.display()))
}

/// sha256 байтов в hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 0.4.9 (задача T0): у настроек игры есть `vulkan` (после `startTimer`), в фикстурах 0.4.7 его нет.
/// Объект настроек — без него, прочие значения — как есть.
pub fn without_vulkan(value: &Value) -> Value {
    match value {
        Value::Object(map) if map.contains_key("startTimer") => Value::Object(
            map.iter()
                .filter(|(key, _)| key.as_str() != "vulkan")
                .map(|(key, item)| (key.clone(), item.clone()))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Байты `game-settings.json` без строк `"vulkan": …` (файл, каким его записал бы 0.4.7).
pub fn without_vulkan_lines(bytes: &[u8]) -> Vec<u8> {
    bytes
        .split_inclusive(|&b| b == b'\n')
        .filter(|line| !line.trim_ascii_start().starts_with(b"\"vulkan\": "))
        .flatten()
        .copied()
        .collect()
}

/// 0.4.9 (задача T0): маркер библиотеки, которую создал сам лаунчер, несёт `createdBy`; в фикстурах
/// 0.4.7 его нет. В снимке дерева у `.hlsr/library.json` строка убирается, размер и sha256 —
/// по оставшемуся тексту.
pub fn marker_as_0_4_7(value: &Value) -> Value {
    const CREATED_BY: &str = ",\n  \"createdBy\": \"launcher\"";
    match value {
        Value::Array(list) => Value::Array(list.iter().map(marker_as_0_4_7).collect()),
        Value::Object(map) => {
            let mut out: serde_json::Map<String, Value> = map
                .iter()
                .map(|(key, item)| (key.clone(), marker_as_0_4_7(item)))
                .collect();
            if let Some(text) = map.get("latin1").and_then(Value::as_str)
                && text.contains("\"id\": \"hlsr-library\"")
                && text.contains(CREATED_BY)
            {
                let text = text.replace(CREATED_BY, "");
                let bytes: Vec<u8> = text.chars().map(|c| c as u8).collect();
                out.insert("size".into(), serde_json::json!(bytes.len()));
                out.insert("sha256".into(), serde_json::json!(sha256_hex(&bytes)));
                out.insert("latin1".into(), serde_json::json!(text));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// Сверить текст с описанием из фикстуры: `{text}` или `{length, sha256}` (длина в байтах UTF-8).
/// Ошибка — понятная строка: что ожидалось и что получилось.
pub fn check_summary(actual: &str, expected: &Value) -> Result<(), String> {
    if let Some(text) = expected["text"].as_str() {
        return if actual == text {
            Ok(())
        } else {
            Err(format!("ожидалось:\n{text}\nполучено:\n{actual}"))
        };
    }
    let length = expected["length"].as_u64().unwrap_or(u64::MAX);
    let sha = str_of(&expected["sha256"]);
    if actual.len() as u64 == length && sha256_hex(actual.as_bytes()) == sha {
        Ok(())
    } else {
        Err(format!(
            "ожидалась запись длиной {length} (sha256 {sha}), получена длиной {} (sha256 {}):\n{actual}",
            actual.len(),
            sha256_hex(actual.as_bytes())
        ))
    }
}

/// Строка из значения фикстуры.
pub fn str_of(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("ожидалась строка, а не {value}"))
}

/// Строка или `null`.
pub fn opt_str(value: &Value) -> Option<&str> {
    value.as_str()
}

/// Массив фикстуры.
pub fn items(value: &Value) -> &Vec<Value> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("ожидался массив, а не {value}"))
}

/// Сравнить JSON с ожиданием; при расхождении — строки, которые отличаются
/// (`-` ожидание 0.4.7, `+` Rust), с двумя строками контекста.
#[track_caller]
pub fn assert_json_eq(actual: &Value, expected: &Value, context: &str) {
    if actual == expected {
        return;
    }
    panic!("{context}: расхождение с 0.4.7\n{}", diff(expected, actual));
}

/// Собрать расхождения нескольких случаев и упасть один раз со всеми.
#[derive(Default)]
pub struct Mismatches(Vec<String>);

/// Ожидание сценария, снятого в Linux, для этой ОС. В Windows:
/// - адрес без слушателя — порт 0 ([`http::dead_port`]), отказ приходит как `EADDRNOTAVAIL`,
///   а не `ECONNREFUSED` (только для `{dead}`: настоящий закрытый порт отвечает отказом);
/// - 7-Zip 21.07 (`resources/7zip/7za.exe`) пишет «Cannot open the file as archive» через CRLF,
///   p7zip 16.02 в Linux — «Can not open the file as archive» через LF.
fn windows_expectation(expected: &Value) -> std::borrow::Cow<'_, Value> {
    const REWRITES: [(&str, &str); 2] = [
        (
            "connect ECONNREFUSED {dead}",
            "connect EADDRNOTAVAIL {dead}",
        ),
        (
            "\nCan not open the file as archive",
            "\r\nCannot open the file as archive",
        ),
    ];
    fn rewrite(value: &mut Value) {
        match value {
            Value::String(text) if REWRITES.iter().any(|(from, _)| text.contains(from)) => {
                for (from, to) in REWRITES {
                    *text = text.replace(from, to);
                }
            }
            Value::Array(items) => items.iter_mut().for_each(rewrite),
            Value::Object(map) => map.values_mut().for_each(rewrite),
            _ => {}
        }
    }
    if !cfg!(windows) {
        return std::borrow::Cow::Borrowed(expected);
    }
    let mut owned = expected.clone();
    rewrite(&mut owned);
    std::borrow::Cow::Owned(owned)
}

impl Mismatches {
    pub fn check(&mut self, actual: &Value, expected: &Value, context: impl std::fmt::Display) {
        let expected = windows_expectation(expected);
        if actual != &*expected {
            self.0
                .push(format!("{context}\n{}", diff(&expected, actual)));
        }
    }

    pub fn check_eq<T: PartialEq + std::fmt::Debug>(
        &mut self,
        actual: T,
        expected: T,
        context: impl std::fmt::Display,
    ) {
        if actual != expected {
            self.0
                .push(format!("{context}\n- {expected:?}\n+ {actual:?}"));
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

fn diff(expected: &Value, actual: &Value) -> String {
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

/// Строка журнала проверки файлов 0.4.9 без чисел, которых в 0.4.7 нет («стоковых cfg нет N,
/// изменённых cfg M»): для сверки с фикстурами 0.4.7.
pub fn integrity_log_as_0_4_7(value: Value) -> Value {
    static EXTRA: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r", стоковых cfg нет \d+, изменённых cfg \d+").unwrap()
    });
    match value {
        Value::String(text) => Value::String(EXTRA.replace(&text, "").into_owned()),
        Value::Array(list) => Value::Array(list.into_iter().map(integrity_log_as_0_4_7).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, item)| (key, integrity_log_as_0_4_7(item)))
                .collect(),
        ),
        other => other,
    }
}
