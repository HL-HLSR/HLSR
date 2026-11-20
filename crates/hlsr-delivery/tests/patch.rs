//! Патчи против 0.4.7: фикстура `patch.json` из `tools/reference/delivery/patch.js`.
//!
//! `PatchService` (цепочка 1.0.0 → 1.0.1 → 1.0.2 из архивов с `payload/`, cfg игрока и
//! сохраняемые пути, метаданные сборки, каждая точка `FAULTS` и сбой процесса на каждой фазе с
//! восстановлением `recover`, откат в том же процессе и неудачный откат, отказы по манифесту и
//! архиву патча, `findPending`, журналы разных версий и битые) — на тех же сценариях, что выполнил
//! модуль 0.4.7 в Node 24, на настоящем 7za из `resources/7zip` и архивах
//! `tests/fixtures/archives/patch`. Сравниваются итог каждого шага, лента событий `state`, строк
//! журнала и точек отказа, деревья библиотеки (путь, размер, sha256; у `.json` и `.previous` —
//! текст: байты `installed.json`, `installed.json.previous`, `<id>.patch.json`).
//!
//! Нормализация — та же, что в скрипте: временная папка → `<root>`, pid и время в имени временного
//! файла атомарной записи → `<pid>`, `<ms>`, текст ошибки 7-Zip битого архива → `<stderr>`;
//! подряд идущие события прогресса распаковки — одна запись, проценты не сравниваются.
//!
//! Сценарии с пометкой `unix` зависят от прав доступа, регистра имён и процессов Linux: в Windows
//! и от root они пропускаются.

mod support;

use hlsr_delivery::archive_service::ArchiveService;
use hlsr_delivery::errors::RawError;
use hlsr_delivery::patch_service::{
    self, FaultInjector, FilesManifestOf, PatchEvent, PatchOptions, PatchRequest, PatchService,
};
use hlsr_delivery::paths::{LibraryPaths, resolve_library};
use hlsr_delivery::ports::{FakeFreeSpace, Logger};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use support::library::{
    self, Norm, Rule, Timeline, TimelineLogger, copy_tree, outcome, push, take,
};
use support::scenario::{self, common_step, path_in, put_file, temp_root};
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("patch.json"));

/// Файлы, у которых в дереве записан текст.
static STATE_FILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\.(json|previous)$|\.json\.corrupt-").unwrap());

fn now() -> i64 {
    FIXTURE["now"].as_i64().unwrap()
}

fn real_7za() -> PathBuf {
    hlsr_core::paths::Resources::detect().sevenzip()
}

/// Сообщения 7za зависят от локали, время записей — от пояса.
fn pinned_env() -> [(&'static str, &'static str); 3] {
    [("LC_ALL", "C.UTF-8"), ("LANG", "C.UTF-8"), ("TZ", "UTC")]
}

fn archive_file(name: &str) -> PathBuf {
    let rel = str_of(&FIXTURE["archives"][name]["file"]);
    path_in(&support::fixture_path("archives"), rel)
}

/// Нормализация сценария: временный файл атомарной записи `.<имя>.<pid>.<время36>.tmp`.
fn norm(root: &Path, stderr: bool) -> Norm {
    let pid = std::process::id();
    let temp = Regex::new(&format!(r"\.{pid}\.[0-9a-z]+\.tmp")).unwrap();
    Norm::new(
        root,
        vec![Rule::Pattern(temp, ".<pid>.<ms>.tmp".to_owned())],
        stderr,
    )
}

// ---------------------------------------------------------------- игра запущена

/// Процесс, у которого в командной строке путь exe игры (так его ищет `isGameRunning` в `/proc`).
struct Game(std::process::Child);

impl Drop for Game {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(unix)]
fn hold_game(exe: &Path) -> Game {
    use std::os::unix::process::CommandExt;
    let child = std::process::Command::new("sleep")
        .arg0(exe)
        .arg("60")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    Game(child)
}

#[cfg(not(unix))]
fn hold_game(_exe: &Path) -> Game {
    unreachable!("сценарий «игра запущена» — только unix")
}

// ---------------------------------------------------------------- сервис сценария

