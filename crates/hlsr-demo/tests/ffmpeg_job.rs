//! Задание с настоящим FFmpeg (самотест `demo-tool-jobs` 0.4.9, коротко): сырьё 2 с от
//! подставного адаптера → MP4 H.264 + AAC, проверка `ffprobe`, `timeline.json`, папка задания
//! удалена; сверка FFmpeg инструмента перед запуском; отмена гасит процесс.
//!
//! FFmpeg: `HLSR_DEMO_FFMPEG` (папка bin или ffmpeg) или первый `ffmpeg` в PATH; он кладётся
//! как инструмент (`Tools/ffmpeg`) во временную папку. Нет FFmpeg — тесты пропускаются.

use hlsr_core::js::Platform;
use hlsr_core::js::process::kill_zero;
use hlsr_demo::Marker;
use hlsr_demo::errors::DemoError;
use hlsr_demo::ffmpeg::{self, Located, RunOptions};
use hlsr_demo::jobs::{
    self, Adapter, JobKind, JobOptions, Material, Progress, RenderProgress, RenderRequest,
};
use hlsr_demo::library::{Described, Entry};
use hlsr_demo::planner::{self, PlanOptions, Source};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};

const PLATFORM: Platform = if cfg!(windows) {
    Platform::Win32
} else {
    Platform::Linux
};

/// Папка с ffmpeg и ffprobe: подмена или PATH.
fn ffmpeg_dir() -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    if let Some(given) = std::env::var_os(ffmpeg::ENV_OVERRIDE).map(PathBuf::from) {
        return Some(if given.is_dir() {
            given
        } else {
            given.parent()?.to_path_buf()
        });
    }
    std::env::split_paths(&std::env::var_os("PATH")?).find(|dir| dir.join(exe).is_file())
}

/// FFmpeg как инструмент во временной папке (жёсткие ссылки, иначе копия).
fn tool_dir(base: &Path) -> Option<PathBuf> {
    let source = ffmpeg_dir()?;
    let dir = base.join("Tools").join("ffmpeg");
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["ffmpeg", "ffprobe"] {
        let file = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_owned()
        };
        let (from, to) = (source.join(&file), dir.join(&file));
        if std::fs::hard_link(&from, &to).is_err() {
            std::fs::copy(&from, &to).ok()?;
        }
    }
    Some(dir)
}

fn item(duration: f64) -> Described {
    Described {
        entry: Entry {
            id: "fake-id".to_owned(),
            kind: "demo",
            source: "manual",
            game_id: None,
            name: "myrun1.dem".to_owned(),
            files: Vec::new(),
            mtime_ms: 0.0,
            series_missing: None,
        },
        engine: Some("goldsrc"),
        game: Some("hl1"),
        map: Some("c1a0".to_owned()),
        maps: Vec::new(),
        duration: Some(duration),
        supported: true,
        reason: None,
        segments: Vec::new(),
        markers: Vec::new(),
        size: 0,
        incomplete: false,
        run_time: None,
        source_files: Some(vec!["myrun1.dem".to_owned()]),
        failed_segment: None,
        warnings: None,
    }
}

/// Подставной адаптер 0.4.9 (`fakeAdapter`): испытательная таблица и синус длиной отрезка.
struct Fake {
    rendered: Cell<u32>,
}

