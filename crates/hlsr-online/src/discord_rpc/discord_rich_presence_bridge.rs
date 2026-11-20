//! `discord-rpc/index.js`: мост между лаунчером и службой Discord. Сигналы доставки (`selected`,
//! `status`, `running`, `stopped`) и состояние игры из моста присутствия превращаются в
//! присутствие; настройка «включено» хранится в `discord-rpc.json`; окно спрашивает состояние
//! (`discord-rpc-get`), включает и выключает присутствие (`discord-rpc-set`), сообщает раздел
//! (`discord-rpc-activity`) и получает `discord-rpc-state` после выбора пользователя. Рендер
//! демки («Демо») сообщает приложение ([`DiscordRichPresenceBridge::set_demo_rendering`]).
//!
//! Каналы окна здесь — методы [`DiscordRichPresenceBridge::get`],
//! [`DiscordRichPresenceBridge::set`] и [`DiscordRichPresenceBridge::set_activity`]; событие —
//! обратный вызов [`StateSink`]. Привязку к `OnlineApi` делает приложение. Сигналы доставки мост
//! принимает как [`PresenceSink`].

use super::constants::Code;
use super::diagnostics_model;
use super::discord_rich_presence_service::{
    BoxFuture, ConnectionHandler, CreateClient, CreateClientOptions, DiscordRichPresenceService,
    LocaleProvider, PresenceClient, PublicState, ServiceOptions, Timers, VersionProvider,
};
use super::preference;
use super::presence_model::{self, Activity, Kind, Presence};
use super::rpc::client::{RpcClient, RpcError};
use super::rpc::ipc::IpcConfig;
use super::rpc::register::{ProtocolClient, SystemProtocolClient};
use super::rpc::util::register;
use hlsr_core::js;
use hlsr_delivery::errors::RawError;
use hlsr_delivery::ports::{Logger, PresenceSink};
use hlsr_delivery::timing::{Clock, Random, system_clock, system_random};
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use tokio::runtime::Handle;

/// Получатель события окна `discord-rpc-state` (`window.webContents.send`).
pub type StateSink = Arc<dyn Fn(&Value) + Send + Sync>;

/// Игровой мост присутствия (подпроект 3), каким его видит Discord: ему сообщают, есть ли
/// подключение (`setDiscordConnected`), и берут его снимок для диагностики.
pub trait GamePresenceLink: Send + Sync {
    fn set_discord_connected(&self, connected: bool);
    fn diagnostics(&self) -> Value;
}

/// Клиент, который вместе с входом просит у ОС протокол `discord-<id>`: `register`, затем
/// `new Client({transport: "ipc"})` (`defaultCreateClient`). Реестр или `xdg-settings` могут
/// ждать, поэтому `register` выполняется на потоке блокирующих задач перед входом.
struct RegisteringClient {
    client: RpcClient,
    protocol: Arc<dyn ProtocolClient>,
    application_id: String,
}

impl PresenceClient for RegisteringClient {
    fn on_disconnected(&self, handler: Arc<dyn Fn() + Send + Sync>) {
        self.client.on_disconnected(handler);
    }

    fn login(&self, client_id: &str) -> BoxFuture<Result<(), RpcError>> {
        let protocol = self.protocol.clone();
        let application_id = self.application_id.clone();
        let login = self.client.login(client_id);
        Box::pin(async move {
            // Ошибка реестра не мешает присутствию.
            let _ =
                tokio::task::spawn_blocking(move || register(protocol.as_ref(), &application_id))
                    .await;
            login.await
        })
    }

    fn set_activity(
        &self,
        activity: &Activity,
    ) -> Result<BoxFuture<Result<Value, RpcError>>, RpcError> {
        self.client.set_activity(activity)
    }

    fn clear_activity(&self) -> BoxFuture<Result<Value, RpcError>> {
        self.client.clear_activity()
    }

    fn destroy(&self) -> BoxFuture<()> {
        self.client.destroy()
    }
}

/// `defaultCreateClient`: клиент IPC по адресам `ipc`, регистрация протокола через `protocol`.
pub fn default_create_client(protocol: Arc<dyn ProtocolClient>, ipc: IpcConfig) -> CreateClient {
    Arc::new(move |options: &CreateClientOptions| {
        Ok(Arc::new(RegisteringClient {
            client: RpcClient::new(ipc.clone()),
            protocol: protocol.clone(),
            application_id: options.application_id.clone(),
        }) as Arc<dyn PresenceClient>)
    })
}

