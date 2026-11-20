//! Сверка плана и заданий «Демо» с 0.4.9: `tests/fixtures/plan.json`
//! (`tools/reference/demo/plan.js`). Аргументы FFmpeg — байт в байт, задание — с подставными
//! адаптером, FFmpeg и ffprobe, как в эталоне.

use hlsr_core::js::to_fixed;
use hlsr_core::js::value::to_number;
use hlsr_core::json_text::{js_number, parse_js_json};
use hlsr_demo::Marker;
use hlsr_demo::errors::DemoError;
use hlsr_demo::estimate::{self, Input, Stats};
use hlsr_demo::ffmpeg::{Located, RunOptions, RunOutput, probe_args};
use hlsr_demo::jobs::{
    self, Adapter, JobKind, JobOptions, Material, Progress, RenderProgress, RenderRequest,
    SegmentProgress,
};
use hlsr_demo::library::{Described, Entry, Segment};
use hlsr_demo::planner::{
    self, Capabilities, Clip, EncodeMaterial, Overlay, Plan, PlanOptions, Settings, Source, Trim,
};
use hlsr_demo::source_render::{Cut, Range};
use serde_json::{Map, Value, json};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn fixture() -> &'static Value {
    static FIXTURE: OnceLock<Value> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/plan.json"
        ))
        .unwrap();
        parse_js_json(&text).unwrap()
    })
}

fn f(value: &Value) -> f64 {
    value.as_f64().unwrap()
}

fn opt_f(value: Option<&Value>) -> Option<f64> {
    value.filter(|v| !v.is_null()).map(f)
}

fn text(value: &Value) -> String {
    value.as_str().unwrap().to_owned()
}

fn numbers(list: &[f64]) -> Value {
    Value::Array(list.iter().copied().map(js_number).collect())
}

/// Ошибка как в эталоне: `{code, ...details}` (и `message`, если он записан).
fn error_value(error: &DemoError, with_message: bool) -> Value {
    let mut out = Map::new();
    out.insert("code".to_owned(), json!(error.code));
    if with_message {
        out.insert("message".to_owned(), json!(error.message));
    }
    out.extend(error.details.clone());
    Value::Object(out)
}

fn check<T>(case: &Value, got: Result<T, DemoError>, to_value: impl Fn(T) -> Value) {
    match got {
        Ok(value) => assert_eq!(to_value(value), case["out"], "{case}"),
        Err(error) => assert_eq!(error_value(&error, false), case["error"], "{case}"),
    }
}

fn kind_of(name: &str) -> &'static str {
    Box::leak(name.to_owned().into_boxed_str())
}

fn markers_of(list: &Value) -> Vec<Marker> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|m| Marker {
            t: to_number(m.get("t")),
            kind: kind_of(m["type"].as_str().unwrap()),
            label: text(&m["label"]),
        })
        .collect()
}

