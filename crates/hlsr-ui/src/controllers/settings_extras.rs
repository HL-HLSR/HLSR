//! Дополнительные настройки Ghosting и HL1 (`GhostingSettingsExtras.vue` 0.4.7, задача 31;
//! дельта 0.4.9: раздел «Цвет и прицел», «Моды», превью у «Призраков», выбор модели, список
//! разделов ведёт страница — `active_nav_id`/`nav_items`):
//! список разделов (персонализация, разделы схемы, оверлей LiveSplit, код настроек), поля
//! раздела по схеме (`ghosting_settings`/`goldsrc_settings` подпроекта 2: bool, number,
//! select, color, text, выбор файла), точка «изменено» и счёт изменений, ползунки с детентом
//! умолчания и шагом, проверка значений, сброс раздела, клавиши списка. Без окна: состояние и
//! чистые функции; изменения уходят наружу значением `{...value, extra}` (`$emit('input')`).
//! Сверка — `tests/personalization_logic.rs` по фикстуре `export-personalization-logic.js`.

use hlsr_core::js::{self, math_round};
use hlsr_core::json_text::stringify;
use hlsr_delivery::ghosting_personalization::default_personalization;
use hlsr_delivery::ghosting_settings::{self, FieldDefault, FieldOption, FieldType};
use hlsr_delivery::goldsrc_settings;
use hlsr_delivery::livesplit_overlay;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Раздел «Персонализация» (редактор HUD и прицела) — всегда первый.
pub const PERSONALIZATION: &str = "personalization";
/// Раздел «Код настроек» (слот `settings-code`).
pub const SETTINGS_CODE: &str = "settings-code";
/// Раздел «Оверлей LiveSplit».
pub const LIVESPLIT: &str = "livesplit";
/// Раздел «Моды» (`CustomPacks.vue` 0.4.9): включаются сразу, без «Сохранить».
pub const MODS: &str = "mods";
/// Разделы, которые меняют кадр превью (`PREVIEW_SECTIONS`); у прочих — поля на всю ширину.
/// «Призраки» и «Эффекты и спреи» в 0.4.9 показывают свою сцену.
const PREVIEW_SECTIONS: [&str; 9] = [
    "personalization",
    "hud",
    "layout",
    "keys",
    "weapon",
    "meters",
    "ghosts",
    "misc",
    "livesplit",
];
const HL1_PREVIEW_SECTIONS: [&str; 5] = [
    "personalization",
    "hl1packs",
    "hl1hud",
    "hl1view",
    "livesplit",
];
/// Значки разделов Ghosting (`ICONS`): у схемы значки — символы Unicode, окно берёт свои.
const ICONS: [(&str, &str); 9] = [
    ("personalization", "fa-palette"),
    ("hud", "fa-font"),
    ("layout", "fa-th-large"),
    ("keys", "fa-keyboard"),
    ("weapon", "fa-crosshairs"),
    ("meters", "fa-stopwatch"),
    ("ghosts", "fa-ghost"),
    ("chat", "fa-comment-alt"),
    ("misc", "fa-sliders-h"),
];
/// На сколько пикселей бегунок ползунка липнет к метке умолчания.
pub const SLIDER_DETENT_PX: f64 = 6.0;

/// Схема полей: Ghosting или HL1 (`schema` — `goldsrc-settings`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schema {
    Ghosting,
    Goldsrc,
}

/// Поле схемы в одном виде для окна.
#[derive(Debug, Clone, Copy)]
pub struct FieldView {
    pub key: &'static str,
    pub kind: FieldType,
    pub default: FieldDefault,
    pub label: &'static str,
    pub hint: Option<&'static str>,
    pub requires: Option<&'static str>,
    pub requires_value: Option<&'static str>,
    pub unit: Option<&'static str>,
    pub scale: Option<[&'static str; 2]>,
}

impl FieldView {
    fn ghosting(field: &ghosting_settings::Field) -> FieldView {
        FieldView {
            key: field.key,
            kind: field.kind,
            default: field.default,
            label: field.label,
            hint: field.hint,
            requires: field.requires,
            requires_value: None,
            unit: field.unit,
            scale: field.scale,
        }
    }

