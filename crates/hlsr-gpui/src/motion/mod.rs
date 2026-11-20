//! Движение 0.4.9 на GPUI. Кривые, длительности и кадры — реестр `hlsr_ui::motion`
//! (`data/motion.json`, выгрузка launcher.css 0.4.9), здесь только как они играют в окне:
//!
//! - [`presence`] / [`swap`] — переходы `<transition>` (`l-pop`, `l-fade`, `l-modal`, `l-swap`,
//!   `l-slide-next/prev`, `l-value` и прочие из реестра): уходящее живёт до конца ухода;
//! - [`keyframes`] / [`rise_in`] — `@keyframes` реестра (`l-rise-in` с лесенкой 24 мс,
//!   `l-pulse` = [`PULSE`], `fa-spin`, свечения…);
//! - [`tween`] / [`hover`] — CSS `transition` числа: точные .16 с `--l-ease` при наведении;
//! - [`press`] — `:active { transform: scale(.97) }` кнопок со своим временем ([`Push`]),
//!   [`active`] — само `:active` (бегунок переключателя, сердце спонсоров);
//! - [`AnimGate`] — `.no-anim`/reduced-motion и пауза бесконечного (окно спрятано, поверх
//!   окно-слой [`layer`], элемент вне видимой области);
//! - [`monitor_frame`] / [`monitor_notify`] — кадр движения (вместо `request_animation_frame`):
//!   у переходов, бесконечного движения и замеров одна сетка кадров — не чаще 60 в секунду, каждый
//!   k-й кадр монитора окна (`gpui::frame_divisor`). Время анимаций — по часам, не по кадрам.
//!
//! `transform` у блоков GPUI нет: сдвиг — `left/top` обёртки (раскладка не меняется, как у
//! translate), масштаб переходов (`l-pop` .97, `gm` .96, `toast` .96, `notif-flow` .08) — rem
//! поддерева в коробке прежнего размера ([`scaled`]), затемнение `l-modal` идёт прозрачностью
//! слоя вместе с окном (у 0.4.9 те же время и кривая, масштаба у `l-modal` нет).

mod driver;
mod gate;
mod keyframes;
mod press;
#[cfg(test)]
mod tests;
mod transit;
mod tween;
mod zoom;

pub use driver::current_layer;
pub use driver::{monitor_frame, monitor_notify};
pub use gate::AnimGate;
pub use hlsr_ui::motion::{Frame, Still};
pub use keyframes::{Keyframes, keyframes, rise_in};
pub use press::{Push, active, press};
pub use transit::{Transit, choose, held, presence, presence_with, swap};
pub use tween::{Timing, Tween, hover, tween};
pub use zoom::{Natural, Origin, scaled};

use crate::scale::u;
use driver::{Driven, Pass};
use gpui::{ElementId, Hsla, IntoElement, Rgba, Styled};
use hlsr_ui::motion::{self as ui, Prop, Registry};
use std::sync::OnceLock;
use std::time::Instant;

/// Всплывающее у кнопки: меню, поповеры, подсказки.
pub const POP: &str = "l-pop";
/// Проявление: оверлеи, заглушки.
pub const FADE: &str = "l-fade";
/// Модальное окно: затемнение и окно вместе.
pub const MODAL: &str = "l-modal";
/// Без перехода (`v-if` без `<transition>`): такого имени в реестре нет — встаёт и уходит сразу.
pub const INSTANT: &str = "none";
/// Смена содержимого на месте (подъём на `--l-rise`).
pub const SWAP: &str = "l-swap";
/// Вкладка правее / левее: свайп на 100 % + `--l-swipe-gap`.
pub const SLIDE_NEXT: &str = "l-slide-next";
pub const SLIDE_PREV: &str = "l-slide-prev";
/// Старое и новое значение растворяются друг в друге (.5 с `--l-ease-fade`).
pub const VALUE: &str = "l-value";
/// `@keyframes l-rise-in`: .26 с `--l-ease`, `both`, лесенка 24 мс.
pub const RISE_IN: &str = "l-rise-in";
/// `@keyframes l-pulse` 1,4 с (кольцо `fa-spin` при reduced-motion).
pub const PULSE: &str = "fa-pulse";
/// Кольцо `fa-spin`: поворот 0,85 с; при `.no-anim`/reduced-motion — [`PULSE`].
pub const SPIN: &str = "fa-spin";

