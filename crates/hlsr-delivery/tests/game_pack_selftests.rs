//! Самотест 0.4.9 `scripts/game-pack-selftest.js`: полная сборка в архиве `.hlsrpack` — экспорт
//! папки игры и импорт с проверкой по подписанному списку файлов. На настоящем 7za из
//! `resources/7zip`.

mod support;

use hlsr_delivery::archive_service::{ArchiveService, CreateOptions};
use hlsr_delivery::errors::{Code, Failure};
use hlsr_delivery::files_manifest::{self, BuildOptions};
use hlsr_delivery::game_delivery_service::{
    GameDeliveryService, PackExtra, Provided, Registry, ServiceOptions,
};
use hlsr_delivery::game_pack::{self, ParseOptions};
use hlsr_delivery::install_service::InstallTarget;
use hlsr_delivery::manifests;
use hlsr_delivery::paths::{LibraryPaths, game_install_path};
use hlsr_delivery::ports::{DepotProgressFn, Logger, MemoryLogger, SteamDepot};
use hlsr_delivery::providers::BoxFuture;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::scenario::temp_root;

const GAME: &str = "hl1-won";

fn mz() -> Vec<u8> {
    let mut bytes = b"MZ".to_vec();
    bytes.extend([7u8; 200]);
    bytes
}

fn real_7za() -> PathBuf {
    hlsr_core::paths::Resources::detect().sevenzip()
}

fn write(root: &Path, relative: &str, data: impl AsRef<[u8]>) {
    let full = root.join(relative);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, data).unwrap();
}

fn read(root: &Path, relative: &str) -> String {
    std::fs::read_to_string(root.join(relative)).unwrap()
}

fn exists(root: &Path, relative: &str) -> bool {
    root.join(relative).exists()
}

/// Лицензии есть (`depotService: {assertOwns: async () => true}`).
struct Owns;

impl SteamDepot for Owns {
    fn assert_owns<'a>(&'a self, _app_ids: &'a [u32]) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(async { Ok(()) })
    }

    fn install<'a>(
        &'a self,
        _paths: &'a LibraryPaths,
        _game_id: &'a str,
        _on_progress: DepotProgressFn,
    ) -> BoxFuture<'a, Result<Value, Failure>> {
        unreachable!("сборки из депо в самотесте нет")
    }
}

struct Env {
    _temp: tempfile::TempDir,
    dir: PathBuf,
    build_root: PathBuf,
    service: Arc<GameDeliveryService>,
    install_path: PathBuf,
}

/// Сборка: exe и данные — в подписанном списке; Bunnymod XT в списке тоже есть, но проверка его
/// не сверяет (лаунчер обновляет его сам).
fn build_files(root: &Path) {
    write(root, "Half-Life/hl.exe", mz());
    write(
        root,
        "Half-Life/valve_WON/pak0.pak",
        "pak-data ".repeat(5000),
    );
    write(root, "Half-Life/valve_WON/config.cfg", "name player\n");
    write(root, "Bunnymod XT/Injector.exe", mz());
}

