//! Параллакс фона: модель `hlsr_ui::parallax` против `LauncherBackground.vue` 0.4.7 (скрипт
//! выполнен в Node, `tools/reference/ui/parallax.js` → `tests/fixtures/parallax.json`): на каждом
//! кадре и каждом событии те же переменные `--l-bg-par-*`/`--l-bg-front-*`, то же «кадр запрошен».

use hlsr_ui::parallax::{self, Parallax};
use serde_json::Value;

const BASE: f64 = 10_000.0;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/parallax.json")).expect("parallax.json")
}

fn pair(value: &Value) -> Option<(f64, f64)> {
    let items = value.as_array()?;
    let parse = |item: &Value| {
        item.as_str()
            .map(|text| text.trim_end_matches("px").parse::<f64>().unwrap())
    };
    Some((parse(&items[0])?, parse(&items[1])?))
}

fn apply(model: &mut Parallax, event: &Value) {
    let numbers = |key: &str| {
        let items = event[key].as_array().unwrap();
        (items[0].as_f64().unwrap(), items[1].as_f64().unwrap())
    };
    if event.get("move").is_some() {
        let (x, y) = numbers("move");
        model.pointer_moved(x, y);
    } else if event.get("leave").is_some() {
        model.pointer_left();
    } else if event.get("blur").is_some() {
        model.focus(false);
    } else if event.get("focus").is_some() {
        model.focus(true);
    } else if let Some(paused) = event.get("paused") {
        model.set_paused(paused.as_bool().unwrap());
    } else if let Some(visible) = event.get("visible") {
        model.set_visible(visible.as_bool().unwrap());
    } else if let Some(on) = event.get("parallax") {
        model.set_parallax(on.as_bool().unwrap());
    } else if event.get("resize").is_some() {
        let (w, h) = numbers("resize");
        model.resized(w, h);
    } else if let Some(hidden) = event.get("hidden") {
        model.set_document_hidden(hidden.as_bool().unwrap());
    } else {
        panic!("событие {event}");
    }
}

fn check(scenario: &str, index: usize, model: &Parallax, expected: &Value) {
    let vars = model.vars();
    let what = format!("{scenario} #{index} {}", expected["kind"]);
    assert_eq!(
        model.frame_pending(),
        expected["pending"].as_bool().unwrap(),
        "{what}: кадр запрошен"
    );
    assert_eq!(
        vars.zoom,
        expected["zoom"]
            .as_str()
            .map(|text| text.parse::<f64>().unwrap()),
        "{what}: увеличение"
    );
    assert_eq!(vars.par, pair(&expected["par"]), "{what}: сдвиг фона");
    assert_eq!(
        vars.front,
        pair(&expected["front"]),
        "{what}: сдвиг переднего плана"
    );
}

