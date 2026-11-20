//! Установка HLSR в Windows (`windows-install/install.js`). Установка — копирование файлов сборки
//! в каталог программы (по умолчанию `C:\HLSR\Launcher`) по списку `install-files.json` со сверкой
//! размера и SHA-256 каждого файла.
//!
//! Замена атомарная. Новая версия собирается рядом, в `.Launcher.hlsr-new-<8hex>`, затем
//! установленный каталог переименовывается в `.Launcher.hlsr-old-<8hex>`, а новый встаёт на его
//! место. Пока открыт хоть один файл старой версии, Windows не даёт переименовать каталог, и рабочая
//! установка остаётся целой. Пользовательские файлы прежнего каталога переносятся в новый жёсткими
//! ссылками (или копией); файлы прежней версии по её списку и деинсталлятор NSIS — нет.
//!
//! Тексты ошибок, предупреждений и журнала — дословно из 0.4.7; тексты ошибок ОС — как у Node
//! (`hlsr_core::js::errno`), не байт в байт.

use super::file_list::{self, FileItem, FileList};
use super::file_ops;
use super::layout::{self, EnvLookup, Identity, win_path};
use super::payload::Source;
use super::processes::{Command, Env, Processes};
use super::registry::{self, InstallEntry, Registry};
use super::shortcuts::{Shortcuts, wants_desktop_shortcut};
use crate::cancel::CancelToken;
use hlsr_core::js::errno;
use hlsr_core::logger::Logger;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Маркер библиотеки, которую создал сам установщик (`C:\HLSR\.hlsr\library.json`): полное удаление
/// уносит её свои папки целиком (`library_cleanup`).
pub const LIBRARY_MARKER: &str = r#"{"schema":1,"id":"hlsr-library","createdBy":"installer"}"#;
/// Маркер уже существовавшей папки (её мог завести человек) — без отметки (0.4.9).
pub const LIBRARY_MARKER_PLAIN: &str = r#"{"schema":1,"id":"hlsr-library"}"#;
/// Без этих файлов лаунчер работает, не будет только оверлея LiveSplit: файлы, которые не прочитать
/// (их мог унести антивирус), установку не останавливают (0.4.9).
pub const OPTIONAL_PREFIX: &str = "resources/livesplit-overlay/";
/// `BUILTIN\Users` в виде SID: имя группы зависит от языка Windows, SID — нет.
pub const USERS_SID: &str = "*S-1-5-32-545";
/// Повторы переименования при обновлении: прежняя версия ещё завершается (60 × 1 с).
pub const UPDATE_RENAME: (u32, Duration) = (60, Duration::from_secs(1));
/// Повторы переименования при установке человеком (5 × 400 мс).
pub const MANUAL_RENAME: (u32, Duration) = (5, Duration::from_millis(400));

/// Код ошибки установки (`InstallError.code`); ошибки без своего кода несут код Node, если он есть.
pub mod code {
    pub const ABORTED: &str = "ABORTED";
    pub const COPY_FAILED: &str = "COPY_FAILED";
    pub const CORRUPT: &str = "CORRUPT";
    pub const PRESERVE_FAILED: &str = "PRESERVE_FAILED";
    pub const RECOVERY_REQUIRED: &str = "RECOVERY_REQUIRED";
    pub const BAD_DIR: &str = "BAD_DIR";
    pub const IN_USE: &str = "IN_USE";
    pub const SWAP_FAILED: &str = "SWAP_FAILED";
    /// Файл забрал антивирус (0.4.9).
    pub const ANTIVIRUS: &str = "ANTIVIRUS";
}

/// Ошибка установки: код (`BAD_DIR`, `IN_USE`… или код Node у ошибок ФС) и сообщение для окна.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct InstallError {
    pub code: Option<String>,
    pub message: String,
    /// Файл списка, на котором остановилось копирование (`error.file`, 0.4.9).
    pub file: Option<String>,
    /// Исходная ошибка ОС (`error.cause`, 0.4.9): по её коду окно объясняет, что случилось.
    pub cause: Option<Box<InstallError>>,
}

impl InstallError {
    pub fn new(code: &str, message: impl Into<String>) -> InstallError {
        InstallError {
            code: Some(code.to_owned()),
            message: message.into(),
            file: None,
            cause: None,
        }
    }

