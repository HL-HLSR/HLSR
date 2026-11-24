//! Самотесты эпизодов Ghosting 0.4.9, перенесённые в Rust: `scripts/episodes-core-selftest.js`
//! (реестр, состояние, запуск, конфиг Ghosting в эпизод и обратно), `scripts/episodes-selftest.js`
//! (эпизоды переезжают rename при обновлении и переустановке Ghosting, park, удаление игры),
//! `scripts/episode-button-selftest.js` и `scripts/episode-install-selftest.js` (подписанный
//! манифест, загрузка 7z с 127.0.0.1, проверка записей до распаковки, подмена папки). На настоящем
//! 7za из `resources/7zip`.
//!
//! Не перенесены: подмена `fs.renameSync` (EBUSY при возврате config.cfg, «крах» посреди подмены)
//! и запуск игры после загрузки (GAME_RUNNING) — подмену fs и процесса Rust здесь не делает; крах
//! после новой папки воспроизводится раскладкой файлов, как в самотесте.

mod support;

use hlsr_delivery::archive_service::ArchiveService;
use hlsr_delivery::environment::Environment;
use hlsr_delivery::episode_button::episode_button;
use hlsr_delivery::episode_install::{self, GAME_ID as GAME};
use hlsr_delivery::episodes::{self, MARKER, SYNC};
use hlsr_delivery::errors::{Code, Failure, to_delivery_error};
use hlsr_delivery::game_delivery_service::{
    GameDeliveryService, Provided, Registry, ServiceOptions,
};
use hlsr_delivery::game_download_manager::StateFile;
use hlsr_delivery::game_removal;
use hlsr_delivery::install_service::{
    FaultInjector, FullUpdate, InstallOptions, InstallRequest, InstallService, InstallTarget,
    IntegrityCheck, IntegrityReport, IntegrityRequest,
};
use hlsr_delivery::main_button::{self, Input};
use hlsr_delivery::manifest_signing::{KeyPair, attach, generate_key_pair};
use hlsr_delivery::manifests;
use hlsr_delivery::mutable_paths;
use hlsr_delivery::paths::{LibraryPaths, game_install_path, resolve_library};
use hlsr_delivery::ports::{Logger, MemoryLogger};
use hlsr_delivery::providers::BoxFuture;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use support::http::{RequestLog, Server, start_server};
use support::scenario::temp_root;

fn full(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |acc, part| acc.join(part))
}

fn write(root: &Path, relative: &str, data: impl AsRef<[u8]>) {
    let file = full(root, relative);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, data).unwrap();
}

