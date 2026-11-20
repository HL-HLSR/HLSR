//! Зависимости доставки от внешнего мира — трейты (спецификация, решение 2).
//! Реализации для ОС — здесь или в подпроекте 3; для тестов — подделки.
//! Трейты добавляются задачами по мере надобности.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Журнал сервисов — общий с онлайном (`hlsr_core::logger`).
pub use hlsr_core::logger::{Logger, MemoryLogger, TracingLogger};

/// Свободное место на томе каталога.
pub trait FreeSpace: Send + Sync {
    /// Свободные байты для каталога `dir`; `None` — узнать не удалось.
    fn free_bytes(&self, dir: &Path) -> Option<u64>;
}

/// Свободное место средствами ОС, как `free-space.js` без кеша.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemFreeSpace;

/// Linux: `statfs(dir)` libuv — `f_bavail × f_bsize`; каталога нет — `None`.
#[cfg(unix)]
impl FreeSpace for SystemFreeSpace {
    fn free_bytes(&self, dir: &Path) -> Option<u64> {
        let stat = rustix::fs::statvfs(dir).ok()?;
        stat.f_bavail.checked_mul(stat.f_bsize)
    }
}

/// Windows: `statfs` libuv — это `GetDiskFreeSpaceW(dir)` (место, доступное
/// пользователю; для файла — его каталог). Не вышло — запасной путь 0.4.7,
/// `fsutil volume diskfree <корень>`: общий объём свободного места тома.
#[cfg(windows)]
impl FreeSpace for SystemFreeSpace {
    fn free_bytes(&self, dir: &Path) -> Option<u64> {
        const ERROR_DIRECTORY: i32 = 267;
        match disk_free(dir) {
            Ok((available, _)) => return Some(available),
            Err(error) if error.raw_os_error() == Some(ERROR_DIRECTORY) => {
                let parent = std::path::absolute(dir)
                    .ok()?
                    .parent()
                    .map(Path::to_path_buf);
                if let Some(Ok((available, _))) = parent.map(|p| disk_free(&p)) {
                    return Some(available);
                }
            }
            Err(_) => {}
        }
        let dir = if dir.as_os_str().is_empty() {
            Path::new("C:\\")
        } else {
            dir
        };
        let root = std::path::absolute(dir)
            .ok()?
            .ancestors()
            .last()?
            .to_path_buf();
        disk_free(&root).ok().map(|(_, total_free)| total_free)
    }
}

/// `GetDiskFreeSpaceExW`: (доступно пользователю, всего свободно на томе).
#[cfg(windows)]
fn disk_free(dir: &Path) -> std::io::Result<(u64, u64)> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide = hlsr_core::win32::wide(dir);
    let (mut available, mut total, mut total_free) = (0u64, 0u64, 0u64);
    // SAFETY: строка завершена нулём, указатели — на локальные u64.
    let ok =
        unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut total_free) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((available, total_free))
}

/// Подделка для тестов: заданные байты по каталогу и счётчик вызовов.
#[derive(Debug, Default)]
pub struct FakeFreeSpace {
    values: Mutex<HashMap<PathBuf, Option<u64>>>,
    calls: AtomicUsize,
}

impl FakeFreeSpace {
    /// Что вернуть для `dir` (по умолчанию — `None`).
    pub fn set(&self, dir: &Path, bytes: Option<u64>) {
        self.values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(dir.to_path_buf(), bytes);
    }

    /// Сколько раз спрашивали.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl FreeSpace for FakeFreeSpace {
    fn free_bytes(&self, dir: &Path) -> Option<u64> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(dir)
            .copied()
            .flatten()
    }
}

// ------------------------------------------------------------------- реестр Windows

/// Куст реестра, в котором 0.4.7 ищет значения: `HKCU` или `HKLM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegistryHive {
    CurrentUser,
    LocalMachine,
}

impl RegistryHive {
    /// Имя куста в первой строке вывода `reg query`.
    pub fn long_name(self) -> &'static str {
        match self {
            RegistryHive::CurrentUser => "HKEY_CURRENT_USER",
            RegistryHive::LocalMachine => "HKEY_LOCAL_MACHINE",
        }
    }
}

