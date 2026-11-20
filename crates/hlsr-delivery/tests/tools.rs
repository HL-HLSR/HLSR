//! Инструменты против 0.4.7: фикстура `tools.json` из `tools/reference/delivery/tools.js`, и
//! самотесты `cancellation-selftest.js` (инструменты), `livesplit-splits-selftest.js` (запуск
//! LiveSplit со сплитами), `linux-selftest.js` (исправления перед запуском, окружение Proton).
//!
//! `ToolService` на локальных серверах и настоящем 7za из `resources/7zip` с архивами
//! `tests/fixtures/archives/tools`: состояние, установка с источников по очереди (sha256, размер,
//! ответы сервера), отказы манифеста, исполняемый файл вне папки, битый архив, отмена на загрузке,
//! под ограничением скорости и на распаковке («медленный» 7za — скрипт из фикстуры, только Linux),
//! вторая установка и удаление во время установки, удаление. Сравниваются итог, события прогресса,
//! строки журнала, журнал запросов и дерево библиотеки (у `.json` — текст: байты `installed.json`).
//!
//! Нормализация — та же, что в скрипте: временная папка → `<root>`, адреса серверов → `{имя}`,
//! текст ошибки 7-Zip у битого архива → `<stderr>`. Часы стоят.
//!
//! В конце — самотесты 0.4.9 (`ToolService.js`): зависимости «Демо» (FFmpeg, bxt-rs), `raw`,
//! `extract`, `extras`, `verifyFiles`.

mod support;

use hlsr_core::json_text::js_number;
use hlsr_delivery::archive_service::{ArchiveService, CreateOptions};
use hlsr_delivery::download_rate_limiter::DownloadRateLimiter;
use hlsr_delivery::errors::Failure;
use hlsr_delivery::paths::{LibraryPaths, resolve_library};
use hlsr_delivery::ports::{Logger, MemoryLogger, RunningTool, StopStatus, ToolLaunch, ToolRunner};
use hlsr_delivery::providers::BoxFuture;
use hlsr_delivery::tool_service::{self, LaunchOptions, ToolOptions, ToolService};
use hlsr_delivery::tools;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use support::http::{
    RequestLog, Server, dead_port, fill, start_server, unfill, without_default_accept,
};
use support::library::{self, Norm, Timeline, TimelineLogger, outcome, take};
use support::scenario::{self, common_step, path_in, temp_root};
use support::{Mismatches, fixture, items, opt_str, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("tools.json"));

static STATE_FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.json$").unwrap());

fn archive_file(name: &str) -> PathBuf {
    support::fixture_path("archives").join("tools").join(name)
}

fn real_7za() -> PathBuf {
    hlsr_core::paths::Resources::detect().sevenzip()
}

/// Сообщения 7za зависят от локали.
fn pinned_env() -> [(&'static str, &'static str); 3] {
    [("LC_ALL", "C.UTF-8"), ("LANG", "C.UTF-8"), ("TZ", "UTC")]
}

fn now() -> i64 {
    FIXTURE["now"].as_i64().unwrap()
}

// ---------------------------------------------------------------- чистые части

#[test]
fn constants_match_0_4_7() {
    let c = &FIXTURE["constants"];
    assert_eq!(c["LIVESPLIT_TOOL_ID"], tool_service::LIVESPLIT_TOOL_ID);
    assert_eq!(
        c["MAX_ARCHIVE_BYTES"],
        json!(tool_service::MAX_ARCHIVE_BYTES)
    );
    assert_eq!(c["STATE_FILE"], tool_service::STATE_FILE);
    assert_eq!(c["LIVESPLIT"], tools::LIVESPLIT);
}

#[test]
fn registry_matches_0_4_7() {
    let r = &FIXTURE["registry"];
    assert_eq!(r["ids"], json!(tools::ids()));
    for case in items(&r["manifest"]) {
        let key = str_of(&case[0]);
        assert_eq!(
            json!(tools::manifest_of(key)),
            case[1],
            "manifestOf {key:?}"
        );
    }
    for case in items(&r["presentation"]) {
        let key = str_of(&case[0]);
        assert_eq!(
            json!(tools::presentation_of(key)),
            case[1],
            "presentationOf {key:?}"
        );
    }
    // Ключи реестра службы в 0.4.9 — ещё и зависимости «Демо» (тест ниже).
    let registry = tools::registry();
    assert_eq!(
        Some(&registry["livesplit"]),
        tools::manifest_of("livesplit")
    );
}

