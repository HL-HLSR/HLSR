//! Ядро внешнего апдейтера против 0.4.7: фикстура `updater-core.json` из
//! `tools/reference/update/updater-core.js` (модули `updater/core/*`, помощники `updater/main.js` и
//! `launcher-update/updater-launch.js` в Node 24 эталона). Чистые функции на корпусе (план и
//! `D:\Games\Launcher`, пути, аргументы установщика, версия, разбор `tasklist`, ожидание выхода,
//! блокировка, журнал, скорость), загрузка с докачкой против локальных серверов 127.0.0.1 и трассы
//! сессии (стадии, события, вызовы, журнал) на заглушках.
//!
//! Тексты сетевых ошибок Node и reqwest разные: в сверке загрузки их заменяет метка `<network>`.

mod support;

use hlsr_core::json_text::{js_number, stringify};
use hlsr_update::cancel::CancelToken;
use hlsr_update::channel::VerifyOutcome;
use hlsr_update::chunks::{self, Assembled, ChunkError, Delta, Manifest};
use hlsr_update::errors::{LuCode, UpdCode, UpdaterError};
use hlsr_update::ports::{BoxFuture, Clock, FakeClock, Logger};
use hlsr_update::updater::download::{
    self, DownloadOptions, DownloadProgress, FixedFreeSpace, SpeedMeter,
};
use hlsr_update::updater::install::{self, InstallOptions, Installed};
use hlsr_update::updater::launch::{self, LaunchOptions};
use hlsr_update::updater::log::UpdaterLog;
use hlsr_update::updater::mode;
use hlsr_update::updater::paths;
use hlsr_update::updater::plan::{self, ValidateOptions};
use hlsr_update::updater::process_wait::{self, ProcessList, WaitInfo, WaitOptions};
use hlsr_update::updater::processes::{FakeLauncher, LaunchCall, RunExit};
use hlsr_update::updater::session::{
    AssembleRequest, DownloadRequest, SessionDeps, SessionEvent, UpdateSession,
};
use hlsr_update::updater::single_instance;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock, Weak};
use std::time::Duration;
use support::http::{self, RequestLog, Server};
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("updater-core.json"));

fn error_json(error: &UpdaterError) -> Value {
    let mut out = json!({"code": error.code.as_str(), "message": error.message});
    if let Some(details) = &error.details {
        out["details"] = details.clone();
    }
    out
}

fn opt_str(value: &Value) -> Option<&str> {
    value.as_str()
}

/// Строки фикстуры — списком.
fn strings(value: &Value) -> Vec<String> {
    items(value)
        .iter()
        .map(|item| str_of(item).to_owned())
        .collect()
}

// ------------------------------------------------------------ коды

#[test]
fn codes_match_node() {
    let codes = &FIXTURE["errors"]["codes"];
    assert_eq!(codes.as_object().unwrap().len(), UpdCode::ALL.len());
    for code in UpdCode::ALL {
        assert_eq!(codes[code.name()], json!(code.as_str()), "{}", code.name());
    }
    let public = UpdaterError::new(UpdCode::BadPlan, "й".repeat(500)).to_public();
    assert_eq!(
        public["message"].as_str().unwrap().chars().count() as u64,
        FIXTURE["errors"]["publicMax"].as_u64().unwrap()
    );
}

// ------------------------------------------------------------ план

#[test]
fn plan_validation_matches_node() {
    let key = str_of(&FIXTURE["plan"]["keys"]["test"]).to_owned();
    let cases = items(&FIXTURE["plan"]["cases"]);
    assert!(cases.len() >= 80, "в фикстуре {} случаев", cases.len());
    let mut mismatches = Mismatches::default();
    for case in cases {
        let options = &case["options"];
        let validate = ValidateOptions {
            public_keys: (options["production"] != json!(true)).then(|| vec![key.clone()]),
            download_base: options["downloadBase"].as_str().map(str::to_owned),
        };
        let actual = match plan::validate(&case["raw"], &validate) {
            Ok(plan) => json!({"ok": plan.to_json()}),
            Err(error) => json!({"error": error_json(&error)}),
        };
        let mut expected = Map::new();
        for key in ["ok", "error"] {
            if let Some(value) = case.get(key) {
                expected.insert(key.into(), value.clone());
            }
        }
        mismatches.check(&actual, &Value::Object(expected), str_of(&case["id"]));
    }
    mismatches.finish("plan.validate");

    for case in items(&FIXTURE["plan"]["plausible"]) {
        assert_eq!(
            plan::is_plausible_launcher_dir(&case["dir"]),
            case["ok"].as_bool().unwrap(),
            "{}",
            case["dir"]
        );
    }
}

/// Приёмка 0.4.7 (карта §0.5): каталог установки без сегмента `HLSR*` — `UPD_BAD_PLAN`.
#[test]
fn install_dir_without_hlsr_segment_is_a_bad_plan() {
    let case = items(&FIXTURE["plan"]["cases"])
        .iter()
        .find(|case| str_of(&case["id"]) == "installDir D:\\Games\\Launcher")
        .unwrap();
    assert_eq!(case["error"]["code"], json!("UPD_BAD_PLAN"));
    assert!(!plan::is_plausible_launcher_dir(&json!(
        "D:\\Games\\Launcher"
    )));
    assert!(plan::is_plausible_launcher_dir(&json!("D:\\hlsr")));
}

/// Отступление: каталог без сегмента `HLSR*` (пробел и кириллица в имени), но с маркером владельца
/// установки HLSR — годный план; без маркера — по-прежнему `UPD_BAD_PLAN`.
#[test]
fn install_dir_with_the_owner_marker_is_a_good_plan() {
    let case = items(&FIXTURE["plan"]["cases"])
        .iter()
        .find(|case| str_of(&case["id"]) == "installDir D:\\Games\\Launcher")
        .unwrap();
    let dir = tempfile::Builder::new()
        .prefix("Мои игры ")
        .tempdir()
        .unwrap();
    let install_dir = dir.path().to_string_lossy().into_owned();
    let mut raw = case["raw"].clone();
    raw["installDir"] = json!(install_dir);
    raw["launcherExe"] = json!(format!("{install_dir}\\HLSR.exe"));
    let options = ValidateOptions {
        public_keys: Some(vec![str_of(&FIXTURE["plan"]["keys"]["test"]).to_owned()]),
        download_base: None,
    };
    // С сегментом `HLSR*` во временном пути план прошёл бы и без маркера — отказ не проверить.
    assert!(
        !plan::is_plausible_launcher_dir(&json!(install_dir)),
        "во временном пути есть сегмент HLSR*: {install_dir}"
    );
    let refused = plan::validate(&raw, &options).unwrap_err();
    assert_eq!(refused.code, UpdCode::BadPlan);
    std::fs::write(dir.path().join(".hlsr-install-owner.json"), "{}").unwrap();
    assert!(plan::is_owned_launcher_dir(&json!(install_dir)));
    assert!(!plan::is_owned_launcher_dir(&json!("\\\\server\\share")));
    let plan = plan::validate(&raw, &options).unwrap();
    assert_eq!(plan.install_dir, install_dir);
}

// ------------------------------------------------------------ пути

