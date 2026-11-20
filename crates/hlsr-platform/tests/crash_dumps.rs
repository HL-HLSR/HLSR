//! Дампы падений (`crash-dumps.js` 0.4.7) против фикстуры `tools/reference/platform/crash-dumps.js`:
//! папки поиска (win32 и linux), сценарии над настоящей временной папкой (снимки, новые дампы,
//! зоны и порядок, дампы других программ в CrashDumps, `leftNewDump`, `isAllowedDumpPath`), самотест
//! `linux-selftest.js` (минидамп при коде 0) и ожидание, пока дамп допишется (`waitForDump`).

mod support;

use hlsr_core::js::Platform;
use hlsr_core::js::path::NodePath;
use hlsr_delivery::environment::Environment;
use hlsr_platform::crash_dumps::{
    self, DumpContext, DumpSnapshot, WaitOptions, dump_directories_on, find_new_dumps,
    is_allowed_dump_path, is_dump_file, left_new_dump, snapshot_dumps, wait_for_dump,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use support::{Mismatches, fixture, items, str_of};

fn env_of(value: &Value) -> Environment {
    let vars: Vec<(String, String)> = value
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| (key.clone(), str_of(value).to_owned()))
                .collect()
        })
        .unwrap_or_default();
    Environment::new(vars, false)
}

fn dirs_value(dirs: &[crash_dumps::DumpDir]) -> Value {
    Value::Array(
        dirs.iter()
            .map(|dir| json!({"id": dir.id, "dir": dir.dir}))
            .collect(),
    )
}

#[test]
fn dump_file_names_like_0_4_7() {
    let data = fixture("crash-dumps.json");
    let mut mismatches = Mismatches::default();
    for case in items(&data["isDumpFile"]) {
        let name = str_of(&case["name"]);
        mismatches.check_eq(
            is_dump_file(name),
            case["dump"].as_bool().unwrap(),
            format!("isDumpFile({name:?})"),
        );
    }
    mismatches.finish("isDumpFile");
}

#[test]
fn dump_directories_like_0_4_7() {
    let data = fixture("crash-dumps.json");
    let mut mismatches = Mismatches::default();
    for (platform, path) in [
        ("win32", NodePath::new(Platform::Win32, "C:\\work")),
        ("linux", NodePath::new(Platform::Linux, "/work")),
    ] {
        for case in items(&data["directories"][platform]) {
            let install = case["installPath"].as_str();
            let game_id = case["gameId"].as_str();
            let dirs = dump_directories_on(&path, install, &env_of(&case["env"]), game_id);
            mismatches.check(
                &dirs_value(&dirs),
                &case["dirs"],
                format!(
                    "{platform} dumpDirectories({install:?}, {}, {game_id:?})",
                    case["env"]
                ),
            );
        }
    }
    mismatches.finish("dumpDirectories");
}

/// Корень сценария: как `fs.realpathSync(fs.mkdtempSync(…))`.
struct Root {
    _dir: tempfile::TempDir,
    path: String,
}

fn work_root() -> Root {
    let dir = tempfile::tempdir().unwrap();
    let real = std::fs::canonicalize(dir.path()).unwrap();
    let text = real.to_string_lossy().into_owned();
    let path = text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned();
    Root { _dir: dir, path }
}

impl Root {
    fn at(&self, rel: &Value) -> Option<String> {
        let rel = rel.as_str()?;
        if rel.is_empty() {
            return Some(String::new());
        }
        let mut parts = vec![self.path.as_str()];
        parts.extend(rel.split('/'));
        Some(NodePath::native().join(&parts))
    }

    fn norm(&self, text: &str) -> String {
        text.replace(&self.path, "<root>").replace('\\', "/")
    }
}

fn set_mtime(file: &Path, seconds: f64) {
    let handle = std::fs::OpenOptions::new().write(true).open(file).unwrap();
    let time = UNIX_EPOCH + Duration::from_secs_f64(seconds);
    handle
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(time)
                .set_accessed(time),
        )
        .unwrap();
}

fn number(value: f64) -> Value {
    hlsr_core::json_text::js_number(value)
}

