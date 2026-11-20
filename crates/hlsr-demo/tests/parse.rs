//! Сверка разбора «Демо» с 0.4.9: `tests/fixtures/parse.json` (`tools/reference/demo/parse.js`).

mod common;

use common::{build, input, write_tree};
use hlsr_core::js::{Platform, cmp_utf16};
use hlsr_core::json_text::js_number;
use hlsr_delivery::demo_archive::Demo;
use hlsr_delivery::manifests::local_config_of;
use hlsr_demo::errors::{DemoError, public_code, public_error};
use hlsr_demo::goldsrc_demo::{self, BUTTONS, MAX_MAP_CHANGES};
use hlsr_demo::headers::{self, DemoHeader, SourceFields, header_value};
use hlsr_demo::library::{self, Entry, GameContext, Record, Segment, SegmentPlaylist};
use hlsr_demo::playlist;
use hlsr_demo::source_demo::{self, CHAPTERS, Finish, FinishOptions, FinishScan};
use hlsr_demo::source_render::{self, Picked, Range};
use hlsr_demo::{Marker, library::id_of};
use serde_json::{Value, json};
use std::path::{MAIN_SEPARATOR_STR, Path};
use std::sync::OnceLock;

fn fixture() -> &'static Value {
    static FIXTURE: OnceLock<Value> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/parse.json"
        ))
        .unwrap();
        serde_json::from_str(&text).unwrap()
    })
}

fn markers(list: &[Marker]) -> Value {
    Value::Array(list.iter().map(Marker::to_value).collect())
}

fn f(value: &Value) -> f64 {
    value.as_f64().unwrap()
}

#[test]
fn errors_public_error() {
    for case in fixture()["errors"].as_array().unwrap() {
        let error = &case["error"];
        let fallback = case["fallback"].as_str();
        let actual = if let Some(code) = error["demo"].as_str() {
            let own = DemoError::new(code).with_details(error["details"].clone());
            assert_eq!(public_error(&own, fallback), own.to_public());
            own.to_public()
        } else {
            public_code(error["errno"].as_str().or(error["code"].as_str()), fallback)
        };
        assert_eq!(actual, case["public"], "{case}");
    }
    // Ошибка ОС — по errno.
    let missing = std::fs::File::open("нет-такого-файла.dem").unwrap_err();
    assert_eq!(
        public_error(&missing, Some("DEMO_FAILED")),
        json!({"code": "DEMO_FAILED"})
    );
}

#[test]
fn headers_from_buffers() {
    for case in fixture()["headers"].as_array().unwrap() {
        let bytes = input(case);
        assert_eq!(
            header_value(&headers::parse_demo_buffer(&bytes)),
            case["result"],
            "{}",
            case["gen"]
        );
    }
}

#[test]
fn headers_from_files() {
    let dir = tempfile::tempdir().unwrap();
    for case in fixture()["headers"].as_array().unwrap().iter().take(14) {
        let file = dir.path().join("a.dem");
        std::fs::write(&file, input(case)).unwrap();
        assert_eq!(
            header_value(&headers::parse_demo_file(&file)),
            case["result"],
            "{}",
            case["gen"]
        );
    }
    // Большой файл: разбирается только заголовок.
    let big = dir.path().join("big.dem");
    std::fs::write(
        &big,
        build(&json!({"fn": "source", "opts": {"signon": 10}})),
    )
    .unwrap();
    std::fs::File::options()
        .write(true)
        .open(&big)
        .unwrap()
        .set_len(64 * 1024 * 1024)
        .unwrap();
    let parsed = headers::parse_demo_file(&big).unwrap();
    assert_eq!(parsed.size, 64 * 1024 * 1024);
    assert_eq!(
        headers::parse_demo_file(&dir.path().join("none.dem")),
        Err("DEMO_NOT_FOUND")
    );
    assert_eq!(
        headers::parse_demo_file(dir.path()),
        Err("DEMO_READ_FAILED")
    );
}

#[test]
fn playlist_decode_and_leaves() {
    let section = &fixture()["playlist"];
    for case in section["decode"].as_array().unwrap() {
        let bytes = input(case);
        let actual = match playlist::decode_playlist(&bytes) {
            Ok(decoded) => decoded.to_value(),
            Err(error) => error.to_value(),
        };
        assert_eq!(actual, case["result"], "{}", case["gen"]);
    }
    for case in section["leaves"].as_array().unwrap() {
        assert_eq!(
            json!(playlist::segment_leaf(case["file"].as_str().unwrap())),
            case["leaf"],
            "{case}"
        );
    }
}

