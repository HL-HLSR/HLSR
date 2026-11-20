//! Мост чата YouTube против 0.4.7 целиком: фикстура `youtube-bridge.json` из
//! `tools/reference/online/youtube-bridge.js` — `YouTubeChatBridge` 0.4.7 с настоящим
//! `nativeImage` против двойников www.youtube.com и yt3.ggpht.com: страница, тела POST,
//! строки `chat.line` со значками ролей, эмодзи, значок участника, сбои, остановка. Паузы опроса
//! сокращены в 20 раз. Ожидание — 0.4.7 с исправленным значком участника (решение 6 спецификации
//! ПП4); результат 0.4.7 как есть лежит в `original`.
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_online::youtube::YouTubeChatBridge;
use hlsr_online::youtube::constants::USER_AGENT;
use serde_json::Value;
use std::sync::Arc;
use support::chat::{Subject, run_scenario};
use support::{diff, fixture, items, str_of};

/// Журнал без различий, которых игра не видит: User-Agent зависит от ОС (в фикстуре — той, где её
/// снимали), а reqwest сам добавляет запросам без `accept` значение `*/*`.
fn normalized(run: &Value, user_agent: &str) -> Value {
    let mut run = run.clone();
    for entry in run["journal"].as_array_mut().into_iter().flatten() {
        let headers = entry["headers"].as_object_mut().unwrap();
        if headers.get("accept").and_then(Value::as_str) == Some("*/*") {
            headers.remove("accept");
        }
        if let Some(agent) = headers.get_mut("user-agent") {
            assert_eq!(agent.as_str(), Some(user_agent), "User-Agent запроса");
            *agent = Value::String("<ua>".to_owned());
        }
    }
    run
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenarios_match_node() {
    let fixture = fixture("youtube-bridge.json");
    let divisor = fixture["delayDivisor"].as_u64().unwrap() as u32;
    let captured_agent = str_of(&fixture["userAgent"]);
    let mut failures = Vec::new();
    for scenario in items(&fixture["scenarios"]) {
        let actual = run_scenario(scenario, |ctx| {
            let bridge = Arc::new(YouTubeChatBridge::new(
                ctx.http, ctx.log, ctx.handle, divisor,
            ));
            let probe = bridge.clone();
            Subject {
                consumer: bridge,
                diagnostics: Box::new(move || probe.diagnostics().to_json()),
            }
        })
        .await;
        let actual = normalized(&actual, USER_AGENT);
        let expected = normalized(&scenario["expected"], captured_agent);
        if actual != expected {
            failures.push(format!(
                "{}\n{}",
                str_of(&scenario["name"]),
                diff(&expected, &actual)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} сценариев разошлись с 0.4.7:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// Ошибка 0.4.7 записана в фикстуру: без правки значок участника не уходил (`MEMBER_BADGE_FAILED`),
/// у остальных сценариев правка ничего не меняет.
#[test]
fn fixture_documents_the_member_badge_bug() {
    let fixture = fixture("youtube-bridge.json");
    let mut with_bug = 0;
    for scenario in items(&fixture["scenarios"]) {
        let name = str_of(&scenario["name"]);
        if scenario["bug"] == true {
            with_bug += 1;
            let original = scenario["original"].to_string();
            assert!(original.contains("MEMBER_BADGE_FAILED"), "{name}");
            assert!(!original.contains("sendBadgeReady"), "{name}");
        } else {
            assert!(scenario.get("original").is_none(), "{name}");
        }
    }
    assert_eq!(with_bug, 3);
    // с правкой значок уходит и ровно один раз на подключение (повтор — после handleReady)
    let main = items(&fixture["scenarios"])
        .iter()
        .find(|s| str_of(&s["name"]).starts_with("значок участника (ошибка"))
        .unwrap();
    let badge_sends = main["expected"]
        .to_string()
        .matches("sendBadgeReady")
        .count();
    assert_eq!(
        badge_sends, 2,
        "значок: при первом сообщении и после handleReady"
    );
}
