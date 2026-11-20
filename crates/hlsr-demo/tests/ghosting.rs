//! Адаптер Ghosting в режиме мода `-ghrender` против 0.4.9 (`tests/fixtures/ghosting.json`,
//! `tools/reference/demo/ghosting.js`): признак в `client.dll`, имя рана, диапазон записей,
//! аргументы игры, стандартный конфиг/HUD, командная строка, разбор журнала `GHRENDER 2`, коды
//! `FAIL`, таймлайн, TGA и сам адаптер с подставными игрой (пишет журнал и кадры, как мод) и
//! FFmpeg. Дальше — строже 0.4.9 (решение 5: без старого пути) и настоящий FFmpeg на кадрах
//! подставной игры.

mod common;

use hlsr_core::json_text::js_number;
use hlsr_core::logger::MemoryLogger;
use hlsr_core::store::Store;
use hlsr_delivery::ports::FreeSpace;
use hlsr_demo::Marker;
use hlsr_demo::errors::DemoError;
use hlsr_demo::ffmpeg::Located;
use hlsr_demo::game_render::{Inspector, Ports, Registry, SteamStatus};
use hlsr_demo::ghosting::{self, Encoder, GhostingAdapter};
use hlsr_demo::ghrender::{self, Fields, GameArgs};
use hlsr_demo::jobs::{Adapter, Material, RenderProgress, RenderRequest};
use hlsr_demo::library::{self, Described, Entry};
use hlsr_demo::planner::Source;
use hlsr_demo::render_host::{Host, HostEvent, StartRequest};
use hlsr_demo::source_render::{self, Range};
use serde_json::{Map, Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/ghosting.json")).unwrap()
}

fn text(value: &Value) -> String {
    value.as_str().unwrap().to_owned()
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().unwrap().iter().map(text).collect()
}

fn fields(value: &Value) -> Fields {
    Fields(
        value
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), text(v)))
            .collect(),
    )
}

fn markers_value(markers: &[Marker]) -> Value {
    Value::Array(markers.iter().map(Marker::to_value).collect())
}

fn marker_of(value: &Value) -> Marker {
    Marker {
        t: value["t"].as_f64().unwrap(),
        kind: match value["type"].as_str().unwrap() {
            "map" => "map",
            "timer" => "timer",
            "saveload" => "saveload",
            other => panic!("метка {other}"),
        },
        label: text(&value["label"]),
    }
}

fn cuts_value(cuts: &[source_render::Cut]) -> Value {
    Value::Array(
        cuts.iter()
            .map(|cut| json!({"from": js_number(cut.from), "to": js_number(cut.to)}))
            .collect(),
    )
}

fn error_value(error: &DemoError) -> Value {
    json!({
        "code": error.code,
        "message": error.message,
        "details": if error.details.is_empty() { Value::Null } else { Value::Object(error.details.clone()) },
    })
}

/// `writeClient(modDir, strings)` самотеста.
fn write_client(mod_dir: &Path, list: &[String]) {
    fs::create_dir_all(mod_dir.join("bin")).unwrap();
    let mut data = vec![0x90u8; 4096];
    data.extend(hlsr_core::js::latin1_bytes(&list.join("\0")));
    data.extend([0u8; 16]);
    fs::write(mod_dir.join("bin").join("client.dll"), data).unwrap();
}

// ---------------------------------------------------------------- чистая логика

#[test]
fn constants_match_049() {
    let f = fixture();
    assert_eq!(f["protocol"], ghrender::PROTOCOL);
    assert_eq!(f["maxClientBytes"], ghrender::MAX_CLIENT_BYTES);
    assert_eq!(f["stockHudCfg"], source_render::STOCK_HUD_CFG);
    for (from, to) in f["failCodes"].as_object().unwrap() {
        assert_eq!(ghrender::fail_code(from), to.as_str(), "{from}");
    }
    assert_eq!(ghrender::fail_code("SOMETHING_NEW"), None);
}

#[test]
fn client_caps_by_strings_with_cache_like_049() {
    let dir = tempfile::tempdir().unwrap();
    let mod_dir = dir.path().join("ghostingmod");
    let file = mod_dir.join("bin").join("client.dll");
    let stamp = UNIX_EPOCH + Duration::from_secs(1_793_534_400); // 2026-11-01T12:00:00Z
    // 0.4.9 сводит ошибку чтения к «не умеет»; здесь она видна вызывающему.
    let missing = ghrender::client_caps(&mod_dir).unwrap_err();
    assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
    for step in fixture()["caps"].as_array().unwrap() {
        if step.get("strings").is_some() {
            write_client(&mod_dir, &strings(&step["strings"]));
        }
        if step["stamp"] == true {
            let handle = fs::File::options().write(true).open(&file).unwrap();
            handle
                .set_times(fs::FileTimes::new().set_accessed(stamp).set_modified(stamp))
                .unwrap();
        }
        if step["big"] == true {
            let mut handle = fs::File::options().write(true).open(&file).unwrap();
            handle.set_len(ghrender::MAX_CLIENT_BYTES + 1).unwrap();
            drop(handle);
            handle = fs::File::options().append(true).open(&file).unwrap();
            handle
                .write_all(b"-ghrender_out -ghrender_segments")
                .unwrap();
        }
        let caps = ghrender::client_caps(&mod_dir).unwrap_or_default();
        assert_eq!(
            json!({"ghrender": caps.ghrender, "segments": caps.segments}),
            step["caps"],
            "{}",
            step["name"]
        );
    }
}

