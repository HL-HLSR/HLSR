//! Двойник сервера для сверки онлайна: локальный HTTP-сервер (только 127.0.0.1) по сценарию, с
//! журналом запросов. Тот же формат сценария и журнала у Node — `tools/reference/online/lib/
//! server.js` (описание формата — в его заголовке): Node 24 (модули 0.4.7) и Rust гоняют одни
//! сценарии, а потом сравнивают журналы.
//!
//! Сценарий (`routes`): `{"<ключ>": [шаг, …]}`; ключ — `"METHOD /путь?запрос"`, `"/путь?запрос"`,
//! `"METHOD /путь"` или `"/путь"` (самая точная запись главнее). Шаги отдаются по очереди, последний
//! повторяется; нет ключа — 404 с пустым телом. Шаг: `status`, `headers` (`[[имя, значение], …]`),
//! `body` | `bodyHex` | `bodySize` (+ `bodyOffset`) | `json`, `contentLength` (число или `null` —
//! без заголовка, тело кусками), `delayMs`, `hold` (ждать `Server::release(имя)`), `cutAfter`,
//! `pauseAfter` + `pauseMs`, `hangUp`. Динамический ответ вместо шагов — `Server::handle`.
//!
//! Журнал: `{server, method, path, query, headers, body}`; заголовки строчными, без `host`,
//! `connection`, `date`, `keep-alive`, значение `x-hlsr-ticket` — `<ticket>`; тело — текст UTF-8
//! или `null`, если у запроса нет ни `content-length`, ни `transfer-encoding`.

#![allow(dead_code)]

use hlsr_core::json_formats::compact;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Заголовки запроса, которых нет в журнале.
pub const VOLATILE_HEADERS: [&str; 4] = ["host", "connection", "date", "keep-alive"];

/// Журнал запросов всех серверов сценария.
pub type Journal = Arc<Mutex<Vec<Value>>>;

/// Динамический ответ: (запись журнала, номер запроса по ключу) → шаг.
pub type Handler = Arc<dyn Fn(&Value, usize) -> Value + Send + Sync>;

struct State {
    name: String,
    routes: Mutex<Map<String, Value>>,
    handlers: Mutex<HashMap<String, Handler>>,
    counters: Mutex<HashMap<String, usize>>,
    gates: Mutex<HashMap<String, watch::Sender<bool>>>,
    journal: Journal,
}

impl State {
    fn gate(&self, name: &str) -> watch::Sender<bool> {
        self.gates
            .lock()
            .unwrap()
            .entry(name.to_owned())
            .or_insert_with(|| watch::channel(false).0)
            .clone()
    }
}

pub struct Server {
    pub name: String,
    pub port: u16,
    /// `127.0.0.1:<порт>`.
    pub authority: String,
    state: Arc<State>,
    accept: JoinHandle<()>,
}

impl Server {
    /// Сценарий сервера; счётчики шагов начинаются заново.
    pub fn set_routes(&self, routes: &Value) {
        *self.state.routes.lock().unwrap() = routes.as_object().cloned().unwrap_or_default();
        self.state.counters.lock().unwrap().clear();
    }

    /// Динамический ответ по ключу вместо списка шагов.
    pub fn handle(
        &self,
        key: &str,
        handler: impl Fn(&Value, usize) -> Value + Send + Sync + 'static,
    ) {
        self.state
            .handlers
            .lock()
            .unwrap()
            .insert(key.to_owned(), Arc::new(handler));
    }

    /// Открыть ворота `name`: ждущие продолжают, будущие не ждут.
    pub fn release(&self, name: &str) {
        self.state.gate(name).send_replace(true);
    }

    /// Открывалка ворот, которую можно отдать другой задаче.
    pub fn releaser(&self) -> impl Fn(&str) + Send + 'static {
        let state = self.state.clone();
        move |name: &str| {
            state.gate(name).send_replace(true);
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

/// Запустить сервер `name`; запросы пишутся в `journal`.
pub async fn start_server(name: &str, journal: Journal) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("сервер сценария");
    let port = listener.local_addr().expect("адрес сервера").port();
    let state = Arc::new(State {
        name: name.to_owned(),
        routes: Mutex::default(),
        handlers: Mutex::default(),
        counters: Mutex::default(),
        gates: Mutex::default(),
        journal,
    });
    let accept = {
        let state = state.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(state.clone(), stream));
            }
        })
    };
    Server {
        name: name.to_owned(),
        port,
        authority: format!("127.0.0.1:{port}"),
        state,
        accept,
    }
}

/// Порт, на котором никто не слушает (соединение будет отвергнуто).
pub async fn dead_port() -> u16 {
    let probe = TcpListener::bind("127.0.0.1:0").await.expect("порт");
    probe.local_addr().expect("адрес").port()
}

