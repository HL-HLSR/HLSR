//! Общий элемент движения: состояние живёт между кадрами по id элемента (как у пружин GPUI),
//! кадры просит только то, что движется, и не чаще общего предела окна — 60 кадров/с по сетке
//! vsync его монитора ([`monitor_frame`]). Бесконечное движение (`Need::Infinite`) кадров не
//! просит, когда [`AnimGate`] его держит или элемент вне видимой области (`content_mask`), а когда
//! просит — с тем же шагом, что и переходы (своего предела частоты у него нет).

use super::gate::AnimGate;
use gpui::{
    AnyElement, App, Bounds, Element, ElementId, EntityId, GlobalElementId, InspectorElementId,
    LayoutId, Pixels, Window, WindowId,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Нужен ли следующий кадр.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Idle,
    /// Переход или разовая анимация: до конца, без пауз.
    Finite,
    /// Повтор без конца: только когда видно.
    Infinite,
}

pub trait Drive: 'static {
    type State: 'static;
    fn init(&mut self, now: Instant) -> Self::State;
    fn build(
        &mut self,
        state: &mut Self::State,
        now: Instant,
        window: &mut Window,
        cx: &mut App,
    ) -> (AnyElement, Need);
    fn prepaint(_state: &mut Self::State, _bounds: Bounds<Pixels>, _window: &mut Window) {}
    fn paint(_state: &mut Self::State, _window: &mut Window) {}
}

thread_local! {
    /// Слой окон, внутри которого идёт prepaint ([`super::layer`]): 0 — основа.
    static LAYER: Cell<usize> = const { Cell::new(0) };
}

/// Слой окон, внутри которого идёт `prepaint` сейчас (0 — основа; `deferred` — тоже 0).
pub fn current_layer() -> usize {
    LAYER.get()
}

pub struct Driven<D: Drive> {
    pub(super) id: ElementId,
    pub(super) drive: D,
    pub(super) layer: Option<usize>,
}

impl<D: Drive> Element for Driven<D> {
    type RequestLayoutState = (AnyElement, Need);
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let now = cx.background_executor().now();
        let (mut child, need) =
            window.with_element_state(id.unwrap(), |state: Option<D::State>, window| {
                let mut state = state.unwrap_or_else(|| self.drive.init(now));
                (self.drive.build(&mut state, now, window, cx), state)
            });
        (child.request_layout(window, cx), (child, need))
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (child, need): &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let outer = LAYER.get();
        let layer = self.layer.unwrap_or(outer);
        LAYER.set(layer);
        child.prepaint(window, cx);
        LAYER.set(outer);
        // После ребёнка: хитбокс наведения ([`super::hover`]) ложится поверх его хитбоксов, и
        // ребёнок с `.occlude()` его не загораживает (как `:hover` предка в CSS).
        window.with_element_state(id.unwrap(), |state: Option<D::State>, window| {
            let mut state = state.expect("состояние кладёт request_layout");
            D::prepaint(&mut state, bounds, window);
            ((), state)
        });
        match need {
            Need::Idle => {}
            Need::Finite => monitor_frame(window),
            Need::Infinite => {
                if AnimGate::runs(cx, layer) && window.content_mask().bounds.intersects(&bounds) {
                    monitor_frame(window);
                }
            }
        }
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        (child, _): &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        child.paint(window, cx);
        window.with_element_state(id.unwrap(), |state: Option<D::State>, window| {
            let mut state = state.expect("состояние кладёт request_layout");
            D::paint(&mut state, window);
            ((), state)
        });
    }
}

/// Кто ждёт кадр монитора в окне.
#[derive(Default)]
struct Lane {
    views: Vec<EntityId>,
    /// Цепочка ожидания уже идёт.
    armed: bool,
    /// Опора сетки кадров: когда виды получили прошлый такой кадр.
    last: Option<Instant>,
}

