//! Слой всплывающего (Ф7, карта ПП5 §0.10, §2): логика 0.4.7 без Slint.
//!
//! - поиск по первым буквам выпадающего списка (`dropdown-typeahead.mjs`);
//! - место выпадающего списка у кнопки (`UiDropdown.positionMenu`): переворот
//!   вверх и вниз, ширина по содержимому с шагом 4, высота в пределах
//!   прокручиваемых предков;
//! - ступени меню игры по высоте окна (`GameMenu.fitToWindow`);
//! - тон тоста по значку (`ToastContainer.toneOf`) и список показанных тостов с
//!   уходящими (`<transition-group>`: ушедший тост остаётся на месте, пока
//!   играет `toast-out`).
//!
//! Значки пунктов — `fa::glyph_parts` (тот же разбор классов, что у `Icon`).

use hlsr_core::js;

/// Сброс набранных букв, мс (`TYPEAHEAD_RESET_MS`).
pub const TYPEAHEAD_RESET_MS: u64 = 700;

fn normalize(text: &str) -> String {
    js::trim(&text.to_lowercase().replace('ё', "е")).to_string()
}

/// `findTypeaheadIndex(items, query, currentIndex)`: буквы, набранные подряд,
/// складываются в префикс; одна и та же буква повторно перебирает подходящие
/// пункты по кругу. `-1` — ничего не нашлось.
pub fn find_typeahead_index<S: AsRef<str>>(labels: &[S], query: &str, current: i32) -> i32 {
    let wanted = normalize(query);
    if wanted.is_empty() || labels.is_empty() {
        return -1;
    }
    let units: Vec<u16> = wanted.encode_utf16().collect();
    let repeated = units.len() > 1 && units.iter().all(|unit| *unit == units[0]);
    let prefix: String = if repeated {
        String::from_utf16_lossy(&units[..1])
    } else {
        wanted
    };
    let count = labels.len() as i64;
    // Новый префикс ищется с текущего пункта, повтор буквы — со следующего.
    let start = if repeated || units.len() == 1 {
        i64::from(current) + 1
    } else {
        i64::from(current.max(0))
    };
    for step in 0..count {
        let index = ((start + step) % count + count) % count;
        if normalize(labels[index as usize].as_ref()).starts_with(&prefix) {
            return index as i32;
        }
    }
    -1
}

/// `isTypeaheadKey(event)`: один знак (`key.length === 1`), не пробел, без Ctrl,
/// Meta и Alt. `text` — `KeyEvent.text` Slint: служебные клавиши в нём —
/// управляющие знаки или частная область U+F700…U+F8FF (у JS это имена вроде
/// «ArrowDown», длиннее одного знака).
pub fn is_typeahead_key(text: &str, ctrl: bool, meta: bool, alt: bool) -> bool {
    let mut chars = text.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return false;
    };
    let special = c.is_control() || ('\u{F700}'..='\u{F8FF}').contains(&c);
    c.len_utf16() == 1 && c != ' ' && !special && !ctrl && !meta && !alt
}

/// `UiDropdown.enabledIndex(index, step)`: ближайший доступный пункт от `index`
/// в сторону `step` по кругу; `-1` — доступных нет.
pub fn enabled_index(disabled: &[bool], index: i32, step: i32) -> i32 {
    let count = disabled.len() as i64;
    for n in 0..count {
        let i = ((i64::from(index) + n * i64::from(step)) % count + count) % count;
        if !disabled[i as usize] {
            return i as i32;
        }
    }
    -1
}

/// `GameMenu.move(delta)`: шаг по кругу, недоступные пропускаются; если
/// доступных нет — остановка на месте последнего шага (как цикл 0.4.7).
pub fn menu_move(disabled: &[bool], active: i32, delta: i32) -> i32 {
    let total = disabled.len() as i64;
    if total == 0 {
        return active;
    }
    let mut next = i64::from(active);
    for _ in 0..total {
        next = ((next + i64::from(delta)) % total + total) % total;
        if !disabled[next as usize] {
            break;
        }
    }
    next as i32
}

