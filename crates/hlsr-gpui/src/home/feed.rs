//! Страница «Лента» (`#home-page-feed` HomePanel 0.4.9): свой список с прокруткой (полоса 6 px,
//! `overscroll-behavior: contain`), посты [`super::news_post`] через 12 px, внизу поле 32;
//! пусто — «Загрузка…» до первого ответа, потом «Новостей пока нет». Строятся только посты у
//! видимой области ([`crate::rows`]).

use super::news_post;
use super::panel::HomePanel;
use crate::rows::Slot;
use crate::scale::u;
use crate::scroll::{Bar, ScrollArea};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{AnyElement, Context, IntoElement, ParentElement, Styled, Window, div};

pub(super) fn page(
    panel: &mut HomePanel,
    window: &mut Window,
    cx: &mut Context<HomePanel>,
) -> AnyElement {
    let view = panel.frame_view(cx);
    let theme = cx.global::<Theme>();
    let (faint, shade) = (theme.color("l-text-faint"), theme.color("l-shade-rgb"));
    let rows = panel.feed_rows.clone();
    let keys = view.posts.iter().map(|post| post.id.clone()).collect();
    let slots = rows.plan(&panel.feed, keys, 12., window, cx);
    let posts: Vec<AnyElement> = slots
        .into_iter()
        .map(|slot| match slot {
            Slot::Row(index) => news_post::render(panel, index, &view.posts[index], window, cx),
            Slot::Space(height) => div().flex_none().h(height).into_any_element(),
        })
        .collect();
    let empty = (!view.empty_posts.is_empty()).then(|| {
        div()
            .text_px(14., 20.)
            .text_color(faint)
            .child(view.empty_posts.clone())
    });
    // shortcut: `mask-image` краёв — заливкой цвета фона. На «Главной» кадра игры нет, фон —
    // `--l-bg` под затемнением, у краёв списка почти `--l-shade-rgb`; нужен кадр — маска.
    let list = ScrollArea::new("home-posts", &panel.feed)
        .bar(Bar::List)
        .contain()
        .fade(shade)
        .flex_1()
        .min_h_0()
        // `.home-panel__list`: поле справа 4 и `margin-right: -4px` — полоса у края колонки.
        .mr(u(-4.))
        .child(
            rows.measure(
                &panel.feed,
                div()
                    .flex()
                    .flex_col()
                    .gap(u(12.))
                    .pr(u(4.))
                    .pb(u(32.))
                    .children(posts)
                    .when_some(empty, |el, empty| el.child(empty)),
            ),
        );
    div()
        .size_full()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(u(16.))
        .child(list)
        .into_any_element()
}
