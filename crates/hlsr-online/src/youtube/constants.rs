//! `youtube/constants.js`: адреса страницы чата и `get_live_chat`, User-Agent Chrome 131, лимиты
//! и тайминги.

use hlsr_core::js::{encode_uri_component, trim};
use regex::Regex;
use std::sync::LazyLock;

pub const HOST: &str = "www.youtube.com";

/// Хосты картинок (эмодзи и значки участников).
pub const IMAGE_HOSTS: [&str; 2] = ["yt3.ggpht.com", "fonts.gstatic.com"];

/// `USER_AGENT`: Chrome 131 на той же ОС, что и лаунчер (Linux или Windows).
pub const USER_AGENT: &str = if cfg!(target_os = "linux") {
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
} else {
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
};

pub const MAX_PAGE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_POLL_BYTES: usize = 2 * 1024 * 1024;
pub const REQUEST_TIMEOUT_MS: u64 = 10_000;

pub const DEFAULT_POLL_MS: f64 = 1000.0;
pub const MIN_POLL_MS: f64 = 1000.0;
pub const MAX_POLL_MS: f64 = 30_000.0;
/// Продолжение просит не больше секунды (`POLL_CAP_MS`): опрос идёт раз в секунду.
pub const POLL_CAP_MS: f64 = 1000.0;

pub const MAX_LINES_PER_POLL: usize = 8;
pub const MAX_CONSECUTIVE_FAILURES: u32 = 5;

pub const MAX_EMOJI_BYTES: usize = 256 * 1024;
pub const DEFAULT_EMOJI_PX: u32 = 36;

pub const MEMBER_BADGE_PX: u32 = 32;
pub const MEMBER_BADGE_NAME: &str = "yt_member";
pub const MAX_EMOJI_CACHED: usize = 256;

pub const LOG_PREFIX: &str = "[youtube]";

/// `liveChatUrl(videoId)`.
pub fn live_chat_url(video_id: &str) -> String {
    format!(
        "https://{HOST}/live_chat?is_popout=1&v={}",
        encode_uri_component(video_id)
    )
}

/// `pollUrl(apiKey)`.
pub fn poll_url(api_key: &str) -> String {
    format!(
        "https://{HOST}/youtubei/v1/live_chat/get_live_chat?key={}",
        encode_uri_component(api_key)
    )
}

static VIDEO_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{5,32}$").expect("шаблон верен"));
static BY_QUERY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[?&]v=([A-Za-z0-9_-]{5,32})").expect("шаблон верен"));
static BY_SHORT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"youtu\.be/([A-Za-z0-9_-]{5,32})").expect("шаблон верен"));
static BY_LIVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"youtube\.com/live/([A-Za-z0-9_-]{5,32})").expect("шаблон верен"));

/// `VIDEO_ID_RE.test(text)`.
pub fn is_video_id(text: &str) -> bool {
    VIDEO_ID.is_match(text)
}

/// `parseVideoId(input)`: идентификатор, адрес `watch?v=`, `youtu.be/` или `youtube.com/live/`.
/// `input` — уже `String(input || "")`.
pub fn parse_video_id(input: &str) -> Option<String> {
    let raw = trim(input);
    if is_video_id(raw) {
        return Some(raw.to_owned());
    }
    for pattern in [&BY_QUERY, &BY_SHORT, &BY_LIVE] {
        if let Some(found) = pattern.captures(raw) {
            return Some(found[1].to_owned());
        }
    }
    None
}
