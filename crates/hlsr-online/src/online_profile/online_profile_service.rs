//! `online-profile/OnlineProfileService.js`: вход по коду устройства (`/v1/link/start` → `poll`),
//! синхронизация профиля и достижений (`/v2/runner/sync`, `secret`), сессии игры (`session/start`
//! → пульс с `sequence` с 2 → `end`), игроки, поиск, профили, оценки, друзья (пачки по 50),
//! действия (`customize`, `speedrun/*`, `delete/*`, `news/*`, привязки `discord/*` и `telegram/*`),
//! ссылки забегов speedrun.com, выход (и на всех устройствах), «Спасибо за поддержку».
//! Отказы сервера разбираются в [`OnlineProfileService::call`] так же, как в 0.4.7 (`:169-219`).
//!
//! Сессия входа — в хранилище задачи 8 ([`SessionStore`], `online-session.v2.json`), а не в
//! `online-session.json` 0.4.7 (бета: файл 0.4.7 общий и не трогается). `secrets.json`,
//! `speedrun-links.json`, `moderation-seen.json`, `launch-days.json` — те же байты, что у 0.4.7.
//!
//! Асинхронность — как у промисов 0.4.7: у каждого «async»-метода синхронная часть (до первого
//! `await` в JS) выполняется сразу при вызове, остальное — в возвращённом future. Работа «в фоне»
//! (`….catch(() => {})`) — задачи tokio; [`OnlineProfileService::settle`] дожидается их. Таймеры
//! (опрос входа, синхронизация раз в 30 с) — через [`Timers`], чтобы тесты двигали их сами.

use super::moderation;
use super::profile_state::{self, LocalAccount, LocalPresence, RUN_PAGE};
use super::session_store::{Session, SessionStore};
use crate::js_value::{coerced, entries, field, is_object, safe_integer, to_number};
use crate::profile::launch_streak::DayZone;
use crate::profile::{achievements, launch_streak};
use crate::security::attestation::{BuildAttestation, ClientModified};
use crate::security::build_identity::BuildIdentity;
use crate::telemetry::config;
use crate::telemetry::transport::{RequestOptions, Response, Transport, TransportError};
use hlsr_core::js;
use hlsr_core::js::iso::iso_from_ms;
use hlsr_core::json_formats::{TmpOptions, write_compact_tmp};
use hlsr_core::json_text::json_parse;
use hlsr_core::notifications::NotificationStore;
use hlsr_core::notifications::catalog::level_for_code;
use hlsr_delivery::ports::{Logger, Shell};
use hlsr_delivery::providers::BoxFuture;
use hlsr_delivery::timing::{Clock, system_clock};
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, Weak};
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Рейтинг игроков: страница и глубина — как у сервера (`Hlsr\Profile\Players`).
pub const PLAYERS_SORTS: [&str; 5] = ["hours", "achievements", "launches", "rating", "new"];
pub const PLAYERS_PAGE: usize = 50;
pub const PLAYERS_DEPTH: i64 = 500;
/// Синхронизация и пульсы сессий игры.
pub const SYNC_INTERVAL_MS: f64 = 30_000.0;
/// Действия `action()` — то, что окно отправляет на сервер как есть.
pub const ACTIONS: [&str; 15] = [
    "customize",
    "delete/challenge",
    "delete/request",
    "delete/cancel",
    "speedrun/link",
    "speedrun/unlink",
    "speedrun/bests",
    "speedrun/pin",
    "speedrun/unpin",
    "news/react",
    "news/mine",
    "discord/code",
    "discord/unlink",
    "telegram/code",
    "telegram/unlink",
];
pub const SECRETS_FILE: &str = "secrets.json";
pub const RUN_LINKS_FILE: &str = "speedrun-links.json";
pub const MODERATION_FILE: &str = "moderation-seen.json";
/// Оплаты, за которые уже показано «Спасибо»: сервер повторяет их в каждой синхронизации 30 дней.
pub const THANKS_FILE: &str = "support-thanks-seen.json";
/// Сколько оплат помнит [`THANKS_FILE`] и сколько берётся из одного ответа.
const THANKS_KEEP: usize = 200;
const THANKS_PER_RESPONSE: usize = 10;
/// Когда получено каждое достижение на этом компьютере, по аккаунтам (только для показа).
pub const TIMES_FILE: &str = "achievement-times.json";
/// Файл прежнего переноса часов «до обновления»: перенос убран, файл удаляется при старте.
pub const HISTORY_MIGRATION_FILE: &str = "online-history-migration.json";
/// API speedrun.com: адрес страницы забега по его id (`data.weblink`).
pub const SPEEDRUN_RUNS_API: &str = "https://www.speedrun.com/api/v1/runs/";

const REQUEST_TIMEOUT_MS: u64 = 10_000;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const RUN_LINK_MAX_BYTES: usize = 512 * 1024;
const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_RUN_LINKS: usize = 500;
const MAX_FRIENDS: usize = 500;
const FRIENDS_BATCH: usize = 50;
const LOGIN_MAX_SECONDS: f64 = 900.0;
const LOGIN_MIN_SECONDS: f64 = 30.0;
const POLL_MIN_SECONDS: f64 = 3.0;
const POLL_RETRY_MS: f64 = 5000.0;

/// `TOKEN`: `^hlsr_[a-f0-9]{16}_[A-Za-z0-9_-]{43}$`.
static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^hlsr_[a-f0-9]{16}_[A-Za-z0-9_-]{43}$").expect("шаблон TOKEN"));
/// SteamID64: `^7656119\d{10}$`.
static STEAM_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^7656119[0-9]{10}$").expect("шаблон SteamID64"));
/// Старая ссылка на забег без игры: `^https://www.speedrun.com/runs/<id>$` (флаг `i`).
static LEGACY_RUN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i-u)^https://www\.speedrun\.com/runs/([a-z0-9]{8})$").expect("шаблон забега")
});
static USER_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z0-9]{4}-[A-Z0-9]{4}$").expect("шаблон кода"));
static DEVICE_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-f0-9]{64}$").expect("шаблон кода устройства"));

// --- Ошибки ---

/// Ошибка метода службы: `error.message` и `error.code` исключения 0.4.7 (код есть не у всех:
/// `RUNNER_STALE_RESPONSE`, `RUNNER_INVALID_*` и сбои транспорта — без кода, и каналы окна
/// отдают за них `RUNNER_NETWORK`). `cooldown` — тело отказа 429 (`RATE_COOLDOWN`/`LIKE_LIMIT`).
#[derive(Debug, Clone, PartialEq)]
pub struct RunnerError {
    pub message: String,
    pub code: Option<String>,
    pub cooldown: Option<Value>,
}

impl RunnerError {
    /// `Object.assign(new Error(code), {code})`.
    pub fn coded(code: impl Into<String>) -> RunnerError {
        let code = code.into();
        RunnerError {
            message: code.clone(),
            code: Some(code),
            cooldown: None,
        }
    }

    /// `new Error(message)` без кода.
    pub fn plain(message: impl Into<String>) -> RunnerError {
        RunnerError {
            message: message.into(),
            code: None,
            cooldown: None,
        }
    }

    /// `new URL(…)` не разобрал адрес: `TypeError`, код `ERR_INVALID_URL`.
    pub fn invalid_url() -> RunnerError {
        RunnerError {
            message: "Invalid URL".to_owned(),
            code: Some("ERR_INVALID_URL".to_owned()),
            cooldown: None,
        }
    }

    fn transport(error: TransportError) -> RunnerError {
        RunnerError {
            message: error.to_string(),
            code: error.code().map(str::to_owned),
            cooldown: None,
        }
    }

    /// Код для окна: `error.code || "RUNNER_NETWORK"` (`online-profile/ipc.js:40`).
    pub fn ipc_code(&self) -> &str {
        self.code
            .as_deref()
            .filter(|code| !code.is_empty())
            .unwrap_or("RUNNER_NETWORK")
    }
}

impl std::fmt::Display for RunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RunnerError {}

// --- Таймеры ---

/// Запущенный таймер (`setTimeout`/`setInterval`); `cancel` — `clearTimeout`/`clearInterval`
/// (сработавший или снятый — без последствий).
pub trait Timer: Send + Sync {
    fn cancel(&self);
}

/// Таймеры службы. По умолчанию — [`TokioTimers`]; тесты сверки подставляют свои.
pub trait Timers: Send + Sync {
    /// `setTimeout(task, delay)`.
    fn set_timeout(&self, delay_ms: f64, task: Box<dyn FnOnce() + Send>) -> Box<dyn Timer>;
    /// `setInterval(task, delay)`.
    fn set_interval(&self, delay_ms: f64, task: Arc<dyn Fn() + Send + Sync>) -> Box<dyn Timer>;
}

/// Задержка как у таймеров Node: не число, меньше 1 или больше 2³¹−1 — 1 мс.
pub fn node_delay(delay_ms: f64) -> Duration {
    let ms = if (1.0..=2_147_483_647.0).contains(&delay_ms) {
        delay_ms.trunc()
    } else {
        1.0
    };
    Duration::from_millis(ms as u64)
}

struct TaskTimer(Option<JoinHandle<()>>);

impl Timer for TaskTimer {
    fn cancel(&self) {
        if let Some(handle) = &self.0 {
            handle.abort();
        }
    }
}

/// Таймер, который отпустили (службу удалили, поле заменили), больше не срабатывает: без этого
/// задача tokio жила бы до конца работы и тикала впустую (`JoinHandle` при `Drop` задачу не
/// останавливает).
impl Drop for TaskTimer {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Таймеры на tokio; вне среды выполнения таймер не ставится (как `unref` — выход не держат).
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioTimers;

impl Timers for TokioTimers {
    fn set_timeout(&self, delay_ms: f64, task: Box<dyn FnOnce() + Send>) -> Box<dyn Timer> {
        let delay = node_delay(delay_ms);
        let handle = tokio::runtime::Handle::try_current().ok().map(|runtime| {
            runtime.spawn(async move {
                tokio::time::sleep(delay).await;
                task();
            })
        });
        Box::new(TaskTimer(handle))
    }

    fn set_interval(&self, delay_ms: f64, task: Arc<dyn Fn() + Send + Sync>) -> Box<dyn Timer> {
        let delay = node_delay(delay_ms);
        let handle = tokio::runtime::Handle::try_current().ok().map(|runtime| {
            runtime.spawn(async move {
                let mut ticks =
                    tokio::time::interval_at(tokio::time::Instant::now() + delay, delay);
                ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    ticks.tick().await;
                    task();
                }
            })
        });
        Box::new(TaskTimer(handle))
    }
}

// --- Настройки ---

/// Колбэк службы (`onChange`, `onUnlock`, …).
pub type Callback<T> = Arc<dyn Fn(T) + Send + Sync>;
/// `accountProvider()`: свой аккаунт Steam на этом компьютере.
pub type AccountProvider = Arc<dyn Fn() -> LocalAccount + Send + Sync>;
/// `friendProvider()`: друзья из локального Steam (запасной список).
pub type FriendProvider = Arc<dyn Fn() -> Vec<Value> + Send + Sync>;
/// `openExternal(url)`: открыть ссылку в браузере; ошибка — браузер не открылся.
pub type OpenExternal = Arc<dyn Fn(String) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;
/// `buildHeader()`: заголовок `X-HLSR-Build` или `None`.
pub type BuildHeader = Arc<dyn Fn() -> BoxFuture<'static, Option<String>> + Send + Sync>;
/// `ticketProvider(force)`: билет аттестации; ошибка — сервер признал сборку изменённой.
pub type TicketProvider =
    Arc<dyn Fn(bool) -> BoxFuture<'static, Result<Option<String>, ClientModified>> + Send + Sync>;
