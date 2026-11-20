//! Процессы установщика и удаления Windows: запуск установленного `HLSR.exe` (`spawn` с
//! `detached`), `icacls` и PowerShell (`spawnSync`/ожидание выхода), отложенное удаление каталога.
//! За трейтом [`Processes`] — ради тестов ([`FakeProcesses`]) и сквозных прогонов без запуска
//! ([`DryRunProcesses`], dev-hook `HLSR_LAUNCH_DRY_RUN`).
//!
//! Окружение дочерних процессов задаётся целиком ([`Env`]): как `process.env` 0.4.7 (на Windows
//! имена без учёта регистра), из которого вызывающий убирает лишнее ([`launch_environment`]).

use serde_json::{Map, Value, json};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// Окружение процесса: пары `имя=значение` в порядке появления. Поиск имени — без учёта регистра
/// ASCII, как у `process.env` на Windows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    vars: Vec<(String, String)>,
}

impl Env {
    /// Окружение этого процесса (имена и значения не в UTF-8 пропускаются).
    pub fn from_process() -> Env {
        Env {
            vars: std::env::vars_os()
                .filter_map(|(name, value)| {
                    Some((name.into_string().ok()?, value.into_string().ok()?))
                })
                .collect(),
        }
    }

    pub fn from_pairs<I, K, V>(pairs: I) -> Env
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut env = Env::default();
        for (name, value) in pairs {
            env.set(&name.into(), &value.into());
        }
        env
    }

    /// Значение переменной (пустая строка — тоже значение).
    pub fn get(&self, name: &str) -> Option<String> {
        self.vars
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }

    /// Задать или заменить переменную (`{ ...env, NAME: value }`).
    pub fn set(&mut self, name: &str, value: &str) {
        match self
            .vars
            .iter_mut()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        {
            Some(slot) => slot.1 = value.to_owned(),
            None => self.vars.push((name.to_owned(), value.to_owned())),
        }
    }

    /// Убрать переменные, имя которых подходит под `drop`.
    pub fn remove_where(&mut self, drop: impl Fn(&str) -> bool) {
        self.vars.retain(|(name, _)| !drop(name));
    }

    pub fn vars(&self) -> &[(String, String)] {
        &self.vars
    }

    /// Функция поиска для проверок путей (`layout::EnvLookup`).
    pub fn lookup(&self) -> impl Fn(&str) -> Option<String> + '_ {
        move |name| self.get(name)
    }
}

/// Переменная окна лаунчера (масштаб Slint и winit): дочерним процессам не передаётся
/// (решение 3 ПП3; тот же список, что `hlsr-platform::child_env`).
pub fn is_launcher_ui_var(name: &str) -> bool {
    name.starts_with("SLINT_") || name.starts_with("WINIT_")
}

/// `launchEnvironment(source)`: окружение без переменных распаковщика (`PORTABLE_EXECUTABLE_*`) —
/// иначе запущенный лаунчер снова принял бы себя за установщик, — и без переменных окна
/// установщика (`SLINT_SCALE_FACTOR` и прочие `SLINT_*`/`WINIT_*`).
pub fn launch_environment(source: &Env) -> Env {
    let mut env = source.clone();
    env.remove_where(|name| name.starts_with("PORTABLE_EXECUTABLE_") || is_launcher_ui_var(name));
    env
}

/// Команда: что и как запустить.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub file: String,
    pub args: Vec<String>,
    /// Рабочий каталог; `None` — каталог этого процесса.
    pub cwd: Option<String>,
    /// Окружение целиком.
    pub env: Env,
    /// Без окна консоли (`windowsHide`).
    pub hidden: bool,
}

impl Command {
    /// Команда для журнала dry-run и тестов: `{file, args, cwd, env, hidden}`.
    pub fn to_value(&self) -> Value {
        let env: Map<String, Value> = self
            .env
            .vars()
            .iter()
            .map(|(name, value)| (name.clone(), Value::String(value.clone())))
            .collect();
        json!({"file": self.file, "args": self.args, "cwd": self.cwd, "env": env, "hidden": self.hidden})
    }
}

/// Ошибка запуска (`spawn` Node): код libuv и сообщение `spawn <файл> <код>`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct SpawnError {
    pub code: Option<String>,
    pub message: String,
}

impl SpawnError {
    /// Ошибка с кодом Node (`EACCES`, `ENOENT`…) для файла `file`.
    pub fn with_code(file: &str, code: &str) -> SpawnError {
        SpawnError {
            code: Some(code.to_owned()),
            message: format!("spawn {file} {code}"),
        }
    }

    fn from_io(file: &str, error: &std::io::Error) -> SpawnError {
        match hlsr_core::js::errno::code(error) {
            Some(code) => SpawnError::with_code(file, &code),
            None => SpawnError {
                code: None,
                message: error.to_string(),
            },
        }
    }
}

