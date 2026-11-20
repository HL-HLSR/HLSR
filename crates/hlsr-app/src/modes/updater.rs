//! Внешний апдейтер Windows (`launcher-update/updater-mode.js` + `updater/main.js` 0.4.7): копия
//! `HLSRUpdater.exe` в `%TEMP%\HLSR-Updater-*` с `--hlsr-updater --plan=<файл>` проверяет план,
//! качает (или собирает из чанков) установщик, ждёт выхода лаунчера, ставит и запускает новую
//! версию; квитанция перехода — в `version-state.json` (писатель — `VersionEvents` ПП4, решение 15).
//!
//! Окно «HLSR Updater» 460×280 (`hlsr_gpui::updater`, `controllers::updater`): стадии, прогресс,
//! «Отменить» и «Закрыть» (прежний лаунчер запускается снова, если установка не состоялась); успех —
//! выход через 2,5 с. Код выхода: 0, 1 — если сессия кончилась ошибкой. Без графики окно не создаётся, и
//! апдейтер работает тихо: ошибка — как «Закрыть». Журнал — `%LOCALAPPDATA%\HLSR\logs\updater.log`
//! (`main.log` лаунчера не трогается: 0.4.7 переключала режим в `entry.js` раньше журнала).

use super::window as mode_window;
use hlsr_core::args::LaunchArgs;
use hlsr_core::notifications::NotificationStore;
use hlsr_gpui::updater::Edit;
use hlsr_online::notifications::version_events::VersionEvents;
use hlsr_ui::controllers::updater::UpdaterCommand;
use hlsr_update::chunks::HttpsFetch;
use hlsr_update::ports::{Clock, Logger, SystemClock, TracingLogger};
use hlsr_update::updater::download::SystemFreeSpace;
use hlsr_update::updater::mode::{UpdaterConfig, UpdaterEvent, UpdaterMain};
use hlsr_update::updater::process_wait::SystemProcessList;
use hlsr_update::updater::processes::pid_is_alive;
use hlsr_update::updater::receipt::ReceiptStore;
use hlsr_update::updater::session::SystemDeps;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

/// Код выхода, если обновление не состоялось.
const FAILED: i32 = 1;
/// Сколько ждать выхода после конца сессии (успех выходит через 2,5 с).
const QUIT_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Квитанция перехода (`writeReceipt(plan.receiptFile, from, to)`) — писателем ПП4.
struct VersionStateReceipts;

impl ReceiptStore for VersionStateReceipts {
    fn write_pending_receipt(&self, file: &str, from: Option<&str>, to: &str) -> bool {
        let file = Path::new(file);
        // Центр уведомлений рядом: квитанция уведомлений не пишет.
        let store = Arc::new(NotificationStore::new(
            file.with_file_name("notifications.json"),
        ));
        let clock: Arc<dyn Fn() -> i64 + Send + Sync> = Arc::new(|| SystemClock.now_ms());
        VersionEvents::new(file, store, Arc::new(TracingLogger), clock)
            .write_pending_receipt(from, Some(to))
            .is_some()
    }
}

/// Журнал апдейтера для зависимостей сессии: тот же `updater.log`, что у [`UpdaterMain`].
#[derive(Default)]
struct LateLog(OnceLock<Arc<dyn Logger>>);

impl Logger for LateLog {
    fn info(&self, message: &str) {
        if let Some(log) = self.0.get() {
            log.info(message);
        }
    }
    fn warn(&self, message: &str) {
        if let Some(log) = self.0.get() {
            log.warn(message);
        }
    }
    fn error(&self, message: &str) {
        if let Some(log) = self.0.get() {
            log.error(message);
        }
    }
}

pub fn run(_args: &LaunchArgs) -> i32 {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("hlsr-updater")
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("апдейтер: рантайм не запущен: {error}");
            return FAILED;
        }
    };
    let code = run_window(&runtime);
    runtime.shutdown_timeout(QUIT_GRACE);
    code
}

