//! Текст Canvas 2D (`fillText`, `strokeText`, `measureText`) для растров превью:
//! пример LiveSplit (`"Segoe UI", "Noto Sans", sans-serif`), подписи HUD Ghosting
//! (`Arial`, `Verdana`), шрифт HUD игрока по имени (системный) или из файла.
//!
//! Раскладка — parley (шрифты ОС через fontique и вшитые шрифты лаунчера), глифы —
//! swash в маску покрытия. Базовая линия `middle` — как у Blink: середина квадрата em
//! (typo-метрики, приведённые к кеглю, [`Metrics`]). Сглаживание глифов у Skia (DirectWrite/FreeType) другое,
//! поэтому текст превью сверяется с допуском (решение 19).

use parley::fontique::{Blob, Collection, CollectionOptions, SourceCache};
use parley::{FontContext, LayoutContext, PositionedLayoutItem, StyleProperty};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::{Format, Join, Stroke, Transform, Vector};
use tiny_skia::{Pixmap, PremultipliedColorU8};

/// Шрифты лаунчера (те же файлы, что импортирует `app.slint`).
const EMBEDDED: [&[u8]; 5] = [
    include_bytes!("../../fonts/Rubik-Light.ttf"),
    include_bytes!("../../fonts/Rubik-Regular.ttf"),
    include_bytes!("../../fonts/Rubik-Medium.ttf"),
    include_bytes!("../../fonts/Rubik-Bold.ttf"),
    include_bytes!("../../fonts/CascadiaMono-Regular.ttf"),
];

/// Шрифт `ctx.font`: насыщенность, кегль (px) и список семейств.
#[derive(Debug, Clone, PartialEq)]
pub struct Font {
    pub weight: u16,
    pub size: f32,
    pub families: Vec<String>,
}

impl Font {
    /// Разобрать `[style] [weight] <size>px <семейства>` (подмножество CSS, которое
    /// используют рендеры превью 0.4.7). Непонятное — Chromium оставил бы прежний шрифт;
    /// здесь — 10px sans-serif, как начальный шрифт холста.
    pub fn parse(css: &str) -> Font {
        let mut weight = 400u16;
        let mut rest = css.trim();
        let mut size = None;
        while let Some((token, tail)) = split_token(rest) {
            if let Some(px) = token.strip_suffix("px")
                && let Ok(v) = px.parse::<f32>()
            {
                size = Some(v);
                rest = tail;
                break;
            }
            match token {
                "bold" | "bolder" => weight = 700,
                "normal" | "italic" | "oblique" => {}
                "lighter" => weight = 300,
                other => {
                    if let Ok(w) = other.parse::<f32>() {
                        weight = w.clamp(1.0, 1000.0) as u16;
                    }
                }
            }
            rest = tail;
        }
        let Some(size) = size else {
            return Font {
                weight: 400,
                size: 10.0,
                families: vec!["sans-serif".into()],
            };
        };
        let families = rest
            .split(',')
            .map(|f| {
                f.trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .trim()
                    .to_owned()
            })
            .filter(|f| !f.is_empty())
            .collect();
        Font {
            weight,
            size,
            families,
        }
    }
}

fn split_token(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    if text.is_empty() {
        return None;
    }
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    Some((&text[..end], &text[end..]))
}

/// `textAlign`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// `textBaseline`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Baseline {
    Alphabetic,
    Middle,
}

struct Glyph {
    data: Blob<u8>,
    index: u32,
    id: u16,
    size: f32,
    x: f32,
    y: f32,
}

/// Строка, разложенная от начала на базовой линии.
pub struct Line {
    glyphs: Vec<Glyph>,
    /// Полные имена шрифтов строки по порядку глифов (без повторов подряд).
    pub faces: Vec<String>,
    /// Ширина (`measureText().width`).
    pub width: f32,
    /// Метрики первого шрифта.
    pub metrics: Metrics,
}

struct Engine {
    fonts: FontContext,
    layout: LayoutContext<()>,
    scaler: ScaleContext,
    /// Файлы шрифтов, добавленные по ходу (`FontFace` из файла игрока): имя → семейство.
    added: Vec<(String, String)>,
}

