//! Локальный HTTP для QA (`providers/LocalHttpProvider.js`): только `http://` на loopback.
//! Есть только в отладочной сборке и с `dev-hooks`: в выпуске («production» 0.4.7) его
//! нельзя включить ничем.

use super::base_provider::BaseProvider;
use super::{BoxFuture, Provider, Resolved, sibling_url};
use crate::errors::{Code, DeliveryError, Failure};
use crate::http_client::{is_loopback, url_hostname};
use hlsr_core::js;
use serde_json::Value;
use url::Url;

#[derive(Debug, Clone)]
pub struct LocalHttpProvider {
    base: BaseProvider,
    url: String,
}

impl LocalHttpProvider {
    pub fn new(descriptor: &Value) -> Result<LocalHttpProvider, DeliveryError> {
        let text = js::to_template_string(descriptor.get("url"));
        let parsed = Url::parse(&text).map_err(|_| {
            DeliveryError::new(
                Code::ProviderUnsupported,
                format!("Некорректный URL: {text}"),
            )
        })?;
        if parsed.scheme() != "http" || !is_loopback(url_hostname(&parsed)) {
            return Err(DeliveryError::new(
                Code::ProviderUnsupported,
                format!("local-http допускает только loopback: {text}"),
            ));
        }
        Ok(LocalHttpProvider {
            base: BaseProvider::new(descriptor),
            url: parsed.into(),
        })
    }
}

impl Provider for LocalHttpProvider {
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
