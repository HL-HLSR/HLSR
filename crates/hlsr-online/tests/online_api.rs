//! Связывание онлайна (Т4.13, сценарий С): фасад [`OnlineApi`] на двойнике API и поддельных
//! портах (окно, браузер, Discord без клиента, ручные таймеры профиля) — старт → вход → игра →
//! выход → закрытие. Сверяются события окна по порядку (`AppManager.js:163-303`, `:1055-1124`,
//! колбэки `OnlineProfileService`), запросы к API, события телеметрии в пачках и файлы папки данных.
//! Отдельно — каналы по имени (`call`), порт уведомлений доставки и перенос входа 0.4.7 до окна, журнал
//! IPC окна профиля (сцены S9/S11) на настоящем фасаде.
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_core::migration::MigrationLog;
use hlsr_core::paths::Paths;
use hlsr_delivery::ports::{MemoryLogger, VersionEventInfo};
use hlsr_online::api::{
    BoxFuture, LinkDialog, OnlineApi, OnlineEvent, OnlineEvents, OnlineOptions, OnlineWindow,
    channels, migrate_session,
};
use hlsr_online::discord_rpc::discord_rich_presence_bridge::ClientFactory;
use hlsr_online::online_profile::online_profile_service::OpenExternal;
use hlsr_online::online_profile::session_migration::STEP_ONLINE_SESSION;
use hlsr_online::online_profile::session_store::{PlainVault, SESSION_FILE, Session, SessionStore};
use hlsr_online::security::build_key::BuildKey;
use hlsr_online::security::official::Standing;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use support::FakeClock;
use support::online_profile::{Events, ManualTimers};
use support::server::{Journal, Server, start_server};

/// 2026-10-16T12:00:00Z.
const NOW: i64 = 1_792_152_000_000;
const TOKEN: &str = "hlsr_0123456789abcdef_Tk-_Tk-_Tk-_Tk-_Tk-_Tk-_Tk-_Tk-_Tk-_Tk-_abc";
const STEAM_ID: &str = "76561198000000111";
const VERSION: &str = "0.5.0";

/// Окно: видимость задаёт тест, показ и диалог записываются.
#[derive(Default)]
struct FakeWindow {
    visible: AtomicBool,
    /// Сколько раз у окна спросили, видно ли его (до входа в аккаунт — только опрос онлайна).
    visible_checks: AtomicUsize,
    reveals: AtomicUsize,
    confirm: AtomicBool,
    dialogs: Mutex<Vec<LinkDialog>>,
}

impl OnlineWindow for FakeWindow {
    fn visible(&self) -> bool {
        self.visible_checks.fetch_add(1, Ordering::SeqCst);
        self.visible.load(Ordering::SeqCst)
    }

    fn reveal(&self) {
        self.reveals.fetch_add(1, Ordering::SeqCst);
    }

    fn confirm_link(&self, dialog: LinkDialog) -> BoxFuture<'static, bool> {
        self.dialogs.lock().unwrap().push(dialog);
        let confirm = self.confirm.load(Ordering::SeqCst);
        Box::pin(async move { confirm })
    }
}

fn profile(extra: Value) -> Value {
    let mut profile = json!({
        "steamId64": STEAM_ID,
        "personaName": "Fake runner",
        "registeredAt": "2026-10-05T10:00:00Z",
        "games": {"hl1-won": {"totalPlaySeconds": 7200, "launchCount": 3,
            "lastLaunchAt": "2026-10-15T10:00:00Z", "lastExitAt": null}},
        "details": {},
        "badges": ["verified"],
        "generationId": "a".repeat(32),
    });
    for (key, value) in extra.as_object().unwrap() {
        profile[key] = value.clone();
    }
    profile
}

