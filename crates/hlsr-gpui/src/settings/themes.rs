//! Выбор темы лаунчера (`LauncherThemePicker.vue` 0.4.9): сетка карточек
//! `repeat(auto-fill, minmax(240px, 1fr))` с миниатюрой лаунчера в цветах темы (шапка, колонка,
//! заголовок игры, «Играть»), под ней — варианты акцента выбранной темы («Бумага»). Выбор
//! применяется сразу (событие `theme(id, accent)` → [`SettingsModal::on_theme`]).

use super::SettingsModal;
use super::sections::Snap;
use crate::icons::fa_glyph;
use crate::kit::button::white;
use crate::motion::{Timing, hover, mix};
use crate::paint::{Radial, ring};
use crate::scale::u;
use crate::text::{RUBIK, TextExt, spaced};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, ClickEvent, Div, FontWeight, Hsla, IntoElement, Rgba, Role, SharedString, div,
    prelude::*,
};
use hlsr_ui::controllers::settings::{ThemeCard, theme_cards};

/// `#RRGGBB` или `rgba(r, g, b, a)` (поверхность Liquid Glass) образца темы.
fn hex(text: &str) -> Hsla {
    let rgba = text
        .strip_prefix("rgba(")
        .and_then(|rest| rest.strip_suffix(')'))
        .and_then(|list| {
            let v: Vec<f32> = list
                .split(',')
                .filter_map(|v| v.trim().parse().ok())
                .collect();
            let &[r, g, b, a] = v.as_slice() else {
                return None;
            };
            Some(Rgba {
                r: r / 255.,
                g: g / 255.,
                b: b / 255.,
                a,
            })
        });
    rgba.or_else(|| Rgba::try_from(text).ok())
        .unwrap_or(Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a: 1.,
        })
        .into()
}

/// Сетка карточек и ряд акцентов; `width` — ширина строки, CSS px.
pub(super) fn picker(snap: &Snap, current: &str, accent: &str, width: f32) -> AnyElement {
    let mut cards = theme_cards();
    // Темы, которых нет в таблице (выключенный Liquid Glass), не показываются.
    cards.retain(|card| snap.theme.table().ids().contains(&card.id));
    let cols = ((width + 16.) / (240. + 16.)).floor().max(1.) as u16;
    // Ширина миниатюры: колонка сетки без полей карточки (12 + 12).
    let mock_w = (width - 16. * f32::from(cols - 1)) / f32::from(cols) - 24.;
    let grid = div()
        .grid()
        .grid_cols(cols)
        .gap(u(16.))
        .pb(u(8.))
        .children(cards.iter().map(|card| {
            let active = card.id == current;
            theme_card(
                snap,
                card,
                active,
                card.swatch_accent(current, accent),
                mock_w,
            )
        }));
    let accents = cards
        .iter()
        .find(|card| card.id == current && !card.accents.is_empty())
        .map(|card| accent_row(snap, card, accent));
    div()
        .flex()
        .flex_col()
        .gap(u(12.))
        .child(grid)
        .children(accents)
        .into_any_element()
}

/// Карточка `.pz-theme`: поле 12, промежуток 12, фон `--l-modal-bg`, рамка inset
/// `--l-modal-border` (наведение — `--l-border-strong`, выбранная — 2 px цветом акцента темы).
fn theme_card(
    snap: &Snap,
    card: &ThemeCard,
    active: bool,
    accent: &str,
    mock_w: f32,
) -> AnyElement {
    let theme = &snap.theme;
    let zoom = snap.zoom;
    let (bg, surface, accent) = (hex(&card.swatch[0]), hex(&card.swatch[1]), hex(accent));
    let (border, strong) = (
        theme.color("l-modal-border"),
        theme.color("l-border-strong"),
    );
    let (text, dim) = (theme.color("l-text"), theme.color("l-text-dim"));
    let (radius, radius_sm) = (theme.rem("l-radius"), theme.rem("l-radius-sm"));
    let modal_bg = theme.color("l-modal-bg");
    let (name, hint_text) = (snap.t(&card.title), snap.t(&card.hint));
    let id = card.id.clone();
    let target = SharedString::from(format!("theme-{}", card.id));
    let on_click = snap.on::<ClickEvent>(move |this: &mut SettingsModal, window, cx| {
        this.on_theme(&id, None, window, cx)
    });
    let inner = target.clone();
    crate::snap::target(
        target.clone(),
        hover(target, Timing::fast(), move |t| {
            let frame = if active {
                ring(zoom, 2., accent)
            } else {
                ring(zoom, 1., mix(border, strong, t))
            };
            div()
                .id(inner.clone())
                .role(Role::RadioButton)
                .button_text()
                .aria_label(name.clone())
                .flex()
                .flex_col()
                .gap(u(12.))
                .p(u(12.))
                .rounded(radius)
                .bg(modal_bg)
                .shadow(vec![frame])
                .font_family(RUBIK)
                .text_color(text)
                .cursor_pointer()
                .on_click(on_click)
                .child(mock(zoom, mock_w, bg, surface, accent, radius_sm))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(u(4.))
                        .px(u(4.))
                        .pb(u(4.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_px(15., 20.)
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(name.clone())
                                .when(active, |el| {
                                    el.child(fa_glyph("fas fa-check", 13.).text_color(accent))
                                }),
                        )
                        .child(
                            div()
                                .text_px(12., 16.)
                                .text_color(dim)
                                .child(hint_text.clone()),
                        ),
                )
        }),
    )
}

