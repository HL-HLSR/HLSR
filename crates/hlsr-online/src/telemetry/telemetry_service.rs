//! `telemetry/TelemetryService.js`: сессия лаунчера и игровые сессии, сердцебиения, маркер
//! оборвавшейся сессии, отчёт об установке, согласие и очистка; плюс то, что в 0.4.7 вокруг
//! службы: обёртки игры из `game-delivery/index.js` (`telemetryGame*`, `telemetryBuildMeta`),
//! обвязка `AppManager` (согласие при старте, `onConsent`, выход из профиля) и отчёт об удалении
//! из `uninstall/cleanup-cli.js` — исправленный: с токеном входа и с дожиданием отправки.
//!
//! Отправки «в фоне» (`sender.flush().catch(() => {})`) выполняют синхронную часть сразу, запрос —
//! в задаче tokio; [`TelemetryService::settle`] дожидается начатых отправок.

use super::config;
use super::consent;
use super::crash_marker;
use super::event_schema::{self, Kind};
use super::identity;
use super::install_receipt;
use super::queue::TelemetryQueue;
use super::sender::{AuthorizationProvider, FlushResult, Sender, SenderOptions};
use super::transport::Transport;
use hlsr_core::js;
use hlsr_core::js::iso::iso_from_ms;
use hlsr_core::json_text::js_number;
use hlsr_delivery::error_map;
use hlsr_delivery::errors::RawError;
use hlsr_delivery::ports::Logger;
use hlsr_delivery::timing::{Clock, Random, system_clock, system_random};
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;
use tokio::task::JoinHandle;

/// Поставщик значения (`localeProvider`, `isWindowVisible`, источник UUID).
pub type Provider<T> = Arc<dyn Fn() -> T + Send + Sync>;

/// `windowsMajor()`: «10» или «11» по номеру сборки Windows (`os.release()`), иначе `unknown`.
pub fn windows_major() -> &'static str {
    #[cfg(windows)]
    {
        use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
        use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
        // SAFETY: структура обнулена, размер задан; RtlGetVersion только заполняет её.
        let info = unsafe {
            let mut info: OSVERSIONINFOW = std::mem::zeroed();
            info.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
            (RtlGetVersion(&mut info) == 0).then_some(info)
        };
        match info {
            Some(info) if info.dwMajorVersion == 10 => {
                if info.dwBuildNumber >= 22000 {
                    "11"
                } else {
                    "10"
                }
            }
            _ => "unknown",
        }
    }
    #[cfg(not(windows))]
    {
        "unknown"
    }
}

/// `windowsArch()`: сборки лаунчера только x64.
pub fn windows_arch() -> &'static str {
    "x64"
}

/// `jitter(valueMs, ratio)` службы: `round(value − spread + random · 2 · spread)`, не меньше 1 с.
pub fn jitter(value_ms: f64, ratio: f64, random: f64) -> f64 {
    let spread = value_ms * ratio;
    js::math_round(value_ms - spread + random * spread * 2.0).max(1000.0)
}

/// Параметры службы (`options` конструктора 0.4.7).
pub struct TelemetryOptions {
    pub user_data_dir: PathBuf,
    /// `launcherVersion`: пусто — `0.0.0`.
    pub launcher_version: Option<String>,
    pub logger: Arc<dyn Logger>,
    pub clock: Clock,
    pub random: Random,
    /// `localeProvider`: по умолчанию `ru`.
    pub locale: Option<Provider<Option<String>>>,
    /// `isWindowVisible`: по умолчанию — видно.
    pub window_visible: Option<Provider<bool>>,
    /// `backendConfigured`: по умолчанию [`config::BACKEND_CONFIGURED`].
    pub backend_configured: Option<bool>,
    /// `installDir`: папка лаунчера с маркером установщика.
    pub install_dir: Option<PathBuf>,
    /// `authorizationProvider`: токен входа; нет поставщика — телеметрия не смотрит на вход.
    pub authorization: Option<AuthorizationProvider>,
    /// `endpoint`: по умолчанию [`config::ENDPOINT_BATCH`].
    pub endpoint: Option<String>,
    pub transport: Arc<dyn Transport>,
    /// Источник UUID (`crypto.randomUUID`); тесты сверки подставляют последовательность.
    pub new_uuid: Option<Provider<String>>,
    /// `windowsMajor()` вместо настоящего: тесты сверки, снятой в Linux, на Windows.
    pub windows_major: Option<&'static str>,
}

