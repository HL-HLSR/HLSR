//! Новость ленты (`NewsPost.vue` 0.4.9): автор (с профилем HLSR — кнопка с аватаром в рамке и
//! ником со значком подписчика, открывает профиль; без профиля — имя), дата, заголовок, тело —
//! Markdown (`news-markdown.js` → [`hlsr_ui::markdown::news`] → `StyledText`/`InteractiveText`),
//! плитки скриншотов (раскладка `news-image-layout`, щелчок — просмотр), реакции с оптимистичным
//! счётом, «Опубликовано …», замечание («Войдите…», «Слишком часто», ссылка не открылась).
//!
//! Поля блоков схлопываются, как в CSS (заголовок → первый абзац, последний абзац → картинки).

use super::panel::{HomePanel, SCOPE, open_profile};
use crate::common::{avatar_frame, nick_name};
use crate::image::device_size;
use crate::kit::button::{focus_handle, focus_visible, white};
use crate::layer::TipExt;
use crate::motion::{Timing, hover, mix};
use crate::paint::ring;
use crate::scale::{u, viewport, zoom, zpx};
use crate::text::{MONO, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, Context, ElementId, FontStyle, FontWeight, HighlightStyle, Hsla,
    InteractiveText, IntoElement, ObjectFit, ParentElement, Role, SharedString, Styled, StyledText,
    UnderlineStyle, Window, div, img, prelude::*, px, relative, rgb,
};
use hlsr_ui::controllers::home::{PostView, ReactionView, Tile};
use hlsr_ui::images::{Fit, ImageRequest, ImageSource};
use hlsr_ui::richtext::{Block, Doc, Inline, Link, List, Role as Tone, Sheet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

/// Таблица `.news-post__body`.
const SHEET: Sheet = Sheet::NEWS;

fn id(post: &str, part: &str) -> ElementId {
    ElementId::Name(format!("news-{post}-{part}").into())
}

fn id_n(post: &str, part: &str, n: usize) -> ElementId {
    ElementId::named_usize(format!("news-{post}-{part}"), n)
}

/// Карточка поста `index` ленты.
pub(super) fn render(
    panel: &mut HomePanel,
    index: usize,
    post: &PostView,
    window: &mut Window,
    cx: &mut Context<HomePanel>,
) -> AnyElement {
    let theme = cx.global::<Theme>().clone();
    let zoom = zoom(window);
    let tr = HomePanel::tr(cx);
    let (text, faint) = (theme.color("l-text"), theme.color("l-text-faint"));
    let solid = theme.color("l-surface-solid");
    // @media (max-width: 1000px) { .news-post { padding: 20px } }
    let (pad_y, pad_x) = if viewport(window).0 <= 1000. {
        (20., 20.)
    } else {
        (24., 28.)
    };

    let author = author(panel, index, post, window, cx);
    let meta = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(u(8.))
        .text_px(12., 18.)
        .text_color(faint)
        .child(author)
        .child("·")
        .child(post.date.clone());

    let doc = panel.doc(&post.id, &post.body);
    let (body, first, last) = body(&doc, &post.id, window, cx);
    let title = div()
        .mt(u(8.))
        .mb(u(first.max(16.)))
        .text_px(22., 30.)
        .font_weight(FontWeight::BOLD)
        .text_color(text)
        .child(post.title.clone());

    let has_images = !post.tiles.is_empty();
    let images = has_images.then(|| images(panel, index, post, last.max(16.), window, cx));
    let after_body = if has_images { 20. } else { last.max(20.) };
    let reactions = reactions(index, post, after_body, &theme, cx);

    let stamp = div()
        .mt(u(12.))
        .text_px(11.5, 16.)
        .text_color(text)
        .opacity(0.38)
        .child(post.stamp.clone());

    let notice = (!post.notice.is_empty()).then(|| {
        let sign_in = post.notice_sign_in.then(|| {
            let accent = theme.color("l-accent");
            div()
                .id(id(&post.id, "sign-in"))
                .ml(u(8.))
                .text_color(accent)
                .underline()
                .cursor_pointer()
                .child(tr("#L_HOME_SIGN_IN", &[]))
                .button_text()
                .on_click(|_, _, cx| open_profile("", cx))
        });
        div()
            .mt(u(12.))
            .flex()
            .flex_wrap()
            .items_center()
            .text_px(12., 20.)
            .text_color(theme.color("l-text-dim"))
            .child(post.notice.clone())
            .children(sign_in)
    });

    div()
        .id(id(&post.id, "post"))
        .flex()
        .flex_col()
        .flex_none()
        .px(u(pad_x))
        .py(u(pad_y))
        .rounded(theme.rem("l-radius"))
        .bg(solid)
        .shadow(vec![ring(zoom, 1., theme.color("l-border"))])
        .child(meta)
        .child(title)
        .child(body)
        .children(images)
        .child(reactions)
        .child(stamp)
        .children(notice)
        .into_any_element()
}

/// Автор: с профилем HLSR — кнопка (аватар 20 px в рамке шириной 2, ник с оформлением и
/// значком подписчика, подсказка «Открыть профиль»); иначе — имя.
fn author(
    panel: &mut HomePanel,
    index: usize,
    post: &PostView,
    window: &mut Window,
    cx: &mut Context<HomePanel>,
) -> AnyElement {
    let theme = cx.global::<Theme>().clone();
    let (dim, text) = (theme.color("l-text-dim"), theme.color("l-text"));
    if post.author_id.is_empty() {
        return div()
            .font_weight(FontWeight::BOLD)
            .text_color(dim)
            .child(post.name.clone())
            .into_any_element();
    }
    let solid = theme.color("l-surface-solid");
    let accent = theme.color("l-accent");
    let zoom = zoom(window);
    let photo = ImageSource::parse(&post.avatar).and_then(|source| {
        let (w, h) = device_size(window, 20., 20.);
        let request = ImageRequest::new(source, SCOPE).with_target(w, h, Fit::Cover);
        panel.pictures.get(request, window, cx).and_then(Result::ok)
    });
    // Буква под картинкой: пока аватар грузится или если он битый, видна она.
    let avatar = div()
        .relative()
        .size(u(20.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .overflow_hidden()
        .bg(white(0.08))
        .shadow(vec![ring(zoom, 1., theme.color("l-border"))])
        .text_px(10., 10.)
        .font_weight(FontWeight::BOLD)
        .text_color(text)
        .child(post.letter.clone())
        .children(photo.map(|photo| {
            // Обрезка `overflow: hidden` в GPUI прямоугольная: круг — у самой картинки.
            img(photo)
                .absolute()
                .inset_0()
                .size_full()
                .rounded_full()
                .object_fit(ObjectFit::Cover)
        }));
    let framed = avatar_frame(id(&post.id, "frame"), 20., avatar)
        .css(post.frame_css.clone())
        .width(2.)
        .hole(solid);
    let nick = nick_name(id_n(&post.id, "nick", index), post.name.clone())
        .look(post.nick_style.clone(), post.nick_css.clone())
        .subscriber(post.subscriber, None);
    let button = id(&post.id, "author");
    let focus = focus_handle(&button, window, cx);
    if focus.is_focused(window) {
        panel.feed_rows.focus(index);
    }
    let ring_on = focus_visible(&focus, window);
    let steam_id = post.author_id.clone();
    let label = HomePanel::tr(cx)("#L_HOME_AUTHOR_OPEN", &[]);
    let inner = button.clone();
    hover(button, Timing::fast(), move |t| {
        div()
            .id(inner)
            .role(Role::Button)
            .button_text()
            .tip(label)
            .relative()
            .flex()
            .items_center()
            .gap(u(8.))
            .min_w_0()
            .my(u(-2.))
            .pt(u(2.))
            .pb(u(2.))
            .pl(u(2.))
            .pr(u(8.))
            .rounded_full()
            .bg(white(0.06 * t))
            .font_weight(FontWeight::BOLD)
            .text_color(mix(dim, text, t))
            .track_focus(&focus)
            .cursor_pointer()
            .on_click(move |_, _, cx| open_profile(&steam_id, cx))
            .child(framed)
            .child(nick)
            // :focus-visible — кольцо 1 px акцентом внутри.
            .when(ring_on, |el| el.shadow(vec![ring(zoom, 1., accent)]))
    })
    .into_any_element()
}

/// Цвет текста таблицы стилей.
fn tone(theme: &Theme, role: Tone) -> Hsla {
    theme.color(match role {
        Tone::Text => "l-text",
        Tone::TextDim => "l-text-dim",
        Tone::TextFaint => "l-text-faint",
        Tone::LAccent => "l-accent",
        Tone::Accent => "accent",
    })
}

fn rgba8([r, g, b, a]: [u8; 4]) -> Hsla {
    gpui::Rgba {
        r: f32::from(r) / 255.,
        g: f32::from(g) / 255.,
        b: f32::from(b) / 255.,
        a: f32::from(a) / 255.,
    }
    .into()
}

/// Тело: блоки потока со схлопнутыми полями между ними; ещё верхнее поле первого блока и
/// нижнее последнего — их схлопывает место вокруг тела.
fn body(
    doc: &Doc,
    post: &str,
    window: &mut Window,
    cx: &mut Context<HomePanel>,
) -> (AnyElement, f32, f32) {
    let theme = cx.global::<Theme>().clone();
    let zoom = zoom(window);
    // max-width: 90ch — ширина «0» Rubik кегля тела.
    let run = gpui::TextRun {
        len: 1,
        font: gpui::font(SHEET.family),
        color: gpui::black(),
        ..Default::default()
    };
    let zero = window
        .text_system()
        .layout_line("0", zpx(zoom, SHEET.size), &[run], None);
    let ch = f32::from(zero.width) / zoom;
    let on_link = {
        let post = post.to_owned();
        let links = doc.links.clone();
        let view = cx.entity().downgrade();
        Rc::new(move |link: usize, _: &mut Window, cx: &mut App| {
            let Some(Link::Href(url)) = links.get(link) else {
                return;
            };
            view.update(cx, |panel, cx| {
                let effects = panel.state.open_post_link(url, &post);
                panel.run(effects, cx);
            })
            .ok();
        }) as LinkHandler
    };
    let mut flow = Flow::default();
    for (index, block) in doc.blocks.iter().enumerate() {
        let (el, top, bottom) = block_el(block, &id_n(post, "block", index), 0, &theme, &on_link);
        flow.push(el, top, bottom);
    }
    let (children, first, last) = flow.finish();
    let el = div()
        .flex()
        .flex_col()
        .when(ch > 0., |el| {
            el.max_w(u(ch * SHEET.max_width_ch.unwrap_or(90.)))
        })
        .text_px(SHEET.size, SHEET.line_height.px(SHEET.size))
        .text_color(tone(&theme, SHEET.color))
        .children(children)
        .into_any_element();
    (el, first, last)
}

type LinkHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// Блоки подряд: между соседними — большее из полей (схлопывание CSS).
#[derive(Default)]
struct Flow {
    children: Vec<AnyElement>,
    first: Option<f32>,
    last: f32,
}

impl Flow {
    fn push(&mut self, el: AnyElement, top: f32, bottom: f32) {
        let el = match self.first {
            None => {
                self.first = Some(top);
                el
            }
            Some(_) => div().mt(u(self.last.max(top))).child(el).into_any_element(),
        };
        self.children.push(el);
        self.last = bottom;
    }

    fn finish(self) -> (Vec<AnyElement>, f32, f32) {
        (self.children, self.first.unwrap_or(0.), self.last)
    }
}

/// Блок и его поля сверху и снизу.
fn block_el(
    block: &Block,
    id: &ElementId,
    depth: usize,
    theme: &Theme,
    on_link: &LinkHandler,
) -> (AnyElement, f32, f32) {
    match block {
        Block::Paragraph(nodes) => {
            let (top, bottom) = SHEET.paragraph;
            (inline_el(id, nodes, theme, on_link), top, bottom)
        }
        Block::Inline(nodes) => (inline_el(id, nodes, theme, on_link), 0., 0.),
        Block::Heading(level, nodes) => {
            let heading = SHEET.heading;
            let size = heading.sizes[usize::from(level.saturating_sub(3)).min(3)];
            let el = div()
                .text_px(size, heading.line_height * size)
                .font_weight(FontWeight(f32::from(heading.weight)))
                .text_color(tone(theme, heading.color))
                .child(inline_el(id, nodes, theme, on_link));
            (el.into_any_element(), heading.margins.0, heading.margins.1)
        }
        Block::List(list) => {
            let (top, bottom) = if depth == 0 {
                SHEET.list.margins
            } else {
                SHEET.list.nested_margins
            };
            (list_el(list, id, depth, theme, on_link), top, bottom)
        }
        Block::Code(code) => {
            let pre = SHEET.pre;
            let (top, bottom) = pre.margins.unwrap_or((SHEET.size, SHEET.size));
            // shortcut: `overflow-x: auto` без полосы — длинная строка обрезается.
            let el = div()
                .px(u(pre.pad_x))
                .py(u(pre.pad_y))
                .rounded(u(pre.radius))
                .bg(rgba8(pre.background))
                .overflow_hidden()
                .font_family(SHEET.code.family)
                .text_size(u(SHEET.code_size(SHEET.size)))
                .text_color(
                    SHEET
                        .code
                        .color
                        .map_or(tone(theme, SHEET.color), |role| tone(theme, role)),
                )
                .whitespace_nowrap()
                .child(code.clone());
            (el.into_any_element(), top, bottom)
        }
    }
}

/// `ul`/`ol`: поле слева 24 под маркер. `N. ` — текстом вплотную к пункту; у `ul` Chromium
/// рисует фигуру 4 × 4 (диск, вложенные — кольцо и квадрат) за 13 px до текста, по середине
/// первой строки.
fn list_el(
    list: &List,
    id: &ElementId,
    depth: usize,
    theme: &Theme,
    on_link: &LinkHandler,
) -> AnyElement {
    let start = list.start.map_or(1, |start| start as i64);
    let color = tone(theme, SHEET.color);
    let line = SHEET.line_height.px(SHEET.size);
    let items = list.items.iter().enumerate().map(|(index, item)| {
        let marker = if list.ordered {
            div()
                .flex_none()
                .w(u(SHEET.list.padding_left))
                .flex()
                .justify_end()
                .whitespace_nowrap()
                .child(format!("{}.\u{a0}", start + index as i64))
        } else {
            let shape = div().size(u(4.));
            let shape = match depth {
                0 => shape.rounded_full().bg(color),
                1 => shape.rounded_full().border_1().border_color(color),
                _ => shape.bg(color),
            };
            div()
                .flex_none()
                .w(u(SHEET.list.padding_left))
                .h(u(line))
                .flex()
                .items_center()
                .pl(u(SHEET.list.padding_left - 17.))
                .child(shape)
        };
        let mut flow = Flow::default();
        for (at, block) in item.iter().enumerate() {
            let child = ElementId::NamedChild(Arc::new(id.clone()), format!("{index}-{at}").into());
            let (el, top, bottom) = block_el(block, &child, depth + 1, theme, on_link);
            flow.push(el, top, bottom);
        }
        let (children, _, _) = flow.finish();
        div().flex().child(marker).child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .children(children),
        )
    });
    div().flex().flex_col().children(items).into_any_element()
}

/// Строчное содержимое одним текстом: подсветки `strong`/`em`/`code`/ссылок, Cascadia Mono у
/// `code`, щелчок по ссылке.
fn inline_el(id: &ElementId, nodes: &[Inline], theme: &Theme, on_link: &LinkHandler) -> AnyElement {
    let text_color = tone(theme, SHEET.strong_color.unwrap_or(SHEET.color));
    let accent = tone(theme, SHEET.link_color);
    let mut text = String::new();
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut mono: Vec<(Range<usize>, SharedString)> = Vec::new();
    let mut links: Vec<(Range<usize>, usize)> = Vec::new();
    for node in nodes {
        match node {
            Inline::Text { text: part, style } => {
                let start = text.len();
                text.push_str(part);
                let range = start..text.len();
                if range.is_empty() {
                    continue;
                }
                let mut look = HighlightStyle::default();
                if style.strong {
                    look.font_weight = Some(FontWeight(f32::from(SHEET.strong_weight)));
                    look.color = Some(text_color);
                }
                if style.em {
                    look.font_style = Some(FontStyle::Italic);
                }
                if style.code {
                    // shortcut: без полей 2/5 px, скругления и кегля .93em — их у прогона текста нет.
                    look.background_color = Some(rgba8(SHEET.code.background));
                    look.color = SHEET.code.color.map(|role| tone(theme, role));
                    mono.push((range.clone(), MONO.into()));
                }
                if let Some(link) = style.link {
                    look.color = Some(accent);
                    look.underline = Some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(accent),
                        wavy: false,
                    });
                    links.push((range.clone(), link as usize));
                }
                if look != HighlightStyle::default() {
                    highlights.push((range, look));
                }
            }
            Inline::Break => text.push('\n'),
            // В новостях картинок нет: `news-markdown.js` без `imageBase`.
            Inline::Image { .. } => {}
        }
    }
    let styled = StyledText::new(SharedString::from(text))
        .with_highlights(highlights)
        .with_font_family_overrides(mono);
    if links.is_empty() {
        return div().child(styled).into_any_element();
    }
    let ranges: Vec<Range<usize>> = links.iter().map(|(range, _)| range.clone()).collect();
    let targets: Vec<usize> = links.iter().map(|(_, link)| *link).collect();
    let on_link = on_link.clone();
    div()
        .child(InteractiveText::new(id.clone(), styled).on_click(
            ranges,
            move |index, window, cx| {
                if let Some(link) = targets.get(index) {
                    on_link(*link, window, cx);
                }
            },
        ))
        .into_any_element()
}

