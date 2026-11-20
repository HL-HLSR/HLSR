//! Проверка обновлений (`UpdateChecker.js`): не чаще раза в 10 минут на игру, один общий
//! запуск на игру, цель из канала, источника или вшитого манифеста, план обновления (патчи или
//! полная сборка), снижение результата при сбое источника.
//!
//! События (`EventEmitter` 0.4.7) — слушатели-функции: вызываются сразу, в том же месте кода, где
//! событие рождается. `checking` приходит после того, как проверка уже числится идущей;
//! `result` — после того, как результат запомнен и проверка больше не числится идущей.
//!
//! Результат — объект с теми же ключами и порядком, что у 0.4.7 ([`UpdateResult`]): его форма
//! зависит от случая (не установлена, цель найдена, сбой, лаунчер слишком старый).
//!
//! Отличия от 0.4.7, неизбежные для будущих Rust. Промис там стартует сразу, а здесь пометка
//! «идёт» и событие `checking` наступают при первом опросе будущего `check`. Промис нельзя
//! отменить, а здесь проверку ведёт вызвавший её `check`: если его будущее сбросили, не дойдя до
//! конца, проверка перестаёт числиться идущей, а ожидавшие её вызовы берут её на себя.

use crate::channel_service::ChannelService;
use crate::errors::{Code, to_delivery_error};
use crate::js_value::{prop, truthy};
use crate::manifest_service::ManifestService;
use crate::origin_updates::OriginUpdates;
use crate::patch_chain::{PlanKind, PlanOptions, plan_update};
use crate::ports::{Logger, TracingLogger};
use crate::semver;
use crate::timing::{Clock, iso_from_ms, system_clock};
use crate::update_target;
use hlsr_core::js;
use hlsr_core::json_text::js_number;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::watch;

/// Не чаще раза в 10 минут.
pub const MIN_INTERVAL_MS: i64 = 10 * 60 * 1000;

/// `installedVersionOf(gameId)`: установленная версия; пусто — не установлена.
pub type InstalledVersionOf = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
/// `localManifestOf(gameId)`: вшитый манифест игры; нет — `None` (`null`).
pub type LocalManifestOf = Arc<dyn Fn(&str) -> Option<Value> + Send + Sync>;

/// Параметры проверки (`options` конструктора).
#[derive(Clone, Default)]
pub struct CheckerOptions {
    pub logger: Option<Arc<dyn Logger>>,
    pub channel_service: Option<Arc<ChannelService>>,
    pub origin_updates: Option<Arc<OriginUpdates>>,
    pub installed_version_of: Option<InstalledVersionOf>,
    pub local_manifest_of: Option<LocalManifestOf>,
    /// `None` и пустая строка — `0.0.0`.
    pub launcher_version: Option<String>,
    /// Интервал между проверками, мс; `None` — 10 минут.
    pub min_interval_ms: Option<i64>,
    /// Часы (`Date.now()` и `new Date()`); для тестов.
    pub clock: Option<Clock>,
}

/// Параметры `check(gameId, options)`.
#[derive(Debug, Clone, Default)]
pub struct CheckOptions {
    /// Проверить, даже если интервал не прошёл.
    pub force: bool,
    /// Причина — приходит в событие `checking`.
    pub reason: Option<String>,
    /// Канал спрашивать без условных заголовков.
    pub ignore_cache: bool,
}

/// Результат проверки: объект 0.4.7 с его ключами и порядком.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateResult(Map<String, Value>);

impl UpdateResult {
    /// Результат как объект JSON.
    pub fn to_json(&self) -> Value {
        Value::Object(self.0.clone())
    }