impl TelemetryOptions {
    /// Умолчания: системные часы и случайность, без поставщиков.
    pub fn new(
        user_data_dir: impl Into<PathBuf>,
        launcher_version: impl Into<String>,
        logger: Arc<dyn Logger>,
        transport: Arc<dyn Transport>,
    ) -> TelemetryOptions {
        TelemetryOptions {
            user_data_dir: user_data_dir.into(),
            launcher_version: Some(launcher_version.into()),
            logger,
            clock: system_clock(),
            random: system_random(),
            locale: None,
            window_visible: None,
            backend_configured: None,
            install_dir: None,
            authorization: None,
            endpoint: None,
            transport,
            new_uuid: None,
            windows_major: None,
        }
    }
}

/// Игровая сессия: `{sessionId, startedMs, meta: {gameVersion, buildId}}`.
#[derive(Debug, Clone)]
struct GameSession {
    session_id: String,
    started_ms: i64,
    meta: Value,
}

#[derive(Default)]
struct State {
    started: bool,
    launcher_session_id: Option<String>,
    launcher_session_started_ms: Option<i64>,
    /// `Map` 0.4.7: порядок вставки, повторный `set` оставляет место.
    game_sessions: Vec<(String, GameSession)>,
}

struct Inner {
    user_data_dir: PathBuf,
    launcher_version: String,
    logger: Arc<dyn Logger>,
    clock: Clock,
    random: Random,
    locale: Option<Provider<Option<String>>>,
    window_visible: Option<Provider<bool>>,
    backend_configured: bool,
    install_dir: Option<PathBuf>,
    authorization: Option<AuthorizationProvider>,
    new_uuid: Provider<String>,
    windows_major: &'static str,
    queue: Arc<Mutex<TelemetryQueue>>,
    sender: Arc<Sender>,
    state: Mutex<State>,
    launcher_timer: Mutex<Option<JoinHandle<()>>>,
    game_timer: Mutex<Option<JoinHandle<()>>>,
    pending: Mutex<Vec<JoinHandle<FlushResult>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Служба телеметрии; копии — одна служба.
#[derive(Clone)]
pub struct TelemetryService {
    inner: Arc<Inner>,
}

impl TelemetryService {
    pub fn new(options: TelemetryOptions) -> TelemetryService {
        let launcher_version = options
            .launcher_version
            .filter(|version| !version.is_empty())
            .unwrap_or_else(|| "0.0.0".to_owned());
        let queue = Arc::new(Mutex::new(TelemetryQueue::new(
            options.user_data_dir.join(config::queue::FILE_NAME),
            config::queue::MAX_EVENTS,
            config::queue::MAX_BYTES,
            options.logger.clone(),
            options.clock.clone(),
        )));
        let sender = Arc::new(Sender::new(SenderOptions {
            queue: queue.clone(),
            endpoint: options.endpoint,
            launcher_version: Some(launcher_version.clone()),
            logger: options.logger.clone(),
            clock: options.clock.clone(),
            random: options.random.clone(),
            transport: options.transport,
            authorization: options.authorization.clone(),
        }));
        TelemetryService {
            inner: Arc::new(Inner {
                user_data_dir: options.user_data_dir,
                launcher_version,
                logger: options.logger,
                clock: options.clock,
                random: options.random,
                locale: options.locale,
                window_visible: options.window_visible,
                backend_configured: options
                    .backend_configured
                    .unwrap_or(config::BACKEND_CONFIGURED),
                install_dir: options.install_dir,
                authorization: options.authorization,
                new_uuid: options
                    .new_uuid
                    .unwrap_or_else(|| Arc::new(identity::random_uuid)),
                windows_major: options.windows_major.unwrap_or_else(windows_major),
                queue,
                sender,
                state: Mutex::default(),
                launcher_timer: Mutex::default(),
                game_timer: Mutex::default(),
                pending: Mutex::default(),
            }),
        }
    }

