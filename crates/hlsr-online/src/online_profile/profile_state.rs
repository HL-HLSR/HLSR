//! Карточка профиля (`online-profile/profile-state.js`): `normalize` того, что прислал сервер,
//! строка списка игроков (`listEntry`) и снимок для окна (`state`: SteamID2, достижения, сводка,
//! присутствие). Сервер своим не считается: всё проходит те же проверки, что на сервере.

use crate::js_value::{coerced, field, finite_number, is_object, safe_integer, text};
use crate::profile::achievements;
use crate::shared::{anniversary_badges, avatar_frames, custom_badges, nick_styles};
use hlsr_core::js;
use hlsr_core::json_text::js_number;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::sync::LazyLock;

pub const GAMES: [&str; 5] = [
    "ghosting-renewed",
    "hl2-2707",
    "hl2-4260",
    "hl2-5288",
    "hl1-won",
];
/// Встроенные значки: `developer`, `verified`, `bughunter`, `booster` и годовщины
/// ([`anniversary_badges::IDS`]).
pub const BADGES: [&str; 4] = ["developer", "verified", "bughunter", "booster"];

fn known_badge(id: &str) -> bool {
    BADGES.contains(&id) || anniversary_badges::IDS.contains(&id)
}
/// `LAUNCHER_IDS`: id игры в лаунчере (вкладка) по id игры профиля.
pub const LAUNCHER_IDS: [(&str, &str); 5] = [
    ("ghosting-renewed", "218"),
    ("hl2-2707", "220"),
    ("hl2-4260", "220"),
    ("hl2-5288", "220"),
    ("hl1-won", "70"),
];

/// Наибольшее время и число запусков в карточке (40 лет в секундах).
const MAX_STAT: f64 = 1_261_440_000.0;
/// SteamID64 нулевого аккаунта.
const STEAM_ID64_BASE: i128 = 76_561_197_960_265_728;

/// `LAUNCHER_IDS[gameId]`.
pub fn launcher_id(game_id: &str) -> Option<&'static str> {
    LAUNCHER_IDS
        .iter()
        .find(|(id, _)| *id == game_id)
        .map(|(_, launcher)| *launcher)
}

fn known_game(value: Option<&Value>) -> Option<&'static str> {
    let id = value?.as_str()?;
    GAMES.iter().find(|game| **game == id).copied()
}

/// `number(value)`: целое 0…1261440000 или 0.
fn number(value: Option<&Value>) -> Value {
    let x = safe_integer(value).filter(|x| (0.0..=MAX_STAT).contains(&(*x as f64)));
    js_number(x.unwrap_or(0) as f64)
}

/// `date(value)`: строка, которую принимает `Date.parse`, или `None`.
pub fn date(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?;
    js::date_parse(text).map(|_| text.to_owned())
}

fn date_value(value: Option<&Value>) -> Value {
    date(value).map_or(Value::Null, Value::String)
}

/// `since(value)`: `boosterSince` и `supporterSince` приходят в unix-секундах (начало буста и
/// первой оплаты); не целое в 1…253402300799 — `null`.
pub fn since(value: Option<&Value>) -> Value {
    json!(safe_integer(value).filter(|x| (1..=253_402_300_799).contains(x)))
}

/// `plainObject(value)`: объект, но не массив.
fn plain_object(value: Option<&Value>) -> Option<&Value> {
    value.filter(|value| value.is_object())
}

/// `perks(raw)`: права поддержавшего проект (только свой профиль): `styles` — можно выбрать
/// заготовку подсветки; `subscriber` — была ли подписка, активна ли и до какого числа. Ключа нет —
/// права нет.
fn perks(raw: Option<&Value>) -> Value {
    let Some(raw) = plain_object(raw) else {
        return json!({"styles": false, "subscriber": null});
    };
    let subscriber = plain_object(raw.get("subscriber")).map(|sub| {
        json!({"active": sub.get("active") == Some(&Value::Bool(true)), "until": date_value(sub.get("until"))})
    });
    json!({"styles": raw.get("styles") == Some(&Value::Bool(true)), "subscriber": subscriber})
}