/// Значение реестра тех типов, которые читает 0.4.7: строка, строка с переменными
/// окружения и число. Значения других типов трейт не отдаёт (`None`): лаунчер их не читает.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryValue {
    /// `REG_SZ`.
    Sz(String),
    /// `REG_EXPAND_SZ` (переменные окружения не раскрыты, как в `reg query`).
    ExpandSz(String),
    /// `REG_DWORD`.
    Dword(u32),
}

impl RegistryValue {
    /// Тип значения, как его печатает `reg query`.
    pub fn type_name(&self) -> &'static str {
        match self {
            RegistryValue::Sz(_) => "REG_SZ",
            RegistryValue::ExpandSz(_) => "REG_EXPAND_SZ",
            RegistryValue::Dword(_) => "REG_DWORD",
        }
    }

    /// Данные, как их печатает `reg query`: строка как есть, число — `0x` и строчные
    /// шестнадцатеричные цифры без нулей слева.
    pub fn data_text(&self) -> String {
        match self {
            RegistryValue::Sz(text) | RegistryValue::ExpandSz(text) => text.clone(),
            RegistryValue::Dword(number) => format!("0x{number:x}"),
        }
    }
}

/// Вывод `reg query HKxx\<ключ> /v <имя>` для найденного значения: пустая строка, ключ с
/// длинным именем куста, строка значения (четыре пробела между полями), пустая строка;
/// переводы строк — `\r\n`. Разбор 0.4.7 (регулярные выражения по этому тексту) на таком
/// выводе даёт тот же ответ, что на выводе настоящего `reg.exe`.
pub fn reg_query_output(
    hive: RegistryHive,
    key: &str,
    name: &str,
    value: &RegistryValue,
) -> String {
    format!(
        "\r\n{}\\{key}\r\n    {name}    {}    {}\r\n\r\n",
        hive.long_name(),
        value.type_name(),
        value.data_text()
    )
}

/// Реестр Windows: то, что 0.4.7 читает командой `reg.exe query`. На Linux реестра нет,
/// и значений в нём тоже (игра идёт в префиксе Proton, а клиента Steam для Windows нет).
pub trait Registry: Send + Sync {
    /// Значение `name` в ключе `key` (путь от куста через `\`) или `None`: нет ключа,
    /// нет значения, неподдерживаемый тип, нет доступа. Ключ и имя без учёта регистра.
    fn read_value(&self, hive: RegistryHive, key: &str, name: &str) -> Option<RegistryValue>;

    /// Вывод `reg query` для этого значения; `None` — `reg.exe` завершился бы с ошибкой
    /// (`execFileSync` бросил бы исключение).
    fn query(&self, hive: RegistryHive, key: &str, name: &str) -> Option<String> {
        self.read_value(hive, key, name)
            .map(|value| reg_query_output(hive, key, name, &value))
    }
}

/// Реестр ОС: на Windows — Win32 API (`RegQueryValueExW`), на других ОС значений нет.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemRegistry;

#[cfg(not(windows))]
impl Registry for SystemRegistry {
    fn read_value(&self, _hive: RegistryHive, _key: &str, _name: &str) -> Option<RegistryValue> {
        None
    }
}

#[cfg(windows)]
impl Registry for SystemRegistry {
    fn read_value(&self, hive: RegistryHive, key: &str, name: &str) -> Option<RegistryValue> {
        read_registry_value(hive, key, name)
    }
}

