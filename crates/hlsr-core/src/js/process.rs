//! `process.kill(pid, 0)` и `os.hostname()` Node 24: «есть ли процесс» по сигналу 0 и имя машины.
//! Так 0.4.7 решает, чья блокировка установки (`install.lock`) ещё действует.

use super::errno;
use std::borrow::Cow;

/// `ToInt32` JS (`x | 0`): не конечное — 0, иначе целая часть по модулю 2³² со знаком.
fn to_int32(x: f64) -> i32 {
    if !x.is_finite() {
        return 0;
    }
    let modulo = x.trunc().rem_euclid(4_294_967_296.0);
    // Остаток — целое от 0 до 2³² − 1: в u32 он помещается точно.
    modulo as u32 as i32
}

/// `process.kill(pid, 0)`: `pid` — число JS (`Number(значение)`). `Ok` — сигнал дошёл (процесс
/// есть); `Err` — код ошибки Node: `ESRCH`, `EPERM`… или `ERR_INVALID_ARG_TYPE`, если `pid` не
/// равен своему `pid | 0` (дробное, `NaN`, вне 32 бит). `0` и отрицательные — как у `kill(2)`:
/// своя группа процессов и группы.
pub fn kill_zero(pid: f64) -> Result<(), Cow<'static, str>> {
    let int = to_int32(pid);
    if pid != f64::from(int) {
        return Err(Cow::Borrowed("ERR_INVALID_ARG_TYPE"));
    }
    uv_kill_zero(int).map_err(|error| match error {
        KillError::NoProcess => Cow::Borrowed("ESRCH"),
        KillError::Os(error) => errno::code(&error).unwrap_or(Cow::Borrowed("UNKNOWN")),
    })
}

/// Отказ `uv_kill`: libuv в Windows отвечает `UV_ESRCH` сам, без перевода кода системы
/// (ERROR_INVALID_PARAMETER иначе стал бы `EINVAL`).
enum KillError {
    NoProcess,
    Os(std::io::Error),
}

/// `uv_kill(pid, 0)` в Linux: `kill(2)` с сигналом 0.
#[cfg(unix)]
fn uv_kill_zero(pid: i32) -> Result<(), KillError> {
    use rustix::process::{
        Pid, test_kill_current_process_group, test_kill_process, test_kill_process_group,
    };
    let result = match pid {
        0 => test_kill_current_process_group(),
        1.. => test_kill_process(Pid::from_raw(pid).expect("pid > 0")),
        // kill(-n, 0) — группа n (kill(-1, 0) — все процессы, куда можно послать сигнал);
        // у i32::MIN нет пары со знаком плюс — ядро отвечает ESRCH.
        _ => match pid.checked_neg().and_then(Pid::from_raw) {
            Some(group) => test_kill_process_group(group),
            None => return Err(KillError::NoProcess),
        },
    };
    result.map_err(|errno| KillError::Os(std::io::Error::from_raw_os_error(errno.raw_os_error())))
}

/// `uv_kill(pid, 0)` в Windows (libuv): `OpenProcess` (`0` — сам процесс), затем
/// `GetExitCodeProcess`; процесс завершён или `OpenProcess` отвечает ERROR_INVALID_PARAMETER —
/// `ESRCH`; прочие ошибки — как перевела бы libuv (ERROR_ACCESS_DENIED — `EPERM`).
#[cfg(windows)]
fn uv_kill_zero(pid: i32) -> Result<(), KillError> {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, OpenProcess, PROCESS_QUERY_INFORMATION,
        PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    };
    let os = |error: u32| KillError::Os(std::io::Error::from_raw_os_error(error as i32));
    let own = pid == 0;
    // SAFETY: OpenProcess и GetCurrentProcess без указателей; ручка закрывается ниже.
    let handle = unsafe {
        if own {
            GetCurrentProcess()
        } else {
            OpenProcess(
                PROCESS_TERMINATE | PROCESS_QUERY_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid as u32,
            )
        }
    };
    if handle.is_null() {
        // SAFETY: последняя ошибка этого потока.
        let error = unsafe { GetLastError() };
        // uv_kill: процесса с таким pid нет — UV_ESRCH.
        return Err(if error == ERROR_INVALID_PARAMETER {
            KillError::NoProcess
        } else {
            os(error)
        });
    }
    let mut status = 0u32;
    // SAFETY: ручка открыта выше, `status` — локальная переменная.
    let ok = unsafe { GetExitCodeProcess(handle, &mut status) };
    // SAFETY: последняя ошибка этого потока — до CloseHandle.
    let error = unsafe { GetLastError() };
    if !own {
        // SAFETY: ручка от OpenProcess, закрывается один раз.
        unsafe { CloseHandle(handle) };
    }
    if ok == 0 {
        return Err(os(error));
    }
    if status != STILL_ACTIVE as u32 {
        // uv__kill: процесс завершён — UV_ESRCH.
        return Err(KillError::NoProcess);
    }
    Ok(())
}

/// `os.hostname()`: имя машины (`uv_os_gethostname`). Linux — `nodename` из `uname`;
/// Windows — DNS-имя компьютера.
#[cfg(unix)]
pub fn hostname() -> String {
    rustix::system::uname()
        .nodename()
        .to_string_lossy()
        .into_owned()
}

/// `os.hostname()`: имя машины (`uv_os_gethostname`). Linux — `nodename` из `uname`;
/// Windows — DNS-имя компьютера.
#[cfg(windows)]
pub fn hostname() -> String {
    use windows_sys::Win32::System::SystemInformation::{
        ComputerNameDnsHostname, GetComputerNameExW,
    };
    let mut size = 0u32;
    // SAFETY: первый вызов с пустым буфером узнаёт размер.
    unsafe { GetComputerNameExW(ComputerNameDnsHostname, std::ptr::null_mut(), &mut size) };
    let mut buffer = vec![0u16; size as usize + 1];
    let mut length = buffer.len() as u32;
    // SAFETY: буфер на `length` единиц UTF-16.
    let ok =
        unsafe { GetComputerNameExW(ComputerNameDnsHostname, buffer.as_mut_ptr(), &mut length) };
    if ok == 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..length as usize])
}

#[cfg(test)]
mod tests {
    use super::to_int32;

    #[test]
    fn to_int32_wraps_like_js() {
        assert_eq!(to_int32(1.5), 1);
        assert_eq!(to_int32(-1.5), -1);
        assert_eq!(to_int32(4_294_967_297.0), 1);
        assert_eq!(to_int32(2_147_483_648.0), i32::MIN);
        assert_eq!(to_int32(f64::NAN), 0);
        assert_eq!(to_int32(f64::INFINITY), 0);
        assert_eq!(to_int32(-0.0), 0);
    }
}
