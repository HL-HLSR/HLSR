//! `twitch-emotes/` (`index.js`): эмоуты Twitch в игре. `get()` — только `https:` и хост
//! `static-cdn.jtvnw.net`; [`TwitchEmoteBridge`] — потребитель чата: `emote.resolve` с именем
//! `tw:<id>` → промах, неподвижный эмоут или анимация. Чата Twitch в лаунчере нет (карта §0.1).

pub mod constants;
mod twitch_emote_service;

pub use twitch_emote_service::TwitchEmoteService;

use crate::chat_http::ChatHttp;
use crate::seventv::send_resolved;
use hlsr_delivery::ports::{ChatBridge, ChatConsumer, ChatMessage, Logger};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;

/// Приставка имён эмоутов Twitch (`protocol.EMOTE_NS_TWITCH`).
pub const EMOTE_NS_TWITCH: &str = "tw:";

/// `protocol.TWITCH_EMOTE_ID_RE`: `^[A-Za-z0-9_]{1,44}$`.
pub fn is_twitch_emote_id(id: &str) -> bool {
    (1..=44).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// `TwitchEmoteBridge`: потребитель чата для эмоутов Twitch.
pub struct TwitchEmoteBridge {
    log: Arc<dyn Logger>,
    handle: Handle,
    service: Arc<TwitchEmoteService>,
    bridge: Arc<Mutex<Option<Arc<dyn ChatBridge>>>>,
}

impl TwitchEmoteBridge {
    pub fn new(http: ChatHttp, log: Arc<dyn Logger>, handle: Handle) -> TwitchEmoteBridge {
        TwitchEmoteBridge {
            service: TwitchEmoteService::new(http, log.clone(), handle.clone()),
            log,
            handle,
            bridge: Arc::default(),
        }
    }

    /// `diagnostics()`: `{cached}`.
    pub fn diagnostics(&self) -> Value {
        self.service.diagnostics()
    }
}

impl ChatConsumer for TwitchEmoteBridge {
    fn attach_bridge(&self, bridge: Arc<dyn ChatBridge>) {
        *self.bridge.lock().unwrap_or_else(|p| p.into_inner()) = Some(bridge);
    }

    fn handle_message(&self, message: &ChatMessage) {
        let ChatMessage::EmoteResolve { name, px, .. } = message else {
            return;
        };
        let Some(id) = name.strip_prefix(EMOTE_NS_TWITCH) else {
            return;
        };
        if !is_twitch_emote_id(id) {
            self.log
                .warn(&format!("{} BAD_ID: {name}", constants::LOG_PREFIX));
            return;
        }

        let (name, id, px) = (name.clone(), id.to_owned(), *px);
        let service = self.service.clone();
        let bridge = self.bridge.clone();
        let log = self.log.clone();
        self.handle.spawn(async move {
            match service.resolve(&id, px).await {
                Ok(image) => {
                    let current = bridge.lock().unwrap_or_else(|p| p.into_inner()).clone();
                    if let Some(current) = current {
                        send_resolved(current.as_ref(), &name, image.as_ref());
                    }
                }
                Err(message) => log.warn(&format!(
                    "{} RESOLVE_FAILED: {name}: {message}",
                    constants::LOG_PREFIX
                )),
            }
        });
    }
}
