//! Видимость окна хоста — то же вычисление, что `hlsr-app/src/visibility.rs`, но состояние окна
//! сообщает хост (`hlsr_window_state`), а не GPUI.

use std::sync::Arc;
use std::sync::Mutex;
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

/// События окна для служб.
pub trait WindowEvents: Send + Sync {
    fn ready(&self);
    fn shown(&self);
    fn focused(&self);
}

#[derive(Default)]
struct Tracker {
    ready: bool,
    visible: bool,
    focused: bool,
}

/// Сверка окна: хост сообщает видимость и фокус, службы получают `ready`/`show`/`focus`.
pub struct Visibility {
    state: WindowVisibility,
    tracker: Mutex<Tracker>,
    events: Mutex<Option<Box<dyn WindowEvents>>>,
}

impl Visibility {
    pub fn new() -> Visibility {
        Visibility {
            state: WindowVisibility::default(),
            tracker: Mutex::new(Tracker::default()),
            events: Mutex::new(None),
        }
    }

    pub fn state(&self) -> WindowVisibility {
        self.state.clone()
    }

    pub fn connect(&self, events: Box<dyn WindowEvents>) {
        if let Ok(mut slot) = self.events.lock() {
            *slot = Some(events);
        }
    }

    /// Окно сейчас: видно ли и в фокусе ли.
    pub fn observe(&self, visible: bool, focused: bool) {
        self.state.set(visible);
        let (ready, shown, gained) = {
            let Ok(mut t) = self.tracker.lock() else {
                return;
            };
            let shown = visible && !t.visible;
            let ready = shown && !t.ready;
            t.ready |= ready;
            let gained = focused && !t.focused;
            t.visible = visible;
            t.focused = focused;
            (ready, shown, gained)
        };
        crate::host::emit("window-visibility", &serde_json::json!(visible));
        let Ok(events) = self.events.lock() else {
            return;
        };
        let Some(events) = events.as_ref() else {
            return;
        };
        if ready {
            events.ready();
        }
        if shown {
            events.shown();
        }
        if gained {
            events.focused();
        }
    }

    pub fn closed(&self) {
        self.state.set(false);
        if let Ok(mut slot) = self.events.lock() {
            slot.take();
        }
    }
}
