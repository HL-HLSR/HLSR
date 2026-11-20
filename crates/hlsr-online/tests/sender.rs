//! Отправка очереди против 0.4.7: фикстура `sender.json` из `tools/reference/online/sender.js`
//! (`telemetry/Sender.js` с `Queue.js` и `transport.js` в Node 24) — те же сценарии против своего
//! двойника сервера: итоги `flush`, снимки, журнал, байты очереди и журнал запросов.
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_delivery::ports::MemoryLogger;
use hlsr_online::telemetry::queue::TelemetryQueue;
use hlsr_online::telemetry::sender::{Sender, SenderOptions};
use hlsr_online::telemetry::transport::HttpTransport;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::server::{Journal, Server, dead_port, start_server};
use support::{FakeClock, Mismatches, files, fixture, items, log_lines, sequence_random, str_of};

struct Run {
    root: std::path::PathBuf,
    queue: Arc<Mutex<TelemetryQueue>>,
    sender: Arc<Sender>,
    clock: FakeClock,
    token: Arc<Mutex<Option<String>>>,
    server: Server,
    journal: Journal,
}

impl Run {
    fn step<'a>(
        &'a self,
        step: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Value> + 'a>> {
        Box::pin(async move {
            match str_of(&step["op"]) {
                "append" => json!(self.queue.lock().unwrap().append(&step["event"])),
                "flush" => self.sender.flush().await.to_json(),
                "advance" => {
                    self.clock.advance(step["ms"].as_i64().unwrap());
                    Value::Null
                }
                "token" => {
                    *self.token.lock().unwrap() = step["value"].as_str().map(str::to_owned);
                    Value::Null
                }
                "purge" => json!(self.queue.lock().unwrap().purge()),
                "size" => json!(self.queue.lock().unwrap().size()),
                "files" => files(&self.root),
                "flushHeld" => {
                    let sender = self.sender.clone();
                    let pending = tokio::spawn(async move { sender.flush().await });
                    let before = self.journal.lock().unwrap().len();
                    while self.journal.lock().unwrap().len() == before {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let mut during = Vec::new();
                    for inner in items(&step["during"]) {
                        during.push(self.step(inner).await);
                    }
                    self.server.release(str_of(&step["gate"]));
                    let result = pending.await.unwrap();
                    json!({"during": during, "result": result.to_json()})
                }
                other => panic!("неизвестный шаг {other}"),
            }
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenarios_match_node() {
    let fixture = fixture("sender.json");
    let now = fixture["now"].as_i64().unwrap();
    let random: Vec<f64> = items(&fixture["random"])
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect();
    let dead = dead_port().await;
    let mut mismatches = Mismatches::default();
    for scenario in items(&fixture["scenarios"]) {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let journal: Journal = Arc::default();
        let server = start_server("api", journal.clone()).await;
        server.set_routes(&scenario["routes"]);
        let logger = Arc::new(MemoryLogger::default());
        let clock = FakeClock::new(now);
        let queue = Arc::new(Mutex::new(TelemetryQueue::new(
            root.join("telemetry-queue.jsonl"),
            0,
            0,
            logger.clone(),
            clock.clock(),
        )));
        let token = Arc::new(Mutex::new(scenario["token"].as_str().map(str::to_owned)));
        let endpoint = match scenario["endpoint"].as_str() {
            Some("dead") => format!("http://127.0.0.1:{dead}/v1/telemetry/batch"),
            Some(path) => format!("http://{}{path}", server.authority),
            None => format!("http://{}/v1/telemetry/batch", server.authority),
        };
        let provider = token.clone();
        let sender = Arc::new(Sender::new(SenderOptions {
            queue: queue.clone(),
            endpoint: Some(endpoint),
            launcher_version: Some("0.5.0".to_owned()),
            logger: logger.clone(),
            clock: clock.clock(),
            random: sequence_random(random.clone()),
            transport: Arc::new(HttpTransport::new()),
            authorization: scenario.get("token").map(|_| {
                Arc::new(move || provider.lock().unwrap().clone())
                    as hlsr_online::telemetry::sender::AuthorizationProvider
            }),
        }));
        let run = Run {
            root,
            queue,
            sender,
            clock,
            token,
            server,
            journal: journal.clone(),
        };
        let name = str_of(&scenario["name"]);
        for (step, expected) in items(&scenario["steps"])
            .iter()
            .zip(items(&scenario["results"]))
        {
            let result = run.step(step).await;
            let actual = json!({
                "result": result,
                "snapshot": run.sender.snapshot(),
                "logs": log_lines(&logger),
            });
            mismatches.check(&actual, expected, format!("{name}: {step}"));
        }
        let journal = Value::Array(journal.lock().unwrap().clone());
        mismatches.check(
            &journal,
            &scenario["journal"],
            format!("{name}: журнал запросов"),
        );
    }
    mismatches.finish("отправка телеметрии");
}
