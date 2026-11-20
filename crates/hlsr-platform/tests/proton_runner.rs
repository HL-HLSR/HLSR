//! Запуск в префиксе Proton против 0.4.7: фикстура `proton.json`
//! (`tools/reference/platform/proton.js`).
//!
//! Ч — `requiredRuntimeName`, `protonFlag`, `windowsPathOf`, `.reg` шрифтов LiveSplit, CPU без
//! нулевого, цель и аргументы моста presence, `coreFontsStatus` на деревьях префикса, постоянные.
//! С — `ProtonRunner` с поддельным запуском процессов (umu, wineserver, winetricks, regedit
//! «пишут» как в скрипте) и заданным окружением лаунчера: команды, аргументы, папка, `stdio`,
//! окружение (без `LD_*`, `PYTHON*`, `QT_PLUGIN_PATH`, `APPDIR` и его путей), порядок, журнал,
//! дерево префиксов (шрифты, отметки, журнал запуска) при часах на месте.

mod proton_support;
mod support;

use hlsr_core::js::Platform;
use hlsr_core::js::path::NodePath;
use hlsr_delivery::ports::MemoryLogger;
use hlsr_delivery::providers::BoxFuture;
use hlsr_platform::linux::prefix_command::ProtonError;
use hlsr_platform::linux::presence_relay::{self, relay_arguments, target_of};
use hlsr_platform::linux::proton_runner::{
    self, LaunchOptions, ProtonBuild, ProtonRunner, ProtonRunnerOptions, ProtonSelection,
    RunnerError, SteamBridge, allowed_cpu_list_without_zero_in, core_fonts_status,
    livesplit_font_registry, proton_flag, required_runtime_name_of, windows_path_of_with,
};
use hlsr_platform::linux::python_runtime::PythonEnvironment;
use hlsr_platform::linux::user_namespaces::UserNamespacesCheck;
use hlsr_platform::spawn::{Env, PATH_DELIMITER};
use proton_support::{Behaviour, Call, FakeSpawner, Root, env_json, tree_of, write_file};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use support::{Mismatches, fixture, items, str_of};

fn proton() -> Value {
    fixture("proton.json")
}

/// Ресурсы лаунчера в рабочем дереве (шрифты LiveSplit, мост presence).
fn resources() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources")
}

#[test]
fn runner_constants_match_node() {
    let fixture = proton();
    let constants = &fixture["pure"]["constants"];
    let mut m = Mismatches::default();
    m.check(
        &json!(proton_runner::CORE_FONT_FILES),
        &constants["coreFonts"],
        "CORE_FONT_FILES",
    );
    let fonts: Vec<Value> = proton_runner::LIVESPLIT_FONTS
        .iter()
        .map(|font| json!({"source": font.source, "target": font.target, "sha256": font.sha256}))
        .collect();
    m.check(
        &json!(fonts),
        &constants["liveSplitFonts"],
        "LIVESPLIT_FONTS",
    );
    m.check_eq(
        proton_runner::LIVESPLIT_FONT_MARKER,
        str_of(&constants["liveSplitMarker"]),
        "LIVESPLIT_FONT_MARKER",
    );
    m.check_eq(
        proton_runner::PREFIX_PROTON_MARKER,
        str_of(&constants["prefixMarker"]),
        "PREFIX_PROTON_MARKER",
    );
    let runtimes: serde_json::Map<String, Value> = proton_runner::RUNTIME_BY_TOOL_APPID
        .iter()
        .map(|(app, runtime)| ((*app).to_owned(), json!(runtime)))
        .collect();
    m.check(
        &Value::Object(runtimes),
        &constants["runtimes"],
        "RUNTIME_BY_TOOL_APPID",
    );
    m.check(
        &json!({"directory": presence_relay::RESOURCE_DIRECTORY, "executable": presence_relay::EXECUTABLE}),
        &constants["relay"],
        "PresenceRelay",
    );
    m.finish("постоянные запуска в префиксе");
}

