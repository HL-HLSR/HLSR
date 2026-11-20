//! Планировщик экспорта (`planner.js`): только чистые функции — сочетания, битрейт, фильтры,
//! аргументы FFmpeg, пересчёт таймлайна. Процессы запускает [`crate::jobs`].
//!
//! Числа — `f64`, как в JS: аргументы FFmpeg пишутся записью `String(number)` 0.4.9.

use crate::errors::DemoError;
use crate::source_render::{Cut, Range};
use crate::{Marker, round3};
use hlsr_core::js::number::number_to_string;
use hlsr_core::js::value::to_number;
use hlsr_core::js::{math_round, slice_utf16, truthy};
use hlsr_core::json_text::js_number;
use serde_json::{Map, Value, json};
use std::cmp::Ordering;

/// Разрешение экспорта: высота, ширина, подпись.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolution {
    pub height: u32,
    pub width: u32,
    pub label: &'static str,
}

/// `RESOLUTIONS`.
pub const RESOLUTIONS: [Resolution; 4] = [
    Resolution {
        height: 720,
        width: 1280,
        label: "720p",
    },
    Resolution {
        height: 1080,
        width: 1920,
        label: "1080p",
    },
    Resolution {
        height: 1440,
        width: 2560,
        label: "1440p",
    },
    Resolution {
        height: 2160,
        width: 3840,
        label: "4K",
    },
];
/// `FPS`.
pub const FPS: [u32; 3] = [30, 60, 120];
/// Пресеты окна экспорта; `preview` подставляет только предпросмотр.
pub const PRESETS: [&str; 2] = ["quality", "discord"];
/// `ENCODER_MODES`.
pub const ENCODER_MODES: [&str; 3] = ["auto", "gpu", "cpu"];
/// Порядок «Авто»: первый рабочий GPU, иначе процессор.
pub const GPU_ENCODERS: [&str; 3] = ["h264_nvenc", "h264_amf", "h264_qsv"];
/// `CPU_ENCODER`.
pub const CPU_ENCODER: &str = "libx264";
/// `SPEED`: пределы скорости.
pub const SPEED_MIN: f64 = 0.25;
pub const SPEED_MAX: f64 = 4.0;
/// Настройки вида в кадре: переключатели «Дополнительных настроек» предпросмотра и экспорта.
pub const VIEW_KEYS: [&str; 4] = ["keyhud", "hud", "ownConfig", "ownHud"];
/// `chapter` и `finish` — главы HL2 и финиш рана Source.
pub const MARKER_TYPES: [&str; 6] = ["map", "segment", "saveload", "timer", "chapter", "finish"];

/// `DISCORD`: лимит файла и запасы.
pub const DISCORD_LIMIT_BYTES: f64 = 10_000_000.0;
/// Запас под контейнер MP4 и неточность битрейта кодировщика.
const DISCORD_CONTAINER_RESERVE: f64 = 0.04;
const DISCORD_AUDIO_BPS: f64 = 96_000.0;
pub const DISCORD_MAX_RETRIES: u32 = 2;
const DISCORD_RETRY_SAFETY: f64 = 0.97;
/// Ниже этого видео перестаёт быть смотрибельным: бит на пиксель кадра.
const MIN_BITS_PER_PIXEL: f64 = 0.005;
const MIN_VIDEO_BPS: f64 = 250_000.0;

/// `AUDIO_BPS[preset]`, иначе как у `quality`.
fn audio_bps_of(preset: &str) -> f64 {
    if preset == "preview" {
        128_000.0
    } else {
        192_000.0
    }
}

/// Грубая оценка размера для качества по CRF/CQ (точного размера там нет).
fn rough_bits_per_pixel(preset: &str) -> f64 {
    if preset == "preview" { 0.03 } else { 0.08 }
}

/// `PIX_FMT[encoder] || "yuv420p"`.
fn pix_fmt(encoder: &str) -> &'static str {
    match encoder {
        "h264_qsv" | "h264_amf" => "nv12",
        _ => "yuv420p",
    }
}

/// `num(value)`: `String(round3(value))`.
fn num(value: f64) -> String {
    number_to_string(round3(value))
}

