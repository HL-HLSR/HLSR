//! Окно подтверждения (`ConfirmModal.vue` 0.4.9): заголовок, текст, пункты (длинный путь
//! переносится где угодно, `overflow-wrap: anywhere`), слот своего содержимого, варианты,
//! заметка, «Отмена» и «Подтвердить» (`confirmDisabled`, опасное действие). Слой поверх окон
//! (z-index 70); Esc и нажатие на подложку — «Отмена»; при открытии фокус на «Отмене».
//!
//! [`ConfirmModal`] — слот оболочки: запрос `askConfirm` — [`ShellController::confirm`], ответ —
//! [`ShellController::resolve_confirm`]. Окна со своим содержимым (ворота FFmpeg, ремонт) держат
//! свой [`Modal`] (`Modal::new(…).on_top()`, `focus_initial(cancel)`) и рисуют [`ConfirmCard`]
//! со слотом.

use super::{Btn, btn};
use crate::icons::fa_glyph;
use crate::kit::radio;
use crate::layer::{Modal, card, listen};
use crate::scale::{u, viewport};
use crate::shell::root::ShellContext;
use crate::text::{TextExt, spaced};
use crate::theme::Theme;
use gpui::{
    AnyElement, App, Context, Div, FocusHandle, FontWeight, IntoElement, Render, Role, Stateful,
    Window, div, prelude::*, rgb, white,
};
use hlsr_ui::controllers::shell::{Confirm, ShellController, initial_choice};
use hlsr_ui::services::Services;
use std::rc::Rc;

/// Что сделали в окне.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// «Отмена», Esc, подложка (`resolve(false)`).
    Cancel,
    /// «Подтвердить»: без вариантов — `true`, с ними — выбранный.
    Accept,
    /// Щелчок по варианту (`choice = id`).
    Pick(String),
    /// Двойной щелчок по варианту: выбрать и принять.
    PickAccept(String),
}

/// Обработчик ответа окна.
pub type OnAnswer = Rc<dyn Fn(Answer, &mut Window, &mut App)>;

/// Пропсы `ConfirmModal`: запрос, выбранный вариант, `confirmDisabled` и слот.
pub struct ConfirmCard {
    pub request: Confirm,
    pub choice: String,
    pub confirm_disabled: bool,
    pub slot: Option<AnyElement>,
    /// Контур фокуса на «Отмене» без клавиатуры ([`super::opened_by_script`]).
    pub ring: bool,
}

impl ConfirmCard {
    pub fn new(request: Confirm, choice: impl Into<String>) -> ConfirmCard {
        ConfirmCard {
            request,
            choice: choice.into(),
            confirm_disabled: false,
            slot: None,
            ring: false,
        }
    }

    /// `.confirm__window`: min(460px, 100% − 80px), поля 24. `cancel` — фокус «Отмены».
    pub fn render(
        self,
        cancel: &FocusHandle,
        on_answer: OnAnswer,
        window: &mut Window,
        cx: &mut App,
    ) -> Stateful<Div> {
        let theme = cx.global::<Theme>().clone();
        let ConfirmCard {
            request,
            choice,
            confirm_disabled,
            slot,
            ring,
        } = self;
        let tr = |key: &str| {
            ShellContext::get(cx).map_or_else(|| key.to_owned(), |shell| shell.shell.tr(key, &[]))
        };
        let (cancel_label, confirm_label) = (
            request
                .cancel_label
                .clone()
                .unwrap_or_else(|| tr("#L_CANCEL")),
            request
                .confirm_label
                .clone()
                .unwrap_or_else(|| tr("#L_CONFIRM")),
        );
        let width = (viewport(window).0 - 80.).min(460.);
        let answer = |answer: Answer| {
            let on_answer = on_answer.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                on_answer(answer.clone(), window, cx)
            }
        };
        let items = (!request.items.is_empty()).then(|| {
            div()
                .id("confirm-items")
                .mt(u(14.))
                .py(u(12.))
                .px(u(14.))
                .max_h(u(132.))
                // shortcut: прокрутка GPUI без полосы 4 px App.vue — пунктов больше 132 px не
                // бывает (пути удаления); полоса — ScrollArea, если появятся длинные списки.
                .overflow_y_scroll()
                .bg(white().opacity(0.05))
                .border_1()
                .border_color(theme.color("l-border"))
                .rounded(theme.rem("l-radius-sm"))
                .text_px(12.5, 12.5 * 1.6)
                .text_color(theme.color("l-text"))
                .children(request.items.iter().map(|item| div().child(item.clone())))
        });
        let choices = (!request.choices.is_empty()).then(|| {
            div()
                .id("confirm-choices")
                .role(Role::RadioGroup)
                .aria_label(request.title.clone())
                .flex()
                .flex_col()
                .gap(u(6.))
                .mt(u(14.))
                .children(request.choices.iter().map(|(id, label)| {
                    let (pick, id) = (on_answer.clone(), id.clone());
                    radio(
                        gpui::ElementId::Name(format!("confirm-choice-{id}").into()),
                        label.clone(),
                        id == choice,
                    )
                    .on_click(move |event, window, cx| {
                        let answer = if event.click_count() == 2 {
                            Answer::PickAccept(id.clone())
                        } else {
                            Answer::Pick(id.clone())
                        };
                        pick(answer, window, cx)
                    })
                }))
        });
        let note = request.note.clone().map(|note| {
            let warn = rgb(0xffb37a);
            div()
                .flex()
                .items_start()
                .gap(u(8.))
                .mt(u(14.))
                .text_px(12.5, 12.5 * 1.45)
                .text_color(warn)
                .child(fa_glyph("fas fa-exclamation-triangle", 12.5).mt(u(2.5)))
                .child(div().flex_1().min_w_0().child(note))
        });
        let primary = if request.danger {
            Btn::Danger
        } else {
            Btn::Primary
        };
        let actions = div()
            .flex()
            .gap(u(10.))
            .mt(u(22.))
            .child(
                btn("confirm-cancel", cancel_label, Btn::Plain)
                    .focus(cancel)
                    .ring(ring)
                    .on_click(answer(Answer::Cancel)),
            )
            .child(crate::snap::target(
                "confirm-accept",
                btn("confirm-accept", confirm_label, primary)
                    .disabled(confirm_disabled)
                    .on_click(answer(Answer::Accept)),
            ));
        card(&theme, window)
            .id("confirm-window")
            .role(Role::AlertDialog)
            .aria_label(request.title.clone())
            .w(u(width))
            .p(u(24.))
            .child(spaced(
                request.title.to_uppercase(),
                15.,
                20.,
                FontWeight::BOLD,
                0.09,
                theme.color("l-text"),
            ))
            .child(
                div()
                    .mt(u(12.))
                    .text_px(13.5, 13.5 * 1.55)
                    .text_color(theme.color("l-text-dim"))
                    .child(request.text.clone()),
            )
            .children(items)
            .children(slot)
            .children(choices)
            .children(note)
            .child(actions)
    }
}

