//! Гауссово размытие тремя проходами «коробкой» — как feGaussianBlur в SVG и Skia (Chromium):
//! свечения по маске на ЦП (ник, рамка аватара, стрелки заставки; вопрос 1 плана GPUI). Края —
//! повтор крайнего пикселя. Ядро бывшего `blur.rs` без частей стекла.

use crate::{Rgba, Rgba8Image};

/// Размеры коробок трёх проходов для `sigma`; `sigma < 0.5` — без размытия
/// (`[0; 3]`). `d = floor(sigma × 3 × √(2π) / 4 + 0.5)`: нечётное — три
/// прохода `d`, чётное — два `d` (со сдвигом на полпикселя влево, затем
/// вправо) и один `d + 1`.
pub fn box_sizes(sigma: f32) -> [u32; 3] {
    if sigma.is_nan() || sigma < 0.5 {
        return [0; 3];
    }
    let d = (sigma * 3.0 * (2.0 * std::f32::consts::PI).sqrt() / 4.0 + 0.5).floor() as u32;
    if d % 2 == 1 { [d, d, d] } else { [d, d, d + 1] }
}

/// Окна трёх проходов: сколько пикселей слева и справа от текущего.
fn windows(sigma: f32) -> Option<[(usize, usize); 3]> {
    let [d, _, _] = box_sizes(sigma);
    if d <= 1 {
        return None;
    }
    let d = d as usize;
    Some(if d % 2 == 1 {
        let r = (d - 1) / 2;
        [(r, r); 3]
    } else {
        let h = d / 2;
        [(h, h - 1), (h - 1, h), (h, h)]
    })
}

/// Проход коробкой по столбцам плоскости (`rows` строк по `columns`), строка за строкой,
/// суммами всех столбцов сразу: цикл векторизуется. За краем — крайняя строка.
fn column_pass(
    src: &[f32],
    dst: &mut [f32],
    columns: usize,
    rows: usize,
    (left, right): (usize, usize),
) {
    if columns == 0 || rows == 0 {
        return;
    }
    let width = (left + right + 1) as f32;
    let last = rows as isize - 1;
    let row = |i: isize| -> &[f32] {
        let i = i.clamp(0, last) as usize;
        &src[i * columns..(i + 1) * columns]
    };
    let mut sums: Vec<f32> = (0..columns)
        .map(|x| (-(left as isize)..=right as isize).map(|i| row(i)[x]).sum())
        .collect();
    for (i, out) in dst.chunks_exact_mut(columns).enumerate() {
        let i = i as isize;
        for (o, s) in out.iter_mut().zip(&sums) {
            *o = s / width;
        }
        // Сумма += пришедший − ушедший.
        let (add, sub) = (row(i + right as isize + 1), row(i - left as isize));
        for ((s, a), b) in sums.iter_mut().zip(add).zip(sub) {
            *s += a - b;
        }
    }
}

/// `src` (`rows` строк по `columns`) по столбцам — в `dst` (`columns` строк по `rows`).
fn transpose(src: &[f32], dst: &mut [f32], columns: usize, rows: usize) {
    // Плитками: и чтение, и запись остаются в кэше.
    const TILE: usize = 32;
    for x0 in (0..columns).step_by(TILE) {
        for y0 in (0..rows).step_by(TILE) {
            for x in x0..(x0 + TILE).min(columns) {
                let line = &mut dst[x * rows..(x + 1) * rows];
                for y in y0..(y0 + TILE).min(rows) {
                    line[y] = src[y * columns + x];
                }
            }
        }
    }
}

/// Размыть одну плоскость `width × height` на месте: по строкам (столбцы транспонированной), затем
/// по столбцам.
fn blur_plane(plane: &mut Vec<f32>, width: usize, height: usize, passes: [(usize, usize); 3]) {
    let mut tmp = vec![0.0; plane.len()];
    transpose(plane, &mut tmp, width, height);
    std::mem::swap(plane, &mut tmp);
    for window in passes {
        column_pass(plane, &mut tmp, height, width, window);
        std::mem::swap(plane, &mut tmp);
    }
    transpose(plane, &mut tmp, height, width);
    std::mem::swap(plane, &mut tmp);
    for window in passes {
        column_pass(plane, &mut tmp, width, height, window);
        std::mem::swap(plane, &mut tmp);
    }
}

/// Размыть маску прозрачности (один байт на пиксель).
pub fn gaussian_blur_alpha(mask: &mut [u8], width: u32, height: u32, sigma: f32) {
    let (w, h) = (width as usize, height as usize);
    assert_eq!(mask.len(), w * h, "маска {width}×{height}");
    let Some(passes) = windows(sigma) else {
        return;
    };
    let mut plane: Vec<f32> = mask.iter().map(|&v| f32::from(v)).collect();
    blur_plane(&mut plane, w, h, passes);
    for (out, v) in mask.iter_mut().zip(plane) {
        *out = v.round().clamp(0.0, 255.0) as u8;
    }
}

