//! Оформление ника (`NickName.vue`, `launcher.css:719-776` 0.4.7, `735-843` 0.4.9; задача 38,
//! решение 13): готовый стиль (`.nick--developer`, `--gold`, `--ghost`, `data/nick-styles.json`)
//! или свой CSS (админка; с 0.4.9 — и заготовка подсветки, которую выбрал поддержавший проект:
//! CSS приходит с сервера, поэтому разбирается на лету, а не берётся из замороженного каталога)
//! → картинка ника с кадрами по времени: градиент на тексте (`background-clip: text`) с
//! движением `background-position` (`nick-shift`), несколько `text-shadow`, свечение
//! `filter: drop-shadow()`, `brightness` (`nick-glow`), прозрачность (`nick-pulse`). Незнакомое
//! свойство — `Err`: экран рисует обычный ник и пишет строку в журнал.
//!
//! Значок подписчика 0.4.9 (`.nick-badge`): картинка 1.05em слева от ника, свечение двумя
//! `drop-shadow` «дышит» 3,6 с ([`badge`]); подсказка — [`badge_title`].

use super::canvas_text::{self, Align, Baseline, Font, Paint as TextPaint, Place};
use super::css::{self, Anim, Filter, Image, Length, Rgba, Shadow};
use crate::motion::{Registry, Still};
use hlsr_core::language::Language;
use serde::Deserialize;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use tiny_skia::{Pixmap, PremultipliedColorU8};

/// Заливка текста.
#[derive(Debug, Clone, PartialEq)]
enum Fill {
    /// Цвет текста (`color`, `-webkit-text-fill-color`); `None` — унаследованный.
    Color(Option<Rgba>),
    /// `background-clip: text`: градиент на буквах; `size` — доля ширины по X
    /// (`background-size: 200% auto` → 2).
    Gradient { image: Image, size: f32 },
}

/// Разобранное оформление.
#[derive(Debug, Clone)]
pub struct Look {
    fill: Fill,
    text_shadows: Vec<Shadow>,
    filters: Vec<Filter>,
    weight: Option<u16>,
    letter_spacing: Option<Length>,
    uppercase: Option<bool>,
    animation: Option<Anim>,
    /// Реестр движения с анимацией оформления под ключом [`MOTION_KEY`] (если она есть).
    motion: Option<Arc<Registry>>,
}

const MOTION_KEY: &str = "nick";

/// Разобрать CSS ника; незнакомое свойство или значение — `None`.
pub fn parse(css_text: &str) -> Option<Look> {
    let mut color = None;
    let mut text_fill = None;
    let mut background: Option<Image> = None;
    let mut size = 1.0f32;
    let mut clip_text = false;
    let mut look = Look {
        fill: Fill::Color(None),
        text_shadows: Vec::new(),
        filters: Vec::new(),
        weight: None,
        letter_spacing: None,
        uppercase: None,
        animation: None,
        motion: None,
    };
    for (name, value) in css::declarations(css_text) {
        match name.as_str() {
            "color" => color = Some(css::color(&value)?),
            "-webkit-text-fill-color" => text_fill = Some(css::color(&value)?),
            "background" | "background-image" => background = Some(css::image(&value)?),
            "background-size" => {
                let words = css::tokens(&value);
                let x = words.first()?.strip_suffix('%')?.parse::<f32>().ok()? / 100.0;
                if words.get(1).is_some_and(|y| *y != "auto") || words.len() > 2 {
                    return None;
                }
                size = x;
            }
            "background-clip" | "-webkit-background-clip" => {
                if value != "text" {
                    return None;
                }
                clip_text = true;
            }
            "text-shadow" => look.text_shadows = css::shadows(&value, false)?,
            "filter" => look.filters = css::filters(&value)?,
            "font-weight" => {
                look.weight = Some(match value.as_str() {
                    "bold" => 700,
                    "normal" => 400,
                    v => v.parse::<f32>().ok()?.clamp(1.0, 1000.0) as u16,
                })
            }
            "letter-spacing" => {
                look.letter_spacing = Some(if value == "normal" {
                    Length::Px(0.0)
                } else {
                    css::length(&value)?
                })
            }
            "text-transform" => {
                look.uppercase = match value.as_str() {
                    "uppercase" => Some(true),
                    "none" => None,
                    _ => return None,
                }
            }
            "animation" => look.animation = Some(css::animation(&value)?),
            _ => return None,
        }
    }
    let transparent = |c: Option<Rgba>| c.is_some_and(|c| c[3] == 0.0);
    look.fill = match background {
        Some(image)
            if clip_text
                && (transparent(text_fill) || (text_fill.is_none() && transparent(color))) =>
        {
            Fill::Gradient { image, size }
        }
        // Градиент без обрезки по тексту закрасил бы прямоугольник ника — не поддерживается.
        Some(_) => return None,
        None => Fill::Color(text_fill.or(color)),
    };
    look.motion = look
        .animation
        .as_ref()
        .map(|anim| Arc::new(anim.registry(MOTION_KEY)));
    Some(look)
}

#[derive(Deserialize)]
struct CatalogFile {
    presets: BTreeMap<String, String>,
}

