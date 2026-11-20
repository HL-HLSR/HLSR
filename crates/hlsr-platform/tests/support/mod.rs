//! Общие помощники тестов платформы: фикстуры сверки с 0.4.7 (`tools/reference/platform/*.js`),
//! сравнение JSON с понятной разницей, временный процесс-«спящий» для проверок над настоящими
//! процессами.

#![allow(dead_code)]

pub mod sleeper;
pub mod world;

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

/// Фикстура, записанная скриптом `tools/reference/platform/<тема>.js`. Разбор — как
/// `JSON.parse` 0.4.7: одиночные суррогаты, которые пишет Node, становятся U+FFFD.
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

impl Mismatches {
    pub fn check(&mut self, actual: &Value, expected: &Value, context: impl std::fmt::Display) {
        if actual != expected {
            self.0
                .push(format!("{context}\n{}", diff(expected, actual)));
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
