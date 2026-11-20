//! Поддельный мир процессов для сценариев: живые PID, сведения о процессах, поиск по имени
//! образа, причина сбоя системы, результаты остановки. Одно состояние на инспектор и остановщик.

use hlsr_core::js::Platform;
use hlsr_delivery::errors::DeliveryError;
use hlsr_delivery::providers::BoxFuture;
use hlsr_platform::process_inspect::{ProcessInfo, ProcessInspector, same_path_on};
use hlsr_platform::process_stop::{StopResult, Stopper};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct World {
    pub alive: HashSet<u32>,
    /// Ответы `isAlive` для одного PID по очереди; последний повторяется.
    pub sequence: Option<(u32, VecDeque<bool>)>,
    pub inspect: HashMap<u32, Option<ProcessInfo>>,
    pub find: HashMap<String, Vec<ProcessInfo>>,
    /// `lastFailure()` инспектора.
    pub failure: Option<String>,
    pub stop_result: Option<StopResult>,
    pub kills_on_stop: bool,
    pub stop_calls: Vec<String>,
}

pub type Shared = Arc<Mutex<World>>;

pub fn world(shared: &Shared) -> std::sync::MutexGuard<'_, World> {
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct FakeInspector(pub Shared);

impl ProcessInspector for FakeInspector {
    fn is_alive(&self, pid: u32) -> bool {
        let mut world = world(&self.0);
        if let Some((target, values)) = &mut world.sequence
            && *target == pid
        {
            return if values.len() > 1 {
                values.pop_front().unwrap()
            } else {
                values[0]
            };
        }
        world.alive.contains(&pid)
    }

    fn inspect(&self, pid: u32) -> Option<ProcessInfo> {
        world(&self.0).inspect.get(&pid).cloned().flatten()
    }

    fn find_by_image_name(&self, image_name: &str) -> Vec<ProcessInfo> {
        world(&self.0)
            .find
            .get(image_name)
            .cloned()
            .unwrap_or_default()
    }

    fn last_failure(&self) -> Option<String> {
        world(&self.0).failure.clone()
    }

    // Сценарии сняты в Windows: пути сравниваются без учёта регистра на любой ОС.
    fn same_path(&self, left: Option<&str>, right: Option<&str>) -> bool {
        same_path_on(Platform::Win32, left, right)
    }
}

pub struct FakeStopper(pub Shared);

impl FakeStopper {
    fn call(&self, name: &str, pid: u32) -> BoxFuture<'_, StopResult> {
        let result = {
            let mut world = world(&self.0);
            world.stop_calls.push(format!("{name}({pid})"));
            if world.kills_on_stop {
                world.alive.remove(&pid);
            }
            world
                .stop_result
                .clone()
                .unwrap_or_else(|| StopResult::success(""))
        };
        Box::pin(async move { result })
    }
}

impl Stopper for FakeStopper {
    fn graceful(&self, pid: u32) -> BoxFuture<'_, StopResult> {
        self.call("graceful", pid)
    }
    fn force(&self, pid: u32) -> BoxFuture<'_, StopResult> {
        self.call("force", pid)
    }
    fn graceful_group(&self, pid: u32) -> BoxFuture<'_, StopResult> {
        self.call("gracefulGroup", pid)
    }
    fn force_group(&self, pid: u32) -> BoxFuture<'_, StopResult> {
        self.call("forceGroup", pid)
    }
}

/// Сведения о процессе из фикстуры (`null` — процесса нет).
pub fn info_of(value: &Value) -> Option<ProcessInfo> {
    if value.is_null() {
        return None;
    }
    let text = |key: &str| value[key].as_str().map(str::to_owned);
    Some(ProcessInfo {
        pid: value["pid"].as_u64().unwrap() as u32,
        executable_path: text("executablePath"),
        name: text("name"),
        started_at: text("startedAt"),
        source: text("source"),
    })
}

/// `{code, message, details}` ошибки доставки, как её записывает сценарий.
pub fn error_value(error: &DeliveryError) -> Value {
    json!({
        "code": error.code.as_str(),
        "message": error.message,
        "details": error.details.clone().unwrap_or(Value::Null),
    })
}
