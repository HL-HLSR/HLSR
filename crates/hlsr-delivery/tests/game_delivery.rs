//! Сервис доставки против 0.4.7: фикстура `game-delivery.json` из
//! `tools/reference/delivery/game-delivery.js`.
//!
//! `GameDeliveryService` на настоящем 7za из `resources/7zip`, архивах
//! `tests/fixtures/archives/game-delivery` и локальном HTTP-сервере: статусы и главная кнопка от
//! «не установлено» до «установлено», проверка, ремонт и удаление, ошибка сети, игра запущена,
//! лицензия Steam и сборка из депо (фейки), пауза и отмена загрузки, установка до последней версии
//! патчем, обновление полной сборкой, экспорт и импорт `.hlsrpack`. Сравниваются итог каждого
//! шага, события `status` (без полей, зависящих от времени и кусков сети; подряд одинаковые —
//! одно), лента журнала и событий версий, деревья библиотеки и запросы к серверу.
//!
//! Нормализация — та же, что в скрипте: временная папка → `<root>`, SHA-256 манифестов игр (в них
//! адрес сервера) → `<manifest:набор/игра>`, pid и время в имени временного файла атомарной записи,
//! случайные имена папок экспорта и импорта, адрес сервера → `{main}`.

mod support;

use hlsr_core::json_text::stringify;
use hlsr_delivery::archive_service::ArchiveService;
use hlsr_delivery::environment::Environment;
use hlsr_delivery::errors::{Code, DeliveryError, Failure, RawError};
use hlsr_delivery::game_delivery_service::{
    GameDeliveryService, PackExtra, Provided, Registry, RepairCall, ServiceOptions,
};
use hlsr_delivery::game_pack;
use hlsr_delivery::manifest_service::ManifestService;
use hlsr_delivery::paths::LibraryPaths;
use hlsr_delivery::ports::{
    DepotProgress, DepotProgressFn, FakeFreeSpace, Logger, Notifier, ProcessProbe, SteamDepot,
    VersionEventInfo, process_not_running,
};
use hlsr_delivery::providers::BoxFuture;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use support::http::{RequestLog, fill, start_server, unfill};
use support::library::{
    Norm, Rule, Timeline, TimelineLogger, fill_value, outcome, push, take, tree_of,
};
use support::scenario::{path_in, temp_root};
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("game-delivery.json"));

/// Файлы, у которых в дереве записан текст.
static STATE_FILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\.json$|\.hlsr-pack\.json$").unwrap());

const GAME: &str = "ghosting-renewed";

/// Поля статуса, зависящие от времени и кусков сети: в событиях и итогах не сравниваются.
const VOLATILE: [&str; 6] = [
    "percent",
    "bytesDownloaded",
    "speedBps",
    "etaSeconds",
    "checked",
    "totalChecked",
];

/// Поля статуса из 0.4.9: эпизоды Ghosting (сверяет `tests/episodes.rs`) и `fileRepair`
/// (`tests/repair_per_file.rs`); в фикстуре 0.4.7 их нет.
const ADDED_IN_0_4_9: [&str; 3] = ["episodes", "episode", "fileRepair"];

/// Счёт проверки файлов 0.4.9 (`summarizeIntegrity`): cfg отдельно. В 0.4.7 их нет.
const INTEGRITY_ADDED_IN_0_4_9: [&str; 3] = ["configMissing", "configChanged", "stockMissing"];

