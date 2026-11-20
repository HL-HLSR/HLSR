//! Linux на настоящей ОС: `/proc`, владелец процесса, ссылки `~/.steam/root`. Запускается только
//! в Linux (в Windows не собирается): процесс «steam» — копия `sleep` под этим именем, которую
//! тест запускает сам; домашняя папка — временная.

#![cfg(target_os = "linux")]

mod support;

use hlsr_platform::linux::path_case::find_case_conflict;
use hlsr_platform::linux::steam_locator::{LocatorOptions, SteamLocator};
use hlsr_platform::steam_fs::RealFs;
use std::os::unix::fs::symlink;
use std::process::{Command, Stdio};
use std::sync::Arc;

#[test]
fn locator_finds_steam_by_links_and_by_proc() {
    let home = tempfile::tempdir().unwrap();
    let home_path = std::fs::canonicalize(home.path()).unwrap();
    let steam = home_path.join(".local/share/Steam");
    std::fs::create_dir_all(steam.join("steamapps")).unwrap();
    std::fs::create_dir_all(home_path.join(".steam")).unwrap();
    symlink(&steam, home_path.join(".steam/root")).unwrap();

    // Процесс с именем `steam`: копия `sleep`; comm — имя файла.
    let bin = tempfile::tempdir().unwrap();
    let exe = bin.path().join("steam");
    std::fs::copy("/bin/sleep", &exe).unwrap();
    let mut child = Command::new(&exe)
        .arg("60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let locator = SteamLocator::new(LocatorOptions {
        fs: Some(Arc::new(RealFs)),
        home: Some(home_path.to_string_lossy().into_owned()),
        ..LocatorOptions::default()
    });
    let found = locator.locate_steam().unwrap();
    assert_eq!(found.install_path, steam.to_string_lossy());

    // Свой процесс `steam` найден по /proc (если у пользователя запущен и настоящий Steam,
    // вернётся любой из них: оба — процессы этого же пользователя).
    let running = locator.assert_steam_running().unwrap();
    assert_eq!(running.source.as_deref(), Some("proc"));
    assert!(running.pid > 0);

    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn case_conflict_is_found_on_a_real_tree() {
    let root = tempfile::tempdir().unwrap();
    let root_path = std::fs::canonicalize(root.path()).unwrap();
    let install = root_path.join("Games/HLSR/Games/ghosting-renewed");
    std::fs::create_dir_all(&install).unwrap();
    let fs = RealFs;
    assert_eq!(find_case_conflict(install.to_str().unwrap(), &fs), None);

    std::fs::create_dir(root_path.join("Games/hlsr")).unwrap();
    let conflict = find_case_conflict(install.to_str().unwrap(), &fs).unwrap();
    assert_eq!(
        conflict.folder,
        root_path.join("Games/HLSR").to_string_lossy()
    );
    assert_eq!(
        conflict.twin,
        root_path.join("Games/hlsr").to_string_lossy()
    );
}
