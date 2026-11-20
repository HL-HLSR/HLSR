//! Список демо (`library.js`): из установленных игр (`demo_archive`) и добавленные вручную
//! (хранятся в Store путями; несуществующие отбрасываются при чтении). Окно ссылается на
//! элементы по id, пути из окна принимаются только при добавлении и проверяются здесь же.

use crate::headers::{self, DemoHeader, entries_value, optional_number};
use crate::playlist;
use crate::source_demo;
use crate::source_render;
use crate::{Marker, markers_value, round3};
use hlsr_core::js::collate::default_locale;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform, cmp_utf16};
use hlsr_core::json_text::js_number;
use hlsr_delivery::ansi_name::readable_name;
use hlsr_delivery::demo_archive::{self, Demo};
use hlsr_delivery::manifests::LocalConfig;
use hlsr_delivery::paths::assert_inside_with;
use icu_collator::options::{CollatorOptions, Strength};
use icu_collator::preferences::CollationNumericOrdering;
use icu_collator::{Collator, CollatorBorrowed, CollatorPreferences};
use icu_locale_core::Locale;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;

pub const EXTENSIONS: [&str; 2] = [".dem", ".ghplaylist"];
pub const MAX_MANUAL: usize = 200;
const PLAYLIST_FILE: &str = "run.ghplaylist";
const MAX_SEGMENTS: usize = 512;
const MAX_ADD: usize = 512;
const MAX_SCAN: usize = 5000;
const MAX_MISSING: usize = 64;
/// Серии демок HL1 в папке игры: `<имя>_<N>.dem` (bxt_autorecord пишет по демке на загрузку).
/// Ghosting сюда не входит: у его ранов свои плейлисты.
const SERIES_GAMES: [&str; 1] = ["hl1-won"];

/// `idOf(...parts)`: первые 16 знаков SHA-256 частей через `\0`.
pub fn id_of(parts: &[&str]) -> String {
    let digest = Sha256::digest(parts.join("\0").as_bytes());
    js::text::hex(&digest)[..16].to_owned()
}

/// `new Intl.Collator(undefined, { numeric: true, sensitivity: "base" })`.
fn collator() -> &'static CollatorBorrowed<'static> {
    static COLLATOR: OnceLock<CollatorBorrowed<'static>> = OnceLock::new();
    COLLATOR.get_or_init(|| {
        let locale: Locale = default_locale().parse().unwrap_or(Locale::UNKNOWN);
        let mut preferences = CollatorPreferences::from(&locale);
        preferences.numeric_ordering = Some(CollationNumericOrdering::True);
        let mut options = CollatorOptions::default();
        options.strength = Some(Strength::Primary);
        Collator::try_new(preferences, options).expect("данные сравнения вшиты в сборку")
    })
}

/// `naturalByName(a, b)`: по имени файла в естественном порядке (2 раньше 10), затем по пути.
pub fn natural_by_name(a: &str, b: &str) -> Ordering {
    let path = NodePath::native();
    collator()
        .compare(&path.basename(a), &path.basename(b))
        .then_with(|| collator().compare(a, b))
}

/// Демо игры для списка: какая игра, где она и её настройки.
#[derive(Debug, Clone)]
pub struct GameContext<'a> {
    pub game_id: String,
    pub install_path: String,
    pub config: &'a LocalConfig,
}

/// Элемент списка (до разбора).
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    /// `demo`, `segments`, `playlist`.
    pub kind: &'static str,
    /// `game` или `manual`.
    pub source: &'static str,
    pub game_id: Option<String>,
    pub name: String,
    pub files: Vec<String>,
    pub mtime_ms: f64,
    /// Серия HL1 из папки игры: номера, которых нет.
    pub series_missing: Option<Vec<u32>>,
}

impl Entry {
    fn new(
        kind: &'static str,
        source: &'static str,
        game_id: Option<&str>,
        name: &str,
        files: Vec<String>,
        mtime_ms: f64,
    ) -> Entry {
        let key = if kind == "segments" {
            let mut sorted = files.clone();
            sorted.sort_by(|a, b| cmp_utf16(a, b));
            sorted.join("|")
        } else {
            files.join("|")
        };
        Entry {
            id: id_of(&[source, game_id.unwrap_or(""), kind, &key.to_lowercase()]),
            kind,
            source,
            game_id: game_id.map(str::to_owned),
            // name — только для показа: имя от игры в UTF-8, записанное через cp1251, читается.
            name: readable_name(name),
            files,
            mtime_ms,
            series_missing: None,
        }
    }

