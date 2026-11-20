//! Служба обновления Windows против 0.4.7: трассы `update-manager-win.json` из
//! `tools/reference/update/update-manager-win.js` (`UpdateManager.js` в vm с поддельными часами;
//! electron-updater, шлюз, внешний апдейтер и `install-location` ведёт сценарий). Здесь те же
//! сценарии проигрываются на портах службы при остановленных часах tokio: снимки `updater-state`,
//! прогресс на панели задач, адрес ленты, уведомления, телеметрия, события версий, квитанция,
//! запуск апдейтера и установщика, выход, строки журнала, ответы каналов — по шагам.
//!
//! Порядок записей сравнивается внутри цепочки: основная цепочка службы, справка о свежести
//! ленты (`probeLatest`), уборка копий апдейтера (`setImmediate`) и итог экрана загрузки идут в
//! 0.4.7 разными цепочками промисов, их взаимный порядок — свойство очереди микрозадач V8.
//! Записи заглушки о повторных вызовах (`checkInProgress`, `downloadInProgress`) и об отмене токена
//! (`cancelDownload`) — внутренности electron-updater: их итог виден по отсутствию новых проверок и
//! загрузок и по состоянию `available` после отмены.
//!
//! Самотесты: `launcher-update-selftest.js` «installation on quit requires an explicit choice…»
//! (часть Windows — сценарии «install on quit…», «quit with error code…», «boot: …»),
//! `diagnostics-selftest.js` «updater errors… visible» (часть `classifyUpdateError` — «feed errors
//! are classified»).
//!
//! Сценарии QA-ленты (`HLSR_UPDATE_FEED`) требуют отладочной сборки или `dev-hooks`.

#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_core::json_text::js_number;
use hlsr_update::api::{UpdateApi, UpdateApiOptions, UpdateEvent};
use hlsr_update::boot_gate::BootDecision;
use hlsr_update::cancel::CancelToken;
use hlsr_update::channel::VerifyOutcome;
use hlsr_update::errors::LuCode;
use hlsr_update::gate::Outcome;
use hlsr_update::ports::{BoxFuture, Clock, Logger};
use hlsr_update::update_manager::{
    self, AppControl, DownloadFailure, DownloadProgressInfo, ExternalUpdater, GatePort,
    InstallLocationFix, InstallerDownload, InstallerRequest, LauncherVersionEvents, ManagerConfig,
    ManagerPorts, ProgressSink, QaGate, SetupRunner, UpdateFeed, UpdateInfo, UpdateManager,
    UpdateNotifier, UpdateTelemetry, UpdateWindow,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("update-manager-win.json"));

type Records = Arc<Mutex<Vec<Value>>>;

fn push(records: &Records, record: Value) {
    records.lock().unwrap().push(record);
}

/// Очередь ответов сценария: по одному на вызов, последний повторяется.
struct Queue {
    items: Vec<Value>,
    index: AtomicUsize,
}

impl Queue {
    fn new(value: &Value, default: Value) -> Queue {
        let items = value
            .as_array()
            .cloned()
            .filter(|items| !items.is_empty())
            .unwrap_or_else(|| vec![default]);
        Queue {
            items,
            index: AtomicUsize::new(0),
        }
    }

    fn next(&self) -> Value {
        let index = self.index.fetch_add(1, Ordering::SeqCst);
        self.items[index.min(self.items.len() - 1)].clone()
    }
}

/// `Date.now()` сценария: начало трассы плюс время tokio.
struct TokioClock {
    base: i64,
    start: tokio::time::Instant,
}

impl Clock for TokioClock {
    fn now_ms(&self) -> i64 {
        self.base + self.start.elapsed().as_millis() as i64
    }
}

struct Log(Records);

impl Logger for Log {
    fn info(&self, message: &str) {
        push(&self.0, json!(["log", "info", message]));
    }
    fn warn(&self, message: &str) {
        push(&self.0, json!(["log", "warn", message]));
    }
    fn error(&self, message: &str) {
        push(&self.0, json!(["log", "error", message]));
    }
}

struct Feed {
    records: Records,
    url: Mutex<String>,
    checks: Queue,
}

impl UpdateFeed for Feed {
    fn set_feed_url(&self, url: &str) {
        *self.url.lock().unwrap() = url.to_owned();
        push(&self.records, json!(["feed", url]));
    }

