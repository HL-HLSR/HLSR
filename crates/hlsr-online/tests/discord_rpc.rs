//! Служба и транспорт Discord против 0.4.7: фикстура `discord-rpc.json` из
//! `tools/reference/online/discord-rpc.js` (Node 24) — мост `discord-rpc/index.js` со службой
//! `DiscordRichPresenceService.js` и настоящим npm `discord-rpc` 4.0.1 против двойника Discord.
//! Те же сценарии (шаги, поддельные часы и таймеры) идут против службы Rust и такого же двойника;
//! сравниваются состояние службы, строки журнала, кадры, которые увидел двойник (op и JSON с
//! нормализованными `nonce` и `pid`), `register`, события окна и вызовы игрового моста.
//! Двойник — именованный канал на Windows и Unix-сокет на Linux; адреса свои, настоящего Discord
//! тесты не касаются. Сценарии сверяются с `discord-rpc-049.json` (тот же скрипт на копии 0.4.9:
//! статус чужой игры не меняет выбранную), остальное — с `discord-rpc.json`.
//!
//! Плюс чистые части: адреса IPC обеих ОС, байты кадров, `uuid`, кадры `SET_ACTIVITY`, задержки
//! повтора — и то, чего в 0.4.7 нет (кадры, склеенные в одном чтении сокета).

#[path = "support/discord.rs"]
mod discord;
mod support;

use discord::{DiscordDouble, Endpoint, FakeTimers, Journal};
use hlsr_core::json_text::js_number;
use hlsr_delivery::ports::MemoryLogger;
use hlsr_delivery::timing::Random;
use hlsr_online::discord_rpc::discord_rich_presence_bridge::{
    BridgeOptions, ClientFactory, DiscordRichPresenceBridge, GamePresenceLink, StateSink,
};
use hlsr_online::discord_rpc::discord_rich_presence_service::{PresenceClient, retry_delay_ms};
use hlsr_online::discord_rpc::presence_model::{self, Activity, Button};
use hlsr_online::discord_rpc::rpc::client::{RpcClient, set_activity_args};
use hlsr_online::discord_rpc::rpc::ipc::{self, IpcConfig, Platform, ipc_path};
use hlsr_online::discord_rpc::rpc::register::{FakeProtocolClient, ProtocolClient};
use hlsr_online::discord_rpc::rpc::util::{register, uuid4122};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use support::{FakeClock, Mismatches, files, fixture, items, sequence_random, str_of};

/// Тайм-аут подключения в сценарии «тайм-аут»: короче 10 с настоящих (сами 10 с — константа,
/// её проверяет отдельный тест; Node в этом сценарии ждёт настоящие 10 с).
const SHORT_TIMEOUT: Duration = Duration::from_millis(1500);

fn leak(text: &str) -> &'static str {
    Box::leak(text.to_owned().into_boxed_str())
}

struct GameLink {
    calls: Arc<Mutex<Vec<Value>>>,
    diagnostics: Arc<Mutex<Value>>,
}

impl GamePresenceLink for GameLink {
    fn set_discord_connected(&self, connected: bool) {
        self.calls
            .lock()
            .unwrap()
            .push(json!(["setDiscordConnected", connected]));
    }

    fn diagnostics(&self) -> Value {
        self.diagnostics.lock().unwrap().clone()
    }
}

struct Progress {
    journal: usize,
    registered: usize,
    window: usize,
    game: usize,
}

struct Run {
    name: String,
    bridge: DiscordRichPresenceBridge,
    double: DiscordDouble,
    journal: Journal,
    logger: Arc<MemoryLogger>,
    pending_log: Vec<Value>,
    timers: Arc<FakeTimers>,
    clock: FakeClock,
    window: Arc<Mutex<Vec<Value>>>,
    game_calls: Arc<Mutex<Vec<Value>>>,
    game_diagnostics: Arc<Mutex<Value>>,
    protocol: Arc<FakeProtocolClient>,
    version: Arc<Mutex<Option<String>>>,
    locale: Arc<Mutex<Option<String>>>,
    user_data: PathBuf,
    progress: Progress,
    started_at: Option<Instant>,
}

