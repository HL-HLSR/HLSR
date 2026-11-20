//! Ремонт (`RepairService`, 0.4.9: только с набора файлов, скачанного с зеркала, — `preparedRoot`).
//!
//! Разбор незавершённого ремонта по журналу (`findPending`, `recover`: журнала нет, битый и другой
//! версии, откат на фазе замены, фаза без замены) — против 0.4.7, фикстура `repair.json` из
//! `tools/reference/delivery/repair.js` (группа «восстановление»: в 0.4.9 эта часть не менялась).
//! Остальные сценарии фикстуры — ремонт из полного архива, которого в 0.4.9 нет; ремонт с набора
//! файлов проверяют тесты ниже, весь путь через службу — `tests/repair_per_file.rs`.
//!
//! Нормализация — та же, что в скрипте: временная папка → `<root>`, pid и время в имени временного
//! файла атомарной записи → `<pid>`, `<ms>`.

mod support;

use hlsr_delivery::errors::{Code, Failure, RawError};
use hlsr_delivery::install_service::FaultInjector;
use hlsr_delivery::paths::{LibraryPaths, resolve_library};
use hlsr_delivery::ports::Logger;
use hlsr_delivery::repair_service::{self, RepairOptions, RepairRequest, RepairService};
use regex::Regex;
use serde_json::{Value, json};
use std::future::Future as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use support::library::{self, Norm, Rule, Timeline, TimelineLogger, outcome, take};
use support::scenario::{self, common_step, path_in, put_file, temp_root};
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("repair.json"));

/// Файлы, у которых в дереве записан текст.
static STATE_FILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\.json$|\.json\.corrupt-").unwrap());

const GAME: &str = "ghosting-renewed";
const GAME_DIR: &str = "lib/Games/ghosting-renewed";

fn now() -> i64 {
    FIXTURE["now"].as_i64().unwrap()
}

fn archive_file(name: &str) -> PathBuf {
    let rel = str_of(&FIXTURE["archives"][name]["file"]);
    path_in(&support::fixture_path("archives"), rel)
}

/// Нормализация сценария: временный файл атомарной записи `.<имя>.<pid>.<время36>.tmp`.
fn norm(root: &Path) -> Norm {
    let pid = std::process::id();
    let temp = Regex::new(&format!(r"\.{pid}\.[0-9a-z]+\.tmp")).unwrap();
    Norm::new(
        root,
        vec![Rule::Pattern(temp, ".<pid>.<ms>.tmp".to_owned())],
        false,
    )
}

fn service(
    paths: &LibraryPaths,
    logger: Arc<dyn Logger>,
    fault: Option<FaultInjector>,
) -> RepairService {
    let clock_now = now();
    RepairService::new(
        paths.clone(),
        RepairOptions {
            logger: Some(logger),
            fault_injector: fault,
            clock: Some(Arc::new(move || clock_now)),
        },
    )
}

// ---------------------------------------------------------------- журнал против 0.4.7

async fn run_step(root: &Path, paths: &LibraryPaths, n: &Arc<Norm>, step: &Value) -> Value {
    let timeline: Timeline = Arc::new(Mutex::new(Vec::new()));
    let logger: Arc<dyn Logger> = Arc::new(TimelineLogger {
        timeline: timeline.clone(),
        norm: n.clone(),
    });
    match str_of(&step["op"]) {
        "recover" => {
            let result = service(paths, logger, None)
                .recover(str_of(&step["gameId"]))
                .map(|recovery| recovery.to_json());
            json!({"outcome": outcome(n, result), "timeline": take(&timeline)})
        }
        "pending" => {
            let result = service(paths, logger, None)
                .find_pending(str_of(&step["gameId"]))
                .map(|pending| pending.unwrap_or(Value::Null))
                .map_err(Failure::from);
            json!({"outcome": outcome(n, result), "timeline": take(&timeline)})
        }
        "tree" => json!({"tree": library::tree_of(root, n, "lib", &STATE_FILE)}),
        "write" => {
            // "<root>" в тексте — временная папка, записанная внутри строки JSON.
            let text = str_of(&step["utf8"]).replace("<root>", n.escaped_root());
            put_file(root, str_of(&step["file"]), &json!({"utf8": text}));
            Value::Null
        }
        _ => common_step(root, step).unwrap_or_else(|| panic!("неизвестный шаг {}", step["op"])),
    }
}

