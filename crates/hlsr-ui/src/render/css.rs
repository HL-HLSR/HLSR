//! Подмножество CSS оформления ника и рамки аватара (задача 38, решение 13; с 0.4.9 — и
//! заготовки подсветки с сервера): цвета,
//! градиенты (`linear-gradient`, `conic-gradient` с `var(--frame-angle)`), тени, фильтры
//! (`drop-shadow`, `hue-rotate`, `brightness`) и `animation` с ключевыми кадрами
//! `launcher.css` (`nick-*`, `frame-*`). Всё прочее — «незнакомое»: такой стиль рисуется
//! обычным ником или без рамки, со строкой в журнале.

use crate::motion::{Animation, Direction, Fill, Iterations, Registry, StillDef, StillKind};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

/// Цвет без предумножения, каналы 0..1.
pub type Rgba = [f32; 4];

/// Длина: px или em.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Length {
    Px(f32),
    Em(f32),
}

impl Length {
    pub fn px(self, em: f32) -> f32 {
        match self {
            Length::Px(v) => v,
            Length::Em(v) => v * em,
        }
    }
}

/// Остановка градиента; `pos` — доля длины (нет — раскладывается поровну, как в CSS).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    pub color: Rgba,
    pub pos: Option<f32>,
}

/// Начальный угол `conic-gradient(from …)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum From {
    Deg(f32),
    /// `var(--frame-angle)` (+ сдвиг): его вращает `frame-spin`.
    FrameAngle,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Image {
    Linear { angle: f32, stops: Vec<Stop> },
    Conic { from: From, stops: Vec<Stop> },
}

/// Тень: `text-shadow`, `box-shadow`, `drop-shadow()`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    pub x: Length,
    pub y: Length,
    pub blur: Length,
    pub spread: Length,
    pub color: Rgba,
}

/// Фильтр (по порядку в `filter`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Filter {
    DropShadow(Shadow),
    HueRotate(f32),
    Brightness(f32),
}

/// `animation`: ключевые кадры `launcher.css` и время.
#[derive(Debug, Clone, PartialEq)]
pub struct Anim {
    pub name: String,
    /// мс.
    pub duration: f64,
    pub delay: f64,
    pub easing: [f64; 4],
    pub iterations: Iterations,
    pub direction: Direction,
}

impl Anim {
    /// Запись реестра движения (`motion::Animation`); без анимаций — как правило
    /// `animation: none !important` в `launcher.css` (`.no-anim .nick`, reduced-motion).
    pub fn to_animation(&self, selector: &str) -> Animation {
        Animation {
            source: "server css".to_owned(),
            selector: selector.to_owned(),
            keyframes: self.name.clone(),
            duration: self.duration,
            easing: self.easing,
            delay: self.delay,
            stagger: None,
            iterations: self.iterations,
            direction: self.direction,
            fill: Fill::None,
            underlying: None,
            still: StillDef {
                prefs: StillKind::None,
                os: StillKind::None,
            },
        }
    }

    /// Реестр движения с одной этой анимацией под ключом `key` (ключевые кадры — из общего
    /// реестра): CSS ника и рамки приходит с сервера (заготовки 0.4.9), общий реестр его не знает.
    pub fn registry(&self, key: &str) -> Registry {
        let base = crate::motion::registry();
        Registry {
            tokens: base.tokens.clone(),
            keyframes: base
                .keyframes
                .iter()
                .filter(|(name, _)| **name == self.name)
                .map(|(name, frames)| (name.clone(), frames.clone()))
                .collect(),
            animations: [(key.to_owned(), self.to_animation(key))].into(),
            transitions: Default::default(),
            ink: base.ink,
            theme: base.theme,
        }
    }
}

/// Разобранный CSS по тексту: до [`Parsed::LIMIT`] разных строк. Экран спрашивает
/// оформление на каждом кадре, а CSS заготовок приходит с сервера — разбирается один раз.
pub struct Parsed<T>(Mutex<BTreeMap<String, Option<Arc<T>>>>);

impl<T> Parsed<T> {
    /// Больше, чем ников и рамок влезает в окно: кадр экрана не разбирает CSS заново.
    const LIMIT: usize = 256;

