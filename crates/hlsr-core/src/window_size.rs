//! Размер окна и масштаб интерфейса — как `src/main/window-size.js` 0.4.9.

use serde_json::Value;

/// Размер в логических пикселях.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Size {
        Size { width, height }
    }
}

/// Окно с учётом масштаба интерфейса: окно и масштаб растут вместе,
/// поэтому раскладка внутри получает те же логические пиксели.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    /// Зум страницы: `ui_scale × layout_zoom` окна до увеличения (`zoomFactor` 0.4.9).
    pub zoom_factor: f64,
    pub ui_scale: f64,
}

pub const BASE_SIZE: Size = Size::new(1600, 900);
pub const MIN_SIZE: Size = Size::new(1152, 648);

/// Интерфейс разложен по сетке 4 px от краёв окна: при 125 % это ровно
/// 5 физических пикселей. Поэтому обе стороны окна кратны 4.
pub const GRID_STEP: u32 = 4;

/// Масштабы интерфейса из настроек («Интерфейс»).
pub const UI_SCALES: [f64; 4] = [0.75, 1.0, 1.25, 1.5];

const TARGET_WIDTH_FRACTION: f64 = 0.625;
const TARGET_HEIGHT_FRACTION: f64 = 0.65;
const WORK_AREA_MARGIN: f64 = 32.0;
const DEFAULT_UI_SCALE: f64 = 1.0;

/// Размер окна для рабочей области монитора (без панели задач).
pub fn compute_window_size(work_area: Size) -> Size {
    let area = normalize_work_area(work_area);
    let (area_w, area_h) = (f64::from(area.width), f64::from(area.height));
    let (base_w, base_h) = (f64::from(BASE_SIZE.width), f64::from(BASE_SIZE.height));

    let desired = f64::min(
        area_w * TARGET_WIDTH_FRACTION / base_w,
        area_h * TARGET_HEIGHT_FRACTION / base_h,
    );
    // На области уже отступа доля отрицательная — как в JS, её поглощает MIN_SIZE.
    let fitting = f64::min(
        (area_w - WORK_AREA_MARGIN) / base_w,
        (area_h - WORK_AREA_MARGIN) / base_h,
    );
    let scale = 1.0_f64.min(desired).min(fitting);

    let width = f64::from(MIN_SIZE.width)
        .max(js_round(base_w * scale))
        .min(area_w);
    let height = f64::from(MIN_SIZE.height)
        .max(js_round(base_h * scale))
        .min(area_h);

    Size::new(
        snap_to_grid(width, area.width),
        snap_to_grid(height, area.height),
    )
}

/// Окно меньше базового (Full HD: 1200×676, 1152×648) раскладывается как 1600×900 и
/// уменьшается целиком; масштаб из настроек умножается на это уменьшение (`layoutZoom`).
pub fn layout_zoom(size: Size) -> f64 {
    let (width, height) = (f64::from(size.width), f64::from(size.height));
    1.0_f64
        .min(width / f64::from(BASE_SIZE.width))
        .min(height / f64::from(BASE_SIZE.height))
}

/// Полный экран: раскладка 1600×900 растягивается на весь экран `area` (`fillZoom`);
/// масштаб интерфейса не участвует.
pub fn fill_zoom(area: Size) -> f64 {
    let area = normalize_work_area(area);
    f64::min(
        f64::from(area.width) / f64::from(BASE_SIZE.width),
        f64::from(area.height) / f64::from(BASE_SIZE.height),
    )
}

/// Окно и масштаб для выбранного масштаба интерфейса. `size` — из
/// [`compute_window_size`]. Окно, которое не помещается в рабочую область,
/// увеличивается сколько влезает.
pub fn scale_layout(size: Size, work_area: Size, ui_scale: f64) -> Layout {
    let area = normalize_work_area(work_area);
    let wanted = if UI_SCALES.contains(&ui_scale) {
        ui_scale
    } else {
        DEFAULT_UI_SCALE
    };
    let (size_w, size_h) = (f64::from(size.width), f64::from(size.height));
    let fit = f64::min(
        f64::from(area.width) / size_w,
        f64::from(area.height) / size_h,
    );
    let scale = if wanted > 1.0 {
        wanted.min(fit).max(1.0)
    } else {
        wanted
    };
    Layout {
        width: to_u32(js_round(size_w * scale).min(f64::from(area.width))),
        height: to_u32(js_round(size_h * scale).min(f64::from(area.height))),
        zoom_factor: scale * layout_zoom(size),
        ui_scale: scale,
    }
}

