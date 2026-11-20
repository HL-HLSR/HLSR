//! Настройки оверлея LiveSplit против 0.4.7: фикстура `livesplit-overlay.json` из
//! `tools/reference/delivery/livesplit-overlay.js` (`shared/livesplit-overlay.js`: `KEYS`, `LIMITS`,
//! `defaultOverlay`, `sanitizeOverlay`, `overlayChanges`). Порядок ключей — по записи `JSON.stringify`.

mod support;

use hlsr_core::json_text::{js_number, stringify};
use hlsr_delivery::livesplit_overlay as lso;
use serde_json::{Map, Value, json};
use std::sync::LazyLock;
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("livesplit-overlay.json"));

fn ser(value: &Value) -> String {
    stringify(value, "")
}

#[test]
fn constants_and_defaults_match_0_4_7() {
    let c = &FIXTURE["constants"];
    assert_eq!(c["KEYS"], json!(lso::KEYS));
    let limits: Map<String, Value> = lso::LIMITS
        .iter()
        .map(|(key, (min, max))| ((*key).to_owned(), json!([js_number(*min), js_number(*max)])))
        .collect();
    assert_eq!(Value::Object(limits), c["LIMITS"]);
    assert_eq!(
        ser(&lso::default_overlay().to_value()),
        str_of(&FIXTURE["defaults"])
    );
}

#[test]
fn sanitize_overlay_matches_0_4_7() {
    let cases = items(&FIXTURE["sanitize"]);
    assert!(cases.len() > 600, "случаев: {}", cases.len());
    let mut m = Mismatches::default();
    for case in cases {
        let input = &case["input"];
        m.check_eq(
            ser(&lso::sanitize_overlay(input).to_value()),
            str_of(&case["result"]).to_owned(),
            format!("sanitizeOverlay({})", ser(input)),
        );
        m.check_eq(
            lso::overlay_changes(input),
            case["changes"].as_u64().unwrap() as usize,
            format!("overlayChanges({})", ser(input)),
        );
    }
    for case in items(&FIXTURE["changes"]) {
        let input = &case["input"];
        m.check_eq(
            lso::overlay_changes(input),
            case["changes"].as_u64().unwrap() as usize,
            format!("overlayChanges({})", ser(input)),
        );
    }
    m.finish("настройки оверлея LiveSplit");
}
