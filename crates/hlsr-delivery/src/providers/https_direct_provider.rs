//! Прямая ссылка https (`providers/HttpsDirectProvider.js`). Какие хосты допустимы,
//! проверяет `ManifestService` (`delivery_endpoints::is_allowed_download_url`), не источник.

use super::base_provider::BaseProvider;
use super::{BoxFuture, Provider, Resolved, sibling_url};
use crate::errors::{Code, DeliveryError, Failure};
use hlsr_core::js;
use serde_json::Value;
use url::Url;

#[derive(Debug, Clone)]
pub struct HttpsDirectProvider {
    base: BaseProvider,
    url: String,
}

impl HttpsDirectProvider {
    pub fn new(descriptor: &Value) -> Result<HttpsDirectProvider, DeliveryError> {
        let text = js::to_template_string(descriptor.get("url"));
        let parsed = Url::parse(&text).map_err(|_| {
            DeliveryError::new(
                Code::ProviderUnsupported,
                format!("Некорректный URL: {text}"),
            )
        })?;
        if parsed.scheme() != "https" {
            return Err(DeliveryError::new(
                Code::ProviderUnsupported,
                "https-direct допускает только https",
            ));
        }
        Ok(HttpsDirectProvider {
            base: BaseProvider::new(descriptor),
            url: parsed.into(),
        })
    }
}

impl Provider for HttpsDirectProvider {
    fn base(&self) -> &BaseProvider {
        &self.base
    }

    fn url(&self) -> Option<&str> {
        Some(&self.url)
    }

    fn resolve(&self) -> BoxFuture<'_, Result<Resolved, Failure>> {
        Box::pin(async move { Ok(Resolved::permanent(self.url.clone())) })
    }

    fn resolve_sibling(&self, filename: &str) -> Result<Resolved, DeliveryError> {
        sibling_url(&self.url, filename).map(Resolved::permanent)
    }
}
