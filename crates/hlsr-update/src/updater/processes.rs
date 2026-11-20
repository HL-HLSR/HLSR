//! Процессы апдейтера и службы обновления: установщик (`spawn` с ожиданием выхода), отвязанный
//! запуск лаунчера, копии апдейтера и `HLSR-Setup-<v>.exe` (`detached`), проверка «процесс жив»
//! (`process.kill(pid, 0)`). За трейтом [`ProcessLauncher`] — ради тестов ([`FakeLauncher`]) и
//! сквозных прогонов без запуска ([`DryRunLauncher`], dev-hook `HLSR_LAUNCH_DRY_RUN`).
//!
//! Окружение дочернего процесса задаётся целиком ([`Spawn::env`]): окружение 0.4.7 без
//! переменных распаковщика (`PORTABLE_EXECUTABLE_*` — иначе лаунчер снова станет установщиком),
//! окна (`SLINT_*`, `WINIT_*`) и AppImage ([`child_env`]).

use crate::ports::BoxFuture;
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

/// Переменная, которую дочерний процесс лаунчера не получает.
pub fn is_launcher_only_var(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("PORTABLE_EXECUTABLE_")
        || upper.starts_with("SLINT_")
        || upper.starts_with("WINIT_")
        || matches!(
            upper.as_str(),
            "APPIMAGE" | "APPDIR" | "ARGV0" | "OWD" | "APPIMAGE_EXTRACT_AND_RUN"
        )
}

/// Окружение дочернего процесса из окружения `source` (`process.env`).
pub fn child_env<I, K, V>(source: I) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    source
        .into_iter()
        .map(|(name, value)| (name.into(), value.into()))
        .filter(|(name, _)| !is_launcher_only_var(name))
        .collect()
}

/// Окружение этого процесса для дочернего (имена и значения не в UTF-8 пропускаются).
pub fn process_child_env() -> Vec<(String, String)> {
    child_env(
        std::env::vars_os().filter_map(|(name, value)| {
            Some((name.into_string().ok()?, value.into_string().ok()?))
        }),
    )
}

/// Задать переменную (`{...env, NAME: value}`; имя без учёта регистра, как в Windows).
pub fn set_env(env: &mut Vec<(String, String)>, name: &str, value: &str) {
    match env
        .iter_mut()
        .find(|(existing, _)| existing.eq_ignore_ascii_case(name))
    {
        Some(slot) => slot.1 = value.to_owned(),
        None => env.push((name.to_owned(), value.to_owned())),
    }
}

/// Что и как запустить.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spawn {
    pub file: PathBuf,
    pub args: Vec<String>,
    /// Рабочий каталог; `None` — каталог этого процесса.
    pub cwd: Option<PathBuf>,
    /// Окружение целиком.
    pub env: Vec<(String, String)>,
    /// `windowsHide`: без окна консоли.
    pub hidden: bool,
    /// `detached`: отвязанный процесс, его выход никто не ждёт.
    pub detached: bool,
}

impl Spawn {
    /// Запись для dry-run и тестов: `{file, args, cwd, env, hidden, detached}`.
    pub fn to_value(&self) -> Value {
        let env: Map<String, Value> = self
            .env
            .iter()
            .map(|(name, value)| (name.clone(), Value::String(value.clone())))
            .collect();
        json!({
            "file": self.file.to_string_lossy(),
            "args": self.args,
            "cwd": self.cwd.as_ref().map(|cwd| cwd.to_string_lossy().into_owned()),
            "env": env,
            "hidden": self.hidden,
            "detached": self.detached,
        })
    }

