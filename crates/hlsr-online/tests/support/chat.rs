//! Прогон сценариев мостов чата против двойников: то же, что `runScenario` из
//! `tools/reference/online/chat-app/lib.js`. Поддельный мост пишет вызовы так же, как Node
//! (`imageArg`: короткая картинка — base64, длинная — SHA-256 и размер), шаги и их ожидание —
//! те же, итог — той же формы, чтобы сравнить с `expected` фикстуры.

#![allow(dead_code)]

use super::server::{Journal, Server, start_server};
use hlsr_delivery::ports::{ChatBridge, ChatConsumer, ChatMessage, MemoryLogger};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const IMAGE_INLINE_LIMIT: usize = 2800;

/// SHA-256 в шестнадцатеричной записи.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `imageArg(value)` из chat-app/lib.js.
pub fn image_arg(value: &str) -> Value {
    use base64::Engine;
    let valid = {
        let body = value.trim_end_matches('=');
        value.len() - body.len() <= 2
            && body
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
    };
    if !valid {
        return json!({ "raw": value });
    }
    if value.len() <= IMAGE_INLINE_LIMIT {
        return json!({ "rgba": value });
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value)
        .or_else(|_| {
            base64::engine::general_purpose::STANDARD_NO_PAD.decode(value.trim_end_matches('='))
        })
        .unwrap_or_default();
    json!({ "sha256": sha256_hex(&bytes), "bytes": bytes.len() })
}

/// Мост, который пишет вызовы (то, что получила бы игра).
pub struct FakeBridge {
    calls: Mutex<Vec<Value>>,
    refuse: HashSet<String>,
}

impl FakeBridge {
    pub fn new(refuse: &[String]) -> Arc<FakeBridge> {
        Arc::new(FakeBridge {
            calls: Mutex::default(),
            refuse: refuse.iter().cloned().collect(),
        })
    }

    pub fn calls(&self) -> Vec<Value> {
        self.calls.lock().unwrap().clone()
    }