fn read(root: &Path, relative: &str) -> Option<String> {
    std::fs::read_to_string(full(root, relative)).ok()
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn library(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let (temp, root) = temp_root();
    for (relative, text) in files {
        write(&root, relative, text);
    }
    (temp, root)
}

fn installed_ep(dir: &str, version: Value) -> [(String, String); 2] {
    [
        (format!("{dir}/gameinfo.txt"), "gameinfo".to_owned()),
        (
            format!("{dir}/{MARKER}"),
            json!({ "version": version }).to_string(),
        ),
    ]
}

fn library_owned(files: Vec<(String, String)>) -> (tempfile::TempDir, PathBuf) {
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    library(&borrowed)
}

fn code(failure: Failure) -> Code {
    to_delivery_error(failure, None).code
}

// ---------------------------------------------------------------- episodes-core-selftest.js

#[test]
fn launch_config_swaps_the_game_directory_and_keeps_the_original() {
    let base = manifests::launch_config_of(GAME).unwrap();
    let ep2 = episodes::launch_config(base, "ep2").unwrap();
    assert_eq!(ep2.game_directory, "ep2");
    assert_eq!(ep2.arguments, ["-steam", "-game", "ep2"]);
    assert_eq!(ep2.steam_app_id, base.steam_app_id);
    assert_eq!(base.arguments, ["-steam", "-game", "ghostingmod"]);
    let ep1 = episodes::launch_config(base, "ep1").unwrap();
    assert_eq!(ep1.game_directory, "episodic");
    assert!(
        std::ptr::eq(
            ep1.arguments,
            episodes::launch_config(base, "ep1").unwrap().arguments
        ),
        "один набор аргументов на процесс"
    );
    assert_eq!(
        episodes::launch_config(base, "ep3").unwrap_err().code,
        Code::LaunchConfigInvalid
    );
    let bare = manifests::LaunchConfig {
        arguments: &["-steam"],
        ..base.clone()
    };
    assert_eq!(
        episodes::launch_config(&bare, "ep1").unwrap_err().code,
        Code::LaunchConfigInvalid
    );
}

#[test]
fn status_needs_both_a_marker_with_a_version_and_gameinfo() {
    let mut files = vec![("episodic/gameinfo.txt".to_owned(), "без маркера".to_owned())];
    files.extend(installed_ep("ep2", json!("1.0.0")));
    let (_temp, root) = library_owned(files);
    assert_eq!(
        Value::Object(episodes::status(Some(&root))),
        json!({ "ep1": { "installed": false, "version": null }, "ep2": { "installed": true, "version": "1.0.0" } })
    );
    assert_eq!(
        episodes::status(None)["ep1"],
        json!({ "installed": false, "version": null })
    );
    let (_temp, no_info) = library_owned(vec![
        (
            format!("episodic/{MARKER}"),
            json!({ "version": "1.0.0" }).to_string(),
        ),
        ("ep2/gameinfo.txt".to_owned(), "g".to_owned()),
        (format!("ep2/{MARKER}"), json!({ "version": 2 }).to_string()),
    ]);
    assert_eq!(
        Value::Object(episodes::status(Some(&no_info))),
        json!({ "ep1": { "installed": false, "version": null }, "ep2": { "installed": false, "version": null } }),
        "маркер без gameinfo, версия не строкой"
    );
}

#[test]
fn sync_config_in_brings_player_files_and_leaves_stock_cfg_alone() {
    let mut files: Vec<(String, String)> = installed_ep("episodic", json!("1.0.0")).into();
    for (relative, text) in [
        ("ghostingmod/cfg/autoexec.cfg", "мой autoexec"),
        ("ghostingmod/cfg/config.cfg", "bind 9 tc_n"),
        ("ghostingmod/cfg/tc_n.cfg", "мой скрипт"),
        ("ghostingmod/cfg/chapter9.cfg", "map d1_town_01"),
        ("ghostingmod/cfg/skill.cfg", "skill hl2"),
        ("ghostingmod/cfg/chapter1.cfg", "map d1_trainstation_01"),
        ("episodic/cfg/chapter1.cfg", "map ep1_citadel_00"),
    ] {
        files.push((relative.to_owned(), text.to_owned()));
    }
    let (_temp, root) = library_owned(files);
    let stock: Vec<String> = [
        "autoexec.cfg",
        "config.cfg",
        "CHAPTER9.cfg",
        "skill.cfg",
        "chapter1.cfg",
    ]
    .map(str::to_owned)
    .to_vec();
    let mut copied = episodes::sync_config_in(&root, "ep1", Some(&stock))
        .unwrap()
        .copied;
    copied.sort();
    assert_eq!(copied, ["autoexec.cfg", "config.cfg", "tc_n.cfg"]);
    assert_eq!(
        read(&root, "episodic/cfg/autoexec.cfg").unwrap(),
        "мой autoexec"
    );
    assert_eq!(read(&root, "episodic/cfg/tc_n.cfg").unwrap(), "мой скрипт");
    assert_eq!(
        read(&root, "episodic/cfg/chapter9.cfg"),
        None,
        "главы HL2 не попадают в эпизод"
    );
    assert_eq!(read(&root, "episodic/cfg/skill.cfg"), None);
    assert_eq!(
        read(&root, "episodic/cfg/chapter1.cfg").unwrap(),
        "map ep1_citadel_00",
        "стоковый cfg эпизода не перезаписан"
    );

    // Сборка Ghosting неизвестна — только autoexec.cfg и config.cfg; Config.cfg ложится как config.cfg.
    let mut files: Vec<(String, String)> = installed_ep("episodic", json!("1.0.0")).into();
    files.push(("ghostingmod/cfg/Config.cfg".to_owned(), "x".to_owned()));
    files.push(("ghostingmod/cfg/tc_n.cfg".to_owned(), "y".to_owned()));
    let (_temp, other) = library_owned(files);
    assert_eq!(
        episodes::sync_config_in(&other, "ep1", None)
            .unwrap()
            .copied,
        ["Config.cfg"]
    );
    assert!(names(&other.join("episodic").join("cfg")).contains(&"config.cfg".to_owned()));
    assert_eq!(read(&other, "episodic/cfg/config.cfg").unwrap(), "x");

    // Нет ghostingmod\cfg или самого эпизода — без ошибок и без папок-призраков.
    let (_temp, bare) = library_owned(installed_ep("episodic", json!("1.0.0")).into());
    assert!(
        episodes::sync_config_in(&bare, "ep1", None)
            .unwrap()
            .copied
            .is_empty()
    );
    let none = episodes::sync_config_in(&bare, "ep2", None).unwrap();
    assert!(none.copied.is_empty() && none.config_sha256.is_none());
    assert!(!bare.join("ep2").exists());
}

/// 24.11: в files.json Ghosting нет ghostingmod/cfg, список сборки пуст. Главы и skill HL2 в
/// эпизод всё равно не идут, свои cfg эпизода (`stockCfg` маркера) не затираются, а главы HL2,
/// скопированные прежним лаунчером, убираются.
#[test]
fn sync_config_in_keeps_hl2_chapters_out_with_an_empty_ghosting_list() {
    let marker = json!({"version": "1.0.0", "stockCfg": ["chapter1.cfg", "skill_episodic.cfg"]});
    let files: Vec<(String, String)> = [
        ("episodic/gameinfo.txt", "gameinfo".to_owned()),
        ("episodic/.hlsr-episode.json", marker.to_string()),
        ("ghostingmod/cfg/config.cfg", "bind 9 tc_n".to_owned()),
        ("ghostingmod/cfg/tc_n.cfg", "мой скрипт".to_owned()),
        (
            "ghostingmod/cfg/chapter1.cfg",
            "map d1_trainstation_01".to_owned(),
        ),
        (
            "ghostingmod/cfg/chapter9a.cfg",
            "map d2_prison_01".to_owned(),
        ),
        ("ghostingmod/cfg/skill.cfg", "skill hl2".to_owned()),
        (
            "ghostingmod/cfg/skill_episodic.cfg",
            "skill ghosting".to_owned(),
        ),
        ("episodic/cfg/chapter1.cfg", "map ep1_citadel_00".to_owned()),
        ("episodic/cfg/skill_episodic.cfg", "skill ep1".to_owned()),
        ("episodic/cfg/chapter9a.cfg", "map d2_prison_01".to_owned()),
        ("episodic/cfg/chapter10.cfg", "свой файл игрока".to_owned()),
    ]
    .into_iter()
    .map(|(path, text)| (path.to_owned(), text))
    .collect();
    let (_temp, root) = library_owned(files);
    let mut copied = episodes::sync_config_in(&root, "ep1", Some(&[]))
        .unwrap()
        .copied;
    copied.sort();
    assert_eq!(copied, ["config.cfg", "tc_n.cfg"]);
    assert_eq!(
        read(&root, "episodic/cfg/chapter1.cfg").unwrap(),
        "map ep1_citadel_00"
    );
    assert_eq!(
        read(&root, "episodic/cfg/skill_episodic.cfg").unwrap(),
        "skill ep1"
    );
    assert_eq!(read(&root, "episodic/cfg/skill.cfg"), None);
    assert_eq!(
        read(&root, "episodic/cfg/chapter9a.cfg"),
        None,
        "копия HL2 убрана"
    );
    assert_eq!(
        read(&root, "episodic/cfg/chapter10.cfg").unwrap(),
        "свой файл игрока",
        "непохожий на HL2 файл не трогается"
    );
}

#[test]
fn sync_config_back_returns_config_only_when_the_game_rewrote_it() {
    let mut files: Vec<(String, String)> = installed_ep("ep2", json!("1.0.0")).into();
    files.push((
        "ghostingmod/cfg/config.cfg".to_owned(),
        "bind 9 tc_n".to_owned(),
    ));
    let (_temp, root) = library_owned(files);
    let episode_config = full(&root, "ep2/cfg/config.cfg");
    let back = |root: &Path| episodes::sync_config_back(root, "ep2").unwrap();

    episodes::sync_config_in(&root, "ep2", None).unwrap();
    assert_eq!(episodes::pending_sync(Some(&root)), ["ep2"]);
    let result = back(&root);
    assert!(!result.returned && !result.conflict);
    assert!(episodes::pending_sync(Some(&root)).is_empty());
    assert!(episodes::pending_sync(None).is_empty());

    episodes::sync_config_in(&root, "ep2", None).unwrap();
    std::fs::write(&episode_config, "bind 0 tc_n").unwrap();
    assert!(back(&root).returned);
    assert_eq!(
        read(&root, "ghostingmod/cfg/config.cfg").unwrap(),
        "bind 0 tc_n"
    );
    assert!(!back(&root).returned, "повтор без отметки ничего не делает");
    assert!(
        !names(&full(&root, "ghostingmod/cfg"))
            .iter()
            .any(|n| n.contains("tmp")),
        "временных файлов нет"
    );

    // Пока эпизод висел (крах), config.cfg Ghosting поменяли — новее он.
    episodes::sync_config_in(&root, "ep2", None).unwrap();
    std::fs::write(&episode_config, "bind 1 old").unwrap();
    write(&root, "ghostingmod/cfg/config.cfg", "bind 2 newer");
    let result = back(&root);
    assert!(!result.returned && result.conflict);
    assert_eq!(
        read(&root, "ghostingmod/cfg/config.cfg").unwrap(),
        "bind 2 newer"
    );

    // Игра упала на записи — пустой config.cfg не возвращается.
    episodes::sync_config_in(&root, "ep2", None).unwrap();
    std::fs::write(&episode_config, "").unwrap();
    assert!(!back(&root).returned);
    assert_eq!(
        read(&root, "ghostingmod/cfg/config.cfg").unwrap(),
        "bind 2 newer"
    );

    // Новый запуск поверх невозвращённого: сначала возврат, потом копия.
    episodes::sync_config_in(&root, "ep2", None).unwrap();
    std::fs::write(&episode_config, "bind 3 from crash").unwrap();
    episodes::sync_config_in(&root, "ep2", None).unwrap();
    assert_eq!(
        read(&root, "ghostingmod/cfg/config.cfg").unwrap(),
        "bind 3 from crash"
    );

    // Битая отметка снимается и не висит вечно.
    write(&root, &format!("ep2/{SYNC}"), "{");
    assert!(!back(&root).returned);
    assert!(episodes::pending_sync(Some(&root)).is_empty());

    // returnPending не бросает и пишет в журнал.
    episodes::sync_config_in(&root, "ep2", None).unwrap();
    std::fs::write(&episode_config, "bind 4 back").unwrap();
    let logger = MemoryLogger::default();
    episodes::return_pending(&root, &logger, None);
    assert_eq!(
        read(&root, "ghostingmod/cfg/config.cfg").unwrap(),
        "bind 4 back"
    );
    assert_eq!(logger.take().len(), 1);
}

#[test]
fn removing_episode_one_takes_episode_two_with_it() {
    let mut files: Vec<(String, String)> = installed_ep("episodic", json!("1.0.0")).into();
    files.extend(installed_ep("ep2", json!("1.0.0")));
    files.push(("ghostingmod/gameinfo.txt".to_owned(), "g".to_owned()));
    let (_temp, root) = library_owned(files);
    assert_eq!(episodes::dependents_of("ep1"), ["ep2"]);
    assert_eq!(episodes::remove(&root, "ep2").unwrap(), ["ep2"]);
    assert_eq!(read(&root, "episodic/gameinfo.txt").unwrap(), "gameinfo");
    std::fs::create_dir_all(root.join("ep2").join("cfg")).unwrap();
    assert_eq!(episodes::remove(&root, "ep1").unwrap(), ["ep2", "episodic"]);
    assert!(!root.join("episodic").exists() && !root.join("ep2").exists());
    assert_eq!(read(&root, "ghostingmod/gameinfo.txt").unwrap(), "g");
    assert_eq!(
        code(episodes::remove(&root, "../x").unwrap_err()),
        Code::LaunchConfigInvalid
    );
}

// ---------------------------------------------------------------- episodes-selftest.js

#[test]
fn episode_dirs_are_mutable_not_user_data_and_no_release_ships_them() {
    let dirs: Vec<&str> = episodes::EPISODES.iter().map(|e| e.dir).collect();
    assert_eq!(
        dirs,
        mutable_paths::EPISODE_DIRS,
        "episodes.rs и EPISODE_DIRS разошлись"
    );
    for path in [
        "episodic/gameinfo.txt",
        "episodic/ep1_pak_000.vpk",
        "ep2/bin/client.dll",
    ] {
        assert!(mutable_paths::matches_mutable(path), "{path}");
    }
    assert!(!mutable_paths::matches_mutable(
        "ghostingmod/bin/client.dll"
    ));
    assert!(!mutable_paths::USER_DATA.contains(&"episodic"));
    for game_id in manifests::game_ids() {
        for (version, embedded) in manifests::files_manifests_of(game_id) {
            let files = embedded.value()["files"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let hit = files.iter().find(|file| {
                let path = file["path"].as_str().unwrap_or_default();
                dirs.iter()
                    .any(|dir| path == *dir || path.starts_with(&format!("{dir}/")))
            });
            assert!(hit.is_none(), "{game_id} {version}: {hit:?}");
        }
    }
}

fn seven_zip() -> PathBuf {
    hlsr_core::paths::Resources::detect().sevenzip()
}

/// Архив 7z из `files` (путь → содержимое): корни архива — первые сегменты путей.
fn make_7z(dir: &Path, name: &str, files: &[(&str, &str)]) -> (PathBuf, Vec<u8>) {
    let stage = dir.join(format!("stage-{name}"));
    let _ = std::fs::remove_dir_all(&stage);
    for (relative, data) in files {
        write(&stage, relative, data);
    }
    let archive = dir.join("archives").join(name);
    std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
    let mut roots: Vec<&str> = files
        .iter()
        .map(|(rel, _)| rel.split('/').next().unwrap())
        .collect();
    roots.dedup();
    let status = std::process::Command::new(seven_zip())
        .args(["a", "-t7z", "-bd", "-y"])
        .arg(&archive)
        .args(&roots)
        .current_dir(&stage)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let bytes = std::fs::read(&archive).unwrap();
    (archive, bytes)
}

struct Clean;

impl IntegrityCheck for Clean {
    fn check<'a>(
        &'a self,
        _request: IntegrityRequest<'a>,
    ) -> BoxFuture<'a, Result<IntegrityReport, Failure>> {
        Box::pin(async {
            Ok(IntegrityReport {
                ok: true,
                ..IntegrityReport::default()
            })
        })
    }
}

