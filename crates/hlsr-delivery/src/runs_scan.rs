//! Записи игры (`runs-scan.js`): `.run` Ghosting и демо `.dem` HL1 в папке игры, языковые
//! папки мода, показ, поиск файла по имени и удаление.
//!
//! Имена в каталоге идут в том порядке, в каком их отдаёт `fs.readdirSync` Node (в Linux —
//! по возрастанию байтов): от него зависит, какие записи попадут в список, когда их больше
//! предела. Время изменения округляется до миллисекунды, как у `Date` из `fs.Stats`.

use crate::errors::DeliveryError;
use crate::file_ops::hex;
use crate::paths::assert_inside_with;
use hlsr_core::js::fs::{mtime_ms, read_dir};
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{Platform, cmp_utf16, errno, iso_string, math_round};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Сколько записей отдаёт показ.
pub const MAX_RESULTS: usize = 1000;
/// Сколько элементов каталога просматривается.
pub const MAX_ENTRIES_SCANNED: usize = 5000;
/// Расширение записей Ghosting.
pub const EXTENSION: &str = ".run";

/// Запись в списке.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunItem {
    /// Первые 16 шестнадцатеричных цифр sha256 имени.
    pub id: String,
    pub name: String,
    /// Для показа: имя от игры в UTF-8, записанное через cp1251 (`ansi-name.js`).
    pub display_name: String,
    pub size: u64,
    /// Время изменения, ISO-строка с миллисекундами.
    pub modified_at: String,
}

/// Ответ показа: `{ status: "error", code }`, `{ status: "no-directory", path?, items: [] }` или
/// `{ status: "ok", path, items, truncated }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanResult {
    Error {
        code: &'static str,
    },
    /// Каталога записей нет; `path` нет у ответа `scan_many` без каталогов.
    NoDirectory {
        path: Option<PathBuf>,
    },
    Ok {
        path: PathBuf,
        items: Vec<RunItem>,
        truncated: bool,
    },
}

/// Ответ удаления: `{ status: "ok", name }` или `{ status: "error", code }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteResult {
    Ok { name: String },
    Error { code: &'static str },
}

/// Путь внутри `root` настоящий (после раскрытия ссылок), а не выходит за `root`.
/// Сравнение без учёта регистра на обеих ОС, как в 0.4.7.
fn contained_realpath(root: &Path, target: &Path) -> bool {
    let (Ok(real_root), Ok(real_target)) =
        (std::fs::canonicalize(root), std::fs::canonicalize(target))
    else {
        return false;
    };
    let real_root = real_root.to_string_lossy().to_lowercase();
    let real_target = real_target.to_string_lossy().to_lowercase();
    let sep = NodePath::native().sep();
    let prefix = if real_root.ends_with(sep) {
        real_root.clone()
    } else {
        format!("{real_root}{sep}")
    };
    real_target == real_root || real_target.starts_with(&prefix)
}

fn stable_id(relative: &str) -> String {
    let digest = Sha256::digest(relative.as_bytes());
    hex(&digest)[..16].to_owned()
}

/// Записи Ghosting — `.run`, демо Half-Life 1 — `.dem` (`runsExtension` манифеста): годится
/// точка и до восьми строчных латинских букв и цифр, иначе — `.run`.
fn extension_of(extension: Option<&str>) -> &str {
    match extension {
        Some(value)
            if value.strip_prefix('.').is_some_and(|rest| {
                (1..=8).contains(&rest.len())
                    && rest
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            }) =>
        {
            value
        }
        _ => EXTENSION,
    }
}

/// Каталог `relative_dir` внутри `install_path`: `assertInside(installPath,
/// path.join(installPath, relativeDir с / → sep), "Каталог записей")`.
fn directory_of(install_path: &Path, relative_dir: &str) -> Result<String, DeliveryError> {
    let path = NodePath::native();
    let install = install_path.to_string_lossy();
    let joined = path.join(&[&install, &relative_dir.replace('/', path.sep())]);
    assert_inside_with(&path, &install, &joined, Some("Каталог записей"))
}

fn date_text(modified_ms: f64) -> String {
    let ms = math_round(modified_ms);
    let time: SystemTime = if ms >= 0.0 {
        UNIX_EPOCH + Duration::from_millis(ms as u64)
    } else {
        UNIX_EPOCH - Duration::from_millis((-ms) as u64)
    };
    iso_string(time)
}

fn by_date_then_name(a: &RunItem, b: &RunItem) -> std::cmp::Ordering {
    // `a.modifiedAt < b.modifiedAt ? 1 : -1` для разных; по имени — по возрастанию.
    if a.modified_at != b.modified_at {
        return cmp_utf16(&b.modified_at, &a.modified_at);
    }
    cmp_utf16(&a.name, &b.name)
}

