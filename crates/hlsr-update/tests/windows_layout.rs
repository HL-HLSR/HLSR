//! Раскладка установки Windows против 0.4.7: фикстура `windows-layout.json` из
//! `tools/reference/update/windows-layout.js` (`layout.js`, `file-list.js`, `install.js:originOf`
//! и записи реестра и ярлыков, которые настоящий `install()` передаёт подменённым реестру и
//! ярлыкам). Самотесты 0.4.7: «identity: боевой HLSR получает ключи прежнего NSIS-установщика»,
//! «refusedDir: системные, относительные и опасные пути», «file-list: список сборки и его проверки»,
//! «originOf: метка страницы загрузки в имени установщика».

mod support;

use hlsr_core::json_formats::compact;
use hlsr_update::windows::file_list::{self, FileListError};
use hlsr_update::windows::layout::{self, App};
use hlsr_update::windows::registry::{
    self, Change, FakeRegistry, InstallEntry, RegValue, Registry, ValueChange,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("windows-layout.json"));

fn app_of(value: &Value) -> Option<App> {
    if value.is_null() {
        return None;
    }
    let field = |name: &str| value[name].as_str().unwrap_or("").to_owned();
    Some(App {
        app_id: field("appId"),
        product_name: field("productName"),
    })
}

fn env_of(value: &Value) -> HashMap<String, String> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, item)| (key.clone(), str_of(item).to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// Окружения скрипта `windows-layout.js` (`ENVS`): метка → переменные.
fn envs() -> HashMap<String, HashMap<String, String>> {
    items(&FIXTURE["envs"])
        .iter()
        .map(|item| (str_of(&item["label"]).to_owned(), env_of(&item["env"])))
        .collect()
}

#[test]
fn constants_match_node() {
    let constants = &FIXTURE["constants"];
    assert_eq!(
        layout::PRODUCTION_APP_ID,
        str_of(&constants["PRODUCTION"]["appId"])
    );
    assert_eq!(
        layout::PRODUCTION_PRODUCT_NAME,
        str_of(&constants["PRODUCTION"]["productName"])
    );
    assert_eq!(layout::ORIGIN_KEY, str_of(&constants["ORIGIN_KEY"]));
    assert_eq!(layout::SEED_FLAG, str_of(&constants["SEED_FLAG"]));
    assert_eq!(layout::DESKTOP_VALUE, str_of(&constants["DESKTOP_VALUE"]));
    assert_eq!(layout::LIBRARY_ROOT, str_of(&constants["LIBRARY_ROOT"]));
    assert_eq!(layout::EXE, str_of(&constants["EXE"]));
    assert_eq!(layout::PUBLISHER, str_of(&constants["PUBLISHER"]));
    assert_eq!(layout::OWNER_MARKER, str_of(&constants["OWNER_MARKER"]));
    assert_eq!(layout::UNINSTALL_FLAG, str_of(&constants["UNINSTALL_FLAG"]));
    assert_eq!(
        layout::RETURN_SETUP_FLAG,
        str_of(&constants["RETURN_SETUP_FLAG"])
    );
    assert_eq!(file_list::FILE, str_of(&constants["FILE"]));
    assert_eq!(file_list::RELATIVE, str_of(&constants["RELATIVE"]));
}

#[test]
fn guid_for_matches_node() {
    let cases = items(&FIXTURE["guids"]);
    assert!(cases.len() >= 10);
    let mut mismatches = Mismatches::default();
    for case in cases {
        let app_id = str_of(&case["appId"]);
        mismatches.check_eq(
            layout::guid_for(app_id),
            str_of(&case["guid"]).to_owned(),
            format!("guidFor({app_id:?})"),
        );
    }
    mismatches.finish("layout.guidFor");
}

/// Самотест «identity: боевой HLSR получает ключи прежнего NSIS-установщика».
#[test]
fn production_identity_keeps_the_nsis_keys() {
    assert_eq!(
        layout::guid_for(layout::PRODUCTION_APP_ID),
        layout::PRODUCTION_GUID
    );
    assert_eq!(
        layout::PRODUCTION_GUID,
        "3f824379-6b41-5725-b424-59cb87f2acae"
    );
    let id = layout::identity(None).unwrap();
    assert_eq!(id.guid, layout::PRODUCTION_GUID);
    assert_eq!(
        id.install_key,
        "HKCU\\Software\\3f824379-6b41-5725-b424-59cb87f2acae"
    );
    assert_eq!(
        id.uninstall_key,
        "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\3f824379-6b41-5725-b424-59cb87f2acae"
    );
    assert_eq!(id.default_install_dir, "C:\\HLSR\\Launcher");
    assert_eq!(id.shortcut_name, "HLSR");
    // Маркер владения, который записал NSIS 0.3.16 (байт в байт).
    assert_eq!(
        id.owner_marker("{E1666AAA-AC03-4A85-B610-167B9EC8A9DA}"),
        "{\"schemaVersion\":1,\"appId\":\"3f824379-6b41-5725-b424-59cb87f2acae\",\"installId\":\"{E1666AAA-AC03-4A85-B610-167B9EC8A9DA}\"}"
    );
    let qa = layout::identity(Some(&App::qa())).unwrap();
    assert_eq!(qa.guid, "a4c160bc-6860-53ec-9475-5b670bd2800e");
    assert_eq!(qa.default_install_dir, "C:\\HLSR-QA\\Launcher");
    assert_eq!(qa.shortcut_name, "HLSR-QA");
    assert_eq!(
        layout::identity(Some(&App {
            app_id: "x".into(),
            product_name: "a\\b".into()
        }))
        .unwrap_err()
        .to_string(),
        "Недопустимое имя продукта: a\\b"
    );
}

/// Имя приложения (`app.getName()` 0.4.7): боевой — `name` из `package.json` (`hlsr`), QA-сборка —
/// `extraMetadata.name=hlsr-qa` (`qa-launcher-build.js`); от него папки данных и журналов.
#[test]
fn app_name_of_production_and_qa() {
    assert_eq!(App::production().name(), "hlsr");
    assert_eq!(layout::PRODUCTION_NAME, hlsr_core::paths::APP_NAME);
    assert_eq!(App::qa().name(), "hlsr-qa");
    assert_eq!(layout::QA_NAME, "hlsr-qa");
    // Имя продукта не решает: QA-сборку отличает appId.
    let other = App {
        app_id: layout::PRODUCTION_APP_ID.to_owned(),
        product_name: layout::QA_PRODUCT_NAME.to_owned(),
    };
    assert_eq!(other.name(), "hlsr");
    let empty = App {
        app_id: String::new(),
        product_name: String::new(),
    };
    assert_eq!(empty.name(), "hlsr");
}

#[test]
fn identity_matches_node() {
    let cases = items(&FIXTURE["identities"]);
    let marker_id = "{E1666AAA-AC03-4A85-B610-167B9EC8A9DA}";
    let mut mismatches = Mismatches::default();
    for case in cases {
        let app = app_of(&case["app"]);
        let label = str_of(&case["label"]);
        let actual = match layout::identity(app.as_ref()) {
            Ok(id) => json!({"identity": {
                "appId": id.app_id,
                "productName": id.product_name,
                "guid": id.guid,
                "installKey": id.install_key,
                "uninstallKey": id.uninstall_key,
                "defaultInstallDir": id.default_install_dir,
                "shortcutName": id.shortcut_name,
                "ownerMarker": id.owner_marker(marker_id),
            }}),
            Err(error) => json!({"error": error.to_string()}),
        };
        let expected = match case.get("identity") {
            Some(identity) => json!({"identity": identity}),
            None => json!({"error": case["error"]}),
        };
        mismatches.check(&actual, &expected, format!("identity: {label}"));
    }
    mismatches.finish("layout.identity");
}

/// Самотест «refusedDir: системные, относительные и опасные пути».
#[test]
fn refused_dir_matches_node() {
    let environments = envs();
    let cases = items(&FIXTURE["refused"]);
    assert!(cases.len() >= 400, "в фикстуре {} случаев", cases.len());
    let mut mismatches = Mismatches::default();
    for case in cases {
        let label = str_of(&case["env"]);
        let env = &environments[label];
        let lookup = |name: &str| env.get(name).cloned();
        let dir = str_of(&case["dir"]);
        mismatches.check_eq(
            layout::refused_dir(dir, &lookup),
            case["result"].as_str().map(str::to_owned),
            format!("refusedDir({dir:?}, {label})"),
        );
    }
    for case in items(&FIXTURE["refusedNoEnv"]) {
        let dir = str_of(&case["dir"]);
        mismatches.check_eq(
            layout::refused_dir(dir, &|_: &str| None),
            case["result"].as_str().map(str::to_owned),
            format!("refusedDir({dir:?}) без окружения"),
        );
    }
    mismatches.finish("layout.refusedDir");
}

#[test]
fn protected_dirs_match_node() {
    let environments = envs();
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["protectedDirs"]) {
        let label = str_of(&case["env"]);
        let env = &environments[label];
        let expected: Vec<String> = items(&case["list"])
            .iter()
            .map(|item| str_of(item).to_owned())
            .collect();
        mismatches.check_eq(
            layout::protected_dirs(&|name: &str| env.get(name).cloned()),
            expected,
            format!("protectedDirs({label})"),
        );
    }
    mismatches.finish("layout.protectedDirs");
}