struct Ctx {
    root: PathBuf,
    lib: PathBuf,
    crash: PathBuf,
    paths: LibraryPaths,
    norm: Arc<Norm>,
    game: Mutex<Option<Game>>,
}

impl Ctx {
    /// Сервис шага: свой журнал, место, часы, точки отказа, файловый манифест прежней версии
    /// (`previousFiles`: версия из фикстуры, `"throws"` — ошибка, нет — вшитый) и слушатель
    /// событий (`crashAt`, `onState`).
    fn service(
        &self,
        step: &Value,
        timeline: &Timeline,
        calls: &Arc<Mutex<Vec<Value>>>,
        free: Option<u64>,
    ) -> Arc<PatchService> {
        let logger: Arc<dyn Logger> = Arc::new(TimelineLogger {
            timeline: timeline.clone(),
            norm: self.norm.clone(),
        });
        let free_space = Arc::new(FakeFreeSpace::default());
        free_space.set(&self.paths.staging, free);
        let options = &step["service"];
        let injector: FaultInjector = {
            let (timeline, fault) = (timeline.clone(), step["fault"].clone());
            let (lib, crash) = (self.lib.clone(), self.crash.clone());
            Arc::new(move |name: &str, journal: &Value| {
                push(&timeline, json!({"fault": name, "phase": journal["phase"]}));
                if fault["point"] != name {
                    return Ok(());
                }
                if fault["crash"] == true {
                    copy_tree(&lib, &crash);
                }
                Err(RawError::new(format!("сбой в точке {name}")).into())
            })
        };
        let files_manifest_of: Option<FilesManifestOf> = step.get("previousFiles").map(|spec| {
            let (spec, calls) = (spec.clone(), calls.clone());
            Arc::new(move |game_id: &str, version: &str| {
                calls.lock().unwrap().push(json!([game_id, version]));
                match spec.as_str() {
                    Some("throws") => None,
                    Some(version) => Some(FIXTURE["files"][version].clone()),
                    None => None,
                }
            }) as FilesManifestOf
        });
        let archive = ArchiveService::new(real_7za(), logger.clone()).with_env(pinned_env());
        let clock_now = now();
        let service = Arc::new(PatchService::new(
            self.paths.clone(),
            PatchOptions {
                archive_service: Some(archive),
                logger: Some(logger),
                space_margin: options["spaceMargin"].as_u64(),
                launcher_version: options["launcherVersion"].as_str().map(str::to_owned),
                files_manifest_of,
                fault_injector: Some(injector),
                free_space: Some(free_space),
                clock: Some(Arc::new(move || clock_now)),
            },
        ));
        let (step, timeline, norm) = (step.clone(), timeline.clone(), self.norm.clone());
        let (root, lib, crash) = (self.root.clone(), self.lib.clone(), self.crash.clone());
        let seen: Mutex<HashMap<String, u64>> = Mutex::new(HashMap::new());
        let crashed = Mutex::new(false);
        service.on_event(move |event: &PatchEvent| {
            // Прогресс распаковки: подряд идущие события — одной записью, проценты не
            // сравниваются (число событий и проценты зависят от вывода 7za).
            if event.percent.is_some() {
                let mut list = timeline.lock().unwrap();
                if list.last().is_none_or(|last| last["ev"] != "progress") {
                    let mut data = norm.value(&event.to_json());
                    data["percent"] = json!("<percent>");
                    list.push(json!({"ev": "progress", "data": data}));
                }
                return;
            }
            push(
                &timeline,
                json!({"ev": "state", "data": norm.value(&event.to_json())}),
            );
            let Some(phase) = event.phase.clone() else {
                return;
            };
            let count = {
                let mut seen = seen.lock().unwrap();
                let count = seen.entry(phase.clone()).or_insert(0);
                *count += 1;
                *count
            };
            if step["crashAt"] == phase.as_str() {
                let mut crashed = crashed.lock().unwrap();
                if !*crashed {
                    *crashed = true;
                    copy_tree(&lib, &crash);
                }
            }
            for action in step["onState"].as_array().into_iter().flatten() {
                if action["phase"] != phase.as_str() || action["nth"].as_u64().unwrap_or(1) != count
                {
                    continue;
                }
                if let Some(write) = action.get("write") {
                    put_file(&root, str_of(&write["file"]), write);
                }
                if let Some(chmod) = action.get("chmod") {
                    scenario::chmod(
                        &path_in(&root, str_of(&chmod["file"])),
                        chmod["mode"].as_u64().unwrap() as u32,
                    );
                }
                if let Some(remove) = action.get("remove") {
                    let path = path_in(&root, str_of(remove));
                    let _ = std::fs::remove_dir_all(&path).or_else(|_| std::fs::remove_file(&path));
                }
                if let Some(dir) = action.get("mkdir") {
                    std::fs::create_dir_all(path_in(&root, str_of(dir))).unwrap();
                }
            }
        });
        service
    }

