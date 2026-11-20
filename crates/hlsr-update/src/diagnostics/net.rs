//! Сеть диагностики (`httpsGet` и `classify*` из `DiagnosticsRunner.js`): GET с `user-agent:
//! HLSR-diagnostics`, простой соединения 6 с, до 5 редиректов, тело не больше 512 КиБ; DNS
//! `github.com`. Запрос и DNS — трейты ([`DiagnosticsRequest`], [`DnsLookup`]): раннер ходит
//! только через них, тесты подставляют подделки.
//!
//! Не переносится: повтор через Chromium при ошибке TLS (`requestWithFallback`, `:102-130`) —
//! в Rust-лаунчере нет Chromium. Ошибка TLS в строке проверки остаётся `TLS_ERROR`, как у 0.4.7,
//! когда повтор тоже не помог. Реализацию порта в приложении можно обернуть повтором с
//! проверкой сертификатов системой (`hlsr_platform::net_fallback`).

use crate::ports::BoxFuture;
use hlsr_core::js::{canonicalize_ignore_case, slice_utf16};
use std::time::Duration;
use url::Url;

/// `NET_TIMEOUT_MS`: простой запроса и ожидание DNS.
pub const NET_TIMEOUT: Duration = Duration::from_millis(6000);
/// `MAX_BODY`: больше — ответ усечён.
pub const MAX_BODY: usize = 512 * 1024;
/// Редиректов не больше пяти (`redirectsLeft`).
pub const MAX_REDIRECTS: u32 = 5;
pub const USER_AGENT: &str = "HLSR-diagnostics";

/// Сетевая ошибка в том виде, который разбирает [`classify_network_error`]: код Node (`ENOTFOUND`,
/// `ETIMEDOUT`, `ECONNRESET`, `ERR_TLS_*`, `CANCELLED`…) и сообщение.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetError {
    pub code: Option<String>,
    pub message: String,
}

impl NetError {
    pub fn new(code: Option<&str>, message: impl Into<String>) -> NetError {
        NetError {
            code: code.map(str::to_owned),
            message: message.into(),
        }
    }

    /// `Object.assign(new Error("отменено"), { code: "CANCELLED" })`.
    pub fn cancelled() -> NetError {
        NetError::new(Some("CANCELLED"), "отменено")
    }

    /// `Object.assign(new Error("таймаут"), { code: "ETIMEDOUT" })`.
    pub fn timeout() -> NetError {
        NetError::new(Some("ETIMEDOUT"), "таймаут")
    }

    pub fn is_cancelled(&self) -> bool {
        self.code.as_deref() == Some("CANCELLED")
    }
}

/// Ответ `httpsGet`: код, тело (`None` — усечено) и признак усечения. Заголовки раннеру не нужны.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Option<Vec<u8>>,
    pub truncated: bool,
}

impl Response {
    pub fn ok(status: u16, body: impl Into<Vec<u8>>) -> Response {
        Response {
            status,
            body: Some(body.into()),
            truncated: false,
        }
    }
}

/// `this.request(url, cancel)`: GET одного адреса. Отмену раннер делает сам — бросает будущее.
pub trait DiagnosticsRequest: Send + Sync {
    fn get<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<Response, NetError>>;
}

/// `this.lookup("github.com", callback)`: имя разрешается или нет.
pub trait DnsLookup: Send + Sync {
    fn lookup<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<(), NetError>>;
}

// ------------------------------------------------------------------- классификация

fn contains_ignore_case(text: &str, needle: &str) -> bool {
    let text: Vec<char> = text.chars().map(canonicalize_ignore_case).collect();
    let needle: Vec<char> = needle.chars().map(canonicalize_ignore_case).collect();
    needle.is_empty() || text.windows(needle.len()).any(|window| window == needle)
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    let mut chars = text.chars().map(canonicalize_ignore_case);
    prefix
        .chars()
        .map(canonicalize_ignore_case)
        .all(|expected| chars.next() == Some(expected))
}

