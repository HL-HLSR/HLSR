//! Режим апдейтера без окна (`updater/main.js`): блокировка, план и `QA_TRUST`, рабочий каталог,
//! события окна (`updater:init`, `updater:stage`, `updater:progress`), выход через 2,5 с после
//! успеха, отмена, закрытие с запуском прежнего лаунчера. Внешний мир — временные каталоги и
//! зависимости сессии по сценарию (установщик и лаунчер не запускаются). План — подписанный
//! тестовым ключом из `updater-core.json` (ключи из `QA_TRUST` — только отладочная сборка и
//! `dev-hooks`).

#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_update::cancel::CancelToken;
use hlsr_update::channel::VerifyOutcome;
use hlsr_update::chunks::{self, Assembled, ChunkError, Delta, Manifest};
use hlsr_update::errors::{UpdCode, UpdaterError};
use hlsr_update::ports::{BoxFuture, FakeClock};
use hlsr_update::updater::download;
use hlsr_update::updater::install::{InstallOptions, Installed};
use hlsr_update::updater::mode::{UpdaterConfig, UpdaterEvent, UpdaterMain};
use hlsr_update::updater::process_wait::WaitInfo;
use hlsr_update::updater::session::{AssembleRequest, DownloadRequest, SessionDeps};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use support::{fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("updater-core.json"));

/// Подписанный план «ok» и его ключ из сверки ядра.
fn signed_plan(patch: impl FnOnce(&mut Value)) -> (Value, String) {
    let case = items(&FIXTURE["plan"]["cases"])
        .iter()
        .find(|case| str_of(&case["id"]) == "ok")
        .unwrap();
    let mut raw = case["raw"].clone();
    patch(&mut raw);
    (raw, str_of(&FIXTURE["plan"]["keys"]["test"]).to_owned())
}

/// Зависимости сессии: загрузка по сценарию, остальное удаётся; запуск лаунчера записывается.
#[derive(Default)]
struct Deps {
    download_error: Option<UpdaterError>,
    /// Загрузка ждёт отмены.
    wait_cancel: bool,
    relaunched: Mutex<Vec<String>>,
}

impl SessionDeps for Deps {
    fn fetch_manifest<'a>(
        &'a self,
        _delta: &'a Delta,
        _version: &'a str,
        _cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<Manifest, ChunkError>> {
        Box::pin(async { Err(ChunkError::ManifestUnavailable) })
    }

    fn assemble<'a>(
        &'a self,
        _request: AssembleRequest,
        _on_progress: chunks::OnProgress,
    ) -> BoxFuture<'a, Result<Assembled, ChunkError>> {
        Box::pin(async { Err(ChunkError::ManifestUnavailable) })
    }

    fn download<'a>(
        &'a self,
        request: DownloadRequest,
        on_progress: download::OnProgress,
    ) -> BoxFuture<'a, Result<PathBuf, UpdaterError>> {
        Box::pin(async move {
            on_progress(download::DownloadProgress {
                done: 5,
                total: 10,
                bytes_per_second: 1.5,
            });
            if self.wait_cancel {
                request.cancel.cancelled().await;
                return Err(UpdaterError::new(UpdCode::Cancelled, "отменено"));
            }
            if let Some(error) = &self.download_error {
                return Err(error.clone());
            }
            let file = request.dir.join(&request.filename);
            std::fs::create_dir_all(&request.dir).unwrap();
            std::fs::write(&file, "installer").unwrap();
            Ok(file)
        })
    }

    fn verify_installer<'a>(
        &'a self,
        _path: &'a Path,
        _installer: &'a Value,
    ) -> BoxFuture<'a, VerifyOutcome> {
        Box::pin(async {
            VerifyOutcome {
                ok: true,
                problems: Vec::new(),
                code: None,
            }
        })
    }

    fn wait_for_exit<'a>(
        &'a self,
        _on_wait: &'a (dyn Fn(&WaitInfo) + Send + Sync),
    ) -> BoxFuture<'a, Result<(), UpdaterError>> {
        Box::pin(async { Ok(()) })
    }

    fn run_installer<'a>(
        &'a self,
        _installer: &'a Path,
        _options: InstallOptions,
    ) -> BoxFuture<'a, Result<(), UpdaterError>> {
        Box::pin(async { Ok(()) })
    }

    fn verify_installed(&self, _launcher_exe: &str, _target_version: &str) -> Installed {
        Installed {
            ok: true,
            code: None,
            detail: None,
            version: Some(None),
            unverified: true,
        }
    }

    fn write_receipt(&self, _file: &str, _from: Option<&str>, _to: &str) -> bool {
        true
    }

    fn relaunch(&self, launcher_exe: &str) -> Result<(), UpdaterError> {
        self.relaunched
            .lock()
            .unwrap()
            .push(launcher_exe.to_owned());
        Ok(())
    }

    fn installer_env(&self) -> Vec<(String, String)> {
        Vec::new()
    }
}

