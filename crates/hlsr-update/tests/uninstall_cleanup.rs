//! Очистка после удаления против 0.4.9: фикстура `uninstall-cleanup.json` из
//! `tools/reference/update/uninstall-cleanup.js` (`library-cleanup.js`, `cleanup-cli.js`): разбор
//! флагов, запретные зоны, проверки корня библиотеки на настоящих деревьях (маркер, junction,
//! канонический путь, маркер-ссылка), запущенные игры, очистка библиотеки (с `purge` — только
//! своё, чужие установки HLSR остаются), настроек и профиля, режим CLI со строками, stdout и
//! отчётом байт в байт. Окружение, домашний каталог и библиотеки Steam заданы
//! явно — настоящие профиль, Steam и `C:\HLSR` не трогаются.

#![cfg(windows)]

mod support;

use hlsr_core::json_text::{parse_js_json, stringify};
use hlsr_update::errors::cleanup_exit;
use hlsr_update::uninstall::cleanup_cli::{self, RunOptions};
use hlsr_update::uninstall::library_cleanup::{
    self as cleanup, CleanupOptions, StaticSteamLibraries,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use support::tree::snapshot;
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("uninstall-cleanup.json"));

fn pid() -> String {
    std::process::id().to_string()
}

/// Числа как у `JSON.parse` (`1.0` → `1`).
fn js_normal(value: &Value) -> Value {
    parse_js_json(&stringify(value, "")).unwrap()
}

struct Norm {
    root: String,
}

impl Norm {
    fn denorm(&self, text: &str) -> String {
        let escaped = serde_json::to_string(&self.root).unwrap();
        text.replace("{root:json}", &escaped[1..escaped.len() - 1])
            .replace("{root}", &self.root)
            .replace("{pid}", &pid())
    }

