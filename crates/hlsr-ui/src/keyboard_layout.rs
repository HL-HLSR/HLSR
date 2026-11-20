//! Раскладка клавиатуры для вкладки «Клавиатура» и имена клавиш движка
//! (`renderer/components/Launcher/keyboard-layout.js` 0.4.7).
//!
//! Координаты — в долях ширины обычной клавиши (u): x, y, ширина, высота. `key` — имя клавиши
//! в движке (`hlsr_delivery::game_binds`); `None` — клавишу игра не различает, и забиндить её
//! нельзя. Для GoldSrc и старого Source левые и правые Shift/Ctrl/Alt — одна клавиша; Source с
//! inputsystem различает их, а также Win, Menu, Num Lock и Scroll Lock ([`SOURCE_KEYS`]).
//!
//! Нажатие разбирается по физическому коду (`event.code` в 0.4.7; из winit — `hlsr-app`,
//! `winit_input::code_name`):
//! от раскладки (русская, AZERTY) имя не зависит, как и в самой игре.

use std::sync::OnceLock;

/// Ширина схемы в u.
pub const BOARD_WIDTH: f64 = 26.0;
/// Правый край основного блока (F-ряд, буквы, пробел): правее — стрелки, Ins/Home, нумпад и мышь
/// (`MAIN_WIDTH` 0.4.9).
pub const MAIN_WIDTH: f64 = 15.0;
/// Высота схемы в u.
pub const BOARD_HEIGHT: f64 = 6.25;

/// Клавиша схемы.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyCap {
    /// Подпись; начинается с `#` — ключ перевода.
    pub label: String,
    /// Имя в движке; `None` — не забиндить.
    pub key: Option<&'static str>,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// `${key || label}@${x},${y}` по схеме GoldSrc (у Source тот же).
    pub id: String,
}

/// Схема движка: клавиши, множество имён на ней (в порядке первого появления) и ширина в u.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub keys: Vec<KeyCap>,
    pub board_keys: Vec<&'static str>,
    /// [`BOARD_WIDTH`] у всей клавиатуры, [`MAIN_WIDTH`] у основного блока.
    pub width: f64,
}

impl Layout {
    /// `boardKeys.has(key)`.
    pub fn has(&self, key: &str) -> bool {
        self.board_keys.contains(&key)
    }
}

/// Движок клавиатуры: `"source"` — Source с inputsystem, всё остальное — GoldSrc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    GoldSrc,
    Source,
}

impl Engine {
    /// Как `engine === "source"` в 0.4.7.
    pub fn from_name(name: &str) -> Engine {
        if name == "source" {
            Engine::Source
        } else {
            Engine::GoldSrc
        }
    }
}

/// Число как `String(number)` в JS для значений схемы (целые и с дробной частью).
fn js_number(value: f64) -> String {
    format!("{value}")
}

fn k(label: &str, key: Option<&'static str>, x: f64, y: f64, w: f64, h: f64) -> KeyCap {
    let id = format!("{}@{},{}", key.unwrap_or(label), js_number(x), js_number(y));
    KeyCap {
        label: label.to_owned(),
        key,
        x,
        y,
        w,
        h,
        id,
    }
}

/// `row(chars, x, y)`: по клавише на символ, подпись — заглавная.
fn row(chars: &'static str, x: f64, y: f64, out: &mut Vec<KeyCap>) {
    for (n, (i, ch)) in chars.char_indices().enumerate() {
        let key = &chars[i..i + ch.len_utf8()];
        out.push(k(&key.to_uppercase(), Some(key), x + n as f64, y, 1.0, 1.0));
    }
}