static SCENARIO: AtomicUsize = AtomicUsize::new(0);

impl Run {
    async fn new(scenario: &Value, now: i64, root: &tempfile::TempDir) -> Run {
        let user_data = root.path().join("user");
        std::fs::create_dir_all(&user_data).unwrap();
        support::setup(&user_data, &scenario["setup"]);
        let sockets = root.path().join("ipc");
        std::fs::create_dir_all(&sockets).unwrap();
        let index = SCENARIO.fetch_add(1, Ordering::SeqCst);
        let endpoint = Endpoint::new(
            sockets,
            format!("hlsr-rust-{}-{index}-", std::process::id()),
        );

        let name = str_of(&scenario["name"]).to_owned();
        let journal: Journal = Arc::default();
        let double =
            DiscordDouble::start(endpoint.clone(), &scenario["double"], journal.clone()).await;

        let logger = Arc::new(MemoryLogger::default());
        let timers = Arc::new(FakeTimers::default());
        let clock = FakeClock::new(now);
        let protocol = Arc::new(FakeProtocolClient::new(true));
        let version = Arc::new(Mutex::new(Some("0.5.0".to_owned())));
        let locale = Arc::new(Mutex::new(Some("ru".to_owned())));

        // Короткий тайм-аут просит сам сценарий (`shortConnectTimeout`), а не подстрока его имени.
        let timeout = if scenario["shortConnectTimeout"] == json!(true) {
            SHORT_TIMEOUT
        } else {
            ipc::CONNECT_TIMEOUT
        };
        let mut options = BridgeOptions::new(&user_data, logger.clone());
        options.now = clock.clock();
        let provider = version.clone();
        options.version_provider = Some(Arc::new(move || provider.lock().unwrap().clone()));
        let provider = locale.clone();
        options.locale_provider = Some(Arc::new(move || provider.lock().unwrap().clone()));
        options.timers = Some(timers.clone());
        options.random = Some(sequence_random(vec![0.5, 0.0, 0.999, 0.25, 0.75]));
        options.client = if scenario["noFactory"] == json!(true) {
            ClientFactory::Missing
        } else {
            ClientFactory::System {
                protocol: protocol.clone() as Arc<dyn ProtocolClient>,
                ipc: endpoint.ipc_config(timeout),
            }
        };
        let bridge = DiscordRichPresenceBridge::new(options);
        let window: Arc<Mutex<Vec<Value>>> = Arc::default();
        let sink_window = window.clone();
        bridge.set_state_sink(Some(Arc::new(move |snapshot: &Value| {
            sink_window
                .lock()
                .unwrap()
                .push(json!(["discord-rpc-state", snapshot]));
        }) as StateSink));

        Run {
            name,
            bridge,
            double,
            journal,
            logger,
            pending_log: Vec::new(),
            timers,
            clock,
            window,
            game_calls: Arc::default(),
            game_diagnostics: Arc::new(Mutex::new(Value::Null)),
            protocol,
            version,
            locale,
            user_data,
            progress: Progress {
                journal: 0,
                registered: 0,
                window: 0,
                game: 0,
            },
            started_at: None,
        }
    }

    fn in_count(&self) -> usize {
        self.journal
            .lock()
            .unwrap()
            .iter()
            .filter(|entry| entry.get("in").is_some())
            .count()
    }

    fn drain_log(&mut self) {
        for (level, text) in self.logger.take() {
            self.pending_log.push(json!([level, text]));
        }
    }