/// Прямоугольник в логических px окна.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }
}

/// Поля меню (`padding: 8px`), предел ширины и высоты, полоса прокрутки
/// (`::-webkit-scrollbar { width: 8px }`), отступ от кнопки и от краёв.
pub const MENU_PADDING: f64 = 8.0;
pub const MENU_MAX_WIDTH: f64 = 360.0;
pub const MENU_MAX_HEIGHT: f64 = 320.0;
pub const MENU_SCROLLBAR: f64 = 8.0;
const MENU_GAP: f64 = 8.0;
/// `positionMenu` вызывается в `$nextTick` сразу после вставки меню: класс `l-pop-enter-from` (`scale(.97)`)
/// ещё не снят, и `getBoundingClientRect()` отдаёт размер меню, уменьшенный до .97. По нему считаются
/// `x` (выравнивание вправо и поля окна) и `y` (открытие вверх): меню встаёт на 3 % своего размера
/// ближе к кнопке, чем «по-честному» (снято эталоном: x и y сдвинуты на 0,03 ширины и 0,03 высоты).
const ENTER_SCALE: f64 = 0.97;

/// Что нужно `positionMenu`, чтобы поставить список.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropdownRequest {
    /// Кнопка списка.
    pub anchor: Rect,
    /// Прокручиваемые предки кнопки (их пересечение); `None` — только окно.
    pub clip: Option<Rect>,
    pub window_width: f64,
    pub window_height: f64,
    /// `max-content` пунктов и заголовка — без полей меню и полосы.
    pub content_width: f64,
    /// `scrollHeight` меню: пункты с полями меню.
    pub scroll_height: f64,
    /// `direction: "up"`.
    pub up: bool,
    /// `align: "right"`.
    pub align_right: bool,
}

/// Где и какого размера список (рамка меню, `box-sizing: border-box`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropdownPlace {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// `max-height`, кратная 4.
    pub max_height: f64,
    /// Открылся вверх (после переворота).
    pub up: bool,
}

/// `UiDropdown.positionMenu` 0.4.9 (zoom 1: длины уже в масштабе интерфейса).
pub fn place_dropdown(request: &DropdownRequest) -> DropdownPlace {
    let anchor = request.anchor;
    let mut top = 0.0_f64;
    let mut bottom = request.window_height;
    let mut left = MENU_GAP;
    let mut right = request.window_width - MENU_GAP;
    // Меню должно помещаться не только в окно, но и в прокручиваемую область.
    if let Some(clip) = request.clip {
        top = top.max(clip.y);
        bottom = bottom.min(clip.bottom());
        left = left.max(clip.x + MENU_GAP);
        right = right.min(clip.right() - MENU_GAP);
    }
    let mut up_space = (anchor.y - top - MENU_GAP).max(0.0);
    let mut down_space = (bottom - anchor.bottom() - MENU_GAP).max(0.0);
    let max_width = MENU_MAX_WIDTH.min(right - left).max(0.0);
    let min_width = anchor.width.min(max_width);
    // Ширину меряют при max-height: 320px: полоса есть, если содержимое выше.
    let bar = if request.scroll_height > MENU_MAX_HEIGHT {
        MENU_SCROLLBAR
    } else {
        0.0
    };
    let max_content = request.content_width + 2.0 * MENU_PADDING + bar;
    // getComputedStyle(menu).width: max-content в пределах min-/max-width.
    let natural = max_content.min(max_width).max(min_width);
    let mut width = max_width.min((natural / 4.0).ceil() * 4.0);
    let wanted = MENU_MAX_HEIGHT.min(request.scroll_height);
    // 0.4.9, низкое окно (Full HD): в прокручиваемой области места нет ни вверх, ни вниз —
    // меню выходит за неё (поверх панели кнопок), в пределах окна.
    if up_space < wanted && down_space < wanted {
        up_space = (anchor.y - MENU_GAP).max(0.0);
        down_space = (request.window_height - anchor.bottom() - MENU_GAP).max(0.0);
    }
    let mut up = request.up;
    if !request.up && down_space < wanted && up_space > down_space {
        up = true;
    } else if request.up && up_space < wanted && down_space > up_space {
        up = false;
    }
    let space = if up { up_space } else { down_space };
    let max_height = (MENU_MAX_HEIGHT.min(space) / 4.0).floor() * 4.0;
    // 0.4.9: список листается — ширина больше на полосу (`offsetWidth − clientWidth`), подписи
    // без многоточия.
    if request.scroll_height > max_height {
        width = max_width.min(width + MENU_SCROLLBAR);
    }
    let height = request.scroll_height.min(max_height);
    let (shown_width, shown_height) = (width * ENTER_SCALE, height * ENTER_SCALE);
    let x = if request.align_right {
        anchor.right() - shown_width
    } else {
        anchor.x
    };
    let y = if up {
        anchor.y - shown_height - MENU_GAP
    } else {
        anchor.bottom() + MENU_GAP
    };
    DropdownPlace {
        x: left.max(x.min(right - shown_width)),
        y,
        width,
        height,
        max_height,
        up,
    }
}