#[cfg(windows)]
fn read_registry_value(hive: RegistryHive, key: &str, name: &str) -> Option<RegistryValue> {
    use hlsr_core::win32::wide;
    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_DWORD, REG_EXPAND_SZ, REG_SZ,
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
    };

    struct Handle(HKEY);
    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: дескриптор получен от RegOpenKeyExW и закрывается один раз.
            unsafe { RegCloseKey(self.0) };
        }
    }

    let root = match hive {
        RegistryHive::CurrentUser => HKEY_CURRENT_USER,
        RegistryHive::LocalMachine => HKEY_LOCAL_MACHINE,
    };
    let wide_key = wide(key);
    let wide_name = wide(name);
    let mut raw: HKEY = std::ptr::null_mut();
    // SAFETY: строка завершена нулём, `raw` — локальная переменная для дескриптора.
    let opened = unsafe { RegOpenKeyExW(root, wide_key.as_ptr(), 0, KEY_READ, &mut raw) };
    if opened != ERROR_SUCCESS {
        return None;
    }
    let handle = Handle(raw);

    // Значение могут изменить между вопросом о размере и чтением: тогда ERROR_MORE_DATA и
    // ещё одна попытка с новым размером.
    let mut size: u32 = 0;
    let mut kind: u32 = 0;
    for _ in 0..4 {
        // SAFETY: пустой буфер с размером — запрос длины; все указатели — на локальные значения.
        let status = unsafe {
            RegQueryValueExW(
                handle.0,
                wide_name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        let mut capacity = size;
        // SAFETY: буфер `data` вмещает `capacity` байт.
        let status = unsafe {
            RegQueryValueExW(
                handle.0,
                wide_name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                data.as_mut_ptr(),
                &mut capacity,
            )
        };
        if status == ERROR_MORE_DATA {
            size = capacity;
            continue;
        }
        if status != ERROR_SUCCESS {
            return None;
        }
        data.truncate(capacity as usize);
        let text = || {
            let units: Vec<u16> = data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect();
            // `reg query` печатает строку до первого нуля.
            let end = units
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(units.len());
            String::from_utf16_lossy(&units[..end])
        };
        return match kind {
            REG_SZ => Some(RegistryValue::Sz(text())),
            REG_EXPAND_SZ => Some(RegistryValue::ExpandSz(text())),
            REG_DWORD if data.len() == 4 => Some(RegistryValue::Dword(u32::from_le_bytes([
                data[0], data[1], data[2], data[3],
            ]))),
            _ => None,
        };
    }
    None
}

/// Подделка реестра для тестов: заданные значения (ключ и имя без учёта регистра) и счётчик
/// вопросов к `read_value`.
#[derive(Debug, Default)]
pub struct FakeRegistry {
    values: Mutex<HashMap<(RegistryHive, String, String), RegistryValue>>,
    calls: AtomicUsize,
}

impl FakeRegistry {
    fn slot(hive: RegistryHive, key: &str, name: &str) -> (RegistryHive, String, String) {
        (hive, key.to_lowercase(), name.to_lowercase())
    }

    /// Задать значение.
    pub fn set(&self, hive: RegistryHive, key: &str, name: &str, value: RegistryValue) {
        self.values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(Self::slot(hive, key, name), value);
    }

    /// Убрать значение (как `reg delete`).
    pub fn remove(&self, hive: RegistryHive, key: &str, name: &str) {
        self.values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&Self::slot(hive, key, name));
    }

    /// Сколько раз спрашивали значение.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Registry for FakeRegistry {
    fn read_value(&self, hive: RegistryHive, key: &str, name: &str) -> Option<RegistryValue> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&Self::slot(hive, key, name))
            .cloned()
    }
}

// ------------------------------------------------------------------- запуск инструментов

/// Запущенный из лаунчера инструмент (`running.get(toolId)` в `ToolService.js`): для какой игры
/// он открыт (на Linux — чей префикс Proton), с каким набором сплитов и закрывается ли сейчас.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunningTool {
    pub game_id: Option<String>,
    pub splits: Option<String>,
    pub stopping: bool,
}

/// Что `ToolService` подготовил к запуску инструмента; сам запуск (Windows — напрямую, Linux —
/// через Proton в префиксе игры), слежение за процессом и остановка — подпроект 3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolLaunch {
    pub tool_id: String,
    /// Исполняемый файл: `<папка инструмента>/<executable>`.
    pub exe: PathBuf,
    /// Папка инструмента (рабочая папка процесса).
    pub root: PathBuf,
    /// Аргументы (у LiveSplit — `-s <сплиты> -l <раскладка>`).
    pub args: Vec<String>,
    /// Для какой игры открыт инструмент.
    pub game_id: Option<String>,
    /// Набор сплитов (`launchInfo.splits`).
    pub splits: Option<String>,
    /// Имя исполняемого файла из `installed.json` или манифеста (для слежения за процессом Wine).
    pub executable: String,
    /// Linux: добавки к окружению Proton (`toolLaunchEnvironment`).
    pub env: Option<Vec<(String, String)>>,
    /// Linux: профиль шрифтов префикса (`livesplit`).
    pub font_profile: Option<String>,
}