    fn now(&self) -> i64 {
        (self.inner.clock)()
    }

    fn dir(&self) -> &std::path::Path {
        &self.inner.user_data_dir
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.inner.state)
    }

    /// Отправитель очереди (`telemetry.sender`).
    pub fn sender(&self) -> &Arc<Sender> {
        &self.inner.sender
    }

    /// Вошёл ли пользователь: поставщика нет — не важно; иначе токен не пустой.
    fn authorized(&self) -> bool {
        self.inner
            .authorization
            .as_ref()
            .is_none_or(|provider| provider().is_some_and(|token| !token.is_empty()))
    }

    /// `enabled()`: бэкенд настроен, вход выполнен (если служба о нём знает), согласие дано.
    pub fn enabled(&self) -> bool {
        self.inner.backend_configured && self.authorized() && consent::read(self.dir()).enabled
    }

    fn ensure_identity(&self) -> Result<identity::Identity, RawError> {
        let new_uuid = self.inner.new_uuid.clone();
        identity::ensure_with(self.dir(), self.now(), move || new_uuid())
    }

    /// `start()`: один раз; без согласия — очистка. Отчёт об обрыве прошлой сессии и об
    /// установке, новая сессия лаунчера, маркер, сердцебиение, отправка.
    pub fn start(&self) {
        {
            let mut state = self.state();
            if state.started {
                return;
            }
            state.started = true;
        }
        if !self.enabled() {
            self.purge_local_data();
            return;
        }
        if let Err(error) = self.start_session() {
            self.inner
                .logger
                .warn(&format!("[telemetry] запуск не удался: {}", error.message));
        }
    }

    fn start_session(&self) -> Result<(), RawError> {
        let id = self.ensure_identity()?;
        if let Some(crashed) = crash_marker::detect(self.dir(), Some(self.now())) {
            let queued = self.push(
                "launcher_crash_detected",
                &json!({"sessionId": crashed.session_id, "occurredAt": crashed.last_alive_at}),
            );
            if queued {
                crash_marker::consume(self.dir());
            }
        }
        self.report_install_if_needed();

        let session_id = (self.inner.new_uuid)();
        {
            let mut state = self.state();
            state.launcher_session_id = Some(session_id.clone());
            state.launcher_session_started_ms = Some(self.now());
        }
        crash_marker::open(self.dir(), &session_id, Some(self.now()))?;
        self.push("launcher_session_started", &json!({}));

        self.schedule_launcher_heartbeat();
        self.inner.sender.start();
        self.flush_in_background();

        let prefix = js::slice_utf16(&id.installation_id, 8);
        self.inner
            .logger
            .info(&format!("[telemetry] включена, установка {prefix}…"));
        Ok(())
    }

    /// `shutdown()`: конец сессии лаунчера и игровых сессий, маркер закрыт, отправка.
    pub fn shutdown(&self) {
        if !self.enabled() {
            self.stop_timers();
            return;
        }
        let (session, started_ms) = {
            let state = self.state();
            (
                state.launcher_session_id.clone(),
                state.launcher_session_started_ms,
            )
        };
        if session.is_some() {
            self.push(
                "launcher_session_ended",
                &json!({"durationSeconds": js_number(self.elapsed_seconds(started_ms))}),
            );
        }
        let games: Vec<String> = self
            .state()
            .game_sessions
            .iter()
            .map(|(id, _)| id.clone())
            .collect();
        for game_id in games {
            self.end_game_session(&game_id, false, Some("unknown"));
        }
        crash_marker::close(self.dir());
        self.flush_in_background();
        self.stop_timers();
    }

