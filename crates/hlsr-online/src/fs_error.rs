//! Ошибки записи файлов в виде исключения Node (`RawError`), общего для доставки и онлайна.

use hlsr_core::json_formats::WriteError;
use hlsr_delivery::errors::RawError;

/// Сбой [`hlsr_core::json_formats`] → `RawError`: код, системный вызов, путь и текст исключения.
pub(crate) fn raw_error(failure: &WriteError) -> RawError {
    RawError {
        code: failure.code().map(std::borrow::Cow::into_owned),
        syscall: Some(failure.stage.syscall().to_owned()),
        path: Some(failure.path.to_string_lossy().into_owned()),
        message: failure.message(),
        stderr: None,
    }
}
