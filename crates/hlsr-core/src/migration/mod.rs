//! Перенос данных 0.4.7 при первом запуске новой версии. Сделанные шаги
//! отмечаются в `migration-slint.json` и больше не повторяются.

pub mod local_storage;

use crate::paths::Paths;
use crate::store::Store;
use serde_json::{Value, json};
use std::time::SystemTime;

pub const MIGRATION_FILE: &str = "migration-slint.json";
pub const STEP_LOCAL_STORAGE: &str = "localStorage";

/// Итог шага переноса.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// Шаг уже был сделан раньше, ничего не делалось.
    pub skipped: bool,
    pub errors: Vec<String>,
}

/// `migration-slint.json`: `{ "<шаг>": { "done": true, "at": "…", "errors": [...] } }`.
#[derive(Debug)]
pub struct MigrationLog {
    store: Store,
}

impl MigrationLog {
    pub fn open(paths: &Paths) -> MigrationLog {
        MigrationLog {
            store: Store::open(paths.file(MIGRATION_FILE), json!({})),
        }
    }

    pub fn is_done(&self, step: &str) -> bool {
        self.store
            .get(step)
            .and_then(|entry| entry.get("done"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// Отметить шаг сделанным. Прочие шаги не трогаются. `false` — файл не записан.
    pub fn record(&mut self, step: &str, errors: &[String], now: SystemTime) -> bool {
        let at = chrono::DateTime::<chrono::Utc>::from(now).format("%Y-%m-%dT%H:%M:%S%.3fZ");
        self.store.set(
            step,
            json!({ "done": true, "at": at.to_string(), "errors": errors }),
        )
    }
}
