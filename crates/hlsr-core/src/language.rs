//! Язык интерфейса: номер в `config.language`, как в 0.4.7 (0 — en, 1 — ru).

use crate::settings::Settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    En = 0,
    Ru = 1,
}

impl Language {
    pub fn index(self) -> i64 {
        self as i64
    }

    pub fn code(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::Ru => "ru",
        }
    }

    pub fn from_index(index: i64) -> Option<Language> {
        match index {
            0 => Some(Language::En),
            1 => Some(Language::Ru),
            _ => None,
        }
    }
}

/// Язык первого запуска, как `initial-language.js` 0.4.7: метка установщика
/// `ru`/`en` главнее; иначе язык системы `ru`, `ru-*`, `ru_*` → русский; иначе английский.
pub fn pick_initial(installer_origin: Option<&str>, system_locale: Option<&str>) -> Language {
    // 0.4.7 берёт из вывода `reg.exe query` первое слово после REG_SZ.
    let origin = installer_origin
        .and_then(|value| value.split_whitespace().next())
        .map(str::to_lowercase);
    match origin.as_deref() {
        Some("ru") => return Language::Ru,
        Some("en") => return Language::En,
        _ => {}
    }
    let locale = system_locale.unwrap_or_default().to_lowercase();
    if locale == "ru" || locale.starts_with("ru-") || locale.starts_with("ru_") {
        Language::Ru
    } else {
        Language::En
    }
}

/// Метка страницы загрузки: `HKCU\Software\HLSR`, значение `InstallerOrigin` (`REG_SZ`).
#[cfg(windows)]
pub fn installer_origin() -> Option<String> {
    use crate::win32::wide;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, RRF_NOEXPAND, RRF_RT_REG_SZ, RegGetValueW,
    };

    let subkey = wide("Software\\HLSR");
    let value = wide("InstallerOrigin");
    let mut buffer: Vec<u16> = Vec::new();
    // Значение может вырасти между запросом размера и чтением.
    for _ in 0..3 {
        let mut size = u32::try_from(buffer.len() * 2).ok()?;
        let data = if buffer.is_empty() {
            null_mut()
        } else {
            buffer.as_mut_ptr().cast()
        };
        // SAFETY: строки оканчиваются нулём; `data` — буфер на `size` байт или null.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                value.as_ptr(),
                // Только REG_SZ: без RRF_NOEXPAND проходит и REG_EXPAND_SZ.
                RRF_RT_REG_SZ | RRF_NOEXPAND,
                null_mut(),
                data,
                &mut size,
            )
        };
        if status == ERROR_SUCCESS && !buffer.is_empty() {
            let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
            return Some(String::from_utf16_lossy(&buffer[..len]));
        }
        if status != ERROR_SUCCESS && status != ERROR_MORE_DATA {
            return None;
        }
        buffer = vec![0; (size as usize).div_ceil(2).max(1)];
    }
    None
}

#[cfg(not(windows))]
pub fn installer_origin() -> Option<String> {
    None
}

/// Язык системы в виде BCP 47 (`ru-RU`), как `app.getLocale()`.
pub fn system_locale() -> Option<String> {
    sys_locale::get_locale()
}

/// Язык из `config.language`; не задан — выбрать по метке установщика и языку системы и записать.
pub fn ensure_language(settings: &mut Settings) -> Language {
    ensure_language_with(settings, || {
        pick_initial(installer_origin().as_deref(), system_locale().as_deref())
    })
}

/// [`ensure_language`] с подменой выбора для тестов.
pub fn ensure_language_with(settings: &mut Settings, pick: impl FnOnce() -> Language) -> Language {
    if let Some(lang) = settings.language() {
        return lang;
    }
    let lang = pick();
    // Не записалось — Store уже предупредил; язык всё равно применяется.
    settings.set_language(lang);
    lang
}
