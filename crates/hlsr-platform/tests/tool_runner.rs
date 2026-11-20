//! Запуск и остановка инструментов: самотесты `linux-selftest.js` (LiveSplit: группы процессов,
//! закрытый своим крестиком при живой сессии `umu`) и поведение `ToolService.js` (`track`, `stop`,
//! `watchWineProcess`) на подделках: сессия Proton, остановка, `/proc`. Ветка Linux не требует
//! системных вызовов, поэтому идёт на любой ОС. Настоящие процессы Windows —
//! `tool_runner_real.rs`.

use hlsr_core::js::Platform;
use hlsr_delivery::errors::{Failure, to_delivery_error};
use hlsr_delivery::paths::{LibraryPaths, resolve_library};
use hlsr_delivery::ports::{
    MemoryLogger, RunningTool, StopStatus, ToolLaunch, ToolRunner, UnavailableToolRunner,
};
use hlsr_delivery::providers::BoxFuture;
use hlsr_delivery::tool_service::{self, LaunchOptions, ToolOptions, ToolService};
use hlsr_platform::game_process_service::{ChildEnd, ChildHandle};
use hlsr_platform::process_stop::{StopResult, Stopper};
use hlsr_platform::tool_runner::{
    PrefixLaunchOptions, PrefixLaunched, PrefixLauncher, ProcessRunning, ToolProcessRunner,
    ToolRunnerOptions, strip_ui_environment, wine_process_running,
};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

// ---------------------------------------------------------------- подделки

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProtonCall {
    game_id: Option<String>,
    exe: PathBuf,
    args: Vec<String>,
    options: PrefixLaunchOptions,
}

/// `protonRunner`: запоминает запуски, «сессия umu» живёт, пока тест не завершит её.
struct FakeProton {
    first_pid: AtomicU32,
    calls: Mutex<Vec<ProtonCall>>,
    exits: Mutex<Vec<oneshot::Sender<ChildEnd>>>,
    prefix: PathBuf,
}

impl FakeProton {
    fn new(first_pid: u32) -> Arc<FakeProton> {
        Arc::new(FakeProton {
            first_pid: AtomicU32::new(first_pid),
            calls: Mutex::default(),
            exits: Mutex::default(),
            prefix: PathBuf::from("prefix"),
        })
    }

    /// Сессия номер `index` вышла (`child.emit("exit")`).
    fn exit(&self, index: usize, code: i64) {
        let sender = self.exits.lock().unwrap().remove(index);
        let _ = sender.send(ChildEnd::Exit {
            code: Some(code),
            signal: None,
        });
    }

    fn sessions(&self) -> usize {
        self.exits.lock().unwrap().len()
    }
}

impl PrefixLauncher for FakeProton {
    fn launch<'a>(
        &'a self,
        game_id: Option<&'a str>,
        exe: &'a Path,
        args: &'a [String],
        options: &'a PrefixLaunchOptions,
    ) -> BoxFuture<'a, Result<PrefixLaunched, Failure>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(ProtonCall {
                game_id: game_id.map(str::to_owned),
                exe: exe.to_path_buf(),
                args: args.to_vec(),
                options: options.clone(),
            });
            let (sender, receiver) = oneshot::channel();
            self.exits.lock().unwrap().push(sender);
            Ok(PrefixLaunched {
                child: Some(ChildHandle {
                    pid: self.first_pid.fetch_add(1, Ordering::SeqCst),
                    wait: Box::pin(async move {
                        receiver.await.unwrap_or(ChildEnd::Exit {
                            code: None,
                            signal: None,
                        })
                    }),
                }),
                prefix: self.prefix.clone(),
            })
        })
    }
}

type Reaction = Box<dyn Fn(&str, u32) -> StopResult + Send + Sync>;

/// Остановка: запоминает вызовы, ответ задаёт `reaction(имя, pid)`.
struct FakeStopper {
    calls: Mutex<Vec<(String, u32)>>,
    reaction: Reaction,
}

