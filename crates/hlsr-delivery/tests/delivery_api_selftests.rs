//! Самотесты 0.4.7 (настройки Ghosting — 0.4.9: файл игрока) уровня фасада доставки, которых нет в сценариях `delivery-api.json`:
//! `ghosting-integration-selftest.js` (настройки Ghosting, коды персонализации, предпросмотр и
//! выбор файлов), `linux-selftest.js` (ярлык AppImage), `ghost-runs-selftest.js` (тест 3:
//! запущенная игра не даёт удалить запись), а также каналы библиотеки и загрузок из
//! `AppManager.js` (у них нет фикстуры: AppManager — модуль ESM, его нельзя загрузить в сверку).

mod support;

use hlsr_core::json_text::stringify;
use hlsr_delivery::api::{DeliveryApi, LIBRARY_FOLDER_NAME, with_hlsr_folder};
use hlsr_delivery::game_actions::LauncherIdentity;
use hlsr_delivery::game_settings_store::GameSettingsStore;
use hlsr_delivery::library_path::default_library_path;
use hlsr_delivery::ports::Logger;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use support::api::{Harness, ListLogger, Setup, harness};
use support::scenario::temp_root;

const NOW: i64 = 1_792_152_000_000;
const GHOSTING: &str = "ghosting-renewed";
const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aGhsAAAAASUVORK5CYII=";

fn launcher(app_image: Option<String>) -> LauncherIdentity {
    LauncherIdentity {
        exec_path: PathBuf::from("/opt/hlsr/hlsr"),
        app_image,
        packaged: true,
        app_path: None,
    }
}

fn setup(root: &Path) -> Harness {
    harness(Setup {
        root: root.to_path_buf(),
        registry: None,
        now: NOW,
        launcher: launcher(None),
    })
}

