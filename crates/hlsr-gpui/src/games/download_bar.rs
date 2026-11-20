//! Нижняя панель «Игр» (`DownloadBar.vue` 0.4.9): блок статуса операции (значок, метка, полоса,
//! «сделано / всего», проценты, скорость и время; появляется сразу, уходит `db-status`), отмена,
//! «Версия сборки» ([`BuildSelector`]) над кнопками, меню игры ([`GameMenu`]) и главная кнопка
//! ([`play_button`]). Под панелью — `--l-footer-fill`, растворённый вверх и слева.
//!
//! Данные — `currentGame`, `currentBuild`, `currentDelivery` оболочки ([`ShellContext`]), тексты
//! и кнопка — [`download_bar::view`] (эпизоды Ghosting: `episodeButton`, отмена паузы эпизода,
//! размер нескачанных эпизодов у сборок, «Удалить эпизод» в меню); «готово» стоит секунду
//! ([`FinishHold`]). Нажатия — сценарии оболочки (`onPlay`, `onInstallEpisode`, …).

use super::build_selector::BuildSelector;
use super::game_menu::GameMenu;
use super::play_button::play_button;
use super::tool_panel::feather;
use crate::icons::fa_glyph;
use crate::layer::TipExt;
use crate::motion::{Timing, held, presence, tween};
use crate::paint::ring;
use crate::scale::{u, viewport, zoom};
use crate::shell::root::ShellContext;
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Context, Entity, FontWeight, IntoElement, Render, SharedString, Task, Window, div,
    prelude::*, relative, rgb, white,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::download_bar::{
    self, BarView, FINISH_HOLD, FinishHold, HoldTimer, INSTALL_EPISODE, Input,
};
use hlsr_ui::controllers::game_demo::{self, GameDemo};
use hlsr_ui::controllers::shell_flows::ShellFlows;
use hlsr_ui::services::Services;
use serde_json::Value;
use std::rc::Rc;

pub struct DownloadBar {
    hold: FinishHold,
    /// Таймер «готово» (`finishTimer`).
    hold_timer: Option<Task<()>>,
    menu: Entity<GameMenu>,
}

impl DownloadBar {
    pub fn new(services: &Services, cx: &mut Context<DownloadBar>) -> DownloadBar {
        let delivery = selection(cx).delivery;
        DownloadBar {
            hold: FinishHold::new(delivery.as_ref()),
            hold_timer: None,
            menu: cx.new(|cx| GameMenu::new(services, cx)),
        }
    }

    /// Наблюдатели `deliveryGameId` и `deliveryState`: «готово» ещё секунду.
    fn observe(&mut self, delivery: Option<&Value>, cx: &mut Context<Self>) {
        match self.hold.observe(delivery) {
            HoldTimer::Keep => {}
            HoldTimer::Clear => self.hold_timer = None,
            HoldTimer::Start => {
                self.hold_timer = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(FINISH_HOLD).await;
                    this.update(cx, |this, cx| {
                        this.hold.expire();
                        this.hold_timer = None;
                        cx.notify();
                    })
                    .ok();
                }));
            }
        }
    }
}

/// Что панель берёт у оболочки.
struct Selection {
    lang: Language,
    demo: GameDemo,
    build: String,
    delivery: Option<Value>,
}

/// `currentGame.demo`, `currentBuild`, `currentDelivery`; без оболочки — пусто.
fn selection(cx: &App) -> Selection {
    let Some(context) = ShellContext::get(cx) else {
        return Selection {
            lang: Language::En,
            demo: GameDemo::default(),
            build: String::new(),
            delivery: None,
        };
    };
    let flows = &context.flows;
    Selection {
        lang: context.shell.language(),
        demo: game_demo::demo_state(&flows.current_game_id()),
        build: flows.current_build(),
        // По `selectedGameId` без запасной игры, как в 0.4.9.
        delivery: flows.current_delivery(),
    }
}

/// Сценарий оболочки по нажатию (`$emit` панели).
fn emit(cx: &App, act: impl FnOnce(&ShellFlows)) {
    if let Some(context) = ShellContext::get(cx) {
        act(&context.flows);
    }
}

/// `onAction`: действие главной кнопки → обработчик оболочки (`@play`, `@install`, …).
fn press(action: &str, flows: &ShellFlows) {
    match action {
        "play" => flows.on_play(),
        "stop" => flows.on_stop_game(),
        "install" => flows.on_install(),
        INSTALL_EPISODE => flows.on_install_episode(),
        "update" => flows.on_apply_update(),
        "pause" => flows.on_pause_download(),
        "resume" | "retry" => flows.on_resume_operation(),
        "repair" => flows.on_menu_action("repair"),
        "explain" => flows.explain(),
        _ => {}
    }
}

