//! `game-presence/protocol.js`: строки JSON между игрой и лаунчером. Версия 1, строка не длиннее
//! 2048 байт; от игры — `hello`, `state`, `chat.subscribe`, `chat.unsubscribe`, `emote.set`,
//! `emote.resolve`; к игре — `discord`, `chat.line`, `emote.begin`, `emote.frame`, `emote.ready`,
//! `emote.miss`, `badge.ready`. Лишнее поле, неверный тип и значение вне диапазона отклоняют
//! сообщение с причиной (`reason`), как в 0.4.9.

use super::endpoint_store::is_token;
use hlsr_core::js::slice_utf16;
use hlsr_core::json_text::{js_key_order, js_number, json_parse, stringify};
use hlsr_core::map_assets;
use hlsr_delivery::ports::ChatMessage;
use serde_json::{Map, Value, json};

pub const PROTOCOL_VERSION: u32 = 1;
/// Длина строки от игры, байт UTF-8.
pub const MAX_LINE_BYTES: usize = 2048;
pub const LIFECYCLES: [&str; 3] = ["menu", "playing", "exited"];
pub const PLATFORMS: [&str; 2] = ["twitch", "youtube"];
pub const EMOTE_PX_MIN: f64 = 8.0;
pub const EMOTE_PX_MAX: f64 = 128.0;

const HELLO_FIELDS: [&str; 4] = ["v", "type", "token", "session"];
const CHAT_SUBSCRIBE_FIELDS: [&str; 5] = ["v", "type", "session", "platform", "id"];
const CHAT_UNSUBSCRIBE_FIELDS: [&str; 3] = ["v", "type", "session"];
const EMOTE_SET_FIELDS: [&str; 4] = ["v", "type", "session", "roomId"];
const EMOTE_RESOLVE_FIELDS: [&str; 5] = ["v", "type", "session", "name", "px"];
const STATE_FIELDS: [&str; 10] = [
    "v",
    "type",
    "session",
    "seq",
    "lifecycle",
    "mapName",
    "fps",
    "timerRunning",
    "timerCompleted",
    "timerElapsedMilliseconds",
];

/// Этап жизни игры: меню, игра на карте, выход.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    Menu,
    Playing,
    Exited,
}

impl Lifecycle {
    pub fn as_str(self) -> &'static str {
        match self {
            Lifecycle::Menu => "menu",
            Lifecycle::Playing => "playing",
            Lifecycle::Exited => "exited",
        }
    }

    fn of(text: &str) -> Option<Lifecycle> {
        match text {
            "menu" => Some(Lifecycle::Menu),
            "playing" => Some(Lifecycle::Playing),
            "exited" => Some(Lifecycle::Exited),
            _ => None,
        }
    }
}

/// Сообщение `state`: что игра сообщает о себе.
#[derive(Debug, Clone, PartialEq)]
pub struct StateMessage {
    pub session: String,
    /// Номер сообщения (целое не меньше 0, число JS).
    pub seq: f64,
    pub lifecycle: Lifecycle,
    /// Карта (`normalizeMapName`) — только в `playing`.
    pub map_name: Option<String>,
    pub fps: Option<i64>,
    pub timer_running: bool,
    pub timer_completed: bool,
    pub elapsed_ms: Option<i64>,
}

/// Принятое сообщение от игры.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Hello {
        token: String,
        session: String,
    },
    State(StateMessage),
    ChatSubscribe {
        session: String,
        platform: String,
        id: String,
    },
    ChatUnsubscribe {
        session: String,
    },
    EmoteSet {
        session: String,
        room_id: String,
    },
    EmoteResolve {
        session: String,
        name: String,
        px: Option<u32>,
    },
}

impl Message {
    /// Сеанс игры, из которого пришло сообщение.
    pub fn session(&self) -> &str {
        match self {
            Message::Hello { session, .. }
            | Message::ChatSubscribe { session, .. }
            | Message::ChatUnsubscribe { session }
            | Message::EmoteSet { session, .. }
            | Message::EmoteResolve { session, .. } => session,
            Message::State(state) => &state.session,
        }
    }

    /// Сообщение потребителям чата (`chatHandler`): всё, кроме `hello` и `state`.
    pub fn to_chat(&self) -> Option<ChatMessage> {
        Some(match self.clone() {
            Message::Hello { .. } | Message::State(_) => return None,
            Message::ChatSubscribe {
                session,
                platform,
                id,
            } => ChatMessage::Subscribe {
                session,
                platform,
                id,
            },
            Message::ChatUnsubscribe { session } => ChatMessage::Unsubscribe { session },
            Message::EmoteSet { session, room_id } => ChatMessage::EmoteSet { session, room_id },
            Message::EmoteResolve { session, name, px } => {
                ChatMessage::EmoteResolve { session, name, px }
            }
        })
    }