struct Reinstall {
    _temp: tempfile::TempDir,
    paths: LibraryPaths,
    service: InstallService,
    install_path: PathBuf,
    park: PathBuf,
    result: Result<Value, Failure>,
    user_data_bytes: u64,
}

/// Старая сборка с эпизодами; `with_update` — полное обновление (`commit_update`), иначе
/// переустановка (`swap_into_library`). `fault` — точка, на которой обновление падает.
async fn reinstall(
    with_update: bool,
    fault: Option<&'static str>,
    new_tree: &[(&str, &str)],
) -> Reinstall {
    reinstall_with(with_update, fault, new_tree, &[], &[]).await
}

/// То же; `preserve` — `preservePaths` манифеста, `old_extra` — ещё файлы старой сборки.
async fn reinstall_with(
    with_update: bool,
    fault: Option<&'static str>,
    new_tree: &[(&str, &str)],
    preserve: &[&str],
    old_extra: &[(&str, &str)],
) -> Reinstall {
    let (temp, root) = temp_root();
    let paths = resolve_library(&root.join("library").to_string_lossy()).unwrap();
    std::fs::create_dir_all(&paths.state).unwrap();
    let install_path = game_install_path(&paths, GAME).unwrap();
    for (relative, text) in [
        ("hl2.exe", "старая"),
        ("ghostingmod/SAVE/quick.sav", "сейв"),
        ("episodic/gameinfo.txt", "эпизод 1"),
        ("ep2/bin/client.dll", "эпизод 2"),
        ("installed.json", r#"{"state":"ready","version":"1.0.0"}"#),
    ]
    .iter()
    .chain(old_extra)
    {
        write(&install_path, relative, text);
    }
    let (archive, bytes) = make_7z(&root, "build.7z", new_tree);
    let manifest = json!({
        "gameId": GAME,
        "version": "1.0.1",
        "expectedInstalledSize": 5,
        "preservePaths": preserve,
        "baseArchive": { "size": bytes.len(), "sha256": support::sha256_hex(&bytes) },
        "filesManifest": { "requiredFiles": ["hl2.exe"] },
    });
    let logger: Arc<dyn Logger> = Arc::new(MemoryLogger::default());
    let fault_injector: Option<FaultInjector> = fault.map(|name| -> FaultInjector {
        Arc::new(move |point: &str, _: &Value| {
            if point == name {
                Err(hlsr_delivery::errors::RawError::new(format!("сбой на {point}")).into())
            } else {
                Ok(())
            }
        })
    });
    let service = InstallService::new(
        paths.clone(),
        InstallOptions {
            archive_service: Some(ArchiveService::with_default_bin(logger.clone())),
            logger: Some(logger),
            space_margin: Some(0),
            fault_injector,
            ..InstallOptions::default()
        },
    );
    let user_data_bytes = service.user_data_bytes(GAME, &install_path, &manifest);
    let update = with_update.then(|| FullUpdate {
        from_version: "1.0.0".to_owned(),
        files_manifest: Some(json!({ "version": "1.0.1" })),
        integrity: Some(Arc::new(Clean)),
    });
    let result = service
        .install(InstallRequest {
            manifest,
            manifest_sha256: None,
            archive_path: archive,
            files_manifest: None,
            published_at: None,
            update,
        })
        .await;
    if fault.is_none() && preserve.is_empty() {
        result.as_ref().unwrap();
    }
    let park = service.episodes_park_path(GAME);
    Reinstall {
        _temp: temp,
        paths,
        service,
        install_path,
        park,
        result,
        user_data_bytes,
    }
}

#[tokio::test]
async fn a_failed_update_after_the_swap_brings_the_old_build_back_with_its_episodes() {
    let env = reinstall(
        true,
        Some("before-installed-write"),
        &[("hl2.exe", "новая")],
    )
    .await;
    assert!(env.result.is_err(), "обновление не прошло");
    assert_eq!(read(&env.install_path, "hl2.exe").unwrap(), "старая");
    assert_eq!(
        read(&env.install_path, "episodic/gameinfo.txt").unwrap(),
        "эпизод 1"
    );
    assert_eq!(
        read(&env.install_path, "ep2/bin/client.dll").unwrap(),
        "эпизод 2"
    );
    assert!(!env.park.exists(), "ничего не осталось отложенным");
}

#[tokio::test]
async fn a_build_with_its_own_episodic_keeps_the_old_episode_parked() {
    let env = reinstall(
        true,
        None,
        &[("hl2.exe", "новая"), ("episodic/gameinfo.txt", "из сборки")],
    )
    .await;
    assert_eq!(
        read(&env.install_path, "episodic/gameinfo.txt").unwrap(),
        "из сборки"
    );
    assert_eq!(
        read(&env.install_path, "ep2/bin/client.dll").unwrap(),
        "эпизод 2"
    );
    assert_eq!(
        read(&env.park, "episodic/gameinfo.txt").unwrap(),
        "эпизод 1"
    );
}

#[tokio::test]
async fn removing_the_game_removes_parked_episodes_and_a_stale_park_does_not_block() {
    let env = reinstall(true, None, &[("hl2.exe", "новая")]).await;
    write(&env.park, "episodic/gameinfo.txt", "старая отложенная");
    // В игре свой episodic: старая копия отходит в сторону.
    env.service.park_episodes(GAME, &env.install_path).unwrap();
    assert!(
        names(&env.park)
            .iter()
            .any(|name| name.starts_with("episodic.stale-"))
    );
    env.service.unpark_episodes(GAME, &env.install_path);
    assert_eq!(
        read(&env.install_path, "episodic/gameinfo.txt").unwrap(),
        "эпизод 1"
    );
    game_removal::remove_game(&env.paths, GAME).unwrap();
    assert!(!env.park.exists(), "6 ГБ не остаются в библиотеке");
}

#[tokio::test]
async fn episodes_parked_by_an_interrupted_swap_come_back_on_start() {
    let env = reinstall(true, None, &[("hl2.exe", "новая")]).await;
    std::fs::create_dir_all(&env.park).unwrap();
    std::fs::rename(env.install_path.join("ep2"), env.park.join("ep2")).unwrap();
    env.service.unpark_episodes(GAME, &env.install_path);
    assert_eq!(
        read(&env.install_path, "ep2/bin/client.dll").unwrap(),
        "эпизод 2"
    );
    assert!(!env.park.exists());
}

#[tokio::test]
async fn episodes_move_into_the_new_build_by_rename() {
    for with_update in [true, false] {
        let env = reinstall(with_update, None, &[("hl2.exe", "новая")]).await;
        assert_eq!(read(&env.install_path, "hl2.exe").unwrap(), "новая");
        assert_eq!(
            read(&env.install_path, "episodic/gameinfo.txt").unwrap(),
            "эпизод 1"
        );
        assert_eq!(
            read(&env.install_path, "ep2/bin/client.dll").unwrap(),
            "эпизод 2"
        );
        assert_eq!(
            env.user_data_bytes,
            "сейв".len() as u64,
            "место под эпизоды не просится, они не копируются"
        );
        if with_update {
            assert_eq!(
                read(&env.install_path, "ghostingmod/SAVE/quick.sav").unwrap(),
                "сейв"
            );
        }
        assert!(names(&env.paths.staging).is_empty(), "staging убран");
        assert!(!env.park.exists());
    }
}

/// 0.4.9: файл игрока, который не переносится (файл на месте папки новой сборки), прерывает
/// установку до подмены папок — `PRESERVE_FAILED`, прежняя сборка с эпизодами цела (в 0.4.7 —
/// строка «не удалось сохранить» в журнале и установка дальше).
#[tokio::test]
async fn a_file_of_the_player_that_cannot_be_carried_stops_the_install() {
    for with_update in [true, false] {
        let env = reinstall_with(
            with_update,
            None,
            &[("hl2.exe", "новая"), ("ghostingmod/maps/a.bsp", "bsp")],
            &["ghostingmod/maps"],
            &[("ghostingmod/maps", "файл на месте папки")],
        )
        .await;
        let error = to_delivery_error(env.result.unwrap_err(), None);
        assert_eq!(error.code, Code::PreserveFailed, "{}", error.message);
        assert!(
            error.message.contains("Прежняя сборка не тронута"),
            "{}",
            error.message
        );
        assert_eq!(read(&env.install_path, "hl2.exe").unwrap(), "старая");
        assert_eq!(
            read(&env.install_path, "ghostingmod/maps").unwrap(),
            "файл на месте папки"
        );
        assert_eq!(
            read(&env.install_path, "episodic/gameinfo.txt").unwrap(),
            "эпизод 1"
        );
        assert!(!env.park.exists());
    }
}

// ---------------------------------------------------------------- episode-button-selftest.js

fn ready() -> Value {
    json!({ "state": "ready", "action": "play", "enabled": true, "spinner": false, "progress": false })
}

fn ghosting(episodes: Value) -> Value {
    json!({ "installedVersion": "1.0.13", "operation": null, "episodes": episodes })
}

fn both_missing() -> Value {
    json!({ "ep1": { "installed": false, "paused": false }, "ep2": { "installed": false, "paused": false } })
}

#[test]
fn episode_button_replaces_only_play_of_an_installed_ghosting() {
    let delivery = ghosting(both_missing());
    assert_eq!(
        episode_button(&ready(), Some(&delivery), None),
        ready(),
        "без эпизода"
    );

    let install = main_button::resolve(&Input {
        delivery: Some(&json!({ "state": "not-installed", "installable": true })),
        ..Input::default()
    });
    assert_eq!(install["state"], "not-installed");
    let missing = json!({ "installedVersion": null, "episodes": both_missing() });
    assert_eq!(
        episode_button(&install, Some(&missing), Some("ep1")),
        install
    );

    let update = main_button::resolve(&Input {
        delivery: Some(&json!({ "installedVersion": "1.0.13" })),
        update: Some(&json!({ "updateAvailable": true, "availableVersion": "1.0.14" })),
        ..Input::default()
    });
    assert_eq!(update["state"], "update-available");
    assert_eq!(
        episode_button(&update, Some(&delivery), Some("ep1")),
        update
    );
    let repair = main_button::resolve(&Input {
        delivery: Some(
            &json!({ "installedVersion": "1.0.13", "state": "files-damaged", "integrity": { "damaged": 2 } }),
        ),
        ..Input::default()
    });
    assert_eq!(repair["state"], "repair-required");
    assert_eq!(
        episode_button(&repair, Some(&delivery), Some("ep2")),
        repair
    );

    for state in [
        "checking-updates",
        "unavailable",
        "error",
        "update-error",
        "update-launcher-required",
        "starting-game",
        "game-running",
        "stopping-game",
        "busy",
        "installing",
        "downloading",
    ] {
        let button = json!({ "state": state, "action": "none", "enabled": false });
        assert_eq!(
            episode_button(&button, Some(&delivery), Some("ep1")),
            button,
            "{state}"
        );
    }

    let paused = ghosting(json!({ "ep1": { "installed": false, "paused": true } }));
    assert_eq!(
        episode_button(&ready(), Some(&paused), Some("ep1")),
        json!({ "state": "episode-paused", "action": "resume", "enabled": true, "spinner": false, "progress": false })
    );
    assert_eq!(
        episode_button(&ready(), Some(&delivery), Some("ep2")),
        json!({ "state": "episode-not-installed", "action": "install", "enabled": true, "spinner": false, "progress": false })
    );
    assert_eq!(
        episode_button(&ready(), Some(&ghosting(Value::Null)), Some("ep1"))["state"],
        "episode-not-installed",
        "статус без episodes"
    );

    let installed =
        ghosting(json!({ "ep1": { "installed": true, "version": "1.0.0", "paused": false } }));
    assert_eq!(
        episode_button(&ready(), Some(&installed), Some("ep1")),
        ready()
    );
    let update = json!({ "state": "update-available", "action": "update", "enabled": true });
    assert_eq!(
        episode_button(&update, Some(&installed), Some("ep1")),
        update
    );
}

// ---------------------------------------------------------------- episode-install-selftest.js

fn unsigned(id: &str, version: &str, archive: Value) -> Map<String, Value> {
    let requires = episodes::episode_of(id).unwrap().requires;
    json!({
        "schemaVersion": 1,
        "gameId": GAME,
        "episode": id,
        "version": version,
        "publishedAt": "2026-11-14T00:00:00.000Z",
        "requires": requires,
        "archive": archive,
    })
    .as_object()
    .unwrap()
    .clone()
}

fn archive_of(id: &str, version: &str, size: usize, sha256: &str) -> Value {
    json!({ "fileName": file_name_of(id, version), "size": size, "sha256": sha256 })
}

fn file_name_of(id: &str, version: &str) -> String {
    format!("{GAME}-{id}-{version}.7z")
}

fn signed(keys: &KeyPair, manifest: &Map<String, Value>) -> Value {
    Value::Object(attach(manifest, &keys.private_key_pem).unwrap())
}

#[test]
fn the_episode_manifest_is_strict_about_signature_fields_and_archive() {
    let keys = generate_key_pair();
    let trusted = [keys.public_key_pem.clone()];
    let a64 = "a".repeat(64);
    let good = signed(
        &keys,
        &unsigned("ep1", "1.0.0", archive_of("ep1", "1.0.0", 10, &a64)),
    );
    let validate = |data: &Value, id: &str| episode_install::validate_manifest(data, id, &trusted);
    assert_eq!(
        validate(&good, "ep1").unwrap()["archive"]["fileName"],
        file_name_of("ep1", "1.0.0")
    );

    let mut flipped = good.clone();
    flipped["version"] = json!("1.0.1");
    assert_eq!(
        validate(&flipped, "ep1").unwrap_err().code,
        Code::SignatureInvalid
    );
    let plain = Value::Object(unsigned(
        "ep1",
        "1.0.0",
        archive_of("ep1", "1.0.0", 10, &a64),
    ));
    assert_eq!(
        validate(&plain, "ep1").unwrap_err().code,
        Code::SignatureInvalid
    );
    let stranger = [generate_key_pair().public_key_pem];
    assert_eq!(
        episode_install::validate_manifest(&good, "ep1", &stranger)
            .unwrap_err()
            .code,
        Code::SignatureInvalid
    );

    let ep2 = signed(
        &keys,
        &unsigned("ep2", "1.0.0", archive_of("ep2", "1.0.0", 10, &a64)),
    );
    assert_eq!(
        validate(&ep2, "ep1").unwrap_err().code,
        Code::ManifestInvalid
    );
    let escape = signed(
        &keys,
        &unsigned(
            "ep1",
            "1.0.0",
            json!({ "fileName": "../x.7z", "size": 10, "sha256": a64 }),
        ),
    );
    assert_eq!(
        validate(&escape, "ep1").unwrap_err().code,
        Code::ManifestInvalid
    );
    let foreign = signed(
        &keys,
        &unsigned("ep1", "1.0.0", archive_of("ep2", "1.0.0", 10, &a64)),
    );
    assert_eq!(
        validate(&foreign, "ep1").unwrap_err().code,
        Code::ManifestInvalid,
        "архив чужого эпизода"
    );
    let mut extra = unsigned("ep1", "1.0.0", archive_of("ep1", "1.0.0", 10, &a64));
    extra.insert("extra".to_owned(), json!(1));
    assert_eq!(
        validate(&signed(&keys, &extra), "ep1").unwrap_err().code,
        Code::ManifestInvalid
    );
    let mut no_requires = unsigned("ep2", "1.0.0", archive_of("ep2", "1.0.0", 10, &a64));
    no_requires.insert("requires".to_owned(), json!([]));
    assert_eq!(
        validate(&signed(&keys, &no_requires), "ep2")
            .unwrap_err()
            .code,
        Code::ManifestInvalid
    );
    let mut bad_size = unsigned("ep1", "1.0.0", archive_of("ep1", "1.0.0", 10, &a64));
    bad_size["archive"]["size"] = json!(1.5);
    assert_eq!(
        validate(&signed(&keys, &bad_size), "ep1").unwrap_err().code,
        Code::ManifestInvalid
    );
}

#[test]
fn the_episode_manifest_address_comes_from_the_ghosting_channel() {
    let env = Environment::new(
        [(
            "HLSR_GAME_DELIVERY_CHANNEL_URL",
            "https://hlsr.pro/downloads/ghosting-renewed/channel/stable.json",
        )],
        false,
    );
    let source = episode_install::manifest_source("ep2", &env).unwrap();
    assert_eq!(
        source.url,
        "https://hlsr.pro/downloads/ghosting-renewed/episodes/ep2/stable.json"
    );
    let none = Environment::new(Vec::<(String, String)>::new(), false);
    assert_eq!(episode_install::manifest_source("ep1", &none), None);
    let manifest = json!({ "archive": { "fileName": file_name_of("ep2", "1.0.0") } });
    let descriptor = episode_install::provider_descriptor(&manifest, &source.url).unwrap();
    assert_eq!(descriptor["type"], "https-direct");
    assert_eq!(
        descriptor["url"],
        format!("https://hlsr.pro/downloads/ghosting-renewed/episodes/ep2/{GAME}-ep2-1.0.0.7z")
    );
}

struct Env {
    _temp: tempfile::TempDir,
    dir: PathBuf,
    keys: KeyPair,
    server: Server,
    log: RequestLog,
    service: Arc<GameDeliveryService>,
    install_path: PathBuf,
}

impl Env {
    async fn new(installed: bool, channel: bool) -> Env {
        let (temp, dir) = temp_root();
        let keys = generate_key_pair();
        let log = RequestLog::default();
        let server = start_server("main", log.clone()).await;
        let mut manifest = manifests::manifest_of(GAME).unwrap().clone();
        manifest
            .as_object_mut()
            .unwrap()
            .shift_remove("buildMetadata");
        let env = if channel {
            Environment::new(
                [
                    ("GAME_DELIVERY_ALLOW_LOCAL_HTTP".to_owned(), "1".to_owned()),
                    (
                        "HLSR_GAME_DELIVERY_CHANNEL_URL".to_owned(),
                        format!("http://{}/{GAME}/channel/stable.json", server.authority),
                    ),
                    (
                        "HLSR_GAME_DELIVERY_CHANNEL_KEY".to_owned(),
                        keys.public_key_pem.replace('\n', "\\n"),
                    ),
                ],
                false,
            )
        } else {
            Environment::new(Vec::<(String, String)>::new(), false)
        };
        let service = GameDeliveryService::new(ServiceOptions {
            library_path: dir.join("library").to_string_lossy().into_owned(),
            launcher_version: Some("0.4.9".to_owned()),
            registry: Some(Registry {
                manifests: vec![(GAME.to_owned(), manifest)],
                files: Registry::bundled().files,
            }),
            channel_service: Provided::Off,
            origin_updates: Provided::Off,
            update_check_interval_ms: Some(0),
            max_attempts: Some(1),
            backoff_base_ms: Some(1),
            env: Some(env),
            logger: Some(Arc::new(MemoryLogger::default())),
            ..ServiceOptions::default()
        })
        .unwrap();
        let install_path = game_install_path(service.paths(), GAME).unwrap();
        if installed {
            write(
                &install_path,
                "ghostingmod/bin/client.dll",
                "ghosting client",
            );
            write(
                &install_path,
                "ghostingmod/cfg/config.cfg",
                "bind w +forward\n",
            );
            service
                .install_service()
                .write_installed(
                    &InstallTarget {
                        game_id: GAME.to_owned(),
                        version: json!("1.0.13"),
                        install_path: install_path.clone(),
                        staging_path: dir.join("unused"),
                        published_at: Some("2026-11-12T00:00:00.000Z".to_owned()),
                        manifest_sha256: None,
                    },
                    json!("a".repeat(64)),
                    None,
                )
                .unwrap();
        }
        Env {
            _temp: temp,
            dir,
            keys,
            server,
            log,
            service,
            install_path,
        }
    }

    fn route(&self, path: String, response: Value) {
        self.server
            .routes
            .lock()
            .unwrap()
            .insert(path, json!([response]));
    }

    /// Выложить эпизод: архив и подписанный манифест рядом, как на hlsr.pro.
    fn publish(&self, id: &str, version: &str, archive: &[u8], sha256: Option<&str>) {
        let file_name = file_name_of(id, version);
        let sha = sha256.map_or_else(|| support::sha256_hex(archive), str::to_owned);
        let manifest = signed(
            &self.keys,
            &unsigned(id, version, archive_of(id, version, archive.len(), &sha)),
        );
        let hex: String = archive.iter().map(|b| format!("{b:02x}")).collect();
        self.route(
            format!("/{GAME}/episodes/{id}/{file_name}"),
            json!({ "bodyHex": hex }),
        );
        self.route(
            format!("/{GAME}/episodes/{id}/stable.json"),
            json!({ "body": manifest.to_string() }),
        );
    }

    fn archive(&self, name: &str, files: &[(&str, &str)]) -> Vec<u8> {
        make_7z(&self.dir, name, files).1
    }

    fn leftovers(&self) -> Vec<String> {
        let paths = self.service.paths();
        let mut all = names(&paths.staging);
        all.extend(names(&paths.downloads.join(GAME)));
        all
    }

    fn park(&self) -> PathBuf {
        self.service.install_service().episodes_park_path(GAME)
    }

    fn archive_requests(&self, id: &str, version: &str) -> usize {
        let tail = format!("/{}", file_name_of(id, version));
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|entry| {
                entry["url"]
                    .as_str()
                    .is_some_and(|url| url.ends_with(&tail))
            })
            .count()
    }

    fn assert_ghosting_untouched(&self) {
        assert_eq!(
            read(&self.install_path, "ghostingmod/bin/client.dll").unwrap(),
            "ghosting client"
        );
        assert_eq!(
            read(&self.install_path, "ghostingmod/cfg/config.cfg").unwrap(),
            "bind w +forward\n"
        );
    }

    fn version_of(&self, id: &str) -> Value {
        episodes::status(Some(&self.install_path))[id]["version"].clone()
    }

    /// `.part` и его состояние, как их оставляет менеджер загрузок на паузе.
    fn paused_part(&self, file_name: &str, bytes: &[u8]) -> (PathBuf, PathBuf) {
        let manager = self.service.download_manager();
        let files = manager.part_paths(GAME, file_name).unwrap();
        std::fs::create_dir_all(&files.dir).unwrap();
        std::fs::write(&files.part, bytes).unwrap();
        manager
            .write_state(
                &files.state,
                &StateFile {
                    game_id: Some(GAME.to_owned()),
                    filename: Some(file_name.to_owned()),
                    expected_size: Some(1000),
                    expected_sha256: Some("d".repeat(64)),
                    provider_id: None,
                    bytes_downloaded: Some(bytes.len() as u64),
                },
            )
            .unwrap();
        (files.part, files.state)
    }
}

