//! Реестр установки: подделка (`FakeRegistry`), изменения (`apply`) и ключи раздела 5.2 карты;
//! настоящий реестр (`SystemRegistry`) — только чтение системных разделов и защитные отказы здесь,
//! запись во временный раздел `HKCU\Software\HLSR-Test-<16hex>` — при `HLSR_CI_REGISTRY=1`
//! (Windows CI, раннер одноразовый). Самотест 0.4.7 «registry: .reg-файл и разбор экспорта,
//! кириллица и кавычки» в части значений.

mod support;

use hlsr_update::windows::layout::{self, App};
use hlsr_update::windows::registry::{
    self, Change, FakeRegistry, InstallEntry, RegOp, RegValue, Registry, ValueChange,
};
use serde_json::json;

fn sz(text: &str) -> RegValue {
    RegValue::Sz(text.to_owned())
}

#[test]
fn keys_accept_node_and_reg_exe_spellings() {
    use registry::{Hive, split_key};
    assert_eq!(
        split_key("HKCU\\Software\\X").unwrap(),
        (Hive::CurrentUser, "Software\\X")
    );
    assert_eq!(
        split_key("hkey_current_user\\Software\\X\\").unwrap(),
        (Hive::CurrentUser, "Software\\X")
    );
    assert_eq!(
        split_key("HKLM\\SOFTWARE").unwrap(),
        (Hive::LocalMachine, "SOFTWARE")
    );
    assert_eq!(split_key("HKCU").unwrap(), (Hive::CurrentUser, ""));
    assert_eq!(
        split_key("HKCR\\X").unwrap_err().to_string(),
        "Неподдерживаемый раздел реестра: HKCR\\X"
    );
    assert_eq!(
        registry::full_key("HKCU\\Software\\X").unwrap(),
        "HKEY_CURRENT_USER\\Software\\X"
    );
}

/// Кириллица и кавычки в `REG_SZ` доходят без искажений (через `.reg` их приходилось экранировать).
#[test]
fn values_keep_cyrillic_quotes_and_backslashes() {
    let registry = FakeRegistry::new();
    let key = "HKCU\\Software\\Тест";
    registry
        .write(key, "InstallLocation", &sz("C:\\Игры\\HLSR"))
        .unwrap();
    registry.write(key, "Quote", &sz("a \"b\"")).unwrap();
    registry
        .write(key, "Size", &RegValue::Dword(0x5f5ff))
        .unwrap();
    registry
        .write(key, "Path", &RegValue::ExpandSz("%APPDATA%\\x".into()))
        .unwrap();
    let values = registry.read(key).unwrap();
    assert_eq!(
        values,
        vec![
            ("InstallLocation".to_owned(), sz("C:\\Игры\\HLSR")),
            ("Quote".to_owned(), sz("a \"b\"")),
            ("Size".to_owned(), RegValue::Dword(0x5f5ff)),
            ("Path".to_owned(), RegValue::ExpandSz("%APPDATA%\\x".into())),
        ]
    );
    // parseRegFile читал только REG_SZ.
    assert_eq!(
        registry.read_value(key, "quote").unwrap().as_sz(),
        Some("a \"b\"")
    );
    assert_eq!(registry.read_value(key, "Size").unwrap().as_sz(), None);
    assert_eq!(RegValue::Dword(0x5f5ff).data_text(), "0x5f5ff");
}

#[test]
fn a_missing_key_is_none_and_an_empty_key_is_empty() {
    let registry = FakeRegistry::new().with_key("HKCU\\Software\\Empty");
    assert!(registry.read("HKCU\\Software\\Nope").is_none());
    assert_eq!(registry.read("HKCU\\Software\\Empty"), Some(vec![]));
    // Родители создаются вместе с дочерним.
    assert_eq!(registry.read("HKCU\\Software"), Some(vec![]));
    assert_eq!(
        registry.subkeys("HKCU\\Software"),
        Some(vec!["Empty".to_owned()])
    );
    assert_eq!(registry.subkeys("HKCU\\Software\\Nope"), None);
}