#[test]
fn same_dir_and_inside_match_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["dirPairs"]) {
        let (a, b) = (str_of(&case["a"]), str_of(&case["b"]));
        mismatches.check_eq(
            layout::is_same_dir(a, b),
            case["same"].as_bool().unwrap(),
            format!("isSameDir({a:?}, {b:?})"),
        );
        mismatches.check_eq(
            layout::is_inside(a, b),
            case["inside"].as_bool().unwrap(),
            format!("isInside({a:?}, {b:?})"),
        );
    }
    mismatches.finish("layout.isSameDir/isInside");
}

#[test]
fn install_id_and_version_shapes_match_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["installIds"]) {
        let value = str_of(&case["value"]);
        mismatches.check_eq(
            layout::is_install_id(value),
            case["ok"].as_bool().unwrap(),
            format!("INSTALL_ID_RE {value:?}"),
        );
    }
    for case in items(&FIXTURE["versions"]) {
        let value = str_of(&case["value"]);
        mismatches.check_eq(
            layout::is_version(value),
            case["ok"].as_bool().unwrap(),
            format!("VERSION_RE {value:?}"),
        );
    }
    mismatches.finish("layout.INSTALL_ID_RE/VERSION_RE");
}

/// Самотест «file-list: список сборки и его проверки».
#[test]
fn file_list_problems_match_node() {
    let cases = items(&FIXTURE["problems"]);
    assert!(cases.len() >= 50);
    let mut mismatches = Mismatches::default();
    for case in cases {
        let actual: Vec<String> = file_list::problems_of(&case["data"]);
        let expected: Vec<String> = items(&case["problems"])
            .iter()
            .map(|item| str_of(item).to_owned())
            .collect();
        mismatches.check_eq(
            actual,
            expected,
            format!("problemsOf: {}", str_of(&case["label"])),
        );
    }
    mismatches.finish("file-list.problemsOf");
}

