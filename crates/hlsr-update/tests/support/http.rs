//! Локальный HTTP-сервер по сценарию — тот же формат читает Node
//! (`tools/reference/delivery/lib/http-server.js`, общий с `tools/reference/update`). Только 127.0.0.1.
//!
//! Маршруты: `{ "<путь без запроса>": [ответ, ответ, …] }`. Запросы к пути получают
//! ответы по очереди; когда список кончился, повторяется последний. Путь без маршрута —
//! 404 с пустым телом. Ключом может быть и путь с запросом (`/download?id=A&export=download`):
//! он главнее пути без запроса. Ответ: `status`, `headers` (`[[имя, значение], …]`), `body`, `bodyHex`
//! (байты в шестнадцатеричной записи) или `bodySize` (байты `i % 251`), `contentLength` (число; `null` — без заголовка, тело до
//! закрытия соединения), `delayMs` (пауза перед заголовками), `cutAfter` (отдать столько
//! байт тела и закрыть соединение), `pauseAfter` + `pauseMs` (пауза посреди тела),
//! `hangUp` (закрыть соединение без ответа), `bodyOffset` (байты тела — `(bodyOffset + i) % 251`:
//! хвост того же файла для докачки), `gates` (`[{at, name}, …]`: отдать тело до байта `at` и
//! ждать `Server::release(name)`; открытые раньше ворота не задерживают; вместе с `cutAfter` —
//! тело кусками до `cutAfter` байт и обрыв).
//!
//! Без `Content-Length` (`contentLength: null`) тело идёт кусками (`Transfer-Encoding:
//! chunked`) и соединение закрывается — как у сервера Node: обрыв посреди такого тела для
//! клиента ошибка, а не конец тела.
//!
//! Журнал запросов: `{server, method, url, headers: {user-agent, accept, range, if-none-match,
//! if-modified-since}}` — только те заголовки, что клиент прислал.

use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

const LOGGED_HEADERS: [&str; 5] = [
    "user-agent",
    "accept",
    "range",
    "if-none-match",
    "if-modified-since",
];

/// Журнал запросов всех серверов сценария.
pub type RequestLog = Arc<Mutex<Vec<Value>>>;

/// Маршруты сервера; задаются после запуска (в ответах бывают адреса других серверов).
pub type Routes = Arc<Mutex<Map<String, Value>>>;

/// Ворота: имя → открыты ли. Открытые остаются открытыми.
type Gates = Arc<Mutex<HashMap<String, watch::Sender<bool>>>>;

fn gate(gates: &Gates, name: &str) -> watch::Sender<bool> {
    gates
        .lock()
        .unwrap()
        .entry(name.to_owned())
        .or_insert_with(|| watch::channel(false).0)
        .clone()
}

pub struct Server {
    pub name: String,
    pub port: u16,
    /// `127.0.0.1:<порт>`.
    pub authority: String,
    pub routes: Routes,
    gates: Gates,
    accept: JoinHandle<()>,
}

impl Server {
    /// Открыть ворота `name`: ждущие продолжают, будущие не ждут.
    pub fn release(&self, name: &str) {
        gate(&self.gates, name).send_replace(true);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

/// Запустить сервер `name`; запросы пишутся в `log`.
pub async fn start_server(name: &str, log: RequestLog) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("сервер сценария");
    let port = listener.local_addr().expect("адрес сервера").port();
    let routes: Routes = Arc::default();
    let counters: Arc<Mutex<HashMap<String, usize>>> = Arc::default();
    let gates: Gates = Arc::default();
    let accept = {
        let (name, routes, gates) = (name.to_owned(), routes.clone(), gates.clone());
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let state = Connection {
                    name: name.clone(),
                    routes: routes.clone(),
                    counters: counters.clone(),
                    gates: gates.clone(),
                    log: log.clone(),
                };
                tokio::spawn(state.serve(stream));
            }
        })
    };
    Server {
        name: name.to_owned(),
        port,
        authority: format!("127.0.0.1:{port}"),
        routes,
        gates,
        accept,
    }
}

/// Порт, на котором никто не слушает (соединение будет отвергнуто).
///
/// В Windows — порт 0: отказ в соединении с закрытым портом loopback там приходит только после
/// повторов SYN (около 2 с; у Node 0.4.7 в Windows так же), и тайм-ауты сценариев, снятых в
/// Linux, где отказ мгновенный, срабатывали раньше него. Соединение с портом 0 Winsock отвергает
/// сразу (`WSAEADDRNOTAVAIL`); ожидания `connect ECONNREFUSED {dead}` в Windows читаются с
/// этим кодом — [`super::Mismatches::check`].
pub async fn dead_port() -> u16 {
    if cfg!(windows) {
        return 0;
    }
    let probe = TcpListener::bind("127.0.0.1:0").await.expect("порт");
    probe.local_addr().expect("адрес").port()
}

