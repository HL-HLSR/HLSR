//! Всплывающее у кнопки: меню и списки (UiDropdown, GameMenu, меню аккаунта 0.4.9).
//!
//! Фокус остаётся на кнопке, пункт «в фокусе» — [`MenuNav::active`] (как
//! `aria-activedescendant` в slint): кнопка с [`menu_keys`] получает стрелки, Home/End, Enter и
//! пробел, Esc, Tab и буквы. Панель — [`popup`]: `deferred` без маски (выходит за прокручиваемые
//! области), `anchored` в координатах окна, место списка — `place_dropdown` 0.4.9 по размеру
//! содержимого (первый кадр меряет его невидимым, как `visibility: hidden` до `positionMenu`),
//! переход `l-pop` (стекло и переход панели — [`PopLook`]); щелчок мимо панели и кнопки или
//! прокрутка вне панели закрывают.

use super::{
    Dismiss, FocusNext, FocusPrev, Handler, MenuDown, MenuEnd, MenuHome, MenuPick, MenuUp, Z_MENU,
};
use crate::liquid;
use crate::motion::{Origin, POP, presence};
use crate::paint::{ring, shadow};
use crate::scale::{u, zoom};
use crate::theme::Theme;
use gpui::{
    App, Bounds, Div, ElementId, InteractiveElement, IntoElement, MouseDownEvent, Pixels, Point,
    Rems, ScrollEvent, Size, Window, anchored, canvas, deferred, div, point, prelude::*,
};
use hlsr_ui::overlay::{
    self, DropdownPlace, DropdownRequest, MENU_PADDING, Rect, TYPEAHEAD_RESET_MS,
};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Контекст клавиш кнопки меню: стрелки, Home/End, Enter и пробел идут в меню.
pub const MENU_CONTEXT: &str = "Menu";

/// Клавиша у кнопки меню.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuKey {
    Up,
    Down,
    Home,
    End,
    /// Enter или пробел.
    Pick,
    /// Esc.
    Dismiss,
    /// Tab или Shift+Tab.
    Tab,
    /// Буква поиска (`isTypeaheadKey`).
    Char(String),
}

/// Что сделать экрану после клавиши.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Клавиша не меню: пусть идёт дальше (Esc закроет окно, Tab переведёт фокус).
    Pass,
    /// Принята, перерисовать.
    Handled,
    /// Выбран пункт; меню закрыто.
    Pick(usize),
}

impl Outcome {
    /// Остановить клавишу, если меню её приняло (зовётся в обработчике [`menu_keys`]).
    pub fn settle(self, cx: &mut App) -> Self {
        if self != Outcome::Pass {
            cx.stop_propagation();
        }
        self
    }
}

/// Открыто ли меню и какой пункт «в фокусе». Логика UiDropdown/GameMenu 0.4.9 из
/// `hlsr_ui::overlay`: недоступные пропускаются, по кругу; поиск по первым буквам (700 мс).
#[derive(Debug, Clone)]
pub struct MenuNav {
    pub open: bool,
    /// Пункт в фокусе; −1 — фокус на кнопке (открыто щелчком).
    pub active: i32,
    query: String,
    typed: Option<Instant>,
}

impl Default for MenuNav {
    fn default() -> Self {
        MenuNav {
            open: false,
            active: -1,
            query: String::new(),
            typed: None,
        }
    }
}

impl MenuNav {
    /// Открыть (`toggle` щелчком): фокус остаётся на кнопке.
    pub fn show(&mut self) {
        self.open = true;
        self.active = -1;
    }

    pub fn hide(&mut self) {
        self.open = false;
        self.active = -1;
    }

