//! Фасад онлайна (онлайн-части `AppManager.js`, каналы `online-profile/ipc.js`,
//! `telemetry/news-ipc.js`, `telemetry/sponsors-ipc.js`, `discord-rpc/index.js`, каналы уведомлений
//! и публичных данных): по
//! методу на канал IPC окна (карта §4), события окна — [`OnlineEvent`] через `broadcast`.
//!
//! Службы создаются в порядке конструктора `AppManager` ([`OnlineApi::new`]: уведомления, события
//! версий, телеметрия, онлайн, лента, спонсоры, события обновления лаунчера, профиль, Discord);
//! отпечаток сборки и аттестация — перед онлайном: публичные запросы идут с ними, как профиль
//! (`security::official`). Дальше — как в 0.4.7: [`OnlineApi::start_discord`] (конец
//! конструктора), [`OnlineApi::initialize`] (согласие, телеметрия, опрос онлайна и профиля),
//! [`OnlineApi::window_loaded`] (`noteActiveDay` после загрузки окна и `ready` окна),
//! [`OnlineApi::window_shown`] (`show`/`restore` окна — опрос онлайна). Остановка (`dispose`):
//! [`OnlineApi::stop_discord`] первым, [`OnlineApi::shutdown`] (телеметрия, онлайн, профиль) после
//! доставки и обновлялки, [`OnlineApi::finish`] дожидается `session/end` и кадров закрытия Discord.
//!
//! Другим подпроектам онлайн отдаёт порты ([`OnlineApi::ports`]): уведомления и события версий
//! игр (`Notifier`), телеметрию (`Telemetry`), достижения (`Achievements`), присутствие Discord
//! (`PresenceSink`); события процесса игры — [`OnlineApi::game_running`] и соседи.
//!
//! Аргументы каналов — значения JSON, как их присылает окно (`None` — `undefined`); ответы — те же
//! объекты, что у 0.4.7. Отказ канала (отклонённый промис IPC) — `Err` с текстом исключения.
//! Каналы можно звать из любого потока: работа идёт в среде tokio фасада.

mod ports;
mod session;

pub use ports::{
    GuardedProtocolClient, OnlineNotifier, discord_register_allowed, system_discord_client,
};
pub use session::migrate_session;

pub use hlsr_delivery::ports::{Logger, TracingLogger};
/// Будущее каналов и портов (`hlsr_delivery::providers::BoxFuture`).
pub use hlsr_delivery::providers::BoxFuture;

use crate::discord_rpc::discord_rich_presence_bridge::{
    BridgeOptions, ClientFactory, DiscordRichPresenceBridge,
};
use crate::notifications::version_events::VersionEvents;
use crate::online_profile::ipc::ProfileIpc;
use crate::online_profile::online_profile_service::{
    AccountProvider, FriendProvider, OnlineProfileOptions, OnlineProfileService, OpenExternal,
    RunnerError, Timers, TokioTimers, build_header_from, notify_client_modified, notify_moderation,
    notify_support_thanks, ticket_provider_from,
};
use crate::online_profile::profile_state::LocalAccount;
use crate::online_profile::session_store::{SESSION_FILE, SessionStore, detect_vault};
use crate::security::attestation::{AttestationOptions, BuildAttestation};
use crate::security::build_identity::BuildIdentity;
use crate::security::build_key::{AttestKey, BuildKey};
use crate::security::official::{AttestedTransport, Official, Standing};
use crate::telemetry::config;
use crate::telemetry::launcher_update_events::{
    AnnouncedVersions, LauncherUpdateEvents, TelemetrySink,
};
use crate::telemetry::news_client::{NewsClient, NewsClientOptions};
use crate::telemetry::news_ipc::{NEWS_FAILED, NewsIpc};
use crate::telemetry::online_client::{OnlineClient, OnlineClientOptions};
use crate::telemetry::sponsors_client::{SponsorsClient, SponsorsClientOptions};
use crate::telemetry::sponsors_ipc::SponsorsIpc;
use crate::telemetry::style_presets_client::{StylePresetsClient, StylePresetsClientOptions};
use crate::telemetry::telemetry_service::{TelemetryOptions, TelemetryService};
use crate::telemetry::transport::{HttpTransport, Transport};
use hlsr_core::js;
use hlsr_core::language::Language;
use hlsr_core::notifications::{FILE_NAME as NOTIFICATIONS_FILE, NotificationStore};
use hlsr_core::paths::Paths;
use hlsr_core::settings::Settings;
use hlsr_delivery::ports::{Achievements, Notifier, PresenceSink, Telemetry};
use hlsr_delivery::timing::{Clock, system_clock};
use serde_json::{Value, json};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, RwLock, Weak};
use std::task::{Context, Poll};
use tokio::runtime::Handle;
use tokio::sync::broadcast;

/// Каналы событий окна.
pub mod channels {
    pub const RUNNER_PROFILE_STATE: &str = "runner-profile-state";
    pub const RUNNER_LOGIN_SUCCESS: &str = "runner-login-success";
    pub const ACHIEVEMENT_UNLOCKED: &str = "achievement-unlocked";
    pub const PUBLIC_ONLINE: &str = "public-online";
    pub const NOTIFICATIONS_CHANGED: &str = "notifications-changed";
    pub const DISCORD_RPC_STATE: &str = "discord-rpc-state";
    /// «Спасибо за поддержку»: окно получает его после `support-thanks-ready` (`ChannelRouter`).
    pub const SUPPORT_THANKS: &str = "support-thanks";
    /// Сборка для окна сменилась (`Standing::as_str`); не канал 0.4.9.
    pub const BUILD_OFFICIAL: &str = "build-official";
}

