//! «Персонализация» (`PersonalizationPanel.vue` 0.4.7, задача 30): вкладки-пилюля
//! (Лаунчер, HL2: Ghosting, Half-Life) с точкой «не сохранено», карточки тем (тема
//! применяется сразу), черновики внешнего вида по игре с грязностью по снимку JSON, сохранить,
//! вернуть, сбросить, «сохранено» 2,4 с, импорт кода. Без окна: состояние; каналы
//! `game-settings-get/save` и таймеры ведёт адаптер и возвращает ответы сюда. Сверка —
//! `tests/personalization_logic.rs` по фикстуре `export-personalization-logic.js`.
//!
//! 0.4.9 (`GamePersonalization.vue`, редактор одной игры на странице «Персонализация»): сброс
//! делает черновик грязным до сохранения (`reset` уходит в `game-settings-save`), `configSha256`
//! файла игрока, конфликт `CONFIG_CHANGED` («Перечитать» / «Отмена»), перечитывание по фокусу.
//! Вкладки и темы 0.4.7 странице 0.4.9 не нужны (темы — в настройках лаунчера).

use crate::controllers::game_availability;
use hlsr_core::js;
use hlsr_core::json_text::stringify;
use hlsr_delivery::ghosting_personalization::default_personalization;
use hlsr_delivery::ghosting_settings;
use hlsr_delivery::goldsrc_settings;
use hlsr_delivery::livesplit_overlay;
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// Вкладка «Лаунчер» (темы).
pub const LAUNCHER_TAB: &str = "launcher";
/// `GHOSTING_GAME_ID`.
pub const GHOSTING_GAME_ID: &str = "218";
/// `GOLDSRC_GAME_IDS`.
pub const GOLDSRC_GAME_IDS: [&str; 1] = ["70"];
/// «Сохранено» держится 2 400 мс.
pub const SAVED_MS: u64 = 2400;
/// Превью игр готовятся через 800 мс после открытия раздела.
pub const PREPARE_MS: u64 = 800;

/// Вкладка панели: id, значок, подпись (ключ строки у «Лаунчера», имя игры у игр).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub id: &'static str,
    pub icon: &'static str,
    pub label: &'static str,
}

/// Игры с редактором внешнего вида (`GAME_TABS`).
const GAME_TABS: [Tab; 2] = [
    Tab {
        id: GHOSTING_GAME_ID,
        icon: "fas fa-ghost",
        label: "HL2: Ghosting",
    },
    Tab {
        id: "70",
        icon: "fas fa-crosshairs",
        label: "Half-Life",
    },
];

/// Карточка темы (`data/theme-swatches.json`: THEMES 0.4.7).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ThemeCard {
    pub id: String,
    pub title: String,
    pub hint: String,
    /// Фон окна, поверхность, акцент (CSS-цвета).
    pub swatch: [String; 3],
}

#[derive(serde::Deserialize)]
struct Swatches {
    themes: Vec<ThemeCard>,
}

/// Темы в порядке 0.4.7.
pub fn themes() -> Vec<ThemeCard> {
    let table: Swatches = serde_json::from_str(include_str!("../../data/theme-swatches.json"))
        .expect("data/theme-swatches.json");
    table.themes
}

/// `themeById(id).id`: незнакомая — первая.
pub fn theme_id_of(id: &str) -> String {
    let list = themes();
    list.iter()
        .find(|t| t.id == id)
        .or(list.first())
        .map(|t| t.id.clone())
        .unwrap_or_else(|| "hlsr".to_owned())
}

/// Черновик игры: всё, что вернул main, черновик внешнего вида и его снимок при загрузке.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub settings: Value,
    pub look: Value,
    pub saved: String,
    /// `configSha256` ответа (0.4.9): `None` — `undefined` (не уходит в `game-settings-save`).
    pub config_sha256: Option<Value>,
    /// После «Сбросить внешний вид» есть что сохранить, даже если вид и так стандартный.
    pub reset: bool,
}

impl Draft {
    /// `{settings, look, saved: snapshot(look), configSha256}`.
    fn of(settings: Value, config_sha256: Option<Value>) -> Draft {
        let look = pick_look(Some(&settings));
        Draft {
            saved: snapshot(&look),
            settings,
            look,
            config_sha256,
            reset: false,
        }
    }
}

