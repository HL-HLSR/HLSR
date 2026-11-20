//! Служба обновления Linux против 0.4.7: трассы `update-manager-linux.json` из
//! `tools/reference/update/update-manager-linux.js` (`linux/UpdateManager.js` в vm с поддельными часами;
//! шлюз, загрузку AppImage, извлечение payload, манифест чанков и `ArchiveInstall` ведёт сценарий). Здесь
//! те же сценарии проигрываются на портах службы при остановленных часах tokio: снимки `updater-state`,
//! вызовы портов, телеметрия, события версий, квитанция, перезапуск, выход, `preventDefault`, строки
//! журнала, содержимое `.downloads` — по шагам и по порядку внутри шага. Ответы каналов сверяются по шагам
//! отдельным списком: синхронную часть промиса 0.4.7 (запрос загрузки, проверка экрана загрузки) служба
//! Rust выполняет в задаче tokio, после ответа канала.
//!
//! Самотесты `launcher-update-selftest.js`: «manager installs only the launcher version the signed
//! channel names», «manager: cancelling a launcher download leaves an available update and supports
//! retry», «installation on quit requires an explicit choice; mandatory boot errors block» (часть Linux),
//! «manager blocks installation during a game and relaunches the installed binary» (на портах),
//! «Linux: перезапуск после обновления сохраняет параметры окна».

mod support;

use hlsr_update::api::{UpdateApi, UpdateApiOptions};
use hlsr_update::boot_gate::BootDecision;
use hlsr_update::cancel::CancelToken;
use hlsr_update::chunks::Manifest;
use hlsr_update::errors::LuCode;
use hlsr_update::gate::Outcome;
use hlsr_update::linux::embedded_payload::Payload;
use hlsr_update::linux::update_manager::{
    self, AppImageDownloader, ChunkManifests, ChunkPrepare, ChunkResult, LinuxGate,
    LinuxManagerConfig, LinuxManagerPorts, LinuxUpdateManager, PayloadExtractor, ProgressSink,
    Relauncher, VersionInstaller,
};
use hlsr_update::ports::{BoxFuture, Clock, Logger};
use hlsr_update::update_manager::{
    AppControl, LauncherVersionEvents, UpdateTelemetry, UpdateWindow,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("update-manager-linux.json"));

#[derive(Clone)]
struct Records {
    list: Arc<Mutex<Vec<Value>>>,
    root: PathBuf,
}

impl Records {
    fn mask(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => {
                let root = self.root.to_string_lossy();
                if !text.contains(root.as_ref()) {
                    return value.clone();
                }
                let replaced = text.replace(root.as_ref(), "{root}").replace('\\', "/");
                Value::String(rand_tail(&replaced))
            }
            Value::Array(list) => Value::Array(list.iter().map(|item| self.mask(item)).collect()),
            other => other.clone(),
        }
    }

    fn push(&self, record: Value) {
        let record = self.mask(&record);
        self.list.lock().unwrap().push(record);
    }
}

/// `update-XXXXXX` → `update-{rand}`.
fn rand_tail(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(index) = rest.find("update-") {
        let after = &rest[index + 7..];
        out.push_str(&rest[..index]);
        if after.len() >= 6 && after[..6].chars().all(|c| c.is_ascii_alphanumeric()) {
            out.push_str("update-{rand}");
            rest = &after[6..];
        } else {
            out.push_str("update-");
            rest = after;
        }
    }
    out.push_str(rest);
    out
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

/// `setImmediate` — один оборот.
async fn tick() {
    tokio::task::yield_now().await;
}

async fn until_abort(cancel: &CancelToken) -> String {
    cancel.cancelled().await;
    update_manager::ABORTED.to_owned()
}

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
        self.0.push(json!(["log", "info", message]));
    }
    fn warn(&self, message: &str) {
        self.0.push(json!(["log", "warn", message]));
    }
    fn error(&self, message: &str) {
        self.0.push(json!(["log", "error", message]));
    }
}