const EP1: [(&str, &str); 2] = [
    (
        "episodic/gameinfo.txt",
        "\"GameInfo\" { game \"Episode One\" }\n",
    ),
    ("episodic/cfg/chapter1.cfg", "map ep1_citadel_00\n"),
];
const EP2: [(&str, &str); 2] = [
    (
        "ep2/gameinfo.txt",
        "\"GameInfo\" { game \"Episode Two\" }\n",
    ),
    ("ep2/cfg/Chapter1.cfg", "map ep2_outland_01\n"),
];

async fn install_code(env: &Env, id: &str) -> Code {
    code(env.service.install_episode(id).await.unwrap_err())
}

#[tokio::test]
async fn episode_one_installs_into_the_ghosting_folder() {
    let env = Env::new(true, true).await;
    let archive = env.archive("ep1.7z", &EP1);
    env.publish("ep1", "1.0.0", &archive, None);

    let result = env.service.install_episode("ep1").await.unwrap();
    assert_eq!(
        result,
        json!({ "status": "installed", "episode": "ep1", "version": "1.0.0" })
    );
    let marker: Value =
        serde_json::from_str(&read(&env.install_path, &format!("episodic/{MARKER}")).unwrap())
            .unwrap();
    assert_eq!(marker["version"], "1.0.0");
    assert_eq!(marker["sha256"], support::sha256_hex(&archive));
    assert_eq!(marker["stockCfg"], json!(["chapter1.cfg"]));
    assert!(
        marker["installedAt"]
            .as_str()
            .is_some_and(|at| at.ends_with('Z'))
    );
    env.assert_ghosting_untouched();
    assert!(
        env.leftovers().is_empty(),
        "ни staging, ни архива в Downloads: {:?}",
        env.leftovers()
    );

    let status = env.service.status_of(GAME);
    assert_eq!(status["state"], "ready");
    assert_eq!(status["episode"], Value::Null);
    assert_eq!(status["operation"], Value::Null);
    assert_eq!(
        status["episodes"]["ep1"],
        json!({ "installed": true, "version": "1.0.0", "size": archive.len(), "paused": false })
    );
    assert!(!env.service.is_busy());
}

