//! Живой прогон каналов (dev-hooks, V2): настоящий лаунчер с настоящими фасадами, но вместо
//! щелчков по окну — сценарий из JSON. `HLSR_LIVE_SCRIPT` — файл `{"steps": [...]}`,
//! `HLSR_LIVE_OUT` — журнал прогона (ответы каналов, все события окна, итог шагов, ошибка
//! сценария), пишется после последнего шага; затем лаунчер выходит, как по «Выход» в трее, с
//! кодом 0, только если все шаги прошли ([`exit_code`]).
//!
//! Шаг — один из:
//! - `{"invoke": канал, "args": [...], "expect": {...}}` — вызов, как `ipcRenderer.invoke` окна;
//!   `expect` — подмножество ответа (объекты сравниваются по своим ключам); строка
//!   `"$/reply/token"` в аргументах — значение по указателю JSON из итога прошлого шага (нет
//!   значения — шаг не прошёл);
//! - `{"wait": событие, "until": {...}, "timeoutMs": N}` — первое событие после прошлого
//!   найденного, чья нагрузка содержит `until`; `"failOn": событие` — если раньше придёт
//!   оно, шаг не прошёл;
//! - `{"sleepMs": N}`;
//! - `{"snapshot": "путь.png"}` — кадр окна лаунчера;
//! - `{"dialog": "путь"}` — ответ следующего диалога выбора или сохранения файла вместо окна
//!   системного диалога (на невидимом столе его некому закрыть).

use hlsr_core::error::exit;
use hlsr_gpui::bridge;
use hlsr_ui::services::MainProcess;
use hlsr_ui::services::router::ChannelRouter;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::runtime::Handle;
use tokio::sync::broadcast::error::RecvError;

/// Ожидание события по умолчанию.
const WAIT_DEFAULT: Duration = Duration::from_secs(60);
/// Как часто смотреть журнал событий, пока ждём.
const WAIT_POLL: Duration = Duration::from_millis(100);

/// Код выхода, если сценарий не прочитан, шаг не прошёл или лаунчер вышел раньше конца
/// (2 свободен, см. `hlsr_core::error::exit`).
const FAILED: i32 = 2;

/// Код выхода лаунчера по итогу сценария.
static EXIT_CODE: AtomicI32 = AtomicI32::new(exit::OK);

/// Код выхода по итогу сценария; без сценария — 0.
pub fn exit_code() -> i32 {
    EXIT_CODE.load(Ordering::Relaxed)
}

/// Ответы диалогов (шаг `dialog`) по порядку.
static DIALOGS: Mutex<VecDeque<PathBuf>> = Mutex::new(VecDeque::new());

/// Ответ диалога из сценария, если он задан.
pub fn take_dialog() -> Option<PathBuf> {
    DIALOGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pop_front()
}