/// `classifyNetworkError(error)`.
pub fn classify_network_error(error: &NetError) -> &'static str {
    let code = error.code.as_deref().unwrap_or("");
    let message = error.message.as_str();

    if matches!(code, "ENOTFOUND" | "EAI_AGAIN" | "ESERVFAIL") {
        return "DNS_ERROR";
    }
    if code == "ETIMEDOUT"
        || contains_ignore_case(message, "таймаут")
        || contains_ignore_case(message, "timeout")
    {
        return "CONNECT_TIMEOUT";
    }
    if code == "ECONNRESET" {
        return "CONNECTION_RESET";
    }
    if starts_with_ignore_case(code, "ERR_TLS")
        || ["UNABLE_TO_VERIFY", "CERT_", "DEPTH_ZERO", "SELF_SIGNED"]
            .iter()
            .any(|part| contains_ignore_case(code, part))
        || contains_ignore_case(message, "TLS")
        || contains_ignore_case(message, "certificate")
    {
        return "TLS_ERROR";
    }
    "UNKNOWN_NETWORK_ERROR"
}

/// `classifyHttpStatus(status)`: `None` — ответ 2xx.
pub fn classify_http_status(status: u16) -> Option<&'static str> {
    match status {
        404 => Some("HTTP_404_FEED_MISSING"),
        403 => Some("HTTP_403_FORBIDDEN"),
        429 => Some("HTTP_429_RATE_LIMIT"),
        status if !(200..300).contains(&status) => Some("UNKNOWN_NETWORK_ERROR"),
        _ => None,
    }
}

/// `errorDetail(error)`: «код: сообщение» (сообщение — первые 120 знаков).
pub fn error_detail(error: &NetError) -> String {
    let code = error.code.as_deref().unwrap_or("");
    let message = slice_utf16(&error.message, 120);
    [code, message.as_str()]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(": ")
}

// ------------------------------------------------------------------- боевая реализация

/// `httpsGet` на `reqwest` с общей настройкой TLS (`hlsr_core::http::client_builder`).
#[derive(Clone)]
pub struct ReqwestRequest {
    client: reqwest::Client,
    idle: Duration,
}

impl ReqwestRequest {
    /// Боевые значения 0.4.7: простой 6 с.
    pub fn new() -> ReqwestRequest {
        ReqwestRequest::with_idle(NET_TIMEOUT)
    }

    /// Другой простой — для тестов.
    pub fn with_idle(idle: Duration) -> ReqwestRequest {
        ReqwestRequest {
            client: hlsr_core::http::client_builder()
                .user_agent(USER_AGENT)
                .build()
                .expect("клиент HTTP собирается из постоянных настроек"),
            idle,
        }
    }

    async fn fetch(&self, url: &str) -> Result<Response, NetError> {
        let mut current = Url::parse(url).map_err(|e| NetError::new(None, e.to_string()))?;
        let mut redirects_left = MAX_REDIRECTS;
        loop {
            let mut response =
                tokio::time::timeout(self.idle, self.client.get(current.clone()).send())
                    .await
                    .map_err(|_| NetError::timeout())?
                    .map_err(|error| net_error(&error, current.as_str()))?;
            let status = response.status().as_u16();

            if (301..=308).contains(&status)
                && redirects_left > 0
                && let Some(location) = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .filter(|location| !location.is_empty())
            {
                let next = current
                    .join(location)
                    .map_err(|e| NetError::new(None, e.to_string()))?;
                drop(response);
                current = next;
                redirects_left -= 1;
                continue;
            }

            let mut body = Vec::new();
            loop {
                let chunk = tokio::time::timeout(self.idle, response.chunk())
                    .await
                    .map_err(|_| NetError::timeout())?
                    .map_err(|error| body_error(&error))?;
                let Some(chunk) = chunk else {
                    return Ok(Response {
                        status,
                        body: Some(body),
                        truncated: false,
                    });
                };
                if body.len() + chunk.len() > MAX_BODY {
                    return Ok(Response {
                        status,
                        body: None,
                        truncated: true,
                    });
                }
                body.extend_from_slice(&chunk);
            }
        }
    }
}

impl Default for ReqwestRequest {
    fn default() -> ReqwestRequest {
        ReqwestRequest::new()
    }
}

impl DiagnosticsRequest for ReqwestRequest {
    fn get<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<Response, NetError>> {
        Box::pin(self.fetch(url))
    }
}

