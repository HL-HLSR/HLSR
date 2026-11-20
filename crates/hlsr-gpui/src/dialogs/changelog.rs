//! Список изменений (`ChangelogModal.vue` 0.4.9, данные `changelog.js` с выпусками 0.4.8 и
//! 0.4.9): крестик, заголовок, вкладки «Лаунчер» / «Игры» (вторая — когда пришли заметки сборок
//! `game-release-notes-all`), страницы со своей прокруткой и сменой `l-slide-*`; выпуск —
//! карточка (текущий и будущий — с отсветом акцента в углу и меткой), карточки поднимаются по
//! очереди (`l-rise-in`, шаг 40 мс). Открыли из карточки обновления — к выпуску прокручено, он
//! обведён. Esc, подложка и крестик закрывают. Логика — `controllers::shell_windows`.

use crate::icons::fa_glyph;
use crate::kit::button::{focus_visible, white};
use crate::kit::{Tab, TabStyle, outline, tabs};
use crate::layer::{Modal, anchor_probe, listen};
use crate::motion::{SLIDE_NEXT, SLIDE_PREV, Timing, apply, hover, keyframes, mix, swap};
use crate::paint::{ring, shadow};
use crate::scale::{u, viewport, zoom};
use crate::scroll::{Bar, Scroll, ScrollArea};
use crate::shell::root::ShellContext;
use crate::text::{TextExt, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Bounds, Context, FocusHandle, FontWeight, IntoElement, Pixels, Render, Role,
    Task, Window, div, prelude::*, px, svg,
};
use hlsr_ui::controllers::shell_flows::ShellFlows;
use hlsr_ui::controllers::shell_windows::{
    GameNotes, Release, all_releases, build_changelog, game_notes_list, game_title, is_focused,
    safe_extra_release,
};
use hlsr_ui::services::Services;
use std::cell::Cell;
use std::rc::Rc;

/// Рамка элемента прошлого кадра (px окна).
type Probe = Rc<Cell<Bounds<Pixels>>>;
/// Заметки игры: имя и версии с пунктами.
type Notes = (String, Vec<(String, Vec<String>)>);

/// Что рисует страница: выпуски лаунчера или заметки игр.
#[derive(Clone)]
struct Page {
    releases: Vec<Release>,
    notes: Vec<Notes>,
    initial: Option<String>,
    /// Отсвет текущего и будущего выпуска: рамки карточек по индексу.
    glow: Vec<Probe>,
    focus: Probe,
}

pub struct ChangelogModal {
    services: Services,
    modal: Modal,
    close: FocusHandle,
    tab: usize,
    slide_next: bool,
    releases: Vec<Release>,
    initial: Option<String>,
    notes: Vec<GameNotes>,
    scrolls: [Scroll; 2],
    glow: Vec<Probe>,
    focus: Probe,
    area: Probe,
    scrolled: bool,
    notes_task: Option<Task<()>>,
}

impl ChangelogModal {
    pub fn new(services: &Services, cx: &mut Context<ChangelogModal>) -> ChangelogModal {
        let close = cx.focus_handle().tab_stop(true);
        let mut modal = Modal::new("changelog", cx);
        modal.focus_initial(close.clone());
        ChangelogModal {
            services: services.clone(),
            modal,
            close,
            tab: 0,
            slide_next: true,
            releases: Vec::new(),
            initial: None,
            notes: Vec::new(),
            scrolls: Default::default(),
            glow: Vec::new(),
            focus: Rc::default(),
            area: Rc::default(),
            scrolled: false,
            notes_task: None,
        }
    }

    fn flows(cx: &App) -> Option<ShellFlows> {
        ShellContext::get(cx).map(|shell| shell.flows.clone())
    }

    fn tr(cx: &App, key: &str) -> String {
        ShellContext::get(cx).map_or_else(|| key.to_owned(), |shell| shell.shell.tr(key, &[]))
    }

