//! Сравнение экранов по маскам текста (решение 9): маска `<вариант>.text.png` эталона — белые
//! прямоугольники текстовых узлов, полей ввода и глифов FontAwesome на чёрном. Вне маски пиксель
//! отличается, если хоть один канал (с прозрачностью) расходится больше порога; в маске — то же
//! после гауссова размытия обеих картинок σ = 1 физ. px (сглаживание FemtoVG и Skia разное, а
//! размытие оставляет положение, размер и насыщенность букв). Доли — от числа пикселей своей
//! области; сцена проходит, если обе не больше 1 %. Области без сравнения (`ignore`) в доли не
//! входят.

use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

/// Прямоугольник снимка в физ. px: левый верхний угол и размер.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    fn contains(&self, x: u32, y: u32) -> bool {
        x >= self.x && x - self.x < self.width && y >= self.y && y - self.y < self.height
    }
}

/// Итог сравнения одного варианта.
#[derive(Debug, Clone, PartialEq)]
pub struct MaskedDiff {
    /// Размеры совпали.
    pub same_size: bool,
    /// Доля отличающихся пикселей вне маски.
    pub outside: f64,
    /// Доля отличающихся пикселей маски до размытия (справочно).
    pub inside_raw: f64,
    /// Доля отличающихся пикселей маски после размытия.
    pub inside: f64,
    /// Пикселей маски.
    pub mask_pixels: usize,
    /// Пикселей областей без сравнения.
    pub ignored: usize,
    /// Наибольшее отличие канала вне маски.
    pub max_outside: u8,
    /// Карта отличий.
    pub map: RgbaImage,
}

fn differs(a: &Rgba<u8>, b: &Rgba<u8>, threshold: u8) -> bool {
    a.0.iter()
        .zip(b.0.iter())
        .any(|(x, y)| x.abs_diff(*y) > threshold)
}

fn in_mask(mask: Option<&RgbaImage>, x: u32, y: u32) -> bool {
    mask.is_some_and(|m| m.get_pixel(x, y)[0] > 127)
}

fn blurred(img: &RgbaImage, sigma: f32) -> RgbaImage {
    let mut buf = hlsr_ui::Rgba8Image::from_bytes(img.width(), img.height(), img.as_raw());
    hlsr_ui::render::shadow_blur::gaussian_blur_rgba(&mut buf, sigma);
    RgbaImage::from_raw(img.width(), img.height(), buf.to_bytes()).expect("размер буфера")
}

/// Сравнить `actual` с эталоном `expected` по маске `mask` (нет маски или она другого размера —
/// вся картинка «вне маски»). Заодно — карта отличий: вне маски — красным, в маске (после
/// размытия) — жёлтым, маска без отличий — синеватой, остальное — затемнённый эталон; разные
/// размеры — эталон целиком красный.
#[cfg(test)]
pub fn masked_diff(
    expected: &RgbaImage,
    actual: &RgbaImage,
    mask: Option<&RgbaImage>,
    threshold: u8,
    sigma: f32,
) -> MaskedDiff {
    masked_diff_except(expected, actual, mask, &[], threshold, sigma)
}

