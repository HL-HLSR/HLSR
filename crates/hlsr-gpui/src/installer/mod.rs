//! Окно установщика 0.4.9 (`src/installer/index.html`, `style.css`): 640 × 560 без рамки, углы и
//! контур рисует страница, как `#app` лаунчера. Шапка — логотип и кнопки окна; заголовок — значок
//! фазы в квадрате, название, чипы «Шаг N из M» и версии, описание; содержимое — шаги «Привет»
//! и SmartScreen, каталог установки, строки с переключателем («Ярлык на рабочем столе», «Также
//! удалить игры»), ход работы (проценты или бегущая полоса 1,3 с туда-обратно), ошибка или
//! предупреждение; низ — «Удалить HLSR», «Открыть Discord», «Отмена»/«Назад» и основная кнопка.
//!
//! Что показать — [`InstallerView`] контроллера `hlsr_ui::controllers::installer`; кнопки зовут
//! его методы. Размеры — CSS px страницы через [`u`] (rem окна — 16 × масштаб страницы).

pub mod icons;

use crate::kit::button::{child_id, enter_key, focus_handle, focus_visible, outline, white};
use crate::motion::{Push, Timing, hover, mix, press, tween};
use crate::paint::ring;
use crate::scale::{u, zoom};
use crate::text::{RUBIK, TextExt, keep_words, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Animation, AnimationExt, AnyElement, App, ClickEvent, Context, ElementId, FontWeight, Hsla,
    IntoElement, ObjectFit, Render, Role, SharedString, Svg, Task, TimedTransformation,
    Transformation, Window, div, img, prelude::*, px, relative, rgb, rgba, svg,
};
use hlsr_ui::controllers::installer::{InstallerController, InstallerView, Meta, Notice, Tone};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// Окно установщика: 640 × 560 CSS px (`installer-window.js`).
pub const SIZE: (f32, f32) = (640., 560.);

// Токены страницы (`body.launcher-root` style.css): акцент — тема HLSR по умолчанию.
fn accent() -> Hsla {
    rgb(0x00abff).into()
}
fn accent_soft() -> Hsla {
    rgba(0x00abff29).into()
}
fn danger() -> Hsla {
    rgb(0xff6b78).into()
}
fn danger_soft() -> Hsla {
    rgba(0xe0455a24).into()
}
fn caution() -> Hsla {
    rgb(0xe6b84a).into()
}

/// Обработчик кнопки страницы.
type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// Корень окна установщика.
pub struct InstallerRoot {
    page: Option<Arc<InstallerController>>,
    view: InstallerView,
    _views: Option<Task<()>>,
}

impl InstallerRoot {
    /// Страница над контроллером: новые виды приходят из `views` (их шлёт `ViewSink`).
    pub fn new(
        page: Arc<InstallerController>,
        mut views: tokio::sync::mpsc::UnboundedReceiver<InstallerView>,
        cx: &mut Context<Self>,
    ) -> Self {
        let view = page.view();
        let task = cx.spawn(async move |this, cx| {
            while let Some(view) = views.recv().await {
                let updated = this.update(cx, |root, cx| {
                    root.view = view;
                    cx.notify();
                });
                if updated.is_err() {
                    break;
                }
            }
        });
        InstallerRoot {
            page: Some(page),
            view,
            _views: Some(task),
        }
    }

    /// Неподвижный вид без окна установки (сцены снимков): кнопки ничего не делают.
    pub fn fixture(view: InstallerView) -> Self {
        InstallerRoot {
            page: None,
            view,
            _views: None,
        }
    }

    fn handler(&self, act: impl Fn(&Arc<InstallerController>) + 'static) -> Handler {
        let page = self.page.clone();
        Rc::new(move |_, _| {
            if let Some(page) = &page {
                act(page);
            }
        })
    }
}

impl Render for InstallerRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::theme::frame(window, cx);
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let radius = u(16.).to_pixels(window.rem_size());
        window.set_window_corner_radius(radius);
        let view = &self.view;
        let (width, height) = SIZE;
        div()
            .id("installer")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(theme.color("l-modal-bg"))
            .font_family(RUBIK)
            .text_px(14., 21.)
            .text_color(theme.color("l-text"))
            .child(window_glow(width, height))
            .child(topbar(self, view, &theme, window, cx))
            .child(hero(view, &theme, zoom))
            .child(body(self, view, &theme, zoom, window, cx))
            .child(footer(self, view, &theme, window, cx))
            // `.window::after`: контур 1 px поверх содержимого, режется `window_corner_mask` один раз.
            .child(crate::window::frame_border(radius))
    }
}