/// Окно «HLSR Updater» (`updater/main.js`): сессия начинается, когда окно открыто; закрыть окно
/// системой можно, если сессии нет, она кончилась или ещё отменяема (`allowClose`), — тогда без
/// перезапуска прежнего лаунчера, как `window-all-closed`.
fn run_window(runtime: &tokio::runtime::Runtime) -> i32 {
    let _guard = runtime.enter();
    let (config, deps, clock, late_log) = config_and_deps();
    let failed = Arc::new(AtomicBool::new(false));
    let errors = failed.clone();
    // События ядра → правки страницы окна (`updater:init`/`stage`/`progress`).
    let (edits, received) = tokio::sync::mpsc::unbounded_channel::<Edit>();
    let main = UpdaterMain::new(config, deps, clock, Arc::new(pid_is_alive), move |event| {
        let edit: Edit = match event {
            UpdaterEvent::Quit => {
                hlsr_gpui::bridge::quit();
                return;
            }
            UpdaterEvent::Init {
                language,
                target_version,
            } => Box::new(move |page| page.init(&language, &target_version)),
            UpdaterEvent::Stage(payload) => {
                errors.store(payload["stage"] == "error", Ordering::SeqCst);
                Box::new(move |page| page.stage(&payload))
            }
            UpdaterEvent::Progress(payload) => Box::new(move |page| page.progress(&payload)),
        };
        let _ = edits.send(edit);
    });
    let _ = late_log.0.set(main.log());
    let worker: Rc<RefCell<Option<tokio::task::JoinHandle<()>>>> = Rc::default();
    let outcome = {
        let main = main.clone();
        let started = worker.clone();
        let handle = runtime.handle().clone();
        let closing = main.clone();
        let commands = main.clone();
        mode_window::run_updater(
            received,
            move |command| match command {
                UpdaterCommand::Cancel => commands.cancel(),
                UpdaterCommand::Close => commands.close(),
            },
            move || {
                *started.borrow_mut() = Some(handle.spawn(async move { main.run().await }));
            },
            move || closing.allow_close(),
        )
    };
    let started = worker.borrow_mut().take();
    match (outcome, started) {
        (Ok(()), worker) => {
            if let Some(worker) = worker {
                worker.abort();
            }
            main.will_quit();
            if failed.load(Ordering::SeqCst) {
                FAILED
            } else {
                0
            }
        }
        (Err(error), None) => {
            // Окна нет (нет графики): апдейтер работает тихо, как до окна.
            eprintln!("апдейтер: {error}, работа без окна");
            runtime.block_on(run_async())
        }
        (Err(error), Some(worker)) => {
            worker.abort();
            main.will_quit();
            tracing::error!("[updater] окно: {error}");
            FAILED
        }
    }
}

/// Настройки и зависимости апдейтера (`process.argv`, пути, загрузка, процессы, квитанция).
fn config_and_deps() -> (UpdaterConfig, Arc<SystemDeps>, Arc<dyn Clock>, Arc<LateLog>) {
    let env = super::mode_env();
    let lookup = env.lookup();
    let exe = std::env::current_exe().unwrap_or_default();
    let config = UpdaterConfig {
        argv: std::env::args().collect(),
        resources: exe
            .parent()
            .map_or_else(|| PathBuf::from("resources"), |dir| dir.join("resources")),
        exe,
        local_app_data: env
            .get("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        temp_dir: super::temp_dir(&lookup),
        pid: std::process::id(),
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        qa_trust: true,
    };
    let late_log = Arc::new(LateLog::default());
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let deps = Arc::new(SystemDeps {
        fetch: Arc::new(HttpsFetch::new()),
        launcher: super::launcher(),
        processes: Arc::new(SystemProcessList),
        receipts: Arc::new(VersionStateReceipts),
        free_space: Arc::new(SystemFreeSpace),
        clock: clock.clone(),
        logger: late_log.clone(),
        env: hlsr_update::windows::processes::launch_environment(&env)
            .vars()
            .to_vec(),
    });
    (config, deps, clock, late_log)
}

/// Без окна (графика недоступна): итог — код выхода и `updater.log`.
async fn run_async() -> i32 {
    let (config, deps, clock, late_log) = config_and_deps();
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    let main = UpdaterMain::new(config, deps, clock, Arc::new(pid_is_alive), move |event| {
        let _ = events.send(event);
    });
    let _ = late_log.0.set(main.log());
    let mut worker = {
        let main = main.clone();
        tokio::spawn(async move { main.run().await })
    };
    let mut code = 0;
    // Сессия кончилась, а выхода нет (непредвиденный путь): не висеть без окна.
    let mut give_up: Option<tokio::time::Instant> = None;
    loop {
        tokio::select! {
            event = received.recv() => match event {
                Some(UpdaterEvent::Stage(payload)) if payload["stage"] == "error" => {
                    code = FAILED;
                    // Окна нет — как «Закрыть»: прежний лаунчер, если установка не состоялась.
                    main.close();
                }
                Some(UpdaterEvent::Quit) | None => break,
                Some(_) => {}
            },
            _ = &mut worker, if give_up.is_none() => {
                give_up = Some(tokio::time::Instant::now() + QUIT_GRACE);
            }
            _ = tokio::time::sleep_until(give_up.unwrap_or_else(tokio::time::Instant::now)),
                if give_up.is_some() => break,
        }
    }
    worker.abort();
    main.will_quit();
    code
}
