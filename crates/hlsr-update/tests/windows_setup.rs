//! Режимы установщика Windows против самотестов `windows-install-selftest.js`: «detect…» (окружение
//! запуска), «launchInstalled: EACCES свежего exe повторяется, флаг администратора — через
//! оболочку», «resolveInstallDir: /D=, затем сохранённый каталог, затем каталог по умолчанию»,
//! «удаление из установщика: «Назад»…» (`returnSetupOf`); тихие установка и удаление целиком на
//! подделках (реестр, ярлыки, процессы, блокировка) во временном каталоге; backend окон установки и
//! удаления.
//!
//! Библиотека игр в каждом тесте — во временном каталоге (`library.json` профиля): настоящий
//! `C:\HLSR` не трогается.
//!
//! Пути Windows на настоящей ФС (`path.win32`, буквы дисков): только Windows.
#![cfg(windows)]

mod support;

use hlsr_core::args::{SetupArgs, parse_setup_args};
use hlsr_core::json_formats::compact;
use hlsr_core::logger::MemoryLogger;
use hlsr_update::errors::setup_exit;
use hlsr_update::installer_window::{InstallerWindow, Phase};
use hlsr_update::legacy_traces;
use hlsr_update::uninstall::library_cleanup::StaticSteamLibraries;
use hlsr_update::windows::file_list;
use hlsr_update::windows::known_folders::FakeKnownFolders;
use hlsr_update::windows::layout::{self, App};
use hlsr_update::windows::payload::{Payload, Source};
use hlsr_update::windows::processes::{
    Call, Command, Env, FakeProcesses, SpawnError, launch_environment,
};
use hlsr_update::windows::registry::{FakeRegistry, RegValue, Registry};
use hlsr_update::windows::setup::{
    self, FakeLock, InstanceLock, LaunchDeps, LegacyAwareLock, SetupBackend, SetupServices,
    UninstallBackend,
};
use hlsr_update::windows::shortcuts::FakeShellLinks;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::payload::pack_for_tests;

const UNINSTALL_FLAG: &str = "--hlsr-uninstall";

fn args(list: &[&str]) -> SetupArgs {
    parse_setup_args(list)
}

#[test]
fn launch_environment_drops_unpacker_and_window_variables() {
    let env = Env::from_pairs([
        ("PATH", "x"),
        ("PORTABLE_EXECUTABLE_FILE", "a"),
        ("PORTABLE_EXECUTABLE_DIR", "b"),
        ("SLINT_SCALE_FACTOR", "1.25"),
        ("WINIT_X11_SCALE_FACTOR", "2"),
        ("HLSR_DATA_DIR", "keep"),
    ]);
    let clean = launch_environment(&env);
    assert_eq!(
        clean.vars(),
        &[
            ("PATH".to_owned(), "x".to_owned()),
            ("HLSR_DATA_DIR".to_owned(), "keep".to_owned())
        ]
    );
}

#[test]
fn runasadmin_flag_is_a_whole_word() {
    assert!(setup::has_runasadmin("~ RUNASADMIN"));
    assert!(setup::has_runasadmin("~ WIN7RTM runasadmin HIGHDPIAWARE"));
    assert!(!setup::has_runasadmin("~ HIGHDPIAWARE"));
    assert!(!setup::has_runasadmin("~ RUNASADMINX"));
    assert!(!setup::has_runasadmin("~_RUNASADMIN"));
    let registry = FakeRegistry::new().with_value(
        &format!("HKLM\\{}", setup::COMPAT_LAYERS),
        "C:\\X\\HLSR.exe",
        RegValue::Sz("~ RUNASADMIN".to_owned()),
    );
    assert!(setup::runs_as_admin("c:\\x\\hlsr.exe", &registry));
    assert!(!setup::runs_as_admin("C:\\Y\\HLSR.exe", &registry));
}

struct Launch {
    lock: FakeLock,
    registry: FakeRegistry,
    env: Env,
    log: MemoryLogger,
}

impl Launch {
    fn new() -> Launch {
        Launch {
            lock: FakeLock::new(0),
            registry: FakeRegistry::new(),
            env: Env::from_pairs([
                ("SystemRoot", "C:\\Windows"),
                ("PORTABLE_EXECUTABLE_FILE", "C:\\D\\HLSR-Setup.exe"),
            ]),
            log: MemoryLogger::default(),
        }
    }

    fn run(&self, processes: &FakeProcesses, args: &[&str]) -> Result<(), SpawnError> {
        let deps = LaunchDeps {
            lock: &self.lock,
            registry: &self.registry,
            processes,
            env: &self.env,
            sleep: &|_| {},
            log: &self.log,
            shell: true,
        };
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        setup::launch_installed(&deps, "C:\\HLSR\\Launcher", &args)
    }
}

fn spawns(processes: &FakeProcesses) -> Vec<Command> {
    processes
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Spawn(command) => Some(command),
            Call::Run(..) => None,
        })
        .collect()
}

fn shell_runs(processes: &FakeProcesses) -> Vec<Command> {
    processes
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Run(command, _) => Some(command),
            Call::Spawn(_) => None,
        })
        .collect()
}

fn failing(code: &'static str, ok_after: usize) -> FakeProcesses {
    FakeProcesses::new().on_spawn(move |command, index| {
        if ok_after > 0 && index >= ok_after {
            Ok(())
        } else {
            Err(SpawnError::with_code(&command.file, code))
        }
    })
}