/// Записи в каталоге `relative_dir` игры.
pub fn scan(install_path: &Path, relative_dir: &str, extension: Option<&str>) -> ScanResult {
    let wanted = extension_of(extension);
    if install_path.as_os_str().is_empty() {
        return ScanResult::Error {
            code: "NOT_INSTALLED",
        };
    }
    let Ok(directory) = directory_of(install_path, relative_dir) else {
        return ScanResult::Error {
            code: "PATH_INVALID",
        };
    };
    let directory = PathBuf::from(directory);
    let Ok(stat) = std::fs::symlink_metadata(&directory) else {
        return ScanResult::NoDirectory {
            path: Some(directory),
        };
    };
    if stat.file_type().is_symlink() || !stat.is_dir() {
        return ScanResult::Error {
            code: "PATH_INVALID",
        };
    }
    if !contained_realpath(install_path, &directory) {
        return ScanResult::Error {
            code: "PATH_ESCAPE",
        };
    }
    let Ok(entries) = read_dir(&directory) else {
        return ScanResult::Error {
            code: "PATH_NOT_READABLE",
        };
    };

    let path = NodePath::native();
    let mut items: Vec<RunItem> = Vec::new();
    let mut scanned = 0;
    let mut truncated = false;
    for entry in entries {
        scanned += 1;
        if scanned > MAX_ENTRIES_SCANNED {
            truncated = true;
            break;
        }
        if !entry.file_type.is_file() {
            continue;
        }
        if path.extname(&entry.name).to_lowercase() != wanted {
            continue;
        }
        let full = PathBuf::from(path.join(&[&directory.to_string_lossy(), &entry.name]));
        if !contained_realpath(install_path, &full) {
            continue;
        }
        let Ok(file_stat) = std::fs::metadata(&full) else {
            continue;
        };
        if !file_stat.is_file() {
            continue;
        }
        let modified = mtime_ms(&file_stat).unwrap_or(0.0);
        items.push(RunItem {
            id: stable_id(&entry.name),
            display_name: crate::ansi_name::readable_name(&entry.name),
            name: entry.name,
            size: file_stat.len(),
            modified_at: date_text(modified),
        });
        if items.len() >= MAX_RESULTS {
            truncated = true;
            break;
        }
    }
    items.sort_by(by_date_then_name);
    ScanResult::Ok {
        path: directory,
        items,
        truncated,
    }
}

/// Путь к записи по имени, если она есть и лежит внутри папки игры.
pub fn resolve_item(
    install_path: &Path,
    relative_dir: &str,
    name: &str,
    extension: Option<&str>,
) -> Option<PathBuf> {
    let path = NodePath::native();
    if name.is_empty() || name != path.basename(name) {
        return None;
    }
    if path.extname(name).to_lowercase() != extension_of(extension) {
        return None;
    }
    let directory = directory_of(install_path, relative_dir).ok()?;
    let target = assert_inside_with(
        &path,
        &directory,
        &path.join(&[&directory, name]),
        Some("Файл записи"),
    )
    .ok()?;
    let target = PathBuf::from(target);
    if !target.exists() {
        return None;
    }
    if !contained_realpath(install_path, &target) {
        return None;
    }
    Some(target)
}

/// Удаляет запись: только обычный файл с нужным расширением внутри каталога записей.
pub fn delete_item(
    install_path: &Path,
    relative_dir: &str,
    name: &str,
    extension: Option<&str>,
) -> DeleteResult {
    let listed = scan(install_path, relative_dir, extension);
    let listed_path = match listed {
        ScanResult::Ok { path, .. } => path,
        ScanResult::Error { code } => return DeleteResult::Error { code },
        ScanResult::NoDirectory { .. } => {
            return DeleteResult::Error {
                code: "PATH_INVALID",
            };
        }
    };
    let path = NodePath::native();
    if name != path.basename(name)
        || name.contains(['\\', '/'])
        || path.extname(name).to_lowercase() != extension_of(extension)
    {
        return DeleteResult::Error {
            code: "PATH_INVALID",
        };
    }
    let target = PathBuf::from(path.join(&[&listed_path.to_string_lossy(), name]));
    let failed = |error: &std::io::Error| DeleteResult::Error {
        code: if errno::code(error).as_deref() == Some("ENOENT") {
            "FILE_NOT_FOUND"
        } else {
            "PATH_NOT_WRITABLE"
        },
    };
    let stat = match std::fs::symlink_metadata(&target) {
        Ok(stat) => stat,
        Err(error) => return failed(&error),
    };
    if !stat.is_file()
        || stat.file_type().is_symlink()
        || !contained_realpath(&listed_path, &target)
    {
        return DeleteResult::Error {
            code: "PATH_INVALID",
        };
    }
    match std::fs::remove_file(&target) {
        Ok(()) => DeleteResult::Ok {
            name: name.to_owned(),
        },
        Err(error) => failed(&error),
    }
}

