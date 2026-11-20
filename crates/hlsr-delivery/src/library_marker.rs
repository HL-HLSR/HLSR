//! Маркер библиотеки `.hlsr/library.json` (`uninstall/library-cleanup.js`,
//! `writeMarker`/`readMarker`): по нему удаление лаунчера узнаёт папку HLSR.

use hlsr_core::js::{self, path::NodePath};
use hlsr_core::json_text::{parse_js_json, stringify};
use serde_json::{Value, json};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const MARKER_SCHEMA: i64 = 1;
pub const MARKER_ID: &str = "hlsr-library";

/// `MARKER_RELATIVE`: `.hlsr/library.json` с разделителем этой ОС.
pub fn marker_relative() -> PathBuf {
    Path::new(".hlsr").join("library.json")
}

/// Файл маркера библиотеки `root`.
pub fn marker_path(root: &Path) -> PathBuf {
    root.join(marker_relative())
}

/// `markerLinked(root)`: `.hlsr` или `library.json` — ссылка (symlink, junction). Такой маркер
/// не считается и через неё не пишется — он мог бы прийти из чужой папки.
pub fn marker_linked(root: &Path) -> bool {
    for part in [root.join(".hlsr"), marker_path(root)] {
        match std::fs::symlink_metadata(&part) {
            Ok(meta) if meta.file_type().is_symlink() => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    false
}

/// `writeMarker(root, extra)`: `{schema, id, root, createdAt[, createdBy]}` двумя пробелами и
/// `\n`, прямой записью (не атомарно), как в 0.4.9. Маркер-ссылка — ошибка.
pub fn write_marker(root: &Path, now: SystemTime, created_by: Option<&str>) -> io::Result<PathBuf> {
    let file = marker_path(root);
    if marker_linked(root) {
        return Err(io::Error::other(format!(
            "маркер библиотеки — ссылка: {}",
            file.display()
        )));
    }
    let mut payload = json!({
        "schema": MARKER_SCHEMA,
        "id": MARKER_ID,
        "root": NodePath::native().resolve(&[&root.to_string_lossy()]),
        "createdAt": js::iso_string(now),
    });
    if let Some(created_by) = created_by {
        payload["createdBy"] = json!(created_by);
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&file, stringify(&payload, "  ") + "\n")?;
    Ok(file)
}

/// `readMarker(root)`: содержимое маркера, если это объект с нашими `id` и `schema` и не ссылка.
pub fn read_marker(root: &Path) -> Option<Value> {
    if marker_linked(root) {
        return None;
    }
    let bytes = std::fs::read(marker_path(root)).ok()?;
    let data = parse_js_json(&String::from_utf8_lossy(&bytes)).ok()?;
    if !matches!(data, Value::Object(_) | Value::Array(_)) {
        return None;
    }
    let schema_ok = data
        .get("schema")
        .and_then(Value::as_f64)
        .is_some_and(|schema| schema == MARKER_SCHEMA as f64);
    (data.get("id").and_then(Value::as_str) == Some(MARKER_ID) && schema_ok).then_some(data)
}