/// `radial-gradient(rx ry at cx cy, color a0, transparent 70%)` в блоке `width`×`height` со
/// скруглением `radius` (CSS px): растр SVG картинкой — у маски `svg()` GPUI поднимает альфу гаммой
/// текста, отсвет вышел бы вдвое ярче (`sponsors/modal.rs`).
fn glow(
    (width, height): (f32, f32),
    (cx, cy): (f32, f32),
    (rx, ry): (f32, f32),
    radius: f32,
    color: Hsla,
    a0: f32,
) -> impl IntoElement + use<> {
    let c = gpui::Rgba::from(color);
    let byte = |x: f32| (x * 255.).round() as u8;
    let hex = format!("#{:02x}{:02x}{:02x}", byte(c.r), byte(c.g), byte(c.b));
    let data = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{width}' height='{height}' \
         viewBox='0 0 {width} {height}'><radialGradient id='g' gradientUnits='userSpaceOnUse' \
         cx='0' cy='0' r='1' gradientTransform='matrix({rx} 0 0 {ry} {cx} {cy})'><stop \
         offset='0' stop-color='{hex}' stop-opacity='{a0}'/><stop offset='0.7' \
         stop-color='{hex}' stop-opacity='0'/></radialGradient><rect width='{width}' \
         height='{height}' rx='{radius}' fill='url(#g)'/></svg>"
    );
    img(Arc::new(gpui::Image::from_bytes(
        gpui::ImageFormat::Svg,
        data.into_bytes(),
    )))
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .object_fit(ObjectFit::Fill)
}

/// `.window`: `radial-gradient(70% 55% at 0% 0%, accent 10%, transparent 70%)`.
fn window_glow(width: f32, height: f32) -> impl IntoElement + use<> {
    glow(
        (width, height),
        (0., 0.),
        (width * 0.7, height * 0.55),
        0.,
        accent(),
        0.1,
    )
}
/// Значок `name` (`.icon`: квадрат 1em = `size` CSS px) цветом `color`.
fn icon(name: &str, size: f32, color: Hsla) -> Svg {
    svg()
        .data(icons::svg(name).unwrap_or_default())
        .flex_none()
        .size(u(size))
        .text_color(color)
}

/// `.spin`: оборот за 0,9 с.
fn spinning(icon: Svg) -> Svg {
    icon.with_timed_transformation(TimedTransformation::rotation(
        Duration::from_millis(900),
        Transformation::default(),
    ))
}

/// Нижняя или верхняя линия блока (`box-shadow: inset 0 ∓1px 0`), без места в раскладке.
fn edge_line(top: bool, color: Hsla) -> impl IntoElement + use<> {
    div()
        .absolute()
        .left_0()
        .right_0()
        .h(px(1.))
        .bg(color)
        .map(|line| if top { line.top_0() } else { line.bottom_0() })
}

fn topbar(
    root: &InstallerRoot,
    view: &InstallerView,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<InstallerRoot>,
) -> impl IntoElement + use<> {
    let brand = div()
        .flex()
        .items_center()
        .child(
            // Кольцо 2 px с точкой 12 px.
            div()
                .flex_none()
                .size(u(30.))
                .mr(u(14.))
                .rounded_full()
                .shadow(vec![ring(zoom(window), 2., white(1.))])
                .flex()
                .items_center()
                .justify_center()
                .child(div().size(u(12.)).rounded_full().bg(white(1.))),
        )
        .child(div().mr(u(-6.)).child(spaced(
            "HLSR",
            21.,
            24.,
            FontWeight::BOLD,
            8. / 28.,
            theme.color("l-text"),
        )))
        .child(div().self_start().mt(u(-2.1)).ml(u(7.6)).child(spaced(
            "BETA",
            21. * 13. / 28.,
            21. * 13. / 28.,
            FontWeight::BOLD,
            0.08,
            theme.color("l-text-dim"),
        )));
    let minimize: Handler = Rc::new(|window, _| window.minimize_window());
    let close = root.handler(|page| {
        page.close();
    });
    crate::window::drag_area(div())
        .relative()
        .flex_none()
        .flex()
        .items_center()
        .justify_between()
        .h(u(68.))
        .pt(u(4.))
        .pr(u(16.))
        .pl(u(32.))
        .child(brand)
        .child(
            div()
                .flex()
                .gap(u(4.))
                .child(control(
                    "installer-minimize",
                    "minimize",
                    &view.minimize,
                    false,
                    theme,
                    minimize,
                    window,
                    cx,
                ))
                .child(control(
                    "installer-close",
                    "cancel",
                    &view.close,
                    true,
                    theme,
                    close,
                    window,
                    cx,
                )),
        )
}

