//! Кнопки (F8): тоны главной кнопки 0.4.9 (`.download-bar__play.tone-*` DownloadBar.vue: ready,
//! progress, repair, danger, muted, neutral — заливка, наведение, нажатие, свечение `box-shadow`,
//! `is-busy`) и `.gx-btn` (settings-fields.scss). Размер, шрифт и содержимое тоновой кнопки задаёт
//! место (главная кнопка — G2): [`Button`] — `Styled` и `ParentElement`.
//!
//! Здесь же общее для контролов F8: фокус по id элемента ([`focus_handle`]), контур
//! `:focus-visible` ([`outline`]), принудительные состояния для витрины ([`Preview`]).
//!
//! Переходы как в CSS: заливка — `--l-fast` (наведение, нажатие — [`active`] и смена тона),
//! свечение тоновой — `--l-med`; `transform: scale(…)` при нажатии — [`press`] ([`Look::push`]).

use crate::motion::{AnimGate, Push, Timing, active, hover, mix, press, tween};
use crate::paint::{ring, shadow};
use crate::scale::{u, zoom};
use crate::text::{RUBIK, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, FocusHandle, FontWeight, Hsla, IntoElement,
    ParentElement, RenderOnce, Role, StyleRefinement, Styled, Window, div, hsla, prelude::*,
    relative, rgb,
};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

/// Обработчик события контрола — того же вида, что `cx.listener(…)`.
pub type Handler<T> = Rc<dyn Fn(&T, &mut Window, &mut App)>;

/// Принудительные `:hover`, `:active`, `:focus-visible` (витрина и сцены без мыши).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Preview {
    pub hover: bool,
    pub pressed: bool,
    pub focus: bool,
}

/// Id состояния контрола под его id.
pub(crate) fn child_id(id: &ElementId, name: &'static str) -> ElementId {
    ElementId::NamedChild(Arc::new(id.clone()), name.into())
}

/// Фокус контрола (остановка Tab): живёт, пока элемент с этим id есть в кадре.
pub(crate) fn focus_handle(id: &ElementId, window: &mut Window, cx: &mut App) -> FocusHandle {
    window
        .use_keyed_state(child_id(id, "focus"), cx, |_, cx| {
            cx.focus_handle().tab_stop(true)
        })
        .read(cx)
        .clone()
}

/// `:focus-visible`: в фокусе, и фокус пришёл с клавиатуры.
pub(crate) fn focus_visible(handle: &FocusHandle, window: &Window) -> bool {
    handle.is_focused(window) && window.last_input_was_keyboard()
}

/// Щелчок Enter: у `<input type="checkbox">` он ничего не меняет (только пробел и мышь).
pub(crate) fn enter_key(event: &ClickEvent) -> bool {
    matches!(event, ClickEvent::Keyboard(key) if key.button == gpui::KeyboardButton::Enter)
}

pub(crate) fn white(alpha: f32) -> Hsla {
    hsla(0., 0., 1., alpha)
}

/// Подложка выключенных кнопок окна-диалога для [`faded`]: `--l-modal-bg`, пока она сплошная
/// (в Liquid Glass под кнопкой стекло — `None`).
pub(crate) fn window_under(theme: &Theme) -> Option<Hsla> {
    let under = theme.color("l-modal-bg");
    (under.a >= 1. && theme.liquid_share() <= 0.).then_some(under)
}

