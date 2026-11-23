//! Окна на GPUI: окно лаунчера (без рамки, прозрачное: углы и тень рисует интерфейс, как `#app`
//! 0.4.9), окна режимов и место окна в рабочей области.

use gpui::{
    Bounds, Div, InteractiveElement, Pixels, Point, Size, Styled, TitlebarOptions,
    WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowDecorations, WindowOptions,
    div, point, px, white,
};

/// Контур окна (`#app::after`: `inset 0 0 0 1px rgba(255,255,255,.12)`).
/// Сдвинут на 1 px наружу с толщиной 2 px и радиусом `r + 1`, чтобы внутренняя граница
/// лежала ровно в 1 px от края окна, а наружная за пределами маски `window_corner_mask` —
/// тогда на скруглённых углах рамка не затухает дважды и совпадает по яркости с прямыми краями.
pub(crate) fn frame_border(radius: Pixels) -> Div {
    let one = px(1.);
    div()
        .absolute()
        .top(-one)
        .right(-one)
        .bottom(-one)
        .left(-one)
        .rounded(radius + one)
        .border_2()
        .border_color(white().opacity(0.12))
}

/// Область, за которую таскают окно без рамки (`-webkit-app-region: drag` 0.4.9). На Windows —
/// `HTCAPTION` по хит-тесту GPUI. На Linux GPUI областей окна не хит-тестит
/// (`on_hit_test_window_control` у X11 и Wayland пустой), и окно было не взять: там перенос
/// начинает композитор по `start_window_move` с нажатия левой кнопки. Нажатие доходит до
/// области там же, где Windows отдаёт `HTCAPTION`: над ней нет перекрывающего (`occlude`)
/// элемента — кнопки и вкладки окно не таскают.
pub fn drag_area<E: InteractiveElement>(el: E) -> E {
    let el = el.window_control_area(WindowControlArea::Drag);
    #[cfg(not(windows))]
    let el = el.on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
        window.start_window_move();
    });
    el
}

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

/// Кадры с частотой монитора окна (`gpui::frame_divisor`) и у окна без фокуса: GPUI по умолчанию держит неактивное окно (и всё при энергосбережении Windows)
/// на 30 кадрах/с, а 0.4.9 — `backgroundThrottling: false`, и Chromium рисует видимое окно по
/// vsync всегда. Скрытое и свёрнутое окно кадров не просит (`AnimGate`), так что лишней работы нет.
pub fn full_rate() -> WindowOptions {
    WindowOptions {
        inactive_frame_interval: None,
        low_power_frame_interval: None,
        thermal_frame_interval: None,
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