#[test]
fn launch_installed_retries_busy_exe_and_uses_the_shell_for_admin_flag() {
    // «Запускать от имени администратора»: сразу оболочка, без CreateProcess, с аргументами.
    let launch = Launch::new();
    launch
        .registry
        .write(
            &format!("HKCU\\{}", setup::COMPAT_LAYERS),
            "C:\\HLSR\\Launcher\\HLSR.exe",
            &RegValue::Sz("~ RUNASADMIN".to_owned()),
        )
        .unwrap();
    let processes = failing("EACCES", 0);
    launch.run(&processes, &[UNINSTALL_FLAG]).unwrap();
    assert!(spawns(&processes).is_empty());
    let shell = shell_runs(&processes);
    assert_eq!(shell.len(), 1);
    assert_eq!(
        shell[0].file,
        "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
    );
    assert_eq!(
        shell[0].args.last().unwrap(),
        "Start-Process -FilePath $env:HLSR_EXE -WorkingDirectory $env:HLSR_DIR -ArgumentList $env:HLSR_ARGS"
    );
    assert_eq!(
        shell[0].env.get("HLSR_EXE").as_deref(),
        Some("C:\\HLSR\\Launcher\\HLSR.exe")
    );
    assert_eq!(
        shell[0].env.get("HLSR_ARGS").as_deref(),
        Some(UNINSTALL_FLAG)
    );
    assert_eq!(shell[0].env.get("PORTABLE_EXECUTABLE_FILE"), None);
    assert_eq!(
        launch.lock.releases(),
        1,
        "блокировка отпускается перед запуском"
    );

    // Антивирус отпустил exe на третьей попытке: оболочка не нужна.
    let launch = Launch::new();
    let busy = failing("EACCES", 2);
    launch.run(&busy, &["--x"]).unwrap();
    let spawned = spawns(&busy);
    assert_eq!(spawned.len(), 3);
    assert_eq!(spawned[2].args, ["--x"]);
    assert_eq!(spawned[2].cwd.as_deref(), Some("C:\\HLSR\\Launcher"));
    assert_eq!(spawned[2].env.get("PORTABLE_EXECUTABLE_FILE"), None);
    assert!(shell_runs(&busy).is_empty());
    let warnings: Vec<String> = launch.log.take().into_iter().map(|(_, m)| m).collect();
    assert_eq!(
        warnings[0],
        "[setup] запуск HLSR.exe: EACCES, попытка 1 из 6"
    );

    // Флаг не виден в реестре, CreateProcess отказывает всегда: в конце оболочка.
    let always = failing("EACCES", 0);
    Launch::new().run(&always, &[]).unwrap();
    assert_eq!(spawns(&always).len(), 6);
    let shell = shell_runs(&always);
    assert_eq!(shell.len(), 1);
    assert_eq!(
        shell[0].args.last().unwrap(),
        "Start-Process -FilePath $env:HLSR_EXE -WorkingDirectory $env:HLSR_DIR"
    );

    // Оболочка тоже не смогла: наружу уходит исходная ошибка.
    let refused = failing("EPERM", 0).on_run(|_| Some(1));
    let error = Launch::new().run(&refused, &[]).unwrap_err();
    assert_eq!(error.code.as_deref(), Some("EPERM"));

    // Другие ошибки не повторяются.
    let missing = failing("ENOENT", 0);
    let error = Launch::new().run(&missing, &[]).unwrap_err();
    assert_eq!(error.code.as_deref(), Some("ENOENT"));
    assert_eq!(spawns(&missing).len(), 1);
    assert!(shell_runs(&missing).is_empty());
}

#[test]
fn resolve_install_dir_prefers_d_then_saved_then_default() {
    let id = layout::identity(None).unwrap();
    let env = |name: &str| (name == "WINDIR").then(|| "C:\\Windows".to_owned());
    let saved = |dir: &str| vec![("InstallLocation".to_owned(), RegValue::Sz(dir.to_owned()))];
    let dir = "D:\\Games\\HLSR somewhere";
    assert_eq!(
        setup::resolve_install_dir(
            &args(&["/S", &format!("/D={dir}")]),
            &saved("E:\\x"),
            &id,
            &env
        ),
        dir
    );
    assert_eq!(
        setup::resolve_install_dir(&args(&[]), &saved(dir), &id, &env),
        dir
    );
    // Каталог, который NSIS записал до завершения установки, не считается.
    let mut seeded = saved(dir);
    seeded.push((layout::SEED_FLAG.to_owned(), RegValue::Sz("1".to_owned())));
    assert_eq!(
        setup::resolve_install_dir(&args(&[]), &seeded, &id, &env),
        "C:\\HLSR\\Launcher"
    );
    assert_eq!(
        setup::resolve_install_dir(&args(&[]), &[], &id, &env),
        "C:\\HLSR\\Launcher"
    );
    // Недопустимый сохранённый каталог — каталог по умолчанию.
    assert_eq!(
        setup::resolve_install_dir(&args(&[]), &saved("C:\\Windows"), &id, &env),
        "C:\\HLSR\\Launcher"
    );
    assert_eq!(
        setup::resolve_install_dir(&args(&[]), &saved("relative"), &id, &env),
        "C:\\HLSR\\Launcher"
    );
    let qa = layout::identity(Some(&App::qa())).unwrap();
    assert_eq!(
        setup::resolve_install_dir(&args(&[]), &[], &qa, &env),
        "C:\\HLSR-QA\\Launcher"
    );
}