/// Итог остановки инструмента.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopStatus {
    NotRunning,
    Stopping,
    Stopped,
    Timeout,
}

impl StopStatus {
    /// `status` ответа 0.4.7.
    pub fn as_str(self) -> &'static str {
        match self {
            StopStatus::NotRunning => "not-running",
            StopStatus::Stopping => "stopping",
            StopStatus::Stopped => "stopped",
            StopStatus::Timeout => "timeout",
        }
    }
}

/// Запуск инструментов (LiveSplit): процессы — подпроект 3. До него — [`UnavailableToolRunner`].
pub trait ToolRunner: Send + Sync {
    /// Запущен ли инструмент (и для чего); `None` — нет.
    fn running(&self, tool_id: &str) -> Option<RunningTool>;

    /// Запустить; `pid` процесса, если он известен.
    fn launch<'a>(
        &'a self,
        request: &'a ToolLaunch,
    ) -> crate::providers::BoxFuture<'a, Result<Option<u32>, crate::errors::Failure>>;

    /// Закрыть: мягко или (`force`) принудительно. `STOP_FAILED` и `PROCESS_NOT_FOUND` — ошибки.
    fn stop<'a>(
        &'a self,
        tool_id: &'a str,
        force: bool,
    ) -> crate::providers::BoxFuture<'a, Result<StopStatus, crate::errors::Failure>>;
}

/// Заглушка до подпроекта 3: ничего не запущено, запуск невозможен. Ответ запуска — как у 0.4.7
/// на Linux без настроенного Proton (`PROTON_NOT_INSTALLED` не входит в `CODES`, поэтому код
/// становится `INSTALL_FAILED`).
#[derive(Debug, Clone, Copy, Default)]
pub struct UnavailableToolRunner;

impl ToolRunner for UnavailableToolRunner {
    fn running(&self, _tool_id: &str) -> Option<RunningTool> {
        None
    }

    fn launch<'a>(
        &'a self,
        _request: &'a ToolLaunch,
    ) -> crate::providers::BoxFuture<'a, Result<Option<u32>, crate::errors::Failure>> {
        Box::pin(async {
            Err(crate::errors::DeliveryError::new(
                crate::errors::Code::coerce("PROTON_NOT_INSTALLED"),
                "Proton не настроен",
            )
            .into())
        })
    }

    fn stop<'a>(
        &'a self,
        _tool_id: &'a str,
        _force: bool,
    ) -> crate::providers::BoxFuture<'a, Result<StopStatus, crate::errors::Failure>> {
        Box::pin(async { Ok(StopStatus::NotRunning) })
    }
}

// ------------------------------------------------------------------- процесс игры

/// Состояние процесса игры, которого лаунчер не отслеживает (`defaultState` → `publicState`
/// в `GameProcessService.js`).
pub fn process_not_running(game_id: &str) -> Value {
    json!({
        "gameId": game_id,
        "state": "not-running",
        "running": false,
        "since": null,
        "startedAt": null,
        "exitCode": null,
        "lastExit": null,
    })
}

/// Процессы игр (`GameProcessService`, подпроект 3) — то, что видит доставка: состояние для
/// статуса (`getProcessState`, объект `publicState`) и запущена ли игра (`isRunning`). О смене
/// состояния подпроект 3 сообщает сервису доставки (`GameDeliveryService::process_state_changed`).
pub trait ProcessProbe: Send + Sync {
    /// `getProcessState(gameId)`: `{gameId, state, running, since, startedAt, exitCode, lastExit}`.
    fn process_state(&self, game_id: &str) -> Value;

    /// `isRunning(gameId)`: игра в состоянии `starting`, `running` или `stopping`.
    fn is_running(&self, game_id: &str) -> bool;

    /// `anyRunning()`: первая (по порядку запуска) запущенная игра.
    fn any_running(&self) -> Option<String>;

    /// `reconcileAfterLauncherRestart(gameIds)`: игры, пережившие перезапуск лаунчера (пусто —
    /// все записи), — `[{gameId, adopted, …}]`.
    fn reconcile_after_launcher_restart(&self, _game_ids: &[String]) -> Vec<Value> {
        Vec::new()
    }