fn write_tree(root: &Path, tree: &Value) {
    for file in items(&tree["files"]) {
        let target = root.join(str_of(&file["path"]));
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        let hex = str_of(&file["hex"]);
        let bytes: Vec<u8> = (0..hex.len() / 2)
            .map(|at| u8::from_str_radix(&hex[at * 2..at * 2 + 2], 16).unwrap())
            .collect();
        std::fs::write(target, bytes).unwrap();
    }
    for dir in items(&tree["emptyDirs"]) {
        std::fs::create_dir_all(root.join(str_of(dir))).unwrap();
    }
}

#[test]
fn file_list_build_matches_node() {
    let cases = items(&FIXTURE["builds"]);
    assert!(cases.len() >= 4);
    for case in cases {
        let tree = &case["tree"];
        let dir = tempfile::tempdir().unwrap();
        write_tree(dir.path(), tree);
        let app = app_of(&tree["app"]).unwrap();
        let list = file_list::build(dir.path(), str_of(&tree["version"]), &app).unwrap();
        let label = str_of(&tree["label"]);
        support::assert_json_eq(&list.to_value(), &case["list"], label);
        assert_eq!(
            compact(&list.to_value()),
            str_of(&case["text"]),
            "{label}: JSON.stringify(list)"
        );
        assert_eq!(
            list.to_text(),
            format!("{}\n", str_of(&case["text"])),
            "{label}"
        );
        assert_eq!(
            list.total_bytes(),
            items(&case["list"]["files"])
                .iter()
                .map(|file| file["size"].as_u64().unwrap())
                .sum::<u64>(),
            "{label}: сумма размеров"
        );
    }
}