    /// `v-if="changelogOpen"`: `data()` (выпуски не новее `app-version`, будущий из карточки
    /// обновления) и `mounted()` (заметки игр, фокус на крестике, прокрутка к выпуску).
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(flows) = Self::flows(cx) else {
            return;
        };
        let (open, initial, extra) = flows.with_state(|state| {
            (
                state.changelog_open,
                state.changelog_version.clone(),
                state.changelog_extra.clone(),
            )
        });
        if open && !self.modal.is_open() {
            let language = ShellContext::get(cx)
                .map_or("ru", |shell| shell.shell.language().code())
                .to_owned();
            let version = self.services.send_sync("app-version", Vec::new());
            let version = version.as_str().unwrap_or_default();
            self.releases = all_releases(
                build_changelog(&language, version),
                safe_extra_release(extra.as_ref()),
            );
            self.initial = initial;
            self.tab = 0;
            self.slide_next = true;
            self.notes = Vec::new();
            self.scrolls = Default::default();
            self.glow = self.releases.iter().map(|_| Probe::default()).collect();
            self.focus = Probe::default();
            self.scrolled = false;
            let reply = self.services.invoke("game-release-notes-all", Vec::new());
            self.notes_task = Some(cx.spawn(async move |this, cx| {
                // `catch` → `null` → не список: заметок нет.
                let notes = reply.await.map(|list| game_notes_list(&list));
                this.update(cx, |this, cx| {
                    this.notes = notes.unwrap_or_default();
                    cx.notify();
                })
                .ok();
            }));
            self.modal.open(window, cx);
        } else if !open && self.modal.is_open() {
            self.notes_task = None;
            self.modal.close(window, cx);
        }
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if let Some(flows) = Self::flows(cx) {
            flows.close_changelog();
        }
        cx.notify();
    }

    fn select(&mut self, tab: usize, cx: &mut Context<Self>) {
        if tab != self.tab {
            self.slide_next = crate::kit::tabs::direction(self.tab, tab);
            self.tab = tab;
            cx.notify();
        }
    }

    /// `scrollToInitial()`: выпуск, к которому открыли окно, — к верху страницы (один раз,
    /// когда его рамка уже есть).
    fn scroll_to_initial(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.scrolled || self.initial.is_none() || !self.modal.is_open() {
            return;
        }
        let (card, area) = (self.focus.get(), self.area.get());
        if card.size.height <= px(0.) || area.size.height <= px(0.) {
            crate::motion::monitor_notify(window, cx.entity_id());
            return;
        }
        self.scrolled = true;
        self.scrolls[0].scroll_to(card.origin.y - area.origin.y, false, window, cx);
        cx.notify();
    }
}

/// `.changelog__items`: точки акцента 6 px, 13 px / 22, между пунктами 6.
fn items(list: &[String], theme: &Theme) -> impl IntoElement + use<> {
    let dot = theme.color("l-accent").opacity(0.7);
    div()
        .flex()
        .flex_col()
        .gap(u(6.))
        .text_px(13., 22.)
        .text_color(theme.color("l-text-dim"))
        .children(list.iter().map(|item| {
            div()
                .relative()
                .pl(u(18.))
                .child(
                    div()
                        .absolute()
                        .left(u(2.))
                        .top(u(8.))
                        .size(u(6.))
                        .rounded_full()
                        .bg(dot),
                )
                .child(item.clone())
        }))
}

/// `.changelog__group-title`: 11 px / 700, разрядка .12em, заглавные, `--l-text-faint`.
fn group_title(text: &str, theme: &Theme) -> impl IntoElement + use<> {
    div().mb(u(8.)).child(spaced(
        text.to_uppercase(),
        11.,
        16.,
        FontWeight::BOLD,
        0.12,
        theme.color("l-text-faint"),
    ))
}

/// `.changelog__badge`: метка «Текущая» (акцент) или «Доступна» (#ffb37a).
fn badge(text: String, upcoming: bool, theme: &Theme, zoom: f32) -> impl IntoElement + use<> {
    let (bg, edge, color) = if upcoming {
        let warm = gpui::rgb(0xffb37a);
        (
            gpui::Hsla::from(warm).opacity(0.12),
            gpui::Hsla::from(warm).opacity(0.35),
            gpui::Hsla::from(warm),
        )
    } else {
        (
            theme.color("accent-soft"),
            theme.color("accent-border"),
            theme.color("accent"),
        )
    };
    div()
        .px(u(8.))
        .rounded(u(6.))
        .bg(bg)
        .shadow(vec![ring(zoom, 1., edge)])
        .child(spaced(
            text.to_uppercase(),
            10.5,
            20.,
            FontWeight::BOLD,
            0.08,
            color,
        ))
}

