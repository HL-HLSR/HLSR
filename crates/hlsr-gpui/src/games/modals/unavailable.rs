//! «Почему игра недоступна» (`UnavailableModal.vue` 0.4.9): значок и тексты причины главной
//! кнопки, «Подробности» с кодом причины, кнопки действий. 0.4.9: без ремонта по файлам у игры
//! (`fileRepair` статуса) «Починить» нет. Esc, подложка и крестик закрывают; фокус — на крестике
//! (первая кнопка окна).

use super::{GameButton, Look, flows, game_button, header, tr};
use crate::icons::fa_glyph;
use crate::layer::{Modal, card, listen};
use crate::motion::{Timing, hover, mix};
use crate::scale::{u, viewport};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::{
    Context, FocusHandle, FontWeight, IntoElement, Render, Role, Window, div, prelude::*, white,
};
use hlsr_core::js;
use hlsr_ui::controllers::shell_windows::{unavailable_actions, unavailable_view};
use std::rc::Rc;

pub struct UnavailableModal {
    modal: Modal,
    close: FocusHandle,
    /// `<details>` раскрыт.
    details: bool,
    ring: bool,
}

impl UnavailableModal {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let close = cx.focus_handle().tab_stop(true);
        let mut modal = Modal::new("unavailable", cx);
        modal.focus_initial(close.clone());
        UnavailableModal {
            modal,
            close,
            details: false,
            ring: false,
        }
    }

    fn open(cx: &gpui::App) -> bool {
        flows(cx).is_some_and(|flows| flows.with_state(|state| state.unavailable_open))
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if Self::open(cx) {
            if !self.modal.is_open() {
                self.details = false;
                self.ring = crate::dialogs::opened_by_script();
            }
            self.modal.open(window, cx);
        } else {
            self.modal.close(window, cx);
        }
        cx.notify();
    }

    fn close(cx: &mut gpui::App) {
        if let Some(flows) = flows(cx) {
            flows.close_window("unavailable");
        }
    }
}

impl Render for UnavailableModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if Self::open(cx) != self.modal.is_open() {
            cx.defer_in(window, Self::sync);
        }
        let theme = cx.global::<Theme>().clone();
        let (reason, file_repair) = flows(cx).map_or_else(
            || ("UNKNOWN_UNAVAILABLE".to_owned(), false),
            |flows| {
                let file_repair = flows
                    .current_delivery()
                    .and_then(|status| status.get("fileRepair").cloned())
                    .is_some_and(|value| js::truthy(&value));
                (flows.unavailable_reason(), file_repair)
            },
        );
        let view = unavailable_view(&reason);
        let title = tr(cx, "#L_UA_TITLE", &[]);
        let (dim, faint) = (theme.color("l-text-dim"), theme.color("l-text-faint"));
        let on_close: super::OnClick = Rc::new(|_, _, cx| Self::close(cx));
        let head = header(&title, &self.close, self.ring, on_close, window, cx);
        let actions = unavailable_actions(&reason, file_repair)
            .into_iter()
            .map(|action| {
                let id = action.id;
                game_button(
                    gpui::ElementId::Name(format!("unavailable-{id}").into()),
                    Some(action.icon),
                    tr(cx, action.key, &[]),
                    Look::Settings,
                )
                .on_click(move |_, _, cx| {
                    if let Some(flows) = flows(cx) {
                        flows.on_unavailable_action(id);
                    }
                })
            })
            .collect::<Vec<GameButton>>();
        let open = self.details;
        let summary_label = tr(cx, "#L_UA_DETAILS", &[]);
        let toggle = cx.listener(|this: &mut Self, _: &gpui::ClickEvent, _, cx| {
            this.details = !this.details;
            cx.notify();
        });
        let summary = hover("unavailable-summary", Timing::fast(), move |t| {
            div()
                .id("unavailable-summary-row")
                .on_click(toggle)
                .role(Role::Button)
                .button_text()
                .flex()
                .items_center()
                .gap(u(4.))
                .text_px(12., 14.)
                .text_color(mix(faint, dim, t))
                .cursor_pointer()
                .child(fa_glyph(
                    if open {
                        "fas fa-caret-down"
                    } else {
                        "fas fa-caret-right"
                    },
                    10.,
                ))
                .child(summary_label)
        });
        let code = open.then(|| {
            div()
                .mt(u(6.))
                .py(u(4.))
                .px(u(10.))
                .bg(white().opacity(0.05))
                .border_1()
                .border_color(theme.color("l-border"))
                .rounded(u(6.))
                .font_family(crate::text::MONO)
                .text_px(12., 14.)
                .text_color(dim)
                .child(reason.clone())
        });
        let icon = div()
            .mt(u(3.))
            .size(u(40.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(white().opacity(0.06))
            .border_1()
            .border_color(theme.color("l-border-strong"))
            .text_color(dim)
            .child(fa_glyph(view.icon, 17.));
        let texts = div()
            .flex()
            .flex_col()
            .gap(u(6.))
            .min_w_0()
            .child(
                div()
                    .text_px(15., 15. * 1.35)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.color("l-text"))
                    .child(tr(cx, view.title, &[])),
            )
            .child(
                div()
                    .text_px(13., 13. * 1.5)
                    .text_color(dim)
                    .child(tr(cx, view.text, &[])),
            );
        let (vw, _) = viewport(window);
        // .unavail__window: min(520px, 100% − 80px).
        let body = card(&theme, window)
            .id("unavailable-window")
            .role(Role::Dialog)
            .aria_label(title)
            .w(u((vw - 80.).min(520.)))
            .overflow_hidden()
            .child(head)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(u(16.))
                    .pt(u(20.))
                    .px(u(24.))
                    .pb(u(22.))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(u(16.))
                            .child(icon)
                            .child(texts),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_start()
                            .child(summary)
                            .children(code),
                    )
                    .child(div().flex().flex_wrap().gap(u(10.)).children(actions)),
            );
        let on_close = listen(cx, |_: &mut UnavailableModal, _, cx| Self::close(cx));
        self.modal
            .render(window, cx, on_close, move || body)
            .into_any_element()
    }
}
