//! Окно-хост на стороне C# (WinUI) вместо `hlsr_gpui::bridge`, `window`, `window_ops` и `tray`
//! лаунчера на GPUI: всё, что службы просили у окна, уходит хосту событием `host:<команда>` через
//! один обратный вызов; HWND и видимость хост сообщает сам ([`set_hwnd`], `hlsr_window_state`).

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, Win32WindowHandle, WindowHandle,
};
use serde_json::Value;
use std::ffi::{CString, c_char};
use std::num::NonZeroIsize;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::RwLock;

/// Обратный вызов хоста: канал и JSON, обе строки UTF-8 с нулём на конце; живут только на время
/// вызова. Зовётся из потоков tokio — хост сам переносит работу в поток окна.
pub type EventCallback = extern "C" fn(channel: *const c_char, payload: *const c_char);

static CALLBACK: RwLock<Option<EventCallback>> = RwLock::new(None);
static HWND: AtomicIsize = AtomicIsize::new(0);
static SHOWN: AtomicBool = AtomicBool::new(false);
static QUIT: AtomicBool = AtomicBool::new(false);

pub fn set_callback(callback: Option<EventCallback>) {
    if let Ok(mut slot) = CALLBACK.write() {
        *slot = callback;
    }
}

pub fn set_hwnd(hwnd: isize) {
    HWND.store(hwnd, Ordering::SeqCst);
}

pub fn set_shown(shown: bool) {
    SHOWN.store(shown, Ordering::SeqCst);
}

/// Событие хосту. `false` — хоста нет (окно закрыто).
pub fn emit(channel: &str, payload: &Value) -> bool {
    let Ok(slot) = CALLBACK.read() else {
        return false;
    };
    let Some(callback) = *slot else {
        return false;
    };
    let channel = CString::new(channel.replace('\0', "")).unwrap_or_default();
    let payload = CString::new(payload.to_string().replace('\0', "")).unwrap_or_default();
    callback(channel.as_ptr(), payload.as_ptr());
    true
}

/// Команда окну: `host:<name>`.
pub fn command(name: &str, payload: Value) -> bool {
    emit(&format!("host:{name}"), &payload)
}

/// `app.quit()`: хост закрывает окно и зовёт `hlsr_shutdown`.
pub fn quit() {
    QUIT.store(true, Ordering::SeqCst);
    command("quit", Value::Null);
}

pub fn quit_requested() -> bool {
    QUIT.load(Ordering::SeqCst)
}

/// Окно открыто (в том числе спрятано в трей).
pub fn window_shown() -> bool {
    SHOWN.load(Ordering::SeqCst)
}

/// HWND окна лаунчера.
pub fn hwnd() -> Option<isize> {
    Some(HWND.load(Ordering::SeqCst)).filter(|hwnd| *hwnd != 0)
}

/// Владелец системных диалогов — окно лаунчера.
pub async fn dialog_owner() -> Option<isize> {
    hwnd()
}

/// Окно-владелец по HWND для `rfd::FileDialog::set_parent`.
pub struct Owner(pub isize);

impl HasWindowHandle for Owner {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.0).ok_or(HandleError::Unavailable)?;
        // SAFETY: HWND окна хоста; диалог только передаёт его системе.
        Ok(unsafe { WindowHandle::borrow_raw(Win32WindowHandle::new(hwnd).into()) })
    }
}

impl HasDisplayHandle for Owner {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::windows())
    }
}