/// Готовые стили `.nick--<id>` (launcher.css).
fn presets() -> &'static BTreeMap<String, Arc<Look>> {
    static PRESETS: OnceLock<BTreeMap<String, Arc<Look>>> = OnceLock::new();
    PRESETS.get_or_init(|| {
        let file: CatalogFile = serde_json::from_str(include_str!("../../data/nick-styles.json"))
            .expect("data/nick-styles.json");
        file.presets
            .into_iter()
            .map(|(id, text)| {
                let look = parse(&text).unwrap_or_else(|| panic!("nick-styles.json: {id}"));
                (id, Arc::new(look))
            })
            .collect()
    })
}

/// CSS ника так, как его применяет 0.4.9 (`sanitizeCss`: сервер и main).
pub fn normalize_css(text: &str) -> String {
    hlsr_online::shared::nick_styles::sanitize_css(&serde_json::Value::String(text.to_owned()))
        .unwrap_or_default()
}

/// Оформление ника `{style, css}`: готовый стиль, иначе свой CSS. `Ok(None)` — оформления
/// нет; `Err` — оно незнакомо (текст для журнала).
pub fn look_of(style: &str, css_text: &str) -> Result<Option<Arc<Look>>, String> {
    if !style.is_empty() {
        return presets()
            .get(style)
            .cloned()
            .map(Some)
            .ok_or_else(|| format!("незнакомый стиль ника «{style}»"));
    }
    let key = normalize_css(css_text);
    if key.is_empty() {
        return Ok(None);
    }
    static PARSED: css::Parsed<Look> = css::Parsed::new();
    PARSED
        .get(&key, parse)
        .map(Some)
        .ok_or_else(|| format!("CSS ника не поддерживается: {key}"))
}

/// Ник для отрисовки: текст и шрифт места, где он стоит.
#[derive(Debug, Clone)]
pub struct NickText<'a> {
    pub text: &'a str,
    /// Кегль, px CSS.
    pub size: f32,
    pub weight: u16,
    /// `letter-spacing`, px CSS.
    pub letter_spacing: f32,
    /// Унаследованный цвет текста.
    pub color: Rgba,
    /// Масштаб устройства.
    pub scale: f32,
}

/// Картинка ника в физических пикселях; места — в логических px от левого верхнего угла
/// строки ника (верх области содержимого шрифта).
pub struct NickArt {
    pub pixels: Pixmap,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Ширина ника (`advance`) и высота области содержимого, px CSS.
    pub advance: f32,
    pub line_height: f32,
    /// Подъём шрифта (от верха области содержимого до линии букв), px CSS.
    pub ascent: f32,
    /// Прозрачность элемента (`nick-pulse`).
    pub opacity: f32,
}

/// Кадр анимации оформления в момент `elapsed` (мс от начала; `None` — t = 0).
pub fn frame(look: &Look, elapsed: Option<f64>, still: Still) -> crate::motion::Frame {
    match &look.motion {
        Some(registry) => {
            registry.keyframes_frame(MOTION_KEY, Some(elapsed.unwrap_or(0.0)), 0, still, true)
        }
        None => crate::motion::Frame::REST,
    }
}

/// Анимация оформления меняется со временем (без неё и в режиме «без анимаций» —
/// картинка постоянна).
pub fn moving(look: &Look, still: Still) -> bool {
    look.motion
        .as_ref()
        .is_some_and(|registry| registry.keyframes_moving(MOTION_KEY, still))
}

/// Цикл анимации оформления, мс (`Registry::keyframes_cycle`).
pub fn cycle(look: &Look, still: Still) -> Option<(f64, f64)> {
    look.motion.as_ref()?.keyframes_cycle(MOTION_KEY, still)
}

/// Анимация меняет только `opacity` (`nick-pulse`): пиксели постоянны, перерастр не нужен.
pub fn opacity_only(look: &Look, still: Still) -> bool {
    moving(look, still)
        && look
            .animation
            .as_ref()
            .is_some_and(|anim| anim.name == "nick-pulse")
}

/// Значение `opacity` анимации в момент `elapsed` (мс).
pub fn opacity_at(look: &Look, elapsed: f64, still: Still) -> f32 {
    frame(look, Some(elapsed), still).opacity as f32
}

/// Как Chromium размывает `drop-shadow()`: σ = радиус / 2, как у `text-shadow`.
const DROP_SIGMA: f32 = 0.5;

/// Знаков ника в растре (имя Steam — до 32, профиль обрезает до 64).
const MAX_CHARS: usize = 256;

/// Ширина текста в растре, физ. px.
const MAX_WIDTH_PX: f32 = 8192.0;

/// Предварительно собранный градиент: `resolve_stops` и `sin/cos` считаются один раз на кадр,
/// а не на каждый пиксель.
pub(crate) enum PreparedImage {
    Linear {
        sin: f32,
        cos: f32,
        inv_len: f32,
        half_w: f32,
        half_h: f32,
        stops: Vec<(f32, Rgba)>,
    },
    Conic {
        half_w: f32,
        half_h: f32,
        stops: Vec<(f32, Rgba)>,
    },
}

