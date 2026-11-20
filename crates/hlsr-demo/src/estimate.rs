//! Оценка времени задания «Демо» (`estimate.js`): запуск игры + кадры игры + кодирование итога.
//!
//! Скорости по умолчанию — замер 11.11 на стенде (Ghosting, 1280×720, 30 к/с):
//! `-ghrender`: ран 0:08 — 17–19 с (почти всё — запуск игры, ~12 с), ран 2:24 — 103 с, из них
//! игра рисует 4291 кадр за 96 с (~45 к/с), кодирование — несколько секунд; старый путь
//! `startdemos`: тот же ран 2:24 — две пачки 76 + 16 с (~63 к/с без запусков). HL1 (bxt-rs) не
//! замерен — осторожная оценка; прошлые задания её поправят ([`learn`]). Другое разрешение:
//! скорость игры — обратно числу пикселей в степени 0,6 (кадр TGA растёт с площадью, отрисовка
//! медленнее), кодирование — обратно числу пикселей.
//!
//! Таблица замеров — значение Store `demo-tool.json` (`speed`):
//! `{render: {"<профиль>@<высота>": {rate, frames}}, encode: {...}}`; читается как в JS, с
//! приведением чисел.

use hlsr_core::js::math::{max, min};
use hlsr_core::js::number::number_to_string;
use hlsr_core::js::value::to_number;
use hlsr_core::js::{math_round, truthy};
use hlsr_core::json_text::js_number;
use serde_json::{Map, Value, json};

const BASE_PIXELS: f64 = 1280.0 * 720.0;
const RENDER_EXPONENT: f64 = 0.6;
/// Предпросмотр дольше этого — окно с советом сразу отрендерить видео.
pub const PREVIEW_PROMPT_SECONDS: f64 = 120.0;
/// Свой замер учитывается от этого числа кадров.
pub const MIN_FRAMES: f64 = 300.0;
/// Вес прошлых замеров — не больше стольких кадров.
const CAP_FRAMES: f64 = 30000.0;
const RATE_MIN: f64 = 1.0;
const RATE_MAX: f64 = 5000.0;

/// Профиль рендера: запуск игры (с), кадров в секунду на 1280×720, доплата за сегмент (с).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Profile {
    pub load: f64,
    pub rate: f64,
    pub per_segment: f64,
}

/// `PROFILES[key]`.
pub fn profile(key: &str) -> Option<Profile> {
    let (load, rate, per_segment) = match key {
        "ghosting-ghrender" => (12.0, 45.0, 0.0),
        "ghosting-startdemos" => (12.0, 63.0, 0.0),
        "hl1" => (10.0, 60.0, 1.0),
        "other" => (10.0, 45.0, 0.0),
        _ => return None,
    };
    Some(Profile {
        load,
        rate,
        per_segment,
    })
}

/// Кодирование итога, выходных кадров в секунду на 1280×720 (`ENCODE`): предпросмотр
/// (veryfast/p2 — замер ~600), «Лучшее качество» (x264 slow CRF 18 / NVENC p5), Discord (x264 в
/// два прохода).
fn encode_base(kind: &str) -> f64 {
    match kind {
        "preview" => 600.0,
        "quality-gpu" | "discord-gpu" => 500.0,
        "discord-cpu" => 100.0,
        _ => 90.0,
    }
}

// Math.max/Math.min JS: NaN не пропускается, как у f64::max.
fn pixels(width: f64, height: f64) -> f64 {
    max(1.0, width * height)
}

fn clamp_rate(rate: f64) -> f64 {
    min(RATE_MAX, max(RATE_MIN, rate))
}

/// `encodeKind(preset, gpu)`: `preview`, `quality-gpu`, `discord-cpu`…
pub fn encode_kind(preset: &str, gpu: bool) -> String {
    if preset == "preview" {
        return "preview".to_owned();
    }
    let base = if preset == "discord" {
        "discord"
    } else {
        "quality"
    };
    format!("{base}-{}", if gpu { "gpu" } else { "cpu" })
}

/// Раздел таблицы замеров (`render` или `encode`), если это объект.
fn section<'a>(learned: &'a Value, key: &str) -> Option<&'a Map<String, Value>> {
    learned.get(key).and_then(Value::as_object)
}