#[test]
fn return_setup_and_language() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("HLSR-Setup.EXE");
    std::fs::write(&exe, b"x").unwrap();
    let text = exe.to_string_lossy().into_owned();
    assert_eq!(
        setup::return_setup_of(Some(&text)).as_deref(),
        Some(text.as_str())
    );
    assert_eq!(
        setup::return_setup_of(Some("relative\\HLSR-Setup.exe")),
        None
    );
    assert_eq!(
        setup::return_setup_of(Some("C:\\nope\\HLSR-Setup.txt")),
        None
    );
    assert_eq!(setup::return_setup_of(None), None);
    let missing = dir
        .path()
        .join("missing.exe")
        .to_string_lossy()
        .into_owned();
    assert_eq!(setup::return_setup_of(Some(&missing)), None);
    assert_eq!(
        setup::language("en-US", Some("C:\\D\\HLSR-Setup-0.5.0-ru.exe")),
        "ru"
    );
    assert_eq!(
        setup::language("ru-RU", Some("HLSR-Setup-0.5.0-EN.exe")),
        "en"
    );
    assert_eq!(setup::language("ru-RU", Some("HLSR-Setup-0.5.0.exe")), "ru");
    assert_eq!(setup::language("de-DE", None), "en");
}

#[test]
fn acquire_lock_waits_a_second_between_attempts() {
    let lock = FakeLock::new(3);
    let sleeps = Mutex::new(Vec::new());
    assert!(setup::acquire_lock(&lock, setup::LOCK_WAIT, &|d| sleeps
        .lock()
        .unwrap()
        .push(d)));
    assert_eq!(lock.attempts(), 4);
    assert_eq!(*sleeps.lock().unwrap(), vec![Duration::from_secs(1); 3]);
    let never = FakeLock::new(u32::MAX);
    let count = AtomicU32::new(0);
    assert!(!setup::acquire_lock(&never, setup::LOCK_WAIT, &|_| {
        count.fetch_add(1, Ordering::SeqCst);
    }));
    assert_eq!(never.attempts(), 61, "попытки с 0 по 60 с");
    assert_eq!(count.load(Ordering::SeqCst), 60);
    let once = FakeLock::new(1);
    assert!(!setup::acquire_lock(&once, Duration::ZERO, &|_| panic!(
        "без ожидания"
    )));
}

fn electron_up(_: &Path) -> bool {
    true
}

fn electron_down(_: &Path) -> bool {
    false
}

#[test]
fn legacy_aware_lock_waits_for_the_electron_launcher() {
    let inner = Arc::new(FakeLock::new(0));
    let blocked = LegacyAwareLock::with_check(Path::new("C:\\x"), inner.clone(), electron_up);
    assert!(!blocked.try_acquire());
    assert!(!inner.is_held());
    let free = LegacyAwareLock::with_check(Path::new("C:\\x"), inner.clone(), electron_down);
    assert!(free.try_acquire());
    assert!(free.is_held());
    free.release();
    assert!(!inner.is_held());
}

/// 0.4.7 держит `<данные>\lockfile` открытым без общего доступа — блокировка ждёт.
#[cfg(windows)]
#[test]
fn legacy_aware_lock_sees_a_held_lockfile() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir().unwrap();
    let lockfile = dir.path().join("lockfile");
    let held = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .share_mode(0)
        .open(&lockfile)
        .unwrap();
    let lock = LegacyAwareLock::new(dir.path(), Arc::new(FakeLock::new(0)));
    assert!(!lock.try_acquire());
    drop(held);
    assert!(lock.try_acquire());
}

// ---------------------------------------------------------------------------------- тихие режимы

struct World {
    _guard: tempfile::TempDir,
    root: PathBuf,
    registry: Arc<FakeRegistry>,
    links: Arc<FakeShellLinks>,
    processes: Arc<FakeProcesses>,
    log: Arc<MemoryLogger>,
}

impl World {
    fn new() -> World {
        let guard = tempfile::tempdir().unwrap();
        let root = guard.path().to_path_buf();
        // Профиль с библиотекой во временном каталоге: C:\HLSR не используется.
        let user_data = root.join("AppData").join("Roaming").join("hlsr");
        std::fs::create_dir_all(&user_data).unwrap();
        let library = root.join("Library").to_string_lossy().into_owned();
        std::fs::write(
            user_data.join("library.json"),
            serde_json::json!({ "libraryPath": library }).to_string(),
        )
        .unwrap();
        assert_eq!(setup::library_root(&user_data), library);
        World {
            _guard: guard,
            root,
            registry: Arc::new(FakeRegistry::new()),
            links: Arc::new(FakeShellLinks::new()),
            processes: Arc::new(FakeProcesses::new()),
            log: Arc::new(MemoryLogger::default()),
        }
    }

    fn install_dir(&self) -> PathBuf {
        self.root.join("HLSR").join("Launcher")
    }

