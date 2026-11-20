//! Процессы по PID и по имени образа (`process-inspect.js`): путь исполняемого файла, имя, время
//! старта; «жив ли процесс».
//!
//! Windows: 0.4.7 спрашивает PowerShell (WMI, затем `Get-Process`) и, если он не ответил,
//! `tasklist`. Здесь вместо PowerShell — прямой WinAPI (Toolhelp32, `QueryFullProcessImageNameW`,
//! `GetProcessTimes`, решение 2 спецификации): те же поля, те же единицы. Цепочка прежняя —
//! система, а если она не ответила, `tasklist` (только имя образа, без пути).
//! Linux: путь и имя из `/proc`, без внешних команд.

use hlsr_core::js::path::NodePath;
use hlsr_core::js::process::kill_zero;
use hlsr_core::js::{self, Platform, date_parse};
use hlsr_delivery::environment::Environment;
use serde_json::{Map, Value};
use std::sync::{Arc, Mutex};

/// `START_TOLERANCE_MS`: насколько могут разойтись время старта в записи и у процесса.
pub const START_TOLERANCE_MS: i64 = 5000;

/// Источник данных, когда система о процессе ответила (в 0.4.7 — `cim` или `process`).
pub const SOURCE_SYSTEM: &str = "winapi";
/// Источник данных, когда ответил только `tasklist` (имя образа, без пути).
pub const SOURCE_TASKLIST: &str = "tasklist";

/// Процесс: `{pid, executablePath, name, startedAt, source}`. Путь и время старта бывают не
/// известны (процесс с другими правами, `tasklist`, Linux — время старта).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub executable_path: Option<String>,
    pub name: Option<String>,
    /// `new Date().toISOString()` момента старта.
    pub started_at: Option<String>,
    /// Откуда сведения; на Linux (`inspectPosix`) поля нет.
    pub source: Option<String>,
}

impl ProcessInfo {
    /// Объект JS: `{pid, executablePath, name, startedAt}` и `source`, если он есть.
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("pid".into(), Value::from(self.pid));
        object.insert("executablePath".into(), self.executable_path.clone().into());
        object.insert("name".into(), self.name.clone().into());
        object.insert("startedAt".into(), self.started_at.clone().into());
        if let Some(source) = &self.source {
            object.insert("source".into(), source.clone().into());
        }
        Value::Object(object)
    }
}

/// `isAlive(pid)`: `process.kill(pid, 0)`; нет прав (`EPERM`) — процесс есть.
pub fn is_alive_number(pid: f64) -> bool {
    if !(pid.is_finite() && pid.fract() == 0.0 && pid > 0.0) {
        return false;
    }
    match kill_zero(pid) {
        Ok(()) => true,
        Err(code) => code == "EPERM",
    }
}

/// `isAlive(pid)` для PID процесса.
pub fn is_alive(pid: u32) -> bool {
    is_alive_number(f64::from(pid))
}

/// Короткий текст ошибки для журнала (`shortError`): пробелы схлопнуты, не длиннее 200 единиц.
pub fn short_error(text: &str) -> String {
    let mut collapsed = String::new();
    let mut in_space = false;
    for c in text.chars() {
        if js::is_whitespace(c) {
            in_space = true;
            continue;
        }
        if in_space && !collapsed.is_empty() {
            collapsed.push(' ');
        }
        in_space = false;
        collapsed.push(c);
    }
    if collapsed.encode_utf16().count() > 200 {
        return format!("{}…", js::slice_utf16(&collapsed, 200));
    }
    collapsed
}

/// `toProcessList(data)`: ответ системы — список, объект `{value: [...]}` или один объект.
pub fn to_process_list(data: &Value) -> Vec<Value> {
    match data {
        Value::Array(items) => items.clone(),
        Value::Object(object) => match object.get("value") {
            Some(Value::Array(items)) => items.clone(),
            _ => vec![data.clone()],
        },
        _ => Vec::new(),
    }
}

