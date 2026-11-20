//! Поля подраздела по схеме (`GhostingSettingsExtras.vue` 0.4.9, `gx--no-nav`): колонка полей,
//! разделитель `pz-fields:<игра>` (мин. 340, превью — 400) и место превью (G9) у разделов с
//! кадром; без превью — поля на всю ширину двумя колонками. Строки: переключатель, число со
//! стрелками и ползунком (детент у умолчания), выбор, цвет, текст с выбором файла; шапка — сброс
//! раздела со счётчиком, заметка, ссылка (паки HL1). «Цвет и прицел» — [`super::hud`], код —
//! [`super::code`]; «Моды» — [`super::packs`], оверлей LiveSplit — [`super::livesplit`].

use super::editor::GamePersonalization;
use crate::icons::{fa_glyph, icon};
use crate::kit::button::{Look as BtnLook, button};
use crate::kit::num_step::{field as num_field, field_unit};
use crate::kit::{
    ColorEvent, ColorField, FieldEvent, Item, Kind, Orientation, TextInput, dropdown, gx, num_step,
    pane_splitter, slider, toggle,
};
use crate::layer::anchor_probe;
use crate::motion::{Timing, hover, mix};
use crate::scale::{u, zoom};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext, ClickEvent, Context, Div, ElementId, Entity, Focusable, FontWeight,
    IntoElement, ParentElement, SharedString, Styled, Window, div, prelude::*, rgb,
};
use hlsr_core::json_text::js_number;
use hlsr_ui::controllers::pane_size;
use hlsr_ui::controllers::settings_extras::{ExtrasEditor, FieldView, js_string};
use serde_json::{Map, Value, json};

/// Колонка полей не уже 340, превью — не уже 400 (`:min` и `:rest` разделителя).
const FIELDS_MIN: f32 = 340.;
const STAGE_MIN: f32 = 400.;

/// `.gx-row__error`: 12 px / 1.4, `#ff8a96`.
fn error_note(text: String) -> Div {
    div()
        .text_px(12., 12. * 1.4)
        .text_color(rgb(0xff8a96))
        .child(text)
}

impl GamePersonalization {
    /// Ширина колонки полей (`grid-template-columns: clamp(340px, var(--gx-fields),
    /// calc(100% - 412px))`, `--gx-fields-w: clamp(340px, 30%, 440px)` у редактора).
    fn fields_width(&self, width: f32) -> f32 {
        let base = self
            .fields_w
            .map_or_else(|| (width * 0.3).clamp(FIELDS_MIN, 440.), |w| w as f32);
        base.min(width - STAGE_MIN - 12.).max(FIELDS_MIN)
    }

    /// Ширина разделителя записана (`pane["pz-fields:<игра>"]` в prefs).
    fn save_pane(&mut self, size: Option<f64>, cx: &mut Context<Self>) {
        let key = format!("pz-fields:{}", self.host.game_id);
        if let Some(root) =
            crate::shell::root::ShellContext::get(cx).and_then(|context| context.root.upgrade())
        {
            let prefs = root.read(cx).prefs();
            let mut prefs = prefs.borrow_mut();
            let pane = pane_size::write_size(prefs.get("pane"), &key, size);
            let mut changes = Map::new();
            changes.insert("pane".into(), pane);
            prefs.save(&changes);
        }
    }

    /// `set(field, value)`: новое значение вида.
    fn set_field(&mut self, field: FieldView, value: Value, cx: &mut Context<Self>) {
        let Some(look) = self.look.as_mut() else {
            return;
        };
        let next = look.extras.set(&field, value);
        self.input(next, cx);
    }

