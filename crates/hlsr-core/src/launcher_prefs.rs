//! `launcher-prefs.json`: настройки лаунчера, которые 0.4.9 держал в localStorage
//! (`hlsr-launcher-prefs`, `hlsr.keyboard.runMode`, `wasLaunched`, `hlsr.pane.<key>` → `pane`).

use crate::paths::Paths;
use crate::store::Store;
use serde_json::{Map, Value, json};

pub const PREFS_FILE: &str = "launcher-prefs.json";

/// Ключи прежних версий, которые `launcher-prefs.js` удаляет при чтении и записи.
const REMOVED_KEYS: [&str; 10] = [
    "autoUpdate",
    "uiScale",
    "telemetry",
    "colorTheme",
    "installPath",
    "concurrent",
    "accent",
    "autostart",
    "tray",
    "onClose",
];

/// `SPEED_LIMITS` из `download-settings.js`.
const SPEED_LIMITS: [f64; 8] = [0.0, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0];

/// Ключи, которые [`LauncherPrefs::save`] берёт из файла, а не из снимка: `ruleCategory` и
/// `keyboardFull` пишут только свои функции, остальные в 0.4.9 — отдельные ключи localStorage.
const KEPT_ON_SAVE: [&str; 5] = [
    "ruleCategory",
    "keyboardFull",
    "keyboardRunMode",
    "wasLaunched",
    "pane",
];

/// Умолчания `launcher-prefs.js` 0.4.9 плюс `keyboardRunMode`, `wasLaunched` и `uncappedFps` (HLSR-CE).
pub fn prefs_defaults() -> Value {
    json!({
        "speedLimit": 0,
        "animations": true,
        "hideWhilePlaying": true,
        "fullscreenButton": true,
        "hideSteamId": false,
        "timerOffered": false,
        "toolSplits": {},
        "liveSplitGame": "",
        "coloredBoot": true,
        "launcherTheme": "hlsr",
        "paperAccent": "clay",
        "railExpanded": false,
        "welcomeSeen": false,
        "configTourSeen": false,
        "ruleCategory": {},
        "keyboardFull": false,
        "ghostingBuild": "HL1 MOVEMENT",
        "keyboardRunMode": "scriptless",
        "wasLaunched": false,
        // HLSR-CE: снять предел 60 кадров/с (`gpui::set_frame_cap`).
        "uncappedFps": false,
    })
}

