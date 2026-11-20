//! Профиль в окне GPUI: бегущая рамка просит кадры только когда видна, окно на фикстурах
//! говорит с главным процессом как 0.4.9.

use super::neon::{SUBSCRIBER, neon};
use crate::app::LauncherRoot;
use crate::layer::Layers;
use crate::motion::{AnimGate, layer};
use crate::scale::{u, zpx};
use crate::shell::root::ShellContext;
use gpui::{
    Context, IntoElement, Render, TestAppContext, Window, WindowHandle, div, prelude::*, size,
};
use hlsr_ui::fixtures::{FixtureServices, Scene, Variant};
use hlsr_ui::services::Services;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

struct Ring {
    layer: Option<usize>,
}

impl Render for Ring {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let ring = div()
            .relative()
            .size(u(160.))
            .child(neon("neon", SUBSCRIBER, 16., 1.));
        match self.layer {
            Some(depth) => div().child(layer(depth, ring)).into_any_element(),
            None => ring.into_any_element(),
        }
    }
}

fn open(cx: &mut TestAppContext, view: Ring) -> WindowHandle<Ring> {
    let window = cx.open_window(size(zpx(1., 200.), zpx(1., 200.)), move |_, _| view);
    cx.run_until_parked();
    window
}

/// Кадр и число тех, кто его просил.
fn frame(window: &WindowHandle<Ring>, cx: &mut TestAppContext) -> usize {
    cx.executor().advance_clock(Duration::from_millis(16));
    let count = window
        .update(cx, |_, window, cx| window.simulate_next_frame(cx))
        .unwrap();
    cx.run_until_parked();
    count
}

/// После смены ворот кадр, попрошенный до неё, приходит один раз; дальше — ни одного.
fn paused(window: &WindowHandle<Ring>, cx: &mut TestAppContext) -> bool {
    frame(window, cx);
    frame(window, cx) == 0
}

fn gate(cx: &mut TestAppContext, change: impl FnOnce(&mut gpui::App)) {
    cx.update(change);
    cx.run_until_parked();
}

#[gpui::test]
fn the_neon_stops_under_the_anim_gate(cx: &mut TestAppContext) {
    let ring = open(cx, Ring { layer: None });
    assert!(cx.update(|cx| super::neon::moving(cx)));
    assert!(frame(&ring, cx) > 0);
    assert!(frame(&ring, cx) > 0);
    // Окно спрятано — рамка стоит; показано — снова бежит.
    gate(cx, |cx| AnimGate::set_window_hidden(cx, true));
    assert!(paused(&ring, cx));
    gate(cx, |cx| AnimGate::set_window_hidden(cx, false));
    assert!(frame(&ring, cx) > 0);
    // Над ней открыто окно-слой — стоит; внутри верхнего слоя (окно профиля) — бежит.
    gate(cx, |cx| AnimGate::set_top_layer(cx, 1));
    assert!(paused(&ring, cx));
    let inside = open(cx, Ring { layer: Some(1) });
    assert!(frame(&inside, cx) > 0);
    assert!(frame(&inside, cx) > 0);
    gate(cx, |cx| AnimGate::set_top_layer(cx, 2));
    assert!(paused(&inside, cx));
    gate(cx, |cx| AnimGate::set_top_layer(cx, 0));
    // Без анимаций (`.no-anim`) — неподвижна.
    gate(cx, |cx| AnimGate::set_no_anim(cx, true));
    assert!(!cx.update(|cx| super::neon::moving(cx)));
    assert!(paused(&inside, cx));
}

fn own_profile() -> Value {
    json!({
        "status": "signed-in", "userCode": null, "error": null, "ban": null, "mute": null,
        "account": {"signedIn": true, "steamId64": "76561199155902676",
                    "steamId2": "STEAM_0:0:597818474", "personaName": "Kanoyo",
                    "avatarDataUri": null, "connection": "connected"},
        "firstRunAt": "2026-03-14T08:20:00.000Z",
        "achievements": [],
        "summary": {"unlocked": 3, "total": 39, "percent": 8},
        "stats": {"totalSeconds": 7200, "gamesPlayed": 1, "launches": 4, "best": null},
        "streak": {"days": 2, "best": 5},
        "badges": ["developer", "anniversary-2"], "customBadges": [],
        "nick": {"style": "developer", "css": null}, "avatarFrame": null,
        "subscriber": true, "supporterSince": 1760000000,
        "details": {"status": "", "pronouns": "", "country": "", "hideSteamId": false,
                    "pinnedRun": null, "speedrun": null},
        "deletionAt": null,
        "rating": {"score": 3, "likes": 3, "dislikes": 0, "dubious": false},
        "presence": {"online": true, "gameId": null, "launcherId": null},
    })
}