    /// Клавиша у кнопки. `labels` — подписи пунктов для поиска по буквам (пусто — поиска нет),
    /// `disabled` — недоступные пункты, `selected` — выбранный пункт списка (−1 — нет).
    pub fn key<S: AsRef<str>>(
        &mut self,
        key: &MenuKey,
        now: Instant,
        labels: &[S],
        disabled: &[bool],
        selected: i32,
    ) -> Outcome {
        let last = disabled.len() as i32 - 1;
        let first_enabled = overlay::enabled_index(disabled, 0, 1);
        let last_enabled = overlay::enabled_index(disabled, last, -1);
        match key {
            // openAndFocus(0) у кнопки; у пункта — следующий по кругу.
            MenuKey::Down | MenuKey::Up => {
                let down = *key == MenuKey::Down;
                self.active = if !self.open || self.active < 0 {
                    if down { first_enabled } else { last_enabled }
                } else {
                    overlay::menu_move(disabled, self.active, if down { 1 } else { -1 })
                };
                self.open = true;
                Outcome::Handled
            }
            MenuKey::Home | MenuKey::End if self.open => {
                self.active = if *key == MenuKey::Home {
                    first_enabled
                } else {
                    last_enabled
                };
                Outcome::Handled
            }
            MenuKey::Pick if self.open && self.active >= 0 => {
                let index = self.active as usize;
                if disabled.get(index).copied().unwrap_or(true) {
                    return Outcome::Handled;
                }
                self.hide();
                Outcome::Pick(index)
            }
            // Enter и пробел у самой кнопки — щелчок по ней.
            MenuKey::Pick => {
                if self.open {
                    self.hide();
                } else {
                    self.show();
                }
                Outcome::Handled
            }
            MenuKey::Dismiss if self.open => {
                self.hide();
                Outcome::Handled
            }
            // Tab закрывает меню, фокус уходит дальше от кнопки.
            MenuKey::Tab => {
                self.hide();
                Outcome::Pass
            }
            MenuKey::Char(text) if !labels.is_empty() => {
                let fresh = self.typed.is_none_or(|at| {
                    now.duration_since(at) >= Duration::from_millis(TYPEAHEAD_RESET_MS)
                });
                if fresh {
                    self.query.clear();
                }
                self.query.push_str(text);
                self.typed = Some(now);
                let current = if self.open && self.active >= 0 {
                    self.active
                } else {
                    selected
                };
                let found = overlay::find_typeahead_index(labels, &self.query, current);
                if found >= 0 {
                    self.active = overlay::enabled_index(disabled, found, 1);
                    self.open = true;
                }
                Outcome::Handled
            }
            _ => Outcome::Pass,
        }
    }
}

/// Клавиши меню у кнопки (`key_context` [`MENU_CONTEXT`]). `on_key` решает сам: принятую
/// клавишу останавливает ([`Outcome::settle`]), иначе она идёт дальше по окну.
pub fn menu_keys<E: InteractiveElement>(
    el: E,
    on_key: impl Fn(&MenuKey, &mut Window, &mut App) + 'static,
) -> E {
    let on_key = Rc::new(on_key);
    // Действие по умолчанию останавливается: дальше идёт, только если on_key не принял.
    let act = |key: MenuKey| {
        let on_key = on_key.clone();
        move |window: &mut Window, cx: &mut App| {
            cx.propagate();
            on_key(&key, window, cx);
        }
    };
    let (up, down, home, end) = (
        act(MenuKey::Up),
        act(MenuKey::Down),
        act(MenuKey::Home),
        act(MenuKey::End),
    );
    let (pick, dismiss, next, prev) = (
        act(MenuKey::Pick),
        act(MenuKey::Dismiss),
        act(MenuKey::Tab),
        act(MenuKey::Tab),
    );
    let chars = on_key.clone();
    el.key_context(MENU_CONTEXT)
        .on_action(move |_: &MenuUp, window, cx| up(window, cx))
        .on_action(move |_: &MenuDown, window, cx| down(window, cx))
        .on_action(move |_: &MenuHome, window, cx| home(window, cx))
        .on_action(move |_: &MenuEnd, window, cx| end(window, cx))
        .on_action(move |_: &MenuPick, window, cx| pick(window, cx))
        .on_action(move |_: &Dismiss, window, cx| dismiss(window, cx))
        .on_action(move |_: &FocusNext, window, cx| next(window, cx))
        .on_action(move |_: &FocusPrev, window, cx| prev(window, cx))
        .on_key_down(move |event, window, cx| {
            let keystroke = &event.keystroke;
            let modifiers = keystroke.modifiers;
            let Some(text) = keystroke.key_char.as_deref() else {
                return;
            };
            if overlay::is_typeahead_key(text, modifiers.control, modifiers.platform, modifiers.alt)
            {
                chars(&MenuKey::Char(text.to_owned()), window, cx);
            }
        })
}

/// Рамка кнопки меню в координатах окна (прошлого кадра): `.child(anchor_probe(cell))` внутри
/// кнопки с `relative`.
pub fn anchor_probe(cell: Rc<Cell<Bounds<Pixels>>>) -> impl IntoElement {
    canvas(move |bounds, _, _| cell.set(bounds), |_, _, _, _| {})
        .absolute()
        .top_0()
        .left_0()
        .size_full()
}

/// Где встаёт панель — список у кнопки (`UiDropdown.positionMenu`): `anchor` — кнопка, `clip` —
/// пересечение прокручиваемых предков (px окна); `up` — `direction: "up"`, `align_right` — по
/// правому краю.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    pub anchor: Bounds<Pixels>,
    pub clip: Option<Bounds<Pixels>>,
    pub up: bool,
    pub align_right: bool,
}

