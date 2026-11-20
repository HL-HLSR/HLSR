//! Рамка аватара (`AvatarFrame.vue`, `launcher.css:778-815` 0.4.7, `845-877` 0.4.9; задача
//! 38, решение 13): слой за аватаром шире него на `--frame-width` со всех сторон, в его
//! середине — подложка `--frame-hole` (`::after`). CSS рамки выдаёт админка, с 0.4.9 — и
//! заготовка подсветки с сервера, поэтому он разбирается на лету (пример выданной рамки —
//! `data/avatar-frames.json`): фон (цвет, `linear-gradient`, `conic-gradient(from
//! var(--frame-angle), …)`), `border`, внешние `box-shadow`, `filter` (`hue-rotate`,
//! `brightness`, `drop-shadow`), `opacity`, анимации `frame-spin`, `frame-hue`,
//! `frame-pulse`. `mask`, `border-image`, `outline` и прочее — незнакомое: аватар без
//! рамки и строка в журнале.

use super::css::{self, Anim, Filter, Image, Length, Rgba, Shadow};
use super::nick::{image_color, over_pm};
use crate::motion::{Registry, Still};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use tiny_skia::{Pixmap, PremultipliedColorU8};

#[derive(Debug, Clone, PartialEq)]
enum Background {
    Color(Rgba),
    Image(Image),
}

#[derive(Debug, Clone)]
pub struct FrameLook {
    background: Option<Background>,
    border: Option<(Length, Rgba)>,
    shadows: Vec<Shadow>,
    filters: Vec<Filter>,
    opacity: f32,
    animation: Option<Anim>,
    /// Реестр движения с анимацией рамки под ключом [`MOTION_KEY`] (если она есть).
    motion: Option<Arc<Registry>>,
}

const MOTION_KEY: &str = "frame";

/// Разобрать CSS рамки; незнакомое — `None`.
pub fn parse(css_text: &str) -> Option<FrameLook> {
    let mut look = FrameLook {
        background: None,
        border: None,
        shadows: Vec::new(),
        filters: Vec::new(),
        opacity: 1.0,
        animation: None,
        motion: None,
    };
    for (name, value) in css::declarations(css_text) {
        match name.as_str() {
            "background" | "background-image" => {
                look.background = Some(match css::image(&value) {
                    Some(image) => Background::Image(image),
                    None => Background::Color(css::color(&value)?),
                });
            }
            "background-color" => look.background = Some(Background::Color(css::color(&value)?)),
            "border" => {
                let mut width = None;
                let mut color = None;
                let mut solid = false;
                for word in css::tokens(&value) {
                    if word == "solid" {
                        solid = true;
                    } else if let Some(l) = css::length(word) {
                        width = Some(l);
                    } else {
                        color = Some(css::color(word)?);
                    }
                }
                if !solid {
                    return None;
                }
                look.border = Some((width?, color?));
            }
            "box-shadow" => {
                if value.contains("inset") {
                    return None;
                }
                look.shadows = css::shadows(&value, true)?;
            }
            "filter" => look.filters = css::filters(&value)?,
            "opacity" => look.opacity = value.parse::<f32>().ok()?.clamp(0.0, 1.0),
            "animation" => {
                let anim = css::animation(&value)?;
                if !anim.name.starts_with("frame-") {
                    return None;
                }
                look.animation = Some(anim);
            }
            _ => return None,
        }
    }
    look.motion = look
        .animation
        .as_ref()
        .map(|anim| Arc::new(anim.registry(MOTION_KEY)));
    Some(look)
}

/// CSS рамки так, как его применяет 0.4.9 (`sanitizeCss`: сервер и main).
pub fn normalize_css(text: &str) -> String {
    hlsr_online::shared::avatar_frames::sanitize_css(&serde_json::Value::String(text.to_owned()))
        .unwrap_or_default()
}

/// Рамка `{css}`: `Ok(None)` — рамки нет, `Err` — CSS незнакомый (текст для журнала).
pub fn look_of(css_text: &str) -> Result<Option<Arc<FrameLook>>, String> {
    let key = normalize_css(css_text);
    if key.is_empty() {
        return Ok(None);
    }
    static PARSED: css::Parsed<FrameLook> = css::Parsed::new();
    PARSED
        .get(&key, parse)
        .map(Some)
        .ok_or_else(|| format!("CSS рамки не поддерживается: {key}"))
}