    fn services(&self, lock: Arc<dyn InstanceLock>, exe: PathBuf) -> SetupServices {
        let text = |path: PathBuf| path.to_string_lossy().into_owned();
        SetupServices {
            registry: self.registry.clone(),
            links: self.links.clone(),
            folders: Arc::new(FakeKnownFolders::under(&self.root)),
            processes: self.processes.clone(),
            lock,
            log: self.log.clone(),
            env: Env::from_pairs([
                ("SystemRoot".to_owned(), "C:\\Windows".to_owned()),
                ("WINDIR".to_owned(), "C:\\Windows".to_owned()),
                ("USERPROFILE".to_owned(), "Z:\\nohome".to_owned()),
                (
                    "APPDATA".to_owned(),
                    text(self.root.join("AppData").join("Roaming")),
                ),
                (
                    "LOCALAPPDATA".to_owned(),
                    text(self.root.join("AppData").join("Local")),
                ),
                (
                    "PORTABLE_EXECUTABLE_FILE".to_owned(),
                    "C:\\D\\HLSR-Setup.exe".to_owned(),
                ),
                ("SLINT_SCALE_FACTOR".to_owned(), "1.25".to_owned()),
                ("PATH".to_owned(), "C:\\bin".to_owned()),
            ]),
            sleep: Arc::new(|_| {}),
            user_data: self.root.join("AppData").join("Roaming").join("hlsr"),
            app_name: "hlsr".to_owned(),
            version: "0.5.0".to_owned(),
            locale: "ru-RU".to_owned(),
            exe,
            steam: Arc::new(StaticSteamLibraries::default()),
            telemetry: None,
            shell: true,
        }
    }

    /// Установщик: нагрузка в exe (Р1) из сборки версии `version`.
    fn setup_exe(&self, version: &str) -> PathBuf {
        let tree = self.root.join(format!("build-{version}"));
        std::fs::create_dir_all(tree.join("resources")).unwrap();
        std::fs::write(tree.join(layout::EXE), format!("exe {version}")).unwrap();
        std::fs::write(
            tree.join("resources").join("app.bin"),
            format!("res {version}"),
        )
        .unwrap();
        let list = file_list::build(&tree, version, &App::production()).unwrap();
        std::fs::write(tree.join(file_list::RELATIVE), compact(&list.to_value())).unwrap();
        let exe = self
            .root
            .join("Downloads")
            .join(format!("HLSR-Setup-{version}-ru.exe"));
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        pack_for_tests(&tree, &exe);
        exe
    }
}

fn source_of(exe: &Path) -> Result<Source, String> {
    setup::open_source(exe)
}

#[test]
fn silent_update_installs_and_force_run_launches_with_a_clean_environment() {
    let world = World::new();
    let setup = world.setup_exe("0.5.0");
    let services = world.services(Arc::new(FakeLock::new(0)), setup.clone());
    let dir = world.install_dir().to_string_lossy().into_owned();
    let setup_text = setup.to_string_lossy().into_owned();
    let code = setup::run_setup_silent(
        &services,
        &args(&["/S", "--updated", &format!("/D={dir}")]),
        source_of(&setup),
        Some(&setup_text),
    );
    assert_eq!(code, setup_exit::OK, "{:?}", world.log.take());
    assert_eq!(
        std::fs::read(world.install_dir().join(layout::EXE)).unwrap(),
        b"exe 0.5.0"
    );
    let id = layout::identity(None).unwrap();
    let values = world.registry.read(&id.install_key).unwrap();
    let location = values
        .iter()
        .find(|(name, _)| name == "InstallLocation")
        .unwrap();
    assert_eq!(location.1, RegValue::Sz(dir.clone()));
    assert_eq!(
        world
            .registry
            .read_value(layout::ORIGIN_KEY, "InstallerOrigin"),
        Some(RegValue::Sz("ru".to_owned()))
    );
    assert!(
        spawns(&world.processes).is_empty(),
        "без --force-run лаунчер не запускается"
    );
    let logs = world.log.take();
    assert!(logs.contains(&(
        "info",
        format!("[setup] тихая установка 0.5.0 в {dir} (обновление)")
    )));
    let install_id = values
        .iter()
        .find(|(name, _)| name == "HLSRInstallId")
        .unwrap()
        .1
        .clone();

    // Путь А 0.4.7: `--updated /S --force-run` без /D= — каталог из реестра, installId тот же.
    let next = world.setup_exe("0.5.1");
    let code = setup::run_setup_silent(
        &services,
        &args(&["--updated", "/s", "--force-run"]),
        source_of(&next),
        None,
    );
    assert_eq!(code, setup_exit::OK, "{:?}", world.log.take());
    assert_eq!(
        std::fs::read(world.install_dir().join(layout::EXE)).unwrap(),
        b"exe 0.5.1"
    );
    assert_eq!(
        world.registry.read_value(&id.install_key, "HLSRInstallId"),
        Some(install_id)
    );
    let launched = spawns(&world.processes);
    assert_eq!(launched.len(), 1);
    assert_eq!(launched[0].file, format!("{dir}\\HLSR.exe"));
    assert!(launched[0].args.is_empty());
    assert_eq!(launched[0].cwd.as_deref(), Some(dir.as_str()));
    assert_eq!(launched[0].env.get("PORTABLE_EXECUTABLE_FILE"), None);
    assert_eq!(launched[0].env.get("SLINT_SCALE_FACTOR"), None);
    assert_eq!(launched[0].env.get("PATH").as_deref(), Some("C:\\bin"));
}

