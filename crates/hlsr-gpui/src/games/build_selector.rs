//! «Версия сборки» над главной кнопкой (`BuildSelector.vue` 0.4.9): карточка шириной с «Играть»
//! (`--l-play-w`, `--l-surface-solid`, рамка тенью — при наведении и фокусе с оттенком акцента),
//! кнопка списка на всю ширину (подпись, значение заглавными, стрелка в плашке), список вверх у
//! правого края — [`dropdown`] набора: значок в плашке (`fa-code-branch`, у «скоро» —
//! `fa-hourglass-half`), подпись, справа размер нескачанного эпизода (`badges`). Шестерёнки
//! «Настройки игры» в 0.4.9 нет. Пункты — [`build_selector::items`].

use crate::icons::{fa_glyph, fa_turned};
use crate::kit::{Handler, Item, ToggleState, dropdown};
use crate::layer::TipExt;
use crate::liquid;
use crate::motion::{Timing, hover, mix, tween};
use crate::paint::ring;
use crate::scale::{u, zoom};
use crate::text::spaced;
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, FontWeight, Hsla, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window,
    div, hsla, prelude::*,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::build_selector;
use hlsr_ui::tr;

/// Выбор сборки: `on_select` — id выбранного пункта (`select-build`).
#[derive(IntoElement)]
pub struct BuildSelector {
    pub lang: Language,
    pub builds: &'static [&'static str],
    pub upcoming: &'static [&'static str],
    pub badges: Vec<(&'static str, String)>,
    pub value: String,
    pub on_select: Handler<str>,
}

fn white(alpha: f32) -> Hsla {
    hsla(0., 0., 1., alpha)
}

impl RenderOnce for BuildSelector {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let value = build_selector::effective_value(self.builds, &self.value);
        let items: Vec<Item> =
            build_selector::items(self.lang, self.builds, self.upcoming, &self.badges)
                .into_iter()
                .map(|item| Item {
                    id: item.id.into(),
                    label: item.label.into(),
                    hint: (!item.hint.is_empty()).then(|| item.hint.into()),
                    badge: (!item.badge.is_empty()).then(|| item.badge.into()),
                    disabled: item.disabled,
                    separated: item.separated,
                })
                .collect();
        let heading = tr(self.lang, "#L_BUILD_VERSION", &[]);
        let border = theme.color("l-border");
        let accent = theme.color("l-accent");
        // color-mix(in srgb, var(--l-accent) 35%, var(--l-border)).
        let lit_border = mix(border, accent, 0.35);
        let (faint, dim, text) = (
            theme.color("l-text-faint"),
            theme.color("l-text-dim"),
            theme.color("l-text"),
        );
        let soft = theme.color("l-accent-soft");
        let (build_h, radius) = (theme.rem("l-build-h"), theme.rem("l-radius"));
        let caption = heading.to_uppercase();
        let shown = value.to_uppercase();
        let title: SharedString = heading.clone().into();
        let on_select = self.on_select.clone();
        let item_dim = dim;
        let item_faint = faint;
        let list = dropdown("build-select", items, Some(value.clone().into()))
            .up()
            .align_right()
            .heading(heading.clone())
            .label(heading)
            .toggle(move |state: ToggleState| {
                hover("build-toggle", Timing::fast(), move |t| {
                    let t = if state.open { 1. } else { t };
                    let lit = t.max(f32::from(u8::from(state.focus)));
                    div()
                        .id("build-toggle-face")
                        .flex_1()
                        .min_w_0()
                        .h(build_h)
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(u(12.))
                        .pl(u(16.))
                        .pr(u(12.))
                        .rounded(radius)
                        .bg(white(0.04 * t))
                        .when(state.focus, |el| el.shadow(vec![ring(zoom, 1., accent)]))
                        .tip(title.clone())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .items_start()
                                .gap(u(2.))
                                .child(spaced(
                                    caption.clone(),
                                    11.,
                                    16.,
                                    FontWeight::BOLD,
                                    0.1,
                                    mix(faint, accent, t),
                                ))
                                .child(div().max_w_full().overflow_hidden().child(spaced(
                                    shown.clone(),
                                    15.,
                                    20.,
                                    FontWeight::BOLD,
                                    0.05,
                                    text,
                                ))),
                        )
                        .child(
                            div()
                                .flex_none()
                                .size(u(24.))
                                .rounded(u(6.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(mix(white(0.04), soft, lit))
                                .shadow(vec![ring(zoom, 1., border.opacity(1. - lit))])
                                // `.open .build-panel__chev > i { transform: rotate(180deg) }`,
                                // `transition: transform var(--l-dur) var(--l-ease)`.
                                .child(tween(
                                    "build-chev",
                                    f32::from(u8::from(state.open)),
                                    Timing::med(),
                                    move |turn| {
                                        fa_turned(
                                            "fas fa-chevron-up",
                                            10.,
                                            180. * turn,
                                            mix(dim, accent, lit),
                                        )
                                    },
                                )),
                        )
                })
            })
            .item_view(move |item: &Item, lit: f32| {
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(u(12.))
                    .child(
                        div()
                            .flex_none()
                            .size(u(28.))
                            .ml(u(-4.))
                            .rounded(u(6.))
                            .flex()
                            .items_center()
                            .justify_center()
                            // Выбранный, под курсором и с клавиатуры (`:global(.ui-dropdown__item…
                            // .build-panel__item-icon)`): плашка `--l-accent-soft`, кольцо
                            // прозрачное, значок `--l-accent`. У «скоро» кольцо `--l-border` остаётся:
                            // `.soon` меняет только фон и цвет, а `lit` у него всегда 0 (наведение
                            // `:not(:disabled)`, клавиши его пропускают, выбранной бывает лишь сборка).
                            .shadow(vec![ring(zoom, 1., border.opacity(1. - lit))])
                            .when(!item.disabled, |el| {
                                el.bg(mix(white(0.04), soft, lit))
                                    .text_color(mix(item_dim, accent, lit))
                            })
                            .when(item.disabled, |el| el.text_color(item_faint))
                            .child(fa_glyph(
                                if item.disabled {
                                    "fas fa-hourglass-half"
                                } else {
                                    "fas fa-code-branch"
                                },
                                12.,
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(item.label.clone()),
                    )
            })
            .on_select(move |item, window, cx| on_select(&item.id, window, cx));

        hover("build-panel", Timing::fast(), move |t| {
            let (solid, edge) = (
                theme.color("l-surface-solid"),
                ring(zoom, 1., mix(border, lit_border, t)),
            );
            let panel = div()
                .flex()
                .items_stretch()
                .w(theme.rem("l-play-w"))
                .bg(solid)
                .rounded(radius)
                .shadow(vec![edge.clone()]);
            // Liquid Glass: `.build-panel` — мягкое стекло над кадром.
            liquid::soft(panel, solid, radius, vec![edge], &theme)
                // `.ui-dropdown { flex: 1 }`: колонка тянет кнопку списка на всю ширину.
                .child(div().flex_1().min_w_0().flex().flex_col().child(list))
        })
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Значок пункта: у «скоро» — песочные часы без плашки (`.soon`), у сборки — ветка в плашке.
    #[test]
    fn upcoming_builds_have_their_own_icon() {
        assert!(crate::icons::fa("fas fa-hourglass-half").is_some());
        assert!(crate::icons::fa("fas fa-code-branch").is_some());
        assert!(crate::icons::fa("fas fa-chevron-up").is_some());
    }
}