#[test]
fn writable_paths_match_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["paths"]["writable"]) {
        let roots: Vec<&str> = items(&case["allowedRoots"])
            .iter()
            .map(|root| root.as_str().unwrap_or(""))
            .collect();
        let files: Vec<&str> = items(&case["allowedFiles"])
            .iter()
            .map(|file| file.as_str().unwrap_or(""))
            .collect();
        let actual = match paths::assert_writable(str_of(&case["target"]), &roots, &files) {
            Ok(()) => json!({"ok": true}),
            Err(error) => json!({"error": error_json(&error)}),
        };
        let expected = if case.get("ok").is_some() {
            json!({"ok": case["ok"]})
        } else {
            json!({"error": case["error"]})
        };
        mismatches.check(&actual, &expected, str_of(&case["target"]));
    }
    for case in items(&FIXTURE["paths"]["safeWork"]) {
        let actual = match paths::assert_safe_work_dir(
            str_of(&case["workDir"]),
            str_of(&case["tempRoot"]),
            opt_str(&case["installDir"]),
        ) {
            Ok(()) => json!({"ok": true}),
            Err(error) => json!({"error": error_json(&error)}),
        };
        let expected = if case.get("ok").is_some() {
            json!({"ok": case["ok"]})
        } else {
            json!({"error": case["error"]})
        };
        mismatches.check(&actual, &expected, str_of(&case["workDir"]));
    }
    mismatches.finish("paths");
}

// ------------------------------------------------------------ установщик

#[test]
fn installer_arguments_match_node() {
    let install = &FIXTURE["install"];
    let mut mismatches = Mismatches::default();
    for case in items(&install["args"]) {
        let dir = match &case["dir"] {
            Value::String(text) if text == "<undefined>" => None,
            Value::String(text) => Some(text.as_str()),
            _ => None,
        };
        mismatches.check(
            &json!(install::installer_args_for(dir)),
            &case["args"],
            format!("{:?}", case["dir"]),
        );
    }
    let tree = install::tree_setup_options(
        Path::new("C:\\Temp\\U\\work\\tree"),
        "C:\\Program Files\\HLSR",
        "0.5.0",
        vec![("Path".to_owned(), "C:\\Windows".to_owned())],
    );
    let portable = tree
        .env
        .iter()
        .find(|(name, _)| name == "PORTABLE_EXECUTABLE_FILE")
        .map(|(_, value)| value.replace('/', "\\"));
    mismatches.check(
        &json!({"args": tree.args, "PORTABLE_EXECUTABLE_FILE": portable, "windowsHide": tree.hidden}),
        &install["treeOptions"],
        "treeSetupOptions",
    );
    for case in items(&install["triples"]) {
        let value = match &case["value"] {
            Value::String(text) if text == "<undefined>" => String::new(),
            Value::Null => String::new(),
            other => hlsr_core::js::to_string(other),
        };
        mismatches.check(
            &json!(install::version_triple(&value)),
            &case["triple"],
            format!("versionTriple {:?}", case["value"]),
        );
    }
    for case in items(&install["productVersions"]) {
        mismatches.check(
            &json!(install::product_version_from_text(str_of(&case["stdout"]))),
            &case["version"],
            format!("ProductVersion {:?}", case["stdout"]),
        );
    }
    assert_eq!(
        json!(install::INSTALLER_ARGS),
        install["constants"]["INSTALLER_ARGS"]
    );
    assert_eq!(
        install::DEFAULT_INSTALL_TIMEOUT.as_millis() as u64,
        install["constants"]["DEFAULT_INSTALL_TIMEOUT_MS"]
            .as_u64()
            .unwrap()
    );
    mismatches.finish("install");
}

/// Отступление: полный установщик 0.5+ получает `/D=` и с пробелом и кириллицей в пути.
#[test]
fn full_installer_gets_the_dir_even_with_a_space() {
    let options = InstallOptions::installer(" D:\\HLSR Тест\\Launcher ", Vec::new());
    assert_eq!(
        options.args,
        ["/S", "--updated", "/D=D:\\HLSR Тест\\Launcher"]
    );
    assert!(!options.hidden);
    assert_eq!(
        InstallOptions::installer("", Vec::new()).args,
        ["/S", "--updated"]
    );
}

#[test]
fn verify_installed_matches_node() {
    let dir = tempfile::tempdir().unwrap();
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["install"]["verifyInstalled"]) {
        let exe = dir.path().join("HLSR.exe");
        let _ = std::fs::remove_file(&exe);
        if case["exists"].as_bool().unwrap() {
            std::fs::write(&exe, "exe").unwrap();
        }
        let actual = case["actual"].as_str().map(str::to_owned);
        let expected = match case["expected"].as_str() {
            Some("<undefined>") | None => None,
            Some(text) => Some(text),
        };
        let result: Installed =
            install::verify_installed(&exe, expected, &|_: &Path| actual.clone());
        mismatches.check(&result.to_json(), &case["result"], format!("{case}"));
    }
    mismatches.finish("verifyInstalled");
}

#[tokio::test]
async fn run_installer_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["install"]["runInstaller"]) {
        let spec = case.clone();
        // Сценарий 0.4.7: `throws` — spawn бросает, без `exit` — событие `error`, `after` дольше
        // `timeoutMs` — тайм-аут, иначе код выхода (`null` — сигнал).
        let launcher = FakeLauncher::new().on_run(move |_| {
            if let Some(message) = spec["throws"].as_str() {
                return Err(message.to_owned());
            }
            let Some(exit) = spec.get("exit") else {
                return Err(str_of(&spec["error"]["message"]).to_owned());
            };
            if let (Some(limit), Some(after)) = (spec["timeoutMs"].as_u64(), spec["after"].as_u64())
                && after > limit
            {
                return Ok(RunExit::TimedOut);
            }
            Ok(RunExit::Exited(exit.as_i64().map(|code| code as i32)))
        });
        let mut options = if case["tree"] == json!(true) {
            install::tree_setup_options(
                Path::new("C:\\T\\tree"),
                "C:\\HLSR\\Launcher",
                "0.5.0",
                Vec::new(),
            )
        } else {
            InstallOptions {
                args: install::installer_args_for(case["installDir"].as_str()),
                env: Vec::new(),
                hidden: false,
                timeout: install::DEFAULT_INSTALL_TIMEOUT,
            }
        };
        if let Some(ms) = case["timeoutMs"].as_u64() {
            options.timeout = Duration::from_millis(ms);
        }
        let result = install::run_installer(
            &launcher,
            Path::new("C:\\T\\HLSR-Setup-0.5.0.exe"),
            &options,
        )
        .await;
        let calls: Vec<Value> = launcher
            .calls()
            .iter()
            .map(|call| {
                let LaunchCall::Run(command, _) = call else {
                    panic!("ожидался запуск с ожиданием");
                };
                json!({
                    "file": command.file.to_string_lossy(),
                    "args": command.args,
                    "options": {
                        "windowsHide": command.hidden,
                        "detached": command.detached,
                        "stdio": "ignore",
                        "PORTABLE_EXECUTABLE_FILE": command.env_var("PORTABLE_EXECUTABLE_FILE")
                            .map(|value| value.replace('/', "\\")),
                    },
                })
            })
            .collect();
        let mut actual = json!({"calls": calls});
        let mut expected = json!({"calls": case["calls"]});
        match result {
            Ok(()) => actual["result"] = json!({"ok": true, "exitCode": 0}),
            Err(error) => actual["error"] = error_json(&error),
        }
        for key in ["result", "error"] {
            if case.get(key).is_some() && case[key].is_object() {
                expected[key] = case[key].clone();
            }
        }
        mismatches.check(&actual, &expected, str_of(&case["id"]));
    }
    mismatches.finish("runInstaller");
}

