//! Самотесты 0.4.7 об импорте cfg, поиске cfg, записях и установке биндов с теми же случаями:
//! `scripts/hl1-selftest.js` (бинды, гайд, импорт), `scripts/game-config-selftest.js` (языковые
//! папки, импорт во вкладку), `scripts/ghost-runs-selftest.js` (удаление записей).
//!
//! Не здесь: части, которым нужен фасад доставки (задача 22: `deleteRun` при запущенной игре).
//! Они переносятся вместе с фасадом.

use hlsr_delivery::cfg_import::{self, Built, ImportFile, ImportFiles, Picked, Root};
use hlsr_delivery::cfg_search::{CfgSearch, LanguageOption, SteamLanguage};
use hlsr_delivery::game_binds::{self, Bind, BindRead};
use hlsr_delivery::game_binds_install::{self, IMPORT_TRAILER_MARK, ImportSource, InstallImport};
use hlsr_delivery::ghosting_personalization;
use hlsr_delivery::goldsrc_import_adopt;
use hlsr_delivery::goldsrc_personalization;
use hlsr_delivery::goldsrc_settings;
use hlsr_delivery::hl1_guide;
use hlsr_delivery::launch_args;
use hlsr_delivery::manifests;
use hlsr_delivery::mutable_paths;
use hlsr_delivery::ports::FakeRegistry;
use hlsr_delivery::runs_scan::{self, DeleteResult, ScanResult};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const GAME_ID: &str = "hl1-won";
const MOD: &str = "Half-Life/valve_WON";

fn search() -> CfgSearch {
    CfgSearch::new(SteamLanguage::new(Arc::new(FakeRegistry::default())))
}

fn temp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn root_of(dir: &tempfile::TempDir) -> PathBuf {
    std::fs::canonicalize(dir.path()).unwrap()
}

fn write(root: &Path, relative: &str, content: &str) {
    let file = relative
        .split('/')
        .fold(root.to_path_buf(), |acc, part| acc.join(part));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, content).unwrap();
}

fn write_latin1(root: &Path, relative: &str, content: &str) {
    let file = relative
        .split('/')
        .fold(root.to_path_buf(), |acc, part| acc.join(part));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, hlsr_core::js::latin1_bytes(content)).unwrap();
}

fn read_utf8(root: &Path, relative: &str) -> String {
    let file = relative
        .split('/')
        .fold(root.to_path_buf(), |acc, part| acc.join(part));
    String::from_utf8(std::fs::read(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display())))
        .unwrap()
}

fn read_latin1(root: &Path, relative: &str) -> String {
    let file = relative
        .split('/')
        .fold(root.to_path_buf(), |acc, part| acc.join(part));
    hlsr_core::js::latin1_string(
        &std::fs::read(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display())),
    )
}

fn exists(root: &Path, relative: &str) -> bool {
    relative
        .split('/')
        .fold(root.to_path_buf(), |acc, part| acc.join(part))
        .exists()
}

fn set_time(root: &Path, relative: &str, time: SystemTime) {
    let file = relative
        .split('/')
        .fold(root.to_path_buf(), |acc, part| acc.join(part));
    std::fs::OpenOptions::new()
        .write(true)
        .open(file)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(time))
        .unwrap();
}

/// `fakeInstall`: файлы установки HL1, которые лаунчер узнаёт.
fn fake_install(root: &Path) {
    write(root, "Half-Life/hl.exe", "MZ");
    write(
        root,
        "Half-Life/valve_WON/liblist.gam",
        "game \"Half-Life\"\n",
    );
    write(root, "Half-Life/steam_appid.txt", "220");
    write(root, "Bunnymod XT/Injector.exe", "MZ");
}

fn files_of(list: &[(&str, &str, &str)]) -> ImportFiles {
    list.iter()
        .map(|(key, name, text)| {
            (
                (*key).to_owned(),
                ImportFile {
                    name: (*name).to_owned(),
                    text: (*text).to_owned(),
                    target: None,
                },
            )
        })
        .collect()
}

fn root(name: &str, text: &str) -> Root {
    Root {
        name: name.to_owned(),
        text: text.to_owned(),
        virtual_root: false,
    }
}

fn picked(list: &[(&str, &str)]) -> Vec<Picked> {
    list.iter()
        .map(|(name, text)| Picked {
            name: (*name).to_owned(),
            text: (*text).to_owned(),
        })
        .collect()
}

fn text_of<'a>(built: &'a Built, file: &str) -> &'a str {
    &built.outputs.iter().find(|o| o.file == file).unwrap().text
}

/// `assert.match(text, /^…$/m)`: есть строка, равная `line` (с `\r` или без).
fn has_line(text: &str, line: &str) -> bool {
    text.split('\n')
        .any(|l| l.strip_suffix('\r').unwrap_or(l) == line)
}

fn overrides(list: &[(&str, &str, &str)]) -> Value {
    Value::Object(
        list.iter()
            .map(|(key, command, base)| {
                ((*key).to_owned(), json!({"command": command, "base": base}))
            })
            .collect(),
    )
}

// ============================================================ hl1-selftest.js

/// `goldsrcPersonalization.applyToInstall(root, LAUNCH_CONFIG["hl1-won"], settings)`.
fn apply_to_install(root: &Path, settings: &Value) {
    let config = manifests::launch_config_of(GAME_ID).unwrap();
    goldsrc_personalization::apply_to_install(&search(), root, config, settings).unwrap();
}

#[test]
fn hl1_binds_cfg_is_written_next_to_personalization_and_exec_added_once() {
    let dir = temp();
    let root = root_of(&dir);
    write_latin1(
        &root,
        "Half-Life/valve_WON/userconfig.cfg",
        "fps_max 100\r\n",
    );
    let settings = json!({"binds": overrides(&[("MOUSE4", "+duck", "")])});
    for _ in 0..2 {
        apply_to_install(&root, &settings);
    }
    let userconfig = read_latin1(&root, "Half-Life/valve_WON/userconfig.cfg");
    assert_eq!(
        userconfig.matches(game_binds::BINDS_EXEC_LINE).count(),
        1,
        "строка exec добавляется один раз"
    );
    assert!(
        userconfig.trim_end().ends_with(game_binds::BINDS_EXEC_LINE),
        "бинды лаунчера выполняются последними"
    );
    assert!(
        read_latin1(&root, "Half-Life/valve_WON/hlsr-binds.cfg")
            .contains("bind \"MOUSE4\" \"+duck\"")
    );
    assert!(mutable_paths::matches_mutable(
        "Half-Life/valve_WON/hlsr-binds.cfg"
    ));

    let read = game_binds_install::read_install_binds(&search(), &root, MOD, "goldsrc");
    assert!(
        read.binds.get("MOUSE4").is_none(),
        "свои бинды лаунчер в «бинды игры» не подмешивает"
    );
}

#[test]
fn script_preset_of_hl1_writes_its_aliases_before_binds_once() {
    let dir = temp();
    let root = root_of(&dir);
    let binds = overrides(&[
        ("f", "+fps_20_hold", ""),
        ("g", "+tau", ""),
        ("h", "tauswitch", ""),
    ]);
    apply_to_install(&root, &json!({"binds": binds}));
    let cfg = read_latin1(&root, "Half-Life/valve_WON/hlsr-binds.cfg");
    let lines: Vec<&str> = cfg.split("\r\n").collect();
    assert!(lines.contains(&"alias +fps_20_hold \"fps_max 20; alias -fps_20_hold fps_20_reset\""));
    assert!(
        lines.contains(&"alias _taubo"),
        "пустой alias пишется без кавычек, как в гайде"
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.starts_with("alias +tau "))
            .count(),
        1,
        "общие alias не повторяются"
    );
    let first = |prefix: &str| {
        lines
            .iter()
            .position(|line| line.starts_with(prefix))
            .unwrap()
    };
    assert!(first("alias") < first("bind"));

    let reset = overrides(&[("f", "", "+fps_20_hold")]);
    apply_to_install(&root, &json!({"binds": reset}));
    assert!(
        !read_latin1(&root, "Half-Life/valve_WON/hlsr-binds.cfg").contains("alias"),
        "снятый скрипт уходит из cfg"
    );
}

