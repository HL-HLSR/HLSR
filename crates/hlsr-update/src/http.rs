//! HTTPS-клиент шлюза (`httpsGet` из `LauncherUpdateGate.js`): GET только по адресам из allowlist —
//! и на каждом редиректе тоже, не больше 5 редиректов, простой соединения 20 с, тело не больше
//! 256 КиБ, `user-agent: HLSR-launcher-update`, без сжатия и системного прокси. Реализация
//! [`HttpGet`] на `reqwest` с общей настройкой TLS (`hlsr_core::http::client_builder`).

use crate::channel::is_allowed_url;
use crate::ports::{BoxFuture, GetError, GetOptions, HttpGet};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

pub const USER_AGENT: &str = "HLSR-launcher-update";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(20_000);
pub const MAX_BODY: usize = 256 * 1024;
pub const MAX_REDIRECTS: u32 = 5;

/// Какие адреса разрешены: в выпуске — только [`is_allowed_url`].
type AllowFn = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// GET по HTTPS в стиле `httpsGet` 0.4.7.
#[derive(Clone)]
pub struct ReqwestGet {
    client: reqwest::Client,
    allow: AllowFn,
}

impl ReqwestGet {
    /// Боевой клиент: https и хосты [`crate::channel::ALLOWED_HOSTS`].
    pub fn production() -> ReqwestGet {
        ReqwestGet {
            client: hlsr_core::http::client_builder()
                .user_agent(USER_AGENT)
                .build()
                .expect("клиент HTTP собирается из постоянных настроек"),
            allow: Arc::new(is_allowed_url),
        }
    }

    /// Клиент с другим списком адресов — для сверки с 0.4.7 на `127.0.0.1` (в 0.4.7 это подмена
    /// `channel.isAllowedUrl` в самотестах). Только в отладочной сборке и с `dev-hooks`.
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    pub fn with_allow(allow: impl Fn(&str) -> bool + Send + Sync + 'static) -> ReqwestGet {
        ReqwestGet {
            allow: Arc::new(allow),
            ..ReqwestGet::production()
        }
    }

    async fn fetch(&self, url: &str, options: &GetOptions) -> Result<Vec<u8>, GetError> {
        let idle = options
            .timeout
            .filter(|timeout| !timeout.is_zero())
            .unwrap_or(DEFAULT_TIMEOUT);
        // `opts.maxBody || MAX_BODY`: нулевой предел — предел по умолчанию.
        let max_body = options.max_body.filter(|max| *max != 0).unwrap_or(MAX_BODY);
        let mut current = url.to_owned();
        let mut redirects_left = MAX_REDIRECTS;

        loop {
            if !(self.allow)(&current) {
                return Err(GetError::NotAllowed(current));
            }
            let target =
                Url::parse(&current).map_err(|_| GetError::Network("Invalid URL".to_owned()))?;
            let mut response = tokio::time::timeout(idle, self.client.get(target.clone()).send())
                .await
                .map_err(|_| GetError::Timeout)?
                .map_err(network_error)?;

            let status = response.status().as_u16();
            if (301..=308).contains(&status)
                && let Some(location) = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .filter(|location| !location.is_empty())
                    .map(str::to_owned)
            {
                // Тело редиректа сбрасывается: соединение не нужно, но и не должно висеть.
                drop(response);
                if redirects_left == 0 {
                    return Err(GetError::TooManyRedirects);
                }
                let next = target
                    .join(&location)
                    .map_err(|_| GetError::Network("Invalid URL".to_owned()))?;
                current = next.to_string();
                redirects_left -= 1;
                continue;
            }

            if status != 200 {
                return Err(GetError::Status(status));
            }

            let mut body = Vec::new();
            loop {
                let chunk = tokio::time::timeout(idle, response.chunk())
                    .await
                    .map_err(|_| GetError::Timeout)?
                    .map_err(network_error)?;
                let Some(chunk) = chunk else {
                    return Ok(body);
                };
                if body.len() + chunk.len() > max_body {
                    return Err(GetError::TooLarge);
                }
                body.extend_from_slice(&chunk);
            }
        }
    }
}

fn network_error(error: reqwest::Error) -> GetError {
    if error.is_timeout() {
        return GetError::Timeout;
    }
    GetError::Network(error.to_string())
}

// ------------------------------------------------------------------- ошибки сети словами Node

/// Сетевая ошибка `reqwest` в том виде, в каком её отдаёт `http`/`https` Node: `error.code` и
/// `error.message`. По тексту ошибки службы обновления выбирают класс (`errorCode` у
/// `linux/UpdateManager.js`: `ENOTFOUND|ECONN|ETIMEDOUT` — `NETWORK`), диагностика — строку проверки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeNetError {
    /// `ENOTFOUND`, `EAI_AGAIN`, `ECONNREFUSED`, `ECONNRESET`, `ERR_TLS_HANDSHAKE`…; `None` — не распознан.
    pub code: Option<String>,
    pub message: String,
}

