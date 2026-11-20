//! Зеркало ремонта против 0.4.9: фикстура `repair-mirror.json` из
//! `tools/reference/delivery/repair-mirror.js` (на копии release-0.4.9).
//!
//! `fileUrl`, `unavailableReason` и `plan` на корпусе (каждая причина отказа плана, раскладка,
//! фильтр провайдеров, предел размера) и `RepairMirrorClient` на локальных серверах: адреса по
//! раскладке `sha256-2`, поток в `.part` с проверкой размера и SHA-256, повтор на том же
//! провайдере, переход к следующему, отказ без повтора при ответе не 200, только HTTPS (loopback —
//! по HTTP), путь файла внутри каталога, отмена до начала и посреди файла, файлы эталона на диске.
//! Сравниваются итог шага, события прогресса после каждого файла (промежуточные зависят от
//! времени и нарезки потока), журнал запросов и дерево целевой папки.

mod support;

use hlsr_delivery::archive_service::CancelHandle;
use hlsr_delivery::errors::Failure;
use hlsr_delivery::repair_mirror::{
    self, FetchOptions, FetchProgress, PlanOptions, RepairMirrorClient, RepairMirrorOptions,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use support::http::{RequestLog, Server, dead_port, fill, start_server, unfill};
use support::{Mismatches, fixture, items, opt_str, sha256_hex, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("repair-mirror.json"));

/// Итог шага: `{ok}` или ошибка (`DeliveryError.toPublic()` или код и текст чужой).
fn failure_of(failure: &Failure, map: &BTreeMap<String, String>) -> Value {
    match failure {
        Failure::Delivery(error) => json!({"error": unfill(&error.to_public(), map)}),
        Failure::Raw(error) => json!({"raw": {
            "code": error.code,
            "message": unfill(&json!(error.message), map),
        }}),
    }
}

#[test]
fn constants_match_0_4_9() {
    let c = &FIXTURE["constants"];
    assert_eq!(c["LAYOUTS"], json!(repair_mirror::LAYOUTS));
    assert_eq!(c["MAX_FILES"], json!(repair_mirror::MAX_FILES));
    assert_eq!(c["SHA256_PATTERN"], json!(repair_mirror::SHA256_PATTERN));
    let t = &FIXTURE["timing"];
    assert_eq!(
        t["PROGRESS_EVERY_MS"],
        json!(repair_mirror::PROGRESS_EVERY_MS)
    );
    assert_eq!(t["LOCK_RETRY_MS"], json!(repair_mirror::LOCK_RETRY_MS));
}

#[test]
fn unavailable_reason_matches_0_4_9() {
    let mut m = Mismatches::default();
    for (index, case) in items(&FIXTURE["unavailableReason"]).iter().enumerate() {
        let options = &case["options"];
        let allowed: Vec<String> = options["allowedProviderTypes"]
            .as_array()
            .map(|list| list.iter().map(|t| str_of(t).to_owned()).collect())
            .unwrap_or_default();
        let reason = repair_mirror::unavailable_reason(
            case.get("mirror").filter(|m| !m.is_null()),
            &PlanOptions {
                version: opt_str(&options["version"]),
                allowed_provider_types: &allowed,
            },
        );
        m.check(
            &json!({"ok": reason}),
            &case["result"],
            format!("unavailableReason, случай {index}"),
        );
    }
    m.finish("unavailableReason");
}

#[test]
fn file_url_matches_0_4_9() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["fileUrl"]) {
        let actual = match repair_mirror::file_url(
            str_of(&case["base"]),
            opt_str(&case["sha256"]),
            opt_str(&case["layout"]),
        ) {
            Ok(url) => json!({"ok": url}),
            Err(error) => json!({"error": error.to_public()}),
        };
        m.check(&actual, &case["result"], format!("fileUrl {case}"));
    }
    m.finish("fileUrl");
}

#[test]
fn plan_matches_0_4_9() {
    let mut m = Mismatches::default();
    for (index, case) in items(&FIXTURE["plan"]).iter().enumerate() {
        let entries: Vec<Value> = match case.get("generate") {
            Some(spec) => (0..spec["count"].as_u64().unwrap())
                .map(|i| {
                    let rel = format!("gen/{i}");
                    json!({"path": rel, "size": spec["size"], "sha256": sha256_hex(rel.as_bytes())})
                })
                .collect(),
            None => items(&case["entries"]).clone(),
        };
        let options = &case["options"];
        let allowed: Vec<String> = options["allowedProviderTypes"]
            .as_array()
            .map(|list| list.iter().map(|t| str_of(t).to_owned()).collect())
            .unwrap_or_default();
        let plan = repair_mirror::plan(
            case.get("mirror").filter(|m| !m.is_null()),
            &entries,
            &PlanOptions {
                version: opt_str(&options["version"]),
                allowed_provider_types: &allowed,
            },
        );
        m.check(
            &json!({"ok": plan.to_json()}),
            &case["result"],
            format!("plan, случай {index}"),
        );
    }
    m.finish("plan");
}

// ---------------------------------------------------------------- сценарии сети

