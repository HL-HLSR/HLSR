//! Vulkan для HL2 Ghosting (`game-delivery/vulkan-backend.js` 0.4.9): переключатель в «Параметрах
//! запуска» (настройка `vulkan`, у игры в `LAUNCH_CONFIG` — `vulkan: true`). Игра рисует через
//! Direct3D 9; прокси `d3d9.dll` лаунчера (`scripts/livesplit-overlay/proxy.h`) при
//! `HLSR_D3D9_BACKEND=vulkan` передаёт вызовы не системной `d3d9.dll`, а DXVK из
//! `backend\dxvk_d3d9.dll` рядом с `hl2.exe`. DXVK приносит сборка Ghosting (с 1.0.13), лаунчер его
//! не скачивает. Только Windows: под Proton d3d9 и так идёт через DXVK.
//!
//! Модуль чистый: его зовут и `game-settings-get` (есть ли переключатель), и запуск (окружение).

use crate::manifests::LaunchConfig;
use hlsr_core::js::Platform;
use hlsr_core::js::path::NodePath;
use std::collections::BTreeMap;
use std::path::Path;

pub const BACKEND: &str = "backend/dxvk_d3d9.dll";
pub const CONFIG_FILE: &str = "dxvk.conf";
/// Журнал и кэш шейдеров DXVK — в папке игры, как у `Play Vulkan.cmd` из сборки DXVK: локальный
/// диск рядом с игрой, уходят вместе с игрой, проверка файлов их пропускает (`mutable_paths`).
pub const LOG_DIR: &str = "renderer-logs";
pub const CACHE_DIR: &str = "renderer-cache";
/// Свои значения пользователя поменяли бы DXVK: `Play Vulkan.cmd` их тоже стирает.
pub const CLEARED: [&str; 3] = ["DXVK_CONFIG", "DXVK_HUD", "DXVK_FILTER_DEVICE_NAME"];
pub const MODE: &str = "HLSR_D3D9_BACKEND";

/// `supported(config, platform)`: у игры есть переключатель (только Windows).
pub fn supported(config: Option<&LaunchConfig>, platform: Platform) -> bool {
    platform == Platform::Win32 && config.is_some_and(|config| config.vulkan == Some(true))
}

/// `availability(installPath, config, fs, platform)`: `None` — у игры нет переключателя (или не
/// Windows), иначе есть ли DXVK в папке игры.
pub fn availability(
    install_path: Option<&Path>,
    config: Option<&LaunchConfig>,
    platform: Platform,
) -> Option<bool> {
    if !supported(config, platform) {
        return None;
    }
    Some(
        install_path
            .filter(|path| !path.as_os_str().is_empty())
            .is_some_and(|path| path.join(BACKEND).exists()),
    )
}

/// `deleteKey(env, name)`: без учёта регистра.
fn delete_key(env: &mut BTreeMap<String, String>, name: &str) {
    env.retain(|key, _| !key.eq_ignore_ascii_case(name));
}

/// `applyEnvironment(env, installPath, vulkan)`: окружение игры. Vulkan — как `Play Vulkan.cmd`
/// (без его аргументов запуска), иначе без `HLSR_D3D9_BACKEND`: прокси берёт системную `d3d9.dll`.
pub fn apply_environment(env: &mut BTreeMap<String, String>, install_path: &str, vulkan: bool) {
    delete_key(env, MODE);
    if !vulkan {
        return;
    }
    for name in CLEARED.iter().chain(&[
        "DXVK_LOG_PATH",
        "DXVK_SHADER_CACHE_PATH",
        "DXVK_CONFIG_FILE",
        "DXVK_LOG_LEVEL",
    ]) {
        delete_key(env, name);
    }
    let path = NodePath::native();
    env.insert(MODE.to_owned(), "vulkan".to_owned());
    env.insert(
        "DXVK_LOG_PATH".to_owned(),
        path.join(&[install_path, LOG_DIR]),
    );
    env.insert(
        "DXVK_SHADER_CACHE_PATH".to_owned(),
        path.join(&[install_path, CACHE_DIR]),
    );
    env.insert(
        "DXVK_CONFIG_FILE".to_owned(),
        path.join(&[install_path, CONFIG_FILE]),
    );
    env.insert("DXVK_LOG_LEVEL".to_owned(), "info".to_owned());
}