/// Снимок для грязности (`JSON.stringify`).
pub fn snapshot(value: &Value) -> String {
    stringify(value, "")
}

/// `normalizePersonalization(value)`: умолчания под значениями, цвета — поканально.
pub fn normalize_personalization(value: Option<&Value>) -> Value {
    let defaults = default_personalization().to_value();
    let Some(Value::Object(source)) = value else {
        return defaults;
    };
    let mut result = match defaults {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    for (key, value) in source {
        result.insert(key.clone(), value.clone());
    }
    let merged = |base: [(&str, i64); 3], value: Option<&Value>| {
        let mut color = Map::new();
        for (key, channel) in base {
            color.insert(key.to_owned(), json!(channel));
        }
        if let Some(Value::Object(given)) = value.filter(|v| js::truthy(v)) {
            for (key, channel) in given {
                color.insert(key.clone(), channel.clone());
            }
        }
        Value::Object(color)
    };
    result.insert(
        "hudColor".into(),
        merged([("r", 255), ("g", 220), ("b", 0)], source.get("hudColor")),
    );
    result.insert(
        "crossColor".into(),
        merged([("r", 0), ("g", 255), ("b", 0)], source.get("crossColor")),
    );
    Value::Object(result)
}

/// `pickLook(settings)`: из настроек игры — только внешний вид.
pub fn pick_look(settings: Option<&Value>) -> Value {
    let null = Value::Null;
    let source = settings.filter(|s| js::truthy(s)).unwrap_or(&null);
    let truthy_or_null = |key: &str| match source.get(key) {
        Some(v) if js::truthy(v) => v.clone(),
        _ => Value::Null,
    };
    let mut look = Map::new();
    look.insert(
        "personalization".into(),
        normalize_personalization(source.get("personalization")),
    );
    look.insert("packs".into(), truthy_or_null("packs"));
    look.insert("bxt".into(), truthy_or_null("bxt"));
    look.insert(
        "liveSplitOverlay".into(),
        livesplit_overlay::sanitize_overlay(source.get("liveSplitOverlay").unwrap_or(&null))
            .to_value(),
    );
    Value::Object(look)
}

/// Примечание в подвале редактора (`<transition name="l-fade">`): ключ строки и тон.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    Invalid,
    /// Файл игрока изменён вне лаунчера (`CONFIG_CHANGED`, 0.4.9).
    Conflict,
    SaveError,
    Saved,
    CodeImported,
    CodeUnchanged,
    Dirty,
}

impl Note {
    pub fn key(self) -> &'static str {
        match self {
            Note::Invalid => "#L_GS_MOD_INVALID",
            Note::Conflict => "#L_GS_CFG_CHANGED",
            Note::SaveError => "#L_PZ_SAVE_FAILED",
            Note::Saved => "#L_PZ_SAVED",
            Note::CodeImported => "#L_PZ_CODE_IMPORTED",
            Note::CodeUnchanged => "#L_PZ_CODE_UNCHANGED",
            Note::Dirty => "#L_GS_X_APPLY_NOTE",
        }
    }

    /// Тон: 0 — обычный, 1 — ошибка, 2 — успех.
    pub fn tone(self) -> i32 {
        match self {
            Note::Invalid | Note::Conflict | Note::SaveError => 1,
            Note::Saved | Note::CodeImported | Note::CodeUnchanged => 2,
            Note::Dirty => 0,
        }
    }
}

/// Состояние панели (`data()`).
#[derive(Debug, Clone)]
pub struct PersonalizationPanel {
    pub active_tab: String,
    pub drafts: HashMap<String, Draft>,
    pub invalid: HashMap<String, bool>,
    pub saving: bool,
    pub save_error: bool,
    pub just_saved: String,
    pub code_imported: String,
    pub visible: bool,
    /// `slideDir`: `true` — next.
    pub slide_next: bool,
    pub current_theme: String,
    /// `conflictGameId` (0.4.9): игра, чей файл игрока изменён вне лаунчера.
    pub conflict_game_id: String,
    /// `focusToken`: перечитывание по фокусу принимает только последний ответ.
    focus_token: u64,
}

