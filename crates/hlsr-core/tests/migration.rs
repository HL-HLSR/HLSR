//! Перенос localStorage 0.4.7 (LevelDB Chromium) в `launcher-prefs.json`.

use hlsr_core::launcher_prefs::{LauncherPrefs, PREFS_FILE, prefs_defaults};
use hlsr_core::migration::local_storage::{
    decode_value, migrate_local_storage, read_local_storage, read_local_storage_item,
};
use hlsr_core::migration::{MIGRATION_FILE, MigrationLog, STEP_LOCAL_STORAGE};
use hlsr_core::paths::Paths;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn fixture_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
}

/// Имена и содержимое всех файлов каталога (рекурсивно), по порядку имён.
fn dir_digest(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                out.push((path.strip_prefix(dir).unwrap().to_owned(), bytes));
            }
        }
    }
    out.sort();
    out
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn reads_values_written_by_electron() {
    let v = read_local_storage(&fixture_dir("leveldb-0.4.7")).unwrap();
    // Итоговое значение из журнала .log перекрывает прежнее из таблицы .ldb.
    assert_eq!(
        v.prefs.as_deref(),
        Some(
            r##"{"launcherTheme":"xen","speedLimit":5,"railExpanded":true,"accent":"#fff","toolSplits":{"hl1-won":"any%"},"liveSplitGame":"hl1-won"}"##
        )
    );
    assert_eq!(v.keyboard_run_mode.as_deref(), Some("scripted"));
    // Записан только первым запуском — лежит в таблице .ldb (snappy).
    assert_eq!(v.was_launched.as_deref(), Some("true"));
}

#[test]
fn reads_utf16_keys_and_skips_removed() {
    let dir = fixture_dir("leveldb-0.4.7");
    assert_eq!(
        read_local_storage_item(&dir, "проба-юникода")
            .unwrap()
            .as_deref(),
        Some("Привет 😀")
    );
    assert_eq!(
        read_local_storage_item(&dir, "удаляемый-ключ").unwrap(),
        None
    );
    assert_eq!(read_local_storage_item(&dir, "заполнитель").unwrap(), None);
    assert_eq!(read_local_storage_item(&dir, "нет-такого").unwrap(), None);
}

#[test]
fn decodes_both_value_encodings() {
    assert_eq!(decode_value(b"\x01abc").as_deref(), Some("abc"));
    assert_eq!(decode_value(b"\x01caf\xe9").as_deref(), Some("café"));
    assert_eq!(
        decode_value(&[0x00, 0x1F, 0x04, 0x40, 0x04]).as_deref(),
        Some("Пр")
    );
    assert_eq!(decode_value(&[0x00, 0x1F, 0x04, 0x40]), None);
    assert_eq!(decode_value(&[0x02, 0x41]), None);
    assert_eq!(decode_value(&[]), None);
    assert_eq!(decode_value(&[0x01]).as_deref(), Some(""));
}

#[test]
fn migration_writes_prefs_once_and_keeps_source() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::under(tmp.path());
    let leveldb = paths.data_dir.join("Local Storage").join("leveldb");
    copy_dir(&fixture_dir("leveldb-0.4.7"), &leveldb);
    let before = dir_digest(&paths.data_dir.join("Local Storage"));

    let r = migrate_local_storage(&paths, SystemTime::UNIX_EPOCH);
    assert!(!r.skipped && r.errors.is_empty(), "{:?}", r.errors);

    let prefs = LauncherPrefs::open(&paths);
    assert_eq!(prefs.theme_id(), "xen");
    assert_eq!(prefs.get("speedLimit").unwrap(), 5);
    assert!(prefs.get("accent").is_none());
    assert_eq!(prefs.get("keyboardRunMode").unwrap(), "scripted");
    assert_eq!(prefs.get("wasLaunched").unwrap(), true);
    assert_eq!(prefs.get("railExpanded").unwrap(), true);
    assert_eq!(
        prefs.get("toolSplits").unwrap(),
        &json!({"hl1-won": "any%"})
    );
    assert_eq!(prefs.get("animations").unwrap(), true);

    // Файл — как запись Store: ключи умолчаний по порядку, затем прочие.
    let written = read_json(&paths.file(PREFS_FILE));
    let keys: Vec<&str> = written
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let default_keys: Vec<String> = prefs_defaults()
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(keys, default_keys);

    assert_eq!(
        read_json(&paths.file(MIGRATION_FILE)),
        json!({ "localStorage": { "done": true, "at": "1970-01-01T00:00:00.000Z", "errors": [] } })
    );
    assert_eq!(dir_digest(&paths.data_dir.join("Local Storage")), before);
    assert!(migrate_local_storage(&paths, SystemTime::UNIX_EPOCH).skipped);
}

