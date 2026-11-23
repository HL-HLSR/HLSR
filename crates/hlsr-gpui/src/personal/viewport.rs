//! Вид кадра превью с зумом и панорамой (`GhostingPreviewViewport.vue` 0.4.9): колесо,
//! перетаскивание, двойной щелчок, клавиши; панель «−», масштаб, «+», «Вписать», «100%»,
//! «Движение», «Развернуть»; перетаскивание оверлея LiveSplit за рамку-ручку. Логика вида —
//! `preview_view` (`hlsr-ui`); кадр — картинка GPUI на месте, которое он считает.
//!
//! `fill` (портрет): окно вида берёт всю высоту, что ему оставили, а ширину кадра подгоняет
//! хозяин (`box` → `PreviewController::set_stage_box`).

use super::preview::{GhostingPreview, Which};
use crate::icons::{fa_glyph, icon};
use crate::kit::button::{Look as BtnLook, button, focus_visible, outline};
use crate::layer::TipExt;
use crate::scale::{u, zoom};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Bounds, Context, CursorStyle, DispatchPhase, ElementId, FocusHandle, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ObjectFit, ParentElement, Pixels, Point, ScrollDelta, ScrollEvent,
    StatefulInteractiveElement, Styled, Window, canvas, div, fill, hsla, img, point, prelude::*,
    px, rgb, size,
};
use hlsr_ui::controllers::preview_view::{self as pv, Geometry, Handle, View};
use std::cell::Cell;
use std::rc::Rc;

/// Свойства вида (`framed`, `fill`, `expandable`); `wrap` — панель в карточке превью
/// переносится на вторую строку (`.gp :deep(.gv__bar)`), в окне во весь лаунчер — нет.
pub(super) struct ViewportProps {
    pub framed: bool,
    pub fill: bool,
    pub expandable: bool,
    pub wrap: bool,
}

/// Что тянут мышью.
#[derive(Clone, Copy)]
enum Drag {
    /// Кадр: точка нажатия (px окна) и вид в начале.
    Pan { x: f32, y: f32, start: View },
    /// Ручку оверлея: точка кадра под указателем от её левого верхнего угла.
    Handle { dx: f64, dy: f64 },
}

/// Состояние одного вида.
pub struct Viewport {
    view: View,
    geometry: Geometry,
    /// Окно вида (`.gv__stage`) в прошлом кадре.
    probe: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<Drag>,
    over_handle: bool,
    focus: FocusHandle,
}

impl Viewport {
    pub fn new(focus: FocusHandle) -> Viewport {
        let geometry = Geometry {
            box_width: 0.,
            box_height: 0.,
            frame_width: 1600.,
            frame_height: 900.,
        };
        Viewport {
            view: pv::initial(&geometry),
            geometry,
            probe: Rc::default(),
            drag: None,
            over_handle: false,
            focus,
        }
    }

    /// Другая ширина кадра: вид с начала, весь кадр (`watch frameWidth`).
    pub fn set_frame(&mut self, width: f64) {
        if self.geometry.frame_width != width {
            self.geometry.frame_width = width;
            self.view = pv::initial(&self.geometry);
        }
    }

    pub fn reset(&mut self) {
        self.view = pv::initial(&self.geometry);
    }

    /// Размер окна вида из прошлого кадра (`measure`); изменился — вид прижат к кадру.
    fn measure(&mut self, zoom: f32) -> bool {
        let size = self.probe.get().size;
        let (w, h) = (
            f64::from(f32::from(size.width) / zoom),
            f64::from(f32::from(size.height) / zoom),
        );
        if (w - self.geometry.box_width).abs() < 0.01 && (h - self.geometry.box_height).abs() < 0.01
        {
            return false;
        }
        self.geometry.box_width = w;
        self.geometry.box_height = h;
        self.view = pv::clamp(&self.view, &self.geometry);
        true
    }

