//! `youtube/` (`index.js`): мост чата YouTube. `request()` — только `https:` и хосты
//! `www.youtube.com`, `yt3.ggpht.com`, `fonts.gstatic.com`, три редиректа, 10 с;
//! [`YouTubeChatBridge`] — потребитель чата: `chat.subscribe youtube` → опрос
//! ([`YouTubeChat`]), строки `chat.line` со значками ролей, эмодзи через `emote.ready` (высота — из
//! последнего `emote.resolve`, по умолчанию 36), значок участника `yt_member`.
//!
//! Исправление ошибки 0.4.7 (карта §0.3, решение 6 спецификации ПП4): значок участника в 0.4.7 не
//! уходил никогда — `pushMemberBadge` звал несуществующий `this.fetchBinary`, `TypeError` ловился и
//! писался как `MEMBER_BADGE_FAILED`. Здесь значок скачивается так же, как эмодзи (до 256 КиБ,
//! высота 32), раскодируется и отправляется один раз на подключение игры.
//!
//! Различия с 0.4.7, которые игра не видит: reqwest добавляет запросам `accept: */*`.

pub mod constants;
pub mod youtube_chat;

use constants::*;
pub use youtube_chat::{
    Continuation, Diagnostics, Emoji, LineSink, Message, Roles, YouTubeChat, YouTubeFetch,
    read_continuation, read_message,
};

use crate::chat_http::{ChatHttp, Fetch};
use crate::fifo_map::InsertionSet;
use crate::seventv::decode_png;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hlsr_core::json_text::{json_parse, stringify};
use hlsr_core::logger::Logger;
use hlsr_delivery::ports::{ChatBridge, ChatConsumer, ChatMessage};
use hlsr_delivery::providers::BoxFuture;
use serde_json::Value;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::runtime::Handle;

fn allow_host(hostname: &str) -> bool {
    hostname == HOST || IMAGE_HOSTS.contains(&hostname)
}

/// `badgeNamesForRoles(roles)`: [ранг, положение] — `yt_owner`/`yt_moderator` и
/// `yt_verified`/`yt_member`; без ранга положение идёт первым.
pub fn badge_names_for_roles(roles: Option<&Roles>) -> [&'static str; 2] {
    let Some(roles) = roles else {
        return ["", ""];
    };
    let rank = if roles.owner {
        "yt_owner"
    } else if roles.moderator {
        "yt_moderator"
    } else {
        ""
    };
    let standing = if roles.verified {
        "yt_verified"
    } else if roles.member {
        "yt_member"
    } else {
        ""
    };
    if rank.is_empty() {
        [standing, ""]
    } else {
        [rank, standing]
    }
}

/// Сеть YouTube: `fetchText`, `postJson`, `fetchBinary` поверх [`ChatHttp`].
#[derive(Clone)]
pub struct HttpYouTube {
    http: ChatHttp,
}

impl HttpYouTube {
    pub fn new(http: ChatHttp) -> HttpYouTube {
        HttpYouTube { http }
    }

    async fn request(&self, url: &str, body: Option<&[u8]>, max_bytes: usize) -> Option<Vec<u8>> {
        self.http
            .fetch(&Fetch {
                url,
                allow: allow_host,
                user_agent: USER_AGENT,
                accept: None,
                headers: &[("accept-language", "en-US,en;q=0.9")],
                body,
                bare_host: true,
                max_bytes,
                timeout: Duration::from_millis(REQUEST_TIMEOUT_MS),
            })
            .await
    }

    /// `fetchBinary(url)`: картинка до 256 КиБ.
    pub async fn fetch_binary(&self, url: &str) -> Option<Vec<u8>> {
        self.request(url, None, MAX_EMOJI_BYTES).await
    }
}

