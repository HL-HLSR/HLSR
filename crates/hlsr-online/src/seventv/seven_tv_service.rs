//! `seventv/SevenTvService.js`: набор эмоутов канала 7TV (`twitch:<room>`, `set:<id>`,
//! `user:<id>`), `resolve(name, px)` → анимация (≥ 2 кадров, ≤ 16, лист ≤ 2048 px) или PNG, кеш
//! на 256 записей.
//!
//! Устройство 1:1 с JS, включая гонки: `emote.set` при загрузке другого набора не отменяет её —
//! запоздавшая загрузка дописывает эмоуты в общую таблицу и взводит `setLoaded`. Обещание набора
//! (`setPromise`) — [`SetPromise`]: его ждёт `resolve`. Состояние — под мьютексом, который не
//! держится через ожидание.
//!
//! Отступление: `connections` не массивом (а, например, объектом) в 0.4.7 даёт `TypeError` на
//! `.find`, и набор остаётся «отвергнутым» навсегда; здесь это «не привязан» (`SET_NOT_LINKED`).

use super::constants as c;
use super::gif::decode_gif;
use super::{build_animation, decode_png, get};
use crate::chat_http::ChatHttp;
use crate::fifo_map::FifoMap;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hlsr_core::js::{to_string, to_template_string, truthy};
use hlsr_core::json_text::json_parse;
use hlsr_core::logger::Logger;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::runtime::Handle;
use tokio::sync::watch;

/// Неподвижный эмоут: `{wide, tall, rgbaBase64}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StillImage {
    pub wide: u32,
    pub tall: u32,
    pub rgba_base64: String,
}

/// Анимированный эмоут: `{wide, tall, frames, frameMs}`, кадры — base64.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimatedImage {
    pub wide: u32,
    pub tall: u32,
    pub frames: Vec<String>,
    pub frame_ms: u32,
}

/// Итог разбора GIF в рабочем потоке.
enum Built {
    DecodeNull,
    NotAnimated,
    Animated(AnimatedImage, usize),
}

/// Результат `resolve`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    Still(StillImage),
    Animated(AnimatedImage),
}

/// Обещание загрузки набора: ждёт, пока задача загрузки не сообщит итог.
#[derive(Clone)]
pub struct SetPromise(watch::Receiver<Option<bool>>);

impl SetPromise {
    /// `Promise.resolve(value)`.
    fn resolved(value: bool) -> SetPromise {
        SetPromise(watch::channel(Some(value)).1)
    }

    /// `await promise`: `true` — набор загружен.
    pub async fn wait(&self) -> bool {
        let mut receiver = self.0.clone();
        match receiver.wait_for(|value| value.is_some()).await {
            Ok(value) => (*value).unwrap_or(false),
            Err(_) => false,
        }
    }
}

/// Источник набора (`parseSource`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum SourceKind {
    Set,
    User,
    Twitch,
}

struct Source {
    kind: SourceKind,
    url: String,
}

