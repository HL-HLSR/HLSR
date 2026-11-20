//! Чистые части диагностики против 0.4.7 (фикстура `diagnostics.json`, `tools/reference/update/
//! diagnostics.js`): маскирование (`redact.js`: `redactText` с `userName`, `safeEnv`,
//! `isSecretEnvName`), `buildManifest`, `log-policy.js` (`logFiles`, `gameLogFiles`), `collect`,
//! имя файла отчёта по времени. Самотест «отчёт берёт журнал запуска игры под Proton и тип
//! графической сессии» перенесён целиком.

mod support;

use hlsr_core::js;
use hlsr_core::redact::{ALLOWED_ENV, is_secret_env_name, redact_text_for, safe_env};
use hlsr_update::diagnostics::export::{CollectOptions, ManifestInput, build_manifest, collect};
use hlsr_update::diagnostics::log_policy::{game_log_files, log_files};
use hlsr_update::diagnostics::zip::timestamp_name_at;
use serde_json::{Map, Value, json};
use std::path::Path;
use std::sync::LazyLock;
use support::{Mismatches, fixture, items, sha256_hex, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("diagnostics.json"));

fn pairs(env: &Value) -> Vec<(String, String)> {
    env.as_object()
        .map(|map| {
            map.iter()
                .map(|(k, v)| (k.clone(), str_of(v).to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

fn object(pairs: Vec<(String, String)>) -> Value {
    Value::Object(
        pairs
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect::<Map<_, _>>(),
    )
}

// ------------------------------------------------------------------ маскирование

#[test]
fn redact_text_matches_the_reference() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["redact"]["cases"]) {
        let text = str_of(&case["text"]);
        let user = case["userName"].as_str();
        mismatches.check_eq(
            redact_text_for(text, user),
            str_of(&case["out"]).to_owned(),
            format!("{text:?} / userName {user:?}"),
        );
    }
    mismatches.finish("redactText");
}

#[test]
fn safe_env_matches_the_reference() {
    assert_eq!(
        ALLOWED_ENV.to_vec(),
        items(&FIXTURE["redact"]["allowedEnv"])
            .iter()
            .map(str_of)
            .collect::<Vec<_>>()
    );
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["redact"]["env"]) {
        let allow: Option<Vec<&str>> = case["allow"]
            .as_array()
            .map(|list| list.iter().map(str_of).collect());
        let out = safe_env(
            &pairs(&case["env"]),
            allow.as_deref(),
            case["userName"].as_str(),
        );
        mismatches.check(
            &object(out),
            &case["out"],
            format!("safeEnv {} / {}", case["env"], case["allow"]),
        );
    }
    mismatches.finish("safeEnv");
}

#[test]
fn secret_env_names_match_the_reference() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["redact"]["secretNames"]) {
        let name = str_of(&case["name"]);
        mismatches.check_eq(
            is_secret_env_name(name),
            case["out"].as_bool().unwrap(),
            format!("isSecretEnvName({name:?})"),
        );
    }
    mismatches.finish("isSecretEnvName");
}

// ------------------------------------------------------------------ buildManifest, collect

fn manifest_input(input: &Value) -> ManifestInput {
    let text = |key: &str| input[key].as_str().unwrap_or("").to_owned();
    ManifestInput {
        now: text("now"),
        app_name: text("appName"),
        app_version: text("appVersion"),
        build_channel: text("buildChannel"),
        electron_version: text("electronVersion"),
        chrome_version: text("chromeVersion"),
        node_version: text("nodeVersion"),
        arch: text("arch"),
        platform: text("platform"),
        os_release: text("osRelease"),
        library_path: text("libraryPath"),
        library_exists: input.get("libraryExists").is_some_and(js::truthy),
        update_state: text("updateState"),
        update_error_code: input["updateErrorCode"].as_str().map(str::to_owned),
        env: pairs(&input["env"]),
        user_name: input["userName"].as_str().map(str::to_owned),
    }
}

#[test]
fn build_manifest_matches_the_reference() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["manifests"]) {
        mismatches.check(
            &build_manifest(&manifest_input(&case["input"])),
            &case["out"],
            format!("buildManifest({})", case["input"]),
        );
    }
    mismatches.finish("buildManifest");
}

#[test]
fn manifest_json_keeps_the_order_of_keys() {
    let manifest = build_manifest(&manifest_input(&FIXTURE["manifests"][1]["input"]));
    let keys: Vec<&String> = manifest.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "schema",
            "generatedAt",
            "app",
            "runtime",
            "os",
            "library",
            "update",
            "env"
        ]
    );
}

// ------------------------------------------------------------------ log-policy

fn layout(root: &Path, files: &Value) {
    let Some(files) = files.as_object() else {
        return;
    };
    for (name, content) in files {
        let file = name.split('/').fold(root.to_path_buf(), |p, s| p.join(s));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let bytes: Vec<u8> = match content {
            Value::String(text) => text.clone().into_bytes(),
            Value::Object(hex) => decode_hex(str_of(&hex["hex"])),
            other => panic!("содержимое файла: {other}"),
        };
        std::fs::write(file, bytes).unwrap();
    }
}

fn decode_hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap())
        .collect()
}

fn relative(root: &Path, files: Vec<String>) -> Vec<String> {
    let root = root.to_string_lossy().into_owned();
    files
        .into_iter()
        .map(|file| {
            file.strip_prefix(&root)
                .unwrap_or(&file)
                .trim_start_matches(['/', '\\'])
                .replace('\\', "/")
        })
        .collect()
}