fn run_scenario(scenario: &Value) -> Vec<Value> {
    let root = work_root();
    let env = Environment::new(
        [
            (
                "LOCALAPPDATA",
                format!("{}{}local", root.path, std::path::MAIN_SEPARATOR),
            ),
            (
                "XDG_DATA_HOME",
                format!("{}{}xdg", root.path, std::path::MAIN_SEPARATOR),
            ),
        ],
        false,
    );
    let mut snapshots: HashMap<String, DumpSnapshot> = HashMap::new();
    let mut results = Vec::new();
    for step in items(&scenario["steps"]) {
        match str_of(&step["op"]) {
            "dir" => std::fs::create_dir_all(root.at(&step["path"]).unwrap()).unwrap(),
            "write" => {
                let file = PathBuf::from(root.at(&step["path"]).unwrap());
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                let size = step["size"].as_u64().unwrap() as usize;
                std::fs::write(&file, vec![0x41; size]).unwrap();
                set_mtime(&file, step["mtime"].as_f64().unwrap());
            }
            "remove" => {
                let _ = std::fs::remove_file(root.at(&step["path"]).unwrap());
            }
            "snapshot" => {
                let install = root.at(&step["install"]);
                let snapshot = snapshot_dumps(install.as_deref(), &env, step["gameId"].as_str());
                let entries: Vec<Value> = snapshot
                    .iter()
                    .map(|(file, info)| {
                        json!({"path": root.norm(file), "size": info.size,
                               "mtimeMs": number(info.mtime_ms), "zone": info.zone})
                    })
                    .collect();
                results.push(json!({"step": "snapshot", "entries": entries}));
                snapshots.insert(str_of(&step["name"]).to_owned(), snapshot);
            }
            "find" => {
                let snapshot = step["snapshot"].as_str().map(|name| &snapshots[name]);
                let install = root.at(&step["install"]);
                let found = find_new_dumps(
                    install.as_deref(),
                    snapshot,
                    step["startedAtMs"].as_f64(),
                    &env,
                    step["gameId"].as_str(),
                );
                let found: Vec<Value> = found
                    .iter()
                    .map(|dump| {
                        json!({"path": root.norm(&dump.path), "size": dump.size,
                               "mtimeMs": number(dump.mtime_ms), "zone": dump.zone})
                    })
                    .collect();
                results.push(json!({"step": "find", "found": found}));
            }
            "left" => {
                let context = step["snapshot"].as_str().map(|name| DumpContext {
                    snapshot: snapshots[name].clone(),
                    game_id: step["gameId"].as_str().map(str::to_owned),
                    install_path: root.at(&step["install"]),
                    started_at_ms: step["startedAtMs"].as_f64(),
                });
                results
                    .push(json!({"step": "left", "left": left_new_dump(context.as_ref(), &env)}));
            }
            "allowed" => {
                let candidate = root.at(&step["candidate"]).unwrap();
                let install = root.at(&step["install"]);
                results.push(json!({"step": "allowed",
                    "allowed": is_allowed_dump_path(&candidate, install.as_deref(), &env)}));
            }
            other => panic!("неизвестный шаг {other}"),
        }
    }
    results
}

/// Сценарии сняты на Windows: регистр имён в CrashDumps, `ROOT2.DMP` и порядок `readdir` — как у
/// тома NTFS.
#[test]
fn scenarios_like_0_4_7() {
    let data = fixture("crash-dumps.json");
    let mut mismatches = Mismatches::default();
    for scenario in items(&data["scenarios"]) {
        let name = str_of(&scenario["name"]);
        let results = run_scenario(scenario);
        for (index, (actual, expected)) in
            results.iter().zip(items(&scenario["results"])).enumerate()
        {
            mismatches.check(actual, expected, format!("{name}: результат {index}"));
        }
        mismatches.check_eq(
            results.len(),
            items(&scenario["results"]).len(),
            format!("{name}: число результатов"),
        );
    }
    mismatches.finish("сценарии дампов");
}

