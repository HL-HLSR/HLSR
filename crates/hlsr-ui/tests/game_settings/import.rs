//! Импорт cfg во вкладку «Конфиги» (`GameSettingsModal.importConfig`, цепочка `exec` —
//! `hl1-selftest.js`): окна выбора файла, правки, отчёт в подвале.

use super::*;

fn picked(name: &str, text: &str) -> Value {
    json!({"status": "ok", "name": name, "dir": "D:/cfgs", "text": text,
        "files": [{"name": name, "dir": "D:/cfgs", "text": text}]})
}

fn chained(name: &str, text: &str) -> Value {
    json!({"status": "ok", "name": name, "dir": "D:/cfgs", "text": text})
}

/// Окно Ghosting с ответами окон выбора файла по очереди.
fn import_rig(picks: Vec<Value>) -> Rig {
    let scene = with_invoke(
        "game-settings-configs",
        json!({"player-config-import-pick": {"$seq": picks}}),
    );
    let rig = rig(&scene);
    rig.mount();
    rig
}

/// Итог импорта — заметка подвала раздела «Конфиг».
fn note(rig: &Rig) -> (bool, String, String) {
    rig.session.select_section("configs");
    let footer = rig.session.view().footer;
    (footer.note_error, footer.note, footer.note_details)
}

#[test]
fn settings_from_a_config_and_its_exec_chain_become_edits() {
    let root = "sensitivity \"4\"\nvolume \"0.5\"\nfps_max \"250\"\nexec extras.cfg\nbind \"x\" \"exec later.cfg\"\n";
    let rig = import_rig(vec![
        picked("config.cfg", root),
        chained("extras.cfg", "sensitivity \"5\"\n"),
    ]);
    rig.session.import_config();
    // Окон выбора два: сам конфиг и extras.cfg; exec внутри bind по клавише не спрашивается.
    let picks = rig.calls_to("player-config-import-pick");
    assert_eq!(picks.len(), 2);
    assert_eq!(picks[0], json!(["218", {"multiple": true}]));
    assert_eq!(
        picks[1],
        json!(["218", {"want": {"name": "extras.cfg", "from": "config.cfg"}, "dir": "D:/cfgs"}])
    );
    // Последнее значение побеждает; проценты — целые.
    let edits = rig.session.edits();
    assert_eq!(edits["sensitivity"], json!(5));
    assert_eq!(edits["volume"], json!(50));
    assert_eq!(edits["fps_max"], json!(250));
    // Чувствительность задана дважды — отчёт говорит, какой файл победил (в подсказке).
    let (error, text, details) = note(&rig);
    assert!(!error);
    assert_eq!(
        text,
        "3 settings taken from config.cfg. Applies after “Save”. Files from exec: 1. Set more than once, the last one wins: sensitivity."
    );
    assert_eq!(details, "sensitivity: config.cfg, extras.cfg → 5");
    assert!(!rig.session.view().import_busy);
}

#[test]
fn a_file_that_matches_the_current_settings_changes_nothing() {
    let rig = import_rig(vec![picked(
        "config.cfg",
        "sensitivity \"3\"\nvolume \"0.2\"\nfps_max \"999\"\n",
    )]);
    rig.session.import_config();
    assert!(rig.session.edits().is_empty());
    let (error, text, _) = note(&rig);
    assert!(!error);
    assert_eq!(
        text,
        "Settings in config.cfg already match the current ones."
    );
}

#[test]
fn an_import_over_unsaved_edits_replaces_them_and_drops_values_equal_to_the_cfg() {
    let rig = import_rig(vec![picked("a.cfg", "sensitivity \"3\"\nvolume \"0.5\"\n")]);
    // Правки до импорта: чувствительность 7 (в cfg 3), громкость 80 (в cfg 20).
    rig.session.config_set_number("sensitivity", "7");
    rig.session.config_set_number("volume", "80");
    rig.session.import_config();
    let edits = rig.session.edits();
    // 3 равно записанному в cfg — правка снята; громкость стала 50.
    assert!(!edits.contains_key("sensitivity"));
    assert_eq!(edits["volume"], json!(50));
    let (_, text, _) = note(&rig);
    assert_eq!(text, "2 settings taken from a.cfg. Applies after “Save”.");
}

