//! Проверка CSS ника и рамки аватара (`shared/css-declarations.js`; на сервере —
//! `Profile/CssDeclarations.php`): только объявления «свойство: значение» из списка, без `url()`,
//! вложенных правил, экранирования, строк, комментариев и управляющих символов.

use hlsr_core::js;
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

/// `FORBIDDEN` (флаг `i` без `u`: регистр не важен только у латиницы) — над текстом в нижнем
/// регистре ASCII.
static FORBIDDEN: LazyLock<Regex> = LazyLock::new(|| {
    let ws = js::WHITESPACE_CLASS;
    Regex::new(&format!(
        r#"url[{ws}]*\(|image[{ws}]*\(|image-set[{ws}]*\(|element[{ws}]*\(|cross-fade[{ws}]*\(|expression|javascript:|@|[{{}}<>\\"'`]|/\*|[\x00-\x1f\x7f]"#
    ))
    .expect("шаблон FORBIDDEN")
});

/// `FORBIDDEN.test(text)`.
pub fn is_forbidden(text: &str) -> bool {
    FORBIDDEN.is_match(&text.to_ascii_lowercase())
}

/// `sanitize(value, properties, max)`: нормализованная строка объявлений, `""` для пустой или
/// `None`, если хоть одно объявление не прошло проверку (или значение не строка).
pub fn sanitize(value: &Value, properties: &[&str], max: usize) -> Option<String> {
    let text = js::trim(value.as_str()?);
    if text.is_empty() {
        return Some(String::new());
    }
    if js::text::utf16_len(text) > max || is_forbidden(text) {
        return None;
    }
    let mut depth: i64 = 0;
    for c in text.chars() {
        if c == '(' {
            depth += 1;
        }
        if c == ')' {
            depth -= 1;
            if depth < 0 {
                return None;
            }
        }
    }
    if depth != 0 {
        return None;
    }
    let mut out = Vec::new();
    for part in text.split(';') {
        if js::trim(part).is_empty() {
            continue;
        }
        let colon = part.find(':')?;
        let property = js::trim(&part[..colon]).to_lowercase();
        let body = js::trim(&part[colon + 1..]);
        if !properties.contains(&property.as_str())
            || body.is_empty()
            || js::text::utf16_len(body) > 300
        {
            return None;
        }
        out.push(format!("{property}: {body}"));
    }
    Some(out.join("; "))
}
