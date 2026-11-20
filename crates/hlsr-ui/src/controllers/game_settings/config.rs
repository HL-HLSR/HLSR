//! «Конфиг»: понятные настройки игры вместо переменных cfg (`GameConfigEditor.vue` 0.4.9) и
//! строка над ними (`GameSettingsModal.vue`: сборка, папка, предупреждение, пустое состояние).
//! Вкладки «Файлы cfg», «Проверка» (HL1) и «Игра» — `config_tabs`; «Игра» — все разделы
//! каталога одной прокруткой с якорями.
//!
//! Значения cfg приходят из `game-config-read` строками (`values`), правки окна — в единицах
//! окна (`edits`): переключатель — булево, число — число, проценты — 0…100. Правка, равная
//! значению в cfg, правкой не считается (`set`).

use super::super::jsv;
use super::draft::{Item, Tr};
use crate::controllers::cfg_check::{self, Badge, RuleCounts};
pub use crate::controllers::config_tabs::{CHECK_ID, FILES_ID, GAME_ID};
use crate::controllers::config_tabs::{ConfigTab, config_tabs};
use hlsr_core::js;
use hlsr_core::language::Language;
use hlsr_delivery::game_config::{
    self, Catalog, CatalogSection, Field, FieldType, GENERATED_FILES, Label, WindowValue,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// Насколько близко к отметке значения по умолчанию, в пикселях, ползунок к ней прилипает
/// (`SLIDER_DETENT_PX`).
pub const SLIDER_DETENT_PX: f64 = 6.0;
/// Файлы, куда лаунчер пишет сам: их имя возле настройки ничего не говорит (`PLAIN_FILES`).
const PLAIN_FILES: [&str; 1] = ["config.cfg"];

/// Правки окна: переменная → значение.
pub type Edits = Map<String, Value>;

/// Значение окна как JSON.
pub fn to_json(value: WindowValue) -> Value {
    match value {
        WindowValue::Bool(on) => Value::Bool(on),
        WindowValue::Num(n) => jsv::number_value(n),
    }
}

/// JSON → значение окна (правка).
pub fn from_json(value: &Value) -> Option<WindowValue> {
    match value {
        Value::Bool(on) => Some(WindowValue::Bool(*on)),
        other => js::value::number(other).map(WindowValue::Num),
    }
}

/// Вкладка в строке над содержимым.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabItem {
    pub id: &'static str,
    pub icon: &'static str,
    pub title: String,
    /// Сколько настроек вкладки изменено и не сохранено (точка).
    pub edits: usize,
}

/// Вид поля (`field.type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `toggle`, `invert`.
    Switch,
    /// `range`.
    Range,
    /// `percent`.
    Percent,
    Select,
}

/// Поле раздела, готовое к показу.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldView {
    pub cvar: String,
    pub kind: Kind,
    pub label: String,
    pub hint: String,
    /// Свой cfg игрока, откуда взято значение (`ownFile`).
    pub source: String,
    /// Подсказка у имени файла (`#L_GS_CFG_SOURCE_HINT` / `_LOCKED`).
    pub source_title: String,
    /// Файл, который лаунчер на месте не правит (`isLocked`).
    pub locked: bool,
    /// Переменная задана в файле игрока несколько раз (`isShadowed`).
    pub shadowed: bool,
    pub edited: bool,
    /// Поле недоступно, пока не включена другая настройка (`!isAvailable`).
    pub off: bool,
    /// «Действует только вместе с «…»».
    pub needs: String,
    pub checked: bool,
    /// Текст в поле числа (`String(current)`).
    pub text: String,
    pub value: f64,
    pub minimum: f64,
    pub maximum: f64,
    pub step: f64,
    pub default_value: f64,
    pub is_default: bool,
    pub has_scale: bool,
    pub scale_start: String,
    pub scale_end: String,
    /// Подсказка ползунка («По умолчанию: …»).
    pub slider_title: String,
    pub options: Vec<Item>,
    pub select_value: String,
    pub select_label: String,
}

/// Раздел «Игры»: заголовок, сброс и две колонки полей.
#[derive(Debug, Clone, PartialEq)]
pub struct SectionView {
    pub id: &'static str,
    pub icon: &'static str,
    pub title: String,
    /// Несохранённых правок в разделе (точка у якоря).
    pub edits: usize,
    /// «Вернуть значения по умолчанию» доступна (`sectionNonDefault`, не `disabled`).
    pub reset_enabled: bool,
    pub columns: [Vec<FieldView>; 2],
}

/// Редактор настроек, готовый к показу.
#[derive(Debug, Clone, PartialEq)]
pub struct EditorView {
    pub tabs: Vec<TabItem>,
    /// Показанная вкладка (`activeSection`).
    pub active: &'static str,
    /// Нажатая вкладка (`activeId`): подсветка только у неё.
    pub selected: Option<&'static str>,
    /// Значок «Проверки» (`checkBadge`).
    pub badge: Option<Badge>,
    pub disabled: bool,
    /// Заметка «Игры» (`#L_GS_CFG_NOTE`).
    pub note: String,
    /// Разделы «Игры» (пусто, если она не показана).
    pub sections: Vec<SectionView>,
}

