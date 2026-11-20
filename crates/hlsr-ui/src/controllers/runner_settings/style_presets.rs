//! Выбор заготовки «подсветка ника + рамка» (`StylePresetPicker.vue` 0.4.9) в настройках
//! профиля. Право (`perks.styles`) и выбранную заготовку (`stylePreset`) присылает сервер в своём
//! профиле, список (`public-style-presets-get`) берёт раздел. Выбор уходит в
//! `runner-customize {stylePreset}` — 0 снимает заготовку; свой ответ, а не `request()`: тот
//! теряет код отказа.

use super::super::bridge::Tr;
use super::super::jsv::{is_str, prop, str_of, truthy};
use super::super::profile::format_date;
use super::super::wall_clock::WallClock;
use super::{Effect, SettingsState, Tag, send};
use serde_json::{Value, json};

/// Что сейчас: номер сохраняемой заготовки и сообщение под блоком.
#[derive(Debug, Clone, Default)]
pub struct Picker {
    pending: Option<f64>,
    message: Option<&'static str>,
    error: bool,
}

/// `mode`: сетка выбора, права нет или заготовок нет (сервер на PHP или админка их не завела).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    None,
    Grid,
    Locked,
}

/// Карточка сетки: «Без подсветки» (id 0) или заготовка.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Card {
    pub id: f64,
    pub title: String,
    /// CSS ника и рамки заготовки; пусто — эту часть она не меняет.
    pub nick_css: String,
    pub frame_css: String,
    pub selected: bool,
    pub pending: bool,
}

/// Всё, что показывает блок.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PickerView {
    /// Нечего показать (заготовок нет, подписки не было) — блока нет совсем.
    pub visible: bool,
    pub mode: Mode,
    /// Строка подписки (если подписка была) и активна ли она.
    pub subscriber_text: String,
    pub subscriber_active: bool,
    pub cards: Vec<Card>,
    /// Идёт сохранение (`aria-busy`; «Снять» выключена).
    pub busy: bool,
    /// Выбрана заготовка: без права её можно снять.
    pub removable: bool,
    /// Ник и аватар предпросмотра.
    pub name: String,
    pub avatar: String,
    pub subscriber_badge: bool,
    pub message: String,
    pub message_error: bool,
}

/// Отказы сервера со своим текстом; остальные — общий.
fn error_text(code: Option<&str>) -> &'static str {
    match code {
        Some("RUNNER_PERK_REQUIRED") => "#L_RUNNER_STYLE_PERK_REQUIRED",
        Some("RUNNER_PRESET_UNKNOWN") => "#L_RUNNER_STYLE_UNKNOWN",
        Some("RUNNER_NETWORK") => "#L_RUNNER_NETWORK",
        _ => "#L_RUNNER_STYLE_ERROR",
    }
}

impl SettingsState {
    fn picker_mode(&self) -> Mode {
        if self.style_presets.is_empty() {
            Mode::None
        } else if prop(&self.perks, "styles") == Some(&Value::Bool(true)) {
            Mode::Grid
        } else {
            Mode::Locked
        }
    }

    fn has_preset(&self, id: f64) -> bool {
        self.style_presets
            .iter()
            .any(|item| prop(item, "id").and_then(Value::as_f64) == Some(id))
    }

    /// `selected`: 0 — без подсветки; `None` — выбрана заготовка, которой в списке нет.
    fn selected_preset(&self) -> Option<f64> {
        match self.style_preset.filter(|id| *id != 0.0) {
            None => Some(0.0),
            Some(id) => self.has_preset(id).then_some(id),
        }
    }

    /// `choose(id)`: второй выбор, пока первый сохраняется, отсекается.
    pub fn choose_style(&mut self, id: f64) -> Vec<Effect> {
        if self.picker.pending.is_some() || Some(id) == self.selected_preset() {
            return Vec::new();
        }
        if id != 0.0 && (self.picker_mode() != Mode::Grid || !self.has_preset(id)) {
            return Vec::new();
        }
        self.picker.pending = Some(id);
        self.picker.message = None;
        send(
            Tag::Style,
            "runner-customize",
            vec![json!({"stylePreset": super::super::jsv::number_value(id)})],
        )
    }

