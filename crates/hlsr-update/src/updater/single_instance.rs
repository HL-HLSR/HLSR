//! Один апдейтер за раз (`updater/core/single-instance.js`): `%LOCALAPPDATA%\HLSR\updater.lock`
//! с `{"pid":…,"ts":…}`. Блокировку живого процесса не трогают (`UPD_ALREADY_RUNNING`), мёртвого
//! — перехватывают; снимает блокировку только её владелец.

use crate::errors::{UpdCode, UpdaterError};
use hlsr_core::js;
use hlsr_core::json_text::json_parse;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// `LOCK_NAME`.
pub const LOCK_NAME: &str = "updater.lock";

/// Взятая блокировка.
#[derive(Debug)]
pub struct InstanceLock {
    pub file: PathBuf,
    pid: u32,
}

impl InstanceLock {
    /// `release()`: файл удаляется, только если в нём всё ещё наш PID.
    pub fn release(&self) {
        let ours = std::fs::read_to_string(&self.file)
            .ok()
            .and_then(|text| json_parse(&text).ok())
            .is_some_and(|raw| raw.get("pid").and_then(Value::as_f64) == Some(f64::from(self.pid)));
        if ours {
            let _ = std::fs::remove_file(&self.file);
        }
    }
}

/// `Number(existing.pid)` для разобранного файла: у не-объекта поля нет (`NaN`).
fn number_of_pid(existing: &Value) -> f64 {
    match existing.as_object().and_then(|object| object.get("pid")) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(text)) => js::to_number(text),
        Some(Value::Bool(flag)) => f64::from(u8::from(*flag)),
        Some(Value::Null) => 0.0,
        _ => f64::NAN,
    }
}

/// `acquire(dir, {pid, isProcessAlive})`; `now_ms` — `Date.now()` для `ts`.
pub fn acquire(
    dir: &Path,
    pid: u32,
    now_ms: i64,
    is_alive: &dyn Fn(f64) -> bool,
) -> Result<InstanceLock, UpdaterError> {
    let file = dir.join(LOCK_NAME);
    let fs_error = |error: std::io::Error, syscall: &str, path: &Path| {
        UpdaterError::new(
            UpdCode::InstallFailed,
            js::errno::fs_message(&error, syscall, &path.to_string_lossy(), None),
        )
    };
    std::fs::create_dir_all(dir).map_err(|error| fs_error(error, "mkdir", dir))?;

    let take_over = || -> Result<InstanceLock, UpdaterError> {
        let text = serde_json::to_string(&json!({"pid": pid, "ts": now_ms}))
            .expect("число в JSON записывается всегда");
        std::fs::write(&file, text).map_err(|error| fs_error(error, "open", &file))?;
        Ok(InstanceLock {
            file: file.clone(),
            pid,
        })
    };

    let existing = std::fs::read_to_string(&file)
        .ok()
        .and_then(|text| json_parse(&text).ok());
    let Some(existing) = existing else {
        return take_over();
    };
    let other = number_of_pid(&existing);
    // `!otherPid || otherPid === pid`: 0 и NaN — ложь.
    if other == 0.0 || other.is_nan() || other == f64::from(pid) {
        return take_over();
    }
    if is_alive(other) {
        let shown = hlsr_core::json_text::js_number(other);
        return Err(UpdaterError::new(
            UpdCode::AlreadyRunning,
            format!("обновление уже выполняется (PID {shown})"),
        )
        .with_details(json!({"pid": shown})));
    }
    take_over()
}
