//! Шапка окна профиля (`.profile__hero` 0.4.9): баннер (свечение акцента и бледная лямбда; у
//! разработчика — второй оттенок, сетка точек и знак «</>»), аватар в рамке с точкой состояния,
//! ник со значком подписчика, плашка значков, описание и местоимения, строка «в игре · в HLSR с …
//! · рейтинг · сомнительный · ⋯», вкладки с `l-ink`. Низкое окно (высота ≤ 700 CSS px) — компактная
//! шапка: баннер 66, аватар 72, ник 22/30, значки и дата одной строкой под ником, вкладки 40.

use super::badges::badge_row;
use super::modal::{ProfileModal, Section};
use crate::common::{avatar_frame, nick_name};
use crate::icons::fa_glyph;
use crate::image::device_size;
use crate::kit::button::{child_id, focus_handle, focus_visible, outline, white};
use crate::kit::{Tab, TabStyle, tabs};
use crate::layer::{Place, PopLook, TipExt, menu_keys, popup};
use crate::motion::{Push, Timing, hover, mix, press, rise_in};
use crate::paint::{Radial, ring, shadow, strips};
use crate::scale::{u, zoom};
use crate::text::TextExt;
use crate::theme::{Gradient, Stop, Theme};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, FontWeight, Hsla, IntoElement, ObjectFit, Radians, Role, Transformation,
    Window, canvas, div, img, linear_color_stop, linear_gradient, point, prelude::*, px, rgb, size,
    svg,
};
use hlsr_ui::controllers::profile::ProfileView;
use hlsr_ui::images::{Fit, ImageRequest, ImageSource};

/// Лямбда баннера (`::after`, viewBox 0 0 177 186).
const LAMBDA: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 177 186'>\
<path d='M47.2 12.9v25.9h21.4l8.7 27.1-65.1 101h32l46.5-68.6 30.5 77 48-15.3-7.6-23.6-22.1 \
6.9-51.8-130.4z'/></svg>";

/// Знак «</>» баннера разработчика (viewBox 0 0 120 96).
const CODE: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 120 96'>\
<path d='M34 18 6 48l28 30 9-9-19-21 19-21zM86 18l-9 9 19 21-19 21 9 9 28-30zM67 8 41 88l12 \
4L79 12z'/></svg>";

/// `color-mix(in srgb, a p%, b)`.
fn blend(a: Hsla, p: f32, b: Hsla) -> Hsla {
    mix(b, a, p)
}

