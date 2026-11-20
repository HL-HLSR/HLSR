//! `twitch-badges/index.js`: девять значков (Twitch и YouTube) из `icons/badges`, высота 64 px;
//! отправляются игре один раз на подключение (`handleReady`). PNG вшиты в программу
//! (`include_bytes!`): рядом с `HLSR.exe` папки `icons` у Rust-сборки нет.

use crate::seventv::decode_png;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hlsr_delivery::ports::{ChatBridge, ChatConsumer, ChatMessage, Logger};
use std::sync::{Arc, Mutex, MutexGuard};

/// Значки Twitch.
pub const BADGE_NAMES: [&str; 6] = [
    "broadcaster",
    "staff",
    "moderator",
    "vip",
    "partner",
    "subscriber",
];

/// Значки YouTube.
pub const YT_BADGE_NAMES: [&str; 3] = ["yt_owner", "yt_moderator", "yt_verified"];

/// Высота значка в игре.
pub const BADGE_PX: u32 = 64;

/// Строка протокола с запасом не больше 64 КиБ (буфер игры).
const MAX_BADGE_LINE_HINT: usize = 64 * 1024;

/// PNG значка по имени (`icons/badges/<имя>.png`).
pub fn badge_png(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "broadcaster" => &include_bytes!("../../../../icons/badges/broadcaster.png")[..],
        "staff" => &include_bytes!("../../../../icons/badges/staff.png")[..],
        "moderator" => &include_bytes!("../../../../icons/badges/moderator.png")[..],
        "vip" => &include_bytes!("../../../../icons/badges/vip.png")[..],
        "partner" => &include_bytes!("../../../../icons/badges/partner.png")[..],
        "subscriber" => &include_bytes!("../../../../icons/badges/subscriber.png")[..],
        "yt_owner" => &include_bytes!("../../../../icons/badges/yt_owner.png")[..],
        "yt_moderator" => &include_bytes!("../../../../icons/badges/yt_moderator.png")[..],
        "yt_verified" => &include_bytes!("../../../../icons/badges/yt_verified.png")[..],
        _ => return None,
    })
}

struct Badge {
    name: &'static str,
    wide: u32,
    tall: u32,
    base64: String,
}

#[derive(Default)]
struct State {
    sent_for_connection: bool,
    cache: Option<Arc<Vec<Badge>>>,
}

/// `TwitchBadgeBridge`: потребитель чата, который шлёт игре значки.
pub struct TwitchBadgeBridge {
    log: Arc<dyn Logger>,
    bridge: Mutex<Option<Arc<dyn ChatBridge>>>,
    state: Mutex<State>,
}

impl TwitchBadgeBridge {
    pub fn new(log: Arc<dyn Logger>) -> TwitchBadgeBridge {
        TwitchBadgeBridge {
            log,
            bridge: Mutex::default(),
            state: Mutex::default(),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `load()`: разобранные значки (один раз); негодные пропускаются с записью в журнал.
    fn load(&self) -> Arc<Vec<Badge>> {
        if let Some(cache) = self.state().cache.clone() {
            return cache;
        }
        let mut loaded = Vec::new();
        for name in BADGE_NAMES.iter().chain(YT_BADGE_NAMES.iter()) {
            let decoded = badge_png(name).and_then(|png| decode_png(png, BADGE_PX));
            let Some(decoded) = decoded.filter(|d| d.wide != 0 && d.tall != 0) else {
                self.log
                    .warn(&format!("[twitch-badges] {name}: не удалось раскодировать"));
                continue;
            };
            let base64 = STANDARD.encode(&decoded.rgba);
            if base64.len() + 256 > MAX_BADGE_LINE_HINT {
                self.log.warn(&format!(
                    "[twitch-badges] {name}: строка не влезает в буфер игры"
                ));
                continue;
            }
            loaded.push(Badge {
                name,
                wide: decoded.wide,
                tall: decoded.tall,
                base64,
            });
        }
        let loaded = Arc::new(loaded);
        self.state().cache = Some(loaded.clone());
        loaded
    }

    /// `pushAll()`: сколько значков принято мостом.
    fn push_all(&self) -> usize {
        let Some(bridge) = self
            .bridge
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        else {
            return 0;
        };
        let badges = self.load();
        let sent = badges
            .iter()
            .filter(|badge| {
                bridge.send_badge_ready(badge.name, badge.wide, badge.tall, &badge.base64)
            })
            .count();
        if sent > 0 {
            self.log
                .info(&format!("[twitch-badges] значков отправлено: {sent}"));
        }
        sent
    }
}

impl ChatConsumer for TwitchBadgeBridge {
    fn attach_bridge(&self, bridge: Arc<dyn ChatBridge>) {
        *self.bridge.lock().unwrap_or_else(|p| p.into_inner()) = Some(bridge);
    }

    fn handle_message(&self, _message: &ChatMessage) {}

    fn handle_ready(&self) {
        {
            let mut state = self.state();
            if state.sent_for_connection {
                return;
            }
            state.sent_for_connection = true;
        }
        self.push_all();
    }

    fn forget(&self) {
        self.state().sent_for_connection = false;
    }
}
