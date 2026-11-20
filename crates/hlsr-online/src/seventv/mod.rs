//! `seventv/` (`index.js`): эмоуты 7TV в игре. `get()` — только `https:` и хосты `7tv.io` и
//! `cdn.7tv.app`, три редиректа, 8 с; `decode_png` и `resize_rgba` — то, что 0.4.7 делал через
//! `nativeImage` ([`native_image`]); [`SevenTvBridge`] — потребитель чата: `emote.set` →
//! `setChannel`, `emote.resolve` (кроме `tw:`) → промах, неподвижный эмоут или анимация.

pub mod constants;
pub mod gif;
pub mod native_image;
mod seven_tv_service;

pub use seven_tv_service::{
    AnimatedImage, Diagnostics, Resolved, SetPromise, SevenTvService, StillImage,
};

use crate::chat_http::{ChatHttp, Fetch};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use gif::{DecodedGif, sample_frames};
use hlsr_core::js::math_round;
use hlsr_delivery::ports::{ChatBridge, ChatConsumer, ChatMessage, Logger};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::runtime::Handle;

fn allow_host(hostname: &str) -> bool {
    hostname == constants::API_HOST || hostname == constants::CDN_HOST
}

/// `get(url, {maxBytes, accept})`: тело ответа или `None`. `max_bytes` `None` — 512 КиБ,
/// `accept` `None` — `*/*`; `timeout_ms` — `REQUEST_TIMEOUT_MS` модуля.
pub async fn get(
    http: &ChatHttp,
    url: &str,
    max_bytes: Option<usize>,
    accept: Option<&str>,
    timeout_ms: u64,
) -> Option<Vec<u8>> {
    http.fetch(&Fetch {
        url,
        allow: allow_host,
        user_agent: "HLSR",
        accept: Some(accept.unwrap_or("*/*")),
        headers: &[],
        body: None,
        bare_host: false,
        max_bytes: max_bytes.unwrap_or(constants::MAX_IMAGE_BYTES),
        timeout: Duration::from_millis(timeout_ms),
    })
    .await
}

/// Результат `decodePng`: размер и RGBA (премультиплицированный, как отдавал `toBitmap`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedPng {
    pub wide: u32,
    pub tall: u32,
    pub rgba: Vec<u8>,
}

/// Размер по высоте `target`: ширина по пропорции, не больше `4 × target` (`MAX_ASPECT`).
/// Возвращает (ширина, высота). Тот же расчёт в `resolveAnimated` и `targetSize`.
pub fn target_size(wide: u32, tall: u32, target: u32) -> (u32, u32) {
    let (wide, tall, target) = (f64::from(wide), f64::from(tall), f64::from(target));
    let mut out_tall = target;
    let mut out_wide = math_round(wide * target / tall).max(1.0);
    let max_wide = target * f64::from(constants::MAX_ASPECT);
    if out_wide > max_wide {
        out_tall = math_round(tall * max_wide / wide).max(1.0);
        out_wide = math_round(max_wide);
    }
    (out_wide as u32, out_tall as u32)
}

/// `decodePng(buffer, px)`: PNG или JPEG → RGBA высотой `px` (ширина по пропорции, не больше
/// `4 × px`); `None`, если картинка не разобралась.
pub fn decode_png(buffer: &[u8], px: u32) -> Option<DecodedPng> {
    let image = native_image::create_from_buffer(buffer)?;
    if image.wide == 0 || image.tall == 0 {
        return None;
    }
    let (target_w, target_h) = target_size(image.wide, image.tall, px);
    if target_w == image.wide && target_h == image.tall {
        return Some(DecodedPng {
            wide: image.wide,
            tall: image.tall,
            rgba: image.rgba,
        });
    }
    let rgba = native_image::resize_good(
        &image.rgba,
        image.wide as usize,
        image.tall as usize,
        target_w as usize,
        target_h as usize,
    );
    Some(DecodedPng {
        wide: target_w,
        tall: target_h,
        rgba,
    })
}

/// `resizeRgba(rgba, wide, tall, outWide, outTall)`: те же размеры — вход как есть; иначе
/// масштаб «good»; `None`, если буфер не по размеру.
pub fn resize_rgba(
    rgba: &[u8],
    wide: u32,
    tall: u32,
    out_wide: u32,
    out_tall: u32,
) -> Option<Vec<u8>> {
    if wide == out_wide && tall == out_tall {
        return Some(rgba.to_vec());
    }
    if wide == 0 || tall == 0 || out_wide == 0 || out_tall == 0 {
        return None;
    }
    if rgba.len() != wide as usize * tall as usize * 4 {
        return None;
    }
    Some(native_image::resize_good(
        rgba,
        wide as usize,
        tall as usize,
        out_wide as usize,
        out_tall as usize,
    ))
}

