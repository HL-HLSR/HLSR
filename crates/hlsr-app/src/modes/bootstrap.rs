//! Установщик AppImage (`installer/bootstrap-main.js` 0.4.7, режим `Bootstrap`): окно выбора
//! каталога и установки (`hlsr_update::linux::bootstrap_installer`) — окно установщика задачи 10,
//! «HLSR — установка». Один экземпляр — по своему каталогу данных `<данные>/installer`
//! (`app.setPath("userData", …/installer)` и `requestSingleInstanceLock`); второй запуск выходит.
//! Ошибка до окна — окно ошибки и код 1 (`showError`).
//!
//! Режим запускается только на Linux; модуль собирается везде, чтобы типы проверялись и на Windows.

use super::window as mode_window;
use crate::dialogs;
use hlsr_core::args::LaunchArgs;
use hlsr_core::instance;
use hlsr_core::paths::{Paths, Resources};
use hlsr_online::online_profile::session_store::{SESSION_FILE, SessionStore, detect_vault};
use hlsr_update::linux::bootstrap_installer::{self, BootstrapOptions, BootstrapServices};
use hlsr_update::linux::{LinuxDirs, SystemRunner, embedded_payload};
use hlsr_update::update_manager::AppControl;
use std::sync::Arc;

const ERROR_TITLE: &str = "HLSR — ошибка установки / installer error";

/// `app.quit()` после «Открыть HLSR»: окно закрывается, когда цикл событий уже идёт.
struct QuitApp(Arc<dyn Fn() + Send + Sync>);

impl AppControl for QuitApp {
    fn quit(&self) {
        (self.0)();
    }
}

fn failed(error: &dyn std::fmt::Display) -> i32 {
    tracing::error!("[installer] {error}");
    dialogs::setup_error(ERROR_TITLE, &error.to_string());
    super::REFUSED
}

pub fn run(_args: &LaunchArgs, paths: &Paths) -> i32 {
    let own = Paths {
        data_dir: paths.data_dir.join("installer"),
        logs_dir: paths.logs_dir.clone(),
        app_name: paths.app_name,
    };
    let lock = match instance::try_lock(&own) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            tracing::info!("[installer] установщик уже открыт");
            return 0;
        }
        Err(error) => return failed(&error),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("hlsr-installer")
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return failed(&error),
    };
    let home = std::env::home_dir().unwrap_or_default();
    let env: Vec<(String, String)> = std::env::vars().collect();
    let dirs = LinuxDirs::from_env(&home, &env, &paths.data_dir);
    let services = BootstrapServices {
        dirs,
        env,
        runner: Arc::new(SystemRunner),
        launcher: super::launcher(),
        app: Arc::new(QuitApp(mode_window::quit_hook())),
        runtime: runtime.handle().clone(),
        forget_session: Arc::new(|dir| {
            SessionStore::at(dir.join(SESSION_FILE), detect_vault()).clear();
        }),
    };
    let language = if hlsr_core::language::system_locale()
        .unwrap_or_default()
        .starts_with("ru")
    {
        "ru"
    } else {
        "en"
    };
    let options = BootstrapOptions {
        root: None,
        payload_dir: embedded_payload::payload_dir(Resources::detect().dir()),
        language: language.to_owned(),
    };
    let window = bootstrap_installer::open_installer(options, services);
    let outcome = mode_window::run_installer("HLSR — установка", window.clone());
    window.closed();
    drop(lock);
    match outcome {
        Ok(()) => 0,
        Err(error) => failed(&error),
    }
}
