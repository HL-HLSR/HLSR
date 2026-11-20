//! Эмодзи реакций ленты (`REACTIONS` 0.4.7: 👍 ❤️ 🔥 😮 🎉; решение 17): пять
//! картинок — растры глифов Noto Color Emoji 2.038 из таблицы CBDT
//! (`tools/reference/lib/emoji-images.py`), которыми Chromium 0.4.7 рисует их на
//! Linux; на Windows 0.4.7 брал Segoe UI Emoji, его вшить нельзя.
//!
//! Картинка строится сразу в физических px окна, как глиф у Skia
//! (`SkFontHost_FreeType`): strike 109 ppem масштабируется к кеглю, рамка
//! глифа — округление наружу, уровень mip-карты — ближайший
//! (`SkMipmap::ComputeLevel`, смещение −0,5; уровни — фильтры 2×2 и 1-2-1),
//! выборка — билинейная, цвета — с умноженной альфой, как отдаёт FreeType.
//! Так одинаково при любом рендерере: уменьшение текстуры без mip-карт
//! (136 px → 15 px) дало бы «лесенку».

use crate::{Rgba, Rgba8Image};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Картинка эмодзи и рамка глифа в логич. px.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmojiImage {
    /// Предумноженная картинка.
    pub image: Arc<Rgba8Image>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub advance: f32,
}

/// Картинки по виду реакции в порядке `REACTIONS`; кодовая точка — глиф
/// смайлика 0.4.7 (у ❤️ селектор варианта U+FE0F своего глифа не имеет).
const IMAGES: [(&str, char, &[u8]); 5] = [
    (
        "like",
        '\u{1F44D}',
        include_bytes!("../assets/emoji/like.png"),
    ),
    (
        "love",
        '\u{2764}',
        include_bytes!("../assets/emoji/love.png"),
    ),
    (
        "fire",
        '\u{1F525}',
        include_bytes!("../assets/emoji/fire.png"),
    ),
    (
        "wow",
        '\u{1F62E}',
        include_bytes!("../assets/emoji/wow.png"),
    ),
    (
        "celebrate",
        '\u{1F389}',
        include_bytes!("../assets/emoji/celebrate.png"),
    ),
];

/// Strike Noto Color Emoji 2.038 (CBLC/CBDT; их проверяет emoji-images.py):
/// 109 ppem, растр 136×128, bearingX 0, bearingY 101, advance 136.
const PPEM: f32 = 109.0;
const BITMAP_WIDTH: usize = 136;
const BITMAP_HEIGHT: usize = 128;
const BEARING_Y: f32 = 101.0;
const ADVANCE: f32 = 136.0;

/// Кэш картинок: столько записей, вытесняется самая старая.
const CACHE_LIMIT: usize = 64;

/// Виды реакций, у которых есть картинка, — в порядке 0.4.7.
pub fn keys() -> impl Iterator<Item = &'static str> {
    IMAGES.iter().map(|(key, ..)| *key)
}

/// Где лежит картинка глифа: физ. px от начала глифа на базовой линии.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    /// Ширина глифа в строке, физ. px.
    pub advance: f32,
    /// Физ. px на пиксель strike.
    pub zoom: f32,
}

/// Субпиксельное позиционирование Chromium (Linux при масштабе > 1, как
/// `richtext::rounded_metrics`): ширина глифа дробная; иначе Blink округляет
/// ширину от Skia до целого px (`SkFontGetGlyphWidthForHarfBuzz`).
fn subpixel(scale: f32) -> bool {
    cfg!(target_os = "linux") && scale > 1.0
}

/// Рамка глифа кегля `size` (логич. px) при масштабе окна `scale`.
pub fn placement(size: f32, scale: f32) -> Placement {
    let zoom = (size * scale / PPEM).max(0.0);
    let zoom = if zoom.is_finite() { zoom } else { 0.0 };
    let top_edge = -BEARING_Y * zoom;
    let bottom_edge = top_edge + BITMAP_HEIGHT as f32 * zoom;
    let right_edge = BITMAP_WIDTH as f32 * zoom;
    let top = top_edge.floor() as i32;
    let advance = ADVANCE * zoom;
    Placement {
        left: 0,
        top,
        width: right_edge.ceil() as u32,
        height: (bottom_edge.ceil() as i32 - top).max(0) as u32,
        advance: if subpixel(scale) {
            advance
        } else {
            advance.round()
        },
        zoom,
    }
}

