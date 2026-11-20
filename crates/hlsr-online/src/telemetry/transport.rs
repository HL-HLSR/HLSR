//! Транспорт API (`telemetry/transport.js`): `request`, `retryAfterMs`, `userAgent`.
//!
//! HTTP/1.1 без сжатия и системного прокси (`accept-encoding: identity`), без редиректов,
//! TLS — rustls с ring и корнями Mozilla, как у доставки (`hlsr_delivery::http_client::client_builder`).
//! Схема `https:`; `http:` — только на `127.0.0.1`/`localhost`/`::1` и только в отладочной
//! сборке или с `dev-hooks` (в выпуске 0.4.7 это был QA-ход; спецификация, решение 2).
//! Тайм-ауты: соединение — 5 с (`CONNECT_TIMEOUT_MS` доставки, как у `globalAgent` Node 24),
//! далее простой на ответ и на каждый кусок тела — `timeoutMs` (15 с по умолчанию). Запасного
//! пути через Chromium для API у 0.4.7 нет, и здесь его нет.
//!
//! Отступление от 0.4.7 (спецификация, решение 6): ответ больше лимита. Node рвёт ответ
//! (`transport.js:54-56`) и либо завершает промис пустым телом, либо не завершает вовсе; здесь
//! запрос завершается ошибкой `TELEMETRY_NETWORK`.

use hlsr_core::js::{date_parse, to_number};
use hlsr_delivery::http_client::{client_builder, is_loopback, node_header, url_hostname};
use hlsr_delivery::providers::BoxFuture;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::time::Duration;
use url::Url;

/// Лимит ответа по умолчанию — и нижняя граница `maxResponseBytes`.
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024;
/// Верхняя граница `maxResponseBytes`.
const MAX_RESPONSE_BYTES_CAP: usize = 2 * 1024 * 1024;
/// Тайм-аут по умолчанию (`opts.timeoutMs || 15000`).
pub const DEFAULT_TIMEOUT_MS: u64 = 15_000;

/// `userAgent(launcherVersion)`: `HLSR-Launcher/<версия>`, без версии — `0.0.0`.
pub fn user_agent(launcher_version: Option<&str>) -> String {
    let version = launcher_version.filter(|version| !version.is_empty());
    format!("HLSR-Launcher/{}", version.unwrap_or("0.0.0"))
}

/// Параметры запроса (`options` в `request(url, options)`).
#[derive(Debug, Clone, Default)]
pub struct RequestOptions {
    /// `method`: по умолчанию `GET`; строчные буквы приводятся к прописным, как у `http.request`.
    pub method: Option<String>,
    /// `headers`: поверх `user-agent`, `accept`, `accept-encoding` (тот же ключ без учёта
    /// регистра их заменяет). `content-type` и `content-length` при теле задаёт транспорт.
    pub headers: Vec<(String, String)>,
    /// `body`: текст UTF-8. Есть тело (даже пустое) — `content-type: application/json` и
    /// `content-length`.
    pub body: Option<String>,
    /// `timeoutMs`: простой соединения; `None` или 0 — 15 с.
    pub timeout_ms: Option<u64>,
    /// `maxResponseBytes`: от 8 КиБ до 2 МиБ (меньшее и большее подтягиваются к границе).
    pub max_response_bytes: Option<usize>,
    /// `launcherVersion` для `user-agent`.
    pub launcher_version: Option<String>,
}

/// Заголовки ответа как объект `response.headers` Node: имена строчные, повтор склеен через
/// `, ` (у заголовков, где Node берёт первое значение, — первое). Порядок — не порядок на
/// проводе.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers(Vec<(String, String)>);

impl Headers {
    /// Заголовки из пар. Имена хранятся как заданы: [`Headers::get`] ищет точное имя, как
    /// `headers["retry-after"]` в JS.
    pub fn from_pairs<K: Into<String>, V: Into<String>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Headers {
        Headers(
            pairs
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        )
    }

    /// `headers[name]`: значение или `None`.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    fn from_map(map: &HeaderMap) -> Headers {
        let mut pairs: Vec<(String, String)> = Vec::new();
        for name in map.keys() {
            if let Some(value) = node_header(map, name.as_str()) {
                pairs.push((name.as_str().to_owned(), value));
            }
        }
        Headers(pairs)
    }
}