    /// Ошибка без кода (`new Error(message)`).
    pub fn plain(message: impl Into<String>) -> InstallError {
        InstallError {
            code: None,
            message: message.into(),
            file: None,
            cause: None,
        }
    }

    /// Ошибка ФС, как её бросает `fs` Node: код и `EXXX: описание, syscall '<путь>'`.
    pub fn fs(error: &io::Error, syscall: &str, path: &Path) -> InstallError {
        InstallError {
            code: errno::code(error).map(|code| code.into_owned()),
            message: errno::fs_message(error, syscall, &path.to_string_lossy(), None),
            file: None,
            cause: None,
        }
    }

    fn with_cause(mut self, cause: InstallError) -> InstallError {
        self.cause = Some(Box::new(cause));
        self
    }

    fn aborted() -> InstallError {
        InstallError::new(code::ABORTED, "Установка отменена")
    }

    pub fn is(&self, wanted: &str) -> bool {
        self.code.as_deref() == Some(wanted)
    }
}

impl From<InstallError> for crate::installer_window::BackendError {
    fn from(error: InstallError) -> Self {
        let cause = error.cause.map(|cause| *cause);
        crate::installer_window::BackendError {
            message: error.message,
            code: error.code,
            file: error.file,
            cause_code: cause.as_ref().and_then(|cause| cause.code.clone()),
            cause_message: cause.map(|cause| cause.message),
        }
    }
}

/// Что лежит на месте каталога установки (`existingKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingKind {
    None,
    Empty,
    /// Наш: есть `HLSR.exe` или маркер владельца.
    Hlsr,
    /// Ссылка, файл или непустой каталог без наших файлов.
    Foreign,
}

impl ExistingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ExistingKind::None => "none",
            ExistingKind::Empty => "empty",
            ExistingKind::Hlsr => "hlsr",
            ExistingKind::Foreign => "foreign",
        }
    }
}

/// `existingKind(dir)`.
pub fn existing_kind(dir: &Path) -> io::Result<ExistingKind> {
    let stat = match std::fs::symlink_metadata(dir) {
        Ok(stat) => stat,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(ExistingKind::None),
        Err(error) => return Err(error),
    };
    if stat.file_type().is_symlink() || !stat.is_dir() {
        return Ok(ExistingKind::Foreign);
    }
    let names: Vec<String> = hlsr_core::js::fs::read_dir_names(dir)?
        .into_iter()
        .map(|name| name.to_lowercase())
        .collect();
    if names.is_empty() {
        return Ok(ExistingKind::Empty);
    }
    let exe = layout::EXE.to_lowercase();
    if names
        .iter()
        .any(|name| *name == exe || name == layout::OWNER_MARKER)
    {
        return Ok(ExistingKind::Hlsr);
    }
    Ok(ExistingKind::Foreign)
}

/// Настоящий каталог, если `dir` — ссылка (junction, symlink) на каталог; иначе `None`.
fn linked_dir(dir: &str) -> Option<String> {
    let link = std::fs::symlink_metadata(dir)
        .ok()?
        .file_type()
        .is_symlink();
    let target_is_dir = std::fs::metadata(dir).is_ok_and(|stat| stat.is_dir());
    (link && target_is_dir)
        .then(|| crate::uninstall::library_cleanup::real_path(dir).ok())
        .flatten()
}

/// `/^\.(.+)\.hlsr-(?:new|old)-[0-9a-f]{8}$/`: имя каталога, остаток которого — остаток прерванной
/// установки; возвращает захваченное имя каталога программы.
pub fn leftover_name(entry: &str) -> Option<&str> {
    let rest = entry.strip_prefix('.')?;
    // Хвост `.hlsr-new-<8hex>` — 18 байт ASCII; имя с не-ASCII в последних 18 байтах (кириллица
    // в соседнем каталоге) не подходит и не должно резаться посреди символа.
    let cut = rest.len().checked_sub(18).filter(|cut| *cut > 0)?;
    if !rest.is_char_boundary(cut) {
        return None;
    }
    let (head, tail) = rest.split_at(cut);
    let tail = tail.as_bytes();
    let kind_ok = tail.starts_with(b".hlsr-new-") || tail.starts_with(b".hlsr-old-");
    let hex_ok = tail[10..]
        .iter()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
    (kind_ok && hex_ok).then_some(head)
}

fn is_old(entry: &str) -> bool {
    entry.contains(".hlsr-old-")
}

