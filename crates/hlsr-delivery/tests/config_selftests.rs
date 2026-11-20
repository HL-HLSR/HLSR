//! Самотесты 0.4.7 о редакторе конфигов с теми же случаями: `scripts/game-config-selftest.js`
//! (вкладка «Конфиги»: цепочка cfg, запись на место, выбор файла, импорт во вкладку),
//! `scripts/cfg-files-selftest.js` (редактор cfg-файлов) и `scripts/build-transfer-selftest.js`
//! (перенос сборки).

mod support;

use hlsr_core::js;
use hlsr_core::json_text::js_number;
use hlsr_delivery::build_transfer::{self, Build, TransferCode, TransferFailure};
use hlsr_delivery::cfg_files::{self, CfgFileCode, CfgFilesError, WriteOptions};
use hlsr_delivery::cfg_import::{self, ImportFile, ImportFiles, Picked, Root};
use hlsr_delivery::cfg_search::{CfgSearch, SteamLanguage};
use hlsr_delivery::game_binds::Encoding;
use hlsr_delivery::game_binds_install;
use hlsr_delivery::game_config::{self, FieldType, WindowValue};
use hlsr_delivery::game_config_install::{self, ConfigCode, WriteConfigError};
use hlsr_delivery::ghosting_personalization;
use hlsr_delivery::ghosting_settings;
use hlsr_delivery::goldsrc_personalization;
use hlsr_delivery::goldsrc_settings;
use hlsr_delivery::ports::FakeRegistry;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use support::scenario::{path_in, set_mtime};

const SOURCE: &str = "hl2";
const GHOSTING: &str = "ghostingmod";
const GOLDSRC: &str = "Half-Life/valve_WON";

fn search() -> CfgSearch {
    CfgSearch::new(SteamLanguage::new(Arc::new(FakeRegistry::default())))
}

fn temp() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    (dir, root)
}

fn put(root: &Path, name: &str, bytes: &[u8]) {
    let file = path_in(root, name);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, bytes).unwrap();
}

/// `tempInstall`: файлы HL1 (`Half-Life/…`) пишутся в latin1, остальные — в UTF-8.
fn temp_install(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let (dir, root) = temp();
    for (name, text) in files {
        let bytes = if name.starts_with("Half-Life") {
            js::latin1_bytes(text)
        } else {
            text.as_bytes().to_vec()
        };
        put(&root, name, &bytes);
    }
    (dir, root)
}

fn read_text(root: &Path, name: &str) -> String {
    String::from_utf8(std::fs::read(path_in(root, name)).unwrap()).unwrap()
}

fn read_latin1(root: &Path, name: &str) -> String {
    js::latin1_string(&std::fs::read(path_in(root, name)).unwrap())
}

fn write_cfg(
    root: &Path,
    game_directory: &str,
    game_id: &str,
    engine: &str,
    changes: Value,
) -> Result<Vec<String>, WriteConfigError> {
    game_config_install::write_config(
        &search(),
        root,
        game_directory,
        game_id,
        engine,
        &changes,
        None,
    )
    .map(|written| written.written)
}

fn read_cfg(
    root: &Path,
    game_directory: &str,
    game_id: &str,
    engine: &str,
) -> game_config_install::ReadConfig {
    game_config_install::read_config(&search(), root, game_directory, game_id, engine)
        .unwrap()
        .expect("игра поддерживается")
}

fn config_code(error: WriteConfigError) -> (ConfigCode, Option<String>) {
    match error {
        WriteConfigError::Config(error) => (error.code, error.cvar),
        other => panic!("ожидался отказ редактора, а не {other}"),
    }
}

fn field(cvar: &str) -> &'static game_config::Field {
    game_config::FIELDS
        .iter()
        .find(|f| f.cvar == cvar)
        .unwrap_or_else(|| panic!("нет настройки {cvar}"))
}

// ======================================================= game-config-selftest.js

#[test]
fn value_is_taken_from_the_file_where_the_engine_sets_it_last() {
    let (_dir, root) = temp_install(&[
        ("hl2/cfg/config.cfg", "sensitivity \"2\"\nvolume \"0.5\"\n"),
        ("hl2/cfg/autoexec.cfg", "exec mine\n"),
        ("hl2/cfg/mine.cfg", "sensitivity 4.5 // моя сенса\n"),
    ]);
    let result = read_cfg(&root, SOURCE, "hl2-4260", "source");
    assert_eq!(result.values.get("sensitivity").unwrap(), "4.5");
    assert_eq!(result.sources.get("sensitivity").unwrap(), "mine.cfg");
    assert_eq!(result.values.get("volume").unwrap(), "0.5");
}

