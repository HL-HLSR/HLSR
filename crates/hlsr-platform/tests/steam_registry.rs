//! Значения реестра, как их видел 0.4.7 через `reg query` (REG_SZ, REG_EXPAND_SZ, REG_DWORD):
//! фикстура `registry.json` из `tools/reference/platform/registry.js`. Здесь значение приходит
//! типизированным (порт `Registry`), а 0.4.7 разбирал текст; сверяется итог для лаунчера.
//! Настоящий реестр не читается: проверка `SystemRegistry` на настоящем Steam — в чек-листе.

mod support;

use hlsr_delivery::ports::{FakeRegistry, RegistryHive, RegistryValue};
use hlsr_platform::steam_locator::{RegData, read_registry_value};
use serde_json::{Value, json};
use std::sync::LazyLock;
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("registry.json"));

const KEY: &str = "Software\\Valve\\Steam";

/// Значение, как его отдаёт реестр: тип и данные.
fn typed(case: &Value) -> RegistryValue {
    match str_of(&case["type"]) {
        "REG_SZ" => RegistryValue::Sz(str_of(&case["data"]).to_owned()),
        "REG_EXPAND_SZ" => RegistryValue::ExpandSz(str_of(&case["data"]).to_owned()),
        "REG_DWORD" => RegistryValue::Dword(case["data"].as_u64().unwrap() as u32),
        other => panic!("тип {other}"),
    }
}

/// `readRegistryValue`: число, строка или `null`.
fn as_json(data: RegData) -> Value {
    match data {
        RegData::Missing => Value::Null,
        RegData::Number(number) => json!(number as u64),
        RegData::Text(text) => json!(text),
    }
}

#[test]
fn registry_values_match_what_0_4_7_read_from_reg_query() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["cases"]) {
        let fake = FakeRegistry::default();
        fake.set(
            RegistryHive::CurrentUser,
            KEY,
            str_of(&case["name"]),
            typed(case),
        );
        m.check(
            &as_json(read_registry_value(&fake, KEY, str_of(&case["name"]))),
            &case["result"],
            format!("{} {}", case["type"], case["data"]),
        );
    }
    m.finish("readRegistryValue");
}

#[test]
fn names_and_keys_ignore_case_and_must_match_whole() {
    let fake = FakeRegistry::default();
    fake.set(
        RegistryHive::CurrentUser,
        KEY,
        "SteamPath",
        RegistryValue::Sz("value".into()),
    );
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["names"]) {
        let key = if case["otherKey"].as_bool() == Some(true) {
            "Software\\Valve\\Other"
        } else if case["upperKey"].as_bool() == Some(true) {
            "SOFTWARE\\VALVE\\STEAM"
        } else {
            KEY
        };
        m.check(
            &as_json(read_registry_value(&fake, key, str_of(&case["asked"]))),
            &case["result"],
            format!("{}", case),
        );
    }
    m.finish("имя значения");
}
