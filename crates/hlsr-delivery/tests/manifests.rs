//! Манифесты, схемы, цепочки патчей против 0.4.7: фикстура `manifests.json` из
//! `tools/reference/delivery/manifests.js` (модули 0.4.7 и ajv 8 в Node 24 на тех же
//! входах). Документы мутаций в фикстуре заданы основой и операциями — тест применяет их
//! к тем же основам.

mod support;

use hlsr_core::js;
use hlsr_core::json_text::{js_number, parse_js_json, stringify};
use hlsr_delivery::archive_entries::ArchiveEntry;
use hlsr_delivery::errors::{Code, Failure};
use hlsr_delivery::file_ops::hex;
use hlsr_delivery::files_manifest::{self, ValidateOptions};
use hlsr_delivery::manifest_service::{InstallableOptions, ManifestService};
use hlsr_delivery::manifests;
use hlsr_delivery::patch_chain::{PlanOptions, plan_update};
use hlsr_delivery::patch_format::{
    self, DiffOptions, PatchValidateOptions, assert_safe_relative_path,
    assert_safe_relative_path_value, default_patch_name, diff, hash_patch_manifest,
    validate_patch_manifest, validate_payload_entries,
};
use hlsr_delivery::schema_validator::{
    CHANNEL_MANIFEST_SCHEMA, GAME_MANIFEST_SCHEMA, Validator, channel_manifest_validator,
    game_manifest_validator, is_date_time, pattern_matches,
};
use hlsr_delivery::update_target::{UpdateTarget, from_channel, from_local};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;
use support::{Mismatches, assert_json_eq, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("manifests.json"));
/// Что изменилось в 0.4.9 (цель обновления без `repairArchives`, вшитые манифесты, реестр), — из
/// `base-049.json` (`tools/reference/delivery/base-049.js`, на тех же случаях).
static BASE: LazyLock<Value> = LazyLock::new(|| fixture("base-049.json"));

fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn sha256_of_json(value: &Value) -> String {
    sha256_hex(stringify(value, "").as_bytes())
}

/// Значение так, как его видит JSON после записи: число в записи JS, порядок ключей прежний.
fn norm(value: &Value) -> Value {
    parse_js_json(&stringify(value, "")).expect("JSON")
}

// ------------------------------------------------------------ основы и операции

fn base(name: &str) -> Value {
    if let Some(game_id) = name.strip_prefix("builtin:") {
        return manifests::manifest_of(game_id)
            .unwrap_or_else(|| panic!("нет вшитого манифеста {game_id}"))
            .clone();
    }
    FIXTURE["bases"][name].clone()
}

/// `{"$repeat": "X", "n": 300}` — строка, `{"$fill": значение, "n": 70}` — массив.
fn decode(value: &Value) -> Value {
    match value {
        Value::Object(object) if object.contains_key("$repeat") => {
            let count = object["n"].as_u64().expect("n") as usize;
            Value::String(str_of(&object["$repeat"]).repeat(count))
        }
        Value::Object(object) if object.contains_key("$fill") => {
            let count = object["n"].as_u64().expect("n") as usize;
            Value::Array(vec![decode(&object["$fill"]); count])
        }
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), decode(value)))
                .collect(),
        ),
        Value::Array(list) => Value::Array(list.iter().map(decode).collect()),
        other => other.clone(),
    }
}

fn step<'a>(root: &'a mut Value, path: &[Value]) -> &'a mut Value {
    let mut current = root;
    for key in path {
        current = match key {
            Value::String(name) => current
                .as_object_mut()
                .and_then(|object| object.get_mut(name))
                .unwrap_or_else(|| panic!("нет поля {name}")),
            number => current
                .as_array_mut()
                .and_then(|list| list.get_mut(number.as_u64().expect("индекс") as usize))
                .unwrap_or_else(|| panic!("нет элемента {number}")),
        };
    }
    current
}

/// `applyOps` скрипта: `set`, `del`, `root`.
fn apply_ops(mut doc: Value, ops: &Value) -> Value {
    for op in items(ops) {
        let kind = str_of(&op[0]);
        if kind == "root" {
            doc = decode(&op[1]);
            continue;
        }
        let path = items(&op[1]);
        let (last, parent_path) = path.split_last().expect("путь");
        let parent = step(&mut doc, parent_path);
        match (kind, last) {
            ("set", Value::String(key)) => {
                parent
                    .as_object_mut()
                    .expect("объект")
                    .insert(key.clone(), decode(&op[2]));
            }
            ("set", index) => {
                let list = parent.as_array_mut().expect("массив");
                let index = index.as_u64().expect("индекс") as usize;
                if index == list.len() {
                    list.push(decode(&op[2]));
                } else {
                    list[index] = decode(&op[2]);
                }
            }
            ("del", Value::String(key)) => {
                if let Some(object) = parent.as_object_mut() {
                    object.shift_remove(key);
                }
            }
            ("del", index) => {
                parent
                    .as_array_mut()
                    .expect("массив")
                    .remove(index.as_u64().expect("индекс") as usize);
            }
            (other, _) => panic!("операция {other}"),
        }
    }
    doc
}

fn case_doc(case: &Value, base_key: &str, ops_key: &str) -> Value {
    apply_ops(base(str_of(&case[base_key])), &case[ops_key])
}

fn ops_text(case: &Value, ops_key: &str) -> String {
    stringify(&case[ops_key], "")
}

// ------------------------------------------------------------ итоги вызовов

fn delivery_outcome<T>(
    result: Result<T, hlsr_delivery::errors::DeliveryError>,
    ok: impl FnOnce(T) -> Value,
) -> Value {
    match result {
        Ok(value) => json!({"ok": ok(value)}),
        Err(error) => json!({"error": error.to_public()}),
    }
}

