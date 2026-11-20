//! Загрузка с докачкой против 0.4.7: фикстура `download.json` из
//! `tools/reference/delivery/download.js` (`GameDownloadManager.js` в Node 24 на тех же
//! сценариях). Сценарий — серверы с ответами по очереди (`tests/support/http.rs`), загрузки
//! подряд, файлы до запуска, действия во время загрузки и вызовы после неё. Сравниваются
//! итог или ошибка, события и строки журнала в порядке появления, содержимое `.part.json` в
//! момент событий, дерево каталога `Downloads`, тексты JSON, журнал запросов.
//!
//! Время и случайность — поддельные, как у Node: часы сценария двигает действие `clock`,
//! разброс повтора задан числом. Куски тела разделены воротами сервера, поэтому число
//! событий `progress` не зависит от сети. Скорость и ETA сверяются тоже: при поддельных часах
//! это проверка формулы, а не шум.
//!
//! Сценарии идут через источник `local-http`, который есть только в сборке с отладочными
//! возможностями.
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_core::js::path::NodePath;
use hlsr_core::json_text::{parse_js_json, stringify};
use hlsr_delivery::errors::{Code, Failure};
use hlsr_delivery::game_download_manager::{
    self as manager, DownloadEvent, DownloadOptions, DownloadRequest, GameDownloadManager,
    StateFile,
};
use hlsr_delivery::http_client::HttpClient;
use hlsr_delivery::mirror_runner::Clock;
use hlsr_delivery::ports::{FreeSpace, Logger};
use hlsr_delivery::providers::base_provider::BaseProvider;
use hlsr_delivery::providers::{self, BoxFuture, Provider, ProviderOptions, Resolved};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use support::http::{RequestLog, Server, dead_port, start_server};
use support::tree::snapshot;
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("download.json"));

/// Тело `bodySize` байт, как отдаёт сервер сценария.
fn payload(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 251) as u8).collect()
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ---------------------------------------------------------------- подстановки

