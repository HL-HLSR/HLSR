//! Окно профиля (`ProfileModal.vue` 0.4.9). Открывается оболочкой (`profileOpen`,
//! `profileTarget` в [`ShellFlows`]); ключ окна — чей профиль и раздел при открытии: другой
//! профиль открывается заново. Свой профиль сразу на настройках — [`open_own_settings`] (меню
//! аккаунта «Настройки профиля», S4).
//!
//! Что внутри — по [`ProfileState`]: чужой профиль грузится или не загрузился, профиля нет,
//! вход через Steam (согласие, код, ошибки), иначе шапка ([`super::header`]), блокировка и
//! разделы «Профиль» ([`super::overview`]), «Достижения» (S10) и «Настройки» (S11) со своей
//! прокруткой и сменой `l-slide-*`. Подписчик — голубая бегущая рамка и свечение окна, 100 %
//! достижений — золотые ([`super::neon`]). Низкое окно (≤ 700 CSS px) — компактная шапка.
//! Слой окна — `on_top` (z 70), Esc и подложка закрывают, фокус при открытии — на крестике.

use super::achievements::AchievementsTab;
use super::neon::{COMPLETE, SUBSCRIBER, neon};
use super::overview::OverviewTab;
use super::runner_settings::RunnerSettingsTab;
use crate::dialogs::welcome::consent_text;
use crate::icons::{fa_glyph, icon, spinner};
use crate::image::Pictures;
use crate::kit::button::{faded, focus_handle, focus_visible, outline, white, window_under};
use crate::kit::{CheckLook, Checked, check, text_link};
use crate::layer::TipExt;
use crate::layer::{MenuNav, Modal, listen};
use crate::motion::{SLIDE_NEXT, SLIDE_PREV, Timing, hover, mix, swap};
use crate::paint::{ring, shadow};
use crate::scale::{u, viewport, zoom};
use crate::shell::root::ShellContext;
use crate::text::{MONO, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AnyView, App, Bounds, ClipboardItem, Context, Entity, FocusHandle, FontWeight,
    Global, IntoElement, Pixels, Render, Role, Task, WeakEntity, Window, div, prelude::*, rgb,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::bridge::Tr;
use hlsr_ui::controllers::profile::{Effect, Kind, NOTE_MS, ProfileState, ProfileView};
use hlsr_ui::controllers::shell_flows::ShellFlows;
use hlsr_ui::controllers::wall_clock::WallClock;
use hlsr_ui::services::Services;
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// Раздел окна.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Profile,
    Achievements,
    Settings,
}

/// Состояние открытого окна для вкладок «Достижения» (S10) и «Настройки» (S11): ответ сервера
/// как есть ([`ProfileState`]), окно (`applySnapshot`, `run` побочных действий), язык и часы.
pub struct ProfileContext {
    pub state: Rc<RefCell<ProfileState>>,
    pub modal: WeakEntity<ProfileModal>,
}

impl Global for ProfileContext {}

impl ProfileContext {
    pub fn get(cx: &App) -> Option<&ProfileContext> {
        cx.try_global::<ProfileContext>()
    }
}

/// Следующее открытие своего профиля — сразу на «Настройках» (`profileSection`).
#[derive(Default)]
struct PendingSettings(bool);

impl Global for PendingSettings {}

/// «Настройки профиля» меню аккаунта: свой профиль, раздел «Настройки» (`onAccountAction`
/// `profile-settings` 0.4.9).
pub fn open_own_settings(cx: &mut App) {
    cx.set_global(PendingSettings(true));
    if let Some(shell) = ShellContext::get(cx) {
        shell.flows.clone().open_runner_profile("");
    }
}