fn failure_outcome<T>(result: Result<T, Failure>, ok: impl FnOnce(T) -> Value) -> Value {
    match result {
        Ok(value) => json!({"ok": ok(value)}),
        Err(Failure::Delivery(error)) => json!({"error": error.to_public()}),
        Err(Failure::Raw(error)) => json!({"raw": {"name": "TypeError", "message": error.message}}),
    }
}

fn raw_outcome<T>(
    result: Result<T, hlsr_delivery::errors::RawError>,
    ok: impl FnOnce(T) -> Value,
) -> Value {
    match result {
        Ok(value) => json!({"ok": ok(value)}),
        Err(error) => json!({"raw": {"name": "TypeError", "message": error.message}}),
    }
}

fn result_of(index: &Value) -> &'static Value {
    &FIXTURE["resultTable"][index.as_u64().expect("индекс") as usize]
}

fn errors_of(case: &Value) -> Vec<String> {
    items(&case["e"])
        .iter()
        .map(|i| str_of(&FIXTURE["errorTable"][i.as_u64().expect("индекс") as usize]).to_owned())
        .collect()
}

fn error_lines(validator: &Validator, doc: &Value) -> Vec<String> {
    validator
        .validate(doc)
        .iter()
        .map(|e| format!("{}|{}|{}", e.instance_path, e.keyword, e.message))
        .collect()
}

// ------------------------------------------------------------ схемы

#[test]
fn schemas_are_the_ones_checked_against_ajv() {
    for (name, text) in [
        ("game", GAME_MANIFEST_SCHEMA),
        ("channel", CHANNEL_MANIFEST_SCHEMA),
    ] {
        let schema = parse_js_json(text).expect("схема");
        assert_eq!(
            sha256_of_json(&schema),
            str_of(&FIXTURE["schemas"][name]),
            "схема {name} отличается от той, по которой записана фикстура"
        );
    }
}

#[test]
fn schema_patterns_match_js_regexps() {
    let patterns = items(&FIXTURE["schemas"]["patterns"]);
    let strings = items(&FIXTURE["patterns"]["strings"]);
    let bits = items(&FIXTURE["patterns"]["bits"]);
    assert_eq!(patterns.len(), bits.len());
    let mut mismatches = Mismatches::default();
    for (pattern, bits) in patterns.iter().zip(bits) {
        let pattern = str_of(pattern);
        let bits = str_of(bits).as_bytes();
        assert_eq!(bits.len(), strings.len());
        for (text, bit) in strings.iter().zip(bits) {
            let text = str_of(text);
            mismatches.check_eq(
                pattern_matches(pattern, text),
                Some(*bit == b'1'),
                format!("/{pattern}/u.test({text:?})"),
            );
        }
    }
    mismatches.finish("шаблоны схем");
}

#[test]
fn date_time_matches_ajv_formats() {
    let strings = items(&FIXTURE["dateTime"]["strings"]);
    let bits = str_of(&FIXTURE["dateTime"]["bits"]).as_bytes();
    assert_eq!(bits.len(), strings.len());
    let mut mismatches = Mismatches::default();
    for (text, bit) in strings.iter().zip(bits) {
        let text = str_of(text);
        mismatches.check_eq(
            is_date_time(text),
            *bit == b'1',
            format!("date-time {text:?}"),
        );
    }
    mismatches.finish("формат date-time");
}

fn check_schema_cases(validator: &Validator, cases: &Value, what: &str) {
    let mut mismatches = Mismatches::default();
    let mut valid = 0;
    for case in items(cases) {
        let doc = case_doc(case, "b", "o");
        let actual = error_lines(validator, &doc);
        let expected = errors_of(case);
        if expected.is_empty() {
            valid += 1;
        }
        mismatches.check_eq(
            actual,
            expected,
            format!(
                "{what}: основа {}, операции {}",
                str_of(&case["b"]),
                ops_text(case, "o")
            ),
        );
    }
    assert!(
        valid > 20,
        "в корпусе {what} должны быть и принятые документы"
    );
    mismatches.finish(what);
}

#[test]
fn game_manifest_schema_errors_match_ajv() {
    check_schema_cases(
        game_manifest_validator(),
        &FIXTURE["gameValidation"],
        "схема игры",
    );
}

#[test]
fn channel_manifest_schema_errors_match_ajv() {
    check_schema_cases(
        channel_manifest_validator(),
        &FIXTURE["channelValidation"],
        "схема канала",
    );
}

#[test]
fn documents_read_by_json_parse_validate_like_ajv() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["rawValidation"]) {
        let which = str_of(&case["schema"]);
        let source = serde_json::to_string(
            &FIXTURE["bases"][if which == "game" {
                "gameRich"
            } else {
                "channelRich"
            }],
        )
        .expect("JSON");
        // `JSON.stringify` основы совпадает с компактной записью hlsr-core.
        let source = stringify(&parse_js_json(&source).expect("JSON"), "");
        let text = source.replacen(str_of(&case["find"]), str_of(&case["replace"]), 1);
        assert_ne!(text, source, "фрагмент не найден");
        let doc = parse_js_json(&text).expect("JSON");
        let validator = if which == "game" {
            game_manifest_validator()
        } else {
            channel_manifest_validator()
        };
        mismatches.check_eq(
            error_lines(validator, &doc),
            errors_of(&json!({"e": case["errors"]})),
            format!(
                "{which}: {} → {}",
                str_of(&case["find"]),
                str_of(&case["replace"])
            ),
        );
    }
    mismatches.finish("документы с числами в записи JS");
}