async fn run_scenario(spec: &Value, m: &mut Mismatches) {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    let log = RequestLog::default();
    let mut map = BTreeMap::new();
    map.insert(
        "dead".to_owned(),
        format!("127.0.0.1:{}", dead_port().await),
    );
    let mut servers: Vec<(Server, Value)> = Vec::new();
    for (name, routes) in spec["servers"].as_object().unwrap() {
        let server = start_server(name, log.clone()).await;
        map.insert(name.clone(), server.authority.clone());
        servers.push((server, routes.clone()));
    }
    for (server, routes) in &servers {
        let routes = fill(routes, &map);
        let mut table = server.routes.lock().unwrap();
        for (key, value) in routes.as_object().unwrap() {
            table.insert(key.clone(), value.clone());
        }
    }
    let mut errors_map = map.clone();
    errors_map.insert(
        "root".to_owned(),
        root.path().to_string_lossy().into_owned(),
    );

    let name = str_of(&spec["name"]);
    assert_eq!(
        items(&spec["steps"]).len(),
        items(&spec["results"]).len(),
        "шагов и результатов поровну"
    );
    for (index, (step, expected)) in items(&spec["steps"])
        .iter()
        .zip(items(&spec["results"]))
        .enumerate()
    {
        let client = RepairMirrorClient::new(RepairMirrorOptions {
            timeout: Some(Duration::from_millis(3000)),
            max_attempts: step["maxAttempts"].as_u64().map(|n| n as u32),
            ..RepairMirrorOptions::default()
        });
        let progress: Arc<Mutex<Vec<Value>>> = Arc::default();
        let mut actual = match str_of(&step["op"]) {
            "fetchInto" => {
                let entries = items(&fill(&step["entries"], &map)).clone();
                let providers: Vec<Value> = items(&fill(&step["providers"], &map))
                    .iter()
                    .map(|url| json!({"url": url}))
                    .collect();
                let signal = CancelHandle::new();
                if step["abortBefore"] == true {
                    signal.cancel();
                }
                // Отмена через 100 мс после того, как сервер увидел столько запросов.
                let watcher = step["abortAfterRequests"].as_u64().map(|count| {
                    let (log, signal) = (log.clone(), signal.clone());
                    tokio::spawn(async move {
                        while (log.lock().unwrap().len() as u64) < count {
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        signal.cancel();
                    })
                });
                // Только события после файла: bytes — сумма размеров первых `fetched` записей.
                let sizes: Vec<u64> = entries
                    .iter()
                    .map(|e| e["size"].as_u64().unwrap())
                    .collect();
                let sink = progress.clone();
                let options = FetchOptions {
                    providers,
                    layout: str_of(&step["layout"]).to_owned(),
                    signal: Some(signal),
                    on_progress: Some(Arc::new(move |p: &FetchProgress| {
                        if p.bytes == sizes[..p.fetched].iter().sum::<u64>() {
                            sink.lock().unwrap().push(p.to_json());
                        }
                    })),
                };
                let outcome = match client.fetch_into(&target, &entries, &options).await {
                    Ok(fetched) => json!({"ok": {"files": fetched.files, "bytes": fetched.bytes}}),
                    Err(failure) => failure_of(&failure, &errors_map),
                };
                if let Some(watcher) = watcher {
                    watcher.abort();
                }
                json!({"outcome": outcome, "progress": progress.lock().unwrap().clone()})
            }
            "fetchOne" => {
                // Файл пишется в one/file.bin через one/file.bin.part; после шага папка пустеет.
                let dir = root.path().join("one");
                let file = dir.join("file.bin");
                std::fs::create_dir_all(&dir).unwrap();
                let base = fill(&step["base"], &map);
                let outcome = match client
                    .fetch_one(
                        str_of(&base),
                        &step["entry"],
                        str_of(&step["layout"]),
                        &file,
                        None,
                        &mut |_| {},
                    )
                    .await
                {
                    Ok(length) => json!({"ok": {
                        "length": length,
                        "sha256": sha256_hex(&std::fs::read(&file).unwrap()),
                    }}),
                    Err(failure) => failure_of(&failure, &errors_map),
                };
                let mut left: Vec<String> = std::fs::read_dir(&dir)
                    .unwrap()
                    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect();
                left.sort();
                let _ = std::fs::remove_file(&file);
                json!({"outcome": outcome, "left": left})
            }
            "tree" => {
                let tree = if target.exists() {
                    support::tree::snapshot(&target, None)
                } else {
                    Value::Null
                };
                json!({"tree": tree})
            }
            other => panic!("{name}: неизвестный шаг {other}"),
        };
        if step["op"] != "tree" {
            let requests: Vec<Value> = std::mem::take(&mut *log.lock().unwrap())
                .iter()
                .map(|entry| {
                    let entry = unfill(entry, &map);
                    let mut out = json!({
                        "server": entry["server"],
                        "method": entry["method"],
                        "url": entry["url"],
                    });
                    if let Some(agent) = entry["headers"].get("user-agent") {
                        out["userAgent"] = agent.clone();
                    }
                    out
                })
                .collect();
            actual["requests"] = Value::Array(requests);
        }
        m.check(
            &actual,
            expected,
            format!("{name}, шаг {index} ({})", step["op"]),
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mirror_scenarios_match_0_4_9() {
    let mut m = Mismatches::default();
    for spec in items(&FIXTURE["scenarios"]) {
        run_scenario(spec, &mut m).await;
    }
    m.finish("зеркало ремонта");
}
