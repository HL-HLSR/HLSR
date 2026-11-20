//! Главы HL2 и финиш рана Ghosting (`source-demo.js`) — метки из карт сегментов и из самих
//! демок.
//!
//! Главы: карты `chapter*.cfg` игры (`ghostingmod/cfg`; HL2 — те же), названия — в окне. Глава
//! ставится на первом сегменте с её картой; повторный заход на ту же карту (сейвлоуд) новой
//! метки не даёт. Титры (chapter14, credits) — не глава.
//!
//! Финиш: данные мода в демке (GHRD v1, gh2013 `tools/demo-recording/GHRD_SPEC.md`) — команды
//! `_ghrd <base64url>` в dem_consolecmd. На триггере финиша (d3_breen_01) мод останавливает
//! таймер и в том же кадре пишет запись TIMER с event 2 (run finish), следом RUN_END с
//! finalTime. Таймер в TIMER уже остановлен и читается нулём, поэтому время финиша — finalTime
//! из RUN_END. Остановка рана без финиша (gh_stop, выход) пишет только RUN_END — метки нет.
//! Подпись MAC не проверяется: метка не вердикт о ране.
//!
//! Формат кадров — протокол демо 3 (Source 2007/2013 SP, Ghosting): cmd u8, tick i32;
//! packet/signon — cmdinfo 76 байт, seqIn, seqOut, длина и данные; consolecmd, datatables,
//! stringtables — длина и данные; usercmd — номер, длина и данные.
//!
//! Строже 0.4.9: файл читается окнами по 1 МБ, а не целиком, и команда консоли длиннее 1 МБ
//! пропускается, не читаясь в память.

use crate::errors::DemoError;
use crate::headers::{SOURCE_HEADER, optional_number, sane};
use crate::source_render::{Picked, segment_map};
use crate::window::Window;
use crate::{Marker, f32_at, i32_at, round3, u16_at};
use hlsr_core::js::{self, math_round, to_fixed};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// Карта → номер главы.
pub const CHAPTERS: [(&str, &str); 14] = [
    ("d1_trainstation_01", "1"),
    ("d1_trainstation_05", "2"),
    ("d1_canals_01", "3"),
    ("d1_canals_06", "4"),
    ("d1_eli_01", "5"),
    ("d1_town_01", "6"),
    ("d2_coast_01", "7"),
    ("d2_coast_09", "8"),
    ("d2_prison_02", "9"),
    ("d2_prison_06", "9a"),
    ("d3_c17_02", "10"),
    ("d3_c17_09", "11"),
    ("d3_citadel_01", "12"),
    ("d3_breen_01", "13"),
];

pub const PROTOCOL: i32 = 3;
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_COMMAND: usize = 1 << 20;
const CMDINFO: u64 = 76;
const SIGNON: u8 = 1;
const PACKET: u8 = 2;
const SYNCTICK: u8 = 3;
const CONSOLECMD: u8 = 4;
const USERCMD: u8 = 5;
const DATATABLES: u8 = 6;
const STOP: u8 = 7;
const STRINGTABLES: u8 = 8;
const GHRD_VERSION: u8 = 1;
const GHRD_CHUNK_HEADER: usize = 7;
const GHRD_MAX_PARTS: u8 = 32;
const GHRD_TIMER: u8 = 0x07;
const GHRD_RUN_END: u8 = 0x08;
const GHRD_FINISH: u8 = 2;
/// Незаконченные записи (части ещё не пришли) — не больше этого, остальное отбрасывается.
const MAX_PENDING: usize = 64;
/// И не больше этого байт (строже 0.4.9): поддельная демка не держит в памяти весь файл.
const MAX_PENDING_BYTES: usize = 4 << 20;

/// Команда консоли демки.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub tick: i32,
    pub text: String,
}