impl Adapter for Fake {
    fn render(&self, request: &RenderRequest) -> Result<Material, DemoError> {
        self.rendered.set(self.rendered.get() + 1);
        let range = request.range;
        let length = range.to - range.from;
        let video = request.job_dir.join("fake.mkv");
        let wav = request.job_dir.join("fake.wav");
        let size = format!("{}x{}", request.width, request.height);
        let args: Vec<String> = [
            "-hide_banner",
            "-nostdin",
            "-y",
            "-loglevel",
            "error",
            "-progress",
            "pipe:1",
            "-nostats",
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc=size={size}:rate={}:duration={length}", request.fps),
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency=440:sample_rate=48000:duration={length}"),
            "-map",
            "0:v",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "12",
            "-pix_fmt",
            "yuv420p",
            &video.to_string_lossy(),
            "-map",
            "1:a",
            "-c:a",
            "pcm_s16le",
            &wav.to_string_lossy(),
        ]
        .iter()
        .map(|item| (*item).to_owned())
        .collect();
        let progress = |seconds: f64| {
            (request.on_progress)(&RenderProgress {
                fraction: (seconds / length).min(1.0),
                segment: None,
                message: None,
            });
        };
        ffmpeg::run(
            &request.ffmpeg.ffmpeg,
            &args,
            &RunOptions {
                cancel: Some(request.cancel),
                on_progress: Some(&progress),
                ..RunOptions::default()
            },
        )?;
        let mut material = Material::new(Source::Video(video.to_string_lossy().into_owned()));
        material.audio = Some(wav.to_string_lossy().into_owned());
        material.offset = Some(range.from);
        material.timeline = vec![
            Marker {
                t: range.from,
                kind: "map",
                label: "c1a0".to_owned(),
            },
            Marker {
                t: range.from + length / 2.0,
                kind: "timer",
                label: "fake".to_owned(),
            },
        ];
        Ok(material)
    }
}

struct Run {
    result: Result<jobs::JobResult, DemoError>,
    phases: Vec<&'static str>,
}

fn run(base: &Path, located: &Located, kind: JobKind, raw: Value, adapter: &Fake) -> Run {
    let item = item(2.0);
    let settings =
        planner::normalize_settings(&raw, item.duration, kind == JobKind::Preview).unwrap();
    let clip = planner::Clip {
        from: settings.range.from,
        to: settings.range.to,
        speed: settings.speed,
        offset: None,
        cuts: Vec::new(),
    };
    let plan = planner::plan_export(
        &settings,
        &PlanOptions {
            encoder: Some(planner::CPU_ENCODER.to_owned()),
            output_duration: planner::clip_duration(&clip),
            ..PlanOptions::default()
        },
    )
    .unwrap();
    let phases = RefCell::new(Vec::new());
    let on_progress = |p: &Progress| {
        let mut list = phases.borrow_mut();
        if list.last() != Some(&p.phase) {
            list.push(p.phase);
        }
    };
    let output = base.join("Videos");
    let result = jobs::run_job(&JobOptions {
        id: "job-1",
        kind,
        item: &item,
        adapter,
        settings: &settings,
        plan: &plan,
        root: &base.join("jobs"),
        output_dir: Some(&output),
        extra_markers: &[],
        ffmpeg: located,
        cancel: &|| false,
        on_progress: Some(&on_progress),
        on_spawn: None,
        logger: None,
        run_fn: None,
        ffprobe_fn: None,
    });
    Run {
        result,
        phases: phases.into_inner(),
    }
}

