//! Остановка процессов (`process-stop.js`). Windows: `taskkill /PID <pid> /T [/F]` — закрывает
//! и дерево процессов (инжектор → `hl.exe`). Linux: `SIGTERM`/`SIGKILL`, в том числе всей группе
//! процессов (`-pid`): Proton запускает игру в своей группе.

use hlsr_core::js::errno;
use hlsr_delivery::providers::BoxFuture;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Итог попытки остановки: `{ok, code, output}`. `code` — код ошибки или статус выхода, `None`
/// при успехе.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopResult {
    pub ok: bool,
    pub code: Option<String>,
    pub output: String,
}

impl StopResult {
    pub fn success(output: impl Into<String>) -> StopResult {
        StopResult {
            ok: true,
            code: None,
            output: output.into(),
        }
    }

    pub fn failure(code: impl Into<String>, output: impl Into<String>) -> StopResult {
        StopResult {
            ok: false,
            code: Some(code.into()),
            output: output.into(),
        }
    }
}

/// `process-stop`: мягкая и принудительная остановка процесса и его группы.
pub trait Stopper: Send + Sync {
    /// `graceful(pid)`: Windows — `taskkill /T`, Linux — `SIGTERM`.
    fn graceful(&self, pid: u32) -> BoxFuture<'_, StopResult>;

    /// `force(pid)`: Windows — `taskkill /T /F`, Linux — `SIGKILL`.
    fn force(&self, pid: u32) -> BoxFuture<'_, StopResult>;

    /// `gracefulGroup(pid)`: Linux — `SIGTERM` группе, Windows — как `graceful`.
    fn graceful_group(&self, pid: u32) -> BoxFuture<'_, StopResult>;

    /// `forceGroup(pid)`: Linux — `SIGKILL` группе, Windows — как `force`.
    fn force_group(&self, pid: u32) -> BoxFuture<'_, StopResult>;
}

/// Остановка средствами ОС.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemStopper;

/// Окно консоли программы не показывается (`windowsHide: true`).
pub(crate) fn hide_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

/// `execFileSync(…, {timeout})`: вывод программы; не вышло за срок — она закрывается.
pub(crate) fn output_with_timeout(
    mut command: Command,
    timeout: Duration,
) -> Result<Output, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let read = |stream: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut stream) = stream {
                let _ = std::io::Read::read_to_end(&mut stream, &mut bytes);
            }
            bytes
        })
    };
    let stdout = read(child.stdout.take().map(|s| Box::new(s) as _));
    let stderr = read(child.stderr.take().map(|s| Box::new(s) as _));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("spawnSync ETIMEDOUT ({} мс)", timeout.as_millis()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(error) => return Err(error.to_string()),
        }
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// `taskkill.exe` из `System32` (по полному пути: текущая папка и `PATH` его не подменят), а если
/// файла там нет — по имени.
#[cfg(windows)]
fn taskkill_program() -> String {
    use hlsr_core::js::Platform;
    use hlsr_core::js::path::NodePath;
    let env = hlsr_delivery::environment::Environment::process();
    let full = NodePath::new(Platform::Win32, "C:\\").join(&[
        &crate::windows_tools::system_root(&env),
        "System32",
        "taskkill.exe",
    ]);
    if std::path::Path::new(&full).exists() {
        full
    } else {
        "taskkill.exe".to_owned()
    }
}

/// `run(command, args, timeoutMs)`: `execFile` с окном скрытым и сроком в 10 с.
#[cfg(windows)]
async fn run_taskkill(pid: u32, force: bool) -> StopResult {
    let mut args = vec!["/PID".to_owned(), pid.to_string(), "/T".to_owned()];
    if force {
        args.push("/F".to_owned());
    }
    let mut command = tokio::process::Command::new(taskkill_program());
    command.args(&args);
    command.creation_flags(0x0800_0000);
    command.kill_on_drop(true);
    command.stdin(Stdio::null());
    match tokio::time::timeout(Duration::from_secs(10), command.output()).await {
        Err(_) => StopResult::failure("ETIMEDOUT", "taskkill.exe: срок вышел"),
        Ok(Err(error)) => StopResult::failure(
            errno::code(&error)
                .map(std::borrow::Cow::into_owned)
                .unwrap_or_else(|| "UNKNOWN".to_owned()),
            error.to_string(),
        ),
        Ok(Ok(output)) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if output.status.success() {
                StopResult::success(text)
            } else {
                StopResult::failure(output.status.code().unwrap_or(-1).to_string(), text)
            }
        }
    }
}

/// `process.kill(pid, signal)` и `process.kill(-pid, signal)` на Linux.
#[cfg(unix)]
fn send_signal(pid: u32, group: bool, signal: rustix::process::Signal, name: &str) -> StopResult {
    use rustix::process::{Pid, kill_process, kill_process_group};
    let Some(target) = i32::try_from(pid).ok().and_then(Pid::from_raw) else {
        return StopResult::failure("ESRCH", "kill ESRCH");
    };
    let result = if group {
        kill_process_group(target, signal)
    } else {
        kill_process(target, signal)
    };
    match result {
        Ok(()) => StopResult::success(name),
        Err(errno) => {
            let error = std::io::Error::from_raw_os_error(errno.raw_os_error());
            let code = errno::code(&error)
                .map(std::borrow::Cow::into_owned)
                .unwrap_or_else(|| "UNKNOWN".to_owned());
            StopResult::failure(code.clone(), format!("kill {code}"))
        }
    }
}

impl Stopper for SystemStopper {
    fn graceful(&self, pid: u32) -> BoxFuture<'_, StopResult> {
        #[cfg(windows)]
        {
            Box::pin(run_taskkill(pid, false))
        }
        #[cfg(unix)]
        {
            Box::pin(
                async move { send_signal(pid, false, rustix::process::Signal::TERM, "SIGTERM") },
            )
        }
    }

    fn force(&self, pid: u32) -> BoxFuture<'_, StopResult> {
        #[cfg(windows)]
        {
            Box::pin(run_taskkill(pid, true))
        }
        #[cfg(unix)]
        {
            Box::pin(
                async move { send_signal(pid, false, rustix::process::Signal::KILL, "SIGKILL") },
            )
        }
    }

    // В Windows `taskkill /T` и так закрывает дерево.
    fn graceful_group(&self, pid: u32) -> BoxFuture<'_, StopResult> {
        #[cfg(windows)]
        {
            self.graceful(pid)
        }
        #[cfg(unix)]
        {
            Box::pin(
                async move { send_signal(pid, true, rustix::process::Signal::TERM, "SIGTERM") },
            )
        }
    }

    fn force_group(&self, pid: u32) -> BoxFuture<'_, StopResult> {
        #[cfg(windows)]
        {
            self.force(pid)
        }
        #[cfg(unix)]
        {
            Box::pin(
                async move { send_signal(pid, true, rustix::process::Signal::KILL, "SIGKILL") },
            )
        }
    }
}
