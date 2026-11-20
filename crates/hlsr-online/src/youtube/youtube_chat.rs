//! `youtube/YouTubeChat.js`: разбор страницы чата (`INNERTUBE_API_KEY`, `clientVersion`,
//! `continuation`), опрос `get_live_chat`, `readMessage` (текст, эмодзи, роли), `readContinuation`,
//! анти-дубли (512 → 256 последних). Опрос идёт раз в секунду: продолжение просит не больше
//! `POLL_CAP_MS`.
//!
//! Устройство 1:1 с JS: `stop` не увеличивает `generation` (его меняет `start`), поэтому остановка
//! во время загрузки страницы не отменяет запуск; ответ опроса, пришедший после остановки или
//! нового запуска, молча отбрасывается; незнакомый значок пишется в журнал один раз на процесс.
//! Таймер опроса — задача tokio; `delay_divisor` сокращает паузы в тестах с настоящей сетью.

use super::constants as c;
use crate::fifo_map::InsertionSet;
use hlsr_core::js::{slice_utf16, to_number, to_template_string, trim, truthy};
use hlsr_core::logger::Logger;
use hlsr_delivery::providers::BoxFuture;
use regex::Regex;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

/// Роли автора сообщения (`readAuthorRoles`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Roles {
    pub owner: bool,
    pub moderator: bool,
    pub verified: bool,
    pub member: bool,
    /// Адрес значка участника: последняя картинка последнего значка.
    pub member_url: Option<String>,
}

impl Roles {
    pub fn to_json(&self) -> Value {
        json!({
            "owner": self.owner,
            "moderator": self.moderator,
            "verified": self.verified,
            "member": self.member,
            "memberUrl": self.member_url,
        })
    }
}

/// Эмодзи в тексте: сокращение (`:smile:`) и адрес картинки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Emoji {
    pub shortcut: String,
    pub url: String,
}

/// Сообщение чата: `{id, nick, text, emoji, roles}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: Option<String>,
    pub nick: String,
    pub text: String,
    pub emoji: Vec<Emoji>,
    pub roles: Roles,
}

impl Message {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "nick": self.nick,
            "text": self.text,
            "emoji": self.emoji.iter().map(|e| json!({"shortcut": e.shortcut, "url": e.url})).collect::<Vec<_>>(),
            "roles": self.roles.to_json(),
        })
    }
}

/// Продолжение опроса: токен и подсказка `timeoutMs` (любое значение JSON).
#[derive(Debug, Clone, PartialEq)]
pub struct Continuation {
    pub token: String,
    pub timeout_ms: Option<Value>,
}

impl Continuation {
    pub fn to_json(&self) -> Value {
        let mut out = json!({"token": self.token});
        if let Some(timeout) = &self.timeout_ms {
            out["timeoutMs"] = timeout.clone();
        }
        out
    }
}

/// `readContinuation(node)`.
pub fn read_continuation(node: Option<&Value>) -> Option<Continuation> {
    let node = node.filter(|node| truthy(node))?;
    let list = node
        .get("continuations")?
        .as_array()
        .filter(|l| !l.is_empty())?;
    let first = &list[0];
    let data = [
        "invalidationContinuationData",
        "timedContinuationData",
        "reloadContinuationData",
        "liveChatReplayContinuationData",
    ]
    .iter()
    .filter_map(|key| first.get(key))
    .find(|data| truthy(data))?;
    let token = data.get("continuation")?.as_str()?;
    Some(Continuation {
        token: token.to_owned(),
        timeout_ms: data.get("timeoutMs").cloned(),
    })
}

/// Незнакомые значки, о которых уже написано (`seenUnknownBadge`): один набор на процесс.
static SEEN_UNKNOWN_BADGE: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

fn last_thumbnail_url(thumbnails: Option<&Value>) -> Option<String> {
    let list = thumbnails?.as_array()?;
    list.last()?.get("url")?.as_str().map(str::to_owned)
}

