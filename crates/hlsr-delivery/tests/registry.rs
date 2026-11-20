//! Трейт `Registry`: вывод `reg query`, подделка для тестов, реестр ОС.
//!
//! 0.4.7 читает реестр через `reg.exe query <ключ> /v <имя>` и разбирает текст
//! регулярными выражениями. Трейт отдаёт значение, а [`reg_query_output`] строит тот самый
//! текст, на котором разбор 0.4.7 даёт тот же ответ.

use hlsr_delivery::ports::{
    FakeRegistry, Registry, RegistryHive, RegistryValue, SystemRegistry, reg_query_output,
};

#[test]
fn reg_query_output_is_the_text_of_reg_exe() {
    let sz = RegistryValue::Sz("russian".into());
    assert_eq!(
        reg_query_output(
            RegistryHive::CurrentUser,
            "Software\\Valve\\Steam",
            "Language",
            &sz
        ),
        "\r\nHKEY_CURRENT_USER\\Software\\Valve\\Steam\r\n    Language    REG_SZ    russian\r\n\r\n"
    );
    let expand = RegistryValue::ExpandSz("%SystemRoot%\\x".into());
    assert_eq!(
        reg_query_output(RegistryHive::LocalMachine, "SOFTWARE\\X", "Path", &expand),
        "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\X\r\n    Path    REG_EXPAND_SZ    %SystemRoot%\\x\r\n\r\n"
    );
    // REG_DWORD печатается как 0x и строчные шестнадцатеричные цифры, без нулей слева.
    for (value, text) in [(0, "0x0"), (0xa00, "0xa00"), (0xdead_beef, "0xdeadbeef")] {
        assert_eq!(RegistryValue::Dword(value).data_text(), text);
    }
    assert_eq!(RegistryValue::Dword(1).type_name(), "REG_DWORD");
    assert_eq!(expand.type_name(), "REG_EXPAND_SZ");
    assert_eq!(sz.type_name(), "REG_SZ");
}

#[test]
fn fake_registry_ignores_case_like_the_real_one_and_counts_calls() {
    let fake = FakeRegistry::default();
    fake.set(
        RegistryHive::CurrentUser,
        "Software\\Valve\\Steam",
        "Language",
        RegistryValue::Sz("german".into()),
    );
    assert_eq!(fake.calls(), 0);
    assert_eq!(
        fake.read_value(
            RegistryHive::CurrentUser,
            "software\\VALVE\\steam",
            "LANGUAGE"
        ),
        Some(RegistryValue::Sz("german".into()))
    );
    assert_eq!(
        fake.read_value(
            RegistryHive::LocalMachine,
            "Software\\Valve\\Steam",
            "Language"
        ),
        None,
        "другой куст"
    );
    assert_eq!(
        fake.read_value(RegistryHive::CurrentUser, "Software\\Valve\\Steam", "Other"),
        None
    );
    assert_eq!(fake.calls(), 3);
    // query: текст, как у reg.exe; нет значения — reg.exe завершается ошибкой (None).
    assert_eq!(
        fake.query(
            RegistryHive::CurrentUser,
            "Software\\Valve\\Steam",
            "Language"
        )
        .as_deref(),
        Some(
            "\r\nHKEY_CURRENT_USER\\Software\\Valve\\Steam\r\n    Language    REG_SZ    german\r\n\r\n"
        )
    );
    assert_eq!(
        fake.query(RegistryHive::CurrentUser, "Software\\Valve\\Steam", "Other"),
        None
    );
}

#[cfg(not(windows))]
#[test]
fn system_registry_outside_windows_has_no_values() {
    assert_eq!(
        SystemRegistry.read_value(
            RegistryHive::CurrentUser,
            "Software\\Valve\\Steam",
            "Language"
        ),
        None
    );
    assert_eq!(
        SystemRegistry.query(
            RegistryHive::LocalMachine,
            "SOFTWARE\\Microsoft",
            "Anything"
        ),
        None
    );
}

/// Живая проверка на Windows (и под Wine): значения, записанные `reg.exe` во временный
/// ключ HKCU, читаются так же, как их печатает `reg query`. Ключ удаляется в конце.
#[cfg(windows)]
mod live {
    use super::*;
    use std::process::Command;

    struct TempKey(String);

    impl TempKey {
        fn new() -> TempKey {
            let key = format!("Software\\HLSR-registry-test-{}", std::process::id());
            TempKey(key)
        }

        fn add(&self, name: &str, kind: &str, data: &str) {
            let status = Command::new("reg.exe")
                .args([
                    "add",
                    &format!("HKCU\\{}", self.0),
                    "/v",
                    name,
                    "/t",
                    kind,
                    "/d",
                    data,
                    "/f",
                ])
                .output()
                .expect("reg.exe запускается");
            assert!(status.status.success(), "reg add {name}: {status:?}");
        }

        fn query(&self, name: &str) -> String {
            let out = Command::new("reg.exe")
                .args(["query", &format!("HKCU\\{}", self.0), "/v", name])
                .output()
                .expect("reg.exe запускается");
            assert!(out.status.success(), "reg query {name}: {out:?}");
            String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n")
        }
    }

    impl Drop for TempKey {
        fn drop(&mut self) {
            let _ = Command::new("reg.exe")
                .args(["delete", &format!("HKCU\\{}", self.0), "/f"])
                .output();
        }
    }

    #[test]
    fn values_written_by_reg_exe_read_back_and_render_like_reg_query() {
        let key = TempKey::new();
        key.add("Text", "REG_SZ", "russian");
        key.add("Spaced", "REG_SZ", "two words");
        key.add("Path", "REG_EXPAND_SZ", "%SystemRoot%\\x");
        key.add("Number", "REG_DWORD", "2560");
        key.add("Zero", "REG_DWORD", "0");
        key.add("Big", "REG_DWORD", "4294967295");
        let cases = [
            ("Text", RegistryValue::Sz("russian".into())),
            ("Spaced", RegistryValue::Sz("two words".into())),
            ("Path", RegistryValue::ExpandSz("%SystemRoot%\\x".into())),
            ("Number", RegistryValue::Dword(2560)),
            ("Zero", RegistryValue::Dword(0)),
            ("Big", RegistryValue::Dword(u32::MAX)),
        ];
        for (name, expected) in cases {
            let got = SystemRegistry.read_value(RegistryHive::CurrentUser, &key.0, name);
            assert_eq!(got.as_ref(), Some(&expected), "{name}");
            let ours = reg_query_output(RegistryHive::CurrentUser, &key.0, name, &expected)
                .replace("\r\n", "\n");
            assert_eq!(ours, key.query(name), "вывод reg query: {name}");
        }
        // Имя значения не различает регистр, ключа и значения нет — None.
        assert_eq!(
            SystemRegistry.read_value(RegistryHive::CurrentUser, &key.0.to_uppercase(), "TEXT"),
            Some(RegistryValue::Sz("russian".into()))
        );
        assert_eq!(
            SystemRegistry.read_value(RegistryHive::CurrentUser, &key.0, "Missing"),
            None
        );
        assert_eq!(
            SystemRegistry.read_value(
                RegistryHive::CurrentUser,
                "Software\\HLSR-no-such-key-ever",
                "Text"
            ),
            None
        );
        // query даёт тот же текст, что reg.exe.
        assert_eq!(
            SystemRegistry
                .query(RegistryHive::CurrentUser, &key.0, "Text")
                .map(|text| text.replace("\r\n", "\n")),
            Some(key.query("Text"))
        );
    }
}
