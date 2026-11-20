//! `seventv/constants.js`: адреса API и CDN 7TV, ступени размеров и лимиты.

use hlsr_core::js::encode_uri_component;

pub const API_HOST: &str = "7tv.io";
pub const CDN_HOST: &str = "cdn.7tv.app";

/// Ступень CDN: номер файла (`<N>x`) и высота картинки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeTier {
    pub tier: u32,
    pub tall: u32,
}

pub const SIZE_TIERS: [SizeTier; 4] = [
    SizeTier { tier: 1, tall: 32 },
    SizeTier { tier: 2, tall: 64 },
    SizeTier { tier: 3, tall: 96 },
    SizeTier { tier: 4, tall: 128 },
];

/// Меньше второй ступени CDN не берёт.
pub const MIN_SIZE_TIER: u32 = 2;

/// Высота эмоута по умолчанию.
pub const EMOTE_PX: u32 = 32;

pub const MAX_SET_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_IMAGE_BYTES: usize = 512 * 1024;
pub const REQUEST_TIMEOUT_MS: u64 = 8000;
pub const MAX_CACHED_IMAGES: usize = 256;
/// Кадров в листе анимации не больше.
pub const MAX_ANIM_FRAMES: usize = 16;
pub const MAX_GIF_BYTES: usize = 4 * 1024 * 1024;
/// Ширина не больше четырёх высот.
pub const MAX_ASPECT: u32 = 4;
/// Лист анимации не шире 2048 px.
pub const MAX_SHEET_WIDE: u32 = 2048;
pub const LOG_PREFIX: &str = "[seventv]";

pub fn emote_set_url(room_id: &str) -> String {
    format!(
        "https://{API_HOST}/v3/users/twitch/{}",
        encode_uri_component(room_id)
    )
}

pub fn emote_set_by_id_url(set_id: &str) -> String {
    format!(
        "https://{API_HOST}/v3/emote-sets/{}",
        encode_uri_component(set_id)
    )
}

pub fn seven_tv_user_url(user_id: &str) -> String {
    format!(
        "https://{API_HOST}/v3/users/{}",
        encode_uri_component(user_id)
    )
}

/// `pickSizeTier(targetPx)`: наименьшая ступень, чья высота не меньше цели, но не ниже второй.
pub fn pick_size_tier(target_px: Option<u32>) -> u32 {
    let want = target_px.filter(|&px| px > 0).unwrap_or(EMOTE_PX);
    for size in SIZE_TIERS {
        if size.tall >= want {
            return size.tier.max(MIN_SIZE_TIER);
        }
    }
    SIZE_TIERS[SIZE_TIERS.len() - 1].tier
}

pub fn emote_gif_url(emote_id: &str, target_px: Option<u32>) -> String {
    let tier = pick_size_tier(target_px);
    format!(
        "https://{CDN_HOST}/emote/{}/{tier}x.gif",
        encode_uri_component(emote_id)
    )
}

pub fn emote_image_url(emote_id: &str, animated: bool, target_px: Option<u32>) -> String {
    let tier = pick_size_tier(target_px);
    let file = if animated {
        format!("{tier}x_static.png")
    } else {
        format!("{tier}x.png")
    };
    format!(
        "https://{CDN_HOST}/emote/{}/{file}",
        encode_uri_component(emote_id)
    )
}
