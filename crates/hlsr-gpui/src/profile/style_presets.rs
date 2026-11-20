//! Выбор заготовки «подсветка ника + рамка» (`StylePresetPicker.vue` 0.4.9) во вкладке
//! «Настройки» профиля: сетка-радио (`repeat(auto-fill, minmax(208px, 1fr))`) — в карточке свой
//! аватар в рамке и ник в стиле заготовки (те же [`crate::common`], которыми профиль рисуется у
//! других), «Без подсветки» первой; без права — сердце, пояснение, «Снять» и «Поддержать»
//! (окно спонсорства). Сверху — пояснение и строка подписки, снизу — сообщение. Логика —
//! `controllers::runner_settings::style_presets`.

use super::badges::SCOPE;
use super::runner_settings::{RunnerSettingsTab, message};
use crate::common::{avatar_frame, nick_name};
use crate::icons::{fa_glyph, spinner};
use crate::image::{Pictures, device_size};
use crate::kit::button::{child_id, focus_handle, focus_visible, outline, white};
use crate::motion::{Push, Timing, hover, mix, press};
use crate::paint::{Radial, ring, shadow};
use crate::scale::{u, zoom};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, Hsla, IntoElement, ObjectFit, RenderImage, Role,
    Toggled, Window, div, img, prelude::*, rgb,
};
use hlsr_ui::controllers::bridge::Tr;
use hlsr_ui::controllers::runner_settings::style_presets::{Card, Mode, PickerView};
use hlsr_ui::images::{Fit, ImageRequest, ImageSource};
use std::sync::Arc;

/// `--sponsor`: розовый окна «Спонсорство» и сердечка в шапке.
const SPONSOR: u32 = 0xdb61a2;

fn sponsor(alpha: f32) -> Hsla {
    Hsla::from(rgb(SPONSOR)).opacity(alpha)
}

/// Строка подписки (`.style-picker__sub`, у активной — розовая).
fn subscription(view: &PickerView, theme: &Theme, zoom: f32) -> gpui::Div {
    let active = view.subscriber_active;
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(u(6.))
        .px(u(10.))
        .py(u(2.))
        .rounded_full()
        .bg(if active { sponsor(0.12) } else { white(0.05) })
        .when(active, |el| el.shadow(vec![ring(zoom, 1., sponsor(0.35))]))
        .text_px(12., 20.)
        .font_weight(FontWeight::MEDIUM)
        .text_color(if active {
            rgb(SPONSOR).into()
        } else {
            theme.color("l-text-dim")
        })
        .whitespace_nowrap()
        .child(fa_glyph("fas fa-star", 10.))
        .child(view.subscriber_text.clone())
}

/// Блок целиком; `width` — ширина раздела (для числа колонок сетки).
pub(super) fn picker(
    view: &PickerView,
    width: f32,
    tr: &Tr,
    window: &mut Window,
    cx: &mut Context<RunnerSettingsTab>,
) -> AnyElement {
    let theme = cx.global::<Theme>().clone();
    let zoom = zoom(window);
    let t = |key: &str| tr(key, &[]);
    let head = (view.mode == Mode::Grid
        || (view.mode == Mode::None && !view.subscriber_text.is_empty()))
    .then(|| {
        div()
            .mb(u(12.))
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_x(u(16.))
            .gap_y(u(8.))
            .when(view.mode == Mode::Grid, |el| {
                el.child(
                    div()
                        .text_px(12., 20.)
                        .text_color(theme.color("l-text-faint"))
                        .child(t("#L_RUNNER_STYLE_HINT")),
                )
            })
            .when(!view.subscriber_text.is_empty(), |el| {
                el.child(subscription(view, &theme, zoom))
            })
    });
    let content = match view.mode {
        Mode::Grid => Some(grid(view, width, &theme, window, cx)),
        Mode::Locked => Some(locked(view, width, tr, &theme, window, cx)),
        Mode::None => None,
    };
    let note =
        (!view.message.is_empty()).then(|| message(&view.message, view.message_error).mt(u(12.)));
    crate::snap::target(
        "rs-style",
        div()
            .relative()
            .pt(u(12.))
            .pb(u(16.))
            .children(head)
            .children(content)
            .children(note)
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(u(1.))
                    .bg(white(0.045)),
            ),
    )
}

