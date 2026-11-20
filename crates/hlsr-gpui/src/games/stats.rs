//! Статистика героя (`GameStats.vue` 0.4.9): три строки — значок, линия, подпись и значение.
//! Значок и столбик текста одной высоты (`--l-stats-icon`), линия места не занимает: с обеих
//! сторон один зазор 20. Меняется только значение — перетекает на месте (`l-value`). Тексты —
//! [`hlsr_ui::controllers::game_stats`].

use super::hero::paper;
use crate::icons::icon;
use crate::motion::{VALUE, swap};
use crate::scale::u;
use crate::shell::alpha;
use crate::text::{TextExt, spaced};
use crate::theme::Theme;
use gpui::{Div, FontWeight, SharedString, div, prelude::*, px, white};
use hlsr_ui::controllers::game_stats::StatsRow;

/// Строки статистики (`.game-stats`).
pub fn game_stats(rows: &[StatsRow], theme: &Theme) -> Div {
    let icon_box = theme.len("l-stats-icon");
    let icon_color = paper(theme, "l-paper-stats-icon", white());
    let label_color = paper(theme, "l-paper-stats-label", alpha(white(), 0.62));
    let divider = paper(theme, "l-paper-stats-divider", alpha(white(), 0.28));
    let value_color = theme.color("l-text");
    let (value_size, value_lh) = (theme.len("l-stats-value"), theme.len("l-stats-value-lh"));
    let row = |index: usize, row: &StatsRow| {
        div()
            .flex()
            .items_center()
            .gap(u(20.))
            .child(
                // Значок прижат к линии; у всех глифов одна область 24 px.
                div()
                    .size(u(icon_box))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .child(
                        div()
                            .w(u(24.))
                            .flex()
                            .justify_center()
                            .child(icon(row.icon, 22., icon_color)),
                    ),
            )
            .child(
                div().relative().w_0().h(u(icon_box)).child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left_0()
                        .w(px(1.))
                        .bg(divider),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.rem("l-stats-texts-gap"))
                    .child(spaced(
                        row.label.to_uppercase(),
                        14.,
                        20.,
                        FontWeight::MEDIUM,
                        0.09,
                        label_color,
                    ))
                    .child(swap(
                        ("hero-stat", index),
                        row.value.clone(),
                        VALUE,
                        move |value: &String| {
                            div()
                                .text_px(value_size, value_lh)
                                .font_weight(FontWeight::BOLD)
                                .text_color(value_color)
                                .whitespace_nowrap()
                                .child(SharedString::from(value.clone()))
                        },
                    )),
            )
    };
    div()
        .flex()
        .flex_col()
        .gap(theme.rem("l-stats-gap"))
        .children(
            rows.iter()
                .enumerate()
                .map(|(index, item)| row(index, item)),
        )
}
