//! Модель профиля против 0.4.7: фикстура `profile-model.json` из
//! `tools/reference/online/profile-model.js` (Node 24) — карточка, снимок, достижения, модерация,
//! CSS ника и рамки, значки, местоимения, ссылки профиля, ссылки в статусе, серия дней в разных
//! часовых поясах. Плюс самотесты 0.4.7 (`online-profile-selftest.js`, `launcher-ui-selftest.js`).
//! Достижения и серия 0.4.9 расходятся с 0.4.7: таблицу, `evaluate` и `record` сверяет
//! `achievements_049.rs`, снимок здесь приводится к виду 0.4.7 (`support::profile_as_047`).
//! Карточка 0.4.9 (подписка, буст, права, привязки, значки годовщин) — `profile-state-049.json`
//! из `tools/reference/online/profile-state-049.js`, без приведения.

mod support;

use hlsr_online::online_profile::moderation;
use hlsr_online::online_profile::profile_state::{self, LocalAccount, LocalPresence};
use hlsr_online::profile::{achievements, launch_streak};
use hlsr_online::shared::{
    anniversary_badges, avatar_frames, custom_badges, link_guard, nick_styles, profile_links,
    pronouns,
};
use serde_json::{Value, json};
use support::{Mismatches, fixture, items, str_of};

fn opt(value: Option<String>) -> Value {
    value.map_or(Value::Null, Value::String)
}

/// `{"undefined": true}` в фикстуре — `undefined` JS.
fn defined(value: &Value) -> Option<&Value> {
    (*value != json!({"undefined": true})).then_some(value)
}

#[test]
fn constants_match_node() {
    let fixture = fixture("profile-model.json");
    let constants = &fixture["constants"];
    let codes: serde_json::Map<String, Value> = moderation::CODES
        .iter()
        .map(|(action, code)| ((*action).to_owned(), json!(code)))
        .collect();
    let pronoun_list: Vec<Value> = pronouns::PRONOUNS
        .iter()
        .map(|(id, ru, en)| json!({"id": id, "ru": ru, "en": en}))
        .collect();
    let actual = json!({
        "GAMES": profile_state::GAMES,
        "HOUR_SECONDS": 3600,
        "ACTIONS": moderation::ACTIONS,
        "CODES": codes,
        "PRONOUNS": pronoun_list,
        "NICK_PRESETS": nick_styles::PRESETS,
        "NICK_PROPERTIES": nick_styles::PROPERTIES,
        "NICK_MAX_CSS": nick_styles::MAX_CSS,
        "FRAME_PROPERTIES": avatar_frames::PROPERTIES.to_vec(),
        "FRAME_MAX_CSS": avatar_frames::MAX_CSS,
        "BADGE_MAX_SVG": custom_badges::MAX_SVG,
        "STREAK_FILE": launch_streak::FILE_NAME,
    });
    // Таблицу достижений и цели серии 0.4.9 сверяет achievements_049.rs.
    let mut constants = constants.clone();
    for key in [
        "ACHIEVEMENTS",
        "SECRET_IDS",
        "UNLOCK_IDS",
        "RENDERER_UNLOCK_IDS",
        "STREAK_GOAL",
    ] {
        constants.as_object_mut().unwrap().shift_remove(key);
    }
    let mut mismatches = Mismatches::default();
    mismatches.check(&actual, &constants, "константы");
    mismatches.finish("константы");
    assert_eq!(achievements::HOUR_SECONDS, 3600.0);
}

