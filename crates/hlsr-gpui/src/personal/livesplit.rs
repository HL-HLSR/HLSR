//! Оверлей LiveSplit в окне игры (`LiveSplitOverlayEditor.vue` 0.4.9, он же 0.4.7): раздел
//! `gx-pane` с карточкой источника (снимок окна LiveSplit или пример; сам — раз в 2 с на Windows,
//! «Обновить» крутится не меньше 600 мс и коротко подсвечивает карточку), выключатель, якоря
//! 3 × 3 и ползунки X/Y, размер и прозрачность, обрезка, маска (нет, цвет, яркость), цвет ключа с
//! пипеткой «фон». Логика — `LivesplitEditor` (`hlsr-ui`); новые настройки уходят редактору
//! событием [`LsoEvent::Input`], снимок — превью ([`LsoEvent::Snapshot`]).

use crate::icons::{fa_glyph, icon};
use crate::kit::button::{Look as BtnLook, button, focus_handle, focus_visible, outline, white};
use crate::kit::num_step::{field as num_field, field_unit};
use crate::kit::{
    ColorEvent, ColorField, FieldEvent, Kind, TextInput, gx, num_step, slider, toggle,
};
use crate::motion::{Timing, hover, keyframes, mix};
use crate::scale::{u, zoom};
use crate::scroll::Scroll;
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext, ClickEvent, Context, Div, ElementId, Entity, EventEmitter, Focusable,
    FontWeight, IntoElement, ParentElement, Render, Role, SharedString, Styled, Subscription,
    Toggled, Window, div, hsla, prelude::*, rgb, rgba,
};
use hlsr_core::js;
use hlsr_core::json_text::js_number;
use hlsr_core::language::Language;
use hlsr_ui::controllers::livesplit_editor::{
    ANCHORS, AUTO_REFRESH_MS, COLOR_FIELDS, CROP_SIDES, KEYS, LUMA_FIELDS, LivesplitEditor,
    MIN_SPIN_MS, NumberField, SIZE_FIELDS, Tone, crop_max,
};
use hlsr_ui::controllers::preview::Snapshot;
use hlsr_ui::services::Services;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Что редактор оверлея сообщает редактору внешнего вида.
pub enum LsoEvent {
    /// `$emit('input', overlay)`: новые настройки после `sanitizeOverlay`.
    Input(Value),
    /// `$emit('snapshot', …)`: снимок окна LiveSplit для превью (нет — пример).
    Snapshot(Option<Rc<Snapshot>>),
}

impl EventEmitter<LsoEvent> for LiveSplitOverlay {}

/// Поле обрезки: `0…LIMITS.crop[1]`, шаг 1, единица `px`.
fn crop_field() -> NumberField {
    NumberField {
        key: "crop",
        label: "",
        hint: None,
        min: 0.,
        max: crop_max(),
        step: 1.,
        unit: Some("px"),
    }
}

pub struct LiveSplitOverlay {
    editor: LivesplitEditor,
    services: Services,
    language: Language,
    /// Поля чисел по ключу настройки (обрезка — `crop-<сторона>`).
    numbers: HashMap<String, Entity<TextInput>>,
    color: Option<Entity<ColorField>>,
    scroll: Scroll,
    /// Номер вспышки карточки (`is-flash`): новый — анимация заново.
    flash: u64,
    /// Поколение таймера (`clearTimeout`): у ждущего старого — не своё, он ничего не делает.
    timer: u64,
    /// Последний кадр вида и стоит ли таймер (`active` 0.4.9: страница не на экране — снимков нет).
    shown_at: Instant,
    idle: bool,
    subs: Vec<Subscription>,
}

impl LiveSplitOverlay {
    /// Раздел открыт (`watch active, immediate`): снимок сразу, потом сам раз в 2 с.
    pub fn new(
        value: Option<Value>,
        game_id: &str,
        services: Services,
        language: Language,
        cx: &mut Context<Self>,
    ) -> LiveSplitOverlay {
        let mut this = LiveSplitOverlay {
            editor: LivesplitEditor::new(value, game_id, cfg!(target_os = "linux")),
            services,
            language,
            numbers: HashMap::new(),
            color: None,
            scroll: Scroll::new(),
            flash: 0,
            timer: 0,
            shown_at: Instant::now(),
            idle: false,
            subs: Vec::new(),
        };
        this.capture(false, false, None, cx);
        this.schedule(cx);
        this
    }

