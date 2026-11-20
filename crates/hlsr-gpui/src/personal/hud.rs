//! «Цвет и прицел» (`GhostingPersonalization.vue` 0.4.9, без изменений с 0.4.7): вкладки «Цвет
//! HUD» / «Прицел»; у HUD — выключатель, палитра со своим цветом, HSV-круг 108 px с курсором,
//! HEX и R/G/B, прозрачность; у прицела — выключатель, пресеты, «прятать дуги» (Ghosting) и
//! «цвет как у HUD», цвет, размер, зазор, толщина, прозрачность и «тонкая настройка» (точка,
//! круг, цвет точки, обводка, стандартный прицел, линии). Логика — `HudEditor`.

use super::editor::GamePersonalization;
use crate::icons::{chevron, fa_glyph};
use crate::image::device_size;
use crate::kit::button::{focus_visible, outline, white};
use crate::kit::color::{cursor_fill, pers_field, pers_input, pers_mono, wheel_image};
use crate::kit::{CheckLook, Checked, ColorEvent, ColorField, FieldEvent, Kind, TextInput, check};
use crate::kit::{slider, toggle};
use crate::motion::{Timing, hover, mix, tween};
use crate::paint::shadow;
use crate::scale::{u, zoom};
use crate::scroll::{Bar, ScrollArea};
use crate::text::{MONO, TextExt, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext, ClickEvent, Context, CursorStyle, DispatchPhase, Div, ElementId,
    Focusable, FontWeight, Hsla, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Role, SharedString, Styled, Window, canvas, div,
    img, prelude::*, relative, rgb,
};
use hlsr_core::json_text::js_number;
use hlsr_ui::controllers::hud_editor::{CROSS_PRESET_IDS, HudEditor, Rgb, hex_to_rgb, swatches};
use hlsr_ui::render::hsv;
use serde_json::{Value, json};

/// Сторона круга, CSS px (`.pers__wheel`).
const WHEEL: f32 = hsv::BOX_PX;
const CHANNELS: [&str; 3] = ["r", "g", "b"];

fn css(color: Rgb) -> Hsla {
    let channel = |v: f64| v.clamp(0., 255.).round() as u32;
    rgb((channel(color.r) << 16) | (channel(color.g) << 8) | channel(color.b)).into()
}

