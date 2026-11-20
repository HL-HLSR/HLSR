//! Общее у источников (`providers/BaseProvider.js`): поля описания, когда нужна новая
//! ссылка, отказ отдавать соседние файлы.

use crate::errors::{Code, DeliveryError};
use hlsr_core::js;
use serde_json::{Map, Value};

/// Описание источника из манифеста (`{id, type, status, url | publicUrl, priority}`).
#[derive(Debug, Clone)]
pub struct BaseProvider {
    descriptor: Value,
}

impl BaseProvider {
    pub fn new(descriptor: &Value) -> BaseProvider {
        BaseProvider {
            descriptor: descriptor.clone(),
        }
    }

    pub fn descriptor(&self) -> &Value {
        &self.descriptor
    }

    /// `id`.
    pub fn id(&self) -> Option<&str> {
        self.descriptor.get("id").and_then(Value::as_str)
    }

    /// `type`.
    pub fn kind(&self) -> Option<&str> {
        self.descriptor.get("type").and_then(Value::as_str)
    }

    /// `describe()`: `{id, type, status}`; поля, которых нет, в JSON не попадают.
    pub fn describe(&self) -> Value {
        let mut out = Map::new();
        for key in ["id", "type", "status"] {
            if let Some(value) = self.descriptor.get(key) {
                out.insert(key.to_owned(), value.clone());
            }
        }
        Value::Object(out)
    }

    /// `shouldRefreshOn(statusCode)`: 401, 403, 410 — ссылка устарела.
    pub fn should_refresh_on(status: u16) -> bool {
        matches!(status, 401 | 403 | 410)
    }

    /// `resolveSibling()` источника, который соседние файлы не отдаёт.
    pub fn sibling_unsupported(&self) -> DeliveryError {
        DeliveryError::new(
            Code::ProviderUnsupported,
            format!(
                "Провайдер {} не умеет отдавать файлы рядом с архивом",
                js::to_template_string(self.descriptor.get("type"))
            ),
        )
    }
}