#[test]
fn each_schema_rejects_the_other_documents_like_ajv() {
    for case in items(&FIXTURE["crossed"]) {
        let validator = if str_of(&case["schema"]) == "game" {
            game_manifest_validator()
        } else {
            channel_manifest_validator()
        };
        let doc = base(str_of(&case["base"]));
        assert_eq!(
            error_lines(validator, &doc),
            errors_of(&json!({"e": case["errors"]})),
            "{} по схеме {}",
            str_of(&case["base"]),
            str_of(&case["schema"])
        );
    }
}

// ------------------------------------------------------------ вшитые манифесты

/// Вшитые манифесты — config/ 0.4.9 (Ghosting 1.0.13, файловые 1.0.10–1.0.13): сверка с
/// `base-049.json`.
#[test]
fn embedded_game_manifests_match_node() {
    let embedded = &BASE["embedded"];
    let service = ManifestService::new(None);
    let games = items(&embedded["games"]);
    assert_eq!(
        manifests::game_ids(),
        games
            .iter()
            .map(|g| str_of(&g["gameId"]))
            .collect::<Vec<_>>()
    );
    for game in games {
        let game_id = str_of(&game["gameId"]);
        let manifest = manifests::manifest_of(game_id).expect("манифест");
        assert_eq!(
            ManifestService::hash(manifest),
            str_of(&game["hash"]),
            "{game_id}: hash"
        );
        assert!(
            game_manifest_validator().is_valid(manifest),
            "{game_id}: схема"
        );
        let loaded = service.validate(manifest, Some("bundled"));
        assert_json_eq(
            &delivery_outcome(loaded, |l| json!({"sha256": l.sha256, "source": l.source})),
            &game["validate"],
            game_id,
        );
    }
}

#[test]
fn embedded_files_manifests_match_node() {
    let mut mismatches = Mismatches::default();
    let rows = items(&BASE["embedded"]["files"]);
    let mut expected_keys: Vec<(String, String)> = Vec::new();
    for row in rows {
        let game_id = str_of(&row["gameId"]);
        let version = str_of(&row["version"]);
        expected_keys.push((game_id.to_owned(), version.to_owned()));
        let data = manifests::files_manifest_of(game_id, version).expect("файловый манифест");
        let context = format!("{game_id} {version}");
        mismatches.check_eq(
            sha256_of_json(data),
            str_of(&row["stringifySha256"]).to_owned(),
            format!("{context}: JSON.stringify"),
        );
        mismatches.check_eq(
            files_manifest::canonical_payload(data)
                .map(|p| sha256_hex(p.as_bytes()))
                .unwrap(),
            str_of(&row["canonicalPayloadSha256"]).to_owned(),
            format!("{context}: canonicalPayload"),
        );
        mismatches.check_eq(
            files_manifest::hash_manifest(data).unwrap(),
            str_of(&row["hashManifest"]).to_owned(),
            format!("{context}: hashManifest"),
        );
        mismatches.check_eq(
            data.get("manifestSha256")
                .and_then(Value::as_str)
                .map(str::to_owned),
            row["manifestSha256"].as_str().map(str::to_owned),
            format!("{context}: manifestSha256 в файле"),
        );
        mismatches.check_eq(
            data["files"].as_array().map(Vec::len),
            Some(row["entries"].as_u64().unwrap() as usize),
            format!("{context}: записей"),
        );
        let options = ValidateOptions {
            game_id: Some(game_id.to_owned()),
            version: Some(version.to_owned()),
            ..ValidateOptions::default()
        };
        let source = format!(
            "bundled:{}",
            str_of(&row["file"]).trim_start_matches("config/")
        );
        mismatches.check(
            &delivery_outcome(
                files_manifest::validate(data, Some(&source), Some(&options)),
                |_| json!(true),
            ),
            &row["validate"],
            format!("{context}: validate"),
        );
    }
    mismatches.finish("вшитые файловые манифесты");

    // В реестре ровно эти версии и в том же порядке.
    for game_id in manifests::game_ids() {
        let versions: Vec<&str> = manifests::files_manifests_of(game_id)
            .iter()
            .map(|(v, _)| *v)
            .collect();
        let expected: Vec<&str> = expected_keys
            .iter()
            .filter(|(g, _)| g == game_id)
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(versions, expected, "{game_id}");
    }
}

#[test]
fn embedded_tool_and_save_manifests_match_node() {
    let extras = items(&FIXTURE["embedded"]["extras"]);
    let tool = extras
        .iter()
        .find(|e| str_of(&e["id"]) == "livesplit")
        .unwrap();
    let saves = extras
        .iter()
        .find(|e| str_of(&e["id"]) == "hl1-practice-saves")
        .unwrap();
    assert_eq!(
        sha256_of_json(manifests::livesplit_tool_manifest().value()),
        str_of(&tool["stringifySha256"])
    );
    assert_eq!(
        sha256_of_json(manifests::hl1_practice_saves_manifest().value()),
        str_of(&saves["stringifySha256"])
    );
}

// ------------------------------------------------------------ ManifestService

#[test]
fn manifest_service_validate_matches_node() {
    let mut mismatches = Mismatches::default();
    // Вшитый манифест Ghosting — 0.4.9: без операций его хеш — из base-049.json.
    let ghosting_049 = items(&BASE["embedded"]["games"])
        .iter()
        .find(|game| game["gameId"] == "ghosting-renewed")
        .map(|game| str_of(&game["hash"]))
        .unwrap();
    for case in items(&FIXTURE["service"]) {
        let doc = case_doc(case, "b", "o");
        let source = case["src"].as_str();
        let actual = delivery_outcome(
            ManifestService::new(None).validate(&doc, source),
            |l| json!({"sha256": l.sha256, "source": l.source}),
        );
        let mut expected = result_of(&case["r"]).clone();
        if case["b"] == "builtin:ghosting-renewed" && items(&case["o"]).is_empty() {
            expected["ok"]["sha256"] = json!(ghosting_049);
        }
        mismatches.check(
            &actual,
            &expected,
            format!(
                "основа {}, операции {}, source {:?}",
                str_of(&case["b"]),
                ops_text(case, "o"),
                source
            ),
        );
    }
    mismatches.finish("ManifestService.validate");
}

