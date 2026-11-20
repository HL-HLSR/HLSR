//! Кольцо `.fa-spin::before` (launcher.css:635): круг 1em с рамкой 0.14em,
//! рамка цветом `color-mix(currentColor 28%, transparent)`, верхняя сторона —
//! `currentColor`.
//!
//! У круга с `border-radius: 50%` и разными цветами сторон Chromium красит
//! каждую сторону внутри её четырёхугольника (`ClipBorderSidePolygon`): верх
//! ограничен диагоналями коробки, поэтому «верхняя» дуга — сектор ±45° от
//! вертикали, а срезы с боковыми дугами косые, по диагоналям. Кольцо
//! растеризуется один раз, дальше его вращает вид (анимация `l-spin`, 0,85 с
//! linear) или гасит `l-pulse` (прозрачность).

use crate::{Rgba as Color, Rgba8Image};

/// Доля `currentColor` в цвете трёх нижних сторон рамки.
const TRACK_MIX: f32 = 0.28;
/// Толщина рамки в долях размера (0.14em).
const BORDER_EM: f32 = 0.14;
/// Подвыборок на сторону пикселя (покрытие кромок; 8 × 8 = 64).
const SUBSAMPLES: u32 = 8;

/// Кольцо квадрата `size` логических px при масштабе `scale`: картинка
/// `round(size × scale)` физических px (не меньше 1).
pub fn ring(size: f32, color: Color, scale: f32) -> Rgba8Image {
    let side = ((size * scale).round() as i64).clamp(1, 4096) as u32;
    let mut buffer = Rgba8Image::new(side, side);
    let pixels = buffer.make_mut_slice();
    let d = side as f32;
    let radius = d / 2.0;
    // Толщина рамки округляется Chromium до целых физических px (кроме
    // тонкой: меньше пикселя рисуется как 1 px).
    let border = (BORDER_EM * d).max(1.0).floor().max(1.0);
    let inner = radius - border;
    let alpha = f32::from(color.a) / 255.0;
    let n = SUBSAMPLES;
    let step = 1.0 / n as f32;
    for py in 0..side {
        for px in 0..side {
            let (mut top, mut rest) = (0u32, 0u32);
            for sy in 0..n {
                for sx in 0..n {
                    let x = px as f32 + (sx as f32 + 0.5) * step - radius;
                    let y = py as f32 + (sy as f32 + 0.5) * step - radius;
                    let r2 = x * x + y * y;
                    if r2 > radius * radius || r2 < inner * inner {
                        continue;
                    }
                    // Верхний сектор: между диагоналями, выше центра.
                    if -y > x.abs() {
                        top += 1;
                    } else {
                        rest += 1;
                    }
                }
            }
            let total = (n * n) as f32;
            let coverage = (top as f32 + TRACK_MIX * rest as f32) / total;
            pixels[(py * side + px) as usize] = Color {
                a: (coverage * alpha * 255.0).round().clamp(0.0, 255.0) as u8,
                ..color
            };
        }
    }
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Color = Color::rgb(255, 255, 255);

    fn alpha(buffer: &Rgba8Image, x: u32, y: u32) -> u8 {
        buffer.as_slice()[(y * buffer.width() + x) as usize].a
    }

    #[test]
    fn top_arc_is_opaque_and_the_rest_is_a_28_percent_track() {
        // 40 px: рамка 5 px, радиусы 20 и 15.
        let ring = ring(40.0, WHITE, 1.0);
        assert_eq!((ring.width(), ring.height()), (40, 40));
        // Середина верхней дуги (x = 20, y = 2): полный цвет.
        assert_eq!(alpha(&ring, 20, 2), 255);
        // Середины боковых и нижней дуг: 28 % (0.28 × 255 = 71.4).
        for (x, y) in [(2, 20), (37, 20), (20, 37)] {
            assert_eq!(alpha(&ring, x, y), 71, "({x}, {y})");
        }
        // Центр и углы коробки пусты.
        for (x, y) in [(20, 20), (0, 0), (39, 0), (0, 39), (39, 39)] {
            assert_eq!(alpha(&ring, x, y), 0, "({x}, {y})");
        }
    }

    #[test]
    fn cuts_between_the_arcs_run_along_the_box_diagonals() {
        let ring = ring(40.0, WHITE, 1.0);
        // Точка кольца выше диагонали (слева от неё по верхней дуге) полностью
        // верхняя; на ту же глубину ниже диагонали — боковая.
        // Радиус 17.5 — середина кольца; угол 30° от вертикали и 60°.
        let at = |degrees: f32| {
            let r = 17.5_f32;
            let a = degrees.to_radians();
            (
                (20.0 + r * a.sin()).floor() as u32,
                (20.0 - r * a.cos()).floor() as u32,
            )
        };
        let (x, y) = at(30.0);
        assert_eq!(alpha(&ring, x, y), 255);
        let (x, y) = at(-30.0);
        assert_eq!(alpha(&ring, x, y), 255);
        let (x, y) = at(60.0);
        assert_eq!(alpha(&ring, x, y), 71);
        let (x, y) = at(-60.0);
        assert_eq!(alpha(&ring, x, y), 71);
        // На самой диагонали (45°): смесь покрытия двух цветов.
        let (x, y) = at(45.0);
        let a = alpha(&ring, x, y);
        assert!((72..255).contains(&a), "{a}");
    }

    #[test]
    fn ring_follows_color_alpha_and_scale() {
        let accent = Color::new(0, 171, 255, 128);
        let ring = ring(14.0, accent, 1.25);
        // 14 × 1.25 = 17.5 → 18 физических px.
        assert_eq!((ring.width(), ring.height()), (18, 18));
        let top = alpha(&ring, 9, 0);
        // Верх: покрытие ≈ полное × 128; цвет без изменений.
        assert!((100..=128).contains(&top), "{top}");
        let pixel = ring.as_slice()[9];
        assert_eq!((pixel.r, pixel.g, pixel.b), (0, 171, 255));
    }

    #[test]
    fn border_is_at_least_one_physical_pixel() {
        // 6 px × 0.14 = 0.84 → рамка 1 px; кольцо не пустое.
        let ring = ring(6.0, WHITE, 1.0);
        assert!(ring.as_slice().iter().any(|p| p.a >= 200));
    }
}