/// [`masked_diff`] без областей `ignore`: их пиксели не сравниваются и в доли не входят (на карте —
/// затемнённый эталон с зелёным оттенком).
pub fn masked_diff_except(
    expected: &RgbaImage,
    actual: &RgbaImage,
    mask: Option<&RgbaImage>,
    ignore: &[Region],
    threshold: u8,
    sigma: f32,
) -> MaskedDiff {
    if expected.dimensions() != actual.dimensions() || expected.width() == 0 {
        return MaskedDiff {
            same_size: false,
            outside: 1.0,
            inside_raw: 1.0,
            inside: 1.0,
            mask_pixels: 0,
            ignored: 0,
            max_outside: 255,
            map: RgbaImage::from_pixel(expected.width(), expected.height(), Rgba([255, 0, 0, 255])),
        };
    }
    let mask = mask.filter(|m| m.dimensions() == expected.dimensions());
    let (be, ba) = match mask {
        Some(_) => (blurred(expected, sigma), blurred(actual, sigma)),
        None => (RgbaImage::new(0, 0), RgbaImage::new(0, 0)),
    };
    let mut map = RgbaImage::new(expected.width(), expected.height());
    let dim = |v: u8| (u16::from(v) * 35 / 100) as u8;
    let (mut out_n, mut out_d, mut in_n, mut in_raw, mut in_d, mut max) = (0, 0, 0, 0, 0, 0u8);
    let mut ignored = 0;
    for (x, y, e) in expected.enumerate_pixels() {
        let a = actual.get_pixel(x, y);
        let shown = if ignore.iter().any(|r| r.contains(x, y)) {
            ignored += 1;
            Rgba([dim(e[0]), dim(e[1]).saturating_add(60), dim(e[2]), 255])
        } else if in_mask(mask, x, y) {
            in_n += 1;
            if differs(e, a, threshold) {
                in_raw += 1;
            }
            if differs(be.get_pixel(x, y), ba.get_pixel(x, y), threshold) {
                in_d += 1;
                Rgba([255, 220, 0, 255])
            } else {
                Rgba([dim(e[0]), dim(e[1]), dim(e[2]).saturating_add(60), 255])
            }
        } else {
            out_n += 1;
            for (p, q) in e.0.iter().zip(a.0.iter()) {
                max = max.max(p.abs_diff(*q));
            }
            if differs(e, a, threshold) {
                out_d += 1;
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([dim(e[0]), dim(e[1]), dim(e[2]), 255])
            }
        };
        map.put_pixel(x, y, shown);
    }
    let ratio = |n: usize, all: usize| if all == 0 { 0.0 } else { n as f64 / all as f64 };
    MaskedDiff {
        same_size: true,
        outside: ratio(out_d, out_n),
        inside_raw: ratio(in_raw, in_n),
        inside: ratio(in_d, in_n),
        mask_pixels: in_n,
        ignored,
        max_outside: max,
        map,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask_left_half(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, _| {
            if x < w / 2 {
                Rgba([255, 255, 255, 255])
            } else {
                Rgba([0, 0, 0, 255])
            }
        })
    }

    #[test]
    fn outside_ratio_counts_only_unmasked_pixels() {
        let a = RgbaImage::from_pixel(10, 10, Rgba([100, 100, 100, 255]));
        let mut b = a.clone();
        // Справа (вне маски) — 1 пиксель из 50 = 2 %.
        b.put_pixel(9, 9, Rgba([200, 100, 100, 255]));
        let d = masked_diff(&a, &b, Some(&mask_left_half(10, 10)), 32, 1.0);
        assert!(d.same_size);
        assert_eq!(d.outside, 0.02);
        assert_eq!(d.inside, 0.0);
        assert_eq!(d.mask_pixels, 50);
        assert_eq!(d.max_outside, 100);
        assert_eq!(d.map.get_pixel(9, 9), &Rgba([255, 0, 0, 255]));
        // Маска без отличий — синеватая, вне маски без отличий — затемнённый эталон.
        assert_eq!(d.map.get_pixel(0, 0), &Rgba([35, 35, 95, 255]));
        assert_eq!(d.map.get_pixel(8, 0), &Rgba([35, 35, 35, 255]));
    }

    #[test]
    fn blur_forgives_antialiasing_but_not_a_moved_glyph() {
        let w = 40;
        let base = RgbaImage::from_pixel(w, 20, Rgba([0, 0, 0, 255]));
        let mut glyph = base.clone();
        for y in 5..15 {
            for x in 5..8 {
                glyph.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        // Сглаживание: край штриха на 50/255 светлее — в каждом пикселе края больше порога.
        let mut soft = glyph.clone();
        for y in 5..15 {
            soft.put_pixel(4, y, Rgba([50, 50, 50, 255]));
            soft.put_pixel(8, y, Rgba([50, 50, 50, 255]));
        }
        let mask = mask_left_half(w, 20);
        let d = masked_diff(&glyph, &soft, Some(&mask), 32, 1.0);
        assert!(d.inside_raw >= 0.05);
        assert_eq!(d.inside, 0.0);
        // Буква сдвинута на 4 px — остаётся и после размытия.
        let mut moved = base.clone();
        for y in 5..15 {
            for x in 9..12 {
                moved.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        assert!(masked_diff(&glyph, &moved, Some(&mask), 32, 1.0).inside > 0.05);
    }

    #[test]
    fn ignored_regions_are_not_compared_nor_counted() {
        let a = RgbaImage::from_pixel(10, 10, Rgba([100, 100, 100, 255]));
        let mut b = a.clone();
        // Блок Slint 3×3 в левом верхнем углу — весь другой; вне его — 1 пиксель из 91.
        for y in 0..3 {
            for x in 0..3 {
                b.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        b.put_pixel(9, 9, Rgba([200, 100, 100, 255]));
        let block = Region {
            x: 0,
            y: 0,
            width: 3,
            height: 3,
        };
        let d = masked_diff_except(&a, &b, None, &[block], 32, 1.0);
        assert_eq!(d.ignored, 9);
        assert_eq!(d.outside, 1.0 / 91.0);
        assert_eq!(d.max_outside, 100);
        assert_eq!(d.map.get_pixel(2, 2), &Rgba([35, 95, 35, 255]));
        assert_eq!(d.map.get_pixel(3, 2), &Rgba([35, 35, 35, 255]));
        // Без областей — те же 10 пикселей против всех 100.
        let all = masked_diff(&a, &b, None, 32, 1.0);
        assert_eq!((all.ignored, all.outside), (0, 0.1));
    }

    #[test]
    fn size_mismatch_fails_everything() {
        let a = RgbaImage::new(4, 4);
        let d = masked_diff(&a, &RgbaImage::new(5, 4), None, 32, 1.0);
        assert!(!d.same_size);
        assert_eq!(d.outside, 1.0);
        assert_eq!(d.map.get_pixel(0, 0), &Rgba([255, 0, 0, 255]));
    }
}
