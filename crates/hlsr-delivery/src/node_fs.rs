//! Чтение и запись текстовых файлов, как `fs.readFileSync(file, encoding)` и
//! `fs.writeFileSync(file, text, encoding)` Node: кодировка `latin1` (байт ↔ символ
//! U+00XX; символ выше U+00FF теряет старший байт) или `utf8` (недопустимые байты читаются
//! как U+FFFD). Ошибки — [`RawError`] с кодом Node (`ENOENT`, `EISDIR`…): по `error.code`
//! 0.4.7 решает, что делать дальше.

use crate::atomic::fd_error;
use crate::errors::RawError;
use crate::game_binds::Encoding;
use hlsr_core::js::fs::read_file;
use hlsr_core::js::{self, errno};
use std::path::Path;

/// Байты → текст в кодировке.
pub fn decode(bytes: &[u8], encoding: Encoding) -> String {
    match encoding {
        Encoding::Latin1 => js::latin1_string(bytes),
        Encoding::Utf8 => js::utf8_string(bytes),
    }
}

/// Текст → байты в кодировке.
pub fn encode(text: &str, encoding: Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Latin1 => js::latin1_bytes(text),
        Encoding::Utf8 => text.as_bytes().to_vec(),
    }
}

/// `fs.readFileSync(file)`: байты файла. Каталог вместо файла: `EISDIR` при чтении.
pub fn read_bytes(file: &Path) -> Result<Vec<u8>, RawError> {
    read_file(file).map_err(|error| {
        if errno::code(&error).as_deref() == Some("EISDIR") {
            // Ошибка `read`, а не `open`: у неё в Node нет пути ни в свойствах, ни в тексте.
            return fd_error(&error, "read");
        }
        RawError::fs(&error, "open", file)
    })
}

/// То же, но «файла нет» (`ENOENT`) — `None`; прочие ошибки — ошибка.
pub fn read_bytes_optional(file: &Path) -> Result<Option<Vec<u8>>, RawError> {
    match read_bytes(file) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.code.as_deref() == Some("ENOENT") => Ok(None),
        Err(error) => Err(error),
    }
}

/// `fs.readFileSync(file, encoding)`. Каталог вместо файла: `EISDIR` при чтении.
pub fn read_text(file: &Path, encoding: Encoding) -> Result<String, RawError> {
    read_bytes(file).map(|bytes| decode(&bytes, encoding))
}

/// То же, но «файла нет» (`ENOENT`) — `None`; прочие ошибки — ошибка.
pub fn read_text_optional(file: &Path, encoding: Encoding) -> Result<Option<String>, RawError> {
    match read_text(file, encoding) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.code.as_deref() == Some("ENOENT") => Ok(None),
        Err(error) => Err(error),
    }
}

/// `fs.writeFileSync(file, text, encoding)`: файл создаётся или обрезается.
pub fn write_text(file: &Path, text: &str, encoding: Encoding) -> Result<(), RawError> {
    write_bytes(file, &encode(text, encoding))
}

/// `fs.writeFileSync(file, bytes)`: файл создаётся или обрезается. Ошибка открытия — с путём
/// (`EACCES: …, open '<путь>'`), ошибка записи — без него (`ENOSPC: …, write`), как в Node.
pub fn write_bytes(file: &Path, bytes: &[u8]) -> Result<(), RawError> {
    use std::io::Write as _;
    let mut handle =
        std::fs::File::create(file).map_err(|error| RawError::fs(&error, "open", file))?;
    handle
        .write_all(bytes)
        .map_err(|error| fd_error(&error, "write"))
}

/// `fs.mkdirSync(dir, { recursive: true })`.
pub fn mkdir_recursive(dir: &Path) -> Result<(), RawError> {
    std::fs::create_dir_all(dir).map_err(|error| RawError::fs(&error, "mkdir", dir))
}

/// `fs.copyFileSync(from, to)`.
pub fn copy_file(from: &Path, to: &Path) -> Result<(), RawError> {
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|error| RawError::fs_with_dest(&error, "copyfile", from, Some(to)))
}

/// `fs.unlinkSync(file)`.
pub fn unlink(file: &Path) -> Result<(), RawError> {
    std::fs::remove_file(file).map_err(|error| RawError::fs(&error, "unlink", file))
}
