//! Реестр игрока через Win32 (вместо `reg.exe` 0.4.9) и снимок в формате `.reg`, как у
//! `reg.exe export`: UTF-16LE с BOM, `Windows Registry Editor Version 5.00`, ключ в скобках и
//! значения (`"имя"="строка"`, `dword:`, `hex(N):`). Разбор понимает и перенос строк `\`,
//! которым `reg.exe` режет длинные `hex:` — так возвращаются снимки, оставленные упавшим 0.4.9.

#![cfg_attr(not(windows), allow(dead_code))]

use super::Registry;
use crate::errors::DemoError;
use std::path::Path;

/// Значение ключа: имя (пустое — по умолчанию), тип `REG_*` и байты как в реестре.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegValue {
    pub name: String,
    pub kind: u32,
    pub data: Vec<u8>,
}

/// Ключ снимка: путь от корня (`Software\…`) и значения.
pub(crate) type RegKey = (String, Vec<RegValue>);

const HEADER: &str = "Windows Registry Editor Version 5.00";
const HKCU: &str = "HKEY_CURRENT_USER";
const REG_SZ: u32 = 1;
const REG_BINARY: u32 = 3;
const REG_DWORD: u32 = 4;

fn failed(what: &str, detail: impl std::fmt::Display) -> DemoError {
    DemoError {
        message: format!("reg {what}: {detail}"),
        ..DemoError::new("REGISTRY_FAILED")
    }
}

/// `HKCU\a\b` или `HKEY_CURRENT_USER\a\b` → `a\b`.
pub(crate) fn split_key(key: &str) -> Option<&str> {
    let (root, rest) = key.split_once('\\')?;
    (root.eq_ignore_ascii_case("HKCU") || root.eq_ignore_ascii_case(HKCU))
        .then(|| rest.trim_end_matches('\\'))
        .filter(|rest| !rest.is_empty())
}

fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn hex(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Строка `REG_SZ` для `"…"`: UTF-16 с одним нулём в конце, без нулей и переводов строк внутри.
fn utf16(bytes: &[u8]) -> Option<Vec<u16>> {
    let (pairs, rest) = bytes.as_chunks::<2>();
    rest.is_empty()
        .then(|| pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect())
}

fn plain_string(data: &[u8]) -> Option<String> {
    let units = utf16(data)?;
    let (last, body) = units.split_last()?;
    if *last != 0 || body.iter().any(|u| [0, 10, 13].contains(u)) {
        return None;
    }
    String::from_utf16(body).ok()
}

fn value_line(value: &RegValue) -> String {
    let name = if value.name.is_empty() {
        "@".to_owned()
    } else {
        quote(&value.name)
    };
    let data = match value.kind {
        REG_SZ if plain_string(&value.data).is_some() => {
            quote(&plain_string(&value.data).unwrap_or_default())
        }
        REG_DWORD if value.data.len() == 4 => format!(
            "dword:{:08x}",
            u32::from_le_bytes(value.data[..4].try_into().expect("4 байта"))
        ),
        REG_BINARY => format!("hex:{}", hex(&value.data)),
        kind => format!("hex({kind:x}):{}", hex(&value.data)),
    };
    format!("{name}={data}")
}

/// Текст снимка ключа `HKCU\<root>` и его подключей (пути ключей — от корня куста).
pub(crate) fn export_text(keys: &[RegKey]) -> String {
    let mut out = format!("{HEADER}\r\n\r\n");
    for (path, values) in keys {
        out.push_str(&format!("[{HKCU}\\{path}]\r\n"));
        for value in values {
            out.push_str(&value_line(value));
            out.push_str("\r\n");
        }
        out.push_str("\r\n");
    }
    out
}

/// Байты файла: UTF-16LE с BOM.
pub(crate) fn encode_file(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    bytes
}

/// Текст файла: UTF-16LE с BOM (`reg.exe`) или UTF-8.
pub(crate) fn decode_file(bytes: &[u8]) -> Result<String, String> {
    if let Some(body) = bytes.strip_prefix(&[0xff, 0xfe]) {
        let units = utf16(body).ok_or("нечётная длина UTF-16")?;
        return String::from_utf16(&units).map_err(|error| error.to_string());
    }
    let body = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    String::from_utf8(body.to_vec()).map_err(|error| error.to_string())
}

/// `"…"` с `\\` и `\"` с начала строки: (текст, остаток после кавычки).
fn unquote(text: &str) -> Option<(String, &str)> {
    let mut out = String::new();
    let mut chars = text.strip_prefix('"')?.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' => out.push(chars.next()?.1),
            '"' => return Some((out, &text[at + 2..])),
            _ => out.push(c),
        }
    }
    None
}

