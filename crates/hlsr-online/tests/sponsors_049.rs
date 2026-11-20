//! Спонсорство против 0.4.9: фикстура `sponsors-049.json` из `tools/reference/online/sponsors-049.js`
//! — `sponsors-client.js` (`/v2`, откат на `/v1`, кеш, `lastList`), `sponsors-ipc.js`
//! (`sponsor-open-link`, `support-open`) и `style-presets-client.js` против подделки транспорта
//! (маршруты по адресу, ответы по очереди, нет маршрута — 404). Сравниваются итог каждого шага,
//! вопросы диалога, открытые адреса и журнал запросов (адрес, метод, тайм-аут, лимит, версия).

mod support;

use hlsr_delivery::providers::BoxFuture;
use hlsr_online::shared::sponsor_sections::{SECTIONS, SUPPORT_KEYS};
use hlsr_online::telemetry::news_ipc::{NewsError, NewsLinks};
use hlsr_online::telemetry::sponsors_client::{SponsorsClient, SponsorsClientOptions};
use hlsr_online::telemetry::sponsors_ipc::SponsorsIpc;
use hlsr_online::telemetry::style_presets_client::{StylePresetsClient, StylePresetsClientOptions};
use hlsr_online::telemetry::transport::{
    Headers, RequestOptions, Response, Transport, TransportError,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use support::{FakeClock, Mismatches, fixture, items, str_of};

/// Подделка `request` скрипта: ответы маршрута по очереди, последний повторяется.
struct FakeTransport {
    routes: Value,
    counters: Mutex<HashMap<String, usize>>,
    calls: Mutex<Vec<Value>>,
}

impl Transport for FakeTransport {
    fn request<'a>(
        &'a self,
        url: &'a str,
        options: &'a RequestOptions,
    ) -> BoxFuture<'a, Result<Response, TransportError>> {
        self.calls.lock().unwrap().push(json!({
            "url": url,
            "method": options.method,
            "timeoutMs": options.timeout_ms,
            "maxResponseBytes": options.max_response_bytes,
            "launcherVersion": options.launcher_version,
        }));
        let reply = self.routes[url]
            .as_array()
            .filter(|list| !list.is_empty())
            .map(|list| {
                let mut counters = self.counters.lock().unwrap();
                let at = counters.entry(url.to_owned()).or_default();
                *at += 1;
                list[(*at - 1).min(list.len() - 1)].clone()
            });
        Box::pin(async move {
            // Ответ не раньше, чем все вызовы подряд начались (`async` подделки в JS).
            tokio::task::yield_now().await;
            let Some(reply) = reply else {
                return Ok(response(404, String::new()));
            };
            if reply.get("error").is_some() {
                return Err(TransportError::Network);
            }
            if let Some(json) = reply.get("json") {
                return Ok(response(200, json.to_string()));
            }
            Ok(response(
                reply["status"].as_u64().unwrap_or(200) as u16,
                reply["body"].as_str().unwrap_or_default().to_owned(),
            ))
        })
    }
}

fn response(status: u16, body: String) -> Response {
    Response {
        status,
        headers: Headers::default(),
        body,
    }
}

#[derive(Default)]
struct Links {
    confirm: bool,
    fails: bool,
    asked: Mutex<Vec<Value>>,
    opened: Mutex<Vec<String>>,
}

impl NewsLinks for Links {
    fn confirm_link<'a>(&'a self, host: &'a str, url: &'a str) -> BoxFuture<'a, bool> {
        self.asked.lock().unwrap().push(json!([host, url]));
        Box::pin(async move { self.confirm })
    }

    fn open_external<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<(), NewsError>> {
        Box::pin(async move {
            if self.fails {
                return Err(NewsError::new("shell"));
            }
            self.opened.lock().unwrap().push(url.to_owned());
            Ok(())
        })
    }
}

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
    Sponsors(Arc<SponsorsClient>, SponsorsIpc),
    Presets(StylePresetsClient),
}

