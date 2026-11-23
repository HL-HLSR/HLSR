//! Ползунок `.gx-range` (settings-fields.scss, GhostingSettingsExtras.vue 0.4.9): дорожка
//! 28 px — полоса 4 px `rgba(255,255,255,.12)`, заливка `--accent` от умолчания до значения,
//! метка умолчания 2 × 12 (гаснет, когда значение по умолчанию), бегунок 18 px с рамкой 2 px
//! `--accent` и тенью `0 1px 3px rgba(0,0,0,.4)`; при нажатии растёт до 1.12 (`--l-fast`), фокус
//! с клавиатуры — кольцо 3 px `--accent-soft`. Центр бегунка ходит от края до края дорожки.
//!
//! Мышь: нажатие ставит бегунок под курсор и захватывает мышь до отпускания (движение за
//! пределами дорожки и окна тоже ведёт); пока тянут, бегунок идёт за курсором свободно
//! (`sliding`), отпускание — `change`. Метка умолчания — детент: ближе 6 px (`SLIDER_DETENT_PX`
//! от ширины `<input>` = дорожка + 18) бегунок встаёт на умолчание. Двойной щелчок — умолчание.
//! Клавиши (`RangeInputType`): ←↓ / →↑ — шаг (у `step="any"` — сотая диапазона), PageUp/PageDown
//! — десятая, Home/End — края. Привязка к шагу — `hlsr_ui::fields::range_snap`.
//!
//! [`Slider::native`] — `<input type="range">` без своего CSS (`.pers__slider` 0.4.9, только
//! `accent-color`): ползунок Chromium в тёмной схеме — высота 18, дорожка 8 px #3b3b3b с рамкой
//! 1 px #858585, до бегунка — акцент, бегунок 16 px акцентом; центр бегунка ходит от 8 px до
//! ширины − 8 (снято с живого 0.4.9, `docs/gpui/live/real/liquid/e-19-perso.png`).

use super::button::{Handler, Preview, child_id, focus_visible, outline};
use crate::motion::{Timing, mix, tween};
use crate::paint::{ring, shadow};
use crate::scale::{u, zoom};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Bounds, CursorStyle, DispatchPhase, Div, ElementId, Entity, EntityId,
    FocusHandle, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Pixels, RenderOnce, Role, SharedString, StyleRefinement, Styled, Window, canvas,
    div, hsla, prelude::*, relative, rgb,
};
use hlsr_ui::fields::{range_key, range_snap};
use std::rc::Rc;

/// Притяжение к метке умолчания, CSS px (`SLIDER_DETENT_PX`).
pub const DETENT_PX: f32 = 6.;
/// Бегунок, CSS px.
const THUMB: f32 = 18.;
/// Бегунок нативного ползунка Chromium, CSS px.
const NATIVE_THUMB: f32 = 16.;

#[derive(IntoElement)]
pub struct Slider {
    id: ElementId,
    value: f64,
    min: f64,
    max: f64,
    step: Option<f64>,
    default: Option<f64>,
    disabled: bool,
    native: bool,
    label: Option<SharedString>,
    preview: Preview,
    base: Div,
    on_input: Option<Handler<f64>>,
    on_change: Option<Handler<f64>>,
}

/// Ползунок `min…max` со значением `value`; ширину задаёт место (по умолчанию — вся).
pub fn slider(id: impl Into<ElementId>, value: f64, min: f64, max: f64) -> Slider {
    Slider {
        id: id.into(),
        value,
        min,
        max,
        step: None,
        default: None,
        disabled: false,
        native: false,
        label: None,
        preview: Preview::default(),
        base: div().w_full(),
        on_input: None,
        on_change: None,
    }
}

impl Slider {
    /// Шаг; без него — `step="any"` (значение как есть, привязывает место).
    pub fn step(mut self, step: f64) -> Self {
        self.step = Some(step);
        self
    }

