//! Чистая логика рендера HL1 через bxt-rs (`goldsrc-render.js` 0.4.9): какие сегменты
//! рендерить, `job.cfg`, разбор `qconsole.log` и прогресса FFmpeg, привязка видео ко времени
//! демки, метки таймлайна и KeyHUD (ASS). Процессы и файлы игры — в [`crate::goldsrc`].

use crate::goldsrc_demo::{GoldsrcScan, MapChange, Press};
use crate::library::Segment;
use crate::planner::{Timing, material_time, normalize_cuts};
use crate::source_render::{Cut, Picked, Range};
use crate::{Marker, round3};
use hlsr_core::js::number::number_to_string;
use hlsr_core::js::{WHITESPACE_CLASS, math_round, slice_utf16};
use hlsr_delivery::goldsrc_personalization;
use regex::Regex;
use std::cmp::Ordering;
use std::sync::LazyLock;

/// Движок режет +команды командной строки на «-» и «+» даже внутри слова: в именах папок
/// задания их нет.
pub const RENDER_DIR: &str = "hlsr_render";
/// Сетевой протокол демок сборки HLSR WON (BUILD 3248); 48 — Steam HL1, этот движок его не
/// играет.
pub const NET_PROTOCOL: i32 = 47;
/// Громкость звука bxt-rs: 0,4 даёт редкие пики 0 dB.
pub const CAPTURE_VOLUME: f64 = 0.3;
/// Меток на ран: у настоящего рана HL1 — десятки, сотни.
pub const MAX_MARKERS: usize = 1000;

/// Клавиша KeyHUD: подпись, бит usercmd ([`crate::goldsrc_demo::BUTTONS`]), место в сетке
/// (единица — клавиша) и ширина.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    pub label: &'static str,
    pub bit: u16,
    pub x: f64,
    pub y: f64,
    pub w: f64,
}

const fn key(label: &'static str, bit: u16, x: f64, y: f64, w: f64) -> Key {
    Key {
        label,
        bit,
        x,
        y,
        w,
    }
}

pub const KEYS: [Key; 9] = [
    key("W", 8, 1.0, 0.0, 1.0),
    key("A", 512, 0.0, 1.0, 1.0),
    key("S", 16, 1.0, 1.0, 1.0),
    key("D", 1024, 2.0, 1.0, 1.0),
    key("DUCK", 4, 0.0, 2.0, 1.5),
    key("JUMP", 2, 1.5, 2.0, 1.5),
    key("USE", 32, 0.0, 3.0, 1.0),
    key("ATK", 1, 1.0, 3.0, 1.0),
    key("ATK2", 2048, 2.0, 3.0, 1.0),
];

/// `planSegments(segments, range)`: сегменты набора со временем демо (сумма длительностей, как
/// в `library.describe`). Рендерятся целиком те, что пересекают интервал; пустые (0 с)
/// пропускаются. Ответ — сегменты и время демо первого из них (`offset`).
pub fn plan_segments(segments: &[Segment], range: Option<Range>) -> (Vec<Picked<'_>>, f64) {
    let mut t = 0.0;
    let all: Vec<Picked> = segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            let duration = segment.duration().filter(|d| *d > 0.0).unwrap_or(0.0);
            let item = Picked {
                segment,
                index,
                start: round3(t),
                end: round3(t + duration),
                duration,
                frames: 0.0,
            };
            t += duration;
            item
        })
        .collect();
    let from = range.map_or(0.0, |r| r.from);
    let to = range.map_or(t, |r| r.to);
    let picked: Vec<Picked> = all
        .into_iter()
        .filter(|item| item.duration > 0.0 && item.end > from && item.start < to)
        .collect();
    let offset = picked.first().map_or(from, |item| item.start);
    (picked, offset)
}

/// `viewLines({hud, ownHud})`: вид в кадре после конфигов игрока. Без своего HUD — значения
/// «Персонализации» по умолчанию; без HUD — HUD игры и BXT целиком и прицел.
fn view_lines(hud: bool, own_hud: bool) -> Vec<String> {
    let mut lines = Vec::new();
    if !own_hud {
        let stock = goldsrc_personalization::to_cfg_lines(&serde_json::json!({}));
        lines.extend(
            stock
                .split('\n')
                .map(|line| line.strip_suffix('\r').unwrap_or(line))
                .filter(|line| !line.is_empty() && !line.starts_with("//"))
                .map(str::to_owned),
        );
    }
    if !hud {
        lines.extend(
            [
                "bxt_disable_hud 1",
                "bxt_hud 0",
                "bxt_cross 0",
                "crosshair 0",
            ]
            .map(str::to_owned),
        );
    }
    lines
}

