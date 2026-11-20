//! Корень оболочки в окне GPUI на фикстурах: что уходит главному процессу при запуске, вкладки,
//! Discord, F11 и полный экран, секретная последовательность, тройной щелчок по логотипу.

use super::root::ShellRoot;
use crate::app::LauncherRoot;
use crate::layer::Layers;
use crate::snap::{bounds_of, pointer_move, press};
use gpui::{Entity, MouseButton, TestAppContext, VisualTestContext, px};
use hlsr_ui::fixtures::{FixtureServices, Scene, Variant};
use hlsr_ui::services::Services;
use serde_json::{Value, json};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

fn fixtures() -> Arc<FixtureServices> {
    let scene: Scene = serde_json::from_value(json!({
        "id": "t", "route": "/launcher", "platform": "win32",
        "window": {"width": 1600, "height": 900},
        "themes": ["hlsr"], "languages": ["ru"], "scales": [1],
        "now": "2026-11-17T12:00:00.000Z", "timezone": "UTC",
        "store": {"settings": {"config": {"language": null}}},
        "invoke": {"window-command": {
            "$args": [
                [["fullscreen-state"], {"ok": true, "fullscreen": false}],
                [["fullscreen"], {"ok": true, "fullscreen": true}]
            ],
            "$else": {"ok": true}
        }}
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

/// Аргументы вызовов `kind` канала `channel` по порядку.
fn calls(fx: &FixtureServices, kind: &str, channel: &str) -> Vec<Value> {
    fx.log()
        .into_iter()
        .filter(|entry| entry["kind"] == kind && entry["channel"] == channel)
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

/// Оболочка на фикстурах и её корень.
fn shell(cx: &mut TestAppContext) -> (Entity<ShellRoot>, &mut VisualTestContext) {
    cx.update(crate::init);
    let services: Services = fixtures();
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.run_until_parked();
    let root = cx.update(|_, cx| ShellRoot::get(cx).expect("оболочка смонтирована"));
    // Переходы появления доиграны (часы теста идут только так).
    for _ in 0..10 {
        frame_after(cx, Duration::from_millis(200));
    }
    (root, cx)
}

/// Кадр окна через `time` часов теста.
fn frame_after(cx: &mut VisualTestContext, time: Duration) {
    cx.executor().advance_clock(time);
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

/// Кэш видов (`crate::cache`): чужая анимация не рисует закэшированные виды заново, а их данные
/// — рисуют, и в том же кадре, где они сменились.
#[gpui::test]
fn cached_views_skip_foreign_frames_and_redraw_on_their_data(cx: &mut TestAppContext) {
    let (root, cx) = shell(cx);
    let (background, views) = root.read_with(cx, |root, _| {
        let frame = &root.frame;
        let views = [
            frame.background.entity_id(),
            frame.glass.entity_id(),
            frame.rail.entity_id(),
            frame.topbar.entity_id(),
            frame.hero.entity_id(),
            frame.download_bar.entity_id(),
        ];
        (frame.background.clone(), views)
    });
    let counts = || views.map(crate::live::renders);
    let start = counts();
    assert!(
        start.iter().all(|&n| n > 0),
        "все виды нарисованы: {start:?}"
    );
    // Что нарисовано заново с `before`: фон, стекло, колонка, шапка, герой, панель.
    let redrawn = |before: [usize; 6]| -> [bool; 6] {
        let now = counts();
        std::array::from_fn(|index| now[index] > before[index])
    };

    // Фон движется (шаг параллакса): нарисованы он и слой стекла (он — не в кэше).
    background.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert_eq!(redrawn(start), [true, true, false, false, false, false]);

    // Кадр под стеклом сдвинулся (параллакс): слой стекла читает его заново с кадром движения
    // окна, колонка остаётся в кэше.
    let pixels = Arc::new(hlsr_ui::Rgba8Image::new(4, 4));
    let backdrop = |x: f32| crate::glass::Backdrop {
        pixels: pixels.clone(),
        frame: hlsr_ui::render::rail_glass::Placement {
            x,
            y: 0.,
            width: 4.,
            height: 4.,
        },
        shade: gpui::rgb(0x05080b),
        view: (4., 4.),
    };
    let tell = |x: f32, cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            background.update(cx, |bg, _| bg.tell_glass(Some(backdrop(x)), window))
        });
        frame_after(cx, Duration::from_millis(34));
    };
    let before = counts();
    tell(0., cx);
    assert_eq!(redrawn(before), [false, true, false, false, false, false]);
    // Сдвиг дальше — то же: закэшированные виды на месте (слой стекла рисуется с каждым кадром
    // корня, он не в кэше).
    let before = counts();
    tell(1., cx);
    assert_eq!(redrawn(before)[2..], [false; 4]);

    // Данные оболочки (другая игра): колонка, шапка, герой и нижняя панель — заново, фон — тоже
    // (новый кадр игры).
    let before = counts();
    let nav = root.read_with(cx, |root, _| root.nav().clone());
    let other = (nav.games().into_iter())
        .map(|game| game.id)
        .find(|id| *id != nav.selected_game())
        .expect("в фикстурах больше одной игры");
    nav.select_game(&other);
    cx.run_until_parked();
    assert_eq!(redrawn(before), [true; 6]);
    assert_eq!(nav.selected_game(), other);
}

/// Окна-диалоги и «Персонализация» в кэше видов: шаг фона их не рисует, данные оболочки — рисуют.
#[gpui::test]
fn windows_and_configs_page_skip_foreign_frames_and_redraw_on_their_data(cx: &mut TestAppContext) {
    let (root, cx) = shell(cx);
    let nav = root.read_with(cx, |root, _| root.nav().clone());
    nav.select_section("configs");
    for _ in 0..5 {
        frame_after(cx, Duration::from_millis(200));
    }
    let (background, views) = root.read_with(cx, |root, _| {
        let frame = &root.frame;
        let page = frame
            .configs
            .as_ref()
            .expect("страница открыта")
            .entity_id();
        let mut views = vec![page];
        views.extend(frame.windows.iter().map(gpui::AnyView::entity_id));
        (frame.background.clone(), views)
    });
    let counts = || {
        views
            .iter()
            .map(|id| crate::live::renders(*id))
            .collect::<Vec<_>>()
    };
    let start = counts();
    assert!(start.iter().all(|&n| n > 0), "все нарисованы: {start:?}");

    // Шаг параллакса: фон — заново, страница и окна — повтор.
    background.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert_eq!(counts(), start);

    // Данные оболочки (перемены контроллеров приходят уведомлением корня, `ShellRoot::sync`):
    // страница и все окна — заново.
    nav.set_online(&json!({"players": 7}));
    cx.run_until_parked();
    assert_eq!(nav.section(), "configs");
    let now = counts();
    assert!(
        now.iter().zip(&start).all(|(now, start)| now > start),
        "{start:?} → {now:?}"
    );
}

/// «Демо» в кэше видов: шаг фона страницу не рисует, её данные и данные оболочки — рисуют.
#[gpui::test]
fn demo_page_skips_foreign_frames_and_redraws_on_its_data(cx: &mut TestAppContext) {
    let (root, cx) = shell(cx);
    let nav = root.read_with(cx, |root, _| root.nav().clone());
    nav.select_demo();
    for _ in 0..5 {
        frame_after(cx, Duration::from_millis(200));
    }
    let (background, page) = root.read_with(cx, |root, _| {
        let frame = &root.frame;
        (
            frame.background.clone(),
            frame.demo.clone().expect("страница открыта"),
        )
    });
    let drawn = || crate::live::renders(page.entity_id());
    let start = drawn();
    assert!(start > 0, "страница нарисована");

    background.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert_eq!(drawn(), start, "шаг параллакса — повтор страницы");

    page.update(cx, |page, cx| page.close_export(cx));
    cx.run_until_parked();
    let own = drawn();
    assert!(own > start, "свои данные — заново");

    nav.set_online(&json!({"players": 7}));
    cx.run_until_parked();
    assert!(drawn() > own, "уведомление корня дошло до страницы");
}

/// Слой стекла берёт кадр под колонкой у фона сам (`RailGlass::render` → `Background::backdrop`):
/// после кадра окна у него тот же кадр, место и затемнение, что у фона.
#[gpui::test]
fn rail_glass_takes_the_background_frame(cx: &mut TestAppContext) {
    // Картинки фона — загрузчиком на ручной среде: ответы его потоков забирает тест
    // (`ManualEnv::run`), а не планировщик gpui из чужого потока.
    let env = hlsr_ui::env::ManualEnv::new();
    let loader = hlsr_ui::images::ImageLoader::new(Rc::new(hlsr_ui::images::NoRemote), &*env);
    cx.update(|cx| cx.set_global(crate::image::Pictures::new(loader)));
    let (root, cx) = shell(cx);
    let (background, glass) = root.read_with(cx, |root, _| {
        (root.frame.background.clone(), root.frame.glass.clone())
    });
    let backdrop = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            let shade = cx
                .global::<crate::theme::Theme>()
                .color("l-shade-rgb")
                .into();
            background
                .read(cx)
                .backdrop(crate::scale::viewport(window), shade)
        })
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while backdrop(cx).is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        env.run();
        frame_after(cx, Duration::from_millis(20));
    }
    let expected = backdrop(cx).expect("кадр игры загружен");
    frame_after(cx, Duration::from_millis(34));
    let taken = glass.read_with(cx, |glass, _| glass.backdrop().cloned());
    assert!(taken.is_some_and(|taken| taken.same(&expected)));
}