impl PersonalizationPanel {
    pub fn new(current_theme: &str) -> PersonalizationPanel {
        PersonalizationPanel {
            active_tab: LAUNCHER_TAB.to_owned(),
            drafts: HashMap::new(),
            invalid: HashMap::new(),
            saving: false,
            save_error: false,
            just_saved: String::new(),
            code_imported: String::new(),
            visible: true,
            slide_next: true,
            current_theme: theme_id_of(current_theme),
            conflict_game_id: String::new(),
            focus_token: 0,
        }
    }

    /// `tabs`: «Лаунчер» и доступные игры с редактором.
    pub fn tabs(&self) -> Vec<Tab> {
        let mut tabs = vec![Tab {
            id: LAUNCHER_TAB,
            icon: "fas fa-paint-brush",
            label: "#L_PZ_LAUNCHER",
        }];
        tabs.extend(
            GAME_TABS
                .iter()
                .filter(|tab| game_availability::is_available(tab.id))
                .cloned(),
        );
        tabs
    }

    pub fn tab_index(&self, id: &str) -> Option<usize> {
        self.tabs().iter().position(|t| t.id == id)
    }

    pub fn is_goldsrc(&self) -> bool {
        GOLDSRC_GAME_IDS.contains(&self.active_tab.as_str())
    }

    pub fn is_game_tab(&self) -> bool {
        self.active_tab != LAUNCHER_TAB
    }

    pub fn draft(&self) -> Option<&Draft> {
        self.drafts.get(&self.active_tab)
    }

