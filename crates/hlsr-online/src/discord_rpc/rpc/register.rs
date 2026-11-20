//! `register(id)` библиотеки `discord-rpc`: в Electron это `app.setAsDefaultProtocolClient(
//! "discord-<id>")` (`src/util.js`), то есть лаунчер при каждой попытке подключения делает
//! `discord-1531057982518071296` обработчиком своего протокола (решение 8 спецификации: повторено
//! 1:1, хотя Discord от этого ничего не получает). Что пишет Electron 44:
//! * Windows — `HKCU\Software\Classes\<протокол>`: значение по умолчанию `URL:<протокол>`, `URL
//!   Protocol` (пустая строка) и `shell\open\command` со значением по умолчанию `"<exe>" "%1"`,
//!   где exe — исполняемый файл процесса. Всё `REG_SZ`; остановка на первой ошибке.
//! * Linux — `xdg-settings set default-url-scheme-handler <протокол> <рабочий стол>`; рабочий
//!   стол — `hlsr.desktop` (`app.setDesktopName`); xdg-settings сам пишет `mimeapps.list`.
//!   Успех — код выхода 0.
//! * Остальные ОС — `false`.

use std::sync::Mutex;

/// `app.setDesktopName("hlsr.desktop")` в `src/main/index.js`.
pub const DESKTOP_NAME: &str = "hlsr.desktop";

/// `app.setAsDefaultProtocolClient(protocol)`: `false` — не удалось (пустой протокол, нет
/// доступа к реестру, нет `xdg-settings`).
pub trait ProtocolClient: Send + Sync {
    fn set_as_default_protocol_client(&self, protocol: &str) -> bool;
}

/// Значение реестра, которое пишет Electron: ключ от `HKCU` (или другого корня), имя значения
/// (пустое — значение по умолчанию) и строка `REG_SZ`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryEntry {
    pub key: String,
    pub name: &'static str,
    pub value: String,
}

/// Команда обработчика: `"<exe>" "%1"`.
pub fn windows_command(exe: &str) -> String {
    format!("\"{exe}\" \"%1\"")
}

/// Что пишет Electron на Windows, в порядке записи.
pub fn windows_entries(protocol: &str, exe: &str) -> Vec<RegistryEntry> {
    let protocol_key = format!("Software\\Classes\\{protocol}");
    vec![
        RegistryEntry {
            key: format!("{protocol_key}\\shell\\open\\command"),
            name: "",
            value: windows_command(exe),
        },
        RegistryEntry {
            key: protocol_key.clone(),
            name: "",
            value: format!("URL:{protocol}"),
        },
        RegistryEntry {
            key: protocol_key,
            name: "URL Protocol",
            value: String::new(),
        },
    ]
}

/// Аргументы `xdg-settings`, как их собирает Electron на Linux.
pub fn xdg_settings_args(protocol: &str) -> Vec<String> {
    ["set", "default-url-scheme-handler", protocol, DESKTOP_NAME]
        .iter()
        .map(|arg| (*arg).to_owned())
        .collect()
}

#[cfg(windows)]
pub mod windows {
    //! Запись в реестр Windows через Win32. Корень передаётся: настоящий — `HKEY_CURRENT_USER`,
    //! в тестах — личный куст приложения (`RegLoadAppKeyW`), чужой реестр они не видят.

    use super::{RegistryEntry, windows_entries};
    use hlsr_core::win32::wide;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegSetValueExW,
    };

    /// Корень записи: открытый ключ реестра.
    #[derive(Debug, Clone, Copy)]
    pub struct Root(HKEY);

    impl Root {
        /// `HKEY_CURRENT_USER`.
        pub fn current_user() -> Root {
            Root(HKEY_CURRENT_USER)
        }

        /// Чужой открытый ключ (личный куст в тестах).
        ///
        /// # Safety
        /// `key` — действительный дескриптор ключа с правом записи, открытый на всё время
        /// использования корня.
        pub unsafe fn from_raw(key: HKEY) -> Root {
            Root(key)
        }
    }

    struct Handle(HKEY);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: дескриптор получен от RegCreateKeyExW и закрывается один раз.
            unsafe { RegCloseKey(self.0) };
        }
    }

    /// Записать значение (`RegKey::Create` + `WriteValue` Chromium); ключ создаётся при нужде.
    pub fn write_value(root: Root, entry: &RegistryEntry) -> std::io::Result<()> {
        let key = wide(&entry.key);
        let mut raw: HKEY = std::ptr::null_mut();
        // SAFETY: строка завершена нулём, `raw` — локальная переменная для дескриптора.
        let status = unsafe {
            RegCreateKeyExW(
                root.0,
                key.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut raw,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(status as i32));
        }
        let handle = Handle(raw);
        let name = wide(entry.name);
        let data = wide(&entry.value);
        // SAFETY: имя и данные — буферы с нулём в конце; размер данных — в байтах, с нулём.
        let status = unsafe {
            RegSetValueExW(
                handle.0,
                name.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr().cast::<u8>(),
                (data.len() * 2) as u32,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(status as i32));
        }
        Ok(())
    }

    /// Записать обработчик протокола под `root`; остановка на первой ошибке, как у Electron.
    pub fn register_in(root: Root, protocol: &str, exe: &str) -> bool {
        if protocol.is_empty() {
            return false;
        }
        windows_entries(protocol, exe)
            .iter()
            .all(|entry| write_value(root, entry).is_ok())
    }

    /// Настоящий реестр текущего пользователя.
    pub fn register(protocol: &str) -> bool {
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        register_in(Root::current_user(), protocol, &exe.to_string_lossy())
    }
}

