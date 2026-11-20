//! Поле цвета `<input type="color">` 0.4.9 (`.gx-color input`, цвет точки прицела): образец
//! 46 × 30 как у Chromium в тёмной схеме (рамка поля 1 px `--l-border`, скругление 8, фон .05,
//! внутри — образец с рамкой 1 px #777, отступы 4 и 2 px внутри полей 2 px).
//!
//! Нажатие (мышь, пробел, Enter) открывает свой поповер вместо пикера Chromium: HSV-круг
//! 108 px (`render/hsv.rs`, как `.pers__color-tools` редактора HUD), поля HEX и R/G/B. Круг,
//! клавиши круга, разбор HEX и каналов — `hud_editor` 0.4.9 (те же формулы, что у цвета HUD).
//! Поповер — [`popup`] слоя F7: место у поля, `l-pop`, щелчок мимо и прокрутка закрывают, Esc —
//! тоже (фокус возвращается полю). Стекла Liquid Glass нет: пикер Chromium не стеклянный.
//! Изменение — [`ColorEvent::Input`] с `#rrggbb`, как `input`.

use super::button::{focus_visible, outline, white};
use super::input::{FieldEvent, FieldStyle, Kind, TextInput};
use crate::image::{device_size, render_image};
use crate::layer::{Dismiss, Place, PopLook, anchor_probe, popup};
use crate::paint::shadow;
use crate::scale::{u, zoom};
use crate::text::{MONO, RUBIK, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Bounds, Context, CursorStyle, DispatchPhase, Div, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, Hsla, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, RenderImage, Role, SharedString,
    Styled, Subscription, Window, canvas, div, img, prelude::*, relative, rgb,
};
use hlsr_ui::controllers::hud_editor::{HudEditor, Rgb, hex_to_rgb, rgb_to_hex};
use hlsr_ui::render::hsv;
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// Сторона круга, px CSS (`.pers__wheel`).
const WHEEL: f32 = hsv::BOX_PX;

/// Событие поля, как у DOM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorEvent {
    /// `input`: выбран цвет `#rrggbb` (строчными).
    Input(SharedString),
}

/// `.pers__input`: 32 px, поля 9, скругление 8.
pub(crate) fn pers_input(theme: &Theme) -> FieldStyle {
    FieldStyle {
        height: 32.,
        pad_x: 9.,
        radius: 8.,
        ..FieldStyle::gx(theme)
    }
}

/// `.pers__input--mono` (HEX).
pub(crate) fn pers_mono(theme: &Theme) -> FieldStyle {
    FieldStyle {
        family: MONO,
        ..pers_input(theme)
    }
}

/// `.pers__field`: подпись 11 px / 600 с разрядкой .06em над полем, строка 20 px `body`;
/// 600 и у поля (`font: inherit` у `.pers__input`).
pub(crate) fn pers_field(name: &'static str, input: Entity<TextInput>, faint: Hsla) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(u(4.))
        .min_w_0()
        .font_weight(FontWeight::SEMIBOLD)
        .child(spaced(name, 11., 20., FontWeight::SEMIBOLD, 0.06, faint))
        .child(input)
}

/// Заливка кольца курсора круга. shortcut: тень GPUI ложится и под прозрачную середину
/// элемента (у CSS — только снаружи), поэтому середина залита цветом круга под ней.
pub(crate) fn cursor_fill(h: f64, s: f64) -> gpui::Rgba {
    let c = hsv::wheel_color_at(h, s);
    rgb(u32::from(c.r) << 16 | u32::from(c.g) << 8 | u32::from(c.b))
}

thread_local! {
    /// Круг в физических px: по картинке на размер (масштабов немного, круг маленький).
    static WHEELS: RefCell<HashMap<u32, Arc<RenderImage>>> = RefCell::new(HashMap::new());
}

/// shortcut: картинки круга не уходят из атласа — по одной на размер, несколько КБ.
pub(crate) fn wheel_image(side: u32) -> Arc<RenderImage> {
    WHEELS.with(|cache| {
        cache
            .borrow_mut()
            .entry(side)
            .or_insert_with(|| render_image(&hsv::wheel_image(side, hsv::SATURATE)))
            .clone()
    })
}