fn routes(base: &str) -> Value {
    json!({
        "GET /v1/public/online": [{"json": {"schemaVersion": 1, "launcherOnline": 12,
            "games": {"hl1-won": {"online": 3, "activeRuns": 1}}}}],
        "POST /v1/link/start": [{"json": {"schemaVersion": 1, "deviceCode": "c".repeat(64),
            "userCode": "ABCD-EFGH", "verifyUrl": format!("{base}/link?code=ABCD-EFGH"),
            "expiresIn": 600, "interval": 5}}],
        "POST /v1/link/poll": [{"json": {"state": "ready", "token": TOKEN, "steamId64": STEAM_ID,
            "profile": profile(json!({}))}}],
        "POST /v2/runner/sync": [{"json": {"profile": profile(json!({})), "moderation": {
            "ban": null, "mute": null, "notices": [{"id": 6, "action": "reset-nick", "reason": "",
                "createdAt": "2026-10-09T09:00:00Z", "expiresAt": null}]},
            "supportThanks": [{"id": "p-1", "kind": "subscription", "until": 1767225600}]}}],
        "POST /v2/runner/secret": [{"json": {"profile":
            profile(json!({"details": {"secrets": ["marathon"]}}))}}],
        "POST /v2/runner/session/start": [{"json": {"profile":
            profile(json!({"presence": {"online": true, "gameId": "hl1-won"}}))}}],
        "POST /v2/runner/session/end": [{"json": {}}],
        "POST /v2/runner/logout": [{"status": 204}],
        "POST /v1/telemetry/batch": [{"json": {"accepted": 1}}],
    })
}

struct Run {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    base: String,
    _server: Server,
    journal: Journal,
    window: Arc<FakeWindow>,
    opened: Arc<Mutex<Vec<String>>>,
    timers: ManualTimers,
    logger: Arc<MemoryLogger>,
    api: Arc<OnlineApi>,
    events: OnlineEvents,
}

async fn run() -> Run {
    run_with(None, true).await
}

/// [`run`]; `session` — вход, сохранённый до старта; `stand` — стенд считается официальной
/// сборкой (иначе — самосборка без ключа).
async fn run_with(session: Option<Session>, stand: bool) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("data");
    std::fs::create_dir_all(&root).unwrap();
    if let Some(session) = session {
        let store = SessionStore::at(root.join(SESSION_FILE), Box::new(PlainVault));
        store.write(&session).unwrap();
    }
    let journal: Journal = Arc::default();
    let server = start_server("api", journal.clone()).await;
    let base = format!("http://{}", server.authority);
    server.set_routes(&routes(&base));
    let window = Arc::new(FakeWindow::default());
    let opened: Arc<Mutex<Vec<String>>> = Arc::default();
    let open_external: OpenExternal = {
        let opened = opened.clone();
        Arc::new(move |url: String| {
            opened.lock().unwrap().push(url);
            Box::pin(async { Ok(()) })
        })
    };
    let timer_events: Events = Arc::default();
    let timers = ManualTimers::new(timer_events);
    let logger = Arc::new(MemoryLogger::default());
    let mut options = OnlineOptions::new(&root, VERSION, window.clone(), open_external);
    options.logger = logger.clone();
    options.clock = FakeClock::new(NOW).clock();
    options.api_base = Some(base.clone());
    options.session = SessionStore::at(root.join(SESSION_FILE), Box::new(PlainVault));
    options.discord = ClientFactory::Missing;
    options.install_dir = None;
    options.profile_timers = Arc::new(timers.clone());
    options.pretend_official = stand;
    if !stand {
        // Самосборка: ключа нет, даже если тест собран с вшитой маской.
        options.attest_key = Arc::new(BuildKey::new(None));
    }
    let api = OnlineApi::new(options);
    let events = api.subscribe();
    Run {
        _dir: dir,
        root,
        base,
        _server: server,
        journal,
        window,
        opened,
        timers,
        logger,
        api,
        events,
    }
}