/// `crypto.randomBytes(n)`.
pub type RandomBytes = Arc<dyn Fn(usize) -> Vec<u8> + Send + Sync>;

/// `opts` конструктора 0.4.7.
pub struct OnlineProfileOptions {
    /// `userDataDir`: папка данных (`secrets.json`, `speedrun-links.json`, …).
    pub user_data_dir: PathBuf,
    /// Хранилище сессии входа (задача 8).
    pub session: SessionStore,
    /// `request`: транспорт API.
    pub transport: Arc<dyn Transport>,
    /// `apiBase`: по умолчанию [`config::API_BASE`].
    pub api_base: Option<String>,
    /// `launcherVersion`: по умолчанию `0.0.0`.
    pub launcher_version: Option<String>,
    pub account_provider: Option<AccountProvider>,
    pub friend_provider: Option<FriendProvider>,
    pub open_external: Option<OpenExternal>,
    /// `onChange(snapshot)` → событие окна `runner-profile-state`.
    pub on_change: Option<Callback<Value>>,
    /// `onAuthenticated(snapshot)` → показать окно и `runner-login-success`.
    pub on_authenticated: Option<Callback<Value>>,
    /// `onConsent(enabled)` → согласие телеметрии.
    pub on_consent: Option<Callback<bool>>,
    /// `onUnlock({id, title, icon})` → `achievement-unlocked`.
    pub on_unlock: Option<Callback<Value>>,
    /// `onModeration(notice)` → уведомление `MOD_*`.
    pub on_moderation: Option<Callback<Value>>,
    /// `onSupportThanks({id, kind, until, styles})` → уведомление `SUPPORT_THANKS` и событие окна.
    pub on_support_thanks: Option<Callback<Value>>,
    pub build_header: Option<BuildHeader>,
    pub ticket_provider: Option<TicketProvider>,
    /// `onClientModified()` → уведомление `CLIENT_MODIFIED` (один раз за запуск).
    pub on_client_modified: Option<Arc<dyn Fn() + Send + Sync>>,
    /// `now`.
    pub clock: Clock,
    pub timers: Arc<dyn Timers>,
    /// `crypto.randomBytes` для токена сессии игры.
    pub random_bytes: Option<RandomBytes>,
    /// `device` входа (`Windows`/`Linux` по ОС); тесты сверки, снятой на другой ОС, задают его.
    pub device: Option<String>,
    /// Пояс, по которому считаются дни «Недели в Мезе»: местный; тесты сверки с Node задают UTC.
    pub day_zone: DayZone,
}

impl OnlineProfileOptions {
    /// Умолчания 0.4.7: без колбэков, часы ОС, таймеры tokio.
    pub fn new(
        user_data_dir: impl Into<PathBuf>,
        session: SessionStore,
        transport: Arc<dyn Transport>,
    ) -> OnlineProfileOptions {
        OnlineProfileOptions {
            user_data_dir: user_data_dir.into(),
            session,
            transport,
            api_base: None,
            launcher_version: None,
            account_provider: None,
            friend_provider: None,
            open_external: None,
            on_change: None,
            on_authenticated: None,
            on_consent: None,
            on_unlock: None,
            on_moderation: None,
            on_support_thanks: None,
            build_header: None,
            ticket_provider: None,
            on_client_modified: None,
            clock: system_clock(),
            timers: Arc::new(TokioTimers),
            random_bytes: None,
            device: None,
            day_zone: DayZone::Local,
        }
    }
}

/// `buildHeader: () => buildIdentity.header()` (`AppManager.js:242`).
pub fn build_header_from(identity: BuildIdentity) -> BuildHeader {
    Arc::new(move || {
        let identity = identity.clone();
        Box::pin(async move { Some(identity.header().await) })
    })
}

/// `ticketProvider: (force) => attestation.ticket(force)` (`AppManager.js:243`).
pub fn ticket_provider_from(attestation: BuildAttestation) -> TicketProvider {
    Arc::new(move |force| {
        let attestation = attestation.clone();
        Box::pin(async move { attestation.ticket(force).await })
    })
}

/// `openExternal: (url) => shell.openExternal(url)` поверх порта [`Shell`].
pub fn open_external_via(shell: Arc<dyn Shell>) -> OpenExternal {
    Arc::new(move |url| {
        let opened = shell.open_external(&url);
        Box::pin(async move { opened })
    })
}

/// `onModeration` (`AppManager.js:258-267`): уведомление `MOD_*` с причиной; у бана и запрета
/// статуса — `meta.until` или `meta.forever`.
pub fn notify_moderation(
    store: &NotificationStore,
    notice: &Value,
) -> std::io::Result<Option<Value>> {
    let action = notice.get("action").and_then(Value::as_str).unwrap_or("");
    let code = moderation::code(action).unwrap_or("");
    let timed = action == "ban" || action == "mute";
    let meta = timed.then(
        || match notice.get("expiresAt").filter(|at| js::truthy(at)) {
            Some(until) => json!({ "until": until }),
            None => json!({ "forever": true }),
        },
    );
    let detail = notice.get("reason").and_then(Value::as_str);
    store.add(
        level_for_code(code).unwrap_or("null"),
        code,
        detail,
        meta.as_ref(),
    )
}

/// `onSupportThanks` (`AppManager.js` 0.4.9): уведомление `SUPPORT_THANKS`, у подписки —
/// `meta.until` (ISO без долей секунды).
pub fn notify_support_thanks(
    store: &NotificationStore,
    notice: &Value,
) -> std::io::Result<Option<Value>> {
    let meta = safe_integer(notice.get("until"))
        .filter(|until| *until != 0)
        .map(|until| json!({ "until": format!("{}Z", &iso_from_ms(until * 1000)[..19]) }));
    store.add("info", "SUPPORT_THANKS", None, meta.as_ref())
}

/// `onClientModified` (`AppManager.js:244-247`): строка журнала и уведомление `CLIENT_MODIFIED`.
pub fn notify_client_modified(
    store: &NotificationStore,
    logger: &dyn Logger,
) -> std::io::Result<Option<Value>> {
    // В 0.4.7 здесь было «app.asar изменён»; в 0.5 сборку подтверждает исполняемый файл
    // (`security::build_identity`), и текст только для журнала — на сервер и в окно он не уходит.
    logger.warn("[online] сервер не узнал сборку лаунчера: исполняемый файл изменён, вход отозван");
    store.add(
        level_for_code("CLIENT_MODIFIED").unwrap_or("null"),
        "CLIENT_MODIFIED",
        None,
        None,
    )
}

// --- Состояние ---

/// Ожидание входа: код устройства, код для пользователя, срок, согласие на телеметрию.
struct PendingLogin {
    device_code: Value,
    user_code: Value,
    expires_at: f64,
    consent: bool,
}

/// Сессия игры на сервере (`{token, sequence, started, busy, ending}`).
struct GameSession {
    token: String,
    sequence: u64,
    started: bool,
    busy: bool,
    ending: bool,
}

type SessionRef = Arc<Mutex<GameSession>>;
type Shared<T> = watch::Receiver<Option<Result<T, RunnerError>>>;

struct State {
    token: Option<String>,
    profile: Option<Value>,
    connected: bool,
    pending: Option<Arc<PendingLogin>>,
    last_error: Option<String>,
    poll_timer: Option<Box<dyn Timer>>,
    timer: Option<Box<dyn Timer>>,
    sync_in_flight: bool,
    /// `Map` 0.4.7: порядок вставки.
    active: Vec<(String, SessionRef)>,
    secrets: Vec<String>,
    run_links: Vec<(String, String)>,
    run_link_loads: HashMap<String, Shared<String>>,
    /// Конец очереди фоновых запросов ссылок забегов (`runLinkQueue`).
    run_link_queue: Option<watch::Receiver<bool>>,
    moderation_seen: Vec<i64>,
    moderation: Value,
    /// `thanksSeen`: id оплат из [`THANKS_FILE`].
    thanks_seen: Vec<String>,
    friends_cache: Option<(String, Value)>,
    friends_load: Option<Shared<Value>>,
    modified_noticed: bool,
    /// `achievement-times.json`: `{steamId64: {id: мс|null}}`.
    times: Map<String, Value>,
    /// День последней отметки серии (`activeDay`): до первой отметки фокус окна ничего не делает.
    active_day: Option<String>,
}

struct Inner {
    root: PathBuf,
    base: String,
    version: String,
    device: String,
    transport: Arc<dyn Transport>,
    session: SessionStore,
    account_provider: AccountProvider,
    friend_provider: FriendProvider,
    open_external: OpenExternal,
    on_change: Option<Callback<Value>>,
    on_authenticated: Option<Callback<Value>>,
    on_consent: Option<Callback<bool>>,
    on_unlock: Option<Callback<Value>>,
    on_moderation: Option<Callback<Value>>,
    on_support_thanks: Option<Callback<Value>>,
    build_header: BuildHeader,
    ticket_provider: TicketProvider,
    on_client_modified: Option<Arc<dyn Fn() + Send + Sync>>,
    clock: Clock,
    timers: Arc<dyn Timers>,
    random_bytes: RandomBytes,
    day_zone: DayZone,
    state: Mutex<State>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Действие при `Drop`: снимает флаг «в пути» и запись о загрузке, даже если будущее отбросили
/// посреди работы (отмена вызывающим, остановка среды выполнения, паника). В JS промис нельзя
/// отменить, поэтому в 0.4.7 флаг всегда доходил до `finally`.
struct Defer<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Defer<F> {
    fn new(action: F) -> Defer<F> {
        Defer(Some(action))
    }

