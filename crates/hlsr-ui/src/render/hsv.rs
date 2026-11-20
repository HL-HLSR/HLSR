//! HSV-круг редактора цвета HUD (`GhostingPersonalization.vue:335-354` 0.4.7, решение 19):
//! битмап 168 × 168, как `drawWheel` рисует его в `<canvas>` (те же байты — сверка по хешу
//! фикстуры `personalization-logic.json`), и картинка для окна: `filter: saturate(0.9)`
//! и уменьшение до 108 CSS px (× масштаб) — на ЦП, одинаково у любого рендерера.

use crate::controllers::hud_editor::hsv_to_rgb;
use crate::{Rgba, Rgba8Image};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

/// Сторона холста 0.4.7 (`canvas width=168 height=168`).
pub const CANVAS: usize = 168;
/// Сторона круга в окне (`.pers__wheel { width: 108px; height: 108px }`).
pub const BOX_PX: f32 = 108.0;
/// `filter: saturate(0.9)`.
pub const SATURATE: f32 = 0.9;

/// `drawWheel`: RGBA без предумножения, 168 × 168, построчно сверху вниз. Вне круга —
/// нули (`createImageData` даёт прозрачный чёрный).
pub fn wheel_bitmap() -> Vec<u8> {
    let size = CANVAS;
    let center = size as f64 / 2.0;
    let radius = center - 4.0;
    let mut data = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let dx = x as f64 - center;
            let dy = y as f64 - center;
            let dist = dx.hypot(dy);
            if dist > radius {
                continue;
            }
            let hue = (dy.atan2(dx) * 180.0 / std::f64::consts::PI + 360.0) % 360.0;
            let c = hsv_to_rgb(hue, (dist / radius).min(1.0), 1.0);
            let i = (y * size + x) * 4;
            data[i] = c.r as u8;
            data[i + 1] = c.g as u8;
            data[i + 2] = c.b as u8;
            // Uint8ClampedArray: дробное округляется к ближайшему чётному при .5, больше 255 — 255.
            data[i + 3] = clamp_u8(((radius - dist) * 40.0).min(255.0));
        }
    }
    data
}

/// Цвет круга под центром курсора (`left/top: 50% + 42% · s`): пиксель [`wheel_bitmap`] на
/// этом месте с `saturate(0.9)`. В 0.4.9 он виден сквозь кольцо курсора.
pub fn wheel_color_at(h: f64, s: f64) -> Rgba {
    let radius = CANVAS as f64 / 2.0 - 4.0;
    let c = hsv_to_rgb(h, (0.42 * s * CANVAS as f64 / radius).min(1.0), 1.0);
    let [r, g, b] = saturate(
        [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0],
        SATURATE,
    )
    .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    Rgba::new(r, g, b, 255)
}

/// Запись в `Uint8ClampedArray`: округление к ближайшему, при равной близости — к чётному.
fn clamp_u8(value: f64) -> u8 {
    if value <= 0.0 {
        return 0;
    }
    if value >= 255.0 {
        return 255;
    }
    let floor = value.floor();
    let fraction = value - floor;
    let up = fraction > 0.5 || (fraction == 0.5 && floor % 2.0 != 0.0);
    (if up { floor + 1.0 } else { floor }) as u8
}

/// `filter: saturate(s)` — матрица Filter Effects, в sRGB, без предумножения.
fn saturate(rgb: [f32; 3], s: f32) -> [f32; 3] {
    let [r, g, b] = rgb;
    [
        (0.213 + 0.787 * s) * r + (0.715 - 0.715 * s) * g + (0.072 - 0.072 * s) * b,
        (0.213 - 0.213 * s) * r + (0.715 + 0.285 * s) * g + (0.072 - 0.072 * s) * b,
        (0.213 - 0.213 * s) * r + (0.715 - 0.715 * s) * g + (0.072 + 0.928 * s) * b,
    ]
}