fn goldsrc_keys() -> Vec<KeyCap> {
    let mut keys = Vec::with_capacity(112);
    let s = Some;
    let push = |keys: &mut Vec<KeyCap>, label: &str, key, x, y, w, h| {
        keys.push(k(label, key, x, y, w, h));
    };
    // Ряд F.
    push(&mut keys, "Esc", s("ESCAPE"), 0.0, 0.0, 1.0, 1.0);
    for (key, x) in F_KEYS.into_iter().zip([
        2.0, 3.0, 4.0, 5.0, 6.5, 7.5, 8.5, 9.5, 11.0, 12.0, 13.0, 14.0,
    ]) {
        push(&mut keys, key, s(key), x, 0.0, 1.0, 1.0);
    }
    push(&mut keys, "Prt", None, 15.5, 0.0, 1.0, 1.0);
    push(&mut keys, "Scr", None, 16.5, 0.0, 1.0, 1.0);
    push(&mut keys, "Pause", s("PAUSE"), 17.5, 0.0, 1.0, 1.0);

    // Цифры.
    push(&mut keys, "`", s("`"), 0.0, 1.25, 1.0, 1.0);
    row("1234567890-=", 1.0, 1.25, &mut keys);
    push(&mut keys, "Backspace", s("BACKSPACE"), 13.0, 1.25, 2.0, 1.0);
    push(&mut keys, "Ins", s("INS"), 15.5, 1.25, 1.0, 1.0);
    push(&mut keys, "Home", s("HOME"), 16.5, 1.25, 1.0, 1.0);
    push(&mut keys, "PgUp", s("PGUP"), 17.5, 1.25, 1.0, 1.0);
    push(&mut keys, "Num", None, 19.0, 1.25, 1.0, 1.0);
    push(&mut keys, "/", s("KP_SLASH"), 20.0, 1.25, 1.0, 1.0);
    push(&mut keys, "*", s("*"), 21.0, 1.25, 1.0, 1.0);
    push(&mut keys, "−", s("KP_MINUS"), 22.0, 1.25, 1.0, 1.0);

    // QWERTY.
    push(&mut keys, "Tab", s("TAB"), 0.0, 2.25, 1.5, 1.0);
    row("qwertyuiop[]", 1.5, 2.25, &mut keys);
    push(&mut keys, "\\", s("\\"), 13.5, 2.25, 1.5, 1.0);
    push(&mut keys, "Del", s("DEL"), 15.5, 2.25, 1.0, 1.0);
    push(&mut keys, "End", s("END"), 16.5, 2.25, 1.0, 1.0);
    push(&mut keys, "PgDn", s("PGDN"), 17.5, 2.25, 1.0, 1.0);
    push(&mut keys, "7", s("KP_HOME"), 19.0, 2.25, 1.0, 1.0);
    push(&mut keys, "8", s("KP_UPARROW"), 20.0, 2.25, 1.0, 1.0);
    push(&mut keys, "9", s("KP_PGUP"), 21.0, 2.25, 1.0, 1.0);
    push(&mut keys, "+", s("KP_PLUS"), 22.0, 2.25, 1.0, 2.0);

    // ASDF.
    push(&mut keys, "Caps", s("CAPSLOCK"), 0.0, 3.25, 1.75, 1.0);
    row("asdfghjkl;'", 1.75, 3.25, &mut keys);
    push(&mut keys, "Enter", s("ENTER"), 12.75, 3.25, 2.25, 1.0);
    push(&mut keys, "4", s("KP_LEFTARROW"), 19.0, 3.25, 1.0, 1.0);
    push(&mut keys, "5", s("KP_5"), 20.0, 3.25, 1.0, 1.0);
    push(&mut keys, "6", s("KP_RIGHTARROW"), 21.0, 3.25, 1.0, 1.0);

    // ZXCV.
    push(&mut keys, "Shift", s("SHIFT"), 0.0, 4.25, 2.25, 1.0);
    row("zxcvbnm,./", 2.25, 4.25, &mut keys);
    push(&mut keys, "Shift", s("SHIFT"), 12.25, 4.25, 2.75, 1.0);
    push(&mut keys, "↑", s("UPARROW"), 16.5, 4.25, 1.0, 1.0);
    push(&mut keys, "1", s("KP_END"), 19.0, 4.25, 1.0, 1.0);
    push(&mut keys, "2", s("KP_DOWNARROW"), 20.0, 4.25, 1.0, 1.0);
    push(&mut keys, "3", s("KP_PGDN"), 21.0, 4.25, 1.0, 1.0);
    push(&mut keys, "↵", s("KP_ENTER"), 22.0, 4.25, 1.0, 2.0);

    // Нижний ряд.
    push(&mut keys, "Ctrl", s("CTRL"), 0.0, 5.25, 1.25, 1.0);
    push(&mut keys, "Win", None, 1.25, 5.25, 1.25, 1.0);
    push(&mut keys, "Alt", s("ALT"), 2.5, 5.25, 1.25, 1.0);
    push(&mut keys, "Space", s("SPACE"), 3.75, 5.25, 6.25, 1.0);
    push(&mut keys, "Alt", s("ALT"), 10.0, 5.25, 1.25, 1.0);
    push(&mut keys, "Win", None, 11.25, 5.25, 1.25, 1.0);
    push(&mut keys, "Menu", None, 12.5, 5.25, 1.25, 1.0);
    push(&mut keys, "Ctrl", s("CTRL"), 13.75, 5.25, 1.25, 1.0);
    push(&mut keys, "←", s("LEFTARROW"), 15.5, 5.25, 1.0, 1.0);
    push(&mut keys, "↓", s("DOWNARROW"), 16.5, 5.25, 1.0, 1.0);
    push(&mut keys, "→", s("RIGHTARROW"), 17.5, 5.25, 1.0, 1.0);
    push(&mut keys, "0", s("KP_INS"), 19.0, 5.25, 2.0, 1.0);
    push(&mut keys, ".", s("KP_DEL"), 21.0, 5.25, 1.0, 1.0);

    // Мышь.
    push(
        &mut keys,
        "#L_KB_MOUSE1",
        s("MOUSE1"),
        23.5,
        1.25,
        1.25,
        1.0,
    );
    push(
        &mut keys,
        "#L_KB_MOUSE2",
        s("MOUSE2"),
        24.75,
        1.25,
        1.25,
        1.0,
    );
    push(
        &mut keys,
        "#L_KB_WHEELUP",
        s("MWHEELUP"),
        23.5,
        2.25,
        1.25,
        1.0,
    );
    push(
        &mut keys,
        "#L_KB_WHEELDOWN",
        s("MWHEELDOWN"),
        24.75,
        2.25,
        1.25,
        1.0,
    );
    push(&mut keys, "#L_KB_MOUSE3", s("MOUSE3"), 23.5, 3.25, 2.5, 1.0);
    push(&mut keys, "M4", s("MOUSE4"), 23.5, 4.25, 1.25, 1.0);
    push(&mut keys, "M5", s("MOUSE5"), 24.75, 4.25, 1.25, 1.0);
    keys
}

