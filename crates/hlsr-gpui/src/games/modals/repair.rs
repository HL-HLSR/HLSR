//! Ремонт по файлам (`RepairModal.vue` 0.4.9) — подтверждение со слотом: «Выбрать всё/Снять»,
//! дерево файлов проверки (папки с галочкой и промежуточным состоянием, шеврон раскрытия), размер,
//! «Скачать N файлов (X)» (ничего не выбрано — недоступна), предупреждение про изменённые cfg.
//! Ответ — отмеченные пути или отказ. Логика — [`RepairTree`].

use super::{Look, flows, game_button, tr};
use crate::dialogs::confirm::{Answer, ConfirmCard, OnAnswer};
use crate::icons::fa_glyph;
use crate::kit::{CheckLook, Checked, check};
use crate::layer::{Modal, listen};
use crate::motion::{Timing, hover, mix};
use crate::scale::u;
use crate::text::{TextExt, break_all, spaced};
use crate::theme::Theme;
use gpui::{
    AnyElement, Context, Div, ElementId, FocusHandle, FontWeight, IntoElement, Render, Role,
    Window, div, prelude::*, white,
};
use hlsr_ui::controllers::boot::format_size;
use hlsr_ui::controllers::repair_tree::{Group, GroupState, RepairTree};
use hlsr_ui::controllers::shell::Confirm;
use serde_json::Value;
use std::rc::Rc;

pub struct RepairModal {
    modal: Modal,
    cancel: FocusHandle,
    /// Файлы открытого окна (новый запрос — новое дерево; сверка по указателю, без копии).
    shown: Option<Rc<[Value]>>,
    tree: RepairTree,
    ring: bool,
}

