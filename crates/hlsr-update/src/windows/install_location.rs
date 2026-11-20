//! Каталог установки в реестре (`launcher-update/install-location.js`). Обновление изнутри
//! лаунчера запускает `HLSR-Setup-<версия>.exe /S --updated` без `/D=`: установщик берёт каталог из
//! `InstallLocation`. Если запись установки ссылается на другой каталог (лаунчер перенесли вручную,
//! а в реестре остался старый путь), обновление поставило бы новую версию не туда. Перед запуском
//! установщика `InstallLocation` приводят к каталогу лаунчера.
//!
//! В 0.4.7 это был `reg.exe query` с разбором текста и `reg add`; здесь те же шаги идут через порт
//! [`Registry`]. Результат и тексты журнала те же.

use super::layout::{self, win_path};
use super::registry::{RegValue, Registry};
use crate::ports::Logger;
use hlsr_core::js;

/// Корень записей «Программ и компонентов» (запрос `reg query … /s`).
pub const UNINSTALL_ROOT: &str = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
/// Корень разделов установки в полной записи (`reg` печатает так).
pub const INSTALL_ROOT: &str = "HKEY_CURRENT_USER\\Software";

/// `^\{?[0-9A-Za-z][0-9A-Za-z.-]{2,78}\}?$` (`GUID_RE`).
fn is_guid_like(name: &str) -> bool {
    let inner = name.strip_prefix('{').unwrap_or(name);
    let inner = inner.strip_suffix('}').unwrap_or(inner);
    let mut chars = inner.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    let rest: Vec<char> = chars.collect();
    (2..=78).contains(&rest.len())
        && rest
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
}

/// Раздел установки по разделу записи удаления: тот же GUID в `Software`.
pub fn install_key_from(uninstall_key: &str) -> Option<String> {
    let guid = uninstall_key.rsplit('\\').next().unwrap_or("");
    is_guid_like(guid).then(|| format!("{INSTALL_ROOT}\\{guid}"))
}

/// Первое слово пути в `UninstallString`: в кавычках — всё до закрывающей, иначе до первого
/// `.exe` (без учёта регистра), за которым пробел или конец строки.
pub fn executable_from_uninstall_string(value: &str) -> Option<String> {
    let raw = js::trim(value);
    if raw.is_empty() {
        return None;
    }
    if let Some(quoted) = raw.strip_prefix('"') {
        // end = raw.indexOf('"', 1); end > 1 ? raw.slice(1, end) : null
        let end = quoted.find('"')?;
        return (end > 0).then(|| quoted[..end].to_owned());
    }
    // /^(.*?\.exe)(\s|$)/i: `.` не захватывает переводы строк, \s — пробелы JS.
    let is_line_break = |c: char| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
    for (at, _) in raw.char_indices() {
        let Some(tail) = raw.get(at..) else { continue };
        let Some(ext) = tail.get(..4) else { continue };
        if !ext.eq_ignore_ascii_case(".exe") {
            continue;
        }
        let after = &tail[4..];
        if raw[..at].chars().any(is_line_break) {
            return None;
        }
        if after.is_empty() || after.chars().next().is_some_and(js::is_whitespace) {
            return Some(raw[..at + 4].to_owned());
        }
    }
    None
}

/// `insideDir(file, dir)`: каталог `file` — именно `dir` (без учёта регистра).
fn inside_dir(file: &str, dir: &str) -> bool {
    if file.is_empty() || dir.is_empty() {
        return false;
    }
    let path = win_path();
    let parent = path.resolve(&[&path.dirname(file)]).to_lowercase();
    parent == layout::resolve_lower(&path, dir)
}

fn visit(registry: &dyn Registry, key: &str, install_dir: &str) -> Option<String> {
    if let Some(value) = registry.read_value(key, "UninstallString") {
        let exe = executable_from_uninstall_string(js::trim(&value.data_text()));
        if exe.is_some_and(|exe| inside_dir(&exe, install_dir)) {
            return Some(key.to_owned());
        }
    }
    for child in registry.subkeys(key).unwrap_or_default() {
        if let Some(found) = visit(registry, &format!("{key}\\{child}"), install_dir) {
            return Some(found);
        }
    }
    None
}