    /// Значение переменной окружения команды.
    pub fn env_var(&self, name: &str) -> Option<&str> {
        self.env
            .iter()
            .find(|(existing, _)| existing.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Как кончился процесс, которого ждали.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunExit {
    /// Вышел сам; код выхода (`None` — убит сигналом, у Node `code === null`).
    Exited(Option<i32>),
    /// Не уложился в срок и снят.
    TimedOut,
}

/// Запуск процессов.
pub trait ProcessLauncher: Send + Sync {
    /// Запустить и не ждать; PID процесса. Ошибка — текст для журнала (`spawn <файл> <код>`).
    fn spawn(&self, command: &Spawn) -> Result<u32, String>;
    /// Запустить и дождаться выхода не дольше `timeout`; не уложился — процесс снимается.
    fn run<'a>(
        &'a self,
        command: &'a Spawn,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<RunExit, String>>;
    /// `process.kill(pid, 0)`: процесс есть (или есть, но чужой — `EPERM`).
    fn is_alive(&self, pid: u32) -> bool;
}

/// `process.kill(pid, 0)` удался или ответил `EPERM`.
pub fn pid_is_alive(pid: f64) -> bool {
    match hlsr_core::js::process::kill_zero(pid) {
        Ok(()) => true,
        Err(code) => code == "EPERM",
    }
}

/// Процессы ОС.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemLauncher;

impl SystemLauncher {
    pub(crate) fn build(command: &Spawn) -> std::process::Command {
        let mut process = std::process::Command::new(&command.file);
        process
            .args(&command.args)
            .env_clear()
            .envs(command.env.iter().map(|(k, v)| (k, v)))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Some(cwd) = &command.cwd {
            process.current_dir(cwd);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let mut flags = 0;
            if command.detached {
                flags |= DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
            } else if command.hidden {
                flags |= CREATE_NO_WINDOW;
            }
            process.creation_flags(flags);
        }
        process
    }

    pub(crate) fn spawn_error(command: &Spawn, error: &std::io::Error) -> String {
        let file = command.file.to_string_lossy();
        match hlsr_core::js::errno::code(error) {
            Some(code) => format!("spawn {file} {code}"),
            None => error.to_string(),
        }
    }
}

impl ProcessLauncher for SystemLauncher {
    fn spawn(&self, command: &Spawn) -> Result<u32, String> {
        Self::build(command)
            .spawn()
            .map(|child| child.id())
            .map_err(|error| Self::spawn_error(command, &error))
    }

    fn run<'a>(
        &'a self,
        command: &'a Spawn,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<RunExit, String>> {
        Box::pin(async move {
            let mut child = Self::build(command)
                .spawn()
                .map_err(|error| Self::spawn_error(command, &error))?;
            let deadline = tokio::time::Instant::now() + timeout;
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => return Ok(RunExit::Exited(status.code())),
                    Ok(None) => {}
                    Err(error) => return Err(error.to_string()),
                }
                if tokio::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(RunExit::TimedOut);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
    }

    fn is_alive(&self, pid: u32) -> bool {
        pid_is_alive(f64::from(pid))
    }
}

/// Вызов, записанный подделкой.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchCall {
    Spawn(Spawn),
    Run(Spawn, Duration),
}

impl LaunchCall {
    pub fn command(&self) -> &Spawn {
        match self {
            LaunchCall::Spawn(command) | LaunchCall::Run(command, _) => command,
        }
    }
}

type SpawnResponder = Box<dyn Fn(&Spawn) -> Result<u32, String> + Send + Sync>;
type RunResponder = Box<dyn Fn(&Spawn) -> Result<RunExit, String> + Send + Sync>;
type AliveResponder = Box<dyn Fn(u32) -> bool + Send + Sync>;

/// Подделка: записывает вызовы; ответы задаёт тест (по умолчанию PID 4242, код 0, процесс жив).
pub struct FakeLauncher {
    calls: Mutex<Vec<LaunchCall>>,
    spawn: SpawnResponder,
    run: RunResponder,
    alive: AliveResponder,
}

impl Default for FakeLauncher {
    fn default() -> FakeLauncher {
        FakeLauncher::new()
    }
}

impl FakeLauncher {
    pub fn new() -> FakeLauncher {
        FakeLauncher {
            calls: Mutex::new(Vec::new()),
            spawn: Box::new(|_| Ok(4242)),
            run: Box::new(|_| Ok(RunExit::Exited(Some(0)))),
            alive: Box::new(|_| true),
        }
    }

    pub fn on_spawn(
        mut self,
        respond: impl Fn(&Spawn) -> Result<u32, String> + Send + Sync + 'static,
    ) -> FakeLauncher {
        self.spawn = Box::new(respond);
        self
    }

    pub fn on_run(
        mut self,
        respond: impl Fn(&Spawn) -> Result<RunExit, String> + Send + Sync + 'static,
    ) -> FakeLauncher {
        self.run = Box::new(respond);
        self
    }

    pub fn on_alive(
        mut self,
        respond: impl Fn(u32) -> bool + Send + Sync + 'static,
    ) -> FakeLauncher {
        self.alive = Box::new(respond);
        self
    }

    pub fn calls(&self) -> Vec<LaunchCall> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn record(&self, call: LaunchCall) {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(call);
    }
}

impl ProcessLauncher for FakeLauncher {
    fn spawn(&self, command: &Spawn) -> Result<u32, String> {
        self.record(LaunchCall::Spawn(command.clone()));
        (self.spawn)(command)
    }

    fn run<'a>(
        &'a self,
        command: &'a Spawn,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<RunExit, String>> {
        self.record(LaunchCall::Run(command.clone(), timeout));
        let result = (self.run)(command);
        Box::pin(async move { result })
    }

    fn is_alive(&self, pid: u32) -> bool {
        (self.alive)(pid)
    }
}

/// Dev-hook `HLSR_LAUNCH_DRY_RUN=<файл>`: вместо запуска дописать в файл строку JSON
/// `{"kind": "spawn"|"run", …команда}`; запуск «удался» (PID 0, код 0, процесс «жив»).
#[cfg(any(debug_assertions, feature = "dev-hooks"))]
#[derive(Debug, Clone)]
pub struct DryRunLauncher {
    file: PathBuf,
}

#[cfg(any(debug_assertions, feature = "dev-hooks"))]
impl DryRunLauncher {
    pub fn new(file: &std::path::Path) -> DryRunLauncher {
        DryRunLauncher {
            file: file.to_owned(),
        }
    }

    fn record(&self, kind: &str, command: &Spawn) {
        use std::io::Write as _;
        let mut line = command.to_value();
        line["kind"] = Value::String(kind.to_owned());
        let appended = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)
            .and_then(|mut file| writeln!(file, "{line}"));
        if let Err(error) = appended {
            tracing::warn!("[dry-run] {}: {error}", self.file.display());
        }
    }

    /// Записанные команды по порядку.
    pub fn read(file: &std::path::Path) -> Vec<Value> {
        std::fs::read_to_string(file)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }
}

#[cfg(any(debug_assertions, feature = "dev-hooks"))]
impl ProcessLauncher for DryRunLauncher {
    fn spawn(&self, command: &Spawn) -> Result<u32, String> {
        self.record("spawn", command);
        Ok(0)
    }

    fn run<'a>(
        &'a self,
        command: &'a Spawn,
        _timeout: Duration,
    ) -> BoxFuture<'a, Result<RunExit, String>> {
        self.record("run", command);
        Box::pin(async { Ok(RunExit::Exited(Some(0))) })
    }

    fn is_alive(&self, _pid: u32) -> bool {
        true
    }
}