/// Без анимаций (`.no-anim`) колонка выдвигается за один кадр — и место под ней в строке
/// оболочки тоже: закэшированная «Главная» сдвигается вместе с ней.
#[gpui::test]
fn rail_slot_follows_the_rail_in_one_frame(cx: &mut TestAppContext) {
    let (root, cx) = shell(cx);
    cx.update(|_, cx| crate::motion::AnimGate::set_no_anim(cx, true));
    click(cx, "topbar-tab-home");
    let left = |cx: &mut VisualTestContext| {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        bounds_of("home-refresh")
            .expect("кнопка «Обновить»")
            .origin
            .x
    };
    let closed = left(cx);
    let nav = root.read_with(cx, |root, _| root.nav().clone());
    nav.toggle_rail();
    cx.run_until_parked();
    // Без обновления окна целиком: кадр после переключения уже со сдвигом.
    let open = bounds_of("home-refresh")
        .expect("кнопка «Обновить»")
        .origin
        .x;
    let shift = cx.update(|window, cx| {
        let theme = cx.global::<crate::theme::Theme>();
        crate::scale::zpx(
            crate::scale::zoom(window),
            theme.len("l-rail-open-w") - theme.len("l-rail-w"),
        )
    });
    assert!(shift > px(0.));
    assert_eq!(open - closed, shift);
    assert_eq!(left(cx), open);
}

