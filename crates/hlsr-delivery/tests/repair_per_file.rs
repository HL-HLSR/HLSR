//! Самотест 0.4.9 `scripts/repair-per-file-selftest.js`: «Проверить файлы» и «Восстановить игру»
//! по одному файлу. Проверка находит пропавшее и изменённое, ремонт берёт только отмеченные файлы и
//! только с пофайлового зеркала (раскладка `sha256-2`), файлы игрока не трогает никогда.
//!
//! Весь путь через `GameDeliveryService`: подписанный канал версии с `repairMirror` (QA-адрес на
//! 127.0.0.1) и зеркало на втором локальном сервере. Не перенесены случаи, которым нужна подмена
//! `fs` Node (EPERM/EBUSY при переименовании `.part`, ENOSPC при записи), и проверки скриптов
//! выпуска (`build-repair-mirror.js`, `build-files-manifest.js`, `release-common`) — это Node.

mod support;

use hlsr_delivery::channel_service::{ChannelOptions, ChannelService};
use hlsr_delivery::environment::Environment;
use hlsr_delivery::errors::{Code, DeliveryError, Failure};
use hlsr_delivery::files_manifest::{self, BuildOptions};
use hlsr_delivery::game_delivery_service::{
    GameDeliveryService, Provided, Registry, RepairCall, ServiceOptions,
};
use hlsr_delivery::install_service::{InstallTarget, IntegrityRequest};
use hlsr_delivery::integrity_service::{IntegrityOptions, IntegrityService};
use hlsr_delivery::manifest_signing::{attach, generate_key_pair};
use hlsr_delivery::manifests;
use hlsr_delivery::mutable_paths::matches_mutable;
use hlsr_delivery::paths::{LibraryPaths, game_install_path};
use hlsr_delivery::ports::{DepotProgressFn, MemoryLogger, SteamDepot};
use hlsr_delivery::providers::BoxFuture;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use support::http::{RequestLog, Server, start_server};
use support::library::copy_tree;
use support::scenario::{path_in, temp_root};
use support::zip::hex;

const GAME: &str = "ghosting-renewed";
const VERSION: &str = "1.0.12";
const MIRROR_PATH: &str = "/ghosting-renewed/repair";

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

/// Псевдослучайные байты (одни и те же от запуска к запуску).
fn bytes(seed: u32, length: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 24) as u8
        })
        .collect()
}

/// Сборка, как в настоящей 1.0.12: стоковые cfg в папке игрока, `config.cfg` пишет игра,
/// `autoexec.cfg` и `hlsr-*.cfg` — лаунчер (они не стоковые).
fn build() -> Vec<(&'static str, Vec<u8>)> {
    let mut exe = b"MZ".to_vec();
    exe.extend(bytes(1, 4000));
    vec![
        ("hl2.exe", exe),
        ("bin/engine.dll", bytes(2, 5000)),
        ("bin/dxsupport.cfg", b"stock dxsupport\n".to_vec()),
        ("bin/dxsupport_sp.cfg", b"stock sp\n".to_vec()),
        ("ghostingmod/bin/client.dll", bytes(3, 7000)),
        ("ghostingmod/maps/d1_canals_01.bsp", bytes(4, 9000)),
        ("ghostingmod/save/start.sav", b"start save".to_vec()),
        (
            "ghostingmod/cfg/chapter3.cfg",
            b"map d1_canals_01\n".to_vec(),
        ),
        (
            "ghostingmod/cfg/skill_manifest.cfg",
            b"exec skill.cfg\n".to_vec(),
        ),
        ("ghostingmod/cfg/config.cfg", b"bind w +forward\n".to_vec()),
        (
            "ghostingmod/cfg/autoexec.cfg",
            b"exec hlsr-personalization.cfg\n".to_vec(),
        ),
        (
            "ghostingmod/cfg/hlsr-personalization.cfg",
            b"gh_hud 1\n".to_vec(),
        ),
    ]
}

fn of(rel: &str) -> Vec<u8> {
    build()
        .into_iter()
        .find(|(path, _)| *path == rel)
        .unwrap()
        .1
}

fn write(root: &Path, rel: &str, data: &[u8]) {
    let file = path_in(root, rel);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, data).unwrap();
}

fn read(root: &Path, rel: &str) -> Vec<u8> {
    std::fs::read(path_in(root, rel)).unwrap()
}

fn exists(root: &Path, rel: &str) -> bool {
    path_in(root, rel).exists()
}

fn remove(root: &Path, rel: &str) {
    std::fs::remove_file(path_in(root, rel)).unwrap();
}

