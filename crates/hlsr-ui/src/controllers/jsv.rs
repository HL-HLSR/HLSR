//! Чтение ответов main как их читает JS рендерера: поле, которого нет, — `undefined`
//! (`None`); `x || 0`; `Math.round`; `String(number)`. Ответы каналов (`DeliveryStatus`,
//! `game-stats-get`, `game-menu-state`, `ToolCard`) приходят значениями JSON — формы 0.4.7.

use hlsr_core::js;
use serde_json::Value;

/// `value.name`: у объекта — поле, у остальных значений (и `null`) — `undefined`.
pub fn prop<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.as_object().and_then(|object| object.get(name))
}

/// `value.name` у необязательного значения (`delivery && delivery.name`).
pub fn field<'a>(value: Option<&'a Value>, name: &str) -> Option<&'a Value> {
    value.and_then(|value| prop(value, name))
}

/// Истинно ли значение в JS (`undefined` — нет).
pub fn truthy(value: Option<&Value>) -> bool {
    value.is_some_and(js::truthy)
}

/// Строка поля; другие значения — `None`.
pub fn str_of(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str)
}

/// `value === expected`.
pub fn is_str(value: Option<&Value>, expected: &str) -> bool {
    str_of(value) == Some(expected)
}

/// Число поля как `f64`; не число — `None`.
pub fn num(value: Option<&Value>) -> Option<f64> {
    value.and_then(js::value::number)
}

/// `value || 0` для числового поля: ложные значения и «не число» — 0.
pub fn or_zero(value: Option<&Value>) -> f64 {
    match value {
        Some(value) if js::truthy(value) => js::value::number(value).unwrap_or(f64::NAN),
        _ => 0.0,
    }
}

/// `Math.round(x)`: половина — к большему.
pub fn round(x: f64) -> f64 {
    if x.is_finite() { (x + 0.5).floor() } else { x }
}

/// `String(x)` для числа: целое — без дробной части.
pub fn number_text(x: f64) -> String {
    js::number::number_to_string(x)
}

/// Число JS как значение JSON: у целого нет дробной части (`JSON.stringify(255)` — `255`,
/// не `255.0`), прочие — как есть; `NaN`/`Infinity` в JSON — `null`.
pub fn number_value(x: f64) -> Value {
    if !x.is_finite() {
        return Value::Null;
    }
    if x.fract() == 0.0 && x.abs() < 9_007_199_254_740_992.0 {
        return Value::from(x as i64);
    }
    Value::from(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_fields_are_undefined_and_or_zero_follows_js() {
        let value = json!({"a": 5, "b": null, "c": 0, "d": "x"});
        assert_eq!(or_zero(prop(&value, "a")), 5.0);
        assert_eq!(or_zero(prop(&value, "b")), 0.0);
        assert_eq!(or_zero(prop(&value, "c")), 0.0);
        assert_eq!(or_zero(prop(&value, "missing")), 0.0);
        assert!(or_zero(prop(&value, "d")).is_nan());
        assert!(!truthy(prop(&value, "b")));
        assert!(truthy(prop(&value, "d")));
        assert_eq!(field(None, "a"), None);
        assert_eq!(prop(&json!(null), "a"), None);
    }

    #[test]
    fn integral_numbers_become_json_integers() {
        assert_eq!(number_value(255.0), json!(255));
        assert_eq!(number_value(-0.0), json!(0));
        assert_eq!(number_value(12.75), json!(12.75));
        assert_eq!(number_value(f64::NAN), Value::Null);
        assert_eq!(serde_json::to_string(&number_value(10.0)).unwrap(), "10");
    }

    #[test]
    fn math_round_rounds_half_up() {
        assert_eq!(round(2.5), 3.0);
        assert_eq!(round(-2.5), -2.0);
        assert_eq!(round(33.4), 33.0);
        assert_eq!(number_text(12.0), "12");
        assert_eq!(number_text(0.5), "0.5");
    }
}
