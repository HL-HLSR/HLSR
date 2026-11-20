//! Поиск Steam на настоящих файлах и настоящих процессах Windows: «Steam» — временная копия
//! `ping` под своим именем (процесс, который тест запустил сам), реестр — поддельный,
//! библиотеки и Source SDK Base — во временных папках. Настоящие реестр, Steam и папки Steam
//! владельца не читаются; имя образа не `steam.exe`, поэтому чужие процессы Steam не попадают
//! ни в поиск, ни в сверку.

#![cfg(windows)]

mod support;

use hlsr_delivery::ports::{FakeRegistry, Logger, MemoryLogger, RegistryHive, RegistryValue};
use hlsr_platform::steam_account::{AccountOptions, SteamAccount};
use hlsr_platform::steam_locator::{LocatorOptions, SteamLocating, SteamLocator};
use std::path::Path;
use std::sync::Arc;
use support::sleeper::Sleeper;

const STEAM_KEY: &str = "Software\\Valve\\Steam";
const ACTIVE_KEY: &str = "Software\\Valve\\Steam\\ActiveProcess";

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Source SDK Base в `library` (appmanifest и три папки с `hl2.exe`).
fn write_sdk(library: &Path) {
    let apps = library.join("steamapps");
    let app = apps.join("common").join("Source SDK Base 2013 Multiplayer");
    std::fs::create_dir_all(app.join("hl2")).unwrap();
    std::fs::create_dir_all(app.join("bin")).unwrap();
    std::fs::create_dir_all(app.join("platform")).unwrap();
    std::fs::write(app.join("hl2.exe"), "").unwrap();
    std::fs::write(
        apps.join("appmanifest_243730.acf"),
        "\"AppState\"\n{\n\t\"appid\"\t\t\"243730\"\n\t\"installdir\"\t\t\"Source SDK Base 2013 Multiplayer\"\n}\n",
    )
    .unwrap();
}

struct World {
    sleeper: Sleeper,
    library: tempfile::TempDir,
    registry: Arc<FakeRegistry>,
    logger: Arc<MemoryLogger>,
    locator: Arc<SteamLocator>,
    account: SteamAccount,
}

fn world() -> World {
    let sleeper = Sleeper::start(60);
    let steam_root = sleeper.root();
    let library = tempfile::tempdir().unwrap();
    write_sdk(library.path());

    // libraryfolders.vdf: вторая библиотека (пути в VDF пишутся с удвоенными обратными чертами).
    std::fs::create_dir_all(steam_root.join("steamapps")).unwrap();
    std::fs::write(
        steam_root.join("steamapps").join("libraryfolders.vdf"),
        format!(
            "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
            text(library.path()).replace('\\', "\\\\")
        ),
    )
    .unwrap();

    let registry = Arc::new(FakeRegistry::default());
    let set = |key: &str, name: &str, value: RegistryValue| {
        registry.set(RegistryHive::CurrentUser, key, name, value);
    };
    set(
        STEAM_KEY,
        "SteamPath",
        RegistryValue::Sz(text(&steam_root).replace('\\', "/")),
    );
    set(
        STEAM_KEY,
        "SteamExe",
        RegistryValue::Sz(text(&sleeper.exe).replace('\\', "/")),
    );
    set(ACTIVE_KEY, "pid", RegistryValue::Dword(sleeper.pid()));
    set(ACTIVE_KEY, "ActiveUser", RegistryValue::Dword(42));

    let logger = Arc::new(MemoryLogger::default());
    let locator = Arc::new(SteamLocator::new(LocatorOptions {
        registry: Some(registry.clone()),
        logger: Some(logger.clone() as Arc<dyn Logger>),
        ..LocatorOptions::default()
    }));
    let account = SteamAccount::new(
        locator.clone() as Arc<dyn SteamLocating>,
        AccountOptions {
            registry: Some(registry.clone()),
            logger: Some(logger.clone() as Arc<dyn Logger>),
            ..AccountOptions::default()
        },
    );
    World {
        sleeper,
        library,
        registry,
        logger,
        locator,
        account,
    }
}

