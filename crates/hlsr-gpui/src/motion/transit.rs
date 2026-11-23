//! `<transition>` Vue: вход и уход по переходу реестра. Уходящее рисуется до конца ухода
//! (элемент живёт, пока не доиграл), смена ключа — старое уходит, новое входит одновременно
//! в той же ячейке (`l-value`, `l-slide-*`) или после (`mode="out-in"`). Прерванный переход
//! идёт от текущих значений. Как в Vue без `appear`, первый показ — без входа.

use super::driver::{Drive, Driven, Need};
use super::gate::AnimGate;
use super::zoom::{Origin, transformed};
use super::{ms, registry};
use gpui::{
    AnyElement, App, Div, ElementId, Empty, IntoElement, ParentElement, Pixels, StyleRefinement,
    Styled, Window, div, point, prelude::FluentBuilder, px,
};
use hlsr_ui::motion::Frame;
use std::time::{Duration, Instant};

/// Содержимое по ключу и кадру перехода.
type Child<K> = Box<dyn FnMut(&K, &Frame) -> AnyElement>;

pub struct Transit<K: Clone + PartialEq + 'static> {
    id: ElementId,
    name: &'static str,
    current: Option<K>,
    out_in: bool,
    appear: bool,
    /// Масштаб перехода и его `transform-origin`; `None` — масштаб не играет.
    origin: Option<Origin>,
    container: Option<Div>,
    child: Child<K>,
}

/// Элемент под `show`: появляется переходом `name`, уходя — доигрывает уход. Рисовать
/// всегда (и при `show = false`): состояние живёт, пока элемент есть в кадре.
pub fn presence<E: IntoElement>(
    id: impl Into<ElementId>,
    show: bool,
    name: &'static str,
    child: impl FnOnce() -> E + 'static,
) -> Transit<()> {
    presence_with(id, show, name, move |_| child())
}

/// [`presence`], где содержимое знает кадр перехода: то, чего в [`Frame`] нет (`border-radius`
/// у `notif-flow`), оно берёт из доли пути свойства, которое идёт с ним в такт.
pub fn presence_with<E: IntoElement>(
    id: impl Into<ElementId>,
    show: bool,
    name: &'static str,
    child: impl FnOnce(&Frame) -> E + 'static,
) -> Transit<()> {
    let mut child = Some(child);
    let child = move |_: &(), frame: &Frame| match child.take() {
        Some(child) => child(frame).into_any_element(),
        None => Empty.into_any_element(),
    };
    Transit::new(id.into(), name, show.then_some(()), Box::new(child))
}

/// Данные для [`presence`] под `show`: пока показано — `value` (и запоминается), уходящее —
/// последнее показанное. Vue уходящий узел `v-if` больше не обновляет: он гаснет с прежним
/// текстом, а не с тем, что уже пришло на смену. `id` — свой, живёт, пока его спрашивают.
pub fn held<T: Clone + 'static>(
    id: impl Into<ElementId>,
    show: bool,
    value: T,
    window: &mut Window,
    cx: &mut App,
) -> T {
    let memo = window.use_keyed_state(id, cx, |_, _| value.clone());
    if show {
        memo.update(cx, |memo, _| *memo = value.clone());
        value
    } else {
        memo.read(cx).clone()
    }
}

/// Содержимое по ключу: новый ключ — старое уходит, новое входит (`:key` в `<transition>`).
pub fn swap<K: Clone + PartialEq + 'static, E: IntoElement>(
    id: impl Into<ElementId>,
    key: K,
    name: &'static str,
    child: impl Fn(&K) -> E + 'static,
) -> Transit<K> {
    let child = move |key: &K, _: &Frame| child(key).into_any_element();
    Transit::new(id.into(), name, Some(key), Box::new(child))
}

/// Цепочка `v-if`/`v-else-if` с ключами в одном `<transition>`: `None` — ничего не показано
/// (прежнее уходит, нового нет; из ничего новое входит сразу, без ожидания ухода).
pub fn choose<K: Clone + PartialEq + 'static, E: IntoElement>(
    id: impl Into<ElementId>,
    key: Option<K>,
    name: &'static str,
    child: impl Fn(&K) -> E + 'static,
) -> Transit<K> {
    let child = move |key: &K, _: &Frame| child(key).into_any_element();
    Transit::new(id.into(), name, key, Box::new(child))
}

impl<K: Clone + PartialEq + 'static> Transit<K> {
    fn new(id: ElementId, name: &'static str, current: Option<K>, child: Child<K>) -> Self {
        Transit {
            id,
            name,
            current,
            out_in: false,
            appear: false,
            origin: None,
            container: Some(div()),
            child,
        }
    }

    /// `mode="out-in"`: новое входит, когда старое ушло.
    pub fn out_in(mut self) -> Self {
        self.out_in = true;
        self
    }

    /// Вход и при первом показе (`appear`).
    pub fn appear(mut self) -> Self {
        self.appear = true;
        self
    }

    /// Играть масштаб перехода (`l-pop` .97, `gm` .96, `toast` .96, `notif-flow` .08) вокруг
    /// `transform-origin`. Без него масштаб реестра не играет.
    pub fn origin(mut self, origin: Origin) -> Self {
        self.origin = Some(origin);
        self
    }
}

/// Контейнер: ячейка, где стоят входящее и уходящее (`overflow_hidden` для `l-slide-*`).
impl<K: Clone + PartialEq + 'static> Styled for Transit<K> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.container.as_mut().expect("до build").style()
    }
}

