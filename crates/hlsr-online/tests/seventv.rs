//! Эмоуты 7TV против 0.4.7: фикстура `seventv.json` из `tools/reference/online/seventv.js` —
//! `SevenTvBridge` 0.4.7 с настоящим `nativeImage` против двойников 7tv.io и cdn.7tv.app; те же
//! сообщения игры → те же вызовы моста (имена, размеры, число кадров, интервал, пиксели), те же
//! запросы к серверам и строки журнала.
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_online::seventv::SevenTvBridge;
use std::sync::Arc;
use support::chat::{Subject, compare_run, run_scenario};
use support::{fixture, items, str_of};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenarios_match_node() {
    let fixture = fixture("seventv.json");
    let mut failures = Vec::new();
    for scenario in items(&fixture["scenarios"]) {
        let actual = run_scenario(scenario, |ctx| {
            let bridge = Arc::new(SevenTvBridge::new(ctx.http, ctx.log, ctx.handle));
            let probe = bridge.clone();
            Subject {
                consumer: bridge,
                diagnostics: Box::new(move || probe.diagnostics().to_json()),
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
