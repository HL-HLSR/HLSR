//! Карточка обновления лаунчера (`UpdateCard.vue`, в 0.4.9 без изменений): справа под шапкой
//! (`right 20, top 52`, 360 px, `z-index: 45` — над шапкой, под окнами), переход `l-pop`. Видна,
//! пока обновление доступно, качается, проверяется или скачано, а проверка вручную — и при
//! проверке и ошибке; «Позже» прячет её (`updateDismissed`) до нового выпуска. Значок, заголовок
//! и строка версии (или ошибки), полоса загрузки с объёмом, скоростью, временем и процентами,
//! заметки выпуска (до 400 знаков), кнопки по состоянию.
//!
//! Состояние и кнопки — сценарии оболочки (`ShellFlows`: `updater-state`, `startUpdateDownload`,
//! `confirmInstallNow`…), вид — `hlsr_ui::controllers::update_card::UpdateState`.

use super::Z_UPDATE_CARD;
use crate::icons::svg_icon;
use crate::kit::button::{child_id, white};
use crate::layer::TipExt;
use crate::motion::{Origin, POP, Push, Timing, hover, mix, presence, press, tween};
use crate::paint::shadow;
use crate::scale::{u, zoom};
use crate::shell::root::ShellContext;
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::{
    AnyElement, App, Context, Empty, FontWeight, Render, SharedString, Window, deferred, div,
    prelude::*, relative,
};
use hlsr_ui::controllers::shell_flows::ShellFlows;
use hlsr_ui::controllers::update_card::UpdateState;
use hlsr_ui::services::Services;

pub struct UpdateCard;

impl UpdateCard {
    pub fn new(_services: &Services, _cx: &mut Context<UpdateCard>) -> UpdateCard {
        UpdateCard
    }
}

type Action = fn(&ShellFlows);

/// Кнопка `.update-card__btn` (`primary` — заливка `--accent`, нажатие `scale(.97)`); `target` —
/// цель стенда.
fn button(
    target: &'static str,
    label: String,
    primary: bool,
    action: Action,
    theme: &Theme,
) -> AnyElement {
    let (bg, hover_bg, pressed, color, border) = if primary {
        (
            theme.color("accent"),
            theme.color("accent-hover"),
            theme.color("accent-pressed"),
            theme.color("accent-contrast-text"),
            gpui::transparent_black(),
        )
    } else {
        (
            white(0.05),
            white(0.1),
            white(0.1),
            theme.color("l-text"),
            theme.color("l-border-strong"),
        )
    };
    let element = div()
        .id(target)
        .flex_1()
        .flex()
        .justify_center()
        .px(u(14.))
        .py(u(9.))
        .border_1()
        .border_color(border)
        .rounded(theme.rem("l-radius-sm"))
        .bg(bg)
        .text_color(color)
        // `line-height: normal` Rubik 12,5 px.
        .text_px(12.5, 15.)
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .cursor_pointer()
        .button_text()
        .hover(move |s| s.bg(hover_bg))
        .active(move |s| s.bg(pressed))
        .on_click(move |_, _, cx| run(cx, action))
        .child(label);
    let element = press(child_id(&target.into(), "press"), Push::fast(0.97), element);
    crate::snap::target(target, element)
}

fn run(cx: &mut App, action: Action) {
    if let Some(shell) = ShellContext::get(cx) {
        action(&shell.flows.clone());
    }
}

