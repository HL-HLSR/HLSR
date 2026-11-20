use hlsr_core::json_store::write_state;
use hlsr_core::json_text::JsonFormat;
use hlsr_core::language::{Language, ensure_language_with, pick_initial};
use hlsr_core::launcher_prefs::{LauncherPrefs, PREFS_FILE, prefs_defaults, sanitize_prefs};
use hlsr_core::paths::Paths;
use hlsr_core::settings::Settings;
use serde_json::{Value, json};

#[test]
fn installer_origin_wins_then_system_locale() {
    assert_eq!(pick_initial(Some("RU"), Some("en-US")), Language::Ru);
    assert_eq!(pick_initial(Some("en"), Some("ru-RU")), Language::En);
    assert_eq!(pick_initial(Some("de"), Some("ru_RU.UTF-8")), Language::Ru);
    assert_eq!(pick_initial(None, Some("ru")), Language::Ru);
    assert_eq!(pick_initial(None, Some("rus")), Language::En);
    assert_eq!(pick_initial(None, None), Language::En);
    // reg.exe query: значение после REG_SZ — первое слово.
    assert_eq!(pick_initial(Some("  ru  "), Some("en")), Language::Ru);
    assert_eq!(pick_initial(Some("ru extra"), Some("en")), Language::Ru);
    assert_eq!(pick_initial(Some(""), Some("ru")), Language::Ru);
}

#[test]
fn ensure_language_writes_once() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    std::fs::write(paths.file("settings.json"), "").unwrap();

    let mut settings = Settings::open(&paths);
    assert_eq!(settings.language(), None);
    assert_eq!(
        ensure_language_with(&mut settings, || Language::Ru),
        Language::Ru
    );

    let mut settings = Settings::open(&paths);
    assert_eq!(settings.language(), Some(Language::Ru));
    assert_eq!(
        ensure_language_with(&mut settings, || panic!("язык уже записан")),
        Language::Ru
    );
}

#[test]
fn ensure_language_keeps_other_config_fields() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    write_state(
        &paths.file("settings.json"),
        &json!({"config": {"language": 2, "gpuAcceleration": false}}),
        JsonFormat::Store,
    )
    .unwrap();
    let mut settings = Settings::open(&paths);
    assert_eq!(
        ensure_language_with(&mut settings, || Language::En),
        Language::En
    );
    assert_eq!(
        std::fs::read_to_string(paths.file("settings.json")).unwrap(),
        "{\n\t\"config\": {\n\t\t\"language\": 0,\n\t\t\"gpuAcceleration\": false\n\t}\n}"
    );
}

#[test]
fn sanitize_removes_legacy_keys_and_bad_speed() {
    let v = sanitize_prefs(
        json!({"accent": "#fff", "uiScale": 2, "speedLimit": 3, "launcherTheme": "xen", "custom": 1}),
    );
    assert!(v.get("accent").is_none() && v.get("uiScale").is_none());
    assert_eq!(v["speedLimit"], 0);
    assert_eq!(v["launcherTheme"], "xen");
    assert_eq!(v["custom"], 1);
    assert_eq!(v["animations"], true);
    assert_eq!(sanitize_prefs(json!([1])), prefs_defaults());
    assert_eq!(sanitize_prefs(Value::Null), prefs_defaults());
    assert_eq!(sanitize_prefs(json!({"speedLimit": "5"}))["speedLimit"], 0);
    assert_eq!(sanitize_prefs(json!({"speedLimit": 5}))["speedLimit"], 5);
    assert_eq!(sanitize_prefs(json!({"speedLimit": 5.0}))["speedLimit"], 5);
    assert_eq!(sanitize_prefs(json!({"speedLimit": 5.5}))["speedLimit"], 0);
}

#[test]
fn sanitize_keeps_defaults_order_then_saved_keys() {
    let v = sanitize_prefs(json!({"custom": 1, "welcomeSeen": true, "animations": false}));
    let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        [
            "speedLimit",
            "animations",
            "hideWhilePlaying",
            "fullscreenButton",
            "hideSteamId",
            "timerOffered",
            "toolSplits",
            "liveSplitGame",
            "coloredBoot",
            "launcherTheme",
            "paperAccent",
            "railExpanded",
            "welcomeSeen",
            "configTourSeen",
            "ruleCategory",
            "keyboardFull",
            "ghostingBuild",
            "keyboardRunMode",
            "wasLaunched",
            "uncappedFps",
            "custom",
        ]
    );
    assert_eq!(v["animations"], false);
    assert_eq!(v["welcomeSeen"], true);
    assert_eq!(v["keyboardRunMode"], "scriptless");
    assert_eq!(v["wasLaunched"], false);
}