    pub fn len(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    fn push(&self, call: Value) {
        self.calls.lock().unwrap().push(call);
    }
}

impl ChatBridge for FakeBridge {
    fn send_chat_line(&self, platform: &str, nick: &str, text: &str, b1: &str, b2: &str) -> bool {
        self.push(json!(["sendChatLine", platform, nick, text, b1, b2]));
        true
    }
    fn send_emote_ready(&self, name: &str, wide: u32, tall: u32, rgba: &str) -> bool {
        self.push(json!(["sendEmoteReady", name, wide, tall, image_arg(rgba)]));
        true
    }
    fn send_emote_begin(
        &self,
        name: &str,
        wide: u32,
        tall: u32,
        frames: u32,
        frame_ms: u32,
    ) -> bool {
        self.push(json!([
            "sendEmoteBegin",
            name,
            wide,
            tall,
            frames,
            frame_ms
        ]));
        true
    }
    fn send_emote_frame(&self, name: &str, index: u32, rgba: &str) -> bool {
        self.push(json!(["sendEmoteFrame", name, index, image_arg(rgba)]));
        true
    }
    fn send_emote_miss(&self, name: &str) -> bool {
        self.push(json!(["sendEmoteMiss", name]));
        true
    }
    fn send_badge_ready(&self, name: &str, wide: u32, tall: u32, rgba: &str) -> bool {
        self.push(json!(["sendBadgeReady", name, wide, tall, image_arg(rgba)]));
        !self.refuse.contains(name)
    }
}

/// Сообщение игры из JSON сценария (`protocol.parseLine` уже отработал); `null` — нет сообщения.
pub fn chat_message(message: &Value) -> Option<ChatMessage> {
    if message.is_null() {
        return None;
    }
    let text = |key: &str| message[key].as_str().unwrap_or_default().to_owned();
    Some(match message["type"].as_str().unwrap() {
        "emote.set" => ChatMessage::EmoteSet {
            session: text("session"),
            room_id: text("roomId"),
        },
        "emote.resolve" => ChatMessage::EmoteResolve {
            session: text("session"),
            name: text("name"),
            px: message["px"].as_u64().map(|px| px as u32),
        },
        "chat.subscribe" => ChatMessage::Subscribe {
            session: text("session"),
            platform: text("platform"),
            id: text("id"),
        },
        "chat.unsubscribe" => ChatMessage::Unsubscribe {
            session: text("session"),
        },
        other => panic!("неизвестное сообщение {other}"),
    })
}

/// Потребитель сценария и его `diagnostics()`.
pub struct Subject {
    pub consumer: Arc<dyn ChatConsumer>,
    pub diagnostics: Box<dyn Fn() -> Value + Send + Sync>,
}

/// Что получает `make`: журнал модуля, сеть с перенаправлением хостов на двойники.
pub struct Context {
    pub log: Arc<MemoryLogger>,
    pub http: hlsr_online::chat_http::ChatHttp,
    pub handle: tokio::runtime::Handle,
}

async fn wait_calls(bridge: &FakeBridge, from: usize, n: usize, limit: Duration) -> bool {
    let until = Instant::now() + limit;
    while bridge.len() - from < n {
        if Instant::now() > until {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    true
}

/// Прогнать сценарий; результат — того же вида, что `expected` фикстуры.
pub async fn run_scenario(scenario: &Value, make: impl FnOnce(Context) -> Subject) -> Value {
    let journal: Journal = Arc::default();
    let mut servers: HashMap<String, Server> = HashMap::new();
    let mut origins = Vec::new();
    for (host, routes) in scenario["routes"].as_object().into_iter().flatten() {
        let server = start_server(host, journal.clone()).await;
        server.set_routes(routes);
        origins.push((host.clone(), server.authority.clone()));
        servers.insert(host.clone(), server);
    }
    let mut http = hlsr_online::chat_http::ChatHttp::with_origins(origins);
    if let Some(ms) = scenario["timeoutMs"].as_u64() {
        http = http.with_timeout(Duration::from_millis(ms));
    }
    let log = Arc::new(MemoryLogger::default());
    let refuse: Vec<String> = scenario["falseFor"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|name| name.as_str().unwrap().to_owned())
        .collect();
    let bridge = FakeBridge::new(&refuse);
    let subject = make(Context {
        log: log.clone(),
        http,
        handle: tokio::runtime::Handle::current(),
    });
    if scenario["noBridge"].as_bool() != Some(true) {
        subject.consumer.attach_bridge(bridge.clone());
    }

    let mut lines: Vec<Value> = Vec::new();
    let mut steps = Vec::new();
    for step in scenario["steps"].as_array().unwrap() {
        let from = bridge.len();
        let mut result = Map::new();
        let mut timed_out = false;
        match step["op"].as_str().unwrap() {
            "message" => {
                if let Some(message) = chat_message(&step["message"]) {
                    subject.consumer.handle_message(&message);
                }
            }
            "ready" => subject.consumer.handle_ready(),
            "forget" => subject.consumer.forget(),
            "diag" => {
                result.insert("diag".to_owned(), (subject.diagnostics)());
            }
            "release" => {
                servers[step["host"].as_str().unwrap()].release(step["gate"].as_str().unwrap())
            }
            "wait" => {}
            other => panic!("неизвестный шаг {other}"),
        }
        let settle = if step["op"] == "wait" {
            json!({ "quietMs": step["ms"] })
        } else {
            step["settle"].clone()
        };
        if let Some(n) = settle["calls"].as_u64() {
            let limit = Duration::from_millis(settle["limitMs"].as_u64().unwrap_or(8000));
            timed_out = !wait_calls(&bridge, from, n as usize, limit).await;
            tokio::time::sleep(Duration::from_millis(
                settle["afterMs"].as_u64().unwrap_or(30),
            ))
            .await;
        } else if let Some(ms) = settle["quietMs"].as_u64() {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
        if timed_out {
            result.insert("timeout".to_owned(), json!(true));
        }
        result.insert(
            "calls".to_owned(),
            Value::Array(bridge.calls()[from..].to_vec()),
        );
        steps.push(Value::Object(result));
        for (level, text) in log.take() {
            lines.push(json!([level, text]));
        }
    }
    if let Some(ms) = scenario["tailMs"].as_u64() {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
    for (level, text) in log.take() {
        lines.push(json!([level, text]));
    }
    let used: usize = steps
        .iter()
        .map(|s| s["calls"].as_array().unwrap().len())
        .sum();
    let tail = bridge.calls()[used..].to_vec();
    let journal = journal.lock().unwrap().clone();
    json!({ "steps": steps, "tail": tail, "log": lines, "journal": journal })
}

/// Сравнить итог прогона с `expected` фикстуры; возвращает разницу или `None`. У сценариев с
/// `journalUnordered` запросы на разные соединения приходят в журнал двойника в любом порядке —
/// журналы сравниваются как наборы (по пути и запросу).
pub fn compare_run(scenario: &Value, actual: &Value) -> Option<String> {
    let expected = &scenario["expected"];
    let (mut actual, mut expected) = (actual.clone(), expected.clone());
    if scenario["journalUnordered"] == true {
        for run in [&mut actual, &mut expected] {
            if let Some(journal) = run["journal"].as_array_mut() {
                journal.sort_by_key(|entry| entry.to_string());
            }
        }
    }
    if actual == expected {
        return None;
    }
    Some(super::diff(&expected, &actual))
}
