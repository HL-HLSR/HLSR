//! Сверка запуска игр на Windows (`game-delivery/LaunchService.js` 0.4.7) по фикстуре
//! `tools/reference/platform/launch-windows.js`: те же сценарии над временной папкой с теми же
//! подделками доставки, службы процессов, Steam, депо, настроек, `spawn`, инспектора процессов,
//! DEP, маски ЦП, сессии патча RInput и реестра — итог, порядок действий (вызовы подделок, `spawn`
//! с аргументами, cwd и окружением), журнал, подсказка DEP, `describeAvailability` и деревья
//! файлов игры, журналов и данных лаунчера (оверлей).
//!
//! Фикстура снята на Windows (пути с `\`), поэтому и сверка — только на Windows.

#![cfg(windows)]

mod proton_support;
mod support;

use hlsr_core::js::{self, path::NodePath};
use hlsr_delivery::errors::{Code, DeliveryError, Failure, RawError};
use hlsr_delivery::manifests::{LaunchConfig, launch_config_of};
use hlsr_delivery::paths::LibraryPaths;
use hlsr_delivery::ports::{
    DepotProgressFn, Logger, Registry, RegistryHive, RegistryValue, SteamDepot,
};
use hlsr_delivery::providers::BoxFuture;
use hlsr_platform::dep_compat::Exemption;
use hlsr_platform::game_process_service::StartedProcess;
use hlsr_platform::launch_service::{
    CpuAffinity, DepCompat, LaunchDelivery, LaunchLocator, LaunchProcesses, LaunchRegistry,
    LaunchService, LaunchServiceOptions, LaunchSettings, LaunchSpawner, SpawnRequest,
};
use hlsr_platform::livesplit_overlay::{self, LiveSplitOverlay, OverlayOptions};
use hlsr_platform::process_inspect::{ProcessInfo, ProcessInspector};
use hlsr_platform::rinput_menu_fix::{MenuSession, PatchStatus};
use hlsr_platform::spawn::{Child, ChildControl, ChildController, Env};
use proton_support::{Root, tree_of, write_file};
use serde_json::{Map, Value, json};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use support::{Mismatches, fixture, items};

/// «Сейчас» сценария: момент запуска инжектора.
fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(1_792_152_000_123)
}

fn ms(value: &Value) -> Duration {
    Duration::from_millis(value.as_u64().unwrap())
}

/// Мир сценария: подделки пишут свои вызовы в общий список действий.
struct World {
    root: Root,
    scenario: Value,
    events: Mutex<Vec<Value>>,
    logs: Mutex<Vec<Value>>,
    settings: Mutex<Value>,
    steam: Mutex<Vec<Value>>,
    menu: Mutex<Vec<Value>>,
    polls: AtomicUsize,
    stamps: Mutex<Vec<(String, String)>>,
    controllers: Mutex<Vec<ChildController>>,
}

impl World {
    fn game(&self) -> String {
        self.root.at("game")
    }

    fn at(&self, relative: &str) -> String {
        NodePath::native().join(&[&self.game(), relative])
    }

    fn norm(&self, value: &Value) -> Value {
        self.root.norm(value)
    }

    fn event(&self, op: &str, fields: Value) {
        let mut object = Map::new();
        object.insert("op".to_owned(), json!(op));
        if let Value::Object(fields) = fields {
            object.extend(fields);
        }
        let value = self.norm(&Value::Object(object));
        self.events.lock().unwrap().push(value);
    }

    fn stamp(&self, value: &str) -> String {
        if let Some((_, label)) = self
            .stamps
            .lock()
            .unwrap()
            .iter()
            .find(|(at, _)| at == value)
        {
            return label.clone();
        }
        if value == "2026-10-16T12:00:01.250Z" {
            return value.to_owned();
        }
        "<now>".to_owned()
    }

    fn found(&self) -> Value {
        self.scenario["injector"]["found"].clone()
    }
}

struct WorldLogger(Arc<World>);

