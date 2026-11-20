//! Достижения профиля (`profile/achievements.js` 0.4.9): 38 штук шести видов. `install` есть у
//! всех; `hours`/`launches` считаются по статистике игр с сервера; `event` видно всем и открывается
//! событием; `secret` открывается событием, но до получения это замок без названия; `server`
//! выдаёт только сервер (дописывает id в `details.secrets`). `tier` — редкость (rainbow, gold,
//! purple); `secret` у серверного — показать как секретное; `bonus` — сверх 100 %.

use crate::js_value::{entries, field, safe_integer, to_number};
use hlsr_core::js;
use hlsr_core::json_text::js_number;
use serde_json::{Map, Value, json};

pub const HOUR_SECONDS: f64 = 3600.0;
pub const LAUNCHES_GOAL: u32 = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Install,
    Hours(u32),
    Event,
    Launches(u32),
    Secret,
    Server,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Install => "install",
            Kind::Hours(_) => "hours",
            Kind::Event => "event",
            Kind::Launches(_) => "launches",
            Kind::Secret => "secret",
            Kind::Server => "server",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Achievement {
    pub id: &'static str,
    /// Ключ локали (`#L_ACH_…`).
    pub title: &'static str,
    pub description: &'static str,
    pub icon: &'static str,
    pub kind: Kind,
    pub tier: Option<&'static str>,
    /// Серверное, показанное как секретное (замок до получения).
    pub secret: bool,
    /// Сверх 100 %: в счётчике «X из Y» его нет.
    pub bonus: bool,
}

const fn a(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    icon: &'static str,
    kind: Kind,
) -> Achievement {
    Achievement {
        id,
        title,
        description,
        icon,
        kind,
        tier: None,
        secret: false,
        bonus: false,
    }
}

impl Achievement {
    const fn tier(mut self, tier: &'static str) -> Achievement {
        self.tier = Some(tier);
        self
    }

    /// Описание как объект `ACHIEVEMENTS` 0.4.9.
    pub fn to_json(&self) -> Value {
        let mut out = json!({
            "id": self.id, "title": self.title, "description": self.description, "icon": self.icon,
            "kind": self.kind.as_str(),
        });
        match self.kind {
            Kind::Hours(hours) => out["hours"] = json!(hours),
            Kind::Launches(launches) => out["launches"] = json!(launches),
            _ => {}
        }
        if let Some(tier) = self.tier {
            out["tier"] = json!(tier);
        }
        if self.secret {
            out["secret"] = json!(true);
        }
        if self.bonus {
            out["bonus"] = json!(true);
        }
        out
    }
}

use Kind::{Event, Hours, Install, Launches, Secret, Server};

