//! Публичная ссылка Яндекс.Диска (`providers/YandexPublicProvider.js`): временная ссылка
//! на файл берётся из API на каждой попытке. Соседние файлы этот источник не отдаёт.

use super::base_provider::BaseProvider;
use super::{BoxFuture, Provider, ProviderOptions, Resolved};
use crate::errors::{Code, DeliveryError, Failure};
use crate::http_client::{HttpClient, RequestOptions};
use hlsr_core::js;
use serde_json::Value;
use std::time::Duration;

pub const API_BASE: &str = "https://cloud-api.yandex.net/v1/disk/public/resources/download";

/// `^https://disk\.yandex\.(ru|com)/d/[A-Za-z0-9_-]+$`.
fn is_public_url(text: &str) -> bool {
    let rest = text
        .strip_prefix("https://disk.yandex.ru/d/")
        .or_else(|| text.strip_prefix("https://disk.yandex.com/d/"));
    rest.is_some_and(|key| {
        !key.is_empty()
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

#[derive(Debug, Clone)]
pub struct YandexPublicProvider {
    base: BaseProvider,
    public_url: String,
    api_base: String,
    http: HttpClient,
    timeout: Option<Duration>,
}

impl YandexPublicProvider {
    pub fn new(
        descriptor: &Value,
        options: &ProviderOptions,
    ) -> Result<YandexPublicProvider, DeliveryError> {
        // `String(descriptor.publicUrl || "")`.
        let public_url = descriptor
            .get("publicUrl")
            .filter(|value| js::truthy(value))
            .map(js::to_string)
            .unwrap_or_default();
        if !is_public_url(&public_url) {
            return Err(DeliveryError::new(
                Code::ProviderUnsupported,
                format!(
                    "Некорректная публичная ссылка Яндекс.Диска: {}",
                    js::to_template_string(descriptor.get("publicUrl"))
                ),
            ));
        }
        Ok(YandexPublicProvider {
            base: BaseProvider::new(descriptor),
            public_url,
            api_base: options.yandex_api_base_or_default().to_owned(),
            http: options.http.clone(),
            timeout: options.timeout,
        })
    }

    /// `buildApiUrl()`: `<API>?public_key=<encodeURIComponent(publicUrl)>`.
    pub fn build_api_url(&self) -> String {
        format!(
            "{}?public_key={}",
            self.api_base,
            js::encode_uri_component(&self.public_url)
        )
    }

    async fn resolve_link(&self) -> Result<Resolved, Failure> {
        let options = RequestOptions {
            headers: vec![("accept".to_owned(), "application/json".to_owned())],
            timeout: self.timeout,
            ..RequestOptions::default()
        };
        let payload = self
            .http
            .get_json(&self.build_api_url(), &options, None)
            .await?;
        let failed = |message: String| -> Failure {
            DeliveryError::new(Code::ProviderLinkFailed, message).into()
        };
        let href = match payload.get("href") {
            Some(Value::String(href)) if js::truthy(&payload) && !href.is_empty() => href.clone(),
            _ => return Err(failed("Яндекс.Диск не вернул ссылку на файл".to_owned())),
        };
        if payload.get("templated").is_some_and(js::truthy) {
            return Err(failed(
                "Яндекс.Диск вернул шаблонную ссылку — такой ответ не поддерживается".to_owned(),
            ));
        }
        // `/^https:\/\//i`.
        if !href
            .get(..8)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
        {
            return Err(failed(format!(
                "Яндекс.Диск вернул ссылку не по https: {href}"
            )));
        }
        Ok(Resolved {
            url: href,
            headers: Vec::new(),
            temporary: true,
        })
    }
}

impl Provider for YandexPublicProvider {
    fn base(&self) -> &BaseProvider {
        &self.base
    }

    fn resolve(&self) -> BoxFuture<'_, Result<Resolved, Failure>> {
        Box::pin(self.resolve_link())
    }
}