    /// `reportInstallIfNeeded()`: установщик оставил новый маркер — событие и квитанция.
    pub fn report_install_if_needed(&self) -> bool {
        let Some(install_dir) = &self.inner.install_dir else {
            return false;
        };
        let detected = install_receipt::detect_install(self.dir(), Some(install_dir));
        let (true, Some(install_id)) = (detected.report, detected.install_id) else {
            return false;
        };
        if self.push("launcher_install_completed", &json!({"sessionId": null})) {
            install_receipt::confirm_install(self.dir(), &install_id, self.now());
            return true;
        }
        false
    }

    /// `reportUninstallInitiated()`.
    pub fn report_uninstall_initiated(&self) -> bool {
        if !self.enabled() {
            return false;
        }
        self.push("launcher_uninstall_initiated", &json!({"sessionId": null}))
    }

    /// `stopTimers()`: сердцебиения и периодическая отправка.
    pub fn stop_timers(&self) {
        for timer in [&self.inner.launcher_timer, &self.inner.game_timer] {
            if let Some(handle) = lock(timer).take() {
                handle.abort();
            }
        }
        self.inner.sender.stop();
    }

    /// `setEnabled(enabled)`: записать согласие; включили — старт, выключили — очистка.
    pub fn set_enabled(&self, enabled: bool) -> Result<Value, RawError> {
        let previous = self.enabled();
        consent::set(self.dir(), enabled, self.now())?;
        if enabled && !previous {
            self.state().started = false;
            self.start();
            return Ok(json!({"enabled": true}));
        }
        if !enabled && previous {
            self.stop_timers();
            {
                let mut state = self.state();
                state.launcher_session_id = None;
                state.game_sessions.clear();
            }
            self.purge_local_data();
            self.inner
                .logger
                .info("[telemetry] выключена, локальные данные телеметрии удалены");
            return Ok(json!({"enabled": false, "purged": true}));
        }
        Ok(json!({"enabled": enabled}))
    }

    /// `purgeLocalData()`: очередь и маркер.
    pub fn purge_local_data(&self) {
        lock(&self.inner.queue).purge();
        crash_marker::close(self.dir());
    }

    /// `resetIdentity()`: новая личность установки.
    pub fn reset_identity(&self) -> Value {
        lock(&self.inner.queue).purge();
        let had = identity::reset(self.dir());
        crash_marker::close(self.dir());
        {
            let mut state = self.state();
            state.launcher_session_id = None;
            state.game_sessions.clear();
        }
        if self.enabled() {
            self.state().started = false;
            self.start();
        }
        json!({"ok": true, "hadIdentity": had})
    }

    /// `elapsedSeconds(startedMs)`: целые секунды до 30 суток; нет начала — 0.
    fn elapsed_seconds(&self, started_ms: Option<i64>) -> f64 {
        let Some(started) = started_ms.filter(|&ms| ms != 0) else {
            return 0.0;
        };
        (((self.now() - started) as f64 / 1000.0).floor())
            .clamp(0.0, event_schema::limits::DURATION_SECONDS)
    }

