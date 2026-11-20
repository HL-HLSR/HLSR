//! Стекло темы Liquid Glass (`launcher.css` 0.4.9, `[data-theme="liquid-glass"]`): кусок подложки
//! блока с `backdrop-filter: blur(N) url(#lg-refract…) saturate(S) brightness(B)` и поверх него
//! заливка, подкраска `--lg-tint` и блик `--lg-sheen`. Шум преломления — из slint
//! (`render/refraction.rs`), без типов Slint; размытие под блоком — [`box_backdrop`] стекла колонки.
//!
//! - `feTurbulence type="fractalNoise" numOctaves="1" seed="17"` — опорная реализация SVG 1.1
//!   (§15.25), тот же генератор, что у Skia; шум берётся в CSS px от левого верхнего угла блока,
//!   координата вниз до целого, как в `GrPerlinNoise2Effect`.
//! - `feGaussianBlur` карты (σ 2–3 px) не делается: период шума — от 80 px, и размытие меняет карту
//!   меньше, чем на уровень байта (у края блока `feDisplacementMap` берёт непредумноженную карту,
//!   её значения у края те же).
//! - Размытие — подложки только под коробкой блока, за её краем зеркало (как у колонки, замер GL).
//! - `feDisplacementMap scale=… xChannelSelector="R" yChannelSelector="G"`: сдвиг выборки
//!   `scale · (c − ½)` CSS px. Выборка за коробкой блока прозрачна (вход фильтра — размытая
//!   подложка коробки), и там видна подложка без фильтра: сверено по снимку 0.4.9 — кнопка меню игры
//!   64 × 64 при `scale` 120 почти вся тёмная, как кадр рядом, а не светлая, как кадр в 60 px выше.
//! - `saturate()` — матрица `feColorMatrix type="saturate"`, `brightness()` — множитель; в sRGB.
//!
//! Кусок считается на сетке с шагом σ/4 CSS px, как кусок колонки ([`box_backdrop`]): размытое
//! преломлённое на ней не теряет, вид растягивает кусок на блок. Кусок шире блока на две клетки с
//! каждой стороны: видеокарта растягивает его с линейной выборкой, а края картинки (к ним
//! подмешиваются соседи по атласу) остаются за обрезкой блока.

use super::rail_glass::{Placement, area, box_backdrop_at, over};
use crate::{Rgba, Rgba8Image};
use std::sync::{Arc, Mutex};

const B_SIZE: usize = 0x100;
const BM: usize = 0xff;
const PERLIN_N: f64 = 4096.0;
const RAND_M: i64 = 2_147_483_647;
const RAND_A: i64 = 16807;
const RAND_Q: i64 = 127_773;
const RAND_R: i64 = 2836;

/// σ подложки в клетках сетки (как у колонки).
pub const SIGMA: f32 = 4.0;
/// Грубая сетка: σ — две клетки, клеток вчетверо меньше. Так считается первый кусок блока в кадре,
/// где блок появился, пока точный считается в фоне. На кадре игры с преломлением разница в среднем
/// меньше уровня, у 99 % точек — до 5 уровней (`coarse_piece_stays_close_to_the_fine_one`).
pub const SIGMA_COARSE: f32 = 2.0;
/// Клеток куска за краем блока с каждой стороны.
const PAD: f32 = 2.0;

/// Решётка и градиенты шума для одного `seed` (функция `init` из SVG 1.1).
pub struct Turbulence {
    lattice: [usize; B_SIZE + B_SIZE + 2],
    gradient: [[[f64; 2]; B_SIZE + B_SIZE + 2]; 4],
}

fn setup_seed(seed: i64) -> i64 {
    let mut s = seed;
    if s <= 0 {
        s = -(s % (RAND_M - 1)) + 1;
    }
    if s > RAND_M - 1 {
        s = RAND_M - 1;
    }
    s
}

fn random(seed: i64) -> i64 {
    let mut result = RAND_A * (seed % RAND_Q) - RAND_R * (seed / RAND_Q);
    if result <= 0 {
        result += RAND_M;
    }
    result
}