struct Run {
    root: tempfile::TempDir,
    main: Arc<UpdaterMain>,
    events: Arc<Mutex<Vec<UpdaterEvent>>>,
    deps: Arc<Deps>,
}

impl Run {
    fn log(&self) -> String {
        std::fs::read_to_string(
            self.root
                .path()
                .join("Local")
                .join("HLSR")
                .join("logs")
                .join("updater.log"),
        )
        .unwrap_or_default()
    }

    fn events(&self) -> Vec<UpdaterEvent> {
        self.events.lock().unwrap().clone()
    }

    fn stages(&self) -> Vec<String> {
        self.events()
            .iter()
            .filter_map(|event| match event {
                UpdaterEvent::Stage(payload) => Some(str_of(&payload["stage"]).to_owned()),
                _ => None,
            })
            .collect()
    }
}

/// Апдейтер с планом `raw` (или без плана), `QA_TRUST` с ключом и зависимостями `deps`.
fn updater(raw: Option<&Value>, key: Option<&str>, deps: Deps, alive: bool) -> Run {
    let root = tempfile::tempdir().unwrap();
    let temp = root.path().join("Temp");
    let stage = temp.join("HLSR-Updater-x-0123456789ab");
    let resources = stage.join("resources");
    std::fs::create_dir_all(&resources).unwrap();
    std::fs::write(stage.join("HLSRUpdater.exe"), "exe").unwrap();
    if let Some(key) = key {
        std::fs::write(
            resources.join("QA_TRUST"),
            serde_json::to_string(&json!({"publicKeys": [key]})).unwrap(),
        )
        .unwrap();
    }
    let mut argv = vec![
        stage.join("HLSRUpdater.exe").to_string_lossy().into_owned(),
        "--hlsr-updater".to_owned(),
    ];
    if let Some(raw) = raw {
        let file = stage.join("update-plan.json");
        std::fs::write(&file, serde_json::to_string(raw).unwrap()).unwrap();
        argv.push(format!("--plan={}", file.display()));
    }
    let events: Arc<Mutex<Vec<UpdaterEvent>>> = Arc::default();
    let deps = Arc::new(deps);
    let main = UpdaterMain::new(
        UpdaterConfig {
            argv,
            exe: stage.join("HLSRUpdater.exe"),
            resources,
            local_app_data: Some(root.path().join("Local")),
            temp_dir: temp,
            pid: 100,
            app_version: "0.5.0".to_owned(),
            qa_trust: true,
        },
        deps.clone(),
        Arc::new(FakeClock::new(1_760_000_000_000)),
        Arc::new(move |_pid| alive),
        {
            let events = events.clone();
            move |event| events.lock().unwrap().push(event)
        },
    );
    Run {
        root,
        main,
        events,
        deps,
    }
}

