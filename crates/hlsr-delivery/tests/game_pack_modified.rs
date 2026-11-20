//! Самотест 0.4.9 `scripts/game-pack-modified-selftest.js`: импорт сборки `.hlsrpack` с
//! изменёнными файлами сборки (кастомный HUD поверх файлов игры). Импорт не отменяется:
//! изменённый контент откладывается (Ghosting — в `ghostingmod/custom/imported`, остальное — в
//! `.hlsr/imported-modified`), изменённые программы не принимаются, оригиналы — с зеркала ремонта.
//! Экспорт такой сборки сперва предупреждает (`PACK_MODIFIED`).
//!
//! Настоящий 7za из `resources/7zip`, подписанный QA-канал с `repairMirror` и зеркало (раскладка
//! `sha256-2`) на 127.0.0.1 — вместо `build-repair-mirror.js` маршруты собираются из списка файлов.

mod support;

use hlsr_delivery::archive_service::ArchiveService;
use hlsr_delivery::channel_service::{ChannelOptions, ChannelService};
use hlsr_delivery::environment::Environment;
use hlsr_delivery::errors::{Code, DeliveryError, Failure};
use hlsr_delivery::files_manifest::{self, BuildOptions};
use hlsr_delivery::game_actions::LauncherIdentity;
use hlsr_delivery::game_delivery_service::{
    GameDeliveryService, PackExtra, Provided, Registry, ServiceOptions,
};
use hlsr_delivery::game_pack;
use hlsr_delivery::install_service::{InstallTarget, IntegrityRequest};
use hlsr_delivery::integrity_service::{IntegrityOptions, IntegrityService};
use hlsr_delivery::manifest_signing::{attach, generate_key_pair};
use hlsr_delivery::manifests;
use hlsr_delivery::paths::{LibraryPaths, game_install_path};
use hlsr_delivery::ports::{DepotProgressFn, Logger, MemoryLogger, SteamDepot};
use hlsr_delivery::providers::BoxFuture;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use support::api::{Setup as ApiSetup, harness};
use support::http::{RequestLog, Server, start_server};
use support::library::copy_tree;
use support::scenario::{path_in, temp_root};
use support::zip::hex;

const VERSION: &str = "1.0.12";

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

/// `MZ(seed)`: заголовок PE, SHA-256 сида и 300 единиц.
fn mz(seed: &str) -> Vec<u8> {
    let mut bytes = b"MZ".to_vec();
    bytes.extend(Sha256::digest(seed.as_bytes()));
    bytes.extend([1u8; 300]);
    bytes
}

fn build(game: &str) -> Vec<(&'static str, Vec<u8>)> {
    match game {
        "ghosting-renewed" => vec![
            ("hl2.exe", mz("hl2")),
            ("bin/engine.dll", mz("engine")),
            ("ghostingmod/bin/client.dll", mz("client")),
            (
                "ghostingmod/scripts/HudLayout.res",
                b"stock layout\n".to_vec(),
            ),
            (
                "ghostingmod/resource/clientscheme.res",
                b"stock scheme\n".to_vec(),
            ),
            ("hl2/resource/hl2_english.txt", b"stock text\n".to_vec()),
            (
                "platform/resource/platform_english.txt",
                b"stock platform\n".to_vec(),
            ),
        ],
        "hl1-won" => vec![
            ("Half-Life/hl.exe", mz("hl")),
            (
                "Half-Life/valve_WON/pak0.pak",
                "pak-data ".repeat(500).into_bytes(),
            ),
            (
                "Half-Life/valve_WON/resource/GameMenu.res",
                b"stock menu".to_vec(),
            ),
        ],
        other => panic!("{other}"),
    }
}

fn write(root: &Path, rel: &str, data: &[u8]) {
    let file = path_in(root, rel);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, data).unwrap();
}

fn read(root: &Path, rel: &str) -> Vec<u8> {
    std::fs::read(path_in(root, rel)).unwrap()
}

fn text(root: &Path, rel: &str) -> String {
    String::from_utf8(read(root, rel)).unwrap()
}

fn exists(root: &Path, rel: &str) -> bool {
    path_in(root, rel).exists()
}