/// Итог шага без полей счёта проверки 0.4.9 (объекты с `samples` и `damaged`).
fn strip_integrity(value: &Value) -> Value {
    match value {
        Value::Array(list) => Value::Array(list.iter().map(strip_integrity).collect()),
        Value::Object(map) => {
            let summary = map.contains_key("samples") && map.contains_key("damaged");
            Value::Object(
                map.iter()
                    .filter(|(key, _)| {
                        !summary || !INTEGRITY_ADDED_IN_0_4_9.contains(&key.as_str())
                    })
                    .map(|(key, item)| (key.clone(), strip_integrity(item)))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

fn skipped(key: &str) -> bool {
    VOLATILE.contains(&key) || ADDED_IN_0_4_9.contains(&key)
}

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

fn iso_now() -> String {
    hlsr_delivery::timing::iso_from_ms(now())
}

// ---------------------------------------------------------------- фейки служб

/// Процессы игр: запущены игры из списка.
#[derive(Default)]
struct FakeProcesses {
    running: Mutex<HashSet<String>>,
}

impl ProcessProbe for FakeProcesses {
    fn process_state(&self, game_id: &str) -> Value {
        if !self.is_running(game_id) {
            return process_not_running(game_id);
        }
        json!({
            "gameId": game_id,
            "state": "running",
            "running": true,
            "since": iso_now(),
            "startedAt": iso_now(),
            "exitCode": null,
            "lastExit": null,
        })
    }

    fn is_running(&self, game_id: &str) -> bool {
        self.running.lock().unwrap().contains(game_id)
    }

    fn any_running(&self) -> Option<String> {
        self.running.lock().unwrap().iter().min().cloned()
    }
}

/// Служба Steam: лицензии есть или нет; установка из депо — два шага хода, запись
/// `installed.json` или сбой.
struct FakeDepot {
    owns: bool,
    fail: bool,
    timeline: Timeline,
}

impl SteamDepot for FakeDepot {
    fn assert_owns<'a>(&'a self, app_ids: &'a [u32]) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(async move {
            push(
                &self.timeline,
                json!({"depot": "assertOwns", "appIds": app_ids}),
            );
            if !self.owns {
                return Err(DeliveryError::new(Code::SteamAppNotOwned, "Нет лицензии")
                    .with_details(json!({"appIds": app_ids}))
                    .into());
            }
            Ok(())
        })
    }

    fn install<'a>(
        &'a self,
        paths: &'a LibraryPaths,
        game_id: &'a str,
        on_progress: DepotProgressFn,
    ) -> BoxFuture<'a, Result<Value, Failure>> {
        Box::pin(async move {
            push(
                &self.timeline,
                json!({"depot": "install", "gameId": game_id}),
            );
            for (files, index) in [(1, 0), (4, 1)] {
                on_progress(&DepotProgress {
                    files,
                    expected_files: Some(4),
                    index,
                    total: 2,
                });
            }
            if self.fail {
                return Err(RawError::new("депо недоступно").into());
            }
            let install_path = paths.games.join(game_id);
            let installed = json!({
                "schemaVersion": 1,
                "gameId": game_id,
                "version": "5288",
                "installPath": install_path.to_string_lossy(),
                "installedAt": iso_now(),
                "state": "ready",
                "source": "steam-depot",
            });
            std::fs::create_dir_all(&install_path).unwrap();
            std::fs::write(install_path.join("hl2.exe"), "MZ").unwrap();
            std::fs::write(
                install_path.join("installed.json"),
                stringify(&installed, "  "),
            )
            .unwrap();
            Ok(installed)
        })
    }
}

/// События версий: вызовы в ленту.
struct FakeVersionEvents {
    timeline: Timeline,
}

fn info_json(info: &VersionEventInfo) -> Value {
    let mut out = Map::new();
    out.insert("hasReleaseNotes".to_owned(), json!(info.has_release_notes));
    if let Some(id) = &info.launcher_id {
        out.insert("launcherId".to_owned(), json!(id));
    }
    Value::Object(out)
}

impl FakeVersionEvents {
    fn record(&self, name: &str, args: Value) -> Result<(), String> {
        push(&self.timeline, json!({"versionEvent": name, "args": args}));
        Ok(())
    }
}

impl Notifier for FakeVersionEvents {
    fn notify(&self, _code: &str, _detail: &str) {}

    fn resolve_update_offers(
        &self,
        target: &str,
        installed_version: Option<&str>,
        game_id: &str,
    ) -> Result<(), String> {
        self.record(
            "resolveUpdateOffers",
            json!([target, installed_version, game_id]),
        )
    }

