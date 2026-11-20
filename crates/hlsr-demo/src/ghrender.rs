//! Режим рендера мода Ghosting `-ghrender` (`ghrender.js`, протокол журнала `GHRENDER 2`): мод
//! сам проигрывает ран или диапазон его записей (`-ghrender_segments N-M`), отдаёт только кадры
//! рана (`f_NNNN.tga`), звук без сдвига (`audio.wav`) и журнал `ghrender.log` с событиями в
//! номерах выходных кадров. Здесь — чистая логика: умеет ли мод режим, аргументы, разбор
//! журнала, таймлайн и вырезанное во времени демо. Процессы — [`crate::ghosting`].

use crate::errors::DemoError;
use crate::source_render::{Cut, Range, clock_label};
use crate::{Marker, round3};
use hlsr_core::js::number::number_to_string;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{is_whitespace, slice_utf16, to_number, trim};
use hlsr_core::json_text::js_number;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Mutex, PoisonError};

/// Первая строка журнала. Протокол 1 (без диапазона) в выпуски не попал: лаунчер его не
/// принимает.
pub const PROTOCOL: &str = "GHRENDER 2";
/// `client.dll` мода ~5,5 МБ; больше 64 МиБ — не он.
pub const MAX_CLIENT_BYTES: u64 = 64 * 1024 * 1024;
const EDGE_BYTES: u64 = 64 * 1024;

/// Что умеет установленный мод.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Caps {
    pub ghrender: bool,
    /// Диапазон записей (`-ghrender_segments`, протокол 2).
    pub segments: bool,
}

static CAPS_CACHE: Mutex<Option<HashMap<String, Caps>>> = Mutex::new(None);

fn contains(data: &[u8], needle: &[u8]) -> bool {
    data.windows(needle.len()).any(|window| window == needle)
}

/// `clientCaps(modDir)`: по строкам параметров в `bin/client.dll` мода, а не по номеру выпуска
/// (выпуски можно переставить). Кэш по размеру, mtime и хэшу начала и конца: robocopy и 7z
/// сохраняют mtime при замене. Ошибку чтения 0.4.9 считает «не умеет»; здесь она отдельно, чтобы
/// журнал не винил версию мода в занятом или недоступном файле.
pub fn client_caps(mod_dir: &Path) -> std::io::Result<Caps> {
    let file = mod_dir.join("bin").join("client.dll");
    let read = || -> std::io::Result<Caps> {
        let mut handle = fs::File::open(&file)?;
        let meta = handle.metadata()?;
        let size = meta.len();
        if !meta.is_file() || size > MAX_CLIENT_BYTES {
            return Ok(Caps::default());
        }
        let edge = EDGE_BYTES.min(size);
        let mut chunk = vec![0u8; edge as usize];
        let mut hash = Sha256::new();
        handle.read_exact(&mut chunk)?;
        hash.update(&chunk);
        handle.seek(SeekFrom::Start(size - edge))?;
        handle.read_exact(&mut chunk)?;
        hash.update(&chunk);
        let mtime = meta.modified()?;
        let key = format!("{}|{size}|{mtime:?}|{:?}", file.display(), hash.finalize());
        let cached = CAPS_CACHE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|cache| cache.get(&key).copied());
        if let Some(caps) = cached {
            return Ok(caps);
        }
        let mut data = Vec::with_capacity(size as usize);
        handle.seek(SeekFrom::Start(0))?;
        handle.read_to_end(&mut data)?;
        let caps = Caps {
            ghrender: contains(&data, b"-ghrender_out"),
            segments: contains(&data, b"-ghrender_segments"),
        };
        let mut cache = CAPS_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        let cache = cache.get_or_insert_with(HashMap::new);
        if cache.len() >= 8 {
            cache.clear();
        }
        cache.insert(key, caps);
        Ok(caps)
    };
    read()
}

/// Части `[A-Za-z0-9_][A-Za-z0-9_.-]*` без «..», не длиннее `max`.
fn safe_part(part: &str, max: usize) -> bool {
    let mut chars = part.chars();
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    part.len() <= max
        && chars.next().is_some_and(word)
        && chars.all(|c| word(c) || c == '.' || c == '-')
        && !part.contains("..")
}

