//! Разбор кадров демки GoldSrc (`goldsrc-demo.js`; HLDEMO, протокол демо 5) для таймлайна и
//! KeyHUD: смены карт (svc_serverinfo → «maps/<карта>.bsp» в сетевом сообщении) и кнопки
//! usercmd каждого сетевого кадра. Раскладка кадров — как в hldemo-rs (YaLTeR): u8 тип, f32
//! время, u32 номер; сетевой кадр — DemoInfo 436 байт (время 4 + ref_params 232 + usercmd 52 +
//! movevars 132 + view 12 + viewmodel 4), 28 байт последовательностей, u32 длина и сообщение.
//! Файл читается окнами (память не растёт с размером демки), битый кадр останавливает разбор:
//! возвращается то, что успели прочитать, и `broken`. Файл недоверенный (скачанная демка):
//! записи директории не пересекаются (каждый байт читается один раз), смен карт и нажатий — не
//! больше пределов.

use crate::errors::DemoError;
use crate::headers::sane;
use crate::window::Window;
use crate::{f32_at, i32_at, round3, u16_at};
use hlsr_core::json_text::js_number;
use serde_json::{Value, json};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

const HEADER: u64 = 544;
const DIR_ENTRY: usize = 92;
const MAX_ENTRIES: i32 = 1024;
const DEMO_INFO: usize = 436;
const USERCMD_AT: usize = 4 + 232;
const BUTTONS_AT: usize = USERCMD_AT + 30;
/// Модель оружия в руках (0 — без оружия): после usercmd, movevars 132 и view 12.
const VIEWMODEL_AT: usize = USERCMD_AT + 52 + 132 + 12;
const SEQUENCE_INFO: usize = 28;
/// Сетевое сообщение GoldSrc — до 64 КБ (NET_MAX_PAYLOAD), с запасом.
const MAX_MESSAGE: i32 = 1 << 20;
/// Пределы на демку: в ране HL1 смен карт — десятки, нажатий — десятки тысяч.
pub const MAX_MAP_CHANGES: usize = 256;
pub const MAX_BUTTON_CHANGES: usize = 500_000;
const MAX_FILE: u64 = 2 * 1024 * 1024 * 1024;

/// Биты IN_* usercmd, которые показывает KeyHUD.
pub const BUTTONS: [(&str, u16); 10] = [
    ("attack", 1),
    ("jump", 2),
    ("duck", 4),
    ("forward", 8),
    ("back", 16),
    ("use", 32),
    ("moveleft", 512),
    ("moveright", 1024),
    ("attack2", 2048),
    ("reload", 8192),
];

/// Смена карты: время, карта и пауза загрузки до первого кадра ClientData.
#[derive(Debug, Clone, PartialEq)]
pub struct MapChange {
    pub t: f64,
    pub map: String,
    pub gap: f64,
}

/// Смена состояния кнопок.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Press {
    pub t: f64,
    pub buttons: u16,
}

/// `scanDemo` без ошибки.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GoldsrcScan {
    pub broken: bool,
    pub duration: f64,
    pub map_changes: Vec<MapChange>,
    pub buttons: Vec<Press>,
    pub start_gap: f64,
}

impl GoldsrcScan {
    pub fn to_value(&self) -> Value {
        json!({
            "ok": true,
            "broken": self.broken,
            "duration": js_number(self.duration),
            "mapChanges": self.map_changes.iter().map(|change| json!({"t": js_number(change.t), "map": change.map, "gap": js_number(change.gap)})).collect::<Vec<_>>(),
            "buttons": self.buttons.iter().map(|press| json!({"t": js_number(press.t), "buttons": press.buttons})).collect::<Vec<_>>(),
            "startGap": js_number(self.start_gap),
        })
    }
}

/// Ответ как в 0.4.9: `{ok: true, …}` или `{ok: false, code}` (отмена и сбой чтения там —
/// исключения).
pub fn scan_value(result: &Result<GoldsrcScan, DemoError>) -> Value {
    match result {
        Ok(scan) => scan.to_value(),
        Err(error) => json!({"ok": false, "code": error.code}),
    }
}

struct Entry {
    kind: i32,
    duration: f64,
    offset: u64,
    length: u64,
}

