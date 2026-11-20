//! Шлюз обновления лаунчера против 0.4.7: фикстура `gate.json` из `tools/reference/update/gate.js`
//! (модули 0.4.7 в Node 24 против локальных серверов 127.0.0.1; `https` подменён на `http`, allowlist —
//! loopback без путей `/blocked…`, как в самотестах 0.4.7 подменяют `httpGet`). Порядок запросов
//! (основной, запасной, `latest.yml` повторно), редиректы вне allowlist, обрыв, 256 КиБ + 1, итог
//! шлюза, `probeLatest`, `verifyInstaller`; прямые GET (`httpsGet`) и экран загрузки (`boot-gate.js`).
//! Самотесты `launcher-update-selftest.js` («gate falls back and rejects unsigned, Windows installer,
//! old archive schema and downgrade», «installation on quit requires an explicit choice; mandatory
//! boot errors block» — часть про экран загрузки) и `diagnostics-selftest.js` («empty, truncated or
//! redirected metadata cannot pass as a healthy channel») — случаи этой же фикстуры.
//!
//! Тексты сетевых ошибок Node (обрыв, отказ в соединении) и V8 (`JSON.parse`) в Rust другие: в
//! сверке их заменяют метки `<network>` и `<json>`, остальные тексты сравниваются дословно.

#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_core::js::Platform;
use hlsr_update::boot_gate::{self, BootGateHost, Subscriber, UpdateState, run_boot_gate};
use hlsr_update::cancel::CancelToken;
use hlsr_update::channel::VerifyOptions;
use hlsr_update::gate::{GateError, GateOptions, LauncherUpdateGate, VerifyInstallerOptions};
use hlsr_update::http::ReqwestGet;
use hlsr_update::ports::{GetError, GetOptions, HttpGet, Logger, MemoryLogger};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use support::fakes::ScriptedHost;
use support::http::{self, RequestLog, Server};
use support::{Mismatches, fixture, items, sha256_hex, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("gate.json"));

fn platform(name: &str) -> Platform {
    match name {
        "win32" => Platform::Win32,
        "linux" => Platform::Linux,
        other => panic!("платформа {other}"),
    }
}

/// Разрешён `http` на 127.0.0.1, кроме путей `/blocked…` (в Node — подмена `channel.isAllowedUrl`).
fn allow_loopback(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    parsed.scheme() == "http"
        && parsed.host_str() == Some("127.0.0.1")
        && !parsed.path().starts_with("/blocked")
}

// ------------------------------------------------------------ сервера и подстановки

/// Серверы сценария и подстановка их адресов в маршруты; `dead` — адрес без слушателя.
struct Servers {
    _servers: Vec<Server>,
    map: BTreeMap<String, String>,
    log: RequestLog,
}

/// `padTo`: тело дополняется пробелами до заданного числа байт (в фикстуре лежит один документ).
fn materialize(routes: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    for (route, steps) in routes.as_object().expect("маршруты сервера") {
        let steps: Vec<Value> = items(steps)
            .iter()
            .map(|step| {
                let mut step = step.clone();
                if let Some(pad_to) = step.as_object_mut().and_then(|s| s.remove("padTo")) {
                    let body = step["body"].as_str().unwrap().to_owned();
                    let missing = pad_to.as_u64().unwrap() as usize - body.len();
                    step["body"] = json!(format!("{body}{}", " ".repeat(missing)));
                }
                step
            })
            .collect();
        out.insert(route.clone(), Value::Array(steps));
    }
    out
}

async fn start(routes: &Value, dead: bool) -> Servers {
    let log = RequestLog::default();
    let mut servers = Vec::new();
    let mut map = BTreeMap::new();
    for name in routes.as_object().expect("серверы").keys() {
        let server = http::start_server(name, log.clone()).await;
        map.insert(name.clone(), server.authority.clone());
        servers.push(server);
    }
    if dead {
        map.insert(
            "dead".to_owned(),
            format!("127.0.0.1:{}", http::dead_port().await),
        );
    }
    for server in &servers {
        let filled = http::fill(&routes[&server.name], &map);
        *server.routes.lock().unwrap() = materialize(&filled);
    }
    Servers {
        _servers: servers,
        map,
        log,
    }
}