/// Части `file` после папки `dir` (`path.relative` без подъёмов); `None` — файл не внутри.
fn parts_inside(dir: &str, file: &str) -> Option<Vec<String>> {
    let path = NodePath::native();
    let split = |text: String| -> Vec<String> {
        text.split(['/', '\\'])
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let base = split(path.resolve(&[dir]));
    let target = split(path.resolve(&[file]));
    let same = |a: &String, b: &String| {
        if cfg!(windows) {
            a.to_lowercase() == b.to_lowercase()
        } else {
            a == b
        }
    };
    (target.len() > base.len() && base.iter().zip(&target).all(|(a, b)| same(a, b)))
        .then(|| target[base.len()..].to_vec())
}

/// `playlistArg(modDir, file)`: имя рана для `-ghrender` в тех формах, которые принимает мод
/// (`gh_playdemo`): «demos/<папка>/run.ghplaylist» или «<имя>.ghplaylist» из `demos/`. Части —
/// только `[A-Za-z0-9_.-]` без «..» (в командную строку не попадёт ничего другого). Плейлист вне
/// `demos/` мода — `None`.
pub fn playlist_arg(mod_dir: &Path, file: &str) -> Option<String> {
    let demos = mod_dir.join("demos");
    let parts = parts_inside(&demos.to_string_lossy(), file)?;
    match parts.as_slice() {
        [dir, name] if name.to_lowercase() == "run.ghplaylist" && safe_part(dir, 50) => {
            Some(format!("demos/{dir}/run.ghplaylist"))
        }
        [name] if name.to_ascii_lowercase().ends_with(".ghplaylist") && safe_part(name, 149) => {
            Some(name.clone())
        }
        _ => None,
    }
}

/// `entriesInPlace(arg, segments)`: мод открывает сегменты по путям из записей плейлиста, а
/// лаунчер — рядом с плейлистом. Совпадают, только если папку рана не переименовали и не
/// скопировали под другим именем: каждая запись — «<папка плейлиста>/<имя сегмента>».
/// `segments` — имя сегмента и путь его записи в плейлисте.
pub fn entries_in_place<'a>(
    arg: &str,
    segments: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> bool {
    let dir = match arg.rfind('/') {
        Some(at) => &arg[..=at],
        None => "demos/",
    };
    let mut any = false;
    for (name, file) in segments {
        any = true;
        if file.is_none_or(|file| file.to_lowercase() != format!("{dir}{name}").to_lowercase()) {
            return false;
        }
    }
    any
}

/// `segmentsArg(picked, wholeCount)`: записи плейлиста (номера с 1, пустые тоже считаются — как
/// `entry=` в журнале), которые задевает интервал: «N» или «N-M»; `None` — все. `indexes` —
/// номера выбранных сегментов с 0 по порядку.
pub fn segments_arg(indexes: &[usize], whole: usize) -> Option<String> {
    let (first, last) = (indexes.first()? + 1, indexes.last()? + 1);
    if indexes.len() == whole {
        return None;
    }
    Some(if first == last {
        first.to_string()
    } else {
        format!("{first}-{last}")
    })
}

/// Аргументы игры для `-ghrender`.
#[derive(Debug, Clone, Copy)]
pub struct GameArgs<'a> {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub keyhud: bool,
    pub integrity: bool,
    pub playlist: &'a str,
    /// Папка задания от папки мода.
    pub out: &'a str,
    pub segments: Option<&'a str>,
    /// `configArguments` (стандартный конфиг или HUD игрока).
    pub config: &'a [String],
}

fn flag(value: bool) -> String {
    if value { "1" } else { "0" }.to_owned()
}

/// `gameArguments(...)`.
pub fn game_arguments(a: &GameArgs) -> Vec<String> {
    let mut args: Vec<String> = [
        "-steam",
        "-game",
        "ghostingmod",
        "-windowed",
        "-noborder",
        "-w",
    ]
    .map(str::to_owned)
    .to_vec();
    args.extend([a.width.to_string(), "-h".into(), a.height.to_string()]);
    args.extend(["-novid".into(), "-multirun".into()]);
    args.extend(["-ghrender".into(), a.playlist.to_owned()]);
    args.extend(["-ghrender_out".into(), a.out.to_owned()]);
    args.extend(["-ghrender_fps".into(), a.fps.to_string()]);
    if let Some(segments) = a.segments {
        args.extend(["-ghrender_segments".into(), segments.to_owned()]);
    }
    args.extend(["-ghrender_keyshud".into(), flag(a.keyhud)]);
    args.extend(["-ghrender_intact".into(), flag(a.integrity)]);
    // Консоль рендера — в папке задания, не в журнал игрока (con_logfile из его autoexec).
    args.extend(["+con_logfile".into(), format!("{}/console.log", a.out)]);
    // Громкость попадает в audio.wav; у игрока бывает 0,05.
    args.extend(["+volume".into(), "0.8".into()]);
    // Плавный вид в демке: рывок камеры — не телепорт, интерполяция не сбрасывается.
    for item in ["+demo_avellimit", "99999", "+demo_interplimit", "99999"] {
        args.push(item.to_owned());
    }
    args.extend(a.config.iter().cloned());
    args
}