/// `opacity` выключенной кнопки (`:disabled { opacity: .45 }`) — группой, как у Chromium. GPUI
/// кладёт `opacity` на каждый слой отдельно: текст смешивается с уже выцветшим фоном, и тёмный
/// текст на светлой кнопке почти сливается с ней. Здесь фон, рамка и тени блока берут
/// прозрачность сами (как раньше), а текст — цвет группы над сплошной подложкой `under`:
/// `α·текст + (1 − α)·под` (над сплошной подложкой это ровно группа).
/// shortcut: подложка одна на кнопку ([`window_under`] — заливка окна), дети со своим цветом и
/// фоном не выцветают; звать у кнопок, чьи дети наследуют цвет текста. Без своего фона (слои не
/// перекрываются, и `opacity` точна) и без подложки — прежняя `opacity`.
pub(crate) fn faded<E: Styled>(mut el: E, alpha: f32, under: Option<Hsla>) -> E {
    let style = el.style();
    let filled = matches!(
        &style.background,
        Some(gpui::Fill::Color(bg)) if bg.as_solid().is_some_and(|c| c.a > 0.)
    );
    let (Some(text), Some(under), true) = (style.text.color, under, filled) else {
        return el.opacity(alpha);
    };
    if let Some(gpui::Fill::Color(bg)) = style.background.as_mut() {
        *bg = bg.opacity(alpha);
    }
    if let Some(border) = style.border_color.as_mut() {
        *border = border.opacity(alpha);
    }
    for shadow in style.box_shadow.iter_mut().flatten() {
        shadow.color = shadow.color.opacity(alpha);
    }
    let group = mix(under, Hsla { a: 1., ..text }, alpha);
    style.text.color = Some(Hsla { a: text.a, ..group });
    el
}

/// `outline: <width>px solid <color>; outline-offset: <offset>px` у блока со скруглением
/// `radius` (CSS px): рамка inset-тенью у блока поверх (родитель — `relative`). Скругление
/// контура, как у Chromium, — радиус блока плюс смещение (у прямого блока углы прямые).
pub fn outline(zoom: f32, radius: f32, offset: f32, width: f32, color: Hsla) -> Div {
    let out = offset + width;
    let round = if radius > 0. { radius + out } else { 0. };
    div()
        .absolute()
        .top(u(-out))
        .left(u(-out))
        .right(u(-out))
        .bottom(u(-out))
        .rounded(u(round.max(0.)))
        .shadow(vec![ring(zoom, width, color)])
}

/// Тон главной кнопки (`buttonTone` DownloadBar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ready,
    Progress,
    Repair,
    Danger,
    Muted,
    Neutral,
}

/// Вид кнопки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// `.download-bar__play.tone-*`: радиус `--l-radius`, контур фокуса со смещением 4 px.
    Tone(Tone),
    /// `.gx-btn`: 30 px, рамка, 12 px / 500.
    Gx,
    /// `.gx-btn--icon`: 34 × 34 без полей.
    GxIcon,
}

/// Цвета и форма вида.
struct Paint {
    bg: Hsla,
    hover_bg: Hsla,
    pressed_bg: Hsla,
    text: Hsla,
    hover_text: Hsla,
    /// `border: 1px` (обычный, при наведении).
    border: Option<(Hsla, Hsla)>,
    /// `box-shadow: inset 0 0 0 1px` (muted, neutral).
    ring: Option<Hsla>,
    /// `box-shadow: 0 6px 22px` (при наведении `0 8px 26px`).
    glow: Option<Hsla>,
    radius: f32,
    focus_offset: f32,
    /// `opacity` у `:disabled` / `.is-busy`.
    disabled_opacity: f32,
}

impl Look {
    /// `:active:not(:disabled)`: `.download-bar__play` и `.tool-panel__action` — `scale(.98)`
    /// `--l-fast`; `.gx-btn` — `scale(.96)` за 90 мс `--l-ease`.
    fn push(self) -> Push {
        match self {
            Look::Tone(_) => Push::fast(0.98),
            Look::Gx | Look::GxIcon => Push::new(
                0.96,
                Timing::new(90., crate::motion::registry().tokens.l_ease),
            ),
        }
    }

