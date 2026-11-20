//! Режимы запуска и аргументы, как `entry.js`/`index.js` 0.4.7; запись
//! STARTUP_ERROR, как `reportStartupError` + `NotificationStore.add`.

use hlsr_core::args::{LaunchArgs, LaunchMode, parse_args_for};
use hlsr_core::notifications::add_startup_error;
use serde_json::{Value, json};
use std::ffi::OsString;
use std::path::Path;

fn args(windows: bool, list: &[&str]) -> LaunchArgs {
    parse_args_for(list.iter().map(OsString::from), windows)
}

fn mode(windows: bool, list: &[&str]) -> LaunchMode {
    args(windows, list).mode
}

#[test]
fn modes_on_windows() {
    assert_eq!(mode(true, &[]), LaunchMode::Launcher);
    assert_eq!(mode(true, &["--no-sandbox"]), LaunchMode::Launcher);
    assert_eq!(mode(true, &["--hlsr-uninstall"]), LaunchMode::Uninstall);
    assert_eq!(
        mode(true, &["--hlsr-uninstall", "/S"]),
        LaunchMode::Uninstall
    );
    assert_eq!(
        mode(true, &["--hlsr-uninstall-cleanup", "--games"]),
        LaunchMode::UninstallCleanup
    );
    // index.js проверяет установщик и удаление раньше очистки.
    assert_eq!(
        mode(true, &["--hlsr-uninstall", "--hlsr-uninstall-cleanup"]),
        LaunchMode::Uninstall
    );
    // entry.js выбирает апдейтер раньше всего остального.
    assert_eq!(
        mode(true, &["--hlsr-updater", "--plan=x"]),
        LaunchMode::Updater
    );
    assert_eq!(
        mode(true, &["--hlsr-uninstall", "--hlsr-updater"]),
        LaunchMode::Updater
    );
    // layout.parseArgs не смотрит дальше /D=… — там путь до конца строки.
    assert_eq!(
        mode(true, &["/D=C:\\HLSR", "--hlsr-uninstall"]),
        LaunchMode::Launcher
    );
    // Флаги сравниваются целиком и с учётом регистра.
    assert_eq!(mode(true, &["--HLSR-UNINSTALL"]), LaunchMode::Launcher);
    assert_eq!(mode(true, &["--hlsr-updater=1"]), LaunchMode::Launcher);
}

#[test]
fn modes_on_linux() {
    // Удаление и апдейтер в 0.4.7 есть только у Windows-сборки.
    assert_eq!(mode(false, &["--hlsr-uninstall"]), LaunchMode::Launcher);
    assert_eq!(mode(false, &["--hlsr-updater"]), LaunchMode::Launcher);
    assert_eq!(
        mode(false, &["--hlsr-uninstall-cleanup"]),
        LaunchMode::UninstallCleanup
    );
    assert_eq!(
        mode(false, &["--hlsr-uninstall", "--hlsr-uninstall-cleanup"]),
        LaunchMode::UninstallCleanup
    );
}

#[test]
fn select_game_is_the_first_such_argument() {
    let game = |list: &[&str]| args(false, list).select_game;
    assert_eq!(game(&["--select-game=hl1-won"]).as_deref(), Some("hl1-won"));
    assert_eq!(
        game(&["--no-sandbox", "--select-game=hl2-4260"]).as_deref(),
        Some("hl2-4260")
    );
    assert_eq!(game(&["--select-game=HL1"]), None);
    assert_eq!(game(&["--select-game=bad_id", "--select-game=hl2"]), None);
    for bad in [
        "--select-game=",
        "--select-game=-a",
        "--select-game=a-",
        "--select-game=a--b",
    ] {
        assert_eq!(game(&[bad]), None, "{bad}");
    }
    assert_eq!(game(&["--select-game"]), None);
    assert_eq!(game(&[]), None);
}