// Таблица — как в 0.4.9, строка на достижение.
#[rustfmt::skip]
pub const ACHIEVEMENTS: [Achievement; 38] = [
    a("first-launch", "#L_ACH_FIRST_TITLE", "#L_ACH_FIRST_DESC", "fas fa-download", Install),
    a("touch-grass", "#L_ACH_GRASS_TITLE", "#L_ACH_GRASS_DESC", "fas fa-seedling", Hours(50)),
    a("full-set", "#L_ACH_FULLSET_TITLE", "#L_ACH_FULLSET_DESC", "fas fa-layer-group", Event),
    a("warmup", "#L_ACH_WARMUP_TITLE", "#L_ACH_WARMUP_DESC", "fas fa-play", Launches(LAUNCHES_GOAL)),
    a("marathon", "#L_ACH_MARATHON_TITLE", "#L_ACH_MARATHON_DESC", "fas fa-hourglass-half", Event),
    a("week-streak", "#L_ACH_WEEK_TITLE", "#L_ACH_WEEK_DESC", "fas fa-calendar-check", Event),
    a("own-binds", "#L_ACH_BINDS_TITLE", "#L_ACH_BINDS_DESC", "fas fa-keyboard", Event),
    a("stopwatch", "#L_ACH_STOPWATCH_TITLE", "#L_ACH_STOPWATCH_DESC", "fas fa-stopwatch", Event),
    a("own-style", "#L_ACH_STYLE_TITLE", "#L_ACH_STYLE_DESC", "fas fa-paintbrush", Event),
    a("community-voice", "#L_ACH_VOICE_TITLE", "#L_ACH_VOICE_DESC", "fas fa-heart", Event),
    a("scout", "#L_ACH_SCOUT_TITLE", "#L_ACH_SCOUT_DESC", "fas fa-binoculars", Event),
    a("chronicler", "#L_ACH_CHRONICLER_TITLE", "#L_ACH_CHRONICLER_DESC", "fas fa-scroll", Event),
    a("back-to-past", "#L_ACH_PAST_TITLE", "#L_ACH_PAST_DESC", "fas fa-history", Secret),
    a("wake-up", "#L_ACH_WAKE_TITLE", "#L_ACH_WAKE_DESC", "fas fa-briefcase", Secret),
    a("almost", "#L_ACH_ALMOST_TITLE", "#L_ACH_ALMOST_DESC", "fas fa-hourglass-end", Secret),
    a("night-shift", "#L_ACH_NIGHT_TITLE", "#L_ACH_NIGHT_DESC", "fas fa-moon", Secret),
    a("found-self", "#L_ACH_SELF_TITLE", "#L_ACH_SELF_DESC", "fas fa-user-check", Secret),
    // 0.4.9. Часы и запуски — те же тексты с числом, что у «Потрогай траву» и «Разминки».
    a("double-shift", "#L_ACH_DOUBLE_TITLE", "#L_ACH_GRASS_DESC", "fas fa-business-time", Hours(100)),
    a("cursed", "#L_ACH_CURSED_TITLE", "#L_ACH_GRASS_DESC", "fas fa-skull", Hours(500)).tier("purple"),
    a("avid-runner", "#L_ACH_AVID_TITLE", "#L_ACH_GRASS_DESC", "fas fa-person-running", Hours(1000)).tier("gold"),
    a("mountain-dweller", "#L_ACH_MOUNTAIN_TITLE", "#L_ACH_WARMUP_DESC", "fas fa-mountain", Launches(500)).tier("purple"),
    a("shut-it-down", "#L_ACH_SHUT_TITLE", "#L_ACH_WARMUP_DESC", "fas fa-power-off", Launches(1000)).tier("gold"),
    a("around-the-clock", "#L_ACH_CLOCK_TITLE", "#L_ACH_CLOCK_DESC", "fas fa-clock", Event).tier("purple"),
    a("hard-worker", "#L_ACH_WORKER_TITLE", "#L_ACH_WORKER_DESC", "fas fa-hammer", Event),
    a("day-after-day", "#L_ACH_DAYS_TITLE", "#L_ACH_DAYS_DESC", "fas fa-calendar-days", Event).tier("purple"),
    a("keep-the-flame", "#L_ACH_FLAME_TITLE", "#L_ACH_FLAME_DESC", "fas fa-fire-flame-curved", Event).tier("gold"),
    a("frankenstein", "#L_ACH_FRANK_TITLE", "#L_ACH_FRANK_DESC", "fas fa-bolt", Event),
    a("hoarder", "#L_ACH_HOARDER_TITLE", "#L_ACH_HOARDER_DESC", "fas fa-box-archive", Event),
    // «Монтажёр»: открывает main, когда во вкладке «Демо» готов экспорт клипа.
    a("video-editor", "#L_ACH_EDITOR_TITLE", "#L_ACH_EDITOR_DESC", "fas fa-film", Event),
    a("patron", "#L_ACH_PATRON_TITLE", "#L_ACH_PATRON_DESC", "fas fa-hand-holding-heart", Server).tier("gold"),
    Achievement {
        secret: true,
        bonus: true,
        ..a("king-of-the-hill", "#L_ACH_KING_TITLE", "#L_ACH_KING_DESC", "fas fa-crown", Server).tier("rainbow")
    },
    a("particle-accelerator", "#L_ACH_BOOSTER_TITLE", "#L_ACH_BOOSTER_DESC", "fas fa-atom", Server).tier("gold"),
    a("respected", "#L_ACH_RESPECT_TITLE", "#L_ACH_RESPECT_DESC", "fas fa-star", Server),
    a("piano", "#L_ACH_PIANO_TITLE", "#L_ACH_PIANO_DESC", "fas fa-music", Secret),
    a("meet-the-maker", "#L_ACH_MAKER_TITLE", "#L_ACH_MAKER_DESC", "fas fa-code", Secret),
    a("impossible", "#L_ACH_IMPOSSIBLE_TITLE", "#L_ACH_IMPOSSIBLE_DESC", "fas fa-ban", Secret),
    a("good-memory", "#L_ACH_MEMORY_TITLE", "#L_ACH_MEMORY_DESC", "fas fa-brain", Secret).tier("gold"),
    a("hesoyam", "#L_ACH_HESOYAM_TITLE", "#L_ACH_HESOYAM_DESC", "fas fa-car", Secret),
];

