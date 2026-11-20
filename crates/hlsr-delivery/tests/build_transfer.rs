//! Перенос сборки (`.hlsrbuild`) против 0.4.7: фикстура `build-transfer.json` из
//! `tools/reference/delivery/build-transfer.js`.
//!
//! `collect` (изменённые и свои cfg), `serialize` (JSON с отступом 1 — текст побайтно), `parse`
//! (формат, версия, игра, пути, base64, sha256, пределы — корпус мутаций документа) и
//! `apply_files` (запись только внутрь папки игры, ссылки, сбои посередине). Сценарий — файлы и
//! шаги в данных; их выполняют модуль 0.4.7 в Node 24 и этот модуль на тех же деревьях.
//!
//! Сценарии, помеченные `unix`, зависят от ОС (ссылки, права доступа, ошибки rename): на Windows
//! они пропускаются.

mod support;

use base64::Engine as _;
use hlsr_core::js;
use hlsr_core::json_text::stringify;
use hlsr_delivery::build_transfer::{
    self, Build, PickedFile, TransferFailure, apply_files, collect_in, parse, serialize_at,
};
use hlsr_delivery::game_settings_store::GameSettingsStore;
use hlsr_delivery::ports::TracingLogger;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, UNIX_EPOCH};
use support::scenario::{self, bytes_of, common_step, path_in, temp_root};
use support::{Mismatches, fixture, items, opt_str, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("build-transfer.json"));

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Файл в виде, который сравнивается: путь, размер, sha256 данных.
fn describe(file: &PickedFile) -> Value {
    json!({"path": file.path, "size": file.data.len(), "sha256": sha256_hex(&file.data)})
}

/// Больше 40 файлов — сводка.
fn summarize(list: Vec<Value>) -> Value {
    if list.len() <= 40 {
        return Value::Array(list);
    }
    let text = stringify(&Value::Array(list.clone()), "");
    json!({
        "count": list.len(),
        "head": list[..3],
        "tail": list[list.len() - 3..],
        "sha256": sha256_hex(text.as_bytes()),
    })
}

/// Крупное значение (настройки на 12 млн знаков) — длиной и sha256 JSON.
fn big_value(value: Option<&Value>) -> Value {
    let Some(value) = value else {
        return Value::Null;
    };
    let text = stringify(value, "");
    if text.encode_utf16().count() > 4000 {
        json!({"jsonLength": text.encode_utf16().count(), "sha256": sha256_hex(text.as_bytes())})
    } else {
        value.clone()
    }
}

fn failure(error: &TransferFailure) -> Value {
    match error {
        TransferFailure::Transfer(error) => {
            json!({"error": {"kind": "transfer", "code": error.code.as_str()}})
        }
        TransferFailure::Raw(error) => json!({"error": {"kind": "raw", "code": error.code}}),
        TransferFailure::Js(_) => json!({"error": {"kind": "type", "type": "TypeError"}}),
    }
}

fn picked(files: &Value) -> Vec<PickedFile> {
    items(files)
        .iter()
        .map(|f| PickedFile {
            path: str_of(&f["path"]).to_owned(),
            data: bytes_of(f),
        })
        .collect()
}