    fn goldsrc(field: &goldsrc_settings::Field) -> FieldView {
        FieldView {
            key: field.key,
            kind: field.kind,
            default: field.default,
            label: field.label,
            hint: field.hint,
            requires: field.requires,
            requires_value: field.requires_value,
            unit: None,
            scale: None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self.kind {
            FieldType::Boolean => "boolean",
            FieldType::Number { .. } => "number",
            FieldType::Select { .. } => "select",
            FieldType::Color => "color",
            FieldType::Text { .. } => "text",
        }
    }

    pub fn default_value(&self) -> Value {
        self.default.to_extra().to_value()
    }

    pub fn options(&self) -> &'static [FieldOption] {
        match self.kind {
            FieldType::Select { options } => options,
            _ => &[],
        }
    }

    /// `min`, `max`, `step || 1` числового поля.
    pub fn range(&self) -> Option<(f64, f64, f64)> {
        match self.kind {
            FieldType::Number { min, max, step } => {
                Some((min, max, if step == 0.0 { 1.0 } else { step }))
            }
            _ => None,
        }
    }

    pub fn max_length(&self) -> usize {
        match self.kind {
            FieldType::Text { max_length } => max_length,
            _ => 240,
        }
    }
}

/// Раздел списка.
#[derive(Debug, Clone)]
pub struct SectionView {
    pub id: &'static str,
    pub title: &'static str,
    pub icon: Option<&'static str>,
    pub link: Option<&'static str>,
    pub fields: Vec<FieldView>,
}

impl Schema {
    /// Разделы схемы (`SECTIONS`).
    pub fn sections(self) -> Vec<SectionView> {
        match self {
            Schema::Ghosting => ghosting_settings::SECTIONS
                .iter()
                .map(|s| SectionView {
                    id: s.id,
                    title: s.title,
                    icon: Some(s.icon),
                    link: None,
                    fields: s.fields.iter().map(FieldView::ghosting).collect(),
                })
                .collect(),
            Schema::Goldsrc => goldsrc_settings::SECTIONS
                .iter()
                .map(|s| SectionView {
                    id: s.id,
                    title: s.title,
                    icon: Some(s.icon),
                    link: s.link,
                    fields: s.fields.iter().map(FieldView::goldsrc).collect(),
                })
                .collect(),
        }
    }

    /// `FIELDS`: все поля подряд.
    pub fn fields(self) -> Vec<FieldView> {
        self.sections().into_iter().flat_map(|s| s.fields).collect()
    }

    pub fn field(self, key: &str) -> Option<FieldView> {
        self.fields().into_iter().find(|f| f.key == key)
    }

    pub fn defaults(self) -> Value {
        match self {
            Schema::Ghosting => ghosting_settings::defaults().to_value(),
            Schema::Goldsrc => goldsrc_settings::defaults().to_value(),
        }
    }

    pub fn sanitize_extras(self, input: &Value) -> Value {
        match self {
            Schema::Ghosting => ghosting_settings::sanitize_extras(input).to_value(),
            Schema::Goldsrc => goldsrc_settings::sanitize_extras(input).to_value(),
        }
    }

    pub fn available(self, field: &FieldView, values: &Value) -> bool {
        match self {
            Schema::Ghosting => ghosting_settings::field(field.key)
                .is_some_and(|f| ghosting_settings::available(f, values)),
            Schema::Goldsrc => goldsrc_settings::field(field.key)
                .is_some_and(|f| goldsrc_settings::available(f, values)),
        }
    }
}

/// Пункт списка разделов.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavItem {
    pub id: &'static str,
    pub title: &'static str,
    /// Классы значка: `fas fa-…`.
    pub icon: String,
    pub changes: usize,
}

/// Положение ползунка и заливки в процентах дорожки (`sliderStyle`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliderStyle {
    pub value: f64,
    pub default: f64,
    pub from: f64,
    pub to: f64,
}

/// Снимок окна LiveSplit, который редактор оверлея отдаёт превью.
pub type Snapshot = crate::controllers::preview::Snapshot;

