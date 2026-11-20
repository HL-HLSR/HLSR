//! Вкладка «Проверка» (`GameCfgCheck.vue` 0.4.9): шапка (заголовок, «Проверить заново»,
//! категория Scriptless/Scripted, заметка), итог одной строкой (чипы уровней и «проверено
//! файлов»), группы «команда + правило» с числом мест; раскрытая группа показывает места,
//! место открывает файл в «Файлах cfg» на строке. Тексты — `hlsr_ui::controllers::cfg_check`.

use crate::icons::fa_glyph;
use crate::kit::button::{Look, button};
use crate::kit::gx;
use crate::motion::{Timing, hover};
use crate::scale::u;
use crate::scroll::{Bar, Scroll, ScrollArea};
use crate::text::{MONO, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ElementId, FontWeight, Hsla, IntoElement, ParentElement, Role, Styled, Window,
    div, hsla, prelude::*,
};
use hlsr_ui::controllers::cfg_check::{CATEGORIES, CheckView, PlaceView};
use std::collections::HashSet;
use std::rc::Rc;

/// Высота выбора категории: поля 2 + 3, строка `body` 20 px.
const CATEGORY_H: f32 = 2. * 2. + 3. * 2. + 20.;

/// Обработчик с аргументом `T`.
pub type On<T> = Rc<dyn Fn(T, &mut Window, &mut App)>;

/// Что делает вкладка: заново, категория, раскрыть группу, открыть место.
pub struct CheckActions {
    pub refresh: On<()>,
    pub category: On<&'static str>,
    pub toggle: On<String>,
    pub open: On<PlaceView>,
}

/// Цвет уровня (`--hc-*`): ошибка, предупреждение, справка (`--l-text-dim`), «нет нарушений».
pub fn level_color(level: &str, theme: &Theme) -> Hsla {
    theme.color(match level {
        "error" => "l-level-error",
        "warning" => "l-level-warning",
        "ok" => "l-level-ok",
        _ => "l-text-dim",
    })
}

fn white(alpha: f32) -> Hsla {
    hsla(0., 0., 1., alpha)
}

/// `.hc__chip`: значок и текст уровня на подложке .06.
fn chip(level: &str, icon: &str, text: String, theme: &Theme) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(u(6.))
        .px(u(10.))
        .py(u(3.))
        .rounded_full()
        .bg(white(0.06))
        .text_px(12.5, 20.)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(level_color(level, theme))
        .child(fa_glyph(&format!("fas {icon}"), 12.5))
        .child(text)
        .into_any_element()
}

/// `.hc__cat`: две кнопки-радио в одной «пилюле».
fn categories(
    current: &str,
    actions: &CheckActions,
    theme: &Theme,
    t: &dyn Fn(&str) -> String,
) -> AnyElement {
    div()
        .id("hc-cat")
        .role(Role::RadioGroup)
        .aria_label(t("#L_HC_CATEGORY"))
        .flex()
        .flex_none()
        .p(u(2.))
        .rounded_full()
        .bg(theme.color("l-chip-bg"))
        .children(CATEGORIES.iter().map(|(id, title)| {
            let on = *id == current;
            let pick = actions.category.clone();
            let (soft, text, dim) = (
                theme.color("accent-soft"),
                theme.color("l-text"),
                theme.color("l-text-dim"),
            );
            let id_name = ElementId::Name(format!("hc-cat-{id}").into());
            let meta = crate::live::Meta::new("radio", id_name.clone())
                .label(Some((*title).into()))
                .checked(on);
            let item = div()
                .id(id_name)
                .role(Role::RadioButton)
                .button_text()
                .aria_selected(on)
                .px(u(12.))
                .py(u(3.))
                .rounded_full()
                .when(on, |el| el.bg(soft))
                // Строка — 20 px `body` (`.hc__cat-item` своей не задаёт).
                .text_px(12., 20.)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if on { text } else { dim })
                .cursor_pointer()
                .on_click(move |_, window, cx| pick(id, window, cx))
                .child(*title);
            crate::live::mark(meta, item)
        }))
        .into_any_element()
}