/// Реестр 0.4.9 и то, чего в выгрузке нет: `l-swap` (в 0.4.9 есть в launcher.css, но ни
/// один компонент его не берёт) и общий `l-rise-in` (в выгрузке — по местам: `account-item`…).
pub fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = ui::registry().clone();
        let tokens = registry.tokens.clone();
        let timings = |duration: f64, easing: [f64; 4]| {
            let timing = |property: &str, props: Vec<Prop>| ui::Timing {
                property: property.into(),
                props,
                duration,
                easing,
                delay: 0.,
            };
            vec![
                timing("opacity", vec![Prop::Opacity]),
                timing(
                    "transform",
                    vec![Prop::X, Prop::Y, Prop::Slide, Prop::Scale],
                ),
            ]
        };
        let swap = ui::Transition {
            source: "launcher.css:470 (.l-swap-*)".into(),
            shown: Default::default(),
            hidden: [(Prop::Opacity, 0.), (Prop::Y, tokens.l_rise)].into(),
            leave_to: [(Prop::Opacity, 0.), (Prop::Y, -tokens.l_rise / 2.)].into(),
            enter: timings(tokens.l_dur, tokens.l_ease),
            leave: timings(tokens.l_dur_fast, tokens.l_ease_in),
        };
        registry.transitions.insert(SWAP.into(), swap);
        let mut rise = registry.animations["account-item"].clone();
        rise.selector = "l-rise-in var(--l-dur) var(--l-ease) both, --i × 24ms".into();
        registry.animations.insert(RISE_IN.into(), rise);
        registry
    })
}

/// Окно-слой номер `depth` (1 — первое открытое): бесконечное внутри идёт, пока слой
/// верхний ([`AnimGate::set_top_layer`]). Ставить внутри `deferred`, вокруг самого окна;
/// свой `deferred` внутри окна (меню) рисуется позже — его содержимое обернуть так же.
pub fn layer(depth: usize, child: impl IntoElement) -> impl IntoElement {
    let id = ElementId::Name(format!("motion-layer-{depth}").into());
    Driven {
        id,
        drive: Pass(Some(child.into_any_element())),
        layer: Some(depth),
    }
}

/// Прозрачность и сдвиг кадра у элемента в потоке: translate → `relative` + `left/top`.
/// У `absolute`-элемента сдвиг заменяет его `left/top`. Масштаб — обёрткой [`scaled`].
pub fn apply<E: Styled>(mut el: E, frame: &Frame) -> E {
    if frame.x != 0. || frame.y != 0. {
        if el.style().position.is_none() {
            el = el.relative();
        }
        el = el.left(u(frame.x as f32)).top(u(frame.y as f32));
    }
    el.opacity(frame.opacity as f32)
}

/// Цвет между `a` и `b` (`t` от [`hover`]/[`tween`]); как transition цвета в Chromium —
/// sRGB с предумноженной альфой (из прозрачного цвет не темнеет).
pub fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (Rgba::from(a), Rgba::from(b));
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    let alpha = lerp(a.a, b.a);
    let channel = |x: f32, y: f32| {
        if alpha > 0. {
            lerp(x * a.a, y * b.a) / alpha
        } else {
            0.
        }
    };
    Rgba {
        r: channel(a.r, b.r),
        g: channel(a.g, b.g),
        b: channel(a.b, b.b),
        a: alpha,
    }
    .into()
}

/// Мс от `since` до `now` (отрицательно, если `since` впереди).
fn ms(now: Instant, since: Instant) -> f64 {
    match now.checked_duration_since(since) {
        Some(elapsed) => elapsed.as_secs_f64() * 1000.,
        None => -(since.duration_since(now).as_secs_f64() * 1000.),
    }
}