/// Текст документа в шаге: строка или `{prefix, repeat: [кусок, раз], suffix}`.
fn text_of(step: &Value) -> String {
    if let Some(gen_) = step.get("gen") {
        let files: Vec<Value> = (0..gen_["files"].as_u64().unwrap())
            .map(|i| {
                let data = vec![
                    str_of(&gen_["byte"]).as_bytes()[0];
                    gen_["size"].as_u64().unwrap() as usize
                ];
                json!({
                    "path": format!("valve/f{i:03}.cfg"),
                    "sha256": sha256_hex(&data),
                    "data": base64::engine::general_purpose::STANDARD.encode(&data),
                })
            })
            .collect();
        return json!({"format": "hlsr-build", "version": 1, "game": {"gameId": "hl1-won"}, "files": files})
            .to_string();
    }
    match &step["text"] {
        Value::String(text) => text.clone(),
        big => {
            format!(
                "{}{}{}",
                opt_str(&big["prefix"]).unwrap_or_default(),
                str_of(&big["repeat"][0]).repeat(big["repeat"][1].as_u64().unwrap() as usize),
                opt_str(&big["suffix"]).unwrap_or_default()
            )
        }
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
        match opt_str(&step["installDir"]) {
            Some(dir) => path_in(&self.root, dir),
            None => self.root.clone(),
        }
    }

    fn step(&self, step: &Value) -> Value {
        if let Some(common) = common_step(&self.root, step) {
            return common;
        }
        let install = self.install(step);
        match str_of(&step["op"]) {
            "collect" => match collect_in(
                &install,
                step.get("previous").filter(|p| !p.is_null()),
                opt_str(&step["locale"]).unwrap_or("en-US"),
            ) {
                Ok(files) => json!({"ok": summarize(files.iter().map(describe).collect())}),
                Err(error) => failure(&error),
            },
            "serialize" => {
                let b = &step["build"];
                let text_field = |name: &str| opt_str(&b[name]);
                let ms = b["exportedAt"].as_f64().filter(|ms| *ms != 0.0);
                let fixed_now = UNIX_EPOCH + Duration::from_millis(1_800_000_000_000);
                let files = picked(&step["files"]);
                let build = Build {
                    launcher_id: str_of(&b["launcherId"]),
                    game_id: str_of(&b["gameId"]),
                    build: text_field("build"),
                    version: text_field("version"),
                    launcher_version: text_field("launcherVersion"),
                    settings: b.get("settings"),
                    files: &files,
                    exported_at: ms.map(|ms| UNIX_EPOCH + Duration::from_millis(ms as u64)),
                };
                let mut text = serialize_at(&build, fixed_now);
                // Время «сейчас» в фикстуре — метка.
                text = text.replace(&js::iso_string(fixed_now), "<now>");
                if text.encode_utf16().count() <= 4000 {
                    json!({"text": text})
                } else {
                    json!({"length": text.encode_utf16().count(), "sha256": sha256_hex(text.as_bytes())})
                }
            }
            "parse" => {
                let text = text_of(step);
                let known = |id: &str| id == "hl1-won";
                let is_known: Option<&dyn Fn(&str) -> bool> =
                    (opt_str(&step["known"]) == Some("hl1-won")).then_some(&known);
                match parse(&text, is_known) {
                    Ok(parsed) => json!({"ok": {
                        "gameId": parsed.game_id,
                        "launcherId": parsed.launcher_id,
                        "build": parsed.build,
                        "version": parsed.version,
                        "exportedAt": parsed.exported_at,
                        "settings": big_value(parsed.settings.as_ref()),
                        "files": summarize(parsed.files.iter().map(describe).collect()),
                    }}),
                    Err(error) => failure(&error),
                }
            }
            "apply" => match apply_files(&install, &picked(&step["files"])) {
                Ok(written) => json!({"ok": written}),
                Err(error) => failure(&error),
            },
            other => panic!("неизвестный шаг {other}"),
        }
    }
}

#[test]
fn constants_are_those_of_0_4_7() {
    let constants = &FIXTURE["constants"];
    assert_eq!(build_transfer::FORMAT, str_of(&constants["FORMAT"]));
    assert_eq!(
        build_transfer::FORMAT_VERSION,
        constants["FORMAT_VERSION"].as_u64().unwrap()
    );
    assert_eq!(build_transfer::EXTENSION, str_of(&constants["EXTENSION"]));
    assert_eq!(
        hlsr_delivery::cfg_files::MAX_FILE_BYTES,
        constants["MAX_FILE_BYTES"].as_u64().unwrap()
    );
}

fn run_scenarios(filter: impl Fn(&str) -> bool, what: &str) {
    let mut mismatches = Mismatches::default();
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
        let runner = Runner::new();
        scenario::setup(&runner.root, &scenario["setup"]);
        assert_eq!(
            items(&scenario["steps"]).len(),
            items(&scenario["results"]).len(),
            "шагов и результатов поровну"
        );
        for (index, step) in items(&scenario["steps"]).iter().enumerate() {
            let actual = runner.step(step);
            mismatches.check(
                &actual,
                &scenario["results"][index],
                format!("{name}: шаг {index} {}", short(step)),
            );
        }
        scenario::open_up(&runner.root);
    }
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
fn collect_takes_edited_and_own_cfgs_in_list_order() {
    run_scenarios(
        |name| name.starts_with("collect"),
        "build-transfer: collect",
    );
}

#[test]
fn serialize_writes_json_with_indent_one_byte_for_byte() {
    run_scenarios(
        |name| name.starts_with("serialize"),
        "build-transfer: serialize",
    );
}

#[test]
fn parse_accepts_and_refuses_documents_like_0_4_7() {
    run_scenarios(|name| name.starts_with("parse"), "build-transfer: parse");
}