fn parse_hex(text: &str) -> Option<Vec<u8>> {
    let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if text.is_empty() {
        return Some(Vec::new());
    }
    text.split(',')
        .map(|byte| u8::from_str_radix(byte, 16).ok())
        .collect()
}

fn parse_value(line: &str) -> Result<Option<RegValue>, String> {
    let bad = || format!("строка не разобрана: {line}");
    let (name, rest) = if let Some(rest) = line.strip_prefix('@') {
        (String::new(), rest)
    } else {
        unquote(line).ok_or_else(bad)?
    };
    let data = rest.trim_start().strip_prefix('=').ok_or_else(bad)?.trim();
    // `-` — удалить значение: снимок так не пишет.
    if data == "-" {
        return Ok(None);
    }
    let (kind, data) = if data.starts_with('"') {
        let (text, tail) = unquote(data).ok_or_else(bad)?;
        if !tail.trim().is_empty() {
            return Err(bad());
        }
        let bytes = text
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        (REG_SZ, bytes)
    } else if let Some(number) = data.strip_prefix("dword:") {
        let value = u32::from_str_radix(number.trim(), 16).map_err(|_| bad())?;
        (REG_DWORD, value.to_le_bytes().to_vec())
    } else if let Some(bytes) = data.strip_prefix("hex:") {
        (REG_BINARY, parse_hex(bytes).ok_or_else(bad)?)
    } else if let Some(rest) = data.strip_prefix("hex(") {
        let (kind, bytes) = rest.split_once("):").ok_or_else(bad)?;
        let kind = u32::from_str_radix(kind, 16).map_err(|_| bad())?;
        (kind, parse_hex(bytes).ok_or_else(bad)?)
    } else {
        return Err(bad());
    };
    Ok(Some(RegValue { name, kind, data }))
}

/// Разбор снимка: ключи с полными именами (`HKEY_CURRENT_USER\…`) и их значения по порядку.
pub(crate) fn parse(text: &str) -> Result<Vec<RegKey>, String> {
    // Логические строки: перенос `\` в конце строки склеивается со следующей.
    let mut lines: Vec<String> = Vec::new();
    let mut joining = false;
    for raw in text.lines() {
        let line = if joining { raw.trim_start() } else { raw };
        let (body, more) = match line.trim_end().strip_suffix('\\') {
            Some(body) if !line.trim_start().starts_with('[') => (body, true),
            _ => (line, false),
        };
        if joining {
            lines.last_mut().expect("строка").push_str(body);
        } else {
            lines.push(body.to_owned());
        }
        joining = more;
    }
    let mut keys: Vec<RegKey> = Vec::new();
    for line in &lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line == HEADER || line == "REGEDIT4" {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            keys.push((name.to_owned(), Vec::new()));
            continue;
        }
        let Some((_, values)) = keys.last_mut() else {
            return Err(format!("значение вне ключа: {line}"));
        };
        if let Some(value) = parse_value(line)? {
            values.push(value);
        }
    }
    Ok(keys)
}

/// Ключи снимка внутри `key` (он сам и его подключи), пути — от корня куста. Чужие ключи файла
/// пропускаются.
pub(crate) fn keys_within(keys: Vec<RegKey>, key: &str) -> Vec<RegKey> {
    let Some(own) = split_key(key) else {
        return Vec::new();
    };
    let own = own.to_lowercase();
    keys.into_iter()
        .filter_map(|(name, values)| {
            let path = split_key(&name)?.to_owned();
            let lower = path.to_lowercase();
            (lower == own || lower.starts_with(&format!("{own}\\"))).then_some((path, values))
        })
        .collect()
}

/// Реестр текущего пользователя (или корня, переданного тестом).
pub struct WinRegistry {
    #[cfg(windows)]
    root: windows_sys::Win32::System::Registry::HKEY,
}