#[test]
fn guide_scripts_take_sensitivity_from_config_and_auto_alias_from_game_cfg() {
    let dir = temp();
    let root = root_of(&dir);
    write_latin1(
        &root,
        "Half-Life/valve_WON/config.cfg",
        "unbindall\r\nbind \"w\" \"+forward\"\r\nsensitivity \"2.25\"\r\n",
    );
    let command = |id: &str| {
        hl1_guide::script_by_id(id).unwrap().actions[0]
            .command
            .clone()
    };
    let (p2p, saveload) = (
        command("pipe2pipe-script-v2"),
        command("simple-saveload-script"),
    );
    let binds = overrides(&[("3", &p2p, ""), ("8", &saveload, "")]);
    apply_to_install(&root, &json!({"binds": binds}));
    let cfg = read_latin1(&root, "Half-Life/valve_WON/hlsr-binds.cfg");
    assert!(!cfg.contains("{sensitivity}"), "токен не попадает в игру");
    assert!(
        cfg.lines()
            .any(|l| l.starts_with("alias ") && l.contains("sens \"sensitivity 2.25\"")),
        "чувствительность игрока подставлена"
    );
    let lines: Vec<&str> = cfg.split("\r\n").collect();
    assert_eq!(
        lines
            .iter()
            .find(|line| line.starts_with("alias auto_alias")),
        Some(&"alias auto_alias"),
        "auto_alias пуст до нажатия скрипта"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("bind \"8\" \"alias auto_alias "))
    );
    assert_eq!(
        read_latin1(&root, "Half-Life/valve_WON/game.cfg"),
        "auto_alias\r\n"
    );

    // Повторная запись не дублирует строку; чужой game.cfg сохраняется.
    write_latin1(&root, "Half-Life/valve_WON/game.cfg", "echo hi\r\n");
    let again = overrides(&[("8", &saveload, "")]);
    for _ in 0..2 {
        apply_to_install(&root, &json!({"binds": again}));
    }
    assert_eq!(
        read_latin1(&root, "Half-Life/valve_WON/game.cfg"),
        "echo hi\r\nauto_alias\r\n"
    );

    // Мусор вместо числа — значение по умолчанию.
    write_latin1(
        &root,
        "Half-Life/valve_WON/config.cfg",
        "sensitivity \"2;quit\"\r\n",
    );
    let only_p2p = overrides(&[("3", &p2p, "")]);
    apply_to_install(&root, &json!({"binds": only_p2p}));
    let cfg = read_latin1(&root, "Half-Life/valve_WON/hlsr-binds.cfg");
    assert!(
        cfg.lines()
            .any(|l| l.starts_with("alias ") && l.contains("sens \"sensitivity 3\""))
    );
    assert_eq!(hl1_guide::clean_sensitivity(Some("0")), "3");
    assert_eq!(hl1_guide::clean_sensitivity(Some("1.5")), "1.5");
}

#[test]
fn guide_tricks_return_fov_crosshair_and_mouse_inversion_of_the_player() {
    let dir = temp();
    let root = root_of(&dir);
    write_latin1(
        &root,
        "Half-Life/valve_WON/config.cfg",
        "sensitivity \"2\"\r\nm_pitch \"-0.022\"\r\n",
    );
    let command = |id: &str| {
        hl1_guide::script_by_id(id).unwrap().actions[0]
            .command
            .clone()
    };
    let (gauss, semi, full) = (
        command("gauss-door-setup-script"),
        command("semi-manual-xen-script"),
        command("fully-automatic-xen-script"),
    );
    let binds = overrides(&[("3", &gauss, ""), ("4", &semi, ""), ("5", &full, "")]);
    apply_to_install(&root, &json!({"fov": 120, "binds": binds}));
    let cfg = read_latin1(&root, "Half-Life/valve_WON/hlsr-binds.cfg");
    assert!(
        !cfg.contains("{m_pitch}") && !cfg.contains("{sensitivity}"),
        "токены не попадают в игру"
    );
    assert!(
        !cfg.contains("default_fov 110") && !cfg.contains("bxt_cross 0\""),
        "значения автора гайда не возвращаются"
    );
    assert!(cfg.contains("alias deffov \"exec hlsr-personalization.cfg\""));
    assert!(cfg.contains("alias defcross \"exec hlsr-personalization.cfg\""));
    assert!(
        !cfg.contains("m_pitch 0.022"),
        "инверсия мыши не сбрасывается"
    );
    assert_eq!(
        cfg.matches("m_pitch -0.022").count(),
        2,
        "оба скрипта Xen возвращают m_pitch игрока"
    );
}

#[test]
fn source_binds_are_read_from_config_and_autoexec_and_binds_cfg_is_exec_d_from_autoexec() {
    let dir = temp();
    let root = root_of(&dir);
    write(
        &root,
        "ghostingmod/cfg/config.cfg",
        "unbindall\nbind \"w\" \"+forward\"\nbind \"NUMLOCK\" \"pause\"\nbind \"SEMICOLON\" \"+use\"\n",
    );
    write(
        &root,
        "ghostingmod/cfg/autoexec.cfg",
        "bind mouse4 \"save quick\" //Клавиша сейва\r\nbind x \"save quick; load quick\"\r\n",
    );
    let read = game_binds_install::read_install_binds(&search(), &root, "ghostingmod", "source");
    assert_eq!(read.binds.get("w").unwrap().command, "+forward");
    assert_eq!(
        read.binds.get("NUMLOCK").unwrap().command,
        "pause",
        "клавиши Source распознаются"
    );
    assert_eq!(read.binds.get(";").unwrap().command, "+use");
    assert_eq!(read.binds.get("MOUSE4").unwrap().source, "autoexec.cfg");
    assert_eq!(
        read.binds.get("x").unwrap().command,
        "save quick; load quick"
    );

    let binds = overrides(&[(";", "+jump", "+use"), ("RSHIFT", "+speed", "")]);
    for _ in 0..2 {
        game_binds_install::apply_binds(
            &search(),
            &root,
            "ghosting-renewed",
            "ghostingmod",
            &binds,
        )
        .unwrap();
    }
    let cfg = read_utf8(&root, "ghostingmod/cfg/hlsr-binds.cfg");
    assert!(
        cfg.contains("bind \"SEMICOLON\" \"+jump\""),
        "Source не понимает «;» в имени клавиши"
    );
    assert!(cfg.contains("bind \"RSHIFT\" \"+speed\""));
    let autoexec = read_utf8(&root, "ghostingmod/cfg/autoexec.cfg");
    assert_eq!(autoexec.matches("exec hlsr-binds.cfg").count(), 1);
    assert!(
        autoexec.ends_with("exec hlsr-binds.cfg\r\n"),
        "строка в конце, окончания строк как в файле"
    );
    let again = game_binds_install::read_install_binds(&search(), &root, "ghostingmod", "source");
    assert!(again.binds.get("RSHIFT").is_none());
}

