//! Кадр превью персонализации целиком (`GhostingSettingsPreview.vue` 0.4.7): кадр HL1 или
//! Ghosting и поверх — оверлей LiveSplit (снимок окна или пример) с обрезкой, маской и
//! прозрачностью, тем же расчётом места, что у `hlsr-overlay.dll`.

use super::raster::{self, Blend};
use hlsr_delivery::livesplit_overlay::Overlay;
use hlsr_platform::livesplit_overlay_shared::{self as shared, KeyedImage, PixelOrder, Placement};
use tiny_skia::{IntSize, Pixmap};

/// Высота кадра превью.
pub const FRAME_HEIGHT: u32 = 900;

/// Ширина кадра по разрешению игры (`frameWidthOf`): кадры сняты в 16:9 при высоте 900;
/// другое соотношение меняет только ширину, пределы — 5:4 и 32:9.
pub fn frame_width_of(screen: Option<(f64, f64)>) -> u32 {
    match screen {
        Some((w, h)) if w > 0.0 && h > 0.0 => {
            hlsr_core::js::math_round(f64::from(FRAME_HEIGHT) * w / h).clamp(1125.0, 3200.0) as u32
        }
        _ => 1600,
    }
}

/// Пример LiveSplit или снимок окна с маской оверлея (`keyImage`).
pub fn keyed_sample(overlay: &Overlay, variant: &str) -> Option<KeyedImage> {
    let sample = super::livesplit_sample::draw_sample(variant)?;
    let rgba = sample.clone().take_demultiplied();
    Some(shared::key_image(
        overlay,
        &rgba,
        sample.width() as usize,
        sample.height() as usize,
        PixelOrder::Rgba,
    ))
}

/// Картинка после маски (RGBA без предумножения) → предумноженный холст.
pub fn keyed_pixmap(keyed: &KeyedImage) -> Option<Pixmap> {
    let mut data = keyed.data.clone();
    for px in data.as_chunks_mut::<4>().0 {
        let a = px[3];
        for c in &mut px[..3] {
            *c = raster::premultiply(*c, a);
        }
    }
    Pixmap::from_vec(
        data,
        IntSize::from_wh(keyed.width as u32, keyed.height as u32)?,
    )
}

/// Где оверлей встанет в кадре шириной `frame_width` (`placement`).
pub fn overlay_rect(overlay: &Overlay, frame_width: u32, keyed: &KeyedImage) -> Option<Placement> {
    shared::placement(
        overlay,
        f64::from(frame_width),
        f64::from(FRAME_HEIGHT),
        keyed.width as f64,
        keyed.height as f64,
    )
}

/// `drawLiveSplit`: оверлей поверх кадра, прозрачность — `opacity`, выключенный
/// (виден только в своём разделе) — ещё × 0.45.
pub fn draw_overlay(frame: &mut Pixmap, overlay: &Overlay, image: &Pixmap, rect: Placement) {
    let alpha = overlay.opacity as f32 / 100.0 * if overlay.enabled { 1.0 } else { 0.45 };
    raster::draw_image(
        frame,
        image,
        rect.left as f64,
        rect.top as f64,
        rect.width as f64,
        rect.height as f64,
        Blend::Over,
        alpha,
    );
}

/// Кадр превью и его уменьшенные копии (по половине, для вида меньше 50 %).
pub struct ViewSource {
    levels: std::cell::RefCell<Vec<std::rc::Rc<Pixmap>>>,
}

impl ViewSource {
    pub fn new(frame: Pixmap) -> ViewSource {
        ViewSource {
            levels: std::cell::RefCell::new(vec![std::rc::Rc::new(frame)]),
        }
    }

    pub fn frame(&self) -> std::rc::Rc<Pixmap> {
        self.levels.borrow()[0].clone()
    }

    /// Копия, уменьшенная в `2^level` раз (среднее 2 × 2).
    fn level(&self, level: usize) -> std::rc::Rc<Pixmap> {
        let mut levels = self.levels.borrow_mut();
        while levels.len() <= level {
            let Some(next) = levels.last().and_then(|p| halve(p)) else {
                break;
            };
            levels.push(std::rc::Rc::new(next));
        }
        levels[level.min(levels.len() - 1)].clone()
    }
}

