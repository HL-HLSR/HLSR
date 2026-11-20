//! Чтение документов JSON так, как читает их JS: свойство, которого нет, — `undefined`;
//! значение в шаблонной строке; `Number.isInteger`; проверка `/^[a-f0-9]{64}$/.test(x || "")`;
//! текст `TypeError` при чтении свойства у `null`. Нужен модулям манифестов, где JS работает
//! с произвольным значением, а не с типизированной записью.

use crate::errors::RawError;
use hlsr_core::js;
use serde_json::Value;

/// `value.name`: у объекта — поле, у остальных значений — `undefined` (`None`).
pub fn prop<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.as_object().and_then(|object| object.get(name))
}

/// Значение в шаблонной строке `${value}`.
pub fn template(value: Option<&Value>) -> String {
    js::to_template_string(value)
}

/// Истинно ли значение (`undefined` — нет).
pub fn truthy(value: Option<&Value>) -> bool {
    value.is_some_and(js::truthy)
}

/// Число из значения JSON; за пределами `f64` (`1e999`) — бесконечность того же знака.
pub fn number(value: &Value) -> Option<f64> {
    js::value::number(value)
}

/// `Number.isInteger(value)`.
pub fn is_integer(value: Option<&Value>) -> bool {
    value
        .and_then(number)
        .is_some_and(|n| n.is_finite() && n.fract() == 0.0)
}

/// Значение как число для сравнения `x < 0`: нечисла в этих проверках не бывает.
pub fn as_number(value: Option<&Value>) -> f64 {
    value.and_then(number).unwrap_or(f64::NAN)
}

/// `/^[a-f0-9]{64}$/.test(value || "")`: значение приводится к строке (`["…"]` — к своему
/// единственному элементу); `$` не принимает `\n` в конце.
pub fn is_hex64(value: Option<&Value>) -> bool {
    let text = value
        .filter(|value| js::truthy(value))
        .map(js::to_string)
        .unwrap_or_default();
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `value === expected` для строки.
pub fn is_str(value: Option<&Value>, expected: &str) -> bool {
    value.and_then(Value::as_str) == Some(expected)
}

/// `TypeError` V8 при чтении свойства: `Cannot read properties of null (reading 'path')`.
/// `receiver` — `None` для `undefined`.
pub fn read_error(receiver: Option<&Value>, name: &str) -> RawError {
    let what = match receiver {
        None => "undefined",
        Some(_) => "null",
    };
    RawError::new(format!(
        "Cannot read properties of {what} (reading '{name}')"
    ))
}

/// `Array.isArray(value)` и поле `files`/`deletedFiles`: список или `None`.
pub fn array(value: Option<&Value>) -> Option<&Vec<Value>> {
    value.and_then(Value::as_array)
}

/// `Number(value)` для значения JSON: число как есть, булево — 0 или 1, `null` — 0, строка и
/// массив — по тексту `String(value)` (`Number("")` и `Number([])` — 0, `Number("0x10")` — 16),
/// объект — `NaN`.
pub fn to_number(value: &Value) -> f64 {
    js::value::to_number(Some(value))
}