pub fn resolution_of(height: u32) -> Option<Resolution> {
    RESOLUTIONS.into_iter().find(|item| item.height == height)
}

/// Настройки экспорта после проверки (`normalizeSettings`).
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub preset: String,
    pub height: u32,
    pub fps: u32,
    pub audio: bool,
    pub keyhud: bool,
    /// Надпись проверки данных демки поверх кадра («GH DEMO DATA: INTACT» у Ghosting).
    pub integrity: bool,
    /// HUD игры в кадре.
    pub hud: bool,
    /// Конфиг игрока (иначе стандартный).
    pub own_config: bool,
    /// HUD из «Персонализации» (иначе стандартный).
    pub own_hud: bool,
    /// `auto`, `gpu`, `cpu`.
    pub encoder: String,
    pub width: u32,
    pub speed: f64,
    pub range: Range,
}

impl Settings {
    /// Как объект JS (порядок ключей — как у `normalizeSettings`).
    pub fn to_value(&self) -> Value {
        json!({
            "preset": self.preset,
            "height": self.height,
            "fps": self.fps,
            "audio": self.audio,
            "keyhud": self.keyhud,
            "integrity": self.integrity,
            "hud": self.hud,
            "ownConfig": self.own_config,
            "ownHud": self.own_hud,
            "encoder": self.encoder,
            "width": self.width,
            "speed": js_number(self.speed),
            "range": range_value(self.range),
        })
    }
}

pub(crate) fn range_value(range: Range) -> Value {
    json!({"from": js_number(range.from), "to": js_number(range.to)})
}

/// `DEFAULTS` (`preview == false`) или `PREVIEW` — значения по умолчанию как объект настроек.
pub fn defaults(preview: bool) -> Value {
    let mut out = json!({
        "preset": if preview { "preview" } else { "quality" },
        "height": if preview { 720 } else { 1080 },
        "fps": if preview { 30 } else { 60 },
        "audio": true,
        "keyhud": false,
        "integrity": false,
        "encoder": "auto",
        "speed": 1,
        "hud": true,
        "ownConfig": true,
        "ownHud": true,
    });
    if preview {
        out.as_object_mut().expect("объект").shift_remove("encoder");
    }
    out
}

fn invalid(field: &str) -> DemoError {
    DemoError::new("SETTINGS_INVALID").with_details(json!({"field": field}))
}

/// `normalizeSettings(raw, duration, { allowPreview })`: значения из окна — только из списков,
/// скорость 0,25–4, отрезок внутри длительности и не короче 0,1 с видео.
pub fn normalize_settings(
    raw: &Value,
    duration: Option<f64>,
    allow_preview: bool,
) -> Result<Settings, DemoError> {
    let empty = Map::new();
    let input = raw.as_object().unwrap_or(&empty);
    let given = |key: &str| input.get(key).filter(|value| !value.is_null());
    let choose_text = |key: &str, allowed: &[&str], default: &str| match given(key) {
        None => Ok(default.to_owned()),
        Some(Value::String(text)) if allowed.contains(&text.as_str()) => Ok(text.clone()),
        Some(_) => Err(invalid(key)),
    };
    let choose_number = |key: &str, allowed: &[u32], default: u32| match given(key) {
        None => Ok(default),
        Some(value @ Value::Number(_)) => {
            let n = to_number(Some(value));
            allowed
                .iter()
                .copied()
                .find(|item| f64::from(*item) == n)
                .ok_or_else(|| invalid(key))
        }
        Some(_) => Err(invalid(key)),
    };
    let flag = |key: &str, default: bool| given(key).map_or(default, |v| *v == Value::Bool(true));

    let presets: &[&str] = if allow_preview {
        &["quality", "discord", "preview"]
    } else {
        &PRESETS
    };
    let preset = choose_text("preset", presets, "quality")?;
    let heights = RESOLUTIONS.map(|item| item.height);
    let height = choose_number("height", &heights, 1080)?;
    let fps = choose_number("fps", &FPS, 60)?;
    let audio = flag("audio", true);
    let keyhud = flag("keyhud", false);
    let integrity = flag("integrity", false);
    let hud = flag("hud", true);
    let own_config = flag("ownConfig", true);
    let own_hud = flag("ownHud", true);
    let encoder = choose_text("encoder", &ENCODER_MODES, "auto")?;
    let width = resolution_of(height).expect("высота из списка").width;

    let speed = given("speed").map_or(1.0, |value| to_number(Some(value)));
    if !speed.is_finite() || !(SPEED_MIN..=SPEED_MAX).contains(&speed) {
        return Err(invalid("speed"));
    }
    let speed = math_round(speed * 100.0) / 100.0;

    let Some(duration) = duration.filter(|d| *d > 0.0) else {
        return Err(DemoError::new("DURATION_UNKNOWN"));
    };
    let (mut from, mut to) = (0.0, duration);
    if let Some(range) = input.get("range").filter(|range| truthy(range)) {
        // У примитива и массива полей from/to нет — NaN.
        let field = |key: &str| to_number(range.as_object().and_then(|r| r.get(key)));
        from = field("from");
        to = field("to");
        // Ползунок может отдать конец чуть дальше длительности — прижимаем.
        if to.is_finite() && to > duration && to <= duration + 0.05 {
            to = duration;
        }
        if !from.is_finite() || !to.is_finite() || from < 0.0 || to > duration || to <= from {
            return Err(DemoError::new("RANGE_INVALID"));
        }
    }
    if (to - from) / speed < 0.1 {
        return Err(DemoError::new("RANGE_INVALID"));
    }
    Ok(Settings {
        preset,
        height,
        fps,
        audio,
        keyhud,
        integrity,
        hud,
        own_config,
        own_hud,
        encoder,
        width,
        speed,
        range: Range {
            from: round3(from),
            to: round3(to),
        },
    })
}

