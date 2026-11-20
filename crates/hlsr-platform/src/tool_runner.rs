//! Запуск, слежение и остановка инструментов (LiveSplit): `launch`/`launchDirect`, `track`,
//! `watchWineProcess` и `stop` из `ToolService.js`. Установка, проверки установленного, сплиты и
//! раскладка игры остаются в `hlsr_delivery::tool_service`; сюда они приходят готовым
//! [`ToolLaunch`].
//!
//! Windows: инструмент запускается напрямую. Он отделён от лаунчера и переживает его закрытие, но
//! пока лаунчер открыт, процесс отслеживается и его можно закрыть из лаунчера (`taskkill /T`,
//! без `/F` окно успевает спросить про сплиты). Linux: инструмент идёт через Proton в префиксе
//! игры ([`PrefixLauncher`], его даёт `ProtonRunner`), закрывается сигналом группе процессов, а
//! закрытый своим крестиком при живой сессии `umu` определяется по процессу Wine
//! (`/proc/*/comm`).
//!
//! Запущенный инструмент запоминается до выхода процесса: по этому интерфейс показывает «Закрыть»
//! вместо «Запустить». О смене состояния (запуск, закрытие, выход) сообщается подписчикам
//! ([`ToolProcessRunner::subscribe`], событие `state` 0.4.7) — по ним `DeliveryApi` шлёт
//! `tool-status` и переписывает `overlay.ini`.

use crate::game_process_service::{ChildEnd, ChildHandle};
use crate::process_stop::{Stopper, SystemStopper};
use hlsr_core::js::{self, Platform};
use hlsr_core::logger::{Logger, TracingLogger};
use hlsr_delivery::errors::{Code, DeliveryError, Failure};
use hlsr_delivery::ports::{RunningTool, StopStatus, ToolLaunch, ToolRunner};
use hlsr_delivery::providers::BoxFuture;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;

/// Ожидание выхода после команды закрыть: 10 с (`setTimeout(…, 10000)` в `stop`).
pub const STOP_WAIT: Duration = Duration::from_secs(10);
/// `watchIntervalMs`: как часто смотреть, жив ли процесс Wine.
pub const WATCH_INTERVAL: Duration = Duration::from_secs(2);

/// Переменные окна лаунчера и их чистка — общие для всех дочерних процессов ([`crate::child_env`]).
pub use crate::child_env::{UI_ENVIRONMENT, strip_ui_environment};

/// Подписчик события `state`: идентификатор инструмента.
pub type StateListener = Arc<dyn Fn(&str) + Send + Sync>;

/// Подписка: для отписки (`off`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenerId(u64);

/// `launchOptions` в `protonRunner.launch(gameId, exe, args, launchOptions)`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrefixLaunchOptions {
    /// `cwd`: папка инструмента.
    pub cwd: PathBuf,
    /// `fontProfile` (`livesplit`).
    pub font_profile: Option<String>,
    /// `env`: добавки к окружению Proton (`toolLaunchEnvironment`).
    pub env: Option<Vec<(String, String)>>,
}

/// Результат `protonRunner.launch`: сессия `umu` (`child`) и префикс игры.
pub struct PrefixLaunched {
    /// `None` — сессии нет (как `child` без значения в 0.4.7): инструмент не отслеживается.
    pub child: Option<ChildHandle>,
    pub prefix: PathBuf,
}

/// Запуск exe в префиксе Proton игры: `ProtonRunner.launch` (задача 8).
pub trait PrefixLauncher: Send + Sync {
    fn launch<'a>(
        &'a self,
        game_id: Option<&'a str>,
        exe: &'a Path,
        args: &'a [String],
        options: &'a PrefixLaunchOptions,
    ) -> BoxFuture<'a, Result<PrefixLaunched, Failure>>;
}

/// `processRunning(name)`: есть ли процесс с таким именем; `None` — узнать нельзя.
pub type ProcessRunning = Arc<dyn Fn(&str) -> Option<bool> + Send + Sync>;

/// `wineProcessRunning(fs, exeName)`: процесс Wine с именем exe есть в `/proc` (Wine называет
/// процесс по exe, `comm` — до 15 знаков). `proc` — каталог `/proc` (в тестах — подделка).
pub fn wine_process_running(proc: &Path, exe_name: &str) -> Option<bool> {
    let wanted = js::slice_utf16(&exe_name.to_lowercase(), 15);
    let entries = std::fs::read_dir(proc).ok()?;
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        // Процесс уже завершился или чужой — следующий.
        let Ok(bytes) = std::fs::read(entry.path().join("comm")) else {
            continue;
        };
        let comm = String::from_utf8_lossy(&bytes);
        if js::trim(&comm).to_lowercase() == wanted {
            return Some(true);
        }
    }
    Some(false)
}