/// `linux-selftest.js`: «a Proton game that exits with code 0 but leaves a fresh minidump counts
/// as a crash».
#[test]
fn proton_exit_zero_with_fresh_minidump_counts_as_crash() {
    let root = tempfile::tempdir().unwrap();
    let install = root.path().to_string_lossy().into_owned();
    let env = Environment::new(Vec::<(String, String)>::new(), false);
    std::fs::write(root.path().join("hl2_old.mdmp"), "old").unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;
    let context = DumpContext {
        snapshot: snapshot_dumps(Some(&install), &env, None),
        game_id: None,
        install_path: Some(install.clone()),
        started_at_ms: Some(now),
    };
    assert!(!left_new_dump(Some(&context), &env));
    std::fs::write(
        root.path()
            .join("hl2_20261005_235150_1_InvalidParameterHandler.mdmp"),
        "dump",
    )
    .unwrap();
    assert!(left_new_dump(Some(&context), &env));
    assert!(!left_new_dump(None, &env));
}

/// `waitForDump`: дамп готов, когда его размер не изменился между двумя проверками.
#[tokio::test]
async fn wait_for_dump_returns_once_the_dump_stops_growing() {
    let root = tempfile::tempdir().unwrap();
    let install = root.path().to_string_lossy().into_owned();
    let env = Environment::new(Vec::<(String, String)>::new(), false);
    let snapshot = snapshot_dumps(Some(&install), &env, Some("hl2-4260"));
    let mod_dir = root.path().join("hl2");
    std::fs::create_dir_all(&mod_dir).unwrap();
    let file = mod_dir.join("crash.mdmp");
    std::fs::write(&file, vec![1u8; 10]).unwrap();

    let writer = {
        let file = file.clone();
        tokio::spawn(async move {
            for size in [20, 30] {
                tokio::time::sleep(Duration::from_millis(60)).await;
                std::fs::write(&file, vec![1u8; size]).unwrap();
            }
        })
    };
    let options = WaitOptions {
        settle_ms: Some(5000),
        poll_ms: Some(100),
        env: Some(env.clone()),
        game_id: Some("hl2-4260".to_owned()),
    };
    let started = std::time::Instant::now();
    let found = wait_for_dump(Some(&install), Some(&snapshot), None, &options).await;
    writer.await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].size, 30, "ждём, пока дамп допишется");
    assert_eq!(found[0].zone, "game-mod");
    assert!(started.elapsed() < Duration::from_secs(4));
}

/// К сроку — только дампы ненулевого размера; нет дампов — пусто.
#[tokio::test]
async fn wait_for_dump_drops_empty_dumps_at_the_deadline() {
    let root = tempfile::tempdir().unwrap();
    let install = root.path().to_string_lossy().into_owned();
    let env = Environment::new(Vec::<(String, String)>::new(), false);
    let options = WaitOptions {
        settle_ms: Some(250),
        poll_ms: Some(50),
        env: Some(env.clone()),
        game_id: None,
    };
    let snapshot = snapshot_dumps(Some(&install), &env, None);
    assert!(
        wait_for_dump(Some(&install), Some(&snapshot), None, &options)
            .await
            .is_empty()
    );
    std::fs::write(root.path().join("empty.dmp"), b"").unwrap();
    assert!(
        wait_for_dump(Some(&install), Some(&snapshot), None, &options)
            .await
            .is_empty()
    );
}

// ---------------------------------------------------------------- crash-report-selftest.js (0.4.9)