/// Кадры после заголовка: команды консоли в `on_command` (байты до первого нуля).
/// Возвращает `broken` — файл оборвался или кадр не разобран.
fn walk<R: Read>(
    frames: &mut Window<R>,
    cancelled: &dyn Fn() -> bool,
    mut on_command: impl FnMut(i32, &[u8]),
) -> Result<bool, DemoError> {
    let io = |_: io::Error| DemoError::new("DEMO_READ_FAILED");
    loop {
        if cancelled() {
            return Err(DemoError::new("CANCELLED"));
        }
        if !frames.need(1).map_err(io)? {
            return Ok(true);
        }
        let cmd = frames.peek();
        // dem_stop в конце файла бывает без полного тика.
        if cmd == STOP {
            return Ok(false);
        }
        if !frames.need(5).map_err(io)? {
            return Ok(true);
        }
        let tick = i32_at(frames.take(5), 1);
        let skip = match cmd {
            SYNCTICK => continue,
            SIGNON | PACKET => CMDINFO + 8,
            USERCMD => 4,
            CONSOLECMD | DATATABLES | STRINGTABLES => 0,
            _ => return Ok(true),
        };
        if !frames.skip(skip).map_err(io)? || !frames.need(4).map_err(io)? {
            return Ok(true);
        }
        let length = i32_at(frames.take(4), 0);
        let Ok(length) = usize::try_from(length) else {
            return Ok(true);
        };
        if cmd == CONSOLECMD && length <= MAX_COMMAND {
            if !frames.need(length).map_err(io)? {
                return Ok(true);
            }
            let text = frames.take(length);
            let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
            on_command(tick, &text[..end]);
        } else if !frames.skip(length as u64).map_err(io)? {
            return Ok(true);
        }
    }
}

fn frames_of(buffer: &[u8]) -> Window<&[u8]> {
    let body = &buffer[SOURCE_HEADER.min(buffer.len())..];
    Window::new(body, body.len() as u64)
}

/// `consoleCommands(buffer)`: команды консоли демки по порядку и `broken` — файл оборвался.
pub fn console_commands(buffer: &[u8]) -> (Vec<Command>, bool) {
    let mut commands = Vec::new();
    let broken = walk(&mut frames_of(buffer), &|| false, |tick, text| {
        commands.push(Command {
            tick,
            text: js::latin1_string(text),
        });
    })
    .unwrap_or(true);
    (commands, broken)
}

/// Запись GHRD: тик последней части, тип и тело.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub tick: i32,
    pub kind: u8,
    pub body: Vec<u8>,
}

struct Pending {
    seq: u32,
    parts: u8,
    chunks: Vec<Option<Vec<u8>>>,
}

impl Pending {
    fn bytes(&self) -> usize {
        self.chunks.iter().flatten().map(Vec::len).sum()
    }
}

/// Сборка записей GHRD из команд `_ghrd`: части собираются по (seq, part).
#[derive(Default)]
struct Ghrd {
    pending: Vec<Pending>,
    /// Байт частей в `pending`.
    bytes: usize,
}

impl Ghrd {
    fn drop_oldest(&mut self) {
        let dropped = self.pending.remove(0);
        self.bytes -= dropped.bytes();
    }

    fn push(&mut self, tick: i32, text: &[u8]) -> Option<Record> {
        let payload = text.strip_prefix(b"_ghrd ")?;
        // Пробелы по краям (trim) декодер base64 всё равно пропускает.
        let chunk = js::buffer_from_base64(&js::latin1_string(payload));
        if chunk.len() <= GHRD_CHUNK_HEADER || chunk[0] != GHRD_VERSION {
            return None;
        }
        let seq = u32::from_le_bytes(chunk[1..5].try_into().expect("4 байта"));
        let (part, parts) = (chunk[5], chunk[6]);
        if parts == 0 || parts > GHRD_MAX_PARTS || part >= parts {
            return None;
        }
        let index = match self.pending.iter().position(|p| p.seq == seq) {
            Some(index) => index,
            None => {
                if self.pending.len() >= MAX_PENDING {
                    self.drop_oldest();
                }
                self.pending.push(Pending {
                    seq,
                    parts,
                    chunks: vec![None; usize::from(parts)],
                });
                self.pending.len() - 1
            }
        };
        let entry = &mut self.pending[index];
        if entry.parts != parts {
            return None;
        }
        let data = chunk[GHRD_CHUNK_HEADER..].to_vec();
        self.bytes += data.len();
        if let Some(old) = entry.chunks[usize::from(part)].replace(data) {
            self.bytes -= old.len();
        }
        if entry.chunks.iter().any(Option::is_none) {
            // Сверх лимита — старейшие незаконченные (и эта, если не влезает одна).
            while self.bytes > MAX_PENDING_BYTES {
                self.drop_oldest();
            }
            return None;
        }
        let done = self.pending.remove(index);
        self.bytes -= done.bytes();
        let record: Vec<u8> = done.chunks.into_iter().flatten().flatten().collect();
        if record.len() < 3 {
            return None;
        }
        let length = usize::from(u16_at(&record, 1));
        let body = record.get(3..3 + length)?.to_vec();
        Some(Record {
            tick,
            kind: record[0],
            body,
        })
    }
}

