//! Синтетические демо и плейлисты (`scripts/lib/demo-fixtures.js` 0.4.9) по рецептам фикстуры
//! `parse.json` — те же байты, что у генератора 0.4.9 (тест сверяет sha256).

#![allow(dead_code)]

use hlsr_core::js::{latin1_bytes, math_round};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Число опции: `"NaN"`, `"Infinity"` — строкой; нет — `default`.
fn num(value: Option<&Value>, default: f64) -> f64 {
    match value {
        Some(Value::String(s)) if s == "NaN" => f64::NAN,
        Some(Value::String(s)) if s == "Infinity" => f64::INFINITY,
        Some(Value::Number(n)) => n.as_f64().expect("число"),
        _ => default,
    }
}

/// `x || 0` для числа JS.
fn or_zero(value: Option<&Value>) -> f64 {
    let x = num(value, 0.0);
    if x.is_nan() { 0.0 } else { x }
}

fn text<'a>(value: Option<&'a Value>, default: &'a str) -> &'a str {
    value.and_then(Value::as_str).unwrap_or(default)
}

fn put(buffer: &mut [u8], at: usize, bytes: &[u8]) {
    let end = (at + bytes.len()).min(buffer.len());
    buffer[at..end].copy_from_slice(&bytes[..end - at]);
}

fn i32le(buffer: &mut [u8], at: usize, value: f64) {
    put(buffer, at, &(value as i32).to_le_bytes());
}

fn f32le(buffer: &mut [u8], at: usize, value: f64) {
    put(buffer, at, &(value as f32).to_le_bytes());
}

/// `goldsrcDemo(options)`.
pub fn goldsrc(opts: &Value) -> Vec<u8> {
    let default_entries = serde_json::json!([
        {"type": 0, "description": "LOADING", "duration": 0.5, "frames": 10},
        {"type": 1, "description": "Playback", "duration": 12.25, "frames": 1225},
    ]);
    let entries = opts["entries"]
        .as_array()
        .unwrap_or_else(|| default_entries.as_array().unwrap());
    let data_bytes = num(opts.get("dataBytes"), 2000.0) as usize;
    let mut header = vec![0u8; 544];
    put(&mut header, 0, b"HLDEMO\0\0");
    i32le(&mut header, 8, num(opts.get("protocol"), 5.0));
    let net = or_zero(opts.get("netProtocol"));
    i32le(&mut header, 12, if net == 0.0 { 48.0 } else { net });
    put(
        &mut header,
        16,
        &latin1_bytes(text(opts.get("map"), "c1a0")),
    );
    put(
        &mut header,
        276,
        &latin1_bytes(text(opts.get("gameDir"), "valve_WON")),
    );
    i32le(&mut header, 536, 1234.0);
    i32le(
        &mut header,
        540,
        num(opts.get("directoryOffset"), (544 + data_bytes) as f64),
    );
    let mut dir = vec![0u8; 4 + entries.len() * 92];
    i32le(
        &mut dir,
        0,
        num(opts.get("entryCount"), entries.len() as f64),
    );
    let share = data_bytes / entries.len().max(1);
    for (index, entry) in entries.iter().enumerate() {
        let at = 4 + index * 92;
        i32le(&mut dir, at, num(entry.get("type"), 0.0));
        put(
            &mut dir,
            at + 4,
            &latin1_bytes(text(entry.get("description"), "")),
        );
        f32le(&mut dir, at + 76, num(entry.get("duration"), 0.0));
        i32le(&mut dir, at + 80, num(entry.get("frames"), 0.0));
        i32le(&mut dir, at + 84, (544 + index * share) as f64);
        i32le(&mut dir, at + 88, share as f64);
    }
    [header, vec![0; data_bytes], dir].concat()
}

