//! Запуск процессов (`spawn` 0.4.7 и его `ChildProcess`) для всех ОС — игры Windows, Proton,
//! инструменты: порт [`Spawner`] с настоящим [`SystemSpawner`] и дочерний процесс [`Child`] — PID, события `spawn`/`error`/
//! `exit` (у процесса с каналами — после их закрытия, как `close`), вывод кусками, сигналы
//! процессу и его группе. Поддельный процесс для тестов собирается [`Child::new`] и управляется
//! [`ChildController`].
//!
//! Окружение детей — как у 0.4.7, без переменных Slint и winit, которые ставит лаунчер
//! (спецификация ПП3, решение 3): общий список — [`crate::child_env`].

use crate::game_process_service::{ChildEnd, ChildHandle};
use hlsr_delivery::errors::RawError;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};

pub use crate::child_env::{Env, is_launcher_ui_var, process_env};

/// `path.delimiter`.
pub const PATH_DELIMITER: char = if cfg!(windows) { ';' } else { ':' };

/// Куда идёт ввод и вывод (`stdio`).
#[derive(Debug, Default)]
pub enum Stdio {
    /// `"ignore"`.
    #[default]
    Ignore,
    /// `["ignore", "pipe", "pipe"]`: вывод читается кусками.
    Pipe,
    /// `["ignore", fd, fd]`: вывод и ошибки — в файл журнала.
    Log(std::fs::File),
}

/// Параметры `spawn`.
#[derive(Debug, Default)]
pub struct SpawnOptions {
    pub env: Env,
    pub cwd: Option<PathBuf>,
    pub stdio: Stdio,
    /// `detached: true`: своя группа процессов.
    pub detached: bool,
    /// `windowsHide` (флаг `CREATE_NO_WINDOW` на Windows). `None` — по умолчанию окно консоли
    /// скрывается у процесса, который не `detached`.
    pub windows_hide: Option<bool>,
}

/// Чем закончился процесс: `exit`/`close` с кодом или сигналом, либо `error`.
#[derive(Debug, Clone, PartialEq)]
pub enum ChildExit {
    Exit {
        code: Option<i32>,
        signal: Option<String>,
    },
    Error(RawError),
}

#[derive(Debug, Clone, Default)]
struct Phase {
    spawned: bool,
    end: Option<ChildExit>,
}

/// Сигналы процессу: `child.kill(signal)` и `process.kill(-pid, signal)`.
pub trait ChildControl: Send + Sync {
    /// `child.kill(signal)`: `true` — сигнал отправлен.
    fn kill(&self, signal: &str) -> bool;
    /// `process.kill(-pid, signal)`: группа процессов; `false` — не вышло (нет группы, Windows).
    fn kill_group(&self, signal: &str) -> bool;
}

struct NoControl;

impl ChildControl for NoControl {
    fn kill(&self, _signal: &str) -> bool {
        false
    }
    fn kill_group(&self, _signal: &str) -> bool {
        false
    }
}

type Output = mpsc::UnboundedReceiver<Vec<u8>>;

struct ChildInner {
    pid: Option<u32>,
    phase: watch::Receiver<Phase>,
    stdout: Mutex<Option<Output>>,
    stderr: Mutex<Option<Output>>,
    control: Arc<dyn ChildControl>,
}

/// Дочерний процесс (`ChildProcess`). Копии — тот же процесс.
#[derive(Clone)]
pub struct Child {
    inner: Arc<ChildInner>,
}

impl std::fmt::Debug for Child {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Child")
            .field("pid", &self.inner.pid)
            .finish()
    }
}

/// Управление процессом со стороны того, кто его запустил: события и вывод.
pub struct ChildController {
    phase: watch::Sender<Phase>,
    stdout: mpsc::UnboundedSender<Vec<u8>>,
    stderr: mpsc::UnboundedSender<Vec<u8>>,
}

impl ChildController {
    /// Событие `spawn`.
    pub fn spawned(&self) {
        self.phase.send_modify(|phase| phase.spawned = true);
    }

    /// Кусок вывода (`stdout.emit("data")`).
    pub fn stdout(&self, chunk: impl Into<Vec<u8>>) {
        let _ = self.stdout.send(chunk.into());
    }

    /// Кусок ошибок (`stderr.emit("data")`).
    pub fn stderr(&self, chunk: impl Into<Vec<u8>>) {
        let _ = self.stderr.send(chunk.into());
    }

    /// `exit`/`close`: первое завершение остаётся.
    pub fn exit(&self, code: Option<i32>, signal: Option<&str>) {
        self.end(ChildExit::Exit {
            code,
            signal: signal.map(str::to_owned),
        });
    }

    /// Событие `error`.
    pub fn error(&self, error: RawError) {
        self.end(ChildExit::Error(error));
    }

    fn end(&self, end: ChildExit) {
        self.phase.send_modify(|phase| {
            if phase.end.is_none() {
                phase.end = Some(end);
            }
        });
    }
}

