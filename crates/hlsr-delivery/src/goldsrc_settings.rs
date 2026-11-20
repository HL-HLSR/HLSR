//! Настройки кастомизации HL1 (WON 2005 + Bunnymod XT), которые не входят в редактор цвета HUD
//! и прицела (`shared/goldsrc-settings.js`). Формат полей тот же, что у `ghosting_settings`,
//! поэтому окно настроек показывает их тем же списком.
//!
//! Все команды из белого списка правил HL1 на speedrun.com. Значения по умолчанию — как у BXT
//! (cvars.hpp): спидометр у него включён изначально.
//!
//! Таблица полей переписана из `SECTIONS` 0.4.7 вместе с подписями (ключи строк для окна) и
//! подсказками. Набор значений — [`Extras`] (объект JS: порядок ключей — порядок `FIELDS` после
//! `sanitizeExtras`, порядок вставки после `Object.assign`).

pub use crate::ghosting_settings::{ExtraValue, Extras, FieldDefault, FieldOption, FieldType};
use crate::js_value::prop;
use hlsr_core::js::{self, math_round};
use serde_json::{Map, Value};

pub const CHANGER_SHAPES: [&str; 9] = [
    "Default",
    "1px",
    "3px",
    "Small Circle",
    "Medium Circle",
    "Large Circle",
    "Small Circle with 1px",
    "Medium Circle with 1px",
    "Large Circle with 1px",
];
pub const CHANGER_COLORS: [&str; 7] = [
    "green",
    "red",
    "blue",
    "lightblue",
    "orange",
    "pink",
    "yellow",
];

pub const HUD_PACKS: [&str; 2] = ["default", "ag-hud"];
pub const CROSSHAIR_PACKS: [&str; 3] = ["default", "execut4ble-crosshair", "crosshair-changer"];

/// Поле схемы: ключ в настройках, тип, умолчание, подписи и условие показа.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub key: &'static str,
    pub kind: FieldType,
    pub default: FieldDefault,
    /// Ключ строки подписи (`#L_GS_H_…`) или название варианта.
    pub label: &'static str,
    /// Ключ строки подсказки или имя cvar BXT, которое окно показывает как подсказку.
    pub hint: Option<&'static str>,
    /// Поле живо, только пока включено (или равно `requires_value`) это поле.
    pub requires: Option<&'static str>,
    pub requires_value: Option<&'static str>,
}

/// Раздел страницы настроек.
#[derive(Debug, Clone, Copy)]
pub struct Section {
    pub id: &'static str,
    /// Ключ строки заголовка.
    pub title: &'static str,
    pub icon: &'static str,
    /// Ссылка раздела (`LINKS`): страница с ресурсами HL1.
    pub link: Option<&'static str>,
    pub fields: &'static [Field],
}

const fn option(value: &'static str, label: &'static str) -> FieldOption {
    FieldOption { value, label }
}

static PACK_HUD_OPTIONS: [FieldOption; 2] = [
    option("default", "#L_GS_H_PACK_NONE"),
    option("ag-hud", "#L_GS_H_PACK_AG_HUD"),
];
static PACK_CROSSHAIR_OPTIONS: [FieldOption; 3] = [
    option("default", "#L_GS_H_PACK_NONE"),
    option("execut4ble-crosshair", "#L_GS_H_PACK_E4_CROSS"),
    option("crosshair-changer", "#L_GS_H_PACK_CHANGER"),
];
static SHAPE_OPTIONS: [FieldOption; 9] = [
    option("Default", "Default"),
    option("1px", "1px"),
    option("3px", "3px"),
    option("Small Circle", "Small Circle"),
    option("Medium Circle", "Medium Circle"),
    option("Large Circle", "Large Circle"),
    option("Small Circle with 1px", "Small Circle with 1px"),
    option("Medium Circle with 1px", "Medium Circle with 1px"),
    option("Large Circle with 1px", "Large Circle with 1px"),
];
static COLOR_OPTIONS: [FieldOption; 7] = [
    option("green", "#L_GS_H_COLOR_GREEN"),
    option("red", "#L_GS_H_COLOR_RED"),
    option("blue", "#L_GS_H_COLOR_BLUE"),
    option("lightblue", "#L_GS_H_COLOR_LIGHTBLUE"),
    option("orange", "#L_GS_H_COLOR_ORANGE"),
    option("pink", "#L_GS_H_COLOR_PINK"),
    option("yellow", "#L_GS_H_COLOR_YELLOW"),
];