    /// Значение по умолчанию: заливка от него, метка, детент и двойной щелчок.
    pub fn default_value(mut self, value: f64) -> Self {
        self.default = Some(value);
        self
    }

    /// Без событий и бледнее (.45, как `.gx-row.is-off`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Вид нативного ползунка Chromium вместо `.gx-range` (см. начало модуля).
    pub fn native(mut self) -> Self {
        self.native = true;
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn preview(mut self, preview: Preview) -> Self {
        self.preview = preview;
        self
    }

    /// `input`: значение под курсором, пока тянут (и при клавише, двойном щелчке).
    pub fn on_input(mut self, handler: impl Fn(&f64, &mut Window, &mut App) + 'static) -> Self {
        self.on_input = Some(Rc::new(handler));
        self
    }

    /// `change`: отпустили, клавиша, двойной щелчок.
    pub fn on_change(mut self, handler: impl Fn(&f64, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl Styled for Slider {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

struct SliderState {
    focus: FocusHandle,
    /// Тянут: где бегунок (за курсором, до отпускания).
    sliding: Option<f64>,
    /// Дорожка в прошлом кадре.
    bounds: Bounds<Pixels>,
}

/// Диапазон, шаг и умолчание — что нужно обработчикам мыши и клавиш.
#[derive(Clone, Copy)]
struct Range {
    min: f64,
    max: f64,
    step: Option<f64>,
    default: Option<f64>,
    native: bool,
}

impl Range {
    fn ratio(self, value: f64) -> f32 {
        let span = self.max - self.min;
        if span > 0. {
            ((value - self.min) / span).clamp(0., 1.) as f32
        } else {
            0.
        }
    }

    /// Значение под курсором `x` на дорожке `bounds` (зум `zoom`): детент и шаг.
    fn at(self, x: Pixels, bounds: Bounds<Pixels>, zoom: f32) -> f64 {
        let width = f32::from(bounds.size.width);
        // Центр бегунка `.gx-range` ходит по всей дорожке, у нативного — без половины бегунка
        // с краёв; ширина `<input>` — дорожка + бегунок и сама дорожка.
        let (lead, input) = if self.native {
            (NATIVE_THUMB / 2. * zoom, width / zoom)
        } else {
            (0., width / zoom + THUMB)
        };
        let travel = width - 2. * lead;
        let along = if travel > 0. {
            (f64::from(f32::from(x - bounds.origin.x) - lead) / f64::from(travel)).clamp(0., 1.)
        } else {
            0.
        };
        let span = self.max - self.min;
        let mut raw = self.min + along * span;
        if let Some(default) = self.default
            && width > 0.
            && span > 0.
        {
            let input = f64::from(input);
            if (raw - default).abs() / span * input <= f64::from(DETENT_PX) {
                raw = default;
            }
        }
        range_snap(raw, self.min, self.max, self.step)
    }
}

fn emit(handler: &Option<Handler<f64>>, value: f64, window: &mut Window, cx: &mut App) {
    if let Some(handler) = handler {
        handler(&value, window, cx);
    }
}

/// Отпустили: `change` с последним значением, если тянули.
fn release(
    state: &Entity<SliderState>,
    change: &Option<Handler<f64>>,
    owner: EntityId,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(last) = state.update(cx, |s, _| s.sliding.take()) {
        emit(change, last, window, cx);
        cx.notify(owner);
    }
}

impl RenderOnce for Slider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Slider {
            id,
            value,
            min,
            max,
            step,
            default,
            disabled,
            native,
            label,
            preview,
            base,
            on_input,
            on_change,
        } = self;
        let range = Range {
            min,
            max,
            step,
            default,
            native,
        };
        let owner = window.current_view();
        let state = window.use_keyed_state(child_id(&id, "slider"), cx, |_, cx| SliderState {
            focus: cx.focus_handle().tab_stop(true),
            sliding: None,
            bounds: Bounds::default(),
        });
        let (focus, sliding) = {
            let s = state.read(cx);
            (s.focus.clone(), s.sliding)
        };
        let theme = cx.global::<Theme>();
        let accent = theme.color("accent");
        let soft = theme.color("accent-soft");
        let focus_color = theme.color("accent-focus");
        let faint = theme.color("l-text-faint");
        let thumb_bg = theme.color("l-switch-thumb-on");
        let zoom = zoom(window);
        let shown = sliding.unwrap_or(value);
        let ratio = range.ratio(shown);
        let base_at = default.map(|default| range.ratio(default));
        let (from, to) = match base_at {
            Some(at) => (ratio.min(at), ratio.max(at)),
            None => (0., ratio),
        };
        let dragging = sliding.is_some();
        let pressed = dragging || preview.pressed;
        let ring_on = preview.focus || focus_visible(&focus, window);
        let bar = |el: Div| el.absolute().top(u(12.)).h(u(4.)).rounded(u(2.));

        // Дорожка этого кадра и, пока тянут, захват мыши окном.
        let measure = state.clone();
        let track = state.clone();
        let (input_drag, change_drag) = (on_input.clone(), on_change.clone());
        let probe = canvas(
            move |bounds, _, cx| measure.update(cx, |s, _| s.bounds = bounds),
            move |_, (), window, _| {
                if !dragging {
                    return;
                }
                // `.pers__slider input { cursor: pointer }`, у `.gx-range` — `grabbing`.
                window.set_window_cursor_style(if native {
                    CursorStyle::PointingHand
                } else {
                    CursorStyle::ClosedHand
                });
                let moved = track.clone();
                let input = input_drag.clone();
                let change = change_drag.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    let (sliding, bounds) = {
                        let s = moved.read(cx);
                        (s.sliding, s.bounds)
                    };
                    if phase != DispatchPhase::Bubble || sliding.is_none() {
                        return;
                    }
                    // Кнопку отпустили там, где окно не слышало: тянуть нечего.
                    if event.pressed_button != Some(MouseButton::Left) {
                        release(&moved, &change, owner, window, cx);
                        return;
                    }
                    let next = range.at(event.position.x, bounds, zoom);
                    if sliding != Some(next) {
                        moved.update(cx, |s, _| s.sliding = Some(next));
                        emit(&input, next, window, cx);
                        cx.notify(owner);
                    }
                });
            },
        )
        .absolute()
        .size_full();

        let pick = state.clone();
        // Отпускание слушает сама дорожка с первого кадра: быстрый щелчок может отпустить
        // раньше, чем кадр с захватом мыши (`dragging`) нарисован.
        let (up, out) = (state.clone(), state);
        let (change_up, change_out) = (on_change.clone(), on_change.clone());
        let (input_down, change_down) = (on_input.clone(), on_change.clone());
        let (input_key, change_key) = (on_input, on_change);
        let group: SharedString = format!("slider-{id}").into();
        let children: Vec<AnyElement> = if native {
            // Состояние у Chromium: дорожку (и заливку) рисует по наведению на весь `<input>`,
            // бегунок — по наведению на сам бегунок (`ThemePainterDefault::PaintSlider*`).
            // shortcut: цвет наведения не снят эталоном — как у флажка Chromium (акцент на 22 %
            // светлее); нажатие отдельно не рисуется. Снять при сверке «Персонализации».
            let over = mix(accent, gpui::white(), 0.22);
            // Цвета Chromium, не темы: 0.4.9 ставит `html { color-scheme: dark }` при любой теме.
            let track = || div().absolute().left_0().right_0().top(u(5.)).h(u(8.));
            // Центр бегунка — от 8 px до ширины − 8: значение и бегунок — в этих пределах.
            let travel = || div().absolute().top_0().bottom_0().left(u(8.)).right(u(8.));
            let mut children = vec![
                track().rounded(u(4.)).bg(rgb(0x3b3b3b)).into_any_element(),
                travel()
                    .child(
                        div()
                            .absolute()
                            .left(u(-8.))
                            .right(relative(1. - ratio))
                            .top(u(5.))
                            .h(u(8.))
                            .rounded_l(u(4.))
                            .bg(accent)
                            .group_hover(group.clone(), move |s| s.bg(over)),
                    )
                    .into_any_element(),
                // Рамка дорожки — поверх заливки значения, как у Chromium.
                track()
                    .rounded(u(4.))
                    .shadow(vec![ring(zoom, 1., rgb(0x858585).into())])
                    .into_any_element(),
                travel()
                    .child(
                        div()
                            .absolute()
                            .left(relative(ratio))
                            .ml(u(-NATIVE_THUMB / 2.))
                            .top(u(1.))
                            .size(u(NATIVE_THUMB))
                            .rounded_full()
                            .bg(accent)
                            .hover(move |s| s.bg(over)),
                    )
                    .into_any_element(),
                probe.into_any_element(),
            ];
            // `.launcher-root :focus-visible`: 2 px `--accent-focus` со смещением 2.
            if ring_on {
                children.push(outline(zoom, 0., 2., 2., focus_color).into_any_element());
            }
            children
        } else {
            let thumb = tween(
                child_id(&id, "thumb"),
                f32::from(u8::from(pressed)),
                Timing::fast(),
                move |t| {
                    let side = THUMB * (1. + 0.12 * t);
                    let outer = if ring_on {
                        shadow(zoom, 0., 0., 0., 3., soft)
                    } else {
                        shadow(zoom, 0., 1., 3., 0., hsla(0., 0., 0., 0.4))
                    };
                    div()
                        .absolute()
                        .left(relative(ratio))
                        .ml(u(-side / 2.))
                        .top(u(14. - side / 2.))
                        .size(u(side))
                        .rounded_full()
                        .bg(thumb_bg)
                        // Рамка 2 px — inset-тенью: растёт с бегунком и с зумом.
                        .shadow(vec![outer, ring(zoom, 2. * side / THUMB, accent)])
                        .cursor(if pressed {
                            CursorStyle::ClosedHand
                        } else {
                            CursorStyle::OpenHand
                        })
                },
            );
            vec![
                bar(div())
                    .left_0()
                    .right_0()
                    .bg(hsla(0., 0., 1., 0.12))
                    .into_any_element(),
                bar(div())
                    .left(relative(from))
                    .w(relative(to - from))
                    .bg(accent)
                    .into_any_element(),
                div()
                    .absolute()
                    .when_some(base_at, |el, at| el.left(relative(at)))
                    .ml(u(-1.))
                    .top(u(8.))
                    .w(u(2.))
                    .h(u(12.))
                    .rounded(u(1.))
                    .bg(faint)
                    .when(base_at.is_none() || default == Some(value), |el| {
                        el.opacity(0.)
                    })
                    .into_any_element(),
                thumb.into_any_element(),
                probe.into_any_element(),
            ]
        };
        let meta = crate::live::Meta::new("slider", id.clone())
            .label(label.clone())
            .disabled(disabled)
            .focused(focus.is_focused(window))
            .value(shown.to_string());
        let element = base
            .id(id.clone())
            .debug_selector(|| format!("{id}/track"))
            .role(Role::Slider)
            .aria_numeric_value(shown)
            .aria_min_numeric_value(min)
            .aria_max_numeric_value(max)
            .when_some(label, |el, label| el.aria_label(label))
            .group(group)
            .relative()
            .h(u(if native { 18. } else { 28. }))
            .children(children)
            .when(disabled, |el| el.opacity(0.45))
            .when(!disabled, |el| {
                el.track_focus(&focus)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        move |event: &MouseDownEvent, window, cx| {
                            // Второе нажатие подряд — `dblclick`: значение по умолчанию.
                            if event.click_count >= 2
                                && let Some(default) = range.default
                            {
                                pick.update(cx, |s, _| s.sliding = None);
                                emit(&input_down, default, window, cx);
                                emit(&change_down, default, window, cx);
                                cx.notify(owner);
                                return;
                            }
                            let bounds = pick.read(cx).bounds;
                            let next = range.at(event.position.x, bounds, zoom);
                            pick.update(cx, |s, _| s.sliding = Some(next));
                            emit(&input_down, next, window, cx);
                            cx.notify(owner);
                        },
                    )
                    .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                        release(&up, &change_up, owner, window, cx)
                    })
                    .on_mouse_up_out(MouseButton::Left, move |_, window, cx| {
                        release(&out, &change_out, owner, window, cx)
                    })
                    .on_key_down(move |event: &KeyDownEvent, window, cx| {
                        let key = match event.keystroke.key.as_str() {
                            "right" | "up" => 0,
                            "left" | "down" => 1,
                            "pageup" => 2,
                            "pagedown" => 3,
                            "home" => 4,
                            "end" => 5,
                            _ => return,
                        };
                        cx.stop_propagation();
                        let next = range_key(value, min, max, step, key);
                        if next != value {
                            emit(&input_key, next, window, cx);
                            emit(&change_key, next, window, cx);
                        }
                    })
            });
        crate::live::mark(meta, element)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scale::zpx;
    use gpui::{
        Context, Modifiers, MouseUpEvent, PlatformInput, Render, TestAppContext, VisualTestContext,
        point,
    };

    /// Пиксели окна теста (зум 1).
    fn p(x: f32) -> Pixels {
        zpx(1., x)
    }

    #[derive(Default)]
    struct Form {
        value: f64,
        inputs: Vec<f64>,
        changes: Vec<f64>,
    }

    impl Render for Form {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            // Дорожка 200 px с 20-го: значение 0…10, умолчание 5 (x = 120).
            div().size_full().p(u(20.)).child(
                div().w(u(200.)).child(
                    slider("s", self.value, 0., 10.)
                        .default_value(5.)
                        .on_input(cx.listener(|this, value: &f64, _, cx| {
                            this.inputs.push(*value);
                            this.value = *value;
                            cx.notify();
                        }))
                        .on_change(cx.listener(|this, value: &f64, _, cx| {
                            this.changes.push(*value);
                            this.value = *value;
                            cx.notify();
                        })),
                ),
            )
        }
    }

    fn open(cx: &mut TestAppContext, value: f64) -> (gpui::Entity<Form>, &mut VisualTestContext) {
        cx.update(crate::init);
        let (form, cx) = cx.add_window_view(move |_, _| Form {
            value,
            ..Form::default()
        });
        cx.run_until_parked();
        (form, cx)
    }

    fn at(x: f32) -> gpui::Point<Pixels> {
        point(p(x), p(34.))
    }

    #[gpui::test]
    fn drag_captures_mouse_and_sticks_to_default(cx: &mut TestAppContext) {
        let (form, cx) = open(cx, 2.);
        let track = cx.debug_bounds("s/track").unwrap();
        assert_eq!((track.left(), track.right()), (p(20.), p(220.)));
        let none = Modifiers::default();
        cx.simulate_mouse_down(at(40.), MouseButton::Left, none);
        // За дорожкой — край; у метки (ближе 6 px от ширины 218) — умолчание.
        cx.simulate_mouse_move(at(400.), MouseButton::Left, none);
        cx.simulate_mouse_move(at(124.), MouseButton::Left, none);
        cx.simulate_mouse_move(at(140.), MouseButton::Left, none);
        cx.simulate_mouse_move(at(0.), MouseButton::Left, none);
        cx.simulate_mouse_move(at(117.), MouseButton::Left, none);
        cx.simulate_mouse_up(at(117.), MouseButton::Left, none);
        // После отпускания мышь ползунок не ведёт.
        cx.simulate_mouse_move(at(60.), None, none);
        form.update(cx, |form, _| {
            assert_eq!(form.inputs, [1., 10., 5., 6., 0., 5.]);
            assert_eq!(form.changes, [5.]);
            assert_eq!(form.value, 5.);
        });
    }

    #[gpui::test]
    fn release_before_next_frame_still_changes(cx: &mut TestAppContext) {
        let (form, cx) = open(cx, 2.);
        let none = Modifiers::default();
        // Нажали и отпустили в одном такте: кадра с захватом мыши ещё нет.
        cx.update(|window, cx| {
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: at(40.),
                    modifiers: none,
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: at(40.),
                    modifiers: none,
                    click_count: 1,
                }),
                cx,
            );
        });
        form.update(cx, |form, _| {
            assert_eq!(form.changes, [1.]);
            assert_eq!(form.value, 1.);
        });
    }

    #[gpui::test]
    fn double_click_returns_default(cx: &mut TestAppContext) {
        let (form, cx) = open(cx, 8.);
        cx.simulate_click(at(40.), Modifiers::default());
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at(40.),
            modifiers: Modifiers::default(),
            click_count: 2,
            first_mouse: false,
        });
        cx.simulate_mouse_up(at(40.), MouseButton::Left, Modifiers::default());
        form.update(cx, |form, _| {
            assert_eq!(form.changes, [1., 5.]);
            assert_eq!(form.value, 5.);
        });
    }

    #[gpui::test]
    fn keys_step_like_chromium(cx: &mut TestAppContext) {
        let (form, cx) = open(cx, 5.);
        // Tab в GPUI не привязан (обход фокуса — F7): следующая остановка Tab напрямую.
        cx.update(|window, cx| window.focus_next(cx));
        // step="any": шаг — сотая диапазона, страница — десятая.
        cx.simulate_keystrokes("right");
        cx.simulate_keystrokes("pagedown");
        cx.simulate_keystrokes("end");
        cx.simulate_keystrokes("end");
        cx.simulate_keystrokes("home");
        cx.simulate_keystrokes("up");
        form.update(cx, |form, _| {
            assert_eq!(form.changes, [5.1, 4.1, 10., 0., 0.1]);
            assert_eq!(form.inputs, form.changes);
        });
    }

    #[test]
    fn range_maps_pointer_with_detent_and_step() {
        let bounds = Bounds::new(point(p(0.), p(0.)), gpui::size(p(100.), p(28.)));
        let range = Range {
            min: 0.,
            max: 1.,
            step: None,
            default: Some(0.5),
            native: false,
        };
        // Ширина <input> 118: 6 px — 0.0508 диапазона.
        assert_eq!(range.at(p(55.), bounds, 1.), 0.5);
        assert_eq!(range.at(p(56.), bounds, 1.), 0.56);
        // Зум 2: та же доля — вдвое меньше CSS px, детент шире.
        assert_eq!(range.at(p(55.), bounds, 2.), 0.5);
        let stepped = Range {
            step: Some(0.25),
            default: None,
            ..range
        };
        assert_eq!(stepped.at(p(60.), bounds, 1.), 0.5);
        assert_eq!(stepped.at(p(-10.), bounds, 1.), 0.);
        // Нативный: центр бегунка 16 px ходит от 8 до 92; зум 2 — от 16 до 84.
        let native = Range {
            default: None,
            native: true,
            ..range
        };
        assert_eq!(native.at(p(8.), bounds, 1.), 0.);
        assert_eq!(native.at(p(50.), bounds, 1.), 0.5);
        assert_eq!(native.at(p(92.), bounds, 1.), 1.);
        assert_eq!(native.at(p(16.), bounds, 2.), 0.);
        assert_eq!(native.at(p(84.), bounds, 2.), 1.);
    }
}
