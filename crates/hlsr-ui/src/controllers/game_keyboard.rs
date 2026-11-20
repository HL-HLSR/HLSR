//! Клавиатура окна настроек игры (`GameKeyboard.vue` 0.4.7): схема клавиш движка с биндами,
//! ромбами скриптов в Scriptless и отметками лаунчера, клавиши вне схемы, легенда, сведения о
//! клавише под указателем или выбранной, назначение — заготовки по категориям с поиском, своя
//! команда, режимы Scriptless/Scripted (`hlsr.keyboard.runMode` → `launcher-prefs.json`
//! `keyboardRunMode`), «вернуть» и «снять».
//!
//! [`Keyboard`] — данные компонента (`data()`), [`Board`] — вычисляемые свойства при данных окна
//! (черновик биндов, бинды cfg игры); действия, меняющие бинды, возвращают новый черновик
//! (`$emit("input", next)`: окно кладёт его в `draft.binds`).
//!
//! 0.4.9: схема целиком или только основной блок (`full`, `keyboardFull` в prefs), бинды, которые
//! cfg ставит по нажатию другой клавиши (`keyBinds.runtime` — свой цвет), «Пианино» (занята каждая
//! клавиша всей схемы в сохранённых биндах).

use super::bind_state::{self, Effective, KeysByCommand};
use crate::keyboard_layout::{BOARD_HEIGHT, Engine, KeyCap, key_name, layout_for, layout_of};
use hlsr_core::js::object::JsObject;
use hlsr_core::js::{self, Platform};
use hlsr_delivery::bind_presets::{self, AliasInfo, Catalog, Loc, Preset, command_key};
use hlsr_delivery::game_binds::{
    self, Bind, BindRead, CommandError, Override, Overrides, RuntimeBinds, clean_command,
    command_error,
};
use serde_json::Value;
use std::collections::HashMap;

/// Переводчик `$t(key, …args)`.
pub type Tr<'a> = &'a dyn Fn(&str, &[&str]) -> String;

/// Ключ режима в localStorage 0.4.7.
pub const RUN_MODE_KEY: &str = "hlsr.keyboard.runMode";

/// Режим рана HL1: в Scriptless скрипты не предлагаются и помечаются.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunMode {
    #[default]
    Scriptless,
    Scripted,
}

impl RunMode {
    pub fn id(self) -> &'static str {
        match self {
            RunMode::Scriptless => "scriptless",
            RunMode::Scripted => "scripted",
        }
    }

    /// `readRunMode()`: только «scripted» — Scripted.
    pub fn from_stored(value: Option<&str>) -> RunMode {
        if value == Some("scripted") {
            RunMode::Scripted
        } else {
            RunMode::Scriptless
        }
    }
}

/// Данные компонента.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keyboard {
    pub hovered: Option<String>,
    pub selected: Option<String>,
    pub custom: String,
    pub custom_error: Option<String>,
    pub query: String,
    pub category: Option<String>,
    pub run_mode: RunMode,
}

/// Клавиша схемы.
#[derive(Debug, Clone, PartialEq)]
pub struct CapView {
    pub id: String,
    pub key: Option<&'static str>,
    pub text: String,
    pub bind: String,
    pub script: bool,
    pub aria: String,
    pub tone: Option<String>,
    pub bound: bool,
    pub wide: bool,
    pub launcher: bool,
    pub selected: bool,
    pub dead: bool,
    /// Клавишу назначает cfg по нажатию другой (`is-runtime`).
    pub runtime: bool,
    /// Положение и размер в долях схемы (0…1).
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl CapView {
    /// Классы кнопки в порядке объекта `classes` 0.4.9.
    pub fn classes(&self) -> Vec<String> {
        let mut list = Vec::new();
        if let Some(tone) = &self.tone {
            list.push(format!("tone-{tone}"));
        }
        for (on, name) in [
            (self.bound, "is-bound"),
            (self.runtime, "is-runtime"),
            (self.wide, "is-wide"),
            (self.launcher, "is-launcher"),
            (self.selected, "is-selected"),
            (self.dead, "is-dead"),
        ] {
            if on {
                list.push(name.to_owned());
            }
        }
        list
    }
}

/// Пункт легенды с цветом.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegendItem {
    pub id: &'static str,
    pub tone: &'static str,
    pub title: String,
}

