//! Состояние игры для Discord и `GamePresenceBridge` против 0.4.7: цепочки сообщений
//! (`tools/reference/platform/presence-state.js`) → новое состояние, `changed`, `ignored`,
//! решение о публикации, снимки, которые получил обработчик, и внутреннее состояние моста.
//! Чат в игре вырезан владельцем (2026-10-16): потребителей чата у моста нет.

mod support;

use hlsr_delivery::ports::MemoryLogger;
use hlsr_delivery::timing::Clock;
use hlsr_platform::game_presence::game_state::{self, GameState};
use hlsr_platform::game_presence::protocol::{Lifecycle, StateMessage};
use hlsr_platform::game_presence::{BridgeOptions, GamePresenceBridge};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use support::{Mismatches, fixture, items, str_of};

/// Сообщение сценария; не `state` (или `null`) — `None`.
fn message_of(value: &Value) -> Option<StateMessage> {
    if value["type"] != json!("state") {
        return None;
    }
    let lifecycle = match str_of(&value["lifecycle"]) {
        "menu" => Lifecycle::Menu,
        "playing" => Lifecycle::Playing,
        _ => Lifecycle::Exited,
    };
    Some(StateMessage {
        session: str_of(&value["session"]).to_owned(),
        seq: value["seq"].as_f64().unwrap(),
        lifecycle,
        map_name: value["mapName"].as_str().map(str::to_owned),
        fps: value["fps"].as_i64(),
        timer_running: value["timerRunning"].as_bool().unwrap(),
        timer_completed: value["timerCompleted"].as_bool().unwrap(),
        elapsed_ms: value["elapsedMs"].as_i64(),
    })
}

#[test]
fn apply_like_0_4_7() {
    let fixture = fixture("presence-state.json");
    let mut mismatches = Mismatches::default();
    for scenario in items(&fixture["apply"]) {
        let name = str_of(&scenario["name"]);
        let mut state = GameState::empty();
        for step in items(&scenario["steps"]) {
            let at = step["at"].as_i64().unwrap();
            let previous = state.clone();
            let applied = game_state::apply(&previous, message_of(&step["message"]).as_ref(), at);
            state = applied.state;
            let context = format!("{name} @{at}");
            mismatches.check(
                &state.to_value(),
                &step["state"],
                format!("{context}: состояние"),
            );
            mismatches.check_eq(
                applied.changed,
                step["changed"].as_bool().unwrap(),
                format!("{context}: changed"),
            );
            mismatches.check_eq(
                applied.ignored,
                step["ignored"].as_str(),
                format!("{context}: ignored"),
            );
            mismatches.check_eq(
                game_state::same_state(&previous, &state),
                step["same"].as_bool().unwrap(),
                format!("{context}: sameState"),
            );
            mismatches.check_eq(
                game_state::same_except_fps(&previous, &state),
                step["sameExceptFps"].as_bool().unwrap(),
                format!("{context}: sameExceptFps"),
            );
            let publish: Vec<bool> = [None, Some(0), Some(14_999), Some(15_000)]
                .into_iter()
                .map(|last| game_state::should_publish(Some(&previous), &state, last, at))
                .collect();
            let expected: Vec<bool> = items(&step["publish"])
                .iter()
                .map(|v| v.as_bool().unwrap())
                .collect();
            mismatches.check_eq(publish, expected, format!("{context}: shouldPublish"));
        }
    }
    mismatches.finish("game-state");
    assert_eq!(
        game_state::should_publish(None, &GameState::empty(), None, 0),
        fixture["firstPublish"].as_bool().unwrap()
    );
}

struct Harness {
    clock: Arc<AtomicI64>,
    emitted: Arc<Mutex<Vec<Value>>>,
    logger: Arc<MemoryLogger>,
    bridge: Arc<GamePresenceBridge>,
}