    /// `dispose()`: опрос процессов остановлен (служба доставки сменилась).
    fn dispose(&self) {}
}

/// Остановка игры (`processService.requestStop(gameId, {force})`): `{status: …}` или ошибка.
pub trait ProcessStopper: Send + Sync {
    fn request_stop<'a>(
        &'a self,
        game_id: &'a str,
        force: bool,
    ) -> crate::providers::BoxFuture<'a, Result<Value, crate::errors::Failure>>;
}

/// Заглушка до подпроекта 3: лаунчер не запускал ни одной игры.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoGameProcesses;

impl ProcessProbe for NoGameProcesses {
    fn process_state(&self, game_id: &str) -> Value {
        process_not_running(game_id)
    }

    fn is_running(&self, _game_id: &str) -> bool {
        false
    }

    fn any_running(&self) -> Option<String> {
        None
    }
}

impl ProcessStopper for NoGameProcesses {
    fn request_stop<'a>(
        &'a self,
        _game_id: &'a str,
        _force: bool,
    ) -> crate::providers::BoxFuture<'a, Result<Value, crate::errors::Failure>> {
        Box::pin(async { Ok(json!({"status": "not-running"})) })
    }
}

// ------------------------------------------------------------------- Steam: лицензии и депо

/// Ход установки из депо (`onProgress` у `DepotInstaller.install`): файлов собрано, сколько
/// ожидается, номер депо и их число.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepotProgress {
    pub files: u64,
    pub expected_files: Option<u64>,
    pub index: usize,
    pub total: usize,
}

pub type DepotProgressFn = std::sync::Arc<dyn Fn(&DepotProgress) + Send + Sync>;

/// Steam (подпроект 3): проверка лицензий (`SteamDepotService.assertOwns`) и установка сборки
/// из депо (`DepotInstaller.install`). Заглушка — отсутствие службы (`depotService: null` в
/// 0.4.7): сборкам с лицензией — `LICENSE_CHECK_UNAVAILABLE`, депо — `PROVIDER_UNSUPPORTED`.
pub trait SteamDepot: Send + Sync {
    /// `assertOwns(appIds)`: все игры есть на аккаунте; иначе ошибка (`STEAM_APP_NOT_OWNED` …).
    fn assert_owns<'a>(
        &'a self,
        app_ids: &'a [u32],
    ) -> crate::providers::BoxFuture<'a, Result<(), crate::errors::Failure>>;

    /// `DepotInstaller.install(gameId, {onProgress})`: сборка в библиотеке, итог — запись
    /// `installed.json`.
    fn install<'a>(
        &'a self,
        paths: &'a crate::paths::LibraryPaths,
        game_id: &'a str,
        on_progress: DepotProgressFn,
    ) -> crate::providers::BoxFuture<'a, Result<Value, crate::errors::Failure>>;
}

// ------------------------------------------------------------------- уведомления (подпроект 4)

/// Сведения к событию версии игры: `{hasReleaseNotes, launcherId}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionEventInfo {
    pub has_release_notes: bool,
    pub launcher_id: Option<String>,
}

/// Центр уведомлений (подпроект 4): уведомление по коду (`notify(code, detail)` фасада 0.4.7,
/// уровень — по каталогу) и события версий игр (`VersionEvents`). Ошибка — текст для журнала.
pub trait Notifier: Send + Sync {
    fn notify(&self, code: &str, detail: &str);

    /// `resolveUpdateOffers(target, installedVersion, gameId)`.
    fn resolve_update_offers(
        &self,
        target: &str,
        installed_version: Option<&str>,
        game_id: &str,
    ) -> Result<(), String>;

    /// `gameUpdateAvailable(version, currentVersion, gameId, options)`.
    fn game_update_available(
        &self,
        version: &str,
        current_version: Option<&str>,
        game_id: &str,
        info: &VersionEventInfo,
    ) -> Result<(), String>;

    /// `gameInstalled(version, gameId, options)`.
    fn game_installed(
        &self,
        version: &str,
        game_id: &str,
        info: &VersionEventInfo,
    ) -> Result<(), String>;

    /// `gameUpdated(fromVersion, toVersion, gameId, options)`.
    fn game_updated(
        &self,
        from_version: Option<&str>,
        to_version: &str,
        game_id: &str,
        info: &VersionEventInfo,
    ) -> Result<(), String>;
}

