//! Скан-код Windows → код UI Events (`KeyboardEvent.code`), как у Chromium: скан-код из
//! `lParam` (биты 16–23), расширенная клавиша (бит 24) — `0xE000 | код`
//! (`GetScanCodeFromLParam` и столбец win в `dom_code_data.inc`). Это та же таблица, по которой
//! winit давал `KeyCode` в `winit_input.rs` slint. Клавиши, которых движок не знает (мультимедиа,
//! F13–F24, японские), — пустой код, как незнакомая клавиша.

/// Код UI Events по скан-коду; незнакомый — пустая строка.
pub fn code_name(scancode: u16) -> &'static str {
    match scancode {
        0x0001 => "Escape",
        0x0002 => "Digit1",
        0x0003 => "Digit2",
        0x0004 => "Digit3",
        0x0005 => "Digit4",
        0x0006 => "Digit5",
        0x0007 => "Digit6",
        0x0008 => "Digit7",
        0x0009 => "Digit8",
        0x000A => "Digit9",
        0x000B => "Digit0",
        0x000C => "Minus",
        0x000D => "Equal",
        0x000E => "Backspace",
        0x000F => "Tab",
        0x0010 => "KeyQ",
        0x0011 => "KeyW",
        0x0012 => "KeyE",
        0x0013 => "KeyR",
        0x0014 => "KeyT",
        0x0015 => "KeyY",
        0x0016 => "KeyU",
        0x0017 => "KeyI",
        0x0018 => "KeyO",
        0x0019 => "KeyP",
        0x001A => "BracketLeft",
        0x001B => "BracketRight",
        0x001C => "Enter",
        0x001D => "ControlLeft",
        0x001E => "KeyA",
        0x001F => "KeyS",
        0x0020 => "KeyD",
        0x0021 => "KeyF",
        0x0022 => "KeyG",
        0x0023 => "KeyH",
        0x0024 => "KeyJ",
        0x0025 => "KeyK",
        0x0026 => "KeyL",
        0x0027 => "Semicolon",
        0x0028 => "Quote",
        0x0029 => "Backquote",
        0x002A => "ShiftLeft",
        0x002B => "Backslash",
        0x002C => "KeyZ",
        0x002D => "KeyX",
        0x002E => "KeyC",
        0x002F => "KeyV",
        0x0030 => "KeyB",
        0x0031 => "KeyN",
        0x0032 => "KeyM",
        0x0033 => "Comma",
        0x0034 => "Period",
        0x0035 => "Slash",
        0x0036 => "ShiftRight",
        0x0037 => "NumpadMultiply",
        0x0038 => "AltLeft",
        0x0039 => "Space",
        0x003A => "CapsLock",
        0x003B => "F1",
        0x003C => "F2",
        0x003D => "F3",
        0x003E => "F4",
        0x003F => "F5",
        0x0040 => "F6",
        0x0041 => "F7",
        0x0042 => "F8",
        0x0043 => "F9",
        0x0044 => "F10",
        // Pause шлёт E1 1D 45: префикс E1 в lParam теряется. Num Lock — 0xE045.
        0x0045 => "Pause",
        0x0046 => "ScrollLock",
        0x0047 => "Numpad7",
        0x0048 => "Numpad8",
        0x0049 => "Numpad9",
        0x004A => "NumpadSubtract",
        0x004B => "Numpad4",
        0x004C => "Numpad5",
        0x004D => "Numpad6",
        0x004E => "NumpadAdd",
        0x004F => "Numpad1",
        0x0050 => "Numpad2",
        0x0051 => "Numpad3",
        0x0052 => "Numpad0",
        0x0053 => "NumpadDecimal",
        0x0056 => "IntlBackslash",
        0x0057 => "F11",
        0x0058 => "F12",
        0xE01C => "NumpadEnter",
        0xE01D => "ControlRight",
        0xE035 => "NumpadDivide",
        0xE037 => "PrintScreen",
        0xE038 => "AltRight",
        0xE045 => "NumLock",
        0xE047 => "Home",
        0xE048 => "ArrowUp",
        0xE049 => "PageUp",
        0xE04B => "ArrowLeft",
        0xE04D => "ArrowRight",
        0xE04F => "End",
        0xE050 => "ArrowDown",
        0xE051 => "PageDown",
        0xE052 => "Insert",
        0xE053 => "Delete",
        0xE05B => "MetaLeft",
        0xE05C => "MetaRight",
        0xE05D => "ContextMenu",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::code_name;
    use hlsr_ui::keyboard_layout::{Engine, InputEvent, key_from_event, layout_for};

    fn all_scancodes() -> impl Iterator<Item = u16> {
        (0..=0xFF).chain(0xE000..=0xE0FF)
    }

    fn key(scancode: u16, engine: Engine) -> Option<&'static str> {
        key_from_event(
            &InputEvent::Key {
                code: code_name(scancode),
            },
            engine,
        )
    }

    #[test]
    fn scancodes_name_like_chromium() {
        for (scancode, code, source, goldsrc) in [
            (0x0036, "ShiftRight", "RSHIFT", "SHIFT"),
            (0x002A, "ShiftLeft", "SHIFT", "SHIFT"),
            (0xE01D, "ControlRight", "RCTRL", "CTRL"),
            (0xE038, "AltRight", "RALT", "ALT"),
            (0xE01C, "NumpadEnter", "KP_ENTER", "KP_ENTER"),
            (0x001C, "Enter", "ENTER", "ENTER"),
            (0x0052, "Numpad0", "KP_INS", "KP_INS"),
            (0xE052, "Insert", "INS", "INS"),
            (0x0037, "NumpadMultiply", "KP_MULTIPLY", "*"),
            (0xE035, "NumpadDivide", "KP_SLASH", "KP_SLASH"),
            (0xE045, "NumLock", "NUMLOCK", ""),
            (0x0045, "Pause", "PAUSE", "PAUSE"),
            (0xE05B, "MetaLeft", "LWIN", ""),
            (0xE05D, "ContextMenu", "APP", ""),
            (0x0029, "Backquote", "`", "`"),
            (0x001E, "KeyA", "a", "a"),
            (0x000B, "Digit0", "0", "0"),
            (0x0058, "F12", "F12", "F12"),
        ] {
            assert_eq!(code_name(scancode), code, "{scancode:#06x}");
            assert_eq!(
                key(scancode, Engine::Source),
                Some(source).filter(|k| !k.is_empty())
            );
            assert_eq!(
                key(scancode, Engine::GoldSrc),
                Some(goldsrc).filter(|k| !k.is_empty())
            );
        }
        // Мультимедиа, F13 и скан-код 0 — незнакомы: захват ждёт дальше.
        for scancode in [0x0000, 0x0064, 0xE022, 0xE0FF] {
            assert_eq!(code_name(scancode), "");
        }
    }

    #[test]
    fn every_code_is_unique() {
        let mut seen = std::collections::HashMap::new();
        for scancode in all_scancodes() {
            let code = code_name(scancode);
            if !code.is_empty()
                && let Some(other) = seen.insert(code, scancode)
            {
                panic!("{code}: {other:#06x} и {scancode:#06x}");
            }
        }
    }

    #[test]
    fn every_board_key_has_a_scancode() {
        // Любую клавишу схемы «Клавиатуры» можно назначить нажатием (мышь и колесо — не скан-код).
        for engine in [Engine::GoldSrc, Engine::Source] {
            for &name in &layout_for(engine).board_keys {
                if name.starts_with("MOUSE") || name.starts_with("MWHEEL") {
                    continue;
                }
                assert!(
                    all_scancodes().any(|scancode| key(scancode, engine) == Some(name)),
                    "{engine:?}: {name}"
                );
            }
        }
    }
}
