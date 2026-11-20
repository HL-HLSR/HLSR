//! `discord-rpc/src/client.js` с транспортом IPC (`transports/ipc.js`): `login`, `setActivity`,
//! `clearActivity`, `destroy`. Кадры уходят и приходят по каналу или сокету Discord
//! ([`super::ipc`]), ответы сопоставляются с запросами по `nonce`.
//!
//! События 0.4.7 и их порядок сохранены: закрытие транспорта (EOF, ошибка, кадр `CLOSE`) один
//! раз за клиента делает три вещи — отказывает ожидающим запросам, сообщает `disconnected`,
//! отказывает ещё не готовому `login` ("connection closed"). Поэтому закрытие до `READY`
//! даёт и `RPC_DISCONNECTED`, и `RPC_DISCORD_UNAVAILABLE`, а уничтоженный по тайм-ауту клиент
//! после закрытия сокета снова сообщает `disconnected`.

use super::ipc::{self, Decoder, Frame, IpcConfig, get_ipc, op};
use super::util::uuid4122;
use crate::discord_rpc::discord_rich_presence_service::{BoxFuture, PresenceClient};
use crate::discord_rpc::presence_model::Activity;
use hlsr_core::js;
use hlsr_core::json_text::js_number;
use hlsr_delivery::timing::{Random, system_random};
use serde_json::{Map, Value, json};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};

/// Ошибка клиента: `Error` JS с необязательным `code` (у ответа Discord с `evt: "ERROR"` это
/// `data.code` как есть — число, строка или что пришло).
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub message: String,
    pub code: Option<Value>,
}