/// Заглушка до подпроекта 4: уведомления никуда не идут.
#[derive(Debug, Clone, Copy, Default)]
pub struct SilentNotifier;

impl Notifier for SilentNotifier {
    fn notify(&self, _code: &str, _detail: &str) {}

    fn resolve_update_offers(
        &self,
        _target: &str,
        _installed_version: Option<&str>,
        _game_id: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    fn game_update_available(
        &self,
        _version: &str,
        _current_version: Option<&str>,
        _game_id: &str,
        _info: &VersionEventInfo,
    ) -> Result<(), String> {
        Ok(())
    }

    fn game_installed(
        &self,
        _version: &str,
        _game_id: &str,
        _info: &VersionEventInfo,
    ) -> Result<(), String> {
        Ok(())
    }

    fn game_updated(
        &self,
        _from_version: Option<&str>,
        _to_version: &str,
        _game_id: &str,
        _info: &VersionEventInfo,
    ) -> Result<(), String> {
        Ok(())
    }
}

/// Телеметрия (подпроект 4): `telemetry.emit(type, fields)`; ошибка — текст для журнала.
pub trait Telemetry: Send + Sync {
    fn emit(&self, event_type: &str, fields: &Value) -> Result<(), String>;
}

/// Заглушка до подпроекта 4: телеметрия выключена.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTelemetry;

impl Telemetry for NoTelemetry {
    fn emit(&self, _event_type: &str, _fields: &Value) -> Result<(), String> {
        Ok(())
    }
}

/// Достижения онлайн-профиля (подпроект 4): `onlineProfiles.achieve(id)`.
pub trait Achievements: Send + Sync {
    fn achieve(&self, id: &str);
}

/// Заглушка до подпроекта 4: профиля нет.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAchievements;

impl Achievements for NoAchievements {
    fn achieve(&self, _id: &str) {}
}

/// Сигналы Discord (подпроект 3): `{type: "status" | "running" | "stopped" | "selected", gameId,
/// …}`; ошибка — текст для журнала.
pub trait PresenceSink: Send + Sync {
    fn signal(&self, signal: &Value) -> Result<(), String>;
}

/// Заглушка до подпроекта 3: обработчика присутствия нет.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoPresence;

impl PresenceSink for NoPresence {
    fn signal(&self, _signal: &Value) -> Result<(), String> {
        Ok(())
    }
}

/// Сообщение игры потребителю чата: то, что `game-presence/protocol.js` 0.4.7 разбирает из строки
/// и не отдаёт как `hello`/`state` (`PresenceBridgeServer.onLine` → `chatHandler`). Поля уже
/// проверены протоколом: `session` — 8…64 шестнадцатеричных знака, `platform` — `twitch` или
/// `youtube`, `room_id` — источник набора эмоутов (`12345`, `twitch:12345`, `set:<id>`,
/// `user:<id>`), `name` — имя эмоута (`tw:<id>` у Twitch), `px` — 8…128 или `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatMessage {
    /// `chat.subscribe {session, platform, id}`.
    Subscribe {
        session: String,
        platform: String,
        id: String,
    },
    /// `chat.unsubscribe {session}`.
    Unsubscribe { session: String },
    /// `emote.set {session, roomId}`.
    EmoteSet { session: String, room_id: String },
    /// `emote.resolve {session, name, px}`.
    EmoteResolve {
        session: String,
        name: String,
        px: Option<u32>,
    },
}

/// Сторона моста присутствия, куда пишут потребители чата (`PresenceBridgeServer.send*` 0.4.7;
/// реализует подпроект 3, задача 4). Строки пишутся игре, пока она на связи; `false` — игры
/// нет или запись не удалась. `rgba_base64` — RGBA в base64, как отдаёт `Buffer.toString`.
/// Аргументы обрезает кодировщик протокола (`encodeChatLine`, `encodeEmoteReady`, …): имя эмоута
/// до 47 знаков, значка до 31, ник до 32, текст до 240.
pub trait ChatBridge: Send + Sync {
    /// `sendChatLine(platform, nick, text, badge1, badge2)`: `chat.line`.
    fn send_chat_line(
        &self,
        platform: &str,
        nick: &str,
        text: &str,
        badge1: &str,
        badge2: &str,
    ) -> bool;

