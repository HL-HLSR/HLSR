use super::*;
use std::rc::Rc;

fn en() -> Tr {
    Rc::new(|key, args| crate::tr(Language::En, key, args))
}

fn ru() -> Tr {
    Rc::new(|key, args| crate::tr(Language::Ru, key, args))
}

fn t(key: &str, args: &[&str]) -> String {
    crate::tr(Language::En, key, args)
}

fn profile(id: &str, name: &str, extra: Value) -> Value {
    let mut profile = json!({
        "steamId64": id, "personaName": name, "nick": null, "avatarFrame": null,
        "details": {"avatarUrl": format!("https://a/{id}.png")},
        "presence": {"online": false, "gameId": null, "launcherId": null},
        "rank": 1, "games": {}, "achievementCount": 0, "rating": {"score": 0}, "registeredAt": null
    });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        profile[key] = value;
    }
    profile
}

fn ready() -> PlayersState {
    let mut players = PlayersState::new();
    players.set_active(true);
    players.set_signed_in(true, "1");
    players
}

fn online_reply(profiles: Vec<Value>, total: i64, playing: i64) -> Reply {
    Ok(
        json!({"status": "ok", "signedIn": true, "profiles": profiles, "total": total,
              "playing": playing, "more": false}),
    )
}

fn request(effects: &[Effect]) -> (Tag, Value) {
    effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Invoke { tag, args, .. } => Some((*tag, args[0].clone())),
            _ => None,
        })
        .expect("запрос")
}

#[test]
fn a_guest_sees_the_sign_in_invitation_and_nothing_is_asked() {
    let mut players = PlayersState::new();
    assert_eq!(players.set_active(true), vec![Effect::Poll(false)]);
    let view = players.view(Language::En, &en());
    assert_eq!(view.message, t("#L_PLAYERS_SIGN_IN", &[]));
    assert!(view.sign_in_button && view.rows.is_empty());
    assert!(players.refresh(false).is_empty());
}

#[test]
fn signing_in_on_the_open_tab_asks_the_online_list_and_starts_polling() {
    let mut players = PlayersState::new();
    players.set_active(true);
    let effects = players.set_signed_in(true, "1");
    assert_eq!(effects.len(), 2);
    let (tag, args) = request(&effects);
    assert_eq!(args, json!({"view": "online"}));
    assert_eq!(effects[1], Effect::Poll(true));
    assert_eq!(
        players.view(Language::En, &en()).message,
        t("#L_HOME_LOADING", &[])
    );
    let Tag::Online(id) = tag else {
        panic!("{tag:?}")
    };
    players.online_reply(
        id,
        online_reply(vec![profile("1", "Kanoyo", json!({}))], 12, 3),
    );
    let view = players.view(Language::En, &en());
    assert_eq!(view.rows.len(), 1);
    assert!(view.rows[0].me);
    assert_eq!(view.modes[0].count, "12");
    assert_eq!(view.summary, t("#L_PLAYERS_SUMMARY", &["12", "3"]));
}

#[test]
fn a_failed_poll_keeps_the_old_list_and_a_limit_is_named() {
    let mut players = ready();
    let (tag, _) = request(&players.refresh(false));
    let Tag::Online(id) = tag else { panic!() };
    players.online_reply(id, online_reply(vec![profile("2", "Zed", json!({}))], 1, 0));
    let (tag, _) = request(&players.poll());
    let Tag::Online(id) = tag else { panic!() };
    players.online_reply(id, Err("net".into()));
    let view = players.view(Language::En, &en());
    assert_eq!(view.rows.len(), 1);
    // Пустой список со сбоем — сообщение.
    let mut empty = ready();
    let (tag, _) = request(&empty.refresh(false));
    let Tag::Online(id) = tag else { panic!() };
    empty.online_reply(
        id,
        Ok(json!({"status": "error", "code": "RUNNER_HTTP_429"})),
    );
    assert_eq!(
        empty.view(Language::En, &en()).message,
        t("#L_PLAYERS_LIMIT", &[])
    );
    let (tag, _) = request(&empty.poll());
    let Tag::Online(id) = tag else { panic!() };
    empty.online_reply(id, Err("net".into()));
    assert_eq!(
        empty.view(Language::En, &en()).message,
        t("#L_PLAYERS_FAILED", &[])
    );
    // Вошёл другой игрок или ответ «не вошёл» — тоже сбой.
    let (tag, _) = request(&empty.poll());
    let Tag::Online(id) = tag else { panic!() };
    empty.online_reply(id, Ok(json!({"status": "ok", "signedIn": false})));
    assert!(empty.view(Language::En, &en()).rows.is_empty());
}

#[test]
fn stale_answers_are_dropped() {
    let mut players = ready();
    let (first, _) = request(&players.refresh(false));
    let (second, _) = request(&players.refresh(false));
    let (Tag::Online(first), Tag::Online(second)) = (first, second) else {
        panic!()
    };
    players.online_reply(
        first,
        online_reply(vec![profile("9", "Old", json!({}))], 1, 0),
    );
    assert!(players.view(Language::En, &en()).rows.is_empty());
    players.online_reply(
        second,
        online_reply(vec![profile("8", "New", json!({}))], 1, 0),
    );
    assert_eq!(players.ids(), ["8"]);
}