impl Engine {
    fn new() -> Engine {
        let mut fonts = FontContext {
            collection: Collection::new(CollectionOptions {
                shared: false,
                system_fonts: true,
            }),
            source_cache: SourceCache::default(),
        };
        for data in EMBEDDED {
            fonts
                .collection
                .register_fonts(Blob::new(Arc::new(data)), None);
        }
        Engine {
            fonts,
            layout: LayoutContext::new(),
            scaler: ScaleContext::new(),
            added: Vec::new(),
        }
    }
}

thread_local! {
    static ENGINE: RefCell<Engine> = RefCell::new(Engine::new());
}

/// Зарегистрировать файл шрифта (TTF/OTF игрока, `FontFace` 0.4.7) под ключом `key`;
/// возвращает имя его семейства (или `None`, если файл не шрифт). Байты не копируются: кадр
/// зовёт это для каждого шрифта игрока, уже известный ключ — сразу имя.
pub fn register_font(key: &str, bytes: Arc<Vec<u8>>) -> Option<String> {
    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        if let Some((_, family)) = engine.added.iter().find(|(k, _)| k == key) {
            return Some(family.clone());
        }
        // Файлов игрока за сеанс немного; при переполнении старые забываются.
        if engine.added.len() > 32 {
            engine.added.clear();
        }
        let blob = Blob::new(bytes);
        let registered = engine.fonts.collection.register_fonts(blob, None);
        let (id, _) = registered.first()?;
        let name = engine.fonts.collection.family_name(*id)?.to_owned();
        engine.added.push((key.to_owned(), name.clone()));
        // Новый шрифт может сменить начертание тех же имён семейств.
        TEXT_CACHE.with(|cache| cache.borrow_mut().clear());
        Some(name)
    })
}

/// Семейство шрифта из файла (как его назовёт fontique: типографское имя, иначе имя
/// семейства); не шрифт — `None`.
pub fn family_of(bytes: &[u8]) -> Option<String> {
    use skrifa::MetadataProvider;
    let font = skrifa::FontRef::from_index(bytes, 0).ok()?;
    let name = |id| {
        font.localized_strings(id)
            .english_or_first()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    };
    name(skrifa::string::StringId::TYPOGRAPHIC_FAMILY_NAME)
        .or_else(|| name(skrifa::string::StringId::FAMILY_NAME))
}

/// Семейство «sans-serif» Chromium: на Windows — Arial.
fn generic(name: &str) -> parley::FontFamilyName<'static> {
    use parley::{FontFamilyName, GenericFamily};
    match name.to_ascii_lowercase().as_str() {
        "sans-serif" if cfg!(windows) => FontFamilyName::Named(Cow::Borrowed("Arial")),
        "sans-serif" => FontFamilyName::Generic(GenericFamily::SansSerif),
        "serif" => FontFamilyName::Generic(GenericFamily::Serif),
        "monospace" => FontFamilyName::Named(Cow::Borrowed("Cascadia Mono")),
        _ => FontFamilyName::Named(Cow::Owned(name.to_owned())),
    }
}

/// Метрики первого шрифта строки, px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// Подъём и спуск, округлённые, как `FontMetrics` Blink (на Windows —
    /// `usWinAscent`/`usWinDescent`, как у DirectWrite, иначе — `hhea` или typo при
    /// `USE_TYPO_METRICS`).
    pub ascent: f32,
    pub descent: f32,
    /// Квадрат em (`NormalizedTypoAscentAndDescent` Blink): typo-подъём и спуск,
    /// приведённые к сумме, равной кеглю, с точностью `LayoutUnit` (1/64 px). По нему
    /// холст ставит `textBaseline = 'middle'`.
    pub em_ascent: f32,
    pub em_descent: f32,
}

fn layout_unit(v: f32) -> f32 {
    (v * 64.0).round() / 64.0
}