    fn request(&self, step: &Value) -> PatchRequest {
        let patch_manifest = match step.get("patchManifest") {
            Some(manifest) => manifest.clone(),
            None => FIXTURE["patches"][str_of(&step["patch"])].clone(),
        };
        PatchRequest {
            game_manifest: FIXTURE["gameManifests"][str_of(&step["game"])].clone(),
            patch_manifest,
            installed_version: step["installedVersion"].as_str().map(str::to_owned),
            archive_path: path_in(&self.root, str_of(&step["archive"])),
        }
    }
}

async fn run_step(ctx: &Ctx, step: &Value) -> Value {
    let timeline: Timeline = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let n = &ctx.norm;
    match str_of(&step["op"]) {
        "apply" => {
            let free = step["freeSpace"].as_u64();
            let service = ctx.service(step, &timeline, &calls, free);
            let result = service
                .apply(ctx.request(step))
                .await
                .map(|applied| applied.to_json());
            if step["brief"] == true {
                // Патч, который дошёл до конца (и проверен в других сценариях), — только итог.
                return match result {
                    Ok(applied) => json!({"outcome": {"ok": applied["status"]}}),
                    Err(failure) => json!({"outcome": library::failure_of(n, &failure)}),
                };
            }
            let mut out = json!({
                "outcome": outcome(n, result),
                "timeline": take(&timeline),
                "busy": service.is_busy(),
                "calls": Value::Array(calls.lock().unwrap().clone()),
            });
            if step["repeat"] == true {
                let again = service
                    .apply(ctx.request(step))
                    .await
                    .map(|applied| applied.to_json());
                out["again"] = outcome(n, again);
            }
            out
        }
        "applyConcurrent" => {
            let first = &step["first"];
            let service = ctx.service(first, &timeline, &calls, None);
            let task = {
                let (service, request) = (service.clone(), ctx.request(first));
                tokio::spawn(async move { service.apply(request).await })
            };
            while !service.is_busy() {
                tokio::task::yield_now().await;
            }
            let busy = service.is_busy();
            let second = service.apply(ctx.request(&step["second"])).await;
            let first = task.await.unwrap();
            json!({
                "busy": busy,
                "second": outcome(n, second.map(|applied| applied.to_json())),
                "first": outcome(n, first.map(|applied| applied.to_json())),
                "timeline": take(&timeline),
            })
        }
        "restoreCrash" => {
            library::restore_crash(&ctx.lib, &ctx.crash);
            Value::Null
        }
        "recover" => {
            let service = ctx.service(step, &timeline, &calls, None);
            let result = service
                .recover(str_of(&step["gameId"]))
                .map(|recovery| recovery.to_json());
            json!({"outcome": outcome(n, result), "timeline": take(&timeline)})
        }
        "pending" => {
            let service = ctx.service(step, &timeline, &calls, None);
            let pending = service.find_pending(str_of(&step["gameId"]));
            json!({
                "outcome": outcome(n, Ok(pending.unwrap_or(Value::Null))),
                "timeline": take(&timeline),
            })
        }
        "holdGame" => {
            let exe = path_in(&ctx.root, str_of(&step["file"]));
            *ctx.game.lock().unwrap() = Some(hold_game(&exe));
            Value::Null
        }
        "releaseGame" => {
            ctx.game.lock().unwrap().take();
            Value::Null
        }
        "tree" => json!({"tree": library::tree_of(&ctx.root, n, "lib", &STATE_FILE)}),
        "write" => {
            // "<root>" в тексте — временная папка, записанная внутри строки JSON.
            let text = str_of(&step["utf8"]).replace("<root>", n.escaped_root());
            put_file(&ctx.root, str_of(&step["file"]), &json!({"utf8": text}));
            Value::Null
        }
        _ => {
            common_step(&ctx.root, step).unwrap_or_else(|| panic!("неизвестный шаг {}", step["op"]))
        }
    }
}

