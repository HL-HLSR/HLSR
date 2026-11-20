//! `discord-rpc/DiscordRichPresenceService.js`: подключение к Discord и показ присутствия.
//!
//! Автомат состояний `disabled → disconnected → connecting → connected`, при отказе —
//! `retry_wait` (5 с · 2ⁿ⁻¹, не больше 5 мин, ±25 %). Присутствие, изменившееся за 1,5 с,
//! уходит одним `SET_ACTIVITY`; одинаковое с уже отправленным не отправляется. `generation`
//! отсекает ответы устаревших подключений. В журнал `RPC_*` одного кода идёт не чаще раза в
//! минуту.
//!
//! Устройство Rust. В Node служба однопоточна, каждый обратный вызов выполняется целиком. Здесь
//! всё состояние под одним замком; каждая операция собирает «следствия» (строки журнала,
//! обратный вызов связи, фоновые задачи) и выполняет их по порядку после снятия замка — так
//! обратные вызовы могут снова войти в службу. Таймеры и клиент подставляются, как
//! `setTimeout`/`createClient` у 0.4.7.

use super::constants::{APPLICATION_ID, Code, State, timings};
use super::presence_model::{self, Activity, Presence};
use super::rpc::client::RpcError;
use hlsr_core::js;
use hlsr_delivery::ports::Logger;
use hlsr_delivery::timing::{Clock, Random, system_clock, system_random};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Клиент Discord, каким его видит служба (`client` у 0.4.7: `on("disconnected")`, `login`,
/// `setActivity`, `clearActivity`, `destroy`). Вызовы не блокируют и не входят в службу
/// синхронно.
pub trait PresenceClient: Send + Sync {
    /// `client.on("disconnected", handler)`: соединение закрылось (один раз за клиента).
    fn on_disconnected(&self, handler: Arc<dyn Fn() + Send + Sync>);
    /// `client.login({clientId})`: готов после `READY`.
    fn login(&self, client_id: &str) -> BoxFuture<Result<(), RpcError>>;
    /// `client.setActivity(activity)`: `Err` — исключение при вызове (например, время вне
    /// диапазона), будущее — ответ Discord.
    fn set_activity(
        &self,
        activity: &Activity,
    ) -> Result<BoxFuture<Result<Value, RpcError>>, RpcError>;
    /// `client.clearActivity()`.
    fn clear_activity(&self) -> BoxFuture<Result<Value, RpcError>>;
    /// `client.destroy()`: кадр закрытия уходит сразу, будущее ждёт закрытия соединения.
    fn destroy(&self) -> BoxFuture<()>;
}

/// Параметры фабрики клиента (`{applicationId}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateClientOptions {
    pub application_id: String,
}

/// `createClient({applicationId})`; `Err` — исключение фабрики.
pub type CreateClient =
    Arc<dyn Fn(&CreateClientOptions) -> Result<Arc<dyn PresenceClient>, RpcError> + Send + Sync>;

/// `versionProvider()`: версия лаунчера; пусто — без версии.
pub type VersionProvider = Arc<dyn Fn() -> Option<String> + Send + Sync>;
/// `localeProvider()`: язык лаунчера; всё, кроме `en`, — русский.
pub type LocaleProvider = Arc<dyn Fn() -> Option<String> + Send + Sync>;
/// `onConnectionChange(connected)`.
pub type ConnectionHandler = Arc<dyn Fn(bool) + Send + Sync>;

/// Метка таймера.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimerId(pub u64);

/// `setTimeout`/`clearTimeout` службы. Сработавший таймер вызывает обратный вызов один раз.
pub trait Timers: Send + Sync {
    fn set(&self, delay_ms: u64, callback: Box<dyn FnOnce() + Send>) -> TimerId;
    fn clear(&self, id: TimerId);
}

/// Таймеры на tokio. Как `unref` у 0.4.7, выход из программы они не держат.
pub struct TokioTimers {
    runtime: Handle,
    next: AtomicU64,
    tasks: Arc<Mutex<HashMap<u64, JoinHandle<()>>>>,
}

