//! Файлы JSON 0.4.7: чтение с повторами, атомарная запись, карантин битых файлов;
//! `atomic.js` доставки (`writeFileAtomic`, `writeJsonAtomic`, `readJsonState`).

use crate::js::{self, errno};
use crate::json_text::{JsonFormat, parse_js_json, to_js_json};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const READ_ATTEMPTS: usize = 4;
const RETRY_PAUSE: Duration = Duration::from_millis(40);

/// Состояние файла, как `readState` в `Store.js`.
#[derive(Debug, Clone, PartialEq)]
pub enum ReadState {
    /// Файла нет: умолчания уместны.
    Missing,
    /// Прочитан, корень — объект.
    Valid(Value),
    /// Есть, но не JSON-объект: чинится карантином.
    Unparsable,
    /// Есть, но не прочитан: содержимое неизвестно, писать поверх нельзя.
    Unreadable { code: String },
}

/// Прочитать файл. «Занятые» ошибки повторяются до 4 раз с паузой 40 мс.
pub fn read_state(path: &Path) -> ReadState {
    let mut attempt = 0;
    loop {
        match js::fs::read_file(path) {
            Ok(bytes) => {
                // Как readFileSync(…, "utf8"): неверные байты — U+FFFD, не ошибка.
                return match parse_js_json(&String::from_utf8_lossy(&bytes)) {
                    Ok(value @ Value::Object(_)) => ReadState::Valid(value),
                    _ => ReadState::Unparsable,
                };
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return ReadState::Missing,
            Err(error) => {
                attempt += 1;
                if !is_busy(&error) || attempt == READ_ATTEMPTS {
                    return ReadState::Unreadable {
                        code: error_code(&error),
                    };
                }
                std::thread::sleep(RETRY_PAUSE);
            }
        }
    }
}

/// Записать атомарно: временный файл, `fsync`, переименование поверх. Формат `Store` при любой
/// ошибке убирает временный `<файл>.tmp`; формат `Atomic` пишет как `atomic.js`
/// ([`write_file_atomic_staged`]).
pub fn write_state(path: &Path, value: &Value, format: JsonFormat) -> io::Result<()> {
    match format {
        JsonFormat::Atomic => write_file_atomic(path, to_js_json(value, format).as_bytes()),
        JsonFormat::Store => {
            let dir = parent_dir(path);
            fs::create_dir_all(dir)?;
            let mut temp = path.as_os_str().to_owned();
            temp.push(".tmp");
            write_and_rename(
                Path::new(&temp),
                path,
                to_js_json(value, format).as_bytes(),
                true,
            )
        }
    }
}

/// Имя временного файла атомарной записи `atomic.js`: `.<имя>.<pid>.<время36>.tmp`
/// (`name` — имя целевого файла без каталога). Формула одна на всех, кто пишет так же.
pub fn atomic_temp_name(name: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    format!(".{name}.{}.{}.tmp", std::process::id(), js::base36(millis))
}

/// Этап `writeFileAtomic`, на котором запись сорвалась.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicStage {
    /// `fs.mkdirSync(dir, { recursive: true })`.
    Mkdir,
    /// `fs.openSync(temp, "w")`.
    Open,
    /// `fs.writeFileSync(handle, contents)`.
    Write,
    /// `fs.fsyncSync(handle)`.
    Fsync,
    /// `fs.renameSync(temp, file)`.
    Rename,
}

impl AtomicStage {
    /// Системный вызов в тексте ошибки Node (`mkdir`, `open`, `write`, `fsync`, `rename`).
    pub fn syscall(self) -> &'static str {
        match self {
            AtomicStage::Mkdir => "mkdir",
            AtomicStage::Open => "open",
            AtomicStage::Write => "write",
            AtomicStage::Fsync => "fsync",
            AtomicStage::Rename => "rename",
        }
    }
}

/// Сбой атомарной записи: этап, ошибка ОС, каталог файла и временный файл.
#[derive(Debug)]
pub struct AtomicError {
    pub stage: AtomicStage,
    pub error: io::Error,
    /// Каталог целевого файла (его создавал этап `Mkdir`).
    pub dir: PathBuf,
    /// Временный файл рядом с целевым.
    pub temp: PathBuf,
}