/// `personalizationChanges(value)`: сколько настроек отличается от умолчаний игры (без
/// `extra` и `crossPreset`) — `ghosting-personalization-defaults.js`.
pub fn personalization_changes(value: Option<&Value>) -> usize {
    let Some(value) = value.filter(|v| js::truthy(v)) else {
        return 0;
    };
    let defaults = default_personalization().to_value();
    let Value::Object(defaults) = defaults else {
        return 0;
    };
    defaults
        .iter()
        .filter(|(key, _)| key.as_str() != "extra" && key.as_str() != "crossPreset")
        .filter(|(key, default)| {
            // JSON.stringify(undefined) — undefined, не равно строке.
            match value.get(key.as_str()) {
                Some(v) => stringify(v, "") != stringify(default, ""),
                None => true,
            }
        })
        .count()
}

/// `same(a, b)`: цвета — по каналам, остальное — строгое равенство JS.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => ["r", "g", "b"]
            .iter()
            .all(|ch| a.get(*ch).map(number) == b.get(*ch).map(number)),
        (Value::Object(_), _) => false,
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Null, Value::Null) => true,
        _ => false,
    }
}

fn number(value: &Value) -> f64 {
    match value {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(text) => js::to_number(text),
        Value::Bool(true) => 1.0,
        Value::Bool(false) | Value::Null => 0.0,
        _ => f64::NAN,
    }
}

/// `String(value)` для подстановки в поле.
pub fn js_string(value: &Value) -> String {
    js::to_string(value)
}

/// Состояние окна доп. настроек (`data()` и props).
#[derive(Debug, Clone)]
pub struct ExtrasEditor {
    pub schema: Schema,
    pub game_id: String,
    value: Map<String, Value>,
    overlay: Option<Value>,
    pub has_code_slot: bool,
    pub active: bool,
    pub active_id: String,
    /// Панель редактора персонализации (`hud`/`cross`) — что показывает превью.
    pub personalization_panel: String,
    pub picking_asset: String,
    /// Ошибка выбора файла по полю: ключ строки (`#L_GS_X_PICK_ERROR`, `#L_GS_X_MODEL_OUTSIDE`).
    pub asset_errors: BTreeMap<String, &'static str>,
    sliding: BTreeMap<String, f64>,
    pub livesplit_snapshot: Option<Snapshot>,
}

impl ExtrasEditor {
    pub fn new(
        schema: Schema,
        game_id: &str,
        value: Value,
        overlay: Option<Value>,
    ) -> ExtrasEditor {
        ExtrasEditor {
            schema,
            game_id: game_id.to_owned(),
            value: match value {
                Value::Object(map) => map,
                _ => Map::new(),
            },
            overlay,
            has_code_slot: true,
            active: true,
            active_id: PERSONALIZATION.to_owned(),
            personalization_panel: "hud".to_owned(),
            picking_asset: String::new(),
            asset_errors: BTreeMap::new(),
            sliding: BTreeMap::new(),
            livesplit_snapshot: None,
        }
    }

    pub fn value(&self) -> Value {
        Value::Object(self.value.clone())
    }

    pub fn set_value(&mut self, value: Value) {
        if let Value::Object(map) = value {
            self.value = map;
        }
    }

    pub fn overlay(&self) -> Option<&Value> {
        self.overlay.as_ref()
    }

    pub fn set_overlay(&mut self, overlay: Option<Value>) {
        self.overlay = overlay;
    }

    /// `navItems`: персонализация, разделы схемы, моды, оверлей (если есть), код (если есть слот).
    pub fn nav_items(&self) -> Vec<NavItem> {
        let mut items = vec![NavItem {
            id: PERSONALIZATION,
            title: "#L_GS_S_LOOK",
            icon: self.section_icon(PERSONALIZATION),
            changes: personalization_changes(Some(&self.value())),
        }];
        for section in self.schema.sections() {
            items.push(NavItem {
                id: section.id,
                title: section.title,
                icon: self.section_icon(section.id),
                changes: self.section_changes(section.id),
            });
        }
        items.push(NavItem {
            id: MODS,
            title: match self.schema {
                Schema::Ghosting => "#L_CPK_TAB_GHOSTING",
                Schema::Goldsrc => "#L_CPK_TAB_HL1",
            },
            icon: self.section_icon(MODS),
            changes: 0,
        });
        if self.overlay.is_some() {
            items.push(NavItem {
                id: LIVESPLIT,
                title: "#L_LSO_TITLE",
                icon: self.section_icon(LIVESPLIT),
                changes: self.section_changes(LIVESPLIT),
            });
        }
        if self.has_code_slot {
            items.push(NavItem {
                id: SETTINGS_CODE,
                title: "#L_PZ_CODE_TITLE",
                icon: self.section_icon(SETTINGS_CODE),
                changes: 0,
            });
        }
        items
    }

