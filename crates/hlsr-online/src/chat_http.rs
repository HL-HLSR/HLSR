//! Сеть мостов чата: `get()` из `seventv/index.js` и `twitch-emotes/index.js`, `request()` из
//! `youtube/index.js` — одна реализация на всех.
//!
//! Что делает 0.4.7 (и здесь): только `https:` и только хосты из списка модуля; до трёх
//! редиректов (статусы 301…308 с `location`, адрес считается от текущего); любой статус кроме 200 —
//! `None`; тело больше лимита — `None`; простой сокета дольше тайм-аута (8 с у 7TV и Twitch, 10 с у
//! YouTube) — `None`; сбой соединения — `None`. Запрос YouTube с телом — `POST` с
//! `content-type: application/json`; редирект запроса с телом снова `POST` с тем же телом.
//!
//! Отличия от Node (не видны игре): reqwest сам добавляет `accept: */*` запросам, у которых нет
//! `accept` (YouTube). TLS — как у доставки и транспорта API (`client_builder`).
//!
//! Для сверки с двойником на `127.0.0.1` хосты можно перенаправить на локальные адреса
//! ([`ChatHttp::with_origins`]): модуль по-прежнему проверяет `https:` и свой список хостов по
//! настоящему адресу, а соединение идёт по `http:` на двойник. Перенаправление действует только в
//! отладочной сборке или с `dev-hooks` (в выпуске его нет — как нет QA-хода у транспорта API).

use hlsr_delivery::http_client::client_builder;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

/// Сколько редиректов разрешено (`MAX_REDIRECTS` у всех трёх модулей).
pub const MAX_REDIRECTS: u32 = 3;

/// Один запрос: что и как просит модуль.
pub struct Fetch<'a> {
    pub url: &'a str,
    /// `allowHost(hostname)` модуля (имя в нижнем регистре).
    pub allow: fn(&str) -> bool,
    pub user_agent: &'a str,
    /// Заголовок `accept`; `None` — не посылать (YouTube).
    pub accept: Option<&'a str>,
    /// Прочие заголовки (`accept-language` YouTube).
    pub headers: &'a [(&'a str, &'a str)],
    /// Тело `POST` (JSON); `None` — `GET`.
    pub body: Option<&'a [u8]>,
    /// YouTube собирает адрес из `hostname` и пути: порт и учётные данные адреса отбрасываются.
    pub bare_host: bool,
    pub max_bytes: usize,
    /// Простой сокета (`REQUEST_TIMEOUT_MS` модуля).
    pub timeout: Duration,
}

/// Сеть мостов чата; копии делят пул соединений.
#[derive(Clone)]
pub struct ChatHttp {
    client: reqwest::Client,
    /// Хост → `127.0.0.1:порт` двойника; `None` — настоящая сеть.
    origins: Option<Arc<HashMap<String, String>>>,
    /// Замена тайм-аутов модулей (сверка с тайм-аутом в сотни миллисекунд).
    timeout: Option<Duration>,
}

impl Default for ChatHttp {
    fn default() -> Self {
        ChatHttp::new()
    }
}

impl ChatHttp {
    pub fn new() -> ChatHttp {
        ChatHttp {
            client: client_builder()
                .build()
                .expect("клиент HTTP собирается из постоянных настроек"),
            origins: None,
            timeout: None,
        }
    }

    /// Сеть с перенаправлением хостов на двойники: пары (хост, `127.0.0.1:порт`). Хост, которого
    /// в списке нет, недоступен — как порт, где никто не слушает. В выпуске без `dev-hooks` список
    /// не действует.
    pub fn with_origins(origins: impl IntoIterator<Item = (String, String)>) -> ChatHttp {
        let mut http = ChatHttp::new();
        http.origins = Some(Arc::new(origins.into_iter().collect()));
        http
    }

    /// Заменить тайм-ауты модулей (сверка с двойником, который отвечает медленно).
    pub fn with_timeout(mut self, timeout: Duration) -> ChatHttp {
        self.timeout = Some(timeout);
        self
    }

    /// Список двойников, если он действует (отладочная сборка или `dev-hooks`).
    fn redirected(&self) -> Option<&HashMap<String, String>> {
        self.origins
            .as_deref()
            .filter(|_| cfg!(any(debug_assertions, feature = "dev-hooks")))
    }

    /// Выполнить запрос; тело ответа или `None`.
    pub async fn fetch(&self, request: &Fetch<'_>) -> Option<Vec<u8>> {
        let mut current = Url::parse(request.url).ok()?;
        let mut left = MAX_REDIRECTS;
        loop {
            if current.scheme() != "https" {
                return None;
            }
            let host = current.host_str().unwrap_or("").to_ascii_lowercase();
            if !(request.allow)(&host) {
                return None;
            }
            let target = self.target(&current, &host, request.bare_host)?;
            let idle = self.timeout.unwrap_or(request.timeout);

            let mut headers = HeaderMap::new();
            let mut put = |name: &str, value: &str| -> Option<()> {
                headers.insert(
                    HeaderName::from_bytes(name.as_bytes()).ok()?,
                    HeaderValue::from_str(value).ok()?,
                );
                Some(())
            };
            put("user-agent", request.user_agent)?;
            if let Some(accept) = request.accept {
                put("accept", accept)?;
            }
            for (name, value) in request.headers {
                put(name, value)?;
            }
            let mut builder = match request.body {
                Some(_) => self.client.post(target),
                None => self.client.get(target),
            };
            if let Some(body) = request.body {
                put("content-type", "application/json")?;
                builder = builder.body(body.to_vec());
            }
            let mut response = tokio::time::timeout(idle, builder.headers(headers).send())
                .await
                .ok()?
                .ok()?;

            let status = response.status().as_u16();
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| String::from_utf8(value.as_bytes().to_vec()).ok())
                .filter(|value| !value.is_empty());
            if (301..=308).contains(&status) && location.is_some() && left > 0 {
                // `response.resume()` + `new URL(location, url)`: ошибка адреса — `None`.
                current = current.join(&location?).ok()?;
                left -= 1;
                continue;
            }
            if status != 200 {
                return None;
            }

            let mut body = Vec::new();
            loop {
                match tokio::time::timeout(idle, response.chunk()).await {
                    Err(_) | Ok(Err(_)) => return None,
                    Ok(Ok(None)) => return Some(body),
                    Ok(Ok(Some(chunk))) => {
                        if body.len() + chunk.len() > request.max_bytes {
                            return None;
                        }
                        body.extend_from_slice(&chunk);
                    }
                }
            }
        }
    }

    /// Адрес, по которому идёт соединение.
    fn target(&self, logical: &Url, host: &str, bare_host: bool) -> Option<Url> {
        let mut path = logical.path().to_owned();
        if let Some(query) = logical.query() {
            path.push('?');
            path.push_str(query);
        }
        if let Some(origins) = self.redirected() {
            let authority = origins.get(host)?;
            return Url::parse(&format!("http://{authority}{path}")).ok();
        }
        if bare_host {
            return Url::parse(&format!("https://{host}{path}")).ok();
        }
        Some(logical.clone())
    }
}