fn write_at(file: &Path, body: &str, mtime_ms: f64) {
    std::fs::write(file, body).unwrap();
    let at = UNIX_EPOCH + Duration::from_millis(mtime_ms as u64);
    std::fs::File::options()
        .write(true)
        .open(file)
        .unwrap()
        .set_modified(at)
        .unwrap();
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// «лог краша: .txt рядом с дампом под тем же именем», «нет .txt — null», «только для дампов».
#[test]
fn the_crash_log_lies_next_to_the_dump() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir
        .path()
        .join("ghosting_crash-2026-10-30-19-08-19-107332.dmp");
    let log = dir
        .path()
        .join("ghosting_crash-2026-10-30-19-08-19-107332.txt");
    std::fs::write(&dump, "MDMP").unwrap();
    std::fs::write(&log, "EXCEPTION_ACCESS_VIOLATION in server.dll+0x1D5F8").unwrap();
    assert_eq!(crash_dumps::crash_log_of(&text(&dump)), Some(text(&log)));

    let alone = dir.path().join("hl2.exe.1234.dmp");
    std::fs::write(&alone, "MDMP").unwrap();
    assert_eq!(crash_dumps::crash_log_of(&text(&alone)), None);

    let mdmp = dir.path().join("crash.mdmp");
    std::fs::write(&mdmp, "MDMP").unwrap();
    std::fs::create_dir(dir.path().join("crash.txt")).unwrap();
    assert_eq!(crash_dumps::crash_log_of(&text(&mdmp)), None);
    assert_eq!(
        crash_dumps::crash_log_of(&text(&dir.path().join("notes.log"))),
        None
    );
}

/// «архив сбоя: самый новый ghosting_crash-*.zip после запуска»; «старый архив, другая игра или
/// нет папки — null».
#[test]
fn the_newest_crash_archive_after_the_start() {
    let env = Environment::new(Vec::<(String, String)>::new(), false);
    let dir = tempfile::tempdir().unwrap();
    let root = text(dir.path());
    let mod_dir = dir.path().join("ghostingmod");
    std::fs::create_dir(&mod_dir).unwrap();
    let started = now_ms() - 60_000.0;
    let at = |name: &str| mod_dir.join(name);
    write_at(
        &at("ghosting_crash-2026-11-01-10-00-00-1.zip"),
        "PK old",
        started - 3_600_000.0,
    );
    assert_eq!(
        crash_dumps::crash_archive_of(Some(&root), Some(started), Some("ghosting-renewed"), &env),
        None
    );
    write_at(
        &at("ghosting_crash-2026-11-07-12-00-00-2.zip"),
        "PK first",
        started + 1000.0,
    );
    write_at(
        &at("ghosting_crash-2026-11-07-12-05-00-3.zip"),
        "PK newest",
        started + 5000.0,
    );
    write_at(
        &at("ghosting-demos-2026-11-07-1200.zip"),
        "PK demos",
        started + 9000.0,
    );
    write_at(
        &at("ghosting_crash-2026-11-07-12-06-00-4.zip"),
        "",
        started + 9000.0,
    );
    std::fs::create_dir(at("ghosting_crash-dir.zip")).unwrap();
    assert_eq!(
        crash_dumps::crash_archive_of(Some(&root), Some(started), Some("ghosting-renewed"), &env),
        Some(text(&at("ghosting_crash-2026-11-07-12-05-00-3.zip")))
    );
    assert_eq!(
        crash_dumps::crash_archive_of(Some(&root), Some(started), Some("hl2-4260"), &env),
        None,
        "у HL2 своя папка мода"
    );
    assert_eq!(
        crash_dumps::crash_archive_of(None, Some(started), Some("ghosting-renewed"), &env),
        None
    );
}

/// «архив сбоя: открывать можно только zip сбоя в папках игры».
#[test]
fn only_a_crash_zip_inside_the_game_can_be_shown() {
    let env = Environment::new(Vec::<(String, String)>::new(), false);
    let dir = tempfile::tempdir().unwrap();
    let root = text(dir.path());
    let mod_dir = dir.path().join("ghostingmod");
    std::fs::create_dir(&mod_dir).unwrap();
    let zip = mod_dir.join("ghosting_crash-2026-11-07-12-00-00-2.zip");
    std::fs::write(&zip, "PK").unwrap();
    std::fs::write(mod_dir.join("notes.zip"), "PK").unwrap();
    assert!(is_allowed_dump_path(&text(&zip), Some(&root), &env));
    assert!(!is_allowed_dump_path(
        &text(&mod_dir.join("notes.zip")),
        Some(&root),
        &env
    ));
    let other = tempfile::tempdir().unwrap();
    let outside = other.path().join("ghosting_crash-x.zip");
    std::fs::write(&outside, "PK").unwrap();
    assert!(!is_allowed_dump_path(&text(&outside), Some(&root), &env));
}