/// `.pz-theme__mock`: 128 px, фон темы и пятно акцента 22 % справа снизу; шапка 20 px
/// (поверхность 80 %), колонка 20 px (60 %) с тремя точками, две строки заголовка, «Играть».
fn mock(zoom: f32, width: f32, bg: Hsla, surface: Hsla, accent: Hsla, radius: gpui::Rems) -> Div {
    let dot = |color: Hsla| div().size(u(8.)).rounded_full().bg(color);
    let tab = |color: Hsla| div().w(u(22.)).h(u(4.)).rounded(u(2.)).bg(color);
    div()
        .relative()
        .h(u(128.))
        .w_full()
        .rounded(radius)
        .overflow_hidden()
        .bg(bg)
        .child(
            div().absolute().inset_0().child(
                Radial {
                    center: (0.85, 1.),
                    radius: (1.2, 0.9),
                    stops: vec![(0., 0.22), (0.6, 0.)],
                }
                .paint(width, 128., accent),
            ),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(u(20.))
                .flex()
                .items_center()
                .gap(u(6.))
                .px(u(8.))
                .bg(surface.opacity(0.8))
                .child(
                    div()
                        // Кольцо `0 0 0 1.5px #fff` снаружи точки 8 px: в GPUI внешняя тень
                        // заливает и середину, поэтому круг 11 px с кольцом внутрь.
                        .size(u(11.))
                        .ml(u(-1.5))
                        .mr(u(4.5))
                        .rounded_full()
                        .shadow(vec![ring(zoom, 1.5, gpui::white())]),
                )
                .child(tab(accent))
                .child(tab(white(0.28))),
        )
        .child(
            div()
                .absolute()
                .top(u(20.))
                .bottom_0()
                .left_0()
                .w(u(20.))
                .flex()
                .flex_col()
                .items_center()
                .gap(u(6.))
                .pt(u(8.))
                .bg(surface.opacity(0.6))
                .child(dot(accent))
                .child(dot(white(0.22)))
                .child(dot(white(0.22))),
        )
        .child(
            div()
                .absolute()
                .left(u(32.))
                .top(u(44.))
                .flex()
                .flex_col()
                .gap(u(6.))
                .child(div().w(u(96.)).h(u(9.)).rounded(u(3.)).bg(white(0.85)))
                .child(div().w(u(56.)).h(u(5.)).rounded(u(3.)).bg(white(0.5))),
        )
        .child(
            div()
                .absolute()
                .right(u(12.))
                .bottom(u(12.))
                .w(u(64.))
                .h(u(18.))
                .rounded(u(4.))
                .bg(accent),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded(radius)
                .shadow(vec![ring(zoom, 1., white(0.06))]),
        )
}

/// `.pz-accents`: подпись и кнопки вариантов (выбранная — заливка 14 % и рамка 60 % цвета).
fn accent_row(snap: &Snap, card: &ThemeCard, current: &str) -> AnyElement {
    let theme = &snap.theme;
    let zoom = snap.zoom;
    let chosen = card
        .accent(current)
        .map(|a| a.id.clone())
        .unwrap_or_default();
    let (border, strong) = (
        theme.color("l-modal-border"),
        theme.color("l-border-strong"),
    );
    let (text, dim) = (theme.color("l-text"), theme.color("l-text-dim"));
    let field = theme.color("l-field-bg");
    let radius = theme.rem("l-radius-sm");
    let buttons = card.accents.iter().map(|item| {
        let active = item.id == chosen;
        let color = hex(&item.color);
        let label = snap.t(&item.title);
        let (theme_id, accent_id) = (card.id.clone(), item.id.clone());
        let on_click = snap.on::<ClickEvent>(move |this: &mut SettingsModal, window, cx| {
            this.on_theme(&theme_id, Some(&accent_id), window, cx)
        });
        let target = SharedString::from(format!("accent-{}", item.id));
        let inner = target.clone();
        crate::snap::target(
            target.clone(),
            hover(target, Timing::fast(), move |t| {
                let (bg, frame, ink) = if active {
                    (color.opacity(0.14), color.opacity(0.6), text)
                } else {
                    (field, mix(border, strong, t), mix(dim, text, t))
                };
                div()
                    .id(inner.clone())
                    .role(Role::RadioButton)
                    .button_text()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .h(u(32.))
                    .px(u(12.))
                    .rounded(radius)
                    .bg(bg)
                    .shadow(vec![ring(zoom, 1., frame)])
                    .font_family(RUBIK)
                    .text_px(13., 16.)
                    .text_color(ink)
                    .cursor_pointer()
                    .on_click(on_click)
                    .child(div().size(u(8.)).rounded(u(2.)).bg(color))
                    .child(label.clone())
            }),
        )
    });
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(u(8.))
        .child(div().mr(u(8.)).child(spaced(
            snap.t("#L_THEME_PAPER_ACCENT").to_uppercase(),
            12.,
            16.,
            FontWeight::SEMIBOLD,
            0.1,
            dim,
        )))
        .children(buttons)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swatch_colors_parse_hex_and_rgba() {
        let glass = Rgba::from(hex("rgba(255, 255, 255, 0.18)"));
        assert_eq!((glass.r, glass.g, glass.b), (1., 1., 1.));
        assert!((glass.a - 0.18).abs() < 1e-6);
        let accent = Rgba::from(hex("#0A84FF"));
        assert!((accent.b - 1.).abs() < 1e-6 && accent.a == 1.);
        assert_eq!(Rgba::from(hex("rgba(1, 2)")).a, 1., "непонятное — чёрный");
    }
}
