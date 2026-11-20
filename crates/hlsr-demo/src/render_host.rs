//! Помощник рендера (`render-host.js` и `resources/demo-render/render-host.ps1` 0.4.9) без
//! PowerShell: свой скрытый рабочий стол (`CreateDesktopW`) — окно игры не появляется на экране
//! игрока; игра на нём (`CreateProcessW` с `lpDesktop`) сразу кладётся в объект задания с
//! `KILL_ON_JOB_CLOSE`; глушатся звуковые сессии только процессов задания
//! (`IAudioSessionManager2` → `ISimpleAudioVolume`). Живую игру не разглушаем никогда: она
//! закрывается заглушённой, а mute, который Windows запоминает по пути exe, снимает короткий
//! беззвучный (`+volume 0`) перезапуск той же игры в конце рендера. Событие `Stopped` сообщает,
//! остался ли mute (`pending`).
//!
//! Помощник — поток лаунчера, а не процесс: команда остановки идёт по каналу, события — в
//! очередь (`Host::events`), итог (PID, код выхода, `pending`, сброс mute) — в общее состояние.
//! Лаунчер упал — Windows сама закрывает задание (игра закрывается) и стол; mute, возможно,
//! остался — его помнит маркер (`game_render::GameRender::recover`). Игра закрывается только
//! `TerminateJobObject` своего задания.

#![cfg_attr(not(windows), allow(dead_code))]

use crate::errors::DemoError;
use hlsr_core::logger::Logger;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const READY_TIMEOUT: Duration = Duration::from_secs(60);
/// Обычно 1–5 с (закрытие и беззвучный перезапуск-сброс mute); сброс ждёт сессий до 30 с.
const STOP_TIMEOUT: Duration = Duration::from_secs(60);

/// Окружение игры: без моста Discord (`HLSR_PRESENCE_PIPE` и прочие `HLSR_*`), переменных
/// Slint/winit/Electron/Node и Steam-переменных запуска из Steam (`STRIPPED_ENV`, без учёта
/// регистра).
pub fn stripped(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    ["HLSR_", "SLINT_", "WINIT_", "ELECTRON_"]
        .iter()
        .any(|prefix| upper.starts_with(prefix))
        || [
            "NODE_OPTIONS",
            "STEAMAPPID",
            "STEAMGAMEID",
            "STEAMOVERLAYGAMEID",
            "STEAMAPPUSER",
        ]
        .contains(&upper.as_str())
}

/// `childEnv(env)`: окружение без [`stripped`] переменных.
pub fn child_env(env: impl IntoIterator<Item = (String, String)>) -> Vec<(String, String)> {
    env.into_iter()
        .filter(|(name, _)| !stripped(name))
        .collect()
}

/// Что запустить (`start(request)` 0.4.9).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StartRequest {
    pub exe: PathBuf,
    pub cwd: PathBuf,
    /// Командная строка целиком (`commandLine`), первым — exe в кавычках.
    pub command_line: String,
    /// Глушить звуковые сессии процессов задания.
    pub mute: bool,
    /// Заглушённые до нас сессии — тоже свои: mute прошлой пачки или сбоя.
    pub unmute_all: bool,
    /// Беззвучный перезапуск, снимающий запомненный mute; пусто — без сброса.
    pub reset_command_line: String,
    /// Чем сбрасывать mute, если игру запускает посредник (HL1: Injector); нет — `exe`.
    pub reset_exe: Option<PathBuf>,
    /// Запущенный процесс — посредник: игра жива, пока в задании есть процессы.
    pub follow_job: bool,
    /// Имя образа без `.exe` (HL1: `hl`): такой процесс вне задания — событие `Foreign`.
    pub watch_image: Option<String>,
    /// Без рендера: только беззвучный сброс mute (`resetCommandLine` одним запуском).
    pub reset_only: bool,
    /// Окружение игры; `None` — окружение лаунчера. [`stripped`] переменные убираются всегда.
    pub env: Option<Vec<(String, String)>>,
}

/// События помощника (строки JSON `render-host.ps1`).
#[derive(Debug, Clone, PartialEq)]
pub enum HostEvent {
    Started {
        pid: u32,
        desktop: String,
    },
    /// Сессия процесса задания заглушена (`was` — была заглушена до нас).
    Muted {
        pid: u32,
        device: u32,
        was: bool,
    },
    Unmuted {
        pid: u32,
        device: u32,
    },
    /// Итог беззвучного перезапуска.
    AudioReset {
        ok: bool,
    },
    /// Игра вышла сама.
    Exited {
        code: u32,
    },
    /// Чужой процесс с образом `watch_image` в нашем сеансе Windows.
    Foreign {
        pid: u32,
    },
    /// Игра закрыта, стол закрыт; `pending` — mute, возможно, остался у Windows.
    Stopped {
        code: Option<u32>,
        alone: bool,
        pending: bool,
    },
    Error {
        place: String,
        code: Option<u32>,
        message: Option<String>,
    },
}

