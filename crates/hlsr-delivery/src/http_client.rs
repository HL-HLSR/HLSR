//! HTTP доставки (`http-client.js`): HTTP/1.1 без сжатия и системного прокси, UA
//! `HLSR-Launcher`, редиректы вручную (не больше 5, с https на http — только на loopback),
//! соединение не дольше 5 с, тайм-аут простоя на каждый запрос (30 с по умолчанию), чтение
//! тела с пределом и общим ограничителем скорости, `getJson`.
//!
//! TLS — rustls с ring и корнями Mozilla (`webpki-roots`): Node проверяет сертификаты
//! по встроенному списку Mozilla. Отмена (`signal` в 0.4.7) — сброс будущего или ответа.

use crate::download_rate_limiter::DownloadRateLimiter;
use crate::errors::{Code, DeliveryError, Failure, RawError};
use bytes::Bytes;
use hlsr_core::json_text::json_parse;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use std::error::Error as _;
use std::time::Duration;
use url::Url;

pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub const MAX_REDIRECTS: u32 = 5;
pub const USER_AGENT: &str = "HLSR-Launcher";

/// Построитель клиента, [`CONNECT_TIMEOUT_MS`] и [`is_loopback`] переехали в `hlsr-core::http`
/// (Р7 подпроекта 6: общий TLS для доставки, онлайна и обновления); здесь они доступны по-прежнему.
pub use hlsr_core::http::{CONNECT_TIMEOUT_MS, client_builder, is_loopback};

/// Предел тела по умолчанию (`readBody`) — 1 МБ.
const DEFAULT_BODY_LIMIT: usize = 1024 * 1024;
/// Предел тела `getJson` по умолчанию — 256 КБ.
const DEFAULT_JSON_LIMIT: usize = 256 * 1024;

/// `URL.protocol`: `https:`.
fn protocol(url: &Url) -> String {
    format!("{}:", url.scheme())
}

/// `URL.hostname`: IPv6 — в скобках.
pub fn url_hostname(url: &Url) -> &str {
    url.host_str().unwrap_or("")
}

/// `URL.host`: имя и порт, если он не стандартный для схемы.
pub fn url_host(url: &Url) -> String {
    match url.port() {
        Some(port) => format!("{}:{port}", url_hostname(url)),
        None => url_hostname(url).to_owned(),
    }
}

/// `new URL(text)`: неверный адрес — `TypeError: Invalid URL` (не `DeliveryError`).
pub fn parse_url(text: &str) -> Result<Url, RawError> {
    Url::parse(text).map_err(|_| invalid_url())
}

fn invalid_url() -> RawError {
    RawError {
        code: Some("ERR_INVALID_URL".to_owned()),
        message: "Invalid URL".to_owned(),
        ..RawError::default()
    }
}

/// `checkRedirect(from, to)`: только http и https; с https на http — только на loopback.
pub fn check_redirect(from: &Url, to: &Url) -> Result<(), DeliveryError> {
    if !matches!(to.scheme(), "http" | "https") {
        return Err(DeliveryError::new(
            Code::InsecureRedirect,
            format!("Редирект на недопустимую схему: {}", protocol(to)),
        ));
    }
    if from.scheme() == "https" && to.scheme() == "http" && !is_loopback(url_hostname(to)) {
        return Err(DeliveryError::new(
            Code::InsecureRedirect,
            format!(
                "Редирект с https на http запрещён: {}",
                to.origin().ascii_serialization()
            ),
        ));
    }
    Ok(())
}

/// Метод запроса.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Method {
    #[default]
    Get,
    Head,
}

/// Параметры запроса (`options` `request`).
#[derive(Debug, Clone, Default)]
pub struct RequestOptions {
    pub method: Method,
    /// Поверх `user-agent: HLSR-Launcher` (тот же ключ его заменяет).
    pub headers: Vec<(String, String)>,
    /// Простой соединения до ответа и между кусками тела; `None` или 0 — 30 с.
    pub timeout: Option<Duration>,
}