/// `ghrdRecords(commands)`: записи GHRD по порядку прихода последней части.
pub fn ghrd_records(commands: &[Command]) -> Vec<Record> {
    let mut ghrd = Ghrd::default();
    commands
        .iter()
        .filter_map(|command| ghrd.push(command.tick, &js::latin1_bytes(&command.text)))
        .collect()
}

/// Финиш рана: тик записи TIMER finish и время таймера (`None` — неизвестно).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Finish {
    pub tick: i32,
    pub time: Option<f64>,
}

/// `scanFinish(...)`: `{ok, finish, broken}`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FinishScan {
    pub ok: bool,
    pub finish: Option<Finish>,
    pub broken: bool,
}

impl FinishScan {
    pub fn to_value(&self) -> Value {
        json!({
            "ok": self.ok,
            "finish": self.finish.map_or(Value::Null, |finish| json!({"tick": finish.tick, "time": optional_number(finish.time)})),
            "broken": self.broken,
        })
    }
}

/// Финиш по записям GHRD: последняя TIMER finish, время — из RUN_END после неё.
fn on_record(finish: &mut Option<Finish>, record: &Record) {
    if record.kind == GHRD_TIMER && record.body.len() >= 9 && record.body[4] == GHRD_FINISH {
        let timer = f32_at(&record.body, 5);
        *finish = Some(Finish {
            tick: record.tick,
            // Строже 0.4.9: бесконечность и время больше года — не время.
            time: (timer > 0.0 && sane(timer)).then(|| round3(timer)),
        });
    } else if record.kind == GHRD_RUN_END
        && let Some(finish) = finish
        && record.body.len() >= 8
    {
        let last = f32_at(&record.body, 4);
        if last > 0.0 && sane(last) {
            finish.time = Some(round3(last));
        }
    }
}

/// Проверка заголовка: `None` — кадры читать, иначе готовый ответ.
fn check_head(head: &[u8]) -> Option<FinishScan> {
    if head.len() < SOURCE_HEADER || &head[..8] != b"HL2DEMO\0" {
        return Some(FinishScan {
            ok: false,
            finish: None,
            broken: true,
        });
    }
    (i32_at(head, 8) != PROTOCOL).then_some(FinishScan {
        ok: false,
        finish: None,
        broken: false,
    })
}

fn scan_frames<R: Read>(
    frames: &mut Window<R>,
    cancelled: &dyn Fn() -> bool,
) -> Result<FinishScan, DemoError> {
    let mut ghrd = Ghrd::default();
    let mut finish = None;
    let broken = walk(frames, cancelled, |tick, text| {
        if let Some(record) = ghrd.push(tick, text) {
            on_record(&mut finish, &record);
        }
    })?;
    Ok(FinishScan {
        ok: true,
        finish,
        broken,
    })
}

/// `scanFinish(buffer)`: `buffer` — весь файл демки.
pub fn scan_finish(buffer: &[u8]) -> FinishScan {
    if let Some(early) = check_head(buffer) {
        return early;
    }
    scan_frames(&mut frames_of(buffer), &|| false).unwrap_or(FinishScan {
        ok: true,
        finish: None,
        broken: true,
    })
}

/// `scanFile(file, {signal})`: то же по файлу, окнами. Файл больше 256 МБ — `DEMO_TOO_LARGE`,
/// отмена — `CANCELLED`, сбой чтения — `DEMO_READ_FAILED`.
pub fn scan_file(file: &Path, cancelled: &dyn Fn() -> bool) -> Result<FinishScan, DemoError> {
    let io = |_: io::Error| DemoError::new("DEMO_READ_FAILED");
    let mut handle = File::open(file).map_err(io)?;
    let metadata = handle.metadata().map_err(io)?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return Err(DemoError::new("DEMO_TOO_LARGE"));
    }
    let mut head = Vec::with_capacity(SOURCE_HEADER);
    (&mut handle)
        .take(SOURCE_HEADER as u64)
        .read_to_end(&mut head)
        .map_err(io)?;
    if let Some(early) = check_head(&head) {
        return Ok(early);
    }
    handle
        .seek(SeekFrom::Start(SOURCE_HEADER as u64))
        .map_err(io)?;
    let length = metadata.len() - SOURCE_HEADER as u64;
    scan_frames(&mut Window::new(&handle, length), cancelled)
}