#[test]
fn names_and_keys_ignore_case_and_keep_the_first_spelling() {
    let registry = FakeRegistry::new();
    registry
        .write("HKCU\\Software\\Abc", "Name", &sz("1"))
        .unwrap();
    registry
        .write("hkcu\\software\\ABC", "NAME", &sz("2"))
        .unwrap();
    assert_eq!(
        registry.read("HKCU\\Software\\abc").unwrap(),
        vec![("Name".to_owned(), sz("2"))]
    );
    assert_eq!(
        registry.subkeys("HKCU\\Software"),
        Some(vec!["Abc".to_owned()])
    );
}

#[test]
fn subkeys_come_in_name_order_and_only_direct_children() {
    let registry = FakeRegistry::new();
    for key in ["B", "a", "C\\deep\\er", "C\\x"] {
        registry
            .write(&format!("HKCU\\Software\\R\\{key}"), "v", &sz("1"))
            .unwrap();
    }
    assert_eq!(
        registry.subkeys("HKCU\\Software\\R"),
        Some(vec!["a".to_owned(), "B".to_owned(), "C".to_owned()])
    );
    assert_eq!(
        registry.subkeys("HKCU\\Software\\R\\C"),
        Some(vec!["deep".to_owned(), "x".to_owned()])
    );
}

#[test]
fn deleting_values_and_trees_is_quiet_when_nothing_is_there() {
    let registry = FakeRegistry::new();
    registry.delete_value("HKCU\\Software\\Nope", "V").unwrap();
    registry.delete_tree("HKCU\\Software\\Nope").unwrap();
    registry
        .write("HKCU\\Software\\X\\Y", "A", &sz("1"))
        .unwrap();
    registry
        .write("HKCU\\Software\\X\\Y", "B", &sz("2"))
        .unwrap();
    registry.write("HKCU\\Software\\X", "C", &sz("3")).unwrap();
    registry.delete_value("HKCU\\Software\\X\\Y", "a").unwrap();
    assert_eq!(
        registry.read("HKCU\\Software\\X\\Y").unwrap(),
        vec![("B".to_owned(), sz("2"))]
    );
    registry.delete_tree("HKCU\\Software\\X").unwrap();
    assert!(registry.read("HKCU\\Software\\X").is_none());
    assert!(registry.read("HKCU\\Software\\X\\Y").is_none());
    // Раздел-сосед с общим началом имени остаётся.
    registry.write("HKCU\\Software\\X2", "A", &sz("1")).unwrap();
    registry.delete_tree("HKCU\\Software\\X").unwrap();
    assert!(registry.read("HKCU\\Software\\X2").is_some());
}

/// Куст и первый уровень целиком не удаляются, HKLM не пишется: защита от опечатки в ключе.
#[test]
fn dangerous_writes_and_deletes_are_refused() {
    let registry = FakeRegistry::new();
    assert!(registry.delete_tree("HKCU").is_err());
    assert!(registry.delete_tree("HKCU\\Software").is_err());
    assert!(registry.delete_tree("HKLM\\Software\\X").is_err());
    assert!(registry.write("HKLM\\Software\\X", "V", &sz("1")).is_err());
    assert!(registry.write("HKCU", "V", &sz("1")).is_err());
    assert!(registry.delete_value("HKLM\\Software\\X", "V").is_err());
    assert!(
        registry.ops().is_empty(),
        "отказы не попадают в журнал операций"
    );
}

#[test]
fn failing_writes_leave_the_registry_untouched() {
    let registry = FakeRegistry::new().with_value("HKCU\\Software\\X", "V", sz("old"));
    registry.set_fail_writes(true);
    assert!(
        registry
            .write("HKCU\\Software\\X", "V", &sz("new"))
            .is_err()
    );
    assert!(registry.delete_value("HKCU\\Software\\X", "V").is_err());
    assert!(registry.delete_tree("HKCU\\Software\\X").is_err());
    assert_eq!(
        registry.read_value("HKCU\\Software\\X", "V"),
        Some(sz("old"))
    );
}

