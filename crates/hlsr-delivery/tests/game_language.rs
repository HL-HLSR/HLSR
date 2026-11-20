//! Язык игры на диске против 0.4.7: фикстура `game-language.json` из `tools/reference/delivery/game-language.js`.
//! Сценарий — данные (файлы и шаги): `apply`, `clear`, `availableLanguages`, `overlayRoot` и
//! `engineLanguage` 0.4.7 в Node 24 и этот крейт выполняют его на одинаковых деревьях во временных
//! папках; сравниваются итог каждого шага и дерево файлов (путь, размер, sha256, текст latin1). Язык
//! Steam из реестра в Node подменён `options.run`, который отдаёт вывод `reg query`; здесь тот же
//! вывод строит `FakeRegistry` (`ports::reg_query_output`). Тексты системных ошибок диска
//! (`message` при `REMOVE_FAILED`, `WRITE_FAILED`) не воспроизводятся: сверяется причина.
//!
//! Сценарии, помеченные `unix`, зависят от ОС (права, ссылки) и на Windows пропускаются.

mod support;

use hlsr_core::js::Platform;
use hlsr_delivery::cfg_search::LanguageOption;
use hlsr_delivery::errors::DeliveryError;
use hlsr_delivery::game_language::{
    self as language, ApplyOptions, EngineOptions, FILE_SETS, apply, available_languages, clear,
    engine_language, is_language, normalize_choice, overlay_root,
};
use hlsr_delivery::ports::{FakeRegistry, RegistryHive, RegistryValue};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use support::scenario::{self, common_step, normalize_text, path_in, temp_root};
use support::{Mismatches, fixture, items, opt_str, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("game-language.json"));

const MOD: &str = "mymod";

/// Реестр из описания шага: `{type, data}` или `null` («reg.exe завершился с ошибкой»).
fn registry_of(reg: &Value) -> FakeRegistry {
    let registry = FakeRegistry::default();
    if reg.is_object() {
        let data = str_of(&reg["data"]).to_owned();
        let value = match str_of(&reg["type"]) {
            "REG_SZ" => RegistryValue::Sz(data),
            "REG_EXPAND_SZ" => RegistryValue::ExpandSz(data),
            "REG_DWORD" => RegistryValue::Dword(
                u32::from_str_radix(data.trim_start_matches("0x"), 16).expect("число REG_DWORD"),
            ),
            other => panic!("тип реестра {other}"),
        };
        registry.set(
            RegistryHive::CurrentUser,
            "Software\\Valve\\Source",
            "Language",
            value,
        );
    }
    registry
}

/// Платформа шага; без неё — Linux, где снята фикстура.
fn platform_of(options: &Value) -> Platform {
    match opt_str(&options["platform"]) {
        Some("win32") => Platform::Win32,
        _ => Platform::Linux,
    }
}

fn error_value(error: &DeliveryError) -> Value {
    json!({"error": {
        "code": error.code.as_str(),
        "message": error.message,
        "details": error.details,
    }})
}

/// Результат вызова, как его строит скрипт: `{ok}` или `{error}`.
fn call(root: &Path, result: Result<Value, DeliveryError>) -> Value {
    let value = match result {
        Ok(ok) => json!({"ok": ok}),
        Err(error) => error_value(&error),
    };
    normalize(root, &value)
}

