//! Подделки для тестов обновления: служба обновления, которую ведёт сценарий.

use hlsr_update::boot_gate::{BootGateHost, Subscriber, UpdateState};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Callback = Arc<dyn Fn(UpdateState) + Send + Sync>;

#[derive(Default)]
struct Inner {
    state: Mutex<Option<UpdateState>>,
    subscribers: Mutex<Vec<(u64, Callback)>>,
    calls: Mutex<Vec<&'static str>>,
    next_id: AtomicU64,
}

impl Inner {
    fn emit(&self, state: UpdateState) {
        *self.state.lock().unwrap() = Some(state);
        // Подписчик может отписаться изнутри: список копируется.
        let list: Vec<Callback> = self
            .subscribers
            .lock()
            .unwrap()
            .iter()
            .map(|(_, callback)| callback.clone())
            .collect();
        for callback in list {
            callback(state);
        }
    }
}

/// Строка состояния из фикстуры.
pub fn state_of(name: &str) -> UpdateState {
    match name {
        "idle" => UpdateState::Idle,
        "checking" => UpdateState::Checking,
        "available" => UpdateState::Available,
        "not-available" => UpdateState::NotAvailable,
        "downloading" => UpdateState::Downloading,
        "verifying" => UpdateState::Verifying,
        "downloaded" => UpdateState::Downloaded,
        "error" => UpdateState::Error,
        other => panic!("состояние {other}"),
    }
}

/// Служба обновления, чьи ответы заданы сценарием фикстуры `gate.json`: на `check()`,
/// `download()` и `install()` через заданное время приходят заданные состояния.
pub struct ScriptedHost {
    inner: Arc<Inner>,
    spec: Value,
}

impl ScriptedHost {
    pub fn new(spec: &Value) -> ScriptedHost {
        let inner = Inner::default();
        *inner.state.lock().unwrap() = Some(state_of(spec["initialState"].as_str().unwrap()));
        ScriptedHost {
            inner: Arc::new(inner),
            spec: spec.clone(),
        }
    }

    pub fn calls(&self) -> Vec<&'static str> {
        self.inner.calls.lock().unwrap().clone()
    }

    pub fn subscribers(&self) -> usize {
        self.inner.subscribers.lock().unwrap().len()
    }

    fn call(&self, name: &'static str, script: &str) {
        self.inner.calls.lock().unwrap().push(name);
        for step in self.spec[script].as_array().unwrap() {
            let delay = Duration::from_millis(step[0].as_u64().unwrap());
            let state = state_of(step[1].as_str().unwrap());
            let inner = self.inner.clone();
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                inner.emit(state);
            });
        }
    }
}

impl BootGateHost for ScriptedHost {
    fn enabled(&self) -> bool {
        self.spec["enabled"].as_bool().unwrap()
    }

    fn state(&self) -> UpdateState {
        self.inner.state.lock().unwrap().unwrap()
    }

    fn subscribe(&self, subscriber: Subscriber) -> Box<dyn FnOnce() + Send> {
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        self.inner
            .subscribers
            .lock()
            .unwrap()
            .push((id, Arc::from(subscriber)));
        let inner = self.inner.clone();
        Box::new(move || {
            inner
                .subscribers
                .lock()
                .unwrap()
                .retain(|(existing, _)| *existing != id);
        })
    }

    fn check(&self) {
        self.call("check", "onCheck");
    }

    fn download(&self) {
        self.call("download", "onDownload");
    }

    fn install(&self) {
        self.call("install", "onInstall");
    }

    fn is_game_running(&self) -> bool {
        self.spec["game"].as_bool().unwrap()
    }

    fn version(&self) -> Option<String> {
        self.spec["version"].as_str().map(str::to_owned)
    }

    fn error(&self) -> Option<String> {
        self.spec["error"].as_str().map(str::to_owned)
    }

    fn is_mandatory(&self) -> bool {
        self.spec["mandatory"].as_bool().unwrap()
    }
}