impl FakeStopper {
    fn new(reaction: impl Fn(&str, u32) -> StopResult + Send + Sync + 'static) -> Arc<FakeStopper> {
        Arc::new(FakeStopper {
            calls: Mutex::default(),
            reaction: Box::new(reaction),
        })
    }

    fn call(&self, name: &str, pid: u32) -> BoxFuture<'_, StopResult> {
        self.calls.lock().unwrap().push((name.to_owned(), pid));
        let result = (self.reaction)(name, pid);
        Box::pin(async move { result })
    }

    fn calls(&self) -> Vec<(String, u32)> {
        self.calls.lock().unwrap().clone()
    }
}

impl Stopper for FakeStopper {
    fn graceful(&self, pid: u32) -> BoxFuture<'_, StopResult> {
        self.call("graceful", pid)
    }
    fn force(&self, pid: u32) -> BoxFuture<'_, StopResult> {
        self.call("force", pid)
    }
    fn graceful_group(&self, pid: u32) -> BoxFuture<'_, StopResult> {
        self.call("graceful_group", pid)
    }
    fn force_group(&self, pid: u32) -> BoxFuture<'_, StopResult> {
        self.call("force_group", pid)
    }
}

fn linux_runner(
    proton: Option<Arc<FakeProton>>,
    stopper: Arc<FakeStopper>,
    process_running: Option<ProcessRunning>,
) -> ToolProcessRunner {
    ToolProcessRunner::new(ToolRunnerOptions {
        platform: Some(Platform::Linux),
        logger: Some(Arc::new(MemoryLogger::default())),
        stopper: Some(stopper),
        proton: proton.map(|proton| proton as Arc<dyn PrefixLauncher>),
        process_running,
        watch_interval: Some(Duration::from_millis(5)),
        stop_timeout: Some(Duration::from_millis(300)),
    })
}

fn request(tool_id: &str, root: &Path, game_id: Option<&str>) -> ToolLaunch {
    ToolLaunch {
        tool_id: tool_id.to_owned(),
        exe: root.join("LiveSplit.exe"),
        root: root.to_path_buf(),
        args: vec!["-s".to_owned(), "splits\\maps\\maps.lss".to_owned()],
        game_id: game_id.map(str::to_owned),
        splits: Some("maps".to_owned()),
        executable: "LiveSplit.exe".to_owned(),
        env: Some(vec![(
            "PROTON_USE_X11_EXCLUSIVE".to_owned(),
            "LiveSplit.exe".to_owned(),
        )]),
        font_profile: Some("livesplit".to_owned()),
    }
}

/// Установленный LiveSplit в `Tools/livesplit` библиотеки.
fn installed_livesplit(root: &Path) -> (LibraryPaths, PathBuf) {
    let paths = resolve_library(&root.join("lib").to_string_lossy()).unwrap();
    let tool = paths.tools.join("livesplit");
    std::fs::create_dir_all(&tool).unwrap();
    std::fs::write(tool.join("LiveSplit.exe"), "test").unwrap();
    std::fs::write(
        tool.join("installed.json"),
        json!({
            "toolId": "livesplit", "version": "1.0.0", "executable": "LiveSplit.exe",
            "installPath": tool.to_string_lossy(),
        })
        .to_string(),
    )
    .unwrap();
    (paths, tool)
}

fn livesplit_registry() -> Map<String, Value> {
    let mut registry = Map::new();
    registry.insert(
        "livesplit".to_owned(),
        json!({"version": "1.0.0", "executable": "LiveSplit.exe"}),
    );
    registry
}

fn linux_service(paths: LibraryPaths, runner: &ToolProcessRunner) -> ToolService {
    let no_fixes: tool_service::LiveSplitFixesFn = Arc::new(|_: &Path, _: &Path| Ok(Vec::new()));
    ToolService::new(
        paths,
        ToolOptions {
            registry: Some(livesplit_registry()),
            platform: Some(Platform::Linux),
            host_environment: Some([("DISPLAY".to_owned(), ":0".to_owned())].into()),
            live_split_fixes: Some(no_fixes),
            runner: Some(Arc::new(runner.clone())),
            logger: Some(Arc::new(MemoryLogger::default())),
            ..ToolOptions::default()
        },
    )
}