    fn check(&self) -> BoxFuture<'static, Result<UpdateInfo, String>> {
        let outcome = self.checks.next();
        push(
            &self.records,
            json!(["check", self.url.lock().unwrap().clone()]),
        );
        if outcome["hang"] == json!(true) {
            return Box::pin(std::future::pending());
        }
        let result = if let Some(error) = outcome["error"].as_str() {
            Err(error.to_owned())
        } else if let Some(available) = outcome.get("available") {
            Ok(UpdateInfo {
                version: str_of(&available["version"]).to_owned(),
                release_date: available.get("releaseDate").cloned(),
                release_notes: available.get("releaseNotes").cloned(),
                file: None,
            })
        } else {
            Ok(UpdateInfo {
                version: "0.4.7".to_owned(),
                release_date: None,
                release_notes: None,
                file: None,
            })
        };
        Box::pin(async move { result })
    }
}

fn outcome_of(value: &Value) -> Outcome {
    Outcome {
        ok: value["ok"].as_bool().unwrap_or(false),
        problems: value["problems"]
            .as_array()
            .map(|list| list.iter().map(|p| str_of(p).to_owned()).collect())
            .unwrap_or_default(),
        update_available: value["updateAvailable"].as_bool().unwrap_or(false),
        downgrade: false,
        unsupported: value["unsupported"].as_bool().unwrap_or(false),
        code: value["code"].as_str().map(lu_code),
        update_required: value["updateRequired"].as_bool().unwrap_or(false),
        manifest: value.get("manifest").cloned(),
        source_url: String::new(),
    }
}

fn lu_code(text: &str) -> LuCode {
    LuCode::ALL
        .into_iter()
        .find(|code| code.as_str() == text)
        .unwrap_or_else(|| panic!("код {text}"))
}

struct Gate {
    records: Records,
    evaluate: Arc<Queue>,
    probe: Arc<Queue>,
    verify: Arc<Queue>,
}

impl GatePort for Gate {
    fn evaluate<'a>(&'a self, version: &'a str) -> BoxFuture<'a, Result<Outcome, String>> {
        push(&self.records, json!(["evaluate", version]));
        let outcome = self.evaluate.next();
        Box::pin(async move {
            match outcome["throw"].as_str() {
                Some(message) => Err(message.to_owned()),
                None => Ok(outcome_of(&outcome)),
            }
        })
    }

    fn probe_latest(&self) -> BoxFuture<'_, Result<Option<Outcome>, String>> {
        push(&self.records, json!(["probe"]));
        let outcome = self.probe.next();
        Box::pin(async move {
            match &outcome {
                Value::Null => Ok(None),
                other => match other["throw"].as_str() {
                    Some(message) => Err(message.to_owned()),
                    None => Ok(Some(outcome_of(other))),
                },
            }
        })
    }

    fn verify_installer<'a>(
        &'a self,
        file: &'a Path,
        expected_version: &'a str,
    ) -> BoxFuture<'a, Result<VerifyOutcome, String>> {
        push(
            &self.records,
            json!(["verifyInstaller", file.to_string_lossy(), {"expectedVersion": expected_version}]),
        );
        let outcome = self.verify.next();
        Box::pin(async move {
            match outcome["throw"].as_str() {
                Some(message) => Err(message.to_owned()),
                None => Ok(VerifyOutcome {
                    ok: outcome["ok"].as_bool().unwrap_or(false),
                    problems: outcome["problems"]
                        .as_array()
                        .map(|list| list.iter().map(|p| str_of(p).to_owned()).collect())
                        .unwrap_or_default(),
                    code: outcome["code"].as_str().map(lu_code),
                }),
            }
        })
    }
}

struct Download {
    records: Records,
    downloads: Queue,
    file: PathBuf,
}

impl InstallerDownload for Download {
    fn download(
        &self,
        _request: InstallerRequest,
        cancel: CancelToken,
        on_progress: ProgressSink,
    ) -> BoxFuture<'static, Result<Option<PathBuf>, DownloadFailure>> {
        push(&self.records, json!(["download"]));
        let outcome = self.downloads.next();
        let file = self.file.clone();
        Box::pin(async move {
            // Как заглушка: первое событие — на следующем обороте, между шагами — тоже.
            tokio::task::yield_now().await;
            for step in outcome["steps"].as_array().into_iter().flatten() {
                let p = &step["progress"];
                on_progress(DownloadProgressInfo {
                    percent: p["percent"].as_f64().unwrap(),
                    transferred: p["transferred"].as_f64().unwrap(),
                    total: p["total"].as_f64().unwrap(),
                    bytes_per_second: p["bytesPerSecond"].as_f64().unwrap(),
                });
                tokio::task::yield_now().await;
            }
            if outcome["waitCancel"] == json!(true) && !cancel.is_cancelled() {
                cancel.cancelled().await;
            }
            if cancel.is_cancelled() {
                return Err(DownloadFailure::Cancelled);
            }
            if let Some(error) = outcome["error"].as_str() {
                return Err(DownloadFailure::Failed(error.to_owned()));
            }
            Ok((outcome["noFile"] != json!(true)).then_some(file))
        })
    }
}

