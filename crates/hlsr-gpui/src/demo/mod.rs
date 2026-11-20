//! «Демо» (`DemoPanel.vue` 0.4.9 и его части): страница (G11), плеер (G12b), окно экспорта
//! (G13). Общее оформление страницы — кнопки `.dmp__btn`, `.dmp__icon-btn`, ссылки, счётчики.

pub mod card;
pub mod drop;
pub mod export_modal;
pub mod ffmpeg_card;
pub mod list;
pub mod panel;
pub mod player;
pub mod screens;
pub mod timeline;
pub mod view_settings;

#[cfg(test)]
mod tests;

use crate::icons::icon;
use crate::kit::button::{child_id, focus_handle, focus_visible, outline, white};
use crate::layer::TipExt;
use crate::motion::{Push, Timing, hover, mix, press};
use crate::paint::shadow;
use crate::scale::{u, zoom};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, Hsla, IntoElement, Role, SharedString,
    Window, div, prelude::*,
};
use std::rc::Rc;

/// Остановка Tab у элемента страницы (кнопки `<button>` 0.4.9): фокус по id и виден ли контур
/// `:focus-visible` — `outline: 2px solid` со смещением `offset` и скруглением `radius`.
pub struct Focus {
    pub handle: gpui::FocusHandle,
    pub ring: Option<gpui::Div>,
}

pub fn focus(id: &ElementId, radius: f32, offset: f32, window: &mut Window, cx: &mut App) -> Focus {
    let handle = focus_handle(id, window, cx);
    let ring = focus_visible(&handle, window).then(|| {
        let color = cx.global::<Theme>().color("l-accent");
        outline(zoom(window), radius, offset, 2., color)
    });
    Focus { handle, ring }
}

/// Щелчок по элементу страницы.
pub type OnClick = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// Вид `.dmp__btn`.
#[derive(Clone, Copy)]
pub struct BtnLook {
    /// `.primary`: заливка акцентом, свечение при наведении.
    pub primary: bool,
    /// `min-height` (40; у шапки карточки — 32, в низком окне — 30).
    pub height: f32,
    /// Поля по бокам (16; у шапки карточки — 14).
    pub px: f32,
    /// Нажатие `scale(.97)` `--l-fast` (`.dmp__btn`, `.dex__btn`; у `.dff__btn` его нет).
    pub push: bool,
}

impl BtnLook {
    pub const PLAIN: BtnLook = BtnLook {
        primary: false,
        height: 40.,
        px: 16.,
        push: true,
    };
}

/// Кнопка `.dmp__btn` (и `.dff__btn` с другой высотой): значок и подпись, рамка
/// `--l-border-strong`, белый .05 → .1 при наведении; `primary` — `--accent`; недоступная — .45.
#[allow(clippy::too_many_arguments)]
pub fn btn(
    id: impl Into<ElementId>,
    look: BtnLook,
    disabled: bool,
    title: Option<String>,
    children: Vec<AnyElement>,
    window: &mut Window,
    cx: &mut App,
    on_click: OnClick,
) -> AnyElement {
    let id = id.into();
    let zoom = zoom(window);
    let focus = focus_handle(&id, window, cx);
    let ring_on = focus_visible(&focus, window);
    let theme = cx.global::<Theme>();
    let radius = theme.len("l-radius-sm");
    let accent = theme.color("l-accent");
    let (bg, hot, text) = if look.primary {
        (
            theme.color("accent"),
            theme.color("accent-hover"),
            theme.color("accent-contrast-text"),
        )
    } else {
        (white(0.05), white(0.1), theme.color("l-text"))
    };
    let border = if look.primary {
        gpui::transparent_black()
    } else {
        theme.color("l-border-strong")
    };
    let glow = accent.opacity(0.35);
    let meta = crate::live::Meta::new("button", id.clone())
        .label(title.clone().map(SharedString::from))
        .disabled(disabled)
        .focused(focus.is_focused(window));
    let inner = id.clone();
    let press_id = child_id(&id, "press");
    let element = hover(id, Timing::fast(), move |t| {
        let t = if disabled { 0. } else { t };
        let el = div()
            .id(inner)
            .role(Role::Button)
            .button_text()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(u(8.))
            .min_h(u(look.height))
            .px(u(look.px))
            .border_1()
            .border_color(border)
            .rounded(u(radius))
            .bg(mix(bg, hot, t))
            .when(look.primary && t > 0., |el| {
                el.shadow(vec![shadow(zoom, 0., 6., 20., 0., glow.opacity(0.35 * t))])
            })
            .text_color(text)
            .text_px(13., 20.)
            .font_weight(FontWeight::SEMIBOLD)
            .whitespace_nowrap()
            .tnum()
            .children(children)
            .when_some(title, |el, title| el.tip(title))
            .when(disabled, |el| el.opacity(0.45))
            .when(!disabled, |el| {
                el.track_focus(&focus)
                    .cursor_pointer()
                    .on_click(move |event, window, cx| on_click(event, window, cx))
            })
            .when(ring_on, |el| {
                el.child(outline(zoom, radius, 2., 2., accent))
            });
        press(press_id, Push::fast(0.97), el).live(look.push && !disabled)
    });
    crate::live::mark(meta, element)
}

