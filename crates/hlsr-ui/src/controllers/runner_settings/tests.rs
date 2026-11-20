use super::*;
use serde_json::json;
use std::rc::Rc;

fn en() -> Tr {
    Rc::new(|key, args| crate::tr(Language::En, key, args))
}

/// 2026-10-16 12:00 UTC.
const NOW: i64 = 1_792_152_000_000;

fn wall() -> WallClock {
    WallClock::fixed(NOW, 0)
}

fn account() -> Value {
    json!({"signedIn": true, "steamId64": "76561199155902676", "steamId2": "STEAM_0:0:597818474",
           "personaName": "Kanoyo"})
}

fn details() -> Value {
    json!({"status": "Бегу", "pronouns": "she", "country": "RU", "discord": "kanoyo",
           "youtube": "", "twitch": "", "hideFromPlayers": false, "hideGame": true,
           "speedrun": null, "pinnedRun": null})
}

fn state() -> SettingsState {
    let mut state = SettingsState::new();
    state.set_props(&account(), &details(), None, None);
    state
}

fn view(state: &SettingsState) -> SettingsView {
    state.view(Language::En, &wall(), &en())
}

fn request(channel: &'static str, tag: Tag, args: Vec<Value>) -> Effect {
    Effect::Request { tag, channel, args }
}

/// Порт `launcher-ui-selftest.js:306` («profile links accept a channel name or link and keep only
/// the right service») через форму настроек: `tidyLink` и `linkInvalid` на `shared/profile-links`.
#[test]
fn profile_links_accept_a_channel_name_or_link_and_keep_only_the_right_service() {
    let normalized = |id: &str, input: &str| {
        let mut state = state();
        state.set_field(id, input);
        if state.link_invalid(id) {
            return None;
        }
        state.tidy_link(id);
        Some(state.field(id).to_owned())
    };
    let yt = |input: &str| normalized("youtube", input);
    let tw = |input: &str| normalized("twitch", input);
    assert_eq!(
        yt("@kanoyo").as_deref(),
        Some("https://www.youtube.com/@kanoyo")
    );
    assert_eq!(
        yt("kanoyo").as_deref(),
        Some("https://www.youtube.com/@kanoyo")
    );
    assert_eq!(
        yt("youtube.com/@kanoyo/videos").as_deref(),
        Some("https://www.youtube.com/@kanoyo"),
        "https:// не обязателен"
    );
    assert_eq!(
        yt("http://m.youtube.com/@kanoyo").as_deref(),
        Some("https://www.youtube.com/@kanoyo")
    );
    assert_eq!(
        yt("youtube.com/channel/UC1234567890123456789012").as_deref(),
        Some("https://www.youtube.com/channel/UC1234567890123456789012")
    );
    assert_eq!(
        yt("https://www.twitch.tv/kanoyo"),
        None,
        "ссылка не на YouTube"
    );
    assert_eq!(yt("youtube.com/watch?v=abc"), None, "видео — не канал");
    assert_eq!(yt("https://youtube.com@evil.example/@x"), None);
    assert_eq!(yt("").as_deref(), Some(""), "пустое поле — без ссылки");
    assert_eq!(
        tw("Kanoyo").as_deref(),
        Some("https://www.twitch.tv/Kanoyo")
    );
    assert_eq!(
        tw("twitch.tv/kanoyo/videos").as_deref(),
        Some("https://www.twitch.tv/kanoyo")
    );
    assert_eq!(
        tw("twitch.tv/directory"),
        None,
        "служебная страница — не канал"
    );
    assert_eq!(tw("kick.com/kanoyo"), None);
    // Ник канала из адреса (`channelName`) — в окне профиля.
    assert_eq!(
        profile_links::channel_name("youtube", &json!("https://www.youtube.com/@kanoyo"))
            .as_deref(),
        Some("@kanoyo")
    );
    assert_eq!(
        profile_links::channel_name(
            "youtube",
            &json!("https://www.youtube.com/channel/UC1234567890123456789012")
        ),
        None
    );
    assert_eq!(
        profile_links::channel_name("twitch", &json!("https://www.twitch.tv/Kanoyo")).as_deref(),
        Some("Kanoyo")
    );
}

#[test]
fn the_form_starts_from_the_details_and_resets_only_while_untouched() {
    let mut state = state();
    let shown = view(&state);
    assert_eq!(
        (
            shown.status.as_str(),
            shown.pronouns.as_str(),
            shown.country.as_str()
        ),
        ("Бегу", "she", "RU")
    );
    assert_eq!(shown.pronouns_label, "she/her");
    assert_eq!(shown.country_label, "Russia");
    assert!(!shown.dirty && !shown.can_save);
    // Новые details, пока форму не правили, обновляют её.
    let mut updated = details();
    updated["status"] = json!("Новый");
    state.set_props(&account(), &updated, None, None);
    assert_eq!(view(&state).status, "Новый");
    // Правка делает форму «грязной»: сервер её не перезаписывает.
    state.set_field("status", "Мой");
    assert!(view(&state).dirty);
    updated["status"] = json!("С сервера");
    state.set_props(&account(), &updated, None, None);
    assert_eq!(view(&state).status, "Мой");
    state.revert();
    assert_eq!(view(&state).status, "С сервера");
    assert!(!view(&state).dirty);
}

