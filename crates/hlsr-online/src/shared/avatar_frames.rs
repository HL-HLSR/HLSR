//! Рамка аватара (`shared/avatar-frames.js`, сервер — `Profile/AvatarFrames.php`): свой CSS слоя
//! позади аватара, без размеров, положения и `url()`.

use super::{css_declarations, nick_styles};
use serde_json::{Value, json};

pub const MAX_CSS: usize = 600;

pub const PROPERTIES: [&str; 37] = [
    "border",
    "border-width",
    "border-style",
    "border-color",
    "border-image",
    "border-image-source",
    "border-image-slice",
    "border-image-width",
    "outline",
    "outline-width",
    "outline-style",
    "outline-color",
    "outline-offset",
    "box-shadow",
    "background",
    "background-color",
    "background-image",
    "background-size",
    "background-position",
    "background-repeat",
    "background-origin",
    "background-clip",
    "mask",
    "mask-image",
    "mask-composite",
    "-webkit-mask",
    "-webkit-mask-image",
    "-webkit-mask-composite",
    "filter",
    "opacity",
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

/// `normalize(raw)`: `{css}` или `None`.
pub fn normalize(raw: &Value) -> Option<Value> {
    if !matches!(raw, Value::Object(_) | Value::Array(_)) {
        return None;
    }
    let css = nick_styles::css_of(raw, sanitize_css);
    (!css.is_empty()).then(|| json!({"css": css}))
}