pub struct ProfileModal {
    services: Services,
    modal: Modal,
    close: FocusHandle,
    pub(super) state: Rc<RefCell<ProfileState>>,
    /// Ключ открытого окна: чей профиль и «сразу на настройках».
    key: Option<(String, bool)>,
    /// Растёт при каждом открытии: ответы прежнего окна не применяются.
    generation: u64,
    pub(super) section: Section,
    slide_next: bool,
    pub(super) more: MenuNav,
    pub(super) more_anchor: Rc<Cell<Bounds<Pixels>>>,
    hide_steam_id: bool,
    pub(super) pictures: Pictures,
    pub(super) language: Language,
    pub(super) wall: WallClock,
    pub(super) tr: Tr,
    /// Кэш снимка `ProfileState::view(...)`: бегущий неон и анимированный ник/рамка перерисовывают
    /// окно 240 раз в секунду, не трогая данные профиля — пересчитывать 40 достижений на каждый
    /// кадр не нужно.
    view_cache: Option<Rc<ProfileView>>,
    view_rev: u64,
    overview: Entity<OverviewTab>,
    achievements: Entity<AchievementsTab>,
    settings: Entity<RunnerSettingsTab>,
    settings_attached: (Option<u64>, bool),
    vote_note: Option<Task<()>>,
    success: Option<Task<()>>,
    _events: Task<()>,
}

fn tr_for(language: Language) -> Tr {
    Rc::new(move |key, args| hlsr_ui::tr(language, key, args))
}

impl ProfileModal {
    pub fn new(services: &Services, cx: &mut Context<ProfileModal>) -> ProfileModal {
        let close = cx.focus_handle().tab_stop(true);
        let mut modal = Modal::new("profile", cx).on_top();
        modal.focus_initial(close.clone());
        let pictures = cx.try_global::<Pictures>().cloned().unwrap_or_else(|| {
            tracing::warn!("[profile] нет Pictures: аватар и значки из админки не загрузятся");
            Pictures::new(hlsr_ui::images::ImageLoader::new(
                Rc::new(hlsr_ui::images::NoRemote),
                &*crate::env::GpuiEnv::new(cx),
            ))
        });
        cx.on_release({
            let pictures = pictures.clone();
            move |_, cx| pictures.release_scope(super::badges::SCOPE, cx)
        })
        .detach();
        let events = crate::bridge::listen(cx, services.subscribe(), |this, event, cx| {
            if !this.modal.is_open() {
                return;
            }
            let effects = match event.channel.as_str() {
                "runner-profile-state" => this.state.borrow_mut().profile_pushed(&event.payload),
                "runner-login-success" => this.state.borrow_mut().login_success(),
                _ => return,
            };
            this.run(effects, cx);
        });
        let state = Rc::new(RefCell::new(ProfileState::new("", false)));
        cx.set_global(ProfileContext {
            state: state.clone(),
            modal: cx.weak_entity(),
        });
        let language = crate::shell::root::language_of(services);
        ProfileModal {
            services: services.clone(),
            modal,
            close,
            state,
            key: None,
            generation: 0,
            section: Section::Profile,
            slide_next: true,
            more: MenuNav::default(),
            more_anchor: Rc::default(),
            hide_steam_id: false,
            pictures,
            language,
            wall: WallClock::system(),
            tr: tr_for(language),
            view_cache: None,
            view_rev: 0,
            overview: cx.new(|_| OverviewTab::new()),
            achievements: cx.new(|cx| AchievementsTab::new(services, cx)),
            settings: cx.new(|cx| RunnerSettingsTab::new(services, cx)),
            settings_attached: (None, false),
            vote_note: None,
            success: None,
            _events: events,
        }
    }

    pub(super) fn invalidate_view(&mut self) {
        self.view_cache = None;
        self.view_rev = self.view_rev.wrapping_add(1);
    }

    pub(super) fn view_rev(&self) -> u64 {
        self.view_rev
    }

    pub(super) fn view_snapshot(&self) -> Option<Rc<ProfileView>> {
        self.view_cache.clone()
    }

    fn current_view(&mut self) -> Rc<ProfileView> {
        if let Some(cached) = &self.view_cache {
            return cached.clone();
        }
        let view = Rc::new(self.state.borrow().view(self.language, &self.wall, &self.tr));
        self.view_cache = Some(view.clone());
        view
    }