#[derive(Default)]
struct Options {
    extra_build: Vec<(&'static str, Vec<u8>)>,
    no_files_manifest: bool,
    no_mirror: bool,
    /// Версия зеркала (и канала, если она новее установленной); `None` — [`VERSION`].
    mirror_version: Option<&'static str>,
    /// Пути, ответ на которые приходит наполовину и виснет.
    stall: Vec<&'static str>,
}

struct Bench {
    _temp: tempfile::TempDir,
    dir: PathBuf,
    install_path: PathBuf,
    service: Arc<GameDeliveryService>,
    files_manifest: Value,
    mirror_log: RequestLog,
    _servers: (Server, Server),
}

async fn setup(options: Options) -> Bench {
    let (temp, dir) = temp_root();
    let build_root = dir.join("build");
    let mut files = build();
    files.extend(options.extra_build.iter().cloned());
    for (rel, data) in &files {
        write(&build_root, rel, data);
    }

    // Как в настоящем манифесте 1.0.12: стартовый сейв сборки описан, хотя папка — игрока.
    let is_mutable = |rel: &str| {
        let lower = rel.to_ascii_lowercase();
        matches_mutable(rel)
            && lower != "ghostingmod/save"
            && !lower.starts_with("ghostingmod/save/")
    };
    let mut build_options = BuildOptions::new(&build_root);
    build_options.game_id = Some(GAME);
    build_options.version = Some(VERSION);
    build_options.is_mutable = Some(&is_mutable);
    let files_manifest = files_manifest::build(build_options).unwrap().manifest;

    let mirror_log = RequestLog::default();
    let mirror = start_server("mirror", mirror_log.clone()).await;
    let mut routes = Map::new();
    for key in ["files", "stockConfigs"] {
        for entry in files_manifest[key].as_array().into_iter().flatten() {
            let rel = entry["path"].as_str().unwrap();
            let sha = entry["sha256"].as_str().unwrap();
            let data = read(&build_root, rel);
            let mut answer = json!({"bodyHex": hex(&data)});
            if options.stall.contains(&rel) {
                answer["gates"] = json!([{"at": data.len() / 2, "name": "never"}]);
            }
            routes.insert(
                format!("{MIRROR_PATH}/{}/{sha}", &sha[..2]),
                json!([answer]),
            );
        }
    }
    *mirror.routes.lock().unwrap() = routes;

    // Подписанный канал версии объявляет зеркало (то, что кладёт game-release-prepare.js).
    let channel_server = start_server("channel", RequestLog::default()).await;
    let keys = generate_key_pair();
    let mirror_version = options.mirror_version.unwrap_or(VERSION);
    let provider = |url: String| json!({"id": "local", "type": "local-http", "status": "permanent", "priority": 10, "url": url});
    let mut channel = json!({
        "schemaVersion": 1,
        "gameId": GAME,
        "channel": "stable",
        "version": mirror_version,
        "publishedAt": "2026-11-01T00:00:00.000Z",
        "minimumLauncherVersion": "0.3.9",
        "enabled": true,
        "updateRequired": false,
        "fullArchive": {"filename": format!("{GAME}-{mirror_version}.7z"), "size": 1, "sha256": "a".repeat(64),
                        "rootPrefix": null, "providers": [provider(format!("http://{}/g.7z", channel_server.authority))]},
        "filesManifest": {"version": mirror_version, "filename": "f.json", "size": 1, "sha256": "b".repeat(64),
                          "providers": [provider(format!("http://{}/f.json", channel_server.authority))]},
        "patches": [],
    });
    if !options.no_mirror {
        channel["repairMirror"] = json!({
            "version": mirror_version,
            "layout": "sha256-2",
            "maxFileSize": 256 * 1024 * 1024,
            "providers": [provider(format!("http://{}{MIRROR_PATH}", mirror.authority))],
        });
    }
    let signed =
        Value::Object(attach(channel.as_object().unwrap(), &keys.private_key_pem).unwrap());
    *channel_server.routes.lock().unwrap() = json!({"/c.json": [{"body": signed.to_string()}]})
        .as_object()
        .unwrap()
        .clone();

    let env = Environment::new(
        [
            ("GAME_DELIVERY_ALLOW_LOCAL_HTTP".to_owned(), "1".to_owned()),
            (
                "HLSR_GAME_DELIVERY_CHANNEL_URL".to_owned(),
                format!("http://{}/c.json", channel_server.authority),
            ),
            (
                "HLSR_GAME_DELIVERY_CHANNEL_KEY".to_owned(),
                keys.public_key_pem.replace('\n', "\\n"),
            ),
        ],
        false,
    );
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
    let library = dir.join("library");
    let service = GameDeliveryService::new(ServiceOptions {
        library_path: library.to_string_lossy().into_owned(),
        launcher_version: Some("0.4.9".to_owned()),
        env: Some(env.clone()),
        registry: Some(Registry {
            manifests: vec![(GAME.to_owned(), manifest)],
            files: if options.no_files_manifest {
                Vec::new()
            } else {
                vec![(
                    GAME.to_owned(),
                    vec![(VERSION.to_owned(), files_manifest.clone())],
                )]
            },
        }),
        channel_service: Provided::Given(Arc::new(ChannelService::new(ChannelOptions {
            env: Some(env),
            max_attempts: Some(1),
            ..ChannelOptions::default()
        }))),
        origin_updates: Provided::Off,
        depot: Some(Arc::new(Owns)),
        update_check_interval_ms: Some(0),
        logger: Some(Arc::new(MemoryLogger::default())),
        ..ServiceOptions::default()
    })
    .unwrap();

    let install_path = game_install_path(service.paths(), GAME).unwrap();
    copy_tree(&build_root, &install_path);
    service
        .install_service()
        .write_installed(
            &InstallTarget {
                game_id: GAME.to_owned(),
                version: json!(VERSION),
                install_path: install_path.clone(),
                staging_path: install_path.clone(),
                published_at: Some("2026-11-01T00:00:00.000Z".to_owned()),
                manifest_sha256: None,
            },
            json!("a".repeat(64)),
            None,
        )
        .unwrap();

    Bench {
        _temp: temp,
        dir,
        install_path,
        service,
        files_manifest,
        mirror_log,
        _servers: (mirror, channel_server),
    }
}

impl Bench {
    fn sha_of(&self, rel: &str) -> String {
        ["files", "stockConfigs"]
            .iter()
            .flat_map(|key| {
                self.files_manifest[*key]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .find(|entry| entry["path"] == rel)
            .unwrap()["sha256"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn url_of(&self, rel: &str) -> String {
        let sha = self.sha_of(rel);
        format!("{MIRROR_PATH}/{}/{sha}", &sha[..2])
    }

    fn requests(&self) -> Vec<String> {
        self.mirror_log
            .lock()
            .unwrap()
            .iter()
            .map(|entry| entry["url"].as_str().unwrap().to_owned())
            .collect()
    }

    fn staging_leftovers(&self) -> Vec<String> {
        let Ok(dir) = std::fs::read_dir(&self.service.paths().staging) else {
            return Vec::new();
        };
        dir.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("repair-mirror-"))
            .collect()
    }

    fn state(&self) -> Value {
        self.service.status_of(GAME)["state"].clone()
    }

    async fn repair(&self, call: RepairCall) -> Result<Value, DeliveryError> {
        self.service.repair(GAME, call).await.map_err(delivery)
    }
}

fn delivery(failure: Failure) -> DeliveryError {
    match failure {
        Failure::Delivery(error) => error,
        Failure::Raw(raw) => panic!("{raw:?}"),
    }
}

fn ask() -> RepairCall {
    RepairCall::default()
}

fn confirmed(files: Option<&[&str]>) -> RepairCall {
    RepairCall {
        confirmed: true,
        skip_recheck: true,
        files: files.map(|files| files.iter().map(|s| (*s).to_owned()).collect()),
    }
}

fn kinds(check: &Value) -> Vec<(String, String)> {
    check["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["path"].as_str().unwrap().to_owned(),
                file["kind"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn pair(path: &str, kind: &str) -> (String, String) {
    (path.to_owned(), kind.to_owned())
}

#[tokio::test]
async fn one_deleted_build_file_is_found_and_only_it_is_fetched() {
    let b = setup(Options::default()).await;
    remove(&b.install_path, "bin/engine.dll");

    let verify = b.service.verify_files(GAME).await.unwrap();
    assert_eq!(verify.missing, ["bin/engine.dll"]);
    assert_eq!(verify.damaged, 1);

    let check = b.repair(ask()).await.unwrap();
    assert_eq!(check["status"], "confirmation-required");
    assert_eq!(
        check["files"],
        json!([{"path": "bin/engine.dll", "size": 5000, "kind": "missing"}])
    );
    assert!(
        b.requests().is_empty(),
        "до подтверждения ничего не качается"
    );

    let done = b
        .repair(confirmed(Some(&["bin/engine.dll"])))
        .await
        .unwrap();
    assert_eq!(done["status"], "repaired");
    assert_eq!(done["repaired"], json!(["bin/engine.dll"]));
    assert_eq!(done["remaining"], json!([]));
    assert_eq!(done["integrity"]["ok"], true);
    assert_eq!(b.requests(), [b.url_of("bin/engine.dll")]);
    assert_eq!(
        read(&b.install_path, "bin/engine.dll"),
        of("bin/engine.dll")
    );
    assert!(b.staging_leftovers().is_empty());
    assert_eq!(b.state(), "repair-complete");
}

#[tokio::test]
async fn a_deleted_stock_cfg_is_its_own_kind_and_is_restored() {
    let b = setup(Options::default()).await;
    remove(&b.install_path, "bin/dxsupport.cfg");

    let verify = b.service.verify_files(GAME).await.unwrap();
    assert_eq!(verify.config_missing, ["bin/dxsupport.cfg"]);
    assert!(verify.missing.is_empty());
    assert_eq!(
        GameDeliveryService::summarize_integrity(&verify)["configMissing"],
        1
    );

    let check = b.repair(ask()).await.unwrap();
    assert_eq!(kinds(&check), [pair("bin/dxsupport.cfg", "cfg-missing")]);

    let done = b
        .repair(confirmed(Some(&["bin/dxsupport.cfg"])))
        .await
        .unwrap();
    assert_eq!(done["repaired"], json!(["bin/dxsupport.cfg"]));
    assert_eq!(
        read(&b.install_path, "bin/dxsupport.cfg"),
        of("bin/dxsupport.cfg")
    );
}

#[tokio::test]
async fn a_player_edited_cfg_is_kept_without_a_tick_and_replaced_with_one() {
    let b = setup(Options::default()).await;
    write(
        &b.install_path,
        "bin/dxsupport_sp.cfg",
        "мой cfg\n".as_bytes(),
    );
    remove(&b.install_path, "bin/engine.dll");

    let check = b.repair(ask()).await.unwrap();
    assert_eq!(
        kinds(&check),
        [
            pair("bin/engine.dll", "missing"),
            pair("bin/dxsupport_sp.cfg", "cfg-changed")
        ]
    );

    // Без списка (старый вызов) — только повреждённое, изменённый cfg не трогается.
    let first = b.repair(confirmed(None)).await.unwrap();
    assert_eq!(first["repaired"], json!(["bin/engine.dll"]));
    assert_eq!(
        read(&b.install_path, "bin/dxsupport_sp.cfg"),
        "мой cfg\n".as_bytes()
    );
    assert_eq!(
        first["remaining"],
        json!([]),
        "изменённый cfg — не повреждение"
    );

    let again = b.repair(ask()).await.unwrap();
    assert_eq!(kinds(&again), [pair("bin/dxsupport_sp.cfg", "cfg-changed")]);
    let second = b
        .repair(confirmed(Some(&["bin/dxsupport_sp.cfg"])))
        .await
        .unwrap();
    assert_eq!(second["repaired"], json!(["bin/dxsupport_sp.cfg"]));
    assert_eq!(
        read(&b.install_path, "bin/dxsupport_sp.cfg"),
        of("bin/dxsupport_sp.cfg")
    );
    let rollback = std::fs::read_dir(&b.service.paths().rollback)
        .unwrap()
        .count();
    assert_eq!(rollback, 0, "старый cfg удалён вместе с откатом");
}

#[tokio::test]
async fn saves_demos_and_player_cfg_are_never_checked_or_repaired() {
    let b = setup(Options::default()).await;
    let user = [
        ("ghostingmod/SAVE/quick.sav", "мой сейв"),
        ("ghostingmod/runs/run1.dem", "мой забег"),
        ("ghostingmod/demo1.dem", "моя демка"),
        ("ghostingmod/cfg/autoexec.cfg", "bind q quit\n"),
        ("ghostingmod/screenshots/shot.jpg", "кадр"),
    ];
    for (rel, data) in user {
        write(&b.install_path, rel, data.as_bytes());
    }
    write(
        &b.install_path,
        "ghostingmod/save/start.sav",
        "изменил игрок".as_bytes(),
    );
    remove(&b.install_path, "bin/engine.dll");
    let listed = b.files_manifest["files"].as_array().unwrap();
    assert!(
        listed
            .iter()
            .any(|file| file["path"] == "ghostingmod/save/start.sav"),
        "сейв есть в манифесте сборки"
    );

    let check = b.repair(ask()).await.unwrap();
    assert_eq!(kinds(&check), [pair("bin/engine.dll", "missing")]);

    let done = b
        .repair(confirmed(Some(&[
            "bin/engine.dll",
            "ghostingmod/save/start.sav",
            "ghostingmod/demo1.dem",
            "ghostingmod/cfg/autoexec.cfg",
        ])))
        .await
        .unwrap();
    assert_eq!(done["repaired"], json!(["bin/engine.dll"]));
    assert_eq!(b.requests(), [b.url_of("bin/engine.dll")]);
    assert_eq!(
        read(&b.install_path, "ghostingmod/save/start.sav"),
        "изменил игрок".as_bytes()
    );
    for (rel, data) in user {
        assert_eq!(read(&b.install_path, rel), data.as_bytes(), "{rel}");
    }
}

#[tokio::test]
async fn no_mirror_for_the_version_is_a_clear_error_and_no_archive_is_fetched() {
    for options in [
        Options {
            no_mirror: true,
            ..Options::default()
        },
        Options {
            mirror_version: Some("1.0.11"),
            ..Options::default()
        },
    ] {
        let b = setup(options).await;
        remove(&b.install_path, "bin/engine.dll");

        let error = b.repair(ask()).await.unwrap_err();
        assert_eq!(error.code, Code::RepairSourceMissing);
        assert!(
            error.message.contains("недоступно — переустанови игру"),
            "{}",
            error.message
        );
        let error = b
            .repair(confirmed(Some(&["bin/engine.dll"])))
            .await
            .unwrap_err();
        assert_eq!(error.code, Code::RepairSourceMissing);
        assert!(b.requests().is_empty());
        assert!(!exists(&b.install_path, "bin/engine.dll"));
        assert!(!b.service.is_busy());
        assert!(
            !b.dir.join("library/Downloads").join(GAME).exists(),
            "полный архив не запрашивается"
        );
    }
}

#[tokio::test]
async fn a_file_missing_on_the_mirror_leaves_the_game_untouched() {
    let b = setup(Options::default()).await;
    b._servers
        .0
        .routes
        .lock()
        .unwrap()
        .shift_remove(&b.url_of("bin/engine.dll"));
    remove(&b.install_path, "bin/engine.dll");
    write(
        &b.install_path,
        "ghostingmod/bin/client.dll",
        "битый".as_bytes(),
    );

    let error = b
        .repair(RepairCall {
            confirmed: true,
            skip_recheck: false,
            files: Some(vec![
                "ghostingmod/bin/client.dll".to_owned(),
                "bin/engine.dll".to_owned(),
            ]),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, Code::RepairSourceMissing);
    assert_eq!(error.details.unwrap()["path"], "bin/engine.dll");
    assert_eq!(
        read(&b.install_path, "ghostingmod/bin/client.dll"),
        "битый".as_bytes(),
        "без всех файлов замены нет"
    );
    assert!(b.staging_leftovers().is_empty());
    assert_eq!(b.state(), "error");
}

#[tokio::test]
async fn an_unticked_file_is_not_fetched_and_stays_in_remaining() {
    let b = setup(Options::default()).await;
    remove(&b.install_path, "bin/engine.dll");
    remove(&b.install_path, "ghostingmod/bin/client.dll");

    let check = b.repair(ask()).await.unwrap();
    let mut paths: Vec<String> = kinds(&check).into_iter().map(|(path, _)| path).collect();
    paths.sort();
    assert_eq!(paths, ["bin/engine.dll", "ghostingmod/bin/client.dll"]);

    let done = b
        .repair(confirmed(Some(&["ghostingmod/bin/client.dll"])))
        .await
        .unwrap();
    assert_eq!(done["repaired"], json!(["ghostingmod/bin/client.dll"]));
    assert_eq!(b.requests(), [b.url_of("ghostingmod/bin/client.dll")]);
    assert_eq!(done["remaining"], json!(["bin/engine.dll"]));
    assert!(!exists(&b.install_path, "bin/engine.dll"));
    assert_eq!(b.state(), "files-damaged");

    assert_eq!(
        b.repair(confirmed(Some(&[]))).await.unwrap(),
        json!({"status": "nothing-selected", "repaired": []})
    );
}

#[tokio::test]
async fn a_big_file_streams_with_progress_by_bytes() {
    let big = bytes(5, 3 * 1024 * 1024);
    let b = setup(Options {
        extra_build: vec![("ghostingmod/ghostingmod_pak.vpk", big.clone())],
        ..Options::default()
    })
    .await;
    remove(&b.install_path, "ghostingmod/ghostingmod_pak.vpk");
    let progress: Arc<Mutex<Vec<u64>>> = Arc::default();
    let sink = progress.clone();
    b.service.on_status(move |status: &Value| {
        if status["state"] == "preparing" && status["operation"] == "repair-download" {
            sink.lock()
                .unwrap()
                .push(status["bytesDownloaded"].as_u64().unwrap());
        }
    });

    let done = b
        .repair(RepairCall {
            confirmed: true,
            files: Some(vec!["ghostingmod/ghostingmod_pak.vpk".to_owned()]),
            ..RepairCall::default()
        })
        .await
        .unwrap();
    assert_eq!(done["repaired"], json!(["ghostingmod/ghostingmod_pak.vpk"]));
    assert!(read(&b.install_path, "ghostingmod/ghostingmod_pak.vpk") == big);
    let progress = progress.lock().unwrap().clone();
    let size = big.len() as u64;
    assert!(
        progress.iter().any(|&bytes| bytes > 0 && bytes < size),
        "промежуточный прогресс: {progress:?}"
    );
    assert_eq!(progress.last(), Some(&size));
}

/// Приёмка B: отмена возвращает прежний статус.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_during_download_replaces_nothing_and_restores_the_status() {
    // Второй файл приходит наполовину и виснет: отменяем, пока он «качается».
    let b = setup(Options {
        stall: vec!["ghostingmod/maps/d1_canals_01.bsp"],
        ..Options::default()
    })
    .await;
    remove(&b.install_path, "bin/engine.dll");
    remove(&b.install_path, "ghostingmod/maps/d1_canals_01.bsp");

    let me: Weak<GameDeliveryService> = Arc::downgrade(&b.service);
    let armed = Arc::new(Mutex::new(false));
    let first = of("bin/engine.dll").len() as u64;
    b.service.on_status(move |status: &Value| {
        let mut armed = armed.lock().unwrap();
        if !*armed
            && status["state"] == "preparing"
            && status["bytesDownloaded"].as_u64().unwrap_or(0) >= first
        {
            *armed = true;
            let me = me.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                if let Some(service) = me.upgrade() {
                    assert!(service.cancel(Some(GAME)).unwrap());
                }
            });
        }
    });
    let error = b
        .repair(RepairCall {
            confirmed: true,
            files: Some(vec![
                "bin/engine.dll".to_owned(),
                "ghostingmod/maps/d1_canals_01.bsp".to_owned(),
            ]),
            ..RepairCall::default()
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, Code::Cancelled);

    assert_eq!(
        b.requests(),
        [
            b.url_of("bin/engine.dll"),
            b.url_of("ghostingmod/maps/d1_canals_01.bsp")
        ]
    );
    assert!(
        !exists(&b.install_path, "bin/engine.dll"),
        "докачанный первый файл в игру не попал"
    );
    assert!(b.staging_leftovers().is_empty());
    assert!(!b.service.is_busy());
    assert_eq!(b.state(), "files-damaged");
    assert!(b.service.status_of(GAME)["error"].is_null());
    assert!(
        !b.service.cancel(Some(GAME)).unwrap(),
        "отменять больше нечего"
    );
}

#[tokio::test]
async fn no_files_manifest_of_the_version_is_an_honest_error() {
    let b = setup(Options {
        no_files_manifest: true,
        ..Options::default()
    })
    .await;
    let code = |failure: Failure| delivery(failure).code;
    assert_eq!(
        code(b.service.verify_files(GAME).await.unwrap_err()),
        Code::FilesManifestMissing
    );
    assert_eq!(
        b.repair(ask()).await.unwrap_err().code,
        Code::FilesManifestMissing
    );
}

#[tokio::test]
async fn stock_cfg_from_ghostingmod_cfg_is_found_and_restored_game_and_launcher_cfg_are_not() {
    let b = setup(Options::default()).await;
    let stock: Vec<&str> = b.files_manifest["stockConfigs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        stock,
        [
            "ghostingmod/cfg/chapter3.cfg",
            "ghostingmod/cfg/skill_manifest.cfg"
        ]
    );
    let listed = b.files_manifest["files"].as_array().unwrap();
    assert!(
        !listed.iter().any(|file| file["path"]
            .as_str()
            .unwrap()
            .starts_with("ghostingmod/cfg/")),
        "в files папки игрока нет"
    );
    remove(&b.install_path, "ghostingmod/cfg/chapter3.cfg");
    write(
        &b.install_path,
        "ghostingmod/cfg/skill_manifest.cfg",
        "мой skill\n".as_bytes(),
    );
    write(
        &b.install_path,
        "ghostingmod/cfg/config.cfg",
        b"bind e +use\n",
    );
    write(
        &b.install_path,
        "ghostingmod/cfg/autoexec.cfg",
        "мой autoexec\n".as_bytes(),
    );
    remove(&b.install_path, "ghostingmod/cfg/hlsr-personalization.cfg");

    let verify = b.service.verify_files(GAME).await.unwrap();
    assert_eq!(verify.config_missing, ["ghostingmod/cfg/chapter3.cfg"]);
    assert_eq!(
        verify.config_changed,
        ["ghostingmod/cfg/skill_manifest.cfg"]
    );

    let check = b.repair(ask()).await.unwrap();
    assert_eq!(
        kinds(&check),
        [
            pair("ghostingmod/cfg/chapter3.cfg", "cfg-missing"),
            pair("ghostingmod/cfg/skill_manifest.cfg", "cfg-changed")
        ]
    );
    let done = b
        .repair(confirmed(Some(&["ghostingmod/cfg/chapter3.cfg"])))
        .await
        .unwrap();
    assert_eq!(done["repaired"], json!(["ghostingmod/cfg/chapter3.cfg"]));
    assert_eq!(b.requests(), [b.url_of("ghostingmod/cfg/chapter3.cfg")]);
    assert_eq!(
        read(&b.install_path, "ghostingmod/cfg/chapter3.cfg"),
        of("ghostingmod/cfg/chapter3.cfg")
    );
    assert_eq!(
        read(&b.install_path, "ghostingmod/cfg/skill_manifest.cfg"),
        "мой skill\n".as_bytes()
    );
    assert_eq!(
        read(&b.install_path, "ghostingmod/cfg/config.cfg"),
        b"bind e +use\n"
    );
    assert_eq!(
        read(&b.install_path, "ghostingmod/cfg/autoexec.cfg"),
        "мой autoexec\n".as_bytes()
    );
    assert!(!exists(
        &b.install_path,
        "ghostingmod/cfg/hlsr-personalization.cfg"
    ));
}

#[tokio::test]
async fn stock_cfgs_are_outside_files_and_manifest_sha256_install_does_not_judge_them() {
    let b = setup(Options::default()).await;
    let mut without = b.files_manifest.clone();
    without
        .as_object_mut()
        .unwrap()
        .shift_remove("stockConfigs");
    let hash = files_manifest::hash_manifest(&b.files_manifest).unwrap();
    assert_eq!(hash, files_manifest::hash_manifest(&without).unwrap());
    assert_eq!(b.files_manifest["manifestSha256"], json!(hash));

    // Так сверяют распакованную сборку установка и обновление.
    remove(&b.install_path, "ghostingmod/cfg/chapter3.cfg");
    write(
        &b.install_path,
        "ghostingmod/cfg/skill_manifest.cfg",
        "мой skill\n".as_bytes(),
    );
    let preserve: Vec<String> = Vec::new();
    let staging = IntegrityService::new(IntegrityOptions::default())
        .check(IntegrityRequest {
            game_id: GAME,
            install_path: &b.install_path,
            files_manifest: &b.files_manifest,
            preserve_paths: &preserve,
            stock_configs: false,
        })
        .await
        .unwrap();
    assert!(staging.ok);
    assert!(staging.config_missing.is_empty() && staging.config_changed.is_empty());
}

#[test]
fn files_manifest_takes_stock_cfgs_only_from_mod_cfg_folders() {
    let base = json!({"schemaVersion": 1, "gameId": GAME, "version": VERSION, "files": []});
    let entry = |rel: &str| json!({"path": rel, "size": 1, "sha256": "a".repeat(64)});
    let with = |stock: Value| {
        let mut data = base.clone();
        data["stockConfigs"] = stock;
        data
    };
    let good = with(json!([entry("ghostingmod/cfg/chapter3.cfg")]));
    assert!(files_manifest::validate(&good, None, None).is_ok());
    for bad in [
        "ghostingmod/SAVE/quick.cfg",
        "ghostingmod/cfg/config.cfg",
        "ghostingmod/cfg/autoexec.cfg",
        "ghostingmod/cfg/hlsr-binds.cfg",
        "ghostingmod/cfg/../../evil.cfg",
        "ghostingmod/cfg/sub/x.cfg",
        "ghostingmod/cfg/notes.txt",
        "bin/engine.dll",
    ] {
        let error = files_manifest::validate(&with(json!([entry(bad)])), None, None).unwrap_err();
        assert_eq!(error.code, Code::ManifestInvalid, "{bad}");
    }
    let twice = with(json!([
        entry("ghostingmod/cfg/a.cfg"),
        entry("ghostingmod/cfg/A.cfg")
    ]));
    assert_eq!(
        files_manifest::validate(&twice, None, None)
            .unwrap_err()
            .code,
        Code::ManifestInvalid
    );
    let not_a_list = with(json!({"path": "ghostingmod/cfg/a.cfg"}));
    assert_eq!(
        files_manifest::validate(&not_a_list, None, None)
            .unwrap_err()
            .message,
        "Некорректный список стоковых cfg в файловом манифесте"
    );
}

#[tokio::test]
async fn install_with_1_0_6_cfgs_is_ready_with_soft_cfg_choices_not_repair() {
    let b = setup(Options::default()).await;
    // Стоковые cfg 1.0.12 — из дерева выпуска; свежая установка получает папку cfg из базы
    // 1.0.6 (патчи её не трогают): одного cfg нет, другой отличается.
    remove(&b.install_path, "ghostingmod/cfg/chapter3.cfg");
    write(
        &b.install_path,
        "ghostingmod/cfg/skill_manifest.cfg",
        "как в 1.0.6\n".as_bytes(),
    );
    b.service
        .set_status(GAME, json!({"installedVersion": VERSION}));

    let verify = b.service.verify_files(GAME).await.unwrap();
    assert!(verify.ok);
    assert_eq!(verify.damaged, 0);
    assert_eq!(verify.config_missing, ["ghostingmod/cfg/chapter3.cfg"]);
    assert_eq!(
        verify.config_changed,
        ["ghostingmod/cfg/skill_manifest.cfg"]
    );
    let status = b.service.status_of(GAME);
    assert_eq!(status["state"], "files-valid");
    assert_eq!(status["integrity"]["stockMissing"], 1);
    assert_eq!(status["mainButton"]["state"], "ready");

    let check = b.repair(ask()).await.unwrap();
    assert_eq!(check["status"], "confirmation-required");
    assert_eq!(check["damaged"], 0);
    assert_eq!(
        kinds(&check),
        [
            pair("ghostingmod/cfg/chapter3.cfg", "cfg-missing"),
            pair("ghostingmod/cfg/skill_manifest.cfg", "cfg-changed")
        ]
    );

    // Пропавший файл сборки — по-прежнему повреждение.
    remove(&b.install_path, "bin/engine.dll");
    let damaged = b.service.verify_files(GAME).await.unwrap();
    assert!(!damaged.ok);
    assert_eq!(damaged.damaged, 1);
    assert_eq!(
        b.service.status_of(GAME)["mainButton"]["state"],
        "repair-required"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn a_junction_instead_of_a_game_folder_is_never_written_through() {
    let b = setup(Options::default()).await;
    let outside = b.dir.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let link = path_in(&b.install_path, "ghostingmod/bin");
    std::fs::remove_dir_all(&link).unwrap();
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&outside)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J");

    let verify = b.service.verify_files(GAME).await.unwrap();
    assert!(
        verify
            .missing
            .iter()
            .any(|path| path == "ghostingmod/bin/client.dll")
    );
    let error = b.repair(confirmed(None)).await.unwrap_err();
    assert_eq!(error.code, Code::PathInvalid);
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    assert!(b.staging_leftovers().is_empty());
}

#[tokio::test]
async fn verify_and_repair_are_only_for_listed_games_in_the_service_too() {
    let b = setup(Options::default()).await;
    let not_enabled = |error: DeliveryError| {
        error.code == Code::RepairSourceMissing && error.details.unwrap()["reason"] == "NOT_ENABLED"
    };
    assert!(not_enabled(delivery(
        b.service.verify_files("hl1-won").await.unwrap_err()
    )));
    assert!(not_enabled(delivery(
        b.service
            .repair("hl1-won", confirmed(None))
            .await
            .unwrap_err()
    )));
    assert_eq!(b.service.status_of(GAME)["fileRepair"], true);
    assert_eq!(b.service.status_of("hl1-won")["fileRepair"], false);
}

#[tokio::test]
async fn with_an_update_and_a_mirror_only_for_the_new_version_it_says_update_first() {
    let b = setup(Options {
        mirror_version: Some("1.0.13"),
        ..Options::default()
    })
    .await;
    let error = b.repair(ask()).await.unwrap_err();
    assert_eq!(error.code, Code::RepairSourceMissing);
    assert_eq!(error.details.unwrap()["reason"], "UPDATE_AVAILABLE");
    assert!(
        error.message.contains("сначала обнови игру"),
        "{}",
        error.message
    );
}

/// Раунд 3 ревью 0.4.9.
#[tokio::test]
async fn an_unticked_missing_stock_cfg_is_not_in_remaining() {
    let b = setup(Options::default()).await;
    remove(&b.install_path, "bin/engine.dll");
    remove(&b.install_path, "ghostingmod/cfg/chapter3.cfg");
    let done = b
        .repair(RepairCall {
            confirmed: true,
            files: Some(vec!["bin/engine.dll".to_owned()]),
            ..RepairCall::default()
        })
        .await
        .unwrap();
    assert_eq!(done["repaired"], json!(["bin/engine.dll"]));
    assert_eq!(done["remaining"], json!([]));
    assert_eq!(done["integrity"]["stockMissing"], 1);
    assert_eq!(b.state(), "repair-complete");
}
