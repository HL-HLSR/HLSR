//! `new Date(ms).toISOString()` против Node 24: фикстура `node-semantics-iso.json` из
//! `tools/reference/node-semantics-iso.js`.

use hlsr_core::js::iso::iso_from_ms;
use serde_json::Value;

fn fixture() -> Value {
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/node-semantics-iso.json"
    );
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

#[test]
fn iso_from_ms_matches_node() {
    let fixture = fixture();
    let cases = fixture["cases"].as_array().unwrap();
    assert!(cases.len() > 30, "корпус слишком мал: {}", cases.len());
    for case in cases {
        let ms = case[0].as_i64().expect("целые миллисекунды");
        assert_eq!(
            iso_from_ms(ms),
            case[1].as_str().unwrap(),
            "new Date({ms}).toISOString()"
        );
    }
}