impl TokioTimers {
    pub fn new(runtime: Handle) -> TokioTimers {
        TokioTimers {
            runtime,
            next: AtomicU64::new(0),
            tasks: Arc::default(),
        }
    }
}

impl Timers for TokioTimers {
    fn set(&self, delay_ms: u64, callback: Box<dyn FnOnce() + Send>) -> TimerId {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let tasks = self.tasks.clone();
        let handle = self.runtime.spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            lock(&tasks).remove(&id);
            callback();
        });
        let mut all = lock(&self.tasks);
        all.retain(|_, task| !task.is_finished());
        all.insert(id, handle);
        TimerId(id)
    }

    fn clear(&self, id: TimerId) {
        if let Some(task) = lock(&self.tasks).remove(&id.0) {
            task.abort();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// `retryDelay()`: `round(base − spread + random · 2 · spread)`, `base = min(5 с · 2^max(0,
/// попытка − 1), 5 мин)`, `spread = base · 25 %`.
pub fn retry_delay_ms(attempt: u32, random: f64) -> f64 {
    let base = (timings::RETRY_BASE_MS as f64 * 2f64.powf(f64::from(attempt.saturating_sub(1))))
        .min(timings::RETRY_MAX_MS as f64);
    let spread = base * timings::RETRY_JITTER;
    js::math_round(base - spread + random * spread * 2.0)
}

/// Публичное состояние (`publicState()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicState {
    pub state: State,
    pub code: Option<Code>,
    pub last_connected_at: Option<i64>,
    pub attempt: u32,
}

impl PublicState {
    pub fn to_json(&self) -> Value {
        json!({
            "state": self.state.as_str(),
            "code": self.code.map(Code::as_str),
            "lastConnectedAt": self.last_connected_at,
            "attempt": self.attempt,
        })
    }
}

/// Параметры службы (`options` конструктора 0.4.7).
pub struct ServiceOptions {
    /// `createClient`; `None` — «клиент Discord недоступен в этой сборке».
    pub create_client: Option<CreateClient>,
    /// `applicationId`: по умолчанию [`APPLICATION_ID`].
    pub application_id: Option<String>,
    pub log: Arc<dyn Logger>,
    pub now: Clock,
    /// `setTimeout`/`clearTimeout`: по умолчанию tokio.
    pub timers: Option<Arc<dyn Timers>>,
    /// `Math.random` для разброса повтора.
    pub random: Random,
    pub version_provider: VersionProvider,
    pub locale_provider: LocaleProvider,
    pub on_connection_change: Option<ConnectionHandler>,
    /// Среда, где идут фоновые задачи: по умолчанию текущая (`Handle::current()`).
    pub runtime: Option<Handle>,
}

impl ServiceOptions {
    /// Умолчания 0.4.7: системные часы и случайность, без версии, язык `ru`.
    pub fn new(log: Arc<dyn Logger>, create_client: Option<CreateClient>) -> ServiceOptions {
        ServiceOptions {
            create_client,
            application_id: None,
            log,
            now: system_clock(),
            timers: None,
            random: system_random(),
            version_provider: Arc::new(|| None),
            locale_provider: Arc::new(|| Some("ru".to_owned())),
            on_connection_change: None,
            runtime: None,
        }
    }
}

enum Effect {
    Log(String),
    Connection(bool),
    Spawn(BoxFuture<()>),
}

struct Core {
    connected: bool,
    state: State,
    client: Option<Arc<dyn PresenceClient>>,
    attempt: u32,
    generation: u64,
    retry_timer: Option<TimerId>,
    debounce_timer: Option<TimerId>,
    /// Поколение таймера повтора/отправки: `clearTimeout` у 0.4.7 отменяет и таймер, чей
    /// обратный вызов уже ждёт замка; здесь такой вызов сверяет своё поколение.
    retry_epoch: u64,
    debounce_epoch: u64,
    pending_model: Option<Presence>,
    sent_model: Option<Presence>,
    last_connected_at_ms: Option<i64>,
    last_code: Option<Code>,
    /// `lastLogged`: код → время последней строки журнала.
    last_logged: HashMap<&'static str, i64>,
    effects: Vec<Effect>,
}

struct Inner {
    create_client: Option<CreateClient>,
    application_id: String,
    log: Arc<dyn Logger>,
    now: Clock,
    timers: Arc<dyn Timers>,
    random: Random,
    version_provider: VersionProvider,
    locale_provider: LocaleProvider,
    connection_handler: Option<ConnectionHandler>,
    runtime: Handle,
    core: Mutex<Core>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

/// Служба присутствия Discord; копии — одна служба.
#[derive(Clone)]
pub struct DiscordRichPresenceService {
    inner: Arc<Inner>,
}

impl DiscordRichPresenceService {
    /// Вне среды tokio нужен `options.runtime`.
    pub fn new(options: ServiceOptions) -> DiscordRichPresenceService {
        let runtime = options.runtime.unwrap_or_else(Handle::current);
        let timers = options
            .timers
            .unwrap_or_else(|| Arc::new(TokioTimers::new(runtime.clone())));
        DiscordRichPresenceService {
            inner: Arc::new(Inner {
                create_client: options.create_client,
                application_id: options
                    .application_id
                    .filter(|id| !id.is_empty())
                    .unwrap_or_else(|| APPLICATION_ID.to_owned()),
                log: options.log,
                now: options.now,
                timers,
                random: options.random,
                version_provider: options.version_provider,
                locale_provider: options.locale_provider,
                connection_handler: options.on_connection_change,
                runtime,
                core: Mutex::new(Core {
                    connected: false,
                    state: State::Disabled,
                    client: None,
                    attempt: 0,
                    generation: 0,
                    retry_timer: None,
                    debounce_timer: None,
                    retry_epoch: 0,
                    debounce_epoch: 0,
                    pending_model: None,
                    sent_model: None,
                    last_connected_at_ms: None,
                    last_code: None,
                    last_logged: HashMap::new(),
                    effects: Vec::new(),
                }),
                tasks: Mutex::new(Vec::new()),
            }),
        }
    }

    /// `publicState()`.
    pub fn public_state(&self) -> PublicState {
        let core = lock(&self.inner.core);
        PublicState {
            state: core.state,
            code: core.last_code,
            last_connected_at: core.last_connected_at_ms,
            attempt: core.attempt,
        }
    }

    /// `pendingModel`: последняя модель `setPresence`.
    pub fn pending_model(&self) -> Option<Presence> {
        lock(&self.inner.core).pending_model.clone()
    }

    /// `service.connected`: есть ли живое подключение.
    pub fn connected(&self) -> bool {
        lock(&self.inner.core).connected
    }

    /// `isEnabled()`.
    pub fn is_enabled(&self) -> bool {
        is_enabled(&lock(&self.inner.core))
    }

    /// `enable()`: `false` — уже включено или идёт выход.
    pub fn enable(&self) -> bool {
        self.inner.locked(|inner, core| {
            if core.state == State::ShuttingDown || is_enabled(core) {
                return false;
            }
            core.generation += 1;
            core.attempt = 0;
            core.state = State::Disconnected;
            inner.note(core, Code::Connecting, Some("включено пользователем"));
            inner.connect(core);
            true
        })
    }

    /// `disable()`: `false` — уже выключено.
    pub fn disable(&self) -> bool {
        self.inner.locked(|inner, core| {
            if core.state == State::Disabled {
                return false;
            }
            core.generation += 1;
            inner.clear_all_timers(core);
            inner.destroy_client(core, true);
            core.state = State::Disabled;
            core.attempt = 0;
            core.pending_model = None;
            core.sent_model = None;
            inner.set_connected_flag(core, false);
            inner.note(core, Code::Disabled, Some("выключено пользователем"));
            true
        })
    }

    /// `shutdown()`: выход из лаунчера; `false` — уже идёт.
    pub fn shutdown(&self) -> bool {
        self.inner.locked(|inner, core| {
            if core.state == State::ShuttingDown {
                return false;
            }
            core.generation += 1;
            inner.clear_all_timers(core);
            inner.destroy_client(core, true);
            core.state = State::ShuttingDown;
            inner.set_connected_flag(core, false);
            core.last_code = Some(Code::Shutdown);
            core.effects.push(Effect::Log(format!(
                "[discord-rpc] {}",
                Code::Shutdown.as_str()
            )));
            true
        })
    }

    /// `setPresence(state)`: язык берётся у `localeProvider`. `true` — модель изменилась и
    /// служба включена (отправка через 1,5 с).
    pub fn set_presence(&self, state: &Value) -> bool {
        let mut object: Map<String, Value> = state.as_object().cloned().unwrap_or_default();
        object.insert(
            "locale".into(),
            (self.inner.locale_provider)().map_or(Value::Null, Value::String),
        );
        let model = presence_model::normalize(&Value::Object(object));
        self.inner.locked(|inner, core| {
            if presence_model::same(Some(&model), core.pending_model.as_ref()) {
                return false;
            }
            core.pending_model = Some(model);
            if !is_enabled(core) {
                return false;
            }
            inner.clear_debounce(core);
            let epoch = core.debounce_epoch;
            let weak = Arc::downgrade(inner);
            core.debounce_timer = Some(inner.timers.set(
                timings::DEBOUNCE_MS,
                Box::new(move || {
                    if let Some(inner) = weak.upgrade() {
                        inner.locked(|inner, core| {
                            if core.debounce_epoch != epoch {
                                return;
                            }
                            core.debounce_timer = None;
                            inner.flush(core);
                        });
                    }
                }),
            ));
            true
        })
    }

    /// `flush()`: отправить присутствие сейчас, если оно изменилось.
    pub fn flush(&self) -> bool {
        self.inner.locked(|inner, core| inner.flush(core))
    }

    /// Дождаться закрытия клиентов, которым служба отправила очистку и `CLOSE` (выход из
    /// лаунчера: кадры должны уйти до конца процесса).
    pub async fn finish(&self) {
        loop {
            let handles: Vec<JoinHandle<()>> = std::mem::take(&mut *lock(&self.inner.tasks));
            if handles.is_empty() {
                return;
            }
            for handle in handles {
                let _ = handle.await;
            }
        }
    }
}

fn is_enabled(core: &Core) -> bool {
    core.state != State::Disabled && core.state != State::ShuttingDown
}

/// `error && error.code ? String(error.code) : null`.
fn error_code_text(error: &RpcError) -> Option<String> {
    error
        .code
        .as_ref()
        .filter(|code| js::truthy(code))
        .map(js::to_string)
}

impl Inner {
    /// Выполнить `f` под замком, затем следствия по порядку (уже без замка).
    fn locked<R>(self: &Arc<Self>, f: impl FnOnce(&Arc<Self>, &mut Core) -> R) -> R {
        let (result, effects) = {
            let mut core = lock(&self.core);
            let result = f(self, &mut core);
            (result, std::mem::take(&mut core.effects))
        };
        for effect in effects {
            match effect {
                Effect::Log(line) => self.log.info(&line),
                Effect::Connection(connected) => {
                    if let Some(handler) = &self.connection_handler {
                        // Потребитель не должен ронять интеграцию.
                        let _ = catch_unwind(AssertUnwindSafe(|| handler(connected)));
                    }
                }
                Effect::Spawn(future) => {
                    let handle = self.runtime.spawn(future);
                    let mut tasks = lock(&self.tasks);
                    tasks.retain(|task| !task.is_finished());
                    tasks.push(handle);
                }
            }
        }
        result
    }

    /// `note(code, message)`: запомнить код; строка журнала — не чаще раза в минуту на код.
    fn note(&self, core: &mut Core, code: Code, message: Option<&str>) {
        core.last_code = Some(code);
        let at = (self.now)();
        if let Some(previous) = core.last_logged.get(code.as_str())
            && at - previous < timings::LOG_THROTTLE_MS as i64
        {
            return;
        }
        core.last_logged.insert(code.as_str(), at);
        let suffix = message
            .filter(|text| !text.is_empty())
            .map_or_else(String::new, |text| format!(": {text}"));
        core.effects.push(Effect::Log(format!(
            "[discord-rpc] {}{suffix}",
            code.as_str()
        )));
    }

    fn set_connected_flag(&self, core: &mut Core, value: bool) -> bool {
        if value == core.connected {
            return false;
        }
        core.connected = value;
        core.effects.push(Effect::Connection(value));
        true
    }

    fn clear_retry(&self, core: &mut Core) {
        core.retry_epoch += 1;
        if let Some(id) = core.retry_timer.take() {
            self.timers.clear(id);
        }
    }

    fn clear_debounce(&self, core: &mut Core) {
        core.debounce_epoch += 1;
        if let Some(id) = core.debounce_timer.take() {
            self.timers.clear(id);
        }
    }

    fn clear_all_timers(&self, core: &mut Core) {
        self.clear_retry(core);
        self.clear_debounce(core);
    }

    fn spawn_cleanup(core: &mut Core, future: BoxFuture<()>) {
        core.effects.push(Effect::Spawn(future));
    }

    /// `destroyClient(clearActivity)`: кадр очистки, затем закрытие; ошибки молчат.
    fn destroy_client(&self, core: &mut Core, clear_activity: bool) {
        let Some(client) = core.client.take() else {
            return;
        };
        if clear_activity {
            let cleared = client.clear_activity();
            Self::spawn_cleanup(
                core,
                Box::pin(async move {
                    let _ = cleared.await;
                }),
            );
        }
        Self::spawn_cleanup(core, client.destroy());
    }

    /// `destroyOrphan(client)`: закрыть клиента, которого служба не приняла.
    fn destroy_orphan(&self, core: &mut Core, client: &Arc<dyn PresenceClient>) {
        Self::spawn_cleanup(core, client.destroy());
    }

    fn connect(self: &Arc<Self>, core: &mut Core) {
        if !is_enabled(core) {
            return;
        }
        if core.state == State::Connecting || core.client.is_some() {
            return;
        }
        let Some(create_client) = &self.create_client else {
            core.state = State::Unavailable;
            self.note(
                core,
                Code::DiscordUnavailable,
                Some("клиент Discord недоступен в этой сборке"),
            );
            return;
        };

        let generation = core.generation;
        core.state = State::Connecting;
        core.attempt += 1;
        self.note(core, Code::Connecting, None);

        let options = CreateClientOptions {
            application_id: self.application_id.clone(),
        };
        let client = match create_client(&options) {
            Ok(client) => client,
            Err(error) => {
                self.on_connect_failed(core, generation, &error);
                return;
            }
        };

        let weak: Weak<Inner> = Arc::downgrade(self);
        client.on_disconnected(Arc::new(move || {
            if let Some(inner) = weak.upgrade() {
                inner.locked(|inner, core| inner.on_disconnected(core, generation));
            }
        }));

        let login = client.login(&self.application_id);
        let inner = self.clone();
        self.runtime.spawn(async move {
            let result = login.await;
            inner.locked(|inner, core| inner.on_login_settled(core, generation, client, result));
        });
    }

    fn on_login_settled(
        self: &Arc<Self>,
        core: &mut Core,
        generation: u64,
        client: Arc<dyn PresenceClient>,
        result: Result<(), RpcError>,
    ) {
        match result {
            Ok(()) => {
                if generation != core.generation {
                    self.destroy_orphan(core, &client);
                    return;
                }
                core.client = Some(client);
                core.state = State::Connected;
                core.attempt = 0;
                core.last_connected_at_ms = Some((self.now)());
                core.last_logged.remove(Code::DiscordUnavailable.as_str());
                core.last_code = Some(Code::Connected);
                core.effects.push(Effect::Log(format!(
                    "[discord-rpc] {}",
                    Code::Connected.as_str()
                )));
                self.set_connected_flag(core, true);
                self.flush(core);
            }
            Err(error) => {
                self.destroy_orphan(core, &client);
                self.on_connect_failed(core, generation, &error);
            }
        }
    }

    fn on_connect_failed(self: &Arc<Self>, core: &mut Core, generation: u64, error: &RpcError) {
        if generation != core.generation {
            return;
        }
        core.client = None;
        self.set_connected_flag(core, false);
        core.state = State::Unavailable;
        let code = error_code_text(error);
        self.note(core, Code::DiscordUnavailable, code.as_deref());
        self.schedule_retry(core, generation);
    }

    fn on_disconnected(self: &Arc<Self>, core: &mut Core, generation: u64) {
        if generation != core.generation {
            return;
        }
        if !is_enabled(core) {
            return;
        }
        core.client = None;
        self.set_connected_flag(core, false);
        core.state = State::Disconnected;
        core.sent_model = None;
        self.note(core, Code::Disconnected, None);
        self.schedule_retry(core, generation);
    }

    fn schedule_retry(self: &Arc<Self>, core: &mut Core, generation: u64) {
        if !is_enabled(core) {
            return;
        }
        self.clear_retry(core);

        let delay = retry_delay_ms(core.attempt, (self.random)());
        core.state = State::RetryWait;
        let epoch = core.retry_epoch;
        let weak = Arc::downgrade(self);
        core.retry_timer = Some(self.timers.set(
            delay as u64,
            Box::new(move || {
                if let Some(inner) = weak.upgrade() {
                    inner.locked(|inner, core| {
                        if core.retry_epoch != epoch {
                            return;
                        }
                        core.retry_timer = None;
                        if generation != core.generation || !is_enabled(core) {
                            return;
                        }
                        core.state = State::Disconnected;
                        inner.connect(core);
                    });
                }
            }),
        ));
    }

    fn flush(self: &Arc<Self>, core: &mut Core) -> bool {
        if core.state != State::Connected {
            return false;
        }
        let Some(client) = core.client.clone() else {
            return false;
        };
        let Some(model) = core.pending_model.clone() else {
            return false;
        };
        if presence_model::same(Some(&model), core.sent_model.as_ref()) {
            return false;
        }

        let version = (self.version_provider)();
        let activity = model.activity(version.as_deref());
        let generation = core.generation;

        match client.set_activity(&activity) {
            Ok(reply) => {
                let inner = self.clone();
                let task = Box::pin(async move {
                    let result = reply.await;
                    inner.locked(|inner, core| match result {
                        Ok(_) => {
                            if generation == core.generation {
                                core.sent_model = Some(model);
                            }
                        }
                        Err(error) => inner.on_update_failed(core, generation, &error),
                    });
                });
                // Не в `finish`: ответ Discord на присутствие выход не ждёт.
                self.runtime.spawn(task);
                true
            }
            Err(error) => {
                self.on_update_failed(core, generation, &error);
                false
            }
        }
    }

    fn on_update_failed(&self, core: &mut Core, generation: u64, error: &RpcError) {
        if generation != core.generation {
            return;
        }
        let code = error_code_text(error);
        self.note(core, Code::UpdateFailed, code.as_deref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_grows_and_stops_at_five_minutes() {
        // random = 0.5: середина разброса — ровно основа.
        let bases: Vec<f64> = (0..10)
            .map(|attempt| retry_delay_ms(attempt, 0.5))
            .collect();
        assert_eq!(
            bases,
            [
                5000.0, 5000.0, 10000.0, 20000.0, 40000.0, 80000.0, 160000.0, 300000.0, 300000.0,
                300000.0
            ]
        );
        assert_eq!(retry_delay_ms(1, 0.0), 3750.0);
        assert_eq!(retry_delay_ms(1, 0.999999), 6250.0);
        assert_eq!(retry_delay_ms(100, 0.0), 225000.0);
        assert_eq!(retry_delay_ms(u32::MAX, 1.0), 375000.0);
    }
}