/// Клавиша вне схемы.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffBoard {
    pub key: String,
    pub name: String,
    pub label: String,
}

/// Категория в строке фильтра.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryView {
    pub id: String,
    pub tone: String,
    pub title: String,
}

/// Заготовка в группе.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionView {
    pub command: String,
    pub label: String,
    pub scripted: bool,
    pub practice: bool,
    pub keys: Vec<String>,
    pub title: String,
}

/// Группа заготовок категории.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupView {
    pub id: String,
    pub tone: String,
    pub title: String,
    pub actions: Vec<ActionView>,
}

/// Бинд, который ставит cfg по нажатию другой клавиши (`shown.runtime`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeShown {
    pub command: String,
    pub label: String,
    pub file: Option<String>,
    /// Имя клавиши, которая его ставит.
    pub trigger: String,
}

/// Сведения о клавише (`describe(key)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub key: String,
    pub name: String,
    pub command: String,
    pub label: String,
    pub tone: String,
    pub runtime: Vec<RuntimeShown>,
    pub note: String,
    pub script_warning: bool,
    pub practice: bool,
    pub source: Option<String>,
    pub overridden: bool,
    pub base_label: String,
    pub alias: Option<String>,
    pub custom_alias: bool,
    pub other_keys: Vec<String>,
}

/// Что окно знает о биндах (свойства компонента).
pub struct Inputs<'a> {
    /// `bindsGameId` (hl1-won, ghosting-renewed, hl2-4260…).
    pub game_id: &'a str,
    /// Черновик переназначений (`draft.binds`).
    pub value: &'a Value,
    /// Бинды cfg игры с несохранённым импортом (`shownKeyBinds`); `None` — игра не установлена.
    pub key_binds: Option<&'a BindRead>,
    /// Язык интерфейса: «en», «ru».
    pub lang: &'a str,
    /// ОС (каталоги заготовок: «save :» или «save /»).
    pub platform: Platform,
    pub t: Tr<'a>,
    /// Вся клавиатура или только основной блок (`full`).
    pub full: bool,
    /// Переназначения последнего сохранения (`savedValue`); `None` — сохранённое не известно.
    pub saved_value: Option<&'a Value>,
    /// Бинды cfg с диска без несохранённого импорта (`savedKeyBinds`).
    pub saved_key_binds: Option<&'a BindRead>,
}

/// `everyKeyBound` без остальной клавиатуры (`watch` «Пианино» на каждое изменение):
/// `saved_value` — `savedValue`, `saved_key_binds` — `savedKeyBinds`.
pub fn every_key_bound(
    game_id: &str,
    platform: Platform,
    saved_value: Option<&Value>,
    saved_key_binds: Option<&BindRead>,
) -> bool {
    let catalog = bind_presets::catalog_for(game_id, platform);
    let engine = Engine::from_name(catalog.map_or("goldsrc", |c| c.engine));
    every_bound(engine, saved_value, saved_key_binds)
}

fn every_bound(engine: Engine, saved: Option<&Value>, file: Option<&BindRead>) -> bool {
    let Some(saved) = saved else {
        return false;
    };
    let empty = JsObject::new();
    let file_binds = file.map_or(&empty, |binds| &binds.binds);
    let effective = bind_state::effective_binds(file_binds, &bind_state::overrides_of(saved));
    let mut keys = layout_for(engine).keys.iter().filter_map(|cap| cap.key);
    let mut any = false;
    keys.all(|key| {
        any = true;
        effective.get(key).is_some()
    }) && any
}

/// Вычисляемые свойства клавиатуры при данных окна.
pub struct Board<'a> {
    kb: &'a Keyboard,
    inputs: &'a Inputs<'a>,
    pub catalog: Option<&'static Catalog>,
    engine: Engine,
    pub overrides: Overrides,
    file_binds: JsObject<Bind>,
    aliases: JsObject<String>,
    alias_info: AliasInfo,
    alias_names: HashMap<String, String>,
    pub effective: JsObject<Effective>,
    keys_by_command: KeysByCommand,
    tones: HashMap<String, String>,
    /// Бинды по нажатию другой клавиши: `{ key: [{ command, file, trigger }] }`.
    runtime: RuntimeBinds,
}