/// `version-state.json`: события версий.
pub const VERSION_STATE_FILE: &str = "version-state.json";
/// Сколько событий окна ждёт медленного получателя.
const EVENT_CAPACITY: usize = 1024;

/// Событие окна: канал и данные, как `webContents.send(channel, payload)`.
#[derive(Debug, Clone, PartialEq)]
pub struct OnlineEvent {
    pub channel: &'static str,
    pub payload: Value,
}

/// Диалог подтверждения ссылки новости (`dialog.showMessageBox` в `AppManager.js:295-301`):
/// вопрос, кнопки «Отмена» и «Открыть», по умолчанию и при закрытии — «Отмена».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkDialog {
    pub title: String,
    /// `#L_HOME_LINK_TEXT` и хост с новой строки.
    pub message: String,
    /// Полный адрес.
    pub detail: String,
    pub cancel: String,
    pub open: String,
}

/// Главное окно, каким его видит онлайн (`this.window` в `AppManager.js`).
pub trait OnlineWindow: Send + Sync {
    /// `windowVisible()`: окно видно и не свёрнуто.
    fn visible(&self) -> bool;
    /// `onAuthenticated`: развернуть, показать и сфокусировать окно.
    fn reveal(&self);
    /// Диалог ссылки новости; `true` — «Открыть».
    fn confirm_link(&self, dialog: LinkDialog) -> BoxFuture<'static, bool>;
}

/// Параметры фасада: то, что `AppManager` берёт у Electron (`app.getPath`, `app.getVersion`,
/// `app.isPackaged`, `shell`, окно), и подмены для тестов.
pub struct OnlineOptions {
    /// `app.getPath("userData")`.
    pub user_data_dir: PathBuf,
    /// `app.getVersion()`.
    pub launcher_version: String,
    pub logger: Arc<dyn Logger>,
    pub clock: Clock,
    /// Транспорт API (один на все службы, как `transport.request` 0.4.7).
    pub transport: Arc<dyn Transport>,
    /// Адрес API вместо `https://api.hlsr.pro` для всех служб (`qa.apiBase` у `local-qa.js`).
    pub api_base: Option<String>,
    /// Хранилище входа (задача 8).
    pub session: SessionStore,
    pub window: Arc<dyn OnlineWindow>,
    /// `shell.openExternal(url)`.
    pub open_external: OpenExternal,
    /// Клиент Discord: настоящий IPC ([`system_discord_client`]) или подмена.
    pub discord: ClientFactory,
    /// `path.dirname(app.getPath("exe"))`: маркер установщика для отчёта об установке.
    pub install_dir: Option<PathBuf>,
    /// `app.isPackaged`: сборка выпуска (у разработки вместо хеша сборки — `dev`).
    pub packaged: bool,
    /// `BuildKey.embedded()`.
    pub attest_key: Arc<dyn AttestKey>,
    /// Стенд (`dev-hooks`, тесты): считать сборку официальной без ключа ([`Official`]).
    pub pretend_official: bool,
    /// Таймеры службы профиля (опрос входа, синхронизация раз в 30 с).
    pub profile_timers: Arc<dyn Timers>,
    /// `accountProvider`: свой аккаунт Steam (подпроект 3 или QA); нет — Steam не найден.
    pub account_provider: Option<AccountProvider>,
    /// `friendProvider`: друзья из локального Steam.
    pub friend_provider: Option<FriendProvider>,
}

impl OnlineOptions {
    /// Параметры запущенного лаунчера: журнал `main.log`, часы ОС, сеть, хранилище входа ОС,
    /// настоящий Discord, папка исполняемого файла, ключ аттестации из сборки.
    pub fn new(
        user_data_dir: impl Into<PathBuf>,
        launcher_version: impl Into<String>,
        window: Arc<dyn OnlineWindow>,
        open_external: OpenExternal,
    ) -> OnlineOptions {
        let user_data_dir = user_data_dir.into();
        OnlineOptions {
            session: SessionStore::at(user_data_dir.join(SESSION_FILE), detect_vault()),
            user_data_dir,
            launcher_version: launcher_version.into(),
            logger: Arc::new(TracingLogger),
            clock: system_clock(),
            transport: Arc::new(HttpTransport::new()),
            api_base: None,
            window,
            open_external,
            discord: system_discord_client(),
            install_dir: std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(PathBuf::from)),
            packaged: cfg!(not(any(debug_assertions, feature = "dev-hooks"))),
            attest_key: Arc::new(BuildKey::embedded()),
            pretend_official: false,
            profile_timers: Arc::new(TokioTimers),
            account_provider: None,
            friend_provider: None,
        }
    }
}

/// Порты онлайна для доставки и платформы (`notifications`/`versionEvents`, `telemetry`,
/// `onlineProfiles`, `discordRpc.attachDelivery`).
#[derive(Clone)]
pub struct OnlinePorts {
    pub notifier: Arc<dyn Notifier>,
    pub telemetry: Arc<dyn Telemetry>,
    pub achievements: Arc<dyn Achievements>,
    pub presence: Arc<dyn PresenceSink>,
}

/// Поставщики Steam для профиля: их можно задать после создания (связывание подпроекта 3).
#[derive(Default)]
struct SteamProviders {
    account: Option<AccountProvider>,
    friends: Option<FriendProvider>,
}

