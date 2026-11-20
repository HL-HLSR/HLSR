//! Единственный экземпляр: блокировка, передача аргументов первому экземпляру
//! и признаки запущенного HLSR 0.4.x (`requestSingleInstanceLock` Electron).
//!
//! Второй экземпляр — отдельный процесс: сам тестовый бинарник с аргументами
//! `--exact probe_entry --nocapture` и переменной `HLSR_INSTANCE_PROBE`.

use hlsr_core::instance::{self, Acquired};
use hlsr_core::paths::Paths;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const PROBE: &str = "HLSR_INSTANCE_PROBE";
const ROOT: &str = "HLSR_INSTANCE_ROOT";
const ARGV: &str = "HLSR_INSTANCE_ARGV";

/// Строка для родительского теста; остальной вывод тестового бинарника отбрасывается.
fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "probe: {line}").unwrap();
    out.flush().unwrap();
}

#[test]
fn probe_entry() {
    let Ok(mode) = std::env::var(PROBE) else {
        return;
    };
    let paths = Paths::under(Path::new(&std::env::var_os(ROOT).expect(ROOT)));
    let argv: Vec<String> = std::env::var(ARGV)
        .map(|json| serde_json::from_str(&json).unwrap())
        .unwrap_or_default();
    let started = Instant::now();
    let acquired = instance::acquire(&paths, argv).unwrap();
    match (mode.as_str(), acquired) {
        ("secondary", Acquired::Forwarded) => say("forwarded"),
        // Сколько заняла передача: время самой пробы, без запуска процесса под нагрузкой.
        ("timed", Acquired::Forwarded) => {
            say(&format!("forwarded {}", started.elapsed().as_millis()))
        }
        ("primary", Acquired::Primary(mut primary)) => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap();
            say(&format!("primary {}", primary.endpoint().unwrap()));
            let mut rx = {
                let _enter = rt.enter();
                primary.listen()
            };
            let got = rt
                .block_on(async { tokio::time::timeout(Duration::from_secs(20), rx.recv()).await });
            say(&format!(
                "argv {}",
                serde_json::to_string(&got.ok().flatten()).unwrap()
            ));
        }
        ("hung", Acquired::Primary(_primary)) => {
            say("primary");
            std::thread::sleep(Duration::from_secs(10));
        }
        (_, Acquired::Primary(_)) => say("primary"),
        (_, Acquired::Forwarded) => say("forwarded"),
    }
}

/// Дочерний процесс-проба и строки, которые он сказал через `say`.
struct Probe {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl Probe {
    fn spawn(mode: &str, root: &Path, argv: &[&str]) -> Probe {
        Self::spawn_with(mode, root, argv, |_| {})
    }

    fn spawn_with(
        mode: &str,
        root: &Path,
        argv: &[&str],
        setup: impl FnOnce(&mut Command),
    ) -> Probe {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "probe_entry", "--nocapture"])
            .env(PROBE, mode)
            .env(ROOT, root)
            .env(ARGV, serde_json::to_string(argv).unwrap())
            // Сокеты и блокировки проб — внутри временной папки теста.
            .env("XDG_RUNTIME_DIR", root.join("run"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        setup(&mut command);
        let mut child = command.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(said) = line.strip_prefix("probe: ") {
                    let _ = tx.send(said.to_owned());
                }
            }
        });
        Probe { child, lines }
    }

    fn next(&self, timeout: Duration) -> String {
        self.lines
            .recv_timeout(timeout)
            .expect("проба ничего не сказала вовремя")
    }

    fn finish(mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "проба не завершилась");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn second_process_forwards_args_to_first() {
    let root = tempfile::tempdir().unwrap();
    let primary = Probe::spawn("primary", root.path(), &[]);
    assert!(
        primary
            .next(Duration::from_secs(20))
            .starts_with("primary ")
    );

    let secondary = Probe::spawn("secondary", root.path(), &["HLSR", "--select-game=hl2"]);
    assert_eq!(secondary.next(Duration::from_secs(20)), "forwarded");
    assert!(secondary.finish().success());

    assert_eq!(
        primary.next(Duration::from_secs(20)),
        r#"argv ["HLSR","--select-game=hl2"]"#
    );
    assert!(primary.finish().success());
}