#[test]
fn launch_environment_matches_0_4_7() {
    for case in items(&FIXTURE["environment"]) {
        let tool = str_of(&case[0]);
        let actual = tool_service::tool_launch_environment(tool, opt_str(&case[1]["DISPLAY"]))
            .map(|pairs| Value::Object(pairs.into_iter().map(|(k, v)| (k, json!(v))).collect()));
        assert_eq!(json!(actual), case[2], "{case}");
    }
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

// ---------------------------------------------------------------- сценарии

struct Ctx {
    root: PathBuf,
    paths: LibraryPaths,
    norm: Arc<Norm>,
    timeline: Timeline,
    limiter: Option<Arc<DownloadRateLimiter>>,
    slow_7za: Option<PathBuf>,
    servers: Vec<Server>,
    requests: RequestLog,
    map: BTreeMap<String, String>,
}

impl Ctx {
    fn logger(&self) -> Arc<dyn Logger> {
        Arc::new(TimelineLogger {
            timeline: self.timeline.clone(),
            norm: self.norm.clone(),
        })
    }

    fn archive(&self, choice: &str) -> Option<ArchiveService> {
        let bin = match choice {
            "none" => return None,
            "slow" => self
                .slow_7za
                .clone()
                .expect("«медленный» 7za — только unix"),
            _ => real_7za(),
        };
        Some(ArchiveService::new(bin, self.logger()).with_env(pinned_env()))
    }

    fn service(&self, registry: &Value, archive: &str) -> ToolService {
        let clock_now = now();
        ToolService::new(
            self.paths.clone(),
            ToolOptions {
                registry: Some(registry.as_object().unwrap().clone()),
                archive: self.archive(archive),
                logger: Some(self.logger()),
                rate_limiter: self.limiter.clone(),
                clock: Some(Arc::new(move || clock_now)),
                ..ToolOptions::default()
            },
        )
    }

    fn size_of(&self, rel: &str) -> i64 {
        std::fs::metadata(path_in(&self.root, rel)).map_or(-1, |meta| meta.len() as i64)
    }

    fn tree(&self) -> Value {
        library::tree_of(&self.root, &self.norm, "lib", &STATE_FILE)
    }

    fn requests_since(&self, mark: usize) -> Value {
        Value::Array(without_default_accept(
            &self.requests.lock().unwrap()[mark..],
        ))
    }
}

/// Тела маршрутов: `bodyArchive` → `bodyHex`, `appendHex` дописывает байты.
fn expand_routes(routes: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    for (route, list) in routes.as_object().unwrap() {
        let list = items(list)
            .iter()
            .map(|step| {
                let Some(name) = opt_str(&step["bodyArchive"]) else {
                    return step.clone();
                };
                let mut step = step.as_object().unwrap().clone();
                step.remove("bodyArchive");
                let tail = step
                    .remove("appendHex")
                    .map(|hex| str_of(&hex).to_owned())
                    .unwrap_or_default();
                let bytes = std::fs::read(archive_file(name)).unwrap();
                step.insert(
                    "bodyHex".to_owned(),
                    json!(support::zip::hex(&bytes) + &tail),
                );
                Value::Object(step)
            })
            .collect();
        out.insert(route.clone(), Value::Array(list));
    }
    out
}

async fn until(what: &str, predicate: impl Fn() -> bool) {
    let started = Instant::now();
    while !predicate() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "не дождались {what}"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// Действия во время установки (`actions` шага).
async fn run_actions(ctx: &Ctx, service: &ToolService, actions: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    let n = &ctx.norm;
    for action in actions {
        if let Some(rel) = opt_str(&action["waitFile"]) {
            let size = action["size"].as_i64();
            until(rel, || match size {
                None => ctx.size_of(rel) >= 0,
                Some(size) => ctx.size_of(rel) == size,
            })
            .await;
        } else if let Some(gate) = opt_str(&action["release"]) {
            for server in &ctx.servers {
                server.release(gate);
            }
        } else if let Some(ms) = action["sleep"].as_u64() {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        } else if let Some(tool) = opt_str(&action["cancel"]) {
            out.push(json!({"action": "cancel", "result": service.cancel(tool)}));
        } else if let Some(tool) = opt_str(&action["status"]) {
            out.push(json!({"action": "status", "result": service.status(tool)}));
        } else if let Some(tool) = opt_str(&action["install"]) {
            let result = service.install(tool, None).await;
            out.push(json!({"action": "install", "result": outcome(n, result)}));
        } else if let Some(tool) = opt_str(&action["remove"]) {
            let result = service.remove(tool);
            out.push(json!({"action": "remove", "result": outcome(n, result)}));
        } else if action["tree"] == true {
            out.push(json!({"action": "tree", "result": ctx.tree()}));
        } else {
            panic!("неизвестное действие {action}");
        }
    }
    out
}

async fn run_scenario(spec: &Value, m: &mut Mismatches) {
    let (_temp, root) = temp_root();
    let requests = RequestLog::default();
    let mut map = BTreeMap::new();
    map.insert(
        "dead".to_owned(),
        format!("127.0.0.1:{}", dead_port().await),
    );
    let mut servers = Vec::new();
    for name in spec["servers"].as_object().unwrap().keys() {
        let server = start_server(name, requests.clone()).await;
        map.insert(name.clone(), server.authority.clone());
        servers.push(server);
    }
    for server in &servers {
        let routes = expand_routes(&fill(&spec["servers"][&server.name], &map));
        server.routes.lock().unwrap().extend(routes);
    }

    let norm = Arc::new(Norm::new(
        &root,
        Vec::new(),
        str_of(&spec["name"]).contains("битый"),
    ));
    // "<root>" в тексте файла — временная папка, записанная внутри строки JSON.
    let setup: Vec<Value> = items(&spec["setup"])
        .iter()
        .map(|item| match opt_str(&item["utf8"]) {
            Some(text) => {
                let mut item = item.clone();
                item["utf8"] = json!(text.replace("<root>", norm.escaped_root()));
                item
            }
            None => item.clone(),
        })
        .collect();
    scenario::setup(&root, &Value::Array(setup));

    let slow_7za = cfg!(unix).then(|| {
        let file = root.join("slow-7za");
        std::fs::write(&file, str_of(&FIXTURE["slowScript"])).unwrap();
        scenario::chmod(&file, 0o755);
        file
    });
    let limiter = spec["service"]["limit"]
        .as_f64()
        .map(|limit| Arc::new(DownloadRateLimiter::new(limit)));
    let ctx = Ctx {
        paths: resolve_library(&root.join("lib").to_string_lossy()).unwrap(),
        norm,
        timeline: Arc::default(),
        limiter,
        slow_7za,
        servers,
        requests,
        map,
        root: root.clone(),
    };
    let registry = fill(&spec["registry"], &ctx.map);
    let mut archive = opt_str(&spec["service"]["archive"])
        .unwrap_or("real")
        .to_owned();

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
        take(&ctx.timeline);
        let mark = ctx.requests.lock().unwrap().len();
        let n = &ctx.norm;
        let tool = opt_str(&step["tool"]).unwrap_or_default();
        let actual = match str_of(&step["op"]) {
            "install" => {
                let mut registry = registry.clone();
                if !step["manifest"].is_null() {
                    registry[tool] = fill(&step["manifest"], &ctx.map);
                }
                let service = ctx.service(&registry, &archive);
                let progress: Mutex<Vec<Value>> = Mutex::default();
                let report = |percent: f64, received: u64| {
                    progress
                        .lock()
                        .unwrap()
                        .push(json!([js_number(percent), received]));
                };
                let actions = step["actions"].as_array().map_or(&[][..], Vec::as_slice);
                let (result, actions) = tokio::join!(
                    service.install(tool, Some(&report)),
                    run_actions(&ctx, &service, actions)
                );
                let mut out = json!({
                    "outcome": outcome(n, result),
                    "progress": std::mem::take(&mut *progress.lock().unwrap()),
                    "timeline": take(&ctx.timeline),
                    "requests": ctx.requests_since(mark),
                    "busy": service.busy_with(),
                });
                if !actions.is_empty() {
                    out["actions"] = Value::Array(actions);
                }
                out
            }
            "status" => json!({"status": ctx.service(&registry, &archive).status(tool)}),
            "readInstalled" => {
                json!({"ok": ctx.service(&registry, &archive).read_installed(tool)})
            }
            "uninstall" => {
                let result = ctx.service(&registry, &archive).remove(tool);
                json!({"outcome": outcome(n, result), "timeline": take(&ctx.timeline)})
            }
            "cancel" => json!({"cancel": ctx.service(&registry, &archive).cancel(tool)}),
            "setArchive" => {
                archive = str_of(&step["archive"]).to_owned();
                Value::Null
            }
            "tree" => json!({"tree": ctx.tree()}),
            _ => common_step(&root, step)
                .unwrap_or_else(|| panic!("{name}: неизвестный шаг {}", step["op"])),
        };
        let actual = unfill(&n.value(&actual), &ctx.map);
        m.check(
            &actual,
            expected,
            format!("{name}, шаг {index} ({})", step["op"]),
        );
    }
    scenario::open_up(&root);
}

/// Сценарии, имя которых начинается с `prefix`; `linuxOnly` и ссылки — только в unix.
async fn run_group(prefix: &str) {
    let mut all = Mismatches::default();
    let mut ran = 0;
    for spec in items(&FIXTURE["scenarios"]) {
        let name = str_of(&spec["name"]);
        if !name.starts_with(prefix) {
            continue;
        }
        let unix_only = spec["linuxOnly"] == true || name.contains("ссылка");
        if unix_only && cfg!(not(unix)) {
            continue;
        }
        ran += 1;
        run_scenario(spec, &mut all).await;
    }
    assert!(ran > 0, "нет сценариев «{prefix}»");
    all.finish(prefix);
}

const GROUPS: [&str; 5] = [
    "состояние",
    "установка",
    "отмена",
    "параллельно",
    "удаление",
];

#[tokio::test]
async fn status_matches_0_4_7() {
    run_group("состояние").await;
}

#[tokio::test]
async fn install_matches_0_4_7() {
    run_group("установка").await;
}

#[tokio::test]
async fn cancel_matches_0_4_7() {
    run_group("отмена").await;
}

#[tokio::test]
async fn concurrent_calls_match_0_4_7() {
    run_group("параллельно").await;
}

#[tokio::test]
async fn remove_matches_0_4_7() {
    run_group("удаление").await;
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

// ---------------------------------------------------------------- запуск (самотесты 0.4.7)

/// Запуск инструментов для самотестов: запоминает запросы, «процесс» живёт, пока его не закрыли.
#[derive(Default)]
struct FakeRunner {
    launched: Mutex<Vec<ToolLaunch>>,
    running: Mutex<Option<RunningTool>>,
}

impl ToolRunner for FakeRunner {
    fn running(&self, _tool_id: &str) -> Option<RunningTool> {
        self.running.lock().unwrap().clone()
    }

    fn launch<'a>(
        &'a self,
        request: &'a ToolLaunch,
    ) -> BoxFuture<'a, Result<Option<u32>, Failure>> {
        Box::pin(async move {
            self.launched.lock().unwrap().push(request.clone());
            *self.running.lock().unwrap() = Some(RunningTool {
                game_id: request.game_id.clone(),
                splits: request.splits.clone(),
                stopping: false,
            });
            Ok(Some(4242))
        })
    }

    fn stop<'a>(
        &'a self,
        _tool_id: &'a str,
        _force: bool,
    ) -> BoxFuture<'a, Result<StopStatus, Failure>> {
        Box::pin(async move {
            Ok(match self.running.lock().unwrap().take() {
                Some(_) => StopStatus::Stopped,
                None => StopStatus::NotRunning,
            })
        })
    }
}

