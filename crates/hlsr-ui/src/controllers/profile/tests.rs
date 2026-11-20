use super::*;
use serde_json::json;
use std::rc::Rc;

fn en() -> Tr {
    Rc::new(|key, args| crate::tr(Language::En, key, args))
}

fn ru() -> Tr {
    Rc::new(|key, args| crate::tr(Language::Ru, key, args))
}

/// 2026-10-16 12:00 UTC, пояс UTC.
fn wall() -> WallClock {
    WallClock::fixed(1_792_152_000_000, 0)
}

const ME: &str = "76561199155902676";
const KATE: &str = "76561198000000002";

fn own_snapshot() -> Value {
    json!({
        "status": "signed-in", "userCode": null, "error": null, "ban": null, "mute": null,
        "account": {"signedIn": true, "steamId64": ME, "steamId2": "STEAM_0:0:597818474",
                    "personaName": "Kanoyo", "avatarDataUri": null, "connection": "connected"},
        "firstRunAt": "2026-03-14T08:20:00.000Z",
        "achievements": [
            {"id": "a", "title": "#L_ACH_FIRST_TITLE", "description": "#L_ACH_FIRST_DESC",
             "icon": "fas fa-download", "unlocked": true, "progress": 1, "value": null, "goal": null},
            {"id": "b", "title": "#L_ACH_GRASS_TITLE", "description": "#L_ACH_GRASS_DESC",
             "icon": "fas fa-seedling", "unlocked": false, "progress": 0.62, "value": 31, "goal": 50},
            {"id": "c", "title": "#L_ACH_FULLSET_TITLE", "description": "#L_ACH_FULLSET_DESC",
             "icon": "fas fa-layer-group", "unlocked": false, "progress": 0, "value": null, "goal": null},
        ],
        "summary": {"unlocked": 1, "total": 3, "percent": 33},
        "stats": {"totalSeconds": 512340, "gamesPlayed": 2,
                  "best": {"gameId": "ghosting-renewed", "seconds": 401220, "launcherId": "218"}},
        "badges": ["developer"], "customBadges": [],
        "nick": {"style": "developer", "css": null}, "avatarFrame": null,
        "details": {"status": "Бегу", "pronouns": "", "country": "RU", "hideSteamId": false,
                    "pinnedRun": null, "speedrun": null},
        "deletionAt": null,
        "rating": {"score": 14, "likes": 15, "dislikes": 1, "dubious": false},
        "presence": {"online": true, "gameId": null, "launcherId": null},
    })
}

fn foreign_snapshot() -> Value {
    let mut view = own_snapshot();
    view["account"] = json!({"signedIn": true, "steamId64": KATE, "steamId2": "STEAM_0:0:19",
        "personaName": "Kate", "avatarDataUri": "https://a/1.png", "connection": "connected"});
    view["badges"] = json!([]);
    view["nick"] = Value::Null;
    view["vote"] = json!({"value": 0, "nextAt": null});
    view
}

fn invoke(channel: &'static str, args: Vec<Value>, tag: Tag) -> Effect {
    Effect::Invoke { tag, channel, args }
}

fn view_of(state: &ProfileState) -> ProfileView {
    state.view(Language::En, &wall(), &en())
}

#[test]
fn own_profile_loads_profile_get_with_a_null_argument_then_syncs() {
    let mut state = ProfileState::new("", false);
    assert_eq!(
        state.load(),
        [invoke("profile-get", vec![Value::Null], Tag::Snapshot)]
    );
    // До ответа окно показывает вход: аккаунта ещё нет.
    assert_eq!(view_of(&state).kind, Kind::SignIn);
    let effects = state.answer(Tag::Snapshot, Ok(own_snapshot()));
    assert_eq!(
        effects,
        [
            Effect::ServerHideId(Some(false)),
            invoke("runner-sync", vec![], Tag::Ignored)
        ]
    );
    assert_eq!(view_of(&state).kind, Kind::Main);
    assert!(!view_of(&state).narrow);
}

