//! Нижняя панель в корне оболочки на фикстурах: эпизоды Ghosting (размер при показе карточки,
//! «Скачать» эпизода — свой канал), меню игры (`game-menu-state`, «Проверить файлы»).

use crate::app::LauncherRoot;
use crate::shell::root::ShellContext;
use crate::snap::{bounds_of, pointer_move, press};
use gpui::{MouseButton, TestAppContext, VisualTestContext};
use hlsr_ui::fixtures::{FixtureServices, Scene, Variant};
use hlsr_ui::services::Services;
use serde_json::{Value, json};
use std::sync::Arc;

fn fixtures() -> Arc<FixtureServices> {
    let ghosting = json!({
        "gameId": "ghosting-renewed", "launcherId": "218", "state": "ready", "percent": 100,
        "installable": true, "source": "http", "operation": null,
        "mainButton": {"state": "ready", "action": "play", "enabled": true},
        "episodes": {
            "ep1": {"installed": false, "size": 2147483648u64, "paused": false},
            "ep2": {"installed": false, "size": 751619277u64, "paused": false}
        }
    });
    let scene: Scene = serde_json::from_value(json!({
        "id": "t", "route": "/launcher", "platform": "win32",
        "window": {"width": 1600, "height": 900},
        "themes": ["hlsr"], "languages": ["ru"], "scales": [1],
        "now": "2026-11-17T12:00:00.000Z", "timezone": "UTC",
        "store": {"settings": {"config": {"language": 1}}},
        "invoke": {
            "game-delivery-info": {"pending": [], "legacyLayout": []},
            "game-delivery-get-status": [ghosting],
            "game-menu-state": {"items": {
                "settings": {"enabled": true}, "folder": {"enabled": true},
                "verify": {"enabled": true}, "uninstall": {"enabled": true}
            }},
            "game-episode-install": {"status": "ok"},
            "game-menu-verify-files": {"ok": true, "damaged": 0}
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

#[gpui::test]
fn episodes_and_the_game_menu_talk_to_main_like_0_4_9(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let fx = fixtures();
    let services: Services = fx.clone();
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.run_until_parked();
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();

    // Карточка Ghosting с эпизодами на экране — размер эпизодов один раз; меню знает игру.
    assert_eq!(calls(&fx, "game-episodes-status"), [json!([])]);
    assert_eq!(calls(&fx, "game-menu-state"), [json!(["ghosting-renewed"])]);

    // «Episode One» в «Версии сборки»: кнопка — «Скачать», качаются оба эпизода.
    cx.update(|_, cx| {
        let flows = ShellContext::get(cx).unwrap().flows.clone();
        flows.on_select_build("Episode One");
    });
    cx.run_until_parked();
    click(cx, "download-play");
    assert_eq!(calls(&fx, "game-episode-install"), [json!(["ep2"])]);
    assert_eq!(calls(&fx, "game-episodes-status").len(), 2);
    assert!(calls(&fx, "game-process-launch").is_empty());

    // Меню: при открытии спрашивает main заново; «Проверить файлы» — канал main.
    click(cx, "game-menu");
    assert_eq!(calls(&fx, "game-menu-state").len(), 2);
    click(cx, "game-menu-verify");
    assert_eq!(
        calls(&fx, "game-menu-verify-files"),
        [json!(["ghosting-renewed"])]
    );
}
