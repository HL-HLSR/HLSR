//! Вкладки редактора конфига (`config-tabs.js` 0.4.9, `GameConfigEditor`): «Файлы cfg»,
//! «Проверка» (HL1) и «Игра». «Игра» — все понятные настройки одной прокруткой, разделы в порядке
//! каталога (`game_config`: Управление, Изображение, Звук, Игра; пустых у игры нет).

use hlsr_delivery::game_config::{Catalog, CatalogSection, Field, Label};

pub const FILES_ID: &str = "cfg-files";
pub const CHECK_ID: &str = "cfg-check";
pub const GAME_ID: &str = "cfg-game";

/// Вкладка: `fields` — настройки вкладки (по ним точка несохранённых правок), `sections` — её
/// разделы.
#[derive(Debug, Clone)]
pub struct ConfigTab<'a> {
    pub id: &'static str,
    pub icon: &'static str,
    pub title: Label,
    pub fields: Vec<&'static Field>,
    pub sections: Vec<&'a CatalogSection>,
}

fn tab<'a>(
    id: &'static str,
    icon: &'static str,
    ru: &'static str,
    en: &'static str,
) -> ConfigTab<'a> {
    ConfigTab {
        id,
        icon,
        title: Label { ru, en },
        fields: Vec::new(),
        sections: Vec::new(),
    }
}

/// `configTabs(catalog, { files, check })`: `files`/`check` — окно передало редактор файлов и
/// итог проверки.
pub fn config_tabs(catalog: Option<&Catalog>, files: bool, check: bool) -> Vec<ConfigTab<'_>> {
    let mut tabs = Vec::new();
    if files {
        tabs.push(tab(FILES_ID, "fa-file-alt", "Файлы cfg", "cfg files"));
    }
    if check {
        tabs.push(tab(
            CHECK_ID,
            "fa-clipboard-check",
            "Проверка",
            "Rule check",
        ));
    }
    let sections: Vec<&CatalogSection> = catalog
        .map(|c| c.sections.iter().collect())
        .unwrap_or_default();
    if !sections.is_empty() {
        tabs.push(ConfigTab {
            fields: sections
                .iter()
                .flat_map(|s| s.fields.iter().copied())
                .collect(),
            sections,
            ..tab(GAME_ID, "fa-sliders-h", "Игра", "Game")
        });
    }
    tabs
}

#[cfg(test)]
mod tests {
    use super::*;
    use hlsr_delivery::game_config::catalog_for;

    fn ids(tabs: &[ConfigTab]) -> Vec<&'static str> {
        tabs.iter().map(|tab| tab.id).collect()
    }

    // config-tabs.js: порядок «Файлы cfg», «Проверка», «Игра»; «Игра» — все разделы каталога.
    #[test]
    fn tabs_follow_the_slots_and_the_catalog() {
        let catalog = catalog_for("hl1-won", "goldsrc").expect("каталог HL1");
        let tabs = config_tabs(Some(&catalog), true, true);
        assert_eq!(ids(&tabs), [FILES_ID, CHECK_ID, GAME_ID]);
        assert_eq!(tabs[0].title.en, "cfg files");
        assert_eq!(tabs[1].icon, "fa-clipboard-check");
        let game = &tabs[2];
        assert_eq!(game.sections.len(), catalog.sections.len());
        let flat: usize = catalog.sections.iter().map(|s| s.fields.len()).sum();
        assert_eq!(game.fields.len(), flat);
        assert!(tabs[0].fields.is_empty() && tabs[0].sections.is_empty());

        assert_eq!(
            ids(&config_tabs(Some(&catalog), true, false)),
            [FILES_ID, GAME_ID]
        );
        assert_eq!(ids(&config_tabs(Some(&catalog), false, false)), [GAME_ID]);
        // Без каталога «Игры» нет.
        assert_eq!(ids(&config_tabs(None, true, true)), [FILES_ID, CHECK_ID]);
        assert!(config_tabs(None, false, false).is_empty());
    }
}
