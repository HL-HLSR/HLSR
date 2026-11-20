//! Режимы запуска, кроме лаунчера (`entry.js` и `index.js:59-62,168-179` 0.4.7): установщик и
//! удаление Windows, очистка после удаления, внешний апдейтер Windows, установщик AppImage.
//!
//! Установщик и удаление с `/S` работают тихо, без `/S` — в окне установщика 640×560; апдейтер —
//! в окне «HLSR Updater» 460×280 (без графики — тихо); установщик AppImage — в окне установщика
//! (задача 10 подпроекта 6, `window.rs`).
//!
//! Внешний мир — порты `hlsr-update`; в отладочной сборке и с `dev-hooks` он перенаправляется
//! (`HLSR_FAKE_REGISTRY`, `HLSR_KNOWN_FOLDERS`, `HLSR_TEMP_DIR`, `HLSR_LAUNCH_DRY_RUN`, задача 5):
//! сквозные проверки (`check-setup.ps1`, приёмка 0.4.7) не трогают настоящие реестр, ярлыки,
//! `%APPDATA%` и процессы. Задана часть перенаправлений — режим не запускается вовсе.

// Режим только Linux; собирается везде, чтобы типы проверялись и на Windows.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod bootstrap;
mod cleanup;
#[cfg(windows)]
mod setup;
mod uninstall_telemetry;
#[cfg(windows)]
mod updater;
pub mod window;

use hlsr_core::args::{LaunchArgs, LaunchMode};
use hlsr_core::paths::Paths;
use hlsr_update::ports::Registry;
use hlsr_update::updater::processes::ProcessLauncher;
use hlsr_update::windows::processes::Env;
use std::path::PathBuf;
use std::sync::Arc;

/// Отказ режима, которого здесь нет (не та ОС) или который ещё без окна.
const REFUSED: i32 = 1;

/// Запустить режим; код выхода процесса.
pub fn run(args: &LaunchArgs, paths: &Paths) -> i32 {
    // Сцена сверки окон (`hlsr-snap windows`): окно без работы режима.
    #[cfg(feature = "dev-hooks")]
    if let Some(scene) = dev_env("HLSR_SNAP_WINDOW") {
        return window::snap(&scene);
    }
    match args.mode {
        LaunchMode::UninstallCleanup => cleanup::run(paths),
        #[cfg(windows)]
        LaunchMode::Setup => setup::run_setup(args, paths),
        #[cfg(windows)]
        LaunchMode::Uninstall => setup::run_uninstall(args, paths),
        #[cfg(windows)]
        LaunchMode::Updater => updater::run(args),
        #[cfg(target_os = "linux")]
        LaunchMode::Bootstrap => bootstrap::run(args, paths),
        mode => {
            tracing::warn!(target: "launcher", "режим {mode:?} на этой ОС не запускается");
            REFUSED
        }
    }
}

/// Отладочная переменная окружения (`HLSR_RENDERER`, `HLSR_SMOKE_*`…) лаунчера и окон режимов:
/// только отладочная сборка и `dev-hooks` приложения; пустая — как нет.
pub fn dev_env(name: &str) -> Option<String> {
    if cfg!(any(debug_assertions, feature = "dev-hooks")) {
        std::env::var(name).ok().filter(|value| !value.is_empty())
    } else {
        None
    }
}

/// Режим, которому нужны перенаправления dev-hooks целиком, если задано хоть одно.
fn writes_user_state(mode: LaunchMode) -> bool {
    matches!(
        mode,
        LaunchMode::Setup
            | LaunchMode::Uninstall
            | LaunchMode::UninstallCleanup
            | LaunchMode::Updater
    )
}

/// Переменные перенаправления dev-hooks (задача 5) и каталоги лаунчера.
const REDIRECTS: [&str; 4] = [
    "HLSR_FAKE_REGISTRY",
    "HLSR_KNOWN_FOLDERS",
    "HLSR_TEMP_DIR",
    "HLSR_LAUNCH_DRY_RUN",
];
const DIRS: [&str; 2] = ["HLSR_DATA_DIR", "HLSR_LOGS_DIR"];

/// Отладочная сборка и `dev-hooks`: задано хоть одно перенаправление — нужны все четыре и
/// каталоги данных и журналов, иначе режим тронул бы настоящий профиль. Текст отказа — `Some`.
/// Выпуск без `dev-hooks` перенаправлений не знает.
pub fn redirect_problem(mode: LaunchMode, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    if !cfg!(any(debug_assertions, feature = "dev-hooks")) || !writes_user_state(mode) {
        return None;
    }
    let set = |name: &&str| env(name).is_some_and(|value| !value.is_empty());
    if !REDIRECTS.iter().any(set) {
        return None;
    }
    let missing: Vec<&str> = REDIRECTS
        .iter()
        .chain(DIRS.iter())
        .filter(|name| !set(name))
        .copied()
        .collect();
    (!missing.is_empty()).then(|| {
        format!(
            "перенаправления dev-hooks заданы не все, не хватает: {} — режим {mode:?} не запущен",
            missing.join(", ")
        )
    })
}

/// `process.env` режима: с перенаправленными папками пользователя под `dev-hooks`.
pub fn mode_env() -> Env {
    #[allow(unused_mut)]
    let mut env = Env::from_process();
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    hlsr_update::windows::dev_hooks::Redirects::from_env(&env.clone()).apply_env(&mut env);
    env
}