#[test]
fn replays_every_scenario_of_the_vue_component() {
    let fixture = fixture();
    let scenarios = fixture["scenarios"].as_array().unwrap();
    assert!(scenarios.len() >= 15);
    let mut frames = 0;
    for scenario in scenarios {
        let name = scenario["name"].as_str().unwrap();
        let window = scenario["window"].as_array().unwrap();
        let (width, height) = (window[0].as_f64().unwrap(), window[1].as_f64().unwrap());
        let mut model = Parallax::new(
            (width, height),
            scenario["focused"].as_bool().unwrap(),
            scenario["reducedMotion"].as_bool().unwrap(),
            scenario["parallax"].as_bool().unwrap(),
        );
        model.mount();
        let mut events = scenario["events"].as_array().cloned().unwrap_or_default();
        events.sort_by(|a, b| a["t"].as_f64().partial_cmp(&b["t"].as_f64()).unwrap());
        let mut trace = scenario["trace"].as_array().unwrap().iter().enumerate();

        let (_, start) = trace.next().unwrap();
        check(name, 0, &model, start);
        let mut next_event = 0;
        for time in scenario["frameTimes"].as_array().unwrap() {
            let time = time.as_f64().unwrap();
            while next_event < events.len() && events[next_event]["t"].as_f64().unwrap() <= time {
                apply(&mut model, &events[next_event]);
                next_event += 1;
                let (index, expected) = trace.next().expect("запись события");
                check(name, index, &model, expected);
            }
            if model.frame_pending() {
                model.frame(BASE + time);
                frames += 1;
                let (index, expected) = trace.next().expect("запись кадра");
                assert_eq!(expected["kind"], "frame");
                check(name, index, &model, expected);
            }
        }
        while next_event < events.len() {
            apply(&mut model, &events[next_event]);
            next_event += 1;
            let (index, expected) = trace.next().expect("запись события");
            check(name, index, &model, expected);
        }
        assert!(trace.next().is_none(), "{name}: лишние записи в фикстуре");
        let amp = scenario["amp"].as_array().unwrap();
        assert_eq!(
            model.amplitude(),
            (amp[0].as_f64().unwrap(), amp[1].as_f64().unwrap()),
            "{name}: амплитуда"
        );
    }
    assert!(frames > 500, "кадров в сценариях: {frames}");
}

#[test]
fn constants_of_the_component() {
    // LauncherBackground.vue:30-45.
    assert_eq!(parallax::ZOOM, 1.05);
    assert_eq!(parallax::SMOOTH_TIME, 0.32);
    assert_eq!(parallax::MAX_DT, 0.05);
    assert_eq!(parallax::EPSILON, 0.05);
    assert_eq!(parallax::FRONT_DEPTH, 2.0);
    // Амплитуда: 72 % запаса по краю (2,5 %), не меньше 6 px (240 px × 2,5 %).
    let mut model = Parallax::new((1600.0, 900.0), true, false, true);
    // До mounted() — запас по умолчанию, как у created().
    assert_eq!(model.amplitude(), (6.0, 6.0));
    model.mount();
    assert!((model.amplitude().0 - 28.8).abs() < 1e-9);
    assert!((model.amplitude().1 - 16.2).abs() < 1e-9);
    let mut small = Parallax::new((100.0, 50.0), true, false, true);
    small.mount();
    assert_eq!(small.amplitude(), (6.0, 6.0));
}

/// Пружина на кадрах `step` мс: без перелёта, у цели цикл останавливается. Возвращает число кадров
/// и время прихода, мс.
fn reaches_the_target(step: f64) -> (usize, f64) {
    let mut model = Parallax::new((1600.0, 900.0), true, false, true);
    model.mount();
    model.pointer_moved(1600.0, 900.0);
    let mut previous = 0.0_f64;
    let mut now = BASE;
    let mut frames = 0;
    for _ in 0..3000 {
        if !model.frame_pending() {
            break;
        }
        now += step;
        model.frame(now);
        frames += 1;
        let x = model.vars().par.unwrap().0;
        // Цель — минус амплитуда; критически демпфированная пружина её не перелетает.
        assert!(x <= previous + 1e-9 && x >= -28.8 - 1e-9, "{step} мс: {x}");
        previous = x;
    }
    assert!(
        !model.frame_pending(),
        "{step} мс: цикл должен остановиться у цели"
    );
    assert_eq!(model.vars().par, Some((-28.8, -16.2)));
    assert_eq!(model.vars().front, Some((-57.6, -32.4)));
    (frames, now - BASE)
}

#[test]
fn smooth_damp_reaches_the_target_without_overshoot() {
    let (frames, _) = reaches_the_target(1000.0 / 60.0);
    assert!(frames > 60, "{frames}");
}