    /// `locale()`: `en…` — `en`, остальное — `ru`.
    fn locale(&self) -> &'static str {
        let value = self
            .inner
            .locale
            .as_ref()
            .and_then(|provider| provider())
            .filter(|locale| !locale.is_empty())
            .unwrap_or_else(|| "ru".to_owned());
        if value.to_lowercase().starts_with("en") {
            "en"
        } else {
            "ru"
        }
    }

    /// `push(type, fields)`: событие по схеме в очередь; `false` — выключено, тип неизвестен
    /// или выключен, событие не прошло проверку (предупреждение в журнал), очередь отказала.
    pub fn push(&self, event_type: &str, fields: &Value) -> bool {
        if !self.enabled() {
            return false;
        }
        let Some(spec) = event_schema::type_spec(event_type) else {
            return false;
        };
        if !event_schema::is_enabled_type(event_type) {
            return false;
        }
        let Ok(id) = self.ensure_identity() else {
            return false;
        };
        let empty = Map::new();
        let overrides = fields.as_object().unwrap_or(&empty);
        let given = |key: &str| {
            overrides
                .get(key)
                .filter(|value| js::truthy(value))
                .cloned()
        };

        let session_id = match spec.kind {
            Kind::Launcher => given("sessionId").unwrap_or_else(|| {
                self.state()
                    .launcher_session_id
                    .clone()
                    .map_or(Value::Null, Value::String)
            }),
            Kind::Game => given("sessionId").unwrap_or(Value::Null),
            Kind::None => Value::Null,
        };
        let occurred_at = given("occurredAt").unwrap_or_else(|| {
            let iso = iso_from_ms(self.now());
            Value::String(format!("{}.000Z", &iso[..iso.len() - ".000Z".len()]))
        });

        let mut event = Map::new();
        event.insert("schemaVersion".into(), json!(event_schema::SCHEMA_VERSION));
        event.insert("eventId".into(), json!((self.inner.new_uuid)()));
        event.insert("installationId".into(), json!(id.installation_id));
        event.insert("sessionId".into(), session_id);
        event.insert("type".into(), json!(event_type));
        event.insert("occurredAt".into(), occurred_at);
        event.insert("launcherVersion".into(), json!(self.inner.launcher_version));
        event.insert("locale".into(), json!(self.locale()));
        event.insert("windowsMajor".into(), json!(self.inner.windows_major));
        event.insert("windowsArch".into(), json!(windows_arch()));
        for field in spec.fields {
            if let Some(value) = overrides.get(*field) {
                event.insert((*field).to_owned(), value.clone());
            }
        }
        let event = Value::Object(event);

        if let Err(reason) = event_schema::validate(&event) {
            self.inner.logger.warn(&format!(
                "[telemetry] событие {event_type} отброшено: {reason}"
            ));
            return false;
        }
        lock(&self.inner.queue).append(&event)
    }

    /// `emit(type, fields)`: [`push`](Self::push) без исключений.
    pub fn emit(&self, event_type: &str, fields: &Value) -> bool {
        self.push(event_type, fields)
    }

    /// `this.sender.flush().catch(() => {})`.
    fn flush_in_background(&self) {
        if let Some(handle) = self.inner.sender.flush_in_background() {
            let mut pending = lock(&self.inner.pending);
            pending.retain(|handle| !handle.is_finished());
            pending.push(handle);
        }
    }

    /// Дождаться отправок, начатых в фоне (завершение приложения, тесты).
    pub async fn settle(&self) {
        loop {
            let handles: Vec<_> = std::mem::take(&mut *lock(&self.inner.pending));
            if handles.is_empty() {
                return;
            }
            for handle in handles {
                let _ = handle.await;
            }
        }
    }

    fn spawn_timer(
        &self,
        slot: fn(&Inner) -> &Mutex<Option<JoinHandle<()>>>,
        delay_ms: f64,
        tick: fn(&TelemetryService),
    ) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        let handle = runtime.spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms as u64)).await;
            let Some(inner) = weak.upgrade() else {
                return;
            };
            // Своя задача уже сработала: снять её без отмены (`clearTimeout` после срабатывания).
            drop(lock(slot(&inner)).take());
            tick(&TelemetryService { inner });
        });
        if let Some(old) = lock(slot(&self.inner)).replace(handle) {
            old.abort();
        }
    }

    /// `scheduleLauncherHeartbeat()`: раз в 2 мин ± 20 %, пока телеметрия включена.
    fn schedule_launcher_heartbeat(&self) {
        if let Some(old) = lock(&self.inner.launcher_timer).take() {
            old.abort();
        }
        let delay = jitter(
            config::heartbeat::LAUNCHER_MS as f64,
            config::heartbeat::JITTER_RATIO,
            (self.inner.random)(),
        );
        self.spawn_timer(
            |inner| &inner.launcher_timer,
            delay,
            |service| {
                service.launcher_heartbeat();
                if service.enabled() {
                    service.schedule_launcher_heartbeat();
                }
            },
        );
    }

    /// `launcherHeartbeat()`: только при видимом окне; маркер обновляется.
    pub fn launcher_heartbeat(&self) {
        if !self.enabled() || self.state().launcher_session_id.is_none() {
            return;
        }
        if !self
            .inner
            .window_visible
            .as_ref()
            .is_none_or(|visible| visible())
        {
            return;
        }
        self.push("launcher_heartbeat", &json!({}));
        crash_marker::touch(self.dir(), Some(self.now()));
        self.flush_in_background();
    }

    /// `scheduleGameHeartbeat()`: раз в минуту ± 20 %, пока есть игровые сессии.
    fn schedule_game_heartbeat(&self) {
        if let Some(old) = lock(&self.inner.game_timer).take() {
            old.abort();
        }
        if self.state().game_sessions.is_empty() {
            return;
        }
        let delay = jitter(
            config::heartbeat::GAME_MS as f64,
            config::heartbeat::JITTER_RATIO,
            (self.inner.random)(),
        );
        self.spawn_timer(
            |inner| &inner.game_timer,
            delay,
            |service| {
                service.game_heartbeat();
                service.schedule_game_heartbeat();
            },
        );
    }

    fn game_fields(game_id: &str, session: &GameSession) -> Map<String, Value> {
        let mut fields = Map::new();
        fields.insert("sessionId".into(), json!(session.session_id));
        fields.insert("gameId".into(), json!(game_id));
        fields.insert("gameVersion".into(), session.meta["gameVersion"].clone());
        fields.insert("buildId".into(), session.meta["buildId"].clone());
        fields
    }

    /// `gameHeartbeat()`: событие на каждую игровую сессию.
    pub fn game_heartbeat(&self) {
        if !self.enabled() {
            return;
        }
        let sessions = self.state().game_sessions.clone();
        for (game_id, session) in &sessions {
            self.push(
                "game_heartbeat",
                &Value::Object(Self::game_fields(game_id, session)),
            );
        }
        self.flush_in_background();
    }

    fn session(&self, game_id: &str) -> Option<GameSession> {
        self.state()
            .game_sessions
            .iter()
            .find(|(id, _)| id == game_id)
            .map(|(_, session)| session.clone())
    }

    fn remove_session(&self, game_id: &str) {
        self.state().game_sessions.retain(|(id, _)| id != game_id);
    }

    /// `beginGameSession(gameId, meta)`: новая игровая сессия; выключено — `None`.
    pub fn begin_game_session(&self, game_id: &str, meta: Option<&Value>) -> Option<String> {
        if !self.enabled() {
            return None;
        }
        let meta_field = |key: &str, fallback: &str| {
            meta.and_then(|meta| meta.get(key))
                .filter(|value| js::truthy(value))
                .cloned()
                .unwrap_or_else(|| json!(fallback))
        };
        let session = GameSession {
            session_id: (self.inner.new_uuid)(),
            started_ms: self.now(),
            meta: json!({
                "gameVersion": meta_field("gameVersion", "0.0.0"),
                "buildId": meta_field("buildId", "unknown"),
            }),
        };
        {
            let mut state = self.state();
            match state.game_sessions.iter_mut().find(|(id, _)| id == game_id) {
                Some((_, existing)) => *existing = session.clone(),
                None => state
                    .game_sessions
                    .push((game_id.to_owned(), session.clone())),
            }
        }
        self.push(
            "game_launch_started",
            &Value::Object(Self::game_fields(game_id, &session)),
        );
        Some(session.session_id)
    }

    /// `confirmGameRunning(gameId)`: игра запустилась — событие и сердцебиение игры.
    pub fn confirm_game_running(&self, game_id: &str) {
        let Some(session) = self.session(game_id) else {
            return;
        };
        if !self.enabled() {
            return;
        }
        self.push(
            "game_launch_succeeded",
            &Value::Object(Self::game_fields(game_id, &session)),
        );
        self.schedule_game_heartbeat();
        self.flush_in_background();
    }

    /// `failGameLaunch(gameId, errorCode)`: код вне списка схемы — `UNKNOWN_SAFE`.
    pub fn fail_game_launch(&self, game_id: &str, error_code: &str) {
        let Some(session) = self.session(game_id) else {
            return;
        };
        if !self.enabled() {
            return;
        }
        let code = if event_schema::ERROR_CODES.contains(&error_code) {
            error_code
        } else {
            "UNKNOWN_SAFE"
        };
        let mut fields = Self::game_fields(game_id, &session);
        fields.insert("errorCode".into(), json!(code));
        self.push("game_launch_failed", &Value::Object(fields));
        self.remove_session(game_id);
        self.schedule_game_heartbeat();
    }

    /// `endGameSession(gameId, {crashed, exitReason})`: падение — `game_crashed`, иначе
    /// `game_session_ended` с причиной из списка схемы (иначе `unknown`).
    pub fn end_game_session(&self, game_id: &str, crashed: bool, exit_reason: Option<&str>) {
        let Some(session) = self.session(game_id) else {
            return;
        };
        if self.enabled() {
            let duration = js_number(self.elapsed_seconds(Some(session.started_ms)));
            let mut fields = Self::game_fields(game_id, &session);
            fields.insert("durationSeconds".into(), duration);
            if crashed {
                fields.insert("errorCode".into(), json!("GAME_CRASH"));
                self.push("game_crashed", &Value::Object(fields));
            } else {
                let reason = exit_reason
                    .filter(|reason| event_schema::EXIT_REASONS.contains(reason))
                    .unwrap_or("unknown");
                fields.insert("exitReason".into(), json!(reason));
                self.push("game_session_ended", &Value::Object(fields));
            }
        }
        self.remove_session(game_id);
        self.schedule_game_heartbeat();
        self.flush_in_background();
    }

    /// `gameSessionId(gameId)`.
    pub fn game_session_id(&self, game_id: &str) -> Option<String> {
        self.session(game_id).map(|session| session.session_id)
    }

    /// `gameSessionMeta(gameId)`: `{gameVersion, buildId}`.
    pub fn game_session_meta(&self, game_id: &str) -> Option<Value> {
        self.session(game_id).map(|session| session.meta)
    }

    /// `snapshot()`: состояние для диагностики.
    pub fn snapshot(&self) -> Value {
        let id = identity::read(self.dir());
        let (launcher_active, games) = {
            let state = self.state();
            (
                state.launcher_session_id.is_some(),
                state.game_sessions.len(),
            )
        };
        json!({
            "backendConfigured": self.inner.backend_configured,
            "enabled": self.enabled(),
            "hasIdentity": id.is_some(),
            "installationIdPrefix": id.map(|id| js::slice_utf16(&id.installation_id, 8)),
            "launcherSessionActive": launcher_active,
            "gameSessionsActive": games,
            "queue": self.inner.sender.snapshot(),
        })
    }

    // --- Обёртки игры из `game-delivery/index.js` (подпроект 3 вызывает их при запуске) ---

    /// `telemetryBuildMeta(gameId)`: `{gameVersion, buildId}` из записи установки (нет записи
    /// или она не читается — `None`).
    pub fn build_meta(installed: Option<&Value>) -> Value {
        let field = |key: &str, fallback: &str| {
            installed
                .and_then(|installed| installed.get(key))
                .filter(|value| js::truthy(value))
                .cloned()
                .unwrap_or_else(|| json!(fallback))
        };
        json!({"gameVersion": field("version", "0.0.0"), "buildId": field("buildId", "unknown")})
    }

    /// `telemetryGameStarting(gameId)`.
    pub fn game_starting(&self, game_id: &str, installed: Option<&Value>) {
        self.begin_game_session(game_id, Some(&Self::build_meta(installed)));
    }

    /// `telemetryGameRunning(gameId)` (без `RunLogReader` — он не переносится).
    pub fn game_running(&self, game_id: &str) {
        self.confirm_game_running(game_id);
    }

    /// `telemetryGameLaunchFailed(gameId, error)`: код ошибки — `errorMap.toTelemetryCode`.
    pub fn game_launch_failed(&self, game_id: &str, error: &Value) {
        self.fail_game_launch(game_id, error_map::to_telemetry_code(error));
    }

    /// `telemetryGameFinished(gameId, lastExit, crashed)`: закрыл пользователь —
    /// `user-closed`, вышла сама — `normal`, упала — `crashed`, иначе `unknown`.
    pub fn game_finished(&self, game_id: &str, last_exit: Option<&Value>, crashed: bool) {
        if crashed {
            self.end_game_session(game_id, true, None);
            return;
        }
        let requested = last_exit
            .and_then(|exit| exit.get("requested"))
            .is_some_and(js::truthy);
        let outcome = last_exit
            .and_then(|exit| exit.get("outcome"))
            .and_then(Value::as_str);
        let reason = if requested {
            "user-closed"
        } else if outcome == Some("exited") {
            "normal"
        } else if outcome == Some("crashed") {
            "crashed"
        } else {
            "unknown"
        };
        self.end_game_session(game_id, false, Some(reason));
    }

    // --- Обвязка `AppManager` ---

    /// Старт при запуске лаунчера (`AppManager.js:1055-1066`): кто вошёл, получает согласие
    /// обратно (выключателя больше нет), затем [`start`](Self::start).
    pub fn start_on_launch(&self) {
        let signed_in = self
            .inner
            .authorization
            .as_ref()
            .is_some_and(|provider| provider().is_some_and(|token| !token.is_empty()));
        if signed_in && !consent::read(self.dir()).enabled {
            if let Err(error) = consent::set(self.dir(), true, self.now()) {
                self.inner
                    .logger
                    .warn(&format!("[telemetry] не запущена: {}", error.message));
                return;
            }
            self.inner
                .logger
                .info("[telemetry] согласие восстановлено: вход в аккаунт выполнен");
        }
        self.start();
    }

    /// `onConsent(enabled)` профиля (`AppManager.js:268-273`): согласие при входе и выходе.
    pub fn on_consent(&self, enabled: bool) -> Result<(), RawError> {
        self.stop_timers();
        self.state().started = false;
        self.set_enabled(enabled)?;
        self.start();
        Ok(())
    }

    /// Снимок профиля изменился (`AppManager.js:256`): вышел — таймеры стоп.
    pub fn on_profile_change(&self, signed_in: bool) {
        if !signed_in {
            self.stop_timers();
        }
    }
}