/// Установленная игра без сети: `installed.json` и файлы.
fn install(root: &Path, game_id: &str, files: &[(&str, &str)]) -> PathBuf {
    let install_path = root.join("lib").join("Games").join(game_id);
    for (rel, text) in files {
        let full = install_path.join(rel);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    std::fs::create_dir_all(&install_path).unwrap();
    let installed = json!({
        "schemaVersion": 1,
        "gameId": game_id,
        "version": "1.0.9",
        "state": "ready",
        "installPath": install_path.to_string_lossy(),
        "installedAt": "2026-10-16T12:00:00.000Z",
    });
    std::fs::write(
        install_path.join("installed.json"),
        stringify(&installed, "  "),
    )
    .unwrap();
    install_path
}

fn ghosting(root: &Path) -> PathBuf {
    install(
        root,
        GHOSTING,
        &[
            ("hl2.exe", "exe\n"),
            ("ghostingmod/gameinfo.txt", "\"GameInfo\" {}\n"),
            ("ghostingmod/cfg/.keep", ""),
        ],
    )
}

/// `game-settings.json` напрямую (как `store.save` самотеста): без применения к игре.
fn store(root: &Path) -> GameSettingsStore {
    let logger: Arc<dyn Logger> = Arc::new(ListLogger {
        entries: Arc::default(),
    });
    GameSettingsStore::new(GameSettingsStore::default_file(&root.join("data")), logger)
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

async fn call(api: &DeliveryApi, channel: &str, args: Value) -> Value {
    let args = args.as_array().cloned().unwrap_or_default();
    api.call(channel, &args)
        .await
        .unwrap_or_else(|| panic!("нет канала {channel}"))
}

// ---------------------------------------------------------------- настройки Ghosting

#[tokio::test]
async fn ghosting_values_from_the_player_file_show_in_the_window_and_survive_save() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    let install_path = ghosting(&root);
    let cfg = install_path.join("ghostingmod/cfg");
    let player = cfg.join("autoexec.cfg");
    let store = store(&root);
    store
        .save(
            GHOSTING,
            &json!({"personalization": {"crossEnabled": true, "crossSize": 23}}),
        )
        .unwrap();
    std::fs::write(cfg.join("config.cfg"), "viewmodel_fov \"50\"\n").unwrap();
    std::fs::write(&player, "viewmodel_fov \"76\"\r\ngh_cross_size \"3\"\r\n").unwrap();

    let result = call(&h.api, "game-settings-get", json!(["218"])).await;
    assert_eq!(result["status"], "ok");
    assert_eq!(
        result["settings"]["personalization"]["crossSize"], 3,
        "файл игрока главнее сохранённого"
    );
    assert_eq!(
        result["settings"]["personalization"]["extra"]["viewmodel_fov"],
        76
    );
    assert_eq!(
        store.get(GHOSTING).to_value()["personalization"]["crossSize"],
        23,
        "открытие окна ничего не сохраняет"
    );

    let saved = call(
        &h.api,
        "game-settings-save",
        json!(["218", result["settings"].clone()]),
    )
    .await;
    assert_eq!(saved["status"], "ok");
    let written = read(&player);
    assert_eq!(written.matches("gh_cross_size").count(), 1, "{written}");
    assert!(written.contains("gh_cross_size \"3\""), "{written}");
    assert!(written.contains("viewmodel_fov \"76\""), "{written}");
    assert!(!cfg.join("hlsr-personalization.cfg").exists());
    std::fs::write(
        &player,
        written.replace("viewmodel_fov \"76\"", "viewmodel_fov \"93\""),
    )
    .unwrap();
    let again = call(&h.api, "game-settings-get", json!(["218"])).await;
    assert_eq!(
        again["settings"]["personalization"]["extra"]["viewmodel_fov"], 93,
        "правка руками видна сразу"
    );
}

#[tokio::test]
async fn ghosting_save_and_reset_with_a_stale_sha_are_refused_config_changed() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    let install_path = ghosting(&root);
    let file = install_path.join("ghostingmod/cfg/autoexec.cfg");
    let sha = || {
        use sha2::Digest;
        sha2::Sha256::digest(std::fs::read(&file).unwrap())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    std::fs::write(&file, "gh_cross_size \"3\"\r\n").unwrap();
    let store = store(&root);
    store
        .save(GHOSTING, &json!({"personalization": {"crossSize": 3}}))
        .unwrap();
    let opened = call(&h.api, "game-settings-get", json!(["218"])).await;
    assert_eq!(opened["configSha256"], json!(sha()));

    // Игрок правит файл руками, пока окно открыто; в окне эту настройку не трогали.
    std::fs::write(&file, "gh_cross_size \"3\"\r\nviewmodel_fov \"70\"\r\n").unwrap();
    let manual = read(&file);
    let mut draft = opened["settings"].clone();
    draft["personalization"]["crossSize"] = json!(14);
    let options = json!({"configSha256": opened["configSha256"]});
    let rejected = call(
        &h.api,
        "game-settings-save",
        json!(["218", draft, null, options.clone()]),
    )
    .await;
    assert_eq!(rejected["status"], "error");
    assert_eq!(rejected["error"]["code"], "CONFIG_CHANGED");
    assert_eq!(read(&file), manual);
    assert_eq!(
        store.get(GHOSTING).to_value()["personalization"]["crossSize"],
        3
    );
    let reset = call(&h.api, "game-settings-reset", json!(["218", options])).await;
    assert_eq!(reset["error"]["code"], "CONFIG_CHANGED");
    assert_eq!(read(&file), manual);

    // Окно перечиталось: свежий sha проходит и возвращается новый.
    let reopened = call(&h.api, "game-settings-get", json!(["218"])).await;
    assert_eq!(
        reopened["settings"]["personalization"]["extra"]["viewmodel_fov"],
        70
    );
    let mut draft = reopened["settings"].clone();
    draft["personalization"]["crossSize"] = json!(14);
    let saved = call(
        &h.api,
        "game-settings-save",
        json!(["218", draft, null, {"configSha256": reopened["configSha256"]}]),
    )
    .await;
    assert_eq!(saved["status"], "ok");
    assert_eq!(saved["configSha256"], json!(sha()));
    assert!(read(&file).contains("viewmodel_fov \"70\""));
    // Старый вызов без sha работает как раньше.
    let again = call(
        &h.api,
        "game-settings-save",
        json!(["218", saved["settings"].clone()]),
    )
    .await;
    assert_eq!(again["status"], "ok");
}

#[tokio::test]
async fn ghosting_reset_writes_defaults_to_the_player_file_and_does_not_resurrect_game_cfg() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    let install_path = ghosting(&root);
    let cfg = install_path.join("ghostingmod/cfg");
    let player = cfg.join("autoexec.cfg");
    call(
        &h.api,
        "game-settings-save",
        json!(["218", {"personalization": {"extra": {"viewmodel_fov": 91, "gh_autosave": false}}}]),
    )
    .await;
    // Файл игрока подготовлен шаблоном: строка `viewmodel_fov 62` шаблона правится на месте.
    let has = |value: &str| {
        let text = read(&player);
        text.contains(&format!("viewmodel_fov {value} "))
            || text.contains(&format!("viewmodel_fov \"{value}\""))
    };
    assert!(has("91"), "{}", read(&player));
    let result = call(&h.api, "game-settings-reset", json!(["218"])).await;
    assert_eq!(result["status"], "ok");
    let extra = &result["settings"]["personalization"]["extra"];
    assert_eq!(extra["viewmodel_fov"], 62);
    assert_eq!(extra["gh_autosave"], true);
    assert!(
        extra.get("gh_keyshud").is_none(),
        "неуправляемые настройки не появляются"
    );
    assert!(has("62"), "{}", read(&player));
    std::fs::write(cfg.join("config.cfg"), "viewmodel_fov \"91\"\n").unwrap();
    let again = call(&h.api, "game-settings-get", json!(["218"])).await;
    assert_eq!(
        again["settings"]["personalization"]["extra"]["viewmodel_fov"],
        62
    );
}