impl Child {
    /// Процесс и его управление. `control` — куда идут сигналы.
    pub fn new(pid: Option<u32>, control: Arc<dyn ChildControl>) -> (Child, ChildController) {
        let (phase_sender, phase) = watch::channel(Phase::default());
        let (stdout_sender, stdout) = mpsc::unbounded_channel();
        let (stderr_sender, stderr) = mpsc::unbounded_channel();
        let child = Child {
            inner: Arc::new(ChildInner {
                pid,
                phase,
                stdout: Mutex::new(Some(stdout)),
                stderr: Mutex::new(Some(stderr)),
                control,
            }),
        };
        let controller = ChildController {
            phase: phase_sender,
            stdout: stdout_sender,
            stderr: stderr_sender,
        };
        (child, controller)
    }

    /// Процесс, который не запустился (`error` сразу).
    pub fn failed(error: RawError) -> Child {
        let (child, controller) = Child::new(None, Arc::new(NoControl));
        controller.error(error);
        child
    }

    /// `child.pid`.
    pub fn pid(&self) -> Option<u32> {
        self.inner.pid
    }

    /// Процесс завершился (`exitCode != null || signalCode != null`).
    pub fn has_exited(&self) -> bool {
        matches!(self.inner.phase.borrow().end, Some(ChildExit::Exit { .. }))
    }

    /// Завершение, если оно уже было.
    pub fn exit_state(&self) -> Option<ChildExit> {
        self.inner.phase.borrow().end.clone()
    }

    /// `once("spawn")` или `once("error")`: что наступит раньше.
    pub async fn spawned(&self) -> Result<(), RawError> {
        let mut receiver = self.inner.phase.clone();
        let phase = match receiver
            .wait_for(|phase| phase.spawned || phase.end.is_some())
            .await
        {
            Ok(phase) => phase.clone(),
            Err(_) => return Err(RawError::new("процесс потерян")),
        };
        match phase.end {
            Some(ChildExit::Error(error)) if !phase.spawned => Err(error),
            _ => Ok(()),
        }
    }

    /// `once("exit")`/`once("close")`/`once("error")`: завершение процесса.
    pub async fn ended(&self) -> ChildExit {
        let mut receiver = self.inner.phase.clone();
        match receiver.wait_for(|phase| phase.end.is_some()).await {
            Ok(phase) => phase.end.clone().expect("завершение есть"),
            Err(_) => ChildExit::Error(RawError::new("процесс потерян")),
        }
    }

    /// Вывод процесса (`stdout`, `stderr`) — один раз; у процесса без каналов он пуст.
    pub fn take_output(&self) -> (Option<Output>, Option<Output>) {
        let take = |slot: &Mutex<Option<Output>>| {
            slot.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
        };
        (take(&self.inner.stdout), take(&self.inner.stderr))
    }

    /// `child.kill(signal)`.
    pub fn kill(&self, signal: &str) -> bool {
        self.inner.control.kill(signal)
    }

    /// `process.kill(-child.pid, signal)`: `false` — не вышло.
    pub fn kill_group(&self, signal: &str) -> bool {
        self.inner.control.kill_group(signal)
    }

    /// Ожидание выхода для службы процессов игры (`registerStartedProcess({child})`).
    pub fn handle(&self) -> Option<ChildHandle> {
        let pid = self.pid()?;
        let child = self.clone();
        Some(ChildHandle {
            pid,
            wait: Box::pin(async move {
                match child.ended().await {
                    ChildExit::Exit { code, signal } => ChildEnd::Exit {
                        code: code.map(i64::from),
                        signal,
                    },
                    ChildExit::Error(error) => ChildEnd::Error(error.message),
                }
            }),
        })
    }
}

/// Запуск процесса (`spawnFn` в параметрах 0.4.7).
pub trait Spawner: Send + Sync {
    fn spawn(&self, command: &str, args: &[String], options: SpawnOptions) -> Child;
}

/// Настоящие процессы.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemSpawner;

/// `spawn <file> ENOENT`: текст и код ошибки запуска, как у Node.
pub fn spawn_error(command: &str, error: &std::io::Error) -> RawError {
    let code = hlsr_core::js::errno::code(error)
        .map(std::borrow::Cow::into_owned)
        .unwrap_or_else(|| "UNKNOWN".to_owned());
    RawError {
        message: format!("spawn {command} {code}"),
        code: Some(code),
        syscall: Some(format!("spawn {command}")),
        path: Some(command.to_owned()),
        stderr: None,
    }
}

/// Сигналы настоящему процессу: себе — через задачу ожидания, группе — `kill(-pid)`.
struct SystemControl {
    pid: Option<u32>,
    kill: mpsc::UnboundedSender<String>,
}

impl ChildControl for SystemControl {
    fn kill(&self, signal: &str) -> bool {
        self.kill.send(signal.to_owned()).is_ok()
    }

    fn kill_group(&self, signal: &str) -> bool {
        #[cfg(unix)]
        {
            use rustix::process::{Pid, kill_process_group};
            self.pid
                .and_then(|pid| i32::try_from(pid).ok())
                .and_then(Pid::from_raw)
                .is_some_and(|pid| kill_process_group(pid, signal_of(signal)).is_ok())
        }
        #[cfg(not(unix))]
        {
            let _ = (self.pid, signal);
            false
        }
    }
}

