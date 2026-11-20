//! `discord-rpc/preference.js`: включено ли присутствие — `discord-rpc.json` в папке данных.
//! Перенос старого `config.rpc` из `settings.json` (`migrate`) и переход на «включено по
//! умолчанию» схемы 2 (`adoptDefault`). Сбой записи — исключение Node ([`RawError`]).

use hlsr_core::js::iso::iso_from_ms;
use hlsr_core::json_store::read_json_safe;
use hlsr_core::json_text::{js_number, json_parse};
use hlsr_delivery::atomic::write_json_atomic;
use hlsr_delivery::errors::RawError;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: i64 = 2;
pub const FILE: &str = "discord-rpc.json";
pub const DEFAULT_ENABLED: bool = true;
pub const LEGACY_FILE: &str = "settings.json";
pub const LEGACY_KEY: &str = "rpc";

/// `SOURCES`: откуда значение.
pub mod sources {
    pub const DEFAULT: &str = "default";
    pub const USER: &str = "user";
    pub const MIGRATED: &str = "migrated";
}

/// `filePath(userDataDir)`.
pub fn file_path(user_data_dir: &Path) -> PathBuf {
    user_data_dir.join(FILE)
}

/// Настройка, как её возвращает `read`.
#[derive(Debug, Clone, PartialEq)]
pub struct Preference {
    pub enabled: bool,
    pub source: String,
    pub migrated: bool,
    /// Старое `config.rpc`, если было.
    pub legacy_value: Option<bool>,
    /// Целое число JS из файла; без файла — 0, не целое — 1.
    pub schema_version: f64,
    /// Прочитано из файла: тогда у объекта 0.4.7 есть `legacyValue` (`null` или булево).
    pub stored: bool,
}

impl Preference {
    /// Объект `read` 0.4.7.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("enabled".into(), json!(self.enabled));
        out.insert("source".into(), json!(self.source));
        out.insert("migrated".into(), json!(self.migrated));
        if self.stored {
            out.insert("legacyValue".into(), json!(self.legacy_value));
        }
        out.insert("schemaVersion".into(), js_number(self.schema_version));
        Value::Object(out)
    }
}

/// `read(userDataDir)`: файла нет, он битый или `enabled` не булево — включено по умолчанию.
pub fn read(user_data_dir: &Path) -> Preference {
    let data = read_json_safe(&file_path(user_data_dir));
    let Some(enabled) = data.get("enabled").and_then(Value::as_bool) else {
        return Preference {
            enabled: DEFAULT_ENABLED,
            source: sources::DEFAULT.to_owned(),
            migrated: false,
            legacy_value: None,
            schema_version: 0.0,
            stored: false,
        };
    };
    let schema_version = data
        .get("schemaVersion")
        .and_then(Value::as_f64)
        .filter(|x| x.is_finite() && x.fract() == 0.0)
        .unwrap_or(1.0);
    Preference {
        enabled,
        source: data
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or(sources::USER)
            .to_owned(),
        migrated: data.get("migrated") == Some(&Value::Bool(true)),
        legacy_value: data.get("legacyValue").and_then(Value::as_bool),
        // `-0` в JSON — 0.
        schema_version: schema_version + 0.0,
        stored: true,
    }
}

/// `write(userDataDir, state)`: схема 2, время записи; пустой источник — `user`.
fn write(
    user_data_dir: &Path,
    enabled: bool,
    source: &str,
    migrated: bool,
    legacy_value: Option<bool>,
    now_ms: i64,
) -> Result<Preference, RawError> {
    let mut next = Map::new();
    next.insert("schemaVersion".into(), json!(SCHEMA_VERSION));
    next.insert("enabled".into(), json!(enabled));
    let source = if source.is_empty() {
        sources::USER
    } else {
        source
    };
    next.insert("source".into(), json!(source));
    next.insert("migrated".into(), json!(migrated));
    next.insert("updatedAt".into(), json!(iso_from_ms(now_ms)));
    if let Some(legacy) = legacy_value {
        next.insert("legacyValue".into(), json!(legacy));
    }
    write_json_atomic(&file_path(user_data_dir), &Value::Object(next))?;
    Ok(read(user_data_dir))
}

/// `set(userDataDir, enabled)`: выбор пользователя.
pub fn set(user_data_dir: &Path, enabled: bool, now_ms: i64) -> Result<Preference, RawError> {
    let current = read(user_data_dir);
    write(
        user_data_dir,
        enabled,
        sources::USER,
        current.migrated,
        current.legacy_value,
        now_ms,
    )
}

/// `reset(userDataDir)`: значение по умолчанию.
pub fn reset(user_data_dir: &Path, now_ms: i64) -> Result<Preference, RawError> {
    write(
        user_data_dir,
        DEFAULT_ENABLED,
        sources::DEFAULT,
        true,
        None,
        now_ms,
    )
}

/// `adoptDefault(userDataDir, known)`: до схемы 2 выключенное без выбора пользователя (и без
/// старого `false`) становится включённым.
pub fn adopt_default(
    user_data_dir: &Path,
    known: Option<Preference>,
    now_ms: i64,
) -> Result<Preference, RawError> {
    let current = known.unwrap_or_else(|| read(user_data_dir));
    if current.schema_version >= SCHEMA_VERSION as f64 {
        return Ok(current);
    }
    let chosen = current.source == sources::USER || current.legacy_value == Some(false);
    write(
        user_data_dir,
        if chosen {
            current.enabled
        } else {
            DEFAULT_ENABLED
        },
        &current.source,
        current.migrated,
        current.legacy_value,
        now_ms,
    )
}

/// Старое `config.rpc` из `settings.json`: `JSON.parse` без среза BOM, только булево.
fn legacy_value(user_data_dir: &Path) -> Option<bool> {
    let bytes = std::fs::read(user_data_dir.join(LEGACY_FILE)).ok()?;
    let parsed = json_parse(&String::from_utf8_lossy(&bytes)).ok()?;
    parsed.get("config")?.get(LEGACY_KEY)?.as_bool()
}

/// `migrate(userDataDir)`: однажды перенести старое `config.rpc`; уже перенесённое — только
/// [`adopt_default`].
pub fn migrate(user_data_dir: &Path, now_ms: i64) -> Result<Preference, RawError> {
    let current = read(user_data_dir);
    if current.migrated {
        return adopt_default(user_data_dir, Some(current), now_ms);
    }
    let legacy = legacy_value(user_data_dir);
    write(
        user_data_dir,
        legacy != Some(false),
        if legacy.is_none() {
            sources::DEFAULT
        } else {
            sources::MIGRATED
        },
        true,
        legacy,
        now_ms,
    )
}