/// Откуда служба берёт клиента Discord.
pub enum ClientFactory {
    /// `defaultCreateClient`: настоящий Discord IPC и регистрация протокола в ОС.
    System {
        protocol: Arc<dyn ProtocolClient>,
        ipc: IpcConfig,
    },
    /// Своя фабрика (`opts.createClient`).
    Custom(CreateClient),
    /// Фабрики нет: «клиент Discord недоступен в этой сборке».
    Missing,
}

impl ClientFactory {
    /// Настоящие адреса и регистрация в ОС.
    pub fn system() -> ClientFactory {
        ClientFactory::System {
            protocol: Arc::new(SystemProtocolClient::default()),
            ipc: IpcConfig::system(),
        }
    }
}

/// Параметры моста (`opts` конструктора 0.4.7).
pub struct BridgeOptions {
    /// `userDataPath`: папка с `discord-rpc.json`.
    pub user_data_dir: PathBuf,
    pub log: Arc<dyn Logger>,
    /// `now`: часы; от них начало сессии (`sessionStartedAtMs`) и время записи настройки.
    pub now: Clock,
    pub version_provider: Option<VersionProvider>,
    pub locale_provider: Option<LocaleProvider>,
    pub client: ClientFactory,
    pub timers: Option<Arc<dyn Timers>>,
    pub random: Option<Random>,
    pub runtime: Option<Handle>,
}

impl BridgeOptions {
    /// Умолчания: системные часы, настоящий Discord, язык `ru`, без версии.
    pub fn new(user_data_dir: impl Into<PathBuf>, log: Arc<dyn Logger>) -> BridgeOptions {
        BridgeOptions {
            user_data_dir: user_data_dir.into(),
            log,
            now: system_clock(),
            version_provider: None,
            locale_provider: None,
            client: ClientFactory::system(),
            timers: None,
            random: None,
            runtime: None,
        }
    }
}

#[derive(Default)]
struct BridgeState {
    sink: Option<StateSink>,
    selected_game_id: Option<String>,
    delivery_kind: Option<Kind>,
    game_presence: Option<Arc<dyn GamePresenceLink>>,
    game_state: Option<Value>,
    /// Раздел лаунчера (`ACTIVITY_KINDS`).
    activity: Option<Kind>,
    /// Идёт предпросмотр или экспорт «Демо».
    demo_rendering: bool,
    /// Эпизод Ghosting из сигнала `running` (`normalize` проверяет значение).
    episode: Option<Value>,
}

struct Inner {
    user_data_dir: PathBuf,
    log: Arc<dyn Logger>,
    now: Clock,
    session_started_at_ms: i64,
    service: DiscordRichPresenceService,
    state: Mutex<BridgeState>,
}

fn lock(state: &Mutex<BridgeState>) -> MutexGuard<'_, BridgeState> {
    state.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Мост Discord; копии — один мост.
#[derive(Clone)]
pub struct DiscordRichPresenceBridge {
    inner: Arc<Inner>,
}

impl DiscordRichPresenceBridge {
    /// Вне среды tokio нужен `options.runtime`.
    pub fn new(options: BridgeOptions) -> DiscordRichPresenceBridge {
        let runtime = options.runtime.unwrap_or_else(Handle::current);
        let create_client = match options.client {
            ClientFactory::System { protocol, ipc } => Some(default_create_client(protocol, ipc)),
            ClientFactory::Custom(create_client) => Some(create_client),
            ClientFactory::Missing => None,
        };
        let inner = Arc::new_cyclic(|weak: &Weak<Inner>| {
            let weak = weak.clone();
            let on_connection_change: ConnectionHandler = Arc::new(move |connected| {
                if let Some(inner) = weak.upgrade() {
                    inner.on_discord_connection(connected);
                }
            });
            let service = DiscordRichPresenceService::new(ServiceOptions {
                create_client,
                application_id: None,
                log: options.log.clone(),
                now: options.now.clone(),
                timers: options.timers,
                random: options.random.unwrap_or_else(system_random),
                version_provider: options
                    .version_provider
                    .unwrap_or_else(|| Arc::new(|| None)),
                locale_provider: options
                    .locale_provider
                    .unwrap_or_else(|| Arc::new(|| Some("ru".to_owned()))),
                on_connection_change: Some(on_connection_change),
                runtime: Some(runtime),
            });
            Inner {
                user_data_dir: options.user_data_dir,
                log: options.log,
                session_started_at_ms: (options.now)(),
                now: options.now,
                service,
                state: Mutex::new(BridgeState::default()),
            }
        });
        DiscordRichPresenceBridge { inner }
    }

