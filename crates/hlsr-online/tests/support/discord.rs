//! Двойник Discord для сверки онлайна: локальный сервер IPC — именованный канал (Windows) или
//! Unix-сокет, — который ведёт сценарий и журнал кадров. Тот же формат сценария и журнала у Node —
//! `tools/reference/online/lib/discord-double.js` (описание формата — в его заголовке): служба
//! 0.4.7 с настоящим npm `discord-rpc` и служба Rust гоняют одни сценарии, а потом сравнивают
//! журналы.
//!
//! Настоящий Discord двойник не касается: адрес — свой каталог (Unix) или своё имя канала
//! с уникальным префиксом (Windows), клиент получает его через `IpcConfig`.
//!
//! Закрытие. Unix: `socket.end()` — половинное закрытие (FIN), собеседник отвечает своим.
//! Именованный канал половинного закрытия не знает: Node закрывает его целиком (проверено на
//! Node 24: `end()` с любой стороны даёт `end` и `close` у обеих), и `eof` в журнале означает
//! «клиент закрыл свою сторону». Клиент Rust на канале Windows сам не закрывает его, а ждёт,
//! пока Discord закроет канал в ответ на кадр `CLOSE`; поэтому двойник на Windows закрывает
//! канал, получив `CLOSE`, и ставит запись `eof` — как это видно в журнале Node. То же при
//! вежливом закрытии со стороны двойника: канал закрывается, `eof` ставится сразу.

#![allow(dead_code)]

use hlsr_online::discord_rpc::rpc::ipc::{self, IpcConfig};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

/// Журнал двойника.
pub type Journal = Arc<Mutex<Vec<Value>>>;

const OP_NAMES: [&str; 5] = ["HANDSHAKE", "FRAME", "CLOSE", "PING", "PONG"];

/// Где двойник слушает: каталог сокетов (Unix) или префикс имён каналов (Windows).
#[derive(Clone)]
pub struct Endpoint {
    dir: PathBuf,
    tag: String,
}

impl Endpoint {
    /// `dir` — каталог для сокетов Unix; `tag` — уникальный префикс имён каналов Windows.
    pub fn new(dir: impl Into<PathBuf>, tag: impl Into<String>) -> Endpoint {
        Endpoint {
            dir: dir.into(),
            tag: tag.into(),
        }
    }

    /// Адрес `discord-ipc-<id>` двойника.
    pub fn address(&self, id: u32) -> String {
        if cfg!(windows) {
            format!("\\\\?\\pipe\\{}discord-ipc-{id}", self.tag)
        } else {
            format!("{}/discord-ipc-{id}", self.dir.display())
        }
    }

    /// Адреса для клиента; `connect_timeout` — как в `RPC_CONNECTION_TIMEOUT`.
    pub fn ipc_config(&self, connect_timeout: std::time::Duration) -> IpcConfig {
        let endpoint = self.clone();
        IpcConfig {
            path: Arc::new(move |id| endpoint.address(id)),
            connect_timeout,
        }
    }
}

enum Cmd {
    Write(Vec<u8>),
    /// `socket.end()` (вежливо) или `socket.destroy()`.
    Close {
        polite: bool,
    },
}

struct Conn {
    number: usize,
    commands: mpsc::UnboundedSender<Cmd>,
    behavior: Mutex<Value>,
}

struct Shared {
    scenario: Value,
    journal: Journal,
    conns: Mutex<Vec<Arc<Conn>>>,
    nonces: Mutex<Vec<String>>,
    gates: Mutex<HashMap<String, watch::Sender<bool>>>,
}

impl Shared {
    fn gate(&self, name: &str) -> watch::Sender<bool> {
        self.gates
            .lock()
            .unwrap()
            .entry(name.to_owned())
            .or_insert_with(|| watch::channel(false).0)
            .clone()
    }

    fn nonce_of(&self, raw: &str) -> String {
        let mut nonces = self.nonces.lock().unwrap();
        let at = nonces
            .iter()
            .position(|seen| seen == raw)
            .unwrap_or_else(|| {
                nonces.push(raw.to_owned());
                nonces.len() - 1
            });
        format!("<nonce:{}>", at + 1)
    }

    fn normalized(&self, json: &Value) -> Value {
        let Some(object) = json.as_object() else {
            return json.clone();
        };
        let mut copy = object.clone();
        if let Some(Value::String(nonce)) = copy.get("nonce").cloned() {
            copy.insert("nonce".into(), json!(self.nonce_of(&nonce)));
        }
        if let Some(args) = copy.get("args").and_then(Value::as_object)
            && args.get("pid").is_some_and(Value::is_number)
        {
            let mut args = args.clone();
            args.insert("pid".into(), json!("<pid>"));
            copy.insert("args".into(), Value::Object(args));
        }
        Value::Object(copy)
    }