#[test]
fn a_foreign_profile_first_asks_whether_we_are_signed_in() {
    let mut state = ProfileState::new(KATE, false);
    assert_eq!(view_of(&state).kind, Kind::Pending);
    assert!(view_of(&state).narrow);
    assert_eq!(state.load(), [invoke("runner-state", vec![], Tag::Own)]);
    let effects = state.answer(Tag::Own, Ok(own_snapshot()));
    assert_eq!(
        effects,
        [invoke(
            "runner-profile-view",
            vec![json!(KATE)],
            Tag::Snapshot
        )]
    );
    // Чужой профиль не синхронизируется и не сверяет SteamID.
    assert!(
        state
            .answer(Tag::Snapshot, Ok(foreign_snapshot()))
            .is_empty()
    );
    let view = view_of(&state);
    assert_eq!(view.kind, Kind::Main);
    assert_eq!(view.name, "Kate");
    assert_eq!(view.avatar, "https://a/1.png");
}

#[test]
fn a_guest_opening_a_foreign_profile_sees_the_sign_in() {
    let mut state = ProfileState::new(KATE, false);
    state.load();
    let guest = json!({"status": "signed-out", "account": {"signedIn": false}});
    assert!(state.answer(Tag::Own, Ok(guest)).is_empty());
    let view = view_of(&state);
    assert_eq!(view.kind, Kind::SignIn);
    assert!(view.guest_view);
    assert!(view.narrow);
    // Вошёл из окна — профиль загружается заново.
    let effects = state.profile_pushed(&own_snapshot());
    assert_eq!(effects, [invoke("runner-state", vec![], Tag::Own)]);
}

#[test]
fn failures_show_retry_for_foreign_and_the_network_text_for_own() {
    let mut foreign = ProfileState::new(KATE, false);
    foreign.load();
    foreign.answer(Tag::Own, Ok(own_snapshot()));
    foreign.answer(Tag::Snapshot, Ok(json!({"status": "error"})));
    let view = view_of(&foreign);
    assert_eq!((view.kind, view.failed), (Kind::Pending, true));
    // «Повторить»: снова грузится, ошибка снята.
    assert_eq!(foreign.load(), [invoke("runner-state", vec![], Tag::Own)]);
    assert!(!view_of(&foreign).failed);

    let mut own = ProfileState::new("", false);
    own.load();
    own.answer(Tag::Snapshot, Err("boom".into()));
    let view = view_of(&own);
    assert_eq!(view.kind, Kind::SignIn);
    assert_eq!(
        view.login_error,
        crate::tr(Language::En, "#L_RUNNER_NETWORK", &[])
    );
}

#[test]
fn a_missing_profile_shows_the_steam_id() {
    let mut state = ProfileState::new(KATE, false);
    state.load();
    state.answer(Tag::Own, Ok(own_snapshot()));
    state.answer(
        Tag::Snapshot,
        Ok(json!({"missing": true, "steamId64": KATE})),
    );
    let view = view_of(&state);
    assert_eq!((view.kind, view.target.as_str()), (Kind::Missing, KATE));
    assert!(view.narrow);
    assert!(view.tabs.is_empty());
    assert!(view.rating.is_none());
}

#[test]
fn known_login_errors_have_their_own_text() {
    let mut state = ProfileState::new("", false);
    state.set_consent(true);
    assert_eq!(
        state.sign_in(),
        [invoke("runner-login", vec![json!(true)], Tag::Login)]
    );
    let signed_out = |error: &str| json!({"status": "signed-out", "account": {"signedIn": false}, "error": error});
    state.answer(Tag::Login, Ok(signed_out("RUNNER_CLIENT_OUTDATED")));
    assert_eq!(
        view_of(&state).login_error,
        crate::tr(Language::En, "#L_RUNNER_CLIENT_OUTDATED", &[])
    );
    state.answer(Tag::Login, Ok(signed_out("SOMETHING_ELSE")));
    assert_eq!(
        view_of(&state).login_error,
        crate::tr(Language::En, "#L_RUNNER_NETWORK", &[])
    );
    // Без согласия вход не уходит.
    state.set_consent(false);
    assert!(state.sign_in().is_empty());
}

#[test]
fn a_pending_login_shows_the_code_and_cancel_returns_to_the_form() {
    let mut state = ProfileState::new("", false);
    state.set_consent(true);
    state.answer(
        Tag::Login,
        Ok(json!({"status": "pending", "userCode": "ABCD-EFGH", "account": {"signedIn": false}})),
    );
    let view = view_of(&state);
    assert!(view.login_pending);
    assert_eq!(view.login_code, "ABCD-EFGH");
    assert_eq!(
        state.cancel_login(),
        [invoke("runner-login-cancel", vec![], Tag::LoginCancel)]
    );
    state.answer(
        Tag::LoginCancel,
        Ok(json!({"status": "signed-out", "userCode": null, "account": {"signedIn": false}})),
    );
    assert!(!view_of(&state).login_pending);
}

