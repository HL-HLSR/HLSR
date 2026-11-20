//! Транспорт API против 0.4.7: фикстура `transport.json` из `tools/reference/online/transport.js`
//! (`telemetry/transport.js` в Node 24). Чистые функции — на небольшом корпусе; запросы — к двойнику
//! сервера (`support/server.rs`, тот же формат, что у Node): итог каждого вызова и журнал запросов
//! (метод, путь, query, заголовки, тело) совпадают с Node.
//!
//! Отступление от 0.4.7 (спецификация, решение 6): ответ больше лимита. Node рвёт ответ и либо
//! завершает промис пустым телом, либо не завершает вовсе (в фикстуре — `overLimit` и `hang`);
//! Rust завершает запрос ошибкой `TELEMETRY_NETWORK`.
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_online::telemetry::config;
use hlsr_online::telemetry::transport::{
    Headers, HttpTransport, OfflineTransport, RequestOptions, Transport, TransportError,
    retry_after_ms, user_agent,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::server::{Journal, dead_port, start_server};
use support::{Mismatches, fixture, items, str_of};

const DROPPED_RESPONSE_HEADERS: [&str; 3] = ["date", "connection", "keep-alive"];

#[test]
fn user_agent_matches_node() {
    for case in items(&fixture("transport.json")["userAgent"]) {
        assert_eq!(
            user_agent(case[0].as_str()),
            str_of(&case[1]),
            "userAgent({})",
            case[0]
        );
    }
}

#[test]
fn retry_after_ms_matches_node() {
    let fixture = fixture("transport.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["retryAfter"]) {
        let headers = Headers::from_pairs(
            case["headers"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(key, value)| (key.clone(), str_of(value).to_owned())),
        );
        let actual = retry_after_ms(&headers, case["nowMs"].as_i64());
        let actual = actual.map_or(Value::Null, hlsr_core::json_text::js_number);
        mismatches.check(&actual, &case["ms"], format!("retryAfterMs {case}"));
    }
    mismatches.finish("retryAfterMs");
}

#[test]
fn config_matches_node() {
    let fixture = fixture("transport.json");
    let expected = &fixture["config"];
    let actual = json!({
        "BACKEND_CONFIGURED": config::BACKEND_CONFIGURED,
        "API_HOST": config::API_HOST,
        "API_BASE": config::API_BASE,
        "ENDPOINTS": {"batch": config::ENDPOINT_BATCH, "online": config::ENDPOINT_ONLINE},
        "SCHEMA_VERSION": config::SCHEMA_VERSION,
        "QUEUE": {
            "fileName": config::queue::FILE_NAME,
            "maxEvents": config::queue::MAX_EVENTS,
            "maxBytes": config::queue::MAX_BYTES,
            "batchSize": config::queue::BATCH_SIZE,
            "maxBatchBytes": config::queue::MAX_BATCH_BYTES,
        },
        "TRANSPORT": {
            "timeoutMs": config::transport::TIMEOUT_MS,
            "backoffBaseMs": config::transport::BACKOFF_BASE_MS,
            "backoffMaxMs": config::transport::BACKOFF_MAX_MS,
            "jitterRatio": config::transport::JITTER_RATIO,
            "maxConsecutiveFailures": config::transport::MAX_CONSECUTIVE_FAILURES,
            "flushIntervalMs": config::transport::FLUSH_INTERVAL_MS,
        },
        "HEARTBEAT": {
            "launcherMs": config::heartbeat::LAUNCHER_MS,
            "gameMs": config::heartbeat::GAME_MS,
            "jitterRatio": config::heartbeat::JITTER_RATIO,
        },
        "ONLINE": {
            "pollMs": config::online::POLL_MS,
            "timeoutMs": config::online::TIMEOUT_MS,
            "staleAfterMs": config::online::STALE_AFTER_MS,
        },
    });
    assert_eq!(&actual, expected, "telemetry/config.js");
}

fn options_of(spec: &Value) -> RequestOptions {
    let mut options = RequestOptions::default();
    if spec.is_null() {
        return options;
    }
    options.method = spec["method"].as_str().map(str::to_owned);
    options.body = spec["body"].as_str().map(str::to_owned);
    options.timeout_ms = spec["timeoutMs"].as_u64();
    options.max_response_bytes = spec["maxResponseBytes"].as_u64().map(|n| n as usize);
    options.launcher_version = spec["launcherVersion"].as_str().map(str::to_owned);
    if let Some(headers) = spec["headers"].as_object() {
        options.headers = headers
            .iter()
            .map(|(key, value)| (key.clone(), str_of(value).to_owned()))
            .collect();
    }
    options
}

/// Текст ответа в фикстуре: `{text}` до 200 байт UTF-8, иначе `{length, sha256}`.
fn summarize(body: &str) -> Value {
    if body.len() <= 200 {
        return json!({"text": body});
    }
    let sha: String = Sha256::digest(body.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    json!({"length": body.len(), "sha256": sha})
}

/// Во сколько раз растянуты все сроки сценариев (тайм-ауты вызовов, задержки и паузы сервера,
/// момент снятия `hold`): отношения те же, что в записи 0.4.7, а запас между ответом сервера и
/// сроком (0,1-0,2 с в записи) становится в четыре раза больше и под нагрузкой машины не кончается.
const TIME_SCALE: u64 = 4;

/// Маршруты сценария с растянутыми `delayMs` и `pauseMs`.
fn scaled_routes(routes: &Value) -> Value {
    match routes {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let scaled = match (key.as_str(), value.as_u64()) {
                        ("delayMs" | "pauseMs", Some(ms)) => json!(ms * TIME_SCALE),
                        _ => scaled_routes(value),
                    };
                    (key.clone(), scaled)
                })
                .collect(),
        ),
        Value::Array(list) => Value::Array(list.iter().map(scaled_routes).collect()),
        other => other.clone(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_match_node() {
    let fixture = fixture("transport.json");
    let scenarios = items(&fixture["scenarios"]);
    assert!(scenarios.len() >= 8);
    let dead = dead_port().await;
    let transport = HttpTransport::new();
    let mut mismatches = Mismatches::default();

    for scenario in scenarios {
        let journal: Journal = Arc::new(Mutex::new(Vec::new()));
        let server = start_server("api", journal.clone()).await;
        server.set_routes(&scaled_routes(&scenario["routes"]));
        let mut results = Vec::new();
        for call in items(&scenario["calls"]) {
            let url = str_of(&call["url"])
                .replace("{authority}", &server.authority)
                .replace("{port}", &server.port.to_string())
                .replace("{dead}", &dead.to_string());
            if let Some(release) = call.get("release") {
                let name = str_of(&release["name"]).to_owned();
                let wait = Duration::from_millis(release["afterMs"].as_u64().unwrap() * TIME_SCALE);
                let release = server.releaser();
                tokio::spawn(async move {
                    tokio::time::sleep(wait).await;
                    release(&name);
                });
            }
            let mut options = options_of(&call["options"]);
            options.timeout_ms = options.timeout_ms.map(|ms| ms * TIME_SCALE);
            let outcome = transport.request(&url, &options).await;
            results.push(match outcome {
                Ok(response) => {
                    let mut headers = Map::new();
                    for (key, value) in response.headers.iter() {
                        if !DROPPED_RESPONSE_HEADERS.contains(&key) {
                            headers.insert(key.to_owned(), json!(value));
                        }
                    }
                    json!({"ok": {
                        "status": response.status,
                        "headers": headers,
                        "body": summarize(&response.body),
                    }})
                }
                Err(error) => {
                    json!({"error": {"message": error.to_string(), "code": error.code()}})
                }
            });
        }
        let name = str_of(&scenario["name"]);
        let journal: Vec<Value> = journal.lock().unwrap().clone();
        mismatches.check(
            &json!(journal),
            &scenario["journal"],
            format!("{name}: журнал запросов"),
        );
        for ((call, expected), actual) in items(&scenario["calls"])
            .iter()
            .zip(items(&scenario["results"]))
            .zip(&results)
        {
            let context = format!("{name}: {} {}", call["url"], call["options"]);
            if expected.get("overLimit").is_some() || expected.get("hang").is_some() {
                // Отступление от 0.4.7: вместо пустого ответа и зависания — ошибка сети.
                let network = json!({"error": {"message": "TELEMETRY_NETWORK", "code": null}});
                mismatches.check(actual, &network, format!("{context} (ответ больше лимита)"));
            } else {
                mismatches.check(actual, expected, context);
            }
        }
    }
    mismatches.finish("транспорт API");
}

#[tokio::test(flavor = "multi_thread")]
async fn handler_answers_like_the_node_double() {
    let server = start_server("api", Journal::default()).await;
    server.handle("POST /dyn", |entry, index| {
        json!({"json": {
            "n": index,
            "path": entry["path"],
            "query": entry["query"],
            "echo": entry["body"],
        }})
    });
    let transport = HttpTransport::new();
    let mut bodies = Vec::new();
    for body in ["{\"a\":1}", "{\"b\":2}"] {
        let options = RequestOptions {
            method: Some("POST".to_owned()),
            body: Some(body.to_owned()),
            ..RequestOptions::default()
        };
        let url = format!("http://{}/dyn?x=1", server.authority);
        bodies.push(json!(transport.request(&url, &options).await.unwrap().body));
    }
    assert_eq!(json!(bodies), fixture("transport.json")["handler"]);
}

#[test]
fn transport_error_codes() {
    assert_eq!(
        TransportError::BadScheme.to_string(),
        "TELEMETRY_BAD_SCHEME"
    );
    assert_eq!(TransportError::Insecure.to_string(), "TELEMETRY_INSECURE");
    assert_eq!(TransportError::Network.to_string(), "TELEMETRY_NETWORK");
    assert_eq!(TransportError::Timeout.to_string(), "TELEMETRY_TIMEOUT");
    assert_eq!(TransportError::InvalidUrl.code(), Some("ERR_INVALID_URL"));
}

/// Транспорт без сети для отладочных запусков: ни сокета, ни ожидания — сразу обрыв связи.
#[tokio::test]
async fn offline_transport_fails_at_once_without_touching_the_network() {
    let started = std::time::Instant::now();
    for url in [
        "https://api.hlsr.pro/v1/public/online",
        "http://127.0.0.1:1/x",
    ] {
        let result = OfflineTransport
            .request(url, &RequestOptions::default())
            .await;
        assert_eq!(result.unwrap_err(), TransportError::Network, "{url}");
    }
    // Настоящее обращение к сети заняло бы секунды (DNS, соединение): предел с запасом на нагрузку.
    assert!(started.elapsed() < Duration::from_secs(5));
}
