//! Нажатие кнопок 0.4.9: `:active { transform: scale(k) }` с `transition: transform <время>`.
//! Пока левая кнопка мыши зажата на элементе (отпущена — где угодно, как `:active` в Chromium),
//! элемент в масштабе `k` вокруг центра ([`press`]; [`super::scaled`], место в потоке не
//! меняется). Где нажатие двигает не сам элемент (бегунок переключателя, сердце), состояние
//! `:active` отдаёт [`active`]. Нажатие и отпускание перерисовывают вид, кадры просит только
//! сам переход.

use super::driver::{Drive, Driven, Need};
use super::tween::{Ease, Timing};
use super::zoom::{self, Natural, Origin};
use gpui::{
    AnyElement, App, Bounds, DispatchPhase, ElementId, Hitbox, HitboxBehavior, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Styled, Window,
};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

/// `.gx-btn`, `.download-bar__play`… — сколько и как быстро (см. [`press`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Push {
    pub k: f64,
    pub timing: Timing,
}

impl Push {
    pub fn new(k: f64, timing: Timing) -> Push {
        Push { k, timing }
    }

    /// `scale(k)` с `transform var(--l-fast)` — так у большинства кнопок 0.4.9.
    pub fn fast(k: f64) -> Push {
        Push::new(k, Timing::fast())
    }
}

/// `:active` элемента: левая кнопка мыши зажата на нём, пока её не отпустят где угодно.
#[derive(Default)]
pub struct Held {
    held: Rc<Cell<bool>>,
    live: bool,
    hitbox: Option<Hitbox>,
}

impl Held {
    /// Зажата ли сейчас; неживое (`:disabled`) отпускает начатое нажатие.
    fn get(&mut self, live: bool) -> bool {
        self.live = live;
        if !live {
            self.held.set(false);
        }
        self.held.get()
    }

    /// Хитбокс поверх ребёнка (его хитбоксы под ним не гаснут, `HitboxBehavior::Normal`).
    fn prepaint(&mut self, bounds: Bounds<Pixels>, window: &mut Window) {
        self.hitbox = self
            .live
            .then(|| window.insert_hitbox(bounds, HitboxBehavior::Normal));
    }

    fn paint(&mut self, window: &mut Window) {
        let Some(hitbox) = self.hitbox.take() else {
            return;
        };
        let view = window.current_view();
        let held = self.held.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Capture
                && event.button == MouseButton::Left
                && !held.get()
                && hitbox.is_hovered(window)
            {
                held.set(true);
                cx.notify(view);
            }
        });
        // Всегда, не только у зажатой: нажатие и отпускание могут прийти до следующего кадра.
        let up = self.held.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture && event.button == MouseButton::Left && up.get() {
                up.set(false);
                cx.notify(view);
            }
        });
        // Отпущена без MouseUp (Alt+Tab, потеря захвата): первое движение без левой кнопки.
        let held = self.held.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture
                && event.pressed_button != Some(MouseButton::Left)
                && held.get()
            {
                held.set(false);
                cx.notify(view);
            }
        });
    }
}

pub struct Press<E> {
    id: ElementId,
    push: Push,
    live: bool,
    /// Принудительное `:active` (витрина, сцены без мыши).
    forced: bool,
    child: Option<E>,
}

/// `child` сжимается по `push`, пока на нём зажата мышь. Место ребёнка у родителя на время
/// масштаба берёт обёртка, поэтому `child` — сам элемент со своими `margin`/`absolute`.
pub fn press<E: IntoElement + Styled + 'static>(
    id: impl Into<ElementId>,
    push: Push,
    child: E,
) -> Press<E> {
    Press {
        id: id.into(),
        push,
        live: true,
        forced: false,
        child: Some(child),
    }
}

impl<E> Press<E> {
    /// `:not(:disabled)`: у неживого нажатия нет (и начатое отпускается).
    pub fn live(mut self, live: bool) -> Self {
        self.live = live;
        self
    }

    /// Нажата без мыши (`Preview::pressed`): сразу в масштабе, без перехода.
    pub fn forced(mut self, forced: bool) -> Self {
        self.forced = forced;
        self
    }

    fn target(&self, held: bool) -> f32 {
        f32::from(u8::from(self.live && (held || self.forced)))
    }
}

impl<E: IntoElement + Styled + 'static> IntoElement for Press<E> {
    type Element = Driven<Press<E>>;

    fn into_element(self) -> Self::Element {
        Driven {
            id: self.id.clone(),
            drive: self,
            layer: None,
        }
    }
}

pub struct PressState {
    ease: Ease,
    held: Held,
    natural: Natural,
}

impl<E: IntoElement + Styled + 'static> Drive for Press<E> {
    type State = PressState;

    fn init(&mut self, _now: Instant) -> PressState {
        PressState {
            ease: Ease::at(self.target(false)),
            held: Held::default(),
            natural: Natural::default(),
        }
    }

    fn build(
        &mut self,
        state: &mut PressState,
        now: Instant,
        _window: &mut Window,
        cx: &mut App,
    ) -> (AnyElement, Need) {
        let target = self.target(state.held.get(self.live));
        let (t, need) = state.ease.step(target, self.push.timing, now, cx);
        let k = 1. - (1. - self.push.k) * f64::from(t);
        let child = self.child.take().expect("один build на кадр");
        let zoomed = zoom::scaled(k, Origin::CENTER, state.natural.clone(), child);
        (zoomed.into_any_element(), need)
    }

    fn prepaint(state: &mut PressState, bounds: Bounds<Pixels>, window: &mut Window) {
        state.held.prepaint(bounds, window);
    }

    fn paint(state: &mut PressState, window: &mut Window) {
        state.held.paint(window);
    }
}

pub struct Active {
    id: ElementId,
    live: bool,
    child: Option<Box<dyn FnOnce(bool) -> AnyElement>>,
}

/// `:active` как есть: `child` строит элемент по тому, зажата ли на нём мышь (переход — у
/// того, что от этого меняется, своим [`super::tween`]).
pub fn active<E: IntoElement>(
    id: impl Into<ElementId>,
    live: bool,
    child: impl FnOnce(bool) -> E + 'static,
) -> Active {
    Active {
        id: id.into(),
        live,
        child: Some(Box::new(move |held| child(held).into_any_element())),
    }
}

impl IntoElement for Active {
    type Element = Driven<Active>;

    fn into_element(self) -> Self::Element {
        Driven {
            id: self.id.clone(),
            drive: self,
            layer: None,
        }
    }
}

impl Drive for Active {
    type State = Held;

    fn init(&mut self, _now: Instant) -> Held {
        Held::default()
    }

    fn build(
        &mut self,
        state: &mut Held,
        _now: Instant,
        _window: &mut Window,
        _cx: &mut App,
    ) -> (AnyElement, Need) {
        let held = state.get(self.live);
        let child = self.child.take().expect("один build на кадр");
        (child(held), Need::Idle)
    }

    fn prepaint(state: &mut Held, bounds: Bounds<Pixels>, window: &mut Window) {
        state.prepaint(bounds, window);
    }

    fn paint(state: &mut Held, window: &mut Window) {
        state.paint(window);
    }
}