/// Блок статуса (`.download-bar__status`).
fn status(view: &BarView, theme: &Theme, zoom: f32) -> impl IntoElement + use<> {
    let dim = theme.color("l-text-dim");
    let fill = theme.color("accent");
    let done = view.fill_done;
    let percent = view.percent as f32;
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap(u(20.))
        .child(
            div()
                .flex_none()
                .size(u(56.))
                .rounded(theme.rem("l-radius-sm"))
                .shadow(vec![ring(zoom, 1., theme.color("l-border-strong"))])
                .flex()
                .items_center()
                .justify_center()
                .text_color(dim)
                .child(fa_glyph("fas fa-arrow-down", 21.)),
        )
        .child(
            // Подпись 20 + 8 + полоса 8 + 8 + цифры 20 = 64.
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(u(8.))
                .child(
                    div()
                        .text_px(14., 20.)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.color("l-text"))
                        .whitespace_nowrap()
                        .child(SharedString::from(view.status_text.clone())),
                )
                .child(tween(
                    "download-fill",
                    percent,
                    Timing::med(),
                    move |percent| {
                        div()
                            .h(u(8.))
                            .rounded(u(4.))
                            .bg(white().opacity(0.09))
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(relative((percent / 100.).max(0.)))
                                    .rounded(u(4.))
                                    .bg(fill)
                                    .when(done, |el| el.opacity(0.35)),
                            )
                    },
                ))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .text_px(12.5, 20.)
                        .text_color(dim)
                        .child(SharedString::from(format!(
                            "{} / {}",
                            view.done_text, view.total_text
                        )))
                        .child(SharedString::from(view.right_meta.clone())),
                ),
        )
}

/// Отмена (`.download-bar__cancel`): 44 × 44, при наведении красноватая.
fn cancel(label: String, theme: &Theme, episode: Option<&'static str>) -> impl IntoElement {
    div()
        .id("download-cancel")
        .flex_none()
        .size(u(44.))
        .rounded(theme.rem("l-radius-sm"))
        .flex()
        .items_center()
        .justify_center()
        .text_color(theme.color("l-text-dim"))
        .cursor_pointer()
        .hover(|s| s.text_color(rgb(0xff8f84)).bg(white().opacity(0.06)))
        .tip(label)
        .button_text()
        .on_click(move |_, _, cx| emit(cx, |flows| flows.on_cancel_download(episode)))
        .child(fa_glyph("fas fa-times", 18.))
}

impl Render for DownloadBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let Selection {
            lang,
            demo,
            build,
            delivery,
        } = selection(cx);
        self.observe(delivery.as_ref(), cx);
        // `episodeCardShown`: карточка Ghosting с эпизодами — нужен их размер.
        emit(cx, |flows| {
            flows.set_episode_card_shown(!demo.episodes.is_empty())
        });
        let view = download_bar::view(&Input {
            lang,
            demo: &demo,
            build: &build,
            delivery: delivery.as_ref(),
            hold: &self.hold,
        });
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);

        let on_action = Rc::new(|id: &str, _: &mut Window, cx: &mut App| {
            emit(cx, |flows| flows.on_menu_action(id))
        });
        let (game_id, episode) = (view.game_id.clone(), view.menu_episode);
        self.menu.update(cx, |menu, cx| {
            menu.set(lang, game_id, episode, on_action, cx)
        });

        let button = view.button.clone();
        let play = play_button(&view.button, window, cx, move |_, cx| {
            if let Some(action) = button.pressed() {
                emit(cx, |flows| press(action, flows));
            }
        });
        let builds = BuildSelector {
            lang,
            builds: demo.builds,
            upcoming: demo.upcoming,
            badges: view.build_badges.clone(),
            value: build,
            on_select: Rc::new(|id: &str, _: &mut Window, cx: &mut App| {
                emit(cx, |flows| flows.on_select_build(id))
            }),
        };
        let (play_h, control_gap) = (theme.len("l-play-h"), theme.len("l-control-gap"));
        let controls = div()
            .relative()
            .flex()
            .items_center()
            .gap(u(control_gap))
            .when(view.can_cancel, |el| {
                el.child(cancel(
                    hlsr_ui::tr(lang, "#L_CANCEL", &[]),
                    &theme,
                    view.paused_episode,
                ))
            })
            // Один зазор между запуском, меню слева и выбором сборки сверху.
            .child(
                div()
                    .absolute()
                    .right_0()
                    .bottom(u(play_h + control_gap))
                    .child(crate::snap::target("build-select", builds)),
            )
            .child(self.menu.clone())
            .child(crate::snap::target("download-play", play));

        let shown = view.show_status;
        // Уходящий блок (`db-status`) — с тем, что было при показе (180 МБ, 10 %), а не уже «0 %».
        let status_view = held("db-status-view", shown, view.clone(), window, cx);
        let status_theme = theme.clone();
        let bar = div()
            .relative()
            .flex()
            .items_center()
            .justify_end()
            .gap(theme.rem("l-bottombar-gap"))
            .h(theme.rem("l-bottombar-h"))
            .pl(theme.rem("l-hero-pad"))
            .pr(theme.rem("l-edge-pad"))
            .child(
                presence("db-status", shown, "db-status", move || {
                    status(&status_view, &status_theme, zoom)
                })
                .flex_1()
                .min_w_0(),
            )
            .child(controls);

        div()
            .w_full()
            .relative()
            .flex_none()
            .child(feather(&theme, viewport(window).0))
            .child(bar)
    }
}

// Только Windows: тест ведёт настоящий `demo_state("218")`, а на Linux эпизодов Ghosting нет
// (`GHOSTING_EPISODES` пуст, как в 0.4.9) — «Episode One» не выбрать, тест упадёт.
#[cfg(all(test, windows))]
mod tests;
