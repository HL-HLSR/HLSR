//! Коды внешнего вида против 0.4.7: фикстура `personalization-code.json` из
//! `tools/reference/delivery/personalization-code.js` (`personalization-code.js`,
//! `shared/personalization-code.js`).
//!
//! Байты сжатия у zlib Node и у Rust различаются, поэтому сверяется не код, а то, что из него следует:
//! полезная нагрузка до сжатия (побайтно), `decode(код Node)`, `decode(encode(x))` и отказы `decode` на
//! кодах с верной суммой (подделки, бомбы, лишние данные, обрыв потока).
//!
//! Два отличия 0.4.9 (проверены прогоном 0.4.9 на этой фикстуре): чужой `gh_spray_file` при чтении
//! обнуляется, а пробелы после JSON — место модов, и без модов в них код отклоняется.

mod support;

use hlsr_core::json_text::stringify;
use hlsr_delivery::personalization_code::{self as pc, CodeError};
use serde_json::{Value, json};
use std::sync::LazyLock;
use support::{Mismatches, check_summary, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("personalization-code.json"));

fn ser(value: &Value) -> String {
    stringify(value, "")
}

/// Подделки, которые 0.4.7 читала, а 0.4.9 отклоняет: пробелы после JSON — не моды.
const WHITESPACE_TAIL_0_4_9: [&str; 5] = [
    "подделка: пробелы после JSON",
    "вывод 32767 байт JSON с пробелами",
    "вывод 32768 байт JSON с пробелами",
    "вывод 16384 байт JSON с пробелами",
    "вывод 16385 байт JSON с пробелами",
];

/// Вид из `decode` со спреем, как в нагрузке: 0.4.9 обнуляет чужой спрей, 0.4.7 его оставляла.
fn with_spray_of(mut look: Value, payload: &[u8]) -> Value {
    let payload: Value = serde_json::from_slice(payload).unwrap();
    if let (Some(spray), Some(extra)) = (
        payload.pointer("/personalization/extra/gh_spray_file"),
        look.pointer_mut("/personalization/extra"),
    ) {
        extra["gh_spray_file"] = spray.clone();
    }
    look
}

/// Вход `decode` из случая: значение или длинная строка, записанная описанием.
fn input_of(case: &Value) -> Value {
    match case.get("inputRepeat") {
        Some(spec) => {
            let count = spec["count"].as_u64().unwrap() as usize;
            Value::String(format!(
                "{}{}{}",
                str_of(&spec["prefix"]),
                str_of(&spec["piece"]).repeat(count),
                str_of(&spec["suffix"])
            ))
        }
        None => case["input"].clone(),
    }
}

#[test]
fn constants_match_0_4_7() {
    let c = &FIXTURE["constants"];
    assert_eq!(c["PREFIX"], json!(pc::PREFIX));
    assert_eq!(c["MAX_CODE_LENGTH"], json!(pc::MAX_CODE_LENGTH));
    assert_eq!(c["MAX_PAYLOAD_BYTES"], json!(pc::MAX_PAYLOAD_BYTES));
    assert_eq!(
        c["moduleLimits"]["MAX_CODE_LENGTH"],
        json!(pc::MAX_CODE_LENGTH)
    );
    assert_eq!(
        c["moduleLimits"]["MAX_PAYLOAD_BYTES"],
        json!(pc::MAX_PAYLOAD_BYTES)
    );
    let ids: serde_json::Map<String, Value> = pc::GAME_IDS
        .iter()
        .map(|(id, game)| ((*id).to_owned(), json!(game)))
        .collect();
    assert_eq!(Value::Object(ids), c["GAME_IDS"]);
}

#[test]
fn payload_decode_of_node_code_and_roundtrip_match_0_4_7() {
    let mut m = Mismatches::default();
    let mut identical = 0;
    let mut total = 0;
    let mut rust_codes = Vec::new();
    for (index, case) in items(&FIXTURE["encode"]).iter().enumerate() {
        let id = str_of(&case["id"]);
        let input = &case["input"];
        let context = format!("{index}: {id} {}", ser(input));
        if let Some(error) = case["error"].as_str() {
            m.check_eq(
                pc::encode(id, input).err().map(CodeError::code),
                Some(error),
                format!("encode, отказ ({context})"),
            );
            continue;
        }
        // Полезная нагрузка до сжатия — те же байты, что у Node.
        let payload = pc::payload(id, input).expect("нагрузка");
        if let Err(message) = check_summary(
            &String::from_utf8(payload.clone()).unwrap(),
            &case["payload"],
        ) {
            m.check_eq(message, String::new(), format!("нагрузка ({context})"));
        }
        // decode(код Node) — тот же вид.
        let node_code = str_of(&case["code"]);
        match pc::decode(id, &json!(node_code)) {
            Ok(look) => {
                let look = with_spray_of(look.to_value(), &payload);
                if let Err(message) = check_summary(&ser(&look), &case["look"]) {
                    m.check_eq(
                        message,
                        String::new(),
                        format!("decode(код Node) ({context})"),
                    );
                }
            }
            Err(error) => m.check_eq(
                error.code().to_owned(),
                String::new(),
                format!("decode(код Node) отказал ({context})"),
            ),
        }
        // decode(encode(x)) — тот же вид.
        let own = pc::encode(id, input).expect("код");
        rust_codes.push(json!({"id": id, "code": own, "nodeCode": node_code}));
        total += 1;
        identical += usize::from(own == node_code);
        match pc::decode(id, &json!(own)) {
            Ok(look) => {
                let look = with_spray_of(look.to_value(), &payload);
                if let Err(message) = check_summary(&ser(&look), &case["look"]) {
                    m.check_eq(
                        message,
                        String::new(),
                        format!("decode(encode(x)) ({context})"),
                    );
                }
            }
            Err(error) => m.check_eq(
                error.code().to_owned(),
                String::new(),
                format!("decode(encode(x)) отказал ({context})"),
            ),
        }
    }
    m.finish("encode / decode");
    // Байты сжатия могут расходиться с zlib Node; код при этом остаётся верным.
    eprintln!("коды, побайтно равные кодам Node: {identical} из {total}");
    // Цепочка паритета (scripts/slint/parity.sh): коды Rust читает 0.4.7 —
    // tools/reference/delivery/personalization-code-rust.js.
    if let Some(out) = std::env::var_os("HLSR_RUST_CODES_OUT") {
        std::fs::write(out, stringify(&Value::Array(rust_codes), "  ")).unwrap();
    }
}

#[test]
fn decode_matches_0_4_7_on_forged_and_damaged_codes() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["decode"]) {
        let id = str_of(&case["id"]);
        let input = input_of(case);
        let name = str_of(&case["name"]);
        let context = format!("{name} ({id})");
        let expected = if WHITESPACE_TAIL_0_4_9.contains(&name) {
            Some("PERSONALIZATION_CODE_INVALID")
        } else {
            case["error"].as_str()
        };
        match (pc::decode(id, &input), expected) {
            (Ok(look), None) => {
                if let Err(message) = check_summary(&ser(&look.to_value()), &case["look"]) {
                    m.check_eq(message, String::new(), context);
                }
            }
            (Ok(_), Some(error)) => m.check_eq("успех", error, context),
            (Err(error), None) => m.check_eq(error.code(), "успех", context),
            (Err(error), Some(expected)) => m.check_eq(error.code(), expected, context),
        }
    }
    m.finish("decode");
}
