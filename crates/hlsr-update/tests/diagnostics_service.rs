//! Обработчики `diagnostics-run` и `diagnostics-cancel` (`AppManager.js:846-924`): номер прогона,
//! повторный вызов во время прогона получает тот же результат, строки уходят окну по мере
//! готовности, последний результат помнится для отчёта, сетевые сбои дают `DIAGNOSTICS_NETWORK`.

mod support;

use hlsr_update::diagnostics::runner::{Check, DiagnosticsRunner, RunStatus};
use hlsr_update::diagnostics::service::{DiagnosticsEvents, DiagnosticsService};
use hlsr_update::ports::{FakeClock, MemoryLogger};
use serde_json::{Value, json};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use support::diagnostics::{StubContext, StubDns, StubRequest, merge};
use support::fixture;

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("diagnostics.json"));

#[derive(Default)]
struct Events {
    progress: Mutex<Vec<(u64, String)>>,
    network: Mutex<Vec<String>>,
}

impl DiagnosticsEvents for Events {
    fn progress(&self, run_id: u64, check: &Check) {
        self.progress
            .lock()
            .unwrap()
            .push((run_id, check.id().to_owned()));
    }

    fn network_failed(&self, detail: &str) {
        self.network.lock().unwrap().push(detail.to_owned());
    }
}

struct Fixture {
    service: Arc<DiagnosticsService>,
    events: Arc<Events>,
    logger: Arc<MemoryLogger>,
    _dir: tempfile::TempDir,
}

fn service(patch: Value) -> Fixture {
    let spec = merge(&FIXTURE, "linux", &patch);
    let dir = tempfile::tempdir().unwrap();
    let context = Arc::new(StubContext::new(
        spec.clone(),
        dir.path().to_path_buf(),
        &FIXTURE,
    ));
    let runner = DiagnosticsRunner::new(
        context,
        Arc::new(StubRequest::new(&FIXTURE, "linux", &spec["net"])),
        Arc::new(StubDns(spec["dns"].clone())),
        Arc::new(FakeClock::new(spec["nowMs"].as_i64().unwrap())),
    )
    .with_launcher_public_keys(vec![
        FIXTURE["keys"]["publicKeyPem"].as_str().unwrap().to_owned(),
    ]);
    let events = Arc::new(Events::default());
    let logger = Arc::new(MemoryLogger::default());
    Fixture {
        service: DiagnosticsService::new(runner, events.clone(), logger.clone()),
        events,
        logger,
        _dir: dir,
    }
}

#[tokio::test]
async fn run_numbers_the_runs_and_reports_progress() {
    let f = service(json!({"files": []}));
    let first = f.service.run().await;
    assert_eq!((first.run_id, first.status), (1, RunStatus::Ok));
    assert!(first.checks.len() > 20);
    // Каждая строка уходит окну с номером прогона, в порядке готовности.
    let progress = f.events.progress.lock().unwrap().clone();
    assert_eq!(progress.len(), first.checks.len());
    assert!(progress.iter().all(|(run, _)| *run == 1));
    assert_eq!(
        progress
            .iter()
            .map(|(_, id)| id.as_str())
            .collect::<Vec<_>>(),
        first.checks.iter().map(Check::id).collect::<Vec<_>>()
    );
    let value = first.to_value();
    assert_eq!(value["runId"], 1);
    assert_eq!(value["status"], "ok");
    assert_eq!(value["checks"][0]["id"], "hlsr-version");

    let second = f.service.run().await;
    assert_eq!(second.run_id, 2);
    assert_eq!(f.service.last_checks().unwrap().len(), second.checks.len());
    assert!(f.events.network.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_second_call_during_a_run_gets_the_same_result() {
    let f = service(json!({"dns": "hang", "files": []}));
    let first = {
        let service = f.service.clone();
        tokio::spawn(async move { service.run().await })
    };
    tokio::time::sleep(Duration::from_millis(60)).await;
    let second = {
        let service = f.service.clone();
        tokio::spawn(async move { service.run().await })
    };
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(f.service.cancel());
    let (first, second) = (first.await.unwrap(), second.await.unwrap());
    assert!(Arc::ptr_eq(&first, &second), "тот же результат");
    assert_eq!((first.run_id, first.status), (1, RunStatus::Cancelled));
    assert_eq!(
        f.logger.take(),
        vec![(
            "info",
            "[diagnostics] прогон уже идёт — переиспользую активный runId".to_owned()
        )]
    );
    // Отмена не мешает следующему прогону.
    let again = {
        let service = f.service.clone();
        tokio::spawn(async move { service.run().await })
    };
    tokio::time::sleep(Duration::from_millis(60)).await;
    f.service.cancel();
    assert_eq!(again.await.unwrap().run_id, 2);
}

#[tokio::test]
async fn network_failures_raise_the_notification() {
    let f = service(json!({
        "files": [],
        "dns": {"error": {"code": "ENOTFOUND"}},
        "net": {"launcher": {"error": {"code": "ECONNRESET", "message": "x"}},
                "game": {"status": 200, "body": "@game-tampered"},
                "github": {"status": 404, "body": ""}},
    }));
    let outcome = f.service.run().await;
    assert_eq!(outcome.status, RunStatus::Ok);
    // Коды сбоев в порядке строк, через запятую.
    assert_eq!(
        f.events.network.lock().unwrap().clone(),
        vec!["DNS_ERROR, CONNECTION_RESET, SIGNATURE_INVALID, HTTP_404_FEED_MISSING".to_owned()]
    );
}
