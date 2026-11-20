//! 7-Zip 0.4.9: самотест `seven-zip-selftest.js` (на Windows — 7-Zip 26.05 из `resources/7zip`,
//! архив с паролем не вешает чтение) и новое в `ArchiveService.js`: `extract(…, {files})` —
//! `7za e` только с выбранными файлами, `create(…, {type: "zip"})` — zip без блока шапки.

use hlsr_delivery::archive_service::{ArchiveService, CreateOptions, ExtractOptions};
use hlsr_delivery::ports::MemoryLogger;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn seven_zip() -> std::path::PathBuf {
    hlsr_core::paths::Resources::detect().sevenzip()
}

fn service() -> ArchiveService {
    ArchiveService::new(seven_zip(), Arc::new(MemoryLogger::default()))
}

fn put(root: &Path, relative: &str, text: &str) {
    let file = root.join(relative);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

#[cfg(windows)]
#[test]
fn windows_runs_7zip_26_05_from_resources() {
    let bin = seven_zip();
    assert!(bin.ends_with("7zip/win/x64/7za.exe"), "{}", bin.display());
    let out = std::process::Command::new(&bin).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("7-Zip (a) 26.05"), "{text}");
    assert!(bin.with_file_name("License.txt").is_file());
}

/// 7-Zip не ждёт пароль: чтение зашифрованного архива отвечает, а не висит.
#[tokio::test]
async fn password_archive_does_not_hang_reading() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("secret.7z");
    put(dir.path(), "note.json", "{}");
    let made = std::process::Command::new(seven_zip())
        .args(["a", "-psecret", "-mhe=on"])
        .arg(&file)
        .arg(dir.path().join("note.json"))
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    let archive = service();
    let limit = Duration::from_secs(15);
    assert!(
        tokio::time::timeout(limit, archive.read_text(&file, "note.json", Some(1024)))
            .await
            .is_ok(),
        "readText завис"
    );
    assert!(
        tokio::time::timeout(limit, archive.list(&file))
            .await
            .is_ok(),
        "list завис"
    );
}

/// `extract(…, {files})`: только эти файлы, плоско в корень; пустой список — весь архив.
#[tokio::test]
async fn extract_only_takes_listed_files_flat() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("src");
    put(&source, "pkg/bin/a.exe", "a");
    put(&source, "pkg/bin/b.exe", "b");
    put(&source, "pkg/doc/readme.txt", "doc");
    let zip = dir.path().join("pkg.zip");
    let archive = service();
    archive
        .create_zip(
            &zip,
            CreateOptions {
                cwd: Some(source),
                files: vec!["pkg".to_owned()],
                // У zip блока шапки нет: head_files не пакуются.
                head_files: vec!["pkg/doc/readme.txt".to_owned()],
                ..CreateOptions::default()
            },
        )
        .await
        .unwrap();
    let listing = archive.list(&zip).await.unwrap();
    assert!(
        listing
            .archive
            .iter()
            .any(|(key, value)| key == "Type" && value == "zip"),
        "{:?}",
        listing.archive
    );
    assert!(!listing.entries.iter().any(|e| e.path == "readme.txt"));

    let flat = dir.path().join("flat");
    archive
        .extract_only(
            &zip,
            &flat,
            &["pkg/bin/a.exe".to_owned(), "pkg/bin/b.exe".to_owned()],
            ExtractOptions::default(),
        )
        .await
        .unwrap();
    let mut names: Vec<String> = std::fs::read_dir(&flat)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["a.exe", "b.exe"]);
    assert_eq!(std::fs::read_to_string(flat.join("b.exe")).unwrap(), "b");

    let full = dir.path().join("full");
    archive
        .extract_only(&zip, &full, &[], ExtractOptions::default())
        .await
        .unwrap();
    assert!(full.join("pkg/doc/readme.txt").is_file());
}