fn outcome_of(value: &Value) -> Outcome {
    Outcome {
        ok: value["ok"].as_bool().unwrap_or(false),
        problems: value["problems"]
            .as_array()
            .map(|list| list.iter().map(|item| str_of(item).to_owned()).collect())
            .unwrap_or_default(),
        update_available: value["updateAvailable"].as_bool().unwrap_or(false),
        downgrade: value["downgrade"].as_bool().unwrap_or(false),
        unsupported: value["unsupported"].as_bool().unwrap_or(false),
        code: value["code"].as_str().and_then(|code| {
            LuCode::ALL
                .into_iter()
                .find(|candidate| candidate.as_str() == code)
        }),
        update_required: value["updateRequired"].as_bool().unwrap_or(false),
        manifest: Some(value["manifest"].clone()).filter(|manifest| !manifest.is_null()),
        source_url: String::new(),
    }
}

struct Gate {
    records: Records,
    queue: Queue,
}

impl LinuxGate for Gate {
    fn evaluate(&self) -> BoxFuture<'static, Result<Outcome, String>> {
        self.records.push(json!(["evaluate"]));
        let outcome = self.queue.next();
        Box::pin(async move {
            if outcome["hang"].as_bool() == Some(true) {
                std::future::pending::<()>().await;
            }
            tick().await;
            if let Some(error) = outcome["throw"].as_str() {
                return Err(error.to_owned());
            }
            Ok(outcome_of(&outcome["result"]))
        })
    }
}

struct Download {
    records: Records,
    queue: Queue,
}

impl AppImageDownloader for Download {
    fn download(
        &self,
        manifest: Value,
        directory: PathBuf,
        cancel: CancelToken,
        on_progress: ProgressSink,
    ) -> BoxFuture<'static, Result<PathBuf, String>> {
        self.records.push(json!([
            "download",
            directory.to_string_lossy(),
            manifest["version"]
        ]));
        let outcome = self.queue.next();
        Box::pin(async move {
            for event in outcome["progress"].as_array().cloned().unwrap_or_default() {
                tick().await;
                on_progress(event);
            }
            tick().await;
            if outcome["waitAbort"].as_bool() == Some(true) {
                return Err(until_abort(&cancel).await);
            }
            if let Some(error) = outcome["error"].as_str() {
                return Err(error.to_owned());
            }
            let file = directory.join(str_of(&manifest["installer"]["filename"]));
            std::fs::write(&file, "appimage").unwrap();
            Ok(file)
        })
    }
}

struct Extract {
    records: Records,
    queue: Queue,
}

impl PayloadExtractor for Extract {
    fn extract(
        &self,
        file: PathBuf,
        directory: PathBuf,
        cancel: CancelToken,
    ) -> BoxFuture<'static, Result<Payload, String>> {
        self.records.push(json!([
            "extract",
            file.to_string_lossy(),
            directory.to_string_lossy()
        ]));
        let outcome = self.queue.next();
        Box::pin(async move {
            tick().await;
            if outcome["waitAbort"].as_bool() == Some(true) {
                return Err(until_abort(&cancel).await);
            }
            if let Some(error) = outcome["error"].as_str() {
                return Err(error.to_owned());
            }
            let version = str_of(&outcome["version"]).to_owned();
            let name = format!("HLSR-{version}-linux-x86_64.tar.gz");
            Ok(Payload {
                manifest: json!({"version": version, "installer": {"format": "tar.gz", "filename": name}}),
                archive: directory
                    .join("squashfs-root")
                    .join("resources")
                    .join("payload")
                    .join(&name),
            })
        })
    }
}

struct Manifests {
    records: Records,
    queue: Queue,
}

impl ChunkManifests for Manifests {
    fn fetch(
        &self,
        delta: Value,
        version: String,
        _cancel: CancelToken,
    ) -> BoxFuture<'static, Result<Manifest, String>> {
        self.records.push(json!([
            "fetchManifest",
            delta["manifest"]["filename"],
            version,
            "linux"
        ]));
        let outcome = self.queue.next();
        Box::pin(async move {
            tick().await;
            if let Some(error) = outcome["error"].as_str() {
                return Err(error.to_owned());
            }
            Ok(Manifest {
                version,
                platform: hlsr_core::js::Platform::Linux,
                files: Vec::new(),
                attest: None,
            })
        })
    }
}

struct Installer {
    records: Records,
    id: usize,
    plan: Value,
    root: PathBuf,
}