const SAMPLE_CFG: &str = "name prolert\r\nalias \"_taubo\" \"\"\r\nalias \"+tau\" \"+attack2;alias _taubo _taubofunc\"\r\nbind \"mouse4\" \"save oarEnt; bind mouse4 +tau; speak fvox/beep; say TAO\"\r\nalias obboshoot \"+use;w 10;-use;+attack2;+jump;w;-attack2;-jump\"\r\nbind n \"obboshoot\"\r\nexec userconfig.cfg\r\nbind z \"fps_max 20\"\r\n";

#[test]
fn imported_config_is_connected_whole_before_the_launcher_files() {
    let dir = temp();
    let root = root_of(&dir);
    fake_install(&root);
    write_latin1(
        &root,
        "Half-Life/valve_WON/config.cfg",
        "unbindall\r\nbind \"w\" \"+forward\"\r\nbind \"n\" \"+use\"\r\nexec userconfig.cfg\r\n",
    );
    write_latin1(
        &root,
        "Half-Life/valve_WON/userconfig.cfg",
        "sensitivity 2\r\nexec hlsr-personalization.cfg\r\nexec hlsr-binds.cfg\r\n",
    );

    let result = game_binds_install::install_import(
        &search(),
        &root,
        GAME_ID,
        MOD,
        ImportSource::Text(SAMPLE_CFG),
        &[],
    )
    .unwrap();
    assert!(matches!(result, InstallImport::Ok { .. }));
    let userconfig = read_latin1(&root, "Half-Life/valve_WON/userconfig.cfg");
    let lines: Vec<&str> = userconfig.split("\r\n").collect();
    assert_eq!(
        &lines[1..4],
        [
            "exec \"player.cfg\"",
            "exec hlsr-personalization.cfg",
            "exec hlsr-binds.cfg"
        ]
    );
    let written = read_latin1(&root, "Half-Life/valve_WON/player.cfg");
    assert!(
        written
            .lines()
            .any(|line| line == "// HLSR: exec userconfig.cfg"),
        "exec userconfig.cfg из импорта зациклил бы выполнение"
    );

    let read = game_binds_install::read_install_binds(&search(), &root, MOD, "goldsrc");
    assert_eq!(
        read.binds.get("n").unwrap().command,
        "obboshoot",
        "бинд на alias из конфига не пропущен"
    );
    assert!(game_binds::is_import_source(
        &read.binds.get("n").unwrap().source,
        &["player.cfg".to_owned()]
    ));
    assert_eq!(
        read.binds.get("w").unwrap().command,
        "+forward",
        "бинды игры вне конфига остаются"
    );
    assert_eq!(
        read.aliases.get("obboshoot").unwrap(),
        "+use;w 10;-use;+attack2;+jump;w;-attack2;-jump"
    );

    // Предпросмотр до «Сохранить» совпадает с тем, что прочитается с диска.
    let before = BindRead {
        binds: [
            (
                "w",
                Bind {
                    command: "+forward".into(),
                    source: "config.cfg".into(),
                },
            ),
            (
                "n",
                Bind {
                    command: "+use".into(),
                    source: "config.cfg".into(),
                },
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect(),
        ..BindRead::default()
    };
    let preview = game_binds::with_import(Some(&before), SAMPLE_CFG);
    let sorted = |read: &BindRead| {
        let mut keys: Vec<String> = read.binds.keys().map(str::to_owned).collect();
        keys.sort();
        keys
    };
    assert_eq!(sorted(&preview), sorted(&read));

    // Повторный импорт не дублирует строку exec.
    game_binds_install::install_import(
        &search(),
        &root,
        GAME_ID,
        MOD,
        ImportSource::Text(SAMPLE_CFG),
        &[],
    )
    .unwrap();
    let again = read_latin1(&root, "Half-Life/valve_WON/userconfig.cfg");
    assert_eq!(again.matches("exec \"player.cfg\"").count(), 1);

    game_binds_install::remove_import(&search(), &root, GAME_ID, MOD).unwrap();
    assert!(!exists(&root, "Half-Life/valve_WON/player.cfg"));
    assert!(!read_latin1(&root, "Half-Life/valve_WON/userconfig.cfg").contains("player.cfg"));
    let after = game_binds_install::read_install_binds(&search(), &root, MOD, "goldsrc");
    assert_eq!(after.binds.get("n").unwrap().command, "+use");
}

#[test]
fn exec_chain_asks_every_file_once_top_level_first() {
    let main = root(
        "main.cfg",
        "exec \"binds.cfg\"\r\nalias +x \"exec scripts/run.cfg\"\r\nexec config.cfg\r\nexec binds.cfg\r\n",
    );
    let none = ImportFiles::new();
    let ask = |files: &ImportFiles, missing: &[&str], top_level_only: bool| {
        let missing: Vec<String> = missing.iter().map(|m| (*m).to_owned()).collect();
        cfg_import::next_wanted("goldsrc", &main, files, &missing, top_level_only)
    };
    let wanted = ask(&none, &[], false).unwrap();
    assert_eq!(
        (
            wanted.key.as_str(),
            wanted.name.as_str(),
            wanted.from.as_str()
        ),
        ("binds.cfg", "binds.cfg", "main.cfg")
    );
    let files = files_of(&[("binds.cfg", "Binds.cfg", "exec more.cfg\nbind k +jump")]);
    assert_eq!(
        ask(&files, &[], false).unwrap().key,
        "scripts/run.cfg",
        "сначала файлы, на которые exec указывает сам конфиг"
    );
    assert_eq!(
        ask(&files, &[], true).unwrap().key,
        "more.cfg",
        "exec в alias не нужен импорту только биндов"
    );
    let all = files_of(&[
        ("binds.cfg", "Binds.cfg", "exec more.cfg\nbind k +jump"),
        ("scripts/run.cfg", "run.cfg", "+jump"),
    ]);
    assert!(
        ask(&all, &["more.cfg"], false).is_none(),
        "config.cfg игры и отказ не спрашиваются"
    );
    // GoldSrc не выполняет файлы без .cfg, Source дописывает расширение.
    assert!(cfg_import::exec_target("jump", "goldsrc").is_none());
    assert_eq!(
        cfg_import::exec_target("jump", "source").unwrap().key,
        "jump.cfg"
    );
    assert!(cfg_import::exec_target("../evil.cfg", "goldsrc").is_none());
}

#[test]
fn import_keeps_player_names_cuts_exec_loops_and_reports_repeats() {
    let built = cfg_import::build_import(
        "goldsrc",
        &root(
            "main.cfg",
            "fps_max 100\nexec a.cfg\nexec b.cfg\nbind f +use\nalias go \"exec a.cfg\"\n",
        ),
        &files_of(&[
            (
                "a.cfg",
                "a.cfg",
                "bind f +attack\nexec main.cfg\nfps_max 250\n",
            ),
            (
                "b.cfg",
                "b.cfg",
                "bind f +attack\nexec main.cfg\nfps_max 250\n",
            ),
        ]),
        &["c.cfg".to_owned()],
    );
    let names: Vec<&str> = built.outputs.iter().map(|o| o.file.as_str()).collect();
    assert_eq!(names, ["main.cfg", "a.cfg", "b.cfg"], "имена как у игрока");
    assert_eq!(built.execs, ["main.cfg"]);
    let main = text_of(&built, "main.cfg");
    assert!(has_line(main, "exec a.cfg"), "exec не переписывается");
    assert!(has_line(main, "exec b.cfg"));
    assert!(main.contains("alias go \"exec a.cfg\""));
    assert!(
        has_line(
            text_of(&built, "a.cfg"),
            "// HLSR: exec loop: exec main.cfg"
        ),
        "a.cfg → main.cfg крутился бы бесконечно"
    );
    assert_eq!(
        built.report.cycles,
        [
            ("a.cfg".to_owned(), "main.cfg".to_owned()),
            ("b.cfg".to_owned(), "main.cfg".to_owned())
        ]
    );
    assert_eq!(
        built.report.duplicate_files,
        [("a.cfg".to_owned(), "b.cfg".to_owned())]
    );
    let mut dup: Vec<String> = built
        .report
        .duplicates
        .iter()
        .map(|d| format!("{} {}", d.kind, d.name))
        .collect();
    dup.sort();
    assert_eq!(dup, ["bind f", "cvar fps_max"]);
    let value = |kind: &str| {
        built
            .report
            .duplicates
            .iter()
            .find(|d| d.kind == kind)
            .unwrap()
            .value
            .clone()
    };
    assert_eq!(value("bind"), "+use", "действует последнее");
    assert_eq!(value("cvar"), "250");
    // Плоский текст — порядок выполнения: файл цепочки на месте своего exec.
    let flat = game_binds::with_import(None, &built.flat_text);
    assert_eq!(flat.binds.get("f").unwrap().command, "+use");
}

#[test]
fn import_with_a_chain_writes_files_under_their_names_and_remove_brings_the_old_ones_back() {
    let dir = temp();
    let root_dir = root_of(&dir);
    fake_install(&root_dir);
    write_latin1(
        &root_dir,
        "Half-Life/valve_WON/config.cfg",
        "bind \"w\" \"+forward\"\r\nexec userconfig.cfg\r\n",
    );
    write_latin1(
        &root_dir,
        "Half-Life/valve_WON/userconfig.cfg",
        "exec hlsr-binds.cfg\r\n",
    );
    write_latin1(
        &root_dir,
        "Half-Life/valve_WON/hlsr-import-old.cfg",
        "bind o +use\r\n",
    );
    write(
        &root_dir,
        "Half-Life/valve_WON/scripts/jump.cfg",
        "// прежний файл игрока\r\n",
    );
    let mut files = files_of(&[(
        "scripts/jump.cfg",
        "Jump.cfg",
        "alias go +jump\nbind j +jump\n",
    )]);
    files.get_mut("scripts/jump.cfg").unwrap().target = Some("scripts/Jump.cfg".to_owned());
    let built = cfg_import::build_import(
        "goldsrc",
        &root("main.cfg", "exec scripts/Jump.cfg\nbind n go\n"),
        &files,
        &[],
    );
    let outputs: Vec<&str> = built.outputs.iter().map(|o| o.file.as_str()).collect();
    assert_eq!(outputs, ["main.cfg", "scripts/Jump.cfg"]);
    let result = game_binds_install::install_import(
        &search(),
        &root_dir,
        GAME_ID,
        MOD,
        ImportSource::Outputs(&built.outputs),
        &built.execs,
    )
    .unwrap();
    assert!(matches!(result, InstallImport::Ok { .. }));
    assert!(
        !exists(&root_dir, "Half-Life/valve_WON/hlsr-import-old.cfg"),
        "файл прошлого импорта удалён"
    );
    assert!(
        read_latin1(&root_dir, "Half-Life/valve_WON/userconfig.cfg")
            .split('\n')
            .any(|l| l == "exec \"main.cfg\"\r")
    );
    let record = game_binds_install::read_import_record(&root_dir).unwrap();
    let recorded: Vec<(&str, bool)> = record
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.created))
        .collect();
    assert_eq!(recorded, [("main.cfg", true), ("scripts/Jump.cfg", false)]);
    let scripts: Vec<String> = std::fs::read_dir(root_dir.join("Half-Life/valve_WON/scripts"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        scripts,
        ["jump.cfg"],
        "файл с другим регистром не задвоился"
    );
    let read = game_binds_install::read_install_binds(&search(), &root_dir, MOD, "goldsrc");
    let imported: Vec<String> = record.files.iter().map(|f| f.path.to_lowercase()).collect();
    assert!(game_binds::is_import_source(
        &read.binds.get("j").unwrap().source,
        &imported
    ));
    assert_eq!(read.aliases.get("go").unwrap(), "+jump");

    game_binds_install::remove_import(&search(), &root_dir, GAME_ID, MOD).unwrap();
    assert!(
        !exists(&root_dir, "Half-Life/valve_WON/main.cfg"),
        "созданный импортом файл удалён"
    );
    assert_eq!(
        read_utf8(&root_dir, "Half-Life/valve_WON/scripts/jump.cfg"),
        "// прежний файл игрока\r\n",
        "заменённый вернулся байт в байт"
    );
    assert!(!read_latin1(&root_dir, "Half-Life/valve_WON/userconfig.cfg").contains("main.cfg"));
    assert!(game_binds_install::read_import_record(&root_dir).is_none());
}

#[test]
fn imported_userconfig_lands_in_place_and_keeps_the_launcher_lines() {
    let dir = temp();
    let root_dir = root_of(&dir);
    fake_install(&root_dir);
    write_latin1(
        &root_dir,
        "Half-Life/valve_WON/config.cfg",
        "exec userconfig.cfg\r\n",
    );
    write_latin1(
        &root_dir,
        "Half-Life/valve_WON/userconfig.cfg",
        "sensitivity 2\r\nexec hlsr-personalization.cfg\r\nexec hlsr-binds.cfg\r\n",
    );
    let built = cfg_import::build_import(
        "goldsrc",
        &root("userconfig.cfg", "bind k +jump\r\n"),
        &ImportFiles::new(),
        &[],
    );
    assert!(
        built.execs.is_empty(),
        "userconfig.cfg движок выполняет сам"
    );
    game_binds_install::install_import(
        &search(),
        &root_dir,
        GAME_ID,
        MOD,
        ImportSource::Outputs(&built.outputs),
        &built.execs,
    )
    .unwrap();
    assert_eq!(
        read_latin1(&root_dir, "Half-Life/valve_WON/userconfig.cfg"),
        "bind k +jump\r\nexec hlsr-personalization.cfg\r\nexec hlsr-binds.cfg\r\n"
    );
    game_binds_install::remove_import(&search(), &root_dir, GAME_ID, MOD).unwrap();
    assert_eq!(
        read_latin1(&root_dir, "Half-Life/valve_WON/userconfig.cfg"),
        "sensitivity 2\r\nexec hlsr-personalization.cfg\r\nexec hlsr-binds.cfg\r\n"
    );
}

/// Буфер команд GoldSrc как в `Cmd_Exec_f` (ReHLDS, engine/cmd.cpp): файл, который не
/// помещается в свободную часть буфера, выполняется построчно, и перед каждой его строкой
/// доделывается остальной буфер. Возвращает значения переменных после выполнения `roots`.
fn goldsrc_exec(
    dir: &Path,
    roots: &[&str],
    max_size: usize,
) -> std::collections::HashMap<String, String> {
    struct Engine<'a> {
        dir: &'a Path,
        max_size: usize,
        buffer: Vec<char>,
        cvars: std::collections::HashMap<String, String>,
    }
    impl Engine<'_> {
        fn read(&self, name: &str) -> Option<String> {
            let real = std::fs::read_dir(self.dir)
                .ok()?
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .find(|entry| entry.to_lowercase() == name.to_lowercase())?;
            Some(hlsr_core::js::latin1_string(
                &std::fs::read(self.dir.join(real)).ok()?,
            ))
        }

        fn run(&mut self, command: &str) {
            let command = command.split("//").next().unwrap_or_default();
            let mut values: Vec<String> = Vec::new();
            let chars: Vec<char> = command.chars().collect();
            let mut i = 0;
            while i < chars.len() {
                if chars[i].is_whitespace() {
                    i += 1;
                    continue;
                }
                let mut end = i;
                if chars[i] == '"'
                    && let Some(close) = chars[i + 1..].iter().position(|&c| c == '"')
                {
                    end = i + 1 + close + 1;
                } else {
                    while end < chars.len() && !chars[end].is_whitespace() {
                        end += 1;
                    }
                }
                let token: String = chars[i..end].iter().collect();
                let token = token.strip_prefix('"').unwrap_or(&token);
                values.push(token.strip_suffix('"').unwrap_or(token).to_owned());
                i = end;
            }
            if values.is_empty() {
                return;
            }
            let verb = values[0].to_lowercase();
            if verb == "exec" {
                if let Some(name) = values.get(1).cloned() {
                    self.exec_file(&name);
                }
            } else if values.len() == 2 && verb != "bind" && verb != "alias" {
                self.cvars.insert(verb, values[1].clone());
            }
        }

        fn execute(&mut self) {
            while !self.buffer.is_empty() {
                let mut end = 0;
                let mut quoted = false;
                while end < self.buffer.len() {
                    if self.buffer[end] == '"' {
                        quoted = !quoted;
                    }
                    if self.buffer[end] == '\n' || (self.buffer[end] == ';' && !quoted) {
                        break;
                    }
                    end += 1;
                }
                let command: String = self.buffer[..end].iter().collect();
                let command = command.strip_suffix('\r').unwrap_or(&command).to_owned();
                self.buffer = self.buffer[(end + 1).min(self.buffer.len())..].to_vec();
                self.run(&command);
            }
        }

        fn exec_file(&mut self, name: &str) {
            let Some(text) = self.read(name) else { return };
            if self.buffer.len() + text.chars().count() + 2 < self.max_size {
                let mut next: Vec<char> = text.chars().collect();
                next.push('\n');
                next.extend(self.buffer.iter());
                self.buffer = next;
                return;
            }
            for line in text.split('\n') {
                self.execute();
                if !line.trim().is_empty() {
                    let mut next: Vec<char> = line.chars().collect();
                    next.push('\n');
                    next.extend(self.buffer.iter());
                    self.buffer = next;
                }
            }
        }
    }
    let mut engine = Engine {
        dir,
        max_size,
        buffer: Vec::new(),
        cvars: std::collections::HashMap::new(),
    };
    for root in roots {
        engine.buffer.extend(format!("exec {root}\n").chars());
        engine.execute();
    }
    engine.cvars
}

