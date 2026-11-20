//! Сеть доставки против 0.4.7: фикстура `network.json` из
//! `tools/reference/delivery/network.js` (модули 0.4.7 в Node 24 на тех же входах).
//! Чистые функции — на корпусе; сценарии — на локальном сервере того же формата
//! (`support/http.rs`): журнал запросов, итоги шагов, журнал зеркал, пауза хостов.
//!
//! Сценарии идут через источник `local-http` и подменённый адрес API Яндекса: оба есть
//! только в сборке с отладочными возможностями.
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_core::json_text::{parse_js_json, stringify};
use hlsr_delivery::download_settings::{self, DownloadSettings};
use hlsr_delivery::errors::{Code, DeliveryError, Failure, RawError};
use hlsr_delivery::http_client::{self, HttpClient, Method, RequestOptions, read_body};
use hlsr_delivery::mirror_runner::{
    self, Attempt, Clock, DiscardPartial, MirrorOutcome, MirrorRequest, MirrorRunner,
    TerminalResult,
};
use hlsr_delivery::ports::MemoryLogger;
use hlsr_delivery::providers::{self, Provider, ProviderOptions, yandex_public_provider};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use support::http::{RequestLog, Server, dead_port, start_server};
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("network.json"));

const YANDEX_API_PATH: &str = "/v1/disk/public/resources/download";

fn allowed() -> Vec<String> {
    items(&FIXTURE["allowed"])
        .iter()
        .map(|t| str_of(t).to_owned())
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn delivery_json(error: &DeliveryError) -> Value {
    json!({ "error": error.to_public() })
}

fn raw_json(error: &RawError) -> Value {
    json!({ "raw": { "code": error.code, "message": error.message } })
}

fn failure_json(failure: &Failure) -> Value {
    match failure {
        Failure::Delivery(error) => delivery_json(error),
        Failure::Raw(error) => raw_json(error),
    }
}

fn ok(value: Value) -> Value {
    json!({ "ok": value })
}

/// Ошибка из фикстуры: `{delivery: {code, message}}` или `{raw: {code, message}}`.
fn failure_from(input: &Value) -> Failure {
    if let Some(delivery) = input.get("delivery") {
        let code = Code::from_name(str_of(&delivery["code"])).expect("код из CODES");
        return DeliveryError::new(code, str_of(&delivery["message"])).into();
    }
    let raw = &input["raw"];
    RawError {
        code: raw["code"].as_str().map(str::to_owned),
        message: str_of(&raw["message"]).to_owned(),
        ..RawError::default()
    }
    .into()
}

// ---------------------------------------------------------------- чистые функции

#[test]
fn constants_match_0_4_7() {
    let c = &FIXTURE["constants"];
    assert_eq!(
        c["DEFAULT_TIMEOUT_MS"],
        json!(http_client::DEFAULT_TIMEOUT_MS)
    );
    assert_eq!(c["MAX_REDIRECTS"], json!(http_client::MAX_REDIRECTS));
    assert_eq!(
        c["HTTP_AGENT_TIMEOUT_MS"],
        json!(http_client::CONNECT_TIMEOUT_MS)
    );
    assert_eq!(
        c["HTTPS_AGENT_TIMEOUT_MS"],
        json!(http_client::CONNECT_TIMEOUT_MS)
    );
    assert_eq!(
        c["DEFAULT_COOLDOWN_MS"],
        json!(mirror_runner::DEFAULT_COOLDOWN_MS)
    );
    assert_eq!(c["MAX_COOLDOWN_MS"], json!(mirror_runner::MAX_COOLDOWN_MS));
    let names = |codes: &[Code]| json!(codes.iter().map(|c| c.as_str()).collect::<Vec<_>>());
    assert_eq!(c["SWITCH_CODES"], names(mirror_runner::SWITCH_CODES));
    assert_eq!(c["STOP_CODES"], names(mirror_runner::STOP_CODES));
    assert_eq!(
        c["DISCARD_PARTIAL_CODES"],
        names(mirror_runner::DISCARD_PARTIAL_CODES)
    );
    assert_eq!(
        c["YANDEX_API_BASE"],
        json!(yandex_public_provider::API_BASE)
    );
    assert_eq!(http_client::USER_AGENT, "HLSR-Launcher");
}

#[test]
fn classify_matches_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["classify"]) {
        // PACK_CORRUPT в 0.4.9 убран из CODES: случая для него нет.
        if case["input"]["delivery"]["code"] == "PACK_CORRUPT" {
            continue;
        }
        let failure = failure_from(&case["input"]);
        let verdict = mirror_runner::classify(&failure);
        let actual = json!({
            "verdict": { "action": verdict.action.as_str(), "code": verdict.code.as_str() },
            "error": mirror_runner::to_delivery(failure).to_public(),
        });
        let expected = json!({ "verdict": case["verdict"], "error": case["error"] });
        m.check(&actual, &expected, format!("classify({})", case["input"]));
    }
    m.finish("classify");
}