/// `stylePreset(value)`: номер выбранной заготовки «подсветка ника + рамка» или `null`.
fn style_preset(value: Option<&Value>) -> Value {
    json!(safe_integer(value).filter(|x| *x > 0))
}

/// Привязка (`telegram`, `discordLink`): объект с `linked: true`, иначе не привязан.
fn linked(raw: Option<&Value>) -> Option<&Value> {
    plain_object(raw).filter(|raw| raw.get("linked") == Some(&Value::Bool(true)))
}

/// Имя из привязки без пробелов по краям (`trim`); не строка — пусто.
fn linked_name(raw: &Value) -> String {
    raw.get("username")
        .and_then(Value::as_str)
        .map_or_else(String::new, |name| js::trim(name).to_owned())
}

/// `telegram(raw)`: привязка Telegram к боту HLSR (только свой профиль): имя без «@» по
/// `^[A-Za-z0-9_]{1,64}$` или `null`. Ключа нет — не привязан.
fn telegram(raw: Option<&Value>) -> Value {
    let Some(raw) = linked(raw) else {
        return json!({"linked": false, "username": null});
    };
    let name = linked_name(raw);
    let valid = (1..=64).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    json!({"linked": true, "username": valid.then_some(name)})
}

/// `discordLink(raw)`: привязка Discord (только свой профиль; не путать с `details.discord` —
/// имя, которое игрок вписал сам): имя до 64 единиц UTF-16 и начало буста.
fn discord_link(raw: Option<&Value>) -> Value {
    let Some(raw) = linked(raw) else {
        return json!({"linked": false, "username": null, "boosterSince": null});
    };
    let name = linked_name(raw);
    let fits = !name.is_empty() && js::text::utf16_len(&name) <= 64;
    json!({"linked": true, "username": fits.then_some(name), "boosterSince": since(raw.get("boosterSince"))})
}

/// `RUN_PAGE`: страница забега на speedrun.com — `/<игра>/runs/<id>` (флаг `i` без `u`).
pub static RUN_PAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i-u)^https://www\.speedrun\.com/[A-Za-z0-9_-]{1,64}/runs?/([a-z0-9]{8})$")
        .expect("шаблон RUN_PAGE")
});

/// `runUrl(run)`: адрес страницы забега, если он ведёт на этот забег, иначе служебный
/// `/runs/<id>` (его разрешает API speedrun.com при открытии).
pub fn run_url(run: &Value) -> String {
    let id = run.get("id");
    if let Some(url) = run.get("url").and_then(Value::as_str)
        && let Some(found) = RUN_PAGE.captures(url)
        && id.and_then(Value::as_str) == Some(&found[1])
    {
        return url.to_owned();
    }
    format!("https://www.speedrun.com/runs/{}", coerced(id))
}

