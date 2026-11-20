//! Сервер моста игры против 0.4.7: сценарии по настоящим сокетам
//! (`tools/reference/platform/presence-server.js` → `presence-server.json`) и самотесты
//! `presence-endpoint-selftest.js` / `linux-selftest.js`, которые касаются сервера. Linux —
//! TCP 127.0.0.1 (идёт на любой ОС), Windows — именованный канал (только на Windows).

mod support;

use base64::Engine as _;
use hlsr_core::js::Platform;
use hlsr_core::logger::MemoryLogger;
use hlsr_delivery::ports::ChatMessage;
use hlsr_platform::game_presence::presence_bridge_server::{
    Handlers, Limits, PresenceBridgeServer, ServerOptions,
};
use hlsr_platform::game_presence::protocol::Message;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::{Mismatches, fixture, items, str_of};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, WriteHalf};
use tokio::task::JoinHandle;

const SETTLE: Duration = Duration::from_millis(90);
/// Сколько раз по 10 мс ждать записанное 0.4.7 после `SETTLE`: предел нужен только затем, чтобы
/// сорванный шаг не завис, а не как срок, в который сервер обязан ответить.
const CONVERGE_STEPS: u32 = 1000;
/// Во сколько раз растянуты сроки сценариев с настоящими таймерами сервера (тайм-ауты рукопожатия и
/// простоя) и паузы `wait` в них: отношения те же, что в записи 0.4.7, а запас между сообщением
/// и сроком в 0,12-0,2 с становится втрое больше и под нагрузкой машины не кончается.
const TIMER_SCALE: u64 = 3;
const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

trait Duplex: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Duplex for T {}
type Stream = Box<dyn Duplex>;

/// Клиент игры: строки, которые он получил, и закрытие связи.
struct Client {
    writer: Option<WriteHalf<Stream>>,
    received: Arc<Mutex<Vec<Value>>>,
    closed: Arc<AtomicBool>,
    reader: JoinHandle<()>,
    seen_received: usize,
    seen_closed: bool,
}

impl Client {
    fn spawn(stream: Stream) -> Client {
        let (mut read, write) = tokio::io::split(stream);
        let received = Arc::new(Mutex::new(Vec::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let reader = {
            let received = received.clone();
            let closed = closed.clone();
            tokio::spawn(async move {
                let mut pending: Vec<u8> = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    match read.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(count) => {
                            pending.extend_from_slice(&chunk[..count]);
                            while let Some(index) = pending.iter().position(|b| *b == b'\n') {
                                let line: Vec<u8> = pending.drain(..=index).collect();
                                let text = String::from_utf8_lossy(&line[..line.len() - 1]);
                                received.lock().unwrap().push(
                                    serde_json::from_str(&text).expect("строка сервера — JSON"),
                                );
                            }
                        }
                    }
                }
                closed.store(true, Ordering::SeqCst);
            })
        };
        Client {
            writer: Some(write),
            received,
            closed,
            reader,
            seen_received: 0,
            seen_closed: false,
        }
    }

    async fn write(&mut self, bytes: &[u8]) {
        if let Some(writer) = &mut self.writer {
            // Сервер мог уже закрыть связь: как в Node, ошибка записи не важна.
            let _ = writer.write_all(bytes).await;
        }
    }

    /// `socket.destroy()` клиента.
    fn destroy(&mut self) {
        self.reader.abort();
        self.writer = None;
        self.closed.store(true, Ordering::SeqCst);
    }
}

