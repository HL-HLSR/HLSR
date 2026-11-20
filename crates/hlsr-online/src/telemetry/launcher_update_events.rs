//! События самообновления лаунчера (`telemetry/launcher-update-events.js`): каждая стадия — один
//! раз на версию. Отметка `TM_LU_*:<версия>` хранится в `version-state.json` (`VersionEvents`),
//! поэтому событие не повторяется и после перезапуска. Вызывает обновлялка (подпроект 6) и
//! старт приложения (квитанция обновления).

use super::event_schema::{ERROR_CODES, is_version};
use hlsr_core::json_text::js_number;
use hlsr_delivery::ports::Logger;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// `STAGE`: ключи отметок в `version-state.json`.
pub mod stage {
    pub const AVAILABLE: &str = "TM_LU_AVAILABLE";
    pub const DOWNLOAD_STARTED: &str = "TM_LU_DL_STARTED";
    pub const DOWNLOAD_COMPLETED: &str = "TM_LU_DL_DONE";
    pub const INSTALLED: &str = "TM_LU_INSTALLED";
    pub const FAILED: &str = "TM_LU_FAILED";
}

/// `FAILURE_CODES`: код обновлялки → код телеметрии.
pub const FAILURE_CODES: [(&str, &str); 10] = [
    ("LU_NO_FILE", "MANIFEST_INVALID"),
    ("LU_VERIFY_FAILED", "CHECKSUM_MISMATCH"),
    ("LU_SIZE_MISMATCH", "CHECKSUM_MISMATCH"),
    ("LU_SHA256_MISMATCH", "CHECKSUM_MISMATCH"),
    ("LU_SHA512_MISMATCH", "CHECKSUM_MISMATCH"),
    ("LU_SIGNATURE_INVALID", "SIGNATURE_INVALID"),
    ("LU_SCHEMA_INVALID", "MANIFEST_INVALID"),
    ("LU_DOWNGRADE", "MANIFEST_INVALID"),
    ("LU_VERSION_MISMATCH", "MANIFEST_INVALID"),
    ("LAUNCHER_UPDATE_ERROR", "UNKNOWN_SAFE"),
];

/// Куда уходят события: `telemetry.enabled()` и `telemetry.emit(type, fields)` (`true` — событие
/// встало в очередь).
pub trait TelemetrySink: Send + Sync {
    fn enabled(&self) -> bool;
    fn emit(&self, event_type: &str, fields: Value) -> bool;
}

/// Отметки «уже объявлено» (`VersionEvents.alreadyAnnounced`/`markAnnounced`); ошибка — текст
/// исключения.
pub trait AnnouncedVersions: Send + Sync {
    fn already_announced(&self, stage: &str, version: &str) -> Result<bool, String>;
    fn mark_announced(&self, stage: &str, version: &str) -> Result<(), String>;
}

/// Переданные байты загрузки: `{bytes, durationMs}`; не число — `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Transfer {
    pub bytes: Option<f64>,
    pub duration_ms: Option<f64>,
}

/// `byteCount(value)`: целое ≥ 0 или `null` (неизвестно — не 0).
pub fn byte_count(value: Option<f64>) -> Option<f64> {
    value.filter(|x| x.is_finite() && x.fract() == 0.0 && *x >= 0.0)
}

pub struct LauncherUpdateEvents {
    telemetry: Option<Arc<dyn TelemetrySink>>,
    version_events: Option<Arc<dyn AnnouncedVersions>>,
    logger: Arc<dyn Logger>,
    started_versions: Mutex<HashSet<Option<String>>>,
}

impl LauncherUpdateEvents {
    pub fn new(
        telemetry: Option<Arc<dyn TelemetrySink>>,
        version_events: Option<Arc<dyn AnnouncedVersions>>,
        logger: Arc<dyn Logger>,
    ) -> LauncherUpdateEvents {
        LauncherUpdateEvents {
            telemetry,
            version_events,
            logger,
            started_versions: Mutex::default(),
        }
    }