// SAFETY: дескриптор ключа реестра можно использовать из любого потока.
unsafe impl Send for WinRegistry {}
// SAFETY: см. выше; вызовы Win32 реестра потокобезопасны.
unsafe impl Sync for WinRegistry {}

impl WinRegistry {
    /// `HKEY_CURRENT_USER`.
    pub fn current_user() -> WinRegistry {
        WinRegistry {
            #[cfg(windows)]
            root: windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
        }
    }

    /// Другой открытый ключ вместо `HKEY_CURRENT_USER` (личный куст в тестах): ключи `HKCU\…`
    /// ищутся в нём.
    ///
    /// # Safety
    /// `root` — действительный дескриптор с полным доступом, открытый всё время жизни реестра.
    #[cfg(windows)]
    pub unsafe fn from_raw(root: windows_sys::Win32::System::Registry::HKEY) -> WinRegistry {
        WinRegistry { root }
    }
}

#[cfg(not(windows))]
impl Registry for WinRegistry {
    fn exists(&self, _key: &str) -> Result<bool, DemoError> {
        Err(failed("query", "реестр есть только в Windows"))
    }
    fn export_key(&self, _key: &str, _file: &Path) -> Result<(), DemoError> {
        Err(failed("export", "реестр есть только в Windows"))
    }
    fn delete_key(&self, _key: &str) -> Result<(), DemoError> {
        Err(failed("delete", "реестр есть только в Windows"))
    }
    fn import_file(&self, _key: &str, _file: &Path) -> Result<(), DemoError> {
        Err(failed("import", "реестр есть только в Windows"))
    }
}

