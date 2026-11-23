//! Вкладка «Профиль» (`.profile__overview` 0.4.9): две равные колонки — слева «Сводка» (плитки
//! по две в ряд; серия входов, «больше всего» и последняя нечётная — во всю ширину), справа
//! закреплённый забег (по содержимому, место 1–3 подсвечивает карточку цветом лямбды) и ссылки
//! одним рядом.

use super::ProfileContext;
use super::modal::ProfileModal;
use crate::icons::fa_glyph;
use crate::kit::button::white;
use crate::layer::TipExt;
use crate::motion::{Timing, hover, mix};
use crate::paint::{Radial, ring};
use crate::scale::{u, zoom};
use crate::scroll::{Bar, Scroll, ScrollArea};
use crate::text::{TextExt, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, FontWeight, Hsla, IntoElement, Render, Role, WeakEntity, Window, div,
    prelude::*, rgb, svg,
};
use hlsr_ui::controllers::profile::{LinksView, RunView, StatView};

/// Лямбда места (`RunPlace.vue`, viewBox 0 0 177 186).
pub(super) const LAMBDA: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 177 186'>\
<path d='M47.2 12.9v25.9h21.4l8.7 27.1-65.1 101h32l46.5-68.6 30.5 77 48-15.3-7.6-23.6-22.1 \
6.9-51.8-130.4z'/></svg>";

/// Что рисует вкладка (клон на каждый кадр перехода).
#[derive(Clone)]
pub(super) struct Overview {
    pub stats: Vec<StatView>,
    pub run: Option<RunView>,
    pub own: bool,
    pub links: Option<LinksView>,
    pub stats_title: String,
    pub record_title: String,
    pub links_title: String,
    pub no_record: String,
    pub no_record_own: String,
    pub open_run: String,
}

/// `.profile__section-title`: 11/16, 700, разрядка .12em, заглавные, `--l-text-faint`.
pub(super) fn section_title(text: &str, theme: &Theme) -> impl IntoElement {
    div().mb(u(12.)).child(spaced(
        text.to_uppercase(),
        11.,
        16.,
        FontWeight::BOLD,
        0.12,
        theme.color("l-text-faint"),
    ))
}

/// Цвет места 1–3 (`--medal`).
pub(super) fn medal_color(medal: &str) -> Option<Hsla> {
    match medal {
        "gold" => Some(rgb(0xf5c542).into()),
        "silver" => Some(rgb(0xcfd8e3).into()),
        "bronze" => Some(rgb(0xd98c4a).into()),
        _ => None,
    }
}

/// Плитка сводки: значок 32 в `--l-accent-soft`, значение 16/22 700, подпись 12/16.
fn stat(row: &StatView, theme: &Theme, zoom: f32) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap(u(12.))
        .py(u(14.))
        .px(u(16.))
        .rounded(theme.rem("l-radius-sm"))
        .bg(white(0.03))
        .shadow(vec![ring(zoom, 1., theme.color("l-border"))])
        .child(
            div()
                .flex_none()
                .size(u(32.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme.rem("l-radius-sm"))
                .bg(theme.color("l-accent-soft"))
                .text_color(theme.color("l-accent"))
                .child(fa_glyph(row.icon, 13.)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_px(16., 22.)
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.color("l-text"))
                        .tnum()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(row.value.clone()),
                )
                .child(
                    div()
                        .text_px(12., 16.)
                        .text_color(theme.color("l-text-faint"))
                        .child(row.label.clone()),
                ),
        )
}

/// Сетка сводки: две колонки по 8 px; широкие и последняя нечётная — во всю ширину.
fn stats(rows: &[StatView], theme: &Theme, zoom: f32) -> impl IntoElement {
    let mut grid = div().flex().flex_col().gap(u(8.));
    let mut pending: Option<&StatView> = None;
    for row in rows {
        if row.wide {
            if let Some(single) = pending.take() {
                grid = grid.child(div().flex().child(stat(single, theme, zoom)));
            }
            grid = grid.child(div().flex().child(stat(row, theme, zoom)));
            continue;
        }
        match pending.take() {
            Some(first) => {
                grid = grid.child(
                    div()
                        .flex()
                        .gap(u(8.))
                        .child(stat(first, theme, zoom))
                        .child(stat(row, theme, zoom)),
                )
            }
            None => pending = Some(row),
        }
    }
    if let Some(single) = pending {
        grid = grid.child(div().flex().child(stat(single, theme, zoom)));
    }
    grid
}