/// Помощник рендера (`RenderHost`); в тестах — подставной.
pub trait Host: Send {
    /// Запустить игру (или только сброс mute); ответ — PID запущенного процесса.
    fn start(&mut self, request: &StartRequest) -> Result<u32, DemoError>;
    /// События с прошлого вызова.
    fn events(&mut self) -> Vec<HostEvent>;
    /// Закрыть игру (заглушённой) и стол; ждёт `Stopped`. `reset` — снять запомненный mute
    /// беззвучным перезапуском (конец рендера), без него — между пачками.
    fn stop(&mut self, reset: bool);
    /// Код выхода игры, вышедшей самой (`exitCode`).
    fn exit_code(&self) -> Option<u32>;
    /// Помощник закончил работу (`closed`).
    fn closed(&self) -> bool;
    /// `pending` из `Stopped`; `None` — помощник не сообщил.
    fn pending(&self) -> Option<bool>;
    /// Итог сброса mute (`AudioReset`); `None` — сброса не было.
    fn audio_reset(&self) -> Option<bool>;
}

fn failed(message: impl Into<String>) -> DemoError {
    DemoError {
        message: message.into(),
        ..DemoError::new("RENDER_HOST_FAILED")
    }
}

#[derive(Default)]
struct State {
    events: VecDeque<HostEvent>,
    pid: Option<u32>,
    failure: Option<String>,
    exit_code: Option<u32>,
    pending: Option<bool>,
    audio_reset: Option<bool>,
    stopped: bool,
    closed: bool,
    /// Объект задания игры, пока он открыт: закрыть игру, если помощник не ответил.
    job: Option<usize>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    logger: Arc<dyn Logger>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn emit(&self, event: HostEvent) {
        // Журнал — до замка: `events()`, `exit_code()`, `closed()` не ждут записи в файл.
        let failure = match &event {
            HostEvent::Error {
                place,
                code,
                message,
            } => {
                let what = code
                    .map(|code| code.to_string())
                    .or_else(|| message.clone())
                    .unwrap_or_default();
                self.logger
                    .warn(&format!("[demo] помощник: ошибка {place} {what}"));
                Some(format!("{place}: {what}"))
            }
            _ => None,
        };
        let mut state = self.lock();
        match &event {
            HostEvent::Started { pid, .. } => state.pid = Some(*pid),
            HostEvent::Exited { code } => state.exit_code = Some(*code),
            HostEvent::AudioReset { ok } => state.audio_reset = Some(*ok),
            HostEvent::Stopped { pending, .. } => {
                state.pending = Some(*pending);
                state.stopped = true;
            }
            HostEvent::Error { .. } if state.pid.is_none() && state.failure.is_none() => {
                state.failure = failure;
            }
            _ => {}
        }
        state.events.push_back(event);
        self.changed.notify_all();
    }

    fn close(&self) {
        self.lock().closed = true;
        self.changed.notify_all();
    }

    /// Ждать, пока `done` не станет истинным; `false` — время вышло.
    fn wait(&self, timeout: Duration, done: impl Fn(&State) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        while !done(&state) {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            state = self
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        true
    }

    /// Закрыть игру через её задание (помощник не ответил). Замок держится до конца: поток
    /// помощника снимает задание из состояния под ним же, а потом закрывает.
    fn kill(&self) {
        let state = self.lock();
        if let Some(job) = state.job {
            terminate_job(job);
        }
    }
}

#[cfg(windows)]
fn terminate_job(job: usize) {
    // SAFETY: дескриптор задания открыт, пока он записан в состоянии (см. `Shared::kill`).
    unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(job as _, 1) };
}

#[cfg(not(windows))]
fn terminate_job(_job: usize) {}

/// Помощник рендера: поток со скрытым столом, заданием и звуком (Windows).
pub struct RenderHost {
    shared: Arc<Shared>,
    commands: Option<Sender<bool>>,
    started: bool,
}

impl RenderHost {
    pub fn new(logger: Arc<dyn Logger>) -> RenderHost {
        RenderHost {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                changed: Condvar::new(),
                logger,
            }),
            commands: None,
            started: false,
        }
    }
}

