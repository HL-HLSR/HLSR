//! Самотесты 0.4.7 службы профиля — те же случаи: `scripts/online-profile-selftest.js` (кроме
//! модели профиля, достижений, серии дней и QA-подмены — они в `profile_model.rs` и `local_qa.rs`,
//! и хранилища сессии — `secure_session.rs`) и `scripts/attestation-selftest.js:238-284` (повтор
//! с новым билетом, выход при `CLIENT_MODIFIED`). Транспорт — замыкание, как `request` самотестов.
//! В конце — самотесты 0.4.9 достижений v2, времени получения и серий.
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_online::online_profile::online_profile_service::{
    OnlineProfileOptions, OnlineProfileService, Timers, TokioTimers,
};
use hlsr_online::online_profile::profile_state::LocalAccount;
use hlsr_online::online_profile::session_store::{PlainVault, SESSION_FILE, Session, SessionStore};
use hlsr_online::profile::launch_streak::{self, DayZone};
use hlsr_online::security::attestation::ClientModified;
use hlsr_online::telemetry::transport::Response;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use support::online_profile::{Events, FnTransport, ManualTimers, Sent, reply, reply_text};

const ID: &str = "76561198000000111";
const OTHER: &str = "76561198000000222";

fn token() -> String {
    format!("hlsr_{}_{}", "a".repeat(16), "b".repeat(43))
}

fn card(extra: Value) -> Value {
    let mut card = json!({
        "steamId64": ID, "personaName": "Fake runner", "registeredAt": "2026-10-05T10:00:00Z",
        "games": {}, "details": {}, "badges": ["verified"], "generationId": "a".repeat(32),
    });
    for (key, value) in extra.as_object().into_iter().flatten() {
        card[key] = value.clone();
    }
    card
}

fn local_account() -> LocalAccount {
    LocalAccount {
        steam_id64: Some(ID.to_owned()),
        persona_name: Some("Fake runner".to_owned()),
        avatar_data_uri: None,
    }
}

fn store(root: &Path) -> SessionStore {
    SessionStore::at(root.join(SESSION_FILE), Box::new(PlainVault))
}

/// Вошедший пользователь (`client.token = token; client.profile = card()`): сессия в хранилище
/// до создания службы.
fn sign_in(root: &Path, profile: Value) {
    store(root)
        .write(&Session {
            token: token(),
            profile,
        })
        .unwrap();
}

struct Client {
    service: OnlineProfileService,
    timers: ManualTimers,
}

fn client(
    root: &Path,
    transport: &FnTransport,
    tweak: impl FnOnce(&mut OnlineProfileOptions),
) -> Client {
    let events: Events = Arc::default();
    let timers = ManualTimers::new(events);
    let mut options = OnlineProfileOptions {
        account_provider: Some(Arc::new(local_account)),
        timers: Arc::new(timers.clone()),
        ..OnlineProfileOptions::new(root, store(root), Arc::new(transport.clone()))
    };
    tweak(&mut options);
    Client {
        service: OnlineProfileService::new(options),
        timers,
    }
}

fn by_suffix(handler: impl Fn(&str, &Sent) -> Response + Send + Sync + 'static) -> FnTransport {
    FnTransport::sync(move |sent| handler(&sent.url, sent))
}

fn profile_reply() -> FnTransport {
    FnTransport::sync(|_| reply(json!({ "profile": card(json!({})) }), 200))
}

fn secrets_file(root: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(root.join("secrets.json")).unwrap()).unwrap()
}

#[test]
fn local_steam_cache_cannot_authenticate() {
    let dir = tempfile::tempdir().unwrap();
    let Client { service, .. } = client(dir.path(), &profile_reply(), |_| {});
    assert_eq!(service.snapshot()["account"]["signedIn"], false);
    assert_eq!(service.get_auth_token(), None);
}

#[tokio::test]
async fn device_login_opens_only_backend_and_hides_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let opened: Arc<Mutex<Vec<String>>> = Arc::default();
    let authenticated = Arc::new(AtomicUsize::new(0));
    let transport = by_suffix(|url, _| {
        if url.ends_with("/start") {
            reply(
                json!({"deviceCode": "c".repeat(64), "userCode": "ABCD-EFGH",
                    "verifyUrl": "https://api.hlsr.pro/link?code=ABCD-EFGH", "expiresIn": 900}),
                200,
            )
        } else if url.ends_with("/poll") {
            reply(
                json!({"state": "ready", "token": token(), "steamId64": ID, "profile": card(json!({}))}),
                200,
            )
        } else {
            reply(json!({ "profile": card(json!({})) }), 200)
        }
    });
    let (opened_in, authenticated_in) = (opened.clone(), authenticated.clone());
    let Client { service, timers } = client(dir.path(), &transport, |options| {
        options.open_external = Some(Arc::new(move |url| {
            opened_in.lock().unwrap().push(url);
            Box::pin(async { Ok(()) })
        }));
        options.on_authenticated = Some(Arc::new(move |_| {
            authenticated_in.fetch_add(1, Ordering::SeqCst);
        }));
    });
    let pending = service.begin_login(false).await.unwrap();
    assert!(opened.lock().unwrap()[0].starts_with("https://api.hlsr.pro/link"));
    assert_eq!(pending["userCode"], "ABCD-EFGH");
    assert!(!pending.to_string().contains(&"c".repeat(64)));
    assert!(timers.fire_timeout());
    service.settle().await;
    assert_eq!(authenticated.load(Ordering::SeqCst), 1);
    assert_eq!(service.snapshot()["account"]["signedIn"], true);
    assert!(!service.snapshot().to_string().contains(&token()));
    // `device: process.platform === "win32" ? "Windows" : "Linux"` (`:289`).
    let poll = transport
        .sent()
        .into_iter()
        .find(|sent| sent.url.ends_with("/poll"));
    assert_eq!(
        poll.unwrap().body()["device"],
        if cfg!(windows) { "Windows" } else { "Linux" }
    );
}