#[tokio::test]
async fn journal_recovery_matches_0_4_7() {
    let mut all = Mismatches::default();
    let mut ran = 0;
    for spec in items(&FIXTURE["scenarios"]) {
        let name = str_of(&spec["name"]);
        if !name.starts_with("восстановление") {
            continue;
        }
        ran += 1;
        let (_temp, root) = temp_root();
        library::setup(&root, &spec["setup"], &archive_file);
        let paths = resolve_library(&root.join("lib").to_string_lossy()).unwrap();
        let n = Arc::new(norm(&root));
        for (index, (step, expected)) in items(&spec["steps"])
            .iter()
            .zip(items(&spec["results"]))
            .enumerate()
        {
            let actual = run_step(&root, &paths, &n, step).await;
            all.check(
                &actual,
                expected,
                format!("{name}, шаг {index} ({})", step["op"]),
            );
        }
        scenario::open_up(&root);
    }
    assert!(ran > 0, "нет сценариев «восстановление»");
    all.finish("восстановление");
}

#[test]
fn constants_match_0_4_7() {
    let c = &FIXTURE["constants"];
    assert_eq!(c["JOURNAL_VERSION"], json!(repair_service::JOURNAL_VERSION));
    assert_eq!(c["FAULTS"], json!(repair_service::FAULTS));
}

// ---------------------------------------------------------------- ремонт с набора файлов зеркала

fn entry(path: &str, text: &str) -> Value {
    json!({"path": path, "size": text.len(), "sha256": support::sha256_hex(text.as_bytes())})
}

