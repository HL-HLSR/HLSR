//! Отправка очереди телеметрии (`telemetry/Sender.js`): `POST /v1/telemetry/batch` с пачкой до 50
//! событий и 64 КиБ, `authorization: Bearer <токен>`. 2xx — отправленное удаляется (если очередь
//! не очищали за время запроса); 429 — пауза из `Retry-After` или задержка; прочие 4xx — пачка
//! выбрасывается; 5xx и обрыв — задержка `30 с · 2^(n−1)` до 30 мин с разбросом ±25 %.

use super::config;
use super::queue::TelemetryQueue;
use super::transport::{RequestOptions, Transport, retry_after_ms};
use hlsr_core::js::{self, iso::iso_from_ms};
use hlsr_core::json_formats::compact;
use hlsr_core::json_text::{js_number, json_parse};
use hlsr_delivery::ports::Logger;
use hlsr_delivery::timing::{Clock, Random};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::task::JoinHandle;

/// Токен входа для `authorization` (`authorizationProvider`); `None` или пусто — не вошёл.
pub type AuthorizationProvider = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// `jitter(valueMs, ratio)`: `round(value − spread + random · 2 · spread)`, не меньше 0.
pub fn jitter(value_ms: f64, ratio: f64, random: f64) -> f64 {
    let spread = value_ms * ratio;
    js::math_round(value_ms - spread + random * spread * 2.0).max(0.0)
}

/// Итог `flush()`.
#[derive(Debug, Clone, PartialEq)]
pub enum FlushResult {
    /// `{skipped}`: `in-flight`, `signed-out`, `backoff`, `empty`.
    Skipped(&'static str),
    /// `{ok: true, sent, accepted}`.
    Sent { sent: usize, accepted: Value },
    /// `{ok: false, reason: "rate-limited", retryAfterMs}`.
    RateLimited { retry_after_ms: Option<f64> },
    /// `{ok: false, reason: "rejected", status}`: сервер отверг пачку (4xx), она выброшена.
    Rejected { status: u16 },
    /// `{ok: false, reason: "server", status}`.
    Server { status: u16 },
    /// `{ok: false, reason: "network"}`.
    Network,
}

impl FlushResult {
    /// Объект, который вернул бы `flush()` 0.4.7.
    pub fn to_json(&self) -> Value {
        match self {
            FlushResult::Skipped(why) => json!({"skipped": why}),
            FlushResult::Sent { sent, accepted } => {
                json!({"ok": true, "sent": sent, "accepted": accepted})
            }
            FlushResult::RateLimited { retry_after_ms } => json!({
                "ok": false,
                "reason": "rate-limited",
                "retryAfterMs": retry_after_ms.map_or(Value::Null, js_number),
            }),
            FlushResult::Rejected { status } => {
                json!({"ok": false, "reason": "rejected", "status": status})
            }
            FlushResult::Server { status } => {
                json!({"ok": false, "reason": "server", "status": status})
            }
            FlushResult::Network => json!({"ok": false, "reason": "network"}),
        }
    }
}

#[derive(Debug, Default)]
struct Stats {
    sent: usize,
    accepted: f64,
    dropped: usize,
    last_error: Option<String>,
    last_sent_at: Option<String>,
}

#[derive(Debug, Default)]
struct State {
    failures: u32,
    next_attempt_at: f64,
    in_flight: bool,
    stats: Stats,
}

/// Параметры [`Sender::new`] (`options` конструктора 0.4.7).
pub struct SenderOptions {
    pub queue: Arc<Mutex<TelemetryQueue>>,
    /// `endpoint`: по умолчанию `ENDPOINTS.batch`.
    pub endpoint: Option<String>,
    /// `launcherVersion`: по умолчанию `0.0.0`.
    pub launcher_version: Option<String>,
    pub logger: Arc<dyn Logger>,
    pub clock: Clock,
    pub random: Random,
    pub transport: Arc<dyn Transport>,
    pub authorization: Option<AuthorizationProvider>,
}

pub struct Sender {
    queue: Arc<Mutex<TelemetryQueue>>,
    endpoint: String,
    launcher_version: String,
    logger: Arc<dyn Logger>,
    clock: Clock,
    random: Random,
    transport: Arc<dyn Transport>,
    authorization: Option<AuthorizationProvider>,
    state: Mutex<State>,
    timer: Mutex<Option<JoinHandle<()>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Снимает `inFlight` и при отмене запроса (как `finally`).
struct InFlight<'a>(&'a Mutex<State>);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        lock(self.0).in_flight = false;
    }
}

