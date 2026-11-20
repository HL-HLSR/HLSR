//! Порт `SteamDepot` (`SteamDepotBridge`): лицензии и установка сборки из депо целиком — поддельный
//! Steam пишет в настоящий `console_log.txt` и создаёт файлы депо через ссылку на слот (на Windows —
//! настоящий junction, `cmd /c mklink /J`; на Linux — симлинк). Плюс проверки ветки Linux (симлинк
//! слота), которые на Windows недоступны.

#[path = "support/depot_world.rs"]
mod depot_world;

use depot_world::{build_service, build_world};
use hlsr_core::js::Platform;
use hlsr_core::logger::MemoryLogger;
use hlsr_delivery::errors::{Code, Failure};
use hlsr_delivery::paths::resolve_library;
use hlsr_delivery::ports::{DepotProgress, SteamDepot};
use hlsr_platform::depot_install::{
    DXLEVEL_KEY, RegistryWriter, SPEEDRUNSETUP_DXLEVEL, SteamDepotBridge,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

struct Recorder(Mutex<Vec<(String, String, u32)>>);

impl RegistryWriter for Recorder {
    fn set_dword(&self, key_path: &str, value_name: &str, value: u32) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .push((key_path.to_owned(), value_name.to_owned(), value));
        Ok(())
    }
}