#[test]
fn a_network_error_in_an_answer_is_not_a_snapshot() {
    let mut state = ProfileState::new("", false);
    state.apply_snapshot(Some(&own_snapshot()));
    assert_eq!(view_of(&state).name, "Kanoyo");
    state.apply_snapshot(Some(&json!({"status": "error"})));
    // Прежнее содержимое остаётся, добавляется ошибка входа.
    assert_eq!(view_of(&state).name, "Kanoyo");
    assert_eq!(state.login_error.as_deref(), Some("RUNNER_NETWORK"));
}

#[test]
fn the_hero_composes_status_extras_presence_and_the_id_button() {
    let mut snapshot = own_snapshot();
    snapshot["details"]["pronouns"] = json!("they");
    snapshot["presence"] = json!({"online": true, "gameId": "hl1-won", "launcherId": "70"});
    let mut state = ProfileState::new("", false);
    state.apply_snapshot(Some(&snapshot));
    let view = view_of(&state);
    assert_eq!(view.status, "Бегу");
    assert_eq!(view.extras, "they/them · Russia");
    assert_eq!(view.playing, "Playing Half-Life");
    assert_eq!(view.since, "With HLSR since 14.03.2026");
    assert!(view.developer && view.badges_shown);
    // 0.4.9: значок разработчика — картинка со свечением, а не глиф.
    assert_eq!(view.badges[0].asset, "developer");
    assert_eq!(view.badges[0].glow, Some([40, 140, 255]));
    let more: Vec<_> = view.more.iter().map(|item| item.id).collect();
    assert_eq!(more, ["copy", "steam"]);
    // Свой SteamID скрыт настройкой на этом компьютере: «Скопировать» остаётся, Steam — нет.
    state.set_hide_steam_id(true);
    let more: Vec<_> = view_of(&state).more.iter().map(|item| item.id).collect();
    assert_eq!(more, ["copy"]);
    assert_eq!(view_of(&state).dot, "online");
}

#[test]
fn the_more_menu_copies_the_id_and_opens_steam() {
    let mut state = ProfileState::new("", false);
    state.apply_snapshot(Some(&own_snapshot()));
    assert_eq!(
        state.choose_more("copy"),
        vec![
            Effect::Copy("STEAM_0:0:597818474".into()),
            Effect::Notify("#L_SB_COPIED")
        ]
    );
    assert_eq!(
        state.choose_more("steam"),
        vec![invoke(
            "runner-open-link",
            vec![json!(format!("https://steamcommunity.com/profiles/{ME}"))],
            Tag::Ignored
        )]
    );
    assert_eq!(view_of(&state).more[1].label, "Open Steam profile");
}

#[test]
fn subscriber_and_complete_frame_only_a_shown_profile() {
    let mut snapshot = own_snapshot();
    snapshot["subscriber"] = json!(true);
    snapshot["supporterSince"] = json!(1_760_000_000);
    let mut state = ProfileState::new("", false);
    state.apply_snapshot(Some(&snapshot));
    let view = view_of(&state);
    assert!(view.subscriber && !view.complete);
    assert_eq!(view.supporter_since, Some(1_760_000_000));
    snapshot["summary"] = json!({"unlocked": 3, "total": 3, "percent": 100});
    state.apply_snapshot(Some(&snapshot));
    assert!(view_of(&state).complete);
    // Окно без профиля (вход) — без рамок.
    let mut guest = ProfileState::new("", false);
    snapshot["account"] = json!({"signedIn": false});
    guest.apply_snapshot(Some(&snapshot));
    let view = view_of(&guest);
    assert!(!view.subscriber && !view.complete);
}