/// [`InFlight`] для отправки в отдельной задаче: держит отправителя.
struct OwnedInFlight(Arc<Sender>);

impl Drop for OwnedInFlight {
    fn drop(&mut self) {
        lock(&self.0.state).in_flight = false;
    }
}

/// Пачка, взятая синхронной частью `flush()` (до первого `await` в 0.4.7).
struct Prepared {
    token: Option<String>,
    count: usize,
    generation: u64,
    body: String,
}

impl Sender {
    pub fn new(options: SenderOptions) -> Sender {
        Sender {
            queue: options.queue,
            endpoint: options
                .endpoint
                .filter(|endpoint| !endpoint.is_empty())
                .unwrap_or_else(|| config::ENDPOINT_BATCH.to_owned()),
            launcher_version: options
                .launcher_version
                .filter(|version| !version.is_empty())
                .unwrap_or_else(|| "0.0.0".to_owned()),
            logger: options.logger,
            clock: options.clock,
            random: options.random,
            transport: options.transport,
            authorization: options.authorization,
            state: Mutex::default(),
            timer: Mutex::default(),
        }
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    /// `start()`: отправка раз в минуту (первая — через минуту). Нужна среда tokio.
    pub fn start(self: &Arc<Self>) {
        let mut timer = lock(&self.timer);
        if timer.is_some() {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let period = Duration::from_millis(config::transport::FLUSH_INTERVAL_MS);
        let sender = Arc::downgrade(self);
        *timer = Some(handle.spawn(async move {
            let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            loop {
                ticks.tick().await;
                let Some(sender) = sender.upgrade() else {
                    return;
                };
                sender.flush().await;
            }
        }));
    }

    /// `stop()`.
    pub fn stop(&self) {
        if let Some(timer) = lock(&self.timer).take() {
            timer.abort();
        }
    }

    fn token(&self) -> Option<String> {
        self.authorization
            .as_ref()
            .map(|provider| provider().unwrap_or_default())
    }

    /// `flush()`: отправить пачку из начала очереди.
    pub async fn flush(&self) -> FlushResult {
        let prepared = match self.prepare() {
            Ok(prepared) => prepared,
            Err(skipped) => return skipped,
        };
        let _in_flight = InFlight(&self.state);
        self.send(prepared).await
    }

    /// `sender.flush().catch(() => {})`: синхронная часть — сейчас (проверки, пачка, `inFlight`),
    /// запрос — в задаче tokio. Вторая отправка, начатая до конца первой, пропускается
    /// (`in-flight`) с той пачкой, что была на момент вызова, — как в 0.4.7. Без среды tokio или
    /// когда отправлять нечего — `None`.
    pub fn flush_in_background(self: &Arc<Self>) -> Option<JoinHandle<FlushResult>> {
        let prepared = self.prepare().ok()?;
        let guard = OwnedInFlight(self.clone());
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        Some(runtime.spawn(async move {
            let guard = guard;
            guard.0.send(prepared).await
        }))
    }

    /// Синхронная часть `flush()`: пропуск или пачка с отметкой `inFlight`.
    fn prepare(&self) -> Result<Prepared, FlushResult> {
        let mut state = lock(&self.state);
        if state.in_flight {
            return Err(FlushResult::Skipped("in-flight"));
        }
        let token = self.token();
        if token.as_deref() == Some("") {
            return Err(FlushResult::Skipped("signed-out"));
        }
        if (self.now() as f64) < state.next_attempt_at {
            return Err(FlushResult::Skipped("backoff"));
        }
        let (batch, generation) = {
            let mut queue = lock(&self.queue);
            let batch = queue.peek(config::queue::BATCH_SIZE, config::queue::MAX_BATCH_BYTES);
            (batch, queue.generation())
        };
        if batch.is_empty() {
            return Err(FlushResult::Skipped("empty"));
        }
        state.in_flight = true;
        let count = batch.len();
        let body = compact(&json!({
            "schemaVersion": config::SCHEMA_VERSION,
            "events": batch,
        }));
        Ok(Prepared {
            token,
            count,
            generation,
            body,
        })
    }

    /// Запрос и разбор ответа; `inFlight` снимает вызывающий.
    async fn send(&self, prepared: Prepared) -> FlushResult {
        let Prepared {
            token,
            count,
            generation,
            body,
        } = prepared;
        let options = RequestOptions {
            method: Some("POST".to_owned()),
            headers: token
                .map(|token| vec![("authorization".to_owned(), format!("Bearer {token}"))])
                .unwrap_or_default(),
            body: Some(body),
            timeout_ms: Some(config::transport::TIMEOUT_MS),
            max_response_bytes: None,
            launcher_version: Some(self.launcher_version.clone()),
        };
        match self.transport.request(&self.endpoint, &options).await {
            Ok(response) => self.handle_response(&response, count, generation),
            Err(error) => {
                self.on_failure(&mut lock(&self.state), error.to_string(), false);
                FlushResult::Network
            }
        }
    }

    fn remove_sent(&self, count: usize, generation: u64) {
        let mut queue = lock(&self.queue);
        if queue.generation() == generation {
            queue.remove(count);
        }
    }

    fn handle_response(
        &self,
        response: &super::transport::Response,
        count: usize,
        generation: u64,
    ) -> FlushResult {
        let status = response.status;
        if (200..300).contains(&status) {
            self.remove_sent(count, generation);
            let accepted = match json_parse(&response.body) {
                Ok(Value::Object(parsed)) => parsed
                    .get("accepted")
                    .and_then(Value::as_f64)
                    .filter(|x| x.is_finite() && x.fract() == 0.0),
                _ => None,
            }
            .unwrap_or(count as f64);
            let mut state = lock(&self.state);
            state.failures = 0;
            state.next_attempt_at = 0.0;
            state.stats.sent += count;
            state.stats.last_sent_at = Some(iso_from_ms(self.now()));
            state.stats.last_error = None;
            state.stats.accepted += accepted;
            return FlushResult::Sent {
                sent: count,
                accepted: js_number(accepted),
            };
        }
        if status == 429 {
            let now = self.now();
            let retry = retry_after_ms(&response.headers, Some(now));
            let mut state = lock(&self.state);
            state.failures += 1;
            let pause = retry.unwrap_or_else(|| self.backoff_ms(state.failures));
            state.next_attempt_at = now as f64 + pause;
            state.stats.last_error = Some("HTTP_429".to_owned());
            return FlushResult::RateLimited {
                retry_after_ms: retry,
            };
        }
        if (400..500).contains(&status) {
            self.remove_sent(count, generation);
            let mut state = lock(&self.state);
            state.stats.dropped += count;
            state.stats.last_error = Some(format!("HTTP_{status}"));
            self.logger.warn(&format!(
                "[telemetry] сервер отклонил {count} событий (HTTP {status}), очередь очищена"
            ));
            self.on_failure(&mut state, format!("HTTP_{status}"), true);
            return FlushResult::Rejected { status };
        }
        self.on_failure(&mut lock(&self.state), format!("HTTP_{status}"), false);
        FlushResult::Server { status }
    }

    /// `onFailure(reason, keepQueueDrained)`.
    fn on_failure(&self, state: &mut State, reason: String, keep_queue_drained: bool) {
        state.failures += 1;
        state.next_attempt_at = self.now() as f64 + self.backoff_ms(state.failures);
        if state.failures == config::transport::MAX_CONSECUTIVE_FAILURES {
            self.logger.warn(&format!(
                "[telemetry] {} неудачных попыток подряд ({reason}), переход к максимальной задержке",
                state.failures
            ));
        }
        state.stats.last_error = Some(reason);
        if keep_queue_drained {
            state.failures = state.failures.min(3);
        }
    }

    /// `backoffMs()`: `min(30 с · 2^(min(n, 8) − 1), 30 мин)` с разбросом ±25 %.
    fn backoff_ms(&self, failures: u32) -> f64 {
        let exponent = failures.min(8);
        let base = (config::transport::BACKOFF_BASE_MS as f64
            * 2f64.powi(exponent.saturating_sub(1) as i32))
        .min(config::transport::BACKOFF_MAX_MS as f64);
        jitter(base, config::transport::JITTER_RATIO, (self.random)())
    }

    /// `snapshot()`: состояние для диагностики.
    pub fn snapshot(&self) -> Value {
        let queued = lock(&self.queue).size();
        let state = lock(&self.state);
        json!({
            "queued": queued,
            "failures": state.failures,
            "nextAttemptInMs": js_number((state.next_attempt_at - self.now() as f64).max(0.0)),
            "sent": state.stats.sent,
            "accepted": js_number(state.stats.accepted),
            "dropped": state.stats.dropped,
            "lastError": state.stats.last_error,
            "lastSentAt": state.stats.last_sent_at,
        })
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        self.stop();
    }
}