impl<'a> Board<'a> {
    pub fn new(kb: &'a Keyboard, inputs: &'a Inputs<'a>) -> Board<'a> {
        let catalog = bind_presets::catalog_for(inputs.game_id, inputs.platform);
        let engine = Engine::from_name(catalog.map_or("goldsrc", |c| c.engine));
        let overrides = bind_state::overrides_of(inputs.value);
        let file_binds = inputs
            .key_binds
            .map(|binds| binds.binds.clone())
            .unwrap_or_default();
        let aliases = inputs
            .key_binds
            .map(|binds| binds.aliases.clone())
            .unwrap_or_default();
        let effective = bind_state::effective_binds(&file_binds, &overrides);
        let keys_by_command = bind_state::keys_by_command(&effective);
        let mut alias_names = HashMap::new();
        for (name, value) in aliases.iter() {
            alias_names.insert(name.to_lowercase(), value.clone());
        }
        let tones = catalog
            .map(|catalog| {
                catalog
                    .categories
                    .iter()
                    .map(|item| (item.id.clone(), item.tone.clone()))
                    .collect()
            })
            .unwrap_or_default();
        Board {
            kb,
            inputs,
            catalog,
            engine,
            alias_info: AliasInfo::new(&aliases),
            overrides,
            file_binds,
            aliases,
            alias_names,
            effective,
            keys_by_command,
            tones,
            runtime: inputs
                .key_binds
                .map(|binds| binds.runtime.clone())
                .unwrap_or_default(),
        }
    }

    /// Схема на экране: вся или основной блок.
    fn layout(&self) -> &'static crate::keyboard_layout::Layout {
        layout_of(self.engine, self.inputs.full)
    }

    /// Ширина схемы на экране в u (`--kb-cols`).
    pub fn cols(&self) -> f64 {
        self.layout().width
    }

    /// `everyKeyBound` («Пианино»): в сохранённых биндах занята каждая клавиша всей схемы и
    /// каждая кнопка мыши с неё (MOUSE1–5, колесо), какой бы вид ни был на экране.
    pub fn every_key_bound(&self) -> bool {
        every_bound(
            self.engine,
            self.inputs.saved_value,
            self.inputs.saved_key_binds,
        )
    }

    fn t(&self, key: &str) -> String {
        (self.inputs.t)(key, &[])
    }

    fn key_name(&self, key: &str) -> String {
        key_name(key, |id| self.t(id))
    }

    /// `text(value)` для подписей каталога: язык интерфейса, иначе английский.
    pub fn text(&self, value: &Loc) -> String {
        let local = if self.inputs.lang == "ru" {
            &value.ru
        } else {
            &value.en
        };
        if local.is_empty() {
            value.en.clone()
        } else {
            local.clone()
        }
    }

    pub fn run_modes(&self) -> bool {
        self.catalog.is_some_and(|catalog| catalog.run_modes)
    }

    /// `scriptless`: режимы есть, выбран Scriptless.
    pub fn scriptless(&self) -> bool {
        self.run_modes() && self.kb.run_mode == RunMode::Scriptless
    }

