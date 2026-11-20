//! Служба профиля и каналы против 0.4.7: фикстура `online-profile-service.json` из
//! `tools/reference/online/online-profile-service.js` (настоящий `OnlineProfileService.js` и
//! `online-profile/ipc.js` на двойнике API и speedrun.com, `SecureSession` без шифрования) — те же
//! сценарии против своего двойника: вход по коду устройства, отказы (401, 403, 428, 429, 503/426,
//! устаревший ответ), синхронизация и достижения, модерация, ссылки забегов, сессии игры, игроки,
//! поиск, профили, оценки, друзья. Сверяются итог и события каждого шага (снимки — хешем текста
//! JSON), файлы папки данных (`online-session.v2.json` — как `online-session.json` 0.4.7) и журнал
//! запросов. Сценарии 0.4.9 (`online-profile-service-049.json`, тот же скрипт на копии 0.4.9):
//! «Спасибо за поддержку», 409, привязки Discord и Telegram, чужой профиль без `details.secrets`,
//! выход на всех устройствах — снимки и файлы как есть, без приведения к 0.4.7.
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_online::online_profile::ipc::ProfileIpc;
use hlsr_online::online_profile::online_profile_service::{
    OnlineProfileOptions, OnlineProfileService, RunnerError,
};
use hlsr_online::online_profile::profile_state::LocalAccount;
use hlsr_online::online_profile::session_store::{PlainVault, SESSION_FILE, Session, SessionStore};
use hlsr_online::profile::launch_streak::DayZone;
use hlsr_online::security::attestation::ClientModified;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use support::online_profile::{
    Events, ManualTimers, ScenarioTransport, digest_as, node_files_as, normalize_journal, push,
    replace_base, take,
};
use support::server::{Journal, Server, start_server};
use support::{FakeClock, Mismatches, fixture, items, str_of};
use tokio::task::JoinHandle;

/// Сколько сценарий ждёт запросов службы; дольше — тест падает, а не висит.
const SCENARIO_WAIT: Duration = Duration::from_secs(30);

fn account(value: &Value) -> LocalAccount {
    let text = |key: &str| value[key].as_str().map(str::to_owned);
    LocalAccount {
        steam_id64: text("steamId64"),
        persona_name: text("personaName"),
        avatar_data_uri: text("avatarDataUri"),
    }
}

fn rejected(result: Result<Value, RunnerError>) -> Value {
    result.unwrap_or_else(|error| json!({ "#rejected": error.message }))
}

/// Пачка `/v2/runner/friends`: в HLSR — каждый второй (`HANDLERS["friends-even"]`).
fn friends_even(entry: &Value, _index: usize) -> Value {
    let body: Value = serde_json::from_str(str_of(&entry["body"])).unwrap();
    let profiles: Vec<Value> = body["friends"]
        .as_array()
        .unwrap()
        .iter()
        .step_by(2)
        .map(|id| json!({ "steamId64": id }))
        .collect();
    json!({ "json": { "profiles": profiles } })
}

fn set_routes(server: &Server, routes: &Value, base: &str) {
    let mut own = serde_json::Map::new();
    for (key, value) in routes.as_object().into_iter().flatten() {
        match value["handler"].as_str() {
            Some("friends-even") => server.handle(key, friends_even),
            Some(other) => panic!("неизвестный обработчик {other}"),
            None => {
                let text = serde_json::to_string(value)
                    .unwrap()
                    .replace("{base}", base);
                own.insert(key.clone(), serde_json::from_str(&text).unwrap());
            }
        }
    }
    server.set_routes(&Value::Object(own));
}

struct Run {
    root: PathBuf,
    base: String,
    speedrun_base: String,
    api: Server,
    journal: Journal,
    clock: FakeClock,
    events: Events,
    timers: ManualTimers,
    transport: Arc<ScenarioTransport>,
    in_flight: Arc<AtomicUsize>,
    account: Arc<Mutex<LocalAccount>>,
    local_friends: Arc<Mutex<Vec<Value>>>,
    build: Arc<Mutex<Option<String>>>,
    ticket: Arc<Mutex<String>>,
    open_fails: Arc<AtomicBool>,
    random: Arc<AtomicU8>,
    device: String,
    version: String,
    services: Vec<OnlineProfileService>,
    ipc: Option<ProfileIpc>,
    background: Vec<JoinHandle<Value>>,
    /// Фикстура 0.4.7: снимки и файлы приводятся к её виду.
    old: bool,
}