#[test]
fn playlist_arg_entries_and_segments_like_049() {
    let f = fixture();
    let dir = tempfile::tempdir().unwrap();
    let mod_dir = dir.path().join("ghostingmod");
    for case in f["playlistArg"].as_array().unwrap() {
        let file = match case.get("parts") {
            Some(parts) => strings(parts)
                .iter()
                .fold(mod_dir.clone(), |p, part| p.join(part)),
            None => PathBuf::from(text(&case["absolute"])),
        };
        let arg = ghrender::playlist_arg(&mod_dir, &file.to_string_lossy());
        assert_eq!(json!(arg), case["arg"], "{case}");
    }
    for case in f["entriesInPlace"].as_array().unwrap() {
        let list: Vec<(String, Option<String>)> = case["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| (text(&s["name"]), s.get("playlistFile").map(text)))
            .collect();
        let got = ghrender::entries_in_place(
            case["arg"].as_str().unwrap(),
            list.iter().map(|(n, f)| (n.as_str(), f.as_deref())),
        );
        assert_eq!(json!(got), case["inPlace"], "{case}");
    }
    for case in f["segmentsArg"].as_array().unwrap() {
        let indexes: Vec<usize> = case["indexes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i.as_u64().unwrap() as usize)
            .collect();
        let got = ghrender::segments_arg(&indexes, case["whole"].as_u64().unwrap() as usize);
        assert_eq!(json!(got), case["arg"], "{case}");
    }
}

#[test]
fn config_arguments_and_command_lines_like_049() {
    let f = fixture();
    for case in f["config"].as_array().unwrap() {
        let plan = source_render::config_arguments(
            case["ownConfig"].as_bool().unwrap(),
            case["ownHud"].as_bool().unwrap(),
            "ghostingmod/hlsr-render/ab12cd34/std",
        );
        assert_eq!(json!(plan.args), case["args"], "{case}");
        assert_eq!(json!(plan.files), case["files"], "{case}");
    }
    for case in f["gameArguments"].as_array().unwrap() {
        let input = &case["input"];
        let config = input.get("config").map(strings).unwrap_or_default();
        let segments = input.get("segments").map(text);
        let args = ghrender::game_arguments(&GameArgs {
            width: input["width"].as_u64().unwrap() as u32,
            height: input["height"].as_u64().unwrap() as u32,
            fps: input["fps"].as_u64().unwrap() as u32,
            keyhud: input["keyhud"].as_bool().unwrap(),
            integrity: input["integrity"].as_bool().unwrap(),
            playlist: input["playlist"].as_str().unwrap(),
            out: input["out"].as_str().unwrap(),
            segments: segments.as_deref(),
            config: &config,
        });
        assert_eq!(json!(args), case["args"], "{input}");
    }
    for case in f["commandLine"].as_array().unwrap() {
        let line = source_render::command_line(&text(&case["exe"]), &strings(&case["args"]));
        assert_eq!(line, text(&case["line"]));
    }
}

#[test]
fn journal_lines_and_fail_codes_like_049() {
    let f = fixture();
    for case in f["parseLine"].as_array().unwrap() {
        let got = ghrender::parse_line(case["line"].as_str().unwrap()).map_or(
            Value::Null,
            |line| json!({"key": line.key, "fields": line.fields.to_value(), "text": line.text}),
        );
        assert_eq!(got, case["parsed"], "{}", case["line"]);
    }
    for case in f["failError"].as_array().unwrap() {
        let line = ghrender::parse_line(case["line"].as_str().unwrap()).unwrap();
        let error = ghrender::fail_error(&line);
        assert_eq!(
            error_value(&error),
            json!({"code": case["code"], "message": case["message"], "details": case["details"]}),
            "{}",
            case["line"]
        );
    }
}

#[test]
fn timeline_and_range_start_like_049() {
    let f = fixture();
    for case in f["timeline"].as_array().unwrap() {
        let input = &case["input"];
        let list = |key: &str| -> Vec<Fields> {
            input[key].as_array().unwrap().iter().map(fields).collect()
        };
        let starts: Vec<f64> = input["starts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let out = ghrender::timeline_of(
            &list("segments"),
            &list("timers"),
            input["frames"].as_f64().unwrap(),
            input["fps"].as_f64().unwrap(),
            &starts,
            input["total"].as_f64().unwrap_or(f64::NAN),
        );
        assert_eq!(
            json!({"offset": js_number(out.offset), "cuts": cuts_value(&out.cuts), "markers": markers_value(&out.markers)}),
            case["out"],
            "{}",
            input["name"]
        );
    }
    for case in f["rangeStart"].as_array().unwrap() {
        let input = &case["input"];
        let markers = input["markers"]
            .as_array()
            .unwrap()
            .iter()
            .map(marker_of)
            .collect();
        let range = input["range"].as_object().map(|r| Range {
            from: r["from"].as_f64().unwrap(),
            to: r["to"].as_f64().unwrap(),
        });
        let out = ghrender::with_range_start(markers, range);
        assert_eq!(markers_value(&out), case["out"], "{}", input["range"]);
    }
}

#[test]
fn tga_info_like_049() {
    for case in fixture()["tga"].as_array().unwrap() {
        let hex = case["hex"].as_str().unwrap();
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
            .collect();
        let got = source_render::tga_info(&bytes).map_or(Value::Null, |info| {
            json!({"width": info.width, "height": info.height, "bpp": info.bpp, "pixFmt": info.pix_fmt,
                "topDown": info.top_down, "offset": info.offset, "bytes": info.bytes, "complete": info.complete})
        });
        assert_eq!(got, case["info"], "{}", case["name"]);
    }
}

// ---------------------------------------------------------------- подставная игра

/// Мод в подставной игре: журнал, кадры `f_NNNN.tga` переименованием, `audio.wav` к `DONE`,
/// выход после `DONE`/`FAIL` (как `FakeHost` самотеста 0.4.9).
#[derive(Default)]
struct Game {
    mod_dir: PathBuf,
    journal: Vec<String>,
    total_frames: u32,
    exit_after_journal: bool,
    /// Первая строка вместо `GHRENDER 2`.
    protocol: Option<String>,
    /// Мод молчит (нет режима `-ghrender`).
    silent: bool,
    /// Мод не создал журнал и вышел с кодом 2.
    exit_silently: bool,
    /// Пауза перед строкой журнала (отмена посреди рендера).
    line_pause: Option<Duration>,
    launches: Mutex<Vec<StartRequest>>,
    resets: Mutex<Vec<bool>>,
    segments_arg: Mutex<Option<String>>,
}

#[derive(Default)]
struct Shared {
    exit: Mutex<Option<u32>>,
    events: Mutex<Vec<HostEvent>>,
    stop: AtomicBool,
}

impl Shared {
    fn exited(&self, code: u32) {
        *self.exit.lock().unwrap() = Some(code);
        self.events.lock().unwrap().push(HostEvent::Exited { code });
    }
}

struct FakeHost {
    game: Arc<Game>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    reset_only: bool,
    stopped: bool,
    pending: Option<bool>,
    audio_reset: Option<bool>,
}

/// Аргументы командной строки, как `argv` самотеста.
fn argv(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut word = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => {
                        word.push(c);
                        word.extend(chars.next());
                    }
                    _ => word.push(c),
                }
            }
            out.push(word);
        } else {
            let mut word = String::new();
            while let Some(&c) = chars.peek().filter(|c| !c.is_whitespace()) {
                word.push(c);
                chars.next();
            }
            out.push(word);
        }
    }
    out
}