    pub const fn new() -> Parsed<T> {
        Parsed(Mutex::new(BTreeMap::new()))
    }

    /// Оформление `key`; `None` — CSS не разбирается (запоминается тоже).
    pub fn get(&self, key: &str, parse: impl FnOnce(&str) -> Option<T>) -> Option<Arc<T>> {
        let mut cache = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(hit) = cache.get(key) {
            return hit.clone();
        }
        let parsed = parse(key).map(Arc::new);
        if cache.len() >= Self::LIMIT {
            cache.clear();
        }
        cache.insert(key.to_owned(), parsed.clone());
        parsed
    }
}

impl<T> Default for Parsed<T> {
    fn default() -> Parsed<T> {
        Parsed::new()
    }
}

/// Ключевые кадры, которые CSS ника и рамки может взять из `launcher.css`.
pub const KEYFRAMES: [&str; 6] = [
    "nick-shift",
    "nick-pulse",
    "nick-glow",
    "frame-spin",
    "frame-hue",
    "frame-pulse",
];

/// Объявления `свойство: значение` (имена — в нижнем регистре).
pub fn declarations(css: &str) -> Vec<(String, String)> {
    css.split(';')
        .filter_map(|part| {
            let (name, value) = part.split_once(':')?;
            let name = name.trim().to_ascii_lowercase();
            let value = value
                .trim()
                .trim_end_matches("!important")
                .trim()
                .to_owned();
            (!name.is_empty() && !value.is_empty()).then_some((name, value))
        })
        .collect()
}

/// Разбить по `sep` вне скобок.
pub fn split_top(text: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(text[start..i].trim());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(text[start..].trim());
    out.into_iter().filter(|s| !s.is_empty()).collect()
}

/// Разбить по пробелам вне скобок.
pub fn tokens(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, None::<usize>);
    for (i, c) in text.char_indices() {
        match c {
            '(' => {
                depth += 1;
                start.get_or_insert(i);
            }
            ')' => depth -= 1,
            c if c.is_whitespace() && depth == 0 => {
                if let Some(s) = start.take() {
                    out.push(&text[s..i]);
                }
            }
            _ => {
                start.get_or_insert(i);
            }
        }
    }
    if let Some(s) = start {
        out.push(&text[s..]);
    }
    out
}

fn named(name: &str) -> Option<Rgba> {
    Some(match name {
        "transparent" => [0.0, 0.0, 0.0, 0.0],
        "black" => [0.0, 0.0, 0.0, 1.0],
        "white" => [1.0, 1.0, 1.0, 1.0],
        "red" => [1.0, 0.0, 0.0, 1.0],
        "gold" => [1.0, 215.0 / 255.0, 0.0, 1.0],
        _ => return None,
    })
}

/// Цвет CSS: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`/`rgba()`, несколько имён.
pub fn color(text: &str) -> Option<Rgba> {
    let text = text.trim().to_ascii_lowercase();
    if let Some(hex) = text.strip_prefix('#') {
        let digit = |i: usize| u8::from_str_radix(hex.get(i..=i)?, 16).ok();
        let pair = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        let v = |n: u8| f32::from(n) / 255.0;
        return match hex.len() {
            3 | 4 => {
                let c = |i: usize| digit(i).map(|d| v(d * 17));
                Some([
                    c(0)?,
                    c(1)?,
                    c(2)?,
                    if hex.len() == 4 { c(3)? } else { 1.0 },
                ])
            }
            6 | 8 => {
                let c = |i: usize| pair(i).map(v);
                Some([
                    c(0)?,
                    c(2)?,
                    c(4)?,
                    if hex.len() == 8 { c(6)? } else { 1.0 },
                ])
            }
            _ => None,
        };
    }
    if let Some(args) = text
        .strip_prefix("rgba(")
        .or_else(|| text.strip_prefix("rgb("))
        .and_then(|t| t.strip_suffix(')'))
    {
        let parts: Vec<&str> = args
            .split([',', ' ', '/'])
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() < 3 {
            return None;
        }
        let channel = |p: &str| -> Option<f32> {
            match p.strip_suffix('%') {
                Some(pct) => pct.parse::<f32>().ok().map(|v| v / 100.0),
                None => p.parse::<f32>().ok().map(|v| v / 255.0),
            }
            .map(|v| v.clamp(0.0, 1.0))
        };
        let alpha = match parts.get(3) {
            Some(p) => match p.strip_suffix('%') {
                Some(pct) => pct.parse::<f32>().ok()? / 100.0,
                None => p.parse::<f32>().ok()?,
            },
            None => 1.0,
        };
        return Some([
            channel(parts[0])?,
            channel(parts[1])?,
            channel(parts[2])?,
            alpha.clamp(0.0, 1.0),
        ]);
    }
    named(&text)
}