    fn game_update_available(
        &self,
        version: &str,
        current_version: Option<&str>,
        game_id: &str,
        info: &VersionEventInfo,
    ) -> Result<(), String> {
        self.record(
            "gameUpdateAvailable",
            json!([version, current_version, game_id, info_json(info)]),
        )
    }

    fn game_installed(
        &self,
        version: &str,
        game_id: &str,
        info: &VersionEventInfo,
    ) -> Result<(), String> {
        self.record("gameInstalled", json!([version, game_id, info_json(info)]))
    }

    fn game_updated(
        &self,
        from_version: Option<&str>,
        to_version: &str,
        game_id: &str,
        info: &VersionEventInfo,
    ) -> Result<(), String> {
        self.record(
            "gameUpdated",
            json!([from_version, to_version, game_id, info_json(info)]),
        )
    }
}

// ---------------------------------------------------------------- прогон

/// Статус без полей, зависящих от времени (события).
fn project(status: &Value) -> Value {
    let mut out = status.clone();
    if let Some(map) = out.as_object_mut() {
        map.retain(|key, _| !skipped(key));
    }
    out
}

/// Статусы внутри итога шага (объекты с `gameId`, `state` и `mainButton`) — без тех же полей.
fn strip_statuses(value: &Value) -> Value {
    match value {
        Value::Array(list) => Value::Array(list.iter().map(strip_statuses).collect()),
        Value::Object(map) => {
            let status = ["gameId", "state", "mainButton"]
                .iter()
                .all(|key| map.contains_key(*key));
            Value::Object(
                map.iter()
                    .filter(|(key, _)| !status || !skipped(key))
                    .map(|(key, item)| (key.clone(), strip_statuses(item)))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

/// Настройки сервиса шага: набор манифестов, запущенные игры, служба Steam.
#[derive(Clone)]
struct Config {
    manifests: String,
    running: Vec<String>,
    depot: Value,
}

struct Ctx {
    root: PathBuf,
    norm: Arc<Norm>,
    timeline: Timeline,
    events: Arc<Mutex<Vec<Value>>>,
    authority: BTreeMap<String, String>,
    files: bool,
}

impl Ctx {
    fn service(&self, config: &Config, processes: &Arc<FakeProcesses>) -> Arc<GameDeliveryService> {
        *processes.running.lock().unwrap() = config.running.iter().cloned().collect();
        let logger: Arc<dyn Logger> = Arc::new(TimelineLogger {
            timeline: self.timeline.clone(),
            norm: self.norm.clone(),
        });
        let manifests = fill(&FIXTURE["manifests"][&config.manifests], &self.authority);
        let all_files = FIXTURE["files"].as_object().unwrap();
        let files: Vec<(String, Value)> = all_files
            .iter()
            .filter(|(version, _)| self.files || version.as_str() == "1.0.0")
            .map(|(version, manifest)| (version.clone(), manifest.clone()))
            .collect();
        let depot: Option<Arc<dyn SteamDepot>> = (!config.depot.is_null()).then(|| {
            Arc::new(FakeDepot {
                owns: config.depot["owns"] == true,
                fail: config.depot["install"] == "fail",
                timeline: self.timeline.clone(),
            }) as Arc<dyn SteamDepot>
        });
        let clock_now = now();
        let service = GameDeliveryService::new(ServiceOptions {
            library_path: self.root.join("lib").to_string_lossy().into_owned(),
            logger: Some(logger.clone()),
            env: Some(Environment::new(
                [("GAME_DELIVERY_ALLOW_LOCAL_HTTP", "1")],
                false,
            )),
            launcher_version: Some("0.4.7".to_owned()),
            registry: Some(Registry {
                manifests: manifests
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(id, manifest)| (id.clone(), manifest.clone()))
                    .collect(),
                files: vec![(GAME.to_owned(), files)],
            }),
            depot,
            processes: Some(processes.clone()),
            channel_service: Provided::Off,
            origin_updates: Provided::Off,
            notifier: Some(Arc::new(FakeVersionEvents {
                timeline: self.timeline.clone(),
            })),
            archive_service: Some(ArchiveService::new(real_7za(), logger).with_env(pinned_env())),
            free_space: Some(Arc::new(FakeFreeSpace::default())),
            max_attempts: Some(1),
            backoff_base_ms: Some(1),
            update_check_interval_ms: Some(0),
            clock: Some(Arc::new(move || clock_now)),
            ..ServiceOptions::default()
        })
        .unwrap();
        let (events, norm) = (self.events.clone(), self.norm.clone());
        service.on_status(move |status| events.lock().unwrap().push(project(&norm.value(status))));
        service
    }

    fn path(&self, rel: &str) -> PathBuf {
        path_in(&self.root, rel)
    }
}

/// Нормализация сценария: SHA-256 манифестов, временный файл атомарной записи, случайные имена
/// папок экспорта и импорта, папка сборки из источника, размер архива в журнале.
fn norm(root: &Path, authority: &BTreeMap<String, String>) -> Norm {
    let mut rules = Vec::new();
    for (set, games) in FIXTURE["manifests"].as_object().unwrap() {
        for (game_id, manifest) in games.as_object().unwrap() {
            rules.push(Rule::Literal(
                ManifestService::hash(&fill(manifest, authority)),
                format!("<manifest:{set}/{game_id}>"),
            ));
        }
    }
    let pid = std::process::id();
    rules.extend([
        Rule::Pattern(
            Regex::new(r"-(export|import)-[0-9a-f]{8}").unwrap(),
            "-$1-<hex>".to_owned(),
        ),
        Rule::Literal(format!("-origin-{pid}"), "-origin-<pid>".to_owned()),
        Rule::Pattern(
            Regex::new(r"(\d+ файлов, )\d+ байт\)").unwrap(),
            "$1<size> байт)".to_owned(),
        ),
        Rule::Pattern(
            Regex::new(&format!(r"\.{pid}\.[0-9a-z]+\.tmp")).unwrap(),
            ".<pid>.<ms>.tmp".to_owned(),
        ),
    ]);
    Norm::new(root, rules, false)
}

/// Маршруты сервера: архив (`bodyArchive`) — байтами.
fn routes(spec: &Value, authority: &BTreeMap<String, String>) -> Map<String, Value> {
    let mut out = Map::new();
    for (key, list) in spec["routes"].as_object().into_iter().flatten() {
        let list: Vec<Value> = items(list)
            .iter()
            .map(|step| {
                let mut step = step.as_object().unwrap().clone();
                if let Some(name) = step.shift_remove("bodyArchive") {
                    let bytes = std::fs::read(archive_file(str_of(&name))).unwrap();
                    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                    step.insert("bodyHex".to_owned(), json!(hex));
                }
                Value::Object(step)
            })
            .collect();
        out.insert(key.clone(), fill(&Value::Array(list), authority));
    }
    out
}

/// Папка сценария той же длины (в записи JSON), что в скрипте: путь установки лежит в
/// `installed.json`, и от его длины зависят размеры, которые считает удаление и экспорт.
fn padded_root(dir: &Path) -> PathBuf {
    let base = format!("{}{}", dir.to_string_lossy(), std::path::MAIN_SEPARATOR);
    let quoted = serde_json::to_string(&base).unwrap();
    let length = quoted.len() - 2;
    let wanted = FIXTURE["rootLength"].as_u64().unwrap() as usize;
    assert!(length < wanted, "временная папка длиннее {wanted}: {base}");
    let root = PathBuf::from(format!("{base}{}", "p".repeat(wanted - length)));
    std::fs::create_dir(&root).unwrap();
    root
}

async fn run_scenario(spec: &Value) -> Mismatches {
    let name = str_of(&spec["name"]);
    let (_temp, temp) = temp_root();
    let root = padded_root(&temp);
    let log: RequestLog = Arc::default();
    let server = start_server("main", log.clone()).await;
    let authority: BTreeMap<String, String> =
        [("main".to_owned(), server.authority.clone())].into();
    *server.routes.lock().unwrap() = routes(spec, &authority);

    let ctx = Ctx {
        norm: Arc::new(norm(&root, &authority)),
        timeline: Arc::default(),
        events: Arc::default(),
        files: spec["files"] == true,
        authority,
        root: root.clone(),
    };
    let mut config = Config {
        manifests: str_of(&spec["manifests"]).to_owned(),
        running: support::scenario::strings(&spec["running"]),
        depot: spec["depot"].clone(),
    };
    let processes = Arc::new(FakeProcesses::default());
    let mut service = ctx.service(&config, &processes);
    let n = ctx.norm.clone();

    let mut mismatches = Mismatches::default();
    let expected = items(&spec["results"]);
    assert_eq!(
        items(&spec["steps"]).len(),
        items(&spec["results"]).len(),
        "шагов и результатов поровну"
    );
    for (index, step) in items(&spec["steps"]).iter().enumerate() {
        // Ремонта из полного архива в 0.4.9 нет (только с зеркала): с такого шага сценарий 0.4.7
        // дальше не сверяется. Ремонт по файлам проверяет `tests/repair_per_file.rs`.
        if step["do"] == "repair" && expected[index]["outcome"]["ok"].is_object() {
            break;
        }
        take(&ctx.timeline);
        ctx.events.lock().unwrap().clear();
        let game = step["game"].as_str().unwrap_or_default();
        let mut result = Map::new();
        let ok = |value: Value| -> Result<Value, Failure> { Ok(value) };
        let outcome_of = |result: Result<Value, Failure>| outcome(&n, result);
        match str_of(&step["do"]) {
            "service" => {
                if let Some(set) = step["manifests"].as_str() {
                    config.manifests = set.to_owned();
                }
                if step.get("running").is_some() {
                    config.running = support::scenario::strings(&step["running"]);
                }
                if step.get("depot").is_some() {
                    config.depot = step["depot"].clone();
                }
                service = ctx.service(&config, &processes);
            }
            "running" => {
                *processes.running.lock().unwrap() = support::scenario::strings(&step["games"])
                    .into_iter()
                    .collect();
            }
            "refresh" => {
                result.insert("outcome".into(), outcome_of(service.refresh(game)));
            }
            "statusOf" => {
                result.insert("outcome".into(), outcome_of(ok(service.status_of(game))));
            }
            "progressOf" => {
                let status = service.status_of(game);
                result.insert(
                    "outcome".into(),
                    outcome_of(ok(json!({
                        "percent": status["percent"],
                        "bytesDownloaded": status["bytesDownloaded"],
                        "totalBytes": status["totalBytes"],
                    }))),
                );
            }
            "install" => {
                result.insert("outcome".into(), outcome_of(service.install(game).await));
            }
            "installToLatest" => {
                result.insert(
                    "outcome".into(),
                    outcome_of(service.install_to_latest(game).await),
                );
            }
            "installPausedAt" => {
                let manifest = &FIXTURE["manifests"][&config.manifests][game];
                let filename = str_of(&manifest["baseArchive"]["filename"]);
                let part = root
                    .join("lib")
                    .join("Downloads")
                    .join(game)
                    .join(format!("{filename}.part"));
                let bytes = step["bytes"].as_u64().unwrap();
                let watch = async {
                    for _ in 0..2000 {
                        if std::fs::metadata(&part).is_ok_and(|meta| meta.len() >= bytes) {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    service.pause()
                };
                let (installed, paused) = tokio::join!(service.install(game), watch);
                result.insert("paused".into(), json!(paused));
                result.insert("outcome".into(), outcome_of(installed));
            }
            "cancel" => {
                result.insert(
                    "outcome".into(),
                    outcome_of(service.cancel(Some(game)).map(|done| json!(done))),
                );
            }
            "verify" => {
                let verified = service
                    .verify_files(game)
                    .await
                    .map(|integrity| GameDeliveryService::summarize_integrity(&integrity));
                result.insert("outcome".into(), outcome_of(verified));
            }
            "repair" => {
                let options = &step["options"];
                let call = RepairCall {
                    confirmed: options["confirmed"] == true,
                    skip_recheck: options["recheck"] == false,
                    files: None,
                };
                result.insert(
                    "outcome".into(),
                    outcome_of(service.repair(game, call).await),
                );
            }
            "uninstall" => {
                let confirmed = step["options"]["confirmed"] == true;
                result.insert(
                    "outcome".into(),
                    outcome_of(service.uninstall_game(game, confirmed).await),
                );
            }
            "checkForUpdates" => {
                let checked = service.check_for_updates(game, true, None).await.map(|out| {
                    json!({"status": out["status"], "plan": out["plan"], "update": out["update"]})
                });
                result.insert("outcome".into(), outcome_of(checked));
            }
            "planUpdate" => {
                result.insert(
                    "outcome".into(),
                    outcome_of(service.plan_update(game).map(|plan| plan.to_json())),
                );
            }
            "applyUpdate" => {
                result.insert(
                    "outcome".into(),
                    outcome_of(service.apply_update(game).await),
                );
            }
            "recoverFullUpdate" => {
                // Как в скрипте: сначала запись о незавершённом обновлении, затем восстановление.
                let pending = service.pending_full_update(game);
                let recovered = service
                    .recover_full_update(game)
                    .map(|recovery| json!({"pending": pending, "recovery": recovery.to_json()}));
                result.insert("outcome".into(), outcome_of(recovered));
            }
            "export" => {
                std::fs::create_dir_all(root.join("out")).unwrap();
                let extra = &step["extra"];
                let given = |key: &str| extra.get(key).cloned();
                let exported = service
                    .export_pack(
                        game,
                        &ctx.path(str_of(&step["target"])),
                        PackExtra {
                            launcher_id: given("launcherId"),
                            build: given("build"),
                            launcher_version: given("launcherVersion"),
                            settings: given("settings"),
                            // 0.4.7 не проверял файлы перед экспортом: шаг сверяется без
                            // проверки 0.4.9 (её сверяет game_pack_modified.rs).
                            allow_modified: true,
                        },
                    )
                    .await
                    .map(|mut out| {
                        if out["size"].is_number() {
                            out["size"] = json!("<size>");
                        }
                        out
                    });
                result.insert("outcome".into(), outcome_of(exported));
            }
            "packMetaText" => {
                let text = service
                    .archive_service()
                    .read_text(
                        &ctx.path(str_of(&step["path"])),
                        game_pack::META_FILE,
                        Some(4 * 1024 * 1024),
                    )
                    .await
                    .map(|text| json!(text))
                    .map_err(Failure::from);
                result.insert("outcome".into(), outcome_of(text));
            }
            "readPackMeta" => {
                let meta = service
                    .read_pack_meta(&ctx.path(str_of(&step["path"])))
                    .await
                    .map(|meta| meta.to_json());
                result.insert("outcome".into(), outcome_of(meta));
            }
            "importPack" => {
                let archive = ctx.path(str_of(&step["path"]));
                let imported = match service.read_pack_meta(&archive).await {
                    Ok(meta) => service.import_pack(&archive, &meta).await,
                    Err(error) => Err(error),
                };
                result.insert("outcome".into(), outcome_of(imported));
            }
            "write" => {
                let full = ctx.path(str_of(&step["file"]));
                std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                let text = match step.get("json") {
                    Some(value) => stringify(&fill_value(value, &root), "  "),
                    None => str_of(&step["utf8"]).to_owned(),
                };
                std::fs::write(&full, text).unwrap();
            }
            "remove" => {
                let _ = std::fs::remove_file(ctx.path(str_of(&step["file"])));
            }
            "tree" => {
                let tree = tree_of(&root, &n, str_of(&step["rel"]), &STATE_FILE);
                result.insert("tree".into(), support::marker_as_0_4_7(&tree));
            }
            "requests" => {
                let entries: Vec<Value> = std::mem::take(&mut *log.lock().unwrap())
                    .into_iter()
                    .map(|entry| {
                        json!({
                            "method": entry["method"],
                            "url": entry["url"],
                            "range": entry["headers"]["range"],
                        })
                    })
                    .collect();
                result.insert(
                    "requests".into(),
                    unfill(&Value::Array(entries), &ctx.authority),
                );
            }
            other => panic!("неизвестный шаг {other}"),
        }
        // Адрес сервера в итогах (источники в планах обновления) — {main}.
        for value in result.values_mut() {
            *value = unfill(value, &ctx.authority);
        }
        if let Some(outcome) = result.get_mut("outcome")
            && step["do"] != "progressOf"
        {
            *outcome = strip_statuses(outcome);
        }
        let mut collapsed: Vec<Value> = Vec::new();
        for event in ctx.events.lock().unwrap().drain(..) {
            if collapsed
                .last()
                .is_none_or(|last| stringify(last, "") != stringify(&event, ""))
            {
                collapsed.push(event);
            }
        }
        result.insert(
            "events".into(),
            unfill(&Value::Array(collapsed), &ctx.authority),
        );
        result.insert(
            "timeline".into(),
            support::integrity_log_as_0_4_7(unfill(&n.value(&take(&ctx.timeline)), &ctx.authority)),
        );
        let actual = strip_integrity(&Value::Object(result));
        let want = &expected[index];
        mismatches.check(
            &actual,
            want,
            format!("{name}, шаг {index} ({})", str_of(&step["do"])),
        );
        // Порядок ключей статусов — тоже видимое поведение.
        if actual == *want && stringify(&actual, "") != stringify(want, "") {
            mismatches.check_eq(
                stringify(&actual, " "),
                stringify(want, " "),
                format!("{name}, шаг {index}: порядок ключей"),
            );
        }
    }
    mismatches
}

fn scenario(name: &str) -> &'static Value {
    items(&FIXTURE["scenarios"])
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("нет сценария {name}"))
}

macro_rules! scenarios {
    ($($(#[$attr:meta])* $test:ident => $name:literal),* $(,)?) => {
        $(
            #[tokio::test]
            $(#[$attr])*
            async fn $test() {
                run_scenario(scenario($name)).await.finish($name);
            }
        )*

        /// Сценарии, у которых есть тест.
        const NAMES: &[&str] = &[$($name),*];
    };
}

scenarios! {
    #[cfg_attr(windows, ignore = "фикстура снята в Linux: размер installed.json (installPath с «\\\\» в JSON)")]
    install_verify_repair_uninstall_matches_0_4_7 => "установка, проверка, ремонт, удаление",
    network_error_matches_0_4_7 => "ошибка сети",
    game_running_matches_0_4_7 => "игра запущена",
    steam_license_matches_0_4_7 => "лицензия Steam",
    #[cfg_attr(windows, ignore = "фикстура снята в Linux: размер installed.json (installPath с «\\\\» в JSON)")]
    depot_build_matches_0_4_7 => "сборка из депо",
    pause_and_cancel_match_0_4_7 => "пауза и отмена загрузки",
    install_to_latest_by_patch_matches_0_4_7 => "установка до последней версии патчем",
    full_update_matches_0_4_7 => "обновление полной сборкой",
    patch_update_matches_0_4_7 => "обновление патчем",
    recovery_after_update_crash_matches_0_4_7 => "восстановление после сбоя обновления",
    #[cfg_attr(windows, ignore = "фикстура снята в Linux: размер installed.json, тексты ошибок p7zip 16.02")]
    pack_export_import_matches_0_4_7 => "экспорт и импорт .hlsrpack",
}

#[test]
fn every_scenario_has_a_test() {
    for spec in items(&FIXTURE["scenarios"]) {
        let name = str_of(&spec["name"]);
        assert!(NAMES.contains(&name), "сценарий «{name}» без теста");
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