#[tokio::test]
async fn personalization_code_updates_draft_only_and_save_writes_the_player_file() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    let install_path = ghosting(&root);
    let store = store(&root);
    store
        .save(
            GHOSTING,
            &json!({"launchArgs": "-console", "rinput": false, "language": "en",
                    "personalization": {"extra": {"gh_timer": true, "gh_keyshud": true}}}),
        )
        .unwrap();
    let file = install_path.join("ghostingmod/cfg/autoexec.cfg");
    std::fs::write(&file, "gh_timer \"1\"\n").unwrap();
    let before = read(store.file());

    let exported = call(
        &h.api,
        "game-personalization-code-export",
        json!(["218", {"personalization": {
            "hudEnabled": true,
            "hudColor": {"r": 10, "g": 160, "b": 180},
            "extra": {"gh_health_x": 111, "viewmodel_fov": 81, "gh_timer": false},
        }}]),
    )
    .await;
    assert_eq!(exported["status"], "ok");
    let code = exported["code"].clone();
    let imported = call(
        &h.api,
        "game-personalization-code-import",
        json!(["218", code]),
    )
    .await;
    assert_eq!(imported["status"], "ok");
    let wrong = call(
        &h.api,
        "game-personalization-code-import",
        json!(["70", exported["code"]]),
    )
    .await;
    assert_eq!(wrong["error"]["code"], "PERSONALIZATION_CODE_WRONG_GAME");
    assert_eq!(read(&file), "gh_timer \"1\"\n", "импорт ещё не меняет игру");
    assert_eq!(read(store.file()), before);

    let saved = call(
        &h.api,
        "game-settings-save",
        json!(["218", imported["look"].clone(), "personalization"]),
    )
    .await;
    assert_eq!(saved["status"], "ok");
    assert_eq!(saved["settings"]["launchArgs"], "-console");
    assert_eq!(saved["settings"]["rinput"], false);
    assert_eq!(saved["settings"]["language"], "en");
    let cfg = read(&file);
    assert!(cfg.contains("gh_health_x \"111\""), "{cfg}");
    assert!(cfg.contains("viewmodel_fov \"81\""), "{cfg}");
    assert!(
        cfg.lines().any(|line| line == "gh_timer \"0\""),
        "выключенное у автора не остаётся включённым: {cfg}"
    );
    assert_eq!(
        cfg.lines()
            .filter(|line| line.starts_with("gh_timer \""))
            .count(),
        1,
        "строка игрока правится на месте"
    );
    assert!(
        cfg.contains("gh_keyshud \"0\""),
        "поля, которых нет у автора, — по умолчанию"
    );
}
#[tokio::test]
async fn personalization_scope_keeps_current_launch_settings_and_binds() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    ghosting(&root);
    install(&root, "hl1-won", &[("Half-Life/hl.exe", "exe\n")]);
    let store = store(&root);
    let code = hlsr_delivery::personalization_code::encode(
        "218",
        &json!({"personalization": {"crossSize": 18}}),
    )
    .unwrap();
    let look = hlsr_delivery::personalization_code::decode("218", &json!(code))
        .unwrap()
        .to_value();
    store
        .save(
            GHOSTING,
            &json!({"launchArgs": "-console", "rinput": false, "language": "en",
                    "binds": {"space": {"command": "+duck", "base": "+jump"}},
                    "width": 1920, "height": 1080}),
        )
        .unwrap();
    let before = store.get(GHOSTING).to_value();
    let mut draft = look.clone();
    draft["launchArgs"] = json!("");
    draft["rinput"] = json!(true);
    draft["binds"] = json!({});
    let saved = call(
        &h.api,
        "game-settings-save",
        json!(["218", draft, "personalization"]),
    )
    .await;
    assert_eq!(saved["status"], "ok");
    assert_eq!(saved["settings"]["personalization"]["crossSize"], 18);
    let mut rest = saved["settings"].clone();
    rest["personalization"] = before["personalization"].clone();
    assert_eq!(rest, before);
    let scope = call(&h.api, "game-settings-save", json!(["218", {}, "unknown"])).await;
    assert_eq!(scope["error"]["code"], "SETTINGS_SCOPE_INVALID");

    store
        .save(
            "hl1-won",
            &json!({"launchArgs": "-console", "rinputMenuFix": true}),
        )
        .unwrap();
    let hl1 = json!({
        "personalization": {"hudEnabled": true},
        "packs": {"hud": "ag-hud", "crosshair": "crosshair-changer", "changerShape": "3px",
                  "changerColor": "pink"},
        "bxt": {"timer": true, "fov": 110},
    });
    let saved = call(
        &h.api,
        "game-settings-save",
        json!(["70", hl1, "personalization"]),
    )
    .await;
    assert_eq!(saved["status"], "ok");
    assert_eq!(saved["settings"]["packs"]["hud"], "ag-hud");
    assert_eq!(saved["settings"]["bxt"]["fov"], 110);
    assert_eq!(saved["settings"]["launchArgs"], "-console");
    assert_eq!(saved["settings"]["rinputMenuFix"], true);
}

// ---------------------------------------------------------------- файлы персонализации