/// Меню над кнопкой: `bottom: calc(100% + 12px)`.
pub const GAME_MENU_LIFT: f64 = 12.0;

/// Как меню игры встало в окно.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GameMenuFit {
    /// 0 — обычная ступень, 1 — `compact`, 2 — `tight`.
    pub step: usize,
    /// `max-height`; 0 — без предела.
    pub max_height: f64,
    /// Высота меню.
    pub height: f64,
    /// Список прокручивается (содержимое выше `max-height`).
    pub scrolls: bool,
}

/// `GameMenu.fitToWindow`: меню открывается вверх и не заходит под шапку; в
/// низком окне пункты сжимаются по ступеням, а если места нет и так — список
/// прокручивается (не ниже 160 px). `heights` — `scrollHeight` меню на ступенях по
/// порядку (обычная, `compact`, `tight`): подписи пунктов переносятся по словам, и
/// высоту меряет раскладка слоя (`GameMenuSizes` в overlay.slint).
pub fn fit_game_menu(heights: &[f64], trigger_top: f64, topbar_bottom: f64) -> GameMenuFit {
    let room = (trigger_top - GAME_MENU_LIFT - topbar_bottom - 8.0).floor();
    if let Some(step) = heights.iter().position(|height| *height <= room) {
        return GameMenuFit {
            step,
            max_height: 0.0,
            height: heights[step],
            scrolls: false,
        };
    }
    let step = heights.len().saturating_sub(1);
    let height = heights.last().copied().unwrap_or(0.0);
    let max_height = room.max(160.0);
    GameMenuFit {
        step,
        max_height,
        height: height.min(max_height),
        scrolls: height > max_height,
    }
}

/// Тон тоста (`.is-success` …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Success,
    Warning,
    Danger,
}

const TONES: [(Tone, &[&str]); 3] = [
    (
        Tone::Success,
        &[
            "fa-check",
            "fa-clipboard-check",
            "fa-check-circle",
            "fa-file-import",
        ],
    ),
    (
        Tone::Warning,
        &["fa-exclamation-triangle", "fa-lock", "fa-clock"],
    ),
    (Tone::Danger, &["fa-exclamation-circle", "fa-times-circle"]),
];

/// `toneOf(toast)`: тон по значку, который передал `notify()`.
pub fn toast_tone(icon: &str) -> Tone {
    let classes: Vec<&str> = icon.split_whitespace().collect();
    TONES
        .iter()
        .find(|(_, icons)| icons.iter().any(|name| classes.contains(name)))
        .map_or(Tone::Info, |(tone, _)| *tone)
}

