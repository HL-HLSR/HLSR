//! Счётчики онлайна на «Главной» (`telemetry/online-client.js`): `GET /v1/public/online` с
//! `If-None-Match` раз в минуту при видимом окне и при показе окна. Снимок — `unknown` (ничего
//! не получено), `ok` и `stale` (последнему ответу больше 5 минут); при каждой попытке клиент
//! отдаёт его в `on_update` (в окно уходит событие `public-online`).
//!
//! Ответ больше лимита (8 КиБ) завершается ошибкой сети. В 0.4.7 транспорт не завершал запрос,
//! `inFlight` оставался навсегда, и счётчики больше не обновлялись до перезапуска (спецификация,
//! решение 6). Запрос, отменённый вызывающим, тоже снимает `inFlight`.

use super::config;
use super::transport::{RequestOptions, Transport};
use crate::js_value;
use hlsr_core::js::iso::iso_from_ms;
use hlsr_core::json_text::{js_number, json_parse};
use hlsr_delivery::timing::Clock;
use serde_json::{Map, Value, json};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;
use tokio::task::JoinHandle;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Игра в снимке: сколько в сети и сколько активных забегов (`null` — нет данных).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GameOnline {
    pub online: Option<f64>,
    pub active_runs: Option<f64>,
}

/// Разобранный ответ сервера.
#[derive(Debug, Clone, PartialEq)]
pub struct OnlineValue {
    pub launcher_online: f64,
    /// В порядке ключей объекта JS.
    pub games: Vec<(String, GameOnline)>,
}

impl OnlineValue {
    /// `{launcherOnline, games}`.
    pub fn to_json(&self) -> Value {
        json!({"launcherOnline": js_number(self.launcher_online), "games": games_json(&self.games)})
    }
}

fn games_json(games: &[(String, GameOnline)]) -> Value {
    let mut map = Map::new();
    for (id, game) in games {
        map.insert(
            id.clone(),
            json!({
                "online": game.online.map_or(Value::Null, js_number),
                "activeRuns": game.active_runs.map_or(Value::Null, js_number),
            }),
        );
    }
    Value::Object(map)
}

/// `status` снимка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnlineStatus {
    /// Ничего не получено.
    Unknown,
    Ok,
    /// Последнему ответу больше 5 минут.
    Stale,
}

impl OnlineStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            OnlineStatus::Unknown => "unknown",
            OnlineStatus::Ok => "ok",
            OnlineStatus::Stale => "stale",
        }
    }
}

/// `snapshot()`: событие `public-online`.
#[derive(Debug, Clone, PartialEq)]
pub struct OnlineSnapshot {
    pub status: OnlineStatus,
    pub launcher_online: Option<f64>,
    pub games: Vec<(String, GameOnline)>,
    /// `toISOString()` времени последнего ответа.
    pub updated_at: Option<String>,
    /// Возраст ответа, мс; у `unknown` нет.
    pub age_ms: Option<i64>,
}

impl OnlineSnapshot {
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("status".to_owned(), json!(self.status.as_str()));
        map.insert(
            "launcherOnline".to_owned(),
            self.launcher_online.map_or(Value::Null, js_number),
        );
        map.insert("games".to_owned(), games_json(&self.games));
        map.insert("updatedAt".to_owned(), json!(self.updated_at));
        if let Some(age) = self.age_ms {
            map.insert("ageMs".to_owned(), json!(age));
        }
        Value::Object(map)
    }
}

