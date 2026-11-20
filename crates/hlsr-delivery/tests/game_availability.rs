//! Реестр доступности игр против 0.4.7: фикстура `game-availability.json` из
//! `tools/reference/delivery/game-availability.js` (`shared/game-availability.js`). Идентификаторы бывают
//! строками и числами: модуль берёт `String(id)`.

mod support;

use hlsr_core::js;
use hlsr_delivery::game_availability as ga;
use serde_json::{Value, json};
use std::sync::LazyLock;
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("game-availability.json"));

/// `String(id)`: `undefined` в фикстуре — поле `undefined: true`.
fn string_of(case: &Value, key: &str) -> String {
    if case["undefined"].as_bool() == Some(true) {
        js::to_template_string(None)
    } else {
        js::to_template_string(Some(&case[key]))
    }
}

#[test]
fn constants_match_0_4_7() {
    let c = &FIXTURE["constants"];
    assert_eq!(c["AVAILABLE_GAME_IDS"], json!(ga::AVAILABLE_GAME_IDS));
    assert_eq!(c["DEFAULT_GAME_ID"], json!(ga::DEFAULT_GAME_ID));
    assert_eq!(c["GAME_ORDER"], json!(ga::GAME_ORDER));
    assert_eq!(c["HIDDEN_GAME_IDS"], json!(ga::HIDDEN_GAME_IDS));
}

#[test]
fn availability_and_hidden_match_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["available"]) {
        m.check_eq(
            ga::is_game_available(&string_of(case, "id")),
            case["result"].as_bool().unwrap(),
            format!("isGameAvailable({})", case["id"]),
        );
    }
    for case in items(&FIXTURE["hidden"]) {
        m.check_eq(
            ga::is_game_hidden(&string_of(case, "id")),
            case["result"].as_bool().unwrap(),
            format!("isGameHidden({})", case["id"]),
        );
    }
    m.finish("isGameAvailable / isGameHidden");
}

#[test]
fn startup_game_matches_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["startup"]) {
        // Нет сохранённого значения (`undefined`) и `null` — разные входы JS, одинаковый итог.
        let saved = if case["undefined"].as_bool() == Some(true) && case.get("saved").is_none() {
            None
        } else {
            Some(&case["saved"])
        };
        m.check_eq(
            ga::resolve_startup_game(saved),
            str_of(&case["result"]).to_owned(),
            format!("resolveStartupGame({})", case["saved"]),
        );
    }
    m.finish("resolveStartupGame");
}

#[test]
fn order_games_matches_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["order"]) {
        let games: Vec<Value> = items(&case["games"]).clone();
        let ordered = ga::order_games(games, |game| js::to_template_string(Some(&game["id"])));
        m.check(
            &Value::Array(ordered),
            &case["result"],
            format!("orderGames({})", case["games"]),
        );
    }
    m.finish("orderGames");
}