    fn preset_of(&self, command: &str) -> Option<&'static Preset> {
        self.catalog.and_then(|catalog| catalog.preset_of(command))
    }

    fn alias_of(&self, word: &str) -> Option<&String> {
        self.alias_names.get(&word.to_lowercase())
    }

    /// `playerAlias(command)`: клавиша назначена на alias игрока (`{name, value}`), если он не
    /// объявлен в точности как в заготовке.
    fn player_alias(&self, command: &str) -> Option<(String, String)> {
        if command.is_empty() || command.chars().any(|c| js::is_whitespace(c) || c == ';') {
            return None;
        }
        let value = self.alias_of(command)?;
        let same = |a: &str, b: &str| collapse(a).to_lowercase() == collapse(b).to_lowercase();
        if let Some(preset) = self.preset_of(command)
            && preset.aliases.iter().any(|(name, text)| {
                name.to_lowercase() == command.to_lowercase() && same(text, value)
            })
        {
            return None;
        }
        Some((command.to_owned(), value.clone()))
    }

    /// `toneOf(command)`.
    fn tone_of(&self, command: &str) -> String {
        if self.player_alias(command).is_some() {
            return "alias".to_owned();
        }
        match self.preset_of(command) {
            Some(preset) => self
                .tones
                .get(&preset.category)
                .cloned()
                .unwrap_or_else(|| "other".to_owned()),
            None => "other".to_owned(),
        }
    }

    /// `labelOf(command)`.
    pub fn label_of(&self, command: &str) -> String {
        if self.player_alias(command).is_some() {
            return command.to_owned();
        }
        match self.preset_of(command) {
            Some(preset) => self.text(&preset.label),
            None => command.to_owned(),
        }
    }

    fn is_scripted(&self, command: &str) -> bool {
        bind_presets::is_scripted(self.catalog, command, &self.alias_info)
    }

    /// `isUnboundByLauncher(key)`: лаунчер снял бинд, который был в cfg.
    fn is_unbound_by_launcher(&self, key: &str) -> bool {
        self.overrides
            .get(key)
            .is_some_and(|entry| entry.command.is_empty() && !entry.base.is_empty())
    }

    /// Бинды по нажатию другой клавиши на клавише `key`.
    fn runtime_of(&self, key: &str) -> &[hlsr_delivery::game_binds::RuntimeBind] {
        self.runtime.get(key).map_or(&[], Vec::as_slice)
    }

    /// Клавиши схемы на экране (вся или основной блок).
    pub fn caps(&self) -> Vec<CapView> {
        let layout = self.layout();
        let cols = layout.width;
        let scriptless = self.scriptless();
        layout
            .keys
            .iter()
            .map(|cap: &KeyCap| {
                let bound = cap.key.and_then(|key| self.effective.get(key));
                // Пустая на старте клавиша, которую cfg назначит по нажатию другой: её бинд —
                // своим цветом.
                let later = match (cap.key, bound) {
                    (Some(key), None) => self.runtime_of(key).first(),
                    _ => None,
                };
                let tone = match (bound, later) {
                    (Some(bound), _) => Some(self.tone_of(&bound.command)),
                    (None, Some(_)) => Some("runtime".to_owned()),
                    (None, None) => None,
                };
                let text = if cap.label.starts_with('#') {
                    self.t(&cap.label)
                } else {
                    cap.label.clone()
                };
                let bind = match (bound, later) {
                    (Some(bound), _) => self.label_of(&bound.command),
                    (None, Some(later)) => self.label_of(&later.command),
                    (None, None) => String::new(),
                };
                let aria = match cap.key {
                    Some(key) => format!(
                        "{}: {}",
                        self.key_name(key),
                        if bound.is_some() || later.is_some() {
                            bind.clone()
                        } else {
                            self.t("#L_KB_UNBOUND")
                        }
                    ),
                    None => text.clone(),
                };
                CapView {
                    id: cap.id.clone(),
                    key: cap.key,
                    script: bound
                        .is_some_and(|bound| scriptless && self.is_scripted(&bound.command)),
                    launcher: bound.is_some_and(|bound| bound.overridden)
                        || cap.key.is_some_and(|key| self.is_unbound_by_launcher(key)),
                    selected: cap
                        .key
                        .is_some_and(|key| self.kb.selected.as_deref() == Some(key)),
                    bound: bound.is_some(),
                    runtime: cap.key.is_some_and(|key| !self.runtime_of(key).is_empty()),
                    wide: cap.w >= 2.0,
                    dead: cap.key.is_none(),
                    tone,
                    text,
                    bind,
                    aria,
                    x: cap.x / cols,
                    y: cap.y / BOARD_HEIGHT,
                    w: cap.w / cols,
                    h: cap.h / BOARD_HEIGHT,
                }
            })
            .collect()
    }

    /// Забинженные клавиши, которых нет на всей схеме («~», «+»…), по имени.
    pub fn off_board(&self) -> Vec<OffBoard> {
        let layout = layout_for(self.engine);
        let mut keys: Vec<&str> = self
            .effective
            .keys()
            .filter(|key| !layout.has(key))
            .collect();
        keys.sort_by(|a, b| js::cmp_utf16(a, b));
        keys.into_iter()
            .map(|key| OffBoard {
                key: key.to_owned(),
                name: self.key_name(key),
                label: self.label_of(&self.effective.get(key).expect("клавиша из итога").command),
            })
            .collect()
    }

    /// Легенда: «прочее»; если на клавишах есть alias игрока — «alias»; если на схеме экрана
    /// есть клавиша с биндом по нажатию — «по нажатию».
    pub fn legend(&self) -> Vec<LegendItem> {
        let mut result = vec![LegendItem {
            id: "other",
            tone: "other",
            title: self.t("#L_KB_G_OTHER"),
        }];
        if self
            .effective
            .values()
            .any(|bound| self.player_alias(&bound.command).is_some())
        {
            result.push(LegendItem {
                id: "alias",
                tone: "alias",
                title: self.t("#L_KB_LEGEND_ALIAS"),
            });
        }
        if self
            .layout()
            .keys
            .iter()
            .any(|cap| cap.key.is_some_and(|key| !self.runtime_of(key).is_empty()))
        {
            result.push(LegendItem {
                id: "runtime",
                tone: "runtime",
                title: self.t("#L_KB_LEGEND_RUNTIME"),
            });
        }
        result
    }

    /// Категории с заготовками для списка (скрипты гайда назначаются во вкладке «Скрипты»).
    pub fn listed_categories(&self) -> Vec<CategoryView> {
        let Some(catalog) = self.catalog else {
            return Vec::new();
        };
        catalog
            .categories
            .iter()
            .filter(|item| {
                catalog
                    .presets
                    .iter()
                    .any(|preset| preset.category == item.id && preset.listed)
            })
            .map(|item| CategoryView {
                id: item.id.clone(),
                tone: item.tone.clone(),
                title: self.text(&item.title),
            })
            .collect()
    }

    fn matches(preset: &Preset, needle: &str) -> bool {
        let mut parts = vec![
            preset.command.as_str(),
            preset.label.ru.as_str(),
            preset.label.en.as_str(),
        ];
        if let Some(note) = &preset.note {
            parts.push(&note.ru);
            parts.push(&note.en);
        }
        let haystack = parts.join(" ").to_lowercase();
        split_whitespace(needle)
            .iter()
            .all(|word| haystack.contains(word.as_str()))
    }

    /// Группы заготовок по категориям: фильтр категории, режима и поиска.
    pub fn groups(&self) -> Vec<GroupView> {
        let Some(catalog) = self.catalog else {
            return Vec::new();
        };
        let needle = js::trim(&self.kb.query).to_lowercase();
        let scriptless = self.scriptless();
        let mut result = Vec::new();
        for item in &catalog.categories {
            if self
                .kb
                .category
                .as_deref()
                .is_some_and(|category| category != item.id)
            {
                continue;
            }
            let actions: Vec<ActionView> = catalog
                .presets
                .iter()
                .filter(|preset| preset.category == item.id && preset.listed)
                .filter(|preset| !(scriptless && preset.scripted))
                .filter(|preset| needle.is_empty() || Self::matches(preset, &needle))
                .map(|preset| {
                    let keys: Vec<String> = self
                        .keys_by_command
                        .get(&command_key(&preset.command))
                        .map(|keys| keys.iter().map(|key| self.key_name(key)).collect())
                        .unwrap_or_default();
                    let note = preset
                        .note
                        .as_ref()
                        .map_or_else(String::new, |note| self.text(note));
                    let joined = keys.join(", ");
                    let title = [preset.command.as_str(), note.as_str(), joined.as_str()]
                        .into_iter()
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ");
                    ActionView {
                        command: preset.command.clone(),
                        label: self.text(&preset.label),
                        scripted: catalog.run_modes && preset.scripted,
                        practice: preset.practice,
                        keys,
                        title,
                    }
                })
                .collect();
            if !actions.is_empty() {
                result.push(GroupView {
                    id: item.id.clone(),
                    tone: item.tone.clone(),
                    title: self.text(&item.title),
                    actions,
                });
            }
        }
        result
    }

    /// `describe(key)`: что на клавише и откуда.
    pub fn describe(&self, key: &str) -> Shown {
        let bound = self.effective.get(key);
        let command = bound.map_or("", |bound| bound.command.as_str());
        let others: Vec<String> = if command.is_empty() {
            Vec::new()
        } else {
            self.keys_by_command
                .get(&command_key(command))
                .map(|keys| keys.iter().filter(|other| *other != key).cloned().collect())
                .unwrap_or_default()
        };
        let word = command.split(' ').next().unwrap_or("");
        let custom = self.player_alias(command);
        let preset = if !command.is_empty() && custom.is_none() {
            self.preset_of(command)
        } else {
            None
        };
        let alias = match &custom {
            Some((_, value)) => Some(value.clone()),
            None => {
                let plain = !command.is_empty()
                    && !command.contains(' ')
                    && !preset.is_some_and(|preset| !preset.aliases.is_empty());
                if plain {
                    self.alias_of(word).cloned()
                } else {
                    None
                }
            }
        };
        let runtime: Vec<RuntimeShown> = self
            .runtime_of(key)
            .iter()
            .map(|item| RuntimeShown {
                command: item.command.clone(),
                label: self.label_of(&item.command),
                file: item.file.clone(),
                trigger: self.key_name(&item.trigger),
            })
            .collect();
        Shown {
            key: key.to_owned(),
            name: self.key_name(key),
            command: command.to_owned(),
            label: match runtime.first() {
                _ if !command.is_empty() => self.label_of(command),
                Some(first) => first.label.clone(),
                None => String::new(),
            },
            tone: match runtime.first() {
                _ if !command.is_empty() => self.tone_of(command),
                Some(_) => "runtime".to_owned(),
                None => "none".to_owned(),
            },
            runtime,
            note: preset
                .and_then(|preset| preset.note.as_ref())
                .map_or_else(String::new, |note| self.text(note)),
            script_warning: !command.is_empty() && self.scriptless() && self.is_scripted(command),
            practice: preset.is_some_and(|preset| preset.practice),
            source: bound.and_then(|bound| bound.source.clone()),
            overridden: bound.is_some_and(|bound| bound.overridden)
                || self.is_unbound_by_launcher(key),
            base_label: match self.overrides.get(key) {
                Some(entry) if !entry.base.is_empty() => self.label_of(&entry.base),
                _ => String::new(),
            },
            alias,
            custom_alias: custom.is_some(),
            other_keys: others.iter().map(|other| self.key_name(other)).collect(),
        }
    }

    /// Клавиша под указателем, иначе выбранная.
    pub fn shown(&self) -> Option<Shown> {
        let key = self.kb.hovered.as_ref().or(self.kb.selected.as_ref())?;
        Some(self.describe(key))
    }

    /// Выбранная клавиша.
    pub fn shown_selected(&self) -> Option<Shown> {
        self.kb.selected.as_ref().map(|key| self.describe(key))
    }

    /// Сведения показывают выбранную клавишу (кнопки «снять» и «вернуть»).
    pub fn is_selected_shown(&self) -> bool {
        self.kb.selected.is_some()
            && self
                .kb
                .hovered
                .as_ref()
                .is_none_or(|hovered| Some(hovered) == self.kb.selected.as_ref())
    }

    pub fn selected_name(&self) -> String {
        self.kb
            .selected
            .as_ref()
            .map_or_else(String::new, |key| self.key_name(key))
    }

    /// Заготовка подсвечена: она стоит на выбранной клавише.
    pub fn chip_active(&self, shown_selected: Option<&Shown>, command: &str) -> bool {
        shown_selected.is_some_and(|shown| {
            !shown.key.is_empty() && command_key(&shown.command) == command_key(command)
        })
    }

    fn base_of(&self, key: &str) -> String {
        bind_state::base_of(key, &self.file_binds, &self.overrides)
    }

    /// Черновик с новым назначением выбранной клавиши.
    fn assigned(&self, key: &str, command: &str) -> Value {
        let mut next = self.overrides.clone();
        next.insert(
            key,
            Override {
                command: clean_command(command),
                base: self.base_of(key),
            },
        );
        game_binds::overrides_to_value(&next)
    }

    /// Alias игрока (cfg) — для тестов и сведений.
    pub fn aliases(&self) -> &JsObject<String> {
        &self.aliases
    }
}