/// `runClock(seconds)`: «1:23:45.678» или «3:05.250» — время таймера рана.
pub fn run_clock(seconds: f64) -> String {
    let ms = math_round(seconds.max(0.0) * 1000.0);
    let hours = (ms / 3_600_000.0).floor();
    let minutes = ((ms % 3_600_000.0) / 60000.0).floor();
    let rest = format!("{:0>6}", to_fixed((ms % 60000.0) / 1000.0, 3));
    if hours > 0.0 {
        format!("{hours}:{minutes:0>2}:{rest}")
    } else {
        format!("{minutes}:{rest}")
    }
}

/// `chapterMarkers(picked)`: метки глав во времени демо, `label` — номер главы.
pub fn chapter_markers(picked: &[Picked]) -> Vec<Marker> {
    let mut seen = HashSet::new();
    let mut markers = Vec::new();
    for item in picked {
        let map = segment_map(item.segment).to_lowercase();
        let Some((_, id)) = CHAPTERS.iter().find(|(chapter, _)| *chapter == map) else {
            continue;
        };
        if !seen.insert(*id) {
            continue;
        }
        markers.push(Marker {
            t: round3(item.start),
            kind: "chapter",
            label: (*id).to_owned(),
        });
    }
    markers
}

/// Что известно о ране для метки финиша.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FinishOptions {
    /// Ран не закончен (`incomplete` плейлиста) — метки нет.
    pub incomplete: bool,
    /// Время таймера из плейлиста — если в записи финиша его нет.
    pub run_time: Option<f64>,
}

/// `finishMarkers(picked, scans, options)`: метка финиша на тике записи TIMER finish в своём
/// сегменте (по тикрейту заголовка), подпись — время таймера.
pub fn finish_markers(
    picked: &[Picked],
    scans: &[Option<FinishScan>],
    options: FinishOptions,
) -> Vec<Marker> {
    if options.incomplete {
        return Vec::new();
    }
    for (k, item) in picked.iter().enumerate().rev() {
        let Some(Some(scan)) = scans.get(k) else {
            continue;
        };
        let Some(finish) = scan.finish.filter(|_| scan.ok) else {
            continue;
        };
        let rate = item
            .segment
            .tickrate()
            .filter(|rate| *rate > 0.0)
            .unwrap_or(100.0);
        let Some(time) = finish.time.or(options.run_time).filter(|time| *time > 0.0) else {
            return Vec::new();
        };
        let t = item
            .end
            .min(item.start + f64::from(finish.tick.max(0)) / rate);
        return vec![Marker {
            t: round3(t),
            kind: "finish",
            label: run_clock(time),
        }];
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base64(bytes: &[u8]) -> String {
        const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for group in bytes.chunks(3) {
            let n = group
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
            for i in 0..=group.len() {
                out.push(char::from(ABC[(n >> (18 - 6 * i) & 63) as usize]));
            }
        }
        out
    }

    fn command(seq: u32, part: u8, parts: u8, data: &[u8]) -> Vec<u8> {
        let mut chunk = vec![GHRD_VERSION];
        chunk.extend(seq.to_le_bytes());
        chunk.extend([part, parts]);
        chunk.extend(data);
        format!("_ghrd {}", base64(&chunk)).into_bytes()
    }

    #[test]
    fn pending_bytes_capped() {
        let mut ghrd = Ghrd::default();
        let big = vec![0u8; 1 << 20];
        for seq in 0..8 {
            assert_eq!(ghrd.push(0, &command(seq, 0, 2, &big)), None);
        }
        assert!(ghrd.bytes <= MAX_PENDING_BYTES);
        assert!(ghrd.pending.len() < 8);
        let record = ghrd
            .push(5, &command(7, 1, 2, &[0]))
            .expect("последний набор цел");
        assert_eq!((record.tick, record.kind, record.body.len()), (5, 0, 0));
        let held: usize = ghrd.pending.iter().map(Pending::bytes).sum();
        assert_eq!(ghrd.bytes, held);
    }

    #[test]
    fn finish_time_sane() {
        let mut timer = vec![0u8; 9];
        timer[4] = GHRD_FINISH;
        timer[5..].copy_from_slice(&f32::INFINITY.to_le_bytes());
        let mut finish = None;
        on_record(
            &mut finish,
            &Record {
                tick: 3,
                kind: GHRD_TIMER,
                body: timer,
            },
        );
        assert_eq!(
            finish,
            Some(Finish {
                tick: 3,
                time: None
            })
        );
        let mut end = vec![0u8; 8];
        end[4..].copy_from_slice(&f32::MAX.to_le_bytes());
        on_record(
            &mut finish,
            &Record {
                tick: 4,
                kind: GHRD_RUN_END,
                body: end,
            },
        );
        assert_eq!(finish.and_then(|f| f.time), None);
    }
}