#[derive(Default)]
struct Options {
    /// Канал без зеркала (нет сети).
    no_mirror: bool,
    extra: Vec<(&'static str, Vec<u8>)>,
}

struct Env {
    _temp: tempfile::TempDir,
    dir: PathBuf,
    game: &'static str,
    install_path: PathBuf,
    service: Arc<GameDeliveryService>,
    files_manifest: Value,
    mirror_log: RequestLog,
    _servers: (Server, Server),
}

/// Установленная сборка версии [`VERSION`].
async fn setup(game: &'static str, options: Options) -> Env {
    let (temp, dir) = temp_root();
    let build_root = dir.join("build");
    for (rel, data) in build(game) {
        write(&build_root, rel, &data);
    }
    let mut build_options = BuildOptions::new(&build_root);
    build_options.game_id = Some(game);
    build_options.version = Some(VERSION);
    let files_manifest = files_manifest::build(build_options).unwrap().manifest;

    // Зеркало ремонта: файлы по SHA-256, как раскладывает build-repair-mirror.js.
    let mirror_log = RequestLog::default();
    let mirror = start_server("mirror", mirror_log.clone()).await;
    let mirror_path = format!("/{game}/repair");
    let mut routes = Map::new();
    for entry in files_manifest["files"].as_array().into_iter().flatten() {
        let sha = entry["sha256"].as_str().unwrap();
        let data = read(&build_root, entry["path"].as_str().unwrap());
        routes.insert(
            format!("{mirror_path}/{}/{sha}", &sha[..2]),
            json!([{"bodyHex": hex(&data)}]),
        );
    }
    *mirror.routes.lock().unwrap() = routes;

    // Подписанный канал версии объявляет зеркало (то, что кладёт game-release-prepare.js).
    let channel_server = start_server("channel", RequestLog::default()).await;
    let keys = generate_key_pair();
    let provider = |url: String| json!({"id": "local", "type": "local-http", "status": "permanent", "priority": 10, "url": url});
    let mut channel = json!({
        "schemaVersion": 1,
        "gameId": game,
        "channel": "stable",
        "version": VERSION,
        "publishedAt": "2026-11-01T00:00:00.000Z",
        "minimumLauncherVersion": "0.3.9",
        "enabled": true,
        "updateRequired": false,
        "fullArchive": {"filename": format!("{game}-{VERSION}.7z"), "size": 1, "sha256": "a".repeat(64),
                        "rootPrefix": null, "providers": [provider(format!("http://{}/g.7z", channel_server.authority))]},
        "filesManifest": {"version": VERSION, "filename": "f.json", "size": 1, "sha256": "b".repeat(64),
                          "providers": [provider(format!("http://{}/f.json", channel_server.authority))]},
        "patches": [],
    });
    if !options.no_mirror {
        channel["repairMirror"] = json!({
            "version": VERSION,
            "layout": "sha256-2",
            "maxFileSize": 256 * 1024 * 1024,
            "providers": [provider(format!("http://{}{mirror_path}", mirror.authority))],
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
        .find(|(id, _)| *id == game)
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
        launcher_version: Some("0.4.9".to_owned()),
        env: Some(env.clone()),
        registry: Some(Registry {
            manifests: vec![(game.to_owned(), manifest)],
            files: vec![(
                game.to_owned(),
                vec![(VERSION.to_owned(), files_manifest.clone())],
            )],
        }),
        channel_service: Provided::Given(Arc::new(ChannelService::new(ChannelOptions {
            env: Some(env),
            max_attempts: Some(1),
            ..ChannelOptions::default()
        }))),
        origin_updates: Provided::Off,
        depot: Some(Arc::new(Owns)),
        update_check_interval_ms: Some(0),
        archive_service: Some(ArchiveService::new(
            hlsr_core::paths::Resources::detect().sevenzip(),
            logger.clone(),
        )),
        logger: Some(logger),
        ..ServiceOptions::default()
    })
    .unwrap();

    let install_path = game_install_path(service.paths(), game).unwrap();
    copy_tree(&build_root, &install_path);
    for (rel, data) in &options.extra {
        write(&install_path, rel, data);
    }
    service
        .install_service()
        .write_installed(
            &InstallTarget {
                game_id: game.to_owned(),
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

    Env {
        _temp: temp,
        dir,
        game,
        install_path,
        service,
        files_manifest,
        mirror_log,
        _servers: (mirror, channel_server),
    }
}

fn delivery(failure: Failure) -> DeliveryError {
    match failure {
        Failure::Delivery(error) => error,
        Failure::Raw(raw) => panic!("{raw:?}"),
    }
}

impl Env {
    async fn export(&self, name: &str, allow_modified: bool) -> Result<PathBuf, DeliveryError> {
        let file = self.dir.join(name);
        let extra = PackExtra {
            launcher_id: Some(json!(self.game)),
            allow_modified,
            ..PackExtra::default()
        };
        self.service
            .export_pack(self.game, &file, extra)
            .await
            .map(|_| file)
            .map_err(delivery)
    }

    async fn import(&self, file: &Path) -> Result<Value, DeliveryError> {
        let meta = self.service.read_pack_meta(file).await.map_err(delivery)?;
        self.service
            .import_pack(file, &meta)
            .await
            .map_err(delivery)
    }

    fn requests(&self) -> usize {
        self.mirror_log.lock().unwrap().len()
    }

    fn staging_is_empty(&self) -> bool {
        std::fs::read_dir(&self.service.paths().staging)
            .map(|mut dir| dir.next().is_none())
            .unwrap_or(true)
    }
}

/// Все файлы папки — найти, не легла ли изменённая программа куда-нибудь.
fn contents(root: &Path) -> Vec<Vec<u8>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            found.extend(contents(&entry.path()));
        } else {
            found.push(std::fs::read(entry.path()).unwrap());
        }
    }
    found
}

#[tokio::test]
async fn ghosting_modified_res_goes_to_custom_dll_is_refused_originals_come_from_the_mirror() {
    let evil_dll = mz("modified client");
    let source = setup(
        "ghosting-renewed",
        Options {
            extra: vec![
                (
                    "ghostingmod/scripts/HudLayout.res",
                    b"my hud layout\n".to_vec(),
                ),
                ("ghostingmod/bin/client.dll", evil_dll.clone()),
                (
                    "platform/resource/platform_english.txt",
                    b"my platform\n".to_vec(),
                ),
                ("ghostingmod/save/quick.sav", b"save".to_vec()),
            ],
            ..Options::default()
        },
    )
    .await;

    // Экспорт сперва предупреждает: сколько изменено и что у Ghosting есть custom.
    let error = source.export("g.hlsrpack", false).await.unwrap_err();
    assert_eq!(error.code, Code::PackModified);
    assert_eq!(error.details, Some(json!({"modified": 3, "custom": true})));
    assert_eq!(source.service.status_of(source.game)["state"], "ready");
    let file = source.export("g.hlsrpack", true).await.unwrap();

    let target = setup("ghosting-renewed", Options::default()).await;
    let result = target.import(&file).await.unwrap();
    let t = &target.install_path;
    assert_eq!(result["status"], "ok");
    assert_eq!(
        result["modified"],
        json!({"changed": 3, "restored": 3, "custom": 1,
               "customDir": t.join("ghostingmod").join("custom").join("imported").to_string_lossy(),
               "backup": 1,
               "backupDir": t.join(".hlsr").join("imported-modified").to_string_lossy(),
               "rejected": 1})
    );
    assert_eq!(target.requests(), 3, "оригиналы — с зеркала");

    // Кастомный HUD: в слое custom (игра читает его первым), на месте сборки — оригинал.
    assert_eq!(
        text(t, "ghostingmod/custom/imported/scripts/HudLayout.res"),
        "my hud layout\n"
    );
    assert_eq!(
        text(t, "ghostingmod/scripts/HudLayout.res"),
        "stock layout\n"
    );
    // Изменённая библиотека нигде не лежит, на месте — оригинал.
    assert_eq!(read(t, "ghostingmod/bin/client.dll"), mz("client"));
    assert!(
        !contents(t).contains(&evil_dll),
        "изменённой .dll нет нигде в папке игры"
    );
    // Файл вне папок, которые игра ищет через custom, — копией в .hlsr/imported-modified.
    assert_eq!(
        text(
            t,
            ".hlsr/imported-modified/platform/resource/platform_english.txt"
        ),
        "my platform\n"
    );
    assert_eq!(
        text(t, "platform/resource/platform_english.txt"),
        "stock platform\n"
    );
    assert_eq!(text(t, "ghostingmod/save/quick.sav"), "save");
    assert!(target.staging_is_empty());

    // Установленная сборка проходит проверку файлов, повторный экспорт не предупреждает и не
    // везёт резервные копии.
    let check = IntegrityService::new(IntegrityOptions::default())
        .check(IntegrityRequest {
            game_id: target.game,
            install_path: t,
            files_manifest: &target.files_manifest,
            preserve_paths: &[],
            stock_configs: false,
        })
        .await
        .unwrap();
    assert!(check.ok);
    let again = target.export("again.hlsrpack", false).await.unwrap();
    let third = setup("ghosting-renewed", Options::default()).await;
    let reimported = third.import(&again).await.unwrap();
    assert_eq!(reimported["modified"], Value::Null);
    assert_eq!(
        text(
            &third.install_path,
            "ghostingmod/custom/imported/scripts/HudLayout.res"
        ),
        "my hud layout\n"
    );
    assert!(!exists(&third.install_path, ".hlsr/imported-modified"));
}

#[tokio::test]
async fn without_network_no_mirror_import_with_modified_files_is_cancelled_build_untouched() {
    let source = setup(
        "ghosting-renewed",
        Options {
            extra: vec![("ghostingmod/scripts/HudLayout.res", b"my hud\n".to_vec())],
            ..Options::default()
        },
    )
    .await;
    let file = source.export("g.hlsrpack", true).await.unwrap();

    let target = setup(
        "ghosting-renewed",
        Options {
            no_mirror: true,
            extra: vec![("ghostingmod/save/mine.sav", b"keep".to_vec())],
        },
    )
    .await;
    let error = target.import(&file).await.unwrap_err();
    assert_eq!(error.code, Code::PackOriginalsUnavailable);
    assert_eq!(error.details.as_ref().unwrap()["changed"], 1);
    let t = &target.install_path;
    assert_eq!(text(t, "ghostingmod/save/mine.sav"), "keep");
    assert_eq!(
        text(t, "ghostingmod/scripts/HudLayout.res"),
        "stock layout\n"
    );
    assert!(!exists(t, "ghostingmod/custom/imported"));
    let installed = target
        .service
        .install_service()
        .read_installed(target.game)
        .unwrap()
        .unwrap();
    assert_eq!(installed["version"], VERSION);
    assert!(target.staging_is_empty());
}

/// Отступление от JS: самотест 0.4.9 подкладывает HL1 результат проверки с `repairMirror`
/// (`updateChecker.results.set`), а у HL1 нет подписанного канала, и источник (`origin-updates.js`)
/// отдаёт `repairMirror: null`. Здесь — то, что видит игрок: экспорт предупреждает без custom,
/// импорт с изменёнными файлами отменяется, сборка не тронута. Раскладку HL1 в
/// `.hlsr/imported-modified` проверяет самотест `set_aside`.
#[tokio::test]
async fn hl1_export_warns_without_custom_import_without_a_mirror_is_cancelled() {
    let source = setup(
        "hl1-won",
        Options {
            extra: vec![(
                "Half-Life/valve_WON/resource/GameMenu.res",
                b"my menu".to_vec(),
            )],
            ..Options::default()
        },
    )
    .await;
    let error = source.export("h.hlsrpack", false).await.unwrap_err();
    assert_eq!(error.code, Code::PackModified);
    assert_eq!(error.details, Some(json!({"modified": 1, "custom": false})));
    let file = source.export("h.hlsrpack", true).await.unwrap();

    let target = setup("hl1-won", Options::default()).await;
    let error = target.import(&file).await.unwrap_err();
    assert_eq!(error.code, Code::PackOriginalsUnavailable);
    assert_eq!(
        error.details.as_ref().unwrap()["samples"],
        json!(["Half-Life/valve_WON/resource/GameMenu.res"])
    );
    let t = &target.install_path;
    assert_eq!(
        text(t, "Half-Life/valve_WON/resource/GameMenu.res"),
        "stock menu"
    );
    assert!(!exists(t, ".hlsr/imported-modified"));
    assert!(target.staging_is_empty());
}

#[tokio::test]
async fn unchanged_build_export_without_question_import_without_mirror_and_modified() {
    let source = setup("hl1-won", Options::default()).await;
    let file = source.export("h.hlsrpack", false).await.unwrap();
    let target = setup(
        "hl1-won",
        Options {
            no_mirror: true,
            ..Options::default()
        },
    )
    .await;
    let result = target.import(&file).await.unwrap();
    assert_eq!(result["modified"], Value::Null);
    assert_eq!(target.requests(), 0);
}

#[test]
fn set_aside_refuses_broken_header_library_and_script_lays_content_out_by_layers() {
    let (_temp, root) = temp_root();
    let files: [(&str, &str); 6] = [
        ("ghostingmod/bin/client.dll", "not a PE at all"),
        ("ghostingmod/bin/hook.asi", "text"),
        ("ghostingmod/cfg/run.bat", "start calc"),
        ("ghostingmod/materials/hud/x.vmt", "vmt"),
        ("hl2/sound/a.wav", "wav"),
        ("steam.inf", "inf"),
    ];
    for (rel, data) in files {
        write(&root, rel, data.as_bytes());
    }
    let mut changed: Vec<String> = files.iter().map(|(rel, _)| (*rel).to_owned()).collect();
    changed.push("missing/file.txt".to_owned());
    let mut result = game_pack::set_aside(&root, "ghosting-renewed", &changed).unwrap();
    result.rejected.sort();
    result.custom.sort();
    assert_eq!(
        result.rejected,
        [
            "ghostingmod/bin/client.dll",
            "ghostingmod/bin/hook.asi",
            "ghostingmod/cfg/run.bat"
        ]
    );
    assert_eq!(
        result.custom,
        [
            "ghostingmod/custom/imported/materials/hud/x.vmt",
            "ghostingmod/custom/imported/sound/a.wav"
        ]
    );
    assert_eq!(result.backup, [".hlsr/imported-modified/steam.inf"]);
    for (rel, _) in files {
        assert!(!exists(&root, rel), "{rel}");
    }
    // У игры без слоя custom всё — копией в .hlsr/imported-modified.
    write(&root, "Half-Life/valve_WON/a.txt", b"a");
    assert_eq!(
        game_pack::set_aside(&root, "hl1-won", &["Half-Life/valve_WON/a.txt".to_owned()])
            .unwrap()
            .backup,
        [".hlsr/imported-modified/Half-Life/valve_WON/a.txt"]
    );
    assert_eq!(
        game_pack::custom_layer_of("ghosting-renewed"),
        Some("ghostingmod/custom/imported")
    );
    assert_eq!(game_pack::custom_layer_of("hl1-won"), None);
}

/// `hl2/x` и `ghostingmod/x` идут в один файл слоя: слой берёт `ghostingmod/x`, `hl2/x` и файл
/// поверх уже лежащего в слое — копией, ничего не затирается.
#[test]
fn set_aside_keeps_both_files_that_map_to_one_layer_file() {
    let (_temp, root) = temp_root();
    write(&root, "hl2/resource/a.res", b"hl2");
    write(&root, "ghostingmod/resource/a.res", b"mod");
    write(&root, "ghostingmod/custom/imported/resource/b.res", b"old");
    write(&root, "hl2/resource/b.res", b"new");
    let changed = [
        "hl2/resource/a.res",
        "ghostingmod/resource/a.res",
        "hl2/resource/b.res",
    ]
    .map(str::to_owned);
    let mut result = game_pack::set_aside(&root, "ghosting-renewed", &changed).unwrap();
    result.backup.sort();
    assert_eq!(
        result.custom,
        ["ghostingmod/custom/imported/resource/a.res"]
    );
    assert_eq!(
        result.backup,
        [
            ".hlsr/imported-modified/hl2/resource/a.res",
            ".hlsr/imported-modified/hl2/resource/b.res"
        ]
    );
    assert_eq!(
        text(&root, "ghostingmod/custom/imported/resource/a.res"),
        "mod"
    );
    assert_eq!(
        text(&root, "ghostingmod/custom/imported/resource/b.res"),
        "old"
    );
    assert_eq!(
        text(&root, ".hlsr/imported-modified/hl2/resource/a.res"),
        "hl2"
    );
    assert_eq!(
        text(&root, ".hlsr/imported-modified/hl2/resource/b.res"),
        "new"
    );
}

/// `game-build-export` → `confirmation-required` (`pack-modified`), путь к архиву остаётся в
/// фасаде; `game-build-export-confirm` с одноразовым токеном сохраняет архив как есть.
#[tokio::test]
async fn export_confirm_takes_a_one_time_token() {
    let (_temp, root) = temp_root();
    let build_root = root.join("build");
    for (rel, data) in build("hl1-won") {
        write(&build_root, rel, &data);
    }
    let mut build_options = BuildOptions::new(&build_root);
    build_options.game_id = Some("hl1-won");
    build_options.version = Some(VERSION);
    let files = files_manifest::build(build_options).unwrap().manifest;
    let mut manifest = manifests::manifests()
        .into_iter()
        .find(|(id, _)| *id == "hl1-won")
        .unwrap()
        .1
        .clone();
    manifest
        .as_object_mut()
        .unwrap()
        .shift_remove("buildMetadata");
    let h = harness(ApiSetup {
        root: root.clone(),
        registry: Some(Registry {
            manifests: vec![("hl1-won".to_owned(), manifest)],
            files: vec![("hl1-won".to_owned(), vec![(VERSION.to_owned(), files)])],
        }),
        now: 1_792_152_000_000,
        launcher: LauncherIdentity {
            exec_path: PathBuf::from("C:/hlsr/HLSR.exe"),
            app_image: None,
            packaged: true,
            app_path: None,
        },
    });
    let install = root.join("lib").join("Games").join("hl1-won");
    copy_tree(&build_root, &install);
    write(
        &install,
        "Half-Life/valve_WON/resource/GameMenu.res",
        b"my menu",
    );
    let installed = json!({"schemaVersion": 1, "gameId": "hl1-won", "version": VERSION, "state": "ready",
        "installPath": install.to_string_lossy(), "installedAt": "2026-11-01T12:00:00.000Z"});
    std::fs::write(install.join("installed.json"), installed.to_string()).unwrap();
    let target = root.join("out.hlsrpack");
    let call = |channel: &'static str, args: Value| {
        let api = h.api.clone();
        async move { api.call(channel, args.as_array().unwrap()).await.unwrap() }
    };
    let ask = || {
        h.dialogs
            .answers
            .lock()
            .unwrap()
            .push_back(json!({"save": target.to_string_lossy()}));
    };

    ask();
    let asked = call("game-build-export", json!(["hl1-won"])).await;
    let token = asked["token"].as_str().unwrap().to_owned();
    assert_eq!(token.len(), 32);
    assert_eq!(
        asked,
        json!({"status": "confirmation-required", "kind": "pack-modified", "token": token,
               "gameId": "hl1-won", "modified": 1, "custom": false})
    );
    assert!(!target.exists());
    // Чужой токен гасит ожидание: и верный после него уже не годится.
    let refused = json!({"status": "error", "code": "PACK_FAILED"});
    assert_eq!(
        call("game-build-export-confirm", json!(["nope"])).await,
        refused
    );
    assert_eq!(
        call("game-build-export-confirm", json!([token])).await,
        refused
    );

    ask();
    let token = call("game-build-export", json!(["hl1-won"])).await["token"].clone();
    let saved = call("game-build-export-confirm", json!([token])).await;
    assert_eq!(saved["status"], "ok", "{saved}");
    assert_eq!(saved["kind"], "pack");
    assert!(target.exists());
    assert_eq!(
        call("game-build-export-confirm", json!([token])).await,
        refused
    );
}
