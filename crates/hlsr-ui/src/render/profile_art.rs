//! Ник с оформлением ([`super::nick`]) и рамка аватара ([`super::frame`]) картинками в
//! физических px. Неподвижные картинки кэшируются. Кадры бесконечной анимации идут по сетке
//! [`STEP_MS`] внутри её цикла: тот же шаг следующего цикла — та же картинка из кэша, поэтому
//! растр и размытие на ЦП — только в первом цикле (время — аргумент чистой функции).
//! Незнакомое оформление — обычный ник или аватар без рамки и одна строка в журнале на
//! стиль (решение 13).

use super::css::Rgba;
use super::{frame, nick, raster};
use crate::motion::Still;
use crate::{Rgba as Color, Rgba8Image};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Картинка ника или рамки и где она лежит (логические px).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProfileArt {
    /// Оформление нарисовано; `false` — обычный ник или аватар без рамки.
    pub styled: bool,
    /// Предумноженная картинка.
    pub image: Arc<Rgba8Image>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Ширина ника в строке.
    pub advance: f32,
    pub content_height: f32,
    pub opacity: f32,
}

/// Неподвижных картинок в кэше; при переполнении кэш очищается.
const CACHE_LIMIT: usize = 64;

/// Разных незнакомых стилей в журнале.
const LOGGED_LIMIT: usize = 256;

/// Шаг кадров анимации по умолчанию, мс (60 Гц для вызовов без известной частоты монитора).
pub const STEP_MS: f64 = 1000.0 / 60.0;

/// Максимальное число шагов в одном цикле для кэширования всех кадров в ОЗУ: при высокой
/// герцовке (240 Гц × 6 с = 1440 кадров) дешёвый пересчёт из `NickBase`/`FrameBase` (~30 мкс)
/// лучше, чем десятки мегабайт кэша.
const MAX_CACHED_STEPS: f64 = 400.0;

/// Шаг сетки анимации по частоте монитора (`None` — 0: точное время кадра без квантования).
pub fn step_ms_for_hz(refresh_hz: Option<f32>) -> f64 {
    match refresh_hz {
        Some(hz) if hz.is_finite() && hz > 1.0 => 1000.0 / f64::from(hz),
        _ => 0.0,
    }
}

