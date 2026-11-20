//! Запуск GoldSrc (`goldsrc-launch.js` 0.4.7): чистые функции и функции над папкой против фикстуры
//! `tools/reference/platform/goldsrc.js` и случаи самотеста `scripts/hl1-selftest.js` (флаги WON,
//! RInput по умолчанию, недоступная папка логов), а также порт [`GoldSrc`] для Linux.

mod proton_support;
mod support;

use hlsr_core::js::path::NodePath;
use hlsr_core::logger::MemoryLogger;
use hlsr_delivery::errors::DeliveryError;
use hlsr_delivery::launch_args;
use hlsr_delivery::manifests::{LaunchConfig, launch_config_of};
use hlsr_platform::goldsrc_launch::{
    self, BXT_LOG_FILE, GOLDSRC_TOGGLES, GoldSrc, GoldSrcLaunch, environment_of, is_goldsrc,
    merge_arguments, prepare_bxt_log, resolve_files, resolve_rinput,
};
use proton_support::{Root, write_file};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use support::{Mismatches, fixture, items};

fn hl1() -> &'static LaunchConfig {
    launch_config_of("hl1-won").unwrap()
}

fn leak_str(text: &str) -> &'static str {
    Box::leak(text.to_owned().into_boxed_str())
}

fn leak_strs(values: &Value) -> &'static [&'static str] {
    let list: Vec<&'static str> = items(values)
        .iter()
        .map(|value| leak_str(value.as_str().unwrap()))
        .collect();
    Box::leak(list.into_boxed_slice())
}

/// Настройки HL1 с правками сценария (`null` — поля нет).
fn config_with(overrides: &Value) -> LaunchConfig {
    let mut config = hl1().clone();
    let Some(overrides) = overrides.as_object() else {
        return config;
    };
    for (key, value) in overrides {
        match key.as_str() {
            "executable" => config.executable = leak_str(value.as_str().unwrap()),
            "injector" => config.injector = value.as_str().map(leak_str),
            "steamAppIdFile" => config.steam_app_id_file = value.as_str().map(leak_str),
            "rinput" => {
                config.rinput_executable = value["executable"].as_str().map(leak_str);
            }
            other => panic!("неизвестная правка {other}"),
        }
    }
    config
}

/// Корень и его родитель (`<root>`, `<tmp>`), как `norm` скрипта.
fn norm_text(root: &Root, text: &str) -> String {
    let parent = Path::new(&root.text)
        .parent()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut text = text.to_owned();
    for (base, label) in [(root.text.as_str(), "<root>"), (parent.as_str(), "<tmp>")] {
        for variant in [
            base.replace('\\', "\\\\"),
            base.to_owned(),
            base.replace('\\', "/"),
        ] {
            text = text.replace(&variant, label);
        }
    }
    text.replace("\\\\", "/").replace('\\', "/")
}