    fn log(&self, entry: Value) {
        self.journal.lock().unwrap().push(entry);
    }
}

/// Двойник Discord.
pub struct DiscordDouble {
    endpoint: Endpoint,
    shared: Arc<Shared>,
    listeners: Mutex<HashMap<u32, JoinHandle<()>>>,
}

fn ready_frame() -> Value {
    json!({
        "cmd": "DISPATCH",
        "data": {
            "v": 1,
            "config": {"cdn_host": "cdn.discordapp.com", "api_endpoint": "//discord.com/api", "environment": "production"},
            "user": {"id": "1", "username": "tester", "discriminator": "0", "global_name": "Tester", "avatar": null, "bot": false, "flags": 0, "premium_type": 0},
        },
        "evt": "READY",
        "nonce": null,
    })
}

impl DiscordDouble {
    /// Запустить двойник по сценарию (`listen`, `connections`).
    pub async fn start(endpoint: Endpoint, scenario: &Value, journal: Journal) -> DiscordDouble {
        let double = DiscordDouble {
            endpoint,
            shared: Arc::new(Shared {
                scenario: scenario.clone(),
                journal,
                conns: Mutex::default(),
                nonces: Mutex::default(),
                gates: Mutex::default(),
            }),
            listeners: Mutex::default(),
        };
        for id in scenario["listen"].as_array().into_iter().flatten() {
            double.listen(id.as_u64().unwrap() as u32).await;
        }
        double
    }

    /// Начать слушать `discord-ipc-<id>`.
    pub async fn listen(&self, id: u32) {
        if self.listeners.lock().unwrap().contains_key(&id) {
            return;
        }
        let address = self.endpoint.address(id);
        let shared = self.shared.clone();
        let accept = listen_on(&address, id, shared);
        self.listeners.lock().unwrap().insert(id, accept);
        // Дать задаче создать канал или сокет до первого подключения.
        tokio::task::yield_now().await;
    }

    /// Открыть ворота `name`.
    pub fn release(&self, name: &str) {
        self.shared.gate(name).send_replace(true);
    }

    /// Закрыть соединение со стороны сервера.
    pub fn close(&self, conn: usize, polite: bool) {
        self.conn(conn).commands.send(Cmd::Close { polite }).ok();
    }

    /// Послать клиенту кадр.
    pub fn send(&self, conn: usize, op: i32, data: &Value) {
        self.conn(conn)
            .commands
            .send(Cmd::Write(ipc::encode(op, data)))
            .ok();
    }

    /// Послать клиенту байты как есть (несколько кадров одним куском, половину кадра).
    pub fn send_raw(&self, conn: usize, bytes: Vec<u8>) {
        self.conn(conn).commands.send(Cmd::Write(bytes)).ok();
    }

    /// Заменить поведение принятого соединения.
    pub fn behave(&self, conn: usize, patch: &Value) {
        let conn = self.conn(conn);
        let mut behavior = conn.behavior.lock().unwrap();
        let mut merged = behavior.as_object().cloned().unwrap_or_default();
        for (key, value) in patch.as_object().into_iter().flatten() {
            merged.insert(key.clone(), value.clone());
        }
        *behavior = Value::Object(merged);
    }

    fn conn(&self, number: usize) -> Arc<Conn> {
        self.shared
            .conns
            .lock()
            .unwrap()
            .get(number - 1)
            .unwrap_or_else(|| panic!("двойник Discord: нет соединения {number}"))
            .clone()
    }

    /// Остановить: закрыть все соединения и адреса.
    pub fn stop(&self) {
        for conn in self.shared.conns.lock().unwrap().iter() {
            conn.commands.send(Cmd::Close { polite: false }).ok();
        }
        for (_, accept) in self.listeners.lock().unwrap().drain() {
            accept.abort();
        }
        #[cfg(unix)]
        for id in 0..=10 {
            let _ = std::fs::remove_file(self.endpoint.address(id));
        }
    }
}

impl Drop for DiscordDouble {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(windows)]
fn listen_on(address: &str, id: u32, shared: Arc<Shared>) -> JoinHandle<()> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let name = address.to_owned();
    // Первый экземпляр создаётся сразу: к моменту возврата `listen` канал уже есть.
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&name)
        .unwrap_or_else(|error| panic!("двойник Discord: канал {name}: {error}"));
    tokio::spawn(async move {
        loop {
            if server.connect().await.is_err() {
                return;
            }
            let connected = server;
            server = match ServerOptions::new().create(&name) {
                Ok(next) => next,
                Err(_) => return,
            };
            accept(&shared, id, connected);
        }
    })
}

