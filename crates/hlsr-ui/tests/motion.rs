//! Реестр движения (`data/motion.json` из CSS 0.4.7) и отсчёт ключевых кадров и
//! переходов по правилам CSS Animations / Transitions.

use hlsr_ui::motion::{CubicBezier, Frame, Iterations, Still, registry};
use serde_json::Value;

const EPS: f64 = 1e-6;

/// Кривая независимым способом: x(t) делением пополам до 1e-13.
fn reference(curve: [f64; 4], x: f64) -> f64 {
    let [x1, y1, x2, y2] = curve;
    let bezier = |p1: f64, p2: f64, t: f64| {
        let u = 1.0 - t;
        3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t
    };
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..200 {
        let mid = (lo + hi) / 2.0;
        if bezier(x1, x2, mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bezier(y1, y2, (lo + hi) / 2.0)
}

fn ease(curve: [f64; 4], x: f64) -> f64 {
    CubicBezier::new(curve).solve(x)
}

fn close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() < EPS,
        "{what}: {actual} ≠ {expected}"
    );
}

#[test]
fn bezier_matches_reference_on_all_tokens() {
    let tokens = &registry().tokens;
    let curves = [
        tokens.l_ease,
        tokens.l_ease_in,
        tokens.l_ease_spring,
        tokens.l_ease_fade,
        tokens.l_ease_swipe,
        [0.25, 0.1, 0.25, 1.0],
        [0.42, 0.0, 0.58, 1.0],
    ];
    for curve in curves {
        for step in 0..=1000 {
            let x = f64::from(step) / 1000.0;
            close(
                ease(curve, x),
                reference(curve, x),
                &format!("{curve:?} при {x}"),
            );
        }
    }
    // Пружина --l-ease-spring перелетает: максимум ≈ 1,098 (y > 1).
    let spring = tokens.l_ease_spring;
    let peak = (0..=1000)
        .map(|i| ease(spring, f64::from(i) / 1000.0))
        .fold(0.0, f64::max);
    assert!(peak > 1.09 && peak < 1.11, "пик пружины {peak}");
    // За пределами [0, 1] — касательные в концах (Chromium).
    close(ease(tokens.l_ease, -0.1), -0.1 * (1.0 / 0.22), "до начала");
    close(
        ease(tokens.l_ease_in, 1.1),
        1.0 + 0.1 * ((0.2 - 1.0) / (0.75 - 1.0)),
        "после конца",
    );
}

/// Токены движения в themes.json (export-themes.js, вычисленные стили Electron)
/// и в выгрузке CSS совпадают — у всех тем одни.
#[test]
fn tokens_match_themes_json() {
    let themes: Value = serde_json::from_str(include_str!("../themes.json")).unwrap();
    let tokens = &registry().tokens;
    let easing = |key: &str| -> [f64; 4] {
        let list = themes["themes"]["hlsr"][key]["easing"].as_array().unwrap();
        [0, 1, 2, 3].map(|i| list[i].as_f64().unwrap())
    };
    let number = |key: &str, kind: &str| themes["themes"]["hlsr"][key][kind].as_f64().unwrap();
    assert_eq!(easing("l-ease"), tokens.l_ease);
    assert_eq!(easing("l-ease-in"), tokens.l_ease_in);
    assert_eq!(easing("l-ease-spring"), tokens.l_ease_spring);
    assert_eq!(easing("l-ease-fade"), tokens.l_ease_fade);
    assert_eq!(easing("l-ease-swipe"), tokens.l_ease_swipe);
    for (key, value) in [
        ("l-dur-fast", tokens.l_dur_fast),
        ("l-dur", tokens.l_dur),
        ("l-dur-slow", tokens.l_dur_slow),
        ("l-dur-fade", tokens.l_dur_fade),
        ("l-dur-swipe", tokens.l_dur_swipe),
        ("l-dur-tab", tokens.l_dur_tab),
        ("l-ink-lead", tokens.l_ink_lead),
        ("l-ink-trail", tokens.l_ink_trail),
    ] {
        assert_eq!(number(key, "duration"), value, "{key}");
    }
    assert_eq!(number("l-rise", "length"), tokens.l_rise);
    assert_eq!(number("l-swipe-gap", "length"), tokens.l_swipe_gap);
    // Одинаково во всех темах: Slint берёт их постоянными (Theme, ThemeEasing).
    for theme in themes["order"].as_array().unwrap() {
        let theme = theme.as_str().unwrap();
        for key in [
            "l-ease",
            "l-ease-swipe",
            "l-dur",
            "l-ink-lead",
            "l-ink-trail",
        ] {
            assert_eq!(
                themes["themes"][theme][key], themes["themes"]["hlsr"][key],
                "{theme} {key}"
            );
        }
    }
}