#[test]
fn hung_primary_times_out_in_three_seconds() {
    let root = tempfile::tempdir().unwrap();
    let primary = Probe::spawn("hung", root.path(), &[]);
    assert_eq!(primary.next(Duration::from_secs(20)), "primary");

    let secondary = Probe::spawn("timed", root.path(), &["--select-game=hl2"]);
    let said = secondary.next(Duration::from_secs(20));
    assert!(secondary.finish().success());
    let millis: u64 = said
        .strip_prefix("forwarded ")
        .unwrap_or_else(|| panic!("проба сказала «{said}»"))
        .parse()
        .unwrap();
    // 3 с ожидания ответа; зависший первый экземпляр держит сокет 10 с, поэтому передача,
    // которая его дожидалась бы, шла бы дольше предела.
    assert!((2900..9000).contains(&millis), "передача шла {millis} мс");
}

#[test]
fn killed_primary_does_not_block_next_start() {
    let root = tempfile::tempdir().unwrap();
    let mut primary = Probe::spawn("hung", root.path(), &[]);
    assert_eq!(primary.next(Duration::from_secs(20)), "primary");
    primary.child.kill().unwrap();
    primary.child.wait().unwrap();

    // Брошенный сокет упавшего процесса не мешает: блокировку снимает система.
    let next = Probe::spawn("primary", root.path(), &[]);
    assert!(next.next(Duration::from_secs(20)).starts_with("primary "));
    let secondary = Probe::spawn("secondary", root.path(), &["--x"]);
    assert_eq!(secondary.next(Duration::from_secs(20)), "forwarded");
    assert_eq!(next.next(Duration::from_secs(20)), r#"argv ["--x"]"#);
}

#[test]
fn dropping_primary_releases_lock() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::under(root.path());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let _enter = rt.enter();

    let Acquired::Primary(mut first) = instance::acquire(&paths, vec![]).unwrap() else {
        panic!("первый запуск должен стать первым экземпляром");
    };
    let _rx = first.listen();
    // Второй захват в том же процессе внутри рантайма: передача не падает с паникой.
    assert!(matches!(
        instance::acquire(&paths, vec!["--same".into()]).unwrap(),
        Acquired::Forwarded
    ));
    // Перезапуск на программный рендерер: блокировку отпускают до запуска нового процесса.
    drop(first);
    let Acquired::Primary(mut second) = instance::acquire(&paths, vec![]).unwrap() else {
        panic!("после drop блокировка должна освободиться");
    };
    assert!(second.endpoint().is_some(), "канал после перезапуска");
    let mut rx = second.listen();

    // Блокировку держит сам процесс теста: проба берёт то же XDG_RUNTIME_DIR.
    let secondary = Probe::spawn_with("secondary", root.path(), &["--again"], |command| {
        match std::env::var_os("XDG_RUNTIME_DIR") {
            Some(dir) => command.env("XDG_RUNTIME_DIR", dir),
            None => command.env_remove("XDG_RUNTIME_DIR"),
        };
    });
    assert_eq!(secondary.next(Duration::from_secs(20)), "forwarded");
    let got = rt.block_on(tokio::time::timeout(Duration::from_secs(10), rx.recv()));
    assert_eq!(got.unwrap().unwrap(), vec!["--again".to_owned()]);
}

#[test]
fn malformed_request_is_ignored() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::under(root.path());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let _enter = rt.enter();
    let Acquired::Primary(mut primary) = instance::acquire(&paths, vec![]).unwrap() else {
        panic!("первый экземпляр");
    };
    let mut rx = primary.listen();
    let endpoint = primary.endpoint().unwrap();

    rt.block_on(async {
        for bad in ["not json\n", "{\"argv\":[1]}\n", "{\"args\":[]}\n"] {
            let reply = instance::send_raw(&endpoint, bad.as_bytes(), Duration::from_secs(3))
                .await
                .unwrap();
            assert_eq!(reply, "", "ответ на {bad:?}");
        }
        let reply = instance::send_raw(
            &endpoint,
            b"{\"argv\":[\"--ok\"]}\n",
            Duration::from_secs(3),
        )
        .await
        .unwrap();
        assert_eq!(reply, "ok\n");
    });
    let got = rt.block_on(tokio::time::timeout(Duration::from_secs(5), rx.recv()));
    assert_eq!(got.unwrap().unwrap(), vec!["--ok".to_owned()]);
}