impl PreparedImage {
    pub(crate) fn new(image: &Image, w: f32, h: f32) -> Self {
        match image {
            Image::Linear { angle, stops } => {
                let (sin, cos) = angle.to_radians().sin_cos();
                let length = ((w * sin).abs() + (h * cos).abs()).max(f32::EPSILON);
                Self::Linear {
                    sin,
                    cos,
                    inv_len: 1.0 / length,
                    half_w: w * 0.5,
                    half_h: h * 0.5,
                    stops: css::resolve_stops(stops),
                }
            }
            Image::Conic { stops, .. } => Self::Conic {
                half_w: w * 0.5,
                half_h: h * 0.5,
                stops: css::resolve_stops(stops),
            },
        }
    }

    #[inline]
    pub(crate) fn color(&self, u: f32, v: f32) -> Rgba {
        match self {
            Self::Linear {
                sin,
                cos,
                inv_len,
                half_w,
                half_h,
                stops,
            } => {
                let t = ((u - half_w) * sin - (v - half_h) * cos) * inv_len + 0.5;
                css::gradient_at(stops, t.clamp(0.0, 1.0))
            }
            Self::Conic {
                half_w,
                half_h,
                stops,
            } => {
                let a = (u - half_w)
                    .atan2(-(v - half_h))
                    .to_degrees()
                    .rem_euclid(360.0);
                css::gradient_at(stops, a / 360.0)
            }
        }
    }
}

/// Не зависящая от времени основа ника: раскладка глифов, ненулевые пиксели маски и заранее
/// размытые `text-shadow` (а при непрозрачной заливке и чистых `drop-shadow` — и они тоже).
struct NickBase {
    cw: i32,
    ch: i32,
    pad: i32,
    advance_px: f32,
    ascent: f32,
    content_h: f32,
    empty: bool,
    non_zero: Vec<(usize, u8)>,
    under: Pixmap,
    prebaked_drops: bool,
}

const NICK_BASE_LIMIT: usize = 64;

thread_local! {
    static NICK_BASES: RefCell<HashMap<String, Rc<NickBase>>> = RefCell::new(HashMap::new());
}

fn fill_is_opaque(fill: &Fill, inherited: Rgba) -> bool {
    match fill {
        Fill::Color(c) => c.unwrap_or(inherited)[3] >= 0.999,
        Fill::Gradient { image, .. } => match image {
            Image::Linear { stops, .. } | Image::Conic { stops, .. } => {
                stops.iter().all(|stop| stop.color[3] >= 0.999)
            }
        },
    }
}