/// Остатки прерванных установок рядом с каталогом программы (`sweepLeftovers`): `.hlsr-new-*`
/// удаляются; `.hlsr-old-*` остаются — в них могут быть единственные копии пользовательских файлов
/// после прерванной замены; без каталога программы рядом это `RECOVERY_REQUIRED`.
pub fn sweep_leftovers(parent: &Path, name: &str, log: &dyn Logger) -> Result<(), InstallError> {
    let Ok(entries) = hlsr_core::js::fs::read_dir_names(parent) else {
        return Ok(());
    };
    let path = win_path();
    let parent_text = parent.to_string_lossy();
    let ours = |entry: &str| {
        leftover_name(entry).is_some_and(|captured| captured.to_lowercase() == name.to_lowercase())
    };
    for entry in entries.iter().filter(|entry| ours(entry) && is_old(entry)) {
        let full = path.join(&[&parent_text, entry]);
        if !parent.join(name).exists() {
            return Err(InstallError::new(
                code::RECOVERY_REQUIRED,
                format!(
                    "Сохранилась прежняя установка: {full}. Восстановите её файлы перед обновлением."
                ),
            ));
        }
        log.warn(&format!("[setup] резервная папка сохранена: {full}"));
    }
    for entry in entries.iter().filter(|entry| ours(entry) && !is_old(entry)) {
        let full = parent.join(entry);
        if let Err(error) = remove_all(&full) {
            let message = errno::fs_message(&error, "rm", &full.to_string_lossy(), None);
            log.warn(&format!("[setup] не удалось убрать {entry}: {message}"));
        }
    }
    Ok(())
}

/// `fs.rmSync(target, { recursive: true, force: true })`: нет — не ошибка; ссылки удаляются сами,
/// без перехода.
pub fn remove_all(target: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(target) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(stat) if stat.file_type().is_symlink() => {
            std::fs::remove_file(target).or_else(|_| std::fs::remove_dir(target))
        }
        Ok(stat) if stat.is_dir() => std::fs::remove_dir_all(target),
        Ok(_) => std::fs::remove_file(target),
    }
}