fn metrics(data: &[u8], index: u32, size: f32) -> Metrics {
    use skrifa::raw::TableProvider;
    let fallback = Metrics {
        ascent: (size * 0.8).round(),
        descent: (size * 0.2).round(),
        em_ascent: size * 0.8,
        em_descent: size * 0.2,
    };
    let Ok(font) = skrifa::FontRef::from_index(data, index) else {
        return fallback;
    };
    let upem = font
        .head()
        .map(|h| f32::from(h.units_per_em()))
        .unwrap_or(1000.0);
    let os2 = font.os2().ok();
    let hhea = font.hhea().ok();
    // USE_TYPO_METRICS — typo и у DirectWrite, и у FreeType; иначе на Windows —
    // usWinAscent/usWinDescent (DirectWrite), на Linux — hhea (FreeType).
    let use_typo = os2
        .as_ref()
        .is_some_and(|o| o.fs_selection().bits() & (1 << 7) != 0);
    let (ascent, descent) = match (&os2, &hhea) {
        (Some(o), _) if use_typo => (
            f32::from(o.s_typo_ascender()),
            -f32::from(o.s_typo_descender()),
        ),
        (Some(os2), _) if cfg!(windows) => (
            f32::from(os2.us_win_ascent()),
            f32::from(os2.us_win_descent()),
        ),
        (_, Some(hhea)) => (
            f32::from(hhea.ascender().to_i16()),
            -f32::from(hhea.descender().to_i16()),
        ),
        _ => return fallback,
    };
    let ascent = (ascent * size / upem).round();
    let descent = (descent * size / upem).round();
    // Квадрат em: typo-метрики, если они есть, иначе подъём и спуск.
    let (ea, ed) = match &os2 {
        Some(o) if o.s_typo_ascender() > 0 || o.s_typo_descender() != 0 => (
            f32::from(o.s_typo_ascender()) * size / upem,
            -f32::from(o.s_typo_descender()) * size / upem,
        ),
        _ => (ascent, descent),
    };
    let (ea, ed) = if ea > 0.0 && ed >= 0.0 {
        (ea, ed)
    } else {
        (ascent, descent)
    };
    let em_ascent = layout_unit(ea * size / (ea + ed).max(f32::EPSILON));
    Metrics {
        ascent,
        descent,
        em_ascent,
        em_descent: layout_unit(size) - em_ascent,
    }
}

/// Полное имя шрифта (`name` ID 4).
fn face_name(data: &[u8], index: u32) -> String {
    use skrifa::MetadataProvider;
    skrifa::FontRef::from_index(data, index)
        .ok()
        .and_then(|f| {
            f.localized_strings(skrifa::string::StringId::FULL_NAME)
                .english_or_first()
                .map(|s| s.to_string())
        })
        .unwrap_or_default()
}

/// Разложить строку шрифтом `font` (одна строка, без переноса).
pub fn layout(text: &str, font: &Font) -> Line {
    layout_spaced(text, font, 0.0)
}

/// [`layout`] с `letter-spacing` (px).
pub fn layout_spaced(text: &str, font: &Font, letter_spacing: f32) -> Line {
    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        let Engine { fonts, layout, .. } = &mut *engine;
        let families: Vec<parley::FontFamilyName<'static>> =
            font.families.iter().map(|f| generic(f)).collect();
        let mut builder = layout.ranged_builder(fonts, text, 1.0, false);
        builder.push_default(parley::FontFamily::List(Cow::Owned(families)));
        builder.push_default(StyleProperty::FontWeight(parley::FontWeight::new(
            f32::from(font.weight),
        )));
        builder.push_default(StyleProperty::FontSize(font.size));
        builder.push_default(StyleProperty::LetterSpacing(letter_spacing));
        builder.push_default(StyleProperty::TextWrapMode(parley::TextWrapMode::NoWrap));
        let mut built: parley::Layout<()> = builder.build(text);
        built.break_all_lines(None);
        let mut glyphs = Vec::new();
        let mut first: Option<(Blob<u8>, u32)> = None;
        if let Some(line) = built.lines().next() {
            let baseline = line.metrics().baseline;
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(run) = item else {
                    continue;
                };
                let font_data = run.run().font();
                if first.is_none() {
                    first = Some((font_data.data.clone(), font_data.index));
                }
                for g in run.positioned_glyphs() {
                    glyphs.push(Glyph {
                        data: font_data.data.clone(),
                        index: font_data.index,
                        id: g.id as u16,
                        size: run.run().font_size(),
                        x: g.x,
                        y: g.y - baseline,
                    });
                }
            }
        }
        let metrics = match &first {
            Some((data, index)) => metrics(data.data(), *index, font.size),
            None => metrics(&[], 0, font.size),
        };
        let mut faces: Vec<String> = Vec::new();
        for g in &glyphs {
            let name = face_name(g.data.data(), g.index);
            if faces.last() != Some(&name) {
                faces.push(name);
            }
        }
        Line {
            glyphs,
            faces,
            width: built.width(),
            metrics,
        }
    })
}

