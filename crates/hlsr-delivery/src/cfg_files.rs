//! Редактор cfg-файлов в настройках игры (`cfg-files.js`): все cfg в папке сборки, текст
//! файла и его запись. Правленый файл лаунчер потом не трогает: обновление и проверка файлов
//! сравнивают его с версией сборки (`user_configs`).
//!
//! Текст отдаётся как есть. cfg не в UTF-8 (старые файлы в cp1251) читаются побайтно как
//! latin1 и так же записываются, чтобы не испортить байты, которые редактор не трогал. Время
//! изменения файла (`mtime`, миллисекунды с дробью) нужно для защиты от чужой правки: игра
//! переписывает config.cfg при выходе.
//!
//! Порядок списка — `localeCompare` Node: он зависит от языка по умолчанию процесса (в `ru`
//! кириллица идёт раньше латиницы). Его выбирает ICU: в Linux из первой непустой `LC_ALL`,
//! `LC_MESSAGES`, `LANG` (не из `LANGUAGE`), в Windows — язык форматов; [`list`] берёт именно его
//! (`hlsr_core::js::collate::default_locale`), а [`list_in`] — язык явно.

use crate::atomic::write_file_atomic;
use crate::errors::RawError;
use crate::game_binds::Encoding;
use crate::js_value;
use crate::node_fs::{decode, encode};
use crate::paths::{assert_inside, is_inside};
use crate::user_configs::is_config_path;
use hlsr_core::js;
use hlsr_core::js::collate::{Collator, Sensitivity};
use hlsr_core::js::fs::{mtime_ms, read_dir, read_file};
use hlsr_core::js::path::NodePath;
use serde_json::Value;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// Файлы больше этого размера в редакторе не открываются (`MAX_FILE_BYTES`).
pub const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_FILES: usize = 400;
const MAX_DEPTH: usize = 5;
const MAX_ENTRIES: usize = 40_000;

/// Папки с ресурсами игры: cfg там не бывает, а файлов тысячи.
const SKIP_DIRS: [&str; 21] = [
    "maps",
    "models",
    "materials",
    "sound",
    "sounds",
    "sprites",
    "gfx",
    "media",
    "resource",
    "save",
    "screenshots",
    "download",
    "downloads",
    "logs",
    "particles",
    "scripts_compiled",
    "shaders",
    "bin",
    "platform_bin",
    "steam_api",
    "__macosx",
];

/// Код отказа (`CfgFileError.code`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfgFileCode {
    /// Путь не cfg-файл внутри папки игры.
    PathInvalid,
    /// Файла нет или он не читается.
    NotFound,
    /// Файл или новый текст больше 512 КиБ.
    TooLarge,
    /// Файл изменили после открытия.
    Changed,
    /// В кодировке latin1 нельзя записать символ.
    Encoding,
}

impl CfgFileCode {
    pub fn as_str(self) -> &'static str {
        match self {
            CfgFileCode::PathInvalid => "PATH_INVALID",
            CfgFileCode::NotFound => "NOT_FOUND",
            CfgFileCode::TooLarge => "TOO_LARGE",
            CfgFileCode::Changed => "CHANGED",
            CfgFileCode::Encoding => "ENCODING",
        }
    }
}

/// `CfgFileError`: отказ с кодом (текст ошибки — сам код).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{}", .code.as_str())]
pub struct CfgFileError {
    pub code: CfgFileCode,
}

/// Чем кончился вызов, если не удался: отказ редактора или ошибка файловой системы (в 0.4.7
/// это исключение Node, фасад пишет в журнал и отвечает `IO_FAILED`).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CfgFilesError {
    #[error(transparent)]
    Cfg(#[from] CfgFileError),
    #[error(transparent)]
    Raw(#[from] RawError),
}

fn cfg_error(code: CfgFileCode) -> CfgFilesError {
    CfgFileError { code }.into()
}

/// Язык по умолчанию для [`list`]: язык по умолчанию процесса, как его выбирает ICU Node
/// (`hlsr_core::js::collate::default_locale`). Это не `app.getLocale()`: тот читает ещё и
/// `LANGUAGE`.
pub fn default_locale() -> String {
    hlsr_core::js::collate::default_locale()
}

/// Файл в списке: путь от папки игры через `/`, размер и время изменения (мс).
#[derive(Debug, Clone, PartialEq)]
pub struct CfgFile {
    pub path: String,
    pub size: u64,
    pub mtime: f64,
}

/// Итог [`list`].
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    pub files: Vec<CfgFile>,
    /// Список обрезан пределом числа файлов (400) или просмотренных элементов (40000).
    pub truncated: bool,
}