fn tga_frame(n: u32) -> Vec<u8> {
    let mut data = vec![0u8; 18 + 8 * 4 * 3];
    data[2] = 2;
    data[12..14].copy_from_slice(&8u16.to_le_bytes());
    data[14..16].copy_from_slice(&4u16.to_le_bytes());
    data[16] = 24;
    data[18] = (n & 0xff) as u8;
    data[19] = (n >> 8) as u8;
    data
}

fn simulate(game: Arc<Game>, shared: Arc<Shared>, args: Vec<String>) {
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|at| args.get(at + 1).cloned())
    };
    let out = value("-ghrender_out")
        .unwrap()
        .split('/')
        .fold(game.mod_dir.clone(), |p, part| p.join(part));
    fs::create_dir_all(&out).unwrap();
    let log = |line: &str| {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(out.join("ghrender.log"))
            .unwrap();
        file.write_all(format!("{line}\n").as_bytes()).unwrap();
    };
    let stopped = || shared.stop.load(Ordering::SeqCst);
    let sleep = |ms| thread::sleep(Duration::from_millis(ms));
    if game.exit_silently {
        sleep(20);
        shared.exited(2);
        return;
    }
    if game.silent {
        while !stopped() {
            sleep(10);
        }
        return;
    }
    let mut frame = 0u32;
    let write_frame = |frame: &mut u32| {
        let tmp = out.join(format!("raw_{frame}.tga"));
        fs::write(&tmp, tga_frame(*frame)).unwrap();
        fs::rename(&tmp, out.join(format!("f_{frame:04}.tga"))).unwrap();
        *frame += 1;
    };
    sleep(20);
    log(game.protocol.as_deref().unwrap_or(ghrender::PROTOCOL));
    *game.segments_arg.lock().unwrap() = value("-ghrender_segments");
    for line in &game.journal {
        if stopped() {
            return;
        }
        if let Some(pause) = game.line_pause {
            thread::sleep(pause);
        }
        if line.starts_with("SEGMENT ")
            && let Some(at) = line.find("frame=")
        {
            let digits: String = line[at + 6..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            while frame < digits.parse::<u32>().unwrap() {
                write_frame(&mut frame);
            }
        }
        let done = line.starts_with("DONE");
        if done || line.starts_with("FAIL") {
            if done {
                while frame < game.total_frames {
                    write_frame(&mut frame);
                }
                fs::write(out.join("audio.wav"), b"RIFF\0\0\0\0WAVE").unwrap();
            }
            log(line);
            sleep(30);
            shared.exited(if done { 0 } else { 1 });
            return;
        }
        log(line);
        sleep(5);
    }
    if game.exit_after_journal {
        shared.exited(0);
    }
}

