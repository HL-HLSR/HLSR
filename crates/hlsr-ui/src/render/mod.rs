//! Растры на ЦП: превью персонализации, ник и рамка, круг HSV, спиннер, стрелки заставки,
//! стекло колонки, разбор CSS (тени, цвета). Вид показывает их картинками.

pub mod baked;
pub mod canvas_text;
pub mod color;
pub mod css;
pub mod frame;
pub mod ghost_mesh;
pub mod ghost_scene;
pub mod ghosting_overlay;
pub mod hl1_overlay;
pub mod hsv;
pub mod liquid;
pub mod livesplit_sample;
pub mod nick;
pub mod preview;
pub mod preview_assets;
pub mod profile_art;
pub mod rail_glass;
pub mod raster;
pub mod shadow_blur;
pub mod spinner;
pub mod triangle;

/// Шаг колеса Chromium за один щелчок: Windows — 3 строки
/// (`SPI_GETWHEELSCROLLLINES` по умолчанию) × 100/3 px, Linux (X11) — 53 px.
pub const WHEEL_STEP: f32 = if cfg!(windows) { 100.0 } else { 53.0 };

/// Шаг колеса Chromium на Windows при `lines` строках за щелчок (`SPI_GETWHEELSCROLLLINES`):
/// `lines × kScrollbarPixelsPerLine` (100/3 px), как WebMouseWheelEventBuilder.
pub fn wheel_step_for_lines(lines: u32) -> f32 {
    lines as f32 * 100.0 / 3.0
}