fn write(root: &Path, rel: &str, text: &str) {
    let file = path_in(root, rel);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

fn read(root: &Path, rel: &str) -> Option<String> {
    std::fs::read_to_string(path_in(root, rel)).ok()
}

/// Библиотека, установка с битым `hl2.exe` без `bin/new.dll`, набор файлов зеркала с целыми.
struct Bench {
    _temp: tempfile::TempDir,
    root: PathBuf,
    paths: LibraryPaths,
    game: PathBuf,
    set: PathBuf,
    manifest: Value,
}

fn bench() -> Bench {
    let (temp, root) = temp_root();
    let paths = resolve_library(&root.join("lib").to_string_lossy()).unwrap();
    let game = path_in(&root, GAME_DIR);
    let set = root.join("set");
    write(&game, "hl2.exe", "битый");
    write(&game, "ghostingmod/cfg/autoexec.cfg", "мой autoexec\n");
    for (rel, text) in [
        ("hl2.exe", "MZ целый"),
        ("bin/new.dll", "новый"),
        ("ghostingmod/cfg/chapter3.cfg", "map d1_canals_01\n"),
        ("ghostingmod/cfg/autoexec.cfg", "чужой autoexec\n"),
    ] {
        write(&set, rel, text);
    }
    let manifest = json!({"files": [entry("hl2.exe", "MZ целый"), entry("bin/new.dll", "новый")],
        "stockConfigs": [entry("ghostingmod/cfg/chapter3.cfg", "map d1_canals_01\n")]});
    Bench {
        _temp: temp,
        root,
        paths,
        game,
        set,
        manifest,
    }
}

impl Bench {
    fn request(&self, damaged: &[&str]) -> RepairRequest {
        RepairRequest {
            game_id: GAME.to_owned(),
            install_path: self.game.clone(),
            files_manifest: self.manifest.clone(),
            preserve_paths: Vec::new(),
            damaged_files: damaged.iter().map(|s| (*s).to_owned()).collect(),
            prepared_root: Some(self.set.clone()),
        }
    }

    fn service(&self, fault: Option<FaultInjector>) -> RepairService {
        let logger: Arc<dyn Logger> = Arc::new(hlsr_delivery::ports::MemoryLogger::default());
        service(&self.paths, logger, fault)
    }

    fn rollback_is_empty(&self) -> bool {
        std::fs::read_dir(&self.paths.rollback).map_or(true, |mut dir| dir.next().is_none())
    }
}

fn code_of(failure: &Failure) -> Option<Code> {
    match failure {
        Failure::Delivery(error) => Some(error.code),
        Failure::Raw(_) => None,
    }
}

#[tokio::test]
async fn files_from_the_mirror_set_replace_and_create() {
    let b = bench();
    let service = b.service(None);
    let report = service
        .repair(b.request(&["hl2.exe", "bin/new.dll", "ghostingmod/cfg/chapter3.cfg"]))
        .await
        .unwrap();
    assert_eq!(
        report.to_json(),
        json!({"status": "repaired", "failed": [],
               "repaired": ["hl2.exe", "bin/new.dll", "ghostingmod/cfg/chapter3.cfg"]})
    );
    assert_eq!(read(&b.game, "hl2.exe").unwrap(), "MZ целый");
    assert_eq!(read(&b.game, "bin/new.dll").unwrap(), "новый");
    // Стоковый cfg в папке игрока чинится по подписанному списку.
    assert_eq!(
        read(&b.game, "ghostingmod/cfg/chapter3.cfg").unwrap(),
        "map d1_canals_01\n"
    );
    assert!(service.find_pending(GAME).unwrap().is_none());
    assert!(b.rollback_is_empty());
    assert!(!service.is_busy());
}

#[tokio::test]
async fn player_files_and_a_missing_set_are_refused_before_any_write() {
    let b = bench();
    let service = b.service(None);
    let error = service
        .repair(b.request(&["hl2.exe", "ghostingmod/cfg/autoexec.cfg"]))
        .await
        .unwrap_err();
    assert_eq!(code_of(&error), Some(Code::RepairFailed));
    assert!(
        error.to_string().contains("пользовательским данным"),
        "{error}"
    );

    for prepared in [None, Some(b.root.join("нет такого"))] {
        let mut request = b.request(&["hl2.exe"]);
        request.prepared_root = prepared;
        let error = service.repair(request).await.unwrap_err();
        assert_eq!(code_of(&error), Some(Code::RepairSourceMissing));
    }
    assert_eq!(read(&b.game, "hl2.exe").unwrap(), "битый");
    assert_eq!(
        read(&b.game, "ghostingmod/cfg/autoexec.cfg").unwrap(),
        "мой autoexec\n"
    );
}

#[tokio::test]
async fn a_failure_after_replace_rolls_back_in_process() {
    let b = bench();
    let injector: FaultInjector = Arc::new(|name: &str, context: &Value| {
        if name == "after-replace" && context["index"] == 1 {
            return Err(RawError::new("сбой в точке after-replace").into());
        }
        Ok(())
    });
    let service = b.service(Some(injector));
    let error = service
        .repair(b.request(&["hl2.exe", "bin/new.dll"]))
        .await
        .unwrap_err();
    assert_eq!(code_of(&error), Some(Code::RepairFailed));
    assert_eq!(read(&b.game, "hl2.exe").unwrap(), "битый");
    assert!(read(&b.game, "bin/new.dll").is_none());
    assert!(service.find_pending(GAME).unwrap().is_none());
    assert!(b.rollback_is_empty());
}

/// Junction вместо папки игры: ни одной записи за её пределы (`assertRealInside` до замены).
#[cfg(windows)]
#[tokio::test]
async fn a_junction_inside_the_game_is_refused_before_any_write() {
    let b = bench();
    let outside = b.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(b.game.join("bin"))
        .arg(&outside)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J");
    let error = b
        .service(None)
        .repair(b.request(&["hl2.exe", "bin/new.dll"]))
        .await
        .unwrap_err();
    assert_eq!(code_of(&error), Some(Code::PathInvalid));
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    assert_eq!(read(&b.game, "hl2.exe").unwrap(), "битый");
}

/// Не из 0.4.7 (там обещание не бросить): ремонт, брошенный посреди проверки эталона,
/// освобождает сервис, как `finally`; журнал остаётся — его разберёт `recover`.
#[tokio::test]
async fn dropped_repair_releases_service() {
    let b = bench();
    let service = b.service(None);
    {
        let mut task = Box::pin(service.repair(b.request(&["hl2.exe", "bin/new.dll"])));
        let polled =
            std::future::poll_fn(|cx| std::task::Poll::Ready(task.as_mut().poll(cx))).await;
        assert!(
            polled.is_pending(),
            "ремонт не должен закончиться за один опрос"
        );
        assert!(service.is_busy());
    }
    assert!(!service.is_busy());
    let pending = service.find_pending(GAME).unwrap().unwrap();
    assert_eq!(pending["phase"], "verify-source");
    let recovered = service.recover(GAME).unwrap();
    assert!(recovered.recovered);
    assert!(service.find_pending(GAME).unwrap().is_none());
    assert_eq!(read(&b.game, "hl2.exe").unwrap(), "битый");
}
