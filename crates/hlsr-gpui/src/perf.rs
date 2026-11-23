//! Профиль кадров лаунчера (замер, не проверка): каждый экран в покое, переходы, движение мыши,
//! длинные списки при прокрутке колесом, обе темы. Окно GPUI теста строит, раскладывает и рисует
//! кадр так же, как настоящее (без видеокарты), поэтому время кадра здесь — его цена на ЦП в
//! потоке окна. Кадр монитора 120 Гц — 8,3 мс на всё.
//!
//! `cargo test --release -p hlsr-gpui --lib perf:: -- --ignored --nocapture --test-threads=1`
//! (`HLSR_PERF_N` — длина списков, по умолчанию 1000).

use crate::app::LauncherRoot;
use crate::configs::sections::GameSettings;
use crate::shell::root::{ShellContext, ShellRoot};
use gpui::{
    Context, Entity, IntoElement, Modifiers, MouseMoveEvent, Pixels, Point, Render, ScrollDelta,
    ScrollEvent, TestAppContext, VisualTestContext, WeakEntity, Window, div, point, prelude::*, px,
    size,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::config_chips::ConfigGame;
use hlsr_ui::fixtures::{FixtureServices, Scene, Variant, screens_049_dir};
use hlsr_ui::services::Services;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Шаг часов на кадр: кадр монитора 60 Гц тестового экрана.
const FRAME: Duration = Duration::from_millis(17);

fn list_len() -> usize {
    std::env::var("HLSR_PERF_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000)
}

fn variant(theme: &str) -> Variant {
    Variant {
        theme: theme.into(),
        lang: "ru".into(),
        scale: 1.0,
        zoom: None,
        rail: None,
    }
}

/// Кадр через [`FRAME`]: часы, кадр монитора, всё, что он разбудил (раскладка и отрисовка).
fn frame(cx: &mut VisualTestContext) -> Duration {
    let start = Instant::now();
    cx.executor().advance_clock(FRAME);
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    start.elapsed()
}

fn frames(cx: &mut VisualTestContext, n: usize) -> Vec<Duration> {
    (0..n).map(|_| frame(cx)).collect()
}

/// Ввод и кадр после него — одна цена.
fn input_frame(cx: &mut VisualTestContext, input: impl gpui::InputEvent) -> Duration {
    let start = Instant::now();
    cx.simulate_event(input);
    start.elapsed() + frame(cx)
}

/// Кадры прокрутки: `notches` щелчков колеса вниз, после каждого — анимация до конца.
fn scroll(cx: &mut VisualTestContext, at: Point<Pixels>, notches: usize) -> Vec<Duration> {
    let mut out = Vec::new();
    for _ in 0..notches {
        out.push(input_frame(
            cx,
            ScrollEvent {
                position: at,
                delta: ScrollDelta::Lines(point(0., -3.)),
                ..Default::default()
            },
        ));
        out.extend(frames(cx, 11));
    }
    out
}

/// Мышь ведут по прямой от `from` до `to` за `steps` кадров (наведение, параллакс).
fn sweep(
    cx: &mut VisualTestContext,
    from: Point<Pixels>,
    to: Point<Pixels>,
    steps: usize,
) -> Vec<Duration> {
    (0..steps)
        .map(|i| {
            let t = i as f32 / (steps - 1).max(1) as f32;
            let at = point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
            input_frame(
                cx,
                MouseMoveEvent {
                    position: at,
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                },
            )
        })
        .collect()
}

fn report(name: &str, frames: &[Duration]) {
    let mut ms: Vec<f64> = frames.iter().map(|d| d.as_secs_f64() * 1e3).collect();
    ms.sort_by(f64::total_cmp);
    let mean = ms.iter().sum::<f64>() / ms.len().max(1) as f64;
    let p95 = ms[(ms.len() * 95 / 100).min(ms.len() - 1)];
    let over = ms.iter().filter(|v| **v > 8.33).count();
    println!(
        "PERF {name:<40} кадров {:>4}  среднее {mean:>7.2}  p95 {p95:>7.2}  макс {:>7.2} мс  >8,3 мс: {over}",
        ms.len(),
        ms.last().copied().unwrap_or_default()
    );
}

// ---- Окно настроек игры ------------------------------------------------------------------------

struct Host(Option<Entity<GameSettings>>);

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::theme::frame(window, cx);
        div().size_full().flex().children(self.0.clone())
    }
}

