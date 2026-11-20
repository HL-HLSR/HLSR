//! Чипы игр страницы «Персонализация» (`config-chips.js` 0.4.9) и её список игр
//! (`configGames` `LauncherShell.vue`): установленные игры в порядке библиотеки.

use super::game_availability;
use super::game_demo;
use super::shell_flows::short_name;
use hlsr_core::js::{self, object::JsObject};
use serde_json::Value;

/// `HL2_ID`: Half-Life 2 («220») — один чип с пометкой «в разработке».
pub const HL2_ID: &str = "220";

/// Игра страницы: `{ id, name, title }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigGame {
    pub id: String,
    /// `shortName` (чип).
    pub name: String,
    /// «Half-Life 2: Ghosting Mod: Renewed» (`gameTitle` окна).
    pub title: String,
}

/// Чип игры: `{ id, name, soon, disabled }`; у неактивного id пустой.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip {
    pub id: String,
    pub name: String,
    pub soon: bool,
    pub disabled: bool,
}

/// `configGames`: доступные игры колонки, у которых доставка знает установленную версию.
pub fn config_games(delivery: &JsObject<Value>) -> Vec<ConfigGame> {
    game_availability::column_games()
        .into_iter()
        .filter(|column| {
            column.available
                && delivery
                    .get(column.game.id)
                    .and_then(|status| status.get("installedVersion"))
                    .is_some_and(js::truthy)
        })
        .map(|column| {
            let id = column.game.id;
            let title = game_demo::header(id, None)
                .map(|header| match header.hero_subtitle {
                    Some(sub) if !sub.is_empty() => format!("{}: {sub}", header.hero_title),
                    _ => header.hero_title,
                })
                .unwrap_or_default();
            ConfigGame {
                id: id.to_owned(),
                name: short_name(id).unwrap_or(column.game.name).to_owned(),
                title,
            }
        })
        .collect()
}

/// `configChips(games)`: HL2 установлена — обычный чип с пометкой, нет — неактивный; последним
/// неактивный LiveSplit.
pub fn config_chips(games: &[ConfigGame]) -> Vec<Chip> {
    let chip = |id: &str, name: &str, soon: bool, disabled: bool| Chip {
        id: id.to_owned(),
        name: name.to_owned(),
        soon,
        disabled,
    };
    let mut chips: Vec<Chip> = games
        .iter()
        .map(|game| {
            let hl2 = game.id == HL2_ID;
            chip(
                &game.id,
                if hl2 { "Half-Life 2" } else { &game.name },
                hl2,
                false,
            )
        })
        .collect();
    if !games.iter().any(|game| game.id == HL2_ID) {
        chips.push(chip("", "Half-Life 2", true, true));
    }
    chips.push(chip("", "LiveSplit", true, true));
    chips
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn game(id: &str, name: &str) -> ConfigGame {
        ConfigGame {
            id: id.into(),
            name: name.into(),
            title: String::new(),
        }
    }

    fn row(chip: &Chip) -> (&str, &str, bool, bool) {
        (&chip.id, &chip.name, chip.soon, chip.disabled)
    }

    #[test]
    fn hl2_is_one_chip_and_livesplit_is_always_last_and_inactive() {
        let chips = config_chips(&[game("218", "HL2 Ghosting"), game("70", "Half-Life")]);
        let rows: Vec<_> = chips.iter().map(row).collect();
        assert_eq!(
            rows,
            [
                ("218", "HL2 Ghosting", false, false),
                ("70", "Half-Life", false, false),
                ("", "Half-Life 2", true, true),
                ("", "LiveSplit", true, true),
            ]
        );
        // HL2 установлена: выбирается как все, но с пометкой «в разработке».
        let chips = config_chips(&[game("220", "HL2"), game("70", "Half-Life")]);
        let rows: Vec<_> = chips.iter().map(row).collect();
        assert_eq!(
            rows,
            [
                ("220", "Half-Life 2", true, false),
                ("70", "Half-Life", false, false),
                ("", "LiveSplit", true, true),
            ]
        );
        assert_eq!(config_chips(&[]).len(), 2);
    }

    #[test]
    fn only_installed_games_reach_the_page_in_column_order() {
        let mut delivery = JsObject::new();
        delivery.insert("70", json!({"installedVersion": "1.0.0"}));
        delivery.insert("220", json!({"installedVersion": null}));
        delivery.insert("218", json!({"installedVersion": "1.0.9"}));
        let games = config_games(&delivery);
        let ids: Vec<&str> = games.iter().map(|game| game.id.as_str()).collect();
        let order: Vec<&str> = game_availability::column_games()
            .iter()
            .map(|column| column.game.id)
            .filter(|id| ["70", "218"].contains(id))
            .collect();
        assert_eq!(ids, order);
        let ghosting = games.iter().find(|game| game.id == "218").unwrap();
        assert_eq!(ghosting.name, "HL2 Ghosting");
        assert!(
            ghosting.title.starts_with("Half-Life 2"),
            "{}",
            ghosting.title
        );
        assert!(config_games(&JsObject::new()).is_empty());
    }
}