struct Request {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

async fn serve(state: Arc<State>, mut stream: TcpStream) {
    let mut buffer = Vec::new();
    while let Some(request) = read_request(&mut stream, &mut buffer).await {
        if !respond(&state, &mut stream, request).await {
            return;
        }
    }
}

fn journal_entry(name: &str, request: &Request) -> Value {
    let (path, query) = match request.target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (request.target.as_str(), ""),
    };
    let mut headers = Map::new();
    for (key, value) in &request.headers {
        if VOLATILE_HEADERS.contains(&key.as_str()) {
            continue;
        }
        let value = if key == "x-hlsr-ticket" {
            "<ticket>".to_owned()
        } else {
            value.clone()
        };
        match headers.get_mut(key) {
            // Повтор заголовка Node склеивает через запятую.
            Some(Value::String(joined)) => {
                joined.push_str(", ");
                joined.push_str(&value);
            }
            _ => {
                headers.insert(key.clone(), Value::String(value));
            }
        }
    }
    let framed = request
        .headers
        .iter()
        .any(|(key, _)| key == "content-length" || key == "transfer-encoding");
    json!({
        "server": name,
        "method": request.method,
        "path": path,
        "query": query,
        "headers": headers,
        "body": if framed { json!(String::from_utf8_lossy(&request.body)) } else { Value::Null },
    })
}

/// Ответить; `false` — соединение закрыто.
async fn respond(state: &State, stream: &mut TcpStream, request: Request) -> bool {
    let entry = journal_entry(&state.name, &request);
    state.journal.lock().unwrap().push(entry.clone());
    let path = entry["path"].as_str().unwrap().to_owned();

    let found = {
        let routes = state.routes.lock().unwrap();
        let handlers = state.handlers.lock().unwrap();
        [
            format!("{} {}", request.method, request.target),
            request.target.clone(),
            format!("{} {}", request.method, path),
            path,
        ]
        .into_iter()
        .find_map(|key| {
            if let Some(handler) = handlers.get(&key) {
                return Some((key, None, Some(handler.clone())));
            }
            let list = routes.get(&key).and_then(Value::as_array)?;
            (!list.is_empty()).then(|| (key.clone(), Some(list.clone()), None))
        })
    };
    let Some((key, list, handler)) = found else {
        return write(
            stream,
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
        )
        .await;
    };
    let index = {
        let mut counters = state.counters.lock().unwrap();
        let index = counters.entry(key).or_default();
        let current = *index;
        *index += 1;
        current
    };
    let step = match (handler, list) {
        (Some(handler), _) => handler(&entry, index),
        (None, Some(list)) => list[index.min(list.len() - 1)].clone(),
        (None, None) => unreachable!("маршрут есть либо в сценарии, либо среди обработчиков"),
    };

    if let Some(ms) = step.get("delayMs").and_then(Value::as_u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
    if let Some(gate) = step.get("hold").and_then(Value::as_str) {
        let mut open = state.gate(gate).subscribe();
        let _ = open.wait_for(|open| *open).await;
    }
    if step.get("hangUp").and_then(Value::as_bool) == Some(true) {
        return false;
    }

    let body = body_of(&step);
    let status = step.get("status").and_then(Value::as_u64).unwrap_or(200);
    let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status)).into_bytes();
    let mut has_content_type = false;
    for pair in step
        .get("headers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (name, value) = (pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
        has_content_type |= name.eq_ignore_ascii_case("content-type");
        head.extend_from_slice(name.as_bytes());
        head.extend_from_slice(b": ");
        // Заголовки HTTP — latin1, как пишет Node.
        head.extend(value.chars().map(|c| u8::try_from(u32::from(c)).unwrap()));
        head.extend_from_slice(b"\r\n");
    }
    if step.get("json").is_some() && !has_content_type {
        head.extend_from_slice(b"content-type: application/json\r\n");
    }
    let close_delimited = matches!(step.get("contentLength"), Some(Value::Null));
    // Без Content-Length Node отдаёт тело кусками; ответ на HEAD тела не имеет.
    let chunked = close_delimited && request.method != "HEAD";
    match step.get("contentLength") {
        Some(Value::Null) => head.extend_from_slice(b"Connection: close\r\n"),
        Some(length) => head.extend_from_slice(format!("Content-Length: {length}\r\n").as_bytes()),
        None => head.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes()),
    }
    if chunked {
        head.extend_from_slice(b"Transfer-Encoding: chunked\r\n");
    }
    head.extend_from_slice(b"\r\n");
    if !write(stream, &head).await {
        return false;
    }

    let payload: &[u8] = if request.method == "HEAD" { &[] } else { &body };
    if let Some(cut) = step.get("cutAfter").and_then(Value::as_u64) {
        let cut = (cut as usize).min(payload.len());
        write_body(stream, chunked, &payload[..cut]).await;
        // Дать байтам уйти до обрыва.
        tokio::time::sleep(Duration::from_millis(20)).await;
        return false;
    }
    if let Some(pause) = step.get("pauseAfter").and_then(Value::as_u64) {
        let pause = (pause as usize).min(payload.len());
        if !write_body(stream, chunked, &payload[..pause]).await {
            return false;
        }
        let ms = step.get("pauseMs").and_then(Value::as_u64).unwrap_or(0);
        tokio::time::sleep(Duration::from_millis(ms)).await;
        return write_body(stream, chunked, &payload[pause..]).await
            && finish_body(stream, chunked).await
            && !close_delimited;
    }
    write_body(stream, chunked, payload).await
        && finish_body(stream, chunked).await
        && !close_delimited
}