impl Host for FakeHost {
    fn start(&mut self, request: &StartRequest) -> Result<u32, DemoError> {
        self.game.launches.lock().unwrap().push(request.clone());
        self.reset_only = request.reset_only;
        let mut events = self.shared.events.lock().unwrap();
        if request.reset_only {
            events.push(HostEvent::Started {
                pid: 0,
                desktop: String::new(),
            });
            return Ok(0);
        }
        let (game, shared) = (self.game.clone(), self.shared.clone());
        let args = argv(&request.command_line);
        self.thread = Some(thread::spawn(move || simulate(game, shared, args)));
        events.push(HostEvent::Started {
            pid: 77,
            desktop: String::new(),
        });
        events.push(HostEvent::Muted {
            pid: 77,
            device: 0,
            was: false,
        });
        Ok(77)
    }

    fn events(&mut self) -> Vec<HostEvent> {
        std::mem::take(&mut *self.shared.events.lock().unwrap())
    }

    fn stop(&mut self, reset: bool) {
        if std::mem::replace(&mut self.stopped, true) {
            return;
        }
        if !self.reset_only {
            self.game.resets.lock().unwrap().push(reset);
        }
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
        let reset = reset || self.reset_only;
        let mut events = self.shared.events.lock().unwrap();
        if reset {
            events.push(HostEvent::AudioReset { ok: true });
            self.audio_reset = Some(true);
        }
        self.pending = Some(!reset);
        events.push(HostEvent::Stopped {
            code: Some(0),
            alone: true,
            pending: !reset,
        });
    }

    fn exit_code(&self) -> Option<u32> {
        *self.shared.exit.lock().unwrap()
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

/// Реестр игрока: ключ → JSON, снимок — JSON (как в самотесте).
#[derive(Clone, Default)]
struct FakeRegistry(Arc<Mutex<Map<String, Value>>>);

impl Registry for FakeRegistry {
    fn exists(&self, key: &str) -> Result<bool, DemoError> {
        Ok(self.0.lock().unwrap().contains_key(key))
    }

    fn export_key(&self, key: &str, file: &Path) -> Result<(), DemoError> {
        let value = self
            .0
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .unwrap_or(Value::Null);
        fs::write(file, value.to_string()).unwrap();
        Ok(())
    }

    fn delete_key(&self, key: &str) -> Result<(), DemoError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }

    fn import_file(&self, key: &str, file: &Path) -> Result<(), DemoError> {
        let value = serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap();
        self.0.lock().unwrap().insert(key.to_owned(), value);
        Ok(())
    }
}

struct NoProcesses;

impl Inspector for NoProcesses {
    fn find_by_image_name(&self, _image: &str) -> Result<Vec<Option<String>>, String> {
        Ok(Vec::new())
    }

