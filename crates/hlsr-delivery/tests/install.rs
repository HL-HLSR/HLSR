//! Установка против 0.4.7: фикстура `install.json` из `tools/reference/delivery/install.js`.
//!
//! `InstallService` (установка из архива, обновление полной сборкой, каждая точка
//! `UPDATE_FAULTS` и восстановление после неё, `install.lock`, уборка staging, `installed.json`),
//! `library_recovery` и `library_migration` — на тех же сценариях, что выполнил модуль 0.4.7 в
//! Node 24, на настоящем 7za из `resources/7zip` и архивах `tests/fixtures/archives/install`.
//! Сравниваются итог каждого шага, лента событий `state`, строк журнала и точек отказа, деревья
//! библиотеки (путь, размер, sha256; у `.json` и `.lock` — текст: байты `installed.json`,
//! `<id>.operation.json`, `<id>.last-failure.json`, `install.lock`).
//!
//! Нормализация — та же, что в скрипте: временная папка → `<root>`, случайная часть имени staging
//! → `<rand>`, pid и имя хоста → `<pid>`, `<host>`, текст ошибки 7-Zip битого архива → `<stderr>`,
//! подряд идущие события `extracting` после первого не сравниваются.
//!
//! Сценарии с пометкой `unix` зависят от прав доступа, pid 1 и текстов ошибок Linux: в Windows и
//! от root они пропускаются.

mod support;

use hlsr_core::js;
use hlsr_delivery::archive_service::ArchiveService;
use hlsr_delivery::errors::{Code, DeliveryError, Failure, RawError};
use hlsr_delivery::install_service::{
    self, Cleanup, FaultInjector, FullUpdate, InstallEvent, InstallOptions, InstallRequest,
    InstallService, IntegrityCheck, IntegrityReport, IntegrityRequest, OrphanOptions,
};
use hlsr_delivery::library_migration::{detect_legacy_layout, migrate_legacy_game};
use hlsr_delivery::library_recovery::{installed_games_at, recover_library_path};
use hlsr_delivery::paths::{LibraryPaths, resolve_library};
use hlsr_delivery::ports::{FakeFreeSpace, Logger};
use hlsr_delivery::providers::BoxFuture;
use regex::Regex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use support::library::{
    self, Norm, Rule, Timeline, TimelineLogger, copy_tree, fill_str, outcome, push, take,
};
use support::scenario::{self, common_step, path_in, put_file, temp_root};
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("install.json"));

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

// ---------------------------------------------------------------- нормализация

/// Нормализация сценария: случайная часть имени staging, pid и имя хоста (в `install.lock` и в
/// имени пробного файла `checkWritable`).
fn norm(root: &Path, stderr: bool) -> Norm {
    let ts36 = js::base36(now() as u128);
    let pid = std::process::id();
    let host = serde_json::to_string(&js::process::hostname()).unwrap();
    Norm::new(
        root,
        vec![
            Rule::Pattern(
                Regex::new(&format!("-{ts36}-[0-9a-f]{{8}}")).unwrap(),
                format!("-{ts36}-<rand>"),
            ),
            Rule::Literal(
                format!(".hlsr-write-test-{pid}-"),
                ".hlsr-write-test-<pid>-".to_owned(),
            ),
            Rule::Literal(format!("\"pid\": {pid},"), "\"pid\": \"<pid>\",".to_owned()),
            Rule::Literal(
                format!("\"host\": {host}"),
                "\"host\": \"<host>\"".to_owned(),
            ),
        ],
        stderr,
    )
}

fn delivery(result: Result<Value, DeliveryError>) -> Result<Value, Failure> {
    result.map_err(Failure::Delivery)
}

/// Файлы, у которых в дереве записан текст.
static STATE_FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.(json|lock)$").unwrap());

fn archive_file(name: &str) -> PathBuf {
    let rel = str_of(&FIXTURE["archives"][name]["file"]);
    path_in(&support::fixture_path("archives"), rel)
}