/// Растр с умноженной альфой.
#[derive(Clone)]
struct Raster {
    width: usize,
    height: usize,
    px: Vec<[u8; 4]>,
}

impl Raster {
    fn at(&self, x: usize, y: usize) -> [u8; 4] {
        self.px[y * self.width + x]
    }
}

/// Умножение на альфу как в FreeType (`sfnt/pngshim.c`, `multiply_alpha`).
fn premultiply(alpha: u8, color: u8) -> u8 {
    let temp = u32::from(alpha) * u32::from(color) + 0x80;
    ((temp + (temp >> 8)) >> 8) as u8
}

fn decode(png: &'static [u8]) -> Option<Raster> {
    let buffer = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    let px = buffer
        .pixels()
        .map(|p| {
            let [r, g, b, a] = p.0;
            [premultiply(a, r), premultiply(a, g), premultiply(a, b), a]
        })
        .collect();
    Some(Raster {
        width: buffer.width() as usize,
        height: buffer.height() as usize,
        px,
    })
}

/// Следующий уровень mip-карты Skia (`SkMipmap::Build`): размер вдвое меньше
/// (не меньше 1); по чётной стороне — среднее пары, по нечётной — 1-2-1;
/// деление с отбрасыванием остатка.
fn downsample(src: &Raster) -> Raster {
    let width = (src.width / 2).max(1);
    let height = (src.height / 2).max(1);
    let taps = |len: usize| -> &'static [u32] {
        if len == 1 {
            &[1]
        } else if len % 2 == 1 {
            &[1, 2, 1]
        } else {
            &[1, 1]
        }
    };
    let (tx, ty) = (taps(src.width), taps(src.height));
    let shift = (tx.iter().sum::<u32>() * ty.iter().sum::<u32>()).trailing_zeros();
    let mut px = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let mut sum = [0u32; 4];
            for (j, wy) in ty.iter().enumerate() {
                for (i, wx) in tx.iter().enumerate() {
                    let sx = (2 * x + i).min(src.width - 1);
                    let sy = (2 * y + j).min(src.height - 1);
                    for (total, channel) in sum.iter_mut().zip(src.at(sx, sy)) {
                        *total += u32::from(channel) * wx * wy;
                    }
                }
            }
            px.push(sum.map(|v| (v >> shift) as u8));
        }
    }
    Raster { width, height, px }
}

/// Номер уровня mip-карты для уменьшения в `zoom` раз (`SkMipmapAccessor`,
/// режим kNearest): log2(1 / zoom) − 0,5, округлённый.
fn mip_level(zoom: f32) -> u32 {
    if !(zoom > 0.0 && zoom < 1.0) {
        return 0;
    }
    let level = (-zoom.log2() - 0.5).max(0.0);
    level.round() as u32
}

/// Картинка вида `key` кеглем `size` (логич. px) при масштабе `scale`; `None`
/// — такого вида нет.
pub fn render(key: &str, size: f32, scale: f32) -> Option<(Rgba8Image, Placement)> {
    let (.., png) = IMAGES.iter().find(|(name, ..)| *name == key)?;
    let place = placement(size, scale);
    let mut out = Rgba8Image::new(place.width, place.height).premultiplied();
    if place.width == 0 || place.height == 0 {
        return Some((out, place));
    }
    let mut level = decode(png)?;
    for _ in 0..mip_level(place.zoom) {
        if level.width == 1 && level.height == 1 {
            break;
        }
        level = downsample(&level);
    }
    // Физ. px → px уровня; растр strike занимает [0, 136·zoom) × [top_edge,
    // top_edge + 128·zoom), рисуется без сглаживания краёв (центр пикселя внутри).
    let kx = level.width as f32 / (BITMAP_WIDTH as f32 * place.zoom);
    let ky = level.height as f32 / (BITMAP_HEIGHT as f32 * place.zoom);
    let top_edge = -BEARING_Y * place.zoom;
    let (rect_w, rect_h) = (
        BITMAP_WIDTH as f32 * place.zoom,
        BITMAP_HEIGHT as f32 * place.zoom,
    );
    let width = place.width as usize;
    let pixels = out.make_mut_slice();
    for j in 0..place.height as usize {
        for i in 0..width {
            let bx = (place.left + i as i32) as f32 + 0.5;
            let by = (place.top + j as i32) as f32 + 0.5 - top_edge;
            if bx < 0.0 || by < 0.0 || bx >= rect_w || by >= rect_h {
                continue;
            }
            let p = bilinear(&level, bx * kx - 0.5, by * ky - 0.5);
            pixels[j * width + i] = Rgba::new(p[0], p[1], p[2], p[3]);
        }
    }
    Some((out, place))
}