#[tokio::test]
async fn forged_login_url_receives_no_secret() {
    let dir = tempfile::tempdir().unwrap();
    let opened = Arc::new(AtomicBool::new(false));
    let transport = FnTransport::sync(|_| {
        reply(
            json!({"deviceCode": "c".repeat(64), "userCode": "ABCD-EFGH", "verifyUrl": "https://evil.example/link"}),
            200,
        )
    });
    let opened_in = opened.clone();
    let Client { service, .. } = client(dir.path(), &transport, |options| {
        options.open_external = Some(Arc::new(move |_| {
            opened_in.store(true, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }));
    });
    let error = service.begin_login(false).await.unwrap_err();
    assert!(error.message.contains("INVALID_LOGIN_URL"));
    assert!(!opened.load(Ordering::SeqCst));
}

#[tokio::test]
async fn no_history_is_sent_and_old_migration_file_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("game-stats.json"),
        r#"{"games":{"hl1-won":{"totalPlaySeconds":3600,"launchCount":2}}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("online-history-migration.json"),
        r#"{"version":1,"completed":{},"games":{}}"#,
    )
    .unwrap();
    sign_in(root, card(json!({})));
    let transport = profile_reply();
    let Client { service, .. } = client(root, &transport, |options| {
        options.account_provider = Some(Arc::new(|| LocalAccount {
            steam_id64: Some(ID.to_owned()),
            ..LocalAccount::default()
        }));
    });
    assert!(!root.join("online-history-migration.json").exists());
    service.sync().await;
    assert!(
        transport
            .sent()
            .iter()
            .all(|sent| !sent.url.ends_with("/migrate"))
    );
    assert_eq!(service.snapshot()["stats"]["totalSeconds"], 0);
    assert!(service.snapshot()["stats"].get("legacySeconds").is_none());
}

#[tokio::test]
async fn delayed_unauthorized_response_cannot_clear_newer_login() {
    let dir = tempfile::tempdir().unwrap();
    sign_in(dir.path(), card(json!({})));
    let finish: Arc<Mutex<Option<tokio::sync::oneshot::Sender<Response>>>> = Arc::default();
    let finish_in = finish.clone();
    let transport = FnTransport::new(move |_| {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        *finish_in.lock().unwrap() = Some(sender);
        Box::pin(async move { Ok(receiver.await.unwrap()) })
    });
    let Client { service, .. } = client(dir.path(), &transport, |_| {});
    let pending = tokio::spawn(service.call("/v2/runner/sync", json!({}), true));
    let newer = format!("{}z", &token()[..token().len() - 1]);
    service.set_token_for_tests(Some(newer.clone()));
    // Запрос уходит после подсчёта заголовка сборки — ждём, пока он будет отправлен.
    while finish.lock().unwrap().is_none() {
        tokio::task::yield_now().await;
    }
    let sender = finish.lock().unwrap().take().unwrap();
    sender.send(reply(json!({}), 401)).unwrap();
    let error = pending.await.unwrap().unwrap_err();
    assert!(error.message.contains("STALE_RESPONSE"));
    assert_eq!(service.get_auth_token(), Some(newer));
}

#[tokio::test]
async fn sign_in_survives_restart_without_keyring() {
    let dir = tempfile::tempdir().unwrap();
    let transport = by_suffix(|url, _| {
        if url.ends_with("/start") {
            reply(
                json!({"deviceCode": "c".repeat(64), "userCode": "ABCD-EFGH",
                    "verifyUrl": "https://api.hlsr.pro/link?code=ABCD-EFGH"}),
                200,
            )
        } else if url.ends_with("/poll") {
            reply(
                json!({"state": "ready", "token": token(), "steamId64": ID, "profile": card(json!({}))}),
                200,
            )
        } else {
            reply(json!({ "profile": card(json!({})) }), 200)
        }
    });
    let Client { service, timers } = client(dir.path(), &transport, |_| {});
    service.begin_login(false).await.unwrap();
    timers.fire_timeout();
    service.settle().await;
    assert_eq!(service.get_auth_token(), Some(token()));
    let restarted = client(dir.path(), &profile_reply(), |_| {}).service;
    assert_eq!(restarted.get_auth_token(), Some(token()));
    assert_eq!(restarted.snapshot()["status"], "signed-in");
}

#[tokio::test]
async fn closing_game_during_heartbeat_still_ends_session() {
    let dir = tempfile::tempdir().unwrap();
    sign_in(dir.path(), card(json!({})));
    let release: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>> = Arc::default();
    let release_in = release.clone();
    let transport = FnTransport::new(move |sent| {
        let route = sent.route();
        if route == "session/heartbeat" {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            *release_in.lock().unwrap() = Some(sender);
            return Box::pin(async move {
                receiver.await.unwrap();
                Ok(reply(json!({ "profile": card(json!({})) }), 200))
            });
        }
        Box::pin(async { Ok(reply(json!({ "profile": card(json!({})) }), 200)) })
    });
    let Client { service, .. } = client(dir.path(), &transport, |_| {});
    service.game_start("ghosting-renewed");
    service.settle().await;
    let heartbeat = tokio::spawn(service.beat("ghosting-renewed", false));
    while release.lock().unwrap().is_none() {
        tokio::task::yield_now().await;
    }
    service.game_end("ghosting-renewed");
    release.lock().unwrap().take().unwrap().send(()).unwrap();
    heartbeat.await.unwrap().unwrap();
    service.settle().await;
    let routes: Vec<String> = transport.sent().iter().map(Sent::route).collect();
    assert_eq!(
        routes,
        ["session/start", "session/heartbeat", "session/end"]
    );
    assert!(!service.is_game_active("ghosting-renewed"));
}