/// Установка кладёт список в каталог программы (`wx`): его читает удаление, существующий файл не
/// затирается.
#[test]
fn file_list_is_written_once_and_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::create_dir_all(source.join("resources")).unwrap();
    std::fs::write(source.join("HLSR.exe"), "exe").unwrap();
    std::fs::write(source.join("resources").join("a.bin"), [0u8, 255, 10, 13]).unwrap();
    let list = file_list::build(&source, "0.5.0", &App::qa()).unwrap();

    let target = dir.path().join("target");
    std::fs::create_dir_all(target.join("resources")).unwrap();
    file_list::write_new(&target, &list).unwrap();
    let text = std::fs::read_to_string(target.join(file_list::RELATIVE)).unwrap();
    assert_eq!(text, format!("{}\n", compact(&list.to_value())));
    assert!(text.ends_with("}\n"));
    assert_eq!(file_list::read(&target).unwrap(), list);
    let error = file_list::write_new(&target, &list).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
}

#[test]
fn file_list_read_matches_node() {
    let cases = items(&FIXTURE["reads"]);
    assert!(cases.len() >= 10);
    for case in cases {
        let label = str_of(&case["label"]);
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("resources")).unwrap();
        if let Some(text) = case["text"].as_str() {
            std::fs::write(dir.path().join(file_list::RELATIVE), text).unwrap();
        }
        let result = file_list::read(dir.path());
        if case["ok"].as_bool().unwrap() {
            let list = result.unwrap_or_else(|error| panic!("{label}: {error}"));
            // read() отдаёт то, что разобрано; установка пишет JSON.stringify(list) + "\n".
            assert_eq!(
                list.to_text(),
                format!("{}\n", compact(&case["data"])),
                "{label}"
            );
            assert_eq!(list.version, str_of(&case["data"]["version"]), "{label}");
            assert_eq!(
                list.files.len(),
                items(&case["data"]["files"]).len(),
                "{label}"
            );
        } else {
            let error = result.expect_err(label);
            match str_of(&case["kind"]) {
                "unreadable" => {
                    assert!(
                        matches!(error, FileListError::Unreadable(_)),
                        "{label}: {error}"
                    );
                    assert!(
                        error
                            .to_string()
                            .starts_with("Список файлов установщика не читается:"),
                        "{label}: {error}"
                    );
                }
                _ => assert_eq!(error.to_string(), str_of(&case["message"]), "{label}"),
            }
        }
    }
}

