//! Самотесты 0.4.7 для запуска игр на Linux (`linux/LaunchService.js`): `linux-selftest.js`
//! (Steam не запущен, шрифты LiveSplit до игры, мост presence, мост к Steam у Ghosting, игра
//! закрылась внутри живой сессии Proton), `hl1-selftest.js` (Linux: инжектор BXT и мост к
//! Steam, параметры Proton, журнал BXT, RInput после BXT и вместе с игрой, патч курсора,
//! зависимости и паки до запуска), `livesplit-overlay-selftest.js` (оверлей в префиксе игры),
//! `ghosting-settings-selftest.js` (Linux: перенос настроек Ghosting перед запуском).
//!
//! Запуск в префиксе и процессы — подделки; `goldsrc-launch.js` — настоящий
//! [`GoldSrcLaunch`](hlsr_platform::goldsrc_launch::GoldSrcLaunch) (сверен с 0.4.7 в `goldsrc.rs`).

mod proton_support;

use hlsr_core::logger::MemoryLogger;
use hlsr_delivery::errors::{Code, DeliveryError};
use hlsr_delivery::ghosting_personalization;
use hlsr_delivery::manifests::{LaunchConfig, launch_config_of};
use hlsr_delivery::providers::BoxFuture;
use hlsr_platform::game_process_service::StartedProcess;
use hlsr_platform::goldsrc_launch::GoldSrcLaunch;
use hlsr_platform::linux::launch_service::{
    LaunchDelivery, LaunchProcesses, LaunchService, LaunchServiceOptions, LaunchSettings,
    LaunchSteam,
};
use hlsr_platform::linux::presence_relay::{self, PresenceRelay, target_of};
use hlsr_platform::linux::proton_runner::{
    LaunchOptions, ProtonBuild, ProtonLaunch, ProtonLauncher, RunnerError, windows_path_of,
};
use hlsr_platform::livesplit_overlay::{self, LiveSplitOverlay, OverlayOptions};
use hlsr_platform::spawn::{Child, ChildController, Env};
use hlsr_platform::steam_locator::SteamInstall;
use proton_support::{Root, Signals, write_file};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn config(game_id: &str) -> &'static LaunchConfig {
    launch_config_of(game_id).unwrap()
}

// ---------------------------------------------------------------- подделки

/// Запуск в префиксе, который видел `ProtonRunner.launch`.
#[derive(Clone)]
struct Launch {
    game_id: String,
    executable: PathBuf,
    args: Vec<String>,
    options: LaunchOptions,
}

type OnLaunch = Box<dyn Fn(&Launch) -> Result<(), RunnerError> + Send + Sync>;

/// Поддельный `protonRunner`: процессы-сессии с PID `100 + n` (или заданным) живут, пока их не
/// завершат.
struct FakeRunner {
    launches: Mutex<Vec<Launch>>,
    sessions: Mutex<Vec<(Child, Arc<ChildController>)>>,
    order: Arc<Mutex<Vec<String>>>,
    on_launch: OnLaunch,
    pid: Option<u32>,
    fonts_fail: bool,
}

impl FakeRunner {
    fn new(order: Arc<Mutex<Vec<String>>>, on_launch: OnLaunch) -> Arc<FakeRunner> {
        Arc::new(FakeRunner {
            launches: Mutex::default(),
            sessions: Mutex::default(),
            order,
            on_launch,
            pid: None,
            fonts_fail: false,
        })
    }

    fn launches(&self) -> Vec<Launch> {
        self.launches.lock().unwrap().clone()
    }

    fn session(&self, index: usize) -> (Child, Arc<ChildController>) {
        self.sessions.lock().unwrap()[index].clone()
    }
}

impl ProtonLauncher for FakeRunner {
    fn launch<'a>(
        &'a self,
        game_id: &'a str,
        executable: &'a Path,
        args: &'a [String],
        options: LaunchOptions,
    ) -> BoxFuture<'a, Result<ProtonLaunch, RunnerError>> {
        Box::pin(async move {
            let launch = Launch {
                game_id: game_id.to_owned(),
                executable: executable.to_path_buf(),
                args: args.to_vec(),
                options,
            };
            self.order.lock().unwrap().push("launch".to_owned());
            let count = {
                let mut launches = self.launches.lock().unwrap();
                launches.push(launch.clone());
                launches.len()
            };
            (self.on_launch)(&launch)?;
            let pid = self.pid.unwrap_or(99 + count as u32);
            let (child, controller) = Child::new(Some(pid), Arc::new(Signals::default()));
            let controller = Arc::new(controller);
            controller.spawned();
            self.sessions
                .lock()
                .unwrap()
                .push((child.clone(), controller));
            Ok(ProtonLaunch {
                child,
                proton: ProtonBuild {
                    path: "/proton".to_owned(),
                    name: "test".to_owned(),
                },
                prefix: "/pfx".to_owned(),
                command: "/test/umu-run".to_owned(),
            })
        })
    }

    fn ensure_live_split_fonts<'a>(
        &'a self,
        game_id: &'a str,
    ) -> BoxFuture<'a, Result<(), RunnerError>> {
        Box::pin(async move {
            self.order.lock().unwrap().push(format!("fonts:{game_id}"));
            if self.fonts_fail {
                return Err(RunnerError::Proton(
                    hlsr_platform::linux::prefix_command::ProtonError::new("regedit упал"),
                ));
            }
            Ok(())
        })
    }

    fn selected(&self) -> Option<ProtonBuild> {
        Some(ProtonBuild {
            path: "/proton".to_owned(),
            name: "test".to_owned(),
        })
    }

    fn umu_path(&self) -> Option<String> {
        Some("/test/umu-run".to_owned())
    }
}

/// Поддельная служба процессов.
#[derive(Default)]
struct FakeProcesses {
    running: Mutex<Vec<String>>,
    cancelled: Mutex<Vec<(String, String)>>,
    exited: Mutex<Vec<(String, u32)>>,
}

impl LaunchProcesses for FakeProcesses {
    fn is_running(&self, game_id: &str) -> bool {
        self.running.lock().unwrap().iter().any(|id| id == game_id)
    }
    fn get_process_state(&self, game_id: &str) -> Value {
        json!({"gameId": game_id, "state": "running"})
    }
    fn mark_starting(&self, _game_id: &str) {}
    fn cancel_starting(&self, game_id: &str, reason: &str) {
        self.cancelled
            .lock()
            .unwrap()
            .push((game_id.to_owned(), reason.to_owned()));
    }
    fn mark_exited(&self, game_id: &str, pid: u32) -> bool {
        self.exited.lock().unwrap().push((game_id.to_owned(), pid));
        self.running.lock().unwrap().retain(|id| id != game_id);
        true
    }
}

/// Поддельная доставка: игра установлена в `install`.
struct FakeDelivery {
    install: Option<PathBuf>,
    registered: Mutex<Vec<(String, Option<i64>)>>,
}

impl LaunchDelivery for FakeDelivery {
    fn is_busy(&self) -> bool {
        false
    }
    fn installed_of(&self, _game_id: &str) -> Result<Value, DeliveryError> {
        match &self.install {
            Some(install) => Ok(json!({"installPath": install.to_string_lossy()})),
            None => panic!("до установки доходить не должно"),
        }
    }
    fn register_started_process(
        &self,
        game_id: &str,
        info: StartedProcess,
    ) -> Result<Value, DeliveryError> {
        self.registered
            .lock()
            .unwrap()
            .push((game_id.to_owned(), info.pid));
        Ok(json!({"state": "running"}))
    }
}