#[test]
fn json_model_round_trips() {
    let model = json!({
        "HKEY_CURRENT_USER\\Software\\A": {"S": {"type": "REG_SZ", "data": "значение"}, "D": {"type": "REG_DWORD", "data": "0x1"}},
        "HKEY_LOCAL_MACHINE\\Software\\B": {"E": {"type": "REG_EXPAND_SZ", "data": "%X%"}},
    });
    let registry = FakeRegistry::from_json(&model).unwrap();
    assert_eq!(registry.to_json(), model);
    assert_eq!(
        registry.read_value("HKLM\\Software\\B", "E"),
        Some(RegValue::ExpandSz("%X%".into()))
    );
    assert!(FakeRegistry::from_json(&json!({"HKCR\\X": {}})).is_err());
    assert!(
        FakeRegistry::from_json(&json!({"HKCU\\X": {"V": {"type": "REG_BINARY", "data": ""}}}))
            .is_err()
    );
}

fn entry<'a>(dir: &'a str, id: &'a str) -> InstallEntry<'a> {
    InstallEntry {
        install_dir: dir,
        install_id: id,
        desktop_shortcut: true,
        version: "0.5.0",
        size_bytes: 4096,
        origin: Some("ru"),
    }
}

const ID: &str = "{E1666AAA-AC03-4A85-B610-167B9EC8A9DA}";

#[test]
fn apply_runs_in_order_and_checks_every_key_first() {
    let registry = FakeRegistry::new();
    let bad = [
        Change::Set {
            key: "HKCU\\Software\\X".into(),
            values: vec![("A".into(), ValueChange::Sz("1".into()))],
        },
        Change::Set {
            key: "HKLM\\Software\\Y".into(),
            values: vec![("B".into(), ValueChange::Sz("2".into()))],
        },
    ];
    assert_eq!(
        registry::apply(&registry, &bad).unwrap_err().to_string(),
        "Неподдерживаемый раздел реестра: HKLM\\Software\\Y"
    );
    assert!(registry.ops().is_empty(), "ни одно изменение не применено");

    let good = [
        Change::Set {
            key: "HKCU\\Software\\X".into(),
            values: vec![
                ("A".into(), ValueChange::Sz("1".into())),
                ("N".into(), ValueChange::Dword(7)),
                ("Gone".into(), ValueChange::Delete),
            ],
        },
        Change::Remove {
            key: "HKCU\\Software\\Old\\Sub".into(),
        },
    ];
    registry::apply(&registry, &good).unwrap();
    let key = |name: &str| format!("HKEY_CURRENT_USER\\{name}");
    assert_eq!(
        registry.ops(),
        vec![
            RegOp::Write {
                key: key("Software\\X"),
                name: "A".into(),
                value: sz("1")
            },
            RegOp::Write {
                key: key("Software\\X"),
                name: "N".into(),
                value: RegValue::Dword(7)
            },
            RegOp::DeleteValue {
                key: key("Software\\X"),
                name: "Gone".into()
            },
            RegOp::DeleteTree {
                key: key("Software\\Old\\Sub")
            },
        ]
    );
}

