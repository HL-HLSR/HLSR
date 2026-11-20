//! Главная кнопка карточки игры по статусу доставки, процесса и обновления
//! (`main-button.js`): что она делает (`action`), включена ли, крутится ли и показывает ли
//! прогресс.

use crate::js_value as jv;
use crate::progress_states::BUSY_STATES;
use serde_json::{Map, Value, json};

/// `ACTIONS`.
pub const ACTIONS: [&str; 10] = [
    "play", "stop", "install", "update", "pause", "resume", "retry", "repair", "explain", "none",
];

/// `UPDATE_OPERATIONS`: операции, при которых кнопка говорит об обновлении.
pub const UPDATE_OPERATIONS: [&str; 2] = ["update", "repair-download"];

/// Входные данные `resolve`: статус доставки, процесса, обновления и идёт ли проверка.
#[derive(Debug, Clone, Copy, Default)]
pub struct Input<'a> {
    pub delivery: Option<&'a Value>,
    pub process: Option<&'a Value>,
    pub update: Option<&'a Value>,
    pub checking_updates: bool,
}

#[derive(Default)]
struct Flags {
    spinner: bool,
    progress: bool,
    extra: Vec<(&'static str, Value)>,
}

fn spinner() -> Flags {
    Flags {
        spinner: true,
        ..Flags::default()
    }
}

fn progress() -> Flags {
    Flags {
        progress: true,
        ..Flags::default()
    }
}

fn spinner_progress() -> Flags {
    Flags {
        spinner: true,
        progress: true,
        ..Flags::default()
    }
}

fn extra(fields: Vec<(&'static str, Value)>) -> Flags {
    Flags {
        extra: fields,
        ..Flags::default()
    }
}

/// `state(name, action, options)`.
fn state(name: &str, action: &str, flags: Flags) -> Value {
    let mut out = Map::new();
    out.insert("state".to_owned(), json!(name));
    out.insert("action".to_owned(), json!(action));
    out.insert("enabled".to_owned(), json!(action != "none"));
    out.insert("spinner".to_owned(), json!(flags.spinner));
    out.insert("progress".to_owned(), json!(flags.progress));
    for (key, value) in flags.extra {
        out.insert(key.to_owned(), value);
    }
    Value::Object(out)
}

fn state_of(value: &Value) -> Option<&str> {
    jv::prop(value, "state").and_then(Value::as_str)
}

fn from_process(process: &Value) -> Option<Value> {
    match state_of(process) {
        Some("stopping") => Some(state("stopping-game", "none", spinner())),
        Some("starting") => Some(state("starting-game", "none", spinner())),
        Some("running") => Some(state("game-running", "stop", Flags::default())),
        _ => None,
    }
}

fn from_update_operation(delivery: &Value) -> Option<Value> {
    match state_of(delivery) {
        Some("downloading") => Some(state("downloading-update", "pause", progress())),
        Some("paused") => Some(state("update-paused", "resume", progress())),
        Some("preparing" | "verifying") => {
            Some(state("verifying-update", "none", spinner_progress()))
        }
        Some("extracting" | "finalizing" | "applying-update") => {
            Some(state("applying-update", "none", spinner_progress()))
        }
        Some("error") => Some(state("update-error", "retry", Flags::default())),
        _ => None,
    }
}

fn from_install_operation(delivery: &Value, installed: bool) -> Option<Value> {
    match state_of(delivery) {
        Some("downloading") => Some(state("downloading", "pause", progress())),
        Some("paused") => Some(state("download-paused", "resume", progress())),
        Some("preparing" | "verifying" | "extracting" | "finalizing") => {
            Some(state("installing", "none", spinner_progress()))
        }
        Some("checking-files" | "repairing" | "exporting" | "importing") => {
            Some(state("busy", "none", spinner_progress()))
        }
        Some("error") if !installed => Some(state("error", "retry", Flags::default())),
        _ => None,
    }
}

/// `x || fallback` для значения JS.
fn or(value: Option<&Value>, fallback: Value) -> Value {
    value
        .filter(|value| hlsr_core::js::truthy(value))
        .cloned()
        .unwrap_or(fallback)
}

/// `resolve(input)`.
pub fn resolve(input: &Input) -> Value {
    let empty = Value::Object(Map::new());
    let not_running = json!({"state": "not-running"});
    let delivery = input
        .delivery
        .filter(|value| hlsr_core::js::truthy(value))
        .unwrap_or(&empty);
    let process = input
        .process
        .filter(|value| hlsr_core::js::truthy(value))
        .unwrap_or(&not_running);
    let update = input.update.filter(|value| hlsr_core::js::truthy(value));
    let installed = jv::truthy(jv::prop(delivery, "installedVersion"));

    if let Some(running) = from_process(process) {
        return running;
    }

    let delivery_state = state_of(delivery);
    let busy = delivery_state.is_some_and(|state| BUSY_STATES.contains(&state))
        || delivery_state == Some("paused");
    let updating = jv::prop(delivery, "operation")
        .and_then(Value::as_str)
        .is_some_and(|operation| UPDATE_OPERATIONS.contains(&operation));

    if busy || delivery_state == Some("error") {
        let resolved = if updating {
            from_update_operation(delivery)
        } else {
            from_install_operation(delivery, installed)
        };
        if let Some(resolved) = resolved {
            return resolved;
        }
    }

    if installed && delivery_state == Some("files-damaged") {
        // `x ? x.field : null`: поля нет — `undefined`, в JSON его нет.
        let field = |name: &str, field: &str| -> Option<Value> {
            match jv::prop(delivery, name).filter(|v| hlsr_core::js::truthy(v)) {
                None => Some(Value::Null),
                Some(value) => jv::prop(value, field).cloned(),
            }
        };
        let mut fields = Vec::new();
        if let Some(damaged) = field("integrity", "damaged") {
            fields.push(("damaged", damaged));
        }
        if let Some(reason) = field("metadataIssue", "code") {
            fields.push(("reason", reason));
        }
        return state("repair-required", "repair", extra(fields));
    }

    if installed
        && jv::is_str(
            jv::prop(delivery, "metadataState"),
            "launcher-update-required",
        )
    {
        let reason = or(
            jv::prop(delivery, "unavailableReason"),
            json!("BUILD_REQUIRES_NEWER_LAUNCHER"),
        );
        return state(
            "update-launcher-required",
            "explain",
            extra(vec![("reason", reason)]),
        );
    }

    if input.checking_updates {
        return state("checking-updates", "none", spinner());
    }

    if installed && update.is_some_and(|update| jv::truthy(jv::prop(update, "updateAvailable"))) {
        let update = update.expect("проверено выше");
        return state(
            "update-available",
            "update",
            extra(vec![
                (
                    "required",
                    json!(jv::truthy(jv::prop(update, "updateRequired"))),
                ),
                (
                    "availableVersion",
                    or(jv::prop(update, "availableVersion"), Value::Null),
                ),
            ]),
        );
    }

    if installed {
        return state("ready", "play", Flags::default());
    }

    if delivery_state == Some("error") {
        return state("error", "retry", Flags::default());
    }

    if jv::prop(delivery, "installable") == Some(&Value::Bool(false)) {
        let reason = or(
            jv::prop(delivery, "unavailableReason"),
            json!("UNKNOWN_UNAVAILABLE"),
        );
        return state("unavailable", "explain", extra(vec![("reason", reason)]));
    }

    state("not-installed", "install", Flags::default())
}
