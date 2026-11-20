//! Самотест 0.4.9 `scripts/delivery-trust-selftest.js`: данные с зеркал и сервера канала, которым
//! нельзя верить без подписи. Удаления из манифеста патча — только по подписанным файловым
//! манифестам, без `preservePaths` (A-1); повтор старого подписанного канала не принимается, файл
//! свежести `game-channel-freshness.json` — байт в байт как у 0.4.9 (A-3). Патч ставится настоящим
//! 7za, архив отдаёт локальный сервер.
//!
//! Не перенесено: `game-channel-sign.js` (скрипт выпуска остаётся на Node) и подмена `fs.rmSync`
//! (ошибка удаления отвергнутого манифеста); вторая половина того теста — ниже.
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_delivery::archive_service::ArchiveService;
use hlsr_delivery::channel_service::{ChannelOptions, ChannelService};
use hlsr_delivery::environment::Environment;
use hlsr_delivery::errors::{Code, Failure};
use hlsr_delivery::files_manifest;
use hlsr_delivery::game_delivery_service::{
    GameDeliveryService, Provided, Registry, ServiceOptions,
};
use hlsr_delivery::install_service::InstallTarget;
use hlsr_delivery::manifest_signing::{KeyPair, attach, generate_key_pair};
use hlsr_delivery::manifests::manifest_of;
use hlsr_delivery::patch_format;
use hlsr_delivery::paths::LibraryPaths;
use hlsr_delivery::ports::{DepotProgressFn, FakeFreeSpace, MemoryLogger, SteamDepot};
use hlsr_delivery::providers::BoxFuture;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use support::http::{RequestLog, Server, start_server};
use support::scenario::temp_root;
use support::sha256_hex;

const GAME: &str = "ghosting-renewed";

fn entry(path: &str, text: &str) -> Value {
    json!({"path": path, "size": text.len(), "sha256": sha256_hex(text.as_bytes())})
}

fn files_manifest_of(version: &str, files: Vec<Value>) -> Value {
    let total: u64 = files
        .iter()
        .map(|file| file["size"].as_u64().unwrap())
        .sum();
    json!({
        "schemaVersion": 1,
        "gameId": GAME,
        "version": version,
        "totalFiles": files.len(),
        "totalSize": total,
        "files": files,
    })
}

fn code_of(failure: &Failure) -> &str {
    match failure {
        Failure::Delivery(error) => error.code.as_str(),
        Failure::Raw(error) => panic!("не ошибка доставки: {error:?}"),
    }
}

fn write(root: &Path, relative: &str, data: &str) {
    let full = root.join(relative);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, data).unwrap();
}

fn warnings(logger: &MemoryLogger) -> Vec<String> {
    logger
        .take()
        .into_iter()
        .filter(|(level, _)| *level == "warn")
        .map(|(_, message)| message)
        .collect()
}

// ---------------------------------------------------------------- манифест патча (A-1)

