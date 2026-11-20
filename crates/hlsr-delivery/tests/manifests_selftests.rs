//! Самотесты 0.4.7, которые относятся к манифестам: `hl1-selftest.js` (реестр, `ManifestService`,
//! вшитые файловые манифесты, изменяемые пути) и `linux-selftest.js` («Linux 7za: Dn», часть про
//! `validatePayloadEntries`). Случаи те же, проверки — те же.

use hlsr_delivery::archive_entries::{ArchiveEntry, parse_listing};
use hlsr_delivery::delivery_endpoints::ALLOWED_DOWNLOAD_HOSTS;
use hlsr_delivery::manifest_service::{InstallableOptions, ManifestService};
use hlsr_delivery::manifests;
use hlsr_delivery::mutable_paths::matches_mutable;
use hlsr_delivery::patch_format::validate_payload_entries;
use serde_json::{Value, json};
use std::collections::HashSet;
use url::Url;

fn launch_config_hl1() -> &'static manifests::LaunchConfig {
    manifests::launch_config_of("hl1-won").expect("настройки запуска HL1")
}

/// «HL1: сборка WON зарегистрирована и доступна в интерфейсе».
#[test]
fn hl1_won_is_registered() {
    let config = launch_config_hl1();
    assert!(hlsr_delivery::game_availability::is_game_available("70"));
    assert_eq!(
        manifests::for_launcher_id("70").unwrap()["gameId"],
        "hl1-won"
    );
    assert_eq!(manifests::launcher_id_of("hl1-won"), Some("70"));
    assert_eq!(
        config.requires_owned_app_ids,
        &[70],
        "без лицензии Half-Life сборка не ставится"
    );
    assert_eq!(config.engine, Some("goldsrc"));
    assert!(
        manifests::required_owned_apps()
            .iter()
            .any(|app| app.app_id == 70)
    );
}

/// «HL1: манифест проходит проверку и качается только с закреплённого хоста».
#[test]
fn hl1_won_manifest_passes_and_downloads_from_pinned_hosts_only() {
    let manifest = manifests::manifest_of("hl1-won").unwrap();
    let options = InstallableOptions {
        launcher_version: Some("0.3.16".to_owned()),
        allowed_provider_types: vec!["https-direct".to_owned()],
    };
    let check = ManifestService::new(None)
        .assert_installable(manifest, Some(&options))
        .expect("манифест HL1 проходит проверку");
    assert!(!check.providers.is_empty());
    for provider in &check.providers {
        let url = Url::parse(provider["url"].as_str().unwrap()).unwrap();
        assert!(
            ALLOWED_DOWNLOAD_HOSTS.contains(&url.host_str().unwrap()),
            "{url}"
        );
    }

    let files: HashSet<&str> = manifests::files_manifest_of("hl1-won", "1.0.0").unwrap()["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    for required in manifest["filesManifest"]["requiredFiles"]
        .as_array()
        .unwrap()
    {
        assert!(files.contains(required.as_str().unwrap()), "{required}");
    }
    let config = launch_config_hl1();
    for relative in [
        Some(config.executable),
        config.injector,
        config.steam_app_id_file,
    ] {
        let relative = relative.unwrap();
        assert!(files.contains(relative), "{relative}");
    }
}

/// «HL1: сейвы, конфиги и служебные файлы игры не считаются повреждением».
#[test]
fn hl1_saves_and_configs_are_not_damage() {
    for relative in [
        "Half-Life/valve_WON/SAVE/quick.sav",
        "Half-Life/valve_WON/config.cfg",
        "Half-Life/valve_WON/userconfig.cfg",
        "Half-Life/valve_WON/demoheader.dmf",
        "Half-Life/errorlogs/errorlog00001.txt",
        "Half-Life/steam_appid.txt",
    ] {
        assert!(matches_mutable(relative), "{relative}");
    }
    assert!(!matches_mutable("Half-Life/hl.exe"));
    assert!(!matches_mutable("Half-Life/hw.dll"));
}

fn entry(path: &str, directory: bool) -> ArchiveEntry {
    ArchiveEntry {
        path: path.to_owned(),
        size: 0.0,
        packed_size: 0.0,
        attributes: String::new(),
        modified: None,
        crc: None,
        encrypted: false,
        is_directory: directory,
        is_symlink: false,
    }
}

/// «Linux 7za: папка payload с атрибутом Dn не считается файлом вне патча» — вторая половина
/// (разбор листинга проверяет `foundation_selftests`).
#[test]
fn linux_7za_dn_directory_payload_is_not_a_file_outside_the_patch() {
    let listing = parse_listing(
        &[
            "Path = example.7z",
            "Type = 7z",
            "",
            "----------",
            "Path = payload",
            "Size = 0",
            "Attributes = Dn",
            "",
            "Path = payload/ghostingmod",
            "Size = 0",
            "Attributes = Dn",
            "",
            "Path = payload/ghostingmod/file.txt",
            "Size = 4",
            "Attributes = A",
            "",
        ]
        .join("\n"),
    );
    assert!(listing.entries[0].is_directory);
    assert!(listing.entries[1].is_directory);

    let manifest: Value = json!({"files": [{"path": "ghostingmod/file.txt"}]});
    assert!(validate_payload_entries(&manifest, &listing.entries).unwrap());

    let mut with_outside = listing.entries.clone();
    with_outside.push(entry("outside.txt", false));
    assert!(validate_payload_entries(&manifest, &with_outside).is_err());
}
