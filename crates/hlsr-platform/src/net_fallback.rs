//! Повтор небольших запросов метаданных при TLS-ошибке (`net-fallback.js`).
//!
//! Основной HTTP лаунчера (как у `https` Node 0.4.7) проверяет сертификаты по вшитому списку
//! Mozilla и не знает о системных прокси. Если сеть подменяет сертификат (антивирус,
//! корпоративный прокси, фильтр провайдера), такой запрос падает с TLS-ошибкой там, где браузер
//! открывает адрес. 0.4.7 в этом случае повторял запрос через Chromium (`electron.net.fetch`).
//! Здесь Chromium нет: тот же запрос повторяется с проверкой сертификатов системой
//! (`rustls-platform-verifier`) и с системным прокси (переменные `HTTPS_PROXY`, `ALL_PROXY`,
//! `NO_PROXY`; решение 5 спецификации ПП3). Так повторяются только метаданные релизов Proton:
//! сам архив сверяется по хешам и идёт основным путём.

use crate::linux::abort::Abort;
use crate::linux::prefix_command::ProtonError;
use hlsr_delivery::providers::BoxFuture;
use regex::Regex;
use std::sync::{Arc, LazyLock};

/// Предел тела по умолчанию: 2 МБ (`opts.limit || 2 * 1024 * 1024`).
pub const DEFAULT_LIMIT: usize = 2 * 1024 * 1024;
/// Редиректов не больше, чем у Chromium (`redirect: "follow"`).
pub const MAX_REDIRECTS: usize = 20;

// `TLS_CODE` и `/TLS|SSL|certificate/i`.
static TLS_CODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i-u)^ERR_TLS|^ERR_SSL|UNABLE_TO_VERIFY|UNABLE_TO_GET_ISSUER|CERT_|DEPTH_ZERO|SELF_SIGNED|EPROTO",
    )
    .expect("шаблон кода TLS")
});
static TLS_MESSAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i-u)TLS|SSL|certificate").expect("шаблон текста TLS"));

/// `isTlsError(error)`: код или текст ошибки говорят о TLS.
pub fn is_tls_error(code: Option<&str>, message: &str) -> bool {
    TLS_CODE.is_match(code.unwrap_or("")) || TLS_MESSAGE.is_match(message)
}

/// Ответ повторного запроса (`chromiumGet`): тело целиком или `None`, если оно больше предела.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackResponse {
    pub status: u16,
    /// Адрес после редиректов (`response.url || url`).
    pub url: String,
    pub body: Option<Vec<u8>>,
    pub truncated: bool,
}

/// Повторный запрос метаданных (`chromiumGet(url, {headers, signal, limit})`).
pub trait FallbackGet: Send + Sync {
    fn get<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(&'a str, &'a str)],
        limit: usize,
        abort: Option<&'a Abort>,
    ) -> BoxFuture<'a, Result<FallbackResponse, ProtonError>>;
}

/// Повтор с системными корнями и системным прокси.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemFallback;

impl FallbackGet for SystemFallback {
    fn get<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(&'a str, &'a str)],
        limit: usize,
        abort: Option<&'a Abort>,
    ) -> BoxFuture<'a, Result<FallbackResponse, ProtonError>> {
        Box::pin(chromium_get(url, headers, limit, abort))
    }
}

/// Повтор по умолчанию.
pub fn system_fallback() -> Arc<dyn FallbackGet> {
    Arc::new(SystemFallback)
}

/// Клиент повтора: TLS с проверкой сертификатов системой, системный прокси, редиректы — сам
/// клиент, как `redirect: "follow"`.
fn client() -> Result<reqwest::Client, ProtonError> {
    use rustls_platform_verifier::BuilderVerifierExt as _;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| ProtonError::new(error.to_string()))?
        .with_platform_verifier()
        .map_err(|error| ProtonError::new(error.to_string()))?
        .with_no_client_auth();
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
        .tls_backend_preconfigured(tls)
        .build()
        .map_err(|error| ProtonError::new(error.to_string()))
}

/// `chromiumGet(url, options)`: GET без кеша, тело — не больше `limit` (больше — `truncated`).
pub async fn chromium_get(
    url: &str,
    headers: &[(&str, &str)],
    limit: usize,
    abort: Option<&Abort>,
) -> Result<FallbackResponse, ProtonError> {
    let work = async {
        let client = client()?;
        let mut request = client.get(url).header("cache-control", "no-store");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let mut response = request
            .send()
            .await
            .map_err(|error| crate::linux::proton_manager::network_error(&error, url))?;
        let status = response.status().as_u16();
        let final_url = response.url().as_str().to_owned();
        let mut body = Vec::new();
        let mut truncated = false;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| crate::linux::proton_manager::network_error(&error, url))?
        {
            if body.len() + chunk.len() > limit {
                truncated = true;
                break;
            }
            body.extend_from_slice(&chunk);
        }
        Ok(FallbackResponse {
            status,
            url: final_url,
            body: (!truncated).then_some(body),
            truncated,
        })
    };
    match abort {
        Some(abort) => tokio::select! {
            result = work => result,
            () = abort.aborted() => Err(ProtonError::abort("This operation was aborted")),
        },
        None => work.await,
    }
}