#[test]
fn edit_stays_on_its_line_indent_quotes_and_comment_do_not_change() {
    let (_dir, root) = temp_install(&[
        (
            "hl2/cfg/config.cfg",
            "unbindall\r\nbind \"w\" \"+forward\"\r\nsensitivity \"2\"\r\n",
        ),
        (
            "hl2/cfg/autoexec.cfg",
            "  sensitivity 4.5 // моя сенса\nfps_max 300; volume 0.4\n",
        ),
    ]);
    write_cfg(
        &root,
        SOURCE,
        "hl2-4260",
        "source",
        json!({"sensitivity": 2.25, "volume": 70}),
    )
    .unwrap();
    assert_eq!(
        read_text(&root, "hl2/cfg/autoexec.cfg"),
        "  sensitivity 2.25 // моя сенса\nfps_max 300; volume 0.7\n"
    );
    assert_eq!(
        read_text(&root, "hl2/cfg/config.cfg"),
        "unbindall\r\nbind \"w\" \"+forward\"\r\nsensitivity \"2\"\r\n"
    );
}

#[test]
fn bad_value_and_foreign_variable_are_refused_files_do_not_change() {
    let (_dir, root) = temp_install(&[("hl2/cfg/config.cfg", "sensitivity \"2\"\n")]);
    let refused = |changes: Value| {
        config_code(write_cfg(&root, SOURCE, "hl2-4260", "source", changes).unwrap_err())
    };
    assert_eq!(
        refused(json!({"sensitivity": 500})).0,
        ConfigCode::InvalidValue
    );
    assert_eq!(
        refused(json!({"sv_cheats": 1})).0,
        ConfigCode::UnknownSetting
    );
    assert_eq!(
        refused(json!({"hud_fastswitch": "1; quit"})).0,
        ConfigCode::InvalidValue
    );
    assert_eq!(
        read_text(&root, "hl2/cfg/config.cfg"),
        "sensitivity \"2\"\n"
    );
}

#[test]
fn hl1_only_what_the_won_rules_allow() {
    let catalog = game_config::catalog_for("hl1-won", "goldsrc").unwrap();
    assert!(
        catalog.by_cvar("brightness").is_none(),
        "brightness запрещён белым списком команд"
    );
    let fps_max = catalog.by_cvar("fps_max").unwrap();
    assert_eq!(fps_max.max, Some(100.0), "fps_max не выше 100");
    assert_eq!(game_config::to_cfg(fps_max, &json!(101), None), None);
}

/// Персонализация перекрывает свои переменные: в редакторе их быть не должно.
#[test]
fn editor_does_not_show_personalization_variables() {
    use std::collections::HashSet;
    let cvars_of = |text: &str| -> HashSet<String> {
        text.split('\n')
            .map(|line| line.trim_end_matches('\r'))
            .flat_map(game_config::line_commands)
            .map(|command| command.args[0].value.to_lowercase())
            .collect()
    };
    let all = ghosting_settings::defaults().to_value();
    let ghosting = cvars_of(&format!(
        "{}\n{}",
        ghosting_personalization::to_cfg_lines(&json!({"extra": all})),
        ghosting_settings::serialize_extras(&all)
    ));
    let goldsrc = cvars_of(&goldsrc_personalization::to_cfg_lines(
        &json!({"bxt": goldsrc_settings::defaults().to_value()}),
    ));

    for field in game_config::catalog_for("ghosting-renewed", "source")
        .unwrap()
        .fields
    {
        assert!(
            !ghosting.contains(&field.cvar.to_lowercase()),
            "{}",
            field.cvar
        );
    }
    for field in game_config::catalog_for("hl1-won", "goldsrc")
        .unwrap()
        .fields
    {
        assert!(
            !goldsrc.contains(&field.cvar.to_lowercase()),
            "{}",
            field.cvar
        );
    }
}

#[test]
fn every_setting_has_labels_in_both_languages_and_right_limits() {
    for field in game_config::FIELDS {
        assert!(
            !field.label.ru.is_empty() && !field.label.en.is_empty(),
            "{}",
            field.cvar
        );
        if field.kind == FieldType::Range {
            let default = match game_config::default_value(field) {
                WindowValue::Num(n) => n,
                WindowValue::Bool(_) => panic!("{}", field.cvar),
            };
            assert!(
                field.min.unwrap() <= default && default <= field.max.unwrap(),
                "{}",
                field.cvar
            );
        }
        let window = match game_config::default_value(field) {
            WindowValue::Bool(b) => json!(b),
            WindowValue::Num(n) => js_number(n),
        };
        assert!(
            game_config::to_cfg(field, &window, None).is_some(),
            "{}",
            field.cvar
        );
    }
}

