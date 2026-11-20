//! Коды ошибок для телеметрии (`telemetry/error-map.js`): код доставки → код события и причина
//! отказа от патча. Фасад доставки пишет их в события обновления и ремонта.

use serde_json::Value;

/// `MAP`: код ошибки → код телеметрии.
pub const MAP: [(&str, &str); 37] = [
    ("STEAM_NOT_RUNNING", "STEAM_NOT_RUNNING"),
    ("STEAM_NOT_INSTALLED", "STEAM_NOT_INSTALLED"),
    ("STEAM_PROCESS_MISMATCH", "STEAM_NOT_RUNNING"),
    ("SOURCE_SDK_NOT_INSTALLED", "SDK_2013_NOT_INSTALLED"),
    ("SOURCE_SDK_INCOMPLETE", "SDK_2013_NOT_INSTALLED"),
    ("NETWORK_ERROR", "NETWORK_UNREACHABLE"),
    ("NETWORK_TIMEOUT", "NETWORK_TIMEOUT"),
    ("TOO_MANY_REDIRECTS", "POSSIBLE_RESOURCE_BLOCK"),
    ("INSECURE_REDIRECT", "POSSIBLE_RESOURCE_BLOCK"),
    ("PROVIDER_FORBIDDEN", "POSSIBLE_RESOURCE_BLOCK"),
    ("PROVIDER_LINK_FAILED", "POSSIBLE_RESOURCE_BLOCK"),
    ("CHANNEL_UNAVAILABLE", "POSSIBLE_RESOURCE_BLOCK"),
    ("CHECKSUM_MISMATCH", "CHECKSUM_MISMATCH"),
    ("SIZE_MISMATCH", "CHECKSUM_MISMATCH"),
    ("CONTENT_LENGTH_MISMATCH", "CHECKSUM_MISMATCH"),
    ("SIGNATURE_INVALID", "SIGNATURE_INVALID"),
    ("MANIFEST_INVALID", "MANIFEST_INVALID"),
    ("MANIFEST_NOT_FOUND", "MANIFEST_INVALID"),
    ("FILES_MANIFEST_MISSING", "MANIFEST_INVALID"),
    ("BUILD_METADATA_INVALID", "MANIFEST_INVALID"),
    ("BUILD_METADATA_MISMATCH", "MANIFEST_INVALID"),
    ("EXTRACT_FAILED", "EXTRACT_FAILED"),
    ("ARCHIVE_LIST_FAILED", "EXTRACT_FAILED"),
    ("ARCHIVE_UNSAFE_ENTRY", "EXTRACT_FAILED"),
    ("ARCHIVE_LIMIT_EXCEEDED", "EXTRACT_FAILED"),
    ("NO_SPACE", "NO_SPACE"),
    ("REPAIR_FAILED", "REPAIR_FAILED"),
    ("REPAIR_SOURCE_MISSING", "REPAIR_FAILED"),
    ("ROLLBACK_FAILED", "REPAIR_FAILED"),
    ("VERIFY_FAILED", "REPAIR_FAILED"),
    ("LAUNCH_FAILED", "LAUNCH_FAILED"),
    ("LAUNCH_CONFIG_INVALID", "LAUNCH_FAILED"),
    ("GAME_EXECUTABLE_MISSING", "LAUNCH_FAILED"),
    ("GAME_DIRECTORY_MISSING", "LAUNCH_FAILED"),
    ("LIBRARY_PATH_CASE_CONFLICT", "LAUNCH_FAILED"),
    ("STEAM_APP_ID_MISMATCH", "LAUNCH_FAILED"),
    ("PROFILE_IMPORT_FAILED", "PROFILE_IMPORT_FAILED"),
];

/// `schema.ERROR_CODES` (`telemetry/event-schema.js`).
pub const ERROR_CODES: [&str; 21] = [
    "STEAM_NOT_RUNNING",
    "STEAM_NOT_INSTALLED",
    "SDK_2013_NOT_INSTALLED",
    "LICENSE_NOT_AVAILABLE",
    "HTTP_404",
    "HTTP_429",
    "HTTP_5XX",
    "NETWORK_TIMEOUT",
    "NETWORK_UNREACHABLE",
    "POSSIBLE_RESOURCE_BLOCK",
    "CHECKSUM_MISMATCH",
    "SIGNATURE_INVALID",
    "MANIFEST_INVALID",
    "EXTRACT_FAILED",
    "NO_SPACE",
    "REPAIR_FAILED",
    "PROFILE_IMPORT_FAILED",
    "LAUNCH_FAILED",
    "GAME_CRASH",
    "LAUNCHER_CRASH",
    "UNKNOWN_SAFE",
];

