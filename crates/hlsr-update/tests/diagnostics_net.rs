//! Сеть диагностики против 0.4.7: настоящий `httpsGet` раннера (Node, `https` подменён на `http`,
//! адрес переписан на локальный сервер) и `ReqwestRequest` против того же сценария — строки
//! проверок `network` (сравнение по id: порядок завершения запросов не задан), журнал запросов
//! сервера (UA `HLSR-diagnostics`, пути, число редиректов), классификация ошибок и статусов.
//! Только 127.0.0.1; настоящие hlsr.pro и GitHub не используются.

mod support;

use hlsr_update::diagnostics::net::{
    DiagnosticsRequest, DnsLookup, NetError, ReqwestRequest, Response, SystemDns,
    classify_http_status, classify_network_error,
};
use hlsr_update::diagnostics::runner::DiagnosticsRunner;
use hlsr_update::ports::{BoxFuture, FakeClock};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use support::diagnostics::{StubContext, StubDns, Tokens, body_bytes, merge, rows};
use support::http::{self, RequestLog};
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("diagnostics.json"));

/// Адрес `https://хост/путь` → `http://<сервер>/<хост>/путь`: так Node переписывает адрес в сверке.
struct Rewriting {
    inner: ReqwestRequest,
    authority: String,
}

impl DiagnosticsRequest for Rewriting {
    fn get<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<Response, NetError>> {
        let parsed = url::Url::parse(url).unwrap();
        let target = if parsed.host_str() == Some("127.0.0.1") {
            url.to_owned()
        } else {
            let host = match parsed.port() {
                Some(port) => format!("{}:{port}", parsed.host_str().unwrap()),
                None => parsed.host_str().unwrap().to_owned(),
            };
            let query = parsed.query().map(|q| format!("?{q}")).unwrap_or_default();
            format!("http://{}/{host}{}{query}", self.authority, parsed.path())
        };
        Box::pin(async move { self.inner.get(&target).await })
    }
}

/// Маршруты сценария: `@имя` — документ фикстуры, `padTo` — дополнить тело пробелами.
fn materialize(routes: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    for (route, steps) in routes.as_object().unwrap() {
        let steps: Vec<Value> = items(steps)
            .iter()
            .map(|step| {
                let mut step = step.clone();
                let pad_to = step.as_object_mut().unwrap().remove("padTo");
                if let Some(body) = step.get("body") {
                    let mut bytes = body_bytes(&FIXTURE, body).unwrap_or_default();
                    if let Some(pad_to) = pad_to.and_then(|p| p.as_u64()) {
                        bytes.resize(pad_to as usize, b' ');
                    }
                    step["body"] = json!(String::from_utf8(bytes).unwrap());
                }
                step
            })
            .collect();
        out.insert(route.clone(), Value::Array(steps));
    }
    out
}

/// Тексты ошибок соединения у Node и reqwest разные — в сверке остаётся метка.
fn normalized(row: &Value) -> Value {
    let mut row = row.clone();
    let detail = row["detail"].as_str().unwrap_or("").to_owned();
    if row["status"] == "fail" && !detail.is_empty() && !detail.starts_with("HTTP ") {
        row["detail"] = json!("<network>");
    }
    row
}

