//! Помощник рендера по-настоящему: заглушки игры (`PING.EXE`, `cmd.exe`) на своём скрытом
//! рабочем столе — на экран ничего не выводится. Игра закрывается заданием: по `stop`, при
//! закрытии канала (лаунчер бросил помощник) и при гибели самого процесса лаунчера
//! (`KILL_ON_JOB_CLOSE`).

#![cfg(windows)]

use hlsr_core::logger::MemoryLogger;
use hlsr_demo::render_host::{Host, HostEvent, RenderHost, StartRequest};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

fn system32(name: &str) -> PathBuf {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    PathBuf::from(root).join("System32").join(name)
}

/// Заглушка игры: `PING.EXE` на `seconds` секунд.
fn ping(seconds: u32) -> StartRequest {
    let exe = system32("PING.EXE");
    StartRequest {
        command_line: format!("\"{}\" -n {seconds} 127.0.0.1", exe.display()),
        cwd: exe.parent().unwrap().to_path_buf(),
        exe,
        mute: true,
        ..StartRequest::default()
    }
}

fn cmd(args: &str) -> StartRequest {
    let exe = system32("cmd.exe");
    StartRequest {
        command_line: format!("\"{}\" {args}", exe.display()),
        cwd: exe.parent().unwrap().to_path_buf(),
        exe,
        ..StartRequest::default()
    }
}

/// Свой дескриптор процесса (по нему ждём выхода: PID мог бы достаться другому).
struct Watched(HANDLE);

impl Watched {
    fn open(pid: u32) -> Watched {
        // SAFETY: только ожидание и запрос сведений.
        let handle = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        assert!(!handle.is_null(), "процесс {pid} не открыт");
        Watched(handle)
    }

    fn exited_within(&self, ms: u32) -> bool {
        // SAFETY: дескриптор открыт.
        unsafe { WaitForSingleObject(self.0, ms) == WAIT_OBJECT_0 }
    }
}

impl Drop for Watched {
    fn drop(&mut self) {
        // SAFETY: дескриптор открыт нами.
        unsafe { CloseHandle(self.0) };
    }
}

/// Дочерние процессы `parent` с образом `image`.
fn children(parent: u32, image: &str) -> Vec<u32> {
    // SAFETY: снимок процессов системы.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    // SAFETY: структура из чисел.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut out = Vec::new();
    // SAFETY: снимок открыт.
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        let len = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(0);
        let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
        if entry.th32ParentProcessID == parent && name.eq_ignore_ascii_case(image) {
            out.push(entry.th32ProcessID);
        }
        // SAFETY: снимок открыт.
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    // SAFETY: снимок открыт нами.
    unsafe { CloseHandle(snapshot) };
    out
}

fn host() -> RenderHost {
    RenderHost::new(Arc::new(MemoryLogger::default()))
}