/// Параметры конструктора (`options` `ToolService`, часть про запуск).
#[derive(Clone, Default)]
pub struct ToolRunnerOptions {
    /// `platform`; по умолчанию — эта ОС.
    pub platform: Option<Platform>,
    pub logger: Option<Arc<dyn Logger>>,
    /// `stopper`: `taskkill` (Windows) или сигналы группе (Linux).
    pub stopper: Option<Arc<dyn Stopper>>,
    /// `protonRunner`: без него на Linux запуск отвечает `PROTON_NOT_INSTALLED`.
    pub proton: Option<Arc<dyn PrefixLauncher>>,
    /// `processRunning`; по умолчанию — поиск в `/proc`.
    pub process_running: Option<ProcessRunning>,
    /// `watchIntervalMs`; по умолчанию 2 с.
    pub watch_interval: Option<Duration>,
    /// Сколько ждать выхода после команды закрыть; по умолчанию 10 с.
    pub stop_timeout: Option<Duration>,
}

/// Запущенный инструмент (`running.get(toolId)`).
struct Record {
    /// Номер запуска: выход старого процесса не трогает запись нового.
    token: u64,
    /// `child.pid`; `None` — процесс не запустился.
    pid: Option<u32>,
    stopping: bool,
    game_id: Option<String>,
    splits: Option<String>,
}

struct Inner {
    platform: Platform,
    logger: Arc<dyn Logger>,
    stopper: Arc<dyn Stopper>,
    proton: Option<Arc<dyn PrefixLauncher>>,
    process_running: ProcessRunning,
    watch_interval: Duration,
    stop_timeout: Duration,
    running: Mutex<HashMap<String, Record>>,
    next_token: AtomicU64,
    /// Кто-то из запущенных вышел или закрыт: будит `stop`.
    changed: Notify,
    listeners: Mutex<(u64, Vec<(u64, StateListener)>)>,
}

/// Реализация [`ToolRunner`]: настоящие процессы инструментов.
#[derive(Clone)]
pub struct ToolProcessRunner {
    inner: Arc<Inner>,
}

fn proton_not_configured() -> Failure {
    // `PROTON_NOT_INSTALLED` не входит в `CODES`, поэтому код становится `INSTALL_FAILED`.
    DeliveryError::new(Code::coerce("PROTON_NOT_INSTALLED"), "Proton не настроен").into()
}

impl ToolProcessRunner {
    pub fn new(options: ToolRunnerOptions) -> ToolProcessRunner {
        ToolProcessRunner {
            inner: Arc::new(Inner {
                platform: options.platform.unwrap_or(Platform::CURRENT),
                logger: options.logger.unwrap_or_else(|| Arc::new(TracingLogger)),
                stopper: options.stopper.unwrap_or_else(|| Arc::new(SystemStopper)),
                proton: options.proton,
                process_running: options.process_running.unwrap_or_else(|| {
                    Arc::new(|name| wine_process_running(Path::new("/proc"), name))
                }),
                watch_interval: options.watch_interval.unwrap_or(WATCH_INTERVAL),
                stop_timeout: options.stop_timeout.unwrap_or(STOP_WAIT),
                running: Mutex::new(HashMap::new()),
                next_token: AtomicU64::new(1),
                changed: Notify::new(),
                listeners: Mutex::new((0, Vec::new())),
            }),
        }
    }

    /// `on("state", listener)`: смена состояния инструмента; вызов — сразу, в потоке, где она
    /// произошла, без блокировок (подписчик может спрашивать [`ToolRunner::running`]).
    pub fn subscribe(&self, listener: StateListener) -> ListenerId {
        let mut listeners = self.inner.listeners();
        listeners.0 += 1;
        let id = listeners.0;
        listeners.1.push((id, listener));
        ListenerId(id)
    }

    /// `off`.
    pub fn unsubscribe(&self, id: ListenerId) {
        self.inner
            .listeners()
            .1
            .retain(|(listener, _)| *listener != id.0);
    }
}

impl Inner {
    fn listeners(&self) -> std::sync::MutexGuard<'_, (u64, Vec<(u64, StateListener)>)> {
        self.listeners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn records(&self) -> std::sync::MutexGuard<'_, HashMap<String, Record>> {
        self.running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `emit("state", toolId)`.
    fn emit_state(&self, tool_id: &str) {
        let listeners: Vec<StateListener> = self
            .listeners()
            .1
            .iter()
            .map(|(_, listener)| Arc::clone(listener))
            .collect();
        for listener in listeners {
            listener(tool_id);
        }
    }

