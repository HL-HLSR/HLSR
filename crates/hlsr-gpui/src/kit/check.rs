//! Флажок `<input type="checkbox">` 0.4.9 с промежуточным состоянием (`indeterminate`,
//! RepairModal). Два вида:
//!
//! - [`CheckLook::Native`] — стандартный флажок Chromium в тёмной схеме с `accent-color`
//!   (RepairModal, GameDemos, DemoExportModal…; значения сняты эталоном в slint, `toggles.slint`):
//!   13 × 13 (18 у `.pers__option`), скругление 2 (3), рамка #858585 на #3b3b3b; включённый —
//!   заливка акцентом (при наведении на 22 % светлее) и тёмная галочка; выключенный — #757575 /
//!   рамка #626262.
//! - [`CheckLook::Box`] — свой флажок (`.welcome__box`): 18 × 18, скругление 5, контур inset
//!   1,5 px `--l-border-strong`; включённый — `--accent` и `fa-check` 10 px, переход `--l-fast`.
//!
//! [`Check`] — строка `<label>`: флажок и дети (подпись) нажимаются вместе; оформление строки
//! (`gap`, кегль, цвет) — у места через `Styled`. Пробел и мышь переключают, Enter — нет.

use super::button::{Handler, Preview, enter_key, focus_handle, focus_visible, outline};
use crate::icons::fa_glyph;
use crate::motion::{Timing, mix, tween};
use crate::paint::ring;
use crate::scale::{u, zoom};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Div, ElementId, Hsla, IntoElement, ParentElement, RenderOnce, Role,
    SharedString, StyleRefinement, Styled, Toggled, Window, div, prelude::*, rgb, svg, white,
};
use std::rc::Rc;

/// Галочка Chromium: два звена обводкой, толщина ≈ 1/7 стороны (сторона 13).
const MARK: &[u8] = b"<svg xmlns='http://www.w3.org/2000/svg' width='13' height='13' viewBox='0 0 13 13'><path d='M3 7L5.7 9.9L10.4 3.3' fill='none' stroke='#fff' stroke-width='2.3'/></svg>";
/// Промежуточное: полоса по центру.
/// shortcut: вид полосы не снят эталоном (в 0.4.7 промежуточного не было); снять при сверке G3.
const BAR: &[u8] = b"<svg xmlns='http://www.w3.org/2000/svg' width='13' height='13' viewBox='0 0 13 13'><rect x='3' y='5.5' width='7' height='2' fill='#fff'/></svg>";

/// Состояние флажка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checked {
    Off,
    On,
    /// `indeterminate`: щелчок включает.
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckLook {
    Native,
    Box,
}

#[derive(IntoElement)]
pub struct Check {
    id: ElementId,
    state: Checked,
    look: CheckLook,
    side: f32,
    disabled: bool,
    label: Option<SharedString>,
    preview: Preview,
    base: Div,
    children: Vec<AnyElement>,
    on_change: Option<Handler<bool>>,
}

/// Флажок вида `look`; подпись — `.child(…)`.
pub fn check(id: impl Into<ElementId>, state: Checked, look: CheckLook) -> Check {
    Check {
        id: id.into(),
        state,
        look,
        side: if look == CheckLook::Box { 18. } else { 13. },
        disabled: false,
        label: None,
        preview: Preview::default(),
        base: div().flex().items_center(),
        children: Vec::new(),
        on_change: None,
    }
}

impl Check {
    /// Сторона стандартного флажка (`.pers__option > input` — 18, RunnerSettings — 16).
    pub fn side(mut self, side: f32) -> Self {
        self.side = side;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// `aria-label`, когда подписи в строке нет.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn preview(mut self, preview: Preview) -> Self {
        self.preview = preview;
        self
    }

