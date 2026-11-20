//! Записи установки в `HKCU` (`windows-install/registry.js`) и ключи раздела 5.2 карты.
//!
//! В 0.4.7 и чтение, и запись шли через `.reg`-файл в UTF-16LE (`reg export` / `reg import` в
//! `%TEMP%\hlsr-reg-<12hex>.reg`): вывод `reg query` печатается в OEM-кодировке консоли, и
//! кириллица в пути ломается. Здесь реестр читается и пишется прямо через Win32 (`RegSetValueExW`
//! и другие), строки идут в UTF-16 как есть: результат в реестре тот же, `reg.exe` и временные
//! файлы не нужны (отступление от 0.4.7, решено планом).
//!
//! Раздел — строка вида `HKCU\Software\X` (допустимы также `HKEY_CURRENT_USER`, `HKLM` и
//! `HKEY_LOCAL_MACHINE` без учёта регистра): так их печатает `reg.exe` и так они написаны в 0.4.7.
//! Писать можно только в `HKCU`; `HKLM` — только читать (`AppCompatFlags\Layers`).

use super::layout::{self, Identity};
use hlsr_core::js::Platform;
use hlsr_core::js::path::NodePath;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Куст реестра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hive {
    CurrentUser,
    LocalMachine,
}

impl Hive {
    /// Полное имя куста, как его печатает `reg.exe`.
    pub fn full_name(self) -> &'static str {
        match self {
            Hive::CurrentUser => "HKEY_CURRENT_USER",
            Hive::LocalMachine => "HKEY_LOCAL_MACHINE",
        }
    }
}

/// Значение реестра. Остальные типы (`REG_BINARY`, `REG_MULTI_SZ`…) установке не нужны и не читаются.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegValue {
    Sz(String),
    ExpandSz(String),
    Dword(u32),
}

impl RegValue {
    /// Имя типа, как в `reg query`.
    pub fn type_name(&self) -> &'static str {
        match self {
            RegValue::Sz(_) => "REG_SZ",
            RegValue::ExpandSz(_) => "REG_EXPAND_SZ",
            RegValue::Dword(_) => "REG_DWORD",
        }
    }

    /// Данные, как их печатает `reg query`: строка как есть, `DWORD` — `0x1`.
    pub fn data_text(&self) -> String {
        match self {
            RegValue::Sz(text) | RegValue::ExpandSz(text) => text.clone(),
            RegValue::Dword(number) => format!("0x{number:x}"),
        }
    }

    /// Строка `REG_SZ`; остальные типы `parseRegFile` 0.4.7 не читал.
    pub fn as_sz(&self) -> Option<&str> {
        match self {
            RegValue::Sz(text) => Some(text),
            _ => None,
        }
    }
}

/// Ошибка реестра: сообщение для журнала и предупреждения окну.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RegistryError(pub String);

fn unsupported(key: &str) -> RegistryError {
    RegistryError(format!("Неподдерживаемый раздел реестра: {key}"))
}

/// Разобрать раздел на куст и путь внутри куста (без хвостовых `\`).
pub fn split_key(key: &str) -> Result<(Hive, &str), RegistryError> {
    let (head, rest) = key.split_once('\\').unwrap_or((key, ""));
    let hive = match head.to_ascii_uppercase().as_str() {
        "HKCU" | "HKEY_CURRENT_USER" => Hive::CurrentUser,
        "HKLM" | "HKEY_LOCAL_MACHINE" => Hive::LocalMachine,
        _ => return Err(unsupported(key)),
    };
    Ok((hive, rest.trim_end_matches('\\')))
}

/// Раздел в полной записи: `HKEY_CURRENT_USER\Software\X`.
pub fn full_key(key: &str) -> Result<String, RegistryError> {
    let (hive, path) = split_key(key)?;
    Ok(if path.is_empty() {
        hive.full_name().to_owned()
    } else {
        format!("{}\\{path}", hive.full_name())
    })
}

/// Реестр Windows. Имена разделов и значений — без учёта регистра, как в реестре.
pub trait Registry: Send + Sync {
    /// Значения раздела; `None` — раздела нет или он не читается. Разделы без значений есть и дают
    /// пустой список (`reg export` для них успешен).
    fn read(&self, key: &str) -> Option<Vec<(String, RegValue)>>;

