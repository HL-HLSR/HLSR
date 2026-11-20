//! Карточка под плеером (`.dmp__card`): название, «неполная», «Экспорт» с процентом, факты,
//! строка интервала постоянной высоты, «Клип не сохранился», «Убрать»; справа — порядок файлов
//! набора или метки. У видео «Отрендеренных» — «Экспорт», папка и корзина значками, метки.
//! Кнопка посередине сворачивает карточку (`.dmp__fold`).

use super::panel::DemoPanel;
use super::{BtnLook, btn, btn_icon, count, icon_btn, link};
use crate::icons::fa_glyph;
use crate::kit::button::white;
use crate::layer::TipExt;
use crate::motion::{Timing, hover, mix};
use crate::paint::ring;
use crate::scale::{u, zoom};
use crate::scroll::{Bar, ScrollArea};
use crate::text::{TextExt, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, Div, FontWeight, Hsla, IntoElement, Role, SharedString, Window, div,
    prelude::*, relative, rgb,
};
use hlsr_ui::controllers::demo_tool::{clock, error_text, format_clock, marker_text};
use serde_json::Value;

/// Цвет точки метки по типу (`.dmp__row-dot.is-*`).
pub fn marker_color(kind: &str, theme: &Theme) -> Hsla {
    match kind {
        "map" => theme.color("accent"),
        "saveload" => rgb(0xffb547).into(),
        "timer" => rgb(0x5fd17f).into(),
        "chapter" => rgb(0xb98cff).into(),
        "finish" => rgb(0xff6b6b).into(),
        _ => theme.color("l-text-dim"),
    }
}

/// Факты `dl`: подпись капсом 11 px и значение 13 px; в низком окне — плотнее.
fn facts(list: Vec<(String, String)>, low: bool, theme: &Theme) -> Div {
    let (faint, text) = (theme.color("l-text-faint"), theme.color("l-text"));
    div()
        .flex()
        .flex_wrap()
        .gap_x(u(if low { 14. } else { 22. }))
        .gap_y(u(if low { 4. } else { 6. }))
        .children(list.into_iter().map(|(label, value)| {
            div()
                .id(SharedString::from(format!("fact-{label}")))
                .min_w_0()
                .max_w(u(if low { 150. } else { 300. }))
                .flex()
                .flex_col()
                .gap(u(1.))
                .child(spaced(
                    label.to_uppercase(),
                    11.,
                    if low { 13. } else { 20. },
                    FontWeight::SEMIBOLD,
                    0.05,
                    faint,
                ))
                .child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_px(13., if low { 17. } else { 20. })
                        .tnum()
                        .text_color(text)
                        .child(value.clone()),
                )
                .tip(value)
        }))
}

/// Заголовок правой части: капс 11,5 px с разрядкой .08em.
fn side_title(text: String, count: Option<AnyElement>, theme: &Theme) -> Div {
    let dim = theme.color("l-text-dim");
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(u(8.))
        .child(spaced(
            text.to_uppercase(),
            11.5,
            20.,
            FontWeight::SEMIBOLD,
            0.08,
            dim,
        ))
        .children(count)
}