    /// Точка окна вида (CSS px) под `position` окна.
    fn local(&self, position: Point<Pixels>, zoom: f32) -> (f64, f64) {
        let origin = self.probe.get().origin;
        (
            f64::from(f32::from(position.x - origin.x) / zoom),
            f64::from(f32::from(position.y - origin.y) / zoom),
        )
    }

    fn source(&self, position: Point<Pixels>, zoom: f32) -> (f64, f64) {
        let (x, y) = self.local(position, zoom);
        pv::source_point(&self.view, &self.geometry, x, y)
    }
}

/// Рамка ручки пунктиром в физических px (`ctx.setLineDash([5, 4])` двумя цветами): тёмная
/// линия целиком, поверх — светлые штрихи `[4, 9)` каждого периода 9 (светлая со сдвигом 5
/// перекрывает тёмную). Квадраты по 1 физ. px — от левого верхнего угла по часовой.
fn paint_handle(
    window: &mut Window,
    origin: Point<Pixels>,
    scale: f32,
    rect: (f64, f64, f64, f64),
    ratio: f64,
    light: Hsla,
) {
    let (dark_quads, light_quads) = handle_quads(rect, ratio);
    let dark = hsla(0., 0., 0., 0.7);
    let to = |v: f64| Pixels::from(v as f32 / scale);
    for (quads, color) in [(dark_quads, dark), (light_quads, light)] {
        for [x0, y0, x1, y1] in quads {
            window.paint_quad(fill(
                Bounds::new(
                    point(origin.x + to(x0), origin.y + to(y0)),
                    size(to(x1 - x0), to(y1 - y0)),
                ),
                color,
            ));
        }
    }
}

/// Квадраты рамки ручки в физ. px (`[x0, y0, x1, y1]`): тёмная линия целиком и светлые штрихи.
fn handle_quads((x, y, w, h): (f64, f64, f64, f64), ratio: f64) -> (Vec<[f64; 4]>, Vec<[f64; 4]>) {
    if w <= 0. || h <= 0. {
        return (Vec::new(), Vec::new());
    }
    // Линия по центрам пикселей (x + 0.5): квадрат на пиксель под ней.
    let (l, t, r, b) = (x - 0.5, y - 0.5, x + w + 0.5, y + h + 0.5);
    let dark = vec![
        [l, t, r, t + 1.],
        [l, b - 1., r, b],
        [l, t, l + 1., b],
        [r - 1., t, r, b],
    ];
    // Стороны пути `strokeRect`: верх → право → низ → лево.
    let sides = [
        ((x, y), (x + w, y)),
        ((x + w, y), (x + w, y + h)),
        ((x + w, y + h), (x, y + h)),
        ((x, y + h), (x, y)),
    ];
    let period = 9. * ratio;
    let (on_from, on_to) = (4. * ratio, 9. * ratio);
    let mut light = Vec::new();
    let mut walked = 0.;
    for ((x0, y0), (x1, y1)) in sides {
        let len = (x1 - x0).abs() + (y1 - y0).abs();
        let (dx, dy) = ((x1 - x0) / len, (y1 - y0) / len);
        let mut k = (walked / period).floor();
        while k * period + on_from - walked < len {
            let from = (k * period + on_from - walked).max(0.);
            let to = (k * period + on_to - walked).min(len);
            if to > from {
                let (ax, ay) = (x0 + dx * from, y0 + dy * from);
                let (bx, by) = (x0 + dx * to, y0 + dy * to);
                // Ширина линии — 1 физ. px поперёк стороны.
                let (qx0, qx1) = (ax.min(bx), ax.max(bx));
                let (qy0, qy1) = (ay.min(by), ay.max(by));
                light.push(if dy == 0. {
                    [qx0, qy0 - 0.5, qx1, qy1 + 0.5]
                } else {
                    [qx0 - 0.5, qy0, qx1 + 0.5, qy1]
                });
            }
            k += 1.;
        }
        walked += len;
    }
    (dark, light)
}