/// Языки Steam, для которых движок заводит папку «<мод>_<язык>».
const GAME_LANGUAGES: [&str; 27] = [
    "russian",
    "ukrainian",
    "german",
    "french",
    "spanish",
    "latam",
    "italian",
    "polish",
    "portuguese",
    "brazilian",
    "czech",
    "hungarian",
    "turkish",
    "swedish",
    "danish",
    "dutch",
    "finnish",
    "norwegian",
    "romanian",
    "bulgarian",
    "greek",
    "thai",
    "vietnamese",
    "japanese",
    "koreana",
    "schinese",
    "tchinese",
];

/// `name.slice(units)`: хвост строки после первых `units` единиц UTF-16. Разрезанная
/// суррогатная пара оставила бы в JS одиночный суррогат, который ни с чем не совпадёт, —
/// здесь это U+FFFD.
fn slice_from_utf16(name: &str, units: usize) -> String {
    let all: Vec<u16> = name.encode_utf16().collect();
    String::from_utf16_lossy(all.get(units..).unwrap_or_default())
}

/// Языковые папки мода рядом с основной: GoldSrc с языком игры, отличным от английского,
/// пишет демо в «<мод>_<язык>» (valve_WON_russian). Регистр в имени не важен: игра под
/// Windows и Proton его не различает.
pub fn language_dirs(install_path: &Path, relative_dir: &str) -> Vec<String> {
    let relative = relative_dir.replace('\\', "/");
    let posix = NodePath::new(Platform::Linux, "");
    let parent = posix.dirname(&relative);
    let base = posix.basename(&relative).to_lowercase();
    let Ok(directory) = directory_of(install_path, &parent) else {
        return Vec::new();
    };
    let Ok(entries) = read_dir(Path::new(&directory)) else {
        return Vec::new();
    };
    let prefix = format!("{base}_");
    let base_units = base.encode_utf16().count();
    let mut result: Vec<String> = entries
        .into_iter()
        .filter(|entry| {
            entry.file_type.is_dir()
                && entry.name.to_lowercase().starts_with(&prefix)
                && GAME_LANGUAGES.contains(
                    &slice_from_utf16(&entry.name, base_units + 1)
                        .to_lowercase()
                        .as_str(),
                )
        })
        .map(|entry| {
            if parent == "." {
                entry.name
            } else {
                format!("{parent}/{}", entry.name)
            }
        })
        .collect();
    result.sort_by(|a, b| cmp_utf16(a, b));
    result
}

/// Все каталоги записей: языковые папки, потом основная.
pub fn run_dirs(install_path: &Path, relative_dir: &str) -> Vec<String> {
    let mut dirs = language_dirs(install_path, relative_dir);
    dirs.push(relative_dir.to_owned());
    dirs
}

/// Записи из нескольких каталогов одним списком. Одинаковое имя в двух каталогах
/// показывается один раз: из первого, где оно нашлось (так же их находят показ и удаление).
pub fn scan_many(
    install_path: &Path,
    relative_dirs: &[String],
    extension: Option<&str>,
) -> ScanResult {
    let results: Vec<ScanResult> = relative_dirs
        .iter()
        .map(|relative| scan(install_path, relative, extension))
        .collect();
    let ok: Vec<&ScanResult> = results
        .iter()
        .filter(|result| matches!(result, ScanResult::Ok { .. }))
        .collect();
    if ok.is_empty() {
        return results
            .last()
            .cloned()
            .unwrap_or(ScanResult::NoDirectory { path: None });
    }
    let mut items: Vec<RunItem> = Vec::new();
    let mut names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut any_truncated = false;
    let mut first_path = None;
    for result in &ok {
        let ScanResult::Ok {
            path,
            items: listed,
            truncated,
        } = result
        else {
            continue;
        };
        first_path.get_or_insert_with(|| path.clone());
        any_truncated |= *truncated;
        for item in listed {
            if !names.insert(item.name.to_lowercase()) {
                continue;
            }
            items.push(item.clone());
        }
    }
    items.sort_by(by_date_then_name);
    let over = items.len() > MAX_RESULTS;
    items.truncate(MAX_RESULTS);
    ScanResult::Ok {
        path: first_path.unwrap_or_default(),
        items,
        truncated: any_truncated || over,
    }
}
