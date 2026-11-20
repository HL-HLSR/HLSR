//! Плитки скриншотов в карточке поста (`shared/news-image-layout.js`). Та же раскладка в
//! предпросмотре админки: один скриншот — во всю ширину по пропорциям выбранной области (от 4:3 до
//! 21:9), несколько — в две колонки 16:9, при нечётном числе первый на всю ширину 2:1.

use crate::js_value;
use hlsr_core::js;
use hlsr_core::json_text::js_number;
use serde_json::{Map, Value};

/// `FULL`: картинка целиком.
pub const FULL: Crop = Crop {
    x: 0.0,
    y: 0.0,
    w: 1.0,
    h: 1.0,
};

/// `MIN_SIDE`: меньше доли картинки область не бывает.
const MIN_SIDE: f64 = 0.02;

/// Область `{x, y, w, h}` в долях картинки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crop {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Crop {
    /// Объект `{x, y, w, h}`.
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        for (key, value) in [("x", self.x), ("y", self.y), ("w", self.w), ("h", self.h)] {
            map.insert(key.to_owned(), js_number(value));
        }
        Value::Object(map)
    }
}

/// `parseCrop(raw)`: область или `None` (картинка целиком).
pub fn parse_crop(raw: Option<&Value>) -> Option<Crop> {
    if !js_value::is_object(raw) {
        return None;
    }
    let mut parts = [0.0; 4];
    for (part, key) in parts.iter_mut().zip(["x", "y", "w", "h"]) {
        let value = js_value::finite_number(js_value::field(raw, key))?;
        if !(0.0..=1.0).contains(&value) {
            return None;
        }
        *part = value;
    }
    let [x, y, w, h] = parts;
    if w < MIN_SIDE || h < MIN_SIDE || x + w > 1.0001 || y + h > 1.0001 {
        return None;
    }
    Some(Crop {
        x,
        y,
        w: w.min(1.0 - x),
        h: h.min(1.0 - y),
    })
}

/// Картинка ленты для раскладки: размеры в пикселях и область.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileImage {
    pub width: f64,
    pub height: f64,
    pub crop: Option<Crop>,
}

fn clamp(value: f64, min: f64, max: f64) -> f64 {
    value.max(min).min(max)
}

/// `tileAspect(image, index, count)`: пропорции плитки (ширина / высота).
pub fn tile_aspect(image: &TileImage, index: usize, count: usize) -> f64 {
    if count == 1 {
        let crop = image.crop.unwrap_or(FULL);
        return clamp(
            (crop.w * image.width) / (crop.h * image.height),
            4.0 / 3.0,
            21.0 / 9.0,
        );
    }
    if count % 2 == 1 && index == 0 {
        2.0
    } else {
        16.0 / 9.0
    }
}

/// Положение картинки в плитке: проценты `width`, `height`, `left`, `top` (CSS-строки).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileStyle {
    pub width: String,
    pub height: String,
    pub left: String,
    pub top: String,
}

impl TileStyle {
    /// Объект `{width, height, left, top}`.
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        for (key, value) in [
            ("width", &self.width),
            ("height", &self.height),
            ("left", &self.left),
            ("top", &self.top),
        ] {
            map.insert(key.to_owned(), Value::from(value.as_str()));
        }
        Value::Object(map)
    }
}

/// `+(value * 100).toFixed(4)` и знак процента: `100%`, `-33.3333%`, `0%`.
fn percent(value: f64) -> String {
    let number = js::to_number(&js::to_fixed(value * 100.0, 4));
    format!("{}%", js_number(number))
}

/// `tileImageStyle(image, aspect)`: область покрывает плитку по центру, как `object-fit: cover`,
/// только относительно области, а не всей картинки.
pub fn tile_image_style(image: &TileImage, aspect: f64) -> TileStyle {
    let crop = image.crop.unwrap_or(FULL);
    let mut w = crop.w * image.width;
    let mut h = crop.h * image.height;
    let mut x = crop.x * image.width;
    let mut y = crop.y * image.height;
    if w / h > aspect {
        let next = h * aspect;
        x += (w - next) / 2.0;
        w = next;
    } else {
        let next = w / aspect;
        y += (h - next) / 2.0;
        h = next;
    }
    TileStyle {
        width: percent(image.width / w),
        height: percent(image.height / h),
        left: percent(-x / w),
        top: percent(-y / h),
    }
}
