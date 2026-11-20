//! Окно FFmpeg перед «Демо» (подтверждение со слотом, LauncherShell 0.4.9): откуда скачается
//! (источник, запасной), сборка, размер; «Скачать» ставит FFmpeg (кнопка недоступна, пока идёт
//! загрузка), «Позже» закрывает, во время загрузки — «Отменить загрузку» (и Esc). В слоте — ошибка
//! установки или «Загрузка» с процентом и полосой. Логика — `ShellFlows::on_select_demo`.

use super::{flows, tr};
use crate::dialogs::confirm::{Answer, ConfirmCard, OnAnswer};
use crate::layer::{Modal, listen};
use crate::scale::u;
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::{
    AnyElement, Context, FocusHandle, IntoElement, Render, Window, div, prelude::*, relative, white,
};
use hlsr_ui::controllers::shell::Confirm;
use hlsr_ui::controllers::shell_flows::FfmpegGate;
use std::rc::Rc;

pub struct FfmpegGateModal {
    modal: Modal,
    cancel: FocusHandle,
    /// Окно в кадре; после закрытия остаётся, пока окно уходит.
    shown: Option<FfmpegGate>,
    ring: bool,
}

impl FfmpegGateModal {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let cancel = cx.focus_handle().tab_stop(true);
        let mut modal = Modal::new("ffmpeg-gate", cx).on_top();
        modal.focus_initial(cancel.clone());
        FfmpegGateModal {
            modal,
            cancel,
            shown: None,
            ring: false,
        }
    }

    fn gate(cx: &gpui::App) -> Option<FfmpegGate> {
        flows(cx).and_then(|flows| flows.with_state(|state| state.ffmpeg_gate.clone()))
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if Self::gate(cx).is_some() {
            if !self.modal.is_open() {
                self.ring = crate::dialogs::opened_by_script();
            }
            self.modal.open(window, cx);
        } else {
            self.modal.close(window, cx);
        }
        cx.notify();
    }

    fn answer(accept: bool, cx: &mut gpui::App) {
        if let Some(flows) = flows(cx) {
            flows.on_ffmpeg_gate(accept);
        }
    }

    /// `.ffgate`: ошибка (`--l-level-error`) или «Загрузка» с процентом и полосой 6 px.
    fn slot(gate: &FfmpegGate, cx: &gpui::App) -> Option<AnyElement> {
        if !gate.installing && gate.error.is_none() {
            return None;
        }
        let theme = cx.global::<Theme>();
        let dim = theme.color("l-text-dim");
        let error = gate.error.clone().map(|error| {
            div()
                .text_px(12.5, 12.5 * 1.45)
                .text_color(theme.color("l-level-error"))
                .child(error)
        });
        let progress = gate.installing.then(|| {
            let fill = relative(gate.percent as f32 / 100.);
            div()
                .flex()
                .flex_col()
                .gap(u(8.))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .text_px(12.5, 15.)
                        .text_color(dim)
                        .child(tr(cx, "#L_DEMO_FFMPEG_INSTALLING", &[]))
                        .child(format!("{}%", gate.percent)),
                )
                .child(
                    div()
                        .h(u(6.))
                        .rounded(u(3.))
                        .bg(white().opacity(0.12))
                        .overflow_hidden()
                        .child(div().h_full().w(fill).bg(theme.color("accent"))),
                )
        });
        Some(
            div()
                .flex()
                .flex_col()
                .gap(u(8.))
                .mt(u(14.))
                .children(error)
                .children(progress)
                .into_any_element(),
        )
    }
}

impl Render for FfmpegGateModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let gate = Self::gate(cx);
        if gate.is_some() != self.modal.is_open() {
            cx.defer_in(window, Self::sync);
        }
        if gate.is_some() {
            self.shown = gate;
        }
        let card = self.shown.clone().map(|gate| {
            let items =
                flows(cx).map_or_else(Vec::new, |flows| flows.ffmpeg_gate_items(&gate.status));
            let cancel = if gate.installing {
                "#L_DEMO_GATE_CANCEL"
            } else {
                "#L_DEMO_GATE_LATER"
            };
            let request = Confirm {
                title: tr(cx, "#L_DEMO_GATE_TITLE", &[]),
                text: tr(cx, "#L_DEMO_GATE_TEXT", &[]),
                items,
                confirm_label: Some(tr(cx, "#L_DEMO_GATE_OK", &[])),
                cancel_label: Some(tr(cx, cancel, &[])),
                ..Confirm::default()
            };
            let on_answer: OnAnswer =
                Rc::new(|answer, _, cx| Self::answer(answer == Answer::Accept, cx));
            ConfirmCard {
                confirm_disabled: gate.installing,
                ring: self.ring,
                slot: Self::slot(&gate, cx),
                ..ConfirmCard::new(request, "")
            }
            .render(&self.cancel, on_answer, window, cx)
            .into_any_element()
        });
        let on_close = listen(cx, |_: &mut FfmpegGateModal, _, cx| Self::answer(false, cx));
        self.modal
            .render(window, cx, on_close, move || {
                card.unwrap_or_else(|| div().into_any_element())
            })
            .into_any_element()
    }
}
