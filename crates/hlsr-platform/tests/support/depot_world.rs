//! Поддельный Steam и мир для проверок службы депо: настоящие папки Steam и библиотеки во
//! временной папке, запуск программ `run` (отвечает в настоящий `console_log.txt`, создаёт файлы
//! депо через ссылку на слот), поддельные часы и пауза (каждая пауза — опрос), поиск Steam.
//! Мир описывается данными сценария: `tools/reference/platform/steam-depot.js` и тесты рядом.

#![allow(dead_code)]

use hlsr_delivery::errors::{DeliveryError, Failure};
use hlsr_delivery::providers::BoxFuture;
use hlsr_platform::steam_depot::{
    CommandRunner, DepotSteam, RunCode, RunError, RunResult, Sleeper, SystemRunner,
};
use hlsr_platform::steam_locator::{RunningSteam, SteamInstall};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Замена корня временной папки на `<root>`: в обычной записи, в записи внутри строки JSON
/// (двойные `\`) и с прямыми косыми — как `scrubber` скрипта.
pub fn scrub_root(root: &Path, text: &str) -> String {
    let raw = root.to_string_lossy().into_owned();
    let escaped = raw.replace('\\', "\\\\");
    let slashes = raw.replace('\\', "/");
    text.replace(&raw, "<root>")
        .replace(&escaped, "<root>")
        .replace(&slashes, "<root>")
}

fn str_of(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("ожидалась строка, а не {value}"))
}

pub type Task = Box<dyn FnOnce() + Send>;

pub struct World {
    pub root: PathBuf,
    pub steam_root: PathBuf,
    pub lib: PathBuf,
    pub elsewhere: PathBuf,
    pub content_dir: PathBuf,
    pub console_log: PathBuf,
    pub scenario: Value,
    pub eol: String,
    pub state: Mutex<WorldState>,
}

pub struct WorldState {
    pub now: i64,
    pub polls: u64,
    pub queue: Vec<(u64, Task)>,
    pub depot_calls: HashMap<String, u32>,
    pub commands: Vec<Value>,
    pub pid: Option<u32>,
    pub locate_error: Option<Value>,
    pub running_error: Option<Value>,
}

impl World {
    pub fn scrub(&self, text: &str) -> String {
        scrub_root(&self.root, text)
    }

    pub fn scrub_value(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.scrub(text)),
            Value::Array(list) => Value::Array(list.iter().map(|v| self.scrub_value(v)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), self.scrub_value(v)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    pub fn append(&self, lines: &[String]) {
        use std::io::Write;
        let mut handle = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.console_log)
            .unwrap();
        for line in lines {
            handle
                .write_all(format!("{line}{}", self.eol).as_bytes())
                .unwrap();
        }
    }

    pub fn state(&self) -> std::sync::MutexGuard<'_, WorldState> {
        self.state.lock().unwrap()
    }

    /// `schedule(afterPolls, fn)`.
    pub fn schedule(self: &Arc<Self>, after_polls: u64, task: Task) {
        if after_polls == 0 {
            task();
        } else {
            let due = self.state().polls + after_polls;
            self.state().queue.push((due, task));
        }
    }

    pub fn steam(&self) -> &Value {
        &self.scenario["steam"]
    }
}

pub fn depot_file(dir: &Path, index: usize) {
    let rel = if index.is_multiple_of(2) {
        format!("bin/f{index}.dll")
    } else {
        format!("hl2/f{index}.dat")
    };
    let full = rel
        .split('/')
        .fold(dir.to_path_buf(), |path, part| path.join(part));
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, index.to_string()).unwrap();
}

pub struct FakeSteam(pub Arc<World>);