async fn connect(platform: Platform, server: &PresenceBridgeServer, pipe_name: &str) -> Client {
    if platform == Platform::Win32 {
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ClientOptions;
            for _ in 0..200 {
                match ClientOptions::new().open(pipe_name) {
                    Ok(pipe) => return Client::spawn(Box::new(pipe)),
                    // ERROR_PIPE_BUSY: сервер ещё готовит следующий экземпляр канала.
                    Err(error) if error.raw_os_error() == Some(231) => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(error) => panic!("канал {pipe_name} не открылся: {error}"),
                }
            }
            panic!("канал {pipe_name} занят");
        }
        #[cfg(not(windows))]
        {
            let _ = (server, pipe_name);
            panic!("именованный канал есть только в Windows");
        }
    }
    let port = server.port().expect("сервер слушает порт");
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("подключение к мосту");
    Client::spawn(Box::new(stream))
}

/// Закодированное в метку значение: адрес канала и порт заменены, как в `mask` скрипта.
fn mask(value: Value, pipe_name: &str, port: Option<u16>) -> Value {
    let mut text = serde_json::to_string(&value).unwrap();
    let escaped = serde_json::to_string(pipe_name).unwrap();
    text = text.replace(&escaped[1..escaped.len() - 1], "<pipe>");
    if let Some(port) = port {
        text = text.replace(&format!("\"{port}\""), "\"<port>\"");
    }
    serde_json::from_str(&text).unwrap()
}

/// Что увидели клиенты за шаг: полученные строки и закрытие. `consume` — считать увиденное
/// прошлым (следующий шаг начнёт с новых строк), иначе только посмотреть.
fn observe(clients: &mut [(String, Client)], consume: bool) -> serde_json::Map<String, Value> {
    let mut seen = serde_json::Map::new();
    for (key, client) in clients {
        let all = client.received.lock().unwrap().clone();
        let received: Vec<Value> = all[client.seen_received..].to_vec();
        let now_closed = client.closed.load(Ordering::SeqCst);
        let closed = now_closed && !client.seen_closed;
        if consume {
            client.seen_received = all.len();
            if now_closed {
                client.seen_closed = true;
            }
        }
        if !received.is_empty() || closed {
            seen.insert(key.clone(), json!({"received": received, "closed": closed}));
        }
    }
    seen
}