#[tokio::test]
async fn a_mirror_patch_manifest_deletes_only_what_the_previous_build_shipped_and_the_new_one_dropped()
 {
    let (_temp, dir) = temp_root();
    let previous = files_manifest_of(
        "1.0.0",
        vec![
            entry("bin/client.dll", "c"),
            entry("bin/old.dll", "old"),
            entry("bin/server.dll", "v1"),
        ],
    );
    let resulting = files_manifest_of(
        "1.0.1",
        vec![entry("bin/client.dll", "c"), entry("bin/server.dll", "v2")],
    );
    let resulting_sha = files_manifest::hash_manifest(&resulting).unwrap();
    let logger = Arc::new(MemoryLogger::default());
    let service = GameDeliveryService::new(ServiceOptions {
        library_path: dir.join("library").to_string_lossy().into_owned(),
        launcher_version: Some("0.4.9".to_owned()),
        registry: Some(Registry {
            manifests: vec![(GAME.to_owned(), manifest_of(GAME).unwrap().clone())],
            files: vec![(
                GAME.to_owned(),
                vec![
                    ("1.0.0".to_owned(), previous),
                    ("1.0.1".to_owned(), resulting),
                ],
            )],
        }),
        channel_service: Provided::Off,
        logger: Some(logger.clone()),
        ..ServiceOptions::default()
    })
    .unwrap();

    let archive = dir.join(format!("{GAME}-1.0.0-to-1.0.1.7z"));
    let candidate = archive.with_extension("json");
    let load = async |deleted: &[&str], to_version: &str| {
        let manifest = json!({
            "schemaVersion": 1,
            "gameId": GAME,
            "fromVersion": "1.0.0",
            "toVersion": to_version,
            "payloadRoot": "payload",
            "files": [entry("bin/server.dll", "v2")],
            "deletedFiles": deleted,
            "archive": {"filename": archive.file_name().unwrap().to_str().unwrap(), "size": 10, "sha256": "a".repeat(64)},
            "resultingFilesManifestSha256": resulting_sha,
        });
        std::fs::write(&candidate, manifest.to_string()).unwrap();
        let step = json!({
            "fromVersion": "1.0.0",
            "toVersion": to_version,
            "archive": {"sha256": "a".repeat(64)},
            "resultingFilesManifestSha256": resulting_sha,
        });
        service
            .load_patch_manifest(GAME, &step, &archive.to_string_lossy(), &[])
            .await
    };

    for forged in [
        &["bin/old.dll", "ghostingmod/SAVE/quick.sav"][..],
        &["ghostingmod/runs/pb.dem"],
        &["bin/client.dll"],
    ] {
        let failure = load(forged, "1.0.1").await.unwrap_err();
        assert_eq!(code_of(&failure), "MANIFEST_INVALID", "{forged:?}");
        assert!(
            !candidate.exists(),
            "отвергнутый манифест не остаётся в Downloads"
        );
    }

    assert_eq!(
        load(&["bin/old.dll"], "1.0.1").await.unwrap()["deletedFiles"],
        json!(["bin/old.dll"]),
        "честный патч принимается как есть"
    );

    // Итоговой версии нет ни во встроенных, ни в сохранённых манифестах: неподписанные удаления
    // пропускаются, сам патч ставится.
    logger.take();
    let unknown = load(&["bin/old.dll", "ghostingmod/SAVE/quick.sav"], "1.0.2")
        .await
        .unwrap();
    assert_eq!(unknown["deletedFiles"], json!([]));
    assert_eq!(unknown["files"].as_array().unwrap().len(), 1);
    let warned = warnings(&logger);
    assert!(
        warned.iter().any(|line| line.contains("пропущены")),
        "{warned:?}"
    );
}

#[tokio::test]
async fn a_rejected_patch_manifest_does_not_take_an_archive_without_7z_with_it() {
    let (_temp, dir) = temp_root();
    let service = GameDeliveryService::new(ServiceOptions {
        library_path: dir.join("library").to_string_lossy().into_owned(),
        launcher_version: Some("0.4.9".to_owned()),
        registry: Some(Registry {
            manifests: vec![(GAME.to_owned(), manifest_of(GAME).unwrap().clone())],
            files: vec![],
        }),
        channel_service: Provided::Off,
        ..ServiceOptions::default()
    })
    .unwrap();
    // Имя без `.7z`: «манифест рядом» — сам архив.
    let archive = dir.join(format!("{GAME}-1.0.0-to-1.0.1.bin"));
    std::fs::write(&archive, "7z bytes").unwrap();
    let step = json!({"fromVersion": "1.0.0", "toVersion": "1.0.1"});

    let failure = service
        .load_patch_manifest(GAME, &step, &archive.to_string_lossy(), &[])
        .await
        .unwrap_err();
    assert_eq!(code_of(&failure), "MANIFEST_INVALID");
    assert_eq!(
        std::fs::read_to_string(&archive).unwrap(),
        "7z bytes",
        "архив цел"
    );
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

/// Обновление 1.0.0 → 1.0.1 через `apply_update`. Манифест патча в форме `build-patch.js` (с
/// `patchManifestSha256`) уже лежит в Downloads рядом с местом архива; архив (настоящий 7z)
/// отдаёт локальный сервер.
struct PatchEnv {
    service: Arc<GameDeliveryService>,
    game: PathBuf,
    json: PathBuf,
    _server: Server,
    _temp: tempfile::TempDir,
}

impl PatchEnv {
    fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.game.join(relative)).ok()
    }

    async fn update(&self) -> Result<Value, Failure> {
        self.service.apply_update(GAME).await
    }
}