/// Окно настроек Ghosting (`218`) на разделе `section`; ответы `invoke` подменены.
fn settings<'a>(
    cx: &'a mut TestAppContext,
    section: &str,
    invoke: &[(&str, Value)],
) -> (Entity<GameSettings>, &'a mut VisualTestContext) {
    cx.update(crate::init);
    let mut scene = Scene::load(&screens_049_dir(), "configs/kb-main").expect("сцена");
    for (channel, reply) in invoke {
        scene.invoke.insert((*channel).into(), reply.clone());
    }
    let services: Services =
        Arc::new(FixtureServices::new(&scene, &variant("hlsr")).expect("фикстура"));
    let game = ConfigGame {
        id: "218".into(),
        name: "HL2".into(),
        title: "Half-Life 2".into(),
    };
    let (host, cx) = cx.add_window_view(|_, _| Host(None));
    cx.simulate_resize(size(px(1100.), px(760.)));
    host.update_in(cx, |host, window, cx| {
        let page = WeakEntity::new_invalid();
        host.0 = Some(cx.new(|cx| {
            GameSettings::new(&services, &game, section, Language::Ru, page, window, cx)
        }));
        cx.notify();
    });
    for _ in 0..20 {
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    let settings = host.read_with(cx, |host, _| host.0.clone().expect("окно"));
    (settings, cx)
}

fn demos(n: usize) -> Value {
    let items: Vec<Value> = (0..n)
        .map(|i| {
            json!({"dir": "valve", "name": format!("run_{i:05}_segment_long_name.dem"),
                   "size": 1_048_576 + i * 977, "mtimeMs": 1_790_000_000_000u64 + i as u64 * 60_000})
        })
        .collect();
    json!({"status": "ok", "demos": items})
}

fn runs(n: usize) -> Value {
    let items: Vec<Value> = (0..n)
        .map(|i| {
            let name = format!("ghost_{i:05}_d1_trainstation_01.run");
            json!({"id": name, "name": name, "size": 20_480 + i * 31,
                   "modifiedAt": "2026-10-01T10:00:00.000Z"})
        })
        .collect();
    json!({"status": "ok", "items": items})
}

/// Точка над телом раздела (правее списка разделов).
fn body_point(cx: &mut VisualTestContext) -> Point<Pixels> {
    let size = cx.update(|window, _| window.viewport_size());
    point(size.width * 0.65, size.height * 0.55)
}

#[gpui::test]
#[ignore]
fn demos_list(cx: &mut TestAppContext) {
    let n = list_len();
    let (_, cx) = settings(cx, "demos", &[("game-demos-list", demos(n))]);
    let at = body_point(cx);
    report(&format!("демо {n}: покой"), &frames(cx, 30));
    report(&format!("демо {n}: прокрутка"), &scroll(cx, at, 8));
    let size = cx.update(|window, _| window.viewport_size());
    report(
        &format!("демо {n}: мышь по списку"),
        &sweep(
            cx,
            point(size.width * 0.6, size.height * 0.2),
            point(size.width * 0.6, size.height * 0.9),
            40,
        ),
    );
}

#[gpui::test]
#[ignore]
fn ghosts_list(cx: &mut TestAppContext) {
    let n = list_len();
    let (_, cx) = settings(cx, "ghosts", &[("game-runs-list", runs(n))]);
    let at = body_point(cx);
    report(&format!("призраки {n}: прокрутка"), &scroll(cx, at, 8));
    let size = cx.update(|window, _| window.viewport_size());
    report(
        &format!("призраки {n}: мышь по списку"),
        &sweep(
            cx,
            point(size.width * 0.6, size.height * 0.2),
            point(size.width * 0.6, size.height * 0.9),
            40,
        ),
    );
}

// ---- Страница «Демо» ---------------------------------------------------------------------------

struct DemoHost(Option<Entity<crate::demo::panel::DemoPanel>>);

impl Render for DemoHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::theme::frame(window, cx);
        div().size_full().flex().children(self.0.clone())
    }
}

