//! Причина, по которой игра недоступна, — по статусу доставки и проверке Steam
//! (`unavailable-reasons.js`).

use crate::js_value as jv;
use serde_json::Value;

/// `REASONS`.
pub const REASONS: [&str; 21] = [
    "STEAM_NOT_RUNNING",
    "STEAM_INTEGRATION_UNAVAILABLE",
    "SOURCE_SDK_2013_MISSING",
    "REQUIRED_LICENSE_MISSING",
    "LICENSE_CHECK_UNAVAILABLE",
    "GAME_NOT_INSTALLED",
    "DELIVERY_CHANNEL_DISABLED",
    "MANIFEST_UNAVAILABLE",
    "MANIFEST_SIGNATURE_INVALID",
    "UPDATE_REQUIRED",
    "GAME_FILES_DAMAGED",
    "BUILD_METADATA_MISSING",
    "BUILD_METADATA_INVALID",
    "BUILD_METADATA_MISMATCH",
    "BUILD_METADATA_UNSUPPORTED",
    "BUILD_REQUIRES_NEWER_LAUNCHER",
    "LIBRARY_UNAVAILABLE",
    "LIBRARY_READ_ONLY",
    "INSUFFICIENT_DISK_SPACE",
    "GAME_PROCESS_ALREADY_RUNNING",
    "UNKNOWN_UNAVAILABLE",
];

/// `INSTALLABLE_REASON_MAP`: причина отказа `checkInstallable` → причина недоступности.
pub const INSTALLABLE_REASON_MAP: [(&str, &str); 8] = [
    ("REMOTE_DISABLED", "DELIVERY_CHANNEL_DISABLED"),
    ("MANIFEST_DISABLED", "DELIVERY_CHANNEL_DISABLED"),
    ("MANIFEST_VERSION_DRAFT", "DELIVERY_CHANNEL_DISABLED"),
    ("MANIFEST_INVALID", "MANIFEST_UNAVAILABLE"),
    ("MANIFEST_UNAVAILABLE", "MANIFEST_UNAVAILABLE"),
    ("SIGNATURE_INVALID", "MANIFEST_SIGNATURE_INVALID"),
    ("LAUNCHER_TOO_OLD", "UPDATE_REQUIRED"),
    ("NO_PROVIDERS", "DELIVERY_CHANNEL_DISABLED"),
];

/// `ERROR_REASON_MAP`: код ошибки операции → причина недоступности.
pub const ERROR_REASON_MAP: [(&str, &str); 13] = [
    ("SIGNATURE_INVALID", "MANIFEST_SIGNATURE_INVALID"),
    ("MANIFEST_INVALID", "MANIFEST_UNAVAILABLE"),
    ("MANIFEST_UNAVAILABLE", "MANIFEST_UNAVAILABLE"),
    ("NETWORK_ERROR", "MANIFEST_UNAVAILABLE"),
    ("NO_SPACE", "INSUFFICIENT_DISK_SPACE"),
    ("PATH_INVALID", "LIBRARY_UNAVAILABLE"),
    ("PATH_NOT_WRITABLE", "LIBRARY_READ_ONLY"),
    ("GAME_RUNNING", "GAME_PROCESS_ALREADY_RUNNING"),
    ("BUILD_METADATA_MISSING", "BUILD_METADATA_MISSING"),
    ("BUILD_METADATA_INVALID", "BUILD_METADATA_INVALID"),
    ("BUILD_METADATA_MISMATCH", "BUILD_METADATA_MISMATCH"),
    ("BUILD_METADATA_UNSUPPORTED", "BUILD_METADATA_UNSUPPORTED"),
    (
        "BUILD_REQUIRES_NEWER_LAUNCHER",
        "BUILD_REQUIRES_NEWER_LAUNCHER",
    ),
];

/// `MAP[key]` для ключа — значения JS (`MAP[1]` ищет `"1"`); `undefined` и не-строки,
/// которых нет в таблице, — `None`.
fn lookup(map: &[(&str, &'static str)], key: Option<&Value>) -> Option<&'static str> {
    let key = key.map(|key| jv::template(Some(key)))?;
    map.iter().find(|(from, _)| *from == key).map(|(_, to)| *to)
}

/// `isKnownReason(code)`: `String(code || "")` из списка [`REASONS`].
pub fn is_known_reason(code: Option<&Value>) -> bool {
    let text = code
        .filter(|code| hlsr_core::js::truthy(code))
        .map(hlsr_core::js::to_string)
        .unwrap_or_default();
    REASONS.contains(&text.as_str())
}

/// `fromDeliveryStatus(status)`: причина недоступности по статусу доставки; `null` — доступна.
/// Причина из сведений о сборке — сам код (строка или иное значение из `metadataIssue.code`).
pub fn from_delivery_status(status: Option<&Value>) -> Value {
    let Some(status) = status.filter(|status| hlsr_core::js::truthy(status)) else {
        return Value::from("UNKNOWN_UNAVAILABLE");
    };

    let issue = jv::prop(status, "metadataIssue");
    if jv::truthy(issue) {
        let code = issue.and_then(|issue| jv::prop(issue, "code"));
        if is_known_reason(code) {
            return code.cloned().unwrap_or(Value::Null);
        }
    }

    if jv::is_str(jv::prop(status, "state"), "files-damaged") {
        return Value::from("GAME_FILES_DAMAGED");
    }

    let error = jv::prop(status, "error");
    if jv::truthy(error)
        && let Some(reason) = lookup(
            &ERROR_REASON_MAP,
            error.and_then(|error| jv::prop(error, "code")),
        )
    {
        return Value::from(reason);
    }

    if jv::prop(status, "installable") == Some(&Value::Bool(false)) {
        let reasons = jv::prop(status, "reasons").filter(|reasons| hlsr_core::js::truthy(reasons));
        if let Some(Value::Array(reasons)) = reasons {
            for code in reasons {
                if let Some(reason) = lookup(&INSTALLABLE_REASON_MAP, Some(code)) {
                    return Value::from(reason);
                }
            }
        }
        return Value::from("DELIVERY_CHANNEL_DISABLED");
    }

    Value::Null
}

/// `fromSteamHealth(health)`: причина недоступности по проверке Steam.
pub fn from_steam_health(health: Option<&Value>) -> Value {
    let Some(health) = health.filter(|health| hlsr_core::js::truthy(health)) else {
        return Value::Null;
    };
    let reason = match jv::prop(health, "code").and_then(Value::as_str) {
        Some("STEAM_NOT_INSTALLED" | "STEAM_NOT_RUNNING" | "STEAM_PROCESS_MISMATCH") => {
            "STEAM_NOT_RUNNING"
        }
        Some("SOURCE_SDK_NOT_INSTALLED" | "SOURCE_SDK_INCOMPLETE") => "SOURCE_SDK_2013_MISSING",
        Some("LAUNCH_CONFIG_INVALID") => "STEAM_INTEGRATION_UNAVAILABLE",
        Some("STEAM_CHECK_FAILED") => "LICENSE_CHECK_UNAVAILABLE",
        _ if jv::truthy(jv::prop(health, "ok")) => return Value::Null,
        _ => "STEAM_INTEGRATION_UNAVAILABLE",
    };
    Value::from(reason)
}
