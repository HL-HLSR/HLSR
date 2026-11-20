//! Чистая логика рендера Source (`source-render.js`): какие сегменты рендерить и метки из
//! плейлиста (их же берёт список демок, `library.describe`), стандартный конфиг и HUD в рендере,
//! кадр TGA игры и командная строка `CreateProcessW`. Пачек `startdemos`, журнала консоли и
//! склейки WAV старого пути нет: рендер Ghosting только через `-ghrender` (решение владельца 5,
//! [`crate::ghosting`]).

use crate::library::Segment;
use crate::{Marker, round3, u16_at};
use hlsr_core::js::{is_whitespace, math_round, to_fixed};
use hlsr_delivery::ghosting_personalization::{
    PERSONALIZATION_FILE, cfg_lines, default_personalization,
};

/// Тикрейт Ghosting; заголовок демо даёт его как ticks / playback_time.
const DEFAULT_TICKRATE: f64 = 100.0;

/// Интервал во времени демо.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub from: f64,
    pub to: f64,
}

/// Сегмент, который рендерится: время в демо и число кадров.
#[derive(Debug, Clone, PartialEq)]
pub struct Picked<'a> {
    pub segment: &'a Segment,
    /// Номер в списке сегментов, с 0.
    pub index: usize,
    pub start: f64,
    pub end: f64,
    pub duration: f64,
    pub frames: f64,
}

/// Вырезанный отрезок времени демо.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cut {
    pub from: f64,
    pub to: f64,
}

/// `planSegments(...)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan<'a> {
    pub picked: Vec<Picked<'a>>,
    pub offset: f64,
    pub cuts: Vec<Cut>,
    pub frames_total: f64,
    /// Сколько кадров хватит до конца интервала; `None` — нужны все.
    pub frames_needed: Option<f64>,
}

/// `planSegments(segments, range, fps)`: сегменты в порядке плейлиста со временем демо (сумма
/// длительностей). Рендерятся непустые (ticks > 1: пустые — спам сейвлоуда,
/// `gh_demo_skip_empty` их тоже пропускает), пересекающие интервал.
pub fn plan_segments(segments: &[Segment], range: Option<Range>, fps: f64) -> Plan<'_> {
    let mut all = Vec::with_capacity(segments.len());
    let mut t = 0.0;
    for (index, segment) in segments.iter().enumerate() {
        let duration = segment.duration().filter(|d| *d >= 0.0).unwrap_or(0.0);
        all.push(Picked {
            segment,
            index,
            start: t,
            end: t + duration,
            duration,
            frames: 0.0,
        });
        t += duration;
    }
    let from = range.map_or(0.0, |r| r.from);
    let to = range.map_or(t, |r| r.to);
    let mut picked: Vec<Picked> = all
        .iter()
        .filter(|item| item.segment.ticks() > 1 && item.end > from && item.start < to)
        .cloned()
        .collect();
    for item in &mut picked {
        let tickrate = item
            .segment
            .tickrate()
            .filter(|rate| *rate > 0.0)
            .unwrap_or(DEFAULT_TICKRATE);
        item.frames = math_round(f64::from(item.segment.ticks()) / tickrate * fps);
    }
    let (Some(first), Some(last)) = (picked.first(), picked.last()) else {
        return Plan {
            picked,
            offset: from,
            cuts: Vec::new(),
            frames_total: 0.0,
            frames_needed: None,
        };
    };
    // Материал — подряд отрендеренные сегменты: пропущенные между ними (пустые) вырезаны из
    // времени демо.
    let (first_index, last_index, first_start) = (first.index, last.index, first.start);
    let cuts: Vec<Cut> = all
        .iter()
        .filter(|item| {
            item.index > first_index
                && item.index < last_index
                && !picked.iter().any(|p| p.index == item.index)
                && item.duration > 0.0
        })
        .map(|item| Cut {
            from: round3(item.start),
            to: round3(item.end),
        })
        .collect();
    let frames_total: f64 = picked.iter().map(|item| item.frames).sum();
    // Кадров достаточно, когда материал дошёл до конца интервала (+2 кадра запаса): клип из
    // начала длинного сегмента не ждёт его конца.
    let removed: f64 = cuts
        .iter()
        .filter(|cut| cut.to <= to)
        .map(|cut| cut.to - cut.from)
        .sum();
    let needed = ((to - first_start - removed) * fps).ceil() + 2.0;
    Plan {
        offset: round3(first_start),
        picked,
        cuts,
        frames_total,
        frames_needed: (needed < frames_total).then_some(needed.max(1.0)),
    }
}

/// `clockLabel(seconds)`: «2:05.250».
pub fn clock_label(seconds: f64) -> String {
    let ms = math_round(seconds.max(0.0) * 1000.0);
    let minutes = (ms / 60000.0).floor();
    let rest = (ms % 60000.0) / 1000.0;
    format!("{minutes}:{:0>6}", to_fixed(rest, 3))
}

