//! Апдейтер и служба обновления на настоящей ОС и настоящих реализациях портов: снимок процессов
//! Toolhelp32 над процессом, который тест запустил сам (копия `cmd.exe` под уникальным именем
//! образа, без окна), `ProductVersion` через `GetFileVersionInfoW`, запуск с кодом выхода, место на
//! диске; лента `latest.yml` и загрузка установщика против локального сервера 127.0.0.1; окружение
//! дочерних процессов. Настоящие `HLSR.exe`, установщик, реестр и `%TEMP%\HLSR-Updater-*` не
//! трогаются.

mod support;

use hlsr_update::update_manager::{
    self, FeedFile, HttpInstallerDownload, InstallerRequest, ProcessSetupRunner, SetupRunner,
    UpdateInfo,
};
use hlsr_update::updater::processes::{self, FakeLauncher, LaunchCall};
use serde_json::json;
use std::sync::Arc;

#[test]
fn child_environment_drops_window_and_unpacker_variables() {
    let env = processes::child_env([
        ("Path", "C:\\Windows"),
        ("PORTABLE_EXECUTABLE_FILE", "C:\\T\\HLSR-Setup.exe"),
        ("PORTABLE_EXECUTABLE_DIR", "C:\\T"),
        ("SLINT_SCALE_FACTOR", "1.25"),
        ("WINIT_X11_SCALE_FACTOR", "1.25"),
        ("APPIMAGE", "/x.AppImage"),
        ("APPDIR", "/tmp/.mount"),
        ("ARGV0", "x"),
        ("OWD", "/home"),
        ("APPIMAGE_EXTRACT_AND_RUN", "1"),
        ("portable_executable_file", "lower"),
        ("LOCALAPPDATA", "C:\\Users\\u\\AppData\\Local"),
    ]);
    assert_eq!(
        env,
        vec![
            ("Path".to_owned(), "C:\\Windows".to_owned()),
            (
                "LOCALAPPDATA".to_owned(),
                "C:\\Users\\u\\AppData\\Local".to_owned()
            ),
        ]
    );
    let mut env = env;
    processes::set_env(&mut env, "path", "D:\\x");
    assert_eq!(env[0], ("Path".to_owned(), "D:\\x".to_owned()));
}

/// Путь А: установщик получает `--updated /S --force-run` (0.4.7: `quitAndInstall(true, true)`
/// electron-updater), отвязанным процессом, с окружением без переменных окна.
#[test]
fn setup_runner_starts_the_installer_detached() {
    let launcher = Arc::new(FakeLauncher::new());
    let runner = ProcessSetupRunner {
        launcher: launcher.clone(),
        env: processes::child_env([("Path", "C:\\Windows"), ("SLINT_SCALE_FACTOR", "2")]),
    };
    runner
        .run(
            std::path::Path::new("C:\\U\\HLSR-Setup-0.5.0.exe"),
            true,
            true,
        )
        .unwrap();
    runner
        .run(
            std::path::Path::new("C:\\U\\HLSR-Setup-0.5.0.exe"),
            true,
            false,
        )
        .unwrap();
    let calls = launcher.calls();
    let LaunchCall::Spawn(first) = &calls[0] else {
        panic!("ожидался отвязанный запуск");
    };
    assert_eq!(first.args, ["--updated", "/S", "--force-run"]);
    assert!(first.detached && !first.hidden && first.cwd.is_none());
    assert_eq!(
        first.env,
        vec![("Path".to_owned(), "C:\\Windows".to_owned())]
    );
    assert_eq!(calls[1].command().args, ["--updated", "/S"]);
}

#[test]
fn latest_yml_is_read_like_electron_updater() {
    let text = "version: 0.5.0\nfiles:\n  - url: HLSR-Setup-0.5.0.exe\n    sha512: abc==\n    size: 123456\n  - url: other.exe\n    size: 1\npath: HLSR-Setup-0.5.0.exe\nsha512: abc==\nreleaseDate: '2026-10-16T09:00:00.000Z'\n";
    let info = update_manager::parse_latest_yml(text);
    assert_eq!(info.version, "0.5.0");
    assert_eq!(info.release_date, Some(json!("2026-10-16T09:00:00.000Z")));
    assert_eq!(info.release_notes, None);
    assert_eq!(
        info.file,
        Some(FeedFile {
            url: "HLSR-Setup-0.5.0.exe".into(),
            sha512: Some("abc==".into()),
            size: Some(123_456),
        })
    );
    let old = update_manager::parse_latest_yml(
        "\u{feff}version: \"0.4.7\"\npath: HLSR-Setup-0.4.7.exe\nreleaseNotes: ~\n",
    );
    assert_eq!(old.version, "0.4.7");
    assert_eq!(old.release_notes, Some(json!(null)));
    assert_eq!(old.file.unwrap().url, "HLSR-Setup-0.4.7.exe");

    assert_eq!(
        update_manager::latest_yml_url(
            "https://hlsr.pro/downloads/launcher/stable",
            1_760_000_000_000
        )
        .unwrap(),
        "https://hlsr.pro/downloads/launcher/stable/latest.yml?noCache=1j742pg00"
    );
    assert_eq!(
        update_manager::latest_yml_url("http://127.0.0.1:8080/feed/?k=v", 5).unwrap(),
        "http://127.0.0.1:8080/feed/latest.yml?k=v"
    );
}