impl Host for RenderHost {
    fn start(&mut self, request: &StartRequest) -> Result<u32, DemoError> {
        if self.started {
            return Err(failed("Помощник уже запущен"));
        }
        self.started = true;
        let (sender, receiver) = mpsc::channel();
        self.commands = Some(sender);
        let shared = Arc::clone(&self.shared);
        let request = request.clone();
        std::thread::Builder::new()
            .name("hlsr-render-host".into())
            .spawn(move || {
                run(&request, &shared, &receiver);
                shared.close();
            })
            .map_err(|error| failed(error.to_string()))?;
        let ready = self.shared.wait(READY_TIMEOUT, |state| {
            state.pid.is_some() || state.failure.is_some() || state.closed
        });
        let state = self.shared.lock();
        if let Some(pid) = state.pid {
            return Ok(pid);
        }
        if let Some(failure) = state.failure.clone() {
            return Err(failed(failure));
        }
        drop(state);
        if !ready {
            self.shared.kill();
            return Err(failed("Помощник не ответил"));
        }
        Err(failed("Помощник вышел"))
    }

    fn events(&mut self) -> Vec<HostEvent> {
        self.shared.lock().events.drain(..).collect()
    }

    fn stop(&mut self, reset: bool) {
        if let Some(commands) = self.commands.take() {
            // Канал закрывается сразу за командой: помощник закончит и без нас.
            let _ = commands.send(reset);
        }
        if !self.started {
            return;
        }
        if !self
            .shared
            .wait(STOP_TIMEOUT, |state| state.stopped || state.closed)
        {
            self.shared.kill();
        }
    }

    fn exit_code(&self) -> Option<u32> {
        self.shared.lock().exit_code
    }

    fn closed(&self) -> bool {
        self.shared.lock().closed
    }

    fn pending(&self) -> Option<bool> {
        self.shared.lock().pending
    }

    fn audio_reset(&self) -> Option<bool> {
        self.shared.lock().audio_reset
    }
}

// ---------------------------------------------------------------- звук

/// Звуковые сессии всех активных устройств вывода: игра открывает сессии не только на
/// устройстве по умолчанию. `visit(pid, устройство, заглушена)` — новое значение mute или
/// ничего. Системная громкость и чужие сессии не трогаются.
trait Audio {
    fn visit(&self, visit: &mut dyn FnMut(u32, u32, bool) -> Option<bool>) -> Result<(), String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Заглушить и запомнить свои.
    Mute,
    /// Снять mute с любых сессий этих PID (сброс).
    UnmuteAny,
}

/// Учёт сессий помощника (`MutedByUs`, `SeenSessions`, `ResetSeen`).
#[derive(Debug, Default)]
struct Book {
    /// Сессии (pid, устройство), заглушённые нами: снимаем глушение только с них.
    muted_by_us: HashSet<(u32, u32)>,
    seen: HashSet<(u32, u32)>,
    /// Устройства, на которых нашлись сессии перезапуска-сброса.
    reset_seen: HashSet<u32>,
}

impl Book {
    /// `Sessions(pids, mode, unmuteAll)`: сколько сессий у этих PID.
    fn sessions(
        &mut self,
        audio: &dyn Audio,
        pids: &[u32],
        mode: Mode,
        unmute_all: bool,
        emit: &mut dyn FnMut(HostEvent),
    ) -> Result<usize, String> {
        let mut found = 0;
        audio.visit(&mut |pid, device, was| {
            if !pids.contains(&pid) {
                return None;
            }
            found += 1;
            if mode == Mode::UnmuteAny {
                self.reset_seen.insert(device);
            }
            let key = (pid, device);
            let first = self.seen.insert(key);
            match mode {
                Mode::Mute => {
                    let set = if !was {
                        self.muted_by_us.insert(key);
                        Some(true)
                    } else {
                        if first && unmute_all {
                            self.muted_by_us.insert(key);
                        }
                        None
                    };
                    if first {
                        emit(HostEvent::Muted { pid, device, was });
                    }
                    set
                }
                Mode::UnmuteAny if was => {
                    self.muted_by_us.remove(&key);
                    emit(HostEvent::Unmuted { pid, device });
                    Some(false)
                }
                Mode::UnmuteAny => None,
            }
        })?;
        Ok(found)
    }

    /// Устройства, где глушили сами.
    fn devices(&self) -> HashSet<u32> {
        self.muted_by_us.iter().map(|(_, device)| *device).collect()
    }

    /// Mute, возможно, остался: свои заглушённые сессии или (unmuteAll) mute прошлой пачки или
    /// сбоя, а сессий игра так и не открыла.
    fn pending(&self, unmute_all: bool) -> bool {
        !self.muted_by_us.is_empty() || (unmute_all && self.seen.is_empty())
    }
}

