//! Снимок окна на невидимом столе (`scripts/slint/headless-win.py`): кадр через `render_to_image`
//! (без показа, GPUI test-support), по желанию сначала наведение мыши, потом выход.

use gpui::{App, Modifiers, MouseMoveEvent, PlatformInput, Window, point, px};
use std::time::Duration;

pub fn schedule(window: &mut Window, cx: &mut App) {
    let Ok(out) = std::env::var("HLSR_SMOKE_SNAPSHOT") else {
        return;
    };
    let delay = std::env::var("HLSR_SMOKE_SNAPSHOT_DELAY")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1500);
    let pointer = std::env::var("HLSR_SMOKE_POINTER").ok().and_then(|value| {
        let (x, y) = value.split_once(',')?;
        Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?))
    });
    window
        .spawn(cx, async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(delay))
                .await;
            if let Some((x, y)) = pointer {
                cx.update(|window, cx| {
                    let zoom = f32::from(window.rem_size()) / 16.;
                    let event = MouseMoveEvent {
                        position: point(px(x * zoom), px(y * zoom)),
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    };
                    window.dispatch_event(PlatformInput::MouseMove(event), cx);
                })
                .ok();
                // Пружина наведения успокаивается за ~0,3 с.
                cx.background_executor()
                    .timer(Duration::from_millis(600))
                    .await;
            }
            cx.update(|window, cx| {
                let error = match window.render_to_image() {
                    Ok(image) => match image.save(&out) {
                        Ok(()) => {
                            eprintln!(
                                "снимок {out}: {}x{}, scale_factor {}, rem {:?}",
                                image.width(),
                                image.height(),
                                window.scale_factor(),
                                window.rem_size()
                            );
                            None
                        }
                        Err(error) => Some(format!("снимок {out} не записан: {error}")),
                    },
                    Err(error) => Some(format!("render_to_image: {error:#}")),
                };
                if let Some(error) = error {
                    // Код 1, чтобы headless-win.py увидел провал; сохранять при выходе нечего.
                    eprintln!("{error}");
                    std::process::exit(1);
                }
                cx.quit();
            })
            .ok();
        })
        .detach();
}