/// Настройки теста 0.4.7 «настройки лаунчера главнее большого импортированного конфига».
fn launcher_settings() -> Value {
    json!({"bxt": {"fov": 120}, "personalization": {}})
}

#[test]
fn launcher_settings_win_over_a_big_imported_config() {
    let dir = temp();
    let root_dir = root_of(&dir);
    fake_install(&root_dir);
    write_latin1(
        &root_dir,
        "Half-Life/valve_WON/config.cfg",
        "unbindall\r\ncrosshair \"1\"\r\nexec userconfig.cfg\r\n",
    );
    // Как у игрока: 9 КБ биндов и в конце свои FOV и прицел.
    let mut lines: Vec<String> = (0..300)
        .map(|i| format!("bind f{i} \"say filler line number {i}\""))
        .collect();
    lines.extend([
        "default_fov 115".to_owned(),
        "crosshair 0".to_owned(),
        String::new(),
    ]);
    let imported = lines.join("\r\n");
    assert!(imported.len() > 8192);
    game_binds_install::install_import(
        &search(),
        &root_dir,
        GAME_ID,
        MOD,
        ImportSource::Text(&imported),
        &[],
    )
    .unwrap();
    apply_to_install(&root_dir, &launcher_settings());

    // Без хвоста импорт выполнился бы позже файлов лаунчера.
    let written = read_latin1(&root_dir, "Half-Life/valve_WON/player.cfg");
    let without_trailer = &written[..written.find(IMPORT_TRAILER_MARK).unwrap()];
    write_latin1(&root_dir, "Half-Life/valve_WON/player.cfg", without_trailer);
    let mod_dir = root_dir.join("Half-Life/valve_WON");
    assert_eq!(
        goldsrc_exec(&mod_dir, &["config.cfg"], 8192)["default_fov"],
        "115",
        "симулятор воспроизводит перекрытие"
    );

    apply_to_install(&root_dir, &launcher_settings());
    let fixed = goldsrc_exec(&mod_dir, &["config.cfg"], 8192);
    assert_eq!(fixed["default_fov"], "120");
    assert_eq!(fixed["crosshair"], "1");
    // Большой буфер не нужен: в нём всё и так по порядку.
    assert_eq!(
        goldsrc_exec(&mod_dir, &["config.cfg"], 65536)["default_fov"],
        "120"
    );

    // Хвост повторяет ровно строки лаунчера после импорта и не копится.
    apply_to_install(&root_dir, &launcher_settings());
    let text = read_latin1(&root_dir, "Half-Life/valve_WON/player.cfg");
    let parts: Vec<&str> = text.split(IMPORT_TRAILER_MARK).collect();
    assert_eq!(parts.len(), 2, "одна пометка хвоста");
    let trailer: Vec<&str> = parts[1]
        .trim()
        .split('\n')
        .map(|l| l.trim_end_matches('\r'))
        .collect();
    assert_eq!(
        trailer,
        ["exec hlsr-personalization.cfg", "exec hlsr-binds.cfg"]
    );

    // Окно настроек читает те же итоговые значения.
    let read = game_binds_install::read_install_binds(&search(), &root_dir, MOD, "goldsrc");
    assert!(game_binds::is_import_source(
        &read.binds.get("F1").unwrap().source,
        &["player.cfg".to_owned()]
    ));
}

