//! Главная кнопка нижней панели (`.download-bar__play` DownloadBar.vue 0.4.9): тон по состоянию
//! `mainButton` (заливка, наведение, нажатие, свечение — набор F8 [`button`]), `is-busy` у
//! выключенной, размер — ступень сетки (`--l-play-w/-h/-font/-icon`), значок и надпись
//! заглавными с разрядкой .12em; пара «значок + надпись» по ширине доводится до кратной 8
//! (`v-grid-width:8`), подсказка — надпись.

use crate::icons::icon;
use crate::kit::{Button, Look, Tone, button};
use crate::layer::TipExt;
use crate::scale::{u, zoom, zpx};
use crate::text::{RUBIK, spaced};
use crate::theme::Theme;
use gpui::{
    App, Font, FontWeight, InteractiveElement, ParentElement, Styled, TextRun, Window, div, font,
    linear_color_stop, linear_gradient,
};
use hlsr_ui::controllers::download_bar::{ButtonView, Tone as BarTone};

/// Тон контроллера → тон набора.
fn tone(tone: BarTone) -> Tone {
    match tone {
        BarTone::Ready => Tone::Ready,
        BarTone::Progress => Tone::Progress,
        BarTone::Repair => Tone::Repair,
        BarTone::Danger => Tone::Danger,
        BarTone::Muted => Tone::Muted,
        BarTone::Neutral => Tone::Neutral,
    }
}

/// `v-grid-width:8` пары «значок 24 + зазор + надпись» (CSS px): вверх до кратной 8; разрядка —
/// как у [`spaced`], после каждого знака.
pub(crate) fn grid_pair_width(
    label: &str,
    size: f32,
    gap: f32,
    window: &mut Window,
    color: gpui::Hsla,
) -> f32 {
    let zoom = zoom(window);
    let run = TextRun {
        len: label.len(),
        font: Font {
            weight: FontWeight::BOLD,
            ..font(RUBIK)
        },
        color,
        ..Default::default()
    };
    let line = window
        .text_system()
        .layout_line(label, zpx(zoom, size), &[run], None);
    let glyphs: usize = line.runs.iter().map(|run| run.glyphs.len()).sum();
    let pair = 24. + gap + f32::from(line.width) / zoom + 0.12 * size * glyphs as f32;
    ((pair / 8. - 0.01).ceil() * 8.).max(0.)
}

/// Главная кнопка `view`; `on_press` — нажатие включённой кнопки.
pub fn play_button(
    view: &ButtonView,
    window: &mut Window,
    cx: &App,
    on_press: impl Fn(&mut Window, &mut App) + 'static,
) -> Button {
    let theme = cx.global::<Theme>();
    let look = Look::Tone(tone(view.tone));
    // --tone-text: светлые темы (Steam 2003, Полночь) ставят тёмный текст; у repair/danger свой.
    let text = match view.tone {
        BarTone::Ready => theme.color("btn-ready-text"),
        BarTone::Progress => theme.color("btn-progress-text"),
        BarTone::Repair => gpui::rgb(0x241a05).into(),
        BarTone::Danger => gpui::white(),
        BarTone::Muted => theme.color("l-text-dim"),
        BarTone::Neutral => theme.color("l-text"),
    };
    let size = theme.len("l-play-font");
    let label = view.label.to_uppercase();
    let pair = grid_pair_width(&label, size, 16., window, text);
    // Liquid Glass: «Играть» — цветное стекло, блик `linear-gradient(180deg, rgba(255,255,255,.22)
    // 0%, transparent 52%)` поверх заливки тона (и при наведении).
    let sheen = theme.liquid().then(|| {
        let white = |a: f32| gpui::white().opacity(a);
        div()
            .absolute()
            .inset_0()
            .rounded(theme.rem("l-radius"))
            .bg(linear_gradient(
                180.,
                linear_color_stop(white(0.22), 0.),
                linear_color_stop(white(0.), 0.52),
            ))
    });
    button("download-play", look)
        .disabled(!view.enabled)
        .label(view.label.clone())
        .w(theme.rem("l-play-w"))
        .h(theme.rem("l-play-h"))
        .on_click(move |_, window, cx| on_press(window, cx))
        .children(sheen)
        .child(
            div()
                .flex()
                .items_center()
                .gap(u(16.))
                .w(u(pair))
                .child(div().w(u(24.)).flex().justify_center().child(icon(
                    view.icon,
                    theme.len("l-play-icon"),
                    text,
                )))
                .child(spaced(label, size, 24., FontWeight::BOLD, 0.12, text)),
        )
        // `title`: подсказка поверх всей кнопки (мышь не перехватывает).
        .child(
            div()
                .id("download-play-title")
                .absolute()
                .inset_0()
                .tip(view.label.clone()),
        )
}