fn s_curve(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

impl Turbulence {
    pub fn new(seed: i64) -> Turbulence {
        let mut lattice = [0usize; B_SIZE + B_SIZE + 2];
        let mut gradient = [[[0.0f64; 2]; B_SIZE + B_SIZE + 2]; 4];
        let mut seed = setup_seed(seed);
        for channel in gradient.iter_mut() {
            for (i, cell) in channel.iter_mut().enumerate().take(B_SIZE) {
                lattice[i] = i;
                for v in cell.iter_mut() {
                    seed = random(seed);
                    *v = ((seed % (B_SIZE + B_SIZE) as i64) - B_SIZE as i64) as f64 / B_SIZE as f64;
                }
                let s = (cell[0] * cell[0] + cell[1] * cell[1]).sqrt();
                if s > 0.0 {
                    cell[0] /= s;
                    cell[1] /= s;
                }
            }
        }
        for i in (1..B_SIZE).rev() {
            let k = lattice[i];
            seed = random(seed);
            let j = (seed % B_SIZE as i64) as usize;
            lattice[i] = lattice[j];
            lattice[j] = k;
        }
        for i in 0..B_SIZE + 2 {
            lattice[B_SIZE + i] = lattice[i];
            for channel in gradient.iter_mut() {
                channel[B_SIZE + i] = channel[i];
            }
        }
        Turbulence { lattice, gradient }
    }

    fn noise2(&self, channel: usize, x: f64, y: f64) -> f64 {
        let setup = |v: f64| {
            let t = v + PERLIN_N;
            let b0 = (t as i64 as usize) & BM;
            let b1 = (b0 + 1) & BM;
            let r0 = t - (t as i64) as f64;
            (b0, b1, r0, r0 - 1.0)
        };
        let (bx0, bx1, rx0, rx1) = setup(x);
        let (by0, by1, ry0, ry1) = setup(y);
        let i = self.lattice[bx0];
        let j = self.lattice[bx1];
        let b00 = self.lattice[i + by0];
        let b10 = self.lattice[j + by0];
        let b01 = self.lattice[i + by1];
        let b11 = self.lattice[j + by1];
        let sx = s_curve(rx0);
        let sy = s_curve(ry0);
        let g = &self.gradient[channel];
        let u = rx0 * g[b00][0] + ry0 * g[b00][1];
        let v = rx1 * g[b10][0] + ry0 * g[b10][1];
        let a = lerp(sx, u, v);
        let u = rx0 * g[b01][0] + ry1 * g[b01][1];
        let v = rx1 * g[b11][0] + ry1 * g[b11][1];
        let b = lerp(sx, u, v);
        lerp(sy, a, b)
    }

    /// Значение `fractalNoise` канала (0 — R … 3 — A) в точке пользовательских координат,
    /// уже переведённое в 0…1: `(сумма + 1) / 2`.
    pub fn fractal(&self, channel: usize, x: f64, y: f64, freq: (f64, f64), octaves: u32) -> f64 {
        let (mut vx, mut vy) = (x * freq.0, y * freq.1);
        let (mut sum, mut ratio) = (0.0, 1.0);
        for _ in 0..octaves {
            sum += self.noise2(channel, vx, vy) / ratio;
            vx *= 2.0;
            vy *= 2.0;
            ratio *= 2.0;
        }
        ((sum + 1.0) / 2.0).clamp(0.0, 1.0)
    }
}

/// Фильтр `#lg-refract` / `#lg-refract-strong` (LauncherShell.vue 0.4.9).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Refraction {
    pub base_frequency: (f64, f64),
    pub scale: f32,
}

/// `#lg-refract`: окна.
pub const LG_REFRACT: Refraction = Refraction {
    base_frequency: (0.002, 0.006),
    scale: 70.0,
};

/// `#lg-refract-strong`: всплывающие панели и «мягкие» кнопки.
pub const LG_REFRACT_STRONG: Refraction = Refraction {
    base_frequency: (0.004, 0.012),
    scale: 120.0,
};

/// Цепочка `backdrop-filter` одной группы правил Liquid Glass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiquidFilter {
    pub blur: f32,
    pub refraction: Refraction,
    pub saturate: f32,
    pub brightness: f32,
}

/// `--lg-filter`: `blur(14px) url(#lg-refract) saturate(170%) brightness(1.06)`.
pub const LG_WINDOW: LiquidFilter = LiquidFilter {
    blur: 14.0,
    refraction: LG_REFRACT,
    saturate: 1.7,
    brightness: 1.06,
};

/// `--lg-filter-panel`: `blur(8px) url(#lg-refract-strong) saturate(160%)`.
pub const LG_PANEL: LiquidFilter = LiquidFilter {
    blur: 8.0,
    refraction: LG_REFRACT_STRONG,
    saturate: 1.6,
    brightness: 1.0,
};

