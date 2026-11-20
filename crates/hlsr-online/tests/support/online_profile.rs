//! Помощники тестов службы профиля: ручные таймеры (шаги `fire`/`tick` сценария), транспорт из
//! замыкания (самотесты 0.4.7 с `request: async (url, options) => …`), транспорт сценария (счётчик
//! запросов в пути и speedrun.com на втором двойнике), свёртка снимков в хеш и нормализация журнала
//! — как в `tools/reference/online/online-profile-service.js`.

#![allow(dead_code)]

use hlsr_core::json_text::{js_number, stringify};
use hlsr_delivery::providers::BoxFuture;
use hlsr_online::online_profile::online_profile_service::{Timer, Timers};
use hlsr_online::telemetry::transport::{
    Headers, HttpTransport, RequestOptions, Response, Transport, TransportError,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// События сценария по порядку: колбэки службы, ссылки, билеты, таймеры.
pub type Events = Arc<Mutex<Vec<Value>>>;

pub fn push(events: &Events, event: Value) {
    events.lock().unwrap().push(event);
}

pub fn take(events: &Events) -> Vec<Value> {
    std::mem::take(&mut *events.lock().unwrap())
}

enum Task {
    Once(Option<Box<dyn FnOnce() + Send>>),
    Repeat(Arc<dyn Fn() + Send + Sync>),
}

struct Entry {
    id: u64,
    kind: &'static str,
    delay: f64,
    task: Task,
}

#[derive(Default)]
struct TimersState {
    list: Vec<Entry>,
    next: u64,
}

/// Таймеры, которые срабатывают только по команде теста; постановка и снятие живого таймера —
/// события `["timeout"|"interval", delay]` и `["cleared", kind, delay]`.
#[derive(Clone)]
pub struct ManualTimers {
    state: Arc<Mutex<TimersState>>,
    events: Events,
}

impl ManualTimers {
    pub fn new(events: Events) -> ManualTimers {
        ManualTimers {
            state: Arc::default(),
            events,
        }
    }

    fn add(&self, kind: &'static str, delay: f64, task: Task) -> Box<dyn Timer> {
        let id = {
            let mut state = self.state.lock().unwrap();
            state.next += 1;
            let id = state.next;
            state.list.push(Entry {
                id,
                kind,
                delay,
                task,
            });
            id
        };
        push(&self.events, json!([kind, js_number(delay)]));
        Box::new(ManualTimer {
            id,
            timers: self.clone(),
        })
    }

    /// Сработать последнему живому `setTimeout`; `false` — таймеров нет.
    pub fn fire_timeout(&self) -> bool {
        let task = {
            let mut state = self.state.lock().unwrap();
            let Some(at) = state.list.iter().rposition(|entry| entry.kind == "timeout") else {
                return false;
            };
            match state.list.remove(at).task {
                Task::Once(task) => task,
                Task::Repeat(_) => None,
            }
        };
        if let Some(task) = task {
            task();
        }
        true
    }

    /// Сработать последнему живому `setInterval` (он остаётся); `false` — таймеров нет.
    pub fn fire_interval(&self) -> bool {
        let task = {
            let state = self.state.lock().unwrap();
            let Some(entry) = state
                .list
                .iter()
                .rev()
                .find(|entry| entry.kind == "interval")
            else {
                return false;
            };
            match &entry.task {
                Task::Repeat(task) => task.clone(),
                Task::Once(_) => return false,
            }
        };
        task();
        true
    }

    /// Живые таймеры: `[kind, delay]`.
    pub fn live(&self) -> Vec<(&'static str, f64)> {
        let state = self.state.lock().unwrap();
        state
            .list
            .iter()
            .map(|entry| (entry.kind, entry.delay))
            .collect()
    }
}

struct ManualTimer {
    id: u64,
    timers: ManualTimers,
}

impl Timer for ManualTimer {
    fn cancel(&self) {
        let removed = {
            let mut state = self.timers.state.lock().unwrap();
            let at = state.list.iter().position(|entry| entry.id == self.id);
            at.map(|at| state.list.remove(at))
        };
        if let Some(entry) = removed {
            push(
                &self.timers.events,
                json!(["cleared", entry.kind, js_number(entry.delay)]),
            );
        }
    }
}

impl Timers for ManualTimers {
    fn set_timeout(&self, delay_ms: f64, task: Box<dyn FnOnce() + Send>) -> Box<dyn Timer> {
        self.add("timeout", delay_ms, Task::Once(Some(task)))
    }

    fn set_interval(&self, delay_ms: f64, task: Arc<dyn Fn() + Send + Sync>) -> Box<dyn Timer> {
        self.add("interval", delay_ms, Task::Repeat(task))
    }
}

/// Ответ транспорта: `{status, body: JSON.stringify(body), headers: {}}` самотестов 0.4.7.
pub fn reply(body: Value, status: u16) -> Response {
    Response {
        status,
        headers: Headers::default(),
        body: stringify(&body, ""),
    }
}

pub fn reply_text(body: &str, status: u16) -> Response {
    Response {
        status,
        headers: Headers::default(),
        body: body.to_owned(),
    }
}

/// Запрос, который видел транспорт: адрес и параметры.
#[derive(Debug, Clone)]
pub struct Sent {
    pub url: String,
    pub options: RequestOptions,
}

impl Sent {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.options
            .headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn body(&self) -> Value {
        self.options
            .body
            .as_deref()
            .map(|body| serde_json::from_str(body).unwrap())
            .unwrap_or(Value::Null)
    }

    /// Путь после `/v2/runner/`.
    pub fn route(&self) -> String {
        self.url
            .split("/v2/runner/")
            .nth(1)
            .unwrap_or(&self.url)
            .to_owned()
    }
}

type Handler = dyn Fn(&Sent) -> BoxFuture<'static, Result<Response, TransportError>> + Send + Sync;

/// Транспорт из замыкания (`request: async (url, options) => …`); запросы — в журнал.
#[derive(Clone)]
pub struct FnTransport {
    handler: Arc<Handler>,
    pub sent: Arc<Mutex<Vec<Sent>>>,
}

impl FnTransport {
    pub fn new(
        handler: impl Fn(&Sent) -> BoxFuture<'static, Result<Response, TransportError>>
        + Send
        + Sync
        + 'static,
    ) -> FnTransport {
        FnTransport {
            handler: Arc::new(handler),
            sent: Arc::default(),
        }
    }

    /// Ответ сразу.
    pub fn sync(handler: impl Fn(&Sent) -> Response + Send + Sync + 'static) -> FnTransport {
        FnTransport::new(move |sent| {
            let response = handler(sent);
            Box::pin(async move { Ok(response) })
        })
    }

    pub fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap().clone()
    }
}

