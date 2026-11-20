//! Самотесты 0.4.7 о паках HL1 (`scripts/hl1-selftest.js`, строки 797–983) с теми же случаями.
//! В 0.4.7 Google Drive и 7-Zip подменены объектами («архив» — строка с именем папки-образца);
//! здесь вместо них локальный сервер, настоящие zip-архивы (тот же состав файлов) и настоящий
//! 7za. Адрес Drive подменяется только в отладочной сборке (`PackOptions::drive_download`).
//!
//! Не здесь: тест «паки применяются до запуска, результат уходит в интерфейс»
//! (`LaunchService`, задача 21).

#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_core::paths::Resources;
use hlsr_delivery::archive_service::ArchiveService;
use hlsr_delivery::goldsrc_packs::{self as packs, Catalogue, Outcome, Pack, PackOptions};
use hlsr_delivery::http_client::HttpClient;
use hlsr_delivery::manifests;
use hlsr_delivery::mutable_paths;
use hlsr_delivery::ports::MemoryLogger;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use support::http::{RequestLog, Server, start_server};
use support::zip::{hex, stored};

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// `packInstall(root)`: установка с оригиналами WON и valve.
fn pack_install(root: &Path) -> PathBuf {
    write(&root.join("Half-Life/hl.exe"), "MZ");
    write(
        &root.join("Half-Life/valve_WON/liblist.gam"),
        "game \"Half-Life\"\n",
    );
    write(&root.join("Half-Life/steam_appid.txt"), "70");
    write(&root.join("Bunnymod XT/Injector.exe"), "MZ");
    let sprites = root.join("Half-Life/valve_WON/sprites");
    write(
        &sprites.join("weapon_tripmine.txt"),
        "7\r\nweapon\t\t320 320hud1\t160\t80\t80\t20\r\ncrosshair\t\t640 crosshairs\t48\t48\t24\t24",
    );
    write(
        &root.join("Half-Life/valve/sprites/crosshairs.spr"),
        "VALVE-CROSS",
    );
    write(&root.join("Half-Life/valve/sprites/hud.txt"), "VALVE HUD");
    sprites
}

const TRIPMINE_PACK: &str = "8\r\nweapon\t\t320 320hud1\t160\t40\t80\t20\r\ncrosshair\t\t320 xhair\t0\t128\t128\t128\r\nweapon\t\t640 640hud3\t0\t45\t170\t45\r\ncrosshair\t\t640 xhair\t0\t128\t128\t128\r\n";

/// `packFixtures(dir)`: три архива-образца, каталог под них и сервер.
struct Rig {
    server: Server,
    log: RequestLog,
    catalogue: Catalogue,
    client: HttpClient,
    archive: ArchiveService,
    logger: Arc<MemoryLogger>,
}

fn entry(zip: &[u8], id: &str, root: &str, kind: &str) -> Pack {
    Pack {
        kind: kind.to_owned(),
        title: id.to_owned(),
        author: "test".to_owned(),
        drive_id: id.to_owned(),
        size: zip.len(),
        sha256: sha(zip),
        root: root.to_owned(),
    }
}

impl Rig {
    async fn new() -> Rig {
        let hud = stored(&[
            ("big AG HUD/", b""),
            ("big AG HUD/hud.txt", b"AG HUD"),
            ("big AG HUD/hud_numbers_0.spr", b"AG0"),
        ]);
        let cross = stored(&[
            ("crosshair by execut4ble/", b""),
            ("crosshair by execut4ble/xhair.spr", b"XHAIR"),
            (
                "crosshair by execut4ble/weapon_tripmine.txt",
                TRIPMINE_PACK.as_bytes(),
            ),
        ]);
        let changer = stored(&[("crosshairs/", b""), ("crosshairs/1px/red.spr", b"RED-1PX")]);
        let catalogue: Catalogue = vec![
            ("ag-hud".to_owned(), entry(&hud, "hud", "big AG HUD", "hud")),
            (
                "execut4ble-crosshair".to_owned(),
                entry(&cross, "cross", "crosshair by execut4ble", "crosshair"),
            ),
            (
                "crosshair-changer".to_owned(),
                entry(&changer, "changer", "crosshairs", "crosshair"),
            ),
        ];
        let log = RequestLog::default();
        let server = start_server("a", log.clone()).await;
        {
            let mut routes = server.routes.lock().unwrap();
            for (id, zip) in [("hud", &hud), ("cross", &cross), ("changer", &changer)] {
                routes.insert(
                    format!("/download?id={id}&export=download"),
                    json!([{"bodyHex": hex(zip)}]),
                );
            }
        }
        let archive = ArchiveService::new(
            Resources::detect().sevenzip(),
            Arc::new(MemoryLogger::default()),
        )
        .with_env([("LC_ALL", "C.UTF-8"), ("LANG", "C.UTF-8"), ("TZ", "UTC")]);
        Rig {
            server,
            log,
            catalogue,
            client: HttpClient::new(),
            archive,
            logger: Arc::new(MemoryLogger::default()),
        }
    }