/// `--lg-filter-soft`: `blur(6px) url(#lg-refract-strong) saturate(140%)`.
pub const LG_SOFT: LiquidFilter = LiquidFilter {
    blur: 6.0,
    refraction: LG_REFRACT_STRONG,
    saturate: 1.4,
    brightness: 1.0,
};

/// `backdrop-filter: blur(8px)` без преломления и цвета: подложка просмотра картинок новости
/// (`.news-viewer` NewsImageViewer.vue 0.4.9) — во всех темах, не только Liquid Glass.
pub const BLUR_8: LiquidFilter = LiquidFilter {
    blur: 8.0,
    refraction: Refraction {
        base_frequency: (0.0, 0.0),
        scale: 0.0,
    },
    saturate: 1.0,
    brightness: 1.0,
};

/// `--lg-tint`: `rgba(255, 255, 255, .07)`.
pub const LG_TINT: Rgba = Rgba::new(255, 255, 255, 18);

/// `--lg-sheen`: `linear-gradient(150deg, rgba(255,255,255,.14) 0%, rgba(255,255,255,.04) 30%,
/// rgba(255,255,255,0) 55%, rgba(255,255,255,.06) 100%)` — (доля, прозрачность белого).
pub const LG_SHEEN: [(f32, f32); 4] = [(0.0, 0.14), (0.3, 0.04), (0.55, 0.0), (1.0, 0.06)];

/// Что рисует блок поверх преломлённой подложки (прямая альфа): `background-color`, затем
/// `background-image: linear-gradient(--lg-tint), --lg-sheen` (у «мягкого» стекла — только своя
/// заливка).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paint {
    pub fill: Rgba,
    pub tint: Rgba,
    pub sheen: bool,
}

/// Готовый кусок: картинка (прямая альфа) и где она лежит в окне (CSS px; шире блока на
/// [`PAD`] клетки с каждой стороны).
#[derive(Debug, Clone)]
pub struct Piece {
    pub image: Rgba8Image,
    pub place: Placement,
}

/// Кусок стекла блока `rect` (CSS px окна) над кадром `source`, который лежит в окне в `frame`
/// (cover и параллакс — у хозяина фона). `top(x, y)` — что лежит над кадром в точке окна (CSS px,
/// прямая альфа): затемнение, передний план, подложка окна-диалога. `None` — блок пустой.
/// Сетка — `cells` клеток на σ размытия ([`SIGMA`] или [`SIGMA_COARSE`]).
pub fn piece_at(
    source: &Rgba8Image,
    frame: Placement,
    rect: Placement,
    filter: &LiquidFilter,
    cells: f32,
    top: impl Fn(f32, f32) -> Rgba,
    paint: &Paint,
) -> Option<Piece> {
    if rect.width < 1.0 || rect.height < 1.0 {
        return None;
    }
    let step = (filter.blur / cells).max(1.0);
    let pad = PAD * step;
    // Размытая подложка коробки блока (за краем — зеркало).
    let under = box_backdrop_at(
        source,
        Placement {
            x: frame.x - rect.x,
            y: frame.y - rect.y,
            ..frame
        },
        rect.width,
        rect.height,
        filter.blur,
        cells,
        |x, y| top(x + rect.x, y + rect.y),
    );
    let (uw, uh) = (under.width() as usize, under.height() as usize);
    let (ux, uy) = (rect.width / uw as f32, rect.height / uh as f32);
    // Подложка клетки без фильтра: там, куда выборка преломления ушла за коробку.
    let plain = |x: f32, y: f32| {
        let half = step / 2.0;
        let under = area(source, frame, (x - half, y - half), (x + half, y + half));
        let p = over(top(x, y), under);
        [p.r, p.g, p.b].map(|c| f32::from(c) / 255.0)
    };
    let out_w = ((rect.width + 2.0 * pad) / step).ceil().max(1.0) as usize;
    let out_h = ((rect.height + 2.0 * pad) / step).ceil().max(1.0) as usize;
    let place = Placement {
        x: rect.x - pad,
        y: rect.y - pad,
        width: out_w as f32 * step,
        height: out_h as f32 * step,
    };
    let refraction = filter.refraction;
    let shifts = shifts(out_w, out_h, step, pad, refraction);
    let matrix = saturate_matrix(filter.saturate);
    // Ось блика: 150deg — вниз и вправо; длина градиента по CSS.
    let (sin, cos) = 150f32.to_radians().sin_cos();
    let line = (rect.width * sin).abs() + (rect.height * cos).abs();
    let center = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
    let mut image = Rgba8Image::new(out_w as u32, out_h as u32);
    for (index, out) in image.make_mut_slice().iter_mut().enumerate() {
        let (i, j) = ((index % out_w) as f32, (index / out_w) as f32);
        let (x, y) = (place.x + (i + 0.5) * step, place.y + (j + 0.5) * step);
        let (dx, dy) = shifts[index];
        let (sx, sy) = (x + dx, y + dy);
        let inside = (rect.x..rect.x + rect.width).contains(&sx)
            && (rect.y..rect.y + rect.height).contains(&sy);
        let mut color = if inside {
            let [r, g, b] = bilinear(&under, (sx - rect.x) / ux - 0.5, (sy - rect.y) / uy - 0.5);
            matrix.map(|row| {
                let v = (row[0] * r + row[1] * g + row[2] * b).clamp(0.0, 1.0);
                (v * filter.brightness).clamp(0.0, 1.0)
            })
        } else {
            plain(x, y)
        };
        for layer in [paint.fill, paint.tint] {
            color = blend(color, layer);
        }
        if paint.sheen {
            let t = ((x - center.0) * sin - (y - center.1) * cos) / line + 0.5;
            color = blend(color, Rgba::new(255, 255, 255, alpha_byte(sheen_at(t))));
        }
        let [r, g, b] = color.map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8);
        *out = Rgba::rgb(r, g, b);
    }
    Some(Piece { image, place })
}

