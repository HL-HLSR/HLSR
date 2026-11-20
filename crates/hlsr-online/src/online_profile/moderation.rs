//! Модерация из админки (`online-profile/moderation.js`, сервер — `Hlsr\Profile\Moderation`):
//! действующие бан и запрет статуса и записи за 30 дней. Каждая новая запись — одно уведомление.

use crate::js_value::{field, is_object, safe_integer};
use hlsr_core::js;
use serde_json::{Value, json};

pub const ACTIONS: [&str; 9] = [
    "ban",
    "unban",
    "mute",
    "unmute",
    "reset-status",
    "reset-nick",
    "reset-avatar",
    "restore-nick",
    "restore-avatar",
];

/// `CODES`: код уведомления (`notifications/catalog`) по действию.
pub const CODES: [(&str, &str); 9] = [
    ("ban", "MOD_BAN"),
    ("unban", "MOD_UNBAN"),
    ("mute", "MOD_MUTE"),
    ("unmute", "MOD_UNMUTE"),
    ("reset-status", "MOD_RESET_STATUS"),
    ("reset-nick", "MOD_RESET_NICK"),
    ("reset-avatar", "MOD_RESET_AVATAR"),
    ("restore-nick", "MOD_RESTORE_NICK"),
    ("restore-avatar", "MOD_RESTORE_AVATAR"),
];

pub const MAX_SEEN: usize = 200;

/// Код уведомления действия.
pub fn code(action: &str) -> Option<&'static str> {
    CODES
        .iter()
        .find(|(key, _)| *key == action)
        .map(|(_, code)| *code)
}

/// `ISO`: `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}([+-]\d{2}:\d{2}|Z)$`.
fn is_iso(text: &str) -> bool {
    let b = text.as_bytes();
    let digits = |range: std::ops::Range<usize>| b[range].iter().all(u8::is_ascii_digit);
    let head = b.len() >= 20
        && digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10)
        && b[10] == b'T'
        && digits(11..13)
        && b[13] == b':'
        && digits(14..16)
        && b[16] == b':'
        && digits(17..19);
    head && match &b[19..] {
        [b'Z'] => true,
        [sign, h1, h2, b':', m1, m2] => {
            matches!(sign, b'+' | b'-') && [h1, h2, m1, m2].iter().all(|d| d.is_ascii_digit())
        }
        _ => false,
    }
}

/// `iso(value)`: строка ISO с зоной, которую принимает `Date.parse`, или `null`.
fn iso(value: Option<&Value>) -> Value {
    match value.and_then(Value::as_str) {
        Some(text) if is_iso(text) && js::date_parse(text).is_some() => json!(text),
        _ => Value::Null,
    }
}

/// `reason(value)`: управляющие символы — пробелы, без пробелов по краям, ≤ 255 единиц UTF-16.
fn reason(value: Option<&Value>) -> String {
    let Some(text) = value.and_then(Value::as_str) else {
        return String::new();
    };
    let spaced: String = text
        .chars()
        .map(|c| {
            if c <= '\u{1f}' || c == '\u{7f}' {
                ' '
            } else {
                c
            }
        })
        .collect();
    js::slice_utf16(js::trim(&spaced), 255)
}

fn positive_id(value: Option<&Value>) -> Option<i64> {
    safe_integer(field(value, "id")).filter(|id| *id > 0)
}

fn restriction(raw: Option<&Value>) -> Value {
    let Some(id) = positive_id(raw).filter(|_| is_object(raw)) else {
        return Value::Null;
    };
    json!({
        "id": id,
        "reason": reason(field(raw, "reason")),
        "createdAt": iso(field(raw, "createdAt")),
        "expiresAt": iso(field(raw, "expiresAt")),
    })
}

/// `normalize(raw)`: `{ban, mute, notices}` — записи (не больше 50) только известных действий.
pub fn normalize(raw: &Value) -> Value {
    let source = Some(raw).filter(|raw| is_object(Some(raw)));
    let notices: Vec<Value> = field(source, "notices")
        .and_then(Value::as_array)
        .map_or(&[][..], |items| &items[..items.len().min(50)])
        .iter()
        .filter_map(|item| {
            let id = positive_id(Some(item)).filter(|_| is_object(Some(item)))?;
            let action = item
                .get("action")
                .and_then(Value::as_str)
                .filter(|action| ACTIONS.contains(action))?;
            Some(json!({
                "id": id,
                "action": action,
                "reason": reason(item.get("reason")),
                "createdAt": iso(item.get("createdAt")),
                "expiresAt": iso(item.get("expiresAt")),
            }))
        })
        .collect();
    json!({
        "ban": restriction(field(source, "ban")),
        "mute": restriction(field(source, "mute")),
        "notices": notices,
    })
}

/// `activeAt(item, now)`: ограничение действует — срока нет или он ещё не вышел.
pub fn active_at(item: &Value, now: f64) -> bool {
    if !js::truthy(item) {
        return false;
    }
    match item.get("expiresAt").filter(|at| js::truthy(at)) {
        None => true,
        Some(at) => js::date_parse(&js::to_template_string(Some(at)))
            .is_some_and(|expires| expires as f64 > now),
    }
}

fn id_of(item: &Value) -> f64 {
    item.get("id").and_then(Value::as_f64).unwrap_or(f64::NAN)
}

/// `unseen(state, seen)`: записи без уведомления — старые сверху.
pub fn unseen(state: &Value, seen: &[i64]) -> Vec<Value> {
    let mut fresh: Vec<Value> = state
        .get("notices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| !seen.iter().any(|id| *id as f64 == id_of(item)))
        .cloned()
        .collect();
    js::sort::sort_by_comparator(&mut fresh, |a, b| id_of(a) - id_of(b));
    fresh
}

/// `remember(seen, notices)`: добавить id записей; последние 200.
pub fn remember(seen: &[i64], notices: &[Value]) -> Vec<i64> {
    let mut next = seen.to_vec();
    next.extend(
        notices
            .iter()
            .filter_map(|item| item.get("id").and_then(Value::as_i64))
            .filter(|id| !seen.contains(id)),
    );
    let from = next.len().saturating_sub(MAX_SEEN);
    next.split_off(from)
}