#[test]
fn demos_are_found_in_the_language_folder_of_the_mod_too() {
    let dir = temp();
    let root_dir = root_of(&dir);
    fake_install(&root_dir);
    write(&root_dir, "Half-Life/valve_WON/a.dem", "x");
    write(&root_dir, "Half-Life/valve_won_russian/b.dem", "x");
    write(&root_dir, "Half-Life/valve_WON_extra/c.dem", "x");
    let dirs = runs_scan::run_dirs(&root_dir, "Half-Life/valve_WON");
    assert_eq!(dirs, ["Half-Life/valve_won_russian", "Half-Life/valve_WON"]);
    let ScanResult::Ok { path, items, .. } = runs_scan::scan_many(&root_dir, &dirs, Some(".dem"))
    else {
        panic!("список записей");
    };
    assert_eq!(
        path,
        root_dir.join("Half-Life").join("valve_won_russian"),
        "«Открыть папку» ведёт в языковую папку"
    );
    let mut names: Vec<&str> = items.iter().map(|item| item.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["a.dem", "b.dem"]);
}

#[test]
fn players_userconfig_is_asked_for_and_connected_from_config() {
    let config = "unbindall\r\nbind \"t\" \"exec userconfig.cfg; say Tero0\"\r\nbind \"-\" \"p2pre\"\r\nexec userconfig.cfg\r\n";
    let main = root("config.cfg", config);
    let none = ImportFiles::new();
    let want = cfg_import::next_wanted("goldsrc", &main, &none, &[], false);
    assert_eq!(
        want.map(|w| w.name).as_deref(),
        Some("userconfig.cfg"),
        "движок выполнит userconfig.cfg игрока после config.cfg"
    );
    let files = files_of(&[(
        "userconfig.cfg",
        "userconfig.cfg",
        "alias p2pre \"say pre\"\r\nexec config.cfg; echo after\r\n",
    )]);
    assert!(cfg_import::next_wanted("goldsrc", &main, &files, &[], false).is_none());
    let built = cfg_import::build_import("goldsrc", &main, &files, &[]);
    let names: Vec<&str> = built.outputs.iter().map(|o| o.file.as_str()).collect();
    assert_eq!(
        names,
        ["config.cfg", "userconfig.cfg"],
        "файлы ложатся на свои места"
    );
    assert!(built.execs.is_empty(), "оба файла движок выполняет сам");
    let config_out = text_of(&built, "config.cfg");
    assert!(has_line(config_out, "exec userconfig.cfg"));
    assert!(
        config_out.contains("bind \"t\" \"exec userconfig.cfg; say Tero0\""),
        "exec в бинде ведёт к userconfig игрока"
    );
    assert!(
        text_of(&built, "userconfig.cfg")
            .lines()
            .any(|l| l.trim_start().starts_with("echo after")
                && l.ends_with("// HLSR: exec config.cfg")),
        "exec config.cfg убран, echo остался"
    );
    assert_eq!(
        game_binds::with_import(None, &built.flat_text)
            .aliases
            .get("p2pre")
            .unwrap(),
        "say pre"
    );
    // Без userconfig.cfg игрока его exec не выполнит userconfig.cfg лаунчера (а тот — снова
    // импорт); exec в бинде остаётся как в игре.
    let alone = cfg_import::build_import("goldsrc", &main, &none, &["userconfig.cfg".to_owned()]);
    assert!(has_line(
        &alone.outputs[0].text,
        "// HLSR: not imported: exec userconfig.cfg"
    ));
    assert!(
        alone.outputs[0]
            .text
            .contains("bind \"t\" \"exec userconfig.cfg; say Tero0\"")
    );
    let unasked = cfg_import::build_import("goldsrc", &main, &none, &[]);
    assert!(
        has_line(
            &unasked.outputs[0].text,
            "// HLSR: not imported: exec userconfig.cfg"
        ),
        "и когда файл не спрашивали"
    );
    // Текст без цепочки (installImport строкой) тоже не вызывает userconfig.cfg.
    let plain = game_binds::import_file_text("exec userconfig.cfg\nexec autoexec.cfg\n", false);
    assert_eq!(
        plain,
        "// HLSR: exec userconfig.cfg\n// HLSR: exec autoexec.cfg\n"
    );
}

