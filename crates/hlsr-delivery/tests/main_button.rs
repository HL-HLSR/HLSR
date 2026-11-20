//! Главная кнопка и причины недоступности против 0.4.7: фикстура `main-button.json` из
//! `tools/reference/delivery/main-button.js` (`main-button.js`, `unavailable-reasons.js`).

mod support;

use hlsr_core::json_text::stringify;
use hlsr_delivery::main_button::{self, Input, UPDATE_OPERATIONS};
use hlsr_delivery::unavailable_reasons::{
    REASONS, from_delivery_status, from_steam_health, is_known_reason,
};
use serde_json::Value;
use std::sync::LazyLock;
use support::{Mismatches, fixture, items};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("main-button.json"));

/// Значение и порядок ключей — как у 0.4.7.
fn check(mismatches: &mut Mismatches, actual: &Value, case: &Value) {
    let context = stringify(&case["input"], "");
    mismatches.check(actual, &case["result"], &context);
    if *actual == case["result"] {
        mismatches.check_eq(
            stringify(actual, ""),
            stringify(&case["result"], ""),
            format!("{context}: порядок ключей"),
        );
    }
}

/// `undefined` в JS: поля нет — `None`.
fn field<'a>(input: &'a Value, key: &str) -> Option<&'a Value> {
    input.get(key)
}

#[test]
fn constants_match_0_4_7() {
    let constants = &FIXTURE["constants"];
    let actions: Vec<&str> = constants["ACTIONS"]
        .as_object()
        .unwrap()
        .values()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert_eq!(actions, main_button::ACTIONS);
    assert_eq!(
        constants["UPDATE_OPERATIONS"],
        serde_json::json!(UPDATE_OPERATIONS)
    );
    assert_eq!(constants["REASONS"], serde_json::json!(REASONS));
}

#[test]
fn main_button_matches_0_4_7() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["button"]) {
        let input = &case["input"];
        let actual = main_button::resolve(&Input {
            delivery: field(input, "delivery"),
            process: field(input, "process"),
            update: field(input, "update"),
            checking_updates: input["checkingUpdates"] == true,
        });
        check(&mut mismatches, &actual, case);
    }
    mismatches.finish("главная кнопка");
}

#[test]
fn unavailable_reasons_match_0_4_7() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["status"]) {
        let input = Some(&case["input"]).filter(|input| !input.is_null());
        check(&mut mismatches, &from_delivery_status(input), case);
    }
    for case in items(&FIXTURE["health"]) {
        let input = Some(&case["input"]).filter(|input| !input.is_null());
        check(&mut mismatches, &from_steam_health(input), case);
    }
    for case in items(&FIXTURE["known"]) {
        let actual = Value::Bool(is_known_reason(Some(&case["input"])));
        check(&mut mismatches, &actual, case);
    }
    mismatches.finish("причины недоступности");
}