/// `String(a).replace(/\s+/g, " ").trim()`.
fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if js::is_whitespace(c) {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(c);
            space = false;
        }
    }
    js::trim(&out).to_owned()
}

/// `needle.split(/\s+/)` для обрезанной строки.
fn split_whitespace(text: &str) -> Vec<String> {
    let mut words = vec![String::new()];
    let mut space = false;
    for c in text.chars() {
        if js::is_whitespace(c) {
            if !space {
                words.push(String::new());
            }
            space = true;
        } else {
            words.last_mut().expect("слово").push(c);
            space = false;
        }
    }
    words
}

impl Keyboard {
    pub fn new(run_mode: RunMode) -> Keyboard {
        Keyboard {
            run_mode,
            ..Keyboard::default()
        }
    }

    /// `watch gameId`: выбор, наведение, категория и поиск сбрасываются.
    pub fn reset_for_game(&mut self) {
        self.selected = None;
        self.hovered = None;
        self.category = None;
        self.query.clear();
    }

    /// Нажатие клавиши схемы или клавиши вне схемы: выбрать или снять выбор.
    pub fn select(&mut self, key: Option<&str>) {
        let Some(key) = key.filter(|key| !key.is_empty()) else {
            return;
        };
        self.selected = if self.selected.as_deref() == Some(key) {
            None
        } else {
            Some(key.to_owned())
        };
        self.custom.clear();
        self.custom_error = None;
    }