#[tokio::test]
async fn preview_assets_are_served_only_for_imported_ghosting_assets() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    let install_path = ghosting(&root);
    let png = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(PNG)
            .unwrap()
    };
    let source = root.join("my spray.png");
    std::fs::write(&source, &png).unwrap();
    h.dialogs
        .answers
        .lock()
        .unwrap()
        .push_back(json!({"open": [source.to_string_lossy()]}));
    let imported = call(
        &h.api,
        "game-personalization-pick-asset",
        json!(["218", "spray"]),
    )
    .await;
    assert_eq!(imported["status"], "ok", "{imported}");
    let value = imported["value"].clone();

    let preview = call(
        &h.api,
        "game-personalization-preview-asset",
        json!(["218", {"kind": "spray", "value": value}]),
    )
    .await;
    assert_eq!(preview["status"], "ok");
    assert_eq!(
        preview["dataUrl"],
        format!("data:image/png;base64,{PNG}").as_str()
    );
    let code = |args: Value| {
        let api = h.api.clone();
        async move { call(&api, "game-personalization-preview-asset", args).await["code"].clone() }
    };
    assert_eq!(
        code(json!(["70", {"kind": "spray", "value": value}])).await,
        "INVALID_ASSET"
    );
    assert_eq!(
        code(json!(["218", {"kind": "executable", "value": "x"}])).await,
        "INVALID_ASSET"
    );
    assert_eq!(code(json!(["218", null])).await, "INVALID_ASSET");
    assert_eq!(
        code(json!(["218", {"kind": "spray", "value": "../outside.png"}])).await,
        "ASSET_INVALID_PATH"
    );
    assert_eq!(
        code(json!(["218", {"kind": "font", "value": "Impact"}])).await,
        "ASSET_READ_FAILED"
    );
    std::fs::remove_file(install_path.join("installed.json")).unwrap();
    assert_eq!(
        code(json!(["218", {"kind": "spray", "value": value}])).await,
        "NOT_INSTALLED"
    );
}

#[tokio::test]
async fn asset_picker_rejects_unsupported_games_and_cancel_leaves_install_untouched() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    let install_path = ghosting(&root);
    let pick = |game: &str, kind: &str| {
        let api = h.api.clone();
        let args = json!([game, kind]);
        async move { call(&api, "game-personalization-pick-asset", args).await }
    };
    assert_eq!(pick("70", "font").await["status"], "error");
    assert_eq!(pick("218", "executable").await["status"], "error");
    assert!(h.take_effects().is_empty(), "диалог не открывался");
    assert_eq!(pick("218", "font").await["status"], "canceled");
    assert_eq!(h.take_effects().len(), 1);
    assert!(!install_path.join("ghostingmod/resource").exists());
}

// ---------------------------------------------------------------- ярлык и записи

#[cfg(unix)]
#[tokio::test]
async fn shortcut_uses_stable_appimage_path_and_game_argument() {
    use std::os::unix::fs::PermissionsExt;
    let (_temp, root) = temp_root();
    let app_image = root.join("HLSR 100%.AppImage");
    let h = harness(Setup {
        root: root.clone(),
        registry: None,
        now: NOW,
        launcher: launcher(Some(app_image.to_string_lossy().into_owned())),
    });
    ghosting(&root);
    let result = call(&h.api, "game-menu-create-shortcut", json!(["218"])).await;
    assert_eq!(result["status"], "ok", "{result}");
    let shortcut = PathBuf::from(result["shortcut"].as_str().unwrap());
    let entry = read(&shortcut);
    assert!(
        entry.contains("HLSR 100%%.AppImage\" --no-sandbox --select-game=ghosting-renewed"),
        "{entry}"
    );
    assert_eq!(
        std::fs::metadata(&shortcut).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let removed = call(
        &h.api,
        "game-menu-uninstall",
        json!(["218", {"confirmed": true}]),
    )
    .await;
    assert_eq!(removed["shortcut"]["removed"], true, "{removed}");
    assert!(!shortcut.exists());
}

#[tokio::test]
async fn running_game_prevents_run_deletion_and_stopped_game_permits_it() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    let install_path = install(
        &root,
        GHOSTING,
        &[
            ("ghostingmod/runs/one.run", "ghost one"),
            ("ghostingmod/runs/two.run", "ghost two"),
        ],
    );
    *h.processes.running.lock().unwrap() = vec![GHOSTING.to_owned()];
    let refused = call(&h.api, "game-runs-delete", json!(["218", "one.run"])).await;
    assert_eq!(refused, json!({"status": "error", "code": "GAME_RUNNING"}));
    assert!(install_path.join("ghostingmod/runs/one.run").exists());
    h.processes.running.lock().unwrap().clear();
    let deleted = call(&h.api, "game-runs-delete", json!(["218", "one.run"])).await;
    assert_eq!(deleted, json!({"status": "ok", "name": "one.run"}));
    assert!(!install_path.join("ghostingmod/runs/one.run").exists());
}

// ---------------------------------------------------------------- библиотека (AppManager)

#[test]
fn hlsr_folder_is_appended_once() {
    assert_eq!(LIBRARY_FOLDER_NAME, "HLSR");
    assert_eq!(with_hlsr_folder(""), "");
    if cfg!(unix) {
        assert_eq!(with_hlsr_folder(" /mnt/games "), "/mnt/games/HLSR");
        assert_eq!(with_hlsr_folder("/mnt/HLSR"), "/mnt/HLSR");
        assert_eq!(with_hlsr_folder("/mnt/hlsr/"), "/mnt/hlsr");
    } else {
        assert_eq!(with_hlsr_folder("D:\\Games"), "D:\\Games\\HLSR");
        assert_eq!(with_hlsr_folder("D:\\hlsr"), "D:\\hlsr");
    }
}