impl RpcError {
    pub fn new(message: impl Into<String>) -> RpcError {
        RpcError {
            message: message.into(),
            code: None,
        }
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RpcError {}

/// Больше этого `setActivity` бросает `RangeError` ("fit into a unix timestamp").
const MAX_TIMESTAMP_MS: f64 = 2_147_483_647_000.0;

/// Сколько `destroy()` ждёт закрытия соединения собеседником: у 0.4.7 обещание не кончается,
/// пока сокет не закрыт; здесь через это время соединение закрывается самим клиентом.
const CLOSE_WAIT: Duration = Duration::from_secs(5);

enum Command {
    Frame(Vec<u8>),
    Shutdown,
}

type Reply = oneshot::Sender<Result<Value, RpcError>>;

#[derive(Default)]
struct State {
    /// `_connectPromise`: результат подключения, один на клиента.
    connect: Option<watch::Sender<Option<Result<(), RpcError>>>>,
    writer: Option<mpsc::UnboundedSender<Command>>,
    /// `_expecting`: запросы, ждущие ответа, в порядке отправки.
    expecting: Vec<(String, Reply)>,
    /// `transport.once('close')` клиента уже сработал.
    close_handled: bool,
    /// `client.on('disconnected', …)`.
    disconnected: Vec<Arc<dyn Fn() + Send + Sync>>,
    /// `transport.once('close', r)` из `close()`.
    close_waiters: Vec<oneshot::Sender<()>>,
    /// Задача чтения: её останавливают, закрывая канал Windows целиком.
    reader: Option<tokio::task::AbortHandle>,
}

struct Inner {
    ipc: IpcConfig,
    random: Random,
    pid: u32,
    state: Mutex<State>,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Клиент Discord IPC.
#[derive(Clone)]
pub struct RpcClient {
    inner: Arc<Inner>,
}

impl RpcClient {
    /// Клиент по настоящим адресам, `pid` процесса и системной случайности.
    pub fn new(ipc: IpcConfig) -> RpcClient {
        RpcClient::with_parts(ipc, system_random(), std::process::id())
    }

    /// С подставленными случайностью (для `nonce`) и `pid`.
    pub fn with_parts(ipc: IpcConfig, random: Random, pid: u32) -> RpcClient {
        RpcClient {
            inner: Arc::new(Inner {
                ipc,
                random,
                pid,
                state: Mutex::new(State::default()),
            }),
        }
    }
}

/// Аргументы `SET_ACTIVITY` так, как их собирает `setActivity(args)`: `timestamps` — при
/// ненулевом начале, `assets` — если задана хоть одна картинка или подпись, `instance` — всегда;
/// ключи в порядке библиотеки, пустые пропускаются. Начало позже 2038 года — `RangeError`.
pub fn set_activity_args(activity: &Activity, pid: u32) -> Result<Value, RpcError> {
    let mut timestamps = None;
    if let Some(start) = activity
        .start_timestamp
        .filter(|start| *start != 0.0 && !start.is_nan())
    {
        if start > MAX_TIMESTAMP_MS {
            return Err(RpcError::new(
                "timestamps.start must fit into a unix timestamp",
            ));
        }
        timestamps = Some(json!({"start": js_number(start)}));
    }

    let mut assets = Map::new();
    assets.insert("large_image".into(), json!(activity.large_image_key));
    assets.insert("large_text".into(), json!(activity.large_image_text));
    if let Some(key) = activity.small_image_key {
        assets.insert("small_image".into(), json!(key));
    }
    if let Some(text) = activity.small_image_text {
        assets.insert("small_text".into(), json!(text));
    }
    let any_asset = !activity.large_image_key.is_empty()
        || !activity.large_image_text.is_empty()
        || activity.small_image_key.is_some_and(|key| !key.is_empty())
        || activity
            .small_image_text
            .is_some_and(|text| !text.is_empty());

    let mut body = Map::new();
    if let Some(state) = &activity.state {
        body.insert("state".into(), json!(state));
    }
    body.insert("details".into(), json!(activity.details));
    if let Some(timestamps) = timestamps {
        body.insert("timestamps".into(), timestamps);
    }
    if any_asset {
        body.insert("assets".into(), Value::Object(assets));
    }
    if !activity.buttons.is_empty() {
        let buttons: Vec<Value> = activity
            .buttons
            .iter()
            .map(|button| json!({"label": button.label, "url": button.url}))
            .collect();
        body.insert("buttons".into(), Value::Array(buttons));
    }
    body.insert("instance".into(), json!(activity.instance));
    Ok(json!({"pid": pid, "activity": Value::Object(body)}))
}

impl Inner {
    fn settle_connect(&self, result: Result<(), RpcError>) {
        if let Some(sender) = &lock(&self.state).connect {
            sender.send_if_modified(|current| {
                if current.is_none() {
                    *current = Some(result);
                    true
                } else {
                    false
                }
            });
        }
    }

    /// Кадр в очередь записи; `false` — писать некуда.
    fn send(&self, op: i32, data: &Value) -> bool {
        let state = lock(&self.state);
        state
            .writer
            .as_ref()
            .is_some_and(|writer| writer.send(Command::Frame(ipc::encode(op, data))).is_ok())
    }

    /// Событие `close` транспорта (EOF, ошибка, кадр `CLOSE`): может повторяться, но
    /// обработчик подключения срабатывает один раз.
    fn emit_close(&self) {
        let (first, handlers, expecting, waiters) = {
            let mut state = lock(&self.state);
            let first = !state.close_handled;
            state.close_handled = true;
            let handlers = if first {
                state.disconnected.clone()
            } else {
                Vec::new()
            };
            let expecting = if first {
                std::mem::take(&mut state.expecting)
            } else {
                Vec::new()
            };
            (
                first,
                handlers,
                expecting,
                std::mem::take(&mut state.close_waiters),
            )
        };
        if first {
            // Порядок 0.4.7: ожидающие отказываются, `disconnected`, отказ подключения. Отказы
            // ожидающим здесь только отправляются: реакции на них идут в других задачах, позже
            // синхронной части `disconnected`.
            for handler in handlers {
                handler();
            }
            for (_, reply) in expecting {
                let _ = reply.send(Err(RpcError::new("connection closed")));
            }
            self.settle_connect(Err(RpcError::new("connection closed")));
        }
        for waiter in waiters {
            let _ = waiter.send(());
        }
    }

    /// Сокет закрыт собеседником или сломан: событие `close`, запись больше не нужна.
    fn socket_gone(&self) {
        self.emit_close();
        let expecting = {
            let mut state = lock(&self.state);
            state.writer = None;
            // Запрос, поставленный между `emit_close` и этим местом, ответа не дождётся.
            std::mem::take(&mut state.expecting)
        };
        for (_, reply) in expecting {
            let _ = reply.send(Err(RpcError::new("connection closed")));
        }
    }

    /// Закрыть соединение целиком: чтение останавливается. Так кончается `destroy`, если
    /// собеседник не закрыл соединение сам (у 0.4.7 оно оставалось открытым навсегда).
    fn close_whole_pipe(&self) {
        if let Some(reader) = lock(&self.state).reader.take() {
            reader.abort();
        }
        self.socket_gone();
    }

    /// `_onRpcMessage(message)`.
    fn on_message(&self, message: &Value) {
        let get = |key: &str| message.get(key);
        if get("cmd").and_then(Value::as_str) == Some("DISPATCH")
            && get("evt").and_then(Value::as_str) == Some("READY")
        {
            self.settle_connect(Ok(()));
            return;
        }
        let Some(nonce) = get("nonce").and_then(Value::as_str) else {
            return;
        };
        let reply = {
            let mut state = lock(&self.state);
            state
                .expecting
                .iter()
                .position(|(expected, _)| expected == nonce)
                .map(|at| state.expecting.remove(at).1)
        };
        let Some(reply) = reply else {
            return;
        };
        let data = get("data").cloned().unwrap_or(Value::Null);
        let result = if get("evt").and_then(Value::as_str) == Some("ERROR") {
            Err(RpcError {
                message: data.get("message").map_or_else(String::new, js::to_string),
                code: data.get("code").cloned(),
            })
        } else {
            Ok(data)
        };
        let _ = reply.send(result);
    }

    fn dispatch(&self, frame: Frame) {
        let Ok(data) = serde_json::from_slice::<Value>(&frame.body) else {
            return;
        };
        match frame.op {
            op::PING => {
                self.send(op::PONG, &data);
            }
            op::FRAME => {
                if js::truthy(&data) {
                    self.on_message(&data);
                }
            }
            op::CLOSE => self.emit_close(),
            _ => {}
        }
    }

    /// `transport.connect()`: найти адрес, послать рукопожатие, читать кадры до закрытия.
    async fn run_transport(self: Arc<Self>, client_id: String) {
        let Some(stream) = get_ipc(&self.ipc).await else {
            self.settle_connect(Err(RpcError::new("Could not connect")));
            return;
        };
        let (mut reader, mut writer) = tokio::io::split(stream);
        let (queue, mut commands) = mpsc::unbounded_channel::<Command>();
        lock(&self.state).writer = Some(queue);

        let inner = self.clone();
        tokio::spawn(async move {
            while let Some(command) = commands.recv().await {
                match command {
                    Command::Frame(bytes) => {
                        if writer.write_all(&bytes).await.is_err() {
                            inner.socket_gone();
                            return;
                        }
                    }
                    Command::Shutdown => {
                        // Unix: FIN, собеседник отвечает своим. У именованного канала Windows
                        // половинного закрытия нет: `shutdown` ничего не делает, а Discord
                        // закрывает канал сам, получив `CLOSE`.
                        let _ = writer.shutdown().await;
                        return;
                    }
                }
            }
        });

        self.send(op::HANDSHAKE, &json!({"v": 1, "client_id": client_id}));

        let mut decoder = Decoder::default();
        let mut buffer = vec![0u8; 16 * 1024];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    decoder.push(&buffer[..read]);
                    loop {
                        match decoder.next_frame() {
                            Ok(Some(frame)) => self.dispatch(frame),
                            Ok(None) => break,
                            Err(_) => {
                                self.socket_gone();
                                return;
                            }
                        }
                    }
                }
            }
        }
        self.socket_gone();
    }