#[test]
fn log_policy_matches_the_reference() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["logPolicy"]) {
        if case.get("tree").is_none() {
            // Каталога нет — журналов игр нет.
            let missing = std::env::temp_dir().join("hlsr-no-such-dir-for-logs");
            mismatches.check_eq(
                game_log_files(&missing.to_string_lossy()),
                Vec::<String>::new(),
                "gameLogFiles(нет каталога)",
            );
            continue;
        }
        let dir = tempfile::tempdir().unwrap();
        layout(dir.path(), &case["files"]);
        let root = dir.path().to_string_lossy().into_owned();
        let tree = str_of(&case["tree"]);
        if let Some(expected) = case.get("gameLogFiles") {
            let actual = relative(dir.path(), game_log_files(&root));
            let expected: Vec<String> = items(expected)
                .iter()
                .map(|f| str_of(f).to_owned())
                .collect();
            mismatches.check_eq(actual, expected, format!("gameLogFiles в «{tree}»"));
            continue;
        }
        let max = case["maxArchives"].as_u64().map(|n| n as usize);
        let actual = relative(dir.path(), log_files(&root, str_of(&case["baseName"]), max));
        let expected: Vec<String> = items(&case["logFiles"])
            .iter()
            .map(|f| str_of(f).to_owned())
            .collect();
        mismatches.check_eq(
            actual,
            expected,
            format!("logFiles({}, {max:?}) в «{tree}»", case["baseName"]),
        );
    }
    mismatches.finish("log-policy");
}

#[test]
fn collect_matches_the_reference() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["collect"]) {
        let id = str_of(&case["id"]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let logs_dir = match &case["logs"] {
            Value::Null => None,
            Value::String(_) => Some(root.join("logs")),
            logs => {
                let named = Value::Object(
                    logs.as_object()
                        .unwrap()
                        .iter()
                        .map(|(name, content)| (format!("logs/{name}"), content.clone()))
                        .collect(),
                );
                layout(root, &named);
                std::fs::create_dir_all(root.join("logs")).unwrap();
                Some(root.join("logs"))
            }
        };
        let opts = &case["opts"];
        let options = CollectOptions {
            manifest: ManifestInput {
                now: "2026-10-16T09:30:05.000Z".to_owned(),
                app_name: "hlsr".to_owned(),
                app_version: "0.5.0".to_owned(),
                platform: "linux".to_owned(),
                arch: "x64".to_owned(),
                env: pairs(&opts["env"]),
                user_name: opts["userName"].as_str().map(str::to_owned),
                ..ManifestInput::default()
            },
            out_dir: root.join("out"),
            logs_dir,
            log_name: opts["logName"].as_str().unwrap_or("").to_owned(),
            max_log_bytes: opts["maxLogBytes"].as_u64(),
            max_archives: opts["maxArchives"].as_u64().map(|n| n as usize),
        };
        let collected = collect(&options).unwrap_or_else(|e| panic!("{id}: {e}"));
        let files: Vec<Value> = collected
            .files
            .iter()
            .map(|file| {
                let bytes = std::fs::read(file).unwrap();
                json!({
                    "name": file.file_name().unwrap().to_string_lossy(),
                    "size": bytes.len(),
                    "sha256": sha256_hex(&bytes),
                })
            })
            .collect();
        mismatches.check(&Value::Array(files), &case["files"], format!("{id}: файлы"));
        mismatches.check(
            &collected.manifest,
            &case["manifest"],
            format!("{id}: манифест"),
        );
    }
    mismatches.finish("collect");
}

/// «отчёт берёт журнал запуска игры под Proton и тип графической сессии»
#[test]
fn report_takes_the_proton_game_log_and_the_graphical_session() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(logs.join("main.log"), "[info] старт\n").unwrap();
    std::fs::write(
        logs.join("proton-ghosting-renewed.log"),
        "# запуск\nerr: /home/alice/Games/HLSR/hl2.exe\n",
    )
    .unwrap();
    std::fs::write(logs.join("proton-Bad_Name.log"), "не журнал игры\n").unwrap();
    let env = [
        ("XDG_CURRENT_DESKTOP", "KDE"),
        ("WAYLAND_DISPLAY", "wayland-0"),
        ("DISPLAY", ":0"),
        ("HLSR_TOKEN", "secret"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
    .collect();
    let collected = collect(&CollectOptions {
        manifest: ManifestInput {
            env,
            user_name: Some("alice".to_owned()),
            ..ManifestInput::default()
        },
        out_dir: dir.path().join("out"),
        logs_dir: Some(logs),
        ..CollectOptions::default()
    })
    .unwrap();
    let mut names: Vec<String> = collected
        .files
        .iter()
        .map(|file| file.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["main.log", "manifest.json", "proton-ghosting-renewed.log"]
    );
    let text = std::fs::read_to_string(dir.path().join("out/proton-ghosting-renewed.log")).unwrap();
    assert!(
        !text.contains("alice"),
        "имя пользователя в путях скрыто: {text}"
    );
    assert_eq!(
        collected.manifest["env"],
        json!({"XDG_CURRENT_DESKTOP": "KDE", "WAYLAND_DISPLAY": "wayland-0", "DISPLAY": ":0"})
    );
}

// ------------------------------------------------------------------ имя файла отчёта

#[test]
fn report_file_name_matches_the_reference() {
    // Фикстура снята при TZ=UTC.
    for case in items(&FIXTURE["timestamps"]) {
        assert_eq!(
            timestamp_name_at(case["ms"].as_i64().unwrap(), 0),
            str_of(&case["name"]),
            "{}",
            case["ms"]
        );
    }
}