    /// `once(stage, version, type, fields)`: событие, если телеметрия включена, версия верна и
    /// стадия для неё ещё не объявлялась; после постановки в очередь — отметка.
    fn once(&self, stage: &str, version: Option<&str>, event_type: &str, fields: Value) -> bool {
        let Some(telemetry) = &self.telemetry else {
            return false;
        };
        if !telemetry.enabled() {
            return false;
        }
        let Some(version) = version.filter(|version| is_version(version)) else {
            return false;
        };
        if let Some(events) = &self.version_events {
            match events.already_announced(stage, version) {
                Ok(false) => {}
                Ok(true) | Err(_) => return false,
            }
        }
        let queued = telemetry.emit(event_type, fields);
        if queued
            && let Some(events) = &self.version_events
            && let Err(message) = events.mark_announced(stage, version)
        {
            self.logger.warn(&format!(
                "[telemetry] стадия {stage} не отмечена: {message}"
            ));
        }
        queued
    }

    /// `available(currentVersion, nextVersion)`.
    pub fn available(&self, current_version: &str, next_version: Option<&str>) -> bool {
        self.once(
            stage::AVAILABLE,
            next_version,
            "launcher_update_available",
            json!({"fromVersion": current_version, "toVersion": next_version}),
        )
    }

    /// `downloadStarted(currentVersion, nextVersion)`: в одном запуске — один раз на версию, даже
    /// если телеметрия выключена.
    pub fn download_started(&self, current_version: &str, next_version: Option<&str>) -> bool {
        let first = self
            .started_versions
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(next_version.map(str::to_owned));
        if !first {
            return false;
        }
        self.once(
            stage::DOWNLOAD_STARTED,
            next_version,
            "launcher_download_started",
            json!({"fromVersion": current_version, "toVersion": next_version}),
        )
    }

    /// `downloadCompleted(currentVersion, nextVersion, transfer)`: полная загрузка и пройденная
    /// проверка установщика. Неизвестный размер уходит как `null`, не 0.
    pub fn download_completed(
        &self,
        current_version: &str,
        next_version: Option<&str>,
        transfer: Option<Transfer>,
    ) -> bool {
        let transfer = transfer.unwrap_or_default();
        let bytes = byte_count(transfer.bytes);
        let duration = byte_count(transfer.duration_ms);
        let average = match (bytes, duration) {
            (Some(bytes), Some(duration)) if duration > 0.0 => {
                js_number((bytes * 1000.0 / duration).floor())
            }
            _ => Value::Null,
        };
        let number = |x: Option<f64>| x.map_or(Value::Null, js_number);
        self.once(
            stage::DOWNLOAD_COMPLETED,
            next_version,
            "launcher_download_completed",
            json!({
                "fromVersion": current_version,
                "toVersion": next_version,
                "downloadBytes": number(bytes),
                "downloadDurationMilliseconds": number(duration),
                "averageBytesPerSecond": average,
            }),
        )
    }

    /// `installed(fromVersion, toVersion)`.
    pub fn installed(&self, from_version: &str, to_version: Option<&str>) -> bool {
        self.once(
            stage::INSTALLED,
            to_version,
            "launcher_update_installed",
            json!({"fromVersion": from_version, "toVersion": to_version}),
        )
    }

    /// `failed(currentVersion, nextVersion, code)`: неверная следующая версия заменяется текущей.
    pub fn failed(
        &self,
        current_version: &str,
        next_version: Option<&str>,
        code: Option<&str>,
    ) -> bool {
        let mapped = code
            .and_then(|code| FAILURE_CODES.iter().find(|(key, _)| *key == code))
            .map_or("UNKNOWN_SAFE", |(_, mapped)| *mapped);
        let error_code = if ERROR_CODES.contains(&mapped) {
            mapped
        } else {
            "UNKNOWN_SAFE"
        };
        let version = next_version
            .filter(|next| is_version(next))
            .unwrap_or(current_version);
        self.once(
            stage::FAILED,
            Some(version),
            "launcher_update_failed",
            json!({"fromVersion": current_version, "toVersion": version, "errorCode": error_code}),
        )
    }
}