    /// Работа закончилась штатно и флаг уже снят: `Drop` ничего не делает.
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl<F: FnOnce()> Drop for Defer<F> {
    fn drop(&mut self) {
        if let Some(action) = self.0.take() {
            action();
        }
    }
}

fn ready<T: Send + 'static>(value: T) -> BoxFuture<'static, T> {
    Box::pin(std::future::ready(value))
}

/// `readJson(file)`: JSON не больше 256 КиБ или `None`.
fn read_json(file: &Path) -> Option<Value> {
    if std::fs::metadata(file).ok()?.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(file).ok()?;
    json_parse(&String::from_utf8_lossy(&bytes)).ok()
}

/// `writeJson(file, data)`: компактно через `<файл>.tmp`, 0600.
fn write_json(file: &Path, data: &Value) {
    let _ = write_compact_tmp(
        file,
        data,
        TmpOptions {
            private: true,
            clear_stale: false,
        },
    );
}

/// Id оплаты в «Спасибо»: строка не длиннее 128 единиц UTF-16.
fn thanks_id(id: &str) -> bool {
    js::text::utf16_len(id) <= 128
}

/// `list.slice(-keep)`.
fn last<T>(mut list: Vec<T>, keep: usize) -> Vec<T> {
    list.drain(..list.len().saturating_sub(keep));
    list
}

/// `/^[a-z0-9]{8}$/i`.
fn is_run_id(id: &str) -> bool {
    id.len() == 8 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// `value || fallback` и `Number(…)` для чисел из ответа сервера.
fn number_or(value: Option<&Value>, fallback: f64) -> f64 {
    match value.filter(|value| js::truthy(value)) {
        Some(value) => to_number(Some(value)),
        None => fallback,
    }
}

/// `JSON.parse(body).error` (у объекта); `None` — не JSON или поля нет.
fn refusal(response: &Response) -> Option<Value> {
    let parsed = json_parse(&response.body).ok()?;
    parsed.as_object()?.get("error").cloned()
}

fn refused_with(response: &Response, code: &str) -> bool {
    refusal(response).as_ref().and_then(Value::as_str) == Some(code)
}

/// Тело ответа объектом (или `null`, если не JSON).
fn parsed_object(response: &Response) -> Option<Value> {
    json_parse(&response.body)
        .ok()
        .filter(|value| is_object(Some(value)))
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// `Object.assign({}, value)`: собственные поля объекта (у массива — индексы).
fn assign_copy(value: &Value) -> Map<String, Value> {
    entries(Some(value))
        .into_iter()
        .map(|(key, item)| (key, item.clone()))
        .collect()
}

async fn wait_shared<T: Clone>(mut receiver: Shared<T>) -> Result<T, RunnerError> {
    match receiver.wait_for(Option::is_some).await {
        Ok(value) => value
            .clone()
            .unwrap_or_else(|| Err(RunnerError::plain("RUNNER_NETWORK"))),
        // Задача загрузки не дожила до конца (среда выполнения останавливается).
        Err(_) => Err(RunnerError::plain("RUNNER_NETWORK")),
    }
}

/// Служба профиля; копии — одна служба.
#[derive(Clone)]
pub struct OnlineProfileService {
    inner: Arc<Inner>,
}

impl OnlineProfileService {
    pub fn new(options: OnlineProfileOptions) -> OnlineProfileService {
        let root = options.user_data_dir;
        // Файл прежнего переноса часов «до обновления»: перенос убран, файл не нужен.
        let _ = std::fs::remove_file(root.join(HISTORY_MIGRATION_FILE));
        // Достижения, открытые событием (и пасхалки): хранятся и здесь, чтобы не потеряться без
        // сервера и уйти на него при следующей синхронизации.
        let secrets: Vec<String> = read_json(&root.join(SECRETS_FILE))
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|id| achievements::UNLOCK_IDS.contains(id))
            .map(str::to_owned)
            .collect();
        // Найденные адреса страниц забегов (id → ссылка).
        let links = read_json(&root.join(RUN_LINKS_FILE));
        let run_links: Vec<(String, String)> = entries(links.as_ref())
            .into_iter()
            .filter(|(id, link)| {
                is_run_id(id) && link.as_str().is_some_and(|link| RUN_PAGE.is_match(link))
            })
            .map(|(id, link)| (id, link.as_str().unwrap_or("").to_owned()))
            .collect();
        // Записи модерации, о которых уже было уведомление.
        let moderation_seen: Vec<i64> = read_json(&root.join(MODERATION_FILE))
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|id| safe_integer(Some(id)))
            .filter(|id| *id > 0)
            .collect();
        let thanks: Vec<String> = read_json(&root.join(THANKS_FILE))
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|id| thanks_id(id))
            .map(str::to_owned)
            .collect();
        let thanks_seen = last(thanks, THANKS_KEEP);
        let (mut token, mut profile) = (None, None);
        if let Some(saved) = options.session.read()
            && TOKEN.is_match(&saved.token)
            && let Some(normalized) = profile_state::normalize(&saved.profile)
        {
            token = Some(saved.token);
            profile = Some(normalized);
        }
        // Записи аккаунта ещё нет — до 0.4.9 или до первого входа здесь время не хранилось.
        let times = achievements::read_times(
            read_json(&root.join(TIMES_FILE)).as_ref(),
            profile
                .as_ref()
                .and_then(|profile: &Value| profile["steamId64"].as_str()),
        );
        let device = options
            .device
            .unwrap_or_else(|| if cfg!(windows) { "Windows" } else { "Linux" }.to_owned());
        OnlineProfileService {
            inner: Arc::new(Inner {
                root,
                base: options
                    .api_base
                    .filter(|base| !base.is_empty())
                    .unwrap_or_else(|| config::API_BASE.to_owned()),
                version: options
                    .launcher_version
                    .filter(|version| !version.is_empty())
                    .unwrap_or_else(|| "0.0.0".to_owned()),
                device,
                transport: options.transport,
                session: options.session,
                account_provider: options
                    .account_provider
                    .unwrap_or_else(|| Arc::new(LocalAccount::default)),
                friend_provider: options
                    .friend_provider
                    .unwrap_or_else(|| Arc::new(Vec::new)),
                open_external: options
                    .open_external
                    .unwrap_or_else(|| Arc::new(|_| Box::pin(async { Ok(()) }))),
                on_change: options.on_change,
                on_authenticated: options.on_authenticated,
                on_consent: options.on_consent,
                on_unlock: options.on_unlock,
                on_moderation: options.on_moderation,
                on_support_thanks: options.on_support_thanks,
                build_header: options
                    .build_header
                    .unwrap_or_else(|| Arc::new(|| Box::pin(async { None }))),
                ticket_provider: options
                    .ticket_provider
                    .unwrap_or_else(|| Arc::new(|_| Box::pin(async { Ok(None) }))),
                on_client_modified: options.on_client_modified,
                clock: options.clock,
                timers: options.timers,
                random_bytes: options.random_bytes.unwrap_or_else(|| {
                    Arc::new(|size| {
                        let mut bytes = vec![0; size];
                        getrandom::fill(&mut bytes).expect("случайные байты ОС");
                        bytes
                    })
                }),
                day_zone: options.day_zone,
                state: Mutex::new(State {
                    token,
                    profile,
                    connected: false,
                    pending: None,
                    last_error: None,
                    poll_timer: None,
                    timer: None,
                    sync_in_flight: false,
                    active: Vec::new(),
                    secrets,
                    run_links,
                    run_link_loads: HashMap::new(),
                    run_link_queue: None,
                    moderation_seen,
                    moderation: moderation::normalize(&Value::Null),
                    thanks_seen,
                    friends_cache: None,
                    friends_load: None,
                    modified_noticed: false,
                    times,
                    active_day: None,
                }),
                tasks: Mutex::default(),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.inner.state)
    }

    fn now(&self) -> i64 {
        (self.inner.clock)()
    }

    fn file(&self, name: &str) -> PathBuf {
        self.inner.root.join(name)
    }

    /// Работа «в фоне» (`….catch(() => {})`); вне среды выполнения tokio — не выполняется.
    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let handle = runtime.spawn(task);
        let mut tasks = lock(&self.inner.tasks);
        tasks.retain(|task| !task.is_finished());
        tasks.push(handle);
    }

    fn spawn_result<T: Send + 'static>(&self, future: BoxFuture<'static, T>) {
        self.spawn(async move {
            let _ = future.await;
        });
    }

    /// Дождаться работы, начатой в фоне (завершение приложения, тесты).
    pub async fn settle(&self) {
        loop {
            let tasks: Vec<_> = std::mem::take(&mut *lock(&self.inner.tasks));
            if tasks.is_empty() {
                return;
            }
            for task in tasks {
                let _ = task.await;
            }
        }
    }

    /// `getAuthToken()`: токен входа (для телеметрии) или `None`.
    pub fn get_auth_token(&self) -> Option<String> {
        self.state().token.clone()
    }

    /// Профиль сервера есть (`onlineProfiles.profile`).
    pub fn has_profile(&self) -> bool {
        self.state().profile.is_some()
    }

    /// `active.has(gameId)`: у игры открыта сессия на сервере (или ждёт входа).
    pub fn is_game_active(&self, game_id: &str) -> bool {
        self.state().active.iter().any(|(id, _)| id == game_id)
    }

    /// Подменить токен в памяти, как `client.token = …` в самотестах 0.4.7.
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    #[doc(hidden)]
    pub fn set_token_for_tests(&self, token: Option<String>) {
        self.state().token = token;
    }

    // --- Снимок и события ---

    /// `snapshot()`: снимок для окна (`runner-profile-state`, `runner-state`, `profile-get`).
    pub fn snapshot(&self) -> Value {
        let account = (self.inner.account_provider)();
        let now = self.now() as f64;
        // Серию входов знает только этот компьютер: у чужих профилей её нет. Файл читается до
        // замка состояния.
        let streak =
            launch_streak::stats_in_zone(&self.inner.root, now as i64, self.inner.day_zone);
        let state = self.state();
        let presence = LocalPresence {
            online: state.connected,
            game_id: playing_game(&state.active),
        };
        let times = state
            .profile
            .as_ref()
            .and_then(|profile| profile["steamId64"].as_str())
            .and_then(|id| state.times.get(id));
        let mut out = profile_state::state(
            state.profile.as_ref(),
            Some(&account),
            state.connected,
            &state.secrets,
            Some(&presence),
            times,
            None,
        );
        out["streak"] = streak;
        let status = if state.pending.is_some() {
            "pending"
        } else if state.token.is_some() {
            "signed-in"
        } else {
            "signed-out"
        };
        let pending = state.pending.as_ref();
        let restriction = |key: &str| {
            let item = &state.moderation[key];
            if state.token.is_some() && moderation::active_at(item, now) {
                item.clone()
            } else {
                Value::Null
            }
        };
        let (ban, mute) = (restriction("ban"), restriction("mute"));
        out["status"] = json!(status);
        out["userCode"] = pending.map_or(Value::Null, |pending| pending.user_code.clone());
        out["expiresAt"] = pending
            .filter(|pending| pending.expires_at.is_finite())
            .map_or(Value::Null, |pending| {
                json!(iso_from_ms(pending.expires_at.trunc() as i64))
            });
        out["error"] = json!(state.last_error);
        out["ban"] = ban;
        out["mute"] = mute;
        out
    }

    /// `changed()`: снимок — в `onChange`.
    pub fn changed(&self) {
        let snapshot = self.snapshot();
        if let Some(on_change) = &self.inner.on_change {
            on_change(snapshot);
        }
    }

    /// Состояние модерации из ответа сервера; о новых записях — уведомление.
    fn accept_moderation(&self, raw: &Value) {
        let (fresh, seen) = {
            let mut state = self.state();
            state.moderation = moderation::normalize(raw);
            let fresh = moderation::unseen(&state.moderation, &state.moderation_seen);
            if fresh.is_empty() {
                return;
            }
            state.moderation_seen = moderation::remember(&state.moderation_seen, &fresh);
            (fresh, state.moderation_seen.clone())
        };
        // Повторное уведомление не страшно: сбой записи не мешает.
        write_json(&self.file(MODERATION_FILE), &json!(seen));
        if let Some(on_moderation) = &self.inner.on_moderation {
            for notice in fresh {
                on_moderation(notice);
            }
        }
    }