fn harness() -> Harness {
    let clock = Arc::new(AtomicI64::new(0));
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let logger = Arc::new(MemoryLogger::default());
    let now: Clock = {
        let clock = clock.clone();
        Arc::new(move || clock.load(Ordering::SeqCst))
    };
    let bridge = GamePresenceBridge::new(BridgeOptions {
        log: Some(logger.clone()),
        now: Some(now),
        on_presence: Some({
            let emitted = emitted.clone();
            Arc::new(move |snapshot: &Value| emitted.lock().unwrap().push(snapshot.clone()))
        }),
        ..BridgeOptions::default()
    });
    Harness {
        clock,
        emitted,
        logger,
        bridge,
    }
}

#[tokio::test]
async fn bridge_like_0_4_7() {
    let fixture = fixture("presence-state.json");
    let mut mismatches = Mismatches::default();
    for scenario in items(&fixture["bridge"]) {
        let name = str_of(&scenario["name"]);
        let h = harness();
        for step in items(&scenario["steps"]) {
            let at = step["at"].as_i64().unwrap();
            h.clock.store(at, Ordering::SeqCst);
            h.emitted.lock().unwrap().clear();
            let returned = match &step["message"] {
                Value::String(text) if text == "gone" => json!(h.bridge.on_game_gone()),
                Value::String(text) if text == "shutdown" => {
                    h.bridge.shutdown();
                    Value::Null
                }
                message => json!(h.bridge.on_message(message_of(message).as_ref())),
            };
            let context = format!("{name} @{at}");
            mismatches.check(
                &returned,
                &step["returned"],
                format!("{context}: результат"),
            );
            let emitted = Value::Array(h.emitted.lock().unwrap().clone());
            mismatches.check(
                &emitted,
                &step["emitted"],
                format!("{context}: снимки обработчику"),
            );
            mismatches.check(
                &h.bridge.snapshot(),
                &step["snapshot"],
                format!("{context}: snapshot"),
            );
            let diagnostics = h.bridge.diagnostics();
            mismatches.check(
                &json!({"lifecycle": diagnostics["lifecycle"], "hasMap": diagnostics["hasMap"]}),
                &step["diagnostics"],
                format!("{context}: diagnostics"),
            );
            mismatches.check(
                &h.bridge.state().to_value(),
                &step["state"],
                format!("{context}: состояние моста"),
            );
            mismatches.check(
                &json!(h.bridge.last_fps_publish_at_ms()),
                &step["lastFpsPublishAtMs"],
                format!("{context}: lastFpsPublishAtMs"),
            );
        }
    }
    mismatches.finish("GamePresenceBridge");
}

#[tokio::test]
async fn a_failing_presence_handler_does_not_break_the_bridge() {
    let fixture = fixture("presence-state.json");
    let logger = Arc::new(MemoryLogger::default());
    let bridge = GamePresenceBridge::new(BridgeOptions {
        log: Some(logger.clone()),
        now: Some(Arc::new(|| 0)),
        on_presence: Some(Arc::new(|_| panic!("обработчик упал"))),
        ..BridgeOptions::default()
    });
    let message = message_of(&json!({
        "type": "state", "session": "0123abcd", "seq": 1, "lifecycle": "playing",
        "mapName": "d1_trainstation_01", "fps": 60, "timerRunning": false,
        "timerCompleted": false, "elapsedMs": null
    }));
    let returned = bridge.on_message(message.as_ref());
    let expected = &fixture["throwingHandler"];
    assert_eq!(json!(returned), expected["returned"]);
    assert_eq!(bridge.state().to_value(), expected["state"]);
    let warnings: Vec<String> = logger
        .take()
        .into_iter()
        .filter(|(level, _)| *level == "warn")
        .map(|(_, text)| text)
        .collect();
    let expected_warnings: Vec<String> = items(&expected["warnings"])
        .iter()
        .map(|v| str_of(v).to_owned())
        .collect();
    assert_eq!(warnings, expected_warnings);
    let _ = &harness().logger;
}