/// Время записи файла или папки (`fs.utimesSync`).
fn set_mtime_any(path: &Path, seconds: f64) {
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs_f64(seconds);
    let mut options = std::fs::OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        options.write(true).custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    }
    #[cfg(not(windows))]
    options.read(true);
    let handle = options.open(path).unwrap();
    handle
        .set_times(std::fs::FileTimes::new().set_modified(time))
        .unwrap();
}

// ---------------------------------------------------------------- сервис сценария

/// Подделка сервиса проверки файлов: отвечает тем, что задал шаг, и записывает вызовы.
struct Integrity {
    spec: Value,
    calls: Arc<Mutex<Vec<Value>>>,
    norm: Arc<Norm>,
}

impl IntegrityCheck for Integrity {
    fn check<'a>(
        &'a self,
        request: IntegrityRequest<'a>,
    ) -> BoxFuture<'a, Result<IntegrityReport, Failure>> {
        Box::pin(async move {
            let mut names: Vec<String> = std::fs::read_dir(request.install_path)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort_by(|a, b| js::cmp_utf16(a, b));
            self.calls.lock().unwrap().push(json!({
                "gameId": request.game_id,
                "installPath": self.norm.string(&request.install_path.to_string_lossy()),
                "version": request.files_manifest["version"],
                "preservePaths": request.preserve_paths,
                "names": names,
            }));
            if let Some(throws) = self.spec.get("throws") {
                return Err(DeliveryError::new(
                    Code::from_name(str_of(&throws["code"])).unwrap(),
                    str_of(&throws["message"]),
                )
                .into());
            }
            let result = &self.spec["result"];
            let list = |key: &str| -> Vec<String> {
                items(&result[key])
                    .iter()
                    .map(|item| match item {
                        Value::String(path) => path.clone(),
                        other => str_of(&other["path"]).to_owned(),
                    })
                    .collect()
            };
            Ok(IntegrityReport {
                ok: result["ok"] == true,
                damaged: result["damaged"].as_u64().unwrap() as usize,
                missing: list("missing"),
                corrupted: list("corrupted"),
                size_mismatch: list("sizeMismatch"),
                extra: list("extra"),
            })
        })
    }
}

struct Ctx {
    root: PathBuf,
    lib: PathBuf,
    crash: PathBuf,
    paths: LibraryPaths,
    norm: Arc<Norm>,
}

