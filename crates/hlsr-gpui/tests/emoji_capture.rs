//! Эмодзи реакций на настоящем окне GPUI (патч `vendor/gpui-pre-windows`, `direct_write.rs`):
//! подстановка к Rubik даёт системный Segoe UI Emoji, глифы COLR v1 рисуются с градиентами, как в
//! Chromium 0.4.9, а не плоскими слоями COLR v0. Снимок кладётся в `target/snap/emoji/` для сверки с
//! Chromium (`emoji.html` той же раскладки).
//!
//! Окно открывается только на невидимом столе, поэтому тест `#[ignore]`:
//! `python scripts/slint/headless-win.py cargo test -p hlsr-gpui --features dev-hooks --test
//! emoji_capture -- --ignored`. Без `HLSR_HEADLESS_DESKTOP` тест падает, а не открывает окно.
#![cfg(all(windows, feature = "dev-hooks"))]

use gpui::{
    App, AppContext, Context, IntoElement, ParentElement, Render, Styled, Window, WindowBounds,
    WindowOptions, div, point, px, rgb, size,
};
use hlsr_gpui::text::RUBIK;
use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

/// Реакции 0.4.9 (`news-contract`) строками: верх строки и размер шрифта, px.
const ROWS: [(f32, f32); 3] = [(8., 12.), (32., 24.), (72., 48.)];
const EMOJI: &str = "👍❤️🔥😮🎉";

struct View;

impl Render for View {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x1b1d22))
            .font_family(RUBIK)
            .text_color(rgb(0xffffff))
            .children(ROWS.map(|(top, size)| {
                div()
                    .absolute()
                    .left(px(0.))
                    .top(px(top))
                    .text_size(px(size))
                    .line_height(px(size))
                    .whitespace_nowrap()
                    .child(EMOJI)
            }))
    }
}

#[test]
#[ignore = "окно GPUI: только под scripts/slint/headless-win.py, с -- --ignored"]
fn reaction_emoji_have_colr_v1_gradients() {
    assert!(
        std::env::var_os("HLSR_HEADLESS_DESKTOP").is_some(),
        "emoji_capture: только под scripts/slint/headless-win.py"
    );
    let shot = Rc::new(RefCell::new(None));
    let platform = gpui_windows::WindowsPlatform::new(false).expect("платформа GPUI");
    let run_shot = shot.clone();
    gpui::Application::with_platform(Rc::new(platform)).run(move |cx: &mut App| {
        cx.text_system()
            .add_fonts(vec![Cow::Borrowed(include_bytes!(
                "../../hlsr-ui/fonts/Rubik-Regular.ttf"
            ))])
            .expect("Rubik");
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(gpui::Bounds::new(
                point(px(0.), px(0.)),
                size(px(300.), px(140.)),
            ))),
            ..Default::default()
        };
        let window = cx
            .open_window(options, |_, cx| cx.new(|_| View))
            .expect("окно");
        let shot = run_shot.clone();
        cx.spawn(async move |cx| {
            // Первые кадры окна — без атласа глифов.
            cx.background_executor()
                .timer(Duration::from_millis(800))
                .await;
            window
                .update(cx, |_, window, _| {
                    let image = window.render_to_image().expect("кадр");
                    *shot.borrow_mut() = Some((window.scale_factor(), image));
                })
                .ok();
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    let (scale, image) = shot.borrow_mut().take().expect("снимка нет");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snap/emoji");
    std::fs::create_dir_all(&dir).expect("папка снимков");
    let path = dir.join(format!("gpui-x{scale}.png"));
    image.save(&path).expect("снимок");
    eprintln!("снимок {}", path.display());

    // Градиент — соседние пиксели отличаются на 1–6 уровней. У плоских слоёв таких пар почти нет:
    // заливка ровная, края сглаживания — скачки крупнее.
    let (top, bottom) = ((72. * scale) as u32, (120. * scale) as u32);
    let mut steps = 0;
    for y in top..bottom.min(image.height()) {
        for x in 1..image.width() {
            let (a, b) = (image.get_pixel(x - 1, y).0, image.get_pixel(x, y).0);
            let diff = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap();
            steps += usize::from((1..=6).contains(&diff));
        }
    }
    let area = ((bottom - top) * image.width()) as usize;
    let share = steps as f32 / area as f32 * 100.;
    eprintln!("плавных шагов {steps} на {area} пикселей ({share:.2} %)");
    // Та же мера по снимкам Chromium 0.4.9 (Skia, COLR v1) `emoji.html` в Edge headless:
    // x1 — 25,22 %, x1,25 — 28,16 %. GPUI x1 дал 24,66 %; не меньше 90 % от Chromium.
    let chromium = match scale {
        1. => 25.22,
        1.25 => 28.16,
        _ => panic!("нет замера Chromium для масштаба {scale}"),
    };
    assert!(
        share >= chromium * 0.9,
        "эмодзи без градиентов: {share:.2} % при {chromium} % у Chromium"
    );
}
