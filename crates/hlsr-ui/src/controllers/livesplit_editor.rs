//! Редактор оверлея LiveSplit (`LiveSplitOverlayEditor.vue` 0.4.7, задача 31): источник
//! превью (снимок окна LiveSplit или пример; обновление каждые 2 с на Windows, кнопка
//! «Обновить» с вращением не меньше 600 мс и вспышкой карточки), включение, якоря 3 × 3, X/Y,
//! размер и прозрачность, обрезка, маска (нет, цвет, яркость), цвет ключа с пипеткой «фон».
//! Без окна: состояние и чистые функции; таймеры и канал `livesplit-overlay-capture` ведёт
//! адаптер. Изменения уходят наружу значением после `sanitizeOverlay` (`$emit('input')`).

use crate::controllers::preview::Snapshot;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hlsr_core::js;
use hlsr_core::json_text::stringify;
use hlsr_delivery::livesplit_overlay::{self, Overlay};
use hlsr_platform::livesplit_overlay_shared::{self as shared, Rgb};
use serde_json::{Map, Value, json};

/// Снимок обновляется сам раз в 2 с (Windows; под Proton — только кнопкой).
pub const AUTO_REFRESH_MS: u64 = 2000;
/// Кнопка «Обновить»: значок крутится не меньше 600 мс.
pub const MIN_SPIN_MS: u64 = 600;

/// Якорь оверлея: углы и края кадра плюс центр (`ANCHORS` + `center`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    pub id: &'static str,
    pub x: f64,
    pub y: f64,
}

pub const ANCHORS: [Anchor; 9] = [
    Anchor {
        id: "top-left",
        x: 0.0,
        y: 0.0,
    },
    Anchor {
        id: "top",
        x: 50.0,
        y: 0.0,
    },
    Anchor {
        id: "top-right",
        x: 100.0,
        y: 0.0,
    },
    Anchor {
        id: "left",
        x: 0.0,
        y: 50.0,
    },
    Anchor {
        id: "right",
        x: 100.0,
        y: 50.0,
    },
    Anchor {
        id: "bottom-left",
        x: 0.0,
        y: 100.0,
    },
    Anchor {
        id: "bottom",
        x: 50.0,
        y: 100.0,
    },
    Anchor {
        id: "bottom-right",
        x: 100.0,
        y: 100.0,
    },
    Anchor {
        id: "center",
        x: 50.0,
        y: 50.0,
    },
];

impl Anchor {
    /// Ключ подписи: `#L_LSO_ANCHOR_TOP_LEFT`.
    pub fn label_key(&self) -> String {
        format!("#L_LSO_ANCHOR_{}", self.id.to_uppercase().replace('-', "_"))
    }
}

/// Числовое поле с ползунком (`SIZE_FIELDS`, `COLOR_FIELDS`, `LUMA_FIELDS`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumberField {
    pub key: &'static str,
    pub label: &'static str,
    pub hint: Option<&'static str>,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    /// Единица рядом с числом (`%`).
    pub unit: Option<&'static str>,
}

