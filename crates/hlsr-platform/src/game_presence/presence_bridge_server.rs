//! `game-presence/PresenceBridgeServer.js`: локальный сервер, к которому подключается игра.
//! Windows — именованный канал `\\.\pipe\hlsr-presence-<16 hex>`; Linux — TCP на 127.0.0.1 (порт
//! прошлого запуска, если он свободен): под Proton канал живёт внутри wineserver, мост в префиксе
//! пересылает байты сюда. Игра представляется сообщением `hello` с токеном (64 hex) в течение 5 с;
//! потом простой дольше 30 с, больше 20 сообщений в секунду, строка длиннее 2048 байт, неверное
//! сообщение — обрыв связи. Одна игра на связи: на Linux чужое соединение её не вытесняет, на
//! Windows новое заменяет прежнее.
//!
//! Сообщения `chat.*` и `emote.*` своего сеанса уходят обработчику чата (`onChat`), строки чата,
//! эмоуты и значки сервер пишет игре через [`ChatBridge`].
//!
//! Отличия от Node (игре не видны): сокет открывается в `start()` сразу, а не через такт цикла
//! событий, поэтому порт известен сразу; остановка (`stop`) отпускает порт, когда рантайм
//! закончит задачу приёма, — не мгновенно, но новые соединения уже не принимаются.

use super::protocol::{self, Message, StateMessage};
use hlsr_core::js::Platform;
use hlsr_core::js::errno;
use hlsr_core::logger::{Logger, TracingLogger};
use hlsr_delivery::ports::{ChatBridge, ChatMessage};
use hlsr_delivery::timing::{Clock, system_clock};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;

/// Пределы соединения (`DEFAULTS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub handshake_timeout_ms: u64,
    pub idle_timeout_ms: u64,
    pub max_messages_per_second: u32,
    pub max_line_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            handshake_timeout_ms: 5000,
            idle_timeout_ms: 30_000,
            max_messages_per_second: 20,
            max_line_bytes: protocol::MAX_LINE_BYTES,
        }
    }
}

/// Принятое сообщение `state` своего сеанса (`onState`).
pub type StateHandler = Arc<dyn Fn(&StateMessage) + Send + Sync>;
/// Игра ушла (`onGameGone(reason)`): `CLOSED`, `SOCKET_ERROR`, `REPLACED`, `SHUTDOWN`, тайм-аут
/// или причина отказа.
pub type GoneHandler = Arc<dyn Fn(&str) + Send + Sync>;
/// Игра представилась: рукопожатие принято (`onGameReady`).
pub type ReadyHandler = Arc<dyn Fn() + Send + Sync>;
/// Сообщение чата своего сеанса (`onChat`): всё, кроме `hello` и `state`.
pub type ChatHandler = Arc<dyn Fn(&ChatMessage) + Send + Sync>;

/// Что сервер сообщает хозяину. Вызовы быстрые, падение обработчика попадает в журнал.
#[derive(Default, Clone)]
pub struct Handlers {
    pub on_state: Option<StateHandler>,
    pub on_game_gone: Option<GoneHandler>,
    pub on_game_ready: Option<ReadyHandler>,
    pub on_chat: Option<ChatHandler>,
}

/// Параметры конструктора (`options`).
#[derive(Default)]
pub struct ServerOptions {
    pub log: Option<Arc<dyn Logger>>,
    pub now: Option<Clock>,
    pub limits: Option<Limits>,
    /// `platform`; по умолчанию — эта ОС.
    pub platform: Option<Platform>,
    /// Токен (64 hex); по умолчанию случайный.
    pub token: Option<String>,
    /// Имя канала; по умолчанию случайное.
    pub pipe_name: Option<String>,
    /// Порт прошлого запуска.
    pub port: Option<u16>,
    pub handlers: Option<Handlers>,
    /// Рантайм задач; по умолчанию — текущий.
    pub runtime: Option<Handle>,
}

/// Счётчики связи (`stats`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub accepted: u64,
    pub rejected: u64,
    pub messages: u64,
    pub dropped: u64,
}

/// `diagnostics()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Diagnostics {
    pub listening: bool,
    pub connected: bool,
    pub stats: Stats,
}

