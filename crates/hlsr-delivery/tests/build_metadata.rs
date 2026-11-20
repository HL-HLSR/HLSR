//! Метаданные сборки против 0.4.7: фикстура `build-metadata.json` из
//! `tools/reference/delivery/build-metadata.js`.
//!
//! `build-metadata.js` (`parse`, `normalize_built_at`: корпус мутаций образца `hlsr-build.json`),
//! `build-metadata-reader.js` (`read` на деревьях файлов: размер, ссылки, выход за установку, кодировка)
//! и `build-metadata-trust.js` (`validate_descriptor`, согласие с манифестом, совместимость с лаунчером,
//! `verify_for_install`, `verify_against_snapshot`, снимок для `installed.json`): причины недоверия по
//! корпусу. Сценарий — файлы и шаги в данных; их выполняют модули 0.4.7 в Node 24 и эти модули на
//! тех же деревьях. Текст ошибки `JSON.parse` у V8 и serde_json разный — после «не являются JSON: »
//! в фикстуре метка. Сценарии, помеченные `unix`, зависят от ОС (ссылки, права доступа): на Windows
//! они пропускаются.

mod support;

use hlsr_core::js;
use hlsr_core::json_text::{json_parse, stringify};
use hlsr_delivery::build_metadata::{
    self, BuildMetadata, BuildMetadataError, Profile, ValidateOptions, normalize_built_at, parse,
};
use hlsr_delivery::build_metadata_reader::{self as reader, ReadError, ReadOptions, Reading};
use hlsr_delivery::build_metadata_trust::{
    self as trust, BuildMetadataTrustError, Verified, VerifyAgainstSnapshot, VerifyForInstall,
    assert_consistent_with_manifest, assert_launcher_compatible, snapshot_from_verified,
    validate_descriptor, verify_against_snapshot, verify_for_install,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::LazyLock;
use support::scenario::{self, common_step, normalize_text, path_in, temp_root};
use support::{Mismatches, fixture, items, opt_str, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("build-metadata.json"));

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Как сохраняет фикстуру `JSON.stringify` и читает её `JSON.parse`: числа в записи JS, `Infinity`
/// становится `null`.
fn js_plain(value: &Value) -> Value {
    json_parse(&stringify(value, "")).unwrap()
}

fn map_strings(value: &Value, convert: &dyn Fn(&str) -> String) -> Value {
    match value {
        Value::String(text) => Value::String(convert(text)),
        Value::Array(list) => Value::Array(list.iter().map(|v| map_strings(v, convert)).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, item)| (key.clone(), map_strings(item, convert)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Значение, прошедшее через JSON, с путями временной папки в строках.
fn plain(root: &std::path::Path, value: &Value) -> Value {
    map_strings(&js_plain(value), &|text| normalize_text(root, text))
}

const JSON_MARKER: &str = "не являются JSON: ";

/// Текст разбора JSON у V8 и у serde_json разный; совпадать обязан код ошибки.
fn mask_json(text: &str) -> String {
    match text.find(JSON_MARKER) {
        Some(at) => format!("{}<json>", &text[..at + JSON_MARKER.len()]),
        None => text.to_owned(),
    }
}

fn error_value(
    root: &std::path::Path,
    kind: &str,
    code: &str,
    message: &str,
    detail: Option<&Value>,
) -> Value {
    let message = mask_json(&normalize_text(root, message));
    let mut out = json!({"kind": kind, "code": code, "message": message});
    if let Some(detail) = detail {
        out["detail"] = plain(root, detail);
    }
    json!({"error": out})
}

fn metadata_failure(root: &std::path::Path, error: &BuildMetadataError) -> Value {
    error_value(
        root,
        "metadata",
        error.code.as_str(),
        &error.message,
        Some(error.detail.as_ref().unwrap_or(&Value::Null)),
    )
}

fn trust_failure(root: &std::path::Path, error: &BuildMetadataTrustError) -> Value {
    error_value(
        root,
        "trust",
        error.code.as_str(),
        &error.message,
        Some(error.detail.as_ref().unwrap_or(&Value::Null)),
    )
}

fn read_failure(root: &std::path::Path, error: &ReadError) -> Value {
    match error {
        ReadError::Metadata(error) => metadata_failure(root, error),
        ReadError::Path(error) => {
            error_value(root, "delivery", error.code.as_str(), &error.message, None)
        }
        ReadError::Io(error) => error_value(
            root,
            "raw",
            error.code.as_deref().expect("у ошибки чтения есть код"),
            &error.message,
            None,
        ),
    }
}

/// `toPublic()` ошибки: `{code, message, detail}`.
fn public_value(root: &std::path::Path, mut public: Value) -> Value {
    let message = mask_json(&normalize_text(root, str_of(&public["message"])));
    public["message"] = json!(message);
    json!({"public": plain(root, &public)})
}

fn profile_of(value: &Value) -> Profile {
    Profile {
        game_id: str_of(&value["gameId"]).to_owned(),
        build_name: str_of(&value["buildName"]).to_owned(),
        architecture: str_of(&value["architecture"]).to_owned(),
        required_steam_app_id: value["requiredSteamAppId"].as_i64().unwrap(),
    }
}

/// Правка образца: поля `drop` убираются, поля `set` ставятся (существующие остаются на месте).
fn doc_of(step: &Value) -> String {
    if let Some(text) = opt_str(&step["text"]) {
        return text.to_owned();
    }
    let mut doc = FIXTURE["sample"].as_object().unwrap().clone();
    for field in step["drop"].as_array().into_iter().flatten() {
        doc.shift_remove(str_of(field));
    }
    for (field, value) in step["set"].as_object().into_iter().flatten() {
        doc.insert(field.clone(), value.clone());
    }
    stringify(&Value::Object(doc), "  ")
}

/// Время проверки, которое заменено на «сейчас», — метка.
fn nowish(iso: &str) -> Value {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    match js::date_parse(iso) {
        Some(ms) if (now - ms).abs() < 120_000 => json!("<now>"),
        _ => json!(iso),
    }
}

struct Runner {
    root: PathBuf,
    _temp: tempfile::TempDir,
}

impl Runner {
    fn new() -> Runner {
        let (temp, root) = temp_root();
        Runner { root, _temp: temp }
    }

    fn install(&self, step: &Value) -> PathBuf {
        if opt_str(&step["install"]) == Some("missing") {
            return path_in(&self.root, "no-such-install");
        }
        path_in(&self.root, opt_str(&step["installDir"]).unwrap_or("inst"))
    }

    fn validate_options<'a>(step: &'a Value, expected: Option<&'a Profile>) -> ValidateOptions<'a> {
        ValidateOptions {
            expected,
            now: step["now"].as_f64(),
            source: opt_str(&step["source"]),
        }
    }

    fn verified_value(&self, step: &Value, verified: &Verified) -> Value {
        let metadata = verified.metadata.to_value();
        let snapshot = snapshot_from_verified(verified).to_value();
        let mut verified_at = json!(verified.verified_at);
        let mut snapshot = snapshot;
        if step["now"].is_null() {
            verified_at = nowish(&verified.verified_at);
            snapshot["buildMetadataVerifiedAt"] = nowish(&verified.verified_at);
        }
        json!({"ok": {
            "verified": {
                "state": "trusted",
                "reason": null,
                "metadata": metadata,
                "filesEntry": verified.files_entry,
                "fileSha256": verified.file_sha256,
                "fileSize": verified.file_size,
                "verifiedAt": verified_at,
                "publishedAt": verified.published_at,
            },
            "snapshot": snapshot,
        }})
    }

    fn step(&self, step: &Value) -> Value {
        if let Some(common) = common_step(&self.root, step) {
            return common;
        }
        let root = self.root.as_path();
        let expected = step
            .get("expected")
            .filter(|v| !v.is_null())
            .map(profile_of);
        match str_of(&step["op"]) {
            "parse" => {
                let options = Self::validate_options(step, expected.as_ref());
                match parse(&doc_of(step), &options) {
                    Ok(dto) => json!({"ok": plain(root, &dto.to_value())}),
                    Err(error) => metadata_failure(root, &error),
                }
            }
            "parsePublic" => {
                let options = Self::validate_options(step, expected.as_ref());
                match parse(&doc_of(step), &options) {
                    Ok(_) => json!({"ok": null}),
                    Err(error) => public_value(root, error.to_public()),
                }
            }
            "descriptorPublic" => {
                match validate_descriptor(step.get("descriptor").unwrap_or(&Value::Null)) {
                    Ok(_) => json!({"ok": null}),
                    Err(error) => public_value(root, error.to_public()),
                }
            }
            "normalizeBuiltAt" => match normalize_built_at(&step["value"], step["now"].as_f64()) {
                Ok(iso) => json!({"ok": iso}),
                Err(error) => metadata_failure(root, &error),
            },
            "read" => {
                let options = ReadOptions {
                    expected: expected.as_ref(),
                    now: step["now"].as_f64(),
                };
                match reader::read(&self.install(step), &options) {
                    Ok(Reading::Absent) => {
                        json!({"ok": {"present": false, "relativePath": build_metadata::RELATIVE_PATH}})
                    }
                    Ok(Reading::Present(found)) => json!({"ok": {
                        "present": true,
                        "dto": found.dto.to_value(),
                        "rawLength": found.raw.encode_utf16().count(),
                        "rawSha256": sha256_hex(found.raw.as_bytes()),
                        "sizeBytes": found.size_bytes,
                        "sha256": found.sha256,
                        "relativePath": build_metadata::RELATIVE_PATH,
                    }}),
                    Err(error) => read_failure(root, &error),
                }
            }
            "validateDescriptor" => {
                let descriptor = step.get("descriptor").unwrap_or(&Value::Null);
                match validate_descriptor(descriptor) {
                    Ok(descriptor) => json!({"ok": descriptor}),
                    Err(error) => trust_failure(root, &error),
                }
            }
            "assertConsistent" => {
                let dto = dto_of(&FIXTURE["dto"]);
                let descriptor = step.get("descriptor").unwrap_or(&Value::Null);
                let manifest = step.get("manifest").unwrap_or(&Value::Null);
                match assert_consistent_with_manifest(&dto, descriptor, manifest) {
                    Ok(()) => json!({"ok": null}),
                    Err(error) => trust_failure(root, &error),
                }
            }
            "assertLauncher" => {
                let mut dto = FIXTURE["dto"].clone();
                for (field, value) in step["dtoSet"].as_object().into_iter().flatten() {
                    dto[field] = value.clone();
                }
                match assert_launcher_compatible(&dto_of(&dto), opt_str(&step["launcherVersion"])) {
                    Ok(()) => json!({"ok": null}),
                    Err(error) => trust_failure(root, &error),
                }
            }
            "verifyInstall" => {
                let options = VerifyForInstall {
                    install_root: &self.install(step),
                    descriptor: step.get("descriptor").unwrap_or(&Value::Null),
                    manifest: step.get("manifest"),
                    files_manifest: step.get("filesManifest"),
                    launcher_version: opt_str(&step["launcherVersion"]),
                    expected: expected.as_ref(),
                    now: step["now"].as_f64(),
                };
                match verify_for_install(&options) {
                    Ok(verified) => self.verified_value(step, &verified),
                    Err(error) => trust_failure(root, &error),
                }
            }
            "verifySnapshot" => {
                let snapshot = match step.get("meta") {
                    Some(meta) => {
                        let mut dto = FIXTURE["dto"].clone();
                        for (field, value) in meta.as_object().unwrap() {
                            dto[field] = value.clone();
                        }
                        Some(json!({"buildMetadata": dto}))
                    }
                    None => step.get("snapshot").cloned(),
                };
                let options = VerifyAgainstSnapshot {
                    install_root: &self.install(step),
                    snapshot: snapshot.as_ref(),
                    launcher_version: opt_str(&step["launcherVersion"]),
                    expected: expected.as_ref(),
                    now: step["now"].as_f64(),
                };
                json!({"ok": plain(root, &verify_against_snapshot(&options).to_value())})
            }
            other => panic!("неизвестный шаг {other}"),
        }
    }
}

/// Метаданные из значения: `minimumLauncherVersion` в шаге `assertLauncher` бывает любым, а разбор
/// строгий, поэтому запись собирается из полей.
fn dto_of(value: &Value) -> BuildMetadata {
    let text = |field: &str| str_of(&value[field]).to_owned();
    BuildMetadata {
        game_id: text("gameId"),
        version: text("version"),
        build_id: text("buildId"),
        build_name: text("buildName"),
        git_commit: text("gitCommit"),
        built_at: text("builtAt"),
        architecture: text("architecture"),
        required_steam_app_id: value["requiredSteamAppId"].as_i64().unwrap(),
        minimum_launcher_version: text("minimumLauncherVersion"),
    }
}

/// Результат сравнивается в виде JSON, каким его видит JS.
fn comparable(value: &Value) -> Value {
    js_plain(value)
}

fn run_scenarios(filter: impl Fn(&str) -> bool, what: &str) {
    let mut mismatches = Mismatches::default();
    let mut ran = 0;
    for scenario in items(&FIXTURE["scenarios"]) {
        let name = str_of(&scenario["name"]);
        if !filter(name) {
            continue;
        }
        if scenario["unix"].as_bool().unwrap_or(false) && cfg!(windows) {
            continue;
        }
        // Права доступа не действуют на root.
        if scenario["permissions"].as_bool().unwrap_or(false) && scenario::is_root_user() {
            continue;
        }
        ran += 1;
        let runner = Runner::new();
        scenario::setup(&runner.root, &scenario["setup"]);
        assert_eq!(
            items(&scenario["steps"]).len(),
            items(&scenario["results"]).len(),
            "шагов и результатов поровну"
        );
        for (index, step) in items(&scenario["steps"]).iter().enumerate() {
            let actual = comparable(&runner.step(step));
            let expected = comparable(&scenario["results"][index]);
            let context = format!("{name}: шаг {index} {}", short(step));
            mismatches.check(&actual, &expected, &context);
            // Равенство значений не видит порядка ключей, а в `installed.json` он попадает в файл.
            if actual == expected {
                let (actual, expected) = (stringify(&actual, ""), stringify(&expected, ""));
                mismatches.check_eq(actual, expected, format!("{context}: порядок ключей"));
            }
        }
        scenario::open_up(&runner.root);
    }
    assert!(ran > 0, "{what}: ни одного сценария");
    mismatches.finish(what);
}

/// Шаг для сообщения: крупные поля не печатаются целиком.
fn short(step: &Value) -> String {
    let text = step.to_string();
    if text.len() > 400 {
        format!("{}…", text.chars().take(400).collect::<String>())
    } else {
        text
    }
}

#[test]
fn constants_are_those_of_0_4_7() {
    let constants = &FIXTURE["constants"];
    assert_eq!(
        build_metadata::SCHEMA_VERSION,
        constants["SCHEMA_VERSION"].as_u64().unwrap()
    );
    assert_eq!(
        build_metadata::MAX_BYTES,
        constants["MAX_BYTES"].as_u64().unwrap()
    );
    assert_eq!(
        build_metadata::RELATIVE_PATH,
        str_of(&constants["RELATIVE_PATH"])
    );
    assert_eq!(
        build_metadata::REQUIRED_FIELDS.to_vec(),
        items(&constants["REQUIRED_FIELDS"])
            .iter()
            .map(str_of)
            .collect::<Vec<_>>()
    );
    let profile = Profile::product();
    let expected = &constants["PRODUCT_PROFILE"];
    assert_eq!(profile.game_id, str_of(&expected["gameId"]));
    assert_eq!(profile.build_name, str_of(&expected["buildName"]));
    assert_eq!(profile.architecture, str_of(&expected["architecture"]));
    assert_eq!(
        profile.required_steam_app_id,
        expected["requiredSteamAppId"].as_i64().unwrap()
    );
    assert_eq!(
        build_metadata::MetadataCode::Invalid.as_str(),
        str_of(&constants["CODES"]["BUILD_METADATA_INVALID"])
    );
    assert_eq!(
        build_metadata::MetadataCode::Unsupported.as_str(),
        str_of(&constants["CODES"]["BUILD_METADATA_UNSUPPORTED"])
    );
    let reasons = &constants["REASONS"];
    for (reason, key) in [
        (trust::Reason::Missing, "MISSING"),
        (trust::Reason::Invalid, "INVALID"),
        (trust::Reason::Mismatch, "MISMATCH"),
        (trust::Reason::Unsupported, "UNSUPPORTED"),
        (
            trust::Reason::RequiresNewerLauncher,
            "REQUIRES_NEWER_LAUNCHER",
        ),
    ] {
        assert_eq!(reason.as_str(), str_of(&reasons[key]), "{key}");
    }
    assert_eq!(
        trust::ALL_REASONS.map(trust::Reason::as_str).to_vec(),
        items(&constants["ALL_REASONS"])
            .iter()
            .map(str_of)
            .collect::<Vec<_>>()
    );
}

#[test]
fn parse_accepts_and_refuses_documents_like_0_4_7() {
    run_scenarios(|name| name.starts_with("parse:"), "build-metadata: parse");
}

#[test]
fn normalize_built_at_matches_node() {
    run_scenarios(
        |name| name.starts_with("normalizeBuiltAt"),
        "build-metadata: normalizeBuiltAt",
    );
}

#[test]
fn descriptor_consistency_and_launcher_checks_match_node() {
    run_scenarios(
        |name| {
            name.starts_with("toPublic")
                || name.starts_with("validateDescriptor")
                || name.starts_with("assertConsistentWithManifest")
                || name.starts_with("assertLauncherCompatible")
        },
        "build-metadata: descriptor",
    );
}

#[test]
fn read_checks_size_links_and_encoding_like_0_4_7() {
    run_scenarios(|name| name.starts_with("read:"), "build-metadata: read");
}

#[test]
fn verify_for_install_gives_the_same_reasons() {
    run_scenarios(
        |name| name.starts_with("verifyForInstall"),
        "build-metadata: verifyForInstall",
    );
}

#[test]
fn verify_against_snapshot_gives_the_same_states() {
    run_scenarios(
        |name| name.starts_with("verifyAgainstSnapshot"),
        "build-metadata: verifyAgainstSnapshot",
    );
}

#[test]
fn snapshot_and_metadata_keys_keep_the_order_of_0_4_7() {
    let dto = dto_of(&FIXTURE["dto"]);
    let verified = Verified {
        metadata: dto,
        files_entry: None,
        file_sha256: "0".repeat(64),
        file_size: 320,
        verified_at: "2026-10-15T12:00:00.000Z".to_owned(),
        published_at: Value::Null,
    };
    let metadata = verified.metadata.to_value();
    let metadata_keys: Vec<&String> = metadata.as_object().unwrap().keys().collect();
    assert_eq!(metadata_keys, build_metadata::REQUIRED_FIELDS);
    let snapshot = snapshot_from_verified(&verified).to_value();
    let keys: Vec<&String> = snapshot.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "buildMetadata",
            "buildMetadataVerifiedAt",
            "buildMetadataSha256",
            "publishedAt"
        ]
    );
    let inner: Vec<&String> = snapshot["buildMetadata"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    assert_eq!(inner, build_metadata::REQUIRED_FIELDS);
}