#[tokio::test]
async fn run_links_are_learned_ahead_and_remembered() {
    let dir = tempfile::tempdir().unwrap();
    sign_in(dir.path(), card(json!({})));
    let lookups = Arc::new(AtomicUsize::new(0));
    let lookups_in = lookups.clone();
    let transport = FnTransport::sync(move |sent| {
        if let Some(run) = sent
            .url
            .strip_prefix("https://www.speedrun.com/api/v1/runs/")
        {
            lookups_in.fetch_add(1, Ordering::SeqCst);
            return reply(
                json!({"data": {"id": run, "weblink": format!("https://www.speedrun.com/hl2hl1/runs/{run}")}}),
                200,
            );
        }
        let pinned = json!({"id": "yw7pd10z", "seconds": 2629.85, "url": "https://www.speedrun.com/runs/yw7pd10z"});
        reply(
            json!({"profile": card(json!({"details": {"pinnedRun": pinned}})), "runs": [
                {"id": "abcd1234", "seconds": 10, "url": "https://www.speedrun.com/runs/abcd1234"},
                pinned,
            ]}),
            200,
        )
    });
    let Client { service, .. } = client(dir.path(), &transport, |_| {});
    service
        .action("speedrun/bests", Some(&json!({})))
        .await
        .unwrap();
    service.settle().await;
    assert_eq!(
        lookups.load(Ordering::SeqCst),
        2,
        "закреплённый и список — по одному разу"
    );
    assert_eq!(
        service.speedrun_run_link("yw7pd10z").await.unwrap(),
        "https://www.speedrun.com/hl2hl1/runs/yw7pd10z"
    );
    assert_eq!(
        lookups.load(Ordering::SeqCst),
        2,
        "нажатие — без speedrun.com"
    );
    let again = client(dir.path(), &transport, |_| {}).service;
    assert_eq!(
        again.speedrun_run_link("abcd1234").await.unwrap(),
        "https://www.speedrun.com/hl2hl1/runs/abcd1234"
    );
    assert_eq!(
        lookups.load(Ordering::SeqCst),
        2,
        "ссылки пережили перезапуск"
    );
}

#[tokio::test]
async fn renderer_unlocks_only_its_own_and_congratulates_once() {
    let dir = tempfile::tempdir().unwrap();
    sign_in(dir.path(), card(json!({})));
    let unlocked: Arc<Mutex<Vec<Value>>> = Arc::default();
    let unlocked_in = unlocked.clone();
    let Client { service, .. } = client(dir.path(), &profile_reply(), |options| {
        options.on_unlock = Some(Arc::new(move |item| unlocked_in.lock().unwrap().push(item)));
    });
    assert!(
        service
            .unlock_from_renderer(Some(&json!("full-set")))
            .await
            .is_err()
    );
    assert!(service.unlock("made-up").await.is_err());
    let first = service
        .unlock_from_renderer(Some(&json!("wake-up")))
        .await
        .unwrap();
    assert_eq!(first["unlocked"], true);
    let second = service
        .unlock_from_renderer(Some(&json!("wake-up")))
        .await
        .unwrap();
    assert_eq!(second["unlocked"], false);
    service.achieve("marathon");
    service.settle().await;
    let unlocked = unlocked.lock().unwrap().clone();
    let ids: Vec<&str> = unlocked
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["wake-up", "marathon"]);
    assert_eq!(unlocked[0]["title"], "#L_ACH_WAKE_TITLE");
    let _restarted = client(dir.path(), &profile_reply(), |options| {
        options.account_provider = Some(Arc::new(LocalAccount::default));
    });
    assert_eq!(secrets_file(dir.path()), json!(["wake-up", "marathon"]));
}

#[tokio::test]
async fn without_login_nothing_unlocks_or_accumulates() {
    let dir = tempfile::tempdir().unwrap();
    let unlocked: Arc<Mutex<Vec<Value>>> = Arc::default();
    let unlocked_in = unlocked.clone();
    let Client { service, .. } = client(dir.path(), &profile_reply(), |options| {
        options.on_unlock = Some(Arc::new(move |item| unlocked_in.lock().unwrap().push(item)));
    });
    let result = service
        .unlock_from_renderer(Some(&json!("wake-up")))
        .await
        .unwrap();
    assert_eq!(result["unlocked"], false);
    service.achieve("marathon");
    service.settle().await;
    assert!(unlocked.lock().unwrap().is_empty());
    assert!(!dir.path().join("secrets.json").exists());
}

#[tokio::test]
async fn own_profile_in_search_finds_self_and_other_profile_scouts() {
    let dir = tempfile::tempdir().unwrap();
    sign_in(dir.path(), card(json!({})));
    let transport = by_suffix(|url, _| {
        if url.ends_with("/v2/runner/search") {
            reply(
                json!({"profiles": [card(json!({"steamId64": OTHER})), card(json!({}))]}),
                200,
            )
        } else if url.ends_with("/v2/runner/profile") {
            reply(json!({"profile": card(json!({"steamId64": OTHER}))}), 200)
        } else {
            reply(json!({"profile": card(json!({}))}), 200)
        }
    });
    let Client { service, .. } = client(dir.path(), &transport, |_| {});
    service.view(Some(&json!(OTHER))).await.unwrap();
    service.search(Some(&json!("Fake"))).await.unwrap();
    service.settle().await;
    let mut secrets: Vec<String> = serde_json::from_value(secrets_file(dir.path())).unwrap();
    secrets.sort();
    assert_eq!(secrets, ["found-self", "scout"]);
}

#[tokio::test]
async fn without_login_profiles_and_search_stay_closed() {
    let dir = tempfile::tempdir().unwrap();
    let transport = profile_reply();
    let Client { service, .. } = client(dir.path(), &transport, |_| {});
    let view = service.view(Some(&json!(OTHER))).await.unwrap_err();
    assert_eq!(view.code.as_deref(), Some("RUNNER_AUTH_REQUIRED"));
    let search = service.search(Some(&json!("Fake"))).await.unwrap_err();
    assert_eq!(search.code.as_deref(), Some("RUNNER_AUTH_REQUIRED"));
    assert!(transport.sent().is_empty());
}

#[tokio::test]
async fn unknown_secret_does_not_block_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    sign_in(dir.path(), card(json!({})));
    std::fs::write(dir.path().join("secrets.json"), r#"["chronicler","scout"]"#).unwrap();
    let sent: Arc<Mutex<Vec<String>>> = Arc::default();
    let sent_in = sent.clone();
    let transport = FnTransport::sync(move |request| {
        if !request.url.ends_with("/v2/runner/secret") {
            return reply(json!({"profile": card(json!({}))}), 200);
        }
        let id = request.body()["id"].as_str().unwrap().to_owned();
        let mut sent = sent_in.lock().unwrap();
        sent.push(id.clone());
        if id == "chronicler" {
            return reply(json!({"error": "Unknown secret"}), 400);
        }
        let known: Vec<&String> = sent.iter().filter(|x| *x != "chronicler").collect();
        reply(
            json!({"profile": card(json!({"details": {"secrets": known}}))}),
            200,
        )
    });
    let Client { service, .. } = client(dir.path(), &transport, |_| {});
    service.sync().await;
    assert_eq!(*sent.lock().unwrap(), ["chronicler", "scout"]);
    assert_eq!(service.snapshot()["details"]["secrets"], json!(["scout"]));
}