/// Цепочка `atempo` с множителями в [0.5, 2]: звук ускоряется без смены высоты.
pub fn atempo_chain(speed: f64) -> Vec<f64> {
    // Вне пределов (0, ±inf, NaN) цикл ниже не кончается; задание проверяет скорость раньше.
    assert!(
        (SPEED_MIN..=SPEED_MAX).contains(&speed),
        "скорость {speed} вне [{SPEED_MIN}, {SPEED_MAX}]"
    );
    let mut chain = Vec::new();
    let mut rest = speed;
    while rest > 2.0 {
        chain.push(2.0);
        rest /= 2.0;
    }
    while rest < 0.5 {
        chain.push(0.5);
        rest /= 0.5;
    }
    chain.push(math_round(rest * 1e6) / 1e6);
    chain
}

/// `normalizeCuts(cuts)`: конечные, непустые, от нуля; по началу; пересекающиеся слиты.
pub fn normalize_cuts(cuts: &[Cut]) -> Vec<Cut> {
    let mut sorted: Vec<Cut> = cuts
        .iter()
        .copied()
        .filter(|cut| {
            cut.from.is_finite() && cut.to.is_finite() && cut.to > cut.from && cut.from >= 0.0
        })
        .collect();
    sorted.sort_by(|a, b| a.from.partial_cmp(&b.from).unwrap_or(Ordering::Equal));
    let mut merged: Vec<Cut> = Vec::new();
    for cut in sorted {
        match merged.last_mut() {
            Some(last) if cut.from <= last.to => last.to = last.to.max(cut.to),
            _ => merged.push(cut),
        }
    }
    merged
}

/// Начало материала (время демо) и вырезанные из него участки (загрузки).
#[derive(Debug, Clone, PartialEq)]
pub struct Timing {
    pub offset: f64,
    pub cuts: Vec<Cut>,
}

/// `materialTime(t, timing)`: время в материале адаптера.
pub fn material_time(t: f64, timing: &Timing) -> f64 {
    let mut removed = 0.0;
    for cut in &timing.cuts {
        let a = cut.from.max(timing.offset);
        let b = cut.to.min(t);
        if b > a {
            removed += b - a;
        }
    }
    (t - timing.offset - removed).max(0.0)
}

/// Отрезок источника для таймлайна: `{from, to, speed, offset, cuts}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    pub from: f64,
    pub to: f64,
    pub speed: f64,
    /// Время демо первого кадра материала; нет — `from`.
    pub offset: Option<f64>,
    pub cuts: Vec<Cut>,
}