/// Список cfg сборки в порядке языка по умолчанию процесса (`list`).
pub fn list(install_path: &Path) -> Listing {
    list_in(install_path, &default_locale())
}

/// Список cfg сборки в порядке языка `locale` (`list`). Папки обходятся в ширину, внутри
/// папки — по имени (`localeCompare`), итог — по пути без учёта регистра и ударений. Папки
/// ресурсов, точечные, симлинки и файлы больше 512 КиБ пропускаются; ошибки чтения папок и
/// файлов не отказ, а пропуск.
pub fn list_in(install_path: &Path, locale: &str) -> Listing {
    let by_name = Collator::new(locale, Sensitivity::Variant);
    let by_path = Collator::new(locale, Sensitivity::Base);
    let path = NodePath::native();
    let mut files: Vec<CfgFile> = Vec::new();
    let mut queue: VecDeque<(PathBuf, String, usize)> =
        VecDeque::from([(install_path.to_path_buf(), String::new(), 0)]);
    let mut seen = 0usize;
    let mut truncated = false;
    while let Some((dir, prefix, depth)) = queue.pop_front() {
        if truncated {
            break;
        }
        let Ok(mut entries) = read_dir(&dir) else {
            continue;
        };
        entries.sort_by(|a, b| by_name.compare(&a.name, &b.name));
        for entry in entries {
            seen += 1;
            if seen > MAX_ENTRIES {
                truncated = true;
                break;
            }
            if entry.name.starts_with('.') {
                continue;
            }
            let full = PathBuf::from(path.join(&[&dir.to_string_lossy(), &entry.name]));
            let relative = if prefix.is_empty() {
                entry.name.clone()
            } else {
                format!("{prefix}/{}", entry.name)
            };
            if entry.file_type.is_dir() {
                if depth < MAX_DEPTH && !SKIP_DIRS.contains(&entry.name.to_lowercase().as_str()) {
                    queue.push_back((full, relative, depth + 1));
                }
                continue;
            }
            if !entry.file_type.is_file() || !is_config_path(&entry.name) {
                continue;
            }
            let Ok(stat) = std::fs::metadata(&full) else {
                continue;
            };
            if stat.len() > MAX_FILE_BYTES {
                continue;
            }
            files.push(CfgFile {
                path: relative,
                size: stat.len(),
                mtime: mtime_ms(&stat).unwrap_or(0.0),
            });
            if files.len() >= MAX_FILES {
                truncated = true;
                break;
            }
        }
    }
    files.sort_by(|a, b| by_path.compare(&a.path, &b.path));
    Listing { files, truncated }
}

/// `fs.realpathSync`: путь с развёрнутыми ссылками. В Windows без префикса `\\?\`.
pub(crate) fn real_path(path: &Path) -> std::io::Result<PathBuf> {
    let real = std::fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        let text = real.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{rest}")));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(rest));
        }
    }
    Ok(real)
}

/// Ошибка `realpath` установки так, как её видит вызывающий (Node: `lstat` пути).
pub(crate) fn real_path_error(error: &std::io::Error, path: &Path) -> RawError {
    RawError::fs(error, "lstat", path)
}

/// Путь из рендерера: только cfg внутри папки игры, в том числе после разворота симлинков
/// (ссылка на файл снаружи не читается и не пишется). Возвращает полный путь и путь для ответа.
fn resolve(install_path: &Path, relative: &str) -> Result<(PathBuf, String), CfgFilesError> {
    let text = relative.replace('\\', "/");
    if !is_config_path(&text)
        || text
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(cfg_error(CfgFileCode::PathInvalid));
    }
    let path = NodePath::native();
    let joined = path.join(&[
        &install_path.to_string_lossy(),
        &text.split('/').collect::<Vec<_>>().join(path.sep()),
    ]);
    let full = assert_inside(install_path, Path::new(&joined), Some("Конфиг"))
        .map_err(|_| cfg_error(CfgFileCode::PathInvalid))?;
    let real = real_path(&full).map_err(|_| cfg_error(CfgFileCode::NotFound))?;
    let root = real_path(install_path).map_err(|e| real_path_error(&e, install_path))?;
    if !is_inside(&root, &real) {
        return Err(cfg_error(CfgFileCode::PathInvalid));
    }
    Ok((full, text))
}

/// Перевод строки файла.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eol {
    Crlf,
    Lf,
}

impl Eol {
    /// Как в ответе 0.4.7: `crlf` или `lf`.
    pub fn as_str(self) -> &'static str {
        match self {
            Eol::Crlf => "crlf",
            Eol::Lf => "lf",
        }
    }
}

/// Прочитанный файл.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadFile {
    pub path: String,
    /// Текст с `\n` вместо `\r\n`.
    pub content: String,
    pub encoding: Encoding,
    pub eol: Eol,
    pub mtime: f64,
}