/// Маска покрытия текста в пикселях холста.
pub struct Coverage {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Заливка или обводка (`lineWidth`, `lineJoin = 'round'`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Paint {
    Fill,
    Stroke(f32),
}

/// Где начало строки: `x`, `y` — точка `fillText`; `max_width` — сжатие по ширине.
#[derive(Debug, Clone, Copy)]
pub struct Place {
    pub x: f32,
    pub y: f32,
    pub align: Align,
    pub baseline: Baseline,
    pub max_width: Option<f32>,
}

/// Покрытие строки на холсте (масштаб холста — 1 px на px).
pub fn coverage(line: &Line, place: Place, paint: Paint) -> Option<Coverage> {
    let squeeze = match place.max_width {
        Some(max) if line.width > max && max > 0.0 => max / line.width,
        _ => 1.0,
    };
    let width = line.width * squeeze;
    let left = match place.align {
        Align::Left => place.x,
        Align::Center => place.x - width / 2.0,
        Align::Right => place.x - width,
    };
    let base = match place.baseline {
        Baseline::Alphabetic => place.y,
        Baseline::Middle => place.y + (line.metrics.em_ascent - line.metrics.em_descent) / 2.0,
    };
    struct Piece {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        data: Vec<u8>,
    }
    let mut pieces = Vec::new();
    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        for g in &line.glyphs {
            let Some(font) = swash::FontRef::from_index(g.data.data(), g.index as usize) else {
                continue;
            };
            let mut scaler = engine.scaler.builder(font).size(g.size).hint(false).build();
            let gx = left + g.x * squeeze;
            let gy = base + g.y;
            let (ix, iy) = (gx.floor(), gy.round());
            let mut render = Render::new(&[Source::Outline]);
            render
                .format(Format::Alpha)
                .offset(Vector::new(gx - ix, 0.0));
            if squeeze != 1.0 {
                render.transform(Some(Transform::scale(squeeze, 1.0)));
            }
            if let Paint::Stroke(w) = paint {
                let mut stroke = Stroke::new(w);
                stroke.join(Join::Round);
                render.style(stroke);
            }
            let Some(image) = render.render(&mut scaler, g.id) else {
                continue;
            };
            if image.placement.width == 0 || image.placement.height == 0 {
                continue;
            }
            pieces.push(Piece {
                x: ix as i32 + image.placement.left,
                y: iy as i32 - image.placement.top,
                w: image.placement.width,
                h: image.placement.height,
                data: image.data,
            });
        }
    });
    if pieces.is_empty() {
        return None;
    }
    let x0 = pieces.iter().map(|p| p.x).min()?;
    let y0 = pieces.iter().map(|p| p.y).min()?;
    let x1 = pieces.iter().map(|p| p.x + p.w as i32).max()?;
    let y1 = pieces.iter().map(|p| p.y + p.h as i32).max()?;
    let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
    let mut data = vec![0u8; (w * h) as usize];
    for p in pieces {
        for row in 0..p.h {
            for col in 0..p.w {
                let v = p.data[(row * p.w + col) as usize];
                if v == 0 {
                    continue;
                }
                let at = ((p.y - y0) as u32 + row) * w + (p.x - x0) as u32 + col;
                let cell = &mut data[at as usize];
                // Глифы одной строки рисуются по очереди: покрытия складываются как
                // source-over одного цвета.
                *cell = (u32::from(*cell) + u32::from(v) * (255 - u32::from(*cell)) / 255).min(255)
                    as u8;
            }
        }
    }
    Some(Coverage {
        x: x0,
        y: y0,
        width: w,
        height: h,
        data,
    })
}

/// Источник цвета текста: сплошной цвет или картинка (градиент) того же размера, что
/// холст.
pub enum Ink<'a> {
    /// Неумноженный RGBA.
    Solid([u8; 4]),
    /// Предумноженная заливка холста (координаты — те же).
    Image(&'a Pixmap),
}

