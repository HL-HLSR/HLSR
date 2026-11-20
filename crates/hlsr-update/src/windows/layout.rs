//! Раскладка Windows-установки HLSR (`windows-install/layout.js`). Значения совпадают с тем, что
//! писал прежний NSIS-установщик electron-builder: новый установщик подхватывает уже установленные
//! копии, а обновлятор 0.4.7 запускает `HLSR-Setup-<версия>.exe /S --updated /D=<каталог>` и ждёт
//! кода 0. Аргументы командной строки (`parseArgs`) разбирает `hlsr_core::args::parse_setup_args`.
//!
//! Пути — по правилам `path.win32` на любой ОС (`hlsr_core::js::path`): так проверки одинаковы
//! на Windows и в тестах на Linux.

use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform};
use sha1::{Digest, Sha1};

/// Пространство имён UUID v5, от которого electron-builder (`NsisTarget`) считал GUID установки
/// из `appId` (в байтах).
const ELECTRON_BUILDER_NS: [u8; 16] = [
    0x50, 0xe0, 0x65, 0xbc, 0x31, 0x34, 0x11, 0xe6, 0x9b, 0xab, 0x38, 0xc9, 0x86, 0x2b, 0xda, 0xf3,
];

/// `appId` боевого HLSR: под ключом `3f824379-…` NSIS регистрировал установку; другой GUID оставил
/// бы прежнюю запись в «Программах и компонентах» рядом с новой.
pub const PRODUCTION_APP_ID: &str = "com.hlsrteam.hlsr";
pub const PRODUCTION_PRODUCT_NAME: &str = "HLSR";
/// GUID боевого HLSR — `guid_for(PRODUCTION_APP_ID)`.
pub const PRODUCTION_GUID: &str = "3f824379-6b41-5725-b424-59cb87f2acae";
/// QA-сборка не встаёт поверх боевого HLSR: свои `appId`, имя, ключи и каталог (решение 18).
pub const QA_APP_ID: &str = "com.hlsrteam.hlsr.qa";
pub const QA_PRODUCT_NAME: &str = "HLSR-QA";
/// Имя приложения (`app.getName()`) боевой сборки: `name` упакованного `package.json` 0.4.7.
pub const PRODUCTION_NAME: &str = hlsr_core::paths::APP_NAME;
/// Имя QA-сборки: `extraMetadata.name=hlsr-qa` в `qa-launcher-build.js` 0.4.7. Electron брал
/// `userData` и журналы от него (`%APPDATA%\hlsr-qa`, `%LOCALAPPDATA%\hlsr-qa\logs`), и QA-сборка
/// не делила профиль с боевой.
pub const QA_NAME: &str = "hlsr-qa";

/// Раздел с меткой страницы загрузки (`InstallerOrigin`); удаление его не трогает.
pub const ORIGIN_KEY: &str = "HKCU\\Software\\HLSR";
/// Значение, которое NSIS ставил на время незавершённой установки.
pub const SEED_FLAG: &str = "HLSRSeededInstallLocation";
/// Выбор человека по ярлыку на рабочем столе: `"1"` или `"0"`.
pub const DESKTOP_VALUE: &str = "HLSRDesktopShortcut";

pub const LIBRARY_ROOT: &str = "C:\\HLSR";
pub const EXE: &str = "HLSR.exe";
pub const PUBLISHER: &str = "HLSR Team";
pub const OWNER_MARKER: &str = ".hlsr-install-owner.json";
/// Запись в «Программах и компонентах» запускает удаление самим `HLSR.exe`.
pub const UNINSTALL_FLAG: &str = "--hlsr-uninstall";
/// Путь к установщику, из которого запущено удаление: «Назад» снова откроет его, когда каталог
/// программы будет удалён.
pub const RETURN_SETUP_FLAG: &str = "--hlsr-return-setup";

/// Значение переменной окружения; пустая строка считается отсутствующей (`e.NAME || …`).
pub type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Окружение процесса: `process.env`.
pub fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// `appId` и имя продукта сборки — поле `app` списка файлов (`install-files.json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub app_id: String,
    pub product_name: String,
}

impl App {
    /// Боевой HLSR (`PRODUCTION`).
    pub fn production() -> App {
        App {
            app_id: PRODUCTION_APP_ID.to_owned(),
            product_name: PRODUCTION_PRODUCT_NAME.to_owned(),
        }
    }

    /// QA-сборка.
    pub fn qa() -> App {
        App {
            app_id: QA_APP_ID.to_owned(),
            product_name: QA_PRODUCT_NAME.to_owned(),
        }
    }

    /// Имя приложения сборки (`app.getName()`): у QA-сборки ([`QA_APP_ID`]) — [`QA_NAME`], у любой
    /// другой — боевое [`PRODUCTION_NAME`].
    pub fn name(&self) -> &'static str {
        if self.app_id == QA_APP_ID {
            QA_NAME
        } else {
            PRODUCTION_NAME
        }
    }
}