#[test]
fn badges_0_4_9_bughunter_booster_and_anniversaries() {
    let mut snapshot = own_snapshot();
    snapshot["badges"] = json!(["bughunter", "booster", "anniversary-1", "anniversary-3"]);
    snapshot["boosterSince"] = json!(1_760_000_000);
    let mut state = ProfileState::new("", false);
    state.apply_snapshot(Some(&snapshot));
    let badges = view_of(&state).badges;
    assert_eq!(badges[0].label, "Bug hunter — found a bug in HLSR");
    assert_eq!(
        badges[1].label,
        "Boosting the HLSR Discord server since 09.10.2025"
    );
    assert_eq!(badges[1].glow, Some([255, 79, 216]));
    assert!(badges[2].anniversary && badges[2].glow.is_none());
    assert_eq!(badges[2].label, "1 year in HLSR");
    assert_eq!(badges[3].asset, "anniversary-3");
    assert_eq!(badges[3].glow, Some([46, 185, 255]));
    let ru = state.view(Language::Ru, &wall(), &ru()).badges;
    assert_eq!(ru[3].label, "3 года в HLSR");
}

#[test]
fn a_status_with_a_link_is_not_shown() {
    let mut snapshot = own_snapshot();
    snapshot["details"]["status"] = json!("смотри hlsr.pro сегодня");
    let mut state = ProfileState::new("", false);
    state.apply_snapshot(Some(&snapshot));
    assert_eq!(view_of(&state).status, "");
}

#[test]
fn a_foreign_steam_id_is_hidden_when_its_owner_asked() {
    let mut snapshot = foreign_snapshot();
    snapshot["details"]["hideSteamId"] = json!(true);
    let mut state = ProfileState::new(KATE, true);
    state.load();
    state.answer(Tag::Own, Ok(own_snapshot()));
    state.answer(Tag::Snapshot, Ok(snapshot));
    // Настройка на этом компьютере чужой профиль не трогает: меню «⋯» пустое.
    assert!(view_of(&state).more.is_empty());
    let mut open = foreign_snapshot();
    open["details"]["hideSteamId"] = json!(false);
    state.apply_snapshot(Some(&open));
    let more: Vec<_> = view_of(&state).more.iter().map(|item| item.id).collect();
    assert_eq!(more, ["copy", "steam"]);
    assert_eq!(
        state.choose_more("copy")[0],
        Effect::Copy("STEAM_0:0:19".into())
    );
}

#[test]
fn tabs_depend_on_whose_profile_it_is() {
    let mut own = ProfileState::new("", false);
    own.apply_snapshot(Some(&own_snapshot()));
    let tabs: Vec<_> = view_of(&own)
        .tabs
        .into_iter()
        .map(|t| (t.label, t.count))
        .collect();
    assert_eq!(
        tabs,
        [
            ("Profile".to_owned(), String::new()),
            ("Achievements".to_owned(), "1/3".to_owned()),
            ("Settings".to_owned(), String::new()),
        ]
    );
    let mut foreign = ProfileState::new(KATE, false);
    foreign.apply_snapshot(Some(&foreign_snapshot()));
    assert_eq!(view_of(&foreign).tabs.len(), 2);
}

#[test]
fn the_summary_rows_and_the_most_played_game() {
    let mut snapshot = own_snapshot();
    snapshot["stats"]["launches"] = json!(48);
    snapshot["streak"] = json!({"days": 3, "best": 12});
    let mut state = ProfileState::new("", false);
    state.apply_snapshot(Some(&snapshot));
    let rows: Vec<_> = view_of(&state)
        .stats
        .into_iter()
        .map(|row| (row.label, row.value, row.wide))
        .collect();
    assert_eq!(
        rows,
        [
            (
                "Total time in games".to_owned(),
                "142 h 19 min".to_owned(),
                false
            ),
            ("Game launches".to_owned(), "48".to_owned(), false),
            ("Games launched".to_owned(), "2".to_owned(), false),
            (
                "Achievements unlocked".to_owned(),
                "1 of 3".to_owned(),
                false
            ),
            (
                "Days in a row · best 12".to_owned(),
                "3 days".to_owned(),
                true
            ),
            (
                "Most played · 111 h 27 min".to_owned(),
                "HL2 Ghosting".to_owned(),
                true
            ),
        ]
    );
    let values: Vec<_> = state
        .view(Language::Ru, &wall(), &ru())
        .stats
        .into_iter()
        .map(|row| row.value)
        .collect();
    assert_eq!(values[3], "1 из 3");
    assert_eq!(values[4], "3 дня");
    // Серия входов — только у своего профиля.
    let mut foreign = ProfileState::new(KATE, false);
    let mut view = foreign_snapshot();
    view["streak"] = json!({"days": 5, "best": 5});
    foreign.apply_snapshot(Some(&view));
    assert_eq!(view_of(&foreign).stats.len(), 5);
    assert_eq!(format_hours(59.0, &en()), "0 min");
    assert_eq!(format_hours(3600.0, &en()), "1 h 0 min");
    assert_eq!(format_hours(f64::NAN, &ru()), "0 мин");
}