/// Слот подтверждения оболочки (`<ConfirmModal v-if="confirmRequest">` LauncherShell 0.4.9).
pub struct ConfirmModal {
    modal: Modal,
    cancel: FocusHandle,
    /// Запрос в окне; после ответа остаётся, пока окно уходит.
    shown: Option<Confirm>,
    choice: String,
    ring: bool,
}

impl ConfirmModal {
    pub fn new(_services: &Services, cx: &mut Context<ConfirmModal>) -> ConfirmModal {
        let cancel = cx.focus_handle().tab_stop(true);
        let mut modal = Modal::new("confirm", cx).on_top();
        modal.focus_initial(cancel.clone());
        ConfirmModal {
            modal,
            cancel,
            shown: None,
            choice: String::new(),
            ring: false,
        }
    }

    fn shell(cx: &App) -> Option<ShellController> {
        ShellContext::get(cx).map(|shell| shell.shell.clone())
    }

    /// Окно за запросом контроллера: новый запрос — выбор по умолчанию, нет запроса — уход.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let request = Self::shell(cx).and_then(|shell| shell.confirm());
        match request {
            Some(request) => {
                if self.shown.as_ref() != Some(&request) || !self.modal.is_open() {
                    self.choice = initial_choice(&request).to_owned();
                    self.shown = Some(request);
                }
                if !self.modal.is_open() {
                    self.ring = super::opened_by_script();
                }
                self.modal.open(window, cx);
            }
            None => self.modal.close(window, cx),
        }
        cx.notify();
    }

    fn answer(&mut self, answer: Answer, window: &mut Window, cx: &mut Context<Self>) {
        let Some(shell) = Self::shell(cx) else {
            return;
        };
        let accept =
            |this: &mut Self, choice: String, window: &mut Window, cx: &mut Context<Self>| {
                this.modal.close(window, cx);
                shell.resolve_confirm(true, &choice);
            };
        match answer {
            Answer::Cancel => {
                self.modal.close(window, cx);
                shell.resolve_confirm(false, "");
            }
            Answer::Accept => accept(self, self.choice.clone(), window, cx),
            Answer::Pick(id) => self.choice = id,
            Answer::PickAccept(id) => {
                self.choice = id.clone();
                accept(self, id, window, cx);
            }
        }
        cx.notify();
    }
}

impl Render for ConfirmModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let pending = Self::shell(cx).and_then(|shell| shell.confirm());
        let stale = match &pending {
            Some(request) => !self.modal.is_open() || self.shown.as_ref() != Some(request),
            None => self.modal.is_open(),
        };
        if stale {
            // Стопка окон и фокус — не посреди отрисовки.
            cx.defer_in(window, Self::sync);
        }
        let content = self.shown.clone().map(|request| {
            let view = cx.entity().downgrade();
            let on_answer: OnAnswer = Rc::new(move |answer, window, cx| {
                view.update(cx, |this, cx| this.answer(answer, window, cx))
                    .ok();
            });
            ConfirmCard {
                ring: self.ring,
                ..ConfirmCard::new(request, self.choice.clone())
            }
            .render(&self.cancel, on_answer, window, cx)
            .into_any_element()
        });
        let on_close = listen(cx, |this: &mut ConfirmModal, window, cx| {
            this.answer(Answer::Cancel, window, cx)
        });
        let modal = self.modal.render(window, cx, on_close, move || {
            content.unwrap_or_else(|| div().into_any_element())
        });
        div()
            .child(modal)
            .child(super::pointer_watch())
            .into_any_element()
    }
}