#[cfg(any(debug_assertions, feature = "dev-hooks"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn feed_and_installer_come_from_a_local_server() {
    use hlsr_update::cancel::CancelToken;
    use hlsr_update::http::ReqwestGet;
    use hlsr_update::ports::SystemClock;
    use hlsr_update::update_manager::{DownloadFailure, HttpFeed, InstallerDownload, UpdateFeed};
    use hlsr_update::updater::download::FixedFreeSpace;
    use std::sync::Mutex;
    use support::http::serve_one;

    let body: Vec<u8> = (0..3000).map(|i| (i % 251) as u8).collect();
    let yml = "version: 0.5.0\nfiles:\n  - url: HLSR-Setup-0.5.0.exe\n    size: 3000\npath: HLSR-Setup-0.5.0.exe\nreleaseDate: '2026-10-16T09:00:00.000Z'\n";
    let (server, log) = serve_one("/feed/latest.yml", json!({"body": yml})).await;
    server.routes.lock().unwrap().insert(
        "/feed/HLSR-Setup-0.5.0.exe".to_owned(),
        json!([{"bodySize": 3000}]),
    );
    let feed_url = format!("http://{}/feed", server.authority);

    let feed = HttpFeed::new(
        Arc::new(ReqwestGet::with_allow(|url| {
            url.starts_with("http://127.0.0.1")
        })),
        Arc::new(SystemClock),
    );
    feed.set_feed_url(&feed_url);
    let info = feed.check().await.unwrap();
    assert_eq!(info.version, "0.5.0");
    let request_url = log.lock().unwrap()[0]["url"].as_str().unwrap().to_owned();
    assert!(
        request_url.starts_with("/feed/latest.yml?noCache="),
        "{request_url}"
    );

    feed.set_feed_url(&format!("http://{}/missing", server.authority));
    let error = feed.check().await.unwrap_err();
    assert_eq!(error, "HttpError: 404");
    assert_eq!(update_manager::classify_update_error(&error), "FEED");

    // Без принятого канала установщик берётся по адресу из latest.yml.
    let dir = tempfile::tempdir().unwrap();
    let download = HttpInstallerDownload {
        dir: dir.path().to_owned(),
        free_space: Arc::new(FixedFreeSpace(None)),
        clock: Arc::new(SystemClock),
    };
    let progress = Arc::new(Mutex::new(Vec::new()));
    let file = download
        .download(
            InstallerRequest {
                info: info.clone(),
                feed_url: feed_url.clone(),
                manifest: None,
            },
            CancelToken::new(),
            {
                let progress = progress.clone();
                Arc::new(move |p| progress.lock().unwrap().push(p))
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), body);
    let last = *progress.lock().unwrap().last().unwrap();
    assert_eq!(
        (last.percent, last.transferred, last.total),
        (100.0, 3000.0, 3000.0)
    );

    let cancel = CancelToken::new();
    cancel.cancel();
    let dir = tempfile::tempdir().unwrap();
    let cancelled = HttpInstallerDownload {
        dir: dir.path().to_owned(),
        free_space: Arc::new(FixedFreeSpace(None)),
        clock: Arc::new(SystemClock),
    }
    .download(
        InstallerRequest {
            info,
            feed_url,
            manifest: None,
        },
        cancel,
        Arc::new(|_| {}),
    )
    .await;
    assert_eq!(cancelled, Err(DownloadFailure::Cancelled));
}

#[test]
fn installer_source_prefers_the_signed_channel() {
    let info = UpdateInfo {
        version: "0.5.0".into(),
        release_date: None,
        release_notes: None,
        file: Some(FeedFile {
            url: "HLSR-Setup-0.5.0.exe".into(),
            sha512: None,
            size: Some(10),
        }),
    };
    let manifest = json!({"installer": {"filename": "HLSR-Setup-0.5.0.exe", "size": 123, "urls": [
        "https://hlsr.pro/downloads/launcher/stable/HLSR-Setup-0.5.0.exe",
        "https://github.com/HL-HLSR/HLSR-Releases/releases/download/v0.5.0/HLSR-Setup-0.5.0.exe",
    ]}});
    let (urls, name, size) = HttpInstallerDownload::source(&InstallerRequest {
        info: info.clone(),
        feed_url: "https://hlsr.pro/downloads/launcher/stable".into(),
        manifest: Some(manifest.clone()),
    })
    .unwrap();
    assert_eq!(urls.len(), 2);
    assert!(urls[0].starts_with("https://hlsr.pro/"));
    assert_eq!((name.as_str(), size), ("HLSR-Setup-0.5.0.exe", 123));

    let (urls, _, size) = HttpInstallerDownload::source(&InstallerRequest {
        info: info.clone(),
        feed_url: "https://github.com/HL-HLSR/HLSR-Releases/releases/latest/download".into(),
        manifest: None,
    })
    .unwrap();
    assert_eq!(
        urls,
        ["https://github.com/HL-HLSR/HLSR-Releases/releases/latest/download/HLSR-Setup-0.5.0.exe"]
    );
    assert_eq!(size, 10);

    // QA-лента на loopback: файл с ленты даже при принятом канале.
    let (urls, _, _) = HttpInstallerDownload::source(&InstallerRequest {
        info,
        feed_url: "http://127.0.0.1:8080/feed/".into(),
        manifest: Some(manifest),
    })
    .unwrap();
    assert_eq!(urls, ["http://127.0.0.1:8080/feed/HLSR-Setup-0.5.0.exe"]);
}