/// Клавиши, которые Source с inputsystem зовёт иначе: по id клавиши схемы.
pub const SOURCE_KEYS: [(&str, &str); 9] = [
    ("Scr@16.5,0", "SCROLLLOCK"),
    ("Num@19,1.25", "NUMLOCK"),
    ("*@21,1.25", "KP_MULTIPLY"),
    ("SHIFT@12.25,4.25", "RSHIFT"),
    ("Win@1.25,5.25", "LWIN"),
    ("ALT@10,5.25", "RALT"),
    ("Win@11.25,5.25", "RWIN"),
    ("Menu@12.5,5.25", "APP"),
    ("CTRL@13.75,5.25", "RCTRL"),
];

fn build_layout(engine: Engine, full: bool) -> Layout {
    let mut keys = goldsrc_keys();
    if engine == Engine::Source {
        for cap in &mut keys {
            if let Some((_, key)) = SOURCE_KEYS.iter().find(|(id, _)| *id == cap.id) {
                cap.key = Some(key);
            }
        }
    }
    if !full {
        keys.retain(|cap| cap.x + cap.w <= MAIN_WIDTH);
    }
    let mut board_keys: Vec<&'static str> = Vec::new();
    for key in keys.iter().filter_map(|cap| cap.key) {
        if !board_keys.contains(&key) {
            board_keys.push(key);
        }
    }
    Layout {
        keys,
        board_keys,
        width: if full { BOARD_WIDTH } else { MAIN_WIDTH },
    }
}

/// `layoutFor(engine)`: вся клавиатура движка (строится один раз).
pub fn layout_for(engine: Engine) -> &'static Layout {
    layout_of(engine, true)
}

/// `layoutFor(engine, { full })`: `full: false` — только основной блок (клавиши с
/// `x + w <= MAIN_WIDTH`); `board_keys` и `width` — этой схемы.
pub fn layout_of(engine: Engine, full: bool) -> &'static Layout {
    static LAYOUTS: [OnceLock<Layout>; 4] = [const { OnceLock::new() }; 4];
    let index = usize::from(engine == Engine::Source) * 2 + usize::from(!full);
    LAYOUTS[index].get_or_init(|| build_layout(engine, full))
}