/// Вид `secret` и серверные с `secret: true`.
pub const SECRET_IDS: [&str; 11] = [
    "back-to-past",
    "wake-up",
    "almost",
    "night-shift",
    "found-self",
    "king-of-the-hill",
    "piano",
    "meet-the-maker",
    "impossible",
    "good-memory",
    "hesoyam",
];

pub const BONUS_IDS: [&str; 1] = ["king-of-the-hill"];

/// Всё, что открывается событием: хранится списком на сервере (`details.secrets`) и в
/// `secrets.json`.
pub const UNLOCK_IDS: [&str; 26] = [
    "full-set",
    "marathon",
    "week-streak",
    "own-binds",
    "stopwatch",
    "own-style",
    "community-voice",
    "scout",
    "chronicler",
    "back-to-past",
    "wake-up",
    "almost",
    "night-shift",
    "found-self",
    "around-the-clock",
    "hard-worker",
    "day-after-day",
    "keep-the-flame",
    "frankenstein",
    "hoarder",
    "video-editor",
    "piano",
    "meet-the-maker",
    "impossible",
    "good-memory",
    "hesoyam",
];

/// Их сервер дописывает в `details.secrets` сам; через `unlock` их не открыть.
pub const SERVER_IDS: [&str; 4] = [
    "patron",
    "king-of-the-hill",
    "particle-accelerator",
    "respected",
];

/// `STORED_IDS = UNLOCK_IDS.concat(SERVER_IDS)`: что принимается из `details.secrets`.
pub fn is_stored(id: &str) -> bool {
    UNLOCK_IDS.contains(&id) || SERVER_IDS.contains(&id)
}

/// Эти открывает сам интерфейс; остальные — только main по своим событиям.
pub const RENDERER_UNLOCK_IDS: [&str; 9] = [
    "back-to-past",
    "wake-up",
    "night-shift",
    "chronicler",
    "frankenstein",
    "hoarder",
    "piano",
    "good-memory",
    "hesoyam",
];

pub const TIERS: [&str; 3] = ["rainbow", "gold", "purple"];

/// Наибольшее время `Date` (9999-12-31T23:59:59.999Z).
const MAX_TIME: i64 = 253_402_300_799_999;

/// Описание по id.
pub fn find(id: &str) -> Option<&'static Achievement> {
    ACHIEVEMENTS.iter().find(|item| item.id == id)
}

/// `games[id] && Number(games[id][key])`: ложная запись — сама запись (`NaN` у не-числа).
fn stat(entry: &Value, key: &str) -> f64 {
    if js::truthy(entry) {
        to_number(field(Some(entry), key))
    } else {
        match entry {
            Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
            _ => f64::NAN,
        }
    }
}

/// `totalLaunches(games)`.
pub fn total_launches(games: &Value) -> f64 {
    entries(Some(games))
        .into_iter()
        .map(|(_, entry)| stat(entry, "launchCount"))
        .filter(|count| count.is_finite() && *count > 0.0)
        .sum()
}

/// `bestGame(games)`: игра с наибольшим временем (первая из равных).
pub fn best_game(games: &Value) -> Option<(String, f64)> {
    let mut best: Option<(String, f64)> = None;
    for (game_id, entry) in entries(Some(games)) {
        let seconds = stat(entry, "totalPlaySeconds");
        if !seconds.is_finite() || seconds <= 0.0 {
            continue;
        }
        if best.as_ref().is_none_or(|(_, top)| seconds > *top) {
            best = Some((game_id, seconds));
        }
    }
    best
}

fn assign(out: &mut Map<String, Value>, rest: Value) {
    if let Value::Object(rest) = rest {
        for (key, value) in rest {
            out.insert(key, value);
        }
    }
}