/// Ответ: `{status, headers, body}`.
#[derive(Debug, Clone)]
pub struct Response {
    /// `statusCode`.
    pub status: u16,
    pub headers: Headers,
    /// Тело текстом UTF-8; неверные байты — U+FFFD.
    pub body: String,
}

/// Ошибка запроса. Сообщение (`Display`) — как `error.message` в 0.4.7: код `TELEMETRY_*`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// Схема не `http:` и не `https:`.
    #[error("TELEMETRY_BAD_SCHEME")]
    BadScheme,
    /// `http:` не на loopback (и в выпуске без `dev-hooks` — любой `http:`).
    #[error("TELEMETRY_INSECURE")]
    Insecure,
    /// Нет соединения, обрыв, ошибка TLS, ответ больше лимита.
    #[error("TELEMETRY_NETWORK")]
    Network,
    /// Соединение или ответ молчали дольше тайм-аута.
    #[error("TELEMETRY_TIMEOUT")]
    Timeout,
    /// `new URL(url)` не разобрал адрес (`TypeError`, код `ERR_INVALID_URL`).
    #[error("Invalid URL")]
    InvalidUrl,
    /// Метод или заголовок, который `http.request` не принял бы (`ERR_INVALID_CHAR`,
    /// `ERR_INVALID_HTTP_TOKEN`).
    #[error("Invalid request")]
    InvalidRequest,
}

impl TransportError {
    /// `error.code` исключения Node; у ошибок `TELEMETRY_*` его нет.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            TransportError::InvalidUrl => Some("ERR_INVALID_URL"),
            _ => None,
        }
    }
}

/// Транспорт API: службы онлайна берут его как `opts.request || transport.request`, чтобы
/// тесты подставляли свой.
pub trait Transport: Send + Sync {
    fn request<'a>(
        &'a self,
        url: &'a str,
        options: &'a RequestOptions,
    ) -> BoxFuture<'a, Result<Response, TransportError>>;
}

/// Транспорт по сети; копии делят пул соединений (как `globalAgent` Node).
#[derive(Debug, Clone)]
pub struct HttpTransport {
    client: reqwest::Client,
}

impl Default for HttpTransport {
    fn default() -> Self {
        HttpTransport::new()
    }
}

impl HttpTransport {
    pub fn new() -> HttpTransport {
        let client = client_builder()
            .build()
            .expect("клиент HTTP собирается из постоянных настроек");
        HttpTransport { client }
    }

    /// `request(url, options)`.
    pub async fn request(
        &self,
        url: &str,
        options: &RequestOptions,
    ) -> Result<Response, TransportError> {
        let max = options
            .max_response_bytes
            .unwrap_or(0)
            .clamp(MAX_RESPONSE_BYTES, MAX_RESPONSE_BYTES_CAP);
        let target = Url::parse(url).map_err(|_| TransportError::InvalidUrl)?;
        match target.scheme() {
            "https" => {}
            "http" => {
                let allowed = cfg!(any(debug_assertions, feature = "dev-hooks"))
                    && is_loopback(url_hostname(&target));
                if !allowed {
                    return Err(TransportError::Insecure);
                }
            }
            _ => return Err(TransportError::BadScheme),
        }
        let idle = Duration::from_millis(
            options
                .timeout_ms
                .filter(|&ms| ms != 0)
                .unwrap_or(DEFAULT_TIMEOUT_MS),
        );

        let method = options.method.as_deref().filter(|m| !m.is_empty());
        let method =
            reqwest::Method::from_bytes(method.unwrap_or("GET").to_ascii_uppercase().as_bytes())
                .map_err(|_| TransportError::InvalidRequest)?;
        let mut headers = HeaderMap::new();
        let mut put = |name: &str, value: &[u8]| -> Result<(), TransportError> {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| TransportError::InvalidRequest)?;
            let value =
                HeaderValue::from_bytes(value).map_err(|_| TransportError::InvalidRequest)?;
            headers.insert(name, value);
            Ok(())
        };
        put(
            "user-agent",
            user_agent(options.launcher_version.as_deref()).as_bytes(),
        )?;
        put("accept", b"application/json")?;
        put("accept-encoding", b"identity")?;
        for (name, value) in &options.headers {
            // Значения заголовков Node пишет в latin1.
            let bytes: Vec<u8> = value
                .chars()
                .map(|c| u8::try_from(u32::from(c)).map_err(|_| TransportError::InvalidRequest))
                .collect::<Result<_, _>>()?;
            put(name, &bytes)?;
        }
        let payload = options.body.as_ref().map(|body| body.as_bytes().to_vec());
        if let Some(payload) = &payload {
            put("content-type", b"application/json")?;
            put("content-length", payload.len().to_string().as_bytes())?;
        } else if !matches!(
            method.as_str(),
            "GET" | "HEAD" | "DELETE" | "OPTIONS" | "TRACE" | "CONNECT"
        ) {
            // Node без тела на таких методах пишет `content-length: 0`.
            put("content-length", b"0")?;
        }