#[test]
fn manifest_service_parse_matches_node() {
    const V8_PREFIX: &str = "Манифест не является JSON: ";
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["parse"]) {
        let text = str_of(&case["text"]);
        let mut actual = delivery_outcome(
            ManifestService::new(None).parse(text, Some("s")),
            |l| json!({"sha256": l.sha256, "source": l.source}),
        );
        // Текст ошибки разбора — V8 в Node, serde_json здесь: сверяются код и начало сообщения.
        if let Some(message) = actual["error"]["message"].as_str()
            && message.starts_with(V8_PREFIX)
        {
            actual["error"]["message"] = json!(format!("{V8_PREFIX}<V8>"));
        }
        mismatches.check(&actual, &case["result"], format!("parse({text:?})"));
    }
    mismatches.finish("ManifestService.parse");
}

#[test]
fn manifest_service_hash_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["hash"]) {
        let text = str_of(&case["text"]);
        let value = parse_js_json(text).expect("JSON");
        mismatches.check_eq(
            ManifestService::hash(&value),
            str_of(&case["hash"]).to_owned(),
            format!("hash({text})"),
        );
    }
    mismatches.finish("ManifestService.hash");
}

#[test]
fn manifest_service_hash_depends_on_key_order() {
    let a = ManifestService::hash(&parse_js_json(r#"{"b":1,"a":2}"#).unwrap());
    let b = ManifestService::hash(&parse_js_json(r#"{"a":2,"b":1}"#).unwrap());
    assert_ne!(a, b);
}

#[test]
fn manifest_service_load_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let service = ManifestService::new(None);
    let missing = dir.path().join("нет.json");
    let error = service.load_from_file(&missing).unwrap_err();
    assert_eq!(error.code, Code::ManifestNotFound);
    assert_eq!(
        error.message,
        format!("Манифест не найден: {}", missing.display())
    );

    let manifest = manifests::manifest_of("hl1-won").unwrap();
    let file = dir.path().join("hl1-won.json");
    std::fs::write(&file, stringify(manifest, "  ")).unwrap();
    let loaded = service.load_from_file(&file).unwrap();
    assert_eq!(loaded.sha256, ManifestService::hash(manifest));
    assert_eq!(loaded.source.as_deref(), file.to_str());

    std::fs::write(&file, "\u{FEFF}{}").unwrap();
    assert_eq!(
        service.load_from_file(&file).unwrap_err().code,
        Code::ManifestInvalid,
        "BOM — не JSON"
    );
}

fn installable_options(opt: &Value) -> Option<InstallableOptions> {
    if opt.as_str() == Some("undefined") {
        return None;
    }
    Some(InstallableOptions {
        launcher_version: opt["launcherVersion"].as_str().map(str::to_owned),
        allowed_provider_types: opt["allowedProviderTypes"]
            .as_array()
            .map(|types| types.iter().map(|t| str_of(t).to_owned()).collect())
            .unwrap_or_default(),
    })
}

#[test]
fn manifest_service_installable_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["installable"]) {
        let manifest = case_doc(case, "b", "o");
        let options = installable_options(&case["opt"]);
        let service = ManifestService::new(case["sv"].as_str());
        let context = format!(
            "основа {}, операции {}, версия {:?}, настройки {}",
            str_of(&case["b"]),
            ops_text(case, "o"),
            case["sv"],
            case["opt"]
        );

        let check = raw_outcome(
            service.check_installable(&manifest, options.as_ref()),
            |check| {
                let ids: Vec<Value> = check.providers.iter().map(|p| p["id"].clone()).collect();
                json!({
                    "ok": check.ok,
                    "reasons": check.reasons.iter().map(|r| r.to_json()).collect::<Vec<_>>(),
                    "providers": ids,
                })
            },
        );
        mismatches.check(
            &check,
            result_of(&case["c"]),
            format!("{context}: checkInstallable"),
        );

        // assertInstallable: ошибка — как в фикстуре; успех — `a` пусто.
        let asserted = service.assert_installable(&manifest, options.as_ref());
        match (&case["a"], asserted) {
            (Value::Null, Ok(_)) => {}
            (Value::Null, Err(error)) => {
                mismatches.check_eq(
                    format!("{error:?}"),
                    "успех".to_owned(),
                    format!("{context}: assertInstallable"),
                );
            }
            (expected, result) => {
                let actual = failure_outcome(result, |_| json!(null));
                mismatches.check(
                    &actual,
                    result_of(expected),
                    format!("{context}: assertInstallable"),
                );
            }
        }
    }
    mismatches.finish("ManifestService.checkInstallable");
}

#[test]
fn manifest_service_order_providers_matches_node() {
    for case in items(&FIXTURE["orderProviders"]) {
        let ordered: Vec<String> = ManifestService::order_providers(items(&case["list"]))
            .iter()
            .map(|p| str_of(&p["id"]).to_owned())
            .collect();
        let expected: Vec<String> = items(&case["ordered"])
            .iter()
            .map(|i| str_of(i).to_owned())
            .collect();
        assert_eq!(ordered, expected, "{}", case["list"]);
    }
}

// ------------------------------------------------------------ files-manifest

#[test]
fn files_manifest_validate_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["filesValidation"]) {
        let doc = apply_ops(FIXTURE["filesBase"].clone(), &case["o"]);
        let options = (case["opt"].as_str() != Some("undefined")).then(|| ValidateOptions {
            game_id: case["opt"]["gameId"].as_str().map(str::to_owned),
            version: case["opt"]["version"].as_str().map(str::to_owned),
            // `a > "2"` в JS сравнивает числа: строка приводится как `Number("2")`.
            max_entries: case["opt"]["maxEntries"]
                .as_f64()
                .or_else(|| case["opt"]["maxEntries"].as_str().map(js::to_number)),
            require_manifest_sha256: case["opt"]["requireManifestSha256"]
                .as_bool()
                .unwrap_or(false),
        });
        let actual = delivery_outcome(
            files_manifest::validate(&doc, Some("src"), options.as_ref()),
            |_| json!(true),
        );
        mismatches.check(
            &actual,
            result_of(&case["r"]),
            format!(
                "операции {}, настройки {}",
                ops_text(case, "o"),
                case["opt"]
            ),
        );
    }
    mismatches.finish("files-manifest.validate");
}