fn nick_base(look: &Look, text: &NickText) -> Option<Rc<NickBase>> {
    let s = text.scale.max(0.01);
    let weight = look.weight.unwrap_or(text.weight);
    let em = text.size;
    let spacing = look
        .letter_spacing
        .map_or(text.letter_spacing, |l| l.px(em));
    let shown: String = text.text.chars().take(MAX_CHARS).collect();
    let value = if look.uppercase == Some(true) {
        shown.to_uppercase()
    } else {
        shown
    };
    let opaque_fill = fill_is_opaque(&look.fill, text.color);
    let only_drops = !look.filters.is_empty()
        && look
            .filters
            .iter()
            .all(|f| matches!(f, Filter::DropShadow(_)));
    let can_prebake_drops = opaque_fill && only_drops;
    let table_rgb = match &look.fill {
        Fill::Color(c) => {
            let c = c.unwrap_or(text.color);
            [to8(c[0]), to8(c[1]), to8(c[2])]
        }
        Fill::Gradient { .. } => [0x7f, 0x80, 0x7f],
    };
    let key = format!(
        "{value}|{weight}|{em}|{spacing}|{s}|{:?}|{:?}|{can_prebake_drops}|{table_rgb:?}",
        look.text_shadows,
        if can_prebake_drops {
            &look.filters[..]
        } else {
            &[]
        }
    );
    if let Some(hit) = NICK_BASES.with(|bases| bases.borrow().get(&key).cloned()) {
        return Some(hit);
    }
    let font = Font {
        weight,
        size: em * s,
        families: vec!["Rubik".to_owned()],
    };
    let line = canvas_text::layout_spaced(&value, &font, spacing * s);
    let ascent = line.metrics.ascent;
    let descent = line.metrics.descent;
    let advance_px = line.width;
    let mut reach = 0.0f32;
    for sh in &look.text_shadows {
        reach = reach.max(sh.blur.px(em) * 1.5 + sh.x.px(em).abs().max(sh.y.px(em).abs()));
    }
    for filter in &look.filters {
        if let Filter::DropShadow(sh) = filter {
            reach = reach
                .max(sh.blur.px(em) * DROP_SIGMA * 3.0 + sh.x.px(em).abs().max(sh.y.px(em).abs()));
        }
    }
    let pad = (reach.min(css::MAX_REACH) * s).ceil() as i32 + 2;
    let width = advance_px.ceil().min(MAX_WIDTH_PX) as i32 + pad * 2;
    let height = (ascent + descent).ceil() as i32 + pad * 2;
    let mut under = Pixmap::new(width.max(1) as u32, height.max(1) as u32)?;
    let cover = canvas_text::coverage(
        &line,
        Place {
            x: pad as f32,
            y: pad as f32 + ascent,
            align: Align::Left,
            baseline: Baseline::Alphabetic,
            max_width: None,
        },
        TextPaint::Fill,
    );
    let (cw, ch) = (under.width() as i32, under.height() as i32);
    let content_h = ascent + descent;
    let base = match cover {
        None => Rc::new(NickBase {
            cw,
            ch,
            pad,
            advance_px,
            ascent,
            content_h,
            empty: true,
            non_zero: Vec::new(),
            under,
            prebaked_drops: can_prebake_drops,
        }),
        Some(cover) => {
            let mut mask = vec![0u8; (cw * ch) as usize];
            let mut non_zero = Vec::new();
            for row in 0..cover.height as i32 {
                for col in 0..cover.width as i32 {
                    let (x, y) = (cover.x + col, cover.y + row);
                    if x >= 0 && y >= 0 && x < cw && y < ch {
                        let m = cover.data[(row * cover.width as i32 + col) as usize];
                        if m != 0 {
                            let idx = (y * cw + x) as usize;
                            mask[idx] = m;
                            non_zero.push((idx, m));
                        }
                    }
                }
            }
            for sh in look.text_shadows.iter().rev() {
                paint_shadow(&mut under, &mask, sh, em, s, 0.5);
            }
            if can_prebake_drops {
                // Суммарная альфа `(text_shadows over непрозрачный текст)` не зависит от сдвига
                // градиента, а `source-over` ассоциативен — можно запечь `drop-shadow` заранее.
                let table = canvas_text::coverage_table(table_rgb);
                let mut alpha: Vec<u8> = under.pixels().iter().map(|p| p.alpha()).collect();
                for &(i, m) in &non_zero {
                    let k = f32::from(table[usize::from(m)]) / 255.0;
                    let a = (k * 255.0 + f32::from(alpha[i]) * (1.0 - k))
                        .round()
                        .clamp(0.0, 255.0) as u8;
                    alpha[i] = a;
                }
                for filter in &look.filters {
                    if let Filter::DropShadow(sh) = filter {
                        let mut drop = Pixmap::new(under.width(), under.height())?;
                        paint_shadow(&mut drop, &alpha, sh, em, s, DROP_SIGMA);
                        for (i, (d, u)) in
                            drop.pixels_mut().iter_mut().zip(under.pixels()).enumerate()
                        {
                            let da = d.alpha();
                            let ca = alpha[i];
                            let inv = 255 - u32::from(ca);
                            alpha[i] =
                                (u32::from(ca) + (u32::from(da) * inv + 127) / 255).min(255) as u8;
                            *d = over_pm(*d, *u);
                        }
                        under = drop;
                    }
                }
            }
            Rc::new(NickBase {
                cw,
                ch,
                pad,
                advance_px,
                ascent,
                content_h,
                empty: false,
                non_zero,
                under,
                prebaked_drops: can_prebake_drops,
            })
        }
    };
    NICK_BASES.with(|bases| {
        let mut bases = bases.borrow_mut();
        if bases.len() >= NICK_BASE_LIMIT {
            bases.clear();
        }
        bases.insert(key, base.clone());
    });
    Some(base)
}

/// Нарисовать ник в момент `elapsed`.
pub fn render(look: &Look, text: &NickText, elapsed: Option<f64>, still: Still) -> Option<NickArt> {
    let f = frame(look, elapsed, still);
    let s = text.scale.max(0.01);
    let em = text.size;
    let base = nick_base(look, text)?;
    let mut canvas = base.under.clone();
    if base.empty {
        return Some(NickArt {
            pixels: canvas,
            x: -(base.pad as f32) / s,
            y: -(base.pad as f32) / s,
            width: base.cw as f32 / s,
            height: base.ch as f32 / s,
            advance: base.advance_px / s,
            line_height: base.content_h / s,
            ascent: base.ascent / s,
            opacity: 1.0,
        });
    }
    let (cw, pad, content_h, advance_px) = (base.cw, base.pad, base.content_h, base.advance_px);
    // Текст.
    let px = canvas.pixels_mut();
    match &look.fill {
        Fill::Color(c) => {
            let c = c.unwrap_or(text.color);
            let table = canvas_text::coverage_table([to8(c[0]), to8(c[1]), to8(c[2])]);
            for &(i, m) in &base.non_zero {
                let k = f32::from(table[usize::from(m)]) / 255.0 * c[3];
                px[i] = over(px[i], [c[0], c[1], c[2]], k);
            }
        }
        Fill::Gradient { image, size } => {
            let w = advance_px;
            let tile_w = w * size;
            // background-position-x в долях (nick-shift: 0 → 200 %): (W − Wt) · p.
            let offset = (w - tile_w) * f.shift as f32;
            let table = canvas_text::coverage_table([0x7f, 0x80, 0x7f]);
            let prepared = PreparedImage::new(image, tile_w, content_h);
            let tile_mod = tile_w.max(1.0);
            let pad_f = pad as f32;
            for &(i, m) in &base.non_zero {
                let (x, y) = ((i as i32 % cw) as f32 + 0.5, (i as i32 / cw) as f32 + 0.5);
                let u = (x - pad_f - offset).rem_euclid(tile_mod);
                let v = y - pad_f;
                let c = prepared.color(u, v);
                let k = f32::from(table[usize::from(m)]) / 255.0 * c[3];
                px[i] = over(px[i], [c[0], c[1], c[2]], k);
            }
        }
    }
    // Фильтры по порядку: свечение под картинкой, яркость — на всё.
    let brightness = if look
        .animation
        .as_ref()
        .is_some_and(|a| a.name == "nick-glow")
    {
        f.brightness as f32
    } else {
        1.0
    };
    if !base.prebaked_drops {
        for filter in &look.filters {
            match filter {
                Filter::DropShadow(sh) => {
                    let alpha: Vec<u8> = canvas.pixels().iter().map(|p| p.alpha()).collect();
                    let mut under = Pixmap::new(canvas.width(), canvas.height())?;
                    paint_shadow(&mut under, &alpha, sh, em, s, DROP_SIGMA);
                    // Картинка поверх своей тени.
                    for (u, c) in under.pixels_mut().iter_mut().zip(canvas.pixels()) {
                        *u = over_pm(*u, *c);
                    }
                    canvas = under;
                }
                Filter::Brightness(b) => apply_brightness(&mut canvas, *b),
                Filter::HueRotate(_) => return None,
            }
        }
    }
    if brightness != 1.0 {
        apply_brightness(&mut canvas, brightness);
    }
    Some(NickArt {
        pixels: canvas,
        x: -(base.pad as f32) / s,
        y: -(base.pad as f32) / s,
        width: base.cw as f32 / s,
        height: base.ch as f32 / s,
        advance: base.advance_px / s,
        line_height: content_h / s,
        ascent: base.ascent / s,
        opacity: f.opacity as f32,
    })
}