impl VersionInstaller for Installer {
    fn prepare(
        &self,
        archive: PathBuf,
        manifest: Value,
        cancel: CancelToken,
    ) -> BoxFuture<'_, Result<(), String>> {
        self.records.push(json!([
            "installer",
            self.id,
            "prepare",
            archive.to_string_lossy(),
            manifest["version"]
        ]));
        Box::pin(async move {
            tick().await;
            let plan = &self.plan["prepare"];
            if plan["waitAbort"].as_bool() == Some(true) {
                return Err(until_abort(&cancel).await);
            }
            if let Some(error) = plan["error"].as_str() {
                return Err(error.to_owned());
            }
            Ok(())
        })
    }

    fn prepare_from_chunks(
        &self,
        manifest: Manifest,
        channel_manifest: Value,
        options: ChunkPrepare,
    ) -> BoxFuture<'_, Result<Option<ChunkResult>, String>> {
        self.records.push(json!([
            "installer",
            self.id,
            "prepareFromChunks",
            manifest.version,
            channel_manifest["version"],
            options.cache_dir.to_string_lossy(),
            options.chunk_base_urls,
        ]));
        Box::pin(async move {
            let chunks = &self.plan["chunks"];
            for event in chunks["progress"].as_array().cloned().unwrap_or_default() {
                tick().await;
                (options.on_progress)(event);
            }
            tick().await;
            if chunks["waitAbort"].as_bool() == Some(true) {
                return Err(until_abort(&options.cancel).await);
            }
            if let Some(error) = chunks["error"].as_str() {
                return Err(error.to_owned());
            }
            Ok(chunks.get("result").map(|result| ChunkResult {
                downloaded_bytes: result["downloadedBytes"].as_u64().unwrap(),
                reused_bytes: result["reusedBytes"].as_u64().unwrap(),
            }))
        })
    }

    fn commit(&self) -> Result<PathBuf, String> {
        self.records.push(json!(["installer", self.id, "commit"]));
        if let Some(error) = self.plan["commit"]["error"].as_str() {
            return Err(error.to_owned());
        }
        let sha16 = str_of(&FIXTURE["constants"]["sha16"]);
        Ok(self
            .root
            .join(format!("versions/0.4.8-{sha16}"))
            .join("hlsr"))
    }

    fn rollback(&self) -> Result<(), String> {
        self.records.push(json!(["installer", self.id, "rollback"]));
        match self.plan["rollback"]["error"].as_str() {
            Some(error) => Err(error.to_owned()),
            None => Ok(()),
        }
    }

    fn discard(&self) {
        self.records.push(json!(["installer", self.id, "discard"]));
    }
}

struct App(Records);

impl AppControl for App {
    fn quit(&self) {
        self.0.push(json!(["quit"]));
    }
}

impl Relauncher for App {
    fn relaunch(&self, exec_path: &Path, args: &[String]) -> Result<(), String> {
        self.0
            .push(json!(["relaunch", exec_path.to_string_lossy(), args]));
        Ok(())
    }
}

impl UpdateWindow for App {
    fn send_state(&self, payload: Value) {
        self.0.push(json!(["state", payload]));
    }
    fn set_progress_bar(&self, _value: f64) {}
}

struct Telemetry {
    records: Records,
    throws: Option<String>,
}

impl UpdateTelemetry for Telemetry {
    fn available(&self, current: &str, next: Option<&str>) -> Result<(), String> {
        self.records
            .push(json!(["telemetry", "available", current, next]));
        Ok(())
    }
    fn download_started(&self, current: &str, next: Option<&str>) -> Result<(), String> {
        self.records
            .push(json!(["telemetry", "downloadStarted", current, next]));
        Ok(())
    }
    fn download_completed(
        &self,
        current: &str,
        next: &str,
        bytes: Option<f64>,
        duration_ms: Option<i64>,
    ) -> Result<(), String> {
        let bytes = bytes.map_or(Value::Null, hlsr_core::json_text::js_number);
        self.records.push(json!(["telemetry", "downloadCompleted", current, next, {"bytes": bytes, "durationMs": duration_ms}]));
        Ok(())
    }
    fn failed(&self, current: &str, next: Option<&str>, code: Option<&str>) -> Result<(), String> {
        self.records
            .push(json!(["telemetry", "failed", current, next, code]));
        self.throws.clone().map_or(Ok(()), Err)
    }
}

struct Events {
    records: Records,
    available: Option<String>,
    receipt: Option<String>,
}

