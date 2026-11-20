//! Надёжность службы профиля там, где в JS её давала невозможность отменить промис: флаги «в
//! пути» (`syncInFlight`, `busy` сессии игры), записи о загрузках друзей и ссылок забегов и
//! таймеры не должны залипать, если будущее отбросили (вызов отменён, задача не запущена, служба
//! удалена). Сверять с 0.4.7 тут нечего: у него таких случаев не бывает.
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_online::online_profile::online_profile_service::{
    OnlineProfileOptions, OnlineProfileService, Timer, Timers, TokioTimers, open_external_via,
};
use hlsr_online::online_profile::profile_state::LocalAccount;
use hlsr_online::online_profile::session_store::{PlainVault, SESSION_FILE, Session, SessionStore};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::online_profile::{FnTransport, ManualTimers, Sent, reply};
use tokio::time::Instant;

const ID: &str = "76561198000000111";

fn token() -> String {
    format!("hlsr_{}_{}", "a".repeat(16), "b".repeat(43))
}

fn card() -> Value {
    json!({
        "steamId64": ID, "personaName": "Fake runner", "registeredAt": "2026-10-05T10:00:00Z",
        "games": {}, "details": {}, "badges": ["verified"], "generationId": "a".repeat(32),
    })
}

fn store(root: &Path) -> SessionStore {
    SessionStore::at(root.join(SESSION_FILE), Box::new(PlainVault))
}

fn sign_in(root: &Path) {
    store(root)
        .write(&Session {
            token: token(),
            profile: card(),
        })
        .unwrap();
}

/// Служба вошедшего пользователя с ручными таймерами.
fn service(root: &Path, transport: &FnTransport) -> OnlineProfileService {
    sign_in(root);
    let options = OnlineProfileOptions {
        account_provider: Some(Arc::new(LocalAccount::default)),
        timers: Arc::new(ManualTimers::new(Arc::default())),
        ..OnlineProfileOptions::new(root, store(root), Arc::new(transport.clone()))
    };
    OnlineProfileService::new(options)
}

/// Транспорт: всегда профиль; пока `hang` поднят, запросы с `route` повисают навсегда.
fn transport(hang: &Arc<AtomicBool>, route: &'static str) -> FnTransport {
    let hang = hang.clone();
    FnTransport::new(move |sent| {
        if hang.load(Ordering::SeqCst) && sent.route() == route {
            return Box::pin(std::future::pending());
        }
        let response = reply(json!({ "profile": card() }), 200);
        Box::pin(async move { Ok(response) })
    })
}

fn count(transport: &FnTransport, route: &str) -> usize {
    transport
        .sent()
        .iter()
        .filter(|sent| sent.route() == route)
        .count()
}

