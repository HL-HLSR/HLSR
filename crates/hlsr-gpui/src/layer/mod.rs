//! Слои поверх окна (Ф9–Ф11 05-ui-shell): окна-диалоги, всплывающие у кнопки, подсказки
//! `title`, тосты. Как `.launcher-root` 0.4.9: z-index окон 50 (настройки и прочие) и 70
//! (подтверждение, профиль), тостов 60, списков 1000; подсказка `title` — над всем.
//!
//! - [`Modal`] — окно-диалог: подложка `--l-modal-scrim` во всё окно (нажатие на неё закрывает,
//!   `@mousedown.self`), переход `l-modal`, Esc, ловушка фокуса Tab/Shift+Tab (`focus-trap.js`),
//!   фокус при открытии и возврат прежнего при закрытии. Подложка `occlude`: полоса шапки
//!   (`WindowControlArea::Drag`) под окном не таскает окно (`no-drag` диалогов 0.4.9).
//! - [`popup`] + [`MenuNav`] — меню и списки: `anchored` в координатах окна, место по
//!   `hlsr_ui::overlay::place_dropdown`, выход за прокрутку (`deferred` без маски), `l-pop`,
//!   закрытие щелчком мимо и прокруткой; стрелки, Home/End, Enter, Esc, Tab, поиск по буквам.
//! - [`TipExt`] — подсказка `title` (вид aura-подсказки Chromium, 500 мс), под окнами не видна,
//!   после нажатия — до ухода указателя ([`tip_watch`] в корне окна).
//! - [`Toasts`] — тосты `notify(text, icon, ms)`: 3 тона, полоса жизни, не больше трёх.
//!
//! Модальные окна кладутся в корень окна (или туда, где нет `overflow_hidden`): `deferred`
//! рисует их поверх всего, но место в раскладке берёт у родителя.

mod modal;
mod popup;
#[cfg(test)]
mod tests;
mod toast;
mod tooltip;

pub use modal::{Modal, card, card_plain};
pub use popup::{
    MENU_CONTEXT, MenuKey, MenuNav, Outcome, Place, PopLook, anchor_probe, menu_keys, pop_plain,
    pop_surface, pop_surface_rounded, popup, popup_scroll,
};
pub use toast::{TOAST_MS, Toasts};
pub use tooltip::{TipExt, tip_watch};

use crate::motion::AnimGate;
use crate::scale::WINDOW_RADIUS;
use gpui::{
    App, Context, FocusHandle, Global, InteractiveElement, KeyBinding, ParentElement, Pixels,
    Window, actions, px,
};

actions!(
    layer,
    [
        /// Esc: закрыть верхнее (меню, окно).
        Dismiss,
        /// Tab / Shift+Tab: следующий / прошлый элемент (в окне — по кругу внутри окна).
        FocusNext,
        FocusPrev,
        /// Клавиши открытого меню (контекст [`MENU_CONTEXT`]).
        MenuUp,
        MenuDown,
        MenuHome,
        MenuEnd,
        /// Enter и пробел: выбрать пункт или открыть меню.
        MenuPick,
    ]
);

/// Порядок отрисовки слоёв (`z-index` 0.4.9): окна, тосты, окна поверх (подтверждение,
/// профиль), меню и списки.
pub const Z_MODAL: usize = 50;
pub const Z_TOAST: usize = 60;
pub const Z_TOP: usize = 70;
pub const Z_MENU: usize = 1000;

/// Клавиши слоёв — до первого окна (из [`crate::init`]).
pub fn init(cx: &mut App) {
    let menu = Some(MENU_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("escape", Dismiss, None),
        KeyBinding::new("tab", FocusNext, None),
        KeyBinding::new("shift-tab", FocusPrev, None),
        KeyBinding::new("up", MenuUp, menu),
        KeyBinding::new("down", MenuDown, menu),
        KeyBinding::new("home", MenuHome, menu),
        KeyBinding::new("end", MenuEnd, menu),
        KeyBinding::new("enter", MenuPick, menu),
        KeyBinding::new("space", MenuPick, menu),
    ]);
}

/// Открытые окна-диалоги окна лаунчера (стопка): `overlayOpen` 0.4.9 — `open_count() > 0`.
#[derive(Default)]
pub struct Layers {
    /// Контейнеры открытых окон, нижнее первым.
    stack: Vec<FocusHandle>,
    /// Открытые окна с размытой подложкой ([`Modal::blur`]).
    blurred: Vec<FocusHandle>,
    fullscreen: bool,
}

impl Global for Layers {}

impl Layers {
    /// Сколько окон открыто (подсказки колонки и параллакс 0.4.9 стоят при `> 0`).
    pub fn open_count(cx: &App) -> usize {
        cx.try_global::<Layers>()
            .map_or(0, |layers| layers.stack.len())
    }

    /// Контейнер верхнего окна: Tab вне окон ведёт в него.
    pub fn top(cx: &App) -> Option<FocusHandle> {
        cx.try_global::<Layers>()?.stack.last().cloned()
    }

    /// Открыто окно с размытой подложкой: её кусок ищет `prepaint` каждого кадра по снимку
    /// окна, и кэш видов (`crate::cache::on`) на это время снят.
    pub fn blurred(cx: &App) -> bool {
        cx.try_global::<Layers>()
            .is_some_and(|layers| !layers.blurred.is_empty())
    }