async fn run_scenario(platform: Platform, scenario: &Value, mismatches: &mut Mismatches) {
    let name = str_of(&scenario["name"]);
    let context = |step: usize, what: &str| format!("{platform:?} «{name}», шаг {step}: {what}");
    let pipe_name = {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).unwrap();
        let hex: String = random.iter().map(|b| format!("{b:02x}")).collect();
        format!("\\\\.\\pipe\\hlsr-presence-{hex}")
    };
    let clock = Arc::new(AtomicI64::new(1_000_000));
    let events: Arc<Mutex<Vec<Value>>> = Arc::default();
    let logger = Arc::new(MemoryLogger::default());
    let mut limits = Limits::default();
    let mut scale = 1;
    if let Some(custom) = scenario["limits"].as_object() {
        scale = TIMER_SCALE;
        if let Some(ms) = custom.get("handshakeTimeoutMs").and_then(Value::as_u64) {
            limits.handshake_timeout_ms = ms * scale;
        }
        if let Some(ms) = custom.get("idleTimeoutMs").and_then(Value::as_u64) {
            limits.idle_timeout_ms = ms * scale;
        }
    }
    let server = PresenceBridgeServer::new(ServerOptions {
        log: Some(logger.clone()),
        now: Some({
            let clock = clock.clone();
            Arc::new(move || clock.load(Ordering::SeqCst))
        }),
        limits: Some(limits),
        platform: Some(platform),
        token: Some(TOKEN.to_owned()),
        pipe_name: Some(pipe_name.clone()),
        handlers: Some(Handlers {
            on_state: Some({
                let events = events.clone();
                Arc::new(move |message| {
                    events.lock().unwrap().push(json!({
                        "kind": "state",
                        "message": Message::State(message.clone()).to_value(),
                    }));
                })
            }),
            on_game_gone: Some({
                let events = events.clone();
                Arc::new(move |reason| {
                    events
                        .lock()
                        .unwrap()
                        .push(json!({"kind": "gone", "reason": reason}));
                })
            }),
            on_game_ready: Some({
                let events = events.clone();
                Arc::new(move || events.lock().unwrap().push(json!({"kind": "ready"})))
            }),
            on_chat: Some({
                let events = events.clone();
                Arc::new(move |message| {
                    events
                        .lock()
                        .unwrap()
                        .push(json!({"kind": "chat", "message": chat_value(message)}));
                })
            }),
        }),
        ..ServerOptions::default()
    });

    let mut clients: Vec<(String, Client)> = Vec::new();
    let mut patient = true;
    for (index, recorded) in items(&scenario["steps"]).iter().enumerate() {
        let step = &recorded["step"];
        let client_name = step["client"].as_str();
        let mut result = Value::Null;
        match str_of(&step["op"]) {
            "start" => {
                result = json!(if server.start() == pipe_name {
                    "pipeName"
                } else {
                    "other"
                });
                tokio::time::sleep(SETTLE).await;
            }
            "stop" => server.stop(),
            "connect" => {
                let client = connect(platform, &server, &pipe_name).await;
                let key = client_name.unwrap().to_owned();
                clients.retain(|(existing, _)| *existing != key);
                clients.push((key, client));
            }
            "send" | "sendBytes" | "burst" | "close" => {
                let client = &mut clients
                    .iter_mut()
                    .find(|(key, _)| Some(key.as_str()) == client_name)
                    .expect("клиент сценария")
                    .1;
                match str_of(&step["op"]) {
                    "send" => {
                        let text = match step["text"].as_str() {
                            Some(text) => text.to_owned(),
                            None => format!("{}\n", serde_json::to_string(&step["line"]).unwrap()),
                        };
                        client.write(text.as_bytes()).await;
                    }
                    "sendBytes" => {
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(str_of(&step["base64"]))
                            .unwrap();
                        client.write(&bytes).await;
                    }
                    "burst" => {
                        let from = step["from"].as_u64().unwrap_or(0);
                        let mut text = String::new();
                        for i in 0..step["count"].as_u64().unwrap() {
                            text.push_str(&format!(
                                "{{\"v\":1,\"type\":\"state\",\"session\":\"0123abcd\",\"seq\":{},\"lifecycle\":\"menu\",\"mapName\":null,\"fps\":60,\"timerRunning\":false,\"timerCompleted\":false,\"timerElapsedMilliseconds\":null}}\n",
                                from + i + 1
                            ));
                        }
                        client.write(text.as_bytes()).await;
                    }
                    _ => client.destroy(),
                }
            }
            "clock" => match step["set"].as_i64() {
                Some(set) => clock.store(set, Ordering::SeqCst),
                None => {
                    clock.fetch_add(step["add"].as_i64().unwrap(), Ordering::SeqCst);
                }
            },
            "wait" => {
                tokio::time::sleep(Duration::from_millis(step["ms"].as_u64().unwrap() * scale))
                    .await
            }
            "discord" => result = json!(server.set_discord_connected(step["value"] == json!(true))),
            "inspect" => {
                let port = server.port();
                let environment: serde_json::Map<String, Value> = server
                    .environment()
                    .into_iter()
                    .map(|(key, value)| (key, Value::String(value)))
                    .collect();
                result = json!({
                    "environment": mask(Value::Object(environment), &pipe_name, port),
                    "endpointKeys": server.endpoint().to_value().as_object().unwrap().keys().collect::<Vec<_>>(),
                    "hasPort": server.endpoint().port.is_some(),
                    "port": if platform == Platform::Win32 {
                        port.map_or(Value::Null, Value::from)
                    } else {
                        json!(if port.is_some() { "number" } else { "object" })
                    },
                });
            }
            other => panic!("неизвестный шаг {other}"),
        }
        if str_of(&step["op"]) != "wait" {
            tokio::time::sleep(SETTLE).await;
        }

        // Что произошло за шаг. Пауза `SETTLE` нужна, чтобы увидеть лишнее; а то, что ожидает
        // запись 0.4.7, под нагрузкой может прийти позже, поэтому недостающее ждём как условие
        // (до `CONVERGE_STEPS` шагов), а не как время. Сорвавшийся шаг дальше не ждут:
        // расхождение и так будет в отчёте.
        let expected_events: Vec<Value> = items(&recorded["events"]).to_vec();
        let expected_logs = strings(&recorded["logs"]);
        let mut taken: Vec<Value> = Vec::new();
        let mut logs: Vec<String> = Vec::new();
        let mut waited = 0;
        loop {
            taken.extend(std::mem::take(&mut *events.lock().unwrap()));
            logs.extend(
                logger
                    .take()
                    .into_iter()
                    .map(|(level, text)| format!("{level}: {text}")),
            );
            let matches = taken == expected_events
                && logs == expected_logs
                && Value::Object(observe(&mut clients, false)) == recorded["clients"]
                && server.diagnostics().to_value() == recorded["diagnostics"];
            if matches || !patient || waited >= CONVERGE_STEPS {
                patient &= matches;
                break;
            }
            waited += 1;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let seen = observe(&mut clients, true);

        // Некоторые шаги — только запись; результат есть лишь у части.
        if !recorded["result"].is_null() || !result.is_null() {
            mismatches.check(&result, &recorded["result"], context(index, "результат"));
        }
        mismatches.check(
            &Value::Array(taken),
            &Value::Array(expected_events),
            context(index, "события"),
        );
        mismatches.check(
            &Value::Object(seen),
            &recorded["clients"],
            context(index, "клиенты"),
        );
        mismatches.check_eq(logs, expected_logs, context(index, "журнал"));
        mismatches.check(
            &server.diagnostics().to_value(),
            &recorded["diagnostics"],
            context(index, "диагностика"),
        );
    }

    for (_, client) in &mut clients {
        client.destroy();
    }
    server.stop();
    tokio::time::sleep(Duration::from_millis(20)).await;
}

