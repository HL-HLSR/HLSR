//! Захват клавиш и секретная последовательность (`GameScripts.vue:436-482`,
//! `LauncherShell.vue:675-686` 0.4.7) без окна.

use hlsr_ui::keyboard_layout::Engine;
use hlsr_ui::keys::{Area, CaptureEnd, KeyInput, RawInput, SECRET_SEQUENCE, Verdict};

const BUTTON: Area = Area {
    x: 100.0,
    y: 50.0,
    width: 120.0,
    height: 32.0,
};

fn down(code: &str) -> RawInput {
    RawInput::KeyDown {
        code: code.into(),
        repeat: false,
    }
}

fn up(code: &str) -> RawInput {
    RawInput::KeyUp { code: code.into() }
}

fn end(key: Option<&'static str>) -> Option<CaptureEnd> {
    Some(CaptureEnd {
        owner: "+attack".into(),
        key,
    })
}

fn capturing(engine: Engine) -> KeyInput {
    let mut input = KeyInput::new();
    input.start_capture("+attack", engine, BUTTON);
    input
}

#[test]
fn key_ends_capture_by_physical_code() {
    let mut input = capturing(Engine::GoldSrc);
    // Буква — по коду клавиши, не по раскладке.
    let handled = input.handle(&down("KeyQ"), false);
    assert_eq!(handled.verdict, Verdict::Consume);
    assert_eq!(handled.capture, end(Some("q")));
    assert_eq!(input.capturing(), None);
    // Отпускание поглощённой клавиши тоже не уходит в окно; следующее — уходит.
    assert_eq!(input.handle(&up("KeyQ"), false).verdict, Verdict::Consume);
    assert_eq!(input.handle(&up("KeyQ"), false).verdict, Verdict::Pass);
}

#[test]
fn modifiers_and_numpad_by_engine() {
    for (code, goldsrc, source) in [
        ("ShiftRight", "SHIFT", "RSHIFT"),
        ("ControlRight", "CTRL", "RCTRL"),
        ("AltRight", "ALT", "RALT"),
        ("ShiftLeft", "SHIFT", "SHIFT"),
        ("Numpad8", "KP_UPARROW", "KP_UPARROW"),
        ("NumpadMultiply", "*", "KP_MULTIPLY"),
        ("NumpadEnter", "KP_ENTER", "KP_ENTER"),
    ] {
        for (engine, want) in [(Engine::GoldSrc, goldsrc), (Engine::Source, source)] {
            let mut input = capturing(engine);
            assert_eq!(
                input.handle(&down(code), false).capture,
                end(Some(want)),
                "{code}"
            );
        }
    }
    // Win у GoldSrc не различается: ожидание продолжается.
    let mut input = capturing(Engine::GoldSrc);
    let handled = input.handle(&down("MetaLeft"), false);
    assert_eq!((handled.verdict, handled.capture), (Verdict::Consume, None));
    assert_eq!(input.capturing(), Some("+attack"));
    assert_eq!(
        input.handle(&down("MetaLeft"), false).capture,
        None,
        "незнакомая клавиша — ждать дальше"
    );
    let mut input = capturing(Engine::Source);
    assert_eq!(
        input.handle(&down("MetaLeft"), false).capture,
        end(Some("LWIN"))
    );
}

#[test]
fn escape_cancels_and_tab_is_a_key() {
    let mut input = capturing(Engine::GoldSrc);
    let handled = input.handle(&down("Escape"), false);
    assert_eq!(handled.verdict, Verdict::Consume, "Esc не закрывает окно");
    assert_eq!(handled.capture, end(None));
    // Без ожидания Esc уходит в окно.
    assert_eq!(input.handle(&up("Escape"), false).verdict, Verdict::Consume);
    assert_eq!(input.handle(&down("Escape"), false).verdict, Verdict::Pass);

    let mut input = capturing(Engine::GoldSrc);
    let handled = input.handle(&down("Tab"), false);
    assert_eq!(handled.verdict, Verdict::Consume, "Tab не уводит фокус");
    assert_eq!(handled.capture, end(Some("TAB")));
}