/// `FALLBACK_MAP`: код ошибки → причина отказа от патча.
pub const FALLBACK_MAP: [(&str, &str); 5] = [
    ("PATCH_CHAIN_MISSING", "PATCH_UNAVAILABLE"),
    ("PATCH_FAILED", "PATCH_APPLY_FAILED"),
    ("CHECKSUM_MISMATCH", "CHECKSUM_MISMATCH"),
    ("REPAIR_SOURCE_MISSING", "SOURCE_MISSING"),
    ("VERSION_MISMATCH", "SOURCE_MISSING"),
];

/// `schema.FALLBACK_REASONS`.
pub const FALLBACK_REASONS: [&str; 6] = [
    "PATCH_UNAVAILABLE",
    "PATCH_APPLY_FAILED",
    "CHECKSUM_MISMATCH",
    "CHAIN_TOO_LONG",
    "SOURCE_MISSING",
    "UNKNOWN_SAFE",
];

const UNKNOWN: &str = "UNKNOWN_SAFE";

/// `fromHttpStatus(status)`.
pub fn from_http_status(status: f64) -> Option<&'static str> {
    if status == 404.0 {
        Some("HTTP_404")
    } else if status == 429.0 {
        Some("HTTP_429")
    } else if (500.0..600.0).contains(&status) {
        Some("HTTP_5XX")
    } else {
        None
    }
}

fn lookup(map: &[(&str, &'static str)], code: &str) -> Option<&'static str> {
    map.iter()
        .find(|(from, _)| *from == code)
        .map(|(_, to)| *to)
}

/// `toTelemetryCode(error)`: `error` — объект ошибки (`{code, details}`) или строка-код.
pub fn to_telemetry_code(input: &Value) -> &'static str {
    if !hlsr_core::js::truthy(input) {
        return UNKNOWN;
    }
    let code = match input {
        Value::String(code) => code.as_str(),
        _ => match input.get("code") {
            Some(Value::String(code)) => code.as_str(),
            _ => return UNKNOWN,
        },
    };

    if code == "HTTP_ERROR" {
        let status = input
            .get("details")
            .filter(|details| hlsr_core::js::truthy(details))
            .and_then(|details| details.get("status"))
            .map(crate::js_value::to_number)
            .filter(|status| status.is_finite());
        return status.and_then(from_http_status).unwrap_or(UNKNOWN);
    }

    match lookup(&MAP, code) {
        Some(mapped) if ERROR_CODES.contains(&mapped) => mapped,
        _ => UNKNOWN,
    }
}

/// `toFallbackReason(error)`.
pub fn to_fallback_reason(input: &Value) -> &'static str {
    if !hlsr_core::js::truthy(input) {
        return UNKNOWN;
    }
    let code = match input {
        Value::String(code) => Some(code.as_str()),
        _ => input.get("code").and_then(Value::as_str),
    };
    match code.and_then(|code| lookup(&FALLBACK_MAP, code)) {
        Some(mapped) if FALLBACK_REASONS.contains(&mapped) => mapped,
        _ => UNKNOWN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codes_map_like_0_4_7() {
        assert_eq!(to_telemetry_code(&json!(null)), "UNKNOWN_SAFE");
        assert_eq!(
            to_telemetry_code(&json!("NETWORK_ERROR")),
            "NETWORK_UNREACHABLE"
        );
        assert_eq!(
            to_telemetry_code(&json!({"code": "SIZE_MISMATCH"})),
            "CHECKSUM_MISMATCH"
        );
        assert_eq!(to_telemetry_code(&json!({"code": 5})), "UNKNOWN_SAFE");
        assert_eq!(
            to_telemetry_code(&json!({"code": "CANCELLED"})),
            "UNKNOWN_SAFE"
        );
        assert_eq!(
            to_telemetry_code(&json!({"code": "HTTP_ERROR", "details": {"status": "503"}})),
            "HTTP_5XX"
        );
        assert_eq!(
            to_telemetry_code(&json!({"code": "HTTP_ERROR", "details": {"status": 403}})),
            "UNKNOWN_SAFE"
        );
        assert_eq!(
            to_telemetry_code(&json!({"code": "HTTP_ERROR"})),
            "UNKNOWN_SAFE"
        );
        assert_eq!(
            to_fallback_reason(&json!({"code": "PATCH_FAILED"})),
            "PATCH_APPLY_FAILED"
        );
        assert_eq!(
            to_fallback_reason(&json!("VERSION_MISMATCH")),
            "SOURCE_MISSING"
        );
        assert_eq!(
            to_fallback_reason(&json!({"code": "NO_SPACE"})),
            "UNKNOWN_SAFE"
        );
        assert_eq!(to_fallback_reason(&json!("")), "UNKNOWN_SAFE");
    }
}
