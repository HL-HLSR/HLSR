//! Тестовый запуск в QA-режиме (`qa-launch.js`): вместо игры — exe из переменной окружения.
//! Здесь — только признак и описание для `game-delivery-info`; сам запуск — подсистема запуска.

use crate::environment::Environment;
use crate::qa_mode;
use serde_json::{Value, json};

pub const EXE_FLAG: &str = "HLSR_GAME_DELIVERY_QA_LAUNCH_EXE";
pub const ARGS_FLAG: &str = "HLSR_GAME_DELIVERY_QA_LAUNCH_ARGS";
pub const DELAY_FLAG: &str = "HLSR_GAME_DELIVERY_QA_LAUNCH_DELAY";

/// `isEnabled(env)`: QA-режим и задан exe.
pub fn is_enabled(env: &Environment) -> bool {
    if env.is_production() || !qa_mode::is_enabled(env) {
        return false;
    }
    env.var(EXE_FLAG).is_some_and(|exe| !exe.is_empty())
}

/// `describe(env)`: `{enabled, executable, args}`.
pub fn describe(env: &Environment) -> Value {
    let given = |name: &str| env.var(name).filter(|value| !value.is_empty());
    json!({
        "enabled": is_enabled(env),
        "executable": given(EXE_FLAG),
        "args": given(ARGS_FLAG),
    })
}
