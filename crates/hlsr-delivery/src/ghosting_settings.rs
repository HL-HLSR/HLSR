//! Настройки Ghosting Mod Renewed, которыми управляет лаунчер (`shared/ghosting-settings.js`):
//! схема полей, нормализация, запись в cfg и разбор обратно.
//!
//! Таблица полей переписана из `SECTIONS` 0.4.7 вместе с подписями (ключи строк для окна),
//! подсказками, единицами и шкалами: окно рисует страницу по этой же схеме. Значения полей —
//! [`ExtraValue`], набор значений — [`Extras`] (объект JS: порядок ключей — порядок `FIELDS`
//! после `sanitizeExtras`, порядок вставки после `importExtras`).

use hlsr_core::js::object::JsObject;
use hlsr_core::js::{self, is_whitespace};
use hlsr_core::json_text::js_number;
use regress::Regex;
use serde_json::{Map, Value};
use std::sync::LazyLock;
use url::Url;

/// Цвет `{ r, g, b }`, каждый канал 0–255.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    /// Объект JS `{ r, g, b }`.
    pub fn to_value(self) -> Value {
        let mut object = Map::new();
        object.insert("r".into(), Value::from(self.r));
        object.insert("g".into(), Value::from(self.g));
        object.insert("b".into(), Value::from(self.b));
        Value::Object(object)
    }

    /// `"r g b"` — как значение цвета в cfg.
    pub fn to_cfg(self) -> String {
        format!("{} {} {}", self.r, self.g, self.b)
    }
}

/// Значение поля после нормализации.
#[derive(Debug, Clone, PartialEq)]
pub enum ExtraValue {
    Bool(bool),
    Number(f64),
    /// Текст и значение выпадающего списка (строка).
    Text(String),
    Color(Color),
}

impl ExtraValue {
    /// Значение для JSON: числа — в записи JS.
    pub fn to_value(&self) -> Value {
        match self {
            ExtraValue::Bool(value) => Value::Bool(*value),
            ExtraValue::Number(value) => js_number(*value),
            ExtraValue::Text(value) => Value::String(value.clone()),
            ExtraValue::Color(color) => color.to_value(),
        }
    }

    /// `String(value)`: так значение попадает в cfg.
    fn to_js_string(&self) -> String {
        match self {
            ExtraValue::Bool(value) => value.to_string(),
            ExtraValue::Number(value) => js::to_string(&js_number(*value)),
            ExtraValue::Text(value) => value.clone(),
            ExtraValue::Color(_) => "[object Object]".to_owned(),
        }
    }
}

/// Набор значений полей (`extra` в настройках): объект JS со значением поля по ключу.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extras(JsObject<ExtraValue>);

impl Extras {
    pub fn new() -> Extras {
        Extras::default()
    }

    pub fn get(&self, key: &str) -> Option<&ExtraValue> {
        self.0.get(key)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    /// `extras[key] = value` (у существующего ключа место прежнее).
    pub fn insert(&mut self, key: &str, value: ExtraValue) {
        self.0.insert(key, value);
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Записи в порядке `Object.entries`.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ExtraValue)> {
        self.0.iter()
    }

    /// `Object.assign(this, other)`: чужие значения поверх своих, новые ключи — в конец.
    pub fn assign(&mut self, other: &Extras) {
        for (key, value) in other.iter() {
            self.0.insert(key, value.clone());
        }
    }

    /// Объект JSON для файла настроек.
    pub fn to_value(&self) -> Value {
        Value::Object(
            self.iter()
                .map(|(key, value)| (key.to_owned(), value.to_value()))
                .collect(),
        )
    }
}

/// Вариант выпадающего списка.
#[derive(Debug, Clone, Copy)]
pub struct FieldOption {
    pub value: &'static str,
    pub label: &'static str,
}

/// Тип поля и его границы.
#[derive(Debug, Clone, Copy)]
pub enum FieldType {
    Boolean,
    Number { min: f64, max: f64, step: f64 },
    Select { options: &'static [FieldOption] },
    Color,
    Text { max_length: usize },
}

/// Значение по умолчанию.
#[derive(Debug, Clone, Copy)]
pub enum FieldDefault {
    Bool(bool),
    Number(f64),
    Text(&'static str),
    Color(Color),
}

impl FieldDefault {
    pub fn to_extra(self) -> ExtraValue {
        match self {
            FieldDefault::Bool(value) => ExtraValue::Bool(value),
            FieldDefault::Number(value) => ExtraValue::Number(value),
            FieldDefault::Text(value) => ExtraValue::Text(value.to_owned()),
            FieldDefault::Color(color) => ExtraValue::Color(color),
        }
    }
}

/// Поле схемы.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub key: &'static str,
    pub kind: FieldType,
    pub default: FieldDefault,
    /// Ключ строки подписи (`#L_GS_X_…`).
    pub label: &'static str,
    pub hint: Option<&'static str>,
    /// Поле работает в игре, только пока включено это (цепочка).
    pub requires: Option<&'static str>,
    pub unit: Option<&'static str>,
    /// Подписи концов шкалы ползунка.
    pub scale: Option<[&'static str; 2]>,
    /// Значение — команда игры, а не значение переменной.
    pub command: bool,
}

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
        unit: None,
        scale: None,
        command: false,
    }
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

    const fn unit(mut self, unit: &'static str) -> Field {
        self.unit = Some(unit);
        self
    }

    const fn scale(mut self, low: &'static str, high: &'static str) -> Field {
        self.scale = Some([low, high]);
        self
    }