#[gpui::test]
#[ignore]
fn demo_page_list(cx: &mut TestAppContext) {
    let n = list_len();
    cx.update(crate::init);
    let mut scene = Scene::load(&screens_049_dir(), "configs/kb-main").expect("сцена");
    let items: Vec<Value> = (0..n)
        .map(|i| {
            json!({"id": format!("d{i}"), "name": format!("run_{i:05}_bhop_long_name"),
                   "gameId": if i % 3 == 0 { "ghosting-renewed" } else { "hl1-won" },
                   "game": "hl1", "map": "c1a0", "duration": 120.5 + i as f64,
                   "supported": i % 7 != 0, "reason": "UNSUPPORTED"})
        })
        .collect();
    scene
        .invoke
        .insert("demo-list".into(), json!({"status": "ok", "items": items}));
    let services: Services =
        Arc::new(FixtureServices::new(&scene, &variant("hlsr")).expect("фикстура"));
    let (host, cx) = cx.add_window_view(|_, _| DemoHost(None));
    cx.simulate_resize(size(px(1500.), px(820.)));
    host.update_in(cx, |host, _, cx| {
        host.0 = Some(cx.new(|cx| crate::demo::panel::DemoPanel::new(&services, cx)));
        cx.notify();
    });
    for _ in 0..20 {
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    let at = point(px(160.), px(450.));
    report(
        &format!("страница «Демо» {n}: прокрутка"),
        &scroll(cx, at, 8),
    );
    report(
        &format!("страница «Демо» {n}: мышь по списку"),
        &sweep(cx, point(px(160.), px(150.)), point(px(160.), px(780.)), 40),
    );
}

// ---- Оболочка ----------------------------------------------------------------------------------

fn shell_fixtures(theme: &str) -> Services {
    let scene: Scene = serde_json::from_value(json!({
        "id": "perf", "route": "/launcher", "platform": "win32",
        "window": {"width": 1600, "height": 900},
        "themes": [theme], "languages": ["ru"], "scales": [1], "parallax": true,
        "now": "2026-11-17T12:00:00.000Z", "timezone": "UTC",
        "store": {"settings": {"config": {"language": null}}},
        "invoke": {"window-command": {"$else": {"ok": true}}}
    }))
    .unwrap();
    Arc::new(FixtureServices::new(&scene, &variant(theme)).unwrap())
}

fn shell<'a>(
    cx: &'a mut TestAppContext,
    theme: &str,
) -> (Entity<ShellRoot>, &'a mut VisualTestContext) {
    cx.update(crate::init);
    let services = shell_fixtures(theme);
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.simulate_resize(size(px(1600.), px(900.)));
    for _ in 0..20 {
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    let root = cx.update(|_, cx| ShellRoot::get(cx).expect("оболочка"));
    (root, cx)
}

fn flows(cx: &mut VisualTestContext) -> hlsr_ui::controllers::shell_flows::ShellFlows {
    cx.update(|_, cx| ShellContext::get(cx).expect("оболочка").flows.clone())
}

/// Все вкладки, колонка и мышь по окну в теме `theme`.
fn tour(cx: &mut TestAppContext, theme: &str) {
    let (root, cx) = shell(cx, theme);
    let nav = root.read_with(cx, |root, _| root.nav().clone());
    let t = |name: &str| format!("[{theme}] {name}");
    report(&t("игры: покой"), &frames(cx, 60));
    report(
        &t("игры: мышь по окну (параллакс)"),
        &sweep(
            cx,
            point(px(100.), px(150.)),
            point(px(1500.), px(800.)),
            60,
        ),
    );
    nav.toggle_rail();
    report(&t("игры: колонка"), &frames(cx, 40));
    nav.toggle_rail();
    frames(cx, 40);
    for section in ["home", "configs", "games"] {
        nav.select_section(section);
        report(&t(&format!("свайп в «{section}»")), &frames(cx, 40));
        report(&t(&format!("{section}: покой")), &frames(cx, 60));
    }
    nav.select_section("home");
    frames(cx, 40);
    report(
        &t("главная: мышь по ленте"),
        &sweep(
            cx,
            point(px(400.), px(200.)),
            point(px(1000.), px(850.)),
            60,
        ),
    );
    report(
        &t("главная: лента, прокрутка"),
        &scroll(cx, point(px(700.), px(600.)), 6),
    );
    report(
        &t("главная: рекорды, прокрутка"),
        &scroll(cx, point(px(1350.), px(450.)), 6),
    );
    nav.select_section("games");
    frames(cx, 40);

    let flows = flows(cx);
    flows.on_tool("settings");
    report(&t("настройки: открытие"), &frames(cx, 30));
    report(&t("настройки: покой"), &frames(cx, 30));
    report(
        &t("настройки: прокрутка"),
        &scroll(cx, point(px(900.), px(500.)), 6),
    );
    flows.close_settings();
    frames(cx, 30);

    flows.on_tool("changelog");
    report(&t("журнал изменений: открытие"), &frames(cx, 30));
    report(
        &t("журнал изменений: прокрутка"),
        &scroll(cx, point(px(800.), px(500.)), 10),
    );
    flows.close_changelog();
    frames(cx, 30);

    flows.open_game_settings();
    report(&t("настройки игры: открытие"), &frames(cx, 30));
    report(&t("настройки игры: покой"), &frames(cx, 30));
}

#[gpui::test]
#[ignore]
fn launcher_hlsr(cx: &mut TestAppContext) {
    tour(cx, "hlsr");
}

#[gpui::test]
#[ignore]
fn launcher_liquid_glass(cx: &mut TestAppContext) {
    tour(cx, "liquid-glass");
}