    /// Служба под мостом.
    pub fn service(&self) -> &DiscordRichPresenceService {
        &self.inner.service
    }

    /// `setWindow(window)`: куда слать `discord-rpc-state`.
    pub fn set_state_sink(&self, sink: Option<StateSink>) {
        lock(&self.inner.state).sink = sink;
    }

    /// `start()`: перенести старую настройку, включить присутствие, если оно включено.
    pub fn start(&self) {
        let inner = &self.inner;
        if let Err(error) = preference::migrate(&inner.user_data_dir, (inner.now)()) {
            inner.log.warn(&format!(
                "[discord-rpc] миграция настройки не удалась: {}",
                error.message
            ));
        }
        if preference::read(&inner.user_data_dir).enabled {
            inner.service.enable();
            inner.publish();
        }
    }

    /// Канал `discord-rpc-get`.
    pub fn get(&self) -> Value {
        self.inner.snapshot()
    }

    /// Канал `discord-rpc-set`: `enabled === true`. Сбой записи настройки — ошибка канала.
    pub fn set(&self, enabled: bool) -> Result<Value, RawError> {
        let inner = &self.inner;
        preference::set(&inner.user_data_dir, enabled, (inner.now)())?;
        if enabled {
            inner.service.enable();
            inner.publish();
        } else {
            inner.service.disable();
        }
        let snapshot = inner.snapshot();
        inner.emit_state(&snapshot);
        Ok(snapshot)
    }

    /// Сигнал доставки (`onSignal`).
    pub fn on_signal(&self, signal: &Value) {
        self.inner.on_signal(signal);
    }

    /// `attachGamePresence(bridge)`: сообщить мосту, есть ли подключение.
    pub fn attach_game_presence(&self, link: Arc<dyn GamePresenceLink>) -> bool {
        link.set_discord_connected(self.inner.service.connected());
        lock(&self.inner.state).game_presence = Some(link);
        true
    }

    /// `setGameState(state)`: состояние игры из моста (`active` ложно — игры нет).
    pub fn set_game_state(&self, state: &Value) {
        let active = state.get("active").is_some_and(js::truthy);
        lock(&self.inner.state).game_state = active.then(|| state.clone());
        self.inner.publish();
    }

    /// Канал `discord-rpc-activity` (`setActivity`): раздел лаунчера; неизвестное значение —
    /// «Игры» (обычные состояния выбора и запуска).
    pub fn set_activity(&self, activity: Option<&Value>) {
        let kind = presence_model::activity_kind(activity);
        {
            let mut state = lock(&self.inner.state);
            if state.activity == kind {
                return;
            }
            state.activity = kind;
        }
        self.inner.publish();
    }

    /// `setDemoRendering(on)`: «Рендерит демку», пока идёт предпросмотр или экспорт «Демо».
    pub fn set_demo_rendering(&self, on: bool) {
        {
            let mut state = lock(&self.inner.state);
            if state.demo_rendering == on {
                return;
            }
            state.demo_rendering = on;
        }
        self.inner.publish();
    }

    /// Модель, которую служба отправит (`service.pendingModel`).
    pub fn pending_presence(&self) -> Option<Presence> {
        self.inner.service.pending_model()
    }

    /// `diagnostics()`: снимок для диагностики.
    pub fn diagnostics(&self) -> Value {
        self.inner.diagnostics()
    }

    /// `shutdown()`.
    pub fn shutdown(&self) {
        self.inner.service.shutdown();
    }

    /// Дождаться кадров закрытия (после [`shutdown`](Self::shutdown)).
    pub async fn finish(&self) {
        self.inner.service.finish().await;
    }
}

impl PresenceSink for DiscordRichPresenceBridge {
    fn signal(&self, signal: &Value) -> Result<(), String> {
        self.on_signal(signal);
        Ok(())
    }
}

/// `String(value)` для не `null` значения; `null` и отсутствие — `None`.
fn game_id_text(value: Option<&Value>) -> Option<String> {
    value.filter(|v| !v.is_null()).map(js::to_string)
}

impl Inner {
    fn snapshot(&self) -> Value {
        let pref = preference::read(&self.user_data_dir);
        let state: PublicState = self.service.public_state();
        json!({
            "enabled": pref.enabled,
            "source": pref.source,
            "state": state.state.as_str(),
            "code": state.code.map(Code::as_str),
            "lastConnectedAt": state.last_connected_at,
        })
    }

    fn emit_state(&self, snapshot: &Value) {
        let sink = lock(&self.state).sink.clone();
        if let Some(sink) = sink {
            sink(snapshot);
        }
    }

