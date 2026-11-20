//! Треугольники заставки (`components/LoadingIndicator.vue` 0.4.7): раскладка 24 стрелок и
//! растр стрелки со свечением.
//!
//! В 0.4.7 стрелка — блок нулевого размера с границами `border-left/right: 18px transparent` и
//! `border-bottom: 32.4px solid`, повёрнутый на 180° у `.down`, с `filter: drop-shadow(0 0 27px)`
//! и анимацией `opacity` (мигание, `blink`). Chromium округляет ширину границы вниз до целых
//! физических пикселей (18 px при 1,25 — 22 физ., 32,4 px — 40 физ.), поэтому блок 36 × 32,4
//! не получается ни при каком масштабе; рамка блока на экране привязывается к пикселям
//! ([`Arrow::snapped`]). Свечение — картинка (σ = 27 px, размытие на ЦП). Треугольник и свечение лежат в одной картинке: прозрачность мигания
//! действует на оба разом, как `opacity` на элементе с `filter`.

use super::shadow_blur::gaussian_blur_alpha;
use crate::{Rgba as Color, Rgba8Image};

/// Картинка стрелки и где лежит её левый верхний угол относительно блока (логические px).
#[derive(Debug, Clone, Default)]
pub struct ArrowRaster {
    /// Без предумножения.
    pub pixels: Rgba8Image,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// `$size` стрелки, px.
const SIZE: f64 = 18.0;
/// Высота границы `$size * 1.8`, px.
const HEIGHT: f64 = SIZE * 1.8;
/// Стрелок в самом широком ряду.
const WIDEST: usize = 7;

/// Стрелка: направление и место в анимации (`outer-N` / `inner-N`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub down: bool,
    /// Класс `outer` (иначе `inner`).
    pub outer: bool,
    /// Номер в классе: `--i` задержки анимации.
    pub index: i32,
}

const fn up(outer: bool, index: i32) -> Slot {
    Slot {
        down: false,
        outer,
        index,
    }
}

const fn down(outer: bool, index: i32) -> Slot {
    Slot {
        down: true,
        outer,
        index,
    }
}

/// Ряды сверху вниз, как в шаблоне `LoadingIndicator.vue`.
pub const ROWS: [&[Slot]; 4] = [
    &[
        up(true, 18),
        down(true, 17),
        up(true, 16),
        down(true, 15),
        up(true, 14),
    ],
    &[
        up(true, 1),
        down(true, 2),
        up(false, 6),
        down(false, 5),
        up(false, 4),
        down(true, 13),
        up(true, 12),
    ],
    &[
        down(true, 3),
        up(true, 4),
        down(false, 1),
        up(false, 2),
        down(false, 3),
        up(true, 11),
        down(true, 10),
    ],
    &[
        down(true, 5),
        up(true, 6),
        down(true, 7),
        up(true, 8),
        down(true, 9),
    ],
];

/// Рамка стрелки в логических px окна.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arrow {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub slot: Slot,
}

/// Размер блока стрелки в логических px: границы округлены вниз до физических пикселей.
fn box_size(scale: f64) -> (f64, f64) {
    let side = (SIZE * scale).floor();
    let bottom = (HEIGHT * scale).floor();
    (2.0 * side / scale, bottom / scale)
}

/// Раскладка индикатора в окне `window` (логические px): индикатор по центру `#content`, ряды
/// по центру индикатора, у стрелок `margin: 0 -9px`. Рамки не округлены (как `getBoundingClientRect`).
pub fn layout(window: (f64, f64), scale: f64) -> Vec<Arrow> {
    let (width, height) = box_size(scale);
    let advance = width - SIZE;
    let indicator_width = WIDEST as f64 * advance;
    let indicator_height = ROWS.len() as f64 * height;
    let left = (window.0 - indicator_width) / 2.0;
    let top = (window.1 - indicator_height) / 2.0;
    let mut arrows = Vec::with_capacity(24);
    for (row, slots) in ROWS.iter().enumerate() {
        let row_width = slots.len() as f64 * advance;
        let row_left = left + (indicator_width - row_width) / 2.0;
        for (column, slot) in slots.iter().enumerate() {
            arrows.push(Arrow {
                x: row_left + column as f64 * advance - SIZE / 2.0,
                y: top + row as f64 * height,
                width,
                height,
                slot: *slot,
            });
        }
    }
    arrows
}