pub const SIZE_FIELDS: [NumberField; 2] = [
    NumberField {
        key: "height",
        label: "#L_LSO_HEIGHT",
        hint: Some("#L_LSO_HEIGHT_HINT"),
        min: 5.0,
        max: 100.0,
        step: 1.0,
        unit: Some("%"),
    },
    NumberField {
        key: "opacity",
        label: "#L_LSO_OPACITY",
        hint: None,
        min: 0.0,
        max: 100.0,
        step: 1.0,
        unit: Some("%"),
    },
];
pub const COLOR_FIELDS: [NumberField; 2] = [
    NumberField {
        key: "similarity",
        label: "#L_LSO_SIMILARITY",
        hint: None,
        min: 1.0,
        max: 1000.0,
        step: 1.0,
        unit: None,
    },
    NumberField {
        key: "smoothness",
        label: "#L_LSO_SMOOTHNESS",
        hint: None,
        min: 1.0,
        max: 1000.0,
        step: 1.0,
        unit: None,
    },
];
pub const LUMA_FIELDS: [NumberField; 4] = [
    NumberField {
        key: "lumaMin",
        label: "#L_LSO_LUMA_MIN",
        hint: None,
        min: 0.0,
        max: 1.0,
        step: 0.01,
        unit: None,
    },
    NumberField {
        key: "lumaMinSmooth",
        label: "#L_LSO_LUMA_MIN_SMOOTH",
        hint: None,
        min: 0.0,
        max: 1.0,
        step: 0.01,
        unit: None,
    },
    NumberField {
        key: "lumaMax",
        label: "#L_LSO_LUMA_MAX",
        hint: None,
        min: 0.0,
        max: 1.0,
        step: 0.01,
        unit: None,
    },
    NumberField {
        key: "lumaMaxSmooth",
        label: "#L_LSO_LUMA_MAX_SMOOTH",
        hint: None,
        min: 0.0,
        max: 1.0,
        step: 0.01,
        unit: None,
    },
];
pub const CROP_SIDES: [&str; 4] = ["top", "right", "bottom", "left"];
/// Маски (`KEYS`): нет, цвет, яркость.
pub use hlsr_delivery::livesplit_overlay::KEYS;

/// Обрезка с каждой стороны: `0…LIMITS.crop[1]` px.
pub fn crop_max() -> f64 {
    livesplit_overlay::LIMITS
        .iter()
        .find(|(key, _)| *key == "crop")
        .map_or(2048.0, |(_, (_, max))| *max)
}

/// Тон карточки источника (`sourceTone`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Idle,
    Ok,
    Warn,
}

/// Положение ползунка и заливки в процентах дорожки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliderStyle {
    pub value: f64,
    pub default: f64,
    pub from: f64,
    pub to: f64,
}

/// Состояние редактора (`data()` и props).
#[derive(Debug, Clone)]
pub struct LivesplitEditor {
    value: Option<Value>,
    pub game_id: String,
    pub active: bool,
    /// `hlsrElectron.platform === 'linux'`: снимок только кнопкой.
    pub linux: bool,
    pub capturing: bool,
    pub flash: bool,
    /// Ответ снимка без `pixels`: байты уходят превью, здесь — только цвет фона из них.
    result: Option<Value>,
    background: Option<Rgb>,
    pub gone: bool,
}

impl LivesplitEditor {
    pub fn new(value: Option<Value>, game_id: &str, linux: bool) -> LivesplitEditor {
        LivesplitEditor {
            value,
            game_id: game_id.to_owned(),
            active: true,
            linux,
            capturing: false,
            flash: false,
            result: None,
            background: None,
            gone: false,
        }
    }

    pub fn set_value(&mut self, value: Option<Value>) {
        self.value = value;
    }

    /// `value` после проверки — те же настройки, что сейчас.
    pub fn same_as(&self, value: Option<&Value>) -> bool {
        self.overlay() == livesplit_overlay::sanitize_overlay(value.unwrap_or(&Value::Null))
    }

    /// `overlay`: настройки после проверки.
    pub fn overlay(&self) -> Overlay {
        livesplit_overlay::sanitize_overlay(self.value.as_ref().unwrap_or(&Value::Null))
    }

    pub fn defaults() -> Overlay {
        livesplit_overlay::default_overlay()
    }

    /// `changes`: сколько настроек отличается от стандартных.
    pub fn changes(&self) -> usize {
        livesplit_overlay::overlay_changes(self.value.as_ref().unwrap_or(&Value::Null))
    }

    pub fn result(&self) -> Option<&Value> {
        self.result.as_ref()
    }

