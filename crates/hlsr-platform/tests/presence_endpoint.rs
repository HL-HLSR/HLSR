//! Файл канала связи с игрой против 0.4.7: фикстура `presence-endpoint.json`
//! (`tools/reference/platform/presence-endpoint.js`: разбор значений, чтение файла, байты записи,
//! удаление) и первые два самотеста `presence-endpoint-selftest.js` (остальные два — про сервер,
//! они в `presence_server.rs`).

mod support;

use hlsr_platform::game_presence::endpoint_store::{self, Endpoint};
use serde_json::{Value, json};
use support::{Mismatches, fixture, items};

fn endpoint_value(endpoint: Option<Endpoint>) -> Value {
    endpoint.map_or(Value::Null, |endpoint| endpoint.to_value())
}

#[test]
fn normalize_like_0_4_7() {
    let fixture = fixture("presence-endpoint.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["normalize"]) {
        let actual = endpoint_value(endpoint_store::normalize(&case["value"]));
        mismatches.check(
            &actual,
            &case["result"],
            format!("normalize({})", case["value"]),
        );
    }
    mismatches.finish("normalize");
}

/// `Number(value.port)`: массив приводится по тексту (`Number(["8080"])` — 8080, `Number([])` — 0,
/// `Number([1, 2])` — `NaN`); общий помощник `hlsr_core::js::value::to_number`.
#[test]
fn port_is_coerced_like_js_number() {
    let with_port = |port: Value| {
        endpoint_store::normalize(&json!({
            "pipeName": format!("{}0123456789abcdef", endpoint_store::PIPE_PREFIX),
            "token": "a".repeat(64),
            "port": port,
        }))
        .expect("имя и токен верны")
        .port
    };
    assert_eq!(with_port(json!(["8080"])), Some(8080));
    assert_eq!(with_port(json!("8080")), Some(8080));
    assert_eq!(with_port(json!([])), None);
    assert_eq!(with_port(json!([1, 2])), None);
    assert_eq!(with_port(json!({})), None);
    assert_eq!(with_port(json!(true)), Some(1));
    assert_eq!(with_port(json!(65536)), None);
}

#[test]
fn load_like_0_4_7() {
    let fixture = fixture("presence-endpoint.json");
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("presence-endpoint.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["load"]) {
        match case["text"].as_str() {
            Some(text) => std::fs::write(&file, text).unwrap(),
            None => {
                let _ = std::fs::remove_file(&file);
            }
        }
        let actual = endpoint_value(endpoint_store::load(&file));
        mismatches.check(&actual, &case["result"], format!("load({})", case["text"]));
    }
    mismatches.finish("load");
}

#[test]
fn save_writes_the_same_bytes() {
    let fixture = fixture("presence-endpoint.json");
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("presence-endpoint.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["save"]) {
        if case["value"] == json!("overwrite") {
            let make = |port: u16| Endpoint {
                pipe_name: format!("{}0123456789abcdef", endpoint_store::PIPE_PREFIX),
                token: "a".repeat(64),
                port: Some(port),
            };
            assert!(endpoint_store::save_endpoint(&file, &make(1)).unwrap());
            assert!(endpoint_store::save_endpoint(&file, &make(2)).unwrap());
        } else {
            let _ = std::fs::remove_file(&file);
            let result = endpoint_store::save(&file, &case["value"]).unwrap();
            mismatches.check(
                &json!(result),
                &case["result"],
                format!("save({})", case["value"]),
            );
        }
        let bytes = std::fs::read(&file)
            .ok()
            .map(|bytes| String::from_utf8(bytes).unwrap());
        let expected = case["bytes"].as_str().map(str::to_owned);
        mismatches.check_eq(bytes, expected, format!("байты save({})", case["value"]));
    }
    mismatches.finish("save");
}

#[test]
fn clear_removes_the_file_and_forgives_the_second_call() {
    let fixture = fixture("presence-endpoint.json");
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("presence-endpoint.json");
    let endpoint = Endpoint {
        pipe_name: format!("{}0123456789abcdef", endpoint_store::PIPE_PREFIX),
        token: "a".repeat(64),
        port: Some(5),
    };
    assert!(endpoint_store::save_endpoint(&file, &endpoint).unwrap());
    endpoint_store::clear(&file);
    assert_eq!(
        !file.exists(),
        fixture["clear"]["removed"].as_bool().unwrap()
    );
    endpoint_store::clear(&file);
    assert_eq!(!file.exists(), fixture["clear"]["again"].as_bool().unwrap());
}

// --- presence-endpoint-selftest.js

#[test]
fn selftest_saved_read_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("presence-endpoint.json");
    let endpoint = Endpoint {
        pipe_name: "\\\\.\\pipe\\hlsr-presence-0123456789abcdef".to_owned(),
        token: "a".repeat(64),
        port: Some(40123),
    };
    assert!(endpoint_store::save_endpoint(&file, &endpoint).unwrap());
    assert_eq!(endpoint_store::load(&file), Some(endpoint));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    endpoint_store::clear(&file);
    assert_eq!(endpoint_store::load(&file), None);
}

#[test]
fn selftest_damaged_file_is_not_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("presence-endpoint.json");
    std::fs::write(
        &file,
        json!({"pipeName": "\\\\.\\pipe\\evil", "token": "x", "port": 1}).to_string(),
    )
    .unwrap();
    assert_eq!(endpoint_store::load(&file), None);
    std::fs::write(&file, "{не json").unwrap();
    assert_eq!(endpoint_store::load(&file), None);
    let bad = json!({"pipeName": "nope", "token": "a".repeat(64)});
    assert!(!endpoint_store::save(&file, &bad).unwrap());
}
