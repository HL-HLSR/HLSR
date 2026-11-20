//! Самотесты 0.4.7 для запуска в префиксе Proton: `linux-selftest.js` (wineserver на хосте,
//! чужой wineserver, сборка без protonfixes, журнал запуска, обёртка, общий префикс игры и
//! LiveSplit, corefonts, тайм-аут и ошибки подготовки, отмена, Wayland), `proton-update-selftest.js`
//! (подготовка префикса после смены Proton — ссылки, только Unix), `hl1-selftest.js` (мост к
//! Steam через Steam Runtime). Процессы — поддельные (`FakeSpawner`).

mod proton_support;

use hlsr_delivery::errors::RawError;
use hlsr_delivery::ports::MemoryLogger;
use hlsr_delivery::providers::BoxFuture;
use hlsr_platform::linux::abort::Abort;
use hlsr_platform::linux::prefix_command::ProtonError;
use hlsr_platform::linux::proton_manager::{ProtonManager, ProtonManagerOptions};
use hlsr_platform::linux::proton_runner::{
    self, LaunchOptions, ProtonBuild, ProtonRunner, ProtonRunnerOptions, ProtonSelection,
    RunnerError, SteamBridge, Wineserver, core_fonts_status, file_identity,
};
use hlsr_platform::linux::python_runtime::PythonEnvironment;
use hlsr_platform::linux::user_namespaces::UserNamespacesCheck;
use hlsr_platform::spawn::{Env, PATH_DELIMITER, process_env};
use hlsr_platform::tool_runner::{PrefixLaunchOptions, PrefixLauncher};
use proton_support::{Behaviour, Call, FakeSpawner, Root, write_file};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn resources() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources")
}

struct Selected(Mutex<Option<ProtonBuild>>);

impl ProtonSelection for Selected {
    fn selected(&self) -> Option<ProtonBuild> {
        self.0.lock().unwrap().clone()
    }
}

struct SameEnv;

impl PythonEnvironment for SameEnv {
    fn prepare_environment<'a>(&'a self, env: Env) -> BoxFuture<'a, Result<Env, ProtonError>> {
        Box::pin(std::future::ready(Ok(env)))
    }
}

struct Unrestricted;

impl UserNamespacesCheck for Unrestricted {
    fn ensure<'a>(&'a self, _env: &'a Env) -> BoxFuture<'a, Result<(), ProtonError>> {
        Box::pin(std::future::ready(Ok(())))
    }
}

/// `writeCoreFonts(prefix)`: шрифты (копии Carlito) и запись в `winetricks.log`.
fn write_core_fonts(prefix: &Path) {
    let carlito = std::fs::read(resources().join("livesplit-fonts/Carlito-Regular.ttf")).unwrap();
    for name in proton_runner::CORE_FONT_FILES {
        write_file(
            &prefix.join("drive_c/windows/Fonts").join(name),
            &carlito,
            None,
        );
    }
    write_file(
        &prefix.join("winetricks.log"),
        "arial\ntimes\ncorefonts\n",
        None,
    );
}

/// `prefixRunner(t, spawnFn, options)`.
struct Rig {
    root: Root,
    runner: ProtonRunner,
    spawner: Arc<FakeSpawner>,
    logger: Arc<MemoryLogger>,
    selected: Arc<Selected>,
    servers: Arc<Mutex<Vec<Wineserver>>>,
    prefix: PathBuf,
    game: PathBuf,
    marker: PathBuf,
    proton: PathBuf,
}

#[derive(Default)]
struct RigOptions {
    timeout_ms: Option<u64>,
    kill_grace_ms: Option<u64>,
    env: Option<Env>,
}

impl Rig {
    fn new(behaviour: Behaviour) -> Rig {
        Rig::with(behaviour, RigOptions::default())
    }

    fn with(behaviour: Behaviour, options: RigOptions) -> Rig {
        let root = Root::new("hlsr-prefix-test-");
        let proton = PathBuf::from(root.at("proton"));
        write_file(
            &proton.join("protonfixes/winetricks"),
            "#!/bin/sh\n",
            Some(0o755),
        );
        let spawner = FakeSpawner::new(behaviour);
        let logger = Arc::new(MemoryLogger::default());
        let selected = Arc::new(Selected(Mutex::new(Some(ProtonBuild {
            path: proton.to_string_lossy().into_owned(),
            name: "Test Proton".to_owned(),
        }))));
        let servers: Arc<Mutex<Vec<Wineserver>>> = Arc::default();
        let list = servers.clone();
        let runner = ProtonRunner::new(ProtonRunnerOptions {
            manager: Some(selected.clone()),
            data_dir: root.path.clone(),
            resources_path: resources(),
            logger: Some(logger.clone()),
            spawner: Some(spawner.clone()),
            prefix_command_timeout_ms: options.timeout_ms,
            prefix_command_kill_grace_ms: options.kill_grace_ms,
            python_runtime: Some(Arc::new(SameEnv)),
            user_namespaces: Some(Arc::new(Unrestricted)),
            prefix_wineservers: Some(Arc::new(move |_: &Path| list.lock().unwrap().clone())),
            umu_path: Some(Arc::new(|| Some("/test/umu-run".to_owned()))),
            env: options.env,
            ..ProtonRunnerOptions::default()
        });
        let prefix = PathBuf::from(runner.prefix_for("ghosting-renewed").unwrap());
        let game = PathBuf::from(root.at("hl2.exe"));
        std::fs::write(&game, "test executable").unwrap();
        Rig {
            marker: prefix.join(".hlsr-corefonts-ready"),
            root,
            runner,
            spawner,
            logger,
            selected,
            servers,
            prefix,
            game,
            proton,
        }
    }

    /// `hostServerRunner(t, exitCode)`: wineserver сборки выходит с `code`, префикс уже есть.
    fn host_server(code: i32) -> Rig {
        let rig = Rig::new(Arc::new(move |_, call: &Call, controller, _| {
            if call.command.ends_with("wineserver") {
                controller.exit(Some(code), None);
            } else {
                controller.spawned();
            }
        }));
        write_file(
            &rig.proton.join("files/bin/wineserver"),
            "#!/bin/sh\n",
            Some(0o755),
        );
        write_file(
            &rig.prefix.join("system.reg"),
            "WINE REGISTRY Version 2\n",
            None,
        );
        rig
    }

    fn options(&self, skip_fonts: bool) -> LaunchOptions {
        LaunchOptions {
            skip_fonts,
            ..LaunchOptions::default()
        }
    }

    fn warnings(&self) -> Vec<String> {
        self.logger
            .take()
            .into_iter()
            .filter(|(level, _)| *level == "warn")
            .map(|(_, message)| message)
            .collect()
    }
}

/// Процесс команды в префиксе: запускается и делает `action` (вывод, выход).
fn command(
    action: impl Fn(&Call, &hlsr_platform::spawn::ChildController) + Send + Sync + 'static,
) -> Behaviour {
    Arc::new(move |_, call: &Call, controller, _| {
        action(call, &controller);
    })
}