impl hlsr_delivery::ports::Telemetry for TelemetryService {
    fn emit(&self, event_type: &str, fields: &Value) -> Result<(), String> {
        TelemetryService::emit(self, event_type, fields);
        Ok(())
    }
}

/// `reportUninstall(userData, opts, say)` из `uninstall/cleanup-cli.js`: событие о запуске
/// удаления. Исправление 0.4.7 (спецификация, решение 6): там служба создавалась без токена, сервер
/// отвечал 401 и `Sender` выбрасывал событие, а отправку никто не ждал. Здесь вызывающий даёт
/// `authorization` из хранилища входа, отправка дожидается (не дольше тайм-аута транспорта).
/// Строки `say` — как в 0.4.7.
pub async fn report_uninstall(options: TelemetryOptions, say: &mut (dyn FnMut(&str) + Send)) {
    let telemetry = TelemetryService::new(TelemetryOptions {
        window_visible: Some(Arc::new(|| false)),
        locale: Some(Arc::new(|| Some("ru".to_owned()))),
        ..options
    });
    if !telemetry.enabled() {
        say("telemetry: выключена, событие удаления не создано");
        return;
    }
    if !telemetry.report_uninstall_initiated() {
        say("telemetry: событие удаления не поставлено в очередь");
        return;
    }
    say("telemetry: отправлено событие о запуске удаления");
    telemetry.inner.sender.flush().await;
}