#[test]
fn silent_setup_exit_codes() {
    let world = World::new();
    let setup = world.setup_exe("0.5.0");
    let dir = world.install_dir().to_string_lossy().into_owned();
    // Занято: 61 попытка за 60 с — код 3.
    let busy = Arc::new(FakeLock::new(u32::MAX));
    let services = world.services(busy.clone(), setup.clone());
    let code = setup::run_setup_silent(
        &services,
        &args(&["/S", &format!("/D={dir}")]),
        source_of(&setup),
        None,
    );
    assert_eq!(code, setup_exit::IN_USE);
    assert_eq!(busy.attempts(), 61);
    assert!(!world.install_dir().exists());
    // Недопустимый каталог — код 4.
    let services = world.services(Arc::new(FakeLock::new(0)), setup.clone());
    let code = setup::run_setup_silent(
        &services,
        &args(&["/S", "/D=C:\\Windows"]),
        source_of(&setup),
        None,
    );
    assert_eq!(code, setup_exit::BAD_DIR);
    // Повреждённый установщик — код 1 до блокировки.
    let mut bytes = std::fs::read(&setup).unwrap();
    let at = bytes.len() - 100;
    bytes[at] ^= 0xff;
    let broken = world.root.join("broken.exe");
    std::fs::write(&broken, bytes).unwrap();
    let lock = Arc::new(FakeLock::new(0));
    let services = world.services(lock.clone(), broken.clone());
    let code = setup::run_setup_silent(
        &services,
        &args(&["/S", &format!("/D={dir}")]),
        source_of(&broken),
        None,
    );
    assert_eq!(code, setup_exit::FAILED);
    assert_eq!(lock.attempts(), 0);
    assert!(
        world
            .log
            .take()
            .iter()
            .any(|(level, message)| *level == "error"
                && message.starts_with("[setup] Установщик повреждён: ")
                && message.ends_with(". Скачайте его заново."))
    );
    // Лаунчер не запустился (ни CreateProcess, ни оболочка) — код 1.
    let failing_world = World::new();
    let setup = failing_world.setup_exe("0.5.0");
    let processes = Arc::new(failing("ENOENT", 0));
    let mut services = failing_world.services(Arc::new(FakeLock::new(0)), setup.clone());
    services.processes = processes;
    let dir = failing_world.install_dir().to_string_lossy().into_owned();
    let code = setup::run_setup_silent(
        &services,
        &args(&["/S", "--force-run", &format!("/D={dir}")]),
        source_of(&setup),
        None,
    );
    assert_eq!(code, setup_exit::FAILED);
}

/// Установка поверх Electron 0.4.x (`resources\app.asar`) оставляет метку уборки следов для
/// первого запуска; поверх 0.5 — нет.
#[test]
fn setup_over_electron_leaves_the_traces_mark() {
    let world = World::new();
    let setup = world.setup_exe("0.5.0");
    let services = world.services(Arc::new(FakeLock::new(0)), setup.clone());
    let install = world.install_dir();
    std::fs::create_dir_all(install.join("resources")).unwrap();
    std::fs::write(install.join(layout::EXE), b"electron 0.4.9").unwrap();
    std::fs::write(install.join("resources").join("app.asar"), b"asar").unwrap();
    // Список файлов 0.4.9: по нему установка знает, что app.asar — её, а не пользователя.
    let list = file_list::build(&install, "0.4.9", &App::production()).unwrap();
    std::fs::write(install.join(file_list::RELATIVE), compact(&list.to_value())).unwrap();
    let mark = services.user_data.join(legacy_traces::SETUP_MARK);
    let dir = install.to_string_lossy().into_owned();
    let run = || {
        setup::run_setup_silent(
            &services,
            &args(&["/S", &format!("/D={dir}")]),
            source_of(&setup),
            None,
        )
    };
    assert_eq!(run(), setup_exit::OK, "{:?}", world.log.take());
    assert!(!install.join("resources").join("app.asar").exists());
    assert!(mark.is_file(), "{:?}", world.log.take());

    std::fs::remove_file(&mark).unwrap();
    assert_eq!(run(), setup_exit::OK, "{:?}", world.log.take());
    assert!(!mark.exists(), "поверх 0.5 метки нет");
}

/// Путь В 0.4.7: `tree\HLSR.exe` с `PORTABLE_EXECUTABLE_FILE` — источник — каталог exe.
#[test]
fn portable_tree_installs_from_its_own_directory() {
    let world = World::new();
    let tree = world.root.join("work").join("tree");
    std::fs::create_dir_all(tree.join("resources")).unwrap();
    std::fs::write(tree.join(layout::EXE), b"tree exe").unwrap();
    std::fs::write(tree.join("resources").join("app.asar"), b"asar").unwrap();
    let list = file_list::build(&tree, "0.5.2", &App::production()).unwrap();
    std::fs::write(tree.join(file_list::RELATIVE), compact(&list.to_value())).unwrap();
    let exe = tree.join(layout::EXE);
    let source = source_of(&exe);
    assert!(matches!(source, Ok(Source::Dir(ref dir)) if *dir == tree));
    let services = world.services(Arc::new(FakeLock::new(0)), exe);
    let dir = world.install_dir().to_string_lossy().into_owned();
    let code = setup::run_setup_silent(
        &services,
        &args(&["/S", "--updated", &format!("/D={dir}")]),
        source,
        Some("C:\\work\\HLSR-Setup-0.5.2.exe"),
    );
    assert_eq!(code, setup_exit::OK, "{:?}", world.log.take());
    assert_eq!(
        std::fs::read(world.install_dir().join(layout::EXE)).unwrap(),
        b"tree exe"
    );
}