impl LauncherVersionEvents for Events {
    fn launcher_update_available(
        &self,
        version: &str,
        current: &str,
        has_release_notes: bool,
    ) -> Result<(), String> {
        self.records.push(
            json!(["versionAvailable", version, current, {"hasReleaseNotes": has_release_notes}]),
        );
        self.available.clone().map_or(Ok(()), Err)
    }
    fn write_pending_receipt(&self, from: &str, to: &str) -> Result<(), String> {
        self.records.push(json!(["receipt", from, to]));
        self.receipt.clone().map_or(Ok(()), Err)
    }
}

/// `clock.flush()`: дать задачам дойти до следующего ожидания.
async fn flush() {
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
}

/// `clock.advance(ms)`: шагами не длиннее секунды, как таймеры JS по очереди.
async fn advance(ms: u64) {
    let mut left = ms;
    while left > 0 {
        let step = left.min(1000);
        tokio::time::advance(Duration::from_millis(step)).await;
        flush().await;
        left -= step;
    }
}

/// Сценарий на службе; `via_api` — каналы и снимки идут через `UpdateApi`.
async fn run(scenario: &Value, via_api: bool) -> Vec<Value> {
    let work = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(work.path())
        .unwrap()
        .join("install root");
    std::fs::create_dir(&root).unwrap();
    let records = Records {
        list: Arc::default(),
        root: root.clone(),
    };
    let rec = |value: Value| records.push(value);
    let throws = &scenario["throws"];
    let app = Arc::new(App(records.clone()));
    let game = Arc::new(AtomicBool::new(
        scenario["gameRunning"].as_bool() == Some(true),
    ));
    let installer_plans = Arc::new(Queue::new(&scenario["installers"], json!({})));
    let instances = Arc::new(AtomicUsize::new(0));
    let constants = &FIXTURE["constants"];
    let config = LinuxManagerConfig {
        current_version: "0.4.7".to_owned(),
        packaged: scenario["packaged"].as_bool() != Some(false),
        install_root: (scenario["installRoot"].as_bool() != Some(false)).then(|| root.clone()),
        argv: std::iter::once(str_of(&constants["exe"]).to_owned())
            .chain(
                scenario["argv"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|arg| str_of(arg).to_owned()),
            )
            .collect(),
    };
    let factory = {
        let (records, plans, instances, root) = (
            records.clone(),
            installer_plans.clone(),
            instances.clone(),
            root.clone(),
        );
        Arc::new(move || -> Arc<dyn VersionInstaller> {
            let id = instances.fetch_add(1, Ordering::SeqCst) + 1;
            records.push(json!(["installer", id, "create"]));
            Arc::new(Installer {
                records: records.clone(),
                id,
                plan: plans.next(),
                root: root.clone(),
            })
        })
    };
    let ports = LinuxManagerPorts {
        gate: Arc::new(Gate {
            records: records.clone(),
            queue: Queue::new(&scenario["evaluate"], json!({"result": manifest_result()})),
        }),
        download: Arc::new(Download {
            records: records.clone(),
            queue: Queue::new(&scenario["downloads"], default_download()),
        }),
        extract: Arc::new(Extract {
            records: records.clone(),
            queue: Queue::new(&scenario["extracts"], json!({"version": "0.4.8"})),
        }),
        chunk_manifests: Arc::new(Manifests {
            records: records.clone(),
            queue: Queue::new(&scenario["manifests"], json!({"ok": true})),
        }),
        create_installer: factory,
        app: app.clone(),
        relauncher: app.clone(),
        logger: Arc::new(Log(records.clone())),
        clock: Arc::new(TokioClock {
            base: constants["start"].as_i64().unwrap(),
            start: tokio::time::Instant::now(),
        }),
        random: Arc::new(|| 0.5),
        window: (!via_api).then(|| app.clone() as Arc<dyn UpdateWindow>),
        telemetry: (scenario["noTelemetry"].as_bool() != Some(true)).then(|| {
            Arc::new(Telemetry {
                records: records.clone(),
                throws: throws["telemetry"].as_str().map(str::to_owned),
            }) as Arc<dyn UpdateTelemetry>
        }),
        version_events: (scenario["noVersionEvents"].as_bool() != Some(true)).then(|| {
            Arc::new(Events {
                records: records.clone(),
                available: throws["versionEvents"].as_str().map(str::to_owned),
                receipt: throws["receipt"].as_str().map(str::to_owned),
            }) as Arc<dyn LauncherVersionEvents>
        }),
    };
    let manager = LinuxUpdateManager::new(config, ports);
    {
        let game = game.clone();
        manager.set_game_running_check(move || game.load(Ordering::SeqCst));
    }
    let api = via_api.then(|| {
        UpdateApi::new(UpdateApiOptions {
            updates: Arc::new(manager.clone()),
            window: Some(app.clone()),
            notifier: None,
            diagnostics: None,
            logger: Arc::new(Log(records.clone())),
        })
    });
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
                        records.push(json!(["return", channel, reply]));
                    });
                } else {
                    let reply = api.call(&channel, &args).await;
                    let reply = reply.unwrap_or_else(|| panic!("канал {channel}")).unwrap();
                    rec(json!(["return", channel, reply]));
                }
            }
            "invoke" => {
                let channel = str_of(&op["channel"]).to_owned();
                let reply = |value: Value| rec(json!(["return", channel, value]));
                match channel.as_str() {
                    "updater-check" => reply(json!(manager.check())),
                    "updater-download" => reply(json!(manager.download())),
                    "updater-cancel" => reply(json!(manager.cancel())),
                    "updater-install-now" => reply(json!(manager.install_now())),
                    "updater-install-on-quit" => reply(json!(manager.install_on_quit())),
                    "updater-set-language" => reply(json!(
                        manager.set_language(op["arg"].as_str().unwrap_or(""))
                    )),
                    "updater-get-state" => reply(manager.get_state()),
                    "boot-gate-run" => {
                        let (manager, records) = (manager.clone(), records.clone());
                        let retry = op["arg"]["retry"].as_bool() == Some(true);
                        tokio::spawn(async move {
                            let decision: BootDecision = manager.boot_gate_run(retry).await;
                            records.push(json!(["return", "boot-gate-run", decision.to_json()]));
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
            "game" => game.store(op["running"].as_bool().unwrap(), Ordering::SeqCst),
            "beforeQuit" => {
                let deferred = match &api {
                    Some(api) => api.before_quit(),
                    None => manager.before_quit(),
                };
                if deferred {
                    rec(json!(["preventDefault"]));
                }
            }
            "dispose" => match &api {
                Some(api) => api.dispose(),
                None => manager.dispose(),
            },
            "downloads" => {
                let base = root.join(".downloads");
                let names = base.exists().then(|| {
                    let mut names = hlsr_core::js::fs::read_dir_names(&base).unwrap();
                    names.sort();
                    names
                });
                rec(json!(["downloads", names]));
            }
            other => panic!("шаг {other}"),
        }
        flush().await;
        rec(json!(["step"]));
    }
    manager.dispose();
    flush().await;
    records.list.lock().unwrap().clone()
}

fn manifest_result() -> Value {
    json!({"ok": true, "problems": [], "updateAvailable": true, "downgrade": false, "unsupported": false,
        "code": null, "updateRequired": false, "manifest": FIXTURE["manifests"]["MANIFEST"]})
}

fn default_download() -> Value {
    json!({"progress": [
        {"transferred": 35000, "total": 70000, "percent": 50, "bytesPerSecond": 1000},
        {"transferred": 70000, "total": 70000, "percent": 100, "bytesPerSecond": 2000},
    ], "file": true})
}

/// Записи по шагам: внутри шага — по порядку, ответы каналов — отдельным списком.
fn segments(records: &[Value]) -> Vec<(Vec<Value>, Vec<Value>)> {
    let mut out = vec![(Vec::new(), Vec::new())];
    for record in records {
        if record[0] == "step" {
            out.push((Vec::new(), Vec::new()));
            continue;
        }
        let last = out.last_mut().unwrap();
        if record[0] == "return" {
            last.1.push(record.clone());
        } else {
            last.0.push(record.clone());
        }
    }
    out
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn traces_match_0_4_7() {
    traces(false).await;
}

/// Те же трассы, но каналы окна — `UpdateApi::call` (задача 9).
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn traces_through_update_api() {
    traces(true).await;
}

async fn traces(via_api: bool) {
    let mut mismatches = Mismatches::default();
    for scenario in items(&FIXTURE["scenarios"]) {
        let name = str_of(&scenario["name"]);
        let actual = run(scenario, via_api).await;
        let expected = items(&scenario["records"]);
        let (actual, expected) = (segments(&actual), segments(expected));
        if actual.len() != expected.len() {
            mismatches.check(
                &json!(actual.len()),
                &json!(expected.len()),
                format!("«{name}»: число шагов"),
            );
            continue;
        }
        for (index, (left, right)) in actual.iter().zip(&expected).enumerate() {
            mismatches.check(
                &json!(left.0),
                &json!(right.0),
                format!("«{name}», шаг {index}: записи"),
            );
            mismatches.check(
                &json!(left.1),
                &json!(right.1),
                format!("«{name}», шаг {index}: ответы"),
            );
        }
    }
    mismatches.finish("linux/UpdateManager.js");
}

#[test]
fn error_codes_match_0_4_7() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["errorCodes"]) {
        let text = case["text"]
            .as_str()
            .map_or_else(|| "строка вместо ошибки 429".to_owned(), str::to_owned);
        mismatches.check(
            &json!(update_manager::error_code(&text)),
            &case["code"],
            format!("errorCode «{text}»"),
        );
    }
    mismatches.finish("errorCode");
}

#[test]
fn relaunch_keeps_window_parameters() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["relaunch"]) {
        let argv: Vec<String> = items(&case["argv"])
            .iter()
            .map(|arg| str_of(arg).to_owned())
            .collect();
        mismatches.check(
            &json!(update_manager::relaunch_arguments(&argv)),
            &case["args"],
            format!("{argv:?}"),
        );
    }
    mismatches.finish("relaunchArguments");
    // Самотест 0.4.7 дословно.
    let args = update_manager::relaunch_arguments(&[
        "/installation/current/hlsr".to_owned(),
        "--ozone-platform=wayland".to_owned(),
        "--no-sandbox".to_owned(),
        "--disable-gpu".to_owned(),
    ]);
    assert_eq!(
        args,
        ["--no-sandbox", "--ozone-platform=wayland", "--disable-gpu"]
    );
}

