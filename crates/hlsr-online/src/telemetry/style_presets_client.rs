//! Заготовки «подсветка ника + рамка аватара» (`telemetry/style-presets-client.js` 0.4.9): из них
//! поддержавший проект выбирает свою в настройках профиля. Сервер отдаёт видимые заготовки по
//! порядку: `[{id, title, nickCss, frameCss}]`. Сервер на PHP заготовок не знает (404) — список
//! пуст, выбора нет.
//!
//! CSS проходит те же проверки, что оформление из профиля ([`nick_styles`], [`avatar_frames`]).
//! Заготовку, чей непустой CSS их не прошёл, лаунчер не показывает: предпросмотр разошёлся бы с
//! тем, что увидят другие игроки. Одновременные `get` делят один запрос.

use super::config;
use super::transport::{RequestOptions, Transport};
use crate::js_value;
use crate::shared::{avatar_frames, nick_styles};
use hlsr_core::js;
use hlsr_core::json_text::json_parse;
use hlsr_delivery::timing::Clock;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};

/// `CACHE_MS`: как `max-age` сервера (300 с).
pub const CACHE_MS: i64 = 5 * 60 * 1000;
const MAX_PRESETS: usize = 100;
/// `style_presets.title VARCHAR(48)`.
const TITLE_MAX: usize = 48;
/// `style_presets.id INT UNSIGNED`.
const ID_MAX: i64 = 4_294_967_295;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// `presetCss(raw, sanitize)`: проверенный CSS, `""` — эту часть заготовка не меняет, `None` —
/// заготовка не годится (CSS не строка или непустой CSS не прошёл проверку).
fn preset_css(raw: Option<&Value>, sanitize: fn(&Value) -> Option<String>) -> Option<String> {
    match raw {
        None | Some(Value::Null) => Some(String::new()),
        Some(Value::String(css)) if js::trim(css).is_empty() => Some(String::new()),
        Some(css @ Value::String(_)) => sanitize(css).filter(|css| !css.is_empty()),
        Some(_) => None,
    }
}

/// `parsePreset(raw)`: `{id, title, nick: {style: null, css}, frame: {css}}` — как `nick` и
/// `avatarFrame` профиля.
pub fn parse_preset(raw: &Value) -> Option<Value> {
    if !raw.is_object() {
        return None;
    }
    let raw = Some(raw);
    let id =
        js_value::safe_integer(js_value::field(raw, "id")).filter(|id| *id > 0 && *id <= ID_MAX)?;
    let title = js_value::field(raw, "title")
        .and_then(Value::as_str)
        .map_or("", js::trim);
    if title.is_empty()
        || title.chars().count() > TITLE_MAX
        || title.chars().any(|c| (c as u32) < 0x20 || c as u32 == 0x7f)
    {
        return None;
    }
    let nick = preset_css(js_value::field(raw, "nickCss"), nick_styles::sanitize_css)?;
    let frame = preset_css(
        js_value::field(raw, "frameCss"),
        avatar_frames::sanitize_css,
    )?;
    Some(
        json!({"id": id, "title": title, "nick": {"style": null, "css": nick}, "frame": {"css": frame}}),
    )
}

/// `parseList(raw)`: заготовки без повторов или `None`, если это не список.
pub fn parse_list(raw: &Value) -> Option<Vec<Value>> {
    let mut seen = HashSet::new();
    Some(
        raw.as_array()?
            .iter()
            .take(MAX_PRESETS)
            .filter_map(parse_preset)
            .filter(|preset| seen.insert(preset["id"].as_i64()))
            .collect(),
    )
}

/// Параметры [`StylePresetsClient::new`].
pub struct StylePresetsClientOptions {
    /// По умолчанию `https://api.hlsr.pro/v2/public/style-presets`.
    pub endpoint: Option<String>,
    /// По умолчанию `0.0.0`.
    pub launcher_version: Option<String>,
    pub clock: Clock,
    pub transport: Arc<dyn Transport>,
}

#[derive(Default)]
struct State {
    presets: Option<Arc<Vec<Value>>>,
    presets_at: i64,
    completed: u64,
    last: Option<Value>,
}

fn answer(ok: bool, presets: Option<&Arc<Vec<Value>>>) -> Value {
    json!({
        "status": if ok { "ok" } else { "error" },
        "presets": presets.map_or(&[][..], |list| list.as_slice()),
    })
}

pub struct StylePresetsClient {
    endpoint: String,
    launcher_version: String,
    clock: Clock,
    transport: Arc<dyn Transport>,
    state: Mutex<State>,
    fetching: tokio::sync::Mutex<()>,
}

impl StylePresetsClient {
    pub fn new(options: StylePresetsClientOptions) -> StylePresetsClient {
        StylePresetsClient {
            endpoint: options
                .endpoint
                .filter(|endpoint| !endpoint.is_empty())
                .unwrap_or_else(|| format!("{}/v2/public/style-presets", config::API_BASE)),
            launcher_version: options
                .launcher_version
                .filter(|version| !version.is_empty())
                .unwrap_or_else(|| "0.0.0".to_owned()),
            clock: options.clock,
            transport: options.transport,
            state: Mutex::default(),
            fetching: tokio::sync::Mutex::new(()),
        }
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    /// `get({force})`: `{status: "ok", presets}` (у старого сервера пусто) или
    /// `{status: "error", presets}` с последним полученным списком (или пустым).
    pub async fn get(&self, force: bool) -> Value {
        let observed = {
            let state = lock(&self.state);
            if !force
                && let Some(presets) = &state.presets
                && self.now() - state.presets_at < CACHE_MS
            {
                return answer(true, Some(presets));
            }
            state.completed
        };
        let _fetching = self.fetching.lock().await;
        {
            let state = lock(&self.state);
            if state.completed != observed
                && let Some(answer) = &state.last
            {
                return answer.clone();
            }
        }
        let loaded = self.load().await;
        let mut state = lock(&self.state);
        let reply = match loaded {
            Some(presets) => {
                let presets = Arc::new(presets);
                state.presets = Some(presets.clone());
                state.presets_at = self.now();
                answer(true, Some(&presets))
            }
            None => answer(false, state.presets.as_ref()),
        };
        state.completed += 1;
        state.last = Some(reply.clone());
        reply
    }

    /// Запрос и разбор; `None` — любой сбой. 404 — пустой список.
    async fn load(&self) -> Option<Vec<Value>> {
        let options = RequestOptions {
            method: Some("GET".to_owned()),
            timeout_ms: Some(config::online::TIMEOUT_MS),
            max_response_bytes: Some(MAX_RESPONSE_BYTES),
            launcher_version: Some(self.launcher_version.clone()),
            ..RequestOptions::default()
        };
        let response = self
            .transport
            .request(&self.endpoint, &options)
            .await
            .ok()?;
        if response.status == 404 {
            return Some(Vec::new());
        }
        if !(200..300).contains(&response.status) {
            return None;
        }
        parse_list(&json_parse(&response.body).ok()?)
    }
}