impl GhostingPreview {
    /// Колесо: масштаб у указателя (`onWheel`).
    fn wheel(&mut self, which: Which, event: &ScrollEvent, zoom: f32) {
        let vp = self.viewport(which);
        if vp.geometry.box_width <= 0. {
            return;
        }
        let delta_y = match event.delta {
            ScrollDelta::Lines(lines) => -f64::from(lines.y * crate::scroll::line_step()),
            ScrollDelta::Pixels(pixels) => -f64::from(f32::from(pixels.y) / zoom),
        };
        let (x, y) = vp.local(event.position, zoom);
        vp.view = pv::wheel(&vp.view, &vp.geometry, delta_y, x, y);
    }

    /// Нажатие в окне вида: ручка оверлея или кадр (`onPointerDown`); двойной щелчок вне
    /// ручки — весь кадр или 100 % у указателя (`onDoubleClick`).
    fn pointer_down(&mut self, which: Which, event: &MouseDownEvent, zoom: f32) {
        let handle = self.state.handle;
        let vp = self.viewport(which);
        if vp.geometry.box_width <= 0. {
            return;
        }
        let source = vp.source(event.position, zoom);
        let on_handle = handle.is_some_and(|h| h.contains(source));
        if event.click_count >= 2 && !on_handle {
            let (x, y) = vp.local(event.position, zoom);
            vp.view = pv::double_click(&vp.view, &vp.geometry, x, y);
        }
        vp.drag = Some(match handle.filter(|_| on_handle) {
            Some(h) => Drag::Handle {
                dx: source.0 - h.left,
                dy: source.1 - h.top,
            },
            None => Drag::Pan {
                x: f32::from(event.position.x),
                y: f32::from(event.position.y),
                start: vp.view,
            },
        });
    }

    /// Указатель двигается, пока тянут (захват окна) — `onPointerMove`.
    fn drag_to(&mut self, which: Which, position: Point<Pixels>, zoom: f32) {
        let vp = self.viewport(which);
        match vp.drag {
            Some(Drag::Pan { x, y, start }) => {
                let dx = f64::from((f32::from(position.x) - x) / zoom);
                let dy = f64::from((f32::from(position.y) - y) / zoom);
                vp.view = pv::drag(&start, &vp.geometry, dx, dy);
            }
            Some(Drag::Handle { dx, dy }) => {
                let (sx, sy) = vp.source(position, zoom);
                self.controller.move_overlay(sx - dx, sy - dy);
            }
            None => {}
        }
    }

    /// Указатель над окном вида без нажатия: над ручкой ли (`overHandle`).
    fn hover_at(&mut self, which: Which, position: Point<Pixels>, zoom: f32) -> bool {
        let handle = self.state.handle;
        let vp = self.viewport(which);
        if vp.drag.is_some() || vp.geometry.box_width <= 0. {
            return false;
        }
        let over = handle.is_some_and(|h| h.contains(vp.source(position, zoom)));
        let changed = over != vp.over_handle;
        vp.over_handle = over;
        changed
    }

    /// Клавиша окна вида (`onKey`): `true` — своя.
    fn key(&mut self, which: Which, event: &KeyDownEvent) -> bool {
        let m = &event.keystroke.modifiers;
        if m.alt || m.control || m.platform {
            return false;
        }
        let key = match event.keystroke.key.as_str() {
            "+" | "=" | "plus" => "+",
            "-" | "_" | "minus" => "-",
            "0" => "0",
            "1" => "1",
            "left" => "ArrowLeft",
            "right" => "ArrowRight",
            "up" => "ArrowUp",
            "down" => "ArrowDown",
            _ => return false,
        };
        let vp = self.viewport(which);
        match pv::key(&vp.view, &vp.geometry, key) {
            Some(view) => {
                vp.view = view;
                true
            }
            None => false,
        }
    }