/// Файлы прежней версии по её списку (нижний регистр), если список этой же сборки. У NSIS списка
/// нет: неизвестные файлы сохраняются — это могут быть игры, конфиги и демо.
fn installed_files(root: &Path, id: &Identity) -> HashSet<String> {
    file_list::read(root)
        .ok()
        .filter(|list| layout::identity(Some(&list.app)).is_ok_and(|other| other.guid == id.guid))
        .map(|list| {
            list.files
                .iter()
                .map(|item| item.path.to_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

fn join_parts(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
    }
    path
}

fn preserve_failed(message: impl Into<String>) -> InstallError {
    InstallError::new(code::PRESERVE_FAILED, message)
}

struct Preserve<'a> {
    root: &'a Path,
    stage: &'a Path,
    old_files: HashSet<String>,
    new_files: HashSet<String>,
    metadata: HashSet<String>,
    obsolete: HashSet<String>,
    nested_library: Option<String>,
    cancel: Option<&'a CancelToken>,
}

impl Preserve<'_> {
    fn walk(&self, relative: &str) -> Result<(), InstallError> {
        let dir = if relative.is_empty() {
            self.root.to_path_buf()
        } else {
            join_parts(self.root, relative)
        };
        let entries = hlsr_core::js::fs::read_dir(&dir)
            .map_err(|error| InstallError::fs(&error, "scandir", &dir))?;
        for entry in entries {
            if self.cancel.is_some_and(CancelToken::is_cancelled) {
                return Err(InstallError::aborted());
            }
            let child = if relative.is_empty() {
                entry.name.clone()
            } else {
                format!("{relative}/{}", entry.name)
            };
            let from = join_parts(self.root, &child);
            let to = join_parts(self.stage, &child);
            let from_text = from.to_string_lossy().into_owned();
            let key = child.to_lowercase();
            let kind = entry.file_type;
            let library_file = self
                .nested_library
                .as_deref()
                .is_some_and(|nested| layout::is_inside(&from_text, nested));
            if self.new_files.contains(&key) {
                if library_file || kind.is_dir() || kind.is_symlink() {
                    return Err(preserve_failed(format!(
                        "Файлы пользователя пересекаются с новой версией: {from_text}. Переместите их за пределы папки лаунчера."
                    )));
                }
                continue;
            }
            if self.metadata.contains(&key) {
                if kind.is_dir() || library_file {
                    return Err(preserve_failed(format!(
                        "Служебный файл новой версии занимает папку пользователя: {from_text}"
                    )));
                }
                continue;
            }
            // Если прежний файл заменили личной папкой или ссылкой, список прошлой версии уже не даёт
            // права удалить её содержимое.
            if !library_file
                && kind.is_file()
                && (self.obsolete.contains(&key) || self.old_files.contains(&key))
            {
                continue;
            }
            let destination = match std::fs::symlink_metadata(&to) {
                Ok(stat) => Some(stat),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(InstallError::fs(&error, "lstat", &to)),
            };
            if kind.is_dir() {
                if destination
                    .as_ref()
                    .is_some_and(|stat| !stat.is_dir() || stat.file_type().is_symlink())
                {
                    return Err(preserve_failed(format!(
                        "Новая версия занимает папку пользователя: {from_text}"
                    )));
                }
                std::fs::create_dir_all(&to)
                    .map_err(|error| InstallError::fs(&error, "mkdir", &to))?;
                self.walk(&child)?;
                continue;
            }
            if destination.is_some() {
                return Err(preserve_failed(format!(
                    "Новая версия занимает файл пользователя: {from_text}"
                )));
            }
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| InstallError::fs(&error, "mkdir", parent))?;
            }
            if kind.is_symlink() {
                let link = file_ops::read_link(&from)
                    .map_err(|error| InstallError::fs(&error, "readlink", &from))?;
                // Сохраняем и оборванную ссылку.
                let directory = std::fs::metadata(&from).is_ok_and(|stat| stat.is_dir());
                file_ops::symlink_like_node(&link, &to, directory)
                    .map_err(|error| InstallError::fs(&error, "symlink", &to))?;
            } else if kind.is_file() {
                // На FAT/exFAT и некоторых сетевых дисках жёстких ссылок нет; копирование тоже не
                // перезапишет чужой файл стадии.
                if std::fs::hard_link(&from, &to).is_err() {
                    copy_exclusive(&from, &to)
                        .map_err(|error| InstallError::fs(&error, "copyfile", &from))?;
                }
            } else {
                return Err(preserve_failed(format!(
                    "Не удалось сохранить объект пользователя: {from_text}"
                )));
            }
        }
        Ok(())
    }
}

/// `fs.copyFile(from, to, COPYFILE_EXCL)`.
fn copy_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    let mut input = std::fs::File::open(from)?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)?;
    io::copy(&mut input, &mut output)?;
    Ok(())
}

/// `preserveUserFiles(root, stage, list, id, libraryRoot, signal)`: пользовательские файлы прежнего
/// каталога — в стадию новой версии. Вложенная библиотека игр переносится целиком.
pub fn preserve_user_files(
    root: &Path,
    stage: &Path,
    list: &FileList,
    id: &Identity,
    library_root: &str,
    cancel: Option<&CancelToken>,
) -> Result<(), InstallError> {
    let root_text = root.to_string_lossy();
    let nested_library = (layout::is_inside(library_root, &root_text)
        && !layout::is_same_dir(&root_text, library_root))
    .then(|| win_path().resolve(&[library_root]));
    let preserve = Preserve {
        root,
        stage,
        old_files: installed_files(root, id),
        new_files: list
            .files
            .iter()
            .map(|item| item.path.to_lowercase())
            .collect(),
        metadata: [
            layout::OWNER_MARKER.to_lowercase(),
            file_list::RELATIVE.to_lowercase(),
        ]
        .into(),
        obsolete: [
            format!("uninstall {}.exe", id.product_name).to_lowercase(),
            "uninstallericon.ico".to_owned(),
        ]
        .into(),
        nested_library,
        cancel,
    };
    preserve.walk("").map_err(|error| {
        if error.is(code::PRESERVE_FAILED) || error.is(code::ABORTED) {
            error
        } else {
            preserve_failed(format!(
                "Пользовательские файлы не перенесены; прежняя установка сохранена: {}",
                error.message
            ))
            .with_cause(error)
        }
    })
}