    /// `sendEmoteReady(name, wide, tall, rgbaBase64)`: `emote.ready`, неподвижный эмоут.
    fn send_emote_ready(&self, name: &str, wide: u32, tall: u32, rgba_base64: &str) -> bool;

    /// `sendEmoteBegin(name, wide, tall, frames, frameMs)`: `emote.begin`, начало анимации.
    fn send_emote_begin(
        &self,
        name: &str,
        wide: u32,
        tall: u32,
        frames: u32,
        frame_ms: u32,
    ) -> bool;

    /// `sendEmoteFrame(name, index, rgbaBase64)`: `emote.frame`, кадр анимации.
    fn send_emote_frame(&self, name: &str, index: u32, rgba_base64: &str) -> bool;

    /// `sendEmoteMiss(name)`: `emote.miss`, эмоута нет.
    fn send_emote_miss(&self, name: &str) -> bool;

    /// `sendBadgeReady(name, wide, tall, rgbaBase64)`: `badge.ready`, значок.
    fn send_badge_ready(&self, name: &str, wide: u32, tall: u32, rgba_base64: &str) -> bool;
}

/// Заглушка до подпроекта 3: игры на связи нет, ничего не уходит.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoChatBridge;

impl ChatBridge for NoChatBridge {
    fn send_chat_line(&self, _: &str, _: &str, _: &str, _: &str, _: &str) -> bool {
        false
    }
    fn send_emote_ready(&self, _: &str, _: u32, _: u32, _: &str) -> bool {
        false
    }
    fn send_emote_begin(&self, _: &str, _: u32, _: u32, _: u32, _: u32) -> bool {
        false
    }
    fn send_emote_frame(&self, _: &str, _: u32, _: &str) -> bool {
        false
    }
    fn send_emote_miss(&self, _: &str) -> bool {
        false
    }
    fn send_badge_ready(&self, _: &str, _: u32, _: u32, _: &str) -> bool {
        false
    }
}

/// Потребитель чата игры (`attachChatConsumer` в `AppManager.js:342-345`): 7TV, Twitch, значки,
/// YouTube. Мост присутствия (подпроект 3, задача 4) вызывает `attach_bridge` при подключении
/// потребителя, `handle_message` — на каждое сообщение игры, кроме `hello`/`state`,
/// `handle_ready` — после рукопожатия, `forget` — когда игра ушла. Вызовы быстрые: тяжёлую работу
/// потребитель уносит в свои задачи (им нужен рантайм tokio, его выдают при создании).
pub trait ChatConsumer: Send + Sync {
    /// `attachBridge(server)`: куда писать игре.
    fn attach_bridge(&self, bridge: std::sync::Arc<dyn ChatBridge>);

    /// `handleMessage(message)`.
    fn handle_message(&self, message: &ChatMessage);

    /// `handleReady()`: у потребителей без этого метода в 0.4.7 — ничего.
    fn handle_ready(&self) {}

    /// `forget()`.
    fn forget(&self) {}
}

/// Заглушка до подпроекта 3: потребитель, который ничего не делает.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoChatConsumer;

impl ChatConsumer for NoChatConsumer {
    fn attach_bridge(&self, _bridge: std::sync::Arc<dyn ChatBridge>) {}
    fn handle_message(&self, _message: &ChatMessage) {}
}

/// Фильтр диалога выбора файла: `{name, extensions}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

impl FileFilter {
    pub fn new(name: impl Into<String>, extensions: &[&str]) -> FileFilter {
        FileFilter {
            name: name.into(),
            extensions: extensions.iter().map(|ext| (*ext).to_owned()).collect(),
        }
    }
}

/// `dialog.showOpenDialog(window, options)`: заголовок, папка или файл по умолчанию, свойства
/// (`openFile`, `openDirectory`, `multiSelections`, `createDirectory`) и фильтры.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenDialog {
    pub title: Option<String>,
    pub default_path: Option<String>,
    pub properties: Vec<&'static str>,
    pub filters: Vec<FileFilter>,
}

/// `dialog.showSaveDialog(window, options)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SaveDialog {
    pub default_path: Option<String>,
    pub filters: Vec<FileFilter>,
}