/// Редактор настроек одной сборки.
#[derive(Debug)]
pub struct ConfigEditor {
    pub game_id: String,
    pub engine: String,
    /// Значения из cfg (`values`).
    pub values: Map<String, Value>,
    /// Файлы, откуда взяты значения (`sources`).
    pub sources: Map<String, Value>,
    /// Переменные, заданные в файле игрока несколько раз (`shadowed`, нижний регистр).
    pub shadowed: Vec<String>,
    /// Файл игрока (`playerFile`: userconfig.cfg / autoexec.cfg).
    pub player_file: String,
    /// Игра запущена: править нельзя (`disabled`).
    pub disabled: bool,
    /// Окно передало редактор файлов cfg (слот `files`): первая вкладка.
    pub has_files: bool,
    catalog: Option<Catalog>,
    active_id: Option<String>,
    sliding: HashMap<String, f64>,
}

fn label_text(label: &Label, lang: Language) -> &'static str {
    // `value[lang] || value.en || ""`
    let own = match lang {
        Language::Ru => label.ru,
        Language::En => label.en,
    };
    if own.is_empty() { label.en } else { own }
}

impl ConfigEditor {
    /// `game-config-read` с `status: "ok"`.
    pub fn from_state(state: &Value, has_files: bool) -> Option<ConfigEditor> {
        let game_id = jsv::str_of(jsv::prop(state, "gameId"))?.to_owned();
        let engine = jsv::str_of(jsv::prop(state, "engine"))?.to_owned();
        let object = |name: &str| match jsv::prop(state, name) {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        };
        let shadowed = jsv::prop(state, "shadowed")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Some(ConfigEditor {
            catalog: game_config::catalog_for(&game_id, &engine),
            game_id,
            engine,
            values: object("values"),
            sources: object("sources"),
            shadowed,
            player_file: jsv::str_of(jsv::prop(state, "playerFile"))
                .unwrap_or_default()
                .to_owned(),
            disabled: jsv::truthy(jsv::prop(state, "running")),
            has_files,
            active_id: None,
            sliding: HashMap::new(),
        })
    }

    /// Перечитанный редактор той же игры: компонент с `:key="gameId"` не пересоздаётся, его
    /// `activeId` и `sliding` остаются.
    pub fn keep_view(&mut self, previous: ConfigEditor) {
        if previous.game_id == self.game_id {
            self.active_id = previous.active_id;
            self.sliding = previous.sliding;
        }
    }

    pub fn catalog(&self) -> Option<&Catalog> {
        self.catalog.as_ref()
    }