#[test]
fn autoexec_is_asked_next_to_config_and_player_file_names_are_kept() {
    let names = |list: &[&str]| list.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
    let companion = cfg_import::companions_of(&names(&["Config.cfg"]));
    assert_eq!(companion.len(), 1);
    assert_eq!(
        (companion[0].name, companion[0].from, companion[0].before),
        ("autoexec.cfg", "config.cfg", true)
    );
    assert!(cfg_import::companions_of(&names(&["config.cfg", "autoexec.cfg"])).is_empty());
    assert!(cfg_import::companions_of(&names(&["userconfig.cfg"])).is_empty());
    let result = cfg_import::root_of_picked(&picked(&[
        ("autoexec.cfg", "alias a1 \"say a\"\n"),
        (
            "config.cfg",
            "bind q \"exec autoexec.cfg\"\nexec userconfig.cfg\n",
        ),
    ]));
    let keys: Vec<&str> = result.files.keys().collect();
    assert_eq!(keys, ["autoexec.cfg", "config.cfg"], "имена как у игрока");
    assert_eq!(
        result.root.text,
        "exec \"autoexec.cfg\"\nexec \"config.cfg\""
    );
    assert!(
        result.root.virtual_root,
        "своего файла у нескольких выбранных нет"
    );
    let same = cfg_import::root_of_picked(&picked(&[("a.cfg", ""), ("A.cfg", "")]));
    let keys: Vec<&str> = same.files.keys().collect();
    assert_eq!(
        keys,
        ["a.cfg", "a-2.cfg"],
        "одинаковые имена не затирают друг друга"
    );
    let want = cfg_import::next_wanted("goldsrc", &result.root, &result.files, &[], false);
    assert_eq!(
        want.map(|w| w.name).as_deref(),
        Some("userconfig.cfg"),
        "autoexec.cfg уже выбран, спрашивается только userconfig.cfg"
    );
    let built = cfg_import::build_import(
        "goldsrc",
        &result.root,
        &result.files,
        &["userconfig.cfg".to_owned()],
    );
    let config = built
        .outputs
        .iter()
        .find(|o| o.text.contains("bind q"))
        .unwrap();
    assert!(config.text.contains("bind q \"exec autoexec.cfg\""));
    assert!(has_line(
        &config.text,
        "// HLSR: not imported: exec userconfig.cfg"
    ));
    let files: Vec<&str> = built.outputs.iter().map(|o| o.file.as_str()).collect();
    assert_eq!(files, ["autoexec.cfg", "config.cfg"]);
    assert!(built.execs.is_empty());
    let own = cfg_import::root_of_picked(&picked(&[
        ("binds.cfg", "bind k +jump\n"),
        ("scripts.cfg", "alias x y\n"),
    ]));
    let own = cfg_import::build_import("goldsrc", &own.root, &own.files, &[]);
    assert_eq!(
        own.execs,
        ["binds.cfg", "scripts.cfg"],
        "свои файлы подключаются из userconfig.cfg по порядку"
    );
}

#[test]
fn repeated_import_of_a_saved_hlsr_import_cfg_asks_the_files_of_its_chain() {
    let main = root(
        "hlsr-import.cfg",
        "// Imported by HLSR Launcher\r\nexec hlsr-import-jump.cfg\r\nexec hlsr-binds.cfg\r\n",
    );
    let want = cfg_import::next_wanted("goldsrc", &main, &ImportFiles::new(), &[], false);
    assert_eq!(
        want.map(|w| w.name).as_deref(),
        Some("hlsr-import-jump.cfg")
    );
    let files = files_of(&[(
        "hlsr-import-jump.cfg",
        "hlsr-import-jump.cfg",
        "bind j +jump\r\n",
    )]);
    let built = cfg_import::build_import("goldsrc", &main, &files, &[]);
    assert!(
        has_line(&built.outputs[0].text, "// HLSR: exec hlsr-binds.cfg"),
        "файлы лаунчера из хвоста не выполняются раньше времени"
    );
    let names: Vec<&str> = built.outputs.iter().map(|o| o.file.as_str()).collect();
    assert_eq!(
        names,
        ["player.cfg", "jump.cfg"],
        "имена прошлых версий без hlsr-import"
    );
    assert!(has_line(&built.outputs[0].text, "exec jump.cfg"));
    assert_eq!(
        game_binds::with_import(None, &built.flat_text)
            .binds
            .get("j")
            .unwrap()
            .command,
        "+jump"
    );
}

fn object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