    fn same_path(&self, left: &str, right: &str) -> bool {
        left == right
    }
}

struct Plenty;

impl FreeSpace for Plenty {
    fn free_bytes(&self, _dir: &Path) -> Option<u64> {
        Some(1_000_000_000_000_000)
    }
}

#[derive(Default)]
struct Sink {
    spawns: Vec<Value>,
    frames: Vec<u32>,
}

struct FakeEncoder(Arc<Mutex<Sink>>);

impl Encoder for FakeEncoder {
    fn write(&mut self, pixels: &[u8]) -> Result<(), DemoError> {
        let tag = u32::from(pixels[0]) + u32::from(pixels[1]) * 256;
        self.0.lock().unwrap().frames.push(tag);
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<(), DemoError> {
        Ok(())
    }
}

const RUN_DURATIONS: [f64; 5] = [3.0, 0.01, 4.0, 0.01, 2.98];

/// Сценарий: игра с ранами `run1` в `demos/` мода, корень заданий, реестр, Store.
struct Scenario {
    _temp: tempfile::TempDir,
    dir: PathBuf,
    mod_dir: PathBuf,
    jobs_root: PathBuf,
    job_dir: PathBuf,
    item: Described,
    game: Arc<Game>,
    registry: FakeRegistry,
    store: Arc<Mutex<Store>>,
    logger: Arc<MemoryLogger>,
    sink: Arc<Mutex<Sink>>,
    adapter: GhostingAdapter,
}

impl Scenario {
    fn new(game: Game, client: &[&str], stall: Duration, real_ffmpeg: bool) -> Scenario {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        let install = dir.join("game");
        let mod_dir = install.join("ghostingmod");
        fs::create_dir_all(&mod_dir).unwrap();
        fs::write(install.join("hl2.exe"), "").unwrap();
        write_client(
            &mod_dir,
            &client.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
        );
        let segments: Vec<Value> = RUN_DURATIONS
            .iter()
            .enumerate()
            .map(|(i, d)| json!({"map": if i < 4 { "m1" } else { "m2" }, "duration": d}))
            .collect();
        let playlist = common::write_ghosting_run(
            &mod_dir.join("demos"),
            "run1",
            &json!(segments),
            &json!({}),
        );
        let item = describe_playlist(&playlist, "run1");
        let jobs_root = dir.join("jobs");
        let job_dir = jobs_root.join("abcdef0123456789");
        fs::create_dir_all(&job_dir).unwrap();
        let registry = FakeRegistry::default();
        registry.0.lock().unwrap().insert(
            ghosting::REGISTRY_KEY.to_owned(),
            json!({"ScreenWidth": 2560}),
        );
        let store = Arc::new(Mutex::new(Store::open(
            dir.join("demo-tool.json"),
            json!({}),
        )));
        let logger = Arc::new(MemoryLogger::default());
        let game = Arc::new(Game {
            mod_dir: mod_dir.clone(),
            ..game
        });
        let factory_game = game.clone();
        let found = install.clone();
        let ports = Ports {
            windows: true,
            jobs_root: Some(jobs_root.clone()),
            store: Some(store.clone()),
            logger: logger.clone(),
            registry: Box::new(registry.clone()),
            inspector: Box::new(NoProcesses),
            install_path: Box::new(move || Some(found.clone())),
            game_running: Box::new(|_| false),
            game_busy: Box::new(|| false),
            steam_status: Box::new(|| SteamStatus {
                ok: true,
                code: None,
            }),
            screen_size: Box::new(|| Some((1920, 1080))),
            host_factory: Box::new(move || {
                Box::new(FakeHost {
                    game: factory_game.clone(),
                    shared: Arc::new(Shared::default()),
                    thread: None,
                    reset_only: false,
                    stopped: false,
                    pending: None,
                    audio_reset: None,
                }) as Box<dyn Host>
            }),
        };
        let sink = Arc::new(Mutex::new(Sink::default()));
        let mut adapter = GhostingAdapter::new(ports, Arc::new(Plenty))
            .with_timeouts(Duration::from_secs(3), stall);
        if !real_ffmpeg {
            let spawn_sink = sink.clone();
            adapter = adapter.with_encoder(Box::new(move |exe: &Path, args: &[String]| {
                spawn_sink
                    .lock()
                    .unwrap()
                    .spawns
                    .push(json!({"exe": exe.to_string_lossy(), "args": args}));
                Ok(Box::new(FakeEncoder(spawn_sink.clone())) as Box<dyn Encoder>)
            }));
        }
        Scenario {
            _temp: temp,
            dir,
            mod_dir,
            jobs_root,
            job_dir,
            item,
            game,
            registry,
            store,
            logger,
            sink,
            adapter,
        }
    }

    /// Пути сценария — `<scenario>`, «/».
    fn rel(&self, value: &Value) -> Value {
        let dir = self.dir.to_string_lossy().into_owned();
        match value {
            Value::String(s) if s.contains(&dir) => {
                Value::String(s.replace(&dir, "<scenario>").replace('\\', "/"))
            }
            Value::Array(items) => Value::Array(items.iter().map(|v| self.rel(v)).collect()),
            Value::Object(map) => {
                Value::Object(map.iter().map(|(k, v)| (k.clone(), self.rel(v))).collect())
            }
            other => other.clone(),
        }
    }

    fn render(
        &self,
        options: &Value,
        ffmpeg: &Located,
        cancel: &dyn Fn() -> bool,
        progress: &dyn Fn(&RenderProgress),
    ) -> Result<Material, DemoError> {
        let mut range = Range {
            from: 0.0,
            to: self.item.duration.unwrap(),
        };
        if let Some(from) = options["range"]["from"].as_f64() {
            range.from = from;
        }
        if let Some(to) = options["range"]["to"].as_f64() {
            range.to = to;
        }
        let request = RenderRequest {
            item: &self.item,
            range,
            fps: 60,
            width: 8,
            height: 4,
            audio: options["audio"] != false,
            keyhud: options["keyhud"] != false,
            integrity: options["integrity"] == true,
            hud: true,
            own_config: options["ownConfig"] != false,
            own_hud: options["ownHud"] != false,
            job_dir: &self.job_dir,
            ffmpeg,
            cancel,
            on_progress: progress,
        };
        self.adapter.render(&request)
    }