/// Тост оболочки (`notify(text, icon)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToastInput {
    pub id: i32,
    pub text: String,
    /// Классы FontAwesome («fas fa-check»).
    pub icon: String,
}

/// Показанный тост: уходящий остаётся на месте, пока играет `toast-out`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToastEntry {
    pub id: i32,
    pub text: String,
    pub icon: String,
    pub tone: Tone,
    pub leaving: bool,
}

/// Что изменилось после [`ToastFeed::sync`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ToastChanges {
    /// Новые тосты (в конце списка, по порядку).
    pub added: Vec<i32>,
    /// Начали уходить.
    pub leaving: Vec<i32>,
    /// Изменился текст или значок.
    pub updated: Vec<i32>,
}

/// Показанные тосты `<transition-group name="toast">`.
#[derive(Debug, Default, Clone)]
pub struct ToastFeed {
    entries: Vec<ToastEntry>,
}

impl ToastFeed {
    pub fn entries(&self) -> &[ToastEntry] {
        &self.entries
    }

    /// Сверить с нынешним списком оболочки: пропавшие начинают уход на своём
    /// месте, новые встают в конец (как вставка Vue: перед следующим из нового
    /// списка, а его нет).
    pub fn sync(&mut self, current: &[ToastInput]) -> ToastChanges {
        let mut changes = ToastChanges::default();
        for entry in &mut self.entries {
            match current.iter().find(|toast| toast.id == entry.id) {
                Some(toast) if !entry.leaving => {
                    if entry.text != toast.text || entry.icon != toast.icon {
                        entry.text.clone_from(&toast.text);
                        entry.icon.clone_from(&toast.icon);
                        entry.tone = toast_tone(&toast.icon);
                        changes.updated.push(entry.id);
                    }
                }
                Some(_) => {}
                None if !entry.leaving => {
                    entry.leaving = true;
                    changes.leaving.push(entry.id);
                }
                None => {}
            }
        }
        for toast in current {
            if !self.entries.iter().any(|entry| entry.id == toast.id) {
                self.entries.push(ToastEntry {
                    id: toast.id,
                    text: toast.text.clone(),
                    icon: toast.icon.clone(),
                    tone: toast_tone(&toast.icon),
                    leaving: false,
                });
                changes.added.push(toast.id);
            }
        }
        changes
    }