/// Панель всплывающего `.ui-dropdown__menu`: `--l-modal-bg`, радиус `--l-radius`, контур
/// `inset 0 0 0 1px --l-modal-border`, тень `--l-shadow-pop` (0 18px 48px rgba(0,0,0,.55)); в
/// теме Liquid Glass — стекло панели (контур и тень заменяет `--lg-rim`, `--lg-shadow`).
pub fn pop_surface(theme: &Theme, zoom: f32) -> Div {
    pop_surface_rounded(theme, zoom, theme.rem("l-radius"))
}

/// [`pop_surface`] со скруглением `radius` (у `notif-flow` оно идёт с переходом).
pub fn pop_surface_rounded(theme: &Theme, zoom: f32, radius: Rems) -> Div {
    liquid::glass(
        plain(theme, zoom, radius),
        liquid::Kind::Panel,
        radius,
        gpui::Pixels::ZERO,
        theme,
        zoom,
    )
}

/// [`pop_surface`] без стекла Liquid Glass: панели, которых нет в правилах темы (launcher.css
/// 0.4.9, 940–949) — `.profile-more__menu`, `.cf__keys`, свой поповер цвета.
pub fn pop_plain(theme: &Theme, zoom: f32) -> Div {
    plain(theme, zoom, theme.rem("l-radius"))
}

fn plain(theme: &Theme, zoom: f32, radius: Rems) -> Div {
    div()
        .bg(theme.color("l-modal-bg"))
        .rounded(radius)
        .shadow(vec![
            ring(zoom, 1., theme.color("l-modal-border")),
            shadow(zoom, 0., 18., 48., 0., gpui::black().opacity(0.55)),
        ])
}

/// Вид панели [`popup`]: стекло, переход и его точка опоры.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PopLook {
    /// Стекло Liquid Glass ([`pop_surface`]); уход тогда — вариант темы ([`liquid::motion`]).
    /// Без стекла — [`pop_plain`] и переход как есть во всех темах.
    pub glass: bool,
    /// Переход `<transition name>` из реестра движения.
    pub motion: &'static str,
    /// `transform-origin`; `None` — как у `.ui-dropdown__menu`: середина верхнего края (нижнего
    /// у списка над кнопкой).
    pub origin: Option<Origin>,
}

impl PopLook {
    /// `.ui-dropdown__menu`: стекло, `l-pop`.
    pub const DROPDOWN: PopLook = PopLook {
        glass: true,
        motion: POP,
        origin: None,
    };
}

/// Что панель помнит между кадрами.
#[derive(Default)]
struct Memory {
    /// Размер содержимого без ограничений (CSS px) — меряется в первом кадре открытия.
    natural: Cell<Option<Size<f32>>>,
    /// Где и какого размера панель (px окна); на время ухода.
    shown: Cell<Option<(Point<Pixels>, Option<DropdownPlace>)>>,
}

/// Панель меню: поверхность [`PopLook`] с полями 8 и прокруткой, `content` — пункты. Рисовать
/// всегда (и закрытой): уход доигрывает. `on_dismiss` — щелчок мимо панели и кнопки,
/// прокрутка вне панели (`onDocumentDown`, `onViewportChange`).
#[allow(clippy::too_many_arguments)]
pub fn popup<I, E, D, C>(
    id: I,
    open: bool,
    place: Place,
    look: PopLook,
    window: &mut Window,
    cx: &mut App,
    on_dismiss: D,
    content: C,
) -> impl IntoElement + use<I, E, D, C>
where
    I: Into<ElementId>,
    E: IntoElement,
    D: Fn(&mut Window, &mut App) + 'static,
    C: FnOnce() -> E + 'static,
{
    popup_scroll(id, open, place, look, None, window, cx, on_dismiss, content)
}