impl Run {
    /// Каналы событий окна с прошлого вызова.
    fn channels(&mut self) -> Vec<&'static str> {
        let mut out = Vec::new();
        while let Some(event) = self.events.try_recv() {
            out.push(event.channel);
        }
        out
    }

    /// Ждать событие канала (фоновый запрос онлайна).
    async fn wait_for(&mut self, channel: &str) -> OnlineEvent {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = self.events.recv().await.expect("фасад жив");
                if event.channel == channel {
                    return event;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("нет события {channel}"))
    }

    /// Дождаться первого опроса онлайна, который `initialize()` запускает задачей: он спрашивает
    /// у окна, видно ли его. Пока он не спросил, показывать окно нельзя — запоздавший опрос
    /// увидел бы уже показанное окно и сделал второй запрос.
    async fn first_poll_saw_the_window(&self) {
        for _ in 0..3000 {
            if self.window.visible_checks.load(Ordering::SeqCst) > 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("опрос онлайна не спросил, видно ли окно");
    }

    /// Дождаться фоновой работы и дослать очередь телеметрии.
    async fn drain(&self) {
        self.api.settle().await;
        self.api.telemetry().sender().flush().await;
        self.api.settle().await;
    }

    /// Запросы к API (кроме телеметрии): `METHOD путь`.
    fn requests(&self) -> Vec<String> {
        let journal = std::mem::take(&mut *self.journal.lock().unwrap());
        let (telemetry, rest): (Vec<Value>, Vec<Value>) = journal
            .into_iter()
            .partition(|entry| entry["path"] == "/v1/telemetry/batch");
        self.journal.lock().unwrap().extend(telemetry);
        rest.iter()
            .map(|entry| {
                format!(
                    "{} {}",
                    entry["method"].as_str().unwrap(),
                    entry["path"].as_str().unwrap()
                )
            })
            .collect()
    }

    /// Типы событий телеметрии во всех пачках по порядку; у пачек — токен входа.
    fn telemetry_types(&self) -> Vec<String> {
        let journal = std::mem::take(&mut *self.journal.lock().unwrap());
        let mut types = Vec::new();
        for entry in journal {
            assert_eq!(entry["path"], "/v1/telemetry/batch", "{entry}");
            assert_eq!(
                entry["headers"]["authorization"],
                json!(format!("Bearer {TOKEN}"))
            );
            let body: Value = serde_json::from_str(entry["body"].as_str().unwrap()).unwrap();
            for event in body["events"].as_array().unwrap() {
                types.push(event["type"].as_str().unwrap().to_owned());
            }
        }
        types
    }

    fn file(&self, name: &str) -> Option<Value> {
        let text = std::fs::read_to_string(self.root.join(name)).ok()?;
        Some(serde_json::from_str(&text).unwrap())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_login_game_logout_close() {
    let mut run = run().await;

    // Старт: Discord, телеметрия без входа (очистка), опрос онлайна при скрытом окне пропущен.
    run.api.start_discord();
    run.api.initialize();
    run.first_poll_saw_the_window().await;
    run.drain().await;
    assert_eq!(run.channels(), Vec::<&str>::new());
    assert_eq!(run.requests(), Vec::<String>::new());

    // Окно загрузилось и показано: день для «Недели в Мезе», `ready`, счётчики онлайна.
    run.window.visible.store(true, Ordering::SeqCst);
    run.api.window_loaded();
    run.api.window_shown();
    let online = run.wait_for(channels::PUBLIC_ONLINE).await;
    assert_eq!(online.payload["launcherOnline"], 12);
    assert_eq!(run.api.public_online_get()["status"], "ok");
    assert_eq!(run.file("launch-days.json").unwrap()["streak"], 1);
    assert_eq!(run.requests(), ["GET /v1/public/online"]);

    // Вход: код устройства, браузер, опрос по таймеру, синхронизация (модерация → уведомление),
    // согласие на телеметрию, окно показано, `runner-login-success`.
    let login = run
        .api
        .call("runner-login", &[json!(true)])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(login["status"], "pending");
    assert_eq!(
        *run.opened.lock().unwrap(),
        [format!("{}/link?code=ABCD-EFGH", run.base)]
    );
    assert!(run.timers.fire_timeout(), "опрос входа");
    run.drain().await;
    // `beginLogin` → ожидание; `accept` синхронизации: модерация, «Спасибо» (уведомление и
    // событие окна), затем снимок; вход.
    assert_eq!(
        run.channels(),
        [
            channels::RUNNER_PROFILE_STATE,
            channels::NOTIFICATIONS_CHANGED,
            channels::NOTIFICATIONS_CHANGED,
            channels::SUPPORT_THANKS,
            channels::RUNNER_PROFILE_STATE,
            channels::RUNNER_LOGIN_SUCCESS,
        ]
    );
    assert_eq!(run.window.reveals.load(Ordering::SeqCst), 1);
    assert_eq!(
        run.requests(),
        [
            "POST /v1/link/start",
            "POST /v1/link/poll",
            "POST /v2/runner/sync"
        ]
    );
    assert!(run.root.join(SESSION_FILE).exists());
    assert_eq!(run.file("telemetry.json").unwrap()["enabled"], true);
    let list = run.api.notifications_list();
    assert_eq!(list["unread"], 2);
    // Оба с одним временем (часы стоят): порядок — как записаны.
    assert_eq!(list["items"][0]["code"], "MOD_RESET_NICK");
    assert_eq!(list["items"][0]["level"], "warning");
    assert_eq!(list["items"][1]["code"], "SUPPORT_THANKS");
    assert_eq!(list["items"][1]["level"], "info");
    assert_eq!(
        list["items"][1]["meta"],
        json!({"until": "2026-01-01T00:00:00Z"})
    );
    assert_eq!(
        run.file("support-thanks-seen.json").unwrap(),
        json!(["p-1"])
    );

    // Достижение от доставки или платформы (порт `Achievements`): поздравление и сервер.
    run.api.ports().achievements.achieve("marathon");
    run.drain().await;
    let mut seen = Vec::new();
    while let Some(event) = run.events.try_recv() {
        if event.channel == channels::ACHIEVEMENT_UNLOCKED {
            assert_eq!(
                event.payload,
                json!({"id": "marathon", "title": "#L_ACH_MARATHON_TITLE",
                    "icon": "fas fa-hourglass-half"})
            );
        }
        seen.push(event.channel);
    }
    assert_eq!(
        seen,
        [
            channels::RUNNER_PROFILE_STATE,
            channels::ACHIEVEMENT_UNLOCKED,
            channels::RUNNER_PROFILE_STATE
        ]
    );
    assert_eq!(run.file("secrets.json").unwrap(), json!(["marathon"]));
    assert_eq!(run.requests(), ["POST /v2/runner/secret"]);

    // Игра: сессия на сервере и игровая сессия телеметрии.
    run.api.game_starting(
        "hl1-won",
        Some(&json!({"version": "1.2.3", "buildId": "build-1"})),
    );
    run.api.game_running("hl1-won");
    run.drain().await;
    assert_eq!(
        run.channels(),
        [
            channels::RUNNER_PROFILE_STATE,
            channels::RUNNER_PROFILE_STATE
        ]
    );
    assert_eq!(run.requests(), ["POST /v2/runner/session/start"]);
    run.api.game_finished(
        "hl1-won",
        Some(&json!({"outcome": "exited", "requested": false, "exitCode": 0})),
        false,
    );
    run.drain().await;
    assert_eq!(run.channels(), [channels::RUNNER_PROFILE_STATE]);
    assert_eq!(run.requests(), ["POST /v2/runner/session/end"]);
    assert_eq!(
        run.telemetry_types(),
        [
            "launcher_session_started",
            "game_launch_started",
            "game_launch_succeeded",
            "game_session_ended"
        ]
    );

    // Выход: отзыв входа, согласие снято, очередь и маркер телеметрии очищены.
    let out = run.api.call("runner-logout", &[]).await.unwrap().unwrap();
    assert_eq!(out["status"], "signed-out");
    run.drain().await;
    assert_eq!(run.channels(), [channels::RUNNER_PROFILE_STATE]);
    assert_eq!(run.requests(), ["POST /v2/runner/logout"]);
    assert!(!run.root.join(SESSION_FILE).exists());
    assert_eq!(run.file("telemetry.json").unwrap()["enabled"], false);
    assert!(!run.root.join("telemetry-session-marker.json").exists());
    assert_eq!(run.api.telemetry().snapshot()["enabled"], false);

    // Discord: выключить — настройка в файле и событие окна.
    let state = run
        .api
        .call("discord-rpc-set", &[json!(false)])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state["enabled"], false);
    let event = run.wait_for(channels::DISCORD_RPC_STATE).await;
    assert_eq!(event.payload, state);
    assert_eq!(run.file("discord-rpc.json").unwrap()["enabled"], false);

    // Закрытие: Discord первым, затем телеметрия, онлайн и профиль; запросов больше нет.
    run.api.stop_discord();
    run.api.shutdown();
    tokio::time::timeout(Duration::from_secs(10), run.api.finish())
        .await
        .unwrap();
    run.drain().await;
    assert_eq!(run.channels(), Vec::<&str>::new());
    assert_eq!(run.requests(), Vec::<String>::new());
    assert_eq!(run.telemetry_types(), Vec::<String>::new());
    let lines = run.logger.take();
    assert!(
        lines.iter().all(|(level, _)| *level != "error"),
        "{lines:?}"
    );
}

/// Самосборка без ключа: счётчики онлайна не спрашиваются, сборка для окна не официальная.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_self_build_asks_the_server_nothing_public() {
    let mut run = run_with(None, false).await;
    assert_eq!(run.api.standing(), Standing::Unofficial);
    run.window.visible.store(true, Ordering::SeqCst);
    run.api.initialize();
    run.api.window_shown();
    run.drain().await;
    assert_eq!(run.api.public_online_get()["status"], "unknown");
    assert_eq!(run.requests(), Vec::<String>::new());
    assert!(!run.channels().contains(&channels::BUILD_OFFICIAL));
    run.api.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channels_by_name() {
    let run = run().await;
    let api = &run.api;
    assert_eq!(api.call("no-such-channel", &[]).await, None);
    assert_eq!(
        api.call("telemetry-status", &[]).await,
        None,
        "мёртвый канал"
    );

    // Уведомления: пусто, не строка — false.
    let call = |channel: &'static str, args: Vec<Value>| {
        let api = api.clone();
        async move { api.call(channel, &args).await.unwrap().unwrap() }
    };
    assert_eq!(
        call("notifications-list", vec![]).await,
        json!({"items": [], "unread": 0})
    );
    assert_eq!(
        call("notifications-mark-read", vec![json!(5)]).await,
        json!(false)
    );
    assert_eq!(
        call("notifications-mark-all-read", vec![]).await,
        json!(false)
    );
    assert_eq!(call("notifications-clear", vec![]).await, json!(false));

    // Лента: аргументы не тех типов — ошибка без кода, без запросов.
    let failed = json!({"status": "error", "code": "NEWS_FAILED"});
    assert_eq!(
        call("public-news-react", vec![json!(12), json!("like")]).await,
        failed
    );
    assert_eq!(call("public-news-mine", vec![json!("12")]).await, failed);
    assert_eq!(call("public-news-mine", vec![json!([12])]).await, failed);
    assert_eq!(call("public-news-open-link", vec![json!(5)]).await, failed);
    // Ссылка новости: вопрос на языке окна (`settings.json` нет — английский), отказ — `cancelled`.
    let reply = call(
        "public-news-open-link",
        vec![json!("https://example.com/path")],
    )
    .await;
    assert_eq!(reply, json!({"status": "cancelled"}));
    assert_eq!(
        *run.window.dialogs.lock().unwrap(),
        [LinkDialog {
            title: "Open link?".to_owned(),
            message: "This link opens the following website:\nexample.com".to_owned(),
            detail: "https://example.com/path".to_owned(),
            cancel: "Cancel".to_owned(),
            open: "Open in browser".to_owned(),
        }]
    );
    run.window.confirm.store(true, Ordering::SeqCst);
    let reply = call(
        "public-news-open-link",
        vec![json!("https://example.com/x")],
    )
    .await;
    assert_eq!(reply, json!({"status": "ok"}));
    assert_eq!(*run.opened.lock().unwrap(), ["https://example.com/x"]);
    // Реакция без входа — код службы профиля.
    let reply = call("public-news-react", vec![json!("12"), json!("like")]).await;
    assert_eq!(
        reply,
        json!({"status": "error", "code": "RUNNER_AUTH_REQUIRED"})
    );

    // Профиль без входа: снимок, друзья — без входа.
    assert_eq!(call("runner-state", vec![]).await["status"], "signed-out");
    assert_eq!(call("profile-get", vec![]).await["status"], "signed-out");
    assert_eq!(
        api.call("friends-list", &[json!({"force": true})]).await,
        Some(Ok(json!({"signedIn": false, "friends": []})))
    );
    assert_eq!(api.game_stats_from_server("hl1-won"), None);

    // Онлайн до первого ответа; Discord без клиента; новая личность телеметрии.
    assert_eq!(
        call("public-online-get", vec![]).await,
        json!({"status": "unknown", "launcherOnline": null, "games": {}, "updatedAt": null})
    );
    assert_eq!(call("discord-rpc-get", vec![]).await["enabled"], true);
    assert_eq!(
        call("telemetry-reset-identity", vec![]).await,
        json!({"ok": true, "hadIdentity": false})
    );
    assert!(run.logger.take().contains(&(
        "info",
        "[telemetry] идентичность сброшена по запросу пользователя".to_owned()
    )));
    assert!(run.journal.lock().unwrap().is_empty());
}