    fn paint(self, theme: &Theme) -> Paint {
        let tone = |bg: &str, hover: &str, pressed: &str, text: Hsla, glow: Option<Hsla>| Paint {
            bg: theme.color(bg),
            hover_bg: theme.color(hover),
            pressed_bg: theme.color(pressed),
            text,
            hover_text: text,
            border: None,
            ring: None,
            glow,
            radius: theme.len("l-radius"),
            focus_offset: 4.,
            disabled_opacity: 0.75,
        };
        let gx = Paint {
            bg: white(0.05),
            hover_bg: white(0.1),
            pressed_bg: white(0.16),
            text: theme.color("l-text-dim"),
            hover_text: theme.color("l-text"),
            border: Some((theme.color("l-border"), theme.color("l-border-strong"))),
            ring: None,
            glow: None,
            radius: 8.,
            focus_offset: 2.,
            disabled_opacity: 0.4,
        };
        match self {
            Look::Tone(Tone::Ready) => tone(
                "btn-ready-bg",
                "btn-ready-hover",
                "btn-ready-pressed",
                theme.color("btn-ready-text"),
                Some(theme.color("btn-ready-glow")),
            ),
            Look::Tone(Tone::Progress) => tone(
                "btn-progress-bg",
                "btn-progress-hover",
                "btn-progress-pressed",
                theme.color("btn-progress-text"),
                Some(theme.color("btn-progress-glow")),
            ),
            Look::Tone(Tone::Repair) => tone(
                "btn-repair-bg",
                "btn-repair-hover",
                "btn-repair-pressed",
                rgb(0x241a05).into(),
                Some(theme.color("btn-repair-glow")),
            ),
            Look::Tone(Tone::Danger) => tone(
                "btn-danger-bg",
                "btn-danger-hover",
                "btn-danger-pressed",
                rgb(0xffffff).into(),
                Some(theme.color("btn-danger-glow")),
            ),
            Look::Tone(Tone::Muted) => Paint {
                ring: Some(theme.color("l-border-strong")),
                ..tone(
                    "btn-muted-bg",
                    "btn-muted-hover",
                    "btn-muted-hover",
                    theme.color("l-text-dim"),
                    None,
                )
            },
            Look::Tone(Tone::Neutral) => Paint {
                bg: white(0.08),
                hover_bg: white(0.13),
                pressed_bg: white(0.16),
                ring: Some(theme.color("l-border-strong")),
                ..tone(
                    "btn-muted-bg",
                    "btn-muted-hover",
                    "btn-muted-hover",
                    theme.color("l-text"),
                    None,
                )
            },
            Look::Gx | Look::GxIcon => gx,
        }
    }
}

/// Цвета тона, которые перетекают при его смене (`tone-*`): заливки и свечение.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Tones {
    bg: Hsla,
    hover: Hsla,
    pressed: Hsla,
    glow: Hsla,
}

impl Tones {
    fn of(paint: &Paint) -> Tones {
        Tones {
            bg: paint.bg,
            hover: paint.hover_bg,
            pressed: paint.pressed_bg,
            glow: paint.glow.unwrap_or_default(),
        }
    }
}

/// Смена тона: `transition: background var(--l-fast), box-shadow var(--l-med)` — от текущих
/// цветов к новым (надпись и значок меняются сразу: `color` не в списке).
struct ToneFade {
    from: Tones,
    to: Tones,
    since: Option<Instant>,
}

impl ToneFade {
    fn at(&self, now: Instant) -> Tones {
        let Some(since) = self.since else {
            return self.to;
        };
        let ms = now.saturating_duration_since(since).as_secs_f64() * 1000.;
        let (fill, glow) = (Timing::fast().at(ms), Timing::med().at(ms));
        Tones {
            bg: mix(self.from.bg, self.to.bg, fill),
            hover: mix(self.from.hover, self.to.hover, fill),
            pressed: mix(self.from.pressed, self.to.pressed, fill),
            glow: mix(self.from.glow, self.to.glow, glow),
        }
    }

