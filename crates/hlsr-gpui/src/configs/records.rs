//! Разделы «Призраки» и «Версия» Ghosting (`GameSettingsModal.vue` 0.4.9): записи `.run` —
//! карточками (значок в плашке, имя, размер, дата, удаление с вопросом в строке), сверху итог,
//! «Обновить» и папка; «Версия» — факты сборки сеткой 2 × N, по одному (`l-rise-in`, `--i`).

use super::footer::{BtnLook, icon_label, settings_btn};
use super::sections::GameSettings;
use crate::icons::{fa_glyph, icon};
use crate::kit::button::white;
use crate::layer::TipExt;
use crate::motion::{Timing, hover, mix, rise_in};
use crate::scale::u;
use crate::text::{RUBIK, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, FontWeight, IntoElement, ParentElement, Role, Styled, WeakEntity, div,
    prelude::*, rgb,
};
use hlsr_ui::controllers::game_settings::{RunsBody, View};

/// `.game-settings__empty`: рамка, 13 px, по центру.
pub fn empty_box(theme: &Theme, text: String) -> AnyElement {
    // Рамка — `inset 0 0 0 1px` (места не занимает): у `border` поля на 1 меньше.
    div()
        .px(u(15.))
        .py(u(31.))
        .rounded(u(theme.len("l-radius-sm")))
        .bg(white(0.02))
        .border_1()
        .border_color(theme.color("l-border"))
        .text_size(u(13.))
        .text_color(theme.color("l-text-faint"))
        .text_center()
        .child(text)
        .into_any_element()
}

/// Кнопка-значок записи (`.game-settings__run-delete`): 32 × 32, наведение .08.
fn run_icon(
    id: String,
    classes: &'static str,
    color: gpui::Hsla,
    disabled: bool,
    label: String,
    theme: &Theme,
    on_click: impl Fn(&mut App) + 'static,
) -> AnyElement {
    let (dim, text, radius) = (
        theme.color("l-text-dim"),
        theme.color("l-text"),
        theme.len("l-radius-sm"),
    );
    let spin = classes.contains("fa-spin");
    let inner = id.clone();
    hover(id, Timing::fast(), move |t| {
        let t = if disabled { 0. } else { t };
        div()
            .id(inner)
            .role(Role::Button)
            .button_text()
            .aria_label(label.clone())
            .tip(label)
            .size(u(32.))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(u(radius))
            .bg(white(0.08 * t))
            .text_color(mix(dim, text, t))
            .child(if spin {
                icon(classes, 13., color)
            } else {
                fa_glyph(classes, 13.)
                    .text_color(if color.a > 0. {
                        color
                    } else {
                        mix(dim, text, t)
                    })
                    .into_any_element()
            })
            .when(disabled, |el| el.opacity(0.5))
            .when(!disabled, |el| {
                el.cursor_pointer().on_click(move |_, _, cx| on_click(cx))
            })
    })
    .into_any_element()
}

/// «Призраки».
pub fn ghosts(
    view: &View,
    weak: WeakEntity<GameSettings>,
    t: &dyn Fn(&str) -> String,
    theme: &Theme,
) -> AnyElement {
    let theme = theme.clone();
    let act = |f: fn(&mut GameSettings)| {
        let weak = weak.clone();
        move |_: &gpui::ClickEvent, _: &mut gpui::Window, cx: &mut App| {
            weak.update(cx, |this, cx| {
                f(this);
                cx.notify();
            })
            .ok();
        }
    };
    let head = div()
        .flex()
        .items_center()
        .gap(u(8.))
        .pt(u(4.))
        .pb(u(16.))
        .child(
            div()
                .text_px(12., 20.)
                .text_color(theme.color("l-text-faint"))
                .child(view.runs_summary.clone()),
        )
        .child(div().flex_1())
        .child(
            settings_btn("gs-runs-refresh", BtnLook::Plain)
                .disabled(!view.runs_refresh_enabled)
                .on_click(act(|this| this.session().refresh_runs()))
                .children(icon_label("fas fa-sync-alt", t("#L_GS_REFRESH"), true)),
        )
        .child(
            settings_btn("gs-runs-folder", BtnLook::Plain)
                .disabled(!view.runs_folder_enabled)
                .on_click(act(|this| this.session().open_runs_folder()))
                .children(icon_label("fas fa-folder-open", t("#L_GS_OPEN_RUNS"), true)),
        );
    let body = match &view.runs_body {
        RunsBody::Loading => empty_box(&theme, t("#L_GS_LOADING")),
        RunsBody::Error(text) => empty_box(&theme, text.clone()),
        RunsBody::Empty => empty_box(&theme, t("#L_NO_DATA")),
        RunsBody::List => {
            let busy = view.runs.iter().any(|run| run.deleting);
            let mut list = div().flex().flex_col().gap(u(8.));
            for run in &view.runs {
                list = list.child(run_card(run, busy, &weak, &theme, t));
            }
            list.into_any_element()
        }
    };
    div()
        .flex()
        .flex_col()
        .child(head)
        .child(body)
        .when(!view.runs_error.is_empty(), |el| {
            el.child(
                div()
                    .mt(u(12.))
                    .text_size(u(12.))
                    .text_color(rgb(0xff8796))
                    .child(view.runs_error.clone()),
            )
        })
        .into_any_element()
}

