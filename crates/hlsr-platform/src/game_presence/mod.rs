//! `game-presence/`: мост между лаунчером и игрой. Игра шлёт состояние (этап, карта, FPS, таймер
//! забега) по локальному каналу; мост сводит его к состоянию для Discord и отдаёт снимок
//! обработчику присутствия (`index.js`: `GamePresenceBridge`).
//!
//! Чат в игре (`attachChatConsumer`): 7TV, эмоуты и значки Twitch, YouTube (`hlsr-online`) получают
//! сообщения `chat.*` и `emote.*`, рукопожатие игры и её уход, а пишут игре через сервер моста.
//!
//! Связь с Discord (`AppManager.js`): снимок состояния игры уходит в `on_presence` — приложение
//! передаёт туда `DiscordRichPresenceBridge::set_game_state` (`hlsr-online`), а сам Discord
//! сообщает мосту, подключён ли он, через [`GamePresenceBridge::set_discord_connected`]
//! (в `hlsr-online` это трейт `GamePresenceLink`). Сигналы доставки (`PresenceSink`) принимает
//! тот же `DiscordRichPresenceBridge`, в этом крейте им делать нечего.

pub mod endpoint_store;
pub mod game_state;
pub mod presence_bridge_server;
pub mod protocol;

use game_state::GameState;
use hlsr_core::logger::{Logger, TracingLogger};
use hlsr_delivery::ports::{ChatBridge, ChatConsumer, ChatMessage};
use hlsr_delivery::timing::{Clock, system_clock};
use presence_bridge_server::{Handlers, Limits, PresenceBridgeServer, ServerOptions};
use protocol::StateMessage;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

/// Куда мост отдаёт снимок состояния игры (`onPresence`): для Discord —
/// `DiscordRichPresenceBridge::set_game_state`.
pub type PresenceHandler = Arc<dyn Fn(&Value) + Send + Sync>;

/// Параметры моста (`options`).
#[derive(Default)]
pub struct BridgeOptions {
    pub log: Option<Arc<dyn Logger>>,
    /// Часы (`now`), мс.
    pub now: Option<Clock>,
    pub on_presence: Option<PresenceHandler>,
    /// Канал прошлого запуска (`presence-endpoint.json`): токен, имя канала и порт.
    pub token: Option<String>,
    pub pipe_name: Option<String>,
    pub port: Option<u16>,
    pub limits: Option<Limits>,
    pub platform: Option<hlsr_core::js::Platform>,
    /// Среда tokio; по умолчанию — текущая.
    pub runtime: Option<tokio::runtime::Handle>,
}

struct Inner {
    state: GameState,
    last_fps_publish_at_ms: Option<i64>,
}

/// `GamePresenceBridge`: сервер игры + состояние для Discord.
pub struct GamePresenceBridge {
    log: Arc<dyn Logger>,
    now: Clock,
    on_presence: PresenceHandler,
    server: PresenceBridgeServer,
    inner: Mutex<Inner>,
    chat_consumers: Mutex<Vec<Arc<dyn ChatConsumer>>>,
}

impl GamePresenceBridge {
    pub fn new(options: BridgeOptions) -> Arc<GamePresenceBridge> {
        let log: Arc<dyn Logger> = options.log.unwrap_or_else(|| Arc::new(TracingLogger));
        let now = options.now.unwrap_or_else(system_clock);
        Arc::new_cyclic(|weak: &Weak<GamePresenceBridge>| {
            let on_state = {
                let weak = weak.clone();
                Arc::new(move |message: &StateMessage| {
                    if let Some(bridge) = weak.upgrade() {
                        bridge.on_message(Some(message));
                    }
                })
            };
            let on_game_gone = {
                let weak = weak.clone();
                Arc::new(move |_reason: &str| {
                    if let Some(bridge) = weak.upgrade() {
                        bridge.on_game_gone();
                    }
                })
            };
            let on_chat = {
                let weak = weak.clone();
                Arc::new(move |message: &ChatMessage| {
                    if let Some(bridge) = weak.upgrade() {
                        bridge.on_chat_message(message);
                    }
                })
            };
            let on_game_ready = {
                let weak = weak.clone();
                Arc::new(move || {
                    if let Some(bridge) = weak.upgrade() {
                        bridge.on_game_ready();
                    }
                })
            };
            let server = PresenceBridgeServer::new(ServerOptions {
                log: Some(log.clone()),
                now: Some(now.clone()),
                platform: options.platform,
                token: options.token,
                pipe_name: options.pipe_name,
                port: options.port,
                limits: options.limits,
                handlers: Some(Handlers {
                    on_state: Some(on_state),
                    on_game_gone: Some(on_game_gone),
                    on_game_ready: Some(on_game_ready),
                    on_chat: Some(on_chat),
                }),
                runtime: options.runtime,
            });
            GamePresenceBridge {
                log,
                now,
                on_presence: options.on_presence.unwrap_or_else(|| Arc::new(|_| {})),
                server,
                inner: Mutex::new(Inner {
                    state: GameState::empty(),
                    last_fps_publish_at_ms: None,
                }),
                chat_consumers: Mutex::new(Vec::new()),
            }
        })
    }