/// `HIDDEN`: секретное без названия — не открытое или открытое у того игрока, но не у смотрящего
/// (`SEALED`: другое описание и значок).
fn hide(item: &mut Map<String, Value>, sealed: bool) {
    let (description, icon) = if sealed {
        ("#L_ACH_SEALED_DESC", "fas fa-lock")
    } else {
        ("#L_ACH_SECRET_DESC", "fas fa-question")
    };
    assign(
        item,
        json!({"title": "#L_ACH_SECRET_TITLE", "description": description, "icon": icon,
            "tier": null, "hidden": true}),
    );
}

/// `evaluate({games, firstRunAt, unlocked, times})`: все 38 в порядке таблицы; секретное до
/// получения — замок (`hidden`). `times` — `{id: мс}` этого аккаунта.
pub fn evaluate(
    games: &Value,
    first_run_at: Option<&str>,
    unlocked: &[&str],
    times: Option<&Value>,
) -> Vec<Value> {
    let best = best_game(games);
    let best_hours = best
        .as_ref()
        .map_or(0.0, |(_, seconds)| seconds / HOUR_SECONDS);
    let launches = total_launches(games);
    let first_run_at = first_run_at.filter(|at| !at.is_empty());
    let at = |id: &str| {
        field(times, id)
            .and_then(Value::as_i64)
            .filter(|time| *time != 0)
            .map(js::iso::iso_from_ms)
    };
    ACHIEVEMENTS
        .iter()
        .map(|item| {
            let mut out = Map::new();
            assign(
                &mut out,
                json!({"id": item.id, "title": item.title, "description": item.description,
                    "icon": item.icon, "tier": item.tier}),
            );
            match item.kind {
                Install => assign(
                    &mut out,
                    json!({"unlocked": true, "progress": 1, "value": null, "goal": null, "at": first_run_at}),
                ),
                Event | Secret | Server => {
                    let open = unlocked.contains(&item.id);
                    assign(
                        &mut out,
                        json!({"unlocked": open, "progress": u8::from(open), "value": null,
                            "goal": null, "at": at(item.id)}),
                    );
                    if SECRET_IDS.contains(&item.id) {
                        out.insert("secret".to_owned(), json!(true));
                        if !open {
                            hide(&mut out, false);
                        }
                    }
                }
                Launches(goal) => {
                    let goal = f64::from(goal);
                    assign(
                        &mut out,
                        json!({
                            "unlocked": launches >= goal,
                            "progress": js_number((launches / goal).clamp(0.0, 1.0)),
                            "value": js_number(launches.min(goal)),
                            "goal": js_number(goal),
                            "at": at(item.id),
                        }),
                    );
                }
                Hours(goal) => {
                    let goal = f64::from(goal);
                    let progress = if goal > 0.0 {
                        (best_hours / goal).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    assign(
                        &mut out,
                        json!({
                            "unlocked": best_hours >= goal,
                            "progress": js_number(progress),
                            "value": js_number(best_hours.floor()),
                            "goal": js_number(goal),
                            "gameId": best.as_ref().map(|(id, _)| id),
                            "at": at(item.id),
                        }),
                    );
                }
            }
            Value::Object(out)
        })
        .collect()
}

/// `mask(list, known)`: секретное, которого смотрящий не открыл, — замок без названия, описания,
/// редкости и id (вместо id — `secret-N`, чтобы ключи списка были разными).
pub fn mask(list: Vec<Value>, known: &[&str]) -> Vec<Value> {
    let mut n = 0;
    list.into_iter()
        .map(|item| {
            let Value::Object(mut item) = item else {
                return item;
            };
            let id = item.get("id").and_then(Value::as_str).unwrap_or("");
            if !item.get("secret").is_some_and(js::truthy) || known.contains(&id) {
                return Value::Object(item);
            }
            n += 1;
            if !item.get("hidden").is_some_and(js::truthy) {
                hide(&mut item, true);
            }
            item.insert("id".to_owned(), json!(format!("secret-{n}")));
            Value::Object(item)
        })
        .collect()
}

/// `order(list)`: полученные по редкости (радужное, золотое, фиолетовое, обычные), потом
/// неполученные, в самом низу — секретные под замком.
pub fn order(mut list: Vec<Value>) -> Vec<Value> {
    let rank = |item: &Value| {
        if !item.get("unlocked").is_some_and(js::truthy) {
            return if item.get("hidden").is_some_and(js::truthy) {
                5
            } else {
                4
            };
        }
        let tier = item.get("tier").and_then(Value::as_str);
        TIERS
            .iter()
            .position(|known| Some(*known) == tier)
            .unwrap_or(3)
    };
    list.sort_by_key(rank);
    list
}

/// `summarize(list)`: `{unlocked, total, percent}` без бонусных (список — до `mask`).
pub fn summarize(list: &[Value]) -> Value {
    let items: Vec<&Value> = list
        .iter()
        .filter(|item| {
            !item
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| BONUS_IDS.contains(&id))
        })
        .collect();
    let unlocked = items
        .iter()
        .filter(|item| item.get("unlocked").is_some_and(js::truthy))
        .count();
    let total = items.len();
    let percent = if total == 0 {
        0.0
    } else {
        js::math_round(unlocked as f64 / total as f64 * 100.0)
    };
    json!({"unlocked": unlocked, "total": total, "percent": js_number(percent)})
}

