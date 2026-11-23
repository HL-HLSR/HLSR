//! Тело страницы «Демо»: список слева (`.dmp__list`: группы по играм, записи; во вкладке
//! «Отрендеренные» — видео папки вывода) и справа плеер с карточкой (`.dmp__main`).

use super::count;
use super::panel::DemoPanel;
use crate::icons::{fa_glyph, icon, svg_icon};
use crate::kit::button::white;
use crate::layer::TipExt;
use crate::motion::{Timing, hover, mix};
use crate::paint::ring;
use crate::rows::Slot;
use crate::scale::{u, zoom};
use crate::scroll::{Bar, ScrollArea};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, Div, FontWeight, Hsla, IntoElement, Role, SharedString, Window, div,
    prelude::*, relative,
};
use hlsr_ui::controllers::demo_panel::DemoPanel as State;
use serde_json::Value;

/// Логотип группы маской (`launcherData.gameArt[…].logo`).
fn group_logo(id: &str) -> Option<&'static str> {
    match id {
        "hl1-won" => Some("logos/half-life.svg"),
        "ghosting-renewed" => Some("logos/ghosting.svg"),
        _ => None,
    }
}

/// Значок вида записи (`kindIcon`).
fn kind_icon(item: &Value) -> &'static str {
    match item.get("kind").and_then(Value::as_str) {
        Some("playlist") => "fas fa-list-ol",
        Some("segments") => "fas fa-layer-group",
        _ => "fas fa-film",
    }
}

/// Пусто или загрузка (`.dmp__empty`): значок 28, заголовок 14, текст 13.
fn empty(theme: &Theme, glyph: AnyElement, title: Option<String>, text: Option<String>) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(u(8.))
        .my_auto()
        .px(u(16.))
        .py(u(24.))
        .text_size(u(13.))
        .line_height(relative(1.45))
        .text_color(theme.color("l-text-dim"))
        .text_center()
        .child(div().mb(u(4.)).child(glyph))
        .when_some(title, |el, title| {
            el.child(
                div()
                    .text_size(u(14.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.color("l-text"))
                    .child(title),
            )
        })
        .when_some(text, |el, text| el.child(div().child(text)))
}

/// Строка записи (`.dmp__item`): значок, имя, мета, дата, «не поддерживается».
struct Row {
    id: SharedString,
    icon: &'static str,
    name: String,
    busy: bool,
    meta: String,
    date: String,
    /// Перенос строки даты (`is-wrap`) у записей «Демок».
    wrap: bool,
    badge: Option<(String, String)>,
    active: bool,
    dim_name: bool,
}

