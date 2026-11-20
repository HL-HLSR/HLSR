//! Атомарная запись файлов доставки (`atomic.js`): `writeFileAtomic` и `writeJsonAtomic`.
//! Запись и имя временного файла — в `hlsr_core::json_store` (`write_file_atomic_staged`,
//! `atomic_temp_name`); здесь этап сбоя переводится в исключение Node (`RawError`: код,
//! системный вызов, путь, текст): `mkdir '<каталог>'`, `open '<временный файл>'`,
//! `write`/`fsync` без пути (операции над дескриптором), `rename '<временный файл>' -> '<файл>'`.
//! Чтение (`readJsonState`, `readJsonSafe`) — там же.

use crate::errors::RawError;
use hlsr_core::js::errno;
use hlsr_core::json_store::{AtomicError, AtomicStage, write_file_atomic_staged};
use hlsr_core::json_text::stringify;
use serde_json::Value;
use std::io;
use std::path::Path;

/// Ошибка операции над дескриптором: у Node в тексте нет пути (`ENOSPC: …, write`).
pub(crate) fn fd_error(error: &io::Error, syscall: &str) -> RawError {
    let code = errno::code(error).map(std::borrow::Cow::into_owned);
    let message = match &code {
        Some(code) => format!("{code}: {}, {syscall}", errno::description(code)),
        None => error.to_string(),
    };
    RawError {
        code,
        syscall: Some(syscall.to_owned()),
        path: None,
        message,
        stderr: None,
    }
}

/// Сбой записи `file` на этапе `failure.stage` → исключение Node для этого этапа.
pub fn raw_error(failure: &AtomicError, file: &Path) -> RawError {
    let syscall = failure.stage.syscall();
    match failure.stage {
        AtomicStage::Mkdir => RawError::fs(&failure.error, syscall, &failure.dir),
        AtomicStage::Open => RawError::fs(&failure.error, syscall, &failure.temp),
        AtomicStage::Write | AtomicStage::Fsync => fd_error(&failure.error, syscall),
        AtomicStage::Rename => {
            RawError::fs_with_dest(&failure.error, syscall, &failure.temp, Some(file))
        }
    }
}

/// `writeFileAtomic(file, contents)`: каталог создаётся, байты пишутся во временный файл
/// рядом, `fsync`, переименование поверх.
pub fn write_file_atomic(file: &Path, contents: &[u8]) -> Result<(), RawError> {
    write_file_atomic_staged(file, contents).map_err(|failure| raw_error(&failure, file))
}

/// `writeJsonAtomic(file, data)`: два пробела и перевод строки в конце.
pub fn write_json_atomic(file: &Path, data: &Value) -> Result<(), RawError> {
    write_file_atomic(file, (stringify(data, "  ") + "\n").as_bytes())
}
