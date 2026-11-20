//! CSS `transition` числа 0…1: точная длительность и кривая, смена цели — от текущего
//! значения. [`hover`] сам следит за мышью над элементом (как `:hover` с `transition:
//! … var(--l-fast)`): элемент без своей сущности и полей.

use super::driver::{Drive, Driven, Need};
use super::gate::AnimGate;
use super::{ms, registry};
use gpui::{
    AnyElement, App, Bounds, DispatchPhase, ElementId, Hitbox, HitboxBehavior, IntoElement,
    MouseMoveEvent, Pixels, Window,
};
use hlsr_ui::motion::CubicBezier;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

/// Время и кривая перехода.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    pub ms: f64,
    pub curve: CubicBezier,
}

impl Timing {
    pub fn new(ms: f64, curve: [f64; 4]) -> Timing {
        Timing {
            ms,
            curve: CubicBezier::new(curve),
        }
    }

    /// `--l-fast`: `--l-dur-fast` (.16 с) `--l-ease` — наведение, нажатие, фокус.
    pub fn fast() -> Timing {
        let tokens = &registry().tokens;
        Timing::new(tokens.l_dur_fast, tokens.l_ease)
    }

    /// `--l-med`: `--l-dur` (.26 с) `--l-ease`.
    pub fn med() -> Timing {
        let tokens = &registry().tokens;
        Timing::new(tokens.l_dur, tokens.l_ease)
    }

    /// `--l-dur-fade` (.5 с) `--l-ease-fade`: перетекание фона (`.launcher-bg-fade-*`).
    pub fn fade() -> Timing {
        let tokens = &registry().tokens;
        Timing::new(tokens.l_dur_fade, tokens.l_ease_fade)
    }

    /// Доля пути через `ms` мс от начала (кривая применена; до начала — 0, после — 1).
    pub fn at(&self, ms: f64) -> f32 {
        if self.ms <= 0. || ms >= self.ms {
            return 1.;
        }
        self.curve.solve((ms / self.ms).max(0.)) as f32
    }
}

pub struct Tween {
    id: ElementId,
    /// `None` — цель «мышь над элементом».
    target: Option<f32>,
    timing: Timing,
    child: Option<Box<dyn FnOnce(f32) -> AnyElement>>,
}

/// Значение, плавно идущее к `target`; `child` строит элемент по нему.
pub fn tween<E: IntoElement>(
    id: impl Into<ElementId>,
    target: f32,
    timing: Timing,
    child: impl FnOnce(f32) -> E + 'static,
) -> Tween {
    Tween::new(id.into(), Some(target), timing, child)
}

/// Наведение: 0 → 1, пока мышь над элементом, с переходом `timing` (обычно [`Timing::fast`]).
pub fn hover<E: IntoElement>(
    id: impl Into<ElementId>,
    timing: Timing,
    child: impl FnOnce(f32) -> E + 'static,
) -> Tween {
    Tween::new(id.into(), None, timing, child)
}

impl Tween {
    fn new<E: IntoElement>(
        id: ElementId,
        target: Option<f32>,
        timing: Timing,
        child: impl FnOnce(f32) -> E + 'static,
    ) -> Tween {
        Tween {
            id,
            target,
            timing,
            child: Some(Box::new(move |value| child(value).into_any_element())),
        }
    }
}

impl IntoElement for Tween {
    type Element = Driven<Tween>;

    fn into_element(self) -> Self::Element {
        Driven {
            id: self.id.clone(),
            drive: self,
            layer: None,
        }
    }
}

/// Ход числа к цели; смена цели — от текущего значения (у [`Tween`] и [`super::press`]).
pub(super) struct Ease {
    from: f32,
    to: f32,
    since: Option<Instant>,
}

impl Ease {
    pub(super) fn at(value: f32) -> Ease {
        Ease {
            from: value,
            to: value,
            since: None,
        }
    }

    /// Значение в `now` при цели `target` и нужен ли следующий кадр.
    pub(super) fn step(
        &mut self,
        target: f32,
        timing: Timing,
        now: Instant,
        cx: &App,
    ) -> (f32, Need) {
        let mut value = self.to;
        if let Some(since) = self.since {
            let t = (ms(now, since) / timing.ms).clamp(0., 1.);
            value = self.from + (self.to - self.from) * timing.curve.solve(t) as f32;
            if t >= 1. {
                self.since = None;
            }
        }
        if target != self.to {
            self.from = value;
            self.to = target;
            self.since = Some(now);
        }
        if AnimGate::still(cx).any() || timing.ms <= 0. {
            self.since = None;
            value = self.to;
        }
        let need = if self.since.is_some() {
            Need::Finite
        } else {
            Need::Idle
        };
        (value, need)
    }
}

pub struct TweenState {
    ease: Ease,
    /// Только у [`hover`]: мышь над элементом (пишет обработчик движения мыши).
    hovered: Option<Rc<Cell<bool>>>,
    /// Окно под курсором в прошлом кадре ([`Window::is_window_hovered`]).
    window_hovered: bool,
    hitbox: Option<Hitbox>,
}

impl Drive for Tween {
    type State = TweenState;

    fn init(&mut self, _now: Instant) -> TweenState {
        TweenState {
            ease: Ease::at(self.target.unwrap_or(0.)),
            hovered: self.target.is_none().then(Rc::default),
            window_hovered: false,
            hitbox: None,
        }
    }

    fn build(
        &mut self,
        state: &mut TweenState,
        now: Instant,
        window: &mut Window,
        cx: &mut App,
    ) -> (AnyElement, Need) {
        if let Some(hovered) = &state.hovered {
            // Курсор ушёл за край окна без MouseMove внутри: GPUI на Windows шлёт не
            // MouseExited, а только смену наведения окна (с перерисовкой всех видов). Сброс
            // по переходу true → false: синтетическая мышь (снимки) окно не наводит.
            let inside = window.is_window_hovered();
            if state.window_hovered && !inside {
                hovered.set(false);
            }
            state.window_hovered = inside;
        }
        let target = match &state.hovered {
            Some(hovered) => f32::from(u8::from(hovered.get())),
            None => self.target.unwrap_or(0.),
        };
        let (value, need) = state.ease.step(target, self.timing, now, cx);
        let child = self.child.take().expect("один build на кадр");
        (child(value), need)
    }

    fn prepaint(state: &mut TweenState, bounds: Bounds<Pixels>, window: &mut Window) {
        if state.hovered.is_some() {
            state.hitbox = Some(window.insert_hitbox(bounds, HitboxBehavior::Normal));
        }
    }

    fn paint(state: &mut TweenState, window: &mut Window) {
        let (Some(hovered), Some(hitbox)) = (state.hovered.clone(), state.hitbox.take()) else {
            return;
        };
        let view = window.current_view();
        // Под неподвижной мышью сменилась раскладка или слой (окно-диалог открылось поверх,
        // кнопка уехала): наведение — по попаданию этого кадра, как у Chromium после раскладки.
        // С клавиатуры наведение не снимается (`is_hovered` тогда всегда ложь); мышь за окном —
        // наведение снял `build`, здесь оно не возвращается.
        if !window.last_input_was_keyboard() {
            let now = hitbox.is_hovered(window);
            if now != hovered.get() && (!now || window.is_window_hovered()) {
                hovered.set(now);
                super::monitor_notify(window, view);
            }
        }
        window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
            let now = hitbox.is_hovered(window);
            if phase == DispatchPhase::Capture && now != hovered.get() {
                hovered.set(now);
                cx.notify(view);
            }
        });
    }
}