#[test]
fn negative_crosshair_gap_is_adopted_for_bxt() {
    let adopted = goldsrc_import_adopt::adopt_imported(
        "bxt_cross 1\nbxt_cross_gap -4\n",
        &Map::new(),
        &Map::new(),
    );
    assert!(adopted.skipped.is_empty());
    assert_eq!(adopted.personalization["crossGap"], json!(-4));
    // Сохранение настроек (launchArgs.sanitizeSettings) зазор не обрезает; BXT его пишет, Ghosting — не меньше 0.
    let settings =
        launch_args::sanitize_settings(&json!({"personalization": adopted.personalization}))
            .settings
            .to_value();
    assert_eq!(
        settings["personalization"]["crossGap"],
        json!(-4),
        "сохранение не обрезает зазор"
    );
    let values = goldsrc_personalization::to_cfg_lines(&settings);
    assert!(has_line(&values, "bxt_cross_gap \"-4\""));
    let ghosting = ghosting_personalization::to_cfg_lines(&settings["personalization"]);
    assert!(ghosting.lines().any(|line| line == "gh_cross_gap \"0\""));
    // Разумный предел зазора — 30 в обе стороны.
    let clamped = launch_args::sanitize_settings(&json!({"personalization": {"crossGap": -99}}));
    assert_eq!(clamped.settings.personalization.cross_gap, -30);
}

#[test]
fn crosshair_hud_and_fov_of_the_config_pass_into_personalization() {
    let text = [
        "default_fov 90",
        "bxt_hud_speedometer 0",
        "bxt_hud_jumpspeed 1",
        "bxt_viewmodel_fov 100",
        "bxt_cross 1",
        "bxt_cross_color \"255 0 0\"",
        "bxt_cross_size 6",
        "bxt_cross_outline 2",
        "bxt_cross_dot_color \"\"",
        "crosshair 0",
        "bxt_hud_color \"0 160 255\"",
        "bxt_cross_gap 99",
        "alias zoom \"default_fov 40\"",
        "bxt_hud_timer 1",
        "bxt_hud_timer 0",
    ]
    .join("\n");
    let result = goldsrc_import_adopt::adopt_imported(
        &text,
        &object(json!({"crossPreset": "standard"})),
        &object(json!({"fov": 70, "timer": true})),
    );
    assert_eq!(
        Value::Object(result.bxt.clone()),
        json!({"fov": 90, "timer": false, "speedometer": false, "jumpspeed": true, "viewmodelFov": 100, "hideCrosshair": false}),
        "последнее значение главнее, alias не в счёт"
    );
    let p = &result.personalization;
    assert_eq!(p["crossEnabled"], json!(true));
    assert_eq!(p["crossPreset"], json!("custom"));
    assert_eq!(p["crossColor"], json!({"r": 255, "g": 0, "b": 0}));
    assert_eq!(p["crossSize"], json!(6));
    assert_eq!(p["crossOutline"], json!(true));
    assert_eq!(p["crossOutlineThickness"], json!(2));
    assert_eq!(p["crossDotColor"], Value::Null);
    assert_eq!(p["crossHideDefault"], json!(true));
    assert_eq!(
        (&p["hudEnabled"], &p["hudColor"]),
        (&json!(true), &json!({"r": 0, "g": 160, "b": 255}))
    );
    assert_eq!(
        result.skipped,
        ["bxt_cross_gap 99"],
        "вне пределов лаунчера"
    );
    // Без прицела BXT штатный прячет отдельный флаг.
    let bare = goldsrc_import_adopt::adopt_imported(
        "crosshair 0\ndefault_fov 150\n",
        &Map::new(),
        &object(json!({"fov": 100})),
    );
    assert_eq!(bare.skipped, ["default_fov 150"]);
    assert_eq!(bare.adopted, ["crosshair"]);
    assert_eq!(
        Value::Object(bare.bxt.clone()),
        json!({"fov": 100, "hideCrosshair": true})
    );
    assert!(
        bare.personalization.get("crossEnabled").is_none(),
        "прицел BXT не включается"
    );
    let bare_cfg = goldsrc_personalization::to_cfg_lines(&json!({"bxt": bare.bxt}));
    assert!(has_line(&bare_cfg, "crosshair \"0\""));
    assert!(has_line(&bare_cfg, "bxt_cross \"0\""));
    let shown = goldsrc_import_adopt::adopt_imported(
        "crosshair 1\n",
        &Map::new(),
        &object(json!({"hideCrosshair": true})),
    );
    assert_eq!(
        shown.bxt["hideCrosshair"],
        json!(false),
        "crosshair 1 снимает флаг"
    );
    // Результат проходит через настройки лаунчера без потерь.
    let written = goldsrc_personalization::to_cfg_lines(&json!({
        "personalization": result.personalization,
        "bxt": result.bxt,
    }));
    for line in [
        "default_fov \"90\"",
        "bxt_cross_color \"255 0 0\"",
        "bxt_cross_outline \"2\"",
        "crosshair \"0\"",
        "bxt_hud_speedometer \"0\"",
    ] {
        assert!(has_line(&written, line), "{line}");
    }
}

// ======================================================= game-config-selftest.js

