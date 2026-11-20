//! Умолчания хранилищ из `StoreDefaults.js` 0.4.7 (`tools/reference/export-defaults.js`).

use serde_json::{Map, Value};
use std::sync::OnceLock;

static DEFAULTS_JSON: &str = include_str!("defaults.json");

/// Умолчания хранилища `library`, `settings` или `scripts`; другое имя → `{}`.
pub fn defaults(config_name: &str) -> Value {
    static ALL: OnceLock<Value> = OnceLock::new();
    let all = ALL.get_or_init(|| {
        serde_json::from_str(DEFAULTS_JSON).expect("defaults.json из 0.4.7 — корректный JSON")
    });
    all.get(config_name)
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()))
}
