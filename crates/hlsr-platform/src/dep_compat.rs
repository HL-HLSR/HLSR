//! Совместимость с DEP для HL1 WON (`dep-compat.js`). `hl.exe` из WON собран без `NX_COMPAT`, а
//! `hw.dll` расшифровывает движок в неисполняемую память; при DEP в режиме OptOut такой `hl.exe`
//! падает на первом переходе в движок. Слой совместимости `DisableNXShowUI` снимает DEP с одного
//! exe — та же запись, что делает окно «Предотвращение выполнения данных» Windows; в `HKCU` права
//! администратора не нужны.
//!
//! 0.4.7 ставил слой через `reg.exe query/add`, а режим системы узнавал через PowerShell CIM. В
//! Rust (решение 2 спецификации) запись идёт прямым `RegSetValueExW`, а режим — `GetSystemDEPPolicy`
//! (те же 0..3). Чистые функции (`layers_of`, `merge_layers`, `parse_policy`, `crash_hint`)
//! перенесены дословно и сверяются с 0.4.7 по фикстуре.

use hlsr_core::js;
use hlsr_delivery::ports::{Logger, Registry, RegistryHive};
use std::sync::Arc;

/// `COMPAT_LAYERS`: ключ слоёв совместимости в `HKCU`.
pub const COMPAT_LAYERS: &str =
    "HKCU\\Software\\Microsoft\\Windows NT\\CurrentVersion\\AppCompatFlags\\Layers";

/// Подключ `COMPAT_LAYERS` без куста — то, что ждёт порт [`Registry`].
pub const LAYERS_SUBKEY: &str =
    "Software\\Microsoft\\Windows NT\\CurrentVersion\\AppCompatFlags\\Layers";

/// `LAYER`: имя слоя, снимающего DEP.
pub const LAYER: &str = "DisableNXShowUI";

/// `ACCESS_VIOLATION` (`STATUS_ACCESS_VIOLATION`): так завершается процесс, остановленный DEP.
pub const ACCESS_VIOLATION: u32 = 0xc000_0005;

/// `POLICY`: режим DEP (`Win32_OperatingSystem.DataExecutionPrevention_SupportPolicy`).
pub fn policy_name(value: i64) -> Option<&'static str> {
    match value {
        0 => Some("AlwaysOff"),
        1 => Some("AlwaysOn"),
        2 => Some("OptIn"),
        3 => Some("OptOut"),
        _ => None,
    }
}

/// Состояние DEP для подсказки к падению: режим системы (`None` — не определён) и как
/// прошла попытка снять DEP со слоя exe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepState {
    pub policy: Option<i64>,
    pub exemption: Exemption,
}

/// Итог [`ensure_exemption`]: слой уже был, поставлен или записать не удалось.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exemption {
    Present,
    Added,
    Failed,
}

impl Exemption {
    pub fn as_str(self) -> &'static str {
        match self {
            Exemption::Present => "present",
            Exemption::Added => "added",
            Exemption::Failed => "failed",
        }
    }
}

/// `layersOf(stdout, exe)`: данные слоёв для `exe` из вывода `reg query`
/// (`    <имя>    REG_SZ    <данные>`); `None` — значения нет. Регулярное выражение 0.4.7:
/// `/^\s*(.+?)\s{2,}REG_SZ\s{2,}(.*)$/`, сравнение имени без учёта регистра, `trim` данных.
pub fn layers_of(stdout: &str, exe: &str) -> Option<String> {
    let target = exe.to_lowercase();
    for raw in stdout.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some((name, data)) = parse_reg_sz_line(line)
            && js::trim(name).to_lowercase() == target
        {
            return Some(js::trim(data).to_owned());
        }
    }
    None
}

