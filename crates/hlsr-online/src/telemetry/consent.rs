//! Согласие на телеметрию (`telemetry/consent.js`): файл `telemetry.json`
//! `{schemaVersion, enabled, source, updatedAt}`, по умолчанию выключено. Чтение понимает
//! UTF-16LE с меткой порядка байтов и без неё (файл, сохранённый Блокнотом Windows) и срезает
//! метку UTF-8; запись — прямая, без временного файла, JSON с отступом в два пробела и без
//! перевода строки в конце.

use super::config;
use crate::fs_error::raw_error;
use hlsr_core::js::iso::iso_from_ms;
use hlsr_core::js::{fs as js_fs, utf8_string};
use hlsr_core::json_formats::write_pretty2;
use hlsr_core::json_text::json_parse;
use hlsr_delivery::errors::RawError;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = config::SCHEMA_VERSION;
pub const CONSENT_FILE: &str = "telemetry.json";
pub const BACKEND_CONFIGURED: bool = config::BACKEND_CONFIGURED;
pub const DEFAULT_ENABLED: bool = false;

/// `SOURCES`: откуда решение.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Default,
    User,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Default => "default",
            Source::User => "user",
        }
    }
}

/// Прочитанное или записанное согласие: `{enabled, source}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Consent {
    pub enabled: bool,
    pub source: Source,
}

pub fn consent_path(user_data_dir: &Path) -> PathBuf {
    user_data_dir.join(CONSENT_FILE)
}

/// `decode(buffer)`: текст файла. `FF FE` в начале — UTF-16LE без метки; второй байт 0 —
/// UTF-16LE без метки; иначе UTF-8 без метки `U+FEFF`.
pub fn decode(buffer: &[u8]) -> String {
    if buffer.len() >= 2 && buffer[0] == 0xff && buffer[1] == 0xfe {
        return utf16le(&buffer[2..]);
    }
    if buffer.len() >= 2 && buffer[1] == 0x00 {
        return utf16le(buffer);
    }
    let text = utf8_string(buffer);
    text.strip_prefix('\u{FEFF}').unwrap_or(&text).to_owned()
}

/// `buffer.toString("utf16le")`: нечётный байт в конце отбрасывается.
fn utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    String::from_utf16_lossy(&units)
}

/// `read(userDataDir)`: решение пользователя, если файл — объект с логическим `enabled`;
/// иначе умолчание.
pub fn read(user_data_dir: &Path) -> Consent {
    let enabled = js_fs::read_file(&consent_path(user_data_dir))
        .ok()
        .and_then(|bytes| json_parse(&decode(&bytes)).ok())
        .and_then(|data| data.get("enabled").and_then(Value::as_bool));
    match enabled {
        Some(enabled) => Consent {
            enabled,
            source: Source::User,
        },
        None => Consent {
            enabled: DEFAULT_ENABLED,
            source: Source::Default,
        },
    }
}

/// `write(userDataDir, enabled, source)`; `now_ms` — `Date.now()` для `updatedAt`.
pub fn write(
    user_data_dir: &Path,
    enabled: bool,
    source: Source,
    now_ms: i64,
) -> Result<Consent, RawError> {
    let state = json!({
        "schemaVersion": SCHEMA_VERSION,
        "enabled": enabled,
        "source": source.as_str(),
        "updatedAt": iso_from_ms(now_ms),
    });
    write_pretty2(&consent_path(user_data_dir), &state).map_err(|failure| raw_error(&failure))?;
    Ok(Consent { enabled, source })
}

/// `set(userDataDir, enabled)`: решение пользователя.
pub fn set(user_data_dir: &Path, enabled: bool, now_ms: i64) -> Result<Consent, RawError> {
    write(user_data_dir, enabled, Source::User, now_ms)
}

/// `canSend(userDataDir)`: бэкенд настроен и согласие дано.
pub fn can_send(user_data_dir: &Path) -> bool {
    BACKEND_CONFIGURED && read(user_data_dir).enabled
}