#[test]
fn theme_id_falls_back_to_hlsr() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    write_state(
        &paths.file(PREFS_FILE),
        &json!({"launcherTheme": 5}),
        JsonFormat::Store,
    )
    .unwrap();
    assert_eq!(LauncherPrefs::open(&paths).theme_id(), "hlsr");

    write_state(
        &paths.file(PREFS_FILE),
        &json!({"launcherTheme": "xen"}),
        JsonFormat::Store,
    )
    .unwrap();
    assert_eq!(LauncherPrefs::open(&paths).theme_id(), "xen");

    let empty = tempfile::tempdir().unwrap();
    assert_eq!(
        LauncherPrefs::open(&Paths::under(empty.path())).theme_id(),
        "hlsr"
    );
}

#[test]
fn prefs_are_sanitized_on_open_and_saved_whole() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    write_state(
        &paths.file(PREFS_FILE),
        &json!({"accent": "#fff", "speedLimit": 3, "railExpanded": true}),
        JsonFormat::Store,
    )
    .unwrap();
    let mut prefs = LauncherPrefs::open(&paths);
    assert!(prefs.get("accent").is_none());
    assert_eq!(prefs.get("speedLimit"), Some(&json!(0)));
    assert_eq!(prefs.get("railExpanded"), Some(&json!(true)));
    assert_eq!(prefs.get("animations"), Some(&json!(true)));

    // Файл до первой записи не меняется.
    let before = std::fs::read_to_string(paths.file(PREFS_FILE)).unwrap();
    assert!(before.contains("accent"));

    assert!(prefs.set("speedLimit", json!(7)));
    assert_eq!(prefs.get("speedLimit"), Some(&json!(0)));
    assert!(prefs.set("welcomeSeen", json!(true)));
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(paths.file(PREFS_FILE)).unwrap()).unwrap();
    let mut expected = prefs_defaults();
    expected["railExpanded"] = json!(true);
    expected["welcomeSeen"] = json!(true);
    assert_eq!(saved, expected);

    assert!(!prefs.set("accent", json!("#000")));
    assert!(prefs.get("accent").is_none());
}

#[test]
fn save_keeps_rule_category_and_separate_keys() {
    // save() 0.4.9: снимок целиком, но ruleCategory и keyboardFull пишут только свои функции.
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let mut prefs = LauncherPrefs::open(&paths);
    assert_eq!(prefs.get("fullscreenButton"), Some(&json!(true)));
    assert_eq!(prefs.get("paperAccent"), Some(&json!("clay")));
    assert_eq!(prefs.get("ghostingBuild"), Some(&json!("HL1 MOVEMENT")));
    assert_eq!(prefs.rule_category_of("hl1-won"), "scriptless");

    assert!(prefs.set_rule_category("hl1-won", "scripted"));
    assert!(prefs.set_rule_category("hl2-4260", "что-то"));
    assert!(prefs.set_keyboard_full(true));
    assert!(prefs.set("keyboardRunMode", json!("scripted")));
    assert!(prefs.set("pane", json!({"keyboard:hl1-won": 412})));
    assert_eq!(prefs.rule_category_of("hl1-won"), "scripted");
    assert_eq!(
        prefs.get("ruleCategory"),
        Some(&json!({"hl1-won": "scripted", "hl2-4260": "scriptless"}))
    );

    // Чужой снимок (как load() до смены категории) с прежними значениями.
    let mut snapshot = prefs_defaults();
    let snapshot_map = snapshot.as_object_mut().unwrap();
    snapshot_map.shift_remove("keyboardRunMode");
    snapshot_map.shift_remove("wasLaunched");
    snapshot["paperAccent"] = json!("sea");
    snapshot["accent"] = json!("#fff");
    assert!(prefs.save(snapshot));

    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(paths.file(PREFS_FILE)).unwrap()).unwrap();
    assert_eq!(saved["paperAccent"], "sea");
    assert!(saved.get("accent").is_none());
    assert_eq!(
        saved["ruleCategory"],
        json!({"hl1-won": "scripted", "hl2-4260": "scriptless"})
    );
    assert_eq!(saved["keyboardFull"], true);
    assert_eq!(saved["keyboardRunMode"], "scripted");
    assert_eq!(saved["pane"], json!({"keyboard:hl1-won": 412}));
    assert_eq!(
        LauncherPrefs::open(&paths).get("paperAccent"),
        Some(&json!("sea"))
    );
}

#[test]
fn set_many_writes_all_keys_by_set_rules() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let mut prefs = LauncherPrefs::open(&paths);
    let changes = json!({"railExpanded": true, "speedLimit": 7, "animations": false});
    assert!(prefs.set_many(changes.as_object().unwrap()));
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(paths.file(PREFS_FILE)).unwrap()).unwrap();
    let mut expected = prefs_defaults();
    expected["railExpanded"] = json!(true);
    expected["animations"] = json!(false);
    assert_eq!(saved, expected);

    // Ключ прежних версий не пишется, остальные — да.
    let changes = json!({"accent": "#000", "welcomeSeen": true});
    assert!(!prefs.set_many(changes.as_object().unwrap()));
    assert!(prefs.get("accent").is_none());
    assert_eq!(
        LauncherPrefs::open(&paths).get("welcomeSeen"),
        Some(&json!(true))
    );
}
