//! Шапка (`.launcher-topbar` LauncherShell.vue 0.4.9): логотип (кольцо с точкой, точка растёт
//! под курсором, двойной щелчок перекрашивает на 1,5 с, тройной — «Назад в прошлое!»), «HLSR»
//! и «BETA», вкладки «Главная / Игры / Персонализация», места под поиск, кнопку спонсоров,
//! уведомления и аккаунт (S4, S5), разделитель, кнопки окна.
//!
//! Вся шапка — зона перетаскивания окна (`-webkit-app-region: drag`); логотип, вкладки, поиск и
//! блок справа закрывают её собой (`occlude`, как `no-drag`). Блок логотипа шириной ровно
//! по колонке (выдвинутая — 240): вкладки едут вместе с её краем (`--l-dur-swipe`
//! `--l-ease-swipe`). Вкладка — ширина по тексту, доведённая до кратной 4 (`v-grid-width`);
//! полоса под выбранной проявляется (`--l-fast`) и растягивается от 0.4 ширины (`--l-med`).

use super::root::ShellRoot;
use super::window_controls;
use crate::kit::button::{focus_handle, focus_visible, outline};
use crate::motion::{AnimGate, Timing, hover, mix, tween};
use crate::scale::{u, zoom, zpx};
use crate::text::{RUBIK, TextExt};
use crate::theme::Theme;
use gpui::{
    Context, ElementId, FontWeight, Hsla, Rgba, TextRun, Window, WindowControlArea, div, font,
    prelude::*, px, relative,
};
use hlsr_ui::controllers::nav::{SECTION_TITLES, SECTIONS};
use hlsr_ui::services::Standing;
use std::time::Instant;

/// Перекраска логотипа идёт `.5s --l-ease` (`transition: color`).
const LOGO_FADE_MS: f32 = 500.;

/// Шапка — свой закэшированный вид ([`crate::cache`]): строит её корень оболочки
/// ([`ShellRoot::topbar`]), а заново она рисуется только по своему уведомлению (переходы,
/// наведение, виды шапки внутри) и уведомлению корня — не с каждым кадром фона или ленты.
pub(super) struct Topbar(pub(super) gpui::WeakEntity<ShellRoot>);

impl Render for Topbar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        self.0
            .update(cx, |root, cx| root.topbar(window, cx).into_any_element())
            .unwrap_or_else(|_| gpui::Empty.into_any_element())
    }
}

/// Ширина текста строки Rubik в CSS px макета.
pub(super) fn text_width(window: &Window, text: &str, size: f32, weight: FontWeight) -> f32 {
    let zoom = zoom(window);
    let run = TextRun {
        len: text.len(),
        font: gpui::Font {
            weight,
            ..font(RUBIK)
        },
        color: gpui::black(),
        ..Default::default()
    };
    let line = window
        .text_system()
        .layout_line(text, zpx(zoom, size), &[run], None);
    f32::from(line.width) / zoom
}