/// Билинейная выборка с прижатием к краю (точка — в координатах центров px).
fn bilinear(src: &Raster, u: f32, v: f32) -> [u8; 4] {
    let x0 = u.floor();
    let y0 = v.floor();
    let (fx, fy) = (u - x0, v - y0);
    let clamp = |value: f32, len: usize| (value.max(0.0) as usize).min(len - 1);
    let (xa, xb) = (clamp(x0, src.width), clamp(x0 + 1.0, src.width));
    let (ya, yb) = (clamp(y0, src.height), clamp(y0 + 1.0, src.height));
    let (p00, p10, p01, p11) = (
        src.at(xa, ya),
        src.at(xb, ya),
        src.at(xa, yb),
        src.at(xb, yb),
    );
    std::array::from_fn(|c| {
        let top = f32::from(p00[c]) * (1.0 - fx) + f32::from(p10[c]) * fx;
        let bottom = f32::from(p01[c]) * (1.0 - fx) + f32::from(p11[c]) * fx;
        (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8
    })
}

/// Ключ кэша: вид и величины в 1/64 px.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
struct Key {
    key: String,
    size: i64,
    scale: i64,
}

fn q(v: f32) -> i64 {
    if v.is_finite() {
        (v * 64.0).round() as i64
    } else {
        0
    }
}

struct Cache {
    images: HashMap<Key, EmojiImage>,
    order: VecDeque<Key>,
}

impl Cache {
    fn get(&mut self, key: &str, size: f32, scale: f32) -> EmojiImage {
        let id = Key {
            key: key.to_owned(),
            size: q(size),
            scale: q(scale),
        };
        if let Some(found) = self.images.get(&id) {
            return found.clone();
        }
        let result = make_glyph(key, size, scale);
        if self.order.len() >= CACHE_LIMIT
            && let Some(old) = self.order.pop_front()
        {
            self.images.remove(&old);
        }
        self.order.push_back(id.clone());
        self.images.insert(id, result.clone());
        result
    }
}

/// Картинка и рамка в логич. px.
fn make_glyph(key: &str, size: f32, scale: f32) -> EmojiImage {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let Some((buffer, place)) = render(key, size, scale) else {
        return EmojiImage::default();
    };
    EmojiImage {
        image: Arc::new(buffer),
        x: place.left as f32 / scale,
        y: place.top as f32 / scale,
        width: place.width as f32 / scale,
        height: place.height as f32 / scale,
        advance: place.advance / scale,
    }
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache {
        images: HashMap::new(),
        order: VecDeque::new(),
    });
}

