//! Свои значки из админки (`shared/custom-badges.js`, сервер — `Profile/CustomBadges.php`): подписи
//! ru/en, пояснение и иконка SVG, которая уходит в окно только `data:`-адресом.

use crate::js_value::{coerced, is_object};
use base64::Engine;
use hlsr_core::js;
use serde_json::{Value, json};

pub const MAX_SVG: usize = 20480;

/// `ID`: `^c-[0-9a-f]{8}$`.
pub fn is_id(text: &str) -> bool {
    text.len() == 10
        && text.starts_with("c-")
        && text[2..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `text(value, max)`: строка без пробелов по краям, не длиннее `max` единиц UTF-16.
fn text(value: Option<&Value>, max: usize) -> String {
    value
        .and_then(Value::as_str)
        .map_or_else(String::new, |s| js::slice_utf16(js::trim(s), max))
}

fn normalize_one(raw: &Value) -> Option<Value> {
    if !is_object(Some(raw)) || !is_id(&coerced(raw.get("id"))) {
        return None;
    }
    let svg = raw.get("svg").and_then(Value::as_str).map_or("", js::trim);
    let after_tag = svg
        .strip_prefix("<svg")
        .and_then(|rest| rest.chars().next());
    if svg.is_empty()
        || js::text::utf16_len(svg) > MAX_SVG
        || !after_tag.is_some_and(|c| c == '>' || js::is_whitespace(c))
    {
        return None;
    }
    let group = |key: &str| raw.get(key).filter(|value| is_object(Some(value)));
    let label = group("label");
    let note = group("note");
    let part =
        |group: Option<&Value>, key: &str, max: usize| text(group.and_then(|g| g.get(key)), max);
    let ru = part(label, "ru", 40);
    if ru.is_empty() {
        return None;
    }
    let en = Some(part(label, "en", 40))
        .filter(|en| !en.is_empty())
        .unwrap_or_else(|| ru.clone());
    let note_ru = part(note, "ru", 160);
    let note_en = Some(part(note, "en", 160))
        .filter(|en| !en.is_empty())
        .unwrap_or_else(|| note_ru.clone());
    Some(json!({
        "id": raw["id"],
        "label": {"ru": ru, "en": en},
        "note": {"ru": note_ru, "en": note_en},
        "icon": format!(
            "data:image/svg+xml;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(svg.as_bytes())
        ),
        "themed": raw.get("themed") == Some(&Value::Bool(true)),
    }))
}

/// `normalize(list)`: первые 32 описания, без повторов `id`.
pub fn normalize(list: &Value) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for raw in list
        .as_array()
        .map_or(&[][..], |items| &items[..items.len().min(32)])
    {
        if let Some(badge) = normalize_one(raw)
            && !out
                .iter()
                .any(|item| item["id"] == badge["id"] && badge["id"].is_string())
        {
            out.push(badge);
        }
    }
    out
}