fn scenario(licenses: Value) -> Value {
    json!({
        "name": "мост",
        "steam": {
            "licenses": licenses,
            "depots": {
                "222": {"main": {"files": 51, "polls": 2, "extraFiles": ["hl2.exe", "hl2/gameinfo.txt"]}},
                "221": {"main": {"files": 529, "polls": 1}},
                "243752": {"main": {"files": 1, "polls": 1}},
            },
        },
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bridge_installs_a_build_through_the_port() {
    let dir = tempfile::tempdir().unwrap();
    let world = build_world(dir.path(), scenario(json!({})));
    let logger = Arc::new(MemoryLogger::default());
    let service = build_service(&world, Platform::CURRENT, logger.clone());
    let registry = Arc::new(Recorder(Mutex::default()));
    let bridge: Arc<dyn SteamDepot> = Arc::new(SteamDepotBridge::new(
        service,
        Some(logger.clone()),
        Some(registry.clone()),
    ));

    bridge.assert_owns(&[]).await.unwrap();
    bridge.assert_owns(&[220]).await.unwrap();

    let paths = resolve_library(&world.lib.to_string_lossy()).unwrap();
    let events = Arc::new(Mutex::new(Vec::<DepotProgress>::new()));
    let collected = events.clone();
    let installed = bridge
        .install(
            &paths,
            "hl2-5288",
            Arc::new(move |progress: &DepotProgress| collected.lock().unwrap().push(*progress)),
        )
        .await
        .unwrap();

    let game = world.lib.join("Games").join("hl2-5288");
    assert_eq!(installed["schemaVersion"], json!(1));
    assert_eq!(installed["gameId"], json!("hl2-5288"));
    assert_eq!(installed["version"], json!("1.0.0"));
    assert_eq!(installed["source"], json!("steam-depot"));
    assert_eq!(installed["state"], json!("ready"));
    assert_eq!(installed["installPath"], json!(game.to_string_lossy()));
    assert_eq!(installed["manifestSha256"], Value::Null);

    // installed.json на диске — то же, что вернули; два пробела отступа и перевод строки в конце.
    let on_disk = std::fs::read_to_string(game.join("installed.json")).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&on_disk).unwrap(), installed);
    assert!(on_disk.starts_with("{\n  \"schemaVersion\": 1,\n") && on_disk.ends_with("}\n"));

    assert_eq!(
        std::fs::read_to_string(game.join("hl2.exe")).unwrap(),
        "hl2.exe"
    );
    assert!(game.join("hl2").join("gameinfo.txt").is_file());
    assert_eq!(
        std::fs::read_to_string(game.join("hl2").join("cfg").join("autoexec.cfg")).unwrap(),
        hlsr_delivery::player_config::template("hl2-5288", Platform::CURRENT)
    );
    assert_eq!(
        std::fs::read_to_string(game.join("steam_appid.txt")).unwrap(),
        "220\n"
    );
    assert_eq!(
        *registry.0.lock().unwrap(),
        vec![(
            DXLEVEL_KEY.to_owned(),
            "DXLevel_V1".to_owned(),
            SPEEDRUNSETUP_DXLEVEL
        )]
    );

    // Ничего лишнего: нет папки staging игры, нет брошенной ссылки слота.
    let staging: Vec<_> = std::fs::read_dir(world.lib.join(".hlsr").join("staging"))
        .unwrap()
        .flatten()
        .collect();
    assert!(staging.is_empty(), "staging: {staging:?}");
    let slots: Vec<_> = std::fs::read_dir(&world.content_dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    assert!(slots.is_empty(), "слоты: {slots:?}");

    // Ход: три депо, число файлов не убывает внутри депо.
    let events = events.lock().unwrap();
    assert!(!events.is_empty());
    assert!(events.iter().all(|event| event.total == 3));
    assert_eq!(events.last().unwrap().index, 2);
    for index in 0..3 {
        let files: Vec<u64> = events
            .iter()
            .filter(|event| event.index == index)
            .map(|event| event.files)
            .collect();
        assert!(files.windows(2).all(|pair| pair[0] <= pair[1]), "{files:?}");
    }
    assert_eq!(events[0].expected_files, Some(103));

    // Лицензия проверялась один раз: второй assert_owns и install берут её из памяти.
    let licenses = world
        .state()
        .commands
        .iter()
        .filter(|command| command["args"][0] == json!("steam://+licenses_for_app"))
        .count();
    assert_eq!(licenses, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bridge_reports_a_missing_license_and_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let world = build_world(dir.path(), scenario(json!({"220": "missing"})));
    let logger = Arc::new(MemoryLogger::default());
    let service = build_service(&world, Platform::CURRENT, logger.clone());
    let bridge = SteamDepotBridge::new(service, Some(logger), None);

    let owned = bridge.assert_owns(&[220]).await.unwrap_err();
    let Failure::Delivery(error) = owned else {
        panic!("ожидалась ошибка доставки");
    };
    assert_eq!(error.code, Code::SteamAppNotOwned);
    assert_eq!(error.details, Some(json!({"appId": 220})));

    let paths = resolve_library(&world.lib.to_string_lossy()).unwrap();
    let failure = bridge
        .install(&paths, "hl2-5288", Arc::new(|_: &DepotProgress| {}))
        .await
        .unwrap_err();
    let Failure::Delivery(error) = failure else {
        panic!("ожидалась ошибка доставки");
    };
    assert_eq!(error.code, Code::SteamAppNotOwned);
    assert!(!world.lib.join("Games").join("hl2-5288").exists());
    assert_eq!(
        std::fs::read_dir(world.lib.join(".hlsr").join("staging"))
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_install_keeps_the_players_files() {
    let dir = tempfile::tempdir().unwrap();
    let world = build_world(dir.path(), scenario(json!({})));
    let logger = Arc::new(MemoryLogger::default());
    let service = build_service(&world, Platform::CURRENT, logger.clone());
    let bridge = SteamDepotBridge::new(service, Some(logger), None);
    let paths = resolve_library(&world.lib.to_string_lossy()).unwrap();
    let noop = || Arc::new(|_: &DepotProgress| {});

    bridge.install(&paths, "hl2-5288", noop()).await.unwrap();
    let game = world.lib.join("Games").join("hl2-5288");
    std::fs::create_dir_all(game.join("hl2").join("SAVE")).unwrap();
    std::fs::write(game.join("hl2").join("SAVE").join("quick.sav"), "мои сейвы").unwrap();
    std::fs::write(
        game.join("hl2").join("cfg").join("autoexec.cfg"),
        "мой autoexec",
    )
    .unwrap();
    std::fs::write(game.join("stray.txt"), "лишнее").unwrap();

    // Поддельный Steam отвечает «разогревом» и «скачиванием» по чётности: вторая сборка — снова
    // пара «разогрев/скачивание».
    bridge.install(&paths, "hl2-5288", noop()).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(game.join("hl2").join("SAVE").join("quick.sav")).unwrap(),
        "мои сейвы"
    );
    assert_eq!(
        std::fs::read_to_string(game.join("hl2").join("cfg").join("autoexec.cfg")).unwrap(),
        "мой autoexec"
    );
    assert!(!game.join("stray.txt").exists());
    assert!(
        !world
            .lib
            .join("Games")
            .join("hl2-5288.hlsr-backup")
            .exists()
    );
}

// ------------------------------------------------------------------ Linux (симлинк слота)

#[cfg(unix)]
mod linux {
    use super::*;
    use hlsr_platform::steam_depot::DownloadOptions;
    use std::os::unix::fs::symlink;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_slot_is_a_symlink_to_the_staging_folder_and_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let world = build_world(dir.path(), scenario(json!({})));
        let logger = Arc::new(MemoryLogger::default());
        let service = build_service(&world, Platform::Linux, logger);
        let target = world
            .lib
            .join(".hlsr")
            .join("staging")
            .join("hl2-5288-depot-test");
        let lib = world.lib.to_string_lossy().into_owned();

        let done = service
            .download(
                "hl2-5288",
                &target.to_string_lossy(),
                DownloadOptions {
                    library_root: Some(lib.as_str()),
                    on_progress: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(done.depots.len(), 3);
        // Файлы депо легли в папку библиотеки через ссылку слота (51 счётный и два именованных).
        assert_eq!(service.count_files(&done.depots[0].1), 53);
        assert_eq!(service.count_files(&done.depots[1].1), 529);
        // `cmd.exe` на Linux не вызывается, слот снят.
        assert!(
            world
                .state()
                .commands
                .iter()
                .all(|command| command["file"] != json!("cmd.exe"))
        );
        assert!(std::fs::symlink_metadata(world.content_dir.join("app_2")).is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn orphan_symlinks_are_swept_and_foreign_ones_stay() {
        let dir = tempfile::tempdir().unwrap();
        let world = build_world(dir.path(), scenario(json!({})));
        let logger = Arc::new(MemoryLogger::default());
        let service = build_service(&world, Platform::Linux, logger.clone());
        std::fs::create_dir_all(&world.content_dir).unwrap();
        let mine = world.lib.join("Downloads").join("old");
        std::fs::create_dir_all(&mine).unwrap();
        symlink(&mine, world.content_dir.join("app_2")).unwrap();
        symlink(&world.elsewhere, world.content_dir.join("app_3")).unwrap();
        std::fs::create_dir(world.content_dir.join("app_4")).unwrap();

        let lib = world.lib.to_string_lossy().into_owned();
        let removed =
            service.sweep_content_slots(&world.content_dir.to_string_lossy(), Some(lib.as_str()));
        assert_eq!(removed, 1);
        assert!(std::fs::symlink_metadata(world.content_dir.join("app_2")).is_err());
        assert!(world.content_dir.join("app_3").is_symlink());
        assert!(world.content_dir.join("app_4").is_dir());
        // Цель ссылки не тронута.
        assert!(mine.is_dir());
        let lines: Vec<String> = logger.take().into_iter().map(|(_, text)| text).collect();
        assert_eq!(
            lines,
            vec![
                format!("[steam-depot] снята ссылка app_2 → {}", mine.display()),
                "[steam-depot] убрано брошенных ссылок: 1".to_owned(),
            ]
        );

        // Обычную папку и несуществующий слот `unlink_content_slot` не трогает.
        assert!(!service.unlink_content_slot(world.content_dir.join("app_4")));
        assert!(!service.unlink_content_slot(world.content_dir.join("app_9")));
        assert!(world.content_dir.join("app_4").is_dir());
    }
}