    /// Ввод в поле своей команды (`@input="customError = null"`).
    pub fn set_custom(&mut self, text: &str) {
        self.custom = text.to_owned();
        self.custom_error = None;
    }

    /// Кнопка категории: «Все» (`None`) или категория; повторное нажатие снимает фильтр.
    pub fn toggle_category(&mut self, id: Option<&str>) {
        self.category = match id {
            None => None,
            Some(id) if self.category.as_deref() == Some(id) => None,
            Some(id) => Some(id.to_owned()),
        };
    }

    /// Назначить заготовку или пустую команду («снять») выбранной клавише.
    pub fn assign(&self, inputs: &Inputs<'_>, command: &str) -> Option<Value> {
        let key = self.selected.as_deref()?;
        Some(Board::new(self, inputs).assigned(key, command))
    }

    /// «Применить» свою команду: ошибка — текст под полем, иначе назначение и пустое поле.
    pub fn assign_custom(&mut self, inputs: &Inputs<'_>) -> Option<Value> {
        let command = clean_command(&self.custom);
        if let Some(error) = command_error(&command) {
            self.custom_error = Some((inputs.t)(
                if error == CommandError::TooLong {
                    "#L_KB_ERR_TOO_LONG"
                } else {
                    "#L_KB_ERR_CHARS"
                },
                &[],
            ));
            return None;
        }
        let next = self.assign(inputs, &command);
        self.custom.clear();
        next
    }

