//! Настройки оверлея LiveSplit в окне игры (`src/shared/livesplit-overlay.js`): умолчания и
//! проверка. Геометрия, маски фона и `overlay.ini` для прокси оверлея относятся к запуску
//! (подпроект 3) и здесь не переносятся.

use crate::js_value::prop;
use hlsr_core::js::{self, math_round};
use hlsr_core::json_text::{js_number, stringify};
use serde_json::{Map, Value};

pub const KEYS: [&str; 3] = ["none", "color", "luma"];

/// Пределы значений: `x` и `y` — где картинка стоит в свободном месте кадра (0 — у левого или
/// верхнего края, 100 — у правого или нижнего), `height` — доля высоты кадра.
pub const LIMITS: [(&str, (f64, f64)); 11] = [
    ("x", (0.0, 100.0)),
    ("y", (0.0, 100.0)),
    ("height", (5.0, 100.0)),
    ("opacity", (0.0, 100.0)),
    ("similarity", (1.0, 1000.0)),
    ("smoothness", (1.0, 1000.0)),
    ("lumaMin", (0.0, 1.0)),
    ("lumaMinSmooth", (0.0, 1.0)),
    ("lumaMax", (0.0, 1.0)),
    ("lumaMaxSmooth", (0.0, 1.0)),
    ("crop", (0.0, 2048.0)),
];

fn limits(name: &str) -> (f64, f64) {
    LIMITS
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, range)| *range)
        .expect("предел задан")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crop {
    pub top: u16,
    pub right: u16,
    pub bottom: u16,
    pub left: u16,
}

/// Настройки оверлея после `sanitizeOverlay`.
#[derive(Debug, Clone, PartialEq)]
pub struct Overlay {
    pub enabled: bool,
    pub x: f64,
    pub y: f64,
    pub height: f64,
    pub opacity: u8,
    /// Одно из [`KEYS`].
    pub key: &'static str,
    pub key_color: KeyColor,
    pub similarity: u16,
    pub smoothness: u16,
    pub luma_min: f64,
    pub luma_min_smooth: f64,
    pub luma_max: f64,
    pub luma_max_smooth: f64,
    pub crop: Crop,
}

impl Overlay {
    /// Объект настроек в порядке ключей 0.4.7 (от него зависят байты `game-settings.json`).
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        let mut put = |key: &str, value: Value| {
            object.insert(key.to_owned(), value);
        };
        put("enabled", Value::Bool(self.enabled));
        put("x", js_number(self.x));
        put("y", js_number(self.y));
        put("height", js_number(self.height));
        put("opacity", Value::from(self.opacity));
        put("key", Value::String(self.key.to_owned()));
        let mut color = Map::new();
        color.insert("r".into(), Value::from(self.key_color.r));
        color.insert("g".into(), Value::from(self.key_color.g));
        color.insert("b".into(), Value::from(self.key_color.b));
        put("keyColor", Value::Object(color));
        put("similarity", Value::from(self.similarity));
        put("smoothness", Value::from(self.smoothness));
        put("lumaMin", js_number(self.luma_min));
        put("lumaMinSmooth", js_number(self.luma_min_smooth));
        put("lumaMax", js_number(self.luma_max));
        put("lumaMaxSmooth", js_number(self.luma_max_smooth));
        let mut crop = Map::new();
        crop.insert("top".into(), Value::from(self.crop.top));
        crop.insert("right".into(), Value::from(self.crop.right));
        crop.insert("bottom".into(), Value::from(self.crop.bottom));
        crop.insert("left".into(), Value::from(self.crop.left));
        put("crop", Value::Object(crop));
        Value::Object(object)
    }
}

