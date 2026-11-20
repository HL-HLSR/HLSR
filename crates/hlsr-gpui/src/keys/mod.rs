//! Захват физической клавиши для биндов игры (кнопка «назначить» во вкладках «Клавиатура» и
//! «Скрипты»). Машина захвата — [`hlsr_ui::keys::KeyInput`] (`GameScripts.vue` 0.4.7): ждёт
//! нажатие, кнопку мыши или колесо над кнопкой, Esc — отмена, щелчок вне кнопки — отмена.
//!
//! `Keystroke` GPUI физического кода не даёт (скан-кода, стороны Shift/Ctrl/Alt, цифрового
//! блока), поэтому на Windows на время захвата ставится перехват сообщений потока окна
//! ([`win32`]): скан-код → код UI Events ([`scancode`]) → имя движка. Нажатия захвата до GPUI не
//! доходят. На Linux вид передаёт события GPUI через [`events`] и [`feed`].
//!
//! Захват один на поток: новый [`start`] отменяет прежний без итога, как `startCapture` 0.4.7;
//! закрытие окна захвата — тоже.

pub mod events;
mod scancode;
#[cfg(windows)]
mod win32;

pub use scancode::code_name;

use gpui::{App, AppContext, Bounds, Pixels, Subscription, Window};
use hlsr_ui::keyboard_layout::Engine;
use hlsr_ui::keys::{Area, CaptureEnd, KeyInput, RawInput, Verdict};
use std::cell::RefCell;

struct Active {
    input: KeyInput,
    deliver: Box<dyn FnOnce(CaptureEnd)>,
    /// Окно закрылось — захват снимается без итога.
    _closed: Subscription,
    /// Перехват снимается вместе с захватом (деструктор).
    #[cfg(windows)]
    _hook: win32::Hook,
}

thread_local! {
    static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) };
}

/// Ждать клавишу для `owner` (команда бинда); `area` — кнопка захвата в координатах окна.
/// Итог (клавиша или `None` — отмена) приходит в `on_end` задачей окна; [`stop`] — без итога.
pub fn start(
    window: &Window,
    cx: &App,
    owner: &str,
    engine: Engine,
    area: Bounds<Pixels>,
    on_end: impl FnOnce(CaptureEnd, &mut Window, &mut App) + 'static,
) {
    stop();
    let mut input = KeyInput::new();
    input.start_capture(owner, engine, to_area(area));
    let executor = cx.foreground_executor().clone();
    let mut async_cx = cx.to_async();
    let handle = window.window_handle();
    // Итог может прийти из перехвата сообщений, вне обновления приложения: только задачей окна.
    let deliver: Box<dyn FnOnce(CaptureEnd)> = Box::new(move |end| {
        executor
            .spawn(async move {
                async_cx
                    .update_window(handle, |_, window, cx| on_end(end, window, cx))
                    .ok();
            })
            .detach();
    });
    // Без перехвата ввод захвата на Windows не приходит: сразу отмена, а не вечное ожидание.
    #[cfg(windows)]
    let Some(hook) = win32::Hook::install(window) else {
        deliver(CaptureEnd {
            owner: owner.to_owned(),
            key: None,
        });
        return;
    };
    let window_id = handle.window_id();
    let closed = cx.on_window_closed(move |_, closed| {
        if closed == window_id {
            stop();
        }
    });
    let active = Active {
        input,
        deliver,
        _closed: closed,
        #[cfg(windows)]
        _hook: hook,
    };
    ACTIVE.with_borrow_mut(|slot| *slot = Some(active));
}

/// Кнопка захвата сдвинулась (раскладка, прокрутка).
pub fn set_area(area: Bounds<Pixels>) {
    ACTIVE.with_borrow_mut(|slot| {
        if let Some(active) = slot {
            active.input.set_capture_area(to_area(area));
        }
    });
}

/// Прервать ожидание без итога (`stopCapture`); вернуть, кто ждал.
pub fn stop() -> Option<String> {
    ACTIVE
        .with_borrow_mut(Option::take)
        .and_then(|mut active| active.input.stop_capture())
}

/// Кто ждёт клавишу.
pub fn capturing() -> Option<String> {
    ACTIVE.with_borrow(|slot| {
        slot.as_ref()
            .and_then(|active| active.input.capturing().map(str::to_owned))
    })
}

/// Событие окна для захвата: `Consume` — дальше его не пускать. Без захвата — всегда `Pass`.
pub fn feed(input: &RawInput) -> Verdict {
    let (verdict, done) = ACTIVE.with_borrow_mut(|slot| {
        let Some(active) = slot.as_mut() else {
            return (Verdict::Pass, None);
        };
        let handled = active.input.handle(input, false);
        let done = handled
            .capture
            .and_then(|end| slot.take().map(|active| (end, active)));
        (handled.verdict, done)
    });
    // Вне заимствования: снятие перехвата и итог.
    if let Some((end, active)) = done {
        (active.deliver)(end);
    }
    verdict
}

fn to_area(bounds: Bounds<Pixels>) -> Area {
    Area {
        x: f64::from(f32::from(bounds.origin.x)),
        y: f64::from(f32::from(bounds.origin.y)),
        width: f64::from(f32::from(bounds.size.width)),
        height: f64::from(f32::from(bounds.size.height)),
    }
}