/// Подпись клавиши для текста: «A», «Num 8», «Колесо вверх».
const KEY_NAMES: [(&str, &str); 45] = [
    ("ESCAPE", "Esc"),
    ("BACKSPACE", "Backspace"),
    ("TAB", "Tab"),
    ("ENTER", "Enter"),
    ("SPACE", "Space"),
    ("CAPSLOCK", "Caps Lock"),
    ("SHIFT", "Shift"),
    ("CTRL", "Ctrl"),
    ("ALT", "Alt"),
    ("PAUSE", "Pause"),
    ("INS", "Insert"),
    ("DEL", "Delete"),
    ("HOME", "Home"),
    ("END", "End"),
    ("PGUP", "Page Up"),
    ("PGDN", "Page Down"),
    ("UPARROW", "↑"),
    ("DOWNARROW", "↓"),
    ("LEFTARROW", "←"),
    ("RIGHTARROW", "→"),
    ("KP_SLASH", "Num /"),
    ("*", "Num *"),
    ("KP_MINUS", "Num −"),
    ("KP_PLUS", "Num +"),
    ("KP_ENTER", "Num Enter"),
    ("KP_HOME", "Num 7"),
    ("KP_UPARROW", "Num 8"),
    ("KP_PGUP", "Num 9"),
    ("KP_LEFTARROW", "Num 4"),
    ("KP_5", "Num 5"),
    ("KP_RIGHTARROW", "Num 6"),
    ("KP_END", "Num 1"),
    ("KP_DOWNARROW", "Num 2"),
    ("KP_PGDN", "Num 3"),
    ("KP_INS", "Num 0"),
    ("KP_DEL", "Num ."),
    ("KP_MULTIPLY", "Num *"),
    ("NUMLOCK", "Num Lock"),
    ("SCROLLLOCK", "Scroll Lock"),
    ("RSHIFT", "Right Shift"),
    ("RCTRL", "Right Ctrl"),
    ("RALT", "Right Alt"),
    ("LWIN", "Win"),
    ("RWIN", "Right Win"),
    ("APP", "Menu"),
];

/// Кнопки мыши и колесо: подпись — ключ перевода.
const KEY_NAME_IDS: [(&str, &str); 7] = [
    ("MOUSE1", "#L_KB_MOUSE1_FULL"),
    ("MOUSE2", "#L_KB_MOUSE2_FULL"),
    ("MOUSE3", "#L_KB_MOUSE3_FULL"),
    ("MOUSE4", "#L_KB_MOUSE4_FULL"),
    ("MOUSE5", "#L_KB_MOUSE5_FULL"),
    ("MWHEELUP", "#L_KB_WHEELUP_FULL"),
    ("MWHEELDOWN", "#L_KB_WHEELDOWN_FULL"),
];

fn lookup(table: &[(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, value)| *value)
}

/// `keyName(key, t)`: `t` — перевод ключа (`$t`).
pub fn key_name(key: &str, t: impl Fn(&str) -> String) -> String {
    if let Some(id) = lookup(&KEY_NAME_IDS, key) {
        return t(id);
    }
    if let Some(name) = lookup(&KEY_NAMES, key) {
        return name.to_owned();
    }
    // key.length === 1: одна кодовая единица UTF-16.
    if key.encode_utf16().count() == 1 {
        key.to_uppercase()
    } else {
        key.to_owned()
    }
}

/// Нажатая клавиша → имя в движке, по `event.code`.
const CODE_KEYS: [(&str, &str); 50] = [
    ("Backquote", "`"),
    ("Minus", "-"),
    ("Equal", "="),
    ("BracketLeft", "["),
    ("BracketRight", "]"),
    ("Backslash", "\\"),
    ("Semicolon", ";"),
    ("Quote", "'"),
    ("Comma", ","),
    ("Period", "."),
    ("Slash", "/"),
    ("Space", "SPACE"),
    ("Enter", "ENTER"),
    ("Tab", "TAB"),
    ("Backspace", "BACKSPACE"),
    ("Escape", "ESCAPE"),
    ("CapsLock", "CAPSLOCK"),
    ("Pause", "PAUSE"),
    ("ShiftLeft", "SHIFT"),
    ("ShiftRight", "SHIFT"),
    ("ControlLeft", "CTRL"),
    ("ControlRight", "CTRL"),
    ("AltLeft", "ALT"),
    ("AltRight", "ALT"),
    ("Insert", "INS"),
    ("Delete", "DEL"),
    ("Home", "HOME"),
    ("End", "END"),
    ("PageUp", "PGUP"),
    ("PageDown", "PGDN"),
    ("ArrowUp", "UPARROW"),
    ("ArrowDown", "DOWNARROW"),
    ("ArrowLeft", "LEFTARROW"),
    ("ArrowRight", "RIGHTARROW"),
    ("Numpad0", "KP_INS"),
    ("Numpad1", "KP_END"),
    ("Numpad2", "KP_DOWNARROW"),
    ("Numpad3", "KP_PGDN"),
    ("Numpad4", "KP_LEFTARROW"),
    ("Numpad5", "KP_5"),
    ("Numpad6", "KP_RIGHTARROW"),
    ("Numpad7", "KP_HOME"),
    ("Numpad8", "KP_UPARROW"),
    ("Numpad9", "KP_PGUP"),
    ("NumpadDecimal", "KP_DEL"),
    ("NumpadDivide", "KP_SLASH"),
    ("NumpadMultiply", "*"),
    ("NumpadSubtract", "KP_MINUS"),
    ("NumpadAdd", "KP_PLUS"),
    ("NumpadEnter", "KP_ENTER"),
];

