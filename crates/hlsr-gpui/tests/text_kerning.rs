//! Кернинг DirectWrite (патч `vendor/gpui-pre-windows`, `apply_font_features`): с типографикой GPUI
//! DirectWrite сам кернинг не включал, а Chromium 0.4.9 кернит всегда. Настоящая система текста
//! Windows, без окон (платформа GPUI держит только скрытое служебное окно).
#![cfg(windows)]

use gpui::{Font, FontFeatures, Platform, TextRun, TextSystem, WindowTextSystem, font, px};
use hlsr_gpui::text::{BODY_SPACING, RUBIK};
use std::borrow::Cow;
use std::sync::Arc;

fn text_system() -> WindowTextSystem {
    let platform = gpui_windows::WindowsPlatform::new(false).expect("платформа GPUI");
    let system = Arc::new(TextSystem::new(platform.text_system()));
    system
        .add_fonts(vec![Cow::Borrowed(include_bytes!(
            "../../hlsr-ui/fonts/Rubik-Regular.ttf"
        ))])
        .expect("Rubik");
    WindowTextSystem::new(system)
}

fn run(text: &str, font: Font, spacing: f32) -> TextRun {
    TextRun {
        len: text.len(),
        font,
        letter_spacing: px(spacing),
        ..Default::default()
    }
}

#[test]
fn kerning_is_on_like_chromium() {
    let system = text_system();
    let width = |text: &str, font: Font| {
        f32::from(
            system
                .layout_line(text, px(14.), &[run(text, font, 0.)], None)
                .width,
        )
    };
    let no_kern = Font {
        features: FontFeatures(Arc::new(vec![("kern".into(), 0)])),
        ..font(RUBIK)
    };
    // Chromium 45 (Electron 0.4.9), Rubik 14 px: «AV» 17,81, без кернинга 18,5; «ко» 15,14 и 15,48.
    for (text, chromium) in [("AV", 17.8125), ("ко", 15.140625)] {
        let kerned = width(text, font(RUBIK));
        assert!((kerned - chromium).abs() < 0.02, "{text}: {kerned}");
        assert!(kerned < width(text, no_kern.clone()) - 0.3, "{text}");
    }

    // «Спонсорство» (#L_SPONSORS_ABOUT, 14/22, max-width 620): первая строка с разрядкой body —
    // 619,69 у Chromium, «или» остаётся на ней. Без кернинга было 620,03 и «или» уходило вниз.
    let about = "Поддержите лаунчер HLSR ежемесячной подпиской по цене пары пачек «Кириешек» или единоразовым донатом! Так вы сможете поддержать нашу работу. Спасибо! <3";
    let lines = system
        .shape_text(
            about.into(),
            px(14.),
            &[run(about, font(RUBIK), BODY_SPACING)],
            Some(px(620.)),
            None,
        )
        .unwrap();
    let line = &lines[0];
    let wrap = &line.wrap_boundaries[0];
    let at = line.unwrapped_layout.runs[wrap.run_ix].glyphs[wrap.glyph_ix].index;
    assert!(about[..at].ends_with("» или "), "{}", &about[..at]);
}