    /// Поле результата (`result.<имя>`); нет такого — `None` (`undefined`).
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    pub fn game_id(&self) -> &str {
        self.get("gameId")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// Поле как логическое значение JS (`!!result.<имя>`).
    pub fn flag(&self, key: &str) -> bool {
        truthy(self.get(key))
    }

    /// `result.installedVersion`; нет — `None`.
    pub fn installed_version(&self) -> Option<&str> {
        self.get("installedVersion").and_then(Value::as_str)
    }

    /// `result.availableVersion`; нет — `None`.
    pub fn available_version(&self) -> Option<&str> {
        self.get("availableVersion").and_then(Value::as_str)
    }

    /// `result.target` — цель обновления, если она найдена.
    pub fn target(&self) -> Option<&Value> {
        self.get("target").filter(|target| js::truthy(target))
    }

    fn with(mut self, key: &str, value: Value) -> UpdateResult {
        self.0.insert(key.to_owned(), value);
        self
    }
}

/// События проверки (`checking`, `result`).
#[derive(Debug, Clone)]
pub enum UpdateEvent {
    Checking {
        game_id: String,
        reason: Option<String>,
    },
    Result(UpdateResult),
}

type Listener = Arc<dyn Fn(&UpdateEvent) + Send + Sync>;

/// Идущая проверка: её ждут остальные вызовы той же игры.
struct Flight {
    id: u64,
    done: watch::Sender<Option<UpdateResult>>,
}

#[derive(Default)]
struct State {
    results: HashMap<String, UpdateResult>,
    last_check_at: HashMap<String, i64>,
    in_flight: HashMap<String, Flight>,
}

pub struct UpdateChecker {
    logger: Arc<dyn Logger>,
    channel_service: Option<Arc<ChannelService>>,
    origin_updates: Option<Arc<OriginUpdates>>,
    installed_version_of: InstalledVersionOf,
    local_manifest_of: LocalManifestOf,
    launcher_version: String,
    min_interval_ms: i64,
    clock: Clock,
    state: Mutex<State>,
    listeners: Mutex<Vec<Listener>>,
    next_flight: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Объект из пар «ключ — значение» с сохранением порядка.
fn object<const N: usize>(pairs: [(&str, Value); N]) -> Map<String, Value> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

/// Не завершённая проверка снимает себя при сбросе будущего: игра перестаёт числиться «идущей», а
/// время последней проверки возвращается к прежнему — `run` записывает его в начале, и сброшенная
/// проверка не должна отодвигать следующую на интервал (вернулся бы прежний результат).
struct FlightGuard<'a> {
    state: &'a Mutex<State>,
    game_id: String,
    id: u64,
    /// `lastCheckAt` до этой проверки; нет — игру ещё не проверяли.
    previous_check_at: Option<i64>,
    finished: bool,
}

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let mut state = lock(self.state);
        if state
            .in_flight
            .get(&self.game_id)
            .is_some_and(|flight| flight.id == self.id)
        {
            state.in_flight.remove(&self.game_id);
            match self.previous_check_at {
                Some(at) => state.last_check_at.insert(self.game_id.clone(), at),
                None => state.last_check_at.remove(&self.game_id),
            };
        }
    }
}

enum Role {
    /// Результат из кеша (`fromCache`).
    Cached(UpdateResult),
    /// Проверка уже идёт: дождаться её.
    Follower(watch::Receiver<Option<UpdateResult>>),
    /// Эта проверка идёт здесь: номер, канал для ожидающих и `lastCheckAt` до неё.
    Leader(u64, watch::Sender<Option<UpdateResult>>, Option<i64>),
}

impl UpdateChecker {
    /// `new UpdateChecker(options)`. Без `installedVersionOf` ничего не установлено, без
    /// `localManifestOf` вшитых манифестов нет.
    pub fn new(options: CheckerOptions) -> UpdateChecker {
        UpdateChecker {
            logger: options.logger.unwrap_or_else(|| Arc::new(TracingLogger)),
            channel_service: options.channel_service,
            origin_updates: options.origin_updates,
            installed_version_of: options
                .installed_version_of
                .unwrap_or_else(|| Arc::new(|_| None)),
            local_manifest_of: options
                .local_manifest_of
                .unwrap_or_else(|| Arc::new(|_| None)),
            launcher_version: options
                .launcher_version
                .filter(|version| !version.is_empty())
                .unwrap_or_else(|| "0.0.0".to_owned()),
            min_interval_ms: options.min_interval_ms.unwrap_or(MIN_INTERVAL_MS),
            clock: options.clock.unwrap_or_else(system_clock),
            state: Mutex::new(State::default()),
            listeners: Mutex::new(Vec::new()),
            next_flight: AtomicU64::new(1),
        }
    }

    /// `on("checking" | "result", listener)`: слушатель вызывается сразу, при рождении события.
    pub fn on_event(&self, listener: impl Fn(&UpdateEvent) + Send + Sync + 'static) {
        lock(&self.listeners).push(Arc::new(listener));
    }