/// Подстановка адресов: `{имя}` → `127.0.0.1:<порт>` во всех строках и ключах.
fn fill(value: &Value, map: &BTreeMap<String, String>) -> Value {
    match value {
        Value::String(text) => Value::String(fill_text(text, map)),
        Value::Array(items) => Value::Array(items.iter().map(|v| fill(v, map)).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(k, v)| (fill_text(k, map), fill(v, map)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn fill_text(text: &str, map: &BTreeMap<String, String>) -> String {
    let mut out = text.to_owned();
    for (name, authority) in map {
        out = out.replace(&format!("{{{name}}}"), authority);
    }
    out
}

/// Обратная замена: адрес → `{имя}`; длинные адреса первыми. В строках с `{root}` разделитель
/// путей Windows (`\`) становится `/`: фикстура снята на Linux.
fn unfill(value: &Value, map: &BTreeMap<String, String>) -> Value {
    let mut pairs: Vec<(&String, &String)> = map.iter().collect();
    pairs.sort_by_key(|(_, authority)| std::cmp::Reverse(authority.len()));
    let text = |t: &str| {
        let out = pairs.iter().fold(t.to_owned(), |t, (name, authority)| {
            t.replace(*authority, &format!("{{{name}}}"))
        });
        if cfg!(windows) && out.contains("{root}") {
            out.replace('\\', "/")
        } else {
            out
        }
    };
    match value {
        Value::String(t) => Value::String(text(t)),
        Value::Array(items) => Value::Array(items.iter().map(|v| unfill(v, map)).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(k, v)| (text(k), unfill(v, map)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Числа так, как их видит JS после `JSON.parse`.
fn normalize(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(k, v)| (k.clone(), normalize(v)))
                .collect(),
        ),
        Value::Number(_) => parse_js_json(&stringify(value, "")).unwrap(),
        other => other.clone(),
    }
}

/// Журнал запросов Rust: reqwest всегда шлёт `Accept: */*`, Node — нет.
fn without_default_accept(log: &[Value]) -> Vec<Value> {
    log.iter()
        .map(|entry| {
            let mut entry = entry.clone();
            let headers = entry["headers"].as_object_mut().unwrap();
            if headers.get("accept") == Some(&json!("*/*")) {
                headers.remove("accept");
            }
            entry
        })
        .collect()
}

// ---------------------------------------------------------------- запись событий

/// Журнал и слушатель событий пишут в один список — порядок виден целиком.
#[derive(Default)]
struct Recorder {
    timeline: Mutex<Vec<Value>>,
    state_file: Mutex<Option<PathBuf>>,
}

impl Recorder {
    fn push(&self, entry: Value) {
        self.timeline.lock().unwrap().push(entry);
    }

    fn event(&self, ev: &str, data: Value) {
        let mut entry = json!({ "ev": ev, "data": data });
        let path = self.state_file.lock().unwrap().clone();
        if let Some(text) = path.and_then(|p| std::fs::read(p).ok()) {
            entry["stateFile"] = json!(String::from_utf8_lossy(&text));
        }
        self.push(entry);
    }

    fn count_states(&self, state: &str) -> usize {
        self.timeline
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e["ev"] == "state" && e["data"]["state"] == state)
            .count()
    }
}

impl Logger for Recorder {
    fn info(&self, message: &str) {
        self.push(json!({ "log": "info", "message": message }));
    }
    fn warn(&self, message: &str) {
        self.push(json!({ "log": "warn", "message": message }));
    }
    fn error(&self, message: &str) {
        self.push(json!({ "log": "error", "message": message }));
    }
}

/// Источник, которому нужна новая ссылка на каждой попытке: `resolve()` отдаёт пути по
/// очереди (последний повторяется).
struct Relinking {
    base: BaseProvider,
    authority: String,
    paths: Vec<String>,
    calls: Mutex<usize>,
}

impl Provider for Relinking {
    fn base(&self) -> &BaseProvider {
        &self.base
    }

    fn resolve(&self) -> BoxFuture<'_, Result<Resolved, Failure>> {
        let mut calls = self.calls.lock().unwrap();
        let path = &self.paths[(*calls).min(self.paths.len() - 1)];
        *calls += 1;
        let resolved = Resolved {
            url: format!("http://{}{path}", self.authority),
            headers: Vec::new(),
            temporary: true,
        };
        Box::pin(async move { Ok(resolved) })
    }
}

/// `freeSpaceProvider: () => value`.
struct Fixed(Option<u64>);

impl FreeSpace for Fixed {
    fn free_bytes(&self, _dir: &Path) -> Option<u64> {
        self.0
    }
}

fn failure_json(failure: &Failure) -> Value {
    match failure {
        Failure::Delivery(error) => json!({ "error": error.to_public() }),
        Failure::Raw(raw) => json!({ "raw": { "code": raw.code, "message": raw.message } }),
    }
}

fn ok(value: Value) -> Value {
    json!({ "ok": value })
}

// ---------------------------------------------------------------- сценарий

struct Run {
    downloads: PathBuf,
    map: BTreeMap<String, String>,
    http: HttpClient,
    recorder: Arc<Recorder>,
    clock: Arc<AtomicI64>,
    servers: Vec<Server>,
}

impl Run {
    fn manager(&self, config: &Value) -> GameDownloadManager {
        let now: Clock = {
            let clock = self.clock.clone();
            Arc::new(move || clock.load(Ordering::SeqCst))
        };
        let random = config["random"].as_f64().unwrap_or(0.5);
        let free = config.get("freeSpace").and_then(Value::as_u64);
        let manager = GameDownloadManager::new(
            &self.downloads,
            DownloadOptions {
                http: Some(self.http.clone()),
                logger: Some(self.recorder.clone()),
                timeout: config["timeout"].as_u64().map(Duration::from_millis),
                max_attempts: config["maxAttempts"].as_u64().map(|n| n as u32),
                backoff_base_ms: config["backoffBaseMs"].as_u64(),
                free_space_reserve: config["freeSpaceReserve"].as_u64(),
                free_space: Some(Arc::new(Fixed(free))),
                clock: Some(now),
                random: Some(Arc::new(move || random)),
                ..DownloadOptions::default()
            },
        );
        let recorder = self.recorder.clone();
        manager.on_event(move |event| match event {
            DownloadEvent::State(state) => recorder.event("state", state.to_json()),
            DownloadEvent::Progress(progress) => recorder.event("progress", progress.to_json()),
        });
        manager
    }

    fn request(&self, spec: &Value) -> DownloadRequest {
        let size = spec["size"].as_u64().map(|n| n as usize);
        let sha = match (size, spec["sha256"].as_str()) {
            (Some(size), Some(kind)) => {
                let ok = sha256(&payload(size));
                Some(match kind {
                    "wrong" => "0".repeat(64),
                    "upper" => ok.to_uppercase(),
                    _ => ok,
                })
            }
            _ => None,
        };
        let authority = self.map[str_of(&spec["server"])].clone();
        let provider: Arc<dyn Provider> = match spec.get("paths") {
            Some(paths) => Arc::new(Relinking {
                base: BaseProvider::new(&json!({ "id": "p" })),
                authority,
                paths: items(paths).iter().map(|p| str_of(p).to_owned()).collect(),
                calls: Mutex::new(0),
            }),
            None => {
                let descriptor = json!({
                    "id": "p",
                    "type": "local-http",
                    "url": format!("http://{authority}{}", str_of(&spec["path"])),
                });
                let options =
                    ProviderOptions::with_allowed_provider_types(self.http.clone(), ["local-http"]);
                Arc::from(providers::create(&descriptor, &options).unwrap())
            }
        };
        DownloadRequest {
            game_id: str_of(&spec["gameId"]).to_owned(),
            provider,
            filename: str_of(&spec["filename"]).to_owned(),
            expected_size: size.map(|n| n as u64),
            expected_sha256: sha,
        }
    }

    fn prepare(&self, item: &Value) {
        let file = self.downloads.join(str_of(&item["path"]));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        if let Some(range) = item.get("payload") {
            let (from, to) = (range[0].as_u64().unwrap(), range[1].as_u64().unwrap());
            std::fs::write(&file, &payload(to as usize)[from as usize..]).unwrap();
        } else if let Some(junk) = item.get("junk") {
            std::fs::write(&file, vec![0xaa; junk.as_u64().unwrap() as usize]).unwrap();
        } else if let Some(target) = item.get("symlink") {
            #[cfg(unix)]
            std::os::unix::fs::symlink(str_of(target), &file).unwrap();
            #[cfg(not(unix))]
            panic!("ссылка {target}: сценарий только для Linux");
        } else {
            std::fs::write(&file, str_of(&item["text"])).unwrap();
        }
    }
}

async fn until(what: &str, mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "не дождались {what}"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// Действия во время загрузки.
async fn drive(run: &Run, manager: &GameDownloadManager, start: &Value, part: &Path) -> Vec<Value> {
    let mut results = Vec::new();
    for action in start["actions"].as_array().into_iter().flatten() {
        if let Some(size) = action.get("waitPart") {
            let size = size.as_u64().unwrap();
            until(&format!(".part из {size} байт"), || {
                std::fs::metadata(part).is_ok_and(|m| m.len() == size)
            })
            .await;
        } else if let Some(state) = action.get("waitState") {
            let nth = action["nth"].as_u64().unwrap_or(1) as usize;
            until(&format!("состояния {state}"), || {
                run.recorder.count_states(str_of(state)) >= nth
            })
            .await;
        } else if let Some(ms) = action.get("clock") {
            run.clock.fetch_add(ms.as_i64().unwrap(), Ordering::SeqCst);
        } else if let Some(name) = action.get("release") {
            for server in &run.servers {
                server.release(str_of(name));
            }
        } else if action.get("pause").is_some() {
            results.push(json!({ "action": "pause", "result": manager.pause() }));
        } else if action.get("cancel").is_some() {
            results.push(json!({ "action": "cancel", "result": manager.cancel() }));
        } else if action.get("busy").is_some() {
            results.push(json!({ "action": "busy", "result": manager.is_busy() }));
        } else if let Some(spec) = action.get("tryStart") {
            let result = match manager.start(run.request(spec)).await {
                Ok(outcome) => ok(outcome.to_json()),
                Err(failure) => failure_json(&failure),
            };
            results.push(json!({ "action": "tryStart", "result": result }));
        } else {
            panic!("неизвестное действие {action}");
        }
    }
    results
}

/// Вызов после загрузки.
fn after_call(manager: &GameDownloadManager, call: &Value) -> Value {
    let game = || str_of(&call["gameId"]);
    let file = || str_of(&call["filename"]);
    let delivery = |result: Result<Value, hlsr_delivery::errors::DeliveryError>| match result {
        Ok(value) => ok(value),
        Err(error) => json!({ "error": error.to_public() }),
    };
    let failure = |result: Result<Value, Failure>| match result {
        Ok(value) => ok(value),
        Err(failure) => failure_json(&failure),
    };
    match str_of(&call["call"]) {
        "findResumable" => delivery(
            manager
                .find_resumable(game(), file())
                .map(|found| found.map_or(Value::Null, |r| r.to_json())),
        ),
        "discard" => failure(manager.discard(game(), file()).map(|()| Value::Null)),
        "discardPaused" => failure(manager.discard_paused(game(), None).map(Value::Bool)),
        "partPaths" => delivery(manager.part_paths(game(), file()).map(|paths| {
            json!({
                "dir": paths.dir.to_string_lossy(),
                "part": paths.part.to_string_lossy(),
                "state": paths.state.to_string_lossy(),
                "target": paths.target.to_string_lossy(),
            })
        })),
        "writeState" => {
            let state = &call["state"];
            let paths = manager.part_paths(game(), file()).unwrap();
            let fields = StateFile {
                game_id: state["gameId"].as_str().map(str::to_owned),
                filename: state["filename"].as_str().map(str::to_owned),
                expected_size: state["expectedSize"].as_u64(),
                expected_sha256: state["expectedSha256"].as_str().map(str::to_owned),
                provider_id: state["providerId"].as_str().map(str::to_owned),
                bytes_downloaded: state["bytesDownloaded"].as_u64(),
            };
            match manager.write_state(&paths.state, &fields) {
                Ok(()) => ok(Value::Null),
                Err(raw) => failure_json(&raw.into()),
            }
        }
        "readState" => {
            let paths = manager.part_paths(game(), file()).unwrap();
            ok(manager.read_state(&paths.state).unwrap_or(Value::Null))
        }
        "pause" => json!(manager.pause()),
        "cancel" => json!(manager.cancel()),
        "isBusy" => json!(manager.is_busy()),
        other => panic!("неизвестный вызов {other}"),
    }
}

fn snapshot_json(downloads: &Path) -> (Value, Value) {
    let tree = snapshot(downloads, None);
    let mut texts = Map::new();
    for entry in tree.as_array().unwrap() {
        let path = str_of(&entry["path"]);
        if entry["kind"] == "file" && path.ends_with(".json") {
            let bytes = std::fs::read(downloads.join(path)).unwrap();
            texts.insert(
                path.to_owned(),
                json!(String::from_utf8_lossy(&bytes).into_owned()),
            );
        }
    }
    (tree, Value::Object(texts))
}

async fn run_scenario(spec: &Value) -> Mismatches {
    let name = str_of(&spec["name"]);
    let root = tempfile::tempdir().unwrap();
    let downloads = root.path().join("Downloads");
    std::fs::create_dir(&downloads).unwrap();

    let log = RequestLog::default();
    let mut map = BTreeMap::new();
    map.insert(
        "dead".to_owned(),
        format!("127.0.0.1:{}", dead_port().await),
    );
    let mut servers = Vec::new();
    for server_name in spec["servers"].as_object().unwrap().keys() {
        let server = start_server(server_name, log.clone()).await;
        map.insert(server_name.clone(), server.authority.clone());
        servers.push(server);
    }
    for server in &servers {
        let routes = fill(&spec["servers"][&server.name], &map);
        *server.routes.lock().unwrap() = routes.as_object().unwrap().clone();
    }

    let clock = Arc::new(AtomicI64::new(FIXTURE["now"].as_i64().unwrap()));
    let run = Run {
        downloads: downloads.clone(),
        map: map.clone(),
        http: HttpClient::new(),
        recorder: Arc::new(Recorder::default()),
        clock,
        servers,
    };
    let mut mask = map;
    mask.insert(
        "root".to_owned(),
        root.path().to_string_lossy().into_owned(),
    );

    let mut m = Mismatches::default();
    let mut config = spec["manager"].clone();
    let mut current = run.manager(&config);
    let native = NodePath::native();
    assert_eq!(
        items(&spec["starts"]).len(),
        items(&spec["results"]).len(),
        "шагов и результатов поровну"
    );
    for (index, (start, expected)) in items(&spec["starts"])
        .iter()
        .zip(items(&spec["results"]))
        .enumerate()
    {
        if let Some(over) = start.get("manager") {
            let mut merged = spec["manager"].as_object().unwrap().clone();
            merged.extend(over.as_object().unwrap().clone());
            config = Value::Object(merged);
            current = run.manager(&config);
        }
        run.recorder.timeline.lock().unwrap().clear();
        run.clock
            .store(FIXTURE["now"].as_i64().unwrap(), Ordering::SeqCst);
        let mark = log.lock().unwrap().len();
        for item in start["prepare"].as_array().into_iter().flatten() {
            run.prepare(item);
        }

        let request = start.get("request");
        let (part, state_file) = match request {
            Some(request) => {
                let base = native.basename(str_of(&request["filename"]));
                let dir = native.join(&[&downloads.to_string_lossy(), str_of(&request["gameId"])]);
                (
                    PathBuf::from(native.join(&[&dir, &format!("{base}.part")])),
                    Some(PathBuf::from(
                        native.join(&[&dir, &format!("{base}.part.json")]),
                    )),
                )
            }
            None => (PathBuf::new(), None),
        };
        *run.recorder.state_file.lock().unwrap() = state_file;

        let (result, actions) = match request {
            Some(request) => {
                let started = async {
                    match current.start(run.request(request)).await {
                        Ok(outcome) => ok(outcome.to_json()),
                        Err(failure) => failure_json(&failure),
                    }
                };
                let (result, actions) = tokio::join!(started, drive(&run, &current, start, &part));
                (Some(result), actions)
            }
            None => (None, drive(&run, &current, start, &part).await),
        };

        let after: Vec<Value> = start["after"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|call| after_call(&current, call))
            .collect();
        let (tree, texts) = snapshot_json(&downloads);
        let requests = without_default_accept(&log.lock().unwrap()[mark..]);

        let mut step = json!({
            "timeline": run.recorder.timeline.lock().unwrap().clone(),
            "tree": tree,
            "texts": texts,
            "requests": requests,
            "busy": current.is_busy(),
        });
        if let Some(result) = result {
            step["result"] = result;
        }
        if !actions.is_empty() {
            step["actions"] = json!(actions);
        }
        if !after.is_empty() {
            step["after"] = json!(after);
        }
        m.check(
            &normalize(&unfill(&step, &mask)),
            &normalize(expected),
            format!("{name}, загрузка {index}: {}", start["request"]),
        );
    }
    m
}

fn scenario(name: &str) -> &'static Value {
    items(&FIXTURE["scenarios"])
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("нет сценария {name}"))
}

macro_rules! scenarios {
    ($($test:ident => $name:literal),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $test() {
                run_scenario(scenario($name)).await.finish($name);
            }
        )*
    };
}

scenarios! {
    complete_matches_0_4_7 => "complete",
    resume_after_cut_matches_0_4_7 => "resume-after-cut",
    range_refused_matches_0_4_7 => "range-refused",
    bad_length_matches_0_4_7 => "bad-length",
    checksum_matches_0_4_7 => "checksum",
    http_status_matches_0_4_7 => "http-status",
    pause_resume_matches_0_4_7 => "pause-resume",
    progress_matches_0_4_7 => "progress",
    restore_matches_0_4_7 => "restore",
    resumable_matches_0_4_7 => "resumable",
    cancel_matches_0_4_7 => "cancel",
    no_space_matches_0_4_7 => "no-space",
    paths_matches_0_4_7 => "paths",
}

/// Только Linux: `.part` — ссылка на `/dev/full`.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn write_error_matches_0_4_7() {
    run_scenario(scenario("write-error"))
        .await
        .finish("write-error");
}

#[test]
fn constants_match_0_4_7() {
    let c = &FIXTURE["constants"];
    assert_eq!(c["STATE_VERSION"], json!(manager::STATE_VERSION));
    let names: Vec<&str> = manager::RETRYABLE_CODES
        .iter()
        .map(|code: &Code| code.as_str())
        .collect();
    assert_eq!(c["RETRYABLE_CODES"], json!(names));
}

#[test]
fn every_scenario_is_checked() {
    let names: Vec<&str> = items(&FIXTURE["scenarios"])
        .iter()
        .map(|s| str_of(&s["name"]))
        .collect();
    let expected = [
        "complete",
        "resume-after-cut",
        "range-refused",
        "bad-length",
        "checksum",
        "http-status",
        "pause-resume",
        "progress",
        "restore",
        "resumable",
        "cancel",
        "no-space",
        "paths",
        "write-error",
    ];
    assert_eq!(names, expected);
}