// ------------------------------------------------------------------ только Rust: настоящий рантайм

/// «AppImage», рантайм которого не завершается сам (`--appimage-extract` огромного образа): в Windows
/// — пакетный файл с бесконечным циклом внутри `cmd.exe` (без дочерних процессов), в Unix — `sleep`.
/// Перед циклом он оставляет в каталоге загрузки отметку `started`.
struct EndlessAppImage {
    directory: Arc<Mutex<Option<PathBuf>>>,
}

impl AppImageDownloader for EndlessAppImage {
    fn download(
        &self,
        _manifest: Value,
        directory: PathBuf,
        _cancel: CancelToken,
        _on_progress: ProgressSink,
    ) -> BoxFuture<'static, Result<PathBuf, String>> {
        *self.directory.lock().unwrap() = Some(directory.clone());
        Box::pin(async move {
            let (name, script) = if cfg!(windows) {
                (
                    "HLSR-Setup.cmd",
                    "@echo started> started\r\n@for /l %%i in (0,0,1) do @rem\r\n",
                )
            } else {
                (
                    "HLSR-Setup.AppImage",
                    "#!/bin/sh\necho started > started\nexec sleep 600\n",
                )
            };
            let file = directory.join(name);
            std::fs::write(&file, script).unwrap();
            Ok(file)
        })
    }
}