    async fn apply(&self, root: &Path, settings: &Value) -> Outcome {
        self.apply_with(root, settings, &self.catalogue).await
    }

    async fn apply_with(&self, root: &Path, settings: &Value, catalogue: &Catalogue) -> Outcome {
        let config = manifests::launch_config_of("hl1-won").unwrap();
        let base = format!("http://{}/download", self.server.authority);
        let logger: &MemoryLogger = &self.logger;
        let options = PackOptions {
            client: Some(&self.client),
            archive: Some(&self.archive),
            logger: Some(logger),
            catalogue: Some(catalogue),
            drive_download: Some(&base),
        };
        packs::apply_packs(root, config, settings, &options)
            .await
            .expect("applyPacks")
    }

    fn downloads(&self) -> usize {
        self.log.lock().unwrap().len()
    }
}

/// Будущее `apply_packs` уходит в другой поток (его запустит служба запуска игры): проверка на
/// этапе компиляции.
#[allow(dead_code)]
fn apply_packs_future_is_send<'a>(
    install: &'a Path,
    config: &'a manifests::LaunchConfig,
    settings: &'a Value,
    options: &'a PackOptions<'a>,
) {
    fn is_send<T: Send>(_: T) {}
    is_send(packs::apply_packs(install, config, settings, options));
}

fn read_utf8(path: &Path) -> String {
    String::from_utf8(std::fs::read(path).unwrap()).unwrap()
}

fn read_latin1(path: &Path) -> String {
    hlsr_core::js::latin1_string(&std::fs::read(path).unwrap())
}

fn names(dir: &Path) -> Vec<String> {
    let mut list: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    list.sort();
    list
}

#[test]
fn packs_are_declared_with_author_pinned_hash_and_are_not_damage() {
    let config = manifests::launch_config_of("hl1-won").unwrap();
    assert!(
        config.packs.is_some(),
        "у HL1 есть папка спрайтов для паков"
    );
    for (id, pack) in packs::catalogue() {
        assert!(
            pack.sha256.len() == 64
                && pack
                    .sha256
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
            "{id}"
        );
        assert!(!pack.author.is_empty() && pack.size > 0, "{id}");
    }
    assert!(mutable_paths::matches_mutable(
        "Half-Life/valve_WON/sprites/hud.txt"
    ));
    assert!(mutable_paths::matches_mutable(
        "Half-Life/valve_WON/sprites/sub/x.spr"
    ));
    assert!(
        !mutable_paths::matches_mutable("Half-Life/valve/sprites/hud.txt"),
        "спрайты valve проверяются как раньше"
    );

    let selection = packs::sanitize_packs(
        &json!({"hud": "evil", "crosshair": "crosshair-changer", "changerShape": "../x"}),
    );
    assert_eq!(
        selection.to_value(),
        json!({"hud": "default", "crosshair": "crosshair-changer", "changerShape": "Default", "changerColor": "green"})
    );
}