impl YouTubeFetch for HttpYouTube {
    fn fetch_text<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            let bytes = self.request(url, None, MAX_PAGE_BYTES).await?;
            Some(String::from_utf8_lossy(&bytes).into_owned())
        })
    }

    fn post_json<'a>(&'a self, url: &'a str, body: &'a Value) -> BoxFuture<'a, Option<Value>> {
        Box::pin(async move {
            let payload = stringify(body, "");
            let bytes = self
                .request(url, Some(payload.as_bytes()), MAX_POLL_BYTES)
                .await?;
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if text.is_empty() {
                return None;
            }
            json_parse(&text).ok()
        })
    }
}

/// Значок участника: адрес, по которому он скачан, размер и RGBA в base64.
struct MemberBadge {
    url: String,
    wide: u32,
    tall: u32,
    base64: String,
}

struct BridgeState {
    sent_emoji: InsertionSet,
    px: u32,
    member_badge: Option<MemberBadge>,
    member_badge_in_flight: bool,
    member_badge_sent_for_connection: bool,
}

struct Inner {
    log: Arc<dyn Logger>,
    handle: Handle,
    net: HttpYouTube,
    chat: Arc<YouTubeChat>,
    bridge: Mutex<Option<Arc<dyn ChatBridge>>>,
    state: Mutex<BridgeState>,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, BridgeState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn bridge(&self) -> Option<Arc<dyn ChatBridge>> {
        self.bridge
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// `sendMemberBadge()`.
    fn send_member_badge(&self) {
        let Some(bridge) = self.bridge() else {
            return;
        };
        let sent = {
            let state = self.state();
            let Some(badge) = &state.member_badge else {
                return;
            };
            bridge.send_badge_ready(MEMBER_BADGE_NAME, badge.wide, badge.tall, &badge.base64)
        };
        if sent {
            self.state().member_badge_sent_for_connection = true;
        }
    }

    /// `pushMemberBadge(url)`.
    async fn push_member_badge(&self, url: Option<&str>) {
        let Some(url) = url.filter(|url| !url.is_empty()) else {
            return;
        };
        if self.bridge().is_none() {
            return;
        }

        {
            let mut state = self.state();
            if let Some(badge) = &state.member_badge {
                if badge.url != url {
                    return;
                }
                let resend = !state.member_badge_sent_for_connection;
                drop(state);
                if resend {
                    self.send_member_badge();
                }
                return;
            }
            if state.member_badge_in_flight {
                return;
            }
            state.member_badge_in_flight = true;
        }

        if let Some(png) = self.net.fetch_binary(url).await {
            let handle = self.handle.clone();
            let decoded = handle
                .spawn_blocking(move || decode_png(&png, MEMBER_BADGE_PX))
                .await
                .ok()
                .flatten();
            if let Some(decoded) = decoded.filter(|decoded| !decoded.rgba.is_empty()) {
                self.state().member_badge = Some(MemberBadge {
                    url: url.to_owned(),
                    wide: decoded.wide,
                    tall: decoded.tall,
                    base64: STANDARD.encode(&decoded.rgba),
                });
                self.send_member_badge();
            }
        }
        self.state().member_badge_in_flight = false;
    }

    /// `pushEmoji(list)`: каждое сокращение — один раз за подключение.
    async fn push_emoji(&self, list: &[Emoji]) {
        if self.bridge().is_none() || list.is_empty() {
            return;
        }
        for item in list {
            if item.shortcut.is_empty() || item.url.is_empty() {
                continue;
            }
            {
                let mut state = self.state();
                if state.sent_emoji.contains(&item.shortcut) {
                    continue;
                }
                state.sent_emoji.insert(&item.shortcut);
                if state.sent_emoji.len() > MAX_EMOJI_CACHED {
                    state.sent_emoji.keep_last(MAX_EMOJI_CACHED / 2);
                }
            }

            let Some(png) = self.net.fetch_binary(&item.url).await else {
                continue;
            };
            let px = self.state().px;
            let decoded = self
                .handle
                .clone()
                .spawn_blocking(move || decode_png(&png, px))
                .await
                .ok()
                .flatten();
            let Some(decoded) = decoded else {
                continue;
            };
            // Мост мог пропасть, пока шла загрузка: тогда отправлять некому.
            if let Some(bridge) = self.bridge() {
                bridge.send_emote_ready(
                    &item.shortcut,
                    decoded.wide,
                    decoded.tall,
                    &STANDARD.encode(&decoded.rgba),
                );
            }
            self.log.info(&format!(
                "{LOG_PREFIX} EMOJI: {} {}x{}",
                item.shortcut, decoded.wide, decoded.tall
            ));
        }
    }

    /// Строка чата: эмодзи и значок участника, потом `chat.line`.
    async fn deliver(&self, message: Message) {
        let badges = badge_names_for_roles(Some(&message.roles));
        tokio::join!(
            self.push_emoji(&message.emoji),
            self.push_member_badge(message.roles.member_url.as_deref())
        );
        if let Some(bridge) = self.bridge() {
            bridge.send_chat_line(
                "youtube",
                &message.nick,
                &message.text,
                badges[0],
                badges[1],
            );
        }
    }
}

/// Начало `async` как у JS: до первого настоящего ожидания — сразу, остальное — в задаче.
fn start_now_or_spawn<F>(handle: &Handle, future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    let mut future = Box::pin(future);
    let mut context = Context::from_waker(Waker::noop());
    if future.as_mut().poll(&mut context) == Poll::Pending {
        handle.spawn(future);
    }
}

/// `YouTubeChatBridge`: потребитель чата для YouTube.
pub struct YouTubeChatBridge {
    inner: Arc<Inner>,
}

impl YouTubeChatBridge {
    /// `delay_divisor` — во сколько раз сократить паузы опроса (в выпуске — 1).
    pub fn new(
        http: ChatHttp,
        log: Arc<dyn Logger>,
        handle: Handle,
        delay_divisor: u32,
    ) -> YouTubeChatBridge {
        let net = HttpYouTube::new(http);
        let chat = YouTubeChat::new(
            Arc::new(net.clone()),
            log.clone(),
            handle.clone(),
            delay_divisor,
        );
        YouTubeChatBridge {
            inner: Arc::new(Inner {
                log,
                handle,
                net,
                chat,
                bridge: Mutex::default(),
                state: Mutex::new(BridgeState {
                    sent_emoji: InsertionSet::default(),
                    px: DEFAULT_EMOJI_PX,
                    member_badge: None,
                    member_badge_in_flight: false,
                    member_badge_sent_for_connection: false,
                }),
            }),
        }
    }

