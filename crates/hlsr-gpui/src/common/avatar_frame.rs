//! Рамка аватара (`AvatarFrame.vue`, в 0.4.9 без изменений): слой позади аватара шире него на
//! `width` со всех сторон — CSS от админки или заготовки подсветки, растр
//! `hlsr_ui::render::frame` по кадрам (`frame-spin/hue/pulse`, свечения размыты на ЦП); в его
//! середине подложка `hole` (по умолчанию `--l-modal-bg`) под полупрозрачной заглушкой
//! аватара. Без рамки — только аватар.

use super::{Art, TICK, elapsed, rgba8};
use crate::kit::button::child_id;
use crate::motion::{AnimGate, keyframes};
use crate::scale::{u, zoom};
use crate::theme::Theme;
use gpui::prelude::*;
use gpui::{AnyElement, App, ElementId, Hsla, ObjectFit, SharedString, Window, canvas, div, img};
use hlsr_ui::render::profile_art::{self, ProfileArt};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

#[derive(IntoElement)]
pub struct AvatarFrame {
    id: ElementId,
    css: SharedString,
    size: f32,
    width: f32,
    radius: Option<f32>,
    hole: Option<Hsla>,
    child: AnyElement,
}

/// Аватар `child` стороной `size` (px CSS) в рамке; `id` — место на экране.
pub fn avatar_frame(id: impl Into<ElementId>, size: f32, child: impl IntoElement) -> AvatarFrame {
    AvatarFrame {
        id: id.into(),
        css: SharedString::default(),
        size,
        width: 3.,
        radius: None,
        hole: None,
        child: child.into_any_element(),
    }
}

impl AvatarFrame {
    /// CSS рамки (`frame.css` карточки профиля); пусто — без рамки.
    pub fn css(mut self, css: impl Into<SharedString>) -> Self {
        self.css = css.into();
        self
    }

    /// `width` — насколько рамка выступает за аватар, px CSS (по умолчанию 3).
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// `radius` в px CSS; по умолчанию `50%` (круг).
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = Some(radius);
        self
    }

    /// `hole` — подложка под аватаром (по умолчанию `--l-modal-bg`).
    pub fn hole(mut self, hole: impl Into<Hsla>) -> Self {
        self.hole = Some(hole.into());
        self
    }
}

struct State {
    start: Instant,
    ring: Art,
    visible: Rc<Cell<bool>>,
    last_key: String,
    last_art: Option<ProfileArt>,
}

impl RenderOnce for AvatarFrame {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let shell = div().relative().flex_none().size(u(self.size));
        if self.css.is_empty() {
            return shell.child(self.child).into_any_element();
        }
        let scale = zoom(window) * window.scale_factor();
        let still = AnimGate::still(cx);
        let now = cx.background_executor().now();
        let step_ms = profile_art::step_ms_for_hz(crate::motion::refresh_hz(window, cx));
        let state = window.use_keyed_state(child_id(&self.id, "frame"), cx, |_, cx| {
            cx.on_release(|state: &mut State, cx| state.ring.release(cx))
                .detach();
            State {
                start: now,
                ring: Art::default(),
                visible: Rc::new(Cell::new(true)),
                last_key: String::new(),
                last_art: None,
            }
        });
        let hole = self
            .hole
            .unwrap_or_else(|| cx.global::<Theme>().color("l-modal-bg"));
        let hole8 = rgba8(hole);
        let moving_frame = profile_art::frame_moving(&self.css, still);
        let (ms, visible, frame_key) = {
            let s = state.read(cx);
            let key = format!(
                "{}|{}|{}|{:?}|{hole8:?}|{scale}|{still:?}",
                self.css, self.size, self.width, self.radius
            );
            (elapsed(now, s.start), s.visible.clone(), key)
        };
        let cached_offscreen = state.read(cx);
        let art = if moving_frame
            && !visible.get()
            && cached_offscreen.last_key == frame_key
            && let Some(prev) = &cached_offscreen.last_art
        {
            prev.clone()
        } else {
            let art = profile_art::frame_art_paced(
                &self.css,
                self.size,
                self.width,
                self.radius,
                hole8,
                scale,
                ms,
                still,
                step_ms,
            );
            state.update(cx, |s, _| {
                s.last_key = frame_key;
                s.last_art = Some(art.clone());
            });
            art
        };
        let shown = art.styled.then_some(&art.image);
        let image = state.update(cx, |state, cx| state.ring.image(shown, window, cx));
        let ring = image.map(|image| {
            img(image)
                .absolute()
                .left(u(art.x))
                .top(u(art.y))
                .w(u(art.width))
                .h(u(art.height))
                .object_fit(ObjectFit::Fill)
                .opacity(art.opacity)
        });
        let probe = (art.styled && moving_frame).then(|| {
            let visible = visible.clone();
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    visible.set(bounds.intersects(&window.content_mask().bounds));
                },
            )
            .absolute()
            .inset_0()
        });
        // Слой рамки позади аватара (`z-index: -1` внутри `isolation: isolate`).
        let framed = shell.children(probe).children(ring).child(self.child);
        if art.styled && moving_frame {
            keyframes(child_id(&self.id, "tick"), TICK, move |_| framed).into_any_element()
        } else {
            framed.into_any_element()
        }
    }
}
