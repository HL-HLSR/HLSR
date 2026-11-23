//! Разделитель панелей `PaneSplitter.vue` 0.4.9: полоса между двумя панелями. Тянут мышью
//! (мышь захвачена до отпускания), стрелками по 16 px, Home/End — к краям, двойной щелчок или
//! Enter — как было (размер по умолчанию). Пределы, шаг и запоминание — `pane_size`.
//!
//! Размер (px CSS) применяет место: [`PaneSplitter::on_input`] — `update:modelValue` (`None` —
//! размер по умолчанию), [`PaneSplitter::on_save`] — что записать в prefs `pane`
//! (`pane_size::write_size`; `None` — забыть). Панели меряются рамками прошлого кадра
//! ([`anchor_probe`](crate::layer::anchor_probe) внутри панели и соседней после полосы).
//! После отпускания или клавиши размер закрепляется кадром позже — тот, что вышел на деле: упор
//! в тесном окне не запоминает размер меньше обычного (`settle` 0.4.9).
//!
//! Вид: полоса 12 px (между колонками) или 8 px (между верхом и низом); ручка 4 × 36
//! `--l-border-strong` видна всегда; линия 2 px во всю длину — при наведении и перетаскивании
//! (`--accent`, .6), с клавиатуры — `--accent-focus`.

use super::button::{Handler, Preview, child_id, focus_visible};
use crate::layer::TipExt;
use crate::motion::{Timing, hover, mix};
use crate::scale::{u, zoom};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Bounds, CursorStyle, DispatchPhase, ElementId, Entity, EntityId, FocusHandle, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, RenderOnce, Role,
    SharedString, Styled, Window, canvas, div, prelude::*, relative,
};
use hlsr_ui::controllers::pane_size::{clamp_size, key_size};
use std::cell::Cell;
use std::rc::Rc;

/// `aria-orientation`: `Vertical` — полоса между колонками (меняется ширина), `Horizontal` —
/// между верхом и низом (высота).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Vertical,
    Horizontal,
}

type Probe = Rc<Cell<Bounds<Pixels>>>;

#[derive(IntoElement)]
pub struct PaneSplitter {
    id: ElementId,
    orientation: Orientation,
    min: f64,
    rest: f64,
    value: Option<f64>,
    pane: Probe,
    next: Probe,
    label: Option<SharedString>,
    hint: Option<SharedString>,
    preview: Preview,
    on_input: Option<Handler<Option<f64>>>,
    on_save: Option<Handler<Option<f64>>>,
}

/// Полоса с пределом `min` у панели; `value` — размер места (`None` — по умолчанию).
pub fn pane_splitter(
    id: impl Into<ElementId>,
    orientation: Orientation,
    min: f64,
    value: Option<f64>,
) -> PaneSplitter {
    PaneSplitter {
        id: id.into(),
        orientation,
        min,
        rest: 0.,
        value,
        pane: Rc::default(),
        next: Rc::default(),
        label: None,
        hint: None,
        preview: Preview::default(),
        on_input: None,
        on_save: None,
    }
}

impl PaneSplitter {
    /// Рамки панели, чей размер меняется (`target`, обычно перед полосой), и соседней.
    pub fn panes(mut self, pane: Probe, next: Probe) -> Self {
        self.pane = pane;
        self.next = next;
        self
    }

    /// Сколько оставить соседней панели (её минимум в CSS места).
    pub fn rest(mut self, rest: f64) -> Self {
        self.rest = rest;
        self
    }

    /// `aria-label` (`#L_SPLIT_*`).
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// `title` (`#L_SPLIT_HINT`).
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn preview(mut self, preview: Preview) -> Self {
        self.preview = preview;
        self
    }