    /// «Применить» доступна: клавиша выбрана, поле не пустое.
    pub fn custom_enabled(&self) -> bool {
        self.selected.is_some() && !js::trim(&self.custom).is_empty()
    }

    /// «Вернуть»: запись остаётся возвратом, чтобы игра снова получила прежний бинд.
    pub fn revert(&self, inputs: &Inputs<'_>) -> Option<Value> {
        let key = self.selected.as_deref()?;
        let mut overrides = bind_state::overrides_of(inputs.value);
        let entry = overrides.get(key)?.clone();
        overrides.insert(
            key,
            Override {
                command: entry.base.clone(),
                base: entry.base,
            },
        );
        Some(game_binds::overrides_to_value(&overrides))
    }
}

/// Заготовка в группе с подсветкой: она стоит на выбранной клавише.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChipView {
    pub action: ActionView,
    pub active: bool,
}

/// Группа для показа.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupShown {
    pub id: String,
    pub tone: String,
    pub title: String,
    pub chips: Vec<ChipView>,
}

/// Клавиша вне схемы для показа.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffBoardShown {
    pub item: OffBoard,
    pub active: bool,
}

/// Всё, что показывает клавиатура.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyboardView {
    /// Есть режимы Scriptless/Scripted (HL1).
    pub run_modes: bool,
    pub run_mode: RunMode,
    /// В легенде — ромб «скрипт» (режимы есть, выбран Scriptless).
    pub scriptless: bool,
    /// Игра не установлена: биндов cfg нет.
    pub not_installed: bool,
    pub legend: Vec<LegendItem>,
    pub off_board: Vec<OffBoardShown>,
    pub caps: Vec<CapView>,
    pub shown: Option<Shown>,
    pub is_selected_shown: bool,
    pub selected: Option<String>,
    /// Заголовок назначения: «Назначить на F» или «Назначить».
    pub title: String,
    pub custom: String,
    pub custom_error: String,
    pub custom_enabled: bool,
    pub query: String,
    pub category: Option<String>,
    pub categories: Vec<CategoryView>,
    pub groups: Vec<GroupShown>,
    /// Вся клавиатура (`full`) и ширина схемы на экране в u (`--kb-cols`).
    pub full: bool,
    pub cols: f64,
    /// «Пианино»: в сохранённых биндах занята вся схема.
    pub every_key_bound: bool,
}