/// Разбор строки `^\s*(.+?)\s{2,}REG_SZ\s{2,}(.*)$`: (имя, данные). Берётся первое вхождение
/// `REG_SZ`, перед и после которого не меньше двух пробельных знаков, а имя непустое.
fn parse_reg_sz_line(line: &str) -> Option<(&str, &str)> {
    let mut from = 0;
    while let Some(rel) = line[from..].find("REG_SZ") {
        let at = from + rel;
        let before = &line[..at];
        let after = &line[at + "REG_SZ".len()..];
        let trailing: usize = before
            .chars()
            .rev()
            .take_while(|c| js::is_whitespace(*c))
            .map(char::len_utf8)
            .sum();
        let leading: usize = after
            .chars()
            .take_while(|c| js::is_whitespace(*c))
            .map(char::len_utf8)
            .sum();
        let two_before = before[before.len() - trailing..].chars().count() >= 2;
        let two_after = after[..leading].chars().count() >= 2;
        if two_before && two_after {
            let name = before[..before.len() - trailing].trim_start_matches(js::is_whitespace);
            if !name.is_empty() {
                return Some((name, &after[leading..]));
            }
        }
        from = at + 1;
    }
    None
}

/// `mergeLayers(current)`: слой дописывается к уже стоящим флагам; `None` — он уже есть.
pub fn merge_layers(current: Option<&str>) -> Option<String> {
    let text = current.unwrap_or("");
    let mut tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.iter().any(|token| token.eq_ignore_ascii_case(LAYER)) {
        return None;
    }
    if tokens.first() == Some(&"~") {
        tokens.remove(0);
    }
    let mut parts = Vec::with_capacity(tokens.len() + 2);
    parts.push("~");
    parts.extend(tokens);
    parts.push(LAYER);
    Some(parts.join(" "))
}

/// `Number.parseInt(text, 10)`: ведущие пробелы, необязательный знак, десятичные цифры;
/// `None` — цифр нет (`NaN`).
fn parse_int10(text: &str) -> Option<i64> {
    let bytes = text.trim_start_matches(js::is_whitespace).as_bytes();
    let mut index = 0;
    let mut sign = 1i64;
    if index < bytes.len() && (bytes[index] == b'+' || bytes[index] == b'-') {
        if bytes[index] == b'-' {
            sign = -1;
        }
        index += 1;
    }
    let start = index;
    let mut value: i64 = 0;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        value = value.saturating_mul(10) + i64::from(bytes[index] - b'0');
        index += 1;
    }
    (index > start).then_some(sign * value)
}

/// `parsePolicy(stdout)`: режим DEP из текста или `None`, если это не 0..3.
pub fn parse_policy(stdout: &str) -> Option<i64> {
    let value = parse_int10(js::trim(stdout))?;
    policy_name(value).map(|_| value)
}

/// `crashHint(state, exitCode)`: `DEP_ALWAYS_ON` при AlwaysOn и `DEP_EXEMPTION_FAILED` при OptOut
/// без записанного слоя; `None` — не DEP. `hl.exe` запускается через инжектор, поэтому код выхода
/// чаще неизвестен; известный и не `STATUS_ACCESS_VIOLATION` — не DEP.
pub fn crash_hint(state: Option<&DepState>, exit_code: Option<i64>) -> Option<&'static str> {
    let state = state?;
    if let Some(code) = exit_code
        && (code as u32) != ACCESS_VIOLATION
    {
        return None;
    }
    if state.policy == Some(1) {
        return Some("DEP_ALWAYS_ON");
    }
    if state.policy == Some(3) && state.exemption == Exemption::Failed {
        return Some("DEP_EXEMPTION_FAILED");
    }
    None
}

/// Чтение и запись слоёв совместимости exe (в 0.4.7 — `reg.exe query/add`).
pub trait DepRegistry: Send + Sync {
    /// Вывод `reg query COMPAT_LAYERS /v <exe>` или `None`, если значения нет либо `reg.exe`
    /// завершился с ошибкой.
    fn query_layers(&self, exe: &str) -> Option<String>;

    /// `reg add COMPAT_LAYERS /v <exe> /t REG_SZ /d <data> /f`.
    fn add_layer(&self, exe: &str, data: &str) -> Result<(), String>;
}

