//! Редактор HUD и прицела «Персонализации» (`GhostingPersonalization.vue` 0.4.7, задача 32):
//! палитра, свой цвет, HSV-круг (курсор, перетаскивание, клавиши), HEX и каналы RGB,
//! прозрачность; пресеты прицела, цвет, ползунки, «тонкая настройка». Без окна: состояние и
//! чистые функции цвета (те же формулы, что в 0.4.7, — сверка `tests/personalization_logic.rs`
//! по фикстуре `export-personalization-logic.js`). Изменения уходят наружу значением
//! `Object.assign({}, value, patch)`, как `$emit('input', …)`.

use super::jsv;
use hlsr_core::js::{self, math_round};
use hlsr_delivery::ghosting_personalization::{CROSS_PRESETS, HUD_PRESETS};
use serde_json::{Map, Value, json};

/// Цвет HUD или прицела: каналы — числа JS (после `Object.assign` могут быть не целыми).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

impl Rgb {
    pub const fn new(r: f64, g: f64, b: f64) -> Rgb {
        Rgb { r, g, b }
    }

    /// Каналы из объекта `{r, g, b}`; нет объекта или канала — `NaN` (как `Number(undefined)`).
    pub fn from_value(value: Option<&Value>) -> Rgb {
        let channel = |name: &str| value.and_then(|v| v.get(name)).map_or(f64::NAN, number_of);
        Rgb {
            r: channel("r"),
            g: channel("g"),
            b: channel("b"),
        }
    }

    /// `{r, g, b}` как в JSON рендерера: целые каналы — без дробной части (`255`, не `255.0`).
    pub fn to_value(self) -> Value {
        json!({
            "r": jsv::number_value(self.r),
            "g": jsv::number_value(self.g),
            "b": jsv::number_value(self.b),
        })
    }
}

/// `Number(value)` для значения JSON.
fn number_of(value: &Value) -> f64 {
    match value {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(text) => js::to_number(text),
        Value::Bool(true) => 1.0,
        Value::Bool(false) | Value::Null => 0.0,
        _ => f64::NAN,
    }
}

/// Оттенок и насыщенность курсора круга.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wheel {
    pub h: f64,
    pub s: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsv {
    pub h: f64,
    pub s: f64,
    pub v: f64,
}

/// Цвет палитры (`HUD_PRESET_LIST`): id, цвет, подпись «RGB r g b».
#[derive(Debug, Clone, PartialEq)]
pub struct Swatch {
    pub id: &'static str,
    pub color: Rgb,
    pub label: String,
}

/// Пресеты прицела (`CROSS_PRESET_IDS`): id и ключ подписи.
pub const CROSS_PRESET_IDS: [(&str, &str); 5] = [
    ("standard", "#L_GS_P_PR_STD"),
    ("dot", "#L_GS_P_PR_DOT"),
    ("classic", "#L_GS_P_PR_CLASSIC"),
    ("circle", "#L_GS_P_PR_CIRCLE"),
    ("crossdot", "#L_GS_P_PR_CROSSDOT"),
];

/// Палитра окна (`HUD_PRESET_LIST` с подписью `RGB r g b`).
pub fn swatches() -> Vec<Swatch> {
    HUD_PRESETS
        .iter()
        .map(|(id, c)| Swatch {
            id,
            color: Rgb::new(f64::from(c.r), f64::from(c.g), f64::from(c.b)),
            label: format!("RGB {} {} {}", c.r, c.g, c.b),
        })
        .collect()
}

/// `clamp(v, a, b)`: `Math.round(Number(v))`, не число — `a`.
pub fn clamp(v: f64, a: i64, b: i64) -> i64 {
    let rounded = math_round(v);
    if !rounded.is_finite() {
        return a;
    }
    (rounded as i64).clamp(a, b)
}

fn to_hex(n: f64) -> String {
    format!("{:02x}", clamp(n, 0, 255))
}

/// `rgbToHex(c)`: `#rrggbb` строчными.
pub fn rgb_to_hex(c: Rgb) -> String {
    format!("#{}{}{}", to_hex(c.r), to_hex(c.g), to_hex(c.b))
}

