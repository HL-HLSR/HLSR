//! Каналы ленты, которые меняют состояние (`telemetry/news-ipc.js`): `public-news-react`,
//! `public-news-mine` и `public-news-open-link`. Окно присылает только вид реакции и номер поста;
//! ответ сервера и «мои» реакции проходят проверку заново. Ссылка новости открывается только после
//! подтверждения настоящего HTTPS-хоста.
//!
//! Службы профиля и системы (`action` онлайн-профиля, диалог, `openExternal`) — трейты: их
//! реализует приложение. Результаты — те же объекты `{status, …}`, что отдавал IPC 0.4.7.

use crate::shared::news_contract::{self, reaction_key};
use hlsr_core::js;
use hlsr_delivery::providers::BoxFuture;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use url::Url;

use super::news_client::NewsClient;

/// Код, когда у исключения его нет (`error.code || "NEWS_FAILED"`).
pub const NEWS_FAILED: &str = "NEWS_FAILED";

/// Ошибка канала ленты: `code` берёт окно (`RUNNER_AUTH_REQUIRED`, `RUNNER_HTTP_429`, …).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct NewsError {
    pub code: Option<String>,
    pub message: String,
}

impl NewsError {
    /// Ошибка без кода (проверка не прошла).
    pub fn new(message: impl Into<String>) -> NewsError {
        NewsError {
            code: None,
            message: message.into(),
        }
    }

    /// Ошибка с кодом службы профиля.
    pub fn with_code(code: impl Into<String>, message: impl Into<String>) -> NewsError {
        NewsError {
            code: Some(code.into()),
            message: message.into(),
        }
    }

    fn reply(&self) -> Value {
        json!({
            "status": "error",
            "code": self.code.as_deref().filter(|code| !code.is_empty()).unwrap_or(NEWS_FAILED),
        })
    }
}

/// `client.updateReactions`.
pub trait NewsReactions: Send + Sync {
    fn update_reactions(&self, id: &str, raw: &Value);
}

impl NewsReactions for NewsClient {
    fn update_reactions(&self, id: &str, raw: &Value) {
        NewsClient::update_reactions(self, id, raw);
    }
}

/// Онлайн-профиль для действий ленты: `profile.action(path, body)` и `profile.achieve(id)`.
pub trait NewsProfile: Send + Sync {
    /// Запрос от имени вошедшего профиля (`/v2/runner/<path>`); без входа — ошибка с кодом.
    fn action<'a>(&'a self, path: &'a str, body: Value) -> BoxFuture<'a, Result<Value, NewsError>>;

    /// Открыть достижение (`typeof profile.achieve === "function"`).
    fn achieve(&self, _id: &str) {}
}

/// Диалог подтверждения и открытие ссылки в браузере.
pub trait NewsLinks: Send + Sync {
    /// Спросить, открывать ли ссылку на хост `host` (`url` — полный адрес).
    fn confirm_link<'a>(&'a self, host: &'a str, url: &'a str) -> BoxFuture<'a, bool>;

    /// `openExternal(url)`.
    fn open_external<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<(), NewsError>>;
}

/// `externalUrl(raw)`: только `https`, с хостом, без учётных данных и порта, без пробелов и
/// управляющих знаков, не длиннее 2048 единиц UTF-16.
pub fn external_url(raw: &str) -> Result<Url, NewsError> {
    let invalid = || NewsError::new("Неверный адрес новости");
    if js::text::utf16_len(raw) > 2048 || raw.chars().any(|c| (c as u32) <= 32 || c as u32 == 127) {
        return Err(invalid());
    }
    let url = Url::parse(raw).map_err(|_| NewsError::new("Invalid URL"))?;
    if url.scheme() != "https"
        || url.host_str().is_none_or(str::is_empty)
        || !url.username().is_empty()
        || url.password().is_some_and(|password| !password.is_empty())
        || url.port().is_some()
    {
        return Err(invalid());
    }
    Ok(url)
}

pub struct NewsIpc {
    client: Arc<dyn NewsReactions>,
    profile: Arc<dyn NewsProfile>,
    links: Arc<dyn NewsLinks>,
}

impl NewsIpc {
    pub fn new(
        client: Arc<dyn NewsReactions>,
        profile: Arc<dyn NewsProfile>,
        links: Arc<dyn NewsLinks>,
    ) -> NewsIpc {
        NewsIpc {
            client,
            profile,
            links,
        }
    }

    /// `public-news-react`: `{status: "ok", postId, reactions, mine}` или `{status: "error", code}`.
    pub async fn react(&self, post_id: &str, reaction: &str) -> Value {
        match self.try_react(post_id, reaction).await {
            Ok(reply) => reply,
            Err(error) => error.reply(),
        }
    }

    async fn try_react(&self, post_id: &str, reaction: &str) -> Result<Value, NewsError> {
        if !news_contract::valid_post_id(post_id) || reaction_key(reaction).is_none() {
            return Err(NewsError::new("Неизвестная реакция или пост"));
        }
        let reply = self
            .profile
            .action(
                "news/react",
                json!({"postId": post_id, "reaction": reaction}),
            )
            .await?;
        let counts = reply.get("reactions").filter(|counts| js::truthy(counts));
        let list = reply.get("mine").filter(|mine| mine.is_array());
        if reply.get("postId").and_then(Value::as_str) != Some(post_id)
            || counts.is_none()
            || list.is_none()
        {
            return Err(NewsError::new("Неверный ответ ленты"));
        }
        let reactions = news_contract::counts(counts);
        let mine = news_contract::mine(list);
        self.client.update_reactions(post_id, &reactions.to_json());
        if !mine.is_empty() {
            self.profile.achieve("community-voice");
        }
        Ok(json!({
            "status": "ok",
            "postId": post_id,
            "reactions": reactions.to_json(),
            "mine": mine,
        }))
    }

    /// `public-news-mine`: `{status: "ok", mine: {<postId>: [вид, …]}}`.
    pub async fn mine(&self, ids: &[String]) -> Value {
        match self.try_mine(ids).await {
            Ok(reply) => reply,
            Err(error) => error.reply(),
        }
    }

    async fn try_mine(&self, ids: &[String]) -> Result<Value, NewsError> {
        if ids.len() > 40 || !ids.iter().all(|id| news_contract::valid_post_id(id)) {
            return Err(NewsError::new("Неверный список постов"));
        }
        let mut unique: Vec<&String> = Vec::new();
        for id in ids {
            if !unique.contains(&id) {
                unique.push(id);
            }
        }
        let reply = self
            .profile
            .action("news/mine", json!({"postIds": unique}))
            .await?;
        let mut mine = Map::new();
        for id in ids {
            let list = reply.get("mine").and_then(|mine| mine.get(id.as_str()));
            mine.insert(id.clone(), json!(news_contract::mine(list)));
        }
        Ok(json!({"status": "ok", "mine": mine}))
    }

    /// `public-news-open-link`: `{status: "ok"}`, `{status: "cancelled"}` или ошибка.
    pub async fn open_link(&self, raw: &str) -> Value {
        match self.try_open_link(raw).await {
            Ok(reply) => reply,
            Err(error) => error.reply(),
        }
    }

    async fn try_open_link(&self, raw: &str) -> Result<Value, NewsError> {
        let url = external_url(raw)?;
        let host = url.host_str().unwrap_or_default();
        if !self.links.confirm_link(host, url.as_str()).await {
            return Ok(json!({"status": "cancelled"}));
        }
        self.links.open_external(url.as_str()).await?;
        Ok(json!({"status": "ok"}))
    }
}