impl Transport for FnTransport {
    fn request<'a>(
        &'a self,
        url: &'a str,
        options: &'a RequestOptions,
    ) -> BoxFuture<'a, Result<Response, TransportError>> {
        let sent = Sent {
            url: url.to_owned(),
            options: options.clone(),
        };
        self.sent.lock().unwrap().push(sent.clone());
        (self.handler)(&sent)
    }
}

/// Транспорт сценария: настоящий HTTP, `https://www.speedrun.com/` — на двойник speedrun,
/// счётчик запросов в пути (`settle` ждёт, пока он не обнулится).
pub struct ScenarioTransport {
    inner: HttpTransport,
    speedrun: String,
    pub in_flight: Arc<AtomicUsize>,
}

impl ScenarioTransport {
    /// `speedrun` — `http://127.0.0.1:<порт>/`.
    pub fn new(speedrun: String) -> ScenarioTransport {
        ScenarioTransport {
            inner: HttpTransport::new(),
            speedrun,
            in_flight: Arc::default(),
        }
    }
}

struct InFlight(Arc<AtomicUsize>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Transport for ScenarioTransport {
    fn request<'a>(
        &'a self,
        url: &'a str,
        options: &'a RequestOptions,
    ) -> BoxFuture<'a, Result<Response, TransportError>> {
        let target = match url.strip_prefix("https://www.speedrun.com/") {
            Some(rest) => format!("{}{rest}", self.speedrun),
            None => url.to_owned(),
        };
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        let guard = InFlight(self.in_flight.clone());
        Box::pin(async move {
            let response = self.inner.request(&target, options).await;
            drop(guard);
            response
        })
    }
}

/// Короткий хеш `JSON.stringify(value)`.
pub fn sha16(value: &Value) -> String {
    let digest = Sha256::digest(stringify(value, "").as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn is_snapshot(value: &Value) -> bool {
    value.as_object().is_some_and(|map| {
        map.contains_key("account")
            && map.contains_key("achievements")
            && map.contains_key("status")
    })
}

/// Снимок службы — хеш и пара полей; остальное — как есть. Снимки 0.4.9 — в вид 0.4.7.
pub fn digest(value: &Value) -> Value {
    digest_as(value, true)
}

/// [`digest`] для фикстур 0.4.9 (`old == false`): снимки как есть.
pub fn digest_as(value: &Value, old: bool) -> Value {
    // Карточка (`normalize`, строка списка игроков): без полей 0.4.9.
    let card = value.get("steamId64").is_some() && value.get("generationId").is_some();
    match super::profile_as_047(value).filter(|_| old) {
        Some(adapted) => digest_one(&adapted, old),
        None if old && card => digest_one(&super::without_049(value), old),
        None => digest_one(value, old),
    }
}

fn digest_one(value: &Value, old: bool) -> Value {
    if is_snapshot(value) {
        return json!({
            "#snapshot": sha16(value), "status": value["status"], "error": value["error"],
            "signedIn": value["account"]["signedIn"],
        });
    }
    match value {
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| digest_as(item, old)).collect())
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), digest_as(item, old)))
                .collect::<Map<_, _>>(),
        ),
        other => other.clone(),
    }
}

