//! Общее рендера игр против `game-render.js` 0.4.9 (`tests/fixtures/game-render.json`,
//! `tools/reference/demo/game-render.js`): те же сценарии с подставными реестром, Store,
//! помощником и списком процессов. Восстановление после сбоя идёт по маркерам, записанным
//! самим 0.4.9. Дальше — настоящий реестр в личном кусте (снимок и возврат, снимок `reg.exe`
//! 0.4.9), папки рендера со ссылками и рендер по одному.

use hlsr_core::logger::MemoryLogger;
use hlsr_core::store::Store;
use hlsr_demo::errors::DemoError;
use hlsr_demo::game_render::{
    self, Active, GameRender, Inspector, Launch, Ports, Registry, Spec, SteamStatus,
};
use hlsr_demo::render_host::{Host, HostEvent, StartRequest};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const GHOSTING: Spec = Spec {
    game_id: "ghosting-renewed",
    image: "hl2.exe",
    registry_key: "HKCU\\Software\\Valve\\Source\\ghostingmod\\Settings",
    marker: "ghosting-render.restore.json",
    reg_file: "ghosting-render.settings.reg",
    unmute_pending: "ghostingUnmutePending",
    label: "Ghosting",
    other_running: None,
    required: &["hl2.exe", "ghostingmod"],
    render_root: |install| install.join("ghostingmod").join("hlsr-render"),
    caps: &["keyhud", "audio", "integrity", "ownConfig", "ownHud"],
};

const HL1: Spec = Spec {
    game_id: "hl1-won",
    image: "hl.exe",
    registry_key: "HKCU\\Software\\Valve\\Half-Life\\Settings",
    marker: "hl1-render.restore.json",
    reg_file: "hl1-render.settings.reg",
    unmute_pending: "hl1UnmutePending",
    label: "HL1",
    other_running: Some("GOLDSRC_RUNNING"),
    required: &["Half-Life/hl.exe", "Bunnymod XT/Injector.exe"],
    render_root: |install| {
        install
            .join("Half-Life")
            .join("valve_WON")
            .join("hlsr_render")
    },
    caps: &["keyhud", "audio", "hud", "ownHud"],
};

fn spec(name: &str) -> &'static Spec {
    match name {
        "ghosting" => &GHOSTING,
        "hl1" => &HL1,
        other => panic!("нет игры {other}"),
    }
}

fn fixture() -> Value {
    let text = include_str!("fixtures/game-render.json");
    serde_json::from_str(text).unwrap()
}

// ---------------------------------------------------------------- подставное

/// Реестр: ключ → объект значений, снимок — JSON (как `fakeRegistry` сценария).
struct FakeRegistry {
    keys: Mutex<Map<String, Value>>,
    calls: Mutex<Vec<String>>,
    fail_import: bool,
    fail_exists: bool,
}

impl FakeRegistry {
    fn new(key: &str, initial: &Value, fail_import: bool, fail_exists: bool) -> Arc<FakeRegistry> {
        let mut keys = Map::new();
        if !initial.is_null() {
            keys.insert(key.to_owned(), initial.clone());
        }
        Arc::new(FakeRegistry {
            keys: Mutex::new(keys),
            calls: Mutex::new(Vec::new()),
            fail_import,
            fail_exists,
        })
    }

    fn call(&self, text: String) {
        self.calls.lock().unwrap().push(text);
    }

    fn calls(&self) -> Value {
        json!(*self.calls.lock().unwrap())
    }

    fn key(&self, key: &str) -> Value {
        self.keys
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn set(&self, key: &str, value: Value) {
        self.keys.lock().unwrap().insert(key.to_owned(), value);
    }
}

fn base(file: &Path) -> String {
    file.file_name().unwrap().to_string_lossy().into_owned()
}

struct SharedRegistry(Arc<FakeRegistry>);

impl Registry for SharedRegistry {
    fn exists(&self, key: &str) -> Result<bool, DemoError> {
        self.0.call(format!("exists {key}"));
        if self.0.fail_exists {
            return Err(failed("REGISTRY_FAILED", "reg query"));
        }
        Ok(self.0.keys.lock().unwrap().contains_key(key))
    }

    fn export_key(&self, key: &str, file: &Path) -> Result<(), DemoError> {
        self.0.call(format!("export {key} {}", base(file)));
        fs::write(file, self.0.key(key).to_string()).unwrap();
        Ok(())
    }

    fn delete_key(&self, key: &str) -> Result<(), DemoError> {
        self.0.call(format!("delete {key}"));
        self.0.keys.lock().unwrap().remove(key);
        Ok(())
    }

    fn import_file(&self, key: &str, file: &Path) -> Result<(), DemoError> {
        self.0.call(format!("import {}", base(file)));
        if self.0.fail_import {
            return Err(failed("REGISTRY_FAILED", "reg import"));
        }
        let value = serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap();
        self.0.set(key, value);
        Ok(())
    }
}

fn failed(code: &str, message: &str) -> DemoError {
    DemoError {
        message: message.into(),
        ..DemoError::new(code)
    }
}

/// Список процессов: только для образа игры (`only_image`) или для любого.
struct FakeInspector {
    image: Option<&'static str>,
    running: Vec<Option<String>>,
    fails: bool,
}

impl Inspector for FakeInspector {
    fn find_by_image_name(&self, image: &str) -> Result<Vec<Option<String>>, String> {
        if self.fails {
            return Err("toolhelp".into());
        }
        Ok(if self.image.is_none_or(|own| own == image) {
            self.running.clone()
        } else {
            Vec::new()
        })
    }