#[test]
fn corrupt_leveldb_gives_defaults_and_records_error() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::under(tmp.path());
    let leveldb = paths.data_dir.join("Local Storage").join("leveldb");
    copy_dir(&fixture_dir("leveldb-0.4.7"), &leveldb);
    std::fs::write(
        leveldb.join("CURRENT"),
        [b"\x00\xff".as_slice(), "мусор\n".as_bytes()].concat(),
    )
    .unwrap();

    let r = migrate_local_storage(&paths, SystemTime::UNIX_EPOCH);
    assert!(!r.skipped);
    assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
    assert_eq!(read_json(&paths.file(PREFS_FILE)), prefs_defaults());
    let log = read_json(&paths.file(MIGRATION_FILE));
    assert_eq!(log["localStorage"]["done"], true);
    assert_eq!(log["localStorage"]["errors"], json!(r.errors));
    assert!(migrate_local_storage(&paths, SystemTime::UNIX_EPOCH).skipped);
}

#[test]
fn corrupt_table_panic_becomes_error() {
    // rusty-leveldb проверяет подпись таблицы .ldb через assert: паника — ошибка переноса.
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::under(tmp.path());
    let leveldb = paths.data_dir.join("Local Storage").join("leveldb");
    copy_dir(&fixture_dir("leveldb-0.4.7"), &leveldb);
    let table = std::fs::read_dir(&leveldb)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "ldb"))
        .unwrap();
    let mut bytes = std::fs::read(&table).unwrap();
    let len = bytes.len();
    bytes[len - 1] ^= 0xFF;
    std::fs::write(&table, bytes).unwrap();

    let r = migrate_local_storage(&paths, SystemTime::UNIX_EPOCH);
    assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
    assert!(r.errors[0].contains("LevelDB упал"), "{:?}", r.errors);
    assert_eq!(read_json(&paths.file(PREFS_FILE)), prefs_defaults());
    assert!(MigrationLog::open(&paths).is_done(STEP_LOCAL_STORAGE));
}

#[test]
fn pane_failure_keeps_other_keys() {
    // hlsr.pane.* лежит в своей таблице .ldb, три ключа — в журнале: get до испорченной
    // таблицы не доходит, обход доходит и падает. Теряются только размеры панелей.
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::under(tmp.path());
    let leveldb = paths.data_dir.join("Local Storage").join("leveldb");
    std::fs::create_dir_all(&leveldb).unwrap();
    let put = |items: &[(&str, &str)]| {
        let options = rusty_leveldb::Options {
            create_if_missing: true,
            reuse_logs: false,
            ..Default::default()
        };
        let mut db = rusty_leveldb::DB::open(&leveldb, options).unwrap();
        for (name, value) in items {
            let key = [b"_file://\x00\x01".as_slice(), name.as_bytes()].concat();
            db.put(&key, &[b"\x01".as_slice(), value.as_bytes()].concat())
                .unwrap();
        }
        db.close().unwrap();
    };
    put(&[("hlsr.pane.keyboard:hl1-won", "412")]);
    // Открытие переносит журнал прошлого запуска в таблицу.
    put(&[
        ("hlsr-launcher-prefs", r#"{"launcherTheme":"paper"}"#),
        ("hlsr.keyboard.runMode", "scripted"),
        ("wasLaunched", "true"),
    ]);
    let table = std::fs::read_dir(&leveldb)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "ldb"))
        .unwrap();
    let mut bytes = std::fs::read(&table).unwrap();
    let len = bytes.len();
    bytes[len - 1] ^= 0xFF;
    std::fs::write(&table, bytes).unwrap();

    let r = migrate_local_storage(&paths, SystemTime::UNIX_EPOCH);
    assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
    assert!(r.errors[0].contains("hlsr.pane.*"), "{:?}", r.errors);
    let written = read_json(&paths.file(PREFS_FILE));
    assert_eq!(written["launcherTheme"], "paper");
    assert_eq!(written["keyboardRunMode"], "scripted");
    assert_eq!(written["wasLaunched"], true);
    assert!(written.get("pane").is_none());
}

