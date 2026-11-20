//! Плейлист рана Ghosting (`playlist.js`): `demos/<ран>/run.ghplaylist` или старый плоский
//! `demos/<имя>.ghplaylist` (например `gh_latest`). Формат GHPL v1 из клиента мода
//! (`gh_demo_playlist.h`): `"GHPL"`, version u8 = 1, incomplete u8, count u16, total f32 (время
//! таймера рана); записи: file str16, map str8, start f32, end f32 (таймер на границах
//! сегмента), size u64, SHA-256 64 ASCII. Пути записей — от папки мода
//! (`demos/myrun1/segment001.dem`).

use crate::errors::is_enoent;
use crate::{f32_at, round3, u16_at};
use hlsr_core::js::{self, path::NodePath};
use hlsr_core::json_text::js_number;
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::path::Path;

pub const MAX_PLAYLIST_BYTES: u64 = 1024 * 1024;
const MAX_FILES: u16 = 4096;
const MAX_SECONDS: f64 = 31_536_000.0;

/// Запись плейлиста.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistEntry {
    pub file: String,
    pub map: String,
    pub start: f64,
    pub end: f64,
    pub size: u64,
    pub sha256: String,
}

impl PlaylistEntry {
    fn fields(&self, out: &mut Map<String, Value>) {
        out.insert("file".to_owned(), json!(self.file));
        out.insert("map".to_owned(), json!(self.map));
        out.insert("start".to_owned(), js_number(self.start));
        out.insert("end".to_owned(), js_number(self.end));
        out.insert("size".to_owned(), json!(self.size));
        out.insert("sha256".to_owned(), json!(self.sha256));
    }
}

/// `decodePlaylist` без ошибки.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub incomplete: bool,
    pub total: f64,
    pub entries: Vec<PlaylistEntry>,
}

impl Decoded {
    pub fn to_value(&self) -> Value {
        let entries = self
            .entries
            .iter()
            .map(|entry| {
                let mut out = Map::new();
                entry.fields(&mut out);
                Value::Object(out)
            })
            .collect();
        json!({"ok": true, "incomplete": self.incomplete, "total": js_number(self.total), "entries": Value::Array(entries)})
    }
}

/// Отказ разбора: код и, смотря по коду, причина, номер сегмента (с 1) и его имя.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistError {
    pub code: &'static str,
    pub reason: Option<&'static str>,
    pub segment: Option<usize>,
    pub name: Option<String>,
}

impl PlaylistError {
    fn code(code: &'static str) -> PlaylistError {
        PlaylistError {
            code,
            reason: None,
            segment: None,
            name: None,
        }
    }

    fn invalid(reason: &'static str) -> PlaylistError {
        PlaylistError {
            reason: Some(reason),
            ..PlaylistError::code("PLAYLIST_INVALID")
        }
    }

    pub fn to_value(&self) -> Value {
        let mut out = Map::new();
        out.insert("ok".to_owned(), json!(false));
        out.insert("code".to_owned(), json!(self.code));
        if let Some(reason) = self.reason {
            out.insert("reason".to_owned(), json!(reason));
        }
        if let Some(segment) = self.segment {
            out.insert("segment".to_owned(), json!(segment));
        }
        if let Some(name) = &self.name {
            out.insert("name".to_owned(), json!(name));
        }
        Value::Object(out)
    }
}

fn sane(value: f64) -> bool {
    value.is_finite() && (0.0..=MAX_SECONDS).contains(&value)
}

/// `decodePlaylist(buffer)`; ошибка — причина `PLAYLIST_INVALID`.
pub fn decode_playlist(buffer: &[u8]) -> Result<Decoded, PlaylistError> {
    if buffer.len() < 12 {
        return Err(PlaylistError::invalid("short"));
    }
    if buffer.len() as u64 > MAX_PLAYLIST_BYTES {
        return Err(PlaylistError::invalid("too-large"));
    }
    if &buffer[..4] != b"GHPL" || buffer[4] != 1 || buffer[5] > 1 {
        return Err(PlaylistError::invalid("magic"));
    }
    let count = u16_at(buffer, 6);
    let total = f32_at(buffer, 8);
    if count == 0 || count > MAX_FILES || !sane(total) {
        return Err(PlaylistError::invalid("header"));
    }

    let mut at = 12;
    let mut take = |length: usize| -> Option<&[u8]> {
        let slice = buffer.get(at..at + length)?;
        at += length;
        Some(slice)
    };
    let mut entries = Vec::with_capacity(usize::from(count));
    let mut names = HashSet::new();
    // Порядок сверяется по сырым f32, как в моде; округление — после проверок.
    let mut previous_start = f64::NEG_INFINITY;
    for _ in 0..count {
        let file = take_string(&mut take, true, 160);
        let map = take_string(&mut take, false, 32);
        let numbers = take(16);
        let sha = take(64);
        let (Some(file), Some(map), Some(numbers), Some(sha)) = (file, map, numbers, sha) else {
            return Err(PlaylistError::invalid("entry"));
        };
        let start = f32_at(numbers, 0);
        let end = f32_at(numbers, 4);
        let size = u64::from_le_bytes(numbers[8..16].try_into().expect("8 байт"));
        let map_ok = map
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
            && !map.contains("..");
        let sha_ok = sha.iter().all(u8::is_ascii_hexdigit);
        if !map_ok
            || !sane(start)
            || !sane(end)
            || end < start
            || start < previous_start
            || !(1076..=1 << 40).contains(&size)
            || !sha_ok
        {
            return Err(PlaylistError::invalid("entry"));
        }
        if !names.insert(file.to_lowercase()) {
            return Err(PlaylistError::invalid("duplicate"));
        }
        previous_start = start;
        entries.push(PlaylistEntry {
            file,
            map,
            start: round3(start),
            end: round3(end),
            size,
            sha256: js::latin1_string(sha).to_ascii_lowercase(),
        });
    }
    if at != buffer.len() {
        return Err(PlaylistError::invalid("trailing"));
    }
    Ok(Decoded {
        incomplete: buffer[5] != 0,
        total: round3(total),
        entries,
    })
}

