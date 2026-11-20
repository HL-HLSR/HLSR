//! Числа JS в текст против Node 24: `String(число)` и `toFixed`. Фикстура
//! `node-semantics-number.json` из `tools/reference/node-semantics-number.js`.

use hlsr_core::js::number::{number_to_string, to_fixed};
use hlsr_core::js::to_number;
use serde_json::Value;

fn fixture() -> Value {
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/node-semantics-number.json"
    );
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

fn str_of(value: &Value) -> &str {
    value.as_str().expect("строка")
}

#[test]
fn number_to_string_matches_node() {
    for case in fixture()["numberToString"].as_array().unwrap() {
        let input = str_of(&case[0]);
        assert_eq!(
            number_to_string(to_number(input)),
            str_of(&case[1]),
            "String(Number({input:?}))"
        );
    }
}

#[test]
fn to_fixed_matches_node() {
    let fixture = fixture();
    let digits: Vec<usize> = fixture["digits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_u64().unwrap() as usize)
        .collect();
    for case in fixture["toFixed"].as_array().unwrap() {
        let input = str_of(&case[0]);
        let x = to_number(input);
        for (digits, expected) in digits.iter().zip(case[1].as_array().unwrap()) {
            assert_eq!(
                to_fixed(x, *digits),
                str_of(expected),
                "Number({input:?}).toFixed({digits})"
            );
        }
    }
}
