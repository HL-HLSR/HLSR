use hlsr_core::json_store::{ReadState, quarantine, read_state, write_state};
use hlsr_core::json_text::{JsonFormat, js_number, parse_js_json, to_js_json};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/json")
        .join(name)
}

#[test]
fn store_files_round_trip_byte_for_byte() {
    for (name, fmt) in [
        ("settings-store.json", JsonFormat::Store),
        ("library-store.json", JsonFormat::Store),
        ("notifications-atomic.json", JsonFormat::Atomic),
    ] {
        let original = std::fs::read_to_string(fixture(name)).unwrap();
        let ReadState::Valid(v) = read_state(&fixture(name)) else {
            panic!("{name}")
        };
        if name.starts_with("notifications") {
            continue; // одиночный суррогат заменён: см. отдельный тест
        }
        assert_eq!(to_js_json(&v, fmt), original, "{name}");
    }
}

#[test]
fn lone_surrogate_becomes_replacement_char() {
    let ReadState::Valid(v) = read_state(&fixture("notifications-atomic.json")) else {
        panic!()
    };
    assert!(
        v["items"][0]["detail"]
            .as_str()
            .unwrap()
            .ends_with('\u{FFFD}')
    );
}

#[test]
fn bom_is_stripped() {
    let ReadState::Valid(v) = read_state(&fixture("bom.json")) else {
        panic!()
    };
    assert_eq!(v["config"]["language"], 0);
}

#[test]
fn non_object_roots_and_garbage_are_unparsable() {
    for name in ["root-null.json", "root-array.json", "broken.json"] {
        assert!(
            matches!(read_state(&fixture(name)), ReadState::Unparsable),
            "{name}"
        );
    }
}

#[test]
fn missing_file_is_missing() {
    assert!(matches!(
        read_state(Path::new("/nonexistent/x.json")),
        ReadState::Missing
    ));
}

#[test]
fn js_numbers_match_number_to_string() {
    let cases = [
        (1.0, "1"),
        (1.25, "1.25"),
        (0.1, "0.1"),
        (1e21, "1e+21"),
        (1e-7, "1e-7"),
        (-0.0, "0"),
        (123456789.0, "123456789"),
    ];
    for (n, s) in cases {
        assert_eq!(js_number(n).to_string(), s);
    }
    assert_eq!(js_number(f64::NAN), Value::Null);
}

#[cfg(unix)]
#[test]
fn store_format_writes_0600_and_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sub/settings.json");
    write_state(&file, &json!({"a": {"b": 1}}), JsonFormat::Store).unwrap();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "{\n\t\"a\": {\n\t\t\"b\": 1\n\t}\n}"
    );
    assert!(!dir.path().join("sub/settings.json.tmp").exists());
}

#[test]
fn quarantine_name_matches_0_4_7() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("settings.json");
    std::fs::write(&file, "{").unwrap();
    let at = SystemTime::UNIX_EPOCH + Duration::from_millis(1_791_202_322_600);
    let moved = quarantine(&file, at).unwrap();
    assert_eq!(
        moved.file_name().unwrap(),
        "settings.json.broken-2026-10-05T12-12-02-600Z"
    );
    assert!(!file.exists());
}

#[test]
fn atomic_format_writes_two_spaces_and_newline_without_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notifications.json");
    write_state(
        &file,
        &json!({"version": 1, "items": []}),
        JsonFormat::Atomic,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "{\n  \"version\": 1,\n  \"items\": []\n}\n"
    );
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["notifications.json"]);
}

#[test]
fn integer_keys_come_first_in_ascending_order() {
    // Так упорядочивает ключи объект JS: индексы массива — первыми, по возрастанию.
    let v =
        parse_js_json(r#"{"b": 1, "130": 2, "70": 3, "07": 4, "4294967295": 5, "50": 6}"#).unwrap();
    assert_eq!(
        to_js_json(&v, JsonFormat::Atomic),
        "{\n  \"50\": 6,\n  \"70\": 3,\n  \"130\": 2,\n  \"b\": 1,\n  \"07\": 4,\n  \"4294967295\": 5\n}\n"
    );
}

#[test]
fn numbers_are_written_as_js_reads_them() {
    let v =
        parse_js_json(r#"{"a": 1.50, "b": 1E5, "c": -0, "d": 1e400, "e": 12345678901234567890}"#)
            .unwrap();
    assert_eq!(
        to_js_json(&v, JsonFormat::Store),
        "{\n\t\"a\": 1.5,\n\t\"b\": 100000,\n\t\"c\": 0,\n\t\"d\": null,\n\t\"e\": 12345678901234567000\n}"
    );
}

#[test]
fn strings_are_escaped_like_js() {
    let v = json!({"s": "\"\\\u{8}\u{c}\n\r\t\u{1}\u{1f}\u{7f}\u{2028}/ё😀"});
    assert_eq!(
        to_js_json(&v, JsonFormat::Atomic),
        "{\n  \"s\": \"\\\"\\\\\\b\\f\\n\\r\\t\\u0001\\u001f\u{7f}\u{2028}/ё😀\"\n}\n"
    );
}

#[test]
fn surrogate_handling_keeps_pairs_and_escaped_backslashes() {
    let v = parse_js_json(r#"{"pair": "😀", "lit": "\\ud83d", "low": "\ude00x"}"#).unwrap();
    assert_eq!(v["pair"], "😀");
    assert_eq!(v["lit"], "\\ud83d");
    assert_eq!(v["low"], "\u{FFFD}x");
}

#[test]
fn invalid_utf8_is_read_with_replacement_chars() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("settings.json");
    std::fs::write(&file, b"{\"s\": \"a\xffb\"}").unwrap();
    let ReadState::Valid(v) = read_state(&file) else {
        panic!()
    };
    assert_eq!(v["s"], "a\u{FFFD}b");
}

#[cfg(unix)]
#[test]
fn directory_is_unreadable_with_code() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        read_state(dir.path()),
        ReadState::Unreadable { code } if code == "EISDIR"
    ));
}