/// Строки: адреса двойников — `{speedrun}/` и `{base}`.
pub fn replace_base(value: &Value, base: &str, speedrun: &str) -> Value {
    match value {
        Value::String(text) => Value::String(
            text.replace(speedrun, "{speedrun}/")
                .replace(base, "{base}"),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| replace_base(item, base, speedrun))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| {
                    (
                        key.replace(speedrun, "{speedrun}/").replace(base, "{base}"),
                        replace_base(item, base, speedrun),
                    )
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Журнал: по серверам (порядок внутри сервера сохраняется); параллельные пачки
/// `/v2/runner/friends` и `/session/end` — по телу.
pub fn normalize_journal(journal: &[Value]) -> Vec<Value> {
    const PARALLEL: [&str; 2] = ["/v2/runner/friends", "/v2/runner/session/end"];
    let text = |value: &Value, key: &str| value[key].as_str().unwrap_or("").to_owned();
    let mut by_server = journal.to_vec();
    by_server.sort_by_key(|entry| text(entry, "server"));
    let mut out = Vec::new();
    let mut at = 0;
    while at < by_server.len() {
        let mut end = at + 1;
        if PARALLEL.contains(&text(&by_server[at], "path").as_str()) {
            while end < by_server.len()
                && text(&by_server[end], "server") == text(&by_server[at], "server")
                && text(&by_server[end], "path") == text(&by_server[at], "path")
            {
                end += 1;
            }
        }
        let mut run = by_server[at..end].to_vec();
        run.sort_by(|a, b| hlsr_core::js::cmp_utf16(&text(a, "body"), &text(b, "body")));
        out.extend(run);
        at = end;
    }
    out
}

/// Снимок папки данных как у 0.4.7: `online-session.v2.json` (`{version:2, kind:"plain", data}`)
/// показывается тем, что записал бы `SecureSession` без шифрования, — `online-session.json`
/// `{version:1, plain:data}`. Файлы 0.4.9 — в вид 0.4.7 (см. [`node_files_as`]).
pub fn node_files(root: &std::path::Path) -> Value {
    node_files_as(root, true)
}

/// [`node_files`]; `old == false` — для фикстур 0.4.9: времена достижений, рекорд серии и новые
/// поля карточки остаются.
pub fn node_files_as(root: &std::path::Path, old: bool) -> Value {
    let mut files = super::files(root);
    let map = files.as_object_mut().unwrap();
    if let Some(v2) = map.remove("online-session.v2.json") {
        let bytes = hlsr_core::js::latin1_bytes(v2.as_str().unwrap());
        let mut envelope: Value = serde_json::from_slice(&bytes).unwrap();
        if old && let Some(profile) = envelope["data"].get_mut("profile") {
            *profile = super::without_049(profile);
        }
        assert_eq!(
            envelope["kind"], "plain",
            "хранилище сессии в тестах — plain"
        );
        let legacy = stringify(&json!({ "version": 1, "plain": envelope["data"] }), "");
        map.insert(
            "online-session.json".to_owned(),
            json!(hlsr_core::js::latin1_string(legacy.as_bytes())),
        );
    }
    // Файлы 0.4.9 в вид 0.4.7: времени достижений не было, у серии не было рекорда.
    if !old {
        return sorted(files);
    }
    map.remove("achievement-times.json");
    if let Some(days) = map.get_mut("launch-days.json")
        && let Ok(mut parsed) = serde_json::from_str::<Value>(days.as_str().unwrap_or(""))
        && let Some(object) = parsed.as_object_mut()
    {
        object.shift_remove("best");
        *days = json!(stringify(&parsed, ""));
    }
    sorted(files)
}

/// Ключи объекта по возрастанию (как `sort()` скрипта).
fn sorted(mut files: Value) -> Value {
    let map = files.as_object_mut().unwrap();
    let mut entries: Vec<(String, Value)> = std::mem::take(map).into_iter().collect();
    entries.sort_by(|a, b| hlsr_core::js::cmp_utf16(&a.0, &b.0));
    Value::Object(entries.into_iter().collect())
}
