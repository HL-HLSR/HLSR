//! `game-stats/GameStatsStore.js`: сколько времени игрок провёл в каждой игре и сколько раз её
//! запускал. Файл `game-stats.json` (`{schemaVersion: 1, games: {<id>: {...}}, updatedAt}`, два
//! пробела отступа, запись атомарная) хранит итоги и открытую сессию; пока игра идёт, раз в 30 с
//! (`heartbeat`) в неё пишется прожитое время, поэтому после падения лаунчера открытая сессия
//! восстанавливается по последнему heartbeat. Повреждённый файл откладывается как
//! `game-stats.json.corrupt-<мс в 36-ричной системе>` (один раз за жизнь хранилища), о чём
//! говорит уведомление `GAME_STATS_CORRUPT`.
//!
//! Файл читается и пишется как значение JSON (`serde_json` с порядком ключей): лишние поля и
//! порядок ключей сохраняются, как у 0.4.7. Секунды — монотонные часы целыми секундами
//! (`process.hrtime()[0]`), поэтому две отметки в одной секунде дают 0.

use hlsr_core::js;
use hlsr_core::json_store::read_json_safe;
use hlsr_core::json_text::{js_key_order, js_number};
use hlsr_core::logger::{Logger, TracingLogger};
use hlsr_core::notifications::NotificationStore;
use hlsr_delivery::atomic::write_json_atomic;
use hlsr_delivery::errors::RawError;
use hlsr_delivery::timing::{Clock, iso_from_ms, system_clock};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};
use tokio::runtime::Handle;
use tokio::task::AbortHandle;

pub const SCHEMA_VERSION: u64 = 1;
pub const FILE_NAME: &str = "game-stats.json";
/// Как часто пишется прожитое время открытой сессии.
pub const HEARTBEAT_MS: u64 = 30 * 1000;

/// Монотонные часы в целых секундах (`monotonicSeconds`).
pub type Monotonic = Arc<dyn Fn() -> i64 + Send + Sync>;

/// `process.hrtime()[0]`: целые секунды от произвольного начала.
pub fn monotonic_seconds() -> Monotonic {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    let origin = *ORIGIN.get_or_init(Instant::now);
    Arc::new(move || i64::try_from(origin.elapsed().as_secs()).unwrap_or(i64::MAX))
}

/// Центр уведомлений для `GAME_STATS_CORRUPT` (`notifications.add({level, code, detail})`).
pub trait StatsNotifications: Send + Sync {
    fn add(&self, level: &str, code: &str, detail: &str) -> Result<(), String>;
}

impl StatsNotifications for NotificationStore {
    fn add(&self, level: &str, code: &str, detail: &str) -> Result<(), String> {
        NotificationStore::add(self, level, code, Some(detail), None)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// Параметры конструктора (`options`).
#[derive(Default)]
pub struct StatsOptions {
    pub logger: Option<Arc<dyn Logger>>,
    pub notifications: Option<Arc<dyn StatsNotifications>>,
    pub now: Option<Clock>,
    pub monotonic: Option<Monotonic>,
    /// `heartbeatMs`; по умолчанию [`HEARTBEAT_MS`].
    pub heartbeat_ms: Option<u64>,
    /// Среда tokio для таймера heartbeat; по умолчанию — текущая (если она есть).
    pub runtime: Option<Handle>,
}

/// Итог закрытой сессии (`sessionEnd`).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEnd {
    pub game_id: String,
    pub seconds: f64,
    pub total_play_seconds: f64,
}

impl SessionEnd {
    /// Объект JS `{gameId, seconds, totalPlaySeconds}`.
    pub fn to_value(&self) -> Value {
        json!({
            "gameId": self.game_id,
            "seconds": js_number(self.seconds),
            "totalPlaySeconds": js_number(self.total_play_seconds),
        })
    }
}

struct State {
    /// `active`: игра → монотонные секунды начала (порядок вставки, как у `Map`).
    active: Vec<(String, i64)>,
    heartbeat: Option<AbortHandle>,
    corrupt: bool,
}

pub struct GameStatsStore {
    file: PathBuf,
    logger: Arc<dyn Logger>,
    notifications: Option<Arc<dyn StatsNotifications>>,
    now: Clock,
    monotonic: Monotonic,
    heartbeat_ms: u64,
    runtime: Option<Handle>,
    me: Weak<GameStatsStore>,
    state: Mutex<State>,
}

/// `Math.max(0, Math.floor(value || 0))`.
fn seconds_of(value: Option<&Value>) -> f64 {
    let number = js::value::to_number(value);
    let number = if number.is_nan() || number == 0.0 {
        0.0
    } else {
        number
    };
    number.floor().max(0.0)
}

fn truthy(value: Option<&Value>) -> bool {
    value.is_some_and(js::truthy)
}

fn new_game(game_id: &str) -> Value {
    json!({
        "gameId": game_id,
        "totalPlaySeconds": 0,
        "lastLaunchAt": null,
        "lastExitAt": null,
        "lastSessionSeconds": null,
        "launchCount": 0,
        "currentSession": null,
    })
}

/// `games` документа.
fn games_of(all: &mut Map<String, Value>) -> &mut Map<String, Value> {
    all.get_mut("games")
        .and_then(Value::as_object_mut)
        .expect("readAll отдаёт games-объект")
}

/// `gameOf(all, gameId)`: запись игры; нет (или пуста) — новая.
fn game_of<'a>(all: &'a mut Map<String, Value>, game_id: &str) -> &'a mut Map<String, Value> {
    let games = games_of(all);
    if !games
        .get(game_id)
        .is_some_and(|game| js::truthy(game) && game.is_object())
    {
        games.insert(game_id.to_owned(), new_game(game_id));
    }
    games
        .get_mut(game_id)
        .and_then(Value::as_object_mut)
        .expect("запись игры — объект")
}