#[test]
fn execut4ble_crosshair_changes_only_the_crosshair_lines_weapon_icons_stay_from_won() {
    let base = b"7\r\nweapon\t\t320 320hud1\t160\t80\t80\t20\r\nammo\t\t320 320hud2\t72\t34\t18\t18\r\ncrosshair\t\t640 crosshairs\t48\t48\t24\t24";
    let pack = b"8\r\nweapon\t\t320 320hud1\t160\t40\t80\t20\r\ncrosshair\t\t320 xhair\t0\t128\t128\t128\r\nzoom\t\t640 xhair\t0\t128\t128\t128\r\n";
    let merged = hlsr_core::js::latin1_string(&packs::merge_crosshair_lines(base, pack));
    let lines: Vec<&str> = merged.trim().split("\r\n").collect();
    assert_eq!(
        lines,
        [
            "4",
            "weapon\t\t320 320hud1\t160\t80\t80\t20",
            "ammo\t\t320 320hud2\t72\t34\t18\t18",
            "crosshair\t\t320 xhair\t0\t128\t128\t128",
            "zoom\t\t640 xhair\t0\t128\t128\t128",
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selected_packs_are_installed_and_disabling_brings_everything_back() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("game");
    let sprites = pack_install(&root);
    let tripmine_before = std::fs::read(sprites.join("weapon_tripmine.txt")).unwrap();
    let rig = Rig::new().await;

    let applied = rig
        .apply(
            &root,
            &json!({"packs": {"hud": "ag-hud", "crosshair": "execut4ble-crosshair"}}),
        )
        .await;
    assert_eq!(applied.status(), "applied");
    assert_eq!(read_utf8(&sprites.join("hud.txt")), "AG HUD");
    assert_eq!(read_utf8(&sprites.join("xhair.spr")), "XHAIR");
    let tripmine = read_latin1(&sprites.join("weapon_tripmine.txt"));
    assert!(tripmine.contains("320hud1\t160\t80"), "иконка мины из WON");
    assert!(
        tripmine.contains("crosshair\t\t640 xhair"),
        "прицел из пака"
    );
    assert!(!tripmine.contains("crosshairs\t48"), "старый прицел убран");

    // Смена прицела: файлы прошлого пака уходят, оригинал мины возвращается.
    rig.apply(
        &root,
        &json!({"packs": {"hud": "ag-hud", "crosshair": "crosshair-changer", "changerShape": "1px", "changerColor": "red"}}),
    )
    .await;
    assert_eq!(read_utf8(&sprites.join("crosshairs.spr")), "RED-1PX");
    assert!(!sprites.join("xhair.spr").exists());
    assert_eq!(
        std::fs::read(sprites.join("weapon_tripmine.txt")).unwrap(),
        tripmine_before
    );

    let off = rig.apply(&root, &json!({"packs": {}})).await;
    assert_eq!(off.status(), "default");
    assert_eq!(names(&sprites), ["weapon_tripmine.txt"]);
    assert_eq!(
        std::fs::read(sprites.join("weapon_tripmine.txt")).unwrap(),
        tripmine_before
    );

    // Скачанные архивы лежат в кеше: повторный выбор не качает заново.
    let before = rig.downloads();
    rig.apply(&root, &json!({"packs": {"hud": "ag-hud"}})).await;
    assert_eq!(rig.downloads(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pack_replaced_on_drive_is_not_installed_and_does_not_touch_earlier_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("game");
    let sprites = pack_install(&root);
    let rig = Rig::new().await;
    rig.apply(&root, &json!({"packs": {"hud": "ag-hud"}})).await;

    let tampered: Catalogue = rig
        .catalogue
        .iter()
        .map(|(id, pack)| {
            let mut pack = pack.clone();
            if id == "crosshair-changer" {
                pack.sha256 = sha("другой файл".as_bytes());
            }
            (id.clone(), pack)
        })
        .collect();
    let result = rig
        .apply_with(
            &root,
            &json!({"packs": {"hud": "ag-hud", "crosshair": "crosshair-changer"}}),
            &tampered,
        )
        .await;
    let Outcome::Failed { code, .. } = &result else {
        panic!("ожидался отказ, а не {result:?}");
    };
    assert_eq!(code.as_str(), "CHECKSUM_MISMATCH");
    assert_eq!(result.status(), "failed");
    assert_eq!(
        read_utf8(&sprites.join("hud.txt")),
        "AG HUD",
        "прошлый HUD остался"
    );
    assert!(!sprites.join("crosshairs.spr").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_changed_by_the_build_after_the_pack_stays_new() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("game");
    let sprites = pack_install(&root);
    let rig = Rig::new().await;
    rig.apply(
        &root,
        &json!({"packs": {"crosshair": "execut4ble-crosshair"}}),
    )
    .await;

    write(&sprites.join("weapon_tripmine.txt"), "NEW BUILD");
    rig.apply(&root, &json!({"packs": {}})).await;
    assert_eq!(read_utf8(&sprites.join("weapon_tripmine.txt")), "NEW BUILD");
}