    /// Запись `tool_id` — ещё тот самый запуск.
    fn is_current(&self, tool_id: &str, token: u64) -> bool {
        self.records()
            .get(tool_id)
            .is_some_and(|record| record.token == token)
    }

    /// `finished` из `track`: процесс вышел (или не запустился) — инструмент больше не открыт.
    fn finished(&self, tool_id: &str, token: u64) {
        {
            let mut records = self.records();
            if records.get(tool_id).map(|record| record.token) != Some(token) {
                return;
            }
            records.remove(tool_id);
        }
        self.changed.notify_waiters();
        self.emit_state(tool_id);
    }

    /// `track(toolId, child, info)`: запомнить запущенный инструмент до выхода процесса.
    /// `wait` — выход (или ошибка) процесса; `pid` `None` — процесс не запустился, и его
    /// `error` приходит сразу.
    fn track(
        self: &Arc<Self>,
        tool_id: &str,
        pid: Option<u32>,
        wait: BoxFuture<'static, ChildEnd>,
        game_id: Option<String>,
        splits: Option<String>,
    ) -> u64 {
        let token = self.next_token.fetch_add(1, Ordering::SeqCst);
        self.records().insert(
            tool_id.to_owned(),
            Record {
                token,
                pid,
                stopping: false,
                game_id,
                splits,
            },
        );
        // Сначала «запущен», потом ожидание выхода: у Node `exit` и `error` приходят позже.
        self.emit_state(tool_id);
        let inner = Arc::clone(self);
        let tool = tool_id.to_owned();
        tokio::spawn(async move {
            let _ = wait.await;
            inner.finished(&tool, token);
        });
        token
    }

    /// `launchDirect(toolId, exe, root, args, info)` (Windows).
    fn launch_direct(self: &Arc<Self>, request: &ToolLaunch) -> Result<Option<u32>, Failure> {
        let mut command = tokio::process::Command::new(&request.exe);
        command
            .args(&request.args)
            .current_dir(&request.root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        strip_ui_environment(&mut command);
        detach(&mut command);

        // Не запустился: у `ChildProcess` Node нет PID, а `error` приходит сразу следом.
        let handle = command.spawn().ok().and_then(ChildHandle::from_tokio);
        let pid = handle.as_ref().map(|handle| handle.pid);
        let wait: BoxFuture<'static, ChildEnd> = match handle {
            Some(handle) => handle.wait,
            None => Box::pin(std::future::ready(ChildEnd::Error("spawn".to_owned()))),
        };
        self.track(
            &request.tool_id,
            pid,
            wait,
            request.game_id.clone(),
            request.splits.clone(),
        );
        self.logger.info(&format!(
            "[tools] {} запущен: {} [{}]",
            request.tool_id,
            request.exe.display(),
            request.args.join(" ")
        ));
        Ok(pid)
    }

    /// Linux: `protonRunner.launch` и слежение за процессом Wine.
    async fn launch_proton(self: &Arc<Self>, request: &ToolLaunch) -> Result<Option<u32>, Failure> {
        let proton = self.proton.as_ref().ok_or_else(proton_not_configured)?;
        let options = PrefixLaunchOptions {
            cwd: request.root.clone(),
            font_profile: request.font_profile.clone(),
            env: request.env.clone(),
        };
        let launched = proton
            .launch(
                request.game_id.as_deref(),
                &request.exe,
                &request.args,
                &options,
            )
            .await?;
        let pid = launched.child.as_ref().map(|child| child.pid);
        if let Some(child) = launched.child {
            let token = self.track(
                &request.tool_id,
                Some(child.pid),
                child.wait,
                request.game_id.clone(),
                request.splits.clone(),
            );
            self.watch_wine_process(&request.tool_id, token, &request.executable);
        }
        self.logger.info(&format!(
            "[tools] {} запущен в префиксе {}: {}",
            request.tool_id,
            launched.prefix.display(),
            request.exe.display()
        ));
        Ok(pid)
    }

