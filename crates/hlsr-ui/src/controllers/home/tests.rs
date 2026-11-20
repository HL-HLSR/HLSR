use super::*;
use std::rc::Rc;

/// 2026-10-16T12:00:00Z.
const NOW: i64 = 1_792_152_000_000;

fn en() -> Tr {
    Rc::new(|key, args| crate::tr(Language::En, key, args))
}

fn t(key: &str, args: &[&str]) -> String {
    crate::tr(Language::En, key, args)
}

fn state() -> HomeState {
    HomeState::new(WallClock::fixed(NOW, 0))
}

fn post(id: &str, extra: Value) -> Value {
    let mut post = json!({
        "kind": "post", "id": id, "title": "HLSR 0.4.7", "body": "Hello **world**", "author": "HLSR",
        "authorProfile": null, "images": [],
        "reactions": {"like": 12, "love": 4, "fire": 7, "wow": 0, "celebrate": 2},
        "date": "2026-10-14T09:30:00.000Z"
    });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        post[key] = value;
    }
    post
}

fn record(id: &str, game: &str, key: &str, sub: &str) -> Value {
    json!({"kind": "record", "id": id, "game": game, "gameKey": key, "category": "New Engine",
           "subcategory": sub, "runners": ["Buster12", "Zed"], "seconds": 2190.285,
           "date": "2026-09-15T22:14:46.000Z", "url": format!("https://www.speedrun.com/runs/{id}")})
}

fn signed_in() -> Value {
    json!({"status": "signed-in", "account": {"signedIn": true, "steamId64": "76561199155902676"}})
}

fn news(items: Vec<Value>) -> Reply {
    Ok(json!({"status": "ok", "items": items}))
}

fn invoke_channels(effects: &[Effect]) -> Vec<&'static str> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Invoke { channel, .. } => Some(*channel),
            _ => None,
        })
        .collect()
}

#[test]
fn mounting_asks_the_account_then_the_feed_without_force() {
    let mut home = state();
    let effects = home.mounted();
    assert_eq!(
        invoke_channels(&effects),
        ["runner-state", "public-news-get"]
    );
    assert_eq!(
        effects[1],
        Effect::Invoke {
            tag: Tag::News,
            channel: "public-news-get",
            args: vec![json!({"force": false})]
        }
    );
    assert!(home.is_loading() && home.is_spinning());
}

#[test]
fn the_feed_splits_into_posts_and_records_and_loading_ends() {
    let mut home = state();
    home.mounted();
    let view = home.view(Language::En, &en());
    assert_eq!(view.empty_posts, t("#L_HOME_LOADING", &[]));
    home.news_reply(news(vec![
        post("41", json!({})),
        record("r1", "Half-Life 2", "hl2", ""),
    ]));
    let view = home.view(Language::En, &en());
    assert_eq!(view.posts.len(), 1);
    assert_eq!(view.records.len(), 1);
    assert_eq!(view.note, "");
    assert_eq!(view.empty_posts, "");
    assert!(!home.is_loading());
    // Спиннер не гаснет сам: останавливает адаптер на целом обороте.
    assert!(home.is_spinning());
    home.stop_spinning();
    assert!(!home.is_spinning());
}

#[test]
fn a_failed_load_shows_the_note_and_keeps_the_stale_items() {
    let mut home = state();
    home.mounted();
    home.news_reply(Err("x".into()));
    let view = home.view(Language::En, &en());
    assert_eq!(view.note, t("#L_HOME_FAILED", &[]));
    assert_eq!(view.empty_posts, t("#L_HOME_NO_POSTS", &[]));
    home.news_reply(news(vec![post("1", json!({}))]));
    assert_eq!(home.view(Language::En, &en()).note, "");
    home.news_reply(Ok(
        json!({"status": "error", "items": [post("1", json!({}))]}),
    ));
    assert_eq!(home.view(Language::En, &en()).note, t("#L_HOME_STALE", &[]));
    // На вкладке игроков замечания нет.
    home.select_tab("players");
    assert_eq!(home.view(Language::En, &en()).note, "");
}