impl RepairModal {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let cancel = cx.focus_handle().tab_stop(true);
        let mut modal = Modal::new("repair", cx).on_top();
        modal.focus_initial(cancel.clone());
        RepairModal {
            modal,
            cancel,
            shown: None,
            tree: RepairTree::default(),
            ring: false,
        }
    }

    fn files(cx: &gpui::App) -> Option<Rc<[Value]>> {
        flows(cx).and_then(|flows| flows.with_state(|state| state.repair_files.clone()))
    }

    fn is_shown(&self, files: &Rc<[Value]>) -> bool {
        self.shown
            .as_ref()
            .is_some_and(|shown| Rc::ptr_eq(shown, files))
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match Self::files(cx) {
            Some(files) => {
                if !self.is_shown(&files) || !self.modal.is_open() {
                    self.tree = RepairTree::new(&files);
                    self.ring = crate::dialogs::opened_by_script();
                    self.shown = Some(files);
                }
                self.modal.open(window, cx);
            }
            None => self.modal.close(window, cx),
        }
        cx.notify();
    }

    fn answer(&mut self, answer: Answer, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = match answer {
            Answer::Accept => Some(self.tree.chosen_paths()),
            Answer::Cancel => None,
            Answer::Pick(_) | Answer::PickAccept(_) => return,
        };
        self.modal.close(window, cx);
        if let Some(flows) = flows(cx) {
            flows.on_repair_resolved(chosen);
        }
        cx.notify();
    }

    fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut RepairTree)) {
        f(&mut self.tree);
        cx.notify();
    }

    /// Строка `.rp__row`: 26 px в высоту, 12,5 px, `--l-text`.
    fn row() -> Div {
        div()
            .flex()
            .items_center()
            .gap(u(6.))
            .min_h(u(26.))
            .text_px(12.5, 15.)
    }

    fn size(&self, bytes: f64, cx: &gpui::App) -> Div {
        let theme = cx.global::<Theme>();
        div()
            .flex_none()
            .text_px(12., 15.)
            .text_color(theme.color("l-text-faint"))
            .child(format_size(bytes, &|key, args| tr(cx, key, args)))
    }

    fn group(&self, group: &Group, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let (dim, text, faint) = (
            theme.color("l-text-dim"),
            theme.color("l-text"),
            theme.color("l-text-faint"),
        );
        let open = !group.folder || self.tree.is_open(&group.key);
        let files = open.then(|| {
            div()
                .flex()
                .flex_col()
                .when(group.folder, |el| el.pl(u(24.)))
                .children(group.files.iter().map(|&file| {
                    let entry = &self.tree.files[file];
                    let pick = check(
                        ElementId::NamedInteger("rp-file".into(), file as u64),
                        if self.tree.is_chosen(file) {
                            Checked::On
                        } else {
                            Checked::Off
                        },
                        CheckLook::Native,
                    )
                    .flex_1()
                    .min_w_0()
                    .gap(u(8.))
                    .on_change(cx.listener(move |this, _: &bool, _, cx| {
                        this.edit(cx, |tree| tree.toggle(file))
                    }))
                    .child(
                        div()
                            .min_w_0()
                            .text_color(text)
                            .child(break_all(&self.tree.name_in(group, file))),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_px(11.5, 15.)
                            .text_color(faint)
                            .child(tr(cx, entry.tag_key(), &[])),
                    );
                    let pick = crate::snap::target(format!("rp-file-{file}"), pick);
                    Self::row().child(pick).child(self.size(entry.size, cx))
                }))
        });
        let folder = group.folder.then(|| {
            let key = group.key.clone();
            let label = tr(
                cx,
                "#L_GM_REPAIR_FOLDER",
                &[if key.is_empty() { "/" } else { &key }],
            );
            let toggle = cx.listener({
                let key = key.clone();
                move |this, _: &gpui::ClickEvent, _, cx| {
                    this.edit(cx, |tree| tree.toggle_open(&key))
                }
            });
            let chevron = hover(
                ElementId::Name(format!("rp-open-{key}").into()),
                Timing::fast(),
                move |t| {
                    div()
                        .id(ElementId::Name(format!("rp-open-btn-{key}").into()))
                        .role(Role::Button)
                        .button_text()
                        .aria_label(label)
                        .size(u(18.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(mix(dim, text, t))
                        .cursor_pointer()
                        .on_click(toggle)
                        .child(fa_glyph(
                            if open {
                                "fas fa-chevron-down"
                            } else {
                                "fas fa-chevron-right"
                            },
                            10.,
                        ))
                },
            );
            let state = match self.tree.group_state(group) {
                GroupState::All => Checked::On,
                GroupState::Some => Checked::Mixed,
                GroupState::None => Checked::Off,
            };
            let set = group.clone();
            let pick = check(
                ElementId::Name(format!("rp-folder-{}", group.key).into()),
                state,
                CheckLook::Native,
            )
            .flex_1()
            .min_w_0()
            .gap(u(8.))
            .on_change(cx.listener(move |this, value: &bool, _, cx| {
                let value = *value;
                let set = set.clone();
                this.edit(cx, move |tree| tree.set_group(&set, value))
            }))
            .child(div().min_w_0().text_color(text).child(break_all(&format!(
                "{}/ ({})",
                group.key,
                group.files.len()
            ))));
            Self::row()
                .child(crate::snap::target(
                    format!("rp-open-{}", group.key),
                    chevron,
                ))
                .child(pick)
                .child(self.size(self.tree.group_size(group), cx))
        });
        div().children(folder).children(files).into_any_element()
    }

    /// Слот `.rp`: кнопки выбора и прокручиваемый список.
    fn slot(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let head = div()
            .flex()
            .gap(u(8.))
            .child(
                game_button(
                    "rp-all",
                    None,
                    tr(cx, "#L_DEMOS_SELECT_ALL", &[]),
                    Look::Small,
                )
                .on_click(cx.listener(|this, _, _, cx| this.edit(cx, |t| t.set_all(true)))),
            )
            .child(
                game_button(
                    "rp-none",
                    None,
                    tr(cx, "#L_DEMOS_CLEAR_ALL", &[]),
                    Look::Small,
                )
                .on_click(cx.listener(|this, _, _, cx| this.edit(cx, |t| t.set_all(false)))),
            );
        let sections = self.tree.sections();
        let list = crate::scroll::overflow_y("rp-scroll", window, cx)
            .max_h(u(260.))
            .py(u(8.))
            .px(u(10.))
            .bg(white().opacity(0.05))
            .border_1()
            .border_color(theme.color("l-border"))
            .rounded(theme.rem("l-radius-sm"))
            .children(sections.iter().enumerate().map(|(index, section)| {
                let title = section.cfg.then(|| {
                    div().mt(u(2.)).mb(u(4.)).child(spaced(
                        tr(cx, "#L_GM_REPAIR_CFG", &[]).to_uppercase(),
                        11.5,
                        20.,
                        FontWeight::SEMIBOLD,
                        0.08,
                        theme.color("l-text-dim"),
                    ))
                });
                div()
                    .when(index > 0, |el| el.mt(u(10.)))
                    .children(title)
                    .children(section.groups.iter().map(|group| self.group(group, cx)))
            }));
        div()
            .flex()
            .flex_col()
            .gap(u(8.))
            .mt(u(14.))
            .min_w_0()
            .child(head)
            .child(list)
            .into_any_element()
    }
}

impl Render for RepairModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let files = Self::files(cx);
        let stale = match &files {
            Some(files) => !self.modal.is_open() || !self.is_shown(files),
            None => self.modal.is_open(),
        };
        if stale {
            cx.defer_in(window, Self::sync);
        }
        let count = self.tree.chosen_count();
        let size = format_size(self.tree.chosen_bytes(), &|key, args| tr(cx, key, args));
        let request = Confirm {
            title: tr(cx, "#L_C_REPAIR_TITLE", &[]),
            text: tr(cx, "#L_GM_REPAIR_PICK", &[]),
            note: self
                .tree
                .warn_configs()
                .then(|| tr(cx, "#L_GM_REPAIR_CFG_WARN", &[])),
            confirm_label: Some(tr(cx, "#L_GM_REPAIR_FETCH", &[&count.to_string(), &size])),
            ..Confirm::default()
        };
        let slot = self.slot(window, cx);
        let view = cx.entity().downgrade();
        let on_answer: OnAnswer = Rc::new(move |answer, window, cx| {
            view.update(cx, |this, cx| this.answer(answer, window, cx))
                .ok();
        });
        let card = ConfirmCard {
            confirm_disabled: count == 0,
            ring: self.ring,
            slot: Some(slot),
            ..ConfirmCard::new(request, "")
        }
        .render(&self.cancel, on_answer, window, cx);
        let on_close = listen(cx, |this: &mut RepairModal, window, cx| {
            this.answer(Answer::Cancel, window, cx)
        });
        self.modal
            .render(window, cx, on_close, move || card)
            .into_any_element()
    }
}