impl Clip {
    fn timing(&self) -> Timing {
        Timing {
            offset: self.offset.unwrap_or(self.from),
            cuts: normalize_cuts(&self.cuts),
        }
    }
}

/// Метка таймлайна видео: время видео и время демо.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineMarker {
    pub t: f64,
    pub kind: &'static str,
    pub label: String,
    pub source_time: f64,
}

impl TimelineMarker {
    pub fn to_value(&self) -> Value {
        json!({
            "t": js_number(self.t),
            "type": self.kind,
            "label": self.label,
            "sourceTime": js_number(self.source_time),
        })
    }
}

/// `mapTimeline(markers, clip)`: метки источника → время видео
/// `t = (t_демо − от − вырезанное) / скорость`; вне отрезка и чужих типов нет.
pub fn map_timeline(markers: &[Marker], clip: &Clip) -> Vec<TimelineMarker> {
    let timing = clip.timing();
    let start = material_time(clip.from, &timing);
    let mut out: Vec<TimelineMarker> = markers
        .iter()
        .filter(|marker| {
            MARKER_TYPES.contains(&marker.kind)
                && marker.t.is_finite()
                && marker.t >= clip.from - 1e-6
                && marker.t <= clip.to + 1e-6
        })
        .map(|marker| TimelineMarker {
            t: round3((material_time(marker.t, &timing) - start) / clip.speed),
            kind: marker.kind,
            label: slice_utf16(&marker.label, 64),
            source_time: round3(marker.t),
        })
        .collect();
    // Как `a.t - b.t`: -0 и 0 равны, порядок сохраняется.
    out.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(Ordering::Equal));
    out
}

/// `clipDuration(clip)`: длительность видео с учётом скорости и вырезанного.
pub fn clip_duration(clip: &Clip) -> f64 {
    let timing = clip.timing();
    round3((material_time(clip.to, &timing) - material_time(clip.from, &timing)) / clip.speed)
}

/// Сочетание разрешение × FPS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Combo {
    pub height: u32,
    pub width: u32,
    pub fps: u32,
}

/// Что умеет адаптер рендера: предел высоты и частоты кадров.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub max_height: u32,
    pub fps: Vec<u32>,
}

/// `allowedCombos(capabilities, supports)`: пересечение возможностей адаптера и проб
/// кодировщика (`supports(height, fps)` — прошла ли проба).
pub fn allowed_combos(
    caps: &Capabilities,
    supports: Option<&dyn Fn(u32, u32) -> bool>,
) -> Vec<Combo> {
    let fps_list: Vec<u32> = FPS
        .into_iter()
        .filter(|fps| caps.fps.contains(fps))
        .collect();
    let mut combos = Vec::new();
    for resolution in RESOLUTIONS {
        if resolution.height > caps.max_height {
            continue;
        }
        for &fps in &fps_list {
            if supports.is_none_or(|supports| supports(resolution.height, fps)) {
                combos.push(Combo {
                    height: resolution.height,
                    width: resolution.width,
                    fps,
                });
            }
        }
    }
    combos
}

/// `minVideoBps(width, height, fps)`.
pub fn min_video_bps(width: u32, height: u32, fps: u32) -> f64 {
    MIN_VIDEO_BPS.max(math_round(
        f64::from(width) * f64::from(height) * f64::from(fps) * MIN_BITS_PER_PIXEL,
    ))
}

/// Бюджет Discord: бит на весь клип, звук и остаток видео.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiscordBudget {
    pub audio_bps: f64,
    pub budget_bits: f64,
    pub video_bps: f64,
}

/// `limitBytes || DISCORD.limitBytes`.
fn limit_or_default(limit_bytes: Option<f64>) -> f64 {
    limit_bytes
        .filter(|limit| *limit != 0.0 && !limit.is_nan())
        .unwrap_or(DISCORD_LIMIT_BYTES)
}

fn budget_bits(limit_bytes: f64) -> f64 {
    limit_bytes * (1.0 - DISCORD_CONTAINER_RESERVE) * 8.0
}