pub struct ColorField {
    hex: String,
    open: bool,
    disabled: bool,
    /// `.pers__native`: плитка 28 × 28 с «+» вместо образца (свой цвет у палитры HUD и прицела).
    plus: bool,
    label: Option<SharedString>,
    focus: FocusHandle,
    wheel_focus: FocusHandle,
    /// Круг, HEX и каналы на значении `{hudColor}`.
    editor: HudEditor,
    hex_input: Entity<TextInput>,
    channels: [Entity<TextInput>; 3],
    anchor: Rc<Cell<Bounds<Pixels>>>,
    wheel_box: Rc<Cell<Bounds<Pixels>>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ColorEvent> for ColorField {}

const CHANNELS: [&str; 3] = ["r", "g", "b"];

impl ColorField {
    /// Поле с цветом `#000000` (значение `<input type="color">` по умолчанию).
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> ColorField {
        let hex_input = cx.new(|cx| {
            TextInput::new(window, cx)
                .style(pers_mono)
                .max_length(7)
                .with_text("#000000")
        });
        let channel = |window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                TextInput::new(window, cx)
                    .kind(Kind::Number {
                        min: "0".into(),
                        max: "255".into(),
                        step: SharedString::default(),
                    })
                    .style(pers_input)
                    .with_text("0")
            })
        };
        let channels = [
            channel(window, cx),
            channel(window, cx),
            channel(window, cx),
        ];
        let mut subscriptions = vec![cx.subscribe(&hex_input, |this, input, event, cx| {
            if *event == FieldEvent::Change {
                let text = input.read(cx).text().to_owned();
                if let Some(value) = this.editor.on_hud_hex(&text) {
                    this.apply(value, cx);
                }
            }
        })];
        for (index, input) in channels.iter().enumerate() {
            subscriptions.push(cx.subscribe(input, move |this, input, event, cx| {
                if *event == FieldEvent::Change {
                    let text = input.read(cx).value();
                    let value = this.editor.on_hud_channel(CHANNELS[index], &text);
                    this.apply(value, cx);
                }
            }));
        }
        ColorField {
            hex: "#000000".into(),
            open: false,
            disabled: false,
            plus: false,
            label: None,
            focus: cx.focus_handle().tab_stop(true),
            wheel_focus: cx.focus_handle().tab_stop(true),
            editor: HudEditor::new(color_value(Rgb::new(0., 0., 0.)), false),
            hex_input,
            channels,
            anchor: Rc::default(),
            wheel_box: Rc::default(),
            _subscriptions: subscriptions,
        }
    }

    /// Начальный цвет (`#rrggbb`).
    pub fn with_hex(mut self, hex: &str, cx: &mut Context<Self>) -> Self {
        self.set_hex(hex, cx);
        self
    }

    /// `aria-label`.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Вид `.pers__native` (плитка палитры с «+»).
    pub fn plus(mut self) -> Self {
        self.plus = true;
        self
    }

    pub fn hex(&self) -> &str {
        &self.hex
    }

    /// Цвет снаружи (`:value`), без события; не `#rrggbb` — без изменений.
    pub fn set_hex(&mut self, hex: &str, cx: &mut Context<Self>) {
        if let Some(color) = hex_to_rgb(hex) {
            self.editor.set_value(color_value(color));
            self.hex = rgb_to_hex(color);
            self.sync(cx);
            cx.notify();
        }
    }

    /// `disabled`: ни нажатия, ни фокуса; открытый поповер закрывается.
    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        self.disabled = disabled;
        if disabled {
            self.open = false;
        }
        cx.notify();
    }

    /// Новое значение редактора: поля, курсор круга, `input`.
    fn apply(&mut self, value: Value, cx: &mut Context<Self>) {
        let color = Rgb::from_value(value.get("hudColor"));
        self.editor.set_value(value);
        let hex = rgb_to_hex(color);
        self.sync(cx);
        if hex != self.hex {
            self.hex = hex.clone();
            cx.emit(ColorEvent::Input(hex.into()));
        }
        cx.notify();
    }

    /// Поля следуют за цветом (`:value="hudHex"`, `:value="value.hudColor[ch]"`).
    fn sync(&mut self, cx: &mut Context<Self>) {
        let color = self.editor.hud_color();
        let hex = rgb_to_hex(color);
        self.hex_input
            .update(cx, |input, cx| input.set_text(hex, cx));
        for (input, channel) in self.channels.iter().zip([color.r, color.g, color.b]) {
            let text = hlsr_core::json_text::js_number(channel).to_string();
            input.update(cx, |input, cx| input.set_text(text, cx));
        }
    }

    pub(crate) fn set_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.open == open {
            return;
        }
        self.open = open;
        if open {
            window.focus(&self.wheel_focus, cx);
        } else {
            self.editor.on_wheel_up();
            let inside = |handle: FocusHandle| handle.contains_focused(window, cx);
            if inside(self.wheel_focus.clone())
                || inside(self.hex_input.focus_handle(cx))
                || self
                    .channels
                    .iter()
                    .any(|input| inside(input.focus_handle(cx)))
            {
                window.focus(&self.focus, cx);
            }
        }
        cx.notify();
    }

    /// Точка в коробке круга (px CSS от её угла).
    fn wheel_point(&self, at: gpui::Point<Pixels>, window: &Window) -> (f64, f64) {
        let zoom = zoom(window);
        let origin = self.wheel_box.get().origin;
        let css = |v: Pixels| f64::from(f32::from(v) / zoom);
        (css(at.x - origin.x), css(at.y - origin.y))
    }

    fn popover(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.global::<Theme>();
        let zoom = zoom(window);
        let faint = theme.color("l-text-faint");
        let ring_on = focus_visible(&self.wheel_focus, window);
        let focus_color = theme.color("accent-focus");
        let side = device_size(window, WHEEL, WHEEL).0;
        let (fx, fy) = self.editor.wheel_cursor();
        let under = cursor_fill(self.editor.wheel.h, self.editor.wheel.s);
        let dragging = self.editor.wheel_dragging();
        let view = cx.entity().downgrade();
        let probe = self.wheel_box.clone();
        // Пока тянут — мышь окна (захват указателя круга).
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
                                this.editor.on_wheel_up();
                                cx.notify();
                                return;
                            }
                            let (x, y) = this.wheel_point(event.position, window);
                            let w = f64::from(WHEEL);
                            if let Some(value) = this.editor.on_wheel_move(x, y, w, w) {
                                this.apply(value, cx);
                            }
                        })
                        .ok();
                });
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        up.update(cx, |this, cx| {
                            this.editor.on_wheel_up();
                            cx.notify();
                        })
                        .ok();
                    }
                });
            },
        )
        .absolute()
        .size_full();
        let wheel = div()
            .id("wheel")
            .role(Role::Slider)
            .aria_numeric_value(self.editor.wheel.h.round())
            .aria_min_numeric_value(0.)
            .aria_max_numeric_value(359.)
            .track_focus(&self.wheel_focus)
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
                    let value = this.editor.on_wheel_down(x, y, w, w);
                    this.apply(value, cx);
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
                if let Some(value) = this.editor.on_wheel_key(key) {
                    this.apply(value, cx);
                }
            }));
        let field = |name, input| pers_field(name, input, faint);
        let [r, g, b] = self.channels.clone();
        let fields = div()
            .flex()
            .flex_col()
            .gap(u(8.))
            .w(u(168.))
            .font_family(RUBIK)
            .child(field("HEX", self.hex_input.clone()))
            .child(
                div()
                    .flex()
                    .gap(u(6.))
                    .child(field("R", r).flex_1())
                    .child(field("G", g).flex_1())
                    .child(field("B", b).flex_1()),
            );
        let dismiss = cx.entity().downgrade();
        let on_dismiss = move |window: &mut Window, cx: &mut App| {
            dismiss
                .update(cx, |this, cx| this.set_open(false, window, cx))
                .ok();
        };
        popup(
            "color-popover",
            self.open,
            Place {
                anchor: self.anchor.get(),
                clip: None,
                up: false,
                align_right: false,
            },
            // Пикер Chromium на его месте не стекло и в Liquid Glass.
            PopLook {
                glass: false,
                ..PopLook::DROPDOWN
            },
            window,
            cx,
            on_dismiss,
            move || {
                div()
                    .p(u(4.))
                    .flex()
                    .items_center()
                    .gap(u(14.))
                    .child(wheel)
                    .child(fields)
            },
        )
    }
}

