//! События GPUI → ввод захвата ([`RawInput`]) для [`super::feed`]. Нужны там, где перехвата
//! сообщений окна нет (Linux): вид передаёт сюда свои `on_key_down`, `on_modifiers_changed`,
//! `on_mouse_down`, `on_scroll`. На Windows перехват (`win32.rs`) видит ввод раньше GPUI:
//! нажатия захвата до вида не доходят, а щелчок и колесо после итога уже ничего не меняют.
//!
//! shortcut: `Keystroke` GPUI несёт имя клавиши, а не физический код — сторона Shift/Ctrl/Alt
//! (всегда левая), цифры и стрелки цифрового блока (как основные), Caps Lock и Num Lock (только
//! состояние) не различаются; буквы — по раскладке (у xkb для нелатинской раскладки GPUI сам
//! подбирает латиницу). Точнее — коды клавиш xkb/evdev из бэкенда Linux, когда дойдёт до живой
//! проверки на Linux.

use gpui::{
    KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, ModifiersChangedEvent, MouseButton,
    MouseDownEvent, NavigationDirection, Pixels, Point, ScrollDelta, ScrollEvent,
};
use hlsr_ui::keys::RawInput;

const LETTERS: [&str; 26] = [
    "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI", "KeyJ", "KeyK", "KeyL",
    "KeyM", "KeyN", "KeyO", "KeyP", "KeyQ", "KeyR", "KeyS", "KeyT", "KeyU", "KeyV", "KeyW", "KeyX",
    "KeyY", "KeyZ",
];
const DIGITS: [&str; 10] = [
    "Digit0", "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8",
    "Digit9",
];
const F_KEYS: [&str; 12] = [
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
];

/// Код UI Events по имени клавиши GPUI; незнакомое — пустая строка.
pub fn key_code(keystroke: &Keystroke) -> &'static str {
    let key = keystroke.key.as_str();
    match key {
        "space" => "Space",
        "enter" => "Enter",
        "tab" => "Tab",
        "backspace" => "Backspace",
        "escape" => "Escape",
        "insert" => "Insert",
        "delete" => "Delete",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        "pause" => "Pause",
        "scroll_lock" => "ScrollLock",
        "print" => "PrintScreen",
        "menu" => "ContextMenu",
        "`" => "Backquote",
        "-" => "Minus",
        "=" => "Equal",
        "[" => "BracketLeft",
        "]" => "BracketRight",
        "\\" => "Backslash",
        ";" => "Semicolon",
        "'" => "Quote",
        "," => "Comma",
        "." => "Period",
        "/" => "Slash",
        _ => match *key.as_bytes() {
            [letter @ b'a'..=b'z'] => LETTERS[usize::from(letter - b'a')],
            [digit @ b'0'..=b'9'] => DIGITS[usize::from(digit - b'0')],
            _ => key
                .strip_prefix('f')
                .and_then(|number| number.parse::<usize>().ok())
                .and_then(|number| F_KEYS.get(number.wrapping_sub(1)).copied())
                .unwrap_or(""),
        },
    }
}

/// Нажатие клавиши (`keydown`).
pub fn key_down(event: &KeyDownEvent) -> RawInput {
    RawInput::KeyDown {
        code: key_code(&event.keystroke).to_owned(),
        repeat: event.is_held,
    }
}

/// Отпускание клавиши (`keyup`).
pub fn key_up(event: &KeyUpEvent) -> RawInput {
    RawInput::KeyUp {
        code: key_code(&event.keystroke).to_owned(),
    }
}

/// Shift, Ctrl, Alt и Win GPUI шлёт только сменой модификаторов: нажаты или отпущены те, что
/// поменялись, — за одно событие их может быть несколько (`before` — модификаторы до события).
pub fn modifiers_changed(before: Modifiers, event: &ModifiersChangedEvent) -> Vec<RawInput> {
    let after = event.modifiers;
    [
        (before.shift, after.shift, "ShiftLeft"),
        (before.control, after.control, "ControlLeft"),
        (before.alt, after.alt, "AltLeft"),
        (before.platform, after.platform, "MetaLeft"),
    ]
    .into_iter()
    .filter(|(was, is, _)| was != is)
    .map(|(_, pressed, code)| {
        let code = code.to_owned();
        if pressed {
            RawInput::KeyDown {
                code,
                repeat: false,
            }
        } else {
            RawInput::KeyUp { code }
        }
    })
    .collect()
}