fn code_of(failure: Failure) -> (String, String) {
    let error = to_delivery_error(failure, None);
    (error.code.as_str().to_owned(), error.message)
}

// ---------------------------------------------------------------- самотесты linux-selftest.js

/// «LiveSplit: повторный запуск и закрытие в отдельной группе процессов».
#[tokio::test(start_paused = true)]
async fn livesplit_launch_stop_in_its_own_process_group() {
    let root = tempfile::tempdir().unwrap();
    let (paths, tool) = installed_livesplit(root.path());
    let proton = FakeProton::new(4242);
    // SIGTERM группе: сессия выходит (`setImmediate(() => child.emit("exit", 0, signal))`).
    let stopper = {
        let proton = proton.clone();
        FakeStopper::new(move |name, _| {
            assert_eq!(name, "graceful_group", "сигнал уходит группе процессов");
            proton.exit(0, 0);
            StopResult::success("SIGTERM")
        })
    };
    let runner = linux_runner(Some(proton.clone()), stopper.clone(), None);
    let states: Arc<Mutex<Vec<(bool, bool)>>> = Arc::default();
    {
        let (watched, states) = (runner.clone(), states.clone());
        runner.subscribe(Arc::new(move |tool_id: &str| {
            let state = watched.running(tool_id);
            states
                .lock()
                .unwrap()
                .push((state.is_some(), state.is_some_and(|s| s.stopping)));
        }));
    }
    let service = linux_service(paths, &runner);

    let launch = LaunchOptions {
        game_id: Some("ghosting-renewed".to_owned()),
        ..LaunchOptions::default()
    };
    let started = service.launch("livesplit", &launch).await.unwrap();
    assert_eq!(started, json!({"status": "started", "pid": 4242}));
    {
        let calls = proton.calls.lock().unwrap();
        assert_eq!(calls[0].game_id.as_deref(), Some("ghosting-renewed"));
        assert_eq!(calls[0].exe, tool.join("LiveSplit.exe"));
        assert_eq!(calls[0].options.cwd, tool);
        assert_eq!(calls[0].options.font_profile.as_deref(), Some("livesplit"));
        assert_eq!(
            calls[0].options.env,
            Some(vec![(
                "PROTON_USE_X11_EXCLUSIVE".to_owned(),
                "LiveSplit.exe".to_owned()
            )])
        );
    }
    assert_eq!(service.status("livesplit")["running"], true);
    let again = service
        .launch("livesplit", &LaunchOptions::default())
        .await
        .unwrap();
    assert_eq!(again["status"], "already-running");
    assert_eq!(proton.sessions(), 1, "второй экземпляр не запускается");
    let removed = service.remove("livesplit").unwrap_err();
    assert_eq!(code_of(removed).0, "ALREADY_RUNNING");

    let stopped = service.stop("livesplit", false).await.unwrap();
    assert_eq!(stopped, StopStatus::Stopped);
    assert_eq!(stopper.calls(), [("graceful_group".to_owned(), 4242)]);
    assert_eq!(service.status("livesplit")["running"], false);
    assert_eq!(
        *states.lock().unwrap(),
        [(true, false), (true, true), (false, false)]
    );
}