        let mut request = self.client.request(method, target).headers(headers);
        if let Some(payload) = payload {
            request = request.body(payload);
        }
        let mut response = match tokio::time::timeout(idle, request.send()).await {
            Err(_) => return Err(TransportError::Timeout),
            Ok(Err(error)) if error.is_timeout() => return Err(TransportError::Timeout),
            Ok(Err(_)) => return Err(TransportError::Network),
            Ok(Ok(response)) => response,
        };

        let status = response.status().as_u16();
        let headers = Headers::from_map(response.headers());
        let mut body = Vec::new();
        loop {
            match tokio::time::timeout(idle, response.chunk()).await {
                Err(_) => return Err(TransportError::Timeout),
                Ok(Err(_)) => return Err(TransportError::Network),
                Ok(Ok(None)) => break,
                Ok(Ok(Some(chunk))) => {
                    if body.len() + chunk.len() > max {
                        return Err(TransportError::Network);
                    }
                    body.extend_from_slice(&chunk);
                }
            }
        }
        Ok(Response {
            status,
            headers,
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }
}

impl Transport for HttpTransport {
    fn request<'a>(
        &'a self,
        url: &'a str,
        options: &'a RequestOptions,
    ) -> BoxFuture<'a, Result<Response, TransportError>> {
        Box::pin(HttpTransport::request(self, url, options))
    }
}

/// Транспорт без сети: каждый запрос сразу завершается `TELEMETRY_NETWORK`, как при обрыве связи.
/// Для отладочных запусков и смоуков, которым нельзя ходить в настоящий `api.hlsr.pro`.
#[derive(Debug, Clone, Copy, Default)]
pub struct OfflineTransport;

impl Transport for OfflineTransport {
    fn request<'a>(
        &'a self,
        _url: &'a str,
        _options: &'a RequestOptions,
    ) -> BoxFuture<'a, Result<Response, TransportError>> {
        Box::pin(async { Err(TransportError::Network) })
    }
}

/// `retryAfterMs(headers, nowMs)`: пауза из `retry-after` в миллисекундах — секунды или дата
/// HTTP, не больше часа; прошедшая дата — 0; нет заголовка или не разобрать — `None`.
/// `now_ms` — `Date.now()`; `None` и 0 — текущее время, как `nowMs || Date.now()`.
pub fn retry_after_ms(headers: &Headers, now_ms: Option<i64>) -> Option<f64> {
    let raw = headers.get("retry-after").filter(|raw| !raw.is_empty())?;
    let seconds = to_number(raw.trim());
    if seconds.is_finite() && seconds >= 0.0 {
        return Some(seconds.min(3600.0) * 1000.0);
    }
    let date = date_parse(raw)?;
    let now = now_ms.filter(|&ms| ms != 0).unwrap_or_else(now_millis);
    let delta = (date - now) as f64;
    Some(if delta > 0.0 {
        delta.min(3_600_000.0)
    } else {
        0.0
    })
}

fn now_millis() -> i64 {
    hlsr_delivery::timing::system_clock()()
}