    /// `acceptSupportThanks(list)`: «Спасибо за поддержку» — по одному на каждую новую оплату
    /// (подписка или донат) из ответа сервера. Повторное окно не страшно: сбой записи не мешает,
    /// сбой колбэка не роняет синхронизацию.
    fn accept_support_thanks(&self, list: &[Value]) {
        let fresh: Vec<&Value> = {
            let mut state = self.state();
            // Повтор id в одном ответе — два окна и два id в файле, как в 0.4.9 (фикстура 049).
            let fresh: Vec<&Value> = list
                .iter()
                .filter(|item| {
                    let id = field(Some(item), "id").and_then(Value::as_str);
                    let kind = field(Some(item), "kind").and_then(Value::as_str);
                    id.is_some_and(|id| thanks_id(id) && !state.thanks_seen.iter().any(|s| s == id))
                        && matches!(kind, Some("subscription" | "donation"))
                })
                .collect();
            if fresh.is_empty() {
                return;
            }
            let mut seen = std::mem::take(&mut state.thanks_seen);
            seen.extend(
                fresh
                    .iter()
                    .map(|item| item["id"].as_str().unwrap_or("").to_owned()),
            );
            state.thanks_seen = last(seen, THANKS_KEEP);
            // Под замком: файл не отстаёт от памяти при одновременных ответах (как у времён).
            write_json(&self.file(THANKS_FILE), &json!(state.thanks_seen));
            fresh
        };
        let Some(on_support_thanks) = &self.inner.on_support_thanks else {
            return;
        };
        for item in fresh {
            on_support_thanks(json!({
                "id": item["id"],
                "kind": item["kind"],
                "until": profile_state::since(item.get("until")),
                "styles": item.get("styles") == Some(&Value::Bool(true)),
            }));
        }
    }

    /// `save()`: сессия в хранилище; не записалась — хранилище очищается.
    fn save(&self) {
        let session = {
            let state = self.state();
            match (&state.token, &state.profile) {
                (Some(token), Some(profile)) => Some(Session {
                    token: token.clone(),
                    profile: profile.clone(),
                }),
                _ => None,
            }
        };
        if let Some(session) = session
            && self.inner.session.write(&session).is_err()
        {
            self.inner.session.clear();
        }
    }

    /// `clearAuth(code)`: выйти из аккаунта в этом лаунчере.
    fn clear_auth(&self, code: Option<&str>) {
        {
            let mut state = self.state();
            state.token = None;
            state.profile = None;
            state.friends_cache = None;
            state.connected = false;
            state.moderation = moderation::normalize(&Value::Null);
        }
        self.inner.session.clear();
        self.state().last_error = code.map(str::to_owned);
        self.changed();
    }

    /// `clientModified()`: сервер признал сборку изменённой — выход и уведомление (одно за
    /// запуск). Возвращает ошибку `RUNNER_CLIENT_MODIFIED`.
    fn client_modified(&self) -> RunnerError {
        self.cancel_login();
        if self.state().token.is_some() {
            self.clear_auth(Some("RUNNER_CLIENT_MODIFIED"));
        } else {
            self.state().last_error = Some("RUNNER_CLIENT_MODIFIED".to_owned());
            self.changed();
        }
        let first = !std::mem::replace(&mut self.state().modified_noticed, true);
        if first && let Some(notice) = &self.inner.on_client_modified {
            notice();
        }
        RunnerError::coded("RUNNER_CLIENT_MODIFIED")
    }

    // --- Запросы ---

    /// `send(endpoint, body, headers, build, forceTicket)`: один запрос с билетом сборки.
    async fn send(
        &self,
        endpoint: &str,
        body: &Value,
        headers: &[(String, String)],
        build: bool,
        force_ticket: bool,
    ) -> Result<Response, RunnerError> {
        let mut sent = headers.to_vec();
        if build {
            match (self.inner.ticket_provider)(force_ticket).await {
                Err(ClientModified) => return Err(self.client_modified()),
                Ok(Some(ticket)) if !ticket.is_empty() => {
                    sent.push(("x-hlsr-ticket".to_owned(), ticket));
                }
                Ok(_) => {}
            }
        }
        let empty = json!({});
        let body = if js::truthy(body) { body } else { &empty };
        let options = RequestOptions {
            method: Some("POST".to_owned()),
            headers: sent,
            body: Some(hlsr_core::json_text::stringify(body, "")),
            timeout_ms: Some(REQUEST_TIMEOUT_MS),
            max_response_bytes: Some(MAX_RESPONSE_BYTES),
            launcher_version: Some(self.inner.version.clone()),
        };
        let url = format!("{}{endpoint}", self.inner.base);
        self.inner
            .transport
            .request(&url, &options)
            .await
            .map_err(RunnerError::transport)
    }

    /// `call(endpoint, body, authorized)`: запрос к API и разбор отказов (`:169-219`).
    /// `authorized` — с токеном входа (`Bearer`); без входа — `RUNNER_AUTH_REQUIRED`.
    pub fn call(
        &self,
        endpoint: &str,
        body: Value,
        authorized: bool,
    ) -> BoxFuture<'static, Result<Value, RunnerError>> {
        let mut headers: Vec<(String, String)> = Vec::new();
        let request_token = {
            let state = self.state();
            if authorized {
                let Some(token) = &state.token else {
                    return ready(Err(RunnerError::coded("RUNNER_AUTH_REQUIRED")));
                };
                headers.push(("authorization".to_owned(), format!("Bearer {token}")));
            }
            state.token.clone()
        };
        let build = (self.inner.build_header)();
        let this = self.clone();
        let endpoint = endpoint.to_owned();
        Box::pin(async move {
            let build = build.await.filter(|build| !build.is_empty());
            if let Some(build) = &build {
                headers.push(("x-hlsr-build".to_owned(), build.clone()));
            }
            let built = build.is_some();
            let mut response = this.send(&endpoint, &body, &headers, built, false).await?;
            // Билет сборки истёк или ещё не получен: один повтор с новым билетом.
            if response.status == 428 && built && refused_with(&response, "ATTEST_REQUIRED") {
                response = this.send(&endpoint, &body, &headers, built, true).await?;
                if response.status == 428 && refused_with(&response, "ATTEST_REQUIRED") {
                    return Err(RunnerError::coded("RUNNER_ATTEST_FAILED"));
                }
            }
            if authorized && request_token != this.state().token {
                return Err(RunnerError::plain("RUNNER_STALE_RESPONSE"));
            }
            if response.status == 401 && authorized {
                this.clear_auth(Some("RUNNER_SESSION_EXPIRED"));
            }
            // Бан и запрет статуса: сервер присылает их срок и причину.
            if response.status == 403 {
                let denied = parsed_object(&response);
                let error = field(denied.as_ref(), "error").and_then(Value::as_str);
                // Сервер не узнал сборку: вход устройства он уже отозвал.
                if error == Some("CLIENT_MODIFIED") {
                    return Err(this.client_modified());
                }
                if let Some(kind @ ("BANNED" | "MUTED")) = error {
                    let key = if kind == "BANNED" { "ban" } else { "mute" };
                    let mut raw = Map::new();
                    if let Some(item) = field(denied.as_ref(), key) {
                        raw.insert(key.to_owned(), item.clone());
                    }
                    let item = moderation::normalize(&Value::Object(raw))[key].clone();
                    this.state().moderation[key] = item;
                    this.changed();
                    return Err(RunnerError::coded(format!("RUNNER_{kind}")));
                }
            }
            if response.status == 429 {
                let limited = parsed_object(&response);
                if let Some(kind @ ("RATE_COOLDOWN" | "LIKE_LIMIT")) =
                    field(limited.as_ref(), "error").and_then(Value::as_str)
                {
                    let mut error = RunnerError::coded(format!("RUNNER_{kind}"));
                    error.cooldown = limited.clone();
                    return Err(error);
                }
            }
            // Версия ещё не в списке сервера или лаунчер слишком старый: онлайн недоступен, но
            // из аккаунта не выкидывает.
            if response.status == 503 || response.status == 426 {
                let refused = parsed_object(&response);
                if let Some(kind @ ("CLIENT_UNVERIFIED" | "CLIENT_OUTDATED")) =
                    field(refused.as_ref(), "error").and_then(Value::as_str)
                {
                    return Err(RunnerError::coded(format!("RUNNER_{kind}")));
                }
            }
            // Выбор заготовки подсветки (customize, stylePreset): права нет или заготовку скрыли;
            // SPEEDRUN_TAKEN — этот speedrun.com уже привязан к другому профилю HLSR.
            if response.status == 409
                && let Some(kind @ ("PERK_REQUIRED" | "PRESET_UNKNOWN" | "SPEEDRUN_TAKEN")) =
                    refusal(&response).as_ref().and_then(Value::as_str)
            {
                return Err(RunnerError::coded(format!("RUNNER_{kind}")));
            }
            if !(200..300).contains(&response.status) {
                return Err(RunnerError::coded(format!(
                    "RUNNER_HTTP_{}",
                    response.status
                )));
            }
            let data = json_parse(&response.body)
                .map_err(|error| RunnerError::plain(format!("SyntaxError: {error}")))?;
            if !is_object(Some(&data)) {
                return Err(RunnerError::plain("RUNNER_INVALID_RESPONSE"));
            }
            Ok(data)
        })
    }

    /// `accept(data)`: модерация и профиль из ответа; чужой профиль — `RUNNER_IDENTITY_MISMATCH`.
    fn accept(&self, data: &Value) -> Result<(), RunnerError> {
        if let Some(raw) = field(Some(data), "moderation").filter(|raw| js::truthy(raw)) {
            self.accept_moderation(raw);
        }
        if let Some(list) = field(Some(data), "supportThanks").and_then(Value::as_array) {
            self.accept_support_thanks(&list[..list.len().min(THANKS_PER_RESPONSE)]);
        }
        let Some(raw) = field(Some(data), "profile").filter(|raw| js::truthy(raw)) else {
            return Ok(());
        };
        let next = profile_state::normalize(raw);
        let mismatch = match (&next, &self.state().profile) {
            (None, _) => true,
            (Some(next), Some(profile)) => next["steamId64"] != profile["steamId64"],
            (Some(_), None) => false,
        };
        let Some(next) = next.filter(|_| !mismatch) else {
            return Err(RunnerError::plain("RUNNER_IDENTITY_MISMATCH"));
        };
        let pinned = next["details"]["pinnedRun"]["url"].clone();
        let has_pinned = js::truthy(&next["details"]["pinnedRun"]);
        {
            let mut state = self.state();
            state.profile = Some(next);
            state.connected = true;
            state.last_error = None;
        }
        self.save();
        self.note_times(None);
        self.changed();
        if has_pinned {
            self.warm_run_links(&[pinned]);
        }
        Ok(())
    }

    // --- Таймеры и вход ---