#[tokio::test]
async fn moderation_notices_once_and_shows_ban_and_mute() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    sign_in(root, card(json!({})));
    let moderation = Arc::new(Mutex::new(json!({
        "ban": {"id": 7, "reason": "Читы", "createdAt": "2026-10-09T10:00:00Z", "expiresAt": "2026-10-16T10:00:00Z"},
        "mute": null,
        "notices": [
            {"id": 7, "action": "ban", "reason": "Читы", "createdAt": "2026-10-09T10:00:00Z", "expiresAt": "2026-10-16T10:00:00Z"},
            {"id": 6, "action": "reset-nick", "reason": "", "createdAt": "2026-10-09T09:00:00Z", "expiresAt": null},
            {"id": 5, "action": "hack", "reason": "x", "createdAt": "2026-10-09T09:00:00Z", "expiresAt": null},
        ],
    })));
    let moderation_in = moderation.clone();
    let transport = FnTransport::sync(move |_| {
        reply(
            json!({"profile": card(json!({})), "moderation": moderation_in.lock().unwrap().clone()}),
            200,
        )
    });
    let notices: Arc<Mutex<Vec<Value>>> = Arc::default();
    let at = |iso: &str| hlsr_core::js::date_parse(iso).unwrap();
    let with = |now: i64| {
        let notices = notices.clone();
        move |options: &mut OnlineProfileOptions| {
            options.on_moderation = Some(Arc::new(move |item| notices.lock().unwrap().push(item)));
            options.clock = Arc::new(move || now);
        }
    };
    let Client { service, .. } = client(root, &transport, with(at("2026-10-10T00:00:00Z")));
    service.sync().await;
    let actions: Vec<Value> = notices
        .lock()
        .unwrap()
        .iter()
        .map(|item| item["action"].clone())
        .collect();
    assert_eq!(
        actions,
        [json!("reset-nick"), json!("ban")],
        "неизвестное действие пропущено, старые сверху"
    );
    assert_eq!(service.snapshot()["ban"]["reason"], "Читы");
    service.sync().await;
    assert_eq!(
        notices.lock().unwrap().len(),
        2,
        "повторная синхронизация повторила уведомление"
    );
    let restarted = client(root, &transport, with(at("2026-10-10T00:00:00Z"))).service;
    restarted.sync().await;
    assert_eq!(
        notices.lock().unwrap().len(),
        2,
        "после перезапуска уведомление пришло снова"
    );

    // Срок бана вышел — состояние его больше не показывает.
    let later = client(root, &transport, with(at("2026-10-17T00:00:00Z"))).service;
    later.sync().await;
    assert_eq!(later.snapshot()["ban"], Value::Null);

    // Запрет статуса приходит отказом на сохранение: состояние его подхватывает.
    *moderation.lock().unwrap() = json!({"ban": null, "mute": null, "notices": []});
    let moderation_in = moderation.clone();
    let muted_transport = FnTransport::sync(move |sent| {
        if sent.url.ends_with("/customize") {
            return reply(
                json!({"error": "MUTED", "mute": {"id": 9, "reason": "Спам", "createdAt": "2026-10-09T10:00:00Z", "expiresAt": null}}),
                403,
            );
        }
        reply(
            json!({"profile": card(json!({})), "moderation": moderation_in.lock().unwrap().clone()}),
            200,
        )
    });
    let muted = client(root, &muted_transport, |_| {}).service;
    let error = muted
        .call("/v2/runner/customize", json!({"status": "x"}), true)
        .await
        .unwrap_err();
    assert_eq!(error.code.as_deref(), Some("RUNNER_MUTED"));
    assert_eq!(muted.snapshot()["mute"]["reason"], "Спам");
}

#[tokio::test]
async fn rating_votes_cooldowns_and_no_self_vote() {
    let dir = tempfile::tempdir().unwrap();
    sign_in(dir.path(), card(json!({})));
    let bodies: Arc<Mutex<Vec<Value>>> = Arc::default();
    let mode = Arc::new(Mutex::new("ok"));
    let (bodies_in, mode_in) = (bodies.clone(), mode.clone());
    let transport = FnTransport::sync(move |sent| {
        if sent.url.ends_with("/v2/runner/profile") {
            return reply(
                json!({"profile": card(json!({"steamId64": OTHER, "rating": {"score": -12, "likes": 1, "dislikes": 13, "dubious": true}})),
                    "vote": {"value": -1, "nextAt": null}}),
                200,
            );
        }
        if !sent.url.ends_with("/v2/runner/rate") {
            return reply(json!({"profile": card(json!({}))}), 200);
        }
        bodies_in.lock().unwrap().push(sent.body());
        match *mode_in.lock().unwrap() {
            "like-limit" => reply(
                json!({"error": "LIKE_LIMIT", "likesAt": "2026-10-12T16:30:00Z",
                    "rating": {"score": 0, "likes": 0, "dislikes": 0, "dubious": false}, "vote": {"value": 0, "nextAt": null}}),
                429,
            ),
            "cooldown" => reply(
                json!({"error": "RATE_COOLDOWN", "rating": {"score": 1, "likes": 1, "dislikes": 0, "dubious": false},
                    "vote": {"value": 1, "nextAt": "2026-10-12T15:00:00Z"}}),
                429,
            ),
            _ => reply(
                json!({"rating": {"score": 1, "likes": 1, "dislikes": 0, "dubious": false},
                    "vote": {"value": 1, "nextAt": "2026-10-12T15:00:00Z"}}),
                200,
            ),
        }
    });
    let Client { service, .. } = client(dir.path(), &transport, |_| {});
    let view = service.view(Some(&json!(OTHER))).await.unwrap();
    assert_eq!(
        view["rating"],
        json!({"score": -12, "likes": 1, "dislikes": 13, "dubious": true})
    );
    assert_eq!(view["vote"]["value"], -1);
    let voted = service
        .rate(Some(&json!(OTHER)), Some(&json!(1)))
        .await
        .unwrap();
    assert_eq!(voted["status"], "ok");
    assert_eq!(
        bodies.lock().unwrap()[0],
        json!({"steamId64": OTHER, "value": 1})
    );
    *mode.lock().unwrap() = "cooldown";
    let waiting = service
        .rate(Some(&json!(OTHER)), Some(&json!(-1)))
        .await
        .unwrap();
    assert_eq!(waiting["status"], "cooldown");
    assert_eq!(waiting["vote"]["value"], 1);
    // Шестой лайк за 4 часа: сервер говорит, когда освободится следующий.
    *mode.lock().unwrap() = "like-limit";
    let limited = service
        .rate(Some(&json!("76561198000000333")), Some(&json!(1)))
        .await
        .unwrap();
    assert_eq!(limited["status"], "like-limit");
    assert_eq!(limited["likesAt"], "2026-10-12T16:30:00Z");
    assert_eq!(limited["vote"]["value"], 0);
    let own = service
        .rate(Some(&json!(ID)), Some(&json!(1)))
        .await
        .unwrap_err();
    assert!(own.message.contains("RUNNER_SELF_VOTE"));
    let bad = service
        .rate(Some(&json!(OTHER)), Some(&json!(2)))
        .await
        .unwrap_err();
    assert!(bad.message.contains("RUNNER_INVALID_ID"));
}

