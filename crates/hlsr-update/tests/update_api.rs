//! `UpdateApi`: каналы окна карты §4.1 по имени (`ipcMain.handle` `UpdateManager.js:533-651`,
//! `linux/UpdateManager.js:87-94`, `AppManager.js:846-987`), события `updater-state` и
//! `diagnostics-progress`, уведомление `DIAGNOSTICS_NETWORK`, выгрузка ZIP по строкам последнего
//! прогона. Трассы самих служб через фасад — `update_manager_win.rs` и `linux_update_manager.rs`
//! (`*_through_update_api`).

mod support;

use hlsr_update::api::{
    DiagnosticsOptions, UpdateApi, UpdateApiOptions, UpdateEvent, UpdateService, channels,
};
use hlsr_update::boot_gate::{BootAction, BootDecision, UpdateState};
use hlsr_update::diagnostics::runner::DiagnosticsRunner;
use hlsr_update::diagnostics::zip::{SaveDialog, ZipExporter};
use hlsr_update::ports::{BoxFuture, FakeClock, MemoryLogger};
use hlsr_update::update_manager::{UpdateNotifier, UpdateWindow};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use support::diagnostics::{StubContext, StubDns, StubRequest, merge};
use support::fixture;

type Calls = Arc<Mutex<Vec<String>>>;

/// Служба, которая записывает вызовы и отвечает по-разному, чтобы ответы было видно.
struct FakeService {
    calls: Calls,
    window: Mutex<Option<Arc<dyn UpdateWindow>>>,
    game: Mutex<Option<Arc<dyn Fn() -> bool + Send + Sync>>>,
}

impl FakeService {
    fn new(calls: Calls) -> Arc<FakeService> {
        Arc::new(FakeService {
            calls,
            window: Mutex::default(),
            game: Mutex::default(),
        })
    }

    fn call(&self, what: impl Into<String>) {
        self.calls.lock().unwrap().push(what.into());
    }

    /// Служба шлёт снимок и прогресс (как `send()` 0.4.7).
    fn emit(&self, state: &str, progress: f64) {
        let window = self
            .window
            .lock()
            .unwrap()
            .clone()
            .expect("окно задано фасадом");
        window.send_state(json!({"state": state}));
        window.set_progress_bar(progress);
    }
}

impl UpdateService for FakeService {
    fn enabled(&self) -> bool {
        true
    }
    fn state(&self) -> UpdateState {
        UpdateState::Idle
    }
    fn get_state(&self) -> Value {
        self.call("get_state");
        json!({"state": "idle", "enabled": true})
    }
    fn check(&self) -> bool {
        self.call("check");
        true
    }
    fn download(&self) -> BoxFuture<'_, Value> {
        self.call("download");
        Box::pin(async { json!("external") })
    }
    fn cancel(&self) -> bool {
        self.call("cancel");
        false
    }
    fn install_now(&self) -> bool {
        self.call("install_now");
        true
    }
    fn install_on_quit(&self) -> bool {
        self.call("install_on_quit");
        false
    }
    fn set_language(&self, language: &str) -> String {
        self.call(format!("set_language {language:?}"));
        if language == "en" { "en" } else { "ru" }.to_owned()
    }
    fn boot_gate_run(&self, retry: bool) -> BoxFuture<'_, BootDecision> {
        self.call(format!("boot_gate_run {retry}"));
        Box::pin(async {
            BootDecision {
                action: BootAction::Proceed,
                reason: "no-update",
                version: None,
                error: None,
                warn: None,
                mandatory: None,
            }
        })
    }
    fn initialize(&self) {
        self.call("initialize");
    }
    fn start_checking_interval(&self) {
        self.call("start_checking_interval");
    }
    fn check_on_focus(&self) {
        self.call("check_on_focus");
    }
    fn set_window(&self, window: Option<Arc<dyn UpdateWindow>>) {
        self.call("set_window");
        *self.window.lock().unwrap() = window;
    }
    fn set_game_running_check(&self, check: Arc<dyn Fn() -> bool + Send + Sync>) {
        self.call("set_game_running_check");
        *self.game.lock().unwrap() = Some(check);
    }
    fn before_quit(&self) -> bool {
        self.call("before_quit");
        true
    }
    fn on_app_quit(&self, exit_code: i32) {
        self.call(format!("on_app_quit {exit_code}"));
    }
    fn dispose(&self) {
        self.call("dispose");
    }
}