/// Карточка выпуска (`.changelog__release`): `title` — шапка (имя и метки), `body` — группы.
struct Card {
    id: String,
    index: usize,
    /// Текущий или будущий: отсвет в углу и кольцо 26 % акцента; рамка — для отсвета.
    lit: Option<Probe>,
    /// Выпуск, к которому открыли окно; рамка — для прокрутки.
    focus: Option<Probe>,
}

impl Card {
    fn render(
        self,
        title: AnyElement,
        body: Vec<AnyElement>,
        theme: &Theme,
        zoom: f32,
    ) -> impl IntoElement + use<> {
        let border = theme.color("l-border");
        let accent = theme.color("l-accent");
        let soft = theme.color("accent-soft");
        let radius = theme.len("l-radius");
        let Card {
            id,
            index,
            lit,
            focus,
        } = self;
        // Отсвет `radial-gradient(120% 140% at 100% 0%, акцент 12% → 0 на 60%)` по рамке
        // прошлого кадра.
        let glow = lit.as_ref().map(|probe| probe.get()).and_then(|bounds| {
            let (w, h) = (
                f32::from(bounds.size.width) / zoom,
                f32::from(bounds.size.height) / zoom,
            );
            (w > 0. && h > 0.).then(|| {
                svg()
                    .data(glow_svg(w, h, radius).as_bytes())
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(u(w))
                    .h(u(h))
                    .text_color(accent)
            })
        });
        let focused = focus.is_some();
        let inner = format!("{id}-card");
        let card = hover(
            gpui::ElementId::Name(id.clone().into()),
            Timing::fast(),
            move |t| {
                let (bg, shadows) = if lit.is_some() {
                    (white(0.03), vec![ring(zoom, 1., mix(border, accent, 0.26))])
                } else if t > 0. || !focused {
                    (
                        mix(white(0.03), white(0.04), t),
                        vec![ring(zoom, 1., mix(border, mix(border, accent, 0.3), t))],
                    )
                } else {
                    (
                        white(0.03),
                        vec![ring(zoom, 1., accent), shadow(zoom, 0., 0., 0., 3., soft)],
                    )
                };
                div()
                    .id(gpui::ElementId::Name(inner.into()))
                    .relative()
                    .flex_none()
                    .py(u(20.))
                    .px(u(24.))
                    .rounded(u(radius))
                    .bg(bg)
                    .shadow(shadows)
                    .children(glow)
                    .children(lit.clone().map(anchor_probe))
                    .children(focus.clone().map(anchor_probe))
                    .child(title)
                    .children(body)
            },
        );
        keyframes(
            gpui::ElementId::Name(id.into()),
            "changelog-release",
            move |frame| apply(div().child(card), frame),
        )
        .index(index.min(6))
    }
}

impl Page {
    /// Вкладка «Лаунчер»: выпуски лаунчера.
    fn launcher(&self, theme: &Theme, zoom: f32, current: &str, upcoming: &str) -> Vec<AnyElement> {
        self.releases
            .iter()
            .enumerate()
            .map(|(index, release)| {
                let focused = is_focused(self.initial.as_deref(), release);
                let lit = (release.current || release.upcoming)
                    .then(|| self.glow.get(index).cloned().unwrap_or_default());
                let title = div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(u(8.))
                    .mb(u(12.))
                    .child(
                        div()
                            .min_w_0()
                            .text_px(16., 24.)
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.color("l-text"))
                            .child(release.title.clone()),
                    )
                    .when(release.current, |el| {
                        el.child(badge(current.to_owned(), false, theme, zoom))
                    })
                    .when(release.upcoming, |el| {
                        el.child(badge(upcoming.to_owned(), true, theme, zoom))
                    })
                    .into_any_element();
                let body = release
                    .groups
                    .iter()
                    .enumerate()
                    .map(|(gi, group)| {
                        div()
                            .when(gi > 0, |el| el.mt(u(16.)))
                            .children(group.title.as_deref().map(|t| group_title(t, theme)))
                            .child(items(&group.items, theme))
                            .into_any_element()
                    })
                    .collect();
                Card {
                    id: format!("launcher-{index}"),
                    index,
                    lit,
                    focus: focused.then(|| self.focus.clone()),
                }
                .render(title, body, theme, zoom)
                .into_any_element()
            })
            .collect()
    }

    /// Вкладка «Игры»: заметки сборок по игре и версии (версия — плашкой).
    fn games(&self, theme: &Theme, zoom: f32) -> Vec<AnyElement> {
        self.notes
            .iter()
            .enumerate()
            .map(|(index, (title, versions))| {
                let head = div()
                    .mb(u(12.))
                    .text_px(16., 24.)
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.color("l-text"))
                    .child(title.clone())
                    .into_any_element();
                let body = versions
                    .iter()
                    .enumerate()
                    .map(|(vi, (version, list))| {
                        let chip = div()
                            .px(u(8.))
                            .py(u(2.))
                            .rounded(u(6.))
                            .bg(white(0.05))
                            .shadow(vec![ring(zoom, 1., theme.color("l-border"))])
                            .child(spaced(
                                version.to_uppercase(),
                                11.,
                                16.,
                                FontWeight::BOLD,
                                0.04,
                                theme.color("l-text-dim"),
                            ));
                        div()
                            .when(vi > 0, |el| el.mt(u(16.)))
                            .child(div().flex().mb(u(8.)).child(chip))
                            .child(items(list, theme))
                            .into_any_element()
                    })
                    .collect();
                Card {
                    id: format!("game-{index}"),
                    index,
                    lit: None,
                    focus: None,
                }
                .render(head, body, theme, zoom)
                .into_any_element()
            })
            .collect()
    }
}