/// Раздел 5.2: записи установки и удаления; `Software\HLSR` (метка страницы загрузки) остаётся.
#[test]
fn install_keys_are_written_and_deleted_as_in_the_map() {
    let id = layout::identity(None).unwrap();
    let registry =
        FakeRegistry::new().with_value(id.install_key.as_str(), layout::SEED_FLAG, sz("C:\\seed"));
    let dir = "C:\\HLSR\\Launcher";
    let entry = entry(dir, ID);
    registry::write_install_keys(&registry, &id, &entry).unwrap();
    registry::write_uninstall_keys(&registry, &id, &entry).unwrap();
    registry::apply(&registry, &[registry::origin_change("ru")]).unwrap();

    let install = |name: &str| registry.read_value(&id.install_key, name);
    assert_eq!(install("InstallLocation"), Some(sz(dir)));
    assert_eq!(install("KeepShortcuts"), Some(sz("true")));
    assert_eq!(install("ShortcutName"), Some(sz("HLSR")));
    assert_eq!(install("HLSRInstallId"), Some(sz(ID)));
    assert_eq!(install("HLSRDesktopShortcut"), Some(sz("1")));
    assert_eq!(
        install(layout::SEED_FLAG),
        None,
        "метка незаконченной установки NSIS удалена"
    );

    let uninstall = |name: &str| registry.read_value(&id.uninstall_key, name);
    assert_eq!(uninstall("DisplayName"), Some(sz("HLSR 0.5.0")));
    assert_eq!(uninstall("DisplayVersion"), Some(sz("0.5.0")));
    assert_eq!(
        uninstall("DisplayIcon"),
        Some(sz("C:\\HLSR\\Launcher\\HLSR.exe,0"))
    );
    assert_eq!(uninstall("Publisher"), Some(sz("HLSR Team")));
    assert_eq!(
        uninstall("UninstallString"),
        Some(sz("\"C:\\HLSR\\Launcher\\HLSR.exe\" --hlsr-uninstall"))
    );
    assert_eq!(
        uninstall("QuietUninstallString"),
        Some(sz("\"C:\\HLSR\\Launcher\\HLSR.exe\" --hlsr-uninstall /S"))
    );
    assert_eq!(uninstall("NoModify"), Some(RegValue::Dword(1)));
    assert_eq!(uninstall("NoRepair"), Some(RegValue::Dword(1)));
    assert_eq!(uninstall("EstimatedSize"), Some(RegValue::Dword(4)));
    assert_eq!(
        registry.read_value(layout::ORIGIN_KEY, "InstallerOrigin"),
        Some(sz("ru"))
    );

    registry::delete_keys(&registry, &id).unwrap();
    assert!(registry.read(&id.install_key).is_none());
    assert!(registry.read(&id.uninstall_key).is_none());
    assert!(
        registry.read(layout::ORIGIN_KEY).is_some(),
        "Software\\HLSR не трогаем"
    );
    assert_eq!(
        registry
            .ops()
            .iter()
            .rev()
            .take(2)
            .cloned()
            .collect::<Vec<_>>(),
        vec![
            RegOp::DeleteTree {
                key: format!("HKEY_CURRENT_USER\\Software\\{}", id.guid)
            },
            RegOp::DeleteTree {
                key: format!(
                    "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{}",
                    id.guid
                )
            },
        ],
        "сначала запись удаления, потом раздел установки"
    );
}

#[test]
fn qa_keys_are_separate_from_production() {
    let production = layout::identity(None).unwrap();
    let qa = layout::identity(Some(&App::qa())).unwrap();
    let registry = FakeRegistry::new();
    registry::apply(
        &registry,
        &registry::install_changes(&production, &entry("C:\\HLSR\\Launcher", ID)),
    )
    .unwrap();
    registry::apply(
        &registry,
        &registry::install_changes(&qa, &entry("C:\\HLSR-QA\\Launcher", ID)),
    )
    .unwrap();
    registry::delete_keys(&registry, &qa).unwrap();
    assert_eq!(
        registry.read_value(&production.install_key, "InstallLocation"),
        Some(sz("C:\\HLSR\\Launcher")),
        "удаление QA не трогает боевую запись"
    );
    assert!(registry.read(&qa.install_key).is_none());
    assert_eq!(registry.read_value(&qa.uninstall_key, "DisplayName"), None);
}

// ------------------------------------------------------------------- настоящий реестр

#[cfg(windows)]
mod system {
    use super::*;
    use hlsr_update::windows::registry::SystemRegistry;