impl WorldLogger {
    fn push(&self, level: &str, message: &str) {
        let message = self.0.root.norm_text(message);
        self.0
            .logs
            .lock()
            .unwrap()
            .push(json!({"level": level, "message": message}));
    }
}

impl Logger for WorldLogger {
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

struct Delivery(Arc<World>);

impl LaunchProcesses for Delivery {
    fn is_running(&self, game_id: &str) -> bool {
        self.0.event("isRunning", json!({"gameId": game_id}));
        js::truthy(&self.0.scenario["running"])
    }
    fn get_process_state(&self, game_id: &str) -> Value {
        self.0.event("getProcessState", json!({"gameId": game_id}));
        json!({"state": "running", "pid": 1})
    }
    fn mark_starting(&self, game_id: &str) {
        self.0.event("markStarting", json!({"gameId": game_id}));
    }
    fn cancel_starting(&self, game_id: &str, reason: &str) {
        self.0.event(
            "cancelStarting",
            json!({"gameId": game_id, "reason": reason}),
        );
    }
    fn mark_exited(&self, _game_id: &str, _pid: u32) -> bool {
        unreachable!("Windows-запуск не зовёт markExited")
    }
}

impl LaunchDelivery for Delivery {
    fn is_busy(&self) -> bool {
        self.0.event("isBusy", json!({}));
        js::truthy(&self.0.scenario["busy"])
    }
    fn installed_of(&self, game_id: &str) -> Result<Value, DeliveryError> {
        self.0.event("installedOf", json!({"gameId": game_id}));
        let mut installed = Map::new();
        installed.insert("installPath".to_owned(), json!(self.0.game()));
        if let Some(extra) = self.0.scenario["installed"].as_object() {
            installed.extend(extra.clone());
        }
        Ok(Value::Object(installed))
    }
    fn register_started_process(
        &self,
        game_id: &str,
        info: StartedProcess,
    ) -> Result<Value, DeliveryError> {
        let started_at = info.started_at.as_deref().map(|at| self.0.stamp(at));
        self.0.event(
            "register",
            json!({"gameId": game_id, "info": {
                "pid": info.pid,
                "executablePath": info.executable_path,
                "startedAt": started_at,
                "child": info.child.is_some(),
            }}),
        );
        Ok(json!({"state": "running", "pid": info.pid}))
    }
}

struct Settings(Arc<World>);

impl LaunchSettings for Settings {
    fn get(&self, game_id: &str) -> Value {
        self.0.event("settings.get", json!({"gameId": game_id}));
        self.0.settings.lock().unwrap().clone()
    }
    fn save(&self, game_id: &str, settings: &Value) -> bool {
        self.0.event(
            "settings.save",
            json!({"gameId": game_id, "settings": settings}),
        );
        if js::truthy(&self.0.scenario["saveFails"]) {
            return false;
        }
        *self.0.settings.lock().unwrap() = settings.clone();
        true
    }
}

struct Locator(Arc<World>);

impl LaunchLocator for Locator {
    fn preflight(&self, app_id: u32) -> Result<Value, DeliveryError> {
        self.0.event("preflight", json!({"appId": app_id}));
        let preflight = &self.0.scenario["preflight"];
        if let Some(error) = preflight.get("error") {
            return Err(DeliveryError::new(
                Code::coerce(error["code"].as_str().unwrap()),
                error["message"].as_str().unwrap(),
            ));
        }
        Ok(preflight["ok"].clone())
    }
    fn describe_steam_only(&self) -> Value {
        self.0.event("describeSteamOnly", json!({}));
        let mut answers = self.0.steam.lock().unwrap();
        if answers.len() > 1 {
            answers.remove(0)
        } else {
            answers[0].clone()
        }
    }
    fn describe_availability(&self, app_id: u32) -> Value {
        self.0
            .event("describeAvailability", json!({"appId": app_id}));
        json!({"ok": true, "code": null, "appId": app_id})
    }
}

struct Depot(Arc<World>);

impl SteamDepot for Depot {
    fn assert_owns<'a>(&'a self, app_ids: &'a [u32]) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(async move {
            self.0.event("assertOwns", json!({"appIds": app_ids}));
            let depot = &self.0.scenario["depot"];
            if depot == "ok" {
                return Ok(());
            }
            Err(DeliveryError::new(
                Code::coerce(depot["code"].as_str().unwrap()),
                depot["message"].as_str().unwrap(),
            )
            .into())
        })
    }
    fn install<'a>(
        &'a self,
        _paths: &'a LibraryPaths,
        _game_id: &'a str,
        _on_progress: DepotProgressFn,
    ) -> BoxFuture<'a, Result<Value, Failure>> {
        Box::pin(async {
            Err(RawError::new("установка из депо не участвует").into())
        })
    }
}