/// Фасад онлайна.
pub struct OnlineApi {
    user_data_dir: PathBuf,
    launcher_version: String,
    logger: Arc<dyn Logger>,
    runtime: Handle,
    events: broadcast::Sender<OnlineEvent>,
    steam: RwLock<SteamProviders>,
    notifications: Arc<NotificationStore>,
    versions: Arc<VersionEvents>,
    telemetry: TelemetryService,
    online: Arc<OnlineClient>,
    news: Arc<NewsClient>,
    sponsors: Arc<SponsorsClient>,
    style_presets: StylePresetsClient,
    launcher_updates: LauncherUpdateEvents,
    profiles: OnlineProfileService,
    profile_ipc: ProfileIpc,
    news_ipc: NewsIpc,
    sponsors_ipc: SponsorsIpc,
    discord: DiscordRichPresenceBridge,
    official: Official,
}

/// Получатель событий окна ([`OnlineApi::subscribe`]).
///
/// Очередь событий ограничена: окно, которое долго не читало, теряет самые старые (`broadcast`
/// сообщает об этом ошибкой `Lagged`). Читатель не должен из-за пропуска выходить из цикла и
/// переставать получать события, поэтому [`OnlineEvents::recv`] и [`OnlineEvents::try_recv`]
/// пропуск переживают: пишут строку в журнал и продолжают с самого старого уцелевшего события.
/// Почти все события — снимки состояния (`runner-profile-state`, `public-online`,
/// `discord-rpc-state`, `notifications-changed`), новый снимок заменяет потерянный; одиночные
/// (`achievement-unlocked`, `runner-login-success`) при отставании на 1024 события теряются.
pub struct OnlineEvents {
    receiver: broadcast::Receiver<OnlineEvent>,
    logger: Arc<dyn Logger>,
}

impl OnlineEvents {
    fn new(receiver: broadcast::Receiver<OnlineEvent>, logger: Arc<dyn Logger>) -> OnlineEvents {
        OnlineEvents { receiver, logger }
    }

    fn lagged(&self, skipped: u64) {
        self.logger.warn(&format!(
            "[online] окно отстало от событий, пропущено: {skipped}"
        ));
    }

    /// Следующее событие; `None` — фасад удалён, событий больше не будет.
    pub async fn recv(&mut self) -> Option<OnlineEvent> {
        loop {
            match self.receiver.recv().await {
                Ok(event) => return Some(event),
                Err(broadcast::error::RecvError::Lagged(skipped)) => self.lagged(skipped),
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// Следующее событие, если оно уже есть; `None` — очередь пуста (или фасад удалён).
    pub fn try_recv(&mut self) -> Option<OnlineEvent> {
        loop {
            match self.receiver.try_recv() {
                Ok(event) => return Some(event),
                Err(broadcast::error::TryRecvError::Lagged(skipped)) => self.lagged(skipped),
                Err(_) => return None,
            }
        }
    }
}

/// Отправить событие окна; нет получателей — окна нет, событие никому не нужно.
fn send(events: &broadcast::Sender<OnlineEvent>, channel: &'static str, payload: Value) {
    let _ = events.send(OnlineEvent { channel, payload });
}

/// `currentLocale()`: `config.language === 1` — `ru`, иначе `en` (`settings.json` читается
/// заново при каждом вызове, как `new Store` 0.4.7).
pub fn current_locale(user_data_dir: &Path) -> &'static str {
    let paths = Paths {
        data_dir: user_data_dir.to_path_buf(),
        logs_dir: user_data_dir.to_path_buf(),
        app_name: hlsr_core::paths::APP_NAME,
    };
    match Settings::open(&paths).language() {
        Some(Language::Ru) => "ru",
        _ => "en",
    }
}

/// `{force: options && options.force === true}`.
fn force_of(options: Option<&Value>) -> bool {
    options.and_then(|options| options.get("force")) == Some(&Value::Bool(true))
}

/// Ошибка канала ленты без кода (`error.code || "NEWS_FAILED"`).
fn news_failed() -> Value {
    json!({"status": "error", "code": NEWS_FAILED})
}

/// Будущее, которое опрашивается в среде tokio фасада: окно может ждать канал в своём потоке, а
/// сеть, таймеры и фоновые задачи служб требуют среды.
struct InRuntime<'a, T> {
    runtime: &'a Handle,
    inner: BoxFuture<'a, T>,
}

impl<T> Future for InRuntime<'_, T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let this = self.get_mut();
        let _runtime = this.runtime.enter();
        this.inner.as_mut().poll(cx)
    }
}