/// Строка метки-кнопки: точка по типу, подпись, время.
fn marker_row(
    id: SharedString,
    color: Hsla,
    label: String,
    time: String,
    window: &mut Window,
    cx: &mut gpui::App,
    on_click: super::OnClick,
) -> AnyElement {
    let super::Focus {
        handle,
        ring: focus,
    } = super::focus(&id.clone().into(), 6., -2., window, cx);
    let theme = cx.global::<Theme>();
    let (dim, text, faint) = (
        theme.color("l-text-dim"),
        theme.color("l-text"),
        theme.color("l-text-faint"),
    );
    let inner = id.clone();
    hover(id, Timing::fast(), move |t| {
        div()
            .id(inner)
            .role(Role::Button)
            .button_text()
            .relative()
            .track_focus(&handle)
            .flex()
            .items_center()
            .gap(u(8.))
            .w_full()
            .min_h(u(28.))
            .px(u(4.))
            .py(u(2.))
            .rounded(u(6.))
            .cursor_pointer()
            .bg(white(0.06 * t))
            .text_size(u(12.5))
            .text_color(mix(dim, text, t))
            .child(div().flex_none().size(u(8.)).rounded(u(2.)).bg(color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_color(text)
                    .child(label),
            )
            .child(div().flex_none().tnum().text_color(faint).child(time))
            .children(focus)
            .on_click(move |event, window, cx| on_click(event, window, cx))
    })
    .into_any_element()
}

/// Пустая карточка (`.dmp__card--empty`): подсказка посередине.
fn empty_card(text: String, theme: &Theme) -> AnyElement {
    card_frame(theme)
        .flex()
        .items_center()
        .justify_center()
        .text_size(u(13.))
        .text_color(theme.color("l-text-faint"))
        .child(text)
        .into_any_element()
}

fn card_frame(theme: &Theme) -> Div {
    div()
        .flex_shrink(1.)
        .min_h_0()
        .overflow_hidden()
        .py(u(14.))
        .px(u(16.))
        .border_1()
        .border_color(theme.color("l-border"))
        .rounded(theme.rem("l-radius-sm"))
        .bg(theme.color("l-modal-bar"))
}

impl DemoPanel {
    /// `.dmp__fold`: полоса 12 px с кнопкой 44 × 14; при наведении — линия акцентом.
    pub(super) fn fold(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let weak = cx.weak_entity();
        let zoom = zoom(window);
        let hidden = self.s.card_hidden;
        let label = self.t(
            if hidden {
                "#L_DEMO_CARD_SHOW"
            } else {
                "#L_DEMO_CARD_HIDE"
            },
            &[],
        );
        let (dim, accent, bar, strong) = (
            theme.color("l-text-dim"),
            theme.color("accent"),
            theme.color("l-modal-bar"),
            theme.color("l-border-strong"),
        );
        let on_click = super::act(&weak, |this, _, _| this.toggle_card());
        let focus = super::focus(&"dmp-fold-btn".into(), 0., 0., window, cx);
        let lit = focus.ring.is_some();
        let el = hover("dmp-fold", Timing::fast(), move |t| {
            let t = if lit { 1. } else { t };
            div()
                .id("dmp-fold-btn")
                .role(Role::Button)
                .button_text()
                .track_focus(&focus.handle)
                .aria_label(SharedString::from(label.clone()))
                .tip(label)
                .relative()
                .flex_none()
                .h(u(12.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(u(5.))
                        .h(u(2.))
                        .rounded(u(1.))
                        .bg(accent.opacity(0.6 * t)),
                )
                .child(
                    div()
                        .relative()
                        .w(u(44.))
                        .h(u(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(u(7.))
                        .bg(bar)
                        .shadow(vec![ring(zoom, 1., mix(strong, accent, t))])
                        .text_color(mix(dim, gpui::white(), t))
                        .child(fa_glyph(
                            if hidden {
                                "fas fa-chevron-up"
                            } else {
                                "fas fa-chevron-down"
                            },
                            9.,
                        )),
                )
                .on_click(move |event, window, cx| on_click(event, window, cx))
        });
        crate::snap::target("dmp-fold", el)
    }

    /// Карточка выбранной демки.
    pub(super) fn demo_card(
        &mut self,
        low: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let weak = cx.weak_entity();
        let t = self.translate();
        let s = &self.s;
        let Some(item) = s.item().cloned() else {
            let key = if s.detail_loading {
                "#L_DEMO_LOADING"
            } else if s.items().is_empty() {
                "#L_DEMO_PICK_EMPTY"
            } else {
                "#L_DEMO_PICK_HINT"
            };
            return empty_card(self.t(key, &[]), &theme);
        };
        let text = theme.color("l-text");
        let name = hlsr_ui::controllers::demo_panel::DemoPanel::item_name(&item, self.language, &t);
        let supported = item.get("supported").is_some_and(hlsr_core::js::truthy);
        let own_export = s.own_export();
        let export_label = match s.job.as_ref().filter(|_| own_export) {
            Some(job) => format!(
                "{} {}%",
                self.t("#L_DEMO_EXPORT", &[]),
                hlsr_core::js::number::number_to_string(job.percent.round().max(0.))
            ),
            None => self.t("#L_DEMO_EXPORT", &[]),
        };
        let export = btn(
            "dmp-export",
            BtnLook {
                primary: true,
                height: if low { 30. } else { 32. },
                px: 14.,
                push: true,
            },
            !supported,
            own_export.then(|| {
                format!(
                    "{} — {}",
                    self.t("#L_DEMO_EXPORT_RUNNING", &[]),
                    self.t("#L_DEMO_EXPORT_OPEN", &[])
                )
            }),
            vec![
                btn_icon(
                    if own_export {
                        "fas fa-spinner fa-spin"
                    } else {
                        "fas fa-file-video"
                    },
                    theme.color("accent-contrast-text"),
                ),
                export_label.into_any_element(),
            ],
            window,
            cx,
            super::act(&weak, |this, _, cx| this.open_export(cx)),
        );
        let incomplete = item
            .get("incomplete")
            .is_some_and(hlsr_core::js::truthy)
            .then(|| {
                let label = self.t("#L_DEMO_INCOMPLETE", &[]);
                div()
                    .id("dmp-tag")
                    .flex_shrink(1.)
                    .min_w(u(24.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .px(u(8.))
                    .py(u(2.))
                    .rounded(u(9.))
                    .bg(white(0.08))
                    .text_color(theme.color("l-text-dim"))
                    .text_px(11., 20.)
                    .font_weight(FontWeight::SEMIBOLD)
                    .tip(label.clone())
                    .child(label)
            });
        let head = div()
            .flex()
            .items_center()
            .gap(u(10.))
            .min_w_0()
            .child(
                div()
                    .id("dmp-info-title")
                    .flex_shrink_0()
                    .max_w_full()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_px(16., 22.)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(text)
                    .tip(name.clone())
                    .child(name),
            )
            .children(incomplete)
            .child(
                div()
                    .ml_auto()
                    .flex_none()
                    .child(crate::snap::target("dmp-export", export)),
            );
        // Строка интервала держит высоту всегда: шкала не уезжает из-под курсора.
        let range = s.range.map(|range| {
            div()
                .flex()
                .items_center()
                .gap(u(8.))
                .child(fa_glyph("fas fa-cut", 12.5).text_color(theme.color("accent")))
                .child(
                    div()
                        .text_color(text)
                        .child(self.t("#L_DEMO_RANGE", &[&clock(range.from), &clock(range.to)])),
                )
                .child(link(
                    "dmp-range-reset",
                    self.t("#L_DEMO_RANGE_RESET", &[]),
                    window,
                    cx,
                    super::act(&weak, |this, _, _| this.s.reset_range()),
                ))
        });
        let note = s.range_note.map(|key| {
            let note = self.t(key, &[]);
            div()
                .id("dmp-range-note")
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .text_color(theme.color("l-level-warning"))
                .tip(note.clone())
                .child(note)
        });
        let range_row = div()
            .flex()
            .items_center()
            .gap(u(8.))
            .min_h(u(20.))
            .whitespace_nowrap()
            .text_size(u(12.5))
            .tnum()
            .text_color(theme.color("accent"))
            .when(range.is_none() && note.is_none(), |el| el.invisible())
            .children(range)
            .children(note);
        let failed = s
            .own_outcome()
            .and_then(|outcome| outcome.error.clone())
            .map(|error| {
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .text_px(12.5, 12.5 * 1.4)
                    .text_color(theme.color("l-level-error"))
                    .child(fa_glyph("fas fa-exclamation-circle", 12.5))
                    .child(div().min_w_0().child(format!(
                        "{}: {}",
                        self.t("#L_DEMO_EX_FAILED", &[]),
                        error_text(&error, &t)
                    )))
                    .child(link(
                        "dmp-failed-details",
                        self.t("#L_DEMO_EX_DETAILS", &[]),
                        window,
                        cx,
                        super::act(&weak, |this, _, cx| this.open_export(cx)),
                    ))
            });
        let manual = item.get("source").and_then(Value::as_str) == Some("manual");
        let actions = manual.then(|| {
            let remove = btn(
                "dmp-remove",
                BtnLook::PLAIN,
                s.removing,
                Some(self.t("#L_DEMO_REMOVE_HINT", &[])),
                vec![
                    btn_icon("fas fa-times", text),
                    self.t("#L_DEMO_REMOVE", &[]).into_any_element(),
                ],
                window,
                cx,
                super::act(&weak, |this, _, cx| this.remove_item(cx)),
            );
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(u(8.))
                .mt_auto()
                .child(crate::snap::target("dmp-remove", remove))
        });
        let info = ScrollArea::new("dmp-info", &self.info_scroll)
            .bar(Bar::Fields)
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .min_h_full()
                    .flex()
                    .flex_col()
                    .gap(u(if low { 6. } else { 8. }))
                    .child(head)
                    .child(facts(self.s.facts(&t), low, &theme))
                    .child(range_row)
                    .children(failed)
                    .children(actions),
            );
        let side = self.demo_side(&item, window, cx);
        self.card_shell(low, info.into_any_element(), side, &theme)
    }

    /// Правая часть карточки демки: порядок файлов набора или метки.
    fn demo_side(
        &mut self,
        item: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let weak = cx.weak_entity();
        let t = self.translate();
        let (dim, text, faint) = (
            theme.color("l-text-dim"),
            theme.color("l-text"),
            theme.color("l-text-faint"),
        );
        if item.get("kind").and_then(Value::as_str) == Some("segments") {
            let series = item.get("series").is_some_and(hlsr_core::js::truthy);
            let manual = item.get("source").and_then(Value::as_str) == Some("manual");
            let segments: Vec<Value> = self
                .s
                .detail
                .as_ref()
                .and_then(|detail| detail.get("segments"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let last = segments.len().saturating_sub(1);
            let mut rows = div().flex().flex_col();
            for (index, segment) in segments.iter().enumerate() {
                let name = segment
                    .get("name")
                    .map(hlsr_core::js::to_string)
                    .unwrap_or_default();
                let time = segment
                    .get("duration")
                    .and_then(Value::as_f64)
                    .map_or_else(String::new, |d| format_clock(d, 1));
                let mut row = div()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .min_h(u(28.))
                    .px(u(4.))
                    .py(u(2.))
                    .text_size(u(12.5))
                    .text_color(dim)
                    .child(
                        div()
                            .flex_none()
                            .min_w(u(18.))
                            .text_right()
                            .tnum()
                            .text_color(faint)
                            .child((index + 1).to_string()),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("dmp-seg-{index}")))
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(text)
                            .tip(name.clone())
                            .child(name.clone()),
                    )
                    .child(div().flex_none().tnum().text_color(faint).child(time));
                if manual {
                    for (delta, glyph, key) in [
                        (-1isize, "fas fa-arrow-up", "#L_DEMO_ORDER_UP"),
                        (1, "fas fa-arrow-down", "#L_DEMO_ORDER_DOWN"),
                    ] {
                        let disabled = self.s.reordering
                            || (delta < 0 && index == 0)
                            || (delta > 0 && index == last);
                        let label = self.t(key, &[]);
                        let on_click = super::act(&weak, move |this, _, cx| {
                            this.move_segment(index, delta, cx)
                        });
                        let id = SharedString::from(format!("dmp-move-{index}-{delta}"));
                        let inner = id.clone();
                        let aria = format!("{label}: {name}");
                        let target = id.clone();
                        let super::Focus {
                            handle,
                            ring: focus,
                        } = super::focus(&id.clone().into(), 6., -2., window, cx);
                        let button = hover(id, Timing::fast(), move |t| {
                            let t = if disabled { 0. } else { t };
                            div()
                                .id(inner)
                                .role(Role::Button)
                                .button_text()
                                .relative()
                                .children(focus)
                                .aria_label(SharedString::from(aria))
                                .tip(label)
                                .flex_none()
                                .size(u(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(u(6.))
                                .bg(white(0.08 * t))
                                .text_color(mix(dim, text, t))
                                .child(fa_glyph(glyph, 11.))
                                .when(disabled, |el| el.opacity(0.3))
                                .when(!disabled, |el| {
                                    el.cursor_pointer().track_focus(&handle).on_click(
                                        move |event, window, cx| on_click(event, window, cx),
                                    )
                                })
                        });
                        row = row.child(crate::snap::target(target, button));
                    }
                }
                rows = rows.child(row);
            }
            let title = self.t(
                if series {
                    "#L_DEMO_SERIES_FILES"
                } else {
                    "#L_DEMO_ORDER_TITLE"
                },
                &[],
            );
            return self.side_frame(
                side_title(title, None, &theme),
                rows.into_any_element(),
                &theme,
            );
        }
        let markers = self.s.side_markers();
        let title = side_title(
            self.t("#L_DEMO_MARKS_TITLE", &[]),
            Some(count(markers.len(), &theme)),
            &theme,
        );
        let body = if markers.is_empty() {
            div()
                .text_size(u(12.5))
                .text_color(faint)
                .child(self.t("#L_DEMO_MARKS_EMPTY", &[]))
                .into_any_element()
        } else {
            let mut rows = div().flex().flex_col();
            for (index, (marker, time)) in markers.into_iter().enumerate() {
                let kind = marker
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let on_click = super::act(&weak, move |this, _, cx| this.jump_to(time, cx));
                rows = rows.child(crate::snap::target(
                    SharedString::from(format!("dmp-mark-{index}")),
                    marker_row(
                        SharedString::from(format!("dmp-mark-row-{index}")),
                        marker_color(kind, &theme),
                        marker_text(&marker, &t),
                        clock(time),
                        window,
                        cx,
                        on_click,
                    ),
                ));
            }
            rows.into_any_element()
        };
        self.side_frame(title, body, &theme)
    }

    /// `.dmp__side`: высоту карточки задаёт левая часть; список листается внутри.
    fn side_frame(&self, title: Div, body: AnyElement, theme: &Theme) -> AnyElement {
        div()
            .relative()
            .flex_none()
            .w(relative(0.34))
            .min_w(u(220.))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .gap(u(6.))
                    .pl(u(16.))
                    .border_l_1()
                    .border_color(theme.color("l-border"))
                    .child(title)
                    .child(
                        ScrollArea::new("dmp-side", &self.side_scroll)
                            .bar(Bar::Fields)
                            .contain()
                            .flex_1()
                            .min_h_0()
                            .child(body),
                    ),
            )
            .into_any_element()
    }

    /// Карточка: левая часть и правая, поля 14/16 (в низком окне 8 и не ниже 156).
    fn card_shell(
        &self,
        low: bool,
        info: AnyElement,
        side: AnyElement,
        theme: &Theme,
    ) -> AnyElement {
        card_frame(theme)
            .when(low, |el| el.min_h(u(156.)).py(u(8.)))
            .flex()
            .gap(u(16.))
            .child(info)
            .child(side)
            .into_any_element()
    }

    /// Карточка открытого видео «Отрендеренных».
    pub(super) fn rendered_card(
        &mut self,
        low: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let weak = cx.weak_entity();
        let t = self.translate();
        let Some(open) = self.s.rendered.open.clone() else {
            return empty_card(self.t("#L_DEMO_RENDERED_PICK", &[]), &theme);
        };
        let text = theme.color("l-text");
        let name = open
            .get("name")
            .map(hlsr_core::js::to_string)
            .unwrap_or_default();
        let source = open.get("sourceId").is_some_and(hlsr_core::js::truthy);
        let busy = self.s.job.is_some();
        let export = btn(
            "dmp-rendered-export",
            BtnLook {
                primary: true,
                height: if low { 30. } else { 32. },
                px: 14.,
                push: true,
            },
            !source || busy,
            Some(self.t(
                if source {
                    "#L_DEMO_RENDERED_EXPORT_HINT"
                } else {
                    "#L_DEMO_RENDERED_NO_SOURCE"
                },
                &[],
            )),
            vec![
                btn_icon("fas fa-file-video", theme.color("accent-contrast-text")),
                self.t("#L_DEMO_EXPORT", &[]).into_any_element(),
            ],
            window,
            cx,
            super::act(&weak, |this, _, cx| this.export_from_rendered(cx)),
        );
        let size = if low { 30. } else { 32. };
        let reveal = icon_btn(
            "dmp-rendered-reveal",
            "fas fa-folder-open",
            size,
            self.t("#L_DEMO_RENDERED_REVEAL", &[]),
            false,
            true,
            window,
            cx,
            super::act(&weak, |this, _, cx| this.reveal_rendered(cx)),
        );
        let delete = icon_btn(
            "dmp-rendered-delete",
            "fas fa-trash-alt",
            size,
            self.t("#L_DEMO_RENDERED_DELETE_HINT", &[]),
            self.s.rendered.deleting,
            true,
            window,
            cx,
            super::act(&weak, |this, _, _| this.ask_delete()),
        );
        let head = div()
            .flex()
            .items_center()
            .gap(u(10.))
            .min_w_0()
            .child(
                div()
                    .id("dmp-rendered-title")
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_px(16., 22.)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(text)
                    .tip(name.clone())
                    .child(name),
            )
            .child(
                div()
                    .ml_auto()
                    .flex_none()
                    .child(crate::snap::target("dmp-rendered-export", export)),
            )
            .child(crate::snap::target("dmp-rendered-reveal", reveal))
            .child(crate::snap::target("dmp-rendered-delete", delete));
        let no_source = (!source).then(|| {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(u(8.))
                .mt_auto()
                .child(
                    div()
                        .text_size(u(12.))
                        .text_color(theme.color("l-text-faint"))
                        .child(self.t("#L_DEMO_RENDERED_NO_SOURCE", &[])),
                )
        });
        let info = ScrollArea::new("dmp-info", &self.info_scroll)
            .bar(Bar::Fields)
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .min_h_full()
                    .flex()
                    .flex_col()
                    .gap(u(if low { 6. } else { 8. }))
                    .child(head)
                    .child(facts(self.s.rendered_facts(&t), low, &theme))
                    .children(no_source),
            );
        let markers: Vec<Value> = open
            .get("markers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let title = side_title(
            self.t("#L_DEMO_MARKS_TITLE", &[]),
            Some(count(markers.len(), &theme)),
            &theme,
        );
        let body = if markers.is_empty() {
            div()
                .text_size(u(12.5))
                .text_color(theme.color("l-text-faint"))
                .child(self.t("#L_DEMO_MARKS_EMPTY", &[]))
                .into_any_element()
        } else {
            let mut rows = div().flex().flex_col();
            for (index, marker) in markers.iter().enumerate() {
                let kind = marker
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let time = marker.get("t").and_then(Value::as_f64).unwrap_or(0.);
                let on_click = super::act(&weak, move |this, _, cx| this.jump_rendered(time, cx));
                rows = rows.child(marker_row(
                    SharedString::from(format!("dmp-rmark-{index}")),
                    marker_color(kind, &theme),
                    marker_text(marker, &t),
                    clock(time),
                    window,
                    cx,
                    on_click,
                ));
            }
            rows.into_any_element()
        };
        let side = self.side_frame(title, body, &theme);
        self.card_shell(low, info.into_any_element(), side, &theme)
    }
}
