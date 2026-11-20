//! Запуск LiveSplit (`LiveSplitLaunchModal.vue` 0.4.9): для какой игры (на Windows — для её
//! оверлея, на Linux — в её префиксе Proton) и с какими сплитами; двойной щелчок по сплитам —
//! выбрать и запустить. Ответ — `{game, gameId, preset}` или отмена (Esc, подложка, «Отмена»).
//! Слой поверх окон (z-index 70); фокус — на «Отмене». Логика — [`LiveSplitDialog`].

use super::tr;
use crate::dialogs::{Btn, btn};
use crate::icons::fa_glyph;
use crate::kit::button::{child_id, focus_handle, focus_visible, outline, white};
use crate::layer::{Modal, card, listen};
use crate::motion::{Push, Timing, hover, mix, press};
use crate::scale::{u, viewport, zoom};
use crate::shell::root::{LiveSplitAsk, ShellRoot};
use crate::text::{RUBIK, TextExt, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, Context, ElementId, FocusHandle, FontWeight, IntoElement, Render,
    Role, Toggled, Window, div, prelude::*,
};
use hlsr_ui::controllers::shell_windows::LiveSplitDialog;

pub struct LiveSplitLaunchModal {
    modal: Modal,
    cancel: FocusHandle,
    ask: Option<LiveSplitAsk>,
    dialog: Option<LiveSplitDialog>,
    ring: bool,
}

impl LiveSplitLaunchModal {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let cancel = cx.focus_handle().tab_stop(true);
        let mut modal = Modal::new("livesplit-launch", cx).on_top();
        modal.focus_initial(cancel.clone());
        LiveSplitLaunchModal {
            modal,
            cancel,
            ask: None,
            dialog: None,
            ring: false,
        }
    }

    /// Новый запрос (прежний сценарий уже получил отказ) — открыть окно.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ask.is_some() {
            if !self.modal.is_open() {
                self.ring = crate::dialogs::opened_by_script();
            }
            self.modal.open(window, cx);
        } else {
            self.modal.close(window, cx);
        }
        cx.notify();
    }

    /// `$emit("resolve", …)`: ответ сценарию, окно уходит.
    fn resolve(&mut self, accept: bool, window: &mut Window, cx: &mut Context<Self>) {
        let pick = self.dialog.as_ref().and_then(LiveSplitDialog::accept);
        if accept && pick.is_none() {
            return;
        }
        if let Some(ask) = self.ask.take() {
            let _ = ask.reply.send(if accept { pick } else { None });
        }
        self.modal.close(window, cx);
        cx.notify();
    }

    fn pick_game(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(dialog) = &mut self.dialog {
            dialog.pick_game(id);
        }
        cx.notify();
    }

    fn pick_preset(&mut self, id: &str, accept: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = &mut self.dialog {
            dialog.preset_id = Some(id.to_owned());
        }
        if accept {
            self.resolve(true, window, cx);
        }
        cx.notify();
    }
}

/// Вариант `.ls-launch__choice` (`role="radio"`): поля 10 × 12, рамка `--l-border` (у выбранного
/// `--accent`), фон .04 (.08 при наведении), значок 13 px, 13,5 px; нажатие — `scale(.98)`.
fn choice(
    id: ElementId,
    selected: bool,
    label: AnyElement,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.global::<Theme>();
    let (accent, dim, text) = (
        theme.color("accent"),
        theme.color("l-text-dim"),
        theme.color("l-text"),
    );
    let border = if selected {
        accent
    } else {
        theme.color("l-border")
    };
    let radius = theme.len("l-radius-sm");
    let focus_color = theme.color("accent-focus");
    let zoom = zoom(window);
    let focus = focus_handle(&id, window, cx);
    let ring = focus_visible(&focus, window);
    let inner = id.clone();
    let press_id = child_id(&id, "press");
    hover(id, Timing::fast(), move |t| {
        let el = div()
            .id(inner)
            .role(Role::RadioButton)
            .button_text()
            .aria_toggled(if selected {
                Toggled::True
            } else {
                Toggled::False
            })
            .relative()
            .flex()
            .items_center()
            .gap(u(10.))
            .px(u(12.))
            .py(u(10.))
            .min_w_0()
            .border_1()
            .border_color(border)
            .rounded(u(radius))
            .bg(mix(white(0.04), white(0.08), t))
            .font_family(RUBIK)
            .text_px(13.5, 16.)
            .text_color(if selected { text } else { mix(dim, text, t) })
            .track_focus(&focus)
            .cursor_pointer()
            .on_click(on_click)
            .child(
                fa_glyph(
                    if selected {
                        "fas fa-dot-circle"
                    } else {
                        "far fa-circle"
                    },
                    13.,
                )
                .when(selected, |el| el.text_color(accent)),
            )
            .child(label)
            .when(ring, |el| {
                el.child(outline(zoom, radius, 2., 2., focus_color))
            });
        press(press_id, Push::fast(0.98), el)
    })
    .into_any_element()
}