#[test]
fn pure_runner_functions_match_node() {
    let fixture = proton();
    let pure = &fixture["pure"];
    let mut m = Mismatches::default();
    for case in items(&pure["runtimes"]) {
        let actual = required_runtime_name_of(case["text"].as_str());
        m.check(
            &json!(actual),
            &case["runtime"],
            format!("requiredRuntimeName {}", case["text"]),
        );
    }
    for case in items(&pure["protonFlags"]) {
        let actual = proton_flag(case["value"].as_str());
        m.check_eq(
            actual,
            case["flag"].as_bool().unwrap(),
            format!("protonFlag {}", case["value"]),
        );
    }
    let posix = NodePath::new(Platform::Linux, "/");
    for case in items(&pure["windowsPaths"]) {
        let file = str_of(&case["file"]);
        m.check_eq(
            windows_path_of_with(&posix, file).as_str(),
            str_of(&case["windows"]),
            format!("windowsPathOf {file:?}"),
        );
    }
    m.check_eq(
        livesplit_font_registry().as_str(),
        str_of(&pure["fontRegistry"]),
        "livesplitFontRegistry",
    );
    for case in items(&pure["cpuLists"]) {
        let actual = allowed_cpu_list_without_zero_in(case["text"].as_str());
        m.check(
            &json!(actual),
            &case["list"],
            format!("allowedCpuListWithoutZero {}", case["text"]),
        );
    }
    for case in items(&pure["presenceTargets"]) {
        let env: Option<Env> = case["env"].as_object().map(|object| {
            object
                .iter()
                .map(|(k, v)| (k.clone(), str_of(v).to_owned()))
                .collect()
        });
        let actual = target_of(env.as_ref()).map_or(Value::Null, |target| target.to_json());
        m.check(
            &actual,
            &case["target"],
            format!("targetOf {}", case["env"]),
        );
    }
    let first: Env = pure["presenceTargets"][0]["env"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), str_of(v).to_owned()))
        .collect();
    let target = target_of(Some(&first)).unwrap();
    m.check(
        &json!(relay_arguments(
            &target,
            "hl2.exe",
            "Z:\\data\\relay.status"
        )),
        &pure["relayArguments"],
        "relayArguments",
    );
    m.finish("чистые функции запуска");
}

#[test]
fn core_fonts_status_matches_node() {
    let fixture = proton();
    let root = Root::new("hlsr-corefonts-");
    let ttf: Vec<u8> = [0, 1, 0, 0].into_iter().chain([0; 20]).collect();
    let all = |dir: &Path, map: &dyn Fn(&str) -> String, content: &dyn Fn(&str) -> Vec<u8>| {
        for font in proton_runner::CORE_FONT_FILES {
            write_file(&dir.join(map(font)), content(font), None);
        }
    };
    let same = |name: &str| name.to_owned();
    let plain = |_: &str| ttf.clone();
    let mut m = Mismatches::default();
    for case in items(&fixture["pure"]["coreFonts"]) {
        let name = str_of(&case["name"]);
        let prefix = root.path.join(name);
        std::fs::create_dir_all(&prefix).unwrap();
        let fonts = prefix.join("drive_c/windows/Fonts");
        match name {
            "fresh" => {}
            "logged-all" => {
                all(&fonts, &same, &plain);
                write_file(
                    &prefix.join("winetricks.log"),
                    "arial\r\n corefonts \n",
                    None,
                );
            }
            "marker-all" => {
                all(&fonts, &same, &plain);
                write_file(&prefix.join(".hlsr-corefonts-ready"), "x", None);
            }
            "unmarked-all" => all(&fonts, &same, &plain),
            "upper-case" => {
                all(&fonts, &|n: &str| n.to_uppercase(), &plain);
                write_file(&prefix.join("winetricks.log"), "corefonts\n", None);
            }
            "headers" => {
                let header = |n: &str| -> Vec<u8> {
                    let with = |head: &[u8], rest: usize| {
                        head.iter()
                            .copied()
                            .chain(std::iter::repeat_n(0, rest))
                            .collect()
                    };
                    match n {
                        "arial.ttf" => with(b"OTTO", 10),
                        "times.ttf" => with(b"true", 10),
                        "comic.ttf" => b"tiny".to_vec(),
                        "cour.ttf" => with(b"wOFF", 10),
                        _ => ttf.clone(),
                    }
                };
                all(&fonts, &same, &header);
                write_file(&prefix.join("winetricks.log"), "corefonts", None);
            }
            "logged-missing" => {
                write_file(&fonts.join("arial.ttf"), &ttf, None);
                write_file(&prefix.join("winetricks.log"), "corefonts\n", None);
            }
            "log-other" => {
                all(&fonts, &same, &plain);
                write_file(&prefix.join("winetricks.log"), "corefonts2\nvcrun\n", None);
            }
            "font-dir" => {
                all(&fonts, &same, &plain);
                std::fs::remove_file(fonts.join("impact.ttf")).unwrap();
                std::fs::create_dir_all(fonts.join("impact.ttf")).unwrap();
                write_file(&prefix.join(".hlsr-corefonts-ready"), "x", None);
            }
            other => panic!("неизвестный случай {other}"),
        }
        m.check(
            &core_fonts_status(&prefix).to_json(),
            &case["status"],
            format!("coreFontsStatus {name}"),
        );
    }
    m.finish("corefonts");
}