impl Ctx {
    fn service(&self, step: &Value, timeline: &Timeline, free: Option<u64>) -> Arc<InstallService> {
        let logger: Arc<dyn Logger> = Arc::new(TimelineLogger {
            timeline: timeline.clone(),
            norm: self.norm.clone(),
        });
        let free_space = Arc::new(FakeFreeSpace::default());
        free_space.set(&self.paths.staging, free);
        let options = &step["service"];
        let injector: FaultInjector = {
            let (timeline, fault) = (timeline.clone(), step["fault"].clone());
            let (root, lib, crash) = (self.root.clone(), self.lib.clone(), self.crash.clone());
            Arc::new(move |name: &str, operation: &Value| {
                push(
                    &timeline,
                    json!({"fault": name, "phase": operation["phase"]}),
                );
                if fault["point"] != name {
                    return Ok(());
                }
                if let Some(chmod) = fault.get("chmod") {
                    scenario::chmod(
                        &path_in(&root, str_of(&chmod["file"])),
                        chmod["mode"].as_u64().unwrap() as u32,
                    );
                }
                if fault["crash"] == true {
                    copy_tree(&lib, &crash);
                }
                Err(RawError::new(format!("сбой в точке {name}")).into())
            })
        };
        let archive = ArchiveService::new(real_7za(), logger.clone()).with_env(pinned_env());
        let clock_now = now();
        let step = step.clone();
        let (root, norm) = (self.root.clone(), self.norm.clone());
        let timeline = timeline.clone();
        Arc::new_cyclic(|weak: &Weak<InstallService>| {
            let service = InstallService::new(
                self.paths.clone(),
                InstallOptions {
                    archive_service: Some(archive),
                    logger: Some(logger),
                    space_margin: options["spaceMargin"].as_u64(),
                    launcher_version: options["launcherVersion"].as_str().map(str::to_owned),
                    metadata_now: options["metadataNow"].as_f64(),
                    fault_injector: Some(injector),
                    free_space: Some(free_space),
                    clock: Some(Arc::new(move || clock_now)),
                },
            );
            let weak = weak.clone();
            let seen: Mutex<HashMap<String, u64>> = Mutex::new(HashMap::new());
            service.on_event(move |event: &InstallEvent| {
                let data = event.to_json();
                let state = str_of(&data["state"]).to_owned();
                {
                    let mut list = timeline.lock().unwrap();
                    let repeat = state == "extracting"
                        && list.last().is_some_and(|last| {
                            last["ev"] == "state" && last["data"]["state"] == "extracting"
                        });
                    if !repeat {
                        list.push(json!({"ev": "state", "data": norm.value(&data)}));
                    }
                }
                let count = {
                    let mut seen = seen.lock().unwrap();
                    let count = seen.entry(state.clone()).or_insert(0);
                    *count += 1;
                    *count
                };
                for action in step["onState"].as_array().into_iter().flatten() {
                    if action["state"] != state.as_str()
                        || action["nth"].as_u64().unwrap_or(1) != count
                    {
                        continue;
                    }
                    if action["cancel"] == true {
                        let result = weak.upgrade().is_some_and(|service| service.cancel());
                        push(&timeline, json!({"action": "cancel", "result": result}));
                    }
                    if let Some(chmod) = action.get("chmod") {
                        scenario::chmod(
                            &path_in(&root, str_of(&chmod["file"])),
                            chmod["mode"].as_u64().unwrap() as u32,
                        );
                    }
                }
            });
            service
        })
    }

    fn request(&self, step: &Value, calls: &Arc<Mutex<Vec<Value>>>) -> InstallRequest {
        let update = step.get("update").map(|upd| FullUpdate {
            from_version: str_of(&upd["fromVersion"]).to_owned(),
            files_manifest: Some(upd["filesManifest"].clone()).filter(|m| !m.is_null()),
            integrity: (!upd["integrity"].is_null()).then(|| {
                Arc::new(Integrity {
                    spec: upd["integrity"].clone(),
                    calls: calls.clone(),
                    norm: self.norm.clone(),
                }) as Arc<dyn IntegrityCheck>
            }),
        });
        InstallRequest {
            manifest: step["manifest"].clone(),
            manifest_sha256: step["manifestSha256"].as_str().map(str::to_owned),
            archive_path: path_in(&self.root, str_of(&step["archive"])),
            files_manifest: None,
            published_at: step["publishedAt"].as_str().map(str::to_owned),
            update,
        }
    }
}

fn game_ids(value: &Value) -> Vec<&str> {
    items(value).iter().map(str_of).collect()
}