#[tokio::test]
async fn players_need_login_check_query_and_parse_cards() {
    let dir = tempfile::tempdir().unwrap();
    let sent: Arc<Mutex<Vec<Value>>> = Arc::default();
    let sent_in = sent.clone();
    let transport = FnTransport::sync(move |request| {
        let body = request.body();
        sent_in.lock().unwrap().push(body.clone());
        if request.url.ends_with("/v2/runner/players") && body["view"] == "online" {
            return reply(
                json!({"online": {"total": 3, "playing": 1, "profiles": [
                    card(json!({"steamId64": OTHER, "presence": {"online": true, "gameId": "hl1-won"}})),
                    {"steamId64": "bad"},
                ]}}),
                200,
            );
        }
        reply(
            json!({"top": {"more": true, "game": "hl1-won", "profiles": [
                card(json!({"rank": 51, "achievementCount": 4, "details": {"secrets": ["scout"]}})),
            ]}}),
            200,
        )
    });
    let signed_out = client(dir.path(), &transport, |_| {}).service;
    assert_eq!(
        signed_out
            .players(Some(&json!({"view": "online"})))
            .await
            .unwrap(),
        json!({"status": "ok", "signedIn": false})
    );
    assert!(sent.lock().unwrap().is_empty());
    sign_in(dir.path(), card(json!({})));
    let Client { service, .. } = client(dir.path(), &transport, |_| {});
    let online = service
        .players(Some(&json!({"view": "online"})))
        .await
        .unwrap();
    assert_eq!(online["total"], 3);
    assert_eq!(online["playing"], 1);
    let ids: Vec<&Value> = online["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| &p["steamId64"])
        .collect();
    assert_eq!(ids, [&json!(OTHER)]);
    assert_eq!(
        online["profiles"][0]["presence"],
        json!({"online": true, "gameId": "hl1-won", "launcherId": "70"})
    );
    let top = service
        .players(Some(
            &json!({"view": "top", "sort": "hours", "game": "hl1-won", "offset": 50}),
        ))
        .await
        .unwrap();
    assert_eq!(
        sent.lock().unwrap().last().unwrap(),
        &json!({"view": "top", "sort": "hours", "game": "hl1-won", "offset": 50})
    );
    assert_eq!(top["more"], true);
    assert_eq!(top["game"], "hl1-won");
    assert_eq!(top["profiles"][0]["rank"], 51);
    assert_eq!(top["profiles"][0]["achievementCount"], 4);
    let count = sent.lock().unwrap().len();
    for query in [
        json!({"sort": "nope"}),
        json!({"sort": "hours", "game": "hl3"}),
        json!({"sort": "hours", "offset": 7}),
        json!({"sort": "hours", "offset": 500}),
        json!({"sort": "hours", "offset": "0"}),
    ] {
        let mut query = query;
        query["view"] = json!("top");
        let error = service.players(Some(&query)).await.unwrap_err();
        assert!(error.message.contains("RUNNER_INVALID_QUERY"), "{query}");
    }
    assert_eq!(sent.lock().unwrap().len(), count);
}

/// `attestation-selftest.js:238-264`: истёкший билет — один повтор с новым.
#[tokio::test]
async fn expired_ticket_is_retried_once_with_a_new_one() {
    let dir = tempfile::tempdir().unwrap();
    let tickets: Arc<Mutex<Vec<bool>>> = Arc::default();
    let answers: Arc<Mutex<Vec<Response>>> = Arc::new(Mutex::new(vec![
        reply(json!({"error": "ATTEST_REQUIRED"}), 428),
        reply(json!({"ok": true}), 200),
    ]));
    let answers_in = answers.clone();
    let transport = FnTransport::sync(move |_| answers_in.lock().unwrap().remove(0));
    let tickets_in = tickets.clone();
    let build = format!("linux/0.4.5/{}", "a".repeat(64));
    let build_in = build.clone();
    let Client { service, .. } = client(dir.path(), &transport, move |options| {
        options.build_header = Some(Arc::new(move || {
            let build = build_in.clone();
            Box::pin(async move { Some(build) })
        }));
        options.ticket_provider = Some(Arc::new(move |force| {
            tickets_in.lock().unwrap().push(force);
            Box::pin(async move { Ok(Some(if force { "NEW" } else { "OLD" }.to_owned())) })
        }));
    });
    assert_eq!(
        service
            .call("/v1/link/start", json!({}), false)
            .await
            .unwrap(),
        json!({"ok": true})
    );
    assert_eq!(*tickets.lock().unwrap(), [false, true]);
    let sent = transport.sent();
    let sent_tickets: Vec<Option<&str>> = sent
        .iter()
        .map(|sent| sent.header("x-hlsr-ticket"))
        .collect();
    assert_eq!(sent_tickets, [Some("OLD"), Some("NEW")]);
    assert_eq!(sent[1].header("x-hlsr-build"), Some(build.as_str()));

    // Новый билет не помог: понятная ошибка вместо цикла повторов.
    *answers.lock().unwrap() = vec![
        reply(json!({"error": "ATTEST_REQUIRED"}), 428),
        reply(json!({"error": "ATTEST_REQUIRED"}), 428),
    ];
    transport.sent.lock().unwrap().clear();
    let error = service
        .call("/v1/link/start", json!({}), false)
        .await
        .unwrap_err();
    assert_eq!(error.code.as_deref(), Some("RUNNER_ATTEST_FAILED"));
    assert_eq!(transport.sent().len(), 2);

    // 428 без нашего кода (чужой прокси) повтором не лечится.
    *answers.lock().unwrap() = vec![reply_text("{}", 428)];
    transport.sent.lock().unwrap().clear();
    let error = service
        .call("/v1/link/start", json!({}), false)
        .await
        .unwrap_err();
    assert_eq!(error.code.as_deref(), Some("RUNNER_HTTP_428"));
    assert_eq!(transport.sent().len(), 1);
}

/// `attestation-selftest.js:266-283`: без сборки билет не просится; `CLIENT_MODIFIED` при
/// аттестации — выход и уведомление.
#[tokio::test]
async fn dev_build_asks_no_ticket_and_client_modified_signs_out() {
    let dir = tempfile::tempdir().unwrap();
    let asked = Arc::new(AtomicUsize::new(0));
    let modified = Arc::new(AtomicUsize::new(0));
    let transport = FnTransport::sync(|_| reply_text("{}", 200));
    let (asked_in, modified_in) = (asked.clone(), modified.clone());
    let Client { service, .. } = client(dir.path(), &transport, move |options| {
        options.build_header = Some(Arc::new(|| Box::pin(async { None })));
        options.ticket_provider = Some(Arc::new(move |_| {
            asked_in.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(Some("T".to_owned())) })
        }));
        options.on_client_modified = Some(Arc::new(move || {
            modified_in.fetch_add(1, Ordering::SeqCst);
        }));
    });
    service
        .call("/v1/link/start", json!({}), false)
        .await
        .unwrap();
    assert_eq!(asked.load(Ordering::SeqCst), 0);

    let refusing_transport = FnTransport::sync(|_| panic!("запроса быть не должно"));
    let modified_in = modified.clone();
    let refusing = client(dir.path(), &refusing_transport, move |options| {
        options.build_header = Some(Arc::new(|| {
            Box::pin(async { Some(format!("linux/0.4.5/{}", "a".repeat(64))) })
        }));
        options.ticket_provider = Some(Arc::new(|_| Box::pin(async { Err(ClientModified) })));
        options.on_client_modified = Some(Arc::new(move || {
            modified_in.fetch_add(1, Ordering::SeqCst);
        }));
    })
    .service;
    let error = refusing
        .call("/v1/link/start", json!({}), false)
        .await
        .unwrap_err();
    assert_eq!(error.code.as_deref(), Some("RUNNER_CLIENT_MODIFIED"));
    assert_eq!(modified.load(Ordering::SeqCst), 1);
    assert!(refusing_transport.sent().is_empty());
}