const fn entry(
    key: &'static str,
    kind: FieldType,
    default: FieldDefault,
    label: &'static str,
) -> Field {
    Field {
        key,
        kind,
        default,
        label,
        hint: None,
        requires: None,
        requires_value: None,
    }
}

const fn boolean(key: &'static str, default: bool, label: &'static str) -> Field {
    entry(key, FieldType::Boolean, FieldDefault::Bool(default), label)
}

const fn number(key: &'static str, default: f64, label: &'static str, min: f64, max: f64) -> Field {
    entry(
        key,
        FieldType::Number {
            min,
            max,
            step: 1.0,
        },
        FieldDefault::Number(default),
        label,
    )
}

impl Field {
    const fn hint(mut self, hint: &'static str) -> Field {
        self.hint = Some(hint);
        self
    }

    const fn requires(mut self, key: &'static str) -> Field {
        self.requires = Some(key);
        self
    }

    const fn requires_value(mut self, key: &'static str, value: &'static str) -> Field {
        self.requires = Some(key);
        self.requires_value = Some(value);
        self
    }
}

/// Положение панели BXT на экране: `bxt_hud_<panel>_anchor` — доля ширины и высоты экрана, в
/// процентах она не зависит от разрешения. Сдвиг панели от этой точки (`bxt_hud_<panel>_offset`)
/// остаётся стандартным, BXT сам ставит, например, спидометр на две строки выше точки. По
/// умолчанию — как в BXT (cvars.hpp). Оба cvar разрешены правилами GoldSrc: в whitelist у них
/// нет обязательного значения.
const fn position_x(
    key: &'static str,
    anchor: &'static str,
    panel: &'static str,
    default: f64,
) -> Field {
    number(key, default, "#L_GS_H_POS_X", 0.0, 100.0)
        .hint(anchor)
        .requires(panel)
}

const fn position_y(
    key: &'static str,
    anchor: &'static str,
    panel: &'static str,
    default: f64,
) -> Field {
    number(key, default, "#L_GS_H_POS_Y", 0.0, 100.0)
        .hint(anchor)
        .requires(panel)
}

/// Разделы в порядке `SECTIONS` 0.4.7.
#[rustfmt::skip]
pub static SECTIONS: &[Section] = &[
    Section {
        id: "hl1packs",
        title: "#L_GS_H_S_PACKS",
        icon: "fa-box-open",
        link: Some("hl1Resources"),
        fields: &[
            entry("packHud", FieldType::Select { options: &PACK_HUD_OPTIONS }, FieldDefault::Text("default"), "#L_GS_H_PACK_HUD").hint("#L_GS_H_PACK_HUD_HINT"),
            entry("packCrosshair", FieldType::Select { options: &PACK_CROSSHAIR_OPTIONS }, FieldDefault::Text("default"), "#L_GS_H_PACK_CROSS").hint("#L_GS_H_PACK_CROSS_HINT"),
            entry("changerShape", FieldType::Select { options: &SHAPE_OPTIONS }, FieldDefault::Text("Default"), "#L_GS_H_CHANGER_SHAPE").requires_value("packCrosshair", "crosshair-changer"),
            entry("changerColor", FieldType::Select { options: &COLOR_OPTIONS }, FieldDefault::Text("green"), "#L_GS_H_CHANGER_COLOR").requires_value("packCrosshair", "crosshair-changer"),
        ],
    },
    Section {
        id: "hl1hud",
        title: "#L_GS_H_S_HUD",
        icon: "fa-stopwatch",
        link: None,
        fields: &[
            boolean("timer", false, "#L_GS_H_TIMER").hint("bxt_hud_timer"),
            position_x("timerX", "bxt_hud_timer_anchor", "timer", 0.0),
            position_y("timerY", "bxt_hud_timer_anchor", "timer", 50.0),
            boolean("speedometer", true, "#L_GS_H_SPEEDOMETER").hint("bxt_hud_speedometer"),
            position_x("speedometerX", "bxt_hud_speedometer_anchor", "speedometer", 50.0),
            position_y("speedometerY", "bxt_hud_speedometer_anchor", "speedometer", 100.0),
            boolean("jumpspeed", false, "#L_GS_H_JUMPSPEED").hint("bxt_hud_jumpspeed"),
            position_x("jumpspeedX", "bxt_hud_jumpspeed_anchor", "jumpspeed", 50.0),
            position_y("jumpspeedY", "bxt_hud_jumpspeed_anchor", "jumpspeed", 100.0),
            boolean("viewangles", false, "#L_GS_H_VIEWANGLES").hint("bxt_hud_viewangles"),
            position_x("viewanglesX", "bxt_hud_viewangles_anchor", "viewangles", 100.0),
            position_y("viewanglesY", "bxt_hud_viewangles_anchor", "viewangles", 0.0),
            boolean("jumpdistance", false, "#L_GS_H_JUMPDISTANCE").hint("bxt_hud_jumpdistance"),
            number("precision", 6.0, "#L_GS_H_PRECISION", 0.0, 16.0).hint("#L_GS_H_PRECISION_HINT"),
            boolean("incorrectFps", true, "#L_GS_H_BAD_FPS").hint("#L_GS_H_BAD_FPS_HINT"),
        ],
    },
    Section {
        id: "hl1view",
        title: "#L_GS_H_S_VIEW",
        icon: "fa-eye",
        link: None,
        fields: &[
            number("fov", 90.0, "#L_GS_H_FOV", 70.0, 120.0).hint("default_fov"),
            boolean("fixWidescreenFov", false, "#L_GS_H_WIDESCREEN").hint("#L_GS_H_WIDESCREEN_HINT"),
            number("viewmodelFov", 0.0, "#L_GS_H_VM_FOV", 0.0, 130.0).hint("#L_GS_H_VM_FOV_HINT"),
            boolean("viewmodelSemitransparent", false, "#L_GS_H_VM_SEMI").hint("bxt_viewmodel_semitransparent"),
            boolean("removeViewmodel", false, "#L_GS_H_VM_REMOVE").hint("bxt_remove_viewmodel"),
            number("viewmodelForward", 0.0, "#L_GS_H_VM_FORWARD", -20.0, 20.0).hint("bxt_viewmodel_ofs_forward"),
            number("viewmodelRight", 0.0, "#L_GS_H_VM_RIGHT", -20.0, 20.0).hint("bxt_viewmodel_ofs_right"),
            number("viewmodelUp", 0.0, "#L_GS_H_VM_UP", -20.0, 20.0).hint("bxt_viewmodel_ofs_up"),
            boolean("viewmodelBobAngled", false, "#L_GS_H_VM_BOB").hint("bxt_viewmodel_bob_angled"),
            boolean("viewmodelNoIdle", false, "#L_GS_H_VM_NO_IDLE").hint("bxt_viewmodel_disable_idle"),
            boolean("viewmodelNoEquip", false, "#L_GS_H_VM_NO_EQUIP").hint("bxt_viewmodel_disable_equip"),
            boolean("hideCrosshair", false, "#L_GS_H_HIDE_CROSS").hint("#L_GS_H_HIDE_CROSS_HINT"),
        ],
    },
];