#[test]
fn relaunch_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["install"]["relaunch"]) {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("HLSR.exe");
        if case["exists"].as_bool().unwrap() {
            std::fs::write(&exe, "exe").unwrap();
        }
        let throws = case["throws"].as_str().map(str::to_owned);
        let launcher = FakeLauncher::new().on_spawn(move |_| match &throws {
            Some(message) => Err(message.clone()),
            None => Ok(77),
        });
        let result = install::relaunch(&launcher, &exe, Vec::new());
        let calls: Vec<Value> = launcher
            .calls()
            .iter()
            .map(|call| {
                let command = call.command();
                json!({
                    "file": command.file.file_name().unwrap().to_string_lossy(),
                    "args": command.args,
                    "cwd": if command.cwd.as_deref() == Some(dir.path()) { json!("{dir}") } else { json!(null) },
                    "detached": command.detached,
                    "stdio": "ignore",
                    "windowsHide": command.hidden,
                })
            })
            .collect();
        let mut actual = json!({"calls": calls});
        match result {
            Ok(pid) => actual["result"] = json!({"ok": true, "pid": pid}),
            Err(error) => actual["error"] = error_json(&error),
        }
        let mut expected = json!({"calls": case["calls"]});
        for key in ["result", "error"] {
            if let Some(value) = case.get(key) {
                expected[key] = value.clone();
            }
        }
        mismatches.check(
            &actual,
            &expected,
            format!("exists={} throws={}", case["exists"], case["throws"]),
        );
    }
    mismatches.finish("relaunch");
}

// ------------------------------------------------------------ ожидание выхода

#[test]
fn tasklist_names_match_node() {
    for case in items(&FIXTURE["processWait"]["tasklist"]) {
        let stdout = case["stdout"].as_str().unwrap_or("");
        let pids = if case["error"] == json!(true) {
            Vec::new()
        } else {
            process_wait::list_pids_from_tasklist_csv(stdout, str_of(&case["image"]))
        };
        assert_eq!(json!(pids), case["pids"], "{case}");
    }
}

/// Процессы по сценарию: списки по очереди, последний повторяется.
struct ScriptedList {
    lists: Vec<Vec<u32>>,
    index: AtomicUsize,
    listed: Mutex<Vec<String>>,
}

impl ProcessList for ScriptedList {
    fn list_pids(&self, image_name: &str) -> Vec<u32> {
        self.listed.lock().unwrap().push(image_name.to_owned());
        let index = self.index.fetch_add(1, Ordering::SeqCst);
        self.lists[index.min(self.lists.len() - 1)].clone()
    }
}

#[tokio::test]
async fn wait_for_exit_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["processWait"]["waits"]) {
        let list = ScriptedList {
            lists: items(&case["lists"])
                .iter()
                .map(|pids| {
                    items(pids)
                        .iter()
                        .map(|pid| pid.as_u64().unwrap() as u32)
                        .collect()
                })
                .collect(),
            index: AtomicUsize::new(0),
            listed: Mutex::new(Vec::new()),
        };
        let clock = Arc::new(FakeClock::new(1000));
        let slack = case["slack"].as_i64().unwrap_or(0);
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let sleep = {
            let (clock, sleeps) = (clock.clone(), sleeps.clone());
            move |duration: Duration| -> BoxFuture<'static, ()> {
                sleeps.lock().unwrap().push(duration.as_millis() as u64);
                clock.advance(duration.as_millis() as i64 + slack);
                Box::pin(async {})
            }
        };
        let waits = Mutex::new(Vec::new());
        let on_wait = |info: &WaitInfo| waits.lock().unwrap().push(info.to_json());
        let result = process_wait::wait_for_exit(WaitOptions {
            image_name: case["imageName"]
                .as_str()
                .unwrap_or(process_wait::LAUNCHER_IMAGE),
            timeout: case["timeoutMs"]
                .as_u64()
                .map_or(process_wait::DEFAULT_TIMEOUT, Duration::from_millis),
            poll: case["pollMs"]
                .as_u64()
                .map_or(process_wait::POLL_INTERVAL, Duration::from_millis),
            list: &list,
            clock: clock.as_ref(),
            sleep: &sleep,
            on_wait: &on_wait,
        })
        .await;
        let mut actual = json!({
            "waits": waits.lock().unwrap().clone(),
            "sleeps": sleeps.lock().unwrap().clone(),
            "listed": list.listed.lock().unwrap().clone(),
        });
        match result {
            Ok(()) => actual["result"] = json!({"ok": true, "waitedFor": []}),
            Err(error) => actual["error"] = error_json(&error),
        }
        let mut expected =
            json!({"waits": case["waits"], "sleeps": case["sleeps"], "listed": case["listed"]});
        for key in ["result", "error"] {
            if let Some(value) = case.get(key) {
                expected[key] = value.clone();
            }
        }
        mismatches.check(&actual, &expected, str_of(&case["id"]));
    }
    mismatches.finish("waitForExit");
    assert_eq!(
        process_wait::DEFAULT_TIMEOUT.as_millis() as u64,
        FIXTURE["processWait"]["defaultTimeoutMs"].as_u64().unwrap()
    );
}

// ------------------------------------------------------------ блокировка

#[test]
fn single_instance_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["lock"]) {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("HLSR");
        if let Some(existing) = case["existing"].as_str() {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(single_instance::LOCK_NAME), existing).unwrap();
        }
        let alive = case["alive"].as_bool().unwrap_or(false);
        let asked = Mutex::new(Vec::new());
        let is_alive = |pid: f64| {
            asked.lock().unwrap().push(js_number(pid));
            alive
        };
        let mut actual = json!({});
        match single_instance::acquire(&dir, 100, 1_760_000_000_123, &is_alive) {
            Ok(lock) => {
                actual["acquired"] = json!(std::fs::read_to_string(&lock.file).unwrap());
                if let Some(replace) = case["replace"].as_str() {
                    std::fs::write(&lock.file, replace).unwrap();
                }
                lock.release();
                actual["afterRelease"] = json!(std::fs::read_to_string(&lock.file).ok());
            }
            Err(error) => {
                actual["error"] = error_json(&error);
                actual["left"] =
                    json!(std::fs::read_to_string(dir.join(single_instance::LOCK_NAME)).unwrap());
            }
        }
        actual["asked"] = json!(asked.lock().unwrap().clone());
        let mut expected = json!({"asked": case["asked"]});
        for key in ["acquired", "afterRelease", "error", "left"] {
            if let Some(value) = case.get(key) {
                expected[key] = value.clone();
            }
        }
        mismatches.check(&actual, &expected, str_of(&case["id"]));
    }
    mismatches.finish("single-instance");
}

// ------------------------------------------------------------ журнал

/// Часы, которые идут на 1 мс при каждом вопросе (`new Date(t++)`).
struct TickingClock(AtomicI64);

impl Clock for TickingClock {
    fn now_ms(&self) -> i64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
}