/// Source с inputsystem различает правые модификаторы (см. [`SOURCE_KEYS`]).
const SOURCE_CODE_KEYS: [(&str, &str); 9] = [
    ("ShiftRight", "RSHIFT"),
    ("ControlRight", "RCTRL"),
    ("AltRight", "RALT"),
    ("MetaLeft", "LWIN"),
    ("MetaRight", "RWIN"),
    ("ContextMenu", "APP"),
    ("NumLock", "NUMLOCK"),
    ("ScrollLock", "SCROLLLOCK"),
    ("NumpadMultiply", "KP_MULTIPLY"),
];

/// Кнопки мыши по `MouseEvent.button` (0 — левая, 1 — средняя, 2 — правая, 3 — назад,
/// 4 — вперёд).
const MOUSE_KEYS: [&str; 5] = ["MOUSE1", "MOUSE3", "MOUSE2", "MOUSE4", "MOUSE5"];

const LETTER_KEYS: [&str; 26] = [
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s",
    "t", "u", "v", "w", "x", "y", "z",
];
const DIGIT_KEYS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
const F_KEYS: [&str; 12] = [
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
];

/// Событие ввода в форме, которую разбирает `keyFromEvent`.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent<'a> {
    /// `keydown`/`keyup`: `event.code` (нет кода — пустая строка).
    Key { code: &'a str },
    /// `mousedown`/`mouseup`: `event.button`.
    MouseButton { button: i32 },
    /// `wheel`: `event.deltaY` (вниз — больше нуля).
    Wheel { delta_y: f64 },
}

/// Имя клавиши по коду нажатия.
fn key_from_code(code: &str, engine: Engine) -> Option<&'static str> {
    if engine == Engine::Source
        && let Some(key) = lookup(&SOURCE_CODE_KEYS, code)
    {
        return Some(key);
    }
    // /^Key([A-Z])$/
    if let Some(&[letter]) = code.strip_prefix("Key").map(str::as_bytes)
        && letter.is_ascii_uppercase()
    {
        return Some(LETTER_KEYS[usize::from(letter - b'A')]);
    }
    // /^Digit(\d)$/
    if let Some(&[digit]) = code.strip_prefix("Digit").map(str::as_bytes)
        && digit.is_ascii_digit()
    {
        return Some(DIGIT_KEYS[usize::from(digit - b'0')]);
    }
    // /^F(\d{1,2})$/ и 1 ≤ N ≤ 12: результат — сам код. Коды с ведущим нулём («F01») регулярное
    // выражение 0.4.7 тоже пропускает, но Chromium их не даёт — здесь они незнакомы.
    if let Some(key) = F_KEYS.iter().copied().find(|key| *key == code) {
        return Some(key);
    }
    lookup(&CODE_KEYS, code)
}

/// `keyFromEvent(event, engine)`: нажатие, кнопка мыши или колесо → имя клавиши или `None`.
pub fn key_from_event(event: &InputEvent<'_>, engine: Engine) -> Option<&'static str> {
    match *event {
        InputEvent::Wheel { delta_y } => {
            if delta_y < 0.0 {
                Some("MWHEELUP")
            } else if delta_y > 0.0 {
                Some("MWHEELDOWN")
            } else {
                None
            }
        }
        InputEvent::MouseButton { button } => usize::try_from(button)
            .ok()
            .and_then(|index| MOUSE_KEYS.get(index).copied()),
        InputEvent::Key { code } => key_from_code(code, engine),
    }
}
