//! Самотесты уровня сервиса доставки: `scripts/hl1-selftest.js` 0.4.9 (другая сборка Half-Life на
//! bxt.rs без закреплённого SHA-256 не предлагается и не ставится) и `scripts/cancellation-selftest.js`
//! (отмена приостановленной игры не трогает идущую загрузку другой). На настоящем 7za из
//! `resources/7zip` и локальном HTTP-сервере.

mod support;

use hlsr_delivery::archive_service::ArchiveService;
use hlsr_delivery::environment::Environment;
use hlsr_delivery::errors::Failure;
use hlsr_delivery::files_manifest::{self, BuildOptions};
use hlsr_delivery::game_delivery_service::{
    GameDeliveryService, Provided, Registry, ServiceOptions,
};
use hlsr_delivery::manifests;
use hlsr_delivery::origin_updates::{Baseline, Origin, OriginOptions, OriginUpdates};
use hlsr_delivery::paths::LibraryPaths;
use hlsr_delivery::ports::{DepotProgressFn, FakeFreeSpace, Logger, MemoryLogger, SteamDepot};
use hlsr_delivery::providers::BoxFuture;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use support::http::{RequestLog, Server, start_server};
use support::scenario::temp_root;

fn real_7za() -> PathBuf {
    hlsr_core::paths::Resources::detect().sevenzip()
}

fn write(root: &Path, relative: &str, data: impl AsRef<[u8]>) {
    let full = root.join(relative);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, data).unwrap();
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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

fn bundled(game_id: &str) -> Value {
    manifests::manifests()
        .into_iter()
        .find(|(id, _)| *id == game_id)
        .unwrap()
        .1
        .clone()
}

// ---------------------------------------------------------------- hl1-selftest.js

/// Сборка WON с меткой `marker` в `build.txt`, упакованная как на bxt.rs.
fn pack_build(dir: &Path, archive: &Path, marker: &str) -> PathBuf {
    let root = dir.join("Half-Life 2005 WON");
    let _ = std::fs::remove_dir_all(&root);
    write(&root, "Half-Life/hl.exe", "MZ");
    write(&root, "Half-Life/steam_appid.txt", "70");
    write(
        &root,
        "Half-Life/valve_WON/liblist.gam",
        "game \"Half-Life\"\n",
    );
    write(&root, "Bunnymod XT/Injector.exe", "MZ");
    write(&root, "Bunnymod XT/BunnymodXT.dll", "MZ");
    write(&root, "Half-Life/valve_WON/build.txt", marker);
    let _ = std::fs::remove_file(archive);
    let status = std::process::Command::new(real_7za())
        .args(["a", "-bd", "-y"])
        .arg(archive)
        .arg("Half-Life 2005 WON")
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    root
}

/// Источник отдаёт текущий архив с его `Last-Modified` и `ETag`.
fn serve_archive(server: &Server, archive: &Path, last_modified: &str, etag: &str) {
    let body = std::fs::read(archive).unwrap();
    let mut routes = Map::new();
    routes.insert(
        "/won.7z".to_owned(),
        json!([{
            "headers": [["last-modified", last_modified], ["etag", etag]],
            "bodyHex": hex(&body),
        }]),
    );
    *server.routes.lock().unwrap() = routes;
}