    /// Вид кадра: окно с картинкой, ручкой и заглушкой и панель масштаба.
    pub(super) fn render_viewport(
        &mut self,
        which: Which,
        props: ViewportProps,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let ratio = f64::from(zoom * window.scale_factor());
        let frame_width = f64::from(self.state.frame_width);
        let images = self.level_images(window, cx);
        let (motion, animating) = (self.state.motion, self.state.animating);
        let (loading, message) = (self.state.loading, self.state.message);
        let handle: Option<Handle> = self.state.handle;
        let message = (!message.is_empty()).then(|| self.t(message, &[]));
        let label = self.t("#L_GS_X_OVERLAY_TITLE", &[]);
        let tr = |key: &str| self.t(key, &[]);
        let (zoom_out, zoom_in, fit, fit_title, actual, motion_label, expand) = (
            tr("#L_GS_X_ZOOM_OUT"),
            tr("#L_GS_X_ZOOM_IN"),
            tr("#L_GS_X_ZOOM_FIT"),
            tr("#L_GS_X_ZOOM_FIT_TITLE"),
            tr("#L_GS_X_ZOOM_ACTUAL"),
            tr("#L_GS_X_OVERLAY_MOTION"),
            tr("#L_GS_X_PREVIEW_EXPAND"),
        );

        let vp = self.viewport(which);
        vp.set_frame(frame_width);
        let changed = vp.measure(zoom);
        let geometry = vp.geometry;
        let box_size = (geometry.box_width, geometry.box_height);
        let paint = pv::paint(&vp.view, &geometry);
        let focus = vp.focus.clone();
        let probe = vp.probe.clone();
        let dragging = vp.drag;
        let over_handle = vp.over_handle;
        let ring = focus_visible(&focus, window);
        if changed && which == Which::Inline && box_size.0 > 0. {
            // `@box`: портрет берёт ширину кадра от окна вида.
            self.controller.set_stage_box(box_size.0, box_size.1);
        }

        // Картинка: уменьшение больше чем вдвое — с уменьшенной копии.
        let device = paint.scale * ratio;
        let wanted = if device > 0. && device < 1. {
            ((1. / device).log2().floor().max(0.)) as usize
        } else {
            0
        };
        let image = (0..=wanted.min(images.len() - 1))
            .rev()
            .find_map(|i| images[i].clone());
        let hot = over_handle || matches!(dragging, Some(Drag::Handle { .. }));
        let light = if hot {
            hsla(0., 0., 1., 1.)
        } else {
            hsla(0., 0., 1., 0.75)
        };
        let cursor = match dragging {
            Some(Drag::Handle { .. }) => CursorStyle::ResizeUpRightDownLeft,
            Some(Drag::Pan { .. }) => CursorStyle::ClosedHand,
            None if over_handle => CursorStyle::ResizeUpRightDownLeft,
            None if paint.zoomed => CursorStyle::OpenHand,
            None => CursorStyle::Arrow,
        };
        let me = cx.entity().downgrade();
        // Окно вида этого кадра; пока тянут — захват мыши окном (`setPointerCapture`).
        let layer = canvas(
            {
                let probe = probe.clone();
                move |bounds, window, _| {
                    let resized = probe.get().size != bounds.size;
                    // Место — для мыши (`local`), рисунок — по размеру.
                    probe.set(bounds);
                    if resized {
                        // Размер изменился после раскладки: ещё кадр с ним.
                        crate::motion::monitor_frame(window);
                    }
                }
            },
            move |bounds, (), window, _| {
                if let Some(h) = handle {
                    let rect = pv::handle_rect(&paint, &h, ratio);
                    paint_handle(
                        window,
                        bounds.origin,
                        window.scale_factor(),
                        rect,
                        ratio,
                        light,
                    );
                }
                if dragging.is_none() {
                    return;
                }
                window.set_window_cursor_style(cursor);
                let moving = me.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let released = event.pressed_button != Some(MouseButton::Left);
                    moving
                        .update(cx, |this, cx| {
                            if released {
                                this.viewport(which).drag = None;
                            } else {
                                this.drag_to(which, event.position, zoom);
                            }
                            cx.notify();
                        })
                        .ok();
                });
                let ending = me.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                        return;
                    }
                    ending
                        .update(cx, |this, cx| {
                            this.viewport(which).drag = None;
                            cx.notify();
                        })
                        .ok();
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let status = message.map(|text| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .gap(u(10.))
                .p(u(16.))
                .bg(hsla(213. / 360., 0.47, 0.037, 0.72))
                .text_color(theme.color("l-text-dim"))
                .text_px(13., 13. * 1.2)
                .text_center()
                .when(loading, |el| {
                    el.child(icon(
                        "fas fa-spinner fa-spin",
                        13.,
                        theme.color("l-text-dim"),
                    ))
                })
                .child(crate::text::balance(text))
        });

        let stage = div()
            .id(ElementId::Name(format!("gv-stage-{which:?}").into()))
            .relative()
            .min_h_0()
            .overflow_hidden()
            .bg(rgb(0x05090e))
            .cursor(cursor)
            .track_focus(&focus)
            .aria_label(label)
            .map(|el| {
                if props.fill {
                    el.flex_1().flex_basis(px(0.))
                } else {
                    el.w_full()
                        .aspect_ratio((frame_width / 900.) as f32)
                        .flex_shrink(1.)
                }
            })
            .when(!props.framed, |el| {
                el.rounded(theme.rem("l-radius-sm"))
                    .shadow(vec![crate::paint::ring(zoom, 1., theme.color("l-border"))])
            })
            .children(image.map(|image| {
                img(image)
                    .absolute()
                    .left(u(paint.left as f32))
                    .top(u(paint.top as f32))
                    .w(u(paint.width as f32))
                    .h(u(paint.height as f32))
                    .object_fit(ObjectFit::Fill)
            }))
            .child(layer)
            .children(status)
            .when(ring, |el| {
                let (radius, offset) = if props.framed {
                    (0., -2.)
                } else {
                    (theme.len("l-radius-sm"), 2.)
                };
                el.child(outline(
                    zoom,
                    radius,
                    offset,
                    2.,
                    theme.color("accent-focus"),
                ))
            })
            .on_scroll(cx.listener(move |this, event: &ScrollEvent, _, cx| {
                this.wheel(which, event, zoom);
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    let focus = this.viewport(which).focus.clone();
                    window.focus(&focus, cx);
                    this.pointer_down(which, event, zoom);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                if this.hover_at(which, event.position, zoom) {
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let vp = this.viewport(which);
                if !*hovered && vp.over_handle {
                    vp.over_handle = false;
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                if this.key(which, event) {
                    cx.stop_propagation();
                    cx.notify();
                }
            }));

        // Карточка превью уже 320 px (`@container gp (max-width: 320px)`): кнопки плотнее.
        let narrow = props.wrap && box_size.0 > 0. && box_size.0 + 2. <= 320.;
        let tipped = |id: &str, text: String, child: AnyElement| {
            div()
                .id(ElementId::Name(format!("gv-tip-{id}-{which:?}").into()))
                .flex_none()
                .tip(text)
                .child(child)
                .into_any_element()
        };
        let id = |name: &str| ElementId::Name(format!("gv-{name}-{which:?}").into());
        let icon_btn = |name: &str, glyph: &str, disabled: bool| {
            button(id(name), BtnLook::Gx)
                .w(u(30.))
                .px_0()
                .disabled(disabled)
                .child(fa_glyph(glyph, 11.))
        };
        let listener = |f: fn(&mut View, &Geometry)| {
            cx.listener(
                move |this: &mut GhostingPreview, _: &gpui::ClickEvent, _, cx| {
                    let vp = this.viewport(which);
                    let mut view = vp.view;
                    f(&mut view, &vp.geometry);
                    vp.view = view;
                    cx.notify();
                },
            )
        };
        let zoom_out_btn = icon_btn("out", "fas fa-minus", !paint.can_zoom_out)
            .label(zoom_out.clone())
            .on_click(listener(|v, g| *v = pv::zoom_by(v, g, 1. / pv::STEP, None)));
        let zoom_in_btn = icon_btn("in", "fas fa-plus", !paint.can_zoom_in)
            .label(zoom_in.clone())
            .on_click(listener(|v, g| *v = pv::zoom_by(v, g, pv::STEP, None)));
        let fit_btn = button(id("fit"), BtnLook::Gx)
            .pressed(paint.at_fit)
            .when(narrow, |el| el.px(u(8.)))
            .child(fit)
            .on_click(listener(|v, g| *v = pv::initial(g)));
        let actual_btn = button(id("actual"), BtnLook::Gx)
            .pressed(paint.actual)
            .when(narrow, |el| el.px(u(8.)))
            .child("100%")
            .on_click(listener(|v, g| *v = pv::actual_size(v, g, None)));
        let level = div()
            .min_w(u(if narrow { 40. } else { 46. }))
            .text_px(12., 12. * 1.2)
            .tnum()
            .text_center()
            .text_color(theme.color("l-text-dim"))
            .child(format!("{}%", paint.percent));
        let motion_btn = motion.then(|| {
            button(id("motion"), BtnLook::Gx)
                .pressed(animating)
                .when(narrow, |el| el.px(u(8.)))
                .child(fa_glyph(
                    if animating {
                        "fas fa-pause"
                    } else {
                        "fas fa-play"
                    },
                    11.,
                ))
                .child(motion_label)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.controller.toggle_motion();
                    cx.notify();
                }))
        });
        let expand_btn = props.expandable.then(|| {
            icon_btn("expand", "fas fa-expand", false)
                .label(expand.clone())
                .on_click(cx.listener(|this, _, window, cx| this.open_expanded(window, cx)))
        });
        let bar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_x(u(if narrow { 4. } else { 6. }))
            .min_h(u(30.))
            .when(props.wrap, |el| el.flex_wrap().gap_y(u(6.)))
            .when(props.framed, |el| {
                el.py(u(7.))
                    .px(u(if narrow { 6. } else { 8. }))
                    .border_b_1()
                    .border_color(theme.color("l-border"))
            })
            .child(tipped(
                "out",
                format!("{zoom_out} (\u{2212})"),
                zoom_out_btn.into_any_element(),
            ))
            .child(level)
            .child(tipped(
                "in",
                format!("{zoom_in} (+)"),
                zoom_in_btn.into_any_element(),
            ))
            .child(tipped(
                "fit",
                format!("{fit_title} (0)"),
                fit_btn.into_any_element(),
            ))
            .child(tipped(
                "actual",
                format!("{actual} (1)"),
                actual_btn.into_any_element(),
            ))
            .child(div().flex_1())
            .children(motion_btn)
            .children(expand_btn.map(|b| {
                crate::snap::target(
                    "gv-expand",
                    tipped("expand", expand.clone(), b.into_any_element()),
                )
            }));

        div()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .text_color(theme.color("l-text"))
            .when(!props.framed, |el| el.gap(u(8.)))
            .map(|el| {
                if props.fill {
                    el.flex_1().flex_basis(px(0.))
                } else {
                    el.flex_shrink(1.)
                }
            })
            .map(|el| {
                if props.framed {
                    el.child(bar).child(stage)
                } else {
                    el.child(stage).child(bar)
                }
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::handle_quads;

    #[test]
    fn handle_frame_is_dark_with_light_dashes_four_of_every_nine_px_dark() {
        // 90 × 45 физ. px: периметр 270 = 30 периодов по 9, светлого — по 5 в каждом.
        let (dark, light) = handle_quads((10.5, 20.5, 90., 45.), 1.);
        assert_eq!(dark.len(), 4);
        assert_eq!(dark[0], [10., 20., 101., 21.]);
        let length: f64 = light.iter().map(|q| (q[2] - q[0]).max(q[3] - q[1])).sum();
        assert!((length - 150.).abs() < 1e-9, "{length}");
        // Первый штрих — с 4-го px верхней стороны, 1 px толщиной.
        assert_eq!(light[0], [14.5, 20., 19.5, 21.]);
        // ×1,25: период растёт с масштабом.
        let (_, light) = handle_quads((0.5, 0.5, 90., 45.), 1.25);
        assert_eq!(light[0][0], 5.5);
        assert!(handle_quads((0.5, 0.5, 0., 10.), 1.).0.is_empty());
    }
}
