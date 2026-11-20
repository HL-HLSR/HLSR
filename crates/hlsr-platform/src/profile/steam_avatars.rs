//! `profile/steam-avatars.js`: аватар одного игрока — сначала наш кеш на диске
//! (`avatar-cache/<SteamID64>.bin`), потом сеть: XML профиля сообщества
//! `https://steamcommunity.com/profiles/<id>?xml=1`, из него адрес `avatarMedium`, затем сама
//! картинка с CDN Steam. Ходить можно только по https и только на хосты Steam: профиль —
//! на `steamcommunity.com`, картинку — на CDN из списка ([`is_allowed_image_host`]); проверка
//! стоит на каждом шаге редиректа (не больше трёх). Ответ XML — не больше 256 КиБ, картинка —
//! не больше 512 КиБ, тайм-аут простоя — 8 с. Ничего не бросается: любая неудача — `None`.
//!
//! Запросы идут через [`HttpGet`]. Настоящий [`WebGet`] ходит в интернет; для проверок есть
//! `WebGet::with_remap`, который подменяет адрес *после* проверок хоста и протокола — так тесты
//! гоняют настоящий код редиректов и пределов по локальному серверу 127.0.0.1, а в лаунчере
//! подмены нет.

use crate::process_inspect::node_path;
use crate::steam_account::MAX_AVATAR_BYTES;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hlsr_core::js::Platform;
use hlsr_core::js::is_whitespace;
use hlsr_delivery::providers::BoxFuture;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

pub const PROFILE_HOST: &str = "steamcommunity.com";
pub const IMAGE_HOSTS: [&str; 3] = [
    "steamcdn-a.akamaihd.net",
    "media.steampowered.com",
    "steamcommunity-a.akamaihd.net",
];
pub const IMAGE_HOST_SUFFIX: &str = ".steamstatic.com";

pub const REQUEST_TIMEOUT_MS: u64 = 8000;
pub const MAX_REDIRECTS: u32 = 3;
/// Предел ответа XML профиля.
pub const MAX_PROFILE_BYTES: usize = 256 * 1024;

/// `isAllowedImageHost(hostname)`.
pub fn is_allowed_image_host(hostname: &str) -> bool {
    let host = hostname.to_lowercase();
    IMAGE_HOSTS.contains(&host.as_str()) || host.ends_with(IMAGE_HOST_SUFFIX)
}

fn skip_whitespace(text: &str) -> &str {
    text.trim_start_matches(is_whitespace)
}

/// `avatarUrlFrom(xml)`: адрес из первого `<avatarMedium>` с адресом `https://…` (в CDATA или
/// без), как у выражения
/// `/<avatarMedium>\s*(?:<!\[CDATA\[)?\s*(https:\/\/[^\]\s<]+)\s*(?:\]\]>)?\s*<\/avatarMedium>/i`.
/// Регистр теги не различают (только ASCII, как флаг `i` без `u`); `\s` — пробелы JS.
pub fn avatar_url_from(xml: &str) -> Option<String> {
    // Строчная копия: те же байты и границы знаков, буквы ASCII — строчные.
    let lower = xml.to_ascii_lowercase();
    for (at, open) in lower.match_indices("<avatarmedium>") {
        let mut offset = at + open.len();
        let skip = |offset: usize| {
            offset + (lower[offset..].len() - skip_whitespace(&lower[offset..]).len())
        };
        offset = skip(offset);
        if lower[offset..].starts_with("<![cdata[") {
            offset += "<![cdata[".len();
        }
        offset = skip(offset);
        if !lower[offset..].starts_with("https://") {
            continue;
        }
        let start = offset;
        offset += "https://".len();
        let length = lower[offset..]
            .find(|c: char| c == ']' || c == '<' || is_whitespace(c))
            .unwrap_or(lower.len() - offset);
        if length == 0 {
            continue;
        }
        offset += length;
        offset = skip(offset);
        if lower[offset..].starts_with("]]>") {
            offset += "]]>".len();
        }
        offset = skip(offset);
        if lower[offset..].starts_with("</avatarmedium>") {
            return Some(xml[start..start + "https://".len() + length].to_owned());
        }
    }
    None
}

/// `imageTypeOf(data)`: `png` или `jpeg` по началу файла; короче 8 байт — ничего.
pub fn image_type_of(data: &[u8]) -> Option<&'static str> {
    if data.len() < 8 {
        return None;
    }
    if data[..4] == [0x89, 0x50, 0x4e, 0x47] {
        return Some("png");
    }
    if data[..3] == [0xff, 0xd8, 0xff] {
        return Some("jpeg");
    }
    None
}

/// Параметры одного запроса (`options` функции `get`).
#[derive(Clone, Copy)]
pub struct GetOptions {
    /// `allowHost(hostname)`: на каждом шаге редиректа; `None` — любой.
    pub allow_host: Option<fn(&str) -> bool>,
    /// Заголовок `accept`.
    pub accept: &'static str,
    /// `maxBytes`: больше — отказ.
    pub max_bytes: usize,
}

impl Default for GetOptions {
    fn default() -> Self {
        GetOptions {
            allow_host: None,
            accept: "*/*",
            max_bytes: MAX_AVATAR_BYTES as usize,
        }
    }
}

/// `get(url, options)`: тело ответа 200 или `None`.
pub trait HttpGet: Send + Sync {
    fn get<'a>(&'a self, url: &'a str, options: GetOptions) -> BoxFuture<'a, Option<Vec<u8>>>;
}

/// Подмена адреса для проверок: настоящий адрес → тот, по которому идёт запрос. Только в
/// отладочной сборке и с `dev-hooks`: в выпуске запрос всегда уходит по проверенному адресу.
#[cfg(any(debug_assertions, feature = "dev-hooks"))]
pub type Remap = Arc<dyn Fn(&Url) -> Url + Send + Sync>;