// 0.4.9: правки «Конфигов» идут в раздел settings файла игрока (game-config-selftest.js 0.4.9).

#[test]
fn variables_set_nowhere_go_to_the_settings_section_of_the_player_file() {
    let (_dir, root) = temp_install(&[
        ("hl2/cfg/config.cfg", "sensitivity \"2\"\r\n"),
        ("hl2/cfg/autoexec.cfg", ""),
    ]);
    let changes = json!({"m_filter": true, "cl_showfps": 2, "m_pitch": true});
    write_cfg(&root, SOURCE, "hl2-4260", "source", changes).unwrap();
    assert_eq!(
        read_text(&root, "hl2/cfg/config.cfg"),
        "sensitivity \"2\"\r\n"
    );
    assert_eq!(
        read_text(&root, "hl2/cfg/autoexec.cfg"),
        "// ===== HLSR: settings =====\r\nm_filter \"1\"\r\ncl_showfps \"2\"\r\nm_pitch \"-0.022\"\r\n"
    );
    assert!(
        !path_in(&root, "hl2/cfg/hlsr-config.cfg").exists(),
        "hlsr-config.cfg не создаётся"
    );
    // Второй раз значение правится на месте, раздел один.
    write_cfg(
        &root,
        SOURCE,
        "hl2-4260",
        "source",
        json!({"cl_showfps": 0}),
    )
    .unwrap();
    assert_eq!(
        read_text(&root, "hl2/cfg/autoexec.cfg"),
        "// ===== HLSR: settings =====\r\nm_filter \"1\"\r\ncl_showfps \"0\"\r\nm_pitch \"-0.022\"\r\n"
    );
}

#[test]
fn variable_only_in_config_cfg_is_written_to_the_player_file() {
    let (_dir, root) = temp_install(&[
        ("hl2/cfg/config.cfg", "sensitivity \"2\"\n"),
        ("hl2/cfg/autoexec.cfg", "cl_showpos 1\n"),
    ]);
    write_cfg(
        &root,
        SOURCE,
        "hl2-4260",
        "source",
        json!({"sensitivity": 3}),
    )
    .unwrap();
    assert_eq!(
        read_text(&root, "hl2/cfg/config.cfg"),
        "sensitivity \"2\"\n"
    );
    assert_eq!(
        read_text(&root, "hl2/cfg/autoexec.cfg"),
        "cl_showpos 1\n\n// ===== HLSR: settings =====\nsensitivity \"3\"\n"
    );
    let read = read_cfg(&root, SOURCE, "hl2-4260", "source");
    assert_eq!(read.values.get("sensitivity").unwrap(), "3");
}

#[test]
fn inversion_keeps_the_magnitude_of_m_pitch() {
    let (_dir, root) = temp_install(&[
        ("hl2/cfg/config.cfg", "m_pitch \"0.03\"\n"),
        ("hl2/cfg/autoexec.cfg", ""),
    ]);
    write_cfg(
        &root,
        SOURCE,
        "hl2-4260",
        "source",
        json!({"m_pitch": true}),
    )
    .unwrap();
    assert_eq!(read_text(&root, "hl2/cfg/config.cfg"), "m_pitch \"0.03\"\n");
    assert!(read_text(&root, "hl2/cfg/autoexec.cfg").contains("m_pitch \"-0.03\""));
    assert_eq!(
        game_config::from_cfg(field("m_pitch"), Some("-0.03")),
        Some(WindowValue::Bool(true))
    );
}

#[test]
fn goldsrc_files_are_latin1_and_crlf_edits_go_to_userconfig() {
    let (_dir, root) = temp_install(&[
        ("Half-Life/valve_WON/config.cfg", "sensitivity \"3\"\r\n"),
        (
            "Half-Life/valve_WON/userconfig.cfg",
            "// café\r\nexec hlsr-personalization.cfg\r\n",
        ),
    ]);
    let changes = json!({"sensitivity": 1.5, "fps_max": 60, "net_graph": 3});
    write_cfg(&root, GOLDSRC, "hl1-won", "goldsrc", changes).unwrap();
    assert_eq!(
        read_latin1(&root, "Half-Life/valve_WON/config.cfg"),
        "sensitivity \"3\"\r\n"
    );
    assert_eq!(
        read_latin1(&root, "Half-Life/valve_WON/userconfig.cfg"),
        "// café\r\nexec hlsr-personalization.cfg\r\n\r\n// ===== HLSR: settings =====\r\nsensitivity \"1.5\"\r\nfps_max \"60\"\r\nnet_graph \"3\"\r\n"
    );
}

