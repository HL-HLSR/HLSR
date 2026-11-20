//! Оформление ника (`shared/nick-styles.js`, сервер — `Profile/NickStyles.php`): готовый стиль из
//! каталога или свой CSS только из свойств вида текста.

use super::css_declarations;
use serde_json::{Value, json};

pub const PRESETS: [&str; 3] = ["developer", "gold", "ghost"];

pub const MAX_CSS: usize = 600;

pub const PROPERTIES: [&str; 26] = [
    "color",
    "background",
    "background-color",
    "background-image",
    "background-size",
    "background-position",
    "background-repeat",
    "background-clip",
    "-webkit-background-clip",
    "-webkit-text-fill-color",
    "-webkit-text-stroke",
    "text-shadow",
    "font-weight",
    "font-style",
    "letter-spacing",
    "text-transform",
    "text-decoration",
    "text-decoration-color",
    "filter",
    "animation",
    "animation-name",
    "animation-duration",
    "animation-timing-function",
    "animation-iteration-count",
    "animation-direction",
    "animation-delay",
];

/// `sanitizeCss(value)`.
pub fn sanitize_css(value: &Value) -> Option<String> {
    css_declarations::sanitize(value, &PROPERTIES, MAX_CSS)
}

/// `raw.css || ""` через проверку; не прошло — `""`.
pub(crate) fn css_of(raw: &Value, sanitize: fn(&Value) -> Option<String>) -> String {
    match raw.get("css").filter(|css| hlsr_core::js::truthy(css)) {
        Some(css) => sanitize(css).unwrap_or_default(),
        None => String::new(),
    }
}

/// `normalize(raw)`: `{style, css}` или `None`, если нет ни стиля из каталога, ни годного CSS.
pub fn normalize(raw: &Value) -> Option<Value> {
    if !matches!(raw, Value::Object(_) | Value::Array(_)) {
        return None;
    }
    let style = raw
        .get("style")
        .and_then(Value::as_str)
        .filter(|style| PRESETS.contains(style))
        .unwrap_or("");
    let css = css_of(raw, sanitize_css);
    (!style.is_empty() || !css.is_empty()).then(|| json!({"style": style, "css": css}))
}
