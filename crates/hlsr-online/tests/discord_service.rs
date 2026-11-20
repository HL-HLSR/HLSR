//! Служба присутствия Discord (`DiscordRichPresenceService.js`) на поддельном клиенте: то, чего
//! настоящий `discord-rpc` не даёт сделать (исключение фабрики, исключение `setActivity`, коды
//! ошибок разных видов, порядок обратных вызовов) и возвращаемые значения методов. Остальное —
//! в `discord_rpc.rs` против 0.4.7 с настоящей библиотекой. Ожидания взяты из текста
//! `DiscordRichPresenceService.js`.

#[path = "support/discord.rs"]
mod discord;
mod support;

use discord::FakeTimers;
use hlsr_delivery::ports::MemoryLogger;
use hlsr_online::discord_rpc::constants::{Code, State};
use hlsr_online::discord_rpc::discord_rich_presence_service::{
    BoxFuture, CreateClient, CreateClientOptions, DiscordRichPresenceService, PresenceClient,
    ServiceOptions,
};
use hlsr_online::discord_rpc::presence_model::Activity;
use hlsr_online::discord_rpc::rpc::client::RpcError;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::FakeClock;
use tokio::sync::oneshot;

struct FakeClient {
    calls: Mutex<Vec<String>>,
    login: Mutex<Option<oneshot::Receiver<Result<(), RpcError>>>>,
    set_activity_error: Mutex<Option<RpcError>>,
    reply_error: Mutex<Option<RpcError>>,
    disconnected: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>,
}

impl FakeClient {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn push(&self, call: &str) {
        self.calls.lock().unwrap().push(call.to_owned());
    }

    /// Закрытие соединения, как его видит служба (`client.emit("disconnected")`).
    fn emit_disconnected(&self) {
        let handlers = self.disconnected.lock().unwrap().clone();
        for handler in handlers {
            handler();
        }
    }
}

impl PresenceClient for FakeClient {
    fn on_disconnected(&self, handler: Arc<dyn Fn() + Send + Sync>) {
        self.disconnected.lock().unwrap().push(handler);
    }

    fn login(&self, client_id: &str) -> BoxFuture<Result<(), RpcError>> {
        self.push(&format!("login {client_id}"));
        let receiver = self.login.lock().unwrap().take().expect("login один раз");
        Box::pin(async move {
            receiver
                .await
                .unwrap_or_else(|_| Err(RpcError::new("брошен")))
        })
    }

    fn set_activity(
        &self,
        activity: &Activity,
    ) -> Result<BoxFuture<Result<Value, RpcError>>, RpcError> {
        self.push(&format!("setActivity {}", activity.details));
        if let Some(error) = self.set_activity_error.lock().unwrap().clone() {
            return Err(error);
        }
        let reply = self.reply_error.lock().unwrap().clone();
        Ok(Box::pin(async move {
            match reply {
                Some(error) => Err(error),
                None => Ok(Value::Null),
            }
        }))
    }

    fn clear_activity(&self) -> BoxFuture<Result<Value, RpcError>> {
        self.push("clearActivity");
        Box::pin(async { Ok(Value::Null) })
    }

    fn destroy(&self) -> BoxFuture<()> {
        self.push("destroy");
        Box::pin(async {})
    }
}

/// Ответы на `login` поддельных клиентов: по одному на клиента, в порядке создания.
type Logins = Arc<Mutex<Vec<oneshot::Sender<Result<(), RpcError>>>>>;

struct Rig {
    service: DiscordRichPresenceService,
    timers: Arc<FakeTimers>,
    clock: FakeClock,
    logger: Arc<MemoryLogger>,
    clients: Arc<Mutex<Vec<Arc<FakeClient>>>>,
    logins: Logins,
    connections: Arc<Mutex<Vec<bool>>>,
    factory_error: Arc<Mutex<Option<RpcError>>>,
}