struct NoSignals;

impl ChildControl for NoSignals {
    fn kill(&self, _signal: &str) -> bool {
        false
    }
    fn kill_group(&self, _signal: &str) -> bool {
        false
    }
}

struct Spawner(Arc<World>);

impl LaunchSpawner for Spawner {
    fn spawn(
        &self,
        file: &Path,
        args: &[String],
        request: SpawnRequest,
    ) -> Result<Child, RawError> {
        let world = &self.0;
        let file_text = file.to_string_lossy().into_owned();
        world.event(
            "spawn",
            json!({
                "file": file_text,
                "args": args,
                "cwd": request.cwd.to_string_lossy(),
                "detached": request.detached,
                "windowsHide": request.windows_hide,
                "env": request.env,
            }),
        );
        let name = NodePath::native().basename(&file_text);
        let scenario = &world.scenario;
        let (behaviour, pid) = match name.as_str() {
            "Injector.exe" => (scenario["injector"]["behaviour"].clone(), 3000),
            "RInput.exe" => (
                scenario.get("rinputBehaviour").cloned().unwrap_or_else(
                    || json!({"exit": scenario.get("rinputExit").cloned().unwrap_or(json!(0))}),
                ),
                3100,
            ),

            _ => (scenario["game"]["behaviour"].clone(), 5000),
        };
        if let Some(message) = behaviour.get("throw").and_then(Value::as_str) {
            return Err(RawError::new(message));
        }
        if let Some(message) = behaviour.get("error").and_then(Value::as_str) {
            return Ok(Child::failed(RawError::new(message)));
        }
        let (child, controller) = Child::new(Some(pid), Arc::new(NoSignals));
        controller.spawned();
        if let Some(code) = behaviour.get("exit").and_then(Value::as_i64) {
            controller.exit(Some(code as i32), None);
        }
        world.controllers.lock().unwrap().push(controller);
        Ok(child)
    }
}

struct Inspector(Arc<World>);