/// Значение перенаправления dev-hooks.
#[cfg(any(debug_assertions, feature = "dev-hooks"))]
fn redirect(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Реестр: файл JSON (`HLSR_FAKE_REGISTRY`) под `dev-hooks`, иначе настоящий (Windows).
pub fn registry() -> Arc<dyn Registry> {
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    if let Some(file) = redirect(hlsr_update::windows::dev_hooks::FAKE_REGISTRY) {
        return match hlsr_update::windows::dev_hooks::JsonFileRegistry::open(&file) {
            Ok(registry) => Arc::new(registry),
            Err(error) => {
                tracing::error!("[dev-hooks] {}: {error}", file.display());
                Arc::new(hlsr_update::ports::FakeRegistry::new())
            }
        };
    }
    #[cfg(windows)]
    {
        Arc::new(hlsr_update::windows::registry::SystemRegistry)
    }
    #[cfg(not(windows))]
    {
        Arc::new(hlsr_update::ports::FakeRegistry::new())
    }
}

/// Запуск процессов службы обновления и апдейтера: dry-run (`HLSR_LAUNCH_DRY_RUN`) под
/// `dev-hooks`, иначе настоящий.
pub fn launcher() -> Arc<dyn ProcessLauncher> {
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    if let Some(file) = redirect(hlsr_update::windows::dev_hooks::LAUNCH_DRY_RUN) {
        return Arc::new(hlsr_update::updater::processes::DryRunLauncher::new(&file));
    }
    Arc::new(hlsr_update::updater::processes::SystemLauncher)
}

/// `os.tmpdir()`: `HLSR_TEMP_DIR` под `dev-hooks`, иначе `TEMP`/`TMP` (Windows) или
/// `std::env::temp_dir`.
pub fn temp_dir(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    if let Some(dir) = redirect(hlsr_update::windows::dev_hooks::TEMP_DIR) {
        return dir;
    }
    if cfg!(windows) {
        PathBuf::from(hlsr_update::windows::known_folders::node_tmpdir(env))
    } else {
        std::env::temp_dir()
    }
}

/// Библиотеки Steam для очистки библиотеки (`steamZones` в `library-cleanup.js`): под
/// перенаправлениями dev-hooks — пусто (реестр Steam владельца не читается).
fn steam_libraries() -> Arc<dyn hlsr_update::ports::SteamLibraries> {
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    if REDIRECTS.iter().any(|name| redirect(name).is_some()) {
        return Arc::new(hlsr_update::ports::StaticSteamLibraries::default());
    }
    Arc::new(LocatedSteam)
}

/// Библиотеки найденного Steam (`SteamLocator`, ПП3); Steam не найден — пусто.
struct LocatedSteam;

impl hlsr_update::ports::SteamLibraries for LocatedSteam {
    fn libraries(&self) -> Vec<String> {
        #[cfg(windows)]
        {
            use hlsr_platform::steam_locator::{LocatorOptions, SteamLocator};
            let locator = SteamLocator::new(LocatorOptions::default());
            locator
                .locate_steam()
                .map(|steam| locator.steam_libraries(&steam))
                .unwrap_or_default()
        }
        #[cfg(not(windows))]
        {
            use hlsr_platform::linux::steam_locator::{LocatorOptions, SteamLocator};
            let locator = SteamLocator::new(LocatorOptions::default());
            locator
                .locate_steam()
                .map(|steam| locator.steam_libraries(&steam))
                .unwrap_or_default()
        }
    }
}

#[cfg(all(test, any(debug_assertions, feature = "dev-hooks")))]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn partial_redirects_refuse_the_modes_that_write() {
        let none = env_of(&[]);
        assert_eq!(redirect_problem(LaunchMode::Setup, &none), None);
        let partial = env_of(&[("HLSR_FAKE_REGISTRY", "r.json"), ("HLSR_DATA_DIR", "d")]);
        let problem = redirect_problem(LaunchMode::Uninstall, &partial).unwrap();
        assert!(
            problem
                .contains("HLSR_KNOWN_FOLDERS, HLSR_TEMP_DIR, HLSR_LAUNCH_DRY_RUN, HLSR_LOGS_DIR"),
            "{problem}"
        );
        assert!(redirect_problem(LaunchMode::UninstallCleanup, &partial).is_some());
        assert!(redirect_problem(LaunchMode::Updater, &partial).is_some());
        // Лаунчер и установщик AppImage перенаправлений не требуют.
        assert_eq!(redirect_problem(LaunchMode::Launcher, &partial), None);
        assert_eq!(redirect_problem(LaunchMode::Bootstrap, &partial), None);
        let full = env_of(&[
            ("HLSR_FAKE_REGISTRY", "r.json"),
            ("HLSR_KNOWN_FOLDERS", "home"),
            ("HLSR_TEMP_DIR", "temp"),
            ("HLSR_LAUNCH_DRY_RUN", "dry.jsonl"),
            ("HLSR_DATA_DIR", "data"),
            ("HLSR_LOGS_DIR", "logs"),
        ]);
        assert_eq!(redirect_problem(LaunchMode::Setup, &full), None);
        // Пустые значения не считаются.
        let empty = env_of(&[("HLSR_FAKE_REGISTRY", "")]);
        assert_eq!(redirect_problem(LaunchMode::Setup, &empty), None);
    }
}