/// Кусок тела: с длиной в шестнадцатеричной записи при `chunked`, иначе как есть. Пустой кусок
/// не пишется (нулевая длина — конец тела).
async fn write_body(stream: &mut TcpStream, chunked: bool, bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return true;
    }
    if !chunked {
        return write(stream, bytes).await;
    }
    let mut framed = format!("{:x}\r\n", bytes.len()).into_bytes();
    framed.extend_from_slice(bytes);
    framed.extend_from_slice(b"\r\n");
    write(stream, &framed).await
}

async fn finish_body(stream: &mut TcpStream, chunked: bool) -> bool {
    !chunked || write(stream, b"0\r\n\r\n").await
}

fn body_of(step: &Value) -> Vec<u8> {
    if let Some(hex) = step.get("bodyHex").and_then(Value::as_str) {
        return (0..hex.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).expect("шестнадцатеричная запись"))
            .collect();
    }
    if let Some(size) = step.get("bodySize").and_then(Value::as_u64) {
        let offset = step.get("bodyOffset").and_then(Value::as_u64).unwrap_or(0);
        return (0..size).map(|i| ((offset + i) % 251) as u8).collect();
    }
    if let Some(value) = step.get("json") {
        return compact(value).into_bytes();
    }
    step.get("body")
        .and_then(Value::as_str)
        .unwrap_or("")
        .as_bytes()
        .to_vec()
}

fn reason(status: u64) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

async fn write(stream: &mut TcpStream, bytes: &[u8]) -> bool {
    stream.write_all(bytes).await.is_ok() && stream.flush().await.is_ok()
}

/// Запрос целиком (заголовок и тело); `None` — соединение закрыто.
async fn read_request(stream: &mut TcpStream, buffer: &mut Vec<u8>) -> Option<Request> {
    let end = loop {
        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
        fill(stream, buffer).await?;
    };
    // Заголовки HTTP — latin1, как читает Node.
    let head: String = buffer[..end].iter().map(|&b| char::from(b)).collect();
    buffer.drain(..end + 4);
    let mut lines = head.split("\r\n");
    let mut start = lines.next()?.split(' ');
    let method = start.next()?.to_owned();
    let target = start.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let find = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, v)| v.as_str())
    };

    let mut body = Vec::new();
    if find("transfer-encoding").is_some_and(|value| value.to_ascii_lowercase().contains("chunked"))
    {
        loop {
            let line_end = loop {
                if let Some(at) = buffer.windows(2).position(|w| w == b"\r\n") {
                    break at;
                }
                fill(stream, buffer).await?;
            };
            let size_text = String::from_utf8_lossy(&buffer[..line_end]).into_owned();
            buffer.drain(..line_end + 2);
            let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
            while buffer.len() < size + 2 {
                fill(stream, buffer).await?;
            }
            body.extend_from_slice(&buffer[..size]);
            buffer.drain(..size + 2);
            if size == 0 {
                break;
            }
        }
    } else if let Some(length) = find("content-length").and_then(|v| v.parse::<usize>().ok()) {
        while buffer.len() < length {
            fill(stream, buffer).await?;
        }
        body = buffer.drain(..length).collect();
    }
    Some(Request {
        method,
        target,
        headers,
        body,
    })
}

async fn fill(stream: &mut TcpStream, buffer: &mut Vec<u8>) -> Option<()> {
    let mut chunk = [0u8; 8192];
    match stream.read(&mut chunk).await {
        Ok(0) | Err(_) => None,
        Ok(n) => {
            buffer.extend_from_slice(&chunk[..n]);
            Some(())
        }
    }
}