async fn run_step(ctx: &Ctx, step: &Value) -> Value {
    let timeline: Timeline = Arc::new(Mutex::new(Vec::new()));
    let n = &ctx.norm;
    match str_of(&step["op"]) {
        "install" => {
            let free = step["freeSpace"].as_u64();
            let service = ctx.service(step, &timeline, free);
            let calls = Arc::new(Mutex::new(Vec::new()));
            let result = service.install(ctx.request(step, &calls)).await;
            let mut out = json!({
                "outcome": outcome(n, result),
                "timeline": take(&timeline),
                "busy": service.is_busy(),
            });
            if step.get("update").is_some() {
                out["integrity"] = Value::Array(calls.lock().unwrap().clone());
            }
            out
        }
        "installConcurrent" => {
            let first = &step["first"];
            let service = ctx.service(first, &timeline, None);
            let calls = Arc::new(Mutex::new(Vec::new()));
            let task = {
                let (service, request) = (service.clone(), ctx.request(first, &calls));
                tokio::spawn(async move { service.install(request).await })
            };
            while !service.is_busy() {
                tokio::task::yield_now().await;
            }
            let busy = service.is_busy();
            let second = service.install(ctx.request(&step["second"], &calls)).await;
            let first = task.await.unwrap();
            json!({
                "busy": busy,
                "second": outcome(n, second),
                "first": outcome(n, first),
                "timeline": take(&timeline),
            })
        }
        "cancelIdle" => json!({"cancel": ctx.service(step, &timeline, None).cancel()}),
        "restoreCrash" => {
            library::restore_crash(&ctx.lib, &ctx.crash);
            Value::Null
        }
        "recover" => {
            let service = ctx.service(step, &timeline, None);
            let result = service
                .recover_full_update(str_of(&step["gameId"]))
                .map(|r| r.to_json());
            json!({"outcome": outcome(n, result), "timeline": take(&timeline)})
        }
        "cleanupPending" => {
            let service = ctx.service(step, &timeline, None);
            let result = service
                .cleanup_pending(str_of(&step["gameId"]))
                .map(|r: Cleanup| r.to_json());
            json!({"outcome": outcome(n, result), "timeline": take(&timeline)})
        }
        "pending" => {
            let service = ctx.service(step, &timeline, None);
            let list = service.list_pending_operations().map(Value::Array);
            let full = service.pending_full_update(str_of(&step["gameId"]));
            json!({
                "list": outcome(n, list),
                "full": outcome(n, Ok(full.unwrap_or(Value::Null))),
                "timeline": take(&timeline),
            })
        }
        "orphans" => {
            let service = ctx.service(step, &timeline, None);
            let removed = service.cleanup_orphan_staging(OrphanOptions {
                min_age_ms: step["minAgeMs"].as_f64(),
                now: step["now"].as_f64(),
            });
            json!({"outcome": {"ok": removed}, "timeline": take(&timeline)})
        }
        "readInstalled" => {
            let service = ctx.service(step, &timeline, None);
            outcome(
                n,
                delivery(
                    service
                        .read_installed(str_of(&step["gameId"]))
                        .map(|v| v.unwrap_or(Value::Null)),
                ),
            )
        }
        "fillInstalled" => {
            let service = ctx.service(step, &timeline, None);
            let result = service
                .fill_installed(str_of(&step["gameId"]), &step["patch"])
                .map(|v| v.unwrap_or(Value::Null));
            outcome(n, result)
        }
        "replaceSnapshot" => {
            let service = ctx.service(step, &timeline, None);
            let result = service
                .replace_build_metadata_snapshot(str_of(&step["gameId"]), &step["snapshot"])
                .map(|v| v.unwrap_or(Value::Null));
            outcome(n, result)
        }
        "installedGamesAt" => {
            let root = fill_str(&step["root"], &ctx.root).unwrap();
            json!({"ok": installed_games_at(&root, &game_ids(&step["gameIds"]))})
        }
        "recoverLibraryPath" => {
            let logger = TimelineLogger {
                timeline: timeline.clone(),
                norm: n.clone(),
            };
            let current = fill_str(&step["current"], &ctx.root);
            let fallback = fill_str(&step["fallback"], &ctx.root);
            let result = recover_library_path(
                current.as_deref(),
                fallback.as_deref(),
                &game_ids(&step["gameIds"]),
                &logger,
            );
            json!({"outcome": outcome(n, Ok(json!(result))), "timeline": take(&timeline)})
        }
        "detectLegacy" => {
            let root = fill_str(&step["root"], &ctx.root).unwrap();
            let result = detect_legacy_layout(&root, &game_ids(&step["gameIds"]))
                .map(|list| Value::Array(list.iter().map(|game| game.to_json()).collect()));
            outcome(n, delivery(result))
        }
        "migrateLegacy" => {
            let root = fill_str(&step["root"], &ctx.root).unwrap();
            let result = migrate_legacy_game(&root, str_of(&step["gameId"])).map(|m| m.to_json());
            outcome(n, result)
        }
        "tree" => json!({"tree": library::tree_of(&ctx.root, n, "lib", &STATE_FILE)}),
        "utime" => {
            set_mtime_any(
                &path_in(&ctx.root, str_of(&step["file"])),
                step["mtime"].as_f64().unwrap(),
            );
            Value::Null
        }
        "write" => {
            let text = str_of(&step["utf8"]).replace("<pid>", &std::process::id().to_string());
            put_file(&ctx.root, str_of(&step["file"]), &json!({"utf8": text}));
            Value::Null
        }
        _ => {
            common_step(&ctx.root, step).unwrap_or_else(|| panic!("неизвестный шаг {}", step["op"]))
        }
    }
}