#[test]
fn a_post_shows_author_dates_reactions_and_body() {
    let mut home = state();
    home.mounted();
    let author = json!({"steamId64": "7", "personaName": "Kanoyo", "avatarUrl": "https://a/1.png",
                        "nick": {"style": "developer", "css": null}, "avatarFrame": {"css": "x: y"}});
    home.news_reply(news(vec![post("41", json!({"authorProfile": author}))]));
    let view = home.view(Language::En, &en());
    let post = &view.posts[0];
    assert_eq!(
        (
            post.author_id.as_str(),
            post.name.as_str(),
            post.letter.as_str()
        ),
        ("7", "Kanoyo", "K")
    );
    assert_eq!(post.nick_style, "developer");
    assert_eq!(post.frame_css, "x: y");
    assert_eq!(post.avatar, "https://a/1.png");
    // Значок подписчика — только при `subscriber: true` (news-client 0.4.9).
    assert!(!post.subscriber);
    assert_eq!(post.date, "14 October 2026");
    assert_eq!(
        post.stamp,
        t("#L_HOME_POSTED_AT", &[])
            .replace("{date}", "14 October 2026")
            .replace("{time}", "09:30")
    );
    assert_eq!(post.body, "Hello **world**");
    let keys: Vec<_> = post.reactions.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(keys, ["like", "love", "fire", "wow", "celebrate"]);
    assert_eq!(post.reactions[0].count, 12);
    assert_eq!(
        post.reactions[0].label,
        format!("{} · 12", t("#L_HOME_REACTION_like", &[]))
    );
    // Без профиля — имя из поста; без имени — запасное.
    home.news_reply(news(vec![
        post_with_author("1", "Editor"),
        post_with_author("2", ""),
    ]));
    let view = home.view(Language::En, &en());
    assert_eq!(view.posts[0].name, "Editor");
    assert_eq!(view.posts[1].name, t("#L_HOME_AUTHOR_FALLBACK", &[]));
    assert_eq!(view.posts[0].author_id, "");
    let card = |subscriber: Value| json!({"authorProfile": {"steamId64": "7", "personaName": "K", "subscriber": subscriber}});
    home.news_reply(news(vec![
        self::post("1", card(json!(true))),
        self::post("2", card(json!("true"))),
    ]));
    let view = home.view(Language::En, &en());
    assert_eq!(
        (view.posts[0].subscriber, view.posts[1].subscriber),
        (true, false)
    );
}

fn post_with_author(id: &str, author: &str) -> Value {
    post(id, json!({"author": author}))
}

#[test]
fn image_tiles_follow_the_layout_module() {
    let mut home = state();
    home.mounted();
    let images = |n: usize| -> Value {
        (0..n)
            .map(|i| json!({"url": format!("https://i/{i}.png"), "width": 480, "height": 270}))
            .collect()
    };
    home.news_reply(news(vec![
        post("1", json!({"images": images(1)})),
        post("3", json!({"images": images(3)})),
        post("2", json!({"images": [
            {"url": "u", "width": 1000, "height": 500, "crop": {"x": 0.25, "y": 0.0, "w": 0.5, "h": 1.0}},
            {"url": "v", "width": 100, "height": 100}
        ]})),
    ]));
    let posts = home.view(Language::En, &en()).posts;
    assert!(posts[0].single_image && posts[0].tiles.len() == 1);
    assert!((posts[0].tiles[0].aspect - 16.0 / 9.0).abs() < 1e-6);
    // Три картинки: первая на всю ширину 2:1.
    assert!(posts[1].tiles[0].wide && !posts[1].tiles[1].wide);
    assert!((posts[1].tiles[0].aspect - 2.0).abs() < 1e-6);
    assert!((posts[1].tiles[1].aspect - 16.0 / 9.0).abs() < 1e-6);
    // Две картинки: обе 16:9; область 0,5 × 1 на 1000 × 500 — картинка шире плитки вдвое.
    assert!(
        (posts[2].tiles[0].width - 2.0).abs() < 1e-6,
        "{:?}",
        posts[2].tiles[0]
    );
    assert!(posts[2].tiles[0].left < 0.0);
}

#[test]
fn record_chips_are_ordered_by_game_with_short_labels() {
    let mut home = state();
    home.mounted();
    home.news_reply(news(vec![
        record("a", "Half-Life 2", "hl2", ""),
        record("b", "Half-Life", "hl1", "Any% · Glitchless"),
        record("c", "Half-Life: Source", "other", ""),
    ]));
    let view = home.view(Language::En, &en());
    let labels: Vec<_> = view.chips.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            t("#L_HOME_ALL_GAMES", &[]).as_str(),
            "HL",
            "HL2",
            "Half-Life: Source"
        ]
    );
    assert!(view.chips[0].active);
    assert_eq!(view.chips[1].title, "Half-Life");
    // Все игры: перед категорией — название игры.
    assert!(view.records.iter().all(|r| r.game.contains(" · ")));
    let sub = view.records.iter().find(|r| r.id == "b").unwrap();
    assert_eq!(sub.subs, ["Any%", "Glitchless"]);
    assert_eq!(sub.runner, "Buster12, Zed");
    assert_eq!(sub.time, "36:30.285");
    assert_eq!(sub.date, "15 September 2026");
}