/// Запуск процессов.
pub trait Processes: Send + Sync {
    /// Запустить отвязанным (`spawn` с `detached: true`, `stdio: "ignore"`) и не ждать; `Ok` — процесс
    /// создан (событие `spawn`).
    fn spawn(&self, command: &Command) -> Result<(), SpawnError>;
    /// Запустить и дождаться выхода (`spawnSync`); код выхода, `None` — не запустился или не
    /// уложился в `timeout` (тогда он завершается).
    fn run(&self, command: &Command, timeout: Option<Duration>) -> Option<i32>;
}

/// Процессы ОС.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemProcesses;

impl SystemProcesses {
    fn build(command: &Command, detached: bool) -> std::process::Command {
        let mut process = std::process::Command::new(&command.file);
        process
            .args(&command.args)
            .env_clear()
            .envs(command.env.vars().iter().map(|(k, v)| (k, v)))
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
            if detached {
                flags |= DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
            } else if command.hidden {
                flags |= CREATE_NO_WINDOW;
            }
            process.creation_flags(flags);
        }
        #[cfg(not(windows))]
        let _ = detached;
        process
    }
}

impl Processes for SystemProcesses {
    fn spawn(&self, command: &Command) -> Result<(), SpawnError> {
        Self::build(command, true)
            .spawn()
            .map(drop)
            .map_err(|error| SpawnError::from_io(&command.file, &error))
    }

    fn run(&self, command: &Command, timeout: Option<Duration>) -> Option<i32> {
        let mut child = Self::build(command, false).spawn().ok()?;
        let deadline = timeout.map(|limit| std::time::Instant::now() + limit);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.code(),
                Ok(None) => {}
                Err(_) => return None,
            }
            if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Вызов, записанный подделкой или dry-run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Spawn(Command),
    Run(Command, Option<Duration>),
}

impl Call {
    pub fn command(&self) -> &Command {
        match self {
            Call::Spawn(command) | Call::Run(command, _) => command,
        }
    }
}

type SpawnResponder = Box<dyn Fn(&Command, usize) -> Result<(), SpawnError> + Send + Sync>;
type RunResponder = Box<dyn Fn(&Command) -> Option<i32> + Send + Sync>;

/// Подделка: записывает вызовы; ответы задаёт тест (по умолчанию всё запускается, код 0).
pub struct FakeProcesses {
    calls: Mutex<Vec<Call>>,
    spawn: SpawnResponder,
    run: RunResponder,
}

impl Default for FakeProcesses {
    fn default() -> FakeProcesses {
        FakeProcesses::new()
    }
}

impl FakeProcesses {
    pub fn new() -> FakeProcesses {
        FakeProcesses {
            calls: Mutex::new(Vec::new()),
            spawn: Box::new(|_, _| Ok(())),
            run: Box::new(|_| Some(0)),
        }
    }

    /// Ответ на `spawn`: функция команды и номера вызова `spawn` (с нуля).
    pub fn on_spawn(
        mut self,
        respond: impl Fn(&Command, usize) -> Result<(), SpawnError> + Send + Sync + 'static,
    ) -> FakeProcesses {
        self.spawn = Box::new(respond);
        self
    }

    /// Ответ на `run`.
    pub fn on_run(
        mut self,
        respond: impl Fn(&Command) -> Option<i32> + Send + Sync + 'static,
    ) -> FakeProcesses {
        self.run = Box::new(respond);
        self
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl Processes for FakeProcesses {
    fn spawn(&self, command: &Command) -> Result<(), SpawnError> {
        let mut calls = self
            .calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let index = calls
            .iter()
            .filter(|call| matches!(call, Call::Spawn(_)))
            .count();
        calls.push(Call::Spawn(command.clone()));
        drop(calls);
        (self.spawn)(command, index)
    }

    fn run(&self, command: &Command, timeout: Option<Duration>) -> Option<i32> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(Call::Run(command.clone(), timeout));
        (self.run)(command)
    }
}

/// Dev-hook `HLSR_LAUNCH_DRY_RUN=<файл>`: вместо запуска дописать в файл строку JSON
/// `{"kind": "spawn"|"run", "file", "args", "cwd", "env", "hidden"}`; запуск «удался», код 0.
#[derive(Debug, Clone)]
pub struct DryRunProcesses {
    file: PathBuf,
}

impl DryRunProcesses {
    pub fn new(file: &Path) -> DryRunProcesses {
        DryRunProcesses {
            file: file.to_owned(),
        }
    }

    fn record(&self, kind: &str, command: &Command) {
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
    pub fn read(file: &Path) -> Vec<Value> {
        std::fs::read_to_string(file)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }
}

impl Processes for DryRunProcesses {
    fn spawn(&self, command: &Command) -> Result<(), SpawnError> {
        self.record("spawn", command);
        Ok(())
    }

    fn run(&self, command: &Command, _timeout: Option<Duration>) -> Option<i32> {
        self.record("run", command);
        Some(0)
    }
}