#[test]
fn file_name_case_on_disk_is_kept() {
    let (_dir, root) = temp_install(&[("hl2/cfg/AutoExec.cfg", "Sensitivity \"2\"\n")]);
    write_cfg(
        &root,
        SOURCE,
        "hl2-4260",
        "source",
        json!({"sensitivity": 3}),
    )
    .unwrap();
    assert_eq!(
        read_text(&root, "hl2/cfg/AutoExec.cfg"),
        "Sensitivity \"3\"\n"
    );
    let names: Vec<String> = std::fs::read_dir(path_in(&root, "hl2/cfg"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["AutoExec.cfg"]);
}

#[test]
fn hl1_won_cfg_is_not_edited_its_value_is_overridden_in_the_player_file() {
    let won = "// Don't mess with this (your run won't be legit otherwise)!\r\n\r\nclockwindow 0\r\nfps_max 100\r\n";
    let (_dir, root) = temp_install(&[
        (
            "Half-Life/valve_WON/config.cfg",
            "fps_max \"100\"\r\nexec userconfig.cfg\r\n",
        ),
        (
            "Half-Life/valve_WON/userconfig.cfg",
            "exec WON.cfg\r\nexec hlsr-personalization.cfg\r\n",
        ),
        ("Half-Life/valve_WON/WON.cfg", won),
    ]);
    let before = read_cfg(&root, GOLDSRC, "hl1-won", "goldsrc");
    assert_eq!(before.sources.get("fps_max").unwrap(), "WON.cfg");
    write_cfg(&root, GOLDSRC, "hl1-won", "goldsrc", json!({"fps_max": 72})).unwrap();
    assert_eq!(read_latin1(&root, "Half-Life/valve_WON/WON.cfg"), won);
    assert!(!path_in(&root, "Half-Life/valve_WON/hlsr-config.cfg").exists());
    assert_eq!(
        read_latin1(&root, "Half-Life/valve_WON/userconfig.cfg"),
        "exec WON.cfg\r\nexec hlsr-personalization.cfg\r\n\r\n// ===== HLSR: settings =====\r\nfps_max \"72\"\r\n"
    );
    let after = read_cfg(&root, GOLDSRC, "hl1-won", "goldsrc");
    assert_eq!(after.values.get("fps_max").unwrap(), "72");
    assert_eq!(after.sources.get("fps_max").unwrap(), "userconfig.cfg");
}

#[test]
fn stale_expected_sha256_gives_config_changed_and_keeps_the_file() {
    let (_dir, root) = temp_install(&[("hl2/cfg/autoexec.cfg", "fps_max 300\n")]);
    let seen = read_cfg(&root, SOURCE, "hl2-4260", "source");
    let sha = seen.sha256.clone().unwrap();
    assert_eq!(sha.len(), 64);
    assert_eq!(seen.player_file, "autoexec.cfg");
    put(&root, "hl2/cfg/autoexec.cfg", b"fps_max 300\nr_lod 1\n");
    let write = |expected: &str| {
        game_config_install::write_config(
            &search(),
            &root,
            SOURCE,
            "hl2-4260",
            "source",
            &json!({"fps_max": 200}),
            Some(Some(expected)),
        )
    };
    match write(&sha) {
        Err(WriteConfigError::Player(error)) => assert!(error.is("CONFIG_CHANGED")),
        other => panic!("ожидался CONFIG_CHANGED: {other:?}"),
    }
    assert_eq!(
        read_text(&root, "hl2/cfg/autoexec.cfg"),
        "fps_max 300\nr_lod 1\n"
    );
    let fresh = read_cfg(&root, SOURCE, "hl2-4260", "source");
    write(fresh.sha256.as_deref().unwrap()).unwrap();
    assert_eq!(
        read_text(&root, "hl2/cfg/autoexec.cfg"),
        "fps_max 200\nr_lod 1\n"
    );
}

#[test]
fn variable_set_twice_in_the_player_file_is_shadowed() {
    let (_dir, root) = temp_install(&[(
        "hl2/cfg/autoexec.cfg",
        "fov 1\nfov 2\nfps_max 300\nvolume 0.5; volume 0.6\n",
    )]);
    assert_eq!(
        read_cfg(&root, SOURCE, "hl2-4260", "source").shadowed,
        ["fov", "volume"]
    );
    let (_single, single) = temp_install(&[("hl2/cfg/autoexec.cfg", "fov 1\n")]);
    assert!(
        read_cfg(&single, SOURCE, "hl2-4260", "source")
            .shadowed
            .is_empty()
    );
}

#[test]
fn source_fps_max_is_at_most_999() {
    let catalog = game_config::catalog_for("hl2-4260", "source").unwrap();
    let fps_max = catalog.by_cvar("fps_max").unwrap();
    assert_eq!(fps_max.max, Some(999.0));
    assert_eq!(game_config::to_cfg(fps_max, &json!(1000), None), None);
    assert_eq!(
        game_config::to_cfg(fps_max, &json!(999), None).as_deref(),
        Some("999")
    );
    let hl1 = game_config::catalog_for("hl1-won", "goldsrc").unwrap();
    assert_eq!(hl1.by_cvar("fps_max").unwrap().max, Some(100.0));
}

#[test]
fn values_from_personalization_files_are_not_edited_in_place() {
    let (_dir, root) = temp_install(&[
        ("ghostingmod/cfg/config.cfg", ""),
        (
            "ghostingmod/cfg/autoexec.cfg",
            "exec hlsr-personalization.cfg\n",
        ),
        ("ghostingmod/cfg/hlsr-personalization.cfg", "volume 0.1\n"),
    ]);
    write_cfg(
        &root,
        GHOSTING,
        "ghosting-renewed",
        "source",
        json!({"volume": 50}),
    )
    .unwrap();
    assert_eq!(
        read_text(&root, "ghostingmod/cfg/hlsr-personalization.cfg"),
        "volume 0.1\n"
    );
    assert_eq!(read_text(&root, "ghostingmod/cfg/config.cfg"), "");
    assert_eq!(
        read_text(&root, "ghostingmod/cfg/autoexec.cfg"),
        "exec hlsr-personalization.cfg\n\n// ===== HLSR: settings =====\nvolume \"0.5\"\n"
    );
}

/// Steam на русском: GoldSrc ставит valve_WON_russian перед valve_WON и пишет туда config.cfg.
/// Лаунчер читает тот config.cfg, что видит игра, а файл игрока кладёт рядом с ним.
#[test]
fn goldsrc_config_of_the_language_folder_wins_while_the_game_writes_there() {
    let (_dir, root) = temp_install(&[
        (
            "Half-Life/valve_WON/config.cfg",
            "sensitivity \"3\"\r\nbind \"q\" \"+jump\"\r\nm_pitch \"0.022\"\r\n",
        ),
        (
            "Half-Life/valve_WON_russian/config.cfg",
            "sensitivity \"2\"\r\nbind \"q\" \"+duck\"\r\nm_pitch \"-0.022\"\r\n",
        ),
    ]);
    let now = SystemTime::now();
    let seconds = |time: SystemTime| time.duration_since(UNIX_EPOCH).unwrap().as_secs_f64();
    let old = seconds(now - Duration::from_secs(60));
    set_mtime(&path_in(&root, "Half-Life/valve_WON/config.cfg"), old);

    let read = read_cfg(&root, GOLDSRC, "hl1-won", "goldsrc");
    assert_eq!(read.values.get("sensitivity").unwrap(), "2");
    let binds = game_binds_install::read_install_binds(&search(), &root, GOLDSRC, "goldsrc");
    assert_eq!(binds.binds.get("q").unwrap().command, "+duck");

    write_cfg(
        &root,
        GOLDSRC,
        "hl1-won",
        "goldsrc",
        json!({"sensitivity": 1.5}),
    )
    .unwrap();
    assert!(
        read_latin1(&root, "Half-Life/valve_WON_russian/userconfig.cfg")
            .contains("sensitivity \"1.5\"\r\n")
    );
    assert!(
        read_latin1(&root, "Half-Life/valve_WON_russian/config.cfg")
            .starts_with("sensitivity \"2\"\r\n")
    );
    assert!(
        read_latin1(&root, "Half-Life/valve_WON/config.cfg").starts_with("sensitivity \"3\"\r\n")
    );

    // Файл игрока лежит там же, где игра держит config.cfg. Новых файлов лаунчера нет.
    let search = search();
    let ctx = hlsr_delivery::player_config::Ctx {
        search: &search,
        install: &root,
        game_id: "hl1-won",
        game_directory: GOLDSRC,
        platform: hlsr_core::js::Platform::Win32,
    };
    // В 0.4.9 `goldsrcSettings.defaults().bxt` — undefined: прежние BXT — умолчания.
    let was = json!({"personalization": {}});
    let next = json!({"personalization": {}, "bxt": {"fov": 100}});
    let result =
        hlsr_delivery::player_config::write_personalization(&ctx, &was, &next, None).unwrap();
    assert_eq!(result.changed, ["default_fov"]);
    assert_eq!(
        result.file,
        path_in(&root, "Half-Life/valve_WON_russian/userconfig.cfg")
    );
    assert!(
        read_latin1(&root, "Half-Life/valve_WON_russian/userconfig.cfg")
            .contains("default_fov \"100\"")
    );
    assert!(!path_in(&root, "Half-Life/valve_WON/userconfig.cfg").exists());
    assert!(!path_in(&root, "Half-Life/valve_WON/hlsr-personalization.cfg").exists());

    // Steam снова на английском: игра пишет config.cfg в valve_WON.
    set_mtime(
        &path_in(&root, "Half-Life/valve_WON_russian/config.cfg"),
        old,
    );
    set_mtime(
        &path_in(&root, "Half-Life/valve_WON/config.cfg"),
        seconds(SystemTime::now()),
    );
    let read = read_cfg(&root, GOLDSRC, "hl1-won", "goldsrc");
    assert_eq!(read.values.get("sensitivity").unwrap(), "3");
}
fn files_of(list: &[(&str, &str, &str)]) -> ImportFiles {
    let mut files = ImportFiles::new();
    for (key, name, text) in list {
        files.insert(
            *key,
            ImportFile {
                name: (*name).to_owned(),
                text: (*text).to_owned(),
                target: None,
            },
        );
    }
    files
}

#[test]
fn import_into_the_tab_last_value_exec_chain_clamping_and_bad_values() {
    let catalog = game_config::catalog_for("hl1-won", "goldsrc").unwrap();
    let built = cfg_import::build_import(
        "goldsrc",
        &Root {
            name: "my.cfg".to_owned(),
            text: "sensitivity 2\r\nexec \"mouse.cfg\"\r\nvolume \"abc\"\r\nm_pitch \"-0.022\" // invert\r\nunknown_cvar 5\r\n".to_owned(),
            virtual_root: false,
        },
        &files_of(&[(
            "mouse.cfg",
            "mouse.cfg",
            "sensitivity 99; alias x \"sensitivity 1\"\n",
        )]),
        &[],
    );
    let imported = game_config::import_values(&catalog, &built.flat_text);
    let value = |cvar: &str| {
        imported
            .values
            .iter()
            .find(|(name, _)| *name == cvar)
            .map(|(_, v)| *v)
    };
    assert_eq!(
        value("sensitivity"),
        Some(WindowValue::Num(10.0)),
        "значение из файла цепочки, прижатое к краю окна; alias не выполняется"
    );
    assert_eq!(
        value("m_pitch"),
        Some(WindowValue::Bool(true)),
        "отрицательный m_pitch — инверсия"
    );
    assert_eq!(imported.skipped, ["volume"]);
    assert!(value("unknown_cvar").is_none());
}

#[test]
fn several_picked_cfgs_import_in_order_like_an_exec_chain() {
    let picked = vec![
        Picked {
            name: "config.cfg".to_owned(),
            text: "sensitivity 2\nexec extra.cfg\n".to_owned(),
        },
        Picked {
            name: "My Binds.cfg".to_owned(),
            text: "sensitivity 4\nbind f \"+use\"\n".to_owned(),
        },
    ];
    let result = cfg_import::root_of_picked(&picked);
    assert_eq!(result.files.len(), 2);
    // Выбранные файлы уже есть: окно выбора просится только для extra.cfg.
    let want = cfg_import::next_wanted("goldsrc", &result.root, &result.files, &[], true).unwrap();
    assert_eq!(want.name, "extra.cfg");
    let built = cfg_import::build_import("goldsrc", &result.root, &result.files, &[want.key]);
    let imported = game_config::import_values(
        &game_config::catalog_for("hl1-won", "goldsrc").unwrap(),
        &built.flat_text,
    );
    assert_eq!(
        imported
            .values
            .iter()
            .find(|(name, _)| *name == "sensitivity")
            .map(|(_, v)| *v),
        Some(WindowValue::Num(4.0))
    );
    // Один файл — без обёртки, как раньше.
    assert!(cfg_import::root_of_picked(&picked[..1]).files.is_empty());
}

// ======================================================= cfg-files-selftest.js

/// `makeGame`.
fn make_game() -> (tempfile::TempDir, PathBuf) {
    let (dir, root) = temp();
    put(&root, "valve/config.cfg", b"bind w +forward\r\n");
    put(&root, "valve/autoexec.cfg", "// мой автоэкзек\n".as_bytes());
    put(
        &root,
        "valve/scripts/jump.cfg",
        &[0x2f, 0x2f, 0x20, 0xcf, 0xf0, 0xe8, 0x0a], // cp1251
    );
    put(&root, "valve/maps/skip.cfg", b"x");
    put(&root, "valve/readme.txt", b"x");
    (dir, root)
}

fn refused(error: CfgFilesError) -> CfgFileCode {
    match error {
        CfgFilesError::Cfg(error) => error.code,
        other => panic!("ожидался отказ редактора, а не {other}"),
    }
}

#[test]
fn cfg_files_list_has_all_cfgs_without_resource_folders_and_non_cfgs() {
    let (_dir, root) = make_game();
    let listing = cfg_files::list_in(&root, "en-US");
    let paths: Vec<&str> = listing.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "valve/autoexec.cfg",
            "valve/config.cfg",
            "valve/scripts/jump.cfg"
        ]
    );
}

