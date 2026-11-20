//! Ссылка-кнопка 0.4.9: текст `--l-accent` без фона и рамки. [`Underline::Always`] —
//! `.settings-link` (подчёркнута, при наведении `filter: brightness(1.15)`),
//! [`Underline::Hover`] — `.demo-ffmpeg__link` (подчёркивание при наведении, значок 11 px через
//! 6 px). Фокус с клавиатуры — контур 2 px `--accent-focus` со смещением 2 px. Ссылка шрифтом
//! своей строки (`.welcome__link`) — [`text_link`].
//! `text-underline-offset` нет: линия по метрикам шрифта.

use super::button::{Handler, Preview, focus_handle, focus_visible, outline};
use crate::icons::fa_glyph;
use crate::scale::{u, zoom};
use crate::text::{RUBIK, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, ElementId, Hsla, IntoElement, RenderOnce, Rgba, Role,
    SharedString, Styled, Window, div, prelude::*, relative,
};
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Underline {
    Always,
    Hover,
}

#[derive(IntoElement)]
pub struct Link {
    id: ElementId,
    text: SharedString,
    size: f32,
    underline: Underline,
    icon: Option<&'static str>,
    preview: Preview,
    on_click: Option<Handler<ClickEvent>>,
}

/// Ссылка `text` кеглем 12 px (`.settings-link`), подчёркнутая.
pub fn link(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Link {
    Link {
        id: id.into(),
        text: text.into(),
        size: 12.,
        underline: Underline::Always,
        icon: None,
        preview: Preview::default(),
        on_click: None,
    }
}

impl Link {
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    pub fn underline(mut self, underline: Underline) -> Self {
        self.underline = underline;
        self
    }

    /// Значок перед текстом (классы `<i>`).
    pub fn icon(mut self, classes: &'static str) -> Self {
        self.icon = Some(classes);
        self
    }

    pub fn preview(mut self, preview: Preview) -> Self {
        self.preview = preview;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

/// Ссылка-кнопка шрифтом своей строки (`.welcome__link`, `.runner-signin__privacy`): `<button>`
/// цветом `--l-accent`, подчёркнутая, без наведения; остановка Tab, фокус с клавиатуры — контур
/// 2 px `--accent` со смещением 2 px. Щелчок до родителя не доходит (`@click.prevent` в `<label>`).
pub fn text_link(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (id, text) = (id.into(), text.into());
    let theme = cx.global::<Theme>();
    let (color, ring) = (theme.color("l-accent"), theme.color("accent"));
    let zoom = zoom(window);
    let focus = focus_handle(&id, window, cx);
    let ring_on = focus_visible(&focus, window);
    let meta = crate::live::Meta::new("link", id.clone())
        .label(Some(text.clone()))
        .focused(focus.is_focused(window));
    let element = div()
        .id(id)
        .role(Role::Button)
        .button_text()
        .aria_label(text.clone())
        .relative()
        .flex_none()
        .text_color(color)
        .track_focus(&focus)
        .cursor_pointer()
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_click(window, cx);
        })
        .child(div().underline().child(text))
        .when(ring_on, |el| el.child(outline(zoom, 0., 2., 2., ring)));
    crate::live::mark(meta, element)
}

/// `filter: brightness(k)` для цвета: каналы × k.
fn brightness(color: Hsla, k: f32) -> Hsla {
    let c = Rgba::from(color);
    Rgba {
        r: (c.r * k).min(1.),
        g: (c.g * k).min(1.),
        b: (c.b * k).min(1.),
        a: c.a,
    }
    .into()
}

impl RenderOnce for Link {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let color = theme.color("l-accent");
        let focus_color = theme.color("accent-focus");
        let zoom = zoom(window);
        let focus = focus_handle(&self.id, window, cx);
        let ring_on = self.preview.focus || focus_visible(&focus, window);
        let Link {
            id,
            text,
            size,
            underline,
            icon,
            preview,
            on_click,
        } = self;
        let bright = brightness(color, 1.15);
        let always = underline == Underline::Always;
        let over = preview.hover;
        // У ссылок нет перехода: наведение — сразу (`:hover`), подчёркивание — только у текста
        // (значок `inline-block` линию не берёт).
        let group: SharedString = format!("link-{id}").into();
        let meta = crate::live::Meta::new("link", id.clone())
            .label(Some(text.clone()))
            .focused(focus.is_focused(window));
        let element = div()
            .id(id)
            .group(group.clone())
            .role(Role::Link)
            // Роль ссылки, но в 0.4.9 это `<button>` (`.settings-link`): разрядки `body` нет.
            .button_text()
            .aria_label(text.clone())
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(6.))
            .font_family(RUBIK)
            .text_size(u(size))
            .line_height(relative(1.4))
            .text_color(if over && always { bright } else { color })
            .when(always, |el| el.hover(move |s| s.text_color(bright)))
            .track_focus(&focus)
            .cursor_pointer()
            .when_some(icon, |el, icon| el.child(fa_glyph(icon, 11.)))
            .child(
                div()
                    .when(always || over, |el| el.underline())
                    .when(!always, |el| el.group_hover(group, |s| s.underline()))
                    .child(text),
            )
            .when_some(on_click, |el, handler| {
                el.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .when(ring_on, |el| {
                el.child(outline(zoom, 0., 2., 2., focus_color))
            });
        crate::live::mark(meta, element)
    }
}