#[test]
fn export_and_preview_with_real_ffmpeg() {
    let base = tempfile::tempdir().unwrap();
    let Some(dir) = tool_dir(base.path()) else {
        eprintln!("ПРОПУЩЕНО: нет FFmpeg — задайте HLSR_DEMO_FFMPEG или положите ffmpeg в PATH");
        return;
    };
    let tool = ffmpeg::locate(PLATFORM, None, Some(&dir)).unwrap();
    assert_eq!(tool.source, "tool");
    let env = ffmpeg::locate(PLATFORM, Some(&dir), None).unwrap();
    let adapter = Fake {
        rendered: Cell::new(0),
    };
    let raw = json!({"preset": "quality", "height": 720, "fps": 30, "encoder": "cpu", "range": {"from": 0, "to": 2}});

    // Сверка перед запуском: FFmpeg инструмента не той сборки — отказ до рендера.
    if let Err(error) = ffmpeg::verify(&tool) {
        assert_eq!(error.code, "FFMPEG_BROKEN");
        let refused = run(base.path(), &tool, JobKind::Export, raw.clone(), &adapter);
        assert_eq!(refused.result.unwrap_err().code, "FFMPEG_BROKEN");
        assert_eq!(adapter.rendered.get(), 0, "рендер не начинался");
        assert_eq!(
            std::fs::read_dir(base.path().join("jobs")).unwrap().count(),
            0
        );
    }

    let done = run(base.path(), &env, JobKind::Export, raw, &adapter);
    let result = done.result.unwrap();
    assert_eq!(
        done.phases,
        ["prepare", "render", "encode", "verify", "done"]
    );
    assert_eq!(result.file, base.path().join("Videos").join("myrun1.mp4"));
    assert_eq!(result.attempts, 1);
    let info = ffmpeg::ffprobe(&env.ffprobe, &result.file, None).unwrap();
    let streams = info["streams"].as_array().unwrap();
    let video = streams.iter().find(|s| s["codec_type"] == "video").unwrap();
    let audio = streams.iter().find(|s| s["codec_type"] == "audio").unwrap();
    assert_eq!(
        (
            &video["codec_name"],
            &video["avg_frame_rate"],
            &video["width"],
            &video["height"]
        ),
        (&json!("h264"), &json!("30/1"), &json!(1280), &json!(720))
    );
    assert_eq!(audio["codec_name"], "aac");
    let timeline: Value =
        serde_json::from_str(&std::fs::read_to_string(result.timeline_file.unwrap()).unwrap())
            .unwrap();
    assert_eq!(timeline["video"]["file"], "myrun1.mp4");
    assert_eq!(
        timeline["markers"][1],
        json!({"t": 1, "type": "timer", "label": "fake", "sourceTime": 1})
    );
    assert_eq!(
        std::fs::read_dir(base.path().join("jobs")).unwrap().count(),
        0,
        "папка задания удалена"
    );

    // Предпросмотр: 720p30 в папке задания, в ней только итог.
    let preview = run(
        base.path(),
        &env,
        JobKind::Preview,
        json!({"preset": "preview", "height": 720, "fps": 30, "range": {"from": 0.5, "to": 2}, "speed": 2}),
        &adapter,
    );
    let result = preview.result.unwrap();
    let job = result.dir.unwrap();
    let names: Vec<_> = std::fs::read_dir(&job)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["output.mp4"]);
    assert_eq!(
        result.timeline["video"]["duration"]
            .as_f64()
            .map(|d| (d - 0.75).abs() < 0.1),
        Some(true)
    );
    jobs::remove_job_dir(&base.path().join("jobs"), &job);
}

#[test]
fn cancel_kills_ffmpeg() {
    let base = tempfile::tempdir().unwrap();
    let Some(dir) = tool_dir(base.path()) else {
        eprintln!("ПРОПУЩЕНО: нет FFmpeg");
        return;
    };
    let env = ffmpeg::locate(PLATFORM, Some(&dir), None).unwrap();
    let args: Vec<String> = [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-progress",
        "pipe:1",
        "-nostats",
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=1280x720:rate=30:duration=600",
        "-f",
        "null",
        "-",
    ]
    .iter()
    .map(|item| (*item).to_owned())
    .collect();
    let started = Cell::new(false);
    let pid = Cell::new(0);
    let on_progress = |_: f64| started.set(true);
    let on_spawn = |id: u32| pid.set(id);
    let cancel = || started.get();
    let error = ffmpeg::run(
        &env.ffmpeg,
        &args,
        &RunOptions {
            cancel: Some(&cancel),
            on_progress: Some(&on_progress),
            on_spawn: Some(&on_spawn),
            ..RunOptions::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "CANCELLED");
    assert!(
        kill_zero(f64::from(pid.get())).is_err(),
        "процесс FFmpeg погашен"
    );

    let missing =
        ffmpeg::run(&base.path().join("nope.exe"), &[], &RunOptions::default()).unwrap_err();
    assert_eq!(missing.code, "FFMPEG_MISSING");
    let failed = ffmpeg::run(
        &env.ffmpeg,
        &["-i".to_owned(), "missing-input.mkv".to_owned()],
        &RunOptions::default(),
    )
    .unwrap_err();
    assert_eq!(failed.code, "FFMPEG_FAILED");
    assert!(
        failed.details["exitCode"]
            .as_i64()
            .is_some_and(|code| code != 0)
    );
    let info = ffmpeg::version(&env.ffmpeg, None).unwrap();
    assert!(
        info.first_line.starts_with("ffmpeg version "),
        "{}",
        info.first_line
    );
    assert!(
        ffmpeg::list_encoders(&env.ffmpeg, None)
            .unwrap()
            .contains("libx264")
    );
}
