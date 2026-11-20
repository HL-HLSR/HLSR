//! Операции с файлами установки (`file-ops.js`): SHA-256 файла, копирование и
//! удаление с `FILE_LOCKED`, путь внутри каталога, «игра запущена», уборка
//! пустых каталогов.

use crate::errors::{Code, DeliveryError, Failure, RawError};
use crate::paths;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{Platform, errno};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncReadExt;

/// Коды Node, при которых файл считается занятым.
pub const LOCK_CODES: [&str; 4] = ["EBUSY", "EPERM", "EACCES", "ETXTBSY"];

/// `isLockError(error)`.
pub fn is_lock_error(error: &RawError) -> bool {
    error
        .code
        .as_deref()
        .is_some_and(|code| LOCK_CODES.contains(&code))
}

/// `wrapLockError(error, relativePath)`: занятый файл → `FILE_LOCKED`, иначе `None`.
pub fn wrap_lock_error(error: &RawError, relative_path: &str) -> Option<DeliveryError> {
    is_lock_error(error).then(|| {
        DeliveryError::new(
            Code::FileLocked,
            format!("Файл занят другим процессом: {relative_path}. Закройте игру и повторите."),
        )
        .with_details(json!({"path": relative_path, "cause": error.code}))
    })
}

/// Строка из байтов хеша.
pub use hlsr_core::js::text::hex;

/// `hashFile(file)`: SHA-256 в hex, чтение блоками по 1 МиБ.
pub async fn hash_file(file: &Path) -> Result<String, RawError> {
    let mut handle = tokio::fs::File::open(file)
        .await
        .map_err(|e| RawError::fs(&e, "open", file))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = handle
            .read(&mut buffer)
            .await
            .map_err(|e| RawError::fs(&e, "read", file))?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hex(&hash.finalize()))
}

/// `exists(file)`.
pub async fn exists(file: &Path) -> bool {
    matches!(tokio::fs::try_exists(file).await, Ok(true))
}

fn label(target: &Path, relative_path: Option<&str>) -> String {
    relative_path
        .filter(|r| !r.is_empty())
        .map_or_else(|| target.to_string_lossy().into_owned(), str::to_owned)
}

fn locked_or_raw(raw: RawError, label: &str) -> Failure {
    match wrap_lock_error(&raw, label) {
        Some(locked) => Failure::Delivery(locked),
        None => Failure::Raw(raw),
    }
}

/// `copyFileSync(source, target, relativePath)`: каталог создаётся; занятый файл — `FILE_LOCKED`.
pub fn copy_file_sync(
    source: &Path,
    target: &Path,
    relative_path: Option<&str>,
) -> Result<(), Failure> {
    if let Some(dir) = target.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| RawError::fs(&e, "mkdir", dir))?;
    }
    fs::copy(source, target).map_err(|e| {
        let raw = RawError::fs_with_dest(&e, "copyfile", source, Some(target));
        locked_or_raw(raw, &label(target, relative_path))
    })?;
    Ok(())
}

/// `makeWritableSync(file)`. Windows: файл с атрибутом «только чтение» нельзя удалить или
/// заменить (EPERM), а копирование этот атрибут переносит на копию. Ссылку не трогаем: атрибут
/// цели вне папки игры остаётся; нет файла — нечего снимать.
pub fn make_writable(file: &Path) {
    #[cfg(windows)]
    if let Ok(meta) = fs::symlink_metadata(file)
        && !meta.file_type().is_symlink()
    {
        let mut permissions = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        let _ = fs::set_permissions(file, permissions);
    }
    #[cfg(not(windows))]
    let _ = file;
}

/// `removeFileSync(target, relativePath)`: `fs.rmSync(target, { force: true })`;
/// занятый файл — `FILE_LOCKED`.
pub fn remove_file_sync(target: &Path, relative_path: Option<&str>) -> Result<(), Failure> {
    make_writable(target);
    rm_sync(target, false, 0, Duration::ZERO)
        .map_err(|raw| locked_or_raw(raw, &label(target, relative_path)))
}