/// Круг для окна: `side` физических пикселей, насыщенность `saturation`; уменьшение —
/// усреднение по площади предумноженных пикселей (как качественный даунсэмплинг Skia).
pub fn wheel_image(side: u32, saturation: f32) -> Rgba8Image {
    let source = wheel_bitmap();
    let side = side.max(1) as usize;
    let mut buffer = Rgba8Image::new(side as u32, side as u32);
    let scale = CANVAS as f32 / side as f32;
    let pixels = buffer.make_mut_slice();
    for y in 0..side {
        for x in 0..side {
            let (x0, x1) = (x as f32 * scale, (x + 1) as f32 * scale);
            let (y0, y1) = (y as f32 * scale, (y + 1) as f32 * scale);
            let mut sum = [0f32; 4];
            let mut area = 0f32;
            let mut sy = y0.floor() as usize;
            while (sy as f32) < y1 && sy < CANVAS {
                let hy = (y1.min(sy as f32 + 1.0) - y0.max(sy as f32)).max(0.0);
                let mut sx = x0.floor() as usize;
                while (sx as f32) < x1 && sx < CANVAS {
                    let wx = (x1.min(sx as f32 + 1.0) - x0.max(sx as f32)).max(0.0);
                    let w = wx * hy;
                    let i = (sy * CANVAS + sx) * 4;
                    let a = f32::from(source[i + 3]) / 255.0;
                    let rgb = saturate(
                        [
                            f32::from(source[i]) / 255.0,
                            f32::from(source[i + 1]) / 255.0,
                            f32::from(source[i + 2]) / 255.0,
                        ],
                        saturation,
                    );
                    sum[0] += rgb[0].clamp(0.0, 1.0) * a * w;
                    sum[1] += rgb[1].clamp(0.0, 1.0) * a * w;
                    sum[2] += rgb[2].clamp(0.0, 1.0) * a * w;
                    sum[3] += a * w;
                    area += w;
                    sx += 1;
                }
                sy += 1;
            }
            let px = &mut pixels[y * side + x];
            if area <= 0.0 || sum[3] <= 0.0 {
                *px = Rgba::TRANSPARENT;
                continue;
            }
            let alpha = sum[3] / area;
            let to = |v: f32| (v / sum[3] * 255.0).round().clamp(0.0, 255.0) as u8;
            *px = Rgba::new(
                to(sum[0]),
                to(sum[1]),
                to(sum[2]),
                (alpha * 255.0).round().clamp(0.0, 255.0) as u8,
            );
        }
    }
    buffer
}

thread_local! {
    static CACHE: RefCell<HashMap<(u32, u32), Arc<Rgba8Image>>> = RefCell::new(HashMap::new());
}

/// Картинка круга для коробки `box_px` логических px при масштабе `scale`, с кэшем по
/// физическому размеру.
pub fn wheel_window_image(box_px: f32, scale: f32) -> Arc<Rgba8Image> {
    let side = (box_px * scale.max(0.01)).round().max(1.0) as u32;
    let key = (side, (SATURATE * 1000.0) as u32);
    CACHE.with(|cache| {
        if let Some(image) = cache.borrow().get(&key) {
            return image.clone();
        }
        let image = Arc::new(wheel_image(side, SATURATE));
        cache.borrow_mut().insert(key, image.clone());
        image
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bitmap_is_transparent_outside_the_ring_and_opaque_inside() {
        let data = wheel_bitmap();
        let px = |x: usize, y: usize| &data[(y * CANVAS + x) * 4..(y * CANVAS + x) * 4 + 4];
        assert_eq!(px(0, 0), [0, 0, 0, 0]);
        // Центр: белый (насыщенность 0); вправо оттенок 0 и насыщенность по расстоянию:
        // на краю круга (x = 164, dist = radius) — чистый красный, на 4 px ближе — s = 0,95.
        assert_eq!(px(84, 84), [255, 255, 255, 255]);
        assert_eq!(&px(164, 84)[..3], [255, 0, 0]);
        assert_eq!(&px(160, 84)[..3], [255, 13, 13]);
        // Кромка: прозрачность растёт по 40 на пиксель к центру, на самом краю — 0.
        assert_eq!(px(164, 84)[3], 0);
        assert_eq!(px(163, 84)[3], 40);
        assert_eq!(px(158, 84)[3], 240);
        assert_eq!(clamp_u8(0.5), 0);
        assert_eq!(clamp_u8(1.5), 2);
        assert_eq!(clamp_u8(2.5), 2);
        assert_eq!(clamp_u8(300.0), 255);
    }

    #[test]
    fn the_window_image_keeps_the_disc_and_desaturates_slightly() {
        let image = wheel_image(108, SATURATE);
        let pixels = image.as_slice();
        let at = |x: usize, y: usize| pixels[y * 108 + x];
        assert_eq!(at(0, 0).a, 0);
        // Центр 108 px усредняет исходные столбцы 84 (белый) и 85 (s = 1/80) — почти белый.
        let centre = at(54, 54);
        assert_eq!((centre.r, centre.a), (255, 255));
        assert!(centre.g >= 250 && centre.b >= 250, "{centre:?}");
        // Красный край после saturate(0.9) чуть серее чистого красного.
        let edge = at(100, 54);
        assert!(edge.r > 220 && edge.g > 0 && edge.g < 40 && edge.b < 40);
        assert_eq!(wheel_window_image(BOX_PX, 1.25).width(), 135);
    }

    #[test]
    fn the_cursor_shows_the_wheel_pixel_under_it() {
        // Центр — белый; s = 1, оттенок 0 — точка 42 % от центра (x = 84 + 70,56): тот же
        // пиксель холста, что у курсора 0.4.9, после saturate(0.9).
        assert_eq!(wheel_color_at(120., 0.), Rgba::new(255, 255, 255, 255));
        let data = wheel_bitmap();
        let i = (84 * CANVAS + 155) * 4;
        let [r, g, b] = saturate([0, 1, 2].map(|k| f32::from(data[i + k]) / 255.0), SATURATE)
            .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as i32);
        let c = wheel_color_at(0., 1.);
        for (got, want) in [(c.r, r), (c.g, g), (c.b, b)] {
            assert!(
                (i32::from(got) - want).abs() <= 4,
                "{c:?} против {r} {g} {b}"
            );
        }
    }
}