const BASE_LAYOUT: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<Layout version=\"1.6.1\">\r\n  <Components>\r\n    <Component>\r\n      <Path>LiveSplit.Splits.dll</Path>\r\n    </Component>\r\n  </Components>\r\n</Layout>\r\n";
const SETTINGS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<Settings>\r\n  <RecentSplits />\r\n  <ActiveAutoSplitters />\r\n</Settings>\r\n";

/// Установленный LiveSplit в `Tools/livesplit` библиотеки `root/lib` и ресурсы с BunnySplit.
fn installed_livesplit(root: &Path) -> (LibraryPaths, PathBuf) {
    let paths = resolve_library(&root.join("lib").to_string_lossy()).unwrap();
    let tool = paths.tools.join("livesplit");
    for kind in ["chapters", "maps"] {
        let dir = tool.join("splits").join(kind);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{kind}.lsl")), BASE_LAYOUT).unwrap();
        std::fs::write(dir.join(format!("{kind}.lss")), "<Run />").unwrap();
    }
    std::fs::create_dir_all(tool.join("Components")).unwrap();
    std::fs::write(tool.join("settings.cfg"), SETTINGS).unwrap();
    std::fs::write(tool.join("LiveSplit.exe"), "").unwrap();
    std::fs::write(
        tool.join("installed.json"),
        json!({"toolId": "livesplit", "version": "1.0.0", "executable": "LiveSplit.exe"})
            .to_string(),
    )
    .unwrap();
    let resources = root.join("res");
    let components = resources.join(hlsr_delivery::livesplit_splits::COMPONENTS_DIRECTORY);
    std::fs::create_dir_all(&components).unwrap();
    std::fs::write(
        components.join(hlsr_delivery::livesplit_splits::BUNNYSPLIT),
        "dll",
    )
    .unwrap();
    (paths, resources)
}

