//! Самотесты 0.4.7 Linux-установки (`launcher-update-selftest.js`, `launcher-chunks-selftest.js` тест 4)
//! и проверки каталогов пользователя. Без ссылок — нативно на любой ОС; установка версий (`commit`:
//! ссылки `current`/`previous`, права), AppImage-заглушка (shell) и `/proc` — только Linux/Unix.

mod support;

use hlsr_update::installer_window::{InstallerState, Phase};
use hlsr_update::linux::bootstrap_installer::{self, BootstrapOptions, BootstrapServices};
use hlsr_update::linux::embedded_payload::{self, Describe};
use hlsr_update::linux::{
    FakeRunner, LinuxDirs, archive_install::ArchiveInstall, launcher_uninstall, library_cleanup,
};
use hlsr_update::update_manager::AppControl;
use hlsr_update::updater::processes::FakeLauncher;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Quit(AtomicUsize);

impl AppControl for Quit {
    fn quit(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn dirs_under(dir: &Path) -> LinuxDirs {
    LinuxDirs {
        home: dir.join("home"),
        data_home: dir.join("data"),
        desktop: dir.join("home").join("Рабочий стол"),
        // Как в Linux: данные лаунчера — каталог `hlsr` (~/.config/hlsr).
        config: dir.join("config").join("hlsr"),
    }
}

struct Opened {
    window: Arc<hlsr_update::installer_window::InstallerWindow>,
    states: Arc<Mutex<Vec<InstallerState>>>,
    #[cfg_attr(not(unix), allow(dead_code))]
    launcher: Arc<FakeLauncher>,
    #[cfg_attr(not(unix), allow(dead_code))]
    quit: Arc<Quit>,
    /// Сколько раз окно просило выйти из сессии (полное удаление).
    forgot: Arc<AtomicUsize>,
}

impl Opened {
    fn last(&self) -> InstallerState {
        self.states
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("состояние окна")
    }
}

fn open(
    root: &Path,
    payload: &Path,
    dirs: &LinuxDirs,
    runtime: &tokio::runtime::Runtime,
) -> Opened {
    let launcher = Arc::new(FakeLauncher::new());
    let quit = Arc::new(Quit::default());
    let forgot = Arc::new(AtomicUsize::new(0));
    let counter = forgot.clone();
    let window = bootstrap_installer::open_installer(
        BootstrapOptions {
            root: Some(root.to_owned()),
            payload_dir: payload.to_owned(),
            language: "ru".to_owned(),
        },
        BootstrapServices {
            dirs: dirs.clone(),
            env: vec![("DISPLAY".to_owned(), ":1".to_owned())],
            runner: Arc::new(FakeRunner::new()),
            launcher: launcher.clone(),
            app: quit.clone(),
            runtime: runtime.handle().clone(),
            forget_session: Arc::new(move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
        },
    );
    let states: Arc<Mutex<Vec<InstallerState>>> = Arc::default();
    let sink = states.clone();
    window.set_listener(move |state| sink.lock().unwrap().push(state.clone()));
    Opened {
        window,
        states,
        launcher,
        quit,
        forgot,
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn launching_from_an_appimage_removes_installer_only_environment() {
    let env = bootstrap_installer::launch_environment(&[
        ("APPIMAGE".to_owned(), "/tmp/setup".to_owned()),
        ("APPDIR".to_owned(), "/tmp/mount".to_owned()),
        (
            "LD_LIBRARY_PATH".to_owned(),
            "/tmp/mount/lib:/custom".to_owned(),
        ),
        ("DISPLAY".to_owned(), ":1".to_owned()),
    ]);
    let get = |name: &str| {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(get("APPIMAGE"), None);
    assert_eq!(get("APPDIR"), None);
    assert_eq!(get("LD_LIBRARY_PATH"), Some("/custom"));
    assert_eq!(get("DISPLAY"), Some(":1"));
    // Только каталоги AppImage — переменная уходит целиком.
    let env = bootstrap_installer::launch_environment(&[
        ("APPDIR".to_owned(), "/tmp/mount".to_owned()),
        (
            "LD_LIBRARY_PATH".to_owned(),
            "/tmp/mount/lib:/tmp/mount/usr/lib".to_owned(),
        ),
    ]);
    assert!(env.is_empty(), "{env:?}");
}

#[test]
fn installer_offers_uninstall_games_only_with_explicit_consent() {
    let runtime = runtime();
    let dir = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(dir.path()).unwrap();
    let dirs = dirs_under(&dir);
    let root = dir.join("opt").join("hlsr");
    let library = dir.join("Games").join("HLSR");
    // Журналы лаунчера — ~/.local/state/hlsr.
    let logs = dirs.home.join(".local").join("state").join("hlsr");
    let fixture = || {
        std::fs::create_dir_all(root.join("versions").join("0.0.1")).unwrap();
        std::fs::write(
            root.join(".hlsr-install.json"),
            json!({"schema": 1}).to_string(),
        )
        .unwrap();
        std::fs::create_dir_all(library.join("Games").join("ghosting-renewed")).unwrap();
        std::fs::write(
            library
                .join("Games")
                .join("ghosting-renewed")
                .join("installed.json"),
            json!({"gameId": "ghosting-renewed"}).to_string(),
        )
        .unwrap();
        std::fs::create_dir_all(&dirs.config).unwrap();
        std::fs::write(
            dirs.config.join("library.json"),
            json!({"libraryPath": library.to_string_lossy()}).to_string(),
        )
        .unwrap();
        std::fs::create_dir_all(logs.join("logs")).unwrap();
        std::fs::write(logs.join("logs").join("main.log"), "log").unwrap();
    };
    fixture();
    // Удалению встроенный архив не нужен, но установщик без описания считается повреждённым.
    let payload = dir.join(embedded_payload::DIRECTORY);
    std::fs::create_dir_all(&payload).unwrap();
    let sha512 = format!("{}==", "A".repeat(86));
    std::fs::write(
        payload.join(embedded_payload::DESCRIPTOR),
        embedded_payload::describe(&Describe {
            version: "0.0.2",
            size: 1,
            sha256: &"0".repeat(64),
            sha512: &sha512,
            unpacked_size: 1,
        })
        .to_string(),
    )
    .unwrap();

    let opened = open(&root, &payload, &dirs, &runtime);
    let initial = opened.window.state();
    assert_eq!(initial.phase, Phase::Idle);
    assert!(initial.can_uninstall);
    let plan = opened.window.uninstall_plan().expect("план удаления");
    assert_eq!(
        plan.library_root.as_deref(),
        Some(library.to_string_lossy().as_ref())
    );
    assert!(plan.library_bytes > 0);
    assert!(opened.window.uninstall(false));
    assert!(!root.exists());
    assert!(
        library
            .join("Games")
            .join("ghosting-renewed")
            .join("installed.json")
            .exists()
    );
    // Без галочки данные остаются: переустановка сохранит настройки и вход.
    assert!(dirs.config.join("library.json").exists());
    assert!(logs.join("logs").join("main.log").exists());
    assert_eq!(opened.forgot.load(Ordering::SeqCst), 0);
    assert_eq!(opened.last().phase, Phase::Uninstalled);
    // «Назад» возвращает к установке в тот же, теперь пустой каталог.
    assert!(opened.window.back());
    let back = opened.window.state();
    assert_eq!(back.phase, Phase::Idle);
    assert!(!back.can_uninstall);
    assert_eq!(back.directory, initial.directory);
    assert!(!opened.window.back());

    // Полное удаление: игры, опустевшая папка HLSR и данные лаунчера, но не чужое рядом с ними.
    fixture();
    let opened = open(&root, &payload, &dirs, &runtime);
    assert!(opened.window.uninstall(true));
    assert!(!library.join("Games").exists());
    assert!(!library.exists());
    assert!(!dirs.config.exists());
    assert!(!logs.exists());
    assert!(dir.join("config").exists());
    assert_eq!(opened.forgot.load(Ordering::SeqCst), 1);
    assert_eq!(opened.last().warning, None);
    // С маркером библиотека уходит целиком, если в ней нет чужого.
    fixture();
    library_cleanup::write_marker(&library).unwrap();
    let opened = open(&root, &payload, &dirs, &runtime);
    assert!(opened.window.uninstall(true));
    assert!(!library.exists());
    // Чужие файлы и папки остаются, окно о них говорит.
    fixture();
    library_cleanup::write_marker(&library).unwrap();
    std::fs::write(library.join("notes.txt"), "mine").unwrap();
    std::fs::create_dir_all(library.join("Games").join("MyMod")).unwrap();
    let opened = open(&root, &payload, &dirs, &runtime);
    assert!(opened.window.uninstall(true));
    let names = |dir: &Path| {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    assert_eq!(names(&library), ["Games", "notes.txt"]);
    assert_eq!(names(&library.join("Games")), ["MyMod"]);
    let warning = opened.last().warning.expect("предупреждение");
    assert_eq!(
        warning,
        format!(
            "Оставлено в {} (2), удаление это не трогает: {}, notes.txt",
            library.to_string_lossy(),
            Path::new("Games").join("MyMod").to_string_lossy()
        )
    );
}

#[test]
fn typed_in_library_keeps_foreign_tools_downloads_backups_demos() {
    // 0.4.9: во вписанной руками библиотеке (~/Games) чужие Downloads, Tools, Demos, Backups остаются.
    let dir = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(dir.path()).unwrap();
    let dirs = dirs_under(&dir);
    let root = dir.join("opt").join("hlsr");
    let install = || {
        std::fs::create_dir_all(root.join("versions").join("0.0.1")).unwrap();
        std::fs::write(root.join(".hlsr-install.json"), "{\"schema\":1}").unwrap();
    };
    install();
    let library = dir.join("Games");
    let files = [
        ("Games/hl2-4260/installed.json", "{\"gameId\":\"hl2-4260\"}"),
        ("Games/hl1-won/readme.txt", "x"),
        ("Downloads/firefox.tar.gz", "x"),
        ("Tools/7z/7z", "x"),
        ("Demos/csgo/a.dem", "x"),
        ("Backups/photos.zip", "x"),
        (".hlsr/library.json", "{}"),
    ];
    for (relative, body) in files {
        let file = library.join(relative);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, body).unwrap();
    }
    let outcome =
        launcher_uninstall::uninstall_launcher(&root, &library.to_string_lossy(), true, &[], &dirs)
            .unwrap();
    let mut alive: Vec<&str> = files
        .iter()
        .map(|(relative, _)| *relative)
        .filter(|relative| library.join(relative).exists())
        .collect();
    alive.sort();
    assert_eq!(
        alive,
        [
            "Backups/photos.zip",
            "Demos/csgo/a.dem",
            "Downloads/firefox.tar.gz",
            "Games/hl1-won/readme.txt",
            "Tools/7z/7z"
        ]
    );
    let hl1 = Path::new("Games")
        .join("hl1-won")
        .to_string_lossy()
        .into_owned();
    assert!(
        outcome.left.contains(&"Downloads".to_owned()) && outcome.left.contains(&hl1),
        "{:?}",
        outcome.left
    );

    // Своя папка «hlsr» (не ровно HLSR) — тоже чужая раскладка.
    let own = dir.join("hlsr");
    install();
    std::fs::create_dir_all(own.join("Downloads")).unwrap();
    std::fs::write(own.join("Downloads").join("mine.zip"), "x").unwrap();
    launcher_uninstall::uninstall_launcher(&root, &own.to_string_lossy(), true, &[], &dirs)
        .unwrap();
    assert!(own.join("Downloads").join("mine.zip").exists());

    // ownDataDir: только каталог «hlsr», не корень и не домашний каталог.
    let home = &dirs.home;
    assert!(launcher_uninstall::own_data_dir(
        &home.join(".config").join("hlsr"),
        home
    ));
    assert!(!launcher_uninstall::own_data_dir(
        &home.join(".config").join("other"),
        home
    ));
    assert!(!launcher_uninstall::own_data_dir(
        &home.join(".config"),
        home
    ));
    let home_named_hlsr = dir.join("hlsr");
    assert!(!launcher_uninstall::own_data_dir(
        &home_named_hlsr,
        &home_named_hlsr
    ));
}

#[test]
fn desktop_copies_are_removed_from_the_xdg_desktop_too() {
    // Решение 17: копия ярлыка ищется на рабочем столе XDG и в ~/Desktop (0.4.7 — только ~/Desktop).
    let dir = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(dir.path()).unwrap();
    let dirs = dirs_under(&dir);
    let root = dir.join("opt").join("hlsr");
    std::fs::create_dir_all(root.join("versions")).unwrap();
    std::fs::write(root.join(".hlsr-install.json"), "{\"schema\":1}\n").unwrap();
    let managed = "[Desktop Entry]\nX-HLSR-Managed=true\n";
    for desktop in [dirs.desktop.clone(), dirs.home.join("Desktop")] {
        std::fs::create_dir_all(&desktop).unwrap();
        std::fs::write(desktop.join("hlsr.desktop"), managed).unwrap();
    }
    let foreign = dir.join("foreign desktop");
    std::fs::create_dir_all(&foreign).unwrap();
    std::fs::write(
        foreign.join("hlsr.desktop"),
        "[Desktop Entry]\nName=Other\n",
    )
    .unwrap();
    let icon = hlsr_update::linux::desktop_integration::icon_path(&dirs.data_home);
    std::fs::create_dir_all(icon.parent().unwrap()).unwrap();
    std::fs::write(&icon, "png").unwrap();

    let library = dir.join("Games").join("HLSR");
    let outcome = launcher_uninstall::uninstall_launcher(
        &root,
        &library.to_string_lossy(),
        false,
        &[],
        &dirs,
    )
    .unwrap();
    assert_eq!(outcome.status, "ok");
    assert!(outcome.desktop_removed && outcome.icon_removed && !outcome.games_removed);
    assert!(!dirs.desktop.join("hlsr.desktop").exists());
    assert!(!dirs.home.join("Desktop").join("hlsr.desktop").exists());
    assert!(foreign.join("hlsr.desktop").exists());
    assert!(!icon.exists());
    assert!(!root.exists());
}

#[test]
fn managed_root_checks_follow_0_4_7() {
    let dir = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(dir.path()).unwrap();
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let other = dir.join("other");
    std::fs::create_dir_all(&other).unwrap();
    // Нет маркера и не корень по умолчанию — не установка.
    let error = launcher_uninstall::assert_managed_root(&other, &home).unwrap_err();
    assert_eq!(
        (error.code, error.message.as_str()),
        ("NOT_MANAGED", "Каталог не похож на установку HLSR")
    );
    // Маркер без versions/.
    std::fs::write(other.join(".hlsr-install.json"), "{\"schema\":1}").unwrap();
    assert_eq!(
        launcher_uninstall::assert_managed_root(&other, &home)
            .unwrap_err()
            .message,
        "Каталог не похож на установку HLSR"
    );
    std::fs::create_dir(other.join("versions")).unwrap();
    assert_eq!(
        launcher_uninstall::assert_managed_root(&other, &home).unwrap(),
        other
    );
    // Нет каталога.
    assert_eq!(
        launcher_uninstall::assert_managed_root(&dir.join("missing"), &home)
            .unwrap_err()
            .message,
        "Управляемая установка HLSR не найдена"
    );
    // Домашний каталог — никогда.
    std::fs::write(home.join(".hlsr-install.json"), "{\"schema\":1}").unwrap();
    std::fs::create_dir(home.join("versions")).unwrap();
    assert!(!launcher_uninstall::is_managed_root(&home, &home));
    // Осиротевший корень по умолчанию: маркер снесён, versions/ уцелел.
    let default = hlsr_update::linux::archive_install::default_root(&home);
    std::fs::create_dir_all(default.join("versions")).unwrap();
    assert!(launcher_uninstall::is_managed_root(&default, &home));
    // План: библиотека — абсолютный путь, не корень и не дом.
    let dirs = LinuxDirs {
        home: home.clone(),
        data_home: dir.join("data"),
        desktop: home.join("Desktop"),
        config: dir.join("config"),
    };
    for (library, message) in [
        ("", "Путь библиотеки не задан"),
        ("   ", "Путь библиотеки не задан"),
        ("Games/HLSR", "Недопустимый путь библиотеки: Games/HLSR"),
    ] {
        let error =
            launcher_uninstall::plan_launcher_uninstall(&other, library, &dirs).unwrap_err();
        assert_eq!(
            (error.code, error.message.as_str()),
            ("PATH_INVALID", message)
        );
    }
    let error = launcher_uninstall::plan_launcher_uninstall(&other, &home.to_string_lossy(), &dirs)
        .unwrap_err();
    assert_eq!(
        error.message,
        "Библиотекой не может быть корень диска или домашний каталог"
    );
}

#[test]
fn xdg_user_dirs_like_xdg_user_dir_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let config = home.join(".config");
    std::fs::create_dir_all(&config).unwrap();
    let env = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    };
    // Файла нет — ~/Desktop.
    let dirs = LinuxDirs::from_env(&home, &env(&[]), &config.join("hlsr"));
    assert_eq!(dirs.desktop, home.join("Desktop"));
    assert_eq!(dirs.data_home, home.join(".local").join("share"));
    std::fs::write(
        config.join("user-dirs.dirs"),
        "# comment\nXDG_DOWNLOAD_DIR=\"$HOME/Загрузки\"\n  XDG_DESKTOP_DIR=\"$HOME/Рабочий стол\"\nXDG_DESKTOP_DIRX=\"/wrong\"\nXDG_DESKTOP_DIR=relative\n",
    )
    .unwrap();
    let dirs = LinuxDirs::from_env(&home, &env(&[]), &config.join("hlsr"));
    assert_eq!(dirs.desktop, home.join("Рабочий стол"));
    std::fs::write(
        config.join("user-dirs.dirs"),
        "XDG_DESKTOP_DIR=\"/srv/desk\\\"top\"\n",
    )
    .unwrap();
    assert_eq!(
        hlsr_update::linux::xdg_user_dir(&home, &config, "DESKTOP"),
        Some(PathBuf::from("/srv/desk\"top"))
    );
    // XDG_DATA_HOME — только абсолютный; XDG_CONFIG_HOME задаёт, где искать user-dirs.dirs.
    let other = dir.path().join("cfg");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(
        other.join("user-dirs.dirs"),
        "XDG_DESKTOP_DIR=\"$HOME/Desk\"\n",
    )
    .unwrap();
    let absolute = dir.path().join("share").to_string_lossy().into_owned();
    let dirs = LinuxDirs::from_env(
        &home,
        &env(&[
            ("XDG_DATA_HOME", &absolute),
            ("XDG_CONFIG_HOME", &other.to_string_lossy()),
        ]),
        &config.join("hlsr"),
    );
    assert_eq!(dirs.data_home, PathBuf::from(&absolute));
    assert_eq!(dirs.desktop, home.join("Desk"));
    let dirs = LinuxDirs::from_env(
        &home,
        &env(&[("XDG_DATA_HOME", "relative/share")]),
        &config.join("hlsr"),
    );
    assert_eq!(dirs.data_home, home.join(".local").join("share"));
}

#[test]
fn library_path_comes_from_the_launcher_store() {
    let dir = tempfile::tempdir().unwrap();
    let dirs = dirs_under(dir.path());
    assert_eq!(
        bootstrap_installer::renderer_library_path(&dirs),
        dirs.home.join("Games").join("HLSR").to_string_lossy()
    );
    std::fs::create_dir_all(&dirs.config).unwrap();
    std::fs::write(
        dirs.config.join("library.json"),
        json!({"libraryPath": "  /data/Игры  "}).to_string(),
    )
    .unwrap();
    assert_eq!(
        bootstrap_installer::renderer_library_path(&dirs),
        "/data/Игры"
    );
    std::fs::write(
        dirs.config.join("library.json"),
        json!({"libraryPath": "   "}).to_string(),
    )
    .unwrap();
    assert_eq!(
        bootstrap_installer::renderer_library_path(&dirs),
        dirs.home.join("Games").join("HLSR").to_string_lossy()
    );
    assert_eq!(
        bootstrap_installer::version_of(Some("versions/0.4.7-a00ebe2bb04a9f54")).as_deref(),
        Some("0.4.7")
    );
    assert_eq!(bootstrap_installer::version_of(Some("elsewhere")), None);
    assert_eq!(bootstrap_installer::version_of(None), None);
}

#[test]
fn broken_payload_is_reported_and_installs_nothing() {
    let runtime = runtime();
    let dir = tempfile::tempdir().unwrap();
    let dirs = dirs_under(dir.path());
    let root = dir.path().join("opt").join("hlsr");
    let opened = open(&root, &dir.path().join("missing payload"), &dirs, &runtime);
    let state = opened.window.state();
    assert_eq!(state.phase, Phase::Error);
    assert!(
        state
            .error
            .as_deref()
            .unwrap()
            .ends_with(". Скачайте установщик заново."),
        "{state:?}"
    );
    assert!(
        state
            .error
            .as_deref()
            .unwrap()
            .starts_with("В установщике нет встроенного HLSR: ")
    );
    assert!(!opened.window.install(false));
    assert!(!root.exists());
}

#[test]
fn parallel_preparation_is_locked_and_cancel_releases_the_lock() {
    let runtime = runtime();
    let dir = tempfile::tempdir().unwrap();
    let release = support::ustar::release("0.4.8", &support::ustar::launcher_entries("0.4.8"));
    let archive = dir.path().join("a.tar.gz");
    std::fs::write(&archive, &release.bytes).unwrap();
    let root = dir.path().join("install");
    let mut first = ArchiveInstall::new(&root);
    runtime
        .block_on(first.prepare(&archive, &release.manifest, None))
        .unwrap();
    let error = runtime
        .block_on(ArchiveInstall::new(&root).prepare(&archive, &release.manifest, None))
        .unwrap_err();
    assert_eq!(error, "Другая установка уже выполняется");
    first.discard();
    let cancel = hlsr_update::cancel::CancelToken::new();
    cancel.cancel();
    let error = runtime
        .block_on(ArchiveInstall::new(&root).prepare(&archive, &release.manifest, Some(&cancel)))
        .unwrap_err();
    assert_eq!(error, "Распаковка отменена");
    assert!(!root.join(".update-lock").exists());
}

/// `.update-lock`, который не удалился, не остаётся навсегда: ошибка удаления не снимает блокировку
/// (`locked()` остаётся истиной), и следующий `discard()` повторяет удаление. Иначе брошенный файл со
/// своим pid и идентичностью (не «устаревший») отказывал бы каждой подготовке до перезапуска.
#[test]
fn a_lock_file_that_would_not_go_is_removed_by_the_next_discard() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("install");
    let lock_path = root.join(".update-lock");
    let mut install = ArchiveInstall::new(&root);
    install.lock().unwrap();
    assert!(install.locked() && lock_path.exists());

    // Удаление не удаётся. Windows: файл держит чужой дескриптор без FILE_SHARE_DELETE;
    // Unix: на месте файла каталог.
    #[cfg(windows)]
    let blocker = {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1 | 2)
            .open(&lock_path)
            .unwrap()
    };
    #[cfg(unix)]
    {
        std::fs::remove_file(&lock_path).unwrap();
        std::fs::create_dir(&lock_path).unwrap();
    }
    install.unlock();
    assert!(install.locked(), "ошибка удаления сняла блокировку");
    assert!(lock_path.exists());
    let error = ArchiveInstall::new(&root).lock().unwrap_err();
    assert_eq!(error, "Другая установка уже выполняется");

    #[cfg(windows)]
    drop(blocker);
    #[cfg(unix)]
    {
        std::fs::remove_dir(&lock_path).unwrap();
        std::fs::write(&lock_path, b"{}").unwrap();
    }
    install.discard();
    assert!(!install.locked());
    assert!(!lock_path.exists());
    let mut next = ArchiveInstall::new(&root);
    next.lock().unwrap();
    next.unlock();
    assert!(!lock_path.exists());
}

#[cfg(unix)]
mod unix {
    //! Установка версий: ссылки `current`/`previous`, права, AppImage-заглушка.

    use super::*;
    use hlsr_update::linux::archive_install;
    use std::os::unix::fs::PermissionsExt;
    use support::ustar::{Release, launcher_entries, release};

    /// Корень с релизами: архивы, описания `payload.json` и AppImage-заглушка с контрактом
    /// `--appimage-extract resources/payload` (`launcher-update-selftest.js:18-48`).
    pub struct Fixture {
        pub dir: tempfile::TempDir,
        pub release: Release,
        pub archive: PathBuf,
        pub app_image: PathBuf,
        pub payload: PathBuf,
    }

    pub fn fixture(version: &str) -> Fixture {
        fixture_with(version, launcher_entries(version))
    }

    pub fn fixture_with(version: &str, entries: Vec<serde_json::Value>) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let release = release(version, &entries);
        let payload = dir.path().join("payload");
        std::fs::create_dir_all(&payload).unwrap();
        let archive = payload.join(format!("HLSR-{version}-linux-x86_64.tar.gz"));
        std::fs::write(&archive, &release.bytes).unwrap();
        let installer = &release.manifest["installer"];
        let descriptor = embedded_payload::describe(&Describe {
            version,
            size: installer["size"].as_u64().unwrap(),
            sha256: installer["sha256"].as_str().unwrap(),
            sha512: installer["sha512"].as_str().unwrap(),
            unpacked_size: installer["unpackedSize"].as_u64().unwrap(),
        });
        std::fs::write(
            payload.join(embedded_payload::DESCRIPTOR),
            descriptor.to_string(),
        )
        .unwrap();
        let app_image = dir
            .path()
            .join(format!("HLSR-Setup-{version}-linux-x86_64.AppImage"));
        std::fs::write(
            &app_image,
            format!(
                "#!/bin/sh\n[ \"$1\" = --appimage-extract ] && [ \"$2\" = resources/payload ] || exit 2\nmkdir -p squashfs-root/resources/payload && cp '{}' '{}' squashfs-root/resources/payload/\n",
                payload.join(embedded_payload::DESCRIPTOR).display(),
                archive.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&app_image, std::fs::Permissions::from_mode(0o755)).unwrap();
        Fixture {
            dir,
            release,
            archive,
            app_image,
            payload,
        }
    }

    fn install(runtime: &tokio::runtime::Runtime, root: &Path, fixture: &Fixture) -> PathBuf {
        let mut install = ArchiveInstall::new(root);
        runtime
            .block_on(install.prepare(&fixture.archive, &fixture.release.manifest, None))
            .unwrap();
        install.commit().unwrap()
    }

    #[test]
    fn initial_installation_next_version_stable_path_and_rollback() {
        let runtime = runtime();
        let first = fixture("0.3.18");
        let second = fixture("0.3.19");
        let root = first.dir.path().join("install with spaces");
        let mut initial = ArchiveInstall::new(&root);
        runtime
            .block_on(initial.prepare(&first.archive, &first.release.manifest, None))
            .unwrap();
        assert_eq!(archive_install::read_current(&root).unwrap(), None);
        initial.commit().unwrap();
        let old = archive_install::read_current(&root).unwrap().unwrap();
        assert_eq!(
            archive_install::managed_root(&root.join("current").join("hlsr")),
            Some(root.clone())
        );
        let mut update = ArchiveInstall::new(&root);
        runtime
            .block_on(update.prepare(&second.archive, &second.release.manifest, None))
            .unwrap();
        assert_eq!(
            archive_install::read_current(&root).unwrap().as_deref(),
            Some(old.as_str())
        );
        let target = update.commit().unwrap();
        assert_ne!(
            archive_install::read_current(&root).unwrap().as_deref(),
            Some(old.as_str())
        );
        assert_eq!(
            std::fs::read_link(root.join("previous"))
                .unwrap()
                .to_string_lossy(),
            old
        );
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(
            std::fs::read_to_string(root.join("start"))
                .unwrap()
                .contains("current/hlsr")
        );
        assert_eq!(
            std::fs::read_to_string(root.join(".hlsr-install.json")).unwrap(),
            "{\"schema\":1}\n"
        );
        update.rollback().unwrap();
        assert_eq!(
            archive_install::read_current(&root).unwrap().as_deref(),
            Some(old.as_str())
        );
    }

    #[test]
    fn corrupted_archive_cannot_alter_an_installed_version() {
        let runtime = runtime();
        let f = fixture("0.3.18");
        let root = f.dir.path().join("install");
        install(&runtime, &root, &f);
        let old = archive_install::read_current(&root).unwrap();
        let mut bytes = std::fs::read(&f.archive).unwrap();
        bytes.extend_from_slice(b"bad");
        std::fs::write(&f.archive, bytes).unwrap();
        let mut again = ArchiveInstall::new(&root);
        assert!(
            runtime
                .block_on(again.prepare(&f.archive, &f.release.manifest, None))
                .is_err()
        );
        assert_eq!(archive_install::read_current(&root).unwrap(), old);
        assert!(!root.join(".update-lock").exists());
    }

    #[test]
    fn autonomous_installer_installs_the_embedded_launcher_offline_and_updates_older_installs() {
        let runtime = runtime();
        let dir = tempfile::tempdir().unwrap();
        let dirs = dirs_under(dir.path());
        let root = dir.path().join("opt").join("hlsr");
        let version = || {
            bootstrap_installer::version_of(
                archive_install::read_current(&root).unwrap().as_deref(),
            )
        };
        // Первая установка: ни канала, ни скачивания, только встроенный архив.
        let first = fixture("0.3.18");
        let fresh = open(&root, &first.payload, &dirs, &runtime);
        let initial = fresh.window.state();
        assert_eq!(
            (initial.phase, initial.version.as_deref(), initial.update),
            (Phase::Idle, Some("0.3.18"), false)
        );
        assert!(fresh.window.install(false));
        assert_eq!(fresh.last().phase, Phase::Done);
        assert_eq!(version().as_deref(), Some("0.3.18"));
        assert!(
            std::fs::metadata(root.join("start"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111
                != 0
        );
        // Команда и пункт меню (меню — без кешей: утилиты подменены).
        assert!(dirs.home.join(".local/bin/hlsr").exists());
        assert!(dirs.data_home.join("applications/hlsr.desktop").exists());
        // Установщик новее установленной версии предлагает обновление.
        let second = fixture("0.3.19");
        let upgrade = open(&root, &second.payload, &dirs, &runtime);
        let offered = upgrade.window.state();
        assert_eq!(
            (
                offered.phase,
                offered.update,
                offered.installed_version.as_deref()
            ),
            (Phase::Idle, true, Some("0.3.18"))
        );
        assert!(upgrade.window.install(true));
        assert_eq!(version().as_deref(), Some("0.3.19"));
        // Копия ярлыка — на рабочий стол XDG, исполняемая.
        let copy = dirs.desktop.join("hlsr.desktop");
        assert_eq!(
            std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777,
            0o755
        );
        // Та же версия уже стоит: установщик сразу предлагает открыть HLSR; запуск — текущая версия.
        let same = open(&root, &second.payload, &dirs, &runtime);
        assert_eq!(same.window.state().phase, Phase::Done);
        assert!(same.window.launch());
        let calls = same.launcher.calls();
        let command = calls[0].command();
        assert_eq!(command.args, ["--no-sandbox"]);
        assert!(command.detached);
        assert!(
            command.file.ends_with("hlsr")
                && command.file.to_string_lossy().contains("/versions/0.3.19-")
        );
        assert_eq!(same.quit.0.load(Ordering::SeqCst), 1);
        // Испорченный встроенный архив не ставится, установленная версия цела.
        let third = fixture("0.3.20");
        let mut bytes = std::fs::read(&third.archive).unwrap();
        bytes.extend_from_slice(b"tampered");
        std::fs::write(&third.archive, bytes).unwrap();
        let broken = open(&root, &third.payload, &dirs, &runtime);
        assert!(!broken.window.install(false));
        assert_eq!(broken.last().phase, Phase::Error);
        assert_eq!(version().as_deref(), Some("0.3.19"));
        // Без описания установщик сразу сообщает, что он повреждён.
        std::fs::remove_file(third.payload.join(embedded_payload::DESCRIPTOR)).unwrap();
        let missing = open(&root, &third.payload, &dirs, &runtime).window.state();
        assert_eq!(missing.phase, Phase::Error);
        assert!(
            missing
                .error
                .unwrap()
                .contains("Скачайте установщик заново")
        );
    }

    #[test]
    fn orphan_staging_is_cleaned_by_the_next_prepare() {
        let runtime = runtime();
        let f = fixture("0.3.20");
        let root = f.dir.path().join("asar-install");
        install(&runtime, &root, &f);
        let orphan = root.join("versions").join(".staging-abcdef");
        std::fs::create_dir_all(&orphan).unwrap();
        std::fs::write(orphan.join("resources-app.asar-partial"), "garbage").unwrap();
        let second = fixture("0.3.21");
        let mut retry = ArchiveInstall::new(&root);
        runtime
            .block_on(retry.prepare(&second.archive, &second.release.manifest, None))
            .unwrap();
        assert!(!orphan.exists());
        retry.discard();
    }
}

#[cfg(target_os = "linux")]
mod linux {
    //! Служба обновления на настоящей установке, загрузка с 127.0.0.1 и чанки (самотесты 0.4.7).

    use super::unix::{Fixture, fixture, fixture_with};
    use super::*;
    use hlsr_update::boot_gate::UpdateState;
    use hlsr_update::cancel::CancelToken;
    use hlsr_update::chunks::{self, ChunkError, ChunkFetch, DescribeOptions, FetchOptions};
    use hlsr_update::gate::Outcome;
    use hlsr_update::linux::SystemRunner;
    use hlsr_update::linux::archive_download::{ArchiveDownload, DownloadOptions};
    use hlsr_update::linux::archive_install;
    use hlsr_update::linux::update_manager::{
        AppImageDownloader, AppImageExtractor, ArchiveInstaller, HttpChunkManifests, LinuxGate,
        LinuxManagerConfig, LinuxManagerPorts, LinuxUpdateManager, ProgressSink, Relauncher,
        VersionInstaller,
    };
    use hlsr_update::ports::{BoxFuture, MemoryLogger, SystemClock};
    use serde_json::Value;
    use std::collections::{BTreeMap, HashMap};
    use std::os::unix::fs::PermissionsExt;

    struct FixedGate(Value);

    impl LinuxGate for FixedGate {
        fn evaluate(&self) -> BoxFuture<'static, Result<Outcome, String>> {
            let manifest = self.0.clone();
            Box::pin(async move {
                Ok(Outcome {
                    ok: true,
                    problems: Vec::new(),
                    update_available: true,
                    downgrade: false,
                    unsupported: false,
                    code: None,
                    update_required: false,
                    manifest: Some(manifest),
                    source_url: String::new(),
                })
            })
        }
    }

    /// Загрузка «копирует AppImage» (`fetchInstaller` самотеста) или отказывает.
    struct CopyAppImage {
        source: Option<PathBuf>,
        calls: Arc<AtomicUsize>,
    }

    impl AppImageDownloader for CopyAppImage {
        fn download(
            &self,
            _manifest: Value,
            directory: PathBuf,
            _cancel: CancelToken,
            _on_progress: ProgressSink,
        ) -> BoxFuture<'static, Result<PathBuf, String>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let source = self.source.clone();
            Box::pin(async move {
                let source = source.ok_or("полный установщик не нужен")?;
                let target = directory.join(source.file_name().unwrap());
                std::fs::copy(&source, &target).map_err(|error| error.to_string())?;
                Ok(target)
            })
        }
    }

    #[derive(Default)]
    struct Relaunches(Mutex<Vec<(PathBuf, Vec<String>)>>);

    impl Relauncher for Relaunches {
        fn relaunch(&self, exec_path: &Path, args: &[String]) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .push((exec_path.to_owned(), args.to_vec()));
            Ok(())
        }
    }

    /// Хранилище hlsr.pro: манифест по имени, куски по хешу (`server` самотеста).
    struct Store {
        dir: PathBuf,
        manifests: HashMap<String, Vec<u8>>,
        requested: Mutex<Vec<String>>,
    }

    impl ChunkFetch for Store {
        fn fetch<'a>(
            &'a self,
            url: &'a str,
            options: &'a FetchOptions,
        ) -> BoxFuture<'a, Result<Vec<u8>, ChunkError>> {
            self.requested.lock().unwrap().push(url.to_owned());
            let name = url.rsplit('/').next().unwrap_or_default().to_owned();
            let body = match self.manifests.get(&name) {
                Some(body) => Ok(body.clone()),
                None => std::fs::read(self.dir.join(&name[..2.min(name.len())]).join(&name))
                    .map_err(|_| ChunkError::Status {
                        status: 404,
                        url: url.to_owned(),
                    }),
            };
            let max = options.max_bytes;
            Box::pin(async move {
                let body = body?;
                if body.len() as u64 > max {
                    return Err(ChunkError::TooLarge {
                        max,
                        url: String::new(),
                    });
                }
                Ok(body)
            })
        }
    }

    fn manager(
        root: &Path,
        manifest: Value,
        download: Arc<dyn AppImageDownloader>,
        fetch: Arc<dyn ChunkFetch>,
        relaunches: Arc<Relaunches>,
        quit: Arc<Quit>,
    ) -> LinuxUpdateManager {
        let logger = Arc::new(MemoryLogger::default());
        let installer_root = root.to_owned();
        let installer_fetch = fetch.clone();
        let installer_log = logger.clone();
        LinuxUpdateManager::new(
            LinuxManagerConfig {
                current_version: "0.3.17".to_owned(),
                packaged: true,
                install_root: Some(root.to_owned()),
                argv: vec![
                    "/opt/hlsr/current/hlsr".to_owned(),
                    "--ozone-platform=x11".to_owned(),
                ],
            },
            LinuxManagerPorts {
                gate: Arc::new(FixedGate(manifest)),
                download,
                extract: Arc::new(AppImageExtractor {
                    runner: Arc::new(SystemRunner),
                    env: vec![("PATH".to_owned(), std::env::var("PATH").unwrap_or_default())],
                }),
                chunk_manifests: Arc::new(HttpChunkManifests {
                    fetch,
                    is_allowed: chunks::production_allow(),
                }),
                create_installer: Arc::new(move || -> Arc<dyn VersionInstaller> {
                    Arc::new(ArchiveInstaller::new(
                        &installer_root,
                        installer_fetch.clone(),
                        chunks::production_allow(),
                        installer_log.clone(),
                        Arc::new(SystemClock),
                    ))
                }),
                app: quit,
                relauncher: relaunches,
                logger,
                clock: Arc::new(SystemClock),
                random: Arc::new(|| 0.5),
                window: None,
                telemetry: None,
                version_events: None,
            },
        )
    }

    async fn settle(manager: &LinuxUpdateManager, states: &[UpdateState]) {
        for _ in 0..2000 {
            if states.contains(&manager.state()) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("состояние {:?}, ждали {states:?}", manager.state());
    }

    fn channel_manifest(f: &Fixture) -> Value {
        let bytes = std::fs::read(&f.app_image).unwrap();
        json!({
            "schemaVersion": 1, "version": f.release.version, "publishedAt": "2026-10-16T12:00:00.000Z",
            "installer": {
                "filename": f.app_image.file_name().unwrap().to_string_lossy(),
                "size": bytes.len(), "sha256": support::sha256_hex(&bytes), "sha512": "x",
                "urls": [format!("https://hlsr.pro/downloads/launcher/linux/stable/{}", f.app_image.file_name().unwrap().to_string_lossy())],
            },
        })
    }

    fn no_chunks() -> Arc<dyn ChunkFetch> {
        Arc::new(Store {
            dir: PathBuf::from("/nonexistent"),
            manifests: HashMap::new(),
            requested: Mutex::default(),
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn manager_blocks_installation_during_a_game_and_relaunches_the_installed_binary() {
        let f = fixture("0.3.18");
        let root = f.dir.path().join("installation");
        let relaunches = Arc::new(Relaunches::default());
        let quit = Arc::new(Quit::default());
        let download = Arc::new(CopyAppImage {
            source: Some(f.app_image.clone()),
            calls: Arc::default(),
        });
        let manager = manager(
            &root,
            channel_manifest(&f),
            download,
            no_chunks(),
            relaunches.clone(),
            quit.clone(),
        );
        assert!(manager.check());
        settle(&manager, &[UpdateState::Available]).await;
        assert!(manager.download());
        settle(&manager, &[UpdateState::Downloaded, UpdateState::Error]).await;
        assert_eq!(
            manager.state(),
            UpdateState::Downloaded,
            "{}",
            manager.get_state()
        );
        manager.set_game_running_check(|| true);
        assert!(!manager.install_now());
        assert_eq!(archive_install::read_current(&root).unwrap(), None);
        manager.set_game_running_check(|| false);
        assert!(manager.install_now());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(quit.0.load(Ordering::SeqCst), 1);
        let (exec, args) = relaunches.0.lock().unwrap()[0].clone();
        assert_eq!(
            archive_install::managed_root(&exec),
            Some(std::fs::canonicalize(&root).unwrap())
        );
        assert_eq!(args, ["--no-sandbox", "--ozone-platform=x11"]);
        // .downloads после загрузки пуст.
        assert_eq!(
            std::fs::read_dir(root.join(".downloads")).unwrap().count(),
            0
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_http_installer_download_falls_back_verifies_bytes_and_feeds_installation() {
        let f = fixture("0.3.18");
        let log = support::http::RequestLog::default();
        let server = support::http::start_server("a", log.clone()).await;
        let body: String = std::fs::read(&f.app_image)
            .unwrap()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        server.routes.lock().unwrap().extend(
            json!({"/broken": [{"status": 503}], "/installer": [{"bodyHex": body}]})
                .as_object()
                .unwrap()
                .clone(),
        );
        let base = format!("http://{}", server.authority);
        let mut manifest = channel_manifest(&f);
        use base64::Engine as _;
        use sha2::Digest as _;
        manifest["installer"]["sha512"] = json!(
            base64::engine::general_purpose::STANDARD
                .encode(sha2::Sha512::digest(std::fs::read(&f.app_image).unwrap()))
        );
        manifest["installer"]["urls"] =
            json!([format!("{base}/broken"), format!("{base}/installer")]);
        let downloads = f.dir.path().join("downloads");
        let prefix = format!("{base}/");
        let options = DownloadOptions {
            is_allowed: Arc::new(move |url: &str| url.starts_with(&prefix)),
            ..DownloadOptions::default()
        };
        let target = ArchiveDownload::new()
            .download(&manifest, &downloads, &options)
            .await
            .unwrap();
        assert_eq!(log.lock().unwrap().len(), 2);
        assert_eq!(
            std::fs::read(&target).unwrap(),
            std::fs::read(&f.app_image).unwrap()
        );
        let payload =
            embedded_payload::extract_from(&target, &downloads, &[], &SystemRunner, None).unwrap();
        assert_eq!(payload.manifest, f.release.manifest);
        let root = f.dir.path().join("install");
        let mut install = ArchiveInstall::new(&root);
        install
            .prepare(&payload.archive, &payload.manifest, None)
            .await
            .unwrap();
        install.commit().unwrap();
        assert!(archive_install::read_current(&root).unwrap().is_some());
    }

    #[test]
    fn extraction_strips_appimage_environment() {
        let runner = FakeRunner::new();
        let dir = tempfile::tempdir().unwrap();
        let app_image = dir.path().join("x.AppImage");
        std::fs::write(&app_image, "x").unwrap();
        let env = vec![
            ("APPIMAGE".to_owned(), "/a".to_owned()),
            ("APPDIR".to_owned(), "/m".to_owned()),
            ("ARGV0".to_owned(), "x".to_owned()),
            ("OWD".to_owned(), "/".to_owned()),
            ("APPIMAGE_EXTRACT_AND_RUN".to_owned(), "1".to_owned()),
            ("HOME".to_owned(), "/home/u".to_owned()),
        ];
        let _ = embedded_payload::extract_from(&app_image, dir.path(), &env, &runner, None);
        let (command, timeout) = runner.calls()[0].clone();
        assert_eq!(command.args, ["--appimage-extract", "resources/payload"]);
        assert_eq!(command.cwd.as_deref(), Some(dir.path()));
        assert_eq!(command.env, [("HOME".to_owned(), "/home/u".to_owned())]);
        assert_eq!(timeout, std::time::Duration::from_secs(600));
        assert_eq!(
            std::fs::metadata(&app_image).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn lock_records_this_process_identity() {
        let runtime = runtime();
        let f = fixture("0.3.18");
        let root = f.dir.path().join("install");
        let mut install = ArchiveInstall::new(&root);
        runtime
            .block_on(install.prepare(&f.archive, &f.release.manifest, None))
            .unwrap();
        let owner: Value =
            serde_json::from_str(&std::fs::read_to_string(root.join(".update-lock")).unwrap())
                .unwrap();
        assert_eq!(owner["pid"], json!(std::process::id()));
        let identity = owner["identity"].as_str().unwrap();
        let (boot, start) = identity.rsplit_once(':').unwrap();
        assert_eq!(boot.len(), 36);
        assert!(start.bytes().all(|b| b.is_ascii_digit()));
        assert_eq!(
            std::fs::metadata(root.join(".update-lock"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            archive_install::process_identity(std::process::id()).as_deref(),
            Some(identity)
        );
        install.discard();
    }

    // ------------------------------------------------------------ чанки (launcher-chunks-selftest.js, тест 4)

    struct Parts {
        elf: Vec<u8>,
        asar: Vec<u8>,
        lib: Vec<u8>,
    }

    fn noise(seed: u32, size: usize) -> Vec<u8> {
        let mut state = seed.max(1);
        (0..size)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state >> 24) as u8
            })
            .collect()
    }

    fn base(seed: u32) -> Parts {
        let mut elf = vec![0u8; 64];
        elf.extend(noise(seed, 600 * 1024));
        elf[..7].copy_from_slice(&[0x7f, 0x45, 0x4c, 0x46, 2, 1, 1]);
        elf[18..20].copy_from_slice(&62u16.to_le_bytes());
        Parts {
            elf,
            asar: noise(seed + 1, 1536 * 1024),
            lib: noise(seed + 2, 400 * 1024),
        }
    }

    fn entries(version: &str, parts: &Parts, next: bool) -> Vec<Value> {
        let hex = |bytes: &[u8]| -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() };
        let release = format!(
            "{}\n",
            json!({"version": version, "platform": "linux", "arch": "x64", "executable": "hlsr"})
        );
        let mut asar = parts.asar.clone();
        let mut list = vec![
            json!({"name": "./", "type": "5"}),
            json!({"name": "./.hlsr-release.json", "text": release}),
            json!({"name": "./hlsr", "mode": 0o755, "hex": hex(&parts.elf)}),
            json!({"name": "./libffmpeg.so", "mode": 0o755, "hex": hex(&parts.lib)}),
            json!({"name": "./resources/", "type": "5"}),
            json!({"name": "./resources/icons/", "type": "5"}),
            json!({"name": "./resources/icons/hlsr.png", "text": "icon"}),
        ];
        if next {
            asar[700 * 1024..700 * 1024 + 300].copy_from_slice(&noise(99, 300));
            asar.extend_from_slice("хвост новой версии".as_bytes());
            list.push(json!({"name": "./resources/new.txt", "text": "только в новой версии"}));
        } else {
            list.push(json!({"name": "./resources/old.txt", "text": "только в старой версии"}));
        }
        list.push(json!({"name": "./resources/app.asar", "hex": hex(&asar)}));
        list
    }

    fn files(root: &Path) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        fn walk(root: &Path, relative: &str, out: &mut BTreeMap<String, String>) {
            for entry in std::fs::read_dir(root.join(relative)).unwrap() {
                let entry = entry.unwrap();
                let name = entry.file_name().to_string_lossy().into_owned();
                let child = if relative.is_empty() {
                    name
                } else {
                    format!("{relative}/{name}")
                };
                if entry.file_type().unwrap().is_dir() {
                    walk(root, &child, out);
                } else {
                    out.insert(
                        child,
                        support::sha256_hex(&std::fs::read(entry.path()).unwrap()),
                    );
                }
            }
        }
        walk(root, "", &mut out);
        out
    }

    /// Дерево версии на диске (то, что распакует архив) и хранилище кусков с манифестом.
    fn chunked(dir: &Path, f: &Fixture, store: &Path) -> (Value, Vec<u8>) {
        let tree = dir.join(format!("tree-{}", f.release.version));
        let mut install = ArchiveInstall::new(&dir.join(format!("unpack-{}", f.release.version)));
        tokio::runtime::Handle::current().block_on(async {
            install
                .prepare(&f.archive, &f.release.manifest, None)
                .await
                .unwrap();
        });
        std::fs::rename(install.stage().unwrap(), &tree).unwrap();
        install.discard();
        let built = chunks::build_store(
            &tree,
            store,
            &DescribeOptions::new(&f.release.version, hlsr_core::js::Platform::Linux),
        )
        .unwrap();
        let text = format!("{}\n", built.manifest.to_value());
        let filename = format!("HLSR-{}-linux.chunks.json", f.release.version);
        let mut manifest = channel_manifest(f);
        manifest["delta"] = json!({
            "schema": 1,
            "manifest": {"filename": filename, "size": text.len(), "sha256": support::sha256_hex(text.as_bytes()),
                "urls": [format!("https://hlsr.pro/downloads/launcher/linux/stable/{filename}")]},
            "chunkBaseUrls": ["https://hlsr.pro/downloads/launcher/chunks"],
        });
        (manifest, text.into_bytes())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn chunked_update_falls_back_to_the_full_installer_and_prunes_old_versions() {
        let dir = tempfile::tempdir().unwrap();
        let parts = base(7);
        let first = fixture_with("0.4.3", entries("0.4.3", &parts, false));
        let next = fixture_with("0.4.4", entries("0.4.4", &parts, true));
        let root = dir.path().join("install");
        for name in ["0.4.1-aaaaaaaaaaaaaaaa", "0.4.2-bbbbbbbbbbbbbbbb"] {
            std::fs::create_dir_all(root.join("versions").join(name)).unwrap();
            std::fs::write(root.join("versions").join(name).join("hlsr"), "old").unwrap();
        }
        let mut initial = ArchiveInstall::new(&root);
        initial
            .prepare(&first.archive, &first.release.manifest, None)
            .await
            .unwrap();
        initial.commit().unwrap();
        let installed = archive_install::read_current(&root).unwrap().unwrap();

        let store = dir.path().join("store");
        let (manifest, manifest_text) =
            tokio::task::block_in_place(|| chunked(dir.path(), &next, &store));
        let fetch = Arc::new(Store {
            dir: store.clone(),
            manifests: HashMap::from([("HLSR-0.4.4-linux.chunks.json".to_owned(), manifest_text)]),
            requested: Mutex::default(),
        });
        let full = Arc::new(CopyAppImage {
            source: None,
            calls: Arc::default(),
        });
        let full_calls = full.calls.clone();
        let quit = Arc::new(Quit::default());
        let update = manager(
            &root,
            manifest,
            full,
            fetch.clone(),
            Arc::default(),
            quit.clone(),
        );
        assert!(update.check());
        settle(&update, &[UpdateState::Available]).await;
        assert!(update.download());
        settle(&update, &[UpdateState::Downloaded, UpdateState::Error]).await;
        assert_eq!(
            update.state(),
            UpdateState::Downloaded,
            "{}",
            update.get_state()
        );
        assert_eq!(full_calls.load(Ordering::SeqCst), 0);
        assert!(
            fetch.requested.lock().unwrap().len() <= 7,
            "{:?}",
            fetch.requested.lock().unwrap()
        );
        assert!(update.install_now());
        let current = archive_install::read_current(&root).unwrap().unwrap();
        assert_eq!(
            files(&root.join(&current)),
            files(&dir.path().join("tree-0.4.4"))
        );
        assert_eq!(
            std::fs::read_link(root.join("previous"))
                .unwrap()
                .to_string_lossy(),
            installed
        );
        let mut versions: Vec<String> =
            hlsr_core::js::fs::read_dir_names(&root.join("versions")).unwrap();
        versions.sort();
        let mut expected = vec![
            installed.trim_start_matches("versions/").to_owned(),
            current.trim_start_matches("versions/").to_owned(),
        ];
        expected.sort();
        assert_eq!(versions, expected);

        // Сломанное хранилище: клиент качает установщик целиком и всё равно ставит.
        let third_parts = Parts {
            lib: noise(77, 400 * 1024),
            ..base(7)
        };
        let third = fixture_with("0.4.5", entries("0.4.5", &third_parts, true));
        let (manifest, manifest_text) = tokio::task::block_in_place(|| {
            chunked(dir.path(), &third, &dir.path().join("store-3"))
        });
        std::fs::remove_dir_all(&store).unwrap();
        let fetch = Arc::new(Store {
            dir: store.clone(),
            manifests: HashMap::from([("HLSR-0.4.5-linux.chunks.json".to_owned(), manifest_text)]),
            requested: Mutex::default(),
        });
        let full = Arc::new(CopyAppImage {
            source: Some(third.app_image.clone()),
            calls: Arc::default(),
        });
        let full_calls = full.calls.clone();
        let mut manifest = manifest;
        manifest["installer"] = channel_manifest(&third)["installer"].clone();
        let fallback = manager(&root, manifest, full, fetch, Arc::default(), quit);
        assert!(fallback.check());
        settle(&fallback, &[UpdateState::Available]).await;
        assert!(fallback.download());
        settle(&fallback, &[UpdateState::Downloaded, UpdateState::Error]).await;
        assert_eq!(
            fallback.state(),
            UpdateState::Downloaded,
            "{}",
            fallback.get_state()
        );
        assert_eq!(full_calls.load(Ordering::SeqCst), 1);
        assert!(fallback.install_now());
        let current = archive_install::read_current(&root).unwrap().unwrap();
        assert_eq!(
            files(&root.join(&current)),
            files(&dir.path().join("tree-0.4.5"))
        );
        assert_eq!(std::fs::read_dir(root.join("versions")).unwrap().count(), 2);
    }
}
