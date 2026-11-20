//! Windows: доступ к памяти чужого процесса для патча курсора RInput (`rinput-menu-fix-win.js`).
//!
//! 0.4.7 поднимал PowerShell с обёрткой на C# над `ReadProcessMemory`/`WriteProcessMemory`/
//! `VirtualProtectEx`, потому что Node не умеет читать чужую память. В Rust (решение 2) эти вызовы
//! делаются напрямую. `hl.exe` 32-битный, модули ищем снимком `TH32CS_SNAPMODULE | SNAPMODULE32` и
//! оставляем только базы ниже 4 ГиБ — такие, как видит 32-битный процесс.

#![cfg(windows)]

use crate::rinput_menu_fix::{self, MenuSession, PatchStatus, ProcessMemory};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_BAD_LENGTH, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    FlushInstructionCache, ReadProcessMemory, WriteProcessMemory,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, PROCESSENTRY32W,
    Process32FirstW, Process32NextW, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtectEx};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
};

/// Снимок с обоими флагами модулей: 64-битные (`SNAPMODULE`) и 32-битные в WOW64 (`SNAPMODULE32`).
const SNAP_MODULES: u32 = TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32;

/// Дескриптор, закрывается при выходе из области видимости.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: дескриптор получен от OpenProcess/CreateToolhelp32Snapshot и закрывается один раз.
        unsafe { CloseHandle(self.0) };
    }
}

fn utf16_name(units: &[u16]) -> String {
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// Имя образа без `.exe` (`/\.exe$/i`; `Process.GetProcessesByName` сравнивает его без учёта
/// регистра).
fn image_stem(name: &str) -> &str {
    let cut = name.len().saturating_sub(4);
    match name.get(cut..) {
        Some(tail) if tail.eq_ignore_ascii_case(".exe") => &name[..cut],
        _ => name,
    }
}

/// PID процессов с именем образа `name` (без `.exe`), как `Process.GetProcessesByName(name)`.
pub fn process_ids(name: &str) -> Result<Vec<u32>, String> {
    let stem = image_stem(name);
    // SAFETY: без указателей; дескриптор закрывается в Drop.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(format!(
            "CreateToolhelp32Snapshot: {}",
            std::io::Error::last_os_error()
        ));
    }
    let handle = Handle(raw);
    // SAFETY: структура из нулей допустима, dwSize задаётся ниже.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut found = Vec::new();
    // SAFETY: дескриптор открыт, `entry` — локальная структура с заданным dwSize.
    let mut more = unsafe { Process32FirstW(handle.0, &mut entry) };
    while more != 0 {
        if image_stem(&utf16_name(&entry.szExeFile)).eq_ignore_ascii_case(stem) {
            found.push(entry.th32ProcessID);
        }
        // SAFETY: как выше.
        more = unsafe { Process32NextW(handle.0, &mut entry) };
    }
    Ok(found)
}

/// Все модули процесса: (имя, база). `ERROR_BAD_LENGTH` — модули как раз меняются, снимок повторяем.
pub fn snapshot_modules(pid: u32) -> Result<Vec<(String, u64)>, String> {
    for _ in 0..5 {
        // SAFETY: без указателей; дескриптор закрывается в Drop.
        let raw = unsafe { CreateToolhelp32Snapshot(SNAP_MODULES, pid) };
        if raw == INVALID_HANDLE_VALUE {
            // SAFETY: последняя ошибка этого потока.
            let error = unsafe { GetLastError() };
            if error == ERROR_BAD_LENGTH {
                continue;
            }
            return Err(format!("CreateToolhelp32Snapshot: код {error}"));
        }
        let handle = Handle(raw);
        // SAFETY: структура из нулей допустима, dwSize задаётся ниже.
        let mut entry: MODULEENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
        let mut modules = Vec::new();
        // SAFETY: дескриптор открыт, `entry` — локальная структура с заданным dwSize.
        let mut more = unsafe { Module32FirstW(handle.0, &mut entry) };
        while more != 0 {
            modules.push((utf16_name(&entry.szModule), entry.modBaseAddr as u64));
            // SAFETY: как выше.
            more = unsafe { Module32NextW(handle.0, &mut entry) };
        }
        return Ok(modules);
    }
    Ok(Vec::new())
}

