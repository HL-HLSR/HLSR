//! Записи архива (`archive-entries.js`): разбор `7za l -slt`, опасные пути,
//! проверка записей перед распаковкой.

use hlsr_core::js;
use hlsr_core::json_text::js_number;
use serde_json::{Map, Value, json};

/// Запись листинга (`normalizeEntry`). Размеры — числа JS.
#[derive(Debug, Clone, PartialEq)]
pub struct ArchiveEntry {
    pub path: String,
    pub size: f64,
    pub packed_size: f64,
    pub attributes: String,
    pub modified: Option<String>,
    pub crc: Option<String>,
    pub encrypted: bool,
    pub is_directory: bool,
    pub is_symlink: bool,
}

impl ArchiveEntry {
    /// Запись как объект 0.4.7 (те же ключи и порядок).
    pub fn to_json(&self) -> Value {
        json!({
            "path": self.path,
            "size": js_number(self.size),
            "packedSize": js_number(self.packed_size),
            "attributes": self.attributes,
            "modified": self.modified,
            "crc": self.crc,
            "encrypted": self.encrypted,
            "isDirectory": self.is_directory,
            "isSymlink": self.is_symlink,
        })
    }
}

/// Итог `parseListing`: поля архива (до строки `----------`) и записи.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Listing {
    /// Поля архива в порядке появления; повтор ключа меняет значение на месте.
    pub archive: Vec<(String, String)>,
    pub entries: Vec<ArchiveEntry>,
}

impl Listing {
    /// Поле архива по имени (`Type`, `Physical Size`…).
    pub fn archive_field(&self, key: &str) -> Option<&str> {
        self.archive
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// `{archive, entries}` как у 0.4.7.
    pub fn to_json(&self) -> Value {
        let archive: Map<String, Value> = self
            .archive
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        json!({
            "archive": archive,
            "entries": self.entries.iter().map(ArchiveEntry::to_json).collect::<Vec<_>>(),
        })
    }
}

/// Блок `Ключ = значение` в порядке появления ключей.
type Block = Vec<(String, String)>;

fn assign(block: &mut Block, key: String, value: String) {
    match block.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = value,
        None => block.push((key, value)),
    }
}

/// `parseListing(stdout)`: блоки до `----------` — поля архива, после — записи.
pub fn parse_listing(stdout: &str) -> Listing {
    let mut listing = Listing::default();
    let mut separator_seen = false;
    let mut current: Option<Block> = None;

    fn flush(current: &mut Option<Block>, separator_seen: bool, listing: &mut Listing) {
        if let Some(block) = current.take().filter(|b| !b.is_empty()) {
            if separator_seen {
                listing.entries.push(normalize_entry(&block));
            } else {
                // Object.assign(archive, current).
                for (key, value) in block {
                    assign(&mut listing.archive, key, value);
                }
            }
        }
    }

    for line in split_lines(stdout) {
        if line.len() >= 10 && line.bytes().all(|b| b == b'-') {
            flush(&mut current, separator_seen, &mut listing);
            separator_seen = true;
            continue;
        }
        if js::trim(line).is_empty() {
            flush(&mut current, separator_seen, &mut listing);
            continue;
        }
        let Some((key, value)) = key_value(line) else {
            continue;
        };
        assign(current.get_or_insert_with(Vec::new), key, value);
    }
    flush(&mut current, separator_seen, &mut listing);
    listing
}

/// `String(stdout).split(/\r?\n/)`.
fn split_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    // `\r` перед `\n` — часть разделителя; у последней строки `\n` нет.
    let last = lines.len() - 1;
    for line in &mut lines[..last] {
        *line = line.strip_suffix('\r').unwrap_or(line);
    }
    lines
}

/// `/^([A-Za-z][A-Za-z0-9 ]*?) = ?(.*)$/`: самый короткий ключ перед « =».
/// `.` не берёт `\r`, U+2028 и U+2029 — строка с ними не разбирается.
fn key_value(line: &str) -> Option<(String, String)> {
    if line.contains(['\r', '\u{2028}', '\u{2029}']) {
        return None;
    }
    let bytes = line.as_bytes();
    if !bytes.first()?.is_ascii_alphabetic() {
        return None;
    }
    for end in 1..bytes.len() {
        if line[end..].starts_with(" =") {
            let rest = &line[end + 2..];
            let value = rest.strip_prefix(' ').unwrap_or(rest);
            return Some((js::trim(&line[..end]).to_owned(), value.to_owned()));
        }
        if !(bytes[end].is_ascii_alphanumeric() || bytes[end] == b' ') {
            return None;
        }
    }
    None
}