#[test]
fn cfg_files_read_and_write_keep_crlf_and_non_utf8_bytes() {
    let (_dir, root) = make_game();
    let config = cfg_files::read(&root, "valve/config.cfg").unwrap();
    assert_eq!(config.content, "bind w +forward\n");
    assert_eq!(config.eol, cfg_files::Eol::Crlf);
    cfg_files::write(
        &root,
        "valve/config.cfg",
        &format!("{}bind s +back\n", config.content),
        &WriteOptions {
            encoding: config.encoding,
            crlf: true,
            expected_mtime: Some(config.mtime),
        },
    )
    .unwrap();
    assert_eq!(
        read_text(&root, "valve/config.cfg"),
        "bind w +forward\r\nbind s +back\r\n"
    );

    let legacy = cfg_files::read(&root, "valve/scripts/jump.cfg").unwrap();
    assert_eq!(legacy.encoding, Encoding::Latin1);
    cfg_files::write(
        &root,
        "valve/scripts/jump.cfg",
        &format!("{}alias j +jump\n", legacy.content),
        &WriteOptions {
            encoding: legacy.encoding,
            crlf: legacy.eol == cfg_files::Eol::Crlf,
            expected_mtime: None,
        },
    )
    .unwrap();
    let bytes = std::fs::read(path_in(&root, "valve/scripts/jump.cfg")).unwrap();
    assert_eq!(
        bytes[..7],
        [0x2f, 0x2f, 0x20, 0xcf, 0xf0, 0xe8, 0x0a],
        "старые байты на месте"
    );
    let error = cfg_files::write(
        &root,
        "valve/scripts/jump.cfg",
        "// кириллица",
        &WriteOptions {
            encoding: Encoding::Latin1,
            ..WriteOptions::default()
        },
    )
    .unwrap_err();
    assert_eq!(refused(error), CfgFileCode::Encoding);
}

