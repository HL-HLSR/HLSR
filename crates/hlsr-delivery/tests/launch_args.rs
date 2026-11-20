//! Параметры запуска и настройки игры против 0.4.7: фикстура `launch-args.json` из
//! `tools/reference/delivery/launch-args.js`. Чистые функции `launch-args.js`: константы, `languageOf`,
//! `affinityMaskWithoutCpu0`, `paramNameOf`, `isForbidden`, `isProtectedEnv`, `parseArgsString`,
//! `validateArgsString`, `parseProtonOptions`, `defaultSettings`, `normalizeSettings`, `sanitizeSettings`,
//! `buildUserArguments`, `mergeLaunchArguments`. Порядок ключей объектов сверяется по записи `JSON.stringify`.

mod support;

use hlsr_core::js;
use hlsr_core::json_text::stringify;
use hlsr_delivery::launch_args::{self as la, ArgsError, ProtonOptions};
use serde_json::{Map, Value, json};
use std::sync::LazyLock;
use support::{Mismatches, check_summary, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("launch-args.json"));

/// Запись `JSON.stringify` без отступов: порядок ключей важен.
fn ser(value: &Value) -> String {
    stringify(value, "")
}

/// Список строк; число в списке доверенных аргументов (`[5, "-novid"]`) JS сохраняет числом,
/// Rust держит `String(5)`.
fn strings(value: &Value) -> Vec<String> {
    items(value)
        .iter()
        .map(|s| js::to_template_string(Some(s)))
        .collect()
}

fn args_value(result: &Result<Vec<String>, ArgsError>) -> Value {
    match result {
        Ok(args) => json!({"ok": true, "args": args}),
        Err(error) => json!({"ok": false, "error": error.to_value()}),
    }
}

fn proton_value(result: &Result<ProtonOptions, ArgsError>) -> Value {
    match result {
        Ok(options) => {
            let mut object = Map::new();
            object.insert("ok".into(), json!(true));
            object.insert("env".into(), options.env_value());
            object.insert("wrapper".into(), json!(options.wrapper));
            object.insert("args".into(), json!(options.args));
            Value::Object(object)
        }
        Err(error) => json!({"ok": false, "error": error.to_value()}),
    }
}

#[test]
fn constants_match_0_4_7() {
    let c = &FIXTURE["constants"];
    assert_eq!(c["MAX_ARGS_LENGTH"], json!(la::MAX_ARGS_LENGTH));
    assert_eq!(c["FORBIDDEN_PARAMS"], json!(la::FORBIDDEN_PARAMS));
    assert_eq!(c["ALWAYS_ARGUMENTS"], json!(la::ALWAYS_ARGUMENTS));
    assert_eq!(c["GOLDSRC_FLAGS"], json!(la::GOLDSRC_FLAGS));
    assert_eq!(c["LANGUAGE_AUTO"], json!(la::LANGUAGE_AUTO));
    assert_eq!(c["MAX_AFFINITY_CPUS"], json!(la::MAX_AFFINITY_CPUS));
    assert_eq!(c["PROTON_COMMAND_TOKEN"], json!(la::PROTON_COMMAND_TOKEN));
    // TOGGLE_ARGUMENTS — объект JS: порядок ключей — порядок аргументов в запуске.
    let keys: Vec<&str> = la::TOGGLE_ARGUMENTS.iter().map(|(key, _)| *key).collect();
    assert_eq!(json!(keys), c["toggleKeys"]);
    let toggles: Map<String, Value> = la::TOGGLE_ARGUMENTS
        .iter()
        .map(|(key, tokens)| ((*key).to_owned(), json!(tokens)))
        .collect();
    assert_eq!(Value::Object(toggles), c["TOGGLE_ARGUMENTS"]);
}

#[test]
fn default_settings_match_0_4_7() {
    // defaultSettings() записывает launchArgs и protonOptions первыми, санитайзер — последними.
    // `vulkan` 0.4.9 — в base_049.rs.
    assert_eq!(
        ser(&support::without_vulkan(&la::default_settings().to_value())),
        str_of(&FIXTURE["defaults"])
    );
    let settings = la::default_settings();
    assert_eq!(settings.launch_args, "");
    assert_eq!(settings.proton_options, "");
    assert!(settings.rinput);
    assert_eq!(settings.language, "auto");
}

#[test]
fn language_matches_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["language"]) {
        let input = if case["input"] == json!("undefined") {
            Value::Null
        } else {
            case["input"].clone()
        };
        m.check_eq(
            la::language_of(&input),
            str_of(&case["result"]).to_owned(),
            format!("languageOf({})", ser(&input)),
        );
    }
    m.finish("languageOf");
}

