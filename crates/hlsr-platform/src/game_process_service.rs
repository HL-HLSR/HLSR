//! Состояния процесса игры (`GameProcessService.js`): `not-running → starting → running →
//! stopping → exited | crashed`. Запись `.hlsr/state/<gameId>.process.json` для живых процессов
//! (после перезапуска лаунчера игра опознаётся по PID, пути и времени старта ±5 с), опрос раз
//! в 1,5 с, остановка за 10 с, перезапуск GoldSrc (тот же exe запущен заново — та же игра).
//!
//! События (`EventEmitter` 0.4.7) — вызовы подписчиков по порядку, без блокировок: `state`
//! (общий), `<имя состояния>` и `finished`. Запись процесса — объект JS с тем же порядком ключей
//! (`serde_json::Map` с `preserve_order`), поэтому файл на диске побайтно такой же.

use crate::process_inspect::{ProcessInfo, ProcessInspector, system_inspector};
use crate::process_stop::{Stopper, SystemStopper};
use hlsr_core::js::number::number_to_string;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform};
use hlsr_core::json_store::read_json_safe;
use hlsr_delivery::atomic::write_json_atomic;
use hlsr_delivery::episodes;
use hlsr_delivery::errors::{Code, DeliveryError, Failure};
use hlsr_delivery::file_ops::hex;
use hlsr_delivery::paths::{assert_inside_with, is_game_id};
use hlsr_delivery::ports::{Logger, ProcessProbe, ProcessStopper, TracingLogger};
use hlsr_delivery::providers::BoxFuture;
use hlsr_delivery::timing::{Clock, iso_from_ms, system_clock};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::task::AbortHandle;

/// `STATE_VERSION`.
pub const STATE_VERSION: u32 = 1;
/// `STATES`.
pub const STATES: [&str; 7] = [
    "not-running",
    "starting",
    "running",
    "stopping",
    "exited",
    "crashed",
    "unknown",
];
/// `ACTIVE_STATES`: игра запускается, идёт или закрывается.
pub const ACTIVE_STATES: [&str; 3] = ["starting", "running", "stopping"];
/// `STOP_TIMEOUT_MS`.
pub const STOP_TIMEOUT_MS: i64 = 10_000;
/// `POLL_INTERVAL_MS`.
pub const POLL_INTERVAL_MS: u64 = 1500;
/// Пауза между проверками в `requestStop`.
const STOP_CHECK_MS: u64 = 250;

/// Запись о процессе игры: объект JS (порядок ключей важен для файла на диске).
pub type Record = Map<String, Value>;

/// `emit(name, payload)`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessEvent {
    /// `state`, `finished` или имя состояния (`running`, `exited`…).
    pub name: String,
    pub payload: Value,
}

pub type Listener = Arc<dyn Fn(&ProcessEvent) + Send + Sync>;

/// Подписка: для отписки (`off`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenerId(u64);

/// Чем закончил дочерний процесс (события `exit` и `error` у `ChildProcess`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildEnd {
    Exit {
        code: Option<i64>,
        /// `SIGTERM`… (Linux).
        signal: Option<String>,
    },
    /// Событие `error`: процесс не запустился или им нельзя управлять.
    Error(String),
}

/// Дочерний процесс, за которым следит сервис: PID и ожидание выхода.
pub struct ChildHandle {
    pub pid: u32,
    pub wait: BoxFuture<'static, ChildEnd>,
}

impl ChildHandle {
    /// Ожидание `tokio::process::Child`; процесс не завершается вместе с сервисом.
    pub fn from_tokio(mut child: tokio::process::Child) -> Option<ChildHandle> {
        let pid = child.id()?;
        Some(ChildHandle {
            pid,
            wait: Box::pin(async move {
                match child.wait().await {
                    Ok(status) => ChildEnd::Exit {
                        code: status.code().map(i64::from),
                        signal: exit_signal(&status),
                    },
                    Err(error) => ChildEnd::Error(error.to_string()),
                }
            }),
        })
    }
}

/// Имя сигнала, которым завершён процесс (Linux).
#[cfg(unix)]
fn exit_signal(status: &std::process::ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt as _;
    status.signal().map(signal_name)
}

#[cfg(not(unix))]
fn exit_signal(_status: &std::process::ExitStatus) -> Option<String> {
    None
}

/// Имя сигнала Linux, как его показывает Node (`SIGTERM`); незнакомый — `SIG<номер>`.
pub fn signal_name(signal: i32) -> String {
    match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        other => return format!("SIG{other}"),
    }
    .to_owned()
}

/// Что передают в `registerStartedProcess(gameId, info)`.
#[derive(Default)]
pub struct StartedProcess {
    /// `Number(info.pid)`; `None` — не число.
    pub pid: Option<i64>,
    pub executable_path: Option<String>,
    pub install_path: Option<String>,
    pub started_at: Option<String>,
    pub process_key: Option<String>,
    /// Процесс-хозяин (сессия `umu` на Linux): закрывается группой, игра в нём — другой процесс.
    pub host_process: bool,
    pub child: Option<ChildHandle>,
    /// Эпизод Ghosting (`ep1`, `ep2`): на выходе (и после перезапуска лаунчера) его config.cfg
    /// вернётся в Ghosting. Незнакомое значение не пишется.
    pub episode: Option<String>,
}

/// `options` `requestStop(gameId, options)`.
#[derive(Debug, Clone, Copy, Default)]
pub struct StopOptions {
    pub force: bool,
    pub timeout_ms: Option<i64>,
}