impl Diagnostics {
    /// Объект JS `{listening, connected, accepted, rejected, messages, dropped}`.
    pub fn to_value(&self) -> Value {
        json!({
            "listening": self.listening,
            "connected": self.connected,
            "accepted": self.stats.accepted,
            "rejected": self.stats.rejected,
            "messages": self.stats.messages,
            "dropped": self.stats.dropped,
        })
    }
}

struct Conn {
    id: u64,
    authenticated: bool,
    session: Option<String>,
    buffer: Vec<u8>,
    window_start_ms: i64,
    window_count: u32,
    timer: Option<AbortHandle>,
    /// Пока отправитель жив, задача соединения работает; сброшен — соединение закрыто.
    cancel: Option<oneshot::Sender<()>>,
    writer: mpsc::UnboundedSender<Vec<u8>>,
}

struct Inner {
    started: bool,
    listening_addr: Option<std::net::SocketAddr>,
    preferred_port: u16,
    accept_task: Option<AbortHandle>,
    connection: Option<Conn>,
    discord_connected: bool,
    stats: Stats,
    next_id: u64,
}

struct Shared {
    log: Arc<dyn Logger>,
    now: Clock,
    limits: Limits,
    platform: Platform,
    token: String,
    pipe_name: String,
    handlers: Handlers,
    runtime: Handle,
    inner: Mutex<Inner>,
}

/// Сервер моста; копии — один сервер.
#[derive(Clone)]
pub struct PresenceBridgeServer {
    shared: Arc<Shared>,
}

/// `newToken()`: 32 случайных байта в hex.
pub fn new_token() -> String {
    random_hex(32)
}

/// `newPipeName()`.
pub fn new_pipe_name() -> String {
    format!("{}{}", super::endpoint_store::PIPE_PREFIX, random_hex(8))
}

