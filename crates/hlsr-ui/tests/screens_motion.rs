//! Движение заставки и фона: реестр `Motion` (выгрузка CSS 0.4.7, `data/motion.json`) против
//! `tools/snap/screens/animations.json` — анимации, которые Electron записал на сценах заставки
//! (`loading-*`) и лаунчера: вход экрана `slide`, мигание стрелок `blink` и подпись `animate`,
//! кроссфейд кадра `launcher-bg-fade`, колебание шаров `tool-orbit`. Время, задержка, повторы,
//! направление, кривые и значения ключевых кадров совпадают (спецификация ПП5, «Сверка»).

use hlsr_ui::motion::{Direction, Iterations, Prop, registry};
use serde_json::Value;

fn catalog() -> Vec<Value> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/snap/screens/animations.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let value: Value = serde_json::from_str(&text).unwrap();
    value["animations"].as_array().unwrap().clone()
}

/// Кривая CSS: ключевое слово или `cubic-bezier(a, b, c, d)`.
fn bezier(text: &str) -> [f64; 4] {
    match text {
        "linear" => [0.0, 0.0, 1.0, 1.0],
        "ease" => [0.25, 0.1, 0.25, 1.0],
        "ease-in" => [0.42, 0.0, 1.0, 1.0],
        "ease-out" => [0.0, 0.0, 0.58, 1.0],
        "ease-in-out" => [0.42, 0.0, 0.58, 1.0],
        other => {
            let inner = other
                .strip_prefix("cubic-bezier(")
                .and_then(|rest| rest.strip_suffix(')'))
                .unwrap_or_else(|| panic!("кривая {other}"));
            let numbers: Vec<f64> = inner
                .split(',')
                .map(|n| n.trim().parse().unwrap())
                .collect();
            [numbers[0], numbers[1], numbers[2], numbers[3]]
        }
    }
}

