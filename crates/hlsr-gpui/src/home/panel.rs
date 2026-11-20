//! Раздел «Главная» (`HomePanel.vue` 0.4.9): заголовок, «Обновить» (своя стрелка-SVG крутится,
//! пока лента грузится, и встаёт только на целом обороте), заметка «⚠ устарело / не
//! загрузилось», вкладки «Лента / Рекорды / Игроки» (`l-ink`, страницы `l-slide-*`), лента
//! ([`super::feed`]) и просмотр картинок ([`super::viewer`]). Создаётся при каждом входе в
//! раздел (`v-if` без `keep-alive`).
//!
//! Логика — [`HomeState`]: каналы зовёт вид, ответы возвращает ей по тегам ([`Tag`]). Страницы
//! «Рекорды» ([`super::records`]) и «Игроки» ([`super::players`]) — S7 ([`PAGES`]).

use super::players::HomePlayers;
use super::records::Records;
use super::{feed, players, records, viewer};
use crate::env::GpuiEnv;
use crate::image::Pictures;
use crate::kit::button::{focus_handle, focus_visible, outline, white};
use crate::kit::{Tab, TabStyle, tabs};
use crate::layer::{Modal, TipExt};
use crate::motion::{
    AnimGate, SLIDE_NEXT, SLIDE_PREV, Timing, hover, keyframes, mix, registry, swap,
};
use crate::rows::Rows;
use crate::scale::{u, zoom};
use crate::scroll::Scroll;
use crate::shell::root::ShellContext;
use crate::text::{TextExt, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AnyView, App, AppContext, Context, ElementId, Entity, FocusHandle, FontWeight,
    Hsla, IntoElement, Render, Role, Task, Transformation, WeakEntity, Window, div, prelude::*,
    radians, rgba, svg,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::bridge::Tr;
use hlsr_ui::controllers::home::{Effect, HomeState, HomeView, TABS, Tag, turn_remaining};
use hlsr_ui::controllers::wall_clock::WallClock;
use hlsr_ui::images::{ImageLoader, NoRemote};
use hlsr_ui::richtext::Doc;
use hlsr_ui::services::{Reply, Services};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Область картинок «Главной» в загрузчике: уходит вместе с разделом.
pub(super) const SCOPE: &str = "home";

/// `@keyframes home-spin` 0.8 с (`.home-panel__refresh-icon.spinning`).
const SPIN: &str = "home-refresh";

/// Своя стрелка 0.4.9: дуга с центром в середине viewBox — при повороте не «гуляет».
const REFRESH_ICON: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3"/><path d="M19.8 4.2v4.6h-4.6"/></svg>"##;

/// Страница вкладки — по порядку [`TABS`].
type PageFn = fn(&mut HomePanel, &mut Window, &mut Context<HomePanel>) -> AnyElement;

/// «Лента» — S6; «Рекорды» и «Игроки» — S7.
const PAGES: [PageFn; 3] = [feed::page, records::page, players::page];

/// Страница вкладки — свой вид, чтобы `l-slide-*` держал уходящую и входящую разом; рисует её
/// панель (состояние «Главной» одно на все вкладки).
struct Page {
    panel: WeakEntity<HomePanel>,
    build: PageFn,
}

impl Render for Page {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let build = self.build;
        self.panel
            .update(cx, |panel, cx| build(panel, window, cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}

pub struct HomePanel {
    services: Services,
    pub(super) state: HomeState,
    pub(super) pictures: Pictures,
    /// Прокрутка ленты (`v-show`: живёт при смене вкладок).
    pub(super) feed: Scroll,
    /// Окно видимости постов ленты.
    pub(super) feed_rows: Rows,
    /// Прокрутка и смена игры «Рекордов».
    pub(super) records: Records,
    /// `<HomePlayers ref="players">`: вкладку и вход отдаёт [`HomePanel::sync_players`].
    pub(super) players: Entity<HomePlayers>,
    pages: [AnyView; 3],
    pub(super) viewer: Modal,
    /// «×» окна просмотра: фокус при открытии.
    pub(super) viewer_close: FocusHandle,
    /// Начало оборотов стрелки и номер круга (новый круг — новая анимация с нуля).
    spin: Option<(Instant, usize)>,
    spins: usize,
    /// Разобранные тела постов: id → (текст, документ). Наведение перерисовывает всю ленту
    /// каждый кадр — Markdown разбирается один раз на текст.
    docs: HashMap<String, (String, Rc<Doc>)>,
    /// Вид последнего кадра панели ([`HomePanel::frame_view`]).
    shown: Option<Rc<HomeView>>,
    _events: Task<()>,
}

impl HomePanel {
    pub fn new(services: &Services, cx: &mut Context<HomePanel>) -> HomePanel {
        let events =
            crate::bridge::listen(cx, services.subscribe(), |this: &mut Self, event, cx| {
                if event.channel == "runner-profile-state" {
                    let effects = this.state.profile_state(&event.payload);
                    this.run(effects, cx);
                    this.sync_players(cx);
                }
            });
        let pictures = cx.try_global::<Pictures>().cloned().unwrap_or_else(|| {
            tracing::warn!("[home] нет Pictures: аватары и картинки новостей не загрузятся");
            Pictures::new(ImageLoader::new(Rc::new(NoRemote), &*GpuiEnv::new(cx)))
        });
        cx.on_release(|this: &mut Self, cx| {
            // `beforeUnmount`: окно просмотра уходит вместе с разделом.
            this.state.unmounted();
            this.pictures.release_scope(SCOPE, cx)
        })
        .detach();
        let weak = cx.entity().downgrade();
        let pages = PAGES.map(|build| {
            let panel = weak.clone();
            AnyView::from(cx.new(|_| Page { panel, build }))
        });
        let players = {
            let pictures = pictures.clone();
            cx.new(|cx| HomePlayers::new(services, pictures, cx))
        };
        let viewer_close = cx.focus_handle().tab_stop(true);
        // `.news-viewer`: подложка rgba(5, 8, 11, .88) и `backdrop-filter: blur(8px)` (решение
        // владельца 17.11: размытие под панелями как в 0.4.9).
        let mut viewer_modal = Modal::new("news-viewer", cx)
            .on_top()
            .scrim(rgba(0x05080be0).into())
            .blur();
        viewer_modal.focus_initial(viewer_close.clone());
        let mut panel = HomePanel {
            services: services.clone(),
            state: HomeState::new(WallClock::system()),
            pictures,
            feed: Scroll::new(),
            feed_rows: Rows::default(),
            records: Records::default(),
            players,
            pages,
            viewer: viewer_modal,
            viewer_close,
            spin: None,
            spins: 0,
            docs: HashMap::new(),
            shown: None,
            _events: events,
        };
        let effects = panel.state.mounted();
        panel.started(cx);
        panel.run(effects, cx);
        panel
    }

    pub(super) fn language(cx: &App) -> Language {
        ShellContext::get(cx).map_or(Language::Ru, |shell| shell.shell.language())
    }

    pub(super) fn tr(cx: &App) -> Tr {
        let language = Self::language(cx);
        Rc::new(move |key: &str, args: &[&str]| hlsr_ui::tr(language, key, args))
    }

    pub(super) fn view(&self, cx: &App) -> HomeView {
        self.state.view(Self::language(cx), &Self::tr(cx))
    }

    /// Вид этого кадра для страницы вкладки. Страница рисуется только внутри кадра панели
    /// (уведомлённая страница помечает грязными и предков), поэтому вид, собранный в
    /// [`Render::render`] панели, — вид этого же кадра: всё состояние в JSON, и сборка на
    /// каждую страницу заново стоила кадру вдвое-втрое.
    pub(super) fn frame_view(&self, cx: &App) -> Rc<HomeView> {
        self.shown.clone().unwrap_or_else(|| Rc::new(self.view(cx)))
    }

    /// Тело поста `id`, разобранное из `body` (`news-markdown.js`); тот же текст — прежний разбор.
    pub(super) fn doc(&mut self, id: &str, body: &str) -> Rc<Doc> {
        if let Some((known, doc)) = self.docs.get(id)
            && known == body
        {
            return doc.clone();
        }
        let doc = Rc::new(hlsr_ui::markdown::news(body));
        self.docs
            .insert(id.to_owned(), (body.to_owned(), doc.clone()));
        doc
    }

    /// Побочные действия логики: каналы — задачей с ответом по тегу.
    pub(super) fn run(&mut self, effects: Vec<Effect>, cx: &mut Context<Self>) {
        for effect in effects {
            match effect {
                Effect::Invoke { tag, channel, args } => {
                    let reply = self.services.invoke(channel, args);
                    cx.spawn(async move |this, cx| {
                        let reply = reply.await;
                        this.update(cx, |this, cx| this.answer(tag, reply, cx)).ok();
                    })
                    .detach();
                }
                Effect::OpenProfile(id) => open_profile(&id, cx),
                // Окно просмотра открывает и закрывает вид: нужен `Window`.
                Effect::Overlay(_) => {}
            }
        }
        cx.notify();
    }

    fn answer(&mut self, tag: Tag, reply: Reply, cx: &mut Context<Self>) {
        match tag {
            Tag::Profile => {
                if let Ok(state) = reply {
                    let effects = self.state.profile_state(&state);
                    self.run(effects, cx);
                    self.sync_players(cx);
                }
            }
            Tag::News => {
                let effects = self.state.news_reply(reply);
                self.loaded(cx);
                self.run(effects, cx);
            }
            Tag::Mine(revision, request) => self.state.mine_reply(revision, request, reply),
            Tag::React(id, key, revision) => self.state.react_reply(&id, &key, revision, reply),
            Tag::Link(id) => self.state.link_reply(&id, reply),
            Tag::Ignored => {}
        }
        cx.notify();
    }

    /// Загрузка началась: стоявшая стрелка начинает новый круг.
    fn started(&mut self, cx: &App) {
        if self.spin.is_none() && self.state.is_spinning() {
            self.spins += 1;
            self.spin = Some((cx.background_executor().now(), self.spins));
        }
    }

    /// `watch loading`: без анимации стрелка встаёт сразу, иначе — на конце оборота
    /// (`animationiteration`), если к нему новая загрузка не началась.
    fn loaded(&mut self, cx: &mut Context<Self>) {
        if self.state.is_loading() {
            return;
        }
        if !registry().keyframes_moving(SPIN, AnimGate::still(cx)) {
            self.stop(cx);
            return;
        }
        let now = cx.background_executor().now();
        let elapsed = self.spin.map_or(0., |(since, _)| {
            now.saturating_duration_since(since).as_secs_f64() * 1000.
        });
        let wait = Duration::from_secs_f64(turn_remaining(elapsed) / 1000.);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| this.stop(cx)).ok();
        })
        .detach();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.state.stop_spinning();
        if !self.state.is_spinning() {
            self.spin = None;
        }
        cx.notify();
    }

    /// «Обновить»: лента заново (`force`). На вкладке игроков — и их список.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let effects = self.state.load(true);
        self.started(cx);
        self.run(effects, cx);
        if self.state.tab() == "players" {
            self.players.update(cx, |players, cx| players.refresh(cx));
        }
    }

    fn select_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(tab) = TABS.get(index) {
            self.state.select_tab(tab);
            self.sync_players(cx);
            cx.notify();
        }
    }

    /// Пропсы `<HomePlayers>`: `active`, `signed-in`, `me`.
    fn sync_players(&mut self, cx: &mut Context<Self>) {
        let active = self.state.tab() == "players";
        let (signed_in, me) = (self.state.is_signed_in(), self.state.me());
        self.players
            .update(cx, |players, cx| players.sync(active, signed_in, &me, cx));
    }

    /// Плитка скриншота: просмотр с этой картинки.
    pub(super) fn show_image(
        &mut self,
        post: &str,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .state
            .show_image(post, index)
            .contains(&Effect::Overlay(true))
        {
            self.viewer.open(window, cx);
        }
        cx.notify();
    }

    pub(super) fn close_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.close_image().contains(&Effect::Overlay(false)) {
            self.viewer.close(window, cx);
        }
        cx.notify();
    }

    /// `.home-panel__refresh`: 32 × 32, `--l-text-dim`, наведение — `--l-text` и подложка .06;
    /// пока лента грузится — `disabled`.
    fn refresh_button(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>();
        let (dim, text, accent) = (
            theme.color("l-text-dim"),
            theme.color("l-text"),
            theme.color("l-accent"),
        );
        let radius = theme.len("l-radius-sm");
        let id = ElementId::from("home-refresh");
        let focus = focus_handle(&id, window, cx);
        let ring_on = focus_visible(&focus, window);
        let zoom = zoom(window);
        let disabled = self.state.is_loading();
        let round = self
            .spin
            .filter(|_| self.state.is_spinning())
            .map(|(_, round)| round);
        let label = Self::tr(cx)("#L_HOME_REFRESH", &[]);
        let on_click = cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
            if !this.state.is_loading() {
                this.refresh(cx);
            }
        });
        let inner = id.clone();
        let button = hover(id, Timing::fast(), move |t| {
            let t = if disabled { 0. } else { t };
            div()
                .id(inner)
                .role(Role::Button)
                .button_text()
                .aria_label(label.clone())
                .tip(label)
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(u(32.))
                .rounded(u(radius))
                .bg(white(0.06 * t))
                .track_focus(&focus)
                .child(refresh_icon(round, mix(dim, text, t)))
                .when(!disabled, |el| el.cursor_pointer().on_click(on_click))
                .when(ring_on, |el| {
                    el.child(outline(zoom, radius, 2., 2., accent))
                })
        });
        crate::snap::target("home-refresh", button)
    }
}

