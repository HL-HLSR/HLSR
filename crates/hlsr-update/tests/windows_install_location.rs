//! Каталог установки в реестре против 0.4.7: фикстура `windows-install-location.json` из
//! `tools/reference/update/windows-install-location.js` (`install-location.js` на подмене `reg.exe`
//! над моделью реестра). Здесь та же модель — `FakeRegistry`.

mod support;

use hlsr_core::logger::MemoryLogger;
use hlsr_update::windows::install_location::{self, INSTALL_ROOT, Options, Status, UNINSTALL_ROOT};
use hlsr_update::windows::registry::{FakeRegistry, Registry};
use serde_json::{Value, json};
use std::sync::LazyLock;
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("windows-install-location.json"));

#[test]
fn constants_match_node() {
    let constants = &FIXTURE["constants"];
    assert_eq!(
        install_location::INSTALL_ROOT,
        str_of(&constants["INSTALL_ROOT"])
    );
    // 0.4.7 запрашивает `reg query HKCU\…`, а печатает `HKEY_CURRENT_USER\…`: корень тот же.
    assert_eq!(
        hlsr_update::windows::registry::full_key(UNINSTALL_ROOT).unwrap(),
        str_of(&constants["UNINSTALL_ROOT"]).replacen("HKCU", "HKEY_CURRENT_USER", 1)
    );
    assert!(INSTALL_ROOT.starts_with("HKEY_CURRENT_USER"));
}

#[test]
fn install_key_from_matches_node() {
    let cases = items(&FIXTURE["installKeyFrom"]);
    assert!(cases.len() >= 25);
    let mut mismatches = Mismatches::default();
    for case in cases {
        let input = case["input"].as_str().unwrap_or("");
        mismatches.check_eq(
            install_location::install_key_from(input),
            case["result"].as_str().map(str::to_owned),
            format!("installKeyFrom({input:?})"),
        );
    }
    mismatches.finish("install-location.installKeyFrom");
}

#[test]
fn executable_from_uninstall_string_matches_node() {
    let cases = items(&FIXTURE["executableFromUninstallString"]);
    assert!(cases.len() >= 35);
    let mut mismatches = Mismatches::default();
    for case in cases {
        // `String(value || "")`: null, undefined и число 5 в корпусе не дают пути.
        let input = case["input"].as_str().unwrap_or("");
        mismatches.check_eq(
            install_location::executable_from_uninstall_string(input),
            case["result"].as_str().map(str::to_owned),
            format!("executableFromUninstallString({input:?})"),
        );
    }
    mismatches.finish("install-location.executableFromUninstallString");
}

/// Модель фикстуры (`{раздел: {имя: {type, data}}}`) — реестр-подделка.
fn registry_of(model: &Value) -> FakeRegistry {
    FakeRegistry::from_json(model).expect("модель реестра")
}

/// Имена значений без учёта регистра, как в `dump()` скрипта.
fn lowered(model: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for (key, values) in model.as_object().unwrap() {
        let mut map = serde_json::Map::new();
        for (name, value) in values.as_object().unwrap() {
            map.insert(name.to_lowercase(), value.clone());
        }
        out.insert(key.clone(), Value::Object(map));
    }
    Value::Object(out)
}

#[test]
fn ensure_install_location_matches_node() {
    let cases = items(&FIXTURE["ensure"]);
    assert!(cases.len() >= 35);
    let mut mismatches = Mismatches::default();
    for case in cases {
        let label = str_of(&case["label"]);
        let registry = registry_of(&case["model"]);
        if case["addFails"].as_bool().unwrap() {
            registry.set_fail_writes(true);
        }
        let logger = MemoryLogger::default();
        let options = Options {
            install_dir: case["installDir"].as_str().unwrap_or(""),
            uninstall_key: case["uninstallKey"].as_str(),
            key: case["key"].as_str(),
        };
        let outcome = install_location::ensure_install_location(&registry, &options, &logger);

        let mut result = serde_json::Map::new();
        result.insert("status".into(), json!(outcome.status.as_str()));
        if let Some(key) = &outcome.key {
            result.insert("key".into(), json!(key));
        }
        if let Some(value) = &outcome.value {
            result.insert("value".into(), json!(value));
        }
        mismatches.check(
            &Value::Object(result),
            &case["result"],
            format!("{label}: итог"),
        );

        let logs: Vec<Value> = logger
            .take()
            .into_iter()
            .map(|(level, message)| json!([level, message]))
            .collect();
        mismatches.check(
            &Value::Array(logs),
            &case["logs"],
            format!("{label}: журнал"),
        );

        // Запись `reg add` не стирает прочие значения раздела.
        registry.set_fail_writes(false);
        mismatches.check(
            &lowered(&registry.to_json()),
            &case["after"],
            format!("{label}: реестр после"),
        );
    }
    mismatches.finish("install-location.ensureInstallLocation");
}

#[test]
fn find_and_read_match_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["finds"]) {
        let label = str_of(&case["label"]);
        let model = items(&FIXTURE["ensure"])
            .iter()
            .find(|item| str_of(&item["label"]) == label)
            .map(|item| item["model"].clone())
            .unwrap();
        let registry = registry_of(&model);
        mismatches.check_eq(
            install_location::find_install_key(&registry, str_of(&case["installDir"])),
            case["key"].as_str().map(str::to_owned),
            format!("findInstallKey: {label}"),
        );
    }
    for case in items(&FIXTURE["reads"]) {
        let label = str_of(&case["label"]);
        let model = items(&FIXTURE["ensure"])
            .iter()
            .find(|item| str_of(&item["label"]) == label)
            .map(|item| item["model"].clone())
            .unwrap();
        let registry = registry_of(&model);
        let key = str_of(&case["key"]);
        mismatches.check_eq(
            install_location::read_install_location(&registry, key),
            str_of(&case["value"]).to_owned(),
            format!("readInstallLocation({key}): {label}"),
        );
    }
    mismatches.finish("install-location.findInstallKey/readInstallLocation");
}

#[test]
fn status_names_are_the_node_ones() {
    for (status, name) in [
        (Status::Ok, "ok"),
        (Status::Written, "written"),
        (Status::WriteFailed, "write-failed"),
        (Status::NoKey, "no-key"),
        (Status::Skipped, "skipped"),
        (Status::Error, "error"),
    ] {
        assert_eq!(status.as_str(), name);
    }
}

/// Реестр владельца не читается: подделка не знает ни одного его ключа.
#[test]
fn a_fake_registry_knows_nothing_of_real_keys() {
    let registry = FakeRegistry::new();
    assert!(
        registry
            .read("HKCU\\Software\\3f824379-6b41-5725-b424-59cb87f2acae")
            .is_none()
    );
    assert!(registry.subkeys(UNINSTALL_ROOT).is_none());
}