    /// `watchWineProcess(toolId, exeName)`. Сессия `umu` не завершается, пока в префиксе
    /// работает игра, поэтому LiveSplit, закрытый своим крестиком или «Закрыть» в меню, лаунчер
    /// считал бы открытым. Следим за самим процессом exe: пропал два опроса подряд — инструмент
    /// закрыт, сессия `umu` закрывается вместе с ним.
    fn watch_wine_process(self: &Arc<Self>, tool_id: &str, token: u64, exe_name: &str) {
        if exe_name.is_empty() {
            return;
        }
        let inner = Arc::clone(self);
        let tool = tool_id.to_owned();
        let exe = exe_name.to_owned();
        tokio::spawn(async move {
            let mut seen = false;
            let mut missing = 0_u32;
            let pid = loop {
                tokio::time::sleep(inner.watch_interval).await;
                if !inner.is_current(&tool, token) {
                    return;
                }
                let Some(alive) = (inner.process_running)(&exe) else {
                    continue;
                };
                if alive {
                    seen = true;
                    missing = 0;
                    continue;
                }
                // Пока Wine поднимает префикс, exe ещё нет: ждём его появления.
                let (pid, stopping) = match inner.records().get(&tool) {
                    Some(record) if record.token == token => (record.pid, record.stopping),
                    _ => return,
                };
                if !seen || stopping {
                    continue;
                }
                missing += 1;
                if missing >= 2 {
                    break pid;
                }
            };
            inner
                .logger
                .info(&format!("[tools] {tool}: {exe} закрыт вне лаунчера"));
            inner.records().remove(&tool);
            if let Some(pid) = pid.filter(|pid| *pid > 0) {
                // Сессия уже могла завершиться.
                let _ = inner.stopper.graceful_group(pid).await;
            }
            inner.changed.notify_waiters();
            inner.emit_state(&tool);
        });
    }

    /// `stop(toolId, options)`.
    async fn stop(&self, tool_id: &str, force: bool) -> Result<StopStatus, Failure> {
        let (token, pid) = {
            let mut records = self.records();
            let Some(record) = records.get_mut(tool_id) else {
                return Ok(StopStatus::NotRunning);
            };
            if record.stopping {
                return Ok(StopStatus::Stopping);
            }
            let Some(pid) = record.pid.filter(|pid| *pid > 0) else {
                return Err(DeliveryError::new(
                    Code::ProcessNotFound,
                    "Процесс инструмента не найден",
                )
                .into());
            };
            record.stopping = true;
            (record.token, pid)
        };
        self.emit_state(tool_id);

        let failed = |message: String| -> Failure {
            if let Some(record) = self.records().get_mut(tool_id)
                && record.token == token
            {
                record.stopping = false;
            }
            self.emit_state(tool_id);
            DeliveryError::new(
                Code::StopFailed,
                format!("Не удалось закрыть инструмент: {message}"),
            )
            .into()
        };

        if self.platform == Platform::Win32 {
            // Без /F taskkill просит окно закрыться (LiveSplit успеет спросить про сплиты), с /F
            // снимает процесс. /T — вместе с дочерними.
            let result = if force {
                self.stopper.force(pid).await
            } else {
                self.stopper.graceful(pid).await
            };
            if !result.ok && self.is_current(tool_id, token) {
                let message = if result.output.is_empty() {
                    format!("taskkill {}", result.code.as_deref().unwrap_or("undefined"))
                } else {
                    result.output
                };
                return Err(failed(message));
            }
        } else {
            let result = if force {
                self.stopper.force_group(pid).await
            } else {
                self.stopper.graceful_group(pid).await
            };
            if !result.ok {
                return Err(failed(result.output));
            }
        }

        let deadline = Instant::now() + self.stop_timeout;
        let stopped = loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.is_current(tool_id, token) {
                break true;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                break false;
            }
        };
        if stopped || !self.is_current(tool_id, token) {
            return Ok(StopStatus::Stopped);
        }
        if let Some(record) = self.records().get_mut(tool_id)
            && record.token == token
        {
            record.stopping = false;
        }
        self.emit_state(tool_id);
        Ok(StopStatus::Timeout)
    }
}

/// `detached: true`: отдельная группа процессов, переживает лаунчер; окно не прячется
/// (`windowsHide: false`).
fn detach(command: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        command.process_group(0);
    }
}

impl ToolRunner for ToolProcessRunner {
    fn running(&self, tool_id: &str) -> Option<RunningTool> {
        self.inner.records().get(tool_id).map(|record| RunningTool {
            game_id: record.game_id.clone(),
            splits: record.splits.clone(),
            stopping: record.stopping,
        })
    }

    fn launch<'a>(
        &'a self,
        request: &'a ToolLaunch,
    ) -> BoxFuture<'a, Result<Option<u32>, Failure>> {
        Box::pin(async move {
            if self.inner.platform == Platform::Win32 {
                self.inner.launch_direct(request)
            } else {
                self.inner.launch_proton(request).await
            }
        })
    }

    fn stop<'a>(
        &'a self,
        tool_id: &'a str,
        force: bool,
    ) -> BoxFuture<'a, Result<StopStatus, Failure>> {
        Box::pin(self.inner.stop(tool_id, force))
    }
}