struct Setup(Records);

impl SetupRunner for Setup {
    fn run(&self, _file: &Path, silent: bool, force_run: bool) -> Result<(), String> {
        push(
            &self.0,
            json!(["install", {"silent": silent, "forceRun": force_run}]),
        );
        Ok(())
    }
}

struct External {
    records: Records,
    available: bool,
    launches: Queue,
    cleanup_removed: usize,
    cleanup_throws: Option<String>,
}

impl ExternalUpdater for External {
    fn available(&self) -> bool {
        self.available
    }

    fn launch(&self, plan: Value) -> BoxFuture<'static, Result<u32, String>> {
        push(&self.records, json!(["external", plan]));
        let outcome = self.launches.next();
        Box::pin(async move {
            match outcome["throw"].as_str() {
                Some(message) => Err(message.to_owned()),
                None => Ok(outcome["pid"].as_u64().unwrap() as u32),
            }
        })
    }

    fn cleanup_old_stages(&self) -> Result<usize, String> {
        push(&self.records, json!(["cleanup"]));
        match &self.cleanup_throws {
            Some(message) => Err(message.clone()),
            None => Ok(self.cleanup_removed),
        }
    }
}

struct Location(Records, Option<String>);

impl InstallLocationFix for Location {
    fn ensure(&self, install_dir: &str) -> Result<(), String> {
        push(&self.0, json!(["installLocation", install_dir]));
        self.1.clone().map_or(Ok(()), Err)
    }
}

struct App(Records);

impl AppControl for App {
    fn quit(&self) {
        push(&self.0, json!(["quit"]));
    }
}

struct Window(Records);

impl UpdateWindow for Window {
    fn send_state(&self, payload: Value) {
        push(&self.0, json!(["state", payload]));
    }

    fn set_progress_bar(&self, value: f64) {
        push(&self.0, json!(["progressBar", js_number(value)]));
    }
}

struct Notifier(Records);

impl UpdateNotifier for Notifier {
    fn add(&self, level: &str, code: &str, detail: &str) -> Result<(), String> {
        push(
            &self.0,
            json!(["notify", {"level": level, "code": code, "detail": detail}]),
        );
        Ok(())
    }
}

struct Telemetry(Records, Option<String>);

impl UpdateTelemetry for Telemetry {
    fn available(&self, current: &str, next: Option<&str>) -> Result<(), String> {
        push(&self.0, json!(["telemetry", "available", current, next]));
        Ok(())
    }

    fn download_started(&self, current: &str, next: Option<&str>) -> Result<(), String> {
        push(
            &self.0,
            json!(["telemetry", "downloadStarted", current, next]),
        );
        Ok(())
    }

    fn download_completed(
        &self,
        current: &str,
        next: &str,
        bytes: Option<f64>,
        duration_ms: Option<i64>,
    ) -> Result<(), String> {
        push(
            &self.0,
            json!(["telemetry", "downloadCompleted", current, next, {
                "bytes": bytes.map(js_number),
                "durationMs": duration_ms,
            }]),
        );
        Ok(())
    }

    fn failed(&self, current: &str, next: Option<&str>, code: Option<&str>) -> Result<(), String> {
        push(&self.0, json!(["telemetry", "failed", current, next, code]));
        self.1.clone().map_or(Ok(()), Err)
    }
}

struct Versions(Records, Option<String>);

impl LauncherVersionEvents for Versions {
    fn launcher_update_available(
        &self,
        version: &str,
        current: &str,
        has_release_notes: bool,
    ) -> Result<(), String> {
        push(
            &self.0,
            json!(["versionAvailable", version, current, {"hasReleaseNotes": has_release_notes}]),
        );
        Ok(())
    }

