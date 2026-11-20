//! Потребители чата у моста игры (`GamePresenceBridge.attachChatConsumer` 0.4.9): сообщения
//! `chat.*`/`emote.*` своего сеанса, рукопожатие, уход игры; строки чата, эмоуты и значки, которые
//! потребитель пишет через сторону моста, доходят до игры. TCP 127.0.0.1 (ветка Linux, идёт на
//! любой ОС).

use hlsr_core::js::Platform;
use hlsr_core::logger::MemoryLogger;
use hlsr_delivery::ports::{ChatBridge, ChatConsumer, ChatMessage};
use hlsr_platform::game_presence::{BridgeOptions, GamePresenceBridge};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<String>>,
    bridge: Mutex<Option<Arc<dyn ChatBridge>>>,
}

impl ChatConsumer for Recorder {
    fn attach_bridge(&self, bridge: Arc<dyn ChatBridge>) {
        *self.bridge.lock().unwrap() = Some(bridge);
    }
    fn handle_message(&self, message: &ChatMessage) {
        self.events.lock().unwrap().push(format!("{message:?}"));
        if let ChatMessage::EmoteResolve { name, .. } = message {
            let bridge = self.bridge.lock().unwrap().clone().unwrap();
            assert!(bridge.send_emote_miss(name));
            assert!(bridge.send_chat_line("twitch", "Runner", "привет", "tw:vip/1", ""));
        }
    }
    fn handle_ready(&self) {
        self.events.lock().unwrap().push("ready".to_owned());
    }
    fn forget(&self) {
        self.events.lock().unwrap().push("forget".to_owned());
    }
}

/// Сломанный потребитель не мешает остальным: его падение уходит в журнал.
struct Broken;

impl ChatConsumer for Broken {
    fn attach_bridge(&self, _bridge: Arc<dyn ChatBridge>) {}
    fn handle_message(&self, _message: &ChatMessage) {
        panic!("сломан");
    }
}

async fn wait_for(recorder: &Recorder, count: usize) -> Vec<String> {
    for _ in 0..500 {
        let events = recorder.events.lock().unwrap().clone();
        if events.len() >= count {
            return events;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    recorder.events.lock().unwrap().clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_consumers_hear_the_game_and_write_back_to_it() {
    let logger = Arc::new(MemoryLogger::default());
    let bridge = GamePresenceBridge::new(BridgeOptions {
        log: Some(logger.clone()),
        platform: Some(Platform::Linux),
        token: Some(TOKEN.to_owned()),
        ..BridgeOptions::default()
    });
    let recorder = Arc::new(Recorder::default());
    bridge.attach_chat_consumer(Arc::new(Broken));
    bridge.attach_chat_consumer(recorder.clone());
    bridge.start();
    // Пока игры нет, писать некуда.
    let side = recorder.bridge.lock().unwrap().clone().unwrap();
    assert!(!side.send_chat_line("twitch", "n", "t", "", ""));

    let port = bridge.server().port().unwrap();
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let send = |value: Value| format!("{value}\n");
    write
        .write_all(
            send(json!({"v": 1, "type": "hello", "token": TOKEN, "session": "0123abcd"}))
                .as_bytes(),
        )
        .await
        .unwrap();
    let ack: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(ack["type"], "discord");
    assert_eq!(wait_for(&recorder, 1).await, ["ready"]);

    let messages = [
        json!({"v": 1, "type": "chat.subscribe", "session": "0123abcd", "platform": "twitch", "id": "12345"}),
        // Чужой сеанс — мимо потребителей.
        json!({"v": 1, "type": "chat.unsubscribe", "session": "ffffffff"}),
        json!({"v": 1, "type": "emote.set", "session": "0123abcd", "roomId": "twitch:12345"}),
        json!({"v": 1, "type": "emote.resolve", "session": "0123abcd", "name": "Kappa", "px": 28}),
    ];
    for message in messages {
        write.write_all(send(message).as_bytes()).await.unwrap();
    }
    let miss: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(miss, json!({"v": 1, "type": "emote.miss", "name": "Kappa"}));
    let chat: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(
        chat,
        json!({"v": 1, "type": "chat.line", "platform": "twitch", "badge1": "tw:vip/1", "badge2": "", "nick": "Runner", "text": "привет"})
    );

    drop(write);
    drop(lines);
    let events = wait_for(&recorder, 5).await;
    assert_eq!(
        events,
        [
            "ready".to_owned(),
            format!(
                "{:?}",
                ChatMessage::Subscribe {
                    session: "0123abcd".into(),
                    platform: "twitch".into(),
                    id: "12345".into()
                }
            ),
            format!(
                "{:?}",
                ChatMessage::EmoteSet {
                    session: "0123abcd".into(),
                    room_id: "twitch:12345".into()
                }
            ),
            format!(
                "{:?}",
                ChatMessage::EmoteResolve {
                    session: "0123abcd".into(),
                    name: "Kappa".into(),
                    px: Some(28)
                }
            ),
            "forget".to_owned(),
        ]
    );
    let warnings: Vec<String> = logger
        .take()
        .into_iter()
        .filter(|(level, _)| *level == "warn")
        .map(|(_, text)| text)
        .collect();
    assert_eq!(
        warnings,
        ["[game-presence] потребитель чата упал: сломан"; 3]
    );
    bridge.shutdown();
}