#[test]
fn the_status_counts_characters_and_refuses_links() {
    let mut state = state();
    state.set_field("status", &"я".repeat(80));
    let shown = view(&state);
    assert_eq!(
        shown.status_hint,
        "Shown on your profile and to friends. 80 / 80"
    );
    assert!(!shown.status_error && shown.can_save);
    state.set_field("status", &"я".repeat(81));
    assert!(view(&state).status_error);
    assert!(!view(&state).can_save);
    // Эмодзи — один символ (Array.from), а не две единицы UTF-16.
    state.set_field("status", &"🏃".repeat(80));
    assert!(view(&state).can_save);
    state.set_field("status", "смотри hlsr.pro");
    let shown = view(&state);
    assert_eq!(
        shown.status_hint,
        "Links are not allowed in the description, disguised ones included"
    );
    assert!(shown.status_error && !shown.can_save);
}

#[test]
fn a_muted_status_is_disabled_with_the_reason() {
    let mut state = state();
    let mute = json!({"expiresAt": "2026-10-30T12:00:00.000Z", "reason": "Спам"});
    state.set_props(&account(), &details(), None, Some(&mute));
    let shown = view(&state);
    assert!(shown.status_muted && shown.status_error);
    assert!(
        shown.status_hint.starts_with("Status is disabled until "),
        "{}",
        shown.status_hint
    );
    assert!(shown.status_hint.ends_with(" Reason: Спам"));
    let forever = json!({"expiresAt": null, "reason": ""});
    state.set_props(&account(), &details(), None, Some(&forever));
    assert_eq!(view(&state).status_hint, "Status is disabled.");
}

#[test]
fn an_invalid_link_blocks_saving_and_changes_the_hint() {
    let mut state = state();
    state.set_field("twitch", "kick.com/kanoyo");
    let shown = view(&state);
    let twitch = shown.links.iter().find(|row| row.id == "twitch").unwrap();
    assert!(twitch.invalid);
    assert_eq!(twitch.hint, "This is not a Twitch channel");
    assert!(!shown.can_save);
    assert!(
        state.save().is_empty(),
        "кнопка недоступна — форма не уходит"
    );
    let discord = shown.links.iter().find(|row| row.id == "discord").unwrap();
    assert_eq!(discord.hint, "", "у Discord подсказки нет");
}

#[test]
fn saving_sends_the_whole_form_with_full_channel_addresses() {
    let mut state = state();
    state.set_field("youtube", "@kanoyo");
    state.set_field("status", "Привет");
    let effects = state.save();
    assert_eq!(
        effects,
        [request(
            "runner-customize",
            Tag::Save,
            vec![
                json!({"status": "Привет", "pronouns": "she", "country": "RU", "discord": "kanoyo",
                        "youtube": "https://www.youtube.com/@kanoyo", "twitch": ""})
            ]
        )]
    );
    assert!(view(&state).busy);
    let snapshot = json!({"details": {"status": "Привет"}});
    let after = state.answer(
        Tag::Save,
        Ok(json!({"status": "ok", "snapshot": snapshot})),
        NOW,
    );
    // Снимок уходит окну раньше, чем форма помечается сохранённой.
    assert_eq!(after, [Effect::Updated(snapshot)]);
    let shown = view(&state);
    assert!(!shown.busy && !shown.dirty);
    assert_eq!(
        (shown.message.as_str(), shown.message_ok),
        ("Profile saved", true)
    );
    // Следующая правка снимает сообщение.
    state.set_field("status", "Ещё");
    assert_eq!(view(&state).message, "");
}

#[test]
fn a_refused_save_shows_the_form_error_and_a_rejected_one_the_network_error() {
    let mut state = state();
    state.set_field("status", "x");
    state.save();
    state.answer(Tag::Save, Ok(json!({"status": "error"})), NOW);
    let shown = view(&state);
    assert_eq!(
        (shown.message.as_str(), shown.message_ok, shown.dirty),
        (
            "Check the text lengths and links. If they are correct, try again later.",
            false,
            true
        )
    );
    state.save();
    state.answer(Tag::Save, Err("отказ".into()), NOW);
    assert_eq!(
        view(&state).message,
        "Could not connect to HLSR. Check your connection and try again."
    );
}

