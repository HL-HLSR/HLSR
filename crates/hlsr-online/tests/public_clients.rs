//! Онлайн, лента и спонсоры против 0.4.7: фикстура `public-clients.json` из
//! `tools/reference/online/public-clients.js` — `online-client.js`, `news-client.js` и
//! `sponsors-client.js` с настоящим `transport.js` против двойника сервера, часы поддельные. Тот же
//! сценарий (маршруты, шаги) выполняет Rust против своего двойника (`support/server.rs`);
//! сравниваются итог каждого шага, снимки, отданные в `onUpdate`, и журнал запросов (метод, путь,
//! заголовки — `if-none-match`, `user-agent`, тело).
//!
//! Спонсоры 0.4.9 ходят на `/v2` с откатом на `/v1` (O-3): их сценарии 0.4.7 здесь пропускаются,
//! те же случаи через откат сверяет `sponsors_049.rs`.
//!
//! Отступление от 0.4.7 (спецификация, решение 6): ответ больше лимита. Транспорт Node рвёт ответ и
//! не завершает запрос, `inFlight` онлайна и `pending` ленты и спонсоров остаются навсегда (в
//! фикстуре — `{hang: true}`); Rust завершает запрос ошибкой, и клиент работает дальше. Что должен
//! вернуть Rust на таком шаге, записано в `fixed` шага.
//!
//! Поля ленты 0.4.9 (`place` и `profiles` записи, `subscriber` карточки автора и спонсора) из итога
//! убираются: их сверяет `news_model.rs` (`news-model-049.json`).
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_delivery::providers::BoxFuture;
use hlsr_online::telemetry::news_client::{NewsClient, NewsClientOptions};
use hlsr_online::telemetry::online_client::{OnlineClient, OnlineClientOptions};
use hlsr_online::telemetry::transport::{HttpTransport, Transport};
use serde_json::{Value, json};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;
use support::server::{Journal, dead_port, start_server};
use support::{FakeClock, Mismatches, fixture, items, str_of};