#[test]
fn updater_log_matches_node() {
    let expected = &FIXTURE["log"];
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("logs").join("updater.log");
    // Date.UTC(2026, 9, 17, 10, 0, 0, 5)
    let clock = Arc::new(TickingClock(AtomicI64::new(1_792_231_200_005)));
    let log = UpdaterLog::new(Some(file.clone()))
        .with_clock(clock)
        .with_echo(false);
    log.info("[updater] запуск");
    log.warn(
        "token=abcdef1234567890abcdef ghp_0123456789abcdefghijABCDEFGHIJ0123 Bearer abc.def.ghi",
    );
    log.error("");
    log.info("");
    log.info(&"x".repeat(2100));
    log.info(&"😀".repeat(1500));
    log.info("https://user:pass@hlsr.pro/a?token=1&x=2");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        str_of(&expected["first"])
    );

    let one = PathBuf::from(format!("{}.1", file.display()));
    std::fs::write(&one, "old rotated\n").unwrap();
    std::fs::write(&file, "a".repeat(512 * 1024 - 1)).unwrap();
    log.info("below the limit");
    assert_eq!(
        std::fs::metadata(&file).unwrap().len(),
        expected["sizeBelow"].as_u64().unwrap()
    );
    log.info("at the limit");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        str_of(&expected["rotated"]["log"])
    );
    assert_eq!(
        std::fs::metadata(&one).unwrap().len(),
        expected["rotated"]["one"].as_u64().unwrap()
    );
    assert_eq!(
        hlsr_update::updater::log::MAX_BYTES,
        expected["maxBytes"].as_u64().unwrap()
    );
}

// ------------------------------------------------------------ скорость

#[test]
fn speed_meter_matches_node() {
    for case in items(&FIXTURE["speed"]) {
        let clock = Arc::new(FakeClock::new(0));
        let mut meter = SpeedMeter::new(clock.clone());
        let rates: Vec<Value> = items(&case["points"])
            .iter()
            .map(|point| {
                clock.set(point[0].as_i64().unwrap());
                meter.push(point[1].as_u64().unwrap());
                js_number(meter.bytes_per_second())
            })
            .collect();
        assert_eq!(json!(rates), case["rates"], "{}", case["points"]);
    }
}

// ------------------------------------------------------------ загрузка

const OWN_TEXTS: [&str; 7] = [
    "таймаут",
    "отменено",
    "отменено пользователем",
    "ответ длиннее размера",
    "слишком много редиректов",
    "редирект вне allowlist",
    "неразбираемый URL",
];

fn download_error(error: &UpdaterError) -> Value {
    let mut out = error_json(error);
    let message = error.message.as_str();
    let own = OWN_TEXTS.contains(&message)
        || message
            .strip_prefix("получено ")
            .and_then(|rest| rest.split_once(" из "))
            .is_some_and(|(a, b)| {
                a.bytes().all(|c| c.is_ascii_digit()) && b.bytes().all(|c| c.is_ascii_digit())
            });
    if error.code == UpdCode::DownloadFailed && !own {
        out["message"] = json!("<network>");
    }
    out
}

fn allow_loopback(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    parsed.scheme() == "http"
        && parsed.host_str() == Some("127.0.0.1")
        && !parsed.path().starts_with("/blocked")
}

fn pattern(size: usize, offset: usize) -> Vec<u8> {
    (0..size).map(|i| ((offset + i) % 251) as u8).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_matches_node() {
    let fixture_download = &FIXTURE["download"];
    let size = fixture_download["size"].as_u64().unwrap();
    assert_eq!(
        download::FREE_SPACE_RESERVE,
        fixture_download["reserve"].as_u64().unwrap()
    );
    let body = pattern(size as usize, 0);
    let mut mismatches = Mismatches::default();
    for case in items(&fixture_download["cases"]) {
        let id = str_of(&case["id"]);
        let log = RequestLog::default();
        let mut servers: Vec<Server> = Vec::new();
        let mut map = BTreeMap::new();
        for name in case["routes"].as_object().unwrap().keys() {
            let server = http::start_server(name, log.clone()).await;
            map.insert(name.clone(), server.authority.clone());
            servers.push(server);
        }
        if case["dead"] == json!(true) {
            map.insert(
                "dead".to_owned(),
                format!("127.0.0.1:{}", http::dead_port().await),
            );
        }
        for server in &servers {
            let routes = http::fill(&case["routes"][&server.name], &map);
            *server.routes.lock().unwrap() = routes.as_object().unwrap().clone();
        }
        let servers = Arc::new(servers);

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("HLSR-Setup-0.5.0.exe");
        if let Some(part) = case["part"].as_u64() {
            let mut bytes = pattern(size.min(part) as usize, 0);
            bytes.resize(part as usize, 0);
            std::fs::write(dir.path().join("HLSR-Setup-0.5.0.exe.part"), bytes).unwrap();
        }
        if let Some(target_size) = case["target"].as_u64() {
            std::fs::write(&target, vec![7u8; target_size as usize]).unwrap();
        }

        let urls_value = http::fill(&case["urls"], &map);
        let urls: Vec<String> = items(&urls_value)
            .iter()
            .map(|url| str_of(url).to_owned())
            .collect();
        let mut options = DownloadOptions::new(
            urls,
            dir.path().to_owned(),
            "HLSR-Setup-0.5.0.exe",
            size,
            Arc::new(allow_loopback),
        );
        if let Some(attempts) = case["attempts"].as_u64() {
            options.attempts = attempts as u32;
        }
        if let Some(reserve) = case["reserve"].as_u64() {
            options.reserve = reserve;
        }
        if let Some(timeout) = case["timeout"].as_u64() {
            options.timeout = Duration::from_millis(timeout);
        }
        options.free_space = Arc::new(FixedFreeSpace(case["free"].as_u64()));
        let cancel = CancelToken::new();
        if case["cancel"] == json!("start") {
            cancel.cancel();
        }
        options.cancel = cancel.clone();
        let last: Arc<Mutex<Option<(u64, u64)>>> = Arc::default();
        let cancel_on_progress = case["cancel"] == json!("progress");
        options.on_progress = Some({
            let (last, servers, cancel) = (last.clone(), servers.clone(), cancel.clone());
            Arc::new(move |progress: DownloadProgress| {
                *last.lock().unwrap() = Some((progress.done, progress.total));
                if cancel_on_progress && progress.done >= 1000 {
                    cancel.cancel();
                    for server in servers.iter() {
                        server.release("g");
                    }
                }
            })
        });

        let result = match download::download(options).await {
            Ok(file) => {
                let bytes = std::fs::read(&file).unwrap();
                json!({
                    "ok": file.file_name().unwrap().to_string_lossy(),
                    "size": bytes.len(),
                    "intact": bytes == body,
                })
            }
            Err(error) => json!({"error": download_error(&error)}),
        };
        for server in servers.iter() {
            server.release("g");
        }
        tokio::time::sleep(Duration::from_millis(30)).await;

        let mut files: Vec<Value> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|entry| {
                json!({
                    "name": entry.file_name().to_string_lossy(),
                    "size": entry.metadata().unwrap().len(),
                })
            })
            .collect();
        files.sort_by(|a, b| str_of(&a["name"]).cmp(str_of(&b["name"])));
        let requests: Vec<Value> = log
            .lock()
            .unwrap()
            .iter()
            .map(|entry| {
                json!([
                    entry["server"],
                    entry["url"],
                    entry["headers"]["range"],
                    entry["headers"]["user-agent"],
                ])
            })
            .collect();
        let requests = http::unfill(&Value::Array(requests), &map);

        let mut actual = json!({
            "result": result,
            "requests": requests,
            "files": files,
            "progress": last.lock().unwrap().map(|(done, total)| json!({"done": done, "total": total})),
        });
        let mut expected = json!({
            "result": case["result"],
            "requests": case["requests"],
            "files": case["files"],
            "progress": case["progress"],
        });
        // Node при отмене посреди тела теряет буфер потока записи (`.part` пуст); Rust успевает
        // записать принятые куски. На поведение не влияет: отменённая сессия каталог не продолжает.
        if id == "cancel during body" {
            actual["files"] = Value::Null;
            expected["files"] = Value::Null;
        }
        mismatches.check(&actual, &expected, id);
    }
    mismatches.finish("download");
}