/// `renameWithRetry(from, to, { attempts, delayMs, wait })`: занятый каталог (`EBUSY`, `EPERM`,
/// `EACCES`, `ENOTEMPTY`) — повтор через `delay`.
pub fn rename_with_retry(
    from: &Path,
    to: &Path,
    (attempts, delay): (u32, Duration),
    sleep: &dyn Fn(Duration),
) -> io::Result<()> {
    let mut attempt = 1;
    loop {
        match file_ops::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) => {
                let busy = errno::code(&error).is_some_and(|code| {
                    matches!(code.as_ref(), "EBUSY" | "EPERM" | "EACCES" | "ENOTEMPTY")
                });
                if attempt >= attempts || !busy {
                    return Err(error);
                }
                sleep(delay);
                attempt += 1;
            }
        }
    }
}

/// `copyVerified`: файл списка из источника в стадию, создаётся заново (`wx`); размер и SHA-256
/// сверяются со списком.
fn copy_verified(
    source: &Source,
    stage: &Path,
    item: &FileItem,
    cancel: Option<&CancelToken>,
) -> Result<u64, InstallError> {
    let to = join_parts(stage, &item.path);
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| InstallError::fs(&error, "mkdir", parent))?;
    }
    // Файл забрал антивирус: Windows не даёт его открыть (EPERM, EACCES, EBUSY), libuv не знает
    // кодов ERROR_VIRUS_INFECTED и ERROR_VIRUS_DELETED (UNKNOWN), или файл источника пропал. Источник
    // и стадия — свои свежие каталоги, поэтому отказ доступа здесь — не права папки (0.4.9).
    let source_gone = || match source {
        Source::Dir(dir) => !join_parts(dir, &item.path).exists(),
        Source::Payload(payload) => !payload.exe().exists(),
    };
    let copy_failed = |error: io::Error, path: &Path| {
        let cause = InstallError::fs(&error, "open", path);
        let blocked = matches!(
            cause.code.as_deref(),
            Some("UNKNOWN" | "EPERM" | "EACCES" | "EBUSY")
        ) || cause.message.to_lowercase().contains("virus")
            || source_gone();
        // Сырой текст ошибки — только в журнал: окно объясняет блокировку своими словами.
        let failure = if blocked {
            InstallError::new(
                code::ANTIVIRUS,
                format!(
                    "Антивирус заблокировал или удалил файл {} ({})",
                    item.path,
                    cause.code.as_deref().unwrap_or("нет файла")
                ),
            )
        } else {
            InstallError::new(
                code::COPY_FAILED,
                format!("Не удалось скопировать {}: {}", item.path, cause.message),
            )
        };
        InstallError {
            file: Some(item.path.clone()),
            ..failure
        }
        .with_cause(cause)
    };
    let mut input = source
        .open(item)
        .map_err(|error| copy_failed(error, &join_parts(&source.location(), &item.path)))?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&to)
        .map_err(|error| copy_failed(error, &to))?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        if cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(InstallError::aborted());
        }
        let read = input
            .read(&mut buffer)
            .map_err(|error| copy_failed(error, &to))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
        output
            .write_all(&buffer[..read])
            .map_err(|error| copy_failed(error, &to))?;
    }
    output.flush().map_err(|error| copy_failed(error, &to))?;
    let sha256 = hlsr_core::js::text::hex(&hasher.finalize());
    if size != item.size || sha256 != item.sha256 {
        return Err(InstallError::new(
            code::CORRUPT,
            format!(
                "Файл установщика повреждён: {}. Скачайте установщик заново.",
                item.path
            ),
        ));
    }
    Ok(size)
}

