//! Личность установки для телеметрии (`telemetry/identity.js`): файл
//! `telemetry-identity.json` `{schemaVersion, installationId (UUID), createdAt}`, пишется
//! атомарно (два пробела и `\n`, как `writeJsonAtomic`).

use super::event_schema::is_uuid;
use hlsr_core::js::iso::iso_from_ms;
use hlsr_core::js::truthy;
use hlsr_core::json_store::read_json_safe;
use hlsr_delivery::atomic::write_json_atomic;
use hlsr_delivery::errors::RawError;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "telemetry-identity.json";
pub const SCHEMA_VERSION: u32 = 1;

/// Личность: `{installationId, createdAt}`. `created_at` — что записано в файле (`|| null`).
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub installation_id: String,
    pub created_at: Value,
}

pub fn identity_path(user_data_dir: &Path) -> PathBuf {
    user_data_dir.join(FILE_NAME)
}

/// `randomUuid()`: UUID версии 4 строчными буквами.
pub fn random_uuid() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("ОС выдаёт случайные байты");
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hlsr_core::js::text::hex(&bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// `read(userDataDir)`: личность из файла или `None`, если файла нет или он не по схеме.
pub fn read(user_data_dir: &Path) -> Option<Identity> {
    let data = read_json_safe(&identity_path(user_data_dir));
    let schema_ok = data
        .get("schemaVersion")
        .and_then(Value::as_f64)
        .is_some_and(|version| version == f64::from(SCHEMA_VERSION));
    let id = data.get("installationId").and_then(Value::as_str)?;
    if !schema_ok || !is_uuid(id) {
        return None;
    }
    let created_at = data.get("createdAt").filter(|value| truthy(value));
    Some(Identity {
        installation_id: id.to_owned(),
        created_at: created_at.cloned().unwrap_or(Value::Null),
    })
}

/// `ensure(userDataDir)`: личность из файла или новая, записанная в него; `now_ms` — `Date.now()`.
pub fn ensure(user_data_dir: &Path, now_ms: i64) -> Result<Identity, RawError> {
    ensure_with(user_data_dir, now_ms, random_uuid)
}

/// [`ensure`] со своим источником UUID (тесты сверки подставляют последовательность).
pub fn ensure_with(
    user_data_dir: &Path,
    now_ms: i64,
    new_uuid: impl FnOnce() -> String,
) -> Result<Identity, RawError> {
    if let Some(existing) = read(user_data_dir) {
        return Ok(existing);
    }
    let installation_id = new_uuid();
    let created_at = iso_from_ms(now_ms);
    let identity = json!({
        "schemaVersion": SCHEMA_VERSION,
        "installationId": installation_id,
        "createdAt": created_at,
    });
    write_json_atomic(&identity_path(user_data_dir), &identity)?;
    Ok(Identity {
        installation_id,
        created_at: Value::String(created_at),
    })
}

/// `reset(userDataDir)`: удалить файл; `true`, если он был и удалён.
pub fn reset(user_data_dir: &Path) -> bool {
    let file = identity_path(user_data_dir);
    file.exists() && std::fs::remove_file(&file).is_ok()
}