/// `resolveInside(root, relativePath, what)` на этой ОС.
pub fn resolve_inside(
    root: &Path,
    relative_path: &str,
    what: Option<&str>,
) -> Result<PathBuf, DeliveryError> {
    resolve_inside_with(
        &NodePath::native(),
        &root.to_string_lossy(),
        relative_path,
        what,
    )
    .map(PathBuf::from)
}

/// `resolveInside`: `root` + путь с `/`, не выходя за `root`.
pub fn resolve_inside_with(
    path: &NodePath,
    root: &str,
    relative_path: &str,
    what: Option<&str>,
) -> Result<String, DeliveryError> {
    let normalized = relative_path.replace('/', path.sep());
    let default_what = format!("Путь \"{relative_path}\"");
    let what = what.filter(|w| !w.is_empty()).unwrap_or(&default_what);
    paths::assert_inside_with(path, root, &path.join(&[root, &normalized]), Some(what))
}

/// `path.join(installPath, relative.replace(/\//g, path.sep))`.
fn install_file(install_path: &Path, relative: &str) -> String {
    let path = NodePath::native();
    path.join(&[
        &install_path.to_string_lossy(),
        &relative.replace('/', path.sep()),
    ])
}

/// `isGameRunning(installPath, executables)`: путь exe (как в списке), если игра
/// запущена. Windows не даёт открыть запущенный exe на запись; Linux его не
/// запирает — ищется командная строка процессов этого пользователя в `/proc`.
pub fn is_game_running(install_path: &Path, executables: &[&str]) -> Option<String> {
    let candidates: &[&str] = if executables.is_empty() {
        &["hl2.exe"]
    } else {
        executables
    };
    match Platform::CURRENT {
        Platform::Win32 => game_running_windows(install_path, candidates),
        Platform::Linux => game_running_posix(install_path, candidates),
    }
}

fn game_running_windows(install_path: &Path, candidates: &[&str]) -> Option<String> {
    for relative in candidates {
        let target = install_file(install_path, relative);
        if !Path::new(&target).exists() {
            continue;
        }
        if let Err(error) = fs::OpenOptions::new().read(true).write(true).open(&target)
            && errno::code(&error).is_some_and(|code| LOCK_CODES.contains(&code.as_ref()))
        {
            return Some((*relative).to_owned());
        }
    }
    None
}

#[cfg(unix)]
fn game_running_posix(install_path: &Path, candidates: &[&str]) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let pids: Vec<String> = fs::read_dir("/proc")
        .ok()?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()))
        .collect();
    let uid = rustix::process::getuid().as_raw();
    for relative in candidates {
        let target = install_file(install_path, relative);
        if !Path::new(&target).exists() {
            continue;
        }
        for pid in &pids {
            // Процесс мог успеть завершиться — тогда пропускается.
            let dir = Path::new("/proc").join(pid);
            if !fs::metadata(&dir).is_ok_and(|meta| meta.uid() == uid) {
                continue;
            }
            let Ok(cmdline) = fs::read(dir.join("cmdline")) else {
                continue;
            };
            if String::from_utf8_lossy(&cmdline)
                .split('\0')
                .any(|arg| arg == target)
            {
                return Some((*relative).to_owned());
            }
        }
    }
    None
}

#[cfg(not(unix))]
fn game_running_posix(_install_path: &Path, _candidates: &[&str]) -> Option<String> {
    None
}