// ---------------------------------------------------------------- запуск с поддельными процессами

/// `manager` с заданной сборкой.
pub struct Selected(pub Option<ProtonBuild>);

impl ProtonSelection for Selected {
    fn selected(&self) -> Option<ProtonBuild> {
        self.0.clone()
    }
}

/// `pythonRuntime.prepareEnvironment` скрипта: только `PYTHONNOUSERSITE=1`.
struct NoUserSite;

impl PythonEnvironment for NoUserSite {
    fn prepare_environment<'a>(&'a self, mut env: Env) -> BoxFuture<'a, Result<Env, ProtonError>> {
        env.insert("PYTHONNOUSERSITE".to_owned(), "1".to_owned());
        Box::pin(std::future::ready(Ok(env)))
    }
}

/// `userNamespaces.ensure` без ограничений.
struct Unrestricted;

impl UserNamespacesCheck for Unrestricted {
    fn ensure<'a>(&'a self, _env: &'a Env) -> BoxFuture<'a, Result<(), ProtonError>> {
        Box::pin(std::future::ready(Ok(())))
    }
}

/// `Date.UTC(2026, 9, 16, 12, 0, 0, 123)`.
fn fixed_now() -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(1_792_152_000_123)
}

fn error_json(error: &RunnerError) -> Value {
    json!({"code": error.code(), "message": error.message()})
}

/// `FIXED_ENV(root)` и `FIXED_PATH(root)` скрипта.
fn fixed_env(root: &Root) -> Env {
    let mut env = Env::new();
    for (key, value) in [
        ("HOME", root.at("home")),
        ("XDG_DATA_HOME", root.at("data")),
        ("DISPLAY", ":0".to_owned()),
        ("WAYLAND_DISPLAY", "wayland-0".to_owned()),
        ("LANG", "ru_RU.UTF-8".to_owned()),
        ("APPDIR", root.at("appimage")),
        ("LD_LIBRARY_PATH", root.at("appimage/usr/lib")),
        ("LD_PRELOAD", "libhack.so".to_owned()),
        ("PYTHONPATH", root.at("appimage/usr/lib/python")),
        ("PYTHONHOME", root.at("appimage/usr")),
        ("QT_PLUGIN_PATH", root.at("appimage/usr/plugins")),
        ("STEAM_COMPAT_DATA_PATH", "/old/compat".to_owned()),
        ("STEAM_COMPAT_CLIENT_INSTALL_PATH", "/old/steam".to_owned()),
        ("PROTON_LOG", "1".to_owned()),
        ("HLSR_EXTRA", "keep".to_owned()),
    ] {
        env.insert(key.to_owned(), value);
    }
    let path = [
        root.at("appimage/usr/bin"),
        root.at("bin"),
        root.at("appimage"),
        "/usr/bin".to_owned(),
    ];
    env.insert("PATH".to_owned(), path.join(&PATH_DELIMITER.to_string()));
    env
}

