//! Форматы JSON-файлов 0.4.7, которых нет в `json_text::JsonFormat` (`Store`, `Atomic`):
//!
//! | Формат | Запись 0.4.7 |
//! |---|---|
//! | Compact | `JSON.stringify(x)` в `<файл>.tmp`, затем `rename`; права 0600 или по умолчанию |
//! | Pretty2 | `JSON.stringify(x, null, 2)` без перевода строки, прямой `writeFileSync` |
//! | JSONL | строка `JSON.stringify(x) + "\n"`; дописывание и перезапись через `<файл>.<pid>.tmp` |
//!
//! Каждая функция повторяет последовательность вызовов `fs` модулей 0.4.7 (в комментарии —
//! откуда) и при сбое сообщает этап ([`WriteStage`]), по которому [`WriteError`] собирает
//! код и текст исключения Node. Временные файлы убирает тот, кто убирает их в 0.4.7:
//! [`rewrite_jsonl`] — сам, остальные — вызывающий (`WriteError::temp` — какой).

use crate::js::errno;
use crate::json_text::stringify;
use serde_json::Value;
use std::borrow::Cow;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

/// `JSON.stringify(value)`.
pub fn compact(value: &Value) -> String {
    stringify(value, "")
}

/// `JSON.stringify(value, null, 2)`: без перевода строки в конце.
pub fn pretty2(value: &Value) -> String {
    stringify(value, "  ")
}

/// Строка JSONL: `JSON.stringify(value) + "\n"`.
pub fn jsonl_line(value: &Value) -> String {
    compact(value) + "\n"
}

/// Этап записи, на котором она сорвалась.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteStage {
    /// `fs.mkdirSync(dirname, { recursive: true })`.
    Mkdir,
    /// Открытие файла (`writeFileSync`, `appendFileSync`).
    Open,
    /// Запись в открытый файл.
    Write,
    /// `fs.renameSync(temp, file)`.
    Rename,
}

impl WriteStage {
    /// Системный вызов в тексте ошибки Node.
    pub fn syscall(self) -> &'static str {
        match self {
            WriteStage::Mkdir => "mkdir",
            WriteStage::Open => "open",
            WriteStage::Write => "write",
            WriteStage::Rename => "rename",
        }
    }
}

/// Сбой записи: этап, ошибка ОС и пути, которые Node назвал бы в тексте.
#[derive(Debug)]
pub struct WriteError {
    pub stage: WriteStage,
    pub error: io::Error,
    /// Путь системного вызова: каталог (`Mkdir`), файл (`Open`), откуда (`Rename`).
    pub path: PathBuf,
    /// Куда переименовывали (`Rename`).
    pub dest: Option<PathBuf>,
    /// Временный файл записи (`<файл>.tmp`, `<файл>.<pid>.tmp`), если он был.
    pub temp: Option<PathBuf>,
}

impl WriteError {
    /// `error.code` исключения Node: `ENOTDIR`, `EISDIR`…
    pub fn code(&self) -> Option<Cow<'static, str>> {
        errno::code(&self.error)
    }

    /// `error.message`: `EISDIR: illegal operation on a directory, open '<путь>'`; у ошибки
    /// записи в открытый файл пути нет (`ENOSPC: no space left on device, write`).
    pub fn message(&self) -> String {
        let syscall = self.stage.syscall();
        match self.stage {
            WriteStage::Write => match errno::code(&self.error) {
                Some(code) => format!("{code}: {}, {syscall}", errno::description(&code)),
                None => self.error.to_string(),
            },
            _ => errno::fs_message(
                &self.error,
                syscall,
                &self.path.to_string_lossy(),
                self.dest
                    .as_deref()
                    .map(|dest| dest.to_string_lossy())
                    .as_deref(),
            ),
        }
    }
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for WriteError {}

/// Параметры [`write_compact_tmp`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TmpOptions {
    /// `{ mode: 0o600 }` (Unix): читает только владелец. Права действуют на новый файл —
    /// остаток `.tmp` прошлой записи сохраняет свои.
    pub private: bool,
    /// Перед записью удалить остаток `<файл>.tmp` (`SecureSession.write`).
    pub clear_stale: bool,
}