impl ProcessInspector for Inspector {
    fn is_alive(&self, pid: u32) -> bool {
        let world = &self.0;
        if world.scenario["game"]["alive"] == json!(false) {
            return false;
        }
        let found = world.found();
        let dead = found["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|entry| entry["pid"] == json!(pid) && js::truthy(&entry["dead"]));
        !dead
    }

    fn inspect(&self, _pid: u32) -> Option<ProcessInfo> {
        let inspect = self.0.scenario["game"]["inspect"].as_object()?;
        let executable = inspect["executable"].as_str().unwrap();
        Some(ProcessInfo {
            pid: 5000,
            executable_path: Some(self.0.at(executable)),
            name: Some(NodePath::native().basename(executable)),
            started_at: inspect["startedAt"].as_str().map(str::to_owned),
            source: None,
        })
    }

    fn find_by_image_name(&self, image_name: &str) -> Vec<ProcessInfo> {
        let world = &self.0;
        let polls = world.polls.fetch_add(1, Ordering::SeqCst) + 1;
        let found = world.found();
        let after = found["after"].as_u64().unwrap_or(99) as usize;
        if polls < after {
            return Vec::new();
        }
        let start = now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        items(&found["entries"])
            .iter()
            .map(|entry| {
                let started_at = entry["offset"].as_i64().map(|offset| {
                    let at = js::iso_string(
                        SystemTime::UNIX_EPOCH + Duration::from_millis((start + offset) as u64),
                    );
                    let label = if offset < 0 {
                        format!("spawn{offset}")
                    } else {
                        format!("spawn+{offset}")
                    };
                    world.stamps.lock().unwrap().push((at.clone(), label));
                    at
                });
                ProcessInfo {
                    pid: entry["pid"].as_u64().unwrap() as u32,
                    executable_path: Some(world.at(entry["executable"].as_str().unwrap())),
                    name: Some(image_name.to_owned()),
                    started_at,
                    source: None,
                }
            })
            .collect()
    }
}

struct Dep(Arc<World>);

impl DepCompat for Dep {
    fn ensure_exemption(&self, executable: &str, _logger: &dyn Logger) -> Exemption {
        self.0
            .event("dep.ensureExemption", json!({"executable": executable}));
        Exemption::Added
    }
    fn query_policy(&self) -> Option<i64> {
        self.0.event("dep.queryPolicy", json!({}));
        self.0.scenario["policy"].as_i64()
    }
}

struct Affinity(Arc<World>, u32);

impl CpuAffinity for Affinity {
    fn cpu_count(&self) -> u32 {
        self.1
    }
    fn set_affinity(&self, pid: u32, mask: u64) -> Result<(), String> {
        self.0
            .event("affinity", json!({"pid": pid, "mask": mask.to_string()}));
        Ok(())
    }
}

struct Session(Arc<World>);

impl MenuSession for Session {
    fn patch(&mut self, process_name: &str) -> Result<PatchStatus, String> {
        self.0
            .event("rinput.patch", json!({"processName": process_name}));
        let mut menu = self.0.menu.lock().unwrap();
        let next = if menu.is_empty() {
            json!("waiting")
        } else {
            menu.remove(0)
        };
        if let Some(error) = next.get("error").and_then(Value::as_str) {
            return Err(error.to_owned());
        }
        Ok(match next.as_str().unwrap() {
            "patched" => PatchStatus::Patched,
            "already" => PatchStatus::Already,
            "unsupported" => PatchStatus::Unsupported,
            _ => PatchStatus::Waiting,
        })
    }
    fn close(&mut self) {
        self.0.event("rinput.close", json!({}));
    }
}

/// Реестр сценария: язык Steam и язык движка Source.
struct FakeRegistry(Map<String, Value>);

impl Registry for FakeRegistry {
    fn read_value(&self, hive: RegistryHive, key: &str, name: &str) -> Option<RegistryValue> {
        let prefix = match hive {
            RegistryHive::CurrentUser => "HKCU",
            RegistryHive::LocalMachine => "HKLM",
        };
        let full = format!("{prefix}\\{key}").to_lowercase();
        let (_, values) = self
            .0
            .iter()
            .find(|(path, _)| path.to_lowercase() == full)?;
        let (_, value) = values
            .as_object()?
            .iter()
            .find(|(value_name, _)| value_name.to_lowercase() == name.to_lowercase())?;
        Some(RegistryValue::Sz(value.as_str()?.to_owned()))
    }
}

fn layout(root: &str, items: &Value) {
    for item in items.as_array().into_iter().flatten() {
        let path = NodePath::native();
        if let Some(dir) = item["dir"].as_str() {
            std::fs::create_dir_all(path.join(&[root, dir])).unwrap();
            continue;
        }
        let file = path.join(&[root, item["file"].as_str().unwrap()]);
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

fn leak_str(text: &str) -> &'static str {
    Box::leak(text.to_owned().into_boxed_str())
}

/// Настройки запуска сценария: `registry_config` — копия настроек `base` с правками.
fn launch_registry(scenario: &Value) -> LaunchRegistry {
    let Some(custom) = scenario["registry_config"].as_object() else {
        return Arc::new(launch_config_of);
    };
    let base = launch_config_of(custom["base"].as_str().unwrap()).unwrap();
    let mut config: LaunchConfig = base.clone();
    if let Some(executable) = custom.get("executable").and_then(Value::as_str) {
        config.executable = leak_str(executable);
    }
    if let Some(app_id) = custom.get("steamAppId").and_then(Value::as_u64) {
        config.steam_app_id = app_id as u32;
    }
    if let Some(required) = custom.get("sourceSdkRequired").and_then(Value::as_bool) {
        config.source_sdk_required = required;
    }
    if let Some(ids) = custom.get("requiresOwnedAppIds").and_then(Value::as_array) {
        let ids: Vec<u32> = ids.iter().map(|id| id.as_u64().unwrap() as u32).collect();
        config.requires_owned_app_ids = Box::leak(ids.into_boxed_slice());
    }
    let config: &'static LaunchConfig = Box::leak(Box::new(config));
    let game_id = scenario["gameId"].as_str().unwrap().to_owned();
    Arc::new(move |id: &str| {
        if id == game_id {
            Some(config)
        } else {
            launch_config_of(id)
        }
    })
}

fn result_value(world: &World, result: Result<Value, DeliveryError>) -> Value {
    match result {
        Ok(value) => json!({"ok": world.norm(&value)}),
        Err(error) => json!({"error": world.norm(&json!({
            "code": error.code.as_str(),
            "message": error.message,
            "details": error.details,
        }))}),
    }
}

async fn run(data: &Value, recorded: &Value) -> Value {
    let scenario = recorded["scenario"].clone();
    let root = Root::new("hlsr-launch-");
    let game = root.at("game");
    std::fs::create_dir_all(&game).unwrap();
    layout(&game, &scenario["layout"]);
    layout(&root.text, &scenario["rootLayout"]);
    let resources = Path::new(&root.text)
        .join("resources")
        .join(livesplit_overlay::RESOURCE_DIRECTORY);
    for name in livesplit_overlay::FILES {
        write_file(&resources.join(name), format!("{name} v1"), None);
    }

    let world = Arc::new(World {
        root,
        settings: Mutex::new(scenario["settings"].clone()),
        steam: Mutex::new(items(&scenario["steam"]).clone()),
        menu: Mutex::new(scenario["menu"].as_array().cloned().unwrap_or_default()),
        scenario: scenario.clone(),
        events: Mutex::new(Vec::new()),
        logs: Mutex::new(Vec::new()),
        polls: AtomicUsize::new(0),
        stamps: Mutex::new(Vec::new()),
        controllers: Mutex::new(Vec::new()),
    });

    let mut registry = data["steamLanguage"].as_object().unwrap().clone();
    if let Some(extra) = scenario["registry"].as_object() {
        registry.extend(extra.clone());
    }
    let env: Env = data["env"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap().to_owned()))
        .collect();
    let timing = &data["timing"];
    let delivery = Arc::new(Delivery(world.clone()));
    let extra_env: Option<Env> = scenario["extraEnv"].as_object().map(|extra| {
        extra
            .iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap().to_owned()))
            .collect()
    });

    let dependencies = scenario
        .get("dependencies")
        .filter(|value| !value.is_null())
        .cloned();
    let packs = scenario
        .get("packs")
        .filter(|value| !value.is_null())
        .cloned();
    let dep_world = world.clone();
    let pack_world = world.clone();
    let session_world = world.clone();
    let root_path = Path::new(&world.root.text).to_path_buf();
    let service = LaunchService::new(LaunchServiceOptions {
        delivery: Some(delivery.clone()),
        processes: Some(delivery),
        logger: Some(Arc::new(WorldLogger(world.clone()))),
        env: Some(env),
        inspector: Some(Arc::new(Inspector(world.clone()))),
        spawner: Some(Arc::new(Spawner(world.clone()))),
        launch_registry: Some(launch_registry(&scenario)),
        settings_store: Some(Arc::new(Settings(world.clone()))),
        logs_dir: js::truthy(&scenario["logsDir"]).then(|| root_path.join("logs")),
        dependency_updater: dependencies.map(|value| {
            let world = dep_world.clone();
            Arc::new(move |install: &Path, _config: &'static LaunchConfig| {
                world.event(
                    "dependencies",
                    json!({"installPath": install.to_string_lossy()}),
                );
                let value = value.clone();
                Box::pin(async move { value }) as BoxFuture<'static, Value>
            }) as _
        }),
        pack_applier: packs.map(|value| {
            let world = pack_world.clone();
            Arc::new(
                move |install: &Path, _config: &'static LaunchConfig, settings: &Value| {
                    world.event(
                        "packs",
                        json!({"installPath": install.to_string_lossy(), "settings": settings}),
                    );
                    let value = value.clone();
                    Box::pin(async move { value }) as BoxFuture<'static, Value>
                },
            ) as _
        }),
        extra_env_provider: extra_env.map(|env| Arc::new(move || Some(env.clone())) as _),
        launcher_version: Some("0.4.7".to_owned()),
        steam_locator: Some(Arc::new(Locator(world.clone()))),
        depot_service: (!scenario["depot"].is_null())
            .then(|| Arc::new(Depot(world.clone())) as Arc<dyn SteamDepot>),
        early_exit_window: Some(ms(&timing["earlyExitWindowMs"])),
        injected_game_timeout: Some(ms(scenario
            .get("injectedTimeoutMs")
            .unwrap_or(&timing["injectedGameTimeoutMs"]))),
        injected_game_poll: Some(ms(&timing["injectedGamePollMs"])),
        injector_exit_grace: None,
        rinput_menu_session: Some(Arc::new(move || {
            session_world.event("rinput.session", json!({}));
            Box::new(Session(session_world.clone())) as Box<dyn MenuSession>
        })),
        rinput_patch_wait: Some(ms(&timing["rinputPatchWaitMs"])),
        rinput_patch_poll: Some(ms(&timing["rinputPatchPollMs"])),
        live_split_overlay: Some(Arc::new(LiveSplitOverlay::new(OverlayOptions {
            base_dir: Some(root_path.join("data")),
            resources_path: Some(root_path.join("resources")),
            ..OverlayOptions::default()
        }))),
        dep_compat: Some(Arc::new(Dep(world.clone()))),
        platform: Some(js::Platform::Win32),
        registry: Some(Arc::new(FakeRegistry(registry))),
        cfg_search: None,
        affinity: Some(Arc::new(Affinity(
            world.clone(),
            data["cpuCount"].as_u64().unwrap() as u32,
        ))),
        now: Some(Arc::new(now)),
    });

    let game_id = scenario["gameId"].as_str().unwrap();
    let result = service.launch(game_id).await;
    let result = result_value(&world, result);
    service.settled().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let after = json!({
        "crashHint": [
            service.dep_crash_hint(None),
            service.dep_crash_hint(Some(-1_073_741_819)),
            service.dep_crash_hint(Some(1)),
        ],
        "availability": world.norm(&service.describe_availability(game_id)),
    });
    let roots = [world.root.text.as_str()];
    json!({
        "result": result,
        "events": world.events.lock().unwrap().clone(),
        "logs": world.logs.lock().unwrap().clone(),
        "after": after,
        "trees": {
            "game": tree_of(Path::new(&game), &roots),
            "logs": tree_of(&root_path.join("logs"), &roots),
            "data": tree_of(&root_path.join("data"), &roots),
        },
    })
}