fn scenes(entry: &Value) -> Vec<&str> {
    entry["scenes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect()
}

fn on_loading(entry: &Value) -> bool {
    scenes(entry)
        .iter()
        .any(|scene| scene.starts_with("loading-"))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

/// Целое из `…-N` в классах стрелки: `outer-13` → 13.
fn index_of(target: &str, class: &str) -> i32 {
    target
        .split('.')
        .find_map(|part| part.strip_prefix(class).and_then(|n| n.strip_prefix('-')))
        .unwrap_or_else(|| panic!("{target}: нет {class}-N"))
        .parse()
        .unwrap()
}

/// Значения дорожки `opacity` из ключевых кадров записи: `(смещение, значение)`.
fn recorded_opacity(entry: &Value) -> Vec<(f64, f64)> {
    entry["keyframes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|frame| {
            (
                frame["offset"].as_f64().unwrap(),
                frame["opacity"].as_str().unwrap().parse().unwrap(),
            )
        })
        .collect()
}

/// Дорожка реестра: `(доля, значение)`.
fn track(keyframes: &str, prop: Prop) -> Vec<(f64, f64)> {
    registry().keyframes[keyframes].tracks[&prop]
        .iter()
        .map(|(at, value)| (*at, value.expect("явные кадры")))
        .collect()
}

#[test]
fn slide_enter_matches_the_recorded_transition() {
    let transition = &registry().transitions["slide"];
    let mut seen = 0;
    for entry in catalog() {
        let Some(name) = entry["vueTransition"]
            .as_str()
            .filter(|n| n.starts_with("slide-"))
        else {
            continue;
        };
        assert_eq!(name, "slide-enter", "в сценах записан только вход");
        let property = entry["name"].as_str().unwrap();
        let timing = transition
            .enter
            .iter()
            .find(|timing| timing.property == property)
            .unwrap_or_else(|| panic!("в реестре нет входа {property}"));
        assert!(
            close(entry["duration"].as_f64().unwrap(), timing.duration),
            "{property}"
        );
        assert!(
            close(entry["delay"].as_f64().unwrap(), timing.delay),
            "{property}"
        );
        assert_eq!(
            bezier(entry["easing"].as_str().unwrap()),
            timing.easing,
            "{property}"
        );
        // Начало входа: прозрачность 0, сдвиг вниз (CSS — 20 px; запись берёт кадр Chromium, когда
        // переход уже начался, поэтому 20 или 25 px — по шагу между кадрами).
        let first = &entry["keyframes"][0];
        match property {
            "opacity" => assert_eq!(first["opacity"], "0"),
            "transform" => assert!(
                first["transform"] == "translate3d(0px, 20px, 0px)"
                    || first["transform"] == "translate3d(0px, 25px, 0px)",
                "{first}"
            ),
            other => panic!("свойство {other}"),
        }
        seen += 1;
    }
    assert!(seen >= 6, "записей входа: {seen}");
    assert_eq!(transition.hidden[&Prop::Y], 20.0);
}

#[test]
fn background_fade_matches_the_recorded_transition() {
    let tokens = &registry().tokens;
    let mut seen = 0;
    for entry in catalog() {
        let Some(name) = entry["vueTransition"]
            .as_str()
            .filter(|n| n.starts_with("launcher-bg-fade"))
        else {
            continue;
        };
        assert_eq!(entry["name"], "opacity", "{name}");
        assert!(
            close(entry["duration"].as_f64().unwrap(), tokens.l_dur_fade),
            "{name}"
        );
        assert_eq!(entry["delay"].as_f64(), Some(0.0), "{name}");
        assert_eq!(
            bezier(entry["easing"].as_str().unwrap()),
            tokens.l_ease_fade,
            "{name}"
        );
        seen += 1;
    }
    assert!(seen >= 5, "записей кроссфейда: {seen}");
}

#[test]
fn arrows_blink_with_the_recorded_delays() {
    let mut targets = 0;
    for entry in catalog()
        .iter()
        .filter(|e| e["type"] == "CSSAnimation" && on_loading(e))
    {
        if !entry["name"].as_str().unwrap().starts_with("blink") {
            continue;
        }
        let target = entry["target"].as_str().unwrap();
        let (id, index) = if target.contains(".outer.") {
            ("loading-arrow-outer", index_of(target, "outer"))
        } else {
            ("loading-arrow-inner", index_of(target, "inner"))
        };
        let def = registry().animation(id).unwrap();
        // `animation-delay: -2s / 18 × i` у внешних, `+2s / 6 × i` у внутренних.
        let delay = def.stagger.expect("лесенка").step * f64::from(index);
        assert!(
            (entry["delay"].as_f64().unwrap() - delay).abs() < 0.001,
            "{target}: задержка {} против {delay}",
            entry["delay"]
        );
        assert!(
            close(entry["duration"].as_f64().unwrap(), def.duration),
            "{target}"
        );
        assert_eq!(entry["iterations"], "infinite");
        assert_eq!(def.iterations, Iterations::Infinite);
        assert_eq!(def.direction, Direction::Normal);
        // Кривая — у каждого ключевого кадра (`animation: blink 2s infinite` — ease).
        for frame in entry["keyframes"].as_array().unwrap() {
            assert_eq!(
                bezier(frame["easing"].as_str().unwrap()),
                def.easing,
                "{target}"
            );
        }
        assert_eq!(
            recorded_opacity(entry),
            track(&def.keyframes, Prop::Opacity),
            "{target}"
        );
        targets += 1;
    }
    assert_eq!(targets, 24, "24 стрелки");
}

#[test]
fn status_breathes_with_the_recorded_opacity() {
    let def = registry().animation("loading-status").unwrap();
    let entries: Vec<Value> = catalog()
        .into_iter()
        .filter(|e| {
            e["type"] == "CSSAnimation"
                && on_loading(e)
                && e["name"].as_str().unwrap().starts_with("animate")
        })
        .collect();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert!(close(entry["duration"].as_f64().unwrap(), def.duration));
    assert_eq!(entry["delay"].as_f64(), Some(def.delay));
    for frame in entry["keyframes"].as_array().unwrap() {
        assert_eq!(bezier(frame["easing"].as_str().unwrap()), def.easing);
    }
    assert_eq!(
        recorded_opacity(entry),
        track(&def.keyframes, Prop::Opacity)
    );
}

#[test]
fn orbs_oscillate_with_the_recorded_phases() {
    let mut orbs = 0;
    for entry in catalog().iter().filter(|e| e["type"] == "CSSAnimation") {
        if !entry["name"].as_str().unwrap().starts_with("tool-orbit") {
            continue;
        }
        let target = entry["target"].as_str().unwrap();
        let id = ["red", "blue", "green"]
            .iter()
            .find(|tone| target.ends_with(&format!("launcher-bg__orb--{tone}")))
            .map(|tone| format!("tool-orb-{tone}"))
            .unwrap_or_else(|| panic!("{target}"));
        let def = registry().animation(&id).unwrap();
        assert!(
            close(entry["duration"].as_f64().unwrap(), def.duration),
            "{id}"
        );
        assert!(close(entry["delay"].as_f64().unwrap(), def.delay), "{id}");
        assert_eq!(entry["direction"], "alternate");
        assert_eq!(def.direction, Direction::Alternate);
        for frame in entry["keyframes"].as_array().unwrap() {
            assert_eq!(
                bezier(frame["easing"].as_str().unwrap()),
                def.easing,
                "{id}"
            );
        }
        // `translate3d(0px, -6px, 0px)` → `translate3d(0px, 8px, 0px)`: дорожка y.
        let expected = track(&def.keyframes, Prop::Y);
        for (frame, (offset, value)) in entry["keyframes"].as_array().unwrap().iter().zip(expected)
        {
            assert!(close(frame["offset"].as_f64().unwrap(), offset), "{id}");
            let transform = frame["transform"].as_str().unwrap();
            let y: f64 = transform
                .trim_start_matches("translate3d(")
                .trim_end_matches(')')
                .split(',')
                .nth(1)
                .unwrap()
                .trim()
                .trim_end_matches("px")
                .parse()
                .unwrap();
            assert!(close(y, value), "{id}: {transform}");
        }
        orbs += 1;
    }
    assert_eq!(orbs, 3, "три шара");
}
