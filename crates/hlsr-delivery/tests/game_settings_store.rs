//! `GameSettingsStore` против 0.4.7: фикстура `game-settings-store.json` из
//! `tools/reference/delivery/game-settings-store.js`. Сценарии — данные: файл `game-settings.json` любой
//! формы (нет файла, пустой, не JSON, не объект, `games` не объект или массив, чужие поля и записи,
//! числовые идентификаторы, имена свойств `Object.prototype`, сбои записи) и серия `get`/`save`/`reset`/
//! `launchArgumentsFor`. После каждого шага сверяются ответ, строки журнала, байты файла (размер, sha256,
//! текст короткого файла) и список имён в папке (временных файлов быть не должно).

mod support;

use hlsr_core::js;
use hlsr_core::json_text::stringify;
use hlsr_delivery::errors::RawError;
use hlsr_delivery::game_settings_store::{GameSettingsStore, StoreError, is_safe_game_id};
use hlsr_delivery::ports::MemoryLogger;
use regex::Regex;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use support::scenario::{common_step, normalize_text, path_in, setup, temp_root};
use support::{Mismatches, fixture, items, opt_str, sha256_hex, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("game-settings-store.json"));

const TEXT_LIMIT: usize = 400;

fn ser(value: &Value) -> String {
    stringify(value, "")
}

/// Длина и sha256 записи `JSON.stringify` (как в сценарии); `vulkan` 0.4.9 не сравнивается.
fn summary(value: &Value) -> Value {
    let text = ser(&support::without_vulkan(value));
    json!({"length": text.len(), "sha256": sha256_hex(text.as_bytes())})
}