#[test]
fn achievements_show_progress_only_for_locked_ones_with_a_goal() {
    let mut state = ProfileState::new("", false);
    state.apply_snapshot(Some(&own_snapshot()));
    let view = view_of(&state);
    assert_eq!(view.counter, "Unlocked 1/3");
    let a = &view.achievements;
    assert!(a[0].unlocked && !a[0].meter);
    assert_eq!(a[0].state, "Unlocked");
    assert!(!a[1].unlocked && a[1].meter);
    assert_eq!(a[1].percent, 62);
    assert_eq!(a[1].state, "31/50");
    assert_eq!(
        a[1].text,
        "Play 50 hours in one of the games in the HLSR launcher"
    );
    assert!(!a[2].meter);
    assert_eq!(a[2].state, "Locked");
}

#[test]
fn achievements_show_rarity_of_unlocked_secret_locks_and_own_dates() {
    let mut snapshot = own_snapshot();
    snapshot["achievements"] = json!([
        {"id": "king-of-the-hill", "title": "#L_ACH_KING_TITLE", "description": "#L_ACH_KING_DESC",
         "icon": "fas fa-crown", "tier": "rainbow", "secret": true, "unlocked": true, "progress": 1,
         "value": null, "goal": null, "at": "2026-11-08T02:41:00.000Z"},
        {"id": "cursed", "title": "#L_ACH_CURSED_TITLE", "description": "#L_ACH_GRASS_DESC",
         "icon": "fas fa-skull", "tier": "purple", "unlocked": false, "progress": 0.1, "value": 50,
         "goal": 500, "at": null},
        {"id": "secret-1", "title": "#L_ACH_SECRET_TITLE", "description": "#L_ACH_SECRET_DESC",
         "icon": "fas fa-question", "tier": null, "secret": true, "hidden": true,
         "unlocked": false, "progress": 0, "value": null, "goal": null, "at": null},
    ]);
    let mut own = ProfileState::new("", false);
    own.apply_snapshot(Some(&snapshot));
    let a = view_of(&own).achievements;
    assert_eq!((a[0].tier, a[0].secret), ("rainbow", true));
    assert_eq!(a[0].at, "Nov 8, 2026, 2:41 AM");
    // Редкость — только у полученного; замок секретного — не «secret».
    assert_eq!((a[1].tier, a[1].at.as_str()), ("", ""));
    assert!(!a[2].secret && !a[2].unlocked);
    let ru = own.view(Language::Ru, &wall(), &ru()).achievements;
    assert_eq!(ru[0].at, "8 ноября 2026, 02:41");
    // У чужого даты нет.
    let mut foreign = ProfileState::new(KATE, false);
    foreign.apply_snapshot(Some(&snapshot));
    assert_eq!(view_of(&foreign).achievements[0].at, "");
    assert_eq!(
        format_achieved_at(Language::En, &wall(), "2026-09-08T14:05:00Z"),
        "Sep 8, 2026, 2:05 PM"
    );
    assert_eq!(format_achieved_at(Language::En, &wall(), "nope"), "");
}

#[test]
fn the_pinned_run_shows_a_medal_and_asks_for_the_place_once() {
    let mut snapshot = own_snapshot();
    snapshot["details"]["speedrun"] = json!({"id": "x7y8z9w0", "name": "Kanoyo"});
    snapshot["details"]["pinnedRun"] = json!({"id": "abcd1234", "seconds": 1648.285,
        "game": "Half-Life", "category": "Any%", "place": 2,
        "url": "https://www.speedrun.com/hl1/runs/abcd1234"});
    let mut state = ProfileState::new("", false);
    assert_eq!(
        state.apply_snapshot(Some(&snapshot)),
        [Effect::ServerHideId(Some(false))]
    );
    let run = view_of(&state).run.expect("забег");
    assert_eq!(
        (run.time.as_str(), run.medal, run.place),
        ("27:28.285", "silver", 2)
    );
    assert_eq!(run.place_label, "2nd place");
    // Без места — один запрос списка рекордов за окно.
    snapshot["details"]["pinnedRun"]["place"] = Value::Null;
    let again = state.apply_snapshot(Some(&snapshot));
    assert_eq!(
        again,
        [
            Effect::ServerHideId(Some(false)),
            invoke("runner-speedrun-bests", vec![], Tag::Ignored)
        ]
    );
    assert_eq!(
        state.apply_snapshot(Some(&snapshot)),
        [Effect::ServerHideId(Some(false))]
    );
    // Чужой профиль списка рекордов не просит.
    let mut foreign = ProfileState::new(KATE, false);
    let mut view = foreign_snapshot();
    view["details"]["speedrun"] = json!({"id": "x7y8z9w0", "name": "Kate"});
    view["details"]["pinnedRun"] = snapshot["details"]["pinnedRun"].clone();
    assert!(foreign.apply_snapshot(Some(&view)).is_empty());
}

