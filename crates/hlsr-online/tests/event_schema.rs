//! Схема событий, коды ошибок и события самообновления против 0.4.7: фикстура `event-schema.json`
//! из `tools/reference/online/event-schema.js` (`event-schema.js`, `error-map.js`,
//! `launcher-update-events.js` в Node 24).

mod support;

use hlsr_core::json_text::js_number;
use hlsr_delivery::ports::MemoryLogger;
use hlsr_online::telemetry::error_map;
use hlsr_online::telemetry::event_schema::{
    self, DISABLED_TYPES, ENVELOPE, ERROR_CODES, EXIT_REASONS, FALLBACK_REASONS, LOCALES, NULLABLE,
    PLAN_KINDS, TYPES, WINDOWS_ARCHS, WINDOWS_MAJORS, limits,
};
use hlsr_online::telemetry::launcher_update_events::{
    AnnouncedVersions, LauncherUpdateEvents, TelemetrySink, Transfer,
};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use support::{Mismatches, fixture, items, str_of};

#[test]
fn types_and_constants_match_node() {
    let fixture = fixture("event-schema.json");
    let types: Vec<Value> = TYPES
        .iter()
        .map(|spec| json!({"type": spec.name, "kind": spec.kind.as_str(), "fields": spec.fields}))
        .collect();
    assert_eq!(Value::Array(types), fixture["types"]);
    let constants = json!({
        "ENVELOPE": ENVELOPE, "ERROR_CODES": ERROR_CODES, "EXIT_REASONS": EXIT_REASONS,
        "PLAN_KINDS": PLAN_KINDS, "FALLBACK_REASONS": FALLBACK_REASONS, "LOCALES": LOCALES,
        "WINDOWS_MAJORS": WINDOWS_MAJORS, "WINDOWS_ARCHS": WINDOWS_ARCHS,
        "LIMITS": {
            "durationSeconds": js_number(limits::DURATION_SECONDS),
            "elapsedMilliseconds": js_number(limits::ELAPSED_MILLISECONDS),
            "segmentMilliseconds": js_number(limits::SEGMENT_MILLISECONDS),
            "restartCount": js_number(limits::RESTART_COUNT),
            "bytes": js_number(limits::BYTES),
            "downloadDurationMilliseconds": js_number(limits::DOWNLOAD_DURATION_MILLISECONDS),
        },
        "NULLABLE": NULLABLE, "DISABLED_TYPES": DISABLED_TYPES,
    });
    assert_eq!(constants, fixture["constants"]);
    assert_eq!(
        json!(event_schema::emitted_types()),
        fixture["emittedTypes"]
    );
    for case in items(&fixture["enabledTypes"]) {
        assert_eq!(
            json!(event_schema::is_enabled_type(str_of(&case[0]))),
            case[1],
            "{case}"
        );
    }
    assert_eq!(
        json!(event_schema::game_telemetry_connected()),
        fixture["gameTelemetryConnected"]
    );
    // Схема ошибок одна у доставки и у телеметрии.
    assert_eq!(error_map::ERROR_CODES, ERROR_CODES);
    assert_eq!(error_map::FALLBACK_REASONS, FALLBACK_REASONS);
}

#[test]
fn validate_matches_node() {
    let fixture = fixture("event-schema.json");
    let cases = items(&fixture["validate"]);
    assert!(cases.len() > 100, "корпус: {}", cases.len());
    let mut mismatches = Mismatches::default();
    for case in cases {
        let actual = match event_schema::validate(&case["event"]) {
            Ok(()) => json!({"ok": true}),
            Err(reason) => json!({"ok": false, "reason": reason}),
        };
        mismatches.check(&actual, &case["result"], str_of(&case["label"]));
    }
    mismatches.finish("validate");
}

#[test]
fn check_field_matches_node() {
    let fixture = fixture("event-schema.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["checkField"]) {
        let field = str_of(&case["field"]);
        let actual = json!(event_schema::check_field(field, &case["value"]));
        mismatches.check(&actual, &case["ok"], format!("{field} = {}", case["value"]));
    }
    mismatches.finish("checkField");
}