thread_local! {
    /// Одна сетка кадров на окно: переходы, бесконечное движение и «ещё кадр после раскладки»
    /// встают на одни и те же кадры монитора. У бесконечного была своя сетка, и вместе с
    /// переходом кадры окна шли со сдвигом фазы: 4/4/8 мс на мониторе 120 Гц.
    static LANES: RefCell<HashMap<WindowId, Lane>> = RefCell::default();
}

/// Кадр движения (вместо `request_animation_frame`): текущий вид перерисуется с ближайшим кадром
/// окна — и переходы, и бесконечное движение. Кадры окна — не чаще 60 в секунду, каждый k-й
/// vblank его монитора (`gpui::frame_divisor`, решение владельца 19.11). На Windows окно будится
/// только так (`vendor/gpui-pre-windows`, `HLSR-PATCH.md`); окна на разных мониторах или Linux —
/// такт композиции, при мониторах разной частоты — самого быстрого, и сетка ниже ([`due`]) держит
/// кадры окна на том же шаге по монитору окна. И «ещё кадр после раскладки» (рамка
/// прошлого кадра, замер) — отсюда же: в покое он идёт со следующим будильником, а пока что-то
/// движется — с кадром движения. Свой `request_animation_frame` у замера, который меняется
/// каждый кадр (страница под выдвигающейся колонкой), гнал окно с частотой композиции.
pub fn monitor_frame(window: &Window) {
    monitor_notify(window, window.current_view());
}

/// То же для вида `view` вне его отрисовки (готов ответ фоновой задачи, которую ведёт движение):
/// он перерисуется с ближайшим кадром монитора вместе с остальным движением окна, а не
/// отдельным кадром между ними.
pub fn monitor_notify(window: &Window, view: EntityId) {
    let id = window.window_handle().window_id();
    let arm = LANES.with_borrow_mut(|all| {
        let lane = all.entry(id).or_default();
        if !lane.views.contains(&view) {
            lane.views.push(view);
        }
        !std::mem::replace(&mut lane.armed, true)
    });
    if arm {
        chain(window, id);
    }
}

/// Кадр в `now` при опоре `last` и кадре монитора окна `refresh`: `None` — рано, иначе новая
/// опора. Шаг сетки — общий предел 60 кадров/с: `gpui::frame_divisor` кадров монитора (на 120 Гц —
/// 2, на 280 Гц — 5). Будильник окна может идти в такт композиции, а не монитора окна, поэтому
/// допуск в обе стороны — четверть шага, но не больше полкадра монитора (иначе на 280 Гц кадр
/// вставал бы на будильник раньше нужного): кадр встаёт на ближайший к сетке будильник, опоздание
/// в допуске сетку не сдвигает (как `FramePacer` GPUI), большее — начинает её заново.
fn due(now: Instant, last: Option<Instant>, refresh: Duration) -> Option<Instant> {
    let Some(last) = last else {
        return Some(now);
    };
    let step = refresh * gpui::frame_divisor(refresh);
    let slack = (step / 4).min(refresh / 2);
    let deadline = last + step;
    if now + slack < deadline {
        return None;
    }
    Some(if now.saturating_duration_since(deadline) <= slack {
        deadline
    } else {
        now
    })
}

fn chain(window: &Window, id: WindowId) {
    window.on_next_frame(move |window, cx| {
        let now = cx.background_executor().now();
        // Монитор неизвестен — шаг 1/60 с, без сетки vsync.
        let refresh = window
            .refresh_interval(cx)
            .unwrap_or(Duration::from_secs(1) / 60);
        let ready = LANES.with_borrow_mut(|all| {
            let Some(lane) = all.get_mut(&id) else {
                return Some(Vec::new());
            };
            lane.last = Some(due(now, lane.last, refresh)?);
            lane.armed = false;
            Some(std::mem::take(&mut lane.views))
        });
        match ready {
            Some(views) => views.into_iter().for_each(|view| cx.notify(view)),
            None => chain(window, id),
        }
    });
}

