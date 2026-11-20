//! Шапка справа в окне GPUI на фикстурах: что уходит главному процессу от поиска, меню
//! аккаунта, кнопки спонсоров и карточки обновления.

use crate::app::LauncherRoot;
use crate::shell::root::ShellContext;
use crate::snap::{bounds_of, pointer_move, press};
use gpui::{MouseButton, TestAppContext, VisualTestContext};
use hlsr_ui::fixtures::{FixtureServices, Scene, Variant};
use hlsr_ui::services::Services;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

fn fixtures() -> Arc<FixtureServices> {
    let scene: Scene = serde_json::from_value(json!({
        "id": "t", "route": "/launcher", "platform": "win32",
        "window": {"width": 1600, "height": 900},
        "themes": ["hlsr"], "languages": ["ru"], "scales": [1],
        "now": "2026-11-17T12:00:00.000Z", "timezone": "UTC",
        "store": {"settings": {"config": {"language": null}}},
        "invoke": {
            "window-command": {"ok": true, "fullscreen": false},
            "runner-state": {
                "status": "signed-in",
                "account": {"signedIn": true, "steamId64": "76561199155902676",
                            "personaName": "Kanoyo", "connection": "connected"},
                "nick": null, "avatarFrame": null, "subscriber": true,
                "presence": {"online": true, "gameId": null, "launcherId": "218"}
            },
            "runner-search": [
                {"steamId64": "1", "personaName": "Kanade", "subscriber": true},
                {"steamId64": "2", "personaName": "Kanoyo2", "details": {"hideSteamId": true}}
            ],
            "friends-avatars": {},
            "updater-get-state": {
                "state": "available", "enabled": true, "currentVersion": "0.4.9",
                "version": "0.5.0", "releaseNotes": "Новое", "progress": null,
                "error": null, "errorCode": null, "checkSource": "auto"
            },
            "updater-download": true
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

/// Аргументы вызовов `invoke` канала `channel` по порядку.
fn calls(fx: &FixtureServices, channel: &str) -> Vec<Value> {
    fx.log()
        .into_iter()
        .filter(|entry| entry["kind"] == "invoke" && entry["channel"] == channel)
        .map(|entry| entry["args"].clone())
        .collect()
}

fn click(cx: &mut VisualTestContext, target: &str) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
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

/// Открыт ли профиль и чей (`profileOpen`, `profileTarget`).
fn profile(cx: &mut VisualTestContext) -> (bool, String) {
    cx.update(|_, cx| {
        ShellContext::get(cx)
            .expect("оболочка")
            .flows
            .with_state(|s| (s.profile_open, s.profile_target.clone()))
    })
}

fn launcher(cx: &mut TestAppContext) -> (Arc<FixtureServices>, &mut VisualTestContext) {
    cx.update(crate::init);
    let fx = fixtures();
    let services: Services = fx.clone();
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.run_until_parked();
    (fx, cx)
}

#[gpui::test]
fn search_waits_350_ms_and_enter_opens_the_selected_runner(cx: &mut TestAppContext) {
    let (fx, cx) = launcher(cx);
    // `mounted`: состояние входа спрашивают и поиск, и меню аккаунта.
    assert!(calls(&fx, "runner-state").len() >= 2);

    click(cx, "runner-search-input");
    cx.simulate_input("kan");
    cx.run_until_parked();
    assert!(calls(&fx, "runner-search").is_empty());
    cx.executor().advance_clock(Duration::from_millis(350));
    cx.run_until_parked();
    assert_eq!(calls(&fx, "runner-search"), [json!(["kan"])]);
    // У кого нет аватара — спрашивают кеш Steam.
    assert_eq!(calls(&fx, "friends-avatars"), [json!([["1", "2"]])]);

    // ↓ — вторая строка, Enter — её профиль (чужой: не свой SteamID).
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    assert_eq!(profile(cx), (true, "2".to_owned()));
}

#[gpui::test]
fn update_card_account_menu_and_the_heart(cx: &mut TestAppContext) {
    let (fx, cx) = launcher(cx);

    // Карточка обновления: «Скачать» — `updater-download`; «Позже» прячет её (в окне 1920 × 1080
    // она закрывает кнопку аккаунта, как и в 0.4.9).
    click(cx, "update-card-download");
    assert_eq!(calls(&fx, "updater-download").len(), 1);
    click(cx, "update-card-later");
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    let dismissed = cx.update(|_, cx| {
        ShellContext::get(cx)
            .expect("оболочка")
            .flows
            .with_state(|s| s.update_dismissed)
    });
    assert!(dismissed);

    // Щелчок открывает меню, ↓ ↓ — «Настройки профиля», Enter — свой профиль.
    click(cx, "account-toggle");
    cx.simulate_keystrokes("down down enter");
    cx.run_until_parked();
    assert_eq!(profile(cx), (true, String::new()));

    // Esc закрывает профиль; сердце открывает «Спонсорство».
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert!(!profile(cx).0);
    click(cx, "sponsors-btn");
    assert!(cx.update(|_, cx| crate::sponsors::modal::sponsors_open(cx)));
}

/// Цели шапки, нарисованные в последнем кадре.
fn shown(cx: &mut VisualTestContext, targets: &[&str]) -> Vec<bool> {
    crate::snap::forget_targets();
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    targets
        .iter()
        .map(|target| bounds_of(target).is_some())
        .collect()
}

/// Самосборка: ни сердца спонсоров, ни поиска игроков, ни меню аккаунта, ни счётчиков онлайна;
/// «Спонсорство» не открывается, «Главная» ничего не спрашивает. Сервер принял билет
/// (`build-official`) — всё как в 0.4.9.
#[gpui::test]
fn the_unofficial_build_has_no_sponsors_players_or_sign_in(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let fx = fixtures();
    fx.set_build(hlsr_ui::services::Standing::Unofficial);
    let services: Services = fx.clone();
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.run_until_parked();
    let header = ["sponsors-btn", "runner-search-input", "account-toggle"];
    assert_eq!(shown(cx, &header), [false; 3]);
    let online = |cx: &mut VisualTestContext| -> Vec<String> {
        cx.update(|_, cx| {
            let nav = ShellContext::get(cx).expect("оболочка").nav.clone();
            nav.games().into_iter().map(|g| g.online_state).collect()
        })
    };
    assert!(online(cx).iter().all(String::is_empty));
    cx.update(|_, cx| crate::sponsors::modal::open_sponsors(cx));
    cx.run_until_parked();
    assert!(!cx.update(|_, cx| crate::sponsors::modal::sponsors_open(cx)));
    click(cx, "topbar-tab-home");
    assert!(calls(&fx, "public-news-get").is_empty());
    assert!(calls(&fx, "public-sponsors-get").is_empty());

    fx.emit("build-official", json!("official"));
    cx.run_until_parked();
    assert_eq!(shown(cx, &header), [true; 3]);
    assert!(online(cx).iter().all(|state| !state.is_empty()));
    assert_eq!(calls(&fx, "public-news-get").len(), 1);
}
