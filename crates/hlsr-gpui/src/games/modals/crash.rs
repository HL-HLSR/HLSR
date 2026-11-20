//! «Игра упала» (`GameCrashModal.vue` 0.4.9, `role="alertdialog"`): игра и код выхода, текст по
//! тому, что нашлось (архив сбоя, файл, ничего), путь целиком с переносом, подсказка DEP; кнопки
//! «Папка» (или «Диагностика», если файла нет), Discord, «Закрыть». Слой поверх окон (z-index 70);
//! Esc и подложка закрывают; фокус — на «Закрыть».

use super::{Look, flows, game_button, tr};
use crate::icons::fa_glyph;
use crate::layer::{Modal, card_plain, listen};
use crate::scale::{u, viewport};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, FocusHandle, FontWeight, IntoElement, Render, Role, Window, div, prelude::*, rgb,
    white,
};
use hlsr_core::js;
use hlsr_ui::controllers::shell_flows::short_name;
use serde_json::Value;

pub struct GameCrashModal {
    modal: Modal,
    close: FocusHandle,
    path: FocusHandle,
    /// Отчёт в окне; после закрытия остаётся, пока окно уходит.
    shown: Value,
    ring: bool,
}

/// Код выхода: исключения Windows (0xC0000005) — только шестнадцатерично.
fn code_text(code: f64) -> String {
    if code > 65535. || code < 0. {
        format!("0x{:X}", code as i64 as u32)
    } else {
        js::number::number_to_string(code)
    }
}

impl GameCrashModal {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let close = cx.focus_handle().tab_stop(true);
        let mut modal = Modal::new("game-crash", cx).on_top();
        modal.focus_initial(close.clone());
        GameCrashModal {
            modal,
            close,
            path: cx.focus_handle().tab_stop(true),
            shown: Value::Null,
            ring: false,
        }
    }

    fn report(cx: &gpui::App) -> Option<Value> {
        flows(cx).and_then(|flows| flows.with_state(|state| state.crash_report.clone()))
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if Self::report(cx).is_some() {
            if !self.modal.is_open() {
                self.ring = crate::dialogs::opened_by_script();
            }
            self.modal.open(window, cx);
        } else {
            self.modal.close(window, cx);
        }
        cx.notify();
    }

    fn act(action: &'static str) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) {
        move |_, _, cx| {
            if let Some(flows) = flows(cx) {
                flows.on_crash_action(action);
            }
        }
    }
}