    fn same_path(&self, left: &str, right: &str) -> bool {
        left.to_lowercase() == right.to_lowercase()
    }
}

/// Помощник по сценарию: события после start и после stop (`FakeHost` сценария).
struct FakeHost {
    fail_start: bool,
    start_events: Vec<HostEvent>,
    stop_events: Vec<HostEvent>,
    record: Arc<Mutex<Vec<Value>>>,
    root: PathBuf,
    queue: Vec<HostEvent>,
    stopped: bool,
    pending: Option<bool>,
    audio_reset: Option<bool>,
}

impl Host for FakeHost {
    fn start(&mut self, request: &StartRequest) -> Result<u32, DemoError> {
        let path = |p: &Path| rel(&self.root, &p.to_string_lossy());
        self.record.lock().unwrap().push(json!({"start": {
            "resetOnly": request.reset_only,
            "exe": path(&request.exe),
            "cwd": path(&request.cwd),
            "commandLine": rel(&self.root, &request.command_line),
            "resetExe": request.reset_exe.as_deref().map(path),
            "resetCommandLine": rel(&self.root, &request.reset_command_line),
            "mute": request.mute,
        }}));
        if self.fail_start {
            return Err(failed("RENDER_HOST_FAILED", "Помощник вышел"));
        }
        self.queue.extend(self.start_events.iter().cloned());
        Ok(4242)
    }

    fn events(&mut self) -> Vec<HostEvent> {
        std::mem::take(&mut self.queue)
    }

    fn stop(&mut self, reset: bool) {
        if std::mem::replace(&mut self.stopped, true) {
            return;
        }
        self.record.lock().unwrap().push(json!({"stop": reset}));
        for event in self.stop_events.clone() {
            match event {
                HostEvent::Stopped { pending, .. } => self.pending = Some(pending),
                HostEvent::AudioReset { ok } => self.audio_reset = Some(ok),
                _ => {}
            }
            self.queue.push(event);
        }
    }

    fn exit_code(&self) -> Option<u32> {
        None
    }

    fn closed(&self) -> bool {
        self.stopped
    }

    fn pending(&self) -> Option<bool> {
        self.pending
    }

    fn audio_reset(&self) -> Option<bool> {
        self.audio_reset
    }
}

fn events(list: &Value) -> Vec<HostEvent> {
    list.as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|e| match e["event"].as_str().unwrap() {
            "audio" => HostEvent::Muted {
                pid: e["pid"].as_u64().unwrap() as u32,
                device: e["device"].as_u64().unwrap() as u32,
                was: e["was"].as_bool().unwrap(),
            },
            "audio-reset" => HostEvent::AudioReset {
                ok: e["ok"].as_bool().unwrap(),
            },
            "stopped" => HostEvent::Stopped {
                code: e["code"].as_u64().map(|c| c as u32),
                alone: e["alone"].as_bool().unwrap(),
                pending: e["pending"].as_bool().unwrap(),
            },
            other => panic!("событие {other}"),
        })
        .collect()
}

/// Строки с путём сценария — от `<scenario>`, «/» (в сценарии 0.4.9 — `<root>/sN`).
fn rel(root: &Path, text: &str) -> String {
    let root = root.to_string_lossy();
    if text.contains(root.as_ref()) {
        text.replace(root.as_ref(), "<scenario>").replace('\\', "/")
    } else {
        text.to_owned()
    }
}

