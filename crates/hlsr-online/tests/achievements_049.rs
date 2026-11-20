//! Достижения и серии против 0.4.9: фикстура `achievements-049.json` из
//! `tools/reference/online/achievements-049.js` — таблица и списки, `evaluate` с временами, `mask`,
//! `order`, `summarize`, `readTimes` (и плоский файл первой сборки), `stamp`, серия дней с рекордом.
//! Серия здесь — в поясе UTC (смещение 0); остальные пояса фикстуры проверяет
//! `profile_model.rs` дочерними процессами с `TZ` (только Unix).

mod support;

use hlsr_online::profile::achievements;
use hlsr_online::profile::launch_streak::{self, DayZone};
use serde_json::{Value, json};
use support::{Mismatches, fixture, items, str_of};

fn strings(value: Option<&Value>) -> Vec<&str> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

#[test]
fn constants_match_0_4_9() {
    let fixture = fixture("achievements-049.json");
    let stored: Vec<&str> = achievements::UNLOCK_IDS
        .iter()
        .chain(&achievements::SERVER_IDS)
        .copied()
        .collect();
    let goals: Vec<Value> = launch_streak::GOALS
        .iter()
        .map(|(id, days)| json!([id, days]))
        .collect();
    let actual = json!({
        "ACHIEVEMENTS": achievements::ACHIEVEMENTS.iter().map(|a| a.to_json()).collect::<Vec<_>>(),
        "SECRET_IDS": achievements::SECRET_IDS,
        "BONUS_IDS": achievements::BONUS_IDS,
        "UNLOCK_IDS": achievements::UNLOCK_IDS,
        "SERVER_IDS": achievements::SERVER_IDS,
        "STORED_IDS": stored,
        "RENDERER_UNLOCK_IDS": achievements::RENDERER_UNLOCK_IDS,
        "HOUR_SECONDS": 3600,
        "STREAK_FILE": launch_streak::FILE_NAME,
        "STREAK_GOALS": goals,
    });
    let mut mismatches = Mismatches::default();
    mismatches.check(&actual, &fixture["constants"], "константы");
    mismatches.finish("константы");
    assert!(stored.iter().all(|id| achievements::is_stored(id)));
    assert!(!achievements::is_stored("made-up"));
}

#[test]
fn evaluate_mask_order_match_0_4_9() {
    let fixture = fixture("achievements-049.json");
    let mut mismatches = Mismatches::default();
    for (index, case) in items(&fixture["evaluate"]).iter().enumerate() {
        let options = &case["options"];
        let games = options.get("games").unwrap_or(&Value::Null);
        let unlocked = strings(options.get("unlocked"));
        let list = achievements::evaluate(
            games,
            options.get("firstRunAt").and_then(Value::as_str),
            &unlocked,
            options.get("times"),
        );
        let best = achievements::best_game(games).map(|(game_id, seconds)| {
            json!({"gameId": game_id, "seconds": hlsr_core::json_text::js_number(seconds)})
        });
        let actual = json!({
            "options": options,
            "evaluate": list,
            "summarize": achievements::summarize(&list),
            "own": achievements::order(achievements::mask(list.clone(), &unlocked)),
            "bestGame": best,
            "totalLaunches": hlsr_core::json_text::js_number(achievements::total_launches(games)),
        });
        mismatches.check(&actual, case, format!("достижения #{index}"));
    }
    for (index, case) in items(&fixture["view"]).iter().enumerate() {
        let options = &case["options"];
        let list =
            achievements::evaluate(&Value::Null, None, &strings(options.get("unlocked")), None);
        for viewer in items(&case["viewers"]) {
            let known = strings(viewer.get("known"));
            let actual = json!(achievements::order(achievements::mask(
                list.clone(),
                &known
            )));
            mismatches.check(
                &actual,
                &viewer["list"],
                format!("чужой #{index}, {known:?}"),
            );
        }
    }
    mismatches.finish("достижения");
}

#[test]
fn times_match_0_4_9() {
    let fixture = fixture("achievements-049.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["readTimes"]) {
        let actual = json!(achievements::read_times(
            Some(&case["raw"]),
            case["account"].as_str()
        ));
        mismatches.check(&actual, &case["out"], format!("readTimes({})", case["raw"]));
    }
    for (index, case) in items(&fixture["stamp"]).iter().enumerate() {
        let options = &case["options"];
        let actual = json!(achievements::stamp(
            case.get("times").and_then(Value::as_object),
            options.get("games").unwrap_or(&Value::Null),
            &strings(options.get("unlocked")),
            case["fresh"].as_str(),
            case["now"].as_i64().unwrap(),
        ));
        mismatches.check(&actual, &case["out"], format!("stamp #{index}"));
    }
    mismatches.finish("время получения");
}

/// Сценарий серии из фикстуры в заданном поясе: `record`, `stats`, запись файла, снимок папки.
fn streak_scenario(section: &Value, zone: DayZone, mismatches: &mut Mismatches) {
    let tz = str_of(&section["tz"]);
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let dir = root.join("data");
    for (step, expected) in items(&section["steps"])
        .iter()
        .zip(items(&section["results"]))
    {
        let now = step["now"].as_i64().unwrap_or(0);
        let actual = match str_of(&step["op"]) {
            "record" => json!(launch_streak::record_in_zone(&dir, now, zone)),
            "stats" => launch_streak::stats_in_zone(&dir, now, zone),
            "files" => support::files(root),
            _ => {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join(launch_streak::FILE_NAME), str_of(&step["utf8"])).unwrap();
                Value::Null
            }
        };
        mismatches.check(&actual, expected, format!("{tz}: {step}"));
    }
}

#[test]
fn launch_streak_matches_0_4_9_in_utc() {
    let fixture = fixture("achievements-049.json");
    let section = items(&fixture["streak"])
        .iter()
        .find(|section| section["tz"] == "UTC")
        .unwrap();
    let mut mismatches = Mismatches::default();
    streak_scenario(section, DayZone::Fixed(0), &mut mismatches);
    mismatches.finish("серия дней");
}
