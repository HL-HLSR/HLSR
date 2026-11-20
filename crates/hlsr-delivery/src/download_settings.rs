//! Лимит скорости загрузок (`DownloadSettings.js` и `src/shared/download-settings.js`):
//! `download-settings.json` `{speedLimit}` в папке данных и общий ограничитель.

use crate::download_rate_limiter::DownloadRateLimiter;
use hlsr_core::json_store::{JsonState, read_json_state, write_state};
use hlsr_core::json_text::JsonFormat;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Допустимые лимиты, МБ/с; 0 — без ограничения.
pub const SPEED_LIMITS: [u32; 8] = [0, 1, 2, 5, 10, 20, 50, 100];

const MIB: f64 = 1024.0 * 1024.0;

/// `validSpeedLimit(value)`: число из списка (`includes`: `-0` — это `0`).
pub fn valid_speed_limit(value: &Value) -> bool {
    speed_limit_of(value).is_some()
}

/// `normalizeSpeedLimit(value)`: значение из списка или 0.
pub fn normalize_speed_limit(value: &Value) -> u32 {
    speed_limit_of(value).unwrap_or(0)
}

fn speed_limit_of(value: &Value) -> Option<u32> {
    let number = value.as_number()?.as_f64()?;
    SPEED_LIMITS
        .into_iter()
        .find(|&limit| f64::from(limit) == number)
}

/// `snapshot()`: `{speedLimit, initialized}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    /// МБ/с, 0 — без ограничения.
    pub speed_limit: u32,
    /// Лимит уже выбран (файл прочитан и значение допустимо).
    pub initialized: bool,
}

impl Snapshot {
    pub fn to_json(&self) -> Value {
        json!({ "speedLimit": self.speed_limit, "initialized": self.initialized })
    }
}

/// Отказ `save`: `{ok: false, code}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SaveError {
    /// Значение не из [`SPEED_LIMITS`].
    #[error("INVALID_SPEED_LIMIT")]
    InvalidSpeedLimit,
    /// Файл есть, но не читается (его не перезаписываем), или запись не удалась.
    #[error("SETTINGS_SAVE_FAILED")]
    SettingsSaveFailed,
}

impl SaveError {
    pub fn code(self) -> &'static str {
        match self {
            SaveError::InvalidSpeedLimit => "INVALID_SPEED_LIMIT",
            SaveError::SettingsSaveFailed => "SETTINGS_SAVE_FAILED",
        }
    }
}

#[derive(Debug)]
pub struct DownloadSettings {
    file: PathBuf,
    speed_limit: u32,
    initialized: bool,
    rate_limiter: Arc<DownloadRateLimiter>,
}

impl DownloadSettings {
    /// Прочитать `file`; лимит сразу действует на ограничитель.
    pub fn new(file: impl AsRef<Path>) -> DownloadSettings {
        let file = file.as_ref().to_path_buf();
        let state = read_json_state(&file);
        let speed_limit = state.data().get("speedLimit").cloned();
        let speed_limit = speed_limit.unwrap_or(Value::Null);
        let initialized = matches!(state, JsonState::Valid(_)) && valid_speed_limit(&speed_limit);
        let speed_limit = normalize_speed_limit(&speed_limit);
        DownloadSettings {
            file,
            speed_limit,
            initialized,
            rate_limiter: Arc::new(DownloadRateLimiter::new(f64::from(speed_limit) * MIB)),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            speed_limit: self.speed_limit,
            initialized: self.initialized,
        }
    }

    /// Общий ограничитель загрузок.
    pub fn rate_limiter(&self) -> &Arc<DownloadRateLimiter> {
        &self.rate_limiter
    }

    /// `save(speedLimit)`: записать `{speedLimit}` (как `writeJsonAtomic`: прочие поля файла
    /// 0.4.7 не сохраняет) и сразу применить.
    pub fn save(&mut self, speed_limit: &Value) -> Result<Snapshot, SaveError> {
        let speed_limit = speed_limit_of(speed_limit).ok_or(SaveError::InvalidSpeedLimit)?;
        // Не перезаписываем файл, который не удалось прочитать.
        if let JsonState::Invalid { .. } = read_json_state(&self.file) {
            return Err(SaveError::SettingsSaveFailed);
        }
        write_state(
            &self.file,
            &json!({ "speedLimit": speed_limit }),
            JsonFormat::Atomic,
        )
        .map_err(|_| SaveError::SettingsSaveFailed)?;
        self.initialized = true;
        self.speed_limit = speed_limit;
        self.rate_limiter.set_limit(f64::from(speed_limit) * MIB);
        Ok(self.snapshot())
    }
}