#[test]
fn mouse_over_button_is_a_key_and_outside_cancels() {
    for (button, key) in [
        (0, Some("MOUSE1")),
        (1, Some("MOUSE3")),
        (2, Some("MOUSE2")),
        (3, Some("MOUSE4")),
        (4, Some("MOUSE5")),
        (5, None),
    ] {
        let mut input = capturing(Engine::GoldSrc);
        let handled = input.handle(
            &RawInput::MouseDown {
                button,
                x: 150.0,
                y: 60.0,
            },
            false,
        );
        // preventDefault у mousedown не отменяет щелчка: нажатие уходит в окно.
        assert_eq!(handled.verdict, Verdict::Pass);
        assert_eq!(handled.capture, end(key), "кнопка {button}");
    }
    let mut input = capturing(Engine::GoldSrc);
    let handled = input.handle(
        &RawInput::MouseDown {
            button: 3,
            x: 10.0,
            y: 10.0,
        },
        false,
    );
    assert_eq!(
        (handled.verdict, handled.capture),
        (Verdict::Pass, end(None))
    );
    // Без ожидания мышь просто уходит в окно.
    let handled = input.handle(
        &RawInput::MouseDown {
            button: 0,
            x: 150.0,
            y: 60.0,
        },
        false,
    );
    assert_eq!((handled.verdict, handled.capture), (Verdict::Pass, None));
}

#[test]
fn wheel_over_button_only() {
    let wheel = |delta_y, x| RawInput::Wheel {
        delta_y,
        x,
        y: 60.0,
    };
    let mut input = capturing(Engine::GoldSrc);
    // Вне кнопки колесо прокручивает, ожидание идёт.
    let handled = input.handle(&wheel(-120.0, 10.0), false);
    assert_eq!((handled.verdict, handled.capture), (Verdict::Pass, None));
    let handled = input.handle(&wheel(-120.0, 150.0), false);
    assert_eq!(handled.verdict, Verdict::Consume);
    assert_eq!(handled.capture, end(Some("MWHEELUP")));

    let mut input = capturing(Engine::GoldSrc);
    assert_eq!(
        input.handle(&wheel(3.0, 150.0), false).capture,
        end(Some("MWHEELDOWN"))
    );
    // Только горизонтальная прокрутка: ожидание кончается без клавиши.
    let mut input = capturing(Engine::GoldSrc);
    assert_eq!(input.handle(&wheel(0.0, 150.0), false).capture, end(None));
}

#[test]
fn restart_and_stop() {
    let mut input = capturing(Engine::GoldSrc);
    input.start_capture("+jump", Engine::GoldSrc, BUTTON);
    assert_eq!(input.capturing(), Some("+jump"));
    let handled = input.handle(&down("Space"), false);
    assert_eq!(
        handled.capture,
        Some(CaptureEnd {
            owner: "+jump".into(),
            key: Some("SPACE")
        })
    );
    input.start_capture("+duck", Engine::GoldSrc, BUTTON);
    assert_eq!(input.stop_capture().as_deref(), Some("+duck"));
    assert_eq!(input.stop_capture(), None);
    // Кнопка сдвинулась — колесо по новому месту.
    input.start_capture("+duck", Engine::GoldSrc, BUTTON);
    input.set_capture_area(Area {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
    });
    let handled = input.handle(
        &RawInput::Wheel {
            delta_y: 1.0,
            x: 5.0,
            y: 5.0,
        },
        false,
    );
    assert_eq!(handled.capture.unwrap().key, Some("MWHEELDOWN"));
}

#[test]
fn secret_sequence_by_physical_keys() {
    let mut input = KeyInput::new();
    let mut matched = 0;
    // Лишние нажатия до последовательности не мешают (последние 10).
    for code in ["KeyX", "ArrowUp"].into_iter().chain(SECRET_SEQUENCE) {
        let handled = input.handle(&down(code), false);
        assert_eq!(handled.verdict, Verdict::Pass);
        matched += usize::from(handled.secret);
    }
    assert_eq!(matched, 1);
    // После совпадения набор начинается заново.
    for code in &SECRET_SEQUENCE[1..] {
        assert!(!input.handle(&down(code), false).secret);
    }
}

#[test]
fn secret_sequence_resets_when_blocked() {
    let mut input = KeyInput::new();
    for code in &SECRET_SEQUENCE[..5] {
        input.handle(&down(code), false);
    }
    // Нажатие поверх окна или в поле ввода сбрасывает набор.
    input.handle(&down("ArrowRight"), true);
    let mut matched = false;
    for code in &SECRET_SEQUENCE[5..] {
        matched |= input.handle(&down(code), false).secret;
    }
    assert!(!matched);
    for code in SECRET_SEQUENCE {
        matched |= input.handle(&down(code), false).secret;
    }
    assert!(matched);
    // Во время захвата нажатия последовательность не видит.
    let mut input = KeyInput::new();
    for code in &SECRET_SEQUENCE[..9] {
        input.handle(&down(code), false);
    }
    input.start_capture("+use", Engine::GoldSrc, BUTTON);
    assert!(!input.handle(&down("KeyA"), false).secret);
}