/// Копия вдвое меньше (среднее 2 × 2); 1 × 1 — `None`.
pub fn halve(src: &Pixmap) -> Option<Pixmap> {
    let (w, h) = ((src.width() / 2).max(1), (src.height() / 2).max(1));
    if w == src.width() && h == src.height() {
        return None;
    }
    let mut out = Pixmap::new(w, h)?;
    let sw = src.width() as usize;
    let (mx, my) = (src.width() as usize - 1, src.height() as usize - 1);
    let px = src.pixels();
    for y in 0..h as usize {
        for x in 0..w as usize {
            let mut acc = [0u32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = px[(2 * y + dy).min(my) * sw + (2 * x + dx).min(mx)];
                acc[0] += u32::from(p.red());
                acc[1] += u32::from(p.green());
                acc[2] += u32::from(p.blue());
                acc[3] += u32::from(p.alpha());
            }
            let [r, g, b, a] = acc.map(|v| ((v + 2) / 4) as u8);
            if let Some(c) =
                tiny_skia::PremultipliedColorU8::from_rgba(r.min(a), g.min(a), b.min(a), a)
            {
                out.pixels_mut()[y * w as usize + x] = c;
            }
        }
    }
    Some(out)
}

/// Ручка оверлея в пикселях кадра.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HandleRect {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

/// Картинка вида (`paint()` `GhostingPreviewViewport.vue`): холст размера вида в
/// физических px, кадр в масштабе `scale` (px вида на px кадра) с точкой `(cx, cy)`
/// кадра в центре, сглаживание `high` (увеличение — бикубический фильтр, уменьшение —
/// уменьшенная копия и билинейный); поверх — рамка ручки пунктиром в физических px.
#[allow(clippy::too_many_arguments)]
pub fn paint_view(
    source: &ViewSource,
    view_scale: f64,
    cx: f64,
    cy: f64,
    box_width: f64,
    box_height: f64,
    ratio: f64,
    handle: Option<HandleRect>,
    hot: bool,
) -> Option<Pixmap> {
    let width = js_round(box_width * ratio).max(1.0) as u32;
    let height = js_round(box_height * ratio).max(1.0) as u32;
    let mut canvas = Pixmap::new(width, height)?;
    let frame = source.frame();
    let (fw, fh) = (f64::from(frame.width()), f64::from(frame.height()));
    let scale = view_scale * ratio;
    let left = f64::from(width) / 2.0 - cx * scale;
    let top = f64::from(height) / 2.0 - cy * scale;
    if scale > 0.0 && scale.is_finite() {
        // Уменьшение больше чем вдвое — с уменьшенной копии.
        let level = if scale < 1.0 {
            ((1.0 / scale).log2().floor().max(0.0)) as usize
        } else {
            0
        };
        let image = source.level(level);
        let (kx, ky) = (
            fw * scale / f64::from(image.width()),
            fh * scale / f64::from(image.height()),
        );
        let paint = tiny_skia::PixmapPaint {
            quality: if scale >= 1.0 {
                tiny_skia::FilterQuality::Bicubic
            } else {
                tiny_skia::FilterQuality::Bilinear
            },
            ..tiny_skia::PixmapPaint::default()
        };
        let transform =
            tiny_skia::Transform::from_row(kx as f32, 0.0, 0.0, ky as f32, left as f32, top as f32);
        canvas.draw_pixmap(0, 0, image.as_ref().as_ref(), &paint, transform, None);
    }
    if let Some(h) = handle {
        let x = js_round(left + h.left * scale) + 0.5;
        let y = js_round(top + h.top * scale) + 0.5;
        let w = js_round(h.width * scale) - 1.0;
        let hh = js_round(h.height * scale) - 1.0;
        if let Some(rect) = tiny_skia::Rect::from_xywh(x as f32, y as f32, w as f32, hh as f32) {
            let path = tiny_skia::PathBuilder::from_rect(rect);
            let r = ratio as f32;
            let dashed = |offset: f32, color: tiny_skia::Color, canvas: &mut Pixmap| {
                let stroke = tiny_skia::Stroke {
                    width: r,
                    dash: tiny_skia::StrokeDash::new(vec![5.0 * r, 4.0 * r], offset),
                    ..tiny_skia::Stroke::default()
                };
                let mut paint = tiny_skia::Paint::default();
                paint.set_color(color);
                paint.anti_alias = true;
                canvas.stroke_path(
                    &path,
                    &paint,
                    &stroke,
                    tiny_skia::Transform::identity(),
                    None,
                );
            };
            dashed(0.0, tiny_skia::Color::from_rgba8(0, 0, 0, 179), &mut canvas);
            let light = if hot {
                tiny_skia::Color::WHITE
            } else {
                tiny_skia::Color::from_rgba8(255, 255, 255, 191)
            };
            dashed(5.0 * r, light, &mut canvas);
        }
    }
    Some(canvas)
}

fn js_round(v: f64) -> f64 {
    hlsr_core::js::math_round(v)
}