/// `/^7656119\d{10}$/`.
fn is_steam_id(key: &str) -> bool {
    key.len() == 17 && key.starts_with("7656119") && key.bytes().all(|b| b.is_ascii_digit())
}

/// `accountTimes(raw)`: `{id: мс или null}` одного аккаунта; `null` — получено, но когда, неизвестно.
fn account_times(raw: &Map<String, Value>) -> Value {
    let mut out = Map::new();
    for item in ACHIEVEMENTS.iter().filter(|item| item.kind != Install) {
        match raw.get(item.id) {
            Some(Value::Null) => {
                out.insert(item.id.to_owned(), Value::Null);
            }
            value => {
                if let Some(time) = safe_integer(value).filter(|t| *t > 0 && *t <= MAX_TIME) {
                    out.insert(item.id.to_owned(), json!(time));
                }
            }
        }
    }
    Value::Object(out)
}

/// `readTimes(raw, account)`: `achievement-times.json` `{steamId64: {id: мс|null}}`. Плоский
/// `{id: мс}` первой сборки с датами — времена `account` (вход из сохранённой сессии); без входа
/// неизвестно чьи.
pub fn read_times(raw: Option<&Value>, account: Option<&str>) -> Map<String, Value> {
    let mut out = Map::new();
    let Some(Value::Object(raw)) = raw else {
        return out;
    };
    let flat = !raw.keys().any(|key| is_steam_id(key));
    // Порядок ключей: SteamID не индексы массива, у JS они в порядке файла, как у `Map`.
    let source: Vec<(&str, &Map<String, Value>)> = if flat {
        account
            .filter(|account| !account.is_empty())
            .map(|account| (account, raw))
            .into_iter()
            .collect()
    } else {
        raw.iter()
            .filter_map(|(key, value)| Some((key.as_str(), value.as_object()?)))
            .collect()
    };
    for (key, times) in source {
        if is_steam_id(key) {
            out.insert(key.to_owned(), account_times(times));
        }
    }
    out
}

/// `stamp(times, options, fresh, now)`: время тому, что открыто, но ещё не записано. Событийное —
/// только в момент своего unlock (`fresh`), порог и выданное сервером — когда его впервые увидели.
/// Без записи аккаунта (`times` нет) — `null`: дата обновления или входа была бы неправдой.
pub fn stamp(
    times: Option<&Map<String, Value>>,
    games: &Value,
    unlocked: &[&str],
    fresh: Option<&str>,
    now: i64,
) -> Map<String, Value> {
    let mut out = times.cloned().unwrap_or_default();
    for item in evaluate(games, None, unlocked, None) {
        let id = item["id"].as_str().unwrap_or("");
        let kind = find(id).map_or(Install, |entry| entry.kind);
        if item["unlocked"] != json!(true) || kind == Install || out.contains_key(id) {
            continue;
        }
        let known = times.is_some() && matches!(kind, Launches(_) | Hours(_) | Server);
        let at = if fresh == Some(id) || known {
            json!(now)
        } else {
            Value::Null
        };
        out.insert(id.to_owned(), at);
    }
    out
}
