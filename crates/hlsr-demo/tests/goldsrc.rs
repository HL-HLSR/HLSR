//! Адаптер HL1 (bxt-rs) против 0.4.9 (`tests/fixtures/hl1.json`, `tools/reference/demo/hl1.js`):
//! сегменты, `job.cfg` и аргументы байт в байт, разбор `qconsole.log` и прогресса, вырезанное,
//! метки, KeyHUD в ASS байт в байт, список concat, окружение игры и сам адаптер с подставной
//! игрой (пишет журнал, видео сегментов и прогресс, как самотест 0.4.9), сброс mute и возврат
//! журналов игрока после сбоя. Дальше — строже 0.4.9 (предел по заголовку) и отмена.

mod common;

use hlsr_core::json_text::js_number;
use hlsr_core::logger::MemoryLogger;
use hlsr_core::store::Store;
use hlsr_delivery::ports::FreeSpace;
use hlsr_demo::Marker;
use hlsr_demo::errors::DemoError;
use hlsr_demo::ffmpeg::Located;
use hlsr_demo::game_render::{self, Inspector, Ports, Registry, SteamStatus};
use hlsr_demo::goldsrc::{self, Bxtrs, Hl1Adapter};
use hlsr_demo::goldsrc_demo::{GoldsrcScan, MapChange, Press};
use hlsr_demo::goldsrc_render::{self as gsr, LogLine};
use hlsr_demo::headers::DemoHeader;
use hlsr_demo::jobs::{Adapter, Material, RenderProgress, RenderRequest};
use hlsr_demo::library::{self, Described, Entry, Segment};
use hlsr_demo::planner::{Source, Timing};
use hlsr_demo::render_host::{Host, HostEvent, StartRequest};
use hlsr_demo::source_render::{Cut, Range};
use serde_json::{Map, Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/hl1.json")).unwrap()
}

fn text(value: &Value) -> String {
    value.as_str().unwrap().to_owned()
}

fn num(value: &Value) -> f64 {
    value.as_f64().unwrap()
}

fn range_of(value: &Value) -> Option<Range> {
    (!value.is_null()).then(|| Range {
        from: num(&value["from"]),
        to: num(&value["to"]),
    })
}

/// Сегмент из длительности (`null` — неизвестна) и карты.
fn segment(index: usize, duration: &Value, map: &str) -> Segment {
    Segment {
        index: index + 1,
        name: format!("s{index}"),
        path: String::new(),
        playlist: None,
        header: Ok(DemoHeader {
            engine: "goldsrc",
            duration: duration.as_f64(),
            map: map.to_owned(),
            ..DemoHeader::default()
        }),
        size: 0,
    }
}

fn segments(durations: &Value, maps: Option<&Value>) -> Vec<Segment> {
    durations
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, d)| segment(i, d, maps.map_or("", |m| m[i].as_str().unwrap())))
        .collect()
}

fn scan_of(value: &Value) -> Option<GoldsrcScan> {
    if value.is_null() {
        return None;
    }
    let list = |key: &str| value[key].as_array().cloned().unwrap_or_default();
    Some(GoldsrcScan {
        broken: value["broken"] == true,
        duration: num(&value["duration"]),
        map_changes: list("mapChanges")
            .iter()
            .map(|c| MapChange {
                t: num(&c["t"]),
                map: text(&c["map"]),
                gap: num(&c["gap"]),
            })
            .collect(),
        buttons: list("buttons").iter().map(press_of).collect(),
        start_gap: num(&value["startGap"]),
    })
}

fn press_of(value: &Value) -> Press {
    Press {
        t: num(&value["t"]),
        buttons: value["buttons"].as_u64().unwrap() as u16,
    }
}

fn scans_of(value: &Value) -> Vec<Option<GoldsrcScan>> {
    value.as_array().unwrap().iter().map(scan_of).collect()
}

fn cuts_value(cuts: &[Cut]) -> Value {
    Value::Array(
        cuts.iter()
            .map(|cut| json!({"from": js_number(cut.from), "to": js_number(cut.to)}))
            .collect(),
    )
}

fn markers_value(markers: &[Marker]) -> Value {
    Value::Array(markers.iter().map(Marker::to_value).collect())
}

