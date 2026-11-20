//! Список файлов Windows-сборки (`windows-install/file-list.js`): путь, размер и SHA-256 каждого
//! файла. Пишется при сборке в `resources/install-files.json` и едет внутри установщика; установка
//! копирует ровно эти файлы и сверяет каждый, так что битая распаковка или файл, унесённый
//! антивирусом, не заменят рабочую установку. Там же `appId` и имя продукта сборки: по ним
//! установщик выбирает ключи реестра и каталог ([`layout::identity`]).

use super::layout::{self, App};
use hlsr_core::js::text::hex;
use hlsr_core::js::{self, fs as js_fs};
use hlsr_core::json_formats::compact;
use hlsr_core::json_text::parse_js_json;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

/// Имя файла списка.
pub const FILE: &str = "install-files.json";
/// Путь файла списка в сборке (всегда через `/`).
pub const RELATIVE: &str = "resources/install-files.json";
const SCHEMA: f64 = 1.0;
/// `Number.MAX_SAFE_INTEGER`.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Файл списка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileItem {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// Список файлов сборки. Поля `version`, `app` и `files` разобраны и проверены; исходный JSON
/// хранится целиком: установка пишет в каталог `JSON.stringify(list) + "\n"` разобранного
/// объекта, со всеми незнакомыми полями.
#[derive(Debug, Clone, PartialEq)]
pub struct FileList {
    pub version: String,
    /// `appId` и имя продукта; пустое имя продукта (в списке его нет) — боевое
    /// (см. [`layout::identity`]).
    pub app: App,
    pub files: Vec<FileItem>,
    raw: Value,
}

impl FileList {
    /// `JSON.stringify(list)`.
    pub fn to_value(&self) -> Value {
        self.raw.clone()
    }

    /// `JSON.stringify(list) + "\n"`: так список лежит в установленном каталоге.
    pub fn to_text(&self) -> String {
        compact(&self.raw) + "\n"
    }

    /// Сумма размеров файлов списка.
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|item| item.size).sum()
    }
}

/// Почему список не построился или не прочитался.
#[derive(Debug, thiserror::Error)]
pub enum FileListError {
    #[error("Ссылка в сборке: {0}")]
    Link(String),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("Список файлов установщика не читается: {0}")]
    Unreadable(String),
    #[error("Список файлов установщика повреждён: {}", .0.iter().take(3).cloned().collect::<Vec<_>>().join("; "))]
    Broken(Vec<String>),
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> FileListError + '_ {
    move |source| FileListError::Io {
        path: path.to_owned(),
        source,
    }
}

/// SHA-256 файла (hex), потоком по мегабайту.
pub fn sha256_file(file: &Path) -> io::Result<String> {
    let mut input = std::fs::File::open(file)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn walk(root: &Path, relative: &str, out: &mut Vec<String>) -> Result<(), FileListError> {
    let dir = if relative.is_empty() {
        root.to_owned()
    } else {
        root.join(relative)
    };
    for entry in js_fs::read_dir(&dir).map_err(io_error(&dir))? {
        let child = if relative.is_empty() {
            entry.name.clone()
        } else {
            format!("{relative}/{}", entry.name)
        };
        if entry.file_type.is_symlink() {
            return Err(FileListError::Link(child));
        }
        if entry.file_type.is_dir() {
            walk(root, &child, out)?;
        } else if entry.file_type.is_file() {
            out.push(child);
        }
    }
    Ok(())
}

/// `build(root, version, app)`: список файлов распакованной сборки. Файл самого списка в него не
/// входит; порядок — по строкам JS (UTF-16); ссылка в сборке — ошибка.
pub fn build(root: &Path, version: &str, app: &App) -> Result<FileList, FileListError> {
    let mut paths = Vec::new();
    walk(root, "", &mut paths)?;
    paths.retain(|path| path != RELATIVE);
    paths.sort_by(|a, b| js::cmp_utf16(a, b));
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let full = root.join(&path);
        let size = std::fs::metadata(&full).map_err(io_error(&full))?.len();
        let sha256 = sha256_file(&full).map_err(io_error(&full))?;
        files.push(FileItem { path, size, sha256 });
    }
    let raw = json!({
        "schema": 1,
        "version": version,
        "app": {"appId": app.app_id, "productName": app.product_name},
        "files": files
            .iter()
            .map(|item| json!({"path": item.path, "size": item.size, "sha256": item.sha256}))
            .collect::<Vec<_>>(),
    });
    Ok(FileList {
        version: version.to_owned(),
        app: app.clone(),
        files,
        raw,
    })
}

fn is_sha256(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(|text| {
        text.len() == 64
            && text
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// `Number.isSafeInteger(value)`.
fn is_safe_integer(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Number(number)) => number
            .as_f64()
            .is_some_and(|x| x.is_finite() && x.fract() == 0.0 && x.abs() <= MAX_SAFE_INTEGER),
        _ => false,
    }
}

/// `item && item.<field>`: поле элемента-объекта; `null` остаётся `null`, у прочих значений поля нет.
fn field_of<'a>(item: &'a Value, name: &str) -> Option<&'a Value> {
    match item {
        Value::Object(map) => map.get(name),
        Value::Null => Some(item),
        _ => None,
    }
}

