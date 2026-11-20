//! Служба телеметрии против 0.4.7: фикстура `telemetry-service.json` из
//! `tools/reference/online/telemetry-service.js` (`TelemetryService.js` с очередью, отправкой,
//! согласием, личностью, маркером и квитанцией; обёртки игры `GameDelivery.prototype.telemetry*`;
//! обвязка `AppManager`) — те же сценарии против своего двойника сервера с поддельными часами,
//! `Math.random` и UUID: итог и журнал каждого шага, файлы, журнал запросов.
//! Плюс исправленный отчёт об удалении (в 0.4.7 — без токена, событие терялось).
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_delivery::ports::MemoryLogger;
use hlsr_online::telemetry::sender::AuthorizationProvider;
use hlsr_online::telemetry::telemetry_service::{
    TelemetryOptions, TelemetryService, report_uninstall,
};
use hlsr_online::telemetry::transport::HttpTransport;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use support::server::{Journal, start_server};
use support::{FakeClock, Mismatches, files, fixture, items, log_lines, sequence_random, str_of};

fn uuid(n: usize) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

struct Run {
    root: PathBuf,
    service: TelemetryService,
    clock: FakeClock,
    token: Arc<Mutex<Option<String>>>,
    visible: Arc<AtomicBool>,
    locale: Arc<Mutex<Option<String>>>,
    installed: Value,
}

impl Run {
    /// `readInstalled(id)` поддельной доставки: `hl2-5288` не читается.
    fn installed(&self, game_id: &str) -> Option<&Value> {
        if game_id == "hl2-5288" {
            return None;
        }
        self.installed.get(game_id)
    }

    async fn step(&self, step: &Value) -> Value {
        let service = &self.service;
        let game = || str_of(&step["gameId"]);
        match str_of(&step["op"]) {
            "appStart" => service.start_on_launch(),
            "onConsent" => service.on_consent(step["enabled"] == json!(true)).unwrap(),
            "profileChange" => service.on_profile_change(step["signedIn"] == json!(true)),
            "appShutdown" => service.shutdown(),
            "setEnabled" => return service.set_enabled(step["enabled"] == json!(true)).unwrap(),
            "resetIdentity" => return service.reset_identity(),
            "reportUninstall" => return json!(service.report_uninstall_initiated()),
            "snapshot" => return service.snapshot(),
            "launcherHeartbeat" => service.launcher_heartbeat(),
            "gameHeartbeat" => service.game_heartbeat(),
            "gameStarting" => {
                service.game_starting(game(), self.installed(game()));
                return json!(service.game_session_id(game()));
            }
            "gameRunning" => service.game_running(game()),
            "gameLaunchFailed" => service.game_launch_failed(game(), &step["error"]),
            "gameFinished" => service.game_finished(
                game(),
                Some(&step["lastExit"]),
                step["crashed"] == json!(true),
            ),
            "emit" => return json!(service.emit(str_of(&step["type"]), &step["fields"])),
            "flush" => {
                if let Some(handle) = service.sender().flush_in_background() {
                    handle.await.unwrap();
                }
            }
            "advance" => self.clock.advance(step["ms"].as_i64().unwrap()),
            "token" => *self.token.lock().unwrap() = step["value"].as_str().map(str::to_owned),
            "visible" => self
                .visible
                .store(step["value"] == json!(true), Ordering::SeqCst),
            "locale" => *self.locale.lock().unwrap() = step["value"].as_str().map(str::to_owned),
            "files" => return files(&self.root),
            other => panic!("неизвестный шаг {other}"),
        }
        Value::Null
    }
}

fn options(
    user_data: &Path,
    logger: Arc<MemoryLogger>,
    clock: &FakeClock,
    random: &[f64],
    token: &Arc<Mutex<Option<String>>>,
    endpoint: String,
) -> TelemetryOptions {
    let counter = Arc::new(AtomicUsize::new(0));
    let provider = token.clone();
    TelemetryOptions {
        clock: clock.clock(),
        random: sequence_random(random.to_vec()),
        endpoint: Some(endpoint),
        authorization: Some(
            Arc::new(move || provider.lock().unwrap().clone()) as AuthorizationProvider
        ),
        new_uuid: Some(Arc::new(move || {
            uuid(counter.fetch_add(1, Ordering::SeqCst) + 1)
        })),
        windows_major: Some("unknown"),
        ..TelemetryOptions::new(user_data, "0.5.0", logger, Arc::new(HttpTransport::new()))
    }
}