    /// Новое `checked` после щелчка или пробела (из промежуточного — `true`).
    pub fn on_change(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl Styled for Check {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl ParentElement for Check {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements)
    }
}

/// Стандартный флажок Chromium: заливка, рамка и знак по состоянию.
fn native(state: Checked, side: f32, disabled: bool, hovered: bool, accent: Hsla) -> Div {
    let on = state != Checked::Off;
    let (bg, border): (Hsla, Option<Hsla>) = if on {
        let fill = if disabled {
            rgb(0x757575).into()
        } else if hovered {
            mix(accent, white(), 0.22)
        } else {
            accent
        };
        (fill, None)
    } else {
        // shortcut: наведение на выключенный (светлее рамка у Chromium) не снято эталоном.
        let border = if disabled { 0x626262 } else { 0x858585 };
        (rgb(0x3b3b3b).into(), Some(rgb(border).into()))
    };
    let mark = match state {
        Checked::Off => None,
        Checked::On => Some(MARK),
        Checked::Mixed => Some(BAR),
    };
    div()
        .relative()
        .flex_none()
        .size(u(side))
        .rounded(u(if side >= 18. { 3. } else { 2. }))
        .bg(bg)
        .when_some(border, |el, color| el.border_1().border_color(color))
        .when_some(mark, |el, mark| {
            el.child(
                svg()
                    .data(mark)
                    .absolute()
                    .top_0()
                    .left_0()
                    .size(u(side))
                    .text_color(rgb(0x3b3b3b)),
            )
        })
}

impl RenderOnce for Check {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let accent = theme.color("accent");
        let contrast = theme.color("accent-contrast-text");
        let strong = theme.color("l-border-strong");
        let focus_color = theme.color("accent-focus");
        let zoom = zoom(window);
        let focus = focus_handle(&self.id, window, cx);
        let ring_on = self.preview.focus || focus_visible(&focus, window);
        let Check {
            id,
            state,
            look,
            side,
            disabled,
            label,
            preview,
            base,
            children,
            on_change,
        } = self;
        let on = state != Checked::Off;
        // У `<label>` наведение на подпись — тоже `:hover` флажка: группа строки.
        let group: SharedString = format!("check-{id}").into();
        let mark = match look {
            CheckLook::Native => {
                // Наведение — только светлее заливка включённого (у выключенного не снято).
                let over = (on && !disabled && !preview.hover).then(|| mix(accent, white(), 0.22));
                native(state, side, disabled, preview.hover, accent)
                    .when_some(over, |el, fill| {
                        el.group_hover(group.clone(), move |s| s.bg(fill))
                    })
                    // У стандартного флажка контур с прямыми углами (снято эталоном в slint).
                    .when(ring_on, |el| {
                        el.child(outline(zoom, 0., 2., 2., focus_color))
                    })
                    .into_any_element()
            }
            CheckLook::Box => {
                let glyph_id =
                    gpui::ElementId::NamedChild(std::sync::Arc::new(id.clone()), "box".into());
                tween(
                    glyph_id,
                    f32::from(u8::from(on)),
                    Timing::fast(),
                    move |t| {
                        let clear = gpui::transparent_black();
                        div()
                            .relative()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(u(18.))
                            .rounded(u(5.))
                            .bg(mix(clear, accent, t))
                            .shadow(vec![ring(zoom, 1.5, mix(strong, clear, t))])
                            .text_color(mix(clear, contrast, t))
                            .child(fa_glyph(
                                if state == Checked::Mixed {
                                    "fas fa-minus"
                                } else {
                                    "fas fa-check"
                                },
                                10.,
                            ))
                            .when(ring_on, |el| {
                                el.child(outline(zoom, 5., 2., 2., focus_color))
                            })
                    },
                )
                .into_any_element()
            }
        };
        let toggled = match state {
            Checked::Off => Toggled::False,
            Checked::On => Toggled::True,
            Checked::Mixed => Toggled::Mixed,
        };
        let meta = crate::live::Meta::new("check", id.clone())
            .label(label.clone())
            .disabled(disabled)
            .focused(focus.is_focused(window))
            .checked(on);
        let element = base
            .id(id)
            .group(group)
            .role(Role::CheckBox)
            .aria_toggled(toggled)
            .when_some(label, |el, label| el.aria_label(label))
            .child(mark)
            .children(children)
            .when(!disabled, |el| {
                el.track_focus(&focus)
                    .cursor_pointer()
                    .when_some(on_change, |el, handler| {
                        el.on_click(move |event, window, cx| {
                            if !enter_key(event) {
                                handler(&(state != Checked::On), window, cx);
                            }
                        })
                    })
            });
        crate::live::mark(meta, element)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Modifiers, Render, TestAppContext};

    struct Form {
        state: Checked,
        changes: Vec<bool>,
    }

    impl Render for Form {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .p(u(20.))
                .child(div().debug_selector(|| "c".into()).child(
                    check("c", self.state, CheckLook::Native).on_change(cx.listener(
                        |this, on: &bool, _, cx| {
                            this.changes.push(*on);
                            this.state = if *on { Checked::On } else { Checked::Off };
                            cx.notify();
                        },
                    )),
                ))
        }
    }

    #[gpui::test]
    fn mixed_click_turns_on(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (form, cx) = cx.add_window_view(|_, _| Form {
            state: Checked::Mixed,
            changes: Vec::new(),
        });
        cx.run_until_parked();
        let at = cx.debug_bounds("c").unwrap().center();
        cx.simulate_click(at, Modifiers::default());
        cx.simulate_click(at, Modifiers::default());
        form.update(cx, |form, _| assert_eq!(form.changes, [true, false]));
    }
}
