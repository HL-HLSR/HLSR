//! `transform: scale(k)` / `translate(x, y)` / `opacity` поддерева (`l-pop` .97, `gm` .96,
//! `toast` .96, `notif-flow` .08, `l-slide-*`, `:active` `press`): преобразование примитивов сцены
//! в фазе `paint` ([`Window::with_paint_transform`]). Раскладка, размер `rem`, кэш шрифтов и
//! кэш видов (`Entity::cached`) не срываются — переходы и масштабирование идут плавно на полной
//! частоте монитора без ступеней и перерастеризации глифов.

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Point, Size, Window, point,
};
use std::cell::Cell;
use std::rc::Rc;

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

/// Сохранённый тип-заглушка (ранее держал размер без масштаба для `rem × k`).
pub type Natural = Rc<Cell<Option<Size<Pixels>>>>;

/// Непрерывный коэффициент масштаба `k` (без ступеней: GPU масштабирует примитивы напрямую).
#[cfg(test)]
pub fn step(k: f64) -> f32 {
    k as f32
}

/// `child` в масштабе `k` вокруг `origin`.
pub fn scaled(k: f64, origin: Origin, _natural: Natural, child: impl IntoElement) -> Zoom {
    Zoom {
        offset: Point::default(),
        k: k as f32,
        origin,
        opacity: 1.,
        child: child.into_any_element(),
    }
}

/// `child` с полным преобразованием перехода в `paint`: сдвиг `offset`, масштаб `k` вокруг
/// `origin` и прозрачность `opacity`.
pub(super) fn transformed(
    offset: Point<Pixels>,
    k: f64,
    origin: Origin,
    opacity: f32,
    child: impl IntoElement,
) -> Zoom {
    Zoom {
        offset,
        k: k as f32,
        origin,
        opacity,
        child: child.into_any_element(),
    }
}

pub struct Zoom {
    offset: Point<Pixels>,
    k: f32,
    origin: Origin,
    opacity: f32,
    child: AnyElement,
}

impl Element for Zoom {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let zoom = f32::from(window.rem_size()) / 16.;
        let (ox, oy) = self.origin.at(bounds.size, zoom);
        let origin = point(bounds.origin.x + ox, bounds.origin.y + oy);
        let k = if self.k > 0. { self.k } else { 0. };
        window.with_paint_transform(self.offset, origin, k, self.opacity, |window| {
            self.child.paint(window, cx);
        });
    }
}