/// Кнопка окна (`.window-controls__btn`): 40 × 40, значок 15 px.
#[allow(clippy::too_many_arguments)]
fn control(
    id: &'static str,
    glyph: &'static str,
    label: &str,
    close: bool,
    theme: &Theme,
    on_click: Handler,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let element_id = ElementId::Name(id.into());
    let focus = focus_handle(&element_id, window, cx);
    let ring_on = focus_visible(&focus, window);
    let zoom = zoom(window);
    let radius = theme.len("l-radius-sm");
    let (hover_bg, active_bg) = if close {
        (
            theme.color("btn-danger-bg"),
            theme.color("btn-danger-pressed"),
        )
    } else {
        (white(0.09), white(0.14))
    };
    let label = SharedString::from(label.to_owned());
    hover(
        ElementId::Name(format!("{id}-hover").into()),
        Timing::fast(),
        move |k| {
            div()
                .id(element_id)
                .track_focus(&focus)
                .role(Role::Button)
                .aria_label(label)
                .relative()
                .occlude()
                .size(u(40.))
                .rounded(u(radius))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .bg(mix(white(0.), hover_bg, k))
                .active(move |s| s.bg(active_bg))
                .on_click(move |_, window, cx| on_click(window, cx))
                .child(icon(glyph, 15., mix(white(0.55), white(1.), k)))
                .when(ring_on, |el| {
                    el.child(outline(zoom, radius, 2., 2., accent()))
                })
        },
    )
    .into_any_element()
}

fn hero(view: &InstallerView, theme: &Theme, zoom: f32) -> impl IntoElement + use<> {
    let tone = match view.badge_tone {
        Tone::Accent | Tone::Ok => accent(),
        Tone::Danger => theme.color("btn-danger-bg"),
        Tone::Muted => theme.color("l-text-dim"),
        Tone::Caution => caution(),
    };
    let badge = div()
        .relative()
        .flex_none()
        .size(u(64.))
        .rounded(u(16.))
        .bg(white(0.02))
        .shadow(vec![ring(zoom, 1., theme.color("l-border"))])
        .flex()
        .items_center()
        .justify_center()
        .child(badge_glow(tone))
        .child(icon(view.badge_icon, 26., tone));
    let chip = |text: &str| {
        div()
            .flex()
            .items_center()
            .gap(u(8.))
            .px(u(10.))
            .py(u(4.))
            .rounded(theme.rem("l-radius-sm"))
            .bg(white(0.05))
            .text_color(theme.color("l-text-dim"))
            .text_px(12.5, 16.)
            .child(text.to_owned())
    };
    let mut chips: Vec<AnyElement> = Vec::new();
    if let Some(step) = &view.step_chip {
        chips.push(chip(step).into_any_element());
    }
    match &view.meta {
        Meta::None => {}
        Meta::Chip(text) => chips.push(chip(text).into_any_element()),
        Meta::Update { from, to } => chips.push(
            chip("")
                .child(
                    div()
                        .text_color(theme.color("l-text-faint"))
                        .child(from.clone()),
                )
                .child(icon("arrowRight", 10., theme.color("l-text-faint")))
                .child(
                    div()
                        .text_color(accent())
                        .font_weight(FontWeight::BOLD)
                        .child(to.clone()),
                )
                .into_any_element(),
        ),
    }
    div()
        .relative()
        .flex_none()
        .pt(u(12.))
        .px(u(32.))
        .pb(u(24.))
        .child(
            div().flex().items_center().gap(u(20.)).child(badge).child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(u(8.))
                    .child(spaced(
                        view.title.clone(),
                        24.,
                        32.,
                        FontWeight::BOLD,
                        0.01,
                        theme.color("l-text"),
                    ))
                    .when(!chips.is_empty(), |el| {
                        el.child(div().flex().flex_wrap().gap(u(8.)).children(chips))
                    }),
            ),
        )
        .child(
            div()
                .mt(u(16.))
                .text_color(theme.color("l-text-dim"))
                .text_px(13.5, 20.)
                .child(view.description.clone()),
        )
        .child(edge_line(false, theme.color("l-modal-border")))
}