/// Анимация, готовая к отправке игре.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Animation {
    pub wide: u32,
    pub tall: u32,
    /// Кадры в base64.
    pub frames: Vec<String>,
    pub frame_ms: u32,
}

/// Общая часть `resolveAnimated` 7TV и `buildAnimated` Twitch: размер, выборка кадров (не больше
/// 16 и не шире листа в 2048 px), масштаб каждого. `None` — кадров меньше двух или масштаб не
/// удался.
pub fn build_animation(decoded: &DecodedGif, target: u32) -> Option<Animation> {
    let (out_wide, out_tall) = target_size(decoded.wide, decoded.tall, target);
    let by_width = (constants::MAX_SHEET_WIDE / out_wide).max(2) as usize;
    let sampled = sample_frames(decoded, constants::MAX_ANIM_FRAMES.min(by_width));
    if sampled.picks.len() < 2 {
        return None;
    }
    let mut frames = Vec::with_capacity(sampled.picks.len());
    for &pick in &sampled.picks {
        let scaled = resize_rgba(
            &decoded.frames[pick].rgba,
            decoded.wide,
            decoded.tall,
            out_wide,
            out_tall,
        )?;
        frames.push(STANDARD.encode(&scaled));
    }
    Some(Animation {
        wide: out_wide,
        tall: out_tall,
        frames,
        frame_ms: sampled.interval_ms,
    })
}

/// Ответ игре по итогу `resolve` (общий у 7TV и Twitch): промах, неподвижный эмоут или анимация.
pub(crate) fn send_resolved(bridge: &dyn ChatBridge, name: &str, image: Option<&Resolved>) {
    match image {
        None => {
            bridge.send_emote_miss(name);
        }
        Some(Resolved::Animated(animated)) => {
            bridge.send_emote_begin(
                name,
                animated.wide,
                animated.tall,
                animated.frames.len() as u32,
                animated.frame_ms,
            );
            for (index, frame) in animated.frames.iter().enumerate() {
                bridge.send_emote_frame(name, index as u32, frame);
            }
        }
        Some(Resolved::Still(still)) => {
            bridge.send_emote_ready(name, still.wide, still.tall, &still.rgba_base64);
        }
    }
}

/// `SevenTvBridge`: потребитель чата 7TV.
pub struct SevenTvBridge {
    log: Arc<dyn Logger>,
    handle: Handle,
    service: Arc<SevenTvService>,
    bridge: Arc<Mutex<Option<Arc<dyn ChatBridge>>>>,
}

impl SevenTvBridge {
    pub fn new(http: ChatHttp, log: Arc<dyn Logger>, handle: Handle) -> SevenTvBridge {
        SevenTvBridge {
            service: SevenTvService::new(http, log.clone(), handle.clone()),
            log,
            handle,
            bridge: Arc::default(),
        }
    }

    /// `diagnostics()`.
    pub fn diagnostics(&self) -> Diagnostics {
        self.service.diagnostics()
    }
}

impl ChatConsumer for SevenTvBridge {
    fn attach_bridge(&self, bridge: Arc<dyn ChatBridge>) {
        *self.bridge.lock().unwrap_or_else(|p| p.into_inner()) = Some(bridge);
    }

    fn handle_message(&self, message: &ChatMessage) {
        match message {
            ChatMessage::EmoteSet { room_id, .. } => {
                let promise = self.service.set_channel(Some(room_id));
                // Исход набора игре не нужен: промах придёт на `emote.resolve`.
                self.handle.spawn(async move {
                    promise.wait().await;
                });
            }
            ChatMessage::EmoteResolve { name, px, .. } => {
                if name.starts_with("tw:") {
                    return;
                }
                let (name, px) = (name.clone(), *px);
                let service = self.service.clone();
                let bridge = self.bridge.clone();
                let log = self.log.clone();
                self.handle.spawn(async move {
                    match service.resolve(&name, px).await {
                        Ok(image) => {
                            let current = bridge.lock().unwrap_or_else(|p| p.into_inner()).clone();
                            if let Some(current) = current {
                                send_resolved(current.as_ref(), &name, image.as_ref());
                            }
                        }
                        Err(message) => log.warn(&format!(
                            "{} RESOLVE_FAILED: {message}",
                            constants::LOG_PREFIX
                        )),
                    }
                });
            }
            ChatMessage::Subscribe { .. } | ChatMessage::Unsubscribe { .. } => {}
        }
    }

    fn forget(&self) {
        self.service.forget();
    }
}