/// Сдвиги преломления клеток куска `out_w × out_h` (шаг `step`, поле `pad` за краем блока, CSS px).
/// Шум берётся от угла блока, поэтому сдвиги зависят только от размера куска и фильтра: блок, чья
/// подложка движется (параллакс), берёт их готовыми. Помнятся последние [`SHIFTS_KEPT`].
fn shifts(out_w: usize, out_h: usize, step: f32, pad: f32, refraction: Refraction) -> Shifts {
    // Без преломления сдвигов нет: шум не считается (подложка во всё окно — сотни тысяч клеток).
    if refraction.scale == 0.0 {
        return vec![(0.0, 0.0); out_w * out_h].into();
    }
    type Key = (usize, usize, u32, u32, u64, u64, u32);
    static CACHE: Mutex<Vec<(Key, Shifts)>> = Mutex::new(Vec::new());
    let (fx, fy) = refraction.base_frequency;
    let key = (
        out_w,
        out_h,
        step.to_bits(),
        pad.to_bits(),
        fx.to_bits(),
        fy.to_bits(),
        refraction.scale.to_bits(),
    );
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(at) = cache.iter().position(|(k, _)| *k == key) {
        let hit = cache.remove(at);
        let shifts = hit.1.clone();
        cache.push(hit);
        return shifts;
    }
    let noise = Turbulence::new(17);
    let shifts: Shifts = (0..out_w * out_h)
        .map(|index| {
            let (i, j) = ((index % out_w) as f32, (index / out_w) as f32);
            // Пользовательские координаты фильтра — CSS px от угла блока, вниз до целого (блок и
            // шаг — по сетке ¼ px, поэтому отсчёт от угла точен).
            let (bx, by) = (
                f64::from(((i + 0.5) * step - pad).floor()),
                f64::from(((j + 0.5) * step - pad).floor()),
            );
            let shift = |channel| {
                let c = noise.fractal(channel, bx, by, refraction.base_frequency, 1) as f32;
                refraction.scale * (c - 0.5)
            };
            (shift(0), shift(1))
        })
        .collect();
    if cache.len() >= SHIFTS_KEPT {
        cache.remove(0);
    }
    cache.push((key, shifts.clone()));
    shifts
}

/// Сдвиги преломления клеток куска (CSS px).
type Shifts = Arc<[(f32, f32)]>;

/// Сколько наборов сдвигов помнить: окна, панели и мягкие кнопки на экране.
const SHIFTS_KEPT: usize = 16;