    /// Новое значение от редактора (`:value`; оверлей могли перетащить в превью).
    pub fn set_value(&mut self, value: Option<Value>, cx: &mut Context<Self>) {
        let changed = !self.editor.same_as(value.as_ref());
        self.editor.set_value(value);
        if changed {
            cx.notify();
        }
    }

    fn t(&self, key: &str, args: &[&str]) -> String {
        hlsr_ui::tr(self.language, key, args)
    }

    /// `schedule()`: следующий тихий снимок через 2 с (под Proton — только кнопкой).
    fn schedule(&mut self, cx: &mut Context<Self>) {
        self.timer += 1;
        if self.editor.linux {
            return;
        }
        let generation = self.timer;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(AUTO_REFRESH_MS))
                .await;
            this.update(cx, |this, cx| {
                if this.timer != generation {
                    return;
                }
                // Кадра не было два срока: вид не на экране — таймер встаёт до следующего кадра.
                if this.shown_at.elapsed() > Duration::from_millis(2 * AUTO_REFRESH_MS) {
                    this.idle = true;
                    return;
                }
                this.capture(true, false, Some(Self::schedule), cx);
            })
            .ok();
        })
        .detach();
    }

    /// `capture(quiet, manual)`: `then` — когда снимок кончился (или уже шёл другой).
    fn capture(
        &mut self,
        quiet: bool,
        manual: bool,
        then: Option<fn(&mut Self, &mut Context<Self>)>,
        cx: &mut Context<Self>,
    ) {
        if !self.editor.begin_capture(quiet) {
            if let Some(then) = then {
                then(self, cx);
            }
            return;
        }
        cx.notify();
        let started = Instant::now();
        let reply = self.services.invoke(
            "livesplit-overlay-capture",
            vec![json!(self.editor.game_id)],
        );
        // Без хранения: вид закрыт — `update` не пройдёт, цепочка кончится сама.
        cx.spawn(async move |this, cx| {
            let reply = reply.await.map_err(|error| format!("{error:?}"));
            let Ok(rest) = this.update(cx, |this, cx| {
                if let Some(snapshot) = this.editor.capture_reply(quiet, reply) {
                    cx.emit(LsoEvent::Snapshot(snapshot.map(Rc::new)));
                }
                cx.notify();
                let spin = Duration::from_millis(if manual { MIN_SPIN_MS } else { 0 });
                spin.saturating_sub(started.elapsed())
            }) else {
                return;
            };
            if !rest.is_zero() {
                cx.background_executor().timer(rest).await;
            }
            this.update(cx, |this, cx| {
                this.editor.end_capture();
                if let Some(then) = then {
                    then(this, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// «Обновить»: снимок, потом вспышка карточки.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.capture(
            false,
            true,
            Some(|this, cx| {
                this.flash += 1;
                cx.notify();
            }),
            cx,
        );
    }

    /// Новые настройки: сразу в своё значение и редактору.
    fn emit_input(&mut self, next: Value, cx: &mut Context<Self>) {
        self.editor.set_value(Some(next.clone()));
        cx.emit(LsoEvent::Input(next));
        cx.notify();
    }

    /// `set(patch)`.
    fn set(&mut self, key: &str, value: Value, cx: &mut Context<Self>) {
        let mut patch = Map::new();
        patch.insert(key.to_owned(), value);
        let next = self.editor.set(patch);
        self.emit_input(next, cx);
    }

    /// Поле числа (`setNumber` / `setCrop` на `change`); текст — за значением, пока поле не в
    /// фокусе.
    fn number_input(
        &mut self,
        key: String,
        field: NumberField,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        let input = match self.numbers.get(&key) {
            Some(input) => input.clone(),
            None => {
                let num = |v: f64| SharedString::from(js_number(v).to_string());
                let start = text.clone();
                let input = cx.new(|cx| {
                    TextInput::new(window, cx)
                        .kind(Kind::Number {
                            min: num(field.min),
                            max: num(field.max),
                            step: num(field.step),
                        })
                        .style(if field.unit.is_some() {
                            field_unit
                        } else {
                            num_field
                        })
                        .align_right()
                        .with_text(start)
                });
                let name = key.clone();
                let sub = cx.subscribe(&input, move |this, input, event: &FieldEvent, cx| {
                    if *event != FieldEvent::Change {
                        return;
                    }
                    let text = input.read(cx).value();
                    let (next, shown) = match name.strip_prefix("crop-") {
                        Some(side) => this.editor.set_crop(side, &text),
                        None => this.editor.set_number(&name, &text),
                    };
                    input.update(cx, |input, cx| {
                        if input.text() != shown {
                            input.set_text(shown, cx);
                        }
                    });
                    this.emit_input(next, cx);
                });
                self.subs.push(sub);
                self.numbers.insert(key, input.clone());
                input
            }
        };
        let focused = input.read(cx).focus_handle(cx).is_focused(window);
        if !focused && input.read(cx).text() != text {
            input.update(cx, |input, cx| input.set_text(text, cx));
        }
        input
    }

    /// Ползунок `.gx-slider` поля; двойной щелчок — стандартное значение.
    fn range(&self, field: NumberField, value: f64, cx: &mut Context<Self>) -> AnyElement {
        let key = field.key;
        let default = LivesplitEditor::defaults().to_value()[key]
            .as_f64()
            .unwrap_or(field.min);
        let set = move |this: &mut Self, raw: &f64, cx: &mut Context<Self>| {
            this.set(key, js_number(*raw), cx)
        };
        slider(format!("lso-range-{key}"), value, field.min, field.max)
            .step(field.step)
            .default_value(default)
            .label(self.t(field.label, &[]))
            .on_input(cx.listener(move |this, raw: &f64, _, cx| set(this, raw, cx)))
            .on_change(cx.listener(move |this, raw: &f64, _, cx| set(this, raw, cx)))
            .into_any_element()
    }

    /// Строка числа с ползунком (`SIZE_FIELDS`, `COLOR_FIELDS`, `LUMA_FIELDS`).
    fn number_row(
        &mut self,
        field: NumberField,
        overlay: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = field.key;
        let value = overlay[key].as_f64().unwrap_or(field.min);
        let input = self.number_input(
            key.to_owned(),
            field,
            js::to_string(&overlay[key]),
            window,
            cx,
        );
        let mut step = num_step(format!("lso-num-{key}"), input);
        if let Some(unit) = field.unit {
            step = step.unit(unit);
        }
        let mut row = gx::row(format!("lso-row-{key}"), self.t(field.label, &[]))
            .changed(self.editor.changed(key))
            .control(div().flex_shrink(1.).min_w(u(64.)).w(u(92.)).child(step));
        if let Some(hint) = field.hint {
            row = row.hint(self.t(hint, &[]));
        }
        row.below(div().mt(u(2.)).child(self.range(field, value, cx)))
            .into_any_element()
    }

    /// Карточка источника `.lso-source`.
    fn source(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let tone = self.editor.source_tone();
        let (hint_key, hint_arg) = self.editor.source_hint();
        let hint = match hint_arg {
            Some(arg) => self.t(hint_key, &[&arg]),
            None => self.t(hint_key, &[]),
        };
        let warn_border: gpui::Hsla = rgba(0xffb84d59).into();
        let (border, icon_color) = match tone {
            Tone::Ok => (theme.color("l-border"), theme.color("accent")),
            Tone::Warn => (warn_border, rgb(0xffb84d).into()),
            Tone::Idle => (theme.color("l-border"), theme.color("l-text-faint")),
        };
        let accent = theme.color("accent");
        let capturing = self.editor.capturing;
        let mut refresh = button("lso-refresh", BtnLook::Gx);
        if !capturing {
            // Пока идёт снимок, кнопка не бледнеет: крутящийся значок и есть ответ.
            refresh = refresh.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.refresh(cx)));
        }
        let refresh = refresh
            // `.lso-source .gx-btn i { margin-right: 6px }`; `fa-spin` — кольцо вместо глифа.
            .child(div().mr(u(6.)).child(if capturing {
                icon("fas fa-sync-alt fa-spin", 11., theme.color("l-text-dim"))
            } else {
                fa_glyph("fas fa-sync-alt", 11.).into_any_element()
            }))
            .child(self.t("#L_LSO_REFRESH", &[]));
        let title = self.t(self.editor.source_title(), &[]);
        let (text, faint, bg) = (
            theme.color("l-text"),
            theme.color("l-text-faint"),
            white(0.03),
        );
        let icon_class = format!("fas {}", self.editor.source_icon());
        let card = move |mix_t: f32| {
            div()
                .flex()
                .items_start()
                .gap(u(12.))
                .mx(u(14.))
                .mt(u(14.))
                .mb(u(8.))
                .px(u(16.))
                .py(u(14.))
                .border_1()
                .border_color(mix(border, accent.opacity(0.7), mix_t))
                .rounded(theme.rem("l-radius-sm"))
                .bg(mix(bg, accent.opacity(0.12), mix_t))
                .text_px(12., 12. * 1.4)
                .child(
                    div()
                        .flex_none()
                        .w(u(16.))
                        .pt(u(2.))
                        .flex()
                        .justify_center()
                        .text_color(icon_color)
                        .child(fa_glyph(&icon_class, 14.)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(u(10.))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(u(4.))
                                .text_color(faint)
                                .child(
                                    div()
                                        .text_color(text)
                                        .text_px(13., 13. * 1.4)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(crate::text::pretty(title.clone())),
                                )
                                // `.lso-source__text { text-wrap: pretty }`.
                                .child(crate::text::pretty(hint.clone())),
                        )
                        .child(
                            div()
                                .flex()
                                .child(crate::snap::target("lso-refresh", refresh)),
                        ),
                )
        };
        if self.flash == 0 {
            return card(0.).into_any_element();
        }
        keyframes(
            ElementId::Name(format!("lso-flash-{}", self.flash).into()),
            "lso-flash",
            move |frame| card(frame.mix as f32),
        )
        .into_any_element()
    }

    /// Якоря `.lso-anchors`: углы, края, центр.
    fn anchors(&self, overlay: &Value, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let (x, y) = (overlay["x"].as_f64(), overlay["y"].as_f64());
        let mut rows = Vec::new();
        for row in 0..3 {
            let mut cells = Vec::new();
            for col in 0..3 {
                let at = (f64::from(col) * 50., f64::from(row) * 50.);
                let Some(anchor) = ANCHORS.iter().find(|a| (a.x, a.y) == at).copied() else {
                    continue;
                };
                let active = x == Some(anchor.x) && y == Some(anchor.y);
                let label = self.t(&anchor.label_key(), &[]);
                let id = ElementId::Name(format!("lso-anchor-{}", anchor.id).into());
                let focus = focus_handle(&id, window, cx);
                let ring = focus_visible(&focus, window);
                let (soft, accent, faint, focus_color) = (
                    theme.color("accent-soft"),
                    theme.color("accent"),
                    theme.color("l-text-faint"),
                    theme.color("accent-focus"),
                );
                let click = cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let mut patch = Map::new();
                    patch.insert("x".into(), js_number(anchor.x));
                    patch.insert("y".into(), js_number(anchor.y));
                    let next = this.editor.set(patch);
                    this.emit_input(next, cx);
                });
                let inner = id.clone();
                let meta = crate::live::Meta::new("radio", id.clone())
                    .label(Some(label.clone().into()))
                    .checked(active)
                    .focused(focus.is_focused(window));
                let cell = hover(id, Timing::fast(), move |t| {
                    div()
                        .id(inner)
                        .role(Role::Button)
                        .button_text()
                        .aria_label(label)
                        .aria_toggled(if active {
                            Toggled::True
                        } else {
                            Toggled::False
                        })
                        .track_focus(&focus)
                        .relative()
                        .w(u(22.))
                        .h(u(16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(u(3.))
                        .bg(if active {
                            soft
                        } else {
                            mix(white(0.06), white(0.12), t)
                        })
                        .cursor_pointer()
                        .on_click(click)
                        .child(
                            div()
                                .size(u(if active { 7.8 } else { 6. }))
                                .rounded_full()
                                .bg(if active { accent } else { faint }),
                        )
                        .when(ring, |el| el.child(outline(zoom, 3., 1., 2., focus_color)))
                });
                cells.push(crate::live::mark(meta, cell));
            }
            rows.push(div().flex().gap(u(3.)).children(cells));
        }
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(u(3.))
            .p(u(4.))
            .border_1()
            .border_color(theme.color("l-border"))
            .rounded(u(7.))
            .bg(hsla(0., 0., 0., 0.25))
            .children(rows)
    }

    /// Маска `.lso-seg`: нет / цвет / яркость.
    fn keys(&self, overlay: &Value, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let current = overlay["key"].as_str().unwrap_or("none").to_owned();
        let items: Vec<AnyElement> = KEYS
            .iter()
            .map(|&key| {
                let active = current == key;
                let label = self.t(&format!("#L_LSO_KEY_{}", key.to_uppercase()), &[]);
                let id = ElementId::Name(format!("lso-key-{key}").into());
                let focus = focus_handle(&id, window, cx);
                let ring = focus_visible(&focus, window);
                let (soft, text, dim, focus_color) = (
                    theme.color("accent-soft"),
                    theme.color("l-text"),
                    theme.color("l-text-dim"),
                    theme.color("accent-focus"),
                );
                let click =
                    cx.listener(move |this, _: &ClickEvent, _, cx| this.set("key", json!(key), cx));
                let inner = id.clone();
                let item = hover(id, Timing::fast(), move |t| {
                    div()
                        .id(inner)
                        .role(Role::RadioButton)
                        .button_text()
                        .aria_toggled(if active {
                            Toggled::True
                        } else {
                            Toggled::False
                        })
                        .track_focus(&focus)
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .px(u(8.))
                        .py(u(6.))
                        .rounded(u(6.))
                        .flex()
                        .justify_center()
                        .text_px(12.5, 20.)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if active { text } else { mix(dim, text, t) })
                        .bg(if active { soft } else { white(0.05 * t) })
                        .cursor_pointer()
                        .on_click(click)
                        .child(label)
                        .when(ring, |el| el.child(outline(zoom, 6., 1., 2., focus_color)))
                });
                crate::snap::target(format!("lso-key-{key}"), item)
            })
            .collect();
        div()
            .flex()
            .gap(u(2.))
            .p(u(3.))
            .border_1()
            .border_color(theme.color("l-border"))
            .rounded(u(9.))
            .bg(hsla(0., 0., 0., 0.2))
            .children(items)
    }

    /// Обрезка `.lso-crop`: четыре стороны сеткой 2 × 2.
    fn crop(&mut self, overlay: &Value, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let dim = cx.global::<Theme>().color("l-text-dim");
        let field = crop_field();
        let mut cells = Vec::new();
        for side in CROP_SIDES {
            let text = js::to_string(&overlay["crop"][side]);
            let input = self.number_input(format!("crop-{side}"), field, text, window, cx);
            cells.push(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(u(8.))
                    .text_color(dim)
                    .text_px(12.5, 20.)
                    .child(self.t(&format!("#L_LSO_CROP_{}", side.to_uppercase()), &[]))
                    .child(
                        div()
                            .flex_none()
                            .w(u(92.))
                            .child(num_step(format!("lso-crop-{side}"), input).unit("px")),
                    ),
            );
        }
        let mut cells = cells.into_iter();
        let mut grid = div().flex().flex_col().gap(u(6.));
        while let (Some(a), Some(b)) = (cells.next(), cells.next()) {
            grid = grid.child(div().flex().gap(u(14.)).child(a).child(b));
        }
        grid
    }

    /// Цвет ключа: поле цвета и HEX, ниже — «Фон LiveSplit».
    fn key_color_row(
        &mut self,
        overlay: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let color = &overlay["keyColor"];
        let channel = |key: &str| color[key].as_u64().unwrap_or(0) as u8;
        let hex = LivesplitEditor::hex(channel("r"), channel("g"), channel("b"));
        let label = self.t("#L_LSO_KEY_COLOR", &[]);
        let picker = match &self.color {
            Some(picker) => picker.clone(),
            None => {
                let start = hex.clone();
                let name = label.clone();
                let picker =
                    cx.new(|cx| ColorField::new(window, cx).with_hex(&start, cx).label(name));
                let sub = cx.subscribe(&picker, |this, _, ColorEvent::Input(hex), cx| {
                    if let Some(next) = this.editor.set_color(hex) {
                        this.emit_input(next, cx);
                    }
                });
                self.subs.push(sub);
                self.color = Some(picker.clone());
                picker
            }
        };
        if picker.read(cx).hex() != hex {
            picker.update(cx, |picker, cx| picker.set_hex(&hex, cx));
        }
        let background = self.editor.background_color();
        let mut pick = button("lso-pick-background", BtnLook::Gx)
            .disabled(background.is_none())
            .child(fa_glyph("fas fa-eye-dropper", 11.).mr(u(6.)))
            .child(self.t("#L_LSO_PICK_BACKGROUND", &[]));
        if let Some(rgb) = background {
            pick = pick.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.set("keyColor", json!({"r": rgb.r, "g": rgb.g, "b": rgb.b}), cx)
            }));
        }
        gx::row("lso-row-keyColor", label)
            .changed(self.editor.changed("keyColor"))
            .control(
                div()
                    .flex()
                    .items_center()
                    .gap(u(10.))
                    .child(picker)
                    .child(gx::code(hex.to_uppercase(), &theme)),
            )
            .below(div().flex().justify_end().child(pick))
            .into_any_element()
    }
}