/// `v-grid-width`: ширина, доведённая вверх до кратной 4 (допуск — сотая шага).
fn grid_width(width: f32) -> f32 {
    (width / 4. - 0.01).ceil() * 4.
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Цвет логотипа с переходом `.5s --l-ease`: откуда, куда и с какого мгновения.
#[derive(Clone, Copy)]
pub(super) struct LogoColor {
    from: Hsla,
    to: Option<Rgba>,
    since: Instant,
}

impl ShellRoot {
    /// Цвет кольца и точки сейчас; идёт переход — нужен следующий кадр.
    fn logo_color(&mut self, theme: &Theme, window: &mut Window, cx: &Context<Self>) -> Hsla {
        let base = theme.color("l-text");
        let target = self.logo.color().map(|c| Rgba {
            r: f32::from(c.r) / 255.,
            g: f32::from(c.g) / 255.,
            b: f32::from(c.b) / 255.,
            a: 1.,
        });
        let now = Instant::now();
        let color_of = |to: Option<Rgba>| to.map_or(base, Hsla::from);
        let current = match self.logo_fade {
            Some(fade) => {
                let t = (now.duration_since(fade.since).as_secs_f32() * 1000. / LOGO_FADE_MS)
                    .clamp(0., 1.);
                let eased = crate::theme::bezier([0.22, 1., 0.36, 1.])(t);
                mix(fade.from, color_of(fade.to), eased)
            }
            None => color_of(target),
        };
        let still = AnimGate::still(cx).any();
        match self.logo_fade {
            Some(fade) if fade.to != target => {
                self.logo_fade = (!still).then_some(LogoColor {
                    from: current,
                    to: target,
                    since: now,
                });
            }
            None if target.is_some() && !still => {
                self.logo_fade = Some(LogoColor {
                    from: base,
                    to: target,
                    since: now,
                });
            }
            _ => {}
        }
        if let Some(fade) = self.logo_fade {
            if now.duration_since(fade.since).as_secs_f32() * 1000. < LOGO_FADE_MS {
                crate::motion::monitor_frame(window);
            } else if fade.to.is_none() {
                self.logo_fade = None;
            }
        }
        if still { color_of(target) } else { current }
    }

    pub(super) fn topbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let gap = theme.len("l-header-gap");
        let inset = theme.len("l-header-inset");
        let rail_w = theme.len("l-rail-w");
        let mark = theme.len("l-brand-mark");
        let open = self.nav.rail_open();
        let section = self.nav.section();
        let (text, text_dim, text_faint) = (
            theme.color("l-text"),
            theme.color("l-text-dim"),
            theme.color("l-text-faint"),
        );
        let accent = theme.color("l-accent");
        let logo_color = self.logo_color(&theme, window, cx);
        let swipe = Timing::new(
            theme.duration("l-dur-swipe").as_secs_f64() * 1000.,
            [0.32, 0.72, 0., 1.],
        );

        // Логотип: щелчки — у кольца (двойной перекрашивает, тройной — пасхалка и пони).
        let logo = self.logo.clone();
        let eggs = self.eggs.clone();
        let mark_hover = hover(
            "topbar-logo-hover",
            Timing::new(260., [0.34, 1.56, 0.64, 1.]),
            {
                move |k| {
                    div()
                        .id("topbar-logo")
                        .size(u(mark))
                        .flex_none()
                        .rounded_full()
                        .border(u(2.))
                        .border_color(logo_color)
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_click(move |event, _, cx| {
                            if logo.click() {
                                eggs.update(cx, |eggs, cx| eggs.show_pony(cx));
                            }
                            if event.click_count() == 2 {
                                logo.double_click();
                            }
                        })
                        .child(
                            div()
                                .size(u(16. * (1. + 0.3 * k)))
                                .flex_none()
                                .rounded_full()
                                .bg(logo_color),
                        )
                }
            },
        );
        let mark_el = crate::snap::target("topbar-logo", mark_hover);
        let brand = tween("topbar-brand", if open { 1. } else { 0. }, swipe, {
            let theme = theme.clone();
            move |t| {
                let side = (rail_w - mark) / 2.;
                div()
                    .id("topbar-brand")
                    .occlude()
                    .flex()
                    .items_center()
                    .flex_none()
                    .w(u(lerp(rail_w + 164., theme.len("l-rail-open-w"), t)))
                    .child(div().ml(u(side)).mr(u(lerp(side, 0., t))).child(mark_el))
                    .child(
                        div()
                            .ml(u(lerp(16., 12., t)))
                            .w(u(lerp(108., 96., t)))
                            .flex_none()
                            .child(crate::text::spaced(
                                "HLSR",
                                28.,
                                32.,
                                FontWeight::BOLD,
                                lerp(8., 5., t) / 28.,
                                text,
                            )),
                    )
                    // «BETA»: верх на 6 px ниже верха блока, 2 px после «HLSR», ширина 38.
                    .child(
                        div()
                            .self_start()
                            .mt(u(6.))
                            .ml(u(2.))
                            .w(u(38.))
                            .flex_none()
                            .child(crate::text::spaced(
                                "BETA",
                                13.,
                                13.,
                                FontWeight::BOLD,
                                0.08,
                                theme.color("l-text-dim"),
                            )),
                    )
            }
        });

        // Вкладки: поле вкладки (половина шага) уходит в промежуток слева.
        let tabs = SECTIONS.iter().zip(SECTION_TITLES).map(|(id, title)| {
            let label = self.shell.tr(title, &[]);
            let width = grid_width(text_width(window, &label, 17., FontWeight::MEDIUM) + gap);
            let active = section == *id;
            let element_id = ElementId::Name(format!("topbar-tab-{id}").into());
            let focus = focus_handle(&element_id, window, cx);
            let ring = focus_visible(&focus, window);
            let target = (*id).to_owned();
            let on_select =
                cx.listener(move |root, _: &gpui::ClickEvent, _, _| root.select_section(&target));
            let ink = tween(
                ElementId::Name(format!("topbar-ink-{id}").into()),
                if active { 1. } else { 0. },
                Timing::fast(),
                move |shown| {
                    tween(
                        ElementId::Name(format!("topbar-ink-scale-{id}").into()),
                        if active { 1. } else { 0.4 },
                        Timing::med(),
                        move |scale| {
                            let side = relative((1. - scale) / 2.);
                            div()
                                .absolute()
                                .left(u(gap / 2.))
                                .right(u(gap / 2.))
                                .bottom_0()
                                .h(u(4.))
                                .child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .left(side)
                                        .right(side)
                                        .rounded(u(2.))
                                        .bg(accent)
                                        .opacity(shown),
                                )
                        },
                    )
                },
            );
            // `transition: color var(--l-fast)`: и смена активной вкладки, и наведение.
            let lit = if active { 1. } else { 0. };
            let tab = tween(
                ElementId::Name(format!("topbar-tab-active-{id}").into()),
                lit,
                Timing::fast(),
                move |lit| {
                    hover(
                        ElementId::Name(format!("topbar-tab-hover-{id}").into()),
                        Timing::fast(),
                        move |k| {
                            let color = mix(mix(text_faint, text_dim, k), text, lit);
                            div()
                                .id(element_id)
                                .track_focus(&focus)
                                .relative()
                                .h(u(48.))
                                .min_w(u(width))
                                .px(u(gap / 2.))
                                .flex()
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .when(!active, |el| el.cursor_pointer())
                                .text_color(color)
                                .button_text()
                                .on_click(on_select)
                                .child(
                                    div()
                                        .text_px(17., 24.)
                                        .font_weight(FontWeight::MEDIUM)
                                        .whitespace_nowrap()
                                        .child(label),
                                )
                                .child(ink)
                                .when(ring, |el| el.child(outline(zoom, 8., -8., 2., accent)))
                        },
                    )
                },
            );
            // Цель шагов сверочного стенда: `topbar-tab-home|games|configs`.
            let tab = crate::snap::target(format!("topbar-tab-{id}"), tab);
            // Якорь обучения: первый шаг подсвечивает вкладку «Персонализация».
            if *id == "configs" {
                crate::configs::anchors::anchor("configs-tab", tab)
            } else {
                tab
            }
        });
        let tabs = div()
            .id("topbar-tabs")
            .occlude()
            .flex()
            .items_center()
            .h(u(48.))
            .ml(u(-gap / 2.))
            .children(tabs);

        // Поиск игроков, спонсоры и аккаунт — только у официальной сборки (`ShellState::build`).
        let build = self.flows.with_state(|state| state.build);
        let official = build == Standing::Official;
        // Поиск занимает всё место между вкладками и значками (без поиска — место остаётся).
        let search = div()
            .id("topbar-search")
            .occlude()
            .flex_grow(1.)
            .flex_shrink(1.)
            .flex_basis(u(288.))
            .min_w(u(152.))
            .ml(u(-gap / 2.))
            .mr(u(-inset))
            .when(official, |el| el.child(self.frame.search.clone()));

        // Справа: спонсоры, уведомления, разделитель, аккаунт, кнопки окна. Линия
        // разделителя места в ряду не занимает.
        let separator = div()
            .relative()
            .ml(u(inset - gap / 2.))
            .mr(u(inset * 2. - gap / 2.))
            .w(u(0.))
            .h(u(32.))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .w(px(1.))
                    .bg(theme.color("l-border")),
            );
        let right = div()
            .id("topbar-right")
            .occlude()
            .flex()
            .items_center()
            .flex_none()
            .gap(u(gap - inset * 2.))
            .when(official, |el| el.child(self.frame.sponsors.clone()))
            .child(self.frame.notifications.clone())
            .child(separator)
            .when(official, |el| el.child(self.frame.account.clone()))
            // Вместо входа — заглушка со ссылкой на выпуски; пока сервер не ответил — ничего.
            .when(build == Standing::Unofficial, |el| {
                el.child(
                    crate::common::official_stub(
                        "topbar-official-releases",
                        "#L_OFFICIAL_LOGIN",
                        12.,
                        window,
                        cx,
                    )
                    .whitespace_nowrap(),
                )
            })
            .child(
                div()
                    .ml(u(inset - 8.))
                    .child(window_controls::buttons(self, window, cx)),
            );

        // `w_full`: в кэше шапка — корень раскладки своего места.
        div()
            .id("launcher-topbar")
            .window_control_area(WindowControlArea::Drag)
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .w_full()
            .h(theme.rem("l-topbar-h"))
            .flex()
            .items_center()
            .pr(u(gap - inset))
            .gap(u(gap))
            .child(brand)
            .child(tabs)
            .child(search)
            .child(right)
    }
}

#[cfg(test)]
mod tests {
    use super::grid_width;

    #[test]
    fn grid_width_rounds_up_to_four_with_a_hundredth_of_slack() {
        assert_eq!(grid_width(126.88), 128.);
        assert_eq!(grid_width(128.), 128.);
        assert_eq!(grid_width(128.03), 128.);
        assert_eq!(grid_width(128.1), 132.);
    }
}