    /// `start()`: синхронизация и список друзей сразу, затем раз в 30 с — синхронизация и пульсы.
    pub fn start(&self) {
        if self.state().timer.is_some() {
            return;
        }
        // Список друзей подтягивается сразу после синхронизации: панель друзей при первом
        // открытии берёт его из памяти, а не ждёт Steam.
        let sync = self.sync();
        let this = self.clone();
        self.spawn(async move {
            sync.await;
            if this.state().token.is_some() {
                let _ = this.friends(false).await;
            }
        });
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        let timer = self.inner.timers.set_interval(
            SYNC_INTERVAL_MS,
            Arc::new(move || {
                if let Some(inner) = weak.upgrade() {
                    OnlineProfileService { inner }.tick();
                }
            }),
        );
        self.state().timer = Some(timer);
    }

    /// Тело таймера `start()`: синхронизация и пульсы всех сессий игры.
    pub fn tick(&self) {
        if self.state().token.is_none() {
            return;
        }
        let sync = self.sync();
        self.spawn(async move {
            sync.await;
        });
        for (id, session) in self.active_sessions() {
            let beat = self.beat_session(id, session, false);
            self.spawn_result(beat);
        }
    }

    /// `stop()`: таймер, ожидание входа; сессии игр закрываются (`session/end`).
    pub fn stop(&self) {
        let timer = self.state().timer.take();
        if let Some(timer) = timer {
            timer.cancel();
        }
        self.cancel_login();
        for (id, session) in self.active_sessions() {
            let beat = self.beat_session(id, session, true);
            self.spawn_result(beat);
        }
    }