#[tokio::test]
async fn reinstalling_an_episode_carries_player_data_into_the_new_folder() {
    let env = Env::new(true, true).await;
    env.publish("ep1", "1.0.0", &env.archive("ep1.7z", &EP1), None);
    env.service.install_episode("ep1").await.unwrap();

    let player = [
        ("episodic/SAVE/quick.sav", "quick"),
        ("episodic/runs/pb.dem", "pb"),
        ("episodic/custom/hud/layout.res", "my hud"),
        ("episodic/screenshots/s.jpg", "shot"),
        ("episodic/run1.dem", "root demo"),
    ];
    for (relative, data) in player {
        write(&env.install_path, relative, data);
    }
    let next = env.archive(
        "ep1-next.7z",
        &[
            ("episodic/gameinfo.txt", "v2\n"),
            ("episodic/save/stock.sav", "stock"),
            ("episodic/maps/new.bsp", "bsp"),
        ],
    );
    env.publish("ep1", "1.0.1", &next, None);
    env.service.install_episode("ep1").await.unwrap();

    assert_eq!(env.version_of("ep1"), "1.0.1");
    for (relative, data) in player {
        assert_eq!(
            read(&env.install_path, relative).unwrap(),
            data,
            "{relative}"
        );
    }
    assert!(
        !names(&env.install_path.join("episodic").join("SAVE")).contains(&"stock.sav".to_owned()),
        "SAVE игрока целиком вместо save из архива"
    );
    assert_eq!(
        read(&env.install_path, "episodic/maps/new.bsp").unwrap(),
        "bsp"
    );
    assert_eq!(
        read(&env.install_path, "episodic/cfg/chapter1.cfg"),
        None,
        "прочее — из нового архива"
    );
    assert!(
        !env.park().join("episodic").exists(),
        "прежняя копия удалена"
    );
    env.assert_ghosting_untouched();
    assert!(env.leftovers().is_empty());
}