async fn run_scenario(spec: &Value, m: &mut Mismatches) {
    let (_temp, root) = temp_root();
    let norm = Arc::new(norm(&root, spec["stderr"] == true));
    library::setup(&root, &spec["setup"], &archive_file);
    let lib = root.join("lib");
    let ctx = Ctx {
        paths: resolve_library(&lib.to_string_lossy()).unwrap(),
        crash: root.join("crash"),
        lib,
        root: root.clone(),
        norm,
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
            expected,
            format!("{name}, шаг {index} ({})", step["op"]),
        );
    }
    scenario::open_up(&root);
}

/// Сценарии, которые 0.4.9 проходит иначе: файл игрока, который не переносится, прерывает
/// установку (`PRESERVE_FAILED`), а не пишется в журнал. Сверка — `tests/episodes.rs`
/// (`a_file_of_the_player_that_cannot_be_carried_stops_the_install`).
const CHANGED_IN_0_4_9: [&str; 1] =
    ["переустановка: сохраняемый путь другого типа и вне папки игры"];

/// Сценарии, имя которых начинается с `prefix`; пометка `unix` — только в Linux и не от root.
async fn run_group(prefix: &str) {
    let mut all = Mismatches::default();
    let mut ran = 0;
    for spec in items(&FIXTURE["scenarios"]) {
        if !str_of(&spec["name"]).starts_with(prefix)
            || CHANGED_IN_0_4_9.contains(&str_of(&spec["name"]))
        {
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

const GROUPS: [&str; 11] = [
    "установка",
    "переустановка",
    "отказ",
    "отмена",
    "блокировка",
    "одновременно",
    "обновление",
    "восстановление",
    "уборка",
    "installed.json",
    "библиотека",
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
    install_matches_0_4_7 => "установка",
    reinstall_matches_0_4_7 => "переустановка",
    failures_match_0_4_7 => "отказ",
    cancel_matches_0_4_7 => "отмена",
    lock_matches_0_4_7 => "блокировка",
    concurrent_install_matches_0_4_7 => "одновременно",
    full_update_and_faults_match_0_4_7 => "обновление",
    recovery_matches_0_4_7 => "восстановление",
    orphan_staging_matches_0_4_7 => "уборка",
    installed_json_matches_0_4_7 => "installed.json",
    library_matches_0_4_7 => "библиотека",
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
    assert_eq!(
        c["INSTALLED_SCHEMA_VERSION"],
        json!(install_service::INSTALLED_SCHEMA_VERSION)
    );
    assert_eq!(c["INSTALLED_FILE"], json!(install_service::INSTALLED_FILE));
    assert_eq!(
        c["UPDATE_SWAP_PHASES"],
        json!(install_service::UPDATE_SWAP_PHASES)
    );
    assert_eq!(c["UPDATE_FAULTS"], json!(install_service::UPDATE_FAULTS));
    assert_eq!(
        c["ORPHAN_STAGING_MIN_AGE_MS"],
        json!(install_service::ORPHAN_STAGING_MIN_AGE_MS)
    );
}

/// Не из 0.4.7 (там обещание не бросить): задача установки, которую бросили посреди работы,
/// отпускает `install.lock` и сервис, как `finally`; журнал и staging остаются, как после сбоя.
#[tokio::test]
async fn dropped_install_releases_lock_and_service() {
    let (_temp, root) = temp_root();
    std::fs::create_dir_all(root.join("lib/.hlsr/state")).unwrap();
    std::fs::create_dir_all(root.join("dl")).unwrap();
    let archive = root.join("dl/ghosting-1.0.0.7z");
    std::fs::copy(archive_file("ghosting-1.0.0.7z"), &archive).unwrap();
    let info = &FIXTURE["archives"]["ghosting-1.0.0.7z"];
    let paths = resolve_library(&root.join("lib").to_string_lossy()).unwrap();
    let service = InstallService::new(paths.clone(), InstallOptions::default());
    let request = InstallRequest {
        manifest: json!({
            "gameId": "ghosting-renewed",
            "version": "1.0.0",
            "expectedInstalledSize": 1000,
            "baseArchive": {"size": info["size"], "sha256": info["sha256"], "rootPrefix": "Ghosting Renewed"},
            "filesManifest": {"requiredFiles": [], "maxEntries": 100, "maxUncompressedSize": 10000000},
        }),
        manifest_sha256: None,
        archive_path: archive,
        files_manifest: None,
        published_at: None,
        update: None,
    };
    let lock = service.lock_path();
    {
        let mut task = Box::pin(service.install(request));
        // Один опрос: блокировка взята, дальше — чтение архива.
        let polled =
            std::future::poll_fn(|cx| std::task::Poll::Ready(task.as_mut().poll(cx))).await;
        assert!(
            polled.is_pending(),
            "установка не должна закончиться за один опрос"
        );
        assert!(service.is_busy());
        assert!(lock.exists());
    }
    assert!(!service.is_busy());
    assert!(!lock.exists());
    assert!(service.operation_path("ghosting-renewed").exists());
}

/// `GAME_RUNNING` на уровне сервиса: обновление не трогает папку запущенной игры. В Linux игра
/// находится по командной строке процесса (`/proc`), поэтому `hl2.exe` здесь — копия `sleep`.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn update_refuses_while_the_game_runs() {
    let spec = items(&FIXTURE["scenarios"])
        .iter()
        .find(|spec| str_of(&spec["name"]) == "обновление полной сборкой")
        .unwrap();
    let (_temp, root) = temp_root();
    library::setup(&root, &spec["setup"], &archive_file);
    let lib = root.join("lib");
    let ctx = Ctx {
        paths: resolve_library(&lib.to_string_lossy()).unwrap(),
        crash: root.join("crash"),
        lib: lib.clone(),
        root: root.clone(),
        norm: Arc::new(norm(&root, false)),
    };
    let game_dir = lib.join("Games/ghosting-renewed");
    let exe = game_dir.join("hl2.exe");
    std::fs::copy("/bin/sleep", &exe).unwrap();
    let mut game = std::process::Command::new(&exe).arg("30").spawn().unwrap();
    let seen = (0..50).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(20));
        hlsr_delivery::file_ops::is_game_running(&game_dir, &[]).is_some()
    });
    let result = run_step(&ctx, &items(&spec["steps"])[0]).await;
    game.kill().unwrap();
    game.wait().unwrap();
    assert!(seen, "процесс игры не найден");
    assert_eq!(
        result["outcome"]["error"],
        json!({
            "code": "GAME_RUNNING",
            "message": "Игра запущена (hl2.exe). Закройте её и повторите обновление.",
            "details": {"path": "hl2.exe"},
        }),
        "{result}"
    );
    assert!(
        std::fs::read(game_dir.join("installed.json"))
            .unwrap()
            .windows(5)
            .any(|w| w == b"1.0.9"),
        "установленная версия не тронута"
    );
}