/// `discordBudget(duration, audio, limitBytes)`.
pub fn discord_budget(duration: f64, audio: bool, limit_bytes: Option<f64>) -> DiscordBudget {
    let audio_bps = if audio { DISCORD_AUDIO_BPS } else { 0.0 };
    let budget_bits = budget_bits(limit_or_default(limit_bytes));
    DiscordBudget {
        audio_bps,
        budget_bits,
        video_bps: (budget_bits / duration - audio_bps).floor(),
    }
}

/// Размер кадра для советов Discord.
#[derive(Debug, Clone, Copy)]
struct Frame {
    width: u32,
    height: u32,
    fps: u32,
}

/// `tooLong(...)`: `DISCORD_TOO_LONG` с рекомендованной длительностью и разрешением пониже,
/// в которое этот клип ещё влезает.
fn too_long(
    reason: &str,
    recommended: Option<f64>,
    budget_bits: f64,
    audio_bps: f64,
    frame: Frame,
    duration: f64,
    limit_bytes: Option<f64>,
) -> DemoError {
    let floor = min_video_bps(frame.width, frame.height, frame.fps);
    let max_seconds = (budget_bits / (floor + audio_bps) * 10.0).floor() / 10.0;
    let lower = RESOLUTIONS
        .iter()
        .filter(|item| item.height < frame.height)
        .rev()
        .find(|item| {
            budget_bits / duration - audio_bps >= min_video_bps(item.width, item.height, frame.fps)
        });
    DemoError::new("DISCORD_TOO_LONG").with_details(json!({
        "recommendedMaxSeconds": js_number(recommended.unwrap_or(max_seconds)),
        "suggestedHeight": lower.map(|item| item.height),
        "limitBytes": js_number(limit_or_default(limit_bytes)),
        "reason": reason,
    }))
}

/// План для окна экспорта и для задания (`planExport`).
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub preset: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub speed: f64,
    pub range: Range,
    pub audio: bool,
    pub keyhud: bool,
    pub encoder: String,
    /// Длительность итогового видео (с учётом скорости и вырезанного).
    pub output_duration: f64,
    pub warnings: Vec<String>,
    /// `bitrate` (Discord) или `quality`.
    pub mode: &'static str,
    pub video_bps: Option<f64>,
    pub audio_bps: f64,
    pub two_pass: bool,
    pub limit_bytes: Option<f64>,
    pub estimated_bytes: f64,
}

impl Plan {
    pub fn to_value(&self) -> Value {
        json!({
            "preset": self.preset,
            "width": self.width,
            "height": self.height,
            "fps": self.fps,
            "speed": js_number(self.speed),
            "range": range_value(self.range),
            "audio": self.audio,
            "keyhud": self.keyhud,
            "encoder": self.encoder,
            "outputDuration": js_number(self.output_duration),
            "warnings": self.warnings,
            "mode": self.mode,
            "videoBps": self.video_bps.map(js_number),
            "audioBps": js_number(self.audio_bps),
            "twoPass": self.two_pass,
            "limitBytes": self.limit_bytes.map(js_number),
            "estimatedBytes": js_number(self.estimated_bytes),
        })
    }

    fn frame(&self) -> Frame {
        Frame {
            width: self.width,
            height: self.height,
            fps: self.fps,
        }
    }
}

/// Параметры `planExport`: кодировщик (по умолчанию процессор), длительность итога, лимит
/// Discord (по умолчанию 10 000 000) и уже накопленные предупреждения.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlanOptions {
    pub encoder: Option<String>,
    pub output_duration: f64,
    pub limit_bytes: Option<f64>,
    pub warnings: Vec<String>,
}