#[test]
fn goldsrc_config_from_the_language_folder_wins_while_the_game_writes_there() {
    let dir = temp();
    let root_dir = root_of(&dir);
    write_latin1(
        &root_dir,
        "Half-Life/valve_WON/config.cfg",
        "sensitivity \"3\"\r\nbind \"q\" \"+jump\"\r\nm_pitch \"0.022\"\r\n",
    );
    write_latin1(
        &root_dir,
        "Half-Life/valve_WON_russian/config.cfg",
        "sensitivity \"2\"\r\nbind \"q\" \"+duck\"\r\nm_pitch \"-0.022\"\r\n",
    );
    let old = SystemTime::now() - Duration::from_secs(60);
    set_time(&root_dir, "Half-Life/valve_WON/config.cfg", old);

    let read = game_binds_install::read_install_binds(&search(), &root_dir, MOD, "goldsrc");
    assert_eq!(read.binds.get("q").unwrap().command, "+duck");
    assert_eq!(read.cvars.get("sensitivity").unwrap(), "2");

    // Файлы лаунчера движок найдёт в valve_WON: их нет в языковой папке.
    apply_to_install(&root_dir, &goldsrc_settings::defaults().to_value());
    assert!(exists(
        &root_dir,
        "Half-Life/valve_WON/hlsr-personalization.cfg"
    ));
    assert!(exists(&root_dir, "Half-Life/valve_WON/hlsr-binds.cfg"));
    let userconfig = read_latin1(&root_dir, "Half-Life/valve_WON/userconfig.cfg");
    assert!(userconfig.contains("exec hlsr-personalization.cfg"));
    assert!(userconfig.contains("exec hlsr-binds.cfg"));
    let in_russian: Vec<String> = std::fs::read_dir(root_dir.join("Half-Life/valve_WON_russian"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(in_russian, ["config.cfg"]);

    // Steam снова на английском: игра пишет config.cfg в valve_WON.
    set_time(&root_dir, "Half-Life/valve_WON_russian/config.cfg", old);
    set_time(
        &root_dir,
        "Half-Life/valve_WON/config.cfg",
        SystemTime::now(),
    );
    let read = game_binds_install::read_install_binds(&search(), &root_dir, MOD, "goldsrc");
    assert_eq!(read.cvars.get("sensitivity").unwrap(), "3");
}

#[test]
fn goldsrc_cfg_folder_is_chosen_by_the_steam_language_time_only_without_it() {
    let dir = temp();
    let root_dir = root_of(&dir);
    write_latin1(
        &root_dir,
        "Half-Life/valve_WON/config.cfg",
        "sensitivity 3\r\n",
    );
    write_latin1(
        &root_dir,
        "Half-Life/valve_WON_russian/config.cfg",
        "sensitivity 2\r\n",
    );
    let old = SystemTime::now() - Duration::from_secs(60);
    set_time(&root_dir, "Half-Life/valve_WON/config.cfg", old);
    let (modd, ru) = (
        root_dir.join("Half-Life").join("valve_WON"),
        root_dir.join("Half-Life").join("valve_WON_russian"),
    );
    let search = search();
    let dirs = |language: Option<&str>| {
        search
            .search_dirs(&root_dir, MOD, "goldsrc", LanguageOption::Given(language))
            .unwrap()
    };

    // Русский config.cfg новее, но Steam уже на английском.
    assert_eq!(dirs(Some("english")), std::slice::from_ref(&modd));
    assert_eq!(dirs(Some("russian")), [ru.clone(), modd.clone()]);
    // Папки языка ещё нет: игра читает valve_WON.
    assert_eq!(dirs(Some("german")), std::slice::from_ref(&modd));
    // Язык не прочитан — по времени записи.
    assert_eq!(dirs(None), [ru.clone(), modd.clone()]);
    set_time(&root_dir, "Half-Life/valve_WON_russian/config.cfg", old);
    set_time(
        &root_dir,
        "Half-Life/valve_WON/config.cfg",
        SystemTime::now(),
    );
    assert_eq!(dirs(None), std::slice::from_ref(&modd));
    assert_eq!(dirs(Some("russian")), [ru, modd]);
}

#[test]
fn import_into_the_tab_takes_values_from_the_chain() {
    let built = cfg_import::build_import(
        "goldsrc",
        &root(
            "my.cfg",
            "sensitivity 2\r\nexec \"mouse.cfg\"\r\nvolume \"abc\"\r\nm_pitch \"-0.022\" // invert\r\nunknown_cvar 5\r\n",
        ),
        &files_of(&[(
            "mouse.cfg",
            "mouse.cfg",
            "sensitivity 99; alias x \"sensitivity 1\"\n",
        )]),
        &[],
    );
    // Дальше вкладка (`gameConfig.importValues`, задача 17) берёт из плоского текста последнее
    // значение переменной: файл цепочки идёт на месте своего exec, alias не выполняется.
    assert_eq!(
        built.flat_text,
        "\"sensitivity\" \"2\"\n\"sensitivity\" \"99\"\n\"alias\" \"x\" \"sensitivity 1\"\n\"volume\" \"abc\"\n\"m_pitch\" \"-0.022\"\n\"unknown_cvar\" \"5\""
    );
}

#[test]
fn several_picked_cfgs_import_in_order_like_an_exec_chain() {
    let list = picked(&[
        ("config.cfg", "sensitivity 2\nexec extra.cfg\n"),
        ("My Binds.cfg", "sensitivity 4\nbind f \"+use\"\n"),
    ]);
    let result = cfg_import::root_of_picked(&list);
    assert_eq!(result.files.len(), 2);
    // Выбранные файлы уже есть: окно выбора просится только для extra.cfg.
    let want = cfg_import::next_wanted("goldsrc", &result.root, &result.files, &[], true).unwrap();
    assert_eq!(want.name, "extra.cfg");
    let built = cfg_import::build_import("goldsrc", &result.root, &result.files, &[want.key]);
    let flat = game_binds::with_import(None, &built.flat_text);
    assert_eq!(flat.cvars.get("sensitivity").unwrap(), "4");
    // Один файл — без обёртки, как раньше.
    assert!(cfg_import::root_of_picked(&list[..1]).files.is_empty());
}

// ===================================================== ghost-runs-selftest.js

fn runs_fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = temp();
    let root = root_of(&dir);
    write(&root, "ghostingmod/runs/one.run", "ghost one");
    write(&root, "ghostingmod/runs/two.run", "ghost two");
    write(&root, "outside.run", "keep outside");
    write(&root, "ghostingmod/runs/config.cfg", "keep config");
    (dir, root)
}

const RUNS: &str = "ghostingmod/runs";

fn names_of(result: ScanResult) -> Vec<String> {
    match result {
        ScanResult::Ok { items, .. } => items.into_iter().map(|i| i.name).collect(),
        other => panic!("не список: {other:?}"),
    }
}

#[test]
fn ghosts_deleting_a_run_removes_only_that_file_and_updates_the_scan() {
    let (_dir, root) = runs_fixture();
    assert_eq!(
        runs_scan::delete_item(&root, RUNS, "one.run", None),
        DeleteResult::Ok {
            name: "one.run".into()
        }
    );
    assert_eq!(names_of(runs_scan::scan(&root, RUNS, None)), ["two.run"]);
    assert_eq!(
        read_utf8(&root, "ghostingmod/runs/config.cfg"),
        "keep config"
    );
    assert_eq!(read_utf8(&root, "outside.run"), "keep outside");
}

#[test]
fn ghosts_paths_outside_runs_non_recordings_folders_and_symlinks_cannot_be_deleted() {
    let (_dir, root) = runs_fixture();
    let directory = root.join("ghostingmod/runs");
    std::fs::create_dir(directory.join("directory.run")).unwrap();
    let absolute = root.join("outside.run").to_string_lossy().into_owned();
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut names = vec![
        "../../outside.run",
        "..\\..\\outside.run",
        absolute.as_str(),
        "config.cfg",
        "directory.run",
    ];
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("outside.run"), directory.join("link.run")).unwrap();
        names.push("link.run");
    }
    for name in &names {
        assert!(
            matches!(
                runs_scan::delete_item(&root, RUNS, name, None),
                DeleteResult::Error { .. }
            ),
            "{name}"
        );
    }
    assert!(exists(&root, "outside.run"));
    #[cfg(unix)]
    {
        assert!(directory.join("link.run").symlink_metadata().is_ok());
        std::fs::rename(&directory, root.join("ghostingmod/runs-real")).unwrap();
        std::os::unix::fs::symlink(root.join("ghostingmod/runs-real"), &directory).unwrap();
        assert!(matches!(
            runs_scan::delete_item(&root, RUNS, "one.run", None),
            DeleteResult::Error { .. }
        ));
        assert!(exists(&root, "ghostingmod/runs-real/one.run"));
    }
}

#[test]
fn hl1_demos_in_the_mod_folder_only_dem_files_are_listed_and_deleted() {
    let dir = temp();
    let root = root_of(&dir);
    let relative = "Half-Life/valve_WON";
    write(&root, "Half-Life/valve_WON/c1a0.dem", "demo");
    write(&root, "Half-Life/valve_WON/RUN.DEM", "demo");
    write(&root, "Half-Life/valve_WON/config.cfg", "keep config");
    write(&root, "Half-Life/valve_WON/ghost.run", "not a demo");

    let mut listed = names_of(runs_scan::scan(&root, relative, Some(".dem")));
    listed.sort();
    assert_eq!(listed, ["RUN.DEM", "c1a0.dem"]);
    assert!(matches!(
        runs_scan::delete_item(&root, relative, "config.cfg", Some(".dem")),
        DeleteResult::Error { .. }
    ));
    assert!(matches!(
        runs_scan::delete_item(&root, relative, "ghost.run", Some(".dem")),
        DeleteResult::Error { .. }
    ));
    assert_eq!(
        runs_scan::resolve_item(&root, relative, "ghost.run", Some(".dem")),
        None
    );
    assert_eq!(
        runs_scan::delete_item(&root, relative, "c1a0.dem", Some(".dem")),
        DeleteResult::Ok {
            name: "c1a0.dem".into()
        }
    );
    assert!(exists(&root, "Half-Life/valve_WON/config.cfg"));

    // Без расширения из конфига — по-прежнему записи Ghosting.
    assert_eq!(
        names_of(runs_scan::scan(&root, relative, None)),
        ["ghost.run"]
    );
    // Мусор вместо расширения не расширяет выборку.
    assert_eq!(
        names_of(runs_scan::scan(&root, relative, Some(""))),
        ["ghost.run"]
    );
}