/// `value.rate` записи замера как число JS; у не объекта поля нет.
fn rate_of(value: &Value) -> f64 {
    to_number(value.as_object().and_then(|entry| entry.get("rate")))
}

/// Скорость из своих замеров: точно это разрешение, иначе ближайшее того же вида — пересчётом
/// по пикселям (высота → 16:9). `None` — замеров нет.
fn learned_rate(
    table: Option<&Map<String, Value>>,
    prefix: &str,
    height: f64,
    exponent: f64,
) -> Option<f64> {
    let table = table?;
    let own = table.get(&format!("{prefix}@{}", number_to_string(height)));
    if let Some(own) = own.map(rate_of).filter(|rate| *rate > 0.0) {
        return Some(own);
    }
    let mut best: Option<(f64, f64)> = None;
    for (key, value) in table {
        // Ключ без «@» никогда не совпадёт с профилем и числом (`slice(0, -1)` в JS).
        let Some(at) = key.rfind('@') else { continue };
        let rate = rate_of(value);
        if &key[..at] != prefix || rate.is_nan() || rate <= 0.0 {
            continue;
        }
        let other = hlsr_core::js::to_number(&key[at + 1..]);
        if other.is_nan() || other <= 0.0 {
            continue;
        }
        if best.is_none_or(|(best_height, _)| (other - height).abs() < (best_height - height).abs())
        {
            best = Some((other, rate));
        }
    }
    best.map(|(other, rate)| rate * ((other * other) / (height * height)).powf(exponent))
}

/// `renderRate(profile, width, height, learned)`.
pub fn render_rate(profile_key: &str, width: f64, height: f64, learned: &Value) -> f64 {
    if let Some(own) = learned_rate(
        section(learned, "render"),
        profile_key,
        height,
        RENDER_EXPONENT,
    ) {
        return clamp_rate(own);
    }
    let base = profile(profile_key).unwrap_or_else(|| profile("other").expect("профиль other"));
    clamp_rate(base.rate / (pixels(width, height) / BASE_PIXELS).powf(RENDER_EXPONENT))
}

/// `encodeRate(kind, width, height, learned)`.
pub fn encode_rate(kind: &str, width: f64, height: f64, learned: &Value) -> f64 {
    if let Some(own) = learned_rate(section(learned, "encode"), kind, height, 1.0) {
        return clamp_rate(own);
    }
    clamp_rate(encode_base(kind) / (pixels(width, height) / BASE_PIXELS))
}

/// Что оценивать: профиль и запуски игры (от адаптера), отрезок демки, кадр, итог.
#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    pub profile: String,
    /// Запусков игры; нет или 0 — один.
    pub launches: Option<f64>,
    /// Сегментов; нет или 0 — один.
    pub segments: Option<f64>,
    /// Длина отрезка в демке, с.
    pub seconds: f64,
    pub fps: f64,
    pub width: f64,
    pub height: f64,
    /// Длина видео, с.
    pub output_seconds: f64,
    pub preset: String,
    pub gpu: bool,
}

/// Оценка: всего и по частям (с), учтены ли свои замеры.
#[derive(Debug, Clone, PartialEq)]
pub struct Estimate {
    pub seconds: f64,
    pub load: f64,
    pub render: f64,
    pub encode: f64,
    pub learned: bool,
}

impl Estimate {
    pub fn to_value(&self) -> Value {
        json!({
            "seconds": js_number(self.seconds),
            "parts": {
                "load": js_number(self.load),
                "render": js_number(self.render),
                "encode": js_number(self.encode),
            },
            "learned": self.learned,
        })
    }
}

/// `x || fallback` для числа: 0 и NaN — ложь.
fn or(value: Option<f64>, fallback: f64) -> f64 {
    value
        .filter(|x| *x != 0.0 && !x.is_nan())
        .unwrap_or(fallback)
}

/// `Math.max(0, Number(x) || 0)`.
fn non_negative(x: f64) -> f64 {
    max(0.0, or(Some(x), 0.0))
}

fn load_of(base: Profile, launches: Option<f64>, segments: Option<f64>) -> f64 {
    base.load * max(1.0, or(launches, 1.0)) + base.per_segment * max(0.0, or(segments, 1.0) - 1.0)
}