fn livesplit_registry() -> Map<String, Value> {
    let mut registry = Map::new();
    registry.insert(
        "livesplit".to_owned(),
        json!({"version": "1.0.0", "executable": "LiveSplit.exe"}),
    );
    registry
}

/// `livesplit-splits-selftest.js`: «ToolService: LiveSplit стартует со сплитами игры, другие —
/// через перезапуск».
#[tokio::test]
async fn livesplit_starts_with_game_splits_and_restarts_for_others() {
    let root = tempfile::tempdir().unwrap();
    let (paths, resources) = installed_livesplit(root.path());
    let runner = Arc::new(FakeRunner::default());
    let service = ToolService::new(
        paths,
        ToolOptions {
            registry: Some(livesplit_registry()),
            platform: Some(hlsr_core::js::Platform::Win32),
            resources_path: Some(resources),
            runner: Some(runner.clone()),
            logger: Some(Arc::new(MemoryLogger::default())),
            ..ToolOptions::default()
        },
    );
    let launch = |game: &str, splits: &str, restart: bool| LaunchOptions {
        game_id: Some(game.to_owned()),
        splits: Some(splits.to_owned()),
        restart,
        server_port: None,
    };

    let started = service
        .launch("livesplit", &launch("hl2-2707", "scripts-maps", false))
        .await
        .unwrap();
    assert_eq!(started, json!({"status": "started", "pid": 4242}));
    assert_eq!(
        runner.launched.lock().unwrap()[0].args[..2],
        ["-s", "splits\\hlsr\\hl2-2707\\scripts-maps.lss"]
    );
    let info = service.running_info("livesplit").unwrap();
    assert_eq!(
        (info.game_id.as_deref(), info.splits.as_deref()),
        (Some("hl2-2707"), Some("scripts-maps"))
    );

    let same = service
        .launch("livesplit", &launch("hl2-2707", "scripts-maps", false))
        .await
        .unwrap();
    assert_eq!(same["status"], "already-running");
    assert_eq!(same["same"], true);
    let other = service
        .launch("livesplit", &launch("hl1-won", "scripted", false))
        .await
        .unwrap();
    assert_eq!(other["status"], "already-running");
    assert_eq!(other["same"], false);
    assert_eq!(runner.launched.lock().unwrap().len(), 1);

    service
        .launch("livesplit", &launch("hl1-won", "scripted", true))
        .await
        .unwrap();
    let launched = runner.launched.lock().unwrap();
    assert_eq!(launched.len(), 2);
    assert_eq!(launched[1].args[1], "splits\\hlsr\\hl1-won\\scripted.lss");
    let info = service.running_info("livesplit").unwrap();
    assert_eq!(
        (info.game_id.as_deref(), info.splits.as_deref()),
        (Some("hl1-won"), Some("scripted"))
    );
    assert!(service.status("livesplit")["running"] == true);
}

