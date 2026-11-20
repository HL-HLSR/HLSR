//! Решение экрана загрузки (`launcher-update/boot-gate.js`, только Windows): проверка обновления
//! с тайм-аутом, затем — скачать, установить, пропустить или заблокировать запуск.
//!
//! Автомат реагирует на события состояния службы обновления прямо в её уведомлении (как подписчик
//! 0.4.7) и принимает решение один раз. После `available` тайм-аут снимается (загрузка идёт на
//! экране загрузки); после `downloaded` установка запускается уже после решения.

use crate::ports::Logger;
use serde_json::{Map, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

/// `DEFAULT_TIMEOUT_MS`: сколько ждать ответа проверки на экране загрузки.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(10_000);

/// `STATES`: состояния службы обновления.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpdateState {
    Idle,
    Checking,
    Available,
    NotAvailable,
    Downloading,
    Verifying,
    Downloaded,
    Error,
}

impl UpdateState {
    /// Строка состояния, как её видит окно (`updater-state`).
    pub const fn as_str(self) -> &'static str {
        match self {
            UpdateState::Idle => "idle",
            UpdateState::Checking => "checking",
            UpdateState::Available => "available",
            UpdateState::NotAvailable => "not-available",
            UpdateState::Downloading => "downloading",
            UpdateState::Verifying => "verifying",
            UpdateState::Downloaded => "downloaded",
            UpdateState::Error => "error",
        }
    }
}

/// Подписка на состояние: пока жива, обратный вызов получает каждое новое состояние.
pub type Subscriber = Box<dyn Fn(UpdateState) + Send + Sync>;

/// Служба обновления с точки зрения экрана загрузки (`host` в 0.4.7).
pub trait BootGateHost: Send + Sync {
    /// Обновления вообще включены (сборка, ОС, режим).
    fn enabled(&self) -> bool;
    fn state(&self) -> UpdateState;
    /// Подписаться на состояния; возвращённая функция отписывает.
    fn subscribe(&self, subscriber: Subscriber) -> Box<dyn FnOnce() + Send>;
    fn check(&self);
    fn download(&self);
    fn install(&self);
    fn is_game_running(&self) -> bool;
    /// Версия скачанного обновления.
    fn version(&self) -> Option<String>;
    fn error(&self) -> Option<String>;
    /// Обновление обязательное (`updateRequired` канала): ошибка блокирует запуск.
    fn is_mandatory(&self) -> bool;
}

/// Что делать экрану загрузки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootAction {
    Proceed,
    Installing,
    Blocked,
}

impl BootAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            BootAction::Proceed => "proceed",
            BootAction::Installing => "installing",
            BootAction::Blocked => "blocked",
        }
    }
}

/// `{action, reason, version?, error?, warn?, mandatory?}` — ответ канала `boot-gate-run`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootDecision {
    pub action: BootAction,
    pub reason: &'static str,
    pub version: Option<String>,
    pub error: Option<String>,
    pub warn: Option<&'static str>,
    pub mandatory: Option<bool>,
}

impl BootDecision {
    fn new(action: BootAction, reason: &'static str) -> BootDecision {
        BootDecision {
            action,
            reason,
            version: None,
            error: None,
            warn: None,
            mandatory: None,
        }
    }

    /// Форма 0.4.7: ключи в порядке объекта, лишних нет.
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("action".into(), self.action.as_str().into());
        map.insert("reason".into(), self.reason.into());
        // У решения `installing` версия есть всегда: `null`, если служба её не назвала.
        if self.action == BootAction::Installing || self.version.is_some() {
            map.insert(
                "version".into(),
                self.version.clone().map_or(Value::Null, Value::from),
            );
        }
        if let Some(mandatory) = self.mandatory {
            map.insert("mandatory".into(), mandatory.into());
        }
        if let Some(warn) = self.warn {
            map.insert("warn".into(), warn.into());
        }
        if let Some(error) = &self.error {
            map.insert("error".into(), error.clone().into());
        }
        Value::Object(map)
    }
}

/// Автомат решения: реагирует на состояние службы сразу, внутри её уведомления (как подписчик
/// 0.4.7, который вызывает `download()`/`install()` прямо из `send`), и один раз принимает решение.
struct Machine {
    host: Arc<dyn BootGateHost>,
    log: Arc<dyn Logger>,
    decision: Mutex<Option<BootDecision>>,
    /// `checkTimer` ещё взведён: после `available` и после решения он снят.
    timer_armed: AtomicBool,
    unsubscribe: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    settled: Notify,
}

