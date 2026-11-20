//! «Код настроек» (`GamePersonalizationCode.vue` 0.4.9): свой код — выгрузить и скопировать
//! (`game-personalization-code-export`, буфер обмена, достижение «Франкенштейн»), чужой —
//! вставить и импортировать (`game-personalization-code-import`); моды из кода переключаются
//! сразу (`custom-packs-get`, затем `custom-packs-set` по одному), итог — заметками под формой.
//! Логика — `LookCode` и `ModsApply`.

use super::editor::GamePersonalization;
use crate::icons::icon;
use crate::kit::button::{focus_handle, focus_visible, outline};
use crate::kit::{FieldEvent, FieldStyle, TextInput};
use crate::motion::{Timing, hover, mix};
use crate::scale::{u, zoom};
use crate::scroll::{Bar, ScrollArea};
use crate::text::{MONO, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext, ClickEvent, ClipboardItem, Context, Div, ElementId, Focusable,
    FontWeight, IntoElement, ParentElement, Role, Styled, Window, div, prelude::*, rgb,
};
use hlsr_ui::controllers::look_code::{LookCode, ModsApply};
use hlsr_ui::controllers::settings_extras::Schema;
use serde_json::{Value, json};

/// Поле кода внутри рамки `.look-code__field`: без своей рамки и фона, моноширинное 12 px / 1.6.
fn code_field(theme: &Theme) -> FieldStyle {
    let clear = gpui::transparent_black();
    FieldStyle {
        height: 12. * 1.6,
        pad_x: 0.,
        family: MONO,
        size: 12.,
        line: 12. * 1.6,
        bg: clear,
        hover_bg: clear,
        border: clear,
        hover_border: clear,
        focus_border: clear,
        invalid_border: clear,
        glow: None,
        disabled_opacity: 0.45,
        ..FieldStyle::gx(theme)
    }
}

impl GamePersonalization {
    fn with_code<R>(&mut self, f: impl FnOnce(&mut LookCode) -> R) -> Option<R> {
        self.look.as_mut().map(|look| f(&mut look.code))
    }

    /// `copyCode()`: код с сервера main, затем в буфер обмена.
    fn copy_code(&mut self, cx: &mut Context<Self>) {
        let Some(revision) = self.with_code(LookCode::begin_copy).flatten() else {
            return;
        };
        let look = self
            .with_code(|code| code.look().clone())
            .unwrap_or(Value::Null);
        let args = vec![json!(self.host.game_id), look];
        cx.notify();
        self.call(
            "game-personalization-code-export",
            args,
            cx,
            move |this, reply, cx| {
                let Some(code) = this
                    .with_code(|state| state.export_reply(revision, reply))
                    .flatten()
                else {
                    return;
                };
                // Журнал 0.4.9 ведёт вызов канала буфера обмена preload; запись — окно.
                if let Some(args) = hlsr_ui::clipboard::bridge_args(&code) {
                    this.fire(hlsr_ui::clipboard::BRIDGE_CHANNEL, args, cx);
                }
                cx.write_to_clipboard(ClipboardItem::new_string(code));
                this.with_code(|state| state.copy_done(revision, true));
                this.fire("runner-secret-unlock", vec![json!("frankenstein")], cx);
            },
        );
    }

    /// `importCode()`: вид из кода, моды — сразу, затем `import` в черновик.
    fn import_code(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.with_code(LookCode::begin_import).flatten() else {
            return;
        };
        let game = self.host.game_id.clone();
        cx.notify();
        self.call(
            "game-personalization-code-import",
            vec![json!(game), json!(text)],
            cx,
            move |this, reply, cx| {
                let Some(imported) = this
                    .with_code(|code| code.import_reply(&game, reply))
                    .flatten()
                else {
                    return;
                };
                match imported.mods {
                    None => this.import_code_look(&imported.look, None, cx),
                    Some(mods) => {
                        let look = imported.look;
                        let args = vec![json!(game)];
                        this.call("custom-packs-get", args, cx, move |this, reply, cx| {
                            let apply = ModsApply::start(mods, reply);
                            this.apply_mods(apply, look, cx);
                        });
                    }
                }
            },
        );
    }

    /// Следующий мод из кода — или итог: заметки и вид в черновик.
    fn apply_mods(&mut self, mut apply: ModsApply, look: Value, cx: &mut Context<Self>) {
        let Some((id, want)) = apply.next_set() else {
            let language = self.host.language;
            let t = move |key: &str, args: &[&str]| hlsr_ui::tr(language, key, args);
            let notes = apply.notes(&t);
            self.with_code(|code| code.mods_done(notes));
            self.import_code_look(&look, apply.packs(), cx);
            return;
        };
        let args = vec![json!(self.host.game_id), json!(id), json!(want)];
        self.call("custom-packs-set", args, cx, move |this, reply, cx| {
            apply.set_reply(&id, want, reply);
            this.apply_mods(apply, look, cx);
        });
    }