impl RequestOptions {
    fn idle_timeout(&self) -> Duration {
        self.timeout
            .filter(|timeout| !timeout.is_zero())
            .unwrap_or(Duration::from_millis(DEFAULT_TIMEOUT_MS))
    }
}

/// Клиент HTTP доставки; копии делят пул соединений.
#[derive(Debug, Clone)]
pub struct HttpClient {
    client: reqwest::Client,
}

impl Default for HttpClient {
    fn default() -> Self {
        HttpClient::new()
    }
}

impl HttpClient {
    pub fn new() -> HttpClient {
        let client = client_builder()
            .build()
            .expect("клиент HTTP собирается из постоянных настроек");
        HttpClient { client }
    }

    /// `request(url, options)`: запрос с редиректами вручную. Ответ 3xx без `Location`
    /// возвращается как есть.
    pub async fn request(&self, url: &str, options: &RequestOptions) -> Result<Response, Failure> {
        let mut current = parse_url(url)?;
        let mut redirects = 0;
        loop {
            let response = self.raw_request(&current, options).await?;
            let location = response.header("location").filter(|l| !l.is_empty());
            match location {
                Some(location) if (300..400).contains(&response.status()) => {
                    drop(response);
                    redirects += 1;
                    if redirects > MAX_REDIRECTS {
                        return Err(DeliveryError::new(
                            Code::TooManyRedirects,
                            format!("Слишком много редиректов: {url}"),
                        )
                        .into());
                    }
                    let next = current.join(&location).map_err(|_| invalid_url())?;
                    check_redirect(&current, &next)?;
                    current = next;
                }
                _ => return Ok(response),
            }
        }
    }

    /// `rawRequest(url, options)`: один запрос без редиректов.
    pub async fn raw_request(
        &self,
        target: &Url,
        options: &RequestOptions,
    ) -> Result<Response, Failure> {
        if !matches!(target.scheme(), "http" | "https") {
            return Err(DeliveryError::new(
                Code::InsecureRedirect,
                format!("Недопустимая схема: {}", protocol(target)),
            )
            .into());
        }
        let idle = options.idle_timeout();
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::USER_AGENT,
            HeaderValue::from_static(USER_AGENT),
        );
        for (name, value) in &options.headers {
            let header = HeaderName::from_bytes(name.as_bytes()).map_err(|_| RawError {
                code: Some("ERR_INVALID_HTTP_TOKEN".to_owned()),
                message: format!("Header name must be a valid HTTP token [\"{name}\"]"),
                ..RawError::default()
            })?;
            let invalid_char = || RawError {
                code: Some("ERR_INVALID_CHAR".to_owned()),
                message: format!("Invalid character in header content [\"{name}\"]"),
                ..RawError::default()
            };
            // Значения заголовков Node пишет в latin1.
            let bytes: Vec<u8> = value
                .chars()
                .map(|c| u8::try_from(u32::from(c)).map_err(|_| invalid_char()))
                .collect::<Result<_, _>>()?;
            let value = HeaderValue::from_bytes(&bytes).map_err(|_| invalid_char())?;
            headers.insert(header, value);
        }
        let method = match options.method {
            Method::Get => reqwest::Method::GET,
            Method::Head => reqwest::Method::HEAD,
        };
        let send = self
            .client
            .request(method, target.clone())
            .headers(headers)
            .send();
        let timed_out = || {
            DeliveryError::new(
                Code::NetworkTimeout,
                format!("Таймаут запроса {}", target.as_str()),
            )
            .into()
        };
        match tokio::time::timeout(idle, send).await {
            Err(_) => Err(timed_out()),
            // Тайм-аут соединения (и ETIMEDOUT ОС при соединении, до которого Node не доходит:
            // его таймер простоя срабатывает раньше). ETIMEDOUT ОС после соединения — сетевая
            // ошибка «read ETIMEDOUT», как в Node.
            Ok(Err(error)) if error.is_timeout() && error.is_connect() => Err(timed_out()),
            Ok(Err(error)) => Err(DeliveryError::new(
                Code::NetworkError,
                format!("Сетевая ошибка: {}", node_message(&error, target)),
            )
            .into()),
            Ok(Ok(inner)) => Ok(Response {
                url: target.clone(),
                inner,
                idle,
            }),
        }
    }

    /// `getJson(url, {headers, timeout, limitBytes})`: тело читается до проверки кода;
    /// 401/403 — `PROVIDER_FORBIDDEN`, не 2xx — `HTTP_ERROR`, не JSON — `PROVIDER_LINK_FAILED`.
    pub async fn get_json(
        &self,
        url: &str,
        options: &RequestOptions,
        limit_bytes: Option<usize>,
    ) -> Result<Value, Failure> {
        let response = self.request(url, options).await?;
        let status = response.status();
        let limit = limit_bytes
            .filter(|&l| l != 0)
            .unwrap_or(DEFAULT_JSON_LIMIT);
        let body = read_body(response, Some(limit), None).await?;
        if status == 401 || status == 403 {
            return Err(DeliveryError::new(
                Code::ProviderForbidden,
                format!("Доступ запрещён: HTTP {status}"),
            )
            .into());
        }
        if !(200..300).contains(&status) {
            return Err(
                DeliveryError::new(Code::HttpError, format!("HTTP {status} для {url}")).into(),
            );
        }
        json_parse(&String::from_utf8_lossy(&body)).map_err(|error| {
            DeliveryError::new(
                Code::ProviderLinkFailed,
                format!("Ответ не является JSON: {error}"),
            )
            .into()
        })
    }
}