fn read_at(file: &mut File, position: u64, length: usize) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(position))?;
    let mut buffer = Vec::with_capacity(length);
    file.take(length as u64).read_to_end(&mut buffer)?;
    Ok(buffer)
}

/// Директория в конце файла: записи между заголовком и директорией, без пересечений (подделка
/// с тысячей записей на один участок не множит работу), иначе `None`.
fn directory(file: &mut File, size: u64) -> io::Result<Option<Vec<Entry>>> {
    let head = read_at(file, 0, HEADER as usize)?;
    if head.len() < HEADER as usize || &head[..8] != b"HLDEMO\0\0" {
        return Ok(None);
    }
    let offset = i64::from(i32_at(&head, 540));
    if offset < HEADER as i64 || offset + 4 > size as i64 {
        return Ok(None);
    }
    // Файл мог укоротиться после stat: недочитанное — не директория.
    let count_buffer = read_at(file, offset as u64, 4)?;
    if count_buffer.len() < 4 {
        return Ok(None);
    }
    let count = i32_at(&count_buffer, 0);
    if !(1..=MAX_ENTRIES).contains(&count)
        || offset + 4 + i64::from(count) * DIR_ENTRY as i64 > size as i64
    {
        return Ok(None);
    }
    let table = read_at(file, offset as u64 + 4, count as usize * DIR_ENTRY)?;
    if table.len() < count as usize * DIR_ENTRY {
        return Ok(None);
    }
    let mut entries = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let at = i * DIR_ENTRY;
        let duration = f32_at(&table, at + 76);
        let data_offset = i64::from(i32_at(&table, at + 84));
        let length = i64::from(i32_at(&table, at + 88));
        // Тот же предел, что у заголовка: иначе демка в списке поддержана, а скан отказывает.
        if !sane(duration)
            || data_offset < HEADER as i64
            || length < 0
            || data_offset + length > offset
        {
            return Ok(None);
        }
        entries.push(Entry {
            kind: i32_at(&table, at),
            duration,
            offset: data_offset as u64,
            length: length as u64,
        });
    }
    let mut spans: Vec<(u64, u64)> = entries.iter().map(|e| (e.offset, e.length)).collect();
    spans.sort_by_key(|span| span.0);
    if spans
        .windows(2)
        .any(|pair| pair[0].0 + pair[0].1 > pair[1].0)
    {
        return Ok(None);
    }
    Ok(Some(entries))
}

