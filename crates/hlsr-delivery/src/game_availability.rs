//! Реестр доступности игр (`src/shared/game-availability.js`): какие игры есть в колонке,
//! в каком порядке, какие скрыты и с какой игры стартует лаунчер.
//!
//! Идентификаторы в JS бывают строками и числами, а модуль всюду берёт `String(id)`; здесь
//! функции принимают уже строку (`hlsr_core::js::to_template_string` даёт `String(value)`).

use hlsr_core::js;
use serde_json::Value;

pub const AVAILABLE_GAME_IDS: [&str; 3] = ["218", "220", "70"];

pub const DEFAULT_GAME_ID: &str = "218";

pub const GAME_ORDER: [&str; 3] = ["218", "220", "70"];

/// Эпизоды HL2 не отдельные игры в колонке: они готовятся как сборки Half-Life 2 и показаны
/// в её списке сборок с пометкой «скоро».
pub const HIDDEN_GAME_IDS: [&str; 4] = ["50", "130", "380", "420"];

/// `isGameAvailable(id)`; `id` — результат `String(id)`.
pub fn is_game_available(id: &str) -> bool {
    AVAILABLE_GAME_IDS.contains(&id)
}

/// `isGameHidden(id)`; `id` — результат `String(id)`.
pub fn is_game_hidden(id: &str) -> bool {
    HIDDEN_GAME_IDS.contains(&id)
}

/// `orderGames(games)`: скрытые убираются, игры из [`GAME_ORDER`] идут первыми в его порядке,
/// остальные — следом в порядке появления. Повтор идентификатора заменяет запись, а место
/// остаётся первым (`new Map(...)`). `id_of` даёт `String(game.id)`.
pub fn order_games<T>(games: Vec<T>, id_of: impl Fn(&T) -> String) -> Vec<T> {
    let mut by_id: Vec<(String, Option<T>)> = Vec::new();
    for game in games {
        let id = id_of(&game);
        if is_game_hidden(&id) {
            continue;
        }
        match by_id.iter_mut().find(|(key, _)| *key == id) {
            Some(slot) => slot.1 = Some(game),
            None => by_id.push((id, Some(game))),
        }
    }
    let mut ordered = Vec::with_capacity(by_id.len());
    for id in GAME_ORDER {
        if let Some(slot) = by_id.iter_mut().find(|(key, _)| key == id)
            && let Some(game) = slot.1.take()
        {
            ordered.push(game);
        }
    }
    ordered.extend(by_id.into_iter().filter_map(|(_, game)| game));
    ordered
}

/// `resolveStartupGame(savedId)`: сохранённая игра, если она доступна, иначе игра по умолчанию.
/// `None` — значения нет (`undefined`), `Some(null)` — `null`.
pub fn resolve_startup_game(saved_id: Option<&Value>) -> String {
    match saved_id {
        Some(value) if !value.is_null() => {
            let id = js::to_template_string(Some(value));
            if is_game_available(&id) {
                return id;
            }
            DEFAULT_GAME_ID.to_owned()
        }
        _ => DEFAULT_GAME_ID.to_owned(),
    }
}