    /// Значение `name` раздела `key`.
    fn read_value(&self, key: &str, name: &str) -> Option<RegValue> {
        let wanted = name.to_lowercase();
        self.read(key)?
            .into_iter()
            .find(|(candidate, _)| candidate.to_lowercase() == wanted)
            .map(|(_, value)| value)
    }

    /// Записать значение; раздел и его родители создаются. Только `HKCU`.
    fn write(&self, key: &str, name: &str, value: &RegValue) -> Result<(), RegistryError>;

    /// Удалить значение (`"имя"=-`); нет раздела или значения — не ошибка.
    fn delete_value(&self, key: &str, name: &str) -> Result<(), RegistryError>;

    /// Удалить раздел со всем содержимым (`[-раздел]`); раздела нет — не ошибка.
    fn delete_tree(&self, key: &str) -> Result<(), RegistryError>;

    /// Имена подразделов (как записаны в реестре); `None` — раздела нет.
    fn subkeys(&self, key: &str) -> Option<Vec<String>>;
}

/// Раздел, который можно удалять целиком: куст и первый уровень (`Software`) — нельзя.
fn deletable_path(key: &str) -> Result<&str, RegistryError> {
    let (hive, path) = split_key(key)?;
    if hive != Hive::CurrentUser {
        return Err(unsupported(key));
    }
    if !path.contains('\\') {
        return Err(RegistryError(format!(
            "Раздел слишком близко к корню, удалять его целиком нельзя: {key}"
        )));
    }
    Ok(path)
}

fn writable_path(key: &str) -> Result<&str, RegistryError> {
    let (hive, path) = split_key(key)?;
    if hive != Hive::CurrentUser || path.is_empty() {
        return Err(unsupported(key));
    }
    Ok(path)
}

// ---------------------------------------------------------------------------------- подделка

/// Операция над подделкой: что и как её просили сделать, по порядку.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegOp {
    Write {
        key: String,
        name: String,
        value: RegValue,
    },
    DeleteValue {
        key: String,
        name: String,
    },
    DeleteTree {
        key: String,
    },
}

#[derive(Debug, Clone)]
struct FakeKey {
    /// Полное имя, как оно записано при создании (`HKEY_CURRENT_USER\Software\X`).
    name: String,
    values: Vec<(String, RegValue)>,
}

#[derive(Debug, Default)]
struct FakeState {
    /// Нижний регистр полного имени → раздел. Родители создаются вместе с дочерним.
    keys: BTreeMap<String, FakeKey>,
    ops: Vec<RegOp>,
    fail_writes: bool,
}

/// Реестр в памяти для тестов: с журналом операций и возможностью отказать в записи.
#[derive(Debug, Default)]
pub struct FakeRegistry {
    state: Mutex<FakeState>,
}