/// `readAuthorRoles(m, log)`.
fn read_author_roles(message: &Value, log: &dyn Logger) -> Roles {
    let mut roles = Roles::default();
    let badges = message
        .get("authorBadges")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for badge in badges {
        let Some(renderer) = badge
            .get("liveChatAuthorBadgeRenderer")
            .filter(|renderer| truthy(renderer))
        else {
            continue;
        };
        let icon_type = renderer
            .get("icon")
            .filter(|icon| truthy(icon))
            .and_then(|icon| icon.get("iconType"))
            .and_then(Value::as_str);
        match icon_type {
            Some("OWNER") => roles.owner = true,
            Some("MODERATOR") => roles.moderator = true,
            Some("VERIFIED") => roles.verified = true,
            _ if renderer.get("customThumbnail").is_some_and(truthy) => {
                roles.member = true;
                let thumbnails = renderer
                    .get("customThumbnail")
                    .and_then(|custom| custom.get("thumbnails"));
                if let Some(url) = last_thumbnail_url(thumbnails) {
                    roles.member_url = Some(url);
                }
            }
            _ => {
                let fields: Vec<&str> = renderer
                    .as_object()
                    .map(|object| object.keys().map(String::as_str).collect())
                    .unwrap_or_default();
                let key = format!("{}:{}", icon_type.unwrap_or("?"), fields.join(","));
                let first_time = SEEN_UNKNOWN_BADGE
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(key.clone());
                if first_time {
                    let tooltip = renderer
                        .get("tooltip")
                        .filter(|tooltip| truthy(tooltip))
                        .map(|tooltip| to_template_string(Some(tooltip)))
                        .unwrap_or_else(|| "нет".to_owned());
                    log.warn(&format!(
                        "[youtube] незнакомый значок: iconType={} tooltip={tooltip} поля={key}",
                        icon_type.unwrap_or("нет")
                    ));
                }
            }
        }
    }
    roles
}

/// `.replace(/ {2,}/g, " ")`.
fn collapse_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = 0;
    for ch in text.chars() {
        if ch == ' ' {
            run += 1;
            if run == 1 {
                out.push(' ');
            }
        } else {
            run = 0;
            out.push(ch);
        }
    }
    out
}