/// Блик значка: `radial-gradient(100% 100% at 30% 20%, tone 24%, transparent 70%)`.
fn badge_glow(tone: Hsla) -> impl IntoElement + use<> {
    glow((64., 64.), (19.2, 12.8), (64., 64.), 16., tone, 0.24)
}
/// Строка (`.row`): значок в квадрате, подпись с пояснением, справа — переключатель или ничего.
fn row(
    theme: &Theme,
    zoom: f32,
    glyph: &'static str,
    label: &str,
    hint: &str,
    danger_row: bool,
) -> gpui::Div {
    let (icon_bg, icon_color) = if danger_row {
        (danger_soft(), danger())
    } else {
        (accent_soft(), accent())
    };
    div()
        .relative()
        .flex()
        .items_center()
        .gap(u(12.))
        .py(u(12.))
        .px(u(16.))
        .rounded(theme.rem("l-radius-sm"))
        .shadow(vec![ring(zoom, 1., theme.color("l-border"))])
        .child(
            div()
                .flex_none()
                .size(u(32.))
                .rounded(theme.rem("l-radius-sm"))
                .bg(icon_bg)
                .flex()
                .items_center()
                .justify_center()
                .child(icon(glyph, 14., icon_color)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_px(14., 20.)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.color("l-text"))
                        .child(label.to_owned()),
                )
                .child(
                    div()
                        .text_px(12., 16.)
                        .text_color(theme.color("l-text-faint"))
                        .map(|el| if danger_row { el } else { el.truncate() })
                        .child(keep_words(hint)),
                ),
        )
}