/// Окно ОС: снимки и прогресс на панели задач.
#[derive(Default)]
struct OsWindow(Mutex<Vec<Value>>);

impl UpdateWindow for OsWindow {
    fn send_state(&self, payload: Value) {
        self.0.lock().unwrap().push(json!(["state", payload]));
    }
    fn set_progress_bar(&self, value: f64) {
        self.0.lock().unwrap().push(json!(["progress", value]));
    }
}

#[derive(Default)]
struct Notifier(Mutex<Vec<Value>>);

impl UpdateNotifier for Notifier {
    fn add(&self, level: &str, code: &str, detail: &str) -> Result<(), String> {
        self.0.lock().unwrap().push(json!([level, code, detail]));
        Ok(())
    }
}

fn drain(events: &mut tokio::sync::broadcast::Receiver<UpdateEvent>) -> Vec<UpdateEvent> {
    let mut out = Vec::new();
    while let Ok(event) = events.try_recv() {
        out.push(event);
    }
    out
}

#[tokio::test]
async fn channels_reach_the_service_by_name() {
    let calls: Calls = Arc::default();
    let service = FakeService::new(calls.clone());
    let os = Arc::new(OsWindow::default());
    let api = UpdateApi::new(UpdateApiOptions {
        updates: service.clone(),
        window: Some(os.clone()),
        notifier: None,
        diagnostics: None,
        logger: Arc::new(MemoryLogger::default()),
    });
    assert_eq!(calls.lock().unwrap().clone(), ["set_window"]);
    calls.lock().unwrap().clear();

    let call = |channel: &'static str, args: Vec<Value>| {
        let api = api.clone();
        async move { api.call(channel, &args).await }
    };
    assert_eq!(
        call(channels::BOOT_GATE_RUN, vec![json!({})]).await,
        Some(Ok(json!({"action": "proceed", "reason": "no-update"})))
    );
    // `opts.retry` — по правде JS.
    call(channels::BOOT_GATE_RUN, vec![json!({"retry": true})]).await;
    call(channels::BOOT_GATE_RUN, vec![json!({"retry": 1})]).await;
    call(channels::BOOT_GATE_RUN, vec![json!({"retry": 0})]).await;
    call(channels::BOOT_GATE_RUN, vec![]).await;
    assert_eq!(
        call(channels::UPDATER_CHECK, vec![]).await,
        Some(Ok(json!(true)))
    );
    assert_eq!(
        call(channels::UPDATER_DOWNLOAD, vec![]).await,
        Some(Ok(json!("external")))
    );
    assert_eq!(
        call(channels::UPDATER_CANCEL, vec![]).await,
        Some(Ok(json!(false)))
    );
    assert_eq!(
        call(channels::UPDATER_INSTALL_NOW, vec![]).await,
        Some(Ok(json!(true)))
    );
    assert_eq!(
        call(channels::UPDATER_INSTALL_ON_QUIT, vec![]).await,
        Some(Ok(json!(false)))
    );
    assert_eq!(
        call(channels::UPDATER_SET_LANGUAGE, vec![json!("en")]).await,
        Some(Ok(json!("en")))
    );
    assert_eq!(
        call(channels::UPDATER_SET_LANGUAGE, vec![json!(7)]).await,
        Some(Ok(json!("ru")))
    );
    assert_eq!(
        call(channels::UPDATER_GET_STATE, vec![]).await,
        Some(Ok(json!({"state": "idle", "enabled": true})))
    );
    // Диагностики нет — её каналы не этого фасада; чужие каналы — тоже.
    assert_eq!(call(channels::DIAGNOSTICS_RUN, vec![]).await, None);
    assert_eq!(call(channels::DIAGNOSTICS_CANCEL, vec![]).await, None);
    assert_eq!(call(channels::EXPORT_DIAGNOSTICS_ZIP, vec![]).await, None);
    assert_eq!(call("export-diagnostics", vec![]).await, None);
    assert_eq!(call("game-delivery-info", vec![]).await, None);

    api.initialize();
    api.set_game_running_check(Arc::new(|| true));
    api.start_checking_interval();
    api.check_on_focus();
    assert!(api.before_quit());
    api.on_app_quit(0);
    api.dispose();
    assert_eq!(
        calls.lock().unwrap().clone(),
        [
            "boot_gate_run false",
            "boot_gate_run true",
            "boot_gate_run true",
            "boot_gate_run false",
            "boot_gate_run false",
            "check",
            "download",
            "cancel",
            "install_now",
            "install_on_quit",
            "set_language \"en\"",
            "set_language \"\"",
            "get_state",
            "initialize",
            "set_game_running_check",
            "start_checking_interval",
            "check_on_focus",
            "before_quit",
            "on_app_quit 0",
            "dispose",
        ]
    );
    assert!((service.game.lock().unwrap().clone().unwrap())());
}