    /// `connect(clientId)`: общий для всех вызовов `login` результат, тайм-аут 10 с.
    async fn connect(self: Arc<Self>, client_id: String) -> Result<(), RpcError> {
        let mut result = {
            let mut state = lock(&self.state);
            if let Some(sender) = &state.connect {
                sender.subscribe()
            } else {
                let (sender, receiver) = watch::channel(None);
                state.connect = Some(sender);
                drop(state);
                let reader = tokio::spawn(self.clone().run_transport(client_id));
                lock(&self.state).reader = Some(reader.abort_handle());
                receiver
            }
        };
        let ready = async {
            loop {
                if let Some(done) = result.borrow_and_update().clone() {
                    return done;
                }
                if result.changed().await.is_err() {
                    return Err(RpcError::new("connection closed"));
                }
            }
        };
        match tokio::time::timeout(self.ipc.connect_timeout, ready).await {
            Ok(done) => done,
            Err(_) => {
                self.settle_connect(Err(RpcError::new("RPC_CONNECTION_TIMEOUT")));
                // Если `READY` успел раньше, чем сработал тайм-аут, победил он.
                let state = lock(&self.state);
                state
                    .connect
                    .as_ref()
                    .and_then(|sender| sender.borrow().clone())
                    .unwrap_or_else(|| Err(RpcError::new("RPC_CONNECTION_TIMEOUT")))
            }
        }
    }