#[tokio::test]
async fn hl1_another_build_on_bxt_rs_without_a_pinned_sha256_is_not_offered_or_installed() {
    let (_temp, dir) = temp_root();
    let archive = dir.join("build.7z");
    let build_root = pack_build(&dir, &archive, "v1");
    let mut options = BuildOptions::new(&build_root);
    options.game_id = Some("hl1-won");
    options.version = Some("1.0.0");
    let v1_files = files_manifest::build(options).unwrap().manifest;
    let v1_bytes = std::fs::read(&archive).unwrap();
    let v1_size = v1_bytes.len() as u64;
    let v1_sha = support::sha256_hex(&v1_bytes);

    let log: RequestLog = Arc::default();
    let server = start_server("main", log).await;
    serve_archive(&server, &archive, "Mon, 17 Jul 2023 11:23:57 GMT", "\"v1\"");

    let mut manifest = bundled("hl1-won");
    manifest["expectedInstalledSize"] = v1_files["totalSize"].clone();
    manifest["baseArchive"]["size"] = json!(v1_size);
    manifest["baseArchive"]["sha256"] = json!(v1_sha);
    manifest["baseArchive"]["providers"] = json!([{
        "id": "bxt-rs",
        "type": "local-http",
        "status": "permanent",
        "priority": 10,
        "url": format!("http://{}/won.7z", server.authority),
    }]);
    let origins = vec![(
        "hl1-won".to_owned(),
        Origin {
            provider_id: "bxt-rs".to_owned(),
            baseline: Baseline {
                version: "1.0.0".to_owned(),
                last_modified: "Mon, 17 Jul 2023 11:23:57 GMT".to_owned(),
                etag: "\"v1\"".to_owned(),
                size: v1_size,
            },
        },
    )];

    let library = dir.join("library");
    let logger = Arc::new(MemoryLogger::default());
    let service = GameDeliveryService::new(ServiceOptions {
        library_path: library.to_string_lossy().into_owned(),
        launcher_version: Some("0.3.16".to_owned()),
        env: Some(Environment::new(
            [("GAME_DELIVERY_ALLOW_LOCAL_HTTP", "1")],
            false,
        )),
        registry: Some(Registry {
            manifests: vec![("hl1-won".to_owned(), manifest.clone())],
            files: vec![("hl1-won".to_owned(), vec![("1.0.0".to_owned(), v1_files)])],
        }),
        channel_service: Provided::Off,
        origin_updates: Provided::Given(Arc::new(OriginUpdates::new(OriginOptions {
            logger: Some(logger.clone()),
            http: None,
            origins: Some(origins),
        }))),
        depot: Some(Arc::new(Owns)),
        update_check_interval_ms: Some(0),
        archive_service: Some(ArchiveService::new(real_7za(), logger.clone())),
        logger: Some(logger.clone()),
        ..ServiceOptions::default()
    })
    .unwrap();

    assert_eq!(
        service.install("hl1-won").await.unwrap()["status"],
        "installed"
    );
    let game = PathBuf::from(
        service.installed_of("hl1-won").unwrap()["installPath"]
            .as_str()
            .unwrap(),
    );
    let build = game.join("Half-Life/valve_WON/build.txt");

    let check = service
        .check_for_updates("hl1-won", true, None)
        .await
        .unwrap();
    assert_eq!(
        check["update"]["updateAvailable"], false,
        "та же сборка — обновлять нечего"
    );

    // На bxt.rs лежит другая сборка: её SHA-256 нигде не закреплён.
    pack_build(&dir, &archive, "v2");
    serve_archive(&server, &archive, "Sun, 20 Sep 2026 00:05:09 GMT", "\"v2\"");
    logger.take();

    let check = service
        .check_for_updates("hl1-won", true, None)
        .await
        .unwrap();
    assert_eq!(
        check["update"]["updateAvailable"], false,
        "незакреплённая сборка не предлагается"
    );
    assert_eq!(
        service.apply_update("hl1-won").await.unwrap()["status"],
        "up-to-date"
    );
    assert_eq!(service.installed_of("hl1-won").unwrap()["version"], "1.0.0");
    assert_eq!(std::fs::read_to_string(&build).unwrap(), "v1");
    let warned = logger
        .take()
        .into_iter()
        .filter(|(_, message)| message.contains("другая сборка"))
        .count();
    assert_eq!(warned, 1);
    assert!(
        !service.paths().state.join("hl1-won.origin.json").exists(),
        "записей источника 0.4.9 не ведёт"
    );
}
// ---------------------------------------------------------------- cancellation-selftest.js

/// Архив сборки из фикстур сервиса доставки.
fn ghosting_archive() -> Vec<u8> {
    std::fs::read(support::fixture_path(
        "archives/game-delivery/ghosting-1.0.0.7z",
    ))
    .unwrap()
}