/// `[server, method, url, user-agent]` по порядку запросов.
fn request_lines(servers: &Servers) -> Value {
    let lines: Vec<Value> = servers
        .log
        .lock()
        .unwrap()
        .iter()
        .map(|entry| {
            json!([
                entry["server"],
                entry["method"],
                entry["url"],
                entry["headers"]["user-agent"]
                    .as_str()
                    .map_or(Value::Null, |ua| json!(ua)),
            ])
        })
        .collect();
    http::unfill(&Value::Array(lines), &servers.map)
}

// ------------------------------------------------------------ классы ошибок

/// `classify` из gate.js: класс ошибки и текст — только у своих сообщений.
fn classify(message: &str) -> (String, Option<String>) {
    let own = |kind: &str| (kind.to_owned(), Some(message.to_owned()));
    if let Some(status) = message.strip_prefix("HTTP ")
        && !status.is_empty()
        && status.bytes().all(|b| b.is_ascii_digit())
    {
        return (format!("status:{status}"), Some(message.to_owned()));
    }
    if message.starts_with("URL вне allowlist") {
        return own("not-allowed");
    }
    match message {
        "слишком много редиректов" => return own("redirects"),
        "ответ слишком большой" => return own("too-large"),
        "таймаут" => return own("timeout"),
        "каналы обновления лаунчера недоступны" => {
            return own("unavailable");
        }
        "The operation was aborted" => return own("aborted"),
        _ => {}
    }
    if message.starts_with("Не SemVer: ") {
        return own("not-semver");
    }
    if message.starts_with("не JSON: ") {
        return ("not-json".to_owned(), None);
    }
    ("network".to_owned(), None)
}

fn unfill_str(text: &str, map: &BTreeMap<String, String>) -> String {
    http::unfill(&json!(text), map).as_str().unwrap().to_owned()
}