#[test]
fn a_privacy_switch_shows_its_value_at_once_and_is_locked_until_the_answer() {
    let mut state = state();
    let before = view(&state);
    assert_eq!(
        before
            .privacy
            .iter()
            .map(|row| (row.flag, row.on))
            .collect::<Vec<_>>(),
        [("hideFromPlayers", true), ("hideGame", false)]
    );
    let sent = state.set_privacy("hideFromPlayers", true);
    assert_eq!(
        sent,
        [request(
            "runner-customize",
            Tag::Privacy("hideFromPlayers".into()),
            vec![json!({"hideFromPlayers": true})]
        )]
    );
    let shown = view(&state);
    assert!(!shown.privacy[0].on && shown.privacy[0].disabled);
    // Отказ: переключатель возвращается к значению сервера, под ним — ошибка.
    state.answer(
        Tag::Privacy("hideFromPlayers".into()),
        Ok(json!({"status": "error"})),
        NOW,
    );
    let shown = view(&state);
    assert!(shown.privacy[0].on && !shown.privacy[0].disabled && shown.privacy[0].error);
    assert_eq!(
        shown.privacy[0].hint,
        "Check the text lengths and links. If they are correct, try again later."
    );
    // Следующее нажатие снимает ошибку.
    state.set_privacy("hideGame", false);
    assert!(!view(&state).privacy[0].error);
}

#[test]
fn the_steam_id_is_hidden_until_revealed_and_hides_again() {
    let mut state = state();
    let shown = view(&state);
    assert_eq!(
        (shown.steam_id.as_str(), shown.id_length, shown.id_revealed),
        ("76561199155902676", 17, false)
    );
    state.toggle_id_revealed();
    assert!(view(&state).id_revealed);
    state.set_active(false);
    assert!(!view(&state).id_revealed, "уход с вкладки прячет номер");
    state.toggle_id_revealed();
    state.hide_id();
    assert!(!view(&state).id_revealed, "окно потеряло фокус");
    // Без аккаунта Steam строки нет, а длина — как у SteamID64.
    state.set_props(&json!({}), &details(), None, None);
    assert_eq!(view(&state).steam_id, "");
    assert_eq!(view(&state).id_length, 17);
    state.set_hide_steam_id(true);
    assert!(!view(&state).show_id);
}

#[test]
fn speedrun_link_sends_the_trimmed_key_once_and_clears_the_field() {
    let mut state = state();
    state.set_speedrun_key("   ");
    assert!(state.link_speedrun().is_empty());
    state.set_speedrun_key("  secret-key ");
    let effects = state.link_speedrun();
    assert_eq!(
        effects,
        [request(
            "runner-speedrun-link",
            Tag::LinkSpeedrun,
            vec![json!("secret-key")]
        )]
    );
    // Пока запрос идёт, второй не уходит.
    state.set_speedrun_key("second");
    assert!(state.link_speedrun().is_empty());
    state.answer(Tag::LinkSpeedrun, Ok(json!({"status": "error"})), NOW);
    assert_eq!(
        view(&state).speedrun_message,
        "Speedrun.com rejected the key. Copy it again from the settings page."
    );
    state.set_speedrun_key("again");
    state.link_speedrun();
    state.answer(Tag::LinkSpeedrun, Err("net".into()), NOW);
    assert_eq!(
        view(&state).speedrun_message,
        "Could not connect to HLSR. Check your connection and try again."
    );
}

#[test]
fn runs_are_listed_pinned_and_unlinked() {
    let mut state = SettingsState::new();
    let mut linked = details();
    linked["speedrun"] = json!({"id": "x7y8z9w0", "name": "Kanoyo"});
    state.set_props(&account(), &linked, None, None);
    let shown = view(&state);
    assert!(shown.speedrun_linked);
    assert_eq!(shown.speedrun_name, "Kanoyo");
    assert_eq!(
        shown.speedrun_hint,
        "Account verified. Choose a record for your profile."
    );
    assert_eq!(
        state.load_runs(),
        [request(
            "runner-speedrun-bests",
            Tag::LoadRuns,
            vec![Value::Null]
        )]
    );
    state.answer(
        Tag::LoadRuns,
        Ok(json!({"status": "ok", "runs": [
            {"id": "aaaaaaaa", "game": "Half-Life", "category": "Any%", "place": 1, "seconds": 1648.285},
            {"id": "bbbbbbbb", "game": "Half-Life 2", "category": "Old Engine", "place": 21, "seconds": 5400}
        ]})),
        NOW,
    );
    let shown = view(&state);
    assert!(shown.runs_loaded);
    assert_eq!(shown.runs[0].place_label, "1st place");
    assert_eq!(shown.runs[0].medal, "gold");
    assert_eq!(shown.runs[0].time, "27:28.285");
    assert_eq!(shown.runs[1].place_label, "21st place");
    assert_eq!(shown.runs[1].time, "1:30:00.000");
    // Закреплённый забег помечается; подсказка — игра · категория · время.
    linked["pinnedRun"] = json!({"id": "aaaaaaaa", "game": "Half-Life", "category": "Any%", "place": 1, "seconds": 1648.285});
    state.set_props(&account(), &linked, None, None);
    let shown = view(&state);
    assert!(shown.runs[0].pinned && !shown.runs[1].pinned);
    assert!(shown.pinned);
    assert_eq!(
        shown.speedrun_hint,
        "On profile: Half-Life · Any% · 27:28.285"
    );
    assert_eq!(
        state.pin_run("bbbbbbbb"),
        [request(
            "runner-speedrun-pin",
            Tag::PinRun,
            vec![json!("bbbbbbbb")]
        )]
    );
    state.answer(Tag::PinRun, Ok(json!({"status": "ok"})), NOW);
    assert_eq!(
        state.unpin_run(),
        [request(
            "runner-speedrun-unpin",
            Tag::UnpinRun,
            vec![Value::Null]
        )]
    );
    // Отвязка стирает список.
    state.answer(Tag::UnpinRun, Ok(json!({"status": "ok"})), NOW);
    state.unlink_speedrun();
    state.answer(Tag::UnlinkSpeedrun, Ok(json!({"status": "ok"})), NOW);
    let shown = view(&state);
    assert!(shown.runs.is_empty() && !shown.runs_loaded);
}