    fn flows(cx: &App) -> Option<ShellFlows> {
        ShellContext::get(cx).map(|shell| shell.flows.clone())
    }

    pub(super) fn t(&self, key: &str) -> String {
        (self.tr)(key, &[])
    }

    /// Что оболочка хочет показать: открыто ли, чей профиль, «Скрыть SteamID».
    fn wanted(cx: &App) -> (bool, String, bool) {
        Self::flows(cx).map_or((false, String::new(), false), |flows| {
            flows.with_state(|s| {
                (
                    s.profile_open,
                    s.profile_target.clone(),
                    s.prefs.get("hideSteamId") == Some(&Value::Bool(true)),
                )
            })
        })
    }

    /// `v-if="profileOpen"` с `:key`: новое окно — `data()` и `mounted()` (`load`).
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (open, target, hide) = Self::wanted(cx);
        if open {
            let pending = cx.try_global::<PendingSettings>().is_some_and(|p| p.0);
            if pending {
                cx.set_global(PendingSettings(false));
            }
            let key = (target.clone(), pending && target.is_empty());
            if self.key.as_ref() != Some(&key) || !self.modal.is_open() {
                self.generation += 1;
                *self.state.borrow_mut() = ProfileState::new(&target, hide);
                self.section = if key.1 {
                    Section::Settings
                } else {
                    Section::Profile
                };
                self.slide_next = true;
                self.more = MenuNav::default();
                self.hide_steam_id = hide;
                self.overview.update(cx, |tab, _| tab.reset_scroll());
                self.achievements.update(cx, |tab, _| tab.reset_scroll());
                self.settings.update(cx, |tab, _| tab.reset_scroll());
                self.vote_note = None;
                self.success = None;
                self.key = Some(key);
                self.invalidate_view();
                let effects = self.state.borrow_mut().load();
                self.run(effects, cx);
                if !self.modal.is_open() {
                    self.modal.open(window, cx);
                }
            }
        } else if self.modal.is_open() {
            self.generation += 1;
            self.key = None;
            self.more.hide();
            self.vote_note = None;
            self.success = None;
            self.invalidate_view();
            self.modal.close(window, cx);
        }
        self.notify_tabs(cx);
        cx.notify();
    }

    fn notify_tabs(&self, cx: &mut Context<Self>) {
        gpui::App::notify(cx, self.overview.entity_id());
        gpui::App::notify(cx, self.achievements.entity_id());
        gpui::App::notify(cx, self.settings.entity_id());
    }

    /// `$emit("close")`.
    fn close(&mut self, cx: &mut Context<Self>) {
        if let Some(flows) = Self::flows(cx) {
            flows.close_profile();
        }
        cx.notify();
    }

    /// Вкладка `index` полосы.
    pub(super) fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        let section = match index {
            0 => Section::Profile,
            1 => Section::Achievements,
            _ => Section::Settings,
        };
        if section != self.section {
            let from = match self.section {
                Section::Profile => 0,
                Section::Achievements => 1,
                Section::Settings => 2,
            };
            self.slide_next = crate::kit::tabs::direction(from, index);
            self.section = section;
            cx.notify();
        }
    }

    /// `applySnapshot(data)` — ответ вкладки «Настройки» (`@updated`).
    pub fn apply_snapshot(&mut self, data: &Value, cx: &mut Context<Self>) {
        let effects = self.state.borrow_mut().apply_snapshot(Some(data));
        self.run(effects, cx);
    }

    /// Выполнить побочные действия логики окна.
    pub fn run(&mut self, effects: Vec<Effect>, cx: &mut Context<Self>) {
        for effect in effects {
            match effect {
                Effect::Invoke { tag, channel, args } => {
                    let reply = self.services.invoke(channel, args);
                    let generation = self.generation;
                    cx.spawn(async move |this, cx| {
                        let reply = reply.await;
                        this.update(cx, |this, cx| {
                            if this.generation != generation {
                                return;
                            }
                            let effects = this.state.borrow_mut().answer(tag, reply);
                            this.run(effects, cx);
                        })
                        .ok();
                    })
                    .detach();
                }
                Effect::ServerHideId(value) => {
                    if let Some(flows) = Self::flows(cx) {
                        flows.on_server_hide_steam_id(value);
                    }
                }
                Effect::Copy(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                Effect::Notify(key) => {
                    if let Some(shell) = ShellContext::get(cx) {
                        shell
                            .shell
                            .notify(&shell.shell.tr(key, &[]), "fas fa-check");
                    }
                }
                Effect::VoteNoteTimer(on) => {
                    self.vote_note = on
                        .then(|| Self::timer(cx, |this| this.state.borrow_mut().vote_note_done()));
                }
                Effect::LoginSuccessTimer(on) => {
                    self.success = on.then(|| {
                        Self::timer(cx, |this| this.state.borrow_mut().login_success_done())
                    });
                }
            }
        }
        self.invalidate_view();
        self.notify_tabs(cx);
        cx.notify();
    }

    /// Подсказка под рейтингом и подтверждение входа висят [`NOTE_MS`].
    fn timer(cx: &mut Context<Self>, done: fn(&mut ProfileModal)) -> Task<()> {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(NOTE_MS))
                .await;
            this.update(cx, |this, cx| {
                done(this);
                this.invalidate_view();
                cx.notify();
            })
            .ok();
        })
    }

    /// Кнопка `.runner-signin__button` (у главной — заливка `--accent`).
    #[allow(clippy::too_many_arguments)]
    fn signin_button(
        &self,
        id: &'static str,
        primary: bool,
        disabled: bool,
        content: AnyElement,
        on_click: impl Fn(&mut ProfileModal, &mut Context<ProfileModal>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>();
        let (accent, accent_hover) = (theme.color("accent"), theme.color("accent-hover"));
        let (strong, text) = (theme.color("l-border-strong"), theme.color("l-text"));
        let contrast = theme.color("accent-contrast-text");
        let radius = theme.len("l-radius-sm");
        let under = window_under(theme);
        let zoom = zoom(window);
        let focus = focus_handle(&id.into(), window, cx);
        let ring_on = focus_visible(&focus, window);
        let click = cx.listener(move |this, _, _, cx| on_click(this, cx));
        hover(id, Timing::fast(), move |t| {
            let t = if disabled { 0. } else { t };
            let (bg, color) = if primary {
                (mix(accent, accent_hover, t), contrast)
            } else {
                (white(0.05), text)
            };
            div()
                .id(id)
                .role(Role::Button)
                .button_text()
                .relative()
                .flex()
                .items_center()
                .gap(u(8.))
                .min_h(u(48.))
                .py(u(12.))
                .px(u(20.))
                .rounded(u(radius))
                .bg(bg)
                .when(!primary, |el| el.shadow(vec![ring(zoom, 1., strong)]))
                .text_px(13.5, 24.)
                .font_weight(FontWeight::BOLD)
                .text_color(color)
                .child(content)
                .when(disabled, |el| faded(el, 0.45, under))
                .when(!disabled, |el| {
                    el.track_focus(&focus).cursor_pointer().on_click(click)
                })
                .when(ring_on, |el| {
                    el.child(outline(zoom, radius, 2., 2., accent))
                })
        })
        .into_any_element()
    }

    /// Ссылка-кнопка `.runner-signin__privacy` (подчёркнутая, акцентом, 12,5 / 20 px).
    fn signin_link(
        &self,
        id: &'static str,
        label: String,
        key: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view = cx.entity().downgrade();
        let on_click = move |_: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| {
                let effects = this.state.borrow().open_external(key);
                this.run(effects, cx);
            })
            .ok();
        };
        div()
            .text_px(12.5, 20.)
            .child(text_link(id, label, on_click, window, cx))
            .into_any_element()
    }

    /// Согласие: флажок и текст с двумя ссылками (щелчок по ссылке флажок не трогает).
    fn consent(&self, theme: &Theme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let parts = [
            self.t("#L_WELCOME_CONSENT_A"),
            self.t("#L_WELCOME_PRIVACY"),
            self.t("#L_WELCOME_CONSENT_B"),
            self.t("#L_WELCOME_TERMS"),
        ];
        let view = cx.entity().downgrade();
        let text = consent_text(
            "profile-consent",
            parts,
            move |key, _, cx| {
                view.update(cx, |this, cx| {
                    let effects = this.state.borrow().open_external(key);
                    this.run(effects, cx);
                })
                .ok();
            },
            window,
            cx,
        );
        let agreed = self.state.borrow().consent();
        let on_change = cx.listener(|this, agreed: &bool, _, cx| {
            this.state.borrow_mut().set_consent(*agreed);
            this.invalidate_view();
            cx.notify();
        });
        crate::snap::target(
            "profile-consent",
            check(
                "profile-consent",
                if agreed { Checked::On } else { Checked::Off },
                CheckLook::Box,
            )
            .items_start()
            .gap(u(10.))
            // `<label>` 0.4.9 — блок во всю ширину до 420. Без ширины колонка `items_start` меряет
            // высоту строки по max-content (Taffy не жмёт её до max-width) — одна строка вместо двух.
            .w_full()
            .max_w(u(420.))
            .mt(u(24.))
            .text_px(12.5, 20.)
            .text_color(theme.color("l-text-dim"))
            .on_change(on_change)
            .child(text),
        )
    }

    /// Окно без профиля: чужой грузится/не загрузился, профиля нет, вход через Steam.
    fn guest(
        &mut self,
        view: &ProfileView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let (text, dim) = (theme.color("l-text"), theme.color("l-text-dim"));
        let steam = div()
            .size(u(56.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.rem("l-radius"))
            .bg(white(0.04))
            .shadow(vec![ring(zoom, 1., theme.color("l-border"))])
            .text_color(dim)
            .child(fa_glyph("fab fa-steam", 32.));
        // Поля CSS схлопываются (h2 16/12, p 12/12, код 20/20): здесь у каждого блока только
        // отступ сверху, равный большему из соседних полей.
        let h2 = |title: String| {
            div()
                .mt(u(16.))
                .text_px(19., 24.)
                .font_weight(FontWeight::BOLD)
                .text_color(text)
                .child(title)
        };
        let p = |line: String| {
            div()
                .mt(u(12.))
                .text_px(13., 24.)
                .text_color(dim)
                .child(line)
        };
        let body = div()
            .id("profile-guest")
            .p(u(32.))
            .flex()
            .flex_col()
            .items_start()
            .text_color(text);
        match view.kind {
            Kind::Pending => {
                let retry = view.failed.then(|| {
                    div().mt(u(16.)).child(
                        self.signin_button(
                            "profile-retry",
                            false,
                            false,
                            div()
                                .child(self.t("#L_PROFILE_VIEW_RETRY"))
                                .into_any_element(),
                            |this, cx| {
                                let effects = this.state.borrow_mut().load();
                                this.run(effects, cx);
                            },
                            window,
                            cx,
                        ),
                    )
                });
                body.items_center()
                    .text_center()
                    .child(steam)
                    .child(h2(self.t(if view.failed {
                        "#L_PROFILE_VIEW_FAILED"
                    } else {
                        "#L_PROFILE_VIEW_LOADING"
                    })))
                    .map(|el| {
                        if view.failed {
                            el.child(p(self.t("#L_PROFILE_VIEW_FAILED_HINT")))
                        } else {
                            el.child(
                                div()
                                    .mt(u(12.))
                                    .mb(u(4.))
                                    .child(spinner(20., theme.color("l-text-faint"))),
                            )
                        }
                    })
                    .children(retry)
                    .into_any_element()
            }
            Kind::Missing => body
                .child(steam)
                .child(h2(view.target.clone()))
                .child(p(self.t("#L_RUNNER_MISSING")))
                .into_any_element(),
            _ => {
                let lead = self.t(if view.guest_view {
                    "#L_RUNNER_VIEW_GUEST"
                } else {
                    "#L_RUNNER_LOGIN_TEXT"
                });
                let mut body = body
                    .child(steam)
                    .child(h2(self.t("#L_RUNNER_TITLE")))
                    .child(p(lead));
                if view.login_pending {
                    let cancel = self.signin_button(
                        "profile-login-cancel",
                        false,
                        false,
                        div().child(self.t("#L_RUNNER_CANCEL")).into_any_element(),
                        |this, cx| {
                            let effects = this.state.borrow().cancel_login();
                            this.run(effects, cx);
                        },
                        window,
                        cx,
                    );
                    let privacy = self.signin_link(
                        "profile-privacy",
                        self.t("#L_RUNNER_PRIVACY"),
                        "privacy",
                        window,
                        cx,
                    );
                    body = body
                        .child(p(self.t("#L_RUNNER_PENDING")))
                        .child(
                            div()
                                .mt(u(20.))
                                .font_family(MONO)
                                .text_px(24., 32.)
                                .font_weight(FontWeight::BOLD)
                                .text_color(text)
                                .child(spaced_code(&view.login_code)),
                        )
                        .child(
                            div()
                                .mt(u(20.))
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_x(u(24.))
                                .gap_y(u(16.))
                                .child(cancel)
                                .child(privacy),
                        );
                } else {
                    let consent = self.consent(&theme, window, cx);
                    let agreed = self.state.borrow().consent();
                    let sign_in = self.signin_button(
                        "profile-sign-in",
                        true,
                        !agreed,
                        div()
                            .flex()
                            .items_center()
                            .gap(u(8.))
                            // Цвет — от кнопки: выключенная выцветает вместе с ним.
                            .child(fa_glyph("fab fa-steam", 13.5))
                            .child(self.t("#L_RUNNER_LOGIN"))
                            .into_any_element(),
                        |this, cx| {
                            let effects = this.state.borrow_mut().sign_in();
                            this.run(effects, cx);
                        },
                        window,
                        cx,
                    );
                    body = body
                        .child(consent)
                        .child(
                            div()
                                .mt(u(16.))
                                .child(crate::snap::target("profile-sign-in", sign_in)),
                        )
                        .child(
                            div()
                                .max_w(u(420.))
                                .mt(u(16.))
                                .text_px(12., 18.)
                                .text_color(theme.color("l-text-faint"))
                                .child(self.t("#L_RUNNER_LOCAL_NOTE")),
                        );
                }
                if !view.login_error.is_empty() {
                    body = body.child(p(view.login_error.clone()).mb(u(12.)));
                }
                body.into_any_element()
            }
        }
    }

    /// Шапка, блокировка и разделы своего или чужого профиля.
    fn main(
        &mut self,
        view: &ProfileView,
        compact: bool,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let hero = self.hero(view, compact, width, window, cx);
        let ban = view.ban.clone().map(|ban| {
            div()
                .mx(u(32.))
                .mb(u(12.))
                .flex()
                .items_start()
                .gap(u(12.))
                .py(u(12.))
                .px(u(16.))
                .rounded(theme.rem("l-radius-sm"))
                .bg(gpui::rgba(0xff5c5c1f))
                .shadow(vec![ring(zoom, 1., gpui::rgba(0xff5c5c59).into())])
                .text_px(12.5, 20.)
                .text_color(theme.color("l-text"))
                .child(
                    div()
                        .mt(u(3.))
                        .text_color(rgb(0xff6b6b))
                        .child(fa_glyph("fas fa-ban", 12.5)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .child(div().font_weight(FontWeight::BOLD).child(ban.title.clone()))
                                .when(!ban.reason.is_empty(), |el| el.child(ban.reason.clone())),
                        )
                        .child(
                            div()
                                .text_color(theme.color("l-text-dim"))
                                .child(self.t("#L_MOD_BANNED_NOTE")),
                        ),
                )
        });
        let overview_tab: AnyView = self.overview.clone().into();
        let achievements: AnyView = self.achievements.clone().into();
        let settings: AnyView = self.settings.clone().into();
        let cache = crate::cache::on(cx);
        let section = match (self.section, view.tabs.len()) {
            // У чужого профиля настроек нет.
            (Section::Settings, len) if len < 3 => Section::Profile,
            (section, _) => section,
        };
        let pages = swap(
            "profile-page",
            section,
            if self.slide_next {
                SLIDE_NEXT
            } else {
                SLIDE_PREV
            },
            move |section: &Section| match section {
                Section::Profile => crate::cache::cached(
                    overview_tab.clone(),
                    gpui::StyleRefinement::default().size_full(),
                    cache,
                ),
                Section::Achievements => crate::cache::cached(
                    achievements.clone(),
                    gpui::StyleRefinement::default().size_full(),
                    cache,
                ),
                Section::Settings => crate::cache::cached(
                    settings.clone(),
                    gpui::StyleRefinement::default().size_full(),
                    cache,
                ),
            },
        )
        .flex_1()
        .min_h_0()
        .overflow_hidden();
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(hero)
            .children(ban.map(|ban| div().pt(u(12.)).child(ban)))
            .child(pages)
            .into_any_element()
    }
}