#[test]
fn css_badges_and_pronouns_match_node() {
    let fixture = fixture("profile-model.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["css"]) {
        let value = &case["value"];
        let actual = json!({
            "value": value,
            "nick": opt(nick_styles::sanitize_css(value)),
            "frame": opt(avatar_frames::sanitize_css(value)),
        });
        mismatches.check(&actual, case, format!("sanitizeCss({value})"));
    }
    for case in items(&fixture["nick"]) {
        let actual = json!(nick_styles::normalize(&case[0]));
        mismatches.check(&actual, &case[1], format!("nick.normalize({})", case[0]));
    }
    for case in items(&fixture["frame"]) {
        let actual = json!(avatar_frames::normalize(&case[0]));
        mismatches.check(
            &actual,
            &case[1],
            format!("avatarFrame.normalize({})", case[0]),
        );
    }
    for (index, case) in items(&fixture["badges"]).iter().enumerate() {
        let actual = json!(custom_badges::normalize(&case[0]));
        mismatches.check(
            &actual,
            &case[1],
            format!("customBadges.normalize #{index}"),
        );
    }
    let pronoun_cases = &fixture["pronouns"];
    for case in items(&pronoun_cases["normalize"]) {
        let actual = json!(pronouns::normalize_pronouns(case[0].as_str()));
        mismatches.check(&actual, &case[1], format!("normalizePronouns({})", case[0]));
    }
    for case in items(&pronoun_cases["label"]) {
        let locale = case[1].as_str().unwrap_or("");
        let actual = json!(pronouns::pronoun_label(case[0].as_str(), locale));
        mismatches.check(
            &actual,
            &case[2],
            format!("pronounLabel({}, {})", case[0], case[1]),
        );
    }
    for case in items(&pronoun_cases["list"]) {
        let list: Vec<Value> = pronouns::pronoun_list(str_of(&case[0]))
            .into_iter()
            .map(|(id, label)| json!({"id": id, "label": label}))
            .collect();
        mismatches.check(&json!(list), &case[1], format!("pronounList({})", case[0]));
    }
    mismatches.finish("CSS, значки, местоимения");
}

#[test]
fn profile_links_match_node() {
    let fixture = fixture("profile-model.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["links"]) {
        let field = str_of(&case["field"]);
        let value = &case["value"];
        let actual = if case.get("normalizeYoutube").is_some() {
            json!({
                "field": field, "value": value,
                "normalizeYoutube": opt(profile_links::normalize_youtube(value)),
                "normalizeTwitch": opt(profile_links::normalize_twitch(value)),
            })
        } else {
            json!({
                "field": field, "value": value,
                "normalize": opt(profile_links::normalize(field, value)),
                "channelName": opt(profile_links::channel_name(field, value)),
            })
        };
        mismatches.check(&actual, case, format!("{field}: {value}"));
    }
    mismatches.finish("ссылки профиля");
}

#[test]
fn link_guard_matches_node() {
    let fixture = fixture("profile-model.json");
    let cases = items(&fixture["linkGuard"]);
    assert!(cases.len() > 100, "корпус: {}", cases.len());
    let mut mismatches = Mismatches::default();
    for case in cases {
        let actual = json!(link_guard::contains_link(&case[0]));
        mismatches.check(&actual, &case[1], format!("containsLink({})", case[0]));
    }
    mismatches.finish("ссылки в статусе");
}

fn local_account(value: &Value) -> Option<LocalAccount> {
    let value = defined(value).filter(|value| value.is_object())?;
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(LocalAccount {
        steam_id64: text("steamId64"),
        persona_name: text("personaName"),
        avatar_data_uri: text("avatarDataUri"),
    })
}