#[test]
fn logout_reports_the_result_as_a_snapshot() {
    let mut state = state();
    assert_eq!(
        state.logout(),
        [request("runner-logout", Tag::Logout, vec![Value::Null])]
    );
    let result = json!({"status": "signed-out", "account": {"signedIn": false}});
    assert_eq!(
        state.answer(Tag::Logout, Ok(result.clone()), NOW),
        [Effect::Updated(result)]
    );
    state.logout();
    state.answer(Tag::Logout, Ok(json!({"status": "error"})), NOW);
    assert_eq!(
        view(&state).account_message,
        "Check the text lengths and links. If they are correct, try again later."
    );
}

#[test]
fn deleting_the_account_counts_down_on_the_servers_ready_time() {
    let mut state = state();
    assert_eq!(
        state.prepare_delete(),
        [request(
            "runner-delete-challenge",
            Tag::PrepareDelete,
            vec![Value::Null]
        )]
    );
    let ready = chrono_iso(NOW + 19_300);
    let effects = state.answer(
        Tag::PrepareDelete,
        Ok(json!({"status": "ok", "challenge": "abc", "readyAt": ready})),
        NOW,
    );
    assert_eq!(effects, [Effect::Countdown(true)]);
    let shown = view(&state);
    assert!(shown.confirming && !shown.agreed);
    // Секунды округляются вверх: 19,3 с до готовности — 20.
    assert_eq!(shown.countdown, 20);
    assert!(!state.tick(NOW + 5_000));
    assert_eq!(view(&state).countdown, 15);
    assert!(!state.tick(NOW + 19_000));
    assert_eq!(view(&state).countdown, 1);
    // Отсчёт кончился — таймер снимается тем же тиком.
    assert!(state.tick(NOW + 20_000));
    assert_eq!(view(&state).countdown, 0);
    assert!(
        !state.tick(NOW + 21_000),
        "после остановки тик таймер не трогает"
    );
    state.set_delete_agreed(true);
    assert_eq!(
        state.confirm_delete(),
        [request(
            "runner-delete-request",
            Tag::ConfirmDelete,
            vec![json!({"challenge": "abc", "confirmed": true})]
        )]
    );
    assert_eq!(
        state.answer(Tag::ConfirmDelete, Ok(json!({"status": "ok"})), NOW),
        []
    );
    assert!(!view(&state).confirming);
}

#[test]
fn closing_the_delete_panel_stops_the_countdown() {
    let mut state = state();
    state.prepare_delete();
    state.answer(
        Tag::PrepareDelete,
        Ok(json!({"challenge": "c", "readyAt": chrono_iso(NOW + 10_000)})),
        NOW,
    );
    assert_eq!(state.close_delete(), [Effect::Countdown(false)]);
    assert!(!view(&state).confirming);
    assert!(state.close_delete().is_empty());
    // Окно ушло: ключ стёрт, таймер остановлен.
    state.set_speedrun_key("k");
    state.prepare_delete();
    state.answer(
        Tag::PrepareDelete,
        Ok(json!({"challenge": "c", "readyAt": chrono_iso(NOW + 10_000)})),
        NOW,
    );
    assert_eq!(state.unmounted(), [Effect::Countdown(false)]);
}

