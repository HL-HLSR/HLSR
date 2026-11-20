//! Окна игры в корне оболочки на фикстурах: ремонт (галочки → «Скачать N файлов» → файлы в
//! `game-menu-repair`), падение игры (окно и «Закрыть»), окно FFmpeg перед «Демо».

use crate::app::LauncherRoot;
use crate::layer::Layers;
use crate::shell::root::ShellContext;
use crate::snap::{bounds_of, pointer_move, press};
use gpui::{MouseButton, TestAppContext, VisualTestContext};
use hlsr_ui::fixtures::{FixtureServices, Scene, Variant};
use hlsr_ui::services::Services;
use serde_json::{Value, json};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

fn fixtures() -> Arc<FixtureServices> {
    let files = json!([{"path": "bin/server.dll", "size": 2048, "kind": "damaged"},
        {"path": "cfg/autoexec.cfg", "size": 10, "kind": "cfg-changed"}]);
    let scene: Scene = serde_json::from_value(json!({
        "id": "t", "route": "/launcher", "platform": "win32",
        "window": {"width": 1600, "height": 900},
        "themes": ["hlsr"], "languages": ["ru"], "scales": [1],
        "now": "2026-11-17T12:00:00.000Z", "timezone": "UTC",
        "store": {"settings": {"config": {"language": 1}},
            "library": {"libraryConfirmed": true, "libraryPath": "D:\\HLSR"}},
        "invoke": {
            "get-library-path": {"path": "D:\\HLSR", "confirmed": true},
            "game-menu-repair": {"$seq": [
                {"status": "confirmation-required", "files": files},
                {"status": "ok", "repaired": ["bin/server.dll"], "remaining": []}]},
            "demo-ffmpeg-status": {"status": "ok", "ffmpeg": {"state": "not-installed",
                "installable": true, "origins": [{"site": "github.com", "release": "n9.0"}],
                "pinnedVersion": "9.0.2", "build": "gpl", "sizeBytes": 104857600}},
            "demo-ffmpeg-install": {"$pending": true}
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
        .iter()
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

fn layers(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| {
        window.refresh();
        Layers::open_count(cx)
    })
}

#[gpui::test]
fn repair_crash_and_ffmpeg_windows(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let fx = fixtures();
    let services: Services = fx.clone();
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    // Без анимаций: ушедшее окно не держит подложку над следующим.
    cx.update(|_, cx| crate::motion::AnimGate::set_no_anim(cx, true));
    cx.run_until_parked();
    let flows = cx.update(|_, cx| ShellContext::get(cx).unwrap().flows.clone());
    // Окна запуска (приветствие) остаются под окнами игры.
    let base = layers(cx);

    // Ремонт Ghosting: окно с файлами; изменённый cfg не отмечен — качается один файл.
    flows.with_state(|state| {
        state.selected_game_id = "218".into();
        state.delivery.insert(
            "218".to_owned(),
            json!({"gameId": "ghosting-renewed", "launcherId": "218", "fileRepair": true}),
        );
    });
    flows.on_menu_action("repair");
    cx.run_until_parked();
    assert_eq!(layers(cx), base + 1);
    click(cx, "confirm-accept");
    assert_eq!(
        calls(&fx, "game-menu-repair")[1],
        json!(["ghosting-renewed", {"confirmed": true, "recheck": false, "files": ["bin/server.dll"]}])
    );
    assert_eq!(layers(cx), base);

    // Падение игры: окно поверх, «Закрыть» убирает его.
    flows.on_event(
        "game-crash-report",
        &json!({"gameId": "ghosting-renewed", "launcherId": "218", "exitCode": -1073741819}),
    );
    cx.run_until_parked();
    assert_eq!(layers(cx), base + 1);
    click(cx, "crash-close");
    assert!(flows.with_state(|state| state.crash_report.is_none()));
    assert_eq!(layers(cx), base);

    // «Демо» без FFmpeg: окно; «Скачать» — установка, кнопка недоступна, вход — после ответа.
    let entered = Rc::new(Cell::new(false));
    let flag = entered.clone();
    flows.on_select_demo(false, move || flag.set(true));
    cx.run_until_parked();
    assert_eq!(layers(cx), base + 1);
    click(cx, "confirm-accept");
    assert_eq!(calls(&fx, "demo-ffmpeg-install").len(), 1);
    assert!(flows.with_state(|state| state.ffmpeg_gate.as_ref().is_some_and(|g| g.installing)));
    assert!(!entered.get());
}