#[test]
fn read_json_state_accepts_any_root_like_atomic_js() {
    use hlsr_core::json_store::{JsonState, read_json_safe, read_json_state};
    assert_eq!(
        read_json_state(Path::new("/nonexistent/x.json")),
        JsonState::Missing
    );
    assert_eq!(
        read_json_state(&fixture("bom.json")),
        JsonState::Valid(json!({"config": {"language": 0}}))
    );
    assert_eq!(
        read_json_state(&fixture("root-array.json")),
        JsonState::Valid(json!([1, 2]))
    );
    assert_eq!(
        read_json_state(&fixture("root-null.json")),
        JsonState::Valid(Value::Null)
    );
    assert!(matches!(
        read_json_state(&fixture("broken.json")),
        JsonState::Invalid { .. }
    ));
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        read_json_state(dir.path()),
        JsonState::Invalid { .. }
    ));
    assert_eq!(read_json_safe(&fixture("broken.json")), Value::Null);
    assert_eq!(read_json_safe(&fixture("root-array.json")), json!([1, 2]));
}

#[test]
fn write_file_atomic_writes_bytes_as_is_without_temp_files() {
    use hlsr_core::json_store::write_file_atomic;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sub").join("files.json");
    write_file_atomic(&file, b"{\"raw\":1}\r\n").unwrap();
    write_file_atomic(&file, "сырые байты\n".as_bytes()).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "сырые байты\n");
    let names: Vec<_> = std::fs::read_dir(file.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["files.json"]);
}

#[test]
fn atomic_temp_name_is_dot_name_pid_time36_tmp() {
    use hlsr_core::json_store::atomic_temp_name;
    let before = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let name = atomic_temp_name("config.cfg");
    // .config.cfg.<pid>.<время36>.tmp → [tmp, время36, pid, .config.cfg]
    let parts: Vec<&str> = name.rsplitn(4, '.').collect();
    assert_eq!(parts[0], "tmp");
    assert_eq!(parts[2], std::process::id().to_string());
    assert_eq!(parts[3], ".config.cfg");
    let time = u128::from_str_radix(parts[1], 36).unwrap();
    assert!(time >= before && time <= before + 60_000, "{name}");
}

#[test]
fn staged_atomic_write_names_the_stage_that_failed() {
    use hlsr_core::json_store::{AtomicStage, write_file_atomic_staged};
    let dir = tempfile::tempdir().unwrap();

    // Вместо каталога файла — файл: этап mkdir.
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    let error = write_file_atomic_staged(&blocker.join("x.cfg"), b"y").unwrap_err();
    assert_eq!(error.stage, AtomicStage::Mkdir);
    assert_eq!(error.dir, blocker);

    // Вместо файла — каталог: сбой на переименовании, временный файл убран.
    let target = dir.path().join("target.cfg");
    std::fs::create_dir(&target).unwrap();
    let error = write_file_atomic_staged(&target, b"y").unwrap_err();
    assert_eq!(error.stage, AtomicStage::Rename);
    assert!(error.temp.starts_with(dir.path()));
    assert!(
        error
            .temp
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(".target.cfg.")
    );
    assert!(!error.temp.exists());
    assert!(target.is_dir());

    // Успех и «тонкая» обёртка: те же байты, ошибка — просто io::Error.
    let file = dir.path().join("ok.cfg");
    write_file_atomic_staged(&file, b"1").unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"1");
    assert!(
        hlsr_core::json_store::write_file_atomic(&target, b"y")
            .unwrap_err()
            .raw_os_error()
            .is_some()
    );
}

#[cfg(unix)]
#[test]
fn staged_atomic_write_fails_at_open_in_a_read_only_directory() {
    use hlsr_core::json_store::{AtomicStage, write_file_atomic_staged};
    use std::os::unix::fs::PermissionsExt;
    if rustix::process::geteuid().is_root() {
        return; // root пишет и в каталог без прав
    }
    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let error = write_file_atomic_staged(&locked.join("a.cfg"), b"y").unwrap_err();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(error.stage, AtomicStage::Open);
    assert_eq!(
        error.error.kind(),
        std::io::ErrorKind::PermissionDenied,
        "{error:?}"
    );
    assert!(!error.temp.exists());
}