/// Ink в Slint (components/tabs.slint) берёт края из Theme.l-ink-lead/trail и
/// кривую ThemeEasing.l-ease-swipe — как .l-ink 0.4.7.
#[test]
fn ink_edges_are_lead_and_trail_tokens() {
    let registry = registry();
    let (tokens, ink) = (&registry.tokens, &registry.ink);
    for edges in [ink.default, ink.next, ink.prev] {
        assert_eq!(edges.left.easing, tokens.l_ease_swipe);
        assert_eq!(edges.right.easing, tokens.l_ease_swipe);
    }
    assert_eq!(
        (ink.default.left.duration, ink.default.right.duration),
        (tokens.l_ink_trail, tokens.l_ink_trail)
    );
    assert_eq!(
        (ink.next.left.duration, ink.next.right.duration),
        (tokens.l_ink_trail, tokens.l_ink_lead)
    );
    assert_eq!(
        (ink.prev.left.duration, ink.prev.right.duration),
        (tokens.l_ink_lead, tokens.l_ink_trail)
    );
    assert_eq!(ink.still, None);
    assert_eq!(registry.theme.duration, tokens.l_dur_slow);
    assert_eq!(registry.theme.easing, tokens.l_ease);
}

#[test]
fn registry_is_complete() {
    let registry = registry();
    for name in [
        "l-pop",
        "l-pop-lg",
        "l-fade",
        "l-modal",
        "l-value",
        "l-slide-next",
        "l-slide-prev",
        "slide",
    ] {
        assert!(registry.transitions.contains_key(name), "{name}");
    }
    for (id, animation) in &registry.animations {
        assert!(
            registry.keyframes.contains_key(&animation.keyframes),
            "{id}: {}",
            animation.keyframes
        );
    }
    // Разовые из плана Ф2 и бесконечные.
    for id in [
        "toast-out",
        "notif-receive",
        "sponsors-beat",
        "loading-arrow-outer",
        "runner-check",
        "fa-spin",
        "fa-pulse",
    ] {
        assert!(registry.animations.contains_key(id), "{id}");
    }
    assert_eq!(
        registry.animations["sponsors-beat"].iterations,
        Iterations::Infinite
    );
}

fn kf(id: &str, t: f64, index: i32) -> Frame {
    registry().keyframes_frame(id, Some(t), index, Still::MOVING, true)
}

#[test]
fn toast_out_keyframes_by_property() {
    let tokens = &registry().tokens;
    // 0 %: исходные значения элемента (min-height 56, padding 12).
    let start = kf("toast-out", 0.0, 0);
    assert_eq!(
        (start.opacity, start.min_height, start.padding, start.height),
        (1.0, 56.0, 12.0, 1.0)
    );
    // 55 % (231 мс): ушёл вправо и погас, высота ещё полная.
    let mid = kf("toast-out", 231.0, 0);
    close(mid.opacity, 0.0, "opacity");
    close(mid.x, 56.0, "x");
    close(mid.scale, 0.96, "scale");
    close(mid.height, 1.0, "height");
    close(mid.margin, 0.0, "margin");
    // min-height и padding заданы только в 100 % — интервал на всё время.
    let p = ease(tokens.l_ease_in, 231.0 / 420.0);
    close(mid.min_height, 56.0 * (1.0 - p), "min-height");
    close(mid.padding, 12.0 * (1.0 - p), "padding");
    // Половина второго интервала: высота и отступ.
    let t = 0.55 * 420.0 + 0.5 * 0.45 * 420.0;
    let q = ease(tokens.l_ease_in, 0.5);
    let late = kf("toast-out", t, 0);
    close(late.height, 1.0 - q, "height");
    close(late.margin, -8.0 * q, "margin");
    // Конец (fill both): схлопнут.
    let end = kf("toast-out", 420.0, 0);
    assert_eq!(
        (end.height, end.margin, end.min_height, end.padding),
        (0.0, -8.0, 0.0, 0.0)
    );
    // Liquid Glass: та же раскладка, кривая --l-ease.
    let lg = registry().keyframes_frame("toast-out-lg", Some(231.0), 0, Still::MOVING, true);
    close(
        lg.min_height,
        56.0 * (1.0 - ease(tokens.l_ease, 0.55)),
        "LG",
    );
}