    fn fields(&self, out: &mut Map<String, Value>) {
        out.insert("id".to_owned(), json!(self.id));
        out.insert("kind".to_owned(), json!(self.kind));
        out.insert("source".to_owned(), json!(self.source));
        out.insert("gameId".to_owned(), json!(self.game_id));
        out.insert("name".to_owned(), json!(self.name));
        out.insert("files".to_owned(), json!(self.files));
        out.insert("mtimeMs".to_owned(), js_number(self.mtime_ms));
        if let Some(missing) = &self.series_missing {
            out.insert("series".to_owned(), json!(true));
            out.insert("missing".to_owned(), json!(missing));
        }
    }

    pub fn to_value(&self) -> Value {
        let mut out = Map::new();
        self.fields(&mut out);
        Value::Object(out)
    }
}

fn is_regular_file(file: &str) -> bool {
    std::fs::symlink_metadata(file).is_ok_and(|stat| stat.file_type().is_file())
}

fn mtime_of(files: &[String]) -> f64 {
    files
        .iter()
        .filter_map(|file| std::fs::metadata(file).ok())
        .filter_map(|stat| js::fs::mtime_ms(&stat).ok())
        .fold(0.0, f64::max)
}

fn basename_without_ext(path: &NodePath, file: &str) -> String {
    let name = path.basename(file);
    let ext = path.extname(file);
    match name.strip_suffix(ext.as_str()) {
        Some(stem) if !ext.is_empty() && !stem.is_empty() => stem.to_owned(),
        _ => name,
    }
}

/// Плейлист Ghosting в папке игры: файл и имя рана.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundPlaylist {
    pub file: String,
    pub name: String,
}

/// `scanPlaylists(context)`: `<demoDir>/<ран>/run.ghplaylist` и плоские `<demoDir>/*.ghplaylist`.
pub fn scan_playlists(context: &GameContext) -> Vec<FoundPlaylist> {
    let path = NodePath::native();
    let mut found = Vec::new();
    for relative in context.config.demo_dirs {
        let mut parts = vec![context.install_path.as_str()];
        parts.extend(relative.split('/'));
        let Ok(dir) = assert_inside_with(&path, &context.install_path, &path.join(&parts), None)
        else {
            continue;
        };
        let Ok(items) = js::fs::read_dir(Path::new(&dir)) else {
            continue;
        };
        for item in items.into_iter().take(MAX_SCAN) {
            if item.file_type.is_symlink() {
                continue;
            }
            let full = path.join(&[&dir, &item.name]);
            if item.file_type.is_file() && path.extname(&item.name).to_lowercase() == ".ghplaylist"
            {
                found.push(FoundPlaylist {
                    name: basename_without_ext(&path, &item.name),
                    file: full,
                });
            } else if item.file_type.is_dir() {
                let file = path.join(&[&full, PLAYLIST_FILE]);
                if is_regular_file(&file) {
                    found.push(FoundPlaylist {
                        file,
                        name: item.name,
                    });
                }
            }
        }
    }
    found
}

/// Демо серии с номером.
#[derive(Debug, Clone, PartialEq)]
pub struct Numbered {
    pub number: u32,
    pub demo: Demo,
}

/// Серия HL1 в одной папке.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub dir: String,
    pub name: String,
    pub demos: Vec<Numbered>,
    pub missing: Vec<u32>,
}

/// `/^(.+)_(\d{1,6})\.dem$/i`: имя серии и номер.
fn series_name(name: &str) -> Option<(&str, u32)> {
    let dot = name.len().checked_sub(4)?;
    if !name.is_char_boundary(dot) || !name[dot..].eq_ignore_ascii_case(".dem") {
        return None;
    }
    let (prefix, digits) = name[..dot].rsplit_once('_')?;
    let line_break = |c: char| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
    if prefix.is_empty()
        || prefix.contains(line_break)
        || !(1..=6).contains(&digits.len())
        || !digits.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some((prefix, digits.parse().ok()?))
}