/// `jobConfig({fps, dir, hud, ownHud})`: видео на каждый сегмент без кадров загрузки,
/// промежуточное видео высокого качества (итог кодирует общий слой). Аргументы FFmpeg bxt-rs
/// режет по пробелам: пути относительные и без пробелов.
pub fn job_config(fps: u32, dir: &str, hud: bool, own_hud: bool) -> String {
    let ffmpeg = format!(
        "-c:v libx264 -preset ultrafast -crf 14 -pix_fmt yuv420p -c:a alac -progress valve_WON/{dir}/progress.txt -stats_period 0.5"
    );
    let mut lines = vec![
        format!("bxt_cap_fps {fps}"),
        format!("bxt_cap_volume {CAPTURE_VOLUME}"),
    ];
    lines.extend(view_lines(hud, own_hud));
    lines.extend([
        "bxt_cap_skip_non_gameplay_frames 1".to_owned(),
        format!("_bxt_cap_override_ffmpeg_args \"{ffmpeg}\""),
        format!("bxt_play_folder {dir}"),
        format!("bxt_cap_separate_start valve_WON/{dir}/out"),
        String::new(),
    ]);
    lines.join("\n")
}

/// `gameArguments({width, height, dir})`: после Injector и полного пути к `hl.exe`; всё прочее
/// — в `job.cfg`.
pub fn game_arguments(width: u32, height: u32, dir: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "-steam",
        "-gl",
        "-32bpp",
        "-game",
        "valve_WON",
        "-noforcemparms",
        "-windowed",
        "-w",
    ]
    .map(str::to_owned)
    .to_vec();
    args.extend([
        width.to_string(),
        "-h".into(),
        height.to_string(),
        "-condebug".into(),
        "+exec".into(),
        format!("{dir}/job.cfg"),
    ]);
    args
}

/// `resetArguments()`: перезапуск для сброса mute — `hl.exe` без внедрения; в меню WON-сборки
/// звука нет.
pub fn reset_arguments() -> Vec<String> {
    [
        "-steam",
        "-gl",
        "-32bpp",
        "-game",
        "valve_WON",
        "-noforcemparms",
        "-windowed",
        "-w",
        "640",
        "-h",
        "480",
        "+volume",
        "0",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// Строка `qconsole.log` (`-condebug`), важная рендеру.
#[derive(Debug, Clone, PartialEq)]
pub enum LogLine {
    /// Началась демка `sNNN.dem` (номер с 1).
    Playing(u32),
    /// bxt-rs закончил видео сегмента.
    Stopped,
    Error {
        code: &'static str,
        /// Строка, не длиннее 300 знаков.
        text: String,
    },
}

/// `.` JS: всё, кроме концов строк.
const ANY: &str = r"[^\n\r\u{2028}\u{2029}]";

static PLAYING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)^Playing demo from (?:{ANY}*[\\/])?(s[0-9]{{3}})\.dem"
    ))
    .expect("шаблон")
});

static ERRORS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let not_space = format!("[^{WHITESPACE_CLASS}]");
    [
        (r"(?i)^Error initializing recording".to_owned(), "FFMPEG_START_FAILED"),
        (r"(?i)^Error during recording|^Resolution has changed".to_owned(), "RENDER_FAILED"),
        // bxt-rs не нашёл движок: его команд нет.
        (
            r"(?i)^Unknown command: ?(bxt_play_folder|bxt_cap_separate_start|_bxt_cap_override_ffmpeg_args|bxt_cap_)".to_owned(),
            "BXTRS_FAILED",
        ),
        (r"(?i)^Error: (no demos found|could not open directory)".to_owned(), "DEMO_OPEN_FAILED"),
        (format!(r"(?i)^(ERROR: )?(couldn't open|Demo file {ANY}* not found)"), "DEMO_OPEN_FAILED"),
        (format!(r"(?i)demo protocol outdated|protocol {ANY}* outdated"), "DEMO_PROTOCOL_UNSUPPORTED"),
        (
            format!(
                r"(?i)^Couldn't find map maps/|^Warning! File maps/{not_space}+ missing during demo playback|^Cannot continue without model maps/|Mod_NumForName: maps/{not_space}+ not found"
            ),
            "MAP_MISSING",
        ),
        (r"(?i)^Host_Error:|^Cannot continue without model ".to_owned(), "GAME_ERROR"),
    ]
    .into_iter()
    .map(|(pattern, code)| (Regex::new(&pattern).expect("шаблон"), code))
    .collect()
});

static OUT_TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("out_time_us=([0-9]+)").expect("шаблон"));