/// `/^[a-z0-9]{8}$/i.test(value)`.
fn is_run_id(value: Option<&Value>) -> bool {
    let text = coerced(value);
    text.len() == 8 && text.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// `presence(raw)`: в сети и текущая игра; игра без отметки онлайна — всё равно в сети.
fn presence(raw: Option<&Value>) -> Map<String, Value> {
    let source = raw.filter(|raw| is_object(Some(raw)));
    let game_id = known_game(field(source, "gameId"));
    let online = field(source, "online") == Some(&Value::Bool(true)) || game_id.is_some();
    let mut out = Map::new();
    out.insert("online".to_owned(), json!(online));
    out.insert("gameId".to_owned(), json!(game_id));
    out
}

/// `rating(raw)`: сумма лайков и дизлайков и пометка «сомнительный».
pub fn rating(raw: &Value) -> Value {
    let count = |key: &str| {
        safe_integer(field(Some(raw), key))
            .filter(|x| *x >= 0)
            .unwrap_or(0)
    };
    if !is_object(Some(raw)) {
        return json!({"score": 0, "likes": 0, "dislikes": 0, "dubious": false});
    }
    let likes = count("likes");
    let dislikes = count("dislikes");
    json!({
        "score": js_number(likes as f64 - dislikes as f64),
        "likes": likes,
        "dislikes": dislikes,
        "dubious": raw.get("dubious") == Some(&Value::Bool(true)),
    })
}

/// `vote(raw)`: свой голос (1, −1 или 0) и когда его можно поменять.
pub fn vote(raw: &Value) -> Value {
    if !is_object(Some(raw)) {
        return json!({"value": 0, "nextAt": null});
    }
    let value = raw
        .get("value")
        .and_then(Value::as_f64)
        .filter(|x| *x == 1.0 || *x == -1.0)
        .unwrap_or(0.0);
    json!({"value": js_number(value), "nextAt": date_value(raw.get("nextAt"))})
}

fn details(source: Option<&Value>) -> Value {
    let source = source.filter(|source| is_object(Some(source)));
    let mut details = Map::new();
    for key in [
        "status",
        "pronouns",
        "country",
        "discord",
        "youtube",
        "twitch",
        "avatarUrl",
    ] {
        details.insert(key.to_owned(), json!(text(field(source, key), 256)));
    }
    if let Some(hide @ Value::Bool(_)) = field(source, "hideSteamId") {
        details.insert("hideSteamId".to_owned(), hide.clone());
    }
    for key in ["hideFromPlayers", "hideGame"] {
        details.insert(
            key.to_owned(),
            json!(field(source, key) == Some(&Value::Bool(true))),
        );
    }
    let secrets: Vec<&Value> = field(source, "secrets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|id| id.as_str().is_some_and(achievements::is_stored))
        .collect();
    details.insert("secrets".to_owned(), json!(secrets));
    let speedrun = field(source, "speedrun").filter(|value| js::truthy(value));
    if speedrun.is_some() && is_run_id(field(speedrun, "id")) {
        details.insert(
            "speedrun".to_owned(),
            json!({
                "id": field(speedrun, "id"),
                "name": text(field(speedrun, "name"), 64),
            }),
        );
    }
    let pinned = field(source, "pinnedRun").filter(|value| js::truthy(value));
    if pinned.is_some()
        && is_run_id(field(pinned, "id"))
        && let Some(seconds) = finite_number(field(pinned, "seconds"))
    {
        let place = safe_integer(field(pinned, "place")).filter(|place| *place > 0);
        details.insert(
            "pinnedRun".to_owned(),
            json!({
                "id": field(pinned, "id"),
                "seconds": js_number(if seconds > 0.0 { seconds } else { 0.0 }),
                "game": text(field(pinned, "game"), 96),
                "category": text(field(pinned, "category"), 96),
                "place": place,
                "url": run_url(pinned.unwrap_or(&Value::Null)),
            }),
        );
    }
    Value::Object(details)
}

/// `normalize(raw)`: карточка профиля или `None`, если это не профиль Steam.
/// `steamId64` — только строка (0.4.7 приводил число к строке: сервер шлёт строку).
pub fn normalize(raw: &Value) -> Option<Value> {
    let steam_id = raw.get("steamId64").and_then(Value::as_str).filter(|id| {
        id.len() == 17 && id.starts_with("7656119") && id.bytes().all(|b| b.is_ascii_digit())
    })?;
    if !is_object(Some(raw)) {
        return None;
    }
    let raw_games = raw.get("games");
    let mut games = Map::new();
    for id in GAMES {
        let Some(game) = field(raw_games, id).filter(|game| is_object(Some(game))) else {
            continue;
        };
        games.insert(
            id.to_owned(),
            json!({
                "gameId": id,
                "totalPlaySeconds": number(game.get("totalPlaySeconds")),
                "launchCount": number(game.get("launchCount")),
                "lastLaunchAt": date_value(game.get("lastLaunchAt")),
                "lastExitAt": date_value(game.get("lastExitAt")),
            }),
        );
    }
    let custom = custom_badges::normalize(raw.get("customBadges").unwrap_or(&Value::Null));
    let badges: Vec<&Value> = raw
        .get("badges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|id| {
            id.as_str().is_some_and(|id| {
                known_badge(id) || custom.iter().any(|badge| badge["id"].as_str() == Some(id))
            })
        })
        .collect();
    let null = Value::Null;
    Some(json!({
        "steamId64": steam_id,
        "personaName": text(raw.get("personaName"), 64),
        "registeredAt": date_value(raw.get("registeredAt")),
        "badges": badges,
        "customBadges": custom,
        "nick": nick_styles::normalize(raw.get("nick").unwrap_or(&null)),
        // Подписка активна сейчас: значок у ника и голубая рамка профиля.
        "subscriber": raw.get("subscriber") == Some(&Value::Bool(true)),
        "boosterSince": since(raw.get("boosterSince")),
        "supporterSince": since(raw.get("supporterSince")),
        "avatarFrame": avatar_frames::normalize(raw.get("avatarFrame").unwrap_or(&null)),
        "presence": presence(raw.get("presence")),
        "games": games,
        "details": details(raw.get("details")),
        "rating": rating(raw.get("rating").unwrap_or(&null)),
        "deletionAt": date_value(raw.get("deletionAt")),
        "generationId": text(raw.get("generationId"), 32),
        "perks": perks(raw.get("perks")),
        "stylePreset": style_preset(raw.get("stylePreset")),
        "telegram": telegram(raw.get("telegram")),
        "discordLink": discord_link(raw.get("discordLink")),
    }))
}

/// `listEntry(raw)`: строка списка игроков — карточка, место в рейтинге и число достижений.
pub fn list_entry(raw: &Value) -> Option<Value> {
    let mut profile = normalize(raw)?;
    let count = |key: &str| safe_integer(raw.get(key)).filter(|x| *x > 0);
    let launcher = profile["presence"]["gameId"].as_str().and_then(launcher_id);
    profile["presence"]["launcherId"] = json!(launcher);
    profile["rank"] = json!(count("rank"));
    profile["achievementCount"] = json!(count("achievementCount").unwrap_or(0));
    Some(profile)
}

/// Свой аккаунт Steam на этом компьютере (`accountProvider()`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalAccount {
    pub steam_id64: Option<String>,
    pub persona_name: Option<String>,
    pub avatar_data_uri: Option<String>,
}

