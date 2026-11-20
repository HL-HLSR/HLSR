//! Самотесты 0.4.7 для установки, перенесённые в Rust:
//! `scripts/delivery-errors-selftest.js`, тест 2 (чтение скачанного архива: вместо «unknown
//! error» — понятная ошибка с именем файла) и `scripts/linux-selftest.js` («a moved or copied
//! library launches the game from where installed.json actually lives»).
//!
//! В самотесте 0.4.7 ошибку чтения архива отдаёт подменённый `fs.createReadStream`; здесь — та
//! же ошибка Node (`UNKNOWN`, `open`, путь), переведённая так же, как её переводит `hashFile`.

use hlsr_core::js::Platform;
use hlsr_delivery::errors::{Code, Failure, RawError, to_delivery_error_on};
use hlsr_delivery::install_service::{InstallOptions, InstallService, hash_read_error};
use hlsr_delivery::paths::{game_install_path, resolve_library};
use serde_json::json;
use std::path::Path;

const ARCHIVE: &str = "C:\\Games\\Downloads\\hl1-won\\hl1-won-1.0.0.7z";

/// Так Node отдаёт ERROR_VIRUS_INFECTED: libuv этот код не знает.
fn unknown_open() -> RawError {
    RawError {
        code: Some("UNKNOWN".to_owned()),
        syscall: Some("open".to_owned()),
        path: Some(ARCHIVE.to_owned()),
        message: format!("UNKNOWN: unknown error, open '{ARCHIVE}'"),
        stderr: None,
    }
}

#[test]
fn reading_downloaded_archive_reports_antivirus_block_with_file_name() {
    let error = hash_read_error(Path::new(ARCHIVE), &unknown_open(), Platform::Win32);
    assert_eq!(error.code, Code::AntivirusBlocked);
    assert!(
        error.message.contains("«hl1-won-1.0.0.7z»"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("исключения антивируса"),
        "{}",
        error.message
    );
    assert_eq!(error.details.as_ref().unwrap()["path"], json!(ARCHIVE));

    assert_eq!(
        to_delivery_error_on(Platform::Win32, Failure::Raw(unknown_open()), None).code,
        Code::AntivirusBlocked,
        "toDeliveryError не узнал блокировку"
    );
    assert_eq!(
        to_delivery_error_on(Platform::Win32, Failure::Raw(RawError::new("boom")), None).code,
        Code::InstallFailed
    );
}

#[test]
fn other_read_errors_name_the_archive_read() {
    // Тот же UNKNOWN в Linux — не антивирус; занятый файл — тоже.
    let error = hash_read_error(Path::new(ARCHIVE), &unknown_open(), Platform::Linux);
    assert_eq!(error.code, Code::InstallFailed);
    assert_eq!(
        error.message,
        format!("Не удалось прочитать архив: UNKNOWN: unknown error, open '{ARCHIVE}'")
    );
}

#[test]
fn moved_library_launches_game_from_where_installed_json_lives() {
    let root = tempfile::tempdir().unwrap();
    let paths = resolve_library(&root.path().to_string_lossy()).unwrap();
    let here = game_install_path(&paths, "ghosting-renewed").unwrap();
    std::fs::create_dir_all(&here).unwrap();
    std::fs::write(
        here.join("installed.json"),
        json!({
            "schemaVersion": 1, "gameId": "ghosting-renewed", "version": "1.0.10", "state": "ready",
            "installPath": "/old/library/Games/ghosting-renewed"
        })
        .to_string(),
    )
    .unwrap();
    let installed = InstallService::new(paths, InstallOptions::default())
        .read_installed("ghosting-renewed")
        .unwrap()
        .unwrap();
    assert_eq!(installed["installPath"], json!(here.to_string_lossy()));
}