    /// Цвета кадра `now` для тона `target`; `true` — переход ещё идёт.
    fn step(&mut self, target: Tones, now: Instant, still: bool) -> (Tones, bool) {
        if target != self.to {
            self.from = self.at(now);
            self.to = target;
            self.since = Some(now);
        }
        let running = self.since.is_some_and(|since| {
            now.saturating_duration_since(since).as_secs_f64() * 1000. < Timing::med().ms
        });
        if still || !running {
            self.since = None;
        }
        (self.at(now), self.since.is_some())
    }
}

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    look: Look,
    base: Div,
    disabled: bool,
    /// `aria-pressed="true"` у `.gx-btn` (`.gv__btn` превью): заливка и рамка акцента.
    pressed: bool,
    label: Option<gpui::SharedString>,
    preview: Preview,
    on_click: Option<Handler<ClickEvent>>,
}

/// Кнопка вида `look`; содержимое — `.child(…)`, размер тоновой — у места.
pub fn button(id: impl Into<ElementId>, look: Look) -> Button {
    let base = div()
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .font_family(RUBIK);
    let base = match look {
        Look::Tone(_) => base,
        Look::Gx | Look::GxIcon => {
            let base = base
                .gap(u(7.))
                .text_size(u(12.))
                .line_height(relative(1.))
                .font_weight(FontWeight::MEDIUM)
                .whitespace_nowrap();
            if look == Look::Gx {
                base.min_w(u(30.)).h(u(30.)).px(u(11.))
            } else {
                base.size(u(34.))
            }
        }
    };
    Button {
        id: id.into(),
        look,
        base,
        disabled: false,
        pressed: false,
        label: None,
        preview: Preview::default(),
        on_click: None,
    }
}

impl Button {
    /// `:disabled`; у тоновой — `.is-busy` (DownloadBar ставит оба).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// `aria-pressed` (переключатель вида: «Вписать», «100%», «Движение»).
    pub fn pressed(mut self, pressed: bool) -> Self {
        self.pressed = pressed;
        self
    }