struct Live {
    manager: LinuxUpdateManager,
    records: Records,
    directory: Arc<Mutex<Option<PathBuf>>>,
    /// Сколько раз цикл проверок запрашивал разброс (`Math.random()`): по разу за круг.
    random_calls: Arc<AtomicUsize>,
    _work: tempfile::TempDir,
}

impl Live {
    fn new() -> Live {
        let work = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(work.path()).unwrap().join("install");
        std::fs::create_dir(&root).unwrap();
        let records = Records {
            list: Arc::default(),
            root: root.clone(),
        };
        let app = Arc::new(App(records.clone()));
        let directory = Arc::new(Mutex::new(None));
        let random_calls = Arc::new(AtomicUsize::new(0));
        let installer = {
            let (records, root) = (records.clone(), root.clone());
            Arc::new(move || -> Arc<dyn VersionInstaller> {
                Arc::new(Installer {
                    records: records.clone(),
                    id: 1,
                    plan: json!({}),
                    root: root.clone(),
                })
            })
        };
        let ports = LinuxManagerPorts {
            gate: Arc::new(Gate {
                records: records.clone(),
                queue: Queue::new(&Value::Null, json!({"result": manifest_result()})),
            }),
            download: Arc::new(EndlessAppImage {
                directory: directory.clone(),
            }),
            extract: Arc::new(update_manager::AppImageExtractor {
                runner: Arc::new(hlsr_update::linux::SystemRunner),
                env: vec![("PATH".to_owned(), "/usr/bin:/bin".to_owned())],
            }),
            chunk_manifests: Arc::new(Manifests {
                records: records.clone(),
                queue: Queue::new(&Value::Null, json!({"ok": true})),
            }),
            create_installer: installer,
            app: app.clone(),
            relauncher: app.clone(),
            logger: Arc::new(Log(records.clone())),
            clock: Arc::new(TokioClock {
                base: FIXTURE["constants"]["start"].as_i64().unwrap(),
                start: tokio::time::Instant::now(),
            }),
            random: {
                let random_calls = random_calls.clone();
                Arc::new(move || {
                    random_calls.fetch_add(1, Ordering::SeqCst);
                    0.5
                })
            },
            window: Some(app.clone()),
            telemetry: None,
            version_events: None,
        };
        let config = LinuxManagerConfig {
            current_version: "0.4.7".to_owned(),
            packaged: true,
            install_root: Some(root),
            argv: vec![str_of(&FIXTURE["constants"]["exe"]).to_owned()],
        };
        Live {
            manager: LinuxUpdateManager::new(config, ports),
            records,
            directory,
            random_calls,
            _work: work,
        }
    }