    fn is_pending(&self, pending: &Arc<PendingLogin>) -> bool {
        self.state()
            .pending
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, pending))
    }

    /// `beginLogin(consent)`: код устройства, браузер на странице входа, опрос.
    pub fn begin_login(&self, consent: bool) -> BoxFuture<'static, Result<Value, RunnerError>> {
        self.cancel_login();
        let started = self.call("/v1/link/start", json!({}), false);
        let this = self.clone();
        Box::pin(async move {
            let started = started.await?;
            let verify = url::Url::parse(&coerced(field(Some(&started), "verifyUrl")))
                .map_err(|_| RunnerError::invalid_url())?;
            let base = url::Url::parse(&this.inner.base).map_err(|_| RunnerError::invalid_url())?;
            let user_code = field(Some(&started), "userCode").cloned();
            let device_code = field(Some(&started), "deviceCode").cloned();
            if verify.origin().ascii_serialization() != base.origin().ascii_serialization()
                || verify.path() != "/link"
                || !USER_CODE.is_match(&coerced(user_code.as_ref()))
                || !DEVICE_CODE.is_match(&coerced(device_code.as_ref()))
            {
                return Err(RunnerError::plain("RUNNER_INVALID_LOGIN_URL"));
            }
            let seconds = js::math::min(
                LOGIN_MAX_SECONDS,
                js::math::max(
                    LOGIN_MIN_SECONDS,
                    number_or(field(Some(&started), "expiresIn"), LOGIN_MAX_SECONDS),
                ),
            );
            let pending = Arc::new(PendingLogin {
                device_code: device_code.unwrap_or(Value::Null),
                user_code: user_code.unwrap_or(Value::Null),
                expires_at: this.now() as f64 + seconds * 1000.0,
                consent,
            });
            {
                let mut state = this.state();
                state.pending = Some(pending.clone());
                state.last_error = None;
            }
            this.changed();
            if (this.inner.open_external)(verify.as_str().to_owned())
                .await
                .is_err()
            {
                this.cancel_login();
                this.state().last_error = Some("RUNNER_BROWSER_FAILED".to_owned());
                this.changed();
                return Err(RunnerError::plain("RUNNER_BROWSER_FAILED"));
            }
            let interval = number_or(field(Some(&started), "interval"), POLL_MIN_SECONDS);
            this.schedule_poll(&pending, js::math::max(POLL_MIN_SECONDS, interval) * 1000.0);
            Ok(this.snapshot())
        })
    }

    fn schedule_poll(&self, pending: &Arc<PendingLogin>, delay_ms: f64) {
        if !self.is_pending(pending) {
            return;
        }
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        let target = pending.clone();
        let timer = self.inner.timers.set_timeout(
            delay_ms,
            Box::new(move || {
                if let Some(inner) = weak.upgrade() {
                    let service = OnlineProfileService { inner };
                    let poll = service.poll(target);
                    service.spawn(poll);
                }
            }),
        );
        self.state().poll_timer = Some(timer);
    }

    /// `poll(pending)`: один опрос входа.
    fn poll(&self, pending: Arc<PendingLogin>) -> BoxFuture<'static, ()> {
        if !self.is_pending(&pending) {
            return ready(());
        }
        if self.now() as f64 >= pending.expires_at {
            self.cancel_login();
            self.state().last_error = Some("RUNNER_LOGIN_EXPIRED".to_owned());
            self.changed();
            return ready(());
        }
        let body = json!({ "deviceCode": pending.device_code, "device": self.inner.device });
        let call = self.call("/v1/link/poll", body, false);
        let this = self.clone();
        Box::pin(async move {
            let outcome = this.poll_result(&pending, call).await;
            if outcome.is_err() && this.is_pending(&pending) {
                this.state().last_error = Some("RUNNER_NETWORK".to_owned());
                this.changed();
                this.schedule_poll(&pending, POLL_RETRY_MS);
            }
        })
    }

    async fn poll_result(
        &self,
        pending: &Arc<PendingLogin>,
        call: BoxFuture<'static, Result<Value, RunnerError>>,
    ) -> Result<(), RunnerError> {
        let result = call.await?;
        if !self.is_pending(pending) {
            return Ok(());
        }
        match field(Some(&result), "state").and_then(Value::as_str) {
            Some("ready") => {
                let steam_id = field(Some(&result), "steamId64")
                    .cloned()
                    .unwrap_or(Value::Null);
                let token = field(Some(&result), "token")
                    .and_then(Value::as_str)
                    .filter(|token| TOKEN.is_match(token));
                let (Some(token), true) = (token, STEAM_ID.is_match(&coerced(Some(&steam_id))))
                else {
                    return Err(RunnerError::plain("RUNNER_INVALID_LOGIN_RESPONSE"));
                };
                let profile = profile_state::normalize(
                    field(Some(&result), "profile").unwrap_or(&Value::Null),
                )
                .or_else(|| {
                    profile_state::normalize(&json!({
                        "steamId64": steam_id, "personaName": "", "games": {}, "details": {},
                    }))
                });
                let same = profile
                    .as_ref()
                    .is_some_and(|profile| profile["steamId64"] == steam_id);
                {
                    let mut state = self.state();
                    state.token = Some(token.to_owned());
                    state.profile = profile;
                }
                if !same {
                    return Err(RunnerError::plain("RUNNER_IDENTITY_MISMATCH"));
                }
                self.save();
                self.cancel_login();
                self.sync().await;
                if self.state().token.is_none() {
                    return Ok(());
                }
                if let Some(on_consent) = &self.inner.on_consent {
                    on_consent(pending.consent);
                }
                let snapshot = self.snapshot();
                if let Some(on_authenticated) = &self.inner.on_authenticated {
                    on_authenticated(snapshot);
                }
                for (id, session) in self.active_sessions() {
                    let beat = self.beat_session(id, session, false);
                    self.spawn_result(beat);
                }
            }
            Some("expired") => {
                self.cancel_login();
                self.state().last_error = Some("RUNNER_LOGIN_EXPIRED".to_owned());
                self.changed();
            }
            _ => {
                let interval = number_or(field(Some(&result), "interval"), POLL_MIN_SECONDS);
                self.schedule_poll(pending, js::math::max(POLL_MIN_SECONDS, interval) * 1000.0);
            }
        }
        Ok(())
    }

    /// `cancelLogin()`: снять ожидание входа и таймер опроса.
    pub fn cancel_login(&self) {
        let timer = {
            let mut state = self.state();
            state.pending = None;
            state.poll_timer.take()
        };
        if let Some(timer) = timer {
            timer.cancel();
        }
    }

    /// `sync()`: профиль с сервера и неотправленные достижения; итог — снимок.
    pub fn sync(&self) -> BoxFuture<'static, Value> {
        {
            let mut state = self.state();
            if state.token.is_none() || state.sync_in_flight {
                drop(state);
                return ready(self.snapshot());
            }
            state.sync_in_flight = true;
        }
        // Флаг снимется и у отброшенного будущего (вызов не опрашивали или отменили).
        let mut in_flight = {
            let this = self.clone();
            Defer::new(move || this.state().sync_in_flight = false)
        };
        let local = (self.inner.account_provider)();
        let persona = {
            let state = self.state();
            let profile = state.profile.as_ref();
            let profile_id = profile.and_then(|profile| profile["steamId64"].as_str());
            if profile.is_some()
                && local.steam_id64.is_some()
                && local.steam_id64.as_deref() == profile_id
            {
                local.persona_name.clone()
            } else {
                profile.and_then(|profile| profile["personaName"].as_str().map(str::to_owned))
            }
        };
        let body = json!({ "personaName": persona.unwrap_or_default(), "launcherVersion": self.inner.version });
        let call = self.call("/v2/runner/sync", body, true);
        let this = self.clone();
        Box::pin(async move {
            let outcome: Result<(), RunnerError> = async {
                this.accept(&call.await?)?;
                // Достижение, которого сервер ещё не знает, не роняет синхронизацию: отказ по
                // одному не мешает остальным, обрыв сети — ждёт.
                for id in this.unsynced_secrets() {
                    let sent = this.call("/v2/runner/secret", json!({ "id": id }), true);
                    let outcome = match sent.await {
                        Ok(data) => this.accept(&data),
                        Err(error) => Err(error),
                    };
                    if let Err(error) = outcome
                        && !error
                            .code
                            .as_deref()
                            .unwrap_or("")
                            .starts_with("RUNNER_HTTP_4")
                    {
                        break;
                    }
                }
                Ok(())
            }
            .await;
            if outcome.is_err() {
                {
                    let mut state = this.state();
                    state.connected = false;
                    if state.token.is_some() {
                        state.last_error = Some("RUNNER_NETWORK".to_owned());
                    }
                }
                this.changed();
            }
            this.state().sync_in_flight = false;
            in_flight.disarm();
            this.snapshot()
        })
    }

    /// `noteActiveDay()`: день для серий (7, 14, 30, 100 дней) — при старте лаунчера и при запуске
    /// игры.
    pub fn note_active_day(&self) {
        let (now, zone) = (self.now(), self.inner.day_zone);
        self.state().active_day = Some(launch_streak::day_of_in_zone(now, zone));
        let days = launch_streak::record_in_zone(&self.inner.root, now, zone);
        for (id, goal) in launch_streak::GOALS {
            if days >= goal {
                self.achieve(id);
            }
        }
    }

    /// `noteNewDay()`: по фокусу окна. Лаунчер открыт со вчера — новый день засчитывается без
    /// перезапуска; в тот же день и до первой отметки при старте ничего не делает.
    pub fn note_new_day(&self) {
        let today = launch_streak::day_of_in_zone(self.now(), self.inner.day_zone);
        let active = self.state().active_day.clone();
        if active.is_some_and(|day| day != today) {
            self.note_active_day();
        }
    }

    // --- Сессии игры ---

    fn active_sessions(&self) -> Vec<(String, SessionRef)> {
        self.state().active.clone()
    }

    fn active_session(&self, id: &str) -> Option<SessionRef> {
        self.state()
            .active
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, session)| session.clone())
    }

    fn remove_active(&self, id: &str) {
        self.state().active.retain(|(key, _)| key != id);
    }

    /// `gameStart(id)`: игра запущена — сессия на сервере.
    pub fn game_start(&self, id: &str) {
        self.note_active_day();
        if !profile_state::GAMES.contains(&id) || self.is_game_active(id) {
            return;
        }
        let token = js::text::hex(&(self.inner.random_bytes)(32));
        let session = Arc::new(Mutex::new(GameSession {
            token,
            sequence: 0,
            started: false,
            busy: false,
            ending: false,
        }));
        self.state().active.push((id.to_owned(), session.clone()));
        self.changed();
        let beat = self.beat_session(id.to_owned(), session, false);
        self.spawn_result(beat);
    }

    /// `gameEnd(id)`: игра закрылась — сессия закрывается.
    pub fn game_end(&self, id: &str) {
        let Some(session) = self.active_session(id) else {
            return;
        };
        lock(&session).ending = true;
        self.changed();
        let beat = self.beat_session(id.to_owned(), session, true);
        self.spawn_result(beat);
    }

    /// `beat(id, session, end)` для открытой сессии игры `id` (самотесты 0.4.7 зовут его сами).
    pub fn beat(&self, id: &str, end: bool) -> BoxFuture<'static, Result<(), RunnerError>> {
        match self.active_session(id) {
            Some(session) => self.beat_session(id.to_owned(), session, end),
            None => ready(Ok(())),
        }
    }

    fn session_call(
        &self,
        session: &SessionRef,
        ending: bool,
    ) -> BoxFuture<'static, Result<Value, RunnerError>> {
        let (token, sequence) = {
            let session = lock(session);
            (session.token.clone(), session.sequence)
        };
        let endpoint = if ending {
            "/v2/runner/session/end"
        } else {
            "/v2/runner/session/heartbeat"
        };
        self.call(
            endpoint,
            json!({ "sessionToken": token, "sequence": sequence }),
            true,
        )
    }

    fn beat_session(
        &self,
        id: String,
        session: SessionRef,
        end: bool,
    ) -> BoxFuture<'static, Result<(), RunnerError>> {
        if self.state().token.is_none() {
            if end {
                self.remove_active(&id);
            }
            return ready(Ok(()));
        }
        let (first, mut busy) = {
            let mut guard = lock(&session);
            if guard.busy {
                if end {
                    guard.ending = true;
                }
                return ready(Ok(()));
            }
            guard.busy = true;
            // Запрос в пути: если будущее отбросят, следующий пульс не должен остаться без хода.
            let busy = {
                let session = session.clone();
                Defer::new(move || lock(&session).busy = false)
            };
            if guard.started {
                // Решение «конец или пульс» фиксируется до запроса: если игра закрылась, пока
                // пульс был в пути, сессию закрывает отдельный запрос в конце.
                let ending = end || guard.ending;
                let pulse = ending || guard.sequence > 0;
                guard.sequence += 1;
                drop(guard);
                (
                    First::Started(ending, pulse.then(|| self.session_call(&session, ending))),
                    busy,
                )
            } else {
                let body = json!({ "gameId": id, "sessionToken": guard.token });
                drop(guard);
                (
                    First::Start(self.call("/v2/runner/session/start", body, true)),
                    busy,
                )
            }
        };
        let this = self.clone();
        Box::pin(async move {
            let outcome = this.beat_body(&id, &session, end, first).await;
            let ending = {
                let mut guard = lock(&session);
                guard.busy = false;
                guard.ending
            };
            busy.disarm();
            if ending && this.is_game_active(&id) {
                this.remove_active(&id);
                lock(&session).sequence += 1;
                let close = this.session_call(&session, true);
                let accept = this.clone();
                this.spawn(async move {
                    if let Ok(data) = close.await {
                        let _ = accept.accept(&data);
                    }
                });
            }
            outcome
        })
    }

    async fn beat_body(
        &self,
        id: &str,
        session: &SessionRef,
        end: bool,
        first: First,
    ) -> Result<(), RunnerError> {
        let (ending, pulse) = match first {
            First::Start(start) => {
                self.accept(&start.await?)?;
                let (ending, pulse) = {
                    let mut guard = lock(session);
                    guard.started = true;
                    let ending = end || guard.ending;
                    let pulse = ending || guard.sequence > 0;
                    guard.sequence += 1;
                    (ending, pulse)
                };
                (ending, pulse.then(|| self.session_call(session, ending)))
            }
            First::Started(ending, pulse) => (ending, pulse),
        };
        if let Some(pulse) = pulse {
            self.accept(&pulse.await?)?;
        }
        if ending {
            self.remove_active(id);
        }
        Ok(())
    }

    // --- Действия и достижения ---

    /// `action(action, body)`: действие окна на сервере; итог — ответ и снимок.
    pub fn action(
        &self,
        action: &str,
        body: Option<&Value>,
    ) -> BoxFuture<'static, Result<Value, RunnerError>> {
        if !ACTIONS.contains(&action) {
            return ready(Err(RunnerError::plain("RUNNER_UNKNOWN_ACTION")));
        }
        let body = body
            .filter(|body| js::truthy(body))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let call = self.call(&format!("/v2/runner/{action}"), body, true);
        let customize = action == "customize";
        let this = self.clone();
        Box::pin(async move {
            let result = call.await?;
            this.accept(&result)?;
            let korea = this
                .state()
                .profile
                .as_ref()
                .is_some_and(|profile| profile["details"]["country"] == "KP");
            if customize && korea {
                this.achieve("impossible");
            }
            if let Some(runs) = field(Some(&result), "runs").and_then(Value::as_array) {
                let urls: Vec<Value> = runs
                    .iter()
                    .map(|run| field(Some(run), "url").cloned().unwrap_or(Value::Null))
                    .collect();
                this.warm_run_links(&urls);
            }
            let mut out = assign_copy(&result);
            out.insert("snapshot".to_owned(), this.snapshot());
            Ok(Value::Object(out))
        })
    }

    /// `noteTimes(fresh)`: время получения — после каждого ответа сервера (новая статистика) и при
    /// unlock; `fresh` — то, что открыто только что. Не записалось — останется до перезапуска.
    fn note_times(&self, fresh: Option<&str>) {
        let now = self.now();
        let mut state = self.state();
        let Some(profile) = &state.profile else {
            return;
        };
        let account = profile["steamId64"].as_str().unwrap_or("").to_owned();
        let mut unlocked = string_list(Some(&profile["details"]["secrets"]));
        unlocked.extend(state.secrets.iter().cloned());
        let unlocked: Vec<&str> = unlocked.iter().map(String::as_str).collect();
        let mine = state.times.get(&account).and_then(Value::as_object);
        let next = achievements::stamp(mine, &profile["games"], &unlocked, fresh, now);
        if mine.is_some_and(|mine| mine.len() == next.len()) {
            return;
        }
        state.times.insert(account, Value::Object(next));
        // Под замком: два вызова из разных потоков не запишут старый снимок последним.
        write_json(&self.file(TIMES_FILE), &Value::Object(state.times.clone()));
    }

    fn unsynced_secrets(&self) -> Vec<String> {
        let state = self.state();
        let known = string_list(
            state
                .profile
                .as_ref()
                .map(|profile| &profile["details"]["secrets"]),
        );
        state
            .secrets
            .iter()
            .filter(|id| !known.contains(id))
            .cloned()
            .collect()
    }

    /// `unlock(id)`: открыть достижение события. `unlocked: true` — только в первый раз (тогда
    /// `onUnlock` просит окно показать поздравление). Без входа ничего не открывается.
    pub fn unlock(&self, id: &str) -> BoxFuture<'static, Result<Value, RunnerError>> {
        if !achievements::UNLOCK_IDS.contains(&id) {
            return ready(Err(RunnerError::plain("RUNNER_UNKNOWN_SECRET")));
        }
        let (unlocked, added) = {
            let mut state = self.state();
            let Some(profile) = state.profile.as_ref().filter(|_| state.token.is_some()) else {
                drop(state);
                return ready(Ok(
                    json!({ "unlocked": false, "snapshot": self.snapshot() }),
                ));
            };
            let known = string_list(Some(&profile["details"]["secrets"]));
            let mine = state.secrets.iter().any(|secret| secret == id);
            let unlocked = !mine && !known.iter().any(|secret| secret == id);
            if !mine {
                state.secrets.push(id.to_owned());
            }
            (unlocked, (!mine).then(|| state.secrets.clone()))
        };
        if let Some(secrets) = added {
            // Не записалось — останется до перезапуска.
            write_json(&self.file(SECRETS_FILE), &json!(secrets));
            self.note_times(unlocked.then_some(id));
            self.changed();
        }
        if unlocked
            && let (Some(on_unlock), Some(item)) = (&self.inner.on_unlock, achievements::find(id))
        {
            on_unlock(json!({ "id": id, "title": item.title, "icon": item.icon }));
        }
        let signed_in = self.state().token.is_some();
        let send = signed_in && self.unsynced_secrets().iter().any(|secret| secret == id);
        let call = send.then(|| self.call("/v2/runner/secret", json!({ "id": id }), true));
        let this = self.clone();
        Box::pin(async move {
            if let Some(call) = call
                && let Ok(data) = call.await
            {
                // Не дошло — дойдёт при синхронизации.
                let _ = this.accept(&data);
            }
            Ok(json!({ "unlocked": unlocked, "snapshot": this.snapshot() }))
        })
    }

    /// `unlockFromRenderer(id)`: из окна — только то, что открывает само окно.
    pub fn unlock_from_renderer(
        &self,
        id: Option<&Value>,
    ) -> BoxFuture<'static, Result<Value, RunnerError>> {
        match id.and_then(Value::as_str) {
            Some(id) if achievements::RENDERER_UNLOCK_IDS.contains(&id) => self.unlock(id),
            _ => ready(Err(RunnerError::plain("RUNNER_UNKNOWN_SECRET"))),
        }
    }

    /// `achieve(id)`: для событий main — ошибки не мешают тому, что их вызвало.
    pub fn achieve(&self, id: &str) {
        let unlock = self.unlock(id);
        self.spawn_result(unlock);
    }

    // --- Ссылки забегов ---

    /// `speedrunRunLink(id)`: адрес страницы забега по его id (`data.weblink` API speedrun.com).
    /// Найденный запоминается; одновременные запросы одного id ждут один ответ.
    pub fn speedrun_run_link(&self, id: &str) -> BoxFuture<'static, Result<String, RunnerError>> {
        if !is_run_id(id) {
            return ready(Err(RunnerError::plain("RUNNER_INVALID_URL")));
        }
        let mut state = self.state();
        if let Some((_, link)) = state.run_links.iter().find(|(key, _)| key == id) {
            return ready(Ok(link.clone()));
        }
        let receiver = match state.run_link_loads.get(id) {
            Some(receiver) => receiver.clone(),
            None => {
                let (sender, receiver) = watch::channel(None);
                state.run_link_loads.insert(id.to_owned(), receiver.clone());
                drop(state);
                let fetch = self.fetch_run_link(id.to_owned());
                let this = self.clone();
                let id = id.to_owned();
                // Запись о загрузке уходит и тогда, когда задачу отбросили, не доведя до конца
                // (в том числе не начав: вне среды tokio фоновая работа не запускается).
                let mut loading = {
                    let (this, id) = (this.clone(), id.clone());
                    Defer::new(move || {
                        this.state().run_link_loads.remove(&id);
                    })
                };
                self.spawn(async move {
                    let result = fetch.await;
                    if let Ok(link) = &result {
                        this.remember_run_link(&id, link);
                    }
                    this.state().run_link_loads.remove(&id);
                    loading.disarm();
                    let _ = sender.send(Some(result));
                });
                return Box::pin(wait_shared(receiver));
            }
        };
        drop(state);
        Box::pin(wait_shared(receiver))
    }

    fn remember_run_link(&self, id: &str, link: &str) {
        let links = {
            let mut state = self.state();
            match state.run_links.iter_mut().find(|(key, _)| key == id) {
                Some(entry) => entry.1 = link.to_owned(),
                None => state.run_links.push((id.to_owned(), link.to_owned())),
            }
            let excess = state.run_links.len().saturating_sub(MAX_RUN_LINKS);
            state.run_links.drain(..excess);
            state
                .run_links
                .iter()
                .map(|(key, link)| (key.clone(), json!(link)))
                .collect::<Map<_, _>>()
        };
        // Не записалось — останется в памяти.
        write_json(&self.file(RUN_LINKS_FILE), &Value::Object(links));
    }

    fn fetch_run_link(&self, id: String) -> BoxFuture<'static, Result<String, RunnerError>> {
        let transport = self.inner.transport.clone();
        let version = self.inner.version.clone();
        Box::pin(async move {
            let options = RequestOptions {
                method: Some("GET".to_owned()),
                timeout_ms: Some(REQUEST_TIMEOUT_MS),
                max_response_bytes: Some(RUN_LINK_MAX_BYTES),
                launcher_version: Some(version),
                ..RequestOptions::default()
            };
            let url = format!("{SPEEDRUN_RUNS_API}{id}");
            let response = transport
                .request(&url, &options)
                .await
                .map_err(RunnerError::transport)?;
            if !(200..300).contains(&response.status) {
                return Err(RunnerError::plain(format!(
                    "RUNNER_HTTP_{}",
                    response.status
                )));
            }
            let parsed = json_parse(&response.body)
                .map_err(|error| RunnerError::plain(format!("SyntaxError: {error}")))?;
            let data = field(Some(&parsed), "data")
                .filter(|data| !data.is_null())
                .ok_or_else(|| RunnerError::plain("TypeError: data"))?;
            let link = field(Some(data), "weblink").and_then(Value::as_str);
            let matched = link.and_then(|link| RUN_PAGE.captures(link));
            match (link, matched) {
                (Some(link), Some(found)) if found[1] == *id => Ok(link.to_owned()),
                _ => Err(RunnerError::plain("RUNNER_INVALID_URL")),
            }
        })
    }

    /// `warmRunLinks(urls)`: адреса старых ссылок (`/runs/<id>`) узнаются в фоне по одному, чтобы
    /// к нажатию они уже были известны.
    fn warm_run_links(&self, urls: &[Value]) {
        let ids: Vec<String> = {
            let state = self.state();
            urls.iter()
                .filter_map(Value::as_str)
                .filter_map(|url| LEGACY_RUN.captures(url).map(|found| found[1].to_owned()))
                .filter(|id| !state.run_links.iter().any(|(key, _)| key == id))
                .collect()
        };
        if ids.is_empty() {
            return;
        }
        let (sender, receiver) = watch::channel(false);
        let previous = self.state().run_link_queue.replace(receiver);
        let this = self.clone();
        self.spawn(async move {
            if let Some(mut previous) = previous {
                let _ = previous.wait_for(|done| *done).await;
            }
            for id in ids {
                let _ = this.speedrun_run_link(&id).await;
            }
            let _ = sender.send(true);
        });
    }

    // --- Игроки, поиск, профили, оценки, друзья ---

    /// `search(query)`: поиск профилей; свой профиль в выдаче — «Нашёл себя».
    pub fn search(&self, query: Option<&Value>) -> BoxFuture<'static, Result<Value, RunnerError>> {
        let Some(query) = query
            .and_then(Value::as_str)
            .filter(|query| js::text::utf16_len(query) <= 64)
        else {
            return ready(Ok(json!([])));
        };
        let call = self.call("/v2/runner/search", json!({ "query": query }), true);
        let this = self.clone();
        Box::pin(async move {
            let data = call.await?;
            let profiles: Vec<Value> = field(Some(&data), "profiles")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(profile_state::normalize)
                .collect();
            let own = this
                .state()
                .profile
                .as_ref()
                .map(|profile| profile["steamId64"].clone());
            if let Some(own) = own
                && profiles.iter().any(|profile| profile["steamId64"] == own)
            {
                this.achieve("found-self");
            }
            let developer = json!("developer");
            if profiles.iter().any(|profile| {
                profile["badges"]
                    .as_array()
                    .is_some_and(|badges| badges.contains(&developer))
            }) {
                this.achieve("meet-the-maker");
            }
            Ok(Value::Array(profiles))
        })
    }

    /// `players(query)`: кто в сети (`view: "online"`) и рейтинг по `sort` (для часов и
    /// запусков — по игре `game`), страницами по 50 до 500-го места.
    pub fn players(&self, query: Option<&Value>) -> BoxFuture<'static, Result<Value, RunnerError>> {
        let source = query.filter(|query| is_object(Some(query)));
        {
            let state = self.state();
            if state.token.is_none() || state.profile.is_none() {
                return ready(Ok(json!({ "status": "ok", "signedIn": false })));
            }
        }
        let list = |raw: Option<&Value>| -> Value {
            Value::Array(
                raw.and_then(Value::as_array)
                    .map_or(&[][..], |items| &items[..items.len().min(PLAYERS_PAGE * 2)])
                    .iter()
                    .filter_map(profile_state::list_entry)
                    .collect(),
            )
        };
        let count = |value: Option<&Value>| safe_integer(value).filter(|x| *x >= 0).unwrap_or(0);
        if field(source, "view").and_then(Value::as_str) == Some("online") {
            let call = self.call("/v2/runner/players", json!({ "view": "online" }), true);
            return Box::pin(async move {
                let data = call.await?;
                let online = field(Some(&data), "online").filter(|online| js::truthy(online));
                Ok(json!({
                    "status": "ok", "signedIn": true, "view": "online",
                    "total": count(field(online, "total")), "playing": count(field(online, "playing")),
                    "profiles": list(field(online, "profiles")),
                }))
            });
        }
        let offset = field(source, "offset").cloned().unwrap_or(json!(0));
        let sort = field(source, "sort")
            .and_then(Value::as_str)
            .filter(|sort| PLAYERS_SORTS.contains(sort));
        let game = field(source, "game").filter(|game| js::truthy(game));
        let game_ok = game.is_none_or(|game| {
            game.as_str()
                .is_some_and(|game| profile_state::GAMES.contains(&game))
        });
        let offset_ok = safe_integer(Some(&offset))
            .is_some_and(|x| (0..PLAYERS_DEPTH).contains(&x) && x % PLAYERS_PAGE as i64 == 0);
        let (Some(sort), true, true) = (sort, game_ok, offset_ok) else {
            return ready(Err(RunnerError::plain("RUNNER_INVALID_QUERY")));
        };
        let body = json!({
            "view": "top", "sort": sort, "game": game.cloned().unwrap_or(json!("")), "offset": offset,
        });
        let call = self.call("/v2/runner/players", body, true);
        let sort = sort.to_owned();
        Box::pin(async move {
            let data = call.await?;
            let top = field(Some(&data), "top").filter(|top| js::truthy(top));
            let game = field(top, "game")
                .and_then(Value::as_str)
                .filter(|game| profile_state::GAMES.contains(game));
            Ok(json!({
                "status": "ok", "signedIn": true, "view": "top", "sort": sort, "game": game,
                "offset": offset, "more": field(top, "more") == Some(&Value::Bool(true)),
                "profiles": list(field(top, "profiles")),
            }))
        })
    }

    /// `view(id)`: чужой (или свой) профиль; чужой — «Разведка».
    pub fn view(&self, id: Option<&Value>) -> BoxFuture<'static, Result<Value, RunnerError>> {
        if !STEAM_ID.is_match(&coerced(id)) {
            return ready(Err(RunnerError::plain("RUNNER_INVALID_ID")));
        }
        let id = id.cloned().unwrap_or(Value::Null);
        let call = self.call("/v2/runner/profile", json!({ "steamId64": id }), true);
        let this = self.clone();
        Box::pin(async move {
            let data = call.await?;
            let profile =
                profile_state::normalize(field(Some(&data), "profile").unwrap_or(&Value::Null));
            if let Some(profile) = &profile
                && js::truthy(&profile["details"]["pinnedRun"])
            {
                this.warm_run_links(&[profile["details"]["pinnedRun"]["url"].clone()]);
            }
            let own = this
                .state()
                .profile
                .as_ref()
                .map(|own| own["steamId64"].clone());
            if let (Some(profile), Some(own)) = (&profile, own)
                && profile["steamId64"] != own
            {
                this.achieve("scout");
            }
            let Some(profile) = profile else {
                return Ok(json!({ "missing": true, "steamId64": id }));
            };
            let mine = {
                let state = this.state();
                let mut mine =
                    string_list(state.profile.as_ref().map(|own| &own["details"]["secrets"]));
                mine.extend(state.secrets.iter().cloned());
                mine
            };
            // `telegram`, `discordLink`, `perks`, `stylePreset` чужого — как прислал сервер, без
            // среза: так и в 0.4.9 (фикстура 049, шаг «чужой профиль»).
            let mut out = profile_state::state(
                Some(&profile),
                Some(&LocalAccount::default()),
                true,
                &[],
                None,
                None,
                Some(&mine),
            );
            // Список полученного чужим окну не нужен: замки уже в achievements, а id открыли бы,
            // какие секретные у него есть.
            if let Some(details) = out["details"].as_object_mut() {
                details.shift_remove("secrets");
            }
            out["vote"] = profile_state::vote(field(Some(&data), "vote").unwrap_or(&Value::Null));
            Ok(out)
        })
    }

    /// `rate(id, value)`: лайк (1), дизлайк (−1) или снять голос (0). Пауза 4 часа и лимит
    /// лайков сервер отдаёт отказом 429 — тогда итог с текущим голосом.
    pub fn rate(
        &self,
        id: Option<&Value>,
        value: Option<&Value>,
    ) -> BoxFuture<'static, Result<Value, RunnerError>> {
        let valid = value
            .filter(|value| value.is_number())
            .and_then(Value::as_f64)
            .is_some_and(|value| value == 1.0 || value == -1.0 || value == 0.0);
        if !STEAM_ID.is_match(&coerced(id)) || !valid {
            return ready(Err(RunnerError::plain("RUNNER_INVALID_ID")));
        }
        let id = id.cloned().unwrap_or(Value::Null);
        let own = self
            .state()
            .profile
            .as_ref()
            .is_some_and(|profile| profile["steamId64"] == id);
        if own {
            return ready(Err(RunnerError::coded("RUNNER_SELF_VOTE")));
        }
        let value = value.cloned().unwrap_or(Value::Null);
        let call = self.call(
            "/v2/runner/rate",
            json!({ "steamId64": id, "value": value }),
            true,
        );
        Box::pin(async move {
            let null = Value::Null;
            match call.await {
                Ok(data) => Ok(json!({
                    "status": "ok",
                    "rating": profile_state::rating(field(Some(&data), "rating").unwrap_or(&null)),
                    "vote": profile_state::vote(field(Some(&data), "vote").unwrap_or(&null)),
                })),
                Err(RunnerError {
                    cooldown: Some(cooldown),
                    ..
                }) => {
                    let get = |key: &str| field(Some(&cooldown), key).unwrap_or(&null);
                    let rating = profile_state::rating(get("rating"));
                    let vote = profile_state::vote(get("vote"));
                    if get("error").as_str() == Some("LIKE_LIMIT") {
                        let likes_at = profile_state::date(field(Some(&cooldown), "likesAt"));
                        Ok(
                            json!({ "status": "like-limit", "likesAt": likes_at, "rating": rating, "vote": vote }),
                        )
                    } else {
                        Ok(json!({ "status": "cooldown", "rating": rating, "vote": vote }))
                    }
                }
                Err(error) => Err(error),
            }
        })
    }

    /// `friends({force})`: друзья Steam и кто из них в HLSR. Список держится в памяти: без `force`
    /// отдаётся готовый (`cached: true`); одновременные запросы ждут один и тот же.
    pub fn friends(&self, force: bool) -> BoxFuture<'static, Result<Value, RunnerError>> {
        let steam_id = {
            let state = self.state();
            let (Some(_), Some(profile)) = (&state.token, &state.profile) else {
                return ready(Ok(json!({ "signedIn": false, "friends": [] })));
            };
            let steam_id = profile["steamId64"].as_str().unwrap_or("").to_owned();
            if let Some((cached_id, data)) = &state.friends_cache
                && *cached_id == steam_id
                && !force
            {
                let mut out = assign_copy(data);
                out.insert("cached".to_owned(), json!(true));
                return ready(Ok(Value::Object(out)));
            }
            if let Some(load) = &state.friends_load {
                return Box::pin(wait_shared(load.clone()));
            }
            steam_id
        };
        let load = self.load_friends();
        let (sender, receiver) = watch::channel(None);
        self.state().friends_load = Some(receiver.clone());
        let this = self.clone();
        // Запись о загрузке уходит и тогда, когда задачу отбросили, не доведя до конца (в том числе
        // не начав: вне среды tokio фоновая работа не запускается): иначе каждый следующий вызов
        // ждал бы мёртвую загрузку.
        let mut loading = {
            let this = this.clone();
            Defer::new(move || this.state().friends_load = None)
        };
        self.spawn(async move {
            let result = load.await;
            {
                let mut state = this.state();
                if let Ok(data) = &result
                    && state.profile.as_ref().and_then(|p| p["steamId64"].as_str())
                        == Some(steam_id.as_str())
                {
                    state.friends_cache = Some((steam_id, data.clone()));
                }
                state.friends_load = None;
            }
            loading.disarm();
            let _ = sender.send(Some(result));
        });
        Box::pin(wait_shared(receiver))
    }

    fn load_friends(&self) -> BoxFuture<'static, Result<Value, RunnerError>> {
        let local = (self.inner.account_provider)();
        let call = self.call("/v2/runner/steam-friends", json!({}), true);
        let this = self.clone();
        Box::pin(async move {
            let remote = call.await?;
            let list: Vec<Value> = match field(Some(&remote), "friends").and_then(Value::as_array) {
                Some(friends) => friends
                    .iter()
                    .take(MAX_FRIENDS)
                    .map(|friend| {
                        if friend.is_null() {
                            return Err(RunnerError::plain("TypeError: friend"));
                        }
                        let mut out = Map::new();
                        for key in ["steamId64", "personaName"] {
                            if let Some(value) = field(Some(friend), key) {
                                out.insert(key.to_owned(), value.clone());
                            }
                        }
                        let avatar = field(Some(friend), "avatarUrl")
                            .filter(|avatar| js::truthy(avatar))
                            .cloned()
                            .unwrap_or(Value::Null);
                        out.insert("avatarDataUri".to_owned(), avatar);
                        Ok(Value::Object(out))
                    })
                    .collect::<Result<_, _>>()?,
                None => {
                    let own = this
                        .state()
                        .profile
                        .as_ref()
                        .map(|profile| profile["steamId64"].as_str().map(str::to_owned))
                        .ok_or_else(|| RunnerError::plain("TypeError: profile"))?;
                    if local.steam_id64.is_some() && local.steam_id64 == own {
                        let mut friends = (this.inner.friend_provider)();
                        friends.truncate(MAX_FRIENDS);
                        friends
                    } else {
                        Vec::new()
                    }
                }
            };
            // Пачки по 50 уходят одновременно, а не по очереди.
            let calls: Vec<_> = list
                .chunks(FRIENDS_BATCH)
                .map(|batch| {
                    let ids: Vec<Value> = batch
                        .iter()
                        .map(|friend| {
                            field(Some(friend), "steamId64")
                                .cloned()
                                .unwrap_or(Value::Null)
                        })
                        .collect();
                    tokio::spawn(this.call("/v2/runner/friends", json!({ "friends": ids }), true))
                })
                .collect();
            let mut found: HashSet<String> = HashSet::new();
            for call in calls {
                let response = call
                    .await
                    .map_err(|_| RunnerError::plain("RUNNER_NETWORK"))??;
                for profile in field(Some(&response), "profiles")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(id) = field(Some(profile), "steamId64").and_then(Value::as_str) {
                        found.insert(id.to_owned());
                    }
                }
            }
            let friends: Vec<Value> = list
                .iter()
                .map(|friend| {
                    let mut out = assign_copy(friend);
                    let id = field(Some(friend), "steamId64").and_then(Value::as_str);
                    out.insert(
                        "inHlsr".to_owned(),
                        json!(id.is_some_and(|id| found.contains(id))),
                    );
                    Value::Object(out)
                })
                .collect();
            Ok(json!({ "signedIn": true, "friends": friends }))
        })
    }

    /// `logout()`: закрыть сессии игр, отозвать вход на сервере, выйти здесь; согласие снято.
    pub fn logout(&self) -> BoxFuture<'static, Value> {
        let this = self.clone();
        // Нестрогий выход ошибкой не кончается.
        Box::pin(async move {
            this.sign_out("/v2/runner/logout", false)
                .await
                .unwrap_or_default()
        })
    }

    /// `logoutAll()`: как [`Self::logout`], но `/v2/runner/logout-all`, и отказ сервера не
    /// прячется: другие устройства остались бы в аккаунте.
    pub fn logout_all(&self) -> BoxFuture<'static, Result<Value, RunnerError>> {
        let this = self.clone();
        Box::pin(async move { this.sign_out("/v2/runner/logout-all", true).await })
    }

    /// Общее у выходов: сессии игр, отзыв входа по `endpoint` (`strict` — отказ сервера
    /// возвращается, здесь вход остаётся), выход здесь, согласие снято.
    async fn sign_out(&self, endpoint: &str, strict: bool) -> Result<Value, RunnerError> {
        for (id, _) in self.active_sessions() {
            if let Some(session) = self.active_session(&id) {
                let _ = self.beat_session(id, session, true).await;
            }
        }
        let revoked = self.call(endpoint, json!({}), true).await;
        if strict {
            revoked?;
        }
        self.clear_auth(None);
        if let Some(on_consent) = &self.inner.on_consent {
            on_consent(false);
        }
        Ok(self.snapshot())
    }

    /// `openExternal` службы (тот же, что у канала `runner-open-link`).
    pub(crate) fn open_external(&self) -> OpenExternal {
        self.inner.open_external.clone()
    }
}