/// `pruneEmptyDirs(root, startDir)`: удалить пустые каталоги от `startDir` вверх
/// до `root` (сам `root` остаётся). В Windows «внутри» — по префиксу без учёта регистра.
pub fn prune_empty_dirs(root: &Path, start_dir: &Path) {
    let path = NodePath::native();
    let stop = path.resolve(&[&root.to_string_lossy()]);
    let mut current = path.resolve(&[&start_dir.to_string_lossy()]);
    let inside = |value: &str| match path.platform() {
        Platform::Win32 => value.to_lowercase().starts_with(&stop.to_lowercase()),
        Platform::Linux => value.starts_with(&format!("{stop}{}", path.sep())),
    };
    while current != stop && inside(&current) {
        let empty = fs::read_dir(&current).is_ok_and(|mut items| items.next().is_none());
        if !empty {
            return;
        }
        if fs::remove_dir(&current).is_err() {
            return;
        }
        match Path::new(&current).parent() {
            Some(parent) => current = parent.to_string_lossy().into_owned(),
            None => return,
        }
    }
}

// ------------------------------------------------------------ fs.rmSync Node 24
//
// Node 24 удаляет через std::filesystem libc++ (Electron собран с libc++ Chromium) и
// сообщает ошибку из C++. Поведение снято с Electron 0.4.7 на Linux и под Wine
// (`tools/reference/node-semantics-win32.js` → `tests/fixtures/fs-win32.json`).

/// Как libc++ `std::filesystem` понимает ошибку ОС (`std::errc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(windows, allow(dead_code))]
enum RmCondition {
    NotFound,
    PermissionDenied,
    OperationNotPermitted,
    DirectoryNotEmpty,
    NotADirectory,
    /// EBUSY, EMFILE, ENFILE: повторяется.
    Busy,
    Other,
}

#[cfg(unix)]
fn rm_condition(error: &io::Error) -> RmCondition {
    match errno::code(error).as_deref() {
        Some("ENOENT") => RmCondition::NotFound,
        Some("EACCES") => RmCondition::PermissionDenied,
        Some("EPERM") => RmCondition::OperationNotPermitted,
        Some("ENOTEMPTY") => RmCondition::DirectoryNotEmpty,
        Some("ENOTDIR") => RmCondition::NotADirectory,
        Some("EBUSY" | "EMFILE" | "ENFILE") => RmCondition::Busy,
        _ => RmCondition::Other,
    }
}

/// Windows: `__win_err_to_errc` libc++. Сверено с Electron 0.4.7 под Wine
/// (`fs-win32.json`) только для 32 (ERROR_SHARING_VIOLATION, занятый exe) и 5
/// (ERROR_ACCESS_DENIED, «только для чтения») — «permission denied», без повторов.
/// Остальные — по таблице libc++ без проверки: «нет файла» 2, 3, 53, 123, 161
/// (до них rmSync не доходит, если файла нет: его отсекает проверка до удаления),
/// «permission denied» 12, 16, 19, 82, 998, «directory not empty» 145, «busy» 4, 142,
/// 170, 2401, 2404.
#[cfg(windows)]
fn rm_condition(error: &io::Error) -> RmCondition {
    match error.raw_os_error() {
        Some(2 | 3 | 53 | 123 | 161) => RmCondition::NotFound,
        Some(5 | 12 | 16 | 19 | 32 | 82 | 998) => RmCondition::PermissionDenied,
        Some(145) => RmCondition::DirectoryNotEmpty,
        Some(4 | 142 | 170 | 2401 | 2404) => RmCondition::Busy,
        _ => RmCondition::Other,
    }
}

/// Байты пути, которые C++-часть вставляет в текст (`path::string()`): UTF-8 в Linux.
#[cfg(unix)]
fn rm_path_bytes(target: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    target.as_os_str().as_bytes().to_vec()
}