#[cfg(windows)]
mod win {
    use super::{RegKey, RegValue, WinRegistry, failed, split_key};
    use crate::errors::DemoError;
    use hlsr_core::win32::wide;
    use windows_sys::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, RegCloseKey, RegCreateKeyExW,
        RegDeleteTreeW, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW, RegSetValueExW,
    };

    pub(super) struct Key(pub HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: дескриптор открыт нами и закрывается один раз.
            unsafe { RegCloseKey(self.0) };
        }
    }

    pub(super) fn key_path<'k>(key: &'k str, what: &str) -> Result<&'k str, DemoError> {
        split_key(key).ok_or_else(|| failed(what, format!("не ключ HKCU: {key}")))
    }

    impl WinRegistry {
        /// Открыть ключ для чтения: `None` — ключа нет.
        pub(super) fn open(&self, sub: &str, what: &str) -> Result<Option<Key>, DemoError> {
            let name = wide(sub);
            let mut key: HKEY = std::ptr::null_mut();
            // SAFETY: строка завершена нулём, `key` — локальная переменная.
            let status = unsafe { RegOpenKeyExW(self.root, name.as_ptr(), 0, KEY_READ, &mut key) };
            match status {
                ERROR_SUCCESS => Ok(Some(Key(key))),
                ERROR_FILE_NOT_FOUND => Ok(None),
                code => Err(failed(what, code)),
            }
        }

        /// Ключ и подключи сверху вниз, пути от корня куста.
        pub(super) fn dump(&self, sub: &str) -> Result<Vec<RegKey>, DemoError> {
            let Some(key) = self.open(sub, "export")? else {
                return Ok(Vec::new());
            };
            let mut out = vec![(sub.to_owned(), values(&key)?)];
            for child in subkeys(&key)? {
                out.extend(self.dump(&format!("{sub}\\{child}"))?);
            }
            Ok(out)
        }

        pub(super) fn write(&self, sub: &str, values: &[RegValue]) -> Result<(), DemoError> {
            let name = wide(sub);
            let mut raw: HKEY = std::ptr::null_mut();
            // SAFETY: строка завершена нулём, `raw` — локальная переменная.
            let status = unsafe {
                RegCreateKeyExW(
                    self.root,
                    name.as_ptr(),
                    0,
                    std::ptr::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_WRITE,
                    std::ptr::null(),
                    &mut raw,
                    std::ptr::null_mut(),
                )
            };
            if status != ERROR_SUCCESS {
                return Err(failed("import", status));
            }
            let key = Key(raw);
            for value in values {
                let value_name = wide(&value.name);
                // SAFETY: имя завершено нулём, данные и их длина переданы вместе.
                let status = unsafe {
                    RegSetValueExW(
                        key.0,
                        value_name.as_ptr(),
                        0,
                        value.kind,
                        value.data.as_ptr(),
                        value.data.len() as u32,
                    )
                };
                if status != ERROR_SUCCESS {
                    return Err(failed("import", status));
                }
            }
            Ok(())
        }

        pub(super) fn delete(&self, sub: &str) -> Result<(), DemoError> {
            let name = wide(sub);
            // SAFETY: строка завершена нулём.
            match unsafe { RegDeleteTreeW(self.root, name.as_ptr()) } {
                ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => {}
                code => return Err(failed("delete", code)),
            }
            // RegDeleteTreeW убирает подключи и значения; сам ключ — если остался.
            if self.open(sub, "delete")?.is_some() {
                // SAFETY: строка завершена нулём.
                match unsafe {
                    windows_sys::Win32::System::Registry::RegDeleteKeyW(self.root, name.as_ptr())
                } {
                    ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => {}
                    code => return Err(failed("delete", code)),
                }
            }
            Ok(())
        }
    }

    fn values(key: &Key) -> Result<Vec<RegValue>, DemoError> {
        let mut out = Vec::new();
        let mut name = vec![0u16; 256];
        let mut data = vec![0u8; 1024];
        let mut index = 0;
        loop {
            let mut name_len = name.len() as u32;
            let mut data_len = data.len() as u32;
            let mut kind = 0u32;
            // SAFETY: буферы и их длины переданы вместе.
            let status = unsafe {
                RegEnumValueW(
                    key.0,
                    index,
                    name.as_mut_ptr(),
                    &mut name_len,
                    std::ptr::null(),
                    &mut kind,
                    data.as_mut_ptr(),
                    &mut data_len,
                )
            };
            match status {
                ERROR_SUCCESS => {
                    out.push(RegValue {
                        name: String::from_utf16_lossy(&name[..name_len as usize]),
                        kind,
                        data: data[..data_len as usize].to_vec(),
                    });
                    index += 1;
                }
                ERROR_MORE_DATA => {
                    // Имя до 16383 знаков; данные — сколько попросили.
                    name.resize(16384, 0);
                    data.resize((data_len as usize).max(data.len() * 2), 0);
                }
                ERROR_NO_MORE_ITEMS => return Ok(out),
                code => return Err(failed("export", code)),
            }
        }
    }

    fn subkeys(key: &Key) -> Result<Vec<String>, DemoError> {
        let mut out = Vec::new();
        // Имя ключа — до 255 знаков.
        let mut name = vec![0u16; 256];
        loop {
            let mut len = name.len() as u32;
            // SAFETY: буфер и его длина переданы вместе.
            let status = unsafe {
                RegEnumKeyExW(
                    key.0,
                    out.len() as u32,
                    name.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            match status {
                ERROR_SUCCESS => out.push(String::from_utf16_lossy(&name[..len as usize])),
                ERROR_NO_MORE_ITEMS => return Ok(out),
                code => return Err(failed("export", code)),
            }
        }
    }
}

#[cfg(windows)]
impl Registry for WinRegistry {
    fn exists(&self, key: &str) -> Result<bool, DemoError> {
        Ok(self.open(win::key_path(key, "query")?, "query")?.is_some())
    }

    fn export_key(&self, key: &str, file: &Path) -> Result<(), DemoError> {
        let keys = self.dump(win::key_path(key, "export")?)?;
        std::fs::write(file, encode_file(&export_text(&keys))).map_err(|e| failed("export", e))
    }

    fn delete_key(&self, key: &str) -> Result<(), DemoError> {
        self.delete(win::key_path(key, "delete")?)
    }

    fn import_file(&self, key: &str, file: &Path) -> Result<(), DemoError> {
        for (path, values) in load(key, file)? {
            self.write(&path, &values)?;
        }
        Ok(())
    }

    fn check_file(&self, key: &str, file: &Path) -> Result<(), DemoError> {
        // Снимок существовавшего ключа всегда содержит сам ключ.
        if load(key, file)?.is_empty() {
            return Err(failed("import", format!("в снимке нет {key}")));
        }
        Ok(())
    }
}

/// Ключи снимка внутри `key`.
fn load(key: &str, file: &Path) -> Result<Vec<RegKey>, DemoError> {
    let bytes = std::fs::read(file).map_err(|e| failed("import", e))?;
    let text = decode_file(&bytes).map_err(|e| failed("import", e))?;
    let keys = parse(&text).map_err(|e| failed("import", e))?;
    Ok(keys_within(keys, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sz(text: &str) -> Vec<u8> {
        text.encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect()
    }

    #[test]
    fn reg_exe_export_with_wrapped_hex_is_parsed() {
        // Как пишет reg.exe export: длинный hex переносится «\» и двумя пробелами.
        let text = "Windows Registry Editor Version 5.00\r\n\r\n\
            [HKEY_CURRENT_USER\\Software\\Valve\\Half-Life\\Settings]\r\n\
            \"EngineDLL\"=\"hw.dll\"\r\n\
            \"ScreenWidth\"=dword:00000500\r\n\
            @=\"по умолчанию \\\"x\\\" C:\\\\HL\"\r\n\
            \"blob\"=hex:00,01,02,03,04,05,06,07,08,09,0a,0b,0c,0d,0e,0f,10,11,12,13,14,15,\\\r\n  \
            16,17,18\r\n\
            \"multi\"=hex(7):61,00,00,00,00,00\r\n\
            \"empty\"=hex:\r\n\r\n\
            [HKEY_CURRENT_USER\\Software\\Valve\\Half-Life\\Settings\\Sub]\r\n\
            \"q\"=hex(b):01,00,00,00,00,00,00,00\r\n\r\n\
            [HKEY_CURRENT_USER\\Software\\Other]\r\n\
            \"x\"=dword:00000001\r\n\r\n";
        let bytes = encode_file(text);
        let keys = parse(&decode_file(&bytes).unwrap()).unwrap();
        assert_eq!(keys.len(), 3);
        let own = keys_within(keys, "HKCU\\Software\\Valve\\Half-Life\\Settings");
        assert_eq!(own.len(), 2, "чужой ключ файла пропущен");
        assert_eq!(own[0].0, "Software\\Valve\\Half-Life\\Settings");
        let values = &own[0].1;
        assert_eq!(
            values[0],
            RegValue {
                name: "EngineDLL".into(),
                kind: 1,
                data: sz("hw.dll")
            }
        );
        assert_eq!(values[1].data, 0x500u32.to_le_bytes());
        assert_eq!(values[2].name, "");
        assert_eq!(values[2].data, sz("по умолчанию \"x\" C:\\HL"));
        assert_eq!(values[3].kind, 3);
        assert_eq!(values[3].data, (0u8..=0x18).collect::<Vec<_>>());
        assert_eq!(values[4].kind, 7);
        assert_eq!(values[5].data, Vec::<u8>::new());
        assert_eq!(own[1].1[0].kind, 0xb);

        // Свой снимок читается обратно тем же.
        let again = parse(&decode_file(&encode_file(&export_text(&own))).unwrap()).unwrap();
        assert_eq!(
            keys_within(
                again,
                "HKEY_CURRENT_USER\\Software\\Valve\\Half-Life\\Settings"
            ),
            own
        );
        assert!(parse("\"x\"=dword:1").is_err(), "значение вне ключа");
        assert!(parse("[HKEY_CURRENT_USER\\a]\r\n\"x\"=what").is_err());
    }

    #[test]
    fn odd_strings_are_kept_as_hex() {
        let values = vec![
            RegValue {
                name: "nl".into(),
                kind: 1,
                data: sz("a\nb"),
            },
            RegValue {
                name: "nonul".into(),
                kind: 1,
                data: vec![0x61, 0],
            },
            RegValue {
                name: "short".into(),
                kind: 4,
                data: vec![1, 2],
            },
        ];
        let text = export_text(&[("Software\\T".into(), values.clone())]);
        assert!(text.contains("\"nl\"=hex(1):61,00,0a,00,62,00,00,00"));
        let back = parse(&text).unwrap();
        assert_eq!(back[0].1, values);
    }
}