#[tokio::test]
async fn select_install_folder_appends_hlsr_and_cancel_is_null() {
    let (_temp, root) = temp_root();
    let mut h = setup(&root);
    let picked = root.join("Игры");
    h.dialogs
        .answers
        .lock()
        .unwrap()
        .push_back(json!({"open": [picked.to_string_lossy()]}));
    let current = root.join("old").to_string_lossy().into_owned();
    let chosen = call(&h.api, "select-install-folder", json!([current])).await;
    assert_eq!(
        chosen,
        json!(picked.join("HLSR").to_string_lossy()),
        "выбранная папка и HLSR в ней"
    );
    assert_eq!(
        h.take_effects(),
        vec![json!({"dialog": "open", "options": {
            "properties": ["openDirectory", "createDirectory"],
            "defaultPath": current,
        }})]
    );
    assert_eq!(
        call(&h.api, "select-install-folder", json!([""])).await,
        Value::Null
    );
    assert!(h.take_events().is_empty());
}

#[tokio::test]
async fn validate_install_path_rejects_bad_paths_and_probes_writes() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    let invalid = json!({"ok": false, "error": "invalid"});
    for raw in [json!(5), json!(null), json!("   "), json!("relative/dir")] {
        assert_eq!(
            call(&h.api, "validate-install-path", json!([raw.clone()])).await,
            invalid,
            "{raw}"
        );
    }
    if cfg!(unix) {
        assert_eq!(
            call(&h.api, "validate-install-path", json!(["/"])).await,
            invalid
        );
    } else {
        for raw in ["C:\\", "C:", "C:\\Windows\\HLSR", "D:\\Program Files\\HLSR"] {
            assert_eq!(
                call(&h.api, "validate-install-path", json!([raw])).await,
                invalid,
                "{raw}"
            );
        }
    }

    let target = root.join("Библиотека").join("HLSR");
    h.free_space.set(&target, Some(10 * 1024 * 1024 + 5));
    let raw = format!(" {}{}", target.display(), std::path::MAIN_SEPARATOR);
    let checked = call(&h.api, "validate-install-path", json!([raw])).await;
    let normalized = format!("{}{}", target.display(), std::path::MAIN_SEPARATOR);
    assert_eq!(
        checked,
        json!({"ok": true, "path": normalized, "freeSpaceMb": 10, "hasCyrillic": true})
    );
    assert!(target.is_dir());
    assert_eq!(
        std::fs::read_dir(&target).unwrap().count(),
        0,
        "пробный файл удалён"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn validate_install_path_reports_no_access() {
    use std::os::unix::fs::PermissionsExt;
    if support::scenario::is_root_user() {
        return;
    }
    let (_temp, root) = temp_root();
    let h = setup(&root);
    let locked = root.join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
    let inside = locked.join("HLSR");
    let result = call(
        &h.api,
        "validate-install-path",
        json!([inside.to_string_lossy()]),
    )
    .await;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result, json!({"ok": false, "error": "no-access"}));
    let log = h.take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0].0, "warn");
    assert!(
        log[0].1.starts_with("Library path is not creatable: "),
        "{log:?}"
    );

    let readonly = root.join("readonly");
    std::fs::create_dir(&readonly).unwrap();
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o500)).unwrap();
    let result = call(
        &h.api,
        "validate-install-path",
        json!([readonly.to_string_lossy()]),
    )
    .await;
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result, json!({"ok": false, "error": "no-access"}));
    assert!(
        h.take_log()[0]
            .1
            .starts_with("Library path is not writable: ")
    );
}

#[tokio::test]
async fn library_path_default_set_and_confirmed() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    let default = default_library_path().to_string_lossy().into_owned();
    let info = call(&h.api, "get-library-path", json!([])).await;
    assert_eq!(info["path"], default.as_str());
    assert_eq!(info["confirmed"], false);
    assert_eq!(info["isDefault"], true);

    h.api.ensure_default_install_path();
    assert_eq!(h.library.get_value("libraryPath"), json!(default));
    assert_eq!(
        h.take_log(),
        vec![(
            "info".to_owned(),
            format!("Default game library path proposed: {default}")
        )]
    );

    let target = root.join("games").join("HLSR");
    let set = call(
        &h.api,
        "set-library-path",
        json!([target.to_string_lossy()]),
    )
    .await;
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(
        h.library.get_value("libraryPath"),
        json!(target.to_string_lossy())
    );
    assert_eq!(h.library.get_value("libraryConfirmed"), json!(true));
    assert!(h.take_log().contains(&(
        "info".to_owned(),
        format!("Game library path set to: {}", target.display())
    )));
    let info = call(&h.api, "get-library-path", json!([])).await;
    assert_eq!(info["path"], json!(target.to_string_lossy()));
    assert_eq!(info["exists"], true);
    assert_eq!(info["confirmed"], true);
    assert_eq!(info["isDefault"], false);

    let refused = call(&h.api, "set-library-path", json!(["relative"])).await;
    assert_eq!(refused, json!({"ok": false, "error": "invalid"}));
    assert_eq!(
        h.library.get_value("libraryPath"),
        json!(target.to_string_lossy())
    );
}

#[tokio::test]
async fn existing_legacy_library_is_kept() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    let legacy = root.join("data").join("library");
    std::fs::create_dir_all(&legacy).unwrap();
    h.library
        .set_value("installed", json!({"218": {"version": "1.0.0"}}));
    h.api.ensure_default_install_path();
    assert_eq!(
        h.library.get_value("libraryPath"),
        json!(legacy.to_string_lossy())
    );
    assert_eq!(h.library.get_value("libraryConfirmed"), json!(true));
    assert_eq!(
        h.take_log(),
        vec![(
            "info".to_owned(),
            format!("Existing game library detected: {}", legacy.display())
        )]
    );
    // Путь уже выбран — второй вызов ничего не меняет.
    h.api.ensure_default_install_path();
    assert!(h.take_log().is_empty());
}

