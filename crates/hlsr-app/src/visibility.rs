//! Видимость окна лаунчера — одно вычисление на всех. 0.4.7 спрашивал `BrowserWindow`:
//! `isVisible() && !isMinimized()` — онлайн (`windowVisible()`: телеметрия, счётчики онлайна) и
//! платформа (`isWindowVisible()` доставки: опрос Steam); его события — службам: `ready` окна
//! (первое появление), `show`/`restore` (`onlineClient.fetchOnce`), `focus` (проверки при фокусе);
//! интерфейсу — `window-visibility` (`minimize`/`hide` → нет, `restore`/`show` → да).
//!
//! Сверка окна — по событиям GPUI в потоке окна: видимость (`WM_SHOWWINDOW`, свернуть и
//! развернуть), фокус, первый кадр. Видимость ставит на паузу бесконечные анимации
//! (`AnimGate`). Значение для потоков tokio — [`WindowVisibility`].

use gpui::{App, Global, Window, WindowHandle};
use hlsr_gpui::app::LauncherRoot;
use hlsr_gpui::motion::AnimGate;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Видно ли окно (`isVisible() && !isMinimized()`) — из любого потока.
#[derive(Clone, Default)]
pub struct WindowVisibility(Arc<AtomicBool>);

impl WindowVisibility {
    pub fn visible(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn set(&self, visible: bool) {
        self.0.store(visible, Ordering::SeqCst);
    }
}

/// События окна для служб (`BrowserWindow` и `ipcMain.on("ready")` 0.4.7).
pub trait WindowEvents {
    /// Первое появление окна — `ready`.
    fn ready(&self);
    /// Окно появилось или развёрнуто — `show`/`restore`.
    fn shown(&self);
    /// Окно получило фокус — `focus`.
    fn focused(&self);
}

/// Что изменилось за одну сверку.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Changes {
    pub ready: bool,
    pub shown: bool,
    pub focused: bool,
    /// Новое значение `window-visibility` интерфейса, если оно разошлось с окном.
    pub ui_visible: Option<bool>,
}

/// Состояние сверки: что было видно в прошлый раз.
#[derive(Debug, Default)]
pub struct Tracker {
    ready: bool,
    visible: bool,
    focused: bool,
}

impl Tracker {
    /// `visible` и `focused` — окно сейчас, `ui_visible` — что знает интерфейс сейчас (его меняют и
    /// сами команды окна: свернуть — раньше сверки).
    pub fn observe(&mut self, visible: bool, focused: bool, ui_visible: bool) -> Changes {
        let shown = visible && !self.visible;
        let ready = shown && !self.ready;
        self.ready |= ready;
        let gained_focus = focused && !self.focused;
        self.visible = visible;
        self.focused = focused;
        Changes {
            ready,
            shown,
            focused: gained_focus,
            ui_visible: (self.ready && ui_visible != visible).then_some(visible),
        }
    }
}

/// События одной сверки — по порядку: `ready`, `show`, `focus`.
pub fn dispatch(changes: Changes, events: &dyn WindowEvents) {
    if changes.ready {
        events.ready();
    }
    if changes.shown {
        events.shown();
    }
    if changes.focused {
        events.focused();
    }
}

/// Видимость и фокус окна лаунчера (глобал потока окна).
#[derive(Clone)]
pub struct Visibility {
    state: WindowVisibility,
    events: Rc<RefCell<Option<Box<dyn WindowEvents>>>>,
    tracker: Rc<RefCell<Tracker>>,
    /// Что знает интерфейс (`window-visibility`): до первого события считает себя видимым.
    ui_visible: Rc<Cell<bool>>,
}

impl Global for Visibility {}

impl Visibility {
    pub fn new() -> Visibility {
        Visibility {
            state: WindowVisibility::default(),
            events: Rc::default(),
            tracker: Rc::default(),
            ui_visible: Rc::new(Cell::new(true)),
        }
    }

    /// Видимость для служб (потоки tokio).
    pub fn state(&self) -> WindowVisibility {
        self.state.clone()
    }

    /// Кому слать события окна. Подключать до показа окна.
    pub fn connect(&self, events: Box<dyn WindowEvents>) {
        *self.events.borrow_mut() = Some(events);
    }

    /// Сверять окно `handle` по событиям GPUI: видимость, фокус и первый кадр.
    pub fn watch(&self, handle: WindowHandle<LauncherRoot>, cx: &mut App) {
        cx.set_global(self.clone());
        let _ = handle.update(cx, |_, window, cx| {
            let this = self.clone();
            window
                .observe_window_visibility(move |_, window, cx| this.check(window, cx))
                .detach();
            let this = self.clone();
            cx.observe_window_activation(window, move |_, window, cx| this.check(window, cx))
                .detach();
            let this = self.clone();
            window.on_next_frame(move |window, cx| this.check(window, cx));
        });
    }

    fn check(&self, window: &Window, cx: &mut App) {
        self.observe(
            crate::window_ops::shown(window),
            window.is_window_active(),
            cx,
        );
    }

    fn observe(&self, visible: bool, focused: bool, cx: &mut App) {
        self.state.set(visible);
        AnimGate::set_window_hidden(cx, !visible);
        let changes = self
            .tracker
            .borrow_mut()
            .observe(visible, focused, self.ui_visible.get());
        if let Some(visible) = changes.ui_visible {
            self.tell_ui(cx, visible);
        }
        if let Some(events) = self.events.borrow().as_deref() {
            dispatch(changes, events);
        }
    }

