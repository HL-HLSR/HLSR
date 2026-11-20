//! Процессы настоящей ОС: тест сам запускает процессы (временная копия `ping` в Windows,
//! `sleep` в Linux) и проверяет поиск по PID и по имени образа, путь, время старта, остановку,
//! службу состояний и сверку после перезапуска лаунчера. Чужие процессы не трогаются:
//! остановка идёт по PID только своих детей.

mod support;

use hlsr_core::js::date_parse;
use hlsr_core::js::iso::iso_from_ms;
use hlsr_delivery::ports::{Logger, MemoryLogger};
use hlsr_platform::game_process_service::{
    ChildHandle, GameProcessService, ProcessEvent, ServiceOptions, StartedProcess, StopOptions,
};
use hlsr_platform::process_inspect::{is_alive, same_path, system_inspector};
use hlsr_platform::process_stop::{Stopper, SystemStopper};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use support::sleeper::Sleeper;

fn ms(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

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

#[test]
fn inspector_finds_a_real_process_by_pid_and_by_image_name() {
    let sleeper = Sleeper::start(60);
    let pid = sleeper.pid();
    let inspector = system_inspector();

    assert!(inspector.is_alive(pid));
    let info = inspector.inspect(pid).expect("процесс найден по PID");
    assert_eq!(info.pid, pid);
    let path = info
        .executable_path
        .as_deref()
        .expect("путь образа известен");
    assert!(
        same_path(Some(path), sleeper.exe.to_str()),
        "путь {path} против {}",
        sleeper.exe.display()
    );
    #[cfg(windows)]
    {
        assert_eq!(
            info.name.as_deref().map(str::to_lowercase),
            Some(sleeper.image.to_lowercase())
        );
        assert_eq!(info.source.as_deref(), Some("winapi"));
        let started = date_parse(info.started_at.as_deref().expect("время старта известно"))
            .expect("время старта — дата");
        // Время старта по часам системы — между часами до запуска и после его возврата (с запасом
        // на точность часов): под нагрузкой сам запуск может занять секунды.
        let (before, after) = (ms(sleeper.started), ms(sleeper.spawned));
        assert!(
            started >= before - 5000 && started <= after + 5000,
            "старт {started} вне запуска {before}..{after}"
        );
    }
    #[cfg(target_os = "linux")]
    {
        // `comm` — первые 15 знаков имени; времени старта у 0.4.7 на Linux нет.
        assert!(sleeper.image.starts_with(info.name.as_deref().unwrap()));
        assert_eq!(info.started_at, None);
    }

    let found = inspector.find_by_image_name(&sleeper.image);
    assert_eq!(
        found.iter().map(|item| item.pid).collect::<Vec<_>>(),
        vec![pid],
        "по имени образа находится только этот процесс"
    );
    assert!(
        inspector
            .find_by_image_name("hlsr-no-such-image-1")
            .is_empty()
    );
    assert!(inspector.find_by_image_name("bad name.exe").is_empty());
}

#[test]
fn inspector_sees_the_current_process_and_not_a_missing_one() {
    let inspector = system_inspector();
    let own = std::process::id();
    let info = inspector.inspect(own).expect("свой процесс виден");
    let exe = std::env::current_exe().unwrap();
    assert!(same_path(info.executable_path.as_deref(), exe.to_str(),));
    assert!(is_alive(own));

    // PID с запасом выше возможных: такого процесса нет.
    let missing = 0x7fff_ff00;
    assert!(!inspector.is_alive(missing));
    assert!(inspector.inspect(missing).is_none());
    assert!(!inspector.is_alive(0));
    assert!(inspector.inspect(0).is_none());
}

/// Запасной путь Windows: система не ответила — спрашивается настоящий `tasklist.exe` (только по
/// PID этого же процесса), в ответе имя образа без пути и причина сбоя в `lastFailure`.
#[cfg(windows)]
#[test]
fn tasklist_answers_when_the_system_does_not() {
    use hlsr_delivery::environment::Environment;
    use hlsr_platform::process_inspect::{
        ProcessInfo, ProcessInspector, SystemProcesses, SystemTasklist, Tasklist, WindowsInspector,
        parse_tasklist,
    };

    struct Broken;
    impl SystemProcesses for Broken {
        fn inspect(&self, _pid: u32) -> Result<Option<ProcessInfo>, String> {
            Err("доступ запрещён".to_owned())
        }
        fn find_by_image_name(&self, _image: &str) -> Result<Vec<ProcessInfo>, String> {
            Err("доступ запрещён".to_owned())
        }
    }

    let own = std::process::id();
    let exe_name = std::env::current_exe()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_lowercase();

    let tasklist = SystemTasklist::new(&Environment::process());
    let output = tasklist
        .run(&format!("PID eq {own}"))
        .expect("tasklist ответил");
    let listed = parse_tasklist(&output);
    assert_eq!(listed.len(), 1, "{output}");
    assert_eq!(listed[0].pid, own);
    assert_eq!(
        listed[0].name.as_deref().map(str::to_lowercase),
        Some(exe_name.clone())
    );

    let inspector = WindowsInspector::new(Arc::new(Broken), Arc::new(tasklist));
    let info = inspector.inspect(own).expect("tasklist нашёл процесс");
    assert_eq!(info.source.as_deref(), Some("tasklist"));
    assert_eq!(info.executable_path, None);
    assert_eq!(info.started_at, None);
    assert_eq!(
        info.name.as_deref().map(str::to_lowercase),
        Some(exe_name.clone())
    );
    assert_eq!(
        inspector.last_failure().as_deref(),
        Some("winapi: доступ запрещён")
    );

    let by_name = inspector.find_by_image_name(&exe_name);
    assert!(by_name.iter().any(|item| item.pid == own), "{by_name:?}");
    assert!(
        inspector
            .last_failure()
            .unwrap()
            .starts_with("winapi: доступ запрещён"),
    );
}

#[test]
fn a_finished_process_is_not_alive_any_more() {
    let mut sleeper = Sleeper::start(60);
    let pid = sleeper.pid();
    let inspector = system_inspector();
    assert!(inspector.is_alive(pid));
    sleeper.kill();
    // `kill` дождался выхода: ручки процесса больше нет ни у кого, кроме, возможно, системы.
    assert!(!inspector.is_alive(pid));
    assert!(inspector.inspect(pid).is_none());
}

#[tokio::test]
async fn stopper_ends_a_real_process() {
    let mut sleeper = Sleeper::start(60);
    let pid = sleeper.pid();
    let stopper = SystemStopper;

    // Мягкая остановка: Windows — `taskkill /T` без окон у процесса может отказать, Linux —
    // `SIGTERM` завершает. Исход любой, главное — вызов возвращается.
    let soft = stopper.graceful(pid).await;
    #[cfg(unix)]
    assert!(soft.ok, "{soft:?}");
    #[cfg(windows)]
    let _ = soft;

    tokio::time::sleep(Duration::from_millis(300)).await;
    if !sleeper.is_exited() {
        let forced = stopper.force(pid).await;
        #[cfg(windows)]
        assert!(forced.ok, "{forced:?}");
        #[cfg(unix)]
        let _ = forced;
    }

    assert!(
        wait_until(10, || sleeper.is_exited()).await,
        "процесс закрыт"
    );
    sleeper.reap();
    assert!(!is_alive(pid));

    // Процесса уже нет: остановка отвечает ошибкой, а не паникой.
    let again = stopper.force(pid).await;
    assert!(!again.ok);
    assert!(again.code.is_some());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn stopper_signals_the_whole_process_group_on_linux() {
    use std::os::unix::process::CommandExt as _;
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("60")
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = child.id();
    let result = SystemStopper.graceful_group(pid).await;
    assert!(result.ok, "{result:?}");
    child.wait().unwrap();
    let missing = SystemStopper.force_group(pid).await;
    assert!(!missing.ok);
    assert_eq!(missing.code.as_deref(), Some("ESRCH"));
}

struct Harness {
    service: GameProcessService,
    logger: Arc<MemoryLogger>,
    events: Arc<Mutex<Vec<ProcessEvent>>>,
    state: std::path::PathBuf,
}

/// `games` — `paths.games`: процесс записи сверяется со своей папкой игры (0.4.9).
fn harness(state: &std::path::Path, games: Option<std::path::PathBuf>) -> Harness {
    let logger = Arc::new(MemoryLogger::default());
    let service = GameProcessService::new(ServiceOptions {
        state_dir: state.to_path_buf(),
        games_dir: games,
        logger: Some(logger.clone() as Arc<dyn Logger>),
        poll_interval_ms: Some(50),
        ..ServiceOptions::default()
    });
    let events: Arc<Mutex<Vec<ProcessEvent>>> = Arc::default();
    {
        let events = events.clone();
        service.subscribe(Arc::new(move |event| {
            events.lock().unwrap().push(event.clone())
        }));
    }
    Harness {
        service,
        logger,
        events,
        state: state.to_path_buf(),
    }
}

impl Harness {
    fn started(&self, sleeper: &Sleeper) -> StartedProcess {
        StartedProcess {
            pid: Some(i64::from(sleeper.pid())),
            executable_path: Some(sleeper.exe.to_string_lossy().into_owned()),
            install_path: Some(sleeper.root().to_string_lossy().into_owned()),
            // Время старта, как его видит система: часы теста до запуска расходятся с ним на время
            // самого запуска, а под нагрузкой оно длится секунды (допуск сверки — 5 с).
            started_at: system_inspector()
                .inspect(sleeper.pid())
                .and_then(|info| info.started_at)
                .or_else(|| Some(iso_from_ms(ms(sleeper.spawned)))),
            ..StartedProcess::default()
        }
    }

    fn finished(&self) -> Option<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .find(|event| event.name == "finished")
            .map(|event| event.payload.clone())
    }

    fn file(&self, game_id: &str) -> std::path::PathBuf {
        self.state.join(format!("{game_id}.process.json"))
    }
}

#[tokio::test]
async fn service_tracks_stops_and_forgets_a_real_game() {
    let state = tempfile::tempdir().unwrap();
    let mut sleeper = Sleeper::start(60);
    let h = harness(state.path(), Some(sleeper.games()));

    let registered = h
        .service
        .register_started_process("hl1-won", h.started(&sleeper))
        .expect("процесс зарегистрирован");
    assert_eq!(registered["state"], "running");
    assert!(h.service.is_running("hl1-won"));
    assert_eq!(h.service.any_running().as_deref(), Some("hl1-won"));
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(h.file("hl1-won")).unwrap()).unwrap();
    assert_eq!(saved["pid"], sleeper.pid());
    assert_eq!(saved["launcherPid"], std::process::id());

    // Мягкая остановка Windows-консоли не срабатывает — как у 0.4.7 это «timeout»; принудительная
    // закрывает процесс.
    let outcome = h
        .service
        .request_stop(
            "hl1-won",
            StopOptions {
                force: true,
                timeout_ms: Some(10_000),
            },
        )
        .await
        .expect("остановка");
    assert_eq!(outcome["status"], "stopped");
    sleeper.reap();

    assert!(!h.service.is_running("hl1-won"));
    assert!(!h.file("hl1-won").exists(), "запись о процессе убрана");
    let finished = h.finished().expect("событие finished");
    assert_eq!(finished["lastExit"]["outcome"], "exited");
    assert_eq!(finished["lastExit"]["requested"], true);
    assert_eq!(
        h.service.get_process_state("hl1-won")["state"],
        "not-running"
    );
}