/// Имя временного файла атомарной записи в тексте ошибки: `.<имя>.<pid>.<время36>.tmp`.
fn mask_temp(text: &str) -> String {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(/\.[^/'\s]+)\.\d+\.[0-9a-z]+\.tmp").unwrap());
    RE.replace_all(text, "${1}.<pid>.<time>.tmp").into_owned()
}

fn snapshot_file(file: &Path) -> Value {
    match std::fs::read(file).map(|bytes| support::without_vulkan_lines(&bytes)) {
        Ok(bytes) => {
            let mut out = json!({"size": bytes.len(), "sha256": sha256_hex(&bytes)});
            if bytes.len() <= TEXT_LIMIT {
                out["latin1"] = json!(js::latin1_string(&bytes));
            }
            out
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(error) => {
            json!({"unreadable": hlsr_core::js::errno::code(&error).map(|c| c.into_owned())})
        }
    }
}

/// Имена в папке сценария (с `/` у папок), по порядку JS.
fn names(root: &Path) -> Value {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = format!("{prefix}{name}");
            if entry.file_type().unwrap().is_dir() {
                out.push(format!("{rel}/"));
                walk(&entry.path(), &format!("{rel}/"), out);
            } else {
                out.push(rel);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, "", &mut out);
    out.sort_by(|a, b| js::cmp_utf16(a, b));
    json!(out)
}

fn threw(root: &Path, error: &RawError) -> Value {
    json!({"threw": {
        "code": error.code,
        "message": mask_temp(&normalize_text(root, &error.message)),
    }})
}

fn run_step(root: &Path, store: &GameSettingsStore, step: &Value) -> Value {
    if let Some(common) = common_step(root, step) {
        return json!({"common": common});
    }
    let id = || str_of(&step["id"]);
    match str_of(&step["op"]) {
        "get" => json!({"ok": summary(&store.get(id()).to_value())}),
        "save" => match store.save(id(), &step["settings"]) {
            Ok(settings) => json!({"ok": {"ok": true, "settings": summary(&settings.to_value())}}),
            Err(StoreError::Rejected(error)) => {
                json!({"ok": {"ok": false, "error": error.to_value()}})
            }
            Err(StoreError::Io(error)) => threw(root, &error),
        },
        "reset" => match store.reset(id()) {
            Ok(settings) => json!({"ok": {"ok": true, "settings": summary(&settings.to_value())}}),
            Err(StoreError::Rejected(error)) => {
                json!({"ok": {"ok": false, "error": error.to_value()}})
            }
            Err(StoreError::Io(error)) => threw(root, &error),
        },
        "launchArgs" => {
            let trusted: Vec<String> = step["trusted"]
                .as_array()
                .map(|list| list.iter().map(|a| str_of(a).to_owned()).collect())
                .unwrap_or_default();
            json!({"ok": store.launch_arguments_for(id(), &trusted)})
        }
        "defaultFile" => {
            let file = GameSettingsStore::default_file(root);
            json!({"ok": normalize_text(root, &file.to_string_lossy())})
        }
        "isSafe" => {
            let value = &step["value"];
            json!({"ok": value.as_str().is_some_and(is_safe_game_id)})
        }
        other => panic!("неизвестный шаг {other}"),
    }
}

/// Ответ шага без имени типа исключения (Rust отдаёт код и текст, тип у него один).
fn without_type(mut expected: Value) -> Value {
    if let Some(threw) = expected.get_mut("threw").and_then(Value::as_object_mut) {
        threw.remove("type");
    }
    expected
}

fn run_scenario(scenario: &Value, m: &mut Mismatches) {
    let name = str_of(&scenario["name"]);
    if scenario["unix"].as_bool() == Some(true) && !cfg!(unix) {
        return;
    }
    let (temp, root) = temp_root();
    setup(&root, &scenario["setup"]);
    let file: PathBuf = path_in(
        &root,
        opt_str(&scenario["file"]).unwrap_or("game-settings.json"),
    );
    let logger = Arc::new(MemoryLogger::default());
    let store = GameSettingsStore::new(file.clone(), logger.clone());
    let mut state = Value::Null;
    assert_eq!(
        items(&scenario["steps"]).len(),
        items(&scenario["results"]).len(),
        "шагов и результатов поровну"
    );
    for (index, (step, expected)) in items(&scenario["steps"])
        .iter()
        .zip(items(&scenario["results"]))
        .enumerate()
    {
        let context = format!("{name}, шаг {index}: {}", ser(step));
        let result = run_step(&root, &store, step);
        m.check(
            &result,
            &without_type(expected["result"].clone()),
            format!("{context}: ответ"),
        );
        let logs: Vec<Value> = logger
            .take()
            .into_iter()
            .map(|(level, message)| json!({"level": level, "message": message}))
            .collect();
        m.check(
            &json!(logs),
            &expected["logs"],
            format!("{context}: журнал"),
        );
        // Состояние файла и папки записано в фикстуре, только если изменилось.
        if expected.get("file").is_some() || expected.get("names").is_some() {
            state = json!({"file": expected["file"], "names": expected["names"]});
        }
        let actual = json!({"file": snapshot_file(&file), "names": names(&root)});
        m.check(&actual, &state, format!("{context}: файл и папка"));
    }
    support::scenario::open_up(&root);
    drop(temp);
}

#[test]
fn scenarios_match_0_4_7() {
    // Сценарии с правами доступа (chmod) сняты не от root: root их права не останавливают.
    let root = support::scenario::is_root_user();
    let mut m = Mismatches::default();
    for scenario in items(&FIXTURE["scenarios"]) {
        if root
            && serde_json::to_string(scenario)
                .unwrap()
                .contains("\"mode\"")
        {
            eprintln!("{}: от root пропущен", str_of(&scenario["name"]));
            continue;
        }
        run_scenario(scenario, &mut m);
    }
    m.finish("GameSettingsStore");
}

#[test]
fn file_name_and_version_match_0_4_7() {
    assert_eq!(
        FIXTURE["constants"]["FILE_NAME"],
        json!("game-settings.json")
    );
    assert_eq!(FIXTURE["constants"]["FILE_VERSION"], json!(1));
    assert_eq!(
        hlsr_delivery::game_settings_store::FILE_NAME,
        "game-settings.json"
    );
    assert_eq!(hlsr_delivery::game_settings_store::FILE_VERSION, 1);
}