/// Кнопка мыши (`MouseEvent.button`: 0 — левая, 1 — средняя, 2 — правая, 3 — назад, 4 — вперёд).
pub fn mouse_down(event: &MouseDownEvent) -> RawInput {
    let button = match event.button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::Navigate(NavigationDirection::Back) => 3,
        MouseButton::Navigate(NavigationDirection::Forward) => 4,
    };
    let (x, y) = xy(event.position);
    RawInput::MouseDown { button, x, y }
}

/// Колесо (`WheelEvent.deltaY`): у GPUI «от себя» — плюс, у DOM — минус. Важен только знак.
pub fn scroll_wheel(event: &ScrollEvent) -> RawInput {
    let delta_y = match event.delta {
        ScrollDelta::Lines(lines) => -f64::from(lines.y),
        ScrollDelta::Pixels(pixels) => -f64::from(f32::from(pixels.y)),
    };
    let (x, y) = xy(event.position);
    RawInput::Wheel { delta_y, x, y }
}

fn xy(point: Point<Pixels>) -> (f64, f64) {
    (f64::from(f32::from(point.x)), f64::from(f32::from(point.y)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::point;
    use hlsr_ui::keyboard_layout::{Engine, InputEvent, key_from_event};

    fn named(key: &str) -> &'static str {
        key_code(&Keystroke {
            key: key.to_owned(),
            key_char: None,
            modifiers: Modifiers::default(),
        })
    }

    #[test]
    fn keystrokes_name_like_ui_events() {
        for (key, code) in [
            ("a", "KeyA"),
            ("z", "KeyZ"),
            ("7", "Digit7"),
            ("f1", "F1"),
            ("f12", "F12"),
            ("space", "Space"),
            ("enter", "Enter"),
            ("up", "ArrowUp"),
            ("pagedown", "PageDown"),
            ("`", "Backquote"),
            ("\\", "Backslash"),
            ("menu", "ContextMenu"),
            ("f13", ""),
            ("f0", ""),
            ("ф", ""),
            ("back", ""),
        ] {
            assert_eq!(named(key), code, "{key}");
        }
        assert_eq!(
            key_from_event(&InputEvent::Key { code: named("tab") }, Engine::GoldSrc),
            Some("TAB")
        );
    }

    #[test]
    fn modifiers_buttons_and_wheel_like_dom() {
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let changed = |modifiers| ModifiersChangedEvent {
            modifiers,
            ..Default::default()
        };
        assert_eq!(
            modifiers_changed(Modifiers::default(), &changed(shift)),
            [RawInput::KeyDown {
                code: "ShiftLeft".into(),
                repeat: false
            }]
        );
        assert_eq!(
            modifiers_changed(shift, &changed(Modifiers::default())),
            [RawInput::KeyUp {
                code: "ShiftLeft".into()
            }]
        );
        assert!(modifiers_changed(shift, &changed(shift)).is_empty());
        // Shift отпущен и Ctrl нажат одним событием — оба перехода.
        let control = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        assert_eq!(
            modifiers_changed(shift, &changed(control)),
            [
                RawInput::KeyUp {
                    code: "ShiftLeft".into()
                },
                RawInput::KeyDown {
                    code: "ControlLeft".into(),
                    repeat: false
                }
            ]
        );

        let down = |button| MouseDownEvent {
            button,
            position: point(10_f32.into(), 20_f32.into()),
            ..Default::default()
        };
        assert_eq!(
            mouse_down(&down(MouseButton::Navigate(NavigationDirection::Back))),
            RawInput::MouseDown {
                button: 3,
                x: 10.0,
                y: 20.0
            }
        );
        assert!(matches!(
            mouse_down(&down(MouseButton::Navigate(NavigationDirection::Forward))),
            RawInput::MouseDown { button: 4, .. }
        ));
        let wheel = scroll_wheel(&ScrollEvent {
            delta: ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        });
        let RawInput::Wheel { delta_y, .. } = wheel else {
            panic!("{wheel:?}")
        };
        assert_eq!(
            key_from_event(&InputEvent::Wheel { delta_y }, Engine::GoldSrc),
            Some("MWHEELUP")
        );
    }
}