/// Ошибка запроса (до ответа), как у Node: `getaddrinfo ENOTFOUND <имя>` (`EAI_AGAIN` — временный
/// сбой DNS, обычный без сети), `connect ECONNREFUSED <узел>:<порт>`, `socket hang up` (соединение
/// закрыто до ответа), `read ECONNRESET`. Чего Node не скажет так же (TLS и прочее) — самая глубокая
/// причина без адреса запроса, код — по тексту цепочки ([`node_code_of_text`]).
///
/// Отступление: у `connect` Node пишет адрес, с которым соединялась (IP после DNS), здесь — узел и
/// порт адреса запроса (у `127.0.0.1` они совпадают). Тайм-ауты (`is_timeout`) вызывающий разбирает сам.
pub fn node_net_error(error: &reqwest::Error, url: &str) -> NodeNetError {
    use std::error::Error as _;
    let target = Url::parse(url).ok();
    let hostname = target
        .as_ref()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_default();
    let address = || {
        let port = target
            .as_ref()
            .and_then(Url::port_or_known_default)
            .unwrap_or(0);
        format!(
            "{}:{port}",
            hostname.trim_start_matches('[').trim_end_matches(']')
        )
    };
    let mut connect = false;
    let mut innermost = error.to_string();
    let mut source = error.source();
    while let Some(current) = source {
        let text = current.to_string();
        if text == "dns error" {
            let code = dns_code(current.source());
            return NodeNetError {
                code: Some(code.to_owned()),
                message: format!("getaddrinfo {code} {hostname}"),
            };
        }
        if text == "connection closed before message completed" {
            return NodeNetError {
                code: Some("ECONNRESET".to_owned()),
                message: "socket hang up".to_owned(),
            };
        }
        connect |= text == "tcp connect error";
        if let Some(io) = current.downcast_ref::<std::io::Error>()
            && let Some(code) = hlsr_core::js::errno::code(io)
        {
            use std::io::ErrorKind as K;
            let connecting = connect
                || matches!(
                    io.kind(),
                    K::ConnectionRefused
                        | K::HostUnreachable
                        | K::NetworkUnreachable
                        | K::AddrNotAvailable
                );
            let message = if connecting {
                format!("connect {code} {}", address())
            } else {
                format!("read {code}")
            };
            return NodeNetError {
                code: Some(code.into_owned()),
                message,
            };
        }
        innermost = text;
        source = current.source();
    }
    NodeNetError {
        code: node_code_of_text(&error_chain(error)).map(str::to_owned),
        message: innermost,
    }
}

/// Ошибка чтения тела после ответа: у Node соединение, закрытое или сброшенное посреди тела,
/// разрушает ответ ошибкой `aborted` с кодом `ECONNRESET` (`socketCloseListener`).
pub fn node_body_error() -> NodeNetError {
    NodeNetError {
        code: Some("ECONNRESET".to_owned()),
        message: "aborted".to_owned(),
    }
}

/// Код Node у неудачного `getaddrinfo` (`dnsException`): `EAI_NONAME`/`EAI_NODATA` → `ENOTFOUND`,
/// временный сбой → `EAI_AGAIN`, неустранимый → `EAI_FAIL`. glibc отдаёт текст `gai_strerror`,
/// Windows — код WSA (`WSATRY_AGAIN` 11002, `WSANO_RECOVERY` 11003).
fn dns_code(cause: Option<&(dyn std::error::Error + 'static)>) -> &'static str {
    let mut source = cause;
    while let Some(current) = source {
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            if cfg!(windows) {
                match io.raw_os_error() {
                    Some(11002) => return "EAI_AGAIN",
                    Some(11003) => return "EAI_FAIL",
                    _ => {}
                }
            }
            let text = io.to_string();
            if text.contains("Temporary failure in name resolution") {
                return "EAI_AGAIN";
            }
            if text.contains("Non-recoverable failure in name resolution") {
                return "EAI_FAIL";
            }
        }
        source = current.source();
    }
    "ENOTFOUND"
}

/// Ошибка `reqwest` со всей цепочкой причин: `error sending request for url (…): client error
/// (Connect): tcp connect error: Connection refused (os error 111)`.
pub fn error_chain(error: &reqwest::Error) -> String {
    use std::error::Error as _;
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// Код Node по тексту цепочки причин `reqwest`, когда разбор по типам не помог: имя не найдено,
/// сертификат, TLS, отказ и обрыв соединения.
pub fn node_code_of_text(chain: &str) -> Option<&'static str> {
    let lower = chain.to_lowercase();
    let has = |parts: &[&str]| parts.iter().any(|part| lower.contains(part));
    if has(&[
        "dns error",
        "failed to lookup",
        "no such host",
        "name or service not known",
        "nodename nor servname",
        "no address associated",
    ]) {
        Some("ENOTFOUND")
    } else if has(&["certificate", "invalid peer", "unknownissuer"]) {
        Some("UNABLE_TO_VERIFY_LEAF_SIGNATURE")
    } else if has(&["tls", "handshake"]) {
        Some("ERR_TLS_HANDSHAKE")
    } else if has(&["refused"]) {
        Some("ECONNREFUSED")
    } else if has(&[
        "reset",
        "broken pipe",
        "closed before",
        "connection closed",
        "end of file",
        "incomplete message",
        "error reading a body",
    ]) {
        Some("ECONNRESET")
    } else {
        None
    }
}

impl HttpGet for ReqwestGet {
    fn get<'a>(
        &'a self,
        url: &'a str,
        options: &'a GetOptions,
    ) -> BoxFuture<'a, Result<Vec<u8>, GetError>> {
        Box::pin(async move {
            match &options.cancel {
                Some(cancel) => tokio::select! {
                    biased;
                    () = cancel.cancelled() => Err(GetError::Aborted),
                    result = self.fetch(url, options) => result,
                },
                None => self.fetch(url, options).await,
            }
        })
    }
}