/// `planExport(settings, options)`.
pub fn plan_export(settings: &Settings, options: &PlanOptions) -> Result<Plan, DemoError> {
    let encoder = options
        .encoder
        .clone()
        .filter(|encoder| !encoder.is_empty())
        .unwrap_or_else(|| CPU_ENCODER.to_owned());
    let duration = options.output_duration;
    let frame = Frame {
        width: settings.width,
        height: settings.height,
        fps: settings.fps,
    };
    let mut plan = Plan {
        preset: settings.preset.clone(),
        width: settings.width,
        height: settings.height,
        fps: settings.fps,
        speed: settings.speed,
        range: settings.range,
        audio: settings.audio,
        keyhud: settings.keyhud,
        two_pass: false,
        output_duration: duration,
        warnings: options.warnings.clone(),
        mode: "quality",
        video_bps: None,
        audio_bps: 0.0,
        limit_bytes: None,
        estimated_bytes: 0.0,
        encoder,
    };

    if settings.preset == "discord" {
        let budget = discord_budget(duration, settings.audio, options.limit_bytes);
        let floor = min_video_bps(settings.width, settings.height, settings.fps);
        if budget.video_bps < floor {
            return Err(too_long(
                "bitrate-floor",
                None,
                budget.budget_bits,
                budget.audio_bps,
                frame,
                duration,
                options.limit_bytes,
            ));
        }
        plan.mode = "bitrate";
        plan.video_bps = Some(budget.video_bps);
        plan.audio_bps = budget.audio_bps;
        plan.two_pass = plan.encoder == CPU_ENCODER;
        plan.limit_bytes = Some(limit_or_default(options.limit_bytes));
        plan.estimated_bytes = math_round((budget.video_bps + budget.audio_bps) * duration / 8.0);
        if budget.video_bps < floor * 2.0 {
            plan.warnings.push("DISCORD_LOW_QUALITY".to_owned());
        }
    } else {
        let audio_bps = if settings.audio {
            audio_bps_of(&settings.preset)
        } else {
            0.0
        };
        let pixels =
            f64::from(settings.width) * f64::from(settings.height) * f64::from(settings.fps);
        plan.audio_bps = audio_bps;
        plan.estimated_bytes = math_round(
            (pixels * rough_bits_per_pixel(&settings.preset) + audio_bps) * duration / 8.0,
        );
    }
    if plan.encoder == CPU_ENCODER && (settings.height >= 2160 || settings.fps >= 120) {
        plan.warnings.push("CPU_SLOW".to_owned());
    }
    Ok(plan)
}

/// `discordRetry(plan, actualBytes, attempt)`: итог больше лимита — битрейт с поправкой по
/// факту. `attempt` — номер повтора (1, 2); дальше или ниже порога — ошибка с советом.
pub fn discord_retry(plan: &Plan, actual_bytes: f64, attempt: u32) -> Result<f64, DemoError> {
    let limit = limit_or_default(plan.limit_bytes);
    let budget = budget_bits(limit);
    if attempt > DISCORD_MAX_RETRIES {
        let recommended = (plan.output_duration * limit / actual_bytes * 0.9 * 10.0).floor() / 10.0;
        return Err(too_long(
            "retries",
            Some(recommended),
            budget,
            plan.audio_bps,
            plan.frame(),
            plan.output_duration,
            Some(limit),
        ));
    }
    let audio_bytes = plan.audio_bps * plan.output_duration / 8.0;
    let video_bytes = actual_bytes - audio_bytes;
    let usable = limit * (1.0 - DISCORD_CONTAINER_RESERVE);
    let wanted = usable - audio_bytes;
    let factor = if wanted > 0.0 && video_bytes > 0.0 {
        wanted / video_bytes
    } else {
        usable / actual_bytes
    } * DISCORD_RETRY_SAFETY;
    // Битрейта нет (`null` в JS) — ноль.
    let next = (plan.video_bps.unwrap_or(0.0) * factor.min(1.0)).floor();
    if next < min_video_bps(plan.width, plan.height, plan.fps) {
        return Err(too_long(
            "bitrate-floor",
            None,
            budget,
            plan.audio_bps,
            plan.frame(),
            plan.output_duration,
            Some(limit),
        ));
    }
    Ok(next)
}

/// Субтитры ASS адаптера во времени материала (KeyHUD HL1): файл в папке задания и время
/// материала первого кадра после `-ss`.
#[derive(Debug, Clone, PartialEq)]
pub struct Overlay {
    pub file: String,
    pub start: f64,
}

