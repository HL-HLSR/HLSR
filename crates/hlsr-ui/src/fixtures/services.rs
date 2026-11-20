//! `FixtureServices` (решение 16): главный процесс на ответах сцены — то же, что заглушка main
//! эталона (`SceneRun` в `tools/reference/lib/electron-screens.js`). Ответ канала — значение или
//! правило: `{"$seq": [...]}` (по номеру вызова, последний повторяется), `{"$args": [[[аргументы],
//! ответ], …], "$else": ответ}`, `{"$pending": true}` (не отвечает), `{"$reject": "текст"}`.
//! Канала нет в фикстуре — `null` и строка в [`FixtureServices::problems`].
//!
//! Журнал — `[{kind, channel, args}]`, как `ipc-log.json` эталона: `invoke`, `send`, `sendSync`,
//! `store-write`, `store-quarantine`, `emit`; чтения хранилищ не пишутся.

use super::scene::{Scene, Variant};
use crate::services::{BoxFuture, MainProcess, Reply, ServiceEvent, Standing, StoreRead};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use tokio::sync::broadcast;

/// Ответ фикстуры на вызов.
#[derive(Debug, Clone, PartialEq)]
pub enum FixtureReply {
    Value(Value),
    Pending,
    Reject(String),
    /// `$args` без совпадения и без `$else`: `null` и строка в проблемах.
    Unmatched,
}

/// `resolveReply` эталона.
pub fn resolve_reply(spec: &Value, index: usize, args: &[Value]) -> FixtureReply {
    if let Value::Object(map) = spec {
        if let Some(Value::Array(seq)) = map.get("$seq") {
            return match seq.get(index.min(seq.len().saturating_sub(1))) {
                Some(item) => resolve_reply(item, index, args),
                None => FixtureReply::Value(Value::Null),
            };
        }
        if let Some(Value::Array(cases)) = map.get("$args") {
            // JSON.stringify(want) === JSON.stringify(args).
            let key = serde_json::to_string(args).unwrap_or_default();
            for case in cases {
                if let Some([want, reply]) = case.as_array().map(Vec::as_slice)
                    && serde_json::to_string(want).unwrap_or_default() == key
                {
                    return resolve_reply(reply, index, args);
                }
            }
            return match map.get("$else") {
                Some(other) => resolve_reply(other, index, args),
                None => FixtureReply::Unmatched,
            };
        }
        if map.get("$pending") == Some(&Value::Bool(true)) {
            return FixtureReply::Pending;
        }
        if let Some(Value::String(text)) = map.get("$reject") {
            return FixtureReply::Reject(text.clone());
        }
    }
    FixtureReply::Value(spec.clone())
}

struct State {
    invoke: Map<String, Value>,
    send_sync: Map<String, Value>,
    store: Map<String, Value>,
    calls: HashMap<String, usize>,
    log: Vec<Value>,
    problems: Vec<String>,
    /// Стенд — официальная сборка, как 0.4.9 ([`FixtureServices::set_build`]).
    build: Standing,
}

/// Главный процесс сцены в одном варианте.
pub struct FixtureServices {
    state: Mutex<State>,
    events: broadcast::Sender<ServiceEvent>,
}

const EVENT_CAPACITY: usize = 256;