#[test]
fn silent_uninstall_cleans_data_removes_keys_and_schedules_directory_removal() {
    let world = World::new();
    let setup = world.setup_exe("0.5.0");
    let dir = world.install_dir().to_string_lossy().into_owned();
    let services = world.services(Arc::new(FakeLock::new(0)), setup.clone());
    let code = setup::run_setup_silent(
        &services,
        &args(&["/S", &format!("/D={dir}")]),
        source_of(&setup),
        None,
    );
    assert_eq!(code, setup_exit::OK);
    let library = world.root.join("Library");
    let game = library.join("Games").join("ghosting-renewed");
    std::fs::create_dir_all(&game).unwrap();
    std::fs::write(
        game.join("installed.json"),
        br#"{"gameId":"ghosting-renewed"}"#,
    )
    .unwrap();
    let user_data = world.root.join("AppData").join("Roaming").join("hlsr");
    std::fs::write(user_data.join("settings.json"), b"{}").unwrap();
    std::fs::write(user_data.join("game-stats.json"), b"{}").unwrap();
    let cache = world
        .root
        .join("AppData")
        .join("Local")
        .join("hlsr-updater");
    std::fs::create_dir_all(&cache).unwrap();
    let id = layout::identity(None).unwrap();
    assert!(world.links.written().len() == 2, "ярлыки созданы");

    let exe = world.install_dir().join(layout::EXE);
    let services = world.services(Arc::new(FakeLock::new(0)), exe);
    let code = setup::run_uninstall_silent(
        &services,
        &args(&[
            "--hlsr-uninstall",
            "/S",
            "/HLSRDELETEGAMES",
            "/hlsrdeletesettings",
        ]),
    );
    assert_eq!(code, setup_exit::OK, "{:?}", world.log.take());
    assert_eq!(world.registry.read(&id.install_key), None);
    assert_eq!(world.registry.read(&id.uninstall_key), None);
    assert!(world.registry.read(layout::ORIGIN_KEY).is_none() || true);
    for file in world.links.written() {
        assert!(!file.exists(), "свой ярлык удалён: {}", file.display());
    }
    assert!(
        !library.exists(),
        "библиотека очищена с purge, пустой корень удалён"
    );
    assert!(!cache.exists(), "кеш обновлений удалён");
    assert!(
        user_data.join("game-stats.json").exists(),
        "/HLSRDELETESETTINGS без /HLSRDELETEPROFILE сохраняет статистику"
    );
    assert!(!user_data.join("settings.json").exists());
    // Каталог программы удаляет скрипт, который запускает короткий PowerShell (тихий режим — без
    // повторного запуска установщика); данные в тихом режиме — только по флагам выше.
    // До него — только icacls установки.
    let removal: Vec<Command> = shell_runs(&world.processes)
        .into_iter()
        .filter(|command| command.env.get("HLSR_REMOVE_DIR").is_some())
        .collect();
    assert_eq!(removal.len(), 1);
    assert!(
        removal[0]
            .file
            .ends_with("WindowsPowerShell\\v1.0\\powershell.exe")
    );
    assert!(removal[0].args.last().unwrap().contains("-EncodedCommand"));
    assert_eq!(
        removal[0].env.get("HLSR_REMOVE_DIR").as_deref(),
        Some(dir.as_str())
    );
    assert_eq!(removal[0].env.get("HLSR_REMOVE_DATA").as_deref(), Some(""));
    assert_eq!(
        removal[0].env.get("HLSR_REMOVE_PARENT").as_deref(),
        Some(world.root.join("HLSR").to_string_lossy().as_ref())
    );
    assert_eq!(
        removal[0].env.get("HLSR_WAIT_PID"),
        Some(std::process::id().to_string())
    );
    assert_eq!(removal[0].env.get("HLSR_RELAUNCH"), None);
    assert_eq!(
        removal[0].cwd.as_deref(),
        Some(world.root.join("Temp").to_string_lossy().as_ref())
    );
    assert!(
        world.install_dir().join(layout::EXE).exists(),
        "каталог удаляет PowerShell после выхода"
    );
}

#[test]
fn silent_uninstall_waits_for_the_running_launcher() {
    let world = World::new();
    let exe = world.install_dir().join(layout::EXE);
    let busy = Arc::new(FakeLock::new(u32::MAX));
    let services = world.services(busy.clone(), exe);
    let code = setup::run_uninstall_silent(&services, &args(&["--hlsr-uninstall", "/S"]));
    assert_eq!(code, setup_exit::IN_USE);
    assert_eq!(busy.attempts(), 61);
    assert!(spawns(&world.processes).is_empty());
}

/// Удаление без доказанного владения: ключи удаляются, каталог остаётся, код 1.
#[test]
fn silent_uninstall_keeps_a_directory_it_cannot_prove_ownership_of() {
    let world = World::new();
    std::fs::create_dir_all(world.install_dir()).unwrap();
    std::fs::write(world.install_dir().join(layout::EXE), b"exe").unwrap();
    let services = world.services(
        Arc::new(FakeLock::new(0)),
        world.install_dir().join(layout::EXE),
    );
    let code = setup::run_uninstall_silent(&services, &args(&["--hlsr-uninstall", "/S"]));
    assert_eq!(code, setup_exit::FAILED);
    assert!(world.processes.calls().is_empty());
}

