//! Эмоуты Twitch против 0.4.7: фикстура `twitch-emotes.json` из
//! `tools/reference/online/twitch-emotes.js` — `TwitchEmoteBridge` 0.4.7 с настоящим
//! `nativeImage` против двойника static-cdn.jtvnw.net. Ожидание — 0.4.7 с исправленной ошибкой
//! однокадрового GIF (решение 6 спецификации ПП4); результат 0.4.7 как есть лежит в `original`.
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_online::twitch_emotes::{TwitchEmoteBridge, is_twitch_emote_id};
use std::sync::Arc;
use support::chat::{Subject, compare_run, run_scenario};
use support::{fixture, items, str_of};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenarios_match_node() {
    let fixture = fixture("twitch-emotes.json");
    let mut failures = Vec::new();
    for scenario in items(&fixture["scenarios"]) {
        let actual = run_scenario(scenario, |ctx| {
            let bridge = Arc::new(TwitchEmoteBridge::new(ctx.http, ctx.log, ctx.handle));
            let probe = bridge.clone();
            Subject {
                consumer: bridge,
                diagnostics: Box::new(move || probe.diagnostics()),
            }
        })
        .await;
        if let Some(diff) = compare_run(scenario, &actual) {
            failures.push(format!("{}\n{diff}", str_of(&scenario["name"])));
        }
    }
    assert!(
        failures.is_empty(),
        "{} сценариев разошлись с 0.4.7:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// Ошибка 0.4.7 записана в фикстуру: без правки однокадровый GIF давал промах или
/// «[object Object]», а у остальных сценариев правка ничего не меняет.
#[test]
fn fixture_documents_the_single_frame_gif_bug() {
    let fixture = fixture("twitch-emotes.json");
    let mut with_bug = 0;
    for scenario in items(&fixture["scenarios"]) {
        if scenario["bug"] == true {
            with_bug += 1;
            assert_ne!(
                scenario["original"]["steps"],
                scenario["expected"]["steps"],
                "{}",
                str_of(&scenario["name"])
            );
        } else {
            assert!(scenario.get("original").is_none());
        }
    }
    assert_eq!(with_bug, 3);
    let literal = fixture["scenarios"].as_array().unwrap().iter().any(|s| {
        s["original"]["steps"]
            .to_string()
            .contains("[object Object]")
    });
    assert!(literal, "в original нет «[object Object]»");
}

#[test]
fn emote_ids_follow_the_protocol() {
    for good in ["1", "25", "emotesv2_abcDEF_09", &"a".repeat(44)] {
        assert!(is_twitch_emote_id(good), "{good}");
    }
    for bad in ["", "bad-id", "a b", "кириллица", "x\n", &"a".repeat(45)] {
        assert!(!is_twitch_emote_id(bad), "{bad:?}");
    }
}