impl GameStatsStore {
    /// Хранилище файла `file`; открытые сессии прошлого запуска закрываются (`recoverInterrupted`).
    pub fn new(
        file: impl Into<PathBuf>,
        options: StatsOptions,
    ) -> Result<Arc<GameStatsStore>, RawError> {
        let store = Arc::new_cyclic(|me| GameStatsStore {
            file: file.into(),
            logger: options.logger.unwrap_or_else(|| Arc::new(TracingLogger)),
            notifications: options.notifications,
            now: options.now.unwrap_or_else(system_clock),
            monotonic: options.monotonic.unwrap_or_else(monotonic_seconds),
            heartbeat_ms: options.heartbeat_ms.unwrap_or(HEARTBEAT_MS),
            runtime: options.runtime.or_else(|| Handle::try_current().ok()),
            me: me.clone(),
            state: Mutex::new(State {
                active: Vec::new(),
                heartbeat: None,
                corrupt: false,
            }),
        });
        store.recover_interrupted()?;
        Ok(store)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    /// Работает ли таймер heartbeat (`heartbeatTimer !== null`).
    pub fn timer_active(&self) -> bool {
        self.state().heartbeat.is_some()
    }

    /// Игры с открытой сессией (ключи `active`).
    pub fn active_games(&self) -> Vec<String> {
        self.state()
            .active
            .iter()
            .map(|(game_id, _)| game_id.clone())
            .collect()
    }

    /// `readAll()`: документ; нет файла или он не принят — пустой (чужой файл откладывается).
    fn read_all(&self, state: &mut State) -> Map<String, Value> {
        let empty = || {
            let mut all = Map::new();
            all.insert("schemaVersion".into(), Value::from(SCHEMA_VERSION));
            all.insert("games".into(), Value::Object(Map::new()));
            all
        };
        if !self.file.exists() {
            return empty();
        }
        match read_json_safe(&self.file) {
            Value::Object(all)
                if all.get("schemaVersion").and_then(Value::as_f64)
                    == Some(SCHEMA_VERSION as f64)
                    && all.get("games").is_some_and(Value::is_object) =>
            {
                all
            }
            _ => {
                self.quarantine(state);
                empty()
            }
        }
    }

    /// `quarantine()`: отложить повреждённый файл (один раз за жизнь хранилища).
    fn quarantine(&self, state: &mut State) {
        if state.corrupt {
            return;
        }
        state.corrupt = true;

        let mut name = self.file.as_os_str().to_owned();
        name.push(format!(
            ".corrupt-{}",
            js::base36((self.now)().max(0) as u128)
        ));
        let target = PathBuf::from(name);
        match std::fs::rename(&self.file, &target) {
            Ok(()) => self.logger.warn(&format!(
                "[game-stats] повреждённый файл карантинирован: {}",
                target
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            )),
            Err(error) => {
                let raw = RawError::fs_with_dest(&error, "rename", &self.file, Some(&target));
                self.logger
                    .warn(&format!("[game-stats] карантин не удался: {}", raw.message));
            }
        }

        if let Some(notifications) = &self.notifications {
            // Уведомление вторично.
            let _ = notifications.add(
                "warning",
                "GAME_STATS_CORRUPT",
                "статистика игры сброшена из-за повреждения файла",
            );
        }
    }

    /// `write(all)`: `updatedAt` и атомарная запись.
    fn write(&self, mut all: Map<String, Value>) -> Result<(), RawError> {
        all.insert("updatedAt".into(), Value::String(iso_from_ms((self.now)())));
        write_json_atomic(&self.file, &Value::Object(all))
    }

    /// `recoverInterrupted()`.
    fn recover_interrupted(&self) -> Result<(), RawError> {
        let mut state = self.state();
        let mut all = self.read_all(&mut state);
        let mut changed = false;

        let games = games_of(&mut all);
        let ids: Vec<String> = js_key_order(games)
            .into_iter()
            .map(|(key, _)| key.clone())
            .collect();
        for game_id in ids {
            let Some(game) = games.get_mut(&game_id).and_then(Value::as_object_mut) else {
                continue;
            };
            let Some(session) = game
                .get("currentSession")
                .filter(|session| js::truthy(session))
                .cloned()
            else {
                continue;
            };

            let credited = seconds_of(session.get("heartbeatSeconds"));
            let total = js::value::to_number(game.get("totalPlaySeconds")) + credited;
            game.insert("totalPlaySeconds".into(), js_number(total));
            game.insert("lastSessionSeconds".into(), js_number(credited));
            let last_exit = ["lastHeartbeatAt", "startedAt"]
                .iter()
                .filter_map(|key| session.get(*key))
                .find(|value| js::truthy(value))
                .cloned()
                .unwrap_or(Value::Null);
            game.insert("lastExitAt".into(), last_exit);
            game.insert("currentSession".into(), Value::Null);
            changed = true;

            self.logger.warn(&format!(
                "[game-stats] незакрытая сессия {game_id} восстановлена по heartbeat: +{} c",
                js::to_string(&js_number(credited))
            ));
        }

        if changed {
            self.write(all)?;
        }
        Ok(())
    }

    /// `sessionStart(gameId, info)`: игра запущена; `pid` — из `info.pid`, 0 и пусто — `null`.
    pub fn session_start(&self, game_id: &str, pid: Option<u64>) -> Result<(), RawError> {
        let mut state = self.state();
        let mut all = self.read_all(&mut state);
        let started_at = iso_from_ms((self.now)());

        let game = game_of(&mut all, game_id);
        let launches = js::value::to_number(game.get("launchCount")) + 1.0;
        game.insert("launchCount".into(), js_number(launches));
        game.insert("lastLaunchAt".into(), Value::String(started_at.clone()));
        game.insert(
            "currentSession".into(),
            json!({
                "startedAt": started_at,
                "lastHeartbeatAt": started_at,
                "heartbeatSeconds": 0,
                "pid": pid.filter(|pid| *pid != 0),
            }),
        );

        let started = (self.monotonic)();
        match state.active.iter_mut().find(|(id, _)| id == game_id) {
            Some(entry) => entry.1 = started,
            None => state.active.push((game_id.to_owned(), started)),
        }
        self.write(all)?;
        self.ensure_heartbeat(&mut state);
        Ok(())
    }

    fn ensure_heartbeat(&self, state: &mut State) {
        if state.heartbeat.is_some() || state.active.is_empty() {
            return;
        }
        let Some(runtime) = &self.runtime else {
            self.logger
                .warn("[game-stats] таймер heartbeat не запущен: нет среды tokio");
            return;
        };
        let me = self.me.clone();
        let every = Duration::from_millis(self.heartbeat_ms.max(1));
        let task = runtime.spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                let Some(store) = me.upgrade() else {
                    break;
                };
                // В 0.4.7 исключение в таймере осталось бы необработанным; здесь — строка в журнал.
                if let Err(error) = store.heartbeat() {
                    store.logger.warn(&format!(
                        "[game-stats] heartbeat не записан: {}",
                        error.message
                    ));
                }
            }
        });
        state.heartbeat = Some(task.abort_handle());
    }

    fn stop_heartbeat_if_idle(&self, state: &mut State) {
        if !state.active.is_empty() {
            return;
        }
        if let Some(timer) = state.heartbeat.take() {
            timer.abort();
        }
    }

    /// `heartbeat()`: прожитое время открытых сессий — в файл.
    pub fn heartbeat(&self) -> Result<(), RawError> {
        let mut state = self.state();
        if state.active.is_empty() {
            self.stop_heartbeat_if_idle(&mut state);
            return Ok(());
        }

        let mut all = self.read_all(&mut state);
        let mut changed = false;
        let now = (self.monotonic)();

        for (game_id, started) in state.active.clone() {
            let game = game_of(&mut all, &game_id);
            let Some(session) = game
                .get_mut("currentSession")
                .filter(|session| js::truthy(session))
                .and_then(Value::as_object_mut)
            else {
                continue;
            };
            session.insert(
                "heartbeatSeconds".into(),
                Value::from((now - started).max(0)),
            );
            session.insert(
                "lastHeartbeatAt".into(),
                Value::String(iso_from_ms((self.now)())),
            );
            changed = true;
        }

        if changed {
            self.write(all)?;
        }
        Ok(())
    }

    /// `sessionEnd(gameId)`: итог сессии или `None`, если открытой сессии нет.
    pub fn session_end(&self, game_id: &str) -> Result<Option<SessionEnd>, RawError> {
        let mut state = self.state();
        let started = state
            .active
            .iter()
            .find(|(id, _)| id == game_id)
            .map(|(_, started)| *started);
        let mut all = self.read_all(&mut state);
        let game = game_of(&mut all, game_id);

        let seconds = if let Some(started) = started {
            let seconds = ((self.monotonic)() - started).max(0) as f64;
            state.active.retain(|(id, _)| id != game_id);
            seconds
        } else if let Some(session) = game
            .get("currentSession")
            .filter(|session| js::truthy(session))
        {
            seconds_of(session.get("heartbeatSeconds"))
        } else {
            self.stop_heartbeat_if_idle(&mut state);
            return Ok(None);
        };

        let total = js::value::to_number(game.get("totalPlaySeconds")) + seconds;
        game.insert("totalPlaySeconds".into(), js_number(total));
        game.insert("lastSessionSeconds".into(), js_number(seconds));
        game.insert(
            "lastExitAt".into(),
            Value::String(iso_from_ms((self.now)())),
        );
        game.insert("currentSession".into(), Value::Null);

        self.write(all)?;
        self.stop_heartbeat_if_idle(&mut state);

        Ok(Some(SessionEnd {
            game_id: game_id.to_owned(),
            seconds,
            total_play_seconds: total,
        }))
    }

    /// `summary(gameId)`: то, что показывает окно (`game-stats-get`).
    pub fn summary(&self, game_id: &str) -> Value {
        let mut state = self.state();
        let all = self.read_all(&mut state);
        let game = all
            .get("games")
            .and_then(Value::as_object)
            .and_then(|games| games.get(game_id))
            .and_then(Value::as_object);

        let Some(game) = game.filter(|game| truthy(game.get("launchCount"))) else {
            return json!({"available": false, "gameId": game_id});
        };

        let mut summary = Map::new();
        summary.insert("available".into(), Value::Bool(true));
        summary.insert("gameId".into(), Value::String(game_id.to_owned()));
        for key in [
            "totalPlaySeconds",
            "lastLaunchAt",
            "lastExitAt",
            "lastSessionSeconds",
            "launchCount",
        ] {
            // Нет поля — `undefined`: в ответе JSON такого ключа нет.
            if let Some(value) = game.get(key) {
                summary.insert(key.into(), value.clone());
            }
        }
        let session_active =
            state.active.iter().any(|(id, _)| id == game_id) || truthy(game.get("currentSession"));
        summary.insert("sessionActive".into(), Value::Bool(session_active));
        Value::Object(summary)
    }

    /// `dispose()`: остановить таймер heartbeat.
    pub fn dispose(&self) {
        if let Some(timer) = self.state().heartbeat.take() {
            timer.abort();
        }
    }
}

impl Drop for GameStatsStore {
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(timer) = state.heartbeat.take() {
            timer.abort();
        }
    }
}