/// Каталог временных файлов в тексте — `{dir}`, разделитель после него всегда «/» (фикстура
/// одна для Windows и Linux).
fn mask_dir(value: &Value, dir: &str) -> Value {
    match value {
        Value::String(text) => {
            Value::String(text.replace(dir, "{dir}").replace("{dir}\\", "{dir}/"))
        }
        Value::Array(list) => Value::Array(list.iter().map(|item| mask_dir(item, dir)).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, item)| (key.clone(), mask_dir(item, dir)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Ошибка шлюза в форме фикстуры; класс из `kind()` обязан совпасть с классом по тексту.
fn gate_error(error: &GateError, map: &BTreeMap<String, String>) -> Value {
    let message = error.to_string();
    let (kind, own) = classify(&message);
    assert_eq!(error.kind(), kind, "класс ошибки «{message}»");
    json!({"error": {
        "kind": kind,
        "message": own.map(|text| unfill_str(&text, map)),
    }})
}

/// Строка журнала шлюза: чужие тексты ошибок заменены метками.
fn mask_line(message: &str) -> String {
    for prefix in [
        "[launcher-update] канал недоступен (",
        "[launcher-update] канал не ответил на справку (",
    ] {
        let Some(rest) = message.strip_prefix(prefix) else {
            continue;
        };
        let Some(end) = rest.find("): ") else {
            continue;
        };
        let (head, tail) = (&message[..prefix.len() + end + 3], &rest[end + 3..]);
        let (kind, own) = classify(tail);
        let text = match own {
            Some(text) => text,
            None if kind == "not-json" => "<json>".to_owned(),
            None => "<network>".to_owned(),
        };
        return format!("{head}{text}");
    }
    message.to_owned()
}

/// «latest.yml не подтверждён: <текст>»: чужой текст сетевой ошибки заменён меткой.
fn mask_problem(problem: &str) -> String {
    const PREFIX: &str = "latest.yml не подтверждён: ";
    let Some(rest) = problem.strip_prefix(PREFIX) else {
        return problem.to_owned();
    };
    if rest.starts_with("SHA-256 latest.yml не совпал") || rest == "недоступен" {
        return problem.to_owned();
    }
    match classify(rest) {
        (_, None) => format!("{PREFIX}<network>"),
        _ => problem.to_owned(),
    }
}

fn mask_outcome(mut outcome: Value) -> Value {
    if let Some(problems) = outcome["problems"].as_array_mut() {
        for problem in problems {
            *problem = json!(mask_problem(problem.as_str().unwrap()));
        }
    }
    outcome
}

// ------------------------------------------------------------ шлюз

const INSTALLER_KINDS: [&str; 4] = ["good", "bad", "short", "missing"];

fn installer_bytes(kind: &str) -> Option<Vec<u8>> {
    let spec = &FIXTURE["installer"];
    let full = str_of(&spec["repeat"])
        .repeat(spec["times"].as_u64().unwrap() as usize)
        .into_bytes();
    assert!(INSTALLER_KINDS.contains(&kind), "файл {kind}");
    match kind {
        "good" => Some(full),
        "bad" => {
            let mut bytes = full;
            *bytes.last_mut().unwrap() = b'!';
            Some(bytes)
        }
        "short" => Some(full[..full.len() - 10].to_vec()),
        _ => None,
    }
}

async fn run_gate_case(case: &Value, dir: &std::path::Path) -> Value {
    let servers = start(&case["routes"], case["dead"].as_bool() == Some(true)).await;
    let map = &servers.map;
    let target = platform(str_of(&case["platform"]));
    let first = if case["dead"].as_bool() == Some(true) {
        "dead"
    } else {
        "p"
    };
    let yml = if target == Platform::Linux {
        "latest-linux.yml"
    } else {
        "latest.yml"
    };
    let logger = Arc::new(MemoryLogger::default());
    let mut options = GateOptions::new(
        str_of(&case["currentVersion"]),
        Arc::new(ReqwestGet::with_allow(allow_loopback)),
    );
    options.channel_urls = vec![
        format!("http://{}/launcher-channel.json", map[first]),
        format!("http://{}/launcher-channel.json", map["f"]),
    ];
    options.latest_yml_urls = vec![
        format!("http://{}/{yml}", map["p"]),
        format!("http://{}/{yml}", map["f"]),
    ];
    options.public_keys = vec![str_of(&FIXTURE["keys"]["A"]).to_owned()];
    options.platform = target;
    options.logger = logger.clone();
    let gate = LauncherUpdateGate::new(options);

    let mut results = Vec::new();
    for step in items(&case["steps"]) {
        let result = match str_of(&step["op"]) {
            "evaluate" => {
                let expected = step["expected"].as_str();
                match gate.evaluate(expected).await {
                    Ok(outcome) => json!({"ok": mask_outcome(outcome.to_json())}),
                    Err(error) => gate_error(&error, map),
                }
            }
            "fetchChannel" => match gate.fetch_channel().await {
                Ok(fetched) => {
                    json!({"ok": {"manifest": fetched.manifest, "sourceUrl": fetched.source_url}})
                }
                Err(error) => gate_error(&error, map),
            },
            "probeLatest" => {
                json!({"ok": gate.probe_latest().await.map(|o| mask_outcome(o.to_json()))})
            }
            "verifyInstaller" => {
                let file = dir.join("installer.bin");
                let _ = std::fs::remove_file(&file);
                if let Some(bytes) = installer_bytes(str_of(&step["file"])) {
                    std::fs::write(&file, bytes).unwrap();
                }
                let outcome = gate
                    .verify_installer(
                        &file,
                        &VerifyInstallerOptions {
                            manifest: step.get("manifest").cloned(),
                            expected_version: step["expectedVersion"].as_str().map(str::to_owned),
                            verify: VerifyOptions::default(),
                        },
                    )
                    .await;
                json!({"ok": {
                    "ok": outcome.ok,
                    "problems": outcome.problems,
                    "code": outcome.code.map(hlsr_update::errors::LuCode::as_str),
                }})
            }
            other => panic!("шаг {other}"),
        };
        results.push(result);
    }
    let dir_text = dir.display().to_string();
    let log: Vec<Value> = logger
        .take()
        .into_iter()
        .map(|(level, message)| json!({"level": level, "message": mask_line(&message)}))
        .collect();
    let result = json!({
        "steps": results,
        "requests": request_lines(&servers),
        "log": log,
    });
    mask_dir(&http::unfill(&result, map), &dir_text)
}

#[tokio::test]
async fn gate_matches_node() {
    let dir = tempfile::tempdir().unwrap();
    let cases = items(&FIXTURE["gate"]);
    assert!(cases.len() >= 140, "в фикстуре {} случаев", cases.len());
    let mut mismatches = Mismatches::default();
    for case in cases {
        let actual = run_gate_case(case, dir.path()).await;
        mismatches.check(&actual, &case["result"], str_of(&case["id"]));
    }
    mismatches.finish("шлюз обновления");
}

// ------------------------------------------------------------ прямые GET

fn body_summary(body: &[u8]) -> Value {
    json!({
        "length": body.len(),
        "sha256": sha256_hex(body),
        "text": (body.len() <= 64).then(|| String::from_utf8_lossy(body).into_owned()),
    })
}

async fn run_get_case(case: &Value) -> Value {
    let dead = str_of(&case["url"]).contains("{dead}");
    let servers = start(&case["routes"], dead).await;
    let url = http::fill_text(str_of(&case["url"]), &servers.map);
    let client = ReqwestGet::with_allow(allow_loopback);
    let options = &case["options"];
    let repeat = case["repeat"].as_u64().unwrap_or(1);
    let mut results = Vec::new();
    for _ in 0..repeat {
        let mut get = GetOptions {
            timeout: options["timeout"].as_u64().map(Duration::from_millis),
            max_body: options["maxBody"].as_u64().map(|n| n as usize),
            cancel: None,
        };
        if let Some(after) = options["abortAfterMs"].as_u64() {
            let token = CancelToken::new();
            if after == 0 {
                token.cancel();
            } else {
                let later = token.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(after)).await;
                    later.cancel();
                });
            }
            get.cancel = Some(token);
        }
        let result = match client.get(&url, &get).await {
            Ok(body) => json!({"ok": body_summary(&body)}),
            Err(error) => {
                let message = error.to_string();
                let (kind, own) = classify(&message);
                assert_eq!(error.kind(), kind, "класс ошибки «{message}»");
                json!({"error": {"kind": kind, "message": own.map(|text| unfill_str(&text, &servers.map))}})
            }
        };
        results.push(result);
    }
    json!({"results": results, "requests": request_lines(&servers)})
}