impl OnlineApi {
    /// Службы онлайна в порядке конструктора `AppManager` (`AppManager.js:163-303`, `:305-313`).
    /// Вызывать в среде tokio (многопоточной): её берут службы, таймеры и Discord.
    pub fn new(options: OnlineOptions) -> Arc<OnlineApi> {
        let OnlineOptions {
            user_data_dir: dir,
            launcher_version: version,
            logger,
            clock,
            transport,
            api_base,
            session,
            window,
            open_external,
            discord,
            install_dir,
            packaged,
            attest_key,
            pretend_official,
            profile_timers,
            account_provider,
            friend_provider,
        } = options;
        let runtime = Handle::current();
        let _runtime = runtime.enter();
        let qa = api_base.as_deref().filter(|base| !base.is_empty());
        let base = qa.unwrap_or(config::API_BASE).to_owned();
        let endpoint = |path: &str| qa.map(|base| format!("{base}{path}"));
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let visible: Arc<dyn Fn() -> bool + Send + Sync> = {
            let window = window.clone();
            Arc::new(move || window.visible())
        };
        let locale: Arc<dyn Fn() -> Option<String> + Send + Sync> = {
            let dir = dir.clone();
            Arc::new(move || Some(current_locale(&dir).to_owned()))
        };

        Arc::new_cyclic(|weak: &Weak<OnlineApi>| {
            // `NotificationStore` → `notifications-changed` (`AppManager.js:163-170`).
            let notifications = Arc::new(NotificationStore::with(
                dir.join(NOTIFICATIONS_FILE),
                logger.clone(),
                clock.clone(),
            ));
            {
                let events = events.clone();
                notifications.set_on_change(Some(Box::new(move |summary| {
                    send(
                        &events,
                        channels::NOTIFICATIONS_CHANGED,
                        json!({"unread": summary.unread, "total": summary.total}),
                    );
                })));
            }
            let versions = Arc::new(VersionEvents::new(
                dir.join(VERSION_STATE_FILE),
                notifications.clone(),
                logger.clone(),
                clock.clone(),
            ));

            // `TelemetryService`: токен — у профиля, который создаётся позже (`AppManager.js:180`).
            let mut telemetry_options =
                TelemetryOptions::new(&dir, version.clone(), logger.clone(), transport.clone());
            telemetry_options.endpoint = endpoint("/v1/telemetry/batch");
            telemetry_options.authorization = Some({
                let weak = weak.clone();
                Arc::new(move || weak.upgrade()?.profiles.get_auth_token())
            });
            telemetry_options.clock = clock.clone();
            telemetry_options.locale = Some(locale.clone());
            telemetry_options.window_visible = Some(visible.clone());
            telemetry_options.install_dir = install_dir;
            let telemetry = TelemetryService::new(telemetry_options);

            // Хеш исполняемого файла считается сразу: лаунчер может обновиться, пока открыт.
            let identity = BuildIdentity::new(version.clone(), packaged);
            identity.warm();
            let official = Official::new(
                attest_key.available(),
                pretend_official,
                Some({
                    let events = events.clone();
                    Arc::new(move |standing: Standing| {
                        send(&events, channels::BUILD_OFFICIAL, json!(standing.as_str()));
                    })
                }),
            );
            let attestation = BuildAttestation::new(AttestationOptions {
                identity: Some(identity.clone()),
                key: Some(attest_key),
                transport: transport.clone(),
                api_base: base.clone(),
                launcher_version: version.clone(),
                clock: clock.clone(),
                logger: logger.clone(),
            });
            // Публичные данные — с отпечатком сборки и билетом профиля.
            let public: Arc<dyn Transport> = Arc::new(AttestedTransport::new(
                transport.clone(),
                build_header_from(identity.clone()),
                ticket_provider_from(attestation.clone()),
                official.clone(),
            ));

            let mut online_options = OnlineClientOptions::new(clock.clone(), public.clone());
            online_options.endpoint = endpoint("/v1/public/online");
            online_options.launcher_version = Some(version.clone());
            // Самосборка без ключа счётчики не спрашивает: сборка всё равно не официальная.
            online_options.backend_configured =
                Some(config::BACKEND_CONFIGURED && official.possible());
            online_options.is_window_visible = Some(visible);
            online_options.on_update = Some({
                let events = events.clone();
                Arc::new(move |snapshot| {
                    send(&events, channels::PUBLIC_ONLINE, snapshot.to_json());
                })
            });
            let online = Arc::new(OnlineClient::new(online_options));
            let news = Arc::new(NewsClient::new(NewsClientOptions {
                endpoint: endpoint("/v1/public/news"),
                launcher_version: Some(version.clone()),
                clock: clock.clone(),
                transport: public.clone(),
            }));
            let sponsors = Arc::new(SponsorsClient::new(SponsorsClientOptions {
                endpoint: endpoint("/v2/public/sponsors"),
                legacy_endpoint: endpoint("/v1/public/sponsors"),
                launcher_version: Some(version.clone()),
                clock: clock.clone(),
                transport: public.clone(),
            }));
            let style_presets = StylePresetsClient::new(StylePresetsClientOptions {
                endpoint: endpoint("/v2/public/style-presets"),
                launcher_version: Some(version.clone()),
                clock: clock.clone(),
                transport: public,
            });
            let launcher_updates = LauncherUpdateEvents::new(
                Some(Arc::new(telemetry.clone()) as Arc<dyn TelemetrySink>),
                Some(versions.clone() as Arc<dyn AnnouncedVersions>),
                logger.clone(),
            );

            let mut profile = OnlineProfileOptions::new(&dir, session, transport);
            profile.api_base = Some(base);
            profile.launcher_version = Some(version.clone());
            profile.clock = clock.clone();
            profile.timers = profile_timers;
            profile.open_external = Some(open_external.clone());
            profile.build_header = Some(build_header_from(identity));
            profile.ticket_provider = Some(ticket_provider_from(attestation));
            Self::bind_profile(
                &mut profile,
                weak,
                &events,
                &notifications,
                &telemetry,
                &window,
                &logger,
            );
            let profiles = OnlineProfileService::new(profile);
            let profile_ipc = ProfileIpc::new(profiles.clone());
            // Ссылки спонсоров спрашиваются тем же диалогом, что ссылки новостей.
            let links = Arc::new(ports::NewsLinksLink {
                window,
                open_external,
                user_data_dir: dir.clone(),
            });
            let news_ipc = NewsIpc::new(
                news.clone(),
                Arc::new(ports::NewsProfileLink(profiles.clone())),
                links.clone(),
            );
            let sponsors_ipc = SponsorsIpc::new(sponsors.clone(), links);

            // `DiscordRichPresenceBridge` и `setWindow` (`AppManager.js:305-312`).
            let mut bridge = BridgeOptions::new(&dir, logger.clone());
            bridge.now = clock;
            bridge.version_provider = Some({
                let version = version.clone();
                Arc::new(move || Some(version.clone()))
            });
            bridge.locale_provider = Some(locale);
            bridge.client = discord;
            bridge.runtime = Some(runtime.clone());
            let discord = DiscordRichPresenceBridge::new(bridge);
            discord.set_state_sink(Some({
                let events = events.clone();
                Arc::new(move |snapshot: &Value| {
                    send(&events, channels::DISCORD_RPC_STATE, snapshot.clone());
                })
            }));

            OnlineApi {
                user_data_dir: dir,
                launcher_version: version,
                logger,
                runtime: runtime.clone(),
                events,
                steam: RwLock::new(SteamProviders {
                    account: account_provider,
                    friends: friend_provider,
                }),
                notifications,
                versions,
                telemetry,
                online,
                news,
                sponsors,
                style_presets,
                launcher_updates,
                profiles,
                profile_ipc,
                news_ipc,
                sponsors_ipc,
                discord,
                official,
            }
        })
    }