#[test]
fn english_places_get_ordinal_suffixes() {
    let tr = en();
    let places: Vec<_> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 101, 111]
        .into_iter()
        .map(|place| place_label(place, "en", &tr))
        .collect();
    assert_eq!(
        places,
        [
            "1st place",
            "2nd place",
            "3rd place",
            "4th place",
            "11th place",
            "12th place",
            "13th place",
            "21st place",
            "22nd place",
            "101st place",
            "111th place"
        ]
    );
    assert_eq!(place_label(3, "ru", &ru()), "3 место");
    assert_eq!([1, 2, 3, 4].map(medal_of), ["gold", "silver", "bronze", ""]);
}

#[test]
fn links_show_channel_names_and_fall_back_to_the_service() {
    let mut snapshot = own_snapshot();
    snapshot["details"]["discord"] = json!("kanoyo");
    snapshot["details"]["youtube"] = json!("https://www.youtube.com/@kanoyo");
    snapshot["details"]["twitch"] = json!("https://www.twitch.tv/Kanoyo");
    let mut state = ProfileState::new("", false);
    state.apply_snapshot(Some(&snapshot));
    let links = view_of(&state).links.expect("ссылки");
    assert_eq!(
        (
            links.discord.as_str(),
            links.youtube_label.as_str(),
            links.twitch_label.as_str()
        ),
        ("kanoyo", "@kanoyo", "Kanoyo")
    );
    // Канал по ID: ника в адресе нет — название сервиса.
    snapshot["details"]["youtube"] =
        json!("https://www.youtube.com/channel/UC1234567890123456789012");
    state.apply_snapshot(Some(&snapshot));
    assert_eq!(
        view_of(&state).links.expect("ссылки").youtube_label,
        "YouTube"
    );
    snapshot["details"]["discord"] = json!("");
    snapshot["details"]["youtube"] = json!("");
    snapshot["details"]["twitch"] = json!("");
    state.apply_snapshot(Some(&snapshot));
    assert!(view_of(&state).links.is_none());
}

#[test]
fn custom_badges_use_the_interface_language_and_the_note() {
    let mut snapshot = own_snapshot();
    snapshot["badges"] = json!(["verified", "c-0a1b2c3d", "unknown", "c-4e5f6a7b"]);
    snapshot["customBadges"] = json!([
        {"id": "c-0a1b2c3d", "label": {"ru": "Бета", "en": "Beta"}, "note": {"ru": "Закрытая бета", "en": ""},
         "icon": "data:image/svg+xml;base64,AAAA", "themed": true},
        {"id": "c-4e5f6a7b", "label": {"ru": "Спонсор", "en": "Sponsor"}, "note": {"ru": "", "en": ""},
         "icon": "data:image/svg+xml;base64,BBBB", "themed": false},
    ]);
    let mut state = ProfileState::new("", false);
    state.apply_snapshot(Some(&snapshot));
    let badges = view_of(&state).badges;
    assert_eq!(badges.len(), 3, "неизвестный значок не показывается");
    assert_eq!(badges[0].label, "Steam account ownership verified");
    // Пояснение — на языке интерфейса, а если его нет — русское.
    assert_eq!(badges[1].label, "Beta — Закрытая бета");
    assert!(badges[1].themed && !badges[2].themed);
    assert_eq!(badges[2].label, "Sponsor");
    assert_eq!(badges[2].image, "data:image/svg+xml;base64,BBBB");
    let ru = state.view(Language::Ru, &wall(), &ru()).badges;
    assert_eq!(ru[2].label, "Спонсор");
}

