//! Общие окна оболочки (S14): подтверждение ([`confirm`]; его карточку со слотом берут и окна
//! с содержимым — ворота FFmpeg, ремонт), список изменений, выбор папки библиотеки,
//! приветствие. Логика — `hlsr_ui::controllers::{shell, shell_windows, welcome, shell_flows}`.

pub mod changelog;
pub mod confirm;
pub mod library_setup;
#[cfg(test)]
mod tests;
pub mod welcome;

use crate::kit::Handler;
use crate::kit::button::{child_id, faded, focus_handle, focus_visible, white, window_under};
use crate::kit::outline;
use crate::motion::{Push, Timing, hover, mix, press};
use crate::scale::{u, zoom};
use crate::text::{RUBIK, TextExt};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ClickEvent, ElementId, FocusHandle, FontWeight, Hsla, IntoElement, RenderOnce, Rgba,
    SharedString, Window, div, prelude::*, rgb,
};
use std::rc::Rc;

/// Вид кнопки окна (`.confirm__btn`, `.library-setup__btn`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    /// Фон .05, рамка `--l-border-strong`; при наведении .1.
    Plain,
    /// `.primary` подтверждения: `--accent`, при наведении `brightness(1.08)`.
    Primary,
    /// `.primary.danger`: #c0392f, белый текст.
    Danger,
    /// `.library-setup__btn.primary`: при наведении `--accent-hover`.
    Accent,
}

/// `filter: brightness(k)`.
fn brightness(color: Hsla, k: f32) -> Hsla {
    let c = Rgba::from(color);
    Rgba {
        r: (c.r * k).min(1.),
        g: (c.g * k).min(1.),
        b: (c.b * k).min(1.),
        a: c.a,
    }
    .into()
}

/// Кнопка окна: `flex: 1`, поля 11 × 18, рамка 1 px, 13,5 px / 600, переход `--l-fast`;
/// `:disabled` — .5; нажатие — `scale(.97)` `--l-fast` (`.confirm__btn`, `.library-setup__btn`,
/// `.ls-launch__btn`, `.thanks__btn`).
#[derive(IntoElement)]
pub struct DialogButton {
    id: ElementId,
    label: SharedString,
    kind: Btn,
    disabled: bool,
    focus: Option<FocusHandle>,
    /// Контур фокуса без клавиатуры (фокус из скрипта до первого щелчка, см. [opened_by_script]).
    ring: bool,
    on_click: Option<Handler<ClickEvent>>,
}

pub fn btn(id: impl Into<ElementId>, label: impl Into<SharedString>, kind: Btn) -> DialogButton {
    DialogButton {
        id: id.into(),
        label: label.into(),
        kind,
        disabled: false,
        focus: None,
        ring: false,
        on_click: None,
    }
}

impl DialogButton {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Свой фокус: на него окно ставит фокус при открытии (`$refs.cancelButton.focus()`).
    pub fn focus(mut self, handle: &FocusHandle) -> Self {
        self.focus = Some(handle.clone());
        self
    }

    pub fn ring(mut self, ring: bool) -> Self {
        self.ring = ring;
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

impl RenderOnce for DialogButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let (accent, accent_hover) = (theme.color("accent"), theme.color("accent-hover"));
        let (strong, text) = (theme.color("l-border-strong"), theme.color("l-text"));
        let contrast = theme.color("accent-contrast-text");
        let focus_color = theme.color("accent-focus");
        let radius = theme.len("l-radius-sm");
        let under = window_under(theme);
        let zoom = zoom(window);
        let DialogButton {
            id,
            label,
            kind,
            disabled,
            focus,
            ring,
            on_click,
        } = self;
        let focus = focus.unwrap_or_else(|| focus_handle(&id, window, cx));
        let ring_on = (ring && focus.is_focused(window)) || focus_visible(&focus, window);
        let meta = crate::live::Meta::new("button", id.clone())
            .label(Some(label.clone()))
            .disabled(disabled)
            .focused(focus.is_focused(window));
        let inner = id.clone();
        let press_id = child_id(&id, "press");
        let element = hover(id, Timing::fast(), move |t| {
            let t = if disabled { 0. } else { t };
            let clear = gpui::transparent_black();
            let (bg, border, color) = match kind {
                Btn::Plain => (mix(white(0.05), white(0.1), t), strong, text),
                Btn::Primary => (mix(accent, brightness(accent, 1.08), t), clear, contrast),
                Btn::Danger => {
                    let red: Hsla = rgb(0xc0392f).into();
                    (mix(red, brightness(red, 1.08), t), clear, gpui::white())
                }
                Btn::Accent => (mix(accent, accent_hover, t), clear, contrast),
            };
            let el = div()
                .id(inner)
                .relative()
                .flex_1()
                .flex()
                .justify_center()
                .py(u(11.))
                .px(u(18.))
                .border_1()
                .border_color(border)
                .rounded(u(radius))
                .bg(bg)
                .font_family(RUBIK)
                .text_px(13.5, 16.)
                .font_weight(FontWeight::BOLD)
                .text_color(color)
                .child(label)
                .when(disabled, |el| faded(el, 0.5, under))
                .when(!disabled, |el| {
                    el.track_focus(&focus)
                        .cursor_pointer()
                        .when_some(on_click, |el, handler| {
                            el.on_click(move |event, window, cx| handler(event, window, cx))
                        })
                })
                .when(ring_on, |el| {
                    el.child(outline(zoom, radius, 2., 2., focus_color))
                });
            press(press_id, Push::fast(0.97), el).live(!disabled)
        });
        crate::live::mark(meta, element)
    }
}

thread_local! {
    /// В окне уже нажимали мышью (поток окна).
    static POINTER_DOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Слушатель нажатий мыши окна (слушать можно только при рисовании): один на окно, в слоте
/// подтверждения; нажатие запоминается до конца процесса.
pub(crate) fn pointer_watch() -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        |_, _, window, _| {
            window.on_mouse_event(|_: &gpui::MouseDownEvent, phase, _, _| {
                if phase == gpui::DispatchPhase::Capture {
                    POINTER_DOWN.with(|seen| seen.set(true));
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size(gpui::px(0.))
}

/// Фокус, который окно ставит из скрипта при открытии, Chromium показывает контуром
/// (`:focus-visible`), пока в окне не нажимали мышью: так окно, открытое при запуске (папка
/// библиотеки, приветствие), сразу видно с клавиатуры. GPUI считает начальный ввод мышью.
pub(crate) fn opened_by_script() -> bool {
    !POINTER_DOWN.with(std::cell::Cell::get)
}
