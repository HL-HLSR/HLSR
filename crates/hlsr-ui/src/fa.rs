//! Значки FontAwesome 7.3.1: таблица имён → кодовая точка и шрифт
//! (`data/fa.json` из `tools/reference/export-fa.js`).
//!
//! 0.4.7 пишет значок классами `<i class="fas fa-play">`, и часть имён
//! склеивается в коде (`GameMenu.vue:47`, `GameConfigEditor.vue:18`, значки из
//! main — `tools.js`, `achievements.js`, `index.js`), поэтому разрешается вся
//! строка классов, а таблица — полная (со старыми псевдонимами `fa-times`,
//! `fa-sync-alt`, `fa-dot-circle`…). Правила — `all.css` пакета:
//! `fas`/`fa-solid`/`fa` — solid 900, `far`/`fa-regular` — regular 400, `fab`/
//! `fa-brands` — brands 400; без префикса — solid (умолчание `.fa-*` в шрифте
//! 900); остальные `fa-*` (`fa-fw`, `fa-lg`, `fa-rotate-90`…) на глиф не
//! влияют. `fa-spin` лаунчер подменяет кольцом (`launcher.css:619`), глиф при
//! этом не рисуется — `Glyph::spin`.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Начертание шрифта значков.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Style {
    Solid,
    Regular,
    Brands,
}

impl Style {
    /// Семейство шрифта (`@font-face` пакета).
    pub fn family(self) -> &'static str {
        match self {
            Style::Solid | Style::Regular => "Font Awesome 7 Free",
            Style::Brands => "Font Awesome 7 Brands",
        }
    }

    /// `font-weight` начертания: solid 900, остальные 400.
    pub fn weight(self) -> i32 {
        match self {
            Style::Solid => 900,
            Style::Regular | Style::Brands => 400,
        }
    }

    fn from_name(name: &str) -> Option<Style> {
        match name {
            "solid" => Some(Style::Solid),
            "regular" => Some(Style::Regular),
            "brands" => Some(Style::Brands),
            _ => None,
        }
    }
}

/// Разрешённый значок.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph {
    pub ch: char,
    pub style: Style,
    /// Среди классов есть `fa-spin`: вместо глифа рисуется кольцо.
    pub spin: bool,
}

#[derive(Deserialize)]
struct RawIcon {
    code: String,
    styles: Vec<String>,
}

#[derive(Deserialize)]
struct RawTable {
    icons: HashMap<String, RawIcon>,
    aliases: HashMap<String, String>,
    prefixes: HashMap<String, String>,
}

/// Таблица имён значков.
pub struct Table {
    icons: HashMap<String, (char, Vec<Style>)>,
    aliases: HashMap<String, String>,
    prefixes: HashMap<String, Style>,
}

const DATA: &str = include_str!("../data/fa.json");

/// Таблица из `data/fa.json` (разбирается один раз).
pub fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| Table::parse(DATA).unwrap_or_else(|error| panic!("data/fa.json: {error}")))
}

impl Table {
    pub fn parse(json: &str) -> Result<Table, String> {
        let raw: RawTable = serde_json::from_str(json).map_err(|error| error.to_string())?;
        let mut icons = HashMap::with_capacity(raw.icons.len());
        for (name, icon) in raw.icons {
            let code = u32::from_str_radix(&icon.code, 16)
                .ok()
                .and_then(char::from_u32)
                .ok_or_else(|| format!("{name}: кодовая точка «{}»", icon.code))?;
            let styles = icon
                .styles
                .iter()
                .map(|style| {
                    Style::from_name(style).ok_or_else(|| format!("{name}: начертание «{style}»"))
                })
                .collect::<Result<_, _>>()?;
            icons.insert(name, (code, styles));
        }
        let prefixes = raw
            .prefixes
            .into_iter()
            .map(|(prefix, style)| {
                Style::from_name(&style)
                    .map(|style| (prefix.clone(), style))
                    .ok_or_else(|| format!("{prefix}: начертание «{style}»"))
            })
            .collect::<Result<_, _>>()?;
        Ok(Table {
            icons,
            aliases: raw.aliases,
            prefixes,
        })
    }

    /// Кодовая точка значка по имени без `fa-` (с псевдонимами v5).
    pub fn code_point(&self, name: &str) -> Option<char> {
        self.entry(name).map(|(code, _)| *code)
    }

    /// Есть ли глиф значка `name` (без `fa-`, с псевдонимами) в начертании
    /// `style` — по таблицам символов самих шрифтов.
    pub fn has_style(&self, name: &str, style: Style) -> bool {
        self.entry(name)
            .is_some_and(|(_, styles)| styles.contains(&style))
    }

    fn entry(&self, name: &str) -> Option<&(char, Vec<Style>)> {
        let name = self.aliases.get(name).map_or(name, String::as_str);
        self.icons.get(name)
    }

    /// Строка классов `<i class="…">` → значок. `None` — имени значка нет.
    pub fn resolve(&self, classes: &str) -> Option<Glyph> {
        let mut style = None;
        let mut ch = None;
        let mut spin = false;
        for token in classes.split_whitespace() {
            if let Some(found) = self.prefixes.get(token) {
                style.get_or_insert(*found);
            } else if token == "fa-spin" {
                spin = true;
            } else if let Some(name) = token.strip_prefix("fa-")
                && ch.is_none()
                && let Some(code) = self.code_point(name)
            {
                ch = Some(code);
            }
        }
        match (ch, spin) {
            (Some(ch), _) => Some(Glyph {
                ch,
                style: style.unwrap_or(Style::Solid),
                spin,
            }),
            // Кольцо не зависит от имени значка.
            (None, true) => Some(Glyph {
                ch: ' ',
                style: style.unwrap_or(Style::Solid),
                spin,
            }),
            (None, false) => None,
        }
    }
}

/// Значок для разметки: `(found, text, family, weight, spin)`. Нет значка —
/// `found = false` и пустой текст.
pub fn glyph_parts(classes: &str) -> (bool, String, &'static str, i32, bool) {
    match table().resolve(classes) {
        Some(glyph) => (
            true,
            glyph.ch.to_string(),
            glyph.style.family(),
            glyph.style.weight(),
            glyph.spin,
        ),
        None => (
            false,
            String::new(),
            Style::Solid.family(),
            Style::Solid.weight(),
            false,
        ),
    }
}