fn random_hex(length: usize) -> String {
    let mut bytes = vec![0u8; length];
    getrandom::fill(&mut bytes).expect("источник случайных чисел ОС");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `tokenMatches(expected, received)`: сравнение за одно и то же время.
pub fn token_matches(expected: &str, received: &str) -> bool {
    if expected.len() != received.len() {
        return false;
    }
    expected
        .bytes()
        .zip(received.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

impl PresenceBridgeServer {
    /// Вне рантайма tokio нужен `options.runtime`; без него — паника с пояснением.
    pub fn new(options: ServerOptions) -> PresenceBridgeServer {
        PresenceBridgeServer {
            shared: Arc::new(Shared {
                log: options.log.unwrap_or_else(|| Arc::new(TracingLogger)),
                now: options.now.unwrap_or_else(system_clock),
                limits: options.limits.unwrap_or_default(),
                platform: options.platform.unwrap_or(Platform::CURRENT),
                token: options.token.unwrap_or_else(new_token),
                pipe_name: options.pipe_name.unwrap_or_else(new_pipe_name),
                handlers: options.handlers.unwrap_or_default(),
                runtime: options.runtime.unwrap_or_else(|| {
                    Handle::try_current().expect(
                        "серверу моста игры нужна среда tokio: создайте его внутри неё или задайте options.runtime",
                    )
                }),
                inner: Mutex::new(Inner {
                    started: false,
                    listening_addr: None,
                    preferred_port: options.port.unwrap_or(0),
                    accept_task: None,
                    connection: None,
                    discord_connected: false,
                    stats: Stats::default(),
                    next_id: 1,
                }),
            }),
        }
    }

    /// `start()`: открыть канал; возвращает имя канала. Повторный вызов ничего не делает.
    pub fn start(&self) -> String {
        self.shared.start()
    }

    /// `stop()`: закрыть связь с игрой (`SHUTDOWN`) и канал.
    pub fn stop(&self) {
        self.shared.stop();
    }

    /// `endpoint()`: имя канала, токен, порт (Linux).
    pub fn endpoint(&self) -> super::endpoint_store::Endpoint {
        super::endpoint_store::Endpoint {
            pipe_name: self.shared.pipe_name.clone(),
            token: self.shared.token.clone(),
            port: self.port(),
        }
    }

    /// `port()`: порт TCP на Linux, когда сервер слушает.
    pub fn port(&self) -> Option<u16> {
        let inner = self.shared.lock();
        self.shared.port_of(&inner)
    }

    /// Адрес, на котором слушает TCP (Linux): только 127.0.0.1.
    pub fn listening_address(&self) -> Option<std::net::SocketAddr> {
        self.shared.lock().listening_addr
    }

    /// `environment()`: переменные окружения игры (`HLSR_PRESENCE_PIPE`, `…_TOKEN`, на Linux
    /// `…_PORT`); пока сервер не запущен или порта нет — пусто.
    pub fn environment(&self) -> Vec<(String, String)> {
        let inner = self.shared.lock();
        if !inner.started {
            return Vec::new();
        }
        let mut env = vec![
            (
                "HLSR_PRESENCE_PIPE".to_owned(),
                self.shared.pipe_name.clone(),
            ),
            ("HLSR_PRESENCE_TOKEN".to_owned(), self.shared.token.clone()),
        ];
        if self.shared.platform == Platform::Win32 {
            return env;
        }
        match self.shared.port_of(&inner) {
            Some(port) if port != 0 => {
                env.push(("HLSR_PRESENCE_PORT".to_owned(), port.to_string()));
                env
            }
            _ => Vec::new(),
        }
    }

    /// `setDiscordConnected(connected)`: сообщить игре, подключён ли Discord. `true`, если
    /// состояние изменилось.
    pub fn set_discord_connected(&self, connected: bool) -> bool {
        {
            let mut inner = self.shared.lock();
            if inner.discord_connected == connected {
                return false;
            }
            inner.discord_connected = connected;
        }
        self.shared.push(protocol::encode_discord_ack(connected));
        true
    }

    /// `diagnostics()`.
    pub fn diagnostics(&self) -> Diagnostics {
        let inner = self.shared.lock();
        Diagnostics {
            listening: inner.started,
            connected: inner
                .connection
                .as_ref()
                .is_some_and(|conn| conn.authenticated),
            stats: inner.stats,
        }
    }
}

/// Сторона моста для потребителей чата (`sendChatLine`, `sendEmote*`, `sendBadgeReady`): строки
/// уходят игре, пока она на связи.
impl ChatBridge for PresenceBridgeServer {
    fn send_chat_line(
        &self,
        platform: &str,
        nick: &str,
        text: &str,
        badge1: &str,
        badge2: &str,
    ) -> bool {
        self.shared.push(protocol::encode_chat_line(
            platform, nick, text, badge1, badge2,
        ))
    }

    fn send_emote_ready(&self, name: &str, wide: u32, tall: u32, rgba_base64: &str) -> bool {
        self.shared
            .push(protocol::encode_emote_ready(name, wide, tall, rgba_base64))
    }

    fn send_emote_begin(
        &self,
        name: &str,
        wide: u32,
        tall: u32,
        frames: u32,
        frame_ms: u32,
    ) -> bool {
        self.shared.push(protocol::encode_emote_begin(
            name, wide, tall, frames, frame_ms,
        ))
    }

    fn send_emote_frame(&self, name: &str, index: u32, rgba_base64: &str) -> bool {
        self.shared
            .push(protocol::encode_emote_frame(name, index, rgba_base64))
    }

    fn send_emote_miss(&self, name: &str) -> bool {
        self.shared.push(protocol::encode_emote_miss(name))
    }

    fn send_badge_ready(&self, name: &str, wide: u32, tall: u32, rgba_base64: &str) -> bool {
        self.shared
            .push(protocol::encode_badge_ready(name, wide, tall, rgba_base64))
    }
}

/// Какой шаг сделать по строке игры (решается под замком, исполняется без него).
enum Step {
    Reject(String),
    Ready,
    State(StateMessage),
    Chat(ChatMessage),
    Nothing,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn port_of(&self, inner: &Inner) -> Option<u16> {
        if !inner.started || self.platform == Platform::Win32 {
            return None;
        }
        inner.listening_addr.map(|address| address.port())
    }

    /// Вызвать обработчик хозяина так, чтобы его падение не ломало сервер.
    fn guarded(&self, what: &str, call: impl FnOnce()) {
        if let Err(text) = hlsr_core::tasks::catch_panic(call) {
            self.log.warn(&format!("[game-presence] {what}: {text}"));
        }
    }

    fn start(self: &Arc<Self>) -> String {
        let mut inner = self.lock();
        if inner.started {
            return self.pipe_name.clone();
        }
        inner.started = true;
        if self.platform == Platform::Win32 {
            self.listen_pipe(&mut inner);
        } else {
            self.listen_tcp(&mut inner);
        }
        self.pipe_name.clone()
    }

    fn listen_tcp(self: &Arc<Self>, inner: &mut Inner) {
        let mut result = std::net::TcpListener::bind(("127.0.0.1", inner.preferred_port));
        // Старый порт заняли — слушаем новый: игра прошлого запуска статус уже не передаст,
        // новые запуски получат свежий адрес.
        if let Err(error) = &result
            && error.kind() == std::io::ErrorKind::AddrInUse
            && inner.preferred_port != 0
        {
            self.log.warn(&format!(
                "[game-presence] порт {} занят, беру новый",
                inner.preferred_port
            ));
            inner.preferred_port = 0;
            result = std::net::TcpListener::bind(("127.0.0.1", 0));
        }
        let listener = match result.and_then(|listener| {
            listener.set_nonblocking(true)?;
            Ok(listener)
        }) {
            Ok(listener) => listener,
            Err(error) => {
                self.unavailable(&error);
                return;
            }
        };
        let address = listener.local_addr().ok();
        let listener = {
            let _guard = self.runtime.enter();
            match tokio::net::TcpListener::from_std(listener) {
                Ok(listener) => listener,
                Err(error) => {
                    self.unavailable(&error);
                    return;
                }
            }
        };
        inner.listening_addr = address;
        let shared = self.clone();
        let task = self.runtime.spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let _ = stream.set_nodelay(true);
                        shared.accept(stream);
                    }
                    Err(error) => {
                        shared.unavailable(&error);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        });
        inner.accept_task = Some(task.abort_handle());
    }

    #[cfg(windows)]
    fn listen_pipe(self: &Arc<Self>, inner: &mut Inner) {
        use tokio::net::windows::named_pipe::ServerOptions;
        let name = self.pipe_name.clone();
        let first = {
            let _guard = self.runtime.enter();
            ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(&name)
        };
        let mut server = match first {
            Ok(server) => server,
            Err(error) => {
                // libuv: ERROR_ACCESS_DENIED при создании первого экземпляра — адрес занят.
                let code = if error.raw_os_error() == Some(5) {
                    "EADDRINUSE".to_owned()
                } else {
                    error_code(&error)
                };
                self.log
                    .warn(&format!("[game-presence] канал недоступен: {code}"));
                return;
            }
        };
        let shared = self.clone();
        let task = self.runtime.spawn(async move {
            loop {
                if let Err(error) = server.connect().await {
                    shared.unavailable(&error);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                let connected = server;
                server = match ServerOptions::new()
                    .reject_remote_clients(true)
                    .create(&name)
                {
                    Ok(next) => next,
                    Err(error) => {
                        shared.unavailable(&error);
                        shared.accept(connected);
                        return;
                    }
                };
                shared.accept(connected);
            }
        });
        inner.accept_task = Some(task.abort_handle());
    }

    #[cfg(not(windows))]
    fn listen_pipe(self: &Arc<Self>, _inner: &mut Inner) {
        // Именованный канал Windows есть только в Windows.
        self.log.warn("[game-presence] канал недоступен: ERR");
    }

    fn unavailable(&self, error: &std::io::Error) {
        self.log.warn(&format!(
            "[game-presence] канал недоступен: {}",
            error_code(error)
        ));
    }

    fn stop(&self) {
        self.close_connection("SHUTDOWN");
        let mut inner = self.lock();
        if inner.started {
            if let Some(task) = inner.accept_task.take() {
                task.abort();
            }
            inner.started = false;
            inner.listening_addr = None;
        }
    }

    // ------------------------------------------------------------------ соединения

    /// `onConnection(socket)`: принять сокет или отказать.
    fn accept<S>(self: &Arc<Self>, stream: S)
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let replaced = {
            let mut inner = self.lock();
            // Сервер остановлен, пока соединение шло к нам.
            if !inner.started {
                return;
            }
            // На Linux TCP доступен любому локальному процессу: пока игра на связи, чужое
            // соединение её не вытесняет. Именованный канал на Windows заменяет прежнее.
            if self.platform != Platform::Win32
                && inner
                    .connection
                    .as_ref()
                    .is_some_and(|conn| conn.authenticated)
            {
                inner.stats.rejected += 1;
                return;
            }
            inner.connection.as_ref().map(|conn| conn.id)
        };
        if let Some(id) = replaced {
            self.drop_connection(id, "REPLACED");
        }

        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (write_tx, write_rx) = mpsc::unbounded_channel();
        let id = {
            let mut inner = self.lock();
            let id = inner.next_id;
            inner.next_id += 1;
            let mut conn = Conn {
                id,
                authenticated: false,
                session: None,
                buffer: Vec::new(),
                window_start_ms: (self.now)(),
                window_count: 0,
                timer: None,
                cancel: Some(cancel_tx),
                writer: write_tx,
            };
            self.arm_timeout(
                &mut conn,
                self.limits.handshake_timeout_ms,
                "HANDSHAKE_TIMEOUT",
            );
            inner.connection = Some(conn);
            id
        };
        let shared = self.clone();
        self.runtime
            .spawn(async move { shared.serve(id, stream, cancel_rx, write_rx).await });
    }

    /// Чтение, запись и закрытие одного соединения.
    async fn serve<S>(
        self: Arc<Self>,
        id: u64,
        mut stream: S,
        mut cancel: oneshot::Receiver<()>,
        mut writes: mpsc::UnboundedReceiver<Vec<u8>>,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let mut buf = vec![0u8; 8192];
        loop {
            tokio::select! {
                _ = &mut cancel => {
                    // `socket.destroy()` в Node не отнимает уже записанное: строки, которые мост
                    // успел поставить в очередь (ответ о Discord), уходят игре до закрытия.
                    while let Ok(bytes) = writes.try_recv() {
                        if stream.write_all(&bytes).await.is_err() {
                            break;
                        }
                    }
                    let _ = stream.flush().await;
                    break;
                }
                data = writes.recv() => match data {
                    Some(bytes) => {
                        if stream.write_all(&bytes).await.is_err() {
                            self.drop_connection(id, "SOCKET_ERROR");
                            break;
                        }
                    }
                    None => break,
                },
                read = stream.read(&mut buf) => match read {
                    Ok(0) => {
                        self.drop_connection(id, "CLOSED");
                        break;
                    }
                    Ok(count) => self.on_data(id, &buf[..count]),
                    Err(error) => {
                        // Закрытие канала клиентом на Windows приходит как «канал закрыт».
                        let reason = if error.kind() == std::io::ErrorKind::BrokenPipe {
                            "CLOSED"
                        } else {
                            "SOCKET_ERROR"
                        };
                        self.drop_connection(id, reason);
                        break;
                    }
                },
            }
        }
    }

    fn arm_timeout(self: &Arc<Self>, conn: &mut Conn, ms: u64, reason: &'static str) {
        if let Some(timer) = conn.timer.take() {
            timer.abort();
        }
        let shared = self.clone();
        let id = conn.id;
        let task = self.runtime.spawn(async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            shared.reject(id, reason);
        });
        conn.timer = Some(task.abort_handle());
    }

    /// `dropConnection(conn, reason)`: уничтожить сокет; если это текущее соединение — забыть его
    /// и сообщить хозяину.
    fn drop_connection(&self, id: u64, reason: &str) {
        {
            let mut inner = self.lock();
            if inner.connection.as_ref().map(|conn| conn.id) != Some(id) {
                return;
            }
            let mut conn = inner.connection.take().expect("соединение проверено выше");
            if let Some(timer) = conn.timer.take() {
                timer.abort();
            }
            // Отправитель сброшен: задача соединения закрывает сокет.
            conn.cancel.take();
        }
        if let Some(handler) = &self.handlers.on_game_gone {
            self.guarded("обработчик отключения упал", || {
                handler(reason)
            });
        }
    }

    fn close_connection(&self, reason: &str) {
        let current = self.lock().connection.as_ref().map(|conn| conn.id);
        if let Some(id) = current {
            self.drop_connection(id, reason);
        }
    }

    /// `reject(conn, reason)`: закрыть соединение с причиной.
    fn reject(&self, id: u64, reason: &str) {
        {
            let mut inner = self.lock();
            let Some(conn) = inner.connection.as_ref() else {
                return;
            };
            if conn.id != id {
                return;
            }
            if conn.authenticated {
                self.log
                    .info(&format!("[game-presence] соединение закрыто: {reason}"));
            } else {
                inner.stats.rejected += 1;
            }
        }
        self.drop_connection(id, reason);
    }

    /// `push(line)`: записать строку игре, пока она на связи.
    fn push(&self, line: String) -> bool {
        let inner = self.lock();
        match &inner.connection {
            Some(conn) if conn.authenticated => conn.writer.send(line.into_bytes()).is_ok(),
            _ => false,
        }
    }

    /// `onData(conn, chunk)`.
    fn on_data(self: &Arc<Self>, id: u64, chunk: &[u8]) {
        {
            let mut inner = self.lock();
            let Some(conn) = inner.connection.as_mut().filter(|conn| conn.id == id) else {
                return;
            };
            conn.buffer.extend_from_slice(chunk);
            if conn.buffer.len() > self.limits.max_line_bytes * 2 {
                drop(inner);
                self.reject(id, "LINE_TOO_LONG");
                return;
            }
        }
        loop {
            let line = {
                let mut inner = self.lock();
                let Some(conn) = inner.connection.as_mut().filter(|conn| conn.id == id) else {
                    return;
                };
                let Some(index) = conn.buffer.iter().position(|byte| *byte == b'\n') else {
                    return;
                };
                let mut line: Vec<u8> = conn.buffer.drain(..=index).collect();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                line
            };
            if line.is_empty() {
                continue;
            }
            if !self.on_line(id, &String::from_utf8_lossy(&line)) {
                return;
            }
        }
    }

    /// `onLine(conn, line)`: `false` — соединение закрыто, остаток буфера не читать.
    fn on_line(self: &Arc<Self>, id: u64, line: &str) -> bool {
        let at = (self.now)();
        let step = {
            let mut inner = self.lock();
            let Inner {
                connection,
                stats,
                discord_connected,
                ..
            } = &mut *inner;
            let Some(conn) = connection.as_mut().filter(|conn| conn.id == id) else {
                return false;
            };

            if at - conn.window_start_ms >= 1000 {
                conn.window_start_ms = at;
                conn.window_count = 0;
            }
            conn.window_count += 1;
            if conn.window_count > self.limits.max_messages_per_second {
                Step::Reject("RATE_LIMIT".to_owned())
            } else {
                match protocol::parse_line(line) {
                    Err(reason) => Step::Reject(reason),
                    Ok(message) => self.route(conn, stats, *discord_connected, message),
                }
            }
        };
        match step {
            Step::Reject(reason) => {
                self.reject(id, &reason);
                false
            }
            Step::Ready => {
                if let Some(handler) = &self.handlers.on_game_ready {
                    self.guarded("обработчик рукопожатия упал", || handler());
                }
                true
            }
            Step::State(message) => {
                if let Some(handler) = &self.handlers.on_state {
                    self.guarded("обработчик состояния упал", || {
                        handler(&message)
                    });
                }
                true
            }
            Step::Chat(message) => {
                if let Some(handler) = &self.handlers.on_chat {
                    self.guarded("обработчик чата упал", || {
                        handler(&message)
                    });
                }
                true
            }
            Step::Nothing => true,
        }
    }

    /// Разбор принятого сообщения (под замком).
    fn route(
        self: &Arc<Self>,
        conn: &mut Conn,
        stats: &mut Stats,
        discord_connected: bool,
        message: Message,
    ) -> Step {
        if !conn.authenticated {
            let Message::Hello { token, session } = message else {
                return Step::Reject("NO_HANDSHAKE".to_owned());
            };
            if !token_matches(&self.token, &token) {
                return Step::Reject("BAD_TOKEN".to_owned());
            }
            conn.authenticated = true;
            conn.session = Some(session);
            stats.accepted += 1;
            self.arm_timeout(conn, self.limits.idle_timeout_ms, "IDLE_TIMEOUT");
            let _ = conn
                .writer
                .send(protocol::encode_discord_ack(discord_connected).into_bytes());
            return Step::Ready;
        }
        if matches!(message, Message::Hello { .. }) {
            return Step::Reject("DUPLICATE_HANDSHAKE".to_owned());
        }
        if conn.session.as_deref() != Some(message.session()) {
            stats.dropped += 1;
            return Step::Nothing;
        }
        stats.messages += 1;
        self.arm_timeout(conn, self.limits.idle_timeout_ms, "IDLE_TIMEOUT");
        match message {
            Message::State(state) => Step::State(state),
            other => other.to_chat().map_or(Step::Nothing, Step::Chat),
        }
    }
}

fn error_code(error: &std::io::Error) -> String {
    errno::code(error).map_or_else(|| "ERR".to_owned(), std::borrow::Cow::into_owned)
}