/// Масштаб из `settings.json` (`config.uiScale`), как `normalizeUiScale`:
/// `Number(value)`, не из [`UI_SCALES`] → 1.
pub fn normalize_ui_scale(value: &Value) -> f64 {
    let scale = js_to_number(value);
    if UI_SCALES.contains(&scale) {
        scale
    } else {
        DEFAULT_UI_SCALE
    }
}

fn normalize_work_area(work_area: Size) -> Size {
    if work_area.width < 1 || work_area.height < 1 {
        BASE_SIZE
    } else {
        work_area
    }
}

/// Ближайшее кратное шагу, но не больше рабочей области.
fn snap_to_grid(value: f64, limit: u32) -> u32 {
    let snapped = js_round(value / f64::from(GRID_STEP)) * f64::from(GRID_STEP);
    if snapped <= f64::from(limit) {
        to_u32(snapped)
    } else {
        limit / GRID_STEP * GRID_STEP
    }
}

/// `Math.round`: половина — к +∞ (у `f64::round` — от нуля).
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

fn to_u32(x: f64) -> u32 {
    // Значения здесь уже неотрицательны и не больше рабочей области.
    x as u32
}

/// `Number(value)` из JS для значения JSON.
fn js_to_number(value: &Value) -> f64 {
    match value {
        Value::Null => 0.0,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => js_string_to_number(s),
        // Массив приводится к строке через join(","): [1.25] → "1.25".
        Value::Array(_) => js_string_to_number(&crate::js::to_string(value)),
        Value::Object(_) => f64::NAN,
    }
}