#[test]
fn cfg_files_a_file_changed_after_opening_is_not_overwritten() {
    let (_dir, root) = make_game();
    let opened = cfg_files::read(&root, "valve/autoexec.cfg").unwrap();
    let file = path_in(&root, "valve/autoexec.cfg");
    std::fs::write(&file, "// игра переписала\n").unwrap();
    let later = SystemTime::now() + Duration::from_secs(5);
    set_mtime(
        &file,
        later.duration_since(UNIX_EPOCH).unwrap().as_secs_f64(),
    );
    let error = cfg_files::write(
        &root,
        "valve/autoexec.cfg",
        "x",
        &WriteOptions {
            expected_mtime: Some(opened.mtime),
            ..WriteOptions::default()
        },
    )
    .unwrap_err();
    assert_eq!(refused(error), CfgFileCode::Changed);
}

#[test]
fn cfg_files_path_outside_not_cfg_and_symlink_outside_are_refused() {
    let (_dir, root) = make_game();
    for bad in [
        "../escape.cfg",
        "valve/../../x.cfg",
        "valve/readme.txt",
        "/etc/passwd",
        "",
    ] {
        assert_eq!(
            refused(cfg_files::read(&root, bad).unwrap_err()),
            CfgFileCode::PathInvalid,
            "{bad}"
        );
    }
    #[cfg(unix)]
    {
        let (_outside_dir, outside) = temp();
        put(&outside, "secret.cfg", b"x");
        std::os::unix::fs::symlink(outside.join("secret.cfg"), path_in(&root, "valve/link.cfg"))
            .unwrap();
        assert_eq!(
            refused(cfg_files::read(&root, "valve/link.cfg").unwrap_err()),
            CfgFileCode::PathInvalid
        );
        assert_eq!(
            refused(
                cfg_files::write(&root, "valve/link.cfg", "y", &WriteOptions::default())
                    .unwrap_err()
            ),
            CfgFileCode::PathInvalid
        );
        assert_eq!(read_text(&outside, "secret.cfg"), "x");
    }
}