fn is_invalid_path(file: &str) -> bool {
    if file.is_empty() || file.starts_with('/') || file.contains(['\\', ':']) {
        return true;
    }
    // /(^|\/)\.\.(\/|$)/: сегмент «..».
    file.split('/').any(|part| part == "..")
}

/// Что не так со списком (пустой список — всё в порядке). Тексты и порядок — как у `problemsOf`.
pub fn problems_of(data: &Value) -> Vec<String> {
    let schema_ok = data
        .get("schema")
        .and_then(Value::as_f64)
        .is_some_and(|x| x == SCHEMA);
    let files = data.get("files").and_then(Value::as_array);
    let (true, Some(files)) = (schema_ok, files) else {
        return vec!["неизвестная схема списка файлов".to_owned()];
    };
    let version = data.get("version").and_then(Value::as_str).unwrap_or("");
    if !layout::is_version(version) {
        let shown = js::to_template_string(data.get("version"));
        return vec![format!("некорректная версия: {shown}")];
    }
    let mut problems = Vec::new();
    if let Err(message) = check_app(data.get("app")) {
        problems.push(message);
    }
    let mut seen = std::collections::HashSet::new();
    for item in files {
        let file = field_of(item, "path");
        let shown = js::to_template_string(file);
        match file.and_then(Value::as_str) {
            Some(text) if !is_invalid_path(text) => {
                if !seen.insert(text.to_lowercase()) {
                    problems.push(format!("повтор: {text}"));
                }
            }
            _ => problems.push(format!("недопустимый путь: {shown}")),
        }
        let size = field_of(item, "size");
        let negative = size
            .and_then(Value::as_f64)
            .is_some_and(|x| x < 0.0 && !x.is_nan());
        if !is_safe_integer(size) || negative {
            problems.push(format!("некорректный размер: {shown}"));
        }
        if !is_sha256(field_of(item, "sha256")) {
            problems.push(format!("некорректный SHA-256: {shown}"));
        }
    }
    let has_exe = files
        .iter()
        .any(|item| field_of(item, "path").and_then(Value::as_str) == Some(layout::EXE));
    if !has_exe {
        problems.push(format!("в списке нет {}", layout::EXE));
    }
    problems
}

/// Поле `app`: `appId` — непустая строка, имя продукта допустимо ([`layout::identity`]).
fn check_app(app: Option<&Value>) -> Result<(), String> {
    let app_id = app
        .and_then(|app| app.get("appId"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty());
    let Some(app_id) = app_id else {
        return Err("нет appId сборки".to_owned());
    };
    let product_name = match app.and_then(|app| app.get("productName")) {
        Some(value) if js::truthy(value) => js::to_string(value),
        _ => String::new(),
    };
    layout::identity(Some(&App {
        app_id: app_id.to_owned(),
        product_name,
    }))
    .map(drop)
    .map_err(|error| error.to_string())
}

/// Записать список в сборку или в каталог установки: `resources/install-files.json`,
/// `JSON.stringify(list) + "\n"`, файл создаётся заново (`wx`) — существующий не затирается.
pub fn write_new(root: &Path, list: &FileList) -> io::Result<()> {
    use std::io::Write as _;
    let file = root.join(RELATIVE);
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(file)?;
    output.write_all(list.to_text().as_bytes())
}

/// Список из разобранного JSON; непроверенный список — [`FileListError::Broken`].
pub fn from_value(data: Value) -> Result<FileList, FileListError> {
    let problems = problems_of(&data);
    if !problems.is_empty() {
        return Err(FileListError::Broken(problems));
    }
    let text = |value: Option<&Value>| value.map(js::to_string).unwrap_or_default();
    let app = data.get("app");
    let app = App {
        app_id: text(app.and_then(|app| app.get("appId"))),
        product_name: match app.and_then(|app| app.get("productName")) {
            Some(value) if js::truthy(value) => js::to_string(value),
            _ => String::new(),
        },
    };
    let files = data["files"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| FileItem {
                    path: text(item.get("path")),
                    size: item["size"].as_f64().unwrap_or(0.0) as u64,
                    sha256: text(item.get("sha256")),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(FileList {
        version: text(data.get("version")),
        app,
        files,
        raw: data,
    })
}

/// `read(root)`: `resources/install-files.json` распакованной сборки, проверенный.
pub fn read(root: &Path) -> Result<FileList, FileListError> {
    let file = root.join(RELATIVE);
    let data = js_fs::read_file(&file)
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            parse_js_json(&js::utf8_string(&bytes)).map_err(|error| error.to_string())
        })
        .map_err(FileListError::Unreadable)?;
    from_value(data)
}