#[test]
fn switching_to_the_rating_asks_the_first_page_once_and_resets_the_scroll() {
    let mut players = ready();
    let effects = players.set_mode(Mode::Top);
    assert_eq!(effects[0], Effect::ResetScroll);
    let (tag, args) = request(&effects);
    assert_eq!(
        args,
        json!({"view": "top", "sort": "hours", "game": "", "offset": 0})
    );
    assert_eq!(*effects.last().unwrap(), Effect::Poll(false));
    let Tag::Top(id) = tag else { panic!() };
    players.top_reply(
        id,
        Ok(json!({"status": "ok", "signedIn": true, "more": true,
                  "profiles": [profile("1", "A", json!({"rank": 1})), profile("2", "B", json!({"rank": 2}))]})),
    );
    assert!(players.view(Language::En, &en()).more);
    // Возврат на вкладку уже загруженный рейтинг не перезапрашивает.
    players.set_mode(Mode::Online);
    let back = players.set_mode(Mode::Top);
    assert!(back.iter().all(|e| !matches!(
        e,
        Effect::Invoke {
            channel: "runner-players",
            ..
        }
    )));
    // «Ещё»: следующая страница с тем же рейтингом, дубликаты отбрасываются.
    let (tag, args) = request(&players.load_top(true));
    assert_eq!(args["offset"], 0);
    let Tag::Top(id) = tag else { panic!() };
    players.top_reply(
        id,
        Ok(json!({"status": "ok", "more": false, "profiles": [profile("2", "B", json!({})), profile("3", "C", json!({}))]})),
    );
    assert_eq!(players.ids(), ["1", "2", "3"]);
    assert!(!players.view(Language::En, &en()).more);
}

#[test]
fn the_page_offset_rounds_down_to_a_whole_page() {
    let mut players = ready();
    players.set_mode(Mode::Top);
    let many: Vec<Value> = (0..120)
        .map(|i| profile(&i.to_string(), "p", json!({})))
        .collect();
    let id = players.request_top;
    players.top_reply(
        id,
        Ok(json!({"status": "ok", "more": true, "profiles": many})),
    );
    let (_, args) = request(&players.load_top(true));
    assert_eq!(args["offset"], 100);
}

#[test]
fn sort_and_game_changes_reload_and_the_game_filter_exists_for_two_sorts_only() {
    let mut players = ready();
    players.set_mode(Mode::Top);
    let view = players.view(Language::En, &en());
    assert_eq!(view.sorts.len(), 5);
    assert_eq!(view.games.len(), 6);
    assert_eq!(view.games[0].label, t("#L_HOME_ALL_GAMES", &[]));
    assert_eq!(view.games[3].label, "HL2 4260");
    let (_, args) = request(&players.set_game("hl2-4260"));
    assert_eq!(args["game"], "hl2-4260");
    let (_, args) = request(&players.set_sort("launches"));
    assert_eq!(args["sort"], "launches");
    assert_eq!(args["game"], "hl2-4260");
    let (_, args) = request(&players.set_sort("rating"));
    assert_eq!(args["game"], "");
    assert_eq!(players.game(), "");
    assert!(players.view(Language::En, &en()).games.is_empty());
    assert!(players.set_sort("rating").is_empty());
}

fn rated(mut players: PlayersState, sort: &str, profiles: Vec<Value>) -> PlayersState {
    players.set_mode(Mode::Top);
    if sort != "hours" {
        players.set_sort(sort);
    }
    let id = players.request_top;
    players.top_reply(
        id,
        Ok(json!({"status": "ok", "more": false, "profiles": profiles})),
    );
    players
}

#[test]
fn values_and_units_follow_the_sort() {
    let games = json!({"hl1-won": {"totalPlaySeconds": 7200, "launchCount": 3},
                       "hl2-4260": {"totalPlaySeconds": 3600, "launchCount": 21}});
    let extra = json!({"games": games, "achievementCount": 4, "rating": {"score": 7},
                       "registeredAt": "2026-03-14T08:20:00.000Z"});
    let one = || vec![profile("5", "Q", extra.clone())];
    let view = |sort: &str, lang: Language, tr: &Tr| {
        rated(ready(), sort, one()).view(lang, tr).rows.remove(0)
    };
    // 3 ч суммарно (дробные часы округляются вниз до десятых).
    assert_eq!(
        view("hours", Language::En, &en()).value,
        t("#L_PLAYERS_HOURS", &["3"])
    );
    assert_eq!(view("hours", Language::En, &en()).unit, "");
    let launches = view("launches", Language::En, &en());
    assert_eq!(launches.value, "24");
    assert_eq!(launches.unit, t("#L_PLAYERS_UNIT_LAUNCHES_MANY", &[]));
    let achievements = view("achievements", Language::Ru, &ru());
    assert_eq!(achievements.value, "4");
    assert_eq!(
        achievements.unit,
        crate::tr(Language::Ru, "#L_PLAYERS_UNIT_ACHIEVEMENTS_FEW", &[])
    );
    let rating = view("rating", Language::En, &en());
    assert_eq!(rating.value, "+7");
    assert_eq!(rating.unit, t("#L_PLAYERS_UNIT_RATING", &[]));
    let new = view("new", Language::En, &en());
    assert_eq!(new.value, "14 Mar 2026");
    assert_eq!(new.unit, t("#L_PLAYERS_UNIT_JOINED", &[]));
    // 0.4.9: у «Новых» места нет, у прочих сортировок — место с сервера.
    assert_eq!((new.rank, rating.rank), (0, 1));
}

