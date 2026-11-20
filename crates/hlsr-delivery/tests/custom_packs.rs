//! Самотест `custom-packs-selftest.js` 0.4.9 (моды игр в персонализации): каталог, доступность,
//! включение и выключение HL1 и Ghosting, сбой посреди копирования (снимки после 1-го, 8-го и
//! последнего файла), потерянное состояние, «только чтение», прослушивание; каналы
//! `custom-packs-*` фасада. Проверки исходников окна (Vue) — задача экранов (G10).

mod support;

use hlsr_core::json_text::stringify;
use hlsr_core::logger::MemoryLogger;
use hlsr_delivery::api::DeliveryApi;
use hlsr_delivery::custom_packs::{self as packs, GAMES, PACKS, Pack, STATE_DIR};
use hlsr_delivery::errors::{Failure, RawError};
use hlsr_delivery::game_actions::LauncherIdentity;
use hlsr_delivery::mutable_paths::matches_mutable;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use support::api::{Setup, harness};
use support::scenario::temp_root;

static LOG: LazyLock<MemoryLogger> = LazyLock::new(MemoryLogger::default);

fn pack(id: &str) -> &'static Pack {
    PACKS.iter().find(|pack| pack.id == id).unwrap()
}

fn viewmodels() -> &'static Pack {
    pack("arianon-viewmodels")
}
fn sounds() -> &'static Pack {
    pack("oxcjae-soundpack-lite")
}
fn hud() -> &'static Pack {
    pack("condensed-hud")
}
fn knife() -> &'static Pack {
    pack("m9-bayonet")
}

const SOURCE_DIR: &str = "Half-Life/valve_WON/hlsr-packs";
const TARGET_DIR: &str = "Half-Life/valve_WON";
const BASE_DIR: &str = "Half-Life/valve";

fn at(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |p, part| p.join(part))
}

fn write(root: &Path, relative: &str, text: &str) {
    let file = at(root, relative);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, text).unwrap();
}

fn read(root: &Path, relative: &str) -> String {
    fs::read_to_string(at(root, relative)).unwrap()
}

/// Папка игры: оригиналы и файлы всех модов игры в `hlsr-packs` (как после патча).
fn make_game(root: &Path, sources: bool) {
    write(root, "Half-Life/hl.exe", "exe");
    write(root, "ghostingmod/scripts/HudLayout.res", "stock layout");
    for pack in &PACKS {
        let game = packs::game_of(pack.game).unwrap();
        for file in pack.files {
            if let Some(base) = game.base_dir {
                write(root, &format!("{base}/{file}"), &format!("original {file}"));
            }
            if sources {
                let source = format!("{}/{}/{file}", game.source_dir, pack.id);
                write(root, &source, &format!("{} {file}", pack.id));
            }
        }
    }
}