#[tokio::test]
async fn an_older_episode_version_is_refused_and_the_same_one_reinstalls() {
    let env = Env::new(true, true).await;
    env.publish("ep1", "1.0.1", &env.archive("ep1-v2.7z", &EP1), None);
    env.service.install_episode("ep1").await.unwrap();

    env.publish("ep1", "1.0.0", &env.archive("ep1.7z", &EP1), None);
    let error = to_delivery_error(env.service.install_episode("ep1").await.unwrap_err(), None);
    assert_eq!(error.code, Code::ManifestInvalid);
    assert!(error.message.contains("Откат версии"), "{}", error.message);
    assert_eq!(env.version_of("ep1"), "1.0.1");
    assert_eq!(
        env.archive_requests("ep1", "1.0.0"),
        0,
        "старый архив даже не качается"
    );
    assert_eq!(
        env.service.status_of(GAME)["error"],
        Value::Null,
        "ошибка эпизода не висит на карточке Ghosting"
    );

    env.publish("ep1", "1.0.1", &env.archive("ep1-v2-again.7z", &EP1), None);
    assert_eq!(
        env.service.install_episode("ep1").await.unwrap()["version"],
        "1.0.1"
    );
}

#[tokio::test]
async fn an_archive_with_a_foreign_root_is_refused_before_extraction() {
    let env = Env::new(true, true).await;
    let mut files = vec![("ep2/x", "x")];
    files.extend(EP1);
    env.publish("ep1", "1.0.0", &env.archive("two-roots.7z", &files), None);
    assert_eq!(install_code(&env, "ep1").await, Code::ArchiveUnsafeEntry);
    assert!(!env.install_path.join("episodic").exists());
    assert!(!env.install_path.join("ep2").exists());
    assert!(env.leftovers().is_empty(), "доказанно плохой архив удалён");
    assert_eq!(env.service.status_of(GAME)["error"], Value::Null);
    assert!(!env.service.is_busy());
}