#[test]
fn affinity_mask_matches_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["affinity"]) {
        let (cpu, name) = match case["mark"].as_str() {
            Some("NaN") => (f64::NAN, "NaN".to_owned()),
            Some("Infinity") => (f64::INFINITY, "Infinity".to_owned()),
            Some("-Infinity") => (f64::NEG_INFINITY, "-Infinity".to_owned()),
            Some("-0") => (-0.0, "-0".to_owned()),
            Some(other) => panic!("неизвестная метка {other}"),
            None => (case["cpu"].as_f64().unwrap(), ser(&case["cpu"])),
        };
        m.check_eq(
            la::affinity_mask_without_cpu0(cpu),
            case["result"].as_str().map(str::to_owned),
            format!("affinityMaskWithoutCpu0({name})"),
        );
    }
    m.finish("affinityMaskWithoutCpu0");
}

#[test]
fn param_names_and_forbidden_params_match_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["params"]) {
        let token = str_of(&case["token"]);
        m.check_eq(
            la::param_name_of(token),
            case["name"].as_str().map(str::to_owned),
            format!("paramNameOf({token:?})"),
        );
        m.check_eq(
            la::is_forbidden(token),
            case["forbidden"].as_bool().unwrap(),
            format!("isForbidden({token:?})"),
        );
    }
    for case in items(&FIXTURE["protectedEnv"]) {
        let name = str_of(&case["name"]);
        m.check_eq(
            la::is_protected_env(name),
            case["result"].as_bool().unwrap(),
            format!("isProtectedEnv({name:?})"),
        );
    }
    m.finish("paramNameOf / isForbidden / isProtectedEnv");
}

fn check_args(case: &Value, m: &mut Mismatches) {
    let raw = &case["raw"];
    let context = |what: &str| format!("{what}({})", ser(raw));
    let check = |m: &mut Mismatches, what: &str, actual: String, expected: &Value| {
        m.check_eq(actual, str_of(expected).to_owned(), context(what));
    };
    check(
        m,
        "parseArgsString",
        ser(&args_value(&la::parse_args_string(raw))),
        &case["parse"],
    );
    check(
        m,
        "validateArgsString",
        ser(&args_value(&la::validate_args_string(raw))),
        &case["validate"],
    );
    check(
        m,
        "parseProtonOptions",
        ser(&proton_value(&la::parse_proton_options(raw))),
        &case["proton"],
    );
}

#[test]
fn argument_strings_match_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["args"]) {
        check_args(case, &mut m);
    }
    for case in items(&FIXTURE["argsNonString"]) {
        check_args(case, &mut m);
    }
    m.finish("разбор строк аргументов и параметров Proton");
}

#[test]
fn normalize_and_sanitize_settings_match_0_4_7() {
    let mut m = Mismatches::default();
    for (index, case) in items(&FIXTURE["settings"]).iter().enumerate() {
        let input = &case["input"];
        let normalized = match la::normalize_settings(input) {
            Ok(settings) => {
                json!({"ok": true, "settings": support::without_vulkan(&settings.to_value())})
            }
            Err(error) => json!({"ok": false, "error": error.to_value()}),
        };
        if let Err(message) = check_summary(&ser(&normalized), &case["normalize"]) {
            m.check_eq(
                message,
                String::new(),
                format!("normalizeSettings, случай {index}: {}", ser(input)),
            );
        }
        let sanitized = la::sanitize_settings(input);
        let sanitized = json!({
            "settings": support::without_vulkan(&sanitized.settings.to_value()),
            "dropped": sanitized.dropped.as_ref().map_or(Value::Null, ArgsError::to_value),
        });
        if let Err(message) = check_summary(&ser(&sanitized), &case["sanitize"]) {
            m.check_eq(
                message,
                String::new(),
                format!("sanitizeSettings, случай {index}: {}", ser(input)),
            );
        }
    }
    m.finish("normalizeSettings / sanitizeSettings");
}

#[test]
fn user_and_merged_arguments_match_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["build"]) {
        let settings = &case["settings"];
        // `String(a)` для элементов списка: в корпусе есть и число.
        let known: Vec<String> = case["known"]
            .as_array()
            .map(|list| {
                list.iter()
                    .map(|a| js::to_template_string(Some(a)))
                    .collect()
            })
            .unwrap_or_default();
        let context = format!("{} / {}", ser(settings), ser(&case["known"]));
        m.check_eq(
            la::build_user_arguments(settings, &known),
            strings(&case["user"]),
            format!("buildUserArguments({context})"),
        );
        m.check_eq(
            la::merge_launch_arguments(&known, settings),
            strings(&case["merged"]),
            format!("mergeLaunchArguments({context})"),
        );
    }
    m.finish("buildUserArguments / mergeLaunchArguments");
}