    pub fn on_input(
        mut self,
        handler: impl Fn(&Option<f64>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_input = Some(Rc::new(handler));
        self
    }

    pub fn on_save(
        mut self,
        handler: impl Fn(&Option<f64>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_save = Some(Rc::new(handler));
        self
    }
}

#[derive(Clone, Copy)]
struct Drag {
    /// Где нажали (px CSS по оси).
    from: f64,
    size: f64,
    /// Предел на время перетаскивания (мерка при нажатии, как `this.max` 0.4.9).
    max: f64,
    value: Option<f64>,
    moved: bool,
}

/// Размер до перетаскивания или клавиши: с ним сравнивается то, что вышло.
#[derive(Clone, Copy)]
struct Start {
    size: f64,
    value: Option<f64>,
}

struct State {
    focus: FocusHandle,
    drag: Option<Drag>,
    settle: Option<Start>,
    /// `aria-valuenow`, `aria-valuemax` по прошлому кадру.
    now: f64,
    max: f64,
}

/// Что нужно обработчикам: ось, пределы, рамки панелей, зум.
#[derive(Clone)]
struct Geometry {
    vertical: bool,
    min: f64,
    rest: f64,
    pane: Probe,
    next: Probe,
    zoom: f32,
}

impl Geometry {
    fn side(&self, bounds: Bounds<Pixels>) -> f64 {
        let side = if self.vertical {
            bounds.size.width
        } else {
            bounds.size.height
        };
        f64::from(f32::from(side) / self.zoom)
    }

    fn coord(&self, at: Point<Pixels>) -> f64 {
        f64::from(f32::from(if self.vertical { at.x } else { at.y }) / self.zoom)
    }

    /// `measure`: размер панели и сколько она может вырасти — столько, сколько отдаст соседняя
    /// до своего минимума (уже меньше него — расти некуда, но и сама не сожмётся). `None` —
    /// панелей ещё нет на экране.
    fn measure(&self) -> Option<(f64, f64)> {
        let (pane, next) = (self.pane.get(), self.next.get());
        if pane == Bounds::default() || next == Bounds::default() {
            return None;
        }
        let size = self.side(pane);
        let now = size.round();
        let grow = (size + self.side(next) - self.rest).round();
        Some((now, self.min.max(now).max(grow)))
    }
}

fn emit(handler: &Option<Handler<Option<f64>>>, value: Option<f64>, w: &mut Window, cx: &mut App) {
    if let Some(handler) = handler {
        handler(&value, w, cx);
    }
}

/// Отпустили: размер закрепится кадром позже, если тянули (щелчок без движения и первый
/// щелчок двойного — нет).
fn release(state: &Entity<State>, owner: EntityId, cx: &mut App) {
    let drag = state.update(cx, |s, _| s.drag.take());
    if let Some(drag) = drag {
        if drag.moved {
            state.update(cx, |s, _| {
                s.settle = Some(Start {
                    size: drag.size,
                    value: drag.value,
                })
            });
        }
        cx.notify(owner);
    }
}

/// `reset`: как было — размер по умолчанию, запомненный забыт.
fn reset(
    state: &Entity<State>,
    input: &Option<Handler<Option<f64>>>,
    save: &Option<Handler<Option<f64>>>,
    owner: EntityId,
    window: &mut Window,
    cx: &mut App,
) {
    state.update(cx, |s, _| {
        s.drag = None;
        s.settle = None;
    });
    emit(input, None, window, cx);
    emit(save, None, window, cx);
    cx.notify(owner);
}

/// Имя клавиши `KeyboardEvent.key` для `pane_size::key_size`.
fn js_key(key: &str) -> Option<&'static str> {
    Some(match key {
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        "home" => "Home",
        "end" => "End",
        _ => return None,
    })
}

impl RenderOnce for PaneSplitter {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let PaneSplitter {
            id,
            orientation,
            min,
            rest,
            value,
            pane,
            next,
            label,
            hint,
            preview,
            on_input,
            on_save,
        } = self;
        let vertical = orientation == Orientation::Vertical;
        let geometry = Geometry {
            vertical,
            min,
            rest,
            pane,
            next,
            zoom: zoom(window),
        };
        let owner = window.current_view();
        let state = window.use_keyed_state(child_id(&id, "split"), cx, |_, cx| State {
            focus: cx.focus_handle().tab_stop(true),
            drag: None,
            settle: None,
            now: 0.,
            max: 0.,
        });
        let (focus, dragging, now, max) = {
            let s = state.read(cx);
            (s.focus.clone(), s.drag.is_some(), s.now, s.max)
        };
        let theme = cx.global::<Theme>();
        let [accent, focus_color, grip] =
            ["accent", "accent-focus", "l-border-strong"].map(|key| theme.color(key));
        let keyboard = preview.focus || focus_visible(&focus, window);
        let cursor = if vertical {
            CursorStyle::ResizeColumn
        } else {
            CursorStyle::ResizeRow
        };