#[test]
fn notif_receive_tracks_are_independent() {
    let tokens = &registry().tokens;
    // Цвет задан только в 82 %: от 0 % к 82 % — один интервал.
    let at = 0.41 * 420.0;
    close(
        kf("notif-receive", at, 0).mix,
        ease(tokens.l_ease, 0.5),
        "mix",
    );
    // Масштаб: 65 % → 82 %.
    let at = 0.735 * 420.0;
    let local = (0.735 - 0.65) / (0.82 - 0.65);
    close(
        kf("notif-receive", at, 0).scale,
        1.0 + 0.12 * ease(tokens.l_ease, local),
        "scale",
    );
    // fill none: после конца — исходные значения.
    assert_eq!(kf("notif-receive", 421.0, 0), Frame::REST);
}

#[test]
fn infinite_alternate_and_negative_delay() {
    let tokens = &registry().tokens;
    // Сердце спонсоров, вторая итерация, 14 %.
    close(kf("sponsors-beat", 1100.0 + 154.0, 0).scale, 1.18, "beat");
    // Шар инструмента: задержка −2 с — уже треть первой итерации.
    let in_out = [0.42, 0.0, 0.58, 1.0];
    let y = |p: f64| -6.0 + 14.0 * ease(in_out, p);
    close(kf("tool-orb-blue", 0.0, 0).y, y(2000.0 / 6000.0), "blue");
    // Вторая итерация alternate — назад.
    close(
        kf("tool-orb-red", 6000.0 + 1500.0, 0).y,
        y(1.0 - 0.25),
        "red alternate",
    );
    // Треугольник заставки outer-3: −333 мс, кривая ease.
    let ease_kw = [0.25, 0.1, 0.25, 1.0];
    let p = 333.333_333_3 / 2000.0;
    close(
        kf("loading-arrow-outer", 0.0, 3).opacity,
        0.1 + 0.9 * ease(ease_kw, p / 0.3),
        "blink",
    );
    let _ = tokens;
}

#[test]
fn stagger_delay_and_fill() {
    let tokens = &registry().tokens;
    // dropdown-item: min(i, 8) × 16 мс; до задержки — from (fill both).
    let before = kf("dropdown-item", 127.0, 10);
    assert_eq!((before.opacity, before.y), (0.0, 8.0));
    let half = kf("dropdown-item", 128.0 + 130.0, 10);
    close(half.opacity, ease(tokens.l_ease, 0.5), "opacity");
    close(half.y, 8.0 * (1.0 - ease(tokens.l_ease, 0.5)), "y");
    // notif-item: 40 мс + i × 30 мс.
    assert_eq!(kf("notif-item", 129.0, 3).opacity, 0.0);
    assert!(kf("notif-item", 131.0, 3).opacity > 0.0);
    // runner-check: fill forwards — до задержки исходное (30), не from.
    assert_eq!(kf("runner-check", 50.0, 0).dash, 30.0);
    close(
        kf("runner-check", 350.0, 0).dash,
        30.0 * (1.0 - ease(tokens.l_ease, 0.5)),
        "dash",
    );
    assert_eq!(kf("runner-check", 10_000.0, 0).dash, 0.0);
    // Длина: задержка + длительность.
    assert_eq!(
        registry().keyframes_length("dropdown-item", 3, Still::MOVING),
        Some(48.0 + 260.0)
    );
    assert_eq!(
        registry().keyframes_length("fa-spin", 0, Still::MOVING),
        None
    );
}

#[test]
fn still_modes_follow_no_anim_and_reduced_motion() {
    let registry = registry();
    let no_anim = Still {
        no_anim: true,
        os_reduced: false,
    };
    let os = Still {
        no_anim: false,
        os_reduced: true,
    };
    let frame = |id: &str, t: f64, index: i32, still: Still| {
        registry.keyframes_frame(id, Some(t), index, still, true)
    };
    // Кольцо вместо вращения мигает (l-pulse 1,4 с): середина — 0.35.
    close(frame("fa-spin", 700.0, 0, no_anim).opacity, 0.35, "pulse");
    close(frame("fa-spin", 700.0, 0, os).opacity, 0.35, "pulse os");
    assert_eq!(frame("fa-spin", 700.0, 0, no_anim).rotation, 0.0);
    // Общее правило: 0,01 мс, задержка остаётся.
    let item = |t| frame("dropdown-item", t, 2, no_anim);
    assert_eq!(item(31.0).opacity, 0.0);
    assert_eq!(item(32.0), Frame::REST);
    assert!(registry.keyframes_moving("dropdown-item", no_anim));
    // animation: none.
    assert_eq!(frame("nick-developer", 1234.0, 0, no_anim), Frame::REST);
    assert!(!registry.keyframes_moving("nick-developer", os));
    // .no-anim у рекордов — none, у ОС — общее правило.
    assert_eq!(frame("home-record-next", 0.0, 2, no_anim), Frame::REST);
    assert_eq!(frame("home-record-next", 0.0, 2, os).opacity, 0.0);
    // ОС у галочки входа: сразу конец.
    assert_eq!(frame("runner-check", 0.0, 0, os).dash, 0.0);
    // Бесконечная с общим правилом стоит (0.4.7 мелькает).
    assert_eq!(frame("sponsors-beat", 154.0, 0, os), Frame::REST);
    // Заставка вне .launcher-root: правила не действуют.
    assert_eq!(
        frame("loading-status", 1000.0, 0, os).opacity,
        frame("loading-status", 1000.0, 0, Still::MOVING).opacity
    );
}