/// `app.setAsDefaultProtocolClient` ОС.
#[derive(Debug, Clone)]
pub struct SystemProtocolClient {
    #[cfg(target_os = "linux")]
    xdg_settings: std::path::PathBuf,
}

// На Linux в поле — путь `xdg-settings`, поэтому `derive(Default)` не подходит.
#[allow(clippy::derivable_impls)]
impl Default for SystemProtocolClient {
    fn default() -> SystemProtocolClient {
        SystemProtocolClient {
            #[cfg(target_os = "linux")]
            xdg_settings: std::path::PathBuf::from("xdg-settings"),
        }
    }
}

impl SystemProtocolClient {
    /// С другой программой вместо `xdg-settings` (тесты кладут сценарий-подделку).
    #[cfg(target_os = "linux")]
    pub fn with_xdg_settings(path: impl Into<std::path::PathBuf>) -> SystemProtocolClient {
        SystemProtocolClient {
            xdg_settings: path.into(),
        }
    }
}

impl ProtocolClient for SystemProtocolClient {
    fn set_as_default_protocol_client(&self, protocol: &str) -> bool {
        if protocol.is_empty() {
            return false;
        }
        #[cfg(windows)]
        {
            windows::register(protocol)
        }
        #[cfg(target_os = "linux")]
        {
            use std::process::{Command, Stdio};
            Command::new(&self.xdg_settings)
                .args(xdg_settings_args(protocol))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            false
        }
    }
}

/// Подделка для тестов: записывает протоколы, отвечает заданным.
#[derive(Debug)]
pub struct FakeProtocolClient {
    calls: Mutex<Vec<String>>,
    result: bool,
}

impl FakeProtocolClient {
    pub fn new(result: bool) -> FakeProtocolClient {
        FakeProtocolClient {
            calls: Mutex::new(Vec::new()),
            result,
        }
    }

    /// Протоколы, которые просили, с начала.
    pub fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }
}

impl ProtocolClient for FakeProtocolClient {
    fn set_as_default_protocol_client(&self, protocol: &str) -> bool {
        self.calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(protocol.to_owned());
        self.result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_entries_like_electron() {
        let entries = windows_entries("discord-1531057982518071296", "C:\\Games\\HLSR\\HLSR.exe");
        assert_eq!(
            entries,
            vec![
                RegistryEntry {
                    key: "Software\\Classes\\discord-1531057982518071296\\shell\\open\\command"
                        .to_owned(),
                    name: "",
                    value: "\"C:\\Games\\HLSR\\HLSR.exe\" \"%1\"".to_owned(),
                },
                RegistryEntry {
                    key: "Software\\Classes\\discord-1531057982518071296".to_owned(),
                    name: "",
                    value: "URL:discord-1531057982518071296".to_owned(),
                },
                RegistryEntry {
                    key: "Software\\Classes\\discord-1531057982518071296".to_owned(),
                    name: "URL Protocol",
                    value: String::new(),
                },
            ]
        );
    }

    #[test]
    fn xdg_settings_arguments() {
        assert_eq!(
            xdg_settings_args("discord-1531057982518071296"),
            [
                "set",
                "default-url-scheme-handler",
                "discord-1531057982518071296",
                "hlsr.desktop"
            ]
        );
    }

    #[test]
    fn empty_protocol_is_refused() {
        assert!(!SystemProtocolClient::default().set_as_default_protocol_client(""));
    }
}