impl Arrow {
    /// Рамка, привязанная к физическим пикселям: края округляются по отдельности
    /// (`PixelSnappedIntRect`).
    pub fn snapped(&self, scale: f64) -> Arrow {
        let snap = |value: f64| (value * scale + 0.5).floor();
        let (x0, x1) = (snap(self.x), snap(self.x + self.width));
        let (y0, y1) = (snap(self.y), snap(self.y + self.height));
        Arrow {
            x: x0 / scale,
            y: y0 / scale,
            width: (x1 - x0) / scale,
            height: (y1 - y0) / scale,
            slot: self.slot,
        }
    }
}

/// Покрытие треугольника в клетке `(x, y)` пикселей (сетка 8 × 8 выборок): вершина сверху по
/// центру, основание внизу; `down` — наоборот.
fn coverage(x: usize, y: usize, width: f64, height: f64, down: bool) -> f32 {
    const N: usize = 8;
    let mut inside = 0;
    for sy in 0..N {
        for sx in 0..N {
            let px = x as f64 + (sx as f64 + 0.5) / N as f64;
            let mut py = y as f64 + (sy as f64 + 0.5) / N as f64;
            if down {
                py = height - py;
            }
            if py < 0.0 || py > height {
                continue;
            }
            // Ширина треугольника на высоте py растёт от 0 у вершины до `width` у основания.
            let half = width / 2.0 * (py / height);
            if (px - width / 2.0).abs() <= half {
                inside += 1;
            }
        }
    }
    inside as f32 / (N * N) as f32
}

/// Стрелка `width × height` (логические px, уже привязанные к пикселям) цвета `color` со
/// свечением `drop-shadow(0 0 blur color)`; `x`, `y` — где левый верхний угол картинки
/// относительно блока стрелки.
pub fn arrow_with_glow(
    width: f32,
    height: f32,
    down: bool,
    blur: f32,
    color: Color,
    scale: f32,
) -> ArrowRaster {
    if !(width > 0.0 && height > 0.0 && scale > 0.0) || color.a == 0 {
        return ArrowRaster::default();
    }
    let sigma = blur * scale;
    let pad = (3.0 * sigma).ceil() as usize + 1;
    let (bw, bh) = (
        (f64::from(width) * f64::from(scale)).round(),
        (f64::from(height) * f64::from(scale)).round(),
    );
    let w = 2 * pad + bw as usize;
    let h = 2 * pad + bh as usize;

    // Покрытие самой стрелки в клетках картинки.
    let mut shape = vec![0.0_f32; w * h];
    for y in 0..bh as usize {
        for x in 0..bw as usize {
            shape[(pad + y) * w + pad + x] = coverage(x, y, bw, bh, down);
        }
    }
    // Тень — размытый силуэт.
    let mut mask: Vec<u8> = shape.iter().map(|c| (c * 255.0).round() as u8).collect();
    if sigma > 0.0 {
        gaussian_blur_alpha(&mut mask, w as u32, h as u32, sigma);
    }

    let tint = [
        f32::from(color.r) / 255.0,
        f32::from(color.g) / 255.0,
        f32::from(color.b) / 255.0,
    ];
    let alpha = f32::from(color.a) / 255.0;
    let mut pixels = Rgba8Image::new(w as u32, h as u32);
    for (index, out) in pixels.make_mut_slice().iter_mut().enumerate() {
        // Премультиплицированные доли: тень, затем стрелка поверх.
        let glow = f32::from(mask[index]) / 255.0 * alpha;
        let body = shape[index] * alpha;
        let a = body + glow * (1.0 - body);
        *out = if a <= 0.0 {
            Color::TRANSPARENT
        } else {
            // Цвет один: премультиплицированный цвет = tint × a, без потерь.
            let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            Color::new(q(tint[0]), q(tint[1]), q(tint[2]), q(a))
        };
    }
    ArrowRaster {
        pixels,
        x: -(pad as f32) / scale,
        y: -(pad as f32) / scale,
        width: w as f32 / scale,
        height: h as f32 / scale,
    }
}