/// Слова атрибутов: `(^|\s)[A-Z_]*<буква>[A-Z_]*(\s|$)` с флагом `i`.
fn has_attribute(attributes: &str, letter: char) -> bool {
    attributes.split(js::is_whitespace).any(|word| {
        !word.is_empty()
            && word.chars().all(|c| c.is_ascii_alphabetic() || c == '_')
            && word.chars().any(|c| c.eq_ignore_ascii_case(&letter))
    })
}

/// `/^l[rwxsSt-]{9}/`: права симлинка в выводе p7zip.
fn is_unix_symlink_mode(attributes: &str) -> bool {
    let mut chars = attributes.chars();
    chars.next() == Some('l')
        && chars
            .take(9)
            .filter(|c| matches!(c, 'r' | 'w' | 'x' | 's' | 'S' | 't' | '-'))
            .count()
            == 9
}

fn field<'a>(block: &'a Block, key: &str) -> Option<&'a str> {
    block
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// `normalizeEntry(raw)`.
fn normalize_entry(raw: &Block) -> ArchiveEntry {
    fn truthy(value: Option<&str>) -> Option<&str> {
        value.filter(|v| !v.is_empty())
    }
    let attributes = truthy(field(raw, "Attributes"))
        .unwrap_or_default()
        .to_owned();
    // Number(raw.Size || 0): не число — 0.
    let size = truthy(field(raw, "Size")).map_or(0.0, js::to_number);
    // Number(raw["Packed Size"] || 0) || 0: NaN и -0 — 0, бесконечность остаётся.
    let packed = truthy(field(raw, "Packed Size")).map_or(0.0, js::to_number);
    ArchiveEntry {
        path: truthy(field(raw, "Path")).unwrap_or_default().to_owned(),
        size: if size.is_finite() { size } else { 0.0 },
        packed_size: if packed.is_nan() || packed == 0.0 {
            0.0
        } else {
            packed
        },
        modified: truthy(field(raw, "Modified")).map(str::to_owned),
        crc: truthy(field(raw, "CRC")).map(str::to_owned),
        encrypted: field(raw, "Encrypted") == Some("+"),
        is_directory: has_attribute(&attributes, 'D'),
        is_symlink: has_attribute(&attributes, 'L') || is_unix_symlink_mode(&attributes),
        attributes,
    }
}

const RESERVED_NAMES: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];

/// `/^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(\.|$)/i` для имени файла.
fn is_reserved_device_name(name: &str) -> bool {
    let upper = name.as_bytes();
    let base = if upper.len() >= 4
        && (upper[..3].eq_ignore_ascii_case(b"COM") || upper[..3].eq_ignore_ascii_case(b"LPT"))
        && (b'1'..=b'9').contains(&upper[3])
    {
        4
    } else if upper.len() >= 3
        && RESERVED_NAMES
            .iter()
            .any(|n| upper[..3].eq_ignore_ascii_case(n.as_bytes()))
    {
        3
    } else {
        return false;
    };
    upper.len() == base || upper[base] == b'.'
}

/// `classifyPath(entryPath)`: что не так с путём записи (пусто — всё в порядке).
pub fn classify_path(entry_path: &str) -> Vec<&'static str> {
    let mut problems = Vec::new();
    let segments: Vec<&str> = entry_path.split(['\\', '/']).collect();
    let bytes = entry_path.as_bytes();
    let is_sep = |b: Option<&u8>| matches!(b, Some(b'\\' | b'/'));

    if js::trim(entry_path).is_empty() {
        problems.push("empty-path");
    }
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        problems.push("drive-path");
    }
    if is_sep(bytes.first()) && is_sep(bytes.get(1)) {
        problems.push("unc-path");
    } else if is_sep(bytes.first()) {
        problems.push("absolute-path");
    }
    if segments.contains(&"..") {
        problems.push("parent-traversal");
    }
    if entry_path.contains('\0') {
        problems.push("null-byte");
    }
    if entry_path.contains(['<', '>', '|', '"', '*', '?']) {
        problems.push("invalid-character");
    }
    if is_reserved_device_name(segments.last().copied().unwrap_or_default()) {
        problems.push("reserved-device-name");
    }
    problems
}