/// Предкоррекция покрытия глифов Skia (`SkMaskGamma`): контраст и гамма краски и
/// устройства. Глифы, растеризованные Skia, «тяжелее» линейного покрытия swash.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextGamma {
    pub contrast: f32,
    pub paint_gamma: f32,
    pub device_gamma: f32,
}

/// Windows — подобрано по эталону 0.4.7 (`tune_text_gamma` в `tests/previews.rs`);
/// Linux — `SK_GAMMA_CONTRAST=0.2`, `SK_GAMMA_EXPONENT=1.2` сборки Chromium.
pub const DEFAULT_GAMMA: TextGamma = if cfg!(windows) {
    TextGamma {
        contrast: 1.0,
        paint_gamma: 2.2,
        device_gamma: 2.2,
    }
} else {
    TextGamma {
        contrast: 0.2,
        paint_gamma: 1.2,
        device_gamma: 1.2,
    }
};

thread_local! {
    static GAMMA: std::cell::Cell<TextGamma> = const { std::cell::Cell::new(DEFAULT_GAMMA) };
    static TABLES: RefCell<Option<(TextGamma, Vec<[u8; 256]>)>> = const { RefCell::new(None) };
}

/// Задать предкоррекцию для этого потока (подбор в тестах).
pub fn set_text_gamma(gamma: TextGamma) {
    GAMMA.with(|g| g.set(gamma));
}

fn to_luma(gamma: f32, v: f32) -> f32 {
    if gamma == 1.0 { v } else { v.powf(gamma) }
}

fn from_luma(gamma: f32, v: f32) -> f32 {
    if gamma == 1.0 { v } else { v.powf(1.0 / gamma) }
}

/// `SkTMaskGamma_build_correcting_lut` для яркости `src` (0..255).
fn correcting_lut(src_i: u8, g: TextGamma) -> [u8; 256] {
    let src = f32::from(src_i) / 255.0;
    let lin_src = to_luma(g.paint_gamma, src);
    let dst = 1.0 - src;
    let lin_dst = to_luma(g.device_gamma, dst);
    let contrast = g.contrast * lin_dst;
    let apply = |a: f32| a + (1.0 - a) * contrast * a;
    let mut table = [0u8; 256];
    for (i, cell) in table.iter_mut().enumerate() {
        let raw = i as f32 / 255.0;
        let srca = apply(raw);
        let v = if (src - dst).abs() < 1.0 / 256.0 {
            srca
        } else {
            let out = from_luma(g.device_gamma, lin_src * srca + (1.0 - srca) * lin_dst);
            (out - dst) / (src - dst)
        };
        *cell = (255.0 * v).round().clamp(0.0, 255.0) as u8;
    }
    table
}

/// Таблица предкоррекции покрытия для цвета текста (3 бита яркости, как
/// `SkTMaskGamma<3, 3, 3>`).
pub fn coverage_table(rgb: [u8; 3]) -> [u8; 256] {
    let g = GAMMA.with(std::cell::Cell::get);
    TABLES.with(|cell| {
        let mut cell = cell.borrow_mut();
        if cell.as_ref().is_none_or(|(have, _)| *have != g) {
            let tables = (0u8..8)
                .map(|i| correcting_lut((u32::from(i) * 255 / 7) as u8, g))
                .collect();
            *cell = Some((g, tables));
        }
        let [r, gg, b] = rgb.map(|c| to_luma(g.paint_gamma, f32::from(c) / 255.0));
        let luma = from_luma(g.paint_gamma, r * 0.2126 + gg * 0.7152 + b * 0.0722);
        let lum = (luma * 255.0).round().clamp(0.0, 255.0) as u8;
        cell.as_ref()
            .map(|(_, t)| t[usize::from(lum >> 5)])
            .unwrap_or([0; 256])
    })
}