/// «Чёрная дыра» (только Linux): сокет слушает с очередью 0 и не принимает соединений;
/// очередь заполнена, новые SYN ядро отбрасывает — соединение висит, как с недоступным
/// хостом.
#[cfg(target_os = "linux")]
pub struct Blackhole {
    /// `127.0.0.1:<порт>`.
    pub authority: String,
    _listener: TcpListener,
    _fillers: Vec<TcpStream>,
}

#[cfg(target_os = "linux")]
pub async fn blackhole() -> Blackhole {
    let socket = tokio::net::TcpSocket::new_v4().expect("сокет");
    socket.bind("127.0.0.1:0".parse().unwrap()).expect("адрес");
    let listener = socket.listen(0).expect("listen");
    let address = listener.local_addr().expect("адрес");
    // Заполнить очередь: соединяться, пока очередное не повиснет.
    let mut fillers = Vec::new();
    while let Ok(Ok(stream)) =
        tokio::time::timeout(Duration::from_millis(300), TcpStream::connect(address)).await
    {
        fillers.push(stream);
    }
    Blackhole {
        authority: address.to_string(),
        _listener: listener,
        _fillers: fillers,
    }
}

// ---------------------------------------------------------------- подстановка адресов

/// `{имя}` → адрес сервера (`127.0.0.1:<порт>`) в тексте.
pub fn fill_text(text: &str, map: &std::collections::BTreeMap<String, String>) -> String {
    let mut out = text.to_owned();
    for (name, authority) in map {
        out = out.replace(&format!("{{{name}}}"), authority);
    }
    out
}