fn normalize(root: &Path, value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(normalize_text(root, text)),
        Value::Array(list) => Value::Array(list.iter().map(|v| normalize(root, v)).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(k, v)| (k.clone(), normalize(root, v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Тексты ошибок диска не сверяются: у результата с причиной сбоя остаётся только она.
fn without_system_text(mut value: Value) -> Value {
    let failed = matches!(
        value["ok"]["reason"].as_str(),
        Some("REMOVE_FAILED" | "WRITE_FAILED" | "WRITE_FAILED_DIRTY")
    );
    if failed {
        value["ok"]["message"] = json!("<системный текст>");
    }
    if value["ok"]["removed"] == json!(false) {
        value["ok"]["message"] = json!("<системный текст>");
    }
    value
}

fn step(root: &Path, step: &Value) -> Value {
    if let Some(common) = common_step(root, step) {
        return common;
    }
    let install = path_in(root, opt_str(&step["installDir"]).unwrap_or("game"));
    let game_directory = opt_str(&step["gameDirectory"]).unwrap_or(MOD);
    let options = &step["options"];
    match str_of(&step["op"]) {
        "apply" => {
            let registry = registry_of(&options["reg"]);
            let apply_options = ApplyOptions {
                engine_language: match options.get("engineLanguage") {
                    Some(value) => LanguageOption::Given(value.as_str()),
                    None => LanguageOption::Detect,
                },
                engine: EngineOptions {
                    platform: platform_of(options),
                    default_language: opt_str(&options["defaultLanguage"]),
                    registry: &registry,
                },
            };
            let result = apply(
                &install,
                game_directory,
                opt_str(&step["choice"]),
                &apply_options,
            );
            without_system_text(call(root, result.map(|applied| applied.to_value())))
        }
        "clear" => without_system_text(call(
            root,
            clear(&install, game_directory).map(|cleared| cleared.to_value()),
        )),
        "available" => call(
            root,
            Ok(json!(available_languages(&install, game_directory))),
        ),
        "root" => call(
            root,
            overlay_root(&install, game_directory).map(|path| json!(path.to_string_lossy())),
        ),
        "engine" => {
            let registry = registry_of(&options["reg"]);
            let found = engine_language(&EngineOptions {
                platform: platform_of(options),
                default_language: opt_str(&options["defaultLanguage"]),
                registry: &registry,
            });
            call(root, Ok(json!(found)))
        }
        other => panic!("неизвестный шаг {other}"),
    }
}

#[test]
fn language_on_scenarios_against_0_4_7() {
    let scenarios = items(&FIXTURE["scenarios"]);
    assert!(scenarios.len() > 50, "сценариев: {}", scenarios.len());
    let mut mismatches = Mismatches::default();
    for scenario in scenarios {
        let name = str_of(&scenario["name"]);
        if scenario["unix"].as_bool().unwrap_or(false) && cfg!(windows) {
            continue;
        }
        if name.starts_with("права") && scenario::is_root_user() {
            continue;
        }
        let (_temp, root) = temp_root();
        scenario::setup(&root, &scenario["setup"]);
        assert_eq!(
            items(&scenario["steps"]).len(),
            items(&scenario["results"]).len(),
            "шагов и результатов поровну"
        );
        for (index, spec) in items(&scenario["steps"]).iter().enumerate() {
            let actual = step(&root, spec);
            let expected = without_system_text(scenario["results"][index].clone());
            mismatches.check(
                &actual,
                &expected,
                format!("{name}: шаг {index} {}", spec["op"]),
            );
        }
        scenario::open_up(&root);
    }
    mismatches.finish("game-language: сценарии");
}

#[test]
fn constants_and_choice_checks_are_the_ones_of_0_4_7() {
    let constants = &FIXTURE["constants"];
    assert_eq!(language::AUTO, str_of(&constants["AUTO"]));
    assert_eq!(language::ENGLISH, str_of(&constants["ENGLISH"]));
    assert_eq!(language::OVERLAY_DIR, str_of(&constants["OVERLAY_DIR"]));
    assert_eq!(language::REGISTRY_KEY, str_of(&constants["REGISTRY_KEY"]));
    assert_eq!(
        language::REGISTRY_VALUE,
        str_of(&constants["REGISTRY_VALUE"])
    );
    let sets: Vec<Value> = FILE_SETS
        .iter()
        .map(|set| json!({"base": set.base, "sources": set.sources}))
        .collect();
    assert_eq!(Value::Array(sets), constants["FILE_SETS"]);

    let mut mismatches = Mismatches::default();
    for sample in items(&FIXTURE["samples"]) {
        let value = sample["value"].as_str();
        mismatches.check(
            &json!(value.is_some_and(is_language)),
            &sample["isLanguage"],
            format!("isLanguage({})", sample["value"]),
        );
        mismatches.check(
            &json!(normalize_choice(value)),
            &sample["normalizeChoice"],
            format!("normalizeChoice({})", sample["value"]),
        );
    }
    mismatches.finish("game-language: выбор языка");
}

#[test]
fn the_overlay_is_rewritten_with_the_readme_and_only_the_planned_files() {
    let (_temp, root) = temp_root();
    let install: PathBuf = path_in(&root, "game");
    let resource = path_in(&install, "mymod/resource");
    std::fs::create_dir_all(&resource).unwrap();
    std::fs::write(resource.join("gameui_russian.txt"), "ru").unwrap();
    let options = ApplyOptions {
        engine_language: LanguageOption::Given(Some("english")),
        ..ApplyOptions::default()
    };
    let applied = apply(&install, MOD, Some("russian"), &options).unwrap();
    assert!(applied.ok && applied.applied);
    assert_eq!(applied.files, ["gameui_english.txt"]);
    let overlay = overlay_root(&install, MOD).unwrap();
    assert_eq!(
        std::fs::read_to_string(overlay.join("resource/gameui_english.txt")).unwrap(),
        "ru"
    );
    let readme = std::fs::read_to_string(overlay.join("README.txt")).unwrap();
    assert!(readme.starts_with("Этой папкой распоряжается лаунчер HLSR.\r\n\r\n"));
    assert!(readme.ends_with("в соседнюю папку внутри custom/.\r\n"));
}