/// Строка с переключателем (`label.row` + `.switch`): щелчок по строке переключает, пробел — тоже.
#[allow(clippy::too_many_arguments)]
fn switch_row(
    id: &'static str,
    glyph: &'static str,
    label: &str,
    hint: &str,
    checked: bool,
    danger_row: bool,
    on_change: Rc<dyn Fn(bool)>,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let zoom = zoom(window);
    let switch_id = ElementId::Name(format!("{id}-switch").into());
    let focus = focus_handle(&switch_id, window, cx);
    let ring_on = focus_visible(&focus, window);
    let on_color = if danger_row {
        theme.color("btn-danger-bg")
    } else {
        accent()
    };
    let base = row(theme, zoom, glyph, label, hint, danger_row);
    let label = SharedString::from(label.to_owned());
    let toggle = on_change.clone();
    hover(
        ElementId::Name(format!("{id}-hover").into()),
        Timing::fast(),
        move |k| {
            // Фокус и роль — у строки (`<label>` с `<input>`): пробел переключает её щелчком.
            base.id(id)
                .track_focus(&focus)
                .role(Role::Switch)
                .aria_label(label)
                .aria_toggled(if checked {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .cursor_pointer()
                .bg(white(0.03 + 0.02 * k))
                .on_click(move |event: &ClickEvent, _, _| {
                    if !enter_key(event) {
                        toggle(!checked);
                    }
                })
                .child(tween(
                    switch_id.clone(),
                    f32::from(u8::from(checked)),
                    Timing::med(),
                    move |t| {
                        div()
                            .id(switch_id)
                            .relative()
                            .flex_none()
                            .flex()
                            .items_center()
                            .w(u(42.))
                            .h(u(24.))
                            .p(u(3.))
                            .rounded(u(12.))
                            .bg(mix(white(0.12), on_color, t))
                            .child(
                                div()
                                    .ml(u(18. * t))
                                    .size(u(18.))
                                    .rounded_full()
                                    .bg(white(1.)),
                            )
                            .when(ring_on, |el| el.child(outline(zoom, 12., 2., 2., accent())))
                    },
                ))
        },
    )
    .into_any_element()
}

fn body(
    root: &InstallerRoot,
    view: &InstallerView,
    theme: &Theme,
    zoom: f32,
    window: &mut Window,
    cx: &mut Context<InstallerRoot>,
) -> impl IntoElement + use<> {
    let text_dim = theme.color("l-text-dim");
    let welcome = view.welcome.as_ref().map(|rows| {
        let glyphs = ["download", "gamepad", "update"];
        div()
            .flex()
            .flex_col()
            .gap(u(8.))
            .children(rows.iter().zip(glyphs).map(|((label, hint), glyph)| {
                row(theme, zoom, glyph, label, hint, false).bg(white(0.03))
            }))
    });
    let callout = view.callout.as_ref().map(|callout| {
        div()
            .pt(u(14.))
            .px(u(18.))
            .pb(u(16.))
            .rounded(u(12.))
            // color-mix(in srgb, caution 7%, rgba(0,0,0,.2)).
            .bg(rgba(0x3f321441))
            .shadow(vec![ring(zoom, 2., caution())])
            .text_color(text_dim)
            .text_px(13., 19.)
            .child(div().child(callout.lead.clone()))
            .child(
                div()
                    .mt(u(8.))
                    .mb(u(6.))
                    .text_px(18., 24.)
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0xf3dca8))
                    .child(callout.title.clone()),
            )
            .child(div().mt(u(6.)).child(callout.steps.clone()))
            .child(div().mt(u(6.)).child(callout.why.clone()))
    });
    let destination =
        (!view.destination_hidden).then(|| destination(root, view, theme, zoom, window, cx));
    let desktop = (!view.desktop_hidden).then(|| {
        let page = root.page.clone();
        switch_row(
            "installer-desktop",
            "desktop",
            &view.desktop,
            &view.desktop_hint,
            view.desktop_checked,
            false,
            Rc::new(move |on| {
                if let Some(page) = &page {
                    page.set_desktop(on);
                }
            }),
            theme,
            window,
            cx,
        )
    });
    let games = view.games_shown.then(|| {
        let page = root.page.clone();
        switch_row(
            "installer-games",
            "gamepad",
            &view.games,
            &view.games_hint,
            view.games_checked,
            true,
            Rc::new(move |on| {
                if let Some(page) = &page {
                    page.set_games(on);
                }
            }),
            theme,
            window,
            cx,
        )
    });
    crate::scroll::overflow_y("installer-body", window, cx)
        .flex_1()
        .min_h_0()
        .pt(u(24.))
        .px(u(32.))
        .pb(u(16.))
        .flex()
        .flex_col()
        .gap(u(8.))
        .children(welcome)
        .children(callout)
        .children(destination)
        .children(desktop.map(|row| div().mt(u(8.)).child(row)))
        .children(games.map(|row| div().mt(u(8.)).child(row)))
        .when(view.activity_shown, |el| {
            el.child(activity(view, theme, cx.reduce_motion()))
        })
        .children(view.notice.as_ref().map(|shown| notice(shown, theme)))
}

/// Каталог установки: подпись группы и поле пути с кнопкой выбора папки.
fn destination(
    root: &InstallerRoot,
    view: &InstallerView,
    theme: &Theme,
    zoom: f32,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let browse_id = ElementId::Name("installer-browse".into());
    let focus = focus_handle(&browse_id, window, cx);
    let ring_on = focus_visible(&focus, window);
    let disabled = view.browse_disabled;
    let on_browse = root.handler(|page| page.browse());
    let dim = theme.color("l-text-dim");
    let text = theme.color("l-text");
    let border = theme.color("l-border");
    let label = SharedString::from(view.browse.clone());
    let browse = (!view.browse_hidden).then(|| {
        hover("installer-browse-hover", Timing::fast(), move |k| {
            let k = if disabled { 0. } else { k };
            div()
                .id(browse_id)
                .role(Role::Button)
                .aria_label(label)
                .relative()
                .flex_none()
                .w(u(44.))
                .flex()
                .items_center()
                .justify_center()
                .bg(white(0.08 * k))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(1.))
                        .bg(border),
                )
                .child(icon("folderOpen", 16., mix(dim, text, k)))
                .when(disabled, |el| el.opacity(0.4))
                .when(!disabled, |el| {
                    el.track_focus(&focus)
                        .cursor_pointer()
                        .active(|s| s.bg(white(0.12)))
                        .on_click(move |_, window, cx| on_browse(window, cx))
                })
                .when(ring_on, |el| el.child(outline(zoom, 0., -2., 2., accent())))
        })
    });
    div()
        .flex()
        .flex_col()
        .child(div().mb(u(8.)).child(spaced(
            view.location.to_uppercase(),
            12.,
            16.,
            FontWeight::BOLD,
            0.08,
            dim,
        )))
        .child(
            div()
                .flex()
                .items_stretch()
                .rounded(theme.rem("l-radius-sm"))
                .bg(white(0.05))
                .shadow(vec![ring(zoom, 1., border)])
                .overflow_hidden()
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .pl(u(14.))
                        .child(icon("folder", 14., theme.color("l-text-faint"))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h(u(44.))
                        .pl(u(10.))
                        .pr(u(14.))
                        .flex()
                        .items_center()
                        .text_px(13., 19.5)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(text)
                        .child(div().min_w_0().truncate().child(view.directory.clone())),
                )
                .children(browse),
        )
}