impl FakeSteam {
    pub fn react(&self, name: &str, rest: &[String]) {
        let world = &self.0;
        let steam = world.steam();
        match name {
            "licenses_for_app" => {
                let id = rest[0].clone();
                let verdict = steam["licenses"][&id]
                    .as_str()
                    .unwrap_or("owned")
                    .to_owned();
                let delay = steam["licenseDelay"][&id].as_u64().unwrap_or(0);
                let lines = match verdict.as_str() {
                    "owned" => vec![
                        format!("Licenses for app {id}:"),
                        "  [ 1234567 ]  - Purchased : Tue Aug 13 08:09:10 2013".to_owned(),
                    ],
                    "missing" => vec![format!("No active license found for appID {id}")],
                    _ => Vec::new(),
                };
                let world2 = world.clone();
                world.schedule(
                    delay,
                    Box::new(move || {
                        let mut all = vec!["[licenses] noise".to_owned()];
                        all.extend(lines);
                        world2.append(&all);
                    }),
                );
            }
            "DepotDownloadProgressTimeout" => {
                if steam["progressTimeout"] != json!("silent") {
                    world.append(&[format!("DepotDownloadProgressTimeout = {}", rest[0])]);
                }
            }
            "sDepotDownloadFileFilter" => {
                if steam["filterEcho"] != json!("silent") {
                    world.append(&[format!("sDepotDownloadFileFilter = {}", rest[0])]);
                }
            }
            "download_depot" => {
                let first = rest[0].clone();
                let depot_id = rest[1].clone();
                let depot = steam["depots"][&depot_id].clone();
                let count = {
                    let mut state = world.state();
                    let count = state.depot_calls.entry(depot_id.clone()).or_insert(0);
                    *count += 1;
                    *count
                };
                if count % 2 == 1 {
                    match depot["prime"].as_str().unwrap_or("complete") {
                        "complete" => {
                            world.append(&[format!("Depot download complete : prime {first}")])
                        }
                        "failed" => {
                            world.append(&[format!("Depot download failed : prime {first}")])
                        }
                        _ => {}
                    }
                    return;
                }
                let main = &depot["main"];
                let total = main["files"].as_u64().unwrap_or(0) as usize;
                let polls = main["polls"].as_u64().unwrap_or(0);
                let outcome = main["outcome"].as_str().unwrap_or("complete").to_owned();
                let target = world
                    .content_dir
                    .join(format!("app_{first}"))
                    .join(format!("depot_{depot_id}"));
                let made = Arc::new(Mutex::new(0usize));
                let make_up_to = {
                    let target = target.clone();
                    let made = made.clone();
                    move |upto: usize| {
                        let mut made = made.lock().unwrap();
                        while *made < upto {
                            depot_file(&target, *made);
                            *made += 1;
                        }
                    }
                };
                // Только для проверок самой сборки: именованные файлы (`hl2.exe`…) кроме счётных.
                let extra: Vec<String> = main["extraFiles"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|name| str_of(name).to_owned())
                    .collect();
                let finish = {
                    let world = world.clone();
                    let target_dir = target.clone();
                    let target = target.to_string_lossy().into_owned();
                    move || {
                        for name in &extra {
                            let full = name
                                .split('/')
                                .fold(target_dir.clone(), |path, part| path.join(part));
                            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                            std::fs::write(full, name).unwrap();
                        }
                        match outcome.as_str() {
                            "complete" => {
                                world.append(&[format!("Depot download complete : {target}")])
                            }
                            "failed" => {
                                world.append(&[format!("Depot download failed : {target}")])
                            }
                            _ => {}
                        }
                    }
                };
                if polls == 0 {
                    make_up_to(total);
                    finish();
                } else {
                    let finish = Arc::new(finish);
                    let make_up_to = Arc::new(make_up_to);
                    for step in 1..=polls {
                        let finish = finish.clone();
                        let make_up_to = make_up_to.clone();
                        world.schedule(
                            step,
                            Box::new(move || {
                                make_up_to(total * step as usize / polls as usize);
                                if step == polls {
                                    finish();
                                }
                            }),
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

impl CommandRunner for FakeSteam {
    fn run<'a>(
        &'a self,
        file: &'a str,
        args: &'a [String],
        timeout_ms: u64,
    ) -> BoxFuture<'a, RunResult> {
        Box::pin(async move {
            let world = &self.0;
            world.state().commands.push(json!({
                "file": world.scrub(file),
                "args": args.iter().map(|a| world.scrub(a)).collect::<Vec<_>>(),
                "timeout": timeout_ms,
            }));
            let steam = world.steam();
            if file == "cmd.exe" {
                if steam["mklink"] == json!("fail") {
                    return RunResult {
                        error: Some(RunError {
                            killed: false,
                            signal: None,
                            code: Some(RunCode::Exit(1)),
                            message: "Command failed".to_owned(),
                        }),
                        status: 1,
                        stdout: String::new(),
                        stderr: steam["mklinkStderr"]
                            .as_str()
                            .unwrap_or("Отказано в доступе.\r\n")
                            .to_owned(),
                    };
                }
                return SystemRunner.run(file, args, timeout_ms).await;
            }
            let offset = world.scenario["steamArgs"].as_array().map_or(0, Vec::len);
            let name = args[offset].strip_prefix("steam://+").unwrap().to_owned();
            let rest: Vec<String> = args[offset + 1..].to_vec();
            self.react(&name, &rest);
            match steam["runErrors"].get(&name) {
                Some(failure) => match str_of(&failure["kind"]) {
                    "timeout" => RunResult {
                        error: Some(RunError {
                            killed: true,
                            signal: Some("SIGTERM".to_owned()),
                            code: None,
                            message: "timeout".to_owned(),
                        }),
                        status: 1,
                        ..RunResult::default()
                    },
                    "enoent" => RunResult {
                        error: Some(RunError {
                            killed: false,
                            signal: None,
                            code: Some(RunCode::Name("ENOENT".to_owned())),
                            message: str_of(&failure["message"]).to_owned(),
                        }),
                        status: 1,
                        ..RunResult::default()
                    },
                    _ => {
                        let code = failure["code"].as_i64().unwrap();
                        RunResult {
                            error: Some(RunError {
                                killed: false,
                                signal: None,
                                code: Some(RunCode::Exit(code)),
                                message: "Command failed".to_owned(),
                            }),
                            status: code,
                            ..RunResult::default()
                        }
                    }
                },
                None => RunResult::default(),
            }
        })
    }
}

impl Sleeper for FakeSteam {
    fn sleep<'a>(&'a self, ms: u64) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let due: Vec<Task> = {
                let mut state = self.0.state();
                state.now += ms as i64;
                state.polls += 1;
                let polls = state.polls;
                let (due, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut state.queue)
                    .into_iter()
                    .partition(|(at, _)| *at <= polls);
                state.queue = rest;
                due.into_iter().map(|(_, task)| task).collect()
            };
            for task in due {
                task();
            }
        })
    }
}

pub struct FakeLocator(pub Arc<World>);

impl DepotSteam for FakeLocator {
    fn locate_steam(&self) -> Result<SteamInstall, DeliveryError> {
        let world = &self.0;
        if let Some(error) = &world.state().locate_error {
            return Err(delivery_error(error));
        }
        Ok(match world.scenario["locator"].as_str() {
            Some("incomplete") => SteamInstall {
                install_path: String::new(),
                steam_exe: String::new(),
                steam_args: None,
            },
            Some("incompleteExe") => SteamInstall {
                install_path: world.steam_root.to_string_lossy().into_owned(),
                steam_exe: String::new(),
                steam_args: None,
            },
            _ => SteamInstall {
                install_path: world.steam_root.to_string_lossy().into_owned(),
                steam_exe: world
                    .steam_root
                    .join("steam.exe")
                    .to_string_lossy()
                    .into_owned(),
                steam_args: world.scenario["steamArgs"]
                    .as_array()
                    .map(|args| args.iter().map(|a| str_of(a).to_owned()).collect()),
            },
        })
    }

