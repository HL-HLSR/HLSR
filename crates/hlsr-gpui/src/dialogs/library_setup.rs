//! Выбор папки библиотеки при первом запуске (`LibrarySetupModal.vue` 0.4.9): путь и свободное
//! место, «Выбрать другую» (системный выбор и проверка), «Использовать эту папку»; ошибки «нет
//! прав», «путь не подходит» и (0.4.9) «папка занята» — «Подтвердить» тогда недоступна;
//! кириллица в пути — предупреждение. Логика — [`LibrarySetup`].
//!
//! Слой 0.4.9 — z-index 15, под шапкой (20): шапка не затемнена, её кнопки и перетаскивание окна
//! работают. Здесь окно — обычный ребёнок корня после шапки, поэтому подложка начинается под
//! полосой шапки (отступление: фон полосы шапки не затемнён). Esc и нажатие на подложку окно не
//! закрывают; Tab ходит по кругу в окне. Стопку окон (`overlayOpen`) и возврат фокуса ведёт
//! [`Modal`] без его подложки.

use super::{Btn, btn};
use crate::layer::{Layers, Modal, card};
use crate::motion::{MODAL, presence};
use crate::scale::{u, viewport};
use crate::shell::root::ShellContext;
use crate::text::{TextExt, spaced};
use crate::theme::Theme;
use gpui::{
    App, Context, FocusHandle, FontWeight, IntoElement, Render, Role, Window, div, prelude::*, rgb,
    white,
};
use hlsr_ui::controllers::shell_flows::ShellFlows;
use hlsr_ui::controllers::shell_windows::LibrarySetup;
use hlsr_ui::services::Services;
use std::cell::RefCell;
use std::rc::Rc;

pub struct LibrarySetupModal {
    services: Services,
    /// Стопка окон и фокус; подложку рисует окно само.
    modal: Modal,
    choose: FocusHandle,
    state: Rc<RefCell<LibrarySetup>>,
    ring: bool,
}

impl LibrarySetupModal {
    pub fn new(services: &Services, cx: &mut Context<LibrarySetupModal>) -> LibrarySetupModal {
        LibrarySetupModal {
            services: services.clone(),
            modal: Modal::new("library-setup", cx),
            choose: cx.focus_handle().tab_stop(true),
            state: Rc::default(),
            ring: false,
        }
    }

    fn flows(cx: &App) -> Option<ShellFlows> {
        ShellContext::get(cx).map(|shell| shell.flows.clone())
    }

    fn tr(cx: &App, key: &str, args: &[&str]) -> String {
        ShellContext::get(cx).map_or_else(|| key.to_owned(), |shell| shell.shell.tr(key, args))
    }