#[test]
fn error_map_matches_node() {
    let fixture = fixture("event-schema.json");
    let map = &fixture["errorMap"];
    let mut mismatches = Mismatches::default();
    for case in items(&map["telemetryCode"]) {
        let actual = json!(error_map::to_telemetry_code(&case[0]));
        mismatches.check(&actual, &case[1], format!("toTelemetryCode({})", case[0]));
    }
    for case in items(&map["fallbackReason"]) {
        let actual = json!(error_map::to_fallback_reason(&case[0]));
        mismatches.check(&actual, &case[1], format!("toFallbackReason({})", case[0]));
    }
    for case in items(&map["fromHttpStatus"]) {
        let actual = json!(error_map::from_http_status(case[0].as_f64().unwrap()));
        mismatches.check(&actual, &case[1], format!("fromHttpStatus({})", case[0]));
    }
    mismatches.finish("error-map");
}

/// Поддельные `telemetry` и `versionEvents` сценария; вызовы — в общий журнал.
struct Fakes {
    record: Mutex<Vec<Value>>,
    announced: Mutex<HashSet<String>>,
    options: Value,
}

impl Fakes {
    fn push(&self, entry: Value) {
        self.record.lock().unwrap().push(entry);
    }
}

impl TelemetrySink for Fakes {
    fn enabled(&self) -> bool {
        self.options["enabled"] != json!(false)
    }
    fn emit(&self, event_type: &str, fields: Value) -> bool {
        self.push(json!({"emit": event_type, "fields": fields}));
        self.options["queued"] != json!(false)
    }
}

impl AnnouncedVersions for Fakes {
    fn already_announced(&self, stage: &str, version: &str) -> Result<bool, String> {
        if self.options["alreadyThrows"] == json!(true) {
            return Err("сбой чтения".to_owned());
        }
        Ok(self
            .announced
            .lock()
            .unwrap()
            .contains(&format!("{stage}:{version}")))
    }
    fn mark_announced(&self, stage: &str, version: &str) -> Result<(), String> {
        self.push(json!({"mark": format!("{stage}:{version}")}));
        if self.options["markThrows"] == json!(true) {
            return Err("сбой записи".to_owned());
        }
        self.announced
            .lock()
            .unwrap()
            .insert(format!("{stage}:{version}"));
        Ok(())
    }
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64().filter(|_| value.is_number())
}

#[test]
fn launcher_update_events_match_node() {
    let fixture = fixture("event-schema.json");
    let mut mismatches = Mismatches::default();
    for scenario in items(&fixture["launcherUpdate"]) {
        let options = scenario["options"].clone();
        let fakes = Arc::new(Fakes {
            record: Mutex::default(),
            announced: Mutex::new(
                options["announced"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|key| str_of(key).to_owned())
                    .collect(),
            ),
            options: options.clone(),
        });
        let logger = Arc::new(MemoryLogger::default());
        let telemetry: Option<Arc<dyn TelemetrySink>> =
            (options["telemetry"] != json!(false)).then(|| fakes.clone() as Arc<dyn TelemetrySink>);
        let version_events: Option<Arc<dyn AnnouncedVersions>> = (options["versionEvents"]
            != json!(false))
        .then(|| fakes.clone() as Arc<dyn AnnouncedVersions>);
        let events = LauncherUpdateEvents::new(telemetry, version_events, logger.clone());
        let mut results = Vec::new();
        for call in items(&scenario["calls"]) {
            let current = str_of(&call[1]);
            let next = call[2].as_str();
            let result = match str_of(&call[0]) {
                "available" => events.available(current, next),
                "downloadStarted" => events.download_started(current, next),
                "downloadCompleted" => events.download_completed(
                    current,
                    next,
                    call[3].is_object().then(|| Transfer {
                        bytes: number(&call[3]["bytes"]),
                        duration_ms: number(&call[3]["durationMs"]),
                    }),
                ),
                "installed" => events.installed(current, next),
                "failed" => events.failed(current, next, call[3].as_str()),
                other => panic!("неизвестный вызов {other}"),
            };
            results.push(json!(result));
            for (level, text) in logger.take() {
                fakes.push(json!({level: text}));
            }
        }
        let name = str_of(&scenario["name"]);
        mismatches.check(
            &json!(results),
            &scenario["results"],
            format!("{name}: итоги"),
        );
        let record = Value::Array(fakes.record.lock().unwrap().clone());
        mismatches.check(&record, &scenario["record"], format!("{name}: вызовы"));
    }
    mismatches.finish("LauncherUpdateEvents");
}
