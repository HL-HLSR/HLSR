// Раскладка клавиатуры для вкладки «Клавиатура». Координаты в долях
// ширины обычной клавиши (u): x, y, ширина, высота. key — имя клавиши в
// движке (src/shared/game-binds.js); null — клавишу игра не различает, и
// забиндить её нельзя. Для GoldSrc и старого Source левые и правые
// Shift/Ctrl/Alt — одна клавиша; Source с inputsystem различает их, а также
// Win, Menu, Num Lock и Scroll Lock (SOURCE_KEYS ниже).

const BOARD_WIDTH = 26;
// Правый край основного блока (F-ряд, буквы, пробел): ниже — стрелки, Ins/Home, нумпад и мышь.
const MAIN_WIDTH = 15;
const BOARD_HEIGHT = 6.25;

const k = (label, key, x, y, w = 1, h = 1) => ({ label, key, x, y, w, h, id: `${key || label}@${x},${y}` });

function row(chars, x, y) {
  return chars.split("").map((ch, i) => k(ch.toUpperCase(), ch, x + i, y));
}

const GOLDSRC_KEYS = [
  // Ряд F
  k("Esc", "ESCAPE", 0, 0),
  k("F1", "F1", 2, 0), k("F2", "F2", 3, 0), k("F3", "F3", 4, 0), k("F4", "F4", 5, 0),
  k("F5", "F5", 6.5, 0), k("F6", "F6", 7.5, 0), k("F7", "F7", 8.5, 0), k("F8", "F8", 9.5, 0),
  k("F9", "F9", 11, 0), k("F10", "F10", 12, 0), k("F11", "F11", 13, 0), k("F12", "F12", 14, 0),
  k("Prt", null, 15.5, 0), k("Scr", null, 16.5, 0), k("Pause", "PAUSE", 17.5, 0),

  // Цифры
  k("`", "`", 0, 1.25),
  ...row("1234567890-=", 1, 1.25),
  k("Backspace", "BACKSPACE", 13, 1.25, 2),
  k("Ins", "INS", 15.5, 1.25), k("Home", "HOME", 16.5, 1.25), k("PgUp", "PGUP", 17.5, 1.25),
  k("Num", null, 19, 1.25), k("/", "KP_SLASH", 20, 1.25), k("*", "*", 21, 1.25), k("−", "KP_MINUS", 22, 1.25),

  // QWERTY
  k("Tab", "TAB", 0, 2.25, 1.5),
  ...row("qwertyuiop[]", 1.5, 2.25),
  k("\\", "\\", 13.5, 2.25, 1.5),
  k("Del", "DEL", 15.5, 2.25), k("End", "END", 16.5, 2.25), k("PgDn", "PGDN", 17.5, 2.25),
  k("7", "KP_HOME", 19, 2.25), k("8", "KP_UPARROW", 20, 2.25), k("9", "KP_PGUP", 21, 2.25),
  k("+", "KP_PLUS", 22, 2.25, 1, 2),

  // ASDF
  k("Caps", "CAPSLOCK", 0, 3.25, 1.75),
  ...row("asdfghjkl;'", 1.75, 3.25),
  k("Enter", "ENTER", 12.75, 3.25, 2.25),
  k("4", "KP_LEFTARROW", 19, 3.25), k("5", "KP_5", 20, 3.25), k("6", "KP_RIGHTARROW", 21, 3.25),

  // ZXCV
  k("Shift", "SHIFT", 0, 4.25, 2.25),
  ...row("zxcvbnm,./", 2.25, 4.25),
  k("Shift", "SHIFT", 12.25, 4.25, 2.75),
  k("↑", "UPARROW", 16.5, 4.25),
  k("1", "KP_END", 19, 4.25), k("2", "KP_DOWNARROW", 20, 4.25), k("3", "KP_PGDN", 21, 4.25),
  k("↵", "KP_ENTER", 22, 4.25, 1, 2),

  // Нижний ряд
  k("Ctrl", "CTRL", 0, 5.25, 1.25), k("Win", null, 1.25, 5.25, 1.25), k("Alt", "ALT", 2.5, 5.25, 1.25),
  k("Space", "SPACE", 3.75, 5.25, 6.25),
  k("Alt", "ALT", 10, 5.25, 1.25), k("Win", null, 11.25, 5.25, 1.25), k("Menu", null, 12.5, 5.25, 1.25),
  k("Ctrl", "CTRL", 13.75, 5.25, 1.25),
  k("←", "LEFTARROW", 15.5, 5.25), k("↓", "DOWNARROW", 16.5, 5.25), k("→", "RIGHTARROW", 17.5, 5.25),
  k("0", "KP_INS", 19, 5.25, 2), k(".", "KP_DEL", 21, 5.25),

  // Мышь
  k("#L_KB_MOUSE1", "MOUSE1", 23.5, 1.25, 1.25), k("#L_KB_MOUSE2", "MOUSE2", 24.75, 1.25, 1.25),
  k("#L_KB_WHEELUP", "MWHEELUP", 23.5, 2.25, 1.25), k("#L_KB_WHEELDOWN", "MWHEELDOWN", 24.75, 2.25, 1.25),
  k("#L_KB_MOUSE3", "MOUSE3", 23.5, 3.25, 2.5),
  k("M4", "MOUSE4", 23.5, 4.25, 1.25), k("M5", "MOUSE5", 24.75, 4.25, 1.25),
];