#[cfg(windows)]
mod windows_only {
    use super::*;
    use hlsr_update::ports::SystemClock;
    use hlsr_update::updater::download::{FreeSpace, SystemFreeSpace};
    use hlsr_update::updater::install;
    use hlsr_update::updater::process_wait::{self, ProcessList, SystemProcessList, WaitOptions};
    use hlsr_update::updater::processes::{ProcessLauncher, RunExit, Spawn, SystemLauncher};
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::time::Duration;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn system32(name: &str) -> PathBuf {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        PathBuf::from(root).join("System32").join(name)
    }

    /// Процесс-заглушка под своим именем образа: копия `cmd.exe /q /k`, ждёт ввода без окна.
    struct Stub {
        child: std::process::Child,
        name: String,
        _dir: tempfile::TempDir,
    }

    impl Stub {
        fn start() -> Stub {
            let dir = tempfile::tempdir().unwrap();
            let name = format!("hlsr-wait-{}.exe", std::process::id());
            let exe = dir.path().join(&name);
            std::fs::copy(system32("cmd.exe"), &exe).unwrap();
            let child = std::process::Command::new(&exe)
                .args(["/q", "/k"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .unwrap();
            Stub {
                child,
                name,
                _dir: dir,
            }
        }
    }

    impl Drop for Stub {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[tokio::test]
    async fn toolhelp_snapshot_waits_for_an_own_process() {
        let mut stub = Stub::start();
        let pid = stub.child.id();
        let list = SystemProcessList;
        assert_eq!(list.list_pids(&stub.name), vec![pid]);
        assert_eq!(list.list_pids(&stub.name.to_uppercase()), vec![pid]);

        let clock = SystemClock;
        let error = process_wait::wait_for_exit(WaitOptions {
            image_name: &stub.name,
            timeout: Duration::from_millis(600),
            poll: Duration::from_millis(100),
            list: &list,
            clock: &clock,
            sleep: &process_wait::tokio_sleep,
            on_wait: &|_| {},
        })
        .await
        .unwrap_err();
        assert_eq!(error.code, hlsr_update::errors::UpdCode::HlsrStillRunning);
        assert_eq!(
            error.message,
            format!("HLSR не завершился за 1 с (PID: {pid})")
        );

        assert!(SystemLauncher.is_alive(pid));
        stub.child.kill().unwrap();
        stub.child.wait().unwrap();
        process_wait::wait_for_exit(WaitOptions {
            image_name: &stub.name,
            timeout: Duration::from_millis(5000),
            poll: Duration::from_millis(100),
            list: &list,
            clock: &clock,
            sleep: &process_wait::tokio_sleep,
            on_wait: &|_| {},
        })
        .await
        .unwrap();
        assert!(!SystemLauncher.is_alive(pid));
    }

    #[test]
    fn product_version_comes_from_version_info() {
        let version = install::read_exe_product_version(&system32("cmd.exe")).unwrap();
        assert!(install::version_triple(&version).is_some(), "{version}");
        // Тестовый exe без VERSIONINFO: версия не читается — установка принимается (0.4.7).
        let me = std::env::current_exe().unwrap();
        assert_eq!(install::read_exe_product_version(&me), None);
        let installed =
            install::verify_installed(&me, Some("0.5.0"), &install::read_exe_product_version);
        assert!(installed.ok && installed.unverified);
    }

    #[tokio::test]
    async fn system_launcher_reports_exit_codes_without_a_window() {
        let command = Spawn {
            file: system32("cmd.exe"),
            args: vec!["/c".into(), "exit".into(), "3".into()],
            cwd: None,
            env: processes::process_child_env(),
            hidden: true,
            detached: false,
        };
        assert_eq!(
            SystemLauncher.run(&command, Duration::from_secs(30)).await,
            Ok(RunExit::Exited(Some(3)))
        );
        let missing = Spawn {
            file: std::env::temp_dir().join("hlsr-missing-installer.exe"),
            ..command
        };
        let error = SystemLauncher
            .run(&missing, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(
            error.starts_with("spawn ") && error.ends_with(" ENOENT"),
            "{error}"
        );
    }

    #[test]
    fn free_space_is_known_for_the_temp_folder() {
        assert!(SystemFreeSpace.free_bytes(&std::env::temp_dir()).unwrap() > 0);
    }
}
