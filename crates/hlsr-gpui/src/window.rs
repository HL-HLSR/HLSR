//! Окна на GPUI: окно лаунчера (без рамки, прозрачное: углы и тень рисует интерфейс, как `#app`
//! 0.4.9), окна режимов и место окна в рабочей области.

use gpui::{
    Bounds, Pixels, Point, Size, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowOptions, point,
};

/// Окно лаунчера: `bounds` — место и размер в логических px.
pub fn launcher_options(bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("HLSR".into()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        is_resizable: false,
        window_decorations: Some(WindowDecorations::Client),
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("hlsr".into()),
        ..full_rate()
    }
}

/// Окно режима (установщик, удаление, апдейтер) с рамкой ОС и заголовком.
pub fn mode_options(title: &str, bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(title.to_owned().into()),
            appears_transparent: false,
            traffic_light_position: None,
        }),
        is_resizable: false,
        app_id: Some("hlsr".into()),
        ..full_rate()
    }
}

/// Кадры по общему пределу (60 в секунду по сетке vsync монитора окна, `gpui::frame_divisor`) и у
/// окна без фокуса: GPUI по умолчанию держит неактивное окно (и всё при энергосбережении Windows)
/// на 30 кадрах/с, а 0.4.9 — `backgroundThrottling: false`, и Chromium рисует видимое окно по
/// vsync всегда. Скрытое и свёрнутое окно кадров не просит (`AnimGate`), так что лишней работы нет.
pub fn full_rate() -> WindowOptions {
    WindowOptions {
        inactive_frame_interval: None,
        low_power_frame_interval: None,
        ..Default::default()
    }
}

/// Окно размером `size` в рабочей области `area`: по центру (`center: true` 0.4.7) или вокруг
/// прежнего центра `center` в пределах области (`applyUiScale` 0.4.7).
pub fn place(
    area: Bounds<Pixels>,
    size: Size<Pixels>,
    center: Option<Point<Pixels>>,
) -> Bounds<Pixels> {
    let axis = |start: Pixels, len: Pixels, inner: Pixels, middle: Option<Pixels>| -> Pixels {
        let (start, len, inner) = (f32::from(start), f32::from(len), f32::from(inner));
        let at = match middle {
            Some(middle) => (f32::from(middle) - inner / 2.)
                .max(start)
                .min(start + len - inner),
            None => start + (len - inner) / 2.,
        };
        Pixels::from(at.round())
    };
    let origin = point(
        axis(
            area.origin.x,
            area.size.width,
            size.width,
            center.map(|c| c.x),
        ),
        axis(
            area.origin.y,
            area.size.height,
            size.height,
            center.map(|c| c.y),
        ),
    );
    Bounds { origin, size }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    /// Логические px без `px(` (линт `scale.rs`: в виде размеры — через `u()`).
    fn lp(v: f32) -> Pixels {
        Pixels::from(v)
    }

    fn area() -> Bounds<Pixels> {
        Bounds {
            origin: point(lp(0.), lp(40.)),
            size: size(lp(1920.), lp(1000.)),
        }
    }

    #[test]
    fn the_window_opens_centered_in_the_work_area() {
        let placed = place(area(), size(lp(1200.), lp(676.)), None);
        assert_eq!(placed.origin, point(lp(360.), lp(202.)));
        // Окно больше области — выступает за края поровну.
        let wide = place(area(), size(lp(2000.), lp(676.)), None);
        assert_eq!(wide.origin.x, lp(-40.));
    }

    #[test]
    fn rescale_keeps_the_center_inside_the_area() {
        let around = place(
            area(),
            size(lp(1500.), lp(845.)),
            Some(point(lp(960.), lp(540.))),
        );
        assert_eq!(around.origin, point(lp(210.), lp(118.)));
        // У края области: окно не уходит за левый и верхний край.
        let corner = place(
            area(),
            size(lp(1500.), lp(845.)),
            Some(point(lp(100.), lp(100.))),
        );
        assert_eq!(corner.origin, point(lp(0.), lp(40.)));
    }
}