    fn consumers(&self) -> Vec<Arc<dyn ChatConsumer>> {
        self.chat_consumers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Вызвать каждого потребителя чата; падение одного не мешает остальным.
    fn each_consumer(&self, what: &str, call: impl Fn(&dyn ChatConsumer)) {
        for consumer in self.consumers() {
            if let Err(error) = hlsr_core::tasks::catch_panic(|| call(consumer.as_ref())) {
                self.log.warn(&format!("[game-presence] {what}: {error}"));
            }
        }
    }

    /// `attachChatConsumer(consumer)`: потребитель чата получает сторону моста для записи игре.
    pub fn attach_chat_consumer(&self, consumer: Arc<dyn ChatConsumer>) {
        self.chat_consumers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(consumer.clone());
        let bridge: Arc<dyn ChatBridge> = Arc::new(self.server.clone());
        consumer.attach_bridge(bridge);
    }

    /// `onChatMessage(message)`.
    pub fn on_chat_message(&self, message: &ChatMessage) {
        self.each_consumer("потребитель чата упал", |consumer| {
            consumer.handle_message(message)
        });
    }

    /// `onGameReady()`: игра представилась.
    pub fn on_game_ready(&self) {
        self.each_consumer(
            "потребитель чата не смог поздороваться",
            |consumer| consumer.handle_ready(),
        );
    }

    /// `forgetConsumers()`: игра ушла — потребители забывают подписки и эмоуты.
    fn forget_consumers(&self) {
        self.each_consumer(
            "потребитель чата не смог очиститься",
            |consumer| consumer.forget(),
        );
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Сервер игры (канал, окружение, диагностика связи).
    pub fn server(&self) -> &PresenceBridgeServer {
        &self.server
    }

    /// `start()`: имя канала.
    pub fn start(&self) -> String {
        self.server.start()
    }

    /// `shutdown()`: закрыть связь, перестать слушать, забыть состояние.
    pub fn shutdown(&self) {
        self.forget_consumers();
        self.server.stop();
        let mut inner = self.inner();
        inner.state = GameState::empty();
        inner.last_fps_publish_at_ms = None;
    }

    /// `environment()`: переменные окружения игры.
    pub fn environment(&self) -> Vec<(String, String)> {
        self.server.environment()
    }

    /// `endpoint()`: канал для `presence-endpoint.json`.
    pub fn endpoint(&self) -> endpoint_store::Endpoint {
        self.server.endpoint()
    }

    /// `onMessage(message)`: сведения от игры. Истина — состояние для Discord изменилось и
    /// отдано обработчику. `None` — сообщение не `state`.
    pub fn on_message(&self, message: Option<&StateMessage>) -> bool {
        let at = (self.now)();
        let snapshot = {
            let mut inner = self.inner();
            let result = game_state::apply(&inner.state, message, at);
            if result.ignored.is_some() {
                return false;
            }
            let previous = std::mem::replace(&mut inner.state, result.state);
            if !result.changed {
                return false;
            }
            if !game_state::should_publish(
                Some(&previous),
                &inner.state,
                inner.last_fps_publish_at_ms,
                at,
            ) {
                return false;
            }
            inner.last_fps_publish_at_ms = if game_state::same_except_fps(&previous, &inner.state) {
                Some(at)
            } else {
                None
            };
            inner.state.snapshot()
        };
        self.emit(&snapshot);
        true
    }

    /// `onGameGone()`: игра ушла. Истина — состояние было активным и сброшено.
    pub fn on_game_gone(&self) -> bool {
        self.forget_consumers();
        let snapshot = {
            let mut inner = self.inner();
            if !inner.state.active {
                return false;
            }
            inner.state = GameState::empty();
            inner.last_fps_publish_at_ms = None;
            inner.state.snapshot()
        };
        self.emit(&snapshot);
        true
    }

    fn emit(&self, snapshot: &Value) {
        if let Err(error) = hlsr_core::tasks::catch_panic(|| (self.on_presence)(snapshot)) {
            self.log.warn(&format!(
                "[game-presence] потребитель состояния упал: {error}"
            ));
        }
    }

    /// `snapshot()`: что видит Discord.
    pub fn snapshot(&self) -> Value {
        self.inner().state.snapshot()
    }

    /// Внутреннее состояние (для проверок).
    pub fn state(&self) -> GameState {
        self.inner().state.clone()
    }

    /// `lastFpsPublishAtMs` (для проверок).
    pub fn last_fps_publish_at_ms(&self) -> Option<i64> {
        self.inner().last_fps_publish_at_ms
    }

    /// `setDiscordConnected(connected)`: сообщить игре, подключён ли Discord.
    pub fn set_discord_connected(&self, connected: bool) -> bool {
        self.server.set_discord_connected(connected)
    }

    /// `diagnostics()`: `{listening, connected, rejected, lifecycle, hasMap}`.
    pub fn diagnostics(&self) -> Value {
        let bridge = self.server.diagnostics();
        let state = self.inner();
        json!({
            "listening": bridge.listening,
            "connected": bridge.connected,
            "rejected": bridge.stats.rejected,
            "lifecycle": state.state.lifecycle.map(protocol::Lifecycle::as_str),
            "hasMap": state.state.map.is_some(),
        })
    }
}