/// `FIELDS`: все поля по порядку разделов.
pub fn fields() -> impl Iterator<Item = &'static Field> {
    SECTIONS.iter().flat_map(|section| section.fields.iter())
}

/// `BY_KEY[key]`.
pub fn field(key: &str) -> Option<&'static Field> {
    fields().find(|field| field.key == key)
}

/// `BXT_KEYS`: поля, которые пишет cfg (всё, кроме паков: `/^pack|^changer/`).
pub fn bxt_keys() -> impl Iterator<Item = &'static str> {
    fields()
        .map(|field| field.key)
        .filter(|key| !key.starts_with("pack") && !key.starts_with("changer"))
}

/// `defaults()`: значения по умолчанию всех полей.
pub fn defaults() -> Extras {
    let mut result = Extras::new();
    for field in fields() {
        result.insert(field.key, field.default.to_extra());
    }
    result
}

/// Поле с `requiresValue` живо, только пока у поля `requires` именно это значение: форма и цвет из
/// Crosshair Changer ничего не значат для других прицелов. Остальные зависимые поля живы, пока
/// значение зависимости истинно и не `"0"`. `values` — плоский объект настроек окна; ключа нет —
/// берётся умолчание зависимости.
pub fn available(item: &Field, values: &Value) -> bool {
    let Some(requires) = item.requires else {
        return true;
    };
    let value = match values.as_object().and_then(|object| object.get(requires)) {
        Some(value) => value.clone(),
        None => field(requires)
            .expect("зависимость есть в схеме")
            .default
            .to_extra()
            .to_value(),
    };
    match item.requires_value {
        Some(expected) => value.as_str() == Some(expected),
        None => js::truthy(&value) && value.as_str() != Some("0"),
    }
}