/// `groupSeries(demos)`: две и больше демки `<имя>_<N>` в одной папке — один набор в числовом
/// порядке (2 раньше 10), пропуски номеров допустимы. Одиночная и без номера — как была.
pub fn group_series(demos: &[Demo]) -> (Vec<Series>, Vec<Demo>) {
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Series> = HashMap::new();
    let mut rest = Vec::new();
    for demo in demos {
        let Some((name, number)) = series_name(&demo.name) else {
            rest.push(demo.clone());
            continue;
        };
        let key = format!("{}\0{}", demo.dir, name.to_lowercase());
        let group = groups.entry(key.clone()).or_insert_with(|| {
            order.push(key);
            Series {
                dir: demo.dir.clone(),
                name: name.to_owned(),
                demos: Vec::new(),
                missing: Vec::new(),
            }
        });
        group.demos.push(Numbered {
            number,
            demo: demo.clone(),
        });
    }
    let mut series = Vec::new();
    for key in order {
        let mut group = groups.remove(&key).expect("группа из списка");
        if group.demos.len() < 2 {
            rest.extend(group.demos.into_iter().map(|numbered| numbered.demo));
            continue;
        }
        group.demos.sort_by(|a, b| {
            a.number
                .cmp(&b.number)
                .then_with(|| collator().compare(&a.demo.name, &b.demo.name))
        });
        // Строже 0.4.9: сверх предела — отдельными демками, а не пропадают из списка.
        let tail = group.demos.split_off(MAX_SEGMENTS.min(group.demos.len()));
        rest.extend(tail.into_iter().map(|numbered| numbered.demo));
        // Нет номеров между первым (с 1, если серия не с 0) и последним.
        let present: HashSet<u32> = group.demos.iter().map(|d| d.number).collect();
        let last = group.demos.last().expect("две и больше").number;
        let mut n = group.demos[0].number.min(1);
        while n <= last && group.missing.len() < MAX_MISSING {
            if !present.contains(&n) {
                group.missing.push(n);
            }
            n += 1;
        }
        series.push(group);
    }
    (series, rest)
}

/// `gameEntries(contexts)`: демки, серии HL1 и плейлисты Ghosting установленных игр.
pub fn game_entries(contexts: &[GameContext]) -> Vec<Entry> {
    let path = NodePath::native();
    let file_of = |context: &GameContext, demo: &Demo| {
        let mut parts = vec![context.install_path.as_str()];
        parts.extend(demo.dir.split('/'));
        parts.push(&demo.name);
        path.join(&parts)
    };
    let mut entries = Vec::new();
    for context in contexts {
        let demos = demo_archive::list_demos(context.config, Path::new(&context.install_path));
        let (series, rest) = if SERIES_GAMES.contains(&context.game_id.as_str()) {
            group_series(&demos)
        } else {
            (Vec::new(), demos)
        };
        for run in series {
            let files = run
                .demos
                .iter()
                .map(|d| file_of(context, &d.demo))
                .collect();
            let mtime = run.demos.iter().map(|d| d.demo.mtime_ms).max().unwrap_or(0) as f64;
            let mut entry = Entry::new(
                "segments",
                "game",
                Some(&context.game_id),
                &run.name,
                files,
                mtime,
            );
            // id не зависит от числа демок: серия, которая пишется прямо сейчас, остаётся выбранной.
            let key = format!("{}/{}", run.dir, run.name).to_lowercase();
            entry.id = id_of(&["game", &context.game_id, "series", &key]);
            entry.series_missing = Some(run.missing);
            entries.push(entry);
        }
        for demo in &rest {
            entries.push(Entry::new(
                "demo",
                "game",
                Some(&context.game_id),
                &demo.name,
                vec![file_of(context, demo)],
                demo.mtime_ms as f64,
            ));
        }
        for run in scan_playlists(context) {
            let mtime = mtime_of(std::slice::from_ref(&run.file));
            entries.push(Entry::new(
                "playlist",
                "game",
                Some(&context.game_id),
                &run.name,
                vec![run.file],
                mtime,
            ));
        }
    }
    entries
}

/// Запись Store `demo-tool.json` (`manual`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    File(String),
    Segments(Vec<String>),
}

impl Record {
    pub fn to_value(&self) -> Value {
        match self {
            Record::File(path) => json!({"kind": "file", "path": path}),
            Record::Segments(paths) => json!({"kind": "segments", "paths": paths}),
        }
    }

    fn paths(&self) -> &[String] {
        match self {
            Record::File(path) => std::slice::from_ref(path),
            Record::Segments(paths) => paths,
        }
    }
}

/// Файл записи остаётся: он на месте или недоступна сама его папка (диск USB или сетевой
/// отключён) — тогда элемент виден с `DEMO_NOT_FOUND`. Файл удалён из существующей папки —
/// запись отбрасывается.
fn keep(path: &NodePath, file: &Value) -> Option<String> {
    let file = file.as_str()?;
    if !path.is_absolute(file) {
        return None;
    }
    (is_regular_file(file) || !Path::new(&path.dirname(file)).exists()).then(|| file.to_owned())
}