/// `StringToNumber` из JS: пробелы JS по краям, пустая строка → 0,
/// `Infinity`, `0x`/`0o`/`0b`, десятичная запись; остальное → NaN.
fn js_string_to_number(s: &str) -> f64 {
    let s = s.trim_matches(crate::js::is_whitespace);
    if s.is_empty() {
        return 0.0;
    }
    match s {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = s.strip_prefix(prefix) {
            return if !digits.is_empty() && digits.chars().all(|c| c.is_digit(radix)) {
                digits.chars().fold(0.0, |acc, c| {
                    acc * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0))
                })
            } else {
                f64::NAN
            };
        }
    }
    // Rust понимает и "inf", "nan" — JS нет: пропускаются только цифры, точка, знак, порядок.
    if s.chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
    {
        s.parse().unwrap_or(f64::NAN)
    } else {
        f64::NAN
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    // Тесты scripts/window-size-selftest.js 0.4.9.

    #[test]
    fn both_sides_fit_grid_and_work_area() {
        for (w, h) in [
            (1920, 1040), // FullHD с панелью задач
            (2048, 1108), // 1440p при 125 %
            (2048, 1100), // 1440p при 125 %, плавающая панель KDE
            (2560, 1400),
            (1366, 728),
            (3840, 2120),
            (1080, 1880), // вертикальный экран уже минимальной ширины
            (1150, 600),  // рабочая область меньше минимального окна
        ] {
            let s = compute_window_size(Size::new(w, h));
            assert_eq!(s.width % GRID_STEP, 0, "{w}x{h}");
            assert_eq!(s.height % GRID_STEP, 0, "{w}x{h}");
            assert!(s.width <= w && s.height <= h, "{w}x{h}");
        }
    }

    #[test]
    fn familiar_screens_keep_sizes() {
        assert_eq!(
            compute_window_size(Size::new(1920, 1040)),
            Size::new(1200, 676)
        );
        assert_eq!(
            compute_window_size(Size::new(2048, 1108)),
            Size::new(1280, 720)
        );
        assert_eq!(
            compute_window_size(Size::new(2560, 1400)),
            Size::new(1600, 900)
        );
        assert_eq!(compute_window_size(Size::new(1366, 728)), MIN_SIZE);
    }

    #[test]
    fn ui_scale_changes_window_and_zoom_together() {
        let area = Size::new(2560, 1400);
        let base = compute_window_size(area);
        // 100 % — размер как до 0.4.3, без увеличения страницы
        assert_eq!(
            scale_layout(base, area, 1.0),
            Layout {
                width: base.width,
                height: base.height,
                zoom_factor: 1.0,
                ui_scale: 1.0
            }
        );
        let small = scale_layout(base, area, 0.75);
        assert_eq!((small.width, small.ui_scale), (1200, 0.75));
        assert_eq!(small.zoom_factor, 0.75);
        // CSS-пиксели раскладки те же
        assert!((f64::from(small.width) / small.ui_scale - f64::from(base.width)).abs() < 1.0);
        // 1440p: 150 % помещается (1600x900 → 2400x1350)
        assert_eq!(scale_layout(base, area, 1.5).ui_scale, 1.5);
        // FullHD: 150 % помещается (1200x676 → 1800x1014)
        let hd = Size::new(1920, 1040);
        assert_eq!(scale_layout(compute_window_size(hd), hd, 1.5).ui_scale, 1.5);
        // 1366x768: 125 % не помещается целиком, окно увеличивается сколько влезает
        let a768 = Size::new(1366, 728);
        let big = scale_layout(compute_window_size(a768), a768, 1.25);
        assert!(
            big.ui_scale > 1.0 && big.ui_scale < 1.25,
            "{}",
            big.ui_scale
        );
        assert!(big.width <= 1366 && big.height <= 728);

        // Full HD (0.4.9): раскладка как 2K (1600×900 CSS), окно уменьшает её целиком.
        for (screen, zoom) in [(hd, 0.75), (Size::new(1536, 824), 0.72)] {
            let fhd = scale_layout(compute_window_size(screen), screen, 1.0);
            assert!((fhd.zoom_factor - zoom).abs() < 0.01, "{}", fhd.zoom_factor);
            assert!((f64::from(fhd.width) / fhd.zoom_factor - 1600.0).abs() < 2.0);
            assert!((f64::from(fhd.height) / fhd.zoom_factor - 900.0).abs() < 2.0);
            assert_eq!(fhd.ui_scale, 1.0);
        }
        // Масштаб из настроек умножается: Full HD 125 % — раскладка всё так же 1600×900.
        let fhd_big = scale_layout(compute_window_size(hd), hd, 1.25);
        let css_width = f64::from(fhd_big.width) / fhd_big.zoom_factor;
        assert!((css_width - 1600.0).abs() < 2.0, "{}", fhd_big.zoom_factor);

        assert_eq!(normalize_ui_scale(&json!("1.25")), 1.25);
        assert_eq!(normalize_ui_scale(&json!(2)), 1.0);
        assert_eq!(normalize_ui_scale(&Value::Null), 1.0);
    }

    #[test]
    fn empty_or_tiny_work_area() {
        // Пустая область заменяется на BASE_SIZE как рабочая область, не как окно.
        assert_eq!(compute_window_size(Size::new(0, 0)), MIN_SIZE);
        assert_eq!(compute_window_size(Size::new(1920, 0)), MIN_SIZE);
        // Уже отступа 32: в JS доля отрицательная, здесь не должно быть переполнения.
        assert_eq!(compute_window_size(Size::new(20, 20)), Size::new(20, 20));
        assert_eq!(
            scale_layout(Size::new(1600, 900), Size::new(0, 0), 1.5),
            Layout {
                width: 1600,
                height: 900,
                zoom_factor: 1.0,
                ui_scale: 1.0
            }
        );
    }

    #[test]
    fn fill_zoom_stretches_base_layout_to_screen() {
        assert_eq!(fill_zoom(Size::new(2560, 1440)), 1.6);
        assert_eq!(fill_zoom(Size::new(1920, 1080)), 1.2);
        // 16:10 — по ширине, снизу больше места.
        assert_eq!(fill_zoom(Size::new(1920, 1200)), 1.2);
        // 21:9 — по высоте.
        assert_eq!(fill_zoom(Size::new(3440, 1440)), 1.6);
        assert_eq!(fill_zoom(Size::new(1366, 768)), 768.0 / 900.0);
        assert_eq!(layout_zoom(Size::new(1200, 676)), 0.75);
        assert_eq!(layout_zoom(BASE_SIZE), 1.0);
        assert_eq!(layout_zoom(Size::new(2000, 1125)), 1.0);
    }

    #[test]
    fn ui_scale_is_read_like_js_number() {
        for (value, expected) in [
            (json!(0.75), 0.75),
            (json!(1.5), 1.5),
            (json!(" 1.25\n"), 1.25),
            (json!("\u{feff}0.75"), 0.75),
            (json!(".75"), 0.75),
            (json!("125e-2"), 1.25),
            (json!(""), 1.0),
            (json!("1.25x"), 1.0),
            (json!(true), 1.0),
            (json!([1.25]), 1.25), // Number([1.25]): массив → "1.25"
            (json!(["1.5"]), 1.5),
            (json!([]), 1.0),
            (json!([1, 2]), 1.0),
            (json!(null), 1.0),
            (json!({"a": 1}), 1.0),
            (json!(1.2), 1.0),
        ] {
            assert_eq!(normalize_ui_scale(&value), expected, "{value}");
        }
    }
}