/// `ensureExemption(exe)`: ставит слой для exe. Исключения внутри — в журнал, итог тот же.
pub fn ensure_exemption(reg: &dyn DepRegistry, exe: &str, logger: &dyn Logger) -> Exemption {
    let current = reg.query_layers(exe).and_then(|out| layers_of(&out, exe));
    let Some(next) = merge_layers(current.as_deref()) else {
        return Exemption::Present;
    };
    match reg.add_layer(exe, &next) {
        Ok(()) => {
            logger.info(&format!(
                "[dep] для {exe} поставлен слой {LAYER}{}",
                current
                    .map(|was| format!(" (было: {was})"))
                    .unwrap_or_default()
            ));
            Exemption::Added
        }
        Err(reason) => {
            logger.warn(&format!("[dep] слой {LAYER} не записан: {reason}"));
            Exemption::Failed
        }
    }
}

/// Реестр слоёв через порт [`Registry`] (чтение) и прямой `RegSetValueExW` (запись).
pub struct SystemDepRegistry {
    registry: Arc<dyn Registry>,
}

impl SystemDepRegistry {
    pub fn new(registry: Arc<dyn Registry>) -> SystemDepRegistry {
        SystemDepRegistry { registry }
    }
}

impl DepRegistry for SystemDepRegistry {
    fn query_layers(&self, exe: &str) -> Option<String> {
        self.registry
            .query(RegistryHive::CurrentUser, LAYERS_SUBKEY, exe)
    }

    #[cfg(windows)]
    fn add_layer(&self, exe: &str, data: &str) -> Result<(), String> {
        write_layer(exe, data)
    }

    #[cfg(not(windows))]
    fn add_layer(&self, _exe: &str, _data: &str) -> Result<(), String> {
        let _ = &self.registry;
        Err("реестр доступен только на Windows".to_owned())
    }
}

/// `GetSystemDEPPolicy`: режим DEP системы 0..3 или `None` — узнать не удалось.
#[cfg(windows)]
pub fn query_policy() -> Option<i64> {
    use windows_sys::Win32::System::SystemInformation::GetSystemDEPPolicy;
    // SAFETY: функция без аргументов, возвращает перечисление.
    let value = unsafe { GetSystemDEPPolicy() };
    policy_name(i64::from(value)).map(|_| i64::from(value))
}

#[cfg(not(windows))]
pub fn query_policy() -> Option<i64> {
    None
}

/// `reg add HKCU\...\Layers /v <exe> /t REG_SZ /d <data> /f`.
#[cfg(windows)]
fn write_layer(exe: &str, data: &str) -> Result<(), String> {
    use hlsr_core::win32::wide;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegSetValueExW,
    };

    struct Handle(HKEY);
    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: дескриптор получен от RegCreateKeyExW и закрывается один раз.
            unsafe { RegCloseKey(self.0) };
        }
    }

    let subkey = wide(LAYERS_SUBKEY);
    let mut raw: HKEY = std::ptr::null_mut();
    let mut disposition = 0u32;
    // SAFETY: строка завершена нулём, `raw` и `disposition` — локальные переменные.
    let created = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut raw,
            &mut disposition,
        )
    };
    if created != ERROR_SUCCESS {
        return Err(format!("RegCreateKeyExW: код {created}"));
    }
    let handle = Handle(raw);
    let name = wide(exe);
    // `reg.exe` пишет строку REG_SZ с завершающим нулём.
    let value = wide(data);
    let bytes = std::mem::size_of_val(value.as_slice()) as u32;
    // SAFETY: буфер `value` живёт до конца вызова, длина в байтах задана точно.
    let set = unsafe {
        RegSetValueExW(
            handle.0,
            name.as_ptr(),
            0,
            REG_SZ,
            value.as_ptr().cast::<u8>(),
            bytes,
        )
    };
    if set != ERROR_SUCCESS {
        return Err(format!("RegSetValueExW: код {set}"));
    }
    Ok(())
}