/// Windows: путь после `toNamespacedPath` (`\\?\C:\…`) в кодовой странице ANSI.
#[cfg(windows)]
fn rm_path_bytes(target: &Path) -> Vec<u8> {
    use windows_sys::Win32::Globalization::{CP_ACP, WideCharToMultiByte};
    let namespaced = NodePath::native().to_namespaced_path(&target.to_string_lossy());
    let wide: Vec<u16> = namespaced.encode_utf16().collect();
    let Ok(units) = i32::try_from(wide.len()) else {
        return namespaced.into_bytes();
    };
    if units == 0 {
        return Vec::new();
    }
    let null = std::ptr::null_mut();
    // SAFETY: длины — от самих буферов; символ замены — системный по умолчанию.
    let size = unsafe {
        WideCharToMultiByte(
            CP_ACP,
            0,
            wide.as_ptr(),
            units,
            null,
            0,
            std::ptr::null(),
            null.cast(),
        )
    };
    let mut out = vec![0u8; usize::try_from(size).unwrap_or(0)];
    // SAFETY: буфер `out` — ровно `size` байт.
    let written = unsafe {
        WideCharToMultiByte(
            CP_ACP,
            0,
            wide.as_ptr(),
            units,
            out.as_mut_ptr(),
            size,
            std::ptr::null(),
            null.cast(),
        )
    };
    out.truncate(usize::try_from(written).unwrap_or(0));
    out
}

/// Ошибка `fs.rmSync` Node 24 — `ErrnoException`: `<код>, <текст>` (байты C++,
/// прочитанные как Latin-1) и ` '<путь>'` (те же байты как UTF-8).
fn rm_error(error: &io::Error, target: &Path) -> RawError {
    let bytes = rm_path_bytes(target);
    let latin1 = |bytes: &[u8]| -> String { bytes.iter().map(|&b| char::from(b)).collect() };
    let shown = latin1(&bytes);
    let permission_denied = if cfg!(windows) { "EPERM" } else { "EACCES" };
    let (code, text) = match rm_condition(error) {
        RmCondition::OperationNotPermitted => {
            ("EPERM", format!("Operation not permitted: {shown}"))
        }
        RmCondition::DirectoryNotEmpty => ("ENOTEMPTY", format!("Directory not empty: {shown}")),
        RmCondition::NotADirectory => ("ENOTDIR", format!("Not a directory: {shown}")),
        RmCondition::PermissionDenied => (permission_denied, format!("Permission denied: {shown}")),
        _ => {
            // Текст ОС (strerror) без приписки Rust « (os error N)»; код — пустой (UV_EUNKNOWN).
            let system = error.to_string();
            let system = system.split(" (os error").next().unwrap_or_default();
            ("", format!("Unknown error: {}", latin1(system.as_bytes())))
        }
    };
    let path = String::from_utf8_lossy(&bytes).into_owned();
    RawError {
        code: Some(code.to_owned()),
        syscall: Some("rm".to_owned()),
        message: format!("{code}, {text} '{path}'"),
        path: Some(path),
        stderr: None,
    }
}

/// Удалить файл или симлинк. В Windows, как у `std::filesystem::remove` libc++
/// (`FileDispositionInfo`), файл с атрибутом «только для чтения» не удаляется —
/// ERROR_ACCESS_DENIED; `std::fs::remove_file` Rust такой удалил бы.
fn remove_entry(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;
        const ERROR_ACCESS_DENIED: i32 = 5;
        let meta = fs::symlink_metadata(path)?;
        if meta.permissions().readonly() {
            return Err(io::Error::from_raw_os_error(ERROR_ACCESS_DENIED));
        }
        if meta.file_type().is_symlink_dir() {
            return fs::remove_dir(path);
        }
    }
    fs::remove_file(path)
}

/// `remove_all` libc++ в Linux (через `openat`): в каталоге удаляется всё, что
/// удаляется, — неудача вложенного не останавливает перебор (libc++ перезаписывает
/// общий `error_code` при каждом `readdir`), и итог — ошибка удаления самого
/// каталога (обычно ENOTEMPTY). Ошибка `readdir` останавливает перебор; если
/// каталог после неё всё же удалился, она и будет итогом. Симлинки не раскрываются.
#[cfg(unix)]
fn remove_tree(path: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.is_dir() {
        return fs::remove_file(path);
    }
    let mut read_error = None;
    for entry in fs::read_dir(path)? {
        match entry {
            Ok(entry) => {
                let _ = remove_tree(&entry.path());
            }
            Err(error) => {
                read_error = Some(error);
                break;
            }
        }
    }
    fs::remove_dir(path)?;
    read_error.map_or(Ok(()), Err)
}