/// Запустить сценарий, если задан `HLSR_LIVE_SCRIPT`. Вызывать после [`crate::services::start`].
pub fn start(runtime: &Handle, router: &Arc<ChannelRouter>) {
    let Some(script) = crate::modes::dev_env("HLSR_LIVE_SCRIPT") else {
        return;
    };
    let out = crate::modes::dev_env("HLSR_LIVE_OUT").map_or_else(
        || PathBuf::from(format!("{script}.out.json")),
        PathBuf::from,
    );
    // Пока сценарий не прошёл целиком — неудача, в том числе при выходе раньше последнего шага.
    EXIT_CODE.store(FAILED, Ordering::Relaxed);
    let router = router.clone();
    // Подписка — до первого шага: события установки идут, пока ждём ответа `invoke`.
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let started = Instant::now();
    let mut source = router.subscribe();
    let sink = events.clone();
    runtime.spawn(async move {
        loop {
            let received = source.recv().await;
            let ms = started.elapsed().as_millis() as u64;
            let event = match received {
                Ok(event) => json!({"t": ms, "channel": event.channel, "payload": event.payload}),
                // Отстали от очереди: пропущенных не вернуть, запись идёт дальше с отметкой.
                Err(RecvError::Lagged(missed)) => json!({"t": ms, "lagged": missed}),
                Err(RecvError::Closed) => break,
            };
            sink.lock().unwrap_or_else(|e| e.into_inner()).push(event);
        }
    });
    runtime.spawn(async move {
        let mut results = Vec::new();
        let error = match load(&script) {
            Ok(steps) => {
                tracing::info!("[live] сценарий {script}: шагов {}", steps.len());
                let mut cursor = 0usize;
                let mut last = Value::Null;
                for (index, step) in steps.iter().enumerate() {
                    let at = started.elapsed().as_millis() as u64;
                    let result = run_step(step, &last, &router, &events, &mut cursor).await;
                    last = result.clone();
                    tracing::info!("[live] шаг {index}: {}", result["ok"]);
                    results.push(json!({"index": index, "t": at, "step": step, "result": result}));
                }
                None
            }
            Err(error) => {
                tracing::error!("[live] {script}: {error}");
                Some(error)
            }
        };
        let passed = results.iter().filter(|r| r["result"]["ok"] == true).count();
        if error.is_none() && passed == results.len() {
            EXIT_CODE.store(exit::OK, Ordering::Relaxed);
        }
        let mut journal = json!({"passed": passed, "total": results.len(), "error": error});
        journal["steps"] = Value::Array(results);
        // Без копии: после последнего шага журнал событий больше не нужен.
        journal["events"] = Value::Array(std::mem::take(
            &mut *events.lock().unwrap_or_else(|e| e.into_inner()),
        ));
        write(&out, &journal);
        tracing::info!("[live] сценарий окончен, журнал {}", out.display());
        bridge::quit();
    });
}

async fn run_step(
    step: &Value,
    last: &Value,
    router: &Arc<ChannelRouter>,
    events: &Arc<Mutex<Vec<Value>>>,
    cursor: &mut usize,
) -> Value {
    if let Some(channel) = step["invoke"].as_str() {
        let args = match resolve(&step["args"], last) {
            Ok(Value::Array(args)) => args,
            Ok(_) => Vec::new(),
            Err(error) => return json!({"ok": false, "error": error}),
        };
        let begun = Instant::now();
        let reply = router.invoke(channel, args).await;
        let ms = begun.elapsed().as_millis() as u64;
        return match reply {
            Ok(value) => {
                let ok = step.get("expect").is_none_or(|want| contains(&value, want));
                json!({"ok": ok, "ms": ms, "reply": value})
            }
            Err(error) => json!({"ok": false, "ms": ms, "error": error}),
        };
    }
    if let Some(channel) = step["wait"].as_str() {
        let until = step.get("until").cloned().unwrap_or(Value::Null);
        let timeout = step["timeoutMs"]
            .as_u64()
            .map_or(WAIT_DEFAULT, Duration::from_millis);
        let fail = step["failOn"].as_str();
        let begun = Instant::now();
        loop {
            let found = {
                let events = events.lock().unwrap_or_else(|e| e.into_inner());
                events
                    .iter()
                    .enumerate()
                    .skip(*cursor)
                    .find(|(_, event)| {
                        (event["channel"] == channel
                            && (until.is_null() || contains(&event["payload"], &until)))
                            || (fail.is_some() && event["channel"].as_str() == fail)
                    })
                    .map(|(index, event)| (index, event.clone()))
            };
            if let Some((index, event)) = found {
                *cursor = index + 1;
                let ok = event["channel"] == channel;
                return json!({"ok": ok, "ms": begun.elapsed().as_millis() as u64, "event": event});
            }
            if begun.elapsed() >= timeout {
                return json!({"ok": false, "error": "timeout"});
            }
            tokio::time::sleep(WAIT_POLL).await;
        }
    }
    if let Some(path) = step["dialog"].as_str() {
        DIALOGS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(PathBuf::from(path));
        return json!({"ok": true});
    }
    if let Some(ms) = step["sleepMs"].as_u64() {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        return json!({"ok": true});
    }
    if let Some(path) = step["snapshot"].as_str().map(str::to_owned) {
        let saved = bridge::call(move |cx| {
            let launcher = crate::window::launcher(cx).ok_or("окна нет")?;
            launcher
                .update(cx, |_, window, _| window.render_to_image())
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?
                .save(&path)
                .map_err(|e| e.to_string())
        })
        .await;
        return match saved {
            Some(Ok(())) => json!({"ok": true}),
            Some(Err(error)) => json!({"ok": false, "error": error}),
            None => json!({"ok": false, "error": "цикла событий нет"}),
        };
    }
    json!({"ok": false, "error": "неизвестный шаг"})
}