    /// Полный экран: у окна нет скругления, подложка — во всё окно без углов.
    pub fn set_fullscreen(cx: &mut App, on: bool) {
        cx.default_global::<Layers>().fullscreen = on;
        cx.refresh_windows();
    }

    /// Радиус окна сейчас: в полном экране углов нет (`--window-radius`).
    pub fn radius(cx: &App) -> Pixels {
        match cx.try_global::<Layers>() {
            Some(layers) if layers.fullscreen => px(0.),
            _ => px(WINDOW_RADIUS),
        }
    }

    /// Слой открытого окна по стопке сейчас (1 — нижнее): нижние могли закрыться.
    fn depth(cx: &App, scope: &FocusHandle) -> Option<usize> {
        let stack = &cx.try_global::<Layers>()?.stack;
        stack
            .iter()
            .position(|open| open == scope)
            .map(|index| index + 1)
    }

    /// Окно открылось: его слой (1 — первое); `blur` — подложка размыта.
    fn push(cx: &mut App, scope: FocusHandle, blur: bool) -> usize {
        let layers = cx.default_global::<Layers>();
        layers.stack.retain(|open| open != &scope);
        layers.blurred.retain(|open| open != &scope);
        if blur {
            layers.blurred.push(scope.clone());
        }
        layers.stack.push(scope);
        let depth = layers.stack.len();
        AnimGate::set_top_layer(cx, depth);
        depth
    }

    fn remove(cx: &mut App, scope: &FocusHandle) {
        let layers = cx.default_global::<Layers>();
        layers.stack.retain(|open| open != scope);
        layers.blurred.retain(|open| open != scope);
        let depth = layers.stack.len();
        AnimGate::set_top_layer(cx, depth);
    }
}

/// Общий обработчик закрытия: подложка и Esc, щелчок мимо и прокрутка.
type Handler = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

/// Обработчик без события для [`Modal::render`] и [`popup`] (`cx.listener` без `&E`).
pub fn listen<V: 'static>(
    cx: &Context<V>,
    f: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> impl Fn(&mut Window, &mut App) + 'static {
    let view = cx.entity().downgrade();
    move |window, cx| {
        view.update(cx, |view, cx| f(view, window, cx)).ok();
    }
}

/// Tab по окну: в корне окна лаунчера. При открытом окне-диалоге фокус ходит по кругу внутри
/// верхнего окна ([`trap`]), иначе — по всем элементам с `tab_stop`. Тут же Esc мимо окна
/// ([`dismiss_top`]) и движения мыши для подсказок ([`tip_watch`]; второй в том же окне ломает
/// снятие запрета после нажатия).
pub fn tab_root<E: InteractiveElement + ParentElement>(el: E) -> E {
    el.on_action(|_: &FocusNext, window, cx| move_focus(true, window, cx))
        .on_action(|_: &FocusPrev, window, cx| move_focus(false, window, cx))
        .on_action(dismiss_top)
        .child(tip_watch())
}

/// Esc дошёл до корня, фокус вне верхнего окна (колокольчик под подложкой после Esc панели
/// уведомлений): `keydown` документа у окон 0.4.9 от фокуса не зависит — Esc уходит верхнему
/// окну. Фокус — в его контейнер, как при открытии. Контейнер не в кадре этого окна — фокус
/// не трогаем: ушёл бы на неотрисованный элемент, а Esc — в никуда.
fn dismiss_top(_: &Dismiss, window: &mut Window, cx: &mut App) {
    match Layers::top(cx) {
        Some(scope) if scope.is_rendered(window) && !scope.contains_focused(window, cx) => {
            window.focus(&scope, cx);
            scope.dispatch_action(&Dismiss, window, cx);
        }
        _ => cx.propagate(),
    }
}

/// Следующий (`forward`) или прошлый элемент с учётом открытых окон.
pub fn move_focus(forward: bool, window: &mut Window, cx: &mut App) {
    match Layers::top(cx) {
        Some(scope) => trap(&scope, forward, window, cx),
        None if forward => window.focus_next(cx),
        None => window.focus_prev(cx),
    }
}

/// `trapFocus`: фокус вне окна — на первый (Shift+Tab — на последний) элемент окна; с
/// последнего Tab ведёт на первый, с первого Shift+Tab — на последний.
pub(crate) fn trap(scope: &FocusHandle, forward: bool, window: &mut Window, cx: &mut App) {
    let inside =
        |window: &Window, cx: &App| scope.contains_focused(window, cx) && !scope.is_focused(window);
    if inside(window, cx) {
        if forward {
            window.focus_next(cx);
        } else {
            window.focus_prev(cx);
        }
        if inside(window, cx) {
            return;
        }
    }
    // Контейнер окна стоит в порядке Tab перед своими элементами (tab_group): от него
    // focus_next идёт по элементам окна по порядку, пока не выйдет наружу.
    window.focus(scope, cx);
    let mut first: Option<FocusHandle> = None;
    let mut last = None;
    // shortcut: не больше 512 шагов — в окне столько элементов не бывает.
    for _ in 0..512 {
        window.focus_next(cx);
        let focused = window.focused(cx);
        if !inside(window, cx) || (first.is_some() && focused == first) {
            break;
        }
        if forward {
            return;
        }
        first = first.or_else(|| focused.clone());
        last = focused;
    }
    window.focus(last.as_ref().unwrap_or(scope), cx);
}
