//! Снимок подложки стекла на настоящем окне GPUI (патч `vendor/gpui-pre-windows`, `backdrop.rs`):
//! в снимке блока — всё, что окно нарисовало до его метки (фон, полупрозрачное поверх фона), и
//! ничего из блока и того, что после него (своя тень, заливка, непрозрачный ребёнок, сосед сверху).
//! С меткой и без неё кадр один и тот же (`render_to_image`, с dev-hooks).
//!
//! Окно открывается только на невидимом столе, поэтому тест `#[ignore]`:
//! `python scripts/slint/headless-win.py cargo test -p hlsr-gpui --features dev-hooks --test
//! backdrop_capture -- --ignored`. Без `HLSR_HEADLESS_DESKTOP` тест падает, а не открывает окно.
#![cfg(windows)]

use gpui::{
    App, AppContext, BoxShadow, Context, Hsla, IntoElement, ParentElement, Render, Styled, Window,
    WindowBounds, WindowOptions, div, point, px, size,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

const BLOCK: (f32, f32, f32, f32) = (40., 20., 120., 60.);

struct View {
    marker: bool,
    /// Кадр без метки уже нарисован: `render_to_image` берёт последнюю нарисованную сцену.
    unmarked: bool,
}

fn rgb(r: u8, g: u8, b: u8, a: f32) -> Hsla {
    gpui::Rgba {
        r: f32::from(r) / 255.,
        g: f32::from(g) / 255.,
        b: f32::from(b) / 255.,
        a,
    }
    .into()
}

fn at(x: f32, y: f32, w: f32, h: f32) -> gpui::Div {
    div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h))
}

impl Render for View {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (x, y, w, h) = BLOCK;
        self.unmarked |= !self.marker;
        let mut shadows = vec![BoxShadow {
            color: rgb(0, 0, 0, 0.6),
            offset: point(px(0.), px(4.)),
            blur_radius: px(8.),
            spread_radius: px(0.),
            inset: false,
        }];
        if self.marker {
            shadows.insert(
                0,
                BoxShadow {
                    color: gpui_windows::backdrop::MARKER,
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(0.),
                    inset: false,
                },
            );
        }
        div()
            .size_full()
            .bg(rgb(200, 0, 0, 1.))
            // Левая половина окна синяя, сверху полоса белого .5 над верхом блока.
            .child(at(0., 0., 100., 100.).bg(rgb(0, 0, 200, 1.)))
            .child(at(x, y, w, 30.).bg(rgb(255, 255, 255, 0.5)))
            .child(
                at(x, y, w, h)
                    .shadow(shadows)
                    .bg(rgb(0, 200, 0, 0.5))
                    .child(at(10., 10., 20., 20.).bg(rgb(255, 255, 255, 1.))),
            )
            .child(at(150., 30., 40., 40.).bg(rgb(230, 230, 0, 1.)))
    }
}

/// Что должно быть под блоком в точке окна (px): синее/красное, сверху белое .5.
fn expected(x: f32, y: f32) -> [u8; 3] {
    let base = if x < 100. {
        [0f32, 0., 200.]
    } else {
        [200., 0., 0.]
    };
    let white = y < BLOCK.1 + 30.;
    base.map(|c| {
        let c = if white { c + (255. - c) * 0.5 } else { c };
        c.round() as u8
    })
}

#[derive(Default)]
struct Log {
    capture: Option<(f32, gpui_windows::backdrop::Capture)>,
    #[cfg(feature = "dev-hooks")]
    frames: Vec<image::RgbaImage>,
    done: bool,
}

#[test]
#[ignore = "окно GPUI: только под scripts/slint/headless-win.py, с -- --ignored"]
fn the_backdrop_is_what_lies_under_the_block() {
    assert!(
        std::env::var_os("HLSR_HEADLESS_DESKTOP").is_some(),
        "backdrop_capture: только под scripts/slint/headless-win.py"
    );
    let log = Rc::new(RefCell::new(Log::default()));
    let platform = gpui_windows::WindowsPlatform::new(false).expect("платформа GPUI");
    let run_log = log.clone();
    gpui::Application::with_platform(Rc::new(platform)).run(move |cx: &mut App| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(gpui::Bounds::new(
                point(px(0.), px(0.)),
                size(px(200.), px(100.)),
            ))),
            ..Default::default()
        };
        let window = cx
            .open_window(options, |_, cx| {
                cx.new(|_| View {
                    marker: true,
                    unmarked: false,
                })
            })
            .expect("окно");
        let log = run_log.clone();
        cx.spawn(async move |cx| {
            for _ in 0..200 {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let step = window.update(cx, |view, window, cx| {
                    let mut log = log.borrow_mut();
                    if log.capture.is_none() {
                        let hwnd = hlsr_gpui::win32::hwnd(window).expect("HWND");
                        let captures = gpui_windows::backdrop::latest(hwnd);
                        let first = captures.and_then(|all| all.first().cloned());
                        log.capture = first.map(|capture| (window.scale_factor(), capture));
                        #[cfg(feature = "dev-hooks")]
                        if log.capture.is_some() {
                            log.frames.push(window.render_to_image().expect("кадр"));
                            view.marker = false;
                            cx.notify();
                        }
                        return false;
                    }
                    #[cfg(feature = "dev-hooks")]
                    if log.frames.len() == 1 {
                        if !view.unmarked {
                            return false;
                        }
                        log.frames.push(window.render_to_image().expect("кадр"));
                    }
                    let _ = (view, cx);
                    log.done = true;
                    true
                });
                if step.unwrap_or(true) {
                    break;
                }
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    let log = log.borrow();
    assert!(log.done, "снимка нет");
    let (scale, capture) = log.capture.as_ref().expect("снимок");
    let (x, y, w, h) = BLOCK;
    let device = |v: f32| (v * scale).round() as u32;
    assert_eq!(
        capture.rect,
        [device(x), device(y), device(w), device(h)],
        "место снимка"
    );
    let [left, top, width, height] = capture.rect;
    let mut wrong = 0;
    for j in 0..height {
        for i in 0..width {
            // Края полос (сглаживание при дробном масштабе) не проверяются.
            let (px_x, px_y) = ((left + i) as f32 / scale, (top + j) as f32 / scale);
            if (px_x - 100.).abs() < 1.5 || (px_y - (y + 30.)).abs() < 1.5 {
                continue;
            }
            let k = ((j * width + i) * 4) as usize;
            let [b, g, r] = [capture.bgra[k], capture.bgra[k + 1], capture.bgra[k + 2]];
            let want = expected(px_x, px_y);
            if [r, g, b]
                .iter()
                .zip(want)
                .any(|(&got, want)| got.abs_diff(want) > 2)
            {
                wrong += 1;
                if wrong < 5 {
                    eprintln!("({px_x}, {px_y}): {:?} вместо {want:?}", [r, g, b]);
                }
            }
        }
    }
    assert_eq!(wrong, 0, "в снимке не то, что под блоком");
    #[cfg(feature = "dev-hooks")]
    {
        assert_eq!(log.frames.len(), 2);
        assert!(
            log.frames[0] == log.frames[1],
            "кадр с меткой отличается от кадра без неё"
        );
    }
}