#[cfg(target_os = "linux")]
#[test]
fn long_data_dir_uses_short_socket_path() {
    let root = tempfile::tempdir().unwrap();
    let long = root.path().join("д".repeat(60));
    let primary = Probe::spawn_with("primary", &long, &[], |command| {
        command.env_remove("XDG_RUNTIME_DIR");
    });
    let said = primary.next(Duration::from_secs(20));
    let socket = std::path::PathBuf::from(said.strip_prefix("primary ").unwrap());
    let uid = std::fs::metadata("/proc/self").map(|m| {
        use std::os::unix::fs::MetadataExt;
        m.uid()
    });
    assert!(
        socket.starts_with(format!("/tmp/hlsr-{}", uid.unwrap())),
        "{socket:?}"
    );
    assert!(socket.as_os_str().len() <= 107, "{socket:?}");

    let secondary = Probe::spawn_with("secondary", &long, &["--long"], |command| {
        command.env_remove("XDG_RUNTIME_DIR");
    });
    assert_eq!(secondary.next(Duration::from_secs(20)), "forwarded");
    assert_eq!(primary.next(Duration::from_secs(20)), r#"argv ["--long"]"#);
    assert!(primary.finish().success());
    assert!(!socket.exists(), "сокет удаляется при выходе");
}

#[cfg(target_os = "linux")]
#[test]
fn socket_lives_in_runtime_dir() {
    let root = tempfile::tempdir().unwrap();
    let primary = Probe::spawn("primary", root.path(), &[]);
    let said = primary.next(Duration::from_secs(20));
    let socket = std::path::PathBuf::from(said.strip_prefix("primary ").unwrap());
    assert_eq!(
        socket.parent().unwrap(),
        root.path().join("run").join("hlsr")
    );
    let name = socket.file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with("instance-") && name.ends_with(".sock"),
        "{name}"
    );
    assert_eq!(name.len(), "instance-".len() + 16 + ".sock".len(), "{name}");
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(socket.parent().unwrap())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[cfg(target_os = "linux")]
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .unwrap()
        .trim()
        .to_owned()
}

#[cfg(target_os = "linux")]
#[test]
fn stale_or_foreign_singleton_lock_is_not_running() {
    let dir = tempfile::tempdir().unwrap();
    let lock = dir.path().join("SingletonLock");
    assert!(!instance::electron_running(dir.path()));

    std::os::unix::fs::symlink("other-host-1", &lock).unwrap();
    assert!(!instance::electron_running(dir.path()));
    assert!(!instance::electron_running_with(dir.path(), |_| true));
    std::fs::remove_file(&lock).unwrap();

    let host = hostname();
    std::os::unix::fs::symlink(format!("{host}-999999999"), &lock).unwrap();
    assert!(!instance::electron_running(dir.path()));
    assert!(instance::electron_running_with(dir.path(), |pid| pid == 999_999_999));
    std::fs::remove_file(&lock).unwrap();

    // Свой процесс жив, но это не HLSR и не Electron.
    std::os::unix::fs::symlink(format!("{host}-{}", std::process::id()), &lock).unwrap();
    assert!(!instance::electron_running(dir.path()));
    std::fs::remove_file(&lock).unwrap();

    std::os::unix::fs::symlink(format!("{host}-abc"), &lock).unwrap();
    assert!(!instance::electron_running_with(dir.path(), |_| true));
}

#[cfg(target_os = "linux")]
#[test]
fn wait_returns_once_lock_disappears() {
    let dir = tempfile::tempdir().unwrap();
    let lock = dir.path().join("SingletonLock");
    std::os::unix::fs::symlink(format!("{}-12345", hostname()), &lock).unwrap();
    let running = |data: &Path| instance::electron_running_with(data, |_| true);

    let started = Instant::now();
    let poll = Duration::from_millis(50);
    assert!(!instance::wait_for_electron_exit_with(
        dir.path(),
        Duration::from_millis(300),
        poll,
        running
    ));
    assert!(started.elapsed() >= Duration::from_millis(300));

    let remover = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        std::fs::remove_file(lock).unwrap();
    });
    let started = Instant::now();
    assert!(instance::wait_for_electron_exit_with(
        dir.path(),
        Duration::from_secs(5),
        poll,
        running
    ));
    // Срок ожидания — 5 с: возврат раньше него и есть «ушёл вместе с замком».
    assert!(started.elapsed() < Duration::from_millis(4500));
    remover.join().unwrap();
}

#[test]
fn not_running_without_lock_file() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!instance::electron_running(dir.path()));
    assert!(instance::wait_for_electron_exit(
        dir.path(),
        Duration::from_secs(10),
        Duration::from_millis(250)
    ));
}

#[cfg(windows)]
#[test]
fn open_lockfile_means_running() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lockfile");
    // Как ProcessSingleton Chromium: GENERIC_WRITE, FILE_SHARE_READ.
    let held = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .share_mode(1)
        .open(&path)
        .unwrap();
    assert!(instance::electron_running(dir.path()));
    drop(held);
    // Брошенный файл без открытого дескриптора — не запущен.
    assert!(!instance::electron_running(dir.path()));
}