    const fn command(mut self) -> Field {
        self.command = true;
        self
    }
}

/// Раздел страницы настроек.
#[derive(Debug, Clone, Copy)]
pub struct Section {
    pub id: &'static str,
    pub icon: &'static str,
    /// Ключ строки заголовка.
    pub title: &'static str,
    pub fields: &'static [Field],
}

static ANCHOR_OPTIONS: &[FieldOption] = &[
    FieldOption {
        value: "0",
        label: "#L_GS_X_ANCHOR_0",
    },
    FieldOption {
        value: "1",
        label: "#L_GS_X_ANCHOR_1",
    },
    FieldOption {
        value: "2",
        label: "#L_GS_X_ANCHOR_2",
    },
    FieldOption {
        value: "3",
        label: "#L_GS_X_ANCHOR_3",
    },
    FieldOption {
        value: "4",
        label: "#L_GS_X_ANCHOR_4",
    },
    FieldOption {
        value: "5",
        label: "#L_GS_X_ANCHOR_5",
    },
    FieldOption {
        value: "6",
        label: "#L_GS_X_ANCHOR_6",
    },
    FieldOption {
        value: "7",
        label: "#L_GS_X_ANCHOR_7",
    },
    FieldOption {
        value: "8",
        label: "#L_GS_X_ANCHOR_8",
    },
    FieldOption {
        value: "9",
        label: "#L_GS_X_ANCHOR_9",
    },
];

static FPS_OPTIONS: &[FieldOption] = &[
    FieldOption {
        value: "0",
        label: "#L_GS_X_FPS_MODE_0",
    },
    FieldOption {
        value: "1",
        label: "#L_GS_X_FPS_MODE_1",
    },
    FieldOption {
        value: "2",
        label: "#L_GS_X_FPS_MODE_2",
    },
];

/// Разделы в порядке `SECTIONS` 0.4.7. Строки таблицы длинные и одинаковые по устройству:
/// в столбик их не раскладывает ни автор, ни форматтер.
#[rustfmt::skip]
pub static SECTIONS: &[Section] = &[
    Section {
        id: "hud",
        icon: "◧",
        title: "#L_GS_X_SECTION_HUD",
        fields: &[
            entry("gh_hud_font", FieldType::Text { max_length: 200 }, FieldDefault::Text(""), "#L_GS_X_GH_HUD_FONT").hint("#L_GS_X_GH_HUD_FONT_HINT"),
            entry("gh_hud_fontscale", FieldType::Number { min: 0.25, max: 4.0, step: 0.05 }, FieldDefault::Number(1.0), "#L_GS_X_GH_HUD_FONTSCALE").requires("gh_hud_font"),
            entry("gh_hud_fontweight", FieldType::Number { min: 0.0, max: 1000.0, step: 100.0 }, FieldDefault::Number(0.0), "#L_GS_X_GH_HUD_FONTWEIGHT").hint("#L_GS_X_GH_HUD_FONTWEIGHT_HINT").requires("gh_hud_font"),
            entry("gh_hud_fontoutline", FieldType::Boolean, FieldDefault::Bool(false), "#L_GS_X_GH_HUD_FONTOUTLINE").requires("gh_hud_font"),
            entry("gh_hud_bg", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_HUD_BG"),
            entry("gh_hud_labels", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_HUD_LABELS"),
            entry("gh_hud_icons", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_HUD_ICONS"),
        ],
    },
    Section {
        id: "layout",
        icon: "⊞",
        title: "#L_GS_X_SECTION_LAYOUT",
        fields: &[
            entry("gh_health_align", FieldType::Select { options: ANCHOR_OPTIONS }, FieldDefault::Text("0"), "#L_GS_X_GH_HEALTH_ALIGN"),
            entry("gh_health_x", FieldType::Number { min: -640.0, max: 640.0, step: 1.0 }, FieldDefault::Number(0.0), "#L_GS_X_GH_HEALTH_X").hint("#L_GS_X_GH_HEALTH_X_HINT"),
            entry("gh_health_y", FieldType::Number { min: -480.0, max: 480.0, step: 1.0 }, FieldDefault::Number(0.0), "#L_GS_X_GH_HEALTH_Y").hint("#L_GS_X_GH_HEALTH_Y_HINT"),
            entry("gh_suit_align", FieldType::Select { options: ANCHOR_OPTIONS }, FieldDefault::Text("0"), "#L_GS_X_GH_SUIT_ALIGN"),
            entry("gh_suit_x", FieldType::Number { min: -640.0, max: 640.0, step: 1.0 }, FieldDefault::Number(0.0), "#L_GS_X_GH_SUIT_X").hint("#L_GS_X_GH_SUIT_X_HINT"),
            entry("gh_suit_y", FieldType::Number { min: -480.0, max: 480.0, step: 1.0 }, FieldDefault::Number(0.0), "#L_GS_X_GH_SUIT_Y").hint("#L_GS_X_GH_SUIT_Y_HINT"),
            entry("gh_ammo_align", FieldType::Select { options: ANCHOR_OPTIONS }, FieldDefault::Text("0"), "#L_GS_X_GH_AMMO_ALIGN"),
            entry("gh_ammo_x", FieldType::Number { min: -640.0, max: 640.0, step: 1.0 }, FieldDefault::Number(0.0), "#L_GS_X_GH_AMMO_X").hint("#L_GS_X_GH_AMMO_X_HINT"),
            entry("gh_ammo_y", FieldType::Number { min: -480.0, max: 480.0, step: 1.0 }, FieldDefault::Number(0.0), "#L_GS_X_GH_AMMO_Y").hint("#L_GS_X_GH_AMMO_Y_HINT"),
            entry("gh_ammo2_align", FieldType::Select { options: ANCHOR_OPTIONS }, FieldDefault::Text("0"), "#L_GS_X_GH_AMMO2_ALIGN"),
            entry("gh_ammo2_x", FieldType::Number { min: -640.0, max: 640.0, step: 1.0 }, FieldDefault::Number(0.0), "#L_GS_X_GH_AMMO2_X").hint("#L_GS_X_GH_AMMO2_X_HINT"),
            entry("gh_ammo2_y", FieldType::Number { min: -480.0, max: 480.0, step: 1.0 }, FieldDefault::Number(0.0), "#L_GS_X_GH_AMMO2_Y").hint("#L_GS_X_GH_AMMO2_Y_HINT"),
        ],
    },
    Section {
        id: "keys",
        icon: "⌨",
        title: "#L_GS_X_SECTION_KEYS",
        fields: &[
            entry("gh_keyshud", FieldType::Boolean, FieldDefault::Bool(false), "#L_GS_X_GH_KEYSHUD"),
            entry("gh_keyshud_full", FieldType::Boolean, FieldDefault::Bool(false), "#L_GS_X_GH_KEYSHUD_FULL").requires("gh_keyshud"),
            entry("gh_keyshud_wheel", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_KEYSHUD_WHEEL").requires("gh_keyshud"),
            entry("gh_keyshud_mouse", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_KEYSHUD_MOUSE").requires("gh_keyshud"),
            entry("gh_keyshud_bg", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_KEYSHUD_BG").requires("gh_keyshud"),
            entry("gh_keyshud_size", FieldType::Number { min: 6.0, max: 64.0, step: 1.0 }, FieldDefault::Number(13.0), "#L_GS_X_GH_KEYSHUD_SIZE").requires("gh_keyshud"),
            entry("gh_keyshud_full_size", FieldType::Number { min: 4.0, max: 32.0, step: 1.0 }, FieldDefault::Number(9.0), "#L_GS_X_GH_KEYSHUD_FULL_SIZE").requires("gh_keyshud_full"),
            entry("gh_keyshud_padding", FieldType::Number { min: 0.0, max: 16.0, step: 1.0 }, FieldDefault::Number(2.0), "#L_GS_X_GH_KEYSHUD_PADDING").requires("gh_keyshud"),
            entry("gh_keyshud_mouse_scale", FieldType::Number { min: 0.1, max: 50.0, step: 0.1 }, FieldDefault::Number(4.0), "#L_GS_X_GH_KEYSHUD_MOUSE_SCALE").requires("gh_keyshud_mouse"),
            entry("gh_keyshud_align", FieldType::Select { options: ANCHOR_OPTIONS }, FieldDefault::Text("9"), "#L_GS_X_GH_KEYSHUD_ALIGN").requires("gh_keyshud"),
            entry("gh_keyshud_x", FieldType::Number { min: -640.0, max: 640.0, step: 1.0 }, FieldDefault::Number(-19.0), "#L_GS_X_GH_KEYSHUD_X").hint("#L_GS_X_GH_KEYSHUD_X_HINT").requires("gh_keyshud"),
            entry("gh_keyshud_y", FieldType::Number { min: -480.0, max: 480.0, step: 1.0 }, FieldDefault::Number(-55.0), "#L_GS_X_GH_KEYSHUD_Y").hint("#L_GS_X_GH_KEYSHUD_Y_HINT").requires("gh_keyshud"),
        ],
    },
    Section {
        id: "weapon",
        icon: "⌖",
        title: "#L_GS_X_SECTION_WEAPON",
        fields: &[
            entry("fov_desired", FieldType::Number { min: 75.0, max: 150.0, step: 1.0 }, FieldDefault::Number(110.0), "#L_GS_X_FOV").hint("#L_GS_X_FOV_HINT").unit("°").scale("#L_GS_X_SCALE_NARROW", "#L_GS_X_SCALE_WIDE"),
            entry("viewmodel_fov", FieldType::Number { min: 10.0, max: 100.0, step: 1.0 }, FieldDefault::Number(62.0), "#L_GS_X_VIEWMODEL_FOV").hint("#L_GS_X_VIEWMODEL_FOV_HINT").unit("°").scale("#L_GS_X_SCALE_CLOSE", "#L_GS_X_SCALE_FAR"),
            entry("viewmodel_offset_x", FieldType::Number { min: -5.0, max: 5.0, step: 0.1 }, FieldDefault::Number(0.0), "#L_GS_X_VIEWMODEL_OFFSET_X").scale("#L_GS_X_SCALE_LEFT", "#L_GS_X_SCALE_RIGHT"),
            entry("viewmodel_offset_y", FieldType::Number { min: -4.0, max: 4.0, step: 0.1 }, FieldDefault::Number(0.0), "#L_GS_X_VIEWMODEL_OFFSET_Y").scale("#L_GS_X_SCALE_BACK", "#L_GS_X_SCALE_FORWARD"),
            entry("viewmodel_offset_z", FieldType::Number { min: -4.0, max: 4.0, step: 0.1 }, FieldDefault::Number(0.0), "#L_GS_X_VIEWMODEL_OFFSET_Z").scale("#L_GS_X_SCALE_DOWN", "#L_GS_X_SCALE_UP"),
            entry("viewmodel_bobbing", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_VIEWMODEL_BOBBING"),
            entry("cl_righthand", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_CL_RIGHTHAND"),
        ],
    },
    Section {
        id: "meters",
        icon: "◷",
        title: "#L_GS_X_SECTION_METERS",
        fields: &[
            entry("gh_timer", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_TIMER"),
            entry("gh_timer_bg", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_TIMER_BG").requires("gh_timer"),
            entry("gh_timer_mirror", FieldType::Boolean, FieldDefault::Bool(false), "#L_GS_X_GH_TIMER_MIRROR").hint("#L_GS_X_GH_TIMER_MIRROR_HINT").requires("gh_timer"),
            entry("gh_timer_mirror_port", FieldType::Number { min: 1.0, max: 65535.0, step: 1.0 }, FieldDefault::Number(16834.0), "#L_GS_X_GH_TIMER_MIRROR_PORT").requires("gh_timer_mirror"),
            entry("gh_timer_mirror_splits", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_TIMER_MIRROR_SPLITS").requires("gh_timer_mirror"),
            entry("gh_timer_mirror_splits_bold", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_TIMER_MIRROR_SPLITS_BOLD").requires("gh_timer_mirror_splits"),
            entry("gh_timer_align", FieldType::Select { options: ANCHOR_OPTIONS }, FieldDefault::Text("0"), "#L_GS_X_GH_TIMER_ALIGN").requires("gh_timer"),
            entry("gh_timer_x", FieldType::Number { min: -640.0, max: 640.0, step: 1.0 }, FieldDefault::Number(0.0), "#L_GS_X_GH_TIMER_X").hint("#L_GS_X_GH_TIMER_X_HINT").requires("gh_timer"),
            entry("gh_timer_y", FieldType::Number { min: -480.0, max: 480.0, step: 1.0 }, FieldDefault::Number(0.0), "#L_GS_X_GH_TIMER_Y").hint("#L_GS_X_GH_TIMER_Y_HINT").requires("gh_timer"),
            entry("gh_speedmeter", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_SPEEDMETER"),
            entry("gh_speedmeter_hvel", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_SPEEDMETER_HVEL").requires("gh_speedmeter"),
            entry("gh_speedmeter_bg", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_SPEEDMETER_BG").requires("gh_speedmeter"),
            entry("gh_speedmeter_label", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_SPEEDMETER_LABEL").requires("gh_speedmeter"),
            entry("gh_speedmeter_align", FieldType::Select { options: ANCHOR_OPTIONS }, FieldDefault::Text("0"), "#L_GS_X_GH_SPEEDMETER_ALIGN").requires("gh_speedmeter"),
            entry("gh_speedmeter_x", FieldType::Number { min: -640.0, max: 640.0, step: 1.0 }, FieldDefault::Number(0.0), "#L_GS_X_GH_SPEEDMETER_X").hint("#L_GS_X_GH_SPEEDMETER_X_HINT").requires("gh_speedmeter"),
            entry("gh_speedmeter_y", FieldType::Number { min: -480.0, max: 480.0, step: 1.0 }, FieldDefault::Number(0.0), "#L_GS_X_GH_SPEEDMETER_Y").hint("#L_GS_X_GH_SPEEDMETER_Y_HINT").requires("gh_speedmeter"),
            entry("cl_showfps", FieldType::Select { options: FPS_OPTIONS }, FieldDefault::Text("1"), "#L_GS_X_CL_SHOWFPS"),
            entry("cl_showpos", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_CL_SHOWPOS"),
            entry("gh_showfps_font", FieldType::Text { max_length: 120 }, FieldDefault::Text(""), "#L_GS_X_GH_SHOWFPS_FONT").hint("#L_GS_X_GH_SHOWFPS_FONT_HINT").requires("cl_showfps"),
            entry("gh_showfps_fontsize", FieldType::Number { min: 0.0, max: 128.0, step: 1.0 }, FieldDefault::Number(0.0), "#L_GS_X_GH_SHOWFPS_FONTSIZE").hint("#L_GS_X_GH_SHOWFPS_FONTSIZE_HINT").requires("cl_showfps"),
            entry("gh_showfps_fontweight", FieldType::Number { min: 0.0, max: 1000.0, step: 100.0 }, FieldDefault::Number(0.0), "#L_GS_X_GH_SHOWFPS_FONTWEIGHT").requires("cl_showfps"),
            entry("gh_showfps_fontoutline", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_SHOWFPS_FONTOUTLINE").requires("cl_showfps"),
            entry("gh_showpos_font", FieldType::Text { max_length: 120 }, FieldDefault::Text(""), "#L_GS_X_GH_SHOWPOS_FONT").hint("#L_GS_X_GH_SHOWPOS_FONT_HINT").requires("cl_showpos"),
            entry("gh_showpos_fontsize", FieldType::Number { min: 0.0, max: 128.0, step: 1.0 }, FieldDefault::Number(0.0), "#L_GS_X_GH_SHOWPOS_FONTSIZE").hint("#L_GS_X_GH_SHOWPOS_FONTSIZE_HINT").requires("cl_showpos"),
            entry("gh_showpos_fontweight", FieldType::Number { min: 0.0, max: 1000.0, step: 100.0 }, FieldDefault::Number(0.0), "#L_GS_X_GH_SHOWPOS_FONTWEIGHT").requires("cl_showpos"),
            entry("gh_showpos_fontoutline", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_SHOWPOS_FONTOUTLINE").requires("cl_showpos"),
        ],
    },
    Section {
        id: "ghosts",
        icon: "♧",
        title: "#L_GS_X_SECTION_GHOSTS",
        fields: &[
            entry("gh_name", FieldType::Text { max_length: 64 }, FieldDefault::Text("Ghost"), "#L_GS_X_GH_NAME"),
            entry("gh_model", FieldType::Text { max_length: 200 }, FieldDefault::Text("models/player.mdl"), "#L_GS_X_GH_MODEL").hint("#L_GS_X_GH_MODEL_HINT"),
            entry("gh_ghost_color", FieldType::Color, FieldDefault::Color(Color { r: 237, g: 133, b: 60 }), "#L_GS_X_GH_GHOST_COLOR"),
            entry("gh_opacity", FieldType::Number { min: 0.0, max: 255.0, step: 1.0 }, FieldDefault::Number(75.0), "#L_GS_X_GH_OPACITY"),
            entry("gh_opacity_range", FieldType::Number { min: 0.0, max: 5000.0, step: 10.0 }, FieldDefault::Number(200.0), "#L_GS_X_GH_OPACITY_RANGE"),
            entry("gh_ghost_names", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_GHOST_NAMES"),
            entry("gh_ghost_names_dist", FieldType::Number { min: 0.0, max: 20000.0, step: 100.0 }, FieldDefault::Number(2000.0), "#L_GS_X_GH_GHOST_NAMES_DIST").hint("#L_GS_X_GH_GHOST_NAMES_DIST_HINT").requires("gh_ghost_names"),
            entry("gh_ghost_names_own_color", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_GHOST_NAMES_OWN_COLOR").requires("gh_ghost_names"),
            entry("gh_draw_trails", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_DRAW_TRAILS"),
            entry("gh_trail_color", FieldType::Color, FieldDefault::Color(Color { r: 237, g: 133, b: 60 }), "#L_GS_X_GH_TRAIL_COLOR").requires("gh_draw_trails"),
            entry("gh_trail_length", FieldType::Number { min: 0.0, max: 60.0, step: 1.0 }, FieldDefault::Number(5.0), "#L_GS_X_GH_TRAIL_LENGTH").requires("gh_draw_trails"),
            entry("gh_show_held_props", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_SHOW_HELD_PROPS"),
            entry("gh_online_race_spectators", FieldType::Boolean, FieldDefault::Bool(false), "#L_GS_X_GH_ONLINE_RACE_SPECTATORS"),
            entry("gh_autorecord", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_AUTORECORD"),
            entry("gh_runcomplete_notify", FieldType::Boolean, FieldDefault::Bool(false), "#L_GS_X_GH_RUNCOMPLETE_NOTIFY"),
        ],
    },
    Section {
        id: "chat",
        icon: "☷",
        title: "#L_GS_X_SECTION_CHAT",
        fields: &[
            entry("gh_chat", FieldType::Boolean, FieldDefault::Bool(false), "#L_GS_X_GH_CHAT"),
            entry("gh_chat_channel", FieldType::Text { max_length: 25 }, FieldDefault::Text(""), "#L_GS_X_GH_CHAT_CHANNEL").hint("#L_GS_X_GH_CHAT_CHANNEL_HINT").requires("gh_chat"),
            entry("gh_chat_youtube", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_CHAT_YOUTUBE").hint("#L_GS_X_GH_CHAT_YOUTUBE_HINT").requires("gh_chat"),
            entry("gh_chat_connect", FieldType::Text { max_length: 512 }, FieldDefault::Text(""), "#L_GS_X_GH_CHAT_CONNECT").hint("#L_GS_X_GH_CHAT_CONNECT_HINT").command().requires("gh_chat_youtube"),
            entry("gh_chat_emotes", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_CHAT_EMOTES").requires("gh_chat"),
            entry("gh_chat_emote_scale", FieldType::Number { min: 0.25, max: 4.0, step: 0.05 }, FieldDefault::Number(2.0), "#L_GS_X_GH_CHAT_EMOTE_SCALE").requires("gh_chat_emotes"),
            entry("gh_chat_emoji_scale", FieldType::Number { min: 0.25, max: 4.0, step: 0.05 }, FieldDefault::Number(1.15), "#L_GS_X_GH_CHAT_EMOJI_SCALE").requires("gh_chat"),
            entry("gh_chat_badges", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_CHAT_BADGES").requires("gh_chat"),
            entry("gh_chat_prefix", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_CHAT_PREFIX").requires("gh_chat"),
            entry("gh_chat_minimal", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_CHAT_MINIMAL").requires("gh_chat"),
            entry("gh_chat_sound", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_CHAT_SOUND").requires("gh_chat"),
            entry("gh_chat_sound_stream", FieldType::Boolean, FieldDefault::Bool(false), "#L_GS_X_GH_CHAT_SOUND_STREAM").requires("gh_chat_sound"),
        ],
    },
    Section {
        id: "misc",
        icon: "◇",
        title: "#L_GS_X_SECTION_MISC",
        fields: &[
            entry("gh_hud_dmgflash", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_HUD_DMGFLASH"),
            entry("gh_hud_radflash", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_HUD_RADFLASH"),
            entry("gh_hud_color_affects_damage", FieldType::Boolean, FieldDefault::Bool(false), "#L_GS_X_GH_HUD_COLOR_AFFECTS_DAMAGE"),
            entry("gh_autosave", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_AUTOSAVE"),
            entry("gh_sprays", FieldType::Boolean, FieldDefault::Bool(true), "#L_GS_X_GH_SPRAYS"),
            entry("gh_spray_file", FieldType::Text { max_length: 512 }, FieldDefault::Text(""), "#L_GS_X_GH_SPRAY_FILE").hint("#L_GS_X_GH_SPRAY_FILE_HINT"),
            entry("gh_spray_scale", FieldType::Number { min: 0.05, max: 2.0, step: 0.05 }, FieldDefault::Number(0.25), "#L_GS_X_GH_SPRAY_SCALE").requires("gh_sprays"),
            entry("gh_spray_max", FieldType::Number { min: 1.0, max: 32.0, step: 1.0 }, FieldDefault::Number(10.0), "#L_GS_X_GH_SPRAY_MAX").requires("gh_sprays"),
        ],
    },
];

/// Все поля подряд по разделам (`FIELDS`).
pub fn fields() -> impl Iterator<Item = &'static Field> {
    SECTIONS.iter().flat_map(|section| section.fields.iter())
}

/// Поле по ключу (`BY_KEY`).
pub fn field(key: &str) -> Option<&'static Field> {
    fields().find(|field| field.key == key)
}

/// Значения по умолчанию всех полей (`defaults()`).
pub fn defaults() -> Extras {
    let mut result = Extras::new();
    for field in fields() {
        result.insert(field.key, field.default.to_extra());
    }
    result
}

/// Включено ли значение, которое пришло от окна: `!!value && value !== "0"`.
fn is_on(value: &Value) -> bool {
    js::truthy(value) && value.as_str() != Some("0")
}

fn default_is_on(default: FieldDefault) -> bool {
    match default {
        FieldDefault::Bool(value) => value,
        FieldDefault::Number(value) => value != 0.0,
        FieldDefault::Text(value) => !value.is_empty() && value != "0",
        FieldDefault::Color(_) => true,
    }
}

/// Поле с `requires` ничего не делает в игре, пока то, что оно требует, выключено или пусто
/// (проверено на Ghosting Mod Renewed 3.9). Требования цепочкой: поле действует, только когда
/// действуют все, что выше. Отсутствующее в `values` значение — по умолчанию.
pub fn available(field: &Field, values: &Value) -> bool {
    let mut key = field.requires;
    while let Some(required) = key {
        let Some(dependency) = self::field(required) else {
            return false;
        };
        let on = match values.as_object().and_then(|object| object.get(required)) {
            Some(value) => is_on(value),
            None => default_is_on(dependency.default),
        };
        if !on {
            return false;
        }
        key = dependency.requires;
    }
    true
}

/// Только то, чем лаунчер уже управляет: открытие страницы, сохранение прицела или сброс
/// одного раздела не должны захватывать все настройки мода.
pub fn reset_extras(input: &Value) -> Extras {
    let clean = sanitize_extras(input);
    let mut result = Extras::new();
    for (key, _) in clean.iter() {
        if let Some(field) = field(key) {
            result.insert(key, field.default.to_extra());
        }
    }
    result
}

/// Путь картинки, которую лаунчер положил в мод:
/// `/^materials\/vgui\/logos\/hlsr\/[a-f0-9]{64}\.(png|bmp|tga)$/`.
pub fn is_managed_spray_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("materials/vgui/logos/hlsr/") else {
        return false;
    };
    let Some((name, extension)) = rest.split_once('.') else {
        return false;
    };
    name.len() == 64
        && name
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        && matches!(extension, "png" | "bmp" | "tga")
}

/// Адрес записи или трансляции YouTube в канонический вид; пустая строка — «отключить»;
/// `None` — адрес не принят (`youtubeUrl`).
fn youtube_url(value: &str) -> Option<String> {
    if value.is_empty() {
        return Some(String::new());
    }
    let url = Url::parse(value).ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some_and(|password| !password.is_empty())
        || url.port().is_some()
    {
        return None;
    }
    let host = url.host_str()?;
    let id: Option<String> = if host == "youtu.be" {
        url.path().strip_prefix('/').map(str::to_owned)
    } else if matches!(host, "youtube.com" | "www.youtube.com" | "m.youtube.com") {
        if matches!(url.path(), "/watch" | "/live_chat" | "/live_chat_replay") {
            url.query_pairs()
                .find(|(name, _)| name == "v")
                .map(|(_, value)| value.into_owned())
        } else {
            // `/^\/live\/([A-Za-z0-9_-]+)\/?$/`
            url.path().strip_prefix("/live/").and_then(|rest| {
                let id = rest.strip_suffix('/').unwrap_or(rest);
                (!id.is_empty() && id.bytes().all(is_id_byte)).then(|| id.to_owned())
            })
        }
    } else {
        None
    };
    let id = id.filter(|id| id.len() == 11 && id.bytes().all(is_id_byte))?;
    Some(format!("https://www.youtube.com/watch?v={id}"))
}

fn is_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

/// `/^#([a-f\d]{6})$/i`: три канала из шестнадцатеричной записи.
fn hex_channels(text: &str) -> Option<[f64; 3]> {
    let digits = text.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| f64::from(u8::from_str_radix(&digits[at..at + 2], 16).unwrap_or(0));
    Some([channel(0), channel(2), channel(4)])
}

/// `text.trim().split(/\s+/).map(Number)`; пустой текст даёт один элемент — `Number("")`.
pub(crate) fn words_to_numbers(text: &str) -> Vec<f64> {
    let trimmed = js::trim(text);
    if trimmed.is_empty() {
        return vec![js::to_number("")];
    }
    trimmed
        .split(is_whitespace)
        .filter(|part| !part.is_empty())
        .map(js::to_number)
        .collect()
}

/// `text` без `prefix`, если он совпал без учёта регистра ASCII: флаг `i` регулярного выражения
/// без `u` сводит регистр только у ASCII (`ſ` не равно `s`, знак Кельвина — не `k`).
pub(crate) fn strip_prefix_ascii_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.as_bytes().get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix.as_bytes())
        .then(|| &text[prefix.len()..])
}

/// `Math.max(0, Math.min(255, Math.round(channel)))`.
fn clamp_channel(channel: f64) -> u8 {
    js::math_round(channel).clamp(0.0, 255.0) as u8
}

fn sanitize_color(value: &Value) -> Option<Color> {
    let channels: Vec<f64> = match value {
        Value::String(text) => match hex_channels(text) {
            Some(channels) => channels.to_vec(),
            None => words_to_numbers(text),
        },
        // Каналы объекта берутся как есть: строка вместо числа — не число.
        Value::Object(object) => ["r", "g", "b"]
            .iter()
            .map(|name| match object.get(*name) {
                Some(Value::Number(_)) => object.get(*name).and_then(js_value_number),
                _ => None,
            })
            .collect::<Option<Vec<f64>>>()?,
        _ => return None,
    };
    if channels.len() != 3 || !channels.iter().all(|channel| channel.is_finite()) {
        return None;
    }
    Some(Color {
        r: clamp_channel(channels[0]),
        g: clamp_channel(channels[1]),
        b: clamp_channel(channels[2]),
    })
}

fn js_value_number(value: &Value) -> Option<f64> {
    value
        .as_number()
        .map(|number| number.as_f64().unwrap_or(f64::INFINITY))
}

fn sanitize_number(min: f64, max: f64, step: f64, value: &Value) -> Option<f64> {
    let numeric = match value {
        Value::Number(_) => js_value_number(value)?,
        Value::String(text) => {
            if js::trim(text).is_empty() {
                return None;
            }
            js::to_number(text)
        }
        _ => return None,
    };
    if !numeric.is_finite() {
        return None;
    }
    let bounded = min.max(max.min(numeric));
    if step >= 1.0 {
        Some(js::math_round(bounded))
    } else {
        // `Number(bounded.toFixed(6))`
        Some(js::to_fixed(bounded, 6).parse().unwrap_or(f64::NAN))
    }
}

fn sanitize_text(field: &Field, max_length: usize, value: &Value) -> Option<String> {
    let Value::String(text) = value else {
        return None;
    };
    if text.encode_utf16().count() > max_length {
        return None;
    }
    // Значения стоят в кавычках в cfg Source. Разделители, кавычки и управляющие символы
    // отклоняются, а не экранируются: Source экранирование игнорирует.
    if text.chars().any(|c| {
        matches!(c, '"' | ';' | '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{2028}' | '\u{2029}')
    }) {
        return None;
    }
    let text = js::trim(text).replace('\\', "/");
    // gh_spray_set открывает путь как есть: сетевая папка дала бы чужому серверу соединение с
    // компьютера игрока (0.4.9, S-5).
    if field.key == "gh_spray_file"
        && (text.starts_with("//") || text.split('/').any(|part| part == ".."))
    {
        return None;
    }
    if field.key == "gh_spray_file"
        && let Some(managed) = text.strip_prefix("ghostingmod/")
        && is_managed_spray_path(managed)
    {
        return Some(managed.to_owned());
    }
    if field.key == "gh_chat_channel" {
        // `/^[a-z0-9_]{0,25}$/i`
        let fits = text.len() <= 25
            && text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        return fits.then(|| text.to_ascii_lowercase());
    }
    if field.key == "gh_chat_connect" {
        return youtube_url(&text);
    }
    Some(text)
}

/// `sanitizeValue(field, value)`: `None` — значение не принято (`undefined`).
fn sanitize_value(field: &Field, value: &Value) -> Option<ExtraValue> {
    match field.kind {
        FieldType::Boolean => match value {
            Value::Bool(flag) => Some(ExtraValue::Bool(*flag)),
            Value::Number(_) => match js_value_number(value)? {
                1.0 => Some(ExtraValue::Bool(true)),
                0.0 => Some(ExtraValue::Bool(false)),
                _ => None,
            },
            Value::String(text) if text == "1" => Some(ExtraValue::Bool(true)),
            Value::String(text) if text == "0" => Some(ExtraValue::Bool(false)),
            _ => None,
        },
        FieldType::Number { min, max, step } => {
            sanitize_number(min, max, step, value).map(ExtraValue::Number)
        }
        FieldType::Select { options } => {
            let text = match value {
                Value::String(text) => text.clone(),
                Value::Number(_) => js::to_string(value),
                _ => return None,
            };
            options
                .iter()
                .find(|option| option.value == text)
                .map(|option| ExtraValue::Text(option.value.to_owned()))
        }
        FieldType::Color => sanitize_color(value).map(ExtraValue::Color),
        FieldType::Text { max_length } => {
            sanitize_text(field, max_length, value).map(ExtraValue::Text)
        }
    }
}

/// `sanitizeExtras(input)`: значения известных полей в порядке `FIELDS`; неизвестное и
/// непринятое отбрасывается. Не объект — пустой набор.
pub fn sanitize_extras(input: &Value) -> Extras {
    let mut result = Extras::new();
    let Some(object) = input.as_object() else {
        return result;
    };
    for field in fields() {
        if let Some(clean) = object
            .get(field.key)
            .and_then(|value| sanitize_value(field, value))
        {
            result.insert(field.key, clean);
        }
    }
    result
}

fn text_of(value: &ExtraValue) -> &str {
    match value {
        ExtraValue::Text(text) => text,
        _ => "",
    }
}

/// `serializeExtras(input)`: строки cfg для `input` после `sanitizeExtras`. Заново нормализует
/// то, что приходит из файла настроек или окна.
pub fn serialize_extras(input: &Value) -> String {
    serialize_clean(&sanitize_extras(input))
}

/// То же для уже нормализованного набора (повторная нормализация его не меняет).
pub fn serialize_clean(clean: &Extras) -> String {
    let mut connections: Vec<String> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    for field in fields() {
        let Some(value) = clean.get(field.key) else {
            continue;
        };
        let line = match field.key {
            "gh_spray_file" => {
                // В отличие от шрифтов HUD, gh_spray_set открывает путь от рабочей папки
                // игры (корня установки), а не от путей поиска Source.
                let text = text_of(value);
                let file = if is_managed_spray_path(text) {
                    format!("ghostingmod/{text}")
                } else {
                    text.to_owned()
                };
                if file.is_empty() {
                    "gh_spray_clear".to_owned()
                } else {
                    format!("gh_spray_set \"{file}\"")
                }
            }
            "gh_chat_connect" => {
                let text = text_of(value);
                if text.is_empty() {
                    connections.push("gh_chat_disconnect youtube".to_owned());
                } else if clean.get("gh_chat") != Some(&ExtraValue::Bool(false))
                    && clean.get("gh_chat_youtube") != Some(&ExtraValue::Bool(false))
                {
                    connections.push(format!("gh_chat_connect \"{text}\""));
                }
                // Выбранный адрес запоминается, не включая выключенный читатель.
                // Комментарий разбирает только importExtras, как данные.
                format!("// HLSR_YOUTUBE_URL \"{text}\"")
            }
            // Угол обзора в Source называется тремя именами; игра читает то, что читает в
            // этот момент, поэтому все три получают одно значение.
            "fov_desired" => {
                let text = value.to_js_string();
                format!("default_fov \"{text}\"\nfov_desired \"{text}\"\nfov \"{text}\"")
            }
            _ => {
                if field.key == "gh_chat_channel" {
                    let text = text_of(value);
                    if text.is_empty() {
                        connections.push("gh_chat_disconnect twitch".to_owned());
                    } else if clean.get("gh_chat") != Some(&ExtraValue::Bool(false)) {
                        connections.push(format!("gh_chat_connect \"{text}\""));
                    }
                }
                let serialized = match value {
                    ExtraValue::Bool(flag) => if *flag { "1" } else { "0" }.to_owned(),
                    ExtraValue::Color(color) => color.to_cfg(),
                    other => other.to_js_string(),
                };
                format!("{} \"{serialized}\"", field.key)
            }
        };
        lines.push(line);
    }
    lines.extend(connections);
    lines.join("\n")
}

/// `/^\s*\/\/ HLSR_YOUTUBE_URL "([^"\r\n]*)"\s*$/`
static YOUTUBE_METADATA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*// HLSR_YOUTUBE_URL "([^"\r\n]*)"\s*$"#).expect("выражение 0.4.7 разбирается")
});

/// `/^\s*([a-z0-9_.]+)(?:\s+(?:"([^"\r\n]*)"|([^"\r\n]*?)))?\s*(?:\/\/.*)?$/i`; флаг `i` без `u`
/// сводит регистр только у ASCII, поэтому буквы класса написаны обоими регистрами.
static COMMAND_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*([a-zA-Z0-9_.]+)(?:\s+(?:"([^"\r\n]*)"|([^"\r\n]*?)))?\s*(?://.*)?$"#)
        .expect("выражение 0.4.7 разбирается")
});

/// `text.split(/\r?\n/)`: `\r` убирается только перед `\n`.
pub fn split_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    let last = lines.len().saturating_sub(1);
    for line in &mut lines[..last] {
        if let Some(stripped) = line.strip_suffix('\r') {
            *line = stripped;
        }
    }
    lines
}

/// `importExtras(text)`: значения из строк cfg (мода или лаунчера). Последнее присваивание
/// побеждает, но место ключа остаётся первым.
pub fn import_extras(text: &str) -> Extras {
    let mut parsed = Extras::new();
    for line in split_lines(text) {
        if let Some(found) = YOUTUBE_METADATA.find(line) {
            let raw = found.group(1).map(|range| &line[range]).unwrap_or_default();
            if let Some(connect) = field("gh_chat_connect")
                && let Some(value) = sanitize_value(connect, &Value::String(raw.to_owned()))
            {
                parsed.insert("gh_chat_connect", value);
            }
            continue;
        }
        let Some(found) = COMMAND_LINE.find(line) else {
            continue;
        };
        let command = found
            .group(1)
            .map(|range| line[range].to_ascii_lowercase())
            .unwrap_or_default();
        let value: String = match found.group(2) {
            Some(range) => line[range].to_owned(),
            None => found
                .group(3)
                .map(|range| js::trim(&line[range]).to_owned())
                .unwrap_or_default(),
        };
        let (key, raw): (&str, &str) = match command.as_str() {
            "gh_spray_set" => ("gh_spray_file", &value),
            "gh_spray_clear" if value.is_empty() => ("gh_spray_file", ""),
            "gh_chat_disconnect"
                if value.eq_ignore_ascii_case("youtube") || value.eq_ignore_ascii_case("all") =>
            {
                ("gh_chat_connect", "")
            }
            other => (other, &value),
        };
        let Some(target) = field(key) else {
            continue;
        };
        if let Some(clean) = sanitize_value(target, &Value::String(raw.to_owned())) {
            parsed.insert(target.key, clean);
        }
    }
    parsed
}