fn setup() -> Env {
    let (temp, dir) = temp_root();
    let build_root = dir.join("build");
    build_files(&build_root);
    let mut options = BuildOptions::new(&build_root);
    options.game_id = Some(GAME);
    options.version = Some("1.0.0");
    let files = files_manifest::build(options).unwrap().manifest;
    let mut manifest = manifests::manifests()
        .into_iter()
        .find(|(id, _)| *id == GAME)
        .unwrap()
        .1
        .clone();
    manifest
        .as_object_mut()
        .unwrap()
        .shift_remove("buildMetadata");
    let logger: Arc<dyn Logger> = Arc::new(MemoryLogger::default());
    let service = GameDeliveryService::new(ServiceOptions {
        library_path: dir.join("library").to_string_lossy().into_owned(),
        launcher_version: Some("0.4.6".to_owned()),
        registry: Some(Registry {
            manifests: vec![(GAME.to_owned(), manifest)],
            files: vec![(GAME.to_owned(), vec![("1.0.0".to_owned(), files)])],
        }),
        channel_service: Provided::Off,
        depot: Some(Arc::new(Owns)),
        update_check_interval_ms: Some(0),
        archive_service: Some(ArchiveService::new(real_7za(), logger.clone())),
        logger: Some(logger),
        ..ServiceOptions::default()
    })
    .unwrap();
    let install_path = game_install_path(service.paths(), GAME).unwrap();
    Env {
        _temp: temp,
        dir,
        build_root,
        service,
        install_path,
    }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn install(env: &Env, extra: &[(&str, &[u8])]) {
    copy_tree(&env.build_root, &env.install_path);
    for (relative, data) in extra {
        write(&env.install_path, relative, data);
    }
    env.service
        .install_service()
        .write_installed(
            &InstallTarget {
                game_id: GAME.to_owned(),
                version: json!("1.0.0"),
                install_path: env.install_path.clone(),
                staging_path: env.install_path.clone(),
                published_at: Some("2026-09-01T00:00:00.000Z".to_owned()),
                manifest_sha256: None,
            },
            json!("a".repeat(64)),
            None,
        )
        .unwrap();
}

async fn export_pack(env: &Env, name: &str) -> (Value, PathBuf) {
    let file = env.dir.join(name);
    let result = env
        .service
        .export_pack(
            GAME,
            &file,
            PackExtra {
                launcher_id: Some(json!("70")),
                build: Some(Value::Null),
                launcher_version: Some(json!("0.4.6")),
                settings: Some(json!({"args": "-dev"})),
                allow_modified: false,
            },
        )
        .await
        .unwrap();
    (result, file)
}

async fn import_pack(env: &Env, file: &Path) -> Result<Value, Failure> {
    let meta = env.service.read_pack_meta(file).await?;
    env.service.import_pack(file, &meta).await
}

fn code(error: &Failure) -> String {
    match error {
        Failure::Delivery(error) => error.code.as_str().to_owned(),
        Failure::Raw(error) => format!("raw: {}", error.message),
    }
}

fn staging_is_empty(env: &Env) -> bool {
    std::fs::read_dir(&env.service.paths().staging)
        .unwrap()
        .next()
        .is_none()
}

#[tokio::test]
async fn export_puts_game_saves_and_cfg_in_one_archive_meta_first_without_installed_json() {
    let env = setup();
    install(
        &env,
        &[
            ("Half-Life/valve_WON/SAVE/quick.sav", b"save"),
            ("Half-Life/valve_WON/config.cfg", b"name mine\n"),
        ],
    );

    let (result, file) = export_pack(&env, "hl1.hlsrpack").await;
    assert_eq!(result["status"], "ok");
    assert!(!PathBuf::from(format!("{}.hlsr-partial", file.display())).exists());

    let listing = ArchiveService::new(real_7za(), Arc::new(MemoryLogger::default()))
        .list(&file)
        .await
        .unwrap();
    // 7za в Windows пишет пути в листинге через «\», ArchiveService отдаёт их как есть.
    let names: Vec<String> = listing
        .entries
        .iter()
        .filter(|entry| !entry.is_directory)
        .map(|entry| entry.path.replace('\\', "/"))
        .collect();
    let has = |name: &str| names.iter().any(|entry| entry == name);
    assert_eq!(names[0], game_pack::META_FILE);
    assert!(has("Half-Life/valve_WON/SAVE/quick.sav"));
    assert!(has("Half-Life/hl.exe"));
    assert!(!has("installed.json"));

    let meta = env.service.read_pack_meta(&file).await.unwrap();
    assert_eq!(meta.game_id, GAME);
    assert_eq!(meta.version, "1.0.0");
    assert_eq!(meta.installed_version, Some(Some("1.0.0".to_owned())));
    assert_eq!(meta.settings, Some(json!({"args": "-dev"})));
    assert_eq!(env.service.status_of(GAME)["state"], "ready");
}

#[tokio::test]
async fn import_into_clean_library_installs_build_with_saves_and_cfg_bxt_downloads_again() {
    let source = setup();
    install(
        &source,
        &[
            ("Half-Life/valve_WON/SAVE/quick.sav", b"save"),
            ("Half-Life/valve_WON/config.cfg", b"name mine\n"),
            ("Bunnymod XT/version", b"1.2.3"),
            ("Bunnymod XT/.hlsr-injector-version", b"1.0"),
        ],
    );
    let (_, file) = export_pack(&source, "hl1.hlsrpack").await;

    let target = setup();
    let meta = target.service.read_pack_meta(&file).await.unwrap();
    assert_eq!(meta.installed_version, Some(None));
    let result = target.service.import_pack(&file, &meta).await.unwrap();

    assert_eq!(result["status"], "ok");
    let installed = target
        .service
        .install_service()
        .read_installed(GAME)
        .unwrap()
        .unwrap();
    assert_eq!(installed["version"], "1.0.0");
    assert_eq!(installed["publishedAt"], "2026-09-01T00:00:00.000Z");
    assert_eq!(
        read(&target.install_path, "Half-Life/valve_WON/SAVE/quick.sav"),
        "save"
    );
    assert_eq!(
        read(&target.install_path, "Half-Life/valve_WON/config.cfg"),
        "name mine\n"
    );
    assert!(
        exists(&target.install_path, "Half-Life/hl.exe"),
        "exe из подписанного списка остаётся"
    );
    // Bunnymod XT проверка не сверяет — из архива он не берётся, а без файла версии лаунчер
    // скачает его перед запуском.
    assert!(!exists(&target.install_path, "Bunnymod XT/Injector.exe"));
    assert!(!exists(
        &target.install_path,
        "Bunnymod XT/.hlsr-injector-version"
    ));
    assert_eq!(result["dropped"], 1);
    assert!(staging_is_empty(&target));
    assert_eq!(target.service.status_of(GAME)["state"], "ready");
}

#[tokio::test]
async fn import_drops_foreign_and_unverified_executables_even_in_saves() {
    let source = setup();
    let mz = mz();
    install(
        &source,
        &[
            ("Half-Life/opengl32.dll", &mz),
            ("Half-Life/valve_WON/SAVE/quick.sav", b"save"),
            ("Half-Life/valve_WON/SAVE/hook.asi", &mz),
            ("Half-Life/valve_WON/SAVE/run.bat", b"start calc"),
            ("Half-Life/valve_WON/sprites/hud.spr", b"sprite"),
        ],
    );
    let (_, file) = export_pack(&source, "hl1.hlsrpack").await;

    let target = setup();
    let result = import_pack(&target, &file).await.unwrap();
    assert_eq!(result["status"], "ok");
    for relative in [
        "Half-Life/opengl32.dll",
        "Half-Life/valve_WON/SAVE/hook.asi",
        "Half-Life/valve_WON/SAVE/run.bat",
    ] {
        assert!(!exists(&target.install_path, relative), "{relative}");
    }
    assert_eq!(
        read(&target.install_path, "Half-Life/valve_WON/SAVE/quick.sav"),
        "save"
    );
    assert_eq!(
        read(&target.install_path, "Half-Life/valve_WON/sprites/hud.spr"),
        "sprite"
    );
}

#[test]
fn export_skips_launcher_copies_in_hlsr_but_takes_the_rest_of_hlsr() {
    let (_temp, root) = temp_root();
    build_files(&root);
    for relative in [
        ".hlsr/backup/20261106-120000/import.json",
        ".hlsr/backup/migration-20261106-123456/Half-Life/valve_WON/userconfig.cfg",
        ".hlsr/cfg-import-backup/userconfig.cfg",
        ".hlsr/imported-modified/Half-Life/valve_WON/pak0.pak",
        ".hlsr/config-setup.json",
        ".hlsr/packs/state.json",
    ] {
        write(&root, relative, "x");
    }
    let files = game_pack::collect(&root).unwrap().files;
    let hlsr: Vec<&String> = files.iter().filter(|f| f.starts_with(".hlsr/")).collect();
    assert_eq!(hlsr, [".hlsr/config-setup.json", ".hlsr/packs/state.json"]);
    assert!(files.iter().any(|f| f == "Half-Life/valve_WON/config.cfg"));
}

// Подробно (зеркало, custom/imported, .dll) — game_pack_modified.rs.
#[tokio::test]
async fn tampered_build_file_export_asks_import_without_originals_is_cancelled_build_untouched() {
    let source = setup();
    install(&source, &[]);
    write(
        &source.install_path,
        "Half-Life/valve_WON/pak0.pak",
        "tampered",
    );
    let file = source.dir.join("hl1.hlsrpack");
    let extra = |allow_modified| PackExtra {
        launcher_id: Some(json!("70")),
        allow_modified,
        ..PackExtra::default()
    };
    let error = source
        .service
        .export_pack(GAME, &file, extra(false))
        .await
        .unwrap_err();
    let Failure::Delivery(error) = error else {
        panic!("{error:?}")
    };
    assert_eq!(error.code, Code::PackModified);
    assert_eq!(error.details.unwrap()["modified"], 1);
    source
        .service
        .export_pack(GAME, &file, extra(true))
        .await
        .unwrap();

    let target = setup();
    install(&target, &[("Half-Life/valve_WON/SAVE/mine.sav", b"keep")]);
    let error = import_pack(&target, &file).await.unwrap_err();
    assert_eq!(code(&error), "PACK_ORIGINALS_UNAVAILABLE");
    assert_eq!(
        read(&target.install_path, "Half-Life/valve_WON/SAVE/mine.sav"),
        "keep"
    );
    let installed = target
        .service
        .install_service()
        .read_installed(GAME)
        .unwrap()
        .unwrap();
    assert_eq!(installed["version"], "1.0.0");
    assert!(staging_is_empty(&target));
}

#[tokio::test]
async fn import_over_installed_build_replaces_it_entirely() {
    let source = setup();
    install(
        &source,
        &[("Half-Life/valve_WON/SAVE/from-pack.sav", b"pack")],
    );
    let (_, file) = export_pack(&source, "hl1.hlsrpack").await;

    let target = setup();
    install(&target, &[("Half-Life/valve_WON/SAVE/old.sav", b"old")]);
    let meta = target.service.read_pack_meta(&file).await.unwrap();
    assert_eq!(meta.installed_version, Some(Some("1.0.0".to_owned())));
    target.service.import_pack(&file, &meta).await.unwrap();
    assert_eq!(
        read(
            &target.install_path,
            "Half-Life/valve_WON/SAVE/from-pack.sav"
        ),
        "pack"
    );
    assert!(!exists(
        &target.install_path,
        "Half-Life/valve_WON/SAVE/old.sav"
    ));
}

#[tokio::test]
async fn cancelled_export_leaves_no_partial_archive() {
    let env = setup();
    // Несжимаемые 24 МБ: упаковка идёт заметное время.
    let mut big = vec![0u8; 24 * 1024 * 1024];
    let mut state = 0x2545_f491_u32;
    for byte in &mut big {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *byte = state as u8;
    }
    install(&env, &[("Half-Life/valve_WON/big.bin", &big)]);
    let file = env.dir.join("cancel.hlsrpack");
    let seen: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    {
        let seen = seen.clone();
        env.service.on_status(move |status| {
            if status["gameId"] == GAME {
                seen.lock().unwrap().push((
                    status["state"].as_str().unwrap_or_default().to_owned(),
                    status["totalBytes"].clone(),
                ));
            }
        });
    }
    let export = env.service.export_pack(
        GAME,
        &file,
        PackExtra {
            launcher_id: Some(json!("70")),
            ..PackExtra::default()
        },
    );
    let cancel = async {
        for _ in 0..500 {
            if env.service.status_of(GAME)["state"] == "exporting" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        env.service.cancel(Some(GAME)).unwrap()
    };
    let (exported, cancelled) = tokio::join!(export, cancel);
    assert!(cancelled);
    assert_eq!(code(&exported.unwrap_err()), "CANCELLED");
    assert!(!file.exists());
    assert!(!PathBuf::from(format!("{}.hlsr-partial", file.display())).exists());
    assert_eq!(env.service.status_of(GAME)["state"], "ready");
    // Прогресс — в байтах папки игры; после отмены карточка не пишет «Установка завершена»:
    // между экспортом и ready стоит cancelled.
    let seen = seen.lock().unwrap();
    let exporting: Vec<&Value> = seen
        .iter()
        .filter(|(state, _)| state == "exporting")
        .map(|(_, total)| total)
        .collect();
    assert!(!exporting.is_empty());
    assert!(
        exporting
            .iter()
            .all(|total| total.as_u64().unwrap() > 24 * 1024 * 1024)
    );
    let mut states: Vec<&str> = Vec::new();
    for (state, _) in seen.iter() {
        if states.last() != Some(&state.as_str()) {
            states.push(state);
        }
    }
    assert_eq!(
        states[states.len() - 3..],
        ["exporting", "cancelled", "ready"]
    );
}

#[tokio::test]
async fn foreign_archive_and_broken_meta_are_rejected() {
    let env = setup();
    let plain = env.dir.join("plain.7z");
    write(&env.dir, "loose/readme.txt", "hi");
    ArchiveService::new(real_7za(), Arc::new(MemoryLogger::default()))
        .create(
            &plain,
            CreateOptions {
                cwd: Some(env.dir.join("loose")),
                files: vec!["readme.txt".to_owned()],
                threads: Some(1.0),
                ..CreateOptions::default()
            },
        )
        .await
        .unwrap();
    let error = env.service.read_pack_meta(&plain).await.unwrap_err();
    assert_eq!(code(&error), "PACK_BAD_FORMAT");

    let base =
        json!({"format": "hlsr-pack", "version": 1, "game": {"gameId": GAME, "version": "1.0.0"}});
    let is_known = |id: &str| manifests::is_known_game(id);
    let is_depot = |id: &str| manifests::is_depot_build(id);
    let options = ParseOptions {
        is_known_game: Some(&is_known),
        is_depot_build: Some(&is_depot),
    };
    let parse = |raw: &Value| game_pack::parse_meta(&raw.to_string(), &options);
    let with = |key: &str, value: Value| {
        let mut out = base.clone();
        out[key] = value;
        out
    };
    assert_eq!(parse(&base).unwrap().game_id, GAME);
    assert_eq!(
        parse(&with("version", json!(2))).unwrap_err().code,
        "NEWER_FORMAT"
    );
    assert_eq!(
        parse(&with("game", json!({"gameId": "nope", "version": "1.0.0"})))
            .unwrap_err()
            .code,
        "UNKNOWN_GAME"
    );
    assert_eq!(
        parse(&with(
            "game",
            json!({"gameId": "hl2-5288", "version": "1.0.0"})
        ))
        .unwrap_err()
        .code,
        "UNKNOWN_GAME"
    );
    assert_eq!(
        parse(&with("game", json!({"gameId": GAME, "version": "../x"})))
            .unwrap_err()
            .code,
        "BAD_FORMAT"
    );
    let unchecked = ParseOptions {
        is_known_game: None,
        is_depot_build: None,
    };
    assert_eq!(
        game_pack::parse_meta("{", &unchecked).unwrap_err().code,
        "BAD_FORMAT"
    );
    let bad_sha = with(
        "game",
        json!({"gameId": GAME, "version": "1.0.0", "manifestSha256": "zz"}),
    );
    assert_eq!(parse(&bad_sha).unwrap().manifest_sha256, None);
}

#[tokio::test]
async fn steam_build_cannot_be_exported_whole() {
    let env = setup();
    let error = env
        .service
        .export_pack(
            "hl2-5288",
            &env.dir.join("x.hlsrpack"),
            PackExtra::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), Code::PackDepotBuild.as_str());
}

#[test]
fn executables_are_recognised_by_header_not_extension() {
    let (_temp, dir) = temp_root();
    let cases: [(&str, Vec<u8>); 4] = [
        ("a.flt", mz()),
        ("b.dat", vec![0x7f, 0x45, 0x4c, 0x46, 1]),
        ("c.cmd", b"x".to_vec()),
        ("d.sav", b"MZ".to_vec()),
    ];
    for (name, data) in &cases {
        write(&dir, name, data);
    }
    write(&dir, "e.txt", "plain");
    write(&dir, "f.sav", "Z");
    for (name, _) in &cases {
        assert!(game_pack::is_executable(&dir.join(name)), "{name}");
    }
    assert!(!game_pack::is_executable(&dir.join("e.txt")));
    assert!(!game_pack::is_executable(&dir.join("f.sav")));
}