    pub fn nav_ids(&self) -> Vec<&'static str> {
        self.nav_items().into_iter().map(|i| i.id).collect()
    }

    pub fn is_livesplit(&self) -> bool {
        self.active_id == LIVESPLIT
    }

    pub fn is_personalization(&self) -> bool {
        self.active_id == PERSONALIZATION
    }

    pub fn is_code(&self) -> bool {
        self.active_id == SETTINGS_CODE
    }

    pub fn is_mods(&self) -> bool {
        self.active_id == MODS
    }

    /// `activeSection.id` для списка страницы (`navState.active`): незнакомый — первый пункт.
    pub fn active_nav_id(&self) -> &'static str {
        let ids = self.nav_ids();
        ids.iter()
            .copied()
            .find(|id| *id == self.active_id)
            .unwrap_or(PERSONALIZATION)
    }

    /// `activeSection`: раздел схемы по `activeId`; первый пункт — не раздел схемы (`None`).
    pub fn active_section(&self) -> Option<SectionView> {
        self.schema
            .sections()
            .into_iter()
            .find(|s| s.id == self.active_id)
    }

    pub fn has_preview(&self) -> bool {
        match self.schema {
            Schema::Ghosting => PREVIEW_SECTIONS.contains(&self.active_id.as_str()),
            Schema::Goldsrc => HL1_PREVIEW_SECTIONS.contains(&self.active_id.as_str()),
        }
    }

    /// `previewSection`: у персонализации — её панель (HUD или прицел).
    pub fn preview_section(&self) -> String {
        if self.is_personalization() {
            self.personalization_panel.clone()
        } else {
            self.active_id.clone()
        }
    }

    /// `fieldColumns`: поля раздела (у паков HL1 — только доступные из зависимых), одной
    /// колонкой при превью, иначе двумя.
    pub fn field_columns(&self) -> Vec<Vec<FieldView>> {
        let Some(section) = self.active_section() else {
            return Vec::new();
        };
        let resolved = self.resolved();
        let fields: Vec<FieldView> = section
            .fields
            .into_iter()
            .filter(|f| {
                self.active_id != "hl1packs"
                    || f.requires_value.is_none()
                    || self.schema.available(f, &resolved)
            })
            .collect();
        if self.has_preview() {
            return vec![fields];
        }
        let middle = fields.len().div_ceil(2);
        let (first, second) = fields.split_at(middle);
        vec![first.to_vec(), second.to_vec()]
    }

    /// `extra`: `value.extra || {}`.
    pub fn extra(&self) -> Map<String, Value> {
        match self.value.get("extra") {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        }
    }

    /// `resolved`: умолчания схемы под значениями игрока.
    pub fn resolved(&self) -> Value {
        let mut result = match self.schema.defaults() {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        for (key, value) in self.extra() {
            result.insert(key, value);
        }
        Value::Object(result)
    }

    pub fn resolved_value(&self, key: &str) -> Value {
        self.resolved().get(key).cloned().unwrap_or(Value::Null)
    }

    /// `invalidFields`: поля игрока, которые проверка схемы отбросила.
    pub fn invalid_fields(&self) -> BTreeSet<String> {
        let extra = self.extra();
        let sanitized = self.schema.sanitize_extras(&Value::Object(extra.clone()));
        self.schema
            .fields()
            .into_iter()
            .filter(|f| extra.contains_key(f.key) && sanitized.get(f.key).is_none())
            .map(|f| f.key.to_owned())
            .collect()
    }

    pub fn valid(&self) -> bool {
        self.invalid_fields().is_empty()
    }

    /// Ключ текста ошибки поля: особые — только у полей чата (YouTube, Twitch).
    pub fn error_key(key: &str) -> &'static str {
        match key {
            "gh_chat_connect" => "#L_GS_X_INVALID_YOUTUBE",
            "gh_chat_channel" => "#L_GS_X_INVALID_TWITCH",
            _ => "#L_GS_X_INVALID_TEXT",
        }
    }

    /// `sectionIcon(id)`: `fas fa-…` — значок схемы, если он класс FontAwesome, иначе из `ICONS`.
    pub fn section_icon(&self, id: &str) -> String {
        let own = match id {
            SETTINGS_CODE => Some("fa-share-alt"),
            LIVESPLIT => Some("fa-layer-group"),
            MODS => Some("fa-cubes"),
            _ => self
                .schema
                .sections()
                .into_iter()
                .find(|s| s.id == id)
                .and_then(|s| s.icon),
        };
        let icon = own
            .filter(|icon| icon.starts_with("fa-"))
            .or_else(|| {
                ICONS
                    .iter()
                    .find(|(key, _)| *key == id)
                    .map(|(_, icon)| *icon)
            })
            .unwrap_or("fa-cog");
        format!("fas {icon}")
    }

    /// Подпись выбранного варианта (`selectLabel`): ключ строки варианта или само значение.
    pub fn select_label(&self, field: &FieldView) -> String {
        let value = self.resolved_value(field.key);
        match field
            .options()
            .iter()
            .find(|o| value.as_str() == Some(o.value))
        {
            Some(option) => option.label.to_owned(),
            None => match value {
                Value::Null => String::new(),
                other => js_string(&other),
            },
        }
    }

    /// `requirementLabel(field)`: подпись зависимости (поля или его варианта).
    pub fn requirement_label(&self, field: &FieldView) -> String {
        let requires = field.requires.unwrap_or("");
        match field.requires_value {
            None => self.field_label(requires),
            Some(expected) => {
                let parent = self.schema.field(requires);
                parent
                    .and_then(|p| {
                        p.options()
                            .iter()
                            .find(|o| o.value == expected)
                            .map(|o| o.label)
                    })
                    .map(str::to_owned)
                    .unwrap_or_else(|| self.field_label(requires))
            }
        }
    }

    pub fn is_available(&self, field: &FieldView) -> bool {
        self.schema.available(field, &self.resolved())
    }

    pub fn field_label(&self, key: &str) -> String {
        self.schema
            .field(key)
            .map_or_else(|| key.to_owned(), |f| f.label.to_owned())
    }

    pub fn is_overridden(&self, key: &str) -> bool {
        self.extra().contains_key(key)
    }

    /// `isChanged(field)`: записано лаунчером и отличается от умолчания игры.
    pub fn is_changed(&self, field: &FieldView) -> bool {
        self.is_overridden(field.key)
            && !same(&self.resolved_value(field.key), &field.default_value())
    }

    /// `sectionChanges(section)`.
    pub fn section_changes(&self, id: &str) -> usize {
        if id == PERSONALIZATION {
            return personalization_changes(Some(&self.value()));
        }
        if id == LIVESPLIT {
            return self
                .overlay
                .as_ref()
                .map_or(0, livesplit_overlay::overlay_changes);
        }
        self.schema
            .sections()
            .into_iter()
            .find(|s| s.id == id)
            .map_or(0, |s| {
                s.fields.iter().filter(|f| self.is_changed(f)).count()
            })
    }

    /// `resetLabel`: подпись кнопки сброса — ключ и число изменений (0 — без счёта).
    pub fn reset_changes(&self) -> usize {
        self.section_changes(&self.active_id.clone())
    }

    /// `hasRange(field)`: конечные пределы не шире 1500.
    pub fn has_range(field: &FieldView) -> bool {
        field
            .range()
            .is_some_and(|(min, max, _)| min.is_finite() && max.is_finite() && max - min <= 1500.0)
    }

    fn at(field: &FieldView, v: f64) -> f64 {
        let (min, max, _) = field.range().unwrap_or((0.0, 100.0, 1.0));
        ((v - min) / (max - min) * 100.0).clamp(0.0, 100.0)
    }

    /// `sliderStyle(field)`.
    pub fn slider_style(&self, field: &FieldView) -> SliderStyle {
        let value = Self::at(field, self.slider_value(field));
        let base = Self::at(field, number(&field.default_value()));
        // `Math.min`/`Math.max`: с NaN (неверное значение поля) — NaN, не второй аргумент.
        let nan = value.is_nan() || base.is_nan();
        SliderStyle {
            value,
            default: base,
            from: if nan { f64::NAN } else { value.min(base) },
            to: if nan { f64::NAN } else { value.max(base) },
        }
    }

    /// `sliderValue(field)`: пока тянут — сырое положение бегунка, иначе значение.
    pub fn slider_value(&self, field: &FieldView) -> f64 {
        match self.sliding.get(field.key) {
            Some(raw) => *raw,
            None => number(&self.resolved_value(field.key)),
        }
    }

    pub fn is_sliding(&self, key: &str) -> bool {
        self.sliding.contains_key(key)
    }

    /// `detent(field, input)`: у метки умолчания бегунок липнет к ней; `width` — ширина
    /// дорожки в px (0 — без детента).
    pub fn detent(field: &FieldView, raw: f64, width: f64) -> f64 {
        let (min, max, _) = field.range().unwrap_or((0.0, 100.0, 1.0));
        let default = number(&field.default_value());
        if width == 0.0 || !default.is_finite() {
            return raw;
        }
        if (raw - default).abs() / (max - min) * width <= SLIDER_DETENT_PX {
            default
        } else {
            raw
        }
    }

    /// `snap(field, raw)`: к ближайшему шагу от `min`, с точностью до 6 знаков.
    pub fn snap(field: &FieldView, raw: f64) -> f64 {
        let (min, _, step) = field.range().unwrap_or((0.0, 100.0, 1.0));
        let value = min + math_round((raw - min) / step) * step;
        js::to_number(&js::to_fixed(value, 6))
    }

    /// `slide(field, input)`: бегунок идёт за указателем, значение — ближайший шаг.
    pub fn slide(&mut self, field: &FieldView, raw: f64, width: f64) -> Option<Value> {
        let detented = Self::detent(field, raw, width);
        self.sliding.insert(field.key.to_owned(), detented);
        self.set_number(field, json!(Self::snap(field, detented))).0
    }

    /// `endSlide(field, input)`: бегунок отпущен — к шагу.
    pub fn end_slide(&mut self, field: &FieldView, raw: f64, width: f64) -> Option<Value> {
        self.sliding.remove(field.key);
        self.set_number(
            field,
            json!(Self::snap(field, Self::detent(field, raw, width))),
        )
        .0
    }

    pub fn select_section(&mut self, id: &str) {
        self.active_id = id.to_owned();
    }

    /// `onNavKey`: индекс раздела по клавише списка (`None` — не клавиша списка).
    pub fn nav_key(&self, key: &str) -> Option<usize> {
        let ids = self.nav_ids();
        let count = ids.len();
        let index = ids
            .iter()
            .position(|id| *id == self.active_id)
            .map_or(-1, |i| i as i64);
        let next = match key {
            "ArrowDown" | "ArrowRight" => (index + 1).rem_euclid(count as i64),
            "ArrowUp" | "ArrowLeft" => (index - 1 + count as i64).rem_euclid(count as i64),
            "Home" => 0,
            "End" => count as i64 - 1,
            _ => return None,
        };
        Some(next as usize)
    }

    /// `set(field, nextValue)`: новое значение окна (`{...value, extra: {...extra, key}}`).
    pub fn set(&mut self, field: &FieldView, next: Value) -> Value {
        self.asset_errors.remove(field.key);
        let mut extra = self.extra();
        extra.insert(field.key.to_owned(), next);
        let mut value = self.value.clone();
        value.insert("extra".into(), Value::Object(extra));
        Value::Object(value)
    }

    pub fn asset_kind(field: &FieldView) -> &'static str {
        match field.key {
            "gh_spray_file" => "spray",
            "gh_hud_font" => "font",
            "gh_model" => "model",
            _ => "",
        }
    }

    /// `setNumber(field, value, input)`: `(новое значение окна, текст поля после)`; значение
    /// не прошло проверку — поле возвращается к текущему, окно не меняется.
    pub fn set_number(&mut self, field: &FieldView, value: Value) -> (Option<Value>, String) {
        let mut probe = Map::new();
        probe.insert(field.key.to_owned(), value);
        let clean = self.schema.sanitize_extras(&Value::Object(probe));
        match clean.get(field.key) {
            None => (None, js_string(&self.resolved_value(field.key))),
            Some(next) => {
                let text = js_string(next);
                (Some(self.set(field, next.clone())), text)
            }
        }
    }

    /// `toHex(color)`: `#rrggbb`; не объект — цвет HUD по умолчанию.
    pub fn to_hex(color: &Value) -> String {
        let source = match color {
            Value::Object(map) => map.clone(),
            _ => {
                let mut map = Map::new();
                map.insert("r".into(), json!(255));
                map.insert("g".into(), json!(220));
                map.insert("b".into(), json!(0));
                map
            }
        };
        let channel = |key: &str| {
            let n = source.get(key).map_or(0.0, number);
            let n = if n.is_finite() { n } else { 0.0 };
            format!("{:02x}", math_round(n).clamp(0.0, 255.0) as u8)
        };
        format!("#{}{}{}", channel("r"), channel("g"), channel("b"))
    }

    /// `setColor(field, value)`: `#rrggbb` → `{r, g, b}`; иное не принимается.
    pub fn set_color(&mut self, field: &FieldView, value: &str) -> Option<Value> {
        let digits = value.strip_prefix('#')?;
        if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).unwrap_or(0);
        Some(self.set(
            field,
            json!({"r": channel(0), "g": channel(2), "b": channel(4)}),
        ))
    }

    /// `resetSection()`: записанные поля раздела — к умолчаниям (остальное не трогается).
    pub fn reset_section(&mut self) -> Option<Value> {
        let section = self.active_section()?;
        let mut extra = self.extra();
        for field in &section.fields {
            self.asset_errors.remove(field.key);
            if extra.contains_key(field.key) {
                extra.insert(field.key.to_owned(), field.default_value());
            }
        }
        let mut value = self.value.clone();
        value.insert("extra".into(), Value::Object(extra));
        Some(Value::Object(value))
    }

    /// `pickAsset(field)` начинается: `Some(kind)` — звать `game-personalization-pick-asset`
    /// (`None` — уже выбирают другой файл).
    pub fn begin_pick(&mut self, field: &FieldView) -> Option<&'static str> {
        if !self.picking_asset.is_empty() {
            return None;
        }
        self.picking_asset = field.key.to_owned();
        self.asset_errors.remove(field.key);
        Some(Self::asset_kind(field))
    }

    /// Ответ `game-personalization-pick-asset`: новое значение окна или ошибка выбора (модель вне
    /// папки мода — своя строка, 0.4.9).
    pub fn asset_picked(
        &mut self,
        field: &FieldView,
        reply: Result<Value, String>,
    ) -> Option<Value> {
        self.picking_asset.clear();
        let error = match reply {
            Ok(result) if result.get("status").and_then(Value::as_str) == Some("ok") => {
                let value = result.get("value").cloned().unwrap_or(Value::Null);
                return Some(self.set(field, value));
            }
            Ok(result)
                if !js::truthy(&result)
                    || result.get("status").and_then(Value::as_str) == Some("canceled") =>
            {
                return None;
            }
            Ok(result)
                if result.get("code").and_then(Value::as_str) == Some("ASSET_OUTSIDE_MOD") =>
            {
                "#L_GS_X_MODEL_OUTSIDE"
            }
            _ => "#L_GS_X_PICK_ERROR",
        };
        self.asset_errors.insert(field.key.to_owned(), error);
        None
    }
}