/// `qaHttpGet`: адреса `http://127.0.0.1` и `http://localhost` — обычным GET без ограничений,
/// всё остальное — как [`ReqwestGet`]. Только для локального канала QA-сборки.
#[cfg(any(debug_assertions, feature = "dev-hooks"))]
pub struct QaHttpGet {
    client: reqwest::Client,
    fallback: ReqwestGet,
}

#[cfg(any(debug_assertions, feature = "dev-hooks"))]
impl QaHttpGet {
    pub fn new() -> QaHttpGet {
        QaHttpGet {
            client: hlsr_core::http::client_builder()
                .build()
                .expect("клиент HTTP собирается из постоянных настроек"),
            fallback: ReqwestGet::production(),
        }
    }
}

#[cfg(any(debug_assertions, feature = "dev-hooks"))]
impl Default for QaHttpGet {
    fn default() -> QaHttpGet {
        QaHttpGet::new()
    }
}

/// `^http:\/\/(127\.0\.0\.1|localhost)(:\d+)?\/`
pub fn is_qa_loopback_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let rest = rest
        .strip_prefix("127.0.0.1")
        .or_else(|| rest.strip_prefix("localhost"));
    let Some(rest) = rest else {
        return false;
    };
    let rest = match rest.strip_prefix(':') {
        Some(after) => {
            let digits = after.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return false;
            }
            &after[digits..]
        }
        None => rest,
    };
    rest.starts_with('/')
}

#[cfg(any(debug_assertions, feature = "dev-hooks"))]
impl HttpGet for QaHttpGet {
    fn get<'a>(
        &'a self,
        url: &'a str,
        options: &'a GetOptions,
    ) -> BoxFuture<'a, Result<Vec<u8>, GetError>> {
        if !is_qa_loopback_url(url) {
            return self.fallback.get(url, options);
        }
        Box::pin(async move {
            let response = self.client.get(url).send().await.map_err(network_error)?;
            if response.status().as_u16() != 200 {
                return Err(GetError::Status(response.status().as_u16()));
            }
            Ok(response.bytes().await.map_err(network_error)?.to_vec())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_failures_get_the_codes_node_gives_them() {
        let glibc = |text: &str| {
            std::io::Error::other(format!("failed to lookup address information: {text}"))
        };
        let code = |error: std::io::Error| dns_code(Some(&error));
        assert_eq!(code(glibc("Name or service not known")), "ENOTFOUND");
        assert_eq!(
            code(glibc("No address associated with hostname")),
            "ENOTFOUND"
        );
        assert_eq!(
            code(glibc("Temporary failure in name resolution")),
            "EAI_AGAIN"
        );
        assert_eq!(
            code(glibc("Non-recoverable failure in name resolution")),
            "EAI_FAIL"
        );
        assert_eq!(dns_code(None), "ENOTFOUND");
        if cfg!(windows) {
            let wsa = std::io::Error::from_raw_os_error;
            assert_eq!(code(wsa(11001)), "ENOTFOUND");
            assert_eq!(code(wsa(11002)), "EAI_AGAIN");
            assert_eq!(code(wsa(11003)), "EAI_FAIL");
            assert_eq!(code(wsa(11004)), "ENOTFOUND");
        }
    }

    #[test]
    fn text_codes_follow_the_diagnostics_rules() {
        let code = node_code_of_text;
        assert_eq!(
            code("client error (Connect): dns error: failed to lookup address information"),
            Some("ENOTFOUND")
        );
        assert_eq!(
            code("invalid peer certificate: UnknownIssuer"),
            Some("UNABLE_TO_VERIFY_LEAF_SIGNATURE")
        );
        assert_eq!(
            code("received fatal alert: HandshakeFailure"),
            Some("ERR_TLS_HANDSHAKE")
        );
        assert_eq!(
            code("tcp connect error: Connection refused (os error 111)"),
            Some("ECONNREFUSED")
        );
        assert_eq!(code("connection reset by peer"), Some("ECONNRESET"));
        assert_eq!(code("something else"), None);
    }
}