    /// `aria-label` (кнопка-значок).
    pub fn label(mut self, label: impl Into<gpui::SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn preview(mut self, preview: Preview) -> Self {
        self.preview = preview;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl Styled for Button {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl ParentElement for Button {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.base.extend(elements)
    }
}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let paint = self.look.paint(theme);
        let focus_color = theme.color("accent-focus");
        let on = self.pressed.then(|| {
            (
                theme.color("accent-soft"),
                theme.color("accent-border"),
                theme.color("l-text"),
            )
        });
        let zoom = zoom(window);
        let focus = focus_handle(&self.id, window, cx);
        let ring_on = self.preview.focus || focus_visible(&focus, window);
        let live = !self.disabled;
        let push = self.look.push();
        let Button {
            id,
            base,
            label,
            preview,
            on_click,
            ..
        } = self;
        let now = cx.background_executor().now();
        let still = AnimGate::still(cx).any();
        let target = Tones::of(&paint);
        let fade = window.use_keyed_state(child_id(&id, "tone"), cx, |_, _| ToneFade {
            from: target,
            to: target,
            since: None,
        });
        let (tones, moving) = fade.update(cx, |fade, _| fade.step(target, now, still));
        if moving {
            crate::motion::monitor_frame(window);
        }
        let glow_id = child_id(&id, "glow");
        let (held_id, fill_id) = (child_id(&id, "held"), child_id(&id, "fill"));
        let press_id = child_id(&id, "press");
        let inner = id.clone();
        let meta = crate::live::Meta::new("button", id.clone())
            .label(label.clone())
            .disabled(!live)
            .focused(focus.is_focused(window));
        let meta = if on.is_some() {
            meta.checked(true)
        } else {
            meta
        };
        let element = hover(id, Timing::fast(), move |t| {
            hover(glow_id, Timing::med(), move |g| {
                active(held_id, live, move |held| {
                    tween(
                        fill_id,
                        f32::from(u8::from(held)),
                        Timing::fast(),
                        move |p| {
                            let forced = |value: f32, on: bool| {
                                if !live {
                                    0.
                                } else if on {
                                    1.
                                } else {
                                    value
                                }
                            };
                            let t = forced(t, preview.hover || preview.pressed);
                            let g = forced(g, preview.hover || preview.pressed);
                            let p = forced(p, preview.pressed);
                            let bg = mix(mix(tones.bg, tones.hover, t), tones.pressed, p);
                            let mut shadows = Vec::new();
                            if live {
                                if paint.glow.is_some() {
                                    shadows.push(shadow(
                                        zoom,
                                        0.,
                                        6. + 2. * g,
                                        22. + 4. * g,
                                        0.,
                                        tones.glow,
                                    ));
                                }
                                if let Some(color) = paint.ring {
                                    shadows.push(ring(zoom, 1., color));
                                }
                            }
                            // `[aria-pressed='true']` в CSS ниже `:hover`: он и побеждает.
                            let (bg, text, border) = match on {
                                Some((bg, border, text)) => {
                                    (bg, text, paint.border.map(|_| (border, border)))
                                }
                                None => (bg, mix(paint.text, paint.hover_text, t), paint.border),
                            };
                            let body = base
                                .id(inner)
                                .role(Role::Button)
                                .button_text()
                                .when_some(label, |el, label| el.aria_label(label))
                                .rounded(u(paint.radius))
                                .bg(bg)
                                .text_color(text)
                                .shadow(shadows)
                                .when_some(border, |el, (border, over)| {
                                    el.border_1().border_color(mix(border, over, t))
                                })
                                .when(!live, |el| el.opacity(paint.disabled_opacity))
                                .when(live, |el| {
                                    el.track_focus(&focus).cursor_pointer().when_some(
                                        on_click,
                                        |el, handler| {
                                            el.on_click(move |event, window, cx| {
                                                handler(event, window, cx)
                                            })
                                        },
                                    )
                                })
                                .when(ring_on, |el| {
                                    el.child(outline(
                                        zoom,
                                        paint.radius,
                                        paint.focus_offset,
                                        2.,
                                        focus_color,
                                    ))
                                });
                            press(press_id, push, body)
                                .live(live)
                                .forced(preview.pressed)
                        },
                    )
                })
            })
        });
        crate::live::mark(meta, element)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Table;

    /// Светлая кнопка с тёмным текстом над `--l-modal-bg`: текст — цвет группы
    /// `α·текст + (1 − α)·подложка`, фон — с прозрачностью; без фона — прежняя `opacity`.
    #[test]
    fn faded_text_is_the_group_colour_over_the_window() {
        let theme = Theme::new(Table::embedded());
        let modal = gpui::Rgba::from(theme.color("l-modal-bg"));
        let (light, dark) = (rgb(0xe0e0e0), rgb(0x111214));
        let under = window_under(&theme);
        let mut el = faded(div().bg(light).text_color(dark), 0.45, under);
        let style = el.style();
        assert_eq!(style.opacity, None);
        let text = gpui::Rgba::from(style.text.color.unwrap());
        let want = 0.45 * (0x11 as f32 / 255.) + 0.55 * modal.r;
        assert!((text.r - want).abs() < 1e-3, "{} {want}", text.r);
        assert_eq!(text.a, 1.);
        let Some(gpui::Fill::Color(bg)) = &style.background else {
            panic!("нет фона");
        };
        assert!((bg.as_solid().unwrap().a - 0.45).abs() < 1e-6);
        let mut plain = faded(div().text_color(dark), 0.45, under);
        assert_eq!(plain.style().opacity, Some(0.45));
        let mut glass = faded(div().bg(light).text_color(dark), 0.45, None);
        assert_eq!(glass.style().opacity, Some(0.45));
    }
}