#[test]
fn apply_files_writes_only_inside_the_game_folder() {
    run_scenarios(|name| name.starts_with("apply"), "build-transfer: apply");
}

// ---------------------------------------------------------------- 0.4.9: build-transfer-selftest.js

fn settings_store() -> (tempfile::TempDir, GameSettingsStore) {
    let (temp, root) = temp_root();
    let store = GameSettingsStore::new(root.join("game-settings.json"), Arc::new(TracingLogger));
    (temp, store)
}

#[test]
fn settings_from_a_file_do_not_set_proton_commands_own_ones_stay() {
    let (_temp, store) = settings_store();
    store
        .save(
            "ghosting-renewed",
            &json!({"protonOptions": "mangohud %command%", "console": true}),
        )
        .unwrap();
    let saved = store
        .save_imported(
            "ghosting-renewed",
            &json!({"protonOptions": "sh -c \"id > /tmp/pwned\" %command%", "console": false, "novid": true}),
        )
        .unwrap()
        .to_value();
    assert_eq!(saved["protonOptions"], "mangohud %command%");
    assert_eq!(saved["console"], false);
    assert_eq!(saved["novid"], true);
}

#[test]
fn settings_from_a_file_do_not_set_commands_or_paths_in_launch_args() {
    let (_temp, store) = settings_store();
    store
        .save("ghosting-renewed", &json!({"launchArgs": "-novid"}))
        .unwrap();
    for args in [
        "+exec //host/share/a.cfg",
        "+connect 203.0.113.1",
        "-config //host/a",
        "-config C:/a.cfg",
        "-x ../a",
    ] {
        let saved = store
            .save_imported("ghosting-renewed", &json!({"launchArgs": args}))
            .unwrap();
        assert_eq!(saved.launch_args, "-novid", "{args}");
    }
}

#[test]
fn plain_launch_args_from_a_file_are_taken() {
    let (_temp, store) = settings_store();
    let saved = store
        .save_imported(
            "ghosting-renewed",
            &json!({"launchArgs": "-w 1920 -h 1080 -freq 144 -high"}),
        )
        .unwrap();
    assert_eq!(saved.launch_args, "-w 1920 -h 1080 -freq 144 -high");
}

/// cfg занят другой программой: открыт без общего доступа (как держит его игра или редактор).
#[cfg(windows)]
#[tokio::test]
async fn locked_cfg_export_names_the_file() {
    use hlsr_delivery::game_actions::LauncherIdentity;
    use std::os::windows::fs::OpenOptionsExt;
    use support::api::{Setup, harness};
    let (_temp, root) = temp_root();
    let install = root.join("lib").join("Games").join("hl1-won");
    for (rel, data) in [
        ("valve/config.cfg", "stock\n"),
        ("valve/autoexec.cfg", "мой autoexec\n"),
        ("valve/scripts/jump.cfg", "alias j +jump\n"),
    ] {
        let file = path_in(&install, rel);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, data).unwrap();
    }
    let previous = json!({"files": [
        {"path": "valve/config.cfg", "sha256": sha256_hex(b"stock\n")},
        {"path": "valve/autoexec.cfg", "sha256": sha256_hex(b"stock autoexec\n")},
    ]});
    let _held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path_in(&install, "valve/autoexec.cfg"))
        .unwrap();

    match build_transfer::collect(&install, Some(&previous)) {
        Err(TransferFailure::Transfer(error)) => {
            assert_eq!(error.code.as_str(), "FILE_LOCKED");
            assert_eq!(error.file.as_deref(), Some("valve/autoexec.cfg"));
            assert_eq!(error.to_string(), "valve/autoexec.cfg: EBUSY");
        }
        other => panic!("{other:?}"),
    }

    let installed = json!({"schemaVersion": 1, "gameId": "hl1-won", "state": "ready",
        "installPath": install.to_string_lossy(), "installedAt": "2026-11-01T12:00:00.000Z"});
    std::fs::write(install.join("installed.json"), stringify(&installed, "  ")).unwrap();
    let h = harness(Setup {
        root: root.clone(),
        registry: None,
        now: 1_792_152_000_000,
        launcher: LauncherIdentity {
            exec_path: PathBuf::from("C:/hlsr/HLSR.exe"),
            app_image: None,
            packaged: true,
            app_path: None,
        },
    });
    let answer = h
        .api
        .call("game-build-export-settings", &[json!("hl1-won")])
        .await
        .unwrap();
    assert_eq!(
        answer,
        json!({"status": "error", "code": "FILE_LOCKED", "file": "valve/autoexec.cfg"})
    );
}