/// Отступление от 0.4.7 (правило 10, Ruling 200 в progress.md): ошибка первой записи журнала
/// там оставляла сервис занятым до перезапуска; здесь сервис свободен, и повтор упирается в ту
/// же ошибку записи, а не в `ALREADY_RUNNING`.
fn departure(name: &str, expected: &Value) -> Value {
    let mut expected = expected.clone();
    if name == "отказ: журнал не пишется — сервис остаётся занят"
        && let Some(result) = expected.as_object_mut()
        && result.contains_key("again")
    {
        result.insert("busy".to_owned(), json!(false));
        let first = result["outcome"].clone();
        result.insert("again".to_owned(), first);
    }
    expected
}

async fn run_scenario(spec: &Value, m: &mut Mismatches) {
    let (_temp, root) = temp_root();
    library::setup(&root, &spec["setup"], &archive_file);
    let lib = root.join("lib");
    let ctx = Ctx {
        paths: resolve_library(&lib.to_string_lossy()).unwrap(),
        crash: root.join("crash"),
        norm: Arc::new(norm(&root, spec["stderr"] == true)),
        game: Mutex::new(None),
        lib,
        root: root.clone(),
    };
    let name = str_of(&spec["name"]);
    assert_eq!(
        items(&spec["steps"]).len(),
        items(&spec["results"]).len(),
        "шагов и результатов поровну"
    );
    for (index, (step, expected)) in items(&spec["steps"])
        .iter()
        .zip(items(&spec["results"]))
        .enumerate()
    {
        let actual = run_step(&ctx, step).await;
        m.check(
            &actual,
            &departure(name, expected),
            format!("{name}, шаг {index} ({})", step["op"]),
        );
    }
    ctx.game.lock().unwrap().take();
    scenario::open_up(&root);
}

/// Сценарии, имя которых начинается с `prefix`; пометка `unix` — только в Linux и не от root.
async fn run_group(prefix: &str) {
    let mut all = Mismatches::default();
    let mut ran = 0;
    for spec in items(&FIXTURE["scenarios"]) {
        if !str_of(&spec["name"]).starts_with(prefix) {
            continue;
        }
        if spec["unix"] == true && (cfg!(not(unix)) || scenario::is_root_user()) {
            continue;
        }
        ran += 1;
        run_scenario(spec, &mut all).await;
    }
    assert!(ran > 0, "нет сценариев «{prefix}»");
    all.finish(prefix);
}

const GROUPS: [&str; 7] = [
    "цепочка",
    "метаданные",
    "сбой",
    "откат",
    "отказ",
    "одновременно",
    "восстановление",
];

macro_rules! groups {
    ($($test:ident => $prefix:literal),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $test() {
                run_group($prefix).await;
            }
        )*
    };
}

groups! {
    patch_chain_matches_0_4_7 => "цепочка",
    build_metadata_matches_0_4_7 => "метаданные",
    crash_recovery_matches_0_4_7 => "сбой",
    rollback_matches_0_4_7 => "откат",
    rejections_match_0_4_7 => "отказ",
    concurrent_patch_matches_0_4_7 => "одновременно",
    journal_recovery_matches_0_4_7 => "восстановление",
}

#[test]
fn every_scenario_is_in_a_group() {
    for spec in items(&FIXTURE["scenarios"]) {
        let name = str_of(&spec["name"]);
        assert!(
            GROUPS.iter().any(|prefix| name.starts_with(prefix)),
            "сценарий «{name}» не входит ни в одну группу"
        );
    }
}

#[test]
fn constants_match_0_4_7() {
    let c = &FIXTURE["constants"];
    assert_eq!(c["JOURNAL_VERSION"], json!(patch_service::JOURNAL_VERSION));
    assert_eq!(c["COMMIT_PHASES"], json!(patch_service::COMMIT_PHASES));
    assert_eq!(c["FAULTS"], json!(patch_service::FAULTS));
}

