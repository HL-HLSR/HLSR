//! Растр Canvas 2D на ЦП для превью персонализации (задачи 33 и 34): картинки и
//! смешивание с теми же округлениями, что у Skia в Chromium, где кадр 0.4.7
//! сравнивается попиксельно (превью HL1 и прицелы BXT).
//!
//! Подобрано по эталону (`tools/reference/snap-previews.js`, превью HL1 без
//! масштаба совпадает до пикселя):
//! - декодер Blink предумножает PNG и WebP как `SkMulDiv255Round` —
//!   `round(c · a / 255)`;
//! - `drawImage` без масштаба в целую точку — спрайт Skia (`SkPMSrcOver`):
//!   `d = s + (d · (256 − sa)) >> 8`;
//! - `multiply` и `lighter` — конвейер lowp: `div255(v) = (v + 255) >> 8`,
//!   `lighter` — сумма с насыщением;
//! - `fillRect` сплошным цветом с `globalAlpha` — `SkBlitRow::Color32`:
//!   `d = c + (d · (256 − a)) >> 8`, `c` — предумноженный цвет.
//!
//! Масштабирование (`imageSmoothingQuality: 'high'`) и поворот — tiny-skia
//! (бикубический фильтр Митчелла, как `kHigh` Skia при увеличении); там кадр
//! сверяется с допуском.

use crate::{Rgba, Rgba8Image};
use tiny_skia::{
    BlendMode, FilterQuality, IntSize, Pixmap, PixmapPaint, PremultipliedColorU8, Transform,
};

/// Ошибка картинки ассета.
#[derive(Debug, thiserror::Error)]
#[error("картинка превью «{name}»: {reason}")]
pub struct RasterError {
    pub name: String,
    pub reason: String,
}

/// `round(c · a / 255)` — предумножение Blink (`SkMulDiv255Round`).
pub fn premultiply(c: u8, a: u8) -> u8 {
    let v = u32::from(c) * u32::from(a) + 128;
    ((v + (v >> 8)) >> 8) as u8
}

/// PNG или WebP → предумноженный RGBA (оттенки серого и RGB — непрозрачные).
pub fn decode(name: &str, bytes: &[u8]) -> Result<Pixmap, RasterError> {
    let image = image::load_from_memory(bytes)
        .map_err(|e| RasterError {
            name: name.to_owned(),
            reason: e.to_string(),
        })?
        .into_rgba8();
    let (width, height) = image.dimensions();
    let mut data = image.into_raw();
    for px in data.as_chunks_mut::<4>().0 {
        let a = px[3];
        if a != 255 {
            px[0] = premultiply(px[0], a);
            px[1] = premultiply(px[1], a);
            px[2] = premultiply(px[2], a);
        }
    }
    let size = IntSize::from_wh(width, height).ok_or_else(|| RasterError {
        name: name.to_owned(),
        reason: "пустая картинка".to_owned(),
    })?;
    Pixmap::from_vec(data, size).ok_or_else(|| RasterError {
        name: name.to_owned(),
        reason: "размер".to_owned(),
    })
}

/// Прямоугольник в пикселях: `x, y, ширина, высота`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Area {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Area {
        Area {
            x,
            y,
            width,
            height,
        }
    }

    pub fn of(pixmap: &Pixmap) -> Area {
        Area::new(0, 0, pixmap.width() as i32, pixmap.height() as i32)
    }
}

/// Как пиксель источника ложится на пиксель кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blend {
    /// `copy`.
    Copy,
    /// `source-over` спрайтом (`drawImage` целиком в целую точку).
    Sprite,
    /// `source-over` конвейером lowp (`drawImage` части картинки, текст).
    Over,
    /// `lighter`.
    Plus,
}

#[inline]
fn div255_lowp(v: u32) -> u32 {
    (v + 255) >> 8
}