#[test]
fn parse_retry_after_matches_0_4_7() {
    let now = FIXTURE["now"].as_i64().unwrap();
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["retryAfter"]) {
        let value = case[0].as_str();
        let actual = mirror_runner::parse_retry_after(value, now);
        m.check_eq(
            actual,
            case[1].as_i64(),
            format!("parseRetryAfter({value:?})"),
        );
    }
    m.finish("parseRetryAfter");
}

#[test]
fn host_and_authority_match_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["hosts"]) {
        let descriptor = match case["url"].as_str() {
            Some(url) => json!({ "id": "x", "url": url }),
            None => json!({ "id": "x" }),
        };
        m.check_eq(
            mirror_runner::host_of(&descriptor),
            case["host"].as_str().map(str::to_owned),
            format!("hostOf({})", case["url"]),
        );
        m.check_eq(
            mirror_runner::authority_of(&descriptor),
            case["authority"].as_str().map(str::to_owned),
            format!("authorityOf({})", case["url"]),
        );
    }
    m.finish("hostOf/authorityOf");
}

#[test]
fn loopback_and_redirect_rules_match_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["loopback"]) {
        let host = str_of(&case[0]);
        m.check_eq(
            http_client::is_loopback(host),
            case[1].as_bool().unwrap(),
            format!("isLoopback({host:?})"),
        );
    }
    for case in items(&FIXTURE["redirects"]) {
        let from = url::Url::parse(str_of(&case["from"])).unwrap();
        let to = url::Url::parse(str_of(&case["to"])).unwrap();
        let actual = match http_client::check_redirect(&from, &to) {
            Ok(()) => ok(Value::Null),
            Err(error) => delivery_json(&error),
        };
        m.check(
            &actual,
            &case["result"],
            format!("checkRedirect({from} → {to})"),
        );
    }
    m.finish("isLoopback/checkRedirect");
}

#[test]
fn speed_limits_match_0_4_7() {
    let speed = &FIXTURE["speed"];
    assert_eq!(speed["limits"], json!(download_settings::SPEED_LIMITS));
    let mut m = Mismatches::default();
    for case in items(&speed["values"]) {
        m.check_eq(
            (
                download_settings::valid_speed_limit(&case[0]),
                download_settings::normalize_speed_limit(&case[0]),
            ),
            (case[1].as_bool().unwrap(), case[2].as_u64().unwrap() as u32),
            format!("speedLimit {}", case[0]),
        );
    }
    m.finish("validSpeedLimit/normalizeSpeedLimit");
}

#[test]
fn download_settings_file_matches_0_4_7() {
    let root = tempfile::tempdir().unwrap();
    let mut m = Mismatches::default();
    for (index, case) in items(&FIXTURE["settings"]).iter().enumerate() {
        let file = root
            .path()
            .join(index.to_string())
            .join("download-settings.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        match &case["initial"] {
            Value::Null => {}
            Value::String(dir) if dir == "<dir>" => std::fs::create_dir(&file).unwrap(),
            Value::String(text) => std::fs::write(&file, text).unwrap(),
            other => panic!("начальный файл {other}"),
        }
        let context = format!("download-settings.json = {}", case["initial"]);
        let mut settings = DownloadSettings::new(&file);
        m.check(&settings.snapshot().to_json(), &case["snapshot"], &context);
        m.check_eq(
            settings.rate_limiter().bytes_per_second(),
            case["limit"].as_f64().unwrap(),
            format!("{context}: лимит"),
        );
        for save in items(&case["saves"]) {
            let result = match settings.save(&save["value"]) {
                Ok(snapshot) => {
                    let mut out = json!({ "ok": true });
                    out.as_object_mut()
                        .unwrap()
                        .extend(snapshot.to_json().as_object().unwrap().clone());
                    out
                }
                Err(error) => json!({ "ok": false, "code": error.code() }),
            };
            let context = format!("{context}: save({})", save["value"]);
            m.check(&result, &save["result"], &context);
            let text = std::fs::read(&file)
                .ok()
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
            m.check_eq(
                text.as_deref(),
                save["file"].as_str(),
                format!("{context}: файл"),
            );
            m.check_eq(
                settings.rate_limiter().bytes_per_second(),
                save["limit"].as_f64().unwrap(),
                format!("{context}: лимит"),
            );
        }
        m.check(
            &DownloadSettings::new(&file).snapshot().to_json(),
            &case["reread"],
            format!("{context}: после перезапуска"),
        );
    }
    m.finish("DownloadSettings");
}

