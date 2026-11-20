//! Файлы телеметрии против 0.4.7: фикстура `telemetry-files.json` из
//! `tools/reference/online/telemetry-files.js` — `consent`, `identity`, `crash-marker`,
//! `install-receipt` (Node 24) на сценариях над папкой данных: решения модулей и байты файлов
//! на диске. Плюс обычные тесты Rust.

mod support;

use hlsr_online::telemetry::{consent, crash_marker, identity, install_receipt};
use serde_json::{Value, json};
use std::path::Path;
use support::{Mismatches, files, fixture, hex_bytes, items, path_in, str_of};

fn consent_json(consent: consent::Consent) -> Value {
    json!({"enabled": consent.enabled, "source": consent.source.as_str()})
}

fn identity_json(identity: Option<identity::Identity>) -> Value {
    identity.map_or(Value::Null, |identity| {
        json!({"installationId": identity.installation_id, "createdAt": identity.created_at})
    })
}

fn step(root: &Path, step: &Value, default_now: i64, uuids: &mut u64) -> Value {
    let now = step["now"].as_i64().unwrap_or(default_now);
    match str_of(&step["op"]) {
        "write" => {
            let file = path_in(root, str_of(&step["file"]));
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            let bytes = match step["utf8"].as_str() {
                Some(text) => text.as_bytes().to_vec(),
                None => hex_bytes(str_of(&step["hex"])),
            };
            std::fs::write(file, bytes).unwrap();
            Value::Null
        }
        "files" => files(root),
        "consent.read" => consent_json(consent::read(root)),
        "consent.set" => consent_json(
            consent::set(root, step["enabled"].as_bool().unwrap(), now).unwrap(),
        ),
        "consent.canSend" => json!(consent::can_send(root)),
        "consent.decode" => json!(consent::decode(&hex_bytes(str_of(&step["hex"])))),
        "identity.read" => identity_json(identity::read(root)),
        "identity.ensure" => {
            let next = || {
                *uuids += 1;
                format!("00000000-0000-4000-8000-{:012}", *uuids)
            };
            identity_json(Some(identity::ensure_with(root, now, next).unwrap()))
        }
        "identity.reset" => json!(identity::reset(root)),
        "marker.open" => {
            crash_marker::open(root, str_of(&step["sessionId"]), Some(now)).unwrap();
            Value::Null
        }
        "marker.touch" => {
            crash_marker::touch(root, Some(now));
            Value::Null
        }
        "marker.detect" => crash_marker::detect(root, Some(now)).map_or(Value::Null, |crashed| {
            json!({"sessionId": crashed.session_id, "lastAliveAt": crashed.last_alive_at})
        }),
        "marker.close" => json!(crash_marker::close(root)),
        "marker.consume" => json!(crash_marker::consume(root)),
        "install.readMarker" => {
            let dir = path_in(root, str_of(&step["installDir"]));
            install_receipt::read_marker(Some(&dir)).map_or(Value::Null, |id| json!(id))
        }
        "install.readState" => install_receipt::read_state(root).map_or(Value::Null, |state| {
            json!({"installId": state.install_id, "reportedAt": state.reported_at})
        }),
        "install.detect" => {
            let dir = step["installDir"].as_str().map(|dir| path_in(root, dir));
            let check = install_receipt::detect_install(root, dir.as_deref());
            json!({
                "report": check.report,
                "installId": check.install_id,
                "reinstall": check.reinstall,
            })
        }
        "install.confirm" => json!(install_receipt::confirm_install(
            root,
            str_of(&step["installId"]),
            now
        )),
        other => panic!("неизвестный шаг {other}"),
    }
}

#[test]
fn scenarios_match_node() {
    let fixture = fixture("telemetry-files.json");
    let default_now = fixture["now"].as_i64().unwrap();
    let scenarios = items(&fixture["scenarios"]);
    assert!(scenarios.len() >= 9);
    let mut mismatches = Mismatches::default();
    // Последовательность UUID одна на все сценарии, как в скрипте Node.
    let mut uuids = 0;
    for scenario in scenarios {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let steps = items(&scenario["steps"]);
        let results: Vec<Value> = steps
            .iter()
            .map(|s| step(&root, s, default_now, &mut uuids))
            .collect();
        let name = str_of(&scenario["name"]);
        for ((s, actual), expected) in steps.iter().zip(&results).zip(items(&scenario["results"])) {
            mismatches.check(actual, expected, format!("{name}: {s}"));
        }
    }
    mismatches.finish("файлы телеметрии");
}

#[test]
fn random_uuid_is_version_4() {
    let id = identity::random_uuid();
    assert_eq!(id.len(), 36);
    let parts: Vec<&str> = id.split('-').collect();
    assert_eq!(
        parts.iter().map(|part| part.len()).collect::<Vec<_>>(),
        [8, 4, 4, 4, 12]
    );
    assert!(parts[2].starts_with('4'), "{id}");
    assert!(
        matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'),
        "{id}"
    );
    assert!(
        id.bytes()
            .all(|b| b == b'-' || b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    );
    assert_ne!(id, identity::random_uuid());
}

#[test]
fn ensure_with_real_uuid_writes_a_readable_identity() {
    let temp = tempfile::tempdir().unwrap();
    let created = identity::ensure(temp.path(), 1_760_000_000_000).unwrap();
    assert_eq!(identity::read(temp.path()).unwrap(), created);
    assert_eq!(created.created_at, json!("2025-10-09T08:53:20.000Z"));
}