/// `linux-selftest.js`: «исправления применяются перед запуском, а их ошибка запуск не
/// останавливает»; на Linux — профиль шрифтов `livesplit` и окружение Proton с `DISPLAY`.
#[tokio::test]
async fn fixes_run_before_launch_and_their_failure_is_only_logged() {
    let root = tempfile::tempdir().unwrap();
    let (paths, _) = installed_livesplit(root.path());
    let tool = paths.tools.join("livesplit");
    let runner = Arc::new(FakeRunner::default());
    let logger = Arc::new(MemoryLogger::default());
    let calls: Arc<Mutex<Vec<(PathBuf, PathBuf)>>> = Arc::default();
    let fail = Arc::new(Mutex::new(false));
    let fixes: tool_service::LiveSplitFixesFn = {
        let (calls, fail) = (calls.clone(), fail.clone());
        Arc::new(move |dir: &Path, resources: &Path| {
            calls
                .lock()
                .unwrap()
                .push((dir.to_path_buf(), resources.to_path_buf()));
            if *fail.lock().unwrap() {
                return Err(hlsr_delivery::errors::RawError::new("нет ресурса"));
            }
            Ok(Vec::new())
        })
    };
    let service = ToolService::new(
        paths,
        ToolOptions {
            registry: Some(livesplit_registry()),
            platform: Some(hlsr_core::js::Platform::Linux),
            resources_path: Some(PathBuf::from("/test/resources")),
            host_environment: Some([("DISPLAY".to_owned(), ":0".to_owned())].into()),
            live_split_fixes: Some(fixes),
            runner: Some(runner.clone()),
            logger: Some(logger.clone()),
            ..ToolOptions::default()
        },
    );

    let started = service
        .launch("livesplit", &LaunchOptions::default())
        .await
        .unwrap();
    assert_eq!(started["status"], "started");
    assert_eq!(
        *calls.lock().unwrap(),
        [(tool.clone(), PathBuf::from("/test/resources"))]
    );
    {
        let launched = runner.launched.lock().unwrap();
        assert_eq!(launched[0].game_id.as_deref(), Some("ghosting-renewed"));
        assert_eq!(launched[0].font_profile.as_deref(), Some("livesplit"));
        assert_eq!(
            launched[0].env,
            Some(vec![(
                "PROTON_USE_X11_EXCLUSIVE".to_owned(),
                "LiveSplit.exe".to_owned()
            )])
        );
        assert_eq!(launched[0].executable, "LiveSplit.exe");
    }
    runner.running.lock().unwrap().take();

    *fail.lock().unwrap() = true;
    let started = service
        .launch("livesplit", &LaunchOptions::default())
        .await
        .unwrap();
    assert_eq!(started["status"], "started");
    assert_eq!(runner.launched.lock().unwrap().len(), 2);
    let lines = logger.take();
    assert!(
        lines.iter().any(|(level, line)| *level == "warn"
            && line.contains("исправления не применены: нет ресурса")),
        "{lines:?}"
    );
}