fn dirname(path: &Path) -> &Path {
    path.parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn temp_with(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn fail(stage: WriteStage, error: io::Error, path: &Path, temp: Option<&Path>) -> WriteError {
    WriteError {
        stage,
        error,
        path: path.to_path_buf(),
        dest: None,
        temp: temp.map(Path::to_path_buf),
    }
}

/// `fs.mkdirSync(dirname(path), { recursive: true })`.
fn make_parent(path: &Path, temp: Option<&Path>) -> Result<(), WriteError> {
    let dir = dirname(path);
    fs::create_dir_all(dir).map_err(|error| fail(WriteStage::Mkdir, error, dir, temp))
}

/// `fs.writeFileSync(file, bytes, { mode })` (флаг `w`) или `fs.appendFileSync` (флаг `a`):
/// ошибка открытия называет файл, ошибка записи — нет.
fn write_file(
    file: &Path,
    bytes: &[u8],
    private: bool,
    append: bool,
    temp: Option<&Path>,
) -> Result<(), WriteError> {
    let mut options = OpenOptions::new();
    options.create(true);
    if append {
        options.append(true);
    } else {
        options.write(true).truncate(true);
    }
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut handle = options
        .open(file)
        .map_err(|error| fail(WriteStage::Open, error, file, temp))?;
    handle
        .write_all(bytes)
        .map_err(|error| fail(WriteStage::Write, error, file, temp))
}

/// `fs.renameSync(from, to)`.
fn rename(from: &Path, to: &Path, temp: Option<&Path>) -> Result<(), WriteError> {
    fs::rename(from, to).map_err(|error| WriteError {
        stage: WriteStage::Rename,
        error,
        path: from.to_path_buf(),
        dest: Some(to.to_path_buf()),
        temp: temp.map(Path::to_path_buf),
    })
}

/// Pretty2, `consent.js` (`writeJson`): `mkdirSync(dirname, { recursive: true })`, затем
/// `writeFileSync(file, JSON.stringify(data, null, 2), "utf8")` — без временного файла.
pub fn write_pretty2(path: &Path, value: &Value) -> Result<(), WriteError> {
    make_parent(path, None)?;
    write_file(path, pretty2(value).as_bytes(), false, false, None)
}

/// Compact через `<файл>.tmp` (`SecureSession.write`, `OnlineProfileService.writeJson`,
/// `launch-streak.record`, `VersionEvents.writeState`): `mkdirSync(dirname, { recursive: true })`,
/// удаление остатка `.tmp` (если [`TmpOptions::clear_stale`]), `writeFileSync(temp,
/// JSON.stringify(data))`, `renameSync(temp, file)`. При сбое `.tmp` остаётся:
/// убирает вызывающий, если его модуль 0.4.7 это делает (`WriteError::temp`).
pub fn write_compact_tmp(
    path: &Path,
    value: &Value,
    options: TmpOptions,
) -> Result<(), WriteError> {
    let temp = temp_with(path, ".tmp");
    make_parent(path, Some(&temp))?;
    if options.clear_stale {
        let _ = fs::remove_file(&temp);
    }
    write_file(
        &temp,
        compact(value).as_bytes(),
        options.private,
        false,
        Some(&temp),
    )?;
    rename(&temp, path, Some(&temp))
}

/// Дописывание строки JSONL (`TelemetryQueue.append`): `mkdirSync(dirname, { recursive: true })`,
/// `appendFileSync(file, line + "\n", "utf8")`. `line` — уже `JSON.stringify(event)`: очередь
/// сначала проверяет её длину.
pub fn append_jsonl_line(path: &Path, line: &str) -> Result<(), WriteError> {
    make_parent(path, None)?;
    write_file(path, format!("{line}\n").as_bytes(), false, true, None)
}

/// Перезапись JSONL (`TelemetryQueue.rewrite`): `<файл>.<pid>.tmp` с `lines.join("\n") + "\n"`
/// (для пустого списка — пустой файл), `renameSync(temp, file)`. Без `mkdir`. При сбое
/// временный файл убирается, ошибка возвращается.
pub fn rewrite_jsonl(path: &Path, lines: &[String]) -> Result<(), WriteError> {
    let temp = temp_with(path, &format!(".{}.tmp", std::process::id()));
    let text = if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n") + "\n"
    };
    let result = write_file(&temp, text.as_bytes(), false, false, Some(&temp))
        .and_then(|()| rename(&temp, path, Some(&temp)));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
