//! Просмотр картинок новости (`NewsImageViewer.vue` 0.4.9): окно до 1180 px по центру, в шапке
//! заголовок поста, «N / M» и «×»; картинка по ширине окна, не выше `100vh − 182px`
//! (`object-fit: contain`), не загрузилась — «Не удалось загрузить»; «‹ Назад» / «Вперёд ›»
//! при нескольких. ←/→ листают, Esc и подложка закрывают, Tab ходит по кнопкам окна, фокус при
//! открытии — на «×». Подложку и стопку окон даёт [`crate::layer::Modal`] (подложка .88 и
//! размытие 8 px того, что под ней, — [`crate::layer::Modal::blur`]).

use super::panel::{HomePanel, SCOPE};
use crate::image::device_size;
use crate::kit::button::{focus_handle, focus_visible, outline, white};
use crate::motion::{Timing, hover};
use crate::scale::{u, viewport, zoom};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, ClickEvent, Context, ElementId, FocusHandle, Hsla, IntoElement, KeyDownEvent,
    ObjectFit, ParentElement, Role, Styled, Window, div, img, prelude::*, rgb,
};
use hlsr_ui::images::{Fit, ImageRequest, ImageSource};

/// Шапка и низ окна с полями `.news-viewer` — столько высоты окна не под картинку.
const CHROME: f32 = 182.;

pub(super) fn content(
    panel: &mut HomePanel,
    window: &mut Window,
    cx: &mut Context<HomePanel>,
) -> AnyElement {
    let Some(viewer) = panel.state.viewer().cloned() else {
        return div().into_any_element();
    };
    let theme = cx.global::<Theme>().clone();
    let tr = HomePanel::tr(cx);
    let (vw, vh) = viewport(window);
    // width: min(1180px, 100%) внутри полей 28; рамка 1 px внутри ширины.
    let width = (vw - 56.).clamp(0., 1180.);
    let inner = (width - 2.).max(0.);
    let max_h = (vh - CHROME).max(0.);
    let count = viewer.images.len();
    let index = viewer.index.min(count.saturating_sub(1));
    let url = viewer.images.get(index).cloned().unwrap_or_default();
    let photo = ImageSource::parse(&url).map(|source| {
        let (w, h) = device_size(window, inner, max_h);
        let request = ImageRequest::new(source, SCOPE).with_target(w, h, Fit::Contain);
        panel.pictures.get(request, window, cx)
    });
    let frame = match photo {
        Some(Some(Ok(photo))) => {
            let size = photo.size(0);
            let aspect = size.height.0.max(1) as f32 / size.width.0.max(1) as f32;
            let height = (inner * aspect).min(max_h);
            img(photo)
                .w(u(inner))
                .h(u(height))
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        }
        // Не загрузилась (или адрес не тот): `@error` → «Не удалось загрузить».
        Some(Some(Err(_))) | None => div()
            .p(u(48.))
            .child(tr("#L_HOME_IMAGE_FAILED", &[]))
            .into_any_element(),
        // Грузится: у `<img>` без картинки высоты нет.
        Some(None) => div().into_any_element(),
    };

    let close = viewer_button(
        "news-viewer-close",
        Some(panel.viewer_close.clone()),
        false,
        "×",
        &theme,
        window,
        cx,
        |panel, window, cx| panel.close_image(window, cx),
    );
    let header = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(u(20.))
        .px(u(16.))
        .py(u(12.))
        .text_px(13., 20.)
        .child(div().flex_1().min_w_0().child(viewer.title.clone()))
        .child(format!("{} / {}", index + 1, count))
        .child(div().text_px(24., 24.).child(close));

    let nav = (count > 1).then(|| {
        let prev = viewer_button(
            "news-viewer-prev",
            None,
            index == 0,
            format!("‹ {}", tr("#L_HOME_IMAGE_PREV", &[])),
            &theme,
            window,
            cx,
            |panel, _, cx| {
                panel.state.move_image(-1);
                cx.notify();
            },
        );
        let next = viewer_button(
            "news-viewer-next",
            None,
            index + 1 >= count,
            format!("{} ›", tr("#L_HOME_IMAGE_NEXT", &[])),
            &theme,
            window,
            cx,
            |panel, _, cx| {
                panel.state.move_image(1);
                cx.notify();
            },
        );
        div()
            .flex()
            .flex_none()
            .justify_between()
            .gap(u(12.))
            .px(u(16.))
            .py(u(12.))
            .child(prev)
            .child(next)
    });

    let on_key = cx.listener(|panel, event: &KeyDownEvent, _, cx| {
        let delta = match event.keystroke.key.as_str() {
            "left" => -1,
            "right" => 1,
            _ => return,
        };
        cx.stop_propagation();
        panel.state.move_image(delta);
        cx.notify();
    });
    div()
        .id("news-viewer")
        .role(Role::Dialog)
        .aria_label(viewer.title.clone())
        .flex()
        .flex_col()
        .w(u(width))
        .max_h(u((vh - 56.).max(0.)))
        .min_h_0()
        .border_1()
        .border_color(theme.color("l-border-strong"))
        .rounded(u(10.))
        .bg(theme.color("l-surface-solid"))
        .overflow_hidden()
        // Шрифт кнопок — от корня лаунчера: 14 px, `line-height: normal` Rubik (1,185).
        .text_px(14., 14. * 1.185)
        .text_color(theme.color("l-text"))
        .on_key_down(on_key)
        .child(header)
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .min_h_0()
                .bg(rgb(0x0b0e11))
                .child(frame),
        )
        .children(nav)
        .into_any_element()
}

/// Кнопка окна: поля 6/12, рамка `--l-border`, радиус 5, наведение .08, `:disabled` .35.
#[allow(clippy::too_many_arguments)]
fn viewer_button(
    name: &'static str,
    focus: Option<FocusHandle>,
    disabled: bool,
    label: impl Into<gpui::SharedString>,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<HomePanel>,
    on_click: impl Fn(&mut HomePanel, &mut Window, &mut Context<HomePanel>) + 'static,
) -> AnyElement {
    let id = ElementId::from(name);
    let focus = focus.unwrap_or_else(|| focus_handle(&id, window, cx));
    let ring_on = focus_visible(&focus, window);
    let zoom = zoom(window);
    let (border, accent): (Hsla, Hsla) = (theme.color("l-border"), theme.color("l-accent"));
    let label = label.into();
    let on_click =
        cx.listener(move |panel, _: &ClickEvent, window, cx| on_click(panel, window, cx));
    let inner = id.clone();
    let button = hover(id, Timing::fast(), move |t| {
        let t = if disabled { 0. } else { t };
        div()
            .id(inner)
            .role(Role::Button)
            .button_text()
            .relative()
            .px(u(12.))
            .py(u(6.))
            .border_1()
            .border_color(border)
            .rounded(u(5.))
            .bg(white(0.08 * t))
            .child(label)
            .when(disabled, |el| el.opacity(0.35))
            .when(!disabled, |el| {
                el.track_focus(&focus).cursor_pointer().on_click(on_click)
            })
            .when(ring_on, |el| el.child(outline(zoom, 5., 2., 2., accent)))
    });
    crate::snap::target(name, button)
}