/// Всё дерево папки: путь → содержимое (папки — пометкой).
fn tree(root: &Path) -> BTreeMap<String, String> {
    fn walk(dir: &Path, rel: &str, out: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            let next = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if entry.file_type().unwrap().is_dir() {
                out.insert(format!("{next}/"), "dir".into());
                walk(&entry.path(), &next, out);
            } else {
                let bytes = fs::read(entry.path()).unwrap();
                out.insert(next, String::from_utf8_lossy(&bytes).into_owned());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, "", &mut out);
    out
}

/// Дерево без служебной папки модов: так игра выглядит для движка.
fn game_tree(root: &Path) -> BTreeMap<String, String> {
    tree(root)
        .into_iter()
        .filter(|(key, _)| !key.starts_with(".hlsr"))
        .collect()
}

fn list(root: Option<&Path>, game: &str) -> Vec<Value> {
    packs::list(root, game, false, &*LOG).unwrap()
}

fn enabled_of(root: &Path, game: &str) -> Value {
    Value::Object(
        list(Some(root), game)
            .iter()
            .map(|pack| {
                (
                    pack["id"].as_str().unwrap().to_owned(),
                    pack["enabled"].clone(),
                )
            })
            .collect(),
    )
}

fn set(root: &Path, pack: &Pack, enabled: bool) {
    packs::set_enabled(root, pack.id, enabled, &*LOG, None).unwrap();
}

fn code(error: Failure) -> String {
    match error {
        Failure::Delivery(error) => error.code.as_str().to_owned(),
        Failure::Raw(error) => error.code.unwrap_or_default(),
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn catalog_hl1_paths_lowercase_and_packs_do_not_overlap() {
    let mut seen = std::collections::HashSet::new();
    for target in packs::targets() {
        assert!(seen.insert(target.to_lowercase()), "{target} в двух модах");
        if let Some(relative) = target.strip_prefix(&format!("{TARGET_DIR}/")) {
            assert_eq!(relative, relative.to_lowercase());
        }
    }
    let sizes: Vec<usize> = list(None, "hl1-won")
        .iter()
        .map(|pack| pack["files"].as_array().unwrap().len())
        .collect();
    assert_eq!(sizes, [15, 23, 76]);
}

#[test]
fn catalog_ghosting_whole_mod_in_custom_and_knife_without_world_model() {
    let ghosting: Vec<(String, usize)> = list(None, "ghosting-renewed")
        .iter()
        .map(|pack| {
            (
                pack["id"].as_str().unwrap().to_owned(),
                pack["files"].as_array().unwrap().len(),
            )
        })
        .collect();
    let expected = [
        ("condensed-hud", 64),
        ("m9-bayonet", 10),
        ("signed-smg", 1),
        ("signed-shotgun", 1),
        ("signed-rpg", 1),
    ];
    assert_eq!(
        ghosting,
        expected.map(|(id, n)| (id.to_owned(), n)).to_vec()
    );
    let gh = &GAMES[1];
    let mut relative = Vec::new();
    for pack in PACKS.iter().filter(|pack| pack.game == "ghosting-renewed") {
        assert_eq!(
            gh.target_dir(pack),
            format!("ghostingmod/custom/{}", pack.id)
        );
        relative.extend(pack.files.iter().map(|file| file.to_lowercase()));
    }
    let unique: std::collections::HashSet<_> = relative.iter().collect();
    assert_eq!(
        unique.len(),
        relative.len(),
        "моды Ghosting не перекрывают файлы друг друга"
    );
    assert!(
        !knife()
            .files
            .iter()
            .any(|f| f.to_lowercase().contains("w_crowbar"))
    );
    for file in [
        "scripts/hudlayout.res",
        "scripts/hudanimations.txt",
        "resource/clientscheme.res",
        "resource/valve_russian.txt",
    ] {
        assert!(hud().files.contains(&file), "{file}");
    }
    assert!(packs::has_packs("ghosting-renewed"));
    assert!(!packs::has_packs("hl2-4260"));
}

#[test]
fn integrity_copies_are_customization_and_packs_in_hlsr_packs_are_checked() {
    for target in packs::targets() {
        assert!(matches_mutable(&target), "{target}");
    }
    for pack in &PACKS {
        let game = packs::game_of(pack.game).unwrap();
        for file in pack.files {
            let source = format!("{}/{}/{file}", game.source_dir, pack.id);
            assert!(!matches_mutable(&source), "{source}");
        }
    }
    assert!(!matches_mutable("Half-Life/valve_WON/models/v_chub.mdl"));
}

#[test]
fn availability_without_pack_files_switch_is_off_and_enable_fails() {
    let (_temp, root) = temp_root();
    make_game(&root, false);
    let available = |root: Option<&Path>, game: &str| -> Vec<bool> {
        list(root, game)
            .iter()
            .map(|pack| pack["available"].as_bool().unwrap())
            .collect()
    };
    assert_eq!(available(Some(&root), "hl1-won"), [false; 3]);
    assert_eq!(available(None, "ghosting-renewed"), [false; 5]);
    for pack in [viewmodels(), hud()] {
        let error = packs::set_enabled(&root, pack.id, true, &*LOG, None).unwrap_err();
        assert_eq!(code(error), "CUSTOM_PACK_NOT_AVAILABLE");
    }
    assert!(!at(&root, STATE_DIR).exists());

    let (_temp2, full) = temp_root();
    make_game(&full, true);
    assert_eq!(available(Some(&full), "hl1-won"), [true; 3]);
    fs::remove_file(at(
        &full,
        &format!("{SOURCE_DIR}/{}/{}", sounds().id, sounds().files[40]),
    ))
    .unwrap();
    assert_eq!(available(Some(&full), "hl1-won"), [true, true, false]);
    let error = packs::set_enabled(&full, "nope", true, &*LOG, None).unwrap_err();
    assert_eq!(code(error), "CUSTOM_PACK_UNKNOWN");
}

#[test]
fn hl1_enable_copies_into_valve_won_and_disable_restores_everything() {
    let (_temp, root) = temp_root();
    make_game(&root, true);
    // Свой файл игрока на месте одной из копий: уходит в резерв и возвращается.
    write(
        &root,
        &format!("{TARGET_DIR}/models/v_crowbar.mdl"),
        "my crowbar",
    );
    let before = game_tree(&root);

    set(&root, viewmodels(), true);
    set(&root, sounds(), true);
    assert_eq!(
        enabled_of(&root, "hl1-won"),
        json!({"arianon-viewmodels": true, "item-sprites": false, "oxcjae-soundpack-lite": true})
    );
    for pack in [viewmodels(), sounds()] {
        for file in pack.files {
            assert_eq!(
                read(&root, &format!("{TARGET_DIR}/{file}")),
                format!("{} {file}", pack.id)
            );
            assert_eq!(
                read(&root, &format!("{BASE_DIR}/{file}")),
                format!("original {file}")
            );
        }
    }

    set(&root, viewmodels(), true); // повторное включение ничего не меняет
    set(&root, viewmodels(), false);
    set(&root, sounds(), false);
    assert_eq!(
        game_tree(&root),
        before,
        "копии убраны, свой файл на месте, пустых папок нет"
    );
    let left: Vec<_> = fs::read_dir(at(&root, STATE_DIR))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(left, ["state.json"], "резервных копий не осталось");
}

#[test]
fn ghosting_pack_lies_as_folder_in_custom_and_game_files_stay() {
    let (_temp, root) = temp_root();
    make_game(&root, true);
    let before = game_tree(&root);
    set(&root, hud(), true);
    set(&root, knife(), true);
    assert_eq!(
        enabled_of(&root, "ghosting-renewed"),
        json!({"condensed-hud": true, "m9-bayonet": true, "signed-smg": false, "signed-shotgun": false, "signed-rpg": false})
    );
    let mut custom: Vec<_> = fs::read_dir(at(&root, "ghostingmod/custom"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    custom.sort();
    assert_eq!(custom, ["condensed-hud", "m9-bayonet"]);
    let now = game_tree(&root);
    assert!(
        now.keys()
            .filter(|key| !before.contains_key(*key))
            .all(|key| key.starts_with("ghostingmod/custom/")),
        "новое — только в custom"
    );
    assert_eq!(
        read(
            &root,
            "ghostingmod/custom/condensed-hud/scripts/hudlayout.res"
        ),
        "condensed-hud scripts/hudlayout.res"
    );
    assert_eq!(
        read(&root, "ghostingmod/scripts/HudLayout.res"),
        "stock layout"
    );

    set(&root, hud(), false);
    set(&root, knife(), false);
    assert_eq!(
        game_tree(&root),
        before,
        "папки модов и пустая custom убраны"
    );
}

#[test]
fn lost_state_pack_is_adopted_as_enabled_and_turns_off() {
    let (_temp, root) = temp_root();
    make_game(&root, true);
    let before = game_tree(&root);
    set(&root, hud(), true);
    set(&root, viewmodels(), true);
    fs::remove_dir_all(at(&root, STATE_DIR)).unwrap();

    assert_eq!(enabled_of(&root, "ghosting-renewed")[hud().id], true);
    assert_eq!(enabled_of(&root, "hl1-won")[viewmodels().id], true);
    // Копия, которую поменяли, — уже не мод: такой пак не считается включённым.
    let v357 = format!("{TARGET_DIR}/models/v_357.mdl");
    write(&root, &v357, "edited");
    assert_eq!(enabled_of(&root, "hl1-won")[viewmodels().id], false);
    write(
        &root,
        &v357,
        &format!("{} models/v_357.mdl", viewmodels().id),
    );

    set(&root, hud(), false);
    set(&root, viewmodels(), false);
    assert_eq!(game_tree(&root), before);
}

fn set_readonly(file: &Path, readonly: bool) {
    let mut permissions = fs::metadata(file).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(readonly);
    fs::set_permissions(file, permissions).unwrap();
}

#[test]
fn read_only_pack_files_enable_and_disable_and_copies_are_writable() {
    let (_temp, root) = temp_root();
    make_game(&root, true);
    let source = |file: &str| at(&root, &format!("{SOURCE_DIR}/{}/{file}", sounds().id));
    for file in sounds().files {
        set_readonly(&source(file), true);
    }
    let mine = sounds().files[0];
    let mine_path = at(&root, &format!("{TARGET_DIR}/{mine}"));
    write(&root, &format!("{TARGET_DIR}/{mine}"), "my sound");
    set_readonly(&mine_path, true);
    let before = game_tree(&root);

    set(&root, sounds(), true);
    // На Windows атрибут переносится с копией; Linux меняет режим только у своих файлов.
    if cfg!(windows) {
        for file in sounds().files {
            let copy = at(&root, &format!("{TARGET_DIR}/{file}"));
            assert!(
                !fs::metadata(copy).unwrap().permissions().readonly(),
                "{file}"
            );
        }
    }
    set(&root, sounds(), false);
    assert_eq!(game_tree(&root), before);
    assert_eq!(enabled_of(&root, "hl1-won")[sounds().id], false);
    for file in sounds().files {
        set_readonly(&source(file), false);
    }
    set_readonly(&mine_path, false);
}

#[test]
fn hl1_disable_keeps_file_edited_after_enable() {
    let (_temp, root) = temp_root();
    make_game(&root, true);
    set(&root, viewmodels(), true);
    write(
        &root,
        &format!("{TARGET_DIR}/models/v_357.mdl"),
        "edited later",
    );
    set(&root, viewmodels(), false);
    assert_eq!(
        read(&root, &format!("{TARGET_DIR}/models/v_357.mdl")),
        "edited later"
    );
    assert!(!at(&root, &format!("{TARGET_DIR}/models/v_crowbar.mdl")).exists());
    assert_eq!(enabled_of(&root, "hl1-won")[viewmodels().id], false);
}

/// Снимки папки после 1-го, 8-го и последнего файла: «питание пропало здесь».
fn snapshots(
    root: &Path,
    total: usize,
    run: impl FnOnce(packs::AfterFile) -> Result<Vec<Value>, Failure>,
) -> Vec<(tempfile::TempDir, PathBuf)> {
    let mut shots = Vec::new();
    let mut index = 0;
    let mut hook = |_: &str| -> Result<(), Failure> {
        index += 1;
        if [1, 8, total].contains(&index) {
            let (temp, copy) = temp_root();
            copy_dir(root, &copy);
            shots.push((temp, copy));
        }
        Ok(())
    };
    run(&mut hook).unwrap();
    shots
}

#[test]
fn crash_during_enable_next_call_rolls_back() {
    for pack in [viewmodels(), knife()] {
        let (_temp, root) = temp_root();
        make_game(&root, true);
        if pack.id == viewmodels().id {
            write(
                &root,
                &format!("{TARGET_DIR}/models/v_gauss.mdl"),
                "my gauss",
            );
        }
        let before = game_tree(&root);
        let shots = snapshots(&root, pack.files.len(), |hook| {
            packs::set_enabled(&root, pack.id, true, &*LOG, Some(hook))
        });
        // Последний снимок — после всех копий, но до записи «включено»: тоже откат.
        assert_eq!(shots.len(), 3);
        for (_keep, shot) in &shots {
            assert_ne!(game_tree(shot), before, "в снимке уже часть копий");
            assert!(packs::recover(shot, &*LOG).unwrap());
            assert_eq!(game_tree(shot), before, "{}", pack.id);
            assert_eq!(enabled_of(shot, pack.game)[pack.id], false);
            assert!(
                !packs::recover(shot, &*LOG).unwrap(),
                "второй раз делать нечего"
            );
        }
    }
}

#[test]
fn crash_during_disable_next_call_finishes_it() {
    for pack in [viewmodels(), knife()] {
        let (_temp, root) = temp_root();
        make_game(&root, true);
        if pack.id == viewmodels().id {
            write(&root, &format!("{TARGET_DIR}/models/v_rpg.mdl"), "my rpg");
        }
        let before = game_tree(&root);
        set(&root, pack, true);
        let shots = snapshots(&root, pack.files.len(), |hook| {
            packs::set_enabled(&root, pack.id, false, &*LOG, Some(hook))
        });
        for (_keep, shot) in &shots {
            // Список с recover сам доделывает операцию (так его зовёт вкладка при закрытой игре).
            let listed = packs::list(Some(shot), pack.game, true, &*LOG).unwrap();
            let item = listed.iter().find(|item| item["id"] == pack.id).unwrap();
            assert_eq!(item["enabled"], false);
            assert_eq!(game_tree(shot), before, "{}", pack.id);
        }
        assert_eq!(game_tree(&root), before);
    }
}

#[test]
fn error_during_enable_rolls_back_at_once() {
    let (_temp, root) = temp_root();
    make_game(&root, true);
    let before = game_tree(&root);
    let mut count = 0;
    let mut hook = |_: &str| -> Result<(), Failure> {
        count += 1;
        if count == 5 {
            let mut error = RawError::new("диск полон");
            error.code = Some("ENOSPC".into());
            return Err(error.into());
        }
        Ok(())
    };
    let error = packs::set_enabled(&root, sounds().id, true, &*LOG, Some(&mut hook)).unwrap_err();
    assert_eq!(code(error), "ENOSPC");
    assert_eq!(game_tree(&root), before);
    assert_eq!(enabled_of(&root, "hl1-won")[sounds().id], false);
    assert!(!packs::recover(&root, &*LOG).unwrap(), "журнал закрыт");
}

/// Ключи `files` журнала вне списка пака не трогаются: испорченный `state.json` не удалит
/// чужой файл в папке игры.
#[test]
fn recover_touches_only_files_of_the_pack() {
    use sha2::{Digest, Sha256};
    let (_temp, root) = temp_root();
    make_game(&root, true);
    write(&root, &format!("{TARGET_DIR}/liblist.gam"), "game");
    let sha256: String = Sha256::digest(b"game")
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let state = json!({"packs": {}, "pending": {"id": viewmodels().id, "op": "disable",
        "files": {"liblist.gam": {"sha256": sha256, "backup": false}}}});
    write(
        &root,
        &format!("{STATE_DIR}/state.json"),
        &state.to_string(),
    );
    assert!(packs::recover(&root, &*LOG).unwrap());
    assert_eq!(read(&root, &format!("{TARGET_DIR}/liblist.gam")), "game");
}

#[test]
fn listening_original_from_valve_and_replacement_from_pack_only() {
    use base64::Engine as _;
    let (_temp, root) = temp_root();
    make_game(&root, true);
    let file = sounds().files[3];
    let decode = |url: String| {
        let data = url
            .strip_prefix("data:audio/wav;base64,")
            .unwrap()
            .to_owned();
        String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap(),
        )
        .unwrap()
    };
    let sound = |id: &str, file: &str, which: &str| packs::read_sound(&root, id, file, Some(which));
    assert_eq!(
        decode(sound(sounds().id, file, "original").unwrap()),
        format!("original {file}")
    );
    assert_eq!(
        decode(sound(sounds().id, file, "pack").unwrap()),
        format!("{} {file}", sounds().id)
    );
    for (id, file, which) in [
        (sounds().id, "../../hl.exe", "pack"),
        (sounds().id, file, "other"),
        (viewmodels().id, viewmodels().files[0], "pack"),
        (hud().id, hud().files[0], "pack"),
    ] {
        assert_eq!(
            code(sound(id, file, which).unwrap_err()),
            "CUSTOM_PACK_FILE_UNKNOWN",
            "{file}"
        );
    }
}

#[test]
fn hud_preview_finds_the_mod_font_in_custom_only_after_enable() {
    use base64::Engine as _;
    use hlsr_delivery::ghosting_assets::read_preview_asset;
    let (_temp, root) = temp_root();
    make_game(&root, true);
    // Наименьший TTF, который пропускает проверка: заголовок и одна пустая таблица.
    let mut ttf = vec![0u8; 28];
    ttf[..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    ttf[4..6].copy_from_slice(&1u16.to_be_bytes());
    ttf[20..24].copy_from_slice(&28u32.to_be_bytes());
    let source = format!(
        "ghostingmod/hlsr-packs/{}/resource/din1451alt2.ttf",
        hud().id
    );
    fs::write(at(&root, &source), &ttf).unwrap();
    let install = root.to_string_lossy();
    let read = || read_preview_asset(&install, "font", "resource/din1451alt2.ttf", None);
    assert_eq!(read().unwrap_err().code, "ASSET_READ_FAILED");
    set(&root, hud(), true);
    let expected = format!(
        "data:font/ttf;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&ttf)
    );
    assert_eq!(read().unwrap().data_url, expected);
}

// ---------------------------------------------------------------- каналы custom-packs-*

fn installed(root: &Path, game_id: &str) -> PathBuf {
    let install_path = root.join("lib").join("Games").join(game_id);
    make_game(&install_path, true);
    let installed = json!({
        "schemaVersion": 1,
        "gameId": game_id,
        "version": "1.0.9",
        "state": "ready",
        "installPath": install_path.to_string_lossy(),
        "installedAt": "2026-10-16T12:00:00.000Z",
    });
    fs::write(
        install_path.join("installed.json"),
        stringify(&installed, "  "),
    )
    .unwrap();
    install_path
}

async fn call(api: &DeliveryApi, channel: &str, args: Value) -> Value {
    api.call(channel, args.as_array().unwrap()).await.unwrap()
}

#[tokio::test]
async fn channels_list_switch_and_listen() {
    let (_temp, root) = temp_root();
    let h = harness(Setup {
        root: root.clone(),
        registry: None,
        now: 1_792_152_000_000,
        launcher: LauncherIdentity {
            exec_path: PathBuf::from("/opt/hlsr/hlsr"),
            app_image: None,
            packaged: true,
            app_path: None,
        },
    });
    let api = &h.api;
    assert_eq!(
        call(api, "custom-packs-get", json!(["hl2-4260"])).await,
        json!({"status": "error", "code": "UNKNOWN_GAME"})
    );
    let missing = call(api, "custom-packs-get", json!(["hl1-won"])).await;
    assert_eq!(missing["installed"], false);
    assert_eq!(missing["packs"].as_array().unwrap().len(), 3);
    let set_missing = call(
        api,
        "custom-packs-set",
        json!(["hl1-won", "item-sprites", true]),
    )
    .await;
    assert_eq!(set_missing["code"], "NOT_INSTALLED");

    let install = installed(&root, "hl1-won");
    let listed = call(api, "custom-packs-get", json!(["hl1-won"])).await;
    assert_eq!(listed["status"], "ok");
    assert_eq!(listed["installed"], true);
    assert_eq!(
        listed["packs"][0],
        json!({"id": "arianon-viewmodels", "kind": "models", "author": "Arianon",
            "files": viewmodels().files, "available": true, "enabled": false})
    );
    assert_eq!(
        call(
            api,
            "custom-packs-set",
            json!(["hl1-won", "condensed-hud", true])
        )
        .await,
        json!({"status": "error", "code": "CUSTOM_PACK_UNKNOWN"})
    );
    let on = call(
        api,
        "custom-packs-set",
        json!(["hl1-won", "item-sprites", true]),
    )
    .await;
    assert_eq!(on["status"], "ok");
    assert_eq!(on["packs"][1]["enabled"], true);
    assert_eq!(
        read(&install, "Half-Life/valve_WON/models/w_357.mdl"),
        "item-sprites models/w_357.mdl"
    );

    let file = sounds().files[0];
    let sound = call(
        api,
        "custom-packs-sound",
        json!(["hl1-won", sounds().id, file, "pack"]),
    )
    .await;
    assert!(
        sound["dataUrl"]
            .as_str()
            .unwrap()
            .starts_with("data:audio/wav;base64,")
    );
    fs::remove_file(at(&install, &format!("{BASE_DIR}/{file}"))).unwrap();
    assert_eq!(
        call(
            api,
            "custom-packs-sound",
            json!(["hl1-won", sounds().id, file, "original"])
        )
        .await,
        json!({"status": "error", "code": "CUSTOM_PACK_NOT_AVAILABLE"})
    );
    assert_eq!(
        call(
            api,
            "custom-packs-sound",
            json!(["hl1-won", "condensed-hud", file, "pack"])
        )
        .await,
        json!({"status": "error", "code": "CUSTOM_PACK_UNKNOWN"})
    );
    assert_eq!(
        call(
            api,
            "custom-packs-sound",
            json!(["ghosting-renewed", hud().id, "x", "pack"])
        )
        .await,
        json!({"status": "error", "code": "NOT_INSTALLED"})
    );

    // Код настроек: моды с диска входят в код и выходят из него отдельно от вида.
    let exported = call(
        api,
        "game-personalization-code-export",
        json!(["hl1-won", {}]),
    )
    .await;
    let imported = call(
        api,
        "game-personalization-code-import",
        json!(["hl1-won", exported["code"]]),
    )
    .await;
    assert_eq!(imported["status"], "ok", "{imported}");
    assert_eq!(
        imported["mods"],
        json!({"arianon-viewmodels": false, "item-sprites": true, "oxcjae-soundpack-lite": false})
    );
    assert!(imported["look"].get("mods").is_none());
}