/// Поле под тени и свечение вокруг ника и рамки, px CSS: дальше тень обрезается (у заготовок
/// оно до ~40 px). Холст растёт квадратом поля и рисуется заново на каждом кадре анимации.
pub const MAX_REACH: f32 = 128.0;

/// Длина `0`, `6px`, `.35em`. CSS приходит с сервера: `inf`, `NaN` и длины больше
/// 1024 px / 64 em — незнакомое оформление.
pub fn length(text: &str) -> Option<Length> {
    let text = text.trim();
    if text == "0" {
        return Some(Length::Px(0.0));
    }
    // NaN не проходит `<=`.
    let number = |v: &str, max: f32| v.parse::<f32>().ok().filter(|v| v.abs() <= max);
    if let Some(v) = text.strip_suffix("px") {
        return number(v, 1024.0).map(Length::Px);
    }
    if let Some(v) = text.strip_suffix("em") {
        return number(v, 64.0).map(Length::Em);
    }
    None
}

/// Угол в градусах: `90deg`, `0.25turn`, `1rad`.
pub fn angle(text: &str) -> Option<f32> {
    let text = text.trim();
    if text == "0" {
        return Some(0.0);
    }
    if let Some(v) = text.strip_suffix("deg") {
        return v.parse().ok();
    }
    if let Some(v) = text.strip_suffix("turn") {
        return v.parse::<f32>().ok().map(|t| t * 360.0);
    }
    if let Some(v) = text.strip_suffix("rad") {
        return v.parse::<f32>().ok().map(f32::to_degrees);
    }
    None
}

fn stops(parts: &[&str]) -> Option<Vec<Stop>> {
    let mut out = Vec::new();
    for part in parts {
        let words = tokens(part);
        let (color_text, rest) = words.split_first()?;
        let color = color(color_text)?;
        match rest {
            [] => out.push(Stop { color, pos: None }),
            [pos] => out.push(Stop {
                color,
                pos: Some(pos.strip_suffix('%')?.parse::<f32>().ok()? / 100.0),
            }),
            _ => return None,
        }
    }
    (out.len() >= 2).then_some(out)
}

/// `linear-gradient(...)` или `conic-gradient(...)`.
pub fn image(text: &str) -> Option<Image> {
    let text = text.trim();
    if let Some(args) = text
        .strip_prefix("linear-gradient(")
        .and_then(|t| t.strip_suffix(')'))
    {
        let parts = split_top(args, ',');
        let (angle, rest) = match parts.first().copied() {
            Some(first) if angle(first).is_some() => (angle(first)?, &parts[1..]),
            Some(first) if first.starts_with("to ") => {
                let a = match first {
                    "to top" => 0.0,
                    "to right" => 90.0,
                    "to bottom" => 180.0,
                    "to left" => 270.0,
                    _ => return None,
                };
                (a, &parts[1..])
            }
            _ => (180.0, &parts[..]),
        };
        return Some(Image::Linear {
            angle,
            stops: stops(rest)?,
        });
    }
    if let Some(args) = text
        .strip_prefix("conic-gradient(")
        .and_then(|t| t.strip_suffix(')'))
    {
        let parts = split_top(args, ',');
        let (from, rest) = match parts.first().copied().and_then(|p| p.strip_prefix("from ")) {
            Some(value) => {
                let value = value.trim();
                let from = if value == "var(--frame-angle)" {
                    From::FrameAngle
                } else {
                    From::Deg(angle(value)?)
                };
                (from, &parts[1..])
            }
            None => (From::Deg(0.0), &parts[..]),
        };
        return Some(Image::Conic {
            from,
            stops: stops(rest)?,
        });
    }
    None
}