impl Machine {
    fn decided(&self) -> Option<BootDecision> {
        self.decision
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// `settle(decision)`: только первое решение; таймер снят, подписка снята.
    fn settle(&self, decision: BootDecision) {
        {
            let mut slot = self
                .decision
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.is_some() {
                return;
            }
            *slot = Some(decision);
        }
        self.timer_armed.store(false, Ordering::SeqCst);
        let unsubscribe = self
            .unsubscribe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(unsubscribe) = unsubscribe {
            unsubscribe();
        }
        self.settled.notify_one();
    }

    fn on_state(&self, state: UpdateState) {
        if self.decided().is_some() {
            return;
        }
        match state {
            UpdateState::NotAvailable => {
                self.settle(BootDecision::new(BootAction::Proceed, "no-update"));
            }
            UpdateState::Available => {
                self.timer_armed.store(false, Ordering::SeqCst);
                self.log
                    .info("[boot] update available — downloading on loading screen");
                self.host.download();
            }
            UpdateState::Downloaded => {
                if self.host.is_game_running() {
                    self.settle(BootDecision::new(BootAction::Proceed, "game-running"));
                    return;
                }
                self.log.info("[boot] verified update ready — installing");
                let mut decision = BootDecision::new(BootAction::Installing, "installing");
                decision.version = self.host.version();
                self.settle(decision);
                self.host.install();
            }
            UpdateState::Error => {
                let error = self
                    .host
                    .error()
                    .filter(|e| !e.is_empty())
                    .unwrap_or_else(|| "unknown".to_owned());
                if self.host.is_mandatory() {
                    let mut decision =
                        BootDecision::new(BootAction::Blocked, "mandatory-update-failed");
                    decision.mandatory = Some(true);
                    decision.error = Some(error);
                    self.settle(decision);
                    return;
                }
                let mut decision = BootDecision::new(BootAction::Proceed, "update-error");
                decision.warn = Some("LU_FEED_UNAVAILABLE");
                decision.error = Some(error);
                self.settle(decision);
            }
            UpdateState::Idle
            | UpdateState::Checking
            | UpdateState::Downloading
            | UpdateState::Verifying => {}
        }
    }
}

/// `runBootGate(host)`. `timeout` — `host.timeoutMs` (по умолчанию [`DEFAULT_TIMEOUT`]).
pub async fn run_boot_gate(
    host: Arc<dyn BootGateHost>,
    log: Arc<dyn Logger>,
    timeout: Duration,
) -> BootDecision {
    if !host.enabled() {
        return BootDecision::new(BootAction::Proceed, "updates-disabled");
    }

    // Порядок 0.4.7: сначала `checkTimer = setTimeout(...)`, затем подписка и `host.check()`. Срок
    // отсчитывается от создания таймера, а не от возврата из `check()`.
    let timer = tokio::time::sleep(timeout);
    tokio::pin!(timer);

    let machine = Arc::new(Machine {
        host: host.clone(),
        log,
        decision: Mutex::new(None),
        timer_armed: AtomicBool::new(true),
        unsubscribe: Mutex::new(None),
        settled: Notify::new(),
    });
    let weak = Arc::downgrade(&machine);
    let unsubscribe = host.subscribe(Box::new(move |state| {
        if let Some(machine) = weak.upgrade() {
            machine.on_state(state);
        }
    }));
    *machine
        .unsubscribe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(unsubscribe);
    host.check();

    let mut timer_pending = true;
    loop {
        if let Some(decision) = machine.decided() {
            return decision;
        }
        tokio::select! {
            biased;
            () = machine.settled.notified() => {}
            () = &mut timer, if timer_pending => {
                timer_pending = false;
                if machine.timer_armed.load(Ordering::SeqCst)
                    && matches!(host.state(), UpdateState::Checking | UpdateState::Idle)
                {
                    machine
                        .log
                        .warn("[boot] update check timed out — starting current version");
                    let mut decision = BootDecision::new(BootAction::Proceed, "check-timeout");
                    decision.warn = Some("UPDATE_CHECK_TIMEOUT");
                    machine.settle(decision);
                }
            }
        }
    }
}