    /// `diagnostics()`.
    pub fn diagnostics(&self) -> Diagnostics {
        self.inner.chat.diagnostics()
    }
}

impl ChatConsumer for YouTubeChatBridge {
    fn attach_bridge(&self, bridge: Arc<dyn ChatBridge>) {
        *self.inner.bridge.lock().unwrap_or_else(|p| p.into_inner()) = Some(bridge);
    }

    fn handle_message(&self, message: &ChatMessage) {
        match message {
            ChatMessage::Subscribe { platform, id, .. } if platform == "youtube" => {
                let inner = self.inner.clone();
                let id = id.clone();
                self.inner.handle.spawn(async move {
                    let sink_inner = inner.clone();
                    let sink: LineSink = Arc::new(move |message: &Message| {
                        let inner = sink_inner.clone();
                        let handle = inner.handle.clone();
                        let message = message.clone();
                        start_now_or_spawn(&handle, async move { inner.deliver(message).await });
                    });
                    inner.chat.start(Some(&id), sink).await;
                });
            }
            ChatMessage::EmoteResolve { px: Some(px), .. } if *px > 0 => {
                self.inner.state().px = *px;
            }
            ChatMessage::Unsubscribe { .. } => self.inner.chat.stop(),
            _ => {}
        }
    }

    fn handle_ready(&self) {
        self.inner.state().member_badge_sent_for_connection = false;
        if self.inner.state().member_badge.is_some() {
            self.inner.send_member_badge();
        }
    }

    fn forget(&self) {
        self.inner.chat.stop();
        let mut state = self.inner.state();
        state.sent_emoji.clear();
        state.member_badge_sent_for_connection = false;
    }
}
