//! Спонсорство в окне GPUI на фикстурах: окно говорит с главным процессом как 0.4.9.

use crate::app::LauncherRoot;
use crate::layer::Layers;
use crate::shell::root::ShellContext;
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use hlsr_ui::fixtures::{FixtureServices, Scene, Variant};
use hlsr_ui::services::Services;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

fn fixtures(invoke: Value) -> Arc<FixtureServices> {
    let scene: Scene = serde_json::from_value(json!({
        "id": "t", "route": "/launcher", "platform": "win32",
        "window": {"width": 1600, "height": 900},
        "themes": ["hlsr"], "languages": ["ru"], "scales": [1],
        "now": "2026-11-17T12:00:00.000Z", "timezone": "UTC",
        "store": {"settings": {"config": {"language": 1}}},
        "invoke": invoke
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

fn settle(cx: &mut VisualTestContext, frames: usize) {
    for _ in 0..frames {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }
}

fn click(cx: &mut VisualTestContext, name: &str) {
    let at = crate::snap::bounds_of(name).unwrap_or_else(|| panic!("нет цели {name}"));
    cx.simulate_click(at.center(), Modifiers::none());
    settle(cx, 4);
}

fn list() -> Value {
    json!({"status": "ok", "version": 2,
        "links": {"bot": "https://t.me/hlsr_bot"},
        "active": [
            {"profile": {"personaName": "Kanoyo", "avatarUrl": null, "nick": null,
                         "avatarFrame": null, "subscriber": true},
             "name": "", "amount": 0, "note": "", "since": "",
             "link": {"mode": "profile", "steamId64": "76561199155902676"}}],
        "donors": [
            {"profile": null, "name": "Большой", "amount": 15000, "note": "", "since": "",
             "link": {"mode": "url", "url": "https://example.com/"}}],
        "past": []})
}

#[gpui::test]
fn the_window_asks_for_the_list_once_and_routes_clicks_like_0_4_9(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let fx = fixtures(json!({
        "public-sponsors-get": list(),
        "support-open": {"status": "ok"},
        "sponsor-open-link": {"status": "ok"},
        "profile-get": null, "runner-sync": null
    }));
    let services: Services = fx.clone();
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.run_until_parked();
    cx.update(|_, cx| super::modal::open_sponsors(cx));
    settle(cx, 30);
    assert_eq!(calls(&fx, "public-sponsors-get"), [json!([])]);
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 1);
    // Кнопки оплаты — в бота через main, адрес окно не знает.
    click(cx, "sponsors-subscribe");
    assert_eq!(calls(&fx, "support-open"), [json!(["subscribe"])]);
    // Свой адрес доната — по разделу и номеру.
    click(cx, "sponsors-donor-0");
    assert_eq!(
        calls(&fx, "sponsor-open-link"),
        [json!([{"section": "donors", "index": 0}])]
    );
    // Подписчик со ссылкой на профиль: окно закрывается, открывается профиль.
    click(cx, "sponsors-person-0");
    settle(cx, 30);
    let (open, target) = cx.update(|_, cx| {
        ShellContext::get(cx)
            .expect("оболочка")
            .flows
            .with_state(|s| (s.profile_open, s.profile_target.clone()))
    });
    assert!(open);
    assert_eq!(target, "76561199155902676");
    // Новое открытие — новый запрос (список не кешируется окном).
    cx.update(|_, cx| super::modal::open_sponsors(cx));
    settle(cx, 30);
    assert_eq!(calls(&fx, "public-sponsors-get").len(), 2);
    cx.simulate_keystrokes("escape");
    settle(cx, 30);
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 0);
}

#[gpui::test]
fn thanks_show_the_first_notice_until_closed(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let fx = fixtures(json!({}));
    let services: Services = fx.clone();
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.run_until_parked();
    // Чужое событие окно не открывает.
    fx.emit("support-thanks", json!({"id": "1", "kind": "gift"}));
    settle(cx, 4);
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 0);
    fx.emit(
        "support-thanks",
        json!({"id": "2", "kind": "subscription", "until": 1_797_000_000, "styles": true}),
    );
    settle(cx, 4);
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 1);
    cx.simulate_keystrokes("escape");
    settle(cx, 30);
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 0);
}

#[test]
fn badge_glow_covers_everything_outside_the_circle_once() {
    use gpui::{Bounds, Pixels, point, size};
    // Значок 48 × 48 (круг r 24) при масштабе экрана 1,25: пиксели устройства по центрам.
    let scale = 1.25;
    let badge = Bounds::new(
        point(Pixels::from(100.), Pixels::from(40.)),
        size(Pixels::from(48.), Pixels::from(48.)),
    );
    let masks = super::modal::outside(badge, Pixels::from(24.), Pixels::from(20.), scale);
    // Строки с краем в одном пикселе устройства слиты: 72 полосы, а не 2 + 2 × 60 строк.
    assert!(masks.len() <= 72, "{}", masks.len());
    let (cx, cy) = (124., 64.);
    for row in 0..((88. + 20.) * scale) as i32 {
        for col in 0..((148. + 20.) * scale) as i32 {
            let (x, y) = ((col as f32 + 0.5) / scale, (row as f32 + 0.5) / scale);
            let hits = masks
                .iter()
                .filter(|m| m.contains(&point(Pixels::from(x), Pixels::from(y))))
                .count();
            assert!(hits <= 1, "дважды в ({x}, {y})");
            let distance = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            let near = x > 80. && x < 168. && y > 20. && y < 108.;
            if distance < 24. - 1. {
                assert_eq!(hits, 0, "тень под кругом в ({x}, {y})");
            } else if distance > 24. + 1. && near {
                assert_eq!(hits, 1, "нет тени в ({x}, {y})");
            }
        }
    }
}