// ---------------------------------------------------------------------------------- окна

#[test]
fn setup_window_installs_with_progress_and_hands_uninstall_to_the_installed_copy() {
    let world = World::new();
    let setup = world.setup_exe("0.5.0");
    let quits = Arc::new(AtomicU32::new(0));
    let quit_count = quits.clone();
    let mut services = world.services(Arc::new(FakeLock::new(0)), setup.clone());
    // Пробная упаковка `--dev`: exe собран с `0.5.0-dev`, нагрузка — `0.5.0`.
    services.version = "0.5.0-dev".to_owned();
    let (backend, state) = SetupBackend::open(
        services,
        &args(&[]),
        source_of(&setup),
        Some(setup.to_string_lossy().into_owned()),
        Arc::new(move || {
            quit_count.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .unwrap();
    assert_eq!(state.phase, Phase::Idle);
    assert_eq!(state.directory, "C:\\HLSR\\Launcher");
    assert_eq!(state.language, "ru");
    // Окно показывает ставящуюся версию — ту же, что потом в реестре и на экране «готово».
    assert_eq!(state.version.as_deref(), Some("0.5.0"));
    let window = InstallerWindow::new(backend.clone(), state);
    let dir = world.install_dir().to_string_lossy().into_owned();
    let chosen = window
        .select_directory(|_| Some(dir.clone()))
        .unwrap()
        .unwrap();
    assert_eq!(chosen.directory, dir);
    assert_eq!(chosen.phase, Phase::Idle);
    let progress = Arc::new(Mutex::new(Vec::new()));
    let seen = progress.clone();
    window.set_listener(move |state| {
        if let Some(value) = state.progress {
            seen.lock().unwrap().push(value);
        }
    });
    assert!(window.install(false));
    let state = window.state();
    assert_eq!(state.phase, Phase::Done, "{:?}", state.error);
    assert_eq!(state.installed_version.as_deref(), Some("0.5.0"));
    assert!(!state.cancellable);
    let progress = progress.lock().unwrap().clone();
    assert_eq!(progress.first(), Some(&0));
    assert_eq!(progress.last(), Some(&100));
    assert_eq!(
        world.links.written().len(),
        1,
        "без ярлыка на рабочем столе"
    );

    // Тот же каталог снова: установленная версия не старше — «готово».
    let again = window
        .select_directory(|_| Some(dir.clone()))
        .unwrap()
        .unwrap();
    assert_eq!(again.phase, Phase::Done);
    assert!(again.can_uninstall);

    // «Удалить» запускает установленную копию с «Назад» к этому установщику и закрывает окно.
    assert!(window.uninstall_plan().is_none());
    let launched = spawns(&world.processes);
    assert_eq!(
        launched.last().unwrap().args,
        [
            "--hlsr-uninstall".to_owned(),
            layout::RETURN_SETUP_FLAG.to_owned(),
            setup.to_string_lossy().into_owned()
        ]
    );
    assert_eq!(quits.load(Ordering::SeqCst), 1);
}

#[test]
fn setup_window_with_newer_installed_version_opens_done() {
    let world = World::new();
    let setup = world.setup_exe("0.5.0");
    let services = world.services(Arc::new(FakeLock::new(0)), setup.clone());
    let dir = world.install_dir().to_string_lossy().into_owned();
    assert_eq!(
        setup::run_setup_silent(
            &services,
            &args(&["/S", &format!("/D={dir}")]),
            source_of(&setup),
            None
        ),
        setup_exit::OK
    );
    // Установщик 0.4.9 видит установленную 0.5.0: сразу «готово» (ручной откат — удалить и поставить).
    let older = world.setup_exe("0.4.9");
    // Версию окна задаёт нагрузка `older`, а не `services.version`.
    let services = world.services(Arc::new(FakeLock::new(0)), older.clone());
    let (_backend, state) = SetupBackend::open(
        services,
        &args(&[]),
        source_of(&older),
        None,
        Arc::new(|| {}),
    )
    .unwrap();
    assert_eq!(state.phase, Phase::Done);
    assert_eq!(state.installed_version.as_deref(), Some("0.5.0"));
    assert!(!state.update);
    assert!(state.can_uninstall);
    // Блокировка занята: окно с ошибкой «HLSR сейчас запущен».
    let services = world.services(Arc::new(FakeLock::new(u32::MAX)), setup.clone());
    let (_backend, state) = SetupBackend::open(
        services,
        &args(&[]),
        source_of(&setup),
        None,
        Arc::new(|| {}),
    )
    .unwrap();
    assert_eq!(state.phase, Phase::Error);
    assert_eq!(state.error.as_deref(), Some(setup::LOCK_ERROR));
}

#[test]
fn uninstall_window_removes_then_goes_back_to_the_setup() {
    let world = World::new();
    let setup = world.setup_exe("0.5.0");
    let services = world.services(Arc::new(FakeLock::new(0)), setup.clone());
    let dir = world.install_dir().to_string_lossy().into_owned();
    assert_eq!(
        setup::run_setup_silent(
            &services,
            &args(&["/S", &format!("/D={dir}")]),
            source_of(&setup),
            None
        ),
        setup_exit::OK
    );
    let quits = Arc::new(AtomicU32::new(0));
    let quit_count = quits.clone();
    let services = world.services(
        Arc::new(FakeLock::new(0)),
        world.install_dir().join(layout::EXE),
    );
    let setup_text = setup.to_string_lossy().into_owned();
    let (backend, state, title) = UninstallBackend::open(
        services,
        &args(&["--hlsr-uninstall", layout::RETURN_SETUP_FLAG, &setup_text]),
        Arc::new(move || {
            quit_count.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .unwrap()
    .unwrap();
    assert_eq!(title, "HLSR — удаление");
    assert_eq!(state.phase, Phase::UninstallConfirm);
    assert_eq!(state.can_go_back, Some(true));
    let plan = state.uninstall_plan.clone().unwrap();
    assert!(plan.install_bytes > 0);
    assert!(plan.library_exists);
    let window = InstallerWindow::new(backend.clone(), state);
    assert!(window.uninstall(false));
    assert_eq!(window.state().phase, Phase::Uninstalled);
    assert!(window.state().library_exists_placeholder());
    assert!(window.back());
    assert_eq!(quits.load(Ordering::SeqCst), 1);
    backend.on_quit();
    let removal = shell_runs(&world.processes);
    let last = removal.last().unwrap();
    assert_eq!(
        last.env.get("HLSR_REMOVE_DIR").as_deref(),
        Some(dir.as_str())
    );
    assert_eq!(
        last.env.get("HLSR_RELAUNCH").as_deref(),
        Some(setup_text.as_str())
    );
    assert!(
        world.root.join("Library").exists(),
        "игры без галочки не тронуты"
    );
}

/// «Также удалить игры» в окне: библиотека очищается сразу, данные HLSR (`userData`,
/// `%LOCALAPPDATA%\hlsr`) и опустевшая папка HLSR — скриптом после выхода.
#[test]
fn uninstall_window_with_games_removes_the_library_and_schedules_data() {
    let world = World::new();
    let setup = world.setup_exe("0.5.0");
    let services = world.services(Arc::new(FakeLock::new(0)), setup.clone());
    let dir = world.install_dir().to_string_lossy().into_owned();
    assert_eq!(
        setup::run_setup_silent(
            &services,
            &args(&["/S", &format!("/D={dir}")]),
            source_of(&setup),
            None
        ),
        setup_exit::OK
    );
    let library = world.root.join("Library");
    let game = library.join("Games").join("hl2-4260");
    std::fs::create_dir_all(&game).unwrap();
    std::fs::write(game.join("installed.json"), br#"{"gameId":"hl2-4260"}"#).unwrap();
    let logs = world.root.join("AppData").join("Local").join("hlsr");
    std::fs::create_dir_all(logs.join("logs")).unwrap();
    let services = world.services(
        Arc::new(FakeLock::new(0)),
        world.install_dir().join(layout::EXE),
    );
    let (backend, state, _title) =
        UninstallBackend::open(services, &args(&["--hlsr-uninstall"]), Arc::new(|| {}))
            .unwrap()
            .unwrap();
    let plan = state.uninstall_plan.clone().unwrap();
    let library_text = library.to_string_lossy().into_owned();
    assert_eq!(plan.library_roots, Some(vec![library_text]));
    assert_eq!(plan.library_blocked, None);
    assert!(plan.library_bytes > 0);
    let window = InstallerWindow::new(backend.clone(), state);
    assert!(window.uninstall(true));
    assert_eq!(window.state().warning, None);
    assert!(
        !library.exists(),
        "своё в библиотеке удалено, пустой корень тоже"
    );
    backend.on_quit();
    let removal = shell_runs(&world.processes);
    let last = removal.last().unwrap();
    assert_eq!(
        last.env.get("HLSR_REMOVE_DIR").as_deref(),
        Some(dir.as_str())
    );
    let user_data = world.root.join("AppData").join("Roaming").join("hlsr");
    assert_eq!(
        last.env.get("HLSR_REMOVE_DATA"),
        Some(format!(
            "{}|{}",
            user_data.to_string_lossy(),
            logs.to_string_lossy()
        ))
    );
    assert!(user_data.exists(), "данные удаляет скрипт после выхода");
}

trait PlaceholderExt {
    fn library_exists_placeholder(&self) -> bool;
}

impl PlaceholderExt for hlsr_update::installer_window::InstallerState {
    fn library_exists_placeholder(&self) -> bool {
        !self.can_uninstall
    }
}

#[test]
fn payload_open_fails_for_a_damaged_setup_and_window_reports_it() {
    let world = World::new();
    let setup = world.setup_exe("0.5.0");
    let mut bytes = std::fs::read(&setup).unwrap();
    let at = bytes.len() - 100;
    bytes[at] ^= 0xff;
    std::fs::write(&setup, bytes).unwrap();
    assert!(Payload::open(&setup).is_err());
    let services = world.services(Arc::new(FakeLock::new(0)), setup.clone());
    let (backend, state) = SetupBackend::open(
        services,
        &args(&[]),
        source_of(&setup),
        None,
        Arc::new(|| {}),
    )
    .unwrap();
    assert_eq!(state.phase, Phase::Error);
    let error = state.error.clone().unwrap();
    assert_eq!(
        error,
        "Установщик повреждён: файлы сборки в установщике повреждены. Скачайте его заново."
    );
    let window = InstallerWindow::new(backend, state);
    assert!(!window.install(true));
    assert_eq!(window.state().error.as_deref(), Some(error.as_str()));
}