/// `shell.openExternal` в 0.4.7 не ждут: окну всегда `ok`; отказ ОС остаётся строкой журнала.
#[tokio::test]
async fn external_links_report_a_refusing_shell_in_the_log_only() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    let ok = call(&h.api, "open-discord", json!([])).await;
    assert_eq!(ok, json!({"status": "ok"}));
    assert!(h.take_log().is_empty());
    let first = h.take_effects();
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0]["shell"], "openExternal");

    *h.shell.open_external_failure.lock().unwrap() = "нет браузера".to_owned();
    for (channel, args) in [
        ("open-discord", json!([])),
        ("open-external-link", json!(["discord"])),
    ] {
        let reply = call(&h.api, channel, args).await;
        assert_eq!(reply, json!({"status": "ok"}), "{channel}");
        assert!(
            h.take_log().contains(&(
                "warn".to_owned(),
                "[links] ссылку открыть не удалось: нет браузера".to_owned()
            )),
            "{channel}"
        );
    }
}

#[tokio::test]
async fn open_games_folder_creates_and_opens_games() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    let lib = root.join("lib");
    h.library
        .set_value("libraryPath", json!(lib.to_string_lossy()));
    let games = lib.join("Games");
    let opened = call(&h.api, "open-games-folder", json!([])).await;
    assert_eq!(
        opened,
        json!({"status": "ok", "path": games.to_string_lossy()})
    );
    assert!(games.is_dir());
    assert_eq!(
        h.take_effects(),
        vec![json!({"shell": "openPath", "path": games.to_string_lossy()})]
    );

    *h.shell.open_path_failure.lock().unwrap() = "нет приложения".to_owned();
    let failed = call(&h.api, "open-games-folder", json!([])).await;
    assert_eq!(failed, json!({"status": "error", "code": "OPEN_FAILED"}));
    assert!(h.take_log().contains(&(
        "warn".to_owned(),
        format!(
            "[games-folder] не удалось открыть {}: нет приложения",
            games.display()
        )
    )));

    // Корень диска не годится в обеих ОС (в Windows «/» — корень текущего диска); относительный
    // путь недопустим только в Linux: в Windows он разрешается от текущего каталога, как в 0.4.7.
    let invalid_paths: &[&str] = if cfg!(windows) {
        &["/"]
    } else {
        &["/", "relative"]
    };
    for path in invalid_paths {
        h.library.set_value("libraryPath", json!(path));
        let invalid = call(&h.api, "open-games-folder", json!([])).await;
        assert_eq!(
            invalid,
            json!({"status": "error", "code": "LIBRARY_UNAVAILABLE"}),
            "{path}"
        );
    }
}

#[tokio::test]
async fn download_settings_snapshot_and_save() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    assert_eq!(
        call(&h.api, "download-settings-get", json!([])).await,
        json!({"speedLimit": 0, "initialized": false})
    );
    assert_eq!(
        call(&h.api, "download-settings-set", json!([7])).await,
        json!({"ok": false, "code": "INVALID_SPEED_LIMIT"})
    );
    assert_eq!(
        call(&h.api, "download-settings-set", json!([5])).await,
        json!({"ok": true, "speedLimit": 5, "initialized": true})
    );
    assert_eq!(
        read(&root.join("data").join("download-settings.json")),
        "{\n  \"speedLimit\": 5\n}\n"
    );
    assert_eq!(
        call(&h.api, "download-settings-get", json!([])).await,
        json!({"speedLimit": 5, "initialized": true})
    );
    // Диагностика читает действующий предел ограничителя (`rateLimiter.bytesPerSecond`).
    assert_eq!(h.api.download_limit_bps(), Some(5.0 * 1024.0 * 1024.0));
}

#[tokio::test]
async fn unknown_channel_is_none() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    assert!(h.api.call("launch-game", &[]).await.is_none());
}

// ---------------------------------------------------------------- 0.4.9: смена папки библиотеки

/// delivery-errors-selftest.js 0.4.9: папка библиотеки не меняется, пока идёт работа.
#[tokio::test]
async fn library_path_is_not_changed_while_busy() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    let target = root.join("other").join("HLSR");
    let target_text = target.to_string_lossy().into_owned();

    h.processes
        .running
        .lock()
        .unwrap()
        .push(GHOSTING.to_owned());
    let refused = call(&h.api, "set-library-path", json!([target_text])).await;
    assert_eq!(
        refused,
        json!({"ok": false, "error": "busy"}),
        "игра запущена"
    );
    h.processes.running.lock().unwrap().clear();

    h.api.set_launching_probe(Arc::new(|| true));
    let refused = call(&h.api, "set-library-path", json!([target_text])).await;
    assert_eq!(
        refused,
        json!({"ok": false, "error": "busy"}),
        "запуск игры"
    );
    assert_eq!(h.library.get_value("libraryPath"), Value::Null);
    assert_eq!(h.library.get_value("libraryConfirmed"), Value::Null);

    // Без работы — как раньше; папку создал сам лаунчер — маркер «создал лаунчер».
    h.api.set_launching_probe(Arc::new(|| false));
    let set = call(&h.api, "set-library-path", json!([target_text])).await;
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(h.library.get_value("libraryPath"), json!(target_text));
    let marker = hlsr_delivery::library_marker::read_marker(&target).expect("маркер");
    assert_eq!(marker["createdBy"], "launcher");
}