#[inline]
fn mix(blend: Blend, s: PremultipliedColorU8, d: PremultipliedColorU8) -> PremultipliedColorU8 {
    let [sr, sg, sb, sa] = [s.red(), s.green(), s.blue(), s.alpha()].map(u32::from);
    let [dr, dg, db, da] = [d.red(), d.green(), d.blue(), d.alpha()].map(u32::from);
    let out = match blend {
        Blend::Copy => return s,
        Blend::Sprite => {
            let scale = 256 - sa;
            [
                sr + ((dr * scale) >> 8),
                sg + ((dg * scale) >> 8),
                sb + ((db * scale) >> 8),
                sa + ((da * scale) >> 8),
            ]
        }
        Blend::Over => {
            let inv = 255 - sa;
            [
                sr + div255_lowp(dr * inv),
                sg + div255_lowp(dg * inv),
                sb + div255_lowp(db * inv),
                sa + div255_lowp(da * inv),
            ]
        }
        Blend::Plus => [sr + dr, sg + dg, sb + db, sa + da],
    };
    let [r, g, b, a] = out.map(|v| v.min(255) as u8);
    PremultipliedColorU8::from_rgba(r.min(a), g.min(a), b.min(a), a)
        .unwrap_or(PremultipliedColorU8::TRANSPARENT)
}

/// `drawImage(src, sx, sy, w, h, dx, dy, w, h)` без масштаба: часть `from`
/// источника в точку `(dx, dy)` кадра; что выходит за края — отсекается.
pub fn blit(dst: &mut Pixmap, src: &Pixmap, from: Area, dx: i32, dy: i32, blend: Blend) {
    let (sw, sh) = (src.width() as i32, src.height() as i32);
    let (tw, th) = (dst.width() as i32, dst.height() as i32);
    // Пересечение части с источником.
    let x0 = from.x.max(0);
    let y0 = from.y.max(0);
    let x1 = (from.x + from.width).min(sw);
    let y1 = (from.y + from.height).min(sh);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let (ox, oy) = (dx - from.x, dy - from.y);
    let src_px = src.pixels();
    let dst_px = dst.pixels_mut();
    for sy in y0..y1 {
        let ty = sy + oy;
        if ty < 0 || ty >= th {
            continue;
        }
        for sx in x0..x1 {
            let tx = sx + ox;
            if tx < 0 || tx >= tw {
                continue;
            }
            let s = src_px[(sy * sw + sx) as usize];
            let at = (ty * tw + tx) as usize;
            dst_px[at] = mix(blend, s, dst_px[at]);
        }
    }
}

/// `globalCompositeOperation = 'multiply'; fillRect` непрозрачным цветом по
/// непрозрачной части: `d = div255(d · c)` (lowp).
pub fn multiply_rgb(dst: &mut Pixmap, area: Area, rgb: [u8; 3]) {
    let tw = dst.width() as i32;
    let th = dst.height() as i32;
    let [cr, cg, cb] = rgb.map(u32::from);
    let px = dst.pixels_mut();
    for y in area.y.max(0)..(area.y + area.height).min(th) {
        for x in area.x.max(0)..(area.x + area.width).min(tw) {
            let at = (y * tw + x) as usize;
            let d = px[at];
            let (dr, dg, db, da) = (
                u32::from(d.red()),
                u32::from(d.green()),
                u32::from(d.blue()),
                u32::from(d.alpha()),
            );
            // lowp multiply: s·(1 − da) + d·(1 − sa) + s·d, sa = 1.
            let ch = |s: u32, d: u32| div255_lowp(s * (255 - da) + s * d).min(255) as u8;
            let a = div255_lowp(255 * (255 - da) + da * 255).min(255) as u8;
            let (r, g, b) = (ch(cr, dr), ch(cg, dg), ch(cb, db));
            px[at] = PremultipliedColorU8::from_rgba(r.min(a), g.min(a), b.min(a), a)
                .unwrap_or(PremultipliedColorU8::TRANSPARENT);
        }
    }
}