impl Rig {
    fn new(with_factory: bool) -> Rig {
        let logger = Arc::new(MemoryLogger::default());
        let timers = Arc::new(FakeTimers::default());
        let clock = FakeClock::new(1_792_152_000_000);
        let clients: Arc<Mutex<Vec<Arc<FakeClient>>>> = Arc::default();
        let logins: Logins = Arc::default();
        let connections: Arc<Mutex<Vec<bool>>> = Arc::default();
        let factory_error: Arc<Mutex<Option<RpcError>>> = Arc::default();

        let create_client: Option<CreateClient> = with_factory.then(|| {
            let clients = clients.clone();
            let logins = logins.clone();
            let factory_error = factory_error.clone();
            Arc::new(move |_options: &CreateClientOptions| {
                if let Some(error) = factory_error.lock().unwrap().clone() {
                    return Err(error);
                }
                let (sender, receiver) = oneshot::channel();
                logins.lock().unwrap().push(sender);
                let client = Arc::new(FakeClient {
                    calls: Mutex::default(),
                    login: Mutex::new(Some(receiver)),
                    set_activity_error: Mutex::default(),
                    reply_error: Mutex::default(),
                    disconnected: Mutex::default(),
                });
                clients.lock().unwrap().push(client.clone());
                Ok(client as Arc<dyn PresenceClient>)
            }) as CreateClient
        });
        let mut options = ServiceOptions::new(logger.clone(), create_client);
        options.now = clock.clock();
        options.timers = Some(timers.clone());
        options.random = support::sequence_random(vec![0.5]);
        let sink = connections.clone();
        options.on_connection_change = Some(Arc::new(move |connected| {
            sink.lock().unwrap().push(connected);
        }));
        Rig {
            service: DiscordRichPresenceService::new(options),
            timers,
            clock,
            logger,
            clients,
            logins,
            connections,
            factory_error,
        }
    }

    fn client(&self, at: usize) -> Arc<FakeClient> {
        self.clients.lock().unwrap()[at].clone()
    }

    fn login_ok(&self, at: usize) {
        self.logins.lock().unwrap().remove(at).send(Ok(())).unwrap();
    }

    fn lines(&self) -> Vec<String> {
        self.logger
            .take()
            .into_iter()
            .map(|(_, text)| text)
            .collect()
    }