    fn registry_value(&self) -> Value {
        self.registry
            .0
            .lock()
            .unwrap()
            .get(ghosting::REGISTRY_KEY)
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn render_dir(&self) -> bool {
        self.mod_dir.join("hlsr-render").exists()
    }

    fn launches(&self) -> Vec<StartRequest> {
        self.game.launches.lock().unwrap().clone()
    }
}

fn describe_playlist(playlist: &Path, name: &str) -> Described {
    library::describe(&Entry {
        id: "id".into(),
        kind: "playlist",
        source: "manual",
        game_id: None,
        name: name.into(),
        files: vec![playlist.to_string_lossy().into_owned()],
        mtime_ms: 0.0,
        series_missing: None,
    })
}

fn fake_ffmpeg() -> Located {
    Located {
        ffmpeg: PathBuf::from("ffmpeg"),
        ffprobe: PathBuf::from("ffprobe"),
        source: "path",
    }
}

const PROTOCOL_2: [&str; 3] = ["-ghrender", "-ghrender_out", "-ghrender_segments"];

fn material_value(material: &Material) -> Value {
    let Source::Video(video) = &material.video else {
        panic!("кадры вместо видео");
    };
    json!({
        "video": video,
        "audio": material.audio,
        "offset": js_number(material.offset.unwrap()),
        "cuts": cuts_value(&material.cuts),
        "timeline": markers_value(&material.timeline),
        "profile": material.profile,
        "launches": js_number(material.launches.unwrap()),
        "segments": js_number(material.segments.unwrap()),
        "frames": js_number(material.frames.unwrap()),
    })
}

/// Строки сценария 0.4.9 с путём `<root>/sN` — `<scenario>`.
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

/// Журнал без замеров времени («, N с»).
fn timeless(logger: &MemoryLogger, scenario: &Scenario) -> Value {
    let lines: Vec<Value> = logger
        .take()
        .into_iter()
        .map(|(level, message)| {
            let message = match message.rfind(", ") {
                Some(at)
                    if message.ends_with(" с")
                        && message[at + 2..message.len() - 3]
                            .chars()
                            .all(|c| c.is_ascii_digit()) =>
                {
                    format!("{}, <N> с", &message[..at])
                }
                _ => message,
            };
            json!([level, message])
        })
        .collect();
    scenario.rel(&Value::Array(lines))
}

#[test]
fn adapter_renders_like_049() {
    for case in fixture()["adapter"].as_array().unwrap() {
        let name = text(&case["name"]);
        let options = &case["options"];
        let game = Game {
            journal: strings(&options["journal"]),
            total_frames: options["totalFrames"].as_u64().unwrap() as u32,
            exit_after_journal: options["exitAfterJournal"] == true,
            ..Game::default()
        };
        let stall = options["stallMs"]
            .as_u64()
            .map_or(Duration::from_secs(75), Duration::from_millis);
        let s = Scenario::new(game, &PROTOCOL_2, stall, false);
        assert_eq!(
            js_number(s.item.duration.unwrap()),
            case["duration"],
            "{name}"
        );
        let progress = Mutex::new(Vec::new());
        let result = s.render(
            options,
            &fake_ffmpeg(),
            &|| false,
            &|info: &RenderProgress| progress.lock().unwrap().push(info.clone()),
        );
        let result = match &result {
            Ok(material) => json!({"material": material_value(material)}),
            Err(error) => json!({"error": error_value(error)}),
        };
        let launches: Vec<Value> = s
            .launches()
            .iter()
            .map(|l| {
                json!({"exe": l.exe.to_string_lossy(), "cwd": l.cwd.to_string_lossy(), "commandLine": l.command_line,
                    "mute": l.mute, "unmuteAll": l.unmute_all, "resetCommandLine": l.reset_command_line})
            })
            .collect();
        let sink = s.sink.lock().unwrap();
        let frames = &sink.frames;
        let mut job_files: Vec<String> = fs::read_dir(&s.job_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        job_files.sort();
        let pending = s
            .store
            .lock()
            .unwrap()
            .get(ghosting::UNMUTE_PENDING)
            .cloned()
            .unwrap_or(Value::Null);
        let got = json!({
            "launches": s.rel(&json!(launches)),
            "segmentsArg": s.game.segments_arg.lock().unwrap().clone(),
            "ffmpeg": s.rel(&json!(sink.spawns)),
            "frames": {"count": frames.len(), "first": frames.iter().take(3).collect::<Vec<_>>(),
                "last": frames.last().map(|f| vec![*f]).unwrap_or_default()},
            "result": s.rel(&result),
            "resets": s.game.resets.lock().unwrap().clone(),
            "registry": s.registry_value(),
            "renderDir": s.render_dir(),
            "jobFiles": job_files,
            "pending": pending,
            "logs": timeless(&s.logger, &s),
        });
        for key in got.as_object().unwrap().keys() {
            assert_eq!(got[key], expected_rel(&case[key]), "{name}: {key}");
        }
        // Номер сегмента в прогрессе — в пределах плана, даже при чужом n= в журнале.
        for info in progress.lock().unwrap().iter() {
            if let Some(segment) = info.segment {
                assert!(
                    segment.index >= 1 && segment.index <= segment.count,
                    "{name}"
                );
            }
        }
        assert!(!s.adapter.game().is_active(), "{name}");
    }
}

#[test]
fn reset_mute_without_export_like_049() {
    let expected = fixture()["resetMute"].clone();
    let s = Scenario::new(Game::default(), &PROTOCOL_2, Duration::from_secs(75), false);
    s.store
        .lock()
        .unwrap()
        .set(ghosting::UNMUTE_PENDING, Value::Bool(true));
    assert_eq!(
        json!(s.adapter.game().mute_reset_blocker()),
        expected["blocker"]
    );
    assert_eq!(json!(s.adapter.reset_mute()), expected["ok"]);
    let launch = &s.launches()[0];
    let id = |line: &str| -> String {
        let at = line.find("hlsr-render/").unwrap() + 12;
        line.replace(&line[at..at + 8], "<id>")
    };
    let got = json!({
        "exe": launch.exe.to_string_lossy(),
        "cwd": launch.cwd.to_string_lossy(),
        "commandLine": id(&launch.command_line),
        "resetCommandLine": id(&launch.reset_command_line),
        "resetOnly": launch.reset_only,
    });
    assert_eq!(s.rel(&got), expected_rel(&expected["launch"]));
    assert_eq!(
        s.store
            .lock()
            .unwrap()
            .get(ghosting::UNMUTE_PENDING)
            .cloned(),
        Some(expected["pending"].clone())
    );
    assert_eq!(s.registry_value(), expected["registry"]);
    assert_eq!(json!(s.render_dir()), expected["renderDir"]);
}

// ---------------------------------------------------------------- строже 0.4.9

fn journal_049() -> Vec<String> {
    strings(&fixture()["adapter"][0]["options"]["journal"])
}

fn assert_restored(s: &Scenario, what: &str) {
    assert_eq!(s.registry_value(), json!({"ScreenWidth": 2560}), "{what}");
    assert!(!s.render_dir(), "{what}");
    assert!(!s.adapter.game().is_active(), "{what}");
    assert!(!s.jobs_root.join(ghosting::MARKER).exists(), "{what}");
}

#[test]
fn without_ghrender_there_is_no_old_path() {
    // client.dll без -ghrender (1.0.12) или протокол 1: отказ до запуска игры.
    for (client, why) in [
        (&["-game", "+exec"][..], "client.dll не знает -ghrender"),
        (
            &["-ghrender", "-ghrender_out"][..],
            "client.dll без -ghrender_segments (протокол 1)",
        ),
    ] {
        let s = Scenario::new(Game::default(), client, Duration::from_secs(75), false);
        let error = s.adapter.can_render(&s.item).unwrap_err();
        assert_eq!(
            (error.code.as_str(), error.message.as_str()),
            ("GHRENDER_UNSUPPORTED", why)
        );
        let error = s
            .render(&json!({}), &fake_ffmpeg(), &|| false, &|_| {})
            .unwrap_err();
        assert_eq!(error.code, "GHRENDER_UNSUPPORTED");
        assert!(s.launches().is_empty(), "{why}");
        assert_restored(&s, why);
    }

    // Одиночная демка, плейлист вне demos/ мода, переименованная папка рана.
    let s = Scenario::new(Game::default(), &PROTOCOL_2, Duration::from_secs(75), false);
    assert!(s.adapter.can_render(&s.item).is_ok());
    // Как в самотесте 0.4.9: то же описание, другой вид или файл.
    let with_entry = |kind: &'static str, file: &Path| Described {
        entry: Entry {
            kind,
            files: vec![file.to_string_lossy().into_owned()],
            ..s.item.entry.clone()
        },
        ..s.item.clone()
    };
    let segment = s.mod_dir.join("demos").join("run1").join("segment001.dem");
    let demo = with_entry("demo", &segment);
    assert_eq!(
        s.adapter.can_render(&demo).unwrap_err().code,
        "RENDER_NOT_READY"
    );
    let outside = s.mod_dir.join("run.ghplaylist");
    assert_eq!(
        s.adapter
            .can_render(&with_entry("playlist", &outside))
            .unwrap_err()
            .code,
        "RENDER_NOT_READY"
    );
    let demos = s.mod_dir.join("demos");
    fs::rename(demos.join("run1"), demos.join("friend3")).unwrap();
    let renamed = describe_playlist(&demos.join("friend3").join("run.ghplaylist"), "friend3");
    assert_eq!(
        s.adapter.can_render(&renamed).unwrap_err().code,
        "PLAYLIST_PATH_ESCAPE"
    );
    assert_eq!(
        s.adapter.estimate_profile(&s.item, None, 60.0),
        ghosting::EstimateProfile {
            key: "ghosting-ghrender",
            launches: 1.0,
            segments: 3.0
        }
    );
}

#[test]
fn mod_without_journal_is_refused_without_fallback() {
    // Молчит, другой протокол (и 1), вышел без журнала: один запуск, без отката на startdemos.
    let cases = [
        (
            Game {
                silent: true,
                ..Game::default()
            },
            "Нет журнала ghrender.log",
        ),
        (
            Game {
                protocol: Some("GHRENDER 1".into()),
                ..Game::default()
            },
            "GHRENDER 1",
        ),
        (
            Game {
                protocol: Some("GHRENDER 9".into()),
                ..Game::default()
            },
            "GHRENDER 9",
        ),
        (
            Game {
                exit_silently: true,
                ..Game::default()
            },
            "Игра вышла без журнала (код 2)",
        ),
    ];
    for (game, message) in cases {
        let game = Game {
            journal: journal_049(),
            total_frames: 590,
            ..game
        };
        let s = Scenario::new(game, &PROTOCOL_2, Duration::from_secs(75), false);
        let s = Scenario {
            adapter: s
                .adapter
                .with_timeouts(Duration::from_millis(300), Duration::from_secs(75)),
            ..s
        };
        let error = s
            .render(&json!({}), &fake_ffmpeg(), &|| false, &|_| {})
            .unwrap_err();
        assert_eq!(
            (error.code.as_str(), error.message.as_str()),
            ("GHRENDER_UNSUPPORTED", message)
        );
        assert_eq!(s.launches().len(), 1, "{message}");
        assert_eq!(*s.game.resets.lock().unwrap(), vec![true], "{message}");
        assert_restored(&s, message);
        assert_eq!(
            s.store.lock().unwrap().get(ghosting::UNMUTE_PENDING),
            None,
            "{message}"
        );
    }
}

#[test]
fn cancel_mid_render_returns_everything() {
    let game = Game {
        journal: journal_049(),
        total_frames: 590,
        line_pause: Some(Duration::from_millis(40)),
        ..Game::default()
    };
    let s = Scenario::new(game, &PROTOCOL_2, Duration::from_secs(75), false);
    let started = SystemTime::now();
    let cancel = || started.elapsed().unwrap() > Duration::from_millis(150);
    let error = s
        .render(&json!({}), &fake_ffmpeg(), &cancel, &|_| {})
        .unwrap_err();
    assert_eq!(error.code, "CANCELLED");
    assert_restored(&s, "отмена");
}

/// Папка с ffmpeg: подмена `HLSR_DEMO_FFMPEG` или PATH (как `tests/ffmpeg_job.rs`).
fn ffmpeg_dir() -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    if let Some(dir) = std::env::var_os("HLSR_DEMO_FFMPEG").map(PathBuf::from) {
        let dir = if dir.is_file() {
            dir.parent()?.to_path_buf()
        } else {
            dir
        };
        return dir.join(exe).is_file().then_some(dir);
    }
    std::env::split_paths(&std::env::var_os("PATH")?).find(|dir| dir.join(exe).is_file())
}

#[test]
fn real_ffmpeg_takes_game_frames() {
    let Some(dir) = ffmpeg_dir() else {
        eprintln!("ПРОПУЩЕНО: нет FFmpeg — задайте HLSR_DEMO_FFMPEG или положите ffmpeg в PATH");
        return;
    };
    let exe = |name: &str| {
        dir.join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_owned()
        })
    };
    let game = Game {
        journal: journal_049(),
        total_frames: 590,
        ..Game::default()
    };
    let s = Scenario::new(game, &PROTOCOL_2, Duration::from_secs(75), true);
    let located = Located {
        ffmpeg: exe("ffmpeg"),
        ffprobe: exe("ffprobe"),
        source: "path",
    };
    let material = s.render(&json!({}), &located, &|| false, &|_| {}).unwrap();
    let Source::Video(video) = &material.video else {
        panic!("кадры вместо видео");
    };
    let probe = std::process::Command::new(&located.ffprobe)
        .args([
            "-v",
            "error",
            "-count_frames",
            "-select_streams",
            "v:0",
            "-show_entries",
        ])
        .args([
            "stream=nb_read_frames,width,height,codec_name",
            "-of",
            "json",
            video,
        ])
        .output()
        .unwrap();
    let info: Value = serde_json::from_slice(&probe.stdout).unwrap();
    let stream = &info["streams"][0];
    assert_eq!(stream["codec_name"], "h264");
    assert_eq!(
        (stream["width"].as_u64(), stream["height"].as_u64()),
        (Some(8), Some(4))
    );
    assert_eq!(stream["nb_read_frames"], "590");
    assert_eq!(
        material.audio.as_deref().map(Path::new).map(Path::exists),
        Some(true)
    );
    assert_restored(&s, "настоящий FFmpeg");
}
