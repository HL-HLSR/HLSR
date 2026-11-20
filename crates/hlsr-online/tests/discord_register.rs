//! Регистрация протокола `discord-<id>` (`register` из `discord-rpc`, решение 8): что пишется в
//! реестр Windows и что просят у `xdg-settings` на Linux.
//!
//! Настоящий реестр и настоящий `xdg-settings` тесты не трогают. Windows: запись идёт в личный
//! куст приложения (`RegLoadAppKeyW` на файл во временной папке) — он виден только этому
//! процессу и не часть реестра. Linux: вместо `xdg-settings` — сценарий-подделка во временной
//! папке, пишущий свои аргументы в файл. Что именно пишет Electron 44, взято из его кода:
//! строки `Software\Classes\`, `\shell`, `\open\command`, `URL Protocol`, `URL:` есть в
//! `electron.exe` эталона, `DefaultIcon` в нём нет.

#[cfg(windows)]
mod windows {
    use hlsr_core::win32::wide;
    use hlsr_online::discord_rpc::rpc::register::windows::{Root, register_in, write_value};
    use hlsr_online::discord_rpc::rpc::register::{RegistryEntry, windows_command};
    use std::path::Path;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, KEY_ALL_ACCESS, KEY_READ, REG_SZ, RegCloseKey, RegLoadAppKeyW, RegOpenKeyExW,
        RegQueryValueExW,
    };

    const PROTOCOL: &str = "discord-1531057982518071296";
    const EXE: &str = "C:\\Games\\HLSR\\HLSR.exe";

    /// Личный куст приложения в файле: пустой, виден только этому процессу.
    struct Hive(HKEY);

    impl Hive {
        fn root(&self) -> Root {
            // SAFETY: дескриптор куста открыт с KEY_ALL_ACCESS и живёт, пока жив `Hive`.
            unsafe { Root::from_raw(self.0) }
        }

        fn load(file: &Path) -> Hive {
            let path = wide(file);
            let mut hive: HKEY = std::ptr::null_mut();
            // SAFETY: путь завершён нулём, `hive` — локальная переменная для дескриптора.
            let status = unsafe { RegLoadAppKeyW(path.as_ptr(), &mut hive, KEY_ALL_ACCESS, 0, 0) };
            assert_eq!(status, ERROR_SUCCESS, "RegLoadAppKeyW: {status}");
            Hive(hive)
        }

        /// Значение `REG_SZ`; `None` — нет ключа или значения.
        fn read(&self, key: &str, name: &str) -> Option<String> {
            let key = wide(key);
            let name = wide(name);
            let mut opened: HKEY = std::ptr::null_mut();
            // SAFETY: строки завершены нулём, `opened` — локальная переменная.
            let status = unsafe { RegOpenKeyExW(self.0, key.as_ptr(), 0, KEY_READ, &mut opened) };
            if status != ERROR_SUCCESS {
                return None;
            }
            let mut kind = 0u32;
            let mut size = 0u32;
            // SAFETY: пустой буфер с размером — запрос длины.
            let first = unsafe {
                RegQueryValueExW(
                    opened,
                    name.as_ptr(),
                    std::ptr::null(),
                    &mut kind,
                    std::ptr::null_mut(),
                    &mut size,
                )
            };
            let mut data = vec![0u16; (size as usize).div_ceil(2)];
            let second = if first == ERROR_SUCCESS {
                // SAFETY: буфер на `size` байт.
                unsafe {
                    RegQueryValueExW(
                        opened,
                        name.as_ptr(),
                        std::ptr::null(),
                        &mut kind,
                        data.as_mut_ptr().cast::<u8>(),
                        &mut size,
                    )
                }
            } else {
                first
            };
            // SAFETY: дескриптор получен выше и закрывается один раз.
            unsafe { RegCloseKey(opened) };
            if second != ERROR_SUCCESS {
                return None;
            }
            assert_eq!(kind, REG_SZ, "значение {key:?} не REG_SZ");
            let units = (size as usize / 2).min(data.len());
            let text = &data[..units];
            let end = text
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(text.len());
            Some(String::from_utf16_lossy(&text[..end]))
        }
    }

    impl Drop for Hive {
        fn drop(&mut self) {
            // SAFETY: дескриптор получен от RegLoadAppKeyW и закрывается один раз.
            unsafe { RegCloseKey(self.0) };
        }
    }

    #[test]
    fn writes_what_electron_writes() {
        let dir = tempfile::tempdir().unwrap();
        let hive = Hive::load(&dir.path().join("hlsr-test.hiv"));

        assert!(register_in(hive.root(), PROTOCOL, EXE));

        let key = format!("Software\\Classes\\{PROTOCOL}");
        assert_eq!(
            hive.read(&format!("{key}\\shell\\open\\command"), ""),
            Some(windows_command(EXE))
        );
        assert_eq!(
            hive.read(&format!("{key}\\shell\\open\\command"), ""),
            Some("\"C:\\Games\\HLSR\\HLSR.exe\" \"%1\"".to_owned())
        );
        assert_eq!(hive.read(&key, ""), Some(format!("URL:{PROTOCOL}")));
        assert_eq!(hive.read(&key, "URL Protocol"), Some(String::new()));
        // DefaultIcon Electron 44 не пишет.
        assert_eq!(hive.read(&format!("{key}\\DefaultIcon"), ""), None);
    }

    #[test]
    fn second_call_overwrites_and_empty_protocol_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let hive = Hive::load(&dir.path().join("hlsr-test.hiv"));

        assert!(!register_in(hive.root(), "", EXE));
        assert!(register_in(hive.root(), PROTOCOL, "C:\\old\\HLSR.exe"));
        assert!(register_in(hive.root(), PROTOCOL, EXE));
        assert_eq!(
            hive.read(
                &format!("Software\\Classes\\{PROTOCOL}\\shell\\open\\command"),
                ""
            ),
            Some(windows_command(EXE))
        );
    }

    #[test]
    fn write_error_stops_registration() {
        // Закрытый корень: запись невозможна, `register_in` отвечает `false`.
        let entry = RegistryEntry {
            key: "Software\\Classes\\x".to_owned(),
            name: "",
            value: "y".to_owned(),
        };
        // SAFETY: пустой дескриптор в запись не доходит — Win32 отвечает ошибкой.
        let closed = unsafe { Root::from_raw(std::ptr::null_mut()) };
        assert!(write_value(closed, &entry).is_err());
        assert!(!register_in(closed, PROTOCOL, EXE));
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use hlsr_online::discord_rpc::rpc::register::{ProtocolClient, SystemProtocolClient};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn fake_xdg_settings(dir: &Path, exit: i32) -> (std::path::PathBuf, std::path::PathBuf) {
        let out = dir.join("arguments.txt");
        let script = dir.join("xdg-settings");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit {exit}\n",
                out.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (script, out)
    }

    #[test]
    fn asks_xdg_settings_for_the_protocol() {
        let dir = tempfile::tempdir().unwrap();
        let (script, out) = fake_xdg_settings(dir.path(), 0);
        let client = SystemProtocolClient::with_xdg_settings(script);
        assert!(client.set_as_default_protocol_client("discord-1531057982518071296"));
        assert_eq!(
            std::fs::read_to_string(out).unwrap(),
            "set\ndefault-url-scheme-handler\ndiscord-1531057982518071296\nhlsr.desktop\n"
        );
    }

    #[test]
    fn failure_exit_code_is_false() {
        let dir = tempfile::tempdir().unwrap();
        let (script, _) = fake_xdg_settings(dir.path(), 3);
        let client = SystemProtocolClient::with_xdg_settings(script);
        assert!(!client.set_as_default_protocol_client("discord-1"));
    }

    #[test]
    fn missing_xdg_settings_is_false() {
        let dir = tempfile::tempdir().unwrap();
        let client = SystemProtocolClient::with_xdg_settings(dir.path().join("none"));
        assert!(!client.set_as_default_protocol_client("discord-1"));
    }
}