impl Render for LiveSplitOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.shown_at = Instant::now();
        if std::mem::take(&mut self.idle) {
            // Снова на экране (`watch active`): снимок сразу и таймер.
            self.capture(false, false, None, cx);
            self.schedule(cx);
        }
        let theme = cx.global::<Theme>().clone();
        let overlay_value = self.editor.overlay().to_value();
        let overlay = &overlay_value;
        let changes = self.editor.changes();
        let reset_label = if changes > 0 {
            format!(
                "{} — {}",
                self.t("#L_GS_X_RESET_SECTION", &[]),
                self.t("#L_GS_X_CHANGED", &[&changes.to_string()])
            )
        } else {
            self.t("#L_GS_X_RESET_SECTION", &[])
        };
        let reset = button("lso-reset", BtnLook::Gx)
            .label(reset_label)
            .disabled(changes == 0)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                let next = this.editor.reset();
                this.emit_input(next, cx);
            }))
            .child(fa_glyph("fas fa-undo-alt", 11.))
            .when(changes > 0, |el| el.child(gx::count(changes, &theme)));
        let head = gx::pane_head(self.t("#L_LSO_TITLE", &[]))
            .reset(reset)
            .note(self.t("#L_LSO_NOTE", &[]));

        let enabled = overlay["enabled"] == Value::Bool(true);
        let enabled_label = self.t("#L_LSO_ENABLED", &[]);
        let enabled_row = gx::row("lso-row-enabled", enabled_label.clone())
            .changed(enabled)
            .control(crate::snap::target(
                "lso-enabled",
                toggle("lso-enabled", enabled)
                    .label(enabled_label)
                    .on_change(
                        cx.listener(|this, on: &bool, _, cx| this.set("enabled", json!(*on), cx)),
                    ),
            ))
            .on_label(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.set("enabled", json!(!enabled), cx)
            }))
            .hint(self.t("#L_LSO_ENABLED_HINT", &[]));

        let axes: Vec<AnyElement> = ["x", "y"]
            .into_iter()
            .map(|axis| {
                let (start, end, label) = if axis == "x" {
                    ("#L_LSO_LEFT", "#L_LSO_RIGHT", "#L_LSO_X")
                } else {
                    ("#L_LSO_TOP", "#L_LSO_BOTTOM", "#L_LSO_Y")
                };
                let field = NumberField {
                    key: axis,
                    label,
                    hint: None,
                    min: 0.,
                    max: 100.,
                    step: 0.5,
                    unit: None,
                };
                let value = overlay[axis].as_f64().unwrap_or(0.);
                gx::slider_ends(
                    self.t(start, &[]),
                    self.range(field, value, cx),
                    self.t(end, &[]),
                    &theme,
                )
                .into_any_element()
            })
            .collect();
        let mut position = gx::row("lso-row-position", self.t("#L_LSO_POSITION", &[]))
            .changed(self.editor.changed("x") || self.editor.changed("y"))
            .control(self.anchors(overlay, window, cx))
            .hint(self.t("#L_LSO_POSITION_HINT", &[]));
        for axis in axes {
            position = position.below(axis);
        }

        let mut rows: Vec<AnyElement> = vec![
            self.source(cx),
            enabled_row.into_any_element(),
            position.into_any_element(),
        ];
        for field in SIZE_FIELDS {
            rows.push(self.number_row(field, overlay, window, cx));
        }
        let crop = self.crop(overlay, window, cx);
        rows.push(
            gx::row("lso-row-crop", self.t("#L_LSO_CROP", &[]))
                .changed(self.editor.changed("crop"))
                .hint(self.t("#L_LSO_CROP_HINT", &[]))
                .below(crop)
                .into_any_element(),
        );
        let key = overlay["key"].as_str().unwrap_or("none").to_owned();
        let keys = self.keys(overlay, window, cx);
        rows.push(
            gx::row("lso-row-key", self.t("#L_LSO_KEY", &[]))
                .changed(key != "none")
                .hint(self.t("#L_LSO_KEY_HINT", &[]))
                .below(keys)
                .into_any_element(),
        );
        if key == "color" {
            rows.push(self.key_color_row(overlay, window, cx));
            for field in COLOR_FIELDS {
                rows.push(self.number_row(field, overlay, window, cx));
            }
        }
        if key == "luma" {
            for field in LUMA_FIELDS {
                rows.push(self.number_row(field, overlay, window, cx));
            }
        }

        gx::pane(&theme)
            .bg(gpui::transparent_black())
            .size_full()
            .child(head)
            .child(crate::snap::target(
                "lso-fields",
                gx::fields("lso-fields", &self.scroll).child(div().children(rows)),
            ))
    }
}