/// Сторона значка подписчика, em (`.nick-badge`: `width`/`height: 1.05em`).
pub const BADGE_EM: f32 = 1.05;
/// Поле между значком и ником, em (`margin-right: .3em`).
pub const BADGE_GAP_EM: f32 = 0.3;
/// Низ значка ниже линии букв, em (`margin-bottom: -.2em` при `vertical-align: baseline`);
/// отрицательные поля сверху и снизу не дают значку поднять строку.
pub const BADGE_DROP_EM: f32 = 0.2;
/// `@keyframes nick-badge-glow` (3,6 с `ease-in-out infinite alternate`): доля `mix`.
const BADGE_GLOW: &str = "nick-badge-glow";

/// Картинка значка с его свечением в физических px; места — в логических px от левого
/// верхнего угла самого значка.
pub struct BadgeArt {
    pub pixels: Pixmap,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Голубой и синий `drop-shadow(0 0 <размытие>em <цвет>)` значка в момент `t` дыхания
/// (`None` — анимации нет: свечение из самого правила `.nick-badge`).
fn badge_glow(t: Option<f32>) -> [(f32, Rgba); 2] {
    let cyan = |a: f32| [56.0 / 255.0, 189.0 / 255.0, 248.0 / 255.0, a];
    let blue = |a: f32| [59.0 / 255.0, 130.0 / 255.0, 246.0 / 255.0, a];
    match t {
        None => [(0.2, cyan(0.75)), (0.5, blue(0.45))],
        Some(t) => {
            // Список фильтров интерполируется по функциям: размытие и цвет (одни RGB — альфа).
            let lerp = |a: f32, b: f32| a + (b - a) * t;
            [
                (lerp(0.15, 0.25), cyan(lerp(0.6, 0.9))),
                (lerp(0.4, 0.65), blue(lerp(0.3, 0.55))),
            ]
        }
    }
}

/// Значок «дышит» (без анимаций свечение стоит).
pub fn badge_moving(still: Still) -> bool {
    crate::motion::registry().keyframes_moving(BADGE_GLOW, still)
}

/// Цикл дыхания значка, мс (`Registry::keyframes_cycle`).
pub fn badge_cycle(still: Still) -> Option<(f64, f64)> {
    crate::motion::registry().keyframes_cycle(BADGE_GLOW, still)
}

/// Доля дыхания значка `mix ∈ [0, 1]` в момент `elapsed` (мс); `None` — свечение стоит.
pub fn badge_mix(elapsed: f64, still: Still) -> Option<f32> {
    badge_moving(still).then(|| {
        crate::motion::registry()
            .keyframes_frame(BADGE_GLOW, Some(elapsed.max(0.0)), 0, still, true)
            .mix as f32
    })
}

/// Разложенный значок подписчика: сам значок и два слоя свечения (`t = 0` и `t = 1`),
/// которые смешиваются на GPU по `opacity(1 - t)` / `opacity(t)` на полной частоте монитора.
pub struct BadgeParts {
    pub icon: Pixmap,
    pub glow_min: Pixmap,
    pub glow_max: Pixmap,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

fn badge_icon_image() -> &'static image::RgbaImage {
    static ICON: OnceLock<image::RgbaImage> = OnceLock::new();
    ICON.get_or_init(|| {
        image::load_from_memory(include_bytes!("../../assets/launcher/subscriber-badge.png"))
            .expect("assets/launcher/subscriber-badge.png")
            .into_rgba8()
    })
}

fn badge_glow_only(icon: &Pixmap, em: f32, s: f32, t: Option<f32>) -> Option<Pixmap> {
    let (total_w, total_h) = (icon.width(), icon.height());
    let glows = badge_glow(t);
    let mut glow_accum = Pixmap::new(total_w, total_h)?;
    let mut combined_alpha: Vec<u8> = icon.pixels().iter().map(|p| p.alpha()).collect();
    for (blur, color) in glows {
        let shadow = Shadow {
            x: Length::Px(0.0),
            y: Length::Px(0.0),
            blur: Length::Em(blur),
            spread: Length::Px(0.0),
            color,
        };
        let mut under = Pixmap::new(total_w, total_h)?;
        paint_shadow(&mut under, &combined_alpha, &shadow, em, s, DROP_SIGMA);
        for (i, (u, g)) in under
            .pixels_mut()
            .iter_mut()
            .zip(glow_accum.pixels())
            .enumerate()
        {
            let ua = u.alpha();
            let ca = combined_alpha[i];
            let inv = 255 - u32::from(ca);
            combined_alpha[i] = (u32::from(ca) + (u32::from(ua) * inv + 127) / 255).min(255) as u8;
            *u = over_pm(*u, *g);
        }
        glow_accum = under;
    }
    Some(glow_accum)
}

/// Значок подписчика разложен на иконку и крайние свечения (для плавного смешения на GPU).
pub fn badge_parts(em: f32, scale: f32) -> Option<BadgeParts> {
    let icon_img = badge_icon_image();
    let s = scale.max(0.01);
    let side = (em * BADGE_EM * s).round().max(1.0) as u32;
    let reach: f32 = badge_glow(Some(1.0))
        .iter()
        .map(|(blur, _)| blur * em * DROP_SIGMA * 3.0)
        .sum();
    let pad = (reach * s).ceil() as u32 + 1;
    let total = side + pad * 2;
    let mut icon = Pixmap::new(total, total)?;
    let small =
        image::imageops::resize(icon_img, side, side, image::imageops::FilterType::Triangle);
    let px = icon.pixels_mut();
    for (y, row) in small.rows().enumerate() {
        for (x, p) in row.enumerate() {
            let [r, g, b, a] = p.0;
            let pm = |c: u8| super::raster::premultiply(c, a);
            if let Some(c) = PremultipliedColorU8::from_rgba(pm(r), pm(g), pm(b), a) {
                px[(y + pad as usize) * total as usize + x + pad as usize] = c;
            }
        }
    }
    let glow_min = badge_glow_only(&icon, em, s, Some(0.0))?;
    let glow_max = badge_glow_only(&icon, em, s, Some(1.0))?;
    let at = -(pad as f32) / s;
    Some(BadgeParts {
        icon,
        glow_min,
        glow_max,
        x: at,
        y: at,
        width: total as f32 / s,
        height: total as f32 / s,
    })
}

/// Значок подписчика у ника кеглем `em` px в момент `elapsed` (мс; `None` — t = 0).
pub fn badge(em: f32, scale: f32, elapsed: Option<f64>, still: Still) -> Option<BadgeArt> {
    let icon = badge_icon_image();
    let s = scale.max(0.01);
    let t = badge_moving(still).then(|| {
        let registry = crate::motion::registry();
        registry
            .keyframes_frame(BADGE_GLOW, Some(elapsed.unwrap_or(0.0)), 0, still, true)
            .mix as f32
    });
    // Картинка вписана в целые физические px, как у Blink (снап прямоугольника `<img>`).
    let side = (em * BADGE_EM * s).round().max(1.0) as u32;
    // Поле под оба свечения в самом сильном кадре: размер холста от времени не зависит.
    let reach: f32 = badge_glow(Some(1.0))
        .iter()
        .map(|(blur, _)| blur * em * DROP_SIGMA * 3.0)
        .sum();
    let pad = (reach * s).ceil() as u32 + 1;
    let total = side + pad * 2;
    let mut canvas = Pixmap::new(total, total)?;
    let small = image::imageops::resize(icon, side, side, image::imageops::FilterType::Triangle);
    let px = canvas.pixels_mut();
    for (y, row) in small.rows().enumerate() {
        for (x, p) in row.enumerate() {
            let [r, g, b, a] = p.0;
            let pm = |c: u8| super::raster::premultiply(c, a);
            if let Some(c) = PremultipliedColorU8::from_rgba(pm(r), pm(g), pm(b), a) {
                px[(y + pad as usize) * total as usize + x + pad as usize] = c;
            }
        }
    }
    // filter: drop-shadow(A) drop-shadow(B) — B отбрасывает уже картинка вместе с тенью A.
    for (blur, color) in badge_glow(t) {
        let shadow = Shadow {
            x: Length::Px(0.0),
            y: Length::Px(0.0),
            blur: Length::Em(blur),
            spread: Length::Px(0.0),
            color,
        };
        let alpha: Vec<u8> = canvas.pixels().iter().map(|p| p.alpha()).collect();
        let mut under = Pixmap::new(total, total)?;
        paint_shadow(&mut under, &alpha, &shadow, em, s, DROP_SIGMA);
        for (u, c) in under.pixels_mut().iter_mut().zip(canvas.pixels()) {
            *u = over_pm(*u, *c);
        }
        canvas = under;
    }
    let at = -(pad as f32) / s;
    Some(BadgeArt {
        pixels: canvas,
        x: at,
        y: at,
        width: total as f32 / s,
        height: total as f32 / s,
    })
}

/// Полных календарных месяцев поддержки (`support-months.js` 0.4.9): с `since` (unix-секунды)
/// до `now_ms`, по UTC. Меньше месяца, будущее и мусор — 0.
pub fn support_months(since: i64, now_ms: f64) -> i64 {
    use chrono::{DateTime, Datelike, Timelike, Utc};
    if since <= 0 || !now_ms.is_finite() {
        return 0;
    }
    let (Some(from), Some(now)) = (
        DateTime::from_timestamp(since, 0),
        DateTime::from_timestamp_millis(now_ms as i64),
    ) else {
        return 0;
    };
    let mut months =
        i64::from(now.year() - from.year()) * 12 + i64::from(now.month()) - i64::from(from.month());
    // День и время месяца ещё не наступили — последний месяц неполный.
    let rest = |d: &DateTime<Utc>| {
        ((i64::from(d.day()) * 24 + i64::from(d.hour())) * 60 + i64::from(d.minute())) * 60
            + i64::from(d.second())
    };
    if rest(&now) < rest(&from) {
        months -= 1;
    }
    months.max(0)
}

/// Подсказка значка (`badgeTitle` NickName.vue 0.4.9): «Поддерживает HLSR N мес. — с
/// ДД.ММ.ГГГГ» (дата — местный день игрока), меньше месяца — своей строкой; без
/// `supporterSince` (списки) — «Подписчик HLSR».
pub fn badge_title(lang: Language, since: Option<i64>, now_ms: f64) -> String {
    use chrono::TimeZone;
    let tr = |key: &str, args: &[&str]| crate::tr::tr(lang, key, args);
    let day = since
        .filter(|since| *since != 0)
        .and_then(|since| chrono::Local.timestamp_opt(since, 0).single());
    let (Some(since), Some(day)) = (since, day) else {
        return tr("#L_NICK_SUBSCRIBER", &[]);
    };
    let day = day.format("%d.%m.%Y").to_string();
    match support_months(since, now_ms) {
        0 => tr("#L_NICK_SUPPORTER_NEW", &["", &day]),
        months => tr("#L_NICK_SUPPORTER_MONTHS", &[&months.to_string(), &day]),
    }
}

fn to8(v: f32) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

/// `source-over` неумноженного цвета с покрытием `k` поверх предумноженного пикселя.
fn over(d: PremultipliedColorU8, c: [f32; 3], k: f32) -> PremultipliedColorU8 {
    let inv = 1.0 - k;
    let ch = |s: f32, d: u8| {
        (s * k * 255.0 + f32::from(d) * inv)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    let a = (k * 255.0 + f32::from(d.alpha()) * inv)
        .round()
        .clamp(0.0, 255.0) as u8;
    PremultipliedColorU8::from_rgba(
        ch(c[0], d.red()).min(a),
        ch(c[1], d.green()).min(a),
        ch(c[2], d.blue()).min(a),
        a,
    )
    .unwrap_or(d)
}

/// `source-over` предумноженных.
pub(crate) fn over_pm(d: PremultipliedColorU8, s: PremultipliedColorU8) -> PremultipliedColorU8 {
    let inv = 255 - u32::from(s.alpha());
    let ch = |s: u8, d: u8| (u32::from(s) + (u32::from(d) * inv + 127) / 255).min(255) as u8;
    let a = ch(s.alpha(), d.alpha());
    PremultipliedColorU8::from_rgba(
        ch(s.red(), d.red()).min(a),
        ch(s.green(), d.green()).min(a),
        ch(s.blue(), d.blue()).min(a),
        a,
    )
    .unwrap_or(d)
}

/// Тень по маске альфы холста: сдвиг, размытие σ = радиус · `sigma`, цвет.
fn paint_shadow(canvas: &mut Pixmap, alpha: &[u8], sh: &Shadow, em: f32, s: f32, sigma: f32) {
    let (w, h) = (canvas.width() as i32, canvas.height() as i32);
    let (dx, dy) = (
        (sh.x.px(em) * s).round() as i32,
        (sh.y.px(em) * s).round() as i32,
    );
    let mut mask = vec![0u8; alpha.len()];
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = (x - dx, y - dy);
            if sx >= 0 && sy >= 0 && sx < w && sy < h {
                mask[(y * w + x) as usize] = alpha[(sy * w + sx) as usize];
            }
        }
    }
    super::shadow_blur::gaussian_blur_alpha(
        &mut mask,
        w as u32,
        h as u32,
        sh.blur.px(em) * sigma * s,
    );
    let c = sh.color;
    let px = canvas.pixels_mut();
    for (i, &m) in mask.iter().enumerate() {
        if m == 0 {
            continue;
        }
        let k = f32::from(m) / 255.0 * c[3];
        px[i] = over(px[i], [c[0], c[1], c[2]], k);
    }
}