impl GamePersonalization {
    /// Новое значение от редактора HUD.
    fn hud_emit(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut HudEditor) -> Option<Value>,
    ) {
        let Some(look) = self.look.as_mut() else {
            return;
        };
        if let Some(next) = f(&mut look.hud) {
            self.input(next, cx);
        } else {
            cx.notify();
        }
    }

    fn hud_set(&mut self, key: &'static str, value: Value, cx: &mut Context<Self>) {
        self.hud_emit(cx, |hud| Some(hud.set(key, value)));
    }

    /// Поля HEX, R/G/B и свои цвета (палитра HUD, прицела, точка) — один раз, в кадре.
    fn hud_widgets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.widgets.hud_hex.is_some() {
            return;
        }
        let hex = cx.new(|cx| TextInput::new(window, cx).style(pers_mono).max_length(7));
        self.subs
            .push(cx.subscribe(&hex, |this, input, event: &FieldEvent, cx| {
                if *event == FieldEvent::Change {
                    let text = input.read(cx).text().to_owned();
                    this.hud_emit(cx, |hud| hud.on_hud_hex(&text));
                }
            }));
        let channel = |window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                TextInput::new(window, cx)
                    .kind(Kind::Number {
                        min: "0".into(),
                        max: "255".into(),
                        step: SharedString::default(),
                    })
                    .style(pers_input)
            })
        };
        let channels = [
            channel(window, cx),
            channel(window, cx),
            channel(window, cx),
        ];
        for (index, input) in channels.iter().enumerate() {
            self.subs.push(
                cx.subscribe(input, move |this, input, event: &FieldEvent, cx| {
                    if *event == FieldEvent::Change {
                        let text = input.read(cx).value();
                        this.hud_emit(cx, |hud| Some(hud.on_hud_channel(CHANNELS[index], &text)));
                    }
                }),
            );
        }
        let pick = self.t("#L_GS_P_PICK", &[]);
        let dot = self.t("#L_GS_P_DOT_COLOR", &[]);
        let pickers = [
            cx.new(|cx| ColorField::new(window, cx).plus().label(pick.clone())),
            cx.new(|cx| ColorField::new(window, cx).plus().label(pick)),
            cx.new(|cx| ColorField::new(window, cx).label(dot)),
        ];
        let [hud_plus, cross_plus, dot_picker] = pickers.clone();
        self.subs.push(
            cx.subscribe(&hud_plus, |this, _, ColorEvent::Input(hex), cx| {
                let hex = hex.to_string();
                this.hud_emit(cx, |hud| hud.on_hud_hex(&hex));
            }),
        );
        self.subs.push(
            cx.subscribe(&cross_plus, |this, _, ColorEvent::Input(hex), cx| {
                let hex = hex.to_string();
                this.hud_emit(cx, |hud| hud.on_cross_hex(&hex));
            }),
        );
        self.subs.push(
            cx.subscribe(&dot_picker, |this, _, ColorEvent::Input(hex), cx| {
                let color = hex_to_rgb(hex).map_or(Value::Null, Rgb::to_value);
                this.hud_set("crossDotColor", color, cx);
            }),
        );
        self.widgets.hud_hex = Some(hex);
        self.widgets.channels = Some(channels);
        self.widgets.pickers = Some(pickers);
    }

    /// Поля за цветом (`:value`), пока в них не пишут.
    fn sync_hud_widgets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(look) = self.look.as_ref() else {
            return;
        };
        let hud = &look.hud;
        let (hud_hex, cross_hex, dot_hex) = (hud.hud_hex(), hud.cross_hex(), hud.dot_hex());
        let color = hud.value().get("hudColor").cloned().unwrap_or(Value::Null);
        let texts: Vec<String> = CHANNELS
            .iter()
            .map(|ch| {
                color
                    .get(*ch)
                    .map_or_else(String::new, hlsr_core::js::to_string)
            })
            .collect();
        let (Some(hex), Some(channels), Some(pickers)) = (
            self.widgets.hud_hex.clone(),
            self.widgets.channels.clone(),
            self.widgets.pickers.clone(),
        ) else {
            return;
        };
        // `.pers__input--mono { text-transform: uppercase }`.
        let fields =
            std::iter::once((hex, hud_hex.to_uppercase())).chain(channels.into_iter().zip(texts));
        for (input, text) in fields {
            if !input.read(cx).focus_handle(cx).is_focused(window) && input.read(cx).text() != text
            {
                input.update(cx, |input, cx| input.set_text(text, cx));
            }
        }
        for (picker, hex) in pickers.iter().zip([hud_hex, cross_hex, dot_hex]) {
            if picker.read(cx).hex() != hex {
                picker.update(cx, |picker, cx| picker.set_hex(&hex, cx));
            }
        }
    }

    /// Точка в коробке круга (CSS px от её угла).
    fn wheel_point(&self, at: gpui::Point<gpui::Pixels>, window: &Window) -> (f64, f64) {
        let zoom = zoom(window);
        let origin = self.widgets.wheel_box.get().origin;
        let css = |v: gpui::Pixels| f64::from(f32::from(v) / zoom);
        (css(at.x - origin.x), css(at.y - origin.y))
    }

    /// HSV-круг с курсором: нажатие, перетаскивание (мышь окна до отпускания), стрелки.
    fn wheel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>();
        let zoom = zoom(window);
        let focus_color = theme.color("accent-focus");
        let Some(look) = self.look.as_ref() else {
            return div().into_any_element();
        };
        let (fx, fy) = look.hud.wheel_cursor();
        let under = cursor_fill(look.hud.wheel.h, look.hud.wheel.s);
        let dragging = look.hud.wheel_dragging();
        let hue = look.hud.wheel.h.round();
        let label = self.t("#L_GS_P_WHEEL", &[]);
        let focus = window
            .use_keyed_state("pers-wheel-focus", cx, |_, cx| {
                cx.focus_handle().tab_stop(true)
            })
            .read(cx)
            .clone();
        let ring_on = focus_visible(&focus, window);
        let side = device_size(window, WHEEL, WHEEL).0;
        let view = cx.entity().downgrade();
        let probe = self.widgets.wheel_box.clone();
        let watch = canvas(
            move |bounds, _, _| probe.set(bounds),
            move |_, (), window, _| {
                if !dragging {
                    return;
                }
                window.set_window_cursor_style(CursorStyle::Crosshair);
                let (moved, up) = (view.clone(), view);
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    moved
                        .update(cx, |this, cx| {
                            if event.pressed_button != Some(MouseButton::Left) {
                                this.hud_emit(cx, |hud| {
                                    hud.on_wheel_up();
                                    None
                                });
                                return;
                            }
                            let (x, y) = this.wheel_point(event.position, window);
                            let w = f64::from(WHEEL);
                            this.hud_emit(cx, |hud| hud.on_wheel_move(x, y, w, w));
                        })
                        .ok();
                });
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        up.update(cx, |this, cx| {
                            this.hud_emit(cx, |hud| {
                                hud.on_wheel_up();
                                None
                            })
                        })
                        .ok();
                    }
                });
            },
        )
        .absolute()
        .size_full();
        div()
            .id("pers-wheel")
            .role(Role::Slider)
            .aria_label(label)
            .aria_numeric_value(hue)
            .aria_min_numeric_value(0.)
            .aria_max_numeric_value(359.)
            .track_focus(&focus)
            .relative()
            .flex_none()
            .size(u(WHEEL))
            .cursor(CursorStyle::Crosshair)
            .child(img(wheel_image(side)).size_full().rounded_full())
            .child(
                div()
                    .absolute()
                    .left(relative(fx as f32))
                    .top(relative(fy as f32))
                    .ml(u(-6.))
                    .mt(u(-6.))
                    .size(u(12.))
                    .rounded_full()
                    .border_2()
                    .border_color(white(1.))
                    .bg(under)
                    .shadow(vec![shadow(zoom, 0., 1., 7., 0., gpui::black())]),
            )
            .child(watch)
            .when(ring_on, |el| {
                el.child(outline(zoom, WHEEL / 2., 3., 2., focus_color))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    let (x, y) = this.wheel_point(event.position, window);
                    let w = f64::from(WHEEL);
                    this.hud_emit(cx, |hud| Some(hud.on_wheel_down(x, y, w, w)));
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let key = match event.keystroke.key.as_str() {
                    "left" => "ArrowLeft",
                    "right" => "ArrowRight",
                    "up" => "ArrowUp",
                    "down" => "ArrowDown",
                    _ => return,
                };
                cx.stop_propagation();
                this.hud_emit(cx, |hud| hud.on_wheel_key(key));
            }))
            .into_any_element()
    }

    /// Ползунок `.pers__slider`: подпись и значение, `<input type="range">` (шаг 1).
    #[allow(clippy::too_many_arguments)]
    fn pers_slider(
        &self,
        key: &'static str,
        label: &str,
        shown: String,
        min: f64,
        max: f64,
        tight: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.global::<Theme>();
        let value = self
            .look
            .as_ref()
            .map_or(min, |look| look.hud.number(key))
            .clamp(min, max);
        let label = self.t(label, &[]);
        div()
            .flex()
            .flex_col()
            .gap(u(6.))
            .mt(u(if tight { 10. } else { 14. }))
            // Строки — 20 px `body`: своей у `.pers__slider` нет.
            .text_px(13., 20.)
            .text_color(theme.color("l-text-dim"))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .gap(u(8.))
                    .child(
                        div()
                            .text_color(theme.color("l-text"))
                            .font_weight(FontWeight::MEDIUM)
                            .child(label.clone()),
                    )
                    .child(
                        div()
                            .text_color(theme.color("accent"))
                            .text_px(12., 20.)
                            .font_weight(FontWeight::SEMIBOLD)
                            .tnum()
                            .child(shown),
                    ),
            )
            .child(
                // Нативный `<input type="range">` 18 px (`accent-color`), не `.gx-range`.
                slider(
                    ElementId::Name(format!("pers-{key}").into()),
                    value,
                    min,
                    max,
                )
                .native()
                .step(1.)
                .label(label)
                .on_input(
                    cx.listener(move |this, v: &f64, _, cx| {
                        this.hud_set(key, json!(*v as i64), cx)
                    }),
                ),
            )
    }

    /// Две колонки (`.pers__slider-grid`, `.pers__toggles`).
    fn pair(a: impl IntoElement, b: impl IntoElement, gap: f32) -> Div {
        div()
            .flex()
            .gap(u(gap))
            .child(div().flex_1().min_w_0().child(a))
            .child(div().flex_1().min_w_0().child(b))
    }

    /// Флажок с подписью (`.pers__check`).
    fn pers_check(&self, key: &'static str, label: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>();
        let on = self.look.as_ref().is_some_and(|look| look.hud.bool(key));
        check(
            ElementId::Name(format!("pers-check-{key}").into()),
            if on { Checked::On } else { Checked::Off },
            CheckLook::Native,
        )
        .gap(u(8.))
        .text_px(13., 13. * 1.2)
        .text_color(theme.color("l-text-dim"))
        .on_change(cx.listener(move |this, on: &bool, _, cx| this.hud_set(key, json!(*on), cx)))
        .child(self.t(label, &[]))
        .into_any_element()
    }

    /// Строка-флажок `.pers__option`: подпись с пояснением слева, флажок 18 px справа.
    fn pers_option(
        &self,
        key: &'static str,
        title: &str,
        hint: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>();
        let on = self.look.as_ref().is_some_and(|look| look.hud.bool(key));
        check(
            ElementId::Name(format!("pers-option-{key}").into()),
            if on { Checked::On } else { Checked::Off },
            CheckLook::Native,
        )
        .side(18.)
        .flex_row_reverse()
        .justify_between()
        .gap(u(12.))
        .py(u(11.))
        .px(u(12.))
        .border_1()
        .border_color(theme.color("l-border"))
        .rounded(u(10.))
        .bg(white(0.018))
        .on_change(cx.listener(move |this, on: &bool, _, cx| this.hud_set(key, json!(*on), cx)))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(u(3.))
                .child(
                    // `b` без своей строки — 20 px `body`.
                    div()
                        .text_px(13., 20.)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.color("l-text"))
                        .child(self.t(title, &[])),
                )
                .child(
                    div()
                        .text_px(12., 12. * 1.35)
                        .text_color(theme.color("l-text-faint"))
                        .child(self.t(hint, &[])),
                ),
        )
        .into_any_element()
    }

    /// Шапка панели `.pers__feature-head`: заголовок, пояснение и выключатель.
    fn feature_head(
        &self,
        title: &str,
        hint: &str,
        on: bool,
        on_change: impl Fn(&bool, &mut Window, &mut gpui::App) + 'static,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.global::<Theme>();
        let title = self.t(title, &[]);
        div()
            .flex()
            .items_start()
            .justify_between()
            .gap(u(12.))
            .pb(u(14.))
            .border_b_1()
            .border_color(theme.color("l-border"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(u(3.))
                    .min_w_0()
                    .child(
                        div()
                            .text_px(15., 15. * 1.3)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title.clone()),
                    )
                    .child(
                        div()
                            .text_px(12., 12. * 1.4)
                            .text_color(theme.color("l-text-faint"))
                            .child(crate::text::pretty(self.t(hint, &[]))),
                    ),
            )
            .child(
                toggle(SharedString::from(format!("pers-{title}")), on)
                    .label(title)
                    .on_change(on_change),
            )
    }

    /// Подпись группы `.pers__label`: 11,5 px / 600, разрядка .08em, заглавными; строка 20 px
    /// `body`.
    fn group_label(&self, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>();
        div()
            .mb(u(8.))
            .child(spaced(
                self.t(key, &[]).to_uppercase(),
                11.5,
                20.,
                FontWeight::SEMIBOLD,
                0.08,
                theme.color("l-text-faint"),
            ))
            .into_any_element()
    }

    /// Образец палитры `.pers__swatch` 28 × 28.
    fn swatch(
        name: String,
        color: Hsla,
        active: bool,
        accent: Hsla,
        zoom: f32,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> AnyElement {
        let id = ElementId::Name(name.clone().into());
        let inner = id.clone();
        let swatch = hover(id, Timing::fast(), move |t| {
            let border = if active {
                white(1.)
            } else {
                mix(white(0.14), white(0.45), t)
            };
            div()
                .id(inner.clone())
                .flex_none()
                .size(u(28.))
                .relative()
                .top(u(-t))
                .rounded(u(8.))
                .border_2()
                .border_color(border)
                .bg(color)
                .when(active, |el| {
                    el.shadow(vec![shadow(zoom, 0., 0., 0., 2., accent)])
                })
                .cursor_pointer()
                .on_click(on_click)
        });
        crate::snap::target(name, swatch)
    }

    /// Панель «Цвет и прицел» (`.pers`), ширина внутри полей — `inner` (пресеты сеткой).
    pub(super) fn render_hud(
        &mut self,
        inner: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.hud_widgets(window, cx);
        self.sync_hud_widgets(window, cx);
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let tight = crate::kit::gx::compact(window);
        let gap = if tight { 10. } else { 14. };
        let Some(look) = self.look.as_ref() else {
            return div().into_any_element();
        };
        let hud = look.hud.clone();
        let panel = hud.active_panel;
        let goldsrc = hud.goldsrc;
        let accent = theme.color("accent");
        let border = theme.color("l-border");
        let (Some(hex_input), Some([r, g, b]), Some([hud_plus, cross_plus, dot_picker])) = (
            self.widgets.hud_hex.clone(),
            self.widgets.channels.clone(),
            self.widgets.pickers.clone(),
        ) else {
            return div().into_any_element();
        };

        // ---- вкладки ----
        let tab = |id: &'static str, icon: &str, label: String, first: bool| {
            let active = panel == id;
            let (dim, text, soft) = (
                theme.color("l-text-dim"),
                theme.color("l-text"),
                theme.color("accent-soft"),
            );
            let icon = icon.to_owned();
            let select = cx.listener(move |this: &mut Self, _: &ClickEvent, _, cx| {
                if let Some(look) = this.look.as_mut() {
                    look.hud.active_panel = id;
                    look.extras.personalization_panel = id.to_owned();
                }
                cx.notify();
            });
            let tab = hover(
                ElementId::Name(format!("pers-tab-{id}").into()),
                Timing::fast(),
                move |t| {
                    div()
                        .id(ElementId::Name(format!("pers-tab-{id}-btn").into()))
                        .role(Role::Tab)
                        .button_text()
                        .aria_selected(active)
                        .relative()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(u(8.))
                        .text_px(13., 13. * 1.2)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if active { text } else { mix(dim, text, t) })
                        .bg(if active { soft } else { white(0.04 * t) })
                        .when(!first, |el| el.border_l_1().border_color(border))
                        .cursor_pointer()
                        .on_click(select)
                        .child(fa_glyph(&icon, 12.).text_color(accent))
                        .child(label.clone())
                        .when(active, |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .left(u(16.))
                                    .right(u(16.))
                                    .bottom(u(-1.))
                                    .h(u(2.))
                                    .bg(accent),
                            )
                        })
                },
            );
            crate::snap::target(format!("pers-tab-{id}"), tab)
        };
        let tabs = div()
            .flex_none()
            .flex()
            .min_h(u(44.))
            .border_b_1()
            .border_color(border)
            .child(tab("hud", "fas fa-tint", self.t("#L_GS_P_HUD", &[]), true))
            .child(tab(
                "cross",
                "fas fa-crosshairs",
                self.t("#L_GS_P_CROSS", &[]),
                false,
            ));

        let body = if panel == "hud" {
            // ---- цвет HUD ----
            let head = self.feature_head(
                "#L_GS_P_HUD_TITLE",
                if goldsrc {
                    "#L_GS_P_HUD_HINT_GOLDSRC"
                } else {
                    "#L_GS_P_HUD_HINT"
                },
                hud.hud_enabled(),
                cx.listener(|this, on: &bool, _, cx| this.hud_set("hudEnabled", json!(*on), cx)),
                cx,
            );
            let swatches_row = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(u(6.))
                .children(swatches().into_iter().map(|p| {
                    let active = hud.is_hud_preset(&p);
                    let color = css(p.color);
                    let id = format!("pers-hud-{}", p.id);
                    let pick = p.clone();
                    Self::swatch(
                        id,
                        color,
                        active,
                        accent,
                        zoom,
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let pick = pick.clone();
                            this.hud_emit(cx, |hud| Some(hud.set_hud_preset(&pick)))
                        }),
                    )
                }))
                .child(hud_plus.clone());
            let faint = theme.color("l-text-faint");
            let field = |name, input| pers_field(name, input, faint);
            let tools = div()
                .flex()
                .items_center()
                .gap(u(14.))
                .mt(u(gap))
                .p(u(12.))
                .border_1()
                .border_color(border)
                .rounded(u(10.))
                .bg(white(0.022))
                .child(self.wheel(window, cx))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(u(8.))
                        .flex_1()
                        .min_w_0()
                        .child(field("HEX", hex_input))
                        .child(
                            div()
                                .flex()
                                .gap(u(6.))
                                .child(field("R", r).flex_1())
                                .child(field("G", g).flex_1())
                                .child(field("B", b).flex_1()),
                        ),
                );
            let alpha = hud
                .hud_alpha_label()
                .unwrap_or_else(|| self.t("#L_GS_P_AUTO", &[]));
            div()
                .child(head)
                .child(
                    div()
                        .mt(u(gap))
                        .child(self.group_label("#L_GS_P_PALETTE", cx))
                        .child(swatches_row),
                )
                .child(tools)
                .child(self.pers_slider("hudAlpha", "#L_GS_P_ALPHA", alpha, -1., 255., tight, cx))
        } else {
            // ---- прицел ----
            let head = self.feature_head(
                "#L_GS_P_CROSS_TITLE",
                "#L_GS_P_CROSS_HINT",
                hud.cross_enabled(),
                cx.listener(|this, on: &bool, _, cx| {
                    let on = *on;
                    this.hud_emit(cx, |hud| Some(hud.toggle_cross_enabled(on)))
                }),
                cx,
            );
            // `repeat(auto-fit, minmax(96px, 1fr))`; полоса прокрутки панели (App.vue) — 4 px.
            let inner = inner - Bar::Thin.thickness();
            let cols = (((inner + 6.) / 102.).floor() as usize).max(1);

            let presets = div().grid().grid_cols(cols as u16).gap(u(6.)).children(
                CROSS_PRESET_IDS.iter().map(|(id, title)| {
                    let active = hud.cross_preset_active(id);
                    let id = *id;
                    let label = self.t(title, &[]);
                    let apply = cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.hud_emit(cx, |hud| hud.apply_cross_preset(id))
                    });
                    let (dim, text, soft, strong) = (
                        theme.color("l-text-dim"),
                        theme.color("l-text"),
                        theme.color("accent-soft"),
                        theme.color("l-border-strong"),
                    );
                    let accent_border = theme.color("accent-border");
                    let preset = hover(
                        ElementId::Name(format!("pers-preset-{id}").into()),
                        Timing::fast(),
                        move |t| {
                            div()
                                .id(ElementId::Name(format!("pers-preset-{id}-btn").into()))
                                .role(Role::Button)
                                .button_text()
                                .w_full()
                                .min_h(u(34.))
                                .px(u(8.))
                                .py(u(6.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .overflow_hidden()
                                .border_1()
                                .rounded(u(8.))
                                .text_px(12.5, 12.5 * 1.2)
                                .cursor_pointer()
                                .when(active, |el| {
                                    el.border_color(accent_border).bg(soft).text_color(text)
                                })
                                .when(!active, |el| {
                                    el.border_color(mix(border, strong, t))
                                        .bg(white(0.04))
                                        .text_color(mix(dim, text, t))
                                })
                                .on_click(apply)
                                .child(label.clone())
                        },
                    );
                    crate::snap::target(format!("pers-preset-{id}"), preset)
                }),
            );
            let mut options = div().flex().flex_col().gap(u(8.)).mt(u(12.));
            if !goldsrc {
                options = options.child(self.pers_option(
                    "hideQuickInfo",
                    "#L_GS_P_HIDE_QUICKINFO",
                    "#L_GS_P_HIDE_QUICKINFO_HINT",
                    cx,
                ));
            }
            options = options.child(self.pers_option(
                "hudAffectsCrosshair",
                "#L_GS_P_AFFECTS_CROSS",
                "#L_GS_P_AFFECTS_CROSS_HINT",
                cx,
            ));
            let mut panel_body = div()
                .child(head)
                .child(
                    div()
                        .mt(u(gap))
                        .child(self.group_label("#L_GS_P_PRESETS", cx))
                        .child(presets),
                )
                .child(options);
            if hud.cross_enabled() {
                let mut cross = div().mt(u(2.));
                if !hud.hud_affects_crosshair() {
                    let row = div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(u(6.))
                        .children(swatches().into_iter().map(|p| {
                            let active = hud.is_cross_color(&p);
                            let color = css(p.color);
                            let id = format!("pers-cross-{}", p.id);
                            let pick = p.clone();
                            Self::swatch(
                                id,
                                color,
                                active,
                                accent,
                                zoom,
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    let pick = pick.clone();
                                    this.hud_emit(cx, |hud| Some(hud.set_cross_color(&pick)))
                                }),
                            )
                        }))
                        .child(cross_plus.clone())
                        .child(
                            div()
                                .ml_auto()
                                .font_family(MONO)
                                .text_px(12.5, 12.5 * 1.2)
                                .text_color(theme.color("l-text-dim"))
                                .child(hud.cross_hex().to_uppercase()),
                        );
                    cross = cross.child(
                        div()
                            .mt(u(gap))
                            .child(self.group_label("#L_GS_P_CROSS_COLOR", cx))
                            .child(row),
                    );
                }
                let num = |key: &str| js_number(hud.number(key)).to_string();
                cross = cross
                    .child(Self::pair(
                        self.pers_slider(
                            "crossSize",
                            "#L_GS_P_SIZE",
                            num("crossSize"),
                            0.,
                            40.,
                            tight,
                            cx,
                        ),
                        self.pers_slider(
                            "crossGap",
                            "#L_GS_P_GAP",
                            num("crossGap"),
                            if goldsrc { -30. } else { 0. },
                            30.,
                            tight,
                            cx,
                        ),
                        14.,
                    ))
                    .child(Self::pair(
                        self.pers_slider(
                            "crossThickness",
                            "#L_GS_P_THICK",
                            num("crossThickness"),
                            1.,
                            10.,
                            tight,
                            cx,
                        ),
                        self.pers_slider(
                            "crossAlpha",
                            "#L_GS_P_ALPHA_C",
                            num("crossAlpha"),
                            0.,
                            255.,
                            tight,
                            cx,
                        ),
                        14.,
                    ));
                let open = hud.show_advanced;
                let (dim, text, faint) = (
                    theme.color("l-text-dim"),
                    theme.color("l-text"),
                    theme.color("l-text-faint"),
                );
                let advanced_label = self.t("#L_GS_P_ADVANCED", &[]);
                let flip = cx.listener(|this, _: &ClickEvent, _, cx| {
                    if let Some(look) = this.look.as_mut() {
                        look.hud.show_advanced = !look.hud.show_advanced;
                    }
                    cx.notify();
                });
                cross = cross.child(hover("pers-advanced", Timing::fast(), move |t| {
                    div()
                        .id("pers-advanced-btn")
                        .role(Role::Button)
                        .button_text()
                        .aria_expanded(open)
                        .flex()
                        .items_center()
                        .justify_between()
                        .w_full()
                        .mt(u(14.))
                        .py(u(10.))
                        .border_t_1()
                        .border_b_1()
                        .border_color(border)
                        .text_px(13., 13. * 1.2)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(mix(dim, text, t))
                        .cursor_pointer()
                        .on_click(flip)
                        .child(advanced_label.clone())
                        // `i.open { transform: rotate(180deg) }` за `--l-fast`.
                        .child(tween(
                            "pers-advanced-chev",
                            f32::from(u8::from(open)),
                            Timing::fast(),
                            move |t| chevron(10., 180. * t, faint),
                        ))
                }));
                if open {
                    let dot_on = hud.cross_dot_color().is_some();
                    let cross_hex = hud.cross_hex();
                    let dot_check = check(
                        "pers-check-dot",
                        if dot_on { Checked::On } else { Checked::Off },
                        CheckLook::Native,
                    )
                    .gap(u(8.))
                    .text_px(13., 13. * 1.2)
                    .text_color(dim)
                    .on_change(cx.listener(move |this, on: &bool, _, cx| {
                        let color = if *on {
                            hex_to_rgb(&cross_hex).map_or(Value::Null, Rgb::to_value)
                        } else {
                            Value::Null
                        };
                        this.hud_set("crossDotColor", color, cx)
                    }))
                    .child(self.t("#L_GS_P_DOT_COLOR", &[]));
                    let mut advanced = div()
                        .pt(u(1.))
                        .child(Self::pair(
                            self.pers_slider(
                                "crossDotSize",
                                "#L_GS_P_DOT",
                                num("crossDotSize"),
                                0.,
                                12.,
                                tight,
                                cx,
                            ),
                            self.pers_slider(
                                "crossCircleRadius",
                                "#L_GS_P_CIRCLE",
                                num("crossCircleRadius"),
                                0.,
                                40.,
                                tight,
                                cx,
                            ),
                            14.,
                        ))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(u(12.))
                                .mt(u(14.))
                                .child(dot_check)
                                .when(dot_on, |el| el.child(dot_picker.clone())),
                        );
                    if hud.bool("crossOutline") {
                        advanced = advanced.child(self.pers_slider(
                            "crossOutlineThickness",
                            "#L_GS_P_OUTLINE_THICK",
                            num("crossOutlineThickness"),
                            1.,
                            4.,
                            tight,
                            cx,
                        ));
                    }
                    advanced = advanced.child(
                        Self::pair(
                            self.pers_check("crossOutline", "#L_GS_P_OUTLINE", cx),
                            self.pers_check("crossHideDefault", "#L_GS_P_HIDE_DEF", cx),
                            8.,
                        )
                        .mt(u(12.)),
                    );
                    let (soft, accent_border) =
                        (theme.color("accent-soft"), theme.color("accent-border"));
                    let direction = |key: &'static str, arrow: &'static str| {
                        let on = hud.bool(key);
                        div()
                            .id(ElementId::Name(format!("pers-line-{key}").into()))
                            .role(Role::CheckBox)
                            .w(u(32.))
                            .h(u(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .border_1()
                            .rounded(u(6.))
                            .border_color(if on { accent_border } else { border })
                            .when(on, |el| el.bg(soft))
                            .text_color(if on { text } else { dim })
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.hud_set(key, json!(!on), cx)
                            }))
                            .child(arrow)
                    };
                    advanced = advanced.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(u(6.))
                            .mt(u(12.))
                            .text_px(13., 13. * 1.2)
                            .text_color(dim)
                            .child(div().mr_auto().child(self.t("#L_GS_P_LINES", &[])))
                            .child(direction("crossTopLine", "↑"))
                            .child(direction("crossRightLine", "→"))
                            .child(direction("crossBottomLine", "↓"))
                            .child(direction("crossLeftLine", "←")),
                    );
                    cross = cross.child(advanced);
                }
                panel_body = panel_body.child(cross);
            }
            panel_body
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .border_1()
            .border_color(border)
            .rounded(theme.rem("l-radius-sm"))
            .child(tabs)
            .child(
                ScrollArea::new("pers-scroll", &self.widgets.pers_scroll)
                    .contain()
                    .flex_1()
                    .min_h_0()
                    .child(div().p(u(if tight { 12. } else { 14. })).child(body)),
            )
            .into_any_element()
    }
}