/// `parseLogLine(line)`.
pub fn parse_log_line(line: &str) -> Option<LogLine> {
    let text = line.strip_suffix('\r').unwrap_or(line);
    if let Some(found) = PLAYING.captures(text) {
        return Some(LogLine::Playing(found[1][1..].parse().ok()?));
    }
    if text.starts_with("Recording stopped.") {
        return Some(LogLine::Stopped);
    }
    ERRORS
        .iter()
        .find(|(re, _)| re.is_match(text))
        .map(|&(_, code)| LogLine::Error {
            code,
            text: slice_utf16(text, 300),
        })
}

/// `progressSeconds(text)`: последнее `out_time_us` из `-progress` FFmpeg (с).
pub fn progress_seconds(text: &str) -> Option<f64> {
    OUT_TIME
        .captures_iter(text)
        .last()
        .map(|found| found[1].parse::<f64>().unwrap_or(f64::NAN) / 1e6)
}

fn changes_of(scan: Option<&GoldsrcScan>) -> &[MapChange] {
    scan.map_or(&[], |scan| &scan.map_changes)
}

/// `lossCuts(picked, videos, scans)`: вырезанное bxt-rs во времени демо. Сколько — точно
/// (демка минус видео сегмента), где — по демке: в начале и на сменах карт, пропорционально
/// оценке из демки (`startGap`, `mapChanges[].gap`).
pub fn loss_cuts(picked: &[Picked], videos: &[f64], scans: &[Option<GoldsrcScan>]) -> Vec<Cut> {
    let mut cuts = Vec::new();
    for (k, item) in picked.iter().enumerate() {
        let video = videos
            .get(k)
            .copied()
            .filter(|v| !v.is_nan())
            .unwrap_or(0.0);
        let loss = item.duration - video;
        if loss.is_nan() || loss <= 0.0 {
            continue;
        }
        let scan = scans.get(k).and_then(Option::as_ref);
        let mut parts = vec![(0.0, scan.map_or(0.0, |scan| scan.start_gap).max(1e-3))];
        parts.extend(
            changes_of(scan)
                .iter()
                .filter(|change| change.t > 0.0 && change.gap > 0.0)
                .map(|change| (change.t, change.gap)),
        );
        let total = parts.iter().fold(0.0, |sum, part| sum + part.1);
        for (at, weight) in parts {
            let length = loss * weight / total;
            let from = (item.start + at).min(item.end);
            cuts.push(Cut {
                from: round3(from),
                to: round3((from + length).min(item.end)),
            });
        }
    }
    normalize_cuts(&cuts)
}

/// `timelineOf(picked, scans, range)`: метки во времени демо — начало, стыки сегментов
/// (сейвлоуд или смена карты) и смены карт внутри сегментов; не больше [`MAX_MARKERS`].
pub fn timeline_of(
    picked: &[Picked],
    scans: &[Option<GoldsrcScan>],
    range: Option<Range>,
) -> Vec<Marker> {
    let mut markers = Vec::new();
    let mut last_map: Option<String> = None;
    for (k, item) in picked.iter().enumerate() {
        let changes = changes_of(scans.get(k).and_then(Option::as_ref));
        let at = round3(if k == 0 {
            item.start.max(range.map_or(0.0, |r| r.from))
        } else {
            item.start
        });
        // Клип с середины сегмента — карта, которая шла к его началу.
        let before = (k == 0)
            .then(|| {
                changes
                    .iter()
                    .rfind(|change| change.t > 0.0 && item.start + change.t <= at)
            })
            .flatten();
        let map = before.map_or_else(
            || item.segment.map().to_owned(),
            |change| change.map.clone(),
        );
        let changed = k > 0
            && !map.is_empty()
            && last_map
                .as_ref()
                .is_some_and(|last| !last.is_empty() && *last != map);
        markers.push(if k == 0 || changed {
            Marker {
                t: at,
                kind: "map",
                label: map.clone(),
            }
        } else {
            Marker {
                t: at,
                kind: "saveload",
                label: (item.index + 1).to_string(),
            }
        });
        if !map.is_empty() {
            last_map = Some(map);
        }
        for change in changes {
            if last_map.as_ref() == Some(&change.map) || item.start + change.t <= at {
                continue;
            }
            if change.t > 0.0 {
                markers.push(Marker {
                    t: round3(item.start + change.t),
                    kind: "map",
                    label: change.map.clone(),
                });
            }
            last_map = Some(change.map.clone());
        }
    }
    markers.truncate(MAX_MARKERS);
    markers
}

/// `assTime(seconds)`: `H:MM:SS.CC`.
pub fn ass_time(seconds: f64) -> String {
    let cs = math_round(seconds * 100.0).max(0.0) as i64;
    format!(
        "{}:{:02}:{:02}.{:02}",
        cs / 360_000,
        cs % 360_000 / 6000,
        cs % 6000 / 100,
        cs % 100
    )
}

