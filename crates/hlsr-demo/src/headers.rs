//! Заголовки демо (`headers.js`). Движок определяется по сигнатуре, а не по расширению.
//! Читается только заголовок (и у GoldSrc — директория сегментов по смещению из заголовка),
//! сам поток демо не трогается.
//!
//! GoldSrc (544 байта): `"HLDEMO\0\0"`, demoProtocol i32, netProtocol i32, map[260],
//! gameDir[260], mapCRC i32, directoryOffset i32. Директория: count i32 и записи по 92 байта:
//! type i32, description[64], flags i32, cdTrack i32, trackTime f32, frames i32, offset i32,
//! length i32.
//!
//! Source (1072 байта): `"HL2DEMO\0"`, demoProtocol i32, netProtocol i32, server[260],
//! client[260], map[260], gameDir[260], playbackTime f32, ticks i32, frames i32,
//! signonLength i32.

use crate::errors::is_enoent;
use crate::{f32_at, i32_at, round3};
use hlsr_core::js;
use hlsr_core::json_text::js_number;
use serde_json::{Map, Value, json};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

pub const GOLDSRC_HEADER: usize = 544;
pub const SOURCE_HEADER: usize = 1072;
pub const DIR_ENTRY: usize = 92;
pub const ENGINE_GOLDSRC: &str = "goldsrc";
pub const ENGINE_SOURCE: &str = "source";

const GOLDSRC_MAGIC: &[u8] = b"HLDEMO\0";
const SOURCE_MAGIC: &[u8] = b"HL2DEMO\0";
const MAX_DIR_ENTRIES: i32 = 1024;
const MAX_SECONDS: f64 = 365.0 * 24.0 * 3600.0;

/// Запись директории GoldSrc.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DirEntry {
    pub kind: i32,
    pub description: String,
    pub duration: f64,
    pub frames: i32,
}

/// Поля заголовка Source.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceFields {
    pub server: String,
    pub client: String,
    pub ticks: i32,
    pub frames: i32,
    pub tickrate: Option<f64>,
}

/// Разобранный заголовок (`{ok: true, …}`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DemoHeader {
    pub engine: &'static str,
    pub demo_protocol: i32,
    pub net_protocol: i32,
    pub map: String,
    pub game_dir: String,
    pub size: u64,
    /// `hl1`, `ghosting`, `hl2` или неизвестная игра.
    pub game: Option<&'static str>,
    pub supported: bool,
    pub reason: Option<&'static str>,
    /// `None` — длительность неизвестна (нет директории, незакрытая запись).
    pub duration: Option<f64>,
    /// GoldSrc: директория (пустая, если не сходится с файлом).
    pub entries: Vec<DirEntry>,
    /// Source: поля, которых у GoldSrc нет.
    pub source: Option<SourceFields>,
}

impl DemoHeader {
    pub fn is_goldsrc(&self) -> bool {
        self.engine == ENGINE_GOLDSRC
    }

    pub fn to_value(&self) -> Value {
        let mut out = Map::new();
        out.insert("ok".to_owned(), json!(true));
        out.insert("engine".to_owned(), json!(self.engine));
        out.insert("demoProtocol".to_owned(), json!(self.demo_protocol));
        out.insert("netProtocol".to_owned(), json!(self.net_protocol));
        if let Some(source) = &self.source {
            out.insert("server".to_owned(), json!(source.server));
            out.insert("client".to_owned(), json!(source.client));
        }
        out.insert("map".to_owned(), json!(self.map));
        out.insert("gameDir".to_owned(), json!(self.game_dir));
        out.insert("size".to_owned(), json!(self.size));
        out.insert("game".to_owned(), json!(self.game));
        out.insert("supported".to_owned(), json!(self.supported));
        out.insert("reason".to_owned(), json!(self.reason));
        out.insert("duration".to_owned(), optional_number(self.duration));
        match &self.source {
            Some(source) => {
                out.insert("ticks".to_owned(), json!(source.ticks));
                out.insert("frames".to_owned(), json!(source.frames));
                out.insert("tickrate".to_owned(), optional_number(source.tickrate));
            }
            None => {
                out.insert("entries".to_owned(), entries_value(&self.entries));
            }
        }
        Value::Object(out)
    }
}

pub(crate) fn optional_number(value: Option<f64>) -> Value {
    value.map_or(Value::Null, js_number)
}

pub(crate) fn entries_value(entries: &[DirEntry]) -> Value {
    Value::Array(
        entries
            .iter()
            .map(|entry| {
                json!({"type": entry.kind, "description": entry.description,
                    "duration": js_number(entry.duration), "frames": entry.frames})
            })
            .collect(),
    )
}