async fn patch_update(deleted: &[&str], both_manifests: bool, files: Option<Value>) -> PatchEnv {
    let (temp, dir) = temp_root();
    let previous = files_manifest_of(
        "1.0.0",
        vec![
            entry("bin/old.dll", "old"),
            entry("bin/server.dll", "v1"),
            entry("ghostingmod/save/start.sav", "start"),
        ],
    );
    let resulting = files_manifest_of("1.0.1", vec![entry("bin/server.dll", "v2")]);
    let resulting_sha = files_manifest::hash_manifest(&resulting).unwrap();
    let archive_name = format!("{GAME}-1.0.0-to-1.0.1.7z");

    // Архив патча: payload/bin/server.dll = "v2".
    let staging = dir.join("payload");
    write(&staging, "payload/bin/server.dll", "v2");
    let archive_file = dir.join(&archive_name);
    let status = std::process::Command::new(hlsr_core::paths::Resources::detect().sevenzip())
        .args(["a", "-bd", "-y"])
        .arg(&archive_file)
        .arg("payload")
        .current_dir(&staging)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let archive_bytes = std::fs::read(&archive_file).unwrap();
    let archive_sha = sha256_hex(&archive_bytes);

    let server = start_server("main", RequestLog::default()).await;
    *server.routes.lock().unwrap() = json!({
        format!("/{archive_name}"): [{"bodyHex": support::zip::hex(&archive_bytes)}],
    })
    .as_object()
    .unwrap()
    .clone();

    let base = manifest_of(GAME).unwrap().clone();
    let mut step = base["patches"][0].clone();
    step["fromVersion"] = json!("1.0.0");
    step["toVersion"] = json!("1.0.1");
    step["archive"]["filename"] = json!(archive_name);
    step["archive"]["size"] = json!(archive_bytes.len());
    step["archive"]["sha256"] = json!(archive_sha);
    step["archive"]["providers"] = json!([{
        "id": "local",
        "type": "local-http",
        "status": "permanent",
        "priority": 10,
        "url": format!("http://{}/{archive_name}", server.authority),
    }]);
    step["deletedFiles"] = json!(deleted.len());
    step["resultingFilesManifestSha256"] = json!(resulting_sha);
    let mut manifest = base;
    manifest["version"] = json!("1.0.1");
    manifest["patches"] = json!([step]);
    manifest.as_object_mut().unwrap().remove("buildMetadata");

    let mut versions = vec![("1.0.0".to_owned(), previous)];
    if both_manifests {
        versions.push(("1.0.1".to_owned(), resulting.clone()));
    }
    let logger = Arc::new(MemoryLogger::default());
    let service = GameDeliveryService::new(ServiceOptions {
        library_path: dir.join("library").to_string_lossy().into_owned(),
        launcher_version: Some("0.4.9".to_owned()),
        env: Some(Environment::new(
            [
                ("GAME_DELIVERY_REMOTE_ENABLED", "1"),
                ("GAME_DELIVERY_ALLOW_LOCAL_HTTP", "1"),
            ],
            false,
        )),
        registry: Some(Registry {
            manifests: vec![(GAME.to_owned(), manifest)],
            files: vec![(GAME.to_owned(), versions)],
        }),
        channel_service: Provided::Off,
        origin_updates: Provided::Off,
        depot: Some(Arc::new(Owns)),
        free_space: Some(Arc::new(FakeFreeSpace::default())),
        update_check_interval_ms: Some(0),
        archive_service: Some(ArchiveService::new(
            hlsr_core::paths::Resources::detect().sevenzip(),
            logger.clone(),
        )),
        logger: Some(logger),
        ..ServiceOptions::default()
    })
    .unwrap();

    let game = service.paths().games.join(GAME);
    for (relative, text) in [
        ("bin/old.dll", "old"),
        ("bin/server.dll", "v1"),
        ("ghostingmod/save/start.sav", "start"),
        ("ghostingmod/SAVE/quick.sav", "мой сейв"),
    ] {
        write(&game, relative, text);
    }
    service
        .install_service()
        .write_installed(
            &InstallTarget {
                game_id: GAME.to_owned(),
                version: json!("1.0.0"),
                install_path: game.clone(),
                staging_path: game.clone(),
                published_at: None,
                manifest_sha256: None,
            },
            json!("a".repeat(64)),
            None,
        )
        .unwrap();

    let mut patch_manifest = Map::new();
    for (key, value) in [
        ("schemaVersion", json!(1)),
        ("gameId", json!(GAME)),
        ("fromVersion", json!("1.0.0")),
        ("toVersion", json!("1.0.1")),
        ("generatedAt", json!("2026-11-07T00:00:00.000Z")),
        ("payloadRoot", json!("payload")),
        (
            "files",
            json!([files.unwrap_or_else(|| entry("bin/server.dll", "v2"))]),
        ),
        ("deletedFiles", json!(deleted)),
        (
            "archive",
            json!({"filename": archive_name, "size": archive_bytes.len(), "sha256": archive_sha}),
        ),
        ("resultingFilesManifestSha256", json!(resulting_sha)),
        ("resultingTotalFiles", resulting["totalFiles"].clone()),
        ("resultingTotalSize", resulting["totalSize"].clone()),
    ] {
        patch_manifest.insert(key.to_owned(), value);
    }
    let mut patch_manifest = Value::Object(patch_manifest);
    patch_manifest["patchManifestSha256"] =
        json!(patch_format::hash_patch_manifest(&patch_manifest).unwrap());
    let downloads = service.paths().downloads.join(GAME);
    std::fs::create_dir_all(&downloads).unwrap();
    let json = downloads.join(format!("{GAME}-1.0.0-to-1.0.1.json"));
    std::fs::write(
        &json,
        hlsr_core::json_text::stringify(&patch_manifest, "  ") + "\n",
    )
    .unwrap();

    PatchEnv {
        service,
        game,
        json,
        _server: server,
        _temp: temp,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_patch_with_deletions_installs_without_the_new_files_manifest_deletions_skipped() {
    let env = patch_update(&["bin/old.dll"], false, None).await;
    let result = env.update().await.unwrap();
    assert_eq!(result["status"], "updated", "{result}");
    assert_eq!(
        env.service.installed_version_of(GAME).as_deref(),
        Some("1.0.1")
    );
    assert_eq!(env.read("bin/server.dll").as_deref(), Some("v2"));
    assert_eq!(
        env.read("bin/old.dll").as_deref(),
        Some("old"),
        "неподписанное удаление пропущено"
    );
    assert_eq!(
        env.read("ghostingmod/SAVE/quick.sav").as_deref(),
        Some("мой сейв")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_patch_does_not_delete_preserve_paths_even_if_the_previous_build_shipped_them() {
    let env = patch_update(&["bin/old.dll", "ghostingmod/save/start.sav"], true, None).await;
    let result = env.update().await.unwrap();
    assert_eq!(result["status"], "updated", "{result}");
    assert_eq!(
        env.read("bin/old.dll"),
        None,
        "подписанное удаление выполнено"
    );
    assert_eq!(
        env.read("ghostingmod/save/start.sav").as_deref(),
        Some("start"),
        "ghostingmod/SAVE в preservePaths"
    );
    assert_eq!(env.read("bin/server.dll").as_deref(), Some("v2"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_patch_manifest_rejected_by_the_patch_service_does_not_stay_in_downloads() {
    // files[] не совпадают с архивом: сверку шага это проходит, проверку содержимого — нет.
    let env = patch_update(&[], false, Some(entry("bin/server.dll", "v3"))).await;
    let failure = env.update().await.unwrap_err();
    assert_eq!(code_of(&failure), "CHECKSUM_MISMATCH");
    assert!(!env.json.exists(), "повтор скачает манифест заново");
    assert_eq!(
        env.service.installed_version_of(GAME).as_deref(),
        Some("1.0.0")
    );
    assert_eq!(env.read("bin/server.dll").as_deref(), Some("v1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forged_deletion_of_a_save_is_refused_and_the_save_stays() {
    let env = patch_update(&["ghostingmod/SAVE/quick.sav"], true, None).await;
    let failure = env.update().await.unwrap_err();
    assert_eq!(code_of(&failure), "MANIFEST_INVALID");
    assert_eq!(
        env.read("ghostingmod/SAVE/quick.sav").as_deref(),
        Some("мой сейв")
    );
    assert!(!env.json.exists());
}

// ---------------------------------------------------------------- свежесть канала (A-3)

fn channel_manifest(version: &str, published_at: &str, keys: &KeyPair) -> Value {
    let provider = json!({"id": "local", "type": "local-http", "status": "permanent", "url": "http://127.0.0.1/x"});
    let doc = json!({
        "schemaVersion": 1,
        "gameId": GAME,
        "channel": "stable",
        "version": version,
        "publishedAt": published_at,
        "minimumLauncherVersion": "0.4.0",
        "updateRequired": false,
        "enabled": true,
        "versionStatus": "assigned",
        "fullArchive": {"filename": format!("{GAME}-{version}.7z"), "size": 1, "sha256": "b".repeat(64), "providers": [provider]},
        "filesManifest": {"version": version, "sha256": "c".repeat(64), "size": 1, "filename": format!("{GAME}-{version}.files.json"), "providers": [provider]},
        "patches": [],
    });
    Value::Object(attach(doc.as_object().unwrap(), &keys.private_key_pem).unwrap())
}

/// Канал с локального сервера и временным ключом; `channel()` — новая служба с тем же файлом
/// свежести, как после перезапуска лаунчера.
struct ChannelFixture {
    keys: KeyPair,
    server: Server,
    url: String,
    freshness_file: PathBuf,
    _temp: tempfile::TempDir,
}

impl ChannelFixture {
    async fn new() -> ChannelFixture {
        let (temp, dir) = temp_root();
        let server = start_server("main", RequestLog::default()).await;
        let url = format!("http://{}/stable.json", server.authority);
        ChannelFixture {
            keys: generate_key_pair(),
            server,
            url,
            freshness_file: dir.join("userData").join("game-channel-freshness.json"),
            _temp: temp,
        }
    }

    fn serve(&self, version: &str, published_at: &str) {
        let body = channel_manifest(version, published_at, &self.keys).to_string();
        *self.server.routes.lock().unwrap() = json!({"/stable.json": [{"body": body}]})
            .as_object()
            .unwrap()
            .clone();
    }

    fn channel(&self, logger: Option<Arc<MemoryLogger>>) -> ChannelService {
        let env = Environment::new(
            [
                ("GAME_DELIVERY_ALLOW_LOCAL_HTTP".to_owned(), "1".to_owned()),
                (
                    "HLSR_GAME_DELIVERY_CHANNEL_URL".to_owned(),
                    self.url.clone(),
                ),
                (
                    "HLSR_GAME_DELIVERY_CHANNEL_KEY".to_owned(),
                    self.keys.public_key_pem.replace('\n', "\\n"),
                ),
            ],
            false,
        );
        ChannelService::new(ChannelOptions {
            logger: logger.map(|logger| logger as _),
            env: Some(env),
            max_attempts: Some(1),
            backoff_base_ms: Some(0),
            freshness_file: Some(self.freshness_file.clone()),
            ..ChannelOptions::default()
        })
    }

    async fn fetch(&self, channel: &ChannelService) -> Result<String, Code> {
        channel
            .fetch(GAME, true)
            .await
            .map(|fetched| fetched.manifest["version"].as_str().unwrap().to_owned())
            .map_err(|error| error.code)
    }

    /// Файл свежести как его пишет 0.4.9: `JSON.stringify(…, null, 2) + "\n"`.
    fn expected_file(&self, published_at: &str) -> String {
        format!(
            "{{\n  \"{GAME}/stable {}\": \"{published_at}\"\n}}\n",
            self.url
        )
    }

    fn file(&self) -> String {
        std::fs::read_to_string(&self.freshness_file).unwrap()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_old_signed_channel_manifest_is_refused_and_a_rollback_needs_a_new_signature() {
    let fixture = ChannelFixture::new().await;

    fixture.serve("1.0.12", "2026-11-01T10:00:00.000Z");
    assert_eq!(
        fixture.fetch(&fixture.channel(None)).await.as_deref(),
        Ok("1.0.12")
    );
    assert_eq!(
        fixture.file(),
        fixture.expected_file("2026-11-01T10:00:00.000Z")
    );

    fixture.serve("1.0.11", "2026-10-31T10:00:00.000Z");
    assert_eq!(
        fixture.fetch(&fixture.channel(None)).await,
        Err(Code::ManifestInvalid),
        "после перезапуска тоже"
    );

    // Владелец откатывает выпуск: прежний манифест подписан заново, дата новее.
    fixture.serve("1.0.11", "2026-11-02T10:00:00.000Z");
    assert_eq!(
        fixture.fetch(&fixture.channel(None)).await.as_deref(),
        Ok("1.0.11")
    );

    fixture.serve("1.0.12", "2026-11-01T10:00:00.000Z");
    assert_eq!(
        fixture.fetch(&fixture.channel(None)).await,
        Err(Code::ManifestInvalid),
        "откат перекрыл прежний выпуск"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_broken_freshness_file_counts_as_empty_and_is_rewritten() {
    let fixture = ChannelFixture::new().await;
    fixture.serve("1.0.12", "2026-11-01T10:00:00.000Z");
    std::fs::create_dir_all(fixture.freshness_file.parent().unwrap()).unwrap();

    for broken in ["5", "\"x\"", "true", "[]", "{мусор"] {
        std::fs::write(&fixture.freshness_file, broken).unwrap();
        assert_eq!(
            fixture.fetch(&fixture.channel(None)).await.as_deref(),
            Ok("1.0.12"),
            "{broken}"
        );
        assert_eq!(
            fixture.file(),
            fixture.expected_file("2026-11-01T10:00:00.000Z"),
            "{broken}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_future_published_at_is_accepted_without_moving_freshness_and_warned_once() {
    let fixture = ChannelFixture::new().await;
    fixture.serve("1.0.12", "2026-11-01T10:00:00.000Z");
    fixture.fetch(&fixture.channel(None)).await.unwrap();

    // Владелец подписал с убежавшими вперёд часами.
    let logger = Arc::new(MemoryLogger::default());
    let channel = fixture.channel(Some(logger.clone()));
    fixture.serve("1.0.13", "2030-01-01T00:00:00.000Z");
    for _ in 0..3 {
        assert_eq!(fixture.fetch(&channel).await.as_deref(), Ok("1.0.13"));
    }
    let warned = warnings(&logger);
    assert_eq!(
        warned
            .iter()
            .filter(|line| line.contains("из будущего"))
            .count(),
        1,
        "{warned:?}"
    );
    assert_eq!(
        fixture.file(),
        fixture.expected_file("2026-11-01T10:00:00.000Z")
    );

    fixture.serve("1.0.13", "2026-11-02T10:00:00.000Z");
    assert_eq!(
        fixture.fetch(&fixture.channel(None)).await.as_deref(),
        Ok("1.0.13"),
        "честная переподпись принимается"
    );
    assert_eq!(
        fixture.file(),
        fixture.expected_file("2026-11-02T10:00:00.000Z")
    );
}

/// Фасад кладёт файл свежести в userData (`channelFreshnessFile`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_service_hands_the_freshness_file_to_its_channel() {
    let fixture = ChannelFixture::new().await;
    fixture.serve("1.0.12", "2026-11-01T10:00:00.000Z");
    let channel = Arc::new(fixture.channel(None));
    let (_temp, dir) = temp_root();
    let service = GameDeliveryService::new(ServiceOptions {
        library_path: dir.join("library").to_string_lossy().into_owned(),
        channel_service: Provided::Given(channel.clone()),
        ..ServiceOptions::default()
    })
    .unwrap();
    let elsewhere = dir.join("userData").join("game-channel-freshness.json");
    service.set_channel_freshness_file(elsewhere.clone());
    fixture.fetch(&channel).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(&elsewhere).unwrap(),
        fixture.expected_file("2026-11-01T10:00:00.000Z")
    );
    assert!(!fixture.freshness_file.exists());
}
