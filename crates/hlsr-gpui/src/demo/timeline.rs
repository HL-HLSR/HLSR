//! Шкала плеера «Демо» (`.dpl__bar` 0.4.9): дорожка, интервал экспорта, сыгранное, метки по
//! типам, головка, подсказка (метки точки и время). Нажатие ставит головку (к метке ближе 6 px),
//! дальше её тянут и за пределами шкалы (захват мыши окном, как `setPointerCapture`). Логика —
//! `controllers::demo_player`.

use super::card::marker_color;
use super::player::{DemoPlayer, props};
use crate::paint::{ring, shadow};
use crate::scale::{u, zoom};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Bounds, Context, DispatchPhase, FontWeight, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Role, SharedString, Window,
    canvas, div, prelude::*, px, relative,
};
use hlsr_ui::controllers::demo_player::Player;
use serde_json::Value;

/// Место `position` на шкале `bounds` (CSS px): x от левого края и ширина.
fn along(position: Point<Pixels>, bounds: Bounds<Pixels>, zoom: f32) -> (f64, f64) {
    (
        f64::from(f32::from(position.x - bounds.origin.x) / zoom),
        f64::from(f32::from(bounds.size.width) / zoom),
    )
}

impl DemoPlayer {
    /// Мышь над шкалой или, пока тянут, над окном.
    fn track_move(&mut self, position: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        let (x, width) = along(position, self.track.get(), zoom(window));
        let t = self.translate();
        let p = props(&self.props);
        if let Some(at) = self.s.track_move(x, width, &p, &t) {
            self.scrub(at, false, cx);
        }
        cx.notify();
    }

    fn track_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let (x, width) = along(event.position, self.track.get(), zoom(window));
        let p = props(&self.props);
        if let Some(at) = self.s.track_down(x, width, &p) {
            self.scrub(at, true, cx);
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Отпустили: видео — на место головки, если до него не дошли; курсор ушёл со шкалы —
    /// подсказки нет.
    fn track_up(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.s.track_up() {
            return;
        }
        if let Some(at) = self.unsought.take() {
            self.scrub(at, true, cx);
        }
        if !self.track.get().contains(&position) {
            self.s.hover = None;
        }
        cx.notify();
    }

    /// Шкала под видео.
    pub(super) fn timeline(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let p = props(&self.props);
        let s: &Player = &self.s;
        let length = s.length(&p);
        let idle = length == 0. || length.is_nan();
        let fraction = |t: f64| s.fraction(t, &p) as f32;
        let accent = theme.color("accent");
        let line = |el: gpui::Div| el.absolute().top(u(8.)).h(u(4.)).rounded(u(2.));

        let range = s.range_on_bar(&p).map(|(left, width)| {
            div()
                .absolute()
                .top(u(4.))
                .h(u(12.))
                .left(relative(left as f32))
                .w(relative(width as f32))
                .rounded(u(3.))
                .bg(accent.opacity(0.22))
                .shadow(vec![ring(zoom, 1., accent.opacity(0.55))])
        });
        let markers = p.markers.iter().map(|marker| {
            let kind = marker
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let t = marker.get("t").and_then(Value::as_f64).unwrap_or(f64::NAN);
            let height = if matches!(kind, "map" | "chapter" | "finish") {
                16.
            } else {
                14.
            };
            div()
                .absolute()
                .top(u(10. - height / 2.))
                .left(relative(fraction(t)))
                .ml(u(-1.5))
                .w(u(3.))
                .h(u(height))
                .rounded(u(2.))
                .bg(marker_color(kind, &theme))
                .shadow(vec![shadow(
                    zoom,
                    0.,
                    0.,
                    0.,
                    1.,
                    gpui::black().opacity(0.6),
                )])
        });
        let position = s.position(&p);
        let tip = s.hover.as_ref().map(|hover| {
            let label = (!hover.label.is_empty()).then(|| {
                let color = match hover.kind.as_str() {
                    "map" | "saveload" | "timer" | "chapter" | "finish" => {
                        marker_color(&hover.kind, &theme)
                    }
                    _ => theme.color("l-text"),
                };
                div()
                    .text_size(u(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color)
                    .child(hover.label.clone())
            });
            // `translateX(-50%)`: подсказка по центру нулевой колонки в точке курсора.
            div()
                .absolute()
                .bottom(u(26.))
                .left(relative(hover.left as f32))
                .w(px(0.))
                .flex()
                .justify_center()
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(u(8.))
                        .px(u(10.))
                        .py(u(5.))
                        .rounded(u(6.))
                        .bg(theme.color("l-surface-solid"))
                        .shadow(vec![
                            shadow(zoom, 0., 6., 18., 0., gpui::black().opacity(0.5)),
                            ring(zoom, 1., theme.color("l-border")),
                        ])
                        .whitespace_nowrap()
                        .children(label)
                        .child(
                            div()
                                .text_size(u(12.))
                                .tnum()
                                .text_color(theme.color("l-text-dim"))
                                .child(hover.time.clone()),
                        ),
                )
        });

        // Пока тянут: движение и отпускание по всему окну.
        let dragging = s.dragging;
        let player = cx.entity().downgrade();
        let capture = canvas(
            |_, _, _| (),
            move |_, (), window, _| {
                if !dragging {
                    return;
                }
                let moved = player.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let position = event.position;
                    // Кнопку отпустили там, где окно не слышало.
                    let released = event.pressed_button != Some(MouseButton::Left);
                    moved
                        .update(cx, |this, cx| {
                            if released {
                                this.track_up(position, cx);
                            } else {
                                this.track_move(position, window, cx);
                            }
                        })
                        .ok();
                });
                let up = player.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                        let position = event.position;
                        up.update(cx, |this, cx| this.track_up(position, cx)).ok();
                    }
                });
            },
        )
        .absolute()
        .size_full();

        let label: SharedString = self.translate()("#L_DEMO_TIMELINE", &[]).into();
        let track = div()
            .id("dpl-track")
            .relative()
            .h(u(20.))
            .role(Role::Slider)
            .aria_label(label)
            .aria_min_numeric_value(0.)
            .aria_max_numeric_value(length.round())
            .aria_numeric_value(position.round())
            .when(!idle, |el| el.cursor_pointer())
            .child(crate::layer::anchor_probe(self.track.clone()))
            .child(
                line(div())
                    .left_0()
                    .right_0()
                    .bg(gpui::white().opacity(0.14)),
            )
            .children(range)
            .child(
                line(div())
                    .left_0()
                    .w(relative(fraction(position)))
                    .bg(accent),
            )
            .children(markers)
            .child(
                div()
                    .absolute()
                    .top(u(4.))
                    .left(relative(fraction(position)))
                    .ml(u(-6.))
                    .size(u(12.))
                    .rounded_full()
                    .bg(gpui::white())
                    .shadow(vec![shadow(
                        zoom,
                        0.,
                        1.,
                        4.,
                        0.,
                        gpui::black().opacity(0.6),
                    )]),
            )
            .children(tip)
            .child(capture)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::track_down))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if !this.s.dragging {
                    this.track_move(event.position, window, cx);
                }
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && !this.s.dragging && this.s.hover.take().is_some() {
                    cx.notify();
                }
            }));
        div()
            .flex_none()
            .pt(u(10.))
            .px(u(14.))
            .pb(u(2.))
            .bg(theme.color("l-modal-bar"))
            .child(crate::snap::target("dpl-track", track))
            .into_any_element()
    }
}