/// Результат разбора как в 0.4.7: `{ok: true, …}` или `{ok: false, code}`.
pub fn header_value(result: &Result<DemoHeader, &'static str>) -> Value {
    match result {
        Ok(header) => header.to_value(),
        Err(code) => json!({"ok": false, "code": code}),
    }
}

/// Текст поля фиксированной длины до первого нуля. Управляющие символы (и битый UTF-8) из
/// чужого файла в интерфейс не пропускаются.
pub(crate) fn text(buffer: &[u8], start: usize, length: usize) -> String {
    let raw = &buffer[start.min(buffer.len())..(start + length).min(buffer.len())];
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    let decoded: String = js::utf8_string(&raw[..end])
        .chars()
        .filter(|c| !matches!(c, '\0'..='\x1f' | '\x7f' | '\u{FFFD}'))
        .collect();
    js::trim(&decoded).to_owned()
}

pub(crate) fn sane(value: f64) -> bool {
    value.is_finite() && (0.0..=MAX_SECONDS).contains(&value)
}

fn goldsrc_game(
    game_dir: &str,
    demo_protocol: i32,
) -> (Option<&'static str>, bool, Option<&'static str>) {
    if !game_dir.eq_ignore_ascii_case("valve") && !game_dir.eq_ignore_ascii_case("valve_won") {
        return (None, false, Some("DEMO_UNSUPPORTED"));
    }
    if demo_protocol != 5 {
        return (Some("hl1"), false, Some("DEMO_PROTOCOL_UNSUPPORTED"));
    }
    (Some("hl1"), true, None)
}

fn source_game(
    game_dir: &str,
    demo_protocol: i32,
) -> (Option<&'static str>, bool, Option<&'static str>) {
    let dir = game_dir.to_lowercase();
    if dir == "ghostingmod" {
        return if demo_protocol == 3 {
            (Some("ghosting"), true, None)
        } else {
            (Some("ghosting"), false, Some("DEMO_PROTOCOL_UNSUPPORTED"))
        };
    }
    // Старые HL2 (2707/4260) и прочие моды показываются, но пока без рендера.
    (
        (dir == "hl2").then_some("hl2"),
        false,
        Some("DEMO_UNSUPPORTED"),
    )
}

/// Директория GoldSrc: `None` — директории нет или она не сходится с файлом.
fn goldsrc_directory(
    offset: i32,
    size: u64,
    read: &mut dyn FnMut(u64, usize) -> io::Result<Vec<u8>>,
) -> io::Result<Option<Vec<DirEntry>>> {
    let offset = i64::from(offset);
    let size = size as i64;
    if offset < GOLDSRC_HEADER as i64 || offset + 4 > size {
        return Ok(None);
    }
    let count_buffer = read(offset as u64, 4)?;
    if count_buffer.len() < 4 {
        return Ok(None);
    }
    let count = i32_at(&count_buffer, 0);
    if !(1..=MAX_DIR_ENTRIES).contains(&count)
        || offset + 4 + i64::from(count) * DIR_ENTRY as i64 > size
    {
        return Ok(None);
    }
    let count = count as usize;
    let table = read(offset as u64 + 4, count * DIR_ENTRY)?;
    if table.len() < count * DIR_ENTRY {
        return Ok(None);
    }
    let mut entries = Vec::with_capacity(count);
    let mut spans = Vec::with_capacity(count);
    for i in 0..count {
        let at = i * DIR_ENTRY;
        let duration = f32_at(&table, at + 76);
        let frames = i32_at(&table, at + 80);
        let data_offset = i64::from(i32_at(&table, at + 84));
        let data_length = i64::from(i32_at(&table, at + 88));
        if !sane(duration)
            || frames < 0
            || data_offset < GOLDSRC_HEADER as i64
            || data_length < 0
            || data_offset + data_length > offset
        {
            return Ok(None);
        }
        entries.push(DirEntry {
            kind: i32_at(&table, at),
            description: text(&table, at + 4, 64),
            duration: round3(duration),
            frames,
        });
        spans.push((data_offset, data_offset + data_length));
    }
    // Записи не пересекаются (как в goldsrc_demo): подделка с тысячей записей на один участок
    // не множит длительность.
    spans.sort_unstable();
    if spans.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Ok(None);
    }
    Ok(Some(entries))
}