/// Поддельный поиск Steam.
struct FakeSteam {
    install: String,
    running: AtomicBool,
    checks: AtomicUsize,
}

impl FakeSteam {
    fn new(install: &str, running: bool) -> Arc<FakeSteam> {
        Arc::new(FakeSteam {
            install: install.to_owned(),
            running: AtomicBool::new(running),
            checks: AtomicUsize::new(0),
        })
    }
}

impl LaunchSteam for FakeSteam {
    fn locate_steam(&self) -> Result<SteamInstall, DeliveryError> {
        Ok(SteamInstall {
            install_path: self.install.clone(),
            steam_exe: "steam".to_owned(),
            steam_args: Some(Vec::new()),
        })
    }
    fn assert_steam_running(&self, _steam: &SteamInstall) -> Result<(), DeliveryError> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(DeliveryError::new(
                Code::SteamNotRunning,
                "Steam не запущен",
            ))
        }
    }
}

/// Поддельные настройки игры.
struct FakeSettings {
    settings: Mutex<Value>,
    fail_save: AtomicBool,
    events: Arc<Mutex<Vec<(String, String)>>>,
    file: Option<PathBuf>,
}

impl LaunchSettings for FakeSettings {
    fn get(&self, _game_id: &str) -> Value {
        self.settings.lock().unwrap().clone()
    }
    fn save(&self, _game_id: &str, settings: &Value) -> bool {
        let cfg = self
            .file
            .as_ref()
            .map(|file| std::fs::read_to_string(file).unwrap())
            .unwrap_or_default();
        self.events.lock().unwrap().push(("save".to_owned(), cfg));
        if self.fail_save.load(Ordering::SeqCst) {
            return false;
        }
        *self.settings.lock().unwrap() = settings.clone();
        true
    }
}

fn warnings(logger: &MemoryLogger) -> Vec<String> {
    logger
        .take()
        .into_iter()
        .filter(|(level, _)| *level == "warn")
        .map(|(_, m)| m)
        .collect()
}

/// `fakeInstall(root)` из `hl1-selftest.js`.
fn fake_install(root: &Path) -> PathBuf {
    write_file(&root.join("Half-Life/hl.exe"), "MZ", None);
    write_file(
        &root.join("Half-Life/valve_WON/liblist.gam"),
        "game \"Half-Life\"\n",
        None,
    );
    write_file(&root.join("Half-Life/steam_appid.txt"), "220", None);
    write_file(&root.join("Bunnymod XT/Injector.exe"), "MZ", None);
    root.to_path_buf()
}

// ---------------------------------------------------------------- linux-selftest

#[tokio::test]
async fn source_game_does_not_start_without_running_steam() {
    let order: Arc<Mutex<Vec<String>>> = Arc::default();
    let runner = FakeRunner::new(
        order.clone(),
        Box::new(|_| panic!("Proton не должен стартовать")),
    );
    let processes = Arc::new(FakeProcesses::default());
    let service = LaunchService::new(LaunchServiceOptions {
        delivery: Some(Arc::new(FakeDelivery {
            install: None,
            registered: Mutex::default(),
        })),
        processes: Some(processes.clone()),
        runner: Some(runner.clone()),
        steam_locator: Some(FakeSteam::new("/steam", false)),
        logger: Some(Arc::new(MemoryLogger::default())),
        ..LaunchServiceOptions::default()
    });
    for game_id in ["ghosting-renewed", "hl2-4260", "hl2-2707"] {
        let error = service.launch(game_id).await.unwrap_err();
        assert_eq!(error.code, Code::SteamNotRunning, "{game_id}");
        // Флаг запуска снят: повторный запуск снова доходит до проверки Steam.
        assert_eq!(
            service.launch(game_id).await.unwrap_err().code,
            Code::SteamNotRunning
        );
    }
    assert!(runner.launches().is_empty());
    let cancelled: Vec<String> = processes
        .cancelled
        .lock()
        .unwrap()
        .iter()
        .map(|(id, _)| id.clone())
        .collect();
    assert_eq!(
        cancelled,
        [
            "ghosting-renewed",
            "ghosting-renewed",
            "hl2-4260",
            "hl2-4260",
            "hl2-2707",
            "hl2-2707"
        ]
    );
    assert_eq!(
        processes.cancelled.lock().unwrap()[0].1,
        "STEAM_NOT_RUNNING: Steam не запущен"
    );
}

/// Установка Source-игры для `startSource` (exe и `gameinfo.txt`).
fn source_install(root: &Path, game_id: &str) -> PathBuf {
    let install = root.join(game_id);
    let config = config(game_id);
    write_file(&install.join(config.executable), "MZ", None);
    write_file(
        &install.join(config.game_directory).join("gameinfo.txt"),
        "\"GameInfo\"\n{\n}\n",
        None,
    );
    install
}

#[tokio::test]
async fn livesplit_fonts_are_prepared_before_the_game_when_the_timer_starts_with_it() {
    let run = |settings: Option<Value>, fonts_fail: bool| async move {
        let root = Root::new("hlsr-timer-fonts-");
        let install = source_install(&root.path, "ghosting-renewed");
        let order: Arc<Mutex<Vec<String>>> = Arc::default();
        let runner = Arc::new(FakeRunner {
            launches: Mutex::default(),
            sessions: Mutex::default(),
            order: order.clone(),
            on_launch: Box::new(|_| Ok(())),
            pid: Some(4242),
            fonts_fail,
        });
        let logger = Arc::new(MemoryLogger::default());
        let service = LaunchService::new(LaunchServiceOptions {
            delivery: Some(Arc::new(FakeDelivery {
                install: Some(install),
                registered: Mutex::default(),
            })),
            processes: Some(Arc::new(FakeProcesses::default())),
            runner: Some(runner.clone()),
            steam_locator: Some(FakeSteam::new("/steam", true)),
            settings_store: settings.map(|settings| {
                Arc::new(FakeSettings {
                    settings: Mutex::new(settings),
                    fail_save: AtomicBool::new(false),
                    events: Arc::default(),
                    file: None,
                }) as Arc<dyn LaunchSettings>
            }),
            logger: Some(logger.clone()),
            start_check: Some(Duration::from_millis(5)),
            ..LaunchServiceOptions::default()
        });
        let result = service.launch("ghosting-renewed").await.unwrap();
        assert_eq!(result["status"], "started");
        let order = order.lock().unwrap().clone();
        (order, warnings(&logger))
    };
    assert_eq!(
        run(Some(json!({"startTimer": true})), false).await.0,
        ["fonts:ghosting-renewed", "launch"]
    );
    assert_eq!(
        run(Some(json!({"startTimer": false})), false).await.0,
        ["launch"],
        "без таймера префикс не трогается"
    );
    assert_eq!(run(None, false).await.0, ["launch"]);
    let (order, warnings) = run(Some(json!({"startTimer": true})), true).await;
    assert_eq!(
        order,
        ["fonts:ghosting-renewed", "launch"],
        "сбой шрифтов игру не останавливает"
    );
    assert!(
        warnings.iter().any(|m| m.contains("шрифты LiveSplit")),
        "{warnings:?}"
    );
}