/// Сегмент описания из полей JS (`map`, `playlistMap`, `ticks`, `duration`, `tickrate`, таймер).
fn segment(spec: &Value) -> Segment {
    let playlist =
        (spec.get("playlistMap").is_some() || spec.get("timerStart").is_some()).then(|| {
            SegmentPlaylist {
                timer_start: spec.get("timerStart").map_or(f64::NAN, f),
                timer_end: spec.get("timerEnd").map_or(f64::NAN, f),
                map: spec["playlistMap"].as_str().unwrap_or_default().to_owned(),
                size_matches: true,
                sha256: String::new(),
                file: String::new(),
            }
        });
    Segment {
        index: 1,
        name: String::new(),
        path: String::new(),
        playlist,
        header: Ok(DemoHeader {
            engine: headers::ENGINE_SOURCE,
            map: spec["map"].as_str().unwrap_or_default().to_owned(),
            duration: spec["duration"].as_f64(),
            source: Some(SourceFields {
                ticks: spec["ticks"].as_i64().unwrap_or(0) as i32,
                tickrate: spec["tickrate"].as_f64(),
                ..SourceFields::default()
            }),
            ..DemoHeader::default()
        }),
        size: 0,
    }
}

fn picked_of<'a>(segments: &'a [Segment], specs: &[Value]) -> Vec<Picked<'a>> {
    specs
        .iter()
        .zip(segments)
        .enumerate()
        .map(|(index, (spec, segment))| Picked {
            segment,
            index,
            start: f(&spec["start"]),
            end: spec.get("end").map_or(0.0, f),
            duration: 0.0,
            frames: 0.0,
        })
        .collect()
}

fn scan_of(value: &Value) -> Option<FinishScan> {
    value.as_object()?;
    Some(FinishScan {
        ok: value["ok"].as_bool().unwrap(),
        finish: value["finish"].as_object().map(|finish| Finish {
            tick: finish["tick"].as_i64().unwrap() as i32,
            time: finish["time"].as_f64(),
        }),
        broken: false,
    })
}

#[test]
fn source_demo_finish_chapters_clock() {
    let section = &fixture()["sourceDemo"];
    let dir = tempfile::tempdir().unwrap();
    for case in section["scans"].as_array().unwrap() {
        let bytes = input(case);
        assert_eq!(
            source_demo::scan_finish(&bytes).to_value(),
            case["finish"],
            "{}",
            case["gen"]
        );
        // Тот же ответ по файлу, окнами.
        let file = dir.path().join("s.dem");
        std::fs::write(&file, &bytes).unwrap();
        assert_eq!(
            source_demo::scan_file(&file, &|| false).unwrap().to_value(),
            case["finish"],
            "{}",
            case["gen"]
        );
        if !case["commands"].is_null() {
            let (commands, broken) = source_demo::console_commands(&bytes);
            let commands: Vec<Value> = commands
                .iter()
                .map(|c| json!({"tick": c.tick, "text": c.text}))
                .collect();
            assert_eq!(
                json!({"commands": commands, "broken": broken}),
                case["commands"],
                "{}",
                case["gen"]
            );
        }
    }
    for case in section["chapters"].as_array().unwrap() {
        let specs = case["picked"].as_array().unwrap();
        let segments: Vec<Segment> = specs.iter().map(|spec| segment(&spec["segment"])).collect();
        assert_eq!(
            markers(&source_demo::chapter_markers(&picked_of(&segments, specs))),
            case["markers"]
        );
    }
    for case in section["finish"].as_array().unwrap() {
        let specs = case["picked"].as_array().unwrap();
        let segments: Vec<Segment> = specs.iter().map(|spec| segment(&spec["segment"])).collect();
        let scans: Vec<Option<FinishScan>> = case["scans"]
            .as_array()
            .unwrap()
            .iter()
            .map(scan_of)
            .collect();
        let options = FinishOptions {
            incomplete: case["options"]["incomplete"].as_bool().unwrap_or(false),
            run_time: case["options"]["runTime"].as_f64(),
        };
        assert_eq!(
            markers(&source_demo::finish_markers(
                &picked_of(&segments, specs),
                &scans,
                options
            )),
            case["markers"],
            "{case}"
        );
    }
    for case in section["clocks"].as_array().unwrap() {
        assert_eq!(
            source_demo::run_clock(f(&case["seconds"])),
            case["label"].as_str().unwrap()
        );
    }
    let table: serde_json::Map<String, Value> = CHAPTERS
        .iter()
        .map(|(map, id)| ((*map).to_owned(), json!(id)))
        .collect();
    assert_eq!(Value::Object(table), section["chaptersTable"]);
}