    pub(super) fn field_by_cvar(&self, cvar: &str) -> Option<&'static Field> {
        self.catalog.as_ref()?.by_cvar(&cvar.to_lowercase())
    }

    /// `navSections`: «Файлы cfg», «Проверка» (есть итог проверки), «Игра».
    pub fn tabs(&self, check: bool) -> Vec<ConfigTab<'_>> {
        config_tabs(self.catalog.as_ref(), self.has_files, check)
    }

    /// `activeSection`: выбранная вкладка, иначе «Игра», иначе первая.
    pub fn active_tab(&self, check: bool) -> Option<&'static str> {
        let tabs = self.tabs(check);
        let found = |id: &str| tabs.iter().find(|tab| tab.id == id).map(|tab| tab.id);
        self.active_id
            .as_deref()
            .and_then(found)
            .or_else(|| found(GAME_ID))
            .or_else(|| tabs.first().map(|tab| tab.id))
    }

    /// Нажатая вкладка (`activeId === section.id`): `.active` только у неё; пока ничего не
    /// нажато, показана «Игра», но не подсвечена ни одна (так в 0.4.9).
    pub fn selected_tab(&self, check: bool) -> Option<&'static str> {
        let id = self.active_id.as_deref()?;
        self.tabs(check)
            .iter()
            .find(|tab| tab.id == id)
            .map(|tab| tab.id)
    }

    /// Вкладка показывает редактор файлов.
    pub fn files_active(&self, check: bool) -> bool {
        self.active_tab(check) == Some(FILES_ID)
    }

    /// `selectSection(id)`: вопрос о несохранённом тексте «Файлов cfg» задаёт место до вызова.
    pub fn select_section(&mut self, id: &str) {
        self.active_id = Some(id.to_owned());
    }

    fn section(&self, section_id: &str) -> Option<&CatalogSection> {
        self.catalog
            .as_ref()?
            .sections
            .iter()
            .find(|s| s.section.id == section_id)
    }

    /// Поля раздела: две независимые колонки (`columns`).
    pub fn columns(&self, section_id: &str) -> [Vec<&'static Field>; 2] {
        let fields = self
            .section(section_id)
            .map(|s| s.fields.clone())
            .unwrap_or_default();
        let middle = fields.len().div_ceil(2);
        [fields[..middle].to_vec(), fields[middle..].to_vec()]
    }

    /// Поля всех разделов каталога (для проб и тестов).
    pub fn all_fields(&self) -> Vec<&'static Field> {
        self.catalog
            .as_ref()
            .map(|catalog| catalog.fields.clone())
            .unwrap_or_default()
    }

    /// `isShadowed`: верхние строки перекрыты нижней.
    pub fn is_shadowed(&self, field: &Field) -> bool {
        let cvar = field.cvar.to_lowercase();
        self.shadowed.contains(&cvar)
    }

    /// Вкладки, разделы «Игры» и значок проверки для показа.
    pub fn editor_view(
        &self,
        edits: &Edits,
        lang: Language,
        tr: Tr<'_>,
        counts: Option<RuleCounts>,
    ) -> EditorView {
        let check = counts.is_some();
        let edited = |fields: &[&'static Field]| {
            fields
                .iter()
                .filter(|field| edits.contains_key(field.cvar))
                .count()
        };
        let tabs = self.tabs(check);
        let active = self.active_tab(check).unwrap_or(GAME_ID);
        let sections = tabs
            .iter()
            .find(|tab| tab.id == active && tab.id == GAME_ID)
            .map(|tab| {
                tab.sections
                    .iter()
                    .map(|section| {
                        let id = section.section.id;
                        let view = |fields: &Vec<&'static Field>| {
                            fields
                                .iter()
                                .map(|field| self.view(edits, field, lang, tr))
                                .collect::<Vec<_>>()
                        };
                        let [left, right] = self.columns(id);
                        SectionView {
                            id,
                            icon: section.section.icon,
                            title: label_text(&section.section.title, lang).to_owned(),
                            edits: edited(&section.fields),
                            reset_enabled: !self.disabled
                                && self.section_non_default(edits, id) > 0,
                            columns: [view(&left), view(&right)],
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let player = if self.player_file.is_empty() {
            "userconfig.cfg"
        } else {
            &self.player_file
        };
        EditorView {
            tabs: tabs
                .iter()
                .map(|tab| TabItem {
                    id: tab.id,
                    icon: tab.icon,
                    title: label_text(&tab.title, lang).to_owned(),
                    edits: edited(&tab.fields),
                })
                .collect(),
            active,
            selected: self.selected_tab(check),
            badge: cfg_check::badge(counts, tr),
            disabled: self.disabled,
            note: tr("#L_GS_CFG_NOTE", &[player]),
            sections,
        }
    }

    // ---- значения -------------------------------------------------------------------------

    fn raw(&self, field: &Field) -> Option<&str> {
        self.values.get(field.cvar).and_then(Value::as_str)
    }

    /// Значение, записанное в cfg (`fromCfg`).
    pub fn stored(&self, field: &Field) -> Option<WindowValue> {
        game_config::from_cfg(field, self.raw(field))
    }

    /// Значение, которое игра получит сейчас: из cfg или по умолчанию (`saved`).
    pub fn saved(&self, field: &Field) -> WindowValue {
        self.stored(field)
            .unwrap_or_else(|| game_config::default_value(field))
    }

    pub fn is_edited(&self, edits: &Edits, field: &Field) -> bool {
        edits.contains_key(field.cvar)
    }

    /// `current(field)`: правка или записанное значение.
    pub fn current(&self, edits: &Edits, field: &Field) -> WindowValue {
        edits
            .get(field.cvar)
            .and_then(from_json)
            .unwrap_or_else(|| self.saved(field))
    }

    pub fn is_default(&self, edits: &Edits, field: &Field) -> bool {
        self.current(edits, field) == game_config::default_value(field)
    }

    /// `ownFile`: файл, откуда взято значение, если это не файл лаунчера и не файл игрока
    /// (правка уйдёт туда же).
    pub fn own_file(&self, field: &Field) -> String {
        match self.sources.get(field.cvar).and_then(Value::as_str) {
            Some(file)
                if !file.is_empty()
                    && !PLAIN_FILES.contains(&file.to_lowercase().as_str())
                    && file.to_lowercase() != self.player_file.to_lowercase() =>
            {
                file.to_owned()
            }
            _ => String::new(),
        }
    }

    /// `isLocked`: файл, который лаунчер на месте не правит (WON.cfg, файлы лаунчера).
    pub fn is_locked(&self, field: &Field) -> bool {
        self.sources
            .get(field.cvar)
            .and_then(Value::as_str)
            .is_some_and(|file| {
                !file.is_empty() && GENERATED_FILES.contains(&file.to_lowercase().as_str())
            })
    }

    fn requirement(&self, field: &Field) -> Option<&'static Field> {
        self.field_by_cvar(field.requires?)
    }

    /// `isAvailable`: нет требования или требуемая настройка включена.
    pub fn is_available(&self, edits: &Edits, field: &Field) -> bool {
        match field.requires {
            None => true,
            Some(_) => self.requirement(field).is_some_and(|required| {
                match self.current(edits, required) {
                    WindowValue::Bool(on) => on,
                    WindowValue::Num(n) => n != 0.0 && !n.is_nan(),
                }
            }),
        }
    }

    // ---- правки ---------------------------------------------------------------------------

    /// `set(field, next)`: значение, равное записанному в cfg, — уже не правка.
    pub fn set(&self, edits: &mut Edits, field: &Field, next: WindowValue) {
        let stored = self.stored(field);
        let default = game_config::default_value(field);
        if Some(next) == stored || (stored.is_none() && next == default) {
            edits.shift_remove(field.cvar);
        } else {
            edits.insert(field.cvar.to_owned(), to_json(next));
        }
    }

    /// Привязка к шагу и пределам (`snap`).
    pub fn snap(field: &Field, raw: f64) -> f64 {
        let (min, step) = (min_of(field), step_of(field));
        let value = min + js::math_round((raw - min) / step) * step;
        let clamped = value.max(min).min(max_of(field));
        js::to_number(&js::to_fixed(clamped, 6))
    }

    /// `setNumber(field, input)`: `Some(текст)` — что показать в поле после правки (число
    /// принято или прежнее значение при пустом/неверном вводе).
    pub fn set_number(&self, edits: &mut Edits, field: &Field, text: &str) -> String {
        let raw = js::to_number(text);
        if text.is_empty() || !raw.is_finite() {
            return number_text(&self.current(edits, field));
        }
        let value = Self::snap(field, raw);
        self.set(edits, field, WindowValue::Num(value));
        js::number::number_to_string(value)
    }

    /// Ползунок тянут: куда он встал с учётом метки умолчания (`detent`). `width` — ширина
    /// ползунка, px.
    pub fn detent(field: &Field, raw: f64, width: f64) -> f64 {
        let base = match game_config::default_value(field) {
            WindowValue::Num(n) => n,
            WindowValue::Bool(_) => return raw,
        };
        let span = max_of(field) - min_of(field);
        if width == 0.0 || span == 0.0 {
            return raw;
        }
        if (raw - base).abs() / span * width <= SLIDER_DETENT_PX {
            base
        } else {
            raw
        }
    }

    pub fn slide(&mut self, field: &Field, raw: f64) {
        self.sliding.insert(field.cvar.to_owned(), raw);
    }

    pub fn end_slide(&mut self, field: &Field) {
        self.sliding.remove(field.cvar);
    }

    /// Значение ползунка: пока его тянут — за курсором (`sliderValue`).
    pub fn slider_value(&self, edits: &Edits, field: &Field) -> f64 {
        match self.sliding.get(field.cvar) {
            Some(raw) => *raw,
            None => match self.current(edits, field) {
                WindowValue::Num(n) => n,
                WindowValue::Bool(on) => f64::from(u8::from(on)),
            },
        }
    }

    /// `resetSection`: значения раздела — к умолчанию; то, что уже равно записанному, — не правка.
    pub fn reset_section(&self, edits: &mut Edits, section_id: &str) {
        let Some(section) = self.section(section_id) else {
            return;
        };
        for field in &section.fields {
            let base = game_config::default_value(field);
            let stored = self.stored(field);
            if stored == Some(base) || stored.is_none() {
                edits.shift_remove(field.cvar);
            } else {
                edits.insert(field.cvar.to_owned(), to_json(base));
            }
        }
    }

    /// Сколько настроек раздела не по умолчанию (`sectionNonDefault`).
    pub fn section_non_default(&self, edits: &Edits, section_id: &str) -> usize {
        self.section(section_id).map_or(0, |section| {
            section
                .fields
                .iter()
                .filter(|field| !self.is_default(edits, field))
                .count()
        })
    }

    // ---- показ ----------------------------------------------------------------------------

    /// Поле для показа в разметке.
    pub fn view(&self, edits: &Edits, field: &Field, lang: Language, tr: Tr<'_>) -> FieldView {
        let current = self.current(edits, field);
        let kind = match field.kind {
            FieldType::Toggle | FieldType::Invert => Kind::Switch,
            FieldType::Range => Kind::Range,
            FieldType::Percent => Kind::Percent,
            FieldType::Select => Kind::Select,
        };
        let (minimum, maximum, step) = (min_of(field), max_of(field), step_of(field));
        let options: Vec<Item> = field
            .options
            .iter()
            .map(|option| {
                Item::new(
                    js::number::number_to_string(option.value),
                    label_text(&option.label, lang),
                )
            })
            .collect();
        let select_value = number_text(&current);
        let select_label = options
            .iter()
            .find(|item| item.id == select_value)
            .map_or_else(|| select_value.clone(), |item| item.label.clone());
        let percent = field.kind == FieldType::Percent;
        let default_text = number_text(&game_config::default_value(field));
        let requirement = self.requirement(field);
        let available = self.is_available(edits, field);
        FieldView {
            cvar: field.cvar.to_owned(),
            kind,
            label: label_text(&field.label, lang).to_owned(),
            hint: field
                .hint
                .as_ref()
                .map(|hint| label_text(hint, lang).to_owned())
                .unwrap_or_default(),
            source_title: {
                let key = if self.is_locked(field) {
                    "#L_GS_CFG_SOURCE_LOCKED"
                } else {
                    "#L_GS_CFG_SOURCE_HINT"
                };
                let own = self.own_file(field);
                if own.is_empty() {
                    String::new()
                } else {
                    tr(key, &[&own])
                }
            },
            source: self.own_file(field),
            locked: self.is_locked(field),
            shadowed: self.is_shadowed(field),
            edited: self.is_edited(edits, field),
            off: !available,
            needs: match (available, requirement) {
                (false, Some(required)) => {
                    tr("#L_GS_X_REQUIRES", &[label_text(&required.label, lang)])
                }
                _ => String::new(),
            },
            checked: matches!(current, WindowValue::Bool(true)),
            text: number_text(&current),
            value: self.slider_value(edits, field),
            minimum,
            maximum,
            step,
            default_value: match game_config::default_value(field) {
                WindowValue::Num(n) => n,
                WindowValue::Bool(on) => f64::from(u8::from(on)),
            },
            is_default: self.is_default(edits, field),
            has_scale: field.scale.is_some(),
            scale_start: field
                .scale
                .as_ref()
                .map(|scale| label_text(&scale[0], lang).to_owned())
                .unwrap_or_default(),
            scale_end: field
                .scale
                .as_ref()
                .map(|scale| label_text(&scale[1], lang).to_owned())
                .unwrap_or_default(),
            slider_title: tr(
                "#L_GS_X_SLIDER_DEFAULT",
                &[&format!("{default_text}{}", if percent { "%" } else { "" })],
            ),
            options,
            select_value,
            select_label,
        }
    }
}

fn min_of(field: &Field) -> f64 {
    if field.kind == FieldType::Percent {
        0.0
    } else {
        field.min.unwrap_or(0.0)
    }
}

fn max_of(field: &Field) -> f64 {
    if field.kind == FieldType::Percent {
        100.0
    } else {
        field.max.unwrap_or(0.0)
    }
}

fn step_of(field: &Field) -> f64 {
    if field.kind == FieldType::Percent {
        1.0
    } else {
        field.step.filter(|step| *step != 0.0).unwrap_or(1.0)
    }
}

fn number_text(value: &WindowValue) -> String {
    match value {
        WindowValue::Num(n) => js::number::number_to_string(*n),
        WindowValue::Bool(on) => on.to_string(),
    }
}

// ---- строка над редактором -----------------------------------------------------------------

/// `configBuildItems`: сборки; отсутствующая помечена.
pub fn build_items(info: Option<&Value>, tr: Tr<'_>) -> Vec<Item> {
    let builds = jsv::field(info, "builds").and_then(Value::as_array);
    builds
        .into_iter()
        .flatten()
        .map(|item| {
            let build = jsv::str_of(jsv::prop(item, "build")).unwrap_or_default();
            let label = if jsv::truthy(jsv::prop(item, "installed")) {
                build.to_owned()
            } else {
                format!("{build} ({})", tr("#L_GS_CFG_BUILD_ABSENT", &[]))
            };
            Item::new(build, label)
        })
        .collect()
}

/// `configBuildLabel`.
pub fn build_label(items: &[Item], current: Option<&str>, tr: Tr<'_>) -> String {
    items
        .iter()
        .find(|item| Some(item.id.as_str()) == current)
        .map_or_else(|| tr("#L_NO_DATA", &[]), |item| item.label.clone())
}

/// `configPathText`.
pub fn path_text(info: Option<&Value>, tr: Tr<'_>) -> String {
    let Some(info) = info.filter(|info| js::truthy(info)) else {
        return tr("#L_NO_DATA", &[]);
    };
    match jsv::str_of(jsv::prop(info, "status")) {
        Some("not-installed") => tr("#L_GS_NOT_INSTALLED", &[]),
        Some("ok") => {
            if jsv::truthy(jsv::prop(info, "exists")) {
                jsv::str_of(jsv::prop(info, "path"))
                    .unwrap_or_default()
                    .to_owned()
            } else {
                tr("#L_GS_CFG_MISSING", &[])
            }
        }
        _ => tr("#L_NO_DATA", &[]),
    }
}

/// `configEmptyText`: пока нет редактора и нет файлов cfg.
pub fn empty_text(state: Option<&Value>, tr: Tr<'_>) -> String {
    match state.and_then(|state| jsv::str_of(jsv::prop(state, "status"))) {
        None => tr("#L_GS_LOADING", &[]),
        Some("not-installed") => tr("#L_GS_NOT_INSTALLED", &[]),
        Some(_) => tr("#L_NO_DATA", &[]),
    }
}

/// `configErrorText(result)`: ответ `game-config-write` с ошибкой.
pub fn error_text(result: Option<&Value>, tr: Tr<'_>) -> String {
    let code = result.and_then(|result| jsv::prop(result, "code"));
    match jsv::str_of(code) {
        Some("GAME_RUNNING") => tr("#L_GS_CFG_RUNNING", &[]),
        Some("NOT_INSTALLED") => tr("#L_GS_NOT_INSTALLED", &[]),
        _ => {
            let code = code
                .filter(|code| js::truthy(code))
                .map(js::to_string)
                .unwrap_or_default();
            tr("#L_GS_CFG_SAVE_FAILED", &[&code])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tr(key: &str, args: &[&str]) -> String {
        let mut text = key.to_owned();
        for arg in args {
            text = format!("{text}({arg})");
        }
        text
    }

    fn editor_of(
        game: &str,
        engine: &str,
        values: Value,
        sources: Value,
        running: bool,
    ) -> ConfigEditor {
        ConfigEditor::from_state(
            &json!({"status": "ok", "gameId": game, "engine": engine, "values": values, "sources": sources, "running": running}),
            true,
        )
        .unwrap()
    }

    /// Ghosting Renewed (Source).
    fn editor(values: Value, sources: Value, running: bool) -> ConfigEditor {
        editor_of("ghosting-renewed", "source", values, sources, running)
    }

    fn field(editor: &ConfigEditor, cvar: &str) -> &'static Field {
        editor
            .field_by_cvar(cvar)
            .unwrap_or_else(|| panic!("{cvar}"))
    }

    // GameConfigEditor.vue: чувствительность 3 из cfg → правка 2.5 → возврат к 3 убирает правку.
    #[test]
    fn an_edit_equal_to_the_cfg_value_is_not_an_edit() {
        let editor = editor(
            json!({"sensitivity": "3"}),
            json!({"sensitivity": "autoexec.cfg"}),
            false,
        );
        let sensitivity = field(&editor, "sensitivity");
        let mut edits = Edits::new();
        assert_eq!(editor.current(&edits, sensitivity), WindowValue::Num(3.0));
        editor.set(&mut edits, sensitivity, WindowValue::Num(2.5));
        assert_eq!(edits.get("sensitivity"), Some(&json!(2.5)));
        assert!(editor.is_edited(&edits, sensitivity));
        assert_eq!(editor.current(&edits, sensitivity), WindowValue::Num(2.5));
        editor.set(&mut edits, sensitivity, WindowValue::Num(3.0));
        assert!(edits.is_empty());
    }

    #[test]
    fn without_a_cfg_value_the_default_is_not_an_edit_either() {
        let editor = editor(json!({}), json!({}), false);
        let sensitivity = field(&editor, "sensitivity");
        let mut edits = Edits::new();
        assert_eq!(
            editor.saved(sensitivity),
            game_config::default_value(sensitivity)
        );
        editor.set(
            &mut edits,
            sensitivity,
            game_config::default_value(sensitivity),
        );
        assert!(edits.is_empty());
        editor.set(&mut edits, sensitivity, WindowValue::Num(7.0));
        assert_eq!(edits.len(), 1);
    }

    #[test]
    fn number_input_snaps_to_the_step_and_reverts_garbage() {
        let editor = editor(json!({"sensitivity": "3"}), json!({}), false);
        let sensitivity = field(&editor, "sensitivity");
        let mut edits = Edits::new();
        // 0,01 — шаг; 2,504 → 2,5; больше максимума — прижимается.
        assert_eq!(editor.set_number(&mut edits, sensitivity, "2.504"), "2.5");
        assert_eq!(edits.get("sensitivity"), Some(&json!(2.5)));
        assert_eq!(editor.set_number(&mut edits, sensitivity, "99"), "10");
        assert_eq!(edits.get("sensitivity"), Some(&json!(10)));
        // Пустое поле и не число — прежнее значение, правка не меняется.
        assert_eq!(editor.set_number(&mut edits, sensitivity, ""), "10");
        assert_eq!(editor.set_number(&mut edits, sensitivity, "abc"), "10");
        assert_eq!(edits.get("sensitivity"), Some(&json!(10)));
    }

    #[test]
    fn slider_detent_pulls_to_the_default_mark_within_six_pixels() {
        let editor = editor(json!({}), json!({}), false);
        let sensitivity = field(&editor, "sensitivity");
        // 0,1…10, умолчание 3, ширина 300 px: 9,9 единиц на 300 px, 6 px ≈ 0,198 единицы.
        assert_eq!(ConfigEditor::detent(sensitivity, 3.15, 300.0), 3.0);
        assert_eq!(ConfigEditor::detent(sensitivity, 3.5, 300.0), 3.5);
        assert_eq!(ConfigEditor::detent(sensitivity, 3.5, 0.0), 3.5);
        // Привязка по шагу в `slide`.
        assert_eq!(ConfigEditor::snap(sensitivity, 3.504), 3.5);
        assert_eq!(ConfigEditor::snap(sensitivity, -1.0), 0.1);
    }

    #[test]
    fn locked_and_plain_sources_are_told_apart() {
        // HL1: fps_max задаёт WON.cfg пакета — его лаунчер не правит.
        let editor = editor_of(
            "hl1-won",
            "goldsrc",
            json!({"sensitivity": "3", "volume": "0.5", "fps_max": "100"}),
            json!({"sensitivity": "config.cfg", "volume": "autoexec.cfg", "fps_max": "WON.cfg"}),
            false,
        );
        assert_eq!(
            editor.own_file(field(&editor, "sensitivity")),
            "",
            "файл лаунчера не показывается"
        );
        assert_eq!(editor.own_file(field(&editor, "volume")), "autoexec.cfg");
        let fps = field(&editor, "fps_max");
        assert!(editor.is_locked(fps));
        assert_eq!(editor.own_file(fps), "WON.cfg");
        assert!(!editor.is_locked(field(&editor, "volume")));
    }

    #[test]
    fn tabs_are_files_check_and_game_and_the_game_shows_every_section() {
        let mut editor = editor_of(
            "hl1-won",
            "goldsrc",
            json!({"sensitivity": "3"}),
            json!({"sensitivity": "userconfig.cfg", "volume": "autoexec.cfg"}),
            false,
        );
        editor.player_file = "userconfig.cfg".to_owned();
        editor.shadowed = vec!["sensitivity".to_owned()];
        let mut edits = Edits::new();
        let ids = |view: &EditorView| view.tabs.iter().map(|t| t.id).collect::<Vec<_>>();
        // Без итога проверки «Проверки» нет; показана «Игра», подсвечена ни одна.
        let view = editor.editor_view(&edits, Language::En, &tr, None);
        assert_eq!(ids(&view), [FILES_ID, GAME_ID]);
        assert_eq!(view.tabs[0].title, "cfg files");
        assert_eq!((view.active, view.selected), (GAME_ID, None));
        assert!(view.badge.is_none());
        assert_eq!(view.note, "#L_GS_CFG_NOTE(userconfig.cfg)");
        let catalog = editor.catalog().unwrap();
        assert_eq!(view.sections.len(), catalog.sections.len());
        let first = &view.sections[0];
        let [left, right] = &first.columns;
        assert!(left.len() >= right.len() && left.len() - right.len() <= 1);
        // Файл игрока не показывается как «свой файл»; перекрытая переменная отмечена.
        let sensitivity = view
            .sections
            .iter()
            .flat_map(|s| s.columns.iter().flatten())
            .find(|f| f.cvar == "sensitivity")
            .unwrap();
        assert_eq!(sensitivity.source, "");
        assert!(sensitivity.shadowed);
        let volume = view
            .sections
            .iter()
            .flat_map(|s| s.columns.iter().flatten())
            .find(|f| f.cvar == "volume")
            .unwrap();
        assert_eq!(volume.source, "autoexec.cfg");
        assert_eq!(volume.source_title, "#L_GS_CFG_SOURCE_HINT(autoexec.cfg)");
        // Правка — точка у вкладки «Игра» и у раздела.
        let field = field(&editor, "sensitivity");
        editor.set(&mut edits, field, WindowValue::Num(5.0));
        let counts = Some(RuleCounts {
            errors: 1,
            warnings: 0,
            failed: false,
        });
        let view = editor.editor_view(&edits, Language::En, &tr, counts);
        assert_eq!(ids(&view), [FILES_ID, CHECK_ID, GAME_ID]);
        assert_eq!(view.tabs[2].edits, 1);
        let section = view
            .sections
            .iter()
            .find(|s| s.id == field.section)
            .unwrap();
        assert_eq!(section.edits, 1);
        assert!(section.reset_enabled);
        assert_eq!(view.badge.as_ref().map(|b| b.count), Some(1));
        // Нажатая вкладка — показана и подсвечена; у «Файлов» разделов нет.
        editor.select_section(FILES_ID);
        let view = editor.editor_view(&edits, Language::En, &tr, counts);
        assert_eq!((view.active, view.selected), (FILES_ID, Some(FILES_ID)));
        assert!(view.sections.is_empty());
        assert!(editor.files_active(true));
        // «Проверка» ушла (итога нет) — снова «Игра».
        editor.select_section(CHECK_ID);
        assert_eq!(editor.active_tab(false), Some(GAME_ID));
        assert_eq!(editor.selected_tab(false), None);
    }

    #[test]
    fn reread_editor_of_the_same_game_keeps_the_tab() {
        let reread = || editor_of("hl1-won", "goldsrc", json!({}), json!({}), false);
        let on_files = || {
            let mut editor = reread();
            editor.select_section(FILES_ID);
            editor
        };
        let mut same = reread();
        same.keep_view(on_files());
        assert!(
            same.files_active(false),
            "после сохранения cfg вкладка «Файлы cfg» остаётся"
        );
        // Вкладки «Проверка» после перечитывания нет: не подсвечена ни одна, показана «Игра».
        let mut gone = reread();
        gone.keep_view({
            let mut editor = reread();
            editor.select_section(CHECK_ID);
            editor
        });
        assert_eq!(gone.selected_tab(false), None);
        assert_eq!(gone.active_tab(false), Some(GAME_ID));
        let mut other = editor(json!({}), json!({}), false);
        other.keep_view(on_files());
        assert_eq!(
            other.selected_tab(false),
            None,
            "у другой игры вкладка своя"
        );
    }

    #[test]
    fn reset_section_returns_the_section_to_its_defaults() {
        let editor = editor(json!({"sensitivity": "5"}), json!({}), false);
        let sensitivity = field(&editor, "sensitivity");
        let mut edits = Edits::new();
        editor.set(&mut edits, sensitivity, WindowValue::Num(1.0));
        editor.reset_section(&mut edits, sensitivity.section);
        // cfg хранит 5, умолчание 3 → правка «вернуть 3».
        assert_eq!(edits.get("sensitivity"), Some(&json!(3)));
        assert_eq!(editor.section_non_default(&edits, sensitivity.section), 0);
        // cfg хранит умолчание — правки нет.
        let editor = self::editor(json!({"sensitivity": "3"}), json!({}), false);
        let mut edits = Edits::new();
        editor.set(
            &mut edits,
            field(&editor, "sensitivity"),
            WindowValue::Num(1.0),
        );
        editor.reset_section(&mut edits, "controls");
        assert!(!edits.contains_key("sensitivity"));
    }

    #[test]
    fn dependent_settings_are_off_until_the_required_one_is_on() {
        let editor = editor(json!({}), json!({}), false);
        let dependent: Vec<&Field> = editor
            .all_fields()
            .into_iter()
            .filter(|field| field.requires.is_some())
            .collect();
        for field in dependent {
            let required = editor
                .requirement(field)
                .expect("требуемая настройка в каталоге");
            let mut edits = Edits::new();
            let on = !matches!(
                game_config::default_value(required),
                WindowValue::Bool(true)
            );
            let view = editor.view(&edits, field, Language::En, &tr);
            if matches!(
                game_config::default_value(required),
                WindowValue::Bool(false)
            ) {
                assert!(view.off, "{}", field.cvar);
                assert!(view.needs.starts_with("#L_GS_X_REQUIRES("));
                editor.set(&mut edits, required, WindowValue::Bool(on));
                assert!(editor.is_available(&edits, field));
            }
        }
    }

    #[test]
    fn view_of_a_select_shows_the_label_or_the_raw_value() {
        let editor = editor(json!({}), json!({}), false);
        let Some(select) = editor
            .all_fields()
            .into_iter()
            .find(|field| field.kind == FieldType::Select)
        else {
            return;
        };
        let edits = Edits::new();
        let view = editor.view(&edits, select, Language::Ru, &tr);
        assert_eq!(view.kind, Kind::Select);
        assert!(!view.options.is_empty());
        assert!(
            view.options
                .iter()
                .any(|item| item.label == view.select_label)
        );
    }

    #[test]
    fn the_row_above_the_editor_shows_build_path_and_empty_states() {
        let info = json!({"status": "ok", "path": "C:\\HLSR\\hl2-4260\\hl2\\cfg", "exists": true,
            "builds": [{"build": "4260", "installed": true}, {"build": "2707", "installed": false}]});
        let items = build_items(Some(&info), &tr);
        assert_eq!(items[0], Item::new("4260", "4260"));
        assert_eq!(items[1], Item::new("2707", "2707 (#L_GS_CFG_BUILD_ABSENT)"));
        assert_eq!(
            build_label(&items, Some("2707"), &tr),
            "2707 (#L_GS_CFG_BUILD_ABSENT)"
        );
        assert_eq!(build_label(&items, None, &tr), "#L_NO_DATA");
        assert_eq!(path_text(Some(&info), &tr), "C:\\HLSR\\hl2-4260\\hl2\\cfg");
        assert_eq!(
            path_text(Some(&json!({"status": "ok", "exists": false})), &tr),
            "#L_GS_CFG_MISSING"
        );
        assert_eq!(
            path_text(Some(&json!({"status": "not-installed"})), &tr),
            "#L_GS_NOT_INSTALLED"
        );
        assert_eq!(
            path_text(Some(&json!({"status": "error"})), &tr),
            "#L_NO_DATA"
        );
        assert_eq!(path_text(None, &tr), "#L_NO_DATA");
        assert_eq!(empty_text(None, &tr), "#L_GS_LOADING");
        assert_eq!(
            empty_text(Some(&json!({"status": "not-installed"})), &tr),
            "#L_GS_NOT_INSTALLED"
        );
        assert_eq!(
            empty_text(Some(&json!({"status": "error"})), &tr),
            "#L_NO_DATA"
        );
    }

    #[test]
    fn write_errors_map_to_texts() {
        let case = |value: Value| error_text(Some(&value), &tr);
        assert_eq!(
            case(json!({"status": "error", "code": "GAME_RUNNING"})),
            "#L_GS_CFG_RUNNING"
        );
        assert_eq!(
            case(json!({"code": "NOT_INSTALLED"})),
            "#L_GS_NOT_INSTALLED"
        );
        assert_eq!(
            case(json!({"code": "EACCES"})),
            "#L_GS_CFG_SAVE_FAILED(EACCES)"
        );
        assert_eq!(error_text(None, &tr), "#L_GS_CFG_SAVE_FAILED()");
    }

    #[test]
    fn a_running_game_disables_the_editor() {
        let editor = editor(json!({}), json!({}), true);
        assert!(editor.disabled);
        assert!(ConfigEditor::from_state(&json!({"status": "ok"}), false).is_none());
    }
}
