//! Перезапуск лаунчера от администратора (`game-launch-restart-elevated`,
//! `game-delivery/index.js:935-1000`): замок единственного экземпляра отпускается, exe лаунчера
//! запускается с `runas` (UAC), лаунчер выходит через 600 мс; отказ — замок берётся обратно.
//! В 0.4.7 запуск шёл через PowerShell `Start-Process -Verb RunAs`, здесь — `ShellExecuteW`
//! (спецификация, решение 2).

use hlsr_delivery::providers::BoxFuture;
use std::time::Duration;

/// `ELEVATION_WAIT_MS`: сколько ждать ответа UAC, прежде чем ответить окну `pending`.
pub const ELEVATION_WAIT: Duration = Duration::from_secs(3 * 60);
/// `ELEVATION_QUIT_DELAY_MS`.
pub const ELEVATION_QUIT_DELAY: Duration = Duration::from_millis(600);

/// Чем кончился запрос прав.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Elevated {
    /// Лаунчер от администратора запущен (`exit` с кодом 0).
    Started,
    /// UAC отклонён или запуск не удался (`exit` с ненулевым кодом): код для журнала.
    Declined(i64),
}

/// Запуск exe лаунчера от администратора. `Err` — сам запуск не начался (`spawn` и `error`
/// PowerShell в 0.4.7).
pub trait Elevator: Send + Sync {
    fn run_as_admin(&self) -> BoxFuture<'static, Result<Elevated, String>>;
}

/// Замок единственного экземпляра (`app.releaseSingleInstanceLock`/`requestSingleInstanceLock`).
pub trait InstanceLock: Send + Sync {
    fn release(&self) -> Result<(), String>;
    fn reacquire(&self);
}

/// Настоящий `ShellExecuteW(NULL, "runas", <exe лаунчера>, …)`: ждёт ответа UAC в потоке
/// блокирующих задач.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemElevator;

#[cfg(windows)]
impl Elevator for SystemElevator {
    fn run_as_admin(&self) -> BoxFuture<'static, Result<Elevated, String>> {
        Box::pin(async {
            let exe = std::env::current_exe().map_err(|error| error.to_string())?;
            tokio::task::spawn_blocking(move || shell_execute_runas(&exe))
                .await
                .map_err(|error| error.to_string())?
        })
    }
}

#[cfg(windows)]
fn shell_execute_runas(exe: &std::path::Path) -> Result<Elevated, String> {
    use hlsr_core::win32::wide;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    const SW_SHOWNORMAL: i32 = 1;

    let (verb, file) = (wide("runas"), wide(exe));
    // SAFETY: строки с нулём в конце живут до конца вызова; окна-владельца нет.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as usize > 32 {
        return Ok(Elevated::Started);
    }
    // SAFETY: без аргументов; читает код ошибки этого потока.
    let error = unsafe { GetLastError() };
    // Отказ в UAC — ERROR_CANCELLED (1223); иначе — код ShellExecuteW.
    let code = if error != 0 {
        i64::from(error)
    } else {
        result as i64
    };
    Ok(Elevated::Declined(code))
}

#[cfg(not(windows))]
impl Elevator for SystemElevator {
    fn run_as_admin(&self) -> BoxFuture<'static, Result<Elevated, String>> {
        Box::pin(async {
            Err("перезапуск от администратора есть только в Windows".to_owned())
        })
    }
}