#[tokio::test]
async fn direct_gets_match_node() {
    let cases = items(&FIXTURE["get"]);
    assert!(cases.len() >= 50, "в фикстуре {} случаев", cases.len());
    let mut mismatches = Mismatches::default();
    for case in cases {
        let actual = run_get_case(case).await;
        mismatches.check(&actual, &case["result"], str_of(&case["id"]));
    }
    mismatches.finish("httpsGet");
}

/// Запрос уходит с `user-agent: HLSR-launcher-update`, без сжатия.
#[tokio::test]
async fn the_request_carries_the_launcher_update_user_agent_and_no_compression() {
    let log = RequestLog::default();
    let server = http::start_server("a", log.clone()).await;
    server
        .routes
        .lock()
        .unwrap()
        .insert("/x".to_owned(), json!([{"body": "hello"}]));
    let url = format!("http://{}/x", server.authority);
    let body = ReqwestGet::with_allow(allow_loopback)
        .get(&url, &GetOptions::default())
        .await
        .unwrap();
    assert_eq!(body, b"hello");
    let entry = log.lock().unwrap()[0].clone();
    assert_eq!(
        entry["headers"]["user-agent"],
        json!(hlsr_update::http::USER_AGENT)
    );
    assert_eq!(hlsr_update::http::USER_AGENT, "HLSR-launcher-update");
    assert_eq!(entry["method"], "GET");
}

/// Боевой клиент не пускает ни `http`, ни чужие хосты, не сделав ни одного запроса.
#[tokio::test]
async fn the_production_client_refuses_foreign_addresses() {
    let client = ReqwestGet::production();
    for url in [
        "http://hlsr.pro/launcher-channel.json",
        "https://example.com/launcher-channel.json",
        "https://hlsr.pro.evil.com/x",
        "http://127.0.0.1:1/x",
        "https://user@hlsr.pro/x",
        "https://hlsr.pro:8443/x",
    ] {
        assert_eq!(
            client.get(url, &GetOptions::default()).await,
            Err(GetError::NotAllowed(url.to_owned())),
            "{url}"
        );
    }
}

// ------------------------------------------------------------ экран загрузки

