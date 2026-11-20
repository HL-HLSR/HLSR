//! `transform: scale(k)` поддерева (`l-pop` .97, `gm` .96, `toast` .96): у блоков GPUI нет
//! transform, поэтому поддерево раскладывается с rem × k (все размеры — rem, `scale.rs`) и стоит
//! в коробке прежнего размера так, что точка опоры (`transform-origin`) остаётся на месте. Снаружи
//! раскладка не меняется, как у transform. Размер без масштаба — с последнего кадра без масштаба;
//! пока его нет (первый кадр входа, прозрачность там 0), масштаба нет.
//!
//! Нажатие (`:active { transform: scale(.97) }`, [`super::press`]) масштабирует сам элемент в
//! потоке: его место у родителя (`position`, `inset`, `margin`, `align-self`) на кадры с масштабом
//! переходит коробке ([`Zoom::flow`]), а он заполняет её внутреннюю часть.

use gpui::{
    AnyElement, App, Bounds, Edges, Element, ElementId, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, Pixels, Position, Refineable, Size, Style, StyleRefinement, Styled,
    Window, relative,
};
use std::cell::Cell;
use std::rc::Rc;

/// Ступень масштаба: глифы растрируются под каждый размер шрифта, ступени их кэшируют (у .96…1 —
/// два десятка размеров на всё время работы), шаг — доли пикселя.
const STEPS: f32 = 512.;

/// Точка опоры: доля размера и сдвиг в CSS px (`calc(100% - 32px) 100%` — `x: (1., -32.)`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Origin {
    pub x: (f32, f32),
    pub y: (f32, f32),
}

impl Origin {
    /// `transform-origin` по умолчанию — центр.
    pub const CENTER: Origin = Origin {
        x: (0.5, 0.),
        y: (0.5, 0.),
    };

    pub const fn new(x: (f32, f32), y: (f32, f32)) -> Origin {
        Origin { x, y }
    }

    /// Точка в коробке `size` (px окна) при зуме `zoom`.
    fn at(self, size: Size<Pixels>, zoom: f32) -> (Pixels, Pixels) {
        (
            size.width * self.x.0 + crate::scale::zpx(zoom, self.x.1),
            size.height * self.y.0 + crate::scale::zpx(zoom, self.y.1),
        )
    }
}

impl Default for Origin {
    fn default() -> Self {
        Origin::CENTER
    }
}

/// Размер поддерева без масштаба: живёт между кадрами у владельца (элемент перехода).
pub type Natural = Rc<Cell<Option<Size<Pixels>>>>;

/// Ступень масштаба `k` (см. [`STEPS`]).
pub fn step(k: f64) -> f32 {
    ((k as f32) * STEPS).round() / STEPS
}

/// `child` в масштабе `k` вокруг `origin`; `natural` — его размер без масштаба (пишется здесь же).
pub fn scaled(k: f64, origin: Origin, natural: Natural, child: impl IntoElement) -> Zoom {
    Zoom {
        k: step(k),
        origin,
        natural,
        flow: StyleRefinement::default(),
        child: child.into_any_element(),
    }
}

/// Будет ли кадр в масштабе `k` (размер без масштаба уже есть) — то же условие, что у
/// [`Zoom::request_layout`].
pub(super) fn scales(k: f64, natural: &Natural) -> bool {
    let k = step(k);
    natural.get().is_some() && k != 1. && k > 0.
}

/// Место элемента у родителя: забирает у `el` то, что ставит его в поток, а сам `el`
/// растягивает на всю коробку (для [`Zoom::flow`]).
pub(super) fn take_flow(el: &mut impl Styled) -> StyleRefinement {
    let style = el.style();
    let flow = StyleRefinement {
        position: style.position.take(),
        inset: std::mem::take(&mut style.inset),
        margin: std::mem::take(&mut style.margin),
        align_self: style.align_self.take(),
        grid_location: style.grid_location.take(),
        ..StyleRefinement::default()
    };
    style.size.width = Some(relative(1.).into());
    style.size.height = Some(relative(1.).into());
    flow
}

pub struct Zoom {
    k: f32,
    origin: Origin,
    natural: Natural,
    /// Место у родителя на кадры с масштабом ([`take_flow`]).
    flow: StyleRefinement,
    child: AnyElement,
}

impl Zoom {
    pub(super) fn flow(mut self, flow: StyleRefinement) -> Zoom {
        self.flow = flow;
        self
    }
}

/// Кадр: rem поддерева (`None` — без масштаба).
pub struct Layout {
    rem: Option<Pixels>,
}

impl Element for Zoom {
    type RequestLayoutState = Layout;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    /// Без масштаба — сам `child`; с масштабом — коробка размера без масштаба, в ней `child` с
    /// rem × k там, где его оставил бы transform.
    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Layout) {
        let k = self.k;
        let Some(size) = self.natural.get().filter(|_| k != 1. && k > 0.) else {
            let child = self.child.request_layout(window, cx);
            return (child, Layout { rem: None });
        };
        let rem = window.rem_size();
        let inner_rem = rem * k;
        let child = window.with_rem_size(Some(inner_rem), |window| {
            self.child.request_layout(window, cx)
        });
        let (ox, oy) = self.origin.at(size, f32::from(rem) / 16.);
        let mut inner = Style {
            position: Position::Absolute,
            ..Style::default()
        };
        inner.inset = Edges {
            left: (ox * (1. - k)).into(),
            top: (oy * (1. - k)).into(),
            ..inner.inset
        };
        inner.size = Size {
            width: (size.width * k).into(),
            height: (size.height * k).into(),
        };
        let inner = window.request_layout(inner, [child], cx);
        let mut outer = Style {
            flex_shrink: 0.,
            ..Style::default()
        };
        outer.refine(&self.flow);
        outer.size = Size {
            width: size.width.into(),
            height: size.height.into(),
        };
        let outer = window.request_layout(outer, [inner], cx);
        (
            outer,
            Layout {
                rem: Some(inner_rem),
            },
        )
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Layout,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Размер без масштаба — только с кадра без масштаба: рамка / k с округлением до пикселя
        // за переход уползала на несколько px (содержимое по ширине коробки мерило само себя).
        if layout.rem.is_none() {
            self.natural.set(Some(bounds.size));
        }
        window.with_rem_size(layout.rem, |window| self.child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        layout: &mut Layout,
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_rem_size(layout.rem, |window| self.child.paint(window, cx));
    }
}