#[test]
fn a_scheduled_deletion_offers_to_cancel_it() {
    let mut state = SettingsState::new();
    state.set_props(
        &account(),
        &details(),
        Some("2026-10-23T12:00:00.000Z"),
        None,
    );
    let shown = view(&state);
    assert!(shown.deletion_scheduled);
    assert!(
        shown
            .deletion_hint
            .starts_with("Your data will be deleted on 10/23/2026"),
        "{}",
        shown.deletion_hint
    );
    assert_eq!(
        state.cancel_delete(),
        [request(
            "runner-delete-cancel",
            Tag::CancelDelete,
            vec![Value::Null]
        )]
    );
    state.answer(Tag::CancelDelete, Ok(json!({"status": "ok"})), NOW);
    assert_eq!(view(&state).account_message, "");
}

#[test]
fn countries_and_pronouns_come_with_a_not_set_item_first() {
    let shown = view(&state());
    assert_eq!(
        shown.pronouns_items[0],
        (String::new(), "Not set".to_owned())
    );
    assert_eq!(shown.pronouns_items.len(), 8);
    assert_eq!(
        shown.country_items[0],
        (String::new(), "Not set".to_owned())
    );
    assert!(shown.country_items.len() > 200);
    let mut state = state();
    state.set_pronouns("bogus");
    assert_eq!(view(&state).pronouns, "");
    assert_eq!(view(&state).pronouns_label, "Not set");
    state.set_country("DE");
    assert_eq!(view(&state).country_label, "Germany");
    state.set_country("ZZ");
    assert_eq!(view(&state).country_label, "ZZ");
    state.set_country("");
    assert_eq!(view(&state).country_label, "Not set");
}

/// ISO-8601 момента `ms` (UTC).
fn chrono_iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .expect("момент")
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ---- 0.4.9: выход везде, speedrun.com занят, заготовки, Telegram, Discord -----------------

fn ru() -> Tr {
    Rc::new(|key, args| crate::tr(Language::Ru, key, args))
}

fn support(state: &mut SettingsState, perks: Value, telegram: Value, discord: Value) {
    state.set_support(&perks, false, None, &telegram, &discord);
}

fn no_link() -> Value {
    json!({"linked": false, "username": null, "boosterSince": null})
}

/// Код, живущий 10 минут от `NOW`.
fn code_reply() -> Value {
    json!({"status": "ok", "code": "123456", "expiresAt": NOW / 1000 + 600})
}

const NETWORK: &str = "Could not connect to HLSR. Check your connection and try again.";
const FORM_ERROR: &str = "Check the text lengths and links. If they are correct, try again later.";

#[test]
fn logout_everywhere_sends_its_own_channel_and_reports_the_snapshot() {
    let mut state = state();
    assert_eq!(
        state.logout_all(),
        [request(
            "runner-logout-all",
            Tag::LogoutAll,
            vec![Value::Null]
        )]
    );
    assert!(view(&state).busy);
    let result = json!({"status": "signed-out", "account": {"signedIn": false}});
    assert_eq!(
        state.answer(Tag::LogoutAll, Ok(result.clone()), NOW),
        [Effect::Updated(result)]
    );
    assert!(!view(&state).busy);
}

#[test]
fn a_speedrun_account_taken_by_another_profile_has_its_own_text() {
    let mut state = state();
    state.set_speedrun_key("key");
    state.link_speedrun();
    let taken = json!({"status": "error", "code": "RUNNER_SPEEDRUN_TAKEN", "snapshot": {"a": 1}});
    assert_eq!(state.answer(Tag::LinkSpeedrun, Ok(taken), NOW), []);
    assert!(
        view(&state)
            .speedrun_message
            .starts_with("This speedrun.com account")
    );
}

#[test]
fn the_highlight_group_shows_with_presets_a_perk_a_subscription_or_telegram() {
    let mut state = state();
    assert!(!view(&state).style_group, "сервер на PHP, поддержки нет");
    support(&mut state, json!({"styles": true}), no_link(), no_link());
    assert!(view(&state).style_group);
    support(
        &mut state,
        json!({"subscriber": {"active": false}}),
        no_link(),
        no_link(),
    );
    assert!(view(&state).style_group);
    support(&mut state, json!({}), json!({"linked": true}), no_link());
    assert!(view(&state).style_group);
    support(&mut state, json!({}), no_link(), no_link());
    assert!(!view(&state).style_group);
    assert_eq!(
        state.load_presets(false),
        [request("public-style-presets-get", Tag::Presets, vec![])]
    );
    state.answer(
        Tag::Presets,
        Ok(json!({"status": "ok", "presets": [{"id": 3}]})),
        NOW,
    );
    assert!(view(&state).style_group);
    // Сбой: список остаётся прежним.
    state.answer(Tag::Presets, Err("сбой".into()), NOW);
    assert!(view(&state).style_group);
    assert_eq!(
        state.load_presets(true),
        [request(
            "public-style-presets-get",
            Tag::Presets,
            vec![json!({"force": true})]
        )]
    );
}