/// Запуск без установки и без исполняемого файла — коды 0.4.7.
#[tokio::test]
async fn launch_refuses_missing_install_and_executable() {
    let root = tempfile::tempdir().unwrap();
    let paths = resolve_library(&root.path().join("lib").to_string_lossy()).unwrap();
    let service = ToolService::new(
        paths.clone(),
        ToolOptions {
            registry: Some(livesplit_registry()),
            runner: Some(Arc::new(FakeRunner::default())),
            logger: Some(Arc::new(MemoryLogger::default())),
            ..ToolOptions::default()
        },
    );
    let code = |result: Result<Value, Failure>| match result.unwrap_err() {
        Failure::Delivery(error) => error.code.as_str().to_owned(),
        Failure::Raw(error) => panic!("{}", error.message),
    };
    let options = LaunchOptions::default();
    assert_eq!(
        code(service.launch("livesplit", &options).await),
        "NOT_INSTALLED"
    );
    let tool = paths.tools.join("livesplit");
    std::fs::create_dir_all(&tool).unwrap();
    std::fs::write(
        tool.join("installed.json"),
        json!({"toolId": "livesplit", "version": "1.0.0"}).to_string(),
    )
    .unwrap();
    assert_eq!(
        code(service.launch("livesplit", &options).await),
        "TOOL_EXECUTABLE_MISSING"
    );
}

/// `cancellation-selftest.js`: «cancelling extraction preserves the installed version and
/// supports retry» — брошенная установка тоже освобождает сервис и убирает staging.
#[tokio::test]
async fn dropped_install_releases_service() {
    let (_temp, root) = temp_root();
    let server = start_server("a", RequestLog::default()).await;
    let name = "livesplit-1.0.0.7z";
    let info = &FIXTURE["archives"][name];
    let bytes = std::fs::read(archive_file(name)).unwrap();
    server.routes.lock().unwrap().insert(
        format!("/{name}"),
        json!([{"bodyHex": support::zip::hex(&bytes), "gates": [{"at": 100, "name": "g"}]}]),
    );
    let mut registry = Map::new();
    registry.insert(
        "livesplit".to_owned(),
        json!({"version": "1.0.0", "executable": "LiveSplit.exe", "archive": {
            "filename": name, "size": info["size"], "sha256": info["sha256"],
            "providers": [{"id": "a", "url": format!("http://{}/{name}", server.authority)}],
        }}),
    );
    let paths = resolve_library(&root.join("lib").to_string_lossy()).unwrap();
    let service = ToolService::new(
        paths.clone(),
        ToolOptions {
            registry: Some(registry),
            archive: Some(ArchiveService::new(
                real_7za(),
                Arc::new(MemoryLogger::default()),
            )),
            logger: Some(Arc::new(MemoryLogger::default())),
            ..ToolOptions::default()
        },
    );
    {
        let part = paths.downloads.join(format!("{name}.part"));
        let install = service.install("livesplit", None);
        tokio::pin!(install);
        tokio::select! {
            _ = &mut install => panic!("установка не должна закончиться до ворот"),
            () = until("куска архива", || std::fs::metadata(&part).is_ok_and(|m| m.len() == 100)) => {}
        }
        assert_eq!(service.busy_with().as_deref(), Some("livesplit"));
    }
    assert_eq!(service.busy_with(), None);
    assert!(!service.cancel("livesplit"));
    assert!(!paths.downloads.join(name).exists());
    server.release("g");
}