/// Недопустимые данные сборки.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    #[error("Недопустимое имя продукта: {0}")]
    ProductName(String),
    #[error("Недопустимый GUID сборки: {0}")]
    Guid(String),
}

/// `^\d+\.\d+\.\d+$` (`VERSION_RE`): версия выпуска только `X.Y.Z`, цифры ASCII.
pub fn is_version(text: &str) -> bool {
    let mut parts = text.split('.');
    let mut count = 0;
    for part in parts.by_ref() {
        count += 1;
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
    }
    count == 3
}

/// `^\{[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12}\}$` (`INSTALL_ID_RE`):
/// идентификатор установки в реестре и маркере владельца — заглавными.
pub fn is_install_id(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != 38 || bytes[0] != b'{' || bytes[37] != b'}' {
        return false;
    }
    bytes[1..37].iter().enumerate().all(|(at, &byte)| match at {
        8 | 13 | 18 | 23 => byte == b'-',
        _ => matches!(byte, b'0'..=b'9' | b'A'..=b'F'),
    })
}

/// GUID сборки `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` (`GUID_RE`); `any_case` — без учёта регистра
/// (`/i`), иначе только строчные.
pub fn is_guid(text: &str, any_case: bool) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(at, &byte)| match at {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => {
                matches!(byte, b'0'..=b'9' | b'a'..=b'f')
                    || (any_case && matches!(byte, b'A'..=b'F'))
            }
        })
}

/// GUID установки: UUID v5 от пространства имён electron-builder и `appId`.
pub fn guid_for(app_id: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(ELECTRON_BUILDER_NS);
    hasher.update(app_id.as_bytes());
    let mut hash: [u8; 20] = hasher.finalize().into();
    hash[6] = (hash[6] & 0x0f) | 0x50;
    hash[8] = (hash[8] & 0x3f) | 0x80;
    let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Чем установка этой сборки отличается от установки другой: ключи реестра, каталог, ярлык.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub app_id: String,
    pub product_name: String,
    pub guid: String,
    /// `HKCU\Software\<guid>`.
    pub install_key: String,
    /// `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\<guid>`.
    pub uninstall_key: String,
    /// `C:\<Продукт>\Launcher`.
    pub default_install_dir: String,
    pub shortcut_name: String,
}

impl Identity {
    /// Маркер владельца каталога: `{"schemaVersion":1,"appId":"<guid>","installId":"{UUID}"}` без
    /// перевода строки; удаление сравнивает его байт в байт.
    pub fn owner_marker(&self, install_id: &str) -> String {
        format!(
            "{{\"schemaVersion\":1,\"appId\":\"{}\",\"installId\":\"{install_id}\"}}",
            self.guid
        )
    }
}

/// `^[A-Za-z0-9][A-Za-z0-9 ._-]*$`.
fn is_product_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-'))
}

/// `identity(app)`: `appId` и имя берутся из списка файлов сборки; пустые (и `None`) — боевые.
pub fn identity(app: Option<&App>) -> Result<Identity, LayoutError> {
    let pick = |value: Option<&str>, default: &'static str| match value {
        Some(text) if !text.is_empty() => text.to_owned(),
        _ => default.to_owned(),
    };
    let app_id = pick(app.map(|app| app.app_id.as_str()), PRODUCTION_APP_ID);
    let product_name = pick(
        app.map(|app| app.product_name.as_str()),
        PRODUCTION_PRODUCT_NAME,
    );
    if !is_product_name(&product_name) {
        return Err(LayoutError::ProductName(product_name));
    }
    let guid = guid_for(&app_id);
    Ok(identity_for(app_id, product_name, guid))
}

/// `identityOfGuid(guid)`: сборка, известная только по GUID из маркера владения (списка файлов нет
/// или он с другим GUID): ключи реестра её, `appId` неизвестен (пустой) — не боевая.
pub fn identity_of_guid(guid: &str) -> Result<Identity, LayoutError> {
    if !is_guid(guid, true) {
        return Err(LayoutError::Guid(guid.to_owned()));
    }
    Ok(identity_for(
        String::new(),
        PRODUCTION_PRODUCT_NAME.to_owned(),
        guid.to_lowercase(),
    ))
}

fn identity_for(app_id: String, product_name: String, guid: String) -> Identity {
    Identity {
        install_key: format!("HKCU\\Software\\{guid}"),
        uninstall_key: format!(
            "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{guid}"
        ),
        default_install_dir: format!("C:\\{product_name}\\Launcher"),
        shortcut_name: product_name.clone(),
        app_id,
        product_name,
        guid,
    }
}

/// `path` Windows с текущим каталогом процесса; вне Windows — корень диска `C:\`.
pub fn win_path() -> NodePath {
    let cwd = if cfg!(windows) {
        std::env::current_dir()
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        "C:\\".to_owned()
    };
    NodePath::new(Platform::Win32, cwd)
}