// ======================================================= build-transfer-selftest.js

fn sha(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Раскладка самотеста: `makeGame` переноса и `files.json` прежней версии.
fn make_transfer_game() -> (tempfile::TempDir, PathBuf, Value) {
    let (dir, root) = temp();
    put(&root, "valve/config.cfg", b"stock\n");
    put(&root, "valve/autoexec.cfg", "мой autoexec\n".as_bytes());
    put(&root, "valve/userconfig.cfg", b"stock user\n");
    put(&root, "valve/scripts/jump.cfg", b"alias j +jump\n");
    let previous = json!({"files": [
        {"path": "valve/config.cfg", "sha256": sha("stock\n")},
        {"path": "valve/autoexec.cfg", "sha256": sha("stock autoexec\n")},
        {"path": "valve/userconfig.cfg", "sha256": sha("stock user\n")},
    ]});
    (dir, root, previous)
}

#[test]
fn transfer_exports_only_edited_and_own_cfgs_import_returns_them() {
    let (_dir, root, previous) = make_transfer_game();
    let files = build_transfer::collect_in(&root, Some(&previous), "en-US").unwrap();
    let mut paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, ["valve/autoexec.cfg", "valve/scripts/jump.cfg"]);

    let settings = json!({"fullscreen": true});
    let text = build_transfer::serialize(&Build {
        launcher_id: "hl1",
        game_id: "hl1-won",
        build: None,
        version: Some("1.0.0"),
        launcher_version: None,
        settings: Some(&settings),
        files: &files,
        exported_at: None,
    });
    let known = |id: &str| id == "hl1-won";
    let parsed = build_transfer::parse(&text, Some(&known)).unwrap();
    assert_eq!(parsed.game_id, "hl1-won");
    assert_eq!(parsed.settings, Some(json!({"fullscreen": true})));

    let (_target_dir, target) = temp();
    std::fs::create_dir(target.join("valve")).unwrap();
    build_transfer::apply_files(&target, &parsed.files).unwrap();
    assert_eq!(read_text(&target, "valve/autoexec.cfg"), "мой autoexec\n");
    assert_eq!(
        read_text(&target, "valve/scripts/jump.cfg"),
        "alias j +jump\n"
    );
}