/// Список теней (`text-shadow`, `box-shadow`): `x y [blur [spread]] color`.
pub fn shadows(text: &str, spread_allowed: bool) -> Option<Vec<Shadow>> {
    if text.trim() == "none" {
        return Some(Vec::new());
    }
    split_top(text, ',')
        .into_iter()
        .map(|one| shadow(one, spread_allowed))
        .collect()
}

fn shadow(text: &str, spread_allowed: bool) -> Option<Shadow> {
    let mut lengths = Vec::new();
    let mut color_value = None;
    for word in tokens(text) {
        if let Some(l) = length(word) {
            lengths.push(l);
        } else {
            color_value = Some(color(word)?);
        }
    }
    let zero = Length::Px(0.0);
    let max = if spread_allowed { 4 } else { 3 };
    if lengths.len() < 2 || lengths.len() > max {
        return None;
    }
    Some(Shadow {
        x: lengths[0],
        y: lengths[1],
        blur: lengths.get(2).copied().unwrap_or(zero),
        spread: lengths.get(3).copied().unwrap_or(zero),
        // Без цвета — currentColor; у ника и рамки — не поддерживается.
        color: color_value?,
    })
}

/// `filter`: `drop-shadow()`, `hue-rotate()`, `brightness()`.
pub fn filters(text: &str) -> Option<Vec<Filter>> {
    if text.trim() == "none" {
        return Some(Vec::new());
    }
    tokens(text)
        .into_iter()
        .map(|f| {
            let (name, args) = f.split_once('(')?;
            let args = args.strip_suffix(')')?;
            match name {
                "drop-shadow" => shadow(args, false).map(Filter::DropShadow),
                "hue-rotate" => angle(args).map(Filter::HueRotate),
                "brightness" => {
                    let v = match args.strip_suffix('%') {
                        Some(p) => p.parse::<f32>().ok()? / 100.0,
                        None => args.parse::<f32>().ok()?,
                    };
                    Some(Filter::Brightness(v))
                }
                _ => None,
            }
        })
        .collect()
}

fn easing(word: &str) -> Option<[f64; 4]> {
    Some(match word {
        "linear" => [0.0, 0.0, 1.0, 1.0],
        "ease" => [0.25, 0.1, 0.25, 1.0],
        "ease-in" => [0.42, 0.0, 1.0, 1.0],
        "ease-out" => [0.0, 0.0, 0.58, 1.0],
        "ease-in-out" => [0.42, 0.0, 0.58, 1.0],
        _ => {
            let args = word.strip_prefix("cubic-bezier(")?.strip_suffix(')')?;
            let v: Vec<f64> = args
                .split(',')
                .map(|p| p.trim().parse::<f64>())
                .collect::<Result<_, _>>()
                .ok()?;
            <[f64; 4]>::try_from(v).ok()?
        }
    })
}

fn time(word: &str) -> Option<f64> {
    if let Some(v) = word.strip_suffix("ms") {
        return v.parse().ok();
    }
    word.strip_suffix('s')?
        .parse::<f64>()
        .ok()
        .map(|s| s * 1000.0)
}

/// `animation` (одна, без запятых): имя — из [`KEYFRAMES`].
pub fn animation(text: &str) -> Option<Anim> {
    let mut anim = Anim {
        name: String::new(),
        duration: 0.0,
        delay: 0.0,
        easing: [0.25, 0.1, 0.25, 1.0],
        iterations: Iterations::Count(1.0),
        direction: Direction::Normal,
    };
    let mut durations = 0;
    for word in tokens(text) {
        if let Some(t) = time(word) {
            if durations == 0 {
                anim.duration = t;
            } else {
                anim.delay = t;
            }
            durations += 1;
        } else if let Some(e) = easing(word) {
            anim.easing = e;
        } else if word == "infinite" {
            anim.iterations = Iterations::Infinite;
        } else if let Ok(n) = word.parse::<f64>() {
            anim.iterations = Iterations::Count(n);
        } else if word == "alternate" {
            anim.direction = Direction::Alternate;
        } else if word == "normal"
            || word == "none"
            || word == "both"
            || word == "forwards"
            || word == "backwards"
            || word == "running"
        {
            // Заполнение и состояние на бесконечные анимации не влияют.
        } else if KEYFRAMES.contains(&word) {
            anim.name = word.to_owned();
        } else {
            // reverse, alternate-reverse, steps(), чужие ключевые кадры.
            return None;
        }
    }
    (!anim.name.is_empty()).then_some(anim)
}