/// Плитки скриншотов: 2 колонки через 8 (одна — до 480 px), нечётное число — первая на всю
/// ширину; картинка — по `tileImageStyle` (доли плитки), наведение — прозрачность .85.
fn images(
    panel: &mut HomePanel,
    at: usize,
    post: &PostView,
    top: f32,
    window: &mut Window,
    cx: &mut Context<HomePanel>,
) -> AnyElement {
    let theme = cx.global::<Theme>().clone();
    let border = theme.color("l-border");
    let tr = HomePanel::tr(cx);
    let (columns, max) = if post.single_image {
        (1, 480.)
    } else {
        (2, 520.)
    };
    let tiles: Vec<AnyElement> = post
        .tiles
        .iter()
        .map(|tile| {
            let width = if post.single_image || tile.wide {
                max
            } else {
                (max - 8.) / 2.
            };
            let photo = tile_photo(panel, tile, width, window, cx);
            let tile_id = id_n(&post.id, "tile", tile.index as usize);
            let label = format!("{} {}", tr("#L_HOME_IMAGE_OPEN", &[]), tile.index + 1);
            let (post_id, index, wide) = (post.id.clone(), tile.index as usize, tile.wide);
            let place = (tile.left, tile.top, tile.width, tile.height);
            let aspect = tile.aspect;
            // shortcut: обрезка GPUI прямоугольная — углы скругляет сама картинка, когда она
            // ровно во всю плитку; кадрированная выходит за плитку и остаётся с углами.
            let fills = (tile.left.abs() + tile.top.abs()) < 0.005
                && (tile.width - 1.).abs() < 0.005
                && (tile.height - 1.).abs() < 0.005;
            let on_click = cx.listener(move |panel, _: &ClickEvent, window, cx| {
                panel.show_image(&post_id, index, window, cx)
            });
            let inner = tile_id.clone();
            let target = format!("home-image-{at}-{index}");
            let tile = hover(tile_id, Timing::fast(), move |t| {
                let (left, top, w, h) = place;
                div()
                    .id(inner)
                    .role(Role::Button)
                    .button_text()
                    .aria_label(label)
                    .relative()
                    .overflow_hidden()
                    .aspect_ratio(aspect)
                    .border_1()
                    .border_color(border)
                    .rounded(u(8.))
                    .bg(rgb(0x101316))
                    .cursor_pointer()
                    .when(wide, |el| el.col_span(2))
                    .on_click(on_click)
                    .children(photo.map(|photo| {
                        img(photo)
                            .absolute()
                            .left(relative(left))
                            .top(relative(top))
                            .w(relative(w))
                            .h(relative(h))
                            .when(fills, |img| img.rounded(u(7.)))
                            .object_fit(ObjectFit::Fill)
                            .opacity(1. - 0.15 * t)
                    }))
            });
            crate::snap::target(target, tile)
        })
        .collect();
    div()
        .grid()
        .grid_cols(columns)
        .gap(u(8.))
        .max_w(u(max))
        .mt(u(top))
        .children(tiles)
        .into_any_element()
}