/// Слой окна: содержимое без своего движения (см. [`super::layer`]).
pub struct Pass(pub(super) Option<AnyElement>);

impl Drive for Pass {
    type State = ();

    fn init(&mut self, _now: Instant) {}

    fn build(
        &mut self,
        _state: &mut (),
        _now: Instant,
        _window: &mut Window,
        _cx: &mut App,
    ) -> (AnyElement, Need) {
        (self.0.take().expect("один build на кадр"), Need::Idle)
    }
}

#[cfg(test)]
mod pacing {
    use super::*;

    /// Кадры за секунду будильников `wake` у окна на мониторе с кадром `refresh`.
    fn frames(wake: Duration, refresh: Duration) -> Vec<Instant> {
        let start = Instant::now();
        let mut last = None;
        let mut frames = Vec::new();
        for tick in 0..(Duration::from_secs(1).as_nanos() / wake.as_nanos()) as u32 {
            let now = start + wake * tick;
            if let Some(next) = due(now, last, refresh) {
                last = Some(next);
                frames.push(now);
            }
        }
        frames
    }

    fn hz(hz: u32) -> Duration {
        Duration::from_nanos(1_000_000_000 / u64::from(hz))
    }

    #[test]
    fn frames_follow_the_window_monitor_not_the_fastest_one() {
        // Будильник окна — 280 Гц (DWM по самому быстрому монитору), окно на мониторе 120 Гц:
        // 60 кадров, ни один не ближе 3/4 шага к прошлому.
        let frames = frames(hz(280), hz(120));
        assert!((57..=61).contains(&frames.len()), "{}", frames.len());
        for pair in frames.windows(2) {
            assert!(pair[1] - pair[0] > Duration::from_micros(12_500));
        }
    }

    /// `gpui::frame_divisor` (`vendor/gpui-pre`, его тесты вне workspace не собираются).
    #[test]
    fn the_window_draws_every_kth_monitor_frame_up_to_60_per_second() {
        for (refresh, divisor) in [
            (30, 1),
            (60, 1),
            // Через кадр было бы реже 45 к/с — каждый кадр (кадров чуть больше 60).
            (61, 1),
            (75, 1),
            (88, 1),
            (90, 2),
            (100, 2),
            (120, 2),
            (125, 2),
            (135, 3),
            (144, 3),
            (165, 3),
            (240, 4),
            (280, 5),
            (360, 6),
        ] {
            assert_eq!(gpui::frame_divisor(hz(refresh)), divisor, "{refresh} Гц");
        }
        // 59,94 и 60,4 Гц — каждый кадр; 119,88 Гц — каждый второй.
        for (refresh, divisor) in [(59.94, 1), (60.4, 1), (119.88, 2)] {
            let frame = Duration::from_secs_f64(1. / refresh);
            assert_eq!(gpui::frame_divisor(frame), divisor, "{refresh} Гц");
        }
        assert_eq!(gpui::frame_divisor(Duration::ZERO), 1);
    }

    #[test]
    fn frames_stay_on_the_vsync_grid_up_to_60_per_second() {
        // Будильник с каждым кадром монитора: кадр — на каждый k-й, ровно.
        for (refresh, every) in [(60, 1), (120, 2), (144, 3), (165, 3), (240, 4), (280, 5)] {
            let frames = frames(hz(refresh), hz(refresh));
            for pair in frames.windows(2) {
                assert_eq!(pair[1] - pair[0], hz(refresh) * every, "{refresh} Гц");
            }
            assert!(frames.len() <= 61, "{refresh} Гц: {}", frames.len());
        }
        // Будильник уже по пределу (Windows: поток vsync будит каждым k-м кадром) — кадр на каждый.
        let mut last = None;
        let start = Instant::now();
        for tick in 0..60 {
            let now = start + hz(280) * 5 * tick;
            last = Some(due(now, last, hz(280)).expect("кадр на каждый будильник"));
        }
    }
}