    fn state(&self) -> String {
        str_of(&self.manager.get_state()["state"]).to_owned()
    }

    /// Проверка, загрузка и запущенный рантайм в состоянии `verifying`; каталог загрузки.
    async fn verifying(&self) -> PathBuf {
        assert!(self.manager.check());
        wait_for("available", || self.state() == "available").await;
        assert!(self.manager.download());
        wait_for("verifying", || self.state() == "verifying").await;
        let directory = self.directory.lock().unwrap().clone().unwrap();
        wait_for("рантайм запущен", || {
            directory.join("started").exists()
        })
        .await;
        assert_eq!(self.state(), "verifying");
        directory
    }
}

/// Ждать условия не дольше 20 с; сколько ждали.
async fn wait_for(what: &str, done: impl Fn() -> bool) -> Duration {
    let start = std::time::Instant::now();
    while !done() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "не дождались: {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    start.elapsed()
}

/// `updater-cancel` в `verifying` снимает `--appimage-extract` сразу (`execFile({signal})` 0.4.7):
/// обновление снова доступно, каталог загрузки убран (живой процесс в Windows его бы держал).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_while_verifying_stops_the_appimage_runtime() {
    let live = Live::new();
    let directory = live.verifying().await;
    assert!(live.manager.cancel());
    let waited = wait_for("available после отмены", || {
        live.state() == "available" && !directory.exists()
    })
    .await;
    assert!(waited < Duration::from_secs(5), "отмена шла {waited:?}");
    let state = live.manager.get_state();
    assert_eq!(state["error"], Value::Null);
    assert_eq!(state["errorCode"], Value::Null);
    live.manager.dispose();
}

/// `before-quit` в `verifying`: выход откладывается, рантайм снимается, затем `app.quit()`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quit_while_verifying_stops_the_appimage_runtime() {
    let live = Live::new();
    let directory = live.verifying().await;
    assert!(live.manager.before_quit());
    let waited = wait_for("quit", || {
        live.records.list.lock().unwrap().contains(&json!(["quit"])) && !directory.exists()
    })
    .await;
    assert!(waited < Duration::from_secs(5), "выход шёл {waited:?}");
    live.manager.dispose();
}

/// После `dispose()` цикл проверок не заводится заново: у 0.4.7 `this.timer` остаётся взведённым и
/// `startCheckingInterval()` выходит сразу. Видно по разбросу (`Math.random()`), который цикл берёт
/// в начале каждого круга.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_check_loop_is_not_restarted_after_dispose() {
    let live = Live::new();
    live.manager.start_checking_interval();
    wait_for("первый круг цикла", || {
        live.random_calls.load(Ordering::SeqCst) == 1
    })
    .await;
    // Повторный запуск живого цикла ничего не меняет.
    live.manager.start_checking_interval();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(live.random_calls.load(Ordering::SeqCst), 1);

    live.manager.dispose();
    live.manager.start_checking_interval();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        live.random_calls.load(Ordering::SeqCst),
        1,
        "после dispose() заведён новый цикл проверок"
    );
}