fn strings(value: &Value) -> Vec<String> {
    items(value).iter().map(|v| str_of(v).to_owned()).collect()
}

async fn replay(platform: Platform, key: &str) {
    let fixture = fixture("presence-server.json");
    let defaults = &fixture["defaults"];
    let limits = Limits::default();
    assert_eq!(
        defaults["handshakeTimeoutMs"].as_u64(),
        Some(limits.handshake_timeout_ms)
    );
    assert_eq!(
        defaults["idleTimeoutMs"].as_u64(),
        Some(limits.idle_timeout_ms)
    );
    assert_eq!(
        defaults["maxMessagesPerSecond"].as_u64(),
        Some(u64::from(limits.max_messages_per_second))
    );
    assert_eq!(
        defaults["maxLineBytes"].as_u64(),
        Some(limits.max_line_bytes as u64)
    );

    let mut mismatches = Mismatches::default();
    let mut count = 0;
    for scenario in items(&fixture["scenarios"]) {
        if str_of(&scenario["platform"]) == key {
            count += 1;
            run_scenario(platform, scenario, &mut mismatches).await;
        }
    }
    assert!(count >= 15, "сценариев {key}: {count}");
    mismatches.finish(&format!("сервер моста ({key})"));
}

/// Вне среды tokio и без `options.runtime` сервер не молчит, а называет причину.
#[test]
#[should_panic(expected = "нужна среда tokio")]
fn a_server_without_a_tokio_runtime_says_so() {
    let _ = PresenceBridgeServer::new(ServerOptions::default());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tcp_scenarios_like_0_4_7() {
    replay(Platform::Linux, "linux").await;
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn named_pipe_scenarios_like_0_4_7() {
    replay(Platform::Win32, "win32").await;
}

// --- presence-endpoint-selftest.js и linux-selftest.js

fn quiet() -> Arc<MemoryLogger> {
    Arc::new(MemoryLogger::default())
}

/// Пока игра не получила строку, ждём её как условие; предел (30 с) нужен только затем, чтобы
/// сорванный тест не завис.
async fn read_line(client: &mut Client) -> Value {
    for _ in 0..3000 {
        {
            let received = client.received.lock().unwrap();
            if received.len() > client.seen_received {
                let line = received[client.seen_received].clone();
                drop(received);
                client.seen_received += 1;
                return line;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("игра не получила строку");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_raises_the_same_channel_token_and_port() {
    let first = PresenceBridgeServer::new(ServerOptions {
        log: Some(quiet()),
        platform: Some(Platform::Linux),
        ..ServerOptions::default()
    });
    first.start();
    let saved = first.endpoint();
    assert!(saved.port.is_some());
    first.stop();
    // Порт освобождается не сразу после `stop()`: ждём, пока его можно занять, а не время.
    let port = saved.port.expect("порт канала");
    for _ in 0..3000 {
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let second = PresenceBridgeServer::new(ServerOptions {
        log: Some(quiet()),
        platform: Some(Platform::Linux),
        token: Some(saved.token.clone()),
        pipe_name: Some(saved.pipe_name.clone()),
        port: saved.port,
        ..ServerOptions::default()
    });
    second.start();
    assert_eq!(second.endpoint(), saved);
    assert!(
        second
            .environment()
            .contains(&("HLSR_PRESENCE_TOKEN".to_owned(), saved.token.clone()))
    );
    second.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_busy_old_port_is_replaced_by_a_new_one() {
    let blocker = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let busy = blocker.local_addr().unwrap().port();
    let logger = quiet();
    let server = PresenceBridgeServer::new(ServerOptions {
        log: Some(logger.clone()),
        platform: Some(Platform::Linux),
        port: Some(busy),
        ..ServerOptions::default()
    });
    server.start();
    let port = server.port().expect("сервер взял новый порт");
    assert_ne!(port, busy);
    assert!(port > 0);
    assert_eq!(
        logger.take(),
        vec![(
            "warn",
            format!("[game-presence] порт {busy} занят, беру новый")
        )]
    );
    server.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn linux_channel_listens_on_loopback_only_and_a_stranger_does_not_displace_the_game() {
    let token = "a".repeat(64);
    let server = PresenceBridgeServer::new(ServerOptions {
        log: Some(quiet()),
        platform: Some(Platform::Linux),
        token: Some(token.clone()),
        ..ServerOptions::default()
    });
    let pipe_name = server.start();
    assert!(
        pipe_name.starts_with("\\\\.\\pipe\\hlsr-presence-")
            && pipe_name.len() == "\\\\.\\pipe\\hlsr-presence-".len() + 16
    );
    assert_eq!(
        server
            .listening_address()
            .map(|address| address.ip().to_string()),
        Some("127.0.0.1".to_owned())
    );
    let port = server.port().unwrap();
    assert_eq!(
        server.environment(),
        vec![
            ("HLSR_PRESENCE_PIPE".to_owned(), pipe_name.clone()),
            ("HLSR_PRESENCE_TOKEN".to_owned(), token.clone()),
            ("HLSR_PRESENCE_PORT".to_owned(), port.to_string()),
        ]
    );

    let mut game = connect(Platform::Linux, &server, &pipe_name).await;
    game.write(
        format!(
            "{}\n",
            json!({"v": 1, "type": "hello", "token": token, "session": "0123abcd"})
        )
        .as_bytes(),
    )
    .await;
    assert_eq!(
        read_line(&mut game).await,
        json!({"v": 1, "type": "discord", "connected": false})
    );
    assert!(server.diagnostics().connected);

    // Пока игра на связи, чужое соединение её не вытесняет.
    let stranger = connect(Platform::Linux, &server, &pipe_name).await;
    for _ in 0..3000 {
        if stranger.closed.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        stranger.closed.load(Ordering::SeqCst),
        "чужое соединение закрыто"
    );
    assert!(server.diagnostics().connected);
    assert!(server.set_discord_connected(true));
    assert_eq!(
        read_line(&mut game).await,
        json!({"v": 1, "type": "discord", "connected": true})
    );
    server.stop();
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn windows_channel_listens_and_gives_the_game_its_environment() {
    // Имя канала у каждого процесса своё: два одновременных запуска тестов не делят его.
    let pipe_name = format!("\\\\.\\pipe\\hlsr-presence-{:016x}", std::process::id());
    let server = PresenceBridgeServer::new(ServerOptions {
        log: Some(quiet()),
        platform: Some(Platform::Win32),
        pipe_name: Some(pipe_name.clone()),
        token: Some("b".repeat(64)),
        ..ServerOptions::default()
    });
    assert!(server.environment().is_empty(), "до запуска окружения нет");
    assert_eq!(server.start(), pipe_name);
    assert_eq!(
        server.environment(),
        vec![
            ("HLSR_PRESENCE_PIPE".to_owned(), pipe_name.clone()),
            ("HLSR_PRESENCE_TOKEN".to_owned(), "b".repeat(64)),
        ]
    );
    assert_eq!(server.port(), None);
    // Канал настоящий: игра подключается и представляется.
    let mut game = connect(Platform::Win32, &server, &pipe_name).await;
    game.write(
        format!(
            "{}\n",
            json!({"v": 1, "type": "hello", "token": "b".repeat(64), "session": "0123abcd"})
        )
        .as_bytes(),
    )
    .await;
    assert_eq!(
        read_line(&mut game).await,
        json!({"v": 1, "type": "discord", "connected": false})
    );
    server.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panicking_handler_is_logged_and_the_connection_survives() {
    let logger = quiet();
    let server = PresenceBridgeServer::new(ServerOptions {
        log: Some(logger.clone()),
        platform: Some(Platform::Linux),
        token: Some(TOKEN.to_owned()),
        handlers: Some(Handlers {
            on_state: Some(Arc::new(|_| panic!("состояние не принято"))),
            ..Handlers::default()
        }),
        ..ServerOptions::default()
    });
    let pipe_name = server.start();
    let mut game = connect(Platform::Linux, &server, &pipe_name).await;
    let hello = json!({"v": 1, "type": "hello", "token": TOKEN, "session": "0123abcd"});
    let state = json!({
        "v": 1, "type": "state", "session": "0123abcd", "seq": 1, "lifecycle": "menu",
        "mapName": null, "fps": 60, "timerRunning": false, "timerCompleted": false,
        "timerElapsedMilliseconds": null
    });
    game.write(format!("{hello}\n{state}\n").as_bytes()).await;
    read_line(&mut game).await;
    tokio::time::sleep(SETTLE).await;
    assert!(server.diagnostics().connected);
    assert_eq!(
        logger.take(),
        vec![(
            "warn",
            "[game-presence] обработчик состояния упал: состояние не принято".to_owned()
        )]
    );
    server.stop();
}

/// Сообщение чата так, как его получает `onChat` 0.4.9 (объект `parseLine`).
fn chat_value(message: &ChatMessage) -> Value {
    match message {
        ChatMessage::Subscribe {
            session,
            platform,
            id,
        } => json!({"type": "chat.subscribe", "session": session, "platform": platform, "id": id}),
        ChatMessage::Unsubscribe { session } => {
            json!({"type": "chat.unsubscribe", "session": session})
        }
        ChatMessage::EmoteSet { session, room_id } => {
            json!({"type": "emote.set", "session": session, "roomId": room_id})
        }
        ChatMessage::EmoteResolve { session, name, px } => {
            json!({"type": "emote.resolve", "session": session, "name": name, "px": px})
        }
    }
}
