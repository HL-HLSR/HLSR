//! База доставки 0.4.9 (задача T0): фикстура `base-049.json` из
//! `tools/reference/delivery/base-049.js` (модули release-0.4.9 на тех же входах) и самотесты
//! `delivery-errors`, `ansi-name`, `user-configs` 0.4.9 на диске.

mod support;

use hlsr_core::js::Platform;
use hlsr_delivery::ansi_name::readable_name;
use hlsr_delivery::custom_packs::{self, GAMES, PACKS};
use hlsr_delivery::errors::{Code, DeliveryError, Failure, RawError, preserve_error_on};
use hlsr_delivery::game_settings_store::{GameSettingsStore, StoreError};
use hlsr_delivery::launch_args::{self, imported_settings};
use hlsr_delivery::library_marker::{marker_linked, read_marker, write_marker};
use hlsr_delivery::links::LINKS;
use hlsr_delivery::paths::{
    assert_real_inside, ensure_layout, mark_created_library, prepare_library_root, resolve_library,
};
use hlsr_delivery::ports::TracingLogger;
use serde_json::{Map, Value, json};
use std::path::Path;
use std::sync::{Arc, LazyLock};
use std::time::SystemTime;
use support::scenario::temp_root;
use support::{assert_json_eq, fixture, items, opt_str, str_of};

static BASE: LazyLock<Value> = LazyLock::new(|| fixture("base-049.json"));

// ---------------------------------------------------------------- errors.js: preserveError

fn raw_error(fields: &Value) -> RawError {
    RawError {
        code: opt_str(&fields["code"]).map(str::to_owned),
        syscall: opt_str(&fields["syscall"]).map(str::to_owned),
        path: opt_str(&fields["path"]).map(str::to_owned),
        message: opt_str(&fields["message"]).unwrap_or_default().to_owned(),
        stderr: None,
    }
}

#[test]
fn preserve_error_like_0_4_9() {
    for case in items(&BASE["errors"]["preserve"]) {
        let platform = match str_of(&case["platform"]) {
            "win32" => Platform::Win32,
            _ => Platform::Linux,
        };
        let failure = match case["error"].get("raw") {
            Some(raw) => Failure::Raw(raw_error(raw)),
            None => {
                let own = &case["error"]["delivery"];
                Failure::Delivery(
                    DeliveryError::new(
                        Code::from_name(str_of(&own["code"])).unwrap(),
                        str_of(&own["message"]),
                    )
                    .with_details(own["details"].clone()),
                )
            }
        };
        assert_json_eq(
            &preserve_error_on(platform, failure).to_public(),
            &case["public"],
            &format!("{} {}", case["platform"], case["error"]),
        );
    }
}

// ---------------------------------------------------------------- custom-packs.js, links.js

#[test]
fn custom_packs_table_like_0_4_9() {
    let expected = &BASE["customPacks"];
    let packs: Vec<Value> = PACKS
        .iter()
        .map(|pack| {
            json!({"id": pack.id, "game": pack.game, "kind": pack.kind, "author": pack.author,
                   "files": pack.files})
        })
        .collect();
    assert_json_eq(&json!(packs), &expected["packs"], "PACKS");
    assert_json_eq(
        &json!(custom_packs::targets()),
        &expected["targets"],
        "targets()",
    );
    let games: Map<String, Value> = GAMES
        .iter()
        .map(|game| {
            let fields = json!({"sourceDir": game.source_dir, "pruneRoot": game.prune_root,
                                "baseDir": game.base_dir});
            (game.id.to_owned(), fields)
        })
        .collect();
    assert_json_eq(&Value::Object(games), &expected["games"], "GAMES");
    assert_eq!(custom_packs::STATE_DIR, str_of(&expected["stateDir"]));
}