/// Поведение `spawnFn` скрипта: wineserver выходит с кодом из списка; остальные «запускаются»,
/// пишут строку в журнал; winetricks ставит шрифты (или падает `failWinetricks` раз), служебные
/// команды выходят с 0.
fn script_behaviour(wineserver_codes: Vec<i32>, fail_winetricks: usize) -> Behaviour {
    let codes = Arc::new(Mutex::new(wineserver_codes));
    let failures = Arc::new(AtomicUsize::new(fail_winetricks));
    Arc::new(move |_index, call: &Call, controller, log| {
        if call.command.ends_with("wineserver") {
            let code = {
                let mut codes = codes.lock().unwrap();
                if codes.is_empty() { 0 } else { codes.remove(0) }
            };
            controller.exit(Some(code), None);
            return;
        }
        controller.spawned();
        if let Some(mut log) = log {
            std::io::Write::write_all(&mut log, b"err:module:import_dll Library X not found\n")
                .unwrap();
        }
        let first = call.args.first().map(String::as_str);
        if first == Some("winetricks") {
            if failures.load(Ordering::SeqCst) > 0 {
                failures.fetch_sub(1, Ordering::SeqCst);
                controller.stdout(b"Executing w_do_call corefonts\n".to_vec());
                controller.stderr(
                    b"HLSR_PRESENCE_TOKEN=do-not-log-me\nCannot find cabextract\n".to_vec(),
                );
                controller.exit(Some(1), None);
                return;
            }
            let prefix = PathBuf::from(&call.env["WINEPREFIX"]);
            let font: Vec<u8> = [0, 1, 0, 0].into_iter().chain([0; 16]).collect();
            for name in proton_runner::CORE_FONT_FILES {
                write_file(
                    &prefix.join("drive_c/windows/Fonts").join(name),
                    &font,
                    None,
                );
            }
            write_file(&prefix.join("winetricks.log"), "corefonts\n", None);
        }
        if matches!(first, Some("winetricks" | "regedit.exe" | "wineboot.exe")) {
            controller.exit(Some(0), None);
        }
    })
}