    /// Убрать ушедший тост; его место в списке.
    pub fn remove(&mut self, id: i32) -> Option<usize> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.id == id && entry.leaving)?;
        self.entries.remove(index);
        Some(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Самотест 0.4.7 «dropdown typeahead jumps by first letters and cycles on a
    /// repeated letter» (scripts/launcher-ui-selftest.js:288).
    #[test]
    fn typeahead_selftest() {
        let items = [
            "Не указана",
            "Германия",
            "Россия",
            "Румыния",
            "Украина",
            "Ёлка",
        ];
        assert_eq!(find_typeahead_index(&items, "р", -1), 2);
        assert_eq!(
            find_typeahead_index(&items, "р", 2),
            3,
            "та же буква ведёт к следующей стране"
        );
        assert_eq!(
            find_typeahead_index(&items, "рр", 3),
            2,
            "перебор идёт по кругу"
        );
        assert_eq!(
            find_typeahead_index(&items, "ру", 2),
            3,
            "набранный префикс уточняет выбор"
        );
        assert_eq!(find_typeahead_index(&items, "РОС", -1), 2);
        assert_eq!(find_typeahead_index(&items, "е", -1), 5, "ё ищется и по е");
        assert_eq!(find_typeahead_index(&items, "щ", 0), -1);
        assert!(is_typeahead_key("р", false, false, false));
        assert!(!is_typeahead_key("a", true, false, false));
        // «Enter» у JS — имя; у Slint — управляющий знак.
        assert!(!is_typeahead_key("\n", false, false, false));
        assert!(!is_typeahead_key(" ", false, false, false));
    }

    #[test]
    fn typeahead_edges() {
        let none: [&str; 0] = [];
        assert_eq!(find_typeahead_index(&none, "a", -1), -1);
        assert_eq!(find_typeahead_index(&["Alpha"], "  ", -1), -1);
        // Пустая подпись — "", ничего не подходит.
        assert_eq!(find_typeahead_index(&["", "b"], "b", -1), 1);
        // Стрелки Slint (частная область), Esc, Tab, Alt, Meta, суррогатная пара.
        for text in ["\u{F700}", "\u{1b}", "\t", "\u{7f}"] {
            assert!(!is_typeahead_key(text, false, false, false), "{text:?}");
        }
        assert!(!is_typeahead_key("x", false, true, false));
        assert!(!is_typeahead_key("x", false, false, true));
        assert!(!is_typeahead_key("😀", false, false, false));
        assert!(!is_typeahead_key("ab", false, false, false));
        assert!(is_typeahead_key("Ё", false, false, false));
    }

    #[test]
    fn enabled_index_and_menu_move_skip_disabled() {
        let disabled = [false, true, false, true];
        assert_eq!(enabled_index(&disabled, 1, 1), 2);
        assert_eq!(enabled_index(&disabled, 3, 1), 0, "по кругу");
        assert_eq!(enabled_index(&disabled, 3, -1), 2);
        assert_eq!(enabled_index(&disabled, -1, -1), 2);
        assert_eq!(enabled_index(&[true, true], 0, 1), -1);
        assert_eq!(enabled_index(&[], 0, 1), -1);
        assert_eq!(menu_move(&disabled, 0, 1), 2);
        assert_eq!(menu_move(&disabled, 2, 1), 0);
        assert_eq!(menu_move(&disabled, 0, -1), 2);
        assert_eq!(menu_move(&disabled, -1, 1), 0);
        // Все недоступны: цикл 0.4.7 заканчивает на последнем шаге.
        assert_eq!(menu_move(&[true, true, true], 0, 1), 0);
        assert_eq!(menu_move(&[], 3, 1), 3);
    }

    fn request(anchor: Rect) -> DropdownRequest {
        DropdownRequest {
            anchor,
            clip: None,
            window_width: 1200.0,
            window_height: 676.0,
            content_width: 150.3,
            scroll_height: 16.0 + 3.0 * 40.0 + 2.0 * 4.0,
            up: false,
            align_right: false,
        }
    }

    #[test]
    fn dropdown_opens_down_with_width_on_the_grid() {
        let place = place_dropdown(&request(Rect {
            x: 100.0,
            y: 100.0,
            width: 120.0,
            height: 40.0,
        }));
        // 150.3 + 16 = 166.3 → 168.
        assert_eq!(place.width, 168.0);
        assert_eq!(place.height, 144.0);
        assert_eq!((place.x, place.y), (100.0, 148.0));
        assert!(!place.up);
        assert_eq!(place.max_height, 320.0);
    }

    #[test]
    fn dropdown_flips_up_at_the_bottom_edge_and_back() {
        let anchor = Rect {
            x: 100.0,
            y: 560.0,
            width: 120.0,
            height: 40.0,
        };
        let place = place_dropdown(&request(anchor));
        assert!(place.up, "внизу 676 − 600 − 8 = 68 < 144, вверху больше");
        assert_eq!(place.y, 560.0 - 144.0 * ENTER_SCALE - 8.0);
        // direction: up у верхнего края — вниз.
        let mut top = request(Rect {
            x: 100.0,
            y: 40.0,
            width: 120.0,
            height: 40.0,
        });
        top.up = true;
        let place = place_dropdown(&top);
        assert!(!place.up);
        assert_eq!(place.y, 88.0);
    }

    #[test]
    fn dropdown_height_fits_the_space_in_steps_of_four() {
        let mut long = request(Rect {
            x: 100.0,
            y: 470.0,
            width: 120.0,
            height: 40.0,
        });
        long.scroll_height = 16.0 + 12.0 * 40.0 + 11.0 * 4.0;
        // Внизу 676 − 510 − 8 = 158, вверху 462: вверх, 320.
        let place = place_dropdown(&long);
        assert!(place.up);
        assert_eq!(place.max_height, 320.0);
        assert_eq!(place.height, 320.0);
        // Полоса прокрутки — в ширине замера: 150.3 + 16 + 8 = 174.3 → 176; список листается —
        // ещё полоса (0.4.9, `offsetWidth − clientWidth`): 184.
        assert_eq!(place.width, 184.0);
        // Окно ниже списка: floor(min(320, 122) / 4) * 4, листается — ширина 168 + 8.
        let mut low = request(Rect {
            x: 100.0,
            y: 130.0,
            width: 120.0,
            height: 40.0,
        });
        low.window_height = 300.0;
        let place = place_dropdown(&low);
        assert!(!place.up);
        assert_eq!((place.max_height, place.height), (120.0, 120.0));
        assert_eq!(place.width, 176.0);
    }

    #[test]
    fn dropdown_leaves_a_low_scroll_area_but_stays_in_the_window() {
        // 0.4.9: в области нет места под 144 ни вверх (141), ни вниз (103) — меню берёт место
        // окна: внизу 676 − 510 − 8 = 158, и не листается.
        let mut low = request(Rect {
            x: 100.0,
            y: 470.0,
            width: 120.0,
            height: 40.0,
        });
        low.clip = Some(Rect {
            x: 0.0,
            y: 321.0,
            width: 1200.0,
            height: 300.0,
        });
        let place = place_dropdown(&low);
        assert!(!place.up);
        assert_eq!((place.max_height, place.height), (156.0, 144.0));
        assert_eq!((place.width, place.y), (168.0, 518.0));
        // Хотя бы в одну сторону место в области есть — меню остаётся в ней: вверху 462.
        low.clip = Some(Rect {
            x: 0.0,
            y: 0.0,
            width: 1200.0,
            height: 600.0,
        });
        let place = place_dropdown(&low);
        assert!(place.up);
        assert_eq!(place.max_height, 320.0);
    }

    #[test]
    fn dropdown_width_limits_and_right_alignment() {
        let mut wide = request(Rect {
            x: 1100.0,
            y: 100.0,
            width: 80.0,
            height: 40.0,
        });
        wide.content_width = 500.0;
        let place = place_dropdown(&wide);
        assert_eq!(place.width, 360.0);
        // Не дальше 8 px от правого края окна (по размеру с `scale(.97)`).
        assert_eq!(place.x, 1200.0 - 8.0 - 360.0 * ENTER_SCALE);
        wide.align_right = true;
        wide.content_width = 10.0;
        let place = place_dropdown(&wide);
        // min-width — ширина кнопки: 80.
        assert_eq!(place.width, 80.0);
        assert_eq!(place.x, 1180.0 - 80.0 * ENTER_SCALE);
        // Дробная кнопка: min-width 81.5 → 84.
        wide.anchor.width = 81.5;
        assert_eq!(place_dropdown(&wide).width, 84.0);
        // Узкая область: max-width — её ширина без полей.
        wide.clip = Some(Rect {
            x: 900.0,
            y: 0.0,
            width: 200.0,
            height: 676.0,
        });
        wide.content_width = 400.0;
        assert_eq!(place_dropdown(&wide).width, 184.0);
    }

    #[test]
    fn game_menu_steps_down_then_scrolls() {
        // 10 однострочных пунктов, два разделителя (GameMenu.vue ITEMS): поля 8 + 8, пункты
        // 48 / 42 / 36, промежутки 4 / 3 / 2, поля над разделёнными 8 / 7 / 6.
        let heights = [
            16.0 + 480.0 + 36.0 + 16.0,
            16.0 + 420.0 + 27.0 + 14.0,
            16.0 + 360.0 + 18.0 + 12.0,
        ];
        let fit = |top: f64| fit_game_menu(&heights, top, 80.0);
        assert_eq!(fit(700.0).step, 0);
        assert_eq!(fit(600.0).step, 1);
        let tight = fit(540.0);
        assert_eq!(
            (tight.step, tight.max_height, tight.scrolls),
            (2, 0.0, false)
        );
        let scroll = fit(400.0);
        assert_eq!(scroll.step, 2);
        assert_eq!(scroll.max_height, 300.0);
        assert_eq!(scroll.height, 300.0);
        assert!(scroll.scrolls);
        assert_eq!(fit(200.0).max_height, 160.0);
    }

    #[test]
    fn wrapped_labels_step_the_game_menu_down_earlier() {
        // Две подписи в две строки (ru: «Добавить ярлык на рабочий стол», «Проверить наличие
        // обновлений»): пункт 8 + 40 + 8, 6 + 40 + 6, 4 + 40 + 4 — на 8, 10, 12 выше.
        let lines = [548.0, 477.0, 406.0];
        let wrapped = [564.0, 497.0, 430.0];
        // Места 660 − 12 − 80 − 8 = 560: однострочное меню встаёт целиком, с переносами — compact.
        assert_eq!(fit_game_menu(&lines, 660.0, 80.0).step, 0);
        let fit = fit_game_menu(&wrapped, 660.0, 80.0);
        assert_eq!((fit.step, fit.height, fit.scrolls), (1, 497.0, false));
        // Места 160: tight выше — прокрутка.
        let low = fit_game_menu(&wrapped, 260.0, 80.0);
        assert_eq!((low.step, low.max_height, low.height), (2, 160.0, 160.0));
        assert!(low.scrolls);
    }

    #[test]
    fn toast_tone_by_icon() {
        assert_eq!(toast_tone("fas fa-check"), Tone::Success);
        assert_eq!(toast_tone("fas fa-file-import"), Tone::Success);
        assert_eq!(toast_tone("fas fa-lock"), Tone::Warning);
        assert_eq!(toast_tone("fas fa-times-circle"), Tone::Danger);
        assert_eq!(toast_tone("fas fa-info-circle"), Tone::Info);
        assert_eq!(toast_tone(""), Tone::Info);
        // Имя класса целиком, не подстрока.
        assert_eq!(toast_tone("fas fa-check-double"), Tone::Info);
    }

    fn input(id: i32) -> ToastInput {
        ToastInput {
            id,
            text: format!("тост {id}"),
            icon: "fas fa-check".into(),
        }
    }

    #[test]
    fn toast_feed_keeps_leaving_toasts_in_place() {
        let mut feed = ToastFeed::default();
        let changes = feed.sync(&[input(1), input(2), input(3)]);
        assert_eq!(changes.added, [1, 2, 3]);
        // Четвёртый: первый уходит на своём месте, новый — в конец.
        let changes = feed.sync(&[input(2), input(3), input(4)]);
        assert_eq!(changes.added, [4]);
        assert_eq!(changes.leaving, [1]);
        let ids: Vec<(i32, bool)> = feed.entries().iter().map(|e| (e.id, e.leaving)).collect();
        assert_eq!(ids, [(1, true), (2, false), (3, false), (4, false)]);
        // Повтор того же списка ничего не меняет.
        assert_eq!(
            feed.sync(&[input(2), input(3), input(4)]),
            ToastChanges::default()
        );
        assert_eq!(feed.remove(1), Some(0));
        assert_eq!(feed.remove(2), None, "не уходит");
        let mut changed = input(3);
        changed.icon = "fas fa-lock".into();
        let changes = feed.sync(&[input(2), changed, input(4)]);
        assert_eq!(changes.updated, [3]);
        assert_eq!(feed.entries()[1].tone, Tone::Warning);
    }
}