impl Render for LiveSplitLaunchModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let asked = ShellRoot::get(cx).and_then(|root| root.read(cx).livesplit_request());
        if let Some(ask) = asked {
            // Окно заменили новым запросом: прежний сценарий получает отказ (`ask_livesplit`).
            self.dialog = Some(LiveSplitDialog::new(
                ask.request.games.clone(),
                ask.request.default_game.as_deref(),
                ask.request.chosen.clone(),
            ));
            self.ask = Some(ask);
        }
        if self.ask.is_some() != self.modal.is_open() {
            cx.defer_in(window, Self::sync);
        }
        let theme = cx.global::<Theme>().clone();
        let dim = theme.color("l-text-dim");
        let title = tr(cx, "#L_TOOL_LIVESPLIT_GAME_TITLE", &[]);
        // Строка подписи — 20 px (`line-height: 20px` у body в base.css 0.4.9).
        let label = |text: String| {
            div().mt(u(18.)).child(spaced(
                text.to_uppercase(),
                11.5,
                20.,
                FontWeight::SEMIBOLD,
                0.08,
                dim,
            ))
        };
        let view = cx.entity().downgrade();
        let mut games = div().flex().flex_col().gap(u(6.)).mt(u(8.));
        let mut presets = Vec::new();
        let mut can_launch = false;
        if let Some(dialog) = &self.dialog {
            can_launch = dialog.game().is_some();
            for game in &dialog.games {
                let (id, view) = (game.id.clone(), view.clone());
                games = games.child(choice(
                    ElementId::Name(format!("ls-game-{}", game.id).into()),
                    dialog.game_id.as_deref() == Some(game.id.as_str()),
                    div().child(game.label.clone()).into_any_element(),
                    move |_, _, cx| {
                        view.update(cx, |this, cx| this.pick_game(&id, cx)).ok();
                    },
                    window,
                    cx,
                ));
            }
            for preset in dialog.presets() {
                let id = preset
                    .get("id")
                    .map(hlsr_core::js::to_string)
                    .unwrap_or_default();
                let (main, small) =
                    LiveSplitDialog::preset_label(preset, |key, args| tr(cx, key, args));
                let label = div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(main)
                    .children(small.map(|small| {
                        div()
                            .mt(u(1.))
                            .text_px(11.5, 14.)
                            .text_color(dim)
                            .child(small)
                    }));
                let (pick, view) = (id.clone(), view.clone());
                presets.push(choice(
                    ElementId::Name(format!("ls-preset-{id}").into()),
                    dialog.preset_id.as_deref() == Some(id.as_str()),
                    label.into_any_element(),
                    move |event, window, cx| {
                        let accept = event.click_count() == 2;
                        view.update(cx, |this, cx| this.pick_preset(&pick, accept, window, cx))
                            .ok();
                    },
                    window,
                    cx,
                ));
            }
        }
        let splits = if presets.is_empty() {
            div()
                .mt(u(14.))
                .text_px(12.5, 12.5 * 1.45)
                .text_color(dim)
                .child(tr(cx, "#L_TOOL_SPLITS_NONE", &[]))
                .into_any_element()
        } else {
            // `grid-template-columns: repeat(2, minmax(0, 1fr))`, зазор 6.
            let mut rows = div().flex().flex_col().gap(u(6.)).mt(u(8.));
            let mut presets = presets.into_iter();
            while let Some(left) = presets.next() {
                let right = presets.next();
                rows = rows.child(
                    div()
                        .flex()
                        .gap(u(6.))
                        .child(div().flex_1().min_w_0().flex().flex_col().child(left))
                        .child(div().flex_1().min_w_0().flex().flex_col().children(right)),
                );
            }
            div()
                .child(label(tr(cx, "#L_TOOL_SPLITS", &[])))
                .child(rows)
                .into_any_element()
        };
        let text_key = if cfg!(target_os = "linux") {
            "#L_TOOL_LIVESPLIT_GAME_TEXT"
        } else {
            "#L_TOOL_LIVESPLIT_GAME_TEXT_WIN"
        };
        let actions = div()
            .flex()
            .gap(u(10.))
            .mt(u(22.))
            .child(
                btn("ls-cancel", tr(cx, "#L_CANCEL", &[]), Btn::Plain)
                    .focus(&self.cancel)
                    .ring(self.ring)
                    .on_click(cx.listener(|this, _, window, cx| this.resolve(false, window, cx))),
            )
            .child(crate::snap::target(
                "ls-launch",
                btn(
                    "ls-launch",
                    tr(cx, "#L_TOOL_LIVESPLIT_GAME_LAUNCH", &[]),
                    Btn::Primary,
                )
                .disabled(!can_launch)
                .on_click(cx.listener(|this, _, window, cx| this.resolve(true, window, cx))),
            ));
        let (vw, vh) = viewport(window);
        // .ls-launch__window: min(520px, 100% − 80px), до 100% − 60px с прокруткой, поля 24.
        let body = card(&theme, window)
            .id("ls-launch-window")
            .role(Role::Dialog)
            .aria_label(title.clone())
            .w(u((vw - 80.).min(520.)))
            .max_h(u(vh - 60.))
            .overflow_y_scroll()
            .p(u(24.))
            .child(spaced(
                title.to_uppercase(),
                15.,
                20.,
                FontWeight::SEMIBOLD,
                0.09,
                theme.color("l-text"),
            ))
            .child(
                div()
                    .mt(u(12.))
                    .text_px(13.5, 13.5 * 1.55)
                    .text_color(dim)
                    .child(tr(cx, text_key, &[])),
            )
            .child(label(tr(cx, "#L_TOOL_LIVESPLIT_GAME", &[])))
            .child(games)
            .child(splits)
            .child(actions);
        let on_close = listen(cx, |this: &mut LiveSplitLaunchModal, window, cx| {
            this.resolve(false, window, cx)
        });
        self.modal
            .render(window, cx, on_close, move || body)
            .into_any_element()
    }
}