fn env_of(pairs: &[(&str, &str)]) -> Env {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[tokio::test]
async fn prefix_wineserver_is_started_on_the_host_before_the_umu_container() {
    let rig = Rig::host_server(0);
    let mut options = rig.options(true);
    options.env = env_of(&[("PROTON_NO_FSYNC", "1"), ("PROTON_NO_NTSYNC", "1")]);
    rig.runner
        .launch("ghosting-renewed", &rig.game, &[], options)
        .await
        .unwrap();
    let calls = rig.spawner.take_calls();
    let commands: Vec<&str> = calls.iter().map(|call| call.command.as_str()).collect();
    assert_eq!(
        commands,
        [
            rig.root.at("proton/files/bin/wineserver").as_str(),
            "/test/umu-run"
        ]
    );
    let server = &calls[0];
    assert!(
        server.args.is_empty(),
        "без -p: сервер уходит сам после последнего клиента"
    );
    assert_eq!(server.env["WINEPREFIX"], rig.prefix.to_string_lossy());
    assert_eq!(
        server.env["WINEFSYNC"], "0",
        "PROTON_NO_FSYNC выключает fsync и у сервера"
    );
    assert_eq!(server.env["PROTON_NO_NTSYNC"], "1");
    assert!(
        !server.env.contains_key("LD_LIBRARY_PATH"),
        "окружение AppImage серверу не передаётся"
    );
    assert_eq!(server.env["PATH"], "/usr/bin:/bin");
    assert_eq!(
        server.stdio, "ignore",
        "фоновый сервер не держит каналы лаунчера"
    );
    assert!(server.detached);

    rig.runner
        .launch("ghosting-renewed", &rig.game, &[], rig.options(true))
        .await
        .unwrap();
    assert_eq!(rig.spawner.take_calls()[0].env["WINEFSYNC"], "1");
}

#[tokio::test]
async fn running_wineserver_and_new_prefix_are_left_alone() {
    let running = Rig::host_server(2);
    let launch = running.runner.environment("ghosting-renewed").unwrap();
    let env = launch.env.clone();
    assert_eq!(
        running
            .runner
            .ensure_host_wineserver(&launch, &env)
            .await
            .unwrap(),
        "running"
    );
    assert!(running.warnings().is_empty());
    std::fs::remove_file(running.prefix.join("system.reg")).unwrap();
    running.spawner.take_calls();
    assert_eq!(
        running
            .runner
            .ensure_host_wineserver(&launch, &env)
            .await
            .unwrap(),
        "no-prefix"
    );
    assert!(
        running.spawner.calls().is_empty(),
        "новый префикс создаёт Proton"
    );

    let broken = Rig::host_server(1);
    broken
        .runner
        .launch("ghosting-renewed", &broken.game, &[], broken.options(true))
        .await
        .unwrap();
    assert_eq!(
        broken.spawner.calls().last().unwrap().command,
        "/test/umu-run",
        "без сервера на хосте запуск идёт как раньше"
    );
    assert!(broken.warnings().join("\n").contains("не запустился"));
    // Нет программы wineserver у сборки — сервер не поднимается.
    let plain = Rig::new(Arc::new(|_, _: &Call, controller, _| controller.spawned()));
    let launch = plain.runner.environment("ghosting-renewed").unwrap();
    assert_eq!(
        plain
            .runner
            .ensure_host_wineserver(&launch, &Env::new())
            .await
            .unwrap(),
        "unavailable"
    );
}

#[tokio::test]
async fn wineserver_of_another_proton_in_the_prefix_stops_the_launch() {
    let rig = Rig::host_server(2);
    let (dev, ino) = file_identity(&rig.proton.join("files/bin/wineserver")).unwrap();
    *rig.servers.lock().unwrap() = vec![Wineserver {
        pid: 4242,
        dev,
        ino,
        exe_path: None,
    }];
    rig.runner
        .launch("ghosting-renewed", &rig.game, &[], rig.options(true))
        .await
        .unwrap();
    assert_eq!(
        rig.spawner.calls().last().unwrap().command,
        "/test/umu-run",
        "сервер того же Proton не мешает"
    );

    let (dev, ino) = file_identity(&rig.game).unwrap();
    *rig.servers.lock().unwrap() = vec![Wineserver {
        pid: 4242,
        dev,
        ino,
        exe_path: Some("/home/u/.local/share/Steam/steamapps/common/Proton - Experimental/files/bin/wineserver".to_owned()),
    }];
    rig.spawner.take_calls();
    let error = rig
        .runner
        .launch("ghosting-renewed", &rig.game, &[], rig.options(true))
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("LAUNCH_FAILED"));
    let message = error.message();
    assert!(
        message.contains("Proton Proton - Experimental"),
        "{message}"
    );
    assert!(
        message.contains("PID 4242") && message.contains("LiveSplit"),
        "{message}"
    );
    assert!(message.contains("а выбран Test Proton"), "{message}");
    let RunnerError::Delivery(delivery) = &error else {
        panic!("ошибка доставки")
    };
    assert_eq!(
        delivery.details,
        Some(serde_json::json!({"pid": 4242, "proton": "Proton - Experimental"}))
    );
    assert!(
        rig.spawner.calls().is_empty(),
        "ни wineserver, ни umu не запускаются"
    );
}

#[cfg(unix)]
#[test]
fn prefix_wineserver_is_found_by_socket_folder_and_program_file() {
    let root = Root::new("hlsr-wineserver-test-");
    let prefix = root.path.join("pfx");
    std::fs::create_dir(&prefix).unwrap();
    let (dev, ino) = file_identity(&prefix).unwrap();
    let uid = rustix::process::getuid().as_raw();
    let server_dir = root
        .path
        .join(format!("tmp/.wine-{uid}/server-{dev:x}-{ino:x}"));
    let other_dir = root.path.join(format!("tmp/.wine-{uid}/server-1-2"));
    std::fs::create_dir_all(&server_dir).unwrap();
    std::fs::create_dir_all(&other_dir).unwrap();
    let binary = root.path.join("proton/files/bin/wineserver");
    write_file(&binary, "server", None);
    let proc = root.path.join("proc");
    for (pid, comm, cwd) in [
        ("10", "wineserver", &server_dir),
        ("11", "wineserver", &other_dir),
        ("12", "hl2.exe", &server_dir),
    ] {
        std::fs::create_dir_all(proc.join(pid)).unwrap();
        std::fs::write(proc.join(pid).join("comm"), format!("{comm}\n")).unwrap();
        std::os::unix::fs::symlink(cwd, proc.join(pid).join("cwd")).unwrap();
        std::os::unix::fs::symlink(&binary, proc.join(pid).join("exe")).unwrap();
    }
    std::fs::create_dir_all(proc.join("self")).unwrap();
    let servers = proton_runner::prefix_wineservers_at(&prefix, &proc, &root.path.join("tmp"), uid);
    let (own_dev, own_ino) = file_identity(&binary).unwrap();
    assert_eq!(
        servers,
        [Wineserver {
            pid: 10,
            dev: own_dev,
            ino: own_ino,
            exe_path: Some(binary.to_string_lossy().into_owned())
        }]
    );
    assert!(
        proton_runner::prefix_wineservers_at(
            &root.path.join("missing"),
            &proc,
            &root.path.join("tmp"),
            uid
        )
        .is_empty()
    );
}

