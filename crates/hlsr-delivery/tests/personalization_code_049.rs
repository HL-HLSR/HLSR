//! Коды внешнего вида против 0.4.9: фикстура `personalization-code-049.json` из
//! `tools/reference/delivery/personalization-code-049.js` — моды после JSON (`hide`), обнуление
//! чужого спрея, отказы на испорченном хвосте.
//!
//! Нагрузка сверяется побайтно (JSON — длиной и sha256, хвост с модами — целиком), вид — по
//! `decode(код Node)` и `decode(encode(x))`. Коды Rust для проверки в Node (0.4.8 и 0.4.9 читают
//! коды Rust): `HLSR_RUST_CODES_049_OUT=<файл>` → `tools/reference/delivery/personalization-code-rust-049.js`.

mod support;

use hlsr_core::json_text::stringify;
use hlsr_delivery::personalization_code as pc;
use serde_json::{Value, json};
use std::sync::LazyLock;
use support::{Mismatches, check_summary, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("personalization-code-049.json"));

fn decoded(id: &str, code: &Value) -> Value {
    match pc::decode(id, code) {
        Ok(look) => json!({"look": look.to_value()}),
        Err(error) => json!({"error": error.code()}),
    }
}

fn expected(case: &Value) -> Value {
    match case.get("error") {
        Some(error) => json!({"error": error}),
        None => json!({"look": case["look"]}),
    }
}

#[test]
fn payload_with_hidden_mods_and_decode_match_0_4_9() {
    let mut m = Mismatches::default();
    let mut rust_codes = Vec::new();
    for (index, case) in items(&FIXTURE["encode"]).iter().enumerate() {
        let id = str_of(&case["id"]);
        let input = &case["input"];
        let context = format!("{index}: {id} {}", stringify(input, ""));
        let payload = String::from_utf8(pc::payload(id, input).unwrap()).unwrap();
        let at = payload.rfind('}').unwrap() + 1;
        if let Err(message) = check_summary(&payload[..at], &case["payload"]) {
            m.check_eq(message, String::new(), format!("JSON ({context})"));
        }
        m.check_eq(
            &payload[at..],
            str_of(&case["tail"]),
            format!("хвост ({context})"),
        );
        m.check(
            &decoded(id, &case["code"]),
            &expected(case),
            format!("decode(код Node) ({context})"),
        );
        let own = pc::encode(id, input).unwrap();
        m.check(
            &decoded(id, &json!(own)),
            &expected(case),
            format!("decode(encode(x)) ({context})"),
        );
        rust_codes.push(json!({"id": id, "input": input, "code": own}));
    }
    m.finish("encode / decode 0.4.9");
    if let Some(out) = std::env::var_os("HLSR_RUST_CODES_049_OUT") {
        std::fs::write(out, stringify(&Value::Array(rust_codes), "  ")).unwrap();
    }
}

#[test]
fn decode_of_forged_tails_matches_0_4_9() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["decode"]) {
        let id = str_of(&case["id"]);
        m.check(
            &decoded(id, &case["code"]),
            &expected(case),
            str_of(&case["name"]),
        );
    }
    m.finish("decode 0.4.9");
}

#[test]
fn hide_is_two_bits_per_whitespace_character() {
    // `{` = 0x7B = 01 11 10 11, `}` = 0x7D = 01 11 11 01.
    assert_eq!(pc::hide(&json!({})), "\t\r\n\r\t\r\r\t");
    assert_eq!(pc::hide(&json!("A")).len(), 3 * 4);
}