/// «LiveSplit: закрытый своим крестиком при живой сессии umu больше не считается открытым».
#[tokio::test(start_paused = true)]
async fn livesplit_closed_by_its_cross_is_not_open_while_umu_session_lives() {
    let root = tempfile::tempdir().unwrap();
    let (paths, _) = installed_livesplit(root.path());
    let proton = FakeProton::new(5151);
    let stopper = FakeStopper::new(|_, _| StopResult::success("SIGTERM"));
    // Процесс Wine: ещё нет, есть, есть, пропал, пропал.
    let alive = Arc::new(Mutex::new(vec![false, true, true, false, false]));
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let process_running: ProcessRunning = {
        let (alive, asked) = (alive.clone(), asked.clone());
        Arc::new(move |name| {
            asked.lock().unwrap().push(name.to_owned());
            let mut list = alive.lock().unwrap();
            Some(if list.is_empty() {
                false
            } else {
                list.remove(0)
            })
        })
    };
    let runner = linux_runner(Some(proton.clone()), stopper.clone(), Some(process_running));
    let (closed_tx, closed_rx) = oneshot::channel();
    {
        let runner = runner.clone();
        let closed_tx = Mutex::new(Some(closed_tx));
        runner.clone().subscribe(Arc::new(move |tool_id: &str| {
            if runner.running(tool_id).is_none()
                && let Some(sender) = closed_tx.lock().unwrap().take()
            {
                let _ = sender.send(());
            }
        }));
    }
    let service = linux_service(paths, &runner);
    let launch = LaunchOptions {
        game_id: Some("ghosting-renewed".to_owned()),
        ..LaunchOptions::default()
    };

    assert_eq!(
        service.launch("livesplit", &launch).await.unwrap()["status"],
        "started"
    );
    assert_eq!(
        service.status("livesplit")["running"],
        true,
        "пока exe не появился, LiveSplit считается открытым"
    );
    tokio::time::timeout(Duration::from_secs(5), closed_rx)
        .await
        .expect("закрытие засчитано")
        .unwrap();
    assert!(
        alive.lock().unwrap().is_empty(),
        "закрытие засчитывается только после двух пропусков подряд"
    );
    assert_eq!(asked.lock().unwrap()[0], "LiveSplit.exe");
    assert_eq!(
        stopper.calls(),
        [("graceful_group".to_owned(), 5151)],
        "сессия umu закрывается вместе с LiveSplit"
    );
    assert!(service.running_info("livesplit").is_none());

    // Снова запускается, а не отвечает «уже запущен».
    *alive.lock().unwrap() = vec![true];
    assert_eq!(
        service.launch("livesplit", &launch).await.unwrap()["status"],
        "started"
    );
    assert_eq!(proton.sessions(), 2);
}

// ---------------------------------------------------------------- ToolService.js: запуск и остановка

#[tokio::test(start_paused = true)]
async fn linux_launch_without_proton_answers_like_the_stub() {
    let root = tempfile::tempdir().unwrap();
    let runner = linux_runner(None, FakeStopper::new(|_, _| StopResult::success("")), None);
    let stub = UnavailableToolRunner
        .launch(&request("livesplit", root.path(), Some("ghosting-renewed")))
        .await
        .unwrap_err();
    let real = runner
        .launch(&request("livesplit", root.path(), Some("ghosting-renewed")))
        .await
        .unwrap_err();
    assert_eq!(code_of(real), code_of(stub));
    assert!(runner.running("livesplit").is_none());
}

#[tokio::test(start_paused = true)]
async fn session_exit_ends_the_tool_and_a_new_launch_keeps_its_own_record() {
    let proton = FakeProton::new(100);
    let runner = linux_runner(
        Some(proton.clone()),
        FakeStopper::new(|_, _| StopResult::success("")),
        Some(Arc::new(|_| None)),
    );
    let root = tempfile::tempdir().unwrap();
    let pid = runner
        .launch(&request("livesplit", root.path(), Some("hl2-4260")))
        .await
        .unwrap();
    assert_eq!(pid, Some(100));
    assert_eq!(
        runner.running("livesplit"),
        Some(RunningTool {
            game_id: Some("hl2-4260".to_owned()),
            splits: Some("maps".to_owned()),
            stopping: false
        })
    );
    // Новый запуск заменяет запись; выход старой сессии новую не трогает.
    runner
        .launch(&request("livesplit", root.path(), Some("hl1-won")))
        .await
        .unwrap();
    proton.exit(0, 0);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        runner
            .running("livesplit")
            .and_then(|r| r.game_id)
            .as_deref(),
        Some("hl1-won")
    );
    proton.exit(0, 0);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(runner.running("livesplit").is_none());
}