    fn holds(&self, until: &Value, base_in: usize, base_events: usize) -> bool {
        let state = self.bridge.service().public_state().to_json();
        until
            .as_object()
            .into_iter()
            .flatten()
            .all(|(key, want)| match key.as_str() {
                "inDelta" => self.in_count() - base_in >= want.as_u64().unwrap() as usize,
                "eventsDelta" => {
                    self.journal.lock().unwrap().len() - base_events
                        >= want.as_u64().unwrap() as usize
                }
                "connected" => json!(self.bridge.service().connected()) == *want,
                other => state[other] == *want,
            })
    }

    fn signature(&mut self) -> String {
        self.drain_log();
        json!([
            self.bridge.service().public_state().to_json(),
            self.journal.lock().unwrap().len(),
            self.pending_log.len(),
            self.window.lock().unwrap().len(),
            self.game_calls.lock().unwrap().len(),
            self.timers.len(),
            self.protocol.calls().len(),
        ])
        .to_string()
    }

    async fn quiesce(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut last = self.signature();
        let mut since = Instant::now();
        while since.elapsed() < Duration::from_millis(90) {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let next = self.signature();
            if next != last {
                last = next;
                since = Instant::now();
            }
            assert!(
                Instant::now() < deadline,
                "{}: нет тишины, {last}",
                self.name
            );
        }
    }