impl Render for ChangelogModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let open = Self::flows(cx).is_some_and(|flows| flows.with_state(|s| s.changelog_open));
        if open != self.modal.is_open() {
            cx.defer_in(window, Self::sync);
        }
        let opened_at = |r: &Release| is_focused(self.initial.as_deref(), r);
        if self.modal.is_open()
            && !self.scrolled
            && self.tab == 0
            && self.releases.iter().any(opened_at)
        {
            cx.defer_in(window, Self::scroll_to_initial);
        }
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let (vw, vh) = viewport(window);
        let title = Self::tr(cx, "#L_CHANGELOG");
        let close_label = Self::tr(cx, "#L_CLOSE");
        let (current, upcoming) = (
            Self::tr(cx, "#L_CL_CURRENT"),
            Self::tr(cx, "#L_CL_UPCOMING"),
        );
        let has_notes = !self.notes.is_empty();
        let tab_labels = [
            (Self::tr(cx, "#L_CL_TAB_LAUNCHER"), "fas fa-rocket"),
            (Self::tr(cx, "#L_CL_TAB_GAMES"), "fas fa-gamepad"),
        ];
        // Пока отсвет рисуется по рамке прошлого кадра — ещё кадр после раскладки.
        let unlit = self
            .releases
            .iter()
            .zip(&self.glow)
            .any(|(r, probe)| (r.current || r.upcoming) && probe.get().size.width <= px(0.));
        if unlit && self.tab == 0 && self.modal.is_open() {
            crate::motion::monitor_notify(window, cx.entity_id());
        }
        let page = Page {
            releases: self.releases.clone(),
            notes: self
                .notes
                .iter()
                .map(|game| (game_title(game), game.versions.clone()))
                .collect(),
            initial: self.initial.clone(),
            glow: self.glow.clone(),
            focus: self.focus.clone(),
        };
        let scrolls = self.scrolls.clone();
        let area = self.area.clone();
        let page_theme = theme.clone();
        let pages = swap(
            "changelog-page",
            self.tab,
            if self.slide_next {
                SLIDE_NEXT
            } else {
                SLIDE_PREV
            },
            move |tab: &usize| {
                let theme = &page_theme;
                let cards = match *tab {
                    0 => page.launcher(theme, zoom, &current, &upcoming),
                    _ => page.games(theme, zoom),
                };
                ScrollArea::new(("changelog-scroll", *tab), &scrolls[(*tab).min(1)])
                    .bar(Bar::Thin)
                    .size_full()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(u(12.))
                            .pt(u(24.))
                            .px(u(32.))
                            .pb(u(32.))
                            .children(cards),
                    )
                    .when(*tab == 0, |el| el.child(anchor_probe(area.clone())))
            },
        )
        .flex_1()
        .min_h_0()
        .overflow_hidden();
        let zoom_ = zoom;
        let focus_color = theme.color("l-accent");
        let close_ring = focus_visible(&self.close, window);
        let close_focus = self.close.clone();
        let (dim, text, border) = (
            theme.color("l-text-dim"),
            theme.color("l-text"),
            theme.color("l-border"),
        );
        let radius_sm = theme.len("l-radius-sm");
        let on_close_click = cx.listener(|this, _, _, cx| this.close(cx));
        let close = hover("changelog-close", Timing::fast(), move |t| {
            div()
                .id("changelog-close-button")
                .button_text()
                .on_click(on_close_click)
                .role(Role::Button)
                .aria_label(close_label)
                .track_focus(&close_focus)
                .absolute()
                .top(u(24.))
                .right(u(32.))
                .size(u(40.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(u(radius_sm))
                .bg(mix(gpui::black().opacity(0.32), white(0.1), t))
                .shadow(vec![ring(zoom_, 1., border)])
                .text_color(mix(dim, text, t))
                .cursor_pointer()
                .child(fa_glyph("fas fa-times", 13.33))
                .when(close_ring, |el| {
                    el.child(outline(zoom_, radius_sm, 2., 2., focus_color))
                })
        });

        let tab_bar = has_notes.then(|| {
            div().mt(u(12.)).child(
                tabs(
                    "changelog-tabs",
                    tab_labels
                        .iter()
                        .map(|(label, glyph)| Tab::new(label.clone()).icon(glyph))
                        .collect(),
                    self.tab,
                    TabStyle::modal(&theme),
                )
                .label(title.clone())
                .on_select(cx.listener(|this, tab: &usize, _, cx| this.select(*tab, cx))),
            )
        });
        let head = div()
            .relative()
            .flex_none()
            .pt(u(24.))
            .px(u(32.))
            .when(!has_notes, |el| el.pb(u(24.)))
            .child(keyframes("changelog-title", "changelog-title", {
                let title = title.clone();
                let color = theme.color("l-text");
                move |frame| {
                    apply(
                        div().pr(u(56.)).child(spaced(
                            title,
                            24.,
                            40.,
                            FontWeight::BOLD,
                            0.01,
                            color,
                        )),
                        frame,
                    )
                }
            }))
            .children(tab_bar)
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(1.))
                    .bg(theme.color("l-modal-border")),
            );
        // .changelog__window: min(760px, 100% − 80px) × min(680px, 100% − 80px).
        let body = div()
            .id("changelog-window")
            .role(Role::Dialog)
            .aria_label(title)
            .relative()
            .flex()
            .flex_col()
            .w(u((vw - 80.).min(760.)))
            .h(u((vh - 80.).min(680.)))
            .overflow_hidden()
            .bg(theme.color("l-modal-bg"))
            .rounded(theme.rem("l-radius"))
            .shadow(vec![
                ring(zoom, 1., theme.color("l-modal-border")),
                shadow(zoom, 0., 24., 72., 0., gpui::black().opacity(0.6)),
            ]);
        let radius = theme.rem("l-radius");
        let body = crate::liquid::glass(
            body,
            crate::liquid::Kind::Window,
            radius,
            px(0.),
            &theme,
            zoom,
        )
        .child(head)
        .child(pages)
        .child(close);
        let on_close = listen(cx, |this: &mut ChangelogModal, _, cx| this.close(cx));
        self.modal
            .render(window, cx, on_close, move || body)
            .into_any_element()
    }
}

/// Отсвет текущего выпуска `radial-gradient(120% 140% at 100% 0%, акцент 12%, transparent 60%)`
/// маской SVG в CSS px карточки, со скруглением карточки: у прямоугольной маски угол с самым ярким
/// отсветом вылез бы за скругление.
fn glow_svg(w: f32, h: f32, radius: f32) -> String {
    format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{w}' height='{h}' viewBox='0 0 {w} {h}'>\
         <radialGradient id='g' gradientUnits='userSpaceOnUse' cx='0' cy='0' r='1' \
         gradientTransform='matrix({rx} 0 0 {ry} {w} 0)'>\
         <stop offset='0' stop-color='#fff' stop-opacity='0.12'/>\
         <stop offset='0.6' stop-color='#fff' stop-opacity='0'/></radialGradient>\
         <rect width='{w}' height='{h}' rx='{radius}' fill='url(#g)'/></svg>",
        rx = 1.2 * w,
        ry = 1.4 * h,
    )
}