/// Баннер `.profile__banner` высотой `height`, шириной окна `width` (CSS px).
fn banner(theme: &Theme, developer: bool, width: f32, height: f32, radius: f32) -> AnyElement {
    let accent = theme.color("l-accent");
    let bg = theme.color("l-modal-bg");
    let text = theme.color("l-text");
    let radial = |center: (f32, f32), radius: (f32, f32), alpha: f32, color: Hsla| {
        Radial {
            center,
            radius,
            stops: vec![(0., alpha), (0.7, 0.)],
        }
        .paint(width, height, color)
        .absolute()
        .top_0()
        .left_0()
    };
    let base = div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(u(height))
        .rounded_t(u(radius))
        .overflow_hidden();
    if !developer {
        let glyph = svg()
            .data(LAMBDA.as_bytes())
            .absolute()
            .right(u(72.))
            .top(u(-36.))
            .w(u(180.))
            .h(u(188.))
            .text_color(text.opacity(0.05))
            .with_transformation(Transformation::rotate(Radians(-12f32.to_radians())));
        return base
            .bg(linear_gradient(
                180.,
                linear_color_stop(blend(accent, 0.12, bg), 0.),
                linear_color_stop(bg, 1.),
            ))
            .child(radial((0., 0.), (0.7, 1.6), 0.3, accent))
            .child(radial((1., 1.), (0.6, 1.4), 0.14, accent))
            .child(glyph)
            .into_any_element();
    }
    let tint = blend(accent, 0.4, rgb(0xa26bff).into());
    // linear-gradient(110deg, …) из трёх стопов — полосами вдоль оси (в GPUI градиент из двух).
    let sweep = Gradient {
        angle: 90.,
        stops: vec![
            Stop {
                pos: 0.,
                color: blend(accent, 0.18, bg).to_rgb(),
            },
            Stop {
                pos: 0.5,
                color: blend(tint, 0.2, bg).to_rgb(),
            },
            Stop {
                pos: 1.,
                color: blend(accent, 0.18, bg).to_rgb(),
            },
        ],
    };
    let dots = canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            // Сетка точек 16 px, тающая к низу (`mask: linear-gradient(#000, transparent 85%)`).
            let step = bounds.size.width / (width / 16.);
            let dot = step * (1.5 / 16.);
            let rows = (height / 16.).ceil() as usize;
            let cols = (width / 16.).ceil() as usize;
            for row in 0..rows {
                let y = step * (row as f32 + 0.5);
                let fade = 1. - (row as f32 * 16. + 8.) / (height * 0.85);
                if fade <= 0. {
                    break;
                }
                for col in 0..cols {
                    let x = step * (col as f32 + 0.5);
                    let origin = bounds.origin + point(x - dot, y - dot);
                    window.paint_quad(
                        gpui::fill(
                            gpui::Bounds::new(origin, size(dot * 2., dot * 2.)),
                            white(0.16 * fade),
                        )
                        .corner_radii(dot),
                    );
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full();
    let glyph = svg()
        .data(CODE.as_bytes())
        .absolute()
        .right(u(64.))
        .top(u(-20.))
        .w(u(200.))
        .h(u(160.))
        .text_color(tint.opacity(0.16))
        .with_transformation(Transformation::rotate(Radians(-12f32.to_radians())));
    base.children(strips(&sweep, px(0.)))
        .child(radial((0., 0.), (0.7, 1.6), 0.42, accent))
        .child(radial((1., 0.), (0.6, 1.6), 0.48, tint))
        .child(dots)
        .child(glyph)
        .into_any_element()
}

/// `.profile__since`: значок 11 px и текст 12,5/20 цветом `--l-text-faint`.
fn since(glyph: &str, text: String, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(u(8.))
        .text_px(12.5, 20.)
        .text_color(theme.color("l-text-faint"))
        .child(fa_glyph(glyph, 11.))
        .child(text)
}

impl ProfileModal {
    /// Аватар 112 (72) со скруглением 20 в рамке шириной 6 и точкой состояния.
    fn avatar(
        &mut self,
        view: &ProfileView,
        side: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let bg = theme.color("l-modal-bg");
        let framed = !view.frame_css.is_empty();
        let photo = ImageSource::parse(&view.avatar).and_then(|source| {
            let (w, h) = device_size(window, side, side);
            let request =
                ImageRequest::new(source, super::badges::SCOPE).with_target(w, h, Fit::Cover);
            self.pictures.get(request, window, cx).and_then(Result::ok)
        });
        let dot_color: Hsla = match view.dot {
            "online" => rgb(0x4ad07a).into(),
            "checking" => rgb(0xffb37a).into(),
            _ => rgb(0x8b939e).into(),
        };
        let mut shadows = vec![shadow(zoom, 0., 12., 32., 0., gpui::black().opacity(0.45))];
        if !framed {
            shadows.insert(0, shadow(zoom, 0., 0., 0., 6., bg));
        }
        let avatar = div()
            .id("profile-avatar")
            .relative()
            .size(u(side))
            .flex()
            .items_center()
            .justify_center()
            .rounded(u(20.))
            .bg(bg)
            .shadow(shadows)
            .text_color(theme.color("l-text-faint"))
            .map(|el| match photo {
                Some(photo) => el.child(
                    img(photo)
                        .size_full()
                        .rounded(u(20.))
                        .object_fit(ObjectFit::Cover),
                ),
                None => el.child(fa_glyph(
                    "fab fa-steam",
                    if side < 100. { 28. } else { 40. },
                )),
            })
            // Точка состояния: 20 × 20, кольцо 4 px цветом окна (`round-dot`).
            .child(
                div()
                    .id("profile-dot")
                    .absolute()
                    .right(u(-4.))
                    .bottom(u(-4.))
                    .size(u(20.))
                    .rounded_full()
                    .bg(bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .tip(view.presence_text.clone())
                    .child(div().size(u(12.)).rounded_full().bg(dot_color)),
            );
        avatar_frame("profile-avatar-frame", side, avatar)
            .css(view.frame_css.clone())
            .width(6.)
            .radius(20.)
            .into_any_element()
    }

    /// Рейтинг: у своего — звезда и счёт, у чужого — лайк, счёт, дизлайк; подсказка после
    /// голоса всплывает под ним.
    fn rating(
        &mut self,
        view: &ProfileView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let rating = view.rating.clone()?;
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let (dim, text) = (theme.color("l-text-dim"), theme.color("l-text"));
        let score_color = match rating.tone {
            "positive" => rgb(0x5fd38a).into(),
            "negative" => rgb(0xff8a96).into(),
            _ => dim,
        };
        let button = |id: &'static str,
                      glyph: &'static str,
                      on: bool,
                      good: bool,
                      title: String,
                      label: String,
                      value: i64,
                      cx: &mut Context<Self>| {
            let on_color: Hsla = if good {
                rgb(0x5fd38a).into()
            } else {
                rgb(0xff8a96).into()
            };
            let voting = rating.voting;
            let click = cx.listener(move |this, _, _, cx| {
                let effects = this.state.borrow_mut().cast_vote(value);
                this.run(effects, cx);
            });
            let press_id = child_id(&id.into(), "press");
            hover(id, Timing::fast(), move |t| {
                let t = if voting { 0. } else { t };
                let el = div()
                    .id(id)
                    .role(Role::Button)
                    .button_text()
                    .aria_label(label)
                    .tip(title)
                    .size(u(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .map(|el| {
                        if on {
                            el.bg(on_color.opacity(0.18)).text_color(on_color)
                        } else {
                            el.bg(white(0.1 * t)).text_color(mix(dim, text, t))
                        }
                    })
                    .when(voting, |el| el.opacity(0.6))
                    .when(!voting, |el| el.cursor_pointer().on_click(click))
                    .child(fa_glyph(glyph, 12.).mt(u(1.)));
                // `transform 90ms ease-out`, `:active:not(:disabled) { scale(.9) }`.
                let push = Push::new(0.9, Timing::new(90., [0., 0., 0.58, 1.]));
                press(press_id, push, el).live(!voting)
            })
        };
        let score = div()
            .font_weight(FontWeight::BOLD)
            .text_color(score_color)
            .tnum()
            .child(rating.score.clone());
        let pill = div()
            .id("profile-rating")
            .flex()
            .items_center()
            .gap(u(2.))
            .p(u(2.))
            .rounded_full()
            .bg(white(0.06))
            .shadow(vec![ring(zoom, 1., theme.color("l-border"))])
            .text_px(12.5, 16.)
            .text_color(dim)
            .tip(rating.hint.clone());
        let pill = if rating.can_vote {
            let like = button(
                "profile-like",
                "fas fa-thumbs-up",
                rating.vote == 1,
                true,
                rating.like_title.clone(),
                rating.like_label.clone(),
                1,
                cx,
            );
            let dislike = button(
                "profile-dislike",
                "fas fa-thumbs-down",
                rating.vote == -1,
                false,
                rating.dislike_title.clone(),
                rating.dislike_label.clone(),
                -1,
                cx,
            );
            pill.child(like)
                .child(
                    div()
                        .min_w(u(26.))
                        .px(u(4.))
                        .mt(u(1.))
                        .flex()
                        .justify_center()
                        .child(score),
                )
                .child(dislike)
        } else {
            // Звезда в ячейке 26 px, как кнопки голоса; счёт без полей справа 10.
            pill.child(
                div()
                    .size(u(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(fa_glyph("fas fa-star", 11.5)),
            )
            .child(div().pr(u(10.)).child(score))
        };
        let note = self
            .state
            .borrow()
            .vote_note_text(self.language, &self.wall, &self.tr);
        let note = (!note.is_empty()).then(|| {
            div()
                .absolute()
                .top(u(26. + 4. + 6.))
                .left_0()
                .py(u(5.))
                .px(u(10.))
                .rounded(theme.rem("l-radius-sm"))
                .bg(theme.color("l-bg-raised"))
                .shadow(vec![
                    ring(zoom, 1., theme.color("l-border")),
                    shadow(zoom, 0., 8., 24., 0., gpui::black().opacity(0.45)),
                ])
                .text_px(12., 16.)
                .text_color(dim)
                .whitespace_nowrap()
                .child(note)
        });
        Some(
            div()
                .relative()
                .flex()
                .child(pill)
                .children(note)
                .into_any_element(),
        )
    }

    /// Кнопка «⋯» и её меню (как меню игры: стрелки, Esc, Tab).
    fn more(
        &mut self,
        view: &ProfileView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if view.more.is_empty() {
            return None;
        }
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let (dim, text, accent) = (
            theme.color("l-text-dim"),
            theme.color("l-text"),
            theme.color("l-accent"),
        );
        let radius = theme.len("l-radius-sm");
        let id = gpui::ElementId::from("profile-more");
        let focus = focus_handle(&id, window, cx);
        let ring_on = focus_visible(&focus, window);
        let open = self.more.open;
        let active = self.more.active;
        let anchor = self.more_anchor.clone();
        let labels: Vec<String> = view.more.iter().map(|item| item.label.clone()).collect();
        let disabled = vec![false; labels.len()];
        let ids: Vec<&'static str> = view.more.iter().map(|item| item.id).collect();
        let keys_ids = ids.clone();
        let on_key = {
            let this = cx.weak_entity();
            move |key: &crate::layer::MenuKey, _: &mut Window, cx: &mut gpui::App| {
                let now = std::time::Instant::now();
                let labels = labels.clone();
                let disabled = disabled.clone();
                let ids = keys_ids.clone();
                this.update(cx, |this, cx| {
                    let outcome = this.more.key(key, now, &labels, &disabled, -1).settle(cx);
                    if let crate::layer::Outcome::Pick(index) = outcome {
                        let effects = this.state.borrow().choose_more(ids[index]);
                        this.run(effects, cx);
                    }
                    cx.notify();
                })
                .ok();
            }
        };
        let toggle = cx.listener(|this, _, _, cx| {
            if this.more.open {
                this.more.hide();
            } else {
                this.more.show();
            }
            cx.notify();
        });
        let more_label = self.t("#L_PROFILE_MORE");
        let trigger = hover("profile-more-hover", Timing::fast(), move |t| {
            let t = if open { 1. } else { t };
            menu_keys(
                div()
                    .id("profile-more")
                    .role(Role::Button)
                    .button_text()
                    .aria_label(more_label.clone())
                    .tip(more_label)
                    .relative()
                    .size(u(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(u(radius))
                    .bg(mix(white(0.04), white(0.08), t))
                    .text_color(mix(dim, text, t))
                    .track_focus(&focus)
                    .cursor_pointer()
                    .on_click(toggle)
                    .child(fa_glyph("fas fa-ellipsis", 13.))
                    .child(crate::layer::anchor_probe(anchor))
                    .when(ring_on, |el| {
                        el.child(outline(zoom, radius, 2., 2., accent))
                    }),
                on_key,
            )
        });
        let items: Vec<_> = view
            .more
            .iter()
            .enumerate()
            .map(|(index, item)| (index, item.id, item.icon, item.label.clone()))
            .collect();
        let theme = cx.global::<Theme>().clone();
        let this = cx.weak_entity();
        let menu_theme = theme.clone();
        let place = Place {
            anchor: self.more_anchor.get(),
            clip: None,
            up: false,
            align_right: false,
        };
        let dismiss = {
            let this = this.clone();
            move |_: &mut Window, cx: &mut gpui::App| {
                this.update(cx, |this, cx| {
                    this.more.hide();
                    cx.notify();
                })
                .ok();
            }
        };
        // `.profile-more__menu` 0.4.9: не стекло ни в одной теме, переход `profile-more`
        // (подъём на `--l-rise`, .96), `transform-origin: 14px 0`.
        let look = PopLook {
            glass: false,
            motion: "profile-more",
            origin: Some(crate::motion::Origin::new((0., 14.), (0., 0.))),
        };
        let menu = popup(
            "profile-more-menu",
            open,
            place,
            look,
            window,
            cx,
            dismiss,
            move || {
                let theme = &menu_theme;
                let (text, dim, accent, border) = (
                    theme.color("l-text"),
                    theme.color("l-text-dim"),
                    theme.color("l-accent"),
                    theme.color("l-border"),
                );
                let soft = theme.color("l-accent-soft");
                div()
                    .w(u(248. - 16.))
                    .flex()
                    .flex_col()
                    .gap(u(4.))
                    .children(items.into_iter().map(move |(index, id, glyph, label)| {
                        let focused = active == index as i32;
                        let this = this.clone();
                        rise_in(
                            ("profile-more-item-in", index),
                            index,
                            div().child(hover(
                                ("profile-more-item", index),
                                Timing::fast(),
                                move |t| {
                                    let t = if focused { 1. } else { t };
                                    div()
                                        .id(("profile-more-item", index))
                                        .role(Role::MenuItem)
                                        .button_text()
                                        .flex()
                                        .items_center()
                                        .gap(u(12.))
                                        .w_full()
                                        .p(u(8.))
                                        .rounded(theme_radius(radius))
                                        .bg(white(0.05 * t))
                                        .shadow(vec![ring(
                                            zoom,
                                            1.,
                                            if focused {
                                                accent
                                            } else {
                                                blend(accent, 0.45, border).opacity(t)
                                            },
                                        )])
                                        .text_px(14., 20.)
                                        .text_color(text)
                                        .whitespace_nowrap()
                                        .cursor_pointer()
                                        .on_click(move |_, _, cx| {
                                            this.update(cx, |this, cx| {
                                                this.more.hide();
                                                let effects = this.state.borrow().choose_more(id);
                                                this.run(effects, cx);
                                                cx.notify();
                                            })
                                            .ok();
                                        })
                                        .child(
                                            div()
                                                .flex_none()
                                                .size(u(32.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded(theme_radius(radius))
                                                .bg(mix(white(0.04), soft, t))
                                                .shadow(vec![ring(
                                                    zoom,
                                                    1.,
                                                    border.opacity(1. - t),
                                                )])
                                                .text_color(mix(dim, accent, t))
                                                .child(fa_glyph(glyph, 13.)),
                                        )
                                        .child(label)
                                },
                            )),
                        )
                    }))
            },
        );
        Some(
            div()
                .relative()
                .flex()
                .child(trigger)
                .child(menu)
                .into_any_element(),
        )
    }

    /// Шапка целиком; `width` — ширина окна (CSS px).
    pub(super) fn hero(
        &mut self,
        view: &ProfileView,
        compact: bool,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let radius = theme.len("l-radius");
        let side = if compact { 72. } else { 112. };
        let avatar = self.avatar(view, side, window, cx);
        let name = div()
            .min_w_0()
            .max_w_full()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .map(|el| {
                if compact {
                    el.text_px(22., 30.).w_full()
                } else {
                    el.text_px(28., 36.)
                }
            })
            .font_weight(FontWeight::BOLD)
            .text_color(theme.color("l-text"))
            .child(
                nick_name("profile-nick", view.name.clone())
                    .look(view.nick_style.clone(), view.nick_css.clone())
                    .subscriber(view.subscriber, view.supporter_since),
            );
        let badges = view
            .badges_shown
            .then(|| badge_row(&view.badges, &self.pictures, window, cx));
        let status = (!view.status.is_empty() || !view.extras.is_empty()).then(|| {
            div()
                .flex()
                .flex_wrap()
                .items_baseline()
                .gap_x(u(12.))
                .gap_y(u(4.))
                .text_px(13.5, 20.)
                .text_color(theme.color("l-text"))
                .when(compact, |el| el.w_full())
                .when(!view.status.is_empty(), |el| el.child(view.status.clone()))
                .when(!view.extras.is_empty(), |el| {
                    el.child(
                        div()
                            .text_px(12., 20.)
                            .text_color(theme.color("l-text-faint"))
                            .child(view.extras.clone()),
                    )
                })
        });
        let rating = self.rating(view, window, cx);
        let dubious = view.dubious.then(|| {
            div()
                .id("profile-dubious")
                .flex()
                .items_center()
                .gap(u(6.))
                .py(u(3.))
                .px(u(9.))
                .rounded_full()
                .bg(gpui::rgba(0xffb02024))
                .shadow(vec![ring(zoom, 1., gpui::rgba(0xffb02066).into())])
                .text_px(12., 16.)
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xffc35a))
                .tip(self.t("#L_RATING_DUBIOUS_HINT"))
                .child(fa_glyph("fas fa-exclamation-triangle", 12.))
                .child(self.t("#L_RATING_DUBIOUS"))
        });
        let more = self.more(view, window, cx);
        let meta = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(u(16.))
            .gap_y(u(8.))
            .when(!view.playing.is_empty(), |el| {
                el.child(since("fas fa-gamepad", view.playing.clone(), &theme))
            })
            .child(since("fas fa-calendar-day", view.since.clone(), &theme))
            .children(rating)
            .children(dubious)
            .children(more);
        let who = div().flex_1().min_w_0().flex().map(|el| {
            if compact {
                el.flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_x(u(16.))
                    .gap_y(u(8.))
                    .pt(u(6.))
                    .child(name)
                    .children(badges)
                    .child(meta)
                    .children(status)
            } else {
                el.flex_col()
                    .items_start()
                    .gap(u(8.))
                    .pt(u(14.))
                    .child(name)
                    .children(badges.map(|row| div().mt(u(2.)).child(row)))
                    .children(status)
                    .child(meta)
            }
        });
        let identity = div()
            .relative()
            .flex()
            .items_start()
            .gap(u(16.))
            .pt(u(if compact { 24. } else { 56. }))
            .pr(u(56.))
            .child(div().flex_none().child(avatar))
            .child(who);
        let tab_list: Vec<Tab> = view
            .tabs
            .iter()
            .map(|tab| {
                let item = Tab::new(tab.label.clone()).icon(tab.icon);
                if tab.count.is_empty() {
                    item
                } else {
                    item.count(tab.count.clone())
                }
            })
            .collect();
        let style = TabStyle {
            height: if compact { 40. } else { 48. },
            ..TabStyle::modal(&theme)
        };
        let active = self.section_index(view);
        let bar = div()
            .mt(u(if compact { 8. } else { 20. }))
            .child(crate::snap::target(
                "profile-tabs",
                tabs("profile-tabs", tab_list, active, style)
                    .label(self.t("#L_PROFILE_TITLE"))
                    .on_select(cx.listener(|this, index: &usize, _, cx| this.select(*index, cx))),
            ));
        div()
            .relative()
            .flex_none()
            .px(u(32.))
            .child(banner(
                &theme,
                view.developer,
                width,
                if compact { 66. } else { 112. },
                radius,
            ))
            .child(identity)
            .child(bar)
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(1.))
                    .bg(theme.color("l-modal-border")),
            )
            .into_any_element()
    }

    /// Номер выбранной вкладки в полосе.
    pub(super) fn section_index(&self, view: &ProfileView) -> usize {
        let index = match self.section {
            Section::Profile => 0,
            Section::Achievements => 1,
            Section::Settings => 2,
        };
        index.min(view.tabs.len().saturating_sub(1))
    }
}

fn theme_radius(radius: f32) -> gpui::Rems {
    u(radius)
}