/// `repeat(auto-fill, minmax(208px, 1fr))` с промежутком 8: сколько колонок в `width`.
fn columns(width: f32) -> u16 {
    (((width + 8.) / 216.).floor() as u16).max(1)
}

/// Сетка-радио: «Без подсветки» и заготовки.
fn grid(
    view: &PickerView,
    width: f32,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<RunnerSettingsTab>,
) -> AnyElement {
    // Фото одно на все карточки: разбор `data:` копирует адрес целиком — не на каждую.
    let photo = photo(view, window, cx);
    let cards: Vec<AnyElement> = view
        .cards
        .iter()
        .enumerate()
        .map(|(index, card)| option(index, card, view, photo.clone(), theme, window, cx))
        .collect();
    div()
        .id("rs-style-grid")
        .role(Role::RadioGroup)
        .grid()
        .grid_cols(columns(width))
        .gap(u(8.))
        .children(cards)
        .into_any_element()
}

/// Фото аккаунта 36×36 (`None` — ещё грузится или нет).
fn photo(
    view: &PickerView,
    window: &mut Window,
    cx: &mut Context<RunnerSettingsTab>,
) -> Option<Arc<RenderImage>> {
    let source = ImageSource::parse(&view.avatar)?;
    let pictures = cx.try_global::<Pictures>()?.clone();
    let (w, h) = device_size(window, 36., 36.);
    let request = ImageRequest::new(source, SCOPE).with_target(w, h, Fit::Cover);
    pictures.get(request, window, cx).and_then(Result::ok)
}

