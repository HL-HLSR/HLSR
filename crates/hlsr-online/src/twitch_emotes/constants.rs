//! `twitch-emotes/constants.js`: CDN Twitch, ступени масштаба и лимиты.

use hlsr_core::js::encode_uri_component;

pub const CDN_HOST: &str = "static-cdn.jtvnw.net";

/// Ступень CDN: файл (`1.0`, `2.0`, `3.0`) и высота картинки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeTier {
    pub scale: &'static str,
    pub tall: u32,
}

pub const SIZE_TIERS: [SizeTier; 3] = [
    SizeTier {
        scale: "1.0",
        tall: 28,
    },
    SizeTier {
        scale: "2.0",
        tall: 56,
    },
    SizeTier {
        scale: "3.0",
        tall: 84,
    },
];

/// Первую ступень CDN не отдаёт в нужном качестве: берётся вторая.
pub const MIN_SCALE: &str = "2.0";

/// Высота эмоута по умолчанию.
pub const EMOTE_PX: u32 = 32;

pub const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
pub const REQUEST_TIMEOUT_MS: u64 = 8000;
pub const MAX_CACHED_IMAGES: usize = 256;
pub const LOG_PREFIX: &str = "[twitch-emotes]";

/// `pickScale(px)`.
pub fn pick_scale(px: Option<u32>) -> &'static str {
    let want = px.filter(|&px| px > 0).unwrap_or(EMOTE_PX);
    for size in SIZE_TIERS {
        if size.tall >= want {
            return if size.scale == "1.0" {
                MIN_SCALE
            } else {
                size.scale
            };
        }
    }
    SIZE_TIERS[SIZE_TIERS.len() - 1].scale
}

/// `emoteImageUrl(id, px)`.
pub fn emote_image_url(id: &str, px: Option<u32>) -> String {
    format!(
        "https://{CDN_HOST}/emoticons/v2/{}/default/dark/{}",
        encode_uri_component(id),
        pick_scale(px)
    )
}