/// Код входа с разрядкой 4 px (`letter-spacing: 4px`): моноширинный, поэтому — пробелы
/// тонкой шириной не нужны; разрядка набирается полями букв.
fn spaced_code(code: &str) -> impl IntoElement {
    div()
        .flex()
        .children(code.chars().map(|ch| div().pr(u(4.)).child(ch.to_string())))
}

impl Render for ProfileModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let (open, target, hide) = Self::wanted(cx);
        let pending = cx.try_global::<PendingSettings>().is_some_and(|p| p.0);
        let stale = self.key.as_ref().is_some_and(|key| key.0 != target);
        if open != self.modal.is_open() || (open && (stale || pending)) {
            cx.defer_in(window, Self::sync);
        }
        if let Some(shell) = ShellContext::get(cx) {
            let language = shell.shell.language();
            if language != self.language {
                self.language = language;
                self.tr = tr_for(language);
                self.invalidate_view();
                self.notify_tabs(cx);
            }
        }
        if hide != self.hide_steam_id {
            self.hide_steam_id = hide;
            self.state.borrow_mut().set_hide_steam_id(hide);
            self.invalidate_view();
            gpui::App::notify(cx, self.settings.entity_id());
        }
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let (vw, vh) = viewport(window);
        let compact = vh <= 700.;
        let view = self.current_view();
        let guest = view.kind != Kind::Main;
        // Вкладка «Настройки» смонтирована, пока окно показывает свой профиль (`v-if`).
        let own = self.modal.is_open() && self.key.is_some() && !guest && view.tabs.len() >= 3;
        let attach_key = (own.then_some(self.generation), self.section == Section::Settings);
        if self.settings_attached != attach_key {
            self.settings_attached = attach_key;
            self.settings
                .update(cx, |tab, cx| tab.attach(attach_key.0, attach_key.1, cx));
        }
        let width = if view.narrow {
            (vw - 64.).min(560.)
        } else {
            (vw - 64.).min(1040.)
        };
        let content = if guest {
            self.guest(&view, window, cx)
        } else {
            self.main(&view, compact, width, window, cx)
        };
        let radius = theme.len("l-radius");
        // Liquid Glass: рамка подписчика или 100 % поверх `--lg-rim` и `--lg-shadow`.
        let mut glows = Vec::new();
        let mut shadows = vec![
            ring(zoom, 1., theme.color("l-modal-border")),
            shadow(zoom, 0., 24., 72., 0., gpui::black().opacity(0.6)),
        ];
        // Рамка подписчика или 100 %: свечение внутри (под содержимым) и снаружи.
        let glow = if view.complete {
            Some([
                (245, 197, 66, 0.22),
                (245, 197, 66, 0.4),
                (217, 140, 30, 0.2),
            ])
        } else if view.subscriber {
            Some([
                (56, 189, 248, 0.22),
                (56, 189, 248, 0.35),
                (59, 130, 246, 0.18),
            ])
        } else {
            None
        };
        if let Some([inner, near, far]) = glow {
            let color = |(r, g, b, a): (u8, u8, u8, f32)| -> gpui::Hsla {
                gpui::Rgba {
                    r: f32::from(r) / 255.,
                    g: f32::from(g) / 255.,
                    b: f32::from(b) / 255.,
                    a,
                }
                .into()
            };
            let mut inset = shadow(zoom, 0., 0., 16., 0., color(inner));
            inset.inset = true;
            glows = vec![
                inset,
                shadow(zoom, 0., 0., 18., 0., color(near)),
                shadow(zoom, 0., 0., 42., 0., color(far)),
            ];
            shadows.splice(0..0, glows.iter().cloned());
        }
        let neon_layer = (view.subscriber || view.complete).then(|| {
            neon(
                "profile-neon",
                if view.complete { COMPLETE } else { SUBSCRIBER },
                radius,
                zoom,
            )
        });
        let close_label = self.t("#L_CLOSE");
        let close_focus = self.close.clone();
        let close_ring = focus_visible(&self.close, window);
        let (dim, text, border, accent) = (
            theme.color("l-text-dim"),
            theme.color("l-text"),
            theme.color("l-border"),
            theme.color("l-accent"),
        );
        let radius_sm = theme.len("l-radius-sm");
        let on_close_click = cx.listener(|this, _, _, cx| this.close(cx));
        let close_top = if compact && !guest { 16. } else { 24. };
        let close = hover("profile-close", Timing::fast(), move |t| {
            div()
                .id("profile-close-button")
                .button_text()
                .on_click(on_close_click)
                .role(Role::Button)
                .aria_label(close_label.clone())
                .tip(close_label)
                .track_focus(&close_focus)
                .absolute()
                .top(u(close_top))
                .right(u(32.))
                .size(u(40.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(u(radius_sm))
                .bg(mix(gpui::black().opacity(0.32), white(0.1), t))
                .shadow(vec![ring(zoom, 1., border)])
                .text_color(mix(dim, text, t))
                .cursor_pointer()
                .child(fa_glyph("fas fa-times", 13.33))
                .when(close_ring, |el| {
                    el.child(outline(zoom, radius_sm, 2., 2., accent))
                })
        });
        // «Вход выполнен» поверх окна снизу по центру, 4 с.
        let welcome = self.state.borrow().login_success_shown().then(|| {
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(u(24.))
                .flex()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(8.))
                        .py(u(8.))
                        .pl(u(12.))
                        .pr(u(16.))
                        .rounded(theme.rem("l-radius"))
                        .bg(theme.color("l-modal-bg"))
                        .shadow(vec![
                            ring(zoom, 1., theme.color("l-modal-border")),
                            shadow(zoom, 0., 8., 24., 0., gpui::black().opacity(0.45)),
                        ])
                        .text_px(12.5, 20.)
                        .text_color(rgb(0x4ad07a))
                        .whitespace_nowrap()
                        .child(icon("fas fa-check-circle", 20., rgb(0x4ad07a).into()))
                        .child(self.t("#L_RUNNER_SUCCESS")),
                )
        });
        let title = self.t("#L_PROFILE_TITLE");
        let body = div()
            .id("profile-window")
            .role(Role::Dialog)
            .aria_label(title)
            .relative()
            .flex()
            .flex_col()
            .w(u(width))
            .map(|el| {
                if view.narrow {
                    el.max_h(u(vh - 64.))
                } else {
                    el.h(u((vh - 64.).min(720.)))
                }
            })
            .overflow_hidden()
            .bg(theme.color("l-modal-bg"))
            .rounded(u(radius))
            .shadow(shadows);
        let body = crate::liquid::glass_with(
            body,
            crate::liquid::Kind::Window,
            u(radius),
            gpui::Pixels::ZERO,
            glows,
            &theme,
            zoom,
        )
        .child(content)
        .children(neon_layer)
        .child(close)
        .children(welcome);
        let on_close = listen(cx, |this: &mut ProfileModal, _, cx| this.close(cx));
        self.modal
            .render(window, cx, on_close, move || body)
            .into_any_element()
    }
}