/// Ответ: код, заголовки и тело, которое читается кусками.
pub struct Response {
    url: Url,
    inner: reqwest::Response,
    idle: Duration,
}

impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Response")
            .field("url", &self.url.as_str())
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

/// Заголовки, повтор которых Node отбрасывает (берёт первый).
const SINGLE_VALUE_HEADERS: &[&str] = &[
    "age",
    "authorization",
    "content-length",
    "content-type",
    "etag",
    "expires",
    "from",
    "host",
    "if-modified-since",
    "if-unmodified-since",
    "last-modified",
    "location",
    "max-forwards",
    "proxy-authorization",
    "referer",
    "retry-after",
    "server",
    "user-agent",
];

/// `headers[name]` как у Node: значение в latin1; повторы склеиваются через `, `,
/// кроме заголовков, где Node берёт первый, и `cookie` (через `; `).
pub fn node_header(headers: &HeaderMap, name: &str) -> Option<String> {
    let name = name.to_ascii_lowercase();
    let mut values = headers.get_all(name.as_str()).iter();
    let latin1 = |value: &HeaderValue| {
        value
            .as_bytes()
            .iter()
            .map(|&b| char::from(b))
            .collect::<String>()
    };
    let first = latin1(values.next()?);
    if SINGLE_VALUE_HEADERS.contains(&name.as_str()) {
        return Some(first);
    }
    let separator = if name == "cookie" { "; " } else { ", " };
    Some(values.fold(first, |mut joined, value| {
        joined.push_str(separator);
        joined.push_str(&latin1(value));
        joined
    }))
}

impl Response {
    /// `statusCode`.
    pub fn status(&self) -> u16 {
        self.inner.status().as_u16()
    }

    /// `finalUrl`: адрес последнего запроса после редиректов.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// `headers[name]` как у Node: значение в latin1; повторы склеиваются через `, `,
    /// кроме заголовков, где Node берёт первый, и `cookie` (через `; `).
    pub fn header(&self, name: &str) -> Option<String> {
        node_header(self.inner.headers(), name)
    }

