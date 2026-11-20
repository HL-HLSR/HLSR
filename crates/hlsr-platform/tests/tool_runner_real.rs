//! Запуск и остановка инструментов над настоящими процессами: тест сам запускает процессы
//! (временная копия системной программы под своим именем) и закрывает только их. Windows:
//! прямой запуск (`launchDirect`), `taskkill /T [/F]`, окружение без переменных окна лаунчера.
//! Linux: сессия в своей группе процессов и сигнал группе.

use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Ждать условие не дольше `seconds` секунд.
async fn wait_until(seconds: u64, mut condition: impl FnMut() -> bool) -> bool {
    let end = std::time::Instant::now() + Duration::from_secs(seconds);
    while std::time::Instant::now() < end {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    condition()
}

/// Своих детей закрывает в конце теста, даже если он упал: `taskkill /F /T` по PID.
#[derive(Default)]
struct Cleanup(Arc<Mutex<Vec<u32>>>);

impl Cleanup {
    fn remember(&self, pid: u32) {
        self.0.lock().unwrap().push(pid);
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        for pid in self.0.lock().unwrap().drain(..) {
            #[cfg(windows)]
            {
                let _ = std::process::Command::new("taskkill.exe")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
            #[cfg(unix)]
            {
                let _ = std::process::Command::new("kill")
                    .args(["-KILL", &format!("-{pid}")])
                    .status();
            }
        }
    }
}

// ----------------------------------------------------------------------------- Windows

#[cfg(windows)]
mod windows {
    use super::*;
    use hlsr_core::js::Platform;
    use hlsr_delivery::errors::{Failure, to_delivery_error};
    use hlsr_delivery::paths::{LibraryPaths, resolve_library};
    use hlsr_delivery::ports::{MemoryLogger, RunningTool, StopStatus, ToolLaunch, ToolRunner};
    use hlsr_delivery::providers::BoxFuture;
    use hlsr_delivery::tool_service::{self, LaunchOptions, ToolOptions, ToolService};
    use hlsr_platform::process_inspect::is_alive;
    use hlsr_platform::process_stop::{StopResult, Stopper, SystemStopper};
    use hlsr_platform::tool_runner::{ToolProcessRunner, ToolRunnerOptions};
    use serde_json::{Map, Value, json};
    use std::path::{Path, PathBuf};

    /// Подставляет аргументы запуска: «инструмент» — копия `ping`, которой нужен срок.
    struct WithArgs {
        inner: ToolProcessRunner,
        args: Vec<String>,
    }

    impl ToolRunner for WithArgs {
        fn running(&self, tool_id: &str) -> Option<RunningTool> {
            self.inner.running(tool_id)
        }

        fn launch<'a>(
            &'a self,
            request: &'a ToolLaunch,
        ) -> BoxFuture<'a, Result<Option<u32>, Failure>> {
            Box::pin(async move {
                let mut changed = request.clone();
                changed.args = self.args.clone();
                self.inner.launch(&changed).await
            })
        }

        fn stop<'a>(
            &'a self,
            tool_id: &'a str,
            force: bool,
        ) -> BoxFuture<'a, Result<StopStatus, Failure>> {
            self.inner.stop(tool_id, force)
        }
    }

    type Reaction = Box<dyn Fn(&str, u32) -> StopResult + Send + Sync>;

    /// Остановка с записью вызовов; поведение каждого вызова задаёт `react`.
    struct ScriptedStopper {
        calls: Mutex<Vec<(String, u32)>>,
        react: Reaction,
    }

    impl ScriptedStopper {
        fn new(
            react: impl Fn(&str, u32) -> StopResult + Send + Sync + 'static,
        ) -> Arc<ScriptedStopper> {
            Arc::new(ScriptedStopper {
                calls: Mutex::default(),
                react: Box::new(react),
            })
        }

        fn calls(&self) -> Vec<(String, u32)> {
            self.calls.lock().unwrap().clone()
        }

        fn call(&self, name: &str, pid: u32) -> BoxFuture<'_, StopResult> {
            self.calls.lock().unwrap().push((name.to_owned(), pid));
            let result = (self.react)(name, pid);
            Box::pin(async move { result })
        }
    }

    impl Stopper for ScriptedStopper {
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

    const PING_SECONDS: &str = "120";

    fn ping_args() -> Vec<String> {
        ["-n", PING_SECONDS, "127.0.0.1"]
            .map(str::to_owned)
            .to_vec()
    }

    fn system_exe(name: &str) -> PathBuf {
        let system = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_owned());
        PathBuf::from(system).join("System32").join(name)
    }

    /// Установленный «LiveSplit» библиотеки: `Tools/livesplit/LiveSplit.exe` — копия `ping`.
    fn installed_livesplit(root: &Path, source: &Path) -> (LibraryPaths, PathBuf) {
        let paths = resolve_library(&root.join("lib").to_string_lossy()).unwrap();
        let tool = paths.tools.join("livesplit");
        std::fs::create_dir_all(&tool).unwrap();
        std::fs::copy(source, tool.join("LiveSplit.exe")).unwrap();
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

    fn registry() -> Map<String, Value> {
        let mut registry = Map::new();
        registry.insert(
            "livesplit".to_owned(),
            json!({"version": "1.0.0", "executable": "LiveSplit.exe"}),
        );
        registry
    }

    fn service(paths: LibraryPaths, runner: Arc<dyn ToolRunner>) -> ToolService {
        let no_fixes: tool_service::LiveSplitFixesFn =
            Arc::new(|_: &Path, _: &Path| Ok(Vec::new()));
        ToolService::new(
            paths,
            ToolOptions {
                registry: Some(registry()),
                platform: Some(Platform::Win32),
                live_split_fixes: Some(no_fixes),
                runner: Some(runner),
                logger: Some(Arc::new(MemoryLogger::default())),
                ..ToolOptions::default()
            },
        )
    }

    fn windows_runner(stopper: Arc<dyn Stopper>, logger: Arc<MemoryLogger>) -> ToolProcessRunner {
        ToolProcessRunner::new(ToolRunnerOptions {
            platform: Some(Platform::Win32),
            logger: Some(logger),
            stopper: Some(stopper),
            stop_timeout: Some(Duration::from_millis(1500)),
            ..ToolRunnerOptions::default()
        })
    }

    /// `linux-selftest.js`: «LiveSplit на Windows: запущенный отслеживается и закрывается через
    /// taskkill» — над настоящим дочерним процессом. Мягкое закрытие «просит окно закрыться»
    /// (подделка не закрывает), принудительное — настоящий `taskkill /F /T`.
    #[tokio::test]
    async fn livesplit_is_tracked_and_closed_through_taskkill() {
        let root = tempfile::tempdir().unwrap();
        let (paths, tool) = installed_livesplit(root.path(), &system_exe("PING.EXE"));
        let cleanup = Cleanup::default();
        let stopper = ScriptedStopper::new(|name, pid| match name {
            "graceful" => StopResult::success(""),
            _ => {
                let real = std::process::Command::new("taskkill.exe")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .output()
                    .unwrap();
                if real.status.success() {
                    StopResult::success("")
                } else {
                    StopResult::failure("1", String::from_utf8_lossy(&real.stderr))
                }
            }
        });
        let logger = Arc::new(MemoryLogger::default());
        let runner = windows_runner(stopper.clone(), logger.clone());
        let states: Arc<Mutex<Vec<(bool, bool)>>> = Arc::default();
        {
            let (runner, states) = (runner.clone(), states.clone());
            runner.clone().subscribe(Arc::new(move |tool_id: &str| {
                let state = runner.running(tool_id);
                states
                    .lock()
                    .unwrap()
                    .push((state.is_some(), state.is_some_and(|s| s.stopping)));
            }));
        }
        let service = service(
            paths,
            Arc::new(WithArgs {
                inner: runner.clone(),
                args: ping_args(),
            }),
        );

        let started = service
            .launch("livesplit", &LaunchOptions::default())
            .await
            .unwrap();
        assert_eq!(started["status"], "started");
        let pid = started["pid"].as_u64().expect("PID запущенного процесса") as u32;
        cleanup.remember(pid);
        assert!(is_alive(pid), "процесс запущен");
        assert_eq!(
            service.status("livesplit")["running"],
            true,
            "кнопка должна стать «Закрыть»"
        );
        let again = service
            .launch("livesplit", &LaunchOptions::default())
            .await
            .unwrap();
        assert_eq!(
            again["status"], "already-running",
            "второй экземпляр не запускается"
        );
        let line = logger.take();
        assert_eq!(
            line,
            [(
                "info",
                format!(
                    "[tools] livesplit запущен: {} [{}]",
                    tool.join("LiveSplit.exe").display(),
                    ping_args().join(" ")
                )
            )]
        );

        assert_eq!(
            service.stop("livesplit", false).await.unwrap(),
            StopStatus::Timeout
        );
        assert_eq!(service.status("livesplit")["running"], true);
        assert_eq!(service.status("livesplit")["stopping"], false);
        assert_eq!(
            service.stop("livesplit", true).await.unwrap(),
            StopStatus::Stopped
        );
        assert_eq!(
            stopper.calls(),
            [("graceful".to_owned(), pid), ("force".to_owned(), pid)]
        );
        assert_eq!(service.status("livesplit")["running"], false);
        assert!(wait_until(5, || !is_alive(pid)).await, "процесс закрыт");
        assert_eq!(
            *states.lock().unwrap(),
            [
                (true, false),
                (true, true),
                (true, false),
                (true, true),
                (false, false)
            ]
        );

        // Закрытый самим пользователем LiveSplit снова запускается из лаунчера.
        let second = service
            .launch("livesplit", &LaunchOptions::default())
            .await
            .unwrap();
        assert_eq!(second["status"], "started");
        let pid = second["pid"].as_u64().unwrap() as u32;
        cleanup.remember(pid);
        assert_eq!(service.status("livesplit")["running"], true);
        let outside = SystemStopper.force(pid).await;
        assert!(outside.ok, "{outside:?}");
        assert!(
            wait_until(5, || service.status("livesplit")["running"] == false).await,
            "выход процесса снимает запись"
        );
    }

    /// Настоящий `taskkill` без `/F` процессу без окна (консольной программе): «can only be
    /// terminated forcefully» — лаунчер отвечает `STOP_FAILED` и инструмент остаётся открытым.
    #[tokio::test]
    async fn real_graceful_taskkill_cannot_close_a_tool_without_a_window() {
        let root = tempfile::tempdir().unwrap();
        let (paths, _) = installed_livesplit(root.path(), &system_exe("PING.EXE"));
        let cleanup = Cleanup::default();
        let runner = windows_runner(Arc::new(SystemStopper), Arc::default());
        let service = service(
            paths,
            Arc::new(WithArgs {
                inner: runner,
                args: ping_args(),
            }),
        );
        let started = service
            .launch("livesplit", &LaunchOptions::default())
            .await
            .unwrap();
        let pid = started["pid"].as_u64().unwrap() as u32;
        cleanup.remember(pid);

        let failure = service.stop("livesplit", false).await.unwrap_err();
        let error = to_delivery_error(failure, None);
        assert_eq!(error.code.as_str(), "STOP_FAILED");
        assert!(
            error.message.starts_with("Не удалось закрыть инструмент: ")
                && error.message.len() > "Не удалось закрыть инструмент: ".len(),
            "{}",
            error.message
        );
        assert!(is_alive(pid));
        assert_eq!(service.status("livesplit")["running"], true);
        assert_eq!(service.status("livesplit")["stopping"], false);

        assert_eq!(
            service.stop("livesplit", true).await.unwrap(),
            StopStatus::Stopped
        );
        assert!(wait_until(5, || !is_alive(pid)).await);
    }

    #[tokio::test]
    async fn failed_close_without_output_names_the_taskkill_code() {
        let root = tempfile::tempdir().unwrap();
        let (paths, _) = installed_livesplit(root.path(), &system_exe("PING.EXE"));
        let cleanup = Cleanup::default();
        let stopper = ScriptedStopper::new(|_, _| StopResult::failure("128", ""));
        let runner = windows_runner(stopper, Arc::default());
        let service = service(
            paths,
            Arc::new(WithArgs {
                inner: runner,
                args: ping_args(),
            }),
        );
        let started = service
            .launch("livesplit", &LaunchOptions::default())
            .await
            .unwrap();
        cleanup.remember(started["pid"].as_u64().unwrap() as u32);

        let failure = service.stop("livesplit", false).await.unwrap_err();
        let error = to_delivery_error(failure, None);
        assert_eq!(error.code.as_str(), "STOP_FAILED");
        assert_eq!(error.message, "Не удалось закрыть инструмент: taskkill 128");
    }

    /// `taskkill` сообщил об ошибке, но процесс к этому времени уже вышел сам — это не отказ.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tool_that_exited_during_a_failed_close_counts_as_stopped() {
        let root = tempfile::tempdir().unwrap();
        let (paths, _) = installed_livesplit(root.path(), &system_exe("PING.EXE"));
        let cleanup = Cleanup::default();
        let runner_cell: Arc<Mutex<Option<ToolProcessRunner>>> = Arc::default();
        let stopper = {
            let runner_cell = runner_cell.clone();
            ScriptedStopper::new(move |_, pid| {
                // Процесс закрывают снаружи и ждут, пока лаунчер увидит выход.
                let _ = std::process::Command::new("taskkill.exe")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .output();
                let runner = runner_cell.lock().unwrap().clone().unwrap();
                let end = std::time::Instant::now() + Duration::from_secs(5);
                while runner.running("livesplit").is_some() && std::time::Instant::now() < end {
                    std::thread::sleep(Duration::from_millis(10));
                }
                StopResult::failure("128", "ERROR: The process was not found.")
            })
        };
        let runner = windows_runner(stopper, Arc::default());
        *runner_cell.lock().unwrap() = Some(runner.clone());
        let service = service(
            paths,
            Arc::new(WithArgs {
                inner: runner,
                args: ping_args(),
            }),
        );
        let started = service
            .launch("livesplit", &LaunchOptions::default())
            .await
            .unwrap();
        cleanup.remember(started["pid"].as_u64().unwrap() as u32);

        assert_eq!(
            service.stop("livesplit", false).await.unwrap(),
            StopStatus::Stopped
        );
        assert_eq!(service.status("livesplit")["running"], false);
    }

    /// Окружение этого процесса менять нельзя (тесты идут потоками, `set_var` небезопасен):
    /// сценарий идёт в повторном запуске этого же тестового бинарника, которому нужные
    /// переменные заданы через `Command::env`.
    const INNER: &str = "HLSR_TEST_RERUN_INNER";

    /// Окружение инструмента — как у лаунчера, без переменных его окна (решение 3).
    #[test]
    fn tool_environment_has_no_window_scale_variables() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "windows::tool_environment_has_no_window_scale_variables_inner",
                "--test-threads=1",
            ])
            .env(INNER, "1")
            .env("SLINT_SCALE_FACTOR", "1.25")
            .env("WINIT_X11_SCALE_FACTOR", "1.25")
            .env("HLSR_TEST_KEEP_ME", "kept")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}
{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Сам сценарий: выполняется только в повторном запуске (в обычном прогоне пуст).
    #[tokio::test]
    async fn tool_environment_has_no_window_scale_variables_inner() {
        if std::env::var_os(INNER).is_none() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let cleanup = Cleanup::default();
        // Копия `cmd.exe` выгружает окружение в файл рядом с собой и выходит.
        let (paths, tool) = installed_livesplit(root.path(), &system_exe("cmd.exe"));
        let runner = windows_runner(Arc::new(SystemStopper), Arc::default());
        let service = service(
            paths,
            Arc::new(WithArgs {
                inner: runner.clone(),
                args: vec!["/c".to_owned(), "set > env.txt".to_owned()],
            }),
        );
        let started = service.launch("livesplit", &LaunchOptions::default()).await;
        cleanup.remember(started.unwrap()["pid"].as_u64().unwrap() as u32);
        assert!(
            wait_until(10, || runner.running("livesplit").is_none()
                && tool.join("env.txt").exists())
            .await,
            "процесс вышел и записал окружение"
        );
        let dump = std::fs::read_to_string(tool.join("env.txt")).unwrap();
        assert!(
            dump.contains("HLSR_TEST_KEEP_ME=kept"),
            "обычное окружение наследуется"
        );
        assert!(!dump.contains("SLINT_SCALE_FACTOR"), "{dump}");
        assert!(!dump.contains("WINIT_X11_SCALE_FACTOR"), "{dump}");
    }
}

