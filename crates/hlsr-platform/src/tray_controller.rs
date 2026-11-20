//! Окно в трее во время игры (`tray/TrayController.js`, связь с играми): игра запустилась —
//! окно прячется в трей; закрылась — через 3 с возвращается, если ни одна игра не идёт
//! (игра могла запуститься снова или идёт другая — тогда окно остаётся в трее и не забирает
//! фокус). Страховка: событие о закрытии игры потерялось — через час окно возвращается, но не
//! поверх идущей игры. Сам значок и окно — порт [`TrayWindow`] (в приложении — трей Slint).
//! Таймеры — задачи tokio: выход они не держат.

use hlsr_delivery::ports::{Logger, TracingLogger};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::task::AbortHandle;

/// `SAFETY_RETURN_MS`.
pub const SAFETY_RETURN: Duration = Duration::from_secs(60 * 60);
/// `FINISH_SETTLE_MS`.
pub const FINISH_SETTLE: Duration = Duration::from_millis(3000);

/// Окно лаунчера и значок трея.
pub trait TrayWindow: Send + Sync {
    /// Окно есть и не закрыто (`window && !window.isDestroyed()`).
    fn available(&self) -> bool;
    /// `ensureTray()` + `window.hide()`: `false` — значок не создан, окно остаётся на экране
    /// (строку журнала пишет порт).
    fn hide(&self) -> bool;
    /// Развернуть, показать, отдать фокус и снять значок.
    fn show(&self);
    /// Снять значок (`destroyTray`) без показа окна.
    fn destroy_tray(&self);
}

/// Идёт ли сейчас хоть одна игра (`isGameRunning`).
pub type GameRunning = Arc<dyn Fn() -> bool + Send + Sync>;

/// Параметры конструктора (`options`).
#[derive(Default)]
pub struct TrayOptions {
    /// `window`: нет окна — трей ничего не делает.
    pub window: Option<Arc<dyn TrayWindow>>,
    pub logger: Option<Arc<dyn Logger>>,
    pub is_game_running: Option<GameRunning>,
    /// `settleMs`; по умолчанию [`FINISH_SETTLE`].
    pub settle: Option<Duration>,
    /// Страховка; по умолчанию [`SAFETY_RETURN`].
    pub safety: Option<Duration>,
    /// Среда tokio для таймеров; по умолчанию — текущая (если она есть).
    pub runtime: Option<Handle>,
}

#[derive(Default)]
struct State {
    enabled: bool,
    hidden: bool,
    settle_timer: Option<AbortHandle>,
    safety_timer: Option<AbortHandle>,
}

/// `TrayController` (связь с играми).
pub struct TrayController {
    me: Weak<TrayController>,
    window: Option<Arc<dyn TrayWindow>>,
    logger: Arc<dyn Logger>,
    is_game_running: GameRunning,
    settle: Duration,
    safety: Duration,
    runtime: Option<Handle>,
    state: Mutex<State>,
}

impl TrayController {
    pub fn new(options: TrayOptions) -> Arc<TrayController> {
        Arc::new_cyclic(|me| TrayController {
            me: me.clone(),
            window: options.window,
            logger: options.logger.unwrap_or_else(|| Arc::new(TracingLogger)),
            is_game_running: options
                .is_game_running
                .unwrap_or_else(|| Arc::new(|| false)),
            settle: options.settle.unwrap_or(FINISH_SETTLE),
            safety: options.safety.unwrap_or(SAFETY_RETURN),
            runtime: options.runtime.or_else(|| Handle::try_current().ok()),
            state: Mutex::new(State {
                enabled: true,
                ..State::default()
            }),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Спрятано ли окно в трей (`hidden`).
    pub fn is_hidden(&self) -> bool {
        self.state().hidden
    }

    /// Включено ли «Прятать во время игры» (`enabled`).
    pub fn is_enabled(&self) -> bool {
        self.state().enabled
    }

    /// `setEnabled(value)`: выключили, пока окно в трее, — окно возвращается.
    pub fn set_enabled(&self, enabled: bool) {
        let hidden = {
            let mut state = self.state();
            state.enabled = enabled;
            state.hidden
        };
        if !enabled && hidden {
            self.show();
        }
    }

    /// `onGameStarted()`.
    pub fn on_game_started(&self) {
        self.cancel_settle();
        if !self.state().enabled {
            return;
        }
        self.hide();
    }

    /// `onGameFinished()`: окно возвращается через `settle`, если игры нет.
    pub fn on_game_finished(&self) {
        if !self.state().hidden {
            return;
        }
        self.cancel_settle();
        let me = self.me.clone();
        let delay = self.settle;
        let timer = self.spawn(async move {
            tokio::time::sleep(delay).await;
            let Some(tray) = me.upgrade() else {
                return;
            };
            tray.state().settle_timer = None;
            if tray.still_playing() {
                return;
            }
            tray.show();
        });
        self.state().settle_timer = timer;
    }

    fn cancel_settle(&self) {
        if let Some(timer) = self.state().settle_timer.take() {
            timer.abort();
        }
    }

    /// `stillPlaying()`.
    fn still_playing(&self) -> bool {
        hlsr_core::tasks::catch_panic(|| (self.is_game_running)()).unwrap_or(false)
    }

    /// `hide()`.
    pub fn hide(&self) {
        if self.state().hidden {
            return;
        }
        let Some(window) = self.window.as_ref().filter(|window| window.available()) else {
            return;
        };
        if !window.hide() {
            return;
        }
        self.state().hidden = true;
        self.arm_safety();
    }

    /// `armSafety()`: раньше страховка через час возвращала окно с фокусом прямо поверх идущей
    /// игры — теперь, пока игра идёт, она взводится заново.
    fn arm_safety(&self) {
        if let Some(timer) = self.state().safety_timer.take() {
            timer.abort();
        }
        let me = self.me.clone();
        let delay = self.safety;
        let timer = self.spawn(async move {
            tokio::time::sleep(delay).await;
            let Some(tray) = me.upgrade() else {
                return;
            };
            tray.state().safety_timer = None;
            if tray.still_playing() {
                tray.arm_safety();
                return;
            }
            tray.logger
                .warn("[tray] событие о завершении игры не пришло, возвращаю окно");
            tray.show();
        });
        self.state().safety_timer = timer;
    }

    /// `show()`.
    pub fn show(&self) {
        {
            let mut state = self.state();
            if let Some(timer) = state.settle_timer.take() {
                timer.abort();
            }
            if let Some(timer) = state.safety_timer.take() {
                timer.abort();
            }
            state.hidden = false;
        }
        let Some(window) = self.window.as_ref().filter(|window| window.available()) else {
            return;
        };
        window.show();
    }

    /// `dispose()`: таймеры и значок.
    pub fn dispose(&self) {
        {
            let mut state = self.state();
            if let Some(timer) = state.settle_timer.take() {
                timer.abort();
            }
            if let Some(timer) = state.safety_timer.take() {
                timer.abort();
            }
        }
        if let Some(window) = &self.window {
            window.destroy_tray();
        }
    }

    fn spawn(
        &self,
        future: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Option<AbortHandle> {
        let runtime = self.runtime.clone().or_else(|| Handle::try_current().ok());
        match runtime {
            Some(runtime) => Some(runtime.spawn(future).abort_handle()),
            None => {
                self.logger
                    .warn("[tray] таймер не запущен: нет среды tokio");
                None
            }
        }
    }
}

impl Drop for TrayController {
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for timer in [state.settle_timer.take(), state.safety_timer.take()]
            .into_iter()
            .flatten()
        {
            timer.abort();
        }
    }
}