/// Первый шаг пульса: открыть сессию или (уже открыта) пульс/конец с решением «конец ли».
enum First {
    Start(BoxFuture<'static, Result<Value, RunnerError>>),
    Started(bool, Option<BoxFuture<'static, Result<Value, RunnerError>>>),
}

/// `playingGame()`: игра, в которую сейчас играют (сессия не закрывается).
fn playing_game(active: &[(String, SessionRef)]) -> Option<String> {
    active
        .iter()
        .find(|(_, session)| !lock(session).ending)
        .map(|(id, _)| id.clone())
}

impl hlsr_delivery::ports::Achievements for OnlineProfileService {
    fn achieve(&self, id: &str) {
        OnlineProfileService::achieve(self, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_delay_like_timers() {
        assert_eq!(node_delay(5000.0), Duration::from_millis(5000));
        assert_eq!(node_delay(f64::NAN), Duration::from_millis(1));
        assert_eq!(node_delay(0.0), Duration::from_millis(1));
        assert_eq!(node_delay(1.5), Duration::from_millis(1));
        assert_eq!(node_delay(3e9), Duration::from_millis(1));
    }

    #[test]
    fn ipc_code_falls_back_to_network() {
        assert_eq!(
            RunnerError::plain("RUNNER_STALE_RESPONSE").ipc_code(),
            "RUNNER_NETWORK"
        );
        assert_eq!(
            RunnerError::coded("RUNNER_HTTP_401").ipc_code(),
            "RUNNER_HTTP_401"
        );
        assert_eq!(RunnerError::invalid_url().ipc_code(), "ERR_INVALID_URL");
    }

    #[test]
    fn moderation_notice_meta() {
        let dir = tempfile::tempdir().unwrap();
        let store = NotificationStore::new(dir.path().join("notifications.json"));
        let ban = json!({"id": 7, "action": "ban", "reason": "Читы", "expiresAt": "2026-10-16T14:00:00Z"});
        let item = notify_moderation(&store, &ban).unwrap().unwrap();
        assert_eq!(item["code"], "MOD_BAN");
        assert_eq!(item["detail"], "Читы");
        assert_eq!(item["meta"], json!({"until": "2026-10-16T14:00:00Z"}));
        let mute = json!({"id": 8, "action": "mute", "reason": "", "expiresAt": null});
        assert_eq!(
            notify_moderation(&store, &mute).unwrap().unwrap()["meta"],
            json!({"forever": true})
        );
        let nick = json!({"id": 9, "action": "reset-nick", "reason": "x", "expiresAt": null});
        let item = notify_moderation(&store, &nick).unwrap().unwrap();
        assert_eq!(item["code"], "MOD_RESET_NICK");
        assert!(item.get("meta").is_none());
        let logger = hlsr_delivery::ports::MemoryLogger::default();
        assert_eq!(
            notify_client_modified(&store, &logger).unwrap().unwrap()["code"],
            "CLIENT_MODIFIED"
        );
        // Объект проверки — исполняемый файл, не `app.asar` 0.4.7: в журнале так и сказано.
        let lines = logger.take();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].0, "warn");
        assert!(lines[0].1.contains("исполняемый файл изменён"));
        assert!(!lines[0].1.contains("asar"));
    }
}