    pub fn status(&self) -> String {
        self.result
            .as_ref()
            .and_then(|r| r.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("pending")
            .to_owned()
    }

    /// LiveSplit запущен для другой игры (`otherGame: true` в ответе).
    pub fn other_prefix(&self) -> bool {
        self.result
            .as_ref()
            .and_then(|r| r.get("otherGame"))
            .is_some_and(|v| v == &Value::Bool(true))
    }

    pub fn source_tone(&self) -> Tone {
        let status = self.status();
        if status == "ok" && !self.other_prefix() {
            Tone::Ok
        } else if status == "error" || self.other_prefix() {
            Tone::Warn
        } else {
            Tone::Idle
        }
    }

    pub fn source_icon(&self) -> &'static str {
        if self.status() == "ok" && !self.other_prefix() {
            "fa-check-circle"
        } else if self.source_tone() == Tone::Warn {
            "fa-exclamation-triangle"
        } else {
            "fa-info-circle"
        }
    }

    pub fn source_title(&self) -> &'static str {
        if self.capturing && self.result.is_none() {
            return "#L_LSO_SOURCE_CAPTURING";
        }
        if self.other_prefix() {
            return "#L_LSO_SOURCE_OTHER_GAME";
        }
        match self.status().as_str() {
            "ok" => "#L_LSO_SOURCE_LIVE",
            "not-running" => "#L_LSO_SOURCE_SAMPLE",
            "no-window" => "#L_LSO_SOURCE_NO_WINDOW",
            "error" => "#L_LSO_SOURCE_ERROR",
            _ => "#L_LSO_SOURCE_SAMPLE",
        }
    }

    /// `sourceHint`: ключ и аргумент (`W×H` у живого снимка).
    pub fn source_hint(&self) -> (&'static str, Option<String>) {
        if self.other_prefix() {
            return ("#L_LSO_SOURCE_OTHER_GAME_HINT", None);
        }
        match self.status().as_str() {
            "ok" => {
                let result = self.result.as_ref().expect("статус ok");
                let dim = |key: &str| js::to_template_string(result.get(key));
                (
                    "#L_LSO_SOURCE_LIVE_HINT",
                    Some(format!("{}\u{d7}{}", dim("width"), dim("height"))),
                )
            }
            "no-window" => ("#L_LSO_SOURCE_NO_WINDOW_HINT", None),
            "error" => ("#L_LSO_SOURCE_ERROR_HINT", None),
            _ => ("#L_LSO_SOURCE_SAMPLE_HINT", None),
        }
    }

    /// Снимок из ответа `livesplit-overlay-capture` (`pixels` — base64 байтов B, G, R, X;
    /// не `width × height × 4` — снимка нет).
    pub fn snapshot_of(result: &Value) -> Option<Snapshot> {
        if result.get("status").and_then(Value::as_str) != Some("ok") {
            return None;
        }
        let pixels = STANDARD.decode(result.get("pixels")?.as_str()?).ok()?;
        let width = usize::try_from(result.get("width")?.as_u64()?).ok()?;
        let height = usize::try_from(result.get("height")?.as_u64()?).ok()?;
        let size = width.checked_mul(height)?.checked_mul(4)?;
        (size > 0 && pixels.len() == size).then_some(Snapshot {
            pixels,
            width,
            height,
        })
    }

    /// `backgroundColor`: цвет фона LiveSplit из снимка (левый нижний угол).
    pub fn background_color(&self) -> Option<Rgb> {
        self.background
    }

    /// Начало снимка (`capture`): `false` — снимок уже идёт.
    pub fn begin_capture(&mut self, quiet: bool) -> bool {
        if self.capturing {
            return false;
        }
        self.capturing = !quiet;
        true
    }

    /// Ответ снимка: `Some(снимок или None)` — отдать превью; `None` — прежний снимок
    /// остаётся (тихое обновление не нашло окна, а живой снимок уже был).
    pub fn capture_reply(
        &mut self,
        quiet: bool,
        reply: Result<Value, String>,
    ) -> Option<Option<Snapshot>> {
        if self.gone {
            return None;
        }
        let result = match reply {
            Ok(value) => value,
            Err(_) => {
                self.result = Some(json!({"status": "error"}));
                self.background = None;
                return Some(None);
            }
        };
        let ok = result.get("status").and_then(Value::as_str) == Some("ok");
        if quiet && !ok && self.status() == "ok" {
            return None;
        }
        let mut stored = if js::truthy(&result) {
            result
        } else {
            json!({"status": "error"})
        };
        let snapshot = Self::snapshot_of(&stored);
        if let Value::Object(map) = &mut stored {
            map.remove("pixels");
        }
        self.result = Some(stored);
        self.background = snapshot
            .as_ref()
            .and_then(|s| shared::background_color(&s.pixels, s.width, s.height));
        Some(snapshot)
    }

    /// Снимок закончен (после минимального вращения у кнопки).
    pub fn end_capture(&mut self) {
        self.capturing = false;
    }

    /// `changed(key)`: значение отличается от стандартного.
    pub fn changed(&self, key: &str) -> bool {
        let overlay = self.overlay().to_value();
        let defaults = Self::defaults().to_value();
        stringify(overlay.get(key).unwrap_or(&Value::Null), "")
            != stringify(defaults.get(key).unwrap_or(&Value::Null), "")
    }

    /// `set(patch)`: `sanitizeOverlay({...overlay, ...patch})`.
    pub fn set(&self, patch: Map<String, Value>) -> Value {
        let mut next = match self.overlay().to_value() {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        for (key, value) in patch {
            next.insert(key, value);
        }
        livesplit_overlay::sanitize_overlay(&Value::Object(next)).to_value()
    }

    /// `setNumber(field, input)`: `(новое значение, текст поля после)`.
    pub fn set_number(&self, key: &str, text: &str) -> (Value, String) {
        let mut patch = Map::new();
        patch.insert(key.to_owned(), Value::String(text.to_owned()));
        let next = self.set(patch);
        let shown = js::to_string(next.get(key).unwrap_or(&Value::Null));
        (next, shown)
    }

    /// `setCrop(side, input)`.
    pub fn set_crop(&self, side: &str, text: &str) -> (Value, String) {
        let overlay = self.overlay().to_value();
        let mut crop = match overlay.get("crop") {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        };
        crop.insert(side.to_owned(), Value::String(text.to_owned()));
        let mut patch = Map::new();
        patch.insert("crop".into(), Value::Object(crop));
        let next = self.set(patch);
        let shown = js::to_string(
            next.get("crop")
                .and_then(|c| c.get(side))
                .unwrap_or(&Value::Null),
        );
        (next, shown)
    }

    /// `setColor(value)`: `#rrggbb` → цвет ключа.
    pub fn set_color(&self, value: &str) -> Option<Value> {
        let digits = value.strip_prefix('#')?;
        if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).unwrap_or(0);
        let mut patch = Map::new();
        patch.insert(
            "keyColor".into(),
            json!({"r": channel(0), "g": channel(2), "b": channel(4)}),
        );
        Some(self.set(patch))
    }

    /// `hex(color)`.
    pub fn hex(r: u8, g: u8, b: u8) -> String {
        format!("#{r:02x}{g:02x}{b:02x}")
    }

    fn at(v: f64, min: f64, max: f64) -> f64 {
        ((v - min) / (max - min) * 100.0).clamp(0.0, 100.0)
    }

    /// `sliderStyle(key, field)`: без поля — 0…100 (`x`, `y`).
    pub fn slider_style(&self, key: &str, range: Option<(f64, f64)>) -> SliderStyle {
        let (min, max) = range.unwrap_or((0.0, 100.0));
        let overlay = self.overlay().to_value();
        let defaults = Self::defaults().to_value();
        let number = |v: Option<&Value>| v.and_then(Value::as_f64).unwrap_or(f64::NAN);
        let value = Self::at(number(overlay.get(key)), min, max);
        let base = Self::at(number(defaults.get(key)), min, max);
        SliderStyle {
            value,
            default: base,
            from: value.min(base),
            to: value.max(base),
        }
    }

    /// `reset()`: стандартные значения, выключатель остаётся.
    pub fn reset(&self) -> Value {
        let mut next = Self::defaults();
        next.enabled = self.overlay().enabled;
        next.to_value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` пикселей BGRX одного цвета (b 1, g 2, r 3), base64 — как в ответе канала.
    fn bgrx(n: usize) -> String {
        STANDARD.encode([1u8, 2, 3, 0].repeat(n))
    }

    #[test]
    fn the_source_card_follows_the_capture_result() {
        let mut e = LivesplitEditor::new(None, "218", false);
        assert_eq!(e.source_title(), "#L_LSO_SOURCE_SAMPLE");
        assert_eq!(e.source_tone(), Tone::Idle);
        assert!(e.begin_capture(false));
        assert!(e.capturing);
        assert!(!e.begin_capture(true));
        assert_eq!(e.source_title(), "#L_LSO_SOURCE_CAPTURING");
        // BGRX 4 × 4 одного цвета; `backgroundColor` смотрит левый нижний угол шагами по 2 px
        // (у снимка высотой 1 px шагов нет — null, как в 0.4.7).
        let reply = json!({"status": "ok", "width": 4, "height": 4, "pixels": bgrx(16)});
        let emitted = e.capture_reply(false, Ok(reply)).unwrap().unwrap();
        assert_eq!((emitted.width, emitted.height), (4, 4));
        e.end_capture();
        assert_eq!(e.source_tone(), Tone::Ok);
        assert_eq!(e.source_icon(), "fa-check-circle");
        assert_eq!(
            e.source_hint(),
            ("#L_LSO_SOURCE_LIVE_HINT", Some("4\u{d7}4".into()))
        );
        assert_eq!(e.background_color(), Some(Rgb { r: 3, g: 2, b: 1 }));
        assert!(e.result().unwrap().get("pixels").is_none());
        // Байтов не `width × height × 4` или не base64 — снимка нет.
        for pixels in [json!(bgrx(15)), json!("@@"), json!([1, 2, 3, 0])] {
            let bad = json!({"status": "ok", "width": 4, "height": 4, "pixels": pixels});
            assert_eq!(LivesplitEditor::snapshot_of(&bad), None);
        }
        let flat = json!({"status": "ok", "width": 4, "height": 1, "pixels": bgrx(4)});
        assert!(e.capture_reply(false, Ok(flat)).is_some());
        assert_eq!(e.background_color(), None);
        // Тихое обновление без окна — прежний снимок остаётся.
        assert_eq!(
            e.capture_reply(true, Ok(json!({"status": "no-window"}))),
            None
        );
        assert_eq!(e.status(), "ok");
        // Отказ канала — ошибка.
        assert_eq!(e.capture_reply(false, Err("boom".into())), Some(None));
        assert_eq!(e.source_tone(), Tone::Warn);
    }

    #[test]
    fn values_are_sanitized_and_the_reset_keeps_the_switch() {
        let e = LivesplitEditor::new(Some(json!({"enabled": true, "x": 1})), "218", false);
        assert_eq!(e.changes(), 1);
        assert!(e.changed("x") && !e.changed("y"));
        let (next, shown) = e.set_number("height", "zzz");
        assert_eq!(shown, "45");
        assert_eq!(next["height"], 45);
        let (next, shown) = e.set_crop("top", "4000");
        assert_eq!(shown, "2048");
        assert_eq!(next["crop"]["top"], 2048);
        assert_eq!(e.set_color("#12"), None);
        assert_eq!(
            e.set_color("#0102ff").unwrap()["keyColor"],
            json!({"r": 1, "g": 2, "b": 255})
        );
        let reset = e.reset();
        assert_eq!(reset["enabled"], true);
        assert_eq!(reset["x"], 100);
        let style = e.slider_style("x", None);
        assert_eq!(
            (style.value, style.default, style.from, style.to),
            (1.0, 100.0, 1.0, 100.0)
        );
    }
}
