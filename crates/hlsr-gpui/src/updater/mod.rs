//! Окно внешнего апдейтера (`src/updater/window`, без изменений с 0.4.7): окно с рамкой ОС,
//! «нативная программа Windows» — системный шрифт, плоские поверхности, светлая или тёмная тема
//! ОС. Знак HLSR, заголовок и целевая версия, стадия, полоса (бегущая 1,4 с, зелёная, красная),
//! байты и скорость, пояснение, код ошибки, одна кнопка («Отменить»/«Закрыть»), Esc — она же.
//!
//! Что показать — [`UpdaterPage`] (`hlsr_ui::controllers::updater`); события ядра приходят
//! правками страницы ([`Edit`]), нажатие отдаёт [`UpdaterCommand`].

use crate::kit::button::{focus_handle, focus_visible, outline};
use crate::paint::ring;
use crate::scale::{u, zoom};
use crate::text::TextExt;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Animation, AnimationExt, Context, ElementId, FontWeight, Hsla, IntoElement, KeyDownEvent,
    Render, Role, Task, Window, WindowAppearance, div, prelude::*, px, relative, rgb,
};
use hlsr_ui::controllers::updater::{FillClass, UpdaterCommand, UpdaterPage};
use std::rc::Rc;
use std::time::Duration;

/// Клиентская область окна 460 × 280 (`updater/main.js`, внешний размер с рамкой Windows 11).
pub const CLIENT: (f32, f32) = (444., 241.);

/// Правка страницы событием ядра (`updater:init`/`stage`/`progress`) из любого потока.
pub type Edit = Box<dyn FnOnce(&mut UpdaterPage) + Send>;

/// `body` и `.upd__*` updater.css в светлой и тёмной теме.
struct Palette {
    bg: Hsla,
    text: Hsla,
    dim: Hsla,
    track: Hsla,
    button: Hsla,
    button_hover: Hsla,
    button_active: Hsla,
    button_border: Hsla,
}

impl Palette {
    fn of(dark: bool) -> Palette {
        let c = |hex: u32| -> Hsla { rgb(hex).into() };
        if dark {
            Palette {
                bg: c(0x202020),
                text: c(0xf0f0f0),
                dim: c(0xa0a0a0),
                track: c(0x3a3a3a),
                button: c(0x2d2d2d),
                button_hover: c(0x363636),
                button_active: c(0x292929),
                button_border: c(0x4a4a4a),
            }
        } else {
            Palette {
                bg: c(0xf3f3f3),
                text: c(0x1a1a1a),
                dim: c(0x616161),
                track: c(0xd1d1d1),
                button: c(0xfdfdfd),
                button_hover: c(0xf5f5f5),
                button_active: c(0xebebeb),
                button_border: c(0xd1d1d1),
            }
        }
    }
}

const BLUE: u32 = 0x0f6cbd;
/// `"Segoe UI Variable Text", "Segoe UI", system-ui`.
const FONT: &str = "Segoe UI Variable Text";

/// Корень окна апдейтера.
pub struct UpdaterRoot {
    page: UpdaterPage,
    on_command: Rc<dyn Fn(UpdaterCommand)>,
    /// Тема снимка; `None` — как у ОС (`prefers-color-scheme`).
    dark: Option<bool>,
    _edits: Option<Task<()>>,
}

impl UpdaterRoot {
    /// Окно над ядром: правки страницы — из `edits`, нажатия — в `on_command`.
    pub fn new(
        mut edits: tokio::sync::mpsc::UnboundedReceiver<Edit>,
        on_command: Rc<dyn Fn(UpdaterCommand)>,
        cx: &mut Context<Self>,
    ) -> Self {
        let task = cx.spawn(async move |this, cx| {
            while let Some(edit) = edits.recv().await {
                let updated = this.update(cx, |root, cx| {
                    edit(&mut root.page);
                    cx.notify();
                });
                if updated.is_err() {
                    break;
                }
            }
        });
        UpdaterRoot {
            page: UpdaterPage::new(),
            on_command,
            dark: None,
            _edits: Some(task),
        }
    }

    /// Неподвижная страница (сцены снимков).
    pub fn fixture(page: UpdaterPage, dark: bool) -> Self {
        UpdaterRoot {
            page,
            on_command: Rc::new(|_| {}),
            dark: Some(dark),
            _edits: None,
        }
    }