#[tokio::test]
async fn polling_notices_that_the_game_disappeared() {
    let state = tempfile::tempdir().unwrap();
    let mut sleeper = Sleeper::start(60);
    let h = harness(state.path(), Some(sleeper.games()));
    h.service
        .register_started_process("hl1-won", h.started(&sleeper))
        .unwrap();

    sleeper.kill();
    let service = h.service.clone();
    assert!(
        wait_until(10, || !service.is_running("hl1-won")).await,
        "опрос раз в 50 мс увидел, что процесса нет"
    );
    let finished = h.finished().expect("событие finished");
    // Код выхода неизвестен — «закрыта штатно», но обнаружено опросом.
    assert_eq!(finished["lastExit"]["outcome"], "exited");
    assert_eq!(finished["lastExit"]["detected"], true);
    assert!(!h.file("hl1-won").exists());
    let lines = h.logger.take();
    assert!(
        lines
            .iter()
            .any(|(_, text)| text.contains("закрыта штатно")),
        "{lines:?}"
    );
}

#[tokio::test]
async fn game_is_adopted_after_a_launcher_restart_and_can_be_stopped() {
    let state = tempfile::tempdir().unwrap();
    let mut sleeper = Sleeper::start(60);
    {
        let before = harness(state.path(), Some(sleeper.games()));
        before
            .service
            .register_started_process("hl1-won", before.started(&sleeper))
            .unwrap();
        // Лаунчер закрыт: отслеживание забыто, запись о процессе осталась на диске.
        before.service.dispose();
        assert!(before.file("hl1-won").exists());
    }

    let after = harness(state.path(), Some(sleeper.games()));
    assert!(!after.service.is_running("hl1-won"));
    let results = after.service.reconcile_after_launcher_restart(&[]);
    assert_eq!(
        results,
        vec![serde_json::json!({"gameId": "hl1-won", "adopted": true, "pid": sleeper.pid()})]
    );
    assert!(after.service.is_running("hl1-won"));

    let outcome = after
        .service
        .request_stop(
            "hl1-won",
            StopOptions {
                force: true,
                timeout_ms: Some(10_000),
            },
        )
        .await
        .expect("остановка");
    assert_eq!(outcome["status"], "stopped");
    sleeper.reap();
    assert!(!after.file("hl1-won").exists());
}

