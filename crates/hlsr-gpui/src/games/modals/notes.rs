//! «Что нового» в версии игры (`GameNotesModal.vue` 0.4.9): заголовок «<игра> <версия>», пункты
//! заметок той же версии (до 20, по 300 знаков) или «пусто». Esc, подложка и крестик закрывают;
//! фокус — на крестике.

use super::{flows, header, tr};
use crate::icons::fa_glyph;
use crate::layer::{Modal, card, listen};
use crate::scale::{u, viewport};
use crate::scroll::{Bar, Scroll, ScrollArea};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::{Context, FocusHandle, IntoElement, Render, Role, Window, div, prelude::*};
use hlsr_ui::controllers::shell_windows::{game_notes_items, game_notes_title};
use std::rc::Rc;

pub struct GameNotesModal {
    modal: Modal,
    close: FocusHandle,
    scroll: Scroll,
    ring: bool,
}

impl GameNotesModal {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let close = cx.focus_handle().tab_stop(true);
        let mut modal = Modal::new("game-notes", cx);
        modal.focus_initial(close.clone());
        GameNotesModal {
            modal,
            close,
            scroll: Scroll::new(),
            ring: false,
        }
    }

    fn open(cx: &gpui::App) -> bool {
        flows(cx).is_some_and(|flows| flows.with_state(|state| state.game_notes_open))
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if Self::open(cx) {
            if !self.modal.is_open() {
                self.scroll = Scroll::new();
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
            flows.close_window("game-notes");
        }
    }
}

impl Render for GameNotesModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if Self::open(cx) != self.modal.is_open() {
            cx.defer_in(window, Self::sync);
        }
        let theme = cx.global::<Theme>().clone();
        let (title, items) = flows(cx).map_or_else(Default::default, |flows| {
            let (version, notes) = flows.with_state(|state| {
                (
                    state.game_notes_version.clone(),
                    state.game_notes_data.clone(),
                )
            });
            let game = flows.current_game_full_title();
            (
                game_notes_title(|key, args| tr(cx, key, args), &game, &version),
                game_notes_items(notes.as_ref(), &version),
            )
        });
        let (dim, faint) = (theme.color("l-text-dim"), theme.color("l-text-faint"));
        let on_close: super::OnClick = Rc::new(|_, _, cx| Self::close(cx));
        let head = header(&title, &self.close, self.ring, on_close, window, cx);
        let content = if items.is_empty() {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(u(10.))
                .pt(u(34.))
                .pb(u(22.))
                .text_px(13., 16.)
                .text_color(faint)
                .child(fa_glyph("far fa-file-alt", 26.))
                .child(tr(cx, "#L_GN_EMPTY", &[]))
        } else {
            // ul: отступ 18 под маркер «•», 13 px / 1.6, между пунктами 4.
            div()
                .flex()
                .flex_col()
                .gap(u(4.))
                .text_px(13., 13. * 1.6)
                .text_color(dim)
                .children(items.into_iter().map(|item| {
                    div()
                        .relative()
                        .pl(u(18.))
                        .child(div().absolute().left(u(5.)).child("•"))
                        .child(item)
                }))
        };
        let (vw, vh) = viewport(window);
        // .game-notes__window: min(560px, 100% − 80px) × до min(520px, 100% − 80px).
        let body = card(&theme, window)
            .id("game-notes-window")
            .role(Role::Dialog)
            .aria_label(title)
            .w(u((vw - 80.).min(560.)))
            .max_h(u((vh - 80.).min(520.)))
            .overflow_hidden()
            .child(head)
            .child(
                ScrollArea::new("game-notes-scroll", &self.scroll)
                    .bar(Bar::Thin)
                    .flex_1()
                    .min_h_0()
                    .child(div().pt(u(20.)).px(u(24.)).pb(u(24.)).child(content)),
            );
        let on_close = listen(cx, |_: &mut GameNotesModal, _, cx| Self::close(cx));
        self.modal
            .render(window, cx, on_close, move || body)
            .into_any_element()
    }
}