// Часы остановлены: пауза `start_check` истекает, только когда рантайм простаивает, то есть после
// того как задача выхода сессии отработала, как бы ни была нагружена машина.
#[tokio::test(start_paused = true)]
async fn started_game_is_registered_and_watched_and_an_early_exit_fails_the_launch() {
    let root = Root::new("hlsr-launch-started-");
    let install = source_install(&root.path, "hl2-4260");
    let runner = FakeRunner::new(Arc::default(), Box::new(|_| Ok(())));
    let delivery = Arc::new(FakeDelivery {
        install: Some(install.clone()),
        registered: Mutex::default(),
    });
    let processes = Arc::new(FakeProcesses::default());
    let logger = Arc::new(MemoryLogger::default());
    let service = LaunchService::new(LaunchServiceOptions {
        delivery: Some(delivery.clone()),
        processes: Some(processes.clone()),
        runner: Some(runner.clone()),
        steam_locator: Some(FakeSteam::new("/steam", true)),
        logger: Some(logger.clone()),
        logs_dir: Some(root.path.join("logs")),
        start_check: Some(Duration::from_millis(5)),
        ..LaunchServiceOptions::default()
    });
    let result = service.launch("hl2-4260").await.unwrap();
    assert_eq!(
        result,
        json!({"status": "started", "process": {"state": "running"}})
    );
    assert_eq!(
        *delivery.registered.lock().unwrap(),
        [("hl2-4260".to_owned(), Some(100))]
    );
    let launch = &runner.launches()[0];
    assert_eq!(launch.executable, install.join("hl2.exe"));
    assert_eq!(launch.options.cwd.as_deref(), Some(install.as_path()));
    assert_eq!(
        launch.options.log_file,
        Some(root.path.join("logs").join("proton-hl2-4260.log"))
    );
    assert!(
        launch
            .args
            .starts_with(&["-steam".to_owned(), "-game".to_owned(), "hl2".to_owned()])
    );
    assert!(
        launch.options.steam_bridge.is_none(),
        "сборки HL2 идут через umu"
    );
    assert!(
        logger
            .take()
            .iter()
            .any(|(_, m)| m == "[launch] hl2-4260: Proton test, префикс /pfx")
    );

    // Proton вышел за 1,5 с — запуск не удался, игра не регистрируется.
    let early = FakeRunner::new(Arc::default(), Box::new(|_| Ok(())));
    let service = LaunchService::new(LaunchServiceOptions {
        delivery: Some(delivery.clone()),
        processes: Some(processes.clone()),
        runner: Some(early.clone()),
        steam_locator: Some(FakeSteam::new("/steam", true)),
        logger: Some(logger.clone()),
        start_check: Some(Duration::from_millis(30)),
        ..LaunchServiceOptions::default()
    });
    let exit = {
        let early = early.clone();
        tokio::spawn(async move {
            while early.sessions.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
            early.session(0).1.exit(Some(3), None);
        })
    };
    let error = service.launch("hl2-4260").await.unwrap_err();
    exit.await.unwrap();
    assert_eq!(error.code, Code::LaunchFailed);
    assert_eq!(
        error.message,
        "Proton завершился сразу после запуска (код 3)"
    );
    assert_eq!(delivery.registered.lock().unwrap().len(), 1);
    assert_eq!(
        processes.cancelled.lock().unwrap().last().unwrap().1,
        "LAUNCH_FAILED: Proton завершился сразу после запуска (код 3)"
    );
}