/// Чтение cfg: UTF-8, если байты — верный UTF-8, иначе latin1 (`read`).
pub fn read(install_path: &Path, relative: &str) -> Result<ReadFile, CfgFilesError> {
    let (full, path) = resolve(install_path, relative)?;
    let stat = std::fs::metadata(&full).map_err(|_| cfg_error(CfgFileCode::NotFound))?;
    if stat.len() > MAX_FILE_BYTES {
        return Err(cfg_error(CfgFileCode::TooLarge));
    }
    let buffer = read_file(&full).map_err(|_| cfg_error(CfgFileCode::NotFound))?;
    let (content, encoding) = match String::from_utf8(buffer) {
        Ok(text) => (text, Encoding::Utf8),
        Err(error) => (js::latin1_string(error.as_bytes()), Encoding::Latin1),
    };
    let stat = std::fs::metadata(&full).map_err(|e| RawError::fs(&e, "stat", &full))?;
    Ok(ReadFile {
        path,
        eol: if content.contains("\r\n") {
            Eol::Crlf
        } else {
            Eol::Lf
        },
        content: content.replace("\r\n", "\n"),
        encoding,
        mtime: mtime_ms(&stat).map_err(|e| RawError::fs(&e, "stat", &full))?,
    })
}

/// Параметры записи.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WriteOptions {
    pub encoding: Encoding,
    /// Записать `\r\n` вместо `\n`.
    pub crlf: bool,
    /// Время изменения файла при чтении: если файл с тех пор поменяли, запись отклоняется.
    /// `NaN` (`Number("abc")`) проверку не включает: сравнение с ним ложно.
    pub expected_mtime: Option<f64>,
}

impl Default for WriteOptions {
    fn default() -> WriteOptions {
        WriteOptions {
            encoding: Encoding::Utf8,
            crlf: false,
            expected_mtime: None,
        }
    }
}

impl WriteOptions {
    /// Параметры из `meta` рендерера, как собирает их обработчик `game-cfg-file-write`:
    /// `meta` не объект — пусто; `encoding === "latin1"`, `eol === "crlf"`, `mtime` — что
    /// угодно кроме `null` (`Number(mtime)`).
    pub fn from_meta(meta: &Value) -> WriteOptions {
        let field = |name: &str| js_value::prop(meta, name);
        WriteOptions {
            encoding: if field("encoding").and_then(Value::as_str) == Some("latin1") {
                Encoding::Latin1
            } else {
                Encoding::Utf8
            },
            crlf: field("eol").and_then(Value::as_str) == Some("crlf"),
            expected_mtime: field("mtime")
                .filter(|v| !v.is_null())
                .map(js_value::to_number),
        }
    }
}

/// `String(content == null ? "" : content)`: текст для записи из значения рендерера.
pub fn content_text(content: &Value) -> String {
    if content.is_null() {
        String::new()
    } else {
        js::to_string(content)
    }
}

/// Итог записи.
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    pub path: String,
    pub mtime: f64,
}

/// Запись только поверх существующего cfg (`write`). Перевод строки приводится к `\n`, потом,
/// если нужно, к `\r\n`; размер считается в байтах после этого.
pub fn write(
    install_path: &Path,
    relative: &str,
    content: &str,
    options: &WriteOptions,
) -> Result<Written, CfgFilesError> {
    let (full, path) = resolve(install_path, relative)?;
    let stat = std::fs::metadata(&full).map_err(|_| cfg_error(CfgFileCode::NotFound))?;
    if let Some(expected) = options.expected_mtime
        && (mtime_ms(&stat).map_err(|e| RawError::fs(&e, "stat", &full))? - expected).abs() > 1.0
    {
        return Err(cfg_error(CfgFileCode::Changed));
    }
    let text = content.replace("\r\n", "\n");
    let with_eol = if options.crlf {
        text.replace('\n', "\r\n")
    } else {
        text
    };
    let buffer = encode(&with_eol, options.encoding);
    if buffer.len() as u64 > MAX_FILE_BYTES {
        return Err(cfg_error(CfgFileCode::TooLarge));
    }
    if options.encoding == Encoding::Latin1 && decode(&buffer, Encoding::Latin1) != with_eol {
        return Err(cfg_error(CfgFileCode::Encoding));
    }
    write_file_atomic(&full, &buffer)?;
    let stat = std::fs::metadata(&full).map_err(|e| RawError::fs(&e, "stat", &full))?;
    Ok(Written {
        path,
        mtime: mtime_ms(&stat).map_err(|e| RawError::fs(&e, "stat", &full))?,
    })
}