impl Render for GameCrashModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let report = Self::report(cx);
        if report.is_some() != self.modal.is_open() {
            cx.defer_in(window, Self::sync);
        }
        if let Some(report) = report {
            self.shown = report;
        }
        let report = self.shown.clone();
        let theme = cx.global::<Theme>().clone();
        let (text, dim) = (theme.color("l-text"), theme.color("l-text-dim"));
        let warn = rgb(0xffb37a);
        let title = tr(cx, "#L_CRM_TITLE", &[]);
        let name = report
            .get("launcherId")
            .filter(|id| js::truthy(id))
            .and_then(|id| short_name(&js::to_string(id)));
        let code = report
            .get("exitCode")
            .filter(|code| !code.is_null())
            .map(|code| {
                tr(
                    cx,
                    "#L_CRM_EXIT_CODE",
                    &[&code_text(js::value::to_number(Some(code)))],
                )
            });
        let subtitle = [name.map(str::to_owned), code]
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let file = report
            .get("file")
            .filter(|file| js::truthy(file))
            .map(js::to_string);
        let key = if report.get("archive").is_some_and(js::truthy) {
            "#L_CRM_TEXT_ARCHIVE"
        } else if file.is_some() {
            "#L_CRM_TEXT_FILE"
        } else {
            "#L_CRM_TEXT_NO_FILE"
        };
        let hint = match report.get("hint").and_then(Value::as_str) {
            Some("DEP_ALWAYS_ON") => Some("#L_CR_DEP_ALWAYS_ON"),
            Some("DEP_EXEMPTION_FAILED") => Some("#L_CR_DEP_EXEMPTION_FAILED"),
            _ => None,
        }
        .map(|key| tr(cx, key, &[]));
        let path_focused = self.path.is_focused(window);
        let path = file.clone().map(|file| {
            div()
                .id("crash-path")
                .track_focus(&self.path)
                .mt(u(10.))
                .py(u(10.))
                .px(u(12.))
                .bg(white().opacity(0.05))
                .border_1()
                .border_color(if path_focused {
                    theme.color("accent")
                } else {
                    theme.color("l-border")
                })
                .rounded(theme.rem("l-radius-sm"))
                .text_px(12.5, 12.5 * 1.5)
                .text_color(text)
                .cursor_text()
                .child(file)
        });
        let first = if file.is_some() {
            game_button(
                "crash-folder",
                Some("fas fa-folder-open"),
                tr(cx, "#L_CRM_OPEN_FOLDER", &[]),
                Look::Crash,
            )
            .on_click(Self::act("open-folder"))
        } else {
            game_button(
                "crash-diagnostics",
                Some("fas fa-file-archive"),
                tr(cx, "#L_CR_EXPORT", &[]),
                Look::Crash,
            )
            .on_click(Self::act("export-diagnostics"))
        };
        let actions = div()
            .flex()
            .gap(u(10.))
            .mt(u(22.))
            .child(first)
            .child(
                game_button(
                    "crash-discord",
                    Some("fab fa-discord"),
                    "Discord",
                    Look::Crash,
                )
                .on_click(Self::act("open-discord")),
            )
            .child(crate::snap::target(
                "crash-close",
                game_button(
                    "crash-close",
                    None,
                    tr(cx, "#L_CLOSE", &[]),
                    Look::CrashPrimary,
                )
                .focus(&self.close)
                .ring(self.ring)
                .on_click(Self::act("close")),
            ));
        let (vw, _) = viewport(window);
        // .crash__window: min(560px, 100% − 80px), поля 24.
        // `.crash__window` в правилах Liquid Glass нет: окно без стекла.
        let body = card_plain(&theme, window)
            .id("crash-window")
            .role(Role::AlertDialog)
            .aria_label(title.clone())
            .w(u((vw - 80.).min(560.)))
            .p(u(24.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(u(14.))
                    .child(
                        fa_glyph("fas fa-exclamation-triangle", 26.)
                            .mt(u(2.))
                            .text_color(warn),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(u(4.))
                            .min_w_0()
                            .child(
                                div()
                                    .text_px(17., 17. * 1.35)
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(text)
                                    .child(title),
                            )
                            .when(!subtitle.is_empty(), |el| {
                                el.child(div().text_px(12.5, 15.).text_color(dim).child(subtitle))
                            }),
                    ),
            )
            .child(
                div()
                    .mt(u(16.))
                    .text_px(13.5, 13.5 * 1.55)
                    .text_color(dim)
                    .child(tr(cx, key, &[])),
            )
            .children(path)
            .children(hint.map(|hint| {
                div()
                    .flex()
                    .items_start()
                    .gap(u(8.))
                    .mt(u(14.))
                    .text_px(12.5, 12.5 * 1.45)
                    .text_color(warn)
                    .child(fa_glyph("fas fa-info-circle", 12.5).mt(u(2.5)))
                    .child(div().flex_1().min_w_0().child(hint))
            }))
            .child(actions);
        let on_close = listen(cx, |_: &mut GameCrashModal, _, cx| {
            if let Some(flows) = flows(cx) {
                flows.on_crash_action("close");
            }
        });
        self.modal
            .render(window, cx, on_close, move || body)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn exit_code_hex_for_exceptions() {
        assert_eq!(super::code_text(-1073741819.), "0xC0000005");
        assert_eq!(super::code_text(70000.), "0x11170");
        assert_eq!(super::code_text(1.), "1");
    }
}