fn frames(list: &[Value]) -> Vec<u8> {
    let head = |kind: u8, time: f64, frame: f64| {
        let mut head = vec![0u8; 9];
        head[0] = kind;
        f32le(&mut head, 1, time);
        i32le(&mut head, 5, frame);
        head
    };
    let mut out = Vec::new();
    for f in list {
        let kind = num(f.get("type"), 0.0) as u8;
        out.extend(head(kind, or_zero(f.get("time")), or_zero(f.get("frame"))));
        match kind {
            0 | 1 => {
                let mut info = vec![0u8; 436 + 28 + 4];
                put(
                    &mut info,
                    236 + 30,
                    &(or_zero(f.get("buttons")) as u16).to_le_bytes(),
                );
                i32le(&mut info, 432, or_zero(f.get("viewmodel")));
                let message = latin1_bytes(text(f.get("message"), ""));
                i32le(&mut info, 464, num(f.get("length"), message.len() as f64));
                out.extend(info);
                out.extend(message);
            }
            3 => out.extend([0u8; 64]),
            4 => out.extend([0u8; 32]),
            _ => {}
        }
    }
    // Тип 5 (конец записи) — с временем последнего кадра.
    out.extend(head(
        5,
        list.last().map_or(0.0, |f| or_zero(f.get("time"))),
        0.0,
    ));
    out
}

/// `goldsrcFramesDemo(options)`.
pub fn goldsrc_frames(opts: &Value) -> Vec<u8> {
    let empty = Vec::new();
    let loading_list = opts["loading"].as_array().unwrap_or(&empty);
    let playback_list = opts["playback"].as_array().unwrap_or(&empty);
    let loading = frames(loading_list);
    let playback = frames(playback_list);
    let mut header = vec![0u8; 544];
    put(&mut header, 0, b"HLDEMO\0\0");
    i32le(&mut header, 8, 5.0);
    i32le(&mut header, 12, num(opts.get("netProtocol"), 47.0));
    put(
        &mut header,
        16,
        &latin1_bytes(text(opts.get("map"), "c1a0")),
    );
    put(
        &mut header,
        276,
        &latin1_bytes(text(opts.get("gameDir"), "valve_WON")),
    );
    i32le(
        &mut header,
        540,
        (544 + loading.len() + playback.len()) as f64,
    );
    let mut dir = vec![0u8; 4 + 2 * 92];
    i32le(&mut dir, 0, 2.0);
    let duration = num(opts.get("duration"), 5.0);
    let rows = [
        ("LOADING", 0.0, 0.0, 544, loading.len()),
        (
            "Playback",
            1.0,
            duration,
            544 + loading.len(),
            playback.len(),
        ),
    ];
    for (index, (name, kind, duration, offset, length)) in rows.into_iter().enumerate() {
        let at = 4 + index * 92;
        i32le(&mut dir, at, kind);
        put(&mut dir, at + 4, name.as_bytes());
        f32le(&mut dir, at + 76, duration);
        i32le(&mut dir, at + 80, playback_list.len() as f64);
        i32le(&mut dir, at + 84, offset as f64);
        i32le(&mut dir, at + 88, length as f64);
    }
    [header, loading, playback, dir].concat()
}

/// `sourceDemo(options)`; `body` — кадры вместо нулей.
pub fn source(opts: &Value, body: Option<Vec<u8>>) -> Vec<u8> {
    let signon = if body.is_some() && opts.get("signon").is_none() {
        0.0
    } else {
        num(opts.get("signon"), 200.0)
    };
    let mut header = vec![0u8; 1072];
    put(&mut header, 0, b"HL2DEMO\0");
    i32le(&mut header, 8, num(opts.get("protocol"), 3.0));
    i32le(&mut header, 12, 24.0);
    put(&mut header, 16, b"localhost:0");
    put(
        &mut header,
        276,
        text(opts.get("client"), "runner").as_bytes(),
    );
    put(
        &mut header,
        536,
        &latin1_bytes(text(opts.get("map"), "d1_trainstation_01")),
    );
    put(
        &mut header,
        796,
        &latin1_bytes(text(opts.get("gameDir"), "ghostingmod")),
    );
    f32le(&mut header, 1056, num(opts.get("playback"), 3.21));
    i32le(&mut header, 1060, num(opts.get("ticks"), 321.0));
    i32le(&mut header, 1064, num(opts.get("frames"), 313.0));
    i32le(&mut header, 1068, signon);
    let body = body
        .unwrap_or_else(|| vec![0; signon as usize + num(opts.get("dataBytes"), 500.0) as usize]);
    [header, body].concat()
}

