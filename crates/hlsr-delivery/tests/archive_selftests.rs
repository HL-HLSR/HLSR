//! Самотесты 0.4.7, где есть настоящий 7za: `game-pack-selftest.js` — экспорт сборки и чужой
//! архив — в той части, что относится к `ArchiveService` (список архива после `create`,
//! чтение служебного файла); часть `GameDeliveryService`/`game-pack` перенесёт своя задача.
//!
//! `practice-saves-selftest.js` («настоящий архив распаковывается 7za») читает
//! `release/saves/<архив>`, который в репозиторий не входит (в 0.4.7 тест пропускается без
//! файла), и ходит через `PracticeSaves`, которого ещё нет; распаковка настоящим 7za
//! целиком проверена в `tests/archive.rs` (сценарии `extract/*`).

use hlsr_delivery::archive_service::{ArchiveService, CreateOptions};
use hlsr_delivery::errors::Code;
use hlsr_delivery::ports::MemoryLogger;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// `META_FILE` из `game-pack.js`.
const META_FILE: &str = ".hlsr-pack.json";

fn service() -> ArchiveService {
    ArchiveService::with_default_bin(Arc::new(MemoryLogger::default()))
        .with_env([("LC_ALL", "C.UTF-8"), ("LANG", "C.UTF-8")])
}

fn write(root: &Path, relative: &str, data: &[u8]) {
    let full = relative
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part));
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, data).unwrap();
}

/// Пути записей без каталогов; в Windows 7-Zip пишет `\`.
async fn file_names(service: &ArchiveService, archive: &Path) -> Vec<String> {
    service
        .list(archive)
        .await
        .unwrap()
        .entries
        .iter()
        .filter(|entry| !entry.is_directory)
        .map(|entry| entry.path.replace('\\', "/"))
        .collect()
}

/// «экспорт: игра, сейвы и cfg в одном архиве, служебный файл первым, без installed.json»:
/// служебный файл лежит вне папки игры (путь абсолютный), `exportPack` кладёт его первым
/// блоком (`headFiles`), остальное — списком из папки игры.
#[tokio::test]
async fn export_puts_the_service_file_first_and_leaves_installed_json_out() {
    let dir = tempfile::tempdir().unwrap();
    let install = dir.path().join("install");
    let mut mz = b"MZ".to_vec();
    mz.extend([7u8; 200]);
    write(&install, "Half-Life/hl.exe", &mz);
    write(&install, "Half-Life/valve_WON/SAVE/quick.sav", b"save");
    write(&install, "Half-Life/valve_WON/config.cfg", b"name mine\n");
    let work = dir.path().join("work");
    write(&work, META_FILE, b"{}");
    let partial = dir.path().join("export.hlsrpack.hlsr-partial");

    let files: Vec<String> = [
        "Half-Life/hl.exe",
        "Half-Life/valve_WON/SAVE/quick.sav",
        "Half-Life/valve_WON/config.cfg",
    ]
    .map(str::to_owned)
    .to_vec();
    let service = service();
    service
        .create(
            &partial,
            CreateOptions {
                cwd: Some(install.clone()),
                files,
                head_files: vec![work.join(META_FILE).to_string_lossy().into_owned()],
                list_dir: Some(work.clone()),
                threads: Some(std::thread::available_parallelism().map_or(1, |n| n.get()) as f64),
                ..CreateOptions::default()
            },
        )
        .await
        .unwrap();

    let names = file_names(&service, &partial).await;
    assert_eq!(names[0], META_FILE);
    assert!(names.contains(&"Half-Life/valve_WON/SAVE/quick.sav".to_owned()));
    assert!(names.contains(&"Half-Life/hl.exe".to_owned()));
    assert!(!names.contains(&"installed.json".to_owned()));
    // Список файлов убран из папки списков.
    let left: Vec<PathBuf> = std::fs::read_dir(&work)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(left, [work.join(META_FILE)]);
}

/// «чужой архив»: архив без служебного файла. `7za e -so` на отсутствующую запись отвечает
/// кодом 0 и пустым выводом, поэтому `readText` отдаёт пустую строку; `readPackMeta` из
/// `GameDeliveryService` отвечает на неё `PACK_BAD_FORMAT`.
#[tokio::test]
async fn foreign_archive_has_no_service_file() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "loose/readme.txt", b"hi");
    let plain = dir.path().join("plain.7z");
    let service = service();
    service
        .create(
            &plain,
            CreateOptions {
                cwd: Some(dir.path().join("loose")),
                files: vec!["readme.txt".to_owned()],
                threads: Some(1.0),
                ..CreateOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(file_names(&service, &plain).await, ["readme.txt"]);
    let meta = service
        .read_text(&plain, META_FILE, Some(4 * 1024 * 1024))
        .await
        .unwrap();
    assert_eq!(meta, "");
}

/// Файл, который не архив, — `ARCHIVE_LIST_FAILED` (так `readPackMeta` узнаёт чужие файлы).
#[tokio::test]
async fn plain_file_is_not_an_archive() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "x.hlsrpack", "это не архив".as_bytes());
    let error = service()
        .read_text(&dir.path().join("x.hlsrpack"), META_FILE, Some(1024))
        .await
        .unwrap_err();
    assert_eq!(error.code, Code::ArchiveListFailed);
}