/// Диалоги выбора файлов и папок (подпроект 5a — окно).
pub trait Dialogs: Send + Sync {
    /// Выбранные пути; пусто — отмена (`canceled` или пустой `filePaths`).
    fn show_open_dialog(
        &self,
        options: OpenDialog,
    ) -> crate::providers::BoxFuture<'_, Vec<PathBuf>>;

    /// Выбранный путь; `None` — отмена.
    fn show_save_dialog(
        &self,
        options: SaveDialog,
    ) -> crate::providers::BoxFuture<'_, Option<PathBuf>>;
}

/// Без окна (фасад платформы по умолчанию, тесты): любой диалог отменён. Диалоги лаунчера —
/// `AppDialogs` приложения.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoDialogs;

impl Dialogs for NoDialogs {
    fn show_open_dialog(
        &self,
        _options: OpenDialog,
    ) -> crate::providers::BoxFuture<'_, Vec<PathBuf>> {
        Box::pin(async { Vec::new() })
    }

    fn show_save_dialog(
        &self,
        _options: SaveDialog,
    ) -> crate::providers::BoxFuture<'_, Option<PathBuf>> {
        Box::pin(async { None })
    }
}

/// Ярлык Windows (`shell.writeShortcutLink` / `readShortcutLink`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShortcutLink {
    pub target: String,
    pub args: String,
    pub description: String,
    pub cwd: String,
    pub icon: String,
    pub icon_index: i32,
}

/// Оболочка ОС (`electron.shell`): открыть папку или ссылку, показать файл, ярлыки Windows.
pub trait Shell: Send + Sync {
    /// `shell.openPath(path)`: пустая строка — открыто, иначе текст ошибки.
    fn open_path<'a>(&'a self, path: &'a Path) -> crate::providers::BoxFuture<'a, String>;

    /// `shell.openExternal(url)`: `Err` с текстом исключения — браузер не открылся (отклонённый
    /// промис).
    fn open_external(&self, url: &str) -> Result<(), String>;

    /// `shell.showItemInFolder(path)`.
    fn show_item_in_folder(&self, path: &Path);

    /// `shell.writeShortcutLink(path, "create", link)`: `false` — не создан.
    fn write_shortcut_link(&self, path: &Path, link: &ShortcutLink) -> bool;

    /// `shell.readShortcutLink(path)`; ошибка — текст исключения.
    fn read_shortcut_link(&self, path: &Path) -> Result<ShortcutLink, String>;
}

/// Без оболочки ОС (фасад платформы по умолчанию, тесты, Linux-ветка ярлыков приложения):
/// открыть ничего нельзя, ярлык `.lnk` не создаётся и не читается. Оболочка лаунчера — `AppShell`
/// приложения (в Windows ярлыки через `hlsr_update::windows::shortcuts::SystemShellLinks`).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoShell;

impl Shell for NoShell {
    fn open_path<'a>(&'a self, _path: &'a Path) -> crate::providers::BoxFuture<'a, String> {
        Box::pin(async { "оболочка ОС недоступна".to_owned() })
    }

    fn open_external(&self, _url: &str) -> Result<(), String> {
        Ok(())
    }

    fn show_item_in_folder(&self, _path: &Path) {}

    fn write_shortcut_link(&self, _path: &Path, _link: &ShortcutLink) -> bool {
        false
    }

    fn read_shortcut_link(&self, _path: &Path) -> Result<ShortcutLink, String> {
        Err("оболочка ОС недоступна".to_owned())
    }
}

/// Настройки оверлея LiveSplit в папках игр (`LiveSplitOverlay` 0.4.7, подпроект 3).
pub trait OverlayConfig: Send + Sync {
    /// `writeConfig(gameId, settings)`: `overlay.ini` игры, если её папка оверлея уже есть.
    fn write_config(&self, game_id: &str, settings: &Value) -> Result<bool, String>;

    /// `bindTo(gameId, settingsOf)`: LiveSplit запущен для игры (или закрыт — `None`);
    /// `true` — привязка сменилась.
    fn bind_to(&self, game_id: Option<&str>, settings_of: &dyn Fn(&str) -> Option<Value>) -> bool;
}