/// Папка, которая была до выбора, отметки «создал лаунчер» не получает.
#[tokio::test]
async fn existing_library_folder_is_not_marked_as_created() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    let target = root.join("mine").join("HLSR");
    std::fs::create_dir_all(&target).unwrap();
    let set = call(
        &h.api,
        "set-library-path",
        json!([target.to_string_lossy()]),
    )
    .await;
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(hlsr_delivery::library_marker::read_marker(&target), None);
}

// ---------------------------------------------------------------- единый файл игрока (0.4.9)

/// Каналы `player-config-*` фасада: опрос, импорт одним файлом и возврат только своей копии,
/// снятие переназначений после импорта, экспорт одним файлом, `game-config-*` с отпечатком,
/// метка файла игрока в списке cfg.
#[tokio::test]
async fn player_config_channels_set_up_import_undo_export_and_configs() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    let install_path = install(&root, "hl1-won", &[("Half-Life/hl.exe", "exe\n")]);
    let player = install_path.join("Half-Life/valve_WON/userconfig.cfg");
    let store = store(&root);

    let state = call(&h.api, "player-config-setup-state", json!(["70"])).await;
    assert_eq!(state, json!({"needed": true}));
    let bad = call(&h.api, "player-config-setup-done", json!(["70", "other"])).await;
    assert_eq!(bad, json!({"status": "error", "code": "BAD_CHOICE"}));
    let done = call(&h.api, "player-config-setup-done", json!(["70", "scratch"])).await;
    assert_eq!(done, json!({"status": "ok"}));
    assert!(read(&player).starts_with("// ===== HLSR: category rules =====\r\nexec WON.cfg"));
    let state = call(&h.api, "player-config-setup-state", json!(["70"])).await;
    assert_eq!(state, json!({"needed": false}));

    // Сохранение биндов: в файл игрока, отпечаток в ответе.
    let saved = call(
        &h.api,
        "game-settings-save",
        json!(["70", {"binds": {"k": {"command": "+duck", "base": ""}}}]),
    )
    .await;
    assert_eq!(saved["status"], "ok");
    assert_eq!(saved["keptEdited"], json!([]));
    assert!(read(&player).contains("bind \"k\" \"+duck\""));
    assert!(saved["configSha256"].is_string());

    // Импорт снимает переназначения из хранилища; «Вернуть прежний» возвращает файл.
    let before = read(&player);
    let imported = call(
        &h.api,
        "player-config-import-apply",
        json!(["70", {
            "root": {"name": "my.cfg", "text": "exec a.cfg\r\nbind f \"+attack2\"\r\n"},
            "files": {"a.cfg": {"name": "a.cfg", "text": "bind g \"+use\"\r\n"}},
        }]),
    )
    .await;
    assert_eq!(imported["status"], "ok", "{imported}");
    assert!(imported["backup"].as_str().unwrap().starts_with(".hlsr"));
    assert_eq!(imported["bindsGameId"], "hl1-won");
    // Цепочка exec раскрыта на месте (flattenImport).
    assert!(
        read(&player).contains("// ===== from a.cfg =====\r\nbind g \"+use\"\r\n"),
        "{}",
        read(&player)
    );
    assert!(read(&player).ends_with("bind f \"+attack2\"\r\n"));
    assert_eq!(imported["summary"]["binds"], 2, "{imported}");
    assert_eq!(imported["report"]["files"], json!(["a.cfg"]), "{imported}");
    assert_eq!(imported["report"]["unsafe"], json!([]), "{imported}");
    assert_eq!(store.get("hl1-won").to_value()["binds"], json!({}));
    let undone = call(&h.api, "player-config-import-undo", json!(["70"])).await;
    assert_eq!(undone["status"], "ok", "{undone}");
    assert_eq!(read(&player), before);
    let unknown = call(&h.api, "player-config-import-apply", json!(["999", "x"])).await;
    assert_eq!(unknown["code"], "UNKNOWN_GAME");
    let broken = call(
        &h.api,
        "player-config-import-apply",
        json!(["70", {"root": {}}]),
    )
    .await;
    assert_eq!(broken["code"], "BINDS_FILE_UNREADABLE");
    // Предел — у раскрытого текста (`built.bytes` 0.4.9): корень мал, exec раскрывает 300 КиБ.
    let unpacked = call(
        &h.api,
        "player-config-import-apply",
        json!(["70", {
            "root": {"name": "my.cfg", "text": "exec a.cfg\r\n".repeat(10)},
            "files": {"a.cfg": {"name": "a.cfg", "text": format!("echo {}\r\n", "x".repeat(30_000))}},
        }]),
    )
    .await;
    assert_eq!(unpacked["code"], "BINDS_FILE_TOO_LARGE", "{unpacked}");

    // «Конфиги»: отпечаток, сверка, метка файла игрока.
    let config = call(&h.api, "game-config-read", json!(["70", null])).await;
    assert_eq!(config["status"], "ok");
    assert_eq!(config["playerFile"], "userconfig.cfg");
    let stale = call(
        &h.api,
        "game-config-write",
        json!(["70", null, {"fps_max": 72}, {"expectedSha256": "0".repeat(64)}]),
    )
    .await;
    assert_eq!(stale, json!({"status": "error", "code": "CONFIG_CHANGED"}));
    let written = call(
        &h.api,
        "game-config-write",
        json!(["70", null, {"fps_max": 72}, {"expectedSha256": config["sha256"]}]),
    )
    .await;
    assert_eq!(written["status"], "ok");
    assert!(read(&player).contains("fps_max \"72\""));
    let listed = call(&h.api, "game-cfg-files-list", json!(["70", null])).await;
    assert_eq!(listed["player"], "Half-Life/valve_WON/userconfig.cfg");

    // Проверка по правилам: неверная категория; экспорт одним файлом.
    let check = call(&h.api, "player-config-check", json!(["70", "other"])).await;
    assert_eq!(check["code"], "BAD_REQUEST");
    let check = call(&h.api, "player-config-check", json!(["70", "scripted"])).await;
    assert_eq!(check["status"], "ok", "{check}");
    assert_eq!(check["file"], "Half-Life/valve_WON/userconfig.cfg");
    assert_eq!(check["stats"]["category"], "scripted");
    assert!(check["findings"].is_array(), "{check}");
    let unsupported = call(&h.api, "player-config-check", json!(["218"])).await;
    assert_eq!(unsupported, json!({"status": "unsupported"}));
    let checked = call(&h.api, "player-config-check", json!(["70"])).await;
    assert_eq!(checked["status"], "ok", "{checked}");
    assert_eq!(checked["stats"]["category"], "scriptless", "{checked}");
    assert!(checked["findings"].is_array(), "{checked}");
    let target = root.join("export.cfg");
    h.dialogs
        .answers
        .lock()
        .unwrap()
        .push_back(json!({"save": target.to_string_lossy()}));
    let exported = call(&h.api, "player-config-export", json!(["70"])).await;
    assert_eq!(exported["status"], "ok", "{exported}");
    assert_eq!(
        std::fs::read(&target).unwrap(),
        std::fs::read(&player).unwrap()
    );
    let canceled = call(&h.api, "player-config-export", json!(["70"])).await;
    assert_eq!(canceled, json!({"status": "canceled"}));
}