fn apply_brightness(canvas: &mut Pixmap, b: f32) {
    for p in canvas.pixels_mut() {
        let a = p.alpha();
        let ch = |v: u8| (f32::from(v) * b).round().clamp(0.0, f32::from(a)) as u8;
        if let Some(c) =
            PremultipliedColorU8::from_rgba(ch(p.red()), ch(p.green()), ch(p.blue()), a)
        {
            *p = c;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STILL: Still = Still {
        no_anim: true,
        os_reduced: false,
    };

    /// CSS заготовки с сервера (0.4.9) рисуется без каталога, своим временем.
    #[test]
    fn server_css_is_parsed_on_the_fly() {
        let css = "color: #8af7e4; animation: nick-pulse 2s ease-in-out infinite";
        let look = look_of("", css).unwrap().unwrap();
        assert!(Arc::ptr_eq(&look, &look_of("", css).unwrap().unwrap()));
        assert!(moving(&look, Still::MOVING) && !moving(&look, STILL));
        // nick-pulse: в середине — .72, без анимаций — исходная 1.
        assert!((frame(&look, Some(1000.0), Still::MOVING).opacity - 0.72).abs() < 1e-6);
        assert_eq!(frame(&look, Some(1000.0), STILL).opacity, 1.0);
        assert!(look_of("", "font-style: italic").is_err());
        assert!(look_of("nope", "").is_err());
        assert!(look_of("", "").unwrap().is_none());
        assert!(look_of("gold", "").unwrap().is_some());
    }

    /// Ник и CSS с сервера: растр не растёт без предела ни от тени, ни от длины ника.
    #[test]
    fn server_nick_raster_is_bounded() {
        let look = look_of(
            "",
            "color: red; text-shadow: 0 0 1024px red; letter-spacing: 64em",
        )
        .unwrap()
        .unwrap();
        let long = "W".repeat(10_000);
        let text = NickText {
            text: &long,
            size: 16.0,
            weight: 400,
            letter_spacing: 0.0,
            color: [1.0; 4],
            scale: 1.0,
        };
        let art = render(&look, &text, None, STILL).unwrap();
        let pad = 2 * (css::MAX_REACH as u32 + 2);
        assert_eq!(art.pixels.width(), MAX_WIDTH_PX as u32 + pad);
        assert!(art.pixels.height() < pad + 64, "{}", art.pixels.height());
    }

    #[test]
    fn badge_breathes_and_stands_still_without_animations() {
        let at = |t: f64, still| badge(16.0, 1.0, Some(t), still).unwrap();
        // 1.05em при 16 px — 17 физ. px; поле под оба свечения не зависит от времени.
        let (from, to) = (at(0.0, Still::MOVING), at(3600.0, Still::MOVING));
        assert_eq!(from.pixels.width(), to.pixels.width());
        let pad = (from.pixels.width() - 17) / 2;
        assert_eq!(from.x, -(pad as f32));
        // Свечение сильнее в конце вдоха: край холста у значка ярче.
        let glow = |art: &BadgeArt| {
            art.pixels.pixels()
                [(pad as usize / 2) * art.pixels.width() as usize + art.pixels.width() as usize / 2]
                .alpha()
        };
        assert!(glow(&to) > glow(&from), "{} > {}", glow(&to), glow(&from));
        // Без анимаций кадр от времени не зависит.
        assert_eq!(
            at(0.0, STILL).pixels.data(),
            at(1800.0, STILL).pixels.data()
        );
        assert!(badge_moving(Still::MOVING) && !badge_moving(STILL));
    }

    #[test]
    fn support_months_counts_whole_utc_months() {
        let at = |iso: &str| {
            chrono::DateTime::parse_from_rfc3339(iso)
                .unwrap()
                .timestamp_millis() as f64
        };
        let since = (at("2026-01-31T10:00:00Z") / 1000.0) as i64;
        assert_eq!(support_months(since, at("2026-02-28T23:59:59Z")), 0);
        assert_eq!(support_months(since, at("2026-03-31T09:59:59Z")), 1);
        assert_eq!(support_months(since, at("2026-03-31T10:00:00Z")), 2);
        assert_eq!(support_months(since, at("2027-01-31T10:00:00Z")), 12);
        assert_eq!(support_months(since, at("2025-12-01T00:00:00Z")), 0);
        assert_eq!(support_months(0, at("2026-03-31T10:00:00Z")), 0);
        assert_eq!(support_months(since, f64::NAN), 0);
    }

    #[test]
    fn badge_title_like_0_4_9() {
        let now = 1_790_000_000_000.0;
        assert_eq!(badge_title(Language::Ru, None, now), "Подписчик HLSR");
        assert_eq!(badge_title(Language::Ru, Some(0), now), "Подписчик HLSR");
        let since = 1_780_000_000;
        let day = {
            use chrono::TimeZone;
            chrono::Local
                .timestamp_opt(since, 0)
                .unwrap()
                .format("%d.%m.%Y")
                .to_string()
        };
        assert_eq!(
            badge_title(Language::Ru, Some(since), now),
            format!("Поддерживает HLSR 3 мес. — с {day}")
        );
        assert_eq!(
            badge_title(Language::En, Some(since), since as f64 * 1000.0 + 1000.0),
            format!("Supporting HLSR for less than a month — since {day}")
        );
    }
}
