//! Буфер обмена: канал `clipboard-write-text` 0.4.7 (`AppManager.js:534-541`, мост `preload.js`
//! `clipboard.writeText`). В 0.4.7 текст копировал main; здесь копирует вид
//! (`cx.write_to_clipboard`), а вызов канала остаётся ради журнала окна.

/// Предел длины текста: 1 МиБ кодовых единиц UTF-16 (`text.length > 1024 * 1024`).
pub const MAX_TEXT_UNITS: usize = 1024 * 1024;

/// Текст длиннее [`MAX_TEXT_UNITS`]: `TOO_LARGE`.
pub fn is_too_large(text: &str) -> bool {
    text.encode_utf16().nth(MAX_TEXT_UNITS).is_some()
}

/// Канал моста `hlsrElectron.clipboard.writeText` 0.4.7 (`preload.js:241-246`): копировал main,
/// и вызов виден в журнале окна (в сценах — как в журнале Electron).
pub const BRIDGE_CHANNEL: &str = "clipboard-write-text";

/// Аргументы вызова [`BRIDGE_CHANNEL`] для `writeText(text)`; текст длиннее [`MAX_TEXT_UNITS`]
/// мост 0.4.7 отклоняет сам, без вызова, — `None`. Вызов — без ожидания ответа (main отвечает тем
/// же пределом).
pub fn bridge_args(text: &str) -> Option<Vec<serde_json::Value>> {
    (!is_too_large(text)).then(|| vec![serde_json::Value::String(text.to_owned())])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_rejects_text_over_the_limit() {
        let limit = "я".repeat(MAX_TEXT_UNITS);
        assert_eq!(
            bridge_args(&limit).map(|args| args.len()),
            Some(1),
            "ровно предел — можно"
        );
        assert!(bridge_args(&format!("{limit}x")).is_none());
        // Суррогатная пара — две единицы UTF-16.
        assert!(is_too_large(&"😀".repeat(MAX_TEXT_UNITS / 2 + 1)));
    }
}