    /// Поле вставки кода (одна строка внутри рамки `textarea`).
    fn code_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Entity<TextInput> {
        if let Some(input) = &self.widgets.code_input {
            return input.clone();
        }
        let placeholder = self.t("#L_PZ_CODE_IMPORT_PLACEHOLDER", &[]);
        let input = cx.new(|cx| {
            TextInput::new(window, cx)
                .style(code_field)
                .max_length(LookCode::MAX_LENGTH)
                .placeholder(placeholder)
        });
        self.subs
            .push(cx.subscribe(&input, |this, input, event: &FieldEvent, cx| {
                if *event == FieldEvent::Input {
                    let text = input.read(cx).text().to_owned();
                    this.with_code(|code| code.set_code_input(&text));
                    cx.notify();
                }
            }));
        self.widgets.code_input = Some(input.clone());
        input
    }

    /// Раздел «Код настроек» (`.gx-code` → `.look-code`): `width` — ширина области.
    pub(super) fn render_code(
        &mut self,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let input = self.code_input(window, cx);
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let area_h = f32::from(self.widgets.probes[0].get().size.height) / zoom;
        let Some(code) = self.look.as_ref().map(|look| &look.code) else {
            return div().into_any_element();
        };
        let busy = code.busy.is_some();
        // Текст поля — только от игрока (`v-model`): обратно из состояния не пишется — отрисовка
        // между нажатиями (до событий поля) стёрла бы набранное.
        input.update(cx, |input, cx| {
            input.set_disabled(code.disabled || busy, cx)
        });
        let (text, dim, border) = (
            theme.color("l-text"),
            theme.color("l-text-dim"),
            theme.color("l-border"),
        );
        let error_color = rgb(0xff8a98);
        let note = |text: String, color| div().text_px(12., 18.).text_color(color).child(text);
        // `repeat(auto-fit, minmax(min(100%, 280px), 1fr))`, зазор 20.
        let inner = (width - 42.).max(0.);
        // auto-fit: пустые дорожки схлопываются — две формы делят всю ширину.
        let cols = (((inner + 20.) / 300.).floor() as usize).clamp(1, 2);
        let field_box = |invalid: bool| {
            div()
                .w_full()
                .h(u(128.))
                .p(u(12.))
                .border_1()
                .border_color(if invalid { error_color.into() } else { border })
                .rounded(theme.rem("l-radius-sm"))
                .bg(theme.color("l-modal-bar"))
                .font_family(MONO)
                .text_px(12., 12. * 1.6)
                .overflow_hidden()
        };
        let form = |label: String, hint: String| {
            div()
                .flex()
                .flex_col()
                .items_start()
                .w_full()
                .min_w_0()
                .child(
                    div()
                        .text_px(14., 20.)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(label),
                )
                .child(
                    div()
                        .min_h(u(36.))
                        .mt(u(6.))
                        .mb(u(12.))
                        .text_px(12., 18.)
                        .text_color(dim)
                        .child(hint),
                )
        };
        let export_text = if code.export_code.is_empty() {
            div()
                .text_color(dim)
                .child(self.t("#L_PZ_CODE_EXPORT_PLACEHOLDER", &[]))
        } else {
            div().text_color(text).child(code.export_code.clone())
        };
        let copy_icon = code.copy_icon();
        let copy = code_button(
            "look-code-copy",
            false,
            !code.can_copy(),
            icon(copy_icon, 13., text),
            self.t(
                if code.copied {
                    "#L_PZ_CODE_COPIED"
                } else {
                    "#L_PZ_CODE_COPY"
                },
                &[],
            ),
            cx.listener(|this, _: &ClickEvent, _, cx| this.copy_code(cx)),
            &theme,
            zoom,
            window,
            cx,
        );
        let import = code_button(
            "look-code-import",
            true,
            !code.can_import(),
            icon(code.import_icon(), 13., text),
            self.t("#L_PZ_CODE_IMPORT", &[]),
            cx.listener(|this, _: &ClickEvent, _, cx| this.import_code(cx)),
            &theme,
            zoom,
            window,
            cx,
        );
        let export_form = form(
            self.t("#L_PZ_CODE_YOURS", &[]),
            self.t("#L_PZ_CODE_EXPORT_HINT", &[]),
        )
        .child(field_box(false).child(export_text))
        .child(copy);
        let import_form = form(
            self.t("#L_PZ_CODE_IMPORT_LABEL", &[]),
            self.t("#L_PZ_CODE_IMPORT_HINT", &[]),
        )
        .child(crate::snap::target(
            "look-code-field",
            field_box(!code.import_error.is_empty())
                .id("look-code-field")
                .cursor_text()
                // Щелчок по рамке — в поле, как по `textarea`.
                .on_mouse_down(gpui::MouseButton::Left, {
                    let focus = input.focus_handle(cx);
                    move |_, window, cx| {
                        window.focus(&focus, cx);
                        // Фокус у поля: предки с `track_focus` его не забирают.
                        window.prevent_default();
                    }
                })
                .child(input),
        ))
        .child(crate::snap::target("look-code-import", import))
        .when(!code.import_error.is_empty(), |el| {
            el.child(note(self.t(code.import_error, &[]), error_color.into()).mt(u(12.)))
        })
        .children(code.mod_notes.iter().map(|n| {
            note(
                n.text.clone(),
                if n.error { error_color.into() } else { dim },
            )
            .mt(u(12.))
        }));
        let tab = if self.schema == Schema::Goldsrc {
            "#L_CPK_TAB_HL1"
        } else {
            "#L_CPK_TAB_GHOSTING"
        };
        let files = |key: String| note(key, dim).mt(u(20.));
        let section = div()
            .p(u(20.))
            .min_h(u(area_h))
            .border_1()
            .border_color(border)
            .rounded(theme.rem("l-radius"))
            .text_color(text)
            .child(
                div()
                    .mb(u(8.))
                    .text_px(20., 28.)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.t("#L_PZ_CODE_TITLE", &[])),
            )
            .child(
                div()
                    .max_w(u(680.))
                    .text_px(13., 20.)
                    .text_color(dim)
                    .child(self.t("#L_PZ_CODE_LEAD", &[code.game_label()])),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(cols as u16)
                    .gap(u(20.))
                    .mt(u(20.))
                    .child(export_form)
                    .child(import_form),
            )
            .when(!code.export_error.is_empty(), |el| {
                el.child(note(self.t(code.export_error, &[]), error_color.into()).mt(u(12.)))
            })
            .when(code.invalid, |el| {
                el.child(note(self.t("#L_GS_MOD_INVALID", &[]), error_color.into()).mt(u(12.)))
            })
            .child(files(self.t("#L_PZ_CODE_MODS_NOTE", &[&self.t(tab, &[])])))
            .when(code.files_note(), |el| {
                el.child(files(self.t("#L_PZ_CODE_FILES", &[])))
            });
        ScrollArea::new("look-code-scroll", &self.widgets.code_scroll)
            .bar(Bar::Fields)
            .contain()
            .size_full()
            .child(section)
            .into_any_element()
    }
}