#[tokio::test(start_paused = true)]
async fn successful_update_quits_after_two_and_a_half_seconds() {
    let (raw, key) = signed_plan(|_| {});
    let run = updater(Some(&raw), Some(&key), Deps::default(), false);
    run.main.run().await;
    assert_eq!(
        run.stages(),
        [
            "download",
            "verify",
            "wait-exit",
            "install",
            "relaunch",
            "done"
        ]
    );
    let events = run.events();
    assert_eq!(
        events[0],
        UpdaterEvent::Init {
            language: "ru".into(),
            target_version: "0.5.0".into()
        }
    );
    assert!(events.contains(&UpdaterEvent::Progress(
        json!({"done": 5, "total": 10, "bytesPerSecond": 1.5})
    )));
    assert!(!events.contains(&UpdaterEvent::Quit));
    tokio::time::sleep(Duration::from_millis(2400)).await;
    assert!(!run.events().contains(&UpdaterEvent::Quit));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(run.events().last(), Some(&UpdaterEvent::Quit));
    assert!(run.main.allow_close());

    let log = run.log();
    for line in [
        "] [info] [updater] запуск, версия HLSR 0.5.0",
        "] [warn] [updater] ВКЛЮЧЕНО QA-доверие (файл QA_TRUST): это не боевой режим",
        "] [info] [updater] план принят: 0.4.7 -> 0.5.0",
        "] [info] [updater] установщик загружен: HLSR-Setup-0.5.0.exe",
        "] [info] [updater] установлена версия 0.5.0",
        "] [info] [updater] обновление завершено, лаунчер запущен",
    ] {
        assert!(log.contains(line), "нет строки «{line}» в журнале:\n{log}");
    }
    // Лаунчер запустила сессия (стадия relaunch); закрытие после успеха второй раз не запускает.
    assert_eq!(
        *run.deps.relaunched.lock().unwrap(),
        [r"C:\HLSR\Launcher\HLSR.exe"]
    );
    run.main.close();
    assert_eq!(run.deps.relaunched.lock().unwrap().len(), 1);
    // Блокировка снимается при выходе.
    let lock = run
        .root
        .path()
        .join("Local")
        .join("HLSR")
        .join("updater.lock");
    assert!(lock.exists());
    run.main.will_quit();
    assert!(!lock.exists());
}

#[tokio::test]
async fn missing_plan_is_reported_without_init() {
    let run = updater(None, None, Deps::default(), false);
    run.main.run().await;
    assert_eq!(
        run.events(),
        [UpdaterEvent::Stage(json!({
            "stage": "error",
            "error": {"code": "UPD_PLAN_UNREADABLE", "message": "путь к плану не передан"},
        }))]
    );
    assert!(
        run.log().contains(
            "[error] [updater] план отклонён: UPD_PLAN_UNREADABLE путь к плану не передан"
        )
    );
    // launcherExe ещё не задан: закрытие ничего не запускает.
    run.main.close();
    assert!(run.deps.relaunched.lock().unwrap().is_empty());
}

/// Карта §0.5: каталог без сегмента `HLSR*` — `UPD_BAD_PLAN`, прежний лаунчер не запускается.
#[tokio::test]
async fn install_dir_without_hlsr_segment_is_refused_and_nothing_is_relaunched() {
    let (raw, key) = signed_plan(|raw| {
        raw["installDir"] = json!("D:\\Games\\Launcher");
        raw["launcherExe"] = json!("D:\\Games\\Launcher\\HLSR.exe");
        raw["language"] = json!("en");
    });
    let run = updater(Some(&raw), Some(&key), Deps::default(), false);
    run.main.run().await;
    let events = run.events();
    assert_eq!(
        events[0],
        UpdaterEvent::Init {
            language: "en".into(),
            target_version: "0.5.0".into()
        }
    );
    assert_eq!(
        events[1],
        UpdaterEvent::Stage(json!({
            "stage": "error",
            "error": {"code": "UPD_BAD_PLAN", "message": "installDir не похож на каталог HLSR"},
        }))
    );
    run.main.close();
    assert!(run.deps.relaunched.lock().unwrap().is_empty());
    assert_eq!(run.events().last(), Some(&UpdaterEvent::Quit));
}