fn presets() -> Value {
    json!({"status": "ok", "presets": [
        {"id": 3, "title": "Неон", "nick": {"style": null, "css": "color: red"},
         "frame": {"css": "border: 1px solid red"}},
        {"id": 5, "title": "Золото", "nick": {"style": null, "css": ""}, "frame": {"css": ""}}
    ]})
}

#[test]
fn the_picker_saves_a_preset_once_and_reloads_the_list_when_it_is_gone() {
    let mut state = state();
    support(&mut state, json!({"styles": true}), no_link(), no_link());
    state.answer(Tag::Presets, Ok(presets()), NOW);
    let shown = state.picker_view(&wall(), &en());
    assert_eq!(shown.mode, style_presets::Mode::Grid);
    assert_eq!(
        shown
            .cards
            .iter()
            .map(|c| (c.id, c.selected))
            .collect::<Vec<_>>(),
        [(0.0, true), (3.0, false), (5.0, false)]
    );
    assert_eq!(shown.cards[0].title, "No highlight");
    assert_eq!(shown.cards[1].frame_css, "border: 1px solid red");
    assert_eq!(shown.name, "Kanoyo");
    assert_eq!(state.choose_style(0.0), [], "уже выбрано");
    assert_eq!(
        state.choose_style(3.0),
        [request(
            "runner-customize",
            Tag::Style,
            vec![json!({"stylePreset": 3})]
        )]
    );
    assert_eq!(state.choose_style(5.0), [], "первый ещё сохраняется");
    assert!(state.picker_view(&wall(), &en()).cards[1].pending);
    let snapshot = json!({"account": {"signedIn": true}, "stylePreset": 3});
    assert_eq!(
        state.answer(
            Tag::Style,
            Ok(json!({"status": "ok", "snapshot": snapshot.clone()})),
            NOW
        ),
        [Effect::Updated(snapshot)]
    );
    state.set_support(
        &json!({"styles": true}),
        false,
        Some(&json!(3)),
        &no_link(),
        &no_link(),
    );
    let shown = state.picker_view(&wall(), &en());
    assert_eq!(
        (shown.message.as_str(), shown.message_error),
        ("Highlight saved", false)
    );
    assert!(shown.cards[1].selected && !shown.busy);
    state.choose_style(5.0);
    assert_eq!(
        state.answer(
            Tag::Style,
            Ok(json!({"status": "error", "code": "RUNNER_PRESET_UNKNOWN"})),
            NOW
        ),
        [request(
            "public-style-presets-get",
            Tag::Presets,
            vec![json!({"force": true})]
        )]
    );
    let shown = state.picker_view(&wall(), &en());
    assert_eq!(
        (shown.message.as_str(), shown.message_error),
        (
            "This highlight is no longer available — pick another one",
            true
        )
    );
    state.choose_style(5.0);
    state.answer(Tag::Style, Err("сеть".into()), NOW);
    assert_eq!(state.picker_view(&wall(), &en()).message, NETWORK);
}

#[test]
fn without_the_perk_only_removing_and_supporting_are_offered() {
    let mut state = state();
    let perks = json!({"styles": false,
                       "subscriber": {"active": true, "until": "2026-12-01T10:00:00.000Z"}});
    state.set_support(&perks, true, Some(&json!(3)), &no_link(), &no_link());
    state.answer(Tag::Presets, Ok(presets()), NOW);
    let shown = state.picker_view(&wall(), &en());
    assert_eq!(shown.mode, style_presets::Mode::Locked);
    assert!(shown.visible && shown.removable && shown.subscriber_active && shown.subscriber_badge);
    assert_eq!(
        shown.subscriber_text,
        "Subscription active until 01.12.2026"
    );
    assert_eq!(state.choose_style(5.0), [], "без права выбрать нельзя");
    assert_eq!(
        state.choose_style(0.0),
        [request(
            "runner-customize",
            Tag::Style,
            vec![json!({"stylePreset": 0})]
        )]
    );
    // Заготовок нет, подписка кончилась: блок только со строкой подписки.
    let mut state = SettingsState::new();
    state.set_support(
        &json!({"subscriber": {"active": false}}),
        false,
        None,
        &no_link(),
        &no_link(),
    );
    let shown = state.picker_view(&wall(), &en());
    assert!(shown.visible && shown.mode == style_presets::Mode::None);
    assert_eq!(shown.subscriber_text, "Subscription ended");
    assert_eq!(shown.name, "Steam account");
    state.set_support(
        &json!({"styles": true}),
        false,
        None,
        &no_link(),
        &no_link(),
    );
    assert!(!state.picker_view(&wall(), &en()).visible);
}

