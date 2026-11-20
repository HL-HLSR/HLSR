//! Мост чата YouTube против 0.4.7, чистые части: фикстура `youtube-chat.json` из
//! `tools/reference/online/youtube-chat.js` (`youtube/{constants,YouTubeChat,index}.js` в Node 24):
//! `parseVideoId`, `badgeNamesForRoles`, `readContinuation`, `readMessage` на корпусах и класс
//! `YouTubeChat` с поддельными `fetchText`/`postJson` и временем tokio (время остановлено —
//! таймеры срабатывают, когда тест двигает его, как `fire()` у поддельных таймеров Node).

mod support;

use hlsr_core::js::to_string;
use hlsr_delivery::ports::MemoryLogger;
use hlsr_delivery::providers::BoxFuture;
use hlsr_online::youtube::constants::{USER_AGENT, parse_video_id};
use hlsr_online::youtube::{
    LineSink, Message, Roles, YouTubeChat, YouTubeFetch, badge_names_for_roles, read_continuation,
    read_message,
};
use serde_json::{Map, Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::{diff, fixture, items, str_of};
use tokio::sync::watch;

#[test]
fn user_agent_is_chrome_131_of_this_platform() {
    let fixture = fixture("youtube-chat.json");
    let key = if cfg!(target_os = "linux") {
        "linux"
    } else {
        "win32"
    };
    assert_eq!(USER_AGENT, str_of(&fixture["userAgents"][key]));
}

#[test]
fn parse_video_id_matches_node() {
    let fixture = fixture("youtube-chat.json");
    let mut failures = Vec::new();
    for case in items(&fixture["parseVideoId"]) {
        // `String(input || "")`: null и пустое — пустая строка, число — его запись.
        let raw = match &case["input"] {
            Value::Null => String::new(),
            other => to_string(other),
        };
        let got = parse_video_id(&raw);
        let want = case["result"].as_str();
        if got.as_deref() != want {
            failures.push(format!("{}: {got:?} вместо {want:?}", case["input"]));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn roles_of(value: &Value) -> Option<Roles> {
    let object = value.as_object()?;
    let flag = |key: &str| object.get(key).and_then(Value::as_bool).unwrap_or(false);
    Some(Roles {
        owner: flag("owner"),
        moderator: flag("moderator"),
        verified: flag("verified"),
        member: flag("member"),
        member_url: None,
    })
}

#[test]
fn badge_names_for_roles_match_node() {
    let fixture = fixture("youtube-chat.json");
    for case in items(&fixture["badgeNames"]) {
        let roles = roles_of(&case["roles"]);
        let got = badge_names_for_roles(roles.as_ref());
        assert_eq!(json!(got), case["result"], "роли {}", case["roles"]);
    }
}

#[test]
fn read_continuation_matches_node() {
    let fixture = fixture("youtube-chat.json");
    for case in items(&fixture["continuations"]) {
        let node = (case["node"] != "undefined").then_some(&case["node"]);
        let got = read_continuation(node)
            .map(|c| c.to_json())
            .unwrap_or(Value::Null);
        assert_eq!(got, case["result"], "узел {}", case["node"]);
    }
}

/// Одним проходом и по порядку: строка «незнакомый значок» пишется один раз на процесс.
#[test]
fn read_message_matches_node() {
    let fixture = fixture("youtube-chat.json");
    let mut failures = Vec::new();
    for case in items(&fixture["readMessage"]) {
        let log = MemoryLogger::default();
        let got = read_message(&case["action"], &log)
            .map(|m| m.to_json())
            .unwrap_or(Value::Null);
        let lines: Vec<Value> = log
            .take()
            .into_iter()
            .map(|(level, text)| json!([level, text]))
            .collect();
        if got != case["result"] || Value::Array(lines.clone()) != case["log"] {
            failures.push(format!(
                "{}\nожидалось {} / {}\nполучено {got} / {}",
                case["action"],
                case["result"],
                case["log"],
                Value::Array(lines)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} расхождений:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// Поддельные `fetchText` и `postJson`: страница, очередь ответов опроса, вызовы.
struct FakeFetch {
    page: Option<String>,
    polls: Vec<Option<Value>>,
    hold_page: bool,
    hold_poll: bool,
    pages: Mutex<Vec<String>>,
    posts: Mutex<Vec<Value>>,
    next_poll: AtomicUsize,
    page_gate: watch::Sender<bool>,
    poll_gate: watch::Sender<bool>,
}

impl YouTubeFetch for FakeFetch {
    fn fetch_text<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            self.pages.lock().unwrap().push(url.to_owned());
            if self.hold_page {
                let _ = self.page_gate.subscribe().wait_for(|open| *open).await;
            }
            self.page.clone()
        })
    }

    fn post_json<'a>(&'a self, url: &'a str, body: &'a Value) -> BoxFuture<'a, Option<Value>> {
        Box::pin(async move {
            self.posts.lock().unwrap().push(json!({
                "url": url,
                "body": hlsr_core::json_text::stringify(body, ""),
            }));
            let at = self
                .next_poll
                .fetch_add(1, Ordering::SeqCst)
                .min(self.polls.len().saturating_sub(1));
            let answer = self.polls.get(at).cloned().flatten();
            if self.hold_poll {
                let _ = self.poll_gate.subscribe().wait_for(|open| *open).await;
            }
            answer
        })
    }
}

/// Дать задачам дойти до ожидания. Короткий сон при остановленном времени заставляет рантайм
/// разобрать все готовые задачи и таймеры (одних `yield_now` для этого мало).
async fn settle() {
    for _ in 0..5 {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

#[tokio::test(start_paused = true)]
async fn chat_scenarios_match_node() {
    let fixture = fixture("youtube-chat.json");
    let mut failures = Vec::new();
    for scenario in items(&fixture["chat"]) {
        let actual = run_chat(scenario).await;
        if actual != scenario["expected"] {
            failures.push(format!(
                "{}\n{}",
                str_of(&scenario["name"]),
                diff(&scenario["expected"], &actual)
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

async fn run_chat(scenario: &Value) -> Value {
    let (page_gate, _) = watch::channel(false);
    let (poll_gate, _) = watch::channel(false);
    let fetch = Arc::new(FakeFetch {
        page: scenario["page"].as_str().map(str::to_owned),
        polls: items(&scenario["polls"])
            .iter()
            .map(|poll| (!poll.is_null()).then(|| poll.clone()))
            .collect(),
        hold_page: scenario["holdPage"] == true,
        hold_poll: scenario["holdPoll"] == true,
        pages: Mutex::default(),
        posts: Mutex::default(),
        next_poll: AtomicUsize::new(0),
        page_gate,
        poll_gate,
    });
    let log = Arc::new(MemoryLogger::default());
    let chat = YouTubeChat::new(
        fetch.clone(),
        log.clone(),
        tokio::runtime::Handle::current(),
        1,
    );

    let lines: Arc<Mutex<Vec<Message>>> = Arc::default();
    let sink: LineSink = {
        let lines = lines.clone();
        Arc::new(move |message: &Message| lines.lock().unwrap().push(message.clone()))
    };
    let texts_only = scenario["linesAsTexts"] == true;

    let mut steps: Vec<Map<String, Value>> = Vec::new();
    let mut pending: Vec<(usize, tokio::task::JoinHandle<bool>)> = Vec::new();
    let mut log_lines: Vec<Value> = Vec::new();
    for step in items(&scenario["steps"]) {
        let (lines_from, pages_from, posts_from) = (
            lines.lock().unwrap().len(),
            fetch.pages.lock().unwrap().len(),
            fetch.posts.lock().unwrap().len(),
        );
        let mut result = Map::new();
        let input = step["input"].as_str().map(str::to_owned);
        match str_of(&step["op"]) {
            "start" => {
                let started = chat.start(input.as_deref(), sink.clone()).await;
                result.insert("value".to_owned(), json!(started));
            }
            "startHeld" => {
                let (chat, sink) = (chat.clone(), sink.clone());
                let handle = tokio::spawn(async move { chat.start(input.as_deref(), sink).await });
                pending.push((steps.len(), handle));
                settle().await;
            }
            "release" => {
                fetch.page_gate.send_replace(true);
                settle().await;
                let mut values = Vec::new();
                for (at, handle) in pending.drain(..) {
                    let value = handle.await.unwrap();
                    if at < steps.len() {
                        steps[at].insert("value".to_owned(), json!(value));
                    }
                    values.push(json!(value));
                }
                result.insert("values".to_owned(), Value::Array(values));
            }
            "fire" | "fireHeld" => {
                let wait = chat.pending_wait_ms();
                result.insert("fired".to_owned(), wait.map_or(Value::Null, |ms| json!(ms)));
                if let Some(ms) = wait {
                    // задача таймера должна успеть дойти до своего сна, иначе срок отсчитается
                    // от момента после сдвига времени
                    settle().await;
                    // таймеры tokio округляют срок вверх до миллисекунды: запас в пару мс
                    tokio::time::advance(Duration::from_millis(ms + 2)).await;
                    settle().await;
                }
            }
            "releasePoll" => {
                fetch.poll_gate.send_replace(true);
                settle().await;
            }
            "stop" => chat.stop(),
            "diag" => {
                result.insert("diag".to_owned(), chat.diagnostics().to_json());
            }
            other => panic!("неизвестный шаг {other}"),
        }
        for (level, text) in log.take() {
            log_lines.push(json!([level, text]));
        }
        let new_lines: Vec<Value> = lines.lock().unwrap()[lines_from..]
            .iter()
            .map(|m| {
                if texts_only {
                    json!(m.text)
                } else {
                    json!([
                        m.nick,
                        m.text,
                        m.emoji
                            .iter()
                            .map(|e| json!({"shortcut": e.shortcut, "url": e.url}))
                            .collect::<Vec<_>>(),
                        m.roles.to_json()
                    ])
                }
            })
            .collect();
        result.insert("lines".to_owned(), Value::Array(new_lines));
        steps.push(result);
        // Строки журнала шага — те, что появились в нём; здесь копим по шагам.
        let step_log: Vec<Value> = std::mem::take(&mut log_lines);
        let last = steps.last_mut().unwrap();
        last.insert("log".to_owned(), Value::Array(step_log));
        last.insert("timers".to_owned(), json!(chat.take_scheduled_waits()));
        last.insert(
            "pages".to_owned(),
            json!(fetch.pages.lock().unwrap()[pages_from..].to_vec()),
        );
        last.insert(
            "posts".to_owned(),
            Value::Array(fetch.posts.lock().unwrap()[posts_from..].to_vec()),
        );
        last.insert("running".to_owned(), json!(chat.is_running()));
    }
    json!({ "steps": steps })
}