#[test]
fn unknown_args_are_forwarded() {
    // `--hlsr-renderer=…` прежних сборок принимается молча, как любой незнакомый аргумент.
    let list = [
        "--no-sandbox",
        "--select-game=hl1-won",
        "--hlsr-renderer=dx12",
        "--что-угодно",
    ];
    assert_eq!(args(true, &list).mode, LaunchMode::Launcher);
    assert_eq!(args(true, &list).forwarded, list);
}

fn read(file: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

fn details(file: &Path) -> Vec<String> {
    read(file)["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["detail"].as_str().unwrap().to_owned())
        .collect()
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

#[test]
fn startup_error_is_written_and_coalesced() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notifications.json");
    add_startup_error(&file, "boom", 1_000).unwrap();
    add_startup_error(&file, "boom", 30_000).unwrap();
    add_startup_error(&file, "boom", 100_000).unwrap();
    let v = read(&file);
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["count"], 2);
    assert_eq!(items[0]["ts"], 30_000);
    assert_eq!(items[0]["id"], "rs-0");
    assert_eq!(items[0]["code"], "STARTUP_ERROR");
    assert_eq!(items[1]["id"], "255s-0");
    assert_eq!(items[1]["count"], 1);
    assert!(std::fs::read_to_string(&file).unwrap().ends_with("}\n"));
}

#[test]
fn startup_error_file_is_byte_exact() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notifications.json");
    add_startup_error(&file, "boom", 1_760_000_000_000).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "{\n  \"version\": 1,\n  \"items\": [\n    {\n      \"id\": \"mgj6k3cw-0\",\n      \
         \"ts\": 1760000000000,\n      \"level\": \"error\",\n      \"code\": \"STARTUP_ERROR\",\n      \
         \"detail\": \"boom\",\n      \"read\": false,\n      \"count\": 1\n    }\n  ]\n}\n"
    );
}

#[test]
fn startup_error_detail_is_trimmed() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notifications.json");

    // Обрезка в единицах UTF-16, как message.slice(0, 400).
    add_startup_error(&file, &"я".repeat(1000), 1_000).unwrap();
    assert_eq!(details(&file)[0], "я".repeat(400));

    // Половину эмодзи 0.4.7 записал бы одиночным суррогатом, его разбор даёт U+FFFD.
    let emoji = format!("a{}", "😀".repeat(300));
    add_startup_error(&file, &emoji, 200_000).unwrap();
    let expected = format!("a{}\u{FFFD}", "😀".repeat(199));
    assert_eq!(details(&file)[1], expected);
    assert_eq!(utf16_len(&expected), 400);

    // Маска удлиняет текст: после неё не больше 500 единиц.
    add_startup_error(&file, &"token=x ".repeat(50), 400_000).unwrap();
    let masked = &details(&file)[2];
    assert_eq!(utf16_len(masked), 500);
    assert!(masked.starts_with("token=<redacted> token=<redacted> "));
}

#[test]
fn startup_error_detail_is_redacted_like_notification_store() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notifications.json");
    add_startup_error(
        &file,
        "Error: EACCES C:\\Users\\Иван\\AppData\\x.json token=abc /home/ivan/.config/hlsr",
        1_000,
    )
    .unwrap();
    // redactText без имени пользователя: пути Linux 0.4.7 не прячет.
    assert_eq!(
        details(&file)[0],
        "Error: EACCES C:\\Users\\<user>\\AppData\\x.json token=<redacted> /home/ivan/.config/hlsr"
    );
}

fn write(file: &Path, value: &Value) {
    std::fs::write(file, serde_json::to_string(value).unwrap()).unwrap();
}

fn startup(id: &str, ts: u64, detail: &str, read: bool) -> Value {
    json!({ "id": id, "ts": ts, "level": "error", "code": "STARTUP_ERROR",
            "detail": detail, "read": read, "count": 1 })
}