/// `videoFilter(plan, overlay)`.
pub fn video_filter(plan: &Plan, overlay: Option<&Overlay>) -> String {
    let mut chain = Vec::new();
    if let Some(overlay) = overlay {
        chain.push(format!("setpts=PTS-STARTPTS+{}/TB", num(overlay.start)));
        chain.push(format!("ass={}", overlay.file));
    }
    chain.push(format!("setpts=(PTS-STARTPTS)/{}", num(plan.speed)));
    chain.push(format!("fps={}", plan.fps));
    chain.push(format!(
        "scale={}:{}:force_original_aspect_ratio=decrease:flags=lanczos",
        plan.width, plan.height
    ));
    chain.push(format!(
        "pad={}:{}:(ow-iw)/2:(oh-ih)/2",
        plan.width, plan.height
    ));
    chain.push("setsar=1".to_owned());
    chain.push(format!("format={}", pix_fmt(&plan.encoder)));
    chain.join(",")
}

/// `audioFilter(speed)`: `apad` + `-t` на выходе — звук не короче и не длиннее видео.
pub fn audio_filter(speed: f64) -> String {
    let mut chain = vec!["asetpts=PTS-STARTPTS".to_owned()];
    if speed != 1.0 {
        chain.extend(
            atempo_chain(speed)
                .into_iter()
                .map(|factor| format!("atempo={}", number_to_string(factor))),
        );
    }
    chain.push("aresample=48000".to_owned());
    chain.push("apad".to_owned());
    chain.join(",")
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

/// `videoCodecArgs(plan, pass, passlog)`: кодировщик и его режим качества или битрейта.
pub fn video_codec_args(plan: &Plan, pass: u8, passlog: &str) -> Result<Vec<String>, DemoError> {
    let encoder = plan.encoder.as_str();
    let discord = plan.preset == "discord";
    let preview = plan.preset == "preview";
    // `String(null)` и `null * 2` у плана без битрейта.
    let bps = plan
        .video_bps
        .map_or_else(|| "null".to_owned(), number_to_string);
    let bufsize = number_to_string(plan.video_bps.unwrap_or(0.0) * 2.0);
    let rate = || {
        vec![
            "-b:v".to_owned(),
            bps.clone(),
            "-maxrate".to_owned(),
            bps.clone(),
            "-bufsize".to_owned(),
            bufsize.clone(),
        ]
    };
    let mut args = vec![
        "-c:v".to_owned(),
        encoder.to_owned(),
        "-pix_fmt".to_owned(),
        pix_fmt(encoder).to_owned(),
        "-r".to_owned(),
        plan.fps.to_string(),
        "-g".to_owned(),
        (plan.fps * 2).to_string(),
    ];
    match encoder {
        "libx264" if discord => {
            args.extend(strings(&[
                "-profile:v",
                "high",
                "-preset",
                "medium",
                "-b:v",
            ]));
            args.push(bps.clone());
            args.push("-pass".to_owned());
            args.push(pass.to_string());
            args.push("-passlogfile".to_owned());
            args.push(passlog.to_owned());
        }
        "libx264" => {
            args.extend(strings(&["-profile:v", "high"]));
            args.extend(strings(if preview {
                &["-preset", "veryfast", "-crf", "26"]
            } else {
                &["-preset", "slow", "-crf", "18"]
            }));
        }
        "h264_nvenc" if discord => {
            args.extend(strings(&["-preset", "p5", "-tune", "hq", "-rc", "vbr"]));
            args.extend(rate());
        }
        "h264_nvenc" => args.extend(strings(if preview {
            &["-preset", "p2", "-rc", "vbr", "-cq", "28", "-b:v", "0"]
        } else {
            &[
                "-preset", "p5", "-tune", "hq", "-rc", "vbr", "-cq", "19", "-b:v", "0",
            ]
        })),
        "h264_qsv" if discord => {
            args.extend(strings(&["-preset", "slower"]));
            args.extend(rate());
        }
        "h264_qsv" => args.extend(strings(if preview {
            &["-preset", "veryfast", "-global_quality", "28"]
        } else {
            &["-preset", "slower", "-global_quality", "20"]
        })),
        "h264_amf" if discord => {
            args.extend(strings(&["-quality", "quality", "-rc", "vbr_peak"]));
            args.extend(rate());
        }
        "h264_amf" => args.extend(strings(if preview {
            &[
                "-quality", "speed", "-rc", "cqp", "-qp_i", "26", "-qp_p", "28",
            ]
        } else {
            &[
                "-quality", "quality", "-rc", "cqp", "-qp_i", "18", "-qp_p", "20",
            ]
        })),
        _ => return Err(DemoError::new("ENCODER_UNAVAILABLE")),
    }
    Ok(args)
}

/// Видеоряд материала: файл адаптера или кадры по шаблону.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Video(String),
    Frames {
        pattern: String,
        fps: f64,
        /// Номер первого кадра; нет (или не целое) — по умолчанию FFmpeg.
        start_number: Option<f64>,
    },
}

