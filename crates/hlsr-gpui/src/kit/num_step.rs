//! Стрелки числа `GxNumStep` 0.4.9 вместо системных: `.gx-num` — поле числа ([`TextInput`] с
//! [`Kind::Number`](super::Kind)) и справа колонка 18 px из двух кнопок (`fa-chevron-up/down`
//! 8 px; наведение .14, нажатие `--accent-soft` и `--accent`). Видна, когда `.gx-num` под
//! курсором или поле в фокусе (`--l-fast`); у выключенного поля её нет. Кнопки не берут фокус и
//! не встают в Tab (`@mousedown.prevent`, `tabindex="-1"`): шаг — `stepUp()`/`stepDown()` поля
//! (его `StepUp`/`StepDown`: `input` и `change`). Единица (`.gx-num__unit`) — перед кнопками.
//!
//! Поле строится с оформлением [`field`] (или [`field_unit`] с единицей).

use super::button::{child_id, white};
use super::input::{FieldStyle, StepDown, StepUp, TextInput};
use crate::icons::fa_glyph;
use crate::motion::{Timing, hover};
use crate::scale::u;
use crate::text::{RUBIK, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Action, App, ElementId, Entity, Focusable, Hsla, IntoElement, MouseButton, RenderOnce,
    SharedString, Styled, Window, div, hsla, prelude::*,
};

/// `.gx-num.has-step .gx-input--num`: слева 8, справа 28 — место под кнопки.
pub fn field(theme: &Theme) -> FieldStyle {
    FieldStyle {
        pad_x: 8.,
        pad_right: Some(28.),
        ..FieldStyle::gx(theme)
    }
}

/// `.gx-num.has-step.has-unit`: справа 46.
pub fn field_unit(theme: &Theme) -> FieldStyle {
    FieldStyle {
        pad_x: 8.,
        pad_right: Some(46.),
        ..FieldStyle::gx(theme)
    }
}

#[derive(IntoElement)]
pub struct NumStep {
    id: ElementId,
    input: Entity<TextInput>,
    unit: Option<SharedString>,
    disabled: bool,
    shown: bool,
}

/// `.gx-num` вокруг поля `input`; ширину задаёт место (`.gx-input--num` — 92 px).
pub fn num_step(id: impl Into<ElementId>, input: Entity<TextInput>) -> NumStep {
    NumStep {
        id: id.into(),
        input,
        unit: None,
        disabled: false,
        shown: false,
    }
}

impl NumStep {
    /// `.gx-num__unit` (`ms`, `%`…).
    pub fn unit(mut self, unit: impl Into<SharedString>) -> Self {
        self.unit = Some(unit.into());
        self
    }

    /// Поле выключено: кнопок нет (`:has(input:disabled)`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Кнопки видны без наведения (витрина).
    pub fn preview_shown(mut self, shown: bool) -> Self {
        self.shown = shown;
        self
    }
}

/// Кнопка шага: шаг полю, фокус остаётся где был.
fn step_button(
    name: &'static str,
    icon: &'static str,
    action: impl Action + Clone,
    input: Entity<TextInput>,
    [soft, accent, dim, text]: [Hsla; 4],
) -> impl IntoElement {
    div()
        .id(name)
        .debug_selector(move || format!("num-step/{name}"))
        .flex()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .bg(white(0.06))
        .text_color(dim)
        .cursor_pointer()
        .hover(move |s| s.bg(white(0.14)).text_color(text))
        .active(move |s| s.bg(soft).text_color(accent))
        .child(fa_glyph(icon, 8.))
        // `@mousedown.prevent`: поле под кнопкой не получает нажатие, фокус не уходит.
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .button_text()
        .on_click(move |_, window, cx| {
            input.focus_handle(cx).dispatch_action(&action, window, cx);
        })
}

impl RenderOnce for NumStep {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let colors = ["accent-soft", "accent", "l-text-dim", "l-text"].map(|key| theme.color(key));
        let faint = theme.color("l-text-faint");
        let focused = self.input.focus_handle(cx).contains_focused(window, cx);
        let NumStep {
            id,
            input,
            unit,
            disabled,
            shown,
        } = self;
        let inner = child_id(&id, "num");
        hover(id, Timing::fast(), move |t| {
            let opacity = if shown || focused { 1. } else { t };
            let up = step_button("up", "fas fa-chevron-up", StepUp, input.clone(), colors);
            let down = step_button(
                "down",
                "fas fa-chevron-down",
                StepDown,
                input.clone(),
                colors,
            );
            div()
                .id(inner)
                .relative()
                .flex()
                .min_w(u(64.))
                .child(input)
                .when_some(unit, |el, unit| {
                    el.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right(u(if disabled { 9. } else { 28. }))
                            .flex()
                            .items_center()
                            .font_family(RUBIK)
                            .text_size(u(13.))
                            .text_color(faint)
                            .child(unit),
                    )
                })
                .when(!disabled, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(u(4.))
                            .right(u(4.))
                            .bottom(u(4.))
                            .w(u(18.))
                            .flex()
                            .flex_col()
                            .rounded(u(6.))
                            .overflow_hidden()
                            .opacity(opacity)
                            .child(up)
                            .child(div().h(gpui::px(1.)).bg(hsla(0., 0., 0., 0.25)))
                            .child(down),
                    )
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{FieldEvent, Kind};
    use gpui::{Context, Modifiers, Render, TestAppContext};

    struct Form {
        input: Entity<TextInput>,
        events: Vec<FieldEvent>,
    }

    impl Render for Form {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .p(u(20.))
                .w(u(132.))
                .child(num_step("n", self.input.clone()))
        }
    }

    #[gpui::test]
    fn buttons_step_without_taking_focus(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (form, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| {
                TextInput::new(window, cx)
                    .kind(Kind::Number {
                        min: "0".into(),
                        max: "1".into(),
                        step: "0.25".into(),
                    })
                    .style(field)
                    .with_text("0.5")
            });
            cx.subscribe(&input, |form: &mut Form, _, event: &FieldEvent, _| {
                form.events.push(event.clone())
            })
            .detach();
            Form {
                input,
                events: Vec::new(),
            }
        });
        cx.run_until_parked();
        let up = cx.debug_bounds("num-step/up").unwrap();
        let down = cx.debug_bounds("num-step/down").unwrap();
        cx.simulate_click(up.center(), Modifiers::default());
        cx.simulate_click(up.center(), Modifiers::default());
        // У max шага нет: события тоже.
        cx.simulate_click(up.center(), Modifiers::default());
        cx.simulate_click(down.center(), Modifiers::default());
        form.update_in(cx, |form, window, cx| {
            assert_eq!(form.input.read(cx).text(), "0.75");
            assert!(!form.input.focus_handle(cx).is_focused(window));
            let changes = form
                .events
                .iter()
                .filter(|event| **event == FieldEvent::Change)
                .count();
            assert_eq!(changes, 3, "{:?}", form.events);
        });
    }
}