fn fixtures() -> Arc<FixtureServices> {
    fixtures_with(own_profile())
}

fn fixtures_with(profile: Value) -> Arc<FixtureServices> {
    fixtures_invoke(json!({"profile-get": profile, "runner-sync": null}))
}

fn fixtures_invoke(invoke: Value) -> Arc<FixtureServices> {
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

#[gpui::test]
fn the_own_profile_loads_like_0_4_9_and_esc_closes_it(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let fx = fixtures();
    let services: Services = fx.clone();
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.run_until_parked();
    cx.update(|_, cx| {
        let flows = ShellContext::get(cx).expect("оболочка").flows.clone();
        flows.open_runner_profile("");
    });
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    // `load()`: profile-get с undefined (в журнале null), затем runner-sync.
    assert_eq!(calls(&fx, "profile-get"), [json!([null])]);
    assert_eq!(calls(&fx, "runner-sync").len(), 1);
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 1);
    let view = cx.update(|_, cx| {
        let profile = super::ProfileContext::get(cx).expect("окно профиля");
        let tr: hlsr_ui::controllers::bridge::Tr =
            std::rc::Rc::new(|key, args| hlsr_ui::tr(hlsr_core::language::Language::Ru, key, args));
        let state = profile.state.borrow();
        state.view(
            hlsr_core::language::Language::Ru,
            &hlsr_ui::controllers::wall_clock::WallClock::fixed(0, 0),
            &tr,
        )
    });
    assert!(view.subscriber && !view.complete);
    assert_eq!(view.badges.len(), 2);
    assert_eq!(view.stats.len(), 5, "запуски и серия входов");
    // Esc закрывает окно: оболочка больше не держит профиль открытым.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let open = cx.update(|_, cx| {
        ShellContext::get(cx)
            .expect("оболочка")
            .flows
            .with_state(|s| s.profile_open)
    });
    assert!(!open);
}

#[gpui::test]
fn the_achievements_tab_lays_cards_in_two_columns_of_equal_rows(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let item = |id: &str, tier: Value, unlocked: bool, goal: Value| {
        json!({"id": id, "title": "#L_ACH_KING_TITLE", "description": "#L_ACH_KING_DESC",
               "icon": "fas fa-crown", "tier": tier, "unlocked": unlocked,
               "progress": if unlocked { 1.0 } else { 0.4 }, "value": 2, "goal": goal,
               "at": if unlocked { json!("2026-11-08T02:41:00.000Z") } else { Value::Null }})
    };
    let mut profile = own_profile();
    profile["achievements"] = json!([
        item("king-of-the-hill", json!("rainbow"), true, Value::Null),
        item("avid-runner", json!("gold"), true, Value::Null),
        item("warmup", Value::Null, false, json!(5)),
    ]);
    let fx = fixtures_with(profile);
    let services: Services = fx.clone();
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.run_until_parked();
    cx.update(|_, cx| {
        let flows = ShellContext::get(cx).expect("оболочка").flows.clone();
        flows.open_runner_profile("");
    });
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    // Вкладка — щелчком в цель стенда `profile-tabs-1`.
    let tab = crate::snap::bounds_of("profile-tabs-1").expect("вкладка «Достижения»");
    cx.simulate_click(tab.center(), gpui::Modifiers::none());
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }
    let a = cx.debug_bounds("ach-0").expect("карточка 0");
    let b = cx.debug_bounds("ach-1").expect("карточка 1");
    let c = cx.debug_bounds("ach-2").expect("карточка 2");
    assert_eq!(a.top(), b.top());
    assert!(b.left() > a.right());
    assert!(c.top() > a.bottom());
    // Ряды одной высоты: у третьей полоса прогресса, первые — без неё.
    assert_eq!(a.size, c.size);
}