#[test]
fn files_manifest_canonical_payload_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["canonicalPayload"]) {
        let text = str_of(&case["json"]);
        let doc = parse_js_json(text).expect("JSON");
        mismatches.check(
            &raw_outcome(files_manifest::canonical_payload(&doc), Value::String),
            &case["payload"],
            format!("canonicalPayload({text})"),
        );
        mismatches.check(
            &raw_outcome(files_manifest::hash_manifest(&doc), Value::String),
            &case["hash"],
            format!("hashManifest({text})"),
        );
    }
    mismatches.finish("files-manifest.canonicalPayload");
}

#[test]
fn files_manifest_serialize_matches_node() {
    for case in items(&FIXTURE["serialize"]) {
        let doc = parse_js_json(str_of(&case["json"])).unwrap();
        assert_eq!(files_manifest::serialize(&doc), str_of(&case["serialized"]));
    }
}

fn names(value: &Value) -> Vec<String> {
    items(value).iter().map(|v| str_of(v).to_owned()).collect()
}

#[test]
fn files_manifest_load_reads_and_validates() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.files.json");
    assert_eq!(
        files_manifest::load(&file).unwrap_err().code,
        Code::ManifestNotFound
    );
    std::fs::write(&file, "{").unwrap();
    assert_eq!(
        files_manifest::load(&file).unwrap_err().code,
        Code::ManifestInvalid
    );
    std::fs::write(&file, files_manifest::serialize(&FIXTURE["filesBase"])).unwrap();
    assert_json_eq(
        &files_manifest::load(&file).unwrap(),
        &FIXTURE["filesBase"],
        "load",
    );
}

// ------------------------------------------------------------ patch-format

#[test]
fn patch_manifest_validate_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["patchValidation"]) {
        let doc = apply_ops(FIXTURE["patchBase"].clone(), &case["o"]);
        let options = (case["opt"].as_str() != Some("undefined")).then(|| PatchValidateOptions {
            expected_from: case["opt"]["expectedFrom"].as_str().map(str::to_owned),
        });
        let actual = failure_outcome(validate_patch_manifest(&doc, options.as_ref()), |_| {
            json!(true)
        });
        mismatches.check(
            &actual,
            result_of(&case["r"]),
            format!(
                "операции {}, настройки {}",
                ops_text(case, "o"),
                case["opt"]
            ),
        );
    }
    mismatches.finish("validatePatchManifest");
}

#[test]
fn patch_manifest_hash_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["hashPatch"]) {
        let text = str_of(&case["json"]);
        let doc = parse_js_json(text).expect("JSON");
        mismatches.check(
            &raw_outcome(patch_format::canonical_payload(&doc), Value::String),
            &case["canonical"],
            format!("patch canonicalPayload({text})"),
        );
        mismatches.check(
            &raw_outcome(hash_patch_manifest(&doc), Value::String),
            &case["hash"],
            format!("hashPatchManifest({text})"),
        );
    }
    mismatches.finish("hashPatchManifest");
    assert_eq!(
        hash_patch_manifest(&FIXTURE["patchBase"]).unwrap(),
        str_of(&FIXTURE["patchBase"]["patchManifestSha256"])
    );
}

#[test]
fn assert_safe_relative_path_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["safePath"]) {
        let what = if case["whatUndefined"].as_bool() == Some(true) {
            None
        } else {
            case["what"].as_str()
        };
        let (context, actual) = if let Some(json) = case["nonString"].as_str() {
            let value = parse_js_json(json).unwrap();
            (
                json.to_string(),
                failure_outcome(assert_safe_relative_path_value(&value, what), Value::String),
            )
        } else if case["value"].is_null() {
            (
                "null".to_owned(),
                failure_outcome(
                    assert_safe_relative_path_value(&Value::Null, what),
                    Value::String,
                ),
            )
        } else {
            let value = str_of(&case["value"]);
            (
                value.to_owned(),
                delivery_outcome(assert_safe_relative_path(value, what), Value::String),
            )
        };
        mismatches.check(
            &actual,
            &case["result"],
            format!("assertSafeRelativePath({context}, {what:?})"),
        );
    }
    mismatches.finish("assertSafeRelativePath");
}

fn entry_from(value: &Value) -> ArchiveEntry {
    ArchiveEntry {
        path: str_of(&value["path"]).to_owned(),
        size: value["size"].as_f64().unwrap(),
        packed_size: value["packedSize"].as_f64().unwrap(),
        attributes: str_of(&value["attributes"]).to_owned(),
        modified: value["modified"].as_str().map(str::to_owned),
        crc: value["crc"].as_str().map(str::to_owned),
        encrypted: value["encrypted"].as_bool().unwrap(),
        is_directory: value["isDirectory"].as_bool().unwrap(),
        is_symlink: value["isSymlink"].as_bool().unwrap(),
    }
}