#[test]
fn source_finish_across_windows() {
    // Демка больше окна (1 МБ): команды по 50 КБ, финиш — в конце, после границ окон.
    let mut list: Vec<(i32, String)> = (0..60)
        .map(|k| (k, format!("echo {}", "x".repeat(50_000))))
        .collect();
    let fixture_commands = fixture()["sourceDemo"]["scans"][0]["gen"]["body"]["commands"].clone();
    list.extend(
        common::commands(&fixture_commands)
            .into_iter()
            .map(|(tick, text)| (tick + 1000, text)),
    );
    let bytes = common::source(&json!({}), Some(common::source_frames(&list, true)));
    assert!(bytes.len() > 2 << 20);
    let whole = source_demo::scan_finish(&bytes);
    assert_eq!(
        whole.finish,
        Some(Finish {
            tick: 1300,
            time: Some(5025.678)
        })
    );
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("long.dem");
    std::fs::write(&file, &bytes).unwrap();
    assert_eq!(source_demo::scan_file(&file, &|| false).unwrap(), whole);
    assert_eq!(
        source_demo::scan_file(&file, &|| true).unwrap_err().code,
        "CANCELLED"
    );
}

#[test]
fn source_render_plans_and_timeline() {
    let section = &fixture()["sourceRender"];
    for case in section["plans"].as_array().unwrap() {
        let segments: Vec<Segment> = case["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(segment)
            .collect();
        let range = case["range"].as_object().map(|r| Range {
            from: f(&r["from"]),
            to: f(&r["to"]),
        });
        let plan = source_render::plan_segments(&segments, range, f(&case["fps"]));
        let mut actual = json!({
            "picked": plan.picked.iter().map(|p| json!({"index": p.index, "start": js_number(p.start), "end": js_number(p.end),
                "duration": js_number(p.duration), "frames": js_number(p.frames)})).collect::<Vec<_>>(),
            "offset": js_number(plan.offset),
            "cuts": plan.cuts.iter().map(|c| json!({"from": js_number(c.from), "to": js_number(c.to)})).collect::<Vec<_>>(),
            "framesTotal": js_number(plan.frames_total),
        });
        // Пустой план — без framesNeeded (undefined в JS).
        if !plan.picked.is_empty() {
            actual["framesNeeded"] = plan.frames_needed.map_or(Value::Null, js_number);
        }
        assert_eq!(actual, case["plan"], "{}", case["segments"]);
        assert_eq!(
            markers(&source_render::timeline_of(&plan.picked, range)),
            case["markers"],
            "{}",
            case["segments"]
        );
    }
    for case in section["clocks"].as_array().unwrap() {
        assert_eq!(
            source_render::clock_label(f(&case["seconds"])),
            case["label"].as_str().unwrap()
        );
    }
}

#[test]
fn goldsrc_scans() {
    let section = &fixture()["goldsrcDemo"];
    let dir = tempfile::tempdir().unwrap();
    for case in section["cases"].as_array().unwrap() {
        let file = dir.path().join("scan.dem");
        std::fs::write(&file, input(case)).unwrap();
        let actual = goldsrc_demo::scan_value(&goldsrc_demo::scan_demo(&file, &|| false));
        assert_eq!(actual, case["scan"], "{}", case["gen"]);
    }
    let buttons: serde_json::Map<String, Value> = BUTTONS
        .iter()
        .map(|(name, bit)| ((*name).to_owned(), json!(bit)))
        .collect();
    assert_eq!(Value::Object(buttons), section["buttons"]);
    assert_eq!(json!(MAX_MAP_CHANGES), section["maxMapChanges"]);
    let file = dir.path().join("scan.dem");
    assert_eq!(
        goldsrc_demo::scan_demo(&file, &|| true).unwrap_err().code,
        "CANCELLED"
    );
}

// ---------------------------------------------------------------- library

/// Пути из фикстуры (`<root>/…` через «/») — настоящие.
fn real(root: &str, text: &str) -> String {
    match text.strip_prefix("<root>") {
        Some(rest) => format!("{root}{}", rest.replace('/', MAIN_SEPARATOR_STR)),
        None => text.to_owned(),
    }
}

fn real_value(root: &str, value: &Value) -> Value {
    match value {
        Value::String(text) => json!(real(root, text)),
        Value::Array(list) => Value::Array(list.iter().map(|v| real_value(root, v)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), real_value(root, v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Как `normalizer` скрипта: корень → `<root>`, «\» → «/», без `mtimeMs`.
fn norm(root: &str, value: &Value) -> Value {
    match value {
        Value::String(text) => json!(text.replace(root, "<root>").replace('\\', "/")),
        Value::Array(list) => Value::Array(list.iter().map(|v| norm(root, v)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| *key != "mtimeMs")
                .map(|(k, v)| (k.clone(), norm(root, v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// id из путей файлов → «<id:files>» (`markId` скрипта); id серии остаётся.
fn mark_id(entry: &Entry, mut value: Value) -> Value {
    let mut files = entry.files.clone();
    if entry.kind == "segments" {
        files.sort_by(|a, b| cmp_utf16(a, b));
    }
    let key = files.join("|").to_lowercase();
    if entry.id
        == id_of(&[
            entry.source,
            entry.game_id.as_deref().unwrap_or(""),
            entry.kind,
            &key,
        ])
    {
        value["id"] = json!("<id:files>");
    }
    value
}

/// Метки рана, которых нет у адаптера (`DemoTool.runMarkers`): главы и финиш.
fn run_markers(described: &library::Described) -> Value {
    if described.engine != Some(headers::ENGINE_SOURCE) {
        return Value::Null;
    }
    let picked = source_render::plan_segments(&described.segments, None, 60.0).picked;
    let mut out = source_demo::chapter_markers(&picked);
    if !described.incomplete && described.segments.iter().all(Segment::ok) {
        let scans: Vec<Option<FinishScan>> = picked
            .iter()
            .map(|p| source_demo::scan_file(Path::new(&p.segment.path), &|| false).ok())
            .collect();
        let options = FinishOptions {
            incomplete: described.incomplete,
            run_time: described.run_time,
        };
        out.extend(source_demo::finish_markers(&picked, &scans, options));
    }
    markers(&out)
}

fn check_items(root: &str, entries: &[Entry], expected: &Value) {
    let expected = expected.as_array().unwrap();
    assert_eq!(entries.len(), expected.len(), "{expected:?}");
    for (entry, item) in entries.iter().zip(expected) {
        let described = library::describe(entry);
        assert_eq!(norm(root, &mark_id(entry, entry.to_value())), item["entry"]);
        assert_eq!(
            norm(root, &mark_id(entry, described.to_value())),
            item["described"],
            "{}",
            entry.name
        );
        assert_eq!(
            norm(root, &mark_id(entry, described.summary())),
            item["summary"],
            "{}",
            entry.name
        );
        assert_eq!(
            norm(root, &run_markers(&described)),
            item["runMarkers"],
            "{}",
            entry.name
        );
    }
}

#[test]
fn library_game_entries() {
    for scenario in fixture()["library"]["games"].as_array().unwrap() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        write_tree(dir.path(), &scenario["tree"]);
        let contexts: Vec<GameContext> = scenario["games"]
            .as_array()
            .unwrap()
            .iter()
            .map(|game| GameContext {
                game_id: game["gameId"].as_str().unwrap().to_owned(),
                install_path: real(
                    &root,
                    &format!("<root>/{}", game["install"].as_str().unwrap()),
                ),
                config: local_config_of(game["gameId"].as_str().unwrap()).unwrap(),
            })
            .collect();
        check_items(&root, &library::game_entries(&contexts), &scenario["items"]);
    }
}

fn record_of(value: &Value) -> Record {
    match value["kind"].as_str().unwrap() {
        "file" => Record::File(value["path"].as_str().unwrap().to_owned()),
        _ => Record::Segments(
            value["paths"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_str().unwrap().to_owned())
                .collect(),
        ),
    }
}

fn records_value(records: &[Record]) -> Value {
    Value::Array(records.iter().map(Record::to_value).collect())
}

#[test]
fn library_manual() {
    let manual = &fixture()["library"]["manual"];
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().into_owned();
    write_tree(dir.path(), &manual["tree"]);
    for step in manual["steps"].as_array().unwrap() {
        // Как у скрипта: перед последним шагом ghost.dem удалён.
        if step.get("note").is_some() {
            std::fs::remove_file(dir.path().join("ghost.dem")).unwrap();
        }
        match step["op"].as_str().unwrap() {
            "addPaths" => {
                let stored: Vec<Record> = match &step["stored"] {
                    Value::String(_) => (0..199)
                        .map(|i| Record::File(real(&root, &format!("<root>/f{i}.dem"))))
                        .collect(),
                    list => list
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|r| record_of(&real_value(&root, r)))
                        .collect(),
                };
                let result = library::add_paths(
                    &stored,
                    &real_value(&root, &step["paths"]),
                    Platform::Win32,
                );
                let rejected: Vec<Value> = result.rejected.iter().map(|r| r.to_value()).collect();
                let mut actual =
                    json!({"added": records_value(&result.added), "rejected": rejected});
                if step["result"].get("stored").is_some() {
                    actual["stored"] = records_value(&result.stored);
                }
                assert_eq!(norm(&root, &actual), step["result"], "{}", step["paths"]);
            }
            "manualEntries" => {
                let (entries, cleaned) =
                    library::manual_entries(&real_value(&root, &step["stored"]));
                assert_eq!(
                    norm(&root, &records_value(&cleaned)),
                    step["cleaned"],
                    "{}",
                    step["stored"]
                );
                if !step["items"].is_null() {
                    check_items(&root, &entries, &step["items"]);
                }
            }
            "describe" => {
                let item = real_value(&root, &step["item"]);
                let entry = Entry {
                    id: item["id"].as_str().unwrap().to_owned(),
                    kind: match item["kind"].as_str().unwrap() {
                        "demo" => "demo",
                        "segments" => "segments",
                        _ => "playlist",
                    },
                    source: "manual",
                    game_id: None,
                    name: item["name"].as_str().unwrap().to_owned(),
                    files: item["files"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|p| p.as_str().unwrap().to_owned())
                        .collect(),
                    mtime_ms: 0.0,
                    series_missing: None,
                };
                let described = library::describe(&entry);
                assert_eq!(
                    norm(&root, &described.to_value()),
                    step["described"],
                    "{}",
                    entry.name
                );
                assert_eq!(
                    norm(&root, &run_markers(&described)),
                    step["runMarkers"],
                    "{}",
                    entry.name
                );
            }
            other => panic!("шаг {other}"),
        }
    }
}

#[test]
fn library_series_ids_natural() {
    let section = &fixture()["library"];
    let demos: Vec<Demo> = section["groupSeries"]["input"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| Demo {
            dir: d["dir"].as_str().unwrap().to_owned(),
            name: d["name"].as_str().unwrap().to_owned(),
            size: d["size"].as_u64().unwrap(),
            mtime_ms: d["mtimeMs"].as_i64().unwrap(),
        })
        .collect();
    let (series, rest) = library::group_series(&demos);
    let actual = json!({
        "series": series.iter().map(|run| json!({
            "dir": run.dir, "name": run.name, "missing": run.missing,
            "demos": run.demos.iter().map(|d| { let mut v = d.demo.to_value(); v["number"] = json!(d.number); v }).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "rest": rest.iter().map(Demo::to_value).collect::<Vec<_>>(),
    });
    assert_eq!(actual, section["groupSeries"]["result"]);
    for case in section["idOf"].as_array().unwrap() {
        let parts: Vec<&str> = case["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap())
            .collect();
        assert_eq!(id_of(&parts), case["id"].as_str().unwrap());
    }
    let strings = |list: &Value| -> Vec<String> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap().to_owned())
            .collect()
    };
    let mut natural = strings(&section["natural"]["input"]);
    natural.sort_by(|a, b| library::natural_by_name(a, b));
    assert_eq!(natural, strings(&section["natural"]["sorted"]));
}
