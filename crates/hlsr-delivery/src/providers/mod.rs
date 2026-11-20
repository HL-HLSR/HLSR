//! Источники загрузки (`providers/index.js`): `https-direct`, `yandex-public` и, только в
//! отладочной сборке и с `dev-hooks`, `local-http`. Фабрика проверяет, что тип разрешён
//! флагами (`feature-flag.js`).

pub mod base_provider;
pub mod https_direct_provider;
#[cfg(any(debug_assertions, feature = "dev-hooks"))]
pub mod local_http_provider;
pub mod yandex_public_provider;

use crate::environment::Environment;
use crate::errors::{Code, DeliveryError, Failure};
use crate::feature_flag;
use crate::http_client::HttpClient;
use base_provider::BaseProvider;
use hlsr_core::js;
use serde_json::{Map, Value, json};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Куда идти за файлом: `{url, headers, temporary}` (у соседнего файла `temporary` — `false`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// Ссылка временная: запрашивается заново на каждой попытке.
    pub temporary: bool,
}

impl Resolved {
    fn permanent(url: String) -> Resolved {
        Resolved {
            url,
            headers: Vec::new(),
            temporary: false,
        }
    }
}

/// Источник загрузки.
pub trait Provider: Send + Sync {
    fn base(&self) -> &BaseProvider;

    /// `id` описания.
    fn id(&self) -> Option<&str> {
        self.base().id()
    }

    /// Постоянный адрес (`provider.url`), если он есть.
    fn url(&self) -> Option<&str> {
        None
    }

    /// `resolve()`: адрес и заголовки для очередной попытки.
    fn resolve(&self) -> BoxFuture<'_, Result<Resolved, Failure>>;

    /// `shouldRefreshOn(statusCode)`: ответ, после которого нужна новая ссылка.
    fn should_refresh_on(&self, status: u16) -> bool {
        BaseProvider::should_refresh_on(status)
    }

    /// `supportsRange()`.
    fn supports_range(&self) -> bool {
        true
    }

    /// `resolveSibling(filename)`: файл рядом с архивом (`files.json`, манифест патча).
    fn resolve_sibling(&self, filename: &str) -> Result<Resolved, DeliveryError> {
        let _ = filename;
        Err(self.base().sibling_unsupported())
    }

    /// `describe()`: `{id, type, status}`.
    fn describe(&self) -> Value {
        self.base().describe()
    }
}

/// Параметры фабрики (`options` `create`).
#[derive(Debug, Clone)]
pub struct ProviderOptions {
    allowed_provider_types: Vec<String>,
    http: HttpClient,
    timeout: Option<Duration>,
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    yandex_api_base: Option<String>,
}

impl ProviderOptions {
    /// Разрешённые типы — по флагам окружения (`featureFlag.allowedProviderTypes(env)`).
    pub fn new(env: &Environment, http: HttpClient) -> ProviderOptions {
        Self::with_allowed_provider_types(http, feature_flag::allowed_provider_types(env))
    }

    /// Разрешённые типы заданы явно (`allowedProviderTypes`). В выпуске без `dev-hooks`
    /// `local-http` из списка выпадает.
    pub fn with_allowed_provider_types<S: Into<String>>(
        http: HttpClient,
        types: impl IntoIterator<Item = S>,
    ) -> ProviderOptions {
        let allowed_provider_types = types
            .into_iter()
            .map(Into::into)
            .filter(|kind: &String| {
                cfg!(any(debug_assertions, feature = "dev-hooks")) || kind != "local-http"
            })
            .collect();
        ProviderOptions {
            allowed_provider_types,
            http,
            timeout: None,
            #[cfg(any(debug_assertions, feature = "dev-hooks"))]
            yandex_api_base: None,
        }
    }

    /// Тайм-аут запроса к API Яндекса (`options.timeout`).
    pub fn timeout(mut self, timeout: Duration) -> ProviderOptions {
        self.timeout = Some(timeout);
        self
    }

    /// Адрес API Яндекса вместо `cloud-api.yandex.net` (`options.apiBase`): только для
    /// тестов и QA.
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    pub fn yandex_api_base(mut self, base: impl Into<String>) -> ProviderOptions {
        self.yandex_api_base = Some(base.into());
        self
    }

    pub fn allowed_provider_types(&self) -> &[String] {
        &self.allowed_provider_types
    }

    fn yandex_api_base_or_default(&self) -> &str {
        #[cfg(any(debug_assertions, feature = "dev-hooks"))]
        if let Some(base) = &self.yandex_api_base {
            return base;
        }
        yandex_public_provider::API_BASE
    }
}

/// `create(descriptor, options)`: источник по описанию из манифеста.
pub fn create(
    descriptor: &Value,
    options: &ProviderOptions,
) -> Result<Box<dyn Provider>, DeliveryError> {
    let kind = descriptor.get("type");
    let allowed = &options.allowed_provider_types;
    let kind = match kind.and_then(Value::as_str) {
        Some(kind) if allowed.iter().any(|a| a == kind) => kind,
        _ => {
            let mut details = Map::new();
            if let Some(kind) = kind {
                details.insert("type".to_owned(), kind.clone());
            }
            details.insert("allowed".to_owned(), json!(allowed));
            return Err(DeliveryError::new(
                Code::RemoteDisabled,
                format!(
                    "Провайдер {} выключен feature flag'ом {}",
                    js::to_template_string(kind),
                    feature_flag::FLAG
                ),
            )
            .with_details(Value::Object(details)));
        }
    };
    match kind {
        "yandex-public" => Ok(Box::new(yandex_public_provider::YandexPublicProvider::new(
            descriptor, options,
        )?)),
        "https-direct" => Ok(Box::new(https_direct_provider::HttpsDirectProvider::new(
            descriptor,
        )?)),
        #[cfg(any(debug_assertions, feature = "dev-hooks"))]
        "local-http" => Ok(Box::new(local_http_provider::LocalHttpProvider::new(
            descriptor,
        )?)),
        _ => Err(DeliveryError::new(
            Code::ProviderUnsupported,
            format!("Неизвестный тип провайдера: {kind}"),
        )),
    }
}

/// `resolveSibling` прямых источников: имя файла без `/` и `\` вместо последнего
/// сегмента пути (`pathname.replace(/[^/]+$/, encodeURIComponent(filename))`).
fn sibling_url(base: &str, filename: &str) -> Result<String, DeliveryError> {
    if filename.contains(['/', '\\']) {
        return Err(DeliveryError::new(
            Code::PathInvalid,
            format!("Недопустимое имя файла: {filename}"),
        ));
    }
    let mut url = url::Url::parse(base).expect("адрес источника разобран при создании");
    let path = url.path();
    let segment_start = path.rfind('/').map_or(0, |slash| slash + 1);
    if segment_start < path.len() {
        let path = format!(
            "{}{}",
            &path[..segment_start],
            js::encode_uri_component(filename)
        );
        url.set_path(&path);
    }
    Ok(url.into())
}