#[test]
fn a_single_game_has_no_chips_and_the_filter_resets_when_its_game_goes() {
    let mut home = state();
    home.mounted();
    home.news_reply(news(vec![record("a", "Half-Life 2", "hl2", "")]));
    assert!(home.view(Language::En, &en()).chips.is_empty());
    home.news_reply(news(vec![
        record("a", "Half-Life 2", "hl2", ""),
        record("b", "Half-Life", "hl1", ""),
    ]));
    home.set_filter("hl1");
    assert_eq!(home.visible_record_ids(), ["b"]);
    let view = home.view(Language::En, &en());
    assert!(view.records[0].game == "New Engine");
    home.news_reply(news(vec![record("a", "Half-Life 2", "hl2", "")]));
    assert_eq!(home.filter(), "all");
}

fn run(id: &str, key: &str, category: &str, sub: &str, place: Value) -> Value {
    json!({"kind": "run", "id": id, "game": key.to_uppercase(), "gameKey": key,
           "category": category, "subcategory": sub, "runners": ["A"], "seconds": 60.5,
           "date": "2026-11-15T10:00:00.000Z", "url": "https://www.speedrun.com/x/runs/1",
           "place": place, "profiles": null})
}

#[test]
fn all_games_keep_three_fresh_runs_per_board_and_every_record() {
    let mut home = state();
    home.mounted();
    let mut items = vec![record("wr", "Half-Life 2", "hl2", "")];
    // Пять забегов одной таблицы, затем другая подкатегория и другая категория.
    items.extend((1..=5).map(|n| run(&format!("a{n}"), "hl2", "Any%", "", Value::Null)));
    items.push(run("b1", "hl2", "Any%", "Glitchless", json!(2)));
    items.push(run("c1", "hl2", "100%", "", json!(4)));
    items.push(record("wr2", "Half-Life 2", "hl2", ""));
    items.push(run("h1", "hl1", "Any%", "", Value::Null));
    home.news_reply(news(items));
    assert_eq!(
        home.visible_record_ids(),
        ["wr", "a1", "a2", "a3", "b1", "c1", "wr2", "h1"]
    );
    // У игры — все её забеги.
    home.set_filter("hl2");
    assert_eq!(home.visible_record_ids().len(), 9);
    home.set_filter("all");

    let view = home.view(Language::En, &en());
    let by_id = |id: &str| view.records.iter().find(|r| r.id == id).unwrap().clone();
    // Рекорд — первое место, свежий забег — место с сервера (только 1–3).
    let wr = by_id("wr");
    assert_eq!(
        (wr.place, wr.place_label.as_str()),
        (1, t("#L_HOME_WR", &[]).as_str())
    );
    let second = by_id("b1");
    assert_eq!(second.place, 2);
    assert_eq!(second.place_label, t("#L_HOME_PLACE", &["2"]));
    assert_eq!(second.subs, ["Glitchless"]);
    for id in ["a1", "c1"] {
        let plain = by_id(id);
        assert_eq!((plain.place, plain.place_label.as_str()), (0, ""));
        assert!(plain.subs.is_empty());
    }
}

#[test]
fn runners_with_a_linked_profile_carry_its_steam_id() {
    let mut home = state();
    home.mounted();
    let mut linked = run("r", "hl2", "Any%", "", json!(1));
    linked["runners"] = json!(["Kanoyo", "Zed", "Mia"]);
    linked["profiles"] = json!(["76561199155902676", null, "bad"]);
    home.news_reply(news(vec![linked]));
    let view = home.view(Language::En, &en());
    let record = &view.records[0];
    assert_eq!(record.runner, "Kanoyo, Zed, Mia");
    let profiles: Vec<&str> = record.runners.iter().map(|r| r.profile.as_str()).collect();
    // Проверку SteamID64 делает news-client (O-45), и ещё раз `runner-profile-view`
    // (`RUNNER_INVALID_ID`); сюда приходит строка или null.
    assert_eq!(profiles, ["76561199155902676", "", "bad"]);
}