/// Все будущие опрашиваются по порядку: каждое доходит до первого `await`, как вызовы подряд в JS
/// (`Promise.all([f(), f(), f()])`).
fn join_all<'a, T: 'a>(futures: Vec<BoxFuture<'a, T>>) -> impl Future<Output = Vec<T>> + 'a {
    let mut slots: Vec<(Option<BoxFuture<'a, T>>, Option<T>)> =
        futures.into_iter().map(|f| (Some(f), None)).collect();
    std::future::poll_fn(move |cx| {
        let mut pending = false;
        for (future, out) in &mut slots {
            if let Some(running) = future {
                match running.as_mut().poll(cx) {
                    Poll::Ready(value) => {
                        *out = Some(value);
                        *future = None;
                    }
                    Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            Poll::Pending
        } else {
            Poll::Ready(slots.iter_mut().filter_map(|(_, out)| out.take()).collect())
        }
    })
}

enum Client {
    Online(Arc<OnlineClient>),
    News(Box<NewsClient>),
}

/// Выполнить шаг; `None` — шаг не дождался итога за `race` мс (запрос завис).
async fn run_step(
    client: &Client,
    step: &Value,
    clock: &FakeClock,
    visible: &AtomicBool,
) -> Option<Value> {
    let force = step["force"].as_bool() == Some(true);
    let many = step["n"].as_u64().unwrap_or(0) as usize;
    let work = async {
        match (str_of(&step["op"]), client) {
            ("advance", _) => {
                clock.advance(step["ms"].as_i64().unwrap());
                Value::Null
            }
            ("visible", _) => {
                visible.store(step["value"].as_bool().unwrap(), Ordering::SeqCst);
                Value::Null
            }
            ("snapshot", Client::Online(online)) => online.snapshot().to_json(),
            ("fetch", Client::Online(online)) => online.fetch_once(force).await.to_json(),
            ("fetchMany", Client::Online(online)) => {
                let calls: Vec<BoxFuture<'_, Value>> = (0..many)
                    .map(|_| {
                        Box::pin(async move { online.fetch_once(force).await.to_json() })
                            as BoxFuture<'_, Value>
                    })
                    .collect();
                Value::Array(join_all(calls).await)
            }
            ("react", Client::News(news)) => {
                news.update_reactions(str_of(&step["id"]), &step["raw"]);
                Value::Null
            }
            ("get", Client::News(news)) => news.get(force).await.to_json(),
            ("getMany", Client::News(news)) => {
                let calls: Vec<BoxFuture<'_, Value>> = (0..many)
                    .map(|_| {
                        Box::pin(async move { news.get(force).await.to_json() })
                            as BoxFuture<'_, Value>
                    })
                    .collect();
                Value::Array(join_all(calls).await)
            }
            (op, _) => panic!("шаг {op} не для этого клиента"),
        }
    };
    match step["race"].as_u64() {
        // Rust не должен зависать: ждём втрое дольше Node и считаем зависание ошибкой теста.
        Some(ms) => tokio::time::timeout(Duration::from_millis(ms * 3), work)
            .await
            .ok(),
        None => Some(work.await),
    }
}

fn with_authority(value: &Value, authority: &str) -> Value {
    serde_json::from_str(&value.to_string().replace("{authority}", authority)).unwrap()
}

/// Итог ленты или спонсоров без полей 0.4.9.
fn without_049(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for key in ["place", "profiles", "subscriber"] {
                map.remove(key);
            }
            map.values_mut().for_each(without_049);
        }
        Value::Array(items) => items.iter_mut().for_each(without_049),
        _ => {}
    }
}

fn with_label(value: &Value, authority: &str) -> Value {
    serde_json::from_str(&value.to_string().replace(authority, "{authority}")).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn scenarios_match_node() {
    let fixture = fixture("public-clients.json");
    let scenarios = items(&fixture["scenarios"]);
    assert!(scenarios.len() >= 20);
    let start = fixture["start"].as_i64().unwrap();
    let version = str_of(&fixture["version"]).to_owned();
    let dead = dead_port().await;
    let transport: Arc<dyn Transport> = Arc::new(HttpTransport::new());
    let mut mismatches = Mismatches::default();

    for scenario in scenarios {
        if str_of(&scenario["client"]) == "sponsors" {
            continue;
        }
        let name = str_of(&scenario["name"]);
        let journal: Journal = Arc::new(Mutex::new(Vec::new()));
        let server = start_server("api", journal.clone()).await;
        server.set_routes(&with_authority(&scenario["routes"], &server.authority));

        let clock = FakeClock::new(start);
        let visible = Arc::new(AtomicBool::new(true));
        let updates: Arc<Mutex<Vec<Value>>> = Arc::default();
        let path = str_of(&scenario["path"]);
        let endpoint = if scenario["dead"].as_bool() == Some(true) {
            format!("http://127.0.0.1:{dead}{path}")
        } else {
            format!("http://{}{path}", server.authority)
        };
        let client = match str_of(&scenario["client"]) {
            "online" => {
                let mut options = OnlineClientOptions::new(clock.clock(), transport.clone());
                options.endpoint = Some(endpoint);
                options.launcher_version = Some(version.clone());
                options.backend_configured = Some(scenario["backend"].as_bool() != Some(false));
                let flag = visible.clone();
                options.is_window_visible = Some(Arc::new(move || flag.load(Ordering::SeqCst)));
                let sink = updates.clone();
                options.on_update = Some(Arc::new(move |snapshot| {
                    sink.lock().unwrap().push(snapshot.to_json());
                }));
                Client::Online(Arc::new(OnlineClient::new(options)))
            }
            "news" => Client::News(Box::new(NewsClient::new(NewsClientOptions {
                endpoint: Some(endpoint),
                launcher_version: Some(version.clone()),
                clock: clock.clock(),
                transport: transport.clone(),
            }))),
            other => panic!("клиент {other}"),
        };

        let expected_results = items(&scenario["results"]);
        for (at, step) in items(&scenario["steps"]).iter().enumerate() {
            let outcome = run_step(&client, step, &clock, &visible).await;
            let context = format!("{name}: шаг {at} {step}");
            let Some(result) = outcome else {
                mismatches.check(&json!({"hang": true}), &json!(null), context);
                continue;
            };
            let mut result = with_label(&result, &server.authority);
            if str_of(&scenario["client"]) != "online" {
                without_049(&mut result);
            }
            let recorded = updates
                .lock()
                .unwrap()
                .drain(..)
                .map(|snapshot| with_label(&snapshot, &server.authority))
                .collect::<Vec<_>>();
            if let Some(fixed) = step.get("fixed") {
                // Отступление от 0.4.7: Node здесь завис (или остался с зависшим запросом).
                mismatches.check(&result, fixed, format!("{context} (исправление зависания)"));
                continue;
            }
            let mut actual = json!({"result": result});
            if str_of(&scenario["client"]) == "online" {
                actual["updates"] = json!(recorded);
            }
            mismatches.check(&actual, &expected_results[at], context);
        }

        if scenario["compareJournal"].as_bool() != Some(false) {
            let journal: Vec<Value> = journal.lock().unwrap().clone();
            mismatches.check(
                &with_label(&json!(journal), &server.authority),
                &scenario["journal"],
                format!("{name}: журнал запросов"),
            );
        }
    }
    mismatches.finish("онлайн, лента, спонсоры");
}

// ------------------------------------------------------------------ только Rust

fn post(id: u32) -> Value {
    json!({"kind": "post", "id": id.to_string(), "title": format!("Пост {id}"), "body": "Текст", "date": "2026-10-05T10:00:00Z"})
}

fn record(id: u32) -> Value {
    json!({
        "kind": "record", "id": format!("run{id:05}"), "game": "Half-Life", "runners": ["Buster12"],
        "seconds": 100.5, "date": "2026-10-05T10:00:00Z",
        "url": format!("https://www.speedrun.com/hl1/runs/run{id:05}"),
    })
}

fn news_options(clock: &FakeClock, endpoint: String) -> NewsClientOptions {
    NewsClientOptions {
        endpoint: Some(endpoint),
        launcher_version: None,
        clock: clock.clock(),
        transport: Arc::new(HttpTransport::new()),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn news_takes_at_most_40_items_and_400_records() {
    let server = start_server("api", Journal::default()).await;
    let posts: Vec<Value> = (1..=45).map(post).collect();
    let records: Vec<Value> = (0..405).map(record).collect();
    server
        .set_routes(&json!({"/v1/public/news": [{"json": {"items": posts, "records": records}}]}));
    let clock = FakeClock::new(1_000);
    let news = NewsClient::new(news_options(
        &clock,
        format!("http://{}/v1/public/news", server.authority),
    ));

    let answer = news.get(false).await;
    assert!(answer.ok);
    assert_eq!(answer.items.len(), 440);
    assert_eq!(answer.items[39].id(), "40");
    assert_eq!(answer.items[40].id(), "run00000");
    assert_eq!(answer.items[439].id(), "run00399");
}

#[tokio::test(flavor = "multi_thread")]
async fn online_start_asks_once_and_stop_ends_the_timer() {
    let journal: Journal = Arc::default();
    let server = start_server("api", journal.clone()).await;
    server.set_routes(&json!({"/v1/public/online": [{"json": {"schemaVersion": 1, "launcherOnline": 4, "games": {}}}]}));
    let clock = FakeClock::new(1_000);
    let transport: Arc<dyn Transport> = Arc::new(HttpTransport::new());
    let mut options = OnlineClientOptions::new(clock.clock(), transport);
    options.endpoint = Some(format!("http://{}/v1/public/online", server.authority));
    let online = Arc::new(OnlineClient::new(options));

    online.start();
    online.start();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while journal.lock().unwrap().is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Ответ разобран: снимок уже не «unknown».
    while online.snapshot().launcher_online.is_none() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    online.stop();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        journal.lock().unwrap().len(),
        1,
        "первый запрос сразу, второй — через минуту"
    );
    assert_eq!(online.snapshot().launcher_online, Some(4.0));
    assert_eq!(online.consecutive_failures(), 0);
    assert_eq!(online.last_error(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_requests_do_not_block_the_clients() {
    let server = start_server("api", Journal::default()).await;
    server.set_routes(&json!({
        "/v1/public/online": [{"hold": "gate", "json": {"schemaVersion": 1, "launcherOnline": 1, "games": {}}}],
        "/v1/public/news": [{"hold": "gate", "json": {"items": [post(1)]}}],
    }));
    let clock = FakeClock::new(1_000);
    let transport: Arc<dyn Transport> = Arc::new(HttpTransport::new());

    let mut online_options = OnlineClientOptions::new(clock.clock(), transport.clone());
    online_options.endpoint = Some(format!("http://{}/v1/public/online", server.authority));
    let online = OnlineClient::new(online_options);
    let news = NewsClient::new(NewsClientOptions {
        endpoint: Some(format!("http://{}/v1/public/news", server.authority)),
        launcher_version: None,
        clock: clock.clock(),
        transport,
    });

    // Запрос отменён на полпути (закрыли окно, вышли из вкладки): клиент не залипает.
    assert!(
        tokio::time::timeout(Duration::from_millis(150), online.fetch_once(false))
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(150), news.get(true))
            .await
            .is_err()
    );
    server.release("gate");
    assert_eq!(
        online.fetch_once(false).await.to_json(),
        json!({"ok": true, "value": {"launcherOnline": 1, "games": {}}})
    );
    let answer = news.get(true).await;
    assert!(answer.ok);
    assert_eq!(answer.items.len(), 1);
}