/// Аватар 36 (`.style-picker__avatar`): картинка или значок Steam.
fn avatar(photo: Option<Arc<RenderImage>>, theme: &Theme) -> gpui::Div {
    div()
        .size(u(36.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .overflow_hidden()
        .bg(white(0.06))
        .text_color(theme.color("l-text-dim"))
        .map(|el| match photo {
            Some(photo) => el.child(
                img(photo)
                    .size_full()
                    .rounded_full()
                    .object_fit(ObjectFit::Cover),
            ),
            None => el.child(fa_glyph("fab fa-steam", 16.)),
        })
}

/// Карточка `.style-picker__option`.
fn option(
    index: usize,
    card: &Card,
    view: &PickerView,
    photo: Option<Arc<RenderImage>>,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<RunnerSettingsTab>,
) -> AnyElement {
    let zoom = zoom(window);
    let id = gpui::ElementId::from(("rs-style-option", index));
    let focus = focus_handle(&id, window, cx);
    let ring_on = focus_visible(&focus, window);
    let radius = theme.len("l-radius-sm");
    let accent = theme.color("accent");
    let focus_color = theme.color("accent-focus");
    let border = theme.color("l-border");
    let (text, faint, dim) = (
        theme.color("l-text"),
        theme.color("l-text-faint"),
        theme.color("l-text-dim"),
    );
    let contrast = theme.color("accent-contrast-text");
    let selected = card.selected;
    let pending = card.pending;
    let busy = view.busy;
    let choice = card.id;
    let click = cx.listener(move |this, _: &ClickEvent, _, cx| {
        let effects = this.state().choose_style(choice);
        this.run(effects, cx);
    });
    let face = avatar(photo, theme);
    let frame = avatar_frame(("rs-style-frame", index), 36., face)
        .css(card.frame_css.clone())
        .width(3.);
    let nick = div()
        .min_w_0()
        .overflow_hidden()
        .text_px(14., 20.)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(text)
        .whitespace_nowrap()
        .child(
            nick_name(("rs-style-nick", index), view.name.clone())
                .look("", card.nick_css.clone())
                .subscriber(view.subscriber_badge, None),
        );
    let title = card.title.clone();
    let mark = if pending {
        Some(spinner(12., dim).into_any_element())
    } else if selected {
        Some(fa_glyph("fas fa-check", 9.).into_any_element())
    } else {
        None
    };
    let inner = id.clone();
    let press_id = child_id(&id, "press");
    let card = hover(id, Timing::fast(), move |t| {
        let base = if selected {
            mix(white(0.), accent, 0.08)
        } else {
            white(0.03)
        };
        let el = div()
            .id(inner)
            .role(Role::RadioButton)
            .button_text()
            .aria_toggled(if selected {
                Toggled::True
            } else {
                Toggled::False
            })
            .aria_label(title.clone())
            .track_focus(&focus)
            .relative()
            .flex()
            .flex_col()
            .items_start()
            .gap(u(10.))
            .min_w_0()
            .px(u(14.))
            .py(u(12.))
            .rounded(u(radius))
            .bg(mix(base, white(0.06), if selected { 0. } else { t }))
            .shadow(vec![if selected {
                ring(zoom, 2., accent)
            } else {
                ring(zoom, 1., border)
            }])
            .text_color(text)
            // `cursor: progress` у GPUI нет: пока сохраняется — обычная стрелка.
            .when(!busy, |el| el.cursor_pointer())
            .on_click(click)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(12.))
                    .w_full()
                    .min_w_0()
                    .pr(u(20.))
                    .child(frame)
                    .child(nick),
            )
            .child(
                div()
                    .max_w_full()
                    .truncate()
                    .text_px(12., 16.)
                    .text_color(faint)
                    .child(title),
            )
            .children(mark.map(|mark| {
                div()
                    .absolute()
                    .top(u(10.))
                    .right(u(10.))
                    .size(u(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .when(selected && !pending, |el| {
                        el.bg(accent).text_color(contrast)
                    })
                    .child(mark)
            }))
            .when(ring_on, |el| {
                el.child(outline(zoom, radius, 2., 2., focus_color))
            });
        // `:active:not([aria-disabled="true"])`: пока стиль сохраняется — без нажатия.
        press(press_id, Push::fast(0.98), el).live(!busy)
    });
    crate::snap::target(format!("rs-style-option-{index}"), card)
}

/// Без права: сердце, пояснение (и строка подписки), «Снять» и «Поддержать».
fn locked(
    view: &PickerView,
    width: f32,
    tr: &Tr,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<RunnerSettingsTab>,
) -> AnyElement {
    let t = |key: &str| tr(key, &[]);
    let zoom = zoom(window);
    let radius = theme.len("l-radius-sm");
    let remove = view.removable.then(|| {
        let click = cx.listener(|this, _: &ClickEvent, _, cx| {
            let effects = this.state().choose_style(0.);
            this.run(effects, cx);
        });
        picker_btn(
            "rs-style-remove",
            false,
            view.busy,
            t("#L_RUNNER_STYLE_REMOVE"),
            click,
            theme,
            window,
            cx,
        )
    });
    // `$emit("open-sponsors")`: оболочка закрывает профиль и открывает «Спонсорство».
    let support = picker_btn(
        "rs-style-support",
        true,
        false,
        t("#L_RUNNER_STYLE_SUPPORT"),
        |_, _, cx| crate::sponsors::modal::open_sponsors(cx),
        theme,
        window,
        cx,
    );
    let wash = Radial {
        center: (0., 0.),
        radius: (1.2, 1.6),
        stops: vec![(0., 1.), (0.7, 0.)],
    };
    let ring_color = mix(theme.color("l-border"), rgb(SPONSOR).into(), 0.25);
    div()
        .relative()
        .flex()
        .items_center()
        .gap(u(16.))
        .p(u(16.))
        .rounded(u(radius))
        .overflow_hidden()
        .bg(white(0.03))
        .shadow(vec![ring(zoom, 1., ring_color)])
        .child(
            // Растр держит пропорции SVG: высота блока — поля, заголовок и пояснение в две строки
            // (окно не уже 1152), плюс строка подписки.
            // shortcut: высота по расчёту, а не по раскладке; при трёх строках пояснения
            // отсвет кончится выше — тогда мерить блок (`canvas`).
            wash.paint(
                width,
                if view.subscriber_text.is_empty() {
                    100.
                } else {
                    128.
                },
                sponsor(0.1),
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .child(
            div()
                .size(u(40.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(sponsor(0.16))
                .shadow(vec![ring(zoom, 1., sponsor(0.4))])
                .text_color(rgb(SPONSOR))
                .child(fa_glyph("fas fa-heart", 15.)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .gap(u(4.))
                .min_w_0()
                .child(
                    div()
                        .text_px(14., 24.)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.color("l-text"))
                        .child(t("#L_RUNNER_STYLE_PERK_REQUIRED")),
                )
                .child(
                    div()
                        .text_px(12., 20.)
                        .text_color(theme.color("l-text-faint"))
                        .child(t("#L_RUNNER_STYLE_LOCKED_HINT")),
                )
                .when(!view.subscriber_text.is_empty(), |el| {
                    el.child(
                        div()
                            .mt(u(4.))
                            .flex()
                            .child(subscription(view, theme, zoom)),
                    )
                }),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .flex_wrap()
                .justify_end()
                .gap(u(8.))
                .children(remove)
                .child(support),
        )
        .into_any_element()
}

/// Кнопка `.style-picker__btn`: розовая «Поддержать» со свечением или прозрачная «Снять».
#[allow(clippy::too_many_arguments)]
fn picker_btn(
    id: &'static str,
    support: bool,
    disabled: bool,
    label: String,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<RunnerSettingsTab>,
) -> AnyElement {
    let zoom = zoom(window);
    let focus = focus_handle(&id.into(), window, cx);
    let ring_on = focus_visible(&focus, window);
    let radius = theme.len("l-radius-sm");
    let focus_color = theme.color("accent-focus");
    let (dim, text) = (theme.color("l-text-dim"), theme.color("l-text"));
    let press_id = child_id(&id.into(), "press");
    let button = hover(id, Timing::fast(), move |t| {
        let t = if disabled { 0. } else { t };
        let el = div()
            .id(id)
            .role(Role::Button)
            .button_text()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(u(8.))
            .min_h(u(40.))
            .px(u(16.))
            .py(u(8.))
            .rounded(u(radius))
            .text_px(13., 24.)
            .font_weight(FontWeight::SEMIBOLD)
            .whitespace_nowrap()
            .map(|el| {
                if support {
                    // `filter: brightness(1.08)` при наведении.
                    let bright = mix(rgb(SPONSOR).into(), gpui::white(), 0.08 * t);
                    el.bg(bright)
                        .shadow(vec![shadow(zoom, 0., 6., 22., 0., sponsor(0.32))])
                        .text_color(gpui::white())
                        .child(fa_glyph("fas fa-heart", 12.))
                } else {
                    el.bg(white(0.06 * t)).text_color(mix(dim, text, t))
                }
            })
            .child(label)
            .when(disabled, |el| el.opacity(0.45))
            .when(!disabled, |el| {
                el.track_focus(&focus).cursor_pointer().on_click(on_click)
            })
            .when(ring_on, |el| {
                el.child(outline(zoom, radius, 2., 2., focus_color))
            });
        press(press_id, Push::fast(0.97), el).live(!disabled)
    });
    crate::snap::target(id, button)
}

#[cfg(test)]
mod tests {
    use super::columns;

    #[test]
    fn auto_fill_columns_follow_208_plus_gap() {
        assert_eq!(columns(976.), 4, "окно 1040: четыре колонки по 238");
        assert_eq!(columns(640.), 3);
        assert_eq!(columns(424.), 2);
        assert_eq!(columns(100.), 1);
    }
}