// ---------------------------------------------------------------- 0.4.9: зависимости «Демо»

/// Реестр `ToolService` 0.4.9 — `REGISTRY` и `DEPENDENCIES`; каналы `tool-*` видят только панель.
#[test]
fn dependencies_are_in_the_service_registry_but_not_on_the_panel() {
    assert_eq!(
        tools::registry().keys().collect::<Vec<_>>(),
        ["livesplit", "ffmpeg", "bxt-rs"]
    );
    assert_eq!(tools::ids(), ["livesplit"]);
    for id in [tools::FFMPEG, tools::BXTRS] {
        assert_eq!(tools::manifest_of(id), None);
        assert_eq!(tools::dependency_of(id), tools::registry().get(id));
    }
    let ffmpeg = tools::dependency_of(tools::FFMPEG).unwrap();
    assert_eq!(ffmpeg["version"], "9.0.2");
    assert_eq!(ffmpeg["extract"].as_array().unwrap().len(), 2);
    let bxtrs = tools::dependency_of(tools::BXTRS).unwrap();
    assert_eq!(bxtrs["raw"], true);
    assert_eq!(bxtrs["extras"].as_array().unwrap().len(), 2);
}

/// Файлы, которые отдаёт сервер: путь → байты.
async fn serve_files(files: &[(&str, Vec<u8>)]) -> Server {
    let server = start_server("a", RequestLog::default()).await;
    let mut routes = server.routes.lock().unwrap();
    for (path, bytes) in files {
        routes.insert(
            format!("/{path}"),
            json!([{"status": 200, "bodyHex": support::zip::hex(bytes)}]),
        );
    }
    drop(routes);
    server
}

/// Описание файла для манифеста: размер, sha256 и адрес на сервере.
fn described(server: &Server, path: &str, bytes: &[u8]) -> Value {
    json!({
        "size": bytes.len(), "sha256": support::sha256_hex(bytes),
        "url": format!("http://{}/{path}", server.authority),
    })
}

fn deps_service(
    paths: &LibraryPaths,
    id: &str,
    manifest: Value,
) -> (ToolService, Arc<MemoryLogger>) {
    let mut registry = Map::new();
    registry.insert(id.to_owned(), manifest);
    let logger = Arc::new(MemoryLogger::default());
    let service = ToolService::new(
        paths.clone(),
        ToolOptions {
            registry: Some(registry),
            archive: Some(ArchiveService::new(real_7za(), logger.clone())),
            logger: Some(logger.clone()),
            ..ToolOptions::default()
        },
    );
    (service, logger)
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// bxt-rs: `raw` (сам файл, не архив) и `extras` — файлы рядом; архив не удаляется, а переносится.
#[tokio::test]
async fn raw_tool_with_extras_lands_next_to_its_license() {
    let root = tempfile::tempdir().unwrap();
    let paths = resolve_library(&root.path().join("lib").to_string_lossy()).unwrap();
    let (dll, copying) = (b"MZ bxt-rs".to_vec(), b"GPL".to_vec());
    let server = serve_files(&[("bxt_rs.dll", dll.clone()), ("COPYING", copying.clone())]).await;
    let archive = described(&server, "bxt_rs.dll", &dll);
    let mut extra = described(&server, "COPYING", &copying);
    extra["name"] = json!("COPYING");
    let manifest = json!({
        "version": "won-1", "executable": "bxt_rs.dll", "raw": true,
        "files": {"bxt_rs.dll": support::sha256_hex(&dll).to_uppercase()},
        "archive": {"filename": "bxt_rs.dll", "size": archive["size"], "sha256": archive["sha256"],
                    "providers": [{"id": "hlsr", "url": archive["url"]}]},
        "extras": [extra],
    });
    let (service, logger) = deps_service(&paths, "bxt-rs", manifest);

    let result = service.install("bxt-rs", None).await.unwrap();
    assert_eq!(result["status"], "installed");
    let target = paths.tools.join("bxt-rs");
    assert_eq!(names(&target), ["COPYING", "bxt_rs.dll", "installed.json"]);
    assert_eq!(std::fs::read(target.join("bxt_rs.dll")).unwrap(), dll);
    assert_eq!(std::fs::read(target.join("COPYING")).unwrap(), copying);
    assert!(!paths.downloads.join("bxt_rs.dll").exists());
    let log = logger.take();
    assert!(
        !log.iter().any(|(_, line)| line.contains("архив не удалён")),
        "{log:?}"
    );
}

/// `extras` без sha256 — `MANIFEST_INVALID`; старая версия не тронута.
#[tokio::test]
async fn extra_without_pinned_hash_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let paths = resolve_library(&root.path().join("lib").to_string_lossy()).unwrap();
    let dll = b"MZ".to_vec();
    let server = serve_files(&[("bxt_rs.dll", dll.clone())]).await;
    let archive = described(&server, "bxt_rs.dll", &dll);
    let manifest = json!({
        "version": "won-1", "executable": "bxt_rs.dll", "raw": true,
        "archive": {"filename": "bxt_rs.dll", "size": archive["size"], "sha256": archive["sha256"],
                    "providers": [{"id": "hlsr", "url": archive["url"]}]},
        "extras": [{"name": "COPYING", "size": 3, "url": archive["url"]}],
    });
    let (service, _) = deps_service(&paths, "bxt-rs", manifest);
    let Err(Failure::Delivery(error)) = service.install("bxt-rs", None).await else {
        panic!("ожидался отказ манифеста");
    };
    assert_eq!(
        (error.code.as_str(), error.message.as_str()),
        (
            "MANIFEST_INVALID",
            "У файла инструмента нет размера или sha256"
        )
    );
    assert!(!paths.tools.join("bxt-rs").exists());
}