// ------------------------------------------------------------ сессия

type Records = Arc<Mutex<Vec<Value>>>;

/// Журнал, который пишет строки в общую трассу.
struct RecordingLogger {
    records: Records,
    work: String,
}

impl RecordingLogger {
    fn push(&self, level: &str, message: &str) {
        self.records
            .lock()
            .unwrap()
            .push(json!(["log", level, mask(message, &self.work)]));
    }
}

impl Logger for RecordingLogger {
    fn info(&self, message: &str) {
        self.push("info", message);
    }
    fn warn(&self, message: &str) {
        self.push("warn", message);
    }
    fn error(&self, message: &str) {
        self.push("error", message);
    }
}

fn mask(text: &str, work: &str) -> String {
    let out = text.replace(work, "{work}");
    if cfg!(windows) {
        out
    } else {
        out.replace('/', "\\")
    }
}

fn throw_of(spec: &Value) -> UpdaterError {
    if let Some(upd) = spec["throwUpd"].as_array() {
        let code = UpdCode::ALL
            .into_iter()
            .find(|code| code.as_str() == upd[0].as_str().unwrap())
            .unwrap();
        let error = UpdaterError::new(code, upd[1].as_str().unwrap());
        return match upd.get(2) {
            Some(details) => error.with_details(details.clone()),
            None => error,
        };
    }
    // Простое исключение JS: сессия оборачивает его в UPD_INSTALL_FAILED.
    UpdaterError::new(UpdCode::InstallFailed, spec["throw"].as_str().unwrap())
}

fn lu_code(text: &str) -> LuCode {
    LuCode::ALL
        .into_iter()
        .find(|code| code.as_str() == text)
        .unwrap_or_else(|| panic!("код {text}"))
}

/// Зависимости сессии по сценарию — как заглушки `updater-core.js`.
struct ScriptedDeps {
    spec: Value,
    records: Records,
    work: String,
    install_dir: String,
    session: OnceLock<Weak<UpdateSession>>,
    installer_runs: AtomicUsize,
}

impl ScriptedDeps {
    fn push(&self, record: Value) {
        self.records.lock().unwrap().push(record);
    }

    fn mask(&self, path: &Path) -> String {
        mask(&path.to_string_lossy(), &self.work)
    }

    fn cancel_now(&self, at: &str) {
        let session = self.session.get().and_then(Weak::upgrade).unwrap();
        let accepted = session.cancel();
        self.push(json!(["cancel", at, accepted]));
    }
}

impl SessionDeps for ScriptedDeps {
    fn fetch_manifest<'a>(
        &'a self,
        _delta: &'a Delta,
        version: &'a str,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<Manifest, ChunkError>> {
        self.push(json!(["fetchManifest", {"version": version, "platform": "win32", "aborted": cancel.is_cancelled()}]));
        let spec = &self.spec["fetchManifest"];
        let result = if spec.is_object() {
            Err(ChunkError::Network(throw_of(spec).message))
        } else {
            Ok(Manifest {
                version: version.to_owned(),
                platform: hlsr_core::js::Platform::Win32,
                files: Vec::new(),
                attest: None,
            })
        };
        Box::pin(async move { result })
    }

    fn assemble<'a>(
        &'a self,
        request: AssembleRequest,
        on_progress: chunks::OnProgress,
    ) -> BoxFuture<'a, Result<Assembled, ChunkError>> {
        self.push(json!(["assemble", {
            "sourceRoot": request.source_root,
            "targetDir": self.mask(&request.target_dir),
            "cacheDir": self.mask(&request.cache_dir),
            "chunkBaseUrls": request.chunk_base_urls,
            "treeExists": request.target_dir.exists(),
        }]));
        for progress in self.spec["assembleProgress"]
            .as_array()
            .into_iter()
            .flatten()
        {
            on_progress(chunks::Progress {
                done: progress["done"].as_u64().unwrap(),
                total: progress["total"].as_u64().unwrap(),
                bytes_per_second: progress["bytesPerSecond"].as_u64().unwrap(),
            });
        }
        let spec = &self.spec["assemble"];
        let result = if spec == "cancel" {
            self.cancel_now("assemble");
            Err(ChunkError::Aborted)
        } else if spec.is_object() {
            Err(ChunkError::Network(throw_of(spec).message))
        } else {
            std::fs::write(request.target_dir.join("HLSR.exe"), "tree exe").unwrap();
            Ok(Assembled {
                downloaded_bytes: 1,
                reused_bytes: 2,
                downloaded_chunks: 1,
            })
        };
        Box::pin(async move { result })
    }

    fn download<'a>(
        &'a self,
        request: DownloadRequest,
        on_progress: download::OnProgress,
    ) -> BoxFuture<'a, Result<PathBuf, UpdaterError>> {
        self.push(json!(["download", {
            "urls": request.urls,
            "dir": self.mask(&request.dir),
            "filename": request.filename,
            "size": request.size,
            "allowed": hlsr_update::channel::is_allowed_url("https://hlsr.pro/downloads/launcher/stable/x"),
            "cancelled": request.cancel.is_cancelled(),
        }]));
        for progress in self.spec["downloadProgress"]
            .as_array()
            .into_iter()
            .flatten()
        {
            on_progress(DownloadProgress {
                done: progress["done"].as_u64().unwrap(),
                total: progress["total"].as_u64().unwrap(),
                bytes_per_second: progress["bytesPerSecond"].as_f64().unwrap(),
            });
        }
        let spec = &self.spec["download"];
        let file = request.dir.join(&request.filename);
        let result = if spec == "cancel" {
            self.cancel_now("download");
            Err(UpdaterError::new(UpdCode::Cancelled, "отменено"))
        } else if spec == "cancel-after" {
            self.cancel_now("download");
            std::fs::write(&file, "installer").unwrap();
            Ok(file)
        } else if spec.is_object() {
            Err(throw_of(spec))
        } else {
            std::fs::write(&file, "installer").unwrap();
            Ok(file)
        };
        Box::pin(async move { result })
    }

    fn verify_installer<'a>(
        &'a self,
        path: &'a Path,
        installer: &'a Value,
    ) -> BoxFuture<'a, VerifyOutcome> {
        self.push(json!(["verify", self.mask(path), installer]));
        let spec = &self.spec["verify"];
        let outcome = if spec.is_object() {
            VerifyOutcome {
                ok: false,
                problems: spec["problems"]
                    .as_array()
                    .map(|list| list.iter().map(|p| str_of(p).to_owned()).collect())
                    .unwrap_or_default(),
                code: spec["code"].as_str().map(lu_code),
            }
        } else {
            VerifyOutcome {
                ok: true,
                problems: Vec::new(),
                code: None,
            }
        };
        Box::pin(async move { outcome })
    }

    fn wait_for_exit<'a>(
        &'a self,
        on_wait: &'a (dyn Fn(&WaitInfo) + Send + Sync),
    ) -> BoxFuture<'a, Result<(), UpdaterError>> {
        self.push(json!(["wait", process_wait::LAUNCHER_IMAGE]));
        for info in self.spec["waits"].as_array().into_iter().flatten() {
            on_wait(&WaitInfo {
                pids: items(&info["pids"])
                    .iter()
                    .map(|pid| pid.as_u64().unwrap() as u32)
                    .collect(),
                remaining_ms: info["remainingMs"].as_i64().unwrap(),
            });
        }
        let spec = &self.spec["wait"];
        let result = if spec.is_object() {
            Err(throw_of(spec))
        } else {
            Ok(())
        };
        Box::pin(async move { result })
    }

    fn run_installer<'a>(
        &'a self,
        installer: &'a Path,
        options: InstallOptions,
    ) -> BoxFuture<'a, Result<(), UpdaterError>> {
        let portable = options
            .env
            .iter()
            .find(|(name, _)| name == "PORTABLE_EXECUTABLE_FILE")
            .map(|(_, value)| mask(value, &self.work));
        let record = match &portable {
            Some(portable) => json!({
                "args": options.args,
                "windowsHide": options.hidden,
                "PORTABLE_EXECUTABLE_FILE": portable,
            }),
            None => {
                // Полный установщик: 0.4.7 передавала `{installDir}`, аргументы строились внутри;
                // `/D=` теперь всегда, и с пробелом в пути (`InstallOptions::installer`).
                assert_eq!(
                    options.args,
                    [
                        "/S".to_owned(),
                        "--updated".to_owned(),
                        format!("/D={}", self.install_dir)
                    ]
                );
                assert!(!options.hidden);
                json!({"installDir": self.install_dir, "PORTABLE_EXECUTABLE_FILE": null})
            }
        };
        self.push(json!(["runInstaller", self.mask(installer), record]));
        let list = self.spec["runInstaller"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| vec![json!("ok")]);
        let index = self.installer_runs.fetch_add(1, Ordering::SeqCst);
        let spec = list[index.min(list.len() - 1)].clone();
        let result = if spec.is_object() {
            Err(throw_of(&spec))
        } else {
            Ok(())
        };
        Box::pin(async move { result })
    }

    fn verify_installed(&self, launcher_exe: &str, target_version: &str) -> Installed {
        self.push(json!(["verifyInstalled", {"launcherExe": launcher_exe, "targetVersion": target_version}]));
        let spec = &self.spec["verifyInstalled"];
        if spec.is_object() {
            Installed {
                ok: spec["ok"].as_bool().unwrap(),
                code: spec["code"].as_str().map(|code| {
                    UpdCode::ALL
                        .into_iter()
                        .find(|c| c.as_str() == code)
                        .unwrap()
                }),
                detail: spec["detail"].as_str().map(str::to_owned),
                version: None,
                unverified: false,
            }
        } else {
            Installed {
                ok: true,
                code: None,
                detail: None,
                version: Some(Some("0.5.0.0".to_owned())),
                unverified: false,
            }
        }
    }

    fn write_receipt(&self, file: &str, from: Option<&str>, to: &str) -> bool {
        self.push(json!(["receipt", file, from, to]));
        self.spec["receipt"].as_bool().unwrap_or(true)
    }

    fn relaunch(&self, launcher_exe: &str) -> Result<(), UpdaterError> {
        self.push(json!(["relaunch", launcher_exe, {}]));
        let spec = &self.spec["relaunch"];
        if spec.is_object() {
            Err(throw_of(spec))
        } else {
            Ok(())
        }
    }

    fn installer_env(&self) -> Vec<(String, String)> {
        vec![("Path".to_owned(), "C:\\Windows".to_owned())]
    }
}