/// Пределы `inspectEntries`; ни одного или 0 — без предела (`|| Infinity`).
#[derive(Debug, Clone, Default)]
pub struct InspectOptions {
    pub max_entries: Option<f64>,
    pub max_uncompressed_size: Option<f64>,
}

/// Запись с опасным путём.
#[derive(Debug, Clone, PartialEq)]
pub struct UnsafeEntry {
    pub path: String,
    pub problems: Vec<&'static str>,
}

/// Отчёт `inspectEntries`.
#[derive(Debug, Clone, PartialEq)]
pub struct InspectReport {
    pub ok: bool,
    pub errors: Vec<&'static str>,
    pub unsafe_entries: Vec<UnsafeEntry>,
    pub symlinks: Vec<String>,
    pub encrypted: Vec<String>,
    /// Первые сегменты безопасных путей в порядке появления.
    pub roots: Vec<String>,
    pub total: usize,
    pub files: usize,
    pub directories: usize,
    pub uncompressed_size: f64,
}

impl InspectReport {
    /// Отчёт как объект 0.4.7.
    pub fn to_json(&self) -> Value {
        json!({
            "ok": self.ok,
            "errors": self.errors,
            "unsafe": self.unsafe_entries.iter()
                .map(|u| json!({"path": u.path, "problems": u.problems}))
                .collect::<Vec<_>>(),
            "symlinks": self.symlinks,
            "encrypted": self.encrypted,
            "roots": self.roots,
            "counts": {"total": self.total, "files": self.files, "directories": self.directories},
            "uncompressedSize": js_number(self.uncompressed_size),
        })
    }
}

fn limit(value: Option<f64>) -> f64 {
    match value {
        Some(v) if v != 0.0 && !v.is_nan() => v,
        _ => f64::INFINITY,
    }
}

/// `inspectEntries(entries, options)`.
pub fn inspect_entries(entries: &[ArchiveEntry], options: &InspectOptions) -> InspectReport {
    let max_entries = limit(options.max_entries);
    let max_uncompressed_size = limit(options.max_uncompressed_size);
    let mut report = InspectReport {
        ok: false,
        errors: Vec::new(),
        unsafe_entries: Vec::new(),
        symlinks: Vec::new(),
        encrypted: Vec::new(),
        roots: Vec::new(),
        total: entries.len(),
        files: 0,
        directories: 0,
        uncompressed_size: 0.0,
    };

    for entry in entries {
        let problems = classify_path(&entry.path);
        if entry.is_symlink {
            report.symlinks.push(entry.path.clone());
        }
        if entry.encrypted {
            report.encrypted.push(entry.path.clone());
        }
        if entry.is_directory {
            report.directories += 1;
        } else {
            report.files += 1;
            report.uncompressed_size += entry.size;
        }
        if problems.is_empty() {
            let first = entry.path.split(['\\', '/']).next().unwrap_or_default();
            if !first.is_empty() && !report.roots.iter().any(|r| r == first) {
                report.roots.push(first.to_owned());
            }
        } else {
            report.unsafe_entries.push(UnsafeEntry {
                path: entry.path.clone(),
                problems,
            });
        }
    }

    if !report.unsafe_entries.is_empty() {
        report.errors.push("unsafe-entry");
    }
    if !report.symlinks.is_empty() {
        report.errors.push("symlink-entry");
    }
    if !report.encrypted.is_empty() {
        report.errors.push("encrypted-entry");
    }
    if entries.len() as f64 > max_entries {
        report.errors.push("too-many-entries");
    }
    if report.uncompressed_size > max_uncompressed_size {
        report.errors.push("uncompressed-size-limit");
    }
    report.ok = report.errors.is_empty();
    report
}