#[tokio::test]
async fn build_without_protonfixes_does_not_call_winetricks_and_explains_why() {
    let rig = Rig::new(command(|_, controller| controller.exit(Some(0), None)));
    let valve = PathBuf::from(rig.root.at("Proton 11.0"));
    std::fs::create_dir_all(&valve).unwrap();
    *rig.selected.0.lock().unwrap() = Some(ProtonBuild {
        path: valve.to_string_lossy().into_owned(),
        name: "Proton 11.0".to_owned(),
    });
    let error = rig
        .runner
        .launch("ghosting-renewed", &rig.game, &[], LaunchOptions::default())
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("LAUNCH_FAILED"));
    assert!(
        error
            .message()
            .contains("Proton 11.0 нет protonfixes/winetricks"),
        "{}",
        error.message()
    );
    assert!(rig.spawner.calls().is_empty());
}

#[tokio::test]
async fn launch_output_goes_to_the_log_with_a_header() {
    let rig = Rig::new(Arc::new(|_, _: &Call, controller, log| {
        if let Some(mut log) = log {
            std::io::Write::write_all(&mut log, b"err:module:import_dll Library X not found\n")
                .unwrap();
        }
        controller.spawned();
    }));
    let log_file = PathBuf::from(rig.root.at("logs/proton-ghosting-renewed.log"));
    write_file(&log_file, "старый запуск\n", None);
    let mut options = rig.options(true);
    options.log_file = Some(log_file.clone());
    rig.runner
        .launch(
            "ghosting-renewed",
            &rig.game,
            &["-novid".to_owned()],
            options,
        )
        .await
        .unwrap();
    assert_eq!(rig.spawner.take_calls()[0].stdio, "log");
    tokio::task::yield_now().await;
    let text = std::fs::read_to_string(&log_file).unwrap();
    let header =
        regex::Regex::new(r"^# \d{4}-\d\d-\d\dT.* Test Proton: .*hl2\.exe -novid\n").unwrap();
    assert!(header.is_match(&text), "{text}");
    assert!(text.contains("Library X not found"));
    assert!(!text.contains("старый запуск"));

    rig.runner
        .launch("ghosting-renewed", &rig.game, &[], rig.options(true))
        .await
        .unwrap();
    assert_eq!(
        rig.spawner.take_calls()[0].stdio,
        "ignore",
        "без файла журнала вывод не сохраняется"
    );
}