    async fn until(&self, what: &str, ready: impl Fn() -> bool) {
        for _ in 0..400 {
            if ready() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("не дождались: {what}");
    }

    async fn settle(&self) {
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
}

fn launcher() -> Value {
    json!({"kind": "launcher", "sessionStartedAtMs": 1760000000123u64})
}

#[tokio::test]
async fn enable_disable_shutdown_return_values() {
    let rig = Rig::new(true);
    let service = &rig.service;
    assert_eq!(service.public_state().state, State::Disabled);
    assert!(!service.is_enabled());
    assert!(!service.disable(), "уже выключено");
    assert!(service.enable());
    assert!(!service.enable(), "уже включено");
    assert!(service.is_enabled());
    assert!(service.disable());
    assert!(!service.disable());
    assert!(service.enable());
    assert!(service.shutdown());
    assert!(!service.shutdown());
    assert!(!service.enable(), "после выхода не включается");
    assert_eq!(service.public_state().state, State::ShuttingDown);
    assert_eq!(service.public_state().code, Some(Code::Shutdown));
}

#[tokio::test]
async fn factory_error_with_code_goes_to_retry() {
    let rig = Rig::new(true);
    *rig.factory_error.lock().unwrap() = Some(RpcError {
        message: "нет".into(),
        code: Some(json!("EACCES")),
    });
    rig.service.enable();
    let state = rig.service.public_state();
    assert_eq!(state.state, State::RetryWait);
    assert_eq!(state.attempt, 1);
    assert_eq!(
        rig.lines(),
        [
            "[discord-rpc] RPC_CONNECTING: включено пользователем",
            "[discord-rpc] RPC_DISCORD_UNAVAILABLE: EACCES"
        ]
    );
    assert_eq!(rig.timers.len(), 1);
}

#[tokio::test]
async fn login_error_code_is_stringified_when_truthy() {
    for (code, suffix) in [
        (Some(json!(5)), ": 5"),
        (Some(json!(0)), ""),
        (Some(json!("")), ""),
        (None, ""),
        (Some(json!(true)), ": true"),
    ] {
        let rig = Rig::new(true);
        rig.service.enable();
        rig.until("клиент", || !rig.clients.lock().unwrap().is_empty())
            .await;
        rig.logins
            .lock()
            .unwrap()
            .remove(0)
            .send(Err(RpcError {
                message: "x".into(),
                code,
            }))
            .unwrap();
        rig.until("повтор", || {
            rig.service.public_state().state == State::RetryWait
        })
        .await;
        assert_eq!(
            rig.lines().last().unwrap(),
            &format!("[discord-rpc] RPC_DISCORD_UNAVAILABLE{suffix}")
        );
        // Отказавший клиент уничтожен.
        assert_eq!(rig.client(0).calls().last().unwrap(), "destroy");
    }
}

#[tokio::test]
async fn presence_set_while_disabled_is_sent_on_connect() {
    let rig = Rig::new(true);
    assert!(
        !rig.service.set_presence(&launcher()),
        "выключено — отправки не будет"
    );
    assert_eq!(rig.timers.len(), 0);
    assert!(!rig.service.set_presence(&launcher()), "та же модель");

    rig.service.enable();
    rig.until("клиент", || !rig.clients.lock().unwrap().is_empty())
        .await;
    rig.login_ok(0);
    rig.until("подключено", || rig.service.connected()).await;
    rig.until("отправка", || rig.client(0).calls().len() >= 2)
        .await;
    assert_eq!(
        rig.client(0).calls(),
        ["login 1531057982518071296", "setActivity HLSR Launcher"]
    );
    assert_eq!(*rig.connections.lock().unwrap(), [true]);
    assert_eq!(
        rig.service.public_state().last_connected_at,
        Some(1_792_152_000_000)
    );
}

#[tokio::test]
async fn set_activity_exception_is_update_failed_and_resent_later() {
    let rig = Rig::new(true);
    rig.service.enable();
    rig.until("клиент", || !rig.clients.lock().unwrap().is_empty())
        .await;
    let client = rig.client(0);
    *client.set_activity_error.lock().unwrap() = Some(RpcError::new(
        "timestamps.start must fit into a unix timestamp",
    ));
    rig.service.set_presence(&launcher());
    rig.login_ok(0);
    rig.until("подключено", || rig.service.connected()).await;

    // Исключение при вызове: строка в журнал, модель не считается отправленной.
    assert_eq!(client.calls().len(), 2);
    assert_eq!(rig.service.public_state().code, Some(Code::UpdateFailed));
    assert!(
        rig.lines()
            .contains(&"[discord-rpc] RPC_UPDATE_FAILED".to_owned())
    );
    // Модель не записана как отправленная: следующий `flush` пробует снова и снова бросает.
    assert!(!rig.service.flush());
    assert_eq!(client.calls().len(), 3);

    // Ошибка снята — тот же flush отправляет снова (модель не была записана как отправленная).
    *client.set_activity_error.lock().unwrap() = None;
    assert!(rig.service.flush());
    rig.settle().await;
    assert!(
        !rig.service.flush(),
        "теперь отправленное совпадает с ожидающим"
    );
}

#[tokio::test]
async fn debounce_sends_only_the_last_model() {
    let rig = Rig::new(true);
    rig.service.enable();
    rig.until("клиент", || !rig.clients.lock().unwrap().is_empty())
        .await;
    rig.login_ok(0);
    rig.until("подключено", || rig.service.connected()).await;
    rig.settle().await;
    let client = rig.client(0);
    let before = client.calls().len();

    assert!(
        rig.service
            .set_presence(&json!({"kind": "browsing", "gameId": "hl1-won"}))
    );
    assert!(
        rig.service
            .set_presence(&json!({"kind": "browsing", "gameId": "hl2-4260"}))
    );
    assert_eq!(rig.timers.len(), 1, "второй вызов заменил таймер");
    assert_eq!(client.calls().len(), before);
    assert!(rig.timers.fire("debounce"));
    rig.settle().await;
    assert_eq!(client.calls().len(), before + 1);
    assert_eq!(
        client.calls().last().unwrap(),
        "setActivity Half-Life 2 (4260)"
    );
}

#[tokio::test]
async fn stale_login_after_disable_is_destroyed_without_activity() {
    let rig = Rig::new(true);
    rig.service.set_presence(&launcher());
    rig.service.enable();
    rig.until("клиент", || !rig.clients.lock().unwrap().is_empty())
        .await;
    rig.service.disable();
    rig.login_ok(0);
    rig.until("уничтожен", || {
        rig.client(0).calls().contains(&"destroy".to_owned())
    })
    .await;
    rig.settle().await;
    // Присутствия не было: клиент не принят.
    assert_eq!(
        rig.client(0).calls(),
        ["login 1531057982518071296", "destroy"]
    );
    assert_eq!(rig.service.public_state().state, State::Disabled);
    assert!(!rig.service.connected());
    assert!(rig.connections.lock().unwrap().is_empty());
}

#[tokio::test]
async fn disconnect_schedules_retry_and_new_client_resends() {
    let rig = Rig::new(true);
    rig.service.set_presence(&launcher());
    rig.service.enable();
    rig.until("клиент", || !rig.clients.lock().unwrap().is_empty())
        .await;
    rig.login_ok(0);
    rig.until("подключено", || rig.service.connected()).await;
    rig.settle().await;
    assert_eq!(rig.client(0).calls().len(), 2);

    rig.client(0).emit_disconnected();
    let state = rig.service.public_state();
    assert_eq!(state.state, State::RetryWait);
    assert_eq!(state.code, Some(Code::Disconnected));
    assert!(!rig.service.connected());
    assert_eq!(*rig.connections.lock().unwrap(), [true, false]);
    // Повтор через 5 с ± 25 % (random 0.5 — ровно 5 с).
    assert_eq!(
        rig.timers.describe(),
        json!([{"kind": "retry", "base": 5000}])
    );

    rig.clock.advance(61_000);
    assert!(rig.timers.fire("retry"));
    rig.until("второй клиент", || {
        rig.clients.lock().unwrap().len() == 2
    })
    .await;
    rig.login_ok(0);
    rig.until("отправка", || rig.client(1).calls().len() >= 2)
        .await;
    assert_eq!(rig.client(1).calls()[1], "setActivity HLSR Launcher");
    assert_eq!(*rig.connections.lock().unwrap(), [true, false, true]);
}

#[tokio::test]
async fn retry_after_disable_does_nothing() {
    let rig = Rig::new(true);
    rig.service.enable();
    rig.until("клиент", || !rig.clients.lock().unwrap().is_empty())
        .await;
    rig.logins
        .lock()
        .unwrap()
        .remove(0)
        .send(Err(RpcError::new("нет")))
        .unwrap();
    rig.until("повтор", || {
        rig.service.public_state().state == State::RetryWait
    })
    .await;
    rig.service.disable();
    assert_eq!(rig.timers.len(), 0, "disable снимает таймеры");
    assert!(!rig.timers.fire("retry"));
    assert_eq!(rig.clients.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn missing_factory_is_unavailable_without_retry() {
    let rig = Rig::new(false);
    rig.service.enable();
    let state = rig.service.public_state();
    assert_eq!(state.state, State::Unavailable);
    assert_eq!(state.attempt, 0);
    assert_eq!(rig.timers.len(), 0);
    assert_eq!(
        rig.lines().last().unwrap(),
        "[discord-rpc] RPC_DISCORD_UNAVAILABLE: клиент Discord недоступен в этой сборке"
    );
}

#[tokio::test]
async fn shutdown_clears_then_closes_and_finish_waits() {
    let rig = Rig::new(true);
    rig.service.enable();
    rig.until("клиент", || !rig.clients.lock().unwrap().is_empty())
        .await;
    rig.login_ok(0);
    rig.until("подключено", || rig.service.connected()).await;

    assert!(rig.service.shutdown());
    rig.service.finish().await;
    let calls = rig.client(0).calls();
    assert_eq!(&calls[calls.len() - 2..], ["clearActivity", "destroy"]);
    assert_eq!(*rig.connections.lock().unwrap(), [true, false]);
    assert_eq!(rig.lines().last().unwrap(), "[discord-rpc] RPC_SHUTDOWN");
}

#[tokio::test]
async fn panicking_connection_handler_does_not_break_the_service() {
    let logger = Arc::new(MemoryLogger::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let logins: Logins = Arc::default();
    let sink = logins.clone();
    let create_client: CreateClient = Arc::new(move |_options: &CreateClientOptions| {
        let (sender, receiver) = oneshot::channel();
        sink.lock().unwrap().push(sender);
        Ok(Arc::new(FakeClient {
            calls: Mutex::default(),
            login: Mutex::new(Some(receiver)),
            set_activity_error: Mutex::default(),
            reply_error: Mutex::default(),
            disconnected: Mutex::default(),
        }) as Arc<dyn PresenceClient>)
    });
    let mut options = ServiceOptions::new(logger, Some(create_client));
    options.timers = Some(Arc::new(FakeTimers::default()));
    options.on_connection_change = Some(Arc::new(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
        panic!("потребитель упал");
    }));
    let service = DiscordRichPresenceService::new(options);
    assert!(service.enable());
    for _ in 0..400 {
        if !logins.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    logins.lock().unwrap().remove(0).send(Ok(())).unwrap();
    for _ in 0..400 {
        if service.connected() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // Потребитель упал при `true`, служба подключена и работает дальше.
    assert!(service.connected());
    assert_eq!(service.public_state().state, State::Connected);
    assert!(service.disable());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(!service.connected());
}