/// Карточка закреплённого забега или пустая карточка.
fn record(page: &Overview, theme: &Theme, zoom: f32, this: WeakEntity<ProfileModal>) -> AnyElement {
    let (text, dim, faint, border) = (
        theme.color("l-text"),
        theme.color("l-text-dim"),
        theme.color("l-text-faint"),
        theme.color("l-border"),
    );
    let radius = theme.rem("l-radius");
    let radius_px = theme.len("l-radius");
    let Some(run) = page.run.clone() else {
        return div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(u(4.))
            .p(u(20.))
            .rounded(radius)
            .bg(white(0.02))
            .shadow(vec![ring(zoom, 1., border)])
            .child(
                div()
                    .mb(u(8.))
                    .text_color(faint)
                    .child(fa_glyph("fas fa-stopwatch", 22.)),
            )
            .child(
                div()
                    .text_px(14., 20.)
                    .font_weight(FontWeight::BOLD)
                    .text_color(text)
                    .child(page.no_record.clone()),
            )
            .when(page.own, |el| {
                el.child(
                    div()
                        .max_w(u(340.))
                        .text_center()
                        .text_px(12.5, 20.)
                        .text_color(dim)
                        .child(page.no_record_own.clone()),
                )
            })
            .into_any_element();
    };
    let tint = medal_color(run.medal).unwrap_or_else(|| theme.color("l-accent"));
    let title = page.open_run.clone();
    let url = run.url.clone();
    let place = (run.place > 0).then(|| {
        let color = medal_color(run.medal).unwrap_or(dim);
        div()
            .flex()
            .items_center()
            .gap(u(8.))
            .text_px(13., 20.)
            .font_weight(FontWeight::BOLD)
            .text_color(color)
            .tnum()
            .whitespace_nowrap()
            .when(!run.medal.is_empty(), |el| {
                el.child(svg().data(LAMBDA.as_bytes()).size(u(20.)).text_color(color))
            })
            .child(run.place_label.clone())
    });
    hover("profile-record", Timing::fast(), move |t| {
        div()
            .id("profile-record-card")
            .role(Role::Button)
            .button_text()
            .aria_label(title.clone())
            .tip(title)
            .relative()
            .flex()
            .flex_col()
            .gap(u(4.))
            .p(u(20.))
            .rounded(radius)
            .overflow_hidden()
            .bg(white(0.03 + 0.02 * t))
            .shadow(vec![ring(zoom, 1., mix(border, tint, 0.22 + 0.23 * t))])
            .cursor_pointer()
            .on_click(move |_, _, cx| {
                this.update(cx, |this, cx| {
                    let effects = this.state.borrow().open_link(&url);
                    this.run(effects, cx);
                })
                .ok();
            })
            // radial-gradient(120% 140% at 100% 0%, tint 14%, transparent 60%).
            .child(
                Radial {
                    center: (1., 0.),
                    radius: (1.2, 1.4),
                    stops: vec![(0., 0.14), (0.6, 0.)],
                }
                .paint_rounded(1., 1., radius_px, tint)
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_px(14., 20.)
                            .font_weight(FontWeight::BOLD)
                            .text_color(text)
                            .child(run.game.clone()),
                    )
                    .child(
                        div()
                            .text_color(mix(faint, text, t))
                            .child(fa_glyph("fas fa-external-link-alt", 11.)),
                    ),
            )
            .child(
                div()
                    .text_px(12.5, 20.)
                    .text_color(dim)
                    .child(run.category.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(u(16.))
                    .pt(u(12.))
                    .child(
                        div()
                            .text_px(30., 36.)
                            .font_weight(FontWeight::BOLD)
                            .text_color(text)
                            .tnum()
                            .child(run.time.clone()),
                    )
                    .children(place),
            )
    })
    .into_any_element()
}