#[tokio::test(start_paused = true)]
async fn boot_gate_matches_node() {
    let cases = items(&FIXTURE["bootGate"]);
    assert!(cases.len() >= 25, "в фикстуре {} случаев", cases.len());
    let mut mismatches = Mismatches::default();
    for case in cases {
        let host = Arc::new(ScriptedHost::new(case));
        let log = Arc::new(MemoryLogger::default());
        let timeout = Duration::from_millis(case["timeoutMs"].as_u64().unwrap());
        let max = Duration::from_millis(case["maxMs"].as_u64().unwrap());
        let decision = tokio::time::timeout(max, run_boot_gate(host.clone(), log.clone(), timeout))
            .await
            .ok();
        // Поздние события службы после решения не должны ничего менять.
        tokio::time::sleep(Duration::from_millis(250)).await;
        let actual = json!({
            "decision": decision.map(|d| d.to_json()),
            "calls": host.calls(),
            "log": log
                .take()
                .into_iter()
                .map(|(level, message)| json!({"level": level, "message": message}))
                .collect::<Vec<_>>(),
            "subscribers": host.subscribers(),
        });
        mismatches.check(&actual, &case["result"], str_of(&case["id"]));
    }
    mismatches.finish("экран загрузки");
    assert_eq!(boot_gate::DEFAULT_TIMEOUT, Duration::from_secs(10));
}

/// «mandatory boot errors block»: обязательное обновление не даёт запуститься при ошибке.
#[tokio::test(start_paused = true)]
async fn mandatory_update_errors_block_the_launch() {
    let case = items(&FIXTURE["bootGate"])
        .iter()
        .find(|case| str_of(&case["id"]) == "mandatory error on check")
        .unwrap();
    let host = Arc::new(ScriptedHost::new(case));
    let decision = run_boot_gate(
        host,
        Arc::new(MemoryLogger::default()),
        Duration::from_millis(60),
    )
    .await;
    assert_eq!(decision.action, boot_gate::BootAction::Blocked);
    assert_eq!(decision.mandatory, Some(true));
}

/// Служба, чей `check()` занимает время: состояние остаётся «проверяется». Время — часы tokio на
/// паузе (`start_paused`): `check()` сдвигает их на `block`, как блокировка потока сдвинула бы
/// настоящие, поэтому ни пауз по-настоящему, ни зависимости от нагрузки машины.
struct SlowCheck {
    block: Duration,
}

impl BootGateHost for SlowCheck {
    fn enabled(&self) -> bool {
        true
    }

    fn state(&self) -> UpdateState {
        UpdateState::Checking
    }

    fn subscribe(&self, _subscriber: Subscriber) -> Box<dyn FnOnce() + Send> {
        Box::new(|| {})
    }

    fn check(&self) {
        // Часы сдвигаются в самом начале `advance`, до первого ожидания: одного опроса хватает.
        let mut advance = std::pin::pin!(tokio::time::advance(self.block));
        let _ = advance
            .as_mut()
            .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()));
    }

    fn download(&self) {}

    fn install(&self) {}

    fn is_game_running(&self) -> bool {
        false
    }

    fn version(&self) -> Option<String> {
        None
    }

    fn error(&self) -> Option<String> {
        None
    }

    fn is_mandatory(&self) -> bool {
        false
    }
}

/// Порядок таймера как в `boot-gate.js`: `setTimeout` взводится до `host.check()`, и срок
/// отсчитывается от этого момента. `check()` на 400 мс при сроке 400 мс: решение приходит сразу
/// после возврата (400 мс на часах tokio), а не ещё через 400 мс (800 мс), если бы срок начался
/// после `check()`. Часы стоят: между 400 и 800 мс нет ни запаса на нагрузку, ни зависимости от неё.
#[tokio::test(start_paused = true)]
async fn the_check_timer_starts_before_the_check_call() {
    let host = Arc::new(SlowCheck {
        block: Duration::from_millis(400),
    });
    let started = tokio::time::Instant::now();
    let decision = run_boot_gate(
        host,
        Arc::new(MemoryLogger::default()),
        Duration::from_millis(400),
    )
    .await;
    let elapsed = started.elapsed();
    assert_eq!(decision.reason, "check-timeout");
    assert_eq!(decision.warn, Some("UPDATE_CHECK_TIMEOUT"));
    // 400 мс плюс округление срока таймера tokio вверх до миллисекунды; 800 мс — срок от возврата.
    assert!(
        elapsed >= Duration::from_millis(400) && elapsed < Duration::from_millis(600),
        "решение через {elapsed:?}: срок отсчитан от возврата из check()"
    );
}

// Логи шлюза идут через тот же `Logger`, что и остальные службы.
#[test]
fn gate_logger_is_the_shared_service_logger() {
    fn takes(_: &dyn Logger) {}
    takes(&MemoryLogger::default());
}