#[tokio::test]
async fn scenarios_match_node() {
    let fixture = fixture("telemetry-service.json");
    let now = fixture["now"].as_i64().unwrap();
    let random: Vec<f64> = items(&fixture["random"])
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect();
    let mut mismatches = Mismatches::default();
    for scenario in items(&fixture["scenarios"]) {
        let name = str_of(&scenario["name"]);
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        support::setup(&root, &scenario["setup"]);
        let user_data = root.join("user");
        std::fs::create_dir_all(&user_data).unwrap();
        let journal: Journal = Arc::default();
        let server = start_server("api", journal.clone()).await;
        server.set_routes(&scenario["routes"]);

        let logger = Arc::new(MemoryLogger::default());
        let clock = FakeClock::new(now);
        let token = Arc::new(Mutex::new(scenario["token"].as_str().map(str::to_owned)));
        let visible = Arc::new(AtomicBool::new(true));
        let locale = Arc::new(Mutex::new(Some("ru".to_owned())));
        let (visible_in, locale_in) = (visible.clone(), locale.clone());
        let service = TelemetryService::new(TelemetryOptions {
            window_visible: Some(Arc::new(move || visible_in.load(Ordering::SeqCst))),
            locale: Some(Arc::new(move || locale_in.lock().unwrap().clone())),
            install_dir: Some(root.join("install")),
            backend_configured: scenario["backendConfigured"].as_bool(),
            ..options(
                &user_data,
                logger.clone(),
                &clock,
                &random,
                &token,
                format!("http://{}/v1/telemetry/batch", server.authority),
            )
        });
        let run = Run {
            root,
            service,
            clock,
            token,
            visible,
            locale,
            installed: fixture["installed"].clone(),
        };
        for (step, expected) in items(&scenario["steps"])
            .iter()
            .zip(items(&scenario["results"]))
        {
            let result = run.step(step).await;
            run.service.settle().await;
            let actual = json!({"result": result, "logs": log_lines(&logger)});
            mismatches.check(&actual, expected, format!("{name}: {step}"));
        }
        run.service.stop_timers();
        let journal = Value::Array(journal.lock().unwrap().clone());
        mismatches.check(
            &journal,
            &scenario["journal"],
            format!("{name}: журнал запросов"),
        );
    }
    mismatches.finish("служба телеметрии");
}

/// Исправление 0.4.7: событие удаления уходит с токеном входа, отправка дожидается.
#[tokio::test]
async fn uninstall_report_is_sent_with_token() {
    let temp = tempfile::tempdir().unwrap();
    let user_data = temp.path().join("user");
    std::fs::create_dir_all(&user_data).unwrap();
    std::fs::write(
        user_data.join("telemetry.json"),
        r#"{"schemaVersion":1,"enabled":true,"source":"user"}"#,
    )
    .unwrap();
    let journal: Journal = Arc::default();
    let server = start_server("api", journal.clone()).await;
    server.set_routes(&json!({"POST /v1/telemetry/batch": [{"json": {"accepted": 1}}]}));
    let clock = FakeClock::new(1_792_152_000_000);
    let endpoint = format!("http://{}/v1/telemetry/batch", server.authority);

    let run = |token: Option<&str>| {
        let token = Arc::new(Mutex::new(token.map(str::to_owned)));
        options(
            &user_data,
            Arc::new(MemoryLogger::default()),
            &clock,
            &[0.5],
            &token,
            endpoint.clone(),
        )
    };

    let mut lines = Vec::new();
    report_uninstall(run(None), &mut |line| lines.push(line.to_owned())).await;
    assert_eq!(lines, ["telemetry: выключена, событие удаления не создано"]);
    assert!(journal.lock().unwrap().is_empty());

    let mut lines = Vec::new();
    report_uninstall(run(Some("tok-u")), &mut |line| lines.push(line.to_owned())).await;
    assert_eq!(lines, ["telemetry: отправлено событие о запуске удаления"]);
    let journal = journal.lock().unwrap();
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0]["headers"]["authorization"], "Bearer tok-u");
    let body: Value = serde_json::from_str(str_of(&journal[0]["body"])).unwrap();
    let event = &body["events"][0];
    assert_eq!(event["type"], "launcher_uninstall_initiated");
    assert_eq!(event["sessionId"], Value::Null);
    assert_eq!(event["locale"], "ru");
    // Отправленное удалено из очереди.
    let queue = std::fs::read_to_string(user_data.join("telemetry-queue.jsonl")).unwrap();
    assert_eq!(queue, "");
}
