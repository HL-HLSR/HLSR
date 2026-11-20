//! Якоря обучения (`data-tour` 0.4.9): элемент, обёрнутый [`anchor`], пишет свою рамку при
//! prepaint каждого кадра, пока обучение идёт ([`begin`]). Обучение рисуется в `deferred` после
//! страницы и забирает рамки кадра ([`settle`]): якоря, которых в кадре не было, на экране нет.

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, SharedString, Window,
};
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Default)]
struct Anchors {
    active: bool,
    /// Рамки текущего кадра.
    frame: HashMap<SharedString, Bounds<Pixels>>,
    /// Рамки прошлого нарисованного кадра.
    last: HashMap<SharedString, Bounds<Pixels>>,
}

thread_local! {
    static ANCHORS: RefCell<Anchors> = RefCell::default();
}

/// Обучение началось: якоря пишут рамки с чистого листа.
pub fn begin() {
    ANCHORS.with(|anchors| {
        *anchors.borrow_mut() = Anchors {
            active: true,
            ..Anchors::default()
        }
    });
}

/// Идёт ли обучение (якоря пишут рамки каждого кадра).
pub fn active() -> bool {
    ANCHORS.with(|anchors| anchors.borrow().active)
}

/// Обучение кончилось: якоря больше ничего не пишут.
pub fn end() {
    ANCHORS.with(|anchors| *anchors.borrow_mut() = Anchors::default());
}

/// Кадр нарисован (prepaint обучения после страницы): его рамки — текущие.
pub fn settle() {
    ANCHORS.with(|anchors| {
        let mut anchors = anchors.borrow_mut();
        anchors.last = std::mem::take(&mut anchors.frame);
    });
}

/// Рамка якоря в прошлом кадре; нет или нулевой размер (`offsetWidth || offsetHeight`) — `None`.
pub fn get(name: &str) -> Option<Bounds<Pixels>> {
    ANCHORS.with(|anchors| {
        anchors
            .borrow()
            .last
            .get(name)
            .copied()
            .filter(|b| f32::from(b.size.width) > 0. || f32::from(b.size.height) > 0.)
    })
}

/// Элемент — якорь обучения с именем `name`.
pub fn anchor(name: impl Into<SharedString>, element: impl IntoElement) -> AnyElement {
    Anchor {
        name: name.into(),
        child: element.into_any_element(),
    }
    .into_any()
}

struct Anchor {
    name: SharedString,
    child: AnyElement,
}

impl Element for Anchor {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        ANCHORS.with(|anchors| {
            let mut anchors = anchors.borrow_mut();
            if anchors.active {
                anchors.frame.insert(self.name.clone(), bounds);
            }
        });
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};

    #[test]
    fn only_the_last_frame_counts_and_only_while_active() {
        let p = Pixels::from;
        let b = Bounds::new(point(p(1.), p(2.)), size(p(3.), p(4.)));
        let put = |name: &str, bounds| {
            ANCHORS.with(|a| {
                let mut a = a.borrow_mut();
                if a.active {
                    a.frame.insert(SharedString::from(name.to_owned()), bounds);
                }
            })
        };
        put("x", b);
        settle();
        assert_eq!(get("x"), None, "без обучения якоря не пишутся");
        begin();
        put("x", b);
        put("zero", Bounds::default());
        assert_eq!(get("x"), None, "кадр ещё не кончился");
        settle();
        assert_eq!(get("x"), Some(b));
        assert_eq!(get("zero"), None, "нулевой размер — якоря нет");
        settle();
        assert_eq!(get("x"), None, "в новом кадре якоря не было");
        end();
    }
}