#[test]
fn links_like_0_4_9() {
    let mut expected: Vec<(&str, &str)> = items(&BASE["links"])
        .iter()
        .map(|pair| (str_of(&pair[0]), str_of(&pair[1])))
        .collect();
    // Отступление 0.5.0: страница проекта и выпусков — HL-HLSR/HLSR вместо HLSR-Releases.
    let github = expected
        .iter()
        .position(|(key, _)| *key == "github")
        .unwrap();
    assert_eq!(
        expected[github].1,
        "https://github.com/HL-HLSR/HLSR-Releases"
    );
    expected[github].1 = "https://github.com/HL-HLSR/HLSR";
    expected.insert(
        github + 1,
        ("releases", "https://github.com/HL-HLSR/HLSR/releases"),
    );
    assert_eq!(LINKS.to_vec(), expected);
}

// ---------------------------------------------------------------- shared/ansi-name.js

#[test]
fn readable_names_like_0_4_9() {
    for case in items(&BASE["ansiName"]) {
        let input = str_of(&case["input"]);
        assert_eq!(readable_name(input), str_of(&case["output"]), "{input:?}");
    }
}

// ---------------------------------------------------------------- launch-args.js, GameSettingsStore

#[test]
fn imported_settings_keep_own_proton_and_safe_args() {
    let expected = &BASE["launchArgs"];
    let current = launch_args::sanitize_settings(&expected["current"]).settings;
    for case in items(&expected["imported"]) {
        let out = imported_settings(&case["raw"], &current);
        let launch_args = match &case["launchArgs"] {
            Value::String(text) if text == "<undefined>" => Value::Null,
            other => other.clone(),
        };
        assert_eq!(out["launchArgs"], launch_args, "{}", case["raw"]);
        assert_eq!(
            out["protonOptions"], case["protonOptions"],
            "{}",
            case["raw"]
        );
    }
}

#[test]
fn vulkan_setting_like_0_4_9() {
    let expected = &BASE["launchArgs"];
    for case in items(&expected["vulkan"]) {
        let settings = launch_args::sanitize_settings(&json!({"vulkan": case["value"]})).settings;
        assert_eq!(
            json!(settings.vulkan),
            case["settings"],
            "{}",
            case["value"]
        );
    }
    let defaults: Value = serde_json::from_str(str_of(&expected["defaultsText"])).unwrap();
    let keys = |value: &Value| {
        value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        keys(&launch_args::default_settings().to_value()),
        keys(&defaults)
    );
    assert_eq!(launch_args::default_settings().to_value()["vulkan"], false);
}

#[test]
fn save_imported_rejects_bad_game_id_and_keeps_own_proton() {
    let (_temp, root) = temp_root();
    let store = GameSettingsStore::new(root.join("game-settings.json"), Arc::new(TracingLogger));
    let error = store.save_imported("../x", &json!({})).unwrap_err();
    assert!(matches!(error, StoreError::Rejected(e) if e.code == "INVALID_GAME_ID"));
    store
        .save(
            "hl1-won",
            &json!({"launchArgs": "-novid", "protonOptions": "PROTON_LOG=1 %command%"}),
        )
        .unwrap();
    let saved = store
        .save_imported(
            "hl1-won",
            &json!({"launchArgs": "+exec x.cfg", "protonOptions": "rm -rf %command%", "vulkan": true}),
        )
        .unwrap();
    assert_eq!(saved.launch_args, "-novid");
    assert_eq!(saved.proton_options, "PROTON_LOG=1 %command%");
    assert!(saved.vulkan);
}

// ---------------------------------------------------------------- paths.js, library-cleanup.js

/// Ссылка на папку: junction в Windows (без прав администратора), symlink в Linux.
fn link_dir(link: &Path, target: &Path) {
    #[cfg(windows)]
    {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "mklink /J");
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
}

fn code(error: &DeliveryError) -> &'static str {
    error.code.as_str()
}