    async fn step(&mut self, step: &Value) -> Value {
        let base_in = self.in_count();
        let base_events = self.journal.lock().unwrap().len();
        let op = str_of(&step["op"]);
        let mut result: Option<Value> = None;
        match op {
            "start" => {
                self.started_at = Some(Instant::now());
                self.bridge.start();
            }
            "get" => result = Some(self.bridge.get()),
            "set" => {
                result = Some(self.bridge.set(step["enabled"] == json!(true)).unwrap());
            }
            "signal" => self.bridge.on_signal(&step["signal"]),
            "gameState" => self.bridge.set_game_state(&step["state"]),
            "attachGame" => {
                *self.game_diagnostics.lock().unwrap() = step["diagnostics"].clone();
                let link = GameLink {
                    calls: self.game_calls.clone(),
                    diagnostics: self.game_diagnostics.clone(),
                };
                result = Some(json!(self.bridge.attach_game_presence(Arc::new(link))));
            }
            "diagnostics" => result = Some(self.bridge.diagnostics()),
            "shutdown" => self.bridge.shutdown(),
            "advance" => self.clock.advance(step["ms"].as_i64().unwrap()),
            "locale" => *self.locale.lock().unwrap() = step["value"].as_str().map(str::to_owned),
            "version" => *self.version.lock().unwrap() = step["value"].as_str().map(str::to_owned),
            "files" => result = Some(files(&self.user_data)),
            "settle" => self.quiesce().await,
            "wait" => {}
            "fire" => result = Some(json!({"fired": self.timers.fire(str_of(&step["timer"]))})),
            "double" => match str_of(&step["action"]) {
                "listen" => {
                    for id in items(&step["ids"]) {
                        self.double.listen(id.as_u64().unwrap() as u32).await;
                    }
                }
                "release" => self.double.release(str_of(&step["gate"])),
                "close" => self.double.close(
                    step["conn"].as_u64().unwrap() as usize,
                    step["polite"] == json!(true),
                ),
                "send" => self.double.send(
                    step["conn"].as_u64().unwrap() as usize,
                    step["frameOp"].as_i64().unwrap() as i32,
                    &step["data"],
                ),
                "behave" => self
                    .double
                    .behave(step["conn"].as_u64().unwrap() as usize, &step["patch"]),
                other => panic!("действие двойника {other}"),
            },
            other => panic!("шаг {other}"),
        }

        if let Some(until) = step.get("until") {
            let timeout = Duration::from_millis(step["timeoutMs"].as_u64().unwrap_or(5000));
            let deadline = Instant::now() + timeout;
            while !self.holds(until, base_in, base_events) {
                assert!(
                    Instant::now() < deadline,
                    "{}: не дождались {until} на шаге {op}; состояние {}, журнал {:?}",
                    self.name,
                    self.bridge.service().public_state().to_json(),
                    self.journal
                        .lock()
                        .unwrap()
                        .iter()
                        .rev()
                        .take(6)
                        .collect::<Vec<_>>(),
                );
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        self.drain_log();

        let mut out = Map::new();
        out.insert("op".into(), json!(op));
        if let Some(result) = result {
            out.insert("result".into(), result);
        }
        if op == "settle" {
            out.insert(
                "state".into(),
                self.bridge.service().public_state().to_json(),
            );
            out.insert("connected".into(), json!(self.bridge.service().connected()));
            out.insert("timers".into(), self.timers.describe());
            if !self.pending_log.is_empty() {
                out.insert(
                    "log".into(),
                    Value::Array(std::mem::take(&mut self.pending_log)),
                );
            }
            let journal = self.journal.lock().unwrap();
            if journal.len() > self.progress.journal {
                out.insert(
                    "double".into(),
                    Value::Array(journal[self.progress.journal..].to_vec()),
                );
            }
            self.progress.journal = journal.len();
            drop(journal);
            let registered = self.protocol.calls();
            if registered.len() > self.progress.registered {
                out.insert(
                    "register".into(),
                    json!(registered[self.progress.registered..]),
                );
            }
            self.progress.registered = registered.len();
            let window = self.window.lock().unwrap();
            if window.len() > self.progress.window {
                out.insert(
                    "window".into(),
                    Value::Array(window[self.progress.window..].to_vec()),
                );
            }
            self.progress.window = window.len();
            drop(window);
            let game = self.game_calls.lock().unwrap();
            if game.len() > self.progress.game {
                out.insert(
                    "game".into(),
                    Value::Array(game[self.progress.game..].to_vec()),
                );
            }
            self.progress.game = game.len();
        }
        Value::Object(out)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scenarios_match_node() {
    let fixture = fixture("discord-rpc-049.json");
    let now = fixture["now"].as_i64().unwrap();
    let mut mismatches = Mismatches::default();
    for scenario in items(&fixture["scenarios"]) {
        let root = tempfile::tempdir().unwrap();
        let mut run = Run::new(scenario, now, &root).await;
        let steps = items(&scenario["steps"]);
        let expected = items(&scenario["results"]);
        for (at, step) in steps.iter().enumerate() {
            let actual = run.step(step).await;
            mismatches.check(
                &actual,
                &expected[at],
                format!("{} · шаг {at} {}", run.name, step["op"]),
            );
        }
        run.bridge.shutdown();
        run.bridge.finish().await;
        run.double.stop();
    }
    mismatches.finish("служба Discord");
}

#[test]
fn ipc_paths_match_node() {
    let fixture = fixture("discord-rpc.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["paths"]) {
        let platform = match str_of(&case["platform"]) {
            "win32" => Platform::Windows,
            _ => Platform::Unix,
        };
        let env = case["env"].as_object().unwrap().clone();
        let lookup = |name: &str| env.get(name).and_then(Value::as_str).map(str::to_owned);
        let actual: Vec<Value> = (0..=ipc::LAST_ID)
            .map(|id| json!(ipc_path(platform, &lookup, id)))
            .collect();
        mismatches.check(
            &Value::Array(actual),
            &case["paths"],
            format!("{} {}", case["platform"], case["env"]),
        );
    }
    mismatches.finish("адреса IPC");
}

#[test]
fn frames_match_node() {
    let fixture = fixture("discord-rpc.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["frames"]) {
        let op = case["op"].as_i64().unwrap() as i32;
        let data = match case.get("dataRepeat") {
            Some(repeat) => json!(str_of(&repeat[0]).repeat(repeat[1].as_u64().unwrap() as usize)),
            None => case["data"].clone(),
        };
        let packet = ipc::encode(op, &data);
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let actual = if case.get("hex").is_some() {
            json!({"hex": hex(&packet)})
        } else {
            json!({"length": packet.len(), "header": hex(&packet[..8]),
                "sha256": hex(&Sha256::digest(&packet))})
        };
        let expected = if case.get("hex").is_some() {
            json!({"hex": case["hex"]})
        } else {
            json!({"length": case["length"], "header": case["header"], "sha256": case["sha256"]})
        };
        mismatches.check(&actual, &expected, format!("кадр {op} {data:.40}"));
    }
    mismatches.finish("кадры IPC");
}

#[test]
fn uuid_matches_node() {
    let fixture = fixture("discord-rpc.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["uuid"]) {
        let random: Random = sequence_random(
            items(&case["random"])
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect(),
        );
        let actual = json!([uuid4122(&random), uuid4122(&random)]);
        mismatches.check(&actual, &case["uuid"], format!("{}", case["random"]));
    }
    mismatches.finish("uuid");
}

#[test]
fn register_asks_for_discord_protocol() {
    let fixture = fixture("discord-rpc.json");
    let client = FakeProtocolClient::new(true);
    assert!(register(&client, "123"));
    assert_eq!(json!(client.calls()), fixture["registerCalls"]);
    assert!(!register(&FakeProtocolClient::new(false), "123"));
}

/// Активность из объекта JS (поля как у аргумента `setActivity`).
fn activity_from_js(object: &Value) -> Activity {
    let text = |key: &str| object.get(key).and_then(Value::as_str);
    Activity {
        details: text("details").unwrap_or_default().to_owned(),
        state: text("state").map(str::to_owned),
        large_image_key: leak(text("largeImageKey").unwrap_or_default()),
        large_image_text: text("largeImageText").unwrap_or_default().to_owned(),
        instance: object["instance"] == json!(true),
        small_image_key: text("smallImageKey").map(leak),
        small_image_text: text("smallImageText").map(leak),
        start_timestamp: object.get("startTimestamp").and_then(Value::as_f64),
        buttons: object
            .get("buttons")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|button| Button {
                label: leak(str_of(&button["label"])),
                url: leak(str_of(&button["url"])),
            })
            .collect(),
    }
}

#[test]
fn set_activity_frames_match_node() {
    let fixture = fixture("discord-rpc.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["activities"]) {
        let activity = match case.get("state") {
            Some(state) => {
                let activity = presentation(state);
                mismatches.check(
                    &activity.to_json(),
                    &case["activity"],
                    format!("toActivity {}", case["state"]),
                );
                activity
            }
            None => activity_from_js(&case["activity"]),
        };
        let frame = &case["frame"];
        match (set_activity_args(&activity, 7), frame.get("throws")) {
            (Ok(mut args), None) => {
                args["pid"] = json!("<pid>");
                mismatches.check(
                    &args,
                    &frame["data"]["args"],
                    format!("кадр {}", case["activity"]),
                );
            }
            (Err(error), Some(throws)) => {
                mismatches.check(
                    &json!(error.message),
                    &throws["message"],
                    format!("ошибка {}", case["activity"]),
                );
            }
            (actual, expected) => panic!("{}: {actual:?} против {expected:?}", case["activity"]),
        }
    }
    mismatches.finish("кадры SET_ACTIVITY");
}

fn presentation(state: &Value) -> Activity {
    presence_model::to_activity(state, Some("0.5.0"))
}

#[test]
fn retry_delays_match_node() {
    let fixture = fixture("discord-rpc.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["retryDelays"]) {
        let attempt = case[0].as_u64().unwrap() as u32;
        let random = case[1].as_f64().unwrap();
        mismatches.check(
            &js_number(retry_delay_ms(attempt, random)),
            &case[2],
            format!("попытка {attempt}, random {random}"),
        );
    }
    mismatches.finish("задержки повтора");
}

#[test]
fn connection_timeout_is_ten_seconds() {
    assert_eq!(ipc::CONNECT_TIMEOUT, Duration::from_secs(10));
    assert_eq!(IpcConfig::system().connect_timeout, Duration::from_secs(10));
    let path = (IpcConfig::system().path)(3);
    if cfg!(windows) {
        assert_eq!(path, "\\\\?\\pipe\\discord-ipc-3");
    } else {
        assert!(path.ends_with("/discord-ipc-3"));
    }
}

// ----------------------------------------------------------- транспорт без службы

struct Link {
    double: DiscordDouble,
    journal: Journal,
    client: RpcClient,
    _root: tempfile::TempDir,
}

async fn link(connections: Value) -> Link {
    let root = tempfile::tempdir().unwrap();
    let index = SCENARIO.fetch_add(1, Ordering::SeqCst);
    let endpoint = Endpoint::new(
        root.path(),
        format!("hlsr-rust-link-{}-{index}-", std::process::id()),
    );
    let journal: Journal = Arc::default();
    let scenario = json!({"listen": [0], "connections": connections});
    let double = DiscordDouble::start(endpoint.clone(), &scenario, journal.clone()).await;
    let client = RpcClient::new(endpoint.ipc_config(Duration::from_secs(5)));
    Link {
        double,
        journal,
        client,
        _root: root,
    }
}

async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < deadline, "не дождались: {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn frames_in(journal: &Journal) -> Vec<Value> {
    journal
        .lock()
        .unwrap()
        .iter()
        .filter_map(|entry| entry.get("in").cloned())
        .collect()
}

/// В 0.4.7 второй кадр, пришедший в том же чтении сокета, теряется; здесь оба разбираются.
#[tokio::test]
async fn frames_glued_in_one_read_are_both_handled() {
    let link = link(json!([{}])).await;
    link.client.login("1531057982518071296").await.unwrap();

    let mut glued = ipc::encode(ipc::op::PING, &json!("first"));
    glued.extend(ipc::encode(ipc::op::PING, &json!("second")));
    link.double.send_raw(1, glued);

    until("два PONG", || frames_in(&link.journal).len() >= 3).await;
    let frames = frames_in(&link.journal);
    assert_eq!(frames[1]["json"], json!("first"));
    assert_eq!(frames[2]["json"], json!("second"));
    assert_eq!(frames[1]["name"], json!("PONG"));
}

/// Кадр, пришедший кусками, собирается.
#[tokio::test]
async fn frame_split_across_reads_is_assembled() {
    let link = link(json!([{}])).await;
    link.client.login("1531057982518071296").await.unwrap();

    let frame = ipc::encode(ipc::op::PING, &json!({"long": "x".repeat(300)}));
    link.double.send_raw(1, frame[..5].to_vec());
    tokio::time::sleep(Duration::from_millis(60)).await;
    link.double.send_raw(1, frame[5..100].to_vec());
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(frames_in(&link.journal).len(), 1, "кадр ещё неполон");
    link.double.send_raw(1, frame[100..].to_vec());

    until("PONG", || frames_in(&link.journal).len() >= 2).await;
    assert_eq!(
        frames_in(&link.journal)[1]["json"],
        json!({"long": "x".repeat(300)})
    );
}

/// `login` дважды — одно подключение (`_connectPromise`).
#[tokio::test]
async fn login_twice_connects_once() {
    let link = link(json!([{}])).await;
    let (first, second) = tokio::join!(
        link.client.login("1531057982518071296"),
        link.client.login("1531057982518071296")
    );
    first.unwrap();
    second.unwrap();
    let opens = link
        .journal
        .lock()
        .unwrap()
        .iter()
        .filter(|entry| entry["event"] == "open")
        .count();
    assert_eq!(opens, 1);
}

/// Ответ `ERROR` отклоняет запрос с `code` и текстом Discord.
#[tokio::test]
async fn error_reply_rejects_with_code_and_message() {
    let link =
        link(json!([{"activity": {"error": {"code": 4000, "message": "Invalid activity"}}}])).await;
    link.client.login("1531057982518071296").await.unwrap();
    let activity =
        presentation(&json!({"kind": "launcher", "sessionStartedAtMs": 1760000000123u64}));
    let error = link
        .client
        .set_activity(&activity)
        .unwrap()
        .await
        .unwrap_err();
    assert_eq!(error.message, "Invalid activity");
    assert_eq!(error.code, Some(json!(4000)));
}

/// `nonce` — строка `uuid4122`, `pid` — процесс.
#[tokio::test]
async fn request_carries_nonce_and_pid() {
    let link = link(json!([{"activity": "silent"}])).await;
    link.client.login("1531057982518071296").await.unwrap();
    let activity = presentation(&json!({"kind": "launcher"}));
    let _pending = link.client.set_activity(&activity).unwrap();
    until("SET_ACTIVITY", || frames_in(&link.journal).len() >= 2).await;
    let frame = frames_in(&link.journal)[1].clone();
    assert_eq!(frame["json"]["cmd"], "SET_ACTIVITY");
    assert_eq!(frame["json"]["nonce"], "<nonce:1>");
    assert_eq!(frame["json"]["args"]["pid"], "<pid>");
}

#[test]
fn nonce_looks_like_node_uuid() {
    let random = hlsr_delivery::timing::system_random();
    for _ in 0..200 {
        let nonce = uuid4122(&random);
        let parts: Vec<&str> = nonce.split('-').collect();
        assert_eq!(
            parts.iter().map(|part| part.len()).collect::<Vec<_>>(),
            [8, 4, 4, 4, 12]
        );
        assert!(nonce.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
        assert_eq!(&parts[2][..1], "4");
        assert!("0123".contains(&parts[3][..1]));
    }
}

/// Без подключения `destroy` ничего не делает и не виснет.
#[tokio::test]
async fn destroy_without_connection_returns() {
    let root = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::new(
        root.path(),
        format!("hlsr-rust-none-{}-", std::process::id()),
    );
    let client = RpcClient::new(endpoint.ipc_config(Duration::from_secs(1)));
    tokio::time::timeout(Duration::from_secs(1), client.destroy())
        .await
        .expect("destroy вернулся");
}

/// Нет ни одного сервера — `login` отказывает "Could not connect".
#[tokio::test]
async fn login_without_server_fails() {
    let root = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::new(
        root.path(),
        format!("hlsr-rust-none-{}-", std::process::id()),
    );
    let client = RpcClient::new(endpoint.ipc_config(Duration::from_secs(5)));
    let error = client.login("1531057982518071296").await.unwrap_err();
    assert_eq!(error.message, "Could not connect");
    assert_eq!(error.code, None);
}

/// Тайм-аут: сервер молчит — `RPC_CONNECTION_TIMEOUT`, соединение остаётся для `destroy`.
#[tokio::test]
async fn silent_server_times_out() {
    let root = tempfile::tempdir().unwrap();
    let index = SCENARIO.fetch_add(1, Ordering::SeqCst);
    let endpoint = Endpoint::new(
        root.path(),
        format!("hlsr-rust-quiet-{}-{index}-", std::process::id()),
    );
    let journal: Journal = Arc::default();
    let scenario = json!({"listen": [0], "connections": [{"handshake": "silent"}]});
    let double = DiscordDouble::start(endpoint.clone(), &scenario, journal.clone()).await;
    let client = RpcClient::new(endpoint.ipc_config(Duration::from_millis(300)));
    let started = Instant::now();
    let error = client.login("1531057982518071296").await.unwrap_err();
    assert_eq!(error.message, "RPC_CONNECTION_TIMEOUT");
    assert!(started.elapsed() >= Duration::from_millis(290));
    client.destroy().await;
    until("CLOSE и eof", || journal.lock().unwrap().len() >= 4).await;
    let entries = journal.lock().unwrap().clone();
    assert_eq!(entries[2]["in"]["name"], "CLOSE");
    assert_eq!(entries[3]["event"], "eof");
    drop(double);
}