/// `readMessage(action, log)`: текстовое сообщение чата или `None`.
pub fn read_message(action: &Value, log: &dyn Logger) -> Option<Message> {
    let renderer = action
        .get("addChatItemAction")?
        .get("item")?
        .get("liveChatTextMessageRenderer")
        .filter(|renderer| truthy(renderer))?;

    let id = renderer
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let nick = renderer
        .get("authorName")
        .and_then(|name| name.get("simpleText"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let runs = renderer
        .get("message")
        .and_then(|message| message.get("runs"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    let mut emoji = Vec::new();
    let mut joined = String::new();
    for run in runs {
        if let Some(text) = run.get("text").and_then(Value::as_str) {
            joined.push_str(text);
            continue;
        }
        let Some(emoji_node) = run.get("emoji").filter(|node| truthy(node)) else {
            continue;
        };
        let shortcut = emoji_node
            .get("shortcuts")
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .and_then(Value::as_str)
            .unwrap_or("");
        if shortcut.is_empty() {
            continue;
        }
        let thumbnails = emoji_node
            .get("image")
            .filter(|image| truthy(image))
            .and_then(|image| image.get("thumbnails"));
        if let Some(url) = last_thumbnail_url(thumbnails) {
            emoji.push(Emoji {
                shortcut: shortcut.to_owned(),
                url,
            });
        }
        joined.push(' ');
        joined.push_str(shortcut);
        joined.push(' ');
    }
    let text = trim(&collapse_spaces(&joined)).to_owned();

    if nick.is_empty() || text.is_empty() {
        return None;
    }
    Some(Message {
        id,
        nick,
        text,
        emoji,
        roles: read_author_roles(renderer, log),
    })
}

/// Сетевые вызовы опроса: `fetchText` и `postJson` 0.4.7.
pub trait YouTubeFetch: Send + Sync {
    /// Текст страницы; `None` — не получилась.
    fn fetch_text<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Option<String>>;
    /// Ответ на POST с JSON-телом; `None` — сбой или не JSON.
    fn post_json<'a>(&'a self, url: &'a str, body: &'a Value) -> BoxFuture<'a, Option<Value>>;
}

/// Приёмник строк чата: `onLine(nick, text, emoji, roles)`.
pub type LineSink = Arc<dyn Fn(&Message) + Send + Sync>;

static API_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""INNERTUBE_API_KEY":"([^"]+)""#).expect("шаблон верен"));
static CLIENT_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""clientVersion":"([0-9.]+)""#).expect("шаблон верен"));
static CONTINUATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""continuation":"([^"]+)""#).expect("шаблон верен"));

/// `pick(html, re)`.
fn pick(html: &str, pattern: &Regex) -> Option<String> {
    pattern.captures(html).map(|found| found[1].to_owned())
}

#[derive(Default)]
struct State {
    video_id: Option<String>,
    api_key: Option<String>,
    client_version: Option<String>,
    token: Option<String>,
    timer: Option<JoinHandle<()>>,
    running: bool,
    failures: u32,
    generation: u64,
    seen: InsertionSet,
    on_line: Option<LineSink>,
    /// Для сверки: пауза ждущего таймера опроса, мс (до сокращения `delay_divisor`).
    pending_wait: Option<u64>,
    /// Для сверки: паузы всех поставленных таймеров с прошлого `take_scheduled_waits`.
    scheduled: Vec<u64>,
}

/// Снимок для диагностики: `{videoId, running, failures, seen}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostics {
    pub video_id: Option<String>,
    pub running: bool,
    pub failures: u32,
    pub seen: usize,
}

impl Diagnostics {
    pub fn to_json(&self) -> Value {
        json!({
            "videoId": self.video_id,
            "running": self.running,
            "failures": self.failures,
            "seen": self.seen,
        })
    }
}

pub struct YouTubeChat {
    fetch: Arc<dyn YouTubeFetch>,
    log: Arc<dyn Logger>,
    handle: Handle,
    delay_divisor: u32,
    state: Mutex<State>,
}

impl YouTubeChat {
    /// `delay_divisor` — во сколько раз сократить паузы опроса (в выпуске — 1).
    pub fn new(
        fetch: Arc<dyn YouTubeFetch>,
        log: Arc<dyn Logger>,
        handle: Handle,
        delay_divisor: u32,
    ) -> Arc<YouTubeChat> {
        Arc::new(YouTubeChat {
            fetch,
            log,
            handle,
            delay_divisor: delay_divisor.max(1),
            state: Mutex::default(),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `start(videoIdOrUrl, onLine)`: `false` — неверный адрес, страница не открылась или запуск
    /// вытеснен другим. `input` `None` — как `null` в 0.4.7.
    pub async fn start(self: &Arc<Self>, input: Option<&str>, on_line: LineSink) -> bool {
        let Some(video_id) = c::parse_video_id(input.unwrap_or("")) else {
            self.log.warn(&format!(
                "{} BAD_VIDEO_ID: {}",
                c::LOG_PREFIX,
                slice_utf16(input.unwrap_or("null"), 60)
            ));
            return false;
        };

        self.stop();
        let generation = {
            let mut state = self.state();
            state.generation += 1;
            state.video_id = Some(video_id.clone());
            state.on_line = Some(on_line);
            state.running = true;
            state.failures = 0;
            state.seen.clear();
            state.generation
        };

        let opened = self.open_chat().await;

        if generation != self.state().generation {
            return false;
        }
        if !opened {
            self.stop();
            return false;
        }

        self.log
            .info(&format!("{} WATCHING: {video_id}", c::LOG_PREFIX));
        self.schedule(c::MIN_POLL_MS);
        true
    }

    /// `stop()`.
    pub fn stop(&self) {
        let (was_running, video_id) = {
            let mut state = self.state();
            if let Some(timer) = state.timer.take() {
                timer.abort();
            }
            state.pending_wait = None;
            let was_running = state.running;
            let video_id = state.video_id.take();
            state.running = false;
            state.api_key = None;
            state.client_version = None;
            state.token = None;
            state.on_line = None;
            state.seen.clear();
            (was_running, video_id)
        };
        if was_running {
            self.log.info(&format!(
                "{} STOPPED: {}",
                c::LOG_PREFIX,
                video_id.as_deref().unwrap_or("null")
            ));
        }
    }

    pub fn is_running(&self) -> bool {
        self.state().running
    }

    /// Для сверки с 0.4.7: пауза ждущего таймера опроса (мс) или `None`.
    pub fn pending_wait_ms(&self) -> Option<u64> {
        self.state().pending_wait
    }

    /// Для сверки с 0.4.7: паузы таймеров, поставленных с прошлого вызова.
    pub fn take_scheduled_waits(&self) -> Vec<u64> {
        std::mem::take(&mut self.state().scheduled)
    }

    /// `openChat()`.
    async fn open_chat(&self) -> bool {
        let video_id = self.state().video_id.clone().unwrap_or_default();
        let html = self.fetch.fetch_text(&c::live_chat_url(&video_id)).await;
        let current = || {
            self.state()
                .video_id
                .clone()
                .unwrap_or_else(|| "null".to_owned())
        };
        let Some(html) = html.filter(|html| !html.is_empty()) else {
            self.log
                .warn(&format!("{} PAGE_FAILED: {}", c::LOG_PREFIX, current()));
            return false;
        };

        if html.encode_utf16().count() < 4096 && !html.contains("INNERTUBE_API_KEY") {
            self.log.warn(&format!(
                "{} PAGE_STUB: YouTube отдал заглушку вместо чата",
                c::LOG_PREFIX
            ));
            return false;
        }

        let api_key = pick(&html, &API_KEY);
        let client_version = pick(&html, &CLIENT_VERSION);
        let token = pick(&html, &CONTINUATION);
        {
            let mut state = self.state();
            state.api_key = api_key.clone();
            state.client_version = client_version;
        }

        let (Some(_), Some(token)) = (api_key, token) else {
            self.log
                .warn(&format!("{} NO_LIVE_CHAT: {}", c::LOG_PREFIX, current()));
            return false;
        };
        self.state().token = Some(token);
        true
    }

    /// `schedule(ms)`: следующий опрос не раньше `MIN_POLL_MS` и не позже `MAX_POLL_MS`.
    fn schedule(self: &Arc<Self>, ms: f64) {
        let mut state = self.state();
        if !state.running {
            return;
        }
        let ms = if ms.is_nan() || ms == 0.0 {
            c::DEFAULT_POLL_MS
        } else {
            ms
        };
        let wait = ms.clamp(c::MIN_POLL_MS, c::MAX_POLL_MS);
        state.pending_wait = Some(wait as u64);
        state.scheduled.push(wait as u64);
        let pause = Duration::from_millis(wait as u64) / self.delay_divisor;
        let this = self.clone();
        state.timer = Some(self.handle.spawn(async move {
            tokio::time::sleep(pause).await;
            // Таймер сработал: дальше его не остановить (как `this.timer = null` в JS).
            {
                let mut state = this.state();
                drop(state.timer.take());
                state.pending_wait = None;
            }
            this.poll().await;
        }));
    }

    /// `poll()`.
    async fn poll(self: &Arc<Self>) {
        let (url, body, generation) = {
            let state = self.state();
            let (Some(token), true) = (state.token.clone(), state.running) else {
                return;
            };
            let version = state
                .client_version
                .clone()
                .unwrap_or_else(|| "2.20240101.00.00".to_owned());
            let body = json!({
                "context": {"client": {"clientName": "WEB", "clientVersion": version}},
                "continuation": token,
            });
            (
                c::poll_url(state.api_key.as_deref().unwrap_or("")),
                body,
                state.generation,
            )
        };

        let answer = self.fetch.post_json(&url, &body).await;

        {
            let state = self.state();
            if !state.running || generation != state.generation {
                return;
            }
        }

        let Some(answer) = answer.filter(truthy) else {
            let failures = {
                let mut state = self.state();
                state.failures += 1;
                state.failures
            };
            if failures >= c::MAX_CONSECUTIVE_FAILURES {
                let video_id = self.state().video_id.clone().unwrap_or_default();
                self.log.warn(&format!(
                    "{} GIVING_UP: {video_id} ({failures} неудач подряд)",
                    c::LOG_PREFIX
                ));
                self.stop();
                return;
            }
            self.schedule(c::DEFAULT_POLL_MS);
            return;
        };
        self.state().failures = 0;

        let chat = answer
            .get("continuationContents")
            .and_then(|contents| contents.get("liveChatContinuation"))
            .filter(|chat| truthy(chat));
        let actions = chat
            .and_then(|chat| chat.get("actions"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();

        let mut delivered = 0;
        for action in actions {
            if delivered >= c::MAX_LINES_PER_POLL {
                break;
            }
            let Some(message) = read_message(action, self.log.as_ref()) else {
                continue;
            };
            if let Some(id) = message.id.as_ref().filter(|id| !id.is_empty()) {
                let mut state = self.state();
                if state.seen.contains(id) {
                    continue;
                }
                state.seen.insert(id);
                if state.seen.len() > 512 {
                    state.seen.keep_last(256);
                }
            }
            delivered += 1;
            let sink = self.state().on_line.clone();
            if let Some(sink) = sink {
                sink(&message);
            }
        }

        let Some(next) = read_continuation(chat) else {
            let video_id = self.state().video_id.clone().unwrap_or_default();
            self.log
                .info(&format!("{} ENDED: {video_id}", c::LOG_PREFIX));
            self.stop();
            return;
        };

        self.state().token = Some(next.token);
        self.schedule(poll_hint(next.timeout_ms.as_ref()));
    }

    /// `diagnostics()`.
    pub fn diagnostics(&self) -> Diagnostics {
        let state = self.state();
        Diagnostics {
            video_id: state.video_id.clone(),
            running: state.running,
            failures: state.failures,
            seen: state.seen.len(),
        }
    }
}

/// `Math.min(next.timeoutMs || DEFAULT_POLL_MS, POLL_CAP_MS)`; нечисло дальше `schedule` заменяет
/// паузой по умолчанию, поэтому итог всегда секунда.
fn poll_hint(timeout_ms: Option<&Value>) -> f64 {
    let value = match timeout_ms {
        Some(value) if truthy(value) => match value {
            Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
            Value::String(text) => to_number(text),
            Value::Bool(true) => 1.0,
            _ => f64::NAN,
        },
        _ => c::DEFAULT_POLL_MS,
    };
    value.min(c::POLL_CAP_MS)
}
