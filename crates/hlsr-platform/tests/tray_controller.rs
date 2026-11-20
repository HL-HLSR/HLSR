//! Трей во время игры (`tray/TrayController.js` 0.4.7) с поддельным окном: случаи
//! `scripts/tray-selftest.js` (окно не возвращается, пока игра снова запущена; окно, которое не
//! пряталось, фокус не забирает) и остальное поведение контроллера — выключение во время игры,
//! страховка через час, значок не создан, `dispose`. Время — часы tokio на паузе.

use hlsr_core::logger::MemoryLogger;
use hlsr_platform::tray_controller::{TrayController, TrayOptions, TrayWindow};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct FakeWindow {
    calls: Mutex<Vec<&'static str>>,
    no_icon: AtomicBool,
    closed: AtomicBool,
}

impl FakeWindow {
    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }
}

impl TrayWindow for FakeWindow {
    fn available(&self) -> bool {
        !self.closed.load(Ordering::SeqCst)
    }

    fn hide(&self) -> bool {
        if self.no_icon.load(Ordering::SeqCst) {
            return false;
        }
        self.calls.lock().unwrap().push("hide");
        true
    }

    fn show(&self) {
        // restore (если свёрнуто), show, focus и снятие значка — одним вызовом порта.
        self.calls.lock().unwrap().push("show");
    }

    fn destroy_tray(&self) {
        self.calls.lock().unwrap().push("destroy");
    }
}

struct Setup {
    tray: Arc<TrayController>,
    window: Arc<FakeWindow>,
    running: Arc<AtomicBool>,
    log: Arc<MemoryLogger>,
}

fn setup(settle: Duration, safety: Option<Duration>) -> Setup {
    let window = Arc::new(FakeWindow::default());
    let running = Arc::new(AtomicBool::new(false));
    let log = Arc::new(MemoryLogger::default());
    let flag = running.clone();
    let tray = TrayController::new(TrayOptions {
        window: Some(window.clone()),
        logger: Some(log.clone()),
        is_game_running: Some(Arc::new(move || flag.load(Ordering::SeqCst))),
        settle: Some(settle),
        safety,
        runtime: None,
    });
    Setup {
        tray,
        window,
        running,
        log,
    }
}

async fn wait(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

/// tray-selftest: «Трей: окно не возвращается, пока игра снова запущена».
#[tokio::test(start_paused = true)]
async fn window_stays_in_tray_while_a_game_is_running_again() {
    let s = setup(Duration::from_millis(20), None);
    s.running.store(true, Ordering::SeqCst);
    s.tray.on_game_started();
    assert_eq!(s.window.calls(), ["hide"]);
    s.tray.on_game_finished();
    wait(40).await;
    assert_eq!(
        s.window.calls(),
        ["hide"],
        "игра перезапустилась — окно в трее"
    );
    s.running.store(false, Ordering::SeqCst);
    s.tray.on_game_finished();
    wait(40).await;
    assert_eq!(s.window.calls(), ["hide", "show"]);
    assert!(!s.tray.is_hidden());
    s.tray.dispose();
}

/// tray-selftest: «Трей: окно, которое не пряталось, после игры фокус не забирает».
#[tokio::test(start_paused = true)]
async fn window_that_was_not_hidden_does_not_take_focus() {
    let s = setup(Duration::from_millis(10), None);
    s.tray.set_enabled(false);
    s.tray.on_game_started();
    s.tray.on_game_finished();
    wait(30).await;
    assert!(s.window.calls().is_empty());
    s.tray.dispose();
    assert_eq!(s.window.calls(), ["destroy"]);
}

/// Возврат окна — через 3 с после выхода, не раньше; новая игра в эти 3 с его отменяет.
#[tokio::test(start_paused = true)]
async fn window_returns_three_seconds_after_the_game() {
    let s = setup(hlsr_platform::tray_controller::FINISH_SETTLE, None);
    s.tray.on_game_started();
    s.tray.on_game_finished();
    wait(2900).await;
    assert_eq!(s.window.calls(), ["hide"]);
    // Игра запустилась снова до возврата окна: таймер снят, окно остаётся в трее.
    s.tray.on_game_started();
    wait(5000).await;
    assert_eq!(s.window.calls(), ["hide"]);
    s.tray.on_game_finished();
    wait(3010).await;
    assert_eq!(s.window.calls(), ["hide", "show"]);
}

/// «Прятать во время игры» выключили, пока окно в трее, — окно возвращается сразу.
#[tokio::test(start_paused = true)]
async fn disabling_while_hidden_shows_the_window() {
    let s = setup(Duration::from_millis(10), None);
    s.tray.on_game_started();
    assert!(s.tray.is_hidden());
    s.tray.set_enabled(false);
    assert_eq!(s.window.calls(), ["hide", "show"]);
    assert!(!s.tray.is_hidden());
    // Выключено: следующая игра окно не прячет.
    s.tray.on_game_started();
    assert_eq!(s.window.calls(), ["hide", "show"]);
    s.tray.set_enabled(true);
    s.tray.on_game_started();
    assert_eq!(s.window.calls(), ["hide", "show", "hide"]);
}

/// Страховка: событие о выходе потерялось — через час окно возвращается, но не поверх идущей
/// игры (страховка взводится заново).
#[tokio::test(start_paused = true)]
async fn safety_returns_the_window_only_when_no_game_runs() {
    let s = setup(Duration::from_millis(10), None);
    s.running.store(true, Ordering::SeqCst);
    s.tray.on_game_started();
    wait(60 * 60 * 1000 + 10).await;
    assert_eq!(s.window.calls(), ["hide"], "игра идёт — окно в трее");
    assert!(s.log.take().is_empty());
    s.running.store(false, Ordering::SeqCst);
    wait(60 * 60 * 1000).await;
    assert_eq!(s.window.calls(), ["hide", "show"]);
    let entries = s.log.take();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].0, "warn");
    assert_eq!(
        entries[0].1,
        "[tray] событие о завершении игры не пришло, возвращаю окно"
    );
}

/// Значок не создан: окно остаётся на экране и после игры не «возвращается» поверх неё.
#[tokio::test(start_paused = true)]
async fn without_tray_icon_the_window_stays_and_is_not_shown_again() {
    let s = setup(Duration::from_millis(10), None);
    s.window.no_icon.store(true, Ordering::SeqCst);
    s.tray.on_game_started();
    assert!(!s.tray.is_hidden());
    s.tray.on_game_finished();
    wait(50).await;
    assert!(s.window.calls().is_empty());
}

/// Окно закрыто: трей ничего не делает.
#[tokio::test(start_paused = true)]
async fn closed_window_is_left_alone() {
    let s = setup(Duration::from_millis(10), None);
    s.window.closed.store(true, Ordering::SeqCst);
    s.tray.on_game_started();
    assert!(!s.tray.is_hidden());
    assert!(s.window.calls().is_empty());
}

/// `dispose()` снимает таймеры: окно потом не появляется.
#[tokio::test(start_paused = true)]
async fn dispose_cancels_pending_return() {
    let s = setup(Duration::from_millis(100), Some(Duration::from_millis(500)));
    s.tray.on_game_started();
    s.tray.on_game_finished();
    s.tray.dispose();
    wait(2000).await;
    assert_eq!(s.window.calls(), ["hide", "destroy"]);
}