    fn assert_steam_running(&self, _steam: &SteamInstall) -> Result<RunningSteam, DeliveryError> {
        let world = &self.0;
        if let Some(error) = &world.state().running_error {
            return Err(delivery_error(error));
        }
        Ok(RunningSteam {
            pid: world.state().pid.unwrap_or(0),
            executable_path: None,
            started_at: None,
            source: None,
        })
    }
}

pub fn delivery_error(error: &Value) -> DeliveryError {
    let code = [
        hlsr_delivery::errors::Code::SteamNotInstalled,
        hlsr_delivery::errors::Code::SteamNotRunning,
    ]
    .into_iter()
    .find(|code| code.as_str() == str_of(&error["code"]))
    .expect("код ошибки сценария");
    DeliveryError::new(code, str_of(&error["message"]))
}

pub fn error_value(world: &World, failure: &Failure) -> Value {
    match failure {
        Failure::Delivery(error) => json!({
            "code": error.code.as_str(),
            "message": world.scrub(&error.message),
            "details": error.details.as_ref().map_or(Value::Null, |d| world.scrub_value(d)),
        }),
        Failure::Raw(error) => json!({"raw": true, "code": error.code}),
    }
}

// ----------------------------------------------------------------- сборка мира и службы

/// Мир для сценария: папки Steam (`Steam/logs`), библиотеки (`Lib/Games`, `Lib/.hlsr/staging`),
/// чужая папка и пустой `console_log.txt`.
pub fn build_world(root: &Path, scenario: Value) -> Arc<World> {
    let steam_root = root.join("Steam");
    let lib = root.join("Lib");
    let elsewhere = root.join("Elsewhere");
    let console_log = steam_root.join("logs").join("console_log.txt");
    for dir in [
        steam_root.join("logs"),
        lib.join("Games"),
        lib.join(".hlsr").join("staging"),
        lib.join("Downloads"),
        elsewhere.clone(),
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(&console_log, "").unwrap();
    Arc::new(World {
        root: root.to_path_buf(),
        content_dir: steam_root.join("steamapps").join("content"),
        steam_root,
        lib,
        elsewhere,
        console_log,
        eol: "\n".to_owned(),
        scenario,
        state: Mutex::new(WorldState {
            now: 1_000_000,
            polls: 0,
            queue: Vec::new(),
            depot_calls: HashMap::new(),
            commands: Vec::new(),
            pid: Some(100),
            locate_error: None,
            running_error: None,
        }),
    })
}

/// Служба депо над поддельным миром.
pub fn build_service(
    world: &Arc<World>,
    platform: hlsr_core::js::Platform,
    logger: Arc<hlsr_core::logger::MemoryLogger>,
) -> Arc<hlsr_platform::steam_depot::SteamDepotService> {
    let fake = Arc::new(FakeSteam(world.clone()));
    let clock_world = world.clone();
    Arc::new(hlsr_platform::steam_depot::SteamDepotService::new(
        hlsr_platform::steam_depot::DepotOptions {
            logger: Some(logger),
            locator: Some(Arc::new(FakeLocator(world.clone()))),
            run: Some(fake.clone()),
            sleep: Some(fake),
            now: Some(Arc::new(move || clock_world.state().now)),
            platform: Some(platform),
            ..hlsr_platform::steam_depot::DepotOptions::default()
        },
    ))
}