#[tokio::test(start_paused = true)]
async fn stop_answers_not_running_stopping_and_timeout() {
    let proton = FakeProton::new(700);
    // Сессия на сигнал не отвечает.
    let stopper = FakeStopper::new(|_, _| StopResult::success("SIGTERM"));
    let runner = linux_runner(
        Some(proton.clone()),
        stopper.clone(),
        Some(Arc::new(|_| None)),
    );
    let root = tempfile::tempdir().unwrap();

    assert_eq!(
        runner.stop("livesplit", false).await.unwrap(),
        StopStatus::NotRunning
    );
    runner
        .launch(&request("livesplit", root.path(), Some("hl1-won")))
        .await
        .unwrap();

    let first = {
        let runner = runner.clone();
        tokio::spawn(async move { runner.stop("livesplit", true).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(runner.running("livesplit").unwrap().stopping);
    assert_eq!(
        runner.stop("livesplit", false).await.unwrap(),
        StopStatus::Stopping
    );
    assert_eq!(first.await.unwrap().unwrap(), StopStatus::Timeout);
    assert!(
        !runner.running("livesplit").unwrap().stopping,
        "после тайм-аута снова можно закрыть"
    );
    assert_eq!(stopper.calls(), [("force_group".to_owned(), 700)]);
}

#[tokio::test(start_paused = true)]
async fn failed_signal_is_stop_failed_and_resets_stopping() {
    let proton = FakeProton::new(800);
    let stopper = FakeStopper::new(|_, _| StopResult::failure("ESRCH", "kill ESRCH"));
    let runner = linux_runner(Some(proton), stopper, Some(Arc::new(|_| None)));
    let root = tempfile::tempdir().unwrap();
    runner
        .launch(&request("livesplit", root.path(), Some("hl1-won")))
        .await
        .unwrap();
    let states: Arc<Mutex<Vec<bool>>> = Arc::default();
    {
        let (runner, states) = (runner.clone(), states.clone());
        runner.clone().subscribe(Arc::new(move |tool_id: &str| {
            states
                .lock()
                .unwrap()
                .push(runner.running(tool_id).unwrap().stopping);
        }));
    }
    let error = runner.stop("livesplit", false).await.unwrap_err();
    assert_eq!(
        code_of(error),
        (
            "STOP_FAILED".to_owned(),
            "Не удалось закрыть инструмент: kill ESRCH".to_owned()
        )
    );
    assert_eq!(*states.lock().unwrap(), [true, false]);
    assert!(!runner.running("livesplit").unwrap().stopping);
}

#[tokio::test(start_paused = true)]
async fn tool_without_a_session_cannot_be_stopped() {
    // `protonRunner.launch` не вернул сессию: инструмент не отслеживается, как у 0.4.7.
    struct NoSession;
    impl PrefixLauncher for NoSession {
        fn launch<'a>(
            &'a self,
            _: Option<&'a str>,
            _: &'a Path,
            _: &'a [String],
            _: &'a PrefixLaunchOptions,
        ) -> BoxFuture<'a, Result<PrefixLaunched, Failure>> {
            Box::pin(async {
                Ok(PrefixLaunched {
                    child: None,
                    prefix: PathBuf::from("prefix"),
                })
            })
        }
    }
    let runner = ToolProcessRunner::new(ToolRunnerOptions {
        platform: Some(Platform::Linux),
        logger: Some(Arc::new(MemoryLogger::default())),
        proton: Some(Arc::new(NoSession)),
        ..ToolRunnerOptions::default()
    });
    let root = tempfile::tempdir().unwrap();
    assert_eq!(
        runner
            .launch(&request("livesplit", root.path(), Some("hl1-won")))
            .await
            .unwrap(),
        None
    );
    assert!(runner.running("livesplit").is_none());
}

#[tokio::test(start_paused = true)]
async fn launch_is_logged_with_the_prefix() {
    let logger = Arc::new(MemoryLogger::default());
    let runner = ToolProcessRunner::new(ToolRunnerOptions {
        platform: Some(Platform::Linux),
        logger: Some(logger.clone()),
        proton: Some(FakeProton::new(9)),
        process_running: Some(Arc::new(|_| None)),
        ..ToolRunnerOptions::default()
    });
    let root = tempfile::tempdir().unwrap();
    let launch = request("livesplit", root.path(), Some("hl1-won"));
    runner.launch(&launch).await.unwrap();
    assert_eq!(
        logger.take(),
        [(
            "info",
            format!(
                "[tools] livesplit запущен в префиксе prefix: {}",
                launch.exe.display()
            )
        )]
    );
}

// ---------------------------------------------------------------- /proc

#[test]
fn wine_process_is_found_by_comm_in_a_fake_proc() {
    let proc = tempfile::tempdir().unwrap();
    let process = |pid: &str, comm: &str| {
        let dir = proc.path().join(pid);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("comm"), comm).unwrap();
    };
    process("1", "systemd\n");
    process("4242", "LiveSplit.exe\n");
    process("4243", "VeryLongToolNam\n");
    // Не процесс: имя не из цифр, как `self` или `sys`.
    process("self", "livesplit.exe\n");
    // Процесс без comm (уже завершился).
    std::fs::create_dir_all(proc.path().join("77")).unwrap();

    let found = |name: &str| wine_process_running(proc.path(), name);
    assert_eq!(found("LiveSplit.exe"), Some(true));
    assert_eq!(found("livesplit.EXE"), Some(true), "без учёта регистра");
    assert_eq!(found("Other.exe"), Some(false));
    assert_eq!(
        found("VeryLongToolName.exe"),
        Some(true),
        "comm обрезан до 15 знаков"
    );
    assert_eq!(found("self"), Some(false));
    assert_eq!(
        wine_process_running(&proc.path().join("none"), "LiveSplit.exe"),
        None
    );
}