    fn denorm_deep(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.denorm(text)),
            Value::Array(list) => {
                Value::Array(list.iter().map(|item| self.denorm_deep(item)).collect())
            }
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, item)| (key.clone(), self.denorm_deep(item)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    fn text(&self, value: &str) -> String {
        let escaped = serde_json::to_string(&self.root).unwrap();
        value
            .replace(&escaped[1..escaped.len() - 1], "{root}")
            .replace(&self.root, "{root}")
            .replace(&format!("PID {}", pid()), "PID {pid}")
    }

    fn deep(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.text(text)),
            Value::Number(number) if number.as_u64() == Some(u64::from(std::process::id())) => {
                json!("{pid}")
            }
            Value::Array(list) => Value::Array(list.iter().map(|item| self.deep(item)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, item)| (key.clone(), self.deep(item)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    fn transform(&self, bytes: Vec<u8>) -> Vec<u8> {
        match String::from_utf8(bytes) {
            Ok(text) => self.text(&text).replace(&pid(), "{pid}").into_bytes(),
            Err(error) => error.into_bytes(),
        }
    }

    /// Снимок дерева; цель junction — без `\\?\`, как у `fs.readlinkSync`.
    fn tree(&self, root: &Path) -> Value {
        let transform = |_rel: &str, bytes: Vec<u8>| self.transform(bytes);
        let mut snap = snapshot(root, Some(&transform));
        for entry in snap.as_array_mut().unwrap() {
            if let Some(target) = entry.get("target").and_then(Value::as_str) {
                let plain = target.strip_prefix(r"\\?\").unwrap_or(target).to_owned();
                entry["target"] = json!(plain);
            }
        }
        self.deep(&snap)
    }
}

fn temp_root() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("hlsr-update-cleanup-")
        .tempdir()
        .unwrap();
    let real = std::fs::canonicalize(dir.path()).unwrap();
    let text = real.to_string_lossy().into_owned();
    let plain = text
        .strip_prefix(r"\\?\")
        .map_or(text.clone(), str::to_owned);
    (dir, PathBuf::from(plain))
}

fn join_rel(root: &Path, rel: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in rel.split('/') {
        path.push(part);
    }
    path
}

fn hex_bytes(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
        .collect()
}

fn setup_tree(root: &Path, items_: &Value, norm: &Norm) {
    for item in items_.as_array().into_iter().flatten() {
        if let Some(dir) = item["dir"].as_str() {
            std::fs::create_dir_all(join_rel(root, dir)).unwrap();
        } else if let Some(link) = item["junction"].as_str() {
            let link = join_rel(root, link);
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            hlsr_update::windows::file_ops::create_junction(
                &join_rel(root, str_of(&item["target"])),
                &link,
            )
            .unwrap();
        } else {
            let file = join_rel(root, str_of(&item["file"]));
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            let bytes = match item["hex"].as_str() {
                Some(hex) => hex_bytes(hex),
                None => norm.denorm(str_of(&item["text"])).into_bytes(),
            };
            std::fs::write(file, bytes).unwrap();
        }
    }
}

/// Окружение `options.env` (объект JS: имена с учётом регистра).
struct Options {
    env: Option<HashMap<String, String>>,
    raw: Value,
    steam: Option<StaticSteamLibraries>,
}

impl Options {
    fn new(raw: &Value) -> Options {
        let env = raw["env"].as_object().map(|map| {
            map.iter()
                .map(|(name, value)| (name.clone(), str_of(value).to_owned()))
                .collect()
        });
        let steam = raw["steamLibraries"].as_array().map(|list| {
            StaticSteamLibraries(list.iter().map(|item| str_of(item).to_owned()).collect())
        });
        Options {
            env,
            raw: raw.clone(),
            steam,
        }
    }

    fn with<T>(&self, run: impl FnOnce(CleanupOptions<'_>) -> T) -> T {
        let lookup = |name: &str| self.env.as_ref().and_then(|env| env.get(name).cloned());
        let text = |name: &str| self.raw[name].as_str().map(str::to_owned);
        let options = CleanupOptions {
            env: self
                .env
                .as_ref()
                .map(|_| &lookup as &dyn Fn(&str) -> Option<String>),
            home: text("home"),
            extra_zones: self.raw["extraZones"]
                .as_array()
                .map(|list| list.iter().map(|item| str_of(item).to_owned()).collect())
                .unwrap_or_default(),
            steam: self.steam.as_ref().map(|steam| steam as _),
            install_dir: text("installDir"),
            purge: self.raw["purge"] == json!(true),
            user_data: text("userData"),
            logs_dir: text("logsDir"),
            updater_cache: text("updaterCache"),
            app_name: text("appName"),
            preserve_profile: self.raw["preserveProfile"] == json!(true),
            is_alive: None,
            own_marker: text("ownMarker"),
        };
        run(options)
    }
}

#[test]
fn codes_and_constants_match_node() {
    let codes = &FIXTURE["codes"];
    for (name, value) in [
        ("OK", cleanup_exit::OK),
        ("USAGE", cleanup_exit::USAGE),
        ("NOTHING_TO_DO", cleanup_exit::NOTHING_TO_DO),
        ("REFUSED_PATH", cleanup_exit::REFUSED_PATH),
        ("REFUSED_MARKER", cleanup_exit::REFUSED_MARKER),
        ("REFUSED_REPARSE", cleanup_exit::REFUSED_REPARSE),
        ("REFUSED_RUNNING", cleanup_exit::REFUSED_RUNNING),
        ("PARTIAL", cleanup_exit::PARTIAL),
        ("INTERNAL", cleanup_exit::INTERNAL),
    ] {
        assert_eq!(codes[name], json!(value), "{name}");
    }
    let constants = &FIXTURE["constants"];
    assert_eq!(
        constants["MARKER_RELATIVE"],
        json!(cleanup::marker_relative())
    );
    assert_eq!(constants["OWNED_ZONES"], json!(cleanup::OWNED_ZONES));
    assert_eq!(constants["PROFILE_FILES"], json!(cleanup::PROFILE_FILES));
    assert_eq!(constants["FLAG"], json!(cleanup_cli::FLAG));
}

#[test]
fn parse_args_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["parsed"]) {
        let argv: Vec<String> = items(&case["argv"])
            .iter()
            .map(|a| str_of(a).to_owned())
            .collect();
        mismatches.check(
            &cleanup_cli::parse_args(&argv).to_value(),
            &case["args"],
            format!("{argv:?}"),
        );
    }
    for case in items(&FIXTURE["invocations"]) {
        let argv: Vec<String> = items(&case["argv"])
            .iter()
            .map(|a| str_of(a).to_owned())
            .collect();
        mismatches.check_eq(
            cleanup_cli::is_cleanup_invocation(&argv),
            case["cleanup"].as_bool().unwrap(),
            format!("{argv:?}"),
        );
    }
    mismatches.finish("parseArgs");
}

#[test]
fn forbidden_zones_and_settings_targets_match_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["zones"]) {
        let options = Options::new(&case["options"]);
        let zones = options.with(|options| cleanup::forbidden_zones(&options));
        mismatches.check(&json!(zones), &case["zones"], &case["options"]);
    }
    for case in items(&FIXTURE["settingsTargets"]) {
        let options = Options::new(&case["options"]);
        let targets = options.with(|options| cleanup::settings_targets(&options));
        mismatches.check(&json!(targets), &case["targets"], &case["options"]);
    }
    mismatches.finish("forbiddenZones, settingsTargets");
}