/// Распределить остановки без позиций (CSS: первая — 0, последняя — 1, промежуточные —
/// поровну между соседями с позицией).
pub fn resolve_stops(stops: &[Stop]) -> Vec<(f32, Rgba)> {
    let n = stops.len();
    let mut pos: Vec<Option<f32>> = stops.iter().map(|s| s.pos).collect();
    if pos[0].is_none() {
        pos[0] = Some(0.0);
    }
    if pos[n - 1].is_none() {
        pos[n - 1] = Some(1.0);
    }
    // Не меньше предыдущей.
    let mut last = 0.0f32;
    for p in pos.iter_mut().flatten() {
        if *p < last {
            *p = last;
        }
        last = *p;
    }
    let mut i = 0;
    while i < n {
        if pos[i].is_some() {
            i += 1;
            continue;
        }
        let start = i - 1;
        let mut end = i;
        while pos[end].is_none() {
            end += 1;
        }
        let (a, b) = (pos[start].unwrap_or(0.0), pos[end].unwrap_or(1.0));
        for (k, slot) in pos.iter_mut().enumerate().take(end).skip(start + 1) {
            *slot = Some(a + (b - a) * (k - start) as f32 / (end - start) as f32);
        }
        i = end;
    }
    stops
        .iter()
        .zip(pos)
        .map(|(s, p)| (p.unwrap_or(0.0), s.color))
        .collect()
}

/// Цвет градиента в точке `t` (интерполяция в предумноженном sRGB, как у Chromium).
pub fn gradient_at(stops: &[(f32, Rgba)], t: f32) -> Rgba {
    let premul = |c: Rgba| [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]];
    let first = stops.first().map_or([0.0; 4], |s| s.1);
    if t <= stops.first().map_or(0.0, |s| s.0) {
        return first;
    }
    for pair in stops.windows(2) {
        let ((p0, c0), (p1, c1)) = (pair[0], pair[1]);
        if t <= p1 {
            let k = if p1 > p0 { (t - p0) / (p1 - p0) } else { 1.0 };
            let (a, b) = (premul(c0), premul(c1));
            let m: Vec<f32> = (0..4).map(|i| a[i] + (b[i] - a[i]) * k).collect();
            let alpha = m[3];
            return if alpha > 0.0 {
                [m[0] / alpha, m[1] / alpha, m[2] / alpha, alpha]
            } else {
                [0.0; 4]
            };
        }
    }
    stops.last().map_or([0.0; 4], |s| s.1)
}

/// Матрица `hue-rotate(deg)` (Filter Effects, sRGB без предумножения).
pub fn hue_rotate(c: Rgba, deg: f32) -> Rgba {
    let (sin, cos) = deg.to_radians().sin_cos();
    let m = [
        [
            0.213 + cos * 0.787 - sin * 0.213,
            0.715 - cos * 0.715 - sin * 0.715,
            0.072 - cos * 0.072 + sin * 0.928,
        ],
        [
            0.213 - cos * 0.213 + sin * 0.143,
            0.715 + cos * 0.285 + sin * 0.140,
            0.072 - cos * 0.072 - sin * 0.283,
        ],
        [
            0.213 - cos * 0.213 - sin * 0.787,
            0.715 - cos * 0.715 + sin * 0.715,
            0.072 + cos * 0.928 + sin * 0.072,
        ],
    ];
    let ch = |r: [f32; 3]| (r[0] * c[0] + r[1] * c[1] + r[2] * c[2]).clamp(0.0, 1.0);
    [ch(m[0]), ch(m[1]), ch(m[2]), c[3]]
}
