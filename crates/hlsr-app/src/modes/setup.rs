//! Установщик и удаление Windows (`windows-install/setup.js` 0.4.7, Р1): `HLSR-Setup-<v>.exe` — этот
//! же exe с нагрузкой (или `tree\HLSR.exe` с `PORTABLE_EXECUTABLE_FILE`, путь В), удаление —
//! `HLSR.exe --hlsr-uninstall`. Тихие режимы (`/S`) — `hlsr_update::windows::setup`: коды 0/1/3/4,
//! ожидание лаунчера (0.4.7 и 0.5) до 60 с, запуск после установки только с `--force-run`. Без
//! `/S` — окна установки и удаления (задача 10, `modes::window`): «HLSR — установка» и
//! «HLSR — удаление», код выхода 0 после закрытия окна.

use super::uninstall_telemetry::OnlineUninstallTelemetry;
use super::window as mode_window;
use crate::dialogs;
use hlsr_core::args::{LaunchArgs, PORTABLE_EXECUTABLE_FILE, SetupArgs};
use hlsr_core::instance::{self, PrimaryInstance};
use hlsr_core::paths::Paths;
use hlsr_update::errors::setup_exit;
use hlsr_update::installer_window::InstallerWindow;
use hlsr_update::ports::{InstanceLock, KnownFolders, Processes};
use hlsr_update::windows::setup::{
    self, LegacyAwareLock, SetupBackend, SetupServices, UNINSTALL_RUNNING_MESSAGE, UninstallBackend,
};
use hlsr_update::windows::shortcuts::SystemShellLinks;
use std::sync::{Arc, Mutex};

/// Блокировка первого экземпляра 0.5 (`requestSingleInstanceLock`): только замок, без канала.
struct CoreLock {
    paths: Paths,
    held: Mutex<Option<PrimaryInstance>>,
}

impl InstanceLock for CoreLock {
    fn try_acquire(&self) -> bool {
        let Ok(mut held) = self.held.lock() else {
            return false;
        };
        if held.is_some() {
            return true;
        }
        match instance::try_lock(&self.paths) {
            Ok(Some(lock)) => {
                *held = Some(lock);
                true
            }
            Ok(None) => false,
            Err(error) => {
                tracing::warn!("[setup] блокировка экземпляра: {error}");
                false
            }
        }
    }

    fn release(&self) {
        if let Ok(mut held) = self.held.lock() {
            *held = None;
        }
    }

    fn is_held(&self) -> bool {
        self.held.lock().is_ok_and(|held| held.is_some())
    }
}

/// Известные папки: перенаправленные (`HLSR_KNOWN_FOLDERS`) под `dev-hooks`, иначе оболочки.
fn folders() -> Arc<dyn KnownFolders> {
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    {
        let env = hlsr_update::windows::processes::Env::from_process();
        if let Some(folders) = hlsr_update::windows::dev_hooks::Redirects::from_env(&env).folders()
        {
            return Arc::new(folders);
        }
    }
    Arc::new(hlsr_update::windows::known_folders::SystemKnownFolders)
}

/// Процессы установщика: dry-run (`HLSR_LAUNCH_DRY_RUN`) под `dev-hooks`, иначе настоящие.
fn processes() -> Arc<dyn Processes> {
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    if let Some(file) = std::env::var_os(hlsr_update::windows::dev_hooks::LAUNCH_DRY_RUN)
        .filter(|value| !value.is_empty())
    {
        return Arc::new(hlsr_update::windows::processes::DryRunProcesses::new(
            std::path::Path::new(&file),
        ));
    }
    Arc::new(hlsr_update::windows::processes::SystemProcesses)
}

/// Службы режимов установщика (`services(electron, identity)` 0.4.7).
fn services(paths: &Paths) -> std::io::Result<SetupServices> {
    let exe = std::env::current_exe()?;
    Ok(SetupServices {
        registry: super::registry(),
        links: Arc::new(SystemShellLinks),
        folders: folders(),
        processes: processes(),
        lock: Arc::new(LegacyAwareLock::new(
            &paths.data_dir,
            Arc::new(CoreLock {
                paths: paths.clone(),
                held: Mutex::default(),
            }),
        )),
        log: Arc::new(hlsr_update::ports::TracingLogger),
        env: super::mode_env(),
        sleep: Arc::new(std::thread::sleep),
        user_data: paths.data_dir.clone(),
        app_name: paths.app_name.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        locale: hlsr_core::language::system_locale().unwrap_or_default(),
        exe,
        steam: super::steam_libraries(),
        telemetry: Some(Arc::new(OnlineUninstallTelemetry)),
        shell: true,
    })
}