fn is_separator(c: char) -> bool {
    c == '\\' || c == '/'
}

/// `.replace(/[\\/]+$/, "")`.
fn trim_separators(text: &str) -> &str {
    text.trim_end_matches(is_separator)
}

/// `path.resolve(value).toLowerCase()`.
pub fn resolve_lower(path: &NodePath, value: &str) -> String {
    path.resolve(&[value]).to_lowercase()
}

/// `lower(value)` модуля: `path.resolve(value)` без хвостовых разделителей, строчными.
fn lower(path: &NodePath, value: &str) -> String {
    let resolved = path.resolve(&[value]);
    trim_separators(&resolved).to_lowercase()
}

/// `path.parse(resolved).root` для пути, полученного из `path.win32.resolve`.
fn win32_root(resolved: &str) -> String {
    let chars: Vec<char> = resolved.chars().collect();
    if chars.len() >= 2 && is_separator(chars[0]) && is_separator(chars[1]) {
        // \\сервер\ресурс\ — корень UNC.
        let mut end = 2;
        let mut parts = 0;
        while end < chars.len() && parts < 2 {
            if is_separator(chars[end]) {
                parts += 1;
            }
            end += 1;
        }
        return chars[..end].iter().collect();
    }
    if chars.len() >= 2 && chars[0].is_ascii_alphabetic() && chars[1] == ':' {
        let end = if chars.len() >= 3 && is_separator(chars[2]) {
            3
        } else {
            2
        };
        return chars[..end].iter().collect();
    }
    if chars.first().copied().is_some_and(is_separator) {
        return chars[..1].iter().collect();
    }
    String::new()
}

/// Каталоги, в которые HLSR не ставится и которые удаление не трогает.
pub fn protected_dirs(env: EnvLookup<'_>) -> Vec<String> {
    let path = win_path();
    let get = |name: &str| env(name).filter(|value| !value.is_empty());
    let mut list = vec![LIBRARY_ROOT.to_owned()];
    if let Some(windir) = get("WINDIR").or_else(|| get("SystemRoot")) {
        list.push(windir.clone());
        list.push(path.join(&[&windir, "System32"]));
    }
    for name in [
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "USERPROFILE",
        "TEMP",
        "TMP",
        "APPDATA",
        "LOCALAPPDATA",
    ] {
        if let Some(value) = get(name) {
            list.push(value);
        }
    }
    if let Some(profile) = get("USERPROFILE") {
        for name in ["Desktop", "Documents", "Downloads"] {
            list.push(path.join(&[&profile, name]));
        }
    }
    list
}

/// Символы, которые нельзя безопасно передать в `cmd.exe` и `reg.exe`.
fn is_unsafe_path_char(c: char) -> bool {
    matches!(
        c,
        '"' | '%' | '^' | '&' | '|' | '<' | '>' | '*' | '?' | '\0' | '\r' | '\n'
    )
}

/// Причина отказа или `None`. Проверка одна для установки и удаления: в каталог, куда нельзя
/// ставить, нельзя и удалять рекурсивно.
pub fn refused_dir(dir: &str, env: EnvLookup<'_>) -> Option<String> {
    refused_dir_in(&win_path(), dir, env)
}

/// [`refused_dir`] с заданными `path` и текущим каталогом.
pub fn refused_dir_in(path: &NodePath, dir: &str, env: EnvLookup<'_>) -> Option<String> {
    if js::trim(dir).is_empty() {
        return Some("каталог не задан".to_owned());
    }
    if !path.is_absolute(dir) {
        return Some(format!("путь не абсолютный: {dir}"));
    }
    let mut leading = dir.chars();
    if leading.next().is_some_and(is_separator) && leading.next().is_some_and(is_separator) {
        return Some(format!("сетевой путь: {dir}"));
    }
    if dir.split(is_separator).any(|part| part == "..") {
        return Some(format!("путь с «..»: {dir}"));
    }
    if dir.chars().any(is_unsafe_path_char) {
        return Some(format!("недопустимые символы в пути: {dir}"));
    }

    let full = lower(path, dir);
    let root = win32_root(&path.resolve(&[dir])).to_lowercase();
    if trim_separators(&root) == full {
        return Some("корень диска".to_owned());
    }
    for blocked in protected_dirs(env) {
        if lower(path, &blocked) == full {
            return Some(format!("системный каталог: {dir}"));
        }
    }
    None
}

/// Один и тот же каталог (без учёта регистра и хвостовых разделителей).
pub fn is_same_dir(a: &str, b: &str) -> bool {
    let path = win_path();
    lower(&path, a) == lower(&path, b)
}

/// `child` равен `parent` или лежит внутри него.
pub fn is_inside(child: &str, parent: &str) -> bool {
    let path = win_path();
    let c = lower(&path, child);
    let p = lower(&path, parent);
    c == p || c.starts_with(&format!("{p}\\")) || c.starts_with(&format!("{p}/"))
}