impl Run {
    fn make(&self) -> ProfileIpc {
        let events = self.events.clone();
        let old = self.old;
        let on = |name: &'static str| {
            let events = events.clone();
            Some(
                Arc::new(move |value: Value| push(&events, json!([name, digest_as(&value, old)])))
                    as Arc<dyn Fn(Value) + Send + Sync>,
            )
        };
        let consent_events = events.clone();
        let modified_events = events.clone();
        let open_events = events.clone();
        let open_fails = self.open_fails.clone();
        let ticket_events = events.clone();
        let ticket = self.ticket.clone();
        let build = self.build.clone();
        let account = self.account.clone();
        let local_friends = self.local_friends.clone();
        let random = self.random.clone();
        let session = SessionStore::at(self.root.join(SESSION_FILE), Box::new(PlainVault));
        let service = OnlineProfileService::new(OnlineProfileOptions {
            api_base: Some(self.base.clone()),
            launcher_version: Some(self.version.clone()),
            account_provider: Some(Arc::new(move || account.lock().unwrap().clone())),
            friend_provider: Some(Arc::new(move || local_friends.lock().unwrap().clone())),
            open_external: Some(Arc::new(move |url: String| {
                push(&open_events, json!(["open", url]));
                let fails = open_fails.load(Ordering::SeqCst);
                Box::pin(async move {
                    if fails {
                        Err("браузер не открылся".to_owned())
                    } else {
                        Ok(())
                    }
                })
            })),
            on_change: on("change"),
            on_authenticated: on("authenticated"),
            on_consent: Some(Arc::new(move |enabled| {
                push(&consent_events, json!(["consent", enabled]))
            })),
            on_unlock: Some(Arc::new({
                let events = events.clone();
                move |item| push(&events, json!(["unlock", item]))
            })),
            on_moderation: Some(Arc::new({
                let events = events.clone();
                move |notice| push(&events, json!(["moderation", notice]))
            })),
            on_support_thanks: Some(Arc::new({
                let events = events.clone();
                move |notice| push(&events, json!(["supportThanks", notice]))
            })),
            on_client_modified: Some(Arc::new(move || {
                push(&modified_events, json!(["clientModified"]))
            })),
            build_header: Some(Arc::new(move || {
                let build = build.lock().unwrap().clone();
                Box::pin(async move { build })
            })),
            ticket_provider: Some(Arc::new(move |force| {
                let mode = ticket.lock().unwrap().clone();
                if mode != "none" {
                    push(&ticket_events, json!(["ticket", force]));
                }
                Box::pin(async move {
                    match mode.as_str() {
                        "none" | "broken" => Ok(None),
                        "modified" => Err(ClientModified),
                        "old-new" => Ok(Some(if force { "NEW" } else { "OLD" }.to_owned())),
                        _ => Ok(Some("T".to_owned())),
                    }
                })
            })),
            clock: self.clock.clock(),
            timers: Arc::new(self.timers.clone()),
            random_bytes: Some(Arc::new(move |size| {
                let n = random.fetch_add(1, Ordering::SeqCst) + 1;
                vec![n; size]
            })),
            device: Some(self.device.clone()),
            // Дни серии — по UTC, как у скрипта съёма (`TZ=UTC`): итог не зависит от пояса машины.
            day_zone: DayZone::Fixed(0),
            ..OnlineProfileOptions::new(&self.root, session, self.transport.clone())
        });
        ProfileIpc::new(service)
    }

    fn service(&self) -> &OnlineProfileService {
        self.ipc.as_ref().expect("служба создана").service()
    }

    fn ipc(&self) -> &ProfileIpc {
        self.ipc.as_ref().expect("служба создана")
    }

    /// Все запросы закончились (как `settle()` скрипта: счётчик в пути — ноль три раза подряд).
    async fn settle(&self) {
        let deadline = Instant::now() + SCENARIO_WAIT;
        let mut quiet = 0;
        while quiet < 3 {
            assert!(
                Instant::now() < deadline,
                "запросы не закончились за {SCENARIO_WAIT:?}"
            );
            for service in &self.services {
                service.settle().await;
            }
            tokio::task::yield_now().await;
            if self.in_flight.load(Ordering::SeqCst) == 0 {
                quiet += 1;
            } else {
                quiet = 0;
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
    }

    async fn step(&mut self, step: &Value) -> Value {
        let id = || str_of(&step["gameId"]).to_owned();
        match str_of(&step["op"]) {
            "construct" => {
                let ipc = self.make();
                self.services.push(ipc.service().clone());
                self.ipc = Some(ipc);
            }
            "ipc" => {
                let channel = str_of(&step["channel"]).to_owned();
                let args = step["args"].as_array().cloned().unwrap_or_default();
                let ipc = self.ipc().clone();
                if step["background"] == json!(true) {
                    self.background.push(tokio::spawn(async move {
                        ipc.handle(&channel, &args).await.unwrap()
                    }));
                    return Value::Null;
                }
                return ipc.handle(&channel, &args).await.expect("канал есть");
            }
            "profile-get" => return self.ipc().profile_get(),
            "friends-list" => {
                let options = step.get("options");
                return rejected(self.ipc().friends_list(options).await);
            }
            "game-stats-get" => {
                return self
                    .ipc()
                    .game_stats_from_server(&id())
                    .unwrap_or(Value::Null);
            }
            "gameStart" => self.service().game_start(&id()),
            "gameEnd" => self.service().game_end(&id()),
            "beat" => {
                let beat = self.service().beat(&id(), step["end"] == json!(true));
                let run = async move {
                    match beat.await {
                        Ok(()) => Value::Null,
                        Err(error) => json!({ "#rejected": error.message }),
                    }
                };
                if step["background"] == json!(true) {
                    self.background.push(tokio::spawn(run));
                    return Value::Null;
                }
                return run.await;
            }
            "achieve" => self.service().achieve(str_of(&step["id"])),
            "unlock" => return rejected(self.service().unlock(str_of(&step["id"])).await),
            "noteActiveDay" => self.service().note_active_day(),
            "start" => self.service().start(),
            "stop" => self.service().stop(),
            "tick" => {
                if !self.timers.fire_interval() {
                    return json!("no-timer");
                }
            }
            "fire" => {
                if !self.timers.fire_timeout() {
                    return json!("no-timer");
                }
            }
            "speedrunRunLink" => {
                let link = self.service().speedrun_run_link(str_of(&step["id"])).await;
                return rejected(link.map(Value::String));
            }
            "advance" => self.clock.advance(step["ms"].as_i64().unwrap()),
            "account" => *self.account.lock().unwrap() = account(&step["value"]),
            "build" => *self.build.lock().unwrap() = step["value"].as_str().map(str::to_owned),
            "ticket" => *self.ticket.lock().unwrap() = str_of(&step["value"]).to_owned(),
            "openFails" => self
                .open_fails
                .store(step["value"] == json!(true), Ordering::SeqCst),
            "routes" => set_routes(&self.api, &step["routes"], &self.base),
            "release" => self.api.release(str_of(&step["gate"])),
            "waitJournal" => {
                let count = step["count"].as_u64().unwrap() as usize;
                let deadline = Instant::now() + SCENARIO_WAIT;
                while self.journal.lock().unwrap().len() < count {
                    assert!(
                        Instant::now() < deadline,
                        "в журнале не появилось {count} запросов за {SCENARIO_WAIT:?}"
                    );
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            }
            "join" => {
                let mut out = Vec::new();
                for handle in std::mem::take(&mut self.background) {
                    out.push(handle.await.unwrap());
                }
                return Value::Array(out);
            }
            "token" => self
                .service()
                .set_token_for_tests(step["value"].as_str().map(str::to_owned)),
            "authToken" => return json!(self.service().get_auth_token()),
            "snapshot" => return self.service().snapshot(),
            "files" => return node_files_as(&self.root, self.old),
            other => panic!("неизвестный шаг {other}"),
        }
        Value::Null
    }
}

fn write_session(root: &Path, session: &Value) {
    let store = SessionStore::at(root.join(SESSION_FILE), Box::new(PlainVault));
    store
        .write(&Session {
            token: str_of(&session["token"]).to_owned(),
            profile: session["profile"].clone(),
        })
        .unwrap();
}

#[tokio::test]
async fn scenarios_match_node() {
    replay("online-profile-service.json", true).await;
}

#[tokio::test]
async fn scenarios_match_node_049() {
    replay("online-profile-service-049.json", false).await;
}

/// 0.4.7 отдавал окну `details.secrets` чужого профиля, 0.4.9 его убирает (`view()`).
fn without_view_secrets(step: &Value, expected: &Value) -> Value {
    let mut expected = expected.clone();
    if step["channel"] == "runner-profile-view"
        && let Some(details) = expected
            .get_mut("result")
            .and_then(|result| result.get_mut("details"))
            .and_then(Value::as_object_mut)
    {
        details.shift_remove("secrets");
    }
    expected
}

async fn replay(name: &str, old: bool) {
    let fixture = fixture(name);
    let now = fixture["now"].as_i64().unwrap();
    let mut mismatches = Mismatches::default();
    for scenario in items(&fixture["scenarios"]) {
        let name = str_of(&scenario["name"]);
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        support::setup(&root, &scenario["setup"]);
        if scenario["session"].is_object() {
            write_session(&root, &scenario["session"]);
        }
        let journal: Journal = Arc::default();
        let api = start_server("api", journal.clone()).await;
        let speedrun = start_server("speedrun", journal.clone()).await;
        let base = format!("http://{}", api.authority);
        let speedrun_base = format!("http://{}/", speedrun.authority);
        set_routes(&api, &scenario["routes"], &base);
        set_routes(&speedrun, &scenario["speedrun"], &base);
        let transport = Arc::new(ScenarioTransport::new(speedrun_base.clone()));
        let events: Events = Arc::default();
        let local_friends = scenario["localFriends"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut run = Run {
            root,
            base,
            speedrun_base,
            api,
            journal: journal.clone(),
            clock: FakeClock::new(now),
            timers: ManualTimers::new(events.clone()),
            events,
            in_flight: transport.in_flight.clone(),
            transport,
            account: Arc::new(Mutex::new(account(&scenario["account"]))),
            local_friends: Arc::new(Mutex::new(local_friends)),
            build: Arc::new(Mutex::new(scenario["build"].as_str().map(str::to_owned))),
            ticket: Arc::new(Mutex::new(str_of(&scenario["ticket"]).to_owned())),
            open_fails: Arc::default(),
            random: Arc::default(),
            device: str_of(&fixture["device"]).to_owned(),
            version: str_of(&fixture["version"]).to_owned(),
            services: Vec::new(),
            ipc: None,
            background: Vec::new(),
            old,
        };
        for (step, expected) in items(&scenario["steps"])
            .iter()
            .zip(items(&scenario["results"]))
        {
            let result = run.step(step).await;
            let op = str_of(&step["op"]);
            if step["background"] == json!(true) {
                // Как микрозадачи Node после `await step(s)`: фоновая задача доходит до запроса.
                for _ in 0..3 {
                    tokio::task::yield_now().await;
                }
            } else if op != "waitJournal" && step["settle"] != json!(false) {
                run.settle().await;
            }
            let actual = replace_base(
                &json!({ "result": digest_as(&result, old), "events": take(&run.events) }),
                &run.base,
                &run.speedrun_base,
            );
            let expected = if old {
                without_view_secrets(step, expected)
            } else {
                expected.clone()
            };
            mismatches.check(&actual, &expected, format!("{name}: {step}"));
        }
        if let Some(ipc) = &run.ipc {
            ipc.service().stop();
        }
        run.settle().await;
        drop(speedrun);
        let journal = Value::Array(normalize_journal(&journal.lock().unwrap()));
        let journal = replace_base(&journal, &run.base, &run.speedrun_base);
        mismatches.check(
            &journal,
            &scenario["journal"],
            format!("{name}: журнал запросов"),
        );
    }
    mismatches.finish("служба профиля");
}
