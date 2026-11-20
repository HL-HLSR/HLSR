//! Подсказка `title` (121 место в 0.4.7, решение 11 slint): вид aura-подсказки Chromium —
//! поля 8 / 4 / 5, у указателя со сдвигом (10, 15), не шире 800 px, цвета
//! `kColorTooltipBackground/Foreground` тёмной схемы, Rubik 12/16 вместо системного шрифта.
//!
//! Показ — `.tip(…)` ([`TipExt`]) поверх `.tooltip` GPUI: через 500 мс неподвижного наведения,
//! скрыта, когда указатель ушёл. Под окном-диалогом элемент не наведён (подложка `occlude`) —
//! подсказки нет. Нажатие мыши прячет подсказку, и пока указатель над тем же элементом и текст
//! тот же, она не возвращается (aura `TooltipController`: `tooltip_window_at_mouse_press_`); уход
//! указателя замечает [`tip_watch`] в корне окна, без него нажатие подсказку не держит.
//! Подсказка Chromium — окно ОС: зум страницы её не меняет, поэтому размеры здесь вне зума.

use crate::scale::zpx;
use crate::text::RUBIK;
use gpui::{
    App, AppContext, Context, DispatchPhase, ElementId, IntoElement, MouseMoveEvent, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Window, WindowId, canvas, div, px,
    rgba,
};
use std::cell::RefCell;
use std::rc::Rc;

/// Сдвиг от указателя: GPUI ставит подсказку на (+1, +1), Chromium — на (+10, +15).
const OFFSET: (f32, f32) = (9., 14.);

/// Элемент подсказки. shortcut: id локальный, элементы с одинаковыми id и текстом в разных
/// родителях не различаются; брать глобальный id, если такие окажутся рядом.
type Key = Option<ElementId>;

/// Подсказка, на которой нажали мышью: элемент, текст при нажатии и был ли указатель над
/// элементом при последнем движении.
struct Pressed {
    id: Key,
    text: SharedString,
    over: bool,
}

thread_local! {
    /// Поток окна.
    static PRESSED: RefCell<Option<Pressed>> = const { RefCell::new(None) };
    /// Окна с [`tip_watch`]: без него запрет после нажатия не снимался бы. Окон за жизнь
    /// процесса единицы, id не повторяются — список не чистится.
    static WATCHED: RefCell<Vec<WindowId>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
thread_local! {
    /// Построенные подсказки (`hidden`) для `layer::tests`.
    pub(super) static BUILT: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
    /// Сколько раз нарисована видимая подсказка, для `layer::tests`.
    pub(super) static SHOWN: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Нажатие на элементе `id` с подсказкой `text`.
fn press(id: &Key, text: SharedString) {
    PRESSED.set(Some(Pressed {
        id: id.clone(),
        text,
        over: true,
    }));
}

/// Указатель движется над элементом `id`.
fn moved_over(id: &Key) {
    PRESSED.with_borrow_mut(|pressed| {
        if let Some(pressed) = pressed
            && pressed.id == *id
        {
            pressed.over = true;
        }
    });
}

/// Движение мыши в окне, до элементов: прошлое движение было не над нажатым текстом — запрет
/// снят.
fn moved() {
    PRESSED.with_borrow_mut(|pressed| match pressed {
        Some(Pressed { over, .. }) if *over => *over = false,
        _ => *pressed = None,
    });
}

/// Подсказку `text` элемента `id` сейчас не показывать: на нём нажали, указатель с него не
/// уходил, текст тот же.
fn held(id: &Key, text: &SharedString) -> bool {
    PRESSED.with_borrow(|pressed| {
        pressed
            .as_ref()
            .is_some_and(|p| p.id == *id && p.text == *text)
    })
}

/// В окне есть [`tip_watch`].
fn watched(window: &Window) -> bool {
    let id = window.window_handle().window_id();
    WATCHED.with_borrow(|watched| watched.contains(&id))
}

struct Tip {
    text: SharedString,
    /// Пустая подсказка: GPUI держит её, пока указатель над элементом.
    hidden: bool,
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if self.hidden {
            return div();
        }
        #[cfg(test)]
        SHOWN.set(SHOWN.get() + 1);
        // Размеры подсказки ОС — логические px окна, без зума страницы.
        let os = |css: f32| zpx(1., css);
        div().pl(os(OFFSET.0)).pt(os(OFFSET.1)).child(
            div()
                .max_w(os(800.))
                .px(os(8.))
                .pt(os(4.))
                .pb(os(5.))
                .bg(rgba(0x202124cc))
                .text_color(rgba(0xe8eaedde))
                .font_family(RUBIK)
                .text_size(os(12.))
                .line_height(os(16.))
                .child(self.text.clone()),
        )
    }
}

/// Подсказка `title` элемента: `div().id(…).tip("Текст")`.
pub trait TipExt: StatefulInteractiveElement + Sized {
    fn tip(self, text: impl Into<SharedString>) -> Self {
        let text: SharedString = text.into();
        self.tip_with(move |_| text.clone())
    }

    /// Текст считается при показе и при нажатии, а не при каждом рисовании.
    fn tip_with(mut self, text: impl Fn(&App) -> SharedString + 'static) -> Self {
        let id: Key = self.interactivity().element_id.clone();
        let text = Rc::new(text);
        let (down, over, down_text) = (id.clone(), id.clone(), text.clone());
        self.capture_any_mouse_down(move |_, window, cx| {
            if watched(window) {
                press(&down, down_text(cx));
            }
        })
        .on_mouse_move(move |_, _, _| moved_over(&over))
        .tooltip(move |_, cx| {
            let text = text(cx);
            let hidden = held(&id, &text);
            #[cfg(test)]
            BUILT.with_borrow_mut(|built| built.push(hidden));
            cx.new(|_| Tip { text, hidden }).into()
        })
    }
}

impl<T: StatefulInteractiveElement> TipExt for T {}

/// Слушатель движений мыши окна для [`TipExt`] (слушать можно только при рисовании): один в
/// корне окна.
pub fn tip_watch() -> impl IntoElement {
    canvas(
        |_, _, _| {},
        |_, _, window, _| {
            let id = window.window_handle().window_id();
            WATCHED.with_borrow_mut(|watched| {
                if !watched.contains(&id) {
                    watched.push(id);
                }
            });
            window.on_mouse_event(|_: &MouseMoveEvent, phase, _, _| {
                if phase == DispatchPhase::Capture {
                    moved();
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size(px(0.))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_holds_the_same_element_and_text_until_the_pointer_leaves() {
        let (a, b): (Key, Key) = (Some("a".into()), Some("b".into()));
        let (text, other): (SharedString, SharedString) = ("Обновить".into(), "Нравится".into());
        press(&a, text.clone());
        assert!(held(&a, &text), "нажатый держится");
        assert!(!held(&a, &other), "текст сменился — показывается");
        assert!(!held(&b, &text), "сосед с тем же текстом показывается");
        // Движения над нажатым: держится.
        for _ in 0..3 {
            moved();
            moved_over(&a);
        }
        assert!(held(&a, &text));
        // Движение мимо, затем любое следующее — запрет снят.
        moved();
        assert!(held(&a, &text), "снимается следующим движением");
        moved();
        assert!(!held(&a, &text));
        // Сразу на соседа с тем же текстом: он не продлевает запрет нажатого.
        press(&a, text.clone());
        moved();
        moved_over(&b);
        moved();
        assert!(!held(&a, &text));
    }
}