    /// Колбэки службы профиля (`AppManager.js:240-285`) и поставщики Steam.
    fn bind_profile(
        profile: &mut OnlineProfileOptions,
        weak: &Weak<OnlineApi>,
        events: &broadcast::Sender<OnlineEvent>,
        notifications: &Arc<NotificationStore>,
        telemetry: &TelemetryService,
        window: &Arc<dyn OnlineWindow>,
        logger: &Arc<dyn Logger>,
    ) {
        profile.account_provider = Some({
            let weak = weak.clone();
            Arc::new(move || {
                weak.upgrade()
                    .and_then(|api| api.steam_providers().0)
                    .map_or_else(LocalAccount::default, |provider| provider())
            })
        });
        profile.friend_provider = Some({
            let weak = weak.clone();
            Arc::new(move || {
                weak.upgrade()
                    .and_then(|api| api.steam_providers().1)
                    .map_or_else(Vec::new, |provider| provider())
            })
        });
        profile.on_client_modified = Some({
            let notifications = notifications.clone();
            let logger = logger.clone();
            Arc::new(move || {
                // Уведомление не должно ронять запрос (`OnlineProfileService.js:145`).
                let _ = notify_client_modified(&notifications, logger.as_ref());
            })
        });
        profile.on_change = Some({
            let events = events.clone();
            let telemetry = telemetry.clone();
            Arc::new(move |snapshot: Value| {
                let signed_in = js::truthy(&snapshot["account"]["signedIn"]);
                send(&events, channels::RUNNER_PROFILE_STATE, snapshot);
                telemetry.on_profile_change(signed_in);
            })
        });
        profile.on_moderation = Some({
            let notifications = notifications.clone();
            // Уведомление не должно ронять синхронизацию (`OnlineProfileService.js:114`).
            Arc::new(move |notice: Value| {
                let _ = notify_moderation(&notifications, &notice);
            })
        });
        profile.on_support_thanks = Some({
            let notifications = notifications.clone();
            let events = events.clone();
            Arc::new(move |notice: Value| {
                let _ = notify_support_thanks(&notifications, &notice);
                send(&events, channels::SUPPORT_THANKS, notice);
            })
        });
        profile.on_consent = Some({
            let telemetry = telemetry.clone();
            let logger = logger.clone();
            Arc::new(move |enabled| {
                if let Err(error) = telemetry.on_consent(enabled) {
                    logger.warn(&format!(
                        "[telemetry] согласие не записано: {}",
                        error.message
                    ));
                }
            })
        });
        profile.on_unlock = Some({
            let events = events.clone();
            Arc::new(move |item: Value| send(&events, channels::ACHIEVEMENT_UNLOCKED, item))
        });
        profile.on_authenticated = Some({
            let events = events.clone();
            let window = window.clone();
            Arc::new(move |_snapshot: Value| {
                window.reveal();
                send(&events, channels::RUNNER_LOGIN_SUCCESS, Value::Null);
            })
        });
    }

    fn steam_providers(&self) -> (Option<AccountProvider>, Option<FriendProvider>) {
        let steam = self.steam.read().unwrap_or_else(|p| p.into_inner());
        (steam.account.clone(), steam.friends.clone())
    }

    /// `accountProvider`/`friendProvider` (`gameDelivery.steamAccountState()`/`steamFriends()`):
    /// Steam подключает подпроект 3.
    pub fn set_steam_providers(
        &self,
        account: Option<AccountProvider>,
        friends: Option<FriendProvider>,
    ) {
        let mut steam = self.steam.write().unwrap_or_else(|p| p.into_inner());
        steam.account = account;
        steam.friends = friends;
    }

    /// Получатель событий окна.
    pub fn subscribe(&self) -> OnlineEvents {
        OnlineEvents::new(self.events.subscribe(), self.logger.clone())
    }