    fn write_pending_receipt(&self, from: &str, to: &str) -> Result<(), String> {
        push(&self.0, json!(["receipt", from, to]));
        self.1.clone().map_or(Ok(()), Err)
    }
}

/// Дать доработать всем задачам, которые уже могут идти.
async fn flush() {
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
}

/// Перевести часы шагами не больше секунды, давая задачам отработать после каждого.
async fn advance(ms: u64) {
    let mut left = ms;
    while left > 0 {
        let step = left.min(1000);
        tokio::time::advance(Duration::from_millis(step)).await;
        flush().await;
        left -= step;
    }
}

/// Сценарий на службе; `via_api` — каналы и снимки идут через `UpdateApi` (окно службы — фасад,
/// окно ОС — запись сценария).
async fn play(scenario: &Value, via_api: bool) -> Vec<Value> {
    let records: Records = Arc::default();
    let start = FIXTURE["start"].as_i64().unwrap();
    let env = &scenario["env"];

    let evaluate = Arc::new(Queue::new(
        &scenario["evaluate"],
        json!({"ok": true, "problems": [], "updateAvailable": true, "manifest": FIXTURE["manifest"]}),
    ));
    let probe = Arc::new(Queue::new(&scenario["probe"], Value::Null));
    let verify = Arc::new(Queue::new(
        &scenario["verify"],
        json!({"ok": true, "problems": []}),
    ));
    let make_gate = {
        let (records, evaluate, probe, verify) = (
            records.clone(),
            evaluate.clone(),
            probe.clone(),
            verify.clone(),
        );
        Box::new(move |qa: Option<&QaGate>| -> Arc<dyn GatePort> {
            push(
                &records,
                json!(["gate", {
                    "currentVersion": "0.4.7",
                    "channelUrls": qa.map(|qa| qa.channel_urls.clone()),
                    "latestYmlUrls": qa.map(|qa| qa.latest_yml_urls.clone()),
                    "qaHttp": qa.is_some(),
                    "publicKeys": qa.and_then(|qa| qa.public_keys.clone()),
                }]),
            );
            Arc::new(Gate {
                records: records.clone(),
                evaluate: evaluate.clone(),
                probe: probe.clone(),
                verify: verify.clone(),
            })
        })
    };

    let ports = ManagerPorts {
        feed: Arc::new(Feed {
            records: records.clone(),
            url: Mutex::new(String::new()),
            checks: Queue::new(&scenario["checks"], json!({"notAvailable": true})),
        }),
        make_gate,
        download: Arc::new(Download {
            records: records.clone(),
            downloads: Queue::new(&scenario["downloads"], json!({"steps": [], "done": true})),
            file: PathBuf::from(str_of(&FIXTURE["downloadedFile"])),
        }),
        setup: Arc::new(Setup(records.clone())),
        external: Arc::new(External {
            records: records.clone(),
            available: scenario["external"] != json!(false),
            launches: Queue::new(&scenario["launch"], json!({"pid": 4321})),
            cleanup_removed: scenario["cleanupRemoved"].as_u64().unwrap_or(0) as usize,
            cleanup_throws: scenario["cleanupThrows"].as_str().map(str::to_owned),
        }),
        install_location: Arc::new(Location(
            records.clone(),
            scenario["installLocationThrows"]
                .as_str()
                .map(str::to_owned),
        )),
        app: Arc::new(App(records.clone())),
        logger: Arc::new(Log(records.clone())),
        clock: Arc::new(TokioClock {
            base: start,
            start: tokio::time::Instant::now(),
        }),
        random: Arc::new(|| 0.5),
        window: (!via_api).then(|| Arc::new(Window(records.clone())) as Arc<dyn UpdateWindow>),
        notifier: None,
        telemetry: None,
        version_events: None,
    };
    let config = ManagerConfig {
        current_version: "0.4.7".to_owned(),
        packaged: scenario["packaged"] != json!(false),
        production: true,
        build_channel: env["HLSR_BUILD_CHANNEL"]
            .as_str()
            .unwrap_or("development")
            .to_owned(),
        update_feed: env["HLSR_UPDATE_FEED"].as_str().map(str::to_owned),
        channel_key: env["HLSR_UPDATE_CHANNEL_KEY"].as_str().map(str::to_owned),
        exe_path: str_of(&FIXTURE["exe"]).to_owned(),
        user_data_dir: str_of(&FIXTURE["userData"]).to_owned(),
    };
    let manager = UpdateManager::new(config, ports);
    let game = Arc::new(AtomicBool::new(scenario["gameRunning"] == json!(true)));
    {
        let game = game.clone();
        manager.set_game_running_check(move || game.load(Ordering::SeqCst));
    }
    manager.set_notifications(Arc::new(Notifier(records.clone())));
    manager.set_version_events(Arc::new(Versions(
        records.clone(),
        scenario["receiptThrows"].as_str().map(str::to_owned),
    )));
    manager.set_update_telemetry(Arc::new(Telemetry(
        records.clone(),
        scenario["telemetryThrows"].as_str().map(str::to_owned),
    )));
    let api = via_api.then(|| {
        UpdateApi::new(UpdateApiOptions {
            updates: Arc::new(manager.clone()),
            window: Some(Arc::new(Window(records.clone()))),
            notifier: None,
            diagnostics: None,
            logger: Arc::new(Log(records.clone())),
        })
    });
    let mut api_events = api.as_ref().map(|api| api.subscribe());
    match &api {
        Some(api) => api.initialize(),
        None => manager.initialize(),
    }
    flush().await;

    for op in items(&scenario["ops"]) {
        match str_of(&op["op"]) {
            "invoke" if api.is_some() => {
                let api = api.clone().unwrap();
                let channel = str_of(&op["channel"]).to_owned();
                let args: Vec<Value> = if op["arg"].is_null() {
                    Vec::new()
                } else {
                    vec![op["arg"].clone()]
                };
                if channel == "boot-gate-run" {
                    let records = records.clone();
                    tokio::spawn(async move {
                        let reply = api.call(&channel, &args).await.unwrap().unwrap();
                        push(&records, json!(["return", channel, reply]));
                    });
                } else {
                    let reply = api.call(&channel, &args).await;
                    let reply = reply.unwrap_or_else(|| panic!("канал {channel}")).unwrap();
                    push(&records, json!(["return", channel, reply]));
                }
            }
            "invoke" => {
                let channel = str_of(&op["channel"]).to_owned();
                let ret = |value: Value| push(&records, json!(["return", channel, value]));
                match channel.as_str() {
                    "updater-check" => ret(json!(manager.check())),
                    "updater-cancel" => ret(json!(manager.cancel())),
                    "updater-install-now" => ret(json!(manager.install_now())),
                    "updater-install-on-quit" => ret(json!(manager.install_on_quit())),
                    "updater-set-language" => {
                        ret(json!(
                            manager.set_language(op["arg"].as_str().unwrap_or(""))
                        ));
                    }
                    "updater-get-state" => ret(manager.get_state()),
                    "updater-download" => {
                        let reply = manager.download().await;
                        ret(reply.to_json());
                    }
                    "boot-gate-run" => {
                        let retry = op["arg"]["retry"] == json!(true);
                        let (manager, records) = (manager.clone(), records.clone());
                        tokio::spawn(async move {
                            let decision: BootDecision = manager.boot_gate_run(retry).await;
                            push(
                                &records,
                                json!(["return", "boot-gate-run", decision.to_json()]),
                            );
                        });
                    }
                    other => panic!("канал {other}"),
                }
            }
            "startCheckingInterval" => match &api {
                Some(api) => api.start_checking_interval(),
                None => manager.start_checking_interval(),
            },
            "checkOnFocus" => match &api {
                Some(api) => api.check_on_focus(),
                None => manager.check_on_focus(),
            },
            "advance" => advance(op["ms"].as_u64().unwrap()).await,
            "game" => game.store(op["running"] == json!(true), Ordering::SeqCst),
            "quit" => {
                let code = op["exitCode"].as_i64().unwrap_or(0) as i32;
                match &api {
                    Some(api) => {
                        api.on_app_quit(code);
                        api.dispose();
                    }
                    None => {
                        manager.on_app_quit(code);
                        manager.dispose();
                    }
                }
            }
            other => panic!("шаг {other}"),
        }
        flush().await;
        push(&records, json!(["step"]));
    }
    manager.dispose();
    let records = records.lock().unwrap().clone();
    // Каждый снимок окна ушёл и событием `updater-state` фасада, в том же порядке.
    if let Some(events) = &mut api_events {
        let mut sent = Vec::new();
        while let Ok(UpdateEvent { channel, payload }) = events.try_recv() {
            assert_eq!(channel, "updater-state");
            sent.push(payload);
        }
        let states: Vec<Value> = records
            .iter()
            .filter(|record| record[0] == "state")
            .map(|record| record[1].clone())
            .collect();
        assert_eq!(sent, states, "события updater-state = снимки окна");
    }
    records
}