    fn with_extras(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut ExtrasEditor) -> Option<Value>,
    ) {
        let Some(look) = self.look.as_mut() else {
            return;
        };
        if let Some(next) = f(&mut look.extras) {
            self.input(next, cx);
        } else {
            cx.notify();
        }
    }

    /// `pickAsset(field)`: спрей, шрифт HUD или модель призрака.
    fn pick_asset(&mut self, field: FieldView, cx: &mut Context<Self>) {
        let Some(kind) = self
            .look
            .as_mut()
            .and_then(|look| look.extras.begin_pick(&field))
        else {
            return;
        };
        let args = vec![json!(self.host.game_id), json!(kind)];
        cx.notify();
        self.call(
            "game-personalization-pick-asset",
            args,
            cx,
            move |this, reply, cx| {
                this.with_extras(cx, |extras| extras.asset_picked(&field, reply));
            },
        );
    }

    /// Поле числа: текст — за значением, пока поле не в фокусе.
    fn number_input(
        &mut self,
        field: FieldView,
        text: String,
        off: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        let input = match self.widgets.numbers.get(field.key) {
            Some(input) => input.clone(),
            None => {
                let (min, max, step) = field.range().unwrap_or((0., 0., 1.));
                let num = |v: f64| SharedString::from(js_number(v).to_string());
                let unit = field.unit.is_some();
                let start = text.clone();
                let input = cx.new(|cx| {
                    TextInput::new(window, cx)
                        .kind(Kind::Number {
                            min: num(min),
                            max: num(max),
                            step: num(step),
                        })
                        .style(if unit { field_unit } else { num_field })
                        .align_right()
                        .with_text(start)
                });
                let sub = cx.subscribe(&input, move |this, input, event: &FieldEvent, cx| {
                    if *event != FieldEvent::Change {
                        return;
                    }
                    let text = input.read(cx).value();
                    let Some(look) = this.look.as_mut() else {
                        return;
                    };
                    let (next, shown) = look.extras.set_number(&field, json!(text));
                    input.update(cx, |input, cx| {
                        if input.text() != shown {
                            input.set_text(shown, cx);
                        }
                    });
                    if let Some(next) = next {
                        this.input(next, cx);
                    }
                });
                self.subs.push(sub);
                self.widgets.numbers.insert(field.key, input.clone());
                input
            }
        };
        let focused = input.read(cx).focus_handle(cx).is_focused(window);
        if !focused && input.read(cx).text() != text {
            input.update(cx, |input, cx| input.set_text(text, cx));
        }
        if self.widgets.numbers_off.contains(field.key) != off {
            if off {
                self.widgets.numbers_off.insert(field.key);
            } else {
                self.widgets.numbers_off.remove(field.key);
            }
            input.update(cx, |input, cx| input.set_disabled(off, cx));
        }
        input
    }

    /// Текстовое поле: `@input` — сразу в вид.
    fn text_input(
        &mut self,
        field: FieldView,
        text: String,
        invalid: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        let input = match self.widgets.texts.get(field.key) {
            Some(input) => input.clone(),
            None => {
                let placeholder = self.t("#L_GS_X_EMPTY_VALUE", &[]);
                let start = text.clone();
                let input = cx.new(|cx| {
                    TextInput::new(window, cx)
                        .max_length(field.max_length())
                        .placeholder(placeholder)
                        .with_text(start)
                });
                let sub = cx.subscribe(&input, move |this, input, event: &FieldEvent, cx| {
                    if *event == FieldEvent::Input {
                        let text = input.read(cx).text().to_owned();
                        this.set_field(field, json!(text), cx);
                    }
                });
                self.subs.push(sub);
                self.widgets.texts.insert(field.key, input.clone());
                input
            }
        };
        let focused = input.read(cx).focus_handle(cx).is_focused(window);
        input.update(cx, |input, cx| {
            if !focused && input.text() != text {
                input.set_text(text, cx);
            }
            input.set_invalid(invalid, cx);
        });
        input
    }

    /// Поле цвета (`<input type="color">`).
    fn color_input(
        &mut self,
        field: FieldView,
        hex: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ColorField> {
        let picker = match self.widgets.colors.get(field.key) {
            Some(picker) => picker.clone(),
            None => {
                let label = self.t(field.label, &[]);
                let picker =
                    cx.new(|cx| ColorField::new(window, cx).with_hex(hex, cx).label(label));
                let sub = cx.subscribe(&picker, move |this, _, ColorEvent::Input(hex), cx| {
                    let hex = hex.to_string();
                    this.with_extras(cx, |extras| extras.set_color(&field, &hex));
                });
                self.subs.push(sub);
                self.widgets.colors.insert(field.key, picker.clone());
                picker
            }
        };
        if picker.read(cx).hex() != hex {
            let hex = hex.to_owned();
            picker.update(cx, |picker, cx| picker.set_hex(&hex, cx));
        }
        picker
    }

    /// Строка поля `.gx-row`.
    fn field_row(
        &mut self,
        field: FieldView,
        packs: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let Some(look) = self.look.as_ref() else {
            return div().into_any_element();
        };
        let extras = &look.extras;
        let key = field.key;
        let label = self.t(field.label, &[]);
        let changed = extras.is_changed(&field);
        let off = !extras.is_available(&field);
        let resolved = extras.resolved_value(key);
        let invalid = extras.invalid_fields().contains(key);
        let asset_error = extras.asset_errors.get(key).copied();
        let picking = extras.picking_asset.clone();
        let needs = off.then(|| {
            let requirement = self.t(&extras.requirement_label(&field), &[]);
            self.t("#L_GS_X_REQUIRES", &[&requirement])
        });
        let select_label = self.t(&extras.select_label(&field), &[]);
        let slider_value = extras.slider_value(&field);
        let hex = ExtrasEditor::to_hex(&resolved);
        let id = |part: &str| ElementId::Name(format!("gx-{part}-{key}").into());

        let mut row = gx::row(id("row"), label.clone()).changed(changed).off(off);
        if packs {
            row = row.stack();
        }
        let kind = field.type_name();
        match kind {
            "boolean" => {
                let on = hlsr_core::js::truthy(&resolved);
                let mut sw = toggle(id("switch"), on).label(label.clone());
                if !off {
                    sw = sw.on_change(cx.listener(move |this, on: &bool, _, cx| {
                        this.set_field(field, json!(*on), cx)
                    }));
                    row = row.on_label(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_field(field, json!(!on), cx)
                    }));
                }
                row = row.control(sw);
            }
            "number" => {
                let input = self.number_input(field, js_string(&resolved), off, window, cx);
                let mut step = num_step(id("num"), input).disabled(off);
                if let Some(unit) = field.unit {
                    step = step.unit(unit);
                }
                row = row.control(div().flex_shrink(1.).min_w(u(64.)).w(u(92.)).child(step));
                if ExtrasEditor::has_range(&field) {
                    let (min, max, _) = field.range().unwrap_or((0., 1., 1.));
                    let default = field.default_value().as_f64().unwrap_or(min);
                    let mut range = slider(id("range"), slider_value, min, max)
                        .default_value(default)
                        .label(label.clone());
                    if !off {
                        // Детент к умолчанию делает `kit::slider` (`Range::at`); ширина 0 — без второго.
                        range = range
                            .on_input(cx.listener(move |this, raw: &f64, _, cx| {
                                let raw = *raw;
                                this.with_extras(cx, |extras| extras.slide(&field, raw, 0.));
                            }))
                            .on_change(cx.listener(move |this, raw: &f64, _, cx| {
                                let raw = *raw;
                                this.with_extras(cx, |extras| extras.end_slide(&field, raw, 0.));
                            }));
                    }
                    row = row.below(match field.scale {
                        Some([start, end]) => {
                            gx::slider_ends(self.t(start, &[]), range, self.t(end, &[]), &theme)
                                .into_any_element()
                        }
                        None => div().mt(u(2.)).child(range).into_any_element(),
                    });
                }
            }
            "select" => {
                let items: Vec<Item> = field
                    .options()
                    .iter()
                    .map(|option| Item::new(option.value, self.t(option.label, &[])))
                    .collect();
                let value = resolved.as_str().map(|v| SharedString::from(v.to_owned()));
                let toggle_id = format!("gx-select-{key}");
                let mut menu = dropdown(toggle_id.clone(), items, value)
                    .align_right()
                    .label(label.clone())
                    .toggle(move |state| {
                        div().w_full().child(gx::select(
                            format!("{toggle_id}-box"),
                            select_label,
                            state.open,
                            state.focus,
                        ))
                    });
                if !off {
                    menu = menu.on_select(cx.listener(move |this, item: &Item, _, cx| {
                        this.set_field(field, json!(item.id.to_string()), cx)
                    }));
                }
                row = row.control(if packs {
                    div().w_full().child(menu)
                } else {
                    div().flex_shrink(1.).min_w_0().w(u(164.)).child(menu)
                });
            }
            "color" => {
                let picker = self.color_input(field, &hex, window, cx);
                row = row.control(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(10.))
                        .child(picker)
                        .child(gx::code(hex.to_uppercase(), &theme)),
                );
            }
            _ => {}
        }
        if let Some(hint) = field.hint {
            row = row.hint(self.t(hint, &[]));
        }
        if kind == "text" {
            let input = self.text_input(field, js_string(&resolved), invalid, window, cx);
            let kind = ExtrasEditor::asset_kind(&field);
            let browse = (!kind.is_empty()).then(|| {
                let busy = picking == key;
                let browse_label = self.t("#L_GS_X_BROWSE", &[]);
                button(id("browse"), BtnLook::GxIcon)
                    .label(format!("{browse_label}: {label}"))
                    .disabled(!picking.is_empty() || off)
                    .on_click(
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.pick_asset(field, cx)),
                    )
                    .child(if busy {
                        icon("fas fa-spinner fa-spin", 13., theme.color("l-text-dim"))
                    } else {
                        fa_glyph("fas fa-folder-open", 13.).into_any_element()
                    })
            });
            row = row.below(
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .child(div().flex_1().min_w_0().child(input))
                    .children(browse),
            );
        }
        // Ошибки — под полем, бледнеют вместе со строкой (`.is-off > :not(.gx-row__needs)`).
        if invalid {
            row = row.below(error_note(self.t(ExtrasEditor::error_key(key), &[])));
        }
        if let Some(error) = asset_error {
            row = row.below(error_note(self.t(error, &[])));
        }
        if let Some(needs) = needs {
            row = row.needs(needs);
        }
        row.into_any_element()
    }

    /// Ссылка шапки (`.gx-pane__link` у паков HL1): без рамки, по левому краю.
    fn pane_link(&self, link: &'static str, title: String, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>();
        let (dim, text) = (theme.color("l-text-dim"), theme.color("l-text"));
        let open = cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.fire("open-external-link", vec![json!(link)], cx)
        });
        hover("gx-pane-link", Timing::fast(), move |t| {
            div()
                .id("gx-pane-link")
                .flex()
                .items_center()
                .gap(u(7.))
                .min_h(u(20.))
                .text_px(12., 12.)
                .font_weight(FontWeight::MEDIUM)
                .text_color(mix(dim, text, t))
                .cursor_pointer()
                .button_text()
                .on_click(open)
                .child(fa_glyph("fas fa-external-link-alt", 11.))
                .child(title)
        })
        .into_any_element()
    }

    /// Раздел схемы `.gx-pane`: шапка и поля (без превью — двумя колонками).
    fn schema_pane(
        &mut self,
        wide: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let Some(look) = self.look.as_ref() else {
            return div().into_any_element();
        };
        let Some(section) = look.extras.active_section() else {
            return div().into_any_element();
        };
        let columns = look.extras.field_columns();
        let changes = look.extras.reset_changes();
        let packs = section.id == "hl1packs";
        let reset_label = if changes > 0 {
            format!(
                "{} — {}",
                self.t("#L_GS_X_RESET_SECTION", &[]),
                self.t("#L_GS_X_CHANGED", &[&changes.to_string()])
            )
        } else {
            self.t("#L_GS_X_RESET_SECTION", &[])
        };
        let reset = button("gx-reset", BtnLook::Gx)
            .label(reset_label)
            .disabled(changes == 0)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.with_extras(cx, |extras| extras.reset_section())
            }))
            .child(fa_glyph("fas fa-undo-alt", 11.))
            .when(changes > 0, |el| el.child(gx::count(changes, &theme)));
        let upper = section.id.to_uppercase();
        let mut head = gx::pane_head(self.t(section.title, &[]))
            .reset(reset)
            .packs(packs)
            .note(self.t(&format!("#L_GS_X_NOTE_{upper}"), &[]));
        if let Some(link) = section.link {
            let title = self.t(&format!("#L_GS_X_LINK_{upper}"), &[]);
            head = head.child(self.pane_link(link, title, cx));
        }
        let mut cols = Vec::new();
        for column in columns {
            let rows: Vec<AnyElement> = column
                .into_iter()
                .map(|field| self.field_row(field, packs, window, cx))
                .collect();
            cols.push(div().flex_1().min_w_0().flex().flex_col().children(rows));
        }
        let body = if wide {
            div().flex().items_start().children(cols)
        } else {
            div().children(cols)
        };
        gx::pane(&theme)
            .bg(gpui::transparent_black())
            .size_full()
            .child(head)
            .child(
                gx::fields(
                    ElementId::Name(format!("gx-fields-{}", section.id).into()),
                    &self.widgets.fields_scroll,
                )
                .child(body),
            )
            .into_any_element()
    }

    /// Оверлей LiveSplit (`<LiveSplitOverlayEditor v-else-if>`): создаётся при открытии
    /// подраздела, потом получает новое значение.
    fn livesplit(&mut self, cx: &mut Context<Self>) -> AnyElement {
        use super::livesplit::{LiveSplitOverlay, LsoEvent};
        let overlay = self.panel.overlay();
        if let Some((view, _)) = &self.widgets.livesplit {
            view.update(cx, |view, cx| view.set_value(overlay, cx));
            return view.clone().into_any_element();
        }
        let (services, language, game) = (
            self.host.services.clone(),
            self.host.language,
            self.host.game_id.clone(),
        );
        let view = cx.new(|cx| LiveSplitOverlay::new(overlay, &game, services, language, cx));
        let sub = cx.subscribe(&view, |this, _, event, cx| match event {
            LsoEvent::Input(value) => this.overlay_input(value.clone(), cx),
            LsoEvent::Snapshot(snapshot) => {
                this.widgets.overlay_snapshot = snapshot.clone();
                cx.notify();
            }
        });
        self.widgets.livesplit = Some((view.clone(), sub));
        view.into_any_element()
    }

    /// Моды (`<CustomPacks v-else-if>`): список грузится при каждом открытии подраздела.
    fn packs(&mut self, cx: &mut Context<Self>) -> AnyElement {
        use super::packs::{CustomPacksView, PacksEvent};
        if let Some((view, _)) = &self.widgets.packs {
            return view.clone().into_any_element();
        }
        let Some(look) = self.look.as_ref() else {
            return div().into_any_element();
        };
        let active = look.extras.active_nav_id();
        let title = look
            .extras
            .nav_items()
            .into_iter()
            .find(|item| item.id == active)
            .map(|item| self.t(item.title, &[]))
            .unwrap_or_default();
        let (services, language, game) = (
            self.host.services.clone(),
            self.host.language,
            self.host.game_id.clone(),
        );
        let view = cx.new(|cx| CustomPacksView::new(&game, title, services, language, cx));
        let sub = cx.subscribe(&view, |this, _, PacksEvent(packs), cx| {
            if let Some(look) = this.look.as_mut() {
                look.mod_packs = Some(packs.clone());
            }
            cx.notify();
        });
        self.widgets.packs = Some((view.clone(), sub));
        view.into_any_element()
    }

    /// Редактор подраздела: колонка полей, разделитель и превью или одна колонка на всю ширину.
    pub(super) fn render_extras(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zoom = zoom(window);
        // Ширина области — рамкой прошлого кадра; изменилась — ещё кадр с новой.
        let width = f32::from(self.widgets.probes[0].get().size.width) / zoom;
        if (width - self.widgets.width).abs() > 0.5 {
            self.widgets.width = width;
            crate::motion::monitor_notify(window, cx.entity_id());
        }
        let Some(look) = self.look.as_ref() else {
            return div().into_any_element();
        };
        let (has_preview, personal, code, livesplit, mods) = (
            look.extras.has_preview(),
            look.extras.is_personalization(),
            look.extras.is_code(),
            look.extras.is_livesplit(),
            look.extras.is_mods(),
        );
        // `v-if`: подраздел закрыт — его вид уходит (таймер снимков, звук, список).
        if !livesplit {
            self.widgets.livesplit = None;
        }
        if !mods {
            self.widgets.packs = None;
        }
        let fields_w = self.fields_width(width);
        let body = if personal {
            self.render_hud(fields_w - 30., window, cx)
        } else if code {
            self.render_code(width, window, cx)
        } else if livesplit {
            self.livesplit(cx)
        } else if mods {
            self.packs(cx)
        } else {
            self.schema_pane(!has_preview, window, cx)
        };
        let [area, pane, stage] = self.widgets.probes.clone();
        let root = div()
            .relative()
            .flex()
            .size_full()
            .min_h_0()
            .child(anchor_probe(area));
        if !has_preview {
            // `:active="active && hasPreview"`: превью без кадра не рисует.
            if let Some(preview) = &self.widgets.preview {
                preview.update(cx, |p, _| p.set_active(false));
            }
            return root
                .child(div().flex_1().min_w_0().h_full().child(body))
                .into_any_element();
        }
        let preview = self.preview(window, cx);
        let hint = self.t("#L_SPLIT_HINT", &[]);
        let label = self.t("#L_SPLIT_FIELDS", &[]);
        root.child(
            div()
                .relative()
                .flex_none()
                .w(u(fields_w))
                .h_full()
                .min_h_0()
                .flex()
                .flex_col()
                .child(anchor_probe(pane.clone()))
                .child(body),
        )
        .child(
            pane_splitter(
                "pz-fields",
                Orientation::Vertical,
                f64::from(FIELDS_MIN),
                self.fields_w,
            )
            .panes(pane, stage.clone())
            .rest(f64::from(STAGE_MIN))
            .label(label)
            .hint(hint)
            .on_input(cx.listener(|this, size: &Option<f64>, _, cx| {
                this.fields_w = *size;
                cx.notify();
            }))
            .on_save(cx.listener(|this, size: &Option<f64>, _, cx| this.save_pane(*size, cx))),
        )
        // Превью (`GhostingSettingsPreview`, G9): `align-self: start`, портрет — во всю высоту.
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .items_start()
                .child(anchor_probe(stage))
                .child(preview),
        )
        .into_any_element()
    }

    /// Превью раздела с кадром: при первом показе создаётся (ему нужно окно), потом получает
    /// новые `props` редактора.
    fn preview(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<super::preview::GhostingPreview> {
        use super::preview::{GhostingPreview, Inputs, PreviewEvent};
        use hlsr_ui::controllers::preview::Variant;
        use hlsr_ui::controllers::settings_extras::{Schema, hud_mod};
        let look = self.look.as_ref().expect("редактор с превью");
        let inputs = Inputs {
            value: look.extras.value(),
            section: look.extras.preview_section(),
            active: true,
            overlay: look.extras.overlay().cloned(),
            overlay_snapshot: self.widgets.overlay_snapshot.clone(),
            overlay_editing: look.extras.is_livesplit(),
            hud_mod: hud_mod(self.schema, look.mod_packs.as_ref()),
        };
        if let Some(preview) = &self.widgets.preview {
            preview.update(cx, |p, _| p.set_inputs(inputs));
            return preview.clone();
        }
        let variant = if self.schema == Schema::Goldsrc {
            Variant::Hl1
        } else {
            Variant::Ghosting
        };
        let (services, language, game) = (
            self.host.services.clone(),
            self.host.language,
            self.host.game_id.clone(),
        );
        let preview = cx
            .new(|cx| GhostingPreview::new(variant, &game, services, language, inputs, window, cx));
        self.subs
            .push(cx.subscribe(&preview, |this, _, event, cx| match event {
                PreviewEvent::Overlay(value) => this.overlay_input(value.clone(), cx),
            }));
        self.widgets.preview = Some(preview.clone());
        preview
    }
}