fn place_row(
    index: usize,
    key: &str,
    place: &PlaceView,
    open: On<PlaceView>,
    theme: &Theme,
) -> AnyElement {
    let id = ElementId::Name(format!("hc-place-{key}-{index}").into());
    let enabled = place.path.is_some();
    let place = place.clone();
    let (accent, text, dim, faint) = (
        theme.color("accent"),
        theme.color("l-text"),
        theme.color("l-text-dim"),
        theme.color("l-text-faint"),
    );
    let inner = id.clone();
    hover(id, Timing::fast(), move |t| {
        let t = if enabled { t } else { 0. };
        let (where_text, command, context) = (
            place.place.clone(),
            place.command.clone(),
            place.context.clone(),
        );
        let place = place.clone();
        let open = open.clone();
        div()
            .id(inner.clone())
            .flex()
            .flex_wrap()
            .items_baseline()
            .gap_x(u(10.))
            .gap_y(u(2.))
            .w_full()
            .px(u(8.))
            .py(u(4.))
            .rounded(u(6.))
            .bg(white(0.07 * t))
            .text_px(12., 20.)
            .text_color(crate::motion::mix(dim, text, t))
            .when(!enabled, |el| el.opacity(0.6))
            .when(enabled, |el| {
                el.cursor_pointer()
                    .on_click(move |_, window, cx| open(place.clone(), window, cx))
            })
            .child(
                div()
                    .flex_none()
                    .font_family(MONO)
                    .text_color(accent)
                    .child(where_text),
            )
            .child(
                div()
                    .min_w_0()
                    .font_family(MONO)
                    .text_color(text)
                    .child(command),
            )
            .child(div().text_color(faint).child(context))
    })
    .into_any_element()
}