#[test]
fn no_local_storage_is_done_without_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::under(tmp.path());
    let r = migrate_local_storage(&paths, SystemTime::UNIX_EPOCH);
    assert!(!r.skipped && r.errors.is_empty());
    assert_eq!(
        read_json(&paths.file(MIGRATION_FILE))["localStorage"]["done"],
        true
    );
    assert!(!paths.file(PREFS_FILE).exists());
}

#[test]
fn unparsable_prefs_keep_other_values() {
    // Как load() 0.4.7: битый JSON → умолчания; режим клавиатуры и wasLaunched — свои ключи.
    let values = hlsr_core::migration::local_storage::LocalStorageValues {
        prefs: Some("{не json".into()),
        keyboard_run_mode: Some("scripted".into()),
        was_launched: Some("true".into()),
        ..Default::default()
    };
    let (prefs, errors) = hlsr_core::migration::local_storage::build_prefs(&values);
    assert_eq!(errors.len(), 1, "{errors:?}");
    let mut expected = prefs_defaults();
    expected["keyboardRunMode"] = json!("scripted");
    expected["wasLaunched"] = json!(true);
    assert_eq!(prefs, expected);
}

#[test]
fn prefs_values_follow_0_4_7_rules() {
    use hlsr_core::migration::local_storage::{LocalStorageValues, build_prefs};
    let build = |prefs: Option<&str>, mode: Option<&str>, launched: Option<&str>| {
        let (value, errors) = build_prefs(&LocalStorageValues {
            prefs: prefs.map(str::to_owned),
            keyboard_run_mode: mode.map(str::to_owned),
            was_launched: launched.map(str::to_owned),
            ..Default::default()
        });
        assert!(errors.is_empty(), "{errors:?}");
        value
    };
    // Нет ключей → умолчания.
    assert_eq!(build(None, None, None), prefs_defaults());
    // JSON.parse("null") || {} и не-объекты → {}.
    assert_eq!(build(Some("null"), None, None), prefs_defaults());
    assert_eq!(build(Some("[1,2]"), None, None), prefs_defaults());
    // Режим: только "scripted" остаётся, прочее — "scriptless".
    assert_eq!(
        build(None, Some("Scripted"), None)["keyboardRunMode"],
        "scriptless"
    );
    // wasLaunched — как if (localStorage.getItem("wasLaunched")): любая непустая строка.
    assert_eq!(build(None, None, Some("true"))["wasLaunched"], true);
    assert_eq!(build(None, None, Some("1"))["wasLaunched"], true);
    assert_eq!(build(None, None, Some(""))["wasLaunched"], false);
    // Чистка launcher-prefs.js: прежние ключи удаляются, speedLimit не из списка → 0,
    // незнакомые ключи остаются после ключей умолчаний.
    let value = build(Some(r#"{"zz":1,"uiScale":1.5,"speedLimit":7}"#), None, None);
    assert_eq!(value["speedLimit"], 0);
    assert!(value.get("uiScale").is_none());
    assert_eq!(value.as_object().unwrap().keys().next_back().unwrap(), "zz");
}

#[test]
fn migrates_0_4_9_keys_like_0_4_9() {
    // leveldb-0.4.9.json: ключи, которые записал Electron 0.4.9, и что из них читают
    // load() из launcher-prefs.js и readSize() из pane-size.js 0.4.9.
    let expected = read_json(&fixture_dir("leveldb-0.4.9.json"));
    let items = expected["localStorage"].as_object().unwrap();
    let v = read_local_storage(&fixture_dir("leveldb-0.4.9")).unwrap();
    assert_eq!(v.prefs.as_deref(), items["hlsr-launcher-prefs"].as_str());
    assert_eq!(
        v.keyboard_run_mode.as_deref(),
        items["hlsr.keyboard.runMode"].as_str()
    );
    assert_eq!(v.was_launched.as_deref(), items["wasLaunched"].as_str());
    // Все hlsr.pane.* как есть, удалённый (writeSize null) — нет.
    let pane_items: Vec<(String, String)> = items
        .iter()
        .filter_map(|(key, value)| {
            let pane = key.strip_prefix("hlsr.pane.")?;
            Some((pane.to_owned(), value.as_str().unwrap().to_owned()))
        })
        .collect();
    assert_eq!(v.pane, pane_items);

    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::under(tmp.path());
    copy_dir(
        &fixture_dir("leveldb-0.4.9"),
        &paths.data_dir.join("Local Storage").join("leveldb"),
    );
    let r = migrate_local_storage(&paths, SystemTime::UNIX_EPOCH);
    assert!(!r.skipped && r.errors.is_empty(), "{:?}", r.errors);

    let mut written = read_json(&paths.file(PREFS_FILE));
    assert_eq!(written["pane"], expected["pane"]);
    assert_eq!(written["keyboardRunMode"], "scripted");
    assert_eq!(written["wasLaunched"], true);
    let prefs = written.as_object_mut().unwrap();
    for key in ["keyboardRunMode", "wasLaunched", "pane"] {
        prefs.shift_remove(key);
    }
    // Байт в байт с JSON.stringify(load()) 0.4.9: значения и порядок ключей.
    assert_eq!(
        hlsr_core::json_text::stringify(&written, ""),
        expected["prefs"].as_str().unwrap()
    );

    let prefs = LauncherPrefs::open(&paths);
    assert_eq!(prefs.rule_category_of("hl1-won"), "scripted");
    assert_eq!(prefs.rule_category_of("ghosting-renewed"), "scriptless");
    assert_eq!(prefs.rule_category_of("hl2-4260"), "scriptless");
}

#[test]
fn pane_sizes_follow_read_size() {
    use hlsr_core::migration::local_storage::{LocalStorageValues, build_prefs};
    let pane = |items: &[(&str, &str)]| {
        let (value, errors) = build_prefs(&LocalStorageValues {
            pane: items
                .iter()
                .map(|&(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            ..Default::default()
        });
        assert!(errors.is_empty(), "{errors:?}");
        value.get("pane").cloned()
    };
    // Number(значение): пробелы, дробь, 0x; не конечное и не больше нуля — не размер.
    assert_eq!(
        pane(&[("a", " 400 "), ("b", "12.5"), ("c", "0x10"), ("d", "1e3")]),
        Some(json!({"a": 400, "b": 12.5, "c": 16, "d": 1000}))
    );
    assert_eq!(
        pane(&[("a", ""), ("b", "-5"), ("c", "Infinity"), ("d", "12px")]),
        None
    );
    assert_eq!(pane(&[]), None);
}

#[test]
fn existing_prefs_file_is_not_overwritten() {
    // launcher-prefs.json уже пишет новая версия: шаг считается сделанным, файл не трогается.
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::under(tmp.path());
    copy_dir(
        &fixture_dir("leveldb-0.4.7"),
        &paths.data_dir.join("Local Storage").join("leveldb"),
    );
    std::fs::write(
        paths.file(PREFS_FILE),
        b"{\n\t\"launcherTheme\": \"hlsr\"\n}",
    )
    .unwrap();
    let r = migrate_local_storage(&paths, SystemTime::UNIX_EPOCH);
    assert!(!r.skipped && r.errors.is_empty());
    assert_eq!(
        std::fs::read(paths.file(PREFS_FILE)).unwrap(),
        b"{\n\t\"launcherTheme\": \"hlsr\"\n}"
    );
    assert!(MigrationLog::open(&paths).is_done(STEP_LOCAL_STORAGE));
}

#[test]
fn migration_log_keeps_other_steps() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::under(tmp.path());
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    std::fs::write(
        paths.file(MIGRATION_FILE),
        br#"{"session":{"done":true,"at":"2026-10-14T00:00:00.000Z","errors":[]},"x":1}"#,
    )
    .unwrap();
    let mut log = MigrationLog::open(&paths);
    assert!(log.is_done("session"));
    assert!(!log.is_done(STEP_LOCAL_STORAGE));
    let at = SystemTime::UNIX_EPOCH + Duration::from_millis(1_760_400_000_123);
    assert!(log.record(STEP_LOCAL_STORAGE, &["ошибка".to_owned()], at));
    let written = read_json(&paths.file(MIGRATION_FILE));
    assert_eq!(written["session"]["done"], true);
    assert_eq!(written["x"], 1);
    assert_eq!(
        written["localStorage"],
        json!({ "done": true, "at": "2025-10-14T00:00:00.123Z", "errors": ["ошибка"] })
    );
    assert!(MigrationLog::open(&paths).is_done(STEP_LOCAL_STORAGE));
}