#[tokio::test]
async fn a_record_of_a_foreign_process_is_dropped_after_a_launcher_restart() {
    let state = tempfile::tempdir().unwrap();
    let mut sleeper = Sleeper::start(60);
    let other = Sleeper::start(60);
    {
        let before = harness(state.path(), Some(sleeper.games()));
        // В записи — путь другого процесса, а PID — этого: PID чужой.
        let mut started = before.started(&sleeper);
        started.executable_path = Some(other.exe.to_string_lossy().into_owned());
        started.install_path = Some(other.root().to_string_lossy().into_owned());
        before
            .service
            .register_started_process("hl1-won", started)
            .unwrap();
        before.service.dispose();
    }
    let after = harness(state.path(), Some(sleeper.games()));
    let results = after.service.reconcile_after_launcher_restart(&[]);
    assert_eq!(
        results,
        vec![serde_json::json!({"gameId": "hl1-won", "adopted": false, "reason": "stale"})]
    );
    assert!(!after.service.is_running("hl1-won"));
    assert!(!after.file("hl1-won").exists());
    sleeper.kill();
}

#[tokio::test]
async fn exit_code_of_a_child_decides_between_exited_and_crashed() {
    for (code, outcome) in [(0, "exited"), (3, "crashed")] {
        let state = tempfile::tempdir().unwrap();
        let h = harness(state.path(), None);

        #[cfg(windows)]
        let mut command = {
            let mut command = tokio::process::Command::new("cmd.exe");
            command.args(["/c", &format!("exit {code}")]);
            command
        };
        #[cfg(unix)]
        let mut command = {
            let mut command = tokio::process::Command::new("/bin/sh");
            command.args(["-c", &format!("exit {code}")]);
            command
        };
        let child = command.spawn().unwrap();
        let handle = ChildHandle::from_tokio(child).expect("PID дочернего процесса");
        let pid = handle.pid;

        // Папка игры — папка программы, путь внутри неё.
        #[cfg(windows)]
        let exe = std::path::PathBuf::from(std::env::var("SystemRoot").unwrap())
            .join("System32")
            .join("cmd.exe");
        #[cfg(unix)]
        let exe = std::path::PathBuf::from("/bin/sh");
        let started = StartedProcess {
            pid: Some(i64::from(pid)),
            executable_path: Some(exe.to_string_lossy().into_owned()),
            install_path: Some(exe.parent().unwrap().to_string_lossy().into_owned()),
            child: Some(handle),
            ..StartedProcess::default()
        };
        h.service
            .register_started_process("hl1-won", started)
            .unwrap();

        let service = h.service.clone();
        assert!(
            wait_until(10, || !service.is_running("hl1-won")).await,
            "выход дочернего процесса замечен"
        );
        let finished = h.finished().expect("событие finished");
        assert_eq!(finished["lastExit"]["outcome"], outcome, "код {code}");
        assert_eq!(finished["lastExit"]["exitCode"], code);
        assert_eq!(finished["lastExit"]["detected"], false);
    }
}

