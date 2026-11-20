//! Квитанция установки (`telemetry/install-receipt.js`): установщик кладёт рядом с `HLSR.exe`
//! файл `.hlsr-install-owner.json` с `installId` (`{GUID}`), лаунчер сверяет его с
//! `telemetry-install-state.json` и один раз на установку сообщает `launcher_install_completed`.
//! Без этой сверки событие уходило бы при каждом запуске.

use hlsr_core::js::iso::iso_from_ms;
use hlsr_core::json_store::read_json_safe;
use hlsr_delivery::atomic::write_json_atomic;
use hlsr_delivery::errors::RawError;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub const MARKER_NAME: &str = ".hlsr-install-owner.json";
pub const STATE_FILE: &str = "telemetry-install-state.json";
pub const SCHEMA_VERSION: u32 = 1;

/// Записанное состояние: `{installId, reportedAt}`.
#[derive(Debug, Clone, PartialEq)]
pub struct InstallState {
    pub install_id: Option<String>,
    /// `data.reportedAt || null`.
    pub reported_at: Value,
}

/// Итог [`detect_install`]: `{report, installId, reinstall}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallCheck {
    pub report: bool,
    pub install_id: Option<String>,
    pub reinstall: bool,
}

pub fn state_path(user_data_dir: &Path) -> PathBuf {
    user_data_dir.join(STATE_FILE)
}

/// `INSTALL_ID_RE`: `{` и 36 знаков из `0-9A-Fa-f-` и `}`.
fn is_install_id(text: &str) -> bool {
    text.len() == 38
        && text.starts_with('{')
        && text.ends_with('}')
        && text[1..37]
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// `readMarker(installDir)`: `installId` установщика прописными буквами.
pub fn read_marker(install_dir: Option<&Path>) -> Option<String> {
    let install_dir = install_dir.filter(|dir| !dir.as_os_str().is_empty())?;
    let data = read_json_safe(&install_dir.join(MARKER_NAME));
    let id = data.get("installId").and_then(Value::as_str)?;
    is_install_id(id).then(|| id.to_ascii_uppercase())
}

/// `readState(userDataDir)`: состояние или `None`, если файла нет или он не по схеме.
pub fn read_state(user_data_dir: &Path) -> Option<InstallState> {
    let data = read_json_safe(&state_path(user_data_dir));
    let schema_ok = data
        .get("schemaVersion")
        .and_then(Value::as_f64)
        .is_some_and(|version| version == f64::from(SCHEMA_VERSION));
    if !schema_ok {
        return None;
    }
    Some(InstallState {
        install_id: data
            .get("installId")
            .and_then(Value::as_str)
            .map(str::to_owned),
        reported_at: data
            .get("reportedAt")
            .filter(|value| hlsr_core::js::truthy(value))
            .cloned()
            .unwrap_or(Value::Null),
    })
}

/// `writeState(userDataDir, installId)`; `now_ms` — `Date.now()` для `reportedAt`.
fn write_state(user_data_dir: &Path, install_id: &str, now_ms: i64) -> Result<(), RawError> {
    write_json_atomic(
        &state_path(user_data_dir),
        &json!({
            "schemaVersion": SCHEMA_VERSION,
            "installId": install_id,
            "reportedAt": iso_from_ms(now_ms),
        }),
    )
}

/// `detectInstall(userDataDir, installDir)`: нужно ли сообщить об установке. Другой
/// `installId`, чем записан, — новая установка (`reinstall`, если запись была).
pub fn detect_install(user_data_dir: &Path, install_dir: Option<&Path>) -> InstallCheck {
    let Some(install_id) = read_marker(install_dir) else {
        return InstallCheck {
            report: false,
            install_id: None,
            reinstall: false,
        };
    };
    let state = read_state(user_data_dir);
    if state
        .as_ref()
        .is_some_and(|state| state.install_id.as_deref() == Some(install_id.as_str()))
    {
        return InstallCheck {
            report: false,
            install_id: Some(install_id),
            reinstall: false,
        };
    }
    let reinstall = state.is_some_and(|state| state.install_id.is_some());
    InstallCheck {
        report: true,
        install_id: Some(install_id),
        reinstall,
    }
}

/// `confirmInstall(userDataDir, installId)`: записать, что об установке сообщено.
pub fn confirm_install(user_data_dir: &Path, install_id: &str, now_ms: i64) -> bool {
    write_state(user_data_dir, install_id, now_ms).is_ok()
}