/// Ход работы: шаг и проценты, полоса (без процента — бегущая; при «уменьшить движение» отрезок
/// стоит у левого края, `animation: none` 0.4.9).
fn activity(view: &InstallerView, theme: &Theme, still: bool) -> impl IntoElement + use<> {
    let track = div()
        .relative()
        .mt(u(10.))
        .h(u(4.))
        .w_full()
        .rounded(u(2.))
        .overflow_hidden()
        .bg(white(0.08));
    let bar = match view.progress {
        Some(value) => track
            .child(
                div()
                    .h_full()
                    .w(relative(value.min(100) as f32 / 100.))
                    .rounded(u(2.))
                    .bg(accent()),
            )
            .into_any_element(),
        None => {
            let segment = div()
                .absolute()
                .top_0()
                .bottom_0()
                .w(relative(0.32))
                .rounded(u(2.))
                .bg(accent());
            if still {
                track.child(segment.left_0()).into_any_element()
            } else {
                // ease-in-out туда и обратно за 1,3 с: отрезок 32 % ездит от 0 до 68 % дорожки.
                let ease = crate::theme::bezier([0.42, 0., 0.58, 1.]);
                track
                    .child(segment.with_animation(
                        "installer-indeterminate",
                        Animation::new(Duration::from_millis(2600)).repeat(),
                        move |el, delta| {
                            let leg = if delta < 0.5 {
                                delta * 2.
                            } else {
                                2. - delta * 2.
                            };
                            el.left(relative(0.68 * ease(leg)))
                        },
                    ))
                    .into_any_element()
            }
        }
    };
    div()
        .mt(u(16.))
        .text_color(theme.color("l-text-dim"))
        .text_px(13., 20.)
        .child(
            div()
                .flex()
                .justify_between()
                .gap(u(12.))
                .child(view.step.clone())
                .child(
                    div()
                        .text_color(theme.color("l-text"))
                        .font_weight(FontWeight::BOLD)
                        .tnum()
                        .child(view.progress_text.clone()),
                ),
        )
        .child(bar)
}

/// Ошибка или предупреждение (`.notice`): значок и текст с переносами строк.
fn notice(notice: &Notice, theme: &Theme) -> impl IntoElement + use<> {
    let (bg, edge, color): (Hsla, Hsla, Hsla) = if notice.danger {
        (danger_soft(), rgba(0xff6b784d).into(), danger())
    } else {
        (
            rgba(0xe8a33d1f).into(),
            rgba(0xffb37a4d).into(),
            rgb(0xffb37a).into(),
        )
    };
    div()
        .mt(u(16.))
        .flex()
        .gap(u(10.))
        .py(u(12.))
        .px(u(14.))
        .rounded(theme.rem("l-radius-sm"))
        .bg(bg)
        .shadow(vec![ring(1., 1., edge)])
        .text_color(color)
        .text_px(12.5, 18.)
        .child(div().flex_none().pt(u(2.)).child(icon("error", 14., color)))
        .child(div().flex_1().min_w_0().child(keep_words(&notice.text)))
}

/// Вид кнопки низа.
#[derive(Clone, Copy, PartialEq)]
enum Look {
    Plain,
    Primary { danger: bool },
    GhostDanger,
}