/// Кадры с шагом 16 мс: переходы и прокрутка доходят до конца.
fn settle(cx: &mut gpui::VisualTestContext, frames: usize) {
    for _ in 0..frames {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }
}

/// Прокрутить раздел колесом, пока цель стенда `name` не окажется в его видимой части; её
/// середина.
fn reveal(cx: &mut gpui::VisualTestContext, name: &str) -> gpui::Point<gpui::Pixels> {
    use gpui::{
        Modifiers, PlatformInput, ScrollDelta, ScrollEvent, ScrollSource, TouchPhase, point,
    };
    for _ in 0..60 {
        settle(cx, 2);
        let body = crate::snap::bounds_of("profile-body").expect("прокрутка раздела");
        if let Some(bounds) = crate::snap::bounds_of(name)
            && bounds.top() >= body.top()
            && bounds.bottom() <= body.bottom()
        {
            return bounds.center();
        }
        cx.update(|window, cx| {
            window.dispatch_event(
                PlatformInput::Scroll(ScrollEvent {
                    position: body.center(),
                    delta: ScrollDelta::Lines(point(0., -2.)),
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                    source: ScrollSource::Pointer,
                }),
                cx,
            )
        });
    }
    panic!("цель {name} не показалась");
}

/// Середина отмеченного контрола `id` (`crate::live`) в новом кадре; `focused` — в фокусе ли.
fn control(cx: &mut gpui::VisualTestContext, id: &str) -> (gpui::Point<gpui::Pixels>, bool) {
    // Без dev-hooks кадр таблицы инспектора начинает тест, а не корень окна.
    cx.update(|window, _| {
        crate::live::begin_frame();
        window.refresh();
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let dump = crate::live::snapshot(window, cx);
        let scale = dump["scale"].as_f64().expect("масштаб") as f32;
        let item = dump["controls"]
            .as_array()
            .expect("контролы")
            .iter()
            .find(|item| item["id"] == id)
            .unwrap_or_else(|| panic!("нет контрола {id}"));
        let [x, y, w, h] = [0, 1, 2, 3].map(|i| item["rect"][i].as_f64().unwrap() as f32 / scale);
        let center = gpui::point(zpx(1., x + w / 2.), zpx(1., y + h / 2.));
        (center, item["focused"] == true)
    })
}

/// Вход в профиле: ссылки согласия (`<button>` в `<label>` 0.4.9) открывают свои страницы
/// щелчком и с клавиатуры (остановка Tab), флажок не переключают.
#[gpui::test]
fn the_consent_links_open_pages_and_leave_the_box(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let fx = fixtures_with(json!({"status": "signed-out", "account": {"signedIn": false}}));
    let services: Services = fx.clone();
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.run_until_parked();
    cx.update(|_, cx| {
        let flows = ShellContext::get(cx).expect("оболочка").flows.clone();
        flows.open_runner_profile("");
    });
    settle(cx, 30);
    for id in ["profile-consent-privacy", "profile-consent-terms"] {
        let (at, _) = control(cx, id);
        cx.simulate_click(at, gpui::Modifiers::none());
        settle(cx, 2);
    }
    assert_eq!(
        calls(&fx, "open-external-link"),
        [json!(["privacy"]), json!(["terms"])]
    );
    // Tab окна лаунчера: по кругу внутри верхнего окна (`crate::layer::tab_root`).
    let mut reached = false;
    for _ in 0..20 {
        cx.update(|window, cx| crate::layer::move_focus(true, window, cx));
        settle(cx, 1);
        if control(cx, "profile-consent-terms").1 {
            reached = true;
            break;
        }
    }
    assert!(reached, "ссылка «условия» — остановка Tab");
    cx.simulate_keystrokes("enter");
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").unwrap(),
    });
    settle(cx, 2);
    assert_eq!(calls(&fx, "open-external-link").len(), 3);
    let consent = cx.update(|_, cx| {
        super::ProfileContext::get(cx)
            .expect("окно профиля")
            .state
            .borrow()
            .consent()
    });
    assert!(!consent, "ссылки флажок не переключают");
}