/// «Файлы cfg»: шаблонный файл игрока открыт и сохранён. Подготовка файла игрока перед каждым
/// вызовом не переписывает шаблон, и mtime открытого файла не уходит (иначе — `CHANGED`).
#[tokio::test]
async fn cfg_file_write_keeps_a_template_player_file_in_place() {
    let (_temp, root) = temp_root();
    let h = setup(&root);
    let install_path = install(&root, "hl1-won", &[("Half-Life/hl.exe", "exe\n")]);
    let player = install_path.join("Half-Life/valve_WON/userconfig.cfg");
    let relative = "Half-Life/valve_WON/userconfig.cfg";

    let opened = call(&h.api, "game-cfg-file-read", json!(["70", null, relative])).await;
    assert_eq!(opened["status"], "ok", "{opened}");
    let mtime = std::fs::metadata(&player).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(30));
    let listed = call(&h.api, "game-cfg-files-list", json!(["70", null])).await;
    assert_eq!(listed["status"], "ok");
    assert_eq!(
        std::fs::metadata(&player).unwrap().modified().unwrap(),
        mtime,
        "шаблон не переписан"
    );
    let content = format!("{}echo saved\n", opened["content"].as_str().unwrap());
    let meta =
        json!({"encoding": opened["encoding"], "eol": opened["eol"], "mtime": opened["mtime"]});
    let written = call(
        &h.api,
        "game-cfg-file-write",
        json!(["70", null, relative, content, meta]),
    )
    .await;
    assert_eq!(written["status"], "ok", "{written}");
    assert!(read(&player).contains("echo saved"));
}

/// Discord `selected` 0.4.9 (`index.js`): выбор игры шлёт id сборки (статусы приходят с ним, RPC
/// сравнивает их с выбранной), неизвестный id — как есть; смена сборки шлёт `selected` новой сборки.
#[tokio::test]
async fn selected_presence_like_0_4_9() {
    let (_dir, root) = temp_root();
    let h = setup(&root);
    let selected = |effects: Vec<Value>| -> Vec<Value> {
        effects
            .into_iter()
            .filter(|effect| effect["presence"]["type"] == "selected")
            .map(|effect| effect["presence"]["gameId"].clone())
            .collect()
    };

    let build = h.api.resolve_game_id(&json!("220")).expect("сборка HL2");
    assert_ne!(build, "220");
    call(&h.api, "game-delivery-selected", json!(["220"])).await;
    assert_eq!(selected(h.take_effects()), [json!(build)]);
    call(&h.api, "game-delivery-selected", json!(["999"])).await;
    assert_eq!(selected(h.take_effects()), [json!("999")]);

    let result = call(&h.api, "game-select-build", json!(["220", "5288"])).await;
    assert_eq!(result["status"], "ok", "{result}");
    assert_eq!(selected(h.take_effects()), [result["gameId"].clone()]);
}