#[tokio::test]
async fn the_real_request_matches_the_reference() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["network"]) {
        let id = str_of(&case["id"]);
        let platform = str_of(&case["platform"]);
        let log = RequestLog::default();
        let server = http::start_server("p", log.clone()).await;
        let mut map = BTreeMap::new();
        map.insert("p".to_owned(), server.authority.clone());
        let authority = if case["dead"] == true {
            format!("127.0.0.1:{}", http::dead_port().await)
        } else {
            server.authority.clone()
        };
        *server.routes.lock().unwrap() = materialize(&http::fill(&case["routes"], &map));

        let spec = merge(
            &FIXTURE,
            platform,
            &json!({"library": "relative", "userData": null, "files": []}),
        );
        let dir = tempfile::tempdir().unwrap();
        let context = Arc::new(StubContext::new(
            spec.clone(),
            dir.path().to_path_buf(),
            &FIXTURE,
        ));
        let runner = DiagnosticsRunner::new(
            context,
            Arc::new(Rewriting {
                inner: ReqwestRequest::new(),
                authority,
            }),
            Arc::new(StubDns(json!("ok"))),
            Arc::new(FakeClock::new(spec["nowMs"].as_i64().unwrap())),
        )
        .with_launcher_public_keys(vec![str_of(&FIXTURE["keys"]["publicKeyPem"]).to_owned()]);
        let result = runner.run(None).await;

        mismatches.check_eq(
            result.status.as_str(),
            str_of(&case["status"]),
            format!("{id}: итог"),
        );
        let tokens = Tokens::new(dir.path());
        let mut actual: Vec<Value> = rows(&result.checks, &tokens)
            .into_iter()
            .filter(|row| row["group"] == "network")
            .map(|row| normalized(&row))
            .collect();
        actual.sort_by(|a, b| str_of(&a["id"]).cmp(str_of(&b["id"])));
        mismatches.check(
            &Value::Array(actual),
            &case["rows"],
            format!("{id}: строки сети"),
        );

        let mut requests: Vec<Value> = log
            .lock()
            .unwrap()
            .iter()
            .map(|entry| {
                json!([
                    entry["method"],
                    entry["url"],
                    entry["headers"]["user-agent"]
                        .as_str()
                        .map_or(Value::Null, |ua| json!(ua)),
                ])
            })
            .collect();
        requests.sort_by(|a, b| str_of(&a[1]).cmp(str_of(&b[1])));
        mismatches.check(
            &Value::Array(requests),
            &case["requests"],
            format!("{id}: журнал сервера"),
        );
    }
    mismatches.finish("сеть диагностики");
}

#[test]
fn network_classification_matches_the_reference() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["classify"]["network"]) {
        // `code: 123` в корпусе — число, у Rust-ошибки код всегда строка (123 → "123").
        let code = match &case["code"] {
            Value::Null => None,
            Value::String(text) => Some(text.clone()),
            other => Some(other.to_string()),
        };
        let error = NetError {
            code,
            message: case["message"].as_str().unwrap_or("").to_owned(),
        };
        mismatches.check_eq(
            classify_network_error(&error),
            str_of(&case["out"]),
            format!(
                "classifyNetworkError({}, {})",
                case["code"], case["message"]
            ),
        );
    }
    for case in items(&FIXTURE["classify"]["http"]) {
        let status = case["status"].as_u64().unwrap() as u16;
        mismatches.check_eq(
            classify_http_status(status),
            case["out"].as_str(),
            format!("classifyHttpStatus({status})"),
        );
    }
    mismatches.finish("классификация");
}

// ------------------------------------------------------------------ только Rust: простои и DNS

#[tokio::test]
async fn a_silent_server_times_out_with_the_code_the_classifier_knows() {
    // Сервер молчит намного дольше простоя: тест не зависит от скорости машины.
    let (server, _log) = http::serve_one("/slow", json!({"body": "late", "delayMs": 30_000})).await;
    let request = ReqwestRequest::with_idle(Duration::from_millis(150));
    let error = request
        .get(&format!("http://{}/slow", server.authority))
        .await
        .unwrap_err();
    assert_eq!(error.code.as_deref(), Some("ETIMEDOUT"));
    assert_eq!(classify_network_error(&error), "CONNECT_TIMEOUT");
}

#[tokio::test]
async fn a_body_that_stops_midway_times_out() {
    let (server, _log) = http::serve_one(
        "/stall",
        json!({"body": "0123456789", "pauseAfter": 4, "pauseMs": 30_000}),
    )
    .await;
    let request = ReqwestRequest::with_idle(Duration::from_millis(150));
    let error = request
        .get(&format!("http://{}/stall", server.authority))
        .await
        .unwrap_err();
    assert_eq!(classify_network_error(&error), "CONNECT_TIMEOUT");
}

#[tokio::test]
async fn the_user_agent_is_hlsr_diagnostics() {
    let (server, log) = http::serve_one("/x", json!({"body": "ok"})).await;
    let response = ReqwestRequest::new()
        .get(&format!("http://{}/x", server.authority))
        .await
        .unwrap();
    assert_eq!(
        (response.status, response.body.as_deref()),
        (200, Some(&b"ok"[..]))
    );
    assert_eq!(
        log.lock().unwrap()[0]["headers"]["user-agent"],
        "HLSR-diagnostics"
    );
}

#[tokio::test]
async fn system_dns_resolves_localhost_without_the_network() {
    SystemDns.lookup("localhost").await.unwrap();
}
