//! Окно установщика без интерфейса против самотестов 0.4.7: `cancellation-selftest.js` («installer:
//! cancellation leaves the window reusable…», «installer: close during preparation waits for abort
//! cleanup», «installer: commit cannot be cancelled halfway…») и `launcher-update-selftest.js`
//! («окно удаления Windows: «Назад» идёт в backend.back или спрятано»).

use hlsr_update::installer_window::{
    BackendError, Ctx, InstallerBackend, InstallerState, InstallerWindow, Phase, StatePatch,
    UninstallPlan,
};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

type InstallFn = dyn Fn(bool, &Ctx<'_>) -> Result<bool, BackendError> + Send + Sync;

/// Backend из замыканий, как объект-backend самотестов.
#[derive(Default)]
struct TestBackend {
    install: Option<Box<InstallFn>>,
    select: bool,
    back: Option<Arc<AtomicU32>>,
    uninstalls: AtomicU32,
}

impl InstallerBackend for TestBackend {
    fn can_select_directory(&self) -> bool {
        self.select
    }

    fn select_directory(&self, directory: &str, ctx: &Ctx<'_>) -> Result<(), String> {
        ctx.send(StatePatch {
            directory: Some(directory.to_owned()),
            phase: Some(Phase::Idle),
            ..StatePatch::default()
        });
        Ok(())
    }

    fn install(&self, desktop_shortcut: bool, ctx: &Ctx<'_>) -> Result<bool, BackendError> {
        match &self.install {
            Some(install) => install(desktop_shortcut, ctx),
            None => Ok(false),
        }
    }

    fn launch(&self, _ctx: &Ctx<'_>) -> Result<bool, String> {
        Ok(false)
    }

    fn uninstall_plan(&self, _ctx: &Ctx<'_>) -> Option<UninstallPlan> {
        Some(UninstallPlan::default())
    }

    fn uninstall(&self, _remove_games: bool, ctx: &Ctx<'_>) -> Result<bool, String> {
        self.uninstalls.fetch_add(1, Ordering::SeqCst);
        ctx.send(StatePatch {
            phase: Some(Phase::Uninstalled),
            can_uninstall: Some(false),
            ..StatePatch::default()
        });
        Ok(true)
    }

    fn has_back(&self) -> bool {
        self.back.is_some()
    }

    fn back(&self, _ctx: &Ctx<'_>) -> bool {
        match &self.back {
            Some(count) => {
                count.fetch_add(1, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }
}

fn idle_state() -> InstallerState {
    InstallerState {
        phase: Phase::Idle,
        directory: "C:\\default".to_owned(),
        ..InstallerState::default()
    }
}

/// Ждать отмены по токену (`untilAbort(ctx.signal)`).
fn until_abort(ctx: &Ctx<'_>) -> Result<bool, BackendError> {
    let token = ctx
        .cancel_token()
        .expect("сигнал есть во время операции")
        .clone();
    while !token.is_cancelled() {
        std::thread::sleep(Duration::from_millis(2));
    }
    Err("The operation was aborted".into())
}

#[test]
fn cancellation_leaves_the_window_reusable() {
    let runs = Arc::new(AtomicU32::new(0));
    let counter = runs.clone();
    let backend = Arc::new(TestBackend {
        select: true,
        install: Some(Box::new(move |_desktop, ctx| {
            let run = counter.fetch_add(1, Ordering::SeqCst) + 1;
            ctx.send(StatePatch::phase(Phase::Installing));
            if run == 1 {
                until_abort(ctx)
            } else {
                ctx.send(StatePatch::phase(Phase::Done));
                Ok(true)
            }
        })),
        ..TestBackend::default()
    });
    let window = Arc::new(InstallerWindow::new(backend, idle_state()));
    let destroyed = Arc::new(AtomicBool::new(false));
    let flag = destroyed.clone();
    window.set_on_destroy(move || flag.store(true, Ordering::SeqCst));
    assert!(window.state().can_choose_directory);
    let chosen = window
        .select_directory(|start| {
            assert_eq!(start, "C:\\default");
            Some("C:\\chosen-launcher".to_owned())
        })
        .unwrap();
    assert_eq!(chosen.unwrap().directory, "C:\\chosen-launcher");

    let worker = window.clone();
    let pending = std::thread::spawn(move || worker.install(true));
    while !window.state().cancellable {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        window
            .select_directory(|_| Some("C:\\x".to_owned()))
            .unwrap(),
        None
    );
    assert!(window.cancel());
    assert!(!pending.join().unwrap());
    assert_eq!(window.state().phase, Phase::Cancelled);
    assert_eq!(window.state().error, None);
    assert!(!destroyed.load(Ordering::SeqCst));
    assert!(window.install(false));
    assert_eq!(window.state().phase, Phase::Done);
    assert!(
        !window.install(false),
        "из фазы done установка не запускается"
    );
    assert!(window.request_close());
    assert!(destroyed.load(Ordering::SeqCst));
    assert!(window.is_closed());
}

#[test]
fn close_during_preparation_waits_for_abort_cleanup() {
    let cleaned = Arc::new(AtomicBool::new(false));
    let flag = cleaned.clone();
    let backend = Arc::new(TestBackend {
        install: Some(Box::new(move |_desktop, ctx| {
            let result = until_abort(ctx);
            flag.store(true, Ordering::SeqCst);
            result
        })),
        ..TestBackend::default()
    });
    let window = Arc::new(InstallerWindow::new(backend, idle_state()));
    let destroyed = Arc::new(AtomicBool::new(false));
    let flag = destroyed.clone();
    window.set_on_destroy(move || flag.store(true, Ordering::SeqCst));
    let worker = window.clone();
    let pending = std::thread::spawn(move || worker.install(false));
    while !window.state().cancellable {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        !window.request_close(),
        "во время работы окно не закрывается сразу"
    );
    pending.join().unwrap();
    assert!(cleaned.load(Ordering::SeqCst));
    assert!(destroyed.load(Ordering::SeqCst));
}

#[test]
fn commit_cannot_be_cancelled_halfway() {
    let (finish_tx, finish_rx) = mpsc::channel::<()>();
    let finish_rx = Mutex::new(finish_rx);
    let signal = Arc::new(Mutex::new(None));
    let seen = signal.clone();
    let backend = Arc::new(TestBackend {
        install: Some(Box::new(move |_desktop, ctx| {
            *seen.lock().unwrap() = ctx.cancel_token().cloned();
            ctx.commit();
            finish_rx.lock().unwrap().recv().unwrap();
            ctx.send(StatePatch::phase(Phase::Done));
            Ok(true)
        })),
        ..TestBackend::default()
    });
    let window = Arc::new(InstallerWindow::new(backend, idle_state()));
    let destroyed = Arc::new(AtomicBool::new(false));
    let flag = destroyed.clone();
    window.set_on_destroy(move || flag.store(true, Ordering::SeqCst));
    let worker = window.clone();
    let pending = std::thread::spawn(move || worker.install(false));
    while signal.lock().unwrap().is_none() || window.state().cancellable {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!window.cancel());
    assert!(!window.request_close());
    let token = signal.lock().unwrap().clone().unwrap();
    assert!(!token.is_cancelled());
    finish_tx.send(()).unwrap();
    assert!(pending.join().unwrap());
    assert!(destroyed.load(Ordering::SeqCst));
    assert!(!token.is_cancelled());
}

/// Ошибка операции (не отмена): фаза `error`, текст до 400 символов UTF-16.
#[test]
fn failure_shows_error_trimmed_to_400_units() {
    let backend = Arc::new(TestBackend {
        install: Some(Box::new(|_desktop, _ctx| Err("ж".repeat(500).into()))),
        ..TestBackend::default()
    });
    let window = InstallerWindow::new(backend, idle_state());
    let states = Arc::new(Mutex::new(Vec::new()));
    let log = states.clone();
    window.set_listener(move |state| log.lock().unwrap().push(state.clone()));
    assert!(!window.install(false));
    let state = window.state();
    assert_eq!(state.phase, Phase::Error);
    assert_eq!(state.error.unwrap().chars().count(), 400);
    assert!(!state.cancellable);
    let sent = states.lock().unwrap();
    assert!(sent[0].cancellable, "сначала отменяемость включается");
    assert!(!sent.last().unwrap().cancellable);
}

#[test]
fn back_goes_to_backend_back_or_is_hidden() {
    let backs = Arc::new(AtomicU32::new(0));
    let state = InstallerState {
        phase: Phase::UninstallConfirm,
        can_uninstall: true,
        can_go_back: Some(true),
        ..InstallerState::default()
    };
    let window = InstallerWindow::new(
        Arc::new(TestBackend {
            back: Some(backs.clone()),
            ..TestBackend::default()
        }),
        state.clone(),
    );
    assert!(!window.back(), "до удаления «Назад» не работает");
    assert!(window.uninstall(false));
    assert!(window.back());
    assert_eq!(backs.load(Ordering::SeqCst), 1);

    // Удаление из «Программ и компонентов»: установщика нет, возвращаться некуда.
    let window = InstallerWindow::new(
        Arc::new(TestBackend::default()),
        InstallerState {
            can_go_back: Some(false),
            ..state
        },
    );
    assert!(window.uninstall(false));
    assert!(!window.back());
    assert_eq!(backs.load(Ordering::SeqCst), 1);
}

/// «Назад» без своего backend.back: тот же каталог проверяется заново (`selectDirectory`), без
/// выбора каталога — окно возвращается к установке.
#[test]
fn back_without_backend_back_returns_to_installation() {
    let state = InstallerState {
        phase: Phase::Uninstalled,
        directory: "C:\\HLSR\\Launcher".to_owned(),
        can_uninstall: false,
        installed_version: Some("0.4.7".to_owned()),
        update: true,
        warning: Some("w".to_owned()),
        ..InstallerState::default()
    };
    let window = InstallerWindow::new(Arc::new(TestBackend::default()), state.clone());
    assert!(window.back());
    let after = window.state();
    assert_eq!(after.phase, Phase::Idle);
    assert_eq!(after.installed_version, None);
    assert!(!after.update);
    assert_eq!(after.warning, None);

    let window = InstallerWindow::new(
        Arc::new(TestBackend {
            select: true,
            ..TestBackend::default()
        }),
        state,
    );
    assert!(window.back());
    assert_eq!(window.state().directory, "C:\\HLSR\\Launcher");
    assert_eq!(window.state().phase, Phase::Idle);
}

#[test]
fn uninstall_needs_can_uninstall_and_plan_is_refused_while_busy() {
    let window = InstallerWindow::new(Arc::new(TestBackend::default()), idle_state());
    assert!(!window.uninstall(true));
    assert!(window.uninstall_plan().is_some());
    assert!(!window.launch(), "запуск только из done");
    assert!(!window.cancel(), "отменять нечего");
    assert_eq!(window.state().to_value()["phase"], "idle");
}

/// 0.4.9: `platform` — ОС процесса, если начальное состояние её не задало.
#[test]
fn platform_defaults_to_the_build_os() {
    let window = InstallerWindow::new(Arc::new(TestBackend::default()), idle_state());
    let expected = if cfg!(windows) { "win32" } else { "linux" };
    assert_eq!(window.state().to_value()["platform"], expected);
    let given = InstallerState {
        platform: "linux".to_owned(),
        ..idle_state()
    };
    let window = InstallerWindow::new(Arc::new(TestBackend::default()), given);
    assert_eq!(window.state().platform, "linux");
}

/// 0.4.9: код ошибки — `ANTIVIRUS`, код ОС причины или свой код; файл — из ошибки; новая ошибка без
/// кода сбрасывает прежние; «Открыть Discord» — только в фазе `error`.
#[test]
fn error_code_and_file_reach_the_window() {
    let failure = |error: BackendError| {
        let backend = Arc::new(TestBackend {
            install: Some(Box::new(move |_desktop, _ctx| Err(error.clone()))),
            ..TestBackend::default()
        });
        let window = InstallerWindow::new(backend, idle_state());
        assert!(!window.install(false));
        window
    };
    let antivirus = failure(BackendError {
        message: "Антивирус заблокировал или удалил файл HLSR.exe (EPERM)".to_owned(),
        code: Some("ANTIVIRUS".to_owned()),
        file: Some("HLSR.exe".to_owned()),
        cause_code: Some("EPERM".to_owned()),
        cause_message: Some("EPERM: operation not permitted".to_owned()),
    });
    let state = antivirus.state().to_value();
    assert_eq!(state["errorCode"], "ANTIVIRUS");
    assert_eq!(state["errorFile"], "HLSR.exe");

    let no_space = failure(BackendError {
        message: "Не удалось скопировать a: ENOSPC".to_owned(),
        code: Some("COPY_FAILED".to_owned()),
        cause_code: Some("ENOSPC".to_owned()),
        ..BackendError::default()
    });
    assert_eq!(no_space.state().error_code.as_deref(), Some("ENOSPC"));
    let in_use = failure(BackendError {
        code: Some("IN_USE".to_owned()),
        ..BackendError::from("Файлы HLSR заняты")
    });
    assert_eq!(in_use.state().error_code.as_deref(), Some("IN_USE"));
    let plain = failure("сломалось".into());
    assert_eq!(plain.state().error_code, None);

    // Ошибка без кода (замок установщика) не наследует прежний код и файл.
    let mut state = antivirus.state();
    StatePatch::phase(Phase::Error)
        .with_error(Some("x".to_owned()))
        .apply(&mut state);
    assert_eq!((state.error_code, state.error_file), (None, None));
    let mut state = antivirus.state();
    StatePatch::default().with_warning(None).apply(&mut state);
    assert_eq!(
        state.error_code.as_deref(),
        Some("ANTIVIRUS"),
        "без ошибки код остаётся"
    );

    let mut opened = 0;
    assert!(antivirus.open_discord(|| opened += 1));
    assert_eq!(opened, 1);
    let idle = InstallerWindow::new(Arc::new(TestBackend::default()), idle_state());
    assert!(!idle.open_discord(|| panic!("не в фазе error")));
}