/// Сброс mute закончен: перезапуск дал сессии на всех устройствах, где глушили (`devices`), или
/// хотя бы одну, если какие — уже неизвестно (`None`).
fn reset_done(launched: bool, devices: Option<&HashSet<u32>>, seen: &HashSet<u32>) -> bool {
    launched
        && match devices {
            None => !seen.is_empty(),
            Some(devices) => !devices.is_empty() && devices.is_subset(seen),
        }
}

// ---------------------------------------------------------------- поток помощника

#[cfg(not(windows))]
fn run(_request: &StartRequest, shared: &Shared, _commands: &Receiver<bool>) {
    shared.emit(HostEvent::Error {
        place: "platform".into(),
        code: None,
        message: Some("RENDER_NOT_READY".into()),
    });
}

#[cfg(windows)]
fn run(request: &StartRequest, shared: &Shared, commands: &Receiver<bool>) {
    win::run(request, shared, commands);
}

#[cfg(windows)]
mod win {
    use super::{Audio, Book, HostEvent, Mode, Shared, StartRequest, child_env, reset_done};
    use hlsr_core::win32::wide;
    use std::collections::HashSet;
    use std::ffi::c_void;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc::{Receiver, RecvTimeoutError};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_ALL, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation, QueryInformationJobObject,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, CreateDesktopW, HDESK};
    use windows_sys::Win32::System::Threading::{
        CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, GetCurrentProcessId,
        GetExitCodeProcess, PROCESS_INFORMATION, ResumeThread, STARTF_USESHOWWINDOW,
        STARTF_USESTDHANDLES, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
    };

    const POLL: Duration = Duration::from_millis(50);
    const WATCH_EVERY: Duration = Duration::from_secs(1);
    const RESET_WAIT: Duration = Duration::from_secs(30);
    const RESET_AFTER_FIRST: Duration = Duration::from_millis(1500);
    /// Ни один процесс задания не держится дольше: при пустом списке (запрос не ответил) — тоже.
    const MAX_PIDS: usize = 256;

    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: дескриптор получен от Win32 и закрывается один раз.
            unsafe { CloseHandle(self.0) };
        }
    }

    struct Desktop(HDESK);

    impl Drop for Desktop {
        fn drop(&mut self) {
            // Стол закрывается, когда его отпустили все процессы: до 10 с.
            for _ in 0..40 {
                // SAFETY: дескриптор стола получен от CreateDesktopW.
                if unsafe { CloseDesktop(self.0) } != 0 {
                    return;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    }

    struct Process {
        handle: Handle,
        pid: u32,
    }

    impl Process {
        fn exited(&self) -> bool {
            // SAFETY: дескриптор процесса открыт.
            unsafe { WaitForSingleObject(self.handle.0, 0) == WAIT_OBJECT_0 }
        }

        fn wait(&self, ms: u32) {
            // SAFETY: дескриптор процесса открыт.
            unsafe { WaitForSingleObject(self.handle.0, ms) };
        }

        fn exit_code(&self) -> u32 {
            let mut code = 0;
            // SAFETY: дескриптор процесса открыт, `code` — локальная переменная.
            unsafe { GetExitCodeProcess(self.handle.0, &mut code) };
            code
        }
    }

    fn last_error() -> u32 {
        // SAFETY: без аргументов.
        unsafe { GetLastError() }
    }

    struct Context<'a> {
        shared: &'a Shared,
        desktop: Vec<u16>,
        env: Vec<u16>,
        audio: SystemAudio,
        audio_failed: bool,
    }

    impl Context<'_> {
        fn emit(&self, event: HostEvent) {
            self.shared.emit(event);
        }

        fn error(&self, place: &str, code: u32) {
            self.emit(HostEvent::Error {
                place: place.into(),
                code: Some(code),
                message: None,
            });
        }

        /// `SafeSessions`: ошибка звука — событие (в журнал один раз), рендер идёт дальше.
        fn sessions(&mut self, book: &mut Book, pids: &[u32], mode: Mode, unmute_all: bool) {
            let shared = self.shared;
            if let Err(message) =
                book.sessions(&self.audio, pids, mode, unmute_all, &mut |e| shared.emit(e))
                && !std::mem::replace(&mut self.audio_failed, true)
            {
                self.emit(HostEvent::Error {
                    place: "audio".into(),
                    code: None,
                    message: Some(message),
                });
            }
        }

        fn new_job(&self) -> Option<Handle> {
            // SAFETY: без атрибутов и имени.
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() {
                self.error("CreateJobObjectW", last_error());
                return None;
            }
            let job = Handle(job);
            // SAFETY: структура из чисел, нули допустимы.
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            // DIE_ON_UNHANDLED_EXCEPTION: окна отчёта об ошибке на скрытом столе никто не увидит.
            limits.BasicLimitInformation.LimitFlags =
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
            // SAFETY: структура и её размер переданы вместе.
            let ok = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const c_void,
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if ok == 0 {
                self.error("SetInformationJobObject", last_error());
                return None;
            }
            Some(job)
        }

        /// Игра на скрытом столе, приостановленной — сразу в задание, потом пуск. Без задания
        /// нет гарантии закрыть игру при сбое — тогда не запускаем.
        fn launch(
            &self,
            exe: &Path,
            command_line: &str,
            cwd: &Path,
            job: &Handle,
        ) -> Option<Process> {
            let app = wide(exe);
            let mut command: Vec<u16> = command_line.encode_utf16().chain([0]).collect();
            let dir = wide(cwd);
            let mut desktop = self.desktop.clone();
            // SAFETY: структура из чисел и указателей, нули допустимы.
            let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
            startup.cb = size_of::<STARTUPINFOW>() as u32;
            startup.lpDesktop = desktop.as_mut_ptr();
            // Свои stdin/stdout/stderr игре не достаются.
            startup.dwFlags = STARTF_USESHOWWINDOW | STARTF_USESTDHANDLES;
            startup.wShowWindow = 1;
            // SAFETY: см. выше.
            let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
            // SAFETY: строки завершены нулём и живут до конца вызова, окружение — блок UTF-16
            // с двумя нулями в конце.
            let ok = unsafe {
                CreateProcessW(
                    app.as_ptr(),
                    command.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
                    self.env.as_ptr() as *const c_void,
                    dir.as_ptr(),
                    &startup,
                    &mut info,
                )
            };
            if ok == 0 {
                self.error("CreateProcessW", last_error());
                return None;
            }
            let process = Handle(info.hProcess);
            let thread = Handle(info.hThread);
            // SAFETY: оба дескриптора открыты.
            if unsafe { AssignProcessToJobObject(job.0, process.0) } == 0 {
                self.error("AssignProcessToJobObject", last_error());
                // SAFETY: процесс приостановлен и наш.
                unsafe { TerminateProcess(process.0, 1) };
                return None;
            }
            // SAFETY: поток приостановлен нами.
            if unsafe { ResumeThread(thread.0) } == u32::MAX {
                self.error("ResumeThread", last_error());
                // SAFETY: процесс приостановлен и наш.
                unsafe { TerminateProcess(process.0, 1) };
                return None;
            }
            Some(Process {
                handle: process,
                pid: info.dwProcessId,
            })
        }

        /// `ResetMute`: игра закрыта заглушённой — Windows запомнил mute для этого exe. Короткий
        /// запуск той же игры (в самой игре звука нет, `+volume 0`) на том же столе: дождаться
        /// её сессий на всех устройствах, где глушили (или ещё 1,5 с после первой), снять
        /// глушение, закрыть. Ответ — сброс не подтверждён.
        fn reset_mute(
            &mut self,
            book: &mut Book,
            exe: &Path,
            command_line: &str,
            cwd: &Path,
            devices: Option<&HashSet<u32>>,
        ) -> bool {
            book.reset_seen.clear();
            let Some(job) = self.new_job() else {
                self.emit(HostEvent::AudioReset { ok: false });
                return true;
            };
            let process = self.launch(exe, command_line, cwd, &job);
            if let Some(process) = &process {
                let pids = [process.pid];
                let deadline = Instant::now() + RESET_WAIT;
                let mut first_seen: Option<Instant> = None;
                while Instant::now() < deadline && !process.exited() {
                    let shared = self.shared;
                    let found = book
                        .sessions(&self.audio, &pids, Mode::UnmuteAny, false, &mut |e| {
                            shared.emit(e)
                        })
                        .unwrap_or(0);
                    if found > 0 && first_seen.is_none() {
                        first_seen = Some(Instant::now());
                    }
                    let covered =
                        devices.is_some_and(|devices| devices.is_subset(&book.reset_seen));
                    if covered || first_seen.is_some_and(|at| at.elapsed() > RESET_AFTER_FIRST) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                std::thread::sleep(Duration::from_millis(300));
                // SAFETY: задание открыто.
                unsafe { TerminateJobObject(job.0, 1) };
                process.wait(5000);
            }
            let done = reset_done(process.is_some(), devices, &book.reset_seen);
            self.emit(HostEvent::AudioReset { ok: done });
            !done
        }
    }

    fn job_pids(job: &Handle) -> Vec<u32> {
        // JOBOBJECT_BASIC_PROCESS_ID_LIST: два u32, затем PID по usize.
        let mut buffer = vec![0usize; 1 + MAX_PIDS];
        // SAFETY: буфер и его размер в байтах переданы вместе.
        let ok = unsafe {
            QueryInformationJobObject(
                job.0,
                JobObjectBasicProcessIdList,
                buffer.as_mut_ptr() as *mut c_void,
                (buffer.len() * size_of::<usize>()) as u32,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Vec::new();
        }
        // SAFETY: в начале буфера — два u32 заголовка.
        let count = unsafe { *(buffer.as_ptr() as *const u32).add(1) } as usize;
        buffer[1..1 + count.min(MAX_PIDS)]
            .iter()
            .map(|pid| *pid as u32)
            .collect()
    }

    fn session_of(pid: u32) -> Option<u32> {
        let mut session = 0;
        // SAFETY: `session` — локальная переменная.
        (unsafe { ProcessIdToSessionId(pid, &mut session) } != 0).then_some(session)
    }

    /// `GetProcessesByName(image)`: PID процессов с образом `<image>.exe` (без учёта регистра).
    fn processes_by_name(image: &str) -> Vec<u32> {
        // SAFETY: снимок процессов системы.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Vec::new();
        }
        let snapshot = Handle(snapshot);
        let want = format!("{image}.exe").to_lowercase();
        // SAFETY: структура из чисел, нули допустимы.
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut out = Vec::new();
        // SAFETY: снимок открыт, `entry.dwSize` задан.
        let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
        while more {
            let len = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(0);
            if String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase() == want {
                out.push(entry.th32ProcessID);
            }
            // SAFETY: см. выше.
            more = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
        }
        out
    }

    /// Блок окружения для `CreateProcessW`: `имя=значение\0…\0`, по имени без учёта регистра.
    fn environment_block(env: Option<&[(String, String)]>) -> Vec<u16> {
        let source: Vec<(String, String)> = match env {
            Some(env) => env.to_vec(),
            None => std::env::vars_os()
                .filter_map(|(name, value)| {
                    Some((name.into_string().ok()?, value.into_string().ok()?))
                })
                .collect(),
        };
        let mut vars = child_env(source);
        vars.sort_by_key(|(name, _)| name.to_uppercase());
        let mut block: Vec<u16> = Vec::new();
        for (name, value) in vars {
            block.extend(format!("{name}={value}").encode_utf16());
            block.push(0);
        }
        if block.is_empty() {
            block.push(0);
        }
        block.push(0);
        block
    }

    fn desktop_name() -> String {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        // SAFETY: без аргументов.
        let pid = unsafe { GetCurrentProcessId() };
        let tick = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u32)
            .unwrap_or(0);
        format!(
            "hlsr-render-{pid}-{}-{tick}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }

    pub(super) fn run(request: &StartRequest, shared: &Shared, commands: &Receiver<bool>) {
        let _com = Com::init();
        let name = desktop_name();
        let desktop_wide = wide(&name);
        // SAFETY: имя завершено нулём, остальное — по умолчанию.
        let raw = unsafe {
            CreateDesktopW(
                desktop_wide.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                GENERIC_ALL,
                std::ptr::null(),
            )
        };
        let mut cx = Context {
            shared,
            desktop: desktop_wide,
            env: environment_block(request.env.as_deref()),
            audio: SystemAudio,
            audio_failed: false,
        };
        if raw.is_null() {
            cx.error("CreateDesktopW", last_error());
            return;
        }
        let desktop = Desktop(raw);
        let mut book = Book::default();
        // Сброс mute запускает саму игру, даже если её запускает посредник (HL1: Injector).
        let reset_exe = request.reset_exe.as_deref().unwrap_or(&request.exe);

        if request.reset_only {
            cx.emit(HostEvent::Started {
                pid: 0,
                desktop: name,
            });
            let left = cx.reset_mute(
                &mut book,
                reset_exe,
                &request.reset_command_line,
                &request.cwd,
                None,
            );
            drop(desktop);
            cx.emit(HostEvent::Stopped {
                code: Some(0),
                alone: false,
                pending: left,
            });
            return;
        }

        let Some(job) = cx.new_job() else {
            return;
        };
        let Some(process) = cx.launch(&request.exe, &request.command_line, &request.cwd, &job)
        else {
            return;
        };
        shared.lock().job = Some(job.0 as usize);
        cx.emit(HostEvent::Started {
            pid: process.pid,
            desktop: name,
        });

        // SAFETY: без аргументов.
        let my_session = session_of(unsafe { GetCurrentProcessId() });
        let mut foreign_seen = HashSet::new();
        let mut last_watch: Option<Instant> = None;
        let mut stopping = false;
        // Игра всегда закрывается заглушённой. reset — снять запомненный mute беззвучным
        // перезапуском (конец рендера); без него (между пачками) следующая пачка стартует уже
        // заглушённой. Закрытый канал — тоже конец, со сбросом.
        let mut reset_after = true;
        let exited_alone;
        // Команды читаются в конце круга (ожидание 50 мс), сразу перед проверкой выхода игры.
        loop {
            if process.exited() && (!request.follow_job || job_pids(&job).is_empty()) {
                exited_alone = !stopping;
                break;
            }
            if stopping {
                exited_alone = false;
                break;
            }
            if request.mute {
                let pids = job_pids(&job);
                cx.sessions(&mut book, &pids, Mode::Mute, request.unmute_all);
            }
            if let Some(image) = request.watch_image.as_deref().filter(|i| !i.is_empty())
                && last_watch.is_none_or(|at| at.elapsed() > WATCH_EVERY)
            {
                last_watch = Some(Instant::now());
                // Сначала процессы, потом список задания: дочерний процесс игры, созданный между
                // двумя запросами, уже в задании и чужим не считается. Пустой список задания —
                // запрос не ответил (своя игра в нём есть всегда): не гадаем.
                let found = processes_by_name(image);
                let mine = job_pids(&job);
                for pid in found {
                    if !mine.is_empty()
                        && my_session.is_some()
                        && session_of(pid) == my_session
                        && !mine.contains(&pid)
                        && foreign_seen.insert(pid)
                    {
                        cx.emit(HostEvent::Foreign { pid });
                    }
                }
            }
            match commands.recv_timeout(POLL) {
                Ok(reset) => {
                    stopping = true;
                    reset_after = reset;
                }
                Err(RecvTimeoutError::Disconnected) => stopping = true,
                Err(RecvTimeoutError::Timeout) => {}
            }
        }

        let mut exit_code = 0;
        if exited_alone {
            exit_code = process.exit_code();
            cx.emit(HostEvent::Exited { code: exit_code });
        }
        // SAFETY: задание открыто.
        unsafe { TerminateJobObject(job.0, 1) };
        process.wait(5000);
        // Игра посредника должна закрыться до сброса: второй hl.exe не стартует рядом с первым.
        for _ in 0..100 {
            if !request.follow_job || job_pids(&job).is_empty() {
                break;
            }
            std::thread::sleep(POLL);
        }
        if !exited_alone {
            exit_code = process.exit_code();
        }
        let devices = book.devices();
        let mut pending = book.pending(request.unmute_all);
        // Сброс — только для устройств, где глушили сами: без них подтверждать нечего.
        if !devices.is_empty() && reset_after && !request.reset_command_line.is_empty() {
            book.muted_by_us.clear();
            book.seen.clear();
            pending = cx.reset_mute(
                &mut book,
                reset_exe,
                &request.reset_command_line,
                &request.cwd,
                Some(&devices),
            );
        }
        shared.lock().job = None;
        drop(job);
        drop(process);
        drop(desktop);
        cx.emit(HostEvent::Stopped {
            code: Some(exit_code),
            alone: exited_alone,
            pending,
        });
    }

    // ------------------------------------------------------------ COM

    struct Com(bool);

    impl Com {
        fn init() -> Com {
            use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
            // SAFETY: поток помощника, своя инициализация COM.
            Com(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
        }
    }

    impl Drop for Com {
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: парный вызов к удачному CoInitializeEx этого потока.
                unsafe { windows::Win32::System::Com::CoUninitialize() };
            }
        }
    }

    /// Сессии устройств вывода через Core Audio.
    struct SystemAudio;

    /// Контекст событий громкости: изменения mute — от помощника.
    const CONTEXT: windows::core::GUID =
        windows::core::GUID::from_u128(0x6f2c_9d41_8a3b_4e57_9c10_5d7e_a1b2_c3d4);

    impl Audio for SystemAudio {
        fn visit(
            &self,
            visit: &mut dyn FnMut(u32, u32, bool) -> Option<bool>,
        ) -> Result<(), String> {
            use windows::Win32::Media::Audio::{
                DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionManager2,
                IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, eRender,
            };
            use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
            use windows::core::Interface;
            let text = |error: windows::core::Error| error.message();
            // SAFETY: COM этого потока инициализирован (`Com::init`); интерфейсы освобождаются
            // своими Drop.
            unsafe {
                let enumerator: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(text)?;
                let devices = enumerator
                    .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
                    .map_err(text)?;
                for device in 0..devices.GetCount().map_err(text)? {
                    let manager: IAudioSessionManager2 = devices
                        .Item(device)
                        .map_err(text)?
                        .Activate(CLSCTX_ALL, None)
                        .map_err(text)?;
                    let sessions = manager.GetSessionEnumerator().map_err(text)?;
                    for k in 0..sessions.GetCount().map_err(text)? {
                        let control = sessions.GetSession(k).map_err(text)?;
                        let pid = control
                            .cast::<IAudioSessionControl2>()
                            .map_err(text)?
                            .GetProcessId()
                            .map_err(text)?;
                        let volume = control.cast::<ISimpleAudioVolume>().map_err(text)?;
                        let muted = volume.GetMute().map_err(text)?.as_bool();
                        if let Some(mute) = visit(pid, device, muted) {
                            volume.SetMute(mute, &CONTEXT).map_err(text)?;
                        }
                    }
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Сессии: (pid, устройство, заглушена).
    struct FakeAudio(RefCell<Vec<(u32, u32, bool)>>);

    impl Audio for FakeAudio {
        fn visit(
            &self,
            visit: &mut dyn FnMut(u32, u32, bool) -> Option<bool>,
        ) -> Result<(), String> {
            for session in self.0.borrow_mut().iter_mut() {
                if let Some(mute) = visit(session.0, session.1, session.2) {
                    session.2 = mute;
                }
            }
            Ok(())
        }
    }

    #[test]
    fn mute_book_keeps_own_sessions_and_pending_like_the_helper() {
        let audio = FakeAudio(RefCell::new(vec![
            (10, 0, false),
            (10, 1, true),
            (99, 0, false),
        ]));
        let mut events = Vec::new();
        let mut book = Book::default();
        let found = book
            .sessions(&audio, &[10], Mode::Mute, false, &mut |e| events.push(e))
            .unwrap();
        assert_eq!(found, 2);
        // Чужая сессия не тронута, заглушённая до нас — не своя без unmuteAll.
        assert_eq!(
            *audio.0.borrow(),
            vec![(10, 0, true), (10, 1, true), (99, 0, false)]
        );
        assert_eq!(book.devices(), HashSet::from([0]));
        assert_eq!(
            events,
            vec![
                HostEvent::Muted {
                    pid: 10,
                    device: 0,
                    was: false
                },
                HostEvent::Muted {
                    pid: 10,
                    device: 1,
                    was: true
                },
            ]
        );
        // Повторный проход: событий нет, учёт тот же.
        events.clear();
        book.sessions(&audio, &[10], Mode::Mute, false, &mut |e| events.push(e))
            .unwrap();
        assert!(events.is_empty());
        assert!(book.pending(false));

        // unmuteAll: заглушённая до нас — тоже своя.
        let mut all = Book::default();
        let audio = FakeAudio(RefCell::new(vec![(10, 1, true)]));
        all.sessions(&audio, &[10], Mode::Mute, true, &mut |_| {})
            .unwrap();
        assert_eq!(all.devices(), HashSet::from([1]));
        // Сессий не видели, а mute прошлой пачки был — он остался.
        assert!(Book::default().pending(true));
        assert!(!Book::default().pending(false));

        // Сброс: снимает mute с любых сессий этих PID и помнит устройства.
        let audio = FakeAudio(RefCell::new(vec![(20, 0, true), (20, 2, false)]));
        let mut reset = Book::default();
        let mut events = Vec::new();
        reset
            .sessions(&audio, &[20], Mode::UnmuteAny, false, &mut |e| {
                events.push(e)
            })
            .unwrap();
        assert_eq!(*audio.0.borrow(), vec![(20, 0, false), (20, 2, false)]);
        assert_eq!(events, vec![HostEvent::Unmuted { pid: 20, device: 0 }]);
        assert!(reset_done(
            true,
            Some(&HashSet::from([0, 2])),
            &reset.reset_seen
        ));
        assert!(!reset_done(
            true,
            Some(&HashSet::from([0, 3])),
            &reset.reset_seen
        ));
        assert!(!reset_done(true, Some(&HashSet::new()), &reset.reset_seen));
        assert!(reset_done(true, None, &reset.reset_seen));
        assert!(!reset_done(true, None, &HashSet::new()));
        assert!(!reset_done(false, None, &reset.reset_seen));
    }

    #[test]
    fn child_env_drops_bridge_ui_and_steam_variables() {
        let env = [
            ("PATH", "x"),
            ("HLSR_PRESENCE_PIPE", "p"),
            ("hlsr_presence_pipe", "p"),
            ("SLINT_BACKEND", "s"),
            ("WINIT_X11_SCALE_FACTOR", "1"),
            ("ELECTRON_RUN_AS_NODE", "1"),
            ("NODE_OPTIONS", "--x"),
            ("SteamAppId", "243730"),
            ("SystemRoot", "C:\\Windows"),
            ("NODE_OPTIONSX", "kept"),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned()));
        let mut names: Vec<String> = child_env(env).into_iter().map(|(k, _)| k).collect();
        names.sort();
        assert_eq!(names, ["NODE_OPTIONSX", "PATH", "SystemRoot"]);
    }
}