/// Ссылка в сборке — ошибка, а не пропуск (`walk` бросает `Ссылка в сборке: …`). На Windows
/// ссылкой служит junction (`mklink /J` не требует прав), на Linux — символическая ссылка.
#[cfg(any(windows, unix))]
#[test]
fn file_list_build_rejects_a_link() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("build");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(root.join("resources")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(root.join("HLSR.exe"), "exe").unwrap();
    let link = root.join("resources").join("link");
    #[cfg(windows)]
    {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "mklink /J");
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let error = file_list::build(&root, "1.2.3", &App::production()).unwrap_err();
    assert_eq!(error.to_string(), "Ссылка в сборке: resources/link");
    assert!(matches!(error, FileListError::Link(_)));
}

#[test]
fn origin_of_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["origins"]) {
        let setup = case["setupFile"].as_str();
        mismatches.check_eq(
            setup.and_then(registry::origin_of),
            case["origin"].as_str(),
            format!("originOf({setup:?})"),
        );
    }
    mismatches.finish("install.originOf");
}

// ------------------------------------------------------------------- записи install()

/// `{ключ: {имя: строка | число}}` из фикстуры — реестр-подделка: строка — `REG_SZ`, число — `REG_DWORD`.
fn registry_of(model: &Value, replace: &dyn Fn(&str) -> String) -> FakeRegistry {
    let mut registry = FakeRegistry::new();
    for (key, values) in model.as_object().unwrap() {
        registry = registry.with_key(key);
        for (name, value) in values.as_object().unwrap() {
            let value = match value {
                Value::String(text) => RegValue::Sz(replace(text)),
                other => RegValue::Dword(other.as_u64().unwrap() as u32),
            };
            registry = registry.with_value(key, name, value);
        }
    }
    registry
}

/// То же, что `registry_of`, наоборот: только разделы со значениями, ключи в записи `HKCU\…`.
fn model_of(registry: &FakeRegistry) -> Value {
    let mut out = Map::new();
    for (key, values) in registry.to_json().as_object().unwrap() {
        let short = key.replacen("HKEY_CURRENT_USER", "HKCU", 1);
        let mut map = Map::new();
        for (name, value) in values.as_object().unwrap() {
            let data = str_of(&value["data"]);
            map.insert(
                name.clone(),
                if str_of(&value["type"]) == "REG_DWORD" {
                    json!(u64::from_str_radix(data.trim_start_matches("0x"), 16).unwrap())
                } else {
                    json!(data)
                },
            );
        }
        out.insert(short, Value::Object(map));
    }
    Value::Object(out)
}

fn changes_json(changes: &[Change]) -> Value {
    Value::Array(
        changes
            .iter()
            .map(|change| match change {
                Change::Remove { key } => json!({"key": key, "remove": true}),
                Change::Set { key, values } => {
                    let mut map = Map::new();
                    for (name, value) in values {
                        map.insert(
                            name.clone(),
                            match value {
                                ValueChange::Sz(text) => json!(text),
                                ValueChange::Dword(number) => json!({"dword": number}),
                                ValueChange::Delete => Value::Null,
                            },
                        );
                    }
                    json!({"key": key, "values": map})
                }
            })
            .collect(),
    )
}