impl Client {
    async fn get(&self, force: bool) -> Value {
        match self {
            Client::Sponsors(client, _) => client.get(force).await,
            Client::Presets(client) => client.get(force).await,
        }
    }
}

fn option(options: &Value, key: &str) -> Option<String> {
    options[key].as_str().map(str::to_owned)
}

#[tokio::test(flavor = "multi_thread")]
async fn scenarios_match_node_049() {
    let fixture = fixture("sponsors-049.json");
    assert_eq!(fixture["constants"]["sections"], json!(SECTIONS));
    assert_eq!(fixture["constants"]["supportKeys"], json!(SUPPORT_KEYS));
    let start = fixture["start"].as_i64().unwrap();
    let version = str_of(&fixture["version"]).to_owned();
    let scenarios = items(&fixture["scenarios"]);
    assert!(scenarios.len() >= 60, "{}", scenarios.len());
    let mut mismatches = Mismatches::default();

    for scenario in scenarios {
        let name = str_of(&scenario["name"]);
        let clock = FakeClock::new(start);
        let transport = Arc::new(FakeTransport {
            routes: scenario["routes"].clone(),
            counters: Mutex::default(),
            calls: Mutex::default(),
        });
        let links = Arc::new(Links {
            confirm: scenario["confirm"] == json!(true),
            fails: scenario["openFails"] == json!(true),
            ..Links::default()
        });
        let options = &scenario["options"];
        let client = if str_of(&scenario["kind"]) == "presets" {
            Client::Presets(StylePresetsClient::new(StylePresetsClientOptions {
                endpoint: option(options, "endpoint"),
                launcher_version: Some(version.clone()),
                clock: clock.clock(),
                transport: transport.clone(),
            }))
        } else {
            let client = Arc::new(SponsorsClient::new(SponsorsClientOptions {
                endpoint: option(options, "endpoint"),
                legacy_endpoint: option(options, "legacyEndpoint"),
                launcher_version: Some(version.clone()),
                clock: clock.clock(),
                transport: transport.clone(),
            }));
            let ipc = SponsorsIpc::new(client.clone(), links.clone());
            Client::Sponsors(client, ipc)
        };

        let expected = items(&scenario["results"]);
        for (at, step) in items(&scenario["steps"]).iter().enumerate() {
            let result = match (str_of(&step["op"]), &client) {
                ("advance", _) => {
                    clock.advance(step["ms"].as_i64().unwrap());
                    Value::Null
                }
                ("get", client) => client.get(step["force"] == json!(true)).await,
                ("getAll", client) => {
                    let calls: Vec<BoxFuture<'_, Value>> = items(&step["forces"])
                        .iter()
                        .map(|force| {
                            Box::pin(client.get(force == &json!(true))) as BoxFuture<'_, Value>
                        })
                        .collect();
                    Value::Array(join_all(calls).await)
                }
                ("lastList", Client::Sponsors(client, _)) => client
                    .last_list()
                    .map_or(Value::Null, |list| Value::Object((*list).clone())),
                ("invoke", Client::Sponsors(_, ipc)) => {
                    let args = items(&step["args"]);
                    match str_of(&step["channel"]) {
                        "sponsor-open-link" => ipc.open_link(args.first()).await,
                        "support-open" => ipc.support_open(args.first(), args.get(1)).await,
                        other => panic!("канал {other}"),
                    }
                }
                (op, _) => panic!("шаг {op} не для этого клиента"),
            };
            let actual = json!({
                "result": result,
                "asked": std::mem::take(&mut *links.asked.lock().unwrap()),
                "opened": std::mem::take(&mut *links.opened.lock().unwrap()),
            });
            mismatches.check(&actual, &expected[at], format!("{name}: шаг {at} {step}"));
        }
        let calls = Value::Array(transport.calls.lock().unwrap().clone());
        mismatches.check(
            &calls,
            &scenario["calls"],
            format!("{name}: журнал запросов"),
        );
    }
    mismatches.finish("спонсорство 0.4.9");
}