    /// Порты для доставки и платформы.
    pub fn ports(&self) -> OnlinePorts {
        OnlinePorts {
            notifier: Arc::new(OnlineNotifier::new(
                self.notifications.clone(),
                self.versions.clone(),
                self.logger.clone(),
            )),
            telemetry: Arc::new(self.telemetry.clone()),
            achievements: Arc::new(self.profiles.clone()),
            presence: Arc::new(self.discord.clone()),
        }
    }

    /// Центр уведомлений (им пишут и другие подпроекты).
    pub fn notifications(&self) -> &Arc<NotificationStore> {
        &self.notifications
    }

    /// События версий (`updateManager.setVersionEvents`, подпроект 6).
    pub fn version_events(&self) -> &Arc<VersionEvents> {
        &self.versions
    }

    /// События обновления лаунчера (`updateManager.setUpdateTelemetry`, подпроект 6).
    pub fn launcher_update_events(&self) -> &LauncherUpdateEvents {
        &self.launcher_updates
    }

    /// Служба телеметрии (снимок для диагностики подпроекта 6).
    pub fn telemetry(&self) -> &TelemetryService {
        &self.telemetry
    }

    /// Служба профиля (`diagnosticsProfile`, подпроект 6).
    pub fn profiles(&self) -> &OnlineProfileService {
        &self.profiles
    }

    /// Мост Discord: привязка игрового моста присутствия (подпроект 3), диагностика.
    pub fn discord(&self) -> &DiscordRichPresenceBridge {
        &self.discord
    }

    /// Папка данных (`app.getPath("userData")`).
    pub fn user_data_dir(&self) -> &Path {
        &self.user_data_dir
    }

    /// Официальна ли сборка для окна; перемены — событие [`channels::BUILD_OFFICIAL`].
    pub fn standing(&self) -> Standing {
        self.official.standing()
    }

    fn in_runtime<'a, T>(&'a self, future: BoxFuture<'a, T>) -> InRuntime<'a, T> {
        InRuntime {
            runtime: &self.runtime,
            inner: future,
        }
    }

    // ------------------------------------------------------------ жизненный цикл

    /// `discordRpc.start()` в конце конструктора `AppManager` (после моста присутствия ПП3).
    pub fn start_discord(&self) {
        let _runtime = self.runtime.enter();
        self.discord.start();
    }

    /// Онлайн-часть `initialize()` (`AppManager.js:1055-1072`): согласие тому, кто вошёл,
    /// телеметрия, опрос онлайна и профиля.
    pub fn initialize(&self) {
        let _runtime = self.runtime.enter();
        self.telemetry.start_on_launch();
        self.online.start();
        self.profiles.start();
    }

    /// Окно загрузилось: `noteActiveDay` (`did-finish-load`, `AppManager.js:289-291`), затем
    /// онлайн-часть `ready` окна (`AppManager.js:1027-1041`): снять устаревшие приглашения обновить
    /// лаунчер, квитанция обновления → `LAUNCHER_UPDATED` и `launcher_update_installed`.
    pub fn window_loaded(&self) {
        let _runtime = self.runtime.enter();
        self.profiles.note_active_day();
        self.versions
            .resolve_update_offers("launcher", Some(&self.launcher_version), None);
        let receipt = self.versions.peek_receipt();
        match self.versions.consume_receipt(&self.launcher_version) {
            Ok(consumed) => {
                if let Some(receipt) = &receipt
                    && receipt.to == self.launcher_version
                {
                    self.launcher_updates
                        .installed(receipt.from_text().unwrap_or("unknown"), Some(&receipt.to));
                }
                if let (Some(_), Some(receipt)) = (consumed, &receipt) {
                    self.logger
                        .info(&format!("[versions] подтверждена установка {}", receipt.to));
                }
            }
            Err(message) => self
                .logger
                .warn(&format!("[versions] receipt не обработан: {message}")),
        }
    }

    /// `focus` окна (`AppManager.js:664-668`): `noteNewDay` — лаунчер открыт со вчера, день серии
    /// засчитывается без перезапуска.
    pub fn window_focused(&self) {
        let _runtime = self.runtime.enter();
        self.profiles.note_new_day();
    }

    /// Окно показано или развёрнуто (`show`/`restore`, `AppManager.js:1074-1075`): счётчики онлайна.
    pub fn window_shown(&self) {
        let online = self.online.clone();
        self.runtime.spawn(async move {
            online.fetch_once(false).await;
        });
    }

    /// `discordRpc.shutdown()` — первым в `dispose()`.
    pub fn stop_discord(&self) {
        let _runtime = self.runtime.enter();
        self.discord.shutdown();
    }

    /// Онлайн-часть `dispose()` после доставки и обновлялки: `telemetry.shutdown()`, затем
    /// `onlineClient.stop()` и `onlineProfiles.stop()` (сессии игр закрываются `session/end`).
    pub fn shutdown(&self) {
        let _runtime = self.runtime.enter();
        self.telemetry.shutdown();
        self.online.stop();
        self.profiles.stop();
    }

    /// Дождаться начатой работы профиля (`session/end`) и кадров закрытия Discord. Отправку
    /// телеметрии при выходе не ждём, как 0.4.7: очередь уйдёт при следующем запуске.
    pub async fn finish(&self) {
        self.in_runtime(Box::pin(async {
            self.profiles.settle().await;
            self.discord.finish().await;
        }))
        .await;
    }