#[test]
fn validate_payload_entries_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["payloadEntries"]) {
        let entries: Vec<ArchiveEntry> = items(&case["entries"]).iter().map(entry_from).collect();
        let actual = failure_outcome(
            validate_payload_entries(&case["manifest"], &entries),
            |ok| json!(ok),
        );
        mismatches.check(&actual, &case["result"], str_of(&case["name"]));
    }
    mismatches.finish("validatePayloadEntries");
}

fn embedded_files(reference: &Value) -> &'static Value {
    manifests::files_manifest_of(str_of(&reference[0]), str_of(&reference[1]))
        .expect("вшитый файловый манифест")
}

fn diff_digest(result: &patch_format::DiffResult) -> Value {
    let paths = |list: &[Value], n: usize| -> Vec<Value> {
        list.iter().take(n).map(|f| f["path"].clone()).collect()
    };
    json!({
        "added": result.added.len(),
        "changed": result.changed.len(),
        "unchanged": result.unchanged.len(),
        "deleted": result.deleted_files.len(),
        "payloadSize": js_number(result.payload_size),
        "sha256": sha256_of_json(&result.to_json()),
        "firstAdded": paths(&result.added, 2),
        "firstChanged": paths(&result.changed, 2),
        "firstDeleted": result.deleted_files.iter().take(2).collect::<Vec<_>>(),
    })
}

#[test]
fn diff_of_embedded_files_manifests_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["diff"]) {
        let old = embedded_files(&case["old"]);
        let new = embedded_files(&case["new"]);
        let preserve: Vec<String> = names(&case["preservePaths"]);
        let prefixes = names(&case["stillShippedPrefixes"]);
        let shipped = |path: &str| {
            prefixes
                .iter()
                .any(|prefix| path.starts_with(prefix.as_str()))
        };
        let options = DiffOptions {
            preserve_paths: &preserve,
            still_shipped: (!prefixes.is_empty()).then_some(&shipped as &dyn Fn(&str) -> bool),
        };
        let actual = delivery_outcome(diff(old, new, &options), |d| diff_digest(&d));
        mismatches.check(
            &actual,
            &case["result"],
            format!("{} → {}", case["old"], case["new"]),
        );
    }
    mismatches.finish("diff двух files.json");
}

#[test]
fn diff_of_synthetic_manifests_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["diffSynthetic"]) {
        let preserve: Vec<String> = case["preservePaths"]
            .as_array()
            .map(|_| names(&case["preservePaths"]))
            .unwrap_or_default();
        let shipped_list = case["shipped"].as_array().map(|_| names(&case["shipped"]));
        let shipped = |path: &str| {
            shipped_list
                .as_ref()
                .is_some_and(|list| list.iter().any(|p| p == path))
        };
        let options = DiffOptions {
            preserve_paths: &preserve,
            still_shipped: shipped_list
                .is_some()
                .then_some(&shipped as &dyn Fn(&str) -> bool),
        };
        let actual = delivery_outcome(diff(&case["old"], &case["new"], &options), |d| {
            norm(&d.to_json())
        });
        // `payloadSize` в Node пишется числом; остальное — записи манифестов как есть.
        mismatches.check(&actual, &case["result"], str_of(&case["name"]));
    }
    mismatches.finish("diff синтетических манифестов");
}

