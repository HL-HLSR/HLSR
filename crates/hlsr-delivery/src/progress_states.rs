//! Состояния с прогрессом (`shared/progress-states.js`).

/// Состояния, в которых идёт операция.
pub const BUSY_STATES: &[&str] = &[
    "preparing",
    "downloading",
    "verifying",
    "extracting",
    "finalizing",
    "applying-update",
    "checking-files",
    "repairing",
    "exporting",
    "importing",
];

/// Состояния с полосой прогресса: занятые и пауза.
pub const PROGRESS_STATES: &[&str] = &[
    "preparing",
    "downloading",
    "verifying",
    "extracting",
    "finalizing",
    "applying-update",
    "checking-files",
    "repairing",
    "exporting",
    "importing",
    "paused",
];

/// Успешные итоги операции.
pub const SUCCESS_STATES: &[&str] = &["ready", "files-valid", "repair-complete"];

pub fn is_busy(state: &str) -> bool {
    BUSY_STATES.contains(&state)
}

pub fn is_progress(state: &str) -> bool {
    PROGRESS_STATES.contains(&state)
}

pub fn is_success(state: &str) -> bool {
    SUCCESS_STATES.contains(&state)
}

/// `shouldHoldFinish(previous, next)`: та же игра перешла из прогресса в успех —
/// полосе нужно дойти до конца. Статус — `(gameId, state)`; пустой `gameId` — нет игры.
pub fn should_hold_finish(previous: Option<(&str, &str)>, next: Option<(&str, &str)>) -> bool {
    let (Some((previous_game, previous_state)), Some((next_game, next_state))) = (previous, next)
    else {
        return false;
    };
    if previous_game.is_empty() || next_game.is_empty() || previous_game != next_game {
        return false;
    }
    is_success(next_state) && is_progress(previous_state)
}