#[tokio::test]
async fn service_state_goes_to_the_window_and_the_taskbar() {
    let service = FakeService::new(Calls::default());
    let os = Arc::new(OsWindow::default());
    let api = UpdateApi::new(UpdateApiOptions {
        updates: service.clone(),
        window: Some(os.clone()),
        notifier: None,
        diagnostics: None,
        logger: Arc::new(MemoryLogger::default()),
    });
    let mut events = api.subscribe();
    service.emit("downloading", 0.25);
    service.emit("downloaded", -1.0);
    assert_eq!(
        drain(&mut events),
        [
            UpdateEvent {
                channel: "updater-state",
                payload: json!({"state": "downloading"})
            },
            UpdateEvent {
                channel: "updater-state",
                payload: json!({"state": "downloaded"})
            },
        ]
    );
    assert_eq!(
        os.0.lock().unwrap().clone(),
        [
            json!(["state", {"state": "downloading"}]),
            json!(["progress", 0.25]),
            json!(["state", {"state": "downloaded"}]),
            json!(["progress", -1.0]),
        ]
    );
}

// ------------------------------------------------------------------ диагностика

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("diagnostics.json"));

struct Dialog(Option<PathBuf>);

impl SaveDialog for Dialog {
    fn pick<'a>(&'a self, _default_path: &'a Path) -> BoxFuture<'a, Option<PathBuf>> {
        let answer = self.0.clone();
        Box::pin(async move { answer })
    }
}

struct Diagnostics {
    api: Arc<UpdateApi>,
    notifier: Arc<Notifier>,
    zip_checks: Arc<Mutex<Vec<usize>>>,
    _dir: tempfile::TempDir,
}

fn with_diagnostics(patch: Value, answer: Option<&str>) -> Diagnostics {
    let spec = merge(&FIXTURE, "linux", &patch);
    let dir = tempfile::tempdir().unwrap();
    let runner = DiagnosticsRunner::new(
        Arc::new(StubContext::new(
            spec.clone(),
            dir.path().to_path_buf(),
            &FIXTURE,
        )),
        Arc::new(StubRequest::new(&FIXTURE, "linux", &spec["net"])),
        Arc::new(StubDns(spec["dns"].clone())),
        Arc::new(FakeClock::new(spec["nowMs"].as_i64().unwrap())),
    )
    .with_launcher_public_keys(vec![
        FIXTURE["keys"]["publicKeyPem"].as_str().unwrap().to_owned(),
    ]);
    let notifier = Arc::new(Notifier::default());
    let zip_checks = Arc::new(Mutex::new(Vec::new()));
    let logger = Arc::new(MemoryLogger::default());
    let api = UpdateApi::new(UpdateApiOptions {
        updates: FakeService::new(Calls::default()),
        window: None,
        notifier: Some(notifier.clone()),
        diagnostics: Some(DiagnosticsOptions {
            runner,
            exporter: ZipExporter {
                dialog: Arc::new(Dialog(answer.map(|name| dir.path().join(name)))),
                logger: logger.clone(),
                clock: Arc::new(FakeClock::new(1_792_143_005_000)),
                sevenzip: dir.path().join("7za"),
                temp_dir: dir.path().to_path_buf(),
                documents_dir: dir.path().join("Documents"),
            },
            zip_data: {
                let zip_checks = zip_checks.clone();
                Arc::new(move |checks| {
                    zip_checks.lock().unwrap().push(checks.len());
                    Err("данные отчёта не собраны".to_owned())
                })
            },
        }),
        logger,
    });
    Diagnostics {
        api,
        notifier,
        zip_checks,
        _dir: dir,
    }
}

