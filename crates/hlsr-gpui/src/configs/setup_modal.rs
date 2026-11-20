//! Опрос при первом запуске игры без конфига (`ConfigSetupModal.vue` 0.4.9): «Настроить с нуля»,
//! «У меня есть свой конфиг», «Пропустить». «Закрыть», Esc и подложка отменяют запуск, ничего не
//! отмечая. Фокус — на «Пропустить»: Enter сразу после «Играть» запускает игру. Логика — опрос
//! `ShellFlows` (`offerConfigSetup`, `onConfigSetupChoose`).

use crate::games::modals::{Look, OnClick, flows, game_button, header, tr};
use crate::layer::{Modal, card, listen};
use crate::scale::{u, viewport};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::{App, Context, FocusHandle, IntoElement, Render, Role, Window, div, prelude::*};
use hlsr_ui::services::Services;
use std::rc::Rc;

pub struct ConfigSetupModal {
    modal: Modal,
    close: FocusHandle,
    skip: FocusHandle,
    ring: bool,
}

impl ConfigSetupModal {
    pub fn new(_services: &Services, cx: &mut Context<ConfigSetupModal>) -> ConfigSetupModal {
        let skip = cx.focus_handle().tab_stop(true);
        let mut modal = Modal::new("config-setup", cx);
        modal.focus_initial(skip.clone());
        ConfigSetupModal {
            modal,
            close: cx.focus_handle().tab_stop(true),
            skip,
            ring: false,
        }
    }

    fn open(cx: &App) -> bool {
        flows(cx).is_some_and(|flows| flows.with_state(|state| state.config_setup_open))
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if Self::open(cx) {
            if !self.modal.is_open() {
                self.ring = crate::dialogs::opened_by_script();
            }
            self.modal.open(window, cx);
        } else {
            self.modal.close(window, cx);
        }
        cx.notify();
    }

    fn choose(choice: Option<&'static str>, cx: &mut App) {
        if let Some(flows) = flows(cx) {
            flows.on_config_setup_choose(choice);
        }
    }
}

impl Render for ConfigSetupModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        if Self::open(cx) != self.modal.is_open() {
            cx.defer_in(window, Self::sync);
        }
        let theme = cx.global::<Theme>().clone();
        let title = tr(cx, "#L_CFG_SETUP_TITLE", &[]);
        let on_close: OnClick = Rc::new(|_, _, cx| Self::choose(None, cx));
        let head = header(&title, &self.close, false, on_close, window, cx);
        let button = |choice: &'static str, icon, key, look| {
            game_button(
                format!("cfgsetup-{choice}"),
                Some(icon),
                tr(cx, key, &[]),
                look,
            )
            .on_click(move |_, _, cx| Self::choose(Some(choice), cx))
        };
        let actions = div()
            .flex()
            .flex_wrap()
            .gap(u(10.))
            .child(button(
                "scratch",
                "fas fa-magic",
                "#L_CFG_SETUP_SCRATCH",
                Look::SettingsPrimary,
            ))
            .child(button(
                "own",
                "fas fa-file-import",
                "#L_CFG_SETUP_OWN",
                Look::Settings,
            ))
            .child(crate::snap::target(
                "cfgsetup-skip",
                button(
                    "skip",
                    "fas fa-forward",
                    "#L_CFG_SETUP_SKIP",
                    Look::Settings,
                )
                .focus(&self.skip)
                .ring(self.ring),
            ));
        let (vw, _) = viewport(window);
        // .cfgsetup__window: min(520px, 100% − 80px).
        let body = card(&theme, window)
            .id("cfgsetup-window")
            .role(Role::Dialog)
            .aria_label(title)
            .w(u((vw - 80.).min(520.)))
            .overflow_hidden()
            .child(head)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(u(18.))
                    .pt(u(20.))
                    .px(u(24.))
                    .pb(u(22.))
                    .child(
                        div()
                            .text_px(14., 21.)
                            .text_color(theme.color("l-text"))
                            .child(tr(cx, "#L_CFG_SETUP_TEXT", &[])),
                    )
                    .child(actions),
            );
        let on_close = listen(cx, |_: &mut ConfigSetupModal, _, cx| Self::choose(None, cx));
        self.modal
            .render(window, cx, on_close, move || body)
            .into_any_element()
    }
}