        // Каждый кадр (в отрисовке: рамки обеих панелей уже этого кадра): мерка для aria,
        // закрепить размер после отпускания/клавиши; пока тянут — мышь окна.
        let probe = {
            let (state, geometry) = (state.clone(), geometry.clone());
            let (input, save) = (on_input.clone(), on_save.clone());
            let move_input = on_input.clone();
            canvas(
                |_, _, _| {},
                move |_, (), window, cx| {
                    if let Some((now, max)) = geometry.measure() {
                        let start = state.update(cx, |s, _| {
                            s.now = now;
                            s.max = max;
                            s.settle.take()
                        });
                        // Не изменился — остаётся прежний: упор в тесном окне не закрепит
                        // размер меньше обычного.
                        if let Some(start) = start {
                            let value = if now == start.size {
                                start.value
                            } else {
                                Some(now)
                            };
                            window.defer(cx, move |window, cx| {
                                emit(&input, value, window, cx);
                                if value != start.value {
                                    emit(&save, value, window, cx);
                                }
                            });
                        }
                    }
                    if !dragging {
                        return;
                    }
                    window.set_window_cursor_style(cursor);
                    let input = move_input;
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        let Some(drag) = state.read(cx).drag else {
                            return;
                        };
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        // Кнопку отпустили там, где окно не слышало.
                        if event.pressed_button != Some(MouseButton::Left) {
                            release(&state, owner, cx);
                            return;
                        }
                        let delta = geometry.coord(event.position) - drag.from;
                        if delta == 0. && !drag.moved {
                            return;
                        }
                        let next = clamp_size(drag.size + delta, geometry.min, drag.max);
                        state.update(cx, |s, _| {
                            s.now = next;
                            if let Some(drag) = &mut s.drag {
                                drag.moved = true;
                            }
                        });
                        emit(&input, Some(next), window, cx);
                        cx.notify(owner);
                    });
                },
            )
            .absolute()
            .size_0()
        };

        let inner = id.clone();
        let (down, up, out, keys) = (state.clone(), state.clone(), state.clone(), state);
        let (down_input, down_save) = (on_input.clone(), on_save.clone());
        let (key_geometry, down_geometry) = (geometry.clone(), geometry);
        hover(id, Timing::fast(), move |t| {
            let lit = if dragging || preview.hover { 1. } else { t };
            let (line, line_opacity, grip) = if keyboard {
                (focus_color, 1., focus_color)
            } else {
                (accent, 0.6 * lit, mix(grip, accent, lit))
            };
            let bar = div()
                .absolute()
                .rounded(u(1.))
                .bg(line)
                .opacity(line_opacity);
            let bar = if vertical {
                bar.top_0()
                    .bottom_0()
                    .left(relative(0.5))
                    .ml(u(-1.))
                    .w(u(2.))
            } else {
                bar.left_0()
                    .right_0()
                    .top(relative(0.5))
                    .mt(u(-1.))
                    .h(u(2.))
            };
            let (gw, gh) = if vertical { (4., 36.) } else { (36., 4.) };
            div()
                .id(inner)
                .role(Role::Splitter)
                .aria_numeric_value(now)
                .aria_min_numeric_value(min)
                .aria_max_numeric_value(max)
                .when_some(label, |el, label| el.aria_label(label))
                .when_some(hint, |el, hint| el.tip(hint))
                .track_focus(&focus)
                .relative()
                .flex_none()
                .self_stretch()
                .flex()
                .items_center()
                .justify_center()
                .map(|el| if vertical { el.w(u(12.)) } else { el.h(u(8.)) })
                .cursor(cursor)
                .child(bar)
                .child(div().relative().w(u(gw)).h(u(gh)).rounded(u(2.)).bg(grip))
                .child(probe)
                .on_mouse_down(
                    MouseButton::Left,
                    move |event: &MouseDownEvent, window, cx| {
                        if event.click_count >= 2 {
                            reset(&down, &down_input, &down_save, owner, window, cx);
                            return;
                        }
                        let Some((now, max)) = down_geometry.measure() else {
                            return;
                        };
                        let from = down_geometry.coord(event.position);
                        down.update(cx, |s, _| {
                            s.drag = Some(Drag {
                                from,
                                size: now,
                                max,
                                value,
                                moved: false,
                            })
                        });
                        cx.notify(owner);
                    },
                )
                .on_mouse_up(MouseButton::Left, move |_, _window, cx| {
                    release(&up, owner, cx)
                })
                .on_mouse_up_out(MouseButton::Left, move |_, _window, cx| {
                    release(&out, owner, cx)
                })
                .on_key_down(move |event: &KeyDownEvent, window, cx| {
                    let key = event.keystroke.key.as_str();
                    if key == "enter" {
                        cx.stop_propagation();
                        reset(&keys, &on_input, &on_save, owner, window, cx);
                        return;
                    }
                    let Some(name) = js_key(key) else { return };
                    let Some((now, max)) = key_geometry.measure() else {
                        return;
                    };
                    let Some(next) = key_size(now, name, key_geometry.min, max) else {
                        return;
                    };
                    cx.stop_propagation();
                    keys.update(cx, |s, _| s.settle = Some(Start { size: now, value }));
                    emit(&on_input, Some(next), window, cx);
                    cx.notify(owner);
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::anchor_probe;
    use crate::scale::zpx;
    use gpui::{Context, Modifiers, Render, TestAppContext, VisualTestContext, point};

    /// Колонки: левая (её ширина — `width`, по умолчанию 200) и правая не уже 300; окно 800.
    struct Form {
        width: Option<f64>,
        inputs: Vec<Option<f64>>,
        saved: Vec<Option<f64>>,
        pane: Probe,
        next: Probe,
    }

    impl Render for Form {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let width = self.width.unwrap_or(200.) as f32;
            div()
                .flex()
                .w(u(800.))
                .h(u(300.))
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .w(u(width))
                        .child(anchor_probe(self.pane.clone())),
                )
                .child(
                    pane_splitter("split", Orientation::Vertical, 130., self.width)
                        .panes(self.pane.clone(), self.next.clone())
                        .rest(300.)
                        .on_input(cx.listener(|this, value: &Option<f64>, _, cx| {
                            this.width = *value;
                            this.inputs.push(*value);
                            cx.notify();
                        }))
                        .on_save(cx.listener(|this, value: &Option<f64>, _, _| {
                            this.saved.push(*value);
                        })),
                )
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_w(u(300.))
                        .child(anchor_probe(self.next.clone())),
                )
        }
    }

    fn open(cx: &mut TestAppContext) -> (Entity<Form>, &mut VisualTestContext) {
        cx.update(crate::init);
        let (form, cx) = cx.add_window_view(|_, _| Form {
            width: None,
            inputs: Vec::new(),
            saved: Vec::new(),
            pane: Rc::default(),
            next: Rc::default(),
        });
        cx.run_until_parked();
        (form, cx)
    }

    fn frame(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }

    fn at(x: f32) -> Point<Pixels> {
        point(zpx(1., x), zpx(1., 100.))
    }

    #[gpui::test]
    fn drag_follows_the_mouse_in_limits_and_saves_on_release(cx: &mut TestAppContext) {
        let (form, cx) = open(cx);
        let none = Modifiers::default();
        // Полоса — 200…212; соседняя отдаёт до 300: максимум 200 + 588 − 300 = 488.
        cx.simulate_mouse_down(at(206.), MouseButton::Left, none);
        frame(cx);
        cx.simulate_mouse_move(at(250.), MouseButton::Left, none);
        cx.simulate_mouse_move(at(900.), MouseButton::Left, none);
        cx.simulate_mouse_move(at(0.), MouseButton::Left, none);
        cx.simulate_mouse_move(at(226.), MouseButton::Left, none);
        cx.simulate_mouse_up(at(226.), MouseButton::Left, none);
        frame(cx);
        form.update(cx, |form, _| {
            assert_eq!(
                form.inputs,
                [Some(244.), Some(488.), Some(130.), Some(220.), Some(220.)]
            );
            assert_eq!(form.saved, [Some(220.)]);
        });
        // Двойной щелчок — как было, запомненный забыт.
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at(226.),
            modifiers: none,
            click_count: 2,
            first_mouse: false,
        });
        cx.simulate_mouse_up(at(226.), MouseButton::Left, none);
        frame(cx);
        form.update(cx, |form, _| {
            assert_eq!(form.width, None);
            assert_eq!(form.saved, [Some(220.), None]);
        });
    }

    #[gpui::test]
    fn keys_step_by_16_home_end_and_enter_resets(cx: &mut TestAppContext) {
        let (form, cx) = open(cx);
        cx.update(|window, cx| window.focus_next(cx));
        frame(cx);
        cx.simulate_keystrokes("right");
        frame(cx);
        cx.simulate_keystrokes("left");
        frame(cx);
        cx.simulate_keystrokes("left");
        frame(cx);
        cx.simulate_keystrokes("end");
        frame(cx);
        cx.simulate_keystrokes("home");
        frame(cx);
        cx.simulate_keystrokes("enter");
        frame(cx);
        form.update(cx, |form, _| {
            // Каждая клавиша — input с новым размером и кадром позже — закреплённый.
            assert_eq!(
                form.inputs,
                [
                    Some(216.),
                    Some(216.),
                    Some(200.),
                    Some(200.),
                    Some(184.),
                    Some(184.),
                    Some(488.),
                    Some(488.),
                    Some(130.),
                    Some(130.),
                    None
                ]
            );
            assert_eq!(
                form.saved,
                [
                    Some(216.),
                    Some(200.),
                    Some(184.),
                    Some(488.),
                    Some(130.),
                    None
                ]
            );
        });
    }
}