/// `remove_all` libc++ в Windows: ошибка любого вложенного файла сразу прерывает
/// удаление (сверено под Wine: каталог с занятым exe и с файлом «только для чтения»).
#[cfg(windows)]
fn remove_tree(path: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.is_dir() {
        return remove_entry(path);
    }
    for entry in fs::read_dir(path)? {
        remove_tree(&entry?.path())?;
    }
    fs::remove_dir(path)
}

/// Пауза перед повтором `fs.rmSync` Node 24 (C++): `n × retryDelay / 1000` целочисленно —
/// в Linux это секунды (`sleep`), в Windows миллисекунды (`Sleep`). При 200 мс и
/// пяти повторах в Linux выходит 0, 0, 0, 0, 1, 1 с.
fn retry_pause(attempt: u64, retry_delay: Duration) -> Duration {
    let millis = u64::try_from(retry_delay.as_millis()).unwrap_or(u64::MAX);
    let units = attempt.saturating_mul(millis) / 1000;
    if cfg!(windows) {
        Duration::from_millis(units)
    } else {
        Duration::from_secs(units)
    }
}

/// `fs.rmSync(target, { force: true, recursive, maxRetries, retryDelay })` Node 24:
/// нет файла — не ошибка; каталог без `recursive` — `ERR_FS_EISDIR`; EBUSY, EMFILE,
/// ENFILE, ENOTEMPTY и EPERM повторяются до `max_retries` раз (пауза — [`retry_pause`]).
pub(crate) fn rm_sync(
    target: &Path,
    recursive: bool,
    max_retries: u32,
    retry_delay: Duration,
) -> Result<(), RawError> {
    if !recursive {
        // Проверки JS-части rmSync до вызова C++.
        match fs::symlink_metadata(target) {
            Err(error) if errno::code(&error).as_deref() == Some("ENOENT") => return Ok(()),
            Err(error) => return Err(RawError::fs(&error, "lstat", target)),
            Ok(meta) if meta.is_dir() => {
                let path = target.to_string_lossy().into_owned();
                return Err(RawError {
                    code: Some("ERR_FS_EISDIR".to_owned()),
                    syscall: Some("rm".to_owned()),
                    message: format!(
                        "Path is a directory: rm returned EISDIR (is a directory) {path}"
                    ),
                    path: Some(path),
                    stderr: None,
                });
            }
            Ok(_) => {}
        }
    }
    // C++ `fs::RmSync` начинает с `symlink_status`: он отдаёт `not_found` и для ENOENT, и для
    // ENOTDIR (часть пути — файл), а с `force` такой путь не ошибка.
    if recursive
        && let Err(error) = fs::symlink_metadata(target)
        && matches!(errno::code(&error).as_deref(), Some("ENOENT" | "ENOTDIR"))
    {
        return Ok(());
    }
    // while (maxRetries >= 0): пауза — после каждой повторяемой неудачи, и последней тоже.
    let mut retries_left = i64::from(max_retries);
    let mut attempt = 1;
    let error = loop {
        let result = if recursive {
            remove_tree(target)
        } else {
            remove_entry(target)
        };
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        let condition = rm_condition(&error);
        if condition == RmCondition::NotFound {
            return Ok(());
        }
        let retryable = matches!(
            condition,
            RmCondition::Busy | RmCondition::DirectoryNotEmpty | RmCondition::OperationNotPermitted
        );
        if !retryable {
            break error;
        }
        std::thread::sleep(retry_pause(attempt, retry_delay));
        retries_left -= 1;
        attempt += 1;
        if retries_left < 0 {
            break error;
        }
    };
    Err(rm_error(&error, target))
}