#[test]
fn patch_manifests_built_from_embedded_versions_match_node() {
    let ghosting = manifests::manifest_of("ghosting-renewed").unwrap();
    let preserve: Vec<String> = ghosting["preservePaths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| str_of(p).to_owned())
        .collect();
    for row in items(&FIXTURE["realPatches"]) {
        let (from, to) = (str_of(&row["from"]), str_of(&row["to"]));
        let old = manifests::files_manifest_of("ghosting-renewed", from).unwrap();
        let new = manifests::files_manifest_of("ghosting-renewed", to).unwrap();
        let result = diff(
            old,
            new,
            &DiffOptions {
                preserve_paths: &preserve,
                still_shipped: None,
            },
        )
        .unwrap();
        let mut doc = json!({
            "schemaVersion": 1,
            "gameId": "ghosting-renewed",
            "fromVersion": from,
            "toVersion": to,
            "payloadRoot": patch_format::PAYLOAD_ROOT,
            "files": result.payload.iter().map(|f| json!({"path": f["path"], "size": f["size"], "sha256": f["sha256"]})).collect::<Vec<_>>(),
            "deletedFiles": result.deleted_files,
            "archive": {
                "filename": format!("{}.7z", default_patch_name("ghosting-renewed", from, to)),
                "size": 123456,
                "sha256": "9".repeat(64),
            },
            "resultingFilesManifestSha256": new["manifestSha256"],
            "resultingTotalFiles": new["totalFiles"],
            "resultingTotalSize": new["totalSize"],
        });
        let hash = hash_patch_manifest(&doc).unwrap();
        assert_eq!(
            hash,
            str_of(&row["hash"]),
            "{from} → {to}: hashPatchManifest"
        );
        assert_eq!(
            sha256_hex(patch_format::canonical_payload(&doc).unwrap().as_bytes()),
            str_of(&row["canonicalSha256"]),
            "{from} → {to}: canonicalPayload"
        );
        doc["patchManifestSha256"] = json!(hash);
        let options = PatchValidateOptions {
            expected_from: Some(from.to_owned()),
        };
        let actual = failure_outcome(validate_patch_manifest(&doc, Some(&options)), |_| {
            json!(true)
        });
        assert_json_eq(
            &actual,
            &row["validate"],
            &format!("{from} → {to}: validatePatchManifest"),
        );
        assert_eq!(
            doc["files"].as_array().unwrap().len() as u64,
            row["files"].as_u64().unwrap()
        );
        assert_eq!(
            doc["deletedFiles"].as_array().unwrap().len() as u64,
            row["deleted"].as_u64().unwrap()
        );
    }
}

// ------------------------------------------------------------ patch-chain

fn chain_patches(encoded: &Value) -> Vec<Value> {
    items(encoded)
        .iter()
        .enumerate()
        .map(|(index, patch)| {
            let mut object = Map::new();
            object.insert("id".to_owned(), json!(format!("p{index}")));
            object.insert("fromVersion".to_owned(), patch[0].clone());
            object.insert("toVersion".to_owned(), patch[1].clone());
            match &patch[2] {
                Value::String(none) if none == "none" => {}
                Value::Null => {
                    object.insert("archive".to_owned(), Value::Null);
                }
                size => {
                    object.insert("archive".to_owned(), json!({"size": size}));
                }
            }
            Value::Object(object)
        })
        .collect()
}

#[test]
fn plan_update_matches_node_on_random_graphs() {
    let mut mismatches = Mismatches::default();
    let cases = items(&FIXTURE["chain"]["random"]);
    let mut kinds = std::collections::BTreeMap::<String, usize>::new();
    for case in cases {
        let patches = chain_patches(&case["p"]);
        let full_size = match &case["s"] {
            Value::String(_) => None,
            Value::Null => Some(f64::NAN),
            number => number.as_f64(),
        };
        let plan = plan_update(&PlanOptions {
            patches: &patches,
            from_version: case["f"].as_str(),
            to_version: case["t"].as_str(),
            full_size,
        });
        // Шаги в фикстуре — номера патчей.
        let mut actual = plan.to_json();
        let steps: Vec<Value> = plan
            .steps
            .iter()
            .map(|s| json!(str_of(&s["id"])[1..].parse::<u64>().unwrap()))
            .collect();
        actual["steps"] = json!(steps);
        let key = format!(
            "{} {}",
            str_of(&actual["kind"]),
            actual["reason"].as_str().unwrap_or("-")
        );
        *kinds.entry(key).or_default() += 1;
        mismatches.check(
            &actual,
            &case["r"],
            format!(
                "{} → {}, полная {}, патчи {}",
                case["f"], case["t"], case["s"], case["p"]
            ),
        );
    }
    mismatches.finish("planUpdate");
    for kind in [
        "full downgrade",
        "full no-chain",
        "full not-installed",
        "full patches-larger",
        "patches -",
        "up-to-date -",
    ] {
        assert!(
            kinds.contains_key(kind),
            "в графах нет случая «{kind}»: {kinds:?}"
        );
    }
}

#[test]
fn plan_update_without_options_matches_node() {
    for case in items(&FIXTURE["chain"]["trivial"]) {
        let options = &case["options"];
        let plan = if options.as_str() == Some("undefined") {
            plan_update(&PlanOptions::default())
        } else {
            plan_update(&PlanOptions {
                patches: &[],
                from_version: options["fromVersion"].as_str(),
                to_version: options["toVersion"].as_str(),
                full_size: options["fullSize"].as_f64(),
            })
        };
        assert_json_eq(
            &plan.to_json(),
            &case["plan"],
            &format!("planUpdate({options})"),
        );
    }
}

// ------------------------------------------------------------ update-target

fn target_to_json(target: Option<UpdateTarget>) -> Value {
    json!({"ok": target.map_or(Value::Null, |t| t.to_json())})
}

fn digest_matches(actual: &Value, expected: &Value, context: &str, mismatches: &mut Mismatches) {
    if let Some(json) = expected.get("j") {
        mismatches.check(&norm(actual), json, context);
    } else {
        mismatches.check_eq(
            sha256_of_json(actual),
            str_of(&expected["d"]).to_owned(),
            context,
        );
    }
}

#[test]
fn update_target_matches_node() {
    let mut mismatches = Mismatches::default();
    let expected = items(&BASE["updateTarget"]);
    for (index, case) in items(&FIXTURE["targets"]["cases"]).iter().enumerate() {
        let doc = if case["b"].is_null() {
            case["v"].clone()
        } else {
            case_doc(case, "b", "o")
        };
        let context = if case["b"].is_null() {
            doc.to_string()
        } else {
            format!("{} {}", str_of(&case["b"]), ops_text(case, "o"))
        };
        digest_matches(
            &target_to_json(from_local(&doc)),
            &expected[index]["l"],
            &format!("fromLocal {context}"),
            &mut mismatches,
        );
        digest_matches(
            &target_to_json(from_channel(&doc)),
            &expected[index]["c"],
            &format!("fromChannel {context}"),
            &mut mismatches,
        );
    }
    mismatches.finish("update-target");
}

// ------------------------------------------------------------ реестр

#[test]
fn registry_matches_node() {
    let registry = &FIXTURE["registry"];
    assert_eq!(manifests::game_ids(), names(&registry["manifestKeys"]));
    for (game_id, versions) in BASE["manifests"]["filesKeys"].as_object().unwrap() {
        let actual: Vec<&str> = manifests::files_manifests_of(game_id)
            .iter()
            .map(|(v, _)| *v)
            .collect();
        assert_eq!(actual, names(versions), "{game_id}");
    }
    let by_launcher: Vec<(String, String)> = manifests::BY_LAUNCHER_ID
        .iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect();
    let expected: Vec<(String, String)> = registry["byLauncherIdKeys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| {
            (
                str_of(key).to_owned(),
                str_of(&registry["byLauncherId"][str_of(key)]).to_owned(),
            )
        })
        .collect();
    assert_eq!(by_launcher, expected);

    let builds: Value = json!({"220": manifests::BUILDS_BY_LAUNCHER_ID[0].1.iter().map(|b| {
        let mut object = Map::new();
        object.insert("build".to_owned(), json!(b.build));
        object.insert("gameId".to_owned(), json!(b.game_id));
        if let Some(delivery) = b.delivery { object.insert("delivery".to_owned(), json!(delivery)); }
        Value::Object(object)
    }).collect::<Vec<_>>()});
    assert_json_eq(
        &builds,
        &registry["buildsByLauncherId"],
        "BUILDS_BY_LAUNCHER_ID",
    );
    assert_eq!(manifests::depot_builds(), names(&registry["depotBuilds"]));

    // Объекты настроек — те же ключи, порядок и значения (сверка текстом).
    let local: Map<String, Value> = manifests::LOCAL_CONFIG
        .iter()
        .map(|(id, c)| ((*id).to_owned(), c.to_json()))
        .collect();
    assert_eq!(
        stringify(&Value::Object(local), ""),
        str_of(&BASE["manifests"]["localConfigText"])
    );
    let launch: Map<String, Value> = manifests::LAUNCH_CONFIG
        .iter()
        .map(|(id, c)| ((*id).to_owned(), c.to_json()))
        .collect();
    assert_eq!(
        stringify(&Value::Object(launch), ""),
        str_of(&BASE["manifests"]["launchConfigText"])
    );

    let titles: Map<String, Value> = manifests::OWNED_APP_TITLES
        .iter()
        .map(|(id, t)| (id.to_string(), json!(t)))
        .collect();
    assert_json_eq(
        &Value::Object(titles),
        &registry["ownedAppTitles"],
        "OWNED_APP_TITLES",
    );
    let owned: Vec<Value> = manifests::required_owned_apps()
        .iter()
        .map(|a| json!({"appId": a.app_id, "title": a.title}))
        .collect();
    assert_json_eq(
        &json!(owned),
        &registry["requiredOwnedApps"],
        "requiredOwnedApps",
    );
}