/// Итог `fetchOnce`.
#[derive(Debug, Clone, PartialEq)]
pub enum FetchResult {
    /// `{skipped}`: `backend-off`, `in-flight`, `hidden`.
    Skipped(&'static str),
    /// `{ok: true, notModified: true}`.
    NotModified,
    /// `{ok: true, value}`.
    Fetched(OnlineValue),
    /// `{ok: false, reason: "http", status}`.
    Http { status: u16 },
    /// `{ok: false, reason: "payload"}`.
    Payload,
    /// `{ok: false, reason: "network"}`.
    Network,
}

impl FetchResult {
    /// Объект, который вернул бы `fetchOnce()` 0.4.7.
    pub fn to_json(&self) -> Value {
        match self {
            FetchResult::Skipped(why) => json!({"skipped": why}),
            FetchResult::NotModified => json!({"ok": true, "notModified": true}),
            FetchResult::Fetched(value) => json!({"ok": true, "value": value.to_json()}),
            FetchResult::Http { status } => {
                json!({"ok": false, "reason": "http", "status": status})
            }
            FetchResult::Payload => json!({"ok": false, "reason": "payload"}),
            FetchResult::Network => json!({"ok": false, "reason": "network"}),
        }
    }
}

/// `onUpdate(snapshot)`.
pub type OnUpdate = Arc<dyn Fn(&OnlineSnapshot) + Send + Sync>;

/// Параметры [`OnlineClient::new`] (`options` конструктора 0.4.7).
pub struct OnlineClientOptions {
    /// По умолчанию `ENDPOINTS.online`.
    pub endpoint: Option<String>,
    /// По умолчанию `0.0.0`.
    pub launcher_version: Option<String>,
    pub clock: Clock,
    pub transport: Arc<dyn Transport>,
    /// `backendConfigured`: по умолчанию `BACKEND_CONFIGURED`.
    pub backend_configured: Option<bool>,
    /// `isWindowVisible`: по умолчанию окно видно всегда.
    pub is_window_visible: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    pub on_update: Option<OnUpdate>,
}

impl OnlineClientOptions {
    /// Параметры по умолчанию с часами и транспортом.
    pub fn new(clock: Clock, transport: Arc<dyn Transport>) -> OnlineClientOptions {
        OnlineClientOptions {
            endpoint: None,
            launcher_version: None,
            clock,
            transport,
            backend_configured: None,
            is_window_visible: None,
            on_update: None,
        }
    }
}

#[derive(Default)]
struct State {
    in_flight: bool,
    etag: Option<String>,
    last_value: Option<OnlineValue>,
    last_value_at: i64,
    last_error: Option<String>,
    consecutive_failures: u32,
}

/// Снимает `inFlight` и при отмене запроса (как `finally`).
struct InFlight<'a>(&'a Mutex<State>);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        lock(self.0).in_flight = false;
    }
}

pub struct OnlineClient {
    endpoint: String,
    launcher_version: String,
    clock: Clock,
    transport: Arc<dyn Transport>,
    backend_configured: bool,
    is_window_visible: Arc<dyn Fn() -> bool + Send + Sync>,
    on_update: Option<OnUpdate>,
    state: Mutex<State>,
    timer: Mutex<Option<JoinHandle<()>>>,
}

