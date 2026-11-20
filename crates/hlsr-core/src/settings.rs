//! `settings.json`: язык, масштаб интерфейса, режим отрисовки.

use crate::defaults::defaults;
use crate::json_text::js_number;
use crate::language::Language;
use crate::paths::Paths;
use crate::store::Store;
use crate::window_size::normalize_ui_scale;
use serde_json::{Map, Value};

pub const SETTINGS_FILE: &str = "settings.json";

/// Режим отрисовки из настроек.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererChoice {
    /// По ОС: Linux — OpenGL; Windows — OpenGL при аппаратном главном адаптере (NVIDIA, AMD,
    /// Intel) вне удалённой сессии, иначе DX12 (`hlsr-app` `renderer.rs`).
    Auto,
    OpenGl,
    Vulkan,
    Dx12,
    Software,
}

impl RendererChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            RendererChoice::Auto => "auto",
            RendererChoice::OpenGl => "opengl",
            RendererChoice::Vulkan => "vulkan",
            RendererChoice::Dx12 => "dx12",
            RendererChoice::Software => "software",
        }
    }

    fn from_setting(value: &str) -> Option<RendererChoice> {
        match value {
            "auto" => Some(RendererChoice::Auto),
            "opengl" => Some(RendererChoice::OpenGl),
            "vulkan" => Some(RendererChoice::Vulkan),
            // DX12 есть только на Windows; на другой ОС — выбор по умолчанию.
            "dx12" if cfg!(windows) => Some(RendererChoice::Dx12),
            "dx12" => Some(RendererChoice::Auto),
            "software" => Some(RendererChoice::Software),
            _ => None,
        }
    }
}

/// `settings.json` поверх [`Store`]. Поля пишутся в `config` как
/// `Object.assign({}, store.get("config"), {…})`: прочие поля и их порядок
/// сохраняются.
#[derive(Debug)]
pub struct Settings {
    store: Store,
}

impl Settings {
    pub fn open(paths: &Paths) -> Settings {
        Settings {
            store: Store::open(paths.file(SETTINGS_FILE), defaults("settings")),
        }
    }

    /// Масштаб интерфейса, как `readUiScale` в `CreateWindow.js`.
    pub fn ui_scale(&self) -> f64 {
        normalize_ui_scale(self.config_field("uiScale").unwrap_or(&Value::Null))
    }

    /// Записать масштаб; не из списка → 1, как `ui-scale.js`.
    pub fn set_ui_scale(&mut self, scale: f64) -> bool {
        let scale = normalize_ui_scale(&js_number(scale));
        self.set_config_field("uiScale", js_number(scale))
    }

    /// Режим отрисовки: `config.renderer`, а без него — снятый флажок
    /// «Аппаратное ускорение» 0.4.7 (`gpuAcceleration === false`) даёт программный.
    /// `gpuAcceleration` только читается.
    pub fn renderer(&self) -> RendererChoice {
        if let Some(choice) = self
            .config_field("renderer")
            .and_then(Value::as_str)
            .and_then(RendererChoice::from_setting)
        {
            return choice;
        }
        if self.config_field("gpuAcceleration") == Some(&Value::Bool(false)) {
            RendererChoice::Software
        } else {
            RendererChoice::Auto
        }
    }

    /// Язык из `config.language`: только целое 0 или 1 (`Number.isInteger`, `1.0` подходит).
    pub fn language(&self) -> Option<Language> {
        let index = self.config_field("language")?.as_f64()?;
        if index.fract() != 0.0 {
            return None;
        }
        Language::from_index(index as i64)
    }

    pub fn set_language(&mut self, lang: Language) -> bool {
        self.set_config_field("language", Value::from(lang.index()))
    }

    fn config_field(&self, key: &str) -> Option<&Value> {
        self.store.get("config")?.get(key)
    }

    fn set_config_field(&mut self, key: &str, value: Value) -> bool {
        // Object.assign({}, config): не объект (нет ключа, null…) → пустой объект.
        let mut config = match self.store.get("config") {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        };
        config.insert(key.to_owned(), value);
        self.store.set("config", Value::Object(config))
    }
}