/// `writeFileAtomic` из `atomic.js` с этапом сбоя: каталог создаётся, байты пишутся во
/// временный `.<имя>.<pid>.<время36>.tmp` рядом, `fsync`, переименование поверх. Права — по
/// умолчанию. Как в 0.4.7, временный файл убирается только при сбое переименования: сбой
/// `write` или `fsync` оставляет его.
pub fn write_file_atomic_staged(path: &Path, contents: &[u8]) -> Result<(), AtomicError> {
    let dir = parent_dir(path);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = dir.join(atomic_temp_name(&name));
    let fail = |stage, error| AtomicError {
        stage,
        error,
        dir: dir.to_path_buf(),
        temp: temp.clone(),
    };

    fs::create_dir_all(dir).map_err(|error| fail(AtomicStage::Mkdir, error))?;
    let mut handle = fs::File::create(&temp).map_err(|error| fail(AtomicStage::Open, error))?;
    let written = handle
        .write_all(contents)
        .map_err(|error| fail(AtomicStage::Write, error))
        .and_then(|()| {
            handle
                .sync_all()
                .map_err(|error| fail(AtomicStage::Fsync, error))
        });
    drop(handle);
    written?;

    fs::rename(&temp, path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        fail(AtomicStage::Rename, error)
    })
}

/// [`write_file_atomic_staged`] без этапа: для тех, кому нужна только ошибка ОС.
pub fn write_file_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_file_atomic_staged(path, contents).map_err(|failure| failure.error)
}

/// Файл, прочитанный как `readJsonState` из `atomic.js`: без повторов, корень — любой JSON.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonState {
    /// Файла нет (`found: false`).
    Missing,
    /// Прочитан и разобран (`valid: true`), BOM срезан.
    Valid(Value),
    /// Есть, но не читается или не JSON (`found: true, valid: false`).
    Invalid { reason: String },
}

impl JsonState {
    /// `data`: значение или `null`, как `readJsonSafe`.
    pub fn data(&self) -> Value {
        match self {
            JsonState::Valid(value) => value.clone(),
            JsonState::Missing | JsonState::Invalid { .. } => Value::Null,
        }
    }
}

/// `readJsonState` из `atomic.js`.
pub fn read_json_state(path: &Path) -> JsonState {
    let bytes = match js::fs::read_file(path) {
        Ok(bytes) => bytes,
        Err(error) if errno::code(&error).as_deref() == Some("ENOENT") => {
            return JsonState::Missing;
        }
        Err(error) => {
            return JsonState::Invalid {
                reason: errno::fs_message(&error, "open", &path.to_string_lossy(), None),
            };
        }
    };
    // Как readFileSync(…, "utf8"): неверные байты — U+FFFD, не ошибка.
    match parse_js_json(&String::from_utf8_lossy(&bytes)) {
        Ok(value) => JsonState::Valid(value),
        Err(error) => JsonState::Invalid {
            reason: error.to_string(),
        },
    }
}

/// `readJsonSafe` из `atomic.js`: значение или `null`.
pub fn read_json_safe(path: &Path) -> Value {
    read_json_state(path).data()
}

/// Отложить битый файл: `<файл>.broken-<время ISO UTC, где : и . заменены на ->`.
pub fn quarantine(path: &Path, now: SystemTime) -> io::Result<PathBuf> {
    let stamp = chrono::DateTime::<chrono::Utc>::from(now).format("%Y-%m-%dT%H-%M-%S-%3fZ");
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".broken-{stamp}"));
    let target = PathBuf::from(name);
    fs::rename(path, &target)?;
    Ok(target)
}

fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn write_and_rename(temp: &Path, path: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    let result = write_temp(temp, bytes, private).and_then(|()| fs::rename(temp, path));
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn write_temp(temp: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        // 0600: в Linux настройки и статистика не видны другим пользователям.
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut file = options.open(temp)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Ошибки, после которых 0.4.7 повторяет чтение (коды Node: на Windows libuv
/// переводит ERROR_ACCESS_DENIED в EPERM, ERROR_SHARING_VIOLATION — в EBUSY).
fn is_busy(error: &io::Error) -> bool {
    matches!(
        errno::code(error).as_deref(),
        Some("EBUSY" | "EPERM" | "EACCES" | "EMFILE" | "ENFILE" | "EAGAIN")
    )
}

/// Код ошибки, как его показывает Node (`EBUSY`, `EACCES`…), для журнала.
fn error_code(error: &io::Error) -> String {
    errno::code(error).map_or_else(|| error.kind().to_string(), std::borrow::Cow::into_owned)
}