/// Цепочка записи (см. заголовок): основная, справка о ленте, уборка, итог экрана загрузки.
fn flow_of(records: &[Value], index: usize) -> Option<&'static str> {
    let record = &records[index];
    let kind = record[0].as_str().unwrap_or("");
    let text = record[2].as_str().unwrap_or("");
    match kind {
        "checkInProgress" | "downloadInProgress" | "cancelDownload" => None,
        "probe" => Some("probe"),
        "cleanup" => Some("cleanup"),
        "return" if record[1] == "boot-gate-run" => Some("boot"),
        "log"
            if text.starts_with("Feed says no update")
                || text.starts_with("Feed freshness probe failed") =>
        {
            Some("probe")
        }
        "log"
            if text.starts_with("[updater] убрано старых копий")
                || text.starts_with("[updater] уборка TEMP") =>
        {
            Some("cleanup")
        }
        "log" if text.starts_with("[boot] decision:") => Some("boot"),
        "notify" if record[1]["code"] == "LU_FEED_STALE" => Some("probe"),
        "notify"
            if records.get(index + 1).is_some_and(|next| {
                next[0] == "log"
                    && next[2]
                        .as_str()
                        .is_some_and(|text| text.starts_with("[boot] decision:"))
            }) =>
        {
            Some("boot")
        }
        _ => Some("main"),
    }
}