/// `sanitizeValue(field, value)`: `None` — значение не годится (`undefined`).
fn sanitize_value(field: &Field, value: &Value) -> Option<ExtraValue> {
    match field.kind {
        FieldType::Boolean => value.as_bool().map(ExtraValue::Bool),
        FieldType::Number { min, max, .. } => {
            let numeric = match value {
                Value::String(text) => {
                    if js::trim(text).is_empty() {
                        return None;
                    }
                    js::to_number(text)
                }
                Value::Number(_) => crate::js_value::number(value)?,
                _ => return None,
            };
            if !numeric.is_finite() {
                return None;
            }
            Some(ExtraValue::Number(math_round(min.max(max.min(numeric)))))
        }
        FieldType::Select { options } => {
            let text = value.as_str()?;
            options
                .iter()
                .any(|option| option.value == text)
                .then(|| ExtraValue::Text(text.to_owned()))
        }
        FieldType::Color | FieldType::Text { .. } => None,
    }
}

/// `sanitizeExtras(input)`: оставляет известные поля с годными значениями, в порядке `FIELDS`.
pub fn sanitize_extras(input: &Value) -> Extras {
    let mut result = Extras::new();
    let Value::Object(object) = input else {
        return result;
    };
    for field in fields() {
        let Some(value) = object.get(field.key) else {
            continue;
        };
        if let Some(clean) = sanitize_value(field, value) {
            result.insert(field.key, clean);
        }
    }
    result
}

/// `resetExtras(input)`: умолчания тех полей, которые есть во входе.
pub fn reset_extras(input: &Value) -> Extras {
    let mut result = Extras::new();
    for (key, _) in sanitize_extras(input).iter() {
        let field = field(key).expect("ключ из схемы");
        result.insert(key, field.default.to_extra());
    }
    result
}

/// Выбор паков и Crosshair Changer (`packs` в настройках игры).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackSelection {
    pub hud: String,
    pub crosshair: String,
    pub changer_shape: String,
    pub changer_color: String,
}

impl PackSelection {
    /// Объект `{ hud, crosshair, changerShape, changerColor }` в порядке ключей 0.4.7.
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("hud".into(), Value::String(self.hud.clone()));
        object.insert("crosshair".into(), Value::String(self.crosshair.clone()));
        object.insert(
            "changerShape".into(),
            Value::String(self.changer_shape.clone()),
        );
        object.insert(
            "changerColor".into(),
            Value::String(self.changer_color.clone()),
        );
        Value::Object(object)
    }
}

/// `fromFlat(flat)`: настройки хранятся двумя полями — `packs` (его читает установщик паков) и
/// `bxt` (его пишет cfg).
#[derive(Debug, Clone, PartialEq)]
pub struct Flat {
    pub packs: PackSelection,
    pub bxt: Extras,
}

impl Flat {
    /// Объект `{ packs, bxt }` в порядке ключей 0.4.7.
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("packs".into(), self.packs.to_value());
        object.insert("bxt".into(), self.bxt.to_value());
        Value::Object(object)
    }
}

/// `toFlat(packs, bxt)`: в окне настроек паки и BXT — один плоский список.
pub fn to_flat(packs: &Value, bxt: &Value) -> Extras {
    let mut result = sanitize_extras(bxt);
    // `packs || {}`: ложное значение даёт пустой объект; поля берутся только у объектов.
    let source = if js::truthy(packs) {
        packs
    } else {
        &Value::Null
    };
    let take = |name: &str| prop(source, name).cloned().unwrap_or(Value::Null);
    let mut picked = Map::new();
    picked.insert("packHud".into(), take("hud"));
    picked.insert("packCrosshair".into(), take("crosshair"));
    picked.insert("changerShape".into(), take("changerShape"));
    picked.insert("changerColor".into(), take("changerColor"));
    result.assign(&sanitize_extras(&Value::Object(picked)));
    result
}

/// Выбор паков и поля BXT из нормализованных значений (после `Object.assign(defaults(), …)`).
pub fn from_extras(clean: &Extras) -> Flat {
    let mut all = defaults();
    all.assign(clean);
    let text = |key: &str| match all.get(key) {
        Some(ExtraValue::Text(text)) => text.clone(),
        other => unreachable!("поле {key} — выбор из списка, а не {other:?}"),
    };
    let mut bxt = Extras::new();
    for key in bxt_keys() {
        bxt.insert(
            key,
            all.get(key).expect("умолчание есть у каждого поля").clone(),
        );
    }
    Flat {
        packs: PackSelection {
            hud: text("packHud"),
            crosshair: text("packCrosshair"),
            changer_shape: text("changerShape"),
            changer_color: text("changerColor"),
        },
        bxt,
    }
}

/// `fromFlat(flat)`.
pub fn from_flat(flat: &Value) -> Flat {
    from_extras(&sanitize_extras(flat))
}