/// Чистка `load()` из `launcher-prefs.js`: не объект → `{}`; прежние ключи
/// удаляются; `speedLimit` не из списка → 0; результат —
/// `Object.assign({}, DEFAULTS, saved)`: сначала ключи умолчаний, затем прочие.
pub fn sanitize_prefs(raw: Value) -> Value {
    let mut saved = match raw {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    for key in REMOVED_KEYS {
        saved.shift_remove(key);
    }
    let speed = normalize_speed_limit(saved.get("speedLimit"));
    saved.insert("speedLimit".to_owned(), speed);

    let Value::Object(mut prefs) = prefs_defaults() else {
        unreachable!("умолчания — объект");
    };
    prefs.extend(saved);
    Value::Object(prefs)
}

/// Значение для записи: `speedLimit` не из списка — 0.
fn normalized(key: &str, value: Value) -> Value {
    if key == "speedLimit" {
        normalize_speed_limit(Some(&value))
    } else {
        value
    }
}

/// `normalizeSpeedLimit`: только число из списка, иначе 0.
fn normalize_speed_limit(value: Option<&Value>) -> Value {
    match value.and_then(Value::as_f64) {
        Some(limit) if SPEED_LIMITS.contains(&limit) => json!(limit as u32),
        _ => json!(0),
    }
}

/// `launcher-prefs.json` поверх [`Store`]. В памяти — всегда очищенный объект,
/// поэтому запись сохраняет его целиком, как `write()` в 0.4.9.
#[derive(Debug)]
pub struct LauncherPrefs {
    store: Store,
}

impl LauncherPrefs {
    pub fn open(paths: &Paths) -> LauncherPrefs {
        let mut store = Store::open(paths.file(PREFS_FILE), prefs_defaults());
        let prefs = sanitize_prefs(store.data().clone());
        store.replace_data(prefs);
        LauncherPrefs { store }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.store.get(key)
    }

    /// Записать значение. Ключи прежних версий не сохраняются (`false`);
    /// `speedLimit` не из списка становится 0.
    pub fn set(&mut self, key: &str, value: Value) -> bool {
        if REMOVED_KEYS.contains(&key) {
            return false;
        }
        self.store.set(key, normalized(key, value))
    }

    /// Несколько ключей одной записью файла (Store пишет его целиком и с `sync_all`), по правилам
    /// [`set`](Self::set). `false` — среди них ключ прежних версий или файл не записан.
    pub fn set_many(&mut self, changes: &Map<String, Value>) -> bool {
        let mut kept: Vec<(&str, Value)> = changes
            .iter()
            .filter(|(key, _)| !REMOVED_KEYS.contains(&key.as_str()))
            .map(|(key, value)| (key.as_str(), normalized(key, value.clone())))
            .collect();
        let all_kept = kept.len() == changes.len();
        let Some((last_key, last_value)) = kept.pop() else {
            return all_kept;
        };
        let mut data = self.store.data().clone();
        for (key, value) in kept {
            data[key] = value;
        }
        self.store.replace_data(data);
        // Store пишет файл только из set(): остальные ключи уже в данных, set запишет их целиком.
        self.store.set(last_key, last_value) && all_kept
    }

    /// `save(prefs)` 0.4.9: снимок целиком, очищенный, как `write()`; ключи [`KEPT_ON_SAVE`]
    /// остаются из файла. `false` — файл не записан.
    pub fn save(&mut self, snapshot: Value) -> bool {
        let Value::Object(mut prefs) = sanitize_prefs(snapshot) else {
            unreachable!("очищенные prefs — объект");
        };
        for key in KEPT_ON_SAVE {
            match self.get(key) {
                Some(kept) => prefs.insert(key.to_owned(), kept.clone()),
                None => prefs.shift_remove(key),
            };
        }
        let rule_category = prefs.get("ruleCategory").cloned().unwrap_or(json!({}));
        self.store.replace_data(Value::Object(prefs));
        // Store пишет файл только из set(): данные уже заменены, set запишет их целиком.
        self.store.set("ruleCategory", rule_category)
    }

    /// `ruleCategoryOf(gameId)`: `"scripted"` или `"scriptless"` (по умолчанию).
    pub fn rule_category_of(&self, game_id: &str) -> &'static str {
        match self
            .get("ruleCategory")
            .and_then(|categories| categories.get(game_id))
        {
            Some(Value::String(category)) if category == "scripted" => "scripted",
            _ => "scriptless",
        }
    }

    /// `setRuleCategory(gameId, category)`: всё, кроме `"scripted"`, — `"scriptless"`.
    pub fn set_rule_category(&mut self, game_id: &str, category: &str) -> bool {
        let mut categories = match self.get("ruleCategory") {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        };
        let category = if category == "scripted" {
            "scripted"
        } else {
            "scriptless"
        };
        categories.insert(game_id.to_owned(), json!(category));
        self.store.set("ruleCategory", Value::Object(categories))
    }

    /// `setKeyboardFull(full)`.
    pub fn set_keyboard_full(&mut self, full: bool) -> bool {
        self.store.set("keyboardFull", json!(full))
    }

    /// Id темы: строка `launcherTheme`, иначе `"hlsr"`. Незнакомый id
    /// заменяет на первую тему уже выбор темы, как `themeById` в 0.4.7.
    pub fn theme_id(&self) -> String {
        self.get("launcherTheme")
            .and_then(Value::as_str)
            .unwrap_or("hlsr")
            .to_owned()
    }
}