#[tokio::test]
async fn presence_relay_starts_after_the_game_which_gets_the_pipe_and_token_but_not_the_port() {
    let root = Root::new("hlsr-presence-relay-");
    let resources = root.path.join("resources");
    write_file(
        &resources
            .join(presence_relay::RESOURCE_DIRECTORY)
            .join(presence_relay::EXECUTABLE),
        "relay",
        None,
    );
    let pipe = "\\\\.\\pipe\\hlsr-presence-0123456789abcdef";
    let token = "b".repeat(64);
    let environment: Env = [
        ("HLSR_PRESENCE_PIPE", pipe.to_owned()),
        ("HLSR_PRESENCE_TOKEN", token.clone()),
        ("HLSR_PRESENCE_PORT", "40123".to_owned()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    assert!(target_of(Some(&Env::new())).is_none());
    let mut other = environment.clone();
    other.insert(
        "HLSR_PRESENCE_PIPE".to_owned(),
        "\\\\.\\pipe\\other".to_owned(),
    );
    assert!(target_of(Some(&other)).is_none());
    let mut zero = environment.clone();
    zero.insert("HLSR_PRESENCE_PORT".to_owned(), "0".to_owned());
    assert!(target_of(Some(&zero)).is_none());

    let running = Arc::new(AtomicBool::new(false));
    let killed: Arc<Mutex<Vec<u32>>> = Arc::default();
    let make = |status: Option<u32>| {
        let runner = Arc::new(FakeRunner {
            launches: Mutex::default(),
            sessions: Mutex::default(),
            order: Arc::default(),
            on_launch: Box::new(move |launch: &Launch| {
                if let Some(status) = status {
                    let file = launch.executable.parent().unwrap().join("relay.status");
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        std::fs::write(file, status.to_string()).unwrap();
                    });
                }
                Ok(())
            }),
            pid: Some(5151),
            fonts_fail: false,
        });
        let env = environment.clone();
        let flag = running.clone();
        let sink = killed.clone();
        let service = LaunchService::new(LaunchServiceOptions {
            presence_relay: Some(Arc::new(PresenceRelay::new(
                Some(root.path.join("data")),
                Some(resources.clone()),
            ))),
            extra_env_provider: Some(Arc::new(move || Some(env.clone()))),
            // Ответ помощника ждём долго (под нагрузкой 20 мс подделки растягиваются); без
            // ответа — коротко, итог тот же: «не ответил».
            presence_wait: Some(Duration::from_millis(if status.is_some() {
                30_000
            } else {
                300
            })),
            presence_settle: Some(Duration::ZERO),
            bxt_poll: Some(Duration::from_millis(5)),
            process_running: Some(Arc::new(move |name: &str| {
                flag.load(Ordering::SeqCst) && name == "hl2.exe"
            })),
            kill_process_group: Some(Arc::new(move |pid| sink.lock().unwrap().push(pid))),
            logger: Some(Arc::new(MemoryLogger::default())),
            runner: Some(runner.clone()),
            ..LaunchServiceOptions::default()
        });
        (service, runner)
    };
    // Только играм с client.dll, который знает про канал.
    let (service, runner) = make(Some(0));
    assert!(service.presence_target(config("hl2-4260")).is_none());
    let target = service.presence_target(config("ghosting-renewed")).unwrap();
    assert_eq!(target.game_env.len(), 2);
    assert_eq!(target.game_env["HLSR_PRESENCE_PIPE"], pipe);
    assert_eq!(target.game_env["HLSR_PRESENCE_TOKEN"], token);

    // Пока процесса игры нет, помощник не запускается.
    let (game, game_control) = Child::new(Some(1), Arc::new(Signals::default()));
    game_control.spawned();
    let started = {
        let service = service.clone();
        let target = target.clone();
        let game = game.clone();
        tokio::spawn(async move {
            service
                .start_presence_relay(
                    "ghosting-renewed",
                    config("ghosting-renewed"),
                    &target,
                    &game,
                )
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(runner.launches().is_empty());
    running.store(true, Ordering::SeqCst);
    assert!(started.await.unwrap());
    let launches = runner.launches();
    assert_eq!(launches.len(), 1);
    assert_eq!(
        launches[0].executable.file_name().unwrap(),
        presence_relay::EXECUTABLE
    );
    assert!(
        launches[0].executable.starts_with(root.path.join("data")),
        "помощник копируется в папку данных"
    );
    assert_eq!(
        launches[0].args[..6],
        ["--pipe", pipe, "--port", "40123", "--name", "hl2.exe"]
    );
    assert!(
        !launches[0].args.iter().any(|arg| arg.contains(&token)),
        "токен помощнику не передаётся"
    );
    assert_eq!(launches[0].options.env["PROTON_HIDE_PROCESS_WINDOW"], "1");
    assert!(killed.lock().unwrap().is_empty());
    // Игра закрылась — сессия помощника тоже.
    game_control.exit(Some(0), None);
    eventually("сессия помощника закрыта", || {
        !killed.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(*killed.lock().unwrap(), [5151]);

    // Игра вышла раньше, чем появилась, — помощник не нужен.
    running.store(false, Ordering::SeqCst);
    let (service, runner) = make(Some(0));
    let (early, early_control) = Child::new(Some(2), Arc::new(Signals::default()));
    early_control.exit(Some(1), None);
    assert!(
        !service
            .start_presence_relay(
                "ghosting-renewed",
                config("ghosting-renewed"),
                &target,
                &early
            )
            .await
    );
    assert!(runner.launches().is_empty());

    // Помощник не ответил — игра идёт, его сессия закрывается.
    running.store(true, Ordering::SeqCst);
    killed.lock().unwrap().clear();
    let (service, _) = make(None);
    let (alive, alive_control) = Child::new(Some(3), Arc::new(Signals::default()));
    alive_control.spawned();
    assert!(
        !service
            .start_presence_relay(
                "ghosting-renewed",
                config("ghosting-renewed"),
                &target,
                &alive
            )
            .await
    );
    assert_eq!(*killed.lock().unwrap(), [5151]);
}

#[test]
fn ghosting_goes_through_the_steam_bridge_and_is_visible_under_its_app_id() {
    let service = LaunchService::new(LaunchServiceOptions {
        steam_locator: Some(FakeSteam::new("/steam", true)),
        logger: Some(Arc::new(MemoryLogger::default())),
        ..LaunchServiceOptions::default()
    });
    let bridge = service
        .steam_bridge_of("/games/ghosting-renewed", config("ghosting-renewed"))
        .unwrap()
        .unwrap();
    assert_eq!(bridge.app_id, 243730);
    assert_eq!(bridge.steam_path, "/steam");
    assert_eq!(
        bridge.install_path.as_deref(),
        Some("/games/ghosting-renewed")
    );
    assert!(
        service
            .steam_bridge_of("/games/hl2", config("hl2-4260"))
            .unwrap()
            .is_none(),
        "сборки HL2 пока идут через umu"
    );
}

// Часы остановлены: опросы слежения (5 мс) и паузы теста идут по виртуальному времени в порядке
// сроков, а не по скорости машины.
#[tokio::test(start_paused = true)]
async fn game_counts_as_closed_when_its_process_is_gone_even_if_livesplit_keeps_the_session() {
    let processes = Arc::new(FakeProcesses::default());
    processes
        .running
        .lock()
        .unwrap()
        .push("hl2-4260".to_owned());
    let alive = Arc::new(AtomicBool::new(false));
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let (flag, log) = (alive.clone(), seen.clone());
    let service = LaunchService::new(LaunchServiceOptions {
        processes: Some(processes.clone()),
        game_watch: Some(Duration::from_millis(5)),
        game_process_running: Some(Arc::new(move |name: &str, game_id: &str| {
            log.lock().unwrap().push(format!("{name}@{game_id}"));
            flag.load(Ordering::SeqCst)
        })),
        logger: Some(Arc::new(MemoryLogger::default())),
        ..LaunchServiceOptions::default()
    });
    let (child, control) = Child::new(Some(5150), Arc::new(Signals::default()));
    control.spawned();
    service.watch_game_process("hl2-4260", config("hl2-4260"), child);
    // Пока процесс игры не появился, выход определяет только сессия.
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(processes.is_running("hl2-4260"));
    alive.store(true, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(processes.is_running("hl2-4260"));
    alive.store(false, Ordering::SeqCst);
    eventually("игра закрыта по процессу", || {
        !processes.is_running("hl2-4260")
    })
    .await;
    assert_eq!(
        *processes.exited.lock().unwrap(),
        [("hl2-4260".to_owned(), 5150)]
    );
    assert_eq!(seen.lock().unwrap()[0], "hl2.exe@hl2-4260");
    // Сессия Proton живёт дальше, её выход ничего не ломает.
    control.exit(Some(0), None);
    tokio::task::yield_now().await;
    assert_eq!(processes.exited.lock().unwrap().len(), 1);

    // Сессия вышла раньше — слежение прекращается.
    processes
        .running
        .lock()
        .unwrap()
        .push("hl2-2707".to_owned());
    let (child, control) = Child::new(Some(6000), Arc::new(Signals::default()));
    alive.store(true, Ordering::SeqCst);
    service.watch_game_process("hl2-2707", config("hl2-2707"), child);
    tokio::time::sleep(Duration::from_millis(20)).await;
    control.exit(Some(0), None);
    alive.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(
        processes.exited.lock().unwrap().len(),
        1,
        "слежение снято вместе с сессией"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn case_twin_next_to_the_library_is_found_before_the_game_starts() {
    let root = Root::new("hlsr-case-");
    let install = root.path.join("Games/HLSR/Games/ghosting-renewed");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::create_dir_all(root.path.join("Games/hlsr")).unwrap();
    let runner = FakeRunner::new(
        Arc::default(),
        Box::new(|_| panic!("Proton не должен стартовать")),
    );
    let service = LaunchService::new(LaunchServiceOptions {
        delivery: Some(Arc::new(FakeDelivery {
            install: Some(install),
            registered: Mutex::default(),
        })),
        processes: Some(Arc::new(FakeProcesses::default())),
        runner: Some(runner.clone()),
        steam_locator: Some(FakeSteam::new("/steam", true)),
        logger: Some(Arc::new(MemoryLogger::default())),
        ..LaunchServiceOptions::default()
    });
    let error = service.launch("ghosting-renewed").await.unwrap_err();
    assert_eq!(error.code, Code::LibraryPathCaseConflict);
    assert_eq!(
        error.details.as_ref().unwrap()["twin"],
        root.path.join("Games/hlsr").to_string_lossy().as_ref()
    );
    assert!(runner.launches().is_empty());
}

#[test]
fn availability_reports_missing_proton_and_umu() {
    let service = LaunchService::new(LaunchServiceOptions {
        runner: Some(FakeRunner::new(Arc::default(), Box::new(|_| Ok(())))),
        ..LaunchServiceOptions::default()
    });
    assert_eq!(
        service.describe_availability("hl2-4260"),
        json!({"ok": true, "code": null})
    );
    assert_eq!(
        service.describe_availability("unknown"),
        json!({"ok": false, "code": "LAUNCH_CONFIG_INVALID"})
    );
    let none = LaunchService::new(LaunchServiceOptions::default());
    assert_eq!(
        none.describe_availability("hl2-4260"),
        json!({"ok": false, "code": "PROTON_NOT_INSTALLED"})
    );
}

// ---------------------------------------------------------------- hl1-selftest (Linux)

fn hl1_service(steam: Arc<FakeSteam>, logs_dir: Option<PathBuf>) -> LaunchService {
    LaunchService::new(LaunchServiceOptions {
        steam_locator: Some(steam),
        goldsrc: Some(Arc::new(GoldSrcLaunch)),
        logs_dir,
        logger: Some(Arc::new(MemoryLogger::default())),
        ..LaunchServiceOptions::default()
    })
}

fn settings(value: Value) -> Value {
    value
}

#[test]
fn hl1_runs_hl_exe_through_the_injector_and_proton_gets_the_steam_bridge() {
    let root = Root::new("hlsr-hl1-");
    let install = fake_install(&root.path.join("hl1-won"));
    let steam = FakeSteam::new("/home/user/.local/share/Steam", true);
    let service = hl1_service(steam.clone(), None);
    let config = config("hl1-won");
    let plan = service
        .goldsrc_launch(
            &install,
            config,
            Some(&settings(json!({"launchArgs": "-w 1280 -h 960"}))),
        )
        .unwrap();
    assert_eq!(
        plan.executable,
        install.join("Bunnymod XT").join("Injector.exe")
    );
    assert_eq!(
        plan.argv[0],
        windows_path_of(&install.join("Half-Life").join("hl.exe").to_string_lossy())
    );
    let mut expected: Vec<String> = config.arguments.iter().map(|a| (*a).to_owned()).collect();
    expected.extend(["-w", "1280", "-h", "960"].map(str::to_owned));
    assert_eq!(plan.argv[1..], expected);
    assert!(
        !plan.argv.iter().any(|arg| arg == "-insecure"),
        "переключатели Source не попадают в HL1"
    );
    assert_eq!(
        plan.options.cwd.as_deref(),
        Some(install.join("Half-Life").as_path())
    );
    assert_eq!(plan.options.env["BXT_DISABLE_VSYNC"], "1");
    assert_eq!(plan.options.env["PROTON_ENABLE_WAYLAND"], "0");
    assert_eq!(plan.options.env["__GL_SYNC_TO_VBLANK"], "0");
    let bridge = plan.options.steam_bridge.as_ref().unwrap();
    assert_eq!(
        (bridge.app_id, bridge.steam_path.as_str()),
        (70, "/home/user/.local/share/Steam")
    );
    assert_eq!(
        bridge.install_path.as_deref(),
        Some(install.to_string_lossy().as_ref())
    );
    assert_eq!(
        steam.checks.load(Ordering::SeqCst),
        1,
        "без запущенного Steam игра не стартует"
    );
    assert_eq!(
        std::fs::read_to_string(install.join("Half-Life/steam_appid.txt")).unwrap(),
        "70\n"
    );
    let forbidden = service
        .goldsrc_launch(
            &install,
            config,
            Some(&json!({"launchArgs": "-game cstrike"})),
        )
        .unwrap();
    let trusted: Vec<String> = config.arguments.iter().map(|a| (*a).to_owned()).collect();
    assert_eq!(
        forbidden.argv[1..],
        trusted,
        "игрок не подменяет мод через -game"
    );
}

#[test]
fn hl1_player_proton_options_reach_the_launch() {
    let root = Root::new("hlsr-hl1-");
    let install = fake_install(&root.path.join("hl1-won"));
    let service = hl1_service(FakeSteam::new("/steam", true), None);
    let config = config("hl1-won");
    let plan = service
        .goldsrc_launch(&install, config, Some(&json!({"protonOptions": "PROTON_USE_WINED3D=1 __GL_SYNC_TO_VBLANK=1 mangohud %command% -nojoy"})))
        .unwrap();
    assert_eq!(plan.options.env["PROTON_USE_WINED3D"], "1");
    assert_eq!(
        plan.options.env["__GL_SYNC_TO_VBLANK"], "1",
        "переменная игрока важнее значения лаунчера, как в Steam"
    );
    assert_eq!(plan.options.env["BXT_DISABLE_VSYNC"], "1");
    assert_eq!(plan.options.wrapper, ["mangohud"]);
    assert_eq!(plan.argv.last().unwrap(), "-nojoy");
    let broken = service
        .goldsrc_launch(
            &install,
            config,
            Some(&json!({"protonOptions": "WINEPREFIX=/x mangohud"})),
        )
        .unwrap();
    assert!(
        broken.options.wrapper.is_empty(),
        "строка с защищённой переменной не применяется"
    );
}

#[test]
fn hl1_bxt_log_goes_to_the_launcher_logs_and_the_old_one_is_removed() {
    let root = Root::new("hlsr-hl1-");
    let install = fake_install(&root.path.join("hl1-won"));
    let logs = root.path.join("logs");
    write_file(&logs.join("bxt.log"), "прошлый запуск\n", None);
    let with_logs = hl1_service(FakeSteam::new("/steam", true), Some(logs.clone()));
    let plan = with_logs
        .goldsrc_launch(&install, config("hl1-won"), None)
        .unwrap();
    assert_eq!(
        plan.options.env["BXT_LOGFILE"],
        windows_path_of(&logs.join("bxt.log").to_string_lossy())
    );
    assert!(
        !logs.join("bxt.log").exists(),
        "BXT дописывает в конец, поэтому старый лог убран"
    );
    let without = hl1_service(FakeSteam::new("/steam", true), None);
    let plan = without
        .goldsrc_launch(&install, config("hl1-won"), None)
        .unwrap();
    assert!(!plan.options.env.contains_key("BXT_LOGFILE"));
}

#[test]
fn hl1_without_steam_or_injector_is_refused_with_a_clear_error() {
    let root = Root::new("hlsr-hl1-");
    let install = fake_install(&root.path.join("hl1-won"));
    let offline = hl1_service(FakeSteam::new("/steam", false), None);
    assert_eq!(
        offline
            .goldsrc_launch(&install, config("hl1-won"), None)
            .unwrap_err()
            .code,
        Code::SteamNotRunning
    );
    std::fs::remove_file(install.join("Bunnymod XT/Injector.exe")).unwrap();
    assert_eq!(
        offline
            .goldsrc_launch(&install, config("hl1-won"), None)
            .unwrap_err()
            .code,
        Code::GameExecutableMissing
    );
}

/// `rinputRig(t, settings, options)`.
struct RInputRig {
    _root: Root,
    install: PathBuf,
    service: LaunchService,
    runner: Arc<FakeRunner>,
    killed: Arc<Mutex<Vec<u32>>>,
    logger: Arc<MemoryLogger>,
}

struct RInputOptions {
    bxt_injected: Arc<dyn Fn() -> bool + Send + Sync>,
    bxt_wait: Duration,
    patcher: Option<hlsr_platform::linux::launch_service::RInputMenuPatcher>,
    patch_wait: Option<Duration>,
    rinput_file: bool,
}

impl Default for RInputOptions {
    fn default() -> Self {
        RInputOptions {
            bxt_injected: Arc::new(|| true),
            bxt_wait: Duration::from_millis(30_000),
            patcher: None,
            patch_wait: None,
            rinput_file: true,
        }
    }
}

fn rinput_rig(options: RInputOptions) -> RInputRig {
    let root = Root::new("hlsr-hl1-");
    let install = fake_install(&root.path.join("hl1-won"));
    if options.rinput_file {
        write_file(&install.join("RInput/RInput.exe"), "MZ", None);
    }
    let runner = FakeRunner::new(Arc::default(), Box::new(|_| Ok(())));
    let killed: Arc<Mutex<Vec<u32>>> = Arc::default();
    let sink = killed.clone();
    let logger = Arc::new(MemoryLogger::default());
    let patcher = options.patcher.unwrap_or_else(|| {
        Arc::new(|_: &str| {
            Box::pin(async { Err::<String, String>("патч не включали".to_owned()) })
                as BoxFuture<'static, Result<String, String>>
        })
    });
    let service = LaunchService::new(LaunchServiceOptions {
        runner: Some(runner.clone()),
        steam_locator: Some(FakeSteam::new("/steam", true)),
        goldsrc: Some(Arc::new(GoldSrcLaunch)),
        kill_process_group: Some(Arc::new(move |pid| sink.lock().unwrap().push(pid))),
        logger: Some(logger.clone()),
        bxt_injected: Some(options.bxt_injected),
        bxt_poll: Some(Duration::from_millis(1)),
        bxt_wait: Some(options.bxt_wait),
        rinput_menu_patcher: Some(patcher),
        rinput_patch_wait: options.patch_wait,
        ..LaunchServiceOptions::default()
    });
    RInputRig {
        _root: root,
        install,
        service,
        runner,
        killed,
        logger,
    }
}

/// Отрицательные проверки («ничего не произошло»): дать фоновым задачам отработать.
async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(5)).await;
}

/// Положительные проверки: ждать, пока условие не выполнится (до 30 с, шаг 2 мс), а не гадать
/// паузой, хватит ли ей нагруженной машине.
async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    for _ in 0..15_000 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    panic!("не дождались: {what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hl1_rinput_waits_for_hl_exe_in_the_same_prefix_and_stops_with_the_game() {
    let rig = rinput_rig(RInputOptions::default());
    rig.service
        .start_goldsrc("hl1-won", &rig.install, config("hl1-won"), None)
        .await
        .unwrap();
    rig.service.rinput_started().await;
    let launches = rig.runner.launches();
    assert_eq!(launches.len(), 2, "игра и RInput");
    let (game, rinput) = (&launches[0], &launches[1]);
    assert_eq!(rinput.game_id, "hl1-won", "тот же префикс, что у игры");
    assert_eq!(
        rinput.executable,
        rig.install.join("RInput").join("RInput.exe")
    );
    assert_eq!(rinput.args, ["hl.exe"]);
    assert_eq!(
        rinput.options.steam_bridge, game.options.steam_bridge,
        "тот же мост Proton, что у игры"
    );
    assert_eq!(
        rinput.options.env["PROTON_HIDE_PROCESS_WINDOW"], "1",
        "консоль RInput не отбирает фокус у игры"
    );
    assert!(rinput.options.skip_fonts);
    rig.runner.session(0).1.exit(Some(0), None);
    eventually(
        "ждущий RInput закрыт вместе с игрой",
        || !rig.killed.lock().unwrap().is_empty(),
    )
    .await;
    assert_eq!(
        *rig.killed.lock().unwrap(),
        [rig.runner.session(1).0.pid().unwrap()],
        "ждущий RInput не переживает игру"
    );

    let done = rinput_rig(RInputOptions::default());
    done.service
        .start_goldsrc("hl1-won", &done.install, config("hl1-won"), None)
        .await
        .unwrap();
    done.service.rinput_started().await;
    done.runner.session(1).1.exit(Some(0), None);
    let mut log = Vec::new();
    eventually("выход RInput записан в журнал", || {
        log.extend(done.logger.take());
        log.iter()
            .any(|(_, m)| m == "[launch] RInput завершился (код 0)")
    })
    .await;
    done.runner.session(0).1.exit(Some(0), None);
    settle().await;
    assert!(
        done.killed.lock().unwrap().is_empty(),
        "завершившийся RInput не трогаем"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hl1_rinput_waits_until_the_bxt_injector_is_done() {
    let injected = Arc::new(AtomicBool::new(false));
    let flag = injected.clone();
    let rig = rinput_rig(RInputOptions {
        bxt_injected: Arc::new(move || flag.load(Ordering::SeqCst)),
        ..RInputOptions::default()
    });
    rig.service
        .start_goldsrc("hl1-won", &rig.install, config("hl1-won"), None)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        rig.runner.launches().len(),
        1,
        "пока BXT внедряется, RInput не лезет в игру"
    );
    injected.store(true, Ordering::SeqCst);
    rig.service.rinput_started().await;
    assert_eq!(rig.runner.launches().len(), 2);

    let closed = rinput_rig(RInputOptions {
        bxt_injected: Arc::new(|| false),
        ..RInputOptions::default()
    });
    closed
        .service
        .start_goldsrc("hl1-won", &closed.install, config("hl1-won"), None)
        .await
        .unwrap();
    closed.runner.session(0).1.exit(Some(1), None);
    closed.service.rinput_started().await;
    assert_eq!(
        closed.runner.launches().len(),
        1,
        "игра закрылась до BXT: RInput не нужен"
    );

    let slow = rinput_rig(RInputOptions {
        bxt_injected: Arc::new(|| false),
        bxt_wait: Duration::from_millis(10),
        ..RInputOptions::default()
    });
    slow.service
        .start_goldsrc("hl1-won", &slow.install, config("hl1-won"), None)
        .await
        .unwrap();
    slow.service.rinput_started().await;
    assert_eq!(
        slow.runner.launches().len(),
        2,
        "без BXT RInput всё равно запускается"
    );
    assert!(
        warnings(&slow.logger)
            .iter()
            .any(|m| m.contains("Bunnymod XT не появился"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hl1_disabled_or_missing_rinput_does_not_block_the_game() {
    let off = rinput_rig(RInputOptions::default());
    off.service
        .start_goldsrc(
            "hl1-won",
            &off.install,
            config("hl1-won"),
            Some(&json!({"rinput": false})),
        )
        .await
        .unwrap();
    off.service.rinput_started().await;
    assert_eq!(off.runner.launches().len(), 1);
    let missing = rinput_rig(RInputOptions {
        rinput_file: false,
        ..RInputOptions::default()
    });
    missing
        .service
        .start_goldsrc("hl1-won", &missing.install, config("hl1-won"), None)
        .await
        .unwrap();
    missing.service.rinput_started().await;
    assert_eq!(missing.runner.launches().len(), 1);
    assert!(
        warnings(&missing.logger)
            .iter()
            .any(|m| m.contains("RInput не найден"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hl1_rinput_menu_cursor_is_patched_only_when_enabled() {
    let calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let statuses = Arc::new(Mutex::new(vec!["waiting", "waiting", "patched"]));
    let (log, list) = (calls.clone(), statuses.clone());
    let patcher: hlsr_platform::linux::launch_service::RInputMenuPatcher =
        Arc::new(move |name: &str| {
            log.lock().unwrap().push(name.to_owned());
            let status = list.lock().unwrap().remove(0).to_owned();
            Box::pin(async move { Ok::<String, String>(status) })
                as BoxFuture<'static, Result<String, String>>
        });
    let rig = rinput_rig(RInputOptions {
        patcher: Some(patcher),
        ..RInputOptions::default()
    });
    rig.service
        .start_goldsrc(
            "hl1-won",
            &rig.install,
            config("hl1-won"),
            Some(&json!({"rinputMenuFix": true})),
        )
        .await
        .unwrap();
    rig.service.rinput_started().await;
    assert_eq!(
        *calls.lock().unwrap(),
        ["hl.exe", "hl.exe", "hl.exe"],
        "ждём, пока RInput поставит перехваты"
    );
    assert!(
        rig.logger
            .take()
            .iter()
            .any(|(_, m)| m.contains("курсор RInput в меню исправлен"))
    );

    let off = rinput_rig(RInputOptions::default());
    off.service
        .start_goldsrc("hl1-won", &off.install, config("hl1-won"), None)
        .await
        .unwrap();
    off.service.rinput_started().await;
    assert!(
        warnings(&off.logger).is_empty(),
        "по умолчанию память игры не трогаем"
    );

    let unsupported: hlsr_platform::linux::launch_service::RInputMenuPatcher =
        Arc::new(|_: &str| {
            Box::pin(async { Ok::<String, String>("unsupported".to_owned()) })
                as BoxFuture<'static, Result<String, String>>
        });
    let other = rinput_rig(RInputOptions {
        patcher: Some(unsupported),
        ..RInputOptions::default()
    });
    other
        .service
        .start_goldsrc(
            "hl1-won",
            &other.install,
            config("hl1-won"),
            Some(&json!({"rinputMenuFix": true})),
        )
        .await
        .unwrap();
    other.service.rinput_started().await;
    assert!(
        warnings(&other.logger)
            .iter()
            .any(|m| m.contains("незнакомая версия RInput"))
    );
    assert_eq!(
        other.runner.launches().len(),
        2,
        "игра и RInput работают и без патча"
    );

    let waiting: hlsr_platform::linux::launch_service::RInputMenuPatcher = Arc::new(|_: &str| {
        Box::pin(async { Ok::<String, String>("waiting".to_owned()) })
            as BoxFuture<'static, Result<String, String>>
    });
    let slow = rinput_rig(RInputOptions {
        patcher: Some(waiting),
        patch_wait: Some(Duration::from_millis(5)),
        ..RInputOptions::default()
    });
    slow.service
        .start_goldsrc(
            "hl1-won",
            &slow.install,
            config("hl1-won"),
            Some(&json!({"rinputMenuFix": true})),
        )
        .await
        .unwrap();
    slow.service.rinput_started().await;
    assert!(
        warnings(&slow.logger)
            .iter()
            .any(|m| m.contains("не перехватил курсор"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hl1_dependencies_and_packs_are_applied_before_the_launch_and_reported() {
    let root = Root::new("hlsr-hl1-");
    let install = fake_install(&root.path.join("hl1-won"));
    let order: Arc<Mutex<Vec<String>>> = Arc::default();
    let runner = FakeRunner::new(order.clone(), Box::new(|_| Ok(())));
    let (deps, packs) = (order.clone(), order.clone());
    let service = LaunchService::new(LaunchServiceOptions {
        runner: Some(runner),
        steam_locator: Some(FakeSteam::new("/steam", true)),
        goldsrc: Some(Arc::new(GoldSrcLaunch)),
        logger: Some(Arc::new(MemoryLogger::default())),
        dependency_updater: Some(Arc::new(move |_: &Path, _: &'static LaunchConfig| {
            deps.lock().unwrap().push("deps".to_owned());
            Box::pin(async { json!([{"id": "bxt", "status": "updated"}]) })
                as BoxFuture<'static, Value>
        })),
        pack_applier: Some(Arc::new(
            move |_: &Path, _: &'static LaunchConfig, settings: &Value| {
                packs.lock().unwrap().push(format!(
                    "packs:{}",
                    settings["packs"]["hud"].as_str().unwrap_or("")
                ));
                Box::pin(async { json!({"status": "applied", "packs": ["ag-hud"]}) })
                    as BoxFuture<'static, Value>
            },
        )),
        bxt_injected: Some(Arc::new(|| true)),
        ..LaunchServiceOptions::default()
    });
    let started = service
        .start_goldsrc(
            "hl1-won",
            &install,
            config("hl1-won"),
            Some(&json!({"packs": {"hud": "ag-hud"}, "rinput": false})),
        )
        .await
        .unwrap();
    assert_eq!(*order.lock().unwrap(), ["deps", "packs:ag-hud", "launch"]);
    assert_eq!(
        started.dependencies,
        Some(json!([{"id": "bxt", "status": "updated"}]))
    );
    assert_eq!(started.packs.unwrap()["status"], "applied");
    assert_eq!(
        started.executable_path,
        install.join("Half-Life").join("hl.exe")
    );
}

// ---------------------------------------------------------------- оверлей LiveSplit (Linux)

/// «Оверлей LiveSplit на Linux: игра получает замену DLL, свои замены пользователя остаются».
#[test]
fn livesplit_overlay_proxy_goes_to_the_game_folder_with_a_wine_override() {
    let root = Root::new("hlsr-overlay-");
    let resources = root.path.join("resources");
    for name in livesplit_overlay::FILES_LINUX {
        let mut bytes = format!("{name} v1 ").into_bytes();
        bytes.extend_from_slice(livesplit_overlay::PROXY_MARKER);
        write_file(
            &resources
                .join(livesplit_overlay::RESOURCE_DIRECTORY)
                .join(name),
            bytes,
            None,
        );
    }
    let overlay = Arc::new(LiveSplitOverlay::new(OverlayOptions {
        base_dir: Some(root.path.join("data")),
        resources_path: Some(resources),
        platform: Some(hlsr_core::js::Platform::Linux),
        ..OverlayOptions::default()
    }));
    let service = LaunchService::new(LaunchServiceOptions {
        live_split_overlay: Some(overlay),
        logger: Some(Arc::new(MemoryLogger::default())),
        ..LaunchServiceOptions::default()
    });
    let install = root.path.join("game");
    std::fs::create_dir_all(install.join("bin")).unwrap();
    let prepared = service.prepare_overlay(
        "ghosting-renewed",
        &install,
        config("ghosting-renewed"),
        Some(&json!({"liveSplitOverlay": {"enabled": true}})),
    );
    assert!(install.join("d3d9.dll").exists());
    let mut options = LaunchOptions::default();
    options
        .env
        .insert("WINEDLLOVERRIDES".to_owned(), "dxgi=n".to_owned());
    options.env.insert("OTHER".to_owned(), "1".to_owned());
    LaunchService::apply_overlay(&mut options, prepared.as_ref());
    assert_eq!(options.env["WINEDLLOVERRIDES"], "dxgi=n;d3d9=n,b");
    assert_eq!(options.env["OTHER"], "1");
    let mut untouched = LaunchOptions::default();
    untouched.env.insert("A".to_owned(), "1".to_owned());
    LaunchService::apply_overlay(&mut untouched, None);
    assert_eq!(untouched.env.len(), 1);
}
// ---------------------------------------------------------------- ghosting-settings (Linux)

struct GhostingRig {
    _root: Root,
    service: LaunchService,
    store: Arc<FakeSettings>,
    events: Arc<Mutex<Vec<(String, String)>>>,
    install: PathBuf,
}

fn ghosting_rig() -> GhostingRig {
    let root = Root::new("hlsr-ghosting-settings-");
    let install = root.path.join("install");
    let cfg = install.join(ghosting_personalization::CFG_DIR);
    std::fs::create_dir_all(&cfg).unwrap();
    let file = cfg.join(ghosting_personalization::PERSONALIZATION_FILE);
    write_file(
        &install.join("hl2.exe"),
        "test executable; never spawned",
        None,
    );
    write_file(
        &install.join("ghostingmod/gameinfo.txt"),
        "\"GameInfo\"\n{\nSteamAppId 243730\n}\n",
        None,
    );
    write_file(&install.join("steam_appid.txt"), "243730\n", None);
    write_file(
        &cfg.join("config.cfg"),
        "viewmodel_fov \"84\"\ngh_timer \"1\"\ngh_cross_size \"30\"\n",
        None,
    );
    let old_file = "// Original personalization\ngh_cross_size \"8\"\n".to_owned();
    std::fs::write(&file, &old_file).unwrap();
    let mut personalization = ghosting_personalization::default_personalization().to_value();
    personalization["hudEnabled"] = json!(true);
    personalization["crossSize"] = json!(8);
    personalization["extra"] = json!({"gh_timer": false});
    let events: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let store = Arc::new(FakeSettings {
        settings: Mutex::new(json!({"personalization": personalization})),
        fail_save: AtomicBool::new(false),
        events: events.clone(),
        file: Some(file.clone()),
    });
    let spawn_events = events.clone();
    let spawn_file = file.clone();
    let runner = FakeRunner::new(
        Arc::default(),
        Box::new(move |_| {
            // Переход 0.4.9 уносит файл персонализации 0.4.7 в копию — тогда пусто.
            let cfg = std::fs::read_to_string(&spawn_file).unwrap_or_default();
            spawn_events.lock().unwrap().push(("spawn".to_owned(), cfg));
            Err(RunnerError::Proton(
                hlsr_platform::linux::prefix_command::ProtonError::new(
                    "STOP_AFTER_PERSONALIZATION",
                ),
            ))
        }),
    );
    let logger = Arc::new(MemoryLogger::default());
    let service = LaunchService::new(LaunchServiceOptions {
        delivery: Some(Arc::new(FakeDelivery {
            install: Some(install.clone()),
            registered: Mutex::default(),
        })),
        processes: Some(Arc::new(FakeProcesses::default())),
        runner: Some(runner),
        settings_store: Some(store.clone()),
        steam_locator: Some(FakeSteam::new("/steam", true)),
        logger: Some(logger.clone()),
        ..LaunchServiceOptions::default()
    });
    GhostingRig {
        _root: root,
        service,
        store,
        events,
        install,
    }
}

fn actions(events: &Mutex<Vec<(String, String)>>) -> Vec<String> {
    events
        .lock()
        .unwrap()
        .iter()
        .map(|(action, _)| action.clone())
        .collect()
}

/// «Linux: запуск не пишет настройки и не переписывает extras из config.cfg» (0.4.9): файл игрока
/// готовит `prepare_for_launch`, настройки на запуске не сохраняются.
#[tokio::test]
async fn ghosting_launch_does_not_write_settings_or_extras() {
    let rig = ghosting_rig();
    let before = rig.store.settings.lock().unwrap().clone();
    let error = rig.service.launch("ghosting-renewed").await.unwrap_err();
    assert!(error.message.contains("STOP_AFTER_PERSONALIZATION"));
    assert_eq!(actions(&rig.events), ["spawn"]);
    assert_eq!(*rig.store.settings.lock().unwrap(), before);
    let cfg = rig.install.join(ghosting_personalization::CFG_DIR);
    assert!(
        cfg.join("autoexec.cfg").exists(),
        "файл игрока создан или получен переходом"
    );
    assert_eq!(
        std::fs::read_to_string(cfg.join("config.cfg")).unwrap(),
        "viewmodel_fov \"84\"\ngh_timer \"1\"\ngh_cross_size \"30\"\n"
    );
}

/// Эпизоды Ghosting на Linux: без установки — `NOT_INSTALLED`, чужой эпизод —
/// `LAUNCH_CONFIG_INVALID`, установленный эпизод запускает `hl2.exe` с `-game ep2` и
/// синхронизирует `config.cfg`/`autoexec.cfg` из `ghostingmod/cfg`.
#[tokio::test]
async fn ghosting_episodes_launch_on_linux_and_validate_install() {
    let rig = ghosting_rig();
    // Чужой эпизод / игра без эпизодов — LAUNCH_CONFIG_INVALID.
    for (game, episode) in [("ghosting-renewed", "ep3"), ("hl2-4260", "ep1")] {
        let error = rig
            .service
            .launch_episode(game, Some(episode))
            .await
            .unwrap_err();
        assert_eq!(error.code.as_str(), "LAUNCH_CONFIG_INVALID");
    }
    // Эпизод не установлен — NOT_INSTALLED до запуска.
    let error = rig
        .service
        .launch_episode("ghosting-renewed", Some("ep2"))
        .await
        .unwrap_err();
    assert_eq!(error.code.as_str(), "NOT_INSTALLED");
    assert!(actions(&rig.events).is_empty());

    // Ставим маркер и GameInfo.txt (реальный регистр в архивах эпизодов!) и проверяем запуск.
    let ep2 = rig.install.join("ep2");
    std::fs::create_dir_all(ep2.join("cfg")).unwrap();
    std::fs::write(ep2.join("GameInfo.txt"), "\"GameInfo\" {}\n").unwrap();
    std::fs::write(
        ep2.join(hlsr_delivery::episodes::MARKER),
        r#"{"version":"1.0.0"}"#,
    )
    .unwrap();
    let seen_argv: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen_argv_cb = seen_argv.clone();
    let seen_episode: Arc<Mutex<Option<String>>> = Arc::default();
    let seen_episode_cb = seen_episode.clone();

    struct EpisodeDelivery {
        install: PathBuf,
        episode: Arc<Mutex<Option<String>>>,
    }
    impl LaunchDelivery for EpisodeDelivery {
        fn is_busy(&self) -> bool {
            false
        }
        fn installed_of(&self, _game_id: &str) -> Result<Value, DeliveryError> {
            Ok(json!({"installPath": self.install.to_string_lossy()}))
        }
        fn register_started_process(
            &self,
            _game_id: &str,
            info: StartedProcess,
        ) -> Result<Value, DeliveryError> {
            *self.episode.lock().unwrap() = info.episode.clone();
            Ok(json!({"state": "running", "episode": info.episode}))
        }
    }

    let runner = FakeRunner::new(
        Arc::default(),
        Box::new(move |call| {
            *seen_argv_cb.lock().unwrap() = call.args.clone();
            Ok(())
        }),
    );
    let service = LaunchService::new(LaunchServiceOptions {
        delivery: Some(Arc::new(EpisodeDelivery {
            install: rig.install.clone(),
            episode: seen_episode_cb,
        })),
        processes: Some(Arc::new(FakeProcesses::default())),
        runner: Some(runner),
        settings_store: Some(rig.store.clone()),
        steam_locator: Some(FakeSteam::new("/steam", true)),
        start_check: Some(Duration::ZERO),
        ..LaunchServiceOptions::default()
    });
    let result = service
        .launch_episode("ghosting-renewed", Some("ep2"))
        .await
        .expect("запуск ep2");
    assert_eq!(result["status"], "started");
    assert_eq!(result["process"]["episode"], "ep2");
    assert_eq!(seen_episode.lock().unwrap().as_deref(), Some("ep2"));
    let argv = seen_argv.lock().unwrap().clone();
    let at = argv.iter().position(|arg| arg == "-game").expect("-game");
    assert_eq!(argv[at + 1], "ep2", "{argv:?}");
    assert!(!argv.iter().any(|arg| arg == "ghostingmod"), "{argv:?}");
    assert!(ep2.join("cfg/autoexec.cfg").exists());
    assert!(ep2.join("cfg/config.cfg").exists());
    assert_eq!(
        hlsr_delivery::episodes::pending_sync(Some(rig.install.as_path())),
        ["ep2"]
    );
}
