//! Самотесты 0.4.7, относящиеся к основе доставки, с теми же случаями:
//! `scripts/delivery-errors-selftest.js` (тест 1 и чистая часть теста 2),
//! `scripts/linux-selftest.js` («Linux 7za: Dn»), `scripts/user-configs-selftest.js`
//! (`keptByPatch`/`isUserEdited`; части `PatchService` и `IntegrityService` — в их задачах).

use hlsr_core::js::Platform;
use hlsr_delivery::archive_entries::parse_listing;
use hlsr_delivery::errors::{Code, Failure, RawError, is_antivirus_block, to_delivery_error_on};
use hlsr_delivery::user_configs::kept_by_patch;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

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

fn stderr(text: &str) -> RawError {
    RawError {
        stderr: Some(text.to_owned()),
        ..RawError::default()
    }
}

#[test]
fn unknown_on_open_in_windows_is_antivirus_block() {
    assert!(is_antivirus_block(&unknown_open(), Platform::Win32));
    assert!(
        !is_antivirus_block(&unknown_open(), Platform::Linux),
        "на Linux UNKNOWN — не антивирус"
    );
    let busy = RawError {
        code: Some("EBUSY".to_owned()),
        syscall: Some("open".to_owned()),
        message: "EBUSY".to_owned(),
        ..RawError::default()
    };
    assert!(
        !is_antivirus_block(&busy, Platform::Win32),
        "занятый файл — не антивирус"
    );
    assert!(is_antivirus_block(
        &stderr(
            "ERROR: hl1-won.7z\nOperation did not complete successfully because the file contains a virus or potentially unwanted software."
        ),
        Platform::Win32
    ));
    assert!(is_antivirus_block(
        &stderr(
            "Операция не была успешно завершена, так как файл содержит вирус или потенциально нежелательную программу."
        ),
        Platform::Win32
    ));
    assert!(
        !is_antivirus_block(
            &stderr("ERROR: Data Error : hl1/valve/pak0.pak"),
            Platform::Win32
        ),
        "битый архив — не антивирус"
    );
}

#[test]
fn reading_downloaded_archive_gives_clear_error_with_file_name() {
    let error = to_delivery_error_on(Platform::Win32, Failure::Raw(unknown_open()), None);
    assert_eq!(
        error.code,
        Code::AntivirusBlocked,
        "toDeliveryError не узнал блокировку"
    );
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
    assert_eq!(error.details.as_ref().unwrap()["path"], ARCHIVE);
    let boom = to_delivery_error_on(Platform::Win32, Failure::Raw(RawError::new("boom")), None);
    assert_eq!(boom.code, Code::InstallFailed);
}

#[test]
fn linux_7za_payload_folder_with_dn_attribute_is_a_directory() {
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
    assert!(!listing.entries[2].is_directory);
}

fn sha(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn entry(relative: &str, text: &str) -> Value {
    json!({"path": relative, "size": text.len(), "sha256": sha(text)})
}

#[test]
fn patch_keeps_player_edited_configs_and_updates_stock_ones() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for (relative, text) in [
        ("mod/stock.cfg", "старый\n"),
        ("mod/edited.cfg", "мой бинд\n"),
        ("mod/own.cfg", "мой файл\n"),
        ("mod/gone.cfg", "удалить\n"),
        ("mod/gone-edited.cfg", "удалить, но я менял\n"),
        ("mod/same.cfg", "новый\n"),
        ("mod/client.dll", "чужой бинарь"),
    ] {
        let full = root.join(relative);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    let previous = json!({"files": [
        entry("mod/stock.cfg", "старый\n"),
        entry("mod/edited.cfg", "стоковый\n"),
        entry("mod/gone.cfg", "удалить\n"),
        entry("mod/gone-edited.cfg", "удалить\n"),
        entry("mod/same.cfg", "старый\n"),
        entry("mod/client.dll", "бинарь"),
    ]});
    let patch = json!({
        "fromVersion": "1.0.0",
        "files": [
            entry("mod/stock.cfg", "новый\n"),
            entry("mod/edited.cfg", "новый\n"),
            entry("mod/own.cfg", "из сборки\n"),
            entry("mod/same.cfg", "новый\n"),
            entry("mod/missing.cfg", "новый\n"),
            entry("mod/client.dll", "новый бинарь"),
        ],
        "deletedFiles": ["mod/gone.cfg", "mod/gone-edited.cfg"],
    });

    let mut kept = kept_by_patch(root, &patch, Some(&previous));
    kept.sort();
    assert_eq!(
        kept,
        ["mod/edited.cfg", "mod/gone-edited.cfg", "mod/own.cfg"]
    );
    assert!(
        kept_by_patch(root, &patch, None).is_empty(),
        "без прежней версии ничего не оставляется"
    );
}