#[test]
fn a_runner_takes_the_profile_at_its_own_position() {
    let mut home = state();
    home.mounted();
    let mut linked = run("r", "hl2", "Any%", "", json!(1));
    linked["runners"] = json!([null, "Zed"]);
    linked["profiles"] = json!(["76561199155902676", "76561199155902677"]);
    home.news_reply(news(vec![linked]));
    let view = home.view(Language::En, &en());
    let runners = &view.records[0].runners;
    assert_eq!(runners.len(), 1);
    assert_eq!(runners[0].profile, "76561199155902677");
}

#[test]
fn the_filter_swaps_in_the_direction_of_the_chip() {
    let mut home = state();
    home.mounted();
    home.news_reply(news(vec![
        record("a", "Half-Life 2", "hl2", ""),
        record("b", "Half-Life", "hl1", ""),
    ]));
    home.set_filter("hl2");
    assert!(home.filter_next());
    home.set_filter("hl1");
    assert!(!home.filter_next());
    home.set_filter("hl1");
    assert!(!home.filter_next());
}

#[test]
fn tabs_slide_forward_or_back() {
    let mut home = state();
    home.select_tab("records");
    assert!(home.view(Language::En, &en()).slide_next);
    home.select_tab("feed");
    let view = home.view(Language::En, &en());
    assert!(!view.slide_next && view.tab == 0);
    home.select_tab("players");
    assert_eq!(home.view(Language::En, &en()).tab, 2);
}

fn react_ready() -> HomeState {
    let mut home = state();
    home.mounted();
    home.news_reply(news(vec![post("41", json!({}))]));
    home.profile_state(&signed_in());
    home
}

#[test]
fn a_guest_pressing_a_reaction_gets_the_sign_in_notice() {
    let mut home = state();
    home.mounted();
    home.news_reply(news(vec![post("41", json!({}))]));
    assert!(home.react("41", "like").is_empty());
    let post = &home.view(Language::En, &en()).posts[0];
    assert_eq!(post.notice, t("#L_HOME_REACT_SIGN_IN", &[]));
    assert!(post.notice_sign_in);
    assert_eq!(post.reactions[0].count, 12);
}

#[test]
fn a_reaction_is_counted_at_once_and_confirmed_by_the_server() {
    let mut home = react_ready();
    let effects = home.react("41", "like");
    assert_eq!(
        effects,
        vec![Effect::Invoke {
            tag: Tag::React("41".into(), "like".into(), 1),
            channel: "public-news-react",
            args: vec![json!("41"), json!("like")]
        }]
    );
    let view = home.view(Language::En, &en());
    assert_eq!(view.posts[0].reactions[0].count, 13);
    assert!(view.posts[0].reactions[0].selected && view.posts[0].pending);
    // Пока ответа нет, вторая реакция на тот же пост не принимается.
    assert!(home.react("41", "fire").is_empty());
    home.react_reply(
        "41",
        "like",
        1,
        Ok(json!({"status": "ok", "reactions": {"like": 14, "love": 4, "fire": 7, "wow": 0, "celebrate": 2}, "mine": ["like"]})),
    );
    let view = home.view(Language::En, &en());
    assert_eq!(view.posts[0].reactions[0].count, 14);
    assert!(!view.posts[0].pending && view.posts[0].reactions[0].selected);
    // Снятие реакции уменьшает счёт, но не ниже нуля.
    home.react("41", "like");
    assert_eq!(home.counts("41")["like"], 13);
    assert!(home.mine_of("41").is_empty());
}

#[test]
fn a_rejected_reaction_is_taken_back_with_the_right_notice() {
    let mut home = react_ready();
    home.react("41", "wow");
    assert_eq!(home.counts("41")["wow"], 1);
    home.react_reply(
        "41",
        "wow",
        1,
        Ok(json!({"status": "error", "code": "RUNNER_HTTP_429"})),
    );
    assert_eq!(home.counts("41")["wow"], 0);
    assert!(home.mine_of("41").is_empty());
    assert_eq!(home.notice("41"), "#L_HOME_REACT_LIMIT");
    home.react("41", "wow");
    home.react_reply("41", "wow", 1, Err("net".into()));
    assert_eq!(home.notice("41"), "#L_HOME_REACT_FAILED");
    assert_eq!(
        home.view(Language::En, &en()).posts[0].notice,
        t("#L_HOME_REACT_FAILED", &[])
    );
}