#[cfg(unix)]
fn listen_on(address: &str, id: u32, shared: Arc<Shared>) -> JoinHandle<()> {
    let _ = std::fs::remove_file(address);
    let listener = tokio::net::UnixListener::bind(address)
        .unwrap_or_else(|error| panic!("двойник Discord: сокет {address}: {error}"));
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            accept(&shared, id, stream);
        }
    })
}

fn accept<S>(shared: &Arc<Shared>, id: u32, stream: S)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (commands, receiver) = mpsc::unbounded_channel();
    let number = {
        let mut conns = shared.conns.lock().unwrap();
        let number = conns.len() + 1;
        let behaviors = shared.scenario["connections"].as_array();
        let behavior = behaviors.filter(|list| !list.is_empty()).map_or_else(
            || json!({}),
            |list| list[(number - 1).min(list.len() - 1)].clone(),
        );
        conns.push(Arc::new(Conn {
            number,
            commands: commands.clone(),
            behavior: Mutex::new(behavior),
        }));
        number
    };
    shared.log(json!({"conn": number, "event": "open", "id": id}));
    let conn = shared.conns.lock().unwrap()[number - 1].clone();
    tokio::spawn(serve(shared.clone(), conn, stream, receiver));
}

async fn serve<S>(
    shared: Arc<Shared>,
    conn: Arc<Conn>,
    stream: S,
    mut commands: mpsc::UnboundedReceiver<Cmd>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut decoder = ipc::Decoder::default();
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        tokio::select! {
            read = reader.read(&mut buffer) => {
                match read {
                    Ok(0) | Err(_) => {
                        shared.log(json!({"conn": conn.number, "event": "eof"}));
                        let _ = writer.shutdown().await;
                        return;
                    }
                    Ok(count) => {
                        decoder.push(&buffer[..count]);
                        while let Ok(Some(frame)) = decoder.next_frame() {
                            let closing = frame.op == ipc::op::CLOSE;
                            on_frame(&shared, &conn, frame);
                            if closing && cfg!(windows) {
                                shared.log(json!({"conn": conn.number, "event": "eof"}));
                                return;
                            }
                        }
                    }
                }
            }
            command = commands.recv() => {
                match command {
                    None | Some(Cmd::Close { polite: false }) => return,
                    Some(Cmd::Write(bytes)) => {
                        if writer.write_all(&bytes).await.is_err() {
                            return;
                        }
                    }
                    Some(Cmd::Close { polite: true }) => {
                        let _ = writer.flush().await;
                        if cfg!(unix) {
                            // Половинное закрытие: ждём, пока клиент закроет свою сторону.
                            let _ = writer.shutdown().await;
                        } else {
                            // Канал закрывается целиком; клиент в 0.4.7 отвечает закрытием.
                            shared.log(json!({"conn": conn.number, "event": "eof"}));
                            return;
                        }
                    }
                }
            }
        }
    }
}

fn on_frame(shared: &Arc<Shared>, conn: &Arc<Conn>, frame: ipc::Frame) {
    let text = String::from_utf8_lossy(&frame.body).into_owned();
    let json: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    let name = usize::try_from(frame.op)
        .ok()
        .and_then(|at| OP_NAMES.get(at))
        .map_or_else(|| frame.op.to_string(), |name| (*name).to_owned());
    shared.log(json!({
        "conn": conn.number,
        "in": {"op": frame.op, "name": name, "json": shared.normalized(&json)},
    }));
    let behavior = conn.behavior.lock().unwrap().clone();
    match frame.op {
        ipc::op::HANDSHAKE => handshake(shared, conn, &behavior["handshake"]),
        ipc::op::FRAME if json["cmd"] == "SET_ACTIVITY" => {
            activity(conn, &behavior["activity"], &json)
        }
        _ => {}
    }
}

fn write(conn: &Conn, op: i32, data: &Value) {
    conn.commands.send(Cmd::Write(ipc::encode(op, data))).ok();
}