/// `parseTasklist(output)`: `tasklist /FO CSV /NH` — `"steam.exe","7440","Console","1","123 456 K"`.
/// Строка «нет задач» не в кавычках и локализована — она просто пропускается.
pub fn parse_tasklist(output: &str) -> Vec<ProcessInfo> {
    let mut found = Vec::new();
    for line in output.split('\n') {
        let line = js::trim(line);
        let Some(rest) = line.strip_prefix('"') else {
            continue;
        };
        let Some(close) = rest.find('"') else {
            continue;
        };
        let name = &rest[..close];
        let Some(after) = rest[close + 1..].strip_prefix(",\"") else {
            continue;
        };
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || !after[digits..].starts_with('"') {
            continue;
        }
        // Число не из u32 для PID бессмысленно: такая строка пропускается.
        let Ok(pid) = after[..digits].parse::<u32>() else {
            continue;
        };
        if pid > 0 {
            found.push(ProcessInfo {
                pid,
                executable_path: None,
                name: Some(name.to_owned()),
                started_at: None,
                source: Some(SOURCE_TASKLIST.to_owned()),
            });
        }
    }
    found
}

/// `IMAGE_NAME`: `/^[A-Za-z0-9._-]{1,64}$/`.
pub fn is_image_name(image_name: &str) -> bool {
    (1..=64).contains(&image_name.len())
        && image_name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Правила `path` для ОС модуля; текущая — с рабочей папкой процесса.
pub(crate) fn node_path(platform: Platform) -> NodePath {
    if platform == Platform::CURRENT {
        NodePath::native()
    } else if platform == Platform::Win32 {
        NodePath::new(Platform::Win32, "C:\\")
    } else {
        NodePath::new(Platform::Linux, "/")
    }
}

/// `samePath(left, right)` по правилам `platform`: Windows — без учёта регистра.
pub fn same_path_on(platform: Platform, left: Option<&str>, right: Option<&str>) -> bool {
    let (Some(left), Some(right)) = (
        left.filter(|p| !p.is_empty()),
        right.filter(|p| !p.is_empty()),
    ) else {
        return false;
    };
    let path = node_path(platform);
    let (left, right) = (path.resolve(&[left]), path.resolve(&[right]));
    match platform {
        Platform::Linux => left == right,
        Platform::Win32 => left.to_lowercase() == right.to_lowercase(),
    }
}

/// `samePath(left, right)` на этой ОС.
pub fn same_path(left: Option<&str>, right: Option<&str>) -> bool {
    same_path_on(Platform::CURRENT, left, right)
}

/// `sameStart(expected, actual)`: время старта в записи и у процесса различаются не больше,
/// чем на [`START_TOLERANCE_MS`]. Без обоих времён старта PID не опознать: его мог получить чужой
/// процесс (0.4.9, A-6) — не совпадает.
pub fn same_start(expected: Option<&str>, actual: Option<&str>) -> bool {
    match (expected.and_then(date_parse), actual.and_then(date_parse)) {
        (Some(a), Some(b)) => (a - b).abs() <= START_TOLERANCE_MS,
        _ => false,
    }
}

/// То, что `GameProcessService` и `SteamLocator` берут у модуля `process-inspect`.
pub trait ProcessInspector: Send + Sync {
    /// `isAlive(pid)`.
    fn is_alive(&self, pid: u32) -> bool {
        is_alive(pid)
    }

    /// `inspect(pid)`: сведения о процессе или `None` — его нет.
    fn inspect(&self, pid: u32) -> Option<ProcessInfo>;

    /// `findByImageName(imageName)`: процессы с таким образом; имя не из `[A-Za-z0-9._-]{1,64}` —
    /// пусто.
    fn find_by_image_name(&self, _image_name: &str) -> Vec<ProcessInfo> {
        Vec::new()
    }

    /// `lastFailure()`: почему последний запрос к системе не дал данных (Windows) — для журнала.
    fn last_failure(&self) -> Option<String> {
        None
    }

    /// `samePath(left, right)`.
    fn same_path(&self, left: Option<&str>, right: Option<&str>) -> bool {
        same_path(left, right)
    }

    /// `sameStart(expected, actual)`.
    fn same_start(&self, expected: Option<&str>, actual: Option<&str>) -> bool {
        same_start(expected, actual)
    }
}

// ------------------------------------------------------------------- Windows

/// Что в 0.4.7 делает PowerShell (WMI и `Get-Process`): сведения о процессах от системы. `Err` —
/// система не ответила (тогда спрашивается `tasklist`), `Ok(None)` — такого процесса нет.
pub trait SystemProcesses: Send + Sync {
    fn inspect(&self, pid: u32) -> Result<Option<ProcessInfo>, String>;
    fn find_by_image_name(&self, image_name: &str) -> Result<Vec<ProcessInfo>, String>;
}

/// `tasklist.exe /FO CSV /NH /FI <фильтр>`: вывод программы или текст ошибки.
pub trait Tasklist: Send + Sync {
    fn run(&self, filter: &str) -> Result<String, String>;
}

/// `createWindowsInspector`: система, а если она не ответила, `tasklist`.
pub struct WindowsInspector {
    system: Arc<dyn SystemProcesses>,
    tasklist: Arc<dyn Tasklist>,
    last_failure: Mutex<Option<String>>,
}

impl WindowsInspector {
    pub fn new(system: Arc<dyn SystemProcesses>, tasklist: Arc<dyn Tasklist>) -> WindowsInspector {
        WindowsInspector {
            system,
            tasklist,
            last_failure: Mutex::new(None),
        }
    }

    /// Инспектор этой ОС: WinAPI и `tasklist.exe` из `System32`.
    #[cfg(windows)]
    pub fn system() -> WindowsInspector {
        WindowsInspector::new(
            Arc::new(winapi::WinApiProcesses),
            Arc::new(SystemTasklist::new(&Environment::process())),
        )
    }

    fn set_failure(&self, text: String) {
        *self
            .last_failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(text);
    }

    /// `runTasklist(filter)`: сбой дописывается к прежней причине.
    fn run_tasklist(&self, filter: &str) -> Vec<ProcessInfo> {
        match self.tasklist.run(filter) {
            Ok(output) => parse_tasklist(&output),
            Err(error) => {
                let mut guard = self
                    .last_failure
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let prefix = guard
                    .as_ref()
                    .map(|failure| format!("{failure}; "))
                    .unwrap_or_default();
                *guard = Some(format!("{prefix}tasklist: {}", short_error(&error)));
                Vec::new()
            }
        }
    }
}

impl ProcessInspector for WindowsInspector {
    fn inspect(&self, pid: u32) -> Option<ProcessInfo> {
        if pid == 0 {
            return None;
        }
        match self.system.inspect(pid) {
            Ok(found) => found.filter(|item| item.pid != 0),
            Err(error) => {
                self.set_failure(format!("winapi: {}", short_error(&error)));
                self.run_tasklist(&format!("PID eq {pid}"))
                    .into_iter()
                    .find(|item| item.pid == pid)
            }
        }
    }

    fn find_by_image_name(&self, image_name: &str) -> Vec<ProcessInfo> {
        if !is_image_name(image_name) {
            return Vec::new();
        }
        let list = match self.system.find_by_image_name(image_name) {
            Ok(list) => list,
            Err(error) => {
                self.set_failure(format!("winapi: {}", short_error(&error)));
                self.run_tasklist(&format!("IMAGENAME eq {image_name}"))
            }
        };
        list.into_iter().filter(|item| item.pid > 0).collect()
    }

    fn last_failure(&self) -> Option<String> {
        self.last_failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn same_path(&self, left: Option<&str>, right: Option<&str>) -> bool {
        same_path_on(Platform::Win32, left, right)
    }
}

/// `tasklist.exe`: `<SystemRoot>\System32\tasklist.exe`, если он есть, иначе по имени (`PATH`).
pub struct SystemTasklist {
    program: String,
}

impl SystemTasklist {
    pub fn new(env: &Environment) -> SystemTasklist {
        let full = NodePath::new(Platform::Win32, "C:\\").join(&[
            &crate::windows_tools::system_root(env),
            "System32",
            "tasklist.exe",
        ]);
        let program = if std::path::Path::new(&full).exists() {
            full
        } else {
            "tasklist.exe".to_owned()
        };
        SystemTasklist { program }
    }
}

impl Tasklist for SystemTasklist {
    fn run(&self, filter: &str) -> Result<String, String> {
        let mut command = std::process::Command::new(&self.program);
        command
            .args(["/FO", "CSV", "/NH", "/FI", filter])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        crate::process_stop::hide_window(&mut command);
        let output =
            crate::process_stop::output_with_timeout(command, std::time::Duration::from_secs(10))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            let text = String::from_utf8_lossy(&output.stderr);
            Err(format!("Command failed: tasklist.exe {text}"))
        }
    }
}

#[cfg(windows)]
pub mod winapi {
    //! Сведения о процессах прямыми вызовами WinAPI: список процессов (Toolhelp32), путь образа
    //! (`QueryFullProcessImageNameW`), время старта (`GetProcessTimes`). Процесс другого
    //! пользователя или с другими правами открыть нельзя (`ERROR_ACCESS_DENIED`): тогда известно
    //! только имя образа из списка, пути и времени старта нет — как у WMI для такого процесса.

    use super::{ProcessInfo, SOURCE_SYSTEM, SystemProcesses};
    use hlsr_core::js::Platform;
    use hlsr_core::js::iso::iso_from_ms;
    use hlsr_core::js::path::NodePath;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, FILETIME, GetLastError, HANDLE,
        INVALID_HANDLE_VALUE, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };

    /// Ручка процесса, закрывается при выходе из области видимости.
    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: ручка получена от OpenProcess или CreateToolhelp32Snapshot и закрывается
            // один раз.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Миллисекунды Unix от 100-наносекундных тиков FILETIME (с 1601-01-01).
    fn unix_ms(time: &FILETIME) -> i64 {
        let ticks = (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
        (ticks / 10_000) as i64 - 11_644_473_600_000
    }

    /// Записи списка процессов: PID и имя образа.
    fn snapshot() -> Result<Vec<(u32, String)>, String> {
        // SAFETY: без указателей; ручка закрывается ниже.
        let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if raw == INVALID_HANDLE_VALUE {
            return Err(format!(
                "CreateToolhelp32Snapshot: {}",
                std::io::Error::last_os_error()
            ));
        }
        let handle = Handle(raw);
        // SAFETY: структура из нулей — допустимое начальное значение, dwSize задаётся ниже.
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = Vec::new();
        // SAFETY: ручка списка открыта, `entry` — локальная структура с заданным dwSize.
        let mut more = unsafe { Process32FirstW(handle.0, &mut entry) };
        while more != 0 {
            let length = entry
                .szExeFile
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(entry.szExeFile.len());
            found.push((
                entry.th32ProcessID,
                String::from_utf16_lossy(&entry.szExeFile[..length]),
            ));
            // SAFETY: как выше.
            more = unsafe { Process32NextW(handle.0, &mut entry) };
        }
        Ok(found)
    }

    /// Путь образа открытого процесса.
    fn image_path(process: &Handle) -> Option<String> {
        let mut capacity = 1024u32;
        for _ in 0..6 {
            let mut buffer = vec![0u16; capacity as usize];
            let mut length = capacity;
            // SAFETY: буфер на `length` единиц UTF-16, ручка открыта.
            let ok = unsafe {
                QueryFullProcessImageNameW(process.0, 0, buffer.as_mut_ptr(), &mut length)
            };
            if ok != 0 {
                return Some(String::from_utf16_lossy(&buffer[..length as usize]));
            }
            capacity = capacity.saturating_mul(4);
        }
        None
    }

    /// Время старта открытого процесса, как `Date.prototype.toISOString`.
    fn started_at(process: &Handle) -> Option<String> {
        let empty = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (empty, empty, empty, empty);
        // SAFETY: ручка открыта, четыре указателя — на локальные структуры.
        let ok = unsafe {
            GetProcessTimes(process.0, &mut created, &mut exited, &mut kernel, &mut user)
        };
        (ok != 0).then(|| iso_from_ms(unix_ms(&created)))
    }

    /// `GetExitCodeProcess == STILL_ACTIVE`: завершённый процесс, на который кто-то держит
    /// ручку, из списка процессов уже исчез и WMI его не показывает.
    fn still_active(process: &Handle) -> bool {
        let mut status = 0u32;
        // SAFETY: ручка открыта, `status` — локальная переменная.
        let ok = unsafe { GetExitCodeProcess(process.0, &mut status) };
        ok == 0 || status == STILL_ACTIVE as u32
    }

    fn open(pid: u32) -> Result<Handle, u32> {
        // SAFETY: без указателей; ручка закрывается в Drop.
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if raw.is_null() {
            // SAFETY: последняя ошибка этого потока.
            return Err(unsafe { GetLastError() });
        }
        Ok(Handle(raw))
    }

    fn file_name(path: &str) -> String {
        NodePath::new(Platform::Win32, "C:\\").basename(path)
    }

    /// Сведения о процессе `pid`, `name_hint` — имя образа из списка процессов, если уже известно.
    fn describe(pid: u32, name_hint: Option<String>) -> Result<Option<ProcessInfo>, String> {
        match open(pid) {
            Ok(process) => {
                if !still_active(&process) {
                    return Ok(None);
                }
                let path = image_path(&process);
                let name = path
                    .as_deref()
                    .map(file_name)
                    .filter(|name| !name.is_empty())
                    .or(name_hint);
                Ok(Some(ProcessInfo {
                    pid,
                    executable_path: path,
                    name,
                    started_at: started_at(&process),
                    source: Some(SOURCE_SYSTEM.to_owned()),
                }))
            }
            // Процесса нет.
            Err(code) if code == ERROR_INVALID_PARAMETER => Ok(None),
            // Процесс есть, но открыть его нельзя: известно только имя образа.
            Err(code) if code == ERROR_ACCESS_DENIED => {
                let name = match name_hint {
                    Some(name) => Some(name),
                    None => snapshot()?
                        .into_iter()
                        .find(|(listed, _)| *listed == pid)
                        .map(|(_, name)| name),
                };
                Ok(name.map(|name| ProcessInfo {
                    pid,
                    executable_path: None,
                    name: Some(name),
                    started_at: None,
                    source: Some(SOURCE_SYSTEM.to_owned()),
                }))
            }
            Err(code) => Err(format!("OpenProcess({pid}): код ошибки {code}")),
        }
    }

    /// Система как источник сведений о процессах.
    pub struct WinApiProcesses;

    impl SystemProcesses for WinApiProcesses {
        fn inspect(&self, pid: u32) -> Result<Option<ProcessInfo>, String> {
            describe(pid, None)
        }

        fn find_by_image_name(&self, image_name: &str) -> Result<Vec<ProcessInfo>, String> {
            let wanted = image_name.to_lowercase();
            let mut found = Vec::new();
            for (pid, name) in snapshot()? {
                if name.to_lowercase() != wanted {
                    continue;
                }
                if let Some(item) = describe(pid, Some(name))? {
                    found.push(item);
                }
            }
            Ok(found)
        }
    }
}

// ------------------------------------------------------------------- Linux

/// Linux: путь и имя процесса читаются из `/proc` без запуска внешних команд (`inspectPosix`);
/// времени старта нет.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PosixInspector;

#[cfg(unix)]
impl PosixInspector {
    fn inspect_pid(pid: u32) -> Option<ProcessInfo> {
        if !is_alive(pid) {
            return None;
        }
        let root = std::path::PathBuf::from(format!("/proc/{pid}"));
        let executable_path = std::fs::read_link(root.join("exe")).ok()?;
        let comm = std::fs::read(root.join("comm")).ok()?;
        Some(ProcessInfo {
            pid,
            executable_path: Some(executable_path.to_string_lossy().into_owned()),
            name: Some(js::trim(&js::utf8_string(&comm)).to_owned()),
            started_at: None,
            source: None,
        })
    }
}

#[cfg(unix)]
impl ProcessInspector for PosixInspector {
    fn inspect(&self, pid: u32) -> Option<ProcessInfo> {
        if pid == 0 {
            return None;
        }
        Self::inspect_pid(pid)
    }

    fn find_by_image_name(&self, image_name: &str) -> Vec<ProcessInfo> {
        if !is_image_name(image_name) {
            return Vec::new();
        }
        let Ok(names) = hlsr_core::js::fs::read_dir_names(std::path::Path::new("/proc")) else {
            return Vec::new();
        };
        names
            .iter()
            .filter(|name| !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()))
            .filter_map(|name| name.parse::<u32>().ok())
            .filter_map(Self::inspect_pid)
            .filter(|item| item.name.as_deref() == Some(image_name))
            .collect()
    }
}

/// Инспектор этой ОС.
pub fn system_inspector() -> Arc<dyn ProcessInspector> {
    #[cfg(windows)]
    {
        Arc::new(WindowsInspector::system())
    }
    #[cfg(unix)]
    {
        Arc::new(PosixInspector)
    }
}
