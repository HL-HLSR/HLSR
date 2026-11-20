//! Каналы окна «Спонсорство» (`telemetry/sponsors-ipc.js` 0.4.9): `sponsor-open-link` и
//! `support-open`. Окно присылает только раздел и номер записи (или ключ кнопки); адрес берётся из
//! последнего списка клиента ([`SponsorsClient::last_list`]), окно не может подсунуть свой. Любая
//! ошибка — `{status: "error", code: "SPONSOR_LINK_INVALID"}`.

use super::news_ipc::{NewsLinks, external_url};
use super::sponsors_client::{SponsorsClient, bot_url};
use crate::js_value;
use crate::shared::sponsor_sections::{SECTIONS, SUPPORT_KEYS};
use serde_json::{Value, json};
use std::sync::Arc;

/// Код ошибки обоих каналов.
pub const SPONSOR_LINK_INVALID: &str = "SPONSOR_LINK_INVALID";

fn invalid() -> Value {
    json!({"status": "error", "code": SPONSOR_LINK_INVALID})
}

pub struct SponsorsIpc {
    client: Arc<SponsorsClient>,
    links: Arc<dyn NewsLinks>,
}

impl SponsorsIpc {
    /// `links` — диалог подтверждения и `openExternal`, те же, что у ссылок новостей.
    pub fn new(client: Arc<SponsorsClient>, links: Arc<dyn NewsLinks>) -> SponsorsIpc {
        SponsorsIpc { client, links }
    }

    /// `sponsor-open-link({section, index})`: свой адрес записи после подтверждения.
    pub async fn open_link(&self, target: Option<&Value>) -> Value {
        self.try_open_link(target).await.unwrap_or_else(invalid)
    }

    async fn try_open_link(&self, target: Option<&Value>) -> Option<Value> {
        let section = js_value::field(target, "section")
            .and_then(Value::as_str)
            .filter(|section| SECTIONS.contains(section))?;
        let index = js_value::finite_number(js_value::field(target, "index"))
            .filter(|index| index.fract() == 0.0 && *index >= 0.0)?;
        let list = self.client.last_list()?;
        let entry = list.get(section)?.as_array()?.get(index as usize)?;
        let link = entry.get("link")?;
        if link.get("mode")?.as_str()? != "url" {
            return None;
        }
        let url = external_url(link.get("url")?.as_str()?).ok()?;
        let host = url.host_str().unwrap_or_default();
        if !self.links.confirm_link(host, url.as_str()).await {
            return Some(json!({"status": "cancelled"}));
        }
        self.links.open_external(url.as_str()).await.ok()?;
        Some(json!({"status": "ok"}))
    }

    /// `support-open(key, code?)`: бот без подтверждения. «Подписка» и «Донат» — бот с
    /// `?start=subscribe|donate`, `bot` — бот как есть или с кодом привязки (`?start=<6 цифр>`).
    /// Адреса Tribute лаунчер не открывает: оплата идёт через бота. Списка ещё нет (окно
    /// «Спонсорство» не открывали) — сначала он запрашивается.
    pub async fn support_open(&self, key: Option<&Value>, code: Option<&Value>) -> Value {
        self.try_support_open(key, code)
            .await
            .unwrap_or_else(invalid)
    }

    async fn try_support_open(&self, key: Option<&Value>, code: Option<&Value>) -> Option<Value> {
        let key = key?.as_str().filter(|key| SUPPORT_KEYS.contains(key))?;
        let code = match code {
            None => None,
            Some(code) => Some(code.as_str().filter(|code| {
                key == "bot" && code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit())
            })?),
        };
        if self.client.last_list().is_none() {
            self.client.get(false).await;
        }
        let list = self.client.last_list();
        let bot = list.as_ref().and_then(|list| list.get("links")?.get("bot"));
        let url = bot_url(bot, if key == "bot" { code } else { Some(key) })?;
        self.links.open_external(&url).await.ok()?;
        Some(json!({"status": "ok"}))
    }
}