const SITE: &str = "https://hlsr.pro/downloads/launcher/stable";

fn session_plan(case: &Value) -> plan::Plan {
    let delta = json!({
        "schema": 1,
        "manifest": {"filename": "HLSR-0.5.0-win32.chunks.json", "size": 10, "sha256": "c".repeat(64), "urls": [format!("{SITE}/HLSR-0.5.0-win32.chunks.json")]},
        "chunkBaseUrls": ["https://hlsr.pro/downloads/launcher/chunks"],
    });
    let manifest = match &case["delta"] {
        Value::Bool(true) => json!({"delta": delta, "installer": {}}),
        Value::String(text) if text == "invalid" => {
            let mut bad = delta.clone();
            bad["chunkBaseUrls"] = json!(["https://example.com/chunks"]);
            json!({"delta": bad, "installer": {}})
        }
        _ => json!({"installer": {}}),
    };
    let install_dir = case["installDir"]
        .as_str()
        .unwrap_or("C:\\HLSR\\Launcher")
        .to_owned();
    plan::Plan {
        manifest,
        current_version: Some(json!("0.4.7")),
        target_version: "0.5.0".to_owned(),
        installer: plan::PlanInstaller {
            filename: "HLSR-Setup-0.5.0.exe".to_owned(),
            size: json!(10),
            sha256: json!("a".repeat(64)),
            sha512: json!("b"),
            urls: vec![
                format!("{SITE}/HLSR-Setup-0.5.0.exe"),
                "https://github.com/HL-HLSR/HLSR-Releases/releases/download/v0.5.0/HLSR-Setup-0.5.0.exe".to_owned(),
            ],
        },
        launcher_exe: format!("{install_dir}\\HLSR.exe"),
        install_dir,
        receipt_file: "C:\\Users\\player\\AppData\\Roaming\\hlsr\\version-state.json".to_owned(),
        language: "ru".to_owned(),
    }
}

