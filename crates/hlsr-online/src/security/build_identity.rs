//! `security/build-identity.js`: отпечаток сборки для онлайна — `X-HLSR-Build =
//! <платформа>/<версия>/<sha256 файла>` — и участки файла для ответа на вызов сервера.
//!
//! Отступление от 0.4.7 (спецификация, решение 5): объект проверки — исполняемый файл, а не
//! `app.asar`. Windows — `HLSR.exe` (`current_exe()`), Linux — `/proc/self/exe`: тот файл, что
//! запущен, даже если его уже заменило обновление. Хеш считается один раз за запуск; отпечаток
//! файла (`размер:mtime`) снимается до чтения, и по изменённому файлу участки не отдаются. Коды
//! ошибок (`ASAR_*`) и `dev`/`missing` в заголовке — как в 0.4.7.

use crate::js_value::safe_integer;
use hlsr_core::js;
use hlsr_core::js::errno;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;
use tokio::sync::OnceCell;

/// Больше куска, который нарезают скрипты выпуска.
pub const MAX_RANGE: usize = 256 * 1024;

/// `process.platform`: `win32` или `linux`.
pub fn platform() -> &'static str {
    if cfg!(windows) { "win32" } else { "linux" }
}

/// Исполняемый файл, который проверяется.
pub fn running_executable() -> PathBuf {
    if cfg!(windows) {
        std::env::current_exe().unwrap_or_else(|_| PathBuf::from("HLSR.exe"))
    } else {
        PathBuf::from("/proc/self/exe")
    }
}

/// `fingerprint(stat)`: `<размер>:<trunc(mtimeMs)>`.
pub fn fingerprint(meta: &Metadata) -> String {
    let mtime_ms = meta
        .modified()
        .ok()
        .map(|time| match time.duration_since(UNIX_EPOCH) {
            Ok(after) => after.as_millis() as i128,
            Err(before) => -(before.duration().as_millis() as i128),
        })
        .unwrap_or(0);
    format!("{}:{mtime_ms}", meta.len())
}

/// Ошибка `readRange`: текст исключения и его `code`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct RangeError {
    pub code: Option<String>,
    pub message: String,
}

impl RangeError {
    fn plain(message: &str) -> RangeError {
        RangeError {
            code: None,
            message: message.to_owned(),
        }
    }

    fn io(error: &std::io::Error, syscall: &str, path: &Path) -> RangeError {
        RangeError {
            code: errno::code(error).map(|code| code.into_owned()),
            message: errno::fs_message(error, syscall, &path.to_string_lossy(), None),
        }
    }
}

struct Inner {
    version: String,
    platform: String,
    packaged: bool,
    file: PathBuf,
    header: OnceCell<String>,
    /// Отпечаток, снятый перед подсчётом хеша (`this.seen`).
    seen: Mutex<Option<String>>,
}

/// Отпечаток сборки; копии — один отпечаток.
#[derive(Clone)]
pub struct BuildIdentity {
    inner: Arc<Inner>,
}

impl BuildIdentity {
    /// Отпечаток запущенного лаунчера: эта платформа, этот исполняемый файл.
    pub fn new(version: impl Into<String>, packaged: bool) -> BuildIdentity {
        BuildIdentity::with_file(version, platform(), packaged, running_executable())
    }

    /// Отпечаток заданного файла (тесты, сверка).
    pub fn with_file(
        version: impl Into<String>,
        platform: impl Into<String>,
        packaged: bool,
        file: impl Into<PathBuf>,
    ) -> BuildIdentity {
        BuildIdentity {
            inner: Arc::new(Inner {
                version: version.into(),
                platform: platform.into(),
                packaged,
                file: file.into(),
                header: OnceCell::new(),
                seen: Mutex::new(None),
            }),
        }
    }

    /// `packaged`: сборка выпуска (у сборки разработки вместо хеша — `dev`).
    pub fn packaged(&self) -> bool {
        self.inner.packaged
    }