    /// `request(cmd, args)`: кадр уходит сразу, ответ — по `nonce`.
    fn request(&self, cmd: &str, args: &Value) -> BoxFuture<Result<Value, RpcError>> {
        let nonce = uuid4122(&self.random);
        let (reply, answer) = oneshot::channel();
        let frame = json!({"cmd": cmd, "args": args, "nonce": nonce});
        let sent = {
            let mut state = lock(&self.state);
            if state.writer.is_some() {
                state.expecting.push((nonce.clone(), reply));
                true
            } else {
                false
            }
        };
        if !sent || !self.send(op::FRAME, &frame) {
            lock(&self.state)
                .expecting
                .retain(|(expected, _)| *expected != nonce);
            return Box::pin(async { Err(RpcError::new("connection closed")) });
        }
        Box::pin(async move {
            answer
                .await
                .unwrap_or_else(|_| Err(RpcError::new("connection closed")))
        })
    }
}

impl PresenceClient for RpcClient {
    fn on_disconnected(&self, handler: Arc<dyn Fn() + Send + Sync>) {
        lock(&self.inner.state).disconnected.push(handler);
    }

    fn login(&self, client_id: &str) -> BoxFuture<Result<(), RpcError>> {
        let inner = self.inner.clone();
        let client_id = client_id.to_owned();
        Box::pin(inner.connect(client_id))
    }

    fn set_activity(
        &self,
        activity: &Activity,
    ) -> Result<BoxFuture<Result<Value, RpcError>>, RpcError> {
        let args = set_activity_args(activity, self.inner.pid)?;
        Ok(self.inner.request("SET_ACTIVITY", &args))
    }

    fn clear_activity(&self) -> BoxFuture<Result<Value, RpcError>> {
        self.inner
            .request("SET_ACTIVITY", &json!({"pid": self.inner.pid}))
    }

    fn destroy(&self) -> BoxFuture<()> {
        let (closed, wait) = oneshot::channel();
        let sent = {
            let mut state = lock(&self.inner.state);
            match &state.writer {
                None => false,
                Some(writer) => {
                    let frame = ipc::encode(op::CLOSE, &json!({}));
                    let sent = writer.send(Command::Frame(frame)).is_ok()
                        && writer.send(Command::Shutdown).is_ok();
                    if sent {
                        state.close_waiters.push(closed);
                    }
                    sent
                }
            }
        };
        let inner = self.inner.clone();
        Box::pin(async move {
            if sent && tokio::time::timeout(CLOSE_WAIT, wait).await.is_err() {
                inner.close_whole_pipe();
            }
        })
    }
}