/// `fillStyle = rgb; globalAlpha = alpha; fillRect` в целых пикселях:
/// `SkBlitRow::Color32` (непрозрачный цвет — заливка, 0 — ничего).
pub fn fill_rect(dst: &mut Pixmap, area: Area, rgb: [u8; 3], alpha: u8) {
    let tw = dst.width() as i32;
    let th = dst.height() as i32;
    if alpha == 0 {
        return;
    }
    let color = [
        premultiply(rgb[0], alpha),
        premultiply(rgb[1], alpha),
        premultiply(rgb[2], alpha),
        alpha,
    ];
    // SkAlpha255To256(255 − a).
    let scale = 256 - u32::from(alpha);
    let px = dst.pixels_mut();
    for y in area.y.max(0)..(area.y + area.height).min(th) {
        for x in area.x.max(0)..(area.x + area.width).min(tw) {
            let at = (y * tw + x) as usize;
            let d = px[at];
            let out = if alpha == 255 {
                color
            } else {
                let blend =
                    |d: u8, c: u8| (u32::from(c) + ((u32::from(d) * scale) >> 8)).min(255) as u8;
                [
                    blend(d.red(), color[0]),
                    blend(d.green(), color[1]),
                    blend(d.blue(), color[2]),
                    blend(d.alpha(), color[3]),
                ]
            };
            px[at] = PremultipliedColorU8::from_rgba(
                out[0].min(out[3]),
                out[1].min(out[3]),
                out[2].min(out[3]),
                out[3],
            )
            .unwrap_or(PremultipliedColorU8::TRANSPARENT);
        }
    }
}

/// `drawImage(src, x, y, w, h)` с `imageSmoothingQuality: 'high'`. Без
/// масштаба в целую точку — спрайт (`blend` — `Sprite` или `Plus`), иначе —
/// tiny-skia с бикубическим фильтром (уменьшение — билинейный).
#[allow(clippy::too_many_arguments)]
pub fn draw_image(
    dst: &mut Pixmap,
    src: &Pixmap,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    blend: Blend,
    opacity: f32,
) {
    let (sw, sh) = (f64::from(src.width()), f64::from(src.height()));
    let whole = |v: f64| (v - v.round()).abs() < 1e-9;
    if opacity >= 1.0 && width == sw && height == sh && whole(x) && whole(y) {
        blit(
            dst,
            src,
            Area::of(src),
            x.round() as i32,
            y.round() as i32,
            blend,
        );
        return;
    }
    let (kx, ky) = (width / sw, height / sh);
    let quality = if kx >= 1.0 && ky >= 1.0 {
        FilterQuality::Bicubic
    } else {
        FilterQuality::Bilinear
    };
    let paint = PixmapPaint {
        opacity: opacity.clamp(0.0, 1.0),
        blend_mode: match blend {
            Blend::Copy => BlendMode::Source,
            Blend::Plus => BlendMode::Plus,
            Blend::Sprite | Blend::Over => BlendMode::SourceOver,
        },
        quality,
    };
    let transform = Transform::from_row(kx as f32, 0.0, 0.0, ky as f32, x as f32, y as f32);
    dst.draw_pixmap(0, 0, src.as_ref(), &paint, transform, None);
}

/// Непрозрачная картинка заданного цвета (`fillRect` на весь холст).
pub fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(width, height)?;
    let c = PremultipliedColorU8::from_rgba(
        premultiply(rgba[0], rgba[3]),
        premultiply(rgba[1], rgba[3]),
        premultiply(rgba[2], rgba[3]),
        rgba[3],
    )?;
    pixmap.pixels_mut().fill(c);
    Some(pixmap)
}

/// Предумноженный RGBA → картинка без предумножения.
pub fn to_image(pixmap: &Pixmap) -> Rgba8Image {
    let mut out = Rgba8Image::new(pixmap.width(), pixmap.height());
    for (o, p) in out.make_mut_slice().iter_mut().zip(pixmap.pixels()) {
        let c = p.demultiply();
        *o = Rgba::new(c.red(), c.green(), c.blue(), c.alpha());
    }
    out
}

/// Предумноженный RGBA → предумноженная картинка.
pub fn to_image_premultiplied(pixmap: &Pixmap) -> Rgba8Image {
    Rgba8Image::from_bytes(pixmap.width(), pixmap.height(), pixmap.data()).premultiplied()
}