    pub fn file(&self) -> &Path {
        &self.inner.file
    }

    fn seen(&self) -> Option<String> {
        self.inner
            .seen
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// `warm()`: посчитать хеш заранее — лаунчер может обновиться, пока открыт.
    pub fn warm(&self) {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let identity = self.clone();
            runtime.spawn(async move {
                identity.header().await;
            });
        }
    }

    /// `hashAsar()`: отпечаток до чтения, затем SHA-256 всего файла.
    fn hash_file(file: &Path) -> (Option<String>, std::io::Result<String>) {
        let seen = std::fs::metadata(file).ok().map(|meta| fingerprint(&meta));
        let hash = (|| {
            let mut handle = File::open(file)?;
            let mut hash = Sha256::new();
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                let read = handle.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hash.update(&buffer[..read]);
            }
            Ok(js::text::hex(&hash.finalize()))
        })();
        (seen, hash)
    }

    /// `header()`: значение `X-HLSR-Build`. Сборка разработки — `dev`, файл не читается —
    /// `missing` (сервер такой хеш не знает и считает сборку изменённой).
    pub async fn header(&self) -> String {
        self.inner
            .header
            .get_or_init(|| async {
                let sha = if self.inner.packaged {
                    let file = self.inner.file.clone();
                    let (seen, hash) = tokio::task::spawn_blocking(move || Self::hash_file(&file))
                        .await
                        .unwrap_or((None, Err(std::io::Error::other("hash"))));
                    *self
                        .inner
                        .seen
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner()) = seen;
                    hash.unwrap_or_else(|_| "missing".to_owned())
                } else {
                    "dev".to_owned()
                };
                format!("{}/{}/{sha}", self.inner.platform, self.inner.version)
            })
            .await
            .clone()
    }

    /// `readRange(offset, length)`: участок файла для ответа на вызов. Неверный участок —
    /// `ASAR_BAD_RANGE`, хеша нет — `ASAR_UNREADABLE`, файл изменился после подсчёта хеша —
    /// `ASAR_CHANGED` (с кодом), файл короче — `ASAR_SHORT`.
    pub async fn read_range(&self, offset: &Value, length: &Value) -> Result<Vec<u8>, RangeError> {
        let (Some(offset), Some(length)) = (safe_integer(Some(offset)), safe_integer(Some(length)))
        else {
            return Err(RangeError::plain("ASAR_BAD_RANGE"));
        };
        if offset < 0 || length <= 0 || length as usize > MAX_RANGE {
            return Err(RangeError::plain("ASAR_BAD_RANGE"));
        }
        self.header().await;
        let Some(seen) = self.seen() else {
            return Err(RangeError::plain("ASAR_UNREADABLE"));
        };
        let file = &self.inner.file;
        let mut handle = File::open(file).map_err(|error| RangeError::io(&error, "open", file))?;
        let meta = handle
            .metadata()
            .map_err(|error| RangeError::io(&error, "fstat", file))?;
        if fingerprint(&meta) != seen {
            return Err(RangeError {
                code: Some("ASAR_CHANGED".to_owned()),
                message: "ASAR_CHANGED".to_owned(),
            });
        }
        let mut buffer = vec![0u8; length as usize];
        let read = read_at(&mut handle, &mut buffer, offset as u64)
            .map_err(|error| RangeError::io(&error, "read", file))?;
        if read != buffer.len() {
            return Err(RangeError::plain("ASAR_SHORT"));
        }
        Ok(buffer)
    }
}

/// Прочитать с позиции, сколько есть (до конца буфера или файла).
fn read_at(handle: &mut File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::io::{Seek, SeekFrom};
    handle.seek(SeekFrom::Start(offset))?;
    let mut total = 0;
    while total < buffer.len() {
        match handle.read(&mut buffer[total..])? {
            0 => break,
            read => total += read,
        }
    }
    Ok(total)
}