/// Таймер `start()` раз в 30 с: синхронизация и пульсы всех сессий (вне фикстуры — их порядок
/// между собой не задан).
#[tokio::test]
async fn interval_syncs_and_beats_every_session() {
    let dir = tempfile::tempdir().unwrap();
    sign_in(dir.path(), card(json!({})));
    let transport = profile_reply();
    let Client { service, timers } = client(dir.path(), &transport, |_| {});
    service.game_start("hl1-won");
    service.game_start("hl2-2707");
    service.settle().await;
    transport.sent.lock().unwrap().clear();
    service.start();
    service.settle().await;
    transport.sent.lock().unwrap().clear();
    assert!(timers.fire_interval());
    service.settle().await;
    let mut routes: Vec<String> = transport.sent().iter().map(Sent::route).collect();
    routes.sort();
    assert_eq!(routes, ["session/heartbeat", "session/heartbeat", "sync"]);
    let bodies: Vec<Value> = transport
        .sent()
        .iter()
        .filter(|sent| sent.route() == "session/heartbeat")
        .map(|sent| sent.body()["sequence"].clone())
        .collect();
    assert_eq!(bodies, [json!(2), json!(2)], "первый пульс — с sequence 2");
    service.stop();
    service.settle().await;
    assert!(timers.live().is_empty());
    assert!(!service.is_game_active("hl1-won") && !service.is_game_active("hl2-2707"));
}

/// Таймеры tokio: срабатывание, повтор, снятие. Время стоит и двигается само, когда все ждут
/// (`start_paused`): ни задержек, ни зависимости от нагрузки машины.
#[tokio::test(start_paused = true)]
async fn tokio_timers_fire_repeat_and_cancel() {
    let once = Arc::new(AtomicUsize::new(0));
    let ticks = Arc::new(AtomicUsize::new(0));
    let (once_in, ticks_in) = (once.clone(), ticks.clone());
    let timers = TokioTimers;
    let _timeout = timers.set_timeout(
        20.0,
        Box::new(move || {
            once_in.fetch_add(1, Ordering::SeqCst);
        }),
    );
    let interval = timers.set_interval(
        10.0,
        Arc::new(move || {
            ticks_in.fetch_add(1, Ordering::SeqCst);
        }),
    );
    tokio::time::sleep(std::time::Duration::from_millis(35)).await;
    assert_eq!(once.load(Ordering::SeqCst), 1);
    assert_eq!(ticks.load(Ordering::SeqCst), 3, "10, 20 и 30 мс");
    interval.cancel();
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert_eq!(ticks.load(Ordering::SeqCst), 3);
    assert_eq!(once.load(Ordering::SeqCst), 1);
}

// ── 0.4.9: достижения v2, время получения, серии (`online-profile-selftest.js` 0.4.9) ──

/// 2026-11-08T02:41:00Z.
const T: i64 = 1_794_105_660_000;
const HOUR: i64 = 3_600_000;

fn find_in(list: &Value, id: &str) -> Option<Value> {
    list.as_array()?
        .iter()
        .find(|item| item["id"] == id)
        .cloned()
}