/// Разложенный значок подписчика: сам значок и два слоя свечения (`t = 0` и `t = 1`).
#[derive(Debug, Clone)]
pub struct BadgePartsArt {
    pub icon: Arc<Rgba8Image>,
    pub glow_min: Arc<Rgba8Image>,
    pub glow_max: Arc<Rgba8Image>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Байт картинок кадров анимации в кэше (цикл ника 15 px — около 4 МБ, рамки 40 px — около
/// 3 МБ); что делать при переполнении — см. [`keep`].
const MOVING_LIMIT_BYTES: usize = 32 << 20;

/// Анимацию, кадр которой не брали дольше этого, при переполнении кэша вытесняют: её нет на
/// экране.
const MOVING_STALE: Duration = Duration::from_secs(1);

/// Кадры одной анимации по шагу цикла.
struct Frames {
    steps: HashMap<u64, ProfileArt>,
    bytes: usize,
    /// Когда кадр анимации брали в последний раз.
    used: Instant,
}

thread_local! {
    static STILL: RefCell<HashMap<String, ProfileArt>> = RefCell::new(HashMap::new());
    static MOVING: RefCell<HashMap<String, Frames>> = RefCell::new(HashMap::new());
    static LAST_FRAME: RefCell<HashMap<String, (u64, ProfileArt)>> = RefCell::new(HashMap::new());
    static BADGE_PARTS: RefCell<HashMap<(u32, u32), BadgePartsArt>> = RefCell::new(HashMap::new());
    static LOGGED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

fn rgba(c: Color) -> Rgba {
    [
        f32::from(c.r) / 255.0,
        f32::from(c.g) / 255.0,
        f32::from(c.b) / 255.0,
        f32::from(c.a) / 255.0,
    ]
}

/// Незнакомое оформление — строка в журнале, одна на стиль; после [`LOGGED_LIMIT`] стилей
/// журнал о них молчит (причина — весь CSS с сервера, набор не растёт без предела).
fn log_unknown(what: &str, reason: &str) {
    let first = LOGGED.with(|seen| {
        let mut seen = seen.borrow_mut();
        seen.len() < LOGGED_LIMIT && seen.insert(reason.to_owned())
    });
    if first {
        tracing::warn!("[profile] {what}: {reason} — показан без оформления");
    }
}

fn cached(key: String, make: impl FnOnce() -> ProfileArt) -> ProfileArt {
    if let Some(hit) = STILL.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    let art = make();
    STILL.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= CACHE_LIMIT {
            c.clear();
        }
        c.insert(key, art.clone());
    });
    art
}

/// Время кадра на сетке `step_ms` внутри цикла `(начало, длина)` и номер шага. `None` —
/// цикла нет, он ещё не начался или сетка не задана (`step_ms <= 0`): кадр рисуется в свой момент.
fn phase_at(elapsed_ms: f64, cycle: Option<(f64, f64)>, step_ms: f64) -> Option<(f64, u64)> {
    if step_ms <= 0.0 {
        return None;
    }
    let (start, length) = cycle?;
    if elapsed_ms < start || length <= 0.0 {
        return None;
    }
    let step = ((elapsed_ms - start) % length / step_ms).floor();
    Some((start + step * step_ms, step as u64))
}

#[cfg(test)]
fn phase(elapsed_ms: f64, cycle: Option<(f64, f64)>) -> Option<(f64, u64)> {
    phase_at(elapsed_ms, cycle, STEP_MS)
}

/// Кадр анимации `draw(время)`: при небольшом числе шагов в цикле — кэш по шагу цикла, при
/// высокой герцовке (240 Гц) — кэш последнего шага и быстрый пересчёт из `NickBase`/`FrameBase`.
fn moving(
    key: impl FnOnce() -> String,
    elapsed_ms: f64,
    cycle: Option<(f64, f64)>,
    step_ms: f64,
    draw: impl FnOnce(f64) -> ProfileArt,
) -> ProfileArt {
    let Some((at, step)) = phase_at(elapsed_ms, cycle, step_ms) else {
        return draw(elapsed_ms);
    };
    let key = key();
    let cache_whole_cycle = cycle.is_some_and(|(_, len)| len / step_ms <= MAX_CACHED_STEPS);
    if !cache_whole_cycle {
        let hit = LAST_FRAME.with_borrow(|all| {
            all.get(&key)
                .filter(|(cached_step, _)| *cached_step == step)
                .map(|(_, art)| art.clone())
        });
        if let Some(hit) = hit {
            return hit;
        }
        let art = draw(at);
        LAST_FRAME.with_borrow_mut(|all| {
            if all.len() >= CACHE_LIMIT {
                all.clear();
            }
            all.insert(key, (step, art.clone()));
        });
        return art;
    }
    let now = Instant::now();
    let hit = MOVING.with_borrow_mut(|all| {
        let frames = all.get_mut(&key)?;
        frames.used = now;
        frames.steps.get(&step).cloned()
    });
    if let Some(hit) = hit {
        return hit;
    }
    let art = draw(at);
    MOVING.with_borrow_mut(|all| keep(all, key, step, &art, now));
    art
}

/// Кадр `step` анимации `key` в кэш. При переполнении уходят только анимации не на экране
/// ([`MOVING_STALE`]); если места всё равно нет, кадр не кэшируется. Видимые не вытесняются:
/// каждая берёт кадры по кругу каждый кадр окна, и любое вытеснение (очистка, LRU) срывает кэш
/// всем сразу, а так закэшированные анимации остаются с кэшем.
fn keep(all: &mut HashMap<String, Frames>, key: String, step: u64, art: &ProfileArt, now: Instant) {
    let bytes = art.image.byte_len();
    let fits = |all: &HashMap<String, Frames>| {
        all.values().map(|frames| frames.bytes).sum::<usize>() + bytes <= MOVING_LIMIT_BYTES
    };
    if !fits(all) {
        all.retain(|_, frames| now.saturating_duration_since(frames.used) < MOVING_STALE);
        if !fits(all) {
            return;
        }
    }
    let frames = all.entry(key).or_insert_with(|| Frames {
        steps: HashMap::new(),
        bytes: 0,
        used: now,
    });
    frames.used = now;
    frames.bytes += bytes;
    if let Some(old) = frames.steps.insert(step, art.clone()) {
        frames.bytes -= old.image.byte_len();
    }
}

fn plain() -> ProfileArt {
    ProfileArt {
        styled: false,
        opacity: 1.0,
        ..Default::default()
    }
}

/// Оформление ника `{style, css}`: `None` — обычный ник (нет его или оно незнакомо).
fn nick_look(style: &str, css: &str) -> Option<Arc<nick::Look>> {
    match nick::look_of(style, css) {
        Ok(look) => look,
        Err(reason) => {
            log_unknown("ник", &reason);
            None
        }
    }
}

fn frame_look(css: &str) -> Option<Arc<frame::FrameLook>> {
    match frame::look_of(css) {
        Ok(look) => look,
        Err(reason) => {
            log_unknown("рамка аватара", &reason);
            None
        }
    }
}

/// Ник `text` с оформлением в момент `elapsed_ms` (шаг сетки по умолчанию [`STEP_MS`]).
#[allow(clippy::too_many_arguments)]
pub fn nick_art(
    style: &str,
    css: &str,
    text: &str,
    size: f32,
    weight: u16,
    letter_spacing: f32,
    color: Color,
    scale: f32,
    elapsed_ms: f64,
    still: Still,
) -> ProfileArt {
    nick_art_paced(
        style,
        css,
        text,
        size,
        weight,
        letter_spacing,
        color,
        scale,
        elapsed_ms,
        still,
        STEP_MS,
    )
}

/// Ник `text` с оформлением в момент `elapsed_ms` с шагом сетки `step_ms` (мс кадра монитора).
#[allow(clippy::too_many_arguments)]
pub fn nick_art_paced(
    style: &str,
    css: &str,
    text: &str,
    size: f32,
    weight: u16,
    letter_spacing: f32,
    color: Color,
    scale: f32,
    elapsed_ms: f64,
    still: Still,
    step_ms: f64,
) -> ProfileArt {
    let Some(look) = nick_look(style, css) else {
        return plain();
    };
    let draw = |at: f64| {
        let text = nick::NickText {
            text,
            size,
            weight,
            letter_spacing,
            color: rgba(color),
            scale,
        };
        match nick::render(&look, &text, Some(at.max(0.0)), still) {
            Some(art) => ProfileArt {
                styled: true,
                image: Arc::new(raster::to_image_premultiplied(&art.pixels)),
                x: art.x,
                y: art.y,
                width: art.width,
                height: art.height,
                advance: art.advance,
                content_height: art.line_height,
                opacity: art.opacity,
            },
            None => {
                log_unknown("ник", &format!("оформление «{style}{css}» не рисуется"));
                plain()
            }
        }
    };
    let base_key = || {
        format!(
            "n|{style}|{css}|{text}|{size}|{weight}|{letter_spacing}|{:?}|{scale}|{still:?}",
            rgba(color)
        )
    };
    if nick::opacity_only(&look, still) {
        let mut art = cached(base_key(), || draw(0.0));
        art.opacity = nick::opacity_at(&look, elapsed_ms.max(0.0), still);
        return art;
    }
    if nick::moving(&look, still) {
        let paced_key = || format!("{}|{}", base_key(), (step_ms * 100.0).round() as u32);
        return moving(
            paced_key,
            elapsed_ms,
            nick::cycle(&look, still),
            step_ms,
            draw,
        );
    }
    cached(base_key(), || draw(elapsed_ms))
}

/// Рамка аватара `size` в момент `elapsed_ms`; `radius` — `None` для круга.
#[allow(clippy::too_many_arguments)]
pub fn frame_art(
    css: &str,
    size: f32,
    width: f32,
    radius: Option<f32>,
    hole: Color,
    scale: f32,
    elapsed_ms: f64,
    still: Still,
) -> ProfileArt {
    frame_art_paced(
        css, size, width, radius, hole, scale, elapsed_ms, still, STEP_MS,
    )
}

/// Рамка аватара `size` в момент `elapsed_ms` с шагом сетки `step_ms` (мс кадра монитора).
#[allow(clippy::too_many_arguments)]
pub fn frame_art_paced(
    css: &str,
    size: f32,
    width: f32,
    radius: Option<f32>,
    hole: Color,
    scale: f32,
    elapsed_ms: f64,
    still: Still,
    step_ms: f64,
) -> ProfileArt {
    let Some(look) = frame_look(css) else {
        return plain();
    };
    if size <= 0.0 {
        return plain();
    }
    let draw = |at: f64| match frame::render(
        &look,
        size,
        width,
        radius,
        rgba(hole),
        scale,
        Some(at.max(0.0)),
        still,
    ) {
        Some(art) => ProfileArt {
            styled: true,
            image: Arc::new(raster::to_image_premultiplied(&art.pixels)),
            x: art.x,
            y: art.y,
            width: art.width,
            height: art.height,
            advance: 0.0,
            content_height: 0.0,
            opacity: art.opacity,
        },
        None => plain(),
    };
    let base_key = || {
        format!(
            "f|{css}|{size}|{width}|{radius:?}|{:?}|{scale}|{still:?}",
            rgba(hole)
        )
    };
    if frame::opacity_only(&look, still) {
        let mut art = cached(base_key(), || draw(0.0));
        art.opacity = frame::opacity_at(&look, elapsed_ms.max(0.0), still);
        return art;
    }
    if frame::moving(&look, still) {
        let paced_key = || format!("{}|{}", base_key(), (step_ms * 100.0).round() as u32);
        return moving(
            paced_key,
            elapsed_ms,
            frame::cycle(&look, still),
            step_ms,
            draw,
        );
    }
    cached(base_key(), || draw(elapsed_ms))
}

/// Разложенный значок подписчика (иконка и крайние свечения `t = 0`/`1`), кэшируемый по размеру.
pub fn badge_parts_art(size: f32, scale: f32) -> Option<BadgePartsArt> {
    let key = ((size * 100.0).round() as u32, (scale * 1000.0).round() as u32);
    if let Some(hit) = BADGE_PARTS.with_borrow(|all| all.get(&key).cloned()) {
        return Some(hit);
    }
    let parts = nick::badge_parts(size, scale)?;
    let art = BadgePartsArt {
        icon: Arc::new(raster::to_image_premultiplied(&parts.icon)),
        glow_min: Arc::new(raster::to_image_premultiplied(&parts.glow_min)),
        glow_max: Arc::new(raster::to_image_premultiplied(&parts.glow_max)),
        x: parts.x,
        y: parts.y,
        width: parts.width,
        height: parts.height,
    };
    BADGE_PARTS.with_borrow_mut(|all| {
        if all.len() >= CACHE_LIMIT {
            all.clear();
        }
        all.insert(key, art.clone());
    });
    Some(art)
}

/// Значок подписчика у ника кеглем `size` (px CSS) в момент `elapsed_ms`; место — от левого
/// верхнего угла самого значка.
pub fn badge_art(size: f32, scale: f32, elapsed_ms: f64, still: Still) -> ProfileArt {
    let draw = |at: f64| match nick::badge(size, scale, Some(at.max(0.0)), still) {
        Some(art) => ProfileArt {
            styled: true,
            image: Arc::new(raster::to_image_premultiplied(&art.pixels)),
            x: art.x,
            y: art.y,
            width: art.width,
            height: art.height,
            advance: 0.0,
            content_height: 0.0,
            opacity: 1.0,
        },
        None => plain(),
    };
    let key = || format!("b|{size}|{scale}|{still:?}");
    if nick::badge_moving(still) {
        return moving(key, elapsed_ms, nick::badge_cycle(still), STEP_MS, draw);
    }
    cached(key(), || draw(elapsed_ms))
}

/// Движется ли ник с оформлением `{style, css}` (нужны кадры).
pub fn nick_moving(style: &str, css: &str, still: Still) -> bool {
    nick_look(style, css).is_some_and(|look| nick::moving(&look, still))
}

/// Движется ли рамка аватара `css` (нужны кадры).
pub fn frame_moving(css: &str, still: Still) -> bool {
    frame_look(css).is_some_and(|look| frame::moving(&look, still))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Кадр движущегося ника — по шагу 1/60 с внутри цикла: тот же шаг через цикл и внутри шага —
    /// та же картинка из кэша, другой шаг — другая; у рамки так же.
    #[test]
    fn moving_frames_come_from_the_cycle_cache() {
        let white = Color::new(255, 255, 255, 255);
        let nick = |t: f64| {
            nick_art(
                "developer",
                "",
                "prolert",
                15.0,
                500,
                0.0,
                white,
                1.0,
                t,
                Still::MOVING,
            )
        };
        // nick-shift 6 с, normal.
        let first = nick(1010.0);
        assert!(first.styled);
        assert!(Arc::ptr_eq(&first.image, &nick(1015.0).image));
        assert!(Arc::ptr_eq(&first.image, &nick(1010.0 + 6000.0).image));
        assert!(!Arc::ptr_eq(&first.image, &nick(1510.0).image));

        let css = "background: conic-gradient(from var(--frame-angle), #f0f, #0ff); \
                   animation: frame-spin 3s linear infinite";
        let ring = |t: f64| frame_art(css, 40.0, 3.0, None, white, 1.0, t, Still::MOVING);
        assert!(Arc::ptr_eq(&ring(200.0).image, &ring(200.0 + 3000.0).image));

        assert_eq!(phase(10.0, None), None);
        assert_eq!(phase(5.0, Some((10.0, 100.0))), None);
        assert_eq!(
            phase(6020.0, Some((0.0, 6000.0))).map(|(_, step)| step),
            Some(1)
        );
    }

    /// Полный кэш: видимая анимация остаётся, новая не кэшируется; ушедшая с экрана вытесняется.
    #[test]
    fn full_moving_cache_keeps_visible_frames() {
        let art = ProfileArt {
            image: Arc::new(Rgba8Image::new(1024, 1024)),
            ..Default::default()
        };
        let mut all = HashMap::new();
        let start = Instant::now();
        for step in 0..8 {
            keep(&mut all, "a".into(), step, &art, start);
        }
        keep(&mut all, "b".into(), 0, &art, start + MOVING_STALE / 2);
        assert_eq!(all["a"].steps.len(), 8);
        assert!(!all.contains_key("b"));

        keep(&mut all, "b".into(), 0, &art, start + MOVING_STALE * 2);
        assert!(!all.contains_key("a"));
        assert_eq!(all["b"].bytes, art.image.byte_len());
    }

    #[test]
    fn high_hz_pacing_and_opacity_only_paths() {
        let white = Color::new(255, 255, 255, 255);
        let step_240 = step_ms_for_hz(Some(240.0));
        assert!((step_240 - 1000.0 / 240.0).abs() < 1e-6);
        let a = nick_art_paced(
            "developer",
            "",
            "prolert",
            15.0,
            500,
            0.0,
            white,
            1.0,
            100.0,
            Still::MOVING,
            step_240,
        );
        let b = nick_art_paced(
            "developer",
            "",
            "prolert",
            15.0,
            500,
            0.0,
            white,
            1.0,
            105.0,
            Still::MOVING,
            step_240,
        );
        assert!(!Arc::ptr_eq(&a.image, &b.image), "240 Гц не квантуется в 60 FPS");

        // nick-pulse (ghost) не перерисовывает растр — меняется только opacity.
        let g0 = nick_art_paced(
            "ghost",
            "",
            "prolert",
            15.0,
            500,
            0.0,
            white,
            1.0,
            0.0,
            Still::MOVING,
            step_240,
        );
        let g1 = nick_art_paced(
            "ghost",
            "",
            "prolert",
            15.0,
            500,
            0.0,
            white,
            1.0,
            1600.0,
            Still::MOVING,
            step_240,
        );
        assert!(Arc::ptr_eq(&g0.image, &g1.image));
        assert!(g1.opacity < g0.opacity);

        let parts = badge_parts_art(15.0, 1.0).expect("badge parts");
        assert_eq!(parts.icon.width(), parts.glow_min.width());
        assert_eq!(parts.icon.width(), parts.glow_max.width());
    }
}