    /// Объект JS, который вернул `parseLine` (`message`).
    pub fn to_value(&self) -> Value {
        match self {
            Message::Hello { token, session } => {
                json!({"type": "hello", "token": token, "session": session})
            }
            Message::State(state) => json!({
                "type": "state",
                "session": state.session,
                "seq": js_number(state.seq),
                "lifecycle": state.lifecycle.as_str(),
                "mapName": state.map_name,
                "fps": state.fps,
                "timerRunning": state.timer_running,
                "timerCompleted": state.timer_completed,
                "elapsedMs": state.elapsed_ms,
            }),
            Message::ChatSubscribe {
                session,
                platform,
                id,
            } => {
                json!({"type": "chat.subscribe", "session": session, "platform": platform, "id": id})
            }
            Message::ChatUnsubscribe { session } => {
                json!({"type": "chat.unsubscribe", "session": session})
            }
            Message::EmoteSet { session, room_id } => {
                json!({"type": "emote.set", "session": session, "roomId": room_id})
            }
            Message::EmoteResolve { session, name, px } => {
                json!({"type": "emote.resolve", "session": session, "name": name, "px": px})
            }
        }
    }
}

/// Результат `parseLine` в виде объекта JS: `{ok: true, message}` или `{ok: false, reason}`.
pub fn outcome_value(outcome: &Result<Message, String>) -> Value {
    match outcome {
        Ok(message) => json!({"ok": true, "message": message.to_value()}),
        Err(reason) => json!({"ok": false, "reason": reason}),
    }
}

fn fail<T>(reason: impl Into<String>) -> Result<T, String> {
    Err(reason.into())
}

/// Первое поле вне списка в порядке `Object.keys`.
fn unknown_field(raw: &Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    for (key, _) in js_key_order(raw) {
        if !allowed.contains(&key.as_str()) {
            return fail(format!("UNKNOWN_FIELD:{key}"));
        }
    }
    Ok(())
}