    /// Ответ `runner-customize`: новый снимок профиля — окну; отказ — его текст, а скрытая или
    /// удалённая заготовка — ещё и список мимо кеша.
    pub(super) fn style_answer(&mut self, reply: Result<Value, String>) -> Vec<Effect> {
        self.picker.pending = None;
        let mut effects = Vec::new();
        let result = match reply {
            Ok(result) => result,
            Err(_) => {
                self.picker.error = true;
                self.picker.message = Some("#L_RUNNER_NETWORK");
                return effects;
            }
        };
        let snapshot = prop(&result, "snapshot").filter(|s| truthy(Some(*s)));
        if truthy(Some(&result))
            && !is_str(prop(&result, "status"), "error")
            && let Some(snapshot) = snapshot
        {
            effects.push(Effect::Updated(snapshot.clone()));
            self.picker.error = false;
            self.picker.message = Some("#L_RUNNER_STYLE_SAVED");
            return effects;
        }
        let code = str_of(prop(&result, "code"));
        self.picker.error = true;
        self.picker.message = Some(error_text(code));
        if code == Some("RUNNER_PRESET_UNKNOWN") {
            effects.extend(self.load_presets(true));
        }
        effects
    }

    pub fn picker_view(&self, wall: &WallClock, tr: &Tr) -> PickerView {
        let t = |key: &str| tr(key, &[]);
        let mode = self.picker_mode();
        let subscriber = prop(&self.perks, "subscriber").filter(|v| v.is_object());
        let subscriber_active = subscriber.is_some_and(|s| truthy(prop(s, "active")));
        let subscriber_text = match subscriber {
            None => String::new(),
            Some(_) if !subscriber_active => t("#L_RUNNER_STYLE_SUB_ENDED"),
            Some(sub) => {
                let until = format_date(wall, str_of(prop(sub, "until")).unwrap_or_default());
                if until.is_empty() {
                    t("#L_RUNNER_STYLE_SUB_ACTIVE")
                } else {
                    tr("#L_RUNNER_STYLE_SUB_UNTIL", &[until.as_str()])
                }
            }
        };
        let selected = self.selected_preset();
        let pending = self.picker.pending;
        let css = |item: &Value, part: &str| {
            str_of(prop(item, part).and_then(|p| prop(p, "css")))
                .unwrap_or_default()
                .to_owned()
        };
        let cards = std::iter::once(Card {
            id: 0.0,
            title: t("#L_RUNNER_STYLE_NONE"),
            ..Card::default()
        })
        .chain(self.style_presets.iter().map(|item| Card {
            id: prop(item, "id").and_then(Value::as_f64).unwrap_or_default(),
            title: str_of(prop(item, "title")).unwrap_or_default().to_owned(),
            nick_css: css(item, "nick"),
            frame_css: css(item, "frame"),
            ..Card::default()
        }))
        .map(|card| Card {
            selected: Some(card.id) == selected,
            pending: Some(card.id) == pending,
            ..card
        })
        .collect();
        let name = str_of(prop(&self.account, "personaName"))
            .filter(|n| !n.is_empty())
            .map_or_else(|| t("#L_SB_STEAM_ACCOUNT"), str::to_owned);
        PickerView {
            visible: mode != Mode::None || !subscriber_text.is_empty(),
            mode,
            subscriber_active,
            subscriber_text,
            cards,
            busy: pending.is_some(),
            removable: self.style_preset.is_some_and(|id| id != 0.0),
            name,
            avatar: str_of(prop(&self.account, "avatarDataUri"))
                .unwrap_or_default()
                .to_owned(),
            subscriber_badge: self.subscriber_badge,
            message: self.picker.message.map_or(String::new(), t),
            message_error: self.picker.error,
        }
    }
}