/// `parseHeader(head, size, read)`: `head` — первые до 1072 байт файла, `size` — его длина,
/// `read(pos, len)` — чтение с позиции (для директории GoldSrc). Ошибка чтения —
/// `DEMO_READ_FAILED`.
pub fn parse_header(
    head: &[u8],
    size: u64,
    mut read: impl FnMut(u64, usize) -> io::Result<Vec<u8>>,
) -> Result<DemoHeader, &'static str> {
    if head.starts_with(GOLDSRC_MAGIC) {
        if head.len() < GOLDSRC_HEADER {
            return Err("DEMO_TRUNCATED");
        }
        let demo_protocol = i32_at(head, 8);
        let game_dir = text(head, 276, 260);
        let entries = goldsrc_directory(i32_at(head, 540), size, &mut read)
            .map_err(|_| "DEMO_READ_FAILED")?;
        let (game, mut supported, mut reason) = goldsrc_game(&game_dir, demo_protocol);
        // Без директории GoldSrc демо не проигрывает (запись оборвалась).
        if entries.is_none() && supported {
            supported = false;
            reason = Some("DEMO_DIRECTORY_INVALID");
        }
        return Ok(DemoHeader {
            engine: ENGINE_GOLDSRC,
            demo_protocol,
            net_protocol: i32_at(head, 12),
            map: text(head, 16, 260),
            game_dir,
            size,
            game,
            supported,
            reason,
            duration: entries
                .as_ref()
                .map(|list| round3(list.iter().map(|entry| entry.duration).sum())),
            entries: entries.unwrap_or_default(),
            source: None,
        });
    }

    if head.starts_with(SOURCE_MAGIC) {
        if head.len() < SOURCE_HEADER {
            return Err("DEMO_TRUNCATED");
        }
        let playback_time = f32_at(head, 1056);
        let ticks = i32_at(head, 1060);
        let frames = i32_at(head, 1064);
        let signon = i32_at(head, 1068);
        if signon < 0 || SOURCE_HEADER as u64 + signon as u64 > size {
            return Err("DEMO_TRUNCATED");
        }
        if !sane(playback_time) || ticks < 0 || frames < 0 {
            return Err("DEMO_HEADER_INVALID");
        }
        let demo_protocol = i32_at(head, 8);
        let game_dir = text(head, 796, 260);
        // Незакрытая демо (вылет игры) хранит нули и ни одного кадра: длительность неизвестна.
        // Закрытый пустой сегмент (мгновенный сейвлоуд) — 0 тиков, 1 кадр.
        let duration = if playback_time > 0.0 {
            Some(round3(playback_time))
        } else if frames > 0 {
            Some(0.0)
        } else {
            None
        };
        let (game, supported, reason) = source_game(&game_dir, demo_protocol);
        return Ok(DemoHeader {
            engine: ENGINE_SOURCE,
            demo_protocol,
            net_protocol: i32_at(head, 12),
            map: text(head, 536, 260),
            game_dir,
            size,
            game,
            supported,
            reason,
            duration,
            entries: Vec::new(),
            source: Some(SourceFields {
                server: text(head, 16, 260),
                client: text(head, 276, 260),
                ticks,
                frames,
                tickrate: duration
                    .filter(|d| *d != 0.0 && ticks != 0)
                    .map(|d| round3(f64::from(ticks) / d)),
            }),
        });
    }

    Err("DEMO_NOT_A_DEMO")
}

fn read_at(file: &mut File, position: u64, length: usize) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(position))?;
    let mut buffer = Vec::with_capacity(length);
    file.take(length as u64).read_to_end(&mut buffer)?;
    Ok(buffer)
}

/// `parseDemoFile(file)`: заголовок файла; читается не больше 1072 байт (и директория GoldSrc).
pub fn parse_demo_file(file: &Path) -> Result<DemoHeader, &'static str> {
    let mut handle = File::open(file).map_err(|error| {
        if is_enoent(&error) {
            "DEMO_NOT_FOUND"
        } else {
            "DEMO_READ_FAILED"
        }
    })?;
    let metadata = handle.metadata().map_err(|_| "DEMO_READ_FAILED")?;
    if !metadata.is_file() {
        return Err("DEMO_READ_FAILED");
    }
    let head = read_at(&mut handle, 0, SOURCE_HEADER).map_err(|_| "DEMO_READ_FAILED")?;
    parse_header(&head, metadata.len(), |position, length| {
        read_at(&mut handle, position, length)
    })
}

/// `parseDemoBuffer(buffer)`.
pub fn parse_demo_buffer(buffer: &[u8]) -> Result<DemoHeader, &'static str> {
    parse_header(
        &buffer[..buffer.len().min(SOURCE_HEADER)],
        buffer.len() as u64,
        |position, length| {
            let start = (position as usize).min(buffer.len());
            Ok(buffer[start..(start + length).min(buffer.len())].to_vec())
        },
    )
}