impl OnlineClient {
    pub fn new(options: OnlineClientOptions) -> OnlineClient {
        OnlineClient {
            endpoint: options
                .endpoint
                .filter(|endpoint| !endpoint.is_empty())
                .unwrap_or_else(|| config::ENDPOINT_ONLINE.to_owned()),
            launcher_version: options
                .launcher_version
                .filter(|version| !version.is_empty())
                .unwrap_or_else(|| "0.0.0".to_owned()),
            clock: options.clock,
            transport: options.transport,
            backend_configured: options
                .backend_configured
                .unwrap_or(config::BACKEND_CONFIGURED),
            is_window_visible: options
                .is_window_visible
                .unwrap_or_else(|| Arc::new(|| true)),
            on_update: options.on_update,
            state: Mutex::default(),
            timer: Mutex::default(),
        }
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    /// Подряд неудачных попыток (сбрасывается любым успехом, в том числе `304`).
    pub fn consecutive_failures(&self) -> u32 {
        lock(&self.state).consecutive_failures
    }

    /// Причина последней неудачи (`TELEMETRY_*`, `HTTP_<код>`, `BAD_PAYLOAD`); `None` — после
    /// успеха.
    pub fn last_error(&self) -> Option<String> {
        lock(&self.state).last_error.clone()
    }

    /// `snapshot()`.
    pub fn snapshot(&self) -> OnlineSnapshot {
        let state = lock(&self.state);
        let Some(value) = &state.last_value else {
            return OnlineSnapshot {
                status: OnlineStatus::Unknown,
                launcher_online: None,
                games: Vec::new(),
                updated_at: None,
                age_ms: None,
            };
        };
        let age = self.now() - state.last_value_at;
        OnlineSnapshot {
            status: if age > config::online::STALE_AFTER_MS as i64 {
                OnlineStatus::Stale
            } else {
                OnlineStatus::Ok
            },
            launcher_online: Some(value.launcher_online),
            games: value.games.clone(),
            updated_at: Some(iso_from_ms(state.last_value_at)),
            age_ms: Some(age),
        }
    }

    /// `start()`: первый запрос сразу, дальше раз в минуту. Нужна среда tokio.
    pub fn start(self: &Arc<Self>) {
        if !self.backend_configured {
            return;
        }
        let mut timer = lock(&self.timer);
        if timer.is_some() {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let period = Duration::from_millis(config::online::POLL_MS);
        let client = Arc::downgrade(self);
        *timer = Some(handle.spawn(poll(client, period)));
    }

    /// `stop()`.
    pub fn stop(&self) {
        if let Some(timer) = lock(&self.timer).take() {
            timer.abort();
        }
    }

    /// `fetchOnce({force})`: один запрос; `force` обходит проверку видимости окна.
    pub async fn fetch_once(&self, force: bool) -> FetchResult {
        if !self.backend_configured {
            return FetchResult::Skipped("backend-off");
        }
        let visible = force || (self.is_window_visible)();
        let etag = {
            let mut state = lock(&self.state);
            if state.in_flight {
                return FetchResult::Skipped("in-flight");
            }
            if !visible {
                return FetchResult::Skipped("hidden");
            }
            state.in_flight = true;
            state.etag.clone()
        };
        let _flight = InFlight(&self.state);

        let mut options = RequestOptions {
            method: Some("GET".to_owned()),
            timeout_ms: Some(config::online::TIMEOUT_MS),
            launcher_version: Some(self.launcher_version.clone()),
            ..RequestOptions::default()
        };
        if let Some(etag) = etag {
            options.headers.push(("if-none-match".to_owned(), etag));
        }
        match self.transport.request(&self.endpoint, &options).await {
            Ok(response) => self.handle(response),
            Err(error) => {
                {
                    let mut state = lock(&self.state);
                    state.consecutive_failures += 1;
                    state.last_error = Some(error.to_string());
                }
                self.notify();
                FetchResult::Network
            }
        }
    }

    fn handle(&self, response: super::transport::Response) -> FetchResult {
        let status = response.status;
        if status == 304 {
            {
                let mut state = lock(&self.state);
                state.last_value_at = self.now();
                state.consecutive_failures = 0;
                state.last_error = None;
            }
            self.notify();
            return FetchResult::NotModified;
        }
        if !(200..300).contains(&status) {
            {
                let mut state = lock(&self.state);
                state.consecutive_failures += 1;
                state.last_error = Some(format!("HTTP_{status}"));
            }
            self.notify();
            return FetchResult::Http { status };
        }
        let Some(parsed) = parse(&response.body) else {
            {
                let mut state = lock(&self.state);
                state.consecutive_failures += 1;
                state.last_error = Some("BAD_PAYLOAD".to_owned());
            }
            self.notify();
            return FetchResult::Payload;
        };
        {
            let mut state = lock(&self.state);
            if let Some(etag) = response.headers.get("etag") {
                state.etag = Some(etag.to_owned());
            }
            state.last_value = Some(parsed.clone());
            state.last_value_at = self.now();
            state.consecutive_failures = 0;
            state.last_error = None;
        }
        self.notify();
        FetchResult::Fetched(parsed)
    }

    /// `notify()`: ошибки колбэка не мешают клиенту.
    fn notify(&self) {
        if let Some(on_update) = &self.on_update {
            let snapshot = self.snapshot();
            let _ = catch_unwind(AssertUnwindSafe(|| on_update(&snapshot)));
        }
    }
}

/// Опрос по таймеру: первый запрос сразу, дальше каждые `period`; клиент не удерживается.
async fn poll(client: Weak<OnlineClient>, period: Duration) {
    let mut ticks = tokio::time::interval(period);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        let Some(client) = client.upgrade() else {
            return;
        };
        client.fetch_once(false).await;
    }
}

/// `/^[0-9a-z][0-9a-z._-]{0,63}$/i`.
fn is_game_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// `Number.isInteger(value) && value >= 0`.
fn count(value: Option<&Value>) -> Option<f64> {
    js_value::finite_number(value).filter(|n| n.fract() == 0.0 && *n >= 0.0)
}

/// `parse(body)`: `{launcherOnline, games}` или `None`, если ответ не годится.
fn parse(body: &str) -> Option<OnlineValue> {
    let data = json_parse(body).ok()?;
    if !data.is_object() {
        return None;
    }
    let version = js_value::finite_number(data.get("schemaVersion"))?;
    if version != 1.0 {
        return None;
    }
    let launcher_online = count(data.get("launcherOnline"))?;

    let mut games = Vec::new();
    let source = data.get("games").filter(|games| games.is_object());
    for (id, value) in js_value::entries(source) {
        if !is_game_id(&id) || !js_value::is_object(Some(value)) {
            continue;
        }
        let online = match value.get("online") {
            Some(Value::Null) => None,
            other => match count(other) {
                Some(online) => Some(online),
                None => continue,
            },
        };
        games.push((
            id,
            GameOnline {
                online,
                active_runs: count(value.get("activeRuns")),
            },
        ));
    }
    Some(OnlineValue {
        launcher_online,
        games,
    })
}
