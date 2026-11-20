//! Удостоверение сборки (решение 18): `appId`, имя, GUID, ключи реестра и каталог. QA-сборка 0.5.0
//! (`com.hlsrteam.hlsr.qa`, `HLSR-QA`, своя пара GUID и каталог `C:\HLSR-QA\Launcher`) не встаёт
//! поверх боевой установки, как QA-сборка 0.4.7 (`scripts/qa-launcher-build.js`), и не делит с ней
//! профиль: имя приложения `hlsr-qa` (`%APPDATA%\hlsr-qa`, `%LOCALAPPDATA%\hlsr-qa\logs`) лаунчер
//! узнаёт по `app` своего `install-files.json`. GUID — UUID v5 от
//! пространства имён electron-builder и `appId` (`windows-install/layout.js`, `guidFor`), боевой —
//! `3f824379-6b41-5725-b424-59cb87f2acae`.

use crate::error::{ReleaseError, Result};
use hlsr_update::windows::layout::{self, App};
use serde_json::{Value, json};

/// Удостоверение боевой (`qa = false`) или QA-сборки.
pub fn identity_json(qa: bool, version: Option<&str>) -> Result<Value> {
    let app = if qa { App::qa() } else { App::production() };
    let identity =
        layout::identity(Some(&app)).map_err(|error| ReleaseError::Message(error.to_string()))?;
    let mut out = json!({
        "qa": qa,
        "appId": identity.app_id,
        "productName": identity.product_name,
        "guid": identity.guid,
        "installKey": identity.install_key,
        "uninstallKey": identity.uninstall_key,
        "defaultInstallDir": identity.default_install_dir,
        "shortcutName": identity.shortcut_name,
        "appName": app.name(),
    });
    if let Some(version) = version {
        out["installerName"] = Value::from(format!("HLSR-Setup-{version}.exe"));
    }
    Ok(out)
}