/// Вкладка целиком (`.gx-pane.hc`).
#[allow(clippy::too_many_arguments)]
pub fn check_pane(
    view: &CheckView,
    category: &str,
    open_groups: &HashSet<String>,
    scroll: &Scroll,
    actions: CheckActions,
    theme: &Theme,
    t: &dyn Fn(&str) -> String,
    tf: &dyn Fn(&str, &[&str]) -> String,
) -> AnyElement {
    let refresh = actions.refresh.clone();
    // `.gx-pane__head` — сетка `minmax(0, 1fr) auto`, ряды по центру: заголовок и «заново»
    // (34), «Категория» и выбор (30); «заново» у левого края своей колонки, как кнопка
    // фиксированной ширины в ячейке сетки. Заметка во всю ширину.
    let cell = |h: f32| div().min_h(u(h)).flex().items_center();
    let head = div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(u(3.))
        .px(u(14.))
        .py(u(10.))
        .border_b_1()
        .border_color(theme.color("l-border"))
        .child(
            div()
                .flex()
                .gap(u(12.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(u(3.))
                        .child(
                            cell(34.).child(
                                div()
                                    .min_w_0()
                                    .text_px(15., 15. * 1.3)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(t("#L_HC_TITLE")),
                            ),
                        )
                        .child(
                            cell(CATEGORY_H).child(
                                div()
                                    .text_px(12., 20.)
                                    .text_color(theme.color("l-text-dim"))
                                    .child(t("#L_HC_CATEGORY_LABEL")),
                            ),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(u(3.))
                        .child(
                            button("hc-refresh", Look::GxIcon)
                                .label(t("#L_HC_REFRESH"))
                                .disabled(view.loading)
                                .child(fa_glyph("fas fa-sync-alt", 13.))
                                .on_click(move |_, window, cx| refresh((), window, cx)),
                        )
                        .child(categories(category, &actions, theme, t)),
                ),
        )
        .child(
            div()
                .text_px(12., 12. * 1.4)
                .text_color(theme.color("l-text-faint"))
                .child(t("#L_HC_NOTE")),
        );
    let state_line = |failed: bool, text: String| {
        div()
            .flex()
            .items_center()
            .gap(u(8.))
            .text_px(13., 13. * 1.2)
            .text_color(if failed {
                level_color("warning", theme)
            } else {
                theme.color("l-text-dim")
            })
            .when(failed, |el| {
                el.child(fa_glyph("fas fa-exclamation-triangle", 13.))
            })
            .child(text)
            .into_any_element()
    };
    let mut body: Vec<AnyElement> = Vec::new();
    if !view.ready {
        body.push(state_line(
            !view.loading,
            t(if view.loading {
                "#L_HC_LOADING"
            } else {
                "#L_HC_FAILED"
            }),
        ));
    } else {
        if view.failed {
            body.push(state_line(true, t("#L_HC_STALE")));
        }
        let s = view.stats;
        let mut summary = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(u(8.))
            .gap_y(u(6.));
        if view.groups.is_empty() {
            summary = summary.child(chip("ok", "fa-check-circle", t("#L_HC_CLEAN"), theme));
        } else {
            let n = |v: u64| v.to_string();
            if s.errors > 0 {
                summary = summary.child(chip(
                    "error",
                    "fa-times-circle",
                    tf("#L_HC_COUNT_ERRORS", &[&n(s.errors)]),
                    theme,
                ));
            }
            if s.warnings > 0 {
                summary = summary.child(chip(
                    "warning",
                    "fa-exclamation-triangle",
                    tf("#L_HC_COUNT_WARNINGS", &[&n(s.warnings)]),
                    theme,
                ));
            }
            if s.infos > 0 {
                summary = summary.child(chip(
                    "info",
                    "fa-info-circle",
                    tf("#L_HC_COUNT_INFOS", &[&n(s.infos)]),
                    theme,
                ));
            }
        }
        summary = summary.child(
            div()
                .text_px(12., 12. * 1.2)
                .text_color(theme.color("l-text-faint"))
                .child(tf(
                    "#L_HC_FILES",
                    &[&s.files.to_string(), &s.commands.to_string()],
                )),
        );
        body.push(summary.into_any_element());
        if !view.groups.is_empty() {
            let groups = view.groups.iter().enumerate().map(|(index, group)| {
                let open = open_groups.contains(&group.key);
                let color = level_color(&group.level, theme);
                let toggle = actions.toggle.clone();
                let key = group.key.clone();
                let id = ElementId::Name(format!("hc-row-{}", group.key).into());
                let inner = id.clone();
                let (text, dim, faint) = (
                    theme.color("l-text"),
                    theme.color("l-text-dim"),
                    theme.color("l-text-faint"),
                );
                let row_view = group.clone();
                let row = hover(id, Timing::fast(), move |t| {
                    let toggle = toggle.clone();
                    let key = key.clone();
                    div()
                        .id(inner.clone())
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_x(u(10.))
                        .gap_y(u(4.))
                        .w_full()
                        .min_h(u(40.))
                        .pl(u(14.))
                        .pr(u(12.))
                        .py(u(6.))
                        .rounded(u(8.))
                        .bg(white(0.05 * t))
                        .text_px(13., 13. * 1.2)
                        .text_color(text)
                        .cursor_pointer()
                        .button_text()
                        .on_click(move |_, window, cx| toggle(key.clone(), window, cx))
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(u(6.))
                                .min_w(u(128.))
                                .text_px(12., 12. * 1.2)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(color)
                                .child(fa_glyph(&format!("fas {}", row_view.icon), 12.))
                                .child(row_view.level_text.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_family(MONO)
                                .text_size(u(12.5))
                                .child(row_view.name.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(u(200.))
                                .text_color(dim)
                                .child(row_view.reason.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_color(dim)
                                .tnum()
                                .child(format!("×\u{a0}{}", row_view.places.len())),
                        )
                        .child(
                            fa_glyph(
                                if open {
                                    "fas fa-chevron-up"
                                } else {
                                    "fas fa-chevron-down"
                                },
                                11.,
                            )
                            .flex_none()
                            .text_color(faint),
                        )
                });
                let places = open.then(|| {
                    div().pl(u(14.)).pr(u(8.)).pb(u(8.)).children(
                        group.places.iter().enumerate().map(|(index, place)| {
                            place_row(index, &group.key, place, actions.open.clone(), theme)
                        }),
                    )
                });
                // `box-shadow: inset 3px 0 0` — полоса уровня слева.
                div()
                    .relative()
                    .rounded(u(8.))
                    .bg(white(0.03))
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(u(3.))
                            .bg(color),
                    )
                    .child(crate::snap::target(format!("hc-row-{index}"), row))
                    .children(places)
            });
            body.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(u(4.))
                    .children(groups)
                    .into_any_element(),
            );
        }
    }
    gx::pane(theme)
        .size_full()
        .child(head)
        .child(
            ScrollArea::new("hc-body", scroll)
                .bar(Bar::Fields)
                .contain()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(u(10.))
                        .px(u(14.))
                        .py(u(12.))
                        .children(body),
                ),
        )
        .into_any_element()
}