/// Раскладка библиотеки, как её создавал прежний установщик (`ensureLibraryLayout`): каталоги
/// `Games`, `Downloads`, `.hlsr` только создаются, маркер пишется, если его нет; игры и чужие поля
/// не трогаются. `created` — папку библиотеки создал сам установщик (маркер с `createdBy`). Ошибки
/// не роняют установку — лаунчер создаст всё сам. `icacls` даёт обычному пользователю права на
/// изменение, даже если каталог когда-то создал администратор.
pub fn ensure_library_layout(
    root: &str,
    created: bool,
    processes: Option<&dyn Processes>,
    env: EnvLookup<'_>,
    log: &dyn Logger,
) {
    let path = win_path();
    let laid_out = (|| -> io::Result<()> {
        for dir in ["", "Games", "Downloads", ".hlsr"] {
            std::fs::create_dir_all(path.join(&[root, dir]))?;
        }
        let hlsr = PathBuf::from(path.join(&[root, ".hlsr"]));
        let marker = hlsr.join("library.json");
        // Через ссылку на месте .hlsr маркер не пишется: он попал бы в чужую папку.
        if !std::fs::symlink_metadata(&hlsr)?.file_type().is_symlink() && !marker.exists() {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&marker)?
                .write_all(
                    if created {
                        LIBRARY_MARKER
                    } else {
                        LIBRARY_MARKER_PLAIN
                    }
                    .as_bytes(),
                )?;
        }
        Ok(())
    })();
    if let Err(error) = laid_out {
        let message = errno::fs_message(&error, "mkdir", root, None);
        log.warn(&format!("[setup] раскладка библиотеки: {message}"));
        return;
    }
    let Some(processes) = processes else {
        return;
    };
    let get = |name: &str| env(name).filter(|value| !value.is_empty());
    let system_root = get("SystemRoot")
        .or_else(|| get("WINDIR"))
        .unwrap_or_else(|| "C:\\Windows".to_owned());
    let command = Command {
        file: path.join(&[&system_root, "System32", "icacls.exe"]),
        args: vec![
            root.to_owned(),
            "/grant".to_owned(),
            format!("{USERS_SID}:(OI)(CI)M"),
        ],
        cwd: None,
        env: Env::from_process(),
        hidden: true,
    };
    let status = processes.run(&command, Some(Duration::from_secs(60)));
    if status != Some(0) {
        let shown = status.map_or_else(|| "null".to_owned(), |code| code.to_string());
        log.warn(&format!("[setup] icacls {root}: код {shown}"));
    }
}

/// Параметры установки (`options` `install`).
pub struct InstallOptions<'a> {
    pub source: &'a Source,
    pub install_dir: &'a str,
    pub registry: &'a dyn Registry,
    pub shortcuts: &'a Shortcuts,
    /// `signal`: отмена разрешена, пока прежняя установка не затронута.
    pub cancel: Option<&'a CancelToken>,
    /// `env` для `refusedDir` и пути `icacls` (по умолчанию `process.env`).
    pub env: EnvLookup<'a>,
    /// Запуск обновлятором: переименование ждёт 60 × 1 с, ярлыки только восстанавливаются.
    pub update: bool,
    /// Выбор человека по ярлыку на рабочем столе.
    pub desktop_shortcut: Option<bool>,
    /// Имя установщика: метка `-ru.exe`/`-en.exe`.
    pub setup_file: Option<&'a str>,
    pub library_root: &'a str,
    pub log: &'a dyn Logger,
    pub sleep: &'a dyn Fn(Duration),
    /// `onProgress(done, total)` после каждого файла.
    pub on_progress: Option<&'a mut dyn FnMut(u64, u64)>,
    /// `onCommit()`: дальше отмены нет.
    pub on_commit: Option<&'a mut dyn FnMut()>,
    /// `run` для `icacls`; `None` — права не выдаются.
    pub processes: Option<&'a dyn Processes>,
}

fn real_sleep(duration: Duration) {
    std::thread::sleep(duration);
}

impl<'a> InstallOptions<'a> {
    /// Параметры по умолчанию: `process.env`, установка человеком без выбора ярлыка, библиотека
    /// `C:\HLSR`, настоящий `sleep`, без прогресса и без `icacls`.
    pub fn new(
        source: &'a Source,
        install_dir: &'a str,
        registry: &'a dyn Registry,
        shortcuts: &'a Shortcuts,
        log: &'a dyn Logger,
    ) -> InstallOptions<'a> {
        InstallOptions {
            source,
            install_dir,
            registry,
            shortcuts,
            cancel: None,
            env: &layout::process_env,
            update: false,
            desktop_shortcut: None,
            setup_file: None,
            library_root: layout::LIBRARY_ROOT,
            log,
            sleep: &real_sleep,
            on_progress: None,
            on_commit: None,
            processes: None,
        }
    }
}

/// Итог установки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallResult {
    pub install_dir: String,
    pub install_id: String,
    pub version: String,
    pub identity: Identity,
    pub warnings: Vec<String>,
}

/// Значение `REG_SZ` раздела (`values.NAME` у `parseRegFile`).
pub fn sz_value(values: &[(String, registry::RegValue)], name: &str) -> Option<String> {
    let wanted = name.to_lowercase();
    values
        .iter()
        .find(|(candidate, _)| candidate.to_lowercase() == wanted)
        .and_then(|(_, value)| value.as_sz())
        .map(str::to_owned)
}