/// Ссылки одним рядом: Discord (без перехода), YouTube и Twitch (кнопки).
fn links(
    links: &LinksView,
    theme: &Theme,
    zoom: f32,
    this: WeakEntity<ProfileModal>,
) -> impl IntoElement {
    let (text, dim, border, accent) = (
        theme.color("l-text"),
        theme.color("l-text-dim"),
        theme.color("l-border"),
        theme.color("l-accent"),
    );
    let radius = theme.rem("l-radius-sm");
    let item = move |id: &'static str,
                     glyph: &'static str,
                     color: u32,
                     name: String,
                     url: Option<String>,
                     this: WeakEntity<ProfileModal>| {
        let button = url.is_some();
        hover(id, Timing::fast(), move |t| {
            let t = if button { t } else { 0. };
            div()
                .id(id)
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(u(10.))
                .min_h(u(44.))
                .px(u(14.))
                .rounded(radius)
                .bg(white(0.03 + 0.03 * t))
                .shadow(vec![ring(zoom, 1., mix(border, accent, 0.35 * t))])
                .text_px(13., 20.)
                .text_color(mix(dim, text, t))
                .child(
                    div()
                        .flex_none()
                        .w(u(20.))
                        .flex()
                        .justify_center()
                        .text_color(rgb(color))
                        .child(fa_glyph(glyph, 16.)),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(name),
                )
                .when_some(url, |el, url| {
                    el.role(Role::Button)
                        .button_text()
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            this.update(cx, |this, cx| {
                                let effects = this.state.borrow().open_link(&url);
                                this.run(effects, cx);
                            })
                            .ok();
                        })
                })
        })
    };
    div()
        .flex()
        .gap(u(8.))
        .when(!links.discord.is_empty(), |el| {
            el.child(item(
                "profile-link-discord",
                "fab fa-discord",
                0x8c9eff,
                links.discord.clone(),
                None,
                this.clone(),
            ))
        })
        .when(!links.youtube.is_empty(), |el| {
            el.child(item(
                "profile-link-youtube",
                "fab fa-youtube",
                0xff5252,
                links.youtube_label.clone(),
                Some(links.youtube.clone()),
                this.clone(),
            ))
        })
        .when(!links.twitch.is_empty(), |el| {
            el.child(item(
                "profile-link-twitch",
                "fab fa-twitch",
                0xb18cff,
                links.twitch_label.clone(),
                Some(links.twitch.clone()),
                this.clone(),
            ))
        })
}

/// Вкладка «Профиль» отдельным закэшированным видом: бегущий неон и анимированный ник/рамка
/// шапки не перестраивают сводку, карточку рекорда и ссылки 240 раз в секунду.
pub(super) struct OverviewTab {
    scroll: Scroll,
}

impl OverviewTab {
    pub fn new() -> OverviewTab {
        OverviewTab {
            scroll: Scroll::new(),
        }
    }

    pub fn reset_scroll(&mut self) {
        self.scroll = Scroll::new();
    }
}

impl Render for OverviewTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let Some(context) = ProfileContext::get(cx) else {
            return div().into_any_element();
        };
        let Some(modal_entity) = context.modal.upgrade() else {
            return div().into_any_element();
        };
        let (view, tr) = {
            let modal = modal_entity.read(cx);
            let view = modal
                .view_snapshot()
                .unwrap_or_else(|| {
                    std::rc::Rc::new(
                        context
                            .state
                            .borrow()
                            .view(modal.language, &modal.wall, &modal.tr),
                    )
                });
            (view, modal.tr.clone())
        };
        let t = |key: &str| tr(key, &[]);
        let page = Overview {
            stats: view.stats.clone(),
            run: view.run.clone(),
            own: view.record_empty_own,
            links: view.links.clone(),
            stats_title: t("#L_PROFILE_STATS"),
            record_title: t("#L_PROFILE_RECORD"),
            links_title: t("#L_PROFILE_LINKS"),
            no_record: t("#L_PROFILE_NO_RECORD"),
            no_record_own: t("#L_PROFILE_NO_RECORD_OWN"),
            open_run: t("#L_PROFILE_OPEN_RUN"),
        };
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        crate::snap::target(
            "profile-body",
            ScrollArea::new(("profile-scroll", 0usize), &self.scroll)
                .bar(Bar::Thin)
                .size_full()
                .child(
                    div()
                        .pt(u(28.))
                        .px(u(32.))
                        .pb(u(32.))
                        .child(overview(&page, &theme, zoom, modal_entity.downgrade())),
                ),
        )
        .into_any_element()
    }
}

/// Вкладка «Профиль» целиком.
// shortcut: одна колонка при окне ≤ 860 CSS px (`@media (max-width: 860px)`) не сделана — окно лаунчера шире 1152.
pub(super) fn overview(
    page: &Overview,
    theme: &Theme,
    zoom: f32,
    this: WeakEntity<ProfileModal>,
) -> AnyElement {
    let links = page.links.as_ref().map(|view| {
        div()
            .mt(u(24.))
            .child(section_title(&page.links_title, theme))
            .child(links(view, theme, zoom, this.clone()))
    });
    div()
        .flex()
        .items_start()
        .gap(u(32.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(section_title(&page.stats_title, theme))
                .child(stats(&page.stats, theme, zoom)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(section_title(&page.record_title, theme))
                .child(record(page, theme, zoom, this.clone()))
                .children(links),
        )
        .into_any_element()
}