/// Картинка плитки шириной `width` CSS px: загрузчик уменьшает её один раз под эту область.
fn tile_photo(
    panel: &mut HomePanel,
    tile: &Tile,
    width: f32,
    window: &mut Window,
    cx: &mut Context<HomePanel>,
) -> Option<Arc<gpui::RenderImage>> {
    let source = ImageSource::parse(&tile.url)?;
    let height = width / tile.aspect.max(0.01);
    let (w, h) = device_size(window, width * tile.width, height * tile.height);
    let request = ImageRequest::new(source, SCOPE).with_target(w, h, Fit::Cover);
    panel.pictures.get(request, window, cx).and_then(Result::ok)
}

/// `.news-reaction`: смайлик и счёт; выбранная — рамка и подложка акцентом; пока ответ на
/// реакцию идёт — `disabled` (.65).
fn reactions(
    at: usize,
    post: &PostView,
    top: f32,
    theme: &Theme,
    cx: &mut Context<HomePanel>,
) -> AnyElement {
    let tr = HomePanel::tr(cx);
    let buttons: Vec<AnyElement> = post
        .reactions
        .iter()
        .enumerate()
        .map(|(index, reaction)| {
            let button = reaction_el(post, index, reaction, theme, &tr, cx);
            crate::snap::target(format!("home-react-{at}-{index}"), button)
        })
        .collect();
    div()
        .flex()
        .flex_wrap()
        .gap(u(6.))
        .mt(u(top))
        .children(buttons)
        .into_any_element()
}