#[tokio::test]
async fn diagnostics_run_reports_rows_with_the_run_id() {
    let d = with_diagnostics(
        json!({
            "files": [],
            "dns": {"error": {"code": "ENOTFOUND"}},
            "net": {"launcher": {"error": {"code": "ECONNRESET", "message": "x"}},
                    "game": {"status": 200, "body": "@game-tampered"},
                    "github": {"status": 404, "body": ""}},
        }),
        None,
    );
    let mut events = d.api.subscribe();
    let reply = d
        .api
        .call(channels::DIAGNOSTICS_RUN, &[])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply["runId"], 1);
    assert_eq!(reply["status"], "ok");
    let checks = reply["checks"].as_array().unwrap();
    // `Object.assign({ runId }, item)`: номер прогона первым, затем строка как есть.
    let progress = drain(&mut events);
    assert_eq!(progress.len(), checks.len());
    for (event, check) in progress.iter().zip(checks) {
        assert_eq!(event.channel, "diagnostics-progress");
        let object = event.payload.as_object().unwrap();
        assert_eq!(object.keys().next().map(String::as_str), Some("runId"));
        assert_eq!(event.payload["runId"], 1);
        let mut row = event.payload.clone();
        row.as_object_mut().unwrap().remove("runId");
        assert_eq!(&row, check);
    }
    assert_eq!(
        d.notifier.0.lock().unwrap().clone(),
        [json!([
            "warning",
            "DIAGNOSTICS_NETWORK",
            "DNS_ERROR, CONNECTION_RESET, SIGNATURE_INVALID, HTTP_404_FEED_MISSING"
        ])]
    );
    assert_eq!(
        d.api.call(channels::DIAGNOSTICS_CANCEL, &[]).await,
        Some(Ok(json!(true)))
    );

    // Выгрузка без выбора файла — отмена; данные отчёта не собираются.
    assert_eq!(
        d.api
            .call(
                channels::EXPORT_DIAGNOSTICS_ZIP,
                &[json!({"includeDump": true})]
            )
            .await,
        Some(Ok(json!({"ok": false, "cancelled": true})))
    );
    assert!(d.zip_checks.lock().unwrap().is_empty());
}

#[tokio::test]
async fn export_takes_the_rows_of_the_last_run() {
    let d = with_diagnostics(json!({"files": []}), Some("report.zip"));
    // До прогона строк нет (`lastDiagnostics = null`).
    assert_eq!(
        d.api.call(channels::EXPORT_DIAGNOSTICS_ZIP, &[]).await,
        Some(Ok(json!({"ok": false, "error": "EXPORT_FAILED"})))
    );
    let reply = d
        .api
        .call(channels::DIAGNOSTICS_RUN, &[])
        .await
        .unwrap()
        .unwrap();
    let rows = reply["checks"].as_array().unwrap().len();
    assert_eq!(
        d.api
            .call(
                channels::EXPORT_DIAGNOSTICS_ZIP,
                &[json!({"includeDump": "yes"})]
            )
            .await,
        Some(Ok(json!({"ok": false, "error": "EXPORT_FAILED"})))
    );
    assert_eq!(d.zip_checks.lock().unwrap().clone(), [0, rows]);
    assert!(d.notifier.0.lock().unwrap().is_empty());
}