#[test]
fn provider_rules_match_0_4_7() {
    let http = HttpClient::new();
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["providers"]) {
        let types: Vec<String> = items(&case["allowed"])
            .iter()
            .map(|t| str_of(t).to_owned())
            .collect();
        let options = ProviderOptions::with_allowed_provider_types(http.clone(), types);
        let descriptor = &case["descriptor"];
        let actual = match providers::create(descriptor, &options) {
            Err(error) => delivery_json(&error),
            Ok(provider) => {
                let mut out = Map::new();
                out.insert("describe".into(), provider.describe());
                out.insert("supportsRange".into(), json!(provider.supports_range()));
                if let Some(url) = provider.url() {
                    out.insert("url".into(), json!(url));
                }
                if descriptor["type"] == "yandex-public" {
                    let yandex =
                        yandex_public_provider::YandexPublicProvider::new(descriptor, &options)
                            .unwrap();
                    out.insert("apiUrl".into(), json!(yandex.build_api_url()));
                }
                let refresh: Vec<u16> = [200, 206, 304, 401, 403, 404, 410, 429, 500]
                    .into_iter()
                    .filter(|&s| provider.should_refresh_on(s))
                    .collect();
                out.insert("refresh".into(), json!(refresh));
                ok(Value::Object(out))
            }
        };
        m.check(
            &actual,
            &case["result"],
            format!("create({descriptor}, {})", case["allowed"]),
        );
        if let Some(url) = descriptor["url"].as_str() {
            m.check_eq(
                hlsr_delivery::delivery_endpoints::is_allowed_download_url(url),
                case["allowedDownloadUrl"].as_bool().unwrap(),
                format!("isAllowedDownloadUrl({url})"),
            );
        }
    }
    m.finish("providers.create");
}

#[test]
fn resolve_sibling_matches_0_4_7() {
    let options = ProviderOptions::with_allowed_provider_types(HttpClient::new(), allowed());
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["siblings"]) {
        let descriptor = match case["url"].as_str() {
            Some(url) if url.starts_with("https:") => {
                json!({ "id": "s", "type": "https-direct", "url": url })
            }
            Some(url) => json!({ "id": "s", "type": "local-http", "url": url }),
            None => {
                json!({ "id": "y", "type": "yandex-public", "publicUrl": "https://disk.yandex.ru/d/x" })
            }
        };
        let provider = providers::create(&descriptor, &options).unwrap();
        let name = str_of(&case["name"]);
        let actual = match provider.resolve_sibling(name) {
            Ok(resolved) => ok(json!({ "url": resolved.url, "headers": {} })),
            Err(error) => delivery_json(&error),
        };
        m.check(
            &actual,
            &case["result"],
            format!("{}: resolveSibling({name:?})", case["url"]),
        );
    }
    m.finish("resolveSibling");
}

// ---------------------------------------------------------------- сценарии