/// Трасса по шагам и цепочкам.
fn by_flow(records: &[Value]) -> Value {
    let mut steps = Vec::new();
    let mut current: serde_json::Map<String, Value> = serde_json::Map::new();
    for index in 0..records.len() {
        if records[index][0] == "step" {
            steps.push(Value::Object(std::mem::take(&mut current)));
            continue;
        }
        if let Some(flow) = flow_of(records, index) {
            current
                .entry(flow)
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .unwrap()
                .push(records[index].clone());
        }
    }
    steps.push(Value::Object(current));
    Value::Array(steps)
}

#[test]
fn update_manager_traces_match_node() {
    traces_match_node(false);
}

/// Те же трассы, но каналы окна — `UpdateApi::call`, снимки — его события (задача 9).
#[test]
fn update_manager_traces_through_update_api() {
    traces_match_node(true);
}

fn traces_match_node(via_api: bool) {
    let scenarios = items(&FIXTURE["scenarios"]);
    assert!(
        scenarios.len() >= 35,
        "в фикстуре {} сценариев",
        scenarios.len()
    );
    let mut mismatches = Mismatches::default();
    for scenario in scenarios {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap();
        let actual = runtime.block_on(play(scenario, via_api));
        let expected: Vec<Value> = items(&scenario["records"]).clone();
        mismatches.check(
            &by_flow(&actual),
            &by_flow(&expected),
            str_of(&scenario["id"]),
        );
    }
    mismatches.finish("UpdateManager");
}

#[test]
fn update_errors_are_classified_like_node() {
    for (text, class) in [
        ("HttpError: 429 Too Many Requests", "RATE_LIMIT"),
        ("rate limit exceeded", "RATE_LIMIT"),
        ("getaddrinfo ENOTFOUND hlsr.pro", "NETWORK"),
        ("net::ERR_INTERNET_DISCONNECTED", "NETWORK"),
        ("HttpError: 404 Not Found", "FEED"),
        ("status 503", "FEED"),
        ("ENOSPC: no space left", "NO_SPACE"),
        ("EPERM: operation not permitted", "ACCESS"),
        ("sha512 checksum mismatch", "VERIFY"),
        ("something odd", "UNKNOWN"),
        ("1429 items", "UNKNOWN"),
    ] {
        assert_eq!(update_manager::classify_update_error(text), class, "{text}");
    }
    assert_eq!(
        update_manager::sanitize_release_notes(Some(&json!("<p>a&nbsp;b</p>\n\n\n\nc"))),
        "a b \n\nc"
    );
    assert_eq!(update_manager::sanitize_release_notes(None), "");
    assert!(update_manager::is_loopback_feed(
        "http://127.0.0.1:8080/feed/"
    ));
    assert!(update_manager::is_loopback_feed("https://localhost"));
    assert!(!update_manager::is_loopback_feed(
        "http://localhost.example.com/"
    ));
    assert!(!update_manager::is_loopback_feed("http://127.0.0.1:/x"));
}