/// `manualEntries(stored)`: элементы из записей Store и очищенные записи.
pub fn manual_entries(stored: &Value) -> (Vec<Entry>, Vec<Record>) {
    let path = NodePath::native();
    let mut entries = Vec::new();
    let mut cleaned = Vec::new();
    let records = stored
        .as_array()
        .map_or(&[][..], |list| &list[..list.len().min(MAX_MANUAL)]);
    for record in records {
        let kind = record.get("kind").and_then(Value::as_str);
        if kind == Some("file")
            && let Some(file) = record.get("path").and_then(|p| keep(&path, p))
        {
            let is_playlist = path.extname(&file).to_lowercase() == ".ghplaylist";
            let name = if is_playlist && path.basename(&file).to_lowercase() == PLAYLIST_FILE {
                path.basename(&path.dirname(&file))
            } else {
                path.basename(&file)
            };
            let mtime = mtime_of(std::slice::from_ref(&file));
            entries.push(Entry::new(
                if is_playlist { "playlist" } else { "demo" },
                "manual",
                None,
                &name,
                vec![file.clone()],
                mtime,
            ));
            cleaned.push(Record::File(file));
        } else if kind == Some("segments")
            && let Some(paths) = record.get("paths").and_then(Value::as_array)
        {
            let files: Vec<String> = paths
                .iter()
                .filter_map(|p| keep(&path, p))
                .take(MAX_SEGMENTS)
                .collect();
            if files.is_empty() {
                continue;
            }
            let name = if files.len() > 1 {
                format!(
                    "{} (+{})",
                    basename_without_ext(&path, &files[0]),
                    files.len() - 1
                )
            } else {
                path.basename(&files[0])
            };
            let mtime = mtime_of(&files);
            entries.push(Entry::new(
                if files.len() > 1 { "segments" } else { "demo" },
                "manual",
                None,
                &name,
                files.clone(),
                mtime,
            ));
            cleaned.push(Record::Segments(files));
        }
    }
    (entries, cleaned)
}

/// Дополнительные поля сегмента из записи плейлиста.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentPlaylist {
    pub timer_start: f64,
    pub timer_end: f64,
    pub map: String,
    pub size_matches: bool,
    pub sha256: String,
    pub file: String,
}

/// Сегмент описания: файл, заголовок (или код отказа) и размер.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// Номер с 1.
    pub index: usize,
    pub name: String,
    pub path: String,
    pub playlist: Option<SegmentPlaylist>,
    pub header: Result<DemoHeader, &'static str>,
    /// Размер файла (у битого — настоящий, не 0).
    pub size: u64,
}

impl Segment {
    pub fn ok(&self) -> bool {
        self.header.is_ok()
    }

    pub fn map(&self) -> &str {
        self.header
            .as_ref()
            .map_or("", |header| header.map.as_str())
    }

    pub fn playlist_map(&self) -> &str {
        self.playlist
            .as_ref()
            .map_or("", |playlist| playlist.map.as_str())
    }

    pub fn duration(&self) -> Option<f64> {
        self.header.as_ref().ok().and_then(|header| header.duration)
    }

    pub fn ticks(&self) -> i32 {
        self.source().map_or(0, |source| source.ticks)
    }

    pub fn tickrate(&self) -> Option<f64> {
        self.source().and_then(|source| source.tickrate)
    }

    pub fn timer_start(&self) -> Option<f64> {
        self.playlist.as_ref().map(|playlist| playlist.timer_start)
    }

    pub fn timer_end(&self) -> Option<f64> {
        self.playlist.as_ref().map(|playlist| playlist.timer_end)
    }

