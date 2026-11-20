//! Каноническая форма канала, подписанного скриптом выпуска: фикстура `signing-canonical.json` из
//! `tools/reference/update/signing-canonical.js`. Канал подписывает Node (`tools/release/node/
//! launcher-release-prepare*.js`: замороженный `manifest-signing.js` 0.4.7), проверяет Rust и 0.4.7;
//! канонический текст обязан совпасть до байта, иначе подпись выпуска у пользователя не сойдётся.
//! Боевые `release/**/launcher-channel.json` — фикстура `signing.json` (`hlsr-delivery`, `release/`
//! на этой машине нет, пересъёмка на Linux-машине).

mod support;

use hlsr_core::json_text::parse_js_json;
use hlsr_core::manifest_signing::{canonicalize, sign, verify};
use serde_json::Value;
use support::{fixture, items, str_of};

#[test]
fn canonical_text_and_signature_match_the_node_release_scripts() {
    let fixture = fixture("signing-canonical.json");
    let private_key = str_of(&fixture["keys"]["privateKeyPem"]);
    let public_key = str_of(&fixture["keys"]["publicKeyPem"]);
    let cases = items(&fixture["cases"]);
    assert!(cases.len() >= 10);
    for case in cases {
        let name = str_of(&case["name"]);
        // Файл так, как его пишет выпуск: JSON.stringify(signed, null, 2) + "\n".
        let signed = parse_js_json(str_of(&case["text"])).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            canonicalize(&signed),
            str_of(&case["canonical"]),
            "{name}: канонический текст"
        );
        // Ed25519 детерминирована: та же подпись, что у Node, а не просто «проверяется».
        assert_eq!(
            sign(&signed, private_key).unwrap(),
            case["signature"],
            "{name}: подпись"
        );
        assert_eq!(signed["signature"], case["signature"], "{name}");
        assert!(
            verify(&signed, &signed["signature"], public_key),
            "{name}: проверка"
        );

        // Любая правка подписанного поля ломает подпись, а правка самой подписи не меняет документ.
        let mut forged = signed.clone();
        forged["version"] = Value::from("0.5.1");
        assert!(
            !verify(&forged, &signed["signature"], public_key),
            "{name}: версия подменена"
        );
        if let Some(build) = forged.get_mut("build") {
            build["executable"]["size"] = Value::from(1);
            assert!(
                !verify(&forged, &signed["signature"], public_key),
                "{name}: блок build подменён"
            );
        }
    }
}

#[test]
fn key_order_in_the_file_does_not_change_the_signed_text() {
    let fixture = fixture("signing-canonical.json");
    let cases = items(&fixture["cases"]);
    let by_name = |wanted: &str| {
        cases
            .iter()
            .find(|case| str_of(&case["name"]) == wanted)
            .unwrap_or_else(|| panic!("нет случая {wanted}"))
    };
    let straight = by_name("windows 0.5.0 with delta and build");
    let reversed = by_name("keys inserted in reverse order");
    assert_ne!(
        str_of(&straight["text"]),
        str_of(&reversed["text"]),
        "файлы различаются порядком ключей"
    );
    assert_eq!(straight["canonical"], reversed["canonical"]);
    assert_eq!(straight["signature"], reversed["signature"]);
}