    fn on_discord_connection(&self, connected: bool) {
        let link = lock(&self.state).game_presence.clone();
        if let Some(link) = link {
            link.set_discord_connected(connected);
        }
    }

    fn on_signal(&self, signal: &Value) {
        if !signal.is_object() {
            return;
        }
        let game_id = game_id_text(signal.get("gameId"));
        {
            let mut state = lock(&self.state);
            let playing = state.delivery_kind == Some(Kind::Playing);
            match signal.get("type").and_then(Value::as_str) {
                Some("selected") => {
                    state.selected_game_id = game_id;
                    if !playing {
                        state.delivery_kind = Some(Kind::Browsing);
                    }
                }
                Some("status") => {
                    if playing {
                        return;
                    }
                    // Статус чужой игры (проверка обновлений в фоне) не меняет выбранную: иначе
                    // при выбранной HL2 Ghosting в Discord была «Half-Life 2 (4260)».
                    let selected = state
                        .selected_game_id
                        .clone()
                        .filter(|id| !id.is_empty())
                        .map(Value::String);
                    if game_id.is_some()
                        && selected.is_some()
                        && !presence_model::same_game(signal.get("gameId"), selected.as_ref())
                    {
                        return;
                    }
                    if game_id.is_some() {
                        state.selected_game_id = game_id;
                    }
                    state.delivery_kind = Some(presence_model::kind_from_delivery(signal));
                }
                Some("running") => {
                    if game_id.is_some() {
                        state.selected_game_id = game_id;
                    }
                    state.delivery_kind = Some(Kind::Playing);
                    state.episode = signal
                        .get("episode")
                        .filter(|episode| js::truthy(episode))
                        .cloned();
                }
                Some("stopped") => {
                    state.delivery_kind = Some(
                        if state
                            .selected_game_id
                            .as_deref()
                            .is_some_and(|id| !id.is_empty())
                        {
                            Kind::Ready
                        } else {
                            Kind::Launcher
                        },
                    );
                    state.game_state = None;
                    state.episode = None;
                }
                _ => return,
            }
        }
        self.publish();
    }

    /// `publish()`: присутствие по сигналам доставки или, если игра идёт, по её состоянию. Игра,
    /// установка, обновление и проверка файлов важнее всего; рендер демки — важнее раздела.
    fn publish(&self) {
        let (kind, selected, game, episode) = {
            let state = lock(&self.state);
            let delivery = state.delivery_kind.unwrap_or(Kind::Launcher);
            let busy = matches!(
                delivery,
                Kind::Playing | Kind::Installing | Kind::Updating | Kind::Repairing
            );
            let kind = match (busy, state.demo_rendering, state.activity) {
                (false, true, _) => Kind::DemoRender,
                (false, false, Some(activity)) => activity,
                _ => delivery,
            };
            (
                kind,
                state.selected_game_id.clone(),
                state.game_state.clone(),
                state.episode.clone(),
            )
        };
        let mut presence = Map::new();
        let game = game.filter(|_| selected.as_deref().is_some_and(|id| !id.is_empty()));
        if let Some(game) = &game {
            let kind = if game.get("lifecycle").and_then(Value::as_str) == Some("playing") {
                Kind::GameMap
            } else {
                Kind::GameMenu
            };
            presence.insert("kind".into(), json!(kind.as_str()));
        } else {
            presence.insert("kind".into(), json!(kind.as_str()));
        }
        presence.insert("gameId".into(), json!(selected));
        presence.insert("episode".into(), episode.unwrap_or(Value::Null));
        presence.insert(
            "sessionStartedAtMs".into(),
            json!(self.session_started_at_ms),
        );
        if let Some(game) = &game {
            for key in ["map", "fps", "running", "runStartedAtMs", "finalTime"] {
                if let Some(value) = game.get(key) {
                    presence.insert(key.into(), value.clone());
                }
            }
        }
        self.service.set_presence(&Value::Object(presence));
    }

    fn diagnostics(&self) -> Value {
        let snapshot = self.snapshot();
        let link = lock(&self.state).game_presence.clone();
        let game_bridge = link.map_or(Value::Null, |link| link.diagnostics());
        diagnostics_model::normalize(&json!({
            "enabled": snapshot["enabled"],
            "state": snapshot["state"],
            "lastConnectedAt": snapshot["lastConnectedAt"],
            "lastCode": snapshot["code"],
            "gameBridge": game_bridge,
        }))
        .to_json()
    }
}