#[allow(clippy::too_many_arguments)]
fn button(
    id: &'static str,
    look: Look,
    glyph: &'static str,
    spin: bool,
    label: &str,
    disabled: bool,
    on_click: Handler,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let element_id = ElementId::Name(id.into());
    let focus = focus_handle(&element_id, window, cx);
    let ring_on = focus_visible(&focus, window);
    let zoom = zoom(window);
    let radius = theme.len("l-radius-sm");
    let text = theme.color("l-text");
    let (bg, hover_bg, active_bg, color, hover_color) = match look {
        Look::Plain => (white(0.05), white(0.1), white(0.1), text, text),
        Look::Primary { danger: false } => {
            let ink: Hsla = rgb(0x04121c).into();
            (
                accent(),
                rgb(0x25b7ff).into(),
                rgb(0x0094dc).into(),
                ink,
                ink,
            )
        }
        Look::Primary { danger: true } => (
            theme.color("btn-danger-bg"),
            theme.color("btn-danger-hover"),
            theme.color("btn-danger-pressed"),
            white(1.),
            white(1.),
        ),
        Look::GhostDanger => (
            white(0.),
            danger_soft(),
            danger_soft(),
            rgb(0xff8f84).into(),
            rgb(0xffb3aa).into(),
        ),
    };
    let disabled_opacity = if matches!(look, Look::Primary { .. }) {
        0.6
    } else {
        0.45
    };
    let border = theme.color("l-border-strong");
    let label = label.to_owned();
    // `.spin { animation: none }` при «уменьшить движение».
    let spin = spin && !cx.reduce_motion();
    let press_id = child_id(&element_id, "press");
    hover(
        ElementId::Name(format!("{id}-hover").into()),
        Timing::fast(),
        move |k| {
            let k = if disabled { 0. } else { k };
            let color = mix(color, hover_color, k);
            let glyph = icon(glyph, 13., color);
            let el = div()
                .id(element_id)
                .role(Role::Button)
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .gap(u(8.))
                .min_h(u(48.))
                .py(u(12.))
                .px(u(if look == Look::GhostDanger { 14. } else { 20. }))
                .rounded(u(radius))
                .bg(mix(bg, hover_bg, k))
                .when(look == Look::Plain, |el| {
                    el.shadow(vec![ring(zoom, 1., border)])
                })
                .when(matches!(look, Look::Primary { .. }), |el| el.min_w(u(168.)))
                .text_px(13.5, 24.)
                .font_weight(FontWeight::BOLD)
                .whitespace_nowrap()
                .text_color(color)
                .child(if spin { spinning(glyph) } else { glyph })
                .child(label)
                .when(disabled, |el| el.opacity(disabled_opacity))
                .when(!disabled, |el| {
                    el.track_focus(&focus)
                        .cursor_pointer()
                        .active(move |s| s.bg(active_bg))
                        .on_click(move |_, window, cx| on_click(window, cx))
                })
                .when(ring_on, |el| {
                    el.child(outline(zoom, radius, 2., 2., accent()))
                });
            // `.btn:active { scale(.97) }`, `.btn:disabled { transform: none }`.
            press(press_id, Push::fast(0.97), el).live(!disabled)
        },
    )
    .into_any_element()
}

fn footer(
    root: &InstallerRoot,
    view: &InstallerView,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<InstallerRoot>,
) -> impl IntoElement + use<> {
    let uninstall = view.uninstall_shown.then(|| {
        button(
            "installer-uninstall",
            Look::GhostDanger,
            "trash",
            false,
            &view.uninstall,
            view.uninstall_disabled,
            root.handler(|page| page.uninstall()),
            theme,
            window,
            cx,
        )
    });
    let discord = view.discord.as_ref().map(|label| {
        button(
            "installer-discord",
            Look::Plain,
            "open",
            false,
            label,
            false,
            root.handler(|page| page.discord()),
            theme,
            window,
            cx,
        )
    });
    let dismiss = view.dismiss.as_ref().map(|dismiss| {
        button(
            "installer-dismiss",
            Look::Plain,
            dismiss.icon,
            false,
            &dismiss.label,
            view.dismiss_disabled,
            root.handler(|page| page.dismiss()),
            theme,
            window,
            cx,
        )
    });
    let action = button(
        "installer-action",
        Look::Primary {
            danger: view.danger_action,
        },
        view.action_icon,
        view.action_spin,
        &view.action_label,
        view.action_disabled,
        root.handler(|page| page.action()),
        theme,
        window,
        cx,
    );
    div()
        .relative()
        .flex_none()
        .flex()
        .items_center()
        .gap(u(12.))
        .py(u(16.))
        .px(u(32.))
        .children(uninstall)
        .child(div().flex_1())
        .children(discord)
        .children(dismiss)
        .child(crate::snap::target("installer-action", action))
        .child(edge_line(true, theme.color("l-modal-border")))
}