#[cfg(any(debug_assertions, feature = "dev-hooks"))]
mod qa_launch {
    use super::*;
    use hlsr_delivery::environment::Environment;
    use hlsr_platform::qa_launch::{ARGS_FLAG, EXE_FLAG, describe, launch};

    fn env(vars: &[(&str, &str)]) -> Environment {
        Environment::new(
            vars.iter()
                .map(|(name, value)| (name.to_string(), value.to_string())),
            false,
        )
    }

    fn qa_env(exe: &str, args: &str) -> Environment {
        env(&[
            ("HLSR_GAME_DELIVERY_QA", "true"),
            ("GAME_DELIVERY_REMOTE_ENABLED", "true"),
            (EXE_FLAG, exe),
            (ARGS_FLAG, args),
        ])
    }

    #[tokio::test]
    async fn qa_launch_starts_the_test_executable_and_hands_over_the_child() {
        let sleeper_source = Sleeper::start(1);
        let library = tempfile::tempdir().unwrap();
        let install = library.path().join("Games").join("hl1-won");
        let install_path = install.to_string_lossy().into_owned();
        // Копия программы под тестовым именем внутри папки игры.
        std::fs::create_dir_all(install.join("bin")).unwrap();
        std::fs::copy(&sleeper_source.exe, install.join("bin").join("qa-game.exe")).unwrap();

        #[cfg(windows)]
        let args = "-n 60 127.0.0.1";
        #[cfg(unix)]
        let args = "60";
        let environment = qa_env("bin/qa-game.exe", args);
        assert_eq!(describe(&environment)["enabled"], true);

        let launched = launch(&install_path, &environment).expect("запуск");
        assert!(is_alive(launched.pid));
        assert!(launched.executable_path.ends_with("qa-game.exe"));
        let started = date_parse(&launched.started_at).unwrap();
        assert!((started - ms(SystemTime::now())).abs() < 5000);

        // Дочерний процесс передаётся службе, она его и закрывает.
        let state = tempfile::tempdir().unwrap();
        let h = harness(state.path(), Some(library.path().join("Games")));
        h.service
            .register_started_process(
                "hl1-won",
                StartedProcess {
                    pid: Some(i64::from(launched.pid)),
                    executable_path: Some(launched.executable_path.clone()),
                    install_path: Some(install_path.clone()),
                    started_at: Some(launched.started_at.clone()),
                    child: Some(launched.child),
                    ..StartedProcess::default()
                },
            )
            .unwrap();
        let outcome = h
            .service
            .request_stop(
                "hl1-won",
                StopOptions {
                    force: true,
                    timeout_ms: Some(10_000),
                },
            )
            .await
            .unwrap();
        assert_eq!(outcome["status"], "stopped");
    }