fn reaction_el(
    post: &PostView,
    index: usize,
    reaction: &ReactionView,
    theme: &Theme,
    tr: &hlsr_ui::controllers::bridge::Tr,
    cx: &mut Context<HomePanel>,
) -> AnyElement {
    let (border, strong) = (theme.color("l-border"), theme.color("l-border-strong"));
    let (dim, text, accent) = (
        theme.color("l-text-dim"),
        theme.color("l-text"),
        theme.color("l-accent"),
    );
    let emoji = reaction.emoji.clone();
    let (selected, disabled) = (reaction.selected, post.pending);
    let title = tr(&format!("#L_HOME_REACTION_{}", reaction.key), &[]);
    let label = reaction.label.clone();
    let count = reaction.count.to_string();
    let (post_id, key) = (post.id.clone(), reaction.key.clone());
    let on_click = cx.listener(move |panel, _: &ClickEvent, _, cx| {
        let effects = panel.state.react(&post_id, &key);
        panel.run(effects, cx);
    });
    let button = id_n(&post.id, "react", index);
    let inner = button.clone();
    hover(button, Timing::fast(), move |t| {
        let t = if disabled { 0. } else { t };
        let (edge, fill, color) = if selected {
            (accent, accent.opacity(0.16), text)
        } else {
            (mix(border, strong, t), white(0.04 * t), dim)
        };
        div()
            .id(inner)
            .role(Role::Button)
            .button_text()
            .aria_label(label)
            .tip(title)
            .flex()
            .items_center()
            .gap(u(7.))
            .min_w(u(54.))
            .min_h(u(30.))
            .px(u(10.))
            .py(u(3.))
            .border_1()
            .border_color(edge)
            .rounded(u(16.))
            .bg(fill)
            .text_color(color)
            .text_px(12., 12. * 1.185)
            .child(emoji)
            .child(count)
            .when(disabled, |el| el.opacity(0.65))
            .when(!disabled, |el| el.cursor_pointer().on_click(on_click))
    })
    .into_any_element()
}
