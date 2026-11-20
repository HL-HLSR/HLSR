//! Библиотека игр при удалении на Linux (`linux/library-cleanup.js` 0.4.7): маркер с полем `root`
//! (`.hlsr/library.json`) и удаление без перехода по ссылкам.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// `.hlsr/library.json`.
pub fn marker_path(root: &Path) -> PathBuf {
    root.join(".hlsr").join("library.json")
}

/// `writeMarker(root)`: `{"schema":1,"id":"hlsr-library","root":"<корень>"}` без перевода строки.
pub fn write_marker(root: &Path) -> Result<PathBuf, String> {
    let file = marker_path(root);
    if let Some(parent) = file.parent() {
        super::make_dirs(parent, 0o777)
            .map_err(|error| super::fs_error(&error, "mkdir", parent))?;
    }
    let text =
        json!({"schema": 1, "id": "hlsr-library", "root": super::resolve(root).to_string_lossy()})
            .to_string();
    std::fs::write(&file, text).map_err(|error| super::fs_error(&error, "open", &file))?;
    Ok(file)
}

/// `readMarker(root)`: маркер, если он про этот корень.
pub fn read_marker(root: &Path) -> Option<Value> {
    let bytes = std::fs::read(marker_path(root)).ok()?;
    let value = hlsr_core::json_text::json_parse(&String::from_utf8_lossy(&bytes)).ok()?;
    let resolved = super::resolve(root).to_string_lossy().into_owned();
    (value.get("schema").and_then(Value::as_f64) == Some(1.0)
        && value.get("id").and_then(Value::as_str) == Some("hlsr-library")
        && value.get("root").and_then(Value::as_str) == Some(resolved.as_str()))
    .then_some(value)
}

/// `findReparsePoint(target)`: первая ссылка на пути от корня ФС; сбой `lstat` — `None`.
pub fn find_reparse_point(target: &Path) -> Option<PathBuf> {
    let full = super::resolve(target);
    let mut current = PathBuf::new();
    for component in full.components() {
        current.push(component);
        if matches!(
            component,
            std::path::Component::RootDir | std::path::Component::Prefix(_)
        ) {
            continue;
        }
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Some(current),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

/// Что не удалилось: путь и код ошибки (или текст).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub path: String,
    pub error: String,
}

/// Счёт удаления: удалено объектов и что не удалилось.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Removal {
    pub removed: u64,
    pub failed: Vec<Failure>,
}

fn failure(path: &Path, error: &std::io::Error) -> Failure {
    Failure {
        path: path.to_string_lossy().into_owned(),
        error: hlsr_core::js::errno::code(error)
            .map(|code| code.into_owned())
            .unwrap_or_else(|| error.to_string()),
    }
}

/// `removeTreeNoFollow(target, result)`: удалить дерево, ссылки удаляются как файлы (не по цели).
/// Корень ФС и домашний каталог — отказ. Неудачное удаление — запись в `result.failed`; каталог, который
/// не читается, прерывает удаление ошибкой (`readdirSync` 0.4.7 бросал исключение).
pub fn remove_tree_no_follow(
    target: &Path,
    home: &Path,
    result: &mut Removal,
) -> Result<(), String> {
    let full = super::resolve(target);
    if full.parent().is_none() || full == super::resolve(home) {
        return Err("Отказ удалять корневой или домашний каталог".to_owned());
    }
    remove(&full, result)
}

fn remove(full: &Path, result: &mut Removal) -> Result<(), String> {
    let Ok(metadata) = std::fs::symlink_metadata(full) else {
        return Ok(());
    };
    if metadata.is_dir() {
        let names = hlsr_core::js::fs::read_dir_names(full)
            .map_err(|error| super::fs_error(&error, "scandir", full))?;
        for name in names {
            remove(&full.join(name), result)?;
        }
        match std::fs::remove_dir(full) {
            Ok(()) => result.removed += 1,
            Err(error) => result.failed.push(failure(full, &error)),
        }
    } else {
        match std::fs::remove_file(full) {
            Ok(()) => result.removed += 1,
            Err(error) => result.failed.push(failure(full, &error)),
        }
    }
    Ok(())
}