/// Положить покрытие на холст (`source-over`, `globalAlpha`).
pub fn draw(pixmap: &mut Pixmap, cover: &Coverage, ink: &Ink, global_alpha: f32) {
    let (tw, th) = (pixmap.width() as i32, pixmap.height() as i32);
    let ga = global_alpha.clamp(0.0, 1.0);
    // Краска не сплошная — средний серый (`ComputeLuminanceColor` Skia).
    let table = coverage_table(match ink {
        Ink::Solid([r, g, b, _]) => [*r, *g, *b],
        Ink::Image(_) => [0x7f, 0x80, 0x7f],
    });
    for row in 0..cover.height as i32 {
        let y = cover.y + row;
        if y < 0 || y >= th {
            continue;
        }
        for col in 0..cover.width as i32 {
            let x = cover.x + col;
            if x < 0 || x >= tw {
                continue;
            }
            let c = f32::from(
                table[usize::from(cover.data[(row * cover.width as i32 + col) as usize])],
            ) / 255.0;
            if c == 0.0 {
                continue;
            }
            let (r, g, b, a) = match ink {
                Ink::Solid([r, g, b, a]) => {
                    let a = f32::from(*a) / 255.0;
                    (
                        f32::from(*r) * a,
                        f32::from(*g) * a,
                        f32::from(*b) * a,
                        a * 255.0,
                    )
                }
                Ink::Image(src) => {
                    let p = src.pixels()[(y * tw + x) as usize];
                    (
                        f32::from(p.red()),
                        f32::from(p.green()),
                        f32::from(p.blue()),
                        f32::from(p.alpha()),
                    )
                }
            };
            let k = c * ga;
            let (sr, sg, sb, sa) = (r * k, g * k, b * k, a * k);
            let at = (y * tw + x) as usize;
            let d = pixmap.pixels()[at];
            let inv = 1.0 - sa / 255.0;
            let mix = |s: f32, d: u8| (s + f32::from(d) * inv).round().clamp(0.0, 255.0) as u8;
            let out_a = mix(sa, d.alpha());
            let out = PremultipliedColorU8::from_rgba(
                mix(sr, d.red()).min(out_a),
                mix(sg, d.green()).min(out_a),
                mix(sb, d.blue()).min(out_a),
                out_a,
            );
            if let Some(out) = out {
                pixmap.pixels_mut()[at] = out;
            }
        }
    }
}

/// Ключ покрытия строки: текст, шрифт, место и заливка (дробные — битами f32).
#[derive(PartialEq, Eq, Hash)]
struct TextKey {
    text: String,
    weight: u16,
    size: u32,
    families: Vec<String>,
    x: u32,
    y: u32,
    align: u8,
    baseline: u8,
    max_width: Option<u32>,
    stroke: Option<u32>,
}

/// Строк в кэше покрытий; при переполнении кэш очищается.
const TEXT_CACHE_LIMIT: usize = 512;

/// Покрытие строки (нет — пустая строка) и её ширина.
type Covered = (Option<Rc<Coverage>>, f32);

thread_local! {
    // Покрытия строк превью: при «движении» кадр рисуется заново 60 раз в секунду, а
    // подписи HUD, панель клавиш и cl_showfps от кадра к кадру те же.
    static TEXT_CACHE: RefCell<HashMap<TextKey, Covered>> = RefCell::new(HashMap::new());
}

/// `fillText`/`strokeText` целиком: разложить, покрыть и положить.
pub fn text(
    pixmap: &mut Pixmap,
    value: &str,
    font: &Font,
    place: Place,
    paint: Paint,
    ink: &Ink,
    global_alpha: f32,
) -> f32 {
    let key = TextKey {
        text: value.to_owned(),
        weight: font.weight,
        size: font.size.to_bits(),
        families: font.families.clone(),
        x: place.x.to_bits(),
        y: place.y.to_bits(),
        align: place.align as u8,
        baseline: place.baseline as u8,
        max_width: place.max_width.map(f32::to_bits),
        stroke: match paint {
            Paint::Fill => None,
            Paint::Stroke(w) => Some(w.to_bits()),
        },
    };
    let cached = TEXT_CACHE.with(|cache| cache.borrow().get(&key).cloned());
    let (cover, width) = match cached {
        Some(hit) => hit,
        None => {
            let line = layout(value, font);
            let cover = coverage(&line, place, paint).map(Rc::new);
            TEXT_CACHE.with(|cache| {
                let mut cache = cache.borrow_mut();
                if cache.len() >= TEXT_CACHE_LIMIT {
                    cache.clear();
                }
                cache.insert(key, (cover.clone(), line.width));
            });
            (cover, line.width)
        }
    };
    if let Some(cover) = cover {
        draw(pixmap, &cover, ink, global_alpha);
    }
    width
}