/// `^[0-9a-f]{8,64}$`.
pub fn is_session(text: &str) -> bool {
    (8..=64).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn session_of(raw: &Map<String, Value>) -> Result<String, String> {
    match raw.get("session").and_then(Value::as_str) {
        Some(session) if is_session(session) => Ok(session.to_owned()),
        _ => fail("BAD_SESSION"),
    }
}

/// `^[A-Za-z0-9_-]{5,32}$`.
pub fn is_video_id(text: &str) -> bool {
    (5..=32).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn is_digits(text: &str, max: usize) -> bool {
    (1..=max).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit())
}

/// `^[0-9]{1,20}$`.
pub fn is_room_id(text: &str) -> bool {
    is_digits(text, 20)
}

/// `^(?:[0-9]{1,20}|twitch:[0-9]{1,20}|(?:set|user):[0-9A-Za-z]{1,32})$`.
pub fn is_emote_source(text: &str) -> bool {
    if is_digits(text, 20) {
        return true;
    }
    if let Some(rest) = text.strip_prefix("twitch:") {
        return is_digits(rest, 20);
    }
    ["set:", "user:"].iter().any(|prefix| {
        text.strip_prefix(prefix).is_some_and(|rest| {
            (1..=32).contains(&rest.len()) && rest.bytes().all(|b| b.is_ascii_alphanumeric())
        })
    })
}

/// `^[A-Za-z0-9_()!?:-]{1,47}$`.
pub fn is_emote_name(text: &str) -> bool {
    (1..=47).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_()!?:-".contains(&b))
}

/// `Number.isInteger(value)` для значения JSON: число JSON — только `Value::Number`.
fn integer_of(value: Option<&Value>) -> Option<f64> {
    let number = value?.as_number()?.as_f64()?;
    (number.is_finite() && number.fract() == 0.0).then_some(number)
}

/// `parseLine(line)`: сообщение или причина отказа.
pub fn parse_line(line: &str) -> Result<Message, String> {
    if line.len() > MAX_LINE_BYTES {
        return fail("TOO_LONG");
    }
    let Ok(raw) = json_parse(line) else {
        return fail("NOT_JSON");
    };
    let Some(raw) = raw.as_object() else {
        return fail("NOT_AN_OBJECT");
    };
    let version = raw.get("v").and_then(|v| v.as_number()?.as_f64());
    if version != Some(f64::from(PROTOCOL_VERSION)) {
        return fail("BAD_VERSION");
    }

    match raw.get("type").and_then(Value::as_str) {
        Some("hello") => parse_hello(raw),
        Some("state") => parse_state(raw),
        Some("chat.subscribe") => parse_chat_subscribe(raw),
        Some("chat.unsubscribe") => parse_chat_unsubscribe(raw),
        Some("emote.set") => parse_emote_set(raw),
        Some("emote.resolve") => parse_emote_resolve(raw),
        _ => fail("UNKNOWN_TYPE"),
    }
}

fn parse_hello(raw: &Map<String, Value>) -> Result<Message, String> {
    unknown_field(raw, &HELLO_FIELDS)?;
    let token = match raw.get("token").and_then(Value::as_str) {
        Some(token) if is_token(token) => token.to_owned(),
        _ => return fail("BAD_TOKEN"),
    };
    let session = session_of(raw)?;
    Ok(Message::Hello { token, session })
}

fn parse_chat_subscribe(raw: &Map<String, Value>) -> Result<Message, String> {
    unknown_field(raw, &CHAT_SUBSCRIBE_FIELDS)?;
    let session = session_of(raw)?;
    let platform = match raw.get("platform").and_then(Value::as_str) {
        Some(platform) if PLATFORMS.contains(&platform) => platform,
        _ => return fail("BAD_PLATFORM"),
    };
    let Some(id) = raw.get("id").and_then(Value::as_str) else {
        return fail("BAD_ID");
    };
    if platform == "youtube" && !is_video_id(id) {
        return fail("BAD_ID");
    }
    if platform == "twitch" && !is_room_id(id) {
        return fail("BAD_ID");
    }
    Ok(Message::ChatSubscribe {
        session,
        platform: platform.to_owned(),
        id: id.to_owned(),
    })
}

fn parse_chat_unsubscribe(raw: &Map<String, Value>) -> Result<Message, String> {
    unknown_field(raw, &CHAT_UNSUBSCRIBE_FIELDS)?;
    Ok(Message::ChatUnsubscribe {
        session: session_of(raw)?,
    })
}

fn parse_emote_set(raw: &Map<String, Value>) -> Result<Message, String> {
    unknown_field(raw, &EMOTE_SET_FIELDS)?;
    let session = session_of(raw)?;
    match raw.get("roomId").and_then(Value::as_str) {
        Some(room_id) if is_emote_source(room_id) => Ok(Message::EmoteSet {
            session,
            room_id: room_id.to_owned(),
        }),
        _ => fail("BAD_ROOM_ID"),
    }
}

fn parse_emote_resolve(raw: &Map<String, Value>) -> Result<Message, String> {
    unknown_field(raw, &EMOTE_RESOLVE_FIELDS)?;
    let session = session_of(raw)?;
    let name = match raw.get("name").and_then(Value::as_str) {
        Some(name) if is_emote_name(name) => name.to_owned(),
        _ => return fail("BAD_EMOTE_NAME"),
    };
    let mut px = None;
    if let Some(value) = raw.get("px") {
        match integer_of(Some(value)) {
            Some(number) if (EMOTE_PX_MIN..=EMOTE_PX_MAX).contains(&number) => {
                px = Some(number as u32);
            }
            _ => return fail("BAD_EMOTE_PX"),
        }
    }
    Ok(Message::EmoteResolve { session, name, px })
}

fn parse_state(raw: &Map<String, Value>) -> Result<Message, String> {
    unknown_field(raw, &STATE_FIELDS)?;
    let session = session_of(raw)?;

    let seq = match integer_of(raw.get("seq")) {
        Some(seq) if seq >= 0.0 => seq,
        _ => return fail("BAD_SEQ"),
    };
    let Some(lifecycle) = raw
        .get("lifecycle")
        .and_then(Value::as_str)
        .and_then(Lifecycle::of)
    else {
        return fail("BAD_LIFECYCLE");
    };
    let Some(timer_running) = raw.get("timerRunning").and_then(Value::as_bool) else {
        return fail("BAD_TIMER_RUNNING");
    };
    let Some(timer_completed) = raw.get("timerCompleted").and_then(Value::as_bool) else {
        return fail("BAD_TIMER_COMPLETED");
    };

    let present = |key: &str| raw.get(key).filter(|value| !value.is_null());

    let mut map_name = None;
    if let Some(value) = present("mapName") {
        map_name = map_assets::normalize_map_name(value.as_str());
        if map_name.is_none() {
            return fail("BAD_MAP_NAME");
        }
    }
    let mut fps = None;
    if let Some(value) = present("fps") {
        fps = map_assets::normalize_fps(hlsr_core::js::value::number(value));
        if fps.is_none() {
            return fail("BAD_FPS");
        }
    }
    let mut elapsed_ms = None;
    if let Some(value) = present("timerElapsedMilliseconds") {
        elapsed_ms = map_assets::normalize_elapsed_ms(hlsr_core::js::value::number(value));
        if elapsed_ms.is_none() {
            return fail("BAD_ELAPSED");
        }
    }

    if timer_running && elapsed_ms.is_none() {
        return fail("RUNNING_WITHOUT_ELAPSED");
    }
    if lifecycle == Lifecycle::Playing && map_name.is_none() {
        return fail("PLAYING_WITHOUT_MAP");
    }
    if lifecycle != Lifecycle::Playing && map_name.is_some() {
        return fail("MAP_OUTSIDE_PLAYING");
    }

    Ok(Message::State(StateMessage {
        session,
        seq,
        lifecycle,
        map_name,
        fps,
        timer_running,
        timer_completed,
        elapsed_ms,
    }))
}

// --------------------------------------------------------------------------- кодировщики

/// `encodeDiscordAck(connected)`: строка игре о том, подключён ли Discord.
pub fn encode_discord_ack(connected: bool) -> String {
    let value = json!({"v": PROTOCOL_VERSION, "type": "discord", "connected": connected});
    line(&value)
}

fn line(value: &Value) -> String {
    stringify(value, "") + "\n"
}

/// `sliceSafe(value, max)`: не больше `max` единиц UTF-16, разрезанная суррогатная пара
/// отбрасывается целиком.
pub fn slice_safe(text: &str, max: usize) -> String {
    let mut used = 0;
    text.chars()
        .take_while(|c| {
            used += c.len_utf16();
            used <= max
        })
        .collect()
}

/// `String(name).slice(0, max)`. shortcut: разрезанную пару JS оставил бы одиночным суррогатом,
/// здесь — U+FFFD (`slice_utf16`); имена эмоутов и значков — ASCII, до игры такое не доходит.
fn name_of(name: &str, max: usize) -> String {
    slice_utf16(name, max)
}

/// `encodeChatLine(platform, nick, text, badge1, badge2)`: строка чата.
pub fn encode_chat_line(
    platform: &str,
    nick: &str,
    text: &str,
    badge1: &str,
    badge2: &str,
) -> String {
    line(&json!({
        "v": PROTOCOL_VERSION,
        "type": "chat.line",
        "platform": slice_safe(platform, 16),
        "badge1": slice_safe(badge1, 31),
        "badge2": slice_safe(badge2, 31),
        "nick": slice_safe(nick, 32),
        "text": slice_safe(text, 240),
    }))
}

/// `encodeEmoteBegin(name, wide, tall, frames, frameMs)`: начало анимированного эмоута.
pub fn encode_emote_begin(name: &str, wide: u32, tall: u32, frames: u32, frame_ms: u32) -> String {
    line(&json!({
        "v": PROTOCOL_VERSION,
        "type": "emote.begin",
        "name": name_of(name, 47),
        "wide": wide,
        "tall": tall,
        "frames": frames,
        "frameMs": frame_ms,
    }))
}

/// `encodeEmoteFrame(name, index, rgbaBase64)`: кадр анимации.
pub fn encode_emote_frame(name: &str, index: u32, rgba_base64: &str) -> String {
    line(&json!({
        "v": PROTOCOL_VERSION,
        "type": "emote.frame",
        "name": name_of(name, 47),
        "index": index,
        "rgba": rgba_base64,
    }))
}

/// `encodeEmoteReady(name, wide, tall, rgbaBase64)`: неподвижный эмоут.
pub fn encode_emote_ready(name: &str, wide: u32, tall: u32, rgba_base64: &str) -> String {
    line(&json!({
        "v": PROTOCOL_VERSION,
        "type": "emote.ready",
        "name": name_of(name, 47),
        "wide": wide,
        "tall": tall,
        "rgba": rgba_base64,
    }))
}

/// `encodeBadgeReady(name, wide, tall, rgbaBase64)`: значок.
pub fn encode_badge_ready(name: &str, wide: u32, tall: u32, rgba_base64: &str) -> String {
    line(&json!({
        "v": PROTOCOL_VERSION,
        "type": "badge.ready",
        "name": name_of(name, 31),
        "wide": wide,
        "tall": tall,
        "rgba": rgba_base64,
    }))
}

/// `encodeEmoteMiss(name)`: эмоута нет.
pub fn encode_emote_miss(name: &str) -> String {
    line(&json!({"v": PROTOCOL_VERSION, "type": "emote.miss", "name": slice_safe(name, 47)}))
}