/// Оверлей LiveSplit в 0.4.9 — прокси в папке игры до запуска, а не инжектор после него: запись
/// 0.4.7 сверяется без следов оверлея (запуск инжектора, его строки журнала, файлы в папке данных
/// и прокси в папке игры; 0.4.9 готовит папку данных и тогда, когда игра падает на запуске). Прокси при запуске проверяют самотесты `launch_windows_selftests`.
fn without_overlay(value: &Value) -> Value {
    let overlay_file = |path: &str| {
        path.starts_with("livesplit-overlay")
            || ["d3d9.dll", "opengl32.dll", "hlsr-overlay.path"]
                .iter()
                .any(|name| path == *name || path.ends_with(&format!("/{name}")))
    };
    let mut out = value.clone();
    if let Some(events) = out["events"].as_array_mut() {
        events.retain(|event| {
            !(event["op"] == "spawn"
                && event["file"]
                    .as_str()
                    .is_some_and(|file| file.contains("/livesplit-overlay/")))
        });
    }
    if let Some(logs) = out["logs"].as_array_mut() {
        logs.retain(|line| {
            !line["message"]
                .as_str()
                .is_some_and(|message| message.contains("оверлей LiveSplit"))
        });
    }
    for tree in ["game", "data"] {
        if let Some(entries) = out["trees"][tree].as_array_mut() {
            entries.retain(|entry| !entry["path"].as_str().is_some_and(overlay_file));
        }
    }
    // Папку данных создаёт только оверлей: без его файлов её как будто нет.
    if out["trees"]["data"].as_array().is_some_and(Vec::is_empty) {
        out["trees"]["data"] = Value::Null;
    }
    out
}