/// Ошибка до окна: в журнал и, без `/S`, окном (`run(...).catch` 0.4.7); код 1.
fn failed(mode: &str, silent: bool, error: &dyn std::fmt::Display) -> i32 {
    tracing::error!("[{mode}] {error}");
    if !silent {
        dialogs::setup_error("HLSR", &error.to_string());
    }
    setup_exit::FAILED
}

/// `HLSR-Setup-<v>.exe [/S] [--updated] [--force-run] [/D=<каталог>]`.
pub fn run_setup(args: &LaunchArgs, paths: &Paths) -> i32 {
    let setup_args = args.setup.clone().unwrap_or_default();
    let services = match services(paths) {
        Ok(services) => services,
        Err(error) => return failed("setup", setup_args.silent, &error),
    };
    let exe = services.exe.clone();
    // Путь В апдейтера 0.4.7: `tree\HLSR.exe` с `PORTABLE_EXECUTABLE_FILE`; иначе установщик —
    // сам этот exe.
    let setup_file = services
        .env
        .get(PORTABLE_EXECUTABLE_FILE)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| exe.to_string_lossy().into_owned());
    if setup_args.silent {
        return setup::run_setup_silent(
            &services,
            &setup_args,
            setup::open_source(&exe),
            Some(&setup_file),
        );
    }
    run_setup_window(&setup_args, services, &exe, setup_file)
}

/// Окно установки (`runSetup` без `/S`): «Установить», «Открыть HLSR», «Удалить HLSR» через
/// установленную копию.
fn run_setup_window(
    setup_args: &SetupArgs,
    services: SetupServices,
    exe: &std::path::Path,
    setup_file: String,
) -> i32 {
    // `app.quit()` бэкенда («Открыть HLSR», «Удалить HLSR») — когда окно уже есть.
    let log = services.log.clone();
    let (backend, state) = match SetupBackend::open(
        services,
        setup_args,
        setup::open_source(exe),
        Some(setup_file),
        mode_window::quit_hook(),
    ) {
        Ok(opened) => opened,
        Err(error) => return failed("setup", false, &error),
    };
    let window = Arc::new(InstallerWindow::new(backend, state).with_log(log));
    let outcome = mode_window::run_installer("HLSR — установка", window.clone());
    window.closed();
    match outcome {
        Ok(()) => setup_exit::OK,
        Err(error) => failed("setup", false, &error),
    }
}

/// `HLSR.exe --hlsr-uninstall [/S] [/HLSRDELETE…] [--hlsr-return-setup <setup.exe>]`.
pub fn run_uninstall(args: &LaunchArgs, paths: &Paths) -> i32 {
    let setup_args = args.setup.clone().unwrap_or_default();
    let services = match services(paths) {
        Ok(services) => services,
        Err(error) => return failed("uninstall", setup_args.silent, &error),
    };
    if setup_args.silent {
        return setup::run_uninstall_silent(&services, &setup_args);
    }
    let log = services.log.clone();
    let (backend, state, title) =
        match UninstallBackend::open(services, &setup_args, mode_window::quit_hook()) {
            Ok(Some(opened)) => opened,
            Ok(None) => {
                // HLSR запущен: сообщение и код 3, окна нет.
                dialogs::setup_warning(UNINSTALL_RUNNING_MESSAGE);
                return setup_exit::IN_USE;
            }
            Err(error) => return failed("uninstall", false, &error),
        };
    let window = Arc::new(InstallerWindow::new(backend.clone(), state).with_log(log));
    let outcome = mode_window::run_installer(&title, window.clone());
    window.closed();
    match outcome {
        Ok(()) => {
            // `will-quit`: отложенное удаление каталога программы и «Назад» к установщику.
            backend.on_quit();
            setup_exit::OK
        }
        Err(error) => failed("uninstall", false, &error),
    }
}