#[tokio::test]
async fn a_wrong_sha256_leaves_the_installed_episode_alone() {
    let env = Env::new(true, true).await;
    let archive = env.archive("ep1.7z", &EP1);
    env.publish("ep1", "1.0.0", &archive, Some(&"b".repeat(64)));
    assert_eq!(install_code(&env, "ep1").await, Code::ChecksumMismatch);
    assert!(!env.install_path.join("episodic").exists());
    assert!(!env.service.is_busy());

    env.publish("ep1", "1.0.0", &archive, None);
    env.service.install_episode("ep1").await.unwrap();
    let next = env.archive("ep1-next.7z", &[("episodic/gameinfo.txt", "v2\n")]);
    env.publish("ep1", "1.0.1", &next, Some(&"c".repeat(64)));
    assert_eq!(install_code(&env, "ep1").await, Code::ChecksumMismatch);
    assert_eq!(env.version_of("ep1"), "1.0.0");
    assert!(
        env.install_path
            .join("episodic")
            .join("cfg")
            .join("chapter1.cfg")
            .exists()
    );
    assert!(env.leftovers().is_empty());
}

#[tokio::test]
async fn episode_two_brings_episode_one_and_removing_one_removes_both() {
    let env = Env::new(true, true).await;
    env.publish("ep1", "1.0.0", &env.archive("ep1.7z", &EP1), None);
    env.publish("ep2", "1.0.0", &env.archive("ep2.7z", &EP2), None);

    let result = env.service.install_episode("ep2").await.unwrap();
    assert_eq!(result["episode"], "ep2");
    let state = episodes::status(Some(&env.install_path));
    assert_eq!(
        (
            state["ep1"]["installed"].clone(),
            state["ep2"]["installed"].clone()
        ),
        (json!(true), json!(true))
    );
    assert_eq!(
        episodes::read_marker(&env.install_path, "ep2")["stockCfg"],
        json!(["chapter1.cfg"])
    );

    let removed = env.service.remove_episode("ep1").unwrap();
    assert_eq!(
        removed,
        json!({ "status": "removed", "removed": ["ep2", "episodic"] })
    );
    assert!(!env.install_path.join("episodic").exists() && !env.install_path.join("ep2").exists());
    env.assert_ghosting_untouched();
    assert!(!env.service.is_busy());
}

#[tokio::test]
async fn without_ghosting_or_with_an_unknown_episode_nothing_starts() {
    let env = Env::new(false, true).await;
    env.publish("ep1", "1.0.0", &env.archive("ep1.7z", &EP1), None);
    assert_eq!(install_code(&env, "ep1").await, Code::NotInstalled);
    assert_eq!(
        code(env.service.remove_episode("ep1").unwrap_err()),
        Code::NotInstalled
    );
    assert_eq!(install_code(&env, "ep3").await, Code::LaunchConfigInvalid);
    assert_eq!(
        episodes::episode_id_of(&json!(["ep1"])).unwrap_err().code,
        Code::LaunchConfigInvalid
    );
    assert!(!env.service.is_busy());
}

