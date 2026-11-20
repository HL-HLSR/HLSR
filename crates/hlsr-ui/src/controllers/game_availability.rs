//! Игры в колонке (`shared/game-availability.js` + `games.config.js` 0.4.7): какие игры видны,
//! в каком порядке, какие доступны, с какой игры стартует лаунчер. Правила — модуль ПП2
//! (`hlsr_delivery::game_availability`), здесь — список игр интерфейса и его выборки.

use hlsr_delivery::game_availability as rules;
use serde_json::Value;

/// Игра из `games.config.js` (то, что берёт интерфейс).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameEntry {
    pub id: &'static str,
    pub name: &'static str,
}

/// `games.config.js` по порядку файла.
pub const GAME_LIST: [GameEntry; 7] = [
    GameEntry {
        id: "70",
        name: "Half-Life",
    },
    GameEntry {
        id: "50",
        name: "Half-Life: Opposing Force",
    },
    GameEntry {
        id: "130",
        name: "Half-Life: Blue Shift",
    },
    GameEntry {
        id: "220",
        name: "Half-Life 2",
    },
    GameEntry {
        id: "218",
        name: "Half-Life 2: Ghosting",
    },
    GameEntry {
        id: "380",
        name: "Half-Life 2: Episode One",
    },
    GameEntry {
        id: "420",
        name: "Half-Life 2: Episode Two",
    },
];

/// Игра колонки: запись и доступность (`available`; недоступная показывается «скоро»).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnGame {
    pub game: GameEntry,
    pub available: bool,
}

/// `games` оболочки: `orderGames(GameList.map(...))` — скрытые убраны, сначала порядок
/// колонки, затем прочие в порядке файла.
pub fn column_games() -> Vec<ColumnGame> {
    rules::order_games(
        GAME_LIST
            .iter()
            .map(|game| ColumnGame {
                game: *game,
                available: rules::is_game_available(game.id),
            })
            .collect(),
        |entry| entry.game.id.to_owned(),
    )
}

/// Доступные игры по порядку колонки (`askLiveSplitLaunch`).
pub fn available_games() -> Vec<GameEntry> {
    rules::order_games(
        GAME_LIST
            .iter()
            .copied()
            .filter(|game| rules::is_game_available(game.id))
            .collect(),
        |game| game.id.to_owned(),
    )
}

/// Можно ли выбрать игру (`isGameAvailable`).
pub fn is_available(id: &str) -> bool {
    rules::is_game_available(id)
}

/// Игра при запуске: `resolveStartupGame(library.get("lastLaunched"))`.
pub fn startup_game(last_launched: Option<&Value>) -> String {
    rules::resolve_startup_game(last_launched)
}