/// Эмодзи вида `key` кеглем `size` (логич. px) при масштабе `scale`, с кэшем; нет такого вида —
/// пустая картинка.
pub fn glyph(key: &str, size: f32, scale: f32) -> EmojiImage {
    CACHE.with(|cache| cache.borrow_mut().get(key, size, scale))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hlsr_online::shared::news_contract::REACTIONS;

    #[test]
    fn images_follow_reactions_of_047() {
        assert_eq!(IMAGES.len(), REACTIONS.len());
        for ((key, codepoint, _), (name, emoji)) in IMAGES.iter().zip(REACTIONS) {
            assert_eq!(*key, name);
            assert_eq!(emoji.chars().next(), Some(*codepoint), "{name}");
        }
    }

    #[test]
    fn every_image_is_the_noto_strike() {
        for (key, _, png) in IMAGES {
            let raster = decode(png).unwrap_or_else(|| panic!("{key}: PNG не читается"));
            assert_eq!(
                (raster.width, raster.height),
                (BITMAP_WIDTH, BITMAP_HEIGHT),
                "{key}"
            );
            assert!(
                raster.px.iter().any(|p| p[3] == 255),
                "{key}: нет непрозрачных px"
            );
            assert!(
                raster
                    .px
                    .iter()
                    .all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]),
                "{key}: цвет больше альфы"
            );
        }
    }

    #[test]
    fn placement_scales_the_strike_to_the_font_size() {
        // Реакция 0.4.7: 12px. zoom = 12/109; растр 14,97 × 14,09 от
        // −11,12 px над базовой линией.
        let p = placement(12.0, 1.0);
        assert_eq!((p.left, p.top, p.width, p.height), (0, -12, 15, 15));
        assert_eq!(p.advance, 15.0);
        let p = placement(12.0, 1.25);
        // 15 физ. px: растр 18,72 × 17,61, верх −13,90.
        assert_eq!((p.left, p.top, p.width, p.height), (0, -14, 19, 18));
        if cfg!(target_os = "linux") {
            assert!((p.advance - 136.0 * 15.0 / 109.0).abs() < 1e-4);
        } else {
            assert_eq!(p.advance, 19.0);
        }
        assert_eq!(placement(0.0, 1.0).width, 0);
        assert_eq!(placement(f32::NAN, 1.0).width, 0);
    }

    #[test]
    fn mip_level_is_nearest_with_sharpen_bias() {
        assert_eq!(mip_level(1.0), 0);
        assert_eq!(mip_level(2.0), 0);
        // 12px: log2(109/12) − 0,5 = 2,68 → 3 (1/8: 17×16).
        assert_eq!(mip_level(12.0 / 109.0), 3);
        // 15 физ. px: 2,36 → 2.
        assert_eq!(mip_level(15.0 / 109.0), 2);
        assert_eq!(mip_level(0.5), 1);
    }

    #[test]
    fn downsample_uses_box_and_121_filters() {
        let raster = Raster {
            width: 3,
            height: 2,
            px: vec![
                [0, 0, 0, 0],
                [100, 100, 100, 200],
                [40, 40, 40, 40],
                [0, 0, 0, 0],
                [100, 100, 100, 200],
                [40, 40, 40, 40],
            ],
        };
        // Ширина 3 — 1-2-1, высота 2 — пара, сумма весов 8: цвет
        // (0 + 2·100 + 40)·2 / 8 = 60, альфа (0 + 2·200 + 40)·2 / 8 = 110.
        let half = downsample(&raster);
        assert_eq!((half.width, half.height), (1, 1));
        assert_eq!(half.px[0], [60, 60, 60, 110]);
    }

    #[test]
    fn render_fills_the_frame_inside_the_bitmap() {
        for key in keys() {
            let (buffer, place) = render(key, 12.0, 1.0).unwrap();
            assert_eq!(
                (buffer.width(), buffer.height()),
                (place.width, place.height)
            );
            let alpha: u32 = buffer.as_slice().iter().map(|p| u32::from(p.a)).sum();
            assert!(alpha > 255 * 40, "{key}: почти пусто ({alpha})");
            assert!(
                buffer
                    .as_slice()
                    .iter()
                    .all(|p| p.r <= p.a && p.g <= p.a && p.b <= p.a),
                "{key}: цвет больше альфы"
            );
        }
        assert!(render("thumbs", 12.0, 1.0).is_none());
        let (empty, _) = render("like", 0.0, 1.0).unwrap();
        assert_eq!(empty.width(), 0);
    }

    #[test]
    fn cache_returns_the_same_image_and_evicts() {
        let mut cache = Cache {
            images: HashMap::new(),
            order: VecDeque::new(),
        };
        let first = cache.get("fire", 12.0, 1.0);
        let again = cache.get("fire", 12.0, 1.0);
        assert_eq!(first.image, again.image);
        assert_eq!(
            (first.width, first.height, first.advance),
            (15.0, 15.0, 15.0)
        );
        assert_eq!((first.x, first.y), (0.0, -12.0));
        for n in 0..CACHE_LIMIT {
            cache.get("wow", 8.0 + n as f32, 1.0);
        }
        assert_eq!(cache.images.len(), CACHE_LIMIT);
        assert!(!cache.images.keys().any(|key| key.key == "fire"));
        assert_eq!(cache.get("nope", 12.0, 1.0).width, 0.0);
    }
}