/// `install(options)`.
pub fn install(mut options: InstallOptions<'_>) -> Result<InstallResult, InstallError> {
    let path = win_path();
    let log = options.log;
    let cancelled = |cancel: Option<&CancelToken>| cancel.is_some_and(CancelToken::is_cancelled);

    if let Some(refused) = layout::refused_dir(options.install_dir, options.env) {
        return Err(InstallError::new(
            code::BAD_DIR,
            format!("Нельзя установить HLSR сюда: {refused}"),
        ));
    }
    let source = path.resolve(&[&options.source.guard().to_string_lossy()]);
    let target = path.resolve(&[options.install_dir]);
    // Отступление от 0.4.9: каталог программы — ссылка на каталог (лаунчер перенесли на другой диск
    // и оставили junction). Ставим в настоящий каталог: иначе он «чужой» (BAD_DIR), а замена
    // переименовала бы саму ссылку и положила новую версию рядом с ней.
    let target = match linked_dir(&target) {
        Some(real) => {
            if let Some(refused) = layout::refused_dir(&real, options.env) {
                return Err(InstallError::new(
                    code::BAD_DIR,
                    format!("Нельзя установить HLSR сюда: {refused}"),
                ));
            }
            log.info(&format!("[setup] {target} — ссылка, установка в {real}"));
            real
        }
        None => target,
    };
    if layout::is_inside(&source, &target) || layout::is_inside(&target, &source) {
        return Err(InstallError::new(
            code::BAD_DIR,
            "Установщик запущен из каталога установки",
        ));
    }

    let list = options
        .source
        .read_list()
        .map_err(|error| InstallError::plain(error.to_string()))?;
    let id = layout::identity(Some(&list.app))
        .map_err(|error| InstallError::plain(error.to_string()))?;
    let target_path = PathBuf::from(&target);
    let kind = existing_kind(&target_path)
        .map_err(|error| InstallError::fs(&error, "lstat", &target_path))?;
    if kind == ExistingKind::Foreign {
        return Err(InstallError::new(
            code::BAD_DIR,
            format!("Каталог {target} занят другими файлами — HLSR туда не ставится"),
        ));
    }

    let parent = PathBuf::from(path.dirname(&target));
    let name = path.basename(&target);
    // Была ли папка библиотеки до установщика (C:\HLSR — она же родитель каталога программы).
    let library_created = !Path::new(options.library_root).exists();
    // Родитель создаётся, только если его нет: для D:\hlsr это корень диска, и создание корня
    // диска в Windows падает с EPERM.
    if !parent.exists() {
        std::fs::create_dir_all(&parent)
            .map_err(|error| InstallError::fs(&error, "mkdir", &parent))?;
    }
    sweep_leftovers(&parent, &name, log)?;

    let values = options.registry.read(&id.install_key).unwrap_or_default();
    let install_id = sz_value(&values, "HLSRInstallId")
        .filter(|value| layout::is_install_id(value))
        .unwrap_or_else(file_ops::new_install_id);

    let stage = parent.join(format!(".{name}.hlsr-new-{}", file_ops::random_suffix()));
    let total = list.total_bytes();
    let mut skipped = Vec::new();
    let swapped = (|| -> Result<(), InstallError> {
        std::fs::create_dir(&stage).map_err(|error| InstallError::fs(&error, "mkdir", &stage))?;
        let mut done = 0;
        for item in &list.files {
            if cancelled(options.cancel) {
                return Err(InstallError::aborted());
            }
            match copy_verified(options.source, &stage, item, options.cancel) {
                Ok(size) => done += size,
                Err(error) => {
                    if let Some(cause) = &error.cause {
                        log.warn(&format!("[setup] {}: {}", error.message, cause.message));
                    }
                    let copy = error.is(code::COPY_FAILED) || error.is(code::ANTIVIRUS);
                    if !copy || !item.path.starts_with(OPTIONAL_PREFIX) {
                        return Err(error);
                    }
                    let _ = remove_all(&join_parts(&stage, &item.path));
                    skipped.push(path.basename(&item.path));
                    done += item.size;
                }
            }
            if let Some(progress) = options.on_progress.as_mut() {
                progress(done, total);
            }
        }
        if !matches!(kind, ExistingKind::None | ExistingKind::Empty) {
            preserve_user_files(
                &target_path,
                &stage,
                &list,
                &id,
                options.library_root,
                options.cancel,
            )?;
        }
        if cancelled(options.cancel) {
            return Err(InstallError::aborted());
        }
        // Отмена разрешена, пока прежняя установка ещё не затронута.
        if let Some(commit) = options.on_commit.as_mut() {
            commit();
        }
        // Список едет в установку: по нему удаление узнаёт identity сборки.
        file_list::write_new(&stage, &list).map_err(|error| {
            InstallError::fs(&error, "open", &join_parts(&stage, file_list::RELATIVE))
        })?;
        let marker = stage.join(layout::OWNER_MARKER);
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)
            .and_then(|mut file| file.write_all(id.owner_marker(&install_id).as_bytes()))
            .map_err(|error| InstallError::fs(&error, "open", &marker))?;

        let mut old = None;
        if kind != ExistingKind::None {
            let backup = parent.join(format!(".{name}.hlsr-old-{}", file_ops::random_suffix()));
            // Обновление приходит, пока прежняя версия ещё завершается: ждём.
            let policy = if options.update {
                UPDATE_RENAME
            } else {
                MANUAL_RENAME
            };
            if let Err(error) = rename_with_retry(&target_path, &backup, policy, options.sleep) {
                let shown =
                    errno::code(&error).map_or_else(|| "undefined".into(), |c| c.into_owned());
                return Err(InstallError::new(
                    code::IN_USE,
                    format!(
                        "Файлы HLSR заняты ({shown}). Закройте HLSR и всё, что открыто из {target}, и повторите."
                    ),
                )
                .with_cause(InstallError::fs(&error, "rename", &target_path)));
            }
            old = Some(backup);
        }
        if let Err(error) = file_ops::rename(&stage, &target_path) {
            if let Some(old) = &old {
                file_ops::rename(old, &target_path)
                    .map_err(|error| InstallError::fs(&error, "rename", old))?;
            }
            let message =
                errno::fs_message(&error, "rename", &stage.to_string_lossy(), Some(&target));
            return Err(InstallError::new(
                code::SWAP_FAILED,
                format!("Не удалось поставить новую версию: {message}"),
            )
            .with_cause(InstallError {
                code: errno::code(&error).map(|code| code.into_owned()),
                ..InstallError::plain(message)
            }));
        }
        if let Some(old) = old
            && let Err(error) = remove_all(&old)
        {
            let message = errno::fs_message(&error, "rm", &old.to_string_lossy(), None);
            log.warn(&format!(
                "[setup] прежняя версия осталась в {}: {message}",
                old.display()
            ));
        }
        Ok(())
    })();
    let _ = remove_all(&stage);
    swapped?;

    let mut warnings = Vec::new();
    if !skipped.is_empty() {
        warnings.push(format!(
            "Оверлей LiveSplit работать не будет: не удалось скопировать {}, файлы мог заблокировать антивирус",
            skipped.join(", ")
        ));
    }
    let create_desktop = wants_desktop_shortcut(
        sz_value(&values, layout::DESKTOP_VALUE).as_deref(),
        options.update,
        options.desktop_shortcut,
    );
    let exe = path.join(&[&target, layout::EXE]);
    let origin = options.setup_file.and_then(registry::origin_of);
    let entry = InstallEntry {
        install_dir: &target,
        install_id: &install_id,
        desktop_shortcut: create_desktop,
        version: &list.version,
        size_bytes: total,
        origin,
    };
    if let Err(error) = registry::apply(options.registry, &registry::install_changes(&id, &entry)) {
        warnings.push(format!(
            "Запись в «Программы и компоненты» не создана: {error}"
        ));
    }
    // Ярлыки: при установке человеком — по его выбору, при обновлении недостающие
    // восстанавливаются, существующие не трогаются.
    warnings.extend(
        options
            .shortcuts
            .install(&exe, options.update, create_desktop),
    );

    ensure_library_layout(
        options.library_root,
        library_created,
        options.processes,
        options.env,
        log,
    );
    log.info(&format!(
        "[setup] HLSR {} установлен в {target}",
        list.version
    ));
    Ok(InstallResult {
        install_dir: target,
        install_id,
        version: list.version,
        identity: id,
        warnings,
    })
}