    #[tokio::test]
    async fn qa_launch_refuses_what_0_4_7_refuses() {
        let install = tempfile::tempdir().unwrap();
        let install_path = install.path().to_string_lossy().into_owned();
        let code = |environment: &Environment| {
            launch(&install_path, environment)
                .err()
                .map(|error| (error.code.as_str().to_owned(), error.message))
        };

        let off = env(&[(EXE_FLAG, "bin/x.exe")]);
        assert_eq!(
            code(&off),
            Some((
                "PROVIDER_UNSUPPORTED".into(),
                "Тестовый запуск доступен только в QA-режиме".into()
            ))
        );

        #[cfg(windows)]
        let absolute = "C:\\Windows\\System32\\cmd.exe";
        #[cfg(unix)]
        let absolute = "/bin/sh";
        assert_eq!(
            code(&qa_env(absolute, "")),
            Some((
                "PATH_INVALID".into(),
                "Тестовый исполняемый файл задаётся путём относительно папки игры".into()
            ))
        );

        let outside = code(&qa_env("../outside.exe", "")).unwrap();
        assert_eq!(outside.0, "PATH_INVALID");
        assert!(
            outside
                .1
                .starts_with("Тестовый исполняемый файл выходит за пределы")
        );

        let missing = code(&qa_env("bin/missing.exe", "")).unwrap();
        assert_eq!(missing.0, "REQUIRED_FILE_MISSING");
        assert!(missing.1.starts_with("Тестовый процесс не найден: "));
    }
}