#[test]
fn telegram_takes_a_code_opens_the_bot_and_reuses_a_live_code() {
    let mut state = state();
    assert_eq!(
        state.link_telegram(NOW),
        [request("runner-telegram-code", Tag::TelegramCode, vec![])]
    );
    assert!(view(&state).telegram.busy);
    let open = request(
        "support-open",
        Tag::TelegramOpen,
        vec![json!("bot"), json!("123456")],
    );
    assert_eq!(
        state.answer(Tag::TelegramCode, Ok(code_reply()), NOW),
        [
            Effect::CodeTimer(Code::Telegram, Some(600_000)),
            open.clone()
        ]
    );
    let shown = view(&state).telegram;
    assert!(!shown.busy && !shown.linked);
    assert_eq!(shown.label, "Telegram: not linked");
    assert_eq!(shown.hints.len(), 2);
    assert!(shown.hints[0].0.ends_with("send it the code 123456."));
    assert_eq!(shown.hints[1], ("Code valid until 12:10".to_owned(), false));
    assert_eq!(
        state.answer(Tag::TelegramOpen, Ok(json!({"status": "ok"})), NOW),
        []
    );
    // Код жив: сначала синхронизация — игрок мог уже привязаться.
    assert_eq!(
        state.link_telegram(NOW + 1000),
        [request("runner-sync", Tag::TelegramSync, vec![])]
    );
    assert_eq!(
        state.answer(Tag::TelegramSync, Err("сеть".into()), NOW),
        [open]
    );
    state.answer(Tag::TelegramOpen, Ok(json!({"status": "error"})), NOW);
    assert_eq!(
        view(&state).telegram.hints,
        [("Could not open the bot. Try again later.".to_owned(), true)]
    );
    state.link_telegram(NOW + 2000);
    let snapshot =
        json!({"account": {"signedIn": true}, "telegram": {"linked": true, "username": "kanoyo"}});
    assert_eq!(
        state.answer(Tag::TelegramSync, Ok(snapshot.clone()), NOW),
        [Effect::Updated(snapshot)]
    );
    support(
        &mut state,
        json!({}),
        json!({"linked": true, "username": "kanoyo"}),
        no_link(),
    );
    let shown = view(&state).telegram;
    assert_eq!(shown.label, "Telegram: linked (@kanoyo)");
    assert_eq!(shown.hints.len(), 1);
    assert!(shown.hints[0].0.starts_with("Subscriptions and donations"));
}

#[test]
fn telegram_errors_rate_limit_bad_codes_and_expiry() {
    let mut state = state();
    state.link_telegram(NOW);
    state.answer(
        Tag::TelegramCode,
        Ok(json!({"status": "error", "code": "RUNNER_HTTP_429"})),
        NOW,
    );
    assert_eq!(
        view(&state).telegram.hints,
        [("Too often, try later.".to_owned(), true)]
    );
    state.link_telegram(NOW);
    assert_eq!(
        state.answer(
            Tag::TelegramCode,
            Ok(json!({"status": "ok", "code": "12345", "expiresAt": 1})),
            NOW
        ),
        []
    );
    assert_eq!(view(&state).telegram.hints, [(FORM_ERROR.to_owned(), true)]);
    state.link_telegram(NOW);
    state.answer(Tag::TelegramCode, Ok(code_reply()), NOW);
    state.code_expired(Code::Telegram);
    let hints = view(&state).telegram.hints;
    assert!(hints[0].0.starts_with("Press “Link”") && !hints[0].1);
    // Код вышел — «Привязать» берёт новый.
    assert_eq!(
        state.link_telegram(NOW),
        [request("runner-telegram-code", Tag::TelegramCode, vec![])]
    );
}

#[test]
fn telegram_refresh_says_when_the_link_is_not_there_yet() {
    let mut state = state();
    assert_eq!(
        state.refresh_telegram(),
        [request("runner-sync", Tag::TelegramRefresh, vec![])]
    );
    assert_eq!(state.refresh_telegram(), [], "уже идёт");
    let snapshot = json!({"account": {"signedIn": true}, "telegram": {"linked": false}});
    assert_eq!(
        state.answer(Tag::TelegramRefresh, Ok(snapshot.clone()), NOW),
        [Effect::Updated(snapshot)]
    );
    assert_eq!(
        view(&state).telegram.hints,
        [(
            "No link found yet. Start the bot with the code and refresh again.".to_owned(),
            false
        )]
    );
    // Привязку принесла синхронизация: «пока не найдена» больше не к месту.
    support(&mut state, json!({}), json!({"linked": true}), no_link());
    assert_eq!(view(&state).telegram.label, "Telegram: linked");
    assert!(
        view(&state).telegram.hints[0]
            .0
            .starts_with("Subscriptions")
    );
    state.refresh_telegram();
    state.answer(
        Tag::TelegramRefresh,
        Ok(json!({"account": {"signedIn": true}, "error": "RUNNER_NETWORK"})),
        NOW,
    );
    assert_eq!(view(&state).telegram.hints, [(NETWORK.to_owned(), true)]);
    state.refresh_telegram();
    state.answer(Tag::TelegramRefresh, Ok(Value::Null), NOW);
    assert_eq!(view(&state).telegram.hints, [(NETWORK.to_owned(), true)]);
    assert_eq!(
        state.unlink_telegram(),
        [request(
            "runner-telegram-unlink",
            Tag::TelegramUnlink,
            vec![]
        )]
    );
    assert_eq!(
        state.answer(
            Tag::TelegramUnlink,
            Ok(json!({"status": "error", "code": "RUNNER_NETWORK"})),
            NOW
        ),
        []
    );
    assert_eq!(view(&state).telegram.hints, [(NETWORK.to_owned(), true)]);
}