fn run_scenario(case: &Value) -> Value {
    let (_guard, root) = temp_root();
    let norm = Norm {
        root: root.to_string_lossy().into_owned(),
    };
    setup_tree(&root, &case["setup"], &norm);
    let input = &case["input"];
    let options = Options::new(&norm.denorm_deep(&input["options"]));
    let raw = input["raw"].as_str().map(|raw| norm.denorm(raw));
    let mut stdout = None;
    let mut report = Value::Null;
    let result = match str_of(&case["op"]) {
        "check" => {
            options.with(|o| cleanup::check_library_root(raw.as_deref().unwrap(), &o).to_value())
        }
        "running" => {
            let games =
                cleanup::running_games(&norm.denorm(str_of(&input["root"])), &cleanup::is_alive);
            Value::Array(
                games
                    .iter()
                    .map(|game| json!({"gameId": game.game_id, "pid": game.pid}))
                    .collect(),
            )
        }
        "cleanupLibrary" => {
            options.with(|o| cleanup::cleanup_library(raw.as_deref().unwrap(), &o).to_value())
        }
        "cleanupSettings" => options.with(|o| cleanup::cleanup_settings(&o).to_value()),
        "cleanupProfile" => options.with(|o| cleanup::cleanup_profile(&o).to_value()),
        "libraryPathFrom" => {
            json!(cleanup_cli::library_path_from(Path::new(
                &norm.denorm(str_of(&input["userData"]))
            )))
        }
        "run" => {
            let argv: Vec<String> = items(&input["argv"])
                .iter()
                .map(|arg| norm.denorm(str_of(arg)))
                .collect();
            let text = |name: &str| options.raw[name].as_str().map(str::to_owned);
            let lookup = |name: &str| options.env.as_ref().and_then(|env| env.get(name).cloned());
            let mut out = Vec::new();
            let result = cleanup_cli::run(
                &RunOptions {
                    argv: argv.clone(),
                    silent: options.raw["silent"] == json!(true),
                    user_data: text("userData"),
                    app_name: text("appName"),
                    launcher_version: None,
                    logs_dir: text("logsDir"),
                    updater_cache: text("updaterCache"),
                    install_dir: text("installDir"),
                    env: options
                        .env
                        .as_ref()
                        .map(|_| &lookup as &dyn Fn(&str) -> Option<String>),
                    home: text("home"),
                    steam: options.steam.as_ref().map(|steam| steam as _),
                    telemetry: None,
                    is_alive: None,
                },
                &mut out,
            );
            stdout = Some(String::from_utf8(out).unwrap());
            let file = root.join("report.txt");
            if let Ok(bytes) = std::fs::read(&file) {
                report = if argv.iter().any(|arg| arg == "--print-library") {
                    json!({"hex": bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()})
                } else {
                    json!({"text": String::from_utf8(bytes).unwrap()})
                };
            }
            json!({"code": result.code, "lines": result.lines})
        }
        other => panic!("операция {other}"),
    };
    let mut out = Map::new();
    out.insert("result".to_owned(), js_normal(&norm.deep(&result)));
    out.insert("stdout".to_owned(), norm.deep(&json!(stdout)));
    out.insert("report".to_owned(), norm.deep(&report));
    out.insert("tree".to_owned(), norm.tree(&root));
    Value::Object(out)
}

#[test]
fn cleanup_scenarios_match_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["scenarios"]) {
        let actual = run_scenario(case);
        let mut expected = Map::new();
        for key in ["result", "stdout", "report", "tree"] {
            expected.insert(key.to_owned(), case[key].clone());
        }
        mismatches.check(&actual, &Value::Object(expected), str_of(&case["label"]));
    }
    mismatches.finish("очистка после удаления");
}

/// Телеметрия удаления: строки порта идут в отчёт сразу после заголовка, только при `userData`.
#[test]
fn telemetry_lines_follow_the_header() {
    struct Telemetry;
    impl cleanup_cli::UninstallTelemetry for Telemetry {
        fn report(&self, user_data: &str, version: &str, say: &mut dyn FnMut(&str)) {
            say(&format!("telemetry: {user_data} {version}"));
        }
    }
    let run = |user_data: Option<&str>| {
        cleanup_cli::run(
            &RunOptions {
                argv: vec!["HLSR.exe".to_owned()],
                silent: true,
                user_data: user_data.map(str::to_owned),
                launcher_version: Some("0.5.0".to_owned()),
                telemetry: Some(&Telemetry),
                ..RunOptions::default()
            },
            &mut std::io::sink(),
        )
    };
    let with = run(Some("C:\\U"));
    assert_eq!(with.lines[1], "telemetry: C:\\U 0.5.0");
    assert_eq!(with.code, cleanup_exit::NOTHING_TO_DO);
    assert_eq!(run(None).lines.len(), 1);
}