    pub fn engine(&self) -> Option<&'static str> {
        self.header.as_ref().ok().map(|header| header.engine)
    }

    pub fn game(&self) -> Option<&'static str> {
        self.header.as_ref().ok().and_then(|header| header.game)
    }

    pub fn supported(&self) -> bool {
        self.header.as_ref().is_ok_and(|header| header.supported)
    }

    /// Код отказа: битый файл или причина, по которой демка без рендера.
    pub fn code(&self) -> Option<&'static str> {
        match &self.header {
            Ok(header) => header.reason.filter(|_| !header.supported),
            Err(code) => Some(code),
        }
    }

    fn source(&self) -> Option<&headers::SourceFields> {
        self.header
            .as_ref()
            .ok()
            .and_then(|header| header.source.as_ref())
    }

    pub fn to_value(&self) -> Value {
        let mut out = Map::new();
        out.insert("index".to_owned(), json!(self.index));
        out.insert("name".to_owned(), json!(self.name));
        out.insert("path".to_owned(), json!(self.path));
        if let Some(playlist) = &self.playlist {
            out.insert("timerStart".to_owned(), js_number(playlist.timer_start));
            out.insert("timerEnd".to_owned(), js_number(playlist.timer_end));
            out.insert("playlistMap".to_owned(), json!(playlist.map));
            out.insert("sizeMatches".to_owned(), json!(playlist.size_matches));
            out.insert("sha256".to_owned(), json!(playlist.sha256));
            out.insert("playlistFile".to_owned(), json!(playlist.file));
        }
        out.insert("ok".to_owned(), json!(self.ok()));
        out.insert("code".to_owned(), json!(self.code()));
        match &self.header {
            Err(_) => {
                for key in ["engine", "game", "map", "duration"] {
                    out.insert(key.to_owned(), Value::Null);
                }
            }
            Ok(header) => {
                out.insert("engine".to_owned(), json!(header.engine));
                out.insert("game".to_owned(), json!(header.game));
                out.insert("supported".to_owned(), json!(header.supported));
                out.insert("map".to_owned(), json!(header.map));
                out.insert("gameDir".to_owned(), json!(header.game_dir));
                out.insert("demoProtocol".to_owned(), json!(header.demo_protocol));
                out.insert("netProtocol".to_owned(), json!(header.net_protocol));
                out.insert("duration".to_owned(), optional_number(header.duration));
                match &header.source {
                    Some(source) => {
                        out.insert("ticks".to_owned(), json!(source.ticks));
                        out.insert("frames".to_owned(), json!(source.frames));
                        out.insert("tickrate".to_owned(), optional_number(source.tickrate));
                        out.insert("client".to_owned(), json!(source.client));
                    }
                    None => {
                        out.insert("entries".to_owned(), entries_value(&header.entries));
                    }
                }
            }
        }
        out.insert("size".to_owned(), json!(self.size));
        Value::Object(out)
    }
}

/// `segmentInfo(file, index, extra)`.
fn segment_info(file: &str, index: usize, playlist: Option<SegmentPlaylist>) -> Segment {
    let header = headers::parse_demo_file(Path::new(file));
    // Битый файл: размер всё равно настоящий (в списке не «0 Б»).
    let size = match &header {
        Ok(header) => header.size,
        Err(_) => std::fs::metadata(file).map_or(0, |stat| stat.len()),
    };
    Segment {
        index: index + 1,
        name: NodePath::native().basename(file),
        path: file.to_owned(),
        playlist,
        header,
        size,
    }
}

/// `boundaryMarkers(segments, kind)`: метки источника на стыках сегментов (время демо — сумма
/// длительностей).
pub fn boundary_markers(segments: &[Segment], kind: &str) -> Vec<Marker> {
    let mut markers = Vec::new();
    let mut t = 0.0;
    for (i, segment) in segments.iter().enumerate() {
        if i > 0 {
            let map = segment.map();
            let changed = !map.is_empty() && map != segments[i - 1].map();
            markers.push(Marker {
                t: round3(t),
                kind: if changed {
                    "map"
                } else if kind == "playlist" {
                    "saveload"
                } else {
                    "segment"
                },
                label: if changed {
                    map.to_owned()
                } else {
                    (i + 1).to_string()
                },
            });
        }
        // Длительность null в JS — 0 (`null >= 0`), так что обрыва здесь не бывает.
        t += segment.duration().unwrap_or(0.0);
    }
    markers
}

/// Полное описание элемента (`describe(item)`).
#[derive(Debug, Clone, PartialEq)]
pub struct Described {
    pub entry: Entry,
    pub engine: Option<&'static str>,
    pub game: Option<&'static str>,
    pub map: Option<String>,
    pub maps: Vec<String>,
    pub duration: Option<f64>,
    pub supported: bool,
    pub reason: Option<&'static str>,
    pub segments: Vec<Segment>,
    pub markers: Vec<Marker>,
    pub size: u64,
    pub incomplete: bool,
    pub run_time: Option<f64>,
    /// Имена файлов источника (плейлист и сегменты); нет, если плейлист не прочитан.
    pub source_files: Option<Vec<String>>,
    /// Номер сломанного сегмента: нет ключа, `null` или номер.
    pub failed_segment: Option<Option<usize>>,
    pub warnings: Option<Vec<&'static str>>,
}