impl Render for UpdateCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((status, dismissed)) = ShellContext::get(cx).map(|shell| {
            shell
                .flows
                .with_state(|s| (s.update_status.clone(), s.update_dismissed))
        }) else {
            return Empty.into_any_element();
        };
        // `<UpdateCard v-if="!updateDismissed">`: спрятанная уходит без перехода.
        if dismissed {
            return Empty.into_any_element();
        }
        let mut state = UpdateState::new();
        state.show(&status, false);
        let view = state.view(&super::tr(cx));
        let tr = super::tr(cx);
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let (text, dim, faint) = (
            theme.color("l-text"),
            theme.color("l-text-dim"),
            theme.color("l-text-faint"),
        );
        let accent = theme.color("accent");
        let percent = view.percent as f32;
        let visible = view.visible;
        let notes_scroll = crate::scroll::Scroll::retained("update-card-notes".into(), window, cx);
        let card = move || {
            let later = tr("#L_U_LATER", &[]);
            let head = div()
                .flex()
                .items_start()
                .gap(u(12.))
                .child(
                    div()
                        .flex_none()
                        .size(u(34.))
                        .rounded_full()
                        .bg(theme.color("accent-soft"))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(svg_icon("glyphs/update.svg", 18., accent)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(u(3.))
                        .child(
                            div()
                                .text_px(13.5, 13.5 * 1.35)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text)
                                .child(view.title.clone()),
                        )
                        .child(
                            div()
                                .text_px(11.5, 11.5 * 1.4)
                                .text_color(faint)
                                .child(view.subtitle.clone()),
                        ),
                )
                .when(view.can_dismiss, |el| {
                    // Цвет SVG не наследуется: наведение — переходом `--l-fast` здесь.
                    let dismiss = hover("update-card-dismiss-hover", Timing::fast(), move |k| {
                        div()
                            .id("update-card-dismiss")
                            .flex_none()
                            .size(u(28.))
                            .rounded(u(7.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(white(0.08 * k))
                            .cursor_pointer()
                            .tip(SharedString::from(later))
                            .button_text()
                            .on_click(|_, _, cx| run(cx, ShellFlows::dismiss_update))
                            .child(svg_icon("glyphs/dismiss.svg", 14., mix(faint, text, k)))
                    });
                    el.child(crate::snap::target("update-card-dismiss", dismiss))
                });
            let progress = view.downloading.then(|| {
                let fill = tween("update-card-fill", percent, Timing::med(), move |p| {
                    div()
                        .h_full()
                        .w(relative(p / 100.))
                        .rounded(u(3.))
                        .bg(accent)
                });
                div()
                    .mt(u(14.))
                    .child(
                        div()
                            .h(u(5.))
                            .rounded(u(3.))
                            .overflow_hidden()
                            .bg(white(0.09))
                            .child(fill),
                    )
                    .child(
                        div()
                            .mt(u(7.))
                            .flex()
                            .flex_wrap()
                            .gap(u(10.))
                            .text_px(11.5, 14.)
                            .text_color(dim)
                            .child(view.transferred.clone())
                            .child(view.speed.clone())
                            .when(!view.eta.is_empty(), |el| el.child(view.eta.clone()))
                            .child(
                                div()
                                    .ml_auto()
                                    .text_color(text)
                                    .child(format!("{}%", view.percent)),
                            ),
                    )
            });
            let notes = (!view.notes.is_empty()).then(|| {
                notes_scroll
                    .overflow(div().id("update-card-notes"))
                    .mt(u(12.))
                    .max_h(u(96.))
                    .text_px(12., 18.)
                    .text_color(dim)
                    .child(view.notes.clone())
            });
            let mut actions: Vec<AnyElement> = Vec::new();
            let mut add = |shown: bool, target, key: &str, primary, action: Action| {
                if shown {
                    actions.push(button(target, tr(key, &[]), primary, action, &theme));
                }
            };
            add(
                view.cancel,
                "update-card-cancel",
                "#L_CANCEL",
                false,
                ShellFlows::cancel_update_download,
            );
            add(
                view.download,
                "update-card-download",
                "#L_U_DOWNLOAD",
                true,
                ShellFlows::start_update_download,
            );
            add(
                view.whats_new,
                "update-card-whats-new",
                "#L_N_WHATS_NEW",
                false,
                ShellFlows::on_update_whats_new,
            );
            add(
                view.later,
                "update-card-later",
                "#L_U_LATER",
                false,
                ShellFlows::dismiss_update,
            );
            add(
                view.install_now,
                "update-card-install-now",
                "#L_U_RESTART_INSTALL",
                true,
                ShellFlows::confirm_install_now,
            );
            add(
                view.install_on_quit,
                "update-card-install-on-quit",
                "#L_U_INSTALL_ON_QUIT",
                false,
                ShellFlows::install_on_quit,
            );
            add(
                view.retry,
                "update-card-retry",
                "#UI_RETRY",
                false,
                ShellFlows::check_for_updates,
            );
            div()
                .id("update-card")
                .occlude()
                .w(u(360.))
                .p(u(16.))
                .bg(theme.color("l-surface-solid"))
                .border_1()
                .border_color(theme.color("l-border-strong"))
                .rounded(theme.rem("l-radius"))
                .shadow(vec![shadow(
                    zoom,
                    0.,
                    16.,
                    40.,
                    0.,
                    gpui::black().opacity(0.5),
                )])
                .child(head)
                .children(progress)
                .children(notes)
                // `&:empty { display: none }`.
                .when(!actions.is_empty(), |el| {
                    el.child(div().mt(u(14.)).flex().gap(u(8.)).children(actions))
                })
        };
        let card = presence("update-card-pop", visible, POP, card)
            .origin(Origin::CENTER)
            .absolute()
            .right(u(20.))
            .top(u(52.));
        deferred(card)
            .with_priority(Z_UPDATE_CARD)
            .into_any_element()
    }
}
