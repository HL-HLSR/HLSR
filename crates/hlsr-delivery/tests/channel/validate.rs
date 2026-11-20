//! Проверка документа канала (`ChannelService.validate`) и `repairArchiveFor` на корпусе:
//! подпись тестовым ключом, схема ajv, игра, имя канала, порядок проверок.

use super::scenario::{FIXTURE, check, delivery_json, ok};
use super::support::{Mismatches, items, str_of};
use hlsr_delivery::channel_config::{ChannelConfig, QA_CHANNEL_ENV, QA_KEY_ENV, QA_URL_ENV};
use hlsr_delivery::channel_service::{ChannelOptions, ChannelService, SUPPORTED_SCHEMA_VERSION};
use hlsr_delivery::files_manifest_store::{MAX_MANIFEST_BYTES, META_VERSION};
use hlsr_delivery::manifest_signing::key_pair_from_seed;
use hlsr_delivery::update_checker::MIN_INTERVAL_MS;
use serde_json::{Value, json};

fn hex_bytes(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

#[test]
fn constants_match_0_4_7() {
    let constants = &FIXTURE["constants"];
    assert_eq!(
        constants["SUPPORTED_SCHEMA_VERSION"],
        json!(SUPPORTED_SCHEMA_VERSION)
    );
    assert_eq!(constants["MAX_MANIFEST_BYTES"], json!(MAX_MANIFEST_BYTES));
    assert_eq!(constants["META_VERSION"], json!(META_VERSION));
    assert_eq!(constants["MIN_INTERVAL_MS"], json!(MIN_INTERVAL_MS));
    assert_eq!(constants["QA_URL_ENV"], json!(QA_URL_ENV));
    assert_eq!(constants["QA_KEY_ENV"], json!(QA_KEY_ENV));
    assert_eq!(constants["QA_CHANNEL_ENV"], json!(QA_CHANNEL_ENV));
}

/// Ключи сценариев выводятся из зерна: PEM из зерна совпадает с записанным Node.
#[test]
fn test_keys_come_from_seeds() {
    for name in ["A", "B"] {
        let key = &FIXTURE["keys"][name];
        let seed = hex_bytes(str_of(&key["seed"]));
        let pair = key_pair_from_seed(seed.as_slice().try_into().unwrap());
        assert_eq!(
            json!(pair.public_key_pem),
            key["publicKeyPem"],
            "ключ {name}"
        );
        assert_eq!(
            json!(pair.private_key_pem),
            key["privateKeyPem"],
            "ключ {name}"
        );
    }
}

/// `{channel, requireSignature, keys: [имена]}` из фикстуры → настройки канала.
fn config_of(game_id: &str, spec: &Value) -> ChannelConfig {
    ChannelConfig {
        game_id: game_id.to_owned(),
        url: None,
        channel: str_of(&spec["channel"]).to_owned(),
        require_signature: spec["requireSignature"].as_bool().unwrap(),
        public_keys: items(&spec["keys"])
            .iter()
            .map(|name| str_of(&FIXTURE["keys"][str_of(name)]["publicKeyPem"]).to_owned())
            .collect(),
        source: None,
    }
}

#[test]
fn validate_matches_0_4_7() {
    let service = ChannelService::new(ChannelOptions::default());
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["validate"]) {
        let game_id = str_of(&case["gameId"]);
        let config = config_of(game_id, &case["config"]);
        let actual = match service.validate(&case["doc"], game_id, &config) {
            Ok(_) => ok(json!({ "valid": true })),
            Err(error) => delivery_json(&error),
        };
        check(
            &mut m,
            &actual,
            &case["result"],
            format!("validate: {}", case["name"]),
        );
    }
    m.finish("ChannelService.validate");
}