/// Сигнал по имени Node.
#[cfg(unix)]
pub fn signal_of(name: &str) -> rustix::process::Signal {
    use rustix::process::Signal;
    match name {
        "SIGKILL" => Signal::KILL,
        "SIGINT" => Signal::INT,
        "SIGHUP" => Signal::HUP,
        _ => Signal::TERM,
    }
}

impl Spawner for SystemSpawner {
    fn spawn(&self, command: &str, args: &[String], options: SpawnOptions) -> Child {
        let mut process = tokio::process::Command::new(command);
        process.args(args).env_clear();
        for (name, value) in &options.env {
            if !is_launcher_ui_var(name) {
                process.env(name, value);
            }
        }
        if let Some(cwd) = &options.cwd {
            process.current_dir(cwd);
        }
        process.stdin(std::process::Stdio::null());
        let piped = matches!(options.stdio, Stdio::Pipe);
        match options.stdio {
            Stdio::Ignore => {
                process
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
            }
            Stdio::Pipe => {
                process
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
            }
            Stdio::Log(file) => {
                let out = file
                    .try_clone()
                    .map_or_else(|_| std::process::Stdio::null(), std::process::Stdio::from);
                process.stdout(out).stderr(std::process::Stdio::from(file));
            }
        }
        detach(&mut process, options.detached, options.windows_hide);
        let mut spawned = match process.spawn() {
            Ok(spawned) => spawned,
            Err(error) => return Child::failed(spawn_error(command, &error)),
        };
        let pid = spawned.id();
        let (kill_sender, mut kill_requests) = mpsc::unbounded_channel::<String>();
        let (child, controller) = Child::new(
            pid,
            Arc::new(SystemControl {
                pid,
                kill: kill_sender,
            }),
        );
        controller.spawned();
        let stdout = spawned.stdout.take();
        let stderr = spawned.stderr.take();
        tokio::spawn(async move {
            let controller = Arc::new(controller);
            let out = forward(stdout, controller.clone(), true);
            let err = forward(stderr, controller.clone(), false);
            let readers = tokio::spawn(async move {
                let _ = tokio::join!(out, err);
            });
            let status = loop {
                tokio::select! {
                    status = spawned.wait() => break status,
                    Some(signal) = kill_requests.recv() => send_kill(&mut spawned, pid, &signal),
                }
            };
            if piped {
                let _ = readers.await;
            }
            match status {
                Ok(status) => controller.exit(status.code(), exit_signal(&status).as_deref()),
                Err(error) => controller.error(RawError::new(error.to_string())),
            }
        });
        child
    }
}

/// Вывод канала кусками в управление процесса.
async fn forward<R>(reader: Option<R>, controller: Arc<ChildController>, stdout: bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt as _;
    let Some(mut reader) = reader else {
        return;
    };
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                let chunk = buffer[..read].to_vec();
                if stdout {
                    controller.stdout(chunk);
                } else {
                    controller.stderr(chunk);
                }
            }
        }
    }
}

/// `child.kill(signal)` настоящему процессу.
fn send_kill(child: &mut tokio::process::Child, pid: Option<u32>, signal: &str) {
    #[cfg(unix)]
    if signal != "SIGKILL"
        && let Some(pid) = pid
            .and_then(|pid| i32::try_from(pid).ok())
            .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process(pid, signal_of(signal));
        return;
    }
    let _ = (pid, signal);
    let _ = child.start_kill();
}

#[cfg(unix)]
fn exit_signal(status: &std::process::ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt as _;
    status
        .signal()
        .map(crate::game_process_service::signal_name)
}

#[cfg(not(unix))]
fn exit_signal(_status: &std::process::ExitStatus) -> Option<String> {
    None
}

/// `detached` (своя группа процессов) и запуск без окна консоли на Windows: флаги создания как
/// у libuv (`detached` — `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`, `windowsHide` —
/// `CREATE_NO_WINDOW`).
fn detach(command: &mut tokio::process::Command, detached: bool, windows_hide: Option<bool>) {
    #[cfg(windows)]
    {
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut flags = 0;
        if windows_hide.unwrap_or(!detached) {
            flags |= CREATE_NO_WINDOW;
        }
        if detached {
            flags |= DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        }
        command.creation_flags(flags);
    }
    #[cfg(unix)]
    {
        let _ = windows_hide;
        if detached {
            command.process_group(0);
        }
    }
}

/// `executableInPath(name, env)`: первый исполняемый `name` в `PATH`.
pub fn executable_in_path(name: &str, env: &Env) -> Option<PathBuf> {
    let path = env.get("PATH").map(String::as_str).unwrap_or("");
    path.split(PATH_DELIMITER)
        .filter(|dir| !dir.is_empty())
        .map(|dir| PathBuf::from(dir).join(name))
        .find(|candidate| is_executable(candidate))
}

/// `fs.accessSync(file, X_OK)`: на Windows — только существование (как у Node).
pub fn is_executable(file: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        rustix::fs::access(file, rustix::fs::Access::EXEC_OK).is_ok()
    }
    #[cfg(not(unix))]
    {
        file.exists()
    }
}