/// recheckA-3, «пауза ~60 мс перед сворачиванием колонки»: переключатель срабатывает на отпускании
/// кнопки мыши (`@click` 0.4.9), а `live.py` держит кнопку 60 мс (`INPUT_SLEEP` между
/// WM_LBUTTONDOWN и WM_LBUTTONUP). Пока кнопка нажата, колонка стоит; с первого же кадра окна
/// после отпускания — едет.
#[gpui::test]
fn rail_collapses_on_release_from_the_next_frame(cx: &mut TestAppContext) {
    let (root, cx) = shell(cx);
    // Монитор 120 Гц, как DISPLAY2 серии recheckA (у тестового экрана — 60 Гц). Будильник окна —
    // кадр окна: каждый второй vblank (предел 60 к/с, `gpui::frame_divisor`, поток vsync Windows).
    let display = cx.update(|window, cx| window.display(cx).expect("экран").id());
    cx.simulate_display_refresh_interval_change(display, Some(Duration::from_secs(1) / 120));
    let wake = Duration::from_secs(1) / 120 * gpui::frame_divisor(Duration::from_secs(1) / 120);
    click(cx, "topbar-tab-home");
    let nav = root.read_with(cx, |root, _| root.nav().clone());
    nav.toggle_rail();
    for _ in 0..5 {
        frame_after(cx, Duration::from_millis(200));
    }
    let left = || {
        bounds_of("home-refresh")
            .expect("кнопка «Обновить»")
            .origin
            .x
    };
    let open = left();
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let at = bounds_of("rail-toggle").expect("переключатель").center();
    cx.update(|window, cx| {
        pointer_move(window, cx, at);
        press(window, cx, at, MouseButton::Left, 1, true);
    });
    for _ in 0..4 {
        frame_after(cx, wake);
    }
    assert!(nav.rail_open(), "нажатие ещё не щелчок");
    assert_eq!(left(), open);
    cx.update(|window, cx| press(window, cx, at, MouseButton::Left, 1, false));
    cx.run_until_parked();
    assert!(!nav.rail_open());
    frame_after(cx, wake);
    let first = left();
    assert!(
        first < open,
        "колонка поехала в первом кадре окна: {first:?} / {open:?}"
    );
}

