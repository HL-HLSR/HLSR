//! Помощники Win32, общие для крейтов лаунчера.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

/// Строка для `…W`-функций Win32: UTF-16 с нулём в конце. `str`, `OsStr` и `Path` — одинаково
/// (у строки Rust `encode_wide` даёт то же, что `encode_utf16`).
pub fn wide<S: AsRef<OsStr> + ?Sized>(text: &S) -> Vec<u16> {
    text.as_ref().encode_wide().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn wide_is_utf16_with_a_trailing_nul() {
        assert_eq!(wide(""), [0]);
        assert_eq!(wide("open"), [111, 112, 101, 110, 0]);
        let text = "C:\\Игры\\HLSR 😀";
        let expected: Vec<u16> = text.encode_utf16().chain([0]).collect();
        assert_eq!(wide(text), expected);
        assert_eq!(wide(&text.to_owned()), expected);
        assert_eq!(wide(Path::new(text)), expected);
        assert_eq!(wide(OsStr::new(text)), expected);
    }
}