async fn runner_scenario(scenario: &Value, m: &mut Mismatches) {
    let name = str_of(&scenario["name"]);
    let options = &scenario["options"];
    let root = Root::new(&format!("hlsr-runner-{name}-"));
    let proton = root.path.join("proton");
    write_file(&proton.join("proton"), "#!/bin/sh\n", Some(0o755));
    write_file(
        &proton.join("protonfixes/winetricks"),
        "#!/bin/sh\n",
        Some(0o755),
    );
    write_file(
        &proton.join("files/bin/wineserver"),
        "#!/bin/sh\n",
        Some(0o755),
    );
    write_file(
        &proton.join("toolmanifest.vdf"),
        "\"manifest\"\n{\n  \"require_tool_appid\" \"4183110\"\n}\n",
        None,
    );
    write_file(&proton.join("version"), "1799999999 test-11.0\n", None);
    write_file(
        &root.path.join("data/umu/steamrt4/_v2-entry-point"),
        "#!/bin/sh\n",
        Some(0o755),
    );
    write_file(&root.path.join("bin/mangohud"), "#!/bin/sh\n", Some(0o755));
    write_file(&root.path.join("game/hl2.exe"), "MZ", None);
    write_file(&root.path.join("hl1/Bunnymod XT/Injector.exe"), "MZ", None);
    write_file(&root.path.join("tools/LiveSplit.exe"), "MZ", None);
    if options["noWinetricks"] == true {
        std::fs::remove_dir_all(proton.join("protonfixes")).unwrap();
    }
    m.check(
        &root.norm(&env_json(&fixed_env(&root))),
        &{
            let mut expected = scenario["env"].clone();
            expected["PATH"] = scenario["path"].clone();
            expected
        },
        format!("{name}: окружение лаунчера"),
    );

    let codes: Vec<i32> = options["wineserverCodes"]
        .as_array()
        .map(|codes| codes.iter().map(|c| c.as_i64().unwrap() as i32).collect())
        .unwrap_or_default();
    let fail = options["failWinetricks"].as_u64().unwrap_or(0) as usize;
    let spawner = FakeSpawner::new(script_behaviour(codes, fail));
    let logger = Arc::new(MemoryLogger::default());
    let selected = (options["noProton"] != true).then(|| ProtonBuild {
        path: proton.to_string_lossy().into_owned(),
        name: "Test Proton".to_owned(),
    });
    let no_umu = options["noUmu"] == true;
    let runner = ProtonRunner::new(ProtonRunnerOptions {
        manager: Some(Arc::new(Selected(selected))),
        data_dir: root.path.clone(),
        resources_path: resources(),
        logger: Some(logger.clone()),
        spawner: Some(spawner.clone()),
        python_runtime: Some(Arc::new(NoUserSite)),
        user_namespaces: Some(Arc::new(Unrestricted)),
        prefix_wineservers: Some(Arc::new(|_: &Path| Vec::new())),
        umu_path: Some(Arc::new(move || {
            (!no_umu).then(|| "/test/umu-run".to_owned())
        })),
        env: Some(fixed_env(&root)),
        now: Some(Arc::new(fixed_now)),
        ..ProtonRunnerOptions::default()
    });
    let at = |path: &str| PathBuf::from(root.at(path));
    for (index, expected) in items(&scenario["steps"]).iter().enumerate() {
        let step = &expected["step"];
        let game_id = step["gameId"].as_str().unwrap_or("");
        let result = match str_of(&step["op"]) {
            "launch" => {
                let given = &step["options"];
                let env: Env = given["env"]
                    .as_object()
                    .map(|env| {
                        env.iter()
                            .map(|(k, v)| (k.clone(), str_of(v).to_owned()))
                            .collect()
                    })
                    .unwrap_or_default();
                let options = LaunchOptions {
                    cwd: given["cwd"].as_str().map(at),
                    env,
                    wrapper: given["wrapper"]
                        .as_array()
                        .map(|w| w.iter().map(|x| str_of(x).to_owned()).collect())
                        .unwrap_or_default(),
                    log_file: given["logFile"].as_str().map(at),
                    steam_bridge: given["steamBridge"].as_object().map(|bridge| SteamBridge {
                        app_id: bridge["appId"].as_i64().unwrap(),
                        steam_path: root.at(str_of(&bridge["steamPath"])),
                        install_path: Some(root.at(str_of(&bridge["installPath"]))),
                    }),
                    skip_fonts: given["skipFonts"] == true,
                    font_profile: given["fontProfile"].as_str().map(str::to_owned),
                    ..LaunchOptions::default()
                };
                let args: Vec<String> = step["args"]
                    .as_array()
                    .map(|a| a.iter().map(|x| str_of(x).to_owned()).collect())
                    .unwrap_or_default();
                match runner
                    .launch(game_id, &at(str_of(&step["executable"])), &args, options)
                    .await
                {
                    Ok(launched) => json!({"ok": {
                        "command": launched.command,
                        "prefix": launched.prefix,
                        "proton": launched.proton.to_json(),
                        "pid": launched.child.pid(),
                    }}),
                    Err(error) => json!({"error": error_json(&error)}),
                }
            }
            "core-fonts" => match runner.ensure_core_fonts(game_id, &Env::new()).await {
                Ok(()) => json!({"ok": null}),
                Err(error) => json!({"error": error_json(&error)}),
            },
            "environment" => match runner.environment(game_id) {
                Ok(launch) => json!({"ok": {
                    "command": launch.command,
                    "prefix": launch.prefix,
                    "env": env_json(&launch.env),
                }}),
                Err(error) => json!({"error": {"code": error.code, "message": error.message}}),
            },
            "write" => {
                write_file(&at(str_of(&step["path"])), str_of(&step["text"]), None);
                json!({"ok": null})
            }
            "prefix-for" => match runner.prefix_for(game_id) {
                Ok(prefix) => json!({"ok": prefix}),
                Err(error) => json!({"error": {"code": error.code, "message": error.message}}),
            },
            other => panic!("неизвестный шаг {other}"),
        };
        // Поддельные процессы отвечают на следующем шаге цикла событий.
        tokio::task::yield_now().await;
        let context = format!("{name}: шаг {index} {step}");
        m.check(
            &root.norm(&result),
            &expected["result"],
            format!("{context}: результат"),
        );
        let calls: Vec<Value> = spawner.take_calls().iter().map(Call::to_json).collect();
        m.check(
            &root.norm(&Value::Array(calls)),
            &expected["calls"],
            format!("{context}: вызовы"),
        );
        let logs: Vec<Value> = logger
            .take()
            .into_iter()
            .map(|(level, message)| json!({"level": level, "message": root.norm_text(&message)}))
            .collect();
        m.check(
            &Value::Array(logs),
            &expected["logs"],
            format!("{context}: журнал"),
        );
    }
    m.check(
        &tree_of(&root.path.join("compatdata"), &[&root.text]),
        &scenario["compatdata"],
        format!("{name}: дерево compatdata"),
    );
    m.check(
        &tree_of(&root.path.join("logs"), &[&root.text]),
        &scenario["logsDir"],
        format!("{name}: папка журналов"),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runner_scenarios_match_node() {
    let fixture = proton();
    let mut m = Mismatches::default();
    for scenario in items(&fixture["runner"]) {
        runner_scenario(scenario, &mut m).await;
    }
    m.finish("запуск в префиксе Proton");
}