    fn emit(&self, event: &UpdateEvent) {
        let listeners: Vec<Listener> = lock(&self.listeners).clone();
        for listener in listeners {
            listener(event);
        }
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    fn iso_now(&self) -> String {
        iso_from_ms(self.now())
    }

    /// `lastResult(gameId)`.
    pub fn last_result(&self, game_id: &str) -> Option<UpdateResult> {
        lock(&self.state).results.get(game_id).cloned()
    }

    fn is_due_in(&self, state: &State, game_id: &str) -> bool {
        // `!last`: время 0 тоже значит «ещё не проверяли».
        match state
            .last_check_at
            .get(game_id)
            .copied()
            .filter(|&last| last != 0)
        {
            None => true,
            Some(last) => self.now() - last >= self.min_interval_ms,
        }
    }

    /// `isDue(gameId)`.
    pub fn is_due(&self, game_id: &str) -> bool {
        self.is_due_in(&lock(&self.state), game_id)
    }

    /// `isChecking(gameId)`.
    pub fn is_checking(&self, game_id: &str) -> bool {
        lock(&self.state).in_flight.contains_key(game_id)
    }

    /// `invalidate(gameId)`: забыть результат и время проверки.
    pub fn invalidate(&self, game_id: &str) {
        let mut state = lock(&self.state);
        state.results.remove(game_id);
        state.last_check_at.remove(game_id);
    }

    /// `check(gameId, options)`: идёт проверка — её результат; интервал не прошёл и результат
    /// есть — он же с `fromCache`; иначе новая проверка.
    pub async fn check(&self, game_id: &str, options: &CheckOptions) -> UpdateResult {
        loop {
            let role = {
                let mut state = lock(&self.state);
                if let Some(flight) = state.in_flight.get(game_id) {
                    Role::Follower(flight.done.subscribe())
                } else if let Some(cached) = state
                    .results
                    .get(game_id)
                    .filter(|_| !options.force && !self.is_due_in(&state, game_id))
                {
                    Role::Cached(cached.clone().with("fromCache", json!(true)))
                } else {
                    let id = self.next_flight.fetch_add(1, Ordering::Relaxed);
                    let (done, _) = watch::channel(None);
                    state.in_flight.insert(
                        game_id.to_owned(),
                        Flight {
                            id,
                            done: done.clone(),
                        },
                    );
                    Role::Leader(id, done, state.last_check_at.get(game_id).copied())
                }
            };
            match role {
                Role::Cached(result) => return result,
                Role::Follower(mut receiver) => {
                    let finished = receiver.wait_for(Option::is_some).await;
                    if let Ok(result) = finished {
                        return result.clone().expect("проверка завершена");
                    }
                    // Первый вызов сбросили, не дождавшись: проверку берёт на себя следующий.
                }
                Role::Leader(id, done, previous_check_at) => {
                    let mut guard = FlightGuard {
                        state: &self.state,
                        game_id: game_id.to_owned(),
                        id,
                        previous_check_at,
                        finished: false,
                    };
                    self.emit(&UpdateEvent::Checking {
                        game_id: game_id.to_owned(),
                        reason: options.reason.clone().filter(|reason| !reason.is_empty()),
                    });
                    // `Promise.resolve().then(() => this.run(…))`: сама проверка — не в том же
                    // шаге, что событие «checking» (одновременные проверки сначала все сообщают
                    // о начале).
                    tokio::task::yield_now().await;
                    let result = self.run(game_id, options).await;
                    let result = self.publish(game_id, result);
                    guard.finished = true;
                    done.send_replace(Some(result.clone()));
                    return result;
                }
            }
        }
    }

    /// `run(gameId, opts)`: сама проверка, до публикации.
    async fn run(&self, game_id: &str, options: &CheckOptions) -> UpdateResult {
        let installed_version = (self.installed_version_of)(game_id).filter(|v| !v.is_empty());
        let previous = self.last_result(game_id);

        lock(&self.state)
            .last_check_at
            .insert(game_id.to_owned(), self.now());

        let Some(installed_version) = installed_version else {
            return UpdateResult(object([
                ("gameId", json!(game_id)),
                ("installed", json!(false)),
                ("installedVersion", Value::Null),
                ("availableVersion", Value::Null),
                ("upToDate", json!(true)),
                ("updateAvailable", json!(false)),
                ("updateRequired", json!(false)),
                ("kind", Value::Null),
                ("totalSize", json!(0)),
                ("steps", json!([])),
                ("source", Value::Null),
                ("checkedAt", json!(self.iso_now())),
                ("error", Value::Null),
            ]));
        };

        let mut target: Option<Value> = None;

        if let Some(channel) = &self.channel_service {
            match channel.fetch(game_id, options.ignore_cache).await {
                Ok(fetched) => {
                    target = update_target::from_channel(&fetched.manifest).map(|target| {
                        let mut target = target.to_json();
                        target["manifestSha256"] = json!(ManifestService::hash(&fetched.manifest));
                        target
                    });
                }
                Err(error) => {
                    if error.code != Code::ChannelDisabled {
                        self.logger.warn(&format!(
                            "[game-delivery] проверка обновлений {game_id} не удалась: {} {}",
                            error.code, error.message
                        ));
                        return self.degrade(
                            game_id,
                            &installed_version,
                            previous,
                            error.to_public(),
                        );
                    }
                }
            }
        }

        if target.is_none()
            && let Some(origin) = self
                .origin_updates
                .as_ref()
                .filter(|origin| origin.supports(game_id))
        {
            let local = (self.local_manifest_of)(game_id);
            match origin.target(game_id, local.as_ref()).await {
                Ok(found) => target = found,
                Err(failure) => {
                    let error = to_delivery_error(failure, Some(Code::ChannelUnavailable));
                    self.logger.warn(&format!(
                        "[game-delivery] проверка источника {game_id} не удалась: {} {}",
                        error.code, error.message
                    ));
                    return self.degrade(game_id, &installed_version, previous, error.to_public());
                }
            }
        }

        if target.is_none() {
            target = (self.local_manifest_of)(game_id)
                .as_ref()
                .and_then(update_target::from_local)
                .map(|target| target.to_json());
        }

        let Some(target) = target else {
            return self.degrade(game_id, &installed_version, previous, Value::Null);
        };

        let minimum = prop(&target, "minimumLauncherVersion").filter(|v| js::truthy(v));
        if let Some(minimum) = minimum
            && semver::lt(&self.launcher_version, &js::to_string(minimum))
        {
            return UpdateResult(object([
                ("gameId", json!(game_id)),
                ("installed", json!(true)),
                ("installedVersion", json!(installed_version)),
                (
                    "availableVersion",
                    prop(&target, "version").cloned().unwrap_or(Value::Null),
                ),
                ("upToDate", json!(false)),
                ("updateAvailable", json!(false)),
                ("updateRequired", json!(false)),
                ("launcherTooOld", json!(true)),
                ("kind", Value::Null),
                ("totalSize", json!(0)),
                ("steps", json!([])),
                (
                    "source",
                    prop(&target, "source").cloned().unwrap_or(Value::Null),
                ),
                ("checkedAt", json!(self.iso_now())),
                (
                    "error",
                    json!({
                        "code": "LAUNCHER_TOO_OLD",
                        "message": format!("Нужен лаунчер {}", js::to_string(minimum)),
                    }),
                ),
            ]));
        }

        let patches: Vec<Value> = prop(&target, "patches")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let full_archive = prop(&target, "fullArchive").filter(|archive| js::truthy(archive));
        let plan = plan_update(&PlanOptions {
            patches: &patches,
            from_version: Some(&installed_version),
            to_version: prop(&target, "version").and_then(Value::as_str),
            // `target.fullArchive ? target.fullArchive.size : 0`; размер не число — `NaN`.
            full_size: full_archive.map(|archive| {
                prop(archive, "size")
                    .and_then(crate::js_value::number)
                    .unwrap_or(f64::NAN)
            }),
        });

        let up_to_date = plan.kind == PlanKind::UpToDate;
        let downgrade = plan.reason == Some("downgrade");
        let update_available = !up_to_date && !downgrade;
        let total_size = if plan.total_size == 0.0 || plan.total_size.is_nan() {
            0.0
        } else {
            plan.total_size
        };

        let steps: Vec<Value> = plan
            .steps
            .iter()
            .map(|step| {
                let mut out = Map::new();
                for key in ["fromVersion", "toVersion"] {
                    if let Some(value) = prop(step, key) {
                        out.insert(key.to_owned(), value.clone());
                    }
                }
                // `step.archive ? step.archive.size : 0`.
                match prop(step, "archive").filter(|archive| js::truthy(archive)) {
                    Some(archive) => {
                        if let Some(size) = prop(archive, "size") {
                            out.insert("size".to_owned(), size.clone());
                        }
                    }
                    None => {
                        out.insert("size".to_owned(), json!(0));
                    }
                }
                Value::Object(out)
            })
            .collect();

        UpdateResult(object([
            ("gameId", json!(game_id)),
            ("installed", json!(true)),
            ("installedVersion", json!(installed_version)),
            (
                "availableVersion",
                prop(&target, "version").cloned().unwrap_or(Value::Null),
            ),
            ("upToDate", json!(up_to_date || downgrade)),
            ("updateAvailable", json!(update_available)),
            ("downgrade", json!(downgrade)),
            (
                "updateRequired",
                json!(update_available && truthy(prop(&target, "updateRequired"))),
            ),
            (
                "kind",
                if up_to_date {
                    Value::Null
                } else {
                    json!(plan.kind.as_str())
                },
            ),
            (
                "reason",
                plan.reason.map_or(Value::Null, |reason| json!(reason)),
            ),
            ("totalSize", js_number(total_size)),
            ("steps", Value::Array(steps)),
            (
                "source",
                prop(&target, "source").cloned().unwrap_or(Value::Null),
            ),
            (
                "publishedAt",
                prop(&target, "publishedAt")
                    .filter(|value| js::truthy(value))
                    .cloned()
                    .unwrap_or(Value::Null),
            ),
            ("checkedAt", json!(self.iso_now())),
            ("target", target),
            ("error", Value::Null),
        ]))
    }

    /// `degrade(gameId, installedVersion, previous, error)`: прежний результат с пометкой
    /// `stale` или, если его нет, «неизвестно».
    fn degrade(
        &self,
        game_id: &str,
        installed_version: &str,
        previous: Option<UpdateResult>,
        error: Value,
    ) -> UpdateResult {
        if let Some(previous) = previous.filter(|previous| previous.flag("installed")) {
            return previous
                .with("stale", json!(true))
                .with("checkedAt", json!(self.iso_now()))
                .with("error", error);
        }
        UpdateResult(object([
            ("gameId", json!(game_id)),
            ("installed", json!(true)),
            ("installedVersion", json!(installed_version)),
            ("availableVersion", Value::Null),
            ("upToDate", json!(true)),
            ("updateAvailable", json!(false)),
            ("updateRequired", json!(false)),
            ("unknown", json!(true)),
            ("kind", Value::Null),
            ("totalSize", json!(0)),
            ("steps", json!([])),
            ("source", Value::Null),
            ("checkedAt", json!(self.iso_now())),
            ("error", error),
        ]))
    }

    /// `publish(gameId, result)`: запомнить, снять «идёт», сообщить слушателям.
    fn publish(&self, game_id: &str, result: UpdateResult) -> UpdateResult {
        {
            let mut state = lock(&self.state);
            state.results.insert(game_id.to_owned(), result.clone());
            state.in_flight.remove(game_id);
        }
        self.emit(&UpdateEvent::Result(result.clone()));
        result
    }

    /// `checkIfOlderThan(gameId, maxAgeMs, reason)`: проверка была недавно — её результат с
    /// `fromCache` (или `None`, если результата нет); иначе новая проверка с `force`.
    pub async fn check_if_older_than(
        &self,
        game_id: &str,
        max_age_ms: i64,
        reason: Option<&str>,
    ) -> Option<UpdateResult> {
        let recent = {
            let state = lock(&self.state);
            state
                .last_check_at
                .get(game_id)
                .copied()
                .filter(|&last| last != 0 && self.now() - last < max_age_ms)
                .map(|_| state.results.get(game_id).cloned())
        };
        if let Some(cached) = recent {
            return cached.map(|cached| cached.with("fromCache", json!(true)));
        }
        let options = CheckOptions {
            force: true,
            reason: Some(
                reason
                    .filter(|reason| !reason.is_empty())
                    .unwrap_or("focus")
                    .to_owned(),
            ),
            ignore_cache: false,
        };
        Some(self.check(game_id, &options).await)
    }
}

impl std::fmt::Debug for UpdateChecker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateChecker")
            .field("launcher_version", &self.launcher_version)
            .finish_non_exhaustive()
    }
}