#[test]
fn wine_process_scan_of_the_real_proc_is_unknown_without_proc() {
    // На Linux `/proc` есть; на Windows его нет — «узнать нельзя».
    let result = wine_process_running(Path::new("/proc"), "hlsr-definitely-not-running.exe");
    if cfg!(target_os = "linux") {
        assert_eq!(result, Some(false));
    } else {
        assert_eq!(result, None);
    }
}

// ---------------------------------------------------------------- Windows: запуск не удался

#[tokio::test(start_paused = true)]
async fn direct_launch_of_a_missing_file_is_tracked_until_its_error() {
    let root = tempfile::tempdir().unwrap();
    let runner = ToolProcessRunner::new(ToolRunnerOptions {
        platform: Some(Platform::Win32),
        logger: Some(Arc::new(MemoryLogger::default())),
        ..ToolRunnerOptions::default()
    });
    let states: Arc<Mutex<Vec<bool>>> = Arc::default();
    {
        let (runner, states) = (runner.clone(), states.clone());
        runner.clone().subscribe(Arc::new(move |tool_id: &str| {
            states
                .lock()
                .unwrap()
                .push(runner.running(tool_id).is_some());
        }));
    }
    let mut launch = request("livesplit", root.path(), None);
    launch.exe = root.path().join("not-here.exe");
    // `ChildProcess` без PID: ответ «запущен» с `pid: null`, затем `error` снимает запись.
    assert_eq!(runner.launch(&launch).await.unwrap(), None);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(*states.lock().unwrap(), [true, false]);
    assert!(runner.running("livesplit").is_none());
}

// ---------------------------------------------------------------- окружение

#[test]
fn window_variables_are_removed_from_the_tool_environment() {
    let mut command = tokio::process::Command::new("tool");
    command
        .env("SLINT_SCALE_FACTOR", "1.25")
        .env("WINIT_X11_SCALE_FACTOR", "1.25")
        .env("HLSR_KEEP", "1");
    strip_ui_environment(&mut command);
    let envs: std::collections::HashMap<String, Option<String>> = command
        .as_std()
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect();
    assert_eq!(envs["SLINT_SCALE_FACTOR"], None);
    assert_eq!(envs["WINIT_X11_SCALE_FACTOR"], None);
    assert_eq!(envs["HLSR_KEEP"].as_deref(), Some("1"));
}