#[tokio::test]
async fn session_traces_match_node() {
    let cases = items(&FIXTURE["session"]);
    assert!(cases.len() >= 25, "в фикстуре {} случаев", cases.len());
    let mut mismatches = Mismatches::default();
    for case in cases {
        let root = tempfile::tempdir().unwrap();
        let work_dir = root.path().join("work");
        std::fs::create_dir_all(&work_dir).unwrap();
        let work = work_dir.to_string_lossy().into_owned();
        let records: Records = Arc::default();
        let plan = session_plan(case);
        let deps = Arc::new(ScriptedDeps {
            spec: case.clone(),
            records: records.clone(),
            work: work.clone(),
            install_dir: plan.install_dir.clone(),
            session: OnceLock::new(),
            installer_runs: AtomicUsize::new(0),
        });
        let logger = Arc::new(RecordingLogger {
            records: records.clone(),
            work: work.clone(),
        });
        let cancel_at: Vec<String> = case["cancelAt"]
            .as_array()
            .map(|list| list.iter().map(|s| str_of(s).to_owned()).collect())
            .unwrap_or_default();
        let session_slot: Arc<OnceLock<Weak<UpdateSession>>> = Arc::default();
        let session = Arc::new(UpdateSession::new(
            plan,
            work_dir.clone(),
            logger,
            deps.clone(),
            {
                let (records, slot) = (records.clone(), session_slot.clone());
                move |event| {
                    let record = match &event {
                        SessionEvent::Stage(payload) => json!(["stage", payload]),
                        SessionEvent::Progress(payload) => json!(["progress", payload]),
                        SessionEvent::Waiting(payload) => json!(["waiting", payload]),
                        SessionEvent::Done(payload) => json!(["done", payload]),
                        SessionEvent::Failed(payload) => json!(["failed", payload]),
                        SessionEvent::Cancelled => json!(["cancelled"]),
                    };
                    records.lock().unwrap().push(record);
                    if let SessionEvent::Stage(payload) = &event
                        && cancel_at
                            .iter()
                            .any(|stage| stage == str_of(&payload["stage"]))
                    {
                        let session = slot.get().and_then(Weak::upgrade).unwrap();
                        let accepted = session.cancel();
                        records
                            .lock()
                            .unwrap()
                            .push(json!(["cancel", payload["stage"], accepted]));
                    }
                }
            },
        ));
        let _ = deps.session.set(Arc::downgrade(&session));
        let _ = session_slot.set(Arc::downgrade(&session));

        let result = session.run().await;
        let after = session.cancel();
        records
            .lock()
            .unwrap()
            .push(json!(["cancel", "after", after]));
        let mut left: Vec<String> = std::fs::read_dir(&work_dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();

        let actual = json!({
            "records": records.lock().unwrap().clone(),
            "result": result.to_json(),
            "left": left,
        });
        let expected =
            json!({"records": case["records"], "result": case["result"], "left": case["left"]});
        mismatches.check(&actual, &expected, str_of(&case["id"]));
    }
    mismatches.finish("UpdateSession");
}

/// `launcher-chunks-selftest.js`, тест 5: «Windows: апдейтер ставит собранную из чанков версию её
/// же установщиком, при сбое качает полный» — случаи «chunks…» и «invalid delta» трасс сессии.
#[test]
fn chunks_selftest_cases_are_in_the_fixture() {
    let ids: Vec<&str> = items(&FIXTURE["session"])
        .iter()
        .map(|case| str_of(&case["id"]))
        .collect();
    for id in [
        "chunks",
        "chunks: assemble fails, full installer",
        "full installer",
    ] {
        assert!(ids.contains(&id), "{id}");
    }
    let chunks = items(&FIXTURE["session"])
        .iter()
        .find(|case| str_of(&case["id"]) == "chunks")
        .unwrap();
    let install = items(&chunks["records"])
        .iter()
        .find(|record| record[0] == "runInstaller")
        .unwrap();
    assert_eq!(install[1], json!("{work}\\tree\\HLSR.exe"));
    assert_eq!(
        install[2]["args"],
        json!(["/S", "--updated", "/D=C:\\HLSR\\Launcher"])
    );
    assert_eq!(
        install[2]["PORTABLE_EXECUTABLE_FILE"],
        json!("{work}\\tree\\HLSR-Setup-0.5.0.exe")
    );
}

// ------------------------------------------------------------ запуск копии

#[tokio::test]
async fn wait_for_startup_matches_node() {
    for case in items(&FIXTURE["launch"]["startup"]) {
        let clock = Arc::new(FakeClock::new(0));
        let slack = case["slack"].as_i64().unwrap_or(0);
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let asked = Mutex::new(Vec::new());
        let dies_at = case["diesAt"].as_i64();
        let alive = |_pid: u32| {
            let t = clock.now_ms();
            asked.lock().unwrap().push(t);
            dies_at.is_none_or(|at| t < at)
        };
        let sleep = {
            let (clock, sleeps) = (clock.clone(), sleeps.clone());
            move |duration: Duration| -> BoxFuture<'static, ()> {
                sleeps.lock().unwrap().push(duration.as_millis() as u64);
                clock.advance(duration.as_millis() as i64 + slack);
                Box::pin(async {})
            }
        };
        let result = launch::wait_for_startup(
            42,
            &alive,
            clock.as_ref(),
            &sleep,
            case["timeoutMs"]
                .as_u64()
                .map_or(launch::STARTUP_TIMEOUT, Duration::from_millis),
            case["stableMs"]
                .as_u64()
                .map_or(launch::STARTUP_STABLE, Duration::from_millis),
        )
        .await;
        let result = match result {
            Ok(()) => json!({"ok": true}),
            Err(reason) => json!({"ok": false, "reason": reason}),
        };
        assert_eq!(result, case["result"], "{}", case["id"]);
        assert_eq!(
            json!(sleeps.lock().unwrap().clone()),
            case["sleeps"],
            "{}",
            case["id"]
        );
        assert_eq!(
            json!(asked.lock().unwrap().clone()),
            case["asked"],
            "{}",
            case["id"]
        );
    }
}

#[test]
fn spawn_updater_matches_node() {
    for case in items(&FIXTURE["launch"]["spawn"]) {
        let pid = case["pid"].as_u64().unwrap_or(0) as u32;
        let launcher = FakeLauncher::new().on_spawn(move |_| Ok(pid));
        let result = launch::spawn_updater(
            &launcher,
            Path::new("C:\\T\\U\\HLSRUpdater.exe"),
            Path::new("C:\\T\\U\\update-plan.json"),
            Vec::new(),
        );
        let call = launcher.calls().remove(0);
        let command = call.command();
        assert_eq!(json!(command.args), case["calls"][0]["args"]);
        assert_eq!(
            json!(
                command
                    .cwd
                    .as_ref()
                    .map(|cwd| cwd.to_string_lossy().replace('/', "\\"))
            ),
            case["calls"][0]["options"]["cwd"]
        );
        assert!(command.detached && !command.hidden);
        match result {
            Ok(pid) => assert_eq!(json!({"pid": pid}), case["result"]),
            Err(error) => assert_eq!(json!(error), case["error"]),
        }
    }
}

#[tokio::test]
async fn launch_logs_match_node() {
    for case in items(&FIXTURE["launch"]["launch"]) {
        let root = tempfile::tempdir().unwrap();
        let app_dir = root.path().join("Launcher");
        let resources = app_dir.join("resources");
        std::fs::create_dir_all(&resources).unwrap();
        std::fs::write(app_dir.join("HLSR.exe"), "exe").unwrap();
        std::fs::write(resources.join("QA_TRUST"), "{}").unwrap();
        let spawn_error = case["spawnThrows"].as_str().map(str::to_owned);
        let started = case["startup"]["ok"] == json!(true);
        let launcher = FakeLauncher::new()
            .on_spawn(move |_| spawn_error.clone().map_or(Ok(77), Err))
            .on_alive(move |_| started);
        let records: Records = Arc::default();
        let logger = RecordingLogger {
            records: records.clone(),
            work: "\u{0}".to_owned(),
        };
        let clock = Arc::new(FakeClock::new(0));
        let sleep = {
            let clock = clock.clone();
            move |duration: Duration| -> BoxFuture<'static, ()> {
                clock.advance(duration.as_millis() as i64);
                Box::pin(async {})
            }
        };
        let plan = json!({"schema": 1});
        let result = launch::launch(LaunchOptions {
            app_exe: &app_dir.join("HLSR.exe"),
            resources: &resources,
            temp_root: root.path(),
            dir: Some(root.path().join("HLSR-Updater-x-0123456789ab")),
            plan: &plan,
            launcher: &launcher,
            env: Vec::new(),
            clock: clock.as_ref(),
            sleep: &sleep,
            logger: &logger,
        })
        .await;
        let lines: Vec<Value> = records
            .lock()
            .unwrap()
            .iter()
            .map(|record| json!([record[1], record[2]]))
            .collect();
        assert_eq!(json!(lines), case["lines"], "{}", case["id"]);
        match result {
            Ok(launched) => {
                assert_eq!(launched.pid, 77);
                // «updater stage» (windows-install-selftest) в новой форме: копия HLSR.exe и
                // QA_TRUST, без app.asar; план — JSON с двумя пробелами.
                let stage = root.path().join("HLSR-Updater-x-0123456789ab");
                assert_eq!(
                    std::fs::read_to_string(stage.join(launch::UPDATER_EXE)).unwrap(),
                    "exe"
                );
                assert_eq!(
                    std::fs::read_to_string(stage.join("resources").join("QA_TRUST")).unwrap(),
                    "{}"
                );
                assert!(!stage.join("resources").join("app.asar").exists());
                assert_eq!(
                    std::fs::read_to_string(stage.join(launch::PLAN_FILE)).unwrap(),
                    "{\n  \"schema\": 1\n}"
                );
                let call = launcher.calls().remove(0);
                assert_eq!(
                    call.command().args,
                    vec![
                        "--hlsr-updater".to_owned(),
                        format!("--plan={}", stage.join(launch::PLAN_FILE).display())
                    ]
                );
            }
            Err(error) => assert_eq!(json!(error), case["error"], "{}", case["id"]),
        }
    }
}