#[test]
fn transfer_without_files_json_steam_build_takes_all_cfgs() {
    let (_dir, root, _previous) = make_transfer_game();
    assert_eq!(
        build_transfer::collect_in(&root, None, "en-US")
            .unwrap()
            .len(),
        4
    );
}

fn transfer_code(result: Result<build_transfer::Parsed, TransferFailure>) -> TransferCode {
    match result.unwrap_err() {
        TransferFailure::Transfer(error) => error.code,
        other => panic!("ожидался отказ переноса, а не {other}"),
    }
}

#[test]
fn transfer_refuses_foreign_format_unknown_game_corruption_and_leaving_the_folder() {
    let known = |id: &str| id == "hl1-won";
    let parse = |text: &str| build_transfer::parse(text, Some(&known));
    let document = |files: Value| {
        json!({"format": "hlsr-build", "version": 1, "game": {"gameId": "hl1-won"}, "files": files})
            .to_string()
    };
    assert_eq!(transfer_code(parse("{}")), TransferCode::BadFormat);
    assert_eq!(transfer_code(parse("not json")), TransferCode::BadFormat);
    assert_eq!(
        transfer_code(parse(
            &json!({"format": "hlsr-build", "version": 99, "game": {"gameId": "hl1-won"}, "files": []})
                .to_string()
        )),
        TransferCode::NewerFormat
    );
    assert_eq!(
        transfer_code(parse(
            &json!({"format": "hlsr-build", "version": 1, "game": {"gameId": "evil"}, "files": []})
                .to_string()
        )),
        TransferCode::UnknownGame
    );
    let data = "eA=="; // base64 от "x"
    for bad in [
        "../x.cfg",
        "valve/../../x.cfg",
        "/etc/x.cfg",
        "C:/x.cfg",
        "valve\\x.cfg",
        "valve/x.exe",
        "valve//x.cfg",
    ] {
        assert_eq!(
            transfer_code(parse(&document(json!([{"path": bad, "data": data}])))),
            TransferCode::BadFormat,
            "{bad}"
        );
    }
    assert_eq!(
        transfer_code(parse(&document(
            json!([{"path": "valve/a.cfg", "data": data, "sha256": "0".repeat(64)}])
        ))),
        TransferCode::Corrupt
    );
}

#[test]
fn transfer_import_does_not_write_through_a_symlink_outside() {
    #[cfg(unix)]
    {
        use hlsr_delivery::build_transfer::PickedFile;
        let (_target_dir, target) = temp();
        let (_outside_dir, outside) = temp();
        std::os::unix::fs::symlink(&outside, target.join("valve")).unwrap();
        let error = build_transfer::apply_files(
            &target,
            &[PickedFile {
                path: "valve/autoexec.cfg".to_owned(),
                data: b"x".to_vec(),
            }],
        )
        .unwrap_err();
        match error {
            TransferFailure::Transfer(error) => assert_eq!(error.code, TransferCode::PathInvalid),
            other => panic!("{other}"),
        }
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    }
}