/// Настоящие запросы https (`get` модуля 0.4.7).
pub struct WebGet {
    client: reqwest::Client,
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    remap: Option<Remap>,
    timeout: Duration,
}

impl Default for WebGet {
    fn default() -> Self {
        WebGet::new()
    }
}

impl WebGet {
    pub fn new() -> WebGet {
        WebGet {
            client: hlsr_delivery::http_client::client_builder()
                .build()
                .expect("клиент HTTP собирается из постоянных настроек"),
            #[cfg(any(debug_assertions, feature = "dev-hooks"))]
            remap: None,
            timeout: Duration::from_millis(REQUEST_TIMEOUT_MS),
        }
    }

    /// Только для проверок: запрос уходит по подменённому адресу (хост и протокол исходного
    /// адреса уже проверены). Есть только в отладочной сборке и с `dev-hooks`.
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    pub fn with_remap(mut self, remap: Remap) -> WebGet {
        self.remap = Some(remap);
        self
    }

    /// Тайм-аут простоя (по умолчанию 8 с).
    pub fn with_timeout(mut self, timeout: Duration) -> WebGet {
        self.timeout = timeout;
        self
    }

    async fn fetch(&self, url: &str, options: GetOptions, redirects_left: u32) -> Option<Vec<u8>> {
        let mut url = url.to_owned();
        let mut left = redirects_left;
        loop {
            let parsed = Url::parse(&url).ok()?;
            if parsed.scheme() != "https" {
                return None;
            }
            if let Some(allow) = options.allow_host
                && !allow(parsed.host_str().unwrap_or(""))
            {
                return None;
            }
            #[cfg(any(debug_assertions, feature = "dev-hooks"))]
            let target = match &self.remap {
                Some(remap) => remap(&parsed),
                None => parsed.clone(),
            };
            #[cfg(not(any(debug_assertions, feature = "dev-hooks")))]
            let target = parsed.clone();

            let send = self
                .client
                .get(target)
                .header(reqwest::header::USER_AGENT, "HLSR")
                .header(reqwest::header::ACCEPT, options.accept)
                .send();
            let mut response = tokio::time::timeout(self.timeout, send).await.ok()?.ok()?;
            let status = response.status().as_u16();

            if (301..=308).contains(&status) && left > 0 {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned);
                if let Some(location) = location {
                    drop(response);
                    url = parsed.join(&location).ok()?.to_string();
                    left -= 1;
                    continue;
                }
            }
            if status != 200 {
                return None;
            }

            let mut body = Vec::new();
            loop {
                let chunk = tokio::time::timeout(self.timeout, response.chunk())
                    .await
                    .ok()?
                    .ok()?;
                let Some(chunk) = chunk else {
                    return Some(body);
                };
                if body.len() + chunk.len() > options.max_bytes {
                    return None;
                }
                body.extend_from_slice(&chunk);
            }
        }
    }
}

impl HttpGet for WebGet {
    fn get<'a>(&'a self, url: &'a str, options: GetOptions) -> BoxFuture<'a, Option<Vec<u8>>> {
        Box::pin(self.fetch(url, options, MAX_REDIRECTS))
    }
}

/// Параметры `fetchAvatar(steamId64, options)`.
pub struct FetchOptions {
    /// `cacheDir`: папка кеша `avatar-cache`; `None` — без кеша.
    pub cache_dir: Option<String>,
    /// `get`: запросы; по умолчанию — настоящие.
    pub get: Option<Arc<dyn HttpGet>>,
    pub platform: Platform,
}

impl Default for FetchOptions {
    fn default() -> Self {
        FetchOptions {
            cache_dir: None,
            get: None,
            platform: Platform::CURRENT,
        }
    }
}

/// `^\d{1,20}$`.
fn is_id(text: &str) -> bool {
    (1..=20).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit())
}

fn data_uri(kind: &str, data: &[u8]) -> String {
    format!("data:image/{kind};base64,{}", STANDARD.encode(data))
}

/// Аватар одного человека: сначала кеш на диске, потом сеть. `data:`-ссылка или `None`.
pub async fn fetch_avatar(steam_id64: &str, options: &FetchOptions) -> Option<String> {
    if !is_id(steam_id64) {
        return None;
    }
    let cache_file = options
        .cache_dir
        .as_deref()
        .map(|dir| node_path(options.platform).join(&[dir, &format!("{steam_id64}.bin")]));

    if let Some(file) = &cache_file
        && let Ok(data) = tokio::fs::read(file).await
        && let Some(kind) = image_type_of(&data)
    {
        return Some(data_uri(kind, &data));
    }

    let get: Arc<dyn HttpGet> = options
        .get
        .clone()
        .unwrap_or_else(|| Arc::new(WebGet::new()));

    let xml = get
        .get(
            &format!("https://{PROFILE_HOST}/profiles/{steam_id64}?xml=1"),
            GetOptions {
                allow_host: Some(|host| host.to_lowercase() == PROFILE_HOST),
                accept: "text/xml",
                max_bytes: MAX_PROFILE_BYTES,
            },
        )
        .await?;

    let url = avatar_url_from(&String::from_utf8_lossy(&xml))?;

    let image = get
        .get(
            &url,
            GetOptions {
                allow_host: Some(is_allowed_image_host),
                ..GetOptions::default()
            },
        )
        .await?;
    let kind = image_type_of(&image)?;

    if let Some(file) = &cache_file {
        // Кеш — удобство: не записался, и ладно.
        if let Some(dir) = Path::new(file).parent() {
            let _ = tokio::fs::create_dir_all(dir).await;
        }
        let _ = tokio::fs::write(file, &image).await;
    }

    Some(data_uri(kind, &image))
}
