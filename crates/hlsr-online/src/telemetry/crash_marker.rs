//! Маркер сессии лаунчера для обнаружения сбоя (`telemetry/crash-marker.js`): файл
//! `telemetry-session-marker.json` `{schemaVersion, sessionId, openedAt, touchedAt}`, пишется
//! атомарно. При чистом выходе маркер удаляют; если при следующем старте он есть и моложе
//! семи дней — прошлая сессия оборвалась (`launcher_crash_detected`).

use hlsr_core::js::iso::iso_from_ms;
use hlsr_core::js::{date_parse, to_string, truthy};
use hlsr_core::json_store::read_json_safe;
use hlsr_delivery::atomic::write_json_atomic;
use hlsr_delivery::errors::RawError;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "telemetry-session-marker.json";
pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_AGE_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// Оборвавшаяся сессия: `{sessionId, lastAliveAt}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashedSession {
    pub session_id: String,
    pub last_alive_at: String,
}

pub fn marker_path(user_data_dir: &Path) -> PathBuf {
    user_data_dir.join(FILE_NAME)
}

/// `nowMs || Date.now()`: время не задано или 0 — текущее.
fn now_or(now_ms: Option<i64>) -> i64 {
    now_ms
        .filter(|&ms| ms != 0)
        .unwrap_or_else(|| hlsr_delivery::timing::system_clock()())
}

/// `open(userDataDir, sessionId, nowMs)`: новый маркер.
pub fn open(user_data_dir: &Path, session_id: &str, now_ms: Option<i64>) -> Result<(), RawError> {
    let now = iso_from_ms(now_or(now_ms));
    write_json_atomic(
        &marker_path(user_data_dir),
        &json!({
            "schemaVersion": SCHEMA_VERSION,
            "sessionId": session_id,
            "openedAt": now,
            "touchedAt": now,
        }),
    )
}

/// `touch(userDataDir, nowMs)`: обновить `touchedAt`, остальные поля файла остаются. Нет
/// маркера или он не объект — ничего; сбой записи глотается.
pub fn touch(user_data_dir: &Path, now_ms: Option<i64>) {
    let mut current = read_json_safe(&marker_path(user_data_dir));
    match &mut current {
        Value::Object(map) => {
            map.insert(
                "touchedAt".to_owned(),
                Value::String(iso_from_ms(now_or(now_ms))),
            );
        }
        // Массив — тоже объект JS: свойство на нём в `JSON.stringify` не попадает.
        Value::Array(_) => {}
        _ => return,
    }
    let _ = write_json_atomic(&marker_path(user_data_dir), &current);
}

/// `close(userDataDir)`: удалить маркер; `false`, если не удалось.
pub fn close(user_data_dir: &Path) -> bool {
    let file = marker_path(user_data_dir);
    !file.exists() || std::fs::remove_file(&file).is_ok()
}

/// `detect(userDataDir, nowMs)`: оборвавшаяся прошлая сессия или `None`. Маркер, который
/// не по схеме, без времени или старше семи дней, удаляется.
pub fn detect(user_data_dir: &Path, now_ms: Option<i64>) -> Option<CrashedSession> {
    let data = read_json_safe(&marker_path(user_data_dir));
    let session_id = data
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|_| {
            data.get("schemaVersion")
                .and_then(Value::as_f64)
                .is_some_and(|version| version == f64::from(SCHEMA_VERSION))
        })
        .map(str::to_owned);
    let Some(session_id) = session_id else {
        close(user_data_dir);
        return None;
    };
    // `Date.parse(data.touchedAt || data.openedAt || "")`: значение любого типа — через String().
    let stamp = ["touchedAt", "openedAt"]
        .iter()
        .filter_map(|key| data.get(key))
        .find(|value| truthy(value))
        .map(to_string)
        .unwrap_or_default();
    let Some(touched_at) = date_parse(&stamp) else {
        close(user_data_dir);
        return None;
    };
    if now_or(now_ms) - touched_at > MAX_AGE_MS {
        close(user_data_dir);
        return None;
    }
    Some(CrashedSession {
        session_id,
        last_alive_at: iso_from_ms(touched_at),
    })
}

/// `consume(userDataDir)`: то же, что [`close`].
pub fn consume(user_data_dir: &Path) -> bool {
    close(user_data_dir)
}
