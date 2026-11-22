//! Помощники режимов из `hlsr-app/src/modes/mod.rs`, нужные службам лаунчера.

use hlsr_update::ports::Registry;
use hlsr_update::updater::processes::ProcessLauncher;
use hlsr_update::windows::processes::Env;
use std::path::PathBuf;
use std::sync::Arc;

pub fn dev_env(name: &str) -> Option<String> {
    if cfg!(any(debug_assertions, feature = "dev-hooks")) {
        std::env::var(name).ok().filter(|value| !value.is_empty())
    } else {
        None
    }
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