fn row(
    row: Row,
    theme: &Theme,
    window: &mut Window,
    cx: &mut gpui::App,
    on_click: super::OnClick,
) -> AnyElement {
    let zoom = zoom(window);
    let Row {
        id,
        icon: glyph,
        name,
        busy,
        meta,
        date,
        wrap,
        badge,
        active,
        dim_name,
    } = row;
    let (dim, text, faint, accent) = (
        theme.color("l-text-dim"),
        theme.color("l-text"),
        theme.color("l-text-faint"),
        theme.color("accent"),
    );
    let (soft, border, warning) = (
        theme.color("accent-soft"),
        theme.color("accent-border"),
        theme.color("l-level-warning"),
    );
    let super::Focus {
        handle,
        ring: focus,
    } = super::focus(&id.clone().into(), 8., -2., window, cx);
    let inner = id.clone();
    let target = id.clone();
    let el = hover(id, Timing::fast(), move |t| {
        let color = if active { text } else { mix(dim, text, t) };
        let line = |text: String, color: Hsla| {
            div()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_px(12., 16.)
                .tnum()
                .text_color(color)
                .child(text)
        };
        let badge = badge.map(|(label, hint)| {
            div()
                .id("badge")
                .self_start()
                .flex()
                .items_center()
                .gap(u(6.))
                .mt(u(3.))
                .px(u(8.))
                .py(u(2.))
                .rounded(u(9.))
                .bg(warning.opacity(0.14))
                .text_color(warning)
                .text_px(11., 16.)
                .font_weight(FontWeight::SEMIBOLD)
                .tip(hint)
                .child(fa_glyph("fas fa-ban", 10.))
                .child(label)
        });
        div()
            .id(inner)
            .role(Role::Button)
            .button_text()
            .relative()
            .track_focus(&handle)
            .flex()
            .items_start()
            .gap(u(10.))
            .w_full()
            .py(u(9.))
            .px(u(10.))
            .rounded(u(8.))
            .cursor_pointer()
            .text_color(color)
            .map(|el| {
                if active {
                    el.bg(soft).shadow(vec![ring(zoom, 1., border)])
                } else {
                    el.bg(white(0.06 * t))
                }
            })
            .child(
                div()
                    .flex_none()
                    .w(u(16.))
                    .mt(u(2.))
                    .flex()
                    .justify_center()
                    .text_color(if active { accent } else { color })
                    .child(fa_glyph(glyph, 13.)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(u(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_px(13., 18.)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if dim_name { dim } else { text })
                            .when(busy, |el| {
                                el.child(div().mr(u(6.)).child(icon(
                                    "fas fa-spinner fa-spin",
                                    11.,
                                    accent,
                                )))
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(name),
                            ),
                    )
                    .child(line(meta, color))
                    .child(if wrap {
                        div().text_px(12., 16.).tnum().text_color(faint).child(date)
                    } else {
                        line(date, faint)
                    })
                    .children(badge),
            )
            .children(focus)
            .on_click(move |event, window, cx| on_click(event, window, cx))
    });
    crate::snap::target(target, el)
}

impl DemoPanel {
    /// Колонка списка: `clamp(260px, 24%, 340px)`, подложка `--l-modal-nav`, полоса GX.
    fn list_column(&self, scroll: &crate::scroll::Scroll, theme: &Theme, content: Div) -> Div {
        div()
            .flex_none()
            .w(relative(0.24))
            .min_w(u(260.))
            .max_w(u(340.))
            .h_full()
            .min_h_0()
            .border_1()
            .border_color(theme.color("l-border"))
            .rounded(theme.rem("l-radius-sm"))
            .bg(theme.color("l-modal-nav"))
            .overflow_hidden()
            .child(
                ScrollArea::new("dmp-list-scroll", scroll)
                    .bar(Bar::Fields)
                    .contain()
                    .size_full()
                    .child(
                        content
                            .min_h_full()
                            .flex()
                            .flex_col()
                            .gap(u(14.))
                            .py(u(10.))
                            .px(u(8.)),
                    ),
            )
    }

    /// Вкладка «Демки»: список по играм и плеер с карточкой.
    pub(super) fn demos_body(
        &mut self,
        low: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let language = self.language;
        let t = self.translate();
        let s = &self.s;
        let content = if s.loading && s.items().is_empty() {
            div().child(empty(
                &theme,
                icon("fas fa-spinner fa-spin", 28., theme.color("l-text-faint")),
                None,
                Some(self.t("#L_DEMO_LOADING", &[])),
            ))
        } else if s.items().is_empty() {
            div().child(empty(
                &theme,
                fa_glyph("fas fa-film", 28.)
                    .text_color(theme.color("l-text-faint"))
                    .into_any_element(),
                Some(self.t("#L_DEMO_EMPTY_TITLE", &[])),
                Some(self.t("#L_DEMO_EMPTY_TEXT", &[])),
            ))
        } else {
            // Группы — плоской колонкой (заголовок, записи), чтобы окно видимости
            // (`crate::rows`) строило только записи у видимой области. Промежутки — полями
            // внутри строк: окно меряет их рамки (14 между группами, 2 внутри группы).
            enum Entry {
                Title(usize),
                Item(usize, usize),
            }
            let groups = s.groups();
            let mut entries = Vec::new();
            let mut keys = Vec::new();
            for (gi, group) in groups.iter().enumerate() {
                entries.push(Entry::Title(gi));
                keys.push(format!("g:{}", group.id));
                for (ii, item) in group.items.iter().enumerate() {
                    entries.push(Entry::Item(gi, ii));
                    let id = item
                        .get("id")
                        .map(hlsr_core::js::to_string)
                        .unwrap_or_default();
                    keys.push(format!("i:{gi}:{id}"));
                }
            }
            let slots = self.list_rows.plan(&self.list_scroll, keys, 0., window, cx);
            let mut list = div().flex().flex_col();
            for slot in slots {
                let index = match slot {
                    Slot::Row(index) => index,
                    Slot::Space(height) => {
                        list = list.child(div().flex_none().h(height));
                        continue;
                    }
                };
                match entries[index] {
                    Entry::Title(gi) => {
                        let group = &groups[gi];
                        let name = match (group.id.as_str(), group.name) {
                            ("manual", _) => self.t("#L_DEMO_GROUP_MANUAL", &[]),
                            (_, Some(name)) => name.to_owned(),
                            _ => self.t("#L_DEMO_GROUP_OTHER", &[]),
                        };
                        let dim = theme.color("l-text-dim");
                        let lead = match group_logo(&group.id) {
                            Some(path) => Some(svg_icon(path, 16., dim).into_any_element()),
                            None if group.id == "manual" => Some(
                                fa_glyph("fas fa-folder-plus", 12.)
                                    .text_color(dim)
                                    .into_any_element(),
                            ),
                            None => None,
                        };
                        let title = div()
                            .flex()
                            .items_center()
                            .gap(u(6.))
                            .mb(u(4.))
                            .px(u(8.))
                            .text_color(dim)
                            .children(lead)
                            .child(crate::text::spaced(
                                name.to_uppercase(),
                                11.5,
                                20.,
                                FontWeight::SEMIBOLD,
                                0.05,
                                dim,
                            ))
                            .child(count(group.items.len(), &theme));
                        list = list.child(
                            div()
                                .flex_none()
                                .when(gi > 0, |el| el.pt(u(14.)))
                                .when(!group.items.is_empty(), |el| el.pb(u(2.)))
                                .child(title),
                        );
                    }
                    Entry::Item(gi, ii) => {
                        let group = &groups[gi];
                        let item = &group.items[ii];
                        let id = item
                            .get("id")
                            .map(hlsr_core::js::to_string)
                            .unwrap_or_default();
                        let row_id = SharedString::from(format!("dmp-item-{id}"));
                        if crate::kit::button::focus_handle(
                            &gpui::ElementId::Name(row_id.clone()),
                            window,
                            cx,
                        )
                        .is_focused(window)
                        {
                            self.list_rows.focus(index);
                        }
                        let supported = item.get("supported").is_some_and(hlsr_core::js::truthy);
                        let badge = (!supported).then(|| {
                            let unavailable = item.get("reason").and_then(Value::as_str)
                                == Some("DEMO_NOT_FOUND");
                            (
                                self.t(
                                    if unavailable {
                                        "#L_DEMO_UNAVAILABLE"
                                    } else {
                                        "#L_DEMO_UNSUPPORTED"
                                    },
                                    &[],
                                ),
                                State::reason_text(item, &t),
                            )
                        });
                        let on_click = {
                            let id = id.clone();
                            self.act(cx, move |this, _, cx| this.select(&id, cx))
                        };
                        let row = row(
                            Row {
                                id: row_id,
                                icon: kind_icon(item),
                                name: State::item_name(item, language, &t),
                                busy: s.exporting(&id),
                                meta: State::item_meta(item),
                                date: State::item_date(item, &t),
                                wrap: true,
                                badge,
                                active: s.selected_id.as_deref() == Some(id.as_str()),
                                dim_name: !supported,
                            },
                            &theme,
                            window,
                            cx,
                            on_click,
                        );
                        let last = ii + 1 == group.items.len();
                        list =
                            list.child(div().flex_none().when(!last, |el| el.pb(u(2.))).child(row));
                    }
                }
            }
            div().child(self.list_rows.measure(&self.list_scroll, list))
        };
        let list = self.list_column(&self.list_scroll, &theme, content);
        let main = self.main_column(false, low, window, cx);
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .gap(u(16.))
            .child(list)
            .child(main)
            .into_any_element()
    }

    /// Вкладка «Отрендеренные»: видео папки вывода.
    pub(super) fn rendered_body(
        &mut self,
        low: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let t = self.translate();
        let rendered = &self.s.rendered;
        let content = if rendered.loading && rendered.items.is_empty() {
            div().child(empty(
                &theme,
                icon("fas fa-spinner fa-spin", 28., theme.color("l-text-faint")),
                None,
                None,
            ))
        } else if rendered.items.is_empty() {
            let path = div()
                .max_w_full()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_size(u(12.))
                .text_color(theme.color("l-text-faint"))
                .child(rendered.output_dir.clone());
            div().child(
                empty(
                    &theme,
                    fa_glyph("fas fa-photo-video", 28.)
                        .text_color(theme.color("l-text-faint"))
                        .into_any_element(),
                    Some(self.t("#L_DEMO_RENDERED_EMPTY_TITLE", &[])),
                    Some(self.t("#L_DEMO_RENDERED_EMPTY_TEXT", &[])),
                )
                .max_w_full()
                .child(path),
            )
        } else {
            let open = self.s.open_name();
            let keys = rendered
                .items
                .iter()
                .enumerate()
                .map(|(index, video)| {
                    let name = video
                        .get("name")
                        .map(hlsr_core::js::to_string)
                        .unwrap_or_default();
                    format!("{index}:{name}")
                })
                .collect();
            let slots = self
                .rendered_rows
                .plan(&self.rendered_scroll, keys, 2., window, cx);
            let mut section = div().flex().flex_col().gap(u(2.));
            for slot in slots {
                let index = match slot {
                    Slot::Row(index) => index,
                    Slot::Space(height) => {
                        section = section.child(div().flex_none().h(height));
                        continue;
                    }
                };
                let video = &rendered.items[index];
                let name = video
                    .get("name")
                    .map(hlsr_core::js::to_string)
                    .unwrap_or_default();
                let row_id = SharedString::from(format!("dmp-video-{index}"));
                if crate::kit::button::focus_handle(
                    &gpui::ElementId::Name(row_id.clone()),
                    window,
                    cx,
                )
                .is_focused(window)
                {
                    self.rendered_rows.focus(index);
                }
                let on_click = {
                    let name = name.clone();
                    self.act(cx, move |this, _, cx| this.open_rendered(name.clone(), cx))
                };
                section = section.child(row(
                    Row {
                        id: row_id,
                        icon: "fas fa-file-video",
                        active: open.as_deref() == Some(name.as_str()),
                        name,
                        busy: false,
                        meta: State::rendered_source(video),
                        date: State::rendered_line(video, &t),
                        wrap: false,
                        badge: None,
                        dim_name: false,
                    },
                    &theme,
                    window,
                    cx,
                    on_click,
                ));
            }
            div().child(self.rendered_rows.measure(&self.rendered_scroll, section))
        };
        let list = self.list_column(&self.rendered_scroll, &theme, content);
        let main = self.main_column(true, low, window, cx);
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .gap(u(16.))
            .child(list)
            .child(main)
            .into_any_element()
    }

    /// `.dmp__main`: плеер (не меньше 60 % высоты), кнопка свернуть карточку, карточка.
    fn main_column(
        &mut self,
        rendered: bool,
        low: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let player = if rendered {
            self.rendered_player.clone()
        } else {
            self.player.clone()
        };
        let fold = self.fold(window, cx);
        let card = (!self.s.card_hidden).then(|| {
            if rendered {
                self.rendered_card(low, window, cx)
            } else {
                self.demo_card(low, window, cx)
            }
        });
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(u(2.))
            .child(
                div()
                    .flex_grow(1.)
                    .flex_shrink(1.)
                    .min_h(relative(0.6))
                    .flex()
                    .flex_col()
                    .child(player),
            )
            .child(fold)
            .children(card)
            .into_any_element()
    }
}