    /// Чтение системного раздела HKLM (не профиля и не записей HLSR владельца) проходит через
    /// `RegEnumValueW`: `ProductName` лежит в `REG_SZ`.
    #[test]
    fn reads_a_system_key() {
        let registry = SystemRegistry;
        let key = "HKLM\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
        let values = registry.read(key).expect("раздел Windows читается");
        assert!(values.iter().any(|(name, _)| name == "ProductName"));
        assert!(
            registry
                .read_value(key, "productname")
                .and_then(|value| value.as_sz().map(str::to_owned))
                .is_some_and(|name| name.starts_with("Windows"))
        );
        let subkeys = registry
            .subkeys("HKLM\\SOFTWARE\\Microsoft\\Windows NT")
            .unwrap();
        assert!(subkeys.iter().any(|name| name == "CurrentVersion"));
        assert!(
            registry
                .read("HKLM\\SOFTWARE\\HLSR-Test-Absent-6f1c")
                .is_none()
        );
        assert!(
            registry
                .subkeys("HKLM\\SOFTWARE\\HLSR-Test-Absent-6f1c")
                .is_none()
        );
    }

    /// Защитные отказы срабатывают до обращения к реестру.
    #[test]
    fn refuses_dangerous_operations_without_touching_the_registry() {
        let registry = SystemRegistry;
        assert!(registry.delete_tree("HKCU").is_err());
        assert!(registry.delete_tree("HKCU\\Software").is_err());
        assert!(registry.delete_tree("HKLM\\SOFTWARE\\X\\Y").is_err());
        assert!(
            registry
                .write("HKLM\\SOFTWARE\\HLSR-Test-Never", "V", &sz("1"))
                .is_err()
        );
        assert!(
            registry
                .delete_value("HKLM\\SOFTWARE\\HLSR-Test-Never", "V")
                .is_err()
        );
        assert!(registry.write("HKCU", "V", &sz("1")).is_err());
    }

    fn unique() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos() as u64);
        format!("{:016x}", nanos ^ (u64::from(std::process::id()) << 40))
    }

    /// Запись во временный раздел `HKCU\Software\HLSR-Test-<16hex>`: только при
    /// `HLSR_CI_REGISTRY=1` (раннер Windows CI одноразовый), на машине разработчика не запускается.
    #[test]
    fn writes_a_temporary_key_in_ci_only() {
        if std::env::var("HLSR_CI_REGISTRY").as_deref() != Ok("1") {
            eprintln!("пропуск: запись в реестр только при HLSR_CI_REGISTRY=1");
            return;
        }
        let registry = SystemRegistry;
        let root = format!("HKCU\\Software\\HLSR-Test-{}", unique());
        let key = format!("{root}\\Игры и \"кавычки\"");
        let result = std::panic::catch_unwind(|| {
            registry
                .write(
                    &key,
                    "InstallLocation",
                    &sz("C:\\Игры\\HLSR \"x\"\\Launcher"),
                )
                .unwrap();
            registry
                .write(&key, "Size", &RegValue::Dword(0xdead_beef))
                .unwrap();
            registry
                .write(&key, "Path", &RegValue::ExpandSz("%APPDATA%\\x".into()))
                .unwrap();
            registry.write(&key, "Gone", &sz("x")).unwrap();
            registry.delete_value(&key, "Gone").unwrap();
            let values = registry.read(&key).unwrap();
            assert_eq!(values.len(), 3);
            assert_eq!(
                registry.read_value(&key, "installlocation"),
                Some(sz("C:\\Игры\\HLSR \"x\"\\Launcher"))
            );
            assert_eq!(
                registry.read_value(&key, "Size"),
                Some(RegValue::Dword(0xdead_beef))
            );
            assert_eq!(
                registry.subkeys(&root),
                Some(vec!["Игры и \"кавычки\"".to_owned()])
            );
            registry.delete_tree(&root).unwrap();
            assert!(registry.read(&root).is_none());
            registry.delete_tree(&root).unwrap();
        });
        // Раздел убирается при любом исходе.
        let _ = registry.delete_tree(&root);
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}