fn wait_until(timeout: Duration, done: impl Fn() -> bool) -> bool {
    let until = Instant::now() + timeout;
    while Instant::now() < until {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    done()
}

#[test]
fn stop_closes_the_game_on_the_hidden_desktop_through_its_job() {
    let mut host = host();
    let pid = host.start(&ping(60)).unwrap();
    assert!(pid > 0);
    let game = Watched::open(pid);
    let events = host.events();
    match &events[0] {
        HostEvent::Started {
            pid: started,
            desktop,
        } => {
            assert_eq!(*started, pid);
            assert!(desktop.starts_with("hlsr-render-"), "{desktop}");
        }
        other => panic!("первым — Started, а не {other:?}"),
    }
    // Игра живёт, пока её не остановили; звук ping не открывает — глушить нечего.
    assert!(!game.exited_within(700));
    assert_eq!(host.exit_code(), None);
    host.stop(true);
    assert!(game.exited_within(5000), "игра закрыта заданием");
    let events = host.events();
    assert_eq!(
        events.last(),
        Some(&HostEvent::Stopped {
            code: Some(1),
            alone: false,
            pending: false
        }),
        "{events:?}"
    );
    assert!(
        events.iter().all(|e| !matches!(e, HostEvent::Error { .. })),
        "{events:?}"
    );
    assert_eq!(host.pending(), Some(false));
    assert_eq!(host.audio_reset(), None, "глушили не мы — сброса нет");
    assert!(wait_until(Duration::from_secs(5), || host.closed()));
}

#[test]
fn a_game_that_exits_by_itself_is_reported_with_its_code() {
    let mut host = host();
    host.start(&cmd("/c exit 7")).unwrap();
    assert!(wait_until(Duration::from_secs(10), || host.closed()));
    assert_eq!(host.exit_code(), Some(7));
    let events = host.events();
    assert!(
        events.contains(&HostEvent::Exited { code: 7 }),
        "{events:?}"
    );
    assert_eq!(
        events.last(),
        Some(&HostEvent::Stopped {
            code: Some(7),
            alone: true,
            pending: false
        })
    );
    // Остановка после выхода — пустая.
    host.stop(true);
}

#[test]
fn follow_job_keeps_the_game_alive_after_the_launcher_process_exits() {
    // Посредник (как Injector HL1): cmd запускает ping и выходит сам, игра — в задании.
    let mut host = host();
    let mut request = cmd("/c start \"\" /b PING.EXE -n 60 127.0.0.1");
    request.follow_job = true;
    let pid = host.start(&request).unwrap();
    assert!(wait_until(Duration::from_secs(10), || !children(
        pid, "PING.EXE"
    )
    .is_empty()));
    let game = Watched::open(children(pid, "PING.EXE")[0]);
    assert!(Watched::open(pid).exited_within(10_000), "посредник вышел");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(host.exit_code(), None, "игра в задании ещё жива");
    assert!(!host.closed());
    host.stop(false);
    assert!(game.exited_within(5000));
    assert!(matches!(
        host.events().last(),
        Some(HostEvent::Stopped { alone: false, .. })
    ));
}

#[test]
fn a_foreign_process_with_the_watched_image_is_reported_once() {
    use std::os::windows::process::CommandExt;
    // Чужая «игра» на том же образе — свой процесс теста, без окна.
    let mut outside = Command::new(system32("PING.EXE"))
        .args(["-n", "30", "127.0.0.1"])
        .stdout(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .unwrap();
    let mut host = host();
    let mut request = ping(30);
    request.watch_image = Some("ping".into());
    let pid = host.start(&request).unwrap();
    let mut seen = Vec::new();
    let outside_id = outside.id();
    for _ in 0..60 {
        seen.extend(host.events());
        if seen.contains(&HostEvent::Foreign { pid: outside_id }) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Ещё круг проверки: повтора нет.
    std::thread::sleep(Duration::from_millis(1200));
    seen.extend(host.events());
    host.stop(true);
    outside.kill().unwrap();
    outside.wait().unwrap();
    let foreign: Vec<u32> = seen
        .iter()
        .filter_map(|e| match e {
            HostEvent::Foreign { pid } => Some(*pid),
            _ => None,
        })
        .collect();
    assert!(foreign.contains(&outside.id()), "{seen:?}");
    assert!(!foreign.contains(&pid), "своя игра — не чужая");
    assert_eq!(
        foreign.iter().filter(|p| **p == outside.id()).count(),
        1,
        "один раз"
    );
}

#[test]
fn dropping_the_host_closes_the_game() {
    let mut host = host();
    let pid = host.start(&ping(60)).unwrap();
    let game = Watched::open(pid);
    drop(host);
    assert!(
        game.exited_within(10_000),
        "закрытый канал — то же, что stop"
    );
}

#[test]
fn reset_only_without_audio_sessions_leaves_mute_pending() {
    let mut host = host();
    let mut request = cmd("/c exit 0");
    request.reset_only = true;
    request.reset_command_line = request.command_line.clone();
    assert_eq!(host.start(&request).unwrap(), 0);
    host.stop(true);
    let events = host.events();
    assert!(
        events.contains(&HostEvent::AudioReset { ok: false }),
        "{events:?}"
    );
    assert_eq!(host.audio_reset(), Some(false));
    assert_eq!(host.pending(), Some(true));
}

#[test]
fn a_missing_game_fails_the_start() {
    let mut host = host();
    let mut request = ping(1);
    request.exe = system32("hlsr-no-such-game.exe");
    let error = host.start(&request).unwrap_err();
    assert_eq!(error.code, "RENDER_HOST_FAILED");
    assert!(
        error.message.starts_with("CreateProcessW: "),
        "{}",
        error.message
    );
}

/// Процесс «лаунчера» для [`the_game_dies_with_the_launcher_process`]: запускает игру и ждёт,
/// пока его убьют. Без переменной окружения — ничего не делает.
#[test]
fn launcher_child() {
    if std::env::var_os("HLSR_DEMO_LAUNCHER_CHILD").is_none() {
        return;
    }
    let mut host = host();
    let pid = host.start(&ping(120)).unwrap();
    println!("GAME {pid}");
    std::thread::sleep(Duration::from_secs(120));
}

#[test]
fn the_game_dies_with_the_launcher_process() {
    // Лаунчер упал (его процесс убит): поток помощника не успевает ничего, Windows закрывает
    // задание (KILL_ON_JOB_CLOSE) — игра закрывается вместе с ним.
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "launcher_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("HLSR_DEMO_LAUNCHER_CHILD", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut game = None;
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        // libtest печатает «test launcher_child ... » без перевода строки — PID в той же строке.
        if let Some(pid) = line.unwrap().split("GAME ").nth(1) {
            game = Some(Watched::open(pid.trim().parse().unwrap()));
            break;
        }
    }
    let game = game.expect("лаунчер запустил игру");
    assert!(!game.exited_within(300));
    // Свой дочерний процесс — по дескриптору.
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(
        game.exited_within(10_000),
        "игра закрыта вместе с лаунчером"
    );
}