#[tokio::test]
async fn episodes_status_reports_size_and_remembers_a_refusal() {
    let offline = Env::new(true, false).await;
    let none = offline.service.episodes_status().await;
    assert_eq!(
        none["ep1"],
        json!({ "installed": false, "version": null, "size": null, "paused": false, "available": false })
    );
    assert_eq!(
        install_code(&offline, "ep1").await,
        Code::ChannelUnavailable
    );

    let env = Env::new(true, true).await;
    let archive = env.archive("ep1.7z", &EP1);
    env.publish("ep1", "1.0.0", &archive, None);
    let view = env.service.episodes_status().await;
    assert_eq!(
        view["ep1"],
        json!({ "installed": false, "version": null, "size": archive.len(), "paused": false, "available": true })
    );
    assert_eq!(view["ep2"]["available"], false);
    assert_eq!(
        env.service.status_of(GAME)["episodes"]["ep1"]["size"],
        archive.len(),
        "размер приходит и событием статуса"
    );

    let asked = |env: &Env| {
        env.log
            .lock()
            .unwrap()
            .iter()
            .filter(|entry| {
                entry["url"]
                    .as_str()
                    .is_some_and(|url| url.ends_with("/ep2/stable.json"))
            })
            .count()
    };
    let before = asked(&env);
    assert_eq!(
        env.service.episodes_status().await["ep2"]["available"],
        false
    );
    assert_eq!(asked(&env), before, "второй раз сервер не спрошен");
}

#[tokio::test]
async fn a_manifest_over_64_kib_is_refused() {
    let env = Env::new(true, true).await;
    env.route(
        format!("/{GAME}/episodes/ep1/stable.json"),
        json!({ "body": format!("{{\"pad\":\"{}\"}}", "x".repeat(70 * 1024)) }),
    );
    let error = env.service.install_episode("ep1").await.unwrap_err();
    assert_eq!(
        code(error),
        Code::HttpError,
        "тело больше 64 КиБ не читается"
    );
    assert!(!env.install_path.join("episodic").exists());
}

#[tokio::test]
async fn cancel_keeps_episode_parts_for_ghosting_and_removes_them_for_the_episode() {
    let env = Env::new(true, true).await;
    // Без манифеста в памяти (как после перезапуска лаунчера) — по файлам в Downloads.
    let (episode_part, _) = env.paused_part(&file_name_of("ep1", "0.9.0"), b"12345");
    assert_eq!(
        env.service.refresh(GAME).unwrap()["episodes"]["ep1"]["paused"],
        true
    );

    // Своя загрузка Ghosting на паузе: её отмена снимает только её .part.
    let (ghosting_part, _) = env.paused_part(&format!("{GAME}-1.0.13.7z"), b"abc");
    env.service.set_status(GAME, json!({ "state": "paused" }));
    assert!(env.service.cancel(Some(GAME)).unwrap());
    assert!(!ghosting_part.exists());
    assert!(episode_part.exists());

    // Отмена эпизода на паузе, когда Ghosting стоит.
    env.service.refresh(GAME).unwrap();
    assert!(env.service.cancel(Some(GAME)).unwrap());
    assert!(!episode_part.exists());
    assert_eq!(
        env.service.status_of(GAME)["episodes"]["ep1"]["paused"],
        false
    );
    assert!(
        !env.service.cancel(Some(GAME)).unwrap(),
        "убирать больше нечего"
    );

    // Из карточки эпизода — только его .part; чужой id — архивы всех.
    let (ep1, _) = env.paused_part(&file_name_of("ep1", "1.0.0"), b"12345");
    let (ep2, _) = env.paused_part(&file_name_of("ep2", "1.0.0"), b"67890");
    env.service.refresh(GAME).unwrap();
    assert!(env.service.cancel_with(Some(GAME), Some("ep2")).unwrap());
    assert!(!ep2.exists());
    assert!(ep1.exists(), "Episode One на паузе остался");
    assert_eq!(
        env.service.status_of(GAME)["episodes"]["ep1"]["paused"],
        true
    );
    assert!(env.service.cancel_with(Some(GAME), Some("../ep1")).unwrap());
    assert!(!ep1.exists());

    // Сервер выложил новую версию: .part прежней убирается, когда начинается новая.
    let (stale, stale_state) = env.paused_part(&file_name_of("ep1", "0.9.0"), b"12345");
    env.publish("ep1", "1.0.0", &env.archive("ep1.7z", &EP1), None);
    env.service.install_episode("ep1").await.unwrap();
    assert!(!stale.exists() && !stale_state.exists());
    assert!(env.leftovers().is_empty());
}

/// Крах после того, как новая папка встала, но до переезда данных игрока: новая — на месте,
/// прежняя с SAVE и демо — в park. Старт лаунчера переносит их в эпизод.
async fn crashed_after_place() -> (Env, PathBuf) {
    let env = Env::new(true, true).await;
    env.publish("ep1", "1.0.0", &env.archive("ep1.7z", &EP1), None);
    env.service.install_episode("ep1").await.unwrap();
    let parked = env.park().join("episodic");
    write(&parked, "SAVE/quick.sav", "quick");
    write(&parked, "run1.dem", "root demo");
    write(&parked, "maps/old.bsp", "old content");
    write(&parked, MARKER, json!({ "version": "0.9.0" }).to_string());
    (env, parked)
}

#[tokio::test]
async fn after_a_crash_unpark_moves_saves_and_demos_and_drops_game_content() {
    let (env, parked) = crashed_after_place().await;
    env.service
        .install_service()
        .unpark_episodes(GAME, &env.install_path);
    let episode = env.install_path.join("episodic");
    assert_eq!(read(&episode, "SAVE/quick.sav").unwrap(), "quick");
    assert_eq!(read(&episode, "run1.dem").unwrap(), "root demo");
    assert!(!episode.join("maps").join("old.bsp").exists());
    assert_eq!(
        env.version_of("ep1"),
        "1.0.0",
        "маркер новой папки не тронут"
    );
    assert!(!parked.exists());
    assert!(!env.park().exists());
}

#[tokio::test]
async fn after_a_crash_with_its_own_save_nothing_of_the_player_is_deleted() {
    let (env, _) = crashed_after_place().await;
    write(&env.install_path, "episodic/save/own.sav", "own");
    env.service
        .install_service()
        .unpark_episodes(GAME, &env.install_path);
    assert_eq!(
        read(&env.install_path, "episodic/save/own.sav").unwrap(),
        "own"
    );
    assert_eq!(
        read(&env.install_path, "episodic/run1.dem").unwrap(),
        "root demo"
    );
    let aside: Vec<String> = names(&env.park())
        .into_iter()
        .filter(|name| name.starts_with("episodic.player-"))
        .collect();
    assert_eq!(aside.len(), 1);
    assert_eq!(
        read(&env.park().join(&aside[0]), "SAVE/quick.sav").unwrap(),
        "quick"
    );
    assert!(!env.park().join("episodic").exists());
}