/// Подстановка меток фикстуры в дерево значений.
fn replaced(value: &Value, replace: &dyn Fn(&str) -> String) -> Value {
    match value {
        Value::String(text) => json!(replace(text)),
        Value::Array(list) => {
            Value::Array(list.iter().map(|item| replaced(item, replace)).collect())
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (replace(key), replaced(item, replace)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Записи реестра и ярлыки, которые настоящий `install()` 0.4.7 передал подменённым реестру и
/// ярлыкам: изменения (`registry.apply`), состояние после них и какие ярлыки записаны.
#[test]
fn install_records_match_node() {
    const FRESH_ID: &str = "{E1666AAA-AC03-4A85-B610-167B9EC8A9DA}";
    const SEEDED_ID: &str = "{0A1B2C3D-0000-4000-8000-ABCDEFABCDEF}";
    const INSTALL_DIR: &str = "C:\\Fixture\\Users\\Игрок\\HLSR\\Launcher";
    let cases = items(&FIXTURE["installs"]);
    assert!(cases.len() >= 12);
    let mut mismatches = Mismatches::default();
    for case in cases {
        let label = str_of(&case["label"]);
        let app = app_of(&case["app"]).unwrap();
        let id = layout::identity(Some(&app)).unwrap();
        let reused = case["installIdReused"].as_bool().unwrap();
        let replace_id = if reused { SEEDED_ID } else { FRESH_ID };
        let replace = |text: &str| {
            text.replace("{install}", INSTALL_DIR)
                .replace("{ID}", replace_id)
        };
        let update = case["update"].as_bool().unwrap();

        // Состояние реестра до установки и решения install(): id и выбор ярлыка на столе.
        let registry = registry_of(&case["registryBefore"], &replace);
        let before = |name: &str| registry.read_value(&id.install_key, name);
        let existing_id = before("HLSRInstallId");
        let install_id = match existing_id.as_ref().and_then(RegValue::as_sz) {
            Some(text) if layout::is_install_id(text) => text.to_owned(),
            _ => FRESH_ID.to_owned(),
        };
        let desktop_value = before(layout::DESKTOP_VALUE);
        let create_desktop = hlsr_update::windows::shortcuts::wants_desktop_shortcut(
            desktop_value.as_ref().and_then(RegValue::as_sz),
            update,
            case["desktopShortcut"].as_bool(),
        );
        let setup_file = case["setupFile"].as_str();
        let total = case["totalBytes"].as_u64().unwrap();
        let entry = InstallEntry {
            install_dir: INSTALL_DIR,
            install_id: &install_id,
            desktop_shortcut: create_desktop,
            version: str_of(&case["version"]),
            size_bytes: total,
            origin: setup_file.and_then(registry::origin_of),
        };

        let changes = registry::install_changes(&id, &entry);
        let replace_final = |text: &str| {
            text.replace("{install}", INSTALL_DIR)
                .replace("{ID}", &install_id)
        };
        mismatches.check(
            &json!([changes_json(&changes)]),
            &replaced(&case["changes"], &replace_final),
            format!("{label}: registry.apply"),
        );

        registry::apply(&registry, &changes).unwrap();
        mismatches.check(
            &model_of(&registry),
            &replaced(&case["registryAfter"], &replace_final),
            format!("{label}: реестр после установки"),
        );
    }
    mismatches.finish("install: записи реестра");
}

#[test]
fn estimated_size_rounds_up_to_kibibytes() {
    let id = layout::identity(None).unwrap();
    let size = |bytes: u64| {
        let entry = InstallEntry {
            install_dir: "C:\\X\\HLSR",
            install_id: "{E1666AAA-AC03-4A85-B610-167B9EC8A9DA}",
            desktop_shortcut: true,
            version: "1.0.0",
            size_bytes: bytes,
            origin: None,
        };
        match registry::uninstall_key_change(&id, &entry) {
            Change::Set { values, .. } => values
                .into_iter()
                .find(|(name, _)| name == "EstimatedSize")
                .map(|(_, value)| value),
            Change::Remove { .. } => None,
        }
    };
    assert_eq!(size(0), Some(ValueChange::Dword(0)));
    assert_eq!(size(1), Some(ValueChange::Dword(1)));
    assert_eq!(size(1024), Some(ValueChange::Dword(1)));
    assert_eq!(size(1025), Some(ValueChange::Dword(2)));
    // Math.ceil(x / 1024) >>> 0: больше 4 ТиБ значение оборачивается.
    assert_eq!(size((1u64 << 32) * 1024 + 1), Some(ValueChange::Dword(1)));
}