    /// `editorValue`: у HL1 — персонализация плюс паки и BXT одним плоским `extra`.
    pub fn editor_value(&self) -> Option<Value> {
        let look = &self.draft()?.look;
        let personalization = look.get("personalization").cloned().unwrap_or(Value::Null);
        if !self.is_goldsrc() {
            return Some(personalization);
        }
        let mut value = match personalization {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        let null = Value::Null;
        value.insert(
            "extra".into(),
            goldsrc_settings::to_flat(
                look.get("packs").unwrap_or(&null),
                look.get("bxt").unwrap_or(&null),
            )
            .to_value(),
        );
        Some(Value::Object(value))
    }

    /// Оверлей LiveSplit черновика.
    pub fn overlay(&self) -> Option<Value> {
        self.draft()?.look.get("liveSplitOverlay").cloned()
    }

    /// `watch activeTab`: `Some(id)` — черновика нет, грузить (`game-settings-get`).
    pub fn set_active_tab(&mut self, id: &str) -> Option<String> {
        // watch срабатывает только на смену значения: та же вкладка — ничего.
        if id == self.active_tab {
            return None;
        }
        let ids: Vec<&str> = self.tabs().iter().map(|t| t.id).collect();
        let from = ids.iter().position(|t| *t == self.active_tab);
        let to = ids.iter().position(|t| *t == id);
        // direction(): правее или та же — next.
        self.slide_next = to.unwrap_or(0) >= from.unwrap_or(0);
        self.active_tab = id.to_owned();
        self.save_error = false;
        // `if (id !== "launcher" && !this.drafts[id]) this.load(id)`: и при уходе/возврате до
        // ответа — запрос снова (ответ, пришедший позже, заменит черновик, как в 0.4.7).
        if id != LAUNCHER_TAB && !self.drafts.contains_key(id) {
            return Some(id.to_owned());
        }
        None
    }

    pub fn is_loading(&self) -> bool {
        self.is_game_tab() && self.draft().is_none()
    }

    /// Ответ `game-settings-get` (`load`): отказ — как пустые настройки, но черновик, который уже
    /// есть (перечитывание по фокусу, «Перечитать»), отказ не заменяет: иначе «Сохранить» записало
    /// бы умолчания поверх вида игрока.
    pub fn loaded(&mut self, id: &str, reply: Result<Value, String>) {
        let ok = matches!(&reply, Ok(r) if r.get("status").and_then(Value::as_str) == Some("ok"));
        if !ok && self.drafts.contains_key(id) {
            return;
        }
        // `result && result.configSha256`: отказ канала (`catch → null`) — `null`.
        let config_sha256 = match &reply {
            Ok(result) if js::truthy(result) => result.get("configSha256").cloned(),
            _ => Some(Value::Null),
        };
        let settings = match reply {
            Ok(result)
                if result.get("status").and_then(Value::as_str) == Some("ok")
                    && result.get("settings").is_some_and(js::truthy) =>
            {
                result.get("settings").cloned().unwrap_or(json!({}))
            }
            _ => json!({}),
        };
        self.drafts
            .insert(id.to_owned(), Draft::of(settings, config_sha256));
    }

    /// `isDirty(id)`: снимок изменился или был «Сбросить».
    pub fn is_dirty(&self, id: &str) -> bool {
        self.drafts
            .get(id)
            .is_some_and(|d| snapshot(&d.look) != d.saved || d.reset)
    }

    /// `conflictShown`: конфликт файла этой игры (и вид не неверный).
    pub fn conflict_shown(&self) -> bool {
        !self.conflict_game_id.is_empty()
            && self.conflict_game_id == self.active_tab
            && !self.is_invalid()
    }

    /// «Отмена» у конфликта (`conflictGameId = ""`).
    pub fn dismiss_conflict(&mut self) {
        self.conflict_game_id.clear();
    }

    /// `reloadAfterChange()`: `Some(id)` — перечитать файл игрока (`load(id)`).
    pub fn reload_after_change(&mut self) -> Option<String> {
        if self.conflict_game_id.is_empty() {
            return None;
        }
        let id = std::mem::take(&mut self.conflict_game_id);
        self.save_error = false;
        if self.code_imported == id {
            self.code_imported.clear();
        }
        Some(id)
    }

    /// Простой: не сохраняется, конфликта нет, черновик загружен и не правлен.
    fn idle(&self, id: &str) -> bool {
        !self.saving
            && self.conflict_game_id.is_empty()
            && self.drafts.contains_key(id)
            && !self.is_dirty(id)
    }

    /// `onWindowFocus()`: `Some((id, токен))` — перечитать файл игрока; ответ — [`Self::focus_loaded`].
    pub fn begin_focus_load(&mut self) -> Option<(String, u64)> {
        let id = self.active_tab.clone();
        if !self.visible || !self.idle(&id) {
            return None;
        }
        self.focus_token += 1;
        Some((id, self.focus_token))
    }

    /// Ответ перечитывания по фокусу: принят, только если запрос последний и черновик не трогали.
    pub fn focus_loaded(&mut self, id: &str, token: u64, reply: Result<Value, String>) -> bool {
        if token != self.focus_token || !self.idle(id) {
            return false;
        }
        self.loaded(id, reply);
        true
    }

    fn look_mut(&mut self) -> Option<&mut Map<String, Value>> {
        let tab = self.active_tab.clone();
        match &mut self.drafts.get_mut(&tab)?.look {
            Value::Object(map) => Some(map),
            _ => None,
        }
    }

    /// `onEditorInput(value)`.
    pub fn on_editor_input(&mut self, value: Value) {
        self.code_imported.clear();
        let goldsrc = self.is_goldsrc();
        let Some(look) = self.look_mut() else {
            return;
        };
        if !goldsrc {
            look.insert("personalization".into(), value);
            return;
        }
        let split = goldsrc_settings::from_flat(value.get("extra").unwrap_or(&Value::Null));
        let previous_extra = look
            .get("personalization")
            .and_then(|p| p.get("extra"))
            .cloned()
            .unwrap_or(Value::Null);
        let mut personalization = match value {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        personalization.insert("extra".into(), previous_extra);
        look.insert("personalization".into(), Value::Object(personalization));
        look.insert("packs".into(), split.packs.to_value());
        look.insert("bxt".into(), split.bxt.to_value());
    }

    /// `onOverlayInput(value)`.
    pub fn on_overlay_input(&mut self, value: Value) {
        self.code_imported.clear();
        if let Some(look) = self.look_mut() {
            look.insert("liveSplitOverlay".into(), value);
        }
    }

    /// `resetGame()`: внешний вид — стандартный; cvar мода, которые лаунчер уже записал, — к
    /// умолчаниям; выключатель оверлея остаётся.
    pub fn reset_game(&mut self) {
        self.code_imported.clear();
        let goldsrc = self.is_goldsrc();
        let tab = self.active_tab.clone();
        if let Some(draft) = self.drafts.get_mut(&tab) {
            draft.reset = true;
        }
        let Some(look) = self.look_mut() else {
            return;
        };
        let extra = look
            .get("personalization")
            .and_then(|p| p.get("extra"))
            .cloned()
            .unwrap_or(Value::Null);
        let mut personalization = match default_personalization().to_value() {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        personalization.insert(
            "extra".into(),
            ghosting_settings::reset_extras(&extra).to_value(),
        );
        look.insert("personalization".into(), Value::Object(personalization));
        if goldsrc {
            look.insert("packs".into(), Value::Null);
            look.insert("bxt".into(), Value::Null);
        }
        let enabled = look
            .get("liveSplitOverlay")
            .filter(|o| js::truthy(o))
            .and_then(|o| o.get("enabled"))
            .is_some_and(js::truthy);
        let mut overlay = livesplit_overlay::default_overlay();
        overlay.enabled = enabled;
        look.insert("liveSplitOverlay".into(), overlay.to_value());
    }

    /// `revertGame()`: черновик отброшен — конфликт и ошибка сохранения к нему больше не относятся.
    pub fn revert_game(&mut self) {
        self.code_imported.clear();
        self.conflict_game_id.clear();
        self.save_error = false;
        let tab = self.active_tab.clone();
        if let Some(draft) = self.drafts.get_mut(&tab) {
            draft.reset = false;
            draft.look = serde_json::from_str(&draft.saved).unwrap_or(Value::Null);
        }
    }

    /// `importCodeLook({gameId, look})`: `true` — принято (выбрать раздел «Персонализация»).
    pub fn import_code_look(&mut self, game_id: &str, look: &Value) -> bool {
        if game_id != self.active_tab || self.draft().is_none() || self.saving {
            return false;
        }
        let tab = self.active_tab.clone();
        if let Some(draft) = self.drafts.get_mut(&tab)
            && let (Value::Object(current), Value::Object(patch)) = (&mut draft.look, look)
        {
            for (key, value) in patch {
                current.insert(key.clone(), value.clone());
            }
        }
        self.save_error = false;
        self.just_saved.clear();
        self.code_imported = tab;
        true
    }

    /// `save()` начинается: `Some((id, payload))` — звать `game-settings-save`.
    pub fn begin_save(&mut self) -> Option<(String, Value)> {
        let id = self.active_tab.clone();
        if self.saving || self.invalid.get(&id).copied().unwrap_or(false) {
            return None;
        }
        let payload = self.drafts.get(&id)?.look.clone();
        self.saving = true;
        // Устаревший ответ перечитывания по фокусу отбрасывается.
        self.focus_token += 1;
        self.save_error = false;
        self.conflict_game_id.clear();
        Some((id, payload))
    }

    /// Четвёртый аргумент `game-settings-save` (0.4.9): `{configSha256?, reset?}`, пусто — `None`.
    pub fn save_options(&self, id: &str) -> Option<Value> {
        let draft = self.drafts.get(id)?;
        let mut options = Map::new();
        if let Some(sha) = &draft.config_sha256 {
            options.insert("configSha256".into(), sha.clone());
        }
        if draft.reset {
            options.insert("reset".into(), json!(true));
        }
        (!options.is_empty()).then_some(Value::Object(options))
    }

    /// Ответ `game-settings-save`: `true` — сохранено (запустить таймер «сохранено»). Файл
    /// игрока правили вне лаунчера (`CONFIG_CHANGED`) — черновик остаётся до «Перечитать»/«Отмена».
    pub fn saved(&mut self, id: &str, payload: Value, reply: Result<Value, String>) -> bool {
        self.saving = false;
        let result = match reply {
            Ok(result) if result.get("status").and_then(Value::as_str) == Some("ok") => result,
            Ok(result)
                if result
                    .get("error")
                    .and_then(|e| e.get("code"))
                    .and_then(Value::as_str)
                    == Some("CONFIG_CHANGED") =>
            {
                self.conflict_game_id = id.to_owned();
                return false;
            }
            _ => {
                self.save_error = true;
                return false;
            }
        };
        let settings = match result.get("settings") {
            Some(s) if js::truthy(s) => s.clone(),
            _ => payload,
        };
        let config_sha256 = result.get("configSha256").cloned();
        self.drafts
            .insert(id.to_owned(), Draft::of(settings, config_sha256));
        self.just_saved = id.to_owned();
        self.code_imported.clear();
        true
    }

    /// Таймер «сохранено» вышел.
    pub fn saved_expired(&mut self) {
        self.just_saved.clear();
    }

    /// `@validity` редактора.
    pub fn set_validity(&mut self, valid: bool) {
        let tab = self.active_tab.clone();
        self.invalid.insert(tab, !valid);
    }

    pub fn is_invalid(&self) -> bool {
        self.invalid.get(&self.active_tab).copied().unwrap_or(false)
    }

    /// Примечание подвала для активной вкладки.
    pub fn note(&self) -> Option<Note> {
        let tab = self.active_tab.as_str();
        if self.is_invalid() {
            Some(Note::Invalid)
        } else if self.conflict_shown() {
            Some(Note::Conflict)
        } else if self.save_error {
            Some(Note::SaveError)
        } else if self.just_saved == tab {
            Some(Note::Saved)
        } else if self.code_imported == tab {
            Some(if self.is_dirty(tab) {
                Note::CodeImported
            } else {
                Note::CodeUnchanged
            })
        } else if self.is_dirty(tab) {
            Some(Note::Dirty)
        } else {
            None
        }
    }

    pub fn can_revert(&self) -> bool {
        self.is_dirty(&self.active_tab) && !self.saving
    }

    pub fn can_save(&self) -> bool {
        self.can_revert() && !self.is_invalid()
    }

    /// `setTheme(id)` оболочки: `Some(id)` — тема меняется (применить и записать в prefs).
    pub fn pick_theme(&mut self, id: &str) -> Option<String> {
        let next = theme_id_of(id);
        if next == self.current_theme {
            return None;
        }
        self.current_theme = next.clone();
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drafts_are_dirty_by_json_snapshot_and_revert_restores_them() {
        let mut panel = PersonalizationPanel::new("lambda");
        assert_eq!(panel.current_theme, "lambda");
        assert_eq!(panel.pick_theme("nope"), Some("hlsr".into()));
        assert_eq!(panel.pick_theme("hlsr"), None);
        assert_eq!(
            panel.tabs().iter().map(|t| t.id).collect::<Vec<_>>(),
            ["launcher", "218", "70"]
        );
        assert_eq!(panel.set_active_tab("218"), Some("218".into()));
        assert!(panel.is_loading());
        assert_eq!(panel.set_active_tab("218"), None);
        // Ушли и вернулись до ответа — запрос снова, как `load(id)` 0.4.7 (черновика ещё нет).
        assert_eq!(panel.set_active_tab("launcher"), None);
        assert_eq!(panel.set_active_tab("218"), Some("218".into()));
        panel.loaded(
            "218",
            Ok(json!({"status": "ok", "settings": {"personalization": {"hudEnabled": true}}})),
        );
        assert!(!panel.is_dirty("218"));
        let mut value = panel.editor_value().unwrap();
        assert_eq!(value["hudEnabled"], true);
        value["hudEnabled"] = json!(false);
        panel.on_editor_input(value);
        assert!(panel.is_dirty("218"));
        assert_eq!(panel.note(), Some(Note::Dirty));
        panel.revert_game();
        assert!(!panel.is_dirty("218"));
        assert_eq!(panel.note(), None);
        // Отказ канала — как пустые настройки.
        panel.set_active_tab("70");
        assert!(!panel.slide_next || panel.slide_next);
        panel.loaded("70", Err("x".into()));
        assert_eq!(panel.draft().unwrap().look["packs"], Value::Null);
        assert!(panel.is_goldsrc());
        // `toFlat(null, null)`: `sanitizeExtras({packHud: undefined, …})` пропускает поля без
        // значения — паков в плоском списке нет, редактор покажет умолчания схемы.
        assert_eq!(panel.editor_value().unwrap()["extra"], json!({}));
    }

    #[test]
    fn saving_replaces_the_snapshot_and_import_marks_the_tab() {
        let mut panel = PersonalizationPanel::new("hlsr");
        panel.set_active_tab("218");
        panel.loaded("218", Ok(json!({"status": "ok", "settings": {}})));
        let mut value = panel.editor_value().unwrap();
        value["hudEnabled"] = json!(true);
        panel.on_editor_input(value);
        let (id, payload) = panel.begin_save().unwrap();
        assert_eq!(id, "218");
        assert!(panel.saving && panel.begin_save().is_none());
        assert!(panel.saved(&id, payload.clone(), Ok(json!({"status": "ok"}))));
        assert!(!panel.is_dirty("218"));
        assert_eq!(panel.note(), Some(Note::Saved));
        panel.saved_expired();
        assert_eq!(panel.note(), None);
        assert!(panel.import_code_look("218", &json!({"personalization": {"hudEnabled": false}})));
        assert_eq!(panel.note(), Some(Note::CodeImported));
        assert!(!panel.import_code_look("70", &json!({})));
        panel.set_validity(false);
        assert_eq!(panel.note(), Some(Note::Invalid));
        assert!(panel.begin_save().is_none());
        panel.set_validity(true);
        let (id, payload) = panel.begin_save().unwrap();
        assert!(!panel.saved(&id, payload, Ok(json!({"status": "error"}))));
        assert_eq!(panel.note(), Some(Note::SaveError));
    }

    #[test]
    fn reset_conflict_and_focus_reload_follow_049() {
        let mut panel = PersonalizationPanel::new("hlsr");
        panel.set_active_tab("218");
        panel.loaded(
            "218",
            Ok(json!({"status": "ok", "settings": {}, "configSha256": "abc"})),
        );
        // Сброс на стандартном виде — всё равно есть что сохранить, и `reset` уходит в main.
        panel.reset_game();
        assert!(panel.is_dirty("218"));
        assert_eq!(
            panel.save_options("218"),
            Some(json!({"configSha256": "abc", "reset": true}))
        );
        // Перечитывание по фокусу при правках не идёт.
        assert_eq!(panel.begin_focus_load(), None);
        let (id, payload) = panel.begin_save().unwrap();
        let changed = json!({"status": "error", "error": {"code": "CONFIG_CHANGED"}});
        assert!(!panel.saved(&id, payload, Ok(changed)));
        assert_eq!(panel.note(), Some(Note::Conflict));
        assert!(!panel.save_error);
        assert_eq!(panel.reload_after_change(), Some("218".into()));
        assert_eq!(panel.note(), Some(Note::Dirty));
        panel.loaded("218", Ok(json!({"status": "ok", "settings": {}})));
        assert!(!panel.is_dirty("218"));
        assert_eq!(panel.save_options("218"), None);
        // Отказ при перечитывании черновик не заменяет.
        let kept = panel.draft().unwrap().look.clone();
        panel.loaded("218", Err("x".into()));
        panel.loaded("218", Ok(json!({"status": "error"})));
        assert_eq!(panel.draft().unwrap().look, kept);
        assert_eq!(panel.save_options("218"), None);
        // Отказ канала при первой загрузке: `configSha256: null` уходит в main.
        let mut fresh = PersonalizationPanel::new("hlsr");
        fresh.set_active_tab("218");
        fresh.loaded("218", Err("x".into()));
        assert_eq!(
            fresh.save_options("218"),
            Some(json!({"configSha256": null}))
        );
        // Устаревший ответ по фокусу отбрасывается.
        let (_, first) = panel.begin_focus_load().unwrap();
        let (_, second) = panel.begin_focus_load().unwrap();
        let reply =
            || Ok(json!({"status": "ok", "settings": {"personalization": {"hudEnabled": true}}}));
        assert!(!panel.focus_loaded("218", first, reply()));
        assert!(panel.focus_loaded("218", second, reply()));
        assert_eq!(
            panel.draft().unwrap().look["personalization"]["hudEnabled"],
            true
        );
    }
}