/// Поля строки журнала «ключ=значение» (повтор ключа заменяет значение, как в объекте JS).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fields(pub Vec<(String, String)>);

impl Fields {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    fn set(&mut self, key: &str, value: &str) {
        match self.0.iter_mut().find(|(name, _)| name == key) {
            Some(slot) => slot.1 = value.to_owned(),
            None => self.0.push((key.to_owned(), value.to_owned())),
        }
    }

    /// `Number(fields[key])`: нет поля — `NaN`.
    pub fn number(&self, key: &str) -> f64 {
        self.get(key).map_or(f64::NAN, to_number)
    }

    pub fn to_value(&self) -> serde_json::Value {
        serde_json::Value::Object(self.0.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
    }
}

/// Строка журнала.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub key: String,
    pub fields: Fields,
    /// Строка без `\r` и пробелов по краям, не длиннее 300 единиц UTF-16.
    pub text: String,
}

/// `parseLine(line)`: «КЛЮЧ имя=значение …»; первая строка — «GHRENDER <версия>». Неизвестные
/// ключи пропускает читатель.
pub fn parse_line(line: &str) -> Option<Line> {
    let text = trim(line.strip_suffix('\r').unwrap_or(line));
    if text.is_empty() {
        return None;
    }
    let mut words = text.split(is_whitespace).filter(|word| !word.is_empty());
    let key = words.next()?.to_owned();
    let mut fields = Fields::default();
    for part in words {
        if let Some(at) = part.find('=').filter(|at| *at > 0) {
            fields.set(&part[..at], &part[at + 1..]);
        }
    }
    Some(Line {
        key,
        fields,
        text: slice_utf16(text, 300),
    })
}

/// `parseInt(value, 10) > 0`: номер записи плейлиста.
fn entry_number(value: Option<&str>) -> Option<f64> {
    let text = value?.trim_start_matches(is_whitespace);
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let digits: String = digits.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || negative {
        return None;
    }
    let n = to_number(&digits);
    (n > 0.0).then_some(n)
}

/// `FAIL code=…` → код лаунчера.
pub fn fail_code(code: &str) -> Option<&'static str> {
    Some(match code {
        "BAD_OPTION" => "BAD_OPTION",
        "PLAYLIST_INVALID" => "PLAYLIST_INVALID",
        "MAP_MISSING" => "MAP_MISSING",
        "SEGMENT_MISSING" => "SEGMENT_MISSING",
        "SEGMENT_CHANGED" => "SEGMENT_CHANGED",
        "SEGMENT_MALFORMED" => "SEGMENT_MALFORMED",
        "SEGMENT_UNSAFE" => "SEGMENT_UNSAFE",
        "DEMO_FAILED" => "DEMO_OPEN_FAILED",
        "CANCELLED" => "GAME_ERROR",
        "WRITE_FAILED" | "NO_FRAMES" | "INTERNAL" => "RENDER_FAILED",
        _ => return None,
    })
}

/// `failError(parsed)`: код лаунчера; у сегментов — номер записи плейлиста (с 1).
pub fn fail_error(line: &Line) -> DemoError {
    let code = line
        .fields
        .get("code")
        .and_then(fail_code)
        .unwrap_or("RENDER_FAILED");
    let mut error = DemoError {
        message: line.text.clone(),
        ..DemoError::new(code)
    };
    if code.starts_with("SEGMENT_")
        && let Some(segment) = entry_number(line.fields.get("entry"))
    {
        error = error.with_details(json!({"segment": js_number(segment)}));
    }
    error
}

/// Таймлайн из журнала во времени демо (`jobs` переводит его во время видео).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Timeline {
    pub offset: f64,
    pub cuts: Vec<Cut>,
    pub markers: Vec<Marker>,
}

struct Anchor<'a> {
    m: f64,
    d: f64,
    line: &'a Fields,
    entry: f64,
}