/// База модуля `name` среди снимка: ниже 4 ГиБ (32-битный вид) и совпадает по имени без учёта
/// регистра; `None` — нет такого.
pub fn module_base_in(modules: &[(String, u64)], name: &str) -> Option<u64> {
    modules
        .iter()
        .find(|(module, base)| *base <= 0xffff_ffff && module.eq_ignore_ascii_case(name))
        .map(|(_, base)| *base)
}

/// Память процесса по его дескриптору.
pub struct WinProcessMemory {
    pid: u32,
    handle: Handle,
}

impl WinProcessMemory {
    /// Открывает процесс с правами на чтение, запись и изменение защиты страниц.
    pub fn open(pid: u32) -> Result<WinProcessMemory, String> {
        let access =
            PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_QUERY_INFORMATION;
        // SAFETY: без указателей; дескриптор закрывается в Drop.
        let raw = unsafe { OpenProcess(access, 0, pid) };
        if raw.is_null() {
            // SAFETY: последняя ошибка этого потока.
            let error = unsafe { GetLastError() };
            return Err(format!("OpenProcess {error}"));
        }
        Ok(WinProcessMemory {
            pid,
            handle: Handle(raw),
        })
    }
}

impl ProcessMemory for WinProcessMemory {
    fn module_base(&mut self, name: &str) -> Result<Option<u64>, String> {
        Ok(module_base_in(&snapshot_modules(self.pid)?, name))
    }

    fn read(&mut self, address: u64, length: usize) -> Result<Vec<u8>, String> {
        let mut buffer = vec![0u8; length];
        let mut done: usize = 0;
        // SAFETY: буфер вмещает `length` байт; дескриптор открыт.
        let ok = unsafe {
            ReadProcessMemory(
                self.handle.0,
                address as *const core::ffi::c_void,
                buffer.as_mut_ptr().cast::<core::ffi::c_void>(),
                length,
                &mut done,
            )
        };
        if ok == 0 {
            // SAFETY: последняя ошибка этого потока.
            return Err(format!("read {}", unsafe { GetLastError() }));
        }
        Ok(buffer)
    }

    fn write(&mut self, address: u64, buf: &[u8]) -> Result<(), String> {
        let address_ptr = address as *const core::ffi::c_void;
        let mut old: u32 = 0;
        // SAFETY: адрес и размер задают область в чужом процессе; `old` — локальная переменная.
        let unprotected = unsafe {
            VirtualProtectEx(
                self.handle.0,
                address_ptr,
                buf.len(),
                PAGE_EXECUTE_READWRITE,
                &mut old,
            )
        };
        let mut done: usize = 0;
        // SAFETY: буфер `buf` живёт до конца вызова; дескриптор открыт.
        let ok = unsafe {
            WriteProcessMemory(
                self.handle.0,
                address_ptr,
                buf.as_ptr().cast::<core::ffi::c_void>(),
                buf.len(),
                &mut done,
            )
        };
        // SAFETY: последняя ошибка — до восстановления защиты.
        let error = unsafe { GetLastError() };
        if unprotected != 0 {
            let mut ignored: u32 = 0;
            // SAFETY: возвращаем прежнюю защиту области.
            unsafe { VirtualProtectEx(self.handle.0, address_ptr, buf.len(), old, &mut ignored) };
        }
        // SAFETY: сбрасываем кеш инструкций после записи кода.
        unsafe { FlushInstructionCache(self.handle.0, address_ptr, buf.len()) };
        if ok == 0 {
            return Err(format!("write {error}"));
        }
        Ok(())
    }
}

/// Патч курсора RInput во всех процессах игры с именем `process_name` (`openSession().patch`).
/// Процесс, который не открылся (`OpenProcess`), или сбой чтения — ошибка всего вызова, как
/// отказ запроса к помощнику в 0.4.7.
pub fn patch_process_name(process_name: &str) -> Result<PatchStatus, String> {
    let mut result = PatchStatus::Waiting;
    for pid in process_ids(process_name)? {
        let mut memory = WinProcessMemory::open(pid)?;
        result = rinput_menu_fix::merge_status(result, rinput_menu_fix::patch_memory(&mut memory)?);
    }
    Ok(result)
}

/// `openSession()`: в 0.4.7 держала процесс PowerShell между опросами; здесь вызовы прямые, и
/// держать нечего — `close` ничего не делает.
#[derive(Debug, Default)]
pub struct Session;

/// `openSession()`.
pub fn open_session() -> Session {
    Session
}

impl MenuSession for Session {
    fn patch(&mut self, process_name: &str) -> Result<PatchStatus, String> {
        patch_process_name(process_name)
    }

    fn close(&mut self) {}
}