    /// Дождаться фоновой работы профиля и начатых отправок телеметрии (тесты).
    pub async fn settle(&self) {
        self.in_runtime(Box::pin(async {
            self.profiles.settle().await;
            self.telemetry.settle().await;
        }))
        .await;
    }

    // ------------------------------------------------------------ игра (подпроекты 2, 3)

    /// `telemetryGameStarting(gameId)` (`launch()`): игровая сессия телеметрии.
    pub fn game_starting(&self, game_id: &str, installed: Option<&Value>) {
        let _runtime = self.runtime.enter();
        self.telemetry.game_starting(game_id, installed);
    }

    /// `telemetryGameLaunchFailed(gameId, error)`.
    pub fn game_launch_failed(&self, game_id: &str, error: &Value) {
        let _runtime = self.runtime.enter();
        self.telemetry.game_launch_failed(game_id, error);
    }

    /// Процесс игры `running` (`game-delivery/index.js:658-659`): сессия на сервере и телеметрия.
    pub fn game_running(&self, game_id: &str) {
        let _runtime = self.runtime.enter();
        self.profiles.game_start(game_id);
        self.telemetry.game_running(game_id);
    }

    /// Процесс игры `finished` (`game-delivery/index.js:712-713`): сессия на сервере закрыта,
    /// телеметрия. `crashed` — `lastExit.outcome === "crashed"` и не по просьбе пользователя.
    pub fn game_finished(&self, game_id: &str, last_exit: Option<&Value>, crashed: bool) {
        let _runtime = self.runtime.enter();
        self.profiles.game_end(game_id);
        self.telemetry.game_finished(game_id, last_exit, crashed);
    }

    /// Онлайн-ветка `game-stats-get`: при входе — игра из снимка сервера (`source: "server"`).
    pub fn game_stats_from_server(&self, game_id: &str) -> Option<Value> {
        self.profile_ipc.game_stats_from_server(game_id)
    }

    // ------------------------------------------------------------ каналы профиля

    /// Каналы `runner-*` и `profile-get` (`online-profile/ipc.js`, `game-delivery/index.js:1166`);
    /// неизвестный канал — `None`.
    pub async fn runner(&self, channel: &str, args: &[Value]) -> Option<Value> {
        self.in_runtime(Box::pin(self.profile_ipc.handle(channel, args)))
            .await
    }

    /// `friends-list({force})`: отказ — отклонённый промис.
    pub async fn friends_list(&self, options: Option<&Value>) -> Result<Value, RunnerError> {
        let force = force_of(options);
        self.in_runtime(Box::pin(self.profiles.friends(force)))
            .await
    }

    // ------------------------------------------------------------ публичные данные

    /// `public-online-get`.
    pub fn public_online_get(&self) -> Value {
        self.online.snapshot().to_json()
    }

    /// `public-news-get({force})`.
    pub async fn public_news_get(&self, options: Option<&Value>) -> Value {
        let force = force_of(options);
        self.in_runtime(Box::pin(
            async move { self.news.get(force).await.to_json() },
        ))
        .await
    }

    /// `public-sponsors-get({force})`.
    pub async fn public_sponsors_get(&self, options: Option<&Value>) -> Value {
        let force = force_of(options);
        self.in_runtime(Box::pin(async move { self.sponsors.get(force).await }))
            .await
    }

    /// `public-style-presets-get({force})`.
    pub async fn public_style_presets_get(&self, options: Option<&Value>) -> Value {
        let force = force_of(options);
        self.in_runtime(Box::pin(async move { self.style_presets.get(force).await }))
            .await
    }

    /// `sponsor-open-link({section, index})`.
    pub async fn sponsor_open_link(&self, target: Option<&Value>) -> Value {
        self.in_runtime(Box::pin(self.sponsors_ipc.open_link(target)))
            .await
    }

    /// `support-open(key, code?)`.
    pub async fn support_open(&self, key: Option<&Value>, code: Option<&Value>) -> Value {
        self.in_runtime(Box::pin(self.sponsors_ipc.support_open(key, code)))
            .await
    }

    /// `public-news-react(postId, reaction)`.
    pub async fn public_news_react(
        &self,
        post_id: Option<&Value>,
        reaction: Option<&Value>,
    ) -> Value {
        match (
            post_id.and_then(Value::as_str),
            reaction.and_then(Value::as_str),
        ) {
            (Some(post_id), Some(reaction)) => {
                self.in_runtime(Box::pin(self.news_ipc.react(post_id, reaction)))
                    .await
            }
            _ => news_failed(),
        }
    }

    /// `public-news-mine(ids)`.
    pub async fn public_news_mine(&self, ids: Option<&Value>) -> Value {
        let Some(list) = ids.and_then(Value::as_array) else {
            return news_failed();
        };
        let ids: Option<Vec<String>> = list
            .iter()
            .map(|id| id.as_str().map(str::to_owned))
            .collect();
        match ids {
            Some(ids) => {
                self.in_runtime(Box::pin(async move { self.news_ipc.mine(&ids).await }))
                    .await
            }
            None => news_failed(),
        }
    }

    /// `public-news-open-link(url)`.
    pub async fn public_news_open_link(&self, url: Option<&Value>) -> Value {
        match url.and_then(Value::as_str) {
            Some(url) => {
                self.in_runtime(Box::pin(self.news_ipc.open_link(url)))
                    .await
            }
            None => news_failed(),
        }
    }

    // ------------------------------------------------------------ уведомления