impl Keyboard {
    /// Состояние для показа.
    pub fn view(&self, inputs: &Inputs<'_>) -> KeyboardView {
        let board = Board::new(self, inputs);
        let shown_selected = board.shown_selected();
        let groups = board
            .groups()
            .into_iter()
            .map(|group| GroupShown {
                chips: group
                    .actions
                    .into_iter()
                    .map(|action| ChipView {
                        active: board.chip_active(shown_selected.as_ref(), &action.command),
                        action,
                    })
                    .collect(),
                id: group.id,
                tone: group.tone,
                title: group.title,
            })
            .collect();
        let title = match &self.selected {
            Some(_) => (inputs.t)("#L_KB_ASSIGN_TO", &[&board.selected_name()]),
            None => (inputs.t)("#L_KB_ASSIGN", &[]),
        };
        KeyboardView {
            run_modes: board.run_modes(),
            run_mode: self.run_mode,
            scriptless: board.scriptless(),
            not_installed: inputs.key_binds.is_none(),
            legend: board.legend(),
            off_board: board
                .off_board()
                .into_iter()
                .map(|item| OffBoardShown {
                    active: self.selected.as_deref() == Some(item.key.as_str()),
                    item,
                })
                .collect(),
            caps: board.caps(),
            shown: board.shown(),
            is_selected_shown: board.is_selected_shown(),
            selected: self.selected.clone(),
            title,
            custom: self.custom.clone(),
            custom_error: self.custom_error.clone().unwrap_or_default(),
            custom_enabled: self.custom_enabled(),
            query: self.query.clone(),
            category: self.category.clone(),
            categories: board.listed_categories(),
            groups,
            full: inputs.full,
            cols: board.cols(),
            every_key_bound: board.every_key_bound(),
        }
    }
}

/// Где хранится режим рана (`localStorage["hlsr.keyboard.runMode"]` 0.4.7 →
/// `launcher-prefs.json` `keyboardRunMode`).
pub trait KeyboardPrefs {
    fn run_mode(&self) -> Option<String>;
    fn set_run_mode(&self, mode: &str);
    /// `prefsStore.load().keyboardFull` 0.4.9.
    fn keyboard_full(&self) -> bool;
    /// `prefsStore.setKeyboardFull(full)`.
    fn set_keyboard_full(&self, full: bool);
}

/// Режим в памяти окна (сцены, тесты; до подключения настоящих prefs — задача 39).
#[derive(Debug, Default)]
pub struct MemoryPrefs(std::cell::RefCell<Option<String>>, std::cell::Cell<bool>);

impl MemoryPrefs {
    pub fn with(mode: Option<&str>) -> MemoryPrefs {
        MemoryPrefs(
            std::cell::RefCell::new(mode.map(str::to_owned)),
            std::cell::Cell::new(false),
        )
    }
}

impl KeyboardPrefs for MemoryPrefs {
    fn run_mode(&self) -> Option<String> {
        self.0.borrow().clone()
    }

    fn set_run_mode(&self, mode: &str) {
        *self.0.borrow_mut() = Some(mode.to_owned());
    }

    fn keyboard_full(&self) -> bool {
        self.1.get()
    }

    fn set_keyboard_full(&self, full: bool) {
        self.1.set(full);
    }
}
