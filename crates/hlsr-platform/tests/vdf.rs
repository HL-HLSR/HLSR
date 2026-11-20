//! Разбор VDF против 0.4.7: фикстура `vdf.json` из `tools/reference/platform/vdf.js`
//! (`vdf.js`). Сверяется и порядок ключей: числовые ключи идут первыми, как в `Object.keys`.

mod support;

use hlsr_platform::vdf::{self, VdfValue, get_insensitive_in};
use serde_json::Value;
use std::sync::LazyLock;
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("vdf.json"));

/// Запись значения с ключами в порядке обхода — по ней видно и порядок.
fn dump(value: &VdfValue) -> String {
    match value {
        VdfValue::String(text) => serde_json::to_string(text).unwrap(),
        VdfValue::Object(object) => {
            let parts: Vec<String> = object
                .iter()
                .map(|(key, value)| {
                    format!("{}:{}", serde_json::to_string(key).unwrap(), dump(value))
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
    }
}

/// То же для ожидания: ключи объекта JSON идут в порядке файла (порядок `Object.keys` Node).
fn dump_expected(value: &Value) -> String {
    match value {
        Value::String(text) => serde_json::to_string(text).unwrap(),
        Value::Object(object) => {
            let parts: Vec<String> = object
                .iter()
                .map(|(key, value)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap(),
                        dump_expected(value)
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        other => panic!("в разборе VDF бывают строки и разделы, а не {other}"),
    }
}

#[test]
fn parse_matches_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["parse"]) {
        let text = str_of(&case["text"]);
        match vdf::parse(text) {
            Ok(object) => {
                let expected = case.get("ok").unwrap_or_else(|| {
                    panic!(
                        "0.4.7 отклонил, а Rust принял: {text:?} → {}",
                        case["error"]
                    )
                });
                m.check_eq(
                    dump(&VdfValue::Object(object)),
                    dump_expected(expected),
                    format!("parse({text:?})"),
                );
            }
            Err(error) => {
                let expected = case
                    .get("error")
                    .unwrap_or_else(|| panic!("0.4.7 принял, а Rust отклонил: {text:?} → {error}"));
                m.check_eq(
                    (error.code.as_str().to_owned(), error.message),
                    (
                        str_of(&expected["code"]).to_owned(),
                        str_of(&expected["message"]).to_owned(),
                    ),
                    format!("parse({text:?})"),
                );
            }
        }
    }
    m.finish("vdf.parse");
}

#[test]
fn get_insensitive_matches_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["getInsensitive"]) {
        let text = str_of(&case["text"]);
        let tree = VdfValue::Object(vdf::parse(text).expect("корпус поиска разбирается"));
        let mut value = Some(&tree);
        for key in items(&case["keys"]) {
            value = get_insensitive_in(value, str_of(key));
        }
        let found = value.is_some();
        m.check_eq(
            found,
            case["found"].as_bool().unwrap(),
            format!("getInsensitive {} в {text:?}", case["keys"]),
        );
        if let Some(value) = value {
            m.check_eq(
                dump(value),
                dump_expected(&case["value"]),
                format!("getInsensitive {} в {text:?}", case["keys"]),
            );
        }
    }
    m.finish("vdf.getInsensitive");
}

#[test]
fn nothing_has_no_keys() {
    for case in items(&FIXTURE["getInsensitiveOfNothing"]) {
        assert_eq!(case["result"], true, "0.4.7: {}", case["object"]);
    }
    assert!(get_insensitive_in(None, "a").is_none());
    assert!(get_insensitive_in(Some(&VdfValue::String("abc".into())), "a").is_none());
}

#[test]
fn max_depth_is_the_one_of_0_4_7() {
    assert_eq!(FIXTURE["maxDepth"], vdf::MAX_DEPTH);
}
