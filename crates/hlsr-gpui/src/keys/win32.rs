//! Перехват ввода окна на время захвата (Windows): хук `WH_GETMESSAGE` потока окна видит
//! сообщение, когда цикл GPUI достаёт его из очереди, — раньше GPUI. Подкласс WndProc не годится:
//! `WM_KEYDOWN`/`WM_SYSKEYDOWN` GPUI разбирает в цикле сообщений (`translate_accelerator`) до
//! `DispatchMessageW`, и нажатие, которое забрала привязка клавиш, до WndProc не доходит.
//! Поглощённое сообщение становится `WM_NULL`: ни GPUI, ни `TranslateMessage` (`WM_CHAR`), ни
//! `DefWindowProcW` (меню по Alt/F10) его не видят.

use hlsr_ui::keys::{RawInput, Verdict};
use std::cell::Cell;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetWindowThreadProcessId, HHOOK, MSG, PM_REMOVE, SetWindowsHookExW,
    UnhookWindowsHookEx, WH_GETMESSAGE, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_MBUTTONDOWN,
    WM_MOUSEWHEEL, WM_NULL, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, XBUTTON1,
    XBUTTON2,
};

thread_local! {
    /// Окно захвата и его масштаб (физические px → логические px GPUI).
    static TARGET: Cell<Option<(HWND, f64)>> = const { Cell::new(None) };
}

/// Перехват стоит, пока жив.
pub(super) struct Hook(HHOOK);

impl Hook {
    pub(super) fn install(window: &gpui::Window) -> Option<Hook> {
        let hwnd = crate::win32::hwnd(window)? as HWND;
        // SAFETY: HWND живого окна этого потока; хук только этого потока (hmod = null), процедура —
        // функция этого модуля.
        let hook = unsafe {
            let thread = GetWindowThreadProcessId(hwnd, std::ptr::null_mut());
            SetWindowsHookExW(
                WH_GETMESSAGE,
                Some(get_message),
                std::ptr::null_mut(),
                thread,
            )
        };
        if hook.is_null() {
            tracing::warn!(
                "[keys] перехват клавиш не встал: {}",
                std::io::Error::last_os_error()
            );
            return None;
        }
        // shortcut: масштаб на начало захвата; смена DPI посреди ожидания сдвинет область кнопки.
        TARGET.set(Some((hwnd, f64::from(window.scale_factor()))));
        Some(Hook(hook))
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        TARGET.set(None);
        // SAFETY: хук поставлен этим потоком и снимается один раз; снимать изнутри процедуры можно.
        unsafe { UnhookWindowsHookEx(self.0) };
    }
}

unsafe extern "system" fn get_message(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // PM_NOREMOVE — только подглядывание: сообщение придёт ещё раз.
    if code >= 0
        && wparam == PM_REMOVE as WPARAM
        && let Some((hwnd, scale)) = TARGET.get()
    {
        // SAFETY: у WH_GETMESSAGE lParam — MSG, который вернут GetMessageW/PeekMessageW; менять можно.
        let msg = unsafe { &mut *(lparam as *mut MSG) };
        if msg.hwnd == hwnd
            && let Some(input) = raw_input(msg, scale)
            && super::feed(&input) == Verdict::Consume
        {
            msg.message = WM_NULL;
        }
    }
    // SAFETY: обычная передача следующему хуку; hhk система не смотрит.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

/// Сообщение окна → ввод в форме DOM; остальные сообщения захват не трогает.
fn raw_input(msg: &MSG, scale: f64) -> Option<RawInput> {
    let (wparam, lparam) = (msg.wParam, msg.lParam);
    let at = |x: i32, y: i32| (f64::from(x) / scale, f64::from(y) / scale);
    let button = |button| {
        let (x, y) = at(low(lparam), high(lparam));
        Some(RawInput::MouseDown { button, x, y })
    };
    match msg.message {
        WM_KEYDOWN | WM_SYSKEYDOWN => Some(RawInput::KeyDown {
            code: key_code(lparam).to_owned(),
            repeat: lparam & (1 << 30) != 0,
        }),
        WM_KEYUP | WM_SYSKEYUP => Some(RawInput::KeyUp {
            code: key_code(lparam).to_owned(),
        }),
        WM_LBUTTONDOWN => button(0),
        WM_MBUTTONDOWN => button(1),
        WM_RBUTTONDOWN => button(2),
        WM_XBUTTONDOWN => match (wparam >> 16) as u16 {
            XBUTTON1 => button(3),
            XBUTTON2 => button(4),
            _ => None,
        },
        WM_MOUSEWHEEL => {
            // Колесо приходит в координатах экрана.
            let mut point = POINT {
                x: low(lparam),
                y: high(lparam),
            };
            // SAFETY: HWND окна сообщения, POINT на стеке.
            unsafe { ScreenToClient(msg.hwnd, &mut point) };
            let (x, y) = at(point.x, point.y);
            // «От себя» у Windows — плюс, у DOM `deltaY` — минус.
            let delta_y = -f64::from((wparam >> 16) as u16 as i16);
            Some(RawInput::Wheel { delta_y, x, y })
        }
        _ => None,
    }
}

/// Код клавиши по `lParam`, как `GetScanCodeFromLParam` Chromium.
fn key_code(lparam: LPARAM) -> &'static str {
    let scancode = ((lparam >> 16) & 0xFF) as u16;
    let extended = lparam & (1 << 24) != 0;
    super::scancode::code_name(if extended {
        0xE000 | scancode
    } else {
        scancode
    })
}

fn low(lparam: LPARAM) -> i32 {
    i32::from(lparam as u16 as i16)
}

fn high(lparam: LPARAM) -> i32 {
    i32::from((lparam >> 16) as u16 as i16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(message: u32, wparam: WPARAM, lparam: LPARAM) -> MSG {
        MSG {
            message,
            wParam: wparam,
            lParam: lparam,
            ..Default::default()
        }
    }

    #[test]
    fn messages_become_dom_input() {
        // Правый Shift: скан-код 0x36; повтор — бит 30.
        assert_eq!(
            raw_input(&msg(WM_KEYDOWN, 0x10, 0x0036_0001 | 1 << 30), 1.0),
            Some(RawInput::KeyDown {
                code: "ShiftRight".into(),
                repeat: true
            })
        );
        // Enter цифрового блока с Alt: расширенный 0x1C, контекст Alt — бит 29.
        assert_eq!(
            raw_input(&msg(WM_SYSKEYUP, 0x0D, 0x011C_0001 | 1 << 29), 1.0),
            Some(RawInput::KeyUp {
                code: "NumpadEnter".into()
            })
        );
        // Кнопка «назад» в (30, -20) физических px при масштабе 2.
        let lparam = (30 & 0xFFFF) | ((-20_i32 as u16 as isize) << 16);
        assert_eq!(
            raw_input(&msg(WM_XBUTTONDOWN, (XBUTTON1 as usize) << 16, lparam), 2.0),
            Some(RawInput::MouseDown {
                button: 3,
                x: 15.0,
                y: -10.0
            })
        );
        assert!(matches!(
            raw_input(&msg(WM_XBUTTONDOWN, (XBUTTON2 as usize) << 16, 0), 1.0),
            Some(RawInput::MouseDown { button: 4, .. })
        ));
        assert_eq!(raw_input(&msg(WM_NULL, 0, 0), 1.0), None);
    }
}
