//! «Спасибо за поддержку» (`SupportThanksModal.vue` 0.4.9): окно как у подтверждения — заголовок,
//! «Спасибо, что поддержали HLSR!», строка про дизайнерский ник (`styles`), «Подписка активна
//! до ДД.ММ.ГГГГ» (у подписки) и «Закрыть». Событие `support-thanks` main присылает после
//! `support-thanks-ready` оболочки; одно окно на все пришедшие оплаты — первая самая свежая,
//! остальные, пока окно открыто, не показываются (`thanksNotice` LauncherShell). Слой поверх
//! окон (z-index 70); Esc, подложка и «Закрыть» закрывают; при открытии фокус на «Закрыть».

use crate::dialogs::{Btn, btn};
use crate::layer::{Modal, card, listen};
use crate::scale::{u, viewport};
use crate::text::{TextExt, spaced};
use crate::theme::Theme;
use gpui::{Context, FocusHandle, FontWeight, IntoElement, Render, Role, Window, div, prelude::*};
use hlsr_core::language::Language;
use hlsr_ui::controllers::sponsors::ThanksNotice;
use hlsr_ui::services::Services;

pub struct SupportThanksModal {
    modal: Modal,
    close: FocusHandle,
    language: Language,
    /// Показанное уведомление; после закрытия остаётся, пока окно уходит.
    notice: Option<ThanksNotice>,
    /// Окно открыто (`thanksNotice !== null`).
    shown: bool,
    /// Контур фокуса на «Закрыть» без клавиатуры ([`crate::dialogs::opened_by_script`]).
    ring: bool,
    _events: gpui::Task<()>,
}

impl SupportThanksModal {
    pub fn new(services: &Services, cx: &mut Context<SupportThanksModal>) -> SupportThanksModal {
        let close = cx.focus_handle().tab_stop(true);
        let mut modal = Modal::new("support-thanks", cx).on_top();
        modal.focus_initial(close.clone());
        let events = crate::bridge::listen(cx, services.subscribe(), |this, event, cx| {
            if event.channel != "support-thanks" || this.shown {
                return;
            }
            if let Some(notice) = ThanksNotice::from_event(&event.payload) {
                this.notice = Some(notice);
                this.shown = true;
                this.ring = crate::dialogs::opened_by_script();
                cx.notify();
            }
        });
        SupportThanksModal {
            modal,
            close,
            language: crate::shell::root::language_of(services),
            notice: None,
            shown: false,
            ring: false,
            _events: events,
        }
    }

    /// `@close`: `thanksNotice = null`.
    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = false;
        self.modal.close(window, cx);
        cx.notify();
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shown {
            self.modal.open(window, cx);
        } else {
            self.modal.close(window, cx);
        }
        cx.notify();
    }
}

impl Render for SupportThanksModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        if self.shown != self.modal.is_open() {
            // Стопка окон и фокус — не посреди отрисовки.
            cx.defer_in(window, Self::sync);
        }
        let theme = cx.global::<Theme>().clone();
        let tr = |key: &str| hlsr_ui::tr(self.language, key, &[]);
        let title = tr("#L_THANKS_TITLE");
        let lines = self
            .notice
            .as_ref()
            .map(|notice| notice.lines(self.language))
            .unwrap_or_default();
        let width = (viewport(window).0 - 80.).min(460.);
        let view = cx.entity().downgrade();
        let close_button = btn("thanks-close", tr("#L_CLOSE"), Btn::Primary)
            .focus(&self.close)
            .ring(self.ring)
            .on_click(move |_, window, cx| {
                view.update(cx, |this, cx| this.dismiss(window, cx)).ok();
            });
        // `.thanks__title`: 15 px, 600, капслок, разрядка .09em; абзацы 13,5 / 1,55.
        let body = card(&theme, window)
            .id("thanks-window")
            .role(Role::Dialog)
            .aria_label(title.clone())
            .w(u(width))
            .p(u(24.))
            .child(spaced(
                title.to_uppercase(),
                15.,
                20.,
                FontWeight::BOLD,
                0.09,
                theme.color("l-text"),
            ))
            .children(lines.into_iter().map(|line| {
                div()
                    .mt(u(12.))
                    .text_px(13.5, 13.5 * 1.55)
                    .text_color(theme.color("l-text-dim"))
                    .child(line)
            }))
            .child(div().mt(u(22.)).flex().child(close_button));
        let on_close = listen(cx, |this: &mut SupportThanksModal, window, cx| {
            this.dismiss(window, cx)
        });
        self.modal
            .render(window, cx, on_close, move || body)
            .into_any_element()
    }
}