/// Время изменения каталога (`fs.utimesSync`).
fn set_mtime(path: &Path, ms: i64) {
    let time = std::time::UNIX_EPOCH + Duration::from_millis(ms as u64);
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        std::fs::OpenOptions::new()
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
            .unwrap()
    };
    #[cfg(not(windows))]
    let file = std::fs::File::open(path).unwrap();
    file.set_modified(time).unwrap();
}

#[test]
fn cleanup_old_stages_matches_node() {
    let expected = &FIXTURE["launch"]["cleanup"];
    let now = expected["now"].as_i64().unwrap();
    let root = tempfile::tempdir().unwrap();
    for entry in items(&expected["entries"]) {
        let full = root.path().join(str_of(&entry["name"]));
        if entry["kind"] == "dir" {
            std::fs::create_dir_all(full.join("work")).unwrap();
            std::fs::write(full.join("HLSRUpdater.exe"), "x").unwrap();
        } else {
            std::fs::write(&full, "x").unwrap();
        }
        set_mtime(&full, now - entry["ageMs"].as_i64().unwrap());
    }
    let removed = launch::cleanup_old_stages(root.path(), launch::STAGE_MAX_AGE_MS, now);
    assert_eq!(json!({"removed": removed}), expected["result"]);
    let mut left: Vec<String> = std::fs::read_dir(root.path())
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(json!(left), expected["left"]);
    assert_eq!(
        launch::cleanup_old_stages(&root.path().join("missing"), launch::STAGE_MAX_AGE_MS, now),
        0
    );
}

#[test]
fn unique_dir_and_plan_text_match_node() {
    let unique = &FIXTURE["launch"]["uniqueDir"];
    let dir = launch::unique_dir(
        Path::new("C:\\Temp"),
        unique["now"].as_i64().unwrap(),
        launch::random_suffix(),
    );
    let name = dir.file_name().unwrap().to_string_lossy().into_owned();
    let prefix = str_of(&unique["prefix"]).rsplit('\\').next().unwrap();
    assert!(name.starts_with(prefix), "{name} / {prefix}");
    assert_eq!(
        name.len() - prefix.len(),
        unique["suffixLength"].as_u64().unwrap() as usize
    );
    assert!(launch::is_stage_name(&name));
    assert_ne!(launch::random_suffix(), launch::random_suffix());

    let plan_case = &FIXTURE["launch"]["plan"];
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("update-plan.json");
    launch::write_plan(&file, &plan_case["plan"]).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        str_of(&plan_case["text"])
    );
    assert_eq!(
        stringify(&plan_case["plan"], "  "),
        str_of(&plan_case["text"])
    );
    assert_eq!(
        launch::UPDATER_EXE,
        str_of(&FIXTURE["launch"]["constants"]["UPDATER_EXE"])
    );
}

// ------------------------------------------------------------ updater/main.js

#[test]
fn main_helpers_match_node() {
    let main = &FIXTURE["main"];
    for case in items(&main["argv"]) {
        assert_eq!(
            json!(mode::plan_path_from_argv(&strings(&case["argv"]))),
            case["plan"],
            "{}",
            case["argv"]
        );
    }

    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, text: &str| {
        let file = dir.path().join(name);
        std::fs::write(&file, text).unwrap();
        file.to_string_lossy().into_owned()
    };
    let missing = dir
        .path()
        .join("missing.json")
        .to_string_lossy()
        .into_owned();
    let inputs: Vec<(&str, Option<String>)> = vec![
        ("no path", None),
        ("empty path", Some(String::new())),
        ("missing file", Some(missing)),
        ("not json", Some(write("bad.json", "{plan"))),
        ("bom", Some(write("bom.json", "\u{feff}{\"schema\":1}"))),
        (
            "ok",
            Some(write("ok.json", "{\"schema\":1,\"language\":\"en\"}")),
        ),
        ("null", Some(write("null.json", "null"))),
    ];
    let dir_text = dir.path().to_string_lossy().into_owned();
    for (id, file) in inputs {
        let case = items(&main["readPlan"])
            .iter()
            .find(|case| str_of(&case["id"]) == id)
            .unwrap();
        let actual = match mode::read_plan_file(file.as_deref()) {
            Ok(value) => json!({"id": id, "result": value}),
            Err(error) => json!({"id": id, "error": {
                "code": error.code.as_str(),
                "message": error.message.replace(&dir_text, "{dir}"),
            }}),
        };
        assert_eq!(&actual, case, "{id}");
    }

    let qa_root = tempfile::tempdir().unwrap();
    let resources = |name: &str, text: Option<&str>| {
        let dir = qa_root.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(text) = text {
            std::fs::write(dir.join("QA_TRUST"), text).unwrap();
        }
        dir
    };
    let marker_dir = resources("i", None);
    std::fs::create_dir_all(marker_dir.join("QA_TRUST")).unwrap();
    let inputs = vec![
        ("missing", resources("a", None)),
        ("not json", resources("b", Some("x"))),
        ("no keys", resources("c", Some("{}"))),
        ("empty keys", resources("d", Some("{\"publicKeys\":[]}"))),
        (
            "keys string",
            resources("e", Some("{\"publicKeys\":\"k\"}")),
        ),
        (
            "keys",
            resources("f", Some("{\"publicKeys\":[\"k1\",\"k2\"]}")),
        ),
        (
            "keys and base",
            resources(
                "g",
                Some("{\"publicKeys\":[\"k\"],\"downloadBase\":\"http://127.0.0.1:9/feed\"}"),
            ),
        ),
        (
            "base not string",
            resources("h", Some("{\"publicKeys\":[\"k\"],\"downloadBase\":5}")),
        ),
        ("marker is a directory", marker_dir),
        ("no resources path", PathBuf::new()),
    ];
    for (id, dir) in inputs {
        let case = items(&main["qaTrust"])
            .iter()
            .find(|case| str_of(&case["id"]) == id)
            .unwrap();
        assert_eq!(
            json!(mode::qa_trust_options(&dir).map(|qa| qa.to_json())),
            case["result"],
            "{id}"
        );
    }

    assert_eq!(
        json!({"width": mode::WINDOW_SIZE.0, "height": mode::WINDOW_SIZE.1}),
        main["window"]
    );
    assert_eq!(json!(mode::WINDOW_TITLE), main["title"]);
}
