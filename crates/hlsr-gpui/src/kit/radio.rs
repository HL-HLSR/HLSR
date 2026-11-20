//! Выбор одного варианта — кнопка `role="radio"` (`.confirm__choice` ConfirmModal.vue 0.4.9):
//! значок `fa-dot-circle` у выбранного и `far fa-circle` у остальных 13 px, поле 10 × 12, рамка
//! `--l-border` (у выбранного `--accent`), фон .04 (при наведении .08), 13,5 px, переход
//! `--l-fast`. Каждый вариант — остановка Tab, Enter и пробел выбирают; двойной щелчок
//! (`click_count` 2 в [`ClickEvent`]) место понимает как «выбрать и принять».

use super::button::{Handler, Preview, focus_handle, focus_visible, outline, white};
use crate::icons::fa_glyph;
use crate::motion::{Timing, hover, mix};
use crate::scale::{u, zoom};
use crate::text::{RUBIK, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ClickEvent, ElementId, IntoElement, RenderOnce, Role, SharedString, Styled, Toggled,
    Window, div, prelude::*,
};
use std::rc::Rc;

#[derive(IntoElement)]
pub struct Radio {
    id: ElementId,
    label: SharedString,
    selected: bool,
    disabled: bool,
    preview: Preview,
    on_click: Option<Handler<ClickEvent>>,
}

pub fn radio(id: impl Into<ElementId>, label: impl Into<SharedString>, selected: bool) -> Radio {
    Radio {
        id: id.into(),
        label: label.into(),
        selected,
        disabled: false,
        preview: Preview::default(),
        on_click: None,
    }
}

impl Radio {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn preview(mut self, preview: Preview) -> Self {
        self.preview = preview;
        self
    }

    /// Щелчок, Enter или пробел.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Radio {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let accent = theme.color("accent");
        let border = if self.selected {
            accent
        } else {
            theme.color("l-border")
        };
        let dim = theme.color("l-text-dim");
        let text = theme.color("l-text");
        let radius = theme.len("l-radius-sm");
        let focus_color = theme.color("accent-focus");
        let zoom = zoom(window);
        let focus = focus_handle(&self.id, window, cx);
        let ring_on = self.preview.focus || focus_visible(&focus, window);
        let Radio {
            id,
            label,
            selected,
            disabled,
            preview,
            on_click,
        } = self;
        let inner = id.clone();
        let meta = crate::live::Meta::new("radio", id.clone())
            .label(Some(label.clone()))
            .disabled(disabled)
            .focused(focus.is_focused(window))
            .checked(selected);
        let element = hover(id, Timing::fast(), move |t| {
            let t = if disabled {
                0.
            } else if preview.hover {
                1.
            } else {
                t
            };
            let color = if selected { text } else { mix(dim, text, t) };
            let icon = if selected {
                "fas fa-dot-circle"
            } else {
                "far fa-circle"
            };
            div()
                .id(inner)
                .role(Role::RadioButton)
                .button_text()
                .aria_toggled(if selected {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .aria_label(label.clone())
                .relative()
                .flex()
                .items_center()
                .gap(u(10.))
                .px(u(12.))
                .py(u(10.))
                .border_1()
                .border_color(border)
                .rounded(u(radius))
                .bg(mix(white(0.04), white(0.08), t))
                .font_family(RUBIK)
                .text_size(u(13.5))
                .text_color(color)
                .child(fa_glyph(icon, 13.).when(selected, |el| el.text_color(accent)))
                .child(label)
                .when(!disabled, |el| {
                    el.track_focus(&focus)
                        .cursor_pointer()
                        .when_some(on_click, |el, handler| {
                            el.on_click(move |event, window, cx| handler(event, window, cx))
                        })
                })
                .when(ring_on, |el| {
                    el.child(outline(zoom, radius, 2., 2., focus_color))
                })
        });
        crate::live::mark(meta, element)
    }
}