/// Стрелка `color` (SVG цвет не наследует); `round` — идёт загрузка: `home-spin` с начала круга.
fn refresh_icon(round: Option<usize>, color: Hsla) -> AnyElement {
    let arrow = move |degrees: f32| {
        svg()
            .data(REFRESH_ICON)
            .flex_none()
            .size(u(18.))
            .text_color(color)
            .with_transformation(Transformation::rotate(radians(degrees.to_radians())))
    };
    match round {
        Some(round) => keyframes(("home-spin", round), SPIN, move |frame| {
            arrow(frame.rotation as f32)
        })
        .into_any_element(),
        None => arrow(0.).into_any_element(),
    }
}

/// `$emit('open-profile', id)`: пустой — вход или свой профиль (`openRunnerProfile`).
pub(super) fn open_profile(id: &str, cx: &App) {
    if let Some(shell) = ShellContext::get(cx) {
        shell.flows.open_runner_profile(id);
    }
}

impl Render for HomePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let theme = cx.global::<Theme>().clone();
        let view = Rc::new(self.view(cx));
        self.shown = Some(view.clone());
        let tr = Self::tr(cx);
        let dim = theme.color("l-text-dim");

        let head = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(u(2.))
            .mb(u(16.))
            .child(spaced(
                tr("#L_HOME_TITLE", &[]).to_uppercase(),
                32.,
                40.,
                FontWeight::BOLD,
                0.08,
                gpui::white(),
            ))
            .child(self.refresh_button(window, cx));

        let note = (!view.note.is_empty()).then(|| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(u(8.))
                .mt(u(-16.))
                .mb(u(24.))
                .text_px(14., 20.)
                .text_color(dim)
                .child(
                    crate::icons::fa_glyph("fas fa-exclamation-triangle", 14.)
                        .text_color(theme.color("l-orange")),
                )
                .child(view.note.clone())
        });

        let labels = ["#L_HOME_FEED", "#L_HOME_RECORDS_TAB", "#L_PLAYERS_TAB"];
        let bar = tabs(
            "home-tabs",
            labels.iter().map(|key| Tab::new(tr(key, &[]))).collect(),
            view.tab as usize,
            TabStyle::home(&theme),
        )
        .label(tr("#L_HOME_TITLE", &[]))
        .on_select(cx.listener(|this, index: &usize, _, cx| this.select_tab(*index, cx)));

        let pages = self.pages.clone();
        let slide = if view.slide_next {
            SLIDE_NEXT
        } else {
            SLIDE_PREV
        };
        let body = swap(
            "home-pages",
            view.tab as usize,
            slide,
            move |tab: &usize| pages[*tab].clone(),
        )
        .flex_1()
        .min_h_0()
        .overflow_hidden();

        let on_close = crate::layer::listen(cx, |this, window, cx| this.close_image(window, cx));
        let content = viewer::content(self, window, cx);
        let modal = self.viewer.render(window, cx, on_close, move || content);

        div()
            .id("home-panel")
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .pt(u(theme.len("l-topbar-h") + 32.))
            .px(u(theme.len("l-hero-pad")))
            .child(head)
            .children(note)
            .child(div().flex_none().mb(u(20.)).child(bar))
            .child(body)
            .child(modal)
    }
}

#[cfg(test)]
mod tests;