/// Подстановка адресов: `{имя}` → `127.0.0.1:<порт>` во всех строках и ключах.
fn fill(value: &Value, map: &BTreeMap<String, String>) -> Value {
    match value {
        Value::String(text) => Value::String(fill_text(text, map)),
        Value::Array(items) => Value::Array(items.iter().map(|v| fill(v, map)).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(k, v)| (fill_text(k, map), fill(v, map)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn fill_text(text: &str, map: &BTreeMap<String, String>) -> String {
    let mut out = text.to_owned();
    for (name, authority) in map {
        out = out.replace(&format!("{{{name}}}"), authority);
    }
    out
}

/// Обратная замена: `127.0.0.1:<порт>` → `{имя}`; длинные адреса первыми
/// (`127.0.0.1:4000` — начало `127.0.0.1:40001`).
fn unfill(value: &Value, map: &BTreeMap<String, String>) -> Value {
    let mut pairs: Vec<(&String, &String)> = map.iter().collect();
    pairs.sort_by_key(|(_, authority)| std::cmp::Reverse(authority.len()));
    let text = |t: &str| {
        pairs.iter().fold(t.to_owned(), |t, (name, authority)| {
            t.replace(*authority, &format!("{{{name}}}"))
        })
    };
    match value {
        Value::String(t) => Value::String(text(t)),
        Value::Array(items) => Value::Array(items.iter().map(|v| unfill(v, map)).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(k, v)| (text(k), unfill(v, map)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Что сравнивается не побайтно (записано в отчёте задачи):
/// - текст ошибки `JSON.parse` V8 после «Ответ не является JSON: » — в Rust текст serde_json;
/// - числа ответа JSON — как их видит JS (через f64).
fn normalize(value: &Value) -> Value {
    const JSON_PREFIX: &str = "Ответ не является JSON: ";
    match value {
        Value::String(text) if text.starts_with(JSON_PREFIX) => {
            Value::String(format!("{JSON_PREFIX}<текст JSON.parse>"))
        }
        Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(k, v)| (k.clone(), normalize(v)))
                .collect(),
        ),
        Value::Number(_) => parse_js_json(&stringify(value, "")).unwrap(),
        other => other.clone(),
    }
}

/// Журнал запросов Rust: reqwest всегда шлёт `Accept: */*`, Node — нет (отчёт задачи).
fn without_default_accept(log: &[Value]) -> Vec<Value> {
    log.iter()
        .map(|entry| {
            let mut entry = entry.clone();
            let headers = entry["headers"].as_object_mut().unwrap();
            if headers.get("accept") == Some(&json!("*/*")) {
                headers.remove("accept");
            }
            entry
        })
        .collect()
}

struct Context {
    http: HttpClient,
    runner: MirrorRunner,
    logger: Arc<MemoryLogger>,
    clock: Arc<AtomicI64>,
    api_base: Option<String>,
}

impl Context {
    fn provider_options(&self) -> ProviderOptions {
        let options = ProviderOptions::with_allowed_provider_types(self.http.clone(), allowed());
        match &self.api_base {
            Some(base) => options.yandex_api_base(base.clone()),
            None => options,
        }
    }
}

const PICKED_HEADERS: [&str; 7] = [
    "content-type",
    "content-length",
    "location",
    "retry-after",
    "x-multi",
    "etag",
    "x-latin",
];

fn request_options(step: &Value) -> RequestOptions {
    RequestOptions {
        method: if step["method"] == "HEAD" {
            Method::Head
        } else {
            Method::Get
        },
        headers: step["headers"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), str_of(v).to_owned()))
            .collect(),
        timeout: step["timeout"].as_u64().map(Duration::from_millis),
    }
}

async fn request_step(context: &Context, step: &Value) -> Value {
    let response = match context
        .http
        .request(str_of(&step["url"]), &request_options(step))
        .await
    {
        Ok(response) => response,
        Err(failure) => return failure_json(&failure),
    };
    let mut headers = Map::new();
    for name in PICKED_HEADERS {
        if let Some(value) = response.header(name) {
            headers.insert(name.to_owned(), json!(value));
        }
    }
    let head = json!({
        "status": response.status(),
        "url": response.url().as_str(),
        "headers": headers,
    });
    let limit = step["limit"].as_u64().map(|l| l as usize);
    let body = match read_body(response, limit, None).await {
        Ok(data) => ok(json!({
            "size": data.len(),
            "sha256": sha256(&data),
            "text": (data.len() <= 64).then(|| String::from_utf8_lossy(&data).into_owned()),
        })),
        Err(error) => delivery_json(&error),
    };
    let mut out = head;
    out["body"] = body;
    ok(out)
}

/// Операция «fetch»: проверка ответа как в `GameDownloadManager.attempt` (без Range и файлов).
async fn fetch_operation(
    context: &Context,
    descriptor: &Value,
    operation: &Value,
) -> Result<Value, Failure> {
    let provider = providers::create(descriptor, &context.provider_options())?;
    let resolved = provider.resolve().await?;
    let options = RequestOptions {
        headers: resolved.headers.clone(),
        ..RequestOptions::default()
    };
    let response = context.http.request(&resolved.url, &options).await?;
    let status = response.status();
    if status == 416 {
        return Err(DeliveryError::new(
            Code::RangeNotSupported,
            "Сервер отклонил диапазон, файл начнётся заново",
        )
        .into());
    }
    if provider.should_refresh_on(status) {
        return Err(DeliveryError::new(
            Code::ProviderForbidden,
            format!("Источник ответил {status} — нужна новая ссылка"),
        )
        .into());
    }
    if status != 200 && status != 206 {
        let retryable = status >= 500 || status == 429 || status == 408;
        let code = if retryable {
            Code::NetworkError
        } else {
            Code::HttpError
        };
        return Err(
            DeliveryError::new(code, format!("Источник ответил HTTP {status}"))
                .with_details(json!({
                    "status": status,
                    "retryAfter": response.header("retry-after").filter(|v| !v.is_empty()),
                }))
                .into(),
        );
    }
    let body = read_body(response, Some(1024 * 1024), None).await?;
    if let Some(size) = operation["size"].as_u64()
        && body.len() as u64 != size
    {
        return Err(DeliveryError::new(
            Code::SizeMismatch,
            format!("Получено {} байт вместо {size}", body.len()),
        )
        .into());
    }
    if let Some(expected) = operation["sha256"].as_str()
        && sha256(&body) != expected
    {
        return Err(DeliveryError::new(Code::ChecksumMismatch, "SHA-256 не совпал").into());
    }
    Ok(json!({ "status": "completed", "size": body.len(), "sha256": sha256(&body) }))
}

/// Операция «sibling»: файл рядом с архивом, как `loadPatchManifest`.
async fn sibling_operation(
    context: &Context,
    descriptor: &Value,
    operation: &Value,
) -> Result<Value, Failure> {
    let provider = providers::create(descriptor, &context.provider_options())?;
    let sibling = provider.resolve_sibling(str_of(&operation["filename"]))?;
    let options = RequestOptions {
        headers: sibling.headers.clone(),
        ..RequestOptions::default()
    };
    let response = context.http.request(&sibling.url, &options).await?;
    if response.status() != 200 {
        let status = response.status();
        return Err(DeliveryError::new(
            Code::ManifestNotFound,
            format!("Манифест патча недоступен: HTTP {status}"),
        )
        .with_details(json!({
            "status": status,
            "retryAfter": response.header("retry-after").filter(|v| !v.is_empty()),
        }))
        .into());
    }
    let body = read_body(response, Some(8 * 1024 * 1024), None).await?;
    Ok(json!(String::from_utf8_lossy(&body)))
}

fn script_result(result: &Value) -> Result<Value, Failure> {
    if let Some(value) = result.get("value") {
        return Ok(value.clone());
    }
    if let Some(error) = result.get("error") {
        let code = Code::from_name(str_of(&error["code"])).unwrap();
        let mut delivery = DeliveryError::new(code, str_of(&error["message"]));
        if let Some(details) = error.get("details") {
            delivery = delivery.with_details(details.clone());
        }
        return Err(delivery.into());
    }
    Err(RawError {
        code: result["raw"]["code"].as_str().map(str::to_owned),
        message: str_of(&result["raw"]["message"]).to_owned(),
        ..RawError::default()
    }
    .into())
}

fn outcome_json(outcome: &MirrorOutcome<Value>) -> Value {
    json!({
        "value": outcome.value,
        "providerId": outcome.provider_id,
        "usedFallback": outcome.used_fallback,
        "attempts": outcome.attempts.iter().map(|a| a.to_json()).collect::<Vec<_>>(),
    })
}

async fn mirror_step(context: &Context, step: &Value) -> Value {
    let calls = Mutex::new(Vec::new());
    let discards = Mutex::new(Vec::new());
    let scripted = Mutex::new(0usize);
    let operation = &step["operation"];
    let descriptors = items(&step["descriptors"]).clone();
    let terminal = |value: &Value| {
        hlsr_core::js::truthy(value) && value.get("status") != Some(&json!("completed"))
    };
    let discard = |descriptor: &Value, code: Code| -> Result<(), Failure> {
        discards
            .lock()
            .unwrap()
            .push(json!({ "id": descriptor["id"], "code": code.as_str() }));
        if step["discardPartial"] == "throw" {
            return Err(RawError::new("файл занят").into());
        }
        Ok(())
    };
    let request = MirrorRequest {
        artifact: str_of(&step["artifact"]),
        descriptors: &descriptors,
        is_terminal_result: step["terminal"]
            .as_bool()
            .unwrap_or(false)
            .then_some(&terminal as &TerminalResult<Value>),
        on_discard_partial: step
            .get("discardPartial")
            .is_some()
            .then_some(&discard as &DiscardPartial),
    };
    let result = context
        .runner
        .run(request, |attempt: Attempt| {
            calls.lock().unwrap().push(json!({
                "index": attempt.index,
                "id": attempt.descriptor.get("id").cloned().unwrap_or(Value::Null),
                "isFallback": attempt.is_fallback,
            }));
            let next = {
                let mut scripted = scripted.lock().unwrap();
                *scripted += usize::from(operation["kind"] == "script");
                *scripted
            };
            async move {
                match operation["kind"].as_str().unwrap() {
                    "fetch" => fetch_operation(context, &attempt.descriptor, operation).await,
                    "sibling" => sibling_operation(context, &attempt.descriptor, operation).await,
                    _ => script_result(&operation["results"][next - 1]),
                }
            }
        })
        .await;
    let result = match result {
        Ok(outcome) => ok(outcome_json(&outcome)),
        Err(error) => delivery_json(&error),
    };
    json!({
        "calls": calls.into_inner().unwrap(),
        "discards": discards.into_inner().unwrap(),
        "result": result,
    })
}

async fn run_step(context: &Context, step: &Value) -> Value {
    match str_of(&step["kind"]) {
        "request" => request_step(context, step).await,
        "getJson" => {
            let options = request_options(step);
            let limit = step["limitBytes"].as_u64().map(|l| l as usize);
            match context
                .http
                .get_json(str_of(&step["url"]), &options, limit)
                .await
            {
                Ok(value) => ok(value),
                Err(failure) => failure_json(&failure),
            }
        }
        "yandex" => {
            let descriptor =
                json!({ "id": "y", "type": "yandex-public", "publicUrl": step["publicUrl"] });
            let provider = yandex_public_provider::YandexPublicProvider::new(
                &descriptor,
                &context.provider_options(),
            )
            .unwrap();
            match provider.resolve().await {
                Ok(resolved) => ok(json!({
                    "url": resolved.url,
                    "headers": {},
                    "temporary": resolved.temporary,
                })),
                Err(failure) => failure_json(&failure),
            }
        }
        "mirror" => mirror_step(context, step).await,
        "parallel" => {
            let runs = items(&step["runs"]).iter().map(|run| async move {
                if let Some(ms) = run["startAfterMs"].as_u64() {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                }
                mirror_step(context, run).await
            });
            Value::Array(join_all(runs).await)
        }
        "clock" => {
            let now = context
                .clock
                .fetch_add(step["advanceMs"].as_i64().unwrap(), Ordering::SeqCst)
                + step["advanceMs"].as_i64().unwrap();
            json!({ "now": now })
        }
        "cooldown" => Value::Object(
            context
                .runner
                .cooldown()
                .snapshot()
                .into_iter()
                .map(|(host, left)| (host, json!(left)))
                .collect(),
        ),
        "timed" => {
            // Время шага в целых секундах: тайм-аут соединения — 5 с, а не тайм-аут запроса.
            let started = std::time::Instant::now();
            let result = Box::pin(run_step(context, &step["step"])).await;
            let elapsed = started.elapsed().as_millis() as f64 / 1000.0;
            json!({ "elapsedS": elapsed.round() as u64, "result": result })
        }
        other => panic!("неизвестный шаг {other}"),
    }
}

/// Все будущие разом, итоги — в исходном порядке.
async fn join_all<F: Future<Output = Value>>(futures: impl Iterator<Item = F>) -> Vec<Value> {
    let tasks: Vec<_> = futures.map(Box::pin).collect();
    let mut results: Vec<Option<Value>> = vec![None; tasks.len()];
    let mut tasks: Vec<_> = tasks.into_iter().map(Some).collect();
    std::future::poll_fn(|cx| {
        for (task, result) in tasks.iter_mut().zip(results.iter_mut()) {
            if let Some(future) = task
                && let std::task::Poll::Ready(value) = future.as_mut().poll(cx)
            {
                *result = Some(value);
                *task = None;
            }
        }
        if tasks.iter().all(Option::is_none) {
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    })
    .await;
    results.into_iter().map(Option::unwrap).collect()
}

async fn run_scenario(scenario: &Value) -> Mismatches {
    let name = str_of(&scenario["name"]);
    let log = RequestLog::default();
    let mut map = BTreeMap::new();
    map.insert(
        "dead".to_owned(),
        format!("127.0.0.1:{}", dead_port().await),
    );
    #[cfg(target_os = "linux")]
    let mut holes = Vec::new();
    #[cfg(target_os = "linux")]
    for hole_name in scenario["blackholes"].as_array().into_iter().flatten() {
        let hole = support::http::blackhole().await;
        map.insert(str_of(hole_name).to_owned(), hole.authority.clone());
        holes.push(hole);
    }
    #[cfg(not(target_os = "linux"))]
    assert!(
        scenario.get("blackholes").is_none(),
        "«чёрные дыры» есть только на Linux"
    );
    let mut servers: Vec<Server> = Vec::new();
    for server_name in scenario["servers"].as_object().unwrap().keys() {
        let server = start_server(server_name, log.clone()).await;
        map.insert(server_name.clone(), server.authority.clone());
        servers.push(server);
    }
    for server in &servers {
        let routes = fill(&scenario["servers"][&server.name], &map);
        *server.routes.lock().unwrap() = routes.as_object().unwrap().clone();
    }

    let clock = Arc::new(AtomicI64::new(FIXTURE["now"].as_i64().unwrap()));
    let logger = Arc::new(MemoryLogger::default());
    let now: Clock = {
        let clock = clock.clone();
        Arc::new(move || clock.load(Ordering::SeqCst))
    };
    let context = Context {
        http: HttpClient::new(),
        runner: MirrorRunner::with_clock(logger.clone(), now),
        logger,
        clock,
        api_base: map.get("y").map(|y| format!("http://{y}{YANDEX_API_PATH}")),
    };

    let mut m = Mismatches::default();
    assert_eq!(
        items(&scenario["steps"]).len(),
        items(&scenario["results"]).len(),
        "шагов и результатов поровну"
    );
    for (index, (step, expected)) in items(&scenario["steps"])
        .iter()
        .zip(items(&scenario["results"]))
        .enumerate()
    {
        let result = run_step(&context, &fill(step, &map)).await;
        let logs: Vec<Value> = context
            .logger
            .take()
            .into_iter()
            .map(|(level, message)| json!({ "level": level, "message": message }))
            .collect();
        let actual = unfill(&json!({ "result": result, "logs": logs }), &map);
        m.check(
            &normalize(&actual),
            &normalize(expected),
            format!("{name}, шаг {index}: {step}"),
        );
    }
    let requests = unfill(&json!(without_default_accept(&log.lock().unwrap())), &map);
    m.check(
        &requests,
        &scenario["requests"],
        format!("{name}: журнал запросов"),
    );
    m
}

fn scenario(name: &str) -> &'static Value {
    items(&FIXTURE["scenarios"])
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("нет сценария {name}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redirects_match_0_4_7() {
    run_scenario(scenario("redirects"))
        .await
        .finish("redirects");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bodies_timeouts_and_get_json_match_0_4_7() {
    run_scenario(scenario("bodies")).await.finish("bodies");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn yandex_provider_matches_0_4_7() {
    run_scenario(scenario("yandex")).await.finish("yandex");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mirror_switch_and_stop_match_0_4_7() {
    run_scenario(scenario("mirrors")).await.finish("mirrors");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_cooldown_matches_0_4_7() {
    run_scenario(scenario("cooldown")).await.finish("cooldown");
}

/// Только Linux: «чёрная дыра» держится на очереди приёма, которую Linux не расширяет.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_timeout_matches_0_4_7() {
    run_scenario(scenario("connect-timeout"))
        .await
        .finish("connect-timeout");
}

#[test]
fn every_scenario_is_checked() {
    let names: Vec<&str> = items(&FIXTURE["scenarios"])
        .iter()
        .map(|s| str_of(&s["name"]))
        .collect();
    assert_eq!(
        names,
        [
            "redirects",
            "bodies",
            "yandex",
            "mirrors",
            "cooldown",
            "connect-timeout"
        ]
    );
}