/// Параметры конструктора (`options`).
#[derive(Clone, Default)]
pub struct ServiceOptions {
    /// `paths.state`: `<библиотека>/.hlsr/state`.
    pub state_dir: PathBuf,
    /// `paths.games`: папки игр (`gameInstallPath`) для сверки процесса записи; по умолчанию —
    /// `Games` библиотеки этой `state_dir`.
    pub games_dir: Option<PathBuf>,
    pub logger: Option<Arc<dyn Logger>>,
    pub inspector: Option<Arc<dyn ProcessInspector>>,
    pub stopper: Option<Arc<dyn Stopper>>,
    pub stop_timeout_ms: Option<i64>,
    pub poll_interval_ms: Option<u64>,
    /// Часы (`Date`).
    pub clock: Option<Clock>,
    /// `process.platform` для путей игры; по умолчанию — эта ОС.
    pub platform: Option<Platform>,
    /// `process.pid` (запись `launcherPid`); по умолчанию — PID лаунчера.
    pub launcher_pid: Option<u32>,
    /// Среда выполнения для опроса и ожидания выхода; по умолчанию — текущая на момент вызова.
    pub runtime: Option<tokio::runtime::Handle>,
}

/// `processKeyOf(pid, executablePath, startedAt)`: первые 32 знака SHA-256 от
/// `<pid>|<путь в нижнем регистре>|<время старта>`.
pub fn process_key_of(pid: f64, executable_path: Option<&str>, started_at: Option<&str>) -> String {
    let text = format!(
        "{}|{}|{}",
        number_to_string(pid),
        executable_path.unwrap_or("").to_lowercase(),
        started_at.unwrap_or("")
    );
    let digest = Sha256::digest(text.as_bytes());
    hex(&digest)[..32].to_owned()
}

struct Entry {
    record: Record,
    /// PID дочернего процесса, за которым следит сервис (`entry.child.pid`).
    child_pid: Option<u32>,
    timer: Option<AbortHandle>,
    stop_requested: bool,
}

#[derive(Default)]
struct State {
    /// `tracked`: `Map` в порядке первой вставки.
    tracked: Vec<(String, Entry)>,
    last_exits: HashMap<String, Value>,
}

impl State {
    fn position(&self, game_id: &str) -> Option<usize> {
        self.tracked.iter().position(|(id, _)| id == game_id)
    }

    fn find(&self, game_id: &str) -> Option<&Entry> {
        self.tracked
            .iter()
            .find(|(id, _)| id == game_id)
            .map(|(_, entry)| entry)
    }

    fn find_mut(&mut self, game_id: &str) -> Option<&mut Entry> {
        self.tracked
            .iter_mut()
            .find(|(id, _)| id == game_id)
            .map(|(_, entry)| entry)
    }

    /// `tracked.set`: у существующего ключа место прежнее.
    fn set(&mut self, game_id: &str, entry: Entry) {
        match self.position(game_id) {
            Some(at) => self.tracked[at].1 = entry,
            None => self.tracked.push((game_id.to_owned(), entry)),
        }
    }

    fn remove(&mut self, game_id: &str) -> Option<Entry> {
        let at = self.position(game_id)?;
        Some(self.tracked.remove(at).1)
    }
}

struct Inner {
    state_dir: PathBuf,
    /// `paths.games`.
    games_dir: String,
    logger: Arc<dyn Logger>,
    inspector: Arc<dyn ProcessInspector>,
    stopper: Arc<dyn Stopper>,
    stop_timeout_ms: i64,
    poll_interval_ms: u64,
    clock: Clock,
    path: NodePath,
    launcher_pid: u32,
    runtime: Option<tokio::runtime::Handle>,
    state: Mutex<State>,
    listeners: Mutex<(u64, Vec<(u64, Listener)>)>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (_, entry) in &state.tracked {
            if let Some(timer) = &entry.timer {
                timer.abort();
            }
        }
    }
}

/// `GameProcessService`.
#[derive(Clone)]
pub struct GameProcessService {
    inner: Arc<Inner>,
}