impl FixtureServices {
    /// Сцена в варианте: язык варианта — в `settings.config.language` хранилища и в
    /// `app-initial-language` (`en` = 0, `ru` = 1), как у эталона.
    pub fn new(scene: &Scene, variant: &Variant) -> Result<FixtureServices, String> {
        let language = variant.language()? as i64;
        let mut store = scene.store.clone();
        let mut settings = match store.remove("settings") {
            Some(Value::Object(map)) => map,
            _ => {
                let mut map = Map::new();
                map.insert("config".into(), json!({}));
                map
            }
        };
        let mut config = match settings.remove("config") {
            Some(Value::Object(map)) => map,
            _ => Map::new(),
        };
        config.insert("language".into(), json!(language));
        settings.insert("config".into(), Value::Object(config));
        store.insert("settings".into(), Value::Object(settings));
        let mut send_sync = scene.send_sync.clone();
        send_sync.insert("app-initial-language".into(), json!(language));
        Ok(FixtureServices {
            state: Mutex::new(State {
                invoke: scene.invoke.clone(),
                send_sync,
                store,
                calls: HashMap::new(),
                log: Vec::new(),
                problems: Vec::new(),
                build: Standing::Official,
            }),
            events: broadcast::channel(EVENT_CAPACITY).0,
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Журнал вызовов.
    pub fn log(&self) -> Vec<Value> {
        self.lock().log.clone()
    }

    /// Проблемы прогона: нет ответа фикстуры и т. п. (без повторов).
    pub fn problems(&self) -> Vec<String> {
        self.lock().problems.clone()
    }

    /// Сборка для окна до монтирования (тесты неофициальной сборки); перемену потом шлёт событие
    /// `build-official` ([`FixtureServices::emit`]).
    pub fn set_build(&self, build: Standing) {
        self.lock().build = build;
    }

    /// Событие окну (шаг `emit`, `__hlsrEmit` эталона).
    pub fn emit(&self, channel: &str, payload: Value) {
        record(&mut self.lock(), "emit", channel, vec![payload.clone()]);
        let _ = self.events.send(ServiceEvent {
            channel: channel.to_owned(),
            payload,
        });
    }

    fn reply(state: &mut State, sync: bool, channel: &str, args: &[Value]) -> FixtureReply {
        let index = {
            let count = state.calls.entry(channel.to_owned()).or_default();
            *count += 1;
            *count - 1
        };
        let table = if sync {
            &state.send_sync
        } else {
            &state.invoke
        };
        let Some(spec) = table.get(channel) else {
            problem(state, format!("нет ответа фикстуры: {channel}"));
            return FixtureReply::Value(Value::Null);
        };
        let reply = resolve_reply(spec, index, args);
        if reply == FixtureReply::Unmatched {
            let args = serde_json::to_string(args).unwrap_or_default();
            problem(
                state,
                format!("нет ответа фикстуры для аргументов: {channel} {args}"),
            );
        }
        reply
    }
}

fn record(state: &mut State, kind: &str, channel: &str, args: Vec<Value>) {
    state
        .log
        .push(json!({"kind": kind, "channel": channel, "args": args}));
}

fn problem(state: &mut State, text: String) {
    if !state.problems.contains(&text) {
        state.problems.push(text);
    }
}

impl MainProcess for FixtureServices {
    fn invoke(&self, channel: &str, args: Vec<Value>) -> BoxFuture<'static, Reply> {
        let mut state = self.lock();
        let reply = FixtureServices::reply(&mut state, false, channel, &args);
        record(&mut state, "invoke", channel, args);
        match reply {
            FixtureReply::Value(value) => Box::pin(std::future::ready(Ok(value))),
            FixtureReply::Unmatched => Box::pin(std::future::ready(Ok(Value::Null))),
            FixtureReply::Reject(text) => Box::pin(std::future::ready(Err(text))),
            FixtureReply::Pending => Box::pin(std::future::pending()),
        }
    }

    fn send(&self, channel: &str, args: Vec<Value>) {
        record(&mut self.lock(), "send", channel, args);
    }

    fn send_sync(&self, channel: &str, args: Vec<Value>) -> Value {
        let mut state = self.lock();
        let reply = FixtureServices::reply(&mut state, true, channel, &args);
        record(&mut state, "sendSync", channel, args);
        match reply {
            FixtureReply::Value(value) => value,
            _ => Value::Null,
        }
    }

    fn store_read(&self, name: &str) -> StoreRead {
        match self.lock().store.get(name) {
            Some(data) => StoreRead::Found(data.clone()),
            None => StoreRead::Missing,
        }
    }

    fn store_write(&self, name: &str, data: Value) -> bool {
        let mut state = self.lock();
        record(&mut state, "store-write", name, vec![data.clone()]);
        state.store.insert(name.to_owned(), data);
        true
    }

    fn store_quarantine(&self, name: &str) -> Value {
        record(&mut self.lock(), "store-quarantine", name, Vec::new());
        json!({"ok": false, "error": "EACCES"})
    }

    fn subscribe(&self) -> broadcast::Receiver<ServiceEvent> {
        self.events.subscribe()
    }

    fn build(&self) -> Standing {
        self.lock().build
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::{Context, Poll, Waker};

    fn now<T>(mut future: BoxFuture<'static, T>) -> Poll<T> {
        future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
    }

    fn scene(invoke: Value, send_sync: Value) -> Scene {
        serde_json::from_value(json!({
            "id": "t", "route": "/launcher", "platform": "win32",
            "window": {"width": 1600, "height": 900},
            "themes": ["hlsr"], "languages": ["ru", "en"], "scales": [1],
            "now": "2026-10-16T12:00:00.000Z", "timezone": "UTC",
            "store": {"settings": {"config": {"language": null, "uiScale": 1}}},
            "invoke": invoke, "sendSync": send_sync
        }))
        .unwrap()
    }

    fn variant(lang: &str) -> Variant {
        Variant {
            theme: "hlsr".into(),
            lang: lang.into(),
            scale: 1.0,
            zoom: None,
            rail: None,
        }
    }

    #[test]
    fn rules_follow_the_reference_stub() {
        let s = scene(
            json!({
                "plain": {"a": 1},
                "seq": {"$seq": [1, 2]},
                "byArgs": {"$args": [[["218"], "hl2"], [["70", null], "hl1"]], "$else": "other"},
                "strict": {"$args": [[["218"], "hl2"]]},
                "never": {"$pending": true},
                "no": {"$reject": "сбой"}
            }),
            json!({"app-version": "0.4.7"}),
        );
        let fx = FixtureServices::new(&s, &variant("ru")).unwrap();
        assert_eq!(
            now(fx.invoke("plain", vec![])),
            Poll::Ready(Ok(json!({"a": 1})))
        );
        assert_eq!(now(fx.invoke("seq", vec![])), Poll::Ready(Ok(json!(1))));
        assert_eq!(now(fx.invoke("seq", vec![])), Poll::Ready(Ok(json!(2))));
        assert_eq!(now(fx.invoke("seq", vec![])), Poll::Ready(Ok(json!(2))));
        assert_eq!(
            now(fx.invoke("byArgs", vec![json!("218")])),
            Poll::Ready(Ok(json!("hl2")))
        );
        assert_eq!(
            now(fx.invoke("byArgs", vec![json!("70"), Value::Null])),
            Poll::Ready(Ok(json!("hl1")))
        );
        assert_eq!(
            now(fx.invoke("byArgs", vec![json!("50")])),
            Poll::Ready(Ok(json!("other")))
        );
        assert_eq!(
            now(fx.invoke("strict", vec![json!("50")])),
            Poll::Ready(Ok(Value::Null))
        );
        assert_eq!(now(fx.invoke("never", vec![])), Poll::Pending);
        assert_eq!(
            now(fx.invoke("no", vec![])),
            Poll::Ready(Err("сбой".to_owned()))
        );
        assert_eq!(
            now(fx.invoke("missing", vec![])),
            Poll::Ready(Ok(Value::Null))
        );
        assert_eq!(fx.send_sync("app-version", vec![]), json!("0.4.7"));
        assert_eq!(fx.send_sync("app-initial-language", vec![]), json!(1));
        assert_eq!(
            fx.problems(),
            vec![
                "нет ответа фикстуры для аргументов: strict [\"50\"]".to_owned(),
                "нет ответа фикстуры: missing".to_owned()
            ]
        );
    }

    #[test]
    fn journal_has_the_reference_shape() {
        let s = scene(json!({"boot-gate-run": {"$pending": true}}), json!({}));
        let fx = FixtureServices::new(&s, &variant("en")).unwrap();
        fx.send_sync("app-default-library-path", vec![]);
        fx.send("ready", vec![]);
        assert_eq!(
            now(fx.invoke("boot-gate-run", vec![json!({})])),
            Poll::Pending
        );
        assert!(fx.store_write("library", json!({"x": 1})));
        fx.emit("updater-state", json!({"state": "idle"}));
        assert_eq!(
            Value::Array(fx.log()),
            json!([
                {"kind": "sendSync", "channel": "app-default-library-path", "args": []},
                {"kind": "send", "channel": "ready", "args": []},
                {"kind": "invoke", "channel": "boot-gate-run", "args": [{}]},
                {"kind": "store-write", "channel": "library", "args": [{"x": 1}]},
                {"kind": "emit", "channel": "updater-state", "args": [{"state": "idle"}]}
            ])
        );
        // Язык варианта — в хранилище, прочие поля настроек на месте.
        assert_eq!(
            fx.store_read("settings"),
            StoreRead::Found(json!({"config": {"language": 0, "uiScale": 1}}))
        );
        assert_eq!(fx.store_read("library"), StoreRead::Found(json!({"x": 1})));
        assert_eq!(fx.store_read("nope"), StoreRead::Missing);
    }

    #[test]
    fn emit_reaches_subscribers() {
        let s = scene(json!({}), json!({}));
        let fx = FixtureServices::new(&s, &variant("en")).unwrap();
        let mut rx = fx.subscribe();
        fx.emit("tool-status", json!({"toolId": "livesplit"}));
        assert_eq!(
            rx.try_recv().unwrap(),
            ServiceEvent {
                channel: "tool-status".into(),
                payload: json!({"toolId": "livesplit"})
            }
        );
    }
}