// Клавиши, которые Source с inputsystem зовёт иначе: по id клавиши схемы.
const SOURCE_KEYS = {
  "Scr@16.5,0": "SCROLLLOCK",
  "Num@19,1.25": "NUMLOCK",
  "*@21,1.25": "KP_MULTIPLY",
  "SHIFT@12.25,4.25": "RSHIFT",
  "Win@1.25,5.25": "LWIN",
  "ALT@10,5.25": "RALT",
  "Win@11.25,5.25": "RWIN",
  "Menu@12.5,5.25": "APP",
  "CTRL@13.75,5.25": "RCTRL",
};

const LAYOUTS = {};
// full: false — только основной блок (клавиши с x + w <= MAIN_WIDTH); boardKeys и width — этой раскладки.
function layoutFor(engine, { full = true } = {}) {
  const id = (engine === "source" ? "source" : "goldsrc") + (full ? "" : "-main");
  if (!LAYOUTS[id]) {
    const all =
      engine === "source"
        ? GOLDSRC_KEYS.map((cap) => (SOURCE_KEYS[cap.id] ? Object.assign({}, cap, { key: SOURCE_KEYS[cap.id] }) : cap))
        : GOLDSRC_KEYS;
    const keys = full ? all : all.filter((cap) => cap.x + cap.w <= MAIN_WIDTH);
    LAYOUTS[id] = {
      keys,
      width: full ? BOARD_WIDTH : MAIN_WIDTH,
      boardKeys: new Set(keys.filter((item) => item.key).map((item) => item.key)),
    };
  }
  return LAYOUTS[id];
}

// Подпись клавиши для текста: «A», «Num 8», «Колесо вверх».
const KEY_NAMES = {
  ESCAPE: "Esc", BACKSPACE: "Backspace", TAB: "Tab", ENTER: "Enter", SPACE: "Space",
  CAPSLOCK: "Caps Lock", SHIFT: "Shift", CTRL: "Ctrl", ALT: "Alt", PAUSE: "Pause",
  INS: "Insert", DEL: "Delete", HOME: "Home", END: "End", PGUP: "Page Up", PGDN: "Page Down",
  UPARROW: "↑", DOWNARROW: "↓", LEFTARROW: "←", RIGHTARROW: "→",
  KP_SLASH: "Num /", "*": "Num *", KP_MINUS: "Num −", KP_PLUS: "Num +", KP_ENTER: "Num Enter",
  KP_HOME: "Num 7", KP_UPARROW: "Num 8", KP_PGUP: "Num 9", KP_LEFTARROW: "Num 4", KP_5: "Num 5",
  KP_RIGHTARROW: "Num 6", KP_END: "Num 1", KP_DOWNARROW: "Num 2", KP_PGDN: "Num 3",
  KP_INS: "Num 0", KP_DEL: "Num .",
  KP_MULTIPLY: "Num *", NUMLOCK: "Num Lock", SCROLLLOCK: "Scroll Lock",
  RSHIFT: "Right Shift", RCTRL: "Right Ctrl", RALT: "Right Alt", LWIN: "Win", RWIN: "Right Win", APP: "Menu",
};
const KEY_NAME_IDS = {
  MOUSE1: "#L_KB_MOUSE1_FULL", MOUSE2: "#L_KB_MOUSE2_FULL", MOUSE3: "#L_KB_MOUSE3_FULL",
  MOUSE4: "#L_KB_MOUSE4_FULL", MOUSE5: "#L_KB_MOUSE5_FULL",
  MWHEELUP: "#L_KB_WHEELUP_FULL", MWHEELDOWN: "#L_KB_WHEELDOWN_FULL",
};