fn norm(root: &Root, value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(norm_text(root, text)),
        Value::Array(list) => Value::Array(list.iter().map(|item| norm(root, item)).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, item)| (key.clone(), norm(root, item)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn layout(root: &Root, items: &Value) {
    for item in items.as_array().into_iter().flatten() {
        let path = NodePath::native();
        if let Some(dir) = item["dir"].as_str() {
            std::fs::create_dir_all(path.join(&[&root.text, dir])).unwrap();
            continue;
        }
        let file = path.join(&[&root.text, item["file"].as_str().unwrap()]);
        let bytes = match item["hex"].as_str() {
            Some(hex) => (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect(),
            None => item["utf8"].as_str().unwrap().as_bytes().to_vec(),
        };
        write_file(Path::new(&file), bytes, None);
    }
}

/// Снимок дерева скрипта: файлы с содержимым в hex.
fn tree(root: &Root) -> Value {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<Value>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if entry.file_type().unwrap().is_dir() {
                out.push(json!({"path": rel, "kind": "dir"}));
                walk(&entry.path(), &rel, out);
            } else {
                let bytes = std::fs::read(entry.path()).unwrap();
                let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
                out.push(json!({"path": rel, "kind": "file", "hex": hex}));
            }
        }
    }
    let mut out = Vec::new();
    walk(Path::new(&root.text), "", &mut out);
    out.sort_by(|a, b| {
        hlsr_core::js::cmp_utf16(a["path"].as_str().unwrap(), b["path"].as_str().unwrap())
    });
    Value::Array(out)
}

fn attempt<T>(root: &Root, result: Result<T, DeliveryError>, ok: impl FnOnce(T) -> Value) -> Value {
    match result {
        Ok(value) => json!({"ok": norm(root, &ok(value))}),
        Err(error) => json!({"error": norm(root, &json!({
            "code": error.code.as_str(),
            "message": error.message,
            "details": error.details,
        }))}),
    }
}

fn logs(root: &Root, logger: &MemoryLogger) -> Value {
    Value::Array(
        logger
            .take()
            .into_iter()
            .map(|(level, message)| json!({"level": level, "message": norm_text(root, &message)}))
            .collect(),
    )
}

/// Текст системной ошибки в полях `message` (после первого «: ») не сверяется.
fn opaque(value: &Value) -> Value {
    match value {
        Value::Array(list) => Value::Array(list.iter().map(opaque).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, item)| {
                    let item = match (key.as_str(), item) {
                        ("message", Value::String(text)) => match text.find(": ") {
                            Some(at) => json!(format!("{}<opaque>", &text[..at + 2])),
                            None => item.clone(),
                        },
                        _ => opaque(item),
                    };
                    (key.clone(), item)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn pure_functions_match_reference() {
    let data = fixture("goldsrc.json");
    let pure = &data["pure"];
    let mut mismatches = Mismatches::default();

    let toggles: Map<String, Value> = GOLDSRC_TOGGLES
        .iter()
        .map(|(key, tokens)| ((*key).to_owned(), json!(tokens)))
        .collect();
    mismatches.check(&Value::Object(toggles), &pure["toggles"], "GOLDSRC_TOGGLES");
    mismatches.check_eq(
        BXT_LOG_FILE,
        pure["bxtLogFile"].as_str().unwrap(),
        "BXT_LOG_FILE",
    );

    for row in items(&pure["isGoldSrc"]) {
        let mut config = hl1().clone();
        config.engine = row["engine"].as_str().map(leak_str);
        mismatches.check_eq(
            is_goldsrc(&config),
            row["result"].as_bool().unwrap(),
            format!("isGoldSrc {}", row["engine"]),
        );
    }

    for row in items(&pure["mergeArguments"]) {
        let mut config = hl1().clone();
        config.arguments = leak_strs(&row["arguments"]);
        let result = merge_arguments(&config, &row["settings"]);
        mismatches.check(
            &json!(result),
            &row["result"],
            format!("mergeArguments {}", row["settings"]),
        );
    }

    for row in items(&pure["environmentOf"]) {
        let mut config = hl1().clone();
        let pairs: Vec<(&'static str, &'static str)> = row["environment"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(key, value)| (leak_str(key), leak_str(value.as_str().unwrap())))
            .collect();
        config.environment = Box::leak(pairs.into_boxed_slice());
        let expected: BTreeMap<String, String> = row["result"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap().to_owned()))
            .collect();
        mismatches.check_eq(
            environment_of(&config),
            expected,
            format!("environmentOf {}", row["environment"]),
        );
    }
    mismatches.finish("goldsrc-launch: чистые функции");
}

#[test]
fn folder_functions_match_reference() {
    let data = fixture("goldsrc.json");
    let files = &data["files"];
    let mut mismatches = Mismatches::default();

    for row in items(&files["resolveFiles"]) {
        let root = Root::new("hlsr-goldsrc-");
        layout(&root, &row["layout"]);
        let config = config_with(&row["config"]);
        let result = attempt(
            &root,
            resolve_files(Path::new(&root.text), &config),
            |files| {
                json!({
                    "executable": path_text(&files.executable),
                    "gameDirectory": path_text(&files.game_directory),
                    "injector": path_text(&files.injector),
                })
            },
        );
        mismatches.check(
            &result,
            &row["result"],
            format!("resolveFiles {}", row["name"]),
        );
    }

    for row in items(&files["enforceSteamAppIdFile"]) {
        let root = Root::new("hlsr-goldsrc-");
        layout(&root, &row["layout"]);
        let config = config_with(&row["config"]);
        let logger = MemoryLogger::default();
        let result = attempt(
            &root,
            goldsrc_launch::enforce_steam_app_id_file(Path::new(&root.text), &config, &logger),
            |file| json!(path_text(&file)),
        );
        let (result, expected) = if row["opaque"] == json!(true) {
            (opaque(&result), opaque(&row["result"]))
        } else {
            (result, row["result"].clone())
        };
        let name = &row["name"];
        mismatches.check(&result, &expected, format!("enforceSteamAppIdFile {name}"));
        mismatches.check(
            &logs(&root, &logger),
            &row["logs"],
            format!("enforce {name}: журнал"),
        );
        mismatches.check(
            &tree(&root),
            &row["tree"],
            format!("enforce {name}: дерево"),
        );
    }

    for row in items(&files["resolveRInput"]) {
        let root = Root::new("hlsr-goldsrc-");
        layout(&root, &row["layout"]);
        let config = config_with(&row["config"]);
        let result = attempt(
            &root,
            resolve_rinput(Path::new(&root.text), &config, &row["settings"]),
            |plan| match plan {
                Some(plan) => json!({
                    "executable": path_text(&plan.executable),
                    "processName": plan.process_name,
                    "missing": plan.missing,
                }),
                None => Value::Null,
            },
        );
        mismatches.check(
            &result,
            &row["result"],
            format!("resolveRInput {}", row["name"]),
        );
    }

    for row in items(&files["prepareBxtLog"]) {
        let root = Root::new("hlsr-goldsrc-");
        layout(&root, &row["layout"]);
        let logger = MemoryLogger::default();
        let dir = row["logsDir"]
            .as_str()
            .map(|dir| Path::new(&root.text).join(dir));
        let result = prepare_bxt_log(dir.as_deref(), &logger);
        let result = json!({"ok": norm(&root, &json!(result.map(|file| path_text(&file))))});
        let mut journal = logs(&root, &logger);
        let mut expected = row["logs"].clone();
        if row["opaque"] == json!(true) {
            journal = opaque(&journal);
            expected = opaque(&expected);
        }
        let name = &row["name"];
        mismatches.check(&result, &row["result"], format!("prepareBxtLog {name}"));
        mismatches.check(&journal, &expected, format!("prepareBxtLog {name}: журнал"));
        mismatches.check(
            &tree(&root),
            &row["tree"],
            format!("prepareBxtLog {name}: дерево"),
        );
    }
    mismatches.finish("goldsrc-launch: файлы");
}

// ---------------------------------------------------------------- самотест hl1-selftest.js

/// «HL1: настройки запуска собираются только в параметры, которые знает WON».
#[test]
fn won_arguments_only_and_player_values_win() {
    let config = hl1();
    let argv = merge_arguments(
        config,
        &json!({
            "windowed": true, "console": true, "developer": true, "condebug": true, "nojoy": true,
            "noaafonts": true, "novid": true, "dx9": true, "high": true, "borderless": true,
            "width": 1280, "height": 960, "refreshRate": 144,
        }),
    );
    assert_eq!(
        argv[config.arguments.len()..],
        [
            "-console",
            "-windowed",
            "-dev",
            "-condebug",
            "-nojoy",
            "-noaafonts",
            "-w",
            "1280",
            "-h",
            "960",
            "-freq",
            "144"
        ],
        "Source-флаги вроде -novid и -dx9 в HL1 не попадают"
    );

    let overridden = merge_arguments(
        config,
        &json!({"width": 1280, "height": 960, "refreshRate": 144, "launchArgs": "-w 800 -h 600 -freq 60"}),
    );
    assert_eq!(
        overridden[config.arguments.len()..],
        ["-w", "800", "-h", "600", "-freq", "60"],
        "-w/-h и -freq игрока главнее настроек"
    );

    let value = launch_args::sanitize_settings(&json!({"width": 1920, "refreshRate": 7})).settings;
    assert!(value.rinput, "RInput по умолчанию включён");
    assert_eq!(value.width, 0, "разрешение без высоты отбрасывается");
    assert_eq!(value.refresh_rate, 0, "частота вне диапазона отбрасывается");
    assert!(
        !launch_args::sanitize_settings(&json!({"rinput": false}))
            .settings
            .rinput
    );
}

/// «HL1: недоступная папка логов не мешает запуску, а только отключает лог BXT».
#[test]
fn broken_logs_folder_only_disables_the_bxt_log() {
    let root = Root::new("hlsr-goldsrc-");
    let blocker = Path::new(&root.text).join("logs");
    write_file(&blocker, "файл вместо папки", None);
    let logger = MemoryLogger::default();
    assert_eq!(prepare_bxt_log(Some(&blocker), &logger), None);
    let lines = logger.take();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].0, "warn");
    assert!(
        lines[0]
            .1
            .starts_with("[launch] лог BXT не подготовлен: EEXIST"),
        "{}",
        lines[0].1
    );
    assert_eq!(prepare_bxt_log(None, &logger), None);
    assert!(logger.take().is_empty());
}

/// Порт [`GoldSrc`] Linux-`LaunchService` — те же функции.
#[test]
fn goldsrc_port_is_the_module() {
    let root = Root::new("hlsr-goldsrc-");
    for (file, text) in [
        ("Half-Life/hl.exe", "MZ"),
        ("Half-Life/valve_WON/liblist.gam", "game \"Half-Life\"\n"),
        ("Bunnymod XT/Injector.exe", "MZ"),
        ("RInput/RInput.exe", "MZ"),
    ] {
        write_file(&Path::new(&root.text).join(file), text, None);
    }
    let install = Path::new(&root.text);
    let port: &dyn GoldSrc = &GoldSrcLaunch;
    let config = hl1();
    assert_eq!(
        port.resolve_files(install, config).unwrap(),
        resolve_files(install, config).unwrap()
    );
    let logger = MemoryLogger::default();
    let file = port
        .enforce_steam_app_id_file(install, config, &logger)
        .unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"70\n");
    assert_eq!(
        port.merge_arguments(config, &json!({"windowed": true})),
        merge_arguments(config, &json!({"windowed": true}))
    );
    let plan = port
        .resolve_rinput(install, config, &Value::Null)
        .unwrap()
        .unwrap();
    assert_eq!(plan.process_name, "hl.exe");
    assert!(!plan.missing);
    assert_eq!(port.environment_of(config), environment_of(config));
    let logs_dir = install.join("logs");
    assert_eq!(
        port.prepare_bxt_log(Some(&logs_dir), &logger),
        Some(logs_dir.join(BXT_LOG_FILE))
    );
}
