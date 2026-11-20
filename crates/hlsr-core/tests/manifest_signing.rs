//! Подпись Ed25519 в `hlsr-core` (Р7 подпроекта 6): та же логика, что `manifest-signing.js`,
//! без ошибки доставки. Сверка с 0.4.7 на корпусе — `crates/hlsr-delivery/tests/signing.rs`
//! (фикстура `signing.json`, через реэкспорт `hlsr_delivery::manifest_signing`).

use hlsr_core::manifest_signing::{
    ALGORITHM, CheckOptions, Reason, SIGNATURE_FIELD, SigningError, attach, canonicalize, check,
    generate_key_pair, key_pair_from_seed, sign, verify,
};
use serde_json::{Value, json};

fn document() -> Value {
    json!({"version": "0.5.0", "b": [1, {"z": null, "a": true}], "a": "значение"})
}

#[test]
fn canonical_form_sorts_keys_and_drops_signatures_at_any_depth() {
    let value =
        json!({"b": 1, "signature": "x", "a": {"signature": 2, "c": [ {"signature": 3, "d": 4} ]}});
    assert_eq!(canonicalize(&value), r#"{"a":{"c":[{"d":4}]},"b":1}"#);
    assert_eq!(SIGNATURE_FIELD, "signature");
    assert_eq!(ALGORITHM, "ed25519");
}

#[test]
fn sign_and_verify_round_trip_with_a_generated_key() {
    let keys = generate_key_pair();
    let signature = sign(&document(), &keys.private_key_pem).unwrap();
    assert_eq!(signature["algorithm"], "ed25519");
    assert!(verify(&document(), &signature, &keys.public_key_pem));
    // Ed25519 детерминирована.
    assert_eq!(signature, sign(&document(), &keys.private_key_pem).unwrap());
    // Любая правка документа ломает подпись.
    let mut changed = document();
    changed["version"] = json!("0.5.1");
    assert!(!verify(&changed, &signature, &keys.public_key_pem));
    // Чужой ключ не подходит.
    let other = generate_key_pair();
    assert!(!verify(&document(), &signature, &other.public_key_pem));
}

#[test]
fn attach_replaces_an_old_signature_and_appends_the_new_one_last() {
    let keys = key_pair_from_seed(&[1u8; 32]);
    let mut doc = document().as_object().unwrap().clone();
    doc.insert(
        "signature".to_owned(),
        json!({"algorithm": "ed25519", "value": "old"}),
    );
    let signed = attach(&doc, &keys.private_key_pem).unwrap();
    assert_eq!(
        signed.keys().map(String::as_str).collect::<Vec<_>>(),
        ["version", "b", "a", "signature"]
    );
    assert!(verify(
        &Value::Object(signed.clone()),
        &signed["signature"],
        &keys.public_key_pem
    ));
}

#[test]
fn check_reports_the_reason_and_required_turns_it_into_an_error() {
    let keys = key_pair_from_seed(&[2u8; 32]);
    let other = key_pair_from_seed(&[3u8; 32]);
    let signed =
        Value::Object(attach(document().as_object().unwrap(), &keys.private_key_pem).unwrap());
    let trusted = [keys.public_key_pem.clone()];
    let strangers = [other.public_key_pem.clone()];

    let good = check(
        &signed,
        &CheckOptions {
            required: true,
            public_keys: &trusted,
        },
    )
    .unwrap();
    assert!(good.signed && good.valid && good.reason.is_none());

    let bad = check(
        &signed,
        &CheckOptions {
            required: false,
            public_keys: &strangers,
        },
    )
    .unwrap();
    assert!(bad.signed && !bad.valid);
    assert_eq!(bad.reason, Some(Reason::BadSignature));
    assert_eq!(
        check(
            &signed,
            &CheckOptions {
                required: true,
                public_keys: &strangers
            }
        ),
        Err(SigningError(
            "Подпись манифеста не прошла проверку".to_owned()
        ))
    );

    let none = check(
        &signed,
        &CheckOptions {
            required: false,
            public_keys: &[],
        },
    )
    .unwrap();
    assert_eq!(none.reason, Some(Reason::NoTrustedKeys));
    assert_eq!(
        check(
            &signed,
            &CheckOptions {
                required: true,
                public_keys: &[]
            }
        ),
        Err(SigningError(
            "Нет доверенных публичных ключей — подпись проверить нечем".to_owned()
        ))
    );

    let unsigned = check(
        &document(),
        &CheckOptions {
            required: false,
            public_keys: &trusted,
        },
    )
    .unwrap();
    assert!(!unsigned.signed);
    assert_eq!(unsigned.reason, Some(Reason::Unsigned));
    assert_eq!(
        check(
            &document(),
            &CheckOptions {
                required: true,
                public_keys: &trusted
            }
        ),
        Err(SigningError(
            "Манифест без подписи отклонён: включена обязательная проверка подписи".to_owned()
        ))
    );
}

#[test]
fn a_foreign_private_key_is_an_error_with_a_reason() {
    let error = sign(&document(), "garbage").unwrap_err();
    assert_eq!(error.0, "Некорректный приватный ключ: это не PEM");
    let keys = key_pair_from_seed(&[4u8; 32]);
    let error = sign(&document(), &keys.public_key_pem).unwrap_err();
    assert_eq!(
        error.0,
        "Некорректный приватный ключ: метка PEM «PUBLIC KEY»"
    );
}