#[test]
fn fixture_archives_are_the_ones_the_script_built() {
    for (name, info) in FIXTURE["archives"].as_object().unwrap() {
        let bytes = std::fs::read(archive_file(name)).unwrap();
        assert_eq!(bytes.len() as u64, info["size"].as_u64().unwrap(), "{name}");
        assert_eq!(
            support::sha256_hex(&bytes),
            str_of(&info["sha256"]),
            "{name}"
        );
    }
}

/// Не из 0.4.7 (там обещание не бросить): патч, который бросили посреди работы, освобождает
/// сервис, как `finally`; журнал остаётся — его разберёт `recover`, как после сбоя процесса.
#[tokio::test]
async fn dropped_patch_releases_service() {
    use std::future::Future as _;
    let (_temp, root) = temp_root();
    let spec = items(&FIXTURE["scenarios"])
        .iter()
        .find(|spec| str_of(&spec["name"]) == "сбой на фазе preflight")
        .unwrap();
    library::setup(&root, &spec["setup"], &archive_file);
    let paths = resolve_library(&root.join("lib").to_string_lossy()).unwrap();
    let service = PatchService::new(
        paths,
        PatchOptions {
            archive_service: Some(ArchiveService::new(
                real_7za(),
                Arc::new(hlsr_delivery::ports::MemoryLogger::default()),
            )),
            ..PatchOptions::default()
        },
    );
    let ctx = Ctx {
        paths: service.paths().clone(),
        crash: root.join("crash"),
        norm: Arc::new(norm(&root, false)),
        game: Mutex::new(None),
        lib: root.join("lib"),
        root: root.clone(),
    };
    let request = ctx.request(&items(&spec["steps"])[0]);
    {
        let mut task = Box::pin(service.apply(request));
        // Один опрос: журнал записан, дальше — список архива (7za).
        let polled =
            std::future::poll_fn(|cx| std::task::Poll::Ready(task.as_mut().poll(cx))).await;
        assert!(
            polled.is_pending(),
            "патч не должен закончиться за один опрос"
        );
        assert!(service.is_busy());
    }
    assert!(!service.is_busy());
    assert!(service.journal_path("ghosting-renewed").exists());
    let pending = service.find_pending("ghosting-renewed").unwrap();
    assert_eq!(pending["phase"], "inspect");
}

/// Отступление от 0.4.7 (правило 10): там ошибка первой записи журнала оставляла сервис
/// занятым до перезапуска. Здесь патч отказывает, а сервис свободен.
#[tokio::test]
async fn journal_write_failure_releases_service() {
    let (_temp, root) = temp_root();
    let spec = items(&FIXTURE["scenarios"])
        .iter()
        .find(|spec| str_of(&spec["name"]) == "сбой на фазе preflight")
        .unwrap();
    library::setup(&root, &spec["setup"], &archive_file);
    let paths = resolve_library(&root.join("lib").to_string_lossy()).unwrap();
    let service = PatchService::new(
        paths,
        PatchOptions {
            archive_service: Some(ArchiveService::new(
                real_7za(),
                Arc::new(hlsr_delivery::ports::MemoryLogger::default()),
            )),
            ..PatchOptions::default()
        },
    );
    // Каталог на месте файла журнала: запись не удаётся.
    let journal = service.journal_path("ghosting-renewed");
    std::fs::create_dir_all(journal.join("blocker")).unwrap();
    let ctx = Ctx {
        paths: service.paths().clone(),
        crash: root.join("crash"),
        norm: Arc::new(norm(&root, false)),
        game: Mutex::new(None),
        lib: root.join("lib"),
        root: root.clone(),
    };
    let request = ctx.request(&items(&spec["steps"])[0]);
    let error = format!("{:?}", service.apply(request).await.unwrap_err());
    // rename поверх каталога: EISDIR в Linux, EPERM в Windows (у Node так же).
    let code = if cfg!(windows) { "EPERM" } else { "EISDIR" };
    assert!(error.contains(code) && error.contains("rename"), "{error}");
    assert!(!service.is_busy());
}
