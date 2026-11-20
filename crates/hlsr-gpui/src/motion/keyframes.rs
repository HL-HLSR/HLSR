//! `@keyframes` по реестру (`animation: …` 0.4.9): задержка с лесенкой по индексу, повторы,
//! `alternate`, заполнение, правило `still`. Играет с монтирования (или с [`Keyframes::since`]);
//! заново — новый id.

use super::driver::{Drive, Driven, Need};
use super::gate::AnimGate;
use super::{apply, ms, registry};
use gpui::{AnyElement, App, ElementId, IntoElement, Styled, Window};
use hlsr_ui::motion::Frame;
use std::time::Instant;

type Build = Box<dyn FnOnce(&Frame) -> AnyElement>;

pub struct Keyframes {
    id: ElementId,
    name: &'static str,
    index: i32,
    since: Option<Instant>,
    child: Option<Build>,
}

/// Анимация реестра `name` (`hlsr_ui::motion::registry().animations`, плюс [`super::RISE_IN`]):
/// `child` строит элемент по кадру (прозрачность, сдвиг, поворот…).
pub fn keyframes<E: IntoElement>(
    id: impl Into<ElementId>,
    name: &'static str,
    child: impl FnOnce(&Frame) -> E + 'static,
) -> Keyframes {
    Keyframes {
        id: id.into(),
        name,
        index: 0,
        since: None,
        child: Some(Box::new(move |frame| child(frame).into_any_element())),
    }
}

/// `l-rise-in` (.26 с, `--l-ease`, лесенка 24 мс по `index`): строка, карточка, раздел.
pub fn rise_in<E: Styled + IntoElement + 'static>(
    id: impl Into<ElementId>,
    index: usize,
    el: E,
) -> Keyframes {
    keyframes(id, super::RISE_IN, move |frame| apply(el, frame)).index(index)
}

impl Keyframes {
    /// `--i` лесенки задержки.
    pub fn index(mut self, index: usize) -> Self {
        self.index = i32::try_from(index).unwrap_or(i32::MAX);
        self
    }

    /// Начало — момент `at`, а не монтирование: элемент, перестроенный в другом месте дерева
    /// (новый путь id — новое состояние), продолжает ту же анимацию, как один узел DOM.
    pub fn since(mut self, at: Instant) -> Self {
        self.since = Some(at);
        self
    }
}

impl IntoElement for Keyframes {
    type Element = Driven<Keyframes>;

    fn into_element(self) -> Self::Element {
        Driven {
            id: self.id.clone(),
            drive: self,
            layer: None,
        }
    }
}

impl Drive for Keyframes {
    type State = Instant;

    fn init(&mut self, now: Instant) -> Instant {
        self.since.unwrap_or(now)
    }

    fn build(
        &mut self,
        start: &mut Instant,
        now: Instant,
        _window: &mut Window,
        cx: &mut App,
    ) -> (AnyElement, Need) {
        let registry = registry();
        let still = AnimGate::still(cx);
        let elapsed = ms(now, *start);
        let frame = registry.keyframes_frame(self.name, Some(elapsed), self.index, still, true);
        let need = if !registry.keyframes_moving(self.name, still) {
            Need::Idle
        } else {
            match registry.keyframes_length(self.name, self.index, still) {
                None => Need::Infinite,
                Some(length) if elapsed < length => Need::Finite,
                Some(_) => Need::Idle,
            }
        };
        let child = self.child.take().expect("один build на кадр");
        (child(&frame), need)
    }
}