    /// `window-visibility` интерфейсу (`CreateWindow.js`: `minimize`/`hide` → `false`,
    /// `restore`/`show` → `true`).
    fn tell_ui(&self, cx: &mut App, visible: bool) {
        self.ui_visible.set(visible);
        crate::window_ops::emit(cx, "window-visibility", serde_json::json!(visible));
    }

    /// «Свернуть» (`window-command minimize`): интерфейс узнаёт раньше, чем окно свернётся.
    pub fn set_ui_visible(cx: &mut App, visible: bool) {
        if let Some(this) = cx.try_global::<Visibility>().cloned() {
            this.tell_ui(cx, visible);
        }
    }

    /// Окно закрыто, а лаунчер работает (Linux: спрятано в трей) — не видно.
    #[cfg_attr(windows, allow(dead_code))]
    pub fn closed_window(cx: &mut App) {
        if let Some(this) = cx.try_global::<Visibility>().cloned() {
            this.observe(false, false, cx);
        }
    }

    /// Цикл событий кончился: окна больше нет (`isDestroyed()` 0.4.7 — не видно).
    pub fn closed(&self) {
        self.state.set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Journal(RefCell<Vec<&'static str>>);

    impl WindowEvents for Journal {
        fn ready(&self) {
            self.0.borrow_mut().push("ready");
        }
        fn shown(&self) {
            self.0.borrow_mut().push("show");
        }
        fn focused(&self) {
            self.0.borrow_mut().push("focus");
        }
    }

    /// Сверки `(видно, фокус)` → события и видимость у интерфейса после каждой сверки.
    fn replay(polls: &[(bool, bool)]) -> (Vec<&'static str>, Vec<bool>) {
        let mut tracker = Tracker::default();
        let journal = Journal::default();
        let mut ui_visible = true;
        let mut ui = Vec::new();
        for &(visible, focused) in polls {
            let changes = tracker.observe(visible, focused, ui_visible);
            if let Some(visible) = changes.ui_visible {
                ui_visible = visible;
            }
            ui.push(ui_visible);
            dispatch(changes, &journal);
        }
        (journal.0.take(), ui)
    }

    #[test]
    fn first_appearance_is_ready_then_show_then_focus() {
        // До показа окна интерфейс считает себя видимым (0.4.7 ничего не слал).
        let (events, ui) = replay(&[(false, false), (false, false), (true, true)]);
        assert_eq!(events, ["ready", "show", "focus"]);
        assert_eq!(ui, [true, true, true]);
    }

    #[test]
    fn taskbar_minimize_and_restore_reach_every_consumer() {
        // Свернули с панели задач (без команды окна): службы и интерфейс видят одно и то же.
        let (events, ui) = replay(&[(true, true), (false, false), (true, true)]);
        assert_eq!(events, ["ready", "show", "focus", "show", "focus"]);
        assert_eq!(ui, [true, false, true]);
    }

    #[test]
    fn ready_comes_once_and_focus_only_on_gain() {
        let (events, _) = replay(&[
            (true, false),
            (true, true),
            (true, false),
            (true, true),
            (false, false),
            (true, false),
        ]);
        assert_eq!(
            events,
            ["ready", "show", "focus", "focus", "show"],
            "второе появление — только `show`"
        );
    }

    #[test]
    fn window_commands_ahead_of_the_poll_are_kept_or_corrected() {
        let mut tracker = Tracker::default();
        assert_eq!(tracker.observe(true, true, true).ui_visible, None);
        // «Свернуть» уже сняло `visible`, окно свернулось — ничего не менять.
        assert_eq!(tracker.observe(false, false, false).ui_visible, None);
        // Окно развернули раньше сверки, а интерфейс ещё «не видно» — исправить.
        assert_eq!(tracker.observe(true, true, false).ui_visible, Some(true));
        // Команда сняла `visible`, а окно так и не свернулось — вернуть.
        assert_eq!(tracker.observe(true, true, false).ui_visible, Some(true));
    }

    #[test]
    fn taskbar_minimize_and_restore_follow_window_events() {
        // Сверки по порядку на событиях окна GPUI — `(видно, фокус)` окна в момент сверки.
        let mut tracker = Tracker::default();
        let journal = Journal::default();
        let mut ui_visible = true;
        let mut step = |visible: bool, focused: bool| {
            let changes = tracker.observe(visible, focused, ui_visible);
            if let Some(visible) = changes.ui_visible {
                ui_visible = visible;
            }
            dispatch(changes, &journal);
            (journal.0.take(), ui_visible)
        };
        assert_eq!(step(true, true), (vec!["ready", "show", "focus"], true));
        // Свернули с панели задач (`WM_SIZE`) — интерфейс гасит фон сразу (`minimize` 0.4.7).
        assert_eq!(step(false, false), (vec![], false));
        // Повторная сверка — ничего нового.
        assert_eq!(step(false, false), (vec![], false));
        // Разворот: фокус может прийти раньше `WM_SIZE` — окно ещё свёрнуто.
        assert_eq!(step(false, true), (vec!["focus"], false));
        // `WM_SIZE` — окно видно: `restore` 0.4.7 (фон, счётчики онлайна) сразу, второго `focus`
        // нет.
        assert_eq!(step(true, true), (vec!["show"], true));
        assert_eq!(step(true, true), (vec![], true));
    }

    #[test]
    fn shared_state_is_the_polled_value() {
        let state = WindowVisibility::default();
        assert!(!state.visible());
        let other = state.clone();
        other.set(true);
        assert!(state.visible());
    }
}
