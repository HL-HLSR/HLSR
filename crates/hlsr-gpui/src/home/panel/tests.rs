//! «Главная» в окне лаунчера на фикстурах: что уходит главному процессу при входе и на
//! «Обновить», реакция гостя, просмотр картинок в стопке окон (открыть плиткой, листать
//! стрелками, закрыть Esc), окно видимости длинного списка рекордов.

use crate::app::LauncherRoot;
use crate::layer::Layers;
use crate::shell::root::ShellContext;
use crate::snap::{bounds_of, forget_targets, pointer_move, press};
use gpui::{MouseButton, ScrollDelta, ScrollEvent, TestAppContext, VisualTestContext, point};
use hlsr_ui::fixtures::{FixtureServices, Scene, Variant};
use hlsr_ui::services::Services;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

fn fixtures() -> Arc<FixtureServices> {
    let image = |n: u32| json!({"url": format!("https://api.hlsr.pro/news/images/{n}.png"), "width": 480, "height": 270});
    fixtures_with(vec![json!({
        "kind": "post", "id": "41", "title": "HLSR 0.4.9",
        "body": "Текст **жирный** и [ссылка](https://hlsr.pro).\n\n- один\n- два",
        "author": "HLSR",
        "authorProfile": {"steamId64": "76561199155902676", "personaName": "Kanoyo",
                          "avatarUrl": "", "nick": {"style": "developer", "css": null},
                          "avatarFrame": null, "subscriber": true},
        "images": [image(1), image(2), image(3)],
        "reactions": {"like": 12, "love": 4, "fire": 7, "wow": 0, "celebrate": 2},
        "date": "2026-10-14T09:30:00.000Z"
    })])
}

fn fixtures_with(items: Vec<Value>) -> Arc<FixtureServices> {
    let scene: Scene = serde_json::from_value(json!({
        "id": "t", "route": "/launcher", "platform": "win32",
        "window": {"width": 1600, "height": 900},
        "themes": ["hlsr"], "languages": ["ru"], "scales": [1],
        "now": "2026-11-17T12:00:00.000Z", "timezone": "UTC",
        "store": {"settings": {"config": {"language": 1}}},
        "invoke": {
            "get-library-path": {"path": "C:\\HLSR", "confirmed": true},
            "runner-state": {"status": "signed-out"},
            "public-news-get": {"status": "ok", "items": items}
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
fn home_feed_talks_to_the_main_process_like_0_4_9(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let fx = fixtures();
    let services: Services = fx.clone();
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.run_until_parked();
    // Гостю при запуске открылось приветствие — «Войти позже».
    cx.update(|_, cx| ShellContext::get(cx).unwrap().flows.on_welcome_later());
    settle(cx);
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 0);
    let before = calls(&fx, "runner-state").len();

    // mounted(): состояние входа, затем лента без force; гостю «мои реакции» не нужны.
    click(cx, "topbar-tab-home");
    assert_eq!(calls(&fx, "runner-state").len(), before + 1);
    assert_eq!(calls(&fx, "public-news-get"), [json!([{"force": false}])]);
    assert!(calls(&fx, "public-news-mine").is_empty());

    // Гость жмёт реакцию — запроса нет (замечание «Войдите»).
    click(cx, "home-react-0-0");
    assert!(calls(&fx, "public-news-react").is_empty());

    // Вторая плитка — просмотр в стопке окон; стрелки листают внутри окна, Esc закрывает.
    click(cx, "home-image-0-1");
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 1);
    cx.simulate_keystrokes("right left right");
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 1);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| Layers::open_count(cx)), 0);

    // «Обновить» — лента заново с force.
    click(cx, "home-refresh");
    assert_eq!(
        calls(&fx, "public-news-get"),
        [json!([{"force": false}]), json!([{"force": true}])]
    );
}

/// Верх построенных карточек рекордов `0..n` в свежем кадре (`None` — карточки в кадре нет).
/// Кадр перед ним — на случай, если замер прошлого попросил ещё один (появилась полоса
/// прокрутки: ширина строк сменилась).
fn record_tops(cx: &mut VisualTestContext, n: usize) -> Vec<Option<f32>> {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    forget_targets();
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    (0..n)
        .map(|index| bounds_of(&format!("home-record-{index}")).map(|b| f32::from(b.top())))
        .collect()
}

/// Длинный список рекордов: строятся только карточки у видимой области, а карточки после
/// прокрутки стоят там же, где стояли бы без окна видимости (распорки — точной высоты).
#[gpui::test]
fn records_build_only_cards_near_the_viewport(cx: &mut TestAppContext) {
    cx.update(crate::init);
    // Без движения: колесо сдвигает сразу (системный reduced-motion — через ворота: они
    // перезаписывают флаг приложения).
    cx.update(|cx| crate::motion::AnimGate::set_reduce_motion(cx, true));
    const N: usize = 40;
    let runs = (0..N)
        .map(|n| {
            json!({"kind": "run", "id": format!("r{n}"), "game": "Half-Life", "gameKey": "hl1",
                   "category": format!("Any% {n}"), "subcategory": "", "runners": ["Zed"],
                   "seconds": 1600 + n, "date": "2026-11-15T18:30:00.000Z",
                   "url": format!("https://www.speedrun.com/hl1/runs/r{n}"),
                   "place": null, "profiles": null})
        })
        .collect();
    let services: Services = fixtures_with(runs);
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.run_until_parked();
    cx.update(|_, cx| ShellContext::get(cx).unwrap().flows.on_welcome_later());
    click(cx, "topbar-tab-home");
    click(cx, "home-tabs-1");
    settle(cx);

    let tops = record_tops(cx, N);
    let (y0, y1) = (
        tops[0].expect("первая видна"),
        tops[1].expect("вторая видна"),
    );
    let pitch = y1 - y0;
    assert!(tops[N - 1].is_none(), "последняя за краем не строится");

    // Десять щелчков колеса вниз (без плавности): сдвиг ровно на них.
    let zoom = cx.update(|window, _| crate::scale::zoom(window));
    let at = bounds_of("home-record-0").unwrap().center();
    cx.update(|window, cx| pointer_move(window, cx, at));
    for _ in 0..10 {
        cx.simulate_event(ScrollEvent {
            position: at,
            delta: ScrollDelta::Lines(point(0., -3.)),
            ..Default::default()
        });
    }
    let shift = 10. * 3. * crate::scroll::line_step() * zoom;
    let tops = record_tops(cx, N);
    assert!(
        tops[0].is_none(),
        "первая ушла за край и не строится: {tops:?}, y0 {y0}, сдвиг {shift}"
    );
    let built: Vec<usize> = (0..N).filter(|&index| tops[index].is_some()).collect();
    assert!(built.len() < N / 2, "строятся {built:?}");
    for index in built {
        let expected = y0 - shift + index as f32 * pitch;
        let top = tops[index].unwrap();
        assert!((top - expected).abs() < 0.5, "{index}: {top} ≠ {expected}");
    }

    // Обратно вверх — первая снова на своём месте.
    for _ in 0..10 {
        cx.simulate_event(ScrollEvent {
            position: at,
            delta: ScrollDelta::Lines(point(0., 3.)),
            ..Default::default()
        });
    }
    let tops = record_tops(cx, N);
    assert_eq!(tops[0], Some(y0));
}