    fn press(&mut self, escape: bool, cx: &mut Context<Self>) {
        let command = if escape {
            self.page.escape()
        } else {
            self.page.click()
        };
        if let Some(command) = command {
            (self.on_command)(command);
        }
        cx.notify();
    }
}

impl Render for UpdaterRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = self.dark.unwrap_or(matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ));
        let palette = Palette::of(dark);
        let view = self.page.view().clone();
        let zoom = zoom(window);
        let fill = view.fill;
        let fill_color: Hsla = match fill.class {
            FillClass::Done => rgb(0x0f7b0f).into(),
            FillClass::Failed => rgb(0xc42b1c).into(),
            _ => rgb(BLUE).into(),
        };
        let bar = div()
            .absolute()
            .top_0()
            .bottom_0()
            .rounded(u(3.))
            .bg(fill_color)
            .w(relative(fill.width() / 100.));
        let bar = if fill.running() && !cx.reduce_motion() {
            // `upd-slide`: от −100 % до 320 % своей ширины (35 %) за 1,4 с, ease-in-out.
            let ease = crate::theme::bezier([0.42, 0., 0.58, 1.]);
            bar.with_animation(
                "updater-slide",
                Animation::new(Duration::from_millis(1400)).repeat(),
                move |el, delta| el.left(relative(0.35 * (-1. + 4.2 * ease(delta)))),
            )
            .into_any_element()
        } else if fill.running() {
            bar.left_0().w_full().into_any_element()
        } else {
            bar.left_0().into_any_element()
        };
        let button_id = ElementId::Name("updater-action".into());
        let focus = focus_handle(&button_id, window, cx);
        let ring_on = focus_visible(&focus, window);
        let button = view.action_shown.then(|| {
            div()
                .id(button_id)
                .track_focus(&focus)
                .role(Role::Button)
                .relative()
                .min_w(u(96.))
                .h(u(30.))
                .px(u(14.))
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(palette.button_border)
                .rounded(u(4.))
                .bg(palette.button)
                .hover(|s| s.bg(palette.button_hover))
                .active(|s| s.bg(palette.button_active))
                .cursor_pointer()
                .text_px(13., 17.3)
                .child(view.action_label.clone())
                .on_click(cx.listener(|root, _, _, cx| root.press(false, cx)))
                .when(ring_on, |el| {
                    el.child(outline(zoom, 4., 1., 2., rgb(BLUE).into()))
                })
        });
        div()
            .id("updater")
            .size_full()
            .flex()
            .flex_col()
            .pt(u(18.))
            .px(u(20.))
            .pb(u(16.))
            .bg(palette.bg)
            .text_color(palette.text)
            .font_family(FONT)
            .text_px(13., 17.3)
            .on_key_down(cx.listener(|root, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    root.press(true, cx);
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(12.))
                    .child(
                        div()
                            .flex_none()
                            .size(u(30.))
                            .rounded_full()
                            .shadow(vec![ring(zoom, 3., rgb(BLUE).into())])
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(div().size(u(12.)).rounded_full().bg(rgb(BLUE))),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .child(
                                div()
                                    .text_px(15., 18.)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(view.title.clone()),
                            )
                            .child(
                                div()
                                    .mt(u(2.))
                                    .text_px(12., 16.)
                                    .text_color(palette.dim)
                                    .child(view.subtitle.clone()),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(u(8.))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(view.stage.clone()),
                    )
                    .child(
                        div()
                            .relative()
                            .h(u(6.))
                            .rounded(u(3.))
                            .overflow_hidden()
                            .bg(palette.track)
                            .child(bar),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .min_h(u(16.))
                            .text_px(12., 16.)
                            .text_color(palette.dim)
                            .tnum()
                            .child(view.bytes.clone())
                            .child(view.speed.clone()),
                    )
                    .child(
                        div()
                            .min_h(u(16.))
                            .text_px(12., 16.)
                            .text_color(palette.dim)
                            .child(view.note.clone()),
                    )
                    .child(
                        div()
                            .min_h(u(14.))
                            .font_family("Consolas")
                            .text_px(11., 13.)
                            .text_color(rgb(0x8a8a8a))
                            .child(view.code.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .pt(u(12.))
                    .min_h(px(0.))
                    .children(button),
            )
    }
}