// ----------------------------------------------------------------------------- Linux

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use hlsr_core::js::Platform;
    use hlsr_delivery::errors::Failure;
    use hlsr_delivery::ports::{MemoryLogger, StopStatus, ToolLaunch, ToolRunner};
    use hlsr_delivery::providers::BoxFuture;
    use hlsr_platform::game_process_service::ChildHandle;
    use hlsr_platform::process_inspect::is_alive;
    use hlsr_platform::tool_runner::{
        PrefixLaunchOptions, PrefixLaunched, PrefixLauncher, ToolProcessRunner, ToolRunnerOptions,
    };
    use std::path::{Path, PathBuf};

    /// Сессия Proton: настоящий `sleep` в своей группе процессов.
    struct SleepSession;

    impl PrefixLauncher for SleepSession {
        fn launch<'a>(
            &'a self,
            _game_id: Option<&'a str>,
            _exe: &'a Path,
            _args: &'a [String],
            options: &'a PrefixLaunchOptions,
        ) -> BoxFuture<'a, Result<PrefixLaunched, Failure>> {
            Box::pin(async move {
                let mut command = tokio::process::Command::new("sleep");
                command.arg("60").current_dir(&options.cwd);
                command.process_group(0);
                let child = command.spawn().expect("sleep");
                Ok(PrefixLaunched {
                    child: ChildHandle::from_tokio(child),
                    prefix: PathBuf::from("prefix"),
                })
            })
        }
    }

    /// Сигнал группе процессов сессии закрывает настоящий процесс (`linux-selftest.js`: группы).
    #[tokio::test]
    async fn group_signal_closes_a_real_session() {
        let root = tempfile::tempdir().unwrap();
        let cleanup = Cleanup::default();
        let runner = ToolProcessRunner::new(ToolRunnerOptions {
            platform: Some(Platform::Linux),
            logger: Some(Arc::new(MemoryLogger::default())),
            proton: Some(Arc::new(SleepSession)),
            process_running: Some(Arc::new(|_| None)),
            stop_timeout: Some(Duration::from_secs(5)),
            ..ToolRunnerOptions::default()
        });
        let request = ToolLaunch {
            tool_id: "livesplit".to_owned(),
            exe: root.path().join("LiveSplit.exe"),
            root: root.path().to_path_buf(),
            args: Vec::new(),
            game_id: Some("ghosting-renewed".to_owned()),
            splits: None,
            executable: "LiveSplit.exe".to_owned(),
            env: None,
            font_profile: None,
        };
        let pid = runner.launch(&request).await.unwrap().expect("PID сессии");
        cleanup.remember(pid);
        assert!(is_alive(pid));
        assert_eq!(
            runner.stop("livesplit", false).await.unwrap(),
            StopStatus::Stopped
        );
        assert!(wait_until(5, || !is_alive(pid)).await);
        assert!(runner.running("livesplit").is_none());
    }
}