/// Первая «maps/<карта>.bsp» сообщения (`/maps\/([A-Za-z0-9_-]{1,64})\.bsp/`).
fn first_map(message: &[u8]) -> Option<&str> {
    let mut from = 0;
    while let Some(found) = message[from..].windows(5).position(|w| w == b"maps/") {
        let start = from + found + 5;
        let run = message[start..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_' || **b == b'-')
            .count();
        if (1..=64).contains(&run) && message[start + run..].starts_with(b".bsp") {
            return std::str::from_utf8(&message[start..start + run]).ok();
        }
        from = from + found + 1;
    }
    None
}

/// `scanDemo(file, {signal})`. `t` — время кадра в сегменте «Playback» (как длительность в
/// директории); `buttons` — только смены состояния кнопок. Кадры загрузки (LOADING) дают только
/// карты (их время — 0). Для привязки видео bxt-rs ко времени демки
/// (`bxt_cap_skip_non_gameplay_frames` пропускает кадры загрузки): `gap` смены карты — от
/// svc_serverinfo до первого кадра ClientData (тип 4: клиент снова в игре); `start_gap` —
/// начало демки до 7-го кадра, если в руках нет оружия (правило bxt-rs), иначе 0.
///
/// Ошибки: `DEMO_TOO_LARGE`, `DEMO_DIRECTORY_INVALID` (в 0.4.9 — ответ `{ok: false}`),
/// `CANCELLED`, `DEMO_READ_FAILED`.
pub fn scan_demo(file: &Path, cancelled: &dyn Fn() -> bool) -> Result<GoldsrcScan, DemoError> {
    let io = |_: io::Error| DemoError::new("DEMO_READ_FAILED");
    let mut handle = File::open(file).map_err(io)?;
    let size = handle.metadata().map_err(io)?.len();
    if size > MAX_FILE {
        return Err(DemoError::new("DEMO_TOO_LARGE"));
    }
    let entries = if size >= HEADER {
        directory(&mut handle, size).map_err(io)?
    } else {
        None
    };
    let Some(entries) = entries else {
        return Err(DemoError::new("DEMO_DIRECTORY_INVALID"));
    };

    let mut out = GoldsrcScan::default();
    let mut start_gap = None;
    let mut previous = None;
    // Смена карты, которая ждёт кадра ClientData.
    let mut waiting: Option<usize> = None;
    let mut armed = None;
    for entry in &entries {
        let playback = entry.kind != 0;
        if playback {
            out.duration += entry.duration;
        }
        handle.seek(SeekFrom::Start(entry.offset)).map_err(io)?;
        let mut reader = Window::new(&handle, entry.length);
        loop {
            if cancelled() {
                return Err(DemoError::new("CANCELLED"));
            }
            if !reader.need(9).map_err(io)? {
                break;
            }
            let frame = reader.take(9);
            let kind = frame[0];
            let time = f32_at(frame, 1);
            let number = i32_at(frame, 5);
            let t = if playback && time.is_finite() {
                round3(time.max(0.0))
            } else {
                0.0
            };
            if kind == 5 {
                break;
            }
            if playback && start_gap.is_none() && number >= 7 {
                start_gap = Some(if armed == Some(true) { 0.0 } else { t });
            }
            if playback
                && kind == 4
                && let Some(index) = waiting.take()
            {
                let change: &mut MapChange = &mut out.map_changes[index];
                change.gap = round3((t - change.t).max(0.0));
            }
            let skip: i64 = match kind {
                0 | 1 => {
                    if !reader.need(DEMO_INFO + SEQUENCE_INFO + 4).map_err(io)? {
                        out.broken = true;
                        break;
                    }
                    let info = reader.take(DEMO_INFO + SEQUENCE_INFO + 4).to_vec();
                    let length = i32_at(&info, DEMO_INFO + SEQUENCE_INFO);
                    if !(0..=MAX_MESSAGE).contains(&length)
                        || !reader.need(length as usize).map_err(io)?
                    {
                        out.broken = true;
                        break;
                    }
                    // Карта — первая «maps/<карта>.bsp» сообщения (svc_serverinfo и список ресурсов).
                    let found = first_map(reader.take(length as usize)).map(str::to_owned);
                    if let Some(map) = found
                        && out.map_changes.last().is_none_or(|last| last.map != map)
                    {
                        if out.map_changes.len() >= MAX_MAP_CHANGES {
                            out.broken = true;
                        } else {
                            out.map_changes.push(MapChange { t, map, gap: 0.0 });
                            if playback {
                                waiting = Some(out.map_changes.len() - 1);
                            }
                        }
                    }
                    if playback && kind == 1 {
                        if armed.is_none() {
                            armed = Some(i32_at(&info, VIEWMODEL_AT) != 0);
                        }
                        let buttons = u16_at(&info, BUTTONS_AT);
                        if previous != Some(buttons) {
                            if out.buttons.len() >= MAX_BUTTON_CHANGES {
                                out.broken = true;
                            } else {
                                out.buttons.push(Press { t, buttons });
                                previous = Some(buttons);
                            }
                        }
                    }
                    continue;
                }
                2 => 0,
                3 => 64,
                4 => 32,
                6 => 84,
                7 => 8,
                8 => {
                    if !reader.need(8).map_err(io)? {
                        out.broken = true;
                        break;
                    }
                    let sample = i32_at(reader.take(8), 4);
                    if (0..=MAX_MESSAGE).contains(&sample) {
                        i64::from(sample) + 16
                    } else {
                        -1
                    }
                }
                9 => {
                    if !reader.need(4).map_err(io)? {
                        out.broken = true;
                        break;
                    }
                    let length = i32_at(reader.take(4), 0);
                    if (0..=MAX_MESSAGE).contains(&length) {
                        i64::from(length)
                    } else {
                        -1
                    }
                }
                _ => -1,
            };
            if skip < 0 || !reader.skip(skip as u64).map_err(io)? {
                out.broken = true;
                break;
            }
        }
    }
    out.duration = round3(out.duration);
    out.start_gap = start_gap.unwrap_or(0.0);
    Ok(out)
}