/// Значение редактора с одним цветом.
fn color_value(color: Rgb) -> Value {
    json!({ "hudColor": color.to_value() })
}

impl Render for ColorField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let zoom = zoom(window);
        let border = theme.color("l-border");
        let focus_color = theme.color("accent-focus");
        let ring_on = focus_visible(&self.focus, window);
        let live = !self.disabled;
        let color: gpui::Hsla = hex_to_rgb(&self.hex)
            .map(|c| rgb(((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32).into())
            .unwrap_or_else(gpui::black);
        let plus = self.plus;
        let tile_border = white(0.14);
        let dim = theme.color("l-text-dim");
        let popover = self.popover(window, cx);
        div()
            .id("color")
            .role(Role::ColorWell)
            .when_some(self.label.clone(), |el, label| el.aria_label(label))
            .on_action(cx.listener(|this, _: &Dismiss, window, cx| {
                if this.open {
                    this.set_open(false, window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .relative()
            .flex_none()
            .rounded(u(8.))
            .when(!plus, |el| {
                el.w(u(46.))
                    .h(u(30.))
                    .border_1()
                    .border_color(border)
                    .bg(white(0.05))
                    .child(
                        div()
                            .absolute()
                            .left(u(4.))
                            .top(u(6.))
                            .right(u(4.))
                            .bottom(u(6.))
                            .border_1()
                            .border_color(rgb(0x777777))
                            .bg(color),
                    )
            })
            .when(plus, |el| {
                el.size(u(28.))
                    .border_2()
                    .border_color(tile_border)
                    .bg(white(0.055))
                    .text_color(dim)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(crate::icons::fa_glyph("fas fa-plus", 10.))
            })
            .child(anchor_probe(self.anchor.clone()))
            .when(live, |el| {
                el.track_focus(&self.focus)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| {
                        let open = !this.open;
                        this.set_open(open, window, cx);
                    }))
            })
            .when(ring_on, |el| {
                el.child(outline(zoom, 8., 2., 2., focus_color))
            })
            .child(popover)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scale::zpx;
    use gpui::{Modifiers, TestAppContext, point};

    struct Form {
        field: Entity<ColorField>,
        seen: Vec<SharedString>,
    }

    impl Render for Form {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().p(u(20.)).child(self.field.clone())
        }
    }

    #[gpui::test]
    fn fields_and_wheel_keys_emit_hex(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (form, cx) = cx.add_window_view(|window, cx| {
            let field = cx.new(|cx| ColorField::new(window, cx).with_hex("#FF0000", cx));
            cx.subscribe(&field, |form: &mut Form, _, ColorEvent::Input(hex), _| {
                form.seen.push(hex.clone())
            })
            .detach();
            Form {
                field,
                seen: Vec::new(),
            }
        });
        let field = form.read_with(cx, |form, _| form.field.clone());
        // Щелчок по полю открывает поповер, фокус — на круге.
        let at = point(zpx(1., 40.), zpx(1., 35.));
        cx.simulate_click(at, Modifiers::default());
        cx.run_until_parked();
        assert!(field.read_with(cx, |field, _| field.open));
        // Оттенок +3°: красный уходит в оранжевый (насыщенность и яркость прежние).
        cx.simulate_keystrokes("right");
        // Канал G из поля: Enter — change.
        let g = field.read_with(cx, |field, _| field.channels[1].clone());
        let focus = cx.update(|_, cx| g.focus_handle(cx));
        cx.update(|window, cx| window.focus(&focus, cx));
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input("300");
        cx.simulate_keystrokes("enter");
        // Esc закрывает поповер, фокус — у поля.
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        let (hex, open, hex_text) = field.read_with(cx, |field, cx| {
            (
                field.hex.clone(),
                field.open,
                field.hex_input.read(cx).text().to_owned(),
            )
        });
        assert!(!open);
        let swatch = field.read_with(cx, |field, _| field.focus.clone());
        assert!(cx.update(|window, _| swatch.is_focused(window)));
        assert_eq!(hex, "#ffff00");
        assert_eq!(hex_text, "#ffff00");
        form.read_with(cx, |form, _| {
            assert_eq!(form.seen.len(), 2);
            assert_eq!(form.seen[1], "#ffff00");
        });
    }
}