/// Сырьё для кодирования: видеоряд, звук (файл или нет), оверлей ASS (имя в папке задания).
#[derive(Debug, Clone, PartialEq)]
pub struct EncodeMaterial {
    pub source: Source,
    pub audio: Option<String>,
    pub overlay: Option<String>,
}

/// Отрезок материала `{start, duration}`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trim {
    pub start: f64,
    pub duration: f64,
}

/// `encodeArgs(...)`: `pass` 0 — один проход, 1/2 — двухпроходное кодирование libx264.
pub fn encode_args(
    material: &EncodeMaterial,
    trim: Trim,
    plan: &Plan,
    output: &str,
    pass: u8,
    passlog: &str,
    null_target: &str,
) -> Result<Vec<String>, DemoError> {
    let seek = [
        "-ss".to_owned(),
        num(trim.start),
        "-t".to_owned(),
        num(trim.duration),
    ];
    let mut args = strings(&[
        "-hide_banner",
        "-nostdin",
        "-y",
        "-loglevel",
        "error",
        "-progress",
        "pipe:1",
        "-nostats",
    ]);
    let input = match &material.source {
        Source::Frames {
            pattern,
            fps,
            start_number,
        } => {
            args.push("-framerate".to_owned());
            args.push(number_to_string(*fps));
            if let Some(start) = start_number.filter(|n| n.is_finite() && n.fract() == 0.0) {
                args.push("-start_number".to_owned());
                args.push(number_to_string(start));
            }
            pattern
        }
        Source::Video(video) => video,
    };
    args.extend(seek.iter().cloned());
    args.push("-i".to_owned());
    args.push(input.clone());
    let audio = material.audio.as_ref().filter(|_| plan.audio && pass != 1);
    if let Some(audio) = audio {
        args.extend(seek.iter().cloned());
        args.push("-i".to_owned());
        args.push(audio.clone());
    }
    args.push("-map".to_owned());
    args.push("0:v:0".to_owned());
    if audio.is_some() {
        args.push("-map".to_owned());
        args.push("1:a:0".to_owned());
    }
    let overlay = material.overlay.as_ref().map(|file| Overlay {
        file: file.clone(),
        start: trim.start,
    });
    args.push("-filter:v".to_owned());
    args.push(video_filter(plan, overlay.as_ref()));
    args.extend(video_codec_args(plan, pass.max(1), passlog)?);
    if audio.is_some() {
        args.push("-filter:a".to_owned());
        args.push(audio_filter(plan.speed));
        args.extend(strings(&["-c:a", "aac", "-b:a"]));
        args.push(number_to_string(plan.audio_bps));
        args.extend(strings(&["-ac", "2"]));
    } else {
        args.push("-an".to_owned());
    }
    args.push("-t".to_owned());
    args.push(num(plan.output_duration));
    if pass == 1 {
        args.extend(strings(&["-f", "null", null_target]));
    } else {
        args.extend(strings(&["-movflags", "+faststart", output]));
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atempo_product_is_speed() {
        for speed in [1.0, 2.0, 4.0, 3.0, 0.5, 0.25, 0.3, 1.25] {
            let chain = atempo_chain(speed);
            assert!(chain.iter().all(|f| (0.5..=2.0).contains(f)));
            assert!((chain.iter().product::<f64>() - speed).abs() < 1e-6);
        }
    }

    #[test]
    fn defaults_are_settings() {
        let settings = normalize_settings(&defaults(false), Some(10.0), false).unwrap();
        assert_eq!(
            (settings.height, settings.fps, settings.preset.as_str()),
            (1080, 60, "quality")
        );
        let preview = normalize_settings(&defaults(true), Some(10.0), true).unwrap();
        assert_eq!(
            (preview.height, preview.fps, preview.encoder.as_str()),
            (720, 30, "auto")
        );
    }
}