fn expected_rel(value: &Value) -> Value {
    match value {
        Value::String(text) => {
            let Some(at) = text.find("<root>/s") else {
                return value.clone();
            };
            let digits = text[at + 8..]
                .find(|c: char| !c.is_ascii_digit())
                .map_or(text.len(), |n| at + 8 + n);
            Value::String(format!("{}<scenario>{}", &text[..at], &text[digits..]))
        }
        Value::Array(items) => Value::Array(items.iter().map(expected_rel).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), expected_rel(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `listing(dir)` сценария: имена по порядку, папки с «/», вглубь; нет папки — `null`.
fn listing(dir: &Path) -> Value {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for name in names {
            let full = dir.join(&name);
            if fs::symlink_metadata(&full).unwrap().is_dir() {
                out.push(format!("{prefix}{name}/"));
                walk(&full, &format!("{prefix}{name}/"), out);
            } else {
                out.push(format!("{prefix}{name}"));
            }
        }
    }
    if !dir.exists() {
        return Value::Null;
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out);
    json!(out)
}

fn read_text(file: &Path) -> Value {
    fs::read_to_string(file).map_or(Value::Null, Value::String)
}

/// Маркер, записанный сейчас: время снимка — как в сценарии.
fn fixed_at(text: Value, now: &str, keep: &str) -> Value {
    let Value::String(text) = text else {
        return text;
    };
    let Some(at) = text.find("\"at\": \"") else {
        return Value::String(text);
    };
    let start = at + 7;
    let end = start + text[start..].find('"').unwrap();
    if &text[start..end] == keep {
        return Value::String(text);
    }
    Value::String(format!("{}{now}{}", &text[..start], &text[end..]))
}

const CRASH_AT: &str = "2026-11-11T10:00:00.000Z";

struct Setup {
    windows: bool,
    jobs_root: Option<PathBuf>,
    store: Option<Arc<Mutex<Store>>>,
    registry: Arc<FakeRegistry>,
    inspector: FakeInspector,
    install: Option<PathBuf>,
    game_running: bool,
    game_busy: bool,
    steam: SteamStatus,
    screen: Option<(u32, u32)>,
    host: Option<Box<dyn Fn() -> FakeHost + Send + Sync>>,
}

impl Setup {
    fn new(registry: Arc<FakeRegistry>) -> Setup {
        Setup {
            windows: true,
            jobs_root: None,
            store: None,
            registry,
            inspector: FakeInspector {
                image: None,
                running: Vec::new(),
                fails: false,
            },
            install: None,
            game_running: false,
            game_busy: false,
            steam: SteamStatus {
                ok: true,
                code: None,
            },
            screen: Some((1920, 1080)),
            host: None,
        }
    }

    fn build(self, spec: &Spec) -> (GameRender, Arc<MemoryLogger>) {
        let logger = Arc::new(MemoryLogger::default());
        let game_id = spec.game_id;
        let (running, busy) = (self.game_running, self.game_busy);
        let host = self.host;
        let ports = Ports {
            windows: self.windows,
            jobs_root: self.jobs_root,
            store: self.store,
            logger: logger.clone(),
            registry: Box::new(SharedRegistry(self.registry)),
            inspector: Box::new(self.inspector),
            install_path: Box::new(move || self.install.clone()),
            game_running: Box::new(move |id| running && id == game_id),
            game_busy: Box::new(move || busy),
            steam_status: Box::new(move || self.steam.clone()),
            screen_size: Box::new(move || self.screen),
            host_factory: Box::new(move || match &host {
                Some(make) => Box::new(make()) as Box<dyn Host>,
                None => panic!("помощник не ожидался"),
            }),
        };
        (GameRender::new(spec.clone(), ports), logger)
    }
}

fn store_in(dir: &Path, flag: Option<(&str, Value)>) -> Arc<Mutex<Store>> {
    let mut store = Store::open(dir.join("demo-tool.json"), json!({}));
    if let Some((key, value)) = flag {
        store.set(key, value);
    }
    Arc::new(Mutex::new(store))
}

fn flag(store: &Arc<Mutex<Store>>, key: &str) -> Value {
    store
        .lock()
        .unwrap()
        .get(key)
        .cloned()
        .unwrap_or(Value::Null)
}

fn logs(logger: &MemoryLogger) -> Value {
    json!(
        logger
            .take()
            .into_iter()
            .map(|(level, message)| json!([level, message]))
            .collect::<Vec<_>>()
    )
}

fn caught(result: Result<Value, DemoError>) -> Value {
    match result {
        Ok(out) => json!({"out": out}),
        Err(error) => json!({"error": {"code": error.code, "message": error.message}}),
    }
}

fn marker_value(marker: Option<Map<String, Value>>) -> Value {
    marker.map_or(Value::Null, Value::Object)
}

fn object(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// Сценарии по порядку: свои временные папки, как `scenarioDir()`.
struct Dirs {
    _temp: tempfile::TempDir,
    root: PathBuf,
    next: usize,
}

impl Dirs {
    fn new() -> Dirs {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        Dirs {
            _temp: temp,
            root,
            next: 0,
        }
    }

    fn scenario(&mut self) -> PathBuf {
        self.next += 1;
        let dir = self.root.join(format!("s{}", self.next));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn stores(&self) -> PathBuf {
        let dir = self.root.join("stores").join(self.next.to_string());
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}

fn install_ghosting(dir: &Path) -> PathBuf {
    let install = dir.join("game");
    fs::create_dir_all(install.join("ghostingmod")).unwrap();
    fs::write(install.join("hl2.exe"), "exe").unwrap();
    install
}

// ---------------------------------------------------------------- сверка с 0.4.9

#[test]
fn specs_and_fps_match_049() {
    let fixture = fixture();
    assert_eq!(fixture["fps"], json!(game_render::FPS));
    for (name, spec) in [("ghosting", &GHOSTING), ("hl1", &HL1)] {
        let render_root = (spec.render_root)(Path::new("I"));
        let view = json!({
            "gameId": spec.game_id, "image": spec.image, "registryKey": spec.registry_key,
            "marker": spec.marker, "regFile": spec.reg_file, "unmutePending": spec.unmute_pending,
            "label": spec.label, "otherRunning": spec.other_running,
            "required": spec.required,
            "renderRoot": render_root.strip_prefix("I").unwrap().to_string_lossy().replace('\\', "/"),
            "caps": spec.caps,
        });
        assert_eq!(view, fixture["specs"][name], "{name}");
    }
}

#[test]
fn snapshot_matches_049() {
    let fixture = fixture();
    let now = fixture["now"].as_str().unwrap();
    let mut dirs = Dirs::new();
    for case in fixture["snapshot"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let spec = spec(case["spec"].as_str().unwrap());
        let root = dirs.scenario();
        let registry = FakeRegistry::new(
            spec.registry_key,
            &case["initial"],
            false,
            case["failExists"].as_bool().unwrap(),
        );
        if case["staleReg"].as_bool().unwrap() {
            fs::write(root.join(spec.reg_file), "old").unwrap();
        }
        if case["markerBefore"].as_bool().unwrap() {
            let marker =
                object(&json!({"version": 1, "existed": true, "at": CRASH_AT, "muted": false}));
            game_render::write_marker(&root, spec, &marker).unwrap();
        }
        let mut setup = Setup::new(registry.clone());
        setup.jobs_root = Some(root.clone());
        let (render, _) = setup.build(spec);
        let muted = case["muted"].as_bool().unwrap();
        let mut registry_taken = false;
        let result = render
            .render(&|| false, |active| {
                active.muted = muted;
                let result = render.snapshot(active, object(&case["extra"]));
                registry_taken = active.registry;
                // Снимок остаётся для сверки: конец задания его не возвращает.
                active.registry = false;
                Ok(result)
            })
            .unwrap();
        assert_eq!(
            caught(result.map(|()| Value::Null)),
            case["result"],
            "{name}"
        );
        assert_eq!(json!(registry_taken), case["registry"], "{name}");
        assert_eq!(registry.calls(), case["calls"], "{name}");
        let marker = fixed_at(read_text(&root.join(spec.marker)), now, CRASH_AT);
        assert_eq!(marker, case["marker"], "{name}");
        assert_eq!(listing(&root), case["files"], "{name}");
    }
}

#[test]
fn recover_after_a_049_crash_matches_049() {
    let fixture = fixture();
    let mut dirs = Dirs::new();
    for case in fixture["recover"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let spec = spec(case["spec"].as_str().unwrap());
        let root = dirs.scenario();
        let registry = FakeRegistry::new(
            spec.registry_key,
            &case["initial"],
            case["failImport"].as_bool().unwrap(),
            false,
        );
        // Маркер и снимок — байты, которые оставил упавший 0.4.9.
        if let Some(text) = case["markerText"].as_str() {
            fs::write(root.join(spec.marker), text).unwrap();
        }
        if let Some(text) = case["regText"].as_str() {
            fs::write(root.join(spec.reg_file), text).unwrap();
        }
        let store = store_in(&dirs.stores(), None);
        let mut setup = Setup::new(registry.clone());
        setup.jobs_root = (!case["noRoot"].as_bool().unwrap()).then(|| root.clone());
        setup.store = Some(store.clone());
        let (render, logger) = setup.build(spec);
        let result = render.recover();
        assert_eq!(
            json!({"out": marker_value(result)}),
            case["result"],
            "{name}"
        );
        assert_eq!(registry.calls(), case["calls"], "{name}");
        assert_eq!(registry.key(spec.registry_key), case["key"], "{name}");
        assert_eq!(flag(&store, spec.unmute_pending), case["flag"], "{name}");
        assert_eq!(logs(&logger), case["logs"], "{name}");
        assert_eq!(listing(&root), case["files"], "{name}");
    }
}

#[test]
fn finish_matches_049() {
    let fixture = fixture();
    let player = json!({"ScreenWidth": 2560, "ScreenHeight": 1440, "ScreenWindowed": 0});
    let render_values =
        json!({"ScreenWidth": 1280, "ScreenHeight": 720, "ScreenWindowed": 1, "Extra": 5});
    let mut dirs = Dirs::new();
    for case in fixture["finish"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let spec = &GHOSTING;
        let root = dirs.scenario();
        let jobs_root = root.join("jobs");
        let install = root.join("game");
        fs::create_dir(&jobs_root).unwrap();
        let registry = FakeRegistry::new(
            spec.registry_key,
            &player,
            case["failImport"].as_bool().unwrap(),
            false,
        );
        let before = &case["flagBefore"];
        let store = store_in(
            &dirs.stores(),
            (!before.is_null()).then(|| (spec.unmute_pending, before.clone())),
        );
        let mut setup = Setup::new(registry.clone());
        setup.jobs_root = Some(jobs_root.clone());
        setup.store = Some(store.clone());
        let (render, logger) = setup.build(spec);
        let (muted, pending_before) = (
            case["muted"].as_bool().unwrap(),
            case["pendingBefore"].as_bool().unwrap(),
        );
        // finish вызывает сам рендер — после задания; оно задаёт состояние, как сценарий.
        render
            .render(&|| false, |active| {
                active.muted = muted;
                active.pending_before = pending_before;
                if case["registry"].as_bool().unwrap() {
                    render.snapshot(active, Map::new())?;
                    registry.set(spec.registry_key, render_values.clone());
                }
                if case["modJobDir"].as_bool().unwrap() {
                    let dir =
                        game_render::make_render_dir(&(spec.render_root)(&install), "abcd1234")?;
                    fs::write(dir.join("f_0000.tga"), "x").unwrap();
                    active.mod_job_dir = Some(dir);
                }
                registry.calls.lock().unwrap().clear();
                Ok(())
            })
            .unwrap();
        // Состояние после finish (`after`) видно по реестру, маркеру и папкам.
        assert_eq!(flag(&store, spec.unmute_pending), case["flag"], "{name}");
        assert_eq!(registry.calls(), case["calls"], "{name}");
        assert_eq!(registry.key(spec.registry_key), case["key"], "{name}");
        assert_eq!(logs(&logger), case["logs"], "{name}");
        assert_eq!(listing(&jobs_root), case["jobs"], "{name}");
        assert_eq!(listing(&install), case["game"], "{name}");
    }
}

#[test]
fn stop_host_and_audio_events_match_049() {
    let fixture = fixture();
    let player = json!({"ScreenWidth": 2560, "ScreenHeight": 1440, "ScreenWindowed": 0});
    let mut dirs = Dirs::new();
    for case in fixture["stopHost"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let spec = &GHOSTING;
        let jobs_root = dirs.scenario();
        let record = Arc::new(Mutex::new(Vec::new()));
        let mut setup = Setup::new(FakeRegistry::new(spec.registry_key, &player, false, false));
        setup.jobs_root = Some(jobs_root.clone());
        setup.steam = serde_steam(&case["steam"]);
        let (start, stop) = (events(&case["startEvents"]), events(&case["stopEvents"]));
        let host_record = record.clone();
        let host_root = jobs_root.clone();
        setup.host = Some(Box::new(move || FakeHost {
            fail_start: false,
            start_events: start.clone(),
            stop_events: stop.clone(),
            record: host_record.clone(),
            root: host_root.clone(),
            queue: Vec::new(),
            stopped: false,
            pending: None,
            audio_reset: None,
        }));
        let (render, _) = setup.build(spec);
        let init = case["initMuted"].as_bool().unwrap();
        let mut seen = (Value::Null, Value::Null, Value::Null);
        render
            .render(&|| false, |active| {
                active.muted = init;
                active.pending_before = init;
                render.snapshot(active, Map::new())?;
                active.registry = false;
                let request = StartRequest {
                    exe: "hl2.exe".into(),
                    ..StartRequest::default()
                };
                render.start_host(active, &request, case["unmuteAll"].as_bool().unwrap())?;
                render.poll(active);
                let after_start = active.muted;
                render.stop_host(active, case["reset"].as_bool().unwrap());
                let marker = game_render::read_marker(&jobs_root, spec);
                seen = (
                    json!(after_start),
                    json!(active.muted),
                    marker.map_or(Value::Null, |m| m["muted"].clone()),
                );
                assert!(active.host.is_none(), "{name}: помощник снят");
                // Конец задания сверяется в finish_matches_049.
                active.muted = false;
                active.pending_before = false;
                Ok(())
            })
            .unwrap();
        let stops: Vec<Value> = record
            .lock()
            .unwrap()
            .iter()
            .filter_map(|entry| entry.get("stop").cloned())
            .collect();
        assert_eq!(json!(stops), case["stop"], "{name}");
        assert_eq!(seen.0, case["afterStart"], "{name}");
        assert_eq!(seen.1, case["muted"], "{name}");
        assert_eq!(seen.2, case["markerMuted"], "{name}");
    }
}

fn serde_steam(value: &Value) -> SteamStatus {
    SteamStatus {
        ok: value["ok"].as_bool().unwrap_or(false),
        code: value["code"].as_str().map(str::to_owned),
    }
}

fn running(value: &Value) -> Vec<Option<String>> {
    value
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|item| item["executablePath"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn mute_reset_blocker_matches_049() {
    let fixture = fixture();
    let mut dirs = Dirs::new();
    for case in fixture["blocker"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let spec = &GHOSTING;
        let root = dirs.scenario();
        let install = install_ghosting(&root);
        let flag_value = match &case["flag"] {
            Value::Null if case.get("flag").is_some() => None,
            Value::Null => Some(Value::Bool(true)),
            other => Some(other.clone()),
        };
        let store = store_in(&dirs.stores(), flag_value.map(|v| (spec.unmute_pending, v)));
        let mut setup = Setup::new(FakeRegistry::new(
            spec.registry_key,
            &Value::Null,
            false,
            false,
        ));
        setup.windows = case["platform"].as_str().is_none_or(|p| p == "win32");
        setup.jobs_root = Some(root.clone());
        setup.store = Some(store);
        setup.install = (case["install"] != json!(false)).then(|| install.clone());
        setup.game_running = case["gameRunning"] == json!(true);
        setup.game_busy = case["gameBusy"] == json!(true);
        setup.inspector = FakeInspector {
            image: None,
            running: running(&case["running"]),
            fails: case["inspectorThrows"] == json!(true),
        };
        setup.steam = if let Some(thrown) = case.get("steamThrows") {
            SteamStatus {
                ok: false,
                code: thrown.as_str().map(str::to_owned),
            }
        } else if case["steam"].is_object() {
            serde_steam(&case["steam"])
        } else {
            SteamStatus {
                ok: true,
                code: None,
            }
        };
        let (render, _) = setup.build(spec);
        let blocker = if case["active"] == json!(true) {
            render
                .render(&|| false, |_| Ok(render.mute_reset_blocker()))
                .unwrap()
        } else {
            render.mute_reset_blocker()
        };
        assert_eq!(json!(blocker), case["blocker"], "{name}");
    }
}

#[test]
fn reset_mute_without_a_render_matches_049() {
    let fixture = fixture();
    let player = json!({"ScreenWidth": 2560, "ScreenHeight": 1440, "ScreenWindowed": 0});
    let mut dirs = Dirs::new();
    for case in fixture["resetMute"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let spec = &GHOSTING;
        let root = dirs.scenario();
        let jobs_root = root.join("jobs");
        let install = install_ghosting(&root);
        let registry = FakeRegistry::new(
            spec.registry_key,
            &player,
            case["failImport"].as_bool().unwrap(),
            false,
        );
        if case["markerBefore"].as_bool().unwrap() {
            fs::create_dir(&jobs_root).unwrap();
            let marker =
                object(&json!({"version": 1, "existed": true, "at": CRASH_AT, "muted": true}));
            game_render::write_marker(&jobs_root, spec, &marker).unwrap();
            fs::write(jobs_root.join(spec.reg_file), player.to_string()).unwrap();
        }
        let store = store_in(
            &dirs.stores(),
            Some((spec.unmute_pending, case["flagBefore"].clone())),
        );
        let record = Arc::new(Mutex::new(Vec::new()));
        let mut setup = Setup::new(registry.clone());
        setup.jobs_root = Some(jobs_root.clone());
        setup.store = Some(store.clone());
        setup.install = Some(install.clone());
        let fail_start = case["failStart"].as_bool().unwrap();
        let stop = events(&case["stopEvents"]);
        let (host_record, host_root) = (record.clone(), root.clone());
        setup.host = Some(Box::new(move || FakeHost {
            fail_start,
            start_events: Vec::new(),
            stop_events: stop.clone(),
            record: host_record.clone(),
            root: host_root.clone(),
            queue: Vec::new(),
            stopped: false,
            pending: None,
            audio_reset: None,
        }));
        let (render, logger) = setup.build(spec);
        let launch = |dir: &Path, active: &mut Active| -> Result<Launch, DemoError> {
            active.mod_job_dir = Some(game_render::make_render_dir(
                &(spec.render_root)(dir),
                "reset01",
            )?);
            let exe = dir.join("hl2.exe");
            Ok(Launch {
                command_line: format!("\"{}\" -game ghostingmod +volume 0", exe.display()),
                exe,
                cwd: dir.to_path_buf(),
            })
        };
        let result = render.reset_mute(&launch);
        assert_eq!(json!(result), case["result"], "{name}");
        assert_eq!(
            json!(*record.lock().unwrap()),
            expected_rel(&case["record"]),
            "{name}"
        );
        assert_eq!(registry.calls(), case["calls"], "{name}");
        assert_eq!(flag(&store, spec.unmute_pending), case["flag"], "{name}");
        assert_eq!(json!(render.is_active()), case["active"], "{name}");
        assert_eq!(logs(&logger), case["logs"], "{name}");
        assert_eq!(listing(&jobs_root), case["jobs"], "{name}");
        assert_eq!(listing(&install), case["game"], "{name}");
    }
}

#[test]
fn preflight_matches_049() {
    let fixture = fixture();
    for case in fixture["preflight"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let spec = spec(case["spec"].as_str().unwrap());
        let mut setup = Setup::new(FakeRegistry::new(
            spec.registry_key,
            &Value::Null,
            false,
            false,
        ));
        setup.game_running = case["gameRunning"] == json!(true);
        setup.game_busy = case["gameBusy"] == json!(true);
        setup.inspector = FakeInspector {
            image: Some(spec.image),
            running: running(&case["running"]),
            fails: case["inspectorThrows"] == json!(true),
        };
        if case["steam"].is_object() {
            setup.steam = serde_steam(&case["steam"]);
        }
        let (render, _) = setup.build(spec);
        let exe = case["exe"].as_str().unwrap();
        let result = caught(render.preflight(Path::new(exe)).map(|()| Value::Null));
        let expected = match case.get("error") {
            Some(error) => json!({"error": error}),
            // Строже 0.4.9: список процессов не получен — отказ, как у mute_reset_blocker.
            None if case["inspectorThrows"] == json!(true) => {
                json!({"error": {"code": "GAME_RUNNING", "message": "GAME_RUNNING"}})
            }
            None => json!({"out": case["out"]}),
        };
        assert_eq!(result, expected, "{name}");
    }
}

#[test]
fn capabilities_match_049() {
    let fixture = fixture();
    for case in fixture["capabilities"].as_array().unwrap() {
        let spec = spec(case["spec"].as_str().unwrap());
        let screen = case["screen"].as_object().map(|s| {
            (
                s["width"].as_u64().unwrap() as u32,
                s["height"].as_u64().unwrap() as u32,
            )
        });
        let mut setup = Setup::new(FakeRegistry::new(
            spec.registry_key,
            &Value::Null,
            false,
            false,
        ));
        setup.screen = screen;
        let (render, _) = setup.build(spec);
        let caps = render.capabilities();
        let mut view = Map::new();
        view.insert("maxHeight".into(), json!(caps.max_height));
        view.insert("fps".into(), json!(caps.fps));
        for name in spec.caps {
            view.insert((*name).into(), json!(true));
        }
        assert_eq!(
            Value::Object(view),
            case["capabilities"],
            "{:?}",
            case["screen"]
        );
    }
}

// ---------------------------------------------------------------- рендер по одному

#[test]
fn render_is_one_at_a_time_and_maps_errors_after_cancel() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path(), Some(("ghostingUnmutePending", json!(true))));
    let mut setup = Setup::new(FakeRegistry::new(
        GHOSTING.registry_key,
        &Value::Null,
        false,
        false,
    ));
    setup.store = Some(store.clone());
    let (render, _) = setup.build(&GHOSTING);
    // Флаг прошлого раза: рендер начинается заглушённым.
    let inner = render
        .render(&|| false, |active| {
            assert!(active.muted && active.pending_before);
            assert!(render.is_active());
            // Второй рендер рядом — JOB_BUSY.
            let busy = render.render(&|| false, |_| Ok(()));
            active.muted = false;
            Ok(busy.unwrap_err().code)
        })
        .unwrap();
    assert_eq!(inner, "JOB_BUSY");
    assert!(!render.is_active());
    // mute снят подтверждённым сбросом — флаг снимается.
    assert_eq!(flag(&store, "ghostingUnmutePending"), json!(false));
    // После отмены любая ошибка задания — CANCELLED.
    let error = render
        .render(&|| true, |_| Err::<(), _>(DemoError::new("GAME_EXITED")))
        .unwrap_err();
    assert_eq!(error.code, "CANCELLED");
    let error = render
        .render(&|| false, |_| Err::<(), _>(DemoError::new("GAME_EXITED")))
        .unwrap_err();
    assert_eq!(error.code, "GAME_EXITED");
}

#[test]
fn dispose_cancels_the_render_and_waits_for_it() {
    let mut setup = Setup::new(FakeRegistry::new(
        GHOSTING.registry_key,
        &Value::Null,
        false,
        false,
    ));
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path(), None);
    setup.store = Some(store.clone());
    let (render, _) = setup.build(&GHOSTING);
    let render = Arc::new(render);
    let worker = {
        let render = render.clone();
        std::thread::spawn(move || {
            render.render(&|| false, |active| {
                active.muted = true;
                while !render.cancelled(&|| false) {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err::<(), _>(DemoError::new("GAME_EXITED"))
            })
        })
    };
    while !render.is_active() {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    render.dispose();
    assert!(!render.is_active());
    assert_eq!(worker.join().unwrap().unwrap_err().code, "CANCELLED");
    // Mute не снят — флаг к снятию следующим рендером; следующий рендер не отменён.
    assert_eq!(flag(&store, "ghostingUnmutePending"), json!(true));
    assert!(!render.cancelled(&|| false));
}

// ---------------------------------------------------------------- строже 0.4.9

#[test]
fn player_registry_and_files_survive_where_049_lost_them() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("jobs");
    fs::create_dir(&root).unwrap();
    let player = json!({"ScreenWidth": 2560});
    let registry = FakeRegistry::new(GHOSTING.registry_key, &player, false, false);

    // Битый маркер: невозвращённый снимок под ним не перезаписывается.
    fs::write(root.join(GHOSTING.marker), "{oops").unwrap();
    fs::write(root.join(GHOSTING.reg_file), "player").unwrap();
    let shared = SharedRegistry(registry.clone());
    let error = game_render::snapshot_registry(&shared, &root, &GHOSTING, Map::new()).unwrap_err();
    assert_eq!(error.code, "REGISTRY_FAILED");
    assert_eq!(
        fs::read_to_string(root.join(GHOSTING.reg_file)).unwrap(),
        "player"
    );

    // Игра запущена мимо лаунчера: recover откладывает возврат до следующего рендера.
    let marker = object(&json!({"version": 1, "existed": true, "at": CRASH_AT, "muted": false}));
    game_render::write_marker(&root, &GHOSTING, &marker).unwrap();
    let mut setup = Setup::new(registry.clone());
    setup.jobs_root = Some(root.clone());
    setup.inspector = FakeInspector {
        image: None,
        running: vec![None],
        fails: false,
    };
    let (render, _) = setup.build(&GHOSTING);
    assert!(render.recover().is_none());
    assert_eq!(registry.calls(), json!([]));
    assert!(root.join(GHOSTING.marker).exists());

    // Без корня заданий снимка нет — игра не запускается; второй помощник без stop — нельзя.
    let mut setup = Setup::new(registry.clone());
    setup.host = Some(Box::new(|| FakeHost {
        fail_start: false,
        start_events: Vec::new(),
        stop_events: Vec::new(),
        record: Arc::new(Mutex::new(Vec::new())),
        root: PathBuf::new(),
        queue: Vec::new(),
        stopped: false,
        pending: None,
        audio_reset: None,
    }));
    let (render, _) = setup.build(&GHOSTING);
    let codes = render
        .render(&|| false, |active| {
            let snapshot = render.snapshot(active, Map::new()).unwrap_err().code;
            render.start_host(active, &StartRequest::default(), false)?;
            let again = render
                .start_host(active, &StartRequest::default(), false)
                .unwrap_err()
                .code;
            Ok((snapshot, again))
        })
        .unwrap();
    assert_eq!(
        codes,
        ("REGISTRY_FAILED".into(), "RENDER_HOST_FAILED".into())
    );

    // Файл на месте папки рендера — чужой, не удаляется.
    let file = dir.path().join("hlsr-render");
    fs::write(&file, "x").unwrap();
    assert_eq!(game_render::cleanup_render_dirs(&file, None), 0);
    assert!(file.exists());
}

// ---------------------------------------------------------------- папки рендера

#[cfg(windows)]
fn junction(link: &Path, target: &Path) {
    // Только в тесте: junction без прав администратора.
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J");
}

#[cfg(windows)]
#[test]
fn render_dirs_are_removed_only_inside_without_following_links() {
    let dir = tempfile::tempdir().unwrap();
    let mod_dir = dir.path().join("ghostingmod");
    let outside = dir.path().join("outside");
    let render_root = mod_dir.join("hlsr-render");
    fs::create_dir_all(render_root.join("old1")).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep.txt"), "x").unwrap();
    fs::write(render_root.join("old1").join("f_0001.tga"), "x").unwrap();
    junction(&render_root.join("old1").join("link"), &outside);
    junction(&render_root.join("link2"), &outside);
    fs::write(mod_dir.join("keep.cfg"), "x").unwrap();
    assert_eq!(game_render::cleanup_render_dirs(&render_root, None), 2);
    assert!(!render_root.exists());
    assert!(outside.join("keep.txt").exists());
    assert!(mod_dir.join("keep.cfg").exists());

    // Сама папка рендера — ссылка: снимается ссылка, цель цела; своя папка поверх — нельзя
    // подменить.
    junction(&render_root, &outside);
    game_render::cleanup_render_dirs(&render_root, None);
    assert!(!render_root.exists());
    assert!(outside.join("keep.txt").exists());

    // keep: своя папка остаётся, остальное уходит.
    let job = game_render::make_render_dir(&render_root, "abcd").unwrap();
    fs::create_dir(render_root.join("old")).unwrap();
    assert_eq!(
        game_render::cleanup_render_dirs(&render_root, Some("ABCD")),
        1
    );
    assert!(job.exists());

    // Демки сегментов: ссылка или копия, чужой файл не перезаписывается.
    let source = dir.path().join("seg.dem");
    fs::write(&source, "demo").unwrap();
    game_render::link_or_copy(&source, &job.join("s001.dem")).unwrap();
    assert_eq!(fs::read_to_string(job.join("s001.dem")).unwrap(), "demo");
    assert!(game_render::link_or_copy(&source, &job.join("s001.dem")).is_err());
}

// ---------------------------------------------------------------- настоящий реестр

#[cfg(windows)]
mod hive {
    //! Реестр через Win32 в личном кусте приложения (`RegLoadAppKeyW` над временным файлом):
    //! реестр пользователя не меняется.

    use super::*;
    use game_render::WinRegistry;
    use hlsr_core::win32::wide;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{HKEY, KEY_ALL_ACCESS, RegCloseKey, RegLoadAppKeyW};

    struct Hive(HKEY);

    impl Drop for Hive {
        fn drop(&mut self) {
            // SAFETY: дескриптор получен от RegLoadAppKeyW; куст выгружается вместе с ним.
            unsafe { RegCloseKey(self.0) };
        }
    }

    fn hive(dir: &Path) -> Hive {
        let file = wide(&dir.join("test.hiv"));
        let mut root: HKEY = std::ptr::null_mut();
        // SAFETY: путь завершён нулём, `root` — локальная переменная.
        let status = unsafe { RegLoadAppKeyW(file.as_ptr(), &mut root, KEY_ALL_ACCESS, 0, 0) };
        assert_eq!(status, ERROR_SUCCESS, "RegLoadAppKeyW: код {status}");
        Hive(root)
    }

    fn utf16_file(text: &str) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        bytes
    }

    /// Снимок, как его оставил бы `reg.exe export` 0.4.9: перенос длинного hex, подключ.
    const PLAYER: &str = "Windows Registry Editor Version 5.00\r\n\r\n\
        [HKEY_CURRENT_USER\\Software\\Valve\\Half-Life\\Settings]\r\n\
        \"EngineDLL\"=\"hw.dll\"\r\n\
        \"ScreenWidth\"=dword:00000a00\r\n\
        \"Имя\"=\"Игрок \\\"1\\\" C:\\\\HL\"\r\n\
        @=\"по умолчанию\"\r\n\
        \"blob\"=hex:00,01,02,03,04,05,06,07,08,09,0a,0b,0c,0d,0e,0f,10,11,12,13,14,15,\\\r\n  \
        16,17,18\r\n\
        \"multi\"=hex(7):61,00,00,00,62,00,00,00,00,00\r\n\
        \"path\"=hex(2):25,00,41,00,25,00,00,00\r\n\
        \"q\"=hex(b):01,02,03,04,05,06,07,08\r\n\
        \"none\"=hex(0):\r\n\r\n\
        [HKEY_CURRENT_USER\\Software\\Valve\\Half-Life\\Settings\\Keys]\r\n\
        \"w\"=\"+forward\"\r\n\r\n\
        [HKEY_CURRENT_USER\\Software\\Other]\r\n\
        \"foreign\"=dword:00000001\r\n\r\n";

    /// Что пишет игра во время рендера: другие значения, новый подключ.
    const RENDER: &str = "Windows Registry Editor Version 5.00\r\n\r\n\
        [HKEY_CURRENT_USER\\Software\\Valve\\Half-Life\\Settings]\r\n\
        \"ScreenWidth\"=dword:00000500\r\n\
        \"Extra\"=dword:00000005\r\n\r\n\
        [HKEY_CURRENT_USER\\Software\\Valve\\Half-Life\\Settings\\Video]\r\n\
        \"mode\"=\"render\"\r\n\r\n";

    fn export(registry: &WinRegistry, dir: &Path, name: &str) -> Vec<u8> {
        let file = dir.join(name);
        registry.export_key(HL1.registry_key, &file).unwrap();
        fs::read(file).unwrap()
    }

    #[test]
    fn snapshot_and_restore_in_a_private_hive() {
        let dir = tempfile::tempdir().unwrap();
        let root = hive(dir.path());
        // SAFETY: куст открыт до конца теста.
        let registry = unsafe { WinRegistry::from_raw(root.0) };
        let jobs = dir.path().join("jobs");
        fs::create_dir(&jobs).unwrap();
        let key = HL1.registry_key;

        // Ключа нет: снимок «не было», возврат удаляет созданный игрой.
        assert!(!registry.exists(key).unwrap());
        registry.delete_key(key).unwrap();
        game_render::snapshot_registry(&registry, &jobs, &HL1, Map::new()).unwrap();
        assert!(!jobs.join(HL1.reg_file).exists());
        let render_file = dir.path().join("render.reg");
        fs::write(&render_file, utf16_file(RENDER)).unwrap();
        registry.import_file(key, &render_file).unwrap();
        assert!(registry.exists(key).unwrap());
        game_render::restore_registry(&registry, &jobs, &HL1).unwrap();
        assert!(!registry.exists(key).unwrap());
        assert_eq!(listing(&jobs), json!([]));

        // Снимок reg.exe 0.4.9: чужой ключ файла не трогается.
        let player_file = dir.path().join("player.reg");
        fs::write(&player_file, utf16_file(PLAYER)).unwrap();
        registry.import_file(key, &player_file).unwrap();
        assert!(!registry.exists("HKCU\\Software\\Other").unwrap());
        let before = export(&registry, dir.path(), "before.reg");

        // Ключ есть: снимок, игра меняет значения и подключи, возврат — как было.
        game_render::snapshot_registry(&registry, &jobs, &HL1, Map::new()).unwrap();
        assert_eq!(fs::read(jobs.join(HL1.reg_file)).unwrap(), before);
        registry.import_file(key, &render_file).unwrap();
        assert_ne!(export(&registry, dir.path(), "changed.reg"), before);
        let marker = game_render::restore_registry(&registry, &jobs, &HL1)
            .unwrap()
            .unwrap();
        assert_eq!(marker["existed"], json!(true));
        assert_eq!(export(&registry, dir.path(), "after.reg"), before);
        assert_eq!(listing(&jobs), json!([]));

        // Битый снимок или снимок без ключа: ключ не удаляется, маркер ждёт.
        for broken in [
            b"\xff\xfe[".to_vec(),
            utf16_file(RENDER.replace("Half-Life", "Other").as_str()),
        ] {
            game_render::snapshot_registry(&registry, &jobs, &HL1, Map::new()).unwrap();
            fs::write(jobs.join(HL1.reg_file), broken).unwrap();
            assert!(game_render::restore_registry(&registry, &jobs, &HL1).is_err());
            assert_eq!(export(&registry, dir.path(), "kept.reg"), before);
            fs::remove_file(jobs.join(HL1.marker)).unwrap();
        }

        // Снимок, оставленный упавшим 0.4.9 (маркер и .reg его байтами), возвращается при старте.
        fs::write(jobs.join(HL1.reg_file), utf16_file(PLAYER)).unwrap();
        fs::write(
            jobs.join(HL1.marker),
            format!("{{\n  \"version\": 1,\n  \"existed\": true,\n  \"at\": \"{CRASH_AT}\",\n  \"muted\": true\n}}\n"),
        )
        .unwrap();
        registry.import_file(key, &render_file).unwrap();
        let store_dir = tempfile::tempdir().unwrap();
        let store = store_in(store_dir.path(), None);
        let logger = Arc::new(MemoryLogger::default());
        let ports = Ports {
            windows: true,
            jobs_root: Some(jobs.clone()),
            store: Some(store.clone()),
            logger: logger.clone(),
            // SAFETY: куст открыт до конца теста.
            registry: Box::new(unsafe { WinRegistry::from_raw(root.0) }),
            inspector: Box::new(FakeInspector {
                image: None,
                running: Vec::new(),
                fails: false,
            }),
            install_path: Box::new(|| None),
            game_running: Box::new(|_| false),
            game_busy: Box::new(|| false),
            steam_status: Box::new(SteamStatus::default),
            screen_size: Box::new(|| None),
            host_factory: Box::new(|| panic!("помощник не нужен")),
        };
        let render = GameRender::new(HL1.clone(), ports);
        assert!(render.recover().is_some());
        assert_eq!(export(&registry, dir.path(), "recovered.reg"), before);
        assert_eq!(flag(&store, HL1.unmute_pending), json!(true));
        assert_eq!(listing(&jobs), json!([]));
        assert_eq!(
            logger.take(),
            [(
                "warn",
                format!("[demo] рендер HL1 прерван сбоем ({CRASH_AT}): реестр игры возвращён")
            )]
        );
    }
}