function keyName(key, t) {
  if (KEY_NAME_IDS[key]) return t(KEY_NAME_IDS[key]);
  if (KEY_NAMES[key]) return KEY_NAMES[key];
  return key.length === 1 ? key.toUpperCase() : key;
}

// Нажатая клавиша → имя в движке. По event.code: от раскладки (русская,
// AZERTY) имя не зависит, как и в самой игре.
const CODE_KEYS = {
  Backquote: "`", Minus: "-", Equal: "=", BracketLeft: "[", BracketRight: "]", Backslash: "\\",
  Semicolon: ";", Quote: "'", Comma: ",", Period: ".", Slash: "/",
  Space: "SPACE", Enter: "ENTER", Tab: "TAB", Backspace: "BACKSPACE", Escape: "ESCAPE",
  CapsLock: "CAPSLOCK", Pause: "PAUSE",
  ShiftLeft: "SHIFT", ShiftRight: "SHIFT", ControlLeft: "CTRL", ControlRight: "CTRL", AltLeft: "ALT", AltRight: "ALT",
  Insert: "INS", Delete: "DEL", Home: "HOME", End: "END", PageUp: "PGUP", PageDown: "PGDN",
  ArrowUp: "UPARROW", ArrowDown: "DOWNARROW", ArrowLeft: "LEFTARROW", ArrowRight: "RIGHTARROW",
  Numpad0: "KP_INS", Numpad1: "KP_END", Numpad2: "KP_DOWNARROW", Numpad3: "KP_PGDN", Numpad4: "KP_LEFTARROW",
  Numpad5: "KP_5", Numpad6: "KP_RIGHTARROW", Numpad7: "KP_HOME", Numpad8: "KP_UPARROW", Numpad9: "KP_PGUP",
  NumpadDecimal: "KP_DEL", NumpadDivide: "KP_SLASH", NumpadMultiply: "*", NumpadSubtract: "KP_MINUS",
  NumpadAdd: "KP_PLUS", NumpadEnter: "KP_ENTER",
};
// Source с inputsystem различает правые модификаторы (см. SOURCE_KEYS).
const SOURCE_CODE_KEYS = {
  ShiftRight: "RSHIFT", ControlRight: "RCTRL", AltRight: "RALT", MetaLeft: "LWIN", MetaRight: "RWIN",
  ContextMenu: "APP", NumLock: "NUMLOCK", ScrollLock: "SCROLLLOCK", NumpadMultiply: "KP_MULTIPLY",
};
const MOUSE_KEYS = ["MOUSE1", "MOUSE3", "MOUSE2", "MOUSE4", "MOUSE5"];

// KeyboardEvent, MouseEvent (mousedown) или WheelEvent → имя клавиши или null.
function keyFromEvent(event, engine) {
  if (!event) return null;
  if (event.type === "wheel") return event.deltaY < 0 ? "MWHEELUP" : event.deltaY > 0 ? "MWHEELDOWN" : null;
  if (event.type === "mousedown" || event.type === "mouseup") return MOUSE_KEYS[event.button] || null;
  const code = event.code || "";
  if (engine === "source" && SOURCE_CODE_KEYS[code]) return SOURCE_CODE_KEYS[code];
  let m;
  if ((m = /^Key([A-Z])$/.exec(code))) return m[1].toLowerCase();
  if ((m = /^Digit(\d)$/.exec(code))) return m[1];
  if ((m = /^F(\d{1,2})$/.exec(code)) && Number(m[1]) >= 1 && Number(m[1]) <= 12) return code;
  return CODE_KEYS[code] || null;
}

export { BOARD_WIDTH, MAIN_WIDTH, BOARD_HEIGHT, layoutFor, keyName, keyFromEvent };