/// Свой онлайн: его лаунчер знает сам, без задержки сервера.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalPresence {
    pub online: bool,
    pub game_id: Option<String>,
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.is_empty())
}

/// `STEAM_0:<Y>:<Z>` из SteamID64 (как BigInt: остаток и частное к нулю).
fn steam_id2(steam_id64: &str) -> String {
    let account = steam_id64.parse::<i128>().unwrap_or(0) - STEAM_ID64_BASE;
    format!("STEAM_0:{}:{}", account % 2, account / 2)
}

fn empty_state() -> Value {
    let total = achievements::ACHIEVEMENTS.len() - achievements::BONUS_IDS.len();
    json!({
        "account": {"signedIn": false},
        "firstRunAt": null,
        "achievements": [],
        "summary": {"unlocked": 0, "total": total, "percent": 0},
        "stats": {"totalSeconds": 0, "gamesPlayed": 0, "launches": 0, "best": null},
        "badges": [],
        "customBadges": [],
        "nick": null,
        "subscriber": false,
        "boosterSince": null,
        "supporterSince": null,
        "avatarFrame": null,
        "details": {},
        "deletionAt": null,
        "rating": rating(&Value::Null),
        "presence": {"online": false, "gameId": null, "launcherId": null},
        "perks": perks(None),
        "stylePreset": null,
        "telegram": telegram(None),
        "discordLink": discord_link(None),
    })
}