fn alpha_byte(a: f32) -> u8 {
    (a * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Прозрачность блика в доле `t` линии градиента (до первого и после последнего стопа — крайние).
fn sheen_at(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    for pair in LG_SHEEN.windows(2) {
        let ((p0, a0), (p1, a1)) = (pair[0], pair[1]);
        if t <= p1 {
            return a0 + (a1 - a0) * (t - p0) / (p1 - p0);
        }
    }
    LG_SHEEN[LG_SHEEN.len() - 1].1
}

/// Непрозрачный цвет (0…1) под полупрозрачным `top`.
fn blend(under: [f32; 3], top: Rgba) -> [f32; 3] {
    let a = f32::from(top.a) / 255.0;
    let top = [top.r, top.g, top.b].map(|c| f32::from(c) / 255.0);
    [0, 1, 2].map(|k| under[k] + (top[k] - under[k]) * a)
}

/// Цвет картинки (0…1) в точке пикселей `(x, y)` (центры пикселей — целые), у края — крайний.
fn bilinear(image: &Rgba8Image, x: f32, y: f32) -> [f32; 3] {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let px = |x: usize, y: usize| {
        let p = image.as_slice()[y * w + x];
        [p.r, p.g, p.b].map(|c| f32::from(c) / 255.0)
    };
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    [0, 1, 2].map(|k| {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bottom = c[k] + (d[k] - c[k]) * fx;
        top + (bottom - top) * fy
    })
}

/// Матрица `saturate(s)` (Filter Effects, `feColorMatrix type="saturate"`).
fn saturate_matrix(s: f32) -> [[f32; 3]; 3] {
    [
        [0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s],
    ]
}

/// Пиксель картинки `image` (место в окне `place`, CSS px) в точке окна, прямая альфа; вне
/// картинки — прозрачный. Картинки `ImageLoader` предумножены — делится на альфу.
pub fn sample(image: &Rgba8Image, place: Placement, x: f32, y: f32) -> Rgba {
    let (w, h) = (image.width() as f32, image.height() as f32);
    if place.width <= 0.0 || place.height <= 0.0 || w == 0.0 || h == 0.0 {
        return Rgba::TRANSPARENT;
    }
    let u = (x - place.x) / place.width * w;
    let v = (y - place.y) / place.height * h;
    if u < 0.0 || v < 0.0 || u >= w || v >= h {
        return Rgba::TRANSPARENT;
    }
    let p = image.as_slice()[v as usize * image.width() as usize + u as usize];
    if !image.is_premultiplied() || p.a == 255 || p.a == 0 {
        return p;
    }
    let k = |c: u8| ((u32::from(c) * 255 + u32::from(p.a) / 2) / u32::from(p.a)).min(255) as u8;
    Rgba::new(k(p.r), k(p.g), k(p.b), p.a)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Placement = Placement {
        x: 0.0,
        y: 0.0,
        width: 1600.0,
        height: 900.0,
    };

    fn flat(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Rgba8Image {
        let mut image = Rgba8Image::new(width, height);
        for (index, p) in image.make_mut_slice().iter_mut().enumerate() {
            let [r, g, b] = pixel(index as u32 % width, index as u32 / width);
            *p = Rgba::rgb(r, g, b);
        }
        image
    }

    fn clear() -> Paint {
        Paint {
            fill: Rgba::TRANSPARENT,
            tint: Rgba::TRANSPARENT,
            sheen: false,
        }
    }

    fn none(_: f32, _: f32) -> Rgba {
        Rgba::TRANSPARENT
    }

    /// Точный кусок (сетка [`SIGMA`]).
    fn piece(
        source: &Rgba8Image,
        frame: Placement,
        rect: Placement,
        filter: &LiquidFilter,
        top: impl Fn(f32, f32) -> Rgba,
        paint: &Paint,
    ) -> Option<Piece> {
        piece_at(source, frame, rect, filter, SIGMA, top, paint)
    }

    /// Цвет куска в точке окна с линейной выборкой, как его растягивает видеокарта.
    fn linear(piece: &Piece, x: f32, y: f32) -> [f32; 3] {
        let (w, h) = (piece.image.width() as usize, piece.image.height() as usize);
        let u =
            ((x - piece.place.x) / piece.place.width * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
        let v =
            ((y - piece.place.y) / piece.place.height * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (i, j) = (u as usize, v as usize);
        let (fu, fv) = (u - i as f32, v - j as f32);
        let at = |i: usize, j: usize| piece.image.as_slice()[j.min(h - 1) * w + i.min(w - 1)];
        let mix = |c: fn(Rgba) -> u8| {
            let row = |j| f32::from(c(at(i, j))) * (1.0 - fu) + f32::from(c(at(i + 1, j))) * fu;
            row(j) * (1.0 - fv) + row(j + 1) * fv
        };
        [mix(|p| p.r), mix(|p| p.g), mix(|p| p.b)]
    }

    /// `.news-viewer`: только размытие — ровный цветной кадр не меняется (ни сдвигов, ни
    /// насыщенности), резкая граница расплывается на σ 8 px, сверху ложится заливка.
    #[test]
    fn blur_8_only_blurs_under_the_fill() {
        let even = flat(160, 90, |_, _| [200, 50, 30]);
        let plain = piece(&even, WINDOW, WINDOW, &BLUR_8, none, &clear()).unwrap();
        let [r, g, b] = linear(&plain, 400.0, 300.0);
        assert!((r - 200.0).abs() < 1.0 && (g - 50.0).abs() < 1.0 && (b - 30.0).abs() < 1.0);

        let edge = flat(
            160,
            90,
            |x, _| if x < 80 { [0, 0, 0] } else { [255, 255, 255] },
        );
        let fill = Paint {
            fill: Rgba::new(5, 8, 11, 224),
            ..clear()
        };
        let piece = piece(&edge, WINDOW, WINDOW, &BLUR_8, none, &fill).unwrap();
        let at = |x: f32| linear(&piece, x, 450.0)[0];
        let k = 224.0 / 255.0;
        let (dark, light) = (5.0 * k, 255.0 + (5.0 - 255.0) * k);
        assert!((at(100.0) - dark).abs() < 1.0, "{}", at(100.0));
        assert!((at(1500.0) - light).abs() < 1.0, "{}", at(1500.0));
        let middle = (dark + light) / 2.0;
        assert!((at(800.0) - middle).abs() < 3.0, "{}", at(800.0));
        // σ слева от границы: около 16 % светлого.
        let near = at(792.0);
        assert!(near > dark + 2.0 && near < middle - 5.0, "{near}");
    }

    #[test]
    fn coarse_piece_stays_close_to_the_fine_one() {
        // Настоящий кадр игры (`hero-hl2.jpg`) во всё окно, блок с преломлением каждой группы.
        // Замер (release, ещё окно 1040×720 и кнопка 64×72): среднее 0,35–0,7 уровня, 99 % точек —
        // до 5 уровней, наибольшее — 12 у края блока, где сдвиг выносит выборку за блок. Шахматка
        // 40 px двух цветов (худшее): среднее 3–5 уровней.
        let frame = image::load_from_memory(include_bytes!("../../assets/launcher/hero-hl2.jpg"))
            .unwrap()
            .to_rgba8();
        let source = flat(frame.width(), frame.height(), |x, y| {
            let [r, g, b, _] = frame.get_pixel(x, y).0;
            [r, g, b]
        });
        let rect = Placement {
            x: 500.0,
            y: 300.0,
            width: 300.0,
            height: 200.0,
        };
        for filter in [LG_WINDOW, LG_PANEL, LG_SOFT] {
            let fine = piece(&source, WINDOW, rect, &filter, none, &clear()).unwrap();
            let coarse =
                piece_at(&source, WINDOW, rect, &filter, SIGMA_COARSE, none, &clear()).unwrap();
            let mut diffs = Vec::new();
            for j in 0..rect.height as usize {
                for i in 0..rect.width as usize {
                    let (x, y) = (rect.x + i as f32 + 0.5, rect.y + j as f32 + 0.5);
                    let (a, b) = (linear(&fine, x, y), linear(&coarse, x, y));
                    diffs.push((0..3).map(|c| (a[c] - b[c]).abs()).fold(0.0, f32::max));
                }
            }
            diffs.sort_by(f32::total_cmp);
            let mean = diffs.iter().sum::<f32>() / diffs.len() as f32;
            let p99 = diffs[diffs.len() * 99 / 100];
            assert!(mean < 1.0 && p99 < 6.0, "σ {}: {mean} {p99}", filter.blur);
        }
    }

    #[test]
    fn lcg_matches_svg_reference_sequence() {
        // random(): «минимальный стандарт» Парка — Миллера, x₁ = 16807·x₀ mod (2³¹ − 1).
        assert_eq!(random(1), 16807);
        assert_eq!(random(16807), 282_475_249);
        assert_eq!(setup_seed(0), 1);
        assert_eq!(setup_seed(-5), 6);
    }

    #[test]
    fn noise_is_half_on_lattice_points_and_smooth_between() {
        let t = Turbulence::new(17);
        for ch in 0..4 {
            assert!((t.fractal(ch, 3.0, 5.0, (1.0, 1.0), 1) - 0.5).abs() < 1e-12);
        }
        let a = t.fractal(0, 100.0, 40.0, (0.002, 0.006), 1);
        let b = t.fractal(0, 101.0, 40.0, (0.002, 0.006), 1);
        assert!((a - b).abs() < 0.01, "{a} {b}");
        assert_ne!(
            a,
            Turbulence::new(18).fractal(0, 100.0, 40.0, (0.002, 0.006), 1)
        );
    }

    #[test]
    fn piece_covers_the_box_with_two_cells_around() {
        let source = flat(160, 90, |_, _| [40, 80, 120]);
        let rect = Placement {
            x: 280.0,
            y: 90.0,
            width: 1040.0,
            height: 720.0,
        };
        let piece = piece(&source, WINDOW, rect, &LG_WINDOW, none, &clear()).unwrap();
        // σ 14 → клетка 3,5 CSS px; по 2 клетки с каждой стороны.
        assert_eq!(piece.place.x, 273.0);
        assert_eq!(piece.place.y, 83.0);
        assert_eq!((piece.image.width(), piece.image.height()), (302, 210));
        assert!(piece.place.width >= rect.width + 14.0);
        let empty = Placement { width: 0.5, ..rect };
        assert!(self::piece(&source, WINDOW, empty, &LG_WINDOW, none, &clear()).is_none());
    }

    #[test]
    fn flat_backdrop_is_only_coloured_by_the_filter() {
        // Серый остаётся серым (насыщенность серого не меняет), brightness(1.06) светлее.
        let source = flat(160, 90, |_, _| [100, 100, 100]);
        let rect = Placement {
            x: 400.0,
            y: 200.0,
            width: 300.0,
            height: 200.0,
        };
        let window = piece(&source, WINDOW, rect, &LG_WINDOW, none, &clear()).unwrap();
        // Выборка за коробкой — подложка без фильтра (100), внутри — с brightness (106).
        let pixels = window.image.as_slice();
        assert!(
            pixels
                .iter()
                .all(|p| p.r == p.b && (p.r == 100 || p.r == 106))
        );
        let middle = pixels[pixels.len() / 2 + window.image.width() as usize / 2];
        assert_eq!(middle, Rgba::rgb(106, 106, 106));
        let panel = piece(&source, WINDOW, rect, &LG_PANEL, none, &clear()).unwrap();
        assert!(
            panel
                .image
                .as_slice()
                .iter()
                .all(|p| *p == Rgba::rgb(100, 100, 100))
        );
        // Насыщенность раздвигает каналы цветного.
        let tinted = flat(160, 90, |_, _| [120, 100, 80]);
        let soft = piece(&tinted, WINDOW, rect, &LG_SOFT, none, &clear()).unwrap();
        let pixels = soft.image.as_slice();
        let p = pixels[pixels.len() / 2 + soft.image.width() as usize / 2];
        assert!(p.r > 120 && p.b < 80, "{p:?}");
    }

    #[test]
    fn refraction_moves_what_is_under_the_box() {
        // Вертикальные полосы по 20 CSS px: без преломления столбец клеток ровный сверху вниз,
        // с ним — полосы гнутся (по высоте шум меняется втрое быстрее).
        let source = flat(1600, 900, |x, _| {
            if x / 40 % 2 == 0 {
                [30, 30, 30]
            } else {
                [220, 220, 220]
            }
        });
        let rect = Placement {
            x: 300.0,
            y: 100.0,
            width: 600.0,
            height: 600.0,
        };
        let soft = piece(&source, WINDOW, rect, &LG_SOFT, none, &clear()).unwrap();
        let w = soft.image.width() as usize;
        let column: Vec<u8> = (0..soft.image.height() as usize)
            .map(|j| soft.image.as_slice()[j * w + w / 2].r)
            .collect();
        let (lo, hi) = (column.iter().min().unwrap(), column.iter().max().unwrap());
        assert!(hi - lo > 60, "полосы не гнутся: {lo}…{hi}");
    }

    #[test]
    fn samples_beyond_the_box_show_the_backdrop_without_the_filter() {
        // Кнопка 64 × 64 у нижнего края светлого: над ней (60 px) светло, под ней темно. Сильное
        // преломление (`scale` 120) уводит выборку за коробку — там подложка без фильтра (тёмная),
        // светлое сверху в кусок не попадает.
        let source = flat(1600, 900, |_, y| {
            if y < 760 {
                [220, 220, 220]
            } else {
                [20, 20, 20]
            }
        });
        let rect = Placement {
            x: 1200.0,
            y: 800.0,
            width: 64.0,
            height: 64.0,
        };
        let soft = piece(&source, WINDOW, rect, &LG_SOFT, none, &clear()).unwrap();
        assert!(
            soft.image.as_slice().iter().all(|p| p.r < 40),
            "сверху подмешалось светлое"
        );
    }

    #[test]
    fn top_layers_lie_over_the_frame_before_the_blur() {
        // Подложка окна-диалога наполовину чёрная: кусок вдвое темнее (без фильтра цвета).
        let source = flat(160, 90, |_, _| [200, 200, 200]);
        let rect = Placement {
            x: 400.0,
            y: 200.0,
            width: 300.0,
            height: 200.0,
        };
        let filter = LiquidFilter {
            saturate: 1.0,
            brightness: 1.0,
            ..LG_PANEL
        };
        let half = |_: f32, _: f32| Rgba::new(0, 0, 0, 128);
        let out = piece(&source, WINDOW, rect, &filter, half, &clear()).unwrap();
        assert!(out.image.as_slice().iter().all(|p| p.r.abs_diff(100) <= 1));
    }

    #[test]
    fn fill_tint_and_sheen_follow_css() {
        // rgba(18,22,30,.58) поверх чёрного, затем белый .07 и блик 150deg: левый верх светлее
        // правого низа (.14 против .06), середина — без блика.
        let source = flat(160, 90, |_, _| [0, 0, 0]);
        let rect = Placement {
            x: 400.0,
            y: 200.0,
            width: 400.0,
            height: 300.0,
        };
        let paint = Paint {
            fill: Rgba::new(18, 22, 30, 148),
            tint: LG_TINT,
            sheen: true,
        };
        let out = piece(&source, WINDOW, rect, &LG_WINDOW, none, &paint).unwrap();
        let w = out.image.width() as usize;
        let at = |i: usize, j: usize| out.image.as_slice()[j * w + i];
        let (corner, middle, end) = (
            at(2, 2),
            at(w / 2, out.image.height() as usize / 2),
            at(w - 3, out.image.height() as usize - 3),
        );
        // Без блика: 0 → 10/13/17 (заливка) → +7 % белого.
        let base: f32 = 30.0 * 148.0 / 255.0;
        let expect = (base + (255.0 - base) * (18.0 / 255.0)).round() as u8;
        // В середине блика почти нет (.008 у t = ½).
        assert!(middle.b.abs_diff(expect) <= 3, "{middle:?} против {expect}");
        assert!(
            corner.b > end.b && end.b > middle.b,
            "{corner:?} {middle:?} {end:?}"
        );
    }

    #[test]
    fn sheen_stops_match_the_gradient() {
        assert!((sheen_at(0.0) - 0.14).abs() < 1e-6);
        assert!((sheen_at(0.15) - 0.09).abs() < 1e-6);
        assert_eq!(sheen_at(0.55), 0.0);
        assert!((sheen_at(1.0) - 0.06).abs() < 1e-6);
        assert!((sheen_at(2.0) - 0.06).abs() < 1e-6);
    }

    #[test]
    fn sample_reads_straight_alpha_and_nothing_outside() {
        let mut image = Rgba8Image::new(2, 1);
        image.make_mut_slice()[1] = Rgba::new(50, 25, 0, 128);
        let image = image.premultiplied();
        let place = Placement {
            x: 10.0,
            y: 10.0,
            width: 20.0,
            height: 10.0,
        };
        assert_eq!(sample(&image, place, 5.0, 15.0), Rgba::TRANSPARENT);
        assert_eq!(
            sample(&image, place, 25.0, 15.0),
            Rgba::new(100, 50, 0, 128)
        );
    }
    /// Цена куска (замер для отчёта, не проверка): `cargo test --release -p hlsr-ui --lib
    /// piece_cost -- --ignored --nocapture`. Кадр 2560 × 1440 в окне 1600 × 900.
    #[test]
    #[ignore]
    fn piece_cost() {
        let source = flat(2560, 1440, |x, y| [(x % 251) as u8, (y % 241) as u8, 90]);
        let cases = [
            ("окно 1040×720", LG_WINDOW, (280.0, 90.0, 1040.0, 720.0)),
            ("меню 260×170", LG_PANEL, (1150.0, 80.0, 260.0, 170.0)),
            ("кнопка 64×72", LG_SOFT, (1192.0, 808.0, 64.0, 72.0)),
            ("сборка 304×64", LG_SOFT, (1272.0, 728.0, 304.0, 64.0)),
        ];
        for (name, filter, (x, y, width, height)) in cases {
            let rect = Placement {
                x,
                y,
                width,
                height,
            };
            let start = std::time::Instant::now();
            let runs = 20;
            for _ in 0..runs {
                std::hint::black_box(piece(&source, WINDOW, rect, &filter, none, &clear()));
            }
            println!(
                "{name}: {:.2} мс",
                start.elapsed().as_secs_f64() * 1000.0 / runs as f64
            );
        }
    }
}