#[test]
fn file_repair_games_like_0_4_9() {
    for case in items(&BASE["manifests"]["isFileRepairGame"]) {
        let id = str_of(&case["id"]);
        assert_eq!(
            json!(manifests::is_file_repair_game(id)),
            case["result"],
            "{id}"
        );
    }
}

#[test]
fn registry_functions_match_node() {
    // 0.4.9: те же входы, что у manifests.json, итоги — из base-049.json.
    let calls = &BASE["manifests"]["calls"];
    let mut mismatches = Mismatches::default();
    for case in items(&calls["forLauncherId"]) {
        let value = str_of(&case["value"]);
        let actual = manifests::for_launcher_id(value).map(|m| str_of(&m["gameId"]).to_owned());
        mismatches.check_eq(
            actual,
            case["gameId"].as_str().map(str::to_owned),
            format!("forLauncherId({value:?})"),
        );
    }
    for case in items(&calls["launcherIdOf"]) {
        let value = str_of(&case["value"]);
        let expected = case["result"]["ok"].as_str().map(str::to_owned);
        mismatches.check_eq(
            manifests::launcher_id_of(value).map(str::to_owned),
            expected,
            format!("launcherIdOf({value:?})"),
        );
    }
    for case in items(&calls["isDepotBuild"]) {
        let value = str_of(&case["value"]);
        mismatches.check_eq(
            manifests::is_depot_build(value),
            case["result"]["ok"].as_bool().unwrap(),
            format!("isDepotBuild({value:?})"),
        );
    }
    for case in items(&calls["isKnownGame"]) {
        let value = str_of(&case["value"]);
        mismatches.check_eq(
            manifests::is_known_game(value),
            case["result"]["ok"].as_bool().unwrap(),
            format!("isKnownGame({value:?})"),
        );
    }
    for case in items(&calls["localConfigOf"]) {
        let value = str_of(&case["value"]);
        let actual = manifests::local_config_of(value).map_or(Value::Null, |c| c.to_json());
        mismatches.check(
            &actual,
            &case["result"]["ok"],
            format!("localConfigOf({value:?})"),
        );
    }
    for case in items(&calls["launchConfigOf"]) {
        let value = str_of(&case["value"]);
        let actual = manifests::launch_config_of(value).map_or(Value::Null, |c| c.to_json());
        mismatches.check(
            &actual,
            &case["result"]["ok"],
            format!("launchConfigOf({value:?})"),
        );
    }
    for case in items(&calls["buildsOf"]) {
        let value = str_of(&case["value"]);
        mismatches.check_eq(
            manifests::builds_of(value)
                .iter()
                .map(|s| (*s).to_owned())
                .collect::<Vec<_>>(),
            names(&case["result"]["ok"]),
            format!("buildsOf({value:?})"),
        );
    }
    for case in items(&calls["buildGameIdOf"]) {
        let (launcher, build) = (str_of(&case["launcher"]), str_of(&case["build"]));
        mismatches.check_eq(
            manifests::build_game_id_of(launcher, build).map(str::to_owned),
            case["result"].as_str().map(str::to_owned),
            format!("buildGameIdOf({launcher:?}, {build:?})"),
        );
    }
    for case in items(&calls["filesManifestOf"]) {
        let (game_id, version) = (str_of(&case["gameId"]), str_of(&case["version"]));
        let actual = manifests::files_manifest_of(game_id, version).map(sha256_of_json);
        mismatches.check_eq(
            actual,
            case["sha256"].as_str().map(str::to_owned),
            format!("filesManifestOf({game_id:?}, {version:?})"),
        );
    }
    for case in items(&calls["filesManifestVersions"]) {
        let game_id = str_of(&case["gameId"]);
        mismatches.check_eq(
            manifests::files_manifest_versions(game_id)
                .iter()
                .map(|s| (*s).to_owned())
                .collect::<Vec<_>>(),
            names(&case["versions"]),
            format!("filesManifestVersions({game_id:?})"),
        );
    }
    mismatches.finish("реестр манифестов");
}