    /// `notifications-list`: `{items, unread}` от новых к старым.
    pub fn notifications_list(&self) -> Value {
        let (items, unread) = self.notifications.list();
        json!({"items": items, "unread": unread})
    }

    /// `notifications-mark-read(id)`: не строка — `false`.
    pub fn notifications_mark_read(&self, id: Option<&Value>) -> Value {
        let marked = id
            .and_then(Value::as_str)
            .is_some_and(|id| self.notifications.mark_read(id).unwrap_or(false));
        Value::Bool(marked)
    }

    /// `notifications-mark-all-read`.
    pub fn notifications_mark_all_read(&self) -> Value {
        Value::Bool(self.notifications.mark_all_read().unwrap_or(false))
    }

    /// `notifications-clear`.
    pub fn notifications_clear(&self) -> Value {
        Value::Bool(self.notifications.clear().unwrap_or(false))
    }

    // ------------------------------------------------------------ Discord и телеметрия

    /// `discord-rpc-get`.
    pub fn discord_rpc_get(&self) -> Value {
        self.discord.get()
    }

    /// `discord-rpc-set(enabled)`: `enabled === true`; настройка не записалась — отказ канала.
    pub fn discord_rpc_set(&self, enabled: Option<&Value>) -> Result<Value, String> {
        let _runtime = self.runtime.enter();
        self.discord
            .set(enabled == Some(&Value::Bool(true)))
            .map_err(|error| error.message)
    }

    /// `discord-rpc-activity` (send окна): раздел, где сейчас игрок.
    pub fn discord_rpc_activity(&self, activity: Option<&Value>) -> Value {
        let _runtime = self.runtime.enter();
        self.discord.set_activity(activity);
        Value::Null
    }

    /// `telemetry-reset-identity` (окно 0.4.7 его не вызывает, канал есть в `preload.js`).
    pub fn telemetry_reset_identity(&self) -> Value {
        let _runtime = self.runtime.enter();
        let result = self.telemetry.reset_identity();
        self.logger
            .info("[telemetry] идентичность сброшена по запросу пользователя");
        json!({"ok": true, "hadIdentity": result["hadIdentity"]})
    }

    // ------------------------------------------------------------ каналы по имени

    /// Вызов канала окна по имени; неизвестный канал — `None`, отказ — `Some(Err(текст))`.
    pub async fn call(&self, channel: &str, args: &[Value]) -> Option<Result<Value, String>> {
        let a = |index: usize| args.get(index);
        let value = match channel {
            "friends-list" => {
                return Some(self.friends_list(a(0)).await.map_err(|error| error.message));
            }
            "discord-rpc-set" => return Some(self.discord_rpc_set(a(0))),
            "public-online-get" => self.public_online_get(),
            "public-news-get" => self.public_news_get(a(0)).await,
            "public-sponsors-get" => self.public_sponsors_get(a(0)).await,
            "public-style-presets-get" => self.public_style_presets_get(a(0)).await,
            "sponsor-open-link" => self.sponsor_open_link(a(0)).await,
            "support-open" => self.support_open(a(0), a(1)).await,
            "public-news-react" => self.public_news_react(a(0), a(1)).await,
            "public-news-mine" => self.public_news_mine(a(0)).await,
            "public-news-open-link" => self.public_news_open_link(a(0)).await,
            "notifications-list" => self.notifications_list(),
            "notifications-mark-read" => self.notifications_mark_read(a(0)),
            "notifications-mark-all-read" => self.notifications_mark_all_read(),
            "notifications-clear" => self.notifications_clear(),
            "discord-rpc-get" => self.discord_rpc_get(),
            "discord-rpc-activity" => self.discord_rpc_activity(a(0)),
            "telemetry-reset-identity" => self.telemetry_reset_identity(),
            _ => return self.runner(channel, args).await.map(Ok),
        };
        Some(Ok(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hlsr_delivery::ports::MemoryLogger;

    fn event(n: u64) -> OnlineEvent {
        OnlineEvent {
            channel: channels::RUNNER_PROFILE_STATE,
            payload: json!(n),
        }
    }

    /// Отставший получатель не теряет связь: пропуск — в журнал, дальше — самые старые из
    /// уцелевших событий по порядку.
    #[tokio::test]
    async fn lagging_receiver_skips_and_keeps_reading() {
        let (sender, receiver) = broadcast::channel(2);
        let logger = Arc::new(MemoryLogger::default());
        let mut events = OnlineEvents::new(receiver, logger.clone());
        for n in 1..=5 {
            sender.send(event(n)).unwrap();
        }
        // В очереди уцелели 4 и 5; 1–3 пропущены.
        assert_eq!(events.recv().await.unwrap().payload, json!(4));
        assert_eq!(events.try_recv().unwrap().payload, json!(5));
        assert!(events.try_recv().is_none());
        assert_eq!(
            logger.take(),
            [(
                "warn",
                "[online] окно отстало от событий, пропущено: 3".to_owned()
            )]
        );

        // `try_recv` тоже переживает пропуск.
        for n in 6..=10 {
            sender.send(event(n)).unwrap();
        }
        assert_eq!(events.try_recv().unwrap().payload, json!(9));
        assert_eq!(events.try_recv().unwrap().payload, json!(10));
        assert_eq!(logger.take().len(), 1);

        // Фасад удалён: события кончились.
        drop(sender);
        assert!(events.recv().await.is_none());
        assert!(events.try_recv().is_none());
    }
}
