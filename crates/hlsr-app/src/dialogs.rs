//! Системные окна сообщений до главного окна: ошибка запуска и запущенный 0.4.

const STARTUP_ERROR_TITLE: &str = "HLSR — ошибка запуска / startup error";
const STARTUP_ERROR_TEXT: &str = "Лаунчер не смог запуститься. Подробности в логе.\n\
     The launcher failed to start. See the log for details.\n\n";
/// `message.slice(0, 600)` в `reportStartupError` 0.4.7.
const STARTUP_ERROR_DETAIL: usize = 600;

const OLD_LAUNCHER_TITLE: &str = "HLSR";
const OLD_LAUNCHER_TEXT: &str = "Закройте запущенный HLSR 0.4 и запустите лаунчер снова.\n\
     Close the running HLSR 0.4 and start the launcher again.";

/// Текст окна ошибки запуска — `dialog.showErrorBox` 0.4.7.
pub fn startup_error_text(message: &str) -> String {
    format!(
        "{STARTUP_ERROR_TEXT}{}",
        slice_utf16(message, STARTUP_ERROR_DETAIL)
    )
}

pub fn startup_error(message: &str) {
    show(STARTUP_ERROR_TITLE, &startup_error_text(message), true);
}

pub fn close_old_launcher() {
    show(OLD_LAUNCHER_TITLE, OLD_LAUNCHER_TEXT, false);
}

/// Ошибка окна установщика или удаления до окна (`dialog.showErrorBox("HLSR", message.slice(0, 600))`
/// в `windows-install/setup.js`, ПП6).
pub fn setup_error(title: &str, message: &str) {
    show(title, slice_utf16(message, STARTUP_ERROR_DETAIL), true);
}

/// Предупреждение до окна (`dialog.showMessageBoxSync({ type: "warning", title: "HLSR" })`; удаление
/// Windows, `modes/setup.rs`).
#[cfg(windows)]
pub fn setup_warning(text: &str) {
    show("HLSR", text, false);
}

/// Первые `len` единиц UTF-16, как `String.prototype.slice`; суррогатная
/// пара на границе не режется пополам, а отбрасывается целиком.
fn slice_utf16(text: &str, len: usize) -> &str {
    let mut units = 0;
    for (index, c) in text.char_indices() {
        units += c.len_utf16();
        if units > len {
            return &text[..index];
        }
    }
    text
}

/// Окно сообщения. Без него (нет zenity на Linux) остаются журнал и stderr.
fn show(title: &str, text: &str, error: bool) {
    eprintln!("{title}\n{text}");
    platform::message_box(title, text, error);
}

#[cfg(windows)]
mod platform {
    use hlsr_core::win32::wide;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MB_ICONERROR, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MessageBoxW,
    };

    pub fn message_box(title: &str, text: &str, error: bool) {
        let (title, text) = (wide(title), wide(text));
        let icon = if error { MB_ICONERROR } else { MB_ICONWARNING };
        // SAFETY: строки с нулём в конце живут до конца вызова; окна-владельца нет.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_OK | icon | MB_SETFOREGROUND,
            )
        };
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn message_box(title: &str, text: &str, error: bool) {
        let level = if error {
            rfd::MessageLevel::Error
        } else {
            rfd::MessageLevel::Warning
        };
        rfd::MessageDialog::new()
            .set_level(level)
            .set_title(title)
            .set_description(text)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_error_text_matches_0_4_7() {
        assert_eq!(
            startup_error_text("boom"),
            "Лаунчер не смог запуститься. Подробности в логе.\n\
             The launcher failed to start. See the log for details.\n\nboom"
        );
        let long = "я".repeat(700);
        assert_eq!(
            startup_error_text(&long).strip_prefix(STARTUP_ERROR_TEXT),
            Some("я".repeat(600).as_str())
        );
        // 599 единиц + эмодзи из двух: в 600 не влезает — отбрасывается.
        let edge = format!("{}😀x", "a".repeat(599));
        assert!(startup_error_text(&edge).ends_with(&"a".repeat(599)));
    }
}