/// `sourceFrames(commands, {stop})`.
pub fn source_frames(commands: &[(i32, String)], stop: bool) -> Vec<u8> {
    let head = |cmd: u8, tick: i32| {
        let mut b = vec![cmd];
        b.extend(tick.to_le_bytes());
        b
    };
    let mut out = head(3, 0);
    for (tick, text) in commands {
        out.extend(head(2, *tick));
        out.extend([0u8; 84]);
        out.extend(3i32.to_le_bytes());
        out.extend([1, 2, 3]);
        out.extend(head(5, *tick));
        out.extend(tick.to_le_bytes());
        out.extend(2i32.to_le_bytes());
        out.extend([9, 9]);
        let data = latin1_bytes(&format!("{text}\0"));
        out.extend(head(4, *tick));
        out.extend((data.len() as i32).to_le_bytes());
        out.extend(data);
    }
    if stop {
        out.extend([7, 0, 0]);
    }
    out
}

pub fn commands(list: &Value) -> Vec<(i32, String)> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|pair| {
            (
                pair[0].as_i64().unwrap() as i32,
                pair[1].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// `playlistBuffer(entries, options)`.
pub fn playlist(entries: &Value, opts: &Value) -> Vec<u8> {
    let entries = entries.as_array().unwrap();
    let mut out = b"GHPL".to_vec();
    out.push(num(opts.get("version"), 1.0) as u8);
    out.push(num(opts.get("incomplete"), 0.0) as u8);
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((num(opts.get("total"), 10.0) as f32).to_le_bytes());
    for entry in entries {
        let file = text(entry.get("file"), "").as_bytes();
        let map = text(entry.get("map"), "").as_bytes();
        out.extend((file.len() as u16).to_le_bytes());
        out.extend(file);
        out.push(map.len() as u8);
        out.extend(map);
        out.extend((num(entry.get("start"), 0.0) as f32).to_le_bytes());
        out.extend((num(entry.get("end"), 0.0) as f32).to_le_bytes());
        out.extend((num(entry.get("size"), 0.0) as u64).to_le_bytes());
        out.extend(latin1_bytes(text(entry.get("sha256"), &"ab".repeat(32))));
    }
    out
}

fn net(time: f64, frame: i32, message: &str) -> Value {
    serde_json::json!({"type": 1, "time": time, "frame": frame, "message": message})
}

/// Буфер по рецепту фикстуры.
pub fn build(recipe: &Value) -> Vec<u8> {
    let null = Value::Null;
    let opts = recipe.get("opts").unwrap_or(&null);
    let mut buffer = match recipe["fn"].as_str().unwrap() {
        "goldsrc" => goldsrc(opts),
        "goldsrcFrames" => goldsrc_frames(opts),
        "source" => source(
            opts,
            recipe.get("body").map(|body| {
                source_frames(
                    &commands(&body["commands"]),
                    body.get("stop") != Some(&Value::Bool(false)),
                )
            }),
        ),
        "playlist" => playlist(&recipe["entries"], opts),
        "text" => latin1_bytes(recipe["text"].as_str().unwrap()),
        "forged" => {
            let message = "maps/a.bsp maps/b.bsp ".repeat(45000);
            let base = goldsrc_frames(
                &serde_json::json!({"duration": 1, "playback": [net(0.0, 0, &message), net(1.0, 100, "")]}),
            );
            let dir_offset = i32::from_le_bytes(base[540..544].try_into().unwrap()) as usize;
            let entry = &base[dir_offset + 4 + 92..dir_offset + 4 + 184];
            let count = recipe["entries"].as_u64().unwrap() as usize;
            let mut out = base[..dir_offset].to_vec();
            out.extend((count as i32).to_le_bytes());
            for _ in 0..count {
                out.extend(entry);
            }
            out
        }
        "flip" => {
            let playback: Vec<Value> = (0..2000)
                .map(|k| {
                    net(
                        f64::from(k) * 0.01,
                        k,
                        &format!("maps/{}.bsp", if k % 2 == 1 { "b" } else { "a" }),
                    )
                })
                .collect();
            goldsrc_frames(&serde_json::json!({"duration": 20, "playback": playback}))
        }
        other => panic!("рецепт {other}"),
    };
    for patch in recipe
        .get("patch")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let at = patch[0].as_u64().unwrap() as usize;
        i32le(&mut buffer, at, patch[1].as_f64().unwrap());
    }
    if let Some(slice) = recipe.get("slice").and_then(Value::as_u64) {
        buffer.truncate(slice as usize);
    }
    if let Some(append) = recipe.get("append").and_then(Value::as_array) {
        buffer.extend(append.iter().map(|b| b.as_u64().unwrap() as u8));
    }
    buffer
}

/// Буфер рецепта с проверкой: тот же размер и sha256, что у генератора 0.4.9.
pub fn input(case: &Value) -> Vec<u8> {
    let bytes = build(&case["gen"]);
    assert_eq!(
        bytes.len() as u64,
        case["size"].as_u64().unwrap(),
        "размер {}",
        case["gen"]
    );
    assert_eq!(
        sha256(&bytes),
        case["sha256"].as_str().unwrap(),
        "байты {}",
        case["gen"]
    );
    bytes
}

/// `writeGhostingRun(demosDir, run, segments, options)`: путь плейлиста.
pub fn write_ghosting_run(demos_dir: &Path, run: &str, segments: &Value, opts: &Value) -> PathBuf {
    let dir = demos_dir.join(run);
    std::fs::create_dir_all(&dir).unwrap();
    let mut timer = 0.0f64;
    let mut entries = Vec::new();
    for (index, segment) in segments.as_array().unwrap().iter().enumerate() {
        let name = format!("segment{:03}.dem", index + 1);
        let duration = num(segment.get("duration"), 0.0);
        let ticks = math_round(duration * 100.0);
        let data = source(
            &serde_json::json!({"map": segment["map"], "playback": duration, "ticks": ticks, "frames": ticks.max(1.0)}),
            segment
                .get("commands")
                .map(|list| source_frames(&commands(list), true)),
        );
        std::fs::write(dir.join(&name), &data).unwrap();
        entries.push(serde_json::json!({"file": format!("demos/{run}/{name}"), "map": segment["map"], "start": timer,
            "end": timer + duration, "size": data.len(), "sha256": sha256(&data)}));
        timer += duration + 0.01;
    }
    let mut options = serde_json::json!({"total": timer});
    if let Some(extra) = opts.as_object() {
        for (key, value) in extra {
            options[key] = value.clone();
        }
    }
    let file = dir.join("run.ghplaylist");
    std::fs::write(&file, playlist(&Value::Array(entries), &options)).unwrap();
    file
}

/// Дерево сценария: файлы по рецептам и раны Ghosting; времена файлов — 1700000010, …20, … по
/// порядку (файлы рана — по имени), как у `writeTree` скрипта.
pub fn write_tree(root: &Path, tree: &Value) {
    let mut mtime = 1_700_000_000u64;
    let mut touch = |file: &Path| {
        mtime += 10;
        let handle = std::fs::File::options().write(true).open(file).unwrap();
        handle
            .set_modified(UNIX_EPOCH + Duration::from_secs(mtime))
            .unwrap();
    };
    for item in tree.as_array().unwrap() {
        if let Some(run) = item.get("run") {
            let demos = run["dir"]
                .as_str()
                .unwrap()
                .split('/')
                .fold(root.to_path_buf(), |p, part| p.join(part));
            let file = write_ghosting_run(
                &demos,
                run["name"].as_str().unwrap(),
                &run["segments"],
                &run["opts"],
            );
            let dir = file.parent().unwrap();
            let mut names: Vec<_> = std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            names.sort();
            for name in names {
                touch(&dir.join(name));
            }
            continue;
        }
        let full = item["path"]
            .as_str()
            .unwrap()
            .split('/')
            .fold(root.to_path_buf(), |p, part| p.join(part));
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, build(&item["gen"])).unwrap();
        touch(&full);
    }
}