#[test]
fn only_the_own_profile_shows_the_ban() {
    let mut snapshot = own_snapshot();
    snapshot["ban"] = json!({"expiresAt": "2026-11-30T18:30:00.000Z", "reason": "Читерство"});
    let mut own = ProfileState::new("", false);
    own.apply_snapshot(Some(&snapshot));
    let ban = view_of(&own).ban.expect("блокировка");
    assert!(
        ban.title.starts_with("Your account is banned until "),
        "{}",
        ban.title
    );
    assert_eq!(ban.reason, "Reason: Читерство");
    snapshot["ban"] = json!({"expiresAt": null, "reason": ""});
    own.apply_snapshot(Some(&snapshot));
    let ban = view_of(&own).ban.expect("блокировка");
    assert_eq!(ban.title, "Your account is permanently banned.");
    assert_eq!(ban.reason, "");
    let mut foreign = ProfileState::new(KATE, false);
    let mut view = foreign_snapshot();
    view["ban"] = json!({"expiresAt": null, "reason": "x"});
    foreign.apply_snapshot(Some(&view));
    assert!(view_of(&foreign).ban.is_none());
}

#[test]
fn only_a_signed_in_viewer_of_a_foreign_profile_can_vote() {
    let mut own = ProfileState::new("", false);
    own.apply_snapshot(Some(&own_snapshot()));
    assert!(!own.can_vote());
    assert!(own.cast_vote(1).is_empty());
    assert!(!view_of(&own).rating.expect("рейтинг").can_vote);

    let mut state = ProfileState::new(KATE, false);
    state.apply_snapshot(Some(&foreign_snapshot()));
    assert!(state.can_vote());
    let rating = view_of(&state).rating.expect("рейтинг");
    assert_eq!(
        (rating.score.as_str(), rating.tone, rating.vote),
        ("+14", "positive", 0)
    );
    assert_eq!(rating.hint, "Rating: 15 likes, 1 dislikes");
    assert_eq!(rating.like_label, "Like");
    assert_eq!(rating.dislike_title, "Dislike");
}

#[test]
fn a_vote_is_sent_and_a_second_press_on_the_same_button_removes_it() {
    let mut state = ProfileState::new(KATE, false);
    state.apply_snapshot(Some(&foreign_snapshot()));
    let sent = state.cast_vote(1);
    assert_eq!(
        sent,
        [
            Effect::VoteNoteTimer(false),
            invoke("runner-rate", vec![json!(KATE), json!(1)], Tag::Rate)
        ]
    );
    // Пока ответа нет, второй голос не уходит.
    assert!(state.cast_vote(-1).is_empty());
    let ok = json!({"status": "ok", "rating": {"score": 15, "likes": 16, "dislikes": 1, "dubious": false},
                    "vote": {"value": 1, "nextAt": "2026-10-16T16:00:00.000Z"}});
    assert!(state.answer(Tag::Rate, Ok(ok)).is_empty());
    let rating = view_of(&state).rating.expect("рейтинг");
    assert_eq!(
        (rating.score.as_str(), rating.vote, rating.voting),
        ("+15", 1, false)
    );
    assert_eq!(rating.like_label, "Remove like");
    // Свой лайк менять можно раз в четыре часа: подсказка про время вместо названия действия.
    assert!(
        rating
            .like_title
            .starts_with("You can change your vote after "),
        "{}",
        rating.like_title
    );
    assert_eq!(
        state.cast_vote(1),
        [
            Effect::VoteNoteTimer(false),
            invoke("runner-rate", vec![json!(KATE), json!(0)], Tag::Rate)
        ]
    );
}