#[test]
fn coalescing_rules() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notifications.json");
    let counts = |file: &Path| -> Vec<Value> {
        read(file)["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["count"].clone())
            .collect()
    };

    // Ровно 60 000 мс — ещё та же запись, 60 001 — уже новая.
    write(
        &file,
        &json!({ "items": [startup("a", 1_000, "boom", false)] }),
    );
    add_startup_error(&file, "boom", 61_000).unwrap();
    assert_eq!(counts(&file), [json!(2)]);
    add_startup_error(&file, "boom", 121_001).unwrap();
    assert_eq!(counts(&file), [json!(2), json!(1)]);

    // Прочитанная запись и другой текст не склеиваются.
    write(
        &file,
        &json!({ "items": [startup("a", 1_000, "boom", true)] }),
    );
    add_startup_error(&file, "boom", 2_000).unwrap();
    assert_eq!(counts(&file).len(), 2);
    add_startup_error(&file, "bang", 3_000).unwrap();
    assert_eq!(counts(&file).len(), 3);

    // Склейка смотрит только на последнюю запись.
    write(
        &file,
        &json!({ "items": [startup("a", 1_000, "boom", false), startup("b", 1_000, "bang", false)] }),
    );
    add_startup_error(&file, "boom", 2_000).unwrap();
    assert_eq!(counts(&file).len(), 3);

    // count считается как (count || 1) + 1 в JS.
    let mut item = startup("a", 1_000, "boom", false);
    item.as_object_mut().unwrap().remove("count");
    write(&file, &json!({ "items": [item] }));
    add_startup_error(&file, "boom", 2_000).unwrap();
    assert_eq!(counts(&file), [json!(2)]);
    let mut item = startup("a", 1_000, "boom", false);
    item["count"] = json!("5");
    write(&file, &json!({ "items": [item] }));
    add_startup_error(&file, "boom", 2_000).unwrap();
    assert_eq!(counts(&file), [json!("51")]);
}

#[test]
fn existing_file_is_filtered_like_read_all() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notifications.json");
    write(
        &file,
        &json!({
            "version": 7,
            "extra": true,
            "items": [
                { "id": "a", "ts": 5, "level": "warning", "code": "X1", "detail": "d",
                  "read": true, "count": 1, "meta": { "gameId": "hl1-won" } },
                { "id": 1, "ts": 5, "level": "info", "code": "X" },
                { "id": "b", "ts": "5", "level": "info", "code": "X" },
                { "id": "c", "ts": 5, "level": "fatal", "code": "X" },
                { "id": "d", "ts": 5, "level": "info", "code": "bad code" },
                { "id": "e", "ts": 5, "level": "info", "code": 123 },
                { "id": "f", "ts": 5, "level": "info" },
                [1, 2],
                null
            ]
        }),
    );
    add_startup_error(&file, "boom", 1_000).unwrap();
    let v = read(&file);
    assert_eq!(
        v.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["version", "items"]
    );
    assert_eq!(v["version"], 1);
    let ids: Vec<&str> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["a", "e", "rs-0"]);
    assert_eq!(v["items"][0]["meta"]["gameId"], "hl1-won");
}

#[test]
fn unreadable_content_starts_over_and_list_is_capped() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notifications.json");
    for text in ["{\"items\":", "[1,2]", "null", "{\"items\":{}}"] {
        std::fs::write(&file, text).unwrap();
        add_startup_error(&file, "boom", 1_000).unwrap();
        assert_eq!(details(&file), ["boom"], "{text}");
    }

    let items: Vec<Value> = (0..100)
        .map(|i| startup(&format!("n{i}"), 1_000, "old", true))
        .collect();
    write(&file, &json!({ "items": items }));
    add_startup_error(&file, "boom", 2_000).unwrap();
    let v = read(&file);
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 100);
    assert_eq!(items[0]["id"], "n1");
    assert_eq!(items[99]["detail"], "boom");
}