#[tokio::test]
async fn wrapper_from_proton_options_runs_umu_and_variables_go_to_the_environment() {
    let rig = Rig::new(Arc::new(|_, call: &Call, controller, _| {
        if call.command.ends_with("wineserver") {
            controller.exit(Some(2), None);
        } else {
            controller.spawned();
        }
    }));
    let bin = PathBuf::from(rig.root.at("bin"));
    write_file(&bin.join("mangohud"), "#!/bin/sh\n", Some(0o755));
    // PATH окружения запуска — как у лаунчера с папкой bin впереди.
    let rig = {
        let mut env = process_env();
        let path = env.get("PATH").cloned().unwrap_or_default();
        env.insert(
            "PATH".to_owned(),
            format!("{}{PATH_DELIMITER}{path}", bin.to_string_lossy()),
        );
        let runner = ProtonRunner::new(ProtonRunnerOptions {
            manager: Some(rig.selected.clone()),
            data_dir: rig.root.path.clone(),
            resources_path: resources(),
            logger: Some(rig.logger.clone()),
            spawner: Some(rig.spawner.clone()),
            python_runtime: Some(Arc::new(SameEnv)),
            user_namespaces: Some(Arc::new(Unrestricted)),
            prefix_wineservers: Some(Arc::new(|_: &Path| Vec::new())),
            umu_path: Some(Arc::new(|| Some("/test/umu-run".to_owned()))),
            env: Some(env),
            ..ProtonRunnerOptions::default()
        });
        Rig { runner, ..rig }
    };
    let mut options = rig.options(true);
    options.wrapper = vec!["mangohud".to_owned(), "--dlsym".to_owned()];
    options.env = env_of(&[("PROTON_USE_WINED3D", "1")]);
    rig.runner
        .launch(
            "ghosting-renewed",
            &rig.game,
            &["-novid".to_owned()],
            options,
        )
        .await
        .unwrap();
    let spawned = rig.spawner.calls().last().unwrap().clone();
    assert_eq!(spawned.command, bin.join("mangohud").to_string_lossy());
    assert_eq!(
        spawned.args,
        [
            "--dlsym",
            "/test/umu-run",
            &rig.game.to_string_lossy(),
            "-novid"
        ]
    );
    assert_eq!(spawned.env["PROTON_USE_WINED3D"], "1");

    let mut missing = rig.options(true);
    missing.wrapper = vec!["no-such-wrapper-hlsr".to_owned()];
    let error = rig
        .runner
        .launch("ghosting-renewed", &rig.game, &[], missing)
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("PROTON_WRAPPER_MISSING"));
    assert!(error.message().contains("no-such-wrapper-hlsr"));
    let mut relative = rig.options(true);
    relative.wrapper = vec!["bin/mangohud".to_owned()];
    let error = rig
        .runner
        .launch("ghosting-renewed", &rig.game, &[], relative)
        .await
        .unwrap_err();
    assert_eq!(
        error.code(),
        Some("PROTON_WRAPPER_MISSING"),
        "относительный путь не ищется"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_selection_and_one_prefix_for_game_and_livesplit() {
    let root = Root::new("hlsr-linux-test-");
    let proton = PathBuf::from(root.at("protons/Proton-Test"));
    let umu = PathBuf::from(root.at("resources/umu/umu-run"));
    let game = PathBuf::from(root.at("game/hl2.exe"));
    let timer = PathBuf::from(root.at("tools/LiveSplit.exe"));
    for (file, mode) in [
        (proton.join("proton"), Some(0o755)),
        (proton.join("protonfixes/winetricks"), None),
        (umu.clone(), Some(0o755)),
        (game.clone(), None),
        (timer.clone(), None),
    ] {
        write_file(&file, "test", mode);
    }
    let fonts = PathBuf::from(root.at("resources/livesplit-fonts"));
    std::fs::create_dir_all(&fonts).unwrap();
    for entry in std::fs::read_dir(resources().join("livesplit-fonts")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), fonts.join(entry.file_name())).unwrap();
    }
    let manager = ProtonManager::new(ProtonManagerOptions {
        data_dir: root.path.clone(),
        home: Some(root.path.clone()),
        ..ProtonManagerOptions::default()
    });
    let proton_text = proton.to_string_lossy().into_owned();
    assert!(
        manager
            .discover()
            .iter()
            .any(|entry| entry.path == proton_text)
    );
    manager.select(&proton_text).unwrap();
    assert_eq!(manager.selected().unwrap().path, proton_text);
    let custom = PathBuf::from(root.at("other/Proton-Custom"));
    write_file(&custom.join("proton"), "test", Some(0o755));
    let custom_text = custom.to_string_lossy().into_owned();
    assert_eq!(
        manager.add_custom(&custom_text).unwrap().unwrap().path,
        custom_text
    );
    assert!(
        manager
            .discover()
            .iter()
            .any(|entry| entry.path == custom_text)
    );
    assert_eq!(
        manager.selected().unwrap().path,
        proton_text,
        "добавление не меняет выбор до сохранения"
    );

    let spawner = FakeSpawner::with_pid(
        Arc::new(|_, call: &Call, controller, _| {
            controller.spawned();
            let first = call.args.first().map(String::as_str);
            if first == Some("winetricks") {
                write_core_fonts(Path::new(&call.env["WINEPREFIX"]));
            }
            if matches!(first, Some("winetricks" | "regedit.exe" | "wineboot.exe")) {
                controller.exit(Some(0), None);
            }
        }),
        |_| Some(12345),
    );
    let runner = ProtonRunner::new(ProtonRunnerOptions {
        manager: Some(Arc::new(manager)),
        data_dir: root.path.clone(),
        resources_path: PathBuf::from(root.at("resources")),
        spawner: Some(spawner.clone()),
        python_runtime: Some(Arc::new(SameEnv)),
        user_namespaces: Some(Arc::new(Unrestricted)),
        prefix_wineservers: Some(Arc::new(|_: &Path| Vec::new())),
        ..ProtonRunnerOptions::default()
    });
    runner
        .launch("ghosting-renewed", &game, &[], LaunchOptions::default())
        .await
        .unwrap();
    let livesplit = || LaunchOptions {
        font_profile: Some("livesplit".to_owned()),
        ..LaunchOptions::default()
    };
    runner
        .launch("ghosting-renewed", &timer, &[], livesplit())
        .await
        .unwrap();
    let calls = spawner.calls();
    assert_eq!(
        calls.len(),
        4,
        "corefonts и профиль LiveSplit устанавливаются один раз"
    );
    assert_eq!(calls[0].args, ["winetricks", "corefonts"]);
    assert_eq!(calls[1].args[0], game.to_string_lossy());
    assert_eq!(calls[2].args[0], "regedit.exe");
    assert_eq!(calls[3].args[0], timer.to_string_lossy());
    let prefix = runner.prefix_for("ghosting-renewed").unwrap();
    for call in &calls {
        assert_eq!(call.command, umu.to_string_lossy());
        assert_eq!(call.env["WINEPREFIX"], prefix);
        assert_eq!(call.env["PROTONPATH"], proton_text);
        assert_eq!(
            call.env["PROTON_VERB"], "run",
            "игра и LiveSplit должны запускаться одновременно"
        );
        assert_eq!(call.env["PROTON_LOCAL_SHADER_CACHE"], "1");
    }
    assert_eq!(calls[2].env["PROTON_USE_X11_EXCLUSIVE"], "LiveSplit.exe");
    assert!(
        Path::new(&prefix)
            .join(proton_runner::LIVESPLIT_FONT_MARKER)
            .exists()
    );
    for font in proton_runner::LIVESPLIT_FONTS {
        assert!(
            Path::new(&prefix)
                .join("drive_c/windows/Fonts")
                .join(font.target)
                .exists()
        );
    }
    assert!(
        !Path::new(&prefix)
            .join("drive_c/windows/temp/hlsr-livesplit-fonts.reg")
            .exists(),
        "файл реестра убран"
    );
    runner
        .launch("ghosting-renewed", &timer, &[], livesplit())
        .await
        .unwrap();
    assert_eq!(
        spawner.calls().len(),
        5,
        "повторный запуск не переустанавливает шрифты"
    );
    let exclude = LaunchOptions {
        exclude_cpu0: true,
        ..LaunchOptions::default()
    };
    runner
        .launch("ghosting-renewed", &game, &[], exclude)
        .await
        .unwrap();
    let affinity = spawner.calls()[5].clone();
    if let (Some(_), Some(taskset)) = (
        proton_runner::allowed_cpu_list_without_zero(),
        hlsr_platform::spawn::executable_in_path("taskset", &process_env()),
    ) {
        assert_eq!(affinity.command, taskset.to_string_lossy());
        assert_eq!(affinity.args[0], "-c");
        assert_eq!(affinity.args[2], umu.to_string_lossy());
    }
    assert_eq!(affinity.env["WINEPREFIX"], prefix);
    assert_eq!(affinity.env["PROTON_VERB"], "run");
}