/// Вход в профиле: согласие (`max-width: 420px`) переносится на две строки высотой 40 px, кнопка
/// входа — на 16 px ниже второй строки, а не поверх неё.
#[gpui::test]
fn the_consent_wraps_above_the_sign_in_button(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let fx = fixtures_with(json!({"status": "signed-out", "account": {"signedIn": false}}));
    let services: Services = fx.clone();
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.run_until_parked();
    cx.update(|_, cx| {
        let flows = ShellContext::get(cx).expect("оболочка").flows.clone();
        flows.open_runner_profile("");
    });
    settle(cx, 30);
    // Кнопка идёт следом в потоке (`margin-top: 16px`): высота блока согласия её и ставит.
    let consent = crate::snap::bounds_of("profile-consent").expect("согласие");
    assert_eq!(consent.size.width, zpx(1., 420.));
    assert_eq!(consent.size.height, zpx(1., 40.));
    let button = crate::snap::bounds_of("profile-sign-in").expect("кнопка входа");
    assert_eq!(button.origin.y, consent.bottom() + zpx(1., 16.));
}

/// Свой профиль поддержавшего: право на подсветку, подписка, Telegram и Discord не привязаны.
fn supporter() -> Value {
    let mut profile = own_profile();
    profile["perks"] = json!({"styles": true,
                              "subscriber": {"active": true, "until": "2026-12-01T10:00:00.000Z"}});
    profile["stylePreset"] = Value::Null;
    profile["telegram"] = json!({"linked": false, "username": null});
    profile["discordLink"] = json!({"linked": false, "username": null, "boosterSince": null});
    profile
}

#[gpui::test]
fn the_settings_tab_talks_to_main_like_0_4_9(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let fx = fixtures_invoke(json!({
        "profile-get": supporter(), "runner-sync": null,
        "public-style-presets-get": {"status": "ok", "presets": [
            {"id": 3, "title": "Неон", "nick": {"style": null, "css": "color: #ff0"},
             "frame": {"css": ""}}]},
        "runner-telegram-code": {"status": "ok", "code": "123456", "expiresAt": 4_102_444_800u64},
        "support-open": {"status": "ok"},
        "runner-discord-code": {"status": "ok", "code": "654321", "expiresAt": 4_102_444_800u64},
        "runner-customize": {"status": "error", "code": "RUNNER_PERK_REQUIRED"}
    }));
    let services: Services = fx.clone();
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.run_until_parked();
    cx.update(|_, cx| {
        let flows = ShellContext::get(cx).expect("оболочка").flows.clone();
        flows.open_runner_profile("");
    });
    settle(cx, 4);
    // `created()` раздела: список заготовок — уже при своём профиле, без аргументов.
    assert_eq!(calls(&fx, "public-style-presets-get"), [json!([])]);
    let tab = crate::snap::bounds_of("profile-tabs-2").expect("вкладка «Настройки»");
    cx.simulate_click(tab.center(), gpui::Modifiers::none());
    settle(cx, 40);
    // Заготовка без права на сервере: отказ его текстом, без снимка.
    let at = reveal(cx, "rs-style-option-1");
    cx.simulate_click(at, gpui::Modifiers::none());
    settle(cx, 4);
    assert_eq!(
        calls(&fx, "runner-customize"),
        [json!([{"stylePreset": 3}])]
    );
    // Telegram: код у сервера, затем бот с ним.
    let at = reveal(cx, "rs-telegram-link");
    cx.simulate_click(at, gpui::Modifiers::none());
    settle(cx, 4);
    assert_eq!(calls(&fx, "runner-telegram-code"), [json!([])]);
    assert_eq!(calls(&fx, "support-open"), [json!(["bot", "123456"])]);
    // Discord: код, вместо «Привязать» — «Скопировать команду».
    let at = reveal(cx, "rs-discord-link");
    cx.simulate_click(at, gpui::Modifiers::none());
    settle(cx, 4);
    assert_eq!(calls(&fx, "runner-discord-code"), [json!([])]);
    reveal(cx, "rs-discord-copy");
    // «Выйти везде» — свой канал с undefined (в журнале null).
    let at = reveal(cx, "rs-logout-all");
    cx.simulate_click(at, gpui::Modifiers::none());
    settle(cx, 4);
    assert_eq!(calls(&fx, "runner-logout-all"), [json!([null])]);
}