/// Значок 12 px перед подписью кнопки (`i { font-size: 12px }`); `fa-spin` — кольцо.
pub fn btn_icon(classes: &str, color: Hsla) -> AnyElement {
    icon(classes, 12., color)
}

/// Кнопка-значок `.dmp__icon-btn`: `size` × `size` (40; у шапки карточки — 32 или 30), рамка
/// `--l-border`, значок `--l-text-dim` → `--l-text`, подложка .06 при наведении. Подпись — в
/// подсказке. `faded` — недоступная бледнее (.45, `.dmp__head-icon`).
#[allow(clippy::too_many_arguments)]
pub fn icon_btn(
    id: impl Into<ElementId>,
    classes: &'static str,
    size: f32,
    label: String,
    disabled: bool,
    faded: bool,
    window: &mut Window,
    cx: &mut App,
    on_click: OnClick,
) -> AnyElement {
    let id = id.into();
    let zoom = zoom(window);
    let focus = focus_handle(&id, window, cx);
    let ring_on = focus_visible(&focus, window);
    let theme = cx.global::<Theme>();
    let (dim, text, border, accent) = (
        theme.color("l-text-dim"),
        theme.color("l-text"),
        theme.color("l-border"),
        theme.color("l-accent"),
    );
    let radius = theme.len("l-radius-sm");
    let meta = crate::live::Meta::new("button", id.clone())
        .label(Some(SharedString::from(label.clone())))
        .disabled(disabled)
        .focused(focus.is_focused(window));
    let inner = id.clone();
    let element = hover(id, Timing::fast(), move |t| {
        let t = if disabled { 0. } else { t };
        div()
            .id(inner)
            .role(Role::Button)
            .button_text()
            .aria_label(SharedString::from(label.clone()))
            .tip(label)
            .relative()
            .size(u(size))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .border_1()
            .border_color(border)
            .rounded(u(radius))
            .bg(white(0.06 * t))
            .child(icon(classes, 14., mix(dim, text, t)))
            .when(disabled && faded, |el| el.opacity(0.45))
            .when(!disabled, |el| {
                el.track_focus(&focus)
                    .cursor_pointer()
                    .on_click(move |event, window, cx| on_click(event, window, cx))
            })
            .when(ring_on, |el| {
                el.child(outline(zoom, radius, 2., 2., accent))
            })
    });
    crate::live::mark(meta, element)
}

/// Ссылка-кнопка `.dmp__link`: подчёркнутый текст `--l-text-dim`, при наведении — `--l-text`.
pub fn link(
    id: impl Into<ElementId>,
    text: String,
    window: &mut Window,
    cx: &mut App,
    on_click: OnClick,
) -> AnyElement {
    let id = id.into();
    let theme = cx.global::<Theme>();
    let (dim, bright) = (theme.color("l-text-dim"), theme.color("l-text"));
    let Focus { handle, ring } = focus(&id, 2., 2., window, cx);
    let meta = crate::live::Meta::new("link", id.clone())
        .label(Some(SharedString::from(text.clone())))
        .focused(handle.is_focused(window));
    let inner = id.clone();
    let element = hover(id, Timing::fast(), move |t| {
        div()
            .id(inner)
            .role(Role::Button)
            .button_text()
            .relative()
            .track_focus(&handle)
            .children(ring)
            .flex_none()
            .cursor_pointer()
            .underline()
            .text_color(mix(dim, bright, t))
            .whitespace_nowrap()
            .child(text)
            .on_click(move |event, window, cx| on_click(event, window, cx))
    });
    crate::live::mark(meta, element)
}

/// Счётчик `.dmp__group-count`: 11 px / 600, подложка .07, `--l-text-faint`.
pub fn count(value: usize, theme: &Theme) -> AnyElement {
    div()
        .flex_none()
        .px(u(7.))
        .py(u(1.))
        .rounded(u(9.))
        .bg(white(0.07))
        .text_color(theme.color("l-text-faint"))
        .text_px(11., 20.)
        .font_weight(FontWeight::SEMIBOLD)
        .tnum()
        .child(value.to_string())
        .into_any_element()
}

/// Обработчик щелчка, который зовёт метод страницы.
pub fn act(
    panel: &gpui::WeakEntity<panel::DemoPanel>,
    f: impl Fn(&mut panel::DemoPanel, &mut Window, &mut gpui::Context<panel::DemoPanel>) + 'static,
) -> OnClick {
    let panel = panel.clone();
    Rc::new(move |_, window, cx| {
        panel
            .update(cx, |panel, cx| {
                f(panel, window, cx);
                cx.notify();
            })
            .ok();
    })
}