/// Ошибка запроса `reqwest` → код и текст Node ([`crate::http::node_net_error`]), по коду раннер
/// выбирает `DNS_ERROR`, `TLS_ERROR` и т. д.
fn net_error(error: &reqwest::Error, url: &str) -> NetError {
    if error.is_timeout() {
        return NetError::timeout();
    }
    let node = crate::http::node_net_error(error, url);
    NetError::new(node.code.as_deref(), node.message)
}

/// Обрыв посреди тела: `aborted` с кодом `ECONNRESET`, как `response.on("error")` у Node.
fn body_error(error: &reqwest::Error) -> NetError {
    if error.is_timeout() {
        return NetError::timeout();
    }
    let node = crate::http::node_body_error();
    NetError::new(node.code.as_deref(), node.message)
}

/// DNS системы (`dns.lookup`) через `tokio::net::lookup_host`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemDns;

impl DnsLookup for SystemDns {
    fn lookup<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<(), NetError>> {
        Box::pin(async move {
            match tokio::net::lookup_host((host, 0)).await {
                Ok(addresses) => {
                    let found = addresses.count() > 0;
                    if found {
                        Ok(())
                    } else {
                        Err(NetError::new(
                            Some("ENOTFOUND"),
                            format!("getaddrinfo ENOTFOUND {host}"),
                        ))
                    }
                }
                Err(e) => Err(NetError::new(
                    Some("ENOTFOUND"),
                    format!("getaddrinfo ENOTFOUND {host}: {e}"),
                )),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_follows_the_reference_rules() {
        let by = |code: Option<&str>, message: &str| {
            classify_network_error(&NetError::new(code, message))
        };
        assert_eq!(by(Some("ENOTFOUND"), ""), "DNS_ERROR");
        assert_eq!(by(Some("EAI_AGAIN"), "x"), "DNS_ERROR");
        assert_eq!(by(Some("ETIMEDOUT"), ""), "CONNECT_TIMEOUT");
        assert_eq!(by(None, "Таймаут"), "CONNECT_TIMEOUT");
        assert_eq!(by(None, "Socket TIMEOUT"), "CONNECT_TIMEOUT");
        assert_eq!(by(Some("ECONNRESET"), ""), "CONNECTION_RESET");
        assert_eq!(by(Some("ERR_TLS_CERT_ALTNAME_INVALID"), ""), "TLS_ERROR");
        assert_eq!(by(Some("unable_to_verify_leaf_signature"), ""), "TLS_ERROR");
        assert_eq!(by(Some("CERT_HAS_EXPIRED"), ""), "TLS_ERROR");
        assert_eq!(by(None, "bad certificate"), "TLS_ERROR");
        assert_eq!(by(Some("ECONNREFUSED"), "refused"), "UNKNOWN_NETWORK_ERROR");
        assert_eq!(by(None, ""), "UNKNOWN_NETWORK_ERROR");
        // `/i` без `u`: ſ (U+017F) не равно s.
        assert_eq!(by(None, "tl\u{17F}"), "UNKNOWN_NETWORK_ERROR");
    }

    #[test]
    fn http_statuses() {
        assert_eq!(classify_http_status(404), Some("HTTP_404_FEED_MISSING"));
        assert_eq!(classify_http_status(403), Some("HTTP_403_FORBIDDEN"));
        assert_eq!(classify_http_status(429), Some("HTTP_429_RATE_LIMIT"));
        assert_eq!(classify_http_status(302), Some("UNKNOWN_NETWORK_ERROR"));
        assert_eq!(classify_http_status(199), Some("UNKNOWN_NETWORK_ERROR"));
        assert_eq!(classify_http_status(500), Some("UNKNOWN_NETWORK_ERROR"));
        assert_eq!(classify_http_status(200), None);
        assert_eq!(classify_http_status(204), None);
    }

    #[test]
    fn detail_is_code_and_clipped_message() {
        assert_eq!(error_detail(&NetError::new(Some("EX"), "m")), "EX: m");
        assert_eq!(error_detail(&NetError::new(None, "m")), "m");
        assert_eq!(error_detail(&NetError::new(Some("EX"), "")), "EX");
        let long = "я".repeat(200);
        assert_eq!(
            error_detail(&NetError::new(None, long)).chars().count(),
            120
        );
    }
}