    /// `v-if="librarySetupOpen"`: `data()` и `mounted()` — проверка пути, фокус на «Выбрать».
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(flows) = Self::flows(cx) else {
            return;
        };
        let (open, path) =
            flows.with_state(|state| (state.library_setup_open, state.install_path.clone()));
        if open && !self.modal.is_open() {
            *self.state.borrow_mut() = LibrarySetup::new(&path);
            self.ring = super::opened_by_script();
            self.modal.open(window, cx);
            let choose = self.choose.clone();
            window.on_next_frame(move |window, cx| window.focus(&choose, cx));
            self.run(cx, move |services, state| async move {
                LibrarySetup::validate(&services, &state, &path).await;
            });
        } else if !open && self.modal.is_open() {
            self.modal.close(window, cx);
        }
        cx.notify();
    }

    /// Шаг окна (проверка, выбор, запись) — задача потока окна; потом перерисовка. Задачи не
    /// отменяются: как у async-методов 0.4.9, повторный щелчок не теряет ответ прошлого шага.
    fn run<F>(
        &mut self,
        cx: &mut Context<Self>,
        step: impl FnOnce(Services, Rc<RefCell<LibrarySetup>>) -> F,
    ) where
        F: std::future::Future<Output = ()> + 'static,
    {
        let job = step(self.services.clone(), self.state.clone());
        cx.spawn(async move |this, cx| {
            job.await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    fn choose(&mut self, cx: &mut Context<Self>) {
        self.run(cx, |services, state| async move {
            LibrarySetup::choose(&services, &state).await;
        });
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let job = {
            let (services, state) = (self.services.clone(), self.state.clone());
            async move { LibrarySetup::confirm(&services, &state).await }
        };
        cx.spawn(async move |this, cx| {
            let confirmed = job.await;
            this.update(cx, |_, cx| {
                // `$emit("confirmed", path)` → `onLibraryConfirmed`: окно закроется по состоянию.
                if let (Some(path), Some(flows)) = (confirmed, Self::flows(cx)) {
                    flows.on_library_confirmed(&path);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for LibrarySetupModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let open = Self::flows(cx).is_some_and(|flows| flows.with_state(|s| s.library_setup_open));
        if open != self.modal.is_open() {
            cx.defer_in(window, Self::sync);
        }
        let theme = cx.global::<Theme>().clone();
        let state = self.state.borrow().clone();
        let tr = |key: &str| Self::tr(cx, key, &[]);
        let (title, text) = (tr("#L_FR_TITLE"), tr("#L_FR_TEXT"));
        let (choose_label, confirm_label) = (tr("#L_FR_CHOOSE"), tr("#L_FR_CONFIRM"));
        let free = state
            .free_space_gb()
            .map(|gb| Self::tr(cx, "#L_FR_FREE_SPACE", &[&gb]));
        let error = state.error.map(|key| Self::tr(cx, key, &[]));
        let cyrillic = state.has_cyrillic.then(|| tr("#UI_FOUND_CYRILLIC"));
        let (vw, vh) = viewport(window);
        let topbar = theme.len("l-topbar-h");
        let notice = |text: String, color: u32| {
            div()
                .mt(u(10.))
                .text_px(12.5, 12.5 * 1.4)
                .text_color(rgb(color))
                .child(text)
        };
        let choose = btn("library-choose", choose_label, Btn::Plain)
            .focus(&self.choose)
            .ring(self.ring)
            .on_click(cx.listener(|this, _, _, cx| this.choose(cx)));
        let confirm = btn("library-confirm", confirm_label, Btn::Accent)
            .disabled(error.is_some())
            .on_click(cx.listener(|this, _, _, cx| this.confirm(cx)));
        // .library-setup__window: min(520px, 100% − 80px), поля 26.
        let window_card = card(&theme, window)
            .id("library-setup-window")
            .role(Role::Dialog)
            .aria_label(title.clone())
            .w(u((vw - 80.).min(520.)))
            .p(u(26.))
            .child(spaced(
                title.to_uppercase(),
                16.,
                20.,
                FontWeight::BOLD,
                0.09,
                theme.color("l-text"),
            ))
            .child(
                div()
                    .mt(u(12.))
                    .mb(u(18.))
                    .text_px(13.5, 13.5 * 1.55)
                    .text_color(theme.color("l-text-dim"))
                    .child(text),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(u(16.))
                    .py(u(13.))
                    .px(u(16.))
                    .bg(white().opacity(0.05))
                    .border_1()
                    .border_color(theme.color("l-border"))
                    .rounded(theme.rem("l-radius-sm"))
                    .child(
                        div()
                            .min_w_0()
                            .text_px(13.5, 20.)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.color("l-text"))
                            .child(state.path.clone()),
                    )
                    .children(free.map(|free| {
                        div()
                            .flex_none()
                            .whitespace_nowrap()
                            .text_px(12., 20.)
                            .text_color(theme.color("l-text-dim"))
                            .child(free)
                    })),
            )
            .children(error.map(|error| notice(error, 0xff6b78)))
            .children(cyrillic.map(|warning| notice(warning, 0xffb37a)))
            .child(
                div()
                    .flex()
                    .gap(u(10.))
                    .mt(u(22.))
                    .child(choose)
                    .child(crate::snap::target("library-confirm", confirm)),
            );
        let scrim = theme.color("l-modal-scrim");
        let radius = Layers::radius(cx);
        let scope = self.modal.scope().clone();
        presence("library-setup", open, MODAL, move || {
            div()
                .absolute()
                .top_0()
                .left_0()
                .w(u(vw))
                .h(u(vh))
                .child(
                    // Подложка — ниже полосы шапки (шапка в 0.4.9 над ней): мышь к странице не
                    // проходит; окно — по центру всего окна.
                    div()
                        .id("library-setup-scrim")
                        .absolute()
                        .top(u(topbar))
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .rounded_b(radius)
                        .bg(scrim)
                        .occlude(),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            // Tab по кругу в окне — `tab_root` корня (окно верхнее в стопке).
                            div()
                                .id("library-setup-scope")
                                .track_focus(&scope)
                                .tab_group()
                                .occlude()
                                .child(window_card),
                        ),
                )
        })
        .absolute()
        .inset_0()
    }
}