fn at_of(service: &OnlineProfileService, id: &str) -> Value {
    find_in(&service.snapshot()["achievements"], id).unwrap()["at"].clone()
}

fn times_file(root: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(root.join("achievement-times.json")).unwrap())
        .unwrap()
}

fn has_secret(root: &Path, id: &str) -> bool {
    root.join("secrets.json").exists()
        && secrets_file(root).as_array().unwrap().contains(&json!(id))
}

#[tokio::test]
async fn server_achievements_come_only_from_the_server() {
    let dir = tempfile::tempdir().unwrap();
    sign_in(
        dir.path(),
        card(json!({"details": {"secrets": ["patron", "respected", "made-up", "scout"]}})),
    );
    let Client { service, .. } = client(dir.path(), &profile_reply(), |_| {});
    let snapshot = service.snapshot();
    assert_eq!(
        snapshot["details"]["secrets"],
        json!(["patron", "respected", "scout"])
    );
    assert_eq!(
        find_in(&snapshot["achievements"], "patron").unwrap()["unlocked"],
        true
    );
    assert!(
        find_in(&snapshot["achievements"], "king-of-the-hill").is_none(),
        "неполученный «Царь горы» — замок без id"
    );
    for id in [
        "patron",
        "king-of-the-hill",
        "particle-accelerator",
        "respected",
    ] {
        assert!(service.unlock(id).await.is_err(), "{id}");
        assert!(
            service
                .unlock_from_renderer(Some(&json!(id)))
                .await
                .is_err(),
            "{id}"
        );
    }
    assert!(!dir.path().join("secrets.json").exists());
}

#[tokio::test]
async fn foreign_secrets_are_sealed_for_the_viewer() {
    let dir = tempfile::tempdir().unwrap();
    sign_in(
        dir.path(),
        card(json!({"details": {"secrets": ["wake-up"]}})),
    );
    std::fs::write(dir.path().join("secrets.json"), r#"["hesoyam"]"#).unwrap();
    let transport = by_suffix(|url, _| {
        if url.ends_with("/v2/runner/profile") {
            let secrets = json!(["piano", "good-memory", "wake-up", "scout"]);
            reply(
                json!({"profile": card(json!({"steamId64": OTHER, "details": {"secrets": secrets}}))}),
                200,
            )
        } else {
            reply(json!({"profile": card(json!({}))}), 200)
        }
    });
    let Client { service, .. } = client(dir.path(), &transport, |_| {});
    let view = service.view(Some(&json!(OTHER))).await.unwrap();
    let list = view["achievements"].as_array().unwrap();
    let sealed: Vec<&Value> = list
        .iter()
        .filter(|item| item["unlocked"] == true && item["hidden"] == true)
        .collect();
    assert_eq!(sealed.len(), 2);
    for item in &sealed {
        assert_eq!(
            [&item["title"], &item["description"], &item["tier"]],
            [
                &json!("#L_ACH_SECRET_TITLE"),
                &json!("#L_ACH_SEALED_DESC"),
                &Value::Null
            ]
        );
        assert!(item["id"].as_str().unwrap().starts_with("secret-"));
    }
    let item = |id: &str| find_in(&view["achievements"], id).unwrap();
    assert_eq!(item("wake-up")["title"], "#L_ACH_WAKE_TITLE");
    assert_eq!(item("hesoyam")["hidden"], true);
    assert_eq!(item("hesoyam")["unlocked"], false);
    assert_eq!(
        view["summary"]["unlocked"], 5,
        "first-launch, scout, wake-up и два скрытых"
    );
    let ids: std::collections::HashSet<&str> = list
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), list.len(), "ключи списка разные");
    service.settle().await;
    let own = service.snapshot()["achievements"].clone();
    assert_eq!(
        find_in(&own, "hesoyam").unwrap()["title"],
        "#L_ACH_HESOYAM_TITLE"
    );
    assert!(
        find_in(&own, "piano").is_none(),
        "своё неполученное — замок без id"
    );
    assert!(
        own.as_array()
            .unwrap()
            .iter()
            .filter(|item| item["hidden"] == true)
            .all(|item| item["description"] == "#L_ACH_SECRET_DESC"
                && item["id"].as_str().unwrap().starts_with("secret-"))
    );
}

#[tokio::test]
async fn overview_has_launches_and_only_own_streak() {
    let dir = tempfile::tempdir().unwrap();
    let games = json!({"hl1-won": {"launchCount": 7, "totalPlaySeconds": 60}, "hl2-2707": {"launchCount": 5}});
    sign_in(dir.path(), card(json!({ "games": games })));
    let other = games.clone();
    let transport = by_suffix(move |url, _| {
        let steam_id = if url.ends_with("/v2/runner/profile") {
            OTHER
        } else {
            ID
        };
        reply(
            json!({"profile": card(json!({"steamId64": steam_id, "games": other}))}),
            200,
        )
    });
    let clock = support::FakeClock::new(T);
    let Client { service, .. } = client(dir.path(), &transport, |options| {
        options.clock = clock.clock();
        options.day_zone = DayZone::Fixed(0);
    });
    assert_eq!(service.snapshot()["stats"]["launches"], 12);
    assert_eq!(
        service.snapshot()["streak"],
        Value::Null,
        "дней ещё не было"
    );
    service.note_active_day();
    assert_eq!(service.snapshot()["streak"], json!({"days": 1, "best": 1}));
    let view = service.view(Some(&json!(OTHER))).await.unwrap();
    assert_eq!(view["stats"]["launches"], 12);
    assert!(view.get("streak").is_none(), "серию чужого сервер не знает");
    service.settle().await;
}

