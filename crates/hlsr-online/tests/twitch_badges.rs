//! Значки Twitch и YouTube против 0.4.7: фикстура `twitch-badges.json` из
//! `tools/reference/online/twitch-badges.js` — `TwitchBadgeBridge` 0.4.7 с настоящим `nativeImage`
//! и значками из `icons/badges`. PNG, вшитые в программу, — те же файлы (SHA-256 в фикстуре).
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_online::twitch_badges::{BADGE_NAMES, TwitchBadgeBridge, YT_BADGE_NAMES, badge_png};
use std::sync::Arc;
use support::chat::{Subject, compare_run, run_scenario, sha256_hex};
use support::{fixture, items, str_of};

#[test]
fn embedded_badges_are_the_files_of_0_4_7() {
    let fixture = fixture("twitch-badges.json");
    let files = fixture["files"].as_object().unwrap();
    assert_eq!(files.len(), BADGE_NAMES.len() + YT_BADGE_NAMES.len());
    for name in BADGE_NAMES.iter().chain(YT_BADGE_NAMES.iter()) {
        let png = badge_png(name).unwrap_or_else(|| panic!("нет значка {name}"));
        assert_eq!(
            sha256_hex(png),
            str_of(&files[&format!("{name}.png")]),
            "{name}"
        );
    }
    assert!(badge_png("nobody").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenarios_match_node() {
    let fixture = fixture("twitch-badges.json");
    let mut failures = Vec::new();
    for scenario in items(&fixture["scenarios"]) {
        let actual = run_scenario(scenario, |ctx| Subject {
            consumer: Arc::new(TwitchBadgeBridge::new(ctx.log)),
            diagnostics: Box::new(|| serde_json::Value::Null),
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