fn presses_value(presses: &[Press]) -> Value {
    Value::Array(
        presses
            .iter()
            .map(|p| json!({"t": js_number(p.t), "buttons": p.buttons}))
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

// ---------------------------------------------------------------- чистая логика

#[test]
fn constants_and_spec_match_049() {
    let fx = fixture();
    assert_eq!(gsr::RENDER_DIR, fx["renderDir"]);
    assert_eq!(json!(gsr::NET_PROTOCOL), fx["netProtocol"]);
    assert_eq!(js_number(gsr::CAPTURE_VOLUME), fx["captureVolume"]);
    assert_eq!(json!(gsr::MAX_MARKERS), fx["maxMarkers"]);
    let keys: Vec<Value> = gsr::KEYS
        .iter()
        .map(|k| json!({"label": k.label, "bit": k.bit, "x": js_number(k.x), "y": js_number(k.y), "w": js_number(k.w)}))
        .collect();
    assert_eq!(json!(keys), fx["keys"]);
    let spec = &goldsrc::SPEC;
    let caps: Map<String, Value> = spec
        .caps
        .iter()
        .map(|c| ((*c).to_owned(), json!(true)))
        .collect();
    let got = json!({
        "gameId": spec.game_id, "image": spec.image, "registryKey": spec.registry_key, "marker": spec.marker,
        "regFile": spec.reg_file, "unmutePending": spec.unmute_pending, "label": spec.label,
        "otherRunning": spec.other_running, "required": spec.required, "caps": caps,
    });
    assert_eq!(got, fx["spec"]);
    let root = (spec.render_root)(Path::new("C:/g"));
    assert_eq!(root, Path::new("C:/g/Half-Life/valve_WON/hlsr_render"));
}

#[test]
fn plan_segments_like_049() {
    for case in fixture()["plan"].as_array().unwrap() {
        let list = segments(&case["durations"], None);
        let (picked, offset) = gsr::plan_segments(&list, range_of(&case["range"]));
        let got: Vec<Value> = picked
            .iter()
            .map(|p| {
                json!([
                    p.index,
                    js_number(p.start),
                    js_number(p.end),
                    js_number(p.duration)
                ])
            })
            .collect();
        assert_eq!(json!(got), case["picked"], "{case}");
        assert_eq!(js_number(offset), case["offset"], "{case}");
    }
}

#[test]
fn job_config_and_arguments_like_049() {
    let fx = fixture();
    for case in fx["jobConfig"].as_array().unwrap() {
        let input = &case["input"];
        let text = gsr::job_config(
            input["fps"].as_u64().unwrap() as u32,
            input["dir"].as_str().unwrap(),
            input["hud"] != false,
            input["ownHud"] != false,
        );
        assert_eq!(text, case["text"], "{input}");
    }
    for case in fx["arguments"]["game"].as_array().unwrap() {
        let args = gsr::game_arguments(
            case["width"].as_u64().unwrap() as u32,
            case["height"].as_u64().unwrap() as u32,
            case["dir"].as_str().unwrap(),
        );
        assert_eq!(json!(args), case["args"]);
    }
    assert_eq!(json!(gsr::reset_arguments()), fx["arguments"]["reset"]);
}

#[test]
fn log_lines_and_progress_like_049() {
    let fx = fixture();
    for case in fx["parseLogLine"].as_array().unwrap() {
        let got = match gsr::parse_log_line(case["line"].as_str().unwrap()) {
            None => Value::Null,
            Some(LogLine::Playing(index)) => json!({"kind": "playing", "index": index}),
            Some(LogLine::Stopped) => json!({"kind": "stopped"}),
            Some(LogLine::Error { code, text }) => {
                json!({"kind": "error", "code": code, "text": text})
            }
        };
        assert_eq!(got, case["parsed"], "{}", case["line"]);
    }
    for case in fx["progressSeconds"].as_array().unwrap() {
        let got = gsr::progress_seconds(case["text"].as_str().unwrap());
        assert_eq!(
            got.map_or(Value::Null, js_number),
            case["seconds"],
            "{case}"
        );
    }
}

#[test]
fn loss_cuts_and_timeline_like_049() {
    let fx = fixture();
    for case in fx["lossCuts"].as_array().unwrap() {
        let list = segments(&case["durations"], None);
        let (picked, _) = gsr::plan_segments(&list, None);
        let videos: Vec<f64> = case["videos"].as_array().unwrap().iter().map(num).collect();
        let cuts = gsr::loss_cuts(&picked, &videos, &scans_of(&case["scans"]));
        assert_eq!(cuts_value(&cuts), case["cuts"], "{case}");
    }
    for case in fx["timeline"].as_array().unwrap() {
        if case.get("count").is_some() {
            let list = segments(
                &json!([10, 10, 10, 10, 10, 10]),
                Some(&json!(["a", "a", "a", "a", "a", "a"])),
            );
            let flips = GoldsrcScan {
                map_changes: (0..256)
                    .map(|k| MapChange {
                        t: f64::from(k) * 0.01,
                        map: if k % 2 == 1 { "b" } else { "a" }.into(),
                        gap: 0.0,
                    })
                    .collect(),
                ..GoldsrcScan::default()
            };
            let (picked, _) = gsr::plan_segments(&list, None);
            let many = gsr::timeline_of(&picked, &vec![Some(flips); 6], None);
            assert_eq!(json!(many.len()), case["count"]);
            assert_eq!(markers_value(&many[..3]), case["head"]);
            assert_eq!(markers_value(&many[many.len() - 2..]), case["tail"]);
            continue;
        }
        let list = segments(&case["durations"], Some(&case["maps"]));
        let (mut picked, _) = gsr::plan_segments(&list, case.get("picks").and_then(range_of));
        if let Some(slice) = case["slice"].as_u64() {
            picked.truncate(slice as usize);
        }
        let markers =
            gsr::timeline_of(&picked, &scans_of(&case["scans"]), range_of(&case["range"]));
        assert_eq!(markers_value(&markers), case["markers"], "{case}");
    }
}

#[test]
fn keyhud_ass_like_049() {
    let fx = fixture();
    for case in fx["assTime"].as_array().unwrap() {
        assert_eq!(gsr::ass_time(num(&case["seconds"])), case["text"], "{case}");
    }
    for case in fx["keyEvents"].as_array().unwrap() {
        let list = segments(&case["durations"], None);
        let (picked, _) = gsr::plan_segments(&list, None);
        let timing = Timing {
            offset: num(&case["timing"]["offset"]),
            cuts: case["timing"]["cuts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| Cut {
                    from: num(&c["from"]),
                    to: num(&c["to"]),
                })
                .collect(),
        };
        let events = gsr::key_events(&picked, &scans_of(&case["scans"]), &timing);
        assert_eq!(presses_value(&events), case["events"], "{case}");
    }
    for case in fx["keyhud"].as_array().unwrap() {
        let events: Vec<Press> = case["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(press_of)
            .collect();
        let ass = gsr::keyhud_ass(
            &events,
            case["width"].as_u64().unwrap() as u32,
            case["height"].as_u64().unwrap() as u32,
            num(&case["end"]),
        );
        assert_eq!(ass, case["text"], "{}x{}", case["width"], case["height"]);
    }
}

#[test]
fn concat_list_and_game_env_like_049() {
    let fx = fixture();
    for case in fx["concatList"].as_array().unwrap() {
        let files: Vec<String> = case["files"].as_array().unwrap().iter().map(text).collect();
        let durations: Vec<f64> = case["durations"]
            .as_array()
            .unwrap()
            .iter()
            .map(num)
            .collect();
        assert_eq!(goldsrc::concat_list(&files, &durations), case["text"]);
    }
    for case in fx["gameEnv"].as_array().unwrap() {
        let env = case["env"]
            .as_array()
            .unwrap()
            .iter()
            .map(|pair| (text(&pair[0]), text(&pair[1])));
        let out = goldsrc::game_env(env, case["dir"].as_str().unwrap());
        assert_eq!(json!(out), case["out"], "{case}");
    }
}

// ---------------------------------------------------------------- подставная игра

/// Игра за помощником (`FakeHost` самотеста 0.4.9): читает `job.cfg` из `+exec`, пишет в
/// `qconsole.log` строки bxt-rs и движка, видео сегментов в `out/` и прогресс; «Recording
/// stopped.» после каждого сегмента.
struct Game {
    unknown: bool,
    no_bxt: bool,
    silent: bool,
    after_first: Option<String>,
    progress_us: u64,
    segment_ms: u64,
    launches: Mutex<Vec<StartRequest>>,
    resets: Mutex<Vec<bool>>,
    cfg: Mutex<Option<String>>,
    injected: Mutex<Option<Vec<String>>>,
}

impl Default for Game {
    fn default() -> Game {
        Game {
            unknown: false,
            no_bxt: false,
            silent: false,
            after_first: None,
            progress_us: 500_000,
            segment_ms: 40,
            launches: Mutex::default(),
            resets: Mutex::default(),
            cfg: Mutex::default(),
            injected: Mutex::default(),
        }
    }
}

#[derive(Default)]
struct Shared {
    exit: Mutex<Option<u32>>,
    closed: AtomicBool,
    events: Mutex<Vec<HostEvent>>,
    stop: AtomicBool,
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

/// Аргументы командной строки (`argv` самотеста; путей с пробелами здесь нет).
fn argv(line: &str) -> Vec<String> {
    line.split_whitespace()
        .map(|s| s.trim_matches('"').to_owned())
        .collect()
}

fn listing(dir: &Path) -> Option<Vec<String>> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .ok()?
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    Some(names)
}

fn simulate(game: Arc<Game>, shared: Arc<Shared>, args: Vec<String>, hl: PathBuf) {
    let exec = args[args.iter().position(|a| a == "+exec").unwrap() + 1].clone();
    let dir = exec[..exec.rfind('/').unwrap()].to_owned();
    let won = hl.join("valve_WON");
    let job_dir = dir.split('/').fold(won.clone(), |p, part| p.join(part));
    *game.cfg.lock().unwrap() = Some(fs::read_to_string(won.join(&exec)).unwrap());
    *game.injected.lock().unwrap() = listing(Path::new(&args[0]).parent().unwrap());
    let append = |name: &str, line: &str| {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(hl.join(name))
            .unwrap();
        file.write_all(line.as_bytes()).unwrap();
    };
    let say = |line: &str| append("qconsole.log", &format!("{line}\r\n"));
    let stopped = || shared.stop.load(Ordering::SeqCst);
    let hang = || {
        while !stopped() {
            thread::sleep(Duration::from_millis(20));
        }
    };
    if !game.no_bxt {
        append("bxt-rs.log", "found the WON engine inside hl.exe\n");
    }
    if game.no_bxt || game.silent {
        return hang();
    }
    if game.unknown {
        return say("Unknown command: bxt_play_folder");
    }
    let demos: Vec<String> = listing(&job_dir)
        .unwrap()
        .into_iter()
        .filter(|name| name.ends_with(".dem"))
        .collect();
    say(&format!("Playing {} demos.", demos.len()));
    for name in demos {
        say(&format!("Playing demo from {dir}/{name}."));
        fs::create_dir_all(job_dir.join("out")).unwrap();
        fs::write(
            job_dir.join("out").join(name.replace(".dem", ".mp4")),
            "video",
        )
        .unwrap();
        let progress = format!("out_time_us={}\nprogress=continue\n", game.progress_us);
        fs::write(job_dir.join("progress.txt"), progress).unwrap();
        if let Some(what) = &game.after_first {
            match what.as_str() {
                "foreign" => shared
                    .events
                    .lock()
                    .unwrap()
                    .push(HostEvent::Foreign { pid: 991 }),
                "exited" => *shared.exit.lock().unwrap() = Some(0),
                "closed" => shared.closed.store(true, Ordering::SeqCst),
                "mapMissing" => {
                    say("Couldn't find map maps/c1a0d.bsp, server will download the map")
                }
                _ => {}
            }
            return hang();
        }
        thread::sleep(Duration::from_millis(game.segment_ms));
        if stopped() {
            return;
        }
        say("Recording stopped.");
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
        let (args, hl) = (argv(&request.command_line), request.cwd.clone());
        self.thread = Some(thread::spawn(move || simulate(game, shared, args, hl)));
        events.push(HostEvent::Started {
            pid: 4242,
            desktop: String::new(),
        });
        events.push(HostEvent::Muted {
            pid: 4243,
            device: 0,
            was: false,
        });
        Ok(4242)
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
        self.stopped || self.shared.closed.load(Ordering::SeqCst)
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
        let value = self.0.lock().unwrap().get(key).cloned().unwrap_or_default();
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

/// Процессы `hl.exe`: пусто или Steam HL1 игрока.
struct Processes(Vec<Option<String>>);

impl Inspector for Processes {
    fn find_by_image_name(&self, _image: &str) -> Result<Vec<Option<String>>, String> {
        Ok(self.0.clone())
    }

    fn same_path(&self, left: &str, right: &str) -> bool {
        left == right
    }
}

struct Space(u64);

impl FreeSpace for Space {
    fn free_bytes(&self, _dir: &Path) -> Option<u64> {
        Some(self.0)
    }
}

/// Сценарий: игра с Injector и BXT, ран из двух демок, bxt_rs.dll, корень заданий, реестр, Store.
struct Scenario {
    _temp: tempfile::TempDir,
    dir: PathBuf,
    hl: PathBuf,
    jobs_root: PathBuf,
    job_dir: PathBuf,
    item: Described,
    game: Arc<Game>,
    registry: FakeRegistry,
    store: Arc<Mutex<Store>>,
    logger: Arc<MemoryLogger>,
    probes: Arc<Mutex<Vec<Value>>>,
    runs: Arc<Mutex<Vec<Value>>>,
    adapter: Hl1Adapter,
}

const KEY: &str = "HKCU\\Software\\Valve\\Half-Life\\Settings";

impl Scenario {
    fn new(options: &Value, game: Game) -> Scenario {
        let fx = fixture();
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        let install = dir.join("game");
        let hl = install.join("Half-Life");
        let bxt_dir = install.join("Bunnymod XT");
        fs::create_dir_all(hl.join("valve_WON")).unwrap();
        fs::create_dir_all(&bxt_dir).unwrap();
        fs::write(hl.join("hl.exe"), "").unwrap();
        for name in ["Injector.exe", "BunnymodXT.dll", "7za.exe", "version"] {
            fs::write(bxt_dir.join(name), name).unwrap();
        }
        fs::write(hl.join("qconsole.log"), "player log\r\n").unwrap();
        let run = fx["run"].as_array().unwrap();
        let demos = [
            &run[0],
            if options["brokenSecond"] == true {
                &fx["brokenSecond"]
            } else {
                &run[1]
            },
        ];
        let files: Vec<String> = demos
            .iter()
            .enumerate()
            .map(|(k, demo)| {
                let file = dir.join(format!("run_{}.dem", k + 1));
                fs::write(&file, common::goldsrc_frames(demo)).unwrap();
                file.to_string_lossy().into_owned()
            })
            .collect();
        let item = library::describe(&Entry {
            id: "id".into(),
            kind: "segments",
            source: "manual",
            game_id: None,
            name: "run".into(),
            files: files.clone(),
            mtime_ms: 0.0,
            series_missing: None,
        });
        if options["removeSecond"] == true {
            fs::remove_file(&files[1]).unwrap();
        }
        if options["appendFirst"] == true {
            fs::OpenOptions::new()
                .append(true)
                .open(&files[0])
                .unwrap()
                .write_all(b"x")
                .unwrap();
        }
        if options["ffmpegInGame"] == true {
            fs::write(hl.join("ffmpeg.exe"), "").unwrap();
        }
        let dll = dir.join("bxt_rs.dll");
        fs::write(&dll, "bxt-rs").unwrap();
        let sha256 = if options["badSha"] == true {
            "0".repeat(64)
        } else {
            common::sha256(b"bxt-rs")
        };
        let jobs_root = dir.join("jobs");
        let job_dir = jobs_root.join("abcdef0123456789");
        fs::create_dir_all(&job_dir).unwrap();
        let registry = FakeRegistry::default();
        registry
            .0
            .lock()
            .unwrap()
            .insert(KEY.into(), json!({"ScreenWidth": 2560}));
        let store = Arc::new(Mutex::new(Store::open(
            dir.join("demo-tool.json"),
            json!({}),
        )));
        let logger = Arc::new(MemoryLogger::default());
        let game = Arc::new(Game {
            unknown: options["unknown"] == true,
            no_bxt: options["noBxt"] == true,
            silent: options["silent"] == true,
            after_first: options["afterFirst"].as_str().map(str::to_owned),
            ..game
        });
        let factory_game = game.clone();
        let found = install.clone();
        let running = options["gameRunning"] == true;
        let steam = options["steam"].as_str().map(str::to_owned);
        let others = if options["otherHl"] == true {
            vec![Some(
                "D:\\Steam\\steamapps\\common\\Half-Life\\hl.exe".to_owned(),
            )]
        } else {
            Vec::new()
        };
        let ports = Ports {
            windows: true,
            jobs_root: Some(jobs_root.clone()),
            store: Some(store.clone()),
            logger: logger.clone(),
            registry: Box::new(registry.clone()),
            inspector: Box::new(Processes(others)),
            install_path: Box::new(move || Some(found.clone())),
            game_running: Box::new(move |id| running && id == "hl1-won"),
            game_busy: Box::new(|| false),
            steam_status: Box::new(move || SteamStatus {
                ok: steam.is_none(),
                code: steam.clone(),
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
        let space = if options["noSpace"] == true {
            1024
        } else {
            u64::MAX
        };
        let probe = options
            .get("probe")
            .cloned()
            .unwrap_or_else(|| json!({"s001.mp4": "1.950000", "s002.mp4": "2.800000"}));
        let (probes, runs) = (
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
        );
        let (probe_log, run_log) = (probes.clone(), runs.clone());
        let mut adapter = Hl1Adapter::new(
            ports,
            Arc::new(Space(space)),
            Box::new(move || {
                Some(Bxtrs {
                    path: dll.clone(),
                    sha256: Some(sha256.clone()),
                })
            }),
        )
        .with_ffmpeg(
            Box::new(move |exe, args, options| {
                run_log.lock().unwrap().push(json!({
                    "exe": exe.to_string_lossy(), "args": args, "cwd": options.cwd.map(|d| d.to_string_lossy()),
                }));
                fs::write(args.last().unwrap(), "material").unwrap();
                Ok(Default::default())
            }),
            Box::new(move |exe, file, _| {
                probe_log
                    .lock()
                    .unwrap()
                    .push(json!([exe.to_string_lossy(), file.to_string_lossy()]));
                let name = file.file_name().unwrap().to_string_lossy().into_owned();
                Ok(json!({"streams": [{"codec_type": "video", "duration": probe.get(&name)}]}))
            }),
        );
        if options["fast"] == true {
            adapter = adapter.with_timeouts(Duration::from_millis(300), Duration::from_millis(300));
        }
        Scenario {
            _temp: temp,
            dir,
            hl,
            jobs_root,
            job_dir,
            item,
            game,
            registry,
            store,
            logger,
            probes,
            runs,
            adapter,
        }
    }

    /// Строки с путём сценария — `<scenario>`, «/».
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

    /// Текст файла задания: путь сценария и с «\», и с «/».
    fn rel_text(&self, file: &Path) -> Value {
        let dir = self.dir.to_string_lossy().into_owned();
        fs::read_to_string(file).map_or(Value::Null, |text| {
            json!(
                text.replace(&dir, "<scenario>")
                    .replace(&dir.replace('\\', "/"), "<scenario>")
            )
        })
    }

    fn render(
        &self,
        options: &Value,
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
        let ask = &options["request"];
        let ffmpeg = Located {
            ffmpeg: self.dir.join("ffmpeg").join("ffmpeg.exe"),
            ffprobe: self.dir.join("ffmpeg").join("ffprobe.exe"),
            source: "tool",
        };
        self.adapter.render(&RenderRequest {
            item: &self.item,
            range,
            fps: 60,
            width: 1280,
            height: 720,
            audio: ask["audio"] != false,
            keyhud: ask["keyhud"] != false,
            integrity: false,
            hud: ask["hud"] != false,
            own_config: true,
            own_hud: ask["ownHud"] != false,
            job_dir: &self.job_dir,
            ffmpeg: &ffmpeg,
            cancel,
            on_progress: progress,
        })
    }

    fn registry_value(&self) -> Value {
        self.registry
            .0
            .lock()
            .unwrap()
            .get(KEY)
            .cloned()
            .unwrap_or_default()
    }

    fn pending(&self) -> Value {
        let store = self.store.lock().unwrap();
        store
            .get(goldsrc::SPEC.unmute_pending)
            .cloned()
            .unwrap_or_default()
    }

    fn render_dir(&self) -> bool {
        self.hl.join("valve_WON").join("hlsr_render").exists()
    }

    /// Журнал без замеров времени.
    fn logs(&self) -> Value {
        let lines: Vec<Value> = self
            .logger
            .take()
            .into_iter()
            .map(|(level, message)| {
                let message = match message.strip_prefix("[demo] HL1: рендер ") {
                    Some(rest) if rest.ends_with(" с") => "[demo] HL1: рендер <N> с".to_owned(),
                    _ => message,
                };
                json!([level, message])
            })
            .collect();
        self.rel(&Value::Array(lines))
    }

    /// Игроку всё как было: реестр, журналы, папка рендера, маркер и копии журналов.
    fn assert_restored(&self, what: &str) {
        assert_eq!(
            self.registry_value(),
            json!({"ScreenWidth": 2560}),
            "{what}"
        );
        assert_eq!(
            fs::read_to_string(self.hl.join("qconsole.log")).unwrap(),
            "player log\r\n",
            "{what}"
        );
        assert!(!self.hl.join("bxt-rs.log").exists(), "{what}");
        assert!(!self.render_dir(), "{what}");
        assert_eq!(
            listing(&self.jobs_root).unwrap(),
            ["abcdef0123456789"],
            "{what}"
        );
        assert!(!self.adapter.game().is_active(), "{what}");
    }
}

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
        "overlay": material.overlay,
        "warnings": material.warnings,
        "profile": material.profile,
        "launches": js_number(material.launches.unwrap()),
        "segments": js_number(material.segments.unwrap()),
        "frames": js_number(material.frames.unwrap()),
    })
}

#[test]
fn adapter_renders_like_049() {
    for case in fixture()["adapter"].as_array().unwrap() {
        let name = text(&case["name"]);
        let options = &case["options"];
        let s = Scenario::new(options, Game::default());
        assert_eq!(
            js_number(s.item.duration.unwrap()),
            case["duration"],
            "{name}"
        );
        let progress = Mutex::new(Vec::<RenderProgress>::new());
        let result = s.render(options, &|| false, &|info| {
            progress.lock().unwrap().push(info.clone())
        });
        let result = match &result {
            Ok(material) => json!({"material": material_value(material)}),
            Err(error) => json!({"error": error_value(error)}),
        };
        let launches: Vec<Value> = s
            .game
            .launches
            .lock()
            .unwrap()
            .iter()
            .map(|l| {
                json!({"exe": l.exe.to_string_lossy(), "cwd": l.cwd.to_string_lossy(), "commandLine": l.command_line,
                    "mute": l.mute, "unmuteAll": l.unmute_all, "followJob": l.follow_job, "watchImage": l.watch_image,
                    "resetExe": l.reset_exe.as_ref().map(|e| e.to_string_lossy()), "resetCommandLine": l.reset_command_line})
            })
            .collect();
        let env = s
            .game
            .launches
            .lock()
            .unwrap()
            .first()
            .and_then(|l| l.env.clone());
        let env = env.map_or(Value::Null, |env| {
            let get = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
            json!({"pathFirst": get("PATH").unwrap().split(';').next(), "discord": get("BXT_DISABLE_DISCORD_RPC"),
                "vsync": get("BXT_DISABLE_VSYNC")})
        });
        let got = json!({
            "launches": s.rel(&json!(launches)),
            "env": s.rel(&env),
            "cfg": s.game.cfg.lock().unwrap().clone(),
            "injected": s.game.injected.lock().unwrap().clone(),
            "probes": s.rel(&json!(*s.probes.lock().unwrap())),
            "ffmpeg": s.rel(&json!(*s.runs.lock().unwrap())),
            "segmentsTxt": s.rel_text(&s.job_dir.join("segments.txt")),
            "keyhud": s.rel_text(&s.job_dir.join("keyhud.ass")),
            "result": s.rel(&result),
            "resets": s.game.resets.lock().unwrap().clone(),
            "registry": s.registry_value(),
            "qconsole": fs::read_to_string(s.hl.join("qconsole.log")).unwrap(),
            "bxtLog": s.hl.join("bxt-rs.log").exists(),
            "renderDir": s.render_dir(),
            "bxtDir": listing(&s.dir.join("game").join("Bunnymod XT")),
            "jobsRoot": listing(&s.jobs_root),
            "jobFiles": listing(&s.job_dir),
            "pending": s.pending(),
            "logs": s.logs(),
        });
        for key in got.as_object().unwrap().keys() {
            assert_eq!(got[key], case[key], "{name}: {key}");
        }
        // Доля не убывает, номер сегмента — в пределах плана.
        let progress = progress.lock().unwrap();
        for pair in progress.windows(2) {
            assert!(pair[1].fraction >= pair[0].fraction, "{name}");
        }
        for info in progress.iter() {
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
fn can_render_and_estimate_like_049() {
    let s = Scenario::new(&json!({}), Game::default());
    assert_eq!(s.adapter.can_render(&s.item), Ok(()));
    let profile = s.adapter.estimate_profile(&s.item, None);
    assert_eq!(
        (profile.key, profile.launches, profile.segments),
        ("hl1", 1.0, 2.0)
    );
    let caps = s.adapter.game().capabilities();
    assert_eq!((caps.max_height, caps.fps), (1080, vec![30, 60, 120]));
    let code = |item: &Described| s.adapter.can_render(item).unwrap_err().code;
    let mut other = s.item.clone();
    other.game = Some("ghosting");
    assert_eq!(code(&other), "DEMO_UNSUPPORTED");
    let mut steam = s.item.clone();
    for segment in &mut steam.segments {
        segment.header.as_mut().unwrap().net_protocol = 48;
    }
    assert_eq!(code(&steam), "DEMO_PROTOCOL_UNSUPPORTED");
    let mut empty = s.item.clone();
    for segment in &mut empty.segments {
        segment.header.as_mut().unwrap().duration = Some(0.0);
    }
    assert_eq!(code(&empty), "DEMO_EMPTY");
}

#[test]
fn reset_mute_without_export_like_049() {
    let expected = fixture()["resetMute"].clone();
    let s = Scenario::new(&json!({}), Game::default());
    s.store
        .lock()
        .unwrap()
        .set(goldsrc::SPEC.unmute_pending, Value::Bool(true));
    assert_eq!(
        json!(s.adapter.game().mute_reset_blocker()),
        expected["blocker"]
    );
    assert_eq!(json!(s.adapter.reset_mute()), expected["ok"]);
    let launch = s.game.launches.lock().unwrap()[0].clone();
    let got = json!({
        "exe": launch.exe.to_string_lossy(), "cwd": launch.cwd.to_string_lossy(), "commandLine": launch.command_line,
        "resetExe": launch.reset_exe.as_ref().map(|e| e.to_string_lossy()), "resetCommandLine": launch.reset_command_line,
        "resetOnly": launch.reset_only,
    });
    assert_eq!(s.rel(&got), expected["launch"]);
    assert_eq!(s.pending(), expected["pending"]);
    assert_eq!(s.registry_value(), expected["registry"]);
    assert_eq!(json!(s.render_dir()), expected["renderDir"]);
    assert_eq!(s.logs(), expected["logs"]);
}

/// Сбой посреди рендера: маркер с журналами игрока, как его оставил 0.4.9; старт лаунчера —
/// реестр и журналы возвращаются, mute помечается к снятию. Снимок `.reg` пропал — журналы
/// возвращаются один раз, маркер ждёт реестра уже без них.
#[test]
fn recover_after_crash_like_049() {
    const AT: &str = "2026-11-18T00:00:00.000Z";
    for case in fixture()["recover"].as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        let root = dir.join("jobs");
        let hl = dir.join("game").join("Half-Life");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&hl).unwrap();
        fs::write(hl.join("qconsole.log"), "player log\r\n").unwrap();
        fs::write(root.join("hl1-render.qconsole.log.orig"), "player log\r\n").unwrap();
        let registry = FakeRegistry::default();
        registry.0.lock().unwrap().insert(
            KEY.into(),
            json!({"ScreenWidth": 1920, "ScreenWindowed": 0}),
        );
        registry
            .export_key(KEY, &root.join(goldsrc::SPEC.reg_file))
            .unwrap();
        let slash_dir = dir.to_string_lossy().replace('\\', "/");
        let marker = case["marker"]
            .to_string()
            .replace("<scenario>", &slash_dir)
            .replace("<at>", AT);
        let marker: Map<String, Value> = serde_json::from_str(&marker).unwrap();
        game_render::write_marker(&root, &goldsrc::SPEC, &marker).unwrap();
        registry
            .0
            .lock()
            .unwrap()
            .insert(KEY.into(), json!({"ScreenWidth": 800, "ScreenWindowed": 1}));
        fs::OpenOptions::new()
            .append(true)
            .open(hl.join("qconsole.log"))
            .unwrap()
            .write_all(b"Playing demo from hlsr_render/abcdef01/s001.dem.\r\n")
            .unwrap();
        fs::write(hl.join("bxt-rs.log"), "found the WON engine\n").unwrap();
        if case["regLost"] == true {
            fs::remove_file(root.join(goldsrc::SPEC.reg_file)).unwrap();
        }
        let store = Arc::new(Mutex::new(Store::open(
            dir.join("demo-tool.json"),
            json!({}),
        )));
        let logger = Arc::new(MemoryLogger::default());
        let adapter = Hl1Adapter::new(
            Ports {
                windows: true,
                jobs_root: Some(root.clone()),
                store: Some(store.clone()),
                logger: logger.clone(),
                registry: Box::new(registry.clone()),
                inspector: Box::new(Processes(Vec::new())),
                install_path: Box::new(|| None),
                game_running: Box::new(|_| false),
                game_busy: Box::new(|| false),
                steam_status: Box::new(SteamStatus::default),
                screen_size: Box::new(|| None),
                host_factory: Box::new(|| panic!("без игры")),
            },
            Arc::new(Space(u64::MAX)),
            Box::new(|| None),
        );
        adapter.recover();
        let after = game_render::read_marker(&root, &goldsrc::SPEC).map_or(Value::Null, |m| {
            let text = Value::Object(m)
                .to_string()
                .replace(AT, "<at>")
                .replace(&slash_dir, "<scenario>");
            serde_json::from_str(&text).unwrap()
        });
        let logs: Vec<Value> = logger
            .take()
            .into_iter()
            .map(|(level, message)| json!([level, message.replace(AT, "<at>")]))
            .collect();
        let what = format!("regLost {}", case["regLost"]);
        assert_eq!(registry.0.lock().unwrap()[KEY], case["registry"], "{what}");
        assert_eq!(
            store
                .lock()
                .unwrap()
                .get(goldsrc::SPEC.unmute_pending)
                .cloned()
                .unwrap_or_default(),
            case["pending"],
            "{what}"
        );
        assert_eq!(
            fs::read_to_string(hl.join("qconsole.log")).unwrap(),
            case["qconsole"],
            "{what}"
        );
        assert_eq!(
            json!(hl.join("bxt-rs.log").exists()),
            case["bxtLog"],
            "{what}"
        );
        assert_eq!(after, case["markerAfter"], "{what}");
        assert_eq!(json!(listing(&root)), case["root"], "{what}");
        assert_eq!(json!(logs), case["logs"], "{what}");
        // Игрок после сбоя снова поиграл: следующий старт его журнал не трогает.
        fs::write(hl.join("qconsole.log"), "new player log\r\n").unwrap();
        adapter.recover();
        assert_eq!(
            fs::read_to_string(hl.join("qconsole.log")).unwrap(),
            "new player log\r\n"
        );
    }
}

// ---------------------------------------------------------------- строже 0.4.9

/// Видео сегмента длиннее демки по заголовку (подделанная демка играет дольше, чем обещает
/// заголовок) — рендер останавливается, игроку всё возвращено.
#[test]
fn render_is_limited_by_header_duration() {
    let s = Scenario::new(
        &json!({"afterFirst": "stall"}),
        Game {
            progress_us: 4_100_000,
            ..Game::default()
        },
    );
    let error = s.render(&json!({}), &|| false, &|_| {}).unwrap_err();
    assert_eq!(error.code, "RENDER_FAILED");
    assert!(
        error.message.contains("самая длинная демка — 3 с"),
        "{}",
        error.message
    );
    assert_eq!(*s.game.resets.lock().unwrap(), [true]);
    s.assert_restored("предел");
    // Ровно на пределе (+1 с) — ещё не отказ: ждём конца сегмента (здесь — застой).
    let s = Scenario::new(
        &json!({"afterFirst": "stall", "fast": true}),
        Game {
            progress_us: 4_000_000,
            ..Game::default()
        },
    );
    let error = s.render(&json!({}), &|| false, &|_| {}).unwrap_err();
    assert_eq!(error.code, "RENDER_TIMEOUT");
    s.assert_restored("на пределе");
    // Последний сегмент кончился раньше, чем progress.txt дошёл до предела: сверка по ffprobe.
    let s = Scenario::new(
        &json!({"probe": {"s001.mp4": "1.950000", "s002.mp4": "4.100000"}}),
        Game::default(),
    );
    let error = s.render(&json!({}), &|| false, &|_| {}).unwrap_err();
    assert_eq!(error.code, "RENDER_FAILED");
    assert!(
        error.message.contains("видео 4.1 с при демке 3 с"),
        "{}",
        error.message
    );
    s.assert_restored("предел ffprobe");
}

#[test]
fn cancel_mid_render_returns_everything() {
    let s = Scenario::new(
        &json!({}),
        Game {
            segment_ms: 2000,
            ..Game::default()
        },
    );
    let cancelled = AtomicBool::new(false);
    let error = thread::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(Duration::from_millis(300));
            cancelled.store(true, Ordering::SeqCst);
        });
        s.render(&json!({}), &|| cancelled.load(Ordering::SeqCst), &|_| {})
            .unwrap_err()
    });
    assert_eq!(error.code, "CANCELLED");
    s.assert_restored("отмена");
}