#[test]
fn a_file_without_settings_of_this_tab_is_reported() {
    let rig = import_rig(vec![picked(
        "autoexec.cfg",
        "bind \"x\" \"+jump\"\nalias go \"say hi\"\n",
    )]);
    rig.session.import_config();
    assert!(rig.session.edits().is_empty());
    let (error, text, _) = note(&rig);
    assert!(error);
    assert_eq!(text, "autoexec.cfg has no settings from this tab.");
}

#[test]
fn values_the_catalog_cannot_read_are_listed_as_not_accepted() {
    let rig = import_rig(vec![picked(
        "c.cfg",
        "sensitivity \"abc\"\nvolume \"0.5\"\n",
    )]);
    rig.session.import_config();
    assert_eq!(rig.session.edits()["volume"], json!(50));
    let (error, text, _) = note(&rig);
    assert!(!error);
    assert_eq!(
        text,
        "1 settings taken from c.cfg. Applies after “Save”. Values not accepted: sensitivity."
    );
}

#[test]
fn a_chain_file_the_player_declines_disables_its_exec() {
    let rig = import_rig(vec![
        picked("config.cfg", "volume \"0.5\"\nexec skipped.cfg\n"),
        json!({"status": "canceled"}),
    ]);
    rig.session.import_config();
    let (error, text, _) = note(&rig);
    assert!(!error);
    assert_eq!(
        text,
        "1 settings taken from config.cfg. Applies after “Save”. Not picked, their exec is disabled: skipped.cfg."
    );
    // Третьего окна выбора файла не было.
    assert_eq!(rig.calls_to("player-config-import-pick").len(), 2);
}

#[test]
fn closing_the_first_dialog_leaves_the_tab_as_it_was() {
    let rig = import_rig(vec![json!({"status": "canceled"})]);
    rig.session.import_config();
    assert!(rig.session.edits().is_empty());
    assert_eq!(note(&rig), (false, String::new(), String::new()));
    assert!(!rig.session.view().import_busy);
}

#[test]
fn unreadable_files_get_their_own_messages() {
    let cases = [
        (
            json!({"status": "error", "code": "BINDS_FILE_TOO_LARGE"}),
            "The file is larger than 1 MB, this is not a cfg.",
        ),
        (
            json!({"status": "error", "code": "BINDS_FILE_UNREADABLE"}),
            "Could not read the file.",
        ),
        (json!({"$reject": "boom"}), "Could not read the file."),
    ];
    for (reply, expected) in cases {
        let rig = import_rig(vec![reply]);
        rig.session.import_config();
        assert_eq!(note(&rig), (true, expected.to_owned(), String::new()));
    }
}

#[test]
fn several_files_picked_at_once_run_in_the_order_of_the_choice() {
    let first = json!({"status": "ok", "name": "a.cfg", "dir": "D:/cfgs", "text": "volume \"0.5\"\n",
    "files": [
        {"name": "a.cfg", "dir": "D:/cfgs", "text": "volume \"0.5\"\n"},
        {"name": "b.cfg", "dir": "D:/cfgs", "text": "volume \"0.7\"\nsensitivity \"4\"\n"},
    ]});
    let rig = import_rig(vec![first]);
    rig.session.import_config();
    let edits = rig.session.edits();
    assert_eq!(edits["volume"], json!(70));
    assert_eq!(edits["sensitivity"], json!(4));
    let (_, text, _) = note(&rig);
    assert!(
        text.starts_with("2 settings taken from a.cfg, b.cfg."),
        "{text}"
    );
}

#[test]
#[ignore = "в 0.4.9 #L_GS_CFG_RUNNING разрешает сохранять при запущенной игре (exec %0%), перенос — G5"]
fn import_is_unavailable_while_the_game_is_running() {
    let scene = with_invoke(
        "game-settings-configs",
        json!({
            "game-config-read": {"$args": [[["218", null], {"status": "ok", "gameId": "ghosting-renewed", "engine": "source", "running": true, "values": {}, "sources": {}}]]},
            "player-config-import-pick": picked("c.cfg", "volume \"0.5\"\n"),
        }),
    );
    let rig = rig(&scene);
    let view = rig.mount();
    assert!(view.editor.as_ref().is_some_and(|editor| editor.disabled));
    assert!(!view.alert_error);
    assert_eq!(
        view.alert,
        "The game is running. Close it to save these settings: on exit it rewrites config.cfg."
    );
    rig.session.import_config();
    assert!(rig.calls_to("player-config-import-pick").is_empty());
    assert!(view.editor.unwrap().disabled);
}
