//! Ошибки «Демо» (`errors.js`): стабильный код для окна (тексты — в локалях) и необязательные
//! подробности без путей пользователя.

use hlsr_core::js::errno;
use hlsr_delivery::errors::DeliveryError;
use serde_json::{Map, Value, json};

/// `DemoError`: код, текст для журнала и подробности.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct DemoError {
    pub code: String,
    pub message: String,
    pub details: Map<String, Value>,
}

impl DemoError {
    /// `new DemoError(code)`: текст — сам код.
    pub fn new(code: impl Into<String>) -> DemoError {
        let code = code.into();
        DemoError {
            message: code.clone(),
            code,
            details: Map::new(),
        }
    }

    /// Третий аргумент конструктора: объект подробностей (иное значение — без них).
    pub fn with_details(mut self, details: Value) -> DemoError {
        if let Value::Object(map) = details {
            self.details = map;
        }
        self
    }

    /// `publicError` для своей ошибки: `{code, ...details}` (подробности поверх кода, как
    /// `Object.assign`).
    pub fn to_public(&self) -> Value {
        let mut out = Map::new();
        out.insert("code".to_owned(), json!(self.code));
        out.extend(self.details.clone());
        Value::Object(out)
    }
}

/// `publicError(error, fallback)`: своя ошибка — её код и подробности; ошибка ОС — `NO_SPACE`
/// для `ENOSPC`, иначе `fallback`; ошибка доставки — её код, если это код контракта.
pub fn public_error(error: &(dyn std::error::Error + 'static), fallback: Option<&str>) -> Value {
    if let Some(own) = error.downcast_ref::<DemoError>() {
        return own.to_public();
    }
    let code = if let Some(io) = error.downcast_ref::<std::io::Error>() {
        errno::code(io).map(|code| code.into_owned())
    } else {
        error
            .downcast_ref::<DeliveryError>()
            .map(|delivery| delivery.code.as_str().to_owned())
    };
    public_code(code.as_deref(), fallback)
}

/// Код чужой ошибки для окна: errno Node (`ENOSPC`, `EACCES`…) — не код контракта.
pub fn public_code(code: Option<&str>, fallback: Option<&str>) -> Value {
    let code = code.unwrap_or_default();
    if code == "ENOSPC" {
        return json!({"code": "NO_SPACE"});
    }
    if is_contract_code(code) && !is_errno(code) {
        return json!({"code": code});
    }
    json!({"code": fallback.unwrap_or("JOB_FAILED")})
}

/// `/^[A-Z][A-Z0-9_]+$/`.
fn is_contract_code(code: &str) -> bool {
    let bytes = code.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_uppercase()
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
}

/// `/^E[A-Z0-9]+$/`.
fn is_errno(code: &str) -> bool {
    let bytes = code.as_bytes();
    bytes.len() >= 2
        && bytes[0] == b'E'
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// `error.code === "ENOENT"` у Node.
pub(crate) fn is_enoent(error: &std::io::Error) -> bool {
    errno::code(error).as_deref() == Some("ENOENT")
}
