//! Общие окна в корне оболочки на фикстурах: папка библиотеки (0.4.9: «папка занята»), слот
//! подтверждения (`askConfirm` → щелчок → ответ сценарию).

use crate::app::LauncherRoot;
use crate::layer::Layers;
use crate::shell::root::ShellContext;
use crate::snap::{bounds_of, pointer_move, press};
use gpui::{MouseButton, TestAppContext, VisualTestContext};
use hlsr_ui::controllers::shell::{Confirm, ConfirmAnswer};
use hlsr_ui::fixtures::{FixtureServices, Scene, Variant};
use hlsr_ui::services::Services;
use serde_json::json;
use std::sync::Arc;

fn fixtures() -> Arc<FixtureServices> {
    let scene: Scene = serde_json::from_value(json!({
        "id": "t", "route": "/launcher", "platform": "win32",
        "window": {"width": 1600, "height": 900},
        "themes": ["hlsr"], "languages": ["ru"], "scales": [1],
        "now": "2026-11-17T12:00:00.000Z", "timezone": "UTC",
        "store": {"settings": {"config": {"language": 1}}},
        "invoke": {
            "get-library-path": {"path": "D:\\Games", "confirmed": false},
            "validate-install-path": {"ok": true, "path": "D:\\Games", "freeSpaceMb": 2048},
            "set-library-path": {"ok": false, "error": "busy"}
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

fn invokes(fx: &FixtureServices, channel: &str) -> usize {
    fx.log()
        .iter()
        .filter(|entry| entry["kind"] == "invoke" && entry["channel"] == channel)
        .count()
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
fn library_busy_then_confirm_slot_answers(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let fx = fixtures();
    let services: Services = fx.clone();
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.run_until_parked();

    // Первый запуск: окно папки открыто (в стопке), путь проверен.
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 1);
    assert_eq!(invokes(&fx, "validate-install-path"), 1);

    // «Использовать эту папку» — busy: окно остаётся, кнопка недоступна (второй щелчок — мимо).
    click(cx, "library-confirm");
    assert_eq!(invokes(&fx, "set-library-path"), 1);
    click(cx, "library-confirm");
    assert_eq!(invokes(&fx, "set-library-path"), 1);
    let busy = cx.update(|_, cx| {
        ShellContext::get(cx)
            .unwrap()
            .flows
            .with_state(|s| s.library_setup_open)
    });
    assert!(busy);

    // askConfirm сценария: окно поверх, «Подтвердить» — ответ «да», окно уходит из стопки.
    let mut answer = cx.update(|_, cx| {
        ShellContext::get(cx).unwrap().shell.ask_confirm(Confirm {
            title: "Удалить".into(),
            text: "C:\\HLSR\\Games\\ghosting-renewed".into(),
            items: vec!["C:\\HLSR\\Games\\ghosting-renewed\\hl2\\cfg".into()],
            note: Some("cfg".into()),
            danger: true,
            ..Confirm::default()
        })
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 2);
    click(cx, "confirm-accept");
    assert_eq!(answer.try_answer(), Some(ConfirmAnswer::Accepted));
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 1);

    // Варианты: двойной щелчок не нужен — «Подтвердить» отвечает выбранным по умолчанию.
    let mut choice = cx.update(|_, cx| {
        ShellContext::get(cx).unwrap().shell.ask_confirm(Confirm {
            title: "Сборка".into(),
            choices: vec![("a".into(), "A".into()), ("b".into(), "B".into())],
            default_choice: Some("b".into()),
            ..Confirm::default()
        })
    });
    cx.run_until_parked();
    click(cx, "confirm-accept");
    assert_eq!(choice.try_answer(), Some(ConfirmAnswer::Choice("b".into())));
}