/// `hexToRgb(hex)`: `#?rrggbb` без учёта регистра и с пробелами по краям; иначе `None`.
pub fn hex_to_rgb(hex: &str) -> Option<Rgb> {
    let text = js::trim(hex);
    let digits = text.strip_prefix('#').unwrap_or(text);
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| f64::from(u8::from_str_radix(&digits[at..at + 2], 16).unwrap_or(0));
    Some(Rgb::new(channel(0), channel(2), channel(4)))
}

/// `rgbToHsv(c)`.
pub fn rgb_to_hsv(c: Rgb) -> Hsv {
    let (r, g, b) = (c.r / 255.0, c.g / 255.0, c.b / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let mut h = 0.0;
    if d != 0.0 {
        if max == r {
            h = ((g - b) / d) % 6.0;
        } else if max == g {
            h = (b - r) / d + 2.0;
        } else {
            h = (r - g) / d + 4.0;
        }
        h *= 60.0;
        if h < 0.0 {
            h += 360.0;
        }
    }
    let s = if max == 0.0 { 0.0 } else { d / max };
    Hsv { h, s, v: max }
}

/// `hsvToRgb(h, s, v)`.
pub fn hsv_to_rgb(h: f64, s: f64, v: f64) -> Rgb {
    let c = v * s;
    let x = c * (1.0 - (((h / 60.0) % 2.0) - 1.0).abs());
    let (r1, g1, b1) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    let m = v - c;
    Rgb::new(
        math_round((r1 + m) * 255.0),
        math_round((g1 + m) * 255.0),
        math_round((b1 + m) * 255.0),
    )
}

/// Значения пресета прицела (`CROSS_PRESET_VALUES[id]`) в порядке ключей 0.4.7.
pub fn cross_preset_values(id: &str) -> Option<Map<String, Value>> {
    let (_, preset) = CROSS_PRESETS.iter().find(|(name, _)| *name == id)?;
    let p = (*preset)?;
    let color = |c: hlsr_delivery::ghosting_settings::Color| json!({"r": c.r, "g": c.g, "b": c.b});
    let mut map = Map::new();
    map.insert("crossColor".into(), color(p.cross_color));
    map.insert("crossAlpha".into(), json!(p.cross_alpha));
    map.insert("crossThickness".into(), json!(p.thickness));
    map.insert("crossSize".into(), json!(p.size));
    map.insert("crossGap".into(), json!(p.gap));
    map.insert("crossOutline".into(), json!(p.outline != 0));
    map.insert("crossOutlineThickness".into(), json!(p.outline_thickness));
    map.insert("crossCircleRadius".into(), json!(p.circle_radius));
    map.insert(
        "crossDotColor".into(),
        p.dot_color.map_or(Value::Null, color),
    );
    map.insert("crossDotSize".into(), json!(p.dot_size));
    map.insert("crossTopLine".into(), json!(p.top_line));
    map.insert("crossBottomLine".into(), json!(p.bottom_line));
    map.insert("crossLeftLine".into(), json!(p.left_line));
    map.insert("crossRightLine".into(), json!(p.right_line));
    map.insert("crossHideDefault".into(), json!(p.hide_default));
    Some(map)
}

/// Состояние редактора (`data()` и props).
#[derive(Debug, Clone)]
pub struct HudEditor {
    value: Map<String, Value>,
    pub goldsrc: bool,
    /// `activePanel`: `hud` или `cross`.
    pub active_panel: &'static str,
    pub show_advanced: bool,
    pub wheel: Wheel,
    wheel_drag: bool,
}

fn wheel_of(color: Rgb) -> Wheel {
    let hsv = rgb_to_hsv(color);
    Wheel {
        h: hsv.h,
        s: hsv.s.max(0.05),
    }
}

/// Оттенок и насыщенность с нижним пределом насыщенности 0,02 (курсор не уходит в центр).
fn floor_s(s: f64) -> f64 {
    s.max(0.02)
}

impl HudEditor {
    pub fn new(value: Value, goldsrc: bool) -> HudEditor {
        let value = match value {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        let wheel = wheel_of(Rgb::from_value(value.get("hudColor")));
        HudEditor {
            value,
            goldsrc,
            active_panel: "hud",
            show_advanced: false,
            wheel,
            wheel_drag: false,
        }
    }

    pub fn value(&self) -> &Map<String, Value> {
        &self.value
    }

    /// `watch value (deep)`: новое значение снаружи; курсор круга следует за цветом, пока его
    /// не тянут.
    pub fn set_value(&mut self, value: Value) {
        if let Value::Object(map) = value {
            self.value = map;
        }
        if !self.wheel_drag {
            self.wheel = wheel_of(self.hud_color());
        }
    }

    pub fn hud_color(&self) -> Rgb {
        Rgb::from_value(self.value.get("hudColor"))
    }

    pub fn cross_color(&self) -> Rgb {
        Rgb::from_value(self.value.get("crossColor"))
    }

    fn flag(&self, key: &str) -> bool {
        self.value.get(key).is_some_and(js::truthy)
    }

    pub fn hud_enabled(&self) -> bool {
        self.flag("hudEnabled")
    }

    pub fn cross_enabled(&self) -> bool {
        self.flag("crossEnabled")
    }

    pub fn hud_affects_crosshair(&self) -> bool {
        self.flag("hudAffectsCrosshair")
    }

    pub fn hide_quick_info(&self) -> bool {
        self.flag("hideQuickInfo")
    }

    pub fn cross_preset(&self) -> String {
        self.value
            .get("crossPreset")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    }

    /// Число поля (`value.crossSize` и т. п.); нет — `NaN`.
    pub fn number(&self, key: &str) -> f64 {
        self.value.get(key).map_or(f64::NAN, number_of)
    }

    pub fn bool(&self, key: &str) -> bool {
        self.flag(key)
    }

    pub fn hud_hex(&self) -> String {
        rgb_to_hex(self.hud_color())
    }

    /// `crossHex`: цвет прицела или цвет HUD, когда «цвет прицела как у HUD».
    pub fn cross_hex(&self) -> String {
        rgb_to_hex(if self.hud_affects_crosshair() {
            self.hud_color()
        } else {
            self.cross_color()
        })
    }

    pub fn cross_dot_color(&self) -> Option<Rgb> {
        self.value
            .get("crossDotColor")
            .filter(|v| js::truthy(v))
            .map(|v| Rgb::from_value(Some(v)))
    }

    pub fn dot_hex(&self) -> String {
        match self.cross_dot_color() {
            Some(color) => rgb_to_hex(color),
            None => self.cross_hex(),
        }
    }

    /// `wheelCursorStyle`: центр курсора в долях коробки круга (`left`, `top`, 0…1).
    pub fn wheel_cursor(&self) -> (f64, f64) {
        let rad = self.wheel.h * std::f64::consts::PI / 180.0;
        (
            (50.0 + 42.0 * self.wheel.s * rad.cos()) / 100.0,
            (50.0 + 42.0 * self.wheel.s * rad.sin()) / 100.0,
        )
    }

    /// `hudAlphaLabel`: `None` — «Авто» (`#L_GS_P_AUTO`), иначе число строкой.
    pub fn hud_alpha_label(&self) -> Option<String> {
        let alpha = self.number("hudAlpha");
        let auto = if self.goldsrc {
            alpha <= 0.0
        } else {
            alpha == -1.0
        };
        if auto {
            None
        } else {
            Some(js_number_text(alpha))
        }
    }

    /// `emit(patch)`: `Object.assign({}, value, patch)`.
    fn emit(&self, patch: Map<String, Value>) -> Value {
        let mut next = self.value.clone();
        for (key, value) in patch {
            next.insert(key, value);
        }
        Value::Object(next)
    }

    /// `set(key, val)`: поле прицела при включённом прицеле делает пресет «свой».
    pub fn set(&self, key: &str, val: Value) -> Value {
        let mut patch = Map::new();
        patch.insert(key.to_owned(), val);
        if key.starts_with("cross") && key != "crossEnabled" && self.cross_enabled() {
            patch.insert("crossPreset".into(), json!("custom"));
        }
        self.emit(patch)
    }

    pub fn is_hud_preset(&self, p: &Swatch) -> bool {
        self.hud_color() == p.color
    }

    pub fn is_cross_color(&self, p: &Swatch) -> bool {
        self.cross_color() == p.color
    }

    pub fn set_hud_preset(&self, p: &Swatch) -> Value {
        let mut patch = Map::new();
        patch.insert("hudColor".into(), p.color.to_value());
        patch.insert("hudEnabled".into(), json!(true));
        self.emit(patch)
    }

    pub fn on_hud_hex(&self, hex: &str) -> Option<Value> {
        let c = hex_to_rgb(hex)?;
        let mut patch = Map::new();
        patch.insert("hudColor".into(), c.to_value());
        patch.insert("hudEnabled".into(), json!(true));
        Some(self.emit(patch))
    }

    /// `onHudChannel(ch, val)`: канал `r`/`g`/`b` из текста поля.
    pub fn on_hud_channel(&self, ch: &str, val: &str) -> Value {
        let mut color = match self.value.get("hudColor") {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        };
        color.insert(ch.to_owned(), json!(clamp(js::to_number(val), 0, 255)));
        let mut patch = Map::new();
        patch.insert("hudColor".into(), Value::Object(color));
        patch.insert("hudEnabled".into(), json!(true));
        self.emit(patch)
    }

    pub fn set_cross_color(&self, p: &Swatch) -> Value {
        let mut patch = Map::new();
        patch.insert("crossColor".into(), p.color.to_value());
        patch.insert("crossPreset".into(), json!("custom"));
        patch.insert("crossEnabled".into(), json!(true));
        self.emit(patch)
    }

    pub fn on_cross_hex(&self, hex: &str) -> Option<Value> {
        let c = hex_to_rgb(hex)?;
        let mut patch = Map::new();
        patch.insert("crossColor".into(), c.to_value());
        patch.insert("crossPreset".into(), json!("custom"));
        patch.insert("crossEnabled".into(), json!(true));
        Some(self.emit(patch))
    }

    /// `applyCrossPreset(id)`: `None` — неизвестный пресет (ничего не меняется).
    pub fn apply_cross_preset(&self, id: &str) -> Option<Value> {
        if id == "standard" {
            let mut patch = Map::new();
            patch.insert("crossPreset".into(), json!("standard"));
            patch.insert("crossEnabled".into(), json!(false));
            return Some(self.emit(patch));
        }
        let values = cross_preset_values(id)?;
        let mut patch = Map::new();
        patch.insert("crossPreset".into(), json!(id));
        patch.insert("crossEnabled".into(), json!(true));
        for (key, value) in values {
            patch.insert(key, value);
        }
        Some(self.emit(patch))
    }

    /// `toggleCrossEnabled(enabled)`: включение со «стандартного» — пресет «классика».
    pub fn toggle_cross_enabled(&self, enabled: bool) -> Value {
        if enabled && self.cross_preset() == "standard" {
            return self
                .apply_cross_preset("classic")
                .expect("пресет classic есть");
        }
        self.set("crossEnabled", json!(enabled))
    }

    /// Активен ли пресет в списке: «стандартный» — пока прицел выключен, прочие — по имени.
    pub fn cross_preset_active(&self, id: &str) -> bool {
        if id == "standard" {
            !self.cross_enabled()
        } else {
            self.cross_enabled() && self.cross_preset() == id
        }
    }

    /// `wheelToColor(clientX, clientY)`: точка в коробке круга `width × height`.
    pub fn wheel_to_color(&mut self, x: f64, y: f64, width: f64, height: f64) -> Value {
        let dx = x - width / 2.0;
        let dy = y - height / 2.0;
        let h = (dy.atan2(dx) * 180.0 / std::f64::consts::PI + 360.0) % 360.0;
        let s = (dx.hypot(dy) / (width * 0.48)).min(1.0);
        self.wheel = Wheel { h, s: floor_s(s) };
        self.emit_wheel(h, floor_s(s))
    }

    fn emit_wheel(&self, h: f64, s: f64) -> Value {
        let v = rgb_to_hsv(self.hud_color()).v.max(0.7);
        let mut patch = Map::new();
        patch.insert("hudColor".into(), hsv_to_rgb(h, s, v).to_value());
        patch.insert("hudEnabled".into(), json!(true));
        self.emit(patch)
    }

    pub fn on_wheel_down(&mut self, x: f64, y: f64, width: f64, height: f64) -> Value {
        self.wheel_drag = true;
        self.wheel_to_color(x, y, width, height)
    }

    pub fn on_wheel_move(&mut self, x: f64, y: f64, width: f64, height: f64) -> Option<Value> {
        self.wheel_drag
            .then(|| self.wheel_to_color(x, y, width, height))
    }

    pub fn on_wheel_up(&mut self) {
        self.wheel_drag = false;
    }

    pub fn wheel_dragging(&self) -> bool {
        self.wheel_drag
    }

    /// `onWheelKey`: стрелки — оттенок ±3°, насыщенность ±0,03; иное — `None` (не обработано).
    pub fn on_wheel_key(&mut self, key: &str) -> Option<Value> {
        let Wheel { mut h, mut s } = self.wheel;
        match key {
            "ArrowLeft" => h = (h - 3.0 + 360.0) % 360.0,
            "ArrowRight" => h = (h + 3.0) % 360.0,
            "ArrowUp" => s = (s + 0.03).min(1.0),
            "ArrowDown" => s = (s - 0.03).max(0.0),
            _ => return None,
        }
        self.wheel = Wheel { h, s: floor_s(s) };
        Some(self.emit_wheel(h, floor_s(s)))
    }
}

/// `String(x)` числа JS.
fn js_number_text(x: f64) -> String {
    if x.is_nan() {
        "NaN".to_owned()
    } else if x.is_infinite() {
        if x > 0.0 { "Infinity" } else { "-Infinity" }.to_owned()
    } else {
        hlsr_core::json_text::js_number(x).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_round_trip_like_the_renderer() {
        assert_eq!(rgb_to_hex(Rgb::new(255.0, 220.0, 0.0)), "#ffdc00");
        assert_eq!(rgb_to_hex(Rgb::new(300.0, -5.0, 12.6)), "#ff000d");
        assert_eq!(hex_to_rgb(" #0A0b0C "), Some(Rgb::new(10.0, 11.0, 12.0)));
        assert_eq!(hex_to_rgb("abcdef"), Some(Rgb::new(171.0, 205.0, 239.0)));
        assert_eq!(hex_to_rgb("#12"), None);
        let hsv = rgb_to_hsv(Rgb::new(255.0, 220.0, 0.0));
        assert!((hsv.h - 51.76470588235294).abs() < 1e-12);
        assert_eq!(hsv.s, 1.0);
        assert_eq!(hsv.v, 1.0);
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), Rgb::new(255.0, 0.0, 0.0));
        assert_eq!(hsv_to_rgb(120.0, 1.0, 0.5), Rgb::new(0.0, 128.0, 0.0));
        assert_eq!(clamp(12.5, 0, 255), 13);
        assert_eq!(clamp(f64::NAN, 0, 255), 0);
        assert_eq!(clamp(300.0, 0, 255), 255);
    }

    #[test]
    fn cross_fields_switch_the_preset_to_custom_only_while_enabled() {
        let mut value =
            hlsr_delivery::ghosting_personalization::default_personalization().to_value();
        let editor = HudEditor::new(value.clone(), false);
        let next = editor.set("crossSize", json!(12));
        assert_eq!(next["crossSize"], 12);
        assert_eq!(next["crossPreset"], "standard");
        value["crossEnabled"] = json!(true);
        value["crossPreset"] = json!("classic");
        let editor = HudEditor::new(value, false);
        let next = editor.set("crossSize", json!(12));
        assert_eq!(next["crossPreset"], "custom");
        let next = editor.set("crossEnabled", json!(false));
        assert_eq!(next["crossPreset"], "classic");
        let next = editor.set("hudEnabled", json!(true));
        assert_eq!(next["crossPreset"], "classic");
    }

    #[test]
    fn the_wheel_follows_the_colour_until_dragged() {
        let value = hlsr_delivery::ghosting_personalization::default_personalization().to_value();
        let mut editor = HudEditor::new(value, false);
        assert_eq!(editor.wheel.s, 1.0);
        let next = editor.on_wheel_down(54.0, 54.0, 108.0, 108.0);
        assert_eq!(editor.wheel, Wheel { h: 0.0, s: 0.02 });
        assert_eq!(next["hudEnabled"], true);
        // Пока тянут, цвет снаружи курсор не двигает.
        let mut outside = next.clone();
        outside["hudColor"] = json!({"r": 0, "g": 255, "b": 0});
        editor.set_value(outside.clone());
        assert_eq!(editor.wheel, Wheel { h: 0.0, s: 0.02 });
        editor.on_wheel_up();
        editor.set_value(outside);
        assert_eq!(editor.wheel.h, 120.0);
        assert_eq!(editor.on_wheel_key("Enter"), None);
    }
}