impl<K: Clone + PartialEq + 'static> IntoElement for Transit<K> {
    type Element = Driven<Transit<K>>;

    fn into_element(self) -> Self::Element {
        Driven {
            id: self.id.clone(),
            drive: self,
            layer: None,
        }
    }
}

pub struct Item<K> {
    key: K,
    entering: bool,
    name: &'static str,
    /// Начало фазы; `None` — вход доигран. В будущем — ждёт ухода старого (out-in).
    since: Option<Instant>,
    /// Значения в момент смены фазы, если прервана идущая.
    origin: Option<Frame>,
}

pub struct TransitState<K> {
    items: Vec<Item<K>>,
    /// Ширина ячейки прошлого кадра: `l-slide-*` сдвигают на 100 % + `--l-swipe-gap`.
    width: Pixels,
}

impl<K: Clone + PartialEq + 'static> Transit<K> {
    fn frame(item: &Item<K>, now: Instant, cx: &App) -> Frame {
        let elapsed = item.since.map(|since| ms(now, since));
        registry().transition_frame(
            item.name,
            item.entering,
            elapsed,
            AnimGate::still(cx),
            item.origin,
        )
    }
}

impl<K: Clone + PartialEq + 'static> Drive for Transit<K> {
    type State = TransitState<K>;

    fn init(&mut self, now: Instant) -> TransitState<K> {
        let items = self.current.clone().map(|key| Item {
            key,
            entering: true,
            name: self.name,
            since: self.appear.then_some(now),
            origin: None,
        });
        TransitState {
            items: items.into_iter().collect(),
            width: px(0.),
        }
    }

    fn build(
        &mut self,
        state: &mut TransitState<K>,
        now: Instant,
        window: &mut Window,
        cx: &mut App,
    ) -> (AnyElement, Need) {
        let still = AnimGate::still(cx);
        let length = |item: &Item<K>| registry().transition_length(item.name, item.entering, still);
        if self.out_in {
            // Сменившийся до показа ключ выходит из очереди. Новый ждёт только видимый уход.
            state.items.retain(|item| {
                !item.entering
                    || self.current.as_ref() == Some(&item.key)
                    || item.since.is_none_or(|since| since < now)
            });
        }
        // Смена фазы: текущее входит, остальное уходит — от значений этого кадра.
        for at in 0..state.items.len() {
            let item = &state.items[at];
            let entering = self.current.as_ref() == Some(&item.key);
            if entering != item.entering {
                let origin = Self::frame(item, now, cx);
                let item = &mut state.items[at];
                item.entering = entering;
                item.name = self.name;
                item.since = Some(now);
                item.origin = Some(origin);
            }
        }
        if let Some(key) = &self.current
            && !state.items.iter().any(|item| &item.key == key)
        {
            let wait = if self.out_in {
                let left = state.items.iter().filter_map(|item| {
                    let since = item.since?;
                    Some(length(item) - ms(now, since))
                });
                left.fold(0.0, f64::max)
            } else {
                0.0
            };
            state.items.push(Item {
                key: key.clone(),
                entering: true,
                name: self.name,
                since: Some(now + Duration::from_secs_f64(wait / 1000.)),
                origin: None,
            });
        }
        // Доигранный вход встаёт, доигранный уход исчезает.
        state.items.retain_mut(|item| {
            let Some(since) = item.since else {
                return true;
            };
            if now < since || ms(now, since) < length(item) {
                return true;
            }
            item.since = None;
            item.origin = None;
            item.entering
        });
        let running = state.items.iter().any(|item| item.since.is_some());
        // Ждущее out-in в кадре ещё нет; последнее — в потоке, прочие лежат поверх ячейки.
        let shown: Vec<&Item<K>> = state
            .items
            .iter()
            .filter(|item| item.since.is_none_or(|since| now >= since))
            .collect();
        let rem = window.rem_size();
        let gap = registry().tokens.l_swipe_gap as f32;
        let mut children = Vec::with_capacity(shown.len());
        for (at, item) in shown.iter().enumerate() {
            let frame = Self::frame(item, now, cx);
            let x = rem * (frame.x as f32 / 16.)
                + (state.width + rem * (gap / 16.)) * frame.slide as f32;
            let offset = point(x, rem * (frame.y as f32 / 16.));
            let scale = if self.origin.is_some() {
                frame.scale
            } else {
                1.
            };
            let wrap = div().size_full().flex().flex_col();
            let wrap = if at + 1 == shown.len() {
                wrap.relative()
            } else {
                wrap.absolute().top_0().left_0()
            };
            // Сдвиг, масштаб и прозрачность идут в `paint` (Window::with_paint_transform),
            // поэтому рамка, `rem` и кэш видов (`Entity::cached`) не срываются на всём переходе.
            let child = wrap.child(transformed(
                offset,
                scale,
                self.origin.unwrap_or_default(),
                frame.opacity as f32,
                (self.child)(&item.key, &frame),
            ));
            children.push(child.into_any_element());
        }
        let mut container = self.container.take().expect("один build на кадр");
        if container.style().position.is_none() {
            container = container.relative();
        }
        let container = container
            .when(children.is_empty(), |container| container.hidden())
            .children(children);
        let need = if running { Need::Finite } else { Need::Idle };
        (container.into_any_element(), need)
    }

    fn prepaint(state: &mut TransitState<K>, bounds: gpui::Bounds<Pixels>, _window: &mut Window) {
        state.width = bounds.size.width;
    }
}