impl FakeRegistry {
    pub fn new() -> FakeRegistry {
        FakeRegistry::default()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Заранее положить значение (без записи в журнал операций).
    pub fn with_value(self, key: &str, name: &str, value: RegValue) -> FakeRegistry {
        {
            let mut state = self.state();
            let full = full_key(key).expect("раздел подделки");
            put(&mut state, &full, name, value);
        }
        self
    }

    /// Положить пустой раздел (без записи в журнал операций).
    pub fn with_key(self, key: &str) -> FakeRegistry {
        {
            let mut state = self.state();
            ensure_key(&mut state, &full_key(key).expect("раздел подделки"));
        }
        self
    }

    /// Следующие записи отказывают (`true`) или снова проходят (`false`).
    pub fn set_fail_writes(&self, fail: bool) {
        self.state().fail_writes = fail;
    }

    /// Журнал операций с начала жизни подделки.
    pub fn ops(&self) -> Vec<RegOp> {
        self.state().ops.clone()
    }

    /// Разделы с хотя бы одним значением: `{ "<полный раздел>": { "<имя>": {type, data} } }`.
    /// Тот же вид читает [`FakeRegistry::from_json`].
    pub fn to_json(&self) -> Value {
        let state = self.state();
        let mut out = Map::new();
        for key in state.keys.values().filter(|key| !key.values.is_empty()) {
            let mut values = Map::new();
            for (name, value) in &key.values {
                values.insert(
                    name.clone(),
                    json!({"type": value.type_name(), "data": value.data_text()}),
                );
            }
            out.insert(key.name.clone(), Value::Object(values));
        }
        Value::Object(out)
    }

    /// Реестр из описания `{ "<раздел>": { "<имя>": {type: "REG_SZ"|"REG_EXPAND_SZ"|"REG_DWORD", data} } }`.
    pub fn from_json(model: &Value) -> Result<FakeRegistry, RegistryError> {
        let registry = FakeRegistry::new();
        let bad = |what: &str| RegistryError(format!("описание реестра: {what}"));
        for (key, values) in model.as_object().ok_or_else(|| bad("не объект"))? {
            let mut state = registry.state();
            let full = full_key(key)?;
            ensure_key(&mut state, &full);
            for (name, value) in values.as_object().ok_or_else(|| bad("раздел не объект"))?
            {
                let data = value["data"].as_str().ok_or_else(|| bad("нет data"))?;
                let parsed = match value["type"].as_str() {
                    Some("REG_SZ") => RegValue::Sz(data.to_owned()),
                    Some("REG_EXPAND_SZ") => RegValue::ExpandSz(data.to_owned()),
                    Some("REG_DWORD") => {
                        RegValue::Dword(parse_dword(data).ok_or_else(|| bad("DWORD"))?)
                    }
                    _ => return Err(bad("неизвестный тип")),
                };
                put(&mut state, &full, name, parsed);
            }
        }
        Ok(registry)
    }
}

fn parse_dword(text: &str) -> Option<u32> {
    match text.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// Раздел и его родители (кроме самого куста).
fn ensure_key(state: &mut FakeState, full: &str) {
    let parts: Vec<&str> = full.split('\\').collect();
    for end in 1..=parts.len() {
        let name = parts[..end].join("\\");
        state
            .keys
            .entry(name.to_lowercase())
            .or_insert_with(|| FakeKey {
                name,
                values: Vec::new(),
            });
    }
}

fn put(state: &mut FakeState, full: &str, name: &str, value: RegValue) {
    ensure_key(state, full);
    let key = state
        .keys
        .get_mut(&full.to_lowercase())
        .expect("раздел создан");
    let wanted = name.to_lowercase();
    match key
        .values
        .iter_mut()
        .find(|(candidate, _)| candidate.to_lowercase() == wanted)
    {
        Some(slot) => slot.1 = value,
        None => key.values.push((name.to_owned(), value)),
    }
}

impl Registry for FakeRegistry {
    fn read(&self, key: &str) -> Option<Vec<(String, RegValue)>> {
        let full = full_key(key).ok()?;
        self.state()
            .keys
            .get(&full.to_lowercase())
            .map(|key| key.values.clone())
    }

    fn write(&self, key: &str, name: &str, value: &RegValue) -> Result<(), RegistryError> {
        writable_path(key)?;
        let full = full_key(key)?;
        let mut state = self.state();
        if state.fail_writes {
            return Err(RegistryError("отказано в доступе".to_owned()));
        }
        state.ops.push(RegOp::Write {
            key: full.clone(),
            name: name.to_owned(),
            value: value.clone(),
        });
        put(&mut state, &full, name, value.clone());
        Ok(())
    }

    fn delete_value(&self, key: &str, name: &str) -> Result<(), RegistryError> {
        writable_path(key)?;
        let full = full_key(key)?;
        let mut state = self.state();
        if state.fail_writes {
            return Err(RegistryError("отказано в доступе".to_owned()));
        }
        state.ops.push(RegOp::DeleteValue {
            key: full.clone(),
            name: name.to_owned(),
        });
        if let Some(found) = state.keys.get_mut(&full.to_lowercase()) {
            let wanted = name.to_lowercase();
            found
                .values
                .retain(|(candidate, _)| candidate.to_lowercase() != wanted);
        }
        Ok(())
    }

    fn delete_tree(&self, key: &str) -> Result<(), RegistryError> {
        deletable_path(key)?;
        let full = full_key(key)?;
        let mut state = self.state();
        if state.fail_writes {
            return Err(RegistryError("отказано в доступе".to_owned()));
        }
        state.ops.push(RegOp::DeleteTree { key: full.clone() });
        let root = full.to_lowercase();
        let prefix = format!("{root}\\");
        state
            .keys
            .retain(|name, _| *name != root && !name.starts_with(&prefix));
        Ok(())
    }

    fn subkeys(&self, key: &str) -> Option<Vec<String>> {
        let full = full_key(key).ok()?;
        let root = full.to_lowercase();
        let state = self.state();
        state.keys.get(&root)?;
        let prefix = format!("{root}\\");
        let mut names: Vec<String> = state
            .keys
            .iter()
            .filter_map(|(name, found)| {
                let rest = name.strip_prefix(&prefix)?;
                (!rest.contains('\\')).then(|| found.name[prefix.len()..].to_owned())
            })
            .collect();
        names.sort_by_key(|name| name.to_lowercase());
        Some(names)
    }
}

// ---------------------------------------------------------------------------------- изменения

/// Что сделать со значением (`{ name: string | { dword } | null }` в 0.4.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueChange {
    Sz(String),
    Dword(u32),
    /// `"имя"=-`.
    Delete,
}

/// Изменение реестра (`changes` метода `apply`): значения раздела или удаление раздела.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Set {
        key: String,
        values: Vec<(String, ValueChange)>,
    },
    /// `[-раздел]`.
    Remove { key: String },
}