/// FFmpeg: `extract` — только эти файлы архива и плоско, `files` — их закреплённые sha256.
#[tokio::test]
async fn extract_takes_only_listed_files_flat_and_verifies_them() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("src");
    let bin = source.join("ffmpeg-9/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(source.join("ffmpeg-9/doc")).unwrap();
    std::fs::write(bin.join("ffmpeg.exe"), "ffmpeg").unwrap();
    std::fs::write(bin.join("ffprobe.exe"), "ffprobe").unwrap();
    std::fs::write(source.join("ffmpeg-9/doc/readme.txt"), "doc").unwrap();
    let zip = root.path().join("ffmpeg-9.zip");
    ArchiveService::new(real_7za(), Arc::new(MemoryLogger::default()))
        .create_zip(
            &zip,
            CreateOptions {
                cwd: Some(source),
                files: vec!["ffmpeg-9".to_owned()],
                ..CreateOptions::default()
            },
        )
        .await
        .unwrap();
    let bytes = std::fs::read(&zip).unwrap();
    let server = serve_files(&[("ffmpeg-9.zip", bytes.clone())]).await;
    let archive = described(&server, "ffmpeg-9.zip", &bytes);
    let manifest = |ffprobe_sha: &str, extra_file: Option<&str>| {
        let mut files = json!({
            "ffmpeg.exe": support::sha256_hex(b"ffmpeg"),
            "ffprobe.exe": ffprobe_sha,
        });
        if let Some(name) = extra_file {
            files[name] = json!("00");
        }
        json!({
            "version": "9.0.2", "executable": "ffmpeg.exe",
            "extract": ["ffmpeg-9/bin/ffmpeg.exe", "ffmpeg-9/bin/ffprobe.exe"],
            "files": files,
            "archive": {"filename": "ffmpeg-9.zip", "size": archive["size"], "sha256": archive["sha256"],
                        "providers": [{"id": "a", "url": archive["url"]}]},
        })
    };
    let paths = resolve_library(&root.path().join("lib").to_string_lossy()).unwrap();
    let failure = |result: Result<Value, Failure>| match result {
        Err(Failure::Delivery(error)) => (error.code.as_str().to_owned(), error.message),
        other => panic!("ожидалась ошибка: {other:?}"),
    };

    let (service, _) = deps_service(&paths, "ffmpeg", manifest("00", None));
    assert_eq!(
        failure(service.install("ffmpeg", None).await),
        (
            "CHECKSUM_MISMATCH".to_owned(),
            "SHA-256 ffprobe.exe не совпал с закреплённым".to_owned()
        )
    );
    let probe = support::sha256_hex(b"ffprobe");
    let (service, _) = deps_service(&paths, "ffmpeg", manifest(&probe, Some("x264.dll")));
    assert_eq!(
        failure(service.install("ffmpeg", None).await),
        (
            "EXTRACT_FAILED".to_owned(),
            "В архиве нет x264.dll".to_owned()
        )
    );
    assert!(!paths.tools.join("ffmpeg").exists());

    let (service, _) = deps_service(&paths, "ffmpeg", manifest(&probe, None));
    let result = service.install("ffmpeg", None).await.unwrap();
    assert_eq!(result["version"], "9.0.2");
    let target = paths.tools.join("ffmpeg");
    assert_eq!(
        names(&target),
        ["ffmpeg.exe", "ffprobe.exe", "installed.json"]
    );
    assert!(!paths.downloads.join("ffmpeg-9.zip").exists());
}
