//! «Рекорды» и «Игроки» в окне лаунчера на фикстурах: что уходит главному процессу (как
//! 0.4.9): игроки — только на открытой вкладке, режим, сортировка, «Обновить»; щелчок по
//! карточке — забег, по бегуну с профилем — профиль, а не забег.

use crate::app::LauncherRoot;
use crate::layer::Layers;
use crate::shell::root::ShellContext;
use crate::snap::{bounds_of, pointer_move, press};
use gpui::{MouseButton, TestAppContext, VisualTestContext};
use hlsr_ui::fixtures::{FixtureServices, Scene, Variant};
use hlsr_ui::services::Services;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const ME: &str = "76561199155902676";

fn profile(id: &str, name: &str, rank: u32) -> Value {
    json!({"steamId64": id, "personaName": name, "nick": null, "avatarFrame": null,
           "subscriber": rank == 1, "details": {"avatarUrl": ""},
           "presence": {"online": true, "gameId": null, "launcherId": null},
           "games": {"hl1-won": {"totalPlaySeconds": 7200, "launchCount": 3}},
           "achievementCount": 4, "rating": {"score": 2}, "registeredAt": null, "rank": rank})
}

fn top(sort: &str) -> Value {
    json!([{"view": "top", "sort": sort, "game": "", "offset": 0}])
}

fn fixtures() -> Arc<FixtureServices> {
    let run = |id: &str, key: &str, place: Value, profiles: Value| {
        json!({"kind": "run", "id": id, "game": key.to_uppercase(), "gameKey": key,
               "category": "Any%", "subcategory": "", "runners": ["Zed", "Mia"],
               "seconds": 1652.1, "date": "2026-11-16T10:00:00.000Z",
               "url": format!("https://www.speedrun.com/{key}/runs/{id}"),
               "place": place, "profiles": profiles})
    };
    let online = json!({"status": "ok", "signedIn": true, "total": 2, "playing": 0,
                        "profiles": [profile(ME, "Kanoyo", 1), profile("76561190000000002", "Zed", 2)]});
    let rated = json!({"status": "ok", "signedIn": true, "more": false,
                       "profiles": [profile(ME, "Kanoyo", 1)]});
    let scene: Scene = serde_json::from_value(json!({
        "id": "t", "route": "/launcher", "platform": "win32",
        "window": {"width": 1600, "height": 900},
        "themes": ["hlsr"], "languages": ["ru"], "scales": [1],
        "now": "2026-11-17T12:00:00.000Z", "timezone": "UTC",
        "store": {"settings": {"config": {"language": 1}}},
        "invoke": {
            "get-library-path": {"path": "C:\\HLSR", "confirmed": true},
            "runner-state": {"status": "signed-in", "account": {"signedIn": true, "steamId64": ME}},
            "runner-players": {"$args": [
                [[{"view": "online"}], online],
                [top("hours"), rated.clone()],
                [top("new"), rated]
            ]},
            "public-news-get": {"status": "ok", "items": [
                run("a1", "hl2", json!(1), json!([null, ME])),
                run("b1", "hl1", Value::Null, Value::Null)
            ]}
        }
    }))
    .unwrap();
    let variant = Variant {
        theme: "hlsr".into(),
        lang: "ru".into(),
        scale: 1.0,
        zoom: None,
        rail: None,
    };
    Arc::new(FixtureServices::new(&scene, &variant).unwrap())
}

fn calls(fx: &FixtureServices, channel: &str) -> Vec<Value> {
    fx.log()
        .into_iter()
        .filter(|entry| entry["kind"] == "invoke" && entry["channel"] == channel)
        .map(|entry| entry["args"].clone())
        .collect()
}

fn settle(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(1000));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn click(cx: &mut VisualTestContext, target: &str) {
    settle(cx);
    let at = bounds_of(target)
        .unwrap_or_else(|| panic!("нет цели {target}"))
        .center();
    cx.update(|window, cx| {
        pointer_move(window, cx, at);
        press(window, cx, at, MouseButton::Left, 1, true);
        press(window, cx, at, MouseButton::Left, 1, false);
    });
    cx.run_until_parked();
}

#[gpui::test]
fn records_and_players_talk_to_the_main_process_like_0_4_9(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let fx = fixtures();
    let services: Services = fx.clone();
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.run_until_parked();
    if cx.update(|_, cx| Layers::open_count(cx)) > 0 {
        cx.update(|_, cx| ShellContext::get(cx).unwrap().flows.on_welcome_later());
    }
    settle(cx);
    click(cx, "topbar-tab-home");
    // Вкладка игроков закрыта — списков не просим.
    assert!(calls(&fx, "runner-players").is_empty());

    // «Рекорды»: щелчок по бегуну с профилем открывает профиль, а не забег.
    click(cx, "home-tabs-1");
    click(cx, "home-runner-0-1");
    assert!(calls(&fx, "runner-open-link").is_empty());
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 1);
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 0);
    // Фильтр «HL»: остаётся забег hl1; карточка — ссылка на speedrun.com.
    click(cx, "home-chip-hl1");
    click(cx, "home-record-0");
    assert_eq!(
        calls(&fx, "runner-open-link"),
        [json!(["https://www.speedrun.com/hl1/runs/b1"])]
    );

    // «Игроки»: открылась вкладка — «В сети»; «Топ» — часы; «Новые» — заново.
    click(cx, "home-tabs-2");
    assert_eq!(calls(&fx, "runner-players"), [json!([{"view": "online"}])]);
    click(cx, "players-mode-top");
    click(cx, "players-sort-new");
    assert_eq!(
        calls(&fx, "runner-players")[1..],
        [top("hours"), top("new")]
    );
    // «Обновить» на вкладке игроков — лента и рейтинг заново.
    click(cx, "home-refresh");
    assert_eq!(
        calls(&fx, "public-news-get").last(),
        Some(&json!([{"force": true}]))
    );
    assert_eq!(calls(&fx, "runner-players").last(), Some(&top("new")));
    assert_eq!(calls(&fx, "runner-players").len(), 4);

    // Строка игрока открывает его профиль.
    click(cx, "player-0");
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 1);
}