/// `timelineOf({segments, timers, frames, fps, starts, total})`. Время видео — кадр/fps; время
/// демо (как в `library.describe`: сумма длительностей сегментов) — по записи плейлиста. Мод
/// отбрасывает начало сегмента, пока таймер рана не пошёл: первый кадр сегмента — начало
/// записи плюс (timer − start). Между показанными сегментами всё лишнее (отброшенные кадры, пустые
/// записи) уходит в `cuts`; хвост после последнего кадра — тоже, чтобы конец интервала не ушёл за
/// видео.
pub fn timeline_of(
    segments: &[Fields],
    timers: &[Fields],
    frames: f64,
    fps: f64,
    starts: &[f64],
    total: f64,
) -> Timeline {
    let mut anchors: Vec<Anchor> = Vec::new();
    for line in segments {
        let Some(entry) = entry_number(line.get("entry")) else {
            continue;
        };
        let demo_start = starts.get(entry as usize - 1).copied();
        let frame = line.number("frame");
        let Some(demo_start) = demo_start.filter(|s| s.is_finite()) else {
            continue;
        };
        // `!(frame >= 0)`: NaN тоже мимо.
        if frame.is_nan() || frame < 0.0 {
            continue;
        }
        let (timer, start) = (line.number("timer"), line.number("start"));
        let gate = if timer.is_finite() && start.is_finite() {
            (timer - start).max(0.0)
        } else {
            0.0
        };
        let m = frame / fps;
        // Мод оставил больше кадров, чем длилась запись: время демо не идёт назад.
        let floor = anchors.last().map_or(0.0, |prev| prev.d + (m - prev.m));
        let d = (demo_start + gate).max(floor);
        anchors.push(Anchor { m, d, line, entry });
    }
    let Some(first) = anchors.first() else {
        return Timeline::default();
    };
    let demo_at = |m: f64| {
        let mut anchor = first;
        for item in &anchors {
            if item.m <= m {
                anchor = item;
            }
        }
        anchor.d + (m - anchor.m)
    };
    let mut cuts = Vec::new();
    for pair in anchors.windows(2) {
        let from = pair[0].d + (pair[1].m - pair[0].m);
        if pair[1].d - from > 0.0005 {
            cuts.push(Cut {
                from: round3(from),
                to: round3(pair[1].d),
            });
        }
    }
    let end = demo_at(frames / fps);
    if total.is_finite() && total - end > 0.0005 {
        cuts.push(Cut {
            from: round3(end),
            to: round3(total),
        });
    }

    let mut markers = Vec::new();
    let mut timer_at: Vec<f64> = Vec::new();
    let marker = |t: f64, kind: &'static str, label: String| Marker { t, kind, label };
    for (k, anchor) in anchors.iter().enumerate() {
        let t = round3(anchor.d);
        let line = anchor.line;
        let map = || line.get("map").unwrap_or_default().to_owned();
        markers.push(if k > 0 && line.get("kind") == Some("load") {
            marker(t, "saveload", number_to_string(anchor.entry))
        } else {
            marker(t, "map", map())
        });
        let timer = line.number("timer");
        let shown = if timer.is_finite() {
            timer
        } else {
            line.number("start")
        };
        if shown.is_finite() {
            markers.push(marker(t, "timer", clock_label(shown)));
            timer_at.push(t);
        }
    }
    for line in timers {
        // Финиш: таймер в записи мода уже остановлен (0) — метку финиша ставит source_demo по
        // RUN_END.
        if line.get("event") == Some("finish") {
            continue;
        }
        let t = round3(demo_at(line.number("frame") / fps));
        let value = line.number("timer");
        if t.is_finite() && value.is_finite() && !timer_at.contains(&t) {
            markers.push(marker(t, "timer", clock_label(value)));
        }
    }
    sort_markers(&mut markers);
    Timeline {
        offset: round3(first.d),
        cuts,
        markers,
    }
}

/// `withRangeStart(markers, range)`: интервал начинается внутри карты — метка карты в его
/// начале, иначе `jobs` отбросит её вместе с началом.
pub fn with_range_start(markers: Vec<Marker>, range: Option<Range>) -> Vec<Marker> {
    let Some(from) = range.map(|r| r.from).filter(|from| *from > 0.0) else {
        return markers;
    };
    let mut label = None;
    for marker in &markers {
        if marker.kind == "map" && marker.t <= from + 1e-6 {
            label = (marker.t < from - 1e-6).then(|| marker.label.clone());
        }
    }
    let Some(label) = label else {
        return markers;
    };
    let mut out = vec![Marker {
        t: round3(from),
        kind: "map",
        label,
    }];
    out.extend(markers);
    sort_markers(&mut out);
    out
}

/// `sort((a, b) => a.t - b.t)`: устойчиво, −0 и 0 равны.
fn sort_markers(markers: &mut [Marker]) {
    markers.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
}