fn local_presence(value: Option<&Value>) -> Option<LocalPresence> {
    let value = value.filter(|value| value.is_object())?;
    Some(LocalPresence {
        online: value.get("online") == Some(&Value::Bool(true)),
        game_id: value
            .get("gameId")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

#[test]
fn profile_state_matches_node() {
    let fixture = fixture("profile-model.json");
    let profile = &fixture["profile"];
    let mut mismatches = Mismatches::default();
    for (index, case) in items(&profile["normalize"]).iter().enumerate() {
        let actual = support::without_049(&json!(profile_state::normalize(&case[0])));
        mismatches.check(
            &actual,
            &case[1],
            format!("normalize #{index}: {}", case[0]),
        );
    }
    for (index, case) in items(&profile["listEntry"]).iter().enumerate() {
        let actual = support::without_049(&json!(profile_state::list_entry(&case[0])));
        mismatches.check(&actual, &case[1], format!("listEntry #{index}"));
    }
    for (index, case) in items(&profile["state"]).iter().enumerate() {
        let state = support::profile_as_047(&state_of(case)).unwrap();
        mismatches.check(&state, &case["state"], format!("state #{index}"));
    }
    for case in items(&profile["rating"]) {
        let raw = defined(&case[0]).unwrap_or(&Value::Null);
        mismatches.check(
            &profile_state::rating(raw),
            &case[1],
            format!("rating({})", case[0]),
        );
    }
    for case in items(&profile["vote"]) {
        mismatches.check(
            &profile_state::vote(&case[0]),
            &case[1],
            format!("vote({})", case[0]),
        );
    }
    for case in items(&profile["date"]) {
        let actual = opt(profile_state::date(Some(&case[0])));
        mismatches.check(&actual, &case[1], format!("date({})", case[0]));
    }
    for case in items(&profile["runUrl"]) {
        let actual = json!(profile_state::run_url(&case[0]));
        mismatches.check(&actual, &case[1], format!("runUrl({})", case[0]));
    }
    mismatches.finish("карточка профиля");
}

/// `state()` по случаю фикстуры (`raw`, `local`, `connected`, `localSecrets`, `localPresence`).
fn state_of(case: &Value) -> Value {
    let normalized = profile_state::normalize(&case["raw"]);
    let secrets: Vec<String> = case
        .get("localSecrets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    profile_state::state(
        normalized.as_ref(),
        local_account(&case["local"]).as_ref(),
        case["connected"] == json!(true),
        &secrets,
        local_presence(case.get("localPresence")).as_ref(),
        None,
        None,
    )
}

#[test]
fn profile_state_049_matches_node() {
    let fixture = fixture("profile-state-049.json");
    let mut mismatches = Mismatches::default();
    mismatches.check(
        &json!(anniversary_badges::IDS),
        &fixture["constants"]["anniversaryIds"],
        "anniversary-badges IDS",
    );
    for (index, case) in items(&fixture["normalize"]).iter().enumerate() {
        let actual = json!(profile_state::normalize(&case[0]));
        mismatches.check(
            &actual,
            &case[1],
            format!("normalize #{index}: {}", case[0]),
        );
    }
    for (index, case) in items(&fixture["listEntry"]).iter().enumerate() {
        let actual = json!(profile_state::list_entry(&case[0]));
        mismatches.check(&actual, &case[1], format!("listEntry #{index}"));
    }
    for (index, case) in items(&fixture["state"]).iter().enumerate() {
        mismatches.check(&state_of(case), &case["state"], format!("state #{index}"));
    }
    mismatches.finish("карточка профиля 0.4.9");
}

fn ids(value: &Value) -> Vec<i64> {
    items(value).iter().map(|id| id.as_i64().unwrap()).collect()
}

#[test]
fn moderation_matches_node() {
    let fixture = fixture("profile-model.json");
    let cases = &fixture["moderation"];
    let raws: Vec<&Value> = items(&cases["normalize"])
        .iter()
        .map(|case| &case[0])
        .collect();
    let mut mismatches = Mismatches::default();
    for (index, case) in items(&cases["normalize"]).iter().enumerate() {
        mismatches.check(
            &moderation::normalize(&case[0]),
            &case[1],
            format!("normalize #{index}"),
        );
    }
    for case in items(&cases["activeAt"]) {
        let actual = json!(moderation::active_at(&case[0], case[1].as_f64().unwrap()));
        mismatches.check(
            &actual,
            &case[2],
            format!("activeAt({}, {})", case[0], case[1]),
        );
    }
    for case in items(&cases["unseen"]) {
        let state = moderation::normalize(raws[case[1].as_u64().unwrap() as usize]);
        let actual = json!(moderation::unseen(&state, &ids(&case[0])));
        mismatches.check(
            &actual,
            &case[2],
            format!("unseen({}, #{})", case[0], case[1]),
        );
    }
    for case in items(&cases["remember"]) {
        let seen = ids(&case[0]);
        let state = moderation::normalize(raws[case[1].as_u64().unwrap() as usize]);
        let actual = json!(moderation::remember(
            &seen,
            &moderation::unseen(&state, &seen)
        ));
        mismatches.check(&actual, &case[2], format!("remember #{}", case[1]));
    }
    mismatches.finish("модерация");
}

/// Серия дней считается по местному времени: каждый пояс фикстуры — дочерний процесс этого же
/// теста с `TZ` (chrono читает `TZ`, как V8).
#[cfg(unix)]
#[test]
fn launch_streak_matches_node_in_time_zones() {
    let fixture = fixture("profile-model.json");
    let exe = std::env::current_exe().unwrap();
    for section in items(&fixture["streak"]) {
        let tz = str_of(&section["tz"]);
        // Дочерний тест в конце пишет в этот файл свой пояс: «прошёл» определяется им, а не
        // подстрокой в выводе харнесса (при `--exact` без совпадения код выхода тоже 0).
        let done = tempfile::tempdir().unwrap();
        let marker = done.path().join("done");
        let output = std::process::Command::new(&exe)
            .args([
                "--exact",
                "launch_streak_child",
                "--ignored",
                "--test-threads=1",
            ])
            .env("TZ", tz)
            .env("HLSR_STREAK_TZ", tz)
            .env("HLSR_STREAK_DONE", &marker)
            .output()
            .unwrap();
        assert!(
            output.status.success() && std::fs::read_to_string(&marker).ok().as_deref() == Some(tz),
            "{tz}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
#[ignore = "запускается из launch_streak_matches_node_in_time_zones с нужным TZ"]
fn launch_streak_child() {
    let Ok(tz) = std::env::var("HLSR_STREAK_TZ") else {
        return;
    };
    let fixture = fixture("profile-model.json");
    let section = items(&fixture["streak"])
        .iter()
        .find(|section| section["tz"] == json!(tz))
        .unwrap();
    let mut mismatches = Mismatches::default();
    for case in items(&section["dayOf"]) {
        let actual = json!(launch_streak::day_of(case[0].as_i64().unwrap()));
        mismatches.check(&actual, &case[1], format!("{tz}: dayOf({})", case[0]));
    }
    for case in items(&section["previousDay"]) {
        let actual = json!(launch_streak::previous_day(str_of(&case[0])));
        mismatches.check(&actual, &case[1], format!("{tz}: previousDay({})", case[0]));
    }
    // `record` и `stats` 0.4.9 (рекорд, часы назад) — из achievements-049.json того же пояса.
    let fixture = support::fixture("achievements-049.json");
    let record = items(&fixture["streak"])
        .iter()
        .find(|section| section["tz"] == json!(tz))
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let dir = root.join("data");
    for (step, expected) in items(&record["steps"])
        .iter()
        .zip(items(&record["results"]))
    {
        let now = step["now"].as_i64().unwrap_or(0);
        let actual = match str_of(&step["op"]) {
            "record" => json!(launch_streak::record(&dir, now)),
            "stats" => launch_streak::stats(&dir, now),
            "files" => support::files(root),
            _ => {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join(launch_streak::FILE_NAME), str_of(&step["utf8"])).unwrap();
                Value::Null
            }
        };
        mismatches.check(&actual, expected, format!("{tz}: {step}"));
    }
    mismatches.finish("серия дней");
    if let Ok(marker) = std::env::var("HLSR_STREAK_DONE") {
        std::fs::write(marker, &tz).unwrap();
    }
}

/// Самотесты 0.4.7 в местном поясе машины (на Windows — без подмены пояса).
#[test]
fn launch_streak_selftest() {
    use chrono::{Local, TimeZone};
    let temp = tempfile::tempdir().unwrap();
    let day = |n: u32, hour: u32| {
        Local
            .with_ymd_and_hms(2026, 10, n, hour, 0, 0)
            .earliest()
            .unwrap()
            .timestamp_millis()
    };
    assert_eq!(launch_streak::record(temp.path(), day(1, 12)), 1);
    assert_eq!(launch_streak::record(temp.path(), day(1, 23)), 1);
    for n in 2..=7 {
        let hour = if n % 2 == 1 { 0 } else { 23 };
        assert_eq!(
            launch_streak::record(temp.path(), day(n, hour)),
            i64::from(n)
        );
    }
    assert_eq!(launch_streak::record(temp.path(), day(9, 12)), 1);
    assert_eq!(launch_streak::previous_day("2026-03-01"), "2026-02-28");
}

const ID: &str = "76561198000000111";

fn card(extra: Value) -> Value {
    let mut card = json!({
        "steamId64": ID, "personaName": "Fake runner", "registeredAt": "2026-10-05T10:00:00Z",
        "games": {}, "details": {}, "badges": ["verified"], "generationId": "a".repeat(32),
    });
    for (key, value) in extra.as_object().unwrap() {
        card[key] = value.clone();
    }
    card
}

/// `online-profile-selftest.js` и `launcher-ui-selftest.js` 0.4.7 — те же случаи.
#[test]
fn selftests_of_0_4_7() {
    // Аватар Steam с сервера, если в локальном кеше его нет.
    let profile = profile_state::normalize(&card(
        json!({"details": {"avatarUrl": "https://avatars.steamstatic.com/abc_full.jpg"}}),
    ))
    .unwrap();
    let local = LocalAccount {
        steam_id64: Some(ID.to_owned()),
        persona_name: Some("Fake runner".to_owned()),
        avatar_data_uri: None,
    };
    let state = profile_state::state(Some(&profile), Some(&local), true, &[], None, None, None);
    assert_eq!(
        state["account"]["avatarDataUri"],
        "https://avatars.steamstatic.com/abc_full.jpg"
    );
    let cached = LocalAccount {
        avatar_data_uri: Some("data:image/png;base64,AAAA".to_owned()),
        ..local
    };
    let state = profile_state::state(Some(&profile), Some(&cached), true, &[], None, None, None);
    assert_eq!(
        state["account"]["avatarDataUri"],
        "data:image/png;base64,AAAA"
    );

    // Скрытие SteamID — только true/false; приватность в списках — только явное true.
    let details = |extra: Value| {
        profile_state::normalize(&card(json!({"details": extra}))).unwrap()["details"].clone()
    };
    assert_eq!(details(json!({"hideSteamId": true}))["hideSteamId"], true);
    assert_eq!(details(json!({"hideSteamId": false}))["hideSteamId"], false);
    assert!(details(json!({})).get("hideSteamId").is_none());
    assert!(
        details(json!({"hideSteamId": "true"}))
            .get("hideSteamId")
            .is_none()
    );
    let privacy = details(json!({"hideFromPlayers": true, "hideGame": "true"}));
    assert_eq!(
        (
            privacy["hideFromPlayers"].clone(),
            privacy["hideGame"].clone()
        ),
        (json!(true), json!(false))
    );

    // Обычные достижения видны закрытыми, секретные до получения — замок без названия (0.4.9);
    // «Разминка» считает запуски.
    let games = json!({"hl1-won": {"launchCount": 10, "totalPlaySeconds": 60}, "hl2-2707": {"launchCount": 7, "totalPlaySeconds": 60}});
    let closed = achievements::evaluate(&games, None, &[], None);
    assert_eq!(closed.len(), achievements::ACHIEVEMENTS.len());
    let find = |list: &[Value], id: &str| list.iter().find(|item| item["id"] == id).cloned();
    let wake = find(&closed, "wake-up").unwrap();
    assert_eq!(
        [
            &wake["unlocked"],
            &wake["hidden"],
            &wake["title"],
            &wake["description"]
        ],
        [
            &json!(false),
            &json!(true),
            &json!("#L_ACH_SECRET_TITLE"),
            &json!("#L_ACH_SECRET_DESC")
        ]
    );
    assert_eq!(find(&closed, "scout").unwrap()["unlocked"], false);
    let warmup = find(&closed, "warmup").unwrap();
    assert_eq!(
        (
            warmup["unlocked"].clone(),
            warmup["value"].clone(),
            warmup["goal"].clone()
        ),
        (json!(false), json!(17), json!(25))
    );
    let open = achievements::evaluate(
        &json!({"hl1-won": {"launchCount": 40}}),
        None,
        &["scout", "almost"],
        None,
    );
    assert_eq!(find(&open, "scout").unwrap()["unlocked"], true);
    let almost = find(&open, "almost").unwrap();
    assert_eq!(
        (
            almost["secret"].clone(),
            almost.get("hidden"),
            almost["title"].clone()
        ),
        (json!(true), None, json!("#L_ACH_ALMOST_TITLE"))
    );
    assert_eq!(find(&open, "wake-up").unwrap()["hidden"], true);
    assert_eq!(find(&open, "warmup").unwrap()["unlocked"], true);

    // Рамка аватара и свои значки проходят те же проверки, что на сервере.
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" fill="#f0f"/></svg>"##;
    let profile = profile_state::normalize(&json!({
        "steamId64": "76561198000000001",
        "badges": ["developer", "c-0a1b2c3d", "c-ffffffff", "c-0000aaaa", "unknown"],
        "customBadges": [
            {"id": "c-0a1b2c3d", "label": {"ru": "Чемпион", "en": ""}, "note": {"ru": "За победу"}, "svg": svg},
            {"id": "c-0000aaaa", "label": {"ru": "В цвет темы"}, "svg": svg, "themed": true},
            {"id": "c-12345678", "label": {"ru": ""}, "svg": svg},
            {"id": "c-87654321", "label": {"ru": "Не SVG"}, "svg": "<html></html>"},
            {"id": "bad", "label": {"ru": "Неверный id"}, "svg": svg},
        ],
        "avatarFrame": {"css": "background: conic-gradient(from var(--frame-angle), #f0f, #0ff); animation: frame-spin 3s linear infinite"},
    }))
    .unwrap();
    assert_eq!(
        profile["badges"],
        json!(["developer", "c-0a1b2c3d", "c-0000aaaa"])
    );
    let badges = profile["customBadges"].as_array().unwrap();
    assert_eq!(
        badges
            .iter()
            .map(|b| b["themed"].clone())
            .collect::<Vec<_>>(),
        vec![json!(false), json!(true)]
    );
    assert_eq!(badges[0]["label"]["en"], "Чемпион");
    assert!(
        badges[0]["icon"]
            .as_str()
            .unwrap()
            .starts_with("data:image/svg+xml;base64,")
    );
    assert!(
        profile["avatarFrame"]["css"]
            .as_str()
            .unwrap()
            .contains("frame-spin")
    );
    let state = profile_state::state(
        Some(&profile),
        Some(&LocalAccount::default()),
        true,
        &[],
        None,
        None,
        None,
    );
    assert_eq!(state["avatarFrame"], profile["avatarFrame"]);
    assert_eq!(state["customBadges"].as_array().unwrap().len(), 2);
    for css in [
        "background: url(https://evil.example/x.png)",
        "width: 300px",
        "inset: -40px",
        "transform: scale(4)",
    ] {
        let frame = profile_state::normalize(
            &json!({"steamId64": "76561198000000001", "avatarFrame": {"css": css}}),
        )
        .unwrap();
        assert_eq!(frame["avatarFrame"], Value::Null, "принят CSS рамки: {css}");
    }

    // Онлайн и текущая игра: чужой профиль — с сервера, свой — из лаунчера.
    let id = "76561198000000001";
    let presence = |raw: Value, local: Option<LocalPresence>, connected: bool| {
        let profile = profile_state::normalize(&raw).unwrap();
        profile_state::state(
            Some(&profile),
            Some(&LocalAccount::default()),
            connected,
            &[],
            local.as_ref(),
            None,
            None,
        )["presence"]
            .clone()
    };
    assert_eq!(
        presence(
            json!({"steamId64": id, "presence": {"online": true, "gameId": "hl1-won"}}),
            None,
            true
        ),
        json!({"online": true, "gameId": "hl1-won", "launcherId": "70"})
    );
    assert_eq!(
        presence(
            json!({"steamId64": id, "presence": {"gameId": "ghosting-renewed"}}),
            None,
            true
        )["online"],
        true
    );
    assert_eq!(
        presence(json!({"steamId64": id}), None, true),
        json!({"online": false, "gameId": null, "launcherId": null})
    );
    let unknown = profile_state::normalize(
        &json!({"steamId64": id, "presence": {"online": true, "gameId": "cs16"}}),
    )
    .unwrap();
    assert_eq!(unknown["presence"]["gameId"], Value::Null);
    let own = LocalPresence {
        online: false,
        game_id: Some("hl2-4260".to_owned()),
    };
    assert_eq!(
        presence(
            json!({"steamId64": id, "presence": {"online": true}}),
            Some(own),
            false
        ),
        json!({"online": false, "gameId": "hl2-4260", "launcherId": "220"})
    );

    // Ссылки профиля: ник или адрес, только нужный сервис.
    let links = |field: &str, value: &str| profile_links::normalize(field, &json!(value));
    let some = |text: &str| Some(text.to_owned());
    assert_eq!(
        links("youtube", "@kanoyo"),
        some("https://www.youtube.com/@kanoyo")
    );
    assert_eq!(
        links("youtube", "kanoyo"),
        some("https://www.youtube.com/@kanoyo")
    );
    assert_eq!(
        links("youtube", "youtube.com/@kanoyo/videos"),
        some("https://www.youtube.com/@kanoyo")
    );
    assert_eq!(
        links("youtube", "http://m.youtube.com/@kanoyo"),
        some("https://www.youtube.com/@kanoyo")
    );
    assert_eq!(
        links("youtube", "youtube.com/channel/UC1234567890123456789012"),
        some("https://www.youtube.com/channel/UC1234567890123456789012")
    );
    assert_eq!(links("youtube", "https://www.twitch.tv/kanoyo"), None);
    assert_eq!(links("youtube", "youtube.com/watch?v=abc"), None);
    assert_eq!(
        links("youtube", "https://youtube.com@evil.example/@x"),
        None
    );
    assert_eq!(links("youtube", ""), some(""));
    assert_eq!(
        links("twitch", "Kanoyo"),
        some("https://www.twitch.tv/Kanoyo")
    );
    assert_eq!(
        links("twitch", "twitch.tv/kanoyo/videos"),
        some("https://www.twitch.tv/kanoyo")
    );
    assert_eq!(links("twitch", "twitch.tv/directory"), None);
    assert_eq!(links("twitch", "kick.com/kanoyo"), None);
    let name = |field: &str, value: &str| profile_links::channel_name(field, &json!(value));
    assert_eq!(
        name("youtube", "https://www.youtube.com/@kanoyo"),
        some("@kanoyo")
    );
    assert_eq!(
        name(
            "youtube",
            "https://www.youtube.com/channel/UC1234567890123456789012"
        ),
        None
    );
    assert_eq!(
        name("twitch", "https://www.twitch.tv/Kanoyo"),
        some("Kanoyo")
    );

    // Стиль ника: только каталог и безопасный CSS текста.
    assert_eq!(
        nick_styles::sanitize_css(&json!(
            " COLOR : #ff0 ;text-shadow: 0 0 8px rgba(255, 255, 0, .5); "
        )),
        some("color: #ff0; text-shadow: 0 0 8px rgba(255, 255, 0, .5)")
    );
    for css in [
        "background: url(https://evil.example/x.png)",
        "color: red; } body { display: none",
        "width: 9999px",
        "position: fixed",
        "font-size: 200px",
        "content: \"x\"",
        "@import x",
        "color: \\72 ed",
        "color: rgb(1, 2",
        "color red",
    ] {
        assert_eq!(nick_styles::sanitize_css(&json!(css)), None, "{css}");
    }
    assert_eq!(
        nick_styles::normalize(&json!({"style": "developer", "css": ""})),
        Some(json!({"style": "developer", "css": ""}))
    );
    assert_eq!(
        nick_styles::normalize(&json!({"style": "retired", "css": "width: 1px"})),
        None
    );
    let profile = profile_state::normalize(&json!({"steamId64": "76561198000000000", "personaName": "x", "games": {}, "details": {}, "nick": {"style": "gold"}})).unwrap();
    let state = profile_state::state(
        Some(&profile),
        Some(&LocalAccount::default()),
        true,
        &[],
        None,
        None,
        None,
    );
    assert_eq!(state["nick"], json!({"style": "gold", "css": ""}));
}