/// `keyEvents(picked, scans, timing)`: нажатия во времени материала; на стыке сегментов кнопки
/// сбрасываются.
pub fn key_events(picked: &[Picked], scans: &[Option<GoldsrcScan>], timing: &Timing) -> Vec<Press> {
    let mut events = Vec::new();
    for (k, item) in picked.iter().enumerate() {
        if let Some(scan) = scans.get(k).and_then(Option::as_ref) {
            events.extend(scan.buttons.iter().map(|press| Press {
                t: material_time((item.start + press.t).min(item.end), timing),
                buttons: press.buttons,
            }));
        }
        events.push(Press {
            t: material_time(item.end, timing),
            buttons: 0,
        });
    }
    events.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(Ordering::Equal));
    events
}

/// Прямоугольник клавиши в кадре.
struct KeyBox {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl KeyBox {
    fn rect(&self, color: &str, alpha: &str) -> String {
        let n = number_to_string;
        let (w, h) = (n(self.w), n(self.h));
        format!(
            "{{\\an7\\pos({},{})\\bord0\\shad0\\1c&H{color}&\\1a&H{alpha}&\\p1}}m 0 0 l {w} 0 {w} {h} 0 {h}{{\\p0}}",
            n(self.x),
            n(self.y)
        )
    }
}

/// `keyhudAss(events, {width, height, end})`: субтитры ASS с векторными клавишами (libass в
/// FFmpeg): серые клавиши всё время, нажатая подсвечена. Размер — от высоты кадра, место —
/// справа над патронами HUD. `end` — длительность материала.
pub fn keyhud_ass(events: &[Press], width: u32, height: u32, end: f64) -> String {
    let (width_f, height_f) = (f64::from(width), f64::from(height));
    let scale = height_f / 1080.0;
    let unit = math_round(44.0 * scale);
    let gap = math_round(6.0 * scale).max(2.0);
    let font = math_round(19.0 * scale);
    let left = math_round(width_f - height_f * 0.04 - (3.0 * unit + 2.0 * gap));
    let top = math_round(height_f * 0.8 - (4.0 * unit + 3.0 * gap));
    let boxed = |key: &Key| KeyBox {
        x: left + math_round(key.x * (unit + gap)),
        y: top + key.y * (unit + gap),
        w: math_round(key.w * unit + (key.w - 1.0) * gap),
        h: unit,
    };
    let mut lines: Vec<String> = vec![
        "[Script Info]".into(),
        "; KeyHUD HLSR: нажатия из usercmd демки".into(),
        "ScriptType: v4.00+".into(),
        format!("PlayResX: {width}"),
        format!("PlayResY: {height}"),
        "ScaledBorderAndShadow: yes".into(),
        "WrapStyle: 2".into(),
        String::new(),
        "[V4+ Styles]".into(),
        "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding".into(),
        format!(
            "Style: K,Arial,{},&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,1,0,0,0,100,100,0,0,1,{},0,5,0,0,0,1",
            number_to_string(font),
            number_to_string(math_round(scale).max(1.0))
        ),
        String::new(),
        "[Events]".into(),
        "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text".into(),
    ];
    let whole = format!("{},{}", ass_time(0.0), ass_time(end));
    for key in &KEYS {
        let b = boxed(key);
        lines.push(format!(
            "Dialogue: 0,{whole},K,,0,0,0,,{}",
            b.rect("000000", "70")
        ));
        lines.push(format!(
            "Dialogue: 2,{whole},K,,0,0,0,,{{\\an5\\pos({},{})}}{}",
            number_to_string(b.x + math_round(b.w / 2.0)),
            number_to_string(b.y + math_round(b.h / 2.0)),
            key.label
        ));
    }
    // Нажатия: интервалы, пока бит клавиши стоит (HL1 HUD — оранжевый).
    for key in &KEYS {
        let b = boxed(key);
        let mut since: Option<f64> = None;
        let mut close = |since: &mut Option<f64>, t: f64| {
            let a = math_round(since.take().unwrap_or(0.0) * 100.0);
            let z = math_round(t.min(end) * 100.0);
            if z > a {
                lines.push(format!(
                    "Dialogue: 1,{},{},K,,0,0,0,,{}",
                    ass_time(a / 100.0),
                    ass_time(z / 100.0),
                    b.rect("00A0FF", "30")
                ));
            }
        };
        for event in events {
            let down = event.buttons & key.bit != 0;
            if down && since.is_none() {
                since = Some(event.t);
            } else if !down && since.is_some() {
                close(&mut since, event.t);
            }
        }
        if since.is_some() {
            close(&mut since, end);
        }
    }
    lines.join("\n") + "\n"
}