/// Колонка выдвигается и сворачивается с кадрами окна (монитор 120 Гц, предел 60 к/с), хотя
/// будильник окна — 280 Гц (DWM по самому быстрому монитору): страница под ней меряет себя каждый
/// кадр, и «ещё кадр после раскладки» гнал окно с каждым будильником.
#[gpui::test]
fn rail_swipe_follows_the_window_monitor(cx: &mut TestAppContext) {
    let (root, cx) = shell(cx);
    let display = cx.update(|window, cx| window.display(cx).expect("экран").id());
    cx.simulate_display_refresh_interval_change(display, Some(Duration::from_secs(1) / 120));
    click(cx, "topbar-tab-home");
    for _ in 0..5 {
        frame_after(cx, Duration::from_millis(200));
    }
    let (nav, glass) = root.read_with(cx, |root, _| (root.nav().clone(), root.frame.glass.clone()));
    // Слой стекла не в кэше: рисуется в каждом кадре корня, то есть окна.
    let frames = || crate::live::renders(glass.entity_id());
    for _ in 0..2 {
        nav.toggle_rail();
        cx.run_until_parked();
        let before = frames();
        // 0,7 с будильников 280 Гц: выдвижение .5 с и подписи.
        for _ in 0..196 {
            frame_after(cx, Duration::from_micros(3_571));
        }
        let drawn = frames() - before;
        assert!(drawn > 20, "колонка движется: {drawn} кадров");
        assert!(drawn <= 45, "{drawn} кадров за 0,7 с при пределе 60 к/с");
    }
}

#[gpui::test]
fn shell_root_talks_to_the_main_process_like_0_4_9(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let fx = fixtures();
    let services: Services = fx.clone();
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.run_until_parked();

    // mounted(): готовность, Discord сразу, «Спасибо» ждёт окна, состояние полного экрана.
    assert_eq!(calls(&fx, "send", "ready").len(), 1);
    assert_eq!(
        calls(&fx, "send", "discord-rpc-activity"),
        [json!(["games"])]
    );
    assert_eq!(calls(&fx, "send", "support-thanks-ready").len(), 1);
    assert_eq!(
        calls(&fx, "invoke", "window-command"),
        [json!(["fullscreen-state"])]
    );
    assert_eq!(
        calls(&fx, "invoke", "game-delivery-selected"),
        [json!(["218"])]
    );

    // Вкладка «Главная» → Discord «home»; повторный щелчок по ней Discord не трогает.
    click(cx, "topbar-tab-home");
    click(cx, "topbar-tab-home");
    click(cx, "topbar-tab-configs");
    assert_eq!(
        calls(&fx, "send", "discord-rpc-activity"),
        [
            json!(["games"]),
            json!(["home"]),
            json!(["personalization"])
        ]
    );

    // F11: окно гаснет, команда, полный экран — у подложек окон углов нет.
    cx.simulate_keystrokes("f11");
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
    assert_eq!(
        calls(&fx, "invoke", "window-command").last(),
        Some(&json!(["fullscreen"]))
    );
    assert_eq!(cx.update(|_, cx| Layers::radius(cx)), px(0.));
    // Система вывела окно из полного экрана — углы вернулись.
    fx.emit("launcher-fullscreen-changed", json!(false));
    cx.run_until_parked();
    assert_ne!(cx.update(|_, cx| Layers::radius(cx)), px(0.));

    // «Вверх, вверх, вниз, вниз, влево, вправо, влево, вправо, B, A» — «Проснись».
    cx.simulate_keystrokes("up up down down left right left right b a");
    cx.run_until_parked();
    assert_eq!(
        calls(&fx, "invoke", "runner-secret-unlock"),
        [json!(["wake-up"])]
    );

    // Три щелчка по логотипу подряд — «Назад в прошлое!».
    for _ in 0..3 {
        click(cx, "topbar-logo");
    }
    assert_eq!(
        calls(&fx, "invoke", "runner-secret-unlock").last(),
        Some(&json!(["back-to-past"]))
    );
}