#[test]
fn a_cooldown_and_the_like_limit_show_a_note_that_fades() {
    let mut state = ProfileState::new(KATE, false);
    state.apply_snapshot(Some(&foreign_snapshot()));
    state.cast_vote(-1);
    let cooldown = json!({"status": "cooldown", "rating": {"score": 3, "likes": 5, "dislikes": 2, "dubious": false},
                          "vote": {"value": 1, "nextAt": "2026-10-16T14:30:00.000Z"}});
    assert_eq!(
        state.answer(Tag::Rate, Ok(cooldown)),
        [Effect::VoteNoteTimer(true)]
    );
    assert_eq!(
        state.vote_note_text(Language::En, &wall(), &en()),
        "You can change your vote after 02:30 PM"
    );
    state.vote_note_done();
    assert!(!state.vote_note_shown());
    state.cast_vote(1);
    let limit = json!({"status": "like-limit", "likesAt": "2026-10-16T15:45:00.000Z",
                       "rating": {"score": 3, "likes": 5, "dislikes": 2, "dubious": false},
                       "vote": {"value": 0, "nextAt": null}});
    state.answer(Tag::Rate, Ok(limit));
    assert_eq!(
        state.vote_note_text(Language::En, &wall(), &en()),
        "You can like up to 5 players every 4 hours. Next like after 03:45 PM"
    );
    state.cast_vote(1);
    state.answer(Tag::Rate, Ok(json!({"status": "error"})));
    assert_eq!(
        state.vote_note_text(Language::En, &wall(), &en()),
        "Couldn't vote. Check your connection."
    );
    state.cast_vote(1);
    state.answer(Tag::Rate, Err("rejected".into()));
    assert!(state.vote_note_shown());
    // Ответы со счётом без перехода в `cooldown`/`like-limit` подсказки не дают.
    state.vote_note_done();
    state.cast_vote(1);
    let ok = json!({"status": "ok", "rating": {"score": 4}, "vote": {"value": 1, "nextAt": null}});
    assert!(state.answer(Tag::Rate, Ok(ok)).is_empty());
}

#[test]
fn a_negative_or_dubious_rating_is_marked() {
    let mut view = foreign_snapshot();
    view["rating"] = json!({"score": -8, "likes": 1, "dislikes": 9, "dubious": true});
    let mut state = ProfileState::new(KATE, false);
    state.apply_snapshot(Some(&view));
    let shown = view_of(&state);
    let rating = shown.rating.expect("рейтинг");
    assert_eq!((rating.score.as_str(), rating.tone), ("-8", "negative"));
    assert!(shown.dubious);
}

#[test]
fn copying_the_id_goes_to_the_clipboard_and_notifies() {
    let mut state = ProfileState::new("", false);
    state.apply_snapshot(Some(&own_snapshot()));
    assert_eq!(
        state.copy_id(),
        [
            Effect::Copy("STEAM_0:0:597818474".into()),
            Effect::Notify("#L_SB_COPIED")
        ]
    );
    assert!(state.copy("").is_empty());
    assert_eq!(
        state.open_steam(),
        [invoke(
            "runner-open-link",
            vec![json!(format!("https://steamcommunity.com/profiles/{ME}"))],
            Tag::Ignored
        )]
    );
    assert_eq!(
        state.open_external("privacy"),
        [invoke(
            "open-external-link",
            vec![json!("privacy")],
            Tag::Ignored
        )]
    );
}

#[test]
fn the_login_confirmation_is_a_timer_driven_toast() {
    let mut state = ProfileState::new("", false);
    assert_eq!(state.login_success(), [Effect::LoginSuccessTimer(true)]);
    assert!(state.login_success_shown());
    state.login_success_done();
    assert!(!state.login_success_shown());
}

#[test]
fn the_server_hide_id_flag_is_reported_only_for_the_own_signed_in_profile() {
    let mut snapshot = own_snapshot();
    snapshot["details"]["hideSteamId"] = json!(true);
    let mut own = ProfileState::new("", false);
    assert_eq!(
        own.apply_snapshot(Some(&snapshot)),
        [Effect::ServerHideId(Some(true))]
    );
    // У старых профилей флага нет — оболочка получает «не булево».
    snapshot["details"]
        .as_object_mut()
        .unwrap()
        .remove("hideSteamId");
    assert_eq!(
        own.apply_snapshot(Some(&snapshot)),
        [Effect::ServerHideId(None)]
    );
    let guest = json!({"status": "signed-out", "account": {"signedIn": false}});
    assert!(own.apply_snapshot(Some(&guest)).is_empty());
}

#[test]
fn dates_follow_the_local_zone_and_bad_dates_are_empty() {
    let wall = WallClock::fixed(1_792_152_000_000, 3 * 3600);
    assert_eq!(format_date(&wall, "2026-03-14T22:30:00.000Z"), "15.03.2026");
    assert_eq!(format_date(&wall, "вчера"), "");
    assert_eq!(format_vote_time(Language::En, &wall, "nope"), "");
    assert_eq!(
        format_vote_time(Language::Ru, &wall, "2026-10-16T14:30:00.000Z"),
        "17:30"
    );
}