/// `defaultOverlay()`.
pub fn default_overlay() -> Overlay {
    Overlay {
        enabled: false,
        x: 100.0,
        y: 50.0,
        height: 45.0,
        opacity: 100,
        key: "none",
        // Как у OBS: зелёный ключ, сходство 400, сглаживание 80.
        key_color: KeyColor { r: 0, g: 255, b: 0 },
        similarity: 400,
        smoothness: 80,
        luma_min: 0.1,
        luma_min_smooth: 0.05,
        luma_max: 1.0,
        luma_max_smooth: 0.0,
        crop: Crop {
            top: 0,
            right: 0,
            bottom: 0,
            left: 0,
        },
    }
}

/// `number(value, [min, max], fallback, integer)`: число или непустая строка с числом, зажатые в
/// пределы; целое — `Math.round`, дробное — до тысячных. Прочее — `fallback`.
fn number(value: Option<&Value>, (min, max): (f64, f64), fallback: f64, integer: bool) -> f64 {
    let parsed = match value {
        Some(number @ Value::Number(_)) => crate::js_value::number(number).unwrap_or(f64::NAN),
        Some(Value::String(text)) if !js::trim(text).is_empty() => js::to_number(text),
        _ => f64::NAN,
    };
    if !parsed.is_finite() {
        return fallback;
    }
    let clamped = max.min(min.max(parsed));
    if integer {
        math_round(clamped)
    } else {
        math_round(clamped * 1000.0) / 1000.0
    }
}

fn color_of(value: Option<&Value>, fallback: KeyColor) -> KeyColor {
    let source = value.unwrap_or(&Value::Null);
    let channel = |name: &str, default: u8| {
        number(prop(source, name), (0.0, 255.0), f64::from(default), true) as u8
    };
    KeyColor {
        r: channel("r", fallback.r),
        g: channel("g", fallback.g),
        b: channel("b", fallback.b),
    }
}

/// `sanitizeOverlay(input)`: любой вход приводится к допустимым значениям.
pub fn sanitize_overlay(input: &Value) -> Overlay {
    let source = input;
    let base = default_overlay();
    let crop = prop(source, "crop").unwrap_or(&Value::Null);
    let crop_limits = limits("crop");
    let side = |name: &str| number(prop(crop, name), crop_limits, 0.0, true) as u16;
    let count = |name: &str, fallback: u16| {
        number(prop(source, name), limits(name), f64::from(fallback), true) as u16
    };
    let fraction =
        |name: &str, fallback: f64| number(prop(source, name), limits(name), fallback, false);
    Overlay {
        enabled: matches!(prop(source, "enabled"), Some(Value::Bool(true))),
        x: fraction("x", base.x),
        y: fraction("y", base.y),
        height: fraction("height", base.height),
        opacity: count("opacity", u16::from(base.opacity)) as u8,
        key: prop(source, "key")
            .and_then(Value::as_str)
            .and_then(|key| KEYS.iter().copied().find(|known| *known == key))
            .unwrap_or(base.key),
        key_color: color_of(prop(source, "keyColor"), base.key_color),
        similarity: count("similarity", base.similarity),
        smoothness: count("smoothness", base.smoothness),
        luma_min: fraction("lumaMin", base.luma_min),
        luma_min_smooth: fraction("lumaMinSmooth", base.luma_min_smooth),
        luma_max: fraction("lumaMax", base.luma_max),
        luma_max_smooth: fraction("lumaMaxSmooth", base.luma_max_smooth),
        crop: Crop {
            top: side("top"),
            right: side("right"),
            bottom: side("bottom"),
            left: side("left"),
        },
    }
}

/// `overlayChanges(value)`: сколько настроек отличается от стандартных (точка «изменено» у
/// раздела). Выключатель не считается: сброс раздела его не трогает.
pub fn overlay_changes(value: &Value) -> usize {
    let clean = sanitize_overlay(value).to_value();
    let base = default_overlay().to_value();
    let (Value::Object(clean), Value::Object(base)) = (&clean, &base) else {
        unreachable!("настройки оверлея — объект");
    };
    base.iter()
        .filter(|(key, _)| key.as_str() != "enabled")
        .filter(|(key, default)| stringify(&clean[key.as_str()], "") != stringify(default, ""))
        .count()
}