/// `timelineOf(picked, range)`: метки во времени демо — начало, смены карт и сейвлоуды на
/// стыках отрендеренных сегментов, таймер рана на стыках и в конце (только то, что есть в
/// плейлисте и заголовках).
pub fn timeline_of(picked: &[Picked], range: Option<Range>) -> Vec<Marker> {
    let mut markers = Vec::new();
    for (k, item) in picked.iter().enumerate() {
        let segment = item.segment;
        let map = segment_map(segment);
        let at = round3(if k == 0 {
            item.start.max(range.map_or(0.0, |r| r.from))
        } else {
            item.start
        });
        if k == 0 {
            markers.push(Marker {
                t: at,
                kind: "map",
                label: map.to_owned(),
            });
        } else {
            let changed = !map.is_empty() && map != segment_map(picked[k - 1].segment);
            markers.push(Marker {
                t: at,
                kind: if changed { "map" } else { "saveload" },
                label: if changed {
                    map.to_owned()
                } else {
                    (item.index + 1).to_string()
                },
            });
        }
        if let Some(start) = segment.timer_start().filter(|s| s.is_finite())
            && at == round3(item.start)
        {
            markers.push(Marker {
                t: at,
                kind: "timer",
                label: clock_label(start),
            });
        }
    }
    if let Some(last) = picked.last()
        && let Some(end) = last.segment.timer_end().filter(|e| e.is_finite())
    {
        markers.push(Marker {
            t: round3(last.end),
            kind: "timer",
            label: clock_label(end),
        });
    }
    markers
}

/// `segment.map || segment.playlistMap || ""`.
pub(crate) fn segment_map(segment: &Segment) -> &str {
    match segment.map() {
        "" => segment.playlist_map(),
        map => map,
    }
}

/// Стандартный HUD «Персонализации» при своём конфиге.
pub const STOCK_HUD_CFG: &str = "hlsr-render-hud.cfg";

/// `configArguments(...)`: аргументы игры и файлы `cfg/…` в папке `dir`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConfigPlan {
    pub args: Vec<String>,
    /// Имя от `dir` («cfg/x.cfg», «/») и текст, по порядку записи.
    pub files: Vec<(String, String)>,
}

/// `configArguments({ownConfig, ownHud, dir})`: стандартный конфиг или HUD в рендере Ghosting.
/// Свой конфиг — как всегда: движок читает `cfg/config.cfg` и `cfg/autoexec.cfg` игрока, а
/// autoexec зовёт `hlsr-personalization.cfg`. Стандартный — папка задания впереди путей поиска
/// (`-insert_search_path`) со своими `config.cfg` и `autoexec.cfg` без настроек; свой HUD при
/// нём — `+exec hlsr-personalization.cfg` игрока, стандартный HUD при своём конфиге — значения
/// «Персонализации» по умолчанию после autoexec. `dir` — папка от корня игры, без пробелов.
pub fn config_arguments(own_config: bool, own_hud: bool, dir: &str) -> ConfigPlan {
    if own_config && own_hud {
        return ConfigPlan::default();
    }
    let mut args = vec!["-insert_search_path".to_owned(), dir.to_owned()];
    let mut files = Vec::new();
    if !own_config {
        let empty = "// HLSR: стандартный конфиг для рендера, без настроек игрока.\n";
        files.push(("cfg/config.cfg".to_owned(), empty.to_owned()));
        files.push(("cfg/autoexec.cfg".to_owned(), empty.to_owned()));
        if own_hud {
            args.extend(["+exec".to_owned(), PERSONALIZATION_FILE.to_owned()]);
        }
    } else {
        files.push((
            format!("cfg/{STOCK_HUD_CFG}"),
            cfg_lines(&default_personalization()),
        ));
        args.extend(["+exec".to_owned(), STOCK_HUD_CFG.to_owned()]);
    }
    ConfigPlan { args, files }
}

/// Несжатый TGA игры (тип 2, 24/32 бит): размер, формат и где пиксели.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TgaInfo {
    pub width: u16,
    pub height: u16,
    pub bpp: u8,
    /// `bgr24` или `bgra` (формат FFmpeg).
    pub pix_fmt: &'static str,
    /// Строки сверху вниз; иначе (как пишет игра) — снизу вверх.
    pub top_down: bool,
    pub offset: usize,
    pub bytes: usize,
    /// Файл дописан до конца пикселей.
    pub complete: bool,
}

/// `tgaInfo(buffer)`.
pub fn tga_info(buffer: &[u8]) -> Option<TgaInfo> {
    if buffer.len() < 18 {
        return None;
    }
    let (width, height, bpp) = (u16_at(buffer, 12), u16_at(buffer, 14), buffer[16]);
    if buffer[1] != 0 || buffer[2] != 2 || (bpp != 24 && bpp != 32) || width == 0 || height == 0 {
        return None;
    }
    let offset = 18 + usize::from(buffer[0]);
    let bytes = usize::from(width) * usize::from(height) * usize::from(bpp / 8);
    Some(TgaInfo {
        width,
        height,
        bpp,
        pix_fmt: if bpp == 24 { "bgr24" } else { "bgra" },
        top_down: buffer[17] & 0x20 != 0,
        offset,
        bytes,
        complete: buffer.len() >= offset + bytes,
    })
}

/// `quoteArg(arg)`: аргумент для `CreateProcessW` по правилам `CommandLineToArgvW`.
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.chars().any(|c| c == '"' || is_whitespace(c)) {
        return arg.to_owned();
    }
    let mut out = String::from('"');
    let mut slashes = 0;
    for ch in arg.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        let escaped = if ch == '"' { slashes * 2 + 1 } else { slashes };
        out.push_str(&"\\".repeat(escaped));
        out.push(ch);
        slashes = 0;
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    out
}

/// `commandLine(exe, args)`: строка для `CreateProcessW`.
pub fn command_line(exe: &str, args: &[String]) -> String {
    std::iter::once(exe)
        .chain(args.iter().map(String::as_str))
        .map(quote_arg)
        .collect::<Vec<_>>()
        .join(" ")
}