impl Described {
    pub fn to_value(&self) -> Value {
        let mut out = Map::new();
        self.entry.fields(&mut out);
        out.insert("engine".to_owned(), json!(self.engine));
        out.insert("game".to_owned(), json!(self.game));
        out.insert("map".to_owned(), json!(self.map));
        out.insert("maps".to_owned(), json!(self.maps));
        out.insert("duration".to_owned(), optional_number(self.duration));
        out.insert("supported".to_owned(), json!(self.supported));
        out.insert("reason".to_owned(), json!(self.reason));
        out.insert(
            "segments".to_owned(),
            Value::Array(self.segments.iter().map(Segment::to_value).collect()),
        );
        out.insert("markers".to_owned(), markers_value(&self.markers));
        out.insert("size".to_owned(), json!(self.size));
        out.insert("incomplete".to_owned(), json!(self.incomplete));
        out.insert("runTime".to_owned(), optional_number(self.run_time));
        if let Some(files) = &self.source_files {
            out.insert("sourceFiles".to_owned(), json!(files));
        }
        if let Some(failed) = self.failed_segment {
            out.insert("failedSegment".to_owned(), json!(failed));
        }
        if let Some(warnings) = &self.warnings {
            out.insert("warnings".to_owned(), json!(warnings));
        }
        Value::Object(out)
    }

    /// `summary(described)`: поля для списка, путь первого файла и число сегментов.
    pub fn summary(&self) -> Value {
        const FIELDS: [&str; 20] = [
            "id",
            "kind",
            "source",
            "gameId",
            "name",
            "mtimeMs",
            "engine",
            "game",
            "map",
            "maps",
            "duration",
            "supported",
            "reason",
            "failedSegment",
            "size",
            "incomplete",
            "runTime",
            "warnings",
            "series",
            "missing",
        ];
        let Value::Object(mut full) = self.to_value() else {
            unreachable!("описание — объект");
        };
        let mut out = Map::new();
        for key in FIELDS {
            if let Some(value) = full.remove(key) {
                out.insert(key.to_owned(), value);
            }
        }
        out.insert("path".to_owned(), json!(self.entry.files.first()));
        out.insert("segmentCount".to_owned(), json!(self.segments.len()));
        Value::Object(out)
    }
}

