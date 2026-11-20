//! Бросание файлов на страницу (`.dmp__drop` 0.4.9): пока файлы над страницей — рамка пунктиром
//! акцентом и подсказка поверх; бросили — демки идут в `demo-add-paths`, остальное — в строку итога.
//! Подложка — заливка `--l-bg` 92 % без размытия (решение по размытию: только стекло панелей).

use super::panel::DemoPanel;
use crate::icons::fa_glyph;
use crate::motion::{FADE, presence};
use crate::scale::u;
use crate::theme::Theme;
use gpui::{
    AnyElement, Context, DragMoveEvent, ExternalPaths, FontWeight, IntoElement, Window, div,
    prelude::*,
};

/// Файлы над страницей (`dragenter`/`dragleave`): мышь в рамке страницы, пока идёт перенос из ОС.
pub fn on_drag_move(
    this: &mut DemoPanel,
    event: &DragMoveEvent<ExternalPaths>,
    _: &mut Window,
    cx: &mut Context<DemoPanel>,
) {
    let inside = event.bounds.contains(&event.event.position);
    if this.s.dragging != inside {
        this.s.dragging = inside;
        cx.notify();
    }
}

/// Слой подсказки: от шапки и краёв страницы с запасом 8 px; появление — `--l-fast`.
pub fn overlay(dragging: bool, panel: &DemoPanel, theme: &Theme) -> AnyElement {
    let (accent, dim, text) = (
        theme.color("accent"),
        theme.color("l-text-dim"),
        theme.color("l-text"),
    );
    let bg = theme.color("l-bg").opacity(0.92);
    let radius = theme.rem("l-radius");
    let (top, edge, hero) = (
        theme.len("l-topbar-h") + 8.,
        theme.len("l-edge-pad") - 8.,
        theme.len("l-hero-pad") - 8.,
    );
    let (title, hint) = (
        panel.t("#L_DEMO_DROP_TITLE", &[]),
        panel.t("#L_DEMO_DROP_TEXT", &[]),
    );
    presence("dmp-drop", dragging, FADE, move || {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .border_2()
            .border_dashed()
            .border_color(accent)
            .rounded(radius)
            .bg(bg)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(u(8.))
                    .text_size(u(13.5))
                    .text_color(dim)
                    .child(
                        fa_glyph("fas fa-file-import", 36.)
                            .mb(u(6.))
                            .text_color(accent),
                    )
                    .child(
                        div()
                            .text_size(u(18.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text)
                            .child(title),
                    )
                    .child(hint),
            )
    })
    .absolute()
    .top(u(top))
    .right(u(edge))
    .bottom(u(edge))
    .left(u(hero))
    .into_any_element()
}