/// Ждать, пока на `route` придёт `n`-й запрос.
async fn until_sent(transport: &FnTransport, route: &str, n: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while count(transport, route) < n {
        assert!(Instant::now() < deadline, "нет запроса {route} #{n}");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

#[tokio::test]
async fn an_unpolled_sync_does_not_block_the_next_one() {
    let dir = tempfile::tempdir().unwrap();
    let transport = transport(&Arc::default(), "");
    let service = service(dir.path(), &transport);

    // Будущее получили и выбросили, не опрашивая: флага «в пути» после него быть не должно.
    drop(service.sync());
    let snapshot = service.sync().await;
    assert_eq!(snapshot["status"], "signed-in");
    assert_eq!(count(&transport, "sync"), 1);
}

#[tokio::test]
async fn a_cancelled_sync_does_not_block_the_next_one() {
    let dir = tempfile::tempdir().unwrap();
    let hang = Arc::new(AtomicBool::new(true));
    let transport = transport(&hang, "sync");
    let service = service(dir.path(), &transport);

    let stuck = tokio::spawn(service.sync());
    until_sent(&transport, "sync", 1).await;
    // Пока запрос висит, второй вызов ничего не шлёт (синхронизация уже идёт).
    service.sync().await;
    assert_eq!(count(&transport, "sync"), 1);
    stuck.abort();
    assert!(stuck.await.unwrap_err().is_cancelled());

    hang.store(false, Ordering::SeqCst);
    service.sync().await;
    assert_eq!(
        count(&transport, "sync"),
        2,
        "после отмены синхронизация снова идёт"
    );
}

#[tokio::test]
async fn a_cancelled_heartbeat_does_not_freeze_the_game_session() {
    let dir = tempfile::tempdir().unwrap();
    let hang = Arc::new(AtomicBool::new(false));
    let transport = transport(&hang, "session/heartbeat");
    let service = service(dir.path(), &transport);

    service.game_start("hl1-won");
    service.settle().await;
    assert_eq!(count(&transport, "session/start"), 1);

    hang.store(true, Ordering::SeqCst);
    let stuck = tokio::spawn(service.beat("hl1-won", false));
    until_sent(&transport, "session/heartbeat", 1).await;
    // Пульс в пути: ещё один и «закрыть» ничего не шлют, а лишь запоминают просьбу.
    service.beat("hl1-won", false).await.unwrap();
    assert_eq!(count(&transport, "session/heartbeat"), 1);
    stuck.abort();
    assert!(stuck.await.unwrap_err().is_cancelled());

    hang.store(false, Ordering::SeqCst);
    service.beat("hl1-won", false).await.unwrap();
    assert_eq!(
        count(&transport, "session/heartbeat"),
        2,
        "сессия не застряла «в пути»"
    );
}

/// Вне среды tokio фоновая загрузка не запускается; запись о ней не должна остаться и ронять
/// все следующие вызовы ошибкой.
#[test]
fn a_dead_friends_load_does_not_poison_later_calls() {
    let dir = tempfile::tempdir().unwrap();
    let transport = transport(&Arc::default(), "");
    let service = service(dir.path(), &transport);

    let dead = service.friends(false);
    drop(dead);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // Синхронная часть вызова должна выполниться уже в среде выполнения: `async`-блок.
    let friends = runtime
        .block_on(async { service.friends(false).await })
        .unwrap();
    assert_eq!(friends, json!({ "signedIn": true, "friends": [] }));
    assert_eq!(count(&transport, "steam-friends"), 1);
}

#[test]
fn a_dead_run_link_load_does_not_poison_later_calls() {
    let dir = tempfile::tempdir().unwrap();
    let transport = FnTransport::sync(|sent: &Sent| {
        assert!(
            sent.url
                .starts_with("https://www.speedrun.com/api/v1/runs/")
        );
        reply(
            json!({ "data": { "weblink": "https://www.speedrun.com/hl1/run/abcd1234" } }),
            200,
        )
    });
    let service = service(dir.path(), &transport);

    drop(service.speedrun_run_link("abcd1234"));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let link = runtime
        .block_on(async { service.speedrun_run_link("abcd1234").await })
        .unwrap();
    assert_eq!(link, "https://www.speedrun.com/hl1/run/abcd1234");
}

// ------------------------------------------------------------------ таймеры

/// Время стоит и двигается само, когда все ждут: сроки таймеров проверяются без ожидания.
#[tokio::test(start_paused = true)]
async fn a_dropped_tokio_timer_never_fires_again() {
    let once = Arc::new(AtomicUsize::new(0));
    let ticks = Arc::new(AtomicUsize::new(0));
    let (once_in, ticks_in) = (once.clone(), ticks.clone());
    let timeout: Box<dyn Timer> = TokioTimers.set_timeout(
        20.0,
        Box::new(move || {
            once_in.fetch_add(1, Ordering::SeqCst);
        }),
    );
    let interval: Box<dyn Timer> = TokioTimers.set_interval(
        10.0,
        Arc::new(move || {
            ticks_in.fetch_add(1, Ordering::SeqCst);
        }),
    );
    tokio::time::sleep(Duration::from_millis(35)).await;
    assert_eq!(ticks.load(Ordering::SeqCst), 3);
    assert_eq!(once.load(Ordering::SeqCst), 1);

    drop(interval);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        ticks.load(Ordering::SeqCst),
        3,
        "отпущенный интервал замолчал"
    );

    // Отпущенный до срабатывания таймер не срабатывает вовсе.
    let late = Arc::new(AtomicUsize::new(0));
    let late_in = late.clone();
    let pending = TokioTimers.set_timeout(
        50.0,
        Box::new(move || {
            late_in.fetch_add(1, Ordering::SeqCst);
        }),
    );
    drop(pending);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(late.load(Ordering::SeqCst), 0);
    drop(timeout);
}

/// Служба удалена — её таймер синхронизации не остаётся жить в среде выполнения.
#[tokio::test(start_paused = true)]
async fn dropping_the_service_stops_its_sync_timer() {
    let dir = tempfile::tempdir().unwrap();
    let transport = transport(&Arc::default(), "");
    let service = {
        sign_in(dir.path());
        let options = OnlineProfileOptions {
            account_provider: Some(Arc::new(LocalAccount::default)),
            ..OnlineProfileOptions::new(dir.path(), store(dir.path()), Arc::new(transport.clone()))
        };
        OnlineProfileService::new(options)
    };
    let alive = || {
        tokio::runtime::Handle::current()
            .metrics()
            .num_alive_tasks()
    };
    let before = alive();
    service.start();
    service.settle().await;
    assert!(alive() > before, "таймер синхронизации поставлен");
    drop(service);
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
    assert_eq!(alive(), before, "после службы не осталось задач");
}

// ------------------------------------------------------------------ браузер

/// Оболочка ОС порта `Shell`: открывает ссылки или отказывает, как заданно.
struct FakeShell {
    opened: std::sync::Mutex<Vec<String>>,
    refuse: AtomicBool,
}

impl hlsr_delivery::ports::Shell for FakeShell {
    fn open_path<'a>(&'a self, _path: &'a Path) -> hlsr_delivery::providers::BoxFuture<'a, String> {
        Box::pin(async { String::new() })
    }

    fn open_external(&self, url: &str) -> Result<(), String> {
        if self.refuse.load(Ordering::SeqCst) {
            return Err("браузер недоступен".to_owned());
        }
        self.opened.lock().unwrap().push(url.to_owned());
        Ok(())
    }

    fn show_item_in_folder(&self, _path: &Path) {}

    fn write_shortcut_link(
        &self,
        _path: &Path,
        _link: &hlsr_delivery::ports::ShortcutLink,
    ) -> bool {
        false
    }

    fn read_shortcut_link(
        &self,
        _path: &Path,
    ) -> Result<hlsr_delivery::ports::ShortcutLink, String> {
        Err("нет ярлыка".to_owned())
    }
}

/// Отказ ОС открыть браузер доходит до входа: `RUNNER_BROWSER_FAILED`, ожидание входа снято.
#[tokio::test]
async fn a_refusing_shell_ends_the_login_with_a_browser_error() {
    let dir = tempfile::tempdir().unwrap();
    let transport = FnTransport::sync(|_| {
        reply(
            json!({
                "schemaVersion": 1, "deviceCode": "c".repeat(64), "userCode": "ABCD-EFGH",
                "verifyUrl": "https://api.hlsr.pro/link?code=ABCD-EFGH",
                "expiresIn": 600, "interval": 5,
            }),
            200,
        )
    });
    let shell = Arc::new(FakeShell {
        opened: Mutex::default(),
        refuse: AtomicBool::new(true),
    });
    let options = OnlineProfileOptions {
        open_external: Some(open_external_via(shell.clone())),
        timers: Arc::new(ManualTimers::new(Arc::default())),
        ..OnlineProfileOptions::new(dir.path(), store(dir.path()), Arc::new(transport))
    };
    let service = OnlineProfileService::new(options);

    let error = service.begin_login(true).await.unwrap_err();
    assert_eq!(error.message, "RUNNER_BROWSER_FAILED");
    let snapshot = service.snapshot();
    assert_eq!(snapshot["status"], "signed-out");
    assert_eq!(snapshot["error"], "RUNNER_BROWSER_FAILED");
    assert!(shell.opened.lock().unwrap().is_empty());

    // Браузер снова открывается: вход начинается, ссылка уходит в оболочку.
    shell.refuse.store(false, Ordering::SeqCst);
    let pending = service.begin_login(true).await.unwrap();
    assert_eq!(pending["status"], "pending");
    assert_eq!(
        *shell.opened.lock().unwrap(),
        ["https://api.hlsr.pro/link?code=ABCD-EFGH"]
    );
}