fn string_of(record: &Record, key: &str) -> Option<String> {
    record.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn is_active(state: Option<&str>) -> bool {
    state.is_some_and(|state| ACTIVE_STATES.contains(&state))
}

fn state_of(record: &Record) -> Option<&str> {
    record.get("state").and_then(Value::as_str)
}

/// PID записи, если это целое число больше нуля.
fn record_pid(record: &Record) -> Option<u32> {
    let pid = record.get("pid").and_then(|pid| match pid {
        Value::Number(number) => number.as_f64(),
        _ => None,
    })?;
    (pid.fract() == 0.0 && pid > 0.0 && pid <= f64::from(u32::MAX)).then_some(pid as u32)
}

fn truthy(record: &Record, key: &str) -> bool {
    record.get(key).is_some_and(js::truthy)
}

/// `episodes.isEpisode(value) ? value : null`: эпизод Ghosting записи — только известный.
fn known_episode(value: Option<&Value>) -> Value {
    match value.and_then(Value::as_str) {
        Some(id) if episodes::is_episode(id) => json!(id),
        _ => Value::Null,
    }
}

/// `record.episode || null`.
fn episode_of(record: &Record) -> Value {
    record
        .get("episode")
        .filter(|value| js::truthy(value))
        .cloned()
        .unwrap_or(Value::Null)
}

impl GameProcessService {
    pub fn new(options: ServiceOptions) -> GameProcessService {
        let platform = options.platform.unwrap_or(Platform::CURRENT);
        // `<библиотека>/.hlsr/state` → `<библиотека>/Games` (`LAYOUT`).
        let games_dir = options.games_dir.map_or_else(
            || {
                let state = options.state_dir.to_string_lossy();
                NodePath::native().join(&[&state, "..", "..", "Games"])
            },
            |games| games.to_string_lossy().into_owned(),
        );
        GameProcessService {
            inner: Arc::new(Inner {
                games_dir,
                state_dir: options.state_dir,
                logger: options.logger.unwrap_or_else(|| Arc::new(TracingLogger)),
                inspector: options.inspector.unwrap_or_else(system_inspector),
                stopper: options.stopper.unwrap_or_else(|| Arc::new(SystemStopper)),
                stop_timeout_ms: options.stop_timeout_ms.unwrap_or(STOP_TIMEOUT_MS),
                poll_interval_ms: options.poll_interval_ms.unwrap_or(POLL_INTERVAL_MS),
                clock: options.clock.unwrap_or_else(system_clock),
                path: crate::process_inspect::node_path(platform),
                launcher_pid: options.launcher_pid.unwrap_or_else(std::process::id),
                runtime: options.runtime,
                state: Mutex::new(State::default()),
                listeners: Mutex::new((0, Vec::new())),
            }),
        }
    }

    /// `on(name, listener)` для всех событий: подписчик получает имя и данные.
    pub fn subscribe(&self, listener: Listener) -> ListenerId {
        let mut listeners = self.inner.listeners();
        listeners.0 += 1;
        let id = listeners.0;
        listeners.1.push((id, listener));
        ListenerId(id)
    }

    /// `off`.
    pub fn unsubscribe(&self, id: ListenerId) {
        self.inner
            .listeners()
            .1
            .retain(|(listener, _)| *listener != id.0);
    }

    /// `statePath(gameId)`: `<state>/<gameId>.process.json`.
    pub fn state_path(&self, game_id: &str) -> Result<PathBuf, DeliveryError> {
        self.inner.state_path(game_id)
    }

    /// `readPersisted(gameId)`: запись нужной версии или `None`.
    pub fn read_persisted(&self, game_id: &str) -> Option<Record> {
        self.inner.read_persisted(game_id)
    }

    /// `publicState(gameId)`.
    pub fn public_state(&self, game_id: &str) -> Value {
        self.inner.public_state(game_id)
    }

    /// `getProcessState(gameId)`.
    pub fn get_process_state(&self, game_id: &str) -> Value {
        self.inner.public_state(game_id)
    }

    /// `isRunning(gameId)`.
    pub fn is_running(&self, game_id: &str) -> bool {
        self.inner.is_running(game_id)
    }

    /// `anyRunning()`: первая запущенная игра по порядку регистрации.
    pub fn any_running(&self) -> Option<String> {
        self.inner.any_running()
    }

    /// `registerStartedProcess(gameId, info)`.
    pub fn register_started_process(
        &self,
        game_id: &str,
        info: StartedProcess,
    ) -> Result<Value, DeliveryError> {
        self.inner.register_started_process(game_id, info)
    }

    /// `markStarting(gameId)`.
    pub fn mark_starting(&self, game_id: &str) -> Value {
        self.inner.mark_starting(game_id)
    }

    /// `cancelStarting(gameId, reason)`.
    pub fn cancel_starting(&self, game_id: &str, reason: Option<&str>) -> Value {
        self.inner.cancel_starting(game_id, reason)
    }

    /// `handleExit(gameId, exitCode, signal, error, detected)`.
    pub fn handle_exit(
        &self,
        game_id: &str,
        exit_code: Option<i64>,
        signal: Option<&str>,
        error: Option<&str>,
        detected: bool,
    ) {
        self.inner
            .handle_exit(game_id, exit_code, signal, error, detected);
    }

    /// `markExited(gameId, pid)`: игра закрылась, а процесс-хозяин жив; `false` — это уже
    /// другой запуск.
    pub fn mark_exited(&self, game_id: &str, pid: u32) -> bool {
        self.inner.mark_exited(game_id, pid)
    }

    /// `unregisterProcess(gameId, result)`.
    pub fn unregister_process(&self, game_id: &str, last_exit: Option<&Value>) -> Value {
        self.inner.unregister_process(game_id, last_exit)
    }

    /// `requestStop(gameId, options)`: `{status: "not-running" | "stopped" | "timeout"}`.
    pub async fn request_stop(
        &self,
        game_id: &str,
        options: StopOptions,
    ) -> Result<Value, DeliveryError> {
        self.inner.request_stop(game_id, options).await
    }

    /// `matchesRecord(record)`: процесс записи — это та самая игра.
    pub fn matches_record(&self, record: &Record) -> bool {
        self.inner.matches_record(record)
    }

    /// `reconcileAfterLauncherRestart(gameIds)`: пусто — все записи в папке состояния.
    pub fn reconcile_after_launcher_restart(&self, game_ids: &[String]) -> Vec<Value> {
        self.inner.reconcile_after_launcher_restart(game_ids)
    }

    /// `persistedGameIds()`.
    pub fn persisted_game_ids(&self) -> Vec<String> {
        self.inner.persisted_game_ids()
    }

    /// Один шаг опроса процесса — то, что делает таймер раз в 1,5 с. `false` — игра не
    /// отслеживается опросом.
    pub fn poll_now(&self, game_id: &str) -> bool {
        self.inner.poll_now(game_id)
    }

    /// `dispose()`: опрос остановлен, отслеживание забыто (записи на диске остаются).
    pub fn dispose(&self) {
        self.inner.dispose();
    }

    /// `event.lastExit.outcome = "crashed"` в обработчике `finished` (игра вышла с кодом 0, но
    /// оставила свежий дамп): последний выход игры — падение и в `publicState`.
    pub fn mark_last_exit_crashed(&self, game_id: &str) {
        self.inner.mark_last_exit_crashed(game_id);
    }
}

impl Inner {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn listeners(&self) -> std::sync::MutexGuard<'_, (u64, Vec<(u64, Listener)>)> {
        self.listeners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `nowIso()`.
    fn now_iso(&self) -> String {
        iso_from_ms((self.clock)())
    }

    fn emit(&self, name: &str, payload: Value) {
        let listeners: Vec<Listener> = self
            .listeners()
            .1
            .iter()
            .map(|(_, listener)| listener.clone())
            .collect();
        let event = ProcessEvent {
            name: name.to_owned(),
            payload,
        };
        for listener in listeners {
            listener(&event);
        }
    }

    // ------------------------------------------------------------ файл состояния

    fn state_path(&self, game_id: &str) -> Result<PathBuf, DeliveryError> {
        if !is_game_id(game_id) {
            return Err(DeliveryError::new(
                Code::PathInvalid,
                format!("Недопустимый gameId: {game_id}"),
            ));
        }
        let state = self.state_dir.to_string_lossy().into_owned();
        // Файл на диске — по правилам этой ОС, а не игры.
        let native = NodePath::native();
        let file = native.join(&[&state, &format!("{game_id}.process.json")]);
        assert_inside_with(&native, &state, &file, Some("Файл состояния процесса"))
            .map(PathBuf::from)
    }

    fn persist(&self, record: &Record) {
        let game_id = string_of(record, "gameId").unwrap_or_default();
        let result = self
            .state_path(&game_id)
            .map_err(|error| error.message)
            .and_then(|file| {
                let mut out = Record::new();
                out.insert("version".to_owned(), json!(STATE_VERSION));
                for (key, value) in record {
                    out.insert(key.clone(), value.clone());
                }
                write_json_atomic(&file, &Value::Object(out)).map_err(|error| error.message)
            });
        if let Err(message) = result {
            self.logger
                .warn(&format!("[game-process] состояние не сохранено: {message}"));
        }
    }

    /// `gameId` — из имени файла: по нему `matches_record` берёт папку игры. Файл лежит в общей
    /// библиотеке: эпизод из него — только известный (по нему на выходе пишется config.cfg).
    fn read_persisted(&self, game_id: &str) -> Option<Record> {
        let file = self.state_path(game_id).ok()?;
        match read_json_safe(&file) {
            Value::Object(mut data) if data.get("version").and_then(Value::as_f64) == Some(1.0) => {
                let episode = known_episode(data.get("episode"));
                data.insert("gameId".to_owned(), json!(game_id));
                data.insert("episode".to_owned(), episode);
                Some(data)
            }
            _ => None,
        }
    }

    fn clear_persisted(&self, game_id: &str) {
        let result = self
            .state_path(game_id)
            .map_err(|error| error.message)
            .and_then(|file| {
                if file.exists() {
                    std::fs::remove_file(&file).map_err(|error| {
                        hlsr_delivery::errors::RawError::fs(&error, "unlink", &file).message
                    })
                } else {
                    Ok(())
                }
            });
        if let Err(message) = result {
            self.logger
                .warn(&format!("[game-process] состояние не удалено: {message}"));
        }
    }

    fn persisted_game_ids(&self) -> Vec<String> {
        let Ok(items) = js::fs::read_dir_names(&self.state_dir) else {
            return Vec::new();
        };
        items
            .into_iter()
            .filter_map(|name| name.strip_suffix(".process.json").map(str::to_owned))
            .collect()
    }

    // ------------------------------------------------------------ состояние

    fn default_state(game_id: &str) -> Record {
        let mut record = Record::new();
        record.insert("gameId".to_owned(), json!(game_id));
        record.insert("state".to_owned(), json!("not-running"));
        for key in [
            "pid",
            "processKey",
            "executablePath",
            "startedAt",
            "since",
            "exitCode",
            "signal",
            "lastExit",
        ] {
            record.insert(key.to_owned(), Value::Null);
        }
        record
    }

    /// `publicState` по записи.
    fn public_of(
        &self,
        game_id: &str,
        state: &Record,
        last_exits: &HashMap<String, Value>,
    ) -> Value {
        let or_null = |key: &str| {
            state
                .get(key)
                .filter(|value| js::truthy(value))
                .cloned()
                .unwrap_or(Value::Null)
        };
        let last_exit = state
            .get("lastExit")
            .filter(|value| js::truthy(value))
            .cloned()
            .or_else(|| last_exits.get(game_id).cloned())
            .unwrap_or(Value::Null);
        json!({
            "gameId": game_id,
            "state": state.get("state").cloned().unwrap_or(Value::Null),
            "running": is_active(state_of(state)),
            "since": or_null("since"),
            "startedAt": or_null("startedAt"),
            "exitCode": match state.get("exitCode") {
                None | Some(Value::Null) => Value::Null,
                Some(code) => code.clone(),
            },
            "lastExit": last_exit,
            "episode": episode_of(state),
        })
    }

    fn public_state(&self, game_id: &str) -> Value {
        let tracked = {
            let state = self.lock();
            state.find(game_id).map(|entry| entry.record.clone())
        };
        let record = match tracked {
            Some(record) => record,
            None => self
                .read_persisted(game_id)
                .unwrap_or_else(|| Self::default_state(game_id)),
        };
        let state = self.lock();
        self.public_of(game_id, &record, &state.last_exits)
    }

    fn is_running(&self, game_id: &str) -> bool {
        self.lock()
            .find(game_id)
            .is_some_and(|entry| is_active(state_of(&entry.record)))
    }

    fn any_running(&self) -> Option<String> {
        self.lock()
            .tracked
            .iter()
            .find(|(_, entry)| is_active(state_of(&entry.record)))
            .map(|(game_id, _)| game_id.clone())
    }

    /// `setState(gameId, state, extra)` под замком: запись обновлена и сохранена (или стёрта);
    /// события отправляет вызывающий после снятия замка. Возвращает `publicState`.
    fn set_state_locked(
        &self,
        locked: &mut State,
        game_id: &str,
        state: &str,
        extra: Vec<(&str, Value)>,
    ) -> Value {
        let since = self.now_iso();
        let apply = |record: &mut Record| {
            for (key, value) in extra {
                record.insert(key.to_owned(), value);
            }
            record.insert("state".to_owned(), json!(state));
            record.insert("since".to_owned(), json!(since));
        };
        let record = match locked.find_mut(game_id) {
            Some(entry) => {
                apply(&mut entry.record);
                entry.record.clone()
            }
            None => {
                let mut record = Self::default_state(game_id);
                apply(&mut record);
                record
            }
        };
        if ACTIVE_STATES.contains(&state) {
            self.persist(&record);
        } else {
            self.clear_persisted(game_id);
        }
        match locked.find(game_id) {
            Some(entry) => self.public_of(game_id, &entry.record, &locked.last_exits),
            None => {
                let persisted = self
                    .read_persisted(game_id)
                    .unwrap_or_else(|| Self::default_state(game_id));
                self.public_of(game_id, &persisted, &locked.last_exits)
            }
        }
    }

    fn emit_state(&self, state: &str, payload: &Value) {
        self.emit("state", payload.clone());
        self.emit(state, payload.clone());
    }

    /// `setState(gameId, state, extra)`.
    fn set_state(&self, game_id: &str, state: &str, extra: Vec<(&str, Value)>) -> Value {
        let payload = {
            let mut locked = self.lock();
            self.set_state_locked(&mut locked, game_id, state, extra)
        };
        self.emit_state(state, &payload);
        payload
    }

    // ------------------------------------------------------------ регистрация

    fn register_started_process(
        self: &Arc<Self>,
        game_id: &str,
        info: StartedProcess,
    ) -> Result<Value, DeliveryError> {
        self.lock().last_exits.remove(game_id);

        let pid = match info.pid {
            Some(pid) if pid > 0 => pid,
            other => {
                return Err(DeliveryError::new(
                    Code::ProcessNotFound,
                    format!(
                        "Некорректный PID: {}",
                        other.map_or_else(|| "undefined".to_owned(), |pid| pid.to_string())
                    ),
                ));
            }
        };

        let Some(executable) = info
            .executable_path
            .as_deref()
            .filter(|path| !path.is_empty() && self.path.is_absolute(path))
        else {
            return Err(DeliveryError::new(
                Code::ProcessMismatch,
                "Нужен абсолютный путь исполняемого файла игры",
            ));
        };
        let Some(install_path) = info.install_path.as_deref().filter(|path| !path.is_empty())
        else {
            return Err(DeliveryError::new(
                Code::ProcessMismatch,
                "Не задана папка установленной игры",
            ));
        };

        let executable_path = assert_inside_with(
            &self.path,
            install_path,
            executable,
            Some("Исполняемый файл игры"),
        )?;

        {
            let mut locked = self.lock();
            if let Some(current) = locked.find_mut(game_id) {
                if matches!(state_of(&current.record), Some("running" | "stopping")) {
                    return Err(DeliveryError::new(
                        Code::AlreadyRunning,
                        format!("Игра {game_id} уже отслеживается"),
                    ));
                }
                if let Some(timer) = current.timer.take() {
                    timer.abort();
                }
            }
        }

        let started_at = info
            .started_at
            .filter(|started| !started.is_empty())
            .unwrap_or_else(|| self.now_iso());

        let mut record = Self::default_state(game_id);
        record.insert("state".to_owned(), json!("running"));
        record.insert("pid".to_owned(), json!(pid));
        record.insert("executablePath".to_owned(), json!(executable_path));
        record.insert(
            "installPath".to_owned(),
            json!(self.path.resolve(&[install_path])),
        );
        record.insert("startedAt".to_owned(), json!(started_at));
        let process_key = info
            .process_key
            .filter(|key| !key.is_empty())
            .unwrap_or_else(|| {
                process_key_of(pid as f64, Some(&executable_path), Some(&started_at))
            });
        record.insert("processKey".to_owned(), json!(process_key));
        record.insert("launcherPid".to_owned(), json!(self.launcher_pid));
        record.insert("registeredAt".to_owned(), json!(self.now_iso()));
        record.insert("hostProcess".to_owned(), json!(info.host_process));
        record.insert(
            "episode".to_owned(),
            known_episode(info.episode.map(Value::String).as_ref()),
        );

        let child_pid = info.child.as_ref().map(|child| child.pid);
        self.lock().set(
            game_id,
            Entry {
                record,
                child_pid,
                timer: None,
                stop_requested: false,
            },
        );

        self.logger.info(&format!(
            "[game-process] игра {game_id} зарегистрирована: PID {pid}, {executable_path}"
        ));

        match info.child {
            Some(child) => self.attach_child(game_id, child),
            None => self.start_polling(game_id),
        }

        self.set_state(game_id, "running", Vec::new());
        Ok(self.public_state(game_id))
    }

    fn mark_starting(&self, game_id: &str) -> Value {
        {
            let mut locked = self.lock();
            if let Some(entry) = locked.find(game_id)
                && is_active(state_of(&entry.record))
            {
                let payload = self.public_of(game_id, &entry.record, &locked.last_exits);
                return payload;
            }
            let mut record = Self::default_state(game_id);
            record.insert("state".to_owned(), json!("starting"));
            locked.set(
                game_id,
                Entry {
                    record,
                    child_pid: None,
                    timer: None,
                    stop_requested: false,
                },
            );
        }
        self.set_state(game_id, "starting", Vec::new())
    }

    fn cancel_starting(&self, game_id: &str, reason: Option<&str>) -> Value {
        let starting = self
            .lock()
            .find(game_id)
            .is_some_and(|entry| state_of(&entry.record) == Some("starting"));
        if !starting {
            return self.public_state(game_id);
        }
        self.logger.warn(&format!(
            "[game-process] запуск {game_id} не состоялся: {}",
            reason
                .filter(|reason| !reason.is_empty())
                .unwrap_or("нет причины")
        ));
        self.unregister_process(game_id, None)
    }

    fn attach_child(self: &Arc<Self>, game_id: &str, child: ChildHandle) {
        let Some(handle) = self.runtime_handle() else {
            self.logger.warn(&format!(
                "[game-process] нет среды выполнения: выход процесса {game_id} не отслеживается"
            ));
            return;
        };
        let weak: Weak<Inner> = Arc::downgrade(self);
        let game_id = game_id.to_owned();
        handle.spawn(async move {
            let end = child.wait.await;
            let Some(inner) = weak.upgrade() else {
                return;
            };
            match end {
                ChildEnd::Exit { code, signal } => {
                    inner.handle_exit(&game_id, code, signal.as_deref(), None, false);
                }
                ChildEnd::Error(message) => {
                    inner.logger.error(&format!(
                        "[game-process] ошибка процесса {game_id}: {message}"
                    ));
                    inner.handle_exit(&game_id, None, None, Some(&message), false);
                }
            }
        });
    }

    fn runtime_handle(&self) -> Option<tokio::runtime::Handle> {
        self.runtime
            .clone()
            .or_else(|| tokio::runtime::Handle::try_current().ok())
    }

    // ------------------------------------------------------------ опрос

    fn start_polling(self: &Arc<Self>, game_id: &str) {
        let mut locked = self.lock();
        let Some(entry) = locked.find_mut(game_id) else {
            return;
        };
        if entry.timer.is_some() {
            return;
        }
        let Some(handle) = self.runtime_handle() else {
            self.logger.warn(&format!(
                "[game-process] нет среды выполнения: опрос процесса {game_id} не запущен"
            ));
            return;
        };
        let weak: Weak<Inner> = Arc::downgrade(self);
        let id = game_id.to_owned();
        let interval = Duration::from_millis(self.poll_interval_ms);
        let task = handle.spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                inner.poll_tick(&id);
            }
        });
        entry.timer = Some(task.abort_handle());
    }

    fn stop_polling(locked: &mut State, game_id: &str) {
        if let Some(entry) = locked.find_mut(game_id)
            && let Some(timer) = entry.timer.take()
        {
            timer.abort();
        }
    }

    /// Тело таймера: процесс исчез — игра закрыта.
    fn poll_tick(&self, game_id: &str) {
        let (pid, stop_requested) = {
            let locked = self.lock();
            let Some(current) = locked.find(game_id) else {
                return;
            };
            (record_pid(&current.record), current.stop_requested)
        };
        let alive = pid.is_some_and(|pid| self.inspector.is_alive(pid));
        if !alive {
            self.handle_exit(game_id, stop_requested.then_some(0), None, None, true);
        }
    }

    fn poll_now(&self, game_id: &str) -> bool {
        let polled = self
            .lock()
            .find(game_id)
            .is_some_and(|entry| entry.timer.is_some());
        if polled {
            self.poll_tick(game_id);
        }
        polled
    }

    // ------------------------------------------------------------ выход

    /// `classifyExit(entry, exitCode, signal, error)`.
    fn classify_exit(
        stop_requested: bool,
        exit_code: Option<i64>,
        signal: Option<&str>,
        error: Option<&str>,
    ) -> &'static str {
        if error.is_some() {
            return "crashed";
        }
        if stop_requested {
            return "exited";
        }
        if signal.is_some_and(|signal| !signal.is_empty()) {
            return "crashed";
        }
        match exit_code {
            None | Some(0) => "exited",
            Some(_) => "crashed",
        }
    }

    /// `findSuccessor(entry)`: процесс игры пропал, но тот же exe из той же папки уже запущен
    /// заново — так GoldSrc перезапускает себя (смена видеорежима, `_restart`).
    fn find_successor(&self, entry: &Entry) -> Option<ProcessInfo> {
        let record = &entry.record;
        if entry.stop_requested || entry.child_pid.is_some() || truthy(record, "hostProcess") {
            return None;
        }
        let executable = string_of(record, "executablePath")?;
        let pid = record_pid(record);
        self.inspector
            .find_by_image_name(&self.path.basename(&executable))
            .into_iter()
            .find(|item| {
                Some(item.pid) != pid
                    && item.executable_path.as_deref().is_some_and(|path| {
                        !path.is_empty() && self.inspector.same_path(Some(path), Some(&executable))
                    })
                    && self.inspector.is_alive(item.pid)
            })
    }

    fn handle_exit(
        &self,
        game_id: &str,
        exit_code: Option<i64>,
        signal: Option<&str>,
        error: Option<&str>,
        detected: bool,
    ) {
        let (last_exit, outcome, payload, episode) = {
            let mut locked = self.lock();
            let Some(entry) = locked.find(game_id) else {
                return;
            };
            if matches!(state_of(&entry.record), Some("exited" | "crashed")) {
                return;
            }

            if detected
                && error.is_none()
                && let Some(successor) = self.find_successor(entry)
            {
                let entry = locked.find_mut(game_id).expect("запись только что найдена");
                self.logger.info(&format!(
                    "[game-process] игра {game_id} перезапустилась: PID {} → {}",
                    js::to_template_string(entry.record.get("pid")),
                    successor.pid
                ));
                let started_at = successor
                    .started_at
                    .clone()
                    .filter(|started| !started.is_empty())
                    .unwrap_or_else(|| self.now_iso());
                let executable = string_of(&entry.record, "executablePath");
                entry.record.insert("pid".to_owned(), json!(successor.pid));
                entry
                    .record
                    .insert("startedAt".to_owned(), json!(started_at));
                entry.record.insert(
                    "processKey".to_owned(),
                    json!(process_key_of(
                        f64::from(successor.pid),
                        executable.as_deref(),
                        Some(&started_at)
                    )),
                );
                let record = entry.record.clone();
                self.persist(&record);
                return;
            }

            Self::stop_polling(&mut locked, game_id);
            let entry = locked.find(game_id).expect("запись только что найдена");

            let outcome = Self::classify_exit(entry.stop_requested, exit_code, signal, error);
            let mut last_exit = Map::new();
            last_exit.insert("outcome".to_owned(), json!(outcome));
            last_exit.insert("exitCode".to_owned(), json!(exit_code));
            last_exit.insert(
                "signal".to_owned(),
                match signal.filter(|signal| !signal.is_empty()) {
                    Some(signal) => json!(signal),
                    None => Value::Null,
                },
            );
            last_exit.insert("requested".to_owned(), json!(entry.stop_requested));
            last_exit.insert("detected".to_owned(), json!(detected));
            last_exit.insert(
                "error".to_owned(),
                match error {
                    Some(message) => json!(message),
                    None => Value::Null,
                },
            );
            last_exit.insert("at".to_owned(), json!(self.now_iso()));
            last_exit.insert(
                "pid".to_owned(),
                entry.record.get("pid").cloned().unwrap_or(Value::Null),
            );
            if let Some(started_at) = entry.record.get("startedAt") {
                last_exit.insert("startedAt".to_owned(), started_at.clone());
            }
            let last_exit = Value::Object(last_exit);
            let episode = episode_of(&entry.record);

            let pid_text = js::to_template_string(last_exit.get("pid"));
            let code_text = js::to_template_string(last_exit.get("exitCode"));
            if outcome == "crashed" {
                let signal_text = last_exit
                    .get("signal")
                    .and_then(Value::as_str)
                    .unwrap_or("нет");
                let error_text = last_exit
                    .get("error")
                    .and_then(Value::as_str)
                    .map(|message| format!(", {message}"))
                    .unwrap_or_default();
                self.logger.error(&format!(
                    "[game-process] игра {game_id} завершилась с ошибкой: код {code_text}, \
                     сигнал {signal_text}, PID {pid_text}{error_text}"
                ));
            } else {
                self.logger.info(&format!(
                    "[game-process] игра {game_id} закрыта штатно (код {code_text}, PID {pid_text})"
                ));
            }

            locked
                .last_exits
                .insert(game_id.to_owned(), last_exit.clone());

            let payload = self.set_state_locked(
                &mut locked,
                game_id,
                outcome,
                vec![
                    (
                        "exitCode",
                        last_exit.get("exitCode").cloned().unwrap_or(Value::Null),
                    ),
                    (
                        "signal",
                        last_exit.get("signal").cloned().unwrap_or(Value::Null),
                    ),
                    ("lastExit", last_exit.clone()),
                ],
            );
            (last_exit, outcome, payload, episode)
        };

        self.emit_state(outcome, &payload);

        self.lock().remove(game_id);
        self.clear_persisted(game_id);

        self.emit(
            "finished",
            json!({"gameId": game_id, "lastExit": last_exit, "episode": episode}),
        );
        let mut final_state = self.public_state(game_id);
        if let Some(object) = final_state.as_object_mut() {
            // В 0.4.7 `lastExit` — один объект: обработчик `finished` мог признать выход
            // падением (`mark_last_exit_crashed`).
            let last_exit = self
                .lock()
                .last_exits
                .get(game_id)
                .cloned()
                .unwrap_or(last_exit);
            object.insert("lastExit".to_owned(), last_exit);
        }
        self.emit("state", final_state);
    }

    fn mark_last_exit_crashed(&self, game_id: &str) {
        if let Some(Value::Object(last_exit)) = self.lock().last_exits.get_mut(game_id) {
            last_exit.insert("outcome".to_owned(), json!("crashed"));
        }
    }

    fn mark_exited(&self, game_id: &str, pid: u32) -> bool {
        let stop_requested = {
            let locked = self.lock();
            let Some(entry) = locked.find(game_id) else {
                return false;
            };
            if record_pid(&entry.record) != Some(pid) {
                return false;
            }
            entry.stop_requested
        };
        self.handle_exit(game_id, stop_requested.then_some(0), None, None, true);
        true
    }

    fn unregister_process(&self, game_id: &str, last_exit: Option<&Value>) -> Value {
        let entry_last_exit = {
            let mut locked = self.lock();
            Self::stop_polling(&mut locked, game_id);
            locked
                .remove(game_id)
                .and_then(|entry| entry.record.get("lastExit").cloned())
        };
        self.clear_persisted(game_id);

        let last_exit = last_exit
            .filter(|value| js::truthy(value))
            .cloned()
            .or_else(|| entry_last_exit.filter(js::truthy))
            .unwrap_or(Value::Null);
        let mut payload = self.public_state(game_id);
        if let Some(object) = payload.as_object_mut() {
            object.insert("lastExit".to_owned(), last_exit);
        }
        self.emit("state", payload.clone());
        payload
    }

    // ------------------------------------------------------------ остановка

    async fn request_stop(
        &self,
        game_id: &str,
        options: StopOptions,
    ) -> Result<Value, DeliveryError> {
        let record = {
            let locked = self.lock();
            match locked.find(game_id) {
                Some(entry) if is_active(state_of(&entry.record)) => entry.record.clone(),
                _ => return Ok(json!({"status": "not-running"})),
            }
        };

        if !self.matches_record(&record) {
            return Err(DeliveryError::new(
                Code::ProcessMismatch,
                "PID больше не принадлежит запущенной игре — завершение отменено",
            )
            .with_details(json!({"gameId": game_id})));
        }

        if let Some(entry) = self.lock().find_mut(game_id) {
            entry.stop_requested = true;
        }
        self.set_state(game_id, "stopping", Vec::new());

        let pid = record_pid(&record).unwrap_or(0);
        let host = truthy(&record, "hostProcess");
        let result = match (host, options.force) {
            (true, true) => self.stopper.force_group(pid).await,
            (true, false) => self.stopper.graceful_group(pid).await,
            (false, true) => self.stopper.force(pid).await,
            (false, false) => self.stopper.graceful(pid).await,
        };

        if !result.ok && !options.force {
            self.logger.warn(&format!(
                "[game-process] запрос на закрытие {game_id} вернул ошибку: {}",
                js::trim(&result.output)
            ));
        }

        let deadline = (self.clock)() + options.timeout_ms.unwrap_or(self.stop_timeout_ms);

        while (self.clock)() < deadline {
            if self.lock().find(game_id).is_none() {
                return Ok(json!({"status": "stopped"}));
            }

            if !self.inspector.is_alive(pid) {
                self.handle_exit(game_id, Some(0), None, None, true);
                return Ok(json!({"status": "stopped"}));
            }

            tokio::time::sleep(Duration::from_millis(STOP_CHECK_MS)).await;
        }

        if self.lock().find(game_id).is_none() {
            return Ok(json!({"status": "stopped"}));
        }

        if options.force {
            return Err(DeliveryError::new(
                Code::StopFailed,
                "Процесс не удалось завершить даже принудительно",
            )
            .with_details(json!({"gameId": game_id})));
        }

        if let Some(entry) = self.lock().find_mut(game_id) {
            entry.stop_requested = false;
        }
        self.set_state(game_id, "running", Vec::new());

        Ok(json!({"status": "timeout", "timeoutMs": self.stop_timeout_ms}))
    }

    fn matches_record(&self, record: &Record) -> bool {
        let Some(pid) = record_pid(record) else {
            return false;
        };
        if !self.inspector.is_alive(pid) {
            return false;
        }

        if truthy(record, "hostProcess") {
            let game_id = string_of(record, "gameId").unwrap_or_default();
            let locked = self.lock();
            return locked
                .find(&game_id)
                .is_some_and(|live| live.child_pid == Some(pid));
        }

        let Some(actual) = self.inspector.inspect(pid) else {
            return false;
        };
        let expected_path = string_of(record, "executablePath");
        if !self
            .inspector
            .same_path(actual.executable_path.as_deref(), expected_path.as_deref())
        {
            return false;
        }

        // Файл состояния лежит в общей библиотеке, его может написать другой пользователь ПК:
        // папка игры берётся своя (`gameInstallPath`), а не из записи.
        let game_id = string_of(record, "gameId").unwrap_or_default();
        let Some(actual_path) = actual.executable_path.as_deref() else {
            return false;
        };
        if !is_game_id(&game_id) {
            return false;
        }
        let install = self.path.join(&[&self.games_dir, &game_id]);
        if assert_inside_with(
            &self.path,
            &install,
            actual_path,
            Some("Исполняемый файл игры"),
        )
        .is_err()
        {
            return false;
        }

        let started_at = string_of(record, "startedAt");
        if !self
            .inspector
            .same_start(started_at.as_deref(), actual.started_at.as_deref())
        {
            return false;
        }

        if let Some(key) = string_of(record, "processKey").filter(|key| !key.is_empty())
            && actual
                .started_at
                .as_deref()
                .is_some_and(|at| !at.is_empty())
        {
            let expected = process_key_of(
                f64::from(pid),
                expected_path.as_deref().filter(|path| !path.is_empty()),
                started_at.as_deref().filter(|at| !at.is_empty()),
            );
            if key != expected {
                return false;
            }
        }

        true
    }

    // ------------------------------------------------------------ после перезапуска лаунчера

    fn reconcile_after_launcher_restart(self: &Arc<Self>, game_ids: &[String]) -> Vec<Value> {
        let ids = if game_ids.is_empty() {
            self.persisted_game_ids()
        } else {
            game_ids.to_vec()
        };
        let mut results = Vec::new();

        for game_id in ids {
            let Some(record) = self.read_persisted(&game_id) else {
                continue;
            };
            let pid_text = js::to_template_string(record.get("pid"));

            if !self.matches_record(&record) {
                self.logger.info(&format!(
                    "[game-process] сохранённый процесс {game_id} (PID {pid_text}) больше не наш — \
                     состояние очищено"
                ));
                self.clear_persisted(&game_id);
                results.push(json!({"gameId": game_id, "adopted": false, "reason": "stale"}));
                let payload = self.public_state(&game_id);
                self.emit("state", payload);
                continue;
            }

            self.logger.warn(&format!(
                "[game-process] игра {game_id} всё ещё запущена (PID {pid_text}) — \
                 восстанавливаю состояние после перезапуска лаунчера"
            ));

            let mut adopted = record.clone();
            adopted.insert("state".to_owned(), json!("running"));
            adopted.insert("adopted".to_owned(), json!(true));
            self.lock().set(
                &game_id,
                Entry {
                    record: adopted,
                    child_pid: None,
                    timer: None,
                    stop_requested: false,
                },
            );

            self.start_polling(&game_id);
            self.set_state(&game_id, "running", vec![("adopted", json!(true))]);

            results.push(json!({
                "gameId": game_id,
                "adopted": true,
                "pid": record.get("pid").cloned().unwrap_or(Value::Null),
            }));
        }

        results
    }

    fn dispose(&self) {
        let mut locked = self.lock();
        for (_, entry) in &mut locked.tracked {
            if let Some(timer) = entry.timer.take() {
                timer.abort();
            }
        }
        locked.tracked.clear();
    }
}

impl ProcessProbe for GameProcessService {
    fn process_state(&self, game_id: &str) -> Value {
        self.public_state(game_id)
    }

    fn is_running(&self, game_id: &str) -> bool {
        GameProcessService::is_running(self, game_id)
    }

    fn any_running(&self) -> Option<String> {
        GameProcessService::any_running(self)
    }

    fn reconcile_after_launcher_restart(&self, game_ids: &[String]) -> Vec<Value> {
        GameProcessService::reconcile_after_launcher_restart(self, game_ids)
    }

    fn dispose(&self) {
        GameProcessService::dispose(self);
    }
}

impl ProcessStopper for GameProcessService {
    fn request_stop<'a>(
        &'a self,
        game_id: &'a str,
        force: bool,
    ) -> BoxFuture<'a, Result<Value, Failure>> {
        Box::pin(async move {
            self.inner
                .request_stop(
                    game_id,
                    StopOptions {
                        force,
                        timeout_ms: None,
                    },
                )
                .await
                .map_err(Failure::from)
        })
    }
}
