//! Окружение дочерних процессов (решение 3 спецификации ПП3): как у 0.4.7, но без переменных,
//! которые лаунчер ставит для своего окна (масштаб Slint и winit: `SLINT_SCALE_FACTOR`,
//! `WINIT_X11_SCALE_FACTOR` и прочие `SLINT_*`/`WINIT_*`). Один список для всех, кого лаунчер
//! запускает: игры (Windows и Proton), инструменты, Steam, `tar`, `7za`.

use std::collections::BTreeMap;

/// Окружение дочернего процесса (`env` у `spawn`).
pub type Env = BTreeMap<String, String>;

/// Переменные масштаба окна, которые лаунчер ставит сам перед созданием окна.
pub const UI_ENVIRONMENT: [&str; 2] = ["SLINT_SCALE_FACTOR", "WINIT_X11_SCALE_FACTOR"];

/// Переменная Slint или winit: игре, Wine и инструментам она не нужна и меняет их масштаб.
pub fn is_launcher_ui_var(name: &str) -> bool {
    name.starts_with("SLINT_") || name.starts_with("WINIT_")
}

/// `process.env` без переменных Slint и winit.
pub fn process_env() -> Env {
    std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .filter(|(name, _)| !is_launcher_ui_var(name))
        .collect()
}

/// Окружение без переменных Slint и winit.
pub fn without_ui_vars(env: Env) -> Env {
    env.into_iter()
        .filter(|(name, _)| !is_launcher_ui_var(name))
        .collect()
}

/// Процесс наследует окружение лаунчера, но без переменных Slint и winit: и тех, что заданы
/// команде явно, и тех, что есть у самого лаунчера.
pub fn strip_ui_environment(command: &mut tokio::process::Command) {
    for name in UI_ENVIRONMENT {
        command.env_remove(name);
    }
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(is_launcher_ui_var) {
            command.env_remove(name);
        }
    }
}