#[test]
fn frozen_and_not_playing() {
    let registry = registry();
    // Снимок: разовая с заполнением — конец, без заполнения — исходное,
    // бесконечная — t = 0.
    let frozen =
        |id: &str, index: i32| registry.keyframes_frame(id, None, index, Still::MOVING, true);
    assert_eq!(frozen("dropdown-item", 5), Frame::REST);
    assert_eq!(frozen("lso-flash", 0), Frame::REST);
    assert_eq!(frozen("toast-life", 0).scale_x, 0.0);
    close(frozen("loading-arrow-inner", 0).opacity, 0.1, "blink t=0");
    close(
        frozen("loading-arrow-outer", 9).opacity,
        kf("loading-arrow-outer", 0.0, 9).opacity,
        "фаза",
    );
    // Класса нет — исходные значения.
    assert_eq!(
        registry.keyframes_frame("toast-life", Some(100.0), 0, Still::MOVING, false),
        Frame::REST
    );
}

#[test]
fn transitions_by_property() {
    let registry = registry();
    let tokens = &registry.tokens;
    let enter =
        |name: &str, t: f64| registry.transition_frame(name, true, Some(t), Still::MOVING, None);
    let leave =
        |name: &str, t: f64| registry.transition_frame(name, false, Some(t), Still::MOVING, None);

    let e = ease(tokens.l_ease, 0.5);
    let pop = enter("l-pop", 130.0);
    close(pop.opacity, e, "pop opacity");
    close(pop.y, -4.0 * (1.0 - e), "pop y");
    close(pop.scale, 0.97 + 0.03 * e, "pop scale");
    let i = ease(tokens.l_ease_in, 0.5);
    let gone = leave("l-pop", 80.0);
    close(gone.opacity, 1.0 - i, "pop leave");
    // Liquid Glass: уход 260 мс --l-ease.
    close(leave("l-pop-lg", 130.0).opacity, 1.0 - e, "pop LG");

    let modal = enter("l-modal", 130.0);
    close(modal.mix, e, "modal scrim");
    close(modal.opacity, e, "modal children");

    let s = ease(tokens.l_ease_swipe, 0.5);
    close(enter("l-slide-next", 175.0).slide, 1.0 - s, "slide next in");
    close(leave("l-slide-next", 175.0).slide, -s, "slide next out");
    close(
        enter("l-slide-prev", 175.0).slide,
        -(1.0 - s),
        "slide prev in",
    );

    close(
        enter("l-value", 250.0).opacity,
        ease(tokens.l_ease_fade, 0.5),
        "value",
    );

    // App.vue: transform 420 мс, opacity 260 мс.
    let app = enter("slide", 210.0);
    close(app.y, 20.0 * (1.0 - e), "app y");
    close(
        app.opacity,
        ease(tokens.l_ease, 210.0 / 260.0),
        "app opacity",
    );
    close(leave("slide", 80.0).y, -12.0 * i, "app leave");

    // Без анимаций — сразу конец; в конце — конечные значения.
    let still = Still {
        no_anim: true,
        os_reduced: false,
    };
    assert_eq!(
        registry.transition_frame("l-pop", true, Some(0.0), still, None),
        Frame::REST
    );
    assert_eq!(
        registry
            .transition_frame("l-pop", false, None, Still::MOVING, None)
            .opacity,
        0.0
    );

    // Прерванный вход: уход от текущих значений.
    let origin = enter("l-pop", 50.0);
    let back = registry.transition_frame("l-pop", false, Some(0.0), Still::MOVING, Some(origin));
    close(back.opacity, origin.opacity, "от текущего");

    assert_eq!(
        registry.transition_length("l-pop", true, Still::MOVING),
        260.0
    );
    assert_eq!(
        registry.transition_length("l-pop", false, Still::MOVING),
        160.0
    );
    assert_eq!(
        registry.transition_length("slide", true, Still::MOVING),
        420.0
    );
    assert_eq!(registry.transition_length("l-pop", true, still), 0.0);

    close(registry.theme_fade_opacity(Some(210.0)), 1.0 - e, "тема");
    assert_eq!(registry.theme_fade_opacity(None), 0.0);
}