/// `hudMod`: у Ghosting включён мод Condensed HUD — превью рисует его HUD.
pub fn hud_mod(schema: Schema, packs: Option<&Value>) -> bool {
    schema == Schema::Ghosting
        && packs.and_then(Value::as_array).is_some_and(|list| {
            list.iter().any(|pack| {
                pack.get("id").and_then(Value::as_str) == Some("condensed-hud")
                    && pack.get("enabled").is_some_and(js::truthy)
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(extra: Value) -> ExtrasEditor {
        let mut value = default_personalization().to_value();
        value["extra"] = extra;
        ExtrasEditor::new(
            Schema::Ghosting,
            "218",
            value,
            Some(json!({"enabled": false})),
        )
    }

    #[test]
    fn the_chat_section_is_listed_before_misc_and_its_errors_name_the_platform() {
        let e = editor(json!({"gh_chat": true, "gh_chat_channel": "bad name!"}));
        let ids = e.nav_ids();
        let chat = ids.iter().position(|id| *id == "chat").unwrap();
        assert_eq!(ids[chat + 1], "misc");
        assert_eq!(e.section_icon("chat"), "fas fa-comment-alt");
        let invalid: Vec<String> = e.invalid_fields().into_iter().collect();
        assert_eq!(invalid, ["gh_chat_channel"]);
        assert_eq!(
            ExtrasEditor::error_key("gh_chat_channel"),
            "#L_GS_X_INVALID_TWITCH"
        );
        assert_eq!(
            ExtrasEditor::error_key("gh_chat_connect"),
            "#L_GS_X_INVALID_YOUTUBE"
        );
        assert_eq!(
            ExtrasEditor::error_key("gh_hud_font"),
            "#L_GS_X_INVALID_TEXT"
        );
    }

    #[test]
    fn sliders_snap_to_the_step_and_stick_to_the_default() {
        let e = editor(json!({}));
        let scale = e.schema.field("gh_hud_fontscale").unwrap();
        assert_eq!(ExtrasEditor::detent(&scale, 1.02, 300.0), 1.0);
        assert_eq!(ExtrasEditor::detent(&scale, 1.08, 300.0), 1.08);
        assert_eq!(ExtrasEditor::snap(&scale, 1.08), 1.1);
        assert_eq!(ExtrasEditor::snap(&scale, 1.02), 1.0);
        assert!(ExtrasEditor::has_range(&scale));
        let style = e.slider_style(&scale);
        assert_eq!(style.value, 20.0);
        assert_eq!(style.from, 20.0);
    }

    #[test]
    fn every_schema_section_has_a_preview_and_mods_take_the_width() {
        let mut e = editor(json!({}));
        // 0.4.9: у «Призраков» своя сцена превью — поля одной колонкой.
        e.select_section("ghosts");
        assert!(e.has_preview());
        assert_eq!(e.field_columns().len(), 1);
        e.select_section(MODS);
        assert!(!e.has_preview() && e.is_mods());
        assert!(e.field_columns().is_empty());
        assert_eq!(e.active_nav_id(), MODS);
        e.select_section("nope");
        assert_eq!(e.active_nav_id(), PERSONALIZATION);
        let ids = e.nav_ids();
        let mods = ids.iter().position(|id| *id == MODS).unwrap();
        assert_eq!(ids[mods + 1], LIVESPLIT);
        assert_eq!(e.nav_items()[mods].title, "#L_CPK_TAB_GHOSTING");
        assert!(hud_mod(
            Schema::Ghosting,
            Some(&json!([{"id": "condensed-hud", "enabled": true}]))
        ));
        assert!(!hud_mod(
            Schema::Goldsrc,
            Some(&json!([{"id": "condensed-hud", "enabled": true}]))
        ));
    }

    #[test]
    fn a_model_outside_the_mod_has_its_own_error() {
        let mut e = editor(json!({}));
        let model = e.schema.field("gh_model").unwrap();
        assert_eq!(ExtrasEditor::asset_kind(&model), "model");
        let outside = json!({"status": "error", "code": "ASSET_OUTSIDE_MOD"});
        assert_eq!(e.begin_pick(&model), Some("model"));
        assert_eq!(e.begin_pick(&model), None);
        assert_eq!(e.asset_picked(&model, Ok(outside)), None);
        assert_eq!(e.asset_errors["gh_model"], "#L_GS_X_MODEL_OUTSIDE");
        e.begin_pick(&model);
        assert_eq!(e.asset_picked(&model, Ok(Value::Null)), None);
        assert!(e.asset_errors.is_empty());
        e.begin_pick(&model);
        assert_eq!(e.asset_picked(&model, Err("x".into())), None);
        assert_eq!(e.asset_errors["gh_model"], "#L_GS_X_PICK_ERROR");
    }

    #[test]
    fn nav_keys_wrap_around() {
        let mut e = editor(json!({}));
        assert_eq!(e.nav_key("ArrowUp"), Some(e.nav_ids().len() - 1));
        assert_eq!(e.nav_key("ArrowDown"), Some(1));
        assert_eq!(e.nav_key("Home"), Some(0));
        assert_eq!(e.nav_key("Tab"), None);
        e.select_section(SETTINGS_CODE);
        assert_eq!(e.nav_key("ArrowRight"), Some(0));
    }
}