/// `estimate(input, learned)`.
pub fn estimate(input: &Input, learned: &Value) -> Estimate {
    let base = profile(&input.profile).unwrap_or_else(|| profile("other").expect("профиль other"));
    let load = load_of(base, input.launches, input.segments);
    let frames = non_negative(input.seconds) * input.fps;
    let render = frames / render_rate(&input.profile, input.width, input.height, learned);
    let kind = encode_kind(&input.preset, input.gpu);
    // Discord кодируется в два прохода только на процессоре: учтено в его скорости.
    let encode = non_negative(input.output_seconds) * input.fps
        / encode_rate(&kind, input.width, input.height, learned);
    let height = number_to_string(input.height);
    let has = |section_key: &str, key: String| {
        section(learned, section_key)
            .and_then(|table| table.get(&key))
            .is_some_and(truthy)
    };
    let own = has("render", format!("{}@{height}", input.profile))
        || has("encode", format!("{kind}@{height}"));
    Estimate {
        seconds: math_round(load + render + encode),
        load: math_round(load),
        render: math_round(render),
        encode: math_round(encode),
        learned: own,
    }
}

/// Замер готового задания (`stats` у [`crate::jobs::JobResult`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    /// Профиль от адаптера; нет — замер рендера не пишется.
    pub profile: Option<String>,
    pub launches: f64,
    pub segments: Option<f64>,
    pub frames: f64,
    pub render_seconds: f64,
    pub height: f64,
    pub preset: String,
    pub gpu: bool,
    pub output_frames: f64,
    pub encode_seconds: f64,
}

impl Stats {
    pub fn to_value(&self) -> Value {
        json!({
            "profile": self.profile,
            "launches": js_number(self.launches),
            "segments": self.segments.map(js_number),
            "frames": js_number(self.frames),
            "renderSeconds": js_number(self.render_seconds),
            "height": js_number(self.height),
            "preset": self.preset,
            "gpu": self.gpu,
            "outputFrames": js_number(self.output_frames),
            "encodeSeconds": js_number(self.encode_seconds),
        })
    }
}

fn merge(previous: Option<&Value>, rate: f64, frames: f64) -> Value {
    let previous_rate = previous.map_or(f64::NAN, rate_of);
    let weight = if previous_rate > 0.0 {
        // `previous.frames || 0`: истинное значение приводится к числу уже в Math.min.
        let stored = previous.and_then(|p| p.get("frames")).filter(|v| truthy(v));
        min(CAP_FRAMES, stored.map_or(0.0, |v| to_number(Some(v))))
    } else {
        0.0
    };
    let value = if weight != 0.0 && !weight.is_nan() {
        (previous_rate * weight + rate * frames) / (weight + frames)
    } else {
        rate
    };
    json!({
        "rate": js_number(math_round(clamp_rate(value) * 100.0) / 100.0),
        "frames": js_number(min(CAP_FRAMES, weight + frames)),
    })
}

/// `learn(learned, stats)`: замер готового задания → новая таблица скоростей. Запуск игры
/// берётся из профиля по умолчанию: один замер не разделяет его и кадры, а короткий ран почти
/// весь — запуск.
pub fn learn(learned: &Value, stats: Option<&Stats>) -> Value {
    let copy = |key: &str| section(learned, key).cloned().unwrap_or_default();
    let mut render = copy("render");
    let mut encode = copy("encode");
    if let Some(stats) = stats.filter(|stats| stats.height > 0.0) {
        let height = number_to_string(stats.height);
        let base = stats.profile.as_deref().and_then(profile);
        if let Some(base) =
            base.filter(|_| stats.frames >= MIN_FRAMES && stats.render_seconds > 0.0)
        {
            let load = load_of(base, Some(stats.launches), stats.segments);
            let busy = max(stats.render_seconds - load, stats.frames / RATE_MAX);
            let key = format!("{}@{height}", stats.profile.as_deref().unwrap_or_default());
            let next = merge(render.get(&key), stats.frames / busy, stats.frames);
            render.insert(key, next);
        }
        if !stats.preset.is_empty()
            && stats.output_frames >= MIN_FRAMES
            && stats.encode_seconds > 0.5
        {
            let key = format!("{}@{height}", encode_kind(&stats.preset, stats.gpu));
            let next = merge(
                encode.get(&key),
                stats.output_frames / stats.encode_seconds,
                stats.output_frames,
            );
            encode.insert(key, next);
        }
    }
    json!({"render": render, "encode": encode})
}