#[tokio::test]
async fn streak_goals_and_new_day_on_window_focus() {
    let dir = tempfile::tempdir().unwrap();
    sign_in(dir.path(), card(json!({})));
    // 2026-01-01T12:00:00Z и дальше по дню.
    let day = |n: i64| 1_767_268_800_000 + (n - 1) * 24 * HOUR;
    let clock = support::FakeClock::new(day(1));
    let Client { service, .. } = client(dir.path(), &profile_reply(), |options| {
        options.clock = clock.clock();
        options.day_zone = DayZone::Fixed(0);
    });
    // До первой отметки при старте фокус окна ничего не делает.
    service.note_new_day();
    assert!(!dir.path().join(launch_streak::FILE_NAME).exists());
    let goals = [
        (7, "week-streak"),
        (14, "hard-worker"),
        (30, "day-after-day"),
        (100, "keep-the-flame"),
    ];
    for n in 1..=100 {
        clock.set(day(n));
        service.note_active_day();
        service.settle().await;
        for (goal, id) in goals {
            assert_eq!(has_secret(dir.path(), id), n >= goal, "{id} на {n}-й день");
        }
    }
    let stats = |at: i64| launch_streak::stats_in_zone(dir.path(), at, DayZone::Fixed(0));
    clock.set(day(102));
    service.note_active_day();
    service.settle().await;
    assert_eq!(stats(day(102)), json!({"days": 1, "best": 100}));
    assert!(
        goals.iter().all(|(_, id)| has_secret(dir.path(), id)),
        "после пропуска полученное на месте"
    );
    assert_eq!(stats(day(105)), json!({"days": 0, "best": 100}));
    // Фокус в тот же день ничего не пишет; на следующий день — день засчитан.
    let file = dir.path().join(launch_streak::FILE_NAME);
    let before = std::fs::read(&file).unwrap();
    clock.set(day(102) + 3 * HOUR);
    service.note_new_day();
    assert_eq!(std::fs::read(&file).unwrap(), before);
    clock.set(day(103) - 11 * HOUR);
    service.note_new_day();
    assert_eq!(stats(clock.get()), json!({"days": 2, "best": 100}));
    service.settle().await;
}

#[tokio::test]
async fn meet_the_maker_and_impossible() {
    let dir = tempfile::tempdir().unwrap();
    sign_in(dir.path(), card(json!({})));
    let developer = Arc::new(AtomicBool::new(false));
    let developer_in = developer.clone();
    let country = Arc::new(Mutex::new("RU"));
    let country_in = country.clone();
    let transport = by_suffix(move |url, _| {
        if url.ends_with("/v2/runner/search") {
            let badges = if developer_in.load(Ordering::SeqCst) {
                json!(["developer", "verified"])
            } else {
                json!(["verified"])
            };
            reply(
                json!({"profiles": [card(json!({"steamId64": OTHER, "badges": badges}))]}),
                200,
            )
        } else {
            let country = *country_in.lock().unwrap();
            reply(
                json!({"profile": card(json!({"details": {"country": country}}))}),
                200,
            )
        }
    });
    let Client { service, .. } = client(dir.path(), &transport, |_| {});
    service.search(Some(&json!("runner"))).await.unwrap();
    service.settle().await;
    assert!(!has_secret(dir.path(), "meet-the-maker"));
    developer.store(true, Ordering::SeqCst);
    service.search(Some(&json!("dev"))).await.unwrap();
    service.settle().await;
    assert!(has_secret(dir.path(), "meet-the-maker"));
    let body = json!({"country": "RU"});
    service.action("customize", Some(&body)).await.unwrap();
    service.settle().await;
    assert!(!has_secret(dir.path(), "impossible"));
    *country.lock().unwrap() = "KP";
    service.action("customize", Some(&body)).await.unwrap();
    service.settle().await;
    assert!(has_secret(dir.path(), "impossible"));
}

#[tokio::test]
async fn achievement_time_is_stamped_once() {
    let dir = tempfile::tempdir().unwrap();
    sign_in(dir.path(), card(json!({})));
    let clock = support::FakeClock::new(T);
    let Client { service, .. } = client(dir.path(), &profile_reply(), |options| {
        options.clock = clock.clock();
    });
    service.unlock("marathon").await.unwrap();
    assert_eq!(at_of(&service, "marathon"), "2026-11-08T02:41:00.000Z");
    clock.advance(HOUR);
    service.unlock("marathon").await.unwrap();
    assert_eq!(at_of(&service, "marathon"), "2026-11-08T02:41:00.000Z");
    assert_eq!(times_file(dir.path()), json!({ ID: {"marathon": T} }));
}

#[tokio::test]
async fn thresholds_are_stamped_when_first_seen() {
    let dir = tempfile::tempdir().unwrap();
    sign_in(dir.path(), card(json!({})));
    let launches = Arc::new(Mutex::new(24));
    let launches_in = launches.clone();
    let transport = FnTransport::sync(move |_| {
        let count = *launches_in.lock().unwrap();
        reply(
            json!({"profile": card(json!({"games": {"hl1-won": {"launchCount": count}}}))}),
            200,
        )
    });
    let clock = support::FakeClock::new(T - 7 * 24 * HOUR);
    let Client { service, .. } = client(dir.path(), &transport, |options| {
        options.clock = clock.clock();
    });
    service.sync().await;
    assert_eq!(at_of(&service, "warmup"), Value::Null);
    clock.set(T);
    *launches.lock().unwrap() = 25;
    service.sync().await;
    assert_eq!(at_of(&service, "warmup"), "2026-11-08T02:41:00.000Z");
    assert_eq!(at_of(&service, "first-launch"), "2026-10-05T10:00:00Z");
}

#[tokio::test]
async fn flat_times_file_of_the_first_build_belongs_to_the_signed_in_account() {
    let dir = tempfile::tempdir().unwrap();
    sign_in(dir.path(), card(json!({})));
    std::fs::write(dir.path().join("secrets.json"), r#"["marathon","scout"]"#).unwrap();
    std::fs::write(
        dir.path().join("achievement-times.json"),
        format!(r#"{{"marathon":{T}}}"#),
    )
    .unwrap();
    let Client { service, .. } = client(dir.path(), &profile_reply(), |_| {});
    assert_eq!(at_of(&service, "marathon"), "2026-11-08T02:41:00.000Z");
    service.sync().await;
    assert_eq!(
        times_file(dir.path()),
        json!({ ID: {"marathon": T, "scout": null} })
    );
    // Испорченный файл времени не ломает список.
    std::fs::write(dir.path().join("achievement-times.json"), "{битый").unwrap();
    let Client { service, .. } = client(dir.path(), &profile_reply(), |_| {});
    assert_eq!(
        find_in(&service.snapshot()["achievements"], "marathon").unwrap()["unlocked"],
        true
    );
    assert_eq!(at_of(&service, "marathon"), Value::Null);
}