fn run_card(
    run: &hlsr_ui::controllers::game_settings::RunRow,
    busy: bool,
    weak: &WeakEntity<GameSettings>,
    theme: &Theme,
    t: &dyn Fn(&str) -> String,
) -> AnyElement {
    let radius = theme.len("l-radius-sm");
    let (border, accent) = (theme.color("l-border"), theme.color("l-accent"));
    let hover_border = mix(border, accent, 0.45);
    let id = run.id.clone();
    let with = |f: fn(&mut GameSettings, &str)| {
        let (weak, id) = (weak.clone(), id.clone());
        move |cx: &mut App| {
            weak.update(cx, |this, cx| {
                f(this, &id);
                cx.notify();
            })
            .ok();
        }
    };
    let file = {
        let reveal = with(|this, id| this.session().reveal_run(id));
        div()
            .id(format!("gs-run-file-{id}"))
            .flex()
            .items_center()
            .gap(u(12.))
            .flex_1()
            .min_w_0()
            .cursor_pointer()
            .tip(t("#L_GS_REVEAL"))
            .button_text()
            .on_click(move |_, _, cx| reveal(cx))
            .child(
                div()
                    .flex_none()
                    .size(u(32.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(u(radius))
                    .bg(theme.color("l-accent-soft"))
                    .text_color(accent)
                    .child(fa_glyph("fas fa-ghost", 13.)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(run.name.clone()),
            )
    };
    let meta = |text: String| {
        div()
            .flex_none()
            .text_size(u(12.))
            .tnum()
            .text_color(theme.color("l-text-faint"))
            .child(text)
    };
    let none = gpui::transparent_black();
    let actions = if run.confirm {
        div()
            .flex()
            .items_center()
            .gap(u(8.))
            .child(
                div()
                    .text_px(12., 20.)
                    .text_color(theme.color("l-text-dim"))
                    .child(t("#L_GS_GHOST_DELETE_CONFIRM")),
            )
            .child(run_icon(
                format!("gs-run-yes-{id}"),
                if run.deleting {
                    "fas fa-spinner fa-spin"
                } else {
                    "fas fa-check"
                },
                rgb(0xff8796).into(),
                busy,
                t("#L_GS_GHOST_DELETE_YES"),
                theme,
                with(|this, id| this.session().delete_run(id)),
            ))
            .child(run_icon(
                format!("gs-run-no-{id}"),
                "fas fa-times",
                none,
                busy,
                t("#L_CANCEL"),
                theme,
                with(|this, _| this.session().cancel_run_delete()),
            ))
    } else {
        div().flex().items_center().gap(u(8.)).child(run_icon(
            format!("gs-run-delete-{id}"),
            "fas fa-trash-alt",
            none,
            busy,
            t("#L_GS_GHOST_DELETE"),
            theme,
            with(|this, id| this.session().confirm_run_delete(id)),
        ))
    };
    let (text, confirm) = (theme.color("l-text"), run.confirm);
    let (size, date) = (meta(run.size.clone()), meta(run.date.clone()));
    hover(format!("gs-run-{id}"), Timing::fast(), move |h| {
        div()
            .flex()
            .items_center()
            .gap(u(12.))
            .min_h(u(56.))
            .pl(u(11.))
            .pr(u(7.))
            .py(u(7.))
            .rounded(u(radius))
            .bg(white(0.03 + 0.02 * h))
            .border_1()
            .border_color(mix(border, hover_border, h))
            .text_size(u(13.))
            .text_color(text)
            .font_family(RUBIK)
            .child(file)
            .when(!confirm, |el| el.child(size).child(date))
            .child(actions)
    })
    .into_any_element()
}

/// «Версия».
pub fn version(view: &View, t: &dyn Fn(&str) -> String, theme: &Theme) -> AnyElement {
    let mut grid = div().grid().grid_cols(2).gap(u(8.)).mt(u(4.)).mb(u(16.));
    for (index, fact) in view.facts.iter().enumerate() {
        let card = div()
            .flex()
            .flex_col()
            .gap(u(2.))
            .min_w_0()
            .px(u(15.))
            .py(u(13.))
            .rounded(u(theme.len("l-radius-sm")))
            .bg(white(0.03))
            .border_1()
            .border_color(theme.color("l-border"))
            .child(
                div()
                    .text_px(15., 22.)
                    .font_weight(FontWeight::BOLD)
                    .tnum()
                    .text_color(theme.color("l-text"))
                    .child(fact.value.clone()),
            )
            .child(
                div()
                    .text_px(12., 18.)
                    .text_color(theme.color("l-text-dim"))
                    .child(fact.label.clone()),
            );
        // Лесенка 0.4.9 — `--i × 30 мс`; у реестра `l-rise-in` ступень 24 мс.
        grid = grid.child(rise_in(format!("gs-fact-{index}"), index, card));
    }
    div()
        .flex()
        .flex_col()
        .child(grid)
        .child(
            div()
                .text_px(12., 20.)
                .text_color(theme.color("l-text-faint"))
                .child(t("#L_GS_VER_SOURCE")),
        )
        .into_any_element()
}