/// Размыть картинку RGBA (прозрачность не предумножена). Размытие — в предумноженных цветах,
/// как у Skia.
pub fn gaussian_blur_rgba(image: &mut Rgba8Image, sigma: f32) {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let Some(passes) = windows(sigma) else {
        return;
    };
    let pixels = image.make_mut_slice();
    let mut planes: [Vec<f32>; 4] = std::array::from_fn(|_| Vec::with_capacity(w * h));
    for p in pixels.iter() {
        let a = f32::from(p.a) / 255.0;
        planes[0].push(f32::from(p.r) * a);
        planes[1].push(f32::from(p.g) * a);
        planes[2].push(f32::from(p.b) * a);
        planes[3].push(f32::from(p.a));
    }
    for plane in &mut planes {
        blur_plane(plane, w, h, passes);
    }
    for (i, p) in pixels.iter_mut().enumerate() {
        let exact = planes[3][i];
        let a = exact.round().clamp(0.0, 255.0);
        // Делить на неокруглённую прозрачность: иначе 255 превращается в 254.
        let unpremultiply = |v: f32| {
            if a > 0.0 {
                (v * 255.0 / exact).round().clamp(0.0, 255.0) as u8
            } else {
                0
            }
        };
        *p = Rgba::new(
            unpremultiply(planes[0][i]),
            unpremultiply(planes[1][i]),
            unpremultiply(planes[2][i]),
            a as u8,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_sizes_follow_svg_formula() {
        assert_eq!(box_sizes(1.0), [2, 2, 3]); // d = floor(1.88 + 0.5) = 2 → чётное
        assert_eq!(box_sizes(8.0), [15, 15, 15]); // d = floor(15.04 + 0.5) = 15 → нечётное
        assert_eq!(box_sizes(0.4), [0, 0, 0]);
        assert_eq!(box_sizes(f32::NAN), [0, 0, 0]);
    }

    /// Один проход коробкой по `count` линиям длины `len` (шаг между пикселями линии — `step`,
    /// между линиями — `line_step`): прямой образец для [`column_pass`].
    fn box_pass(
        src: &[f32],
        dst: &mut [f32],
        (len, step, count, line_step): (usize, usize, usize, usize),
        (left, right): (usize, usize),
    ) {
        let width = (left + right + 1) as f32;
        let last = len as isize - 1;
        let at = |base: usize, i: isize| src[base + i.clamp(0, last) as usize * step];
        for line in 0..count {
            let base = line * line_step;
            let mut sum: f32 = (-(left as isize)..=right as isize)
                .map(|i| at(base, i))
                .sum();
            for i in 0..len as isize {
                dst[base + i as usize * step] = sum / width;
                sum += at(base, i + right as isize + 1) - at(base, i - left as isize);
            }
        }
    }

    #[test]
    fn plane_blur_is_the_box_passes_to_the_bit() {
        for (width, height) in [(1, 1), (1, 9), (9, 1), (37, 23), (70, 41)] {
            let plane: Vec<f32> = (0..width * height)
                .map(|i| ((i * 7919) % 251) as f32 * if i % 5 == 0 { 0.37 } else { 1.0 })
                .collect();
            for sigma in [1.0f32, 3.0, 8.0, 20.0] {
                let passes = windows(sigma).unwrap();
                let mut got = plane.clone();
                blur_plane(&mut got, width, height, passes);
                let mut want = plane.clone();
                let mut tmp = vec![0.0; want.len()];
                for window in passes {
                    box_pass(&want, &mut tmp, (width, 1, height, width), window);
                    std::mem::swap(&mut want, &mut tmp);
                }
                for window in passes {
                    box_pass(&want, &mut tmp, (height, width, width, 1), window);
                    std::mem::swap(&mut want, &mut tmp);
                }
                let same = got
                    .iter()
                    .zip(&want)
                    .all(|(a, b)| a.to_bits() == b.to_bits());
                assert!(same, "{width}×{height}, σ {sigma}");
            }
        }
    }

    #[test]
    fn blur_preserves_total_alpha_and_spreads() {
        let mut m = vec![0u8; 41 * 41];
        m[20 * 41 + 20] = 255;
        gaussian_blur_alpha(&mut m, 41, 41, 3.0);
        let sum: u32 = m.iter().map(|&v| v as u32).sum();
        assert!((sum as i32 - 255).abs() <= 41, "{sum}");
        assert!(m[20 * 41 + 20] < 64 && m[20 * 41 + 23] > 0);
    }

    #[test]
    fn blur_is_symmetric_for_odd_boxes() {
        let mut odd = vec![0u8; 31 * 31];
        odd[15 * 31 + 15] = 255;
        gaussian_blur_alpha(&mut odd, 31, 31, 8.0); // d = 15 → нечётное
        for d in 1..10 {
            assert_eq!(odd[15 * 31 + 15 - d], odd[15 * 31 + 15 + d], "сдвиг {d}");
        }
    }

    #[test]
    fn rgba_blur_keeps_opaque_color() {
        let mut image = Rgba8Image::new(16, 16);
        image.make_mut_slice().fill(Rgba::rgb(10, 200, 30));
        gaussian_blur_rgba(&mut image, 4.0);
        assert!(
            image
                .as_slice()
                .iter()
                .all(|p| *p == Rgba::rgb(10, 200, 30))
        );
    }

    #[test]
    fn rgba_blur_uses_premultiplied_colors() {
        // Прозрачный «красный» не окрашивает соседей: размывается только видимое.
        let mut image = Rgba8Image::new(9, 1);
        let px = image.make_mut_slice();
        px.fill(Rgba::new(255, 0, 0, 0));
        px[4] = Rgba::rgb(0, 0, 255);
        gaussian_blur_rgba(&mut image, 1.0);
        let p = image.as_slice()[3];
        assert!(p.a > 0 && p.r == 0 && p.b == 255, "{p:?}");
    }
}