#[test]
fn real_inside_refuses_paths_through_links() {
    let (_temp, root) = temp_root();
    let game = root.join("game");
    let outside = root.join("outside");
    std::fs::create_dir_all(game.join("cfg")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("x.cfg"), "x").unwrap();

    assert_real_inside(&game, &game.join("cfg").join("new.cfg"), None).unwrap();
    assert_real_inside(&game, &game.join("deep").join("er").join("a.cfg"), None).unwrap();

    link_dir(&game.join("linked"), &outside);
    let error = assert_real_inside(
        &game,
        &game.join("linked").join("x.cfg"),
        Some("Файл игрока"),
    )
    .unwrap_err();
    assert_eq!(code(&error), "PATH_INVALID");
    assert!(
        error
            .message
            .starts_with("Файл игрока ведёт через ссылку за пределы ")
    );
    let error = assert_real_inside(&game, &game.join("linked").join("new").join("y.cfg"), None)
        .unwrap_err();
    assert_eq!(code(&error), "PATH_INVALID");
    // Сама ссылка — тоже отказ, даже без файла за ней.
    let error = assert_real_inside(&game, &game.join("linked"), None).unwrap_err();
    assert_eq!(code(&error), "PATH_INVALID");
    // Корня нет — отказ (realpath не развернуть).
    let error = assert_real_inside(&root.join("missing"), &root.join("missing").join("a"), None)
        .unwrap_err();
    assert_eq!(code(&error), "PATH_INVALID");
}

#[test]
fn library_marker_created_by_launcher_and_never_through_links() {
    let (_temp, root) = temp_root();
    let now = SystemTime::now();

    // ensureLayout: папки не было — маркер «создал лаунчер»; была — без отметки.
    let fresh = resolve_library(&root.join("fresh").to_string_lossy()).unwrap();
    ensure_layout(&fresh, now).unwrap();
    assert_eq!(read_marker(&fresh.root).unwrap()["createdBy"], "launcher");
    let existing_root = root.join("existing");
    std::fs::create_dir_all(&existing_root).unwrap();
    let existing = resolve_library(&existing_root.to_string_lossy()).unwrap();
    ensure_layout(&existing, now).unwrap();
    assert_eq!(read_marker(&existing.root).unwrap().get("createdBy"), None);

    // prepareLibraryRoot / markCreatedLibrary.
    let picked = root.join("picked").join("HLSR");
    prepare_library_root(&picked).unwrap();
    assert!(picked.is_dir());
    assert!(mark_created_library(&picked, now));
    assert_eq!(read_marker(&picked).unwrap()["createdBy"], "launcher");
    let mine = root.join("mine");
    std::fs::create_dir_all(&mine).unwrap();
    prepare_library_root(&mine).unwrap();
    assert!(!mark_created_library(&mine, now));
    assert_eq!(read_marker(&mine), None);

    // .hlsr — ссылка на чужую папку: маркер не читается и через неё не пишется.
    let foreign = root.join("foreign");
    write_marker(&foreign, now, Some("installer")).unwrap();
    let linked = root.join("linked");
    std::fs::create_dir_all(&linked).unwrap();
    link_dir(&linked.join(".hlsr"), &foreign.join(".hlsr"));
    assert!(marker_linked(&linked));
    assert_eq!(read_marker(&linked), None);
    assert!(write_marker(&linked, now, None).is_err());
    assert_eq!(read_marker(&foreign).unwrap()["createdBy"], "installer");
}

// ---------------------------------------------------------------- user-configs.js (makeWritableSync — foundation_os.rs)

/// user-configs-selftest.js 0.4.9: нечитаемый cfg считается изменённым, отсутствующий — нет.
#[cfg(windows)]
#[test]
fn unreadable_config_counts_as_edited() {
    use hlsr_delivery::user_configs::is_user_edited;
    use std::os::windows::fs::OpenOptionsExt;
    let (_temp, root) = temp_root();
    let previous = json!({"files": [{"path": "cfg/a.cfg", "sha256": "0".repeat(64)}]});
    std::fs::create_dir_all(root.join("cfg")).unwrap();
    std::fs::write(root.join("cfg").join("a.cfg"), "a").unwrap();
    // Открыт без общего доступа: чтение даёт EBUSY.
    let _held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(root.join("cfg").join("a.cfg"))
        .unwrap();
    assert!(is_user_edited(&root, "cfg/a.cfg", Some(&previous), None));
    assert!(!is_user_edited(
        &root,
        "cfg/missing.cfg",
        Some(&previous),
        None
    ));
}