/// Запись «Программ и компонентов», у которой `UninstallString` ведёт в `install_dir`: раздел в
/// полной записи `HKEY_CURRENT_USER\…`. Разделы обходятся в глубину в порядке перечисления
/// (так `reg query /s` печатал их), берётся первый подходящий.
pub fn find_install_key(registry: &dyn Registry, install_dir: &str) -> Option<String> {
    let root = super::registry::full_key(UNINSTALL_ROOT).ok()?;
    visit(registry, &root, install_dir)
}

/// Значение `InstallLocation` раздела (без пробелов по краям); нет значения — пустая строка.
pub fn read_install_location(registry: &dyn Registry, key: &str) -> String {
    match registry.read_value(key, "InstallLocation") {
        Some(RegValue::Dword(number)) => format!("0x{number:x}"),
        Some(RegValue::Sz(text) | RegValue::ExpandSz(text)) => js::trim(&text).to_owned(),
        None => String::new(),
    }
}

/// Итог проверки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Значение уже верное.
    Ok,
    /// Значение исправлено.
    Written,
    /// Запись не удалась.
    WriteFailed,
    /// Своей записи установки нет.
    NoKey,
    /// Проверка не нужна (не Windows).
    Skipped,
    /// Проверка упала с ошибкой.
    Error,
}

impl Status {
    /// Строка `status` 0.4.7.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Written => "written",
            Status::WriteFailed => "write-failed",
            Status::NoKey => "no-key",
            Status::Skipped => "skipped",
            Status::Error => "error",
        }
    }
}

/// Результат [`ensure_install_location`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub status: Status,
    /// Раздел установки (`HKEY_CURRENT_USER\Software\<guid>`).
    pub key: Option<String>,
    /// Значение `InstallLocation` после проверки (у `write-failed` — прежнее).
    pub value: Option<String>,
}

impl Outcome {
    fn status(status: Status) -> Outcome {
        Outcome {
            status,
            key: None,
            value: None,
        }
    }
}

/// Параметры проверки.
#[derive(Debug, Clone, Default)]
pub struct Options<'a> {
    /// Каталог лаунчера (каталог `HLSR.exe`).
    pub install_dir: &'a str,
    /// Раздел записи удаления, если уже известен.
    pub uninstall_key: Option<&'a str>,
    /// Раздел установки, если уже известен.
    pub key: Option<&'a str>,
}

/// Привести `InstallLocation` своей записи установки к каталогу лаунчера.
pub fn ensure_install_location(
    registry: &dyn Registry,
    options: &Options<'_>,
    logger: &dyn Logger,
) -> Outcome {
    let install_dir = options.install_dir;
    if install_dir.is_empty() {
        return Outcome::status(Status::NoKey);
    }
    let non_empty = |value: Option<&str>| value.filter(|text| !text.is_empty()).map(str::to_owned);
    let uninstall_key =
        non_empty(options.uninstall_key).or_else(|| find_install_key(registry, install_dir));
    let key =
        non_empty(options.key).or_else(|| uninstall_key.as_deref().and_then(install_key_from));
    let Some(key) = key else {
        logger.info("[update] своей записи установки в реестре нет — каталог не поправить");
        return Outcome::status(Status::NoKey);
    };

    let current = read_install_location(registry, &key);
    let path = win_path();
    if !current.is_empty()
        && layout::resolve_lower(&path, &current) == layout::resolve_lower(&path, install_dir)
    {
        return Outcome {
            status: Status::Ok,
            key: Some(key),
            value: Some(current),
        };
    }

    if registry
        .write(
            &key,
            "InstallLocation",
            &RegValue::Sz(install_dir.to_owned()),
        )
        .is_err()
    {
        logger.warn(&format!(
            "[update] не удалось записать InstallLocation={install_dir}"
        ));
        return Outcome {
            status: Status::WriteFailed,
            key: Some(key),
            value: Some(current),
        };
    }

    let was = if current.is_empty() {
        "значения не было, ".to_owned()
    } else {
        format!("было \"{current}\", ")
    };
    logger.info(&format!(
        "[update] InstallLocation приведён к каталогу лаунчера: {was}стало \"{install_dir}\""
    ));
    Outcome {
        status: Status::Written,
        key: Some(key),
        value: Some(install_dir.to_owned()),
    }
}