/// Журнал IPC окна профиля (сцены S9/S11, `hlsr-gpui::profile::tests`) на настоящем фасаде: каждый
/// канал обслужен, ответы — того вида, что ждёт окно (`RunnerSettings.vue` 0.4.9: код из 6 цифр и
/// срок; чужой профиль без `details.secrets`; выход везде — снимок без входа).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_window_journal_on_real_facade() {
    const OTHER: &str = "76561198000000222";
    let run = run_with(
        Some(Session {
            token: TOKEN.to_owned(),
            profile: profile(json!({})),
        }),
        true,
    )
    .await;
    let code = json!({"code": "123456", "expiresAt": 1_792_152_600});
    let mut routes = routes(&run.base);
    let extra = json!({
        "POST /v2/runner/search": [{"json": {"profiles": [profile(json!({"steamId64": OTHER}))]}}],
        "POST /v2/runner/profile": [{"json": {"profile": profile(json!({"steamId64": OTHER,
            "subscriber": true, "details": {"secrets": ["piano"]}})), "vote": {"value": 0}}}],
        "POST /v2/runner/customize": [{"json": {"profile": profile(json!({"stylePreset": 3,
            "perks": {"styles": true}}))}}],
        "POST /v2/runner/telegram/code": [{"json": code}],
        "POST /v2/runner/telegram/unlink": [{"json": {"profile": profile(json!({}))}}],
        "POST /v2/runner/discord/code": [{"json": code}],
        "POST /v2/runner/discord/unlink": [{"json": {"profile": profile(json!({}))}}],
        "POST /v2/runner/speedrun/bests": [{"json": {"runs": []}}],
        "POST /v2/runner/logout-all": [{"json": {}}],
    });
    for (key, value) in extra.as_object().unwrap() {
        routes[key] = value.clone();
    }
    run._server.set_routes(&routes);
    let journal = [
        ("runner-state", json!([])),
        ("profile-get", json!([])),
        ("runner-sync", json!([])),
        ("runner-search", json!(["ka"])),
        ("runner-profile-view", json!([OTHER])),
        ("runner-customize", json!([{"stylePreset": 3}])),
        ("runner-telegram-code", json!([])),
        ("runner-telegram-unlink", json!([])),
        ("runner-discord-code", json!([])),
        ("runner-discord-unlink", json!([])),
        ("runner-speedrun-bests", json!([])),
        ("runner-logout-all", json!([null])),
    ];
    let mut replies = Vec::new();
    for (channel, args) in journal {
        let reply = run.api.call(channel, args.as_array().unwrap()).await;
        let reply = reply.unwrap_or_else(|| panic!("{channel}: канала нет у фасада"));
        let reply = reply.unwrap_or_else(|e| panic!("{channel}: отказ {e}"));
        assert_ne!(reply["status"], "error", "{channel}: {reply}");
        replies.push(reply);
    }
    assert_eq!(
        replies[0]["telegram"],
        json!({"linked": false, "username": null})
    );
    let view = &replies[4];
    assert_eq!(view["subscriber"], true);
    assert!(
        view["details"].get("secrets").is_none(),
        "чужой профиль без details.secrets"
    );
    assert_eq!(replies[5]["snapshot"]["stylePreset"], 3);
    assert_eq!(replies[5]["snapshot"]["perks"]["styles"], true);
    for at in [6, 8] {
        assert_eq!(replies[at]["code"], "123456");
        assert_eq!(replies[at]["expiresAt"], 1_792_152_600);
    }
    assert_eq!(replies[11]["status"], "signed-out");
    let journal = run.requests();
    for path in [
        "POST /v2/runner/telegram/code",
        "POST /v2/runner/telegram/unlink",
        "POST /v2/runner/discord/code",
        "POST /v2/runner/discord/unlink",
        "POST /v2/runner/logout-all",
    ] {
        assert!(
            journal.iter().any(|line| line == path),
            "{path}: {journal:?}"
        );
    }
}