/// `describe(item)`: движок, игра, карты, сегменты, длительность и метки источника.
/// Читает заголовок каждого сегмента (до 4096): вызывать не в потоке окна.
pub fn describe(item: &Entry) -> Described {
    let path = NodePath::native();
    let mut out = Described {
        entry: item.clone(),
        engine: None,
        game: None,
        map: None,
        maps: Vec::new(),
        duration: None,
        supported: false,
        reason: None,
        segments: Vec::new(),
        markers: Vec::new(),
        size: 0,
        incomplete: false,
        run_time: None,
        source_files: None,
        failed_segment: None,
        warnings: None,
    };
    let first_file = item.files.first().map_or("", String::as_str);
    let segments: Vec<Segment> = if item.kind == "playlist" {
        let read = match playlist::read_playlist(first_file) {
            Ok(read) => read,
            Err(error) => {
                out.reason = Some(error.code);
                out.failed_segment = Some(error.segment);
                return out;
            }
        };
        out.incomplete = read.incomplete;
        out.run_time = Some(read.total);
        let mut files = vec![path.basename(first_file)];
        files.extend(read.segments.iter().map(|segment| segment.name.clone()));
        out.source_files = Some(files);
        read.segments
            .into_iter()
            .enumerate()
            .map(|(index, segment)| {
                let extra = SegmentPlaylist {
                    timer_start: segment.entry.start,
                    timer_end: segment.entry.end,
                    map: segment.entry.map,
                    size_matches: segment.size_matches,
                    sha256: segment.entry.sha256,
                    file: segment.entry.file,
                };
                segment_info(&segment.path, index, Some(extra))
            })
            .collect()
    } else {
        let segments: Vec<Segment> = item
            .files
            .iter()
            .enumerate()
            .map(|(index, file)| segment_info(file, index, None))
            .collect();
        out.source_files = Some(segments.iter().map(|s| s.name.clone()).collect());
        segments
    };

    out.size = segments.iter().map(|segment| segment.size).sum();
    let first = segments.first();
    let broken = segments.iter().find(|segment| !segment.ok());
    let unsupported = segments.iter().find(|s| s.ok() && !s.supported());
    let mixed = segments.iter().any(|segment| {
        segment.ok()
            && (segment.engine() != first.and_then(Segment::engine)
                || segment.game() != first.and_then(Segment::game))
    });
    out.engine = first.and_then(Segment::engine);
    out.game = first.and_then(Segment::game);
    out.map = first
        .map(Segment::map)
        .filter(|map| !map.is_empty())
        .map(str::to_owned);
    for segment in &segments {
        let map = segment.map();
        if !map.is_empty() && out.maps.last().map(String::as_str) != Some(map) {
            out.maps.push(map.to_owned());
        }
    }
    // Длительность null в JS — 0 (`null >= 0`): сумма известна всегда.
    let total = round3(segments.iter().map(|s| s.duration().unwrap_or(0.0)).sum());
    out.duration = (total > 0.0).then_some(total);
    // Source (Ghosting): те же метки, что даст рендер, — пустые сегменты (≤ 1 тика) пропущены,
    // одна функция с адаптером, и главы HL2 по картам. HL1: стыки сегментов набора.
    if out.engine == Some(headers::ENGINE_SOURCE) && broken.is_none() && !mixed {
        let picked = source_render::plan_segments(&segments, None, 60.0).picked;
        let mut markers = source_render::timeline_of(&picked, None);
        markers.extend(source_demo::chapter_markers(&picked));
        markers.sort_by(|a, b| a.t.total_cmp(&b.t));
        out.markers = markers;
    } else {
        out.markers = boundary_markers(&segments, item.kind);
    }
    // Номер сегмента — только у рана из нескольких: у одиночной демки «Сегмент 1:» лишний.
    let numbered = |segment: &Segment| (segments.len() > 1).then_some(segment.index);
    if let Some(segment) = broken {
        out.reason = segment.code();
        out.failed_segment = Some(numbered(segment));
    } else if mixed {
        out.reason = Some("SEGMENTS_MIXED");
    } else if let Some(segment) = unsupported {
        out.reason = segment.code();
        out.failed_segment = Some(numbered(segment));
    } else {
        out.supported = true;
    }
    if out.supported && out.duration.is_none() {
        out.warnings = Some(vec!["DURATION_UNKNOWN"]);
    }
    out.segments = segments;
    out
}

/// Принятый путь из окна.
#[derive(Debug, Clone, PartialEq)]
pub struct Accepted {
    pub name: String,
    pub path: String,
    pub ext: String,
    /// Заголовок демки (у плейлиста нет).
    pub header: Option<DemoHeader>,
}

/// Отказ: имя файла (без пути) и код.
#[derive(Debug, Clone, PartialEq)]
pub struct Rejected {
    pub name: String,
    pub code: &'static str,
}

impl Rejected {
    pub fn to_value(&self) -> Value {
        json!({"name": self.name, "code": self.code})
    }
}

/// Имя файла пути из окна для отказа (без пути, до 260 знаков).
fn shown_name(raw: &Value) -> String {
    raw.as_str().map_or(String::new(), |raw| {
        js::slice_utf16(&NodePath::native().basename(raw), 260)
    })
}

/// `checkPath(raw, platform)`: путь из окна (перетаскивание, диалог) — существует, расширение,
/// заголовок. UNC и пути устройств Windows не принимаются.
pub fn check_path(raw: &Value, platform: Platform) -> Result<Accepted, Rejected> {
    let path = NodePath::native();
    let text = raw.as_str();
    let name = shown_name(raw);
    let reject = |code| Rejected {
        name: name.clone(),
        code,
    };
    let raw = match text {
        Some(raw)
            if !raw.is_empty()
                && js::text::utf16_len(raw) <= 4096
                && !raw.contains('\0')
                && path.is_absolute(raw) =>
        {
            raw
        }
        _ => return Err(reject("PATH_INVALID")),
    };
    if platform == Platform::Win32
        && raw.starts_with(['\\', '/'])
        && raw[1..].starts_with(['\\', '/'])
    {
        return Err(reject("PATH_INVALID"));
    }
    let full = path.resolve(&[raw]);
    let ext = path.extname(&full).to_lowercase();
    if !EXTENSIONS.contains(&ext.as_str()) {
        return Err(reject("FILE_TYPE_UNSUPPORTED"));
    }
    if !is_regular_file(&full) {
        return Err(reject("DEMO_NOT_FOUND"));
    }
    if ext == ".ghplaylist" {
        return match playlist::read_playlist(&full) {
            Ok(_) => Ok(Accepted {
                name: name.clone(),
                path: full,
                ext,
                header: None,
            }),
            Err(error) => Err(reject(error.code)),
        };
    }
    match headers::parse_demo_file(Path::new(&full)) {
        Ok(header) => Ok(Accepted {
            name: name.clone(),
            path: full,
            ext,
            header: Some(header),
        }),
        Err(code) => Err(reject(code)),
    }
}

