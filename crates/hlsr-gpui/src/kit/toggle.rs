//! Переключатель `.gx-switch` (settings-fields.scss): дорожка 42 × 24, поле 3, бегунок 18;
//! включённый — дорожка `--accent`, бегунок `--l-switch-thumb-on` сдвинут на 18 px; дорожка и
//! сдвиг идут `--l-med`. Пробел и мышь переключают, Enter — нет (`<input type="checkbox">`).
//! `disabled` — без событий и бледнее (.45, как `.gx-row.is-off`).
//!
//! [`Toggle::spring`] — `.settings-switch` окна настроек игры (GameSettingsModal.vue): бегунок
//! едет пружиной (`transform var(--l-dur) var(--l-ease-spring)`) и, пока переключатель нажат,
//! вытягивается до 22 px (`width var(--l-fast)`; у включённого — влево, `translateX(14px)`).

use super::button::{
    Handler, Preview, child_id, enter_key, focus_handle, focus_visible, outline, white,
};
use crate::motion::{Timing, active, mix, registry, tween};
use crate::scale::{u, zoom};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ElementId, IntoElement, RenderOnce, Role, SharedString, Styled, Toggled, Window, div,
    prelude::*,
};
use std::rc::Rc;

#[derive(IntoElement)]
pub struct Toggle {
    id: ElementId,
    checked: bool,
    disabled: bool,
    spring: bool,
    label: Option<SharedString>,
    preview: Preview,
    on_change: Option<Handler<bool>>,
}

pub fn toggle(id: impl Into<ElementId>, checked: bool) -> Toggle {
    Toggle {
        id: id.into(),
        checked,
        disabled: false,
        spring: false,
        label: None,
        preview: Preview::default(),
        on_change: None,
    }
}

impl Toggle {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// `.settings-switch` настроек игры: пружина и вытягивание при нажатии (см. модуль).
    pub fn spring(mut self) -> Self {
        self.spring = true;
        self
    }

    /// `aria-label` (подпись строки).
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn preview(mut self, preview: Preview) -> Self {
        self.preview = preview;
        self
    }

    /// Новое значение после щелчка или пробела.
    pub fn on_change(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Toggle {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let accent = theme.color("accent");
        let thumb = if self.checked {
            theme.color("l-switch-thumb-on")
        } else {
            white(1.)
        };
        let focus_color = theme.color("accent-focus");
        let zoom = zoom(window);
        let focus = focus_handle(&self.id, window, cx);
        let ring_on = self.preview.focus || focus_visible(&focus, window);
        let Toggle {
            id,
            checked,
            disabled,
            spring,
            label,
            on_change,
            ..
        } = self;
        let inner = id.clone();
        let (press_id, x_id, w_id) = (
            child_id(&id, "press"),
            child_id(&id, "thumb-x"),
            child_id(&id, "thumb-w"),
        );
        let tokens = &registry().tokens;
        let spring_timing = Timing::new(tokens.l_dur, tokens.l_ease_spring);
        let meta = crate::live::Meta::new("toggle", id.clone())
            .label(label.clone())
            .disabled(disabled)
            .focused(focus.is_focused(window))
            .checked(checked);
        let body = move |held: bool| {
            tween(id, f32::from(u8::from(checked)), Timing::med(), move |t| {
                let knob = move |x: f32, w: f32| {
                    div()
                        .debug_selector(|| "toggle-thumb".into())
                        .ml(u(x))
                        .w(u(w))
                        .h(u(18.))
                        .rounded_full()
                        .bg(thumb)
                };
                let knob = if spring {
                    let x = match (checked, held) {
                        (true, true) => 14.,
                        (true, false) => 18.,
                        (false, _) => 0.,
                    };
                    tween(x_id, x, spring_timing, move |x| {
                        tween(w_id, f32::from(u8::from(held)), Timing::fast(), move |w| {
                            knob(x, 18. + 4. * w)
                        })
                    })
                    .into_any_element()
                } else {
                    knob(18. * t, 18.).into_any_element()
                };
                div()
                    .id(inner)
                    .role(Role::Switch)
                    .aria_toggled(if checked {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .when_some(label, |el, label| el.aria_label(label))
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .w(u(42.))
                    .h(u(24.))
                    .p(u(3.))
                    .rounded(u(12.))
                    .bg(mix(white(0.12), accent, t))
                    .child(knob)
                    .when(disabled, |el| el.opacity(0.45))
                    .when(!disabled, |el| {
                        el.track_focus(&focus).cursor_pointer().when_some(
                            on_change,
                            |el, handler| {
                                el.on_click(move |event, window, cx| {
                                    if !enter_key(event) {
                                        handler(&!checked, window, cx);
                                    }
                                })
                            },
                        )
                    })
                    .when(ring_on, |el| {
                        el.child(outline(zoom, 12., 2., 2., focus_color))
                    })
            })
        };
        // `:active` нужно только бегунку `.settings-switch`.
        let element = if spring {
            active(press_id, !disabled, body).into_any_element()
        } else {
            body(false).into_any_element()
        };
        crate::live::mark(meta, element)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scale::zpx;
    use gpui::{Context, Modifiers, MouseButton, Render, TestAppContext, VisualTestContext};
    use std::time::Duration;

    struct Switch {
        on: bool,
    }

    impl Render for Switch {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .p(u(20.))
                .child(toggle("t", self.on).spring().on_change(cx.listener(
                    |this, on: &bool, _, cx| {
                        this.on = *on;
                        cx.notify();
                    },
                )))
        }
    }

    /// Бегунок, когда пружина `--l-dur` и вытягивание `--l-fast` дошли до конца.
    fn thumb(cx: &mut VisualTestContext) -> gpui::Bounds<gpui::Pixels> {
        let ms = registry().tokens.l_dur as u64 + 20;
        cx.executor().advance_clock(Duration::from_millis(ms));
        for _ in 0..3 {
            cx.update(|window, cx| window.simulate_next_frame(cx));
            cx.run_until_parked();
        }
        cx.debug_bounds("toggle-thumb").unwrap()
    }

    #[gpui::test]
    fn spring_thumb_stretches_while_held(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (_, cx) = cx.add_window_view(|_, _| Switch { on: false });
        cx.run_until_parked();
        let off = thumb(cx);
        assert_eq!(off.size.width, zpx(1., 18.));
        let at = off.center();
        // Выключенный тянется вправо до 22 px, левый край на месте.
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        let held = thumb(cx);
        assert_eq!(
            (held.origin.x, held.size.width),
            (off.origin.x, zpx(1., 22.))
        );
        // Отпустили — включился: 18 px, сдвиг 18.
        cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        let on = thumb(cx);
        assert_eq!(
            (on.origin.x, on.size.width),
            (off.origin.x + zpx(1., 18.), zpx(1., 18.))
        );
        // Включённый тянется влево (`translateX(14px)`): правый край на месте.
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        let held = thumb(cx);
        assert_eq!((held.right(), held.size.width), (on.right(), zpx(1., 22.)));
        cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        assert_eq!(thumb(cx), off);
    }
}