/// `state(profile, localAccount, connected, localSecrets, localPresence, localTimes,
/// viewerSecrets)`: снимок для окна. `local_secrets` — достижения, открытые здесь и ещё не
/// дошедшие до сервера; `local_times` — когда получены свои (`achievement-times.json`);
/// `viewer_secrets` — открытое у смотрящего, когда профиль чужой: его секретные, которых смотрящий
/// не открыл, — замок.
pub fn state(
    profile: Option<&Value>,
    local_account: Option<&LocalAccount>,
    connected: bool,
    local_secrets: &[String],
    local_presence: Option<&LocalPresence>,
    local_times: Option<&Value>,
    viewer_secrets: Option<&[String]>,
) -> Value {
    let Some(profile) = profile.filter(|profile| js::truthy(profile)) else {
        return empty_state();
    };
    let null = Value::Null;
    let get = |key: &str| profile.get(key).unwrap_or(&null);
    let steam_id64 = get("steamId64").as_str().unwrap_or("");
    let details = get("details");
    let games = get("games");
    let mut secrets: Vec<&str> = details
        .get("secrets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    secrets.extend(local_secrets.iter().map(String::as_str));
    let all = achievements::evaluate(games, get("registeredAt").as_str(), &secrets, local_times);
    let known: Vec<&str> = match viewer_secrets {
        Some(viewer) => viewer.iter().map(String::as_str).collect(),
        None => secrets.clone(),
    };
    let summary = achievements::summarize(&all);
    let list = achievements::order(achievements::mask(all, &known));
    let best = achievements::best_game(games);
    let same_local =
        local_account.filter(|account| account.steam_id64.as_deref() == Some(steam_id64));
    let persona_name = non_empty(get("personaName").as_str())
        .or_else(|| non_empty(same_local.and_then(|a| a.persona_name.as_deref())))
        .unwrap_or(steam_id64);
    let avatar = non_empty(same_local.and_then(|a| a.avatar_data_uri.as_deref()))
        .or_else(|| non_empty(details.get("avatarUrl").and_then(Value::as_str)));
    let mut shown = match local_presence {
        Some(local) => {
            let mut shown = Map::new();
            shown.insert("online".to_owned(), json!(local.online));
            let game = local.game_id.as_ref().map(|id| json!(id));
            shown.insert("gameId".to_owned(), json!(known_game(game.as_ref())));
            shown
        }
        None => presence(profile.get("presence")),
    };
    let launcher = shown["gameId"].as_str().and_then(launcher_id);
    shown.insert("launcherId".to_owned(), json!(launcher));
    let game_list: Vec<&Value> = games
        .as_object()
        .into_iter()
        .flat_map(Map::values)
        .collect();
    let total_seconds: f64 = game_list
        .iter()
        .filter_map(|game| game.get("totalPlaySeconds").and_then(Value::as_f64))
        .sum();
    let games_played = game_list
        .iter()
        .filter(|game| {
            game.get("totalPlaySeconds")
                .and_then(Value::as_f64)
                .is_some_and(|x| x > 0.0)
        })
        .count();
    let best = best.map(|(game_id, seconds)| {
        json!({"gameId": game_id, "seconds": js_number(seconds), "launcherId": launcher_id(&game_id)})
    });
    let or_default = |key: &str, default: Value| {
        profile
            .get(key)
            .filter(|value| js::truthy(value))
            .cloned()
            .unwrap_or(default)
    };
    json!({
        "account": {
            "signedIn": true,
            "steamId64": get("steamId64"),
            "steamId2": steam_id2(steam_id64),
            "personaName": persona_name,
            "avatarDataUri": avatar,
            "connection": if connected { "connected" } else { "offline" },
        },
        "firstRunAt": get("registeredAt"),
        "achievements": list,
        "summary": summary,
        "stats": {"totalSeconds": js_number(total_seconds), "gamesPlayed": games_played,
            "launches": js_number(achievements::total_launches(games)), "best": best},
        "badges": get("badges"),
        "customBadges": or_default("customBadges", json!([])),
        "nick": get("nick"),
        "subscriber": get("subscriber") == &Value::Bool(true),
        "boosterSince": since(profile.get("boosterSince")),
        "supporterSince": since(profile.get("supporterSince")),
        "avatarFrame": or_default("avatarFrame", Value::Null),
        "details": details,
        "deletionAt": get("deletionAt"),
        "games": games,
        "rating": or_default("rating", rating(&Value::Null)),
        "presence": shown,
        // Разбор того же вида повторяется: сюда может прийти и сырой профиль.
        "perks": perks(profile.get("perks")),
        "stylePreset": style_preset(profile.get("stylePreset")),
        "telegram": telegram(profile.get("telegram")),
        "discordLink": discord_link(profile.get("discordLink")),
    })
}