#[test]
fn signing_out_while_a_reaction_is_pending_restores_the_counts_and_drops_the_answer() {
    let mut home = react_ready();
    home.react("41", "like");
    home.profile_state(&json!({"status": "signed-out"}));
    assert_eq!(home.counts("41")["like"], 12);
    assert!(home.mine_of("41").is_empty());
    // Ответ на реакцию прежнего входа отбрасывается.
    home.react_reply(
        "41",
        "like",
        1,
        Ok(json!({"status": "ok", "reactions": {"like": 99}, "mine": ["like"]})),
    );
    assert_eq!(home.counts("41")["like"], 12);
}

#[test]
fn my_reactions_load_after_the_account_and_skip_pending_posts() {
    let mut home = state();
    home.mounted();
    home.news_reply(news(vec![post("41", json!({})), post("40", json!({}))]));
    let effects = home.profile_state(&signed_in());
    assert_eq!(
        effects,
        vec![Effect::Invoke {
            tag: Tag::Mine(1, 2),
            channel: "public-news-mine",
            args: vec![json!(["41", "40"])]
        }]
    );
    home.mine_reply(
        1,
        2,
        Ok(json!({"status": "ok", "mine": {"41": ["like", "fire"], "40": ["love"]}})),
    );
    assert_eq!(home.mine_of("41"), ["like", "fire"]);
    // Ответ на устаревший запрос не принимается.
    home.mine_reply(1, 1, Ok(json!({"status": "ok", "mine": {"40": []}})));
    assert_eq!(home.mine_of("40"), ["love"]);
    // Тот же вход повторно ничего не спрашивает.
    assert!(home.profile_state(&signed_in()).is_empty());
}

#[test]
fn links_in_a_post_report_failures_on_that_post() {
    let mut home = react_ready();
    let effects = home.open_post_link("https://hlsr.pro", "41");
    assert_eq!(invoke_channels(&effects), ["public-news-open-link"]);
    home.link_reply("41", Ok(json!({"status": "ok"})));
    assert_eq!(home.notice("41"), "");
    home.link_reply("41", Ok(json!({"status": "error"})));
    assert_eq!(home.notice("41"), "#L_HOME_LINK_FAILED");
    assert_eq!(
        invoke_channels(&home.open_record("https://x")),
        ["runner-open-link"]
    );
}

#[test]
fn the_image_viewer_opens_at_the_tile_moves_within_bounds_and_closes() {
    let mut home = state();
    home.mounted();
    home.news_reply(news(vec![post(
        "41",
        json!({"images": [{"url": "a", "width": 1, "height": 1}, {"url": "b", "width": 1, "height": 1}, {"url": "c", "width": 1, "height": 1}]}),
    )]));
    assert_eq!(home.show_image("41", 1), vec![Effect::Overlay(true)]);
    assert_eq!(home.viewer().unwrap().index, 1);
    home.move_image(1);
    home.move_image(1);
    assert_eq!(home.viewer().unwrap().index, 2);
    home.move_image(-5);
    assert_eq!(home.viewer().unwrap().index, 0);
    assert_eq!(home.viewer().unwrap().title, "HLSR 0.4.7");
    assert_eq!(home.close_image(), vec![Effect::Overlay(false)]);
    assert!(home.close_image().is_empty());
}

#[test]
fn run_times_are_formatted_like_speedrun_com() {
    for (seconds, text) in [
        (2190.285, "36:30.285"),
        (59.9996, "1:00.000"),
        (3600.0, "1:00:00.000"),
        (7325.5, "2:02:05.500"),
        (0.0, "0:00.000"),
        (12.04, "0:12.040"),
    ] {
        assert_eq!(format_run(seconds), text, "{seconds}");
    }
}

#[test]
fn the_refresh_icon_stops_on_a_whole_turn() {
    // Ответ в тот же кадр, что и начало: стрелка делает целый оборот.
    assert_eq!(turn_remaining(0.0), 800.0);
    assert_eq!(turn_remaining(300.0), 500.0);
    assert_eq!(turn_remaining(800.0), 0.0);
    assert_eq!(turn_remaining(1_000.0), 600.0);
}

#[test]
fn direction_is_forward_for_the_same_or_a_later_item() {
    let ids: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
    assert!(direction_next(&ids, "a", "b"));
    assert!(direction_next(&ids, "b", "b"));
    assert!(!direction_next(&ids, "c", "a"));
}
