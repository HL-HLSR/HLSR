//! `game-presence/game-state.js`: состояние игры для Discord — карта, частота кадров, таймер
//! забега. `apply` принимает сообщение `state` игры и даёт новое состояние; `should_publish`
//! решает, стоит ли показывать изменение (частота кадров — не чаще раза в 15 с).

use super::protocol::{Lifecycle, StateMessage};
use hlsr_core::json_text::js_number;
use hlsr_core::map_assets;
use serde_json::{Value, json};

/// Допуск, с которым начало забега считается прежним.
pub const ANCHOR_TOLERANCE_MS: i64 = 1500;
/// Как часто можно публиковать изменение одной только частоты кадров.
pub const FPS_MIN_INTERVAL_MS: i64 = 15_000;

/// Состояние игры (`emptyState()` и далее).
#[derive(Debug, Clone, PartialEq)]
pub struct GameState {
    pub active: bool,
    pub session: Option<String>,
    /// Номер последнего принятого сообщения; до первого — `-1`.
    pub last_seq: f64,
    pub lifecycle: Option<Lifecycle>,
    pub map: Option<String>,
    pub map_known: bool,
    pub fps: Option<i64>,
    pub running: bool,
    pub run_started_at_ms: Option<i64>,
    pub final_time: bool,
    pub run_seen_on_map: Option<String>,
}

impl Default for GameState {
    fn default() -> Self {
        GameState::empty()
    }
}

impl GameState {
    /// `emptyState()`.
    pub fn empty() -> GameState {
        GameState {
            active: false,
            session: None,
            last_seq: -1.0,
            lifecycle: None,
            map: None,
            map_known: false,
            fps: None,
            running: false,
            run_started_at_ms: None,
            final_time: false,
            run_seen_on_map: None,
        }
    }
}

impl GameState {
    /// Объект JS `emptyState()` и далее, в порядке ключей 0.4.7.
    pub fn to_value(&self) -> Value {
        json!({
            "active": self.active,
            "session": self.session,
            "lastSeq": js_number(self.last_seq),
            "lifecycle": self.lifecycle.map(Lifecycle::as_str),
            "map": self.map,
            "mapKnown": self.map_known,
            "fps": self.fps,
            "running": self.running,
            "runStartedAtMs": self.run_started_at_ms,
            "finalTime": self.final_time,
            "runSeenOnMap": self.run_seen_on_map,
        })
    }

    /// `snapshot()` моста: то, что видит Discord.
    pub fn snapshot(&self) -> Value {
        json!({
            "active": self.active,
            "lifecycle": self.lifecycle.map(Lifecycle::as_str),
            "map": self.map,
            "fps": self.fps,
            "running": self.running,
            "runStartedAtMs": self.run_started_at_ms,
            "finalTime": self.final_time,
        })
    }
}

/// `sameState(a, b)`: то, что видно в Discord (сеанс и номер сообщения не сравниваются).
pub fn same_state(a: &GameState, b: &GameState) -> bool {
    a.active == b.active
        && a.lifecycle == b.lifecycle
        && a.map == b.map
        && a.map_known == b.map_known
        && a.fps == b.fps
        && a.running == b.running
        && a.run_started_at_ms == b.run_started_at_ms
        && a.final_time == b.final_time
}

/// `sameExceptFps(a, b)`.
pub fn same_except_fps(a: &GameState, b: &GameState) -> bool {
    a.active == b.active
        && a.lifecycle == b.lifecycle
        && a.map == b.map
        && a.map_known == b.map_known
        && a.running == b.running
        && a.run_started_at_ms == b.run_started_at_ms
        && a.final_time == b.final_time
}

/// Итог `apply`: новое состояние, изменилось ли оно (для Discord) и почему сообщение пропущено.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub state: GameState,
    pub changed: bool,
    /// `NOT_STATE` или `STALE_SEQ`.
    pub ignored: Option<&'static str>,
}

/// `apply(previous, message, nowMs)`; `None` — сообщение не `state`.
pub fn apply(previous: &GameState, message: Option<&StateMessage>, now_ms: i64) -> Applied {
    let Some(message) = message else {
        return Applied {
            state: previous.clone(),
            changed: false,
            ignored: Some("NOT_STATE"),
        };
    };
    apply_state(previous, message, now_ms)
}

fn apply_state(prior: &GameState, message: &StateMessage, now_ms: i64) -> Applied {
    let new_session = prior.session.as_deref() != Some(message.session.as_str());
    let base = if new_session {
        GameState::empty()
    } else {
        prior.clone()
    };

    if !new_session && message.seq <= base.last_seq {
        return Applied {
            state: prior.clone(),
            changed: false,
            ignored: Some("STALE_SEQ"),
        };
    }

    let mut next = base.clone();
    next.session = Some(message.session.clone());
    next.last_seq = message.seq;
    next.lifecycle = Some(message.lifecycle);

    if message.lifecycle == Lifecycle::Exited {
        let mut gone = GameState::empty();
        gone.session = Some(message.session.clone());
        gone.last_seq = message.seq;
        gone.lifecycle = Some(Lifecycle::Exited);
        return Applied {
            changed: !same_state(prior, &gone),
            state: gone,
            ignored: None,
        };
    }

    next.active = true;
    next.fps = message.fps;

    if message.lifecycle == Lifecycle::Menu {
        next.map = None;
        next.map_known = false;
        next.running = false;
        next.run_started_at_ms = None;
        next.final_time = false;
        return Applied {
            changed: !same_state(prior, &next),
            state: next,
            ignored: None,
        };
    }

    let map_changed = base.map != message.map_name;
    next.map = message.map_name.clone();
    next.map_known = map_assets::is_known_map(message.map_name.as_deref());

    if map_changed {
        next.run_started_at_ms = None;
        next.run_seen_on_map = None;
    }

    next.running = message.timer_running;

    if message.timer_running {
        next.run_seen_on_map = message.map_name.clone();
        // Для идущего таймера протокол требует `elapsed`.
        let candidate = now_ms - message.elapsed_ms.unwrap_or(0);
        let anchor = if map_changed {
            None
        } else {
            base.run_started_at_ms
        };
        let keep = base.running
            && anchor.is_some_and(|anchor| (candidate - anchor).abs() <= ANCHOR_TOLERANCE_MS);
        next.run_started_at_ms = if keep { anchor } else { Some(candidate) };
        next.final_time = false;
    } else {
        next.run_started_at_ms = None;
        let was_reset = message.elapsed_ms == Some(0);
        next.final_time = message.timer_completed
            && !was_reset
            && next.run_seen_on_map == next.map
            && next.map.as_deref() == Some(map_assets::FINAL_TIME_MAP);
    }

    Applied {
        changed: !same_state(prior, &next),
        state: next,
        ignored: None,
    }
}

/// `shouldPublish(previous, next, lastFpsPublishAtMs, nowMs)`.
pub fn should_publish(
    previous: Option<&GameState>,
    next: &GameState,
    last_fps_publish_at_ms: Option<i64>,
    now_ms: i64,
) -> bool {
    let Some(previous) = previous else {
        return true;
    };
    if same_state(previous, next) {
        return false;
    }
    if !same_except_fps(previous, next) {
        return true;
    }
    match last_fps_publish_at_ms {
        None => true,
        Some(last) => now_ms - last >= FPS_MIN_INTERVAL_MS,
    }
}