#[tokio::test]
async fn production_keys_reject_a_plan_without_qa_trust() {
    let (raw, _) = signed_plan(|_| {});
    let run = updater(Some(&raw), None, Deps::default(), false);
    run.main.run().await;
    assert_eq!(
        run.events()[1],
        UpdaterEvent::Stage(json!({
            "stage": "error",
            "error": {"code": "UPD_SIGNATURE_INVALID", "message": "подпись Ed25519 не прошла проверку"},
        }))
    );
}

#[tokio::test]
async fn failed_update_relaunches_the_previous_launcher_on_close() {
    let (raw, key) = signed_plan(|_| {});
    let deps = Deps {
        download_error: Some(UpdaterError::new(UpdCode::DownloadHttp, "HTTP 503")),
        ..Deps::default()
    };
    let run = updater(Some(&raw), Some(&key), deps, false);
    run.main.run().await;
    assert_eq!(run.stages(), ["download", "error"]);
    assert!(run.main.allow_close());
    run.main.close();
    assert_eq!(
        *run.deps.relaunched.lock().unwrap(),
        ["C:\\HLSR\\Launcher\\HLSR.exe"]
    );
    assert_eq!(run.events().last(), Some(&UpdaterEvent::Quit));
    assert!(
        run.log()
            .contains("[info] [updater] обновление не состоялось — запущена прежняя версия")
    );
    assert!(
        run.log()
            .contains("[error] [updater] UPD_DOWNLOAD_HTTP: HTTP 503")
    );
}

#[tokio::test]
async fn cancel_is_accepted_only_while_downloading() {
    let (raw, key) = signed_plan(|_| {});
    let deps = Deps {
        wait_cancel: true,
        ..Deps::default()
    };
    let run = updater(Some(&raw), Some(&key), deps, false);
    let main = run.main.clone();
    let task = tokio::spawn(async move { main.run().await });
    while run.stages().is_empty() {
        tokio::task::yield_now().await;
    }
    assert!(run.main.allow_close());
    run.main.cancel();
    task.await.unwrap();
    assert_eq!(run.stages(), ["download", "cancelled"]);
    run.main.cancel();
    let log = run.log();
    assert!(log.contains("[info] [updater] отмена принята"), "{log}");
    assert!(
        log.contains("[info] [updater] отмена отклонена (поздно)"),
        "{log}"
    );
}

#[tokio::test]
async fn a_running_updater_holds_the_lock() {
    let (raw, key) = signed_plan(|_| {});
    let run = updater(Some(&raw), Some(&key), Deps::default(), true);
    let lock_dir = run.root.path().join("Local").join("HLSR");
    std::fs::create_dir_all(&lock_dir).unwrap();
    std::fs::write(lock_dir.join("updater.lock"), "{\"pid\":200,\"ts\":1}").unwrap();
    run.main.run().await;
    assert_eq!(
        run.events(),
        [
            UpdaterEvent::Init {
                language: "ru".into(),
                target_version: String::new()
            },
            UpdaterEvent::Stage(json!({
                "stage": "error",
                "error": {"code": "UPD_ALREADY_RUNNING", "message": "обновление уже выполняется (PID 200)"},
            })),
        ]
    );
    assert!(
        run.log()
            .contains("[warn] [updater] UPD_ALREADY_RUNNING: обновление уже выполняется (PID 200)")
    );
}

#[test]
fn updater_invocation_and_directories() {
    use hlsr_update::updater::mode;
    assert!(mode::is_updater_invocation(
        &["--hlsr-updater".to_owned(), "--plan=x".to_owned()],
        true
    ));
    assert!(!mode::is_updater_invocation(
        &["--hlsr-updater".to_owned()],
        false
    ));
    assert_eq!(mode::APP_NAME, "HLSR Updater");
    assert_eq!(mode::QUIT_DELAY, Duration::from_millis(2500));
    let temp = Path::new("T");
    assert_eq!(
        mode::logs_dir(Some(Path::new("L")), temp),
        Path::new("L").join("HLSR").join("logs")
    );
    assert_eq!(mode::lock_dir(None, temp), temp.join("HLSR"));
    assert_eq!(mode::lock_dir(Some(Path::new("")), temp), temp.join("HLSR"));
}