impl Change {
    pub fn key(&self) -> &str {
        match self {
            Change::Set { key, .. } | Change::Remove { key } => key,
        }
    }
}

/// `apply(changes)`: сначала проверяются все разделы (в 0.4.7 так же собирался весь `.reg`-файл
/// до `reg import`), затем изменения идут по порядку; первая ошибка прерывает остальное.
pub fn apply(registry: &dyn Registry, changes: &[Change]) -> Result<(), RegistryError> {
    for change in changes {
        match split_key(change.key())? {
            (Hive::CurrentUser, _) => {}
            _ => return Err(unsupported(change.key())),
        }
    }
    for change in changes {
        match change {
            Change::Set { key, values } => {
                for (name, value) in values {
                    match value {
                        ValueChange::Sz(text) => {
                            registry.write(key, name, &RegValue::Sz(text.clone()))?
                        }
                        ValueChange::Dword(number) => {
                            registry.write(key, name, &RegValue::Dword(*number))?
                        }
                        ValueChange::Delete => registry.delete_value(key, name)?,
                    }
                }
            }
            Change::Remove { key } => registry.delete_tree(key)?,
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------- ключи установки

/// Метка страницы загрузки: `HLSR-Setup-<версия>-ru.exe` / `-en.exe`. Файл без метки её не
/// оставляет — язык тогда берётся из системы.
pub fn origin_of(setup_file: &str) -> Option<&'static str> {
    let name = NodePath::new(Platform::Win32, "C:\\").basename(setup_file);
    // /-(ru|en)\.exe$/i: «-ru.exe» или «-en.exe» в конце имени; регистр сворачивается только в ASCII.
    let lower = name.to_ascii_lowercase();
    if lower.ends_with("-ru.exe") {
        Some("ru")
    } else if lower.ends_with("-en.exe") {
        Some("en")
    } else {
        None
    }
}

/// Данные установки для раздела `Software\<guid>` и записи «Программ и компонентов».
#[derive(Debug, Clone)]
pub struct InstallEntry<'a> {
    /// Каталог установки (уже приведённый к абсолютному виду).
    pub install_dir: &'a str,
    /// `{UUID}` заглавными; при обновлении — прежний.
    pub install_id: &'a str,
    /// Нужен ли ярлык на рабочем столе (пишется в `HLSRDesktopShortcut` как `1`/`0`).
    pub desktop_shortcut: bool,
    /// `X.Y.Z` из списка файлов.
    pub version: &'a str,
    /// Сумма размеров файлов списка.
    pub size_bytes: u64,
    /// `ru`/`en` по имени установщика ([`origin_of`]).
    pub origin: Option<&'a str>,
}

/// `Software\<guid>`: каталог, имя ярлыка, идентификатор установки, выбор по ярлыку на столе;
/// метка незаконченной установки NSIS удаляется.
pub fn install_key_change(id: &Identity, entry: &InstallEntry<'_>) -> Change {
    Change::Set {
        key: id.install_key.clone(),
        values: vec![
            (
                "InstallLocation".to_owned(),
                ValueChange::Sz(entry.install_dir.to_owned()),
            ),
            (
                "KeepShortcuts".to_owned(),
                ValueChange::Sz("true".to_owned()),
            ),
            (
                "ShortcutName".to_owned(),
                ValueChange::Sz(id.shortcut_name.clone()),
            ),
            (
                "HLSRInstallId".to_owned(),
                ValueChange::Sz(entry.install_id.to_owned()),
            ),
            (
                layout::DESKTOP_VALUE.to_owned(),
                ValueChange::Sz(if entry.desktop_shortcut { "1" } else { "0" }.to_owned()),
            ),
            (layout::SEED_FLAG.to_owned(), ValueChange::Delete),
        ],
    }
}

/// `…\Uninstall\<guid>`: запись «Программ и компонентов».
pub fn uninstall_key_change(id: &Identity, entry: &InstallEntry<'_>) -> Change {
    let exe = NodePath::new(Platform::Win32, "C:\\").join(&[entry.install_dir, layout::EXE]);
    let text = |value: String| ValueChange::Sz(value);
    Change::Set {
        key: id.uninstall_key.clone(),
        values: vec![
            (
                "DisplayName".to_owned(),
                text(format!("{} {}", id.product_name, entry.version)),
            ),
            ("DisplayVersion".to_owned(), text(entry.version.to_owned())),
            ("DisplayIcon".to_owned(), text(format!("{exe},0"))),
            ("Publisher".to_owned(), text(layout::PUBLISHER.to_owned())),
            (
                "InstallLocation".to_owned(),
                text(entry.install_dir.to_owned()),
            ),
            (
                "UninstallString".to_owned(),
                text(format!("\"{exe}\" {}", layout::UNINSTALL_FLAG)),
            ),
            (
                "QuietUninstallString".to_owned(),
                text(format!("\"{exe}\" {} /S", layout::UNINSTALL_FLAG)),
            ),
            ("NoModify".to_owned(), ValueChange::Dword(1)),
            ("NoRepair".to_owned(), ValueChange::Dword(1)),
            (
                "EstimatedSize".to_owned(),
                // Math.ceil(байты / 1024) и `>>> 0` при записи в .reg.
                ValueChange::Dword(entry.size_bytes.div_ceil(1024) as u32),
            ),
        ],
    }
}

/// `Software\HLSR`: метка страницы загрузки.
pub fn origin_change(origin: &str) -> Change {
    Change::Set {
        key: layout::ORIGIN_KEY.to_owned(),
        values: vec![(
            "InstallerOrigin".to_owned(),
            ValueChange::Sz(origin.to_owned()),
        )],
    }
}

/// Все изменения установки одним списком и в порядке `install.js`: `Software\<guid>`, запись
/// удаления и, если есть метка, `Software\HLSR`.
pub fn install_changes(id: &Identity, entry: &InstallEntry<'_>) -> Vec<Change> {
    let mut changes = vec![
        install_key_change(id, entry),
        uninstall_key_change(id, entry),
    ];
    if let Some(origin) = entry.origin {
        changes.push(origin_change(origin));
    }
    changes
}

/// Записать `Software\<guid>`.
pub fn write_install_keys(
    registry: &dyn Registry,
    id: &Identity,
    entry: &InstallEntry<'_>,
) -> Result<(), RegistryError> {
    apply(registry, &[install_key_change(id, entry)])
}

/// Записать запись «Программ и компонентов».
pub fn write_uninstall_keys(
    registry: &dyn Registry,
    id: &Identity,
    entry: &InstallEntry<'_>,
) -> Result<(), RegistryError> {
    apply(registry, &[uninstall_key_change(id, entry)])
}

/// Удаление: оба GUID-раздела, сначала запись «Программ и компонентов». `Software\HLSR` (метка
/// страницы загрузки) остаётся.
pub fn delete_keys(registry: &dyn Registry, id: &Identity) -> Result<(), RegistryError> {
    apply(
        registry,
        &[
            Change::Remove {
                key: id.uninstall_key.clone(),
            },
            Change::Remove {
                key: id.install_key.clone(),
            },
        ],
    )
}

// ---------------------------------------------------------------------------------- Win32

/// Реестр ОС: Win32 (`RegCreateKeyExW`, `RegSetValueExW`, `RegDeleteTreeW`, `RegEnumKeyExW`…).
#[cfg(windows)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemRegistry;

#[cfg(windows)]
mod system {
    use super::{
        Hive, RegValue, Registry, RegistryError, SystemRegistry, deletable_path, split_key,
        writable_path,
    };
    use hlsr_core::win32::wide;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_PATH_NOT_FOUND,
        ERROR_SUCCESS,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ENUMERATE_SUB_KEYS, KEY_QUERY_VALUE,
        KEY_SET_VALUE, REG_DWORD, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegEnumKeyExW, RegEnumValueW,
        RegOpenKeyExW, RegQueryInfoKeyW, RegSetValueExW,
    };

    struct Handle(HKEY);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: дескриптор получен от RegOpenKeyExW/RegCreateKeyExW и закрывается один раз.
            unsafe { RegCloseKey(self.0) };
        }
    }

    fn root(hive: Hive) -> HKEY {
        match hive {
            Hive::CurrentUser => HKEY_CURRENT_USER,
            Hive::LocalMachine => HKEY_LOCAL_MACHINE,
        }
    }

    /// Открыть раздел; `Ok(None)` — раздела нет.
    fn open(hive: Hive, path: &str, access: u32) -> Result<Option<Handle>, u32> {
        let wide_path = wide(path);
        let mut raw: HKEY = null_mut();
        // SAFETY: строка завершена нулём, `raw` — локальная переменная для дескриптора.
        let status = unsafe { RegOpenKeyExW(root(hive), wide_path.as_ptr(), 0, access, &mut raw) };
        match status {
            ERROR_SUCCESS => Ok(Some(Handle(raw))),
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(None),
            other => Err(other),
        }
    }

    fn failure(operation: &str, status: u32) -> RegistryError {
        RegistryError(format!("{operation}: код {status}"))
    }

    /// Строка из UTF-16 до первого нуля (как печатает `reg query`).
    fn text_of(data: &[u8]) -> String {
        let units: Vec<u16> = data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        let end = units
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(units.len());
        String::from_utf16_lossy(&units[..end])
    }

    /// Размеры для буферов перечисления: подразделы (знаков) и значения (знаков имени, байт данных).
    fn limits(handle: &Handle) -> Option<(u32, u32, u32)> {
        let (mut max_sub, mut max_name, mut max_data) = (0u32, 0u32, 0u32);
        // SAFETY: все указатели — на локальные переменные или пустые.
        let status = unsafe {
            RegQueryInfoKeyW(
                handle.0,
                null_mut(),
                null_mut(),
                null(),
                null_mut(),
                &mut max_sub,
                null_mut(),
                null_mut(),
                &mut max_name,
                &mut max_data,
                null_mut(),
                null_mut(),
            )
        };
        (status == ERROR_SUCCESS).then_some((max_sub, max_name, max_data))
    }

    impl Registry for SystemRegistry {
        fn read(&self, key: &str) -> Option<Vec<(String, RegValue)>> {
            let (hive, path) = split_key(key).ok()?;
            let handle = open(hive, path, KEY_QUERY_VALUE).ok()??;
            let (_, mut max_name, mut max_data) = limits(&handle)?;
            let mut values = Vec::new();
            let mut index = 0u32;
            let mut grown = 0;
            loop {
                let mut name = vec![0u16; max_name as usize + 1];
                let mut name_len = name.len() as u32;
                let mut data = vec![0u8; max_data as usize];
                let mut data_len = max_data;
                let mut kind = 0u32;
                // SAFETY: буферы вмещают заявленные длины, указатели — на локальные значения.
                let status = unsafe {
                    RegEnumValueW(
                        handle.0,
                        index,
                        name.as_mut_ptr(),
                        &mut name_len,
                        null(),
                        &mut kind,
                        data.as_mut_ptr(),
                        &mut data_len,
                    )
                };
                match status {
                    ERROR_SUCCESS => {
                        data.truncate(data_len as usize);
                        let name = String::from_utf16_lossy(&name[..name_len as usize]);
                        match kind {
                            REG_SZ => values.push((name, RegValue::Sz(text_of(&data)))),
                            REG_EXPAND_SZ => {
                                values.push((name, RegValue::ExpandSz(text_of(&data))))
                            }
                            REG_DWORD if data.len() == 4 => values.push((
                                name,
                                RegValue::Dword(u32::from_le_bytes([
                                    data[0], data[1], data[2], data[3],
                                ])),
                            )),
                            _ => {}
                        }
                        index += 1;
                    }
                    ERROR_NO_MORE_ITEMS => break,
                    ERROR_MORE_DATA if grown < 8 => {
                        // Значение выросло между вопросом о размерах и чтением.
                        grown += 1;
                        max_name = max_name.max(name_len).saturating_mul(2).max(260);
                        max_data = max_data.max(data_len).saturating_mul(2).max(1024);
                    }
                    _ => return None,
                }
            }
            Some(values)
        }

        fn write(&self, key: &str, name: &str, value: &RegValue) -> Result<(), RegistryError> {
            let path = writable_path(key)?;
            let wide_path = wide(path);
            let mut raw: HKEY = null_mut();
            // SAFETY: строка завершена нулём, `raw` — локальная переменная для дескриптора.
            let status = unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    wide_path.as_ptr(),
                    0,
                    null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_SET_VALUE,
                    null(),
                    &mut raw,
                    null_mut(),
                )
            };
            if status != ERROR_SUCCESS {
                return Err(failure("RegCreateKeyExW", status));
            }
            let handle = Handle(raw);
            let wide_name = wide(name);
            let (kind, bytes): (u32, Vec<u8>) = match value {
                RegValue::Sz(text) => (
                    REG_SZ,
                    wide(text)
                        .iter()
                        .flat_map(|unit| unit.to_le_bytes())
                        .collect(),
                ),
                RegValue::ExpandSz(text) => (
                    REG_EXPAND_SZ,
                    wide(text)
                        .iter()
                        .flat_map(|unit| unit.to_le_bytes())
                        .collect(),
                ),
                RegValue::Dword(number) => (REG_DWORD, number.to_le_bytes().to_vec()),
            };
            // SAFETY: имя завершено нулём, `bytes` вмещает `len` байт.
            let status = unsafe {
                RegSetValueExW(
                    handle.0,
                    wide_name.as_ptr(),
                    0,
                    kind,
                    bytes.as_ptr(),
                    bytes.len() as u32,
                )
            };
            if status != ERROR_SUCCESS {
                return Err(failure("RegSetValueExW", status));
            }
            Ok(())
        }

        fn delete_value(&self, key: &str, name: &str) -> Result<(), RegistryError> {
            let path = writable_path(key)?;
            let Some(handle) = open(Hive::CurrentUser, path, KEY_SET_VALUE)
                .map_err(|status| failure("RegOpenKeyExW", status))?
            else {
                return Ok(());
            };
            let wide_name = wide(name);
            // SAFETY: имя завершено нулём.
            let status = unsafe { RegDeleteValueW(handle.0, wide_name.as_ptr()) };
            match status {
                ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
                other => Err(failure("RegDeleteValueW", other)),
            }
        }

        fn delete_tree(&self, key: &str) -> Result<(), RegistryError> {
            let path = deletable_path(key)?;
            let wide_path = wide(path);
            // SAFETY: строка завершена нулём; путь не пустой и глубже первого уровня (deletable_path).
            let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide_path.as_ptr()) };
            match status {
                ERROR_SUCCESS | ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(()),
                other => Err(failure("RegDeleteTreeW", other)),
            }
        }

        fn subkeys(&self, key: &str) -> Option<Vec<String>> {
            let (hive, path) = split_key(key).ok()?;
            let handle = open(hive, path, KEY_ENUMERATE_SUB_KEYS | KEY_QUERY_VALUE).ok()??;
            let (max_sub, _, _) = limits(&handle)?;
            let mut names = Vec::new();
            let mut index = 0u32;
            let mut capacity = max_sub.max(255) + 1;
            loop {
                let mut name = vec![0u16; capacity as usize];
                let mut name_len = capacity;
                // SAFETY: буфер вмещает `name_len` знаков, прочие указатели пустые.
                let status = unsafe {
                    RegEnumKeyExW(
                        handle.0,
                        index,
                        name.as_mut_ptr(),
                        &mut name_len,
                        null(),
                        null_mut(),
                        null_mut(),
                        null_mut(),
                    )
                };
                match status {
                    ERROR_SUCCESS => {
                        names.push(String::from_utf16_lossy(&name[..name_len as usize]));
                        index += 1;
                    }
                    ERROR_NO_MORE_ITEMS => break,
                    ERROR_MORE_DATA => capacity = capacity.saturating_mul(2),
                    _ => return None,
                }
            }
            Some(names)
        }
    }
}