fn frame_of(look: &FrameLook, elapsed: Option<f64>, still: Still) -> crate::motion::Frame {
    match &look.motion {
        Some(registry) => {
            registry.keyframes_frame(MOTION_KEY, Some(elapsed.unwrap_or(0.0)), 0, still, true)
        }
        None => crate::motion::Frame::REST,
    }
}

/// Рамка меняется со временем.
pub fn moving(look: &FrameLook, still: Still) -> bool {
    look.motion
        .as_ref()
        .is_some_and(|registry| registry.keyframes_moving(MOTION_KEY, still))
}

/// Цикл анимации рамки, мс (`Registry::keyframes_cycle`).
pub fn cycle(look: &FrameLook, still: Still) -> Option<(f64, f64)> {
    look.motion.as_ref()?.keyframes_cycle(MOTION_KEY, still)
}

/// Картинка слоя рамки в физических px; места — в логических px от левого верхнего угла
/// аватара (рамка выступает на `width` и на поле теней).
pub struct FrameArt {
    pub pixels: Pixmap,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Прозрачность слоя (`opacity` и `frame-pulse`).
    pub opacity: f32,
}

/// Рамка вокруг аватара `size` (px CSS) шириной `width` с подложкой `hole`. `radius` —
/// `--frame-radius` в px; `None` — `50%` (круг: у слоя и у подложки тоже 50 %).
#[allow(clippy::too_many_arguments)]
pub fn render(
    look: &FrameLook,
    size: f32,
    width: f32,
    radius: Option<f32>,
    hole: Rgba,
    scale: f32,
    elapsed: Option<f64>,
    still: Still,
) -> Option<FrameArt> {
    let f = frame_of(look, elapsed, still);
    let s = scale.max(0.01);
    let ring = size + width * 2.0;
    // Скругления слоя (calc(--frame-radius + --frame-width)) и подложки, физ. px.
    let outer_r = radius.map_or(ring * s / 2.0, |r| ((r + width) * s).min(ring * s / 2.0));
    let inner_r = radius.map_or(size * s / 2.0, |r| (r * s).min(size * s / 2.0));
    let em = 16.0;
    let mut reach = 0.0f32;
    for sh in &look.shadows {
        reach = reach.max(
            sh.blur.px(em) * 1.5 + sh.spread.px(em) + sh.x.px(em).abs().max(sh.y.px(em).abs()),
        );
    }
    for filter in &look.filters {
        if let Filter::DropShadow(sh) = filter {
            reach = reach.max(sh.blur.px(em) * 1.5 + sh.x.px(em).abs().max(sh.y.px(em).abs()));
        }
    }
    let pad = (reach.min(css::MAX_REACH) * s).ceil() as i32 + 1;
    let side_px = (ring * s).round() as i32;
    let total = side_px + pad * 2;
    let centre = total as f32 / 2.0;
    let half = ring * s / 2.0;
    let inner = size * s / 2.0;
    let base = base(look, total, half, inner, outer_r, inner_r, s)?;
    // Внешние тени уже на холсте.
    let mut canvas = base.shadows.clone();
    let disc = &base.disc;
    // Фон, обрезанный кругом.
    if let Some(background) = &look.background {
        let conic = match background {
            Background::Image(Image::Conic { stops, .. }) => css::resolve_stops(stops),
            _ => Vec::new(),
        };
        for (i, m) in disc.iter().enumerate() {
            if *m == 0 {
                continue;
            }
            let (x, y) = (
                (i as i32 % total) as f32 + 0.5,
                (i as i32 / total) as f32 + 0.5,
            );
            let (u, v) = ((x - centre) / s + ring / 2.0, (y - centre) / s + ring / 2.0);
            let c = match background {
                Background::Color(c) => *c,
                Background::Image(Image::Conic { from, .. }) => {
                    let start = match from {
                        css::From::Deg(d) => *d,
                        css::From::FrameAngle => f.angle as f32,
                    };
                    let a = (u - ring / 2.0).atan2(-(v - ring / 2.0)).to_degrees();
                    let t = (a - start).rem_euclid(360.0) / 360.0;
                    css::gradient_at(&conic, t)
                }
                Background::Image(image) => image_color(image, u, v, ring, ring),
            };
            let k = f32::from(*m) / 255.0 * c[3];
            canvas.pixels_mut()[i] = mix(canvas.pixels()[i], c, k);
        }
    }
    // Рамка border внутри круга.
    if let Some((_, color)) = look.border {
        for (i, m) in disc.iter().enumerate() {
            let k = (f32::from(*m) - f32::from(base.border[i])).max(0.0) / 255.0 * color[3];
            if k > 0.0 {
                canvas.pixels_mut()[i] = mix(canvas.pixels()[i], color, k);
            }
        }
    }
    // ::after — подложка в середине.
    for (i, m) in base.hole.iter().enumerate() {
        let k = f32::from(*m) / 255.0 * hole[3];
        if k > 0.0 {
            canvas.pixels_mut()[i] = mix(canvas.pixels()[i], hole, k);
        }
    }
    // Фильтры: объявленные; frame-hue подменяет filter анимацией.
    let filters: Vec<Filter> = if look
        .animation
        .as_ref()
        .is_some_and(|a| a.name == "frame-hue")
    {
        vec![Filter::HueRotate(f.hue as f32)]
    } else {
        look.filters.clone()
    };
    for filter in filters {
        match filter {
            Filter::HueRotate(deg) => {
                for p in canvas.pixels_mut() {
                    let a = p.alpha();
                    if a == 0 {
                        continue;
                    }
                    let c = p.demultiply();
                    let rgba = [
                        f32::from(c.red()) / 255.0,
                        f32::from(c.green()) / 255.0,
                        f32::from(c.blue()) / 255.0,
                        f32::from(a) / 255.0,
                    ];
                    let out = css::hue_rotate(rgba, deg);
                    let to = |v: f32| super::raster::premultiply((v * 255.0).round() as u8, a);
                    if let Some(c) =
                        PremultipliedColorU8::from_rgba(to(out[0]), to(out[1]), to(out[2]), a)
                    {
                        *p = c;
                    }
                }
            }
            Filter::Brightness(b) => {
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
            Filter::DropShadow(sh) => {
                let mut mask: Vec<u8> = canvas.pixels().iter().map(|p| p.alpha()).collect();
                let (dx, dy) = (
                    (sh.x.px(em) * s).round() as i32,
                    (sh.y.px(em) * s).round() as i32,
                );
                let mut shifted = vec![0u8; mask.len()];
                for y in 0..total {
                    for x in 0..total {
                        let (sx, sy) = (x - dx, y - dy);
                        if sx >= 0 && sy >= 0 && sx < total && sy < total {
                            shifted[(y * total + x) as usize] = mask[(sy * total + sx) as usize];
                        }
                    }
                }
                mask = shifted;
                super::shadow_blur::gaussian_blur_alpha(
                    &mut mask,
                    total as u32,
                    total as u32,
                    sh.blur.px(em) * 0.5 * s,
                );
                let mut under = Pixmap::new(total as u32, total as u32)?;
                for (i, m) in mask.iter().enumerate() {
                    let k = f32::from(*m) / 255.0 * sh.color[3];
                    if k > 0.0 {
                        under.pixels_mut()[i] = mix(under.pixels()[i], sh.color, k);
                    }
                }
                for (u, c) in under.pixels_mut().iter_mut().zip(canvas.pixels()) {
                    *u = over_pm(*u, *c);
                }
                canvas = under;
            }
        }
    }
    let offset = -(width + pad as f32 / s);
    Some(FrameArt {
        pixels: canvas,
        x: offset,
        y: offset,
        width: total as f32 / s,
        height: total as f32 / s,
        opacity: look.opacity * f.opacity as f32,
    })
}

/// Что у рамки не зависит от времени: внешние тени на холсте и маски слоя, края `border` и
/// подложки. Кадр `frame-spin/hue/pulse` меняет только заливку и фильтры, поэтому тени не
/// размываются заново на каждом кадре и у одинаковых рамок экрана одни на всех.
struct Base {
    shadows: Pixmap,
    disc: Vec<u8>,
    /// Внутренний край `border` (пусто без него).
    border: Vec<u8>,
    hole: Vec<u8>,
}

/// Основ в кэше; при переполнении он очищается.
const BASE_LIMIT: usize = 16;

thread_local! {
    static BASES: RefCell<HashMap<String, Rc<Base>>> = RefCell::new(HashMap::new());
}

#[allow(clippy::too_many_arguments)]
fn base(
    look: &FrameLook,
    total: i32,
    half: f32,
    inner: f32,
    outer_r: f32,
    inner_r: f32,
    s: f32,
) -> Option<Rc<Base>> {
    let em = 16.0;
    let key = format!(
        "{:?}|{:?}|{total}|{half}|{inner}|{outer_r}|{inner_r}|{s}",
        look.shadows,
        look.border.map(|(width, _)| width)
    );
    if let Some(hit) = BASES.with(|bases| bases.borrow().get(&key).cloned()) {
        return Some(hit);
    }
    let centre = total as f32 / 2.0;
    let len = (total * total) as usize;
    let mut shadows = Pixmap::new(total as u32, total as u32)?;
    let mut disc = vec![0u8; len];
    shape_mask(&mut disc, total, centre, centre, half, outer_r);
    for sh in look.shadows.iter().rev() {
        let mut mask = vec![0u8; len];
        let spread = sh.spread.px(em) * s;
        let (dx, dy) = (sh.x.px(em) * s, sh.y.px(em) * s);
        shape_mask(
            &mut mask,
            total,
            centre + dx,
            centre + dy,
            half + spread,
            (outer_r + spread).max(0.0),
        );
        super::shadow_blur::gaussian_blur_alpha(
            &mut mask,
            total as u32,
            total as u32,
            sh.blur.px(em) * 0.5 * s,
        );
        // Тень не рисуется под самим блоком (box-shadow снаружи рамки).
        for (i, m) in mask.iter().enumerate() {
            let k = f32::from(*m) / 255.0 * (1.0 - f32::from(disc[i]) / 255.0) * sh.color[3];
            if k > 0.0 {
                shadows.pixels_mut()[i] = mix(shadows.pixels()[i], sh.color, k);
            }
        }
    }
    let mut border = Vec::new();
    if let Some((bw, _)) = look.border {
        let bw = bw.px(em) * s;
        border = vec![0u8; len];
        shape_mask(
            &mut border,
            total,
            centre,
            centre,
            half - bw,
            (outer_r - bw).max(0.0),
        );
    }
    let mut hole = vec![0u8; len];
    shape_mask(&mut hole, total, centre, centre, inner, inner_r);
    let base = Rc::new(Base {
        shadows,
        disc,
        border,
        hole,
    });
    BASES.with(|bases| {
        let mut bases = bases.borrow_mut();
        if bases.len() >= BASE_LIMIT {
            bases.clear();
        }
        bases.insert(key, base.clone());
    });
    Some(base)
}

/// Покрытие квадрата с полустороной `half` и скруглением `r` (`r = half` — круг;
/// 4 × 4 подвыборки на пиксель).
fn shape_mask(mask: &mut [u8], side: i32, cx: f32, cy: f32, half: f32, r: f32) {
    if half <= 0.0 {
        return;
    }
    let r = r.clamp(0.0, half);
    let n = 4;
    let inside = |px: f32, py: f32| {
        let (ax, ay) = (px.abs(), py.abs());
        if ax > half || ay > half {
            return false;
        }
        // Угол: точка дальше центра скругления, чем r.
        let (qx, qy) = (ax - (half - r), ay - (half - r));
        !(qx > 0.0 && qy > 0.0 && qx * qx + qy * qy > r * r)
    };
    for y in 0..side {
        let fy = y as f32;
        if fy + 1.0 < cy - half || fy > cy + half {
            continue;
        }
        for x in 0..side {
            let fx = x as f32;
            if fx + 1.0 < cx - half || fx > cx + half {
                continue;
            }
            let mut hits = 0;
            for sy in 0..n {
                for sx in 0..n {
                    let px = fx + (sx as f32 + 0.5) / n as f32 - cx;
                    let py = fy + (sy as f32 + 0.5) / n as f32 - cy;
                    if inside(px, py) {
                        hits += 1;
                    }
                }
            }
            let v = (hits * 255 / (n * n)) as u8;
            let cell = &mut mask[(y * side + x) as usize];
            *cell = (*cell).max(v);
        }
    }
}

/// `source-over` неумноженного цвета с покрытием `k`.
fn mix(d: PremultipliedColorU8, c: Rgba, k: f32) -> PremultipliedColorU8 {
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

#[cfg(test)]
mod tests {
    use super::*;

    const HOLE: Rgba = [0.0, 0.0, 1.0, 1.0];

    fn alpha(art: &FrameArt, x: u32, y: u32) -> u8 {
        art.pixels.pixels()[(y * art.pixels.width() + x) as usize].alpha()
    }

    #[test]
    fn round_and_rounded_rings_differ_only_in_the_corners() {
        let look = parse("background: #8af7e4").unwrap();
        let round = render(&look, 40.0, 3.0, None, HOLE, 1.0, None, Still::MOVING).unwrap();
        // Аватар 40 px, рамка 3 px со всех сторон: слой 46 px (+1 px поля) от угла (−4, −4).
        assert_eq!((round.x, round.y), (-4.0, -4.0));
        assert_eq!(round.pixels.width(), 48);
        // Угол круга пуст, середина края — рамка.
        assert_eq!(alpha(&round, 2, 2), 0);
        assert_eq!(alpha(&round, 24, 2), 255);
        // --frame-radius 10px: слой скруглён на 13 px — угол в 3 px от края закрыт.
        let rounded = render(&look, 40.0, 3.0, Some(10.0), HOLE, 1.0, None, Still::MOVING).unwrap();
        assert_eq!(alpha(&rounded, 5, 5), 255);
        assert_eq!(alpha(&rounded, 1, 1), 0);
        // Середина — подложка (hole) под аватаром.
        let mid = rounded.pixels.pixels()[(24 * 48 + 24) as usize];
        assert_eq!((mid.red(), mid.green(), mid.blue()), (0, 0, 255));
    }

    #[test]
    fn unknown_frame_css_is_not_drawn() {
        assert!(parse("border-image: linear-gradient(red, blue) 1").is_none());
        assert!(parse("outline: 2px solid red").is_none());
        assert!(parse("box-shadow: inset 0 0 4px red").is_none());
        assert!(parse("animation: nick-shift 3s linear infinite").is_none());
        assert!(look_of("background: red; mask: none").is_err());
        assert!(look_of("").unwrap().is_none());
    }

    /// CSS с сервера: бесконечные и огромные длины не разбираются, поле под тень ограничено.
    #[test]
    fn server_lengths_are_bounded() {
        for bad in ["infpx", "NaNpx", "-infem", "1e30em", "2000px", "65em"] {
            assert_eq!(css::length(bad), None, "{bad}");
        }
        assert_eq!(css::length("-1024px"), Some(Length::Px(-1024.0)));
        let look = parse("background: red; box-shadow: 0 0 1024px 64em red").unwrap();
        let art = render(&look, 40.0, 3.0, None, HOLE, 1.0, None, Still::MOVING).unwrap();
        // Слой 46 px и поле не шире MAX_REACH (+1 px) с каждой стороны.
        assert_eq!(art.pixels.width(), 46 + 2 * (css::MAX_REACH as u32 + 1));
    }

    /// Заготовка с сервера (0.4.9) рисуется без каталога; выданные рамки разбираются.
    #[test]
    fn server_frames_are_parsed_on_the_fly() {
        let file: serde_json::Value =
            serde_json::from_str(include_str!("../../data/avatar-frames.json")).unwrap();
        for css in file["frames"].as_array().unwrap() {
            assert!(look_of(css.as_str().unwrap()).unwrap().is_some(), "{css}");
        }
        let preset = "background: linear-gradient(90deg, #5ec8ff, #ff7ad9); \
                      animation: frame-hue 4s linear infinite";
        let look = look_of(preset).unwrap().unwrap();
        // Второй запрос — тот же разбор, без повторного.
        assert!(Arc::ptr_eq(&look, &look_of(preset).unwrap().unwrap()));
        assert!(moving(&look, Still::MOVING));
        let still = Still {
            no_anim: true,
            os_reduced: false,
        };
        assert!(!moving(&look, still));
        // frame-hue: на половине круга — 180°.
        assert!((frame_of(&look, Some(2000.0), Still::MOVING).hue - 180.0).abs() < 1e-6);
    }
}