#[test]
fn the_subscriber_badge_needs_a_true_flag() {
    let rows = rated(
        ready(),
        "hours",
        vec![
            profile("1", "A", json!({"subscriber": true})),
            profile("2", "B", json!({"subscriber": "yes"})),
            profile("3", "C", json!({})),
        ],
    )
    .view(Language::En, &en())
    .rows;
    let flags: Vec<bool> = rows.iter().map(|row| row.subscriber).collect();
    assert_eq!(flags, [true, false, false]);
}

#[test]
fn hours_under_ten_keep_one_decimal_and_a_chosen_game_counts_alone() {
    let games = json!({"hl1-won": {"totalPlaySeconds": 5400, "launchCount": 3},
                       "hl2-4260": {"totalPlaySeconds": 90000, "launchCount": 1}});
    let mut players = rated(
        ready(),
        "hours",
        vec![profile("5", "Q", json!({"games": games}))],
    );
    // 25,5 ч: от десяти — целые.
    assert_eq!(
        players.view(Language::En, &en()).rows[0].value,
        t("#L_PLAYERS_HOURS", &["26"])
    );
    let (_, args) = request(&players.set_game("hl1-won"));
    assert_eq!(args["game"], "hl1-won");
    let id = players.request_top;
    players.top_reply(
        id,
        Ok(json!({"status": "ok", "more": false,
                  "profiles": [profile("5", "Q", json!({"games": {"hl1-won": {"totalPlaySeconds": 5400, "launchCount": 3}}}))]})),
    );
    assert_eq!(
        players.view(Language::En, &en()).rows[0].value,
        t("#L_PLAYERS_HOURS", &["1.5"])
    );
}

#[test]
fn presence_and_names_have_their_fallbacks() {
    let mut players = ready();
    let (tag, _) = request(&players.refresh(false));
    let Tag::Online(id) = tag else { panic!() };
    players.online_reply(
        id,
        online_reply(
            vec![
                profile("1", "kanoyo", json!({"presence": {"online": true, "gameId": "ghosting-renewed", "launcherId": "218"}})),
                profile("2", "", json!({"presence": {"online": true, "gameId": null, "launcherId": null}})),
                profile("3", "x", json!({"details": {}})),
            ],
            3,
            1,
        ),
    );
    let rows = players.view(Language::En, &en()).rows;
    assert_eq!(rows[0].presence, "playing");
    assert_eq!(
        rows[0].presence_text,
        t("#L_PRESENCE_PLAYING", &["HL2 Ghosting"])
    );
    assert_eq!(rows[0].letter, "K");
    assert_eq!(rows[1].presence, "online");
    assert_eq!(rows[1].name, "2");
    assert_eq!(rows[1].letter, "?");
    assert_eq!(rows[2].presence, "offline");
    assert_eq!(rows[2].avatar, "");
    // В «В сети» значения и места нет.
    assert!(rows.iter().all(|r| r.value.is_empty() && r.rank == 0));
}

#[test]
fn the_tab_polls_only_while_open_signed_in_and_online() {
    let mut players = PlayersState::new();
    assert_eq!(players.set_signed_in(true, "1"), vec![Effect::Poll(false)]);
    let opened = players.set_active(true);
    assert_eq!(*opened.last().unwrap(), Effect::Poll(true));
    let closed = players.set_active(false);
    assert_eq!(closed, vec![Effect::Poll(false)]);
    players.set_active(true);
    let rating = players.set_mode(Mode::Top);
    assert_eq!(*rating.last().unwrap(), Effect::Poll(false));
    // Выход из аккаунта сбрасывает списки.
    players.set_signed_in(false, "");
    assert!(players.view(Language::En, &en()).sign_in_button);
    assert_eq!(players.unmounted(), vec![Effect::Poll(false)]);
}

#[test]
fn a_hidden_window_does_not_poll_and_polling_comes_back_without_an_extra_request() {
    let mut players = ready();
    // HomePlayers.vue `schedule()`: `!document.hidden` — опроса нет, пока окно спрятано.
    assert_eq!(players.set_hidden(true), vec![Effect::Poll(false)]);
    assert_eq!(players.set_hidden(false), vec![Effect::Poll(true)]);
    players.set_mode(Mode::Top);
    assert_eq!(players.set_hidden(true), vec![Effect::Poll(false)]);
}