/// Конфиги на запуске в 0.4.9 — только подготовка файла игрока (`prepare_for_launch`), а не бинды,
/// стандартный cfg и перенос персонализации Ghosting в настройки 0.4.7: запись 0.4.7 сверяется без
/// cfg-файлов и `.hlsr` игры, без `settings.save` и без строк журнала о конфигах. Запуск 0.4.9 с
/// файлом игрока проверяют `episode_launch` и `launch_windows_selftests`, сам файл — фикстура
/// `single-config-049` доставки.
fn without_configs(value: &Value) -> Value {
    let config_file =
        |path: &str| path.ends_with(".cfg") || path == ".hlsr" || path.starts_with(".hlsr/");
    let mut out = value.clone();
    if let Some(events) = out["events"].as_array_mut() {
        events.retain(|event| event["op"] != "settings.save");
    }
    if let Some(logs) = out["logs"].as_array_mut() {
        logs.retain(|line| {
            !line["message"].as_str().is_some_and(|message| {
                [
                    "бинды не записаны",
                    "персонализация не применена",
                    "конфиг игрока не подготовлен",
                ]
                .iter()
                .any(|text| message.contains(text))
            })
        });
    }
    if let Some(entries) = out["trees"]["game"].as_array_mut() {
        entries.retain(|entry| !entry["path"].as_str().is_some_and(config_file));
        // Папки, которые остались без содержимого после фильтра (их создавала запись cfg).
        let paths: Vec<String> = entries
            .iter()
            .filter_map(|entry| entry["path"].as_str().map(str::to_owned))
            .collect();
        entries.retain(|entry| {
            entry["kind"] != "dir"
                || paths
                    .iter()
                    .any(|path| path.starts_with(&format!("{}/", entry["path"].as_str().unwrap())))
        });
    }
    out
}

#[tokio::test]
async fn windows_launch_scenarios_match_0_4_7() {
    let data = fixture("launch-windows.json");
    let mut mismatches = Mismatches::default();
    for recorded in items(&data["scenarios"]) {
        let name = recorded["scenario"]["name"].as_str().unwrap();
        let actual = without_configs(&without_overlay(&run(&data, recorded).await));
        let recorded = without_configs(&without_overlay(recorded));
        for key in ["result", "events", "logs", "after", "trees"] {
            mismatches.check(&actual[key], &recorded[key], format!("{name}: {key}"));
        }
    }
    mismatches.finish("запуск на Windows");
}