/// Строка с длиной u16 (`wide`) или u8: не пустая, короче `capacity`, без нулей.
fn take_string<'a>(
    take: &mut impl FnMut(usize) -> Option<&'a [u8]>,
    wide: bool,
    capacity: usize,
) -> Option<String> {
    let prefix = take(if wide { 2 } else { 1 })?;
    let length = if wide {
        usize::from(u16_at(prefix, 0))
    } else {
        usize::from(prefix[0])
    };
    if length == 0 || length >= capacity {
        return None;
    }
    let raw = take(length)?;
    (!raw.contains(&0)).then(|| js::utf8_string(raw))
}

/// `segmentLeaf(file)`: сегмент из записи плейлиста — только файл в папке самого плейлиста
/// (`demos/<папка>/segmentNNN.dem` → `segmentNNN.dem`). Обход вверх, абсолютные пути, обратные
/// слеши и не `.dem` — `None`.
pub fn segment_leaf(file: &str) -> Option<String> {
    let rest = file.strip_prefix("demos/")?;
    let leaf = match rest.split_once('/') {
        Some((dir, leaf)) => {
            let dir_ok = (1..=64).contains(&dir.len())
                && dir
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
            if !dir_ok {
                return None;
            }
            leaf
        }
        None => rest,
    };
    let leaf_ok = (1..=149).contains(&leaf.len())
        && leaf
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-');
    if !leaf_ok || file.contains("..") {
        return None;
    }
    let path = NodePath::native();
    (path.extname(leaf).to_lowercase() == ".dem").then(|| leaf.to_owned())
}

/// Сегмент прочитанного плейлиста: запись, имя файла, путь и совпадение размера.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistSegment {
    pub entry: PlaylistEntry,
    pub name: String,
    pub path: String,
    pub size_matches: bool,
}

/// `readPlaylist` без ошибки.
#[derive(Debug, Clone, PartialEq)]
pub struct Playlist {
    pub incomplete: bool,
    pub total: f64,
    pub segments: Vec<PlaylistSegment>,
}

impl Playlist {
    pub fn to_value(&self) -> Value {
        let segments = self
            .segments
            .iter()
            .map(|segment| {
                let mut out = Map::new();
                segment.entry.fields(&mut out);
                out.insert("name".to_owned(), json!(segment.name));
                out.insert("path".to_owned(), json!(segment.path));
                out.insert("sizeMatches".to_owned(), json!(segment.size_matches));
                Value::Object(out)
            })
            .collect();
        json!({"ok": true, "incomplete": self.incomplete, "total": js_number(self.total), "segments": Value::Array(segments)})
    }
}

/// `readPlaylist(file)`: плейлист и его сегменты — обычные файлы рядом с ним.
pub fn read_playlist(file: &str) -> Result<Playlist, PlaylistError> {
    let buffer = match std::fs::metadata(file) {
        Ok(stat) if !stat.is_file() => return Err(PlaylistError::invalid("not-file")),
        Ok(stat) if stat.len() > MAX_PLAYLIST_BYTES => {
            return Err(PlaylistError::invalid("too-large"));
        }
        Ok(_) => js::fs::read_file(Path::new(file)),
        Err(error) => Err(error),
    }
    .map_err(|error| {
        PlaylistError::code(if is_enoent(&error) {
            "DEMO_NOT_FOUND"
        } else {
            "DEMO_READ_FAILED"
        })
    })?;
    let decoded = decode_playlist(&buffer)?;

    let path = NodePath::native();
    let dir = path.dirname(file);
    let mut segments = Vec::with_capacity(decoded.entries.len());
    for (index, entry) in decoded.entries.into_iter().enumerate() {
        let Some(leaf) = segment_leaf(&entry.file) else {
            return Err(PlaylistError {
                segment: Some(index + 1),
                ..PlaylistError::code("PLAYLIST_PATH_ESCAPE")
            });
        };
        let full = path.join(&[&dir, &leaf]);
        let size = std::fs::symlink_metadata(&full)
            .ok()
            .filter(|stat| stat.file_type().is_file())
            .map(|stat| stat.len());
        let Some(size) = size else {
            return Err(PlaylistError {
                segment: Some(index + 1),
                name: Some(leaf),
                ..PlaylistError::code("SEGMENT_MISSING")
            });
        };
        segments.push(PlaylistSegment {
            size_matches: size == entry.size,
            entry,
            name: leaf,
            path: full,
        });
    }
    Ok(Playlist {
        incomplete: decoded.incomplete,
        total: decoded.total,
        segments,
    })
}