fn cuts_of(list: Option<&Value>) -> Vec<Cut> {
    list.and_then(Value::as_array)
        .map(|cuts| {
            cuts.iter()
                .map(|c| Cut {
                    from: f(&c["from"]),
                    to: f(&c["to"]),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn cuts_value(cuts: &[Cut]) -> Value {
    Value::Array(
        cuts.iter()
            .map(|c| json!({"from": js_number(c.from), "to": js_number(c.to)}))
            .collect(),
    )
}

fn clip_of(opts: &Value) -> Clip {
    Clip {
        from: f(&opts["from"]),
        to: f(&opts["to"]),
        speed: f(&opts["speed"]),
        offset: opt_f(opts.get("offset")),
        cuts: cuts_of(opts.get("cuts")),
    }
}

#[test]
fn settings() {
    for case in fixture()["settings"].as_array().unwrap() {
        let got = planner::normalize_settings(
            &case["raw"],
            opt_f(case.get("duration")),
            case["allowPreview"] == true,
        );
        check(case, got, |s| s.to_value());
    }
}

#[test]
fn atempo_and_audio_filter() {
    let filters = &fixture()["filters"];
    for case in filters["atempo"].as_array().unwrap() {
        assert_eq!(
            numbers(&planner::atempo_chain(f(&case["speed"]))),
            case["chain"],
            "{case}"
        );
    }
    for case in filters["audio"].as_array().unwrap() {
        assert_eq!(
            planner::audio_filter(f(&case["speed"])),
            case["filter"],
            "{case}"
        );
    }
}

#[test]
fn timeline_and_cuts() {
    let section = &fixture()["timeline"];
    for case in section["cuts"].as_array().unwrap() {
        assert_eq!(
            cuts_value(&planner::normalize_cuts(&cuts_of(Some(&case["cuts"])))),
            case["out"],
            "{case}"
        );
    }
    let markers = markers_of(&section["markers"]);
    for case in section["maps"].as_array().unwrap() {
        let got: Vec<Value> = planner::map_timeline(&markers, &clip_of(&case["opts"]))
            .iter()
            .map(|m| m.to_value())
            .collect();
        assert_eq!(Value::Array(got), case["out"], "{case}");
    }
    for case in section["clips"].as_array().unwrap() {
        assert_eq!(
            js_number(planner::clip_duration(&clip_of(&case["opts"]))),
            case["out"],
            "{case}"
        );
    }
}

#[test]
fn combos() {
    for case in fixture()["combos"].as_array().unwrap() {
        let caps = Capabilities {
            max_height: case["caps"]["maxHeight"].as_u64().unwrap() as u32,
            fps: case["caps"]["fps"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u32)
                .collect(),
        };
        let deny: Vec<(u32, u32)> = case["deny"]
            .as_array()
            .unwrap()
            .iter()
            .map(|pair| {
                (
                    pair[0].as_u64().unwrap() as u32,
                    pair[1].as_u64().unwrap() as u32,
                )
            })
            .collect();
        let supports = |height: u32, fps: u32| !deny.contains(&(height, fps));
        let got: Vec<Value> = planner::allowed_combos(&caps, Some(&supports))
            .iter()
            .map(|c| json!({"height": c.height, "width": c.width, "fps": c.fps}))
            .collect();
        assert_eq!(Value::Array(got), case["out"], "{case}");
    }
}

fn plan_options(opts: &Value) -> PlanOptions {
    PlanOptions {
        encoder: opts
            .get("encoder")
            .and_then(Value::as_str)
            .map(str::to_owned),
        output_duration: f(&opts["outputDuration"]),
        limit_bytes: opt_f(opts.get("limitBytes")),
        warnings: opts
            .get("warnings")
            .and_then(Value::as_array)
            .map(|w| w.iter().map(text).collect())
            .unwrap_or_default(),
    }
}

fn plan_of(index: usize) -> Result<Plan, DemoError> {
    let spec = &fixture()["planner"]["plans"][index];
    let settings = planner::normalize_settings(
        &spec["raw"],
        opt_f(spec.get("duration")),
        spec.get("allowPreview") == Some(&json!(true)),
    )?;
    planner::plan_export(&settings, &plan_options(&spec["opts"]))
}

/// План эталона с подменой полей (`Object.assign(plan, override)`).
fn plan_with(case: &Value) -> Plan {
    let mut plan = plan_of(case["plan"].as_u64().unwrap() as usize).unwrap();
    let over = &case["override"];
    if let Some(encoder) = over.get("encoder").and_then(Value::as_str) {
        plan.encoder = encoder.to_owned();
    }
    if let Some(two_pass) = over.get("twoPass").and_then(Value::as_bool) {
        plan.two_pass = two_pass;
    }
    if let Some(bps) = over.get("videoBps") {
        plan.video_bps = Some(f(bps));
    }
    if let Some(speed) = over.get("speed") {
        plan.speed = f(speed);
    }
    plan
}

#[test]
fn plans_and_retries() {
    let section = &fixture()["planner"];
    for (index, case) in section["plans"].as_array().unwrap().iter().enumerate() {
        check(case, plan_of(index), |plan| plan.to_value());
    }
    for case in section["retries"].as_array().unwrap() {
        let plan = plan_of(case["plan"].as_u64().unwrap() as usize).unwrap();
        let got = planner::discord_retry(
            &plan,
            f(&case["actualBytes"]),
            case["attempt"].as_u64().unwrap() as u32,
        );
        check(case, got, js_number);
    }
    for case in section["minVideoBps"].as_array().unwrap() {
        let [w, h, fps] = ["width", "height", "fps"].map(|key| case[key].as_u64().unwrap() as u32);
        assert_eq!(
            js_number(planner::min_video_bps(w, h, fps)),
            case["out"],
            "{case}"
        );
    }
}

#[test]
fn ffmpeg_args_byte_for_byte() {
    let section = &fixture()["planner"];
    for case in section["codec"].as_array().unwrap() {
        let plan = plan_with(case);
        let got = planner::video_codec_args(
            &plan,
            case["pass"].as_u64().unwrap() as u8,
            case["passlog"].as_str().unwrap(),
        );
        check(case, got, |args| json!(args));
    }
    for case in section["filters"].as_array().unwrap() {
        let overlay = case["overlay"].as_object().map(|o| Overlay {
            file: text(&o["file"]),
            start: f(&o["start"]),
        });
        assert_eq!(
            planner::video_filter(&plan_with(case), overlay.as_ref()),
            case["out"],
            "{case}"
        );
    }
    let materials = &section["materials"];
    for case in section["encodes"].as_array().unwrap() {
        let m = &materials[case["material"].as_str().unwrap()];
        let source = match m.get("pattern") {
            Some(pattern) => Source::Frames {
                pattern: text(pattern),
                fps: f(&m["fps"]),
                start_number: opt_f(m.get("startNumber")),
            },
            None => Source::Video(text(&m["video"])),
        };
        let material = EncodeMaterial {
            source,
            audio: m.get("audio").and_then(Value::as_str).map(str::to_owned),
            overlay: m.get("overlay").and_then(Value::as_str).map(str::to_owned),
        };
        let trim = Trim {
            start: f(&case["trim"]["start"]),
            duration: f(&case["trim"]["duration"]),
        };
        let got = planner::encode_args(
            &material,
            trim,
            &plan_with(case),
            "C:/job/out.mp4",
            case["pass"].as_u64().unwrap() as u8,
            "C:/job/pass",
            case.get("nullTarget")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
        .unwrap();
        assert_eq!(json!(got), case["out"], "{case}");
    }
    for case in fixture()["ffmpeg"]["probe"].as_array().unwrap() {
        let [w, h, fps] = ["width", "height", "fps"].map(|key| case[key].as_u64().unwrap() as u32);
        assert_eq!(
            json!(probe_args(case["encoder"].as_str().unwrap(), w, h, fps)),
            case["out"],
            "{case}"
        );
    }
    assert_eq!(
        fixture()["ffmpeg"]["pinned"],
        hlsr_demo::ffmpeg::PINNED_VERSION
    );
}

fn input_of(value: &Value) -> Input {
    let optional = |key: &str| value.get(key).map(|v| to_number(Some(v)));
    Input {
        profile: text(&value["profile"]),
        launches: optional("launches"),
        segments: optional("segments"),
        seconds: to_number(value.get("seconds")),
        fps: f(&value["fps"]),
        width: f(&value["width"]),
        height: f(&value["height"]),
        output_seconds: to_number(value.get("outputSeconds")),
        preset: text(&value["preset"]),
        gpu: value["gpu"] == true,
    }
}

fn stats_of(value: &Value) -> Option<Stats> {
    value.as_object()?;
    Some(Stats {
        profile: value["profile"].as_str().map(str::to_owned),
        launches: f(&value["launches"]),
        segments: opt_f(value.get("segments")),
        frames: f(&value["frames"]),
        render_seconds: f(&value["renderSeconds"]),
        height: f(&value["height"]),
        preset: text(&value["preset"]),
        gpu: value["gpu"] == true,
        output_frames: f(&value["outputFrames"]),
        encode_seconds: f(&value["encodeSeconds"]),
    })
}

#[test]
fn estimates_and_learning() {
    let section = &fixture()["estimate"];
    let learned = section["learned"].as_array().unwrap();
    for case in section["estimate"].as_array().unwrap() {
        let table = &learned[case["learned"].as_u64().unwrap() as usize];
        assert_eq!(
            estimate::estimate(&input_of(&case["input"]), table).to_value(),
            case["out"],
            "{case}"
        );
    }
    let stats = section["stats"].as_array().unwrap();
    let mut table = Value::Null;
    for case in section["learn"].as_array().unwrap() {
        if case["stats"] == 0 {
            table = learned[case["learned"].as_u64().unwrap() as usize].clone();
        }
        let out = estimate::learn(
            &table,
            stats_of(&stats[case["stats"].as_u64().unwrap() as usize]).as_ref(),
        );
        assert_eq!(out, case["out"], "{case}");
        table = out;
    }
    let big = Stats {
        profile: Some("hl1".to_owned()),
        launches: 1.0,
        segments: Some(1.0),
        frames: 30000.0,
        render_seconds: 1010.0,
        height: 1080.0,
        preset: "quality".to_owned(),
        gpu: true,
        output_frames: 0.0,
        encode_seconds: 0.0,
    };
    let previous = json!({"render": {"hl1@1080": {"rate": 10, "frames": 100000}}});
    assert_eq!(estimate::learn(&previous, Some(&big)), section["learnBig"]);
    assert_eq!(estimate::encode_kind("discord", true), "discord-gpu");
}

fn settings_with(range: &Value, speed: f64, preset: &str) -> Settings {
    let mut settings = planner::normalize_settings(&json!({}), Some(1e9), false).unwrap();
    settings.range = Range {
        from: f(&range["from"]),
        to: f(&range["to"]),
    };
    settings.speed = speed;
    settings.preset = preset.to_owned();
    settings
}

#[test]
fn output_names() {
    let names = &fixture()["names"];
    for case in names["safe"].as_array().unwrap() {
        assert_eq!(
            jobs::safe_base_name(case["name"].as_str().unwrap()),
            case["out"],
            "{case}"
        );
    }
    for case in names["output"].as_array().unwrap() {
        let s = &case["settings"];
        let settings = settings_with(&s["range"], f(&s["speed"]), s["preset"].as_str().unwrap());
        let got = jobs::output_base_name(
            case["item"]["name"].as_str().unwrap(),
            opt_f(case["item"].get("duration")),
            &settings,
        );
        assert_eq!(got, case["out"], "{case}");
    }
}

#[test]
fn save_output_without_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let source = dir.path().join("source.mp4");
    std::fs::write(&source, "mp4").unwrap();
    std::fs::write(out.join("run.timeline.json"), "{}").unwrap();
    std::fs::write(out.join("run (2).mp4"), "busy").unwrap();
    let timeline = json!({
        "version": 1,
        "source": {"id": "abc", "files": ["a.dem"], "engine": "goldsrc", "game": "hl1", "map": null},
        "video": {"file": "output.mp4", "duration": 1.5, "fps": 30, "width": 1280, "height": 720, "speed": 0.75,
            "range": {"from": 0.25, "to": 1.375}, "encoder": "libx264"},
        "markers": [{"t": 0, "type": "map", "label": "c1a0", "sourceTime": 0.25},
            {"t": 1.333, "type": "timer", "label": "Финиш «1:02»", "sourceTime": 1.25}],
    });
    let expected = &fixture()["save"];
    for case in expected["saved"].as_array().unwrap() {
        let saved = jobs::save_output(&out, "run", &source, &timeline).unwrap();
        assert_eq!(
            saved.file.file_name().unwrap().to_str().unwrap(),
            case["file"]
        );
        assert_eq!(
            saved.timeline_file.file_name().unwrap().to_str().unwrap(),
            case["timelineFile"]
        );
        assert_eq!(
            std::fs::read_to_string(&saved.timeline_file).unwrap(),
            case["text"]
        );
    }
    let mut files: Vec<String> = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    files.sort();
    assert_eq!(json!(files), expected["files"]);
}

// ---------------------------------------------------------------- задание на подставных

fn described(item: &Value) -> Described {
    let segments: Vec<Segment> = item["segments"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(index, s)| Segment {
            index: index + 1,
            name: text(&s["name"]),
            path: text(&s["name"]),
            playlist: None,
            header: Err("DEMO_UNREADABLE"),
            size: 0,
        })
        .collect();
    let leak = |key: &str| item.get(key).and_then(Value::as_str).map(kind_of);
    Described {
        entry: Entry {
            id: text(&item["id"]),
            kind: "segments",
            source: "game",
            game_id: None,
            name: text(&item["name"]),
            files: Vec::new(),
            mtime_ms: 0.0,
            series_missing: None,
        },
        engine: leak("engine"),
        game: leak("game"),
        map: item.get("map").and_then(Value::as_str).map(str::to_owned),
        maps: Vec::new(),
        duration: opt_f(item.get("duration")),
        supported: true,
        reason: None,
        segments,
        markers: Vec::new(),
        size: 0,
        incomplete: false,
        run_time: None,
        source_files: item
            .get("sourceFiles")
            .and_then(Value::as_array)
            .map(|l| l.iter().map(text).collect()),
        failed_segment: None,
        warnings: None,
    }
}

/// Подставной адаптер эталона: сырьё — пустышки в папке задания; прогресс 0,5, 0,75 и 1.
struct Stub {
    material: Value,
    outside: PathBuf,
}

impl Adapter for Stub {
    fn render(&self, request: &RenderRequest) -> Result<Material, DemoError> {
        let progress = |fraction: f64, index: u32, message: Option<&str>| {
            (request.on_progress)(&RenderProgress {
                fraction,
                segment: Some(SegmentProgress { index, count: 2 }),
                message: message.map(str::to_owned),
            });
        };
        progress(0.5, 1, None);
        progress(0.75, 1, None);
        progress(1.0, 2, Some("done"));
        let m = &self.material;
        let place = |key: &str, create: bool| -> Option<String> {
            let value = m.get(key)?.as_str()?;
            let file = if value == "<outside>" {
                self.outside.clone()
            } else {
                request.job_dir.join(value)
            };
            if create && !value.starts_with("missing") && value != "<outside>" {
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(&file, "raw").unwrap();
            }
            Some(file.to_string_lossy().into_owned())
        };
        let video = match place("pattern", false) {
            Some(pattern) => Source::Frames {
                pattern,
                fps: f(&m["fps"]),
                start_number: opt_f(m.get("startNumber")),
            },
            None => Source::Video(place("video", true).unwrap()),
        };
        let overlay = m.get("overlay").and_then(Value::as_str).map(str::to_owned);
        if let Some(name) = &overlay
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
            && !name.starts_with("missing")
        {
            std::fs::write(request.job_dir.join(name), "ass").unwrap();
        }
        let deep = request.job_dir.join("frames-left").join("deep");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("f_0001.tga"), "tga").unwrap();
        Ok(Material {
            audio: place("audio", true),
            offset: opt_f(m.get("offset")),
            timeline: m.get("timeline").map(markers_of).unwrap_or_default(),
            cuts: cuts_of(m.get("cuts")),
            overlay,
            warnings: m
                .get("warnings")
                .and_then(Value::as_array)
                .map(|w| w.iter().map(text).collect())
                .unwrap_or_default(),
            profile: m.get("profile").and_then(Value::as_str).map(str::to_owned),
            launches: opt_f(m.get("launches")),
            segments: opt_f(m.get("segments")),
            frames: opt_f(m.get("frames")),
            ..Material::new(video)
        })
    }
}

fn listing(dir: &Path) -> Value {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Value::Null;
    };
    let mut names: Vec<String> = entries
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    json!(names)
}

fn probe_for(kind: &str, plan: &Plan, with_audio: bool, seconds: f64) -> Result<Value, DemoError> {
    Ok(match kind {
        "good" => {
            let mut streams = vec![
                json!({"codec_type": "video", "codec_name": "h264", "avg_frame_rate": format!("{}/1", plan.fps),
                "width": plan.width, "height": plan.height, "duration": to_fixed(seconds, 6)}),
            ];
            if with_audio {
                streams.push(
                    json!({"codec_type": "audio", "codec_name": "aac", "duration": "99.000000"}),
                );
            }
            json!({"streams": streams, "format": {"duration": "99.000000"}})
        }
        "short" => json!({"streams": [
            {"codec_type": "video", "codec_name": "h264", "avg_frame_rate": "30/1", "width": 1280, "height": 720, "duration": "2.000000"},
            {"codec_type": "audio", "codec_name": "aac", "duration": "10.000000"}], "format": {"duration": "10.000000"}}),
        "wrong" => json!({"streams": [
            {"codec_type": "video", "codec_name": "hevc", "avg_frame_rate": "60/1", "width": 1920, "height": 1080, "duration": "10.4"},
            {"codec_type": "audio", "codec_name": "mp3"}]}),
        "empty" => json!({}),
        "missing" => {
            return Err(DemoError {
                message: "нет ffprobe".to_owned(),
                ..DemoError::new("FFMPEG_MISSING")
            });
        }
        "broken" => {
            return Err(DemoError {
                message: "ffprobe упал".to_owned(),
                ..DemoError::new("FFMPEG_FAILED")
            });
        }
        other => panic!("{other}"),
    })
}

#[test]
fn jobs_with_stubs() {
    let fx = fixture();
    for case in fx["jobs"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let rel = |value: &str| {
            value
                .replace(&*root.to_string_lossy(), "<root>")
                .replace('\\', "/")
        };
        let jobs_root = root.join("jobs");
        let output_dir = root.join("Videos");
        let outside = root.join("outside.mkv");
        std::fs::write(&outside, "x").unwrap();
        let item = described(&fx["items"][case["item"].as_str().unwrap()]);
        let settings =
            planner::normalize_settings(&case["raw"], item.duration, case["allowPreview"] == true)
                .unwrap();
        assert_eq!(settings.to_value(), case["settings"], "{name}");
        let clip = Clip {
            from: settings.range.from,
            to: settings.range.to,
            speed: settings.speed,
            offset: None,
            cuts: Vec::new(),
        };
        let plan = planner::plan_export(
            &settings,
            &PlanOptions {
                encoder: Some(text(&case["encoder"])),
                output_duration: planner::clip_duration(&clip),
                limit_bytes: opt_f(case.get("limitBytes")),
                warnings: Vec::new(),
            },
        )
        .unwrap();
        assert_eq!(plan.to_value(), case["plan"], "{name}");

        let material = &case["material"];
        let with_audio = settings.audio && material.get("audio").is_some_and(|a| !a.is_null());
        let sizes: Vec<u64> = case["sizes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .collect();
        let calls = RefCell::new(Vec::new());
        let progress = RefCell::new(Vec::new());
        let encodes = Cell::new(0usize);
        let seconds = Cell::new(0.0);
        let run_fn =
            |exe: &Path, args: &[String], options: &RunOptions| -> Result<RunOutput, DemoError> {
                calls.borrow_mut().push(json!({
                    "exe": rel(&exe.to_string_lossy()),
                    "args": args.iter().map(|a| rel(a)).collect::<Vec<_>>(),
                    "cwd": options.cwd.map(|cwd| rel(&cwd.to_string_lossy())),
                }));
                let at = args.iter().rposition(|a| a == "-t").unwrap();
                seconds.set(args[at + 1].parse().unwrap());
                if args.iter().any(|a| a == "-movflags") {
                    let size = sizes[encodes.get().min(sizes.len() - 1)];
                    std::fs::write(args.last().unwrap(), vec![0u8; size as usize]).unwrap();
                    encodes.set(encodes.get() + 1);
                }
                Ok(RunOutput::default())
            };
        let ffprobe_fn = |exe: &Path, file: &Path, _: Option<&dyn Fn() -> bool>| {
            calls.borrow_mut().push(json!({"exe": rel(&exe.to_string_lossy()), "args": ["<ffprobe>", rel(&file.to_string_lossy())]}));
            probe_for(
                case["probe"].as_str().unwrap(),
                &plan,
                with_audio,
                seconds.get(),
            )
        };
        let on_progress = |p: &Progress| progress.borrow_mut().push(p.to_value());
        let extra = if case["extra"] == true {
            markers_of(&fx["extraMarkers"])
        } else {
            Vec::new()
        };
        let located = Located {
            ffmpeg: "C:/ff/ffmpeg.exe".into(),
            ffprobe: "C:/ff/ffprobe.exe".into(),
            source: "env",
        };
        let kind = if case["kind"] == "preview" {
            JobKind::Preview
        } else {
            JobKind::Export
        };
        let adapter = Stub {
            material: material.clone(),
            outside: outside.clone(),
        };
        let outcome = jobs::run_job(&JobOptions {
            id: "job-1",
            kind,
            item: &item,
            adapter: &adapter,
            settings: &settings,
            plan: &plan,
            root: &jobs_root,
            output_dir: (kind == JobKind::Export).then_some(output_dir.as_path()),
            extra_markers: &extra,
            ffmpeg: &located,
            cancel: &|| false,
            on_progress: Some(&on_progress),
            on_spawn: None,
            logger: None,
            run_fn: Some(&run_fn),
            ffprobe_fn: Some(&ffprobe_fn),
        });
        match outcome {
            Ok(result) => {
                let expected = &case["result"];
                assert_eq!(result.kind.as_str(), expected["kind"], "{name}");
                assert_eq!(json!(result.size), expected["size"], "{name}");
                assert_eq!(json!(result.attempts), expected["attempts"], "{name}");
                assert_eq!(result.encoder, expected["encoder"], "{name}");
                assert_eq!(
                    result.video_bps.map(js_number),
                    expected["videoBps"].as_f64().map(js_number),
                    "{name}"
                );
                assert_eq!(json!(result.warnings), expected["warnings"], "{name}");
                let mut stats = result.stats.to_value();
                stats.as_object_mut().unwrap().shift_remove("renderSeconds");
                stats.as_object_mut().unwrap().shift_remove("encodeSeconds");
                assert_eq!(stats, expected["stats"], "{name}");
                assert_eq!(
                    rel(&result.file.to_string_lossy()),
                    expected["file"],
                    "{name}"
                );
                assert_eq!(
                    result.dir.map(|d| rel(&d.to_string_lossy())),
                    expected["dir"].as_str().map(str::to_owned),
                    "{name}"
                );
                assert_eq!(
                    result
                        .timeline_file
                        .as_ref()
                        .map(|t| rel(&t.to_string_lossy())),
                    expected["timelineFile"].as_str().map(str::to_owned),
                    "{name}"
                );
                assert_eq!(result.timeline, expected["timeline"], "{name}");
                if let Some(file) = &result.timeline_file {
                    assert_eq!(
                        std::fs::read_to_string(file).unwrap(),
                        expected["timelineText"],
                        "{name}"
                    );
                }
                assert!(case["error"].is_null(), "{name}: эталон упал");
            }
            Err(error) => assert_eq!(error_value(&error, true), case["error"], "{name}"),
        }
        assert_eq!(json!(*calls.borrow()), case["calls"], "{name}");
        assert_eq!(json!(*progress.borrow()), case["progress"], "{name}");
        assert_eq!(listing(&jobs_root), case["jobs"], "{name}");
        assert_eq!(listing(&jobs_root.join("job-1")), case["jobDir"], "{name}");
        assert_eq!(listing(&output_dir), case["videos"], "{name}");
    }
}

// ---------------------------------------------------------------- корень заданий (самотест 0.4.9)

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

#[cfg(unix)]
fn junction(link: &Path, target: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[test]
fn jobs_root_refuses_links_and_files() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("keep.txt"), "x").unwrap();
    let link = dir.path().join("link-root");
    junction(&link, &outside);
    fn code<T>(result: Result<T, DemoError>) -> String {
        result.map(drop).unwrap_err().code
    }
    assert_eq!(code(jobs::check_root(&link)), "JOBS_DIR_UNSAFE");
    assert_eq!(code(jobs::prepare_root(Some(&link))), "JOBS_DIR_UNSAFE");
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3 * 24 * 3600);
    assert_eq!(
        code(jobs::cleanup_stale(&link, later, jobs::STALE)),
        "JOBS_DIR_UNSAFE"
    );
    assert!(
        outside.join("keep.txt").exists(),
        "по ссылке-корню ничего не удалено"
    );

    let file_root = dir.path().join("file-root");
    std::fs::write(&file_root, "").unwrap();
    assert!(code(jobs::prepare_root(Some(&file_root))).starts_with("JOBS_DIR_"));
    let real = dir.path().join("real");
    std::fs::create_dir(&real).unwrap();
    assert_eq!(jobs::check_root(&real).unwrap(), real);
    assert_eq!(code(jobs::prepare_root(None)), "JOBS_DIR_UNAVAILABLE");
    assert_eq!(
        jobs::cleanup_stale(&dir.path().join("missing"), later, jobs::STALE).unwrap(),
        0
    );
}

#[test]
fn cleanup_keeps_link_targets_and_root_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(jobs::JOBS_DIR);
    std::fs::create_dir(&root).unwrap();
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(outside.join("deep")).unwrap();
    std::fs::write(outside.join("deep").join("keep.txt"), "x").unwrap();
    junction(&root.join("old-link"), &outside);
    let job = root.join("old-job");
    std::fs::create_dir(&job).unwrap();
    std::fs::write(job.join("frame.tga"), "x").unwrap();
    junction(&job.join("inner-link"), &outside);
    std::fs::write(root.join("ghosting-render.settings.reg"), "x").unwrap();
    let now = std::time::SystemTime::now();
    // Свежие папки по сроку остаются; при старте (срок 0) уходят все, файлы корня — нет.
    assert_eq!(jobs::cleanup_stale(&root, now, jobs::STALE).unwrap(), 0);
    let later = now + std::time::Duration::from_secs(3 * 24 * 3600);
    assert_eq!(jobs::cleanup_stale(&root, later, jobs::STALE).unwrap(), 2);
    assert_eq!(listing(&root), json!(["ghosting-render.settings.reg"]));
    assert!(
        outside.join("deep").join("keep.txt").exists(),
        "цель ссылок цела"
    );
    std::fs::create_dir(root.join("fresh")).unwrap();
    assert_eq!(
        jobs::cleanup_stale(
            &root,
            std::time::SystemTime::now(),
            std::time::Duration::ZERO
        )
        .unwrap(),
        1
    );
    assert_eq!(listing(&root), json!(["ghosting-render.settings.reg"]));
}

#[test]
fn ffmpeg_locate() {
    use hlsr_core::js::Platform;
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(hlsr_demo::ffmpeg::locate(Platform::Win32, None, None), None);
    assert_eq!(
        hlsr_demo::ffmpeg::locate(Platform::Win32, None, Some(dir.path())),
        None
    );
    let env = hlsr_demo::ffmpeg::locate(Platform::Win32, Some(dir.path()), None).unwrap();
    assert_eq!(
        (env.ffmpeg.clone(), env.ffprobe.clone(), env.source),
        (
            dir.path().join("ffmpeg.exe"),
            dir.path().join("ffprobe.exe"),
            "env"
        )
    );
    let linux = hlsr_demo::ffmpeg::locate(Platform::Linux, None, None).unwrap();
    assert_eq!(
        (linux.ffmpeg, linux.source),
        (PathBuf::from("ffmpeg"), "path")
    );
    std::fs::write(dir.path().join("ffmpeg.exe"), "").unwrap();
    std::fs::write(dir.path().join("ffprobe.exe"), "").unwrap();
    let tool = hlsr_demo::ffmpeg::locate(Platform::Win32, None, Some(dir.path())).unwrap();
    assert_eq!(tool.source, "tool");
    // Сверка перед запуском: пустышки не совпадают с sha256 манифеста.
    assert_eq!(
        hlsr_demo::ffmpeg::verify(&tool).unwrap_err().code,
        "FFMPEG_BROKEN"
    );
    assert!(
        hlsr_demo::ffmpeg::verify(&env).is_ok(),
        "подмена не сверяется"
    );
}