/// [`popup`], прокрутка панели — в `scroll` (список держит пункт с клавиатуры в виду, F11).
#[allow(clippy::too_many_arguments)]
pub fn popup_scroll<I, E, D, C>(
    id: I,
    open: bool,
    place: Place,
    look: PopLook,
    scroll: Option<crate::scroll::Scroll>,
    window: &mut Window,
    cx: &mut App,
    on_dismiss: D,
    content: C,
) -> impl IntoElement + use<I, E, D, C>
where
    I: Into<ElementId>,
    E: IntoElement,
    D: Fn(&mut Window, &mut App) + 'static,
    C: FnOnce() -> E + 'static,
{
    let id: ElementId = id.into();
    let memory = window
        .use_keyed_state(
            ElementId::NamedChild(std::sync::Arc::new(id.clone()), "popup".into()),
            cx,
            |_, _| Rc::new(Memory::default()),
        )
        .read(cx)
        .clone();
    let zoom = zoom(window);
    let view = window.viewport_size();
    let theme = cx.global::<Theme>();
    let (surface, motion) = if look.glass {
        (pop_surface(theme, zoom), liquid::motion(theme, look.motion))
    } else {
        (pop_plain(theme, zoom), look.motion)
    };
    if !open {
        memory.natural.set(None);
    }
    let anchor = place.anchor;
    // Место — по размеру содержимого; пока его нет — мерить невидимым у кнопки.
    let measuring = open && memory.natural.get().is_none();
    if open {
        let shown = match memory.natural.get() {
            Some(natural) => {
                let css = |value: Pixels| f64::from(f32::from(value) / zoom);
                let rect = |bounds: Bounds<Pixels>| Rect {
                    x: css(bounds.origin.x),
                    y: css(bounds.origin.y),
                    width: css(bounds.size.width),
                    height: css(bounds.size.height),
                };
                let dropdown = overlay::place_dropdown(&DropdownRequest {
                    anchor: rect(anchor),
                    clip: place.clip.map(rect),
                    window_width: css(view.width),
                    window_height: css(view.height),
                    content_width: f64::from(natural.width) - 2. * MENU_PADDING,
                    scroll_height: f64::from(natural.height),
                    up: place.up,
                    align_right: place.align_right,
                });
                let at = point(
                    crate::scale::zpx(zoom, dropdown.x as f32),
                    crate::scale::zpx(zoom, dropdown.y as f32),
                );
                Some((at, Some(dropdown)))
            }
            None => Some((anchor.bottom_left(), None)),
        };
        memory.shown.set(shown);
    }
    let (at, sized) = memory.shown.get().unwrap_or_default();
    let up = sized.map_or(place.up, |sized| sized.up);
    let on_dismiss: Handler = Rc::new(on_dismiss);
    let meta = crate::live::Meta::new("popup", id.clone()).checked(open);
    // Кадр замера (размер без ограничений) — свой переход: масштаб берёт размер без масштаба
    // со второго кадра, где у панели уже ширина и max-height места.
    let transit = if measuring {
        ElementId::NamedChild(Arc::new(id.clone()), "measure".into())
    } else {
        id.clone()
    };
    let scroll = scroll.unwrap_or_else(|| {
        let scroll = crate::scroll::Scroll::retained(id.clone(), window, cx);
        if measuring {
            scroll.scroll_to(Pixels::ZERO, false, window, cx);
        }
        scroll
    });
    let panel = presence(transit, open, motion, move || {
        let surface = surface.id("popup-surface").p(u(MENU_PADDING as f32));
        // Как 0.4.9: ширина на открытие, высота — max-height: пункты, сменившиеся при
        // открытом списке, меняют высоту без нового замера.
        let surface = match sized {
            Some(place) => scroll.overflow(
                surface
                    .w(u(place.width as f32))
                    .max_h(u(place.max_height as f32)),
            ),
            None => surface,
        }
        .child(content());
        // Слушатели — рядом с панелью, не внутри прокрутки: их рамка не едет с содержимым.
        let watch = if measuring {
            // Первый кадр: размер без ограничений, панель не видна и мышь не ловит.
            let natural = memory.clone();
            canvas(
                move |bounds, window, _| {
                    let size = bounds.size;
                    natural.natural.set(Some(Size {
                        width: f32::from(size.width) / zoom,
                        height: f32::from(size.height) / zoom,
                    }));
                    crate::motion::monitor_frame(window);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .into_any_element()
        } else {
            dismiss_watch(anchor, on_dismiss).into_any_element()
        };
        let panel = div().id("popup").relative().child(surface).child(watch);
        let panel = if measuring {
            panel.opacity(0.)
        } else {
            panel.occlude()
        };
        crate::live::mark(meta, panel)
    })
    .appear()
    // `.ui-dropdown__menu.down/.up` (после переворота): `transform-origin: 50% 0` / `50% 100%`.
    .origin(
        look.origin
            .unwrap_or(Origin::new((0.5, 0.), (if up { 1. } else { 0. }, 0.))),
    );
    deferred(anchored().position(at).snap_to_window().child(panel)).with_priority(Z_MENU)
}

/// Слушает окно, пока панель на экране: нажатие мимо панели и кнопки, прокрутка вне панели.
fn dismiss_watch(anchor: Bounds<Pixels>, on_dismiss: Handler) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let on_down = on_dismiss.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                let at = event.position;
                if phase.capture() && !bounds.contains(&at) && !anchor.contains(&at) {
                    on_down(window, cx);
                }
            });
            let on_scroll = on_dismiss;
            window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                if phase.capture() && !bounds.contains(&event.position) {
                    on_scroll(window, cx);
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}