    /// Следующий кусок тела; `None` — тело кончилось. Обрыв соединения и простой дольше
    /// тайм-аута запроса — ошибка потока ответа Node: `aborted` (`ECONNRESET`).
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, RawError> {
        match tokio::time::timeout(self.idle, self.inner.chunk()).await {
            Ok(Ok(chunk)) => Ok(chunk),
            Ok(Err(_)) | Err(_) => Err(RawError {
                code: Some("ECONNRESET".to_owned()),
                message: "aborted".to_owned(),
                ..RawError::default()
            }),
        }
    }
}

/// `readBody(response, limitBytes, rateLimiter)`: тело целиком, не больше `limit_bytes`
/// (`None` или 0 — 1 МБ). Каждый кусок сначала проходит ограничитель, потом считается.
pub async fn read_body(
    mut response: Response,
    limit_bytes: Option<usize>,
    rate_limiter: Option<&DownloadRateLimiter>,
) -> Result<Vec<u8>, DeliveryError> {
    let limit = limit_bytes
        .filter(|&l| l != 0)
        .unwrap_or(DEFAULT_BODY_LIMIT);
    let mut body = Vec::new();
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => return Ok(body),
            Err(error) => {
                return Err(DeliveryError::new(
                    Code::NetworkError,
                    format!("Обрыв ответа: {}", error.message),
                ));
            }
        };
        if let Some(limiter) = rate_limiter {
            limiter.acquire(chunk.len()).await;
        }
        if body.len() + chunk.len() > limit {
            return Err(DeliveryError::new(
                Code::HttpError,
                "Ответ превысил допустимый размер",
            ));
        }
        body.extend_from_slice(&chunk);
    }
}

/// Текст сетевой ошибки, как его даёт Node (`connect ECONNREFUSED 127.0.0.1:8080`,
/// `getaddrinfo ENOTFOUND host`, `socket hang up`); чего Node не скажет так же — текст
/// ошибки Rust.
fn node_message(error: &reqwest::Error, target: &Url) -> String {
    let address = || {
        let host = url_hostname(target)
            .trim_start_matches('[')
            .trim_end_matches(']');
        let port = target.port_or_known_default().unwrap_or(0);
        format!("{host}:{port}")
    };
    let mut source: Option<&(dyn std::error::Error + 'static)> = error.source();
    let mut innermost = error.to_string();
    while let Some(current) = source {
        let text = current.to_string();
        if text == "dns error" {
            return format!("getaddrinfo ENOTFOUND {}", url_hostname(target));
        }
        if text == "connection closed before message completed" {
            return "socket hang up".to_owned();
        }
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            if let Some(tls) = io.get_ref().and_then(|e| e.downcast_ref::<rustls::Error>()) {
                return tls_message(tls);
            }
            if let Some(code) = hlsr_core::js::errno::code(io) {
                return match io.kind() {
                    std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::HostUnreachable
                    | std::io::ErrorKind::NetworkUnreachable
                    | std::io::ErrorKind::AddrNotAvailable => {
                        format!("connect {code} {}", address())
                    }
                    _ => format!("read {code}"),
                };
            }
        }
        if let Some(tls) = current.downcast_ref::<rustls::Error>() {
            return tls_message(tls);
        }
        innermost = text;
        source = current.source();
    }
    innermost
}

/// Ошибки сертификата словами OpenSSL, как их пишет Node.
fn tls_message(error: &rustls::Error) -> String {
    use rustls::CertificateError as E;
    match error {
        rustls::Error::InvalidCertificate(E::UnknownIssuer) => {
            "unable to get local issuer certificate".to_owned()
        }
        rustls::Error::InvalidCertificate(E::Expired | E::ExpiredContext { .. }) => {
            "certificate has expired".to_owned()
        }
        rustls::Error::InvalidCertificate(E::NotValidYet | E::NotValidYetContext { .. }) => {
            "certificate is not yet valid".to_owned()
        }
        rustls::Error::InvalidCertificate(
            E::NotValidForName | E::NotValidForNameContext { .. },
        ) => "Hostname/IP does not match certificate's altnames".to_owned(),
        other => other.to_string(),
    }
}