#[test]
fn preflight_succeeds_on_real_files_and_a_real_process() {
    let w = world();
    let result = w
        .locator
        .preflight(None)
        .expect("Steam запущен, SDK на месте");

    assert!(
        hlsr_platform::process_inspect::same_path(
            Some(&result.steam.steam_exe),
            w.sleeper.exe.to_str()
        ),
        "{}",
        result.steam.steam_exe
    );
    assert_eq!(result.running.pid, w.sleeper.pid());
    assert_eq!(result.running.source.as_deref(), Some("registry"));
    assert!(result.running.started_at.is_some(), "время старта известно");
    assert_eq!(result.app.app_id, 243730);
    assert!(
        hlsr_platform::process_inspect::same_path(
            Some(&result.app.library),
            w.library.path().to_str()
        ),
        "{}",
        result.app.library
    );
    assert_eq!(result.app.installdir, "Source SDK Base 2013 Multiplayer");

    let availability = w.locator.describe_availability(None);
    assert_eq!(availability["ok"], true);
    assert_eq!(availability["steam"]["source"], "registry");
    assert_eq!(w.locator.describe_steam_only()["ok"], true);
    assert!(w.logger.take().is_empty(), "без предупреждений");
}

#[test]
fn a_process_that_left_is_not_running_any_more() {
    let mut w = world();
    let steam = w.locator.locate_steam().unwrap();
    assert!(w.locator.assert_steam_running(&steam).is_ok());

    w.sleeper.kill();
    let error = w.locator.assert_steam_running(&steam).unwrap_err();
    assert_eq!(error.code.as_str(), "STEAM_NOT_RUNNING");
    assert!(error.message.contains("мёртв"), "{}", error.message);
    assert_eq!(w.account.refresh_connection(), "offline");
}

#[test]
fn a_foreign_process_under_the_registry_pid_is_not_steam() {
    let mut w = world();
    // PID другого процесса (самого теста): путь не совпадает с путём Steam из реестра.
    w.registry.set(
        RegistryHive::CurrentUser,
        ACTIVE_KEY,
        "pid",
        RegistryValue::Dword(std::process::id()),
    );
    let steam = w.locator.locate_steam().unwrap();

    // Настоящий «Steam» при этом работает: он находится по имени образа и пути.
    let found = w
        .locator
        .assert_steam_running(&steam)
        .expect("найден сканированием");
    assert_eq!(found.source.as_deref(), Some("scan"));
    assert_eq!(found.pid, w.sleeper.pid());
    let lines = w.logger.take();
    assert!(
        lines
            .iter()
            .any(|(level, text)| *level == "warn" && text.contains("не Steam")),
        "{lines:?}"
    );

    // А если его нет — это не Steam, и причина названа.
    w.sleeper.kill();
    let error = w.locator.assert_steam_running(&steam).unwrap_err();
    assert_eq!(error.code.as_str(), "STEAM_NOT_RUNNING");
    assert!(error.message.contains("не Steam"), "{}", error.message);
}

#[test]
fn account_reads_the_user_from_real_files() {
    let w = world();
    let root = w.sleeper.root();
    let config = root.join("config");
    std::fs::create_dir_all(config.join("avatarcache")).unwrap();
    // ActiveUser 42 — SteamID64 76561197960265770.
    std::fs::write(
        config.join("loginusers.vdf"),
        "\"users\"\n{\n\t\"76561197960265770\"\n\t{\n\t\t\"PersonaName\"\t\t\"  Игрок  \"\n\t}\n}\n",
    )
    .unwrap();
    let mut png = vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    png.extend_from_slice(b"image");
    std::fs::write(
        config.join("avatarcache").join("76561197960265770.png"),
        &png,
    )
    .unwrap();

    assert_eq!(w.account.refresh_connection(), "connected");
    let info = w.account.describe();
    assert!(info.signed_in);
    assert_eq!(info.steam_id64.as_deref(), Some("76561197960265770"));
    assert_eq!(info.steam_id2.as_deref(), Some("STEAM_0:0:21"));
    assert_eq!(info.persona_name.as_deref(), Some("Игрок"));
    assert_eq!(
        info.avatar_data_uri.as_deref(),
        Some("data:image/png;base64,iVBORw0KGgppbWFnZQ==")
    );
    assert_eq!(info.connection, "connected");
}