/// `parseSource(source)`: `kind:id` или голый идентификатор канала Twitch.
fn parse_source(source: &str) -> Option<Source> {
    let colon = source.find(':').filter(|&at| at > 0);
    let (kind, id) = match colon {
        Some(at) => (&source[..at], &source[at + 1..]),
        None => ("twitch", source),
    };
    if id.is_empty() {
        return None;
    }
    match kind {
        "set" => Some(Source {
            kind: SourceKind::Set,
            url: c::emote_set_by_id_url(id),
        }),
        "user" => Some(Source {
            kind: SourceKind::User,
            url: c::seven_tv_user_url(id),
        }),
        "twitch" => Some(Source {
            kind: SourceKind::Twitch,
            url: c::emote_set_url(id),
        }),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct Entry {
    id: String,
    animated: bool,
}

#[derive(Default)]
struct State {
    room_id: Option<String>,
    by_name: HashMap<String, Entry>,
    cache: FifoMap<Option<StillImage>>,
    set_loaded: bool,
    set_promise: Option<SetPromise>,
}

/// Снимок для диагностики: `{roomId, setLoaded, emotes, cached}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostics {
    pub room_id: Option<String>,
    pub set_loaded: bool,
    pub emotes: usize,
    pub cached: usize,
}

impl Diagnostics {
    pub fn to_json(&self) -> Value {
        json!({
            "roomId": self.room_id,
            "setLoaded": self.set_loaded,
            "emotes": self.emotes,
            "cached": self.cached,
        })
    }
}

pub struct SevenTvService {
    http: ChatHttp,
    log: Arc<dyn Logger>,
    handle: Handle,
    state: Mutex<State>,
}

impl SevenTvService {
    pub fn new(http: ChatHttp, log: Arc<dyn Logger>, handle: Handle) -> Arc<SevenTvService> {
        Arc::new(SevenTvService {
            http,
            log,
            handle,
            state: Mutex::default(),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `setChannel(source)`: загрузка набора в фоне; пустой источник сбрасывает канал.
    pub fn set_channel(self: &Arc<Self>, source: Option<&str>) -> SetPromise {
        let room_id = source.filter(|s| !s.is_empty()).map(str::to_owned);

        let mut state = self.state();
        if state.room_id == room_id && (state.set_loaded || state.set_promise.is_some()) {
            return state
                .set_promise
                .clone()
                .unwrap_or_else(|| SetPromise::resolved(true));
        }

        state.room_id = room_id.clone();
        state.by_name.clear();
        state.cache.clear();
        state.set_loaded = false;

        let Some(room_id) = room_id else {
            state.set_promise = None;
            return SetPromise::resolved(false);
        };

        // Синхронное начало `loadSet`: непонятный источник забывает канал до присвоения обещания.
        let Some(source) = parse_source(&room_id) else {
            state.room_id = None;
            state.set_loaded = false;
            let promise = SetPromise::resolved(false);
            state.set_promise = Some(promise.clone());
            return promise;
        };

        let (sender, receiver) = watch::channel(None);
        let promise = SetPromise(receiver);
        state.set_promise = Some(promise.clone());
        drop(state);

        let this = self.clone();
        self.handle.spawn(async move {
            let loaded = this.load_set(&room_id, source).await;
            sender.send_replace(Some(loaded));
        });
        promise
    }

    /// `fetchJson(url, what)`: тело набора как JSON или `None` (с записью в журнал).
    async fn fetch_json(&self, url: &str, what: &str) -> Option<Value> {
        let body = get(
            &self.http,
            url,
            Some(c::MAX_SET_BYTES),
            Some("application/json"),
            c::REQUEST_TIMEOUT_MS,
        )
        .await;
        let Some(body) = body else {
            self.log
                .warn(&format!("{} SET_FETCH_FAILED: {what}", c::LOG_PREFIX));
            return None;
        };
        match json_parse(&String::from_utf8_lossy(&body)) {
            Ok(value) => Some(value),
            Err(_) => {
                self.log
                    .warn(&format!("{} SET_NOT_JSON: {what}", c::LOG_PREFIX));
                None
            }
        }
    }

    /// `emotesFromJson(json)`.
    fn emotes_from_json(json: &Value) -> Option<&Vec<Value>> {
        if !truthy(json) {
            return None;
        }
        if let Some(list) = json.get("emotes").and_then(Value::as_array) {
            return Some(list);
        }
        json.get("emote_set")
            .filter(|set| truthy(set))
            .and_then(|set| set.get("emotes"))
            .and_then(Value::as_array)
    }

    /// `loadSet(roomId)`.
    async fn load_set(self: &Arc<Self>, room_id: &str, source: Source) -> bool {
        let Some(mut json) = self.fetch_json(&source.url, room_id).await.filter(truthy) else {
            self.forget_channel();
            return false;
        };

        if source.kind == SourceKind::User {
            let link = json
                .get("connections")
                .and_then(Value::as_array)
                .and_then(|list| {
                    list.iter()
                        .find(|c| truthy(c) && c.get("emote_set_id").is_some_and(truthy))
                })
                .and_then(|c| c.get("emote_set_id"))
                .cloned();
            let Some(link) = link else {
                self.log
                    .warn(&format!("{} SET_NOT_LINKED: {room_id}", c::LOG_PREFIX));
                self.forget_channel();
                return false;
            };
            let what = to_template_string(Some(&link));
            let Some(next) = self
                .fetch_json(&c::emote_set_by_id_url(&to_string(&link)), &what)
                .await
                .filter(truthy)
            else {
                self.forget_channel();
                return false;
            };
            json = next;
        }

        let empty = Vec::new();
        let emotes = Self::emotes_from_json(&json).unwrap_or(&empty);

        let mut state = self.state();
        for emote in emotes {
            if !truthy(emote) {
                continue;
            }
            let (Some(name), Some(id)) = (
                emote.get("name").and_then(Value::as_str),
                emote.get("id").and_then(Value::as_str),
            ) else {
                continue;
            };
            let animated = emote
                .get("data")
                .filter(|data| truthy(data))
                .and_then(|data| data.get("animated"))
                .is_some_and(truthy);
            state.by_name.insert(
                name.to_owned(),
                Entry {
                    id: id.to_owned(),
                    animated,
                },
            );
        }

        state.set_loaded = true;
        let count = state.by_name.len();
        drop(state);
        self.log.info(&format!(
            "{} SET_LOADED: {room_id}, эмоутов {count}",
            c::LOG_PREFIX
        ));
        true
    }

    /// `forget()`: кеш и набор канала очищены.
    pub fn forget(&self) {
        let had;
        {
            let mut state = self.state();
            had = state.cache.len() + state.by_name.len();
            state.cache.clear();
            state.by_name.clear();
            Self::forget_channel_locked(&mut state);
        }
        if had > 0 {
            self.log.info(&format!(
                "{} FORGOTTEN: кэш и набор канала очищены",
                c::LOG_PREFIX
            ));
        }
    }

    fn forget_channel(&self) {
        Self::forget_channel_locked(&mut self.state());
    }

    fn forget_channel_locked(state: &mut State) {
        state.room_id = None;
        state.set_promise = None;
        state.set_loaded = false;
    }

    /// `cacheKey(name, px)`.
    fn cache_key(name: &str, px: Option<u32>) -> String {
        match px.filter(|&px| px > 0) {
            Some(px) => format!("{name}@{px}"),
            None => name.to_owned(),
        }
    }

    fn remember(&self, key: &str, value: Option<StillImage>) {
        self.state()
            .cache
            .remember(key, value, c::MAX_CACHED_IMAGES);
    }

    /// `resolve(name, px)`: эмоут или `None` (промах). `Err` — исключение, которое в 0.4.7
    /// выбросил бы `decodeGif` на обрезанном заголовке кадра (текст — как у Node).
    pub async fn resolve(
        self: &Arc<Self>,
        name: &str,
        px: Option<u32>,
    ) -> Result<Option<Resolved>, String> {
        let promise = self.state().set_promise.clone();
        if let Some(promise) = promise {
            promise.wait().await;
        }

        let key = Self::cache_key(name, px);
        let entry = {
            let state = self.state();
            if !state.set_loaded {
                return Ok(None);
            }
            if let Some(hit) = state.cache.get(&key) {
                return Ok(hit.clone().map(Resolved::Still));
            }
            state.by_name.get(name).cloned()
        };
        let Some(entry) = entry else {
            self.remember(&key, None);
            return Ok(None);
        };

        if entry.animated
            && let Some(animated) = self.resolve_animated(name, &entry, px).await?
        {
            return Ok(Some(Resolved::Animated(animated)));
        }

        let png = get(
            &self.http,
            &c::emote_image_url(&entry.id, entry.animated, px),
            Some(c::MAX_IMAGE_BYTES),
            Some("image/png"),
            c::REQUEST_TIMEOUT_MS,
        )
        .await;
        let Some(png) = png else {
            self.log
                .warn(&format!("{} IMAGE_FETCH_FAILED: {name}", c::LOG_PREFIX));
            return Ok(None);
        };

        let target = px.filter(|&px| px > 0).unwrap_or(c::EMOTE_PX);
        let decoded = self
            .handle
            .spawn_blocking(move || decode_png(&png, target))
            .await
            .ok()
            .flatten();
        let Some(decoded) = decoded else {
            self.log
                .warn(&format!("{} IMAGE_DECODE_FAILED: {name}", c::LOG_PREFIX));
            return Ok(None);
        };

        let image = StillImage {
            wide: decoded.wide,
            tall: decoded.tall,
            rgba_base64: STANDARD.encode(&decoded.rgba),
        };
        self.remember(&key, Some(image.clone()));
        Ok(Some(Resolved::Still(image)))
    }

    /// `resolveAnimated(name, entry, px)`.
    async fn resolve_animated(
        &self,
        name: &str,
        entry: &Entry,
        px: Option<u32>,
    ) -> Result<Option<AnimatedImage>, String> {
        let gif = get(
            &self.http,
            &c::emote_gif_url(&entry.id, px),
            Some(c::MAX_GIF_BYTES),
            Some("image/gif"),
            c::REQUEST_TIMEOUT_MS,
        )
        .await;
        let Some(gif) = gif else {
            return Ok(None);
        };

        let target = px.filter(|&px| px > 0).unwrap_or(c::EMOTE_PX);
        let built = self
            .handle
            .spawn_blocking(move || -> Result<Built, String> {
                let decoded = match decode_gif(&gif) {
                    Err(error) => return Err(error.to_string()),
                    Ok(None) => return Ok(Built::DecodeNull),
                    Ok(Some(decoded)) => decoded,
                };
                let total = decoded.frames.len();
                Ok(match build_animation(&decoded, target) {
                    None => Built::NotAnimated,
                    Some(animation) => Built::Animated(
                        AnimatedImage {
                            wide: animation.wide,
                            tall: animation.tall,
                            frames: animation.frames,
                            frame_ms: animation.frame_ms,
                        },
                        total,
                    ),
                })
            })
            .await
            .map_err(|error| error.to_string())??;

        match built {
            Built::DecodeNull => {
                self.log
                    .warn(&format!("{} GIF_DECODE_FAILED: {name}", c::LOG_PREFIX));
                Ok(None)
            }
            // Меньше двух кадров или масштаб не удался: 0.4.7 молча возвращает `null`.
            Built::NotAnimated => Ok(None),
            Built::Animated(image, total) => {
                self.log.info(&format!(
                    "{} ANIMATED: {name} {}x{}, {} из {total} кадров, {}мс",
                    c::LOG_PREFIX,
                    image.wide,
                    image.tall,
                    image.frames.len(),
                    image.frame_ms
                ));
                Ok(Some(image))
            }
        }
    }

    /// `diagnostics()`.
    pub fn diagnostics(&self) -> Diagnostics {
        let state = self.state();
        Diagnostics {
            room_id: state.room_id.clone(),
            set_loaded: state.set_loaded,
            emotes: state.by_name.len(),
            cached: state.cache.len(),
        }
    }
}