fn handshake(shared: &Arc<Shared>, conn: &Arc<Conn>, how: &Value) {
    if how == "silent" {
        return;
    }
    if how == "drop" {
        conn.commands.send(Cmd::Close { polite: false }).ok();
        return;
    }
    let gate = how.get("hold").and_then(Value::as_str).map(str::to_owned);
    let close = how.get("close").cloned();
    let end = how.get("end") != Some(&Value::Bool(false));
    let shared = shared.clone();
    let conn = conn.clone();
    tokio::spawn(async move {
        if let Some(gate) = gate {
            let mut open = shared.gate(&gate).subscribe();
            let _ = open.wait_for(|released| *released).await;
        }
        if let Some(close) = close {
            write(&conn, ipc::op::CLOSE, &close);
            if end {
                conn.commands.send(Cmd::Close { polite: true }).ok();
            }
            return;
        }
        write(&conn, ipc::op::FRAME, &ready_frame());
    });
}

fn activity(conn: &Arc<Conn>, how: &Value, request: &Value) {
    if how == "silent" {
        return;
    }
    if how == "drop" {
        conn.commands.send(Cmd::Close { polite: false }).ok();
        return;
    }
    let nonce = request["nonce"].clone();
    if let Some(error) = how.get("error") {
        write(
            conn,
            ipc::op::FRAME,
            &json!({"cmd": "SET_ACTIVITY", "data": error, "evt": "ERROR", "nonce": nonce}),
        );
        return;
    }
    let echo = request
        .pointer("/args/activity")
        .cloned()
        .unwrap_or(Value::Null);
    write(
        conn,
        ipc::op::FRAME,
        &json!({"cmd": "SET_ACTIVITY", "data": echo, "evt": null, "nonce": nonce}),
    );
}

/// Ждущий таймер: номер, задержка (мс), обратный вызов.
type PendingTimer = (u64, u64, Box<dyn FnOnce() + Send>);

/// Поддельные таймеры службы: ждущие таймеры, которые тест запускает сам.
pub struct FakeTimers {
    next: std::sync::atomic::AtomicU64,
    pending: Mutex<Vec<PendingTimer>>,
}

impl Default for FakeTimers {
    fn default() -> FakeTimers {
        FakeTimers {
            next: std::sync::atomic::AtomicU64::new(0),
            pending: Mutex::default(),
        }
    }
}

impl hlsr_online::discord_rpc::discord_rich_presence_service::Timers for FakeTimers {
    fn set(
        &self,
        delay_ms: u64,
        callback: Box<dyn FnOnce() + Send>,
    ) -> hlsr_online::discord_rpc::discord_rich_presence_service::TimerId {
        let id = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        self.pending.lock().unwrap().push((id, delay_ms, callback));
        hlsr_online::discord_rpc::discord_rich_presence_service::TimerId(id)
    }

    fn clear(&self, id: hlsr_online::discord_rpc::discord_rich_presence_service::TimerId) {
        self.pending
            .lock()
            .unwrap()
            .retain(|(pending, _, _)| *pending != id.0);
    }
}

impl FakeTimers {
    /// Основа повтора, к которой относится задержка с разбросом ±25 %.
    fn band_of(delay_ms: u64) -> Option<u64> {
        let mut base = 5000.0_f64;
        while base <= 300_000.0 {
            let delay = delay_ms as f64;
            if delay >= base * 0.75 && delay <= base * 1.25 {
                return Some(base as u64);
            }
            base *= 2.0;
        }
        let delay = delay_ms as f64;
        (225_000.0..=375_000.0).contains(&delay).then_some(300_000)
    }

    /// Запустить первый таймер вида `debounce` (1,5 с) или `retry` (повтор).
    pub fn fire(&self, kind: &str) -> bool {
        let callback = {
            let mut pending = self.pending.lock().unwrap();
            let at = pending.iter().position(|(_, delay, _)| {
                (if *delay == 1500 { "debounce" } else { "retry" }) == kind
            });
            at.map(|at| pending.remove(at).2)
        };
        match callback {
            Some(callback) => {
                callback();
                true
            }
            None => false,
        }
    }

    /// Ждущие таймеры как в фикстуре: `{kind: "debounce"}` и `{kind: "retry", base}`.
    pub fn describe(&self) -> Value {
        Value::Array(
            self.pending
                .lock()
                .unwrap()
                .iter()
                .map(|(_, delay, _)| {
                    if *delay == 1500 {
                        json!({"kind": "debounce"})
                    } else {
                        let mut entry = Map::new();
                        entry.insert("kind".into(), json!("retry"));
                        entry.insert("base".into(), json!(Self::band_of(*delay)));
                        Value::Object(entry)
                    }
                })
                .collect(),
        )
    }

    pub fn len(&self) -> usize {
        self.pending.lock().unwrap().len()
    }
}