#[tokio::test]
async fn existing_corefonts_without_marker_do_not_rerun_winetricks() {
    let rig = Rig::new(command(|_, controller| controller.spawned()));
    write_core_fonts(&rig.prefix);
    assert!(!rig.marker.exists());
    rig.runner
        .launch("ghosting-renewed", &rig.game, &[], LaunchOptions::default())
        .await
        .unwrap();
    let calls: Vec<Vec<String>> = rig
        .spawner
        .calls()
        .into_iter()
        .map(|call| call.args)
        .collect();
    assert_eq!(calls, [vec![rig.game.to_string_lossy().into_owned()]]);
    assert!(rig.marker.exists());
    assert!(core_fonts_status(&rig.prefix).ready);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parallel_font_preparation_shares_one_installation() {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let rig = Rig::new(command(move |call, controller| {
        seen.fetch_add(1, Ordering::SeqCst);
        write_core_fonts(Path::new(&call.env["WINEPREFIX"]));
        controller.exit(Some(0), None);
    }));
    let env = Env::new();
    let (a, b) = tokio::join!(
        rig.runner.ensure_core_fonts("ghosting-renewed", &env),
        rig.runner.ensure_core_fonts("ghosting-renewed", &env)
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(rig.runner.fonts_pending(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_prefix_exiting_zero_without_corefonts_is_retried_once() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let seen = attempts.clone();
    let rig = Rig::new(command(move |call, controller| {
        if seen.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
            write_core_fonts(Path::new(&call.env["WINEPREFIX"]));
        }
        controller.exit(Some(0), None);
    }));
    let env = Env::new();
    let (a, b) = tokio::join!(
        rig.runner.ensure_core_fonts("ghosting-renewed", &env),
        rig.runner.ensure_core_fonts("ghosting-renewed", &env)
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert!(rig.marker.exists());
    assert!(
        rig.logger
            .take()
            .iter()
            .any(|(level, message)| *level == "warn" && message.contains("повторяю установку"))
    );
    rig.runner
        .ensure_core_fonts("ghosting-renewed", &env)
        .await
        .unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn exit_zero_without_fonts_keeps_diagnostics_and_no_marker() {
    let rig = Rig::new(command(|_, controller| {
        controller.stderr(b"HLSR_TOKEN=secret\nRuntime initialization incomplete\n".to_vec());
        controller.exit(Some(0), None);
    }));
    let error = rig
        .runner
        .ensure_core_fonts("ghosting-renewed", &Env::new())
        .await
        .unwrap_err();
    assert!(
        error
            .message()
            .contains("Runtime initialization incomplete"),
        "{}",
        error.message()
    );
    assert!(!error.message().contains("secret"));
    assert!(
        error
            .message()
            .starts_with("Установка corefonts завершилась без необходимых шрифтов: andalemo.ttf")
    );
    assert!(!rig.marker.exists());
}

#[tokio::test]
async fn marker_or_winetricks_log_without_fonts_does_not_hide_a_damaged_prefix() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let rig = Rig::new(command(move |_, controller| {
        seen.fetch_add(1, Ordering::SeqCst);
        controller.exit(Some(0), None);
    }));
    std::fs::create_dir_all(&rig.prefix).unwrap();
    std::fs::write(&rig.marker, "old launcher marker").unwrap();
    let error = rig
        .runner
        .ensure_core_fonts("ghosting-renewed", &Env::new())
        .await
        .unwrap_err();
    assert!(error.message().contains("без необходимых шрифтов"));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "старая отметка не отменяет повтор на холодном префиксе"
    );
    std::fs::write(rig.prefix.join("winetricks.log"), "corefonts\n").unwrap();
    let error = rig
        .runner
        .ensure_core_fonts("ghosting-renewed", &Env::new())
        .await
        .unwrap_err();
    assert!(
        regex::Regex::new("отмечены установленными.*arial.ttf")
            .unwrap()
            .is_match(error.message())
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "umu отказывается повторять записанный verb"
    );
}

#[tokio::test]
async fn preparation_errors_keep_code_and_bounded_cleaned_diagnostics() {
    let rig = Rig::new(command(|call, controller| {
        assert_eq!(call.stdio, "pipe");
        assert!(call.detached);
        controller.stdout(format!("{}\ncorefonts download\n", "x".repeat(40_000)).into_bytes());
        controller.stderr(b"HLSR_PRESENCE_TOKEN=do-not-log-me\nCannot find cabextract\n".to_vec());
        controller.exit(Some(1), None);
    }));
    let error = rig
        .runner
        .ensure_core_fonts("ghosting-renewed", &Env::new())
        .await
        .unwrap_err();
    let RunnerError::Proton(error) = error else {
        panic!("ошибка команды")
    };
    assert_eq!(error.code.as_deref(), Some("PROTON_PREFIX_COMMAND_FAILED"));
    assert_eq!(error.exit_code, Some(1));
    assert!(error.message.contains("umu-run winetricks corefonts"));
    assert!(error.message.contains("Cannot find cabextract"));
    assert!(error.message.contains("corefonts download"));
    assert!(!error.message.contains("do-not-log-me"));
    assert!(error.stdout.unwrap().len() <= 8192);
    assert!(error.stderr.unwrap().len() <= 8192);
    assert!(!rig.marker.exists());
}

// Часы остановлены: тайм-аут срабатывает, только когда рантайм простаивает, то есть «зависание»
// истекает сразу, а подделка повтора (запись шрифтов и выход в одной задаче) никогда не
// упирается в тайм-аут, как бы ни была нагружена машина.
#[tokio::test(start_paused = true)]
async fn hung_preparation_stops_on_timeout_and_can_be_retried() {
    let retry = Arc::new(AtomicBool::new(false));
    let again = retry.clone();
    let rig = Rig::with(
        command(move |call, controller| {
            if !again.load(Ordering::SeqCst) {
                controller.stderr(b"download stalled".to_vec());
                return;
            }
            write_core_fonts(Path::new(&call.env["WINEPREFIX"]));
            controller.exit(Some(0), None);
        }),
        RigOptions {
            timeout_ms: Some(20),
            kill_grace_ms: Some(10),
            ..RigOptions::default()
        },
    );
    let error = rig
        .runner
        .ensure_core_fonts("ghosting-renewed", &Env::new())
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("PROTON_PREFIX_COMMAND_TIMEOUT"));
    assert!(error.message().contains("download stalled"));
    assert_eq!(
        *rig.spawner.signals.0.lock().unwrap(),
        ["SIGTERM", "SIGKILL"]
    );
    assert_eq!(rig.runner.fonts_pending(), 0);
    assert!(!rig.marker.exists());
    retry.store(true, Ordering::SeqCst);
    rig.runner
        .ensure_core_fonts("ghosting-renewed", &Env::new())
        .await
        .unwrap();
    assert!(rig.marker.exists());
}

#[tokio::test]
async fn spawn_enoent_stays_a_launch_error() {
    let rig = Rig::new(command(|_, controller| {
        controller.error(RawError {
            code: Some("ENOENT".to_owned()),
            message: "spawn /test/umu-run ENOENT".to_owned(),
            ..RawError::default()
        });
    }));
    let error = rig
        .runner
        .ensure_core_fonts("ghosting-renewed", &Env::new())
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("ENOENT"));
    assert!(error.message().contains("ENOENT"));
    assert!(!rig.marker.exists());
}

#[tokio::test]
async fn concurrently_installed_corefonts_are_accepted_only_for_explicit_already_installed() {
    let unrelated = Arc::new(AtomicBool::new(false));
    let flag = unrelated.clone();
    let rig = Rig::new(command(move |call, controller| {
        let prefix = PathBuf::from(&call.env["WINEPREFIX"]);
        write_core_fonts(&prefix);
        let text = if flag.load(Ordering::SeqCst) {
            "runtime initialization failed".to_owned()
        } else {
            format!(
                "winetricks verb 'corefonts' is already installed in '{}'",
                prefix.to_string_lossy()
            )
        };
        controller.stderr(text.into_bytes());
        controller.exit(Some(1), None);
    }));
    rig.runner
        .ensure_core_fonts("ghosting-renewed", &Env::new())
        .await
        .unwrap();
    assert!(rig.marker.exists());
    std::fs::remove_dir_all(&rig.prefix).unwrap();
    unrelated.store(true, Ordering::SeqCst);
    let error = rig
        .runner
        .ensure_core_fonts("ghosting-renewed", &Env::new())
        .await
        .unwrap_err();
    assert!(error.message().contains("runtime initialization failed"));
    assert!(!rig.marker.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_preview_during_shared_setup_does_not_launch_the_game_late() {
    let abort = Abort::new();
    let signal = abort.clone();
    let rig = Rig::new(command(move |call, controller| {
        write_core_fonts(Path::new(&call.env["WINEPREFIX"]));
        signal.abort();
        controller.exit(Some(0), None);
    }));
    let options = LaunchOptions {
        abort: Some(abort.clone()),
        ..LaunchOptions::default()
    };
    let error = rig
        .runner
        .launch("ghosting-renewed", &rig.game, &[], options)
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("ABORT_ERR"));
    assert_eq!(error.message(), "Запуск отменён");
    let calls: Vec<Vec<String>> = rig
        .spawner
        .calls()
        .into_iter()
        .map(|call| call.args)
        .collect();
    assert_eq!(
        calls,
        [vec!["winetricks".to_owned(), "corefonts".to_owned()]]
    );
    assert!(
        core_fonts_status(&rig.prefix).ready,
        "общая подготовка шрифтов доведена до конца"
    );
    let options = LaunchOptions {
        skip_fonts: true,
        abort: Some(abort),
        ..LaunchOptions::default()
    };
    let error = rig
        .runner
        .launch("ghosting-renewed", &rig.game, &[], options)
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("ABORT_ERR"));
    assert_eq!(rig.spawner.calls().len(), 1);
}

#[test]
fn wineland_enables_wayland_and_local_shader_cache_only_in_a_suitable_session() {
    let root = Root::new("hlsr-wineland-env-");
    let environment = |game: &str, pairs: &[(&str, &str)]| {
        let runner = ProtonRunner::new(ProtonRunnerOptions {
            manager: Some(Arc::new(Selected(Mutex::new(Some(ProtonBuild {
                path: "/tmp/proton-wineland".to_owned(),
                name: "Proton Wineland".to_owned(),
            }))))),
            data_dir: root.path.clone(),
            resources_path: PathBuf::from("/tmp"),
            umu_path: Some(Arc::new(|| Some("/usr/bin/umu-run".to_owned()))),
            env: Some(env_of(pairs)),
            ..ProtonRunnerOptions::default()
        });
        runner.environment(game).unwrap().env
    };
    let env = environment("ghosting-renewed", &[("WAYLAND_DISPLAY", "wayland-0")]);
    assert_eq!(env["PROTON_ENABLE_WAYLAND"], "1");
    assert_eq!(env["PROTON_LOCAL_SHADER_CACHE"], "1");
    assert_eq!(
        environment("ghosting-renewed", &[])["PROTON_ENABLE_WAYLAND"],
        "0"
    );
    let overridden = environment(
        "ghosting-renewed",
        &[
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("PROTON_ENABLE_WAYLAND", "0"),
            ("PROTON_LOCAL_SHADER_CACHE", "0"),
        ],
    );
    assert_eq!(overridden["PROTON_ENABLE_WAYLAND"], "0");
    assert_eq!(overridden["PROTON_LOCAL_SHADER_CACHE"], "0");
    // HL1 идёт через X11: всё в её префиксе получает тот же режим.
    assert_eq!(
        environment(
            "hl1-won",
            &[
                ("WAYLAND_DISPLAY", "wayland-0"),
                ("PROTON_ENABLE_WAYLAND", "1")
            ]
        )["PROTON_ENABLE_WAYLAND"],
        "0"
    );
    assert_eq!(
        environment("hl1-won", &[("WAYLAND_DISPLAY", "wayland-0")])["PROTON_ENABLE_WAYLAND"],
        "0"
    );
    assert_eq!(
        environment("hl2-4260", &[("WAYLAND_DISPLAY", "wayland-0")])["PROTON_ENABLE_WAYLAND"],
        "1"
    );
}

#[test]
fn environment_strips_appimage_and_launcher_variables() {
    let root = Root::new("hlsr-runner-env-");
    let appdir = "/tmp/.mount_hlsr";
    let runner = ProtonRunner::new(ProtonRunnerOptions {
        manager: Some(Arc::new(Selected(Mutex::new(Some(ProtonBuild {
            path: "/p".to_owned(),
            name: "P".to_owned(),
        }))))),
        data_dir: root.path.clone(),
        umu_path: Some(Arc::new(|| Some("/u".to_owned()))),
        env: Some(env_of(&[
            ("APPDIR", appdir),
            (
                "PATH",
                &format!("{appdir}/usr/bin{PATH_DELIMITER}/usr/bin{PATH_DELIMITER}{appdir}"),
            ),
            ("LD_LIBRARY_PATH", "/x"),
            ("PYTHONHOME", "/x"),
            ("QT_PLUGIN_PATH", "/x"),
            ("UMU_ID", "keep"),
        ])),
        ..ProtonRunnerOptions::default()
    });
    let env = runner.environment("hl2-4260").unwrap().env;
    for key in ["APPDIR", "LD_LIBRARY_PATH", "PYTHONHOME", "QT_PLUGIN_PATH"] {
        assert!(!env.contains_key(key), "{key}");
    }
    assert_eq!(env["PATH"], "/usr/bin");
    assert_eq!(env["UMU_ID"], "keep");
    assert_eq!(env["GAMEID"], "0");
    assert_eq!(env["STORE"], "none");
    // Переменные Slint и winit из окружения лаунчера дети не получают (решение 3).
    let launcher = process_env();
    assert!(
        launcher
            .keys()
            .all(|key| !key.starts_with("SLINT_") && !key.starts_with("WINIT_"))
    );
}

#[tokio::test]
async fn missing_proton_or_umu_explains_what_to_do() {
    let root = Root::new("hlsr-runner-missing-");
    let runner = ProtonRunner::new(ProtonRunnerOptions {
        manager: Some(Arc::new(Selected(Mutex::new(None)))),
        data_dir: root.path.clone(),
        umu_path: Some(Arc::new(|| Some("/u".to_owned()))),
        ..ProtonRunnerOptions::default()
    });
    assert_eq!(
        runner.environment("hl2-4260").unwrap_err().message,
        "Proton не установлен. Откройте настройки и скачайте Proton Wineland."
    );
    let runner = ProtonRunner::new(ProtonRunnerOptions {
        manager: Some(Arc::new(Selected(Mutex::new(Some(ProtonBuild {
            path: "/p".to_owned(),
            name: "P".to_owned(),
        }))))),
        data_dir: root.path.clone(),
        umu_path: Some(Arc::new(|| None)),
        ..ProtonRunnerOptions::default()
    });
    assert_eq!(
        runner.environment("hl2-4260").unwrap_err().message,
        "umu-run не найден. Пересоберите AppImage с umu или установите umu-launcher."
    );
    assert_eq!(
        runner.prefix_for("../evil").unwrap_err().message,
        "Недопустимый идентификатор игры для Proton-префикса"
    );
    let missing = PathBuf::from(root.at("missing.exe"));
    let error = runner
        .launch("hl2-4260", &missing, &[], LaunchOptions::default())
        .await
        .unwrap_err();
    assert_eq!(
        error.message(),
        format!("Исполняемый файл не найден: {}", missing.to_string_lossy())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steam_bridge_runs_proton_through_the_steam_runtime_without_umu() {
    let root = Root::new("hlsr-bridge-");
    let proton = PathBuf::from(root.at("proton-test"));
    write_file(&proton.join("proton"), "#!/bin/sh\n", Some(0o755));
    write_file(
        &proton.join("toolmanifest.vdf"),
        "\"manifest\"\n{\n  \"require_tool_appid\" \"4183110\"\n}\n",
        None,
    );
    write_file(
        &PathBuf::from(root.at("resources/umu/umu-run")),
        "#!/bin/sh\n",
        Some(0o755),
    );
    let data_home = root.at("data");
    let entry_point = PathBuf::from(root.at("data/umu/steamrt4/_v2-entry-point"));
    write_file(&entry_point, "#!/bin/sh\n", Some(0o755));
    let injector = PathBuf::from(root.at("game/Bunnymod XT/Injector.exe"));
    write_file(&injector, "MZ", None);
    let mut env = process_env();
    env.insert("XDG_DATA_HOME".to_owned(), data_home);
    env.insert("UMU_ID".to_owned(), "0".to_owned());
    let spawner = FakeSpawner::with_pid(
        Arc::new(|_, _: &Call, controller, _| controller.spawned()),
        |_| Some(4242),
    );
    let runner = ProtonRunner::new(ProtonRunnerOptions {
        manager: Some(Arc::new(Selected(Mutex::new(Some(ProtonBuild {
            path: proton.to_string_lossy().into_owned(),
            name: "test".to_owned(),
        }))))),
        data_dir: root.path.clone(),
        resources_path: PathBuf::from(root.at("resources")),
        spawner: Some(spawner.clone()),
        python_runtime: Some(Arc::new(SameEnv)),
        user_namespaces: Some(Arc::new(Unrestricted)),
        prefix_wineservers: Some(Arc::new(|_: &Path| Vec::new())),
        env: Some(env),
        ..ProtonRunnerOptions::default()
    });
    let game_dir = root.at("game");
    let options = LaunchOptions {
        skip_fonts: true,
        env: env_of(&[("BXT_DISABLE_VSYNC", "1")]),
        steam_bridge: Some(SteamBridge {
            app_id: 70,
            steam_path: "/steam".to_owned(),
            install_path: Some(game_dir.clone()),
        }),
        ..LaunchOptions::default()
    };
    let args = ["Z:\\game\\hl.exe".to_owned(), "-steam".to_owned()];
    let result = runner
        .launch("hl1-won", &injector, &args, options)
        .await
        .unwrap();
    let call = spawner.calls()[0].clone();
    let entry = entry_point.to_string_lossy().into_owned();
    let entry = hlsr_core::js::path::NodePath::native().normalize(&entry);
    assert_eq!(call.command, entry);
    assert_eq!(result.command, entry);
    assert_eq!(
        call.args,
        [
            "--verb=run",
            "--",
            &root.at("proton-test/proton"),
            "run",
            &injector.to_string_lossy(),
            "Z:\\game\\hl.exe",
            "-steam"
        ]
    );
    for key in ["UMU_ID", "GAMEID", "STORE", "PROTON_VERB", "WINEPREFIX"] {
        assert!(
            !call.env.contains_key(key),
            "{key} отключил бы мост к Steam"
        );
    }
    assert_eq!(
        call.env["STEAM_COMPAT_DATA_PATH"],
        runner.prefix_for("hl1-won").unwrap(),
        "префикс тот же, что у umu"
    );
    assert_eq!(call.env["STEAM_COMPAT_CLIENT_INSTALL_PATH"], "/steam");
    assert_eq!(call.env["STEAM_COMPAT_INSTALL_PATH"], game_dir);
    assert_eq!(call.env["SteamAppId"], "70");
    assert_eq!(call.env["SteamGameId"], "70");
    assert_eq!(call.env["STEAM_COMPAT_APP_ID"], "70");
    let runtime_dir = hlsr_core::js::path::NodePath::native().dirname(&entry);
    assert_eq!(
        call.env["STEAM_COMPAT_TOOL_PATHS"],
        format!("{}:{runtime_dir}", proton.to_string_lossy())
    );
    assert_eq!(
        call.env["STEAM_COMPAT_MOUNTS"],
        call.env["STEAM_COMPAT_TOOL_PATHS"]
    );
    assert_eq!(call.env["BXT_DISABLE_VSYNC"], "1");
    assert_eq!(call.cwd.as_deref(), Some(injector.parent().unwrap()));

    std::fs::remove_file(&entry_point).unwrap();
    let options = LaunchOptions {
        skip_fonts: true,
        steam_bridge: Some(SteamBridge {
            app_id: 70,
            steam_path: "/steam".to_owned(),
            install_path: None,
        }),
        ..LaunchOptions::default()
    };
    let error = runner
        .launch("hl1-won", &injector, &[], options)
        .await
        .unwrap_err();
    assert!(
        error.message().contains("Steam Runtime steamrt4 не найден"),
        "{}",
        error.message()
    );
}

#[tokio::test]
async fn tools_are_launched_in_the_game_prefix_through_prefix_launcher() {
    let rig = Rig::new(command(|_, controller| controller.spawned()));
    write_core_fonts(&rig.prefix);
    let launcher: &dyn PrefixLauncher = &rig.runner;
    let options = PrefixLaunchOptions {
        cwd: rig.root.path.clone(),
        font_profile: None,
        env: Some(vec![(
            "PROTON_USE_X11_EXCLUSIVE".to_owned(),
            "LiveSplit.exe".to_owned(),
        )]),
    };
    let launched = launcher
        .launch(
            Some("ghosting-renewed"),
            &rig.game,
            &["-x".to_owned()],
            &options,
        )
        .await
        .unwrap();
    assert_eq!(launched.prefix, rig.prefix);
    assert_eq!(launched.child.map(|child| child.pid), Some(1001));
    let call = rig.spawner.calls()[0].clone();
    assert_eq!(call.cwd.as_deref(), Some(rig.root.path.as_path()));
    assert_eq!(call.env["PROTON_USE_X11_EXCLUSIVE"], "LiveSplit.exe");
    assert_eq!(call.args, [rig.game.to_string_lossy().as_ref(), "-x"]);
    assert!(
        launcher
            .launch(None, &rig.game, &[], &options)
            .await
            .is_err()
    );
}

// ---------------------------------------------------------------- подготовка префикса (ссылки)

#[cfg(unix)]
mod prefix_links {
    use super::*;
    use std::os::unix::fs::symlink;

    /// Прежняя сборка в `protons/` и новая, на которую перешёл игрок.
    fn builds(root: &Root) -> (PathBuf, PathBuf) {
        let old = PathBuf::from(root.at("protons/proton-wineland-11.0-20261003-hlsr3-x86_64"));
        let target = PathBuf::from(root.at("protons/proton-wineland-11.0-20991231-hlsr99-x86_64"));
        write_file(&old.join("proton"), "#!/bin/sh\n", Some(0o755));
        write_file(&target.join("proton"), "#!/bin/sh\n", Some(0o755));
        (old, target)
    }

    fn runner(root: &Root, proton: &Path, spawner: Arc<FakeSpawner>) -> ProtonRunner {
        ProtonRunner::new(ProtonRunnerOptions {
            manager: Some(Arc::new(Selected(Mutex::new(Some(ProtonBuild {
                path: proton.to_string_lossy().into_owned(),
                name: "Wineland".to_owned(),
            }))))),
            data_dir: root.path.clone(),
            resources_path: root.path.clone(),
            spawner: Some(spawner),
            python_runtime: Some(Arc::new(SameEnv)),
            user_namespaces: Some(Arc::new(Unrestricted)),
            prefix_wineservers: Some(Arc::new(|_: &Path| Vec::new())),
            umu_path: Some(Arc::new(|| Some("/test/umu-run".to_owned()))),
            ..ProtonRunnerOptions::default()
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stale_prefix_links_are_prepared_before_launch_and_shared() {
        let root = Root::new("hlsr-proton-update-");
        let (old, target) = builds(&root);
        std::fs::remove_dir_all(&old).unwrap();
        let prefix = root.path.join("compatdata/test/pfx");
        let builtin = prefix.join("drive_c/windows/system32/kernel32.dll");
        let replacement = target.join("files/lib/wine/x86_64-windows/kernel32.dll");
        write_file(&replacement, "Wine builtin DLL", None);
        std::fs::create_dir_all(builtin.parent().unwrap()).unwrap();
        symlink(
            old.join("files/lib/wine/x86_64-windows/kernel32.dll"),
            &builtin,
        )
        .unwrap();
        let saved = prefix.join("drive_c/save.dat");
        std::fs::write(&saved, "keep save").unwrap();
        let unrelated = prefix.join("drive_c/unrelated");
        symlink("/missing/user/file", &unrelated).unwrap();
        let obsolete = prefix.join("drive_c/obsolete.dll");
        symlink(
            old.join("files/lib/wine/x86_64-windows/opencl.dll"),
            &obsolete,
        )
        .unwrap();
        let game = root.path.join("game.exe");
        std::fs::write(&game, "test executable").unwrap();
        let (b, r) = (builtin.clone(), replacement.clone());
        let spawner = FakeSpawner::new(Arc::new(move |_, call: &Call, controller, _| {
            controller.spawned();
            if call.args.first().map(String::as_str) == Some("wineboot.exe") {
                std::fs::remove_file(&b).unwrap();
                symlink(&r, &b).unwrap();
                controller.exit(Some(0), None);
            }
        }));
        let runner = runner(&root, &target, spawner.clone());
        let env = Env::new();
        let (a, c) = tokio::join!(
            runner.ensure_prefix_runtime("test", &env),
            runner.ensure_prefix_runtime("test", &env)
        );
        a.unwrap();
        c.unwrap();
        let skip = || LaunchOptions {
            skip_fonts: true,
            ..LaunchOptions::default()
        };
        runner.launch("test", &game, &[], skip()).await.unwrap();
        runner.launch("test", &game, &[], skip()).await.unwrap();
        let calls: Vec<Vec<String>> = spawner.calls().into_iter().map(|call| call.args).collect();
        let game_text = game.to_string_lossy().into_owned();
        assert_eq!(
            calls,
            [
                vec!["wineboot.exe".to_owned(), "-u".to_owned()],
                vec![game_text.clone()],
                vec![game_text]
            ]
        );
        assert!(builtin.exists());
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), "keep save");
        assert_eq!(
            std::fs::read_link(&unrelated).unwrap(),
            Path::new("/missing/user/file")
        );
        assert_eq!(
            std::fs::read_link(&obsolete).unwrap(),
            old.join("files/lib/wine/x86_64-windows/opencl.dll")
        );
        assert_eq!(runner.prefix_pending(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wineboot_under_a_foreign_proton_is_not_repeated_until_the_prefix_changes() {
        let root = Root::new("hlsr-proton-foreign-");
        let (old, _) = builds(&root);
        let experimental =
            PathBuf::from(root.at(".local/share/Steam/steamapps/common/Proton - Experimental"));
        write_file(&experimental.join("proton"), "#!/bin/sh\n", Some(0o755));
        std::fs::write(
            experimental.join("version"),
            "1791000000 experimental-11.0\n",
        )
        .unwrap();
        let relative = "files/lib/wine/x86_64-windows/explorer.exe";
        write_file(&experimental.join(relative), "Wine builtin", None);
        let prefix = root.path.join("compatdata/test/pfx");
        std::fs::create_dir_all(prefix.join("drive_c/windows")).unwrap();
        symlink(
            old.join(relative),
            prefix.join("drive_c/windows/explorer.exe"),
        )
        .unwrap();
        std::fs::write(prefix.join("version"), "experimental-11.0\n").unwrap();
        let game = root.path.join("game.exe");
        std::fs::write(&game, "test executable").unwrap();
        let spawner = FakeSpawner::new(Arc::new(|_, call: &Call, controller, _| {
            controller.spawned();
            if call.args.first().map(String::as_str) == Some("wineboot.exe") {
                controller.exit(Some(0), None);
            }
        }));
        let runner = runner(&root, &experimental, spawner.clone());
        let skip = || LaunchOptions {
            skip_fonts: true,
            ..LaunchOptions::default()
        };
        runner.launch("test", &game, &[], skip()).await.unwrap();
        runner.launch("test", &game, &[], skip()).await.unwrap();
        let first = |calls: Vec<Call>| -> Vec<String> {
            calls.into_iter().map(|c| c.args[0].clone()).collect()
        };
        let game_text = game.to_string_lossy().into_owned();
        assert_eq!(
            first(spawner.take_calls()),
            ["wineboot.exe", &game_text, &game_text]
        );
        // Префикс обновил другой Proton: отметка больше не действует.
        std::fs::write(prefix.join("version"), "wineland-11.0\n").unwrap();
        runner.launch("test", &game, &[], skip()).await.unwrap();
        assert_eq!(first(spawner.take_calls()), ["wineboot.exe", &game_text]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_prefix_preparation_prevents_launch_and_permits_retry() {
        let root = Root::new("hlsr-proton-failed-");
        let (old, target) = builds(&root);
        let prefix = root.path.join("compatdata/test/pfx/drive_c");
        std::fs::create_dir_all(&prefix).unwrap();
        symlink(target.join("missing.dll"), prefix.join("old.dll")).unwrap();
        std::fs::write(old.join("missing.dll"), "Wine builtin DLL").unwrap();
        let game = root.path.join("game.exe");
        std::fs::write(&game, "test").unwrap();
        let preparations = Arc::new(AtomicUsize::new(0));
        let count = preparations.clone();
        let spawner = FakeSpawner::new(Arc::new(move |_, call: &Call, controller, _| {
            controller.spawned();
            if call.args.first().map(String::as_str) == Some("wineboot.exe") {
                count.fetch_add(1, Ordering::SeqCst);
                controller.stderr(b"wineboot failed".to_vec());
                controller.exit(Some(1), None);
            }
        }));
        let runner = runner(&root, &old, spawner.clone());
        for _ in 0..2 {
            let options = LaunchOptions {
                skip_fonts: true,
                ..LaunchOptions::default()
            };
            let error = runner
                .launch("test", &game, &[], options)
                .await
                .unwrap_err();
            assert!(
                error.message().contains("wineboot failed"),
                "{}",
                error.message()
            );
        }
        assert_eq!(preparations.load(Ordering::SeqCst), 2);
        assert_eq!(runner.prefix_pending(), 0);
        assert_eq!(std::fs::read_to_string(&game).unwrap(), "test");
        assert!(
            spawner
                .calls()
                .iter()
                .all(|call| call.args[0] == "wineboot.exe"),
            "игра не запускалась"
        );
    }
}