/// `addPaths` без ошибки: новые записи Store и отказы.
#[derive(Debug, Clone, PartialEq)]
pub struct Added {
    pub stored: Vec<Record>,
    pub added: Vec<Record>,
    pub rejected: Vec<Rejected>,
}

/// `addPaths(stored, rawPaths, platform)`: новые записи Store из добавленных путей. Несколько
/// демо HL1 одним набором — ран из сегментов (естественный порядок по имени), остальное по
/// одному.
pub fn add_paths(stored: &[Record], raw_paths: &Value, platform: Platform) -> Added {
    let path = NodePath::native();
    let mut rejected = Vec::new();
    let mut accepted = Vec::new();
    let raw_paths = raw_paths.as_array().map_or(&[][..], Vec::as_slice);
    let (raw_paths, over) = raw_paths.split_at(raw_paths.len().min(MAX_ADD));
    for raw in raw_paths {
        match check_path(raw, platform) {
            Ok(item) => accepted.push(item),
            Err(item) => rejected.push(item),
        }
    }
    // Строже 0.4.9: пути сверх предела не пропадают молча.
    rejected.extend(over.iter().map(|raw| Rejected {
        name: shown_name(raw),
        code: "LIST_FULL",
    }));
    let mut known: HashSet<String> = stored
        .iter()
        .flat_map(Record::paths)
        .map(|file| file.to_lowercase())
        .collect();
    let fresh: Vec<Accepted> = accepted
        .into_iter()
        .filter(|item| known.insert(item.path.to_lowercase()))
        .collect();
    let is_hl1 = |item: &Accepted| {
        item.header
            .as_ref()
            .is_some_and(|h| h.engine == headers::ENGINE_GOLDSRC && h.game == Some("hl1"))
    };
    let hl1: Vec<&Accepted> = fresh.iter().filter(|item| is_hl1(item)).collect();
    let mut records = Vec::new();
    if hl1.len() > 1 {
        let mut paths: Vec<String> = hl1.iter().map(|item| item.path.clone()).collect();
        paths.sort_by(|a, b| natural_by_name(a, b));
        paths.truncate(MAX_SEGMENTS);
        records.push(Record::Segments(paths));
    }
    for item in &fresh {
        if hl1.len() < 2 || !is_hl1(item) {
            records.push(Record::File(item.path.clone()));
        }
    }
    let room = MAX_MANUAL.saturating_sub(stored.len());
    for record in records.iter().skip(room) {
        for file in record.paths() {
            rejected.push(Rejected {
                name: path.basename(file),
                code: "LIST_FULL",
            });
        }
    }
    records.truncate(room);
    let mut all = stored.to_vec();
    all.extend(records.iter().cloned());
    Added {
        stored: all,
        added: records,
        rejected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_paths_reports_over_limit() {
        let raw = Value::Array(vec![json!("x"); MAX_ADD + 2]);
        let added = add_paths(&[], &raw, Platform::Win32);
        assert_eq!(added.rejected.len(), MAX_ADD + 2);
        let full: Vec<&Rejected> = added
            .rejected
            .iter()
            .filter(|r| r.code == "LIST_FULL")
            .collect();
        assert_eq!(full.len(), 2);
        assert_eq!(full[0].name, "x");
    }

    #[test]
    fn long_series_keeps_tail() {
        let demos: Vec<Demo> = (1..=MAX_SEGMENTS as u32 + 2)
            .map(|n| Demo {
                dir: "d".to_owned(),
                name: format!("run_{n}.dem"),
                size: 1,
                mtime_ms: 0,
            })
            .collect();
        let (series, rest) = group_series(&demos);
        assert_eq!(series[0].demos.len(), MAX_SEGMENTS);
        let names: Vec<&str> = rest.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["run_513.dem", "run_514.dem"]);
    }
}