/// Кнопка `.look-code__button` (`is-primary` — `--accent-soft`): 36 px, поля 16, 13 px / 600.
#[allow(clippy::too_many_arguments)]
fn code_button(
    id: &'static str,
    primary: bool,
    disabled: bool,
    glyph: AnyElement,
    label: String,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
    theme: &Theme,
    zoom: f32,
    window: &mut Window,
    cx: &mut gpui::App,
) -> Div {
    let id = ElementId::Name(id.into());
    let focus = focus_handle(&id, window, cx);
    let ring_on = focus_visible(&focus, window);
    let (border, hover_border, bg, hover_bg) = if primary {
        (
            theme.color("accent-border"),
            theme.color("accent"),
            theme.color("accent-soft"),
            // `var(--accent-soft-hover, var(--accent-soft))`.
            theme
                .try_color("accent-soft-hover")
                .unwrap_or_else(|| theme.color("accent-soft")),
        )
    } else {
        (
            theme.color("l-border"),
            theme.color("l-border-strong"),
            theme.color("l-modal-bar"),
            theme.color("l-surface"),
        )
    };
    let focus_color = theme.color("accent-focus");
    let radius = theme.len("l-radius-sm");
    let live = !disabled;
    let inner = id.clone();
    div().mt(u(12.)).child(hover(id, Timing::fast(), move |t| {
        let t = if live { t } else { 0. };
        div()
            .id(inner)
            .role(Role::Button)
            .button_text()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .gap(u(8.))
            .min_h(u(36.))
            .px(u(16.))
            .border_1()
            .border_color(mix(border, hover_border, t))
            .rounded(u(radius))
            .bg(mix(bg, hover_bg, t))
            .text_px(13., 13. * 1.2)
            .font_weight(FontWeight::SEMIBOLD)
            .child(glyph)
            .child(label)
            .when(!live, |el| el.opacity(0.45))
            .when(live, |el| {
                el.track_focus(&focus).cursor_pointer().on_click(on_click)
            })
            .when(ring_on, |el| {
                el.child(outline(zoom, radius, 2., 2., focus_color))
            })
    }))
}