#[test]
fn discord_gives_a_code_with_steps_and_syncs_on_focus() {
    let mut state = state();
    let shown = view(&state).discord;
    assert_eq!(shown.label, "Discord: not linked");
    assert!(shown.hint.starts_with("Link Discord to your profile"));
    assert_eq!(shown.steps.len(), 5);
    assert!(shown.steps[1].1, "шаг со ссылкой «Открыть»");
    assert_eq!(shown.invite, "https://discord.com/invite/8Np6brWBuz");
    assert_eq!(
        shown.steps[2].0,
        "In any channel, type the command: /link code. Copy command puts it on your clipboard."
    );
    assert_eq!(state.window_focused(), [], "кода нет");
    assert_eq!(
        state.request_discord_code(),
        [request("runner-discord-code", Tag::DiscordCode, vec![])]
    );
    assert!(view(&state).discord.busy);
    assert_eq!(
        state.answer(Tag::DiscordCode, Ok(code_reply()), NOW),
        [Effect::CodeTimer(Code::Discord, Some(600_000))]
    );
    let shown = view(&state).discord;
    assert!(shown.code && !shown.busy);
    assert_eq!(shown.command, "/link 123456");
    assert_eq!(shown.hint, "Code valid until 12:10");
    assert_eq!(shown.steps.len(), 4, "шаг «нажмите Привязать» не нужен");
    let ru = state.view(Language::Ru, &wall(), &ru()).discord;
    assert_eq!(ru.command, "/привязать 123456");
    assert!(
        ru.steps[1]
            .0
            .contains("/привязать 123456 (или /link 123456)")
    );
    assert_eq!(
        state.window_focused(),
        [request("runner-sync", Tag::DiscordSync, vec![])]
    );
    let snapshot = json!({"account": {"signedIn": true}});
    assert_eq!(
        state.answer(Tag::DiscordSync, Ok(snapshot.clone()), NOW),
        [Effect::Updated(snapshot)]
    );
    assert!(view(&state).discord.code);
    // Срок вышел — фокус снимает код.
    assert_eq!(
        state.answer(Tag::DiscordSync, Err("сеть".into()), NOW + 600_000),
        [Effect::CodeTimer(Code::Discord, None)]
    );
    assert!(!view(&state).discord.code);
}

#[test]
fn discord_link_arrives_drops_the_code_and_shows_the_boost() {
    let mut state = state();
    state.request_discord_code();
    state.answer(Tag::DiscordCode, Ok(code_reply()), NOW);
    let linked = json!({"linked": true, "username": "kanoyo", "boosterSince": 1_794_700_800});
    assert_eq!(
        state.set_support(&json!({}), false, None, &no_link(), &linked),
        [Effect::CodeTimer(Code::Discord, None)]
    );
    let shown = view(&state).discord;
    assert!(shown.linked && !shown.code);
    assert_eq!(shown.label, "Discord: kanoyo");
    assert_eq!(shown.hint, "Boosting the server since 15 November 2026");
    assert_eq!(
        state.view(Language::Ru, &wall(), &ru()).discord.hint,
        "Бустит сервер с 15 ноября 2026 г."
    );
    assert_eq!(
        state.unlink_discord(),
        [
            Effect::CodeTimer(Code::Discord, None),
            request("runner-discord-unlink", Tag::DiscordUnlink, vec![]),
        ]
    );
    state.set_support(
        &json!({}),
        false,
        None,
        &no_link(),
        &json!({"linked": true}),
    );
    assert_eq!(view(&state).discord.label, "Discord: linked");
    assert_eq!(view(&state).discord.hint, "");
}

#[test]
fn a_refused_discord_code_ends_with_the_general_text_as_in_049() {
    let mut state = state();
    state.request_discord_code();
    state.answer(
        Tag::DiscordCode,
        Ok(json!({"status": "error", "code": "RUNNER_HTTP_429"})),
        NOW,
    );
    assert_eq!(view(&state).discord.message, FORM_ERROR);
    state.toggle_discord_help();
    assert!(view(&state).discord.help);
}
