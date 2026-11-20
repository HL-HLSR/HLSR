//! Числа из значений JSON так, как их приводит JS: `Number(value)` и числовой литерал за пределами
//! `f64`.

use serde_json::Value;

/// Число JSON как `f64`; за пределами диапазона (`1e999`) JS читает бесконечность того же знака.
pub fn number(value: &Value) -> Option<f64> {
    value.as_number().map(|n| {
        n.as_f64().unwrap_or_else(|| {
            if n.to_string().starts_with('-') {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }
        })
    })
}

/// `Number(value)` для значения JSON; отсутствующее (`undefined`) — `NaN`, `null` — 0, булево —
/// 0 или 1, строка и массив — по тексту `String(value)` (`Number("")` и `Number([])` — 0,
/// `Number("0x10")` — 16), объект — `NaN`.
pub fn to_number(value: Option<&Value>) -> f64 {
    match value {
        None | Some(Value::Object(_)) => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(number_value @ Value::Number(_)) => number(number_value).unwrap_or(f64::NAN),
        Some(Value::String(text)) => super::to_number(text),
        Some(array @ Value::Array(_)) => super::to_number(&super::to_string(array)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_beyond_f64_are_infinite() {
        let huge: Value = serde_json::from_str("1e999").unwrap();
        let tiny: Value = serde_json::from_str("-1e999").unwrap();
        assert_eq!(number(&huge), Some(f64::INFINITY));
        assert_eq!(number(&tiny), Some(f64::NEG_INFINITY));
        assert_eq!(number(&json!(1.5)), Some(1.5));
        assert_eq!(number(&json!("1")), None);
        assert_eq!(to_number(Some(&huge)), f64::INFINITY);
    }

    #[test]
    fn to_number_follows_js_number() {
        assert!(to_number(None).is_nan());
        assert!(to_number(Some(&json!({}))).is_nan());
        assert_eq!(to_number(Some(&json!(null))), 0.0);
        assert_eq!(to_number(Some(&json!(true))), 1.0);
        assert_eq!(to_number(Some(&json!(false))), 0.0);
        assert_eq!(to_number(Some(&json!(7))), 7.0);
        assert_eq!(to_number(Some(&json!(""))), 0.0);
        assert_eq!(to_number(Some(&json!(" 12 "))), 12.0);
        assert_eq!(to_number(Some(&json!("0x10"))), 16.0);
        assert!(to_number(Some(&json!("12px"))).is_nan());
        assert_eq!(to_number(Some(&json!([]))), 0.0);
        assert_eq!(to_number(Some(&json!(["5"]))), 5.0);
        assert!(to_number(Some(&json!([1, 2]))).is_nan());
    }
}