/// Шаги сценария: файл `{"steps": [...]}`, хотя бы один шаг.
fn load(script: &str) -> Result<Vec<Value>, String> {
    let text = std::fs::read_to_string(script).map_err(|e| e.to_string())?;
    let mut value = serde_json::from_str::<Value>(&text).map_err(|e| e.to_string())?;
    match value.get_mut("steps").map(Value::take) {
        Some(Value::Array(steps)) if !steps.is_empty() => Ok(steps),
        _ => Err("нет шагов: нужен {\"steps\": [...]}".to_owned()),
    }
}

/// Строки `"$<указатель JSON>"` — значения из итога прошлого шага; нет значения — ошибка.
fn resolve(value: &Value, last: &Value) -> Result<Value, String> {
    Ok(match value {
        Value::String(text) => match text.strip_prefix('$') {
            Some(pointer) => last
                .pointer(pointer)
                .cloned()
                .ok_or_else(|| format!("в итоге прошлого шага нет {text}"))?,
            None => value.clone(),
        },
        Value::Array(list) => Value::Array(
            list.iter()
                .map(|item| resolve(item, last))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| Ok((key.clone(), resolve(item, last)?)))
                .collect::<Result<_, String>>()?,
        ),
        _ => value.clone(),
    })
}

/// `want` — подмножество `have`: у объектов сверяются только ключи `want`.
fn contains(have: &Value, want: &Value) -> bool {
    match (have, want) {
        (Value::Object(have), Value::Object(want)) => want
            .iter()
            .all(|(key, want)| have.get(key).is_some_and(|have| contains(have, want))),
        _ => have == want,
    }
}

fn write(out: &PathBuf, journal: &Value) {
    if let Ok(text) = serde_json::to_string_pretty(journal)
        && let Err(error) = std::fs::write(out, text + "\n")
    {
        tracing::warn!("[live] журнал {} не записан: {error}", out.display());
    }
}

#[cfg(test)]
mod tests {
    use super::{contains, resolve};
    use serde_json::json;

    #[test]
    fn expect_is_a_subset_of_the_reply() {
        let reply = json!({"status": "ok", "game": {"id": "hl1-won", "state": "ready"}});
        assert!(contains(&reply, &json!({"status": "ok"})));
        assert!(contains(&reply, &json!({"game": {"state": "ready"}})));
        assert!(!contains(&reply, &json!({"game": {"state": "busy"}})));
        assert!(!contains(&reply, &json!({"missing": null})));
    }

    #[test]
    fn dollar_strings_take_values_from_the_last_step() {
        let last = json!({"reply": {"token": "t1", "demos": [{"dir": "a", "name": "b"}]}});
        assert_eq!(
            resolve(
                &json!(["$/reply/token", ["$/reply/demos/0"], {"x": "$"}, "plain"]),
                &last
            ),
            Ok(json!(["t1", [{"dir": "a", "name": "b"}], {"x": last}, "plain"]))
        );
        assert!(resolve(&json!([{"x": "$/nope"}]), &last).is_err());
        assert!(resolve(&json!(["$HOME/demo"]), &last).is_err());
    }
}