/// Журнал IPC окна «Спонсорство» (сцены S12, `hlsr-gpui::sponsors::tests`) и кнопок заготовок и
/// Telegram в настройках профиля (S11) на настоящем фасаде: `/v2` с ботом, адреса — только из
/// списка, ссылка записи — после диалога, бот без диалога; список для «Привязать» main
/// запрашивает сам.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sponsors_window_journal_on_real_facade() {
    let run = run().await;
    run.window.confirm.store(true, Ordering::SeqCst);
    let mut routes = routes(&run.base);
    routes["GET /v2/public/sponsors"] = json!([{"json": {
        "links": {"bot": "https://t.me/HLSRSupportBot", "donate": "https://web.tribute.tg/d"},
        "active": [{"card": {"personaName": "Активный", "subscriber": true}, "since": "2026-10-30",
            "link": {"mode": "profile", "steamId64": STEAM_ID}}],
        "donors": [{"card": null, "name": "Большой", "amountRub": 15000, "note": "",
            "link": {"mode": "url", "url": "https://example.com/me"}}],
        "past": []}}]);
    routes["GET /v2/public/style-presets"] = json!([{"json": [
        {"id": 3, "title": "Золото", "nickCss": "color: #ffd700", "frameCss": ""}]}]);
    run._server.set_routes(&routes);
    let journal = [
        ("support-open", json!(["bot", "123456"])),
        ("public-sponsors-get", json!([])),
        ("support-open", json!(["subscribe"])),
        (
            "sponsor-open-link",
            json!([{"section": "donors", "index": 0}]),
        ),
        (
            "sponsor-open-link",
            json!([{"section": "active", "index": 0}]),
        ),
        ("public-style-presets-get", json!([])),
    ];
    let mut replies = Vec::new();
    for (channel, args) in journal {
        let reply = run.api.call(channel, args.as_array().unwrap()).await;
        let reply = reply.unwrap_or_else(|| panic!("{channel}: канала нет у фасада"));
        replies.push(reply.unwrap_or_else(|e| panic!("{channel}: отказ {e}")));
    }
    assert_eq!(replies[0], json!({"status": "ok"}));
    let list = &replies[1];
    assert_eq!(
        (&list["status"], &list["version"]),
        (&json!("ok"), &json!(2))
    );
    assert_eq!(list["links"]["bot"], "https://t.me/HLSRSupportBot");
    assert_eq!(list["active"][0]["profile"]["subscriber"], true);
    assert_eq!(list["donors"][0]["amount"], 15000);
    assert_eq!(replies[2], json!({"status": "ok"}));
    assert_eq!(replies[3], json!({"status": "ok"}));
    assert_eq!(
        replies[4],
        json!({"status": "error", "code": "SPONSOR_LINK_INVALID"})
    );
    assert_eq!(replies[5]["status"], "ok");
    assert_eq!(replies[5]["presets"][0]["nick"]["css"], "color: #ffd700");
    assert_eq!(
        *run.opened.lock().unwrap(),
        [
            "https://t.me/HLSRSupportBot?start=123456",
            "https://t.me/HLSRSupportBot?start=subscribe",
            "https://example.com/me",
        ]
    );
    let dialogs = run.window.dialogs.lock().unwrap().clone();
    assert_eq!(dialogs.len(), 1, "диалог только у ссылки записи");
    assert_eq!(dialogs[0].detail, "https://example.com/me");
    assert!(dialogs[0].message.ends_with("\nexample.com"), "{dialogs:?}");
    // Список запрошен один раз (кеш минута), заготовки — один.
    assert_eq!(
        run.requests(),
        ["GET /v2/public/sponsors", "GET /v2/public/style-presets"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delivery_notifier_writes_notifications_and_version_events() {
    let mut run = run().await;
    let notifier = run.api.ports().notifier;
    // Код вне каталога молча пропускается (`levelForCode` → `null`).
    notifier.notify("NOT_IN_CATALOG", "x");
    assert_eq!(run.channels(), Vec::<&str>::new());
    notifier.notify("UPDATE_FAILED", "сеть");
    let info = VersionEventInfo {
        has_release_notes: true,
        launcher_id: Some("70".to_owned()),
    };
    notifier
        .game_update_available("1.1.0", Some("1.0.0"), "hl1-won", &info)
        .unwrap();
    // Повтор той же версии не объявляется (`version-state.json`).
    notifier
        .game_update_available("1.1.0", Some("1.0.0"), "hl1-won", &info)
        .unwrap();
    notifier
        .resolve_update_offers("game", Some("1.1.0"), "hl1-won")
        .unwrap();
    assert_eq!(
        run.channels(),
        [
            channels::NOTIFICATIONS_CHANGED,
            channels::NOTIFICATIONS_CHANGED,
            channels::NOTIFICATIONS_CHANGED
        ]
    );
    let list = run.api.notifications_list();
    let codes: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["UPDATE_FAILED"]);
    assert_eq!(list["items"][0]["level"], "error");
    assert_eq!(list["items"][0]["detail"], "сеть");
    assert!(
        run.file("version-state.json").unwrap()["seen"]
            .as_object()
            .unwrap()
            .keys()
            .any(|key| key.starts_with("GAME_UPDATE_AVAILABLE:hl1-won:1.1.0"))
    );
    // Канал окна: отметить прочитанным по id.
    let id = list["items"][0]["id"].clone();
    assert_eq!(
        run.api.call("notifications-mark-read", &[id]).await,
        Some(Ok(json!(true)))
    );
    assert_eq!(run.channels(), [channels::NOTIFICATIONS_CHANGED]);
    assert_eq!(run.api.notifications_list()["unread"], 0);
}

/// Вход 0.4.7 (`online-session.json`, конверт `plain`) переносится до окна один раз; служба
/// профиля видит его как вход.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_migration_before_window() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    let legacy = json!({"version": 1, "plain": {"token": TOKEN, "profile": profile(json!({}))}});
    std::fs::write(paths.file("online-session.json"), legacy.to_string()).unwrap();
    let store = SessionStore::at(paths.file(SESSION_FILE), Box::new(PlainVault));
    let logger = MemoryLogger::default();
    let report = migrate_session(&paths, &store, &logger, SystemTime::now());
    assert!(!report.skipped && report.errors.is_empty(), "{report:?}");
    assert_eq!(store.read().unwrap().token, TOKEN);
    assert!(MigrationLog::open(&paths).is_done(STEP_ONLINE_SESSION));
    // Файл 0.4.7 не тронут; повтор — пропуск.
    assert_eq!(
        std::fs::read_to_string(paths.file("online-session.json")).unwrap(),
        legacy.to_string()
    );
    assert!(migrate_session(&paths, &store, &logger, SystemTime::now()).skipped);
    assert!(logger.take().is_empty());

    // Фасад поверх перенесённого входа: снимок «вошёл», токен — у телеметрии.
    let window: Arc<dyn OnlineWindow> = Arc::new(FakeWindow::default());
    let mut options = OnlineOptions::new(
        &paths.data_dir,
        VERSION,
        window,
        Arc::new(|_| Box::pin(async { Ok(()) })),
    );
    options.session = store;
    options.discord = ClientFactory::Missing;
    options.install_dir = None;
    options.api_base = Some("http://127.0.0.1:9".to_owned());
    let api = OnlineApi::new(options);
    assert_eq!(
        api.call("runner-state", &[]).await.unwrap().unwrap()["status"],
        "signed-in"
    );
    assert_eq!(api.profiles().get_auth_token().as_deref(), Some(TOKEN));
}

/// Без `online-session.json` переносить нечего: шаг отмечается, служба ключей не нужна.
#[test]
fn session_migration_without_legacy_file() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    let store = SessionStore::at(paths.file(SESSION_FILE), Box::new(PlainVault));
    let report = migrate_session(&paths, &store, &MemoryLogger::default(), SystemTime::now());
    assert!(!report.skipped && report.errors.is_empty());
    assert!(store.read().is_none());
    assert!(!Path::new(&paths.file(SESSION_FILE)).exists());
    assert!(MigrationLog::open(&paths).is_done(STEP_ONLINE_SESSION));
}
