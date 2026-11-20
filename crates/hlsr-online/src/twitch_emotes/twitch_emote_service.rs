//! `twitch-emotes/TwitchEmoteService.js`: `tw:<id>` → картинка с CDN Twitch; сначала пробуется
//! GIF (анимация или первый кадр), потом PNG; кеш на 256 записей.
//!
//! Исправление ошибки 0.4.7 (карта §0.3, решение 6 спецификации ПП4): у однокадрового GIF
//! `buildStillFromGif` передавал в `resizeRgba` объект `{rgba, delayMs}` вместо буфера — игре
//! уходил промах, а при совпадении размеров строка «[object Object]» вместо пикселей. Здесь
//! берётся кадр, и игра получает картинку.

use super::constants as c;
use crate::chat_http::{ChatHttp, Fetch};
use crate::fifo_map::FifoMap;
use crate::seventv::gif::decode_gif;
use crate::seventv::{
    AnimatedImage, Resolved, StillImage, build_animation, decode_png, resize_rgba, target_size,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hlsr_core::logger::Logger;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::runtime::Handle;

fn allow_host(hostname: &str) -> bool {
    hostname == c::CDN_HOST
}

pub struct TwitchEmoteService {
    http: ChatHttp,
    log: Arc<dyn Logger>,
    handle: Handle,
    cache: Mutex<FifoMap<Resolved>>,
}

/// Итог разбора картинки в рабочем потоке.
enum Built {
    Animated(AnimatedImage, usize),
    StillFromGif(StillImage),
    /// GIF разобран, но неподвижной картинки не вышло: 0.4.7 молча возвращает `null`.
    GifNothing,
    Png(StillImage),
    PngFailed,
}

impl TwitchEmoteService {
    pub fn new(http: ChatHttp, log: Arc<dyn Logger>, handle: Handle) -> Arc<TwitchEmoteService> {
        Arc::new(TwitchEmoteService {
            http,
            log,
            handle,
            cache: Mutex::default(),
        })
    }

    fn cache(&self) -> MutexGuard<'_, FifoMap<Resolved>> {
        self.cache.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn remember(&self, key: &str, value: Resolved) {
        self.cache().remember(key, value, c::MAX_CACHED_IMAGES);
    }

    /// `resolve(id, px)`: `Err` — исключение `decodeGif` на обрезанном заголовке кадра.
    pub async fn resolve(&self, id: &str, px: Option<u32>) -> Result<Option<Resolved>, String> {
        let target = px.filter(|&px| px > 0).unwrap_or(c::EMOTE_PX);
        let key = format!("{id}@{target}");
        let hit = self.cache().get(&key).cloned();
        if let Some(hit) = hit {
            return Ok(Some(hit));
        }

        let buf = self
            .http
            .fetch(&Fetch {
                url: &c::emote_image_url(id, px),
                allow: allow_host,
                user_agent: "HLSR",
                accept: Some("image/*"),
                headers: &[],
                body: None,
                bare_host: false,
                max_bytes: c::MAX_IMAGE_BYTES,
                timeout: Duration::from_millis(c::REQUEST_TIMEOUT_MS),
            })
            .await;
        let Some(buf) = buf else {
            self.log
                .warn(&format!("{} FETCH_FAILED: {id}", c::LOG_PREFIX));
            return Ok(None);
        };

        let built = self
            .handle
            .spawn_blocking(move || -> Result<Built, String> {
                let gif = decode_gif(&buf).map_err(|error| error.to_string())?;
                if let Some(gif) = gif {
                    let total = gif.frames.len();
                    if let Some(animation) = build_animation(&gif, target) {
                        return Ok(Built::Animated(
                            AnimatedImage {
                                wide: animation.wide,
                                tall: animation.tall,
                                frames: animation.frames,
                                frame_ms: animation.frame_ms,
                            },
                            total,
                        ));
                    }
                    // `buildStillFromGif`: первый кадр (исправлено: буфер кадра, а не объект).
                    let (out_wide, out_tall) = target_size(gif.wide, gif.tall, target);
                    let scaled =
                        resize_rgba(&gif.frames[0].rgba, gif.wide, gif.tall, out_wide, out_tall);
                    return Ok(match scaled {
                        Some(rgba) => Built::StillFromGif(StillImage {
                            wide: out_wide,
                            tall: out_tall,
                            rgba_base64: STANDARD.encode(rgba),
                        }),
                        None => Built::GifNothing,
                    });
                }
                Ok(match decode_png(&buf, target) {
                    Some(decoded) => Built::Png(StillImage {
                        wide: decoded.wide,
                        tall: decoded.tall,
                        rgba_base64: STANDARD.encode(&decoded.rgba),
                    }),
                    None => Built::PngFailed,
                })
            })
            .await
            .map_err(|error| error.to_string())??;

        match built {
            Built::Animated(image, total) => {
                self.log.info(&format!(
                    "{} ANIMATED: {id} {}x{}, {} из {total} кадров, {}мс",
                    c::LOG_PREFIX,
                    image.wide,
                    image.tall,
                    image.frames.len(),
                    image.frame_ms
                ));
                let resolved = Resolved::Animated(image);
                self.remember(&key, resolved.clone());
                Ok(Some(resolved))
            }
            Built::StillFromGif(image) | Built::Png(image) => {
                let resolved = Resolved::Still(image);
                self.remember(&key, resolved.clone());
                Ok(Some(resolved))
            }
            Built::GifNothing => Ok(None),
            Built::PngFailed => {
                self.log
                    .warn(&format!("{} DECODE_FAILED: {id}", c::LOG_PREFIX));
                Ok(None)
            }
        }
    }

    /// `diagnostics()`: `{cached}`.
    pub fn diagnostics(&self) -> Value {
        json!({ "cached": self.cache().len() })
    }
}