fn manifest_for(game_id: &str, file: &str, archive: &[u8], authority: &str) -> Value {
    json!({
        "schemaVersion": 1,
        "gameId": game_id,
        "channel": "stable",
        "version": "1.0.0",
        "publishedAt": "2026-10-01T10:00:00.000Z",
        "minimumLauncherVersion": "0.3.0",
        "expectedInstalledSize": 1000,
        "preservePaths": [],
        "baseArchive": {
            "filename": file,
            "size": archive.len(),
            "sha256": support::sha256_hex(archive),
            "rootPrefix": "Ghosting Renewed",
            "providers": [{
                "id": "local",
                "type": "local-http",
                "status": "permanent",
                "priority": 10,
                "url": format!("http://{authority}/{file}"),
            }],
        },
        "filesManifest": {"requiredFiles": ["hl2.exe"], "maxEntries": 100, "maxUncompressedSize": 10_000_000},
        "patches": [],
        "enabled": true,
    })
}

async fn wait_for_size(file: &Path, bytes: u64) {
    for _ in 0..2000 {
        if std::fs::metadata(file).is_ok_and(|meta| meta.len() >= bytes) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("{} не дорос до {bytes} байт", file.display());
}

#[tokio::test]
async fn cancelling_a_paused_game_does_not_interrupt_another_active_download() {
    let (_temp, dir) = temp_root();
    let archive = ghosting_archive();
    let log: RequestLog = Arc::default();
    let server = start_server("main", log).await;
    let gated =
        |gate: &str| json!([{"bodyHex": hex(&archive), "gates": [{"at": 300, "name": gate}]}]);
    *server.routes.lock().unwrap() = [
        ("/paused.7z".to_owned(), gated("paused")),
        ("/active.7z".to_owned(), gated("active")),
    ]
    .into_iter()
    .collect();

    let paused_game = "ghosting-renewed";
    let active_game = "hl2-2707";
    let logger: Arc<dyn Logger> = Arc::new(MemoryLogger::default());
    let service = GameDeliveryService::new(ServiceOptions {
        library_path: dir.join("lib").to_string_lossy().into_owned(),
        launcher_version: Some("0.4.7".to_owned()),
        env: Some(Environment::new(
            [("GAME_DELIVERY_ALLOW_LOCAL_HTTP", "1")],
            false,
        )),
        registry: Some(Registry {
            manifests: vec![
                (
                    paused_game.to_owned(),
                    manifest_for(paused_game, "paused.7z", &archive, &server.authority),
                ),
                (
                    active_game.to_owned(),
                    manifest_for(active_game, "active.7z", &archive, &server.authority),
                ),
            ],
            files: Vec::new(),
        }),
        channel_service: Provided::Off,
        origin_updates: Provided::Off,
        depot: Some(Arc::new(Owns)),
        free_space: Some(Arc::new(FakeFreeSpace::default())),
        max_attempts: Some(1),
        archive_service: Some(ArchiveService::new(real_7za(), logger.clone())),
        logger: Some(logger),
        ..ServiceOptions::default()
    })
    .unwrap();
    let downloads = dir.join("lib").join("Downloads");

    // Первая игра — на паузе.
    let part = downloads.join(paused_game).join("paused.7z.part");
    let pause = async {
        wait_for_size(&part, 300).await;
        service.pause()
    };
    let (installed, paused) = tokio::join!(service.install(paused_game), pause);
    assert!(paused);
    assert_eq!(installed.unwrap()["status"], "paused");
    assert_eq!(service.status_of(paused_game)["state"], "paused");

    // Вторая качается; отмена первой не трогает её загрузку.
    let active_part = downloads.join(active_game).join("active.7z.part");
    let cancel = async {
        wait_for_size(&active_part, 300).await;
        let cancelled = service.cancel(Some(paused_game)).unwrap();
        let other = service.cancel(Some("hl2-4260")).unwrap();
        let busy = service.is_busy();
        server.release("active");
        (cancelled, other, busy)
    };
    let (installed, (cancelled, other, busy)) = tokio::join!(service.install(active_game), cancel);
    assert!(cancelled, "приостановленная игра отменяется");
    assert!(!other, "чужая игра без загрузки — нечего отменять");
    assert!(busy, "загрузка второй игры идёт");
    assert_eq!(installed.unwrap()["status"], "installed");
    assert_eq!(service.status_of(paused_game)["state"], "not-installed");
    assert!(!part.exists());
    assert_eq!(service.status_of(active_game)["state"], "ready");
}
