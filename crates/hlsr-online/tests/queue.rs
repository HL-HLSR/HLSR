//! Очередь телеметрии против 0.4.7: фикстура `queue.json` из `tools/reference/online/queue.js`
//! (`telemetry/Queue.js` в Node 24): итоги шагов, строки журнала и байты файлов, включая имя
//! карантина по поддельным часам.

mod support;

use hlsr_delivery::ports::MemoryLogger;
use hlsr_online::telemetry::queue::TelemetryQueue;
use serde_json::{Value, json};
use std::sync::Arc;
use support::{FakeClock, Mismatches, files, fixture, items, log_lines, setup, str_of};

fn usize_of(value: &Value) -> usize {
    value.as_u64().unwrap_or(0) as usize
}

#[test]
fn scenarios_match_node() {
    let fixture = fixture("queue.json");
    let now = fixture["now"].as_i64().unwrap();
    let mut mismatches = Mismatches::default();
    for scenario in items(&fixture["scenarios"]) {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        setup(&root, &scenario["setup"]);
        let logger = Arc::new(MemoryLogger::default());
        let options = &scenario["options"];
        let mut queue = TelemetryQueue::new(
            root.join("telemetry-queue.jsonl"),
            usize_of(&options["maxEvents"]),
            usize_of(&options["maxBytes"]),
            logger.clone(),
            FakeClock::new(now).clock(),
        );
        let name = str_of(&scenario["name"]);
        for (step, expected) in items(&scenario["steps"])
            .iter()
            .zip(items(&scenario["results"]))
        {
            let result = match str_of(&step["op"]) {
                "files" => files(&root),
                "size" => json!(queue.size()),
                "readAll" => {
                    let all = queue.read_all();
                    json!({"events": all.events, "broken": all.broken})
                }
                "append" => json!(queue.append(&step["event"])),
                "peek" => json!(queue.peek(usize_of(&step["max"]), usize_of(&step["bytes"]))),
                "remove" => {
                    queue.remove(usize_of(&step["count"]));
                    Value::Null
                }
                "purge" => json!(queue.purge()),
                "generation" => json!(queue.generation()),
                other => panic!("неизвестный шаг {other}"),
            };
            let actual = json!({"result": result, "logs": log_lines(&logger)});
            mismatches.check(&actual, expected, format!("{name}: {step}"));
        }
    }
    mismatches.finish("очередь телеметрии");
}