/// Кадры с частотой монитора (requestAnimationFrame идёт с ней; у владельца 279 и 180 Гц): шаг
/// пружины — по времени кадра, поэтому кадров больше, а приходит слой к цели за то же время.
#[test]
fn smooth_damp_at_monitor_rates_arrives_in_the_same_time() {
    let (frames_60, at_60) = reaches_the_target(1000.0 / 60.0);
    for hz in [144.0, 279.0] {
        let (frames, at) = reaches_the_target(1000.0 / hz);
        assert!(
            frames as f64 > frames_60 as f64 * hz / 60.0 * 0.9,
            "{hz} Гц: {frames} кадров против {frames_60} на 60 Гц"
        );
        // Покой наступает, когда и расстояние, и скорость меньше EPSILON: ±пара кадров 60 Гц.
        assert!((at - at_60).abs() < 40.0, "{hz} Гц: {at} мс против {at_60}");
    }
}

/// Траектория сценария фикстуры (значения 0.4.7) и та же, посчитанная моделью: время кадра, мс,
/// и сдвиг фона после него.
fn trajectory_of(fixture: &Value, name: &str) -> Vec<(f64, (f64, f64))> {
    let scenario = fixture["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|scenario| scenario["name"] == name)
        .unwrap_or_else(|| panic!("нет сценария «{name}»"));
    let window = scenario["window"].as_array().unwrap();
    let mut model = Parallax::new(
        (window[0].as_f64().unwrap(), window[1].as_f64().unwrap()),
        true,
        false,
        true,
    );
    model.mount();
    let events = scenario["events"].as_array().unwrap();
    let mut next_event = 0;
    let frames: Vec<&Value> = scenario["trace"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["kind"] == "frame")
        .collect();
    let mut points = Vec::new();
    let mut seen = 0;
    for time in scenario["frameTimes"].as_array().unwrap() {
        let time = time.as_f64().unwrap();
        while next_event < events.len() && events[next_event]["t"].as_f64().unwrap() <= time {
            apply(&mut model, &events[next_event]);
            next_event += 1;
        }
        if model.frame_pending() {
            model.frame(BASE + time);
            let par = model.vars().par.unwrap();
            assert_eq!(Some(par), pair(&frames[seen]["par"]), "{name}: {time} мс");
            seen += 1;
            points.push((time, par));
        }
    }
    assert_eq!(seen, frames.len(), "{name}: кадров");
    points
}

/// Сдвиг в момент `at`, мс, между кадрами — по прямой (до первого кадра — ноль).
fn at_time(points: &[(f64, (f64, f64))], at: f64) -> (f64, f64) {
    let after = points.partition_point(|(time, _)| *time <= at);
    match (after.checked_sub(1).map(|i| points[i]), points.get(after)) {
        (None, _) => (0.0, 0.0),
        (Some((_, value)), None) => value,
        (Some((t0, a)), Some((t1, b))) => {
            let k = (at - t0) / (t1 - t0);
            (a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k)
        }
    }
}

/// 144 и 279 Гц против 60 Гц (сценарии фикстуры, значения 0.4.7, модель сходится с ними на каждом
/// кадре): в одни и те же моменты фон на одном месте — разница меньше полпикселя (первый кадр после
/// покоя считается за 1/60 с при любой частоте, как в 0.4.7: на быстром мониторе слой чуть впереди).
#[test]
fn monitor_rates_follow_the_trajectory_of_0_4_7_in_time() {
    let fixture = fixture();
    let base = trajectory_of(&fixture, "движение к правому нижнему углу, 60 Гц");
    for name in [
        "движение к правому нижнему углу, 144 Гц",
        "движение к правому нижнему углу, 279 Гц",
    ] {
        let fast = trajectory_of(&fixture, name);
        assert!(fast.len() > base.len() * 2, "{name}: кадров {}", fast.len());
        let mut worst = 0.0_f64;
        for (time, value) in &base {
            let there = at_time(&fast, *time);
            worst = worst
                .max((there.0 - value.0).abs())
                .max((there.1 - value.1).abs());
        }
        assert!(worst < 0.5, "{name}: расхождение с 60 Гц {worst} px");
        println!("{name}: наибольшее расхождение с 60 Гц — {worst:.3} px");
    }
    // Остальные сценарии частоты монитора сходятся с 0.4.7 кадр в кадр.
    trajectory_of(
        &fixture,
        "середина окна, затем в левый верхний угол, 180 Гц",
    );
    trajectory_of(&fixture, "279 Гц с пропущенными кадрами");
}

/// Кадры из настоящего Electron 44 (`tools/reference/ui/parallax-electron.js` →
/// `tests/fixtures/parallax-electron.json`): `mousemove` в точку, затем метки кадров
/// requestAnimationFrame и значения `--l-bg-par-*`/`--l-bg-front-*` после шага компонента. Модель с
/// теми же метками кадров даёт те же значения до знака — траектория через 100/300/600 мс после
/// движения мыши совпадает с замером (спецификация ПП5, «Сверка»).
#[test]
fn replays_frames_measured_in_electron() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/parallax-electron.json"))
        .expect("parallax-electron.json");
    let runs = fixture["runs"].as_array().unwrap();
    assert!(runs.len() >= 2, "угол и противоположный угол");
    for run in runs {
        let name = run["name"].as_str().unwrap();
        assert_eq!(run["hasFocus"], true, "{name}: окно в фокусе");
        assert_eq!(run["zoom"], "1.05", "{name}: увеличение фона");
        assert_eq!(run["innerWidth"], run["clientWidth"], "{name}");
        assert_eq!(run["innerHeight"], run["clientHeight"], "{name}");
        // Замер начинается в покое: пружина в нуле, скорость нулевая, кадр не запрошен.
        assert_eq!(pair(&run["start"]["par"]), Some((0.0, 0.0)), "{name}");
        assert_eq!(pair(&run["start"]["front"]), Some((0.0, 0.0)), "{name}");

        let size = (
            run["clientWidth"].as_f64().unwrap(),
            run["clientHeight"].as_f64().unwrap(),
        );
        let mut model = Parallax::new(size, true, false, true);
        model.mount();
        model.pointer_moved(run["x"].as_f64().unwrap(), run["y"].as_f64().unwrap());
        assert!(model.frame_pending(), "{name}: mousemove запросил кадр");

        let frames = run["frames"].as_array().unwrap();
        let first = frames[0]["now"].as_f64().unwrap();
        assert!(frames.len() > 20, "{name}: кадров {}", frames.len());
        assert!(
            frames.last().unwrap()["now"].as_f64().unwrap() - first >= 600.0,
            "{name}: замер короче 600 мс"
        );
        let mut checkpoints = [100.0, 300.0, 600.0].map(|at| (at, None::<(f64, f64)>));
        for (index, frame) in frames.iter().enumerate() {
            let now = frame["now"].as_f64().unwrap();
            // Кадр без запрошенного шага (покой) значений не меняет — как у компонента.
            if model.frame_pending() {
                model.frame(now);
            }
            let vars = model.vars();
            let what = format!("{name} кадр {index} ({:.1} мс)", now - first);
            assert_eq!(vars.par, pair(&frame["par"]), "{what}: сдвиг фона");
            assert_eq!(vars.front, pair(&frame["front"]), "{what}: передний план");
            for (at, seen) in &mut checkpoints {
                if seen.is_none() && now - first >= *at {
                    *seen = vars.par;
                }
            }
        }
        // Цель — противоположный курсору край: −амплитуда по x для курсора справа, +амплитуда слева.
        let amplitude = model.amplitude();
        let toward = if run["x"].as_f64().unwrap() > size.0 / 2.0 {
            -1.0
        } else {
            1.0
        };
        let (par_x, _) = checkpoints[2].1.expect("значение через 600 мс");
        assert!(
            par_x * toward > 0.0 && par_x.abs() < amplitude.0,
            "{name}: через 600 мс {par_x}, амплитуда {}",
            amplitude.0
        );
        println!("{name}: сдвиг фона через 100/300/600 мс — {checkpoints:?}");
    }
}
