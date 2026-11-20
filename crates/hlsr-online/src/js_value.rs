//! Проверки значений JSON так, как их делают модули профиля 0.4.7: `Number.isSafeInteger`,
//! `Number(value)`, `typeof value === "string" ? value.slice(0, max) : ""`, `String(value)` в
//! `regex.test(value)`.

use hlsr_core::js;
use serde_json::Value;

/// `Number.MAX_SAFE_INTEGER`.
pub const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// `Number.isSafeInteger(value)`: значение — целое число не больше 2⁵³ − 1 по модулю.
pub fn safe_integer(value: Option<&Value>) -> Option<i64> {
    let x = value.filter(|v| v.is_number())?.as_f64()?;
    (x.is_finite() && x.fract() == 0.0 && x.abs() <= MAX_SAFE_INTEGER).then_some(x as i64)
}

/// `Number.isFinite(value)` без приведения: только число.
pub fn finite_number(value: Option<&Value>) -> Option<f64> {
    value
        .filter(|v| v.is_number())
        .and_then(Value::as_f64)
        .filter(|x| x.is_finite())
}

/// `Number(value)`; отсутствующее значение (`undefined`) — `NaN`.
pub fn to_number(value: Option<&Value>) -> f64 {
    js::value::to_number(value)
}

/// `String(value)` для `regex.test(value)`: `undefined`, `null` — словами.
pub fn coerced(value: Option<&Value>) -> String {
    js::to_template_string(value)
}

/// `typeof value === "string" ? value.slice(0, max) : ""` (единицы UTF-16).
pub fn text(value: Option<&Value>, max: usize) -> String {
    value
        .and_then(Value::as_str)
        .map_or_else(String::new, |s| js::slice_utf16(s, max))
}

/// `value && typeof value === "object"`: объект или массив.
pub fn is_object(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Object(_) | Value::Array(_)))
}

/// `value[key]` у объекта; у массива и примитива таких полей нет.
pub fn field<'a>(value: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    value
        .and_then(Value::as_object)
        .and_then(|map| map.get(key))
}

/// Ключи `Object.keys(value)` и значения: у объекта — в порядке JS, у массива — индексы.
pub fn entries(value: Option<&Value>) -> Vec<(String, &Value)> {
    match value {
        Some(Value::Object(map)) => hlsr_core::json_text::js_key_order(map)
            .into_iter()
            .map(|(key, value)| (key.clone(), value))
            .collect(),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(index, value)| (index.to_string(), value))
            .collect(),
        _ => Vec::new(),
    }
}
