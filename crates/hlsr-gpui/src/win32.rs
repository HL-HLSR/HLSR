//! Окно GPUI и Win32: HWND, углы DWM, показ поверх других окон, видимость, перенос окна (у GPUI
//! есть только `resize`).

use gpui::{Pixels, Point, Window};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle,
    Win32WindowHandle, WindowHandle,
};
use std::num::NonZeroIsize;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    DwmSetWindowAttribute,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IsIconic, IsWindowVisible, SW_RESTORE, SW_SHOW, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    SetForegroundWindow, SetWindowPos, ShowWindow,
};

/// HWND окна (кнопка панели задач, DWM).
pub fn hwnd(window: &Window) -> Option<isize> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

fn raw(window: &Window) -> Option<HWND> {
    hwnd(window).map(|hwnd| hwnd as HWND)
}

/// Окно-владелец по HWND ([`hwnd`]) для системного диалога в другом потоке
/// (`rfd::FileDialog::set_parent`): диалог модален окну и открывается на его мониторе.
pub struct Owner(pub isize);

impl HasWindowHandle for Owner {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.0).ok_or(HandleError::Unavailable)?;
        // SAFETY: HWND окна лаунчера; диалог только передаёт его системе, а неверный HWND система
        // отвергает ошибкой, не падением.
        Ok(unsafe { WindowHandle::borrow_raw(Win32WindowHandle::new(hwnd).into()) })
    }
}

impl HasDisplayHandle for Owner {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::windows())
    }
}

/// Форма задаётся клиентской областью (DirectComposition + GPU-маска углов `window_corner_mask`).
/// Отключаем системное скругление и контур DWM в Windows 11, чтобы они не конфликтовали с радиусом.
pub fn system_corners_off(window: &Window) {
    let Some(hwnd) = raw(window) else { return };
    let preference = DWMWCP_DONOTROUND;
    let border = DWMWA_COLOR_NONE;
    // SAFETY: HWND живого окна этого потока; значение — 4 байта на стеке на время вызова.
    let hr = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&raw const preference).cast(),
            size_of_val(&preference) as u32,
        )
    };
    if hr < 0 {
        tracing::debug!("[window] углы DWM не заданы: 0x{hr:08x}");
    }
    // SAFETY: HWND живого окна, значение атрибута живёт до возврата DWM.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR as u32,
            (&raw const border).cast(),
            size_of_val(&border) as u32,
        );
    }
}

/// `revealWindow` 0.4.7: развернуть, показать, отдать фокус.
pub fn reveal(window: &Window) {
    let Some(hwnd) = raw(window) else { return };
    // SAFETY: HWND живого окна этого потока.
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }
}

/// `isVisible() && !isMinimized()` окна.
pub fn shown(window: &Window) -> bool {
    let Some(hwnd) = raw(window) else {
        return false;
    };
    // SAFETY: HWND живого окна этого потока.
    unsafe { IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0 }
}

/// Перенести окно: `origin` — логические px окна.
pub fn move_to(window: &Window, origin: Point<Pixels>) {
    let Some(hwnd) = raw(window) else { return };
    let scale = window.scale_factor();
    let x = (f32::from(origin.x) * scale).round() as i32;
    let y = (f32::from(origin.y) * scale).round() as i32;
    // SAFETY: HWND живого окна этого потока; размер и порядок окон не меняются.
    let ok = unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
    if ok == 0 {
        tracing::warn!(
            "[window] окно не перенесено: {}",
            std::io::Error::last_os_error()
        );
    }
}