/// Подстановка адресов: `{имя}` → `127.0.0.1:<порт>` во всех строках и ключах (`fill` скриптов).
pub fn fill(value: &Value, map: &std::collections::BTreeMap<String, String>) -> Value {
    match value {
        Value::String(text) => Value::String(fill_text(text, map)),
        Value::Array(list) => Value::Array(list.iter().map(|v| fill(v, map)).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(k, v)| (fill_text(k, map), fill(v, map)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Обратная замена (`unfill` скриптов): адрес → `{имя}`, длинные адреса первыми. В строках с
/// `{root}` разделитель путей Windows (`\`) становится `/`: фикстуры сняты на Linux.
pub fn unfill(value: &Value, map: &std::collections::BTreeMap<String, String>) -> Value {
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
        Value::Array(list) => Value::Array(list.iter().map(|v| unfill(v, map)).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(k, v)| (text(k), unfill(v, map)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Порча байтов тела маршрута (`mutate` сценариев): последний байт, лишний нулевой, на байт короче.
pub fn mutate(mut bytes: Vec<u8>, how: Option<&str>) -> Vec<u8> {
    match how {
        None => {}
        Some("flipLast") => {
            if let Some(last) = bytes.last_mut() {
                *last ^= 0xff;
            }
        }
        Some("append") => bytes.push(0),
        Some("truncate") => {
            bytes.pop();
        }
        Some(other) => panic!("неизвестная порча {other}"),
    }
    bytes
}

/// Журнал запросов Rust: reqwest всегда шлёт `Accept: */*`, Node — нет.
pub fn without_default_accept(log: &[Value]) -> Vec<Value> {
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

/// Сервер с одним маршрутом и одним ответом — для самотестов.
pub async fn serve_one(path: &str, response: Value) -> (Server, RequestLog) {
    let log = RequestLog::default();
    let server = start_server("a", log.clone()).await;
    server
        .routes
        .lock()
        .unwrap()
        .insert(path.to_owned(), json!([response]));
    (server, log)
}

struct Connection {
    name: String,
    routes: Routes,
    counters: Arc<Mutex<HashMap<String, usize>>>,
    gates: Gates,
    log: RequestLog,
}

struct Request {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
}

impl Connection {
    async fn serve(self, mut stream: TcpStream) {
        let mut buffer = Vec::new();
        while let Some(request) = read_request(&mut stream, &mut buffer).await {
            if !self.respond(&mut stream, request).await {
                return;
            }
        }
    }

    /// Ответить; `false` — соединение закрыто.
    async fn respond(&self, stream: &mut TcpStream, request: Request) -> bool {
        let pathname = {
            let path = request
                .url
                .split(['?', '#'])
                .next()
                .unwrap_or("")
                .to_owned();
            let full = self
                .routes
                .lock()
                .unwrap()
                .get(&request.url)
                .is_some_and(|list| list.as_array().is_some_and(|list| !list.is_empty()));
            if full { request.url.clone() } else { path }
        };
        let mut headers = Map::new();
        for name in LOGGED_HEADERS {
            if let Some((_, value)) = request.headers.iter().find(|(n, _)| n == name) {
                headers.insert(name.to_owned(), json!(value));
            }
        }
        self.log.lock().unwrap().push(json!({
            "server": self.name,
            "method": request.method,
            "url": request.url,
            "headers": headers,
        }));

        let step = {
            let routes = self.routes.lock().unwrap();
            let list = routes.get(&pathname).and_then(Value::as_array).cloned();
            match list.filter(|list| !list.is_empty()) {
                None => None,
                Some(list) => {
                    let mut counters = self.counters.lock().unwrap();
                    let index = counters.entry(pathname.clone()).or_default();
                    let step = list[(*index).min(list.len() - 1)].clone();
                    *index += 1;
                    Some(step)
                }
            }
        };
        let Some(step) = step else {
            return write(
                stream,
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
            )
            .await;
        };

        if let Some(ms) = step.get("delayMs").and_then(Value::as_u64) {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
        if step.get("hangUp").and_then(Value::as_bool) == Some(true) {
            return false;
        }

        let body = body_of(&step);
        let status = step.get("status").and_then(Value::as_u64).unwrap_or(200);
        let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status)).into_bytes();
        for pair in step
            .get("headers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let (name, value) = (pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
            head.extend_from_slice(name.as_bytes());
            head.extend_from_slice(b": ");
            // Заголовки HTTP — latin1, как пишет Node.
            head.extend(value.chars().map(|c| u8::try_from(u32::from(c)).unwrap()));
            head.extend_from_slice(b"\r\n");
        }
        let close_delimited = matches!(step.get("contentLength"), Some(Value::Null));
        // Без Content-Length Node отдаёт тело кусками; ответ на HEAD тела не имеет.
        let chunked = close_delimited && request.method != "HEAD";
        match step.get("contentLength") {
            Some(Value::Null) => head.extend_from_slice(b"Connection: close\r\n"),
            Some(length) => {
                head.extend_from_slice(format!("Content-Length: {length}\r\n").as_bytes())
            }
            None => {
                head.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes())
            }
        }
        if chunked {
            head.extend_from_slice(b"Transfer-Encoding: chunked\r\n");
        }
        head.extend_from_slice(b"\r\n");
        if !write(stream, &head).await {
            return false;
        }

        let payload: &[u8] = if request.method == "HEAD" { &[] } else { &body };
        if let Some(gates) = step.get("gates").and_then(Value::as_array) {
            let cut = step.get("cutAfter").and_then(Value::as_u64);
            let limit = cut.map_or(payload.len(), |cut| (cut as usize).min(payload.len()));
            let bytes = &payload[..limit];
            let mut points: Vec<(usize, &str)> = gates
                .iter()
                .map(|g| {
                    (
                        g["at"].as_u64().unwrap() as usize,
                        g["name"].as_str().unwrap(),
                    )
                })
                .collect();
            points.sort_by_key(|(at, _)| *at);
            let mut sent = 0;
            for (at, name) in points {
                if at <= sent || at >= bytes.len() {
                    continue;
                }
                if !write_body(stream, chunked, &bytes[sent..at]).await {
                    return false;
                }
                sent = at;
                let mut open = gate(&self.gates, name).subscribe();
                let _ = open.wait_for(|open| *open).await;
            }
            if !write_body(stream, chunked, &bytes[sent..]).await {
                return false;
            }
            if cut.is_some() {
                // Дать байтам уйти до обрыва.
                tokio::time::sleep(Duration::from_millis(20)).await;
                return false;
            }
            return finish_body(stream, chunked).await && !close_delimited;
        }
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
}

/// Кусок тела: с длиной в шестнадцатеричной записи при `chunked`, иначе как есть. Пустой
/// кусок не пишется (нулевая длина — конец тела).
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
    step.get("body")
        .and_then(Value::as_str)
        .unwrap_or("")
        .as_bytes()
        .to_vec()
}

fn reason(status: u64) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        206 => "Partial Content",
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        410 => "Gone",
        416 => "Range Not Satisfiable",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Unknown",
    }
}

async fn write(stream: &mut TcpStream, bytes: &[u8]) -> bool {
    stream.write_all(bytes).await.is_ok() && stream.flush().await.is_ok()
}

/// Заголовок запроса (тела у запросов клиента нет); `None` — соединение закрыто.
async fn read_request(stream: &mut TcpStream, buffer: &mut Vec<u8>) -> Option<Request> {
    loop {
        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
            buffer.drain(..end + 4);
            let mut lines = head.split("\r\n");
            let mut start = lines.next()?.split(' ');
            let method = start.next()?.to_owned();
            let url = start.next()?.to_owned();
            let headers = lines
                .filter_map(|line| line.split_once(':'))
                .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
                .collect();
            return Some(Request {
                method,
                url,
                headers,
            });
        }
        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
        }
    }
}
