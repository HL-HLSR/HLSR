//! Уборка следов 0.4.7–0.4.9 (Р12, карта §0.10) на снимках каталогов, какими их оставляет 0.4.7,
//! 0.4.8 или 0.4.9 после перехода (следы у них те же): `%TEMP%`, `%LOCALAPPDATA%`, `%APPDATA%` и
//! `~/.config/hlsr` во временных корнях. Что удалено и что осталось — снимком дерева; профиль
//! Chromium лаунчера (`Local Storage`, `Local State`, `Cookies`), `online-session.json`, данные 0.4.9
//! (`demo-tool.json`, `launch-days.json`, `demo-jobs`) и версии `0.4.x-*` остаются.

mod support;

use hlsr_update::legacy_traces::{LegacyDirs, cleanup_legacy_traces, targets};
#[cfg(windows)]
use hlsr_update::legacy_traces::{SETUP_MARK, cleanup_on_launch};
use hlsr_update::ports::MemoryLogger;
use serde_json::Value;
use std::fs;
use std::path::Path;
use support::tree::snapshot;

fn file(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn paths(tree: &Value) -> Vec<String> {
    tree.as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap().to_owned())
        .collect()
}

/// Как выглядит профиль после перехода 0.4.7–0.4.9 → 0.5 на Windows.
fn windows_tree(root: &Path) {
    // Копии внешнего апдейтера Electron: свежая (только что обновила) и старая.
    file(
        root,
        "Temp/HLSR-Updater-mgx4z2a1-0123456789ab/HLSRUpdater.exe",
        "electron",
    );
    file(
        root,
        "Temp/HLSR-Updater-mgx4z2a1-0123456789ab/resources/app.asar",
        "asar",
    );
    file(
        root,
        "Temp/HLSR-Updater-mgx4z2a1-0123456789ab/update-plan.json",
        "{}",
    );
    file(
        root,
        "Temp/HLSR-Updater-lq0aaaaa-abcdefabcdef/work/HLSR-Setup-0.4.6.exe",
        "old",
    );
    // Похожие имена, которые не копии апдейтера, и чужое.
    file(
        root,
        "Temp/HLSR-Updater-mgx4z2a1-0123456789ab.log",
        "file, not a stage",
    );
    file(
        root,
        "Temp/HLSR-Updater-latest/readme.txt",
        "not the 0.4.7 pattern",
    );
    file(
        root,
        "Temp/HLSR-Updater-ABC-0123456789ab/x",
        "upper case — not 0.4.7",
    );
    file(root, "Temp/hlsr-diag-1/report.txt", "other");
    // Кеш electron-updater и профиль Chromium апдейтера.
    file(
        root,
        "Local/hlsr-updater/pending/HLSR-Setup-0.5.0.exe",
        "installer",
    );
    file(root, "Local/hlsr-updater/installer.exe", "installer");
    file(root, "Roaming/HLSR Updater/Local State", "{}");
    file(
        root,
        "Roaming/HLSR Updater/Local Storage/leveldb/000003.log",
        "ls",
    );
    // Остаётся: журналы, данные и профиль Chromium лаунчера, апдейтер 0.5.
    file(root, "Local/hlsr/logs/main.log", "log");
    // `%LOCALAPPDATA%\HLSR\logs` апдейтера 0.5 — тот же каталог без учёта регистра.
    file(root, "Local/hlsr/logs/updater.log", "log");
    file(root, "Roaming/hlsr/Local Storage/leveldb/000003.log", "ls");
    file(root, "Roaming/hlsr/Local State", "{}");
    file(root, "Roaming/hlsr/Cookies", "c");
    file(root, "Roaming/hlsr/online-session.json", "{}");
    // Кеши Chromium лаунчера 0.4.x — следы; `Network` (куки) и `Session Storage` — нет.
    file(root, "Roaming/hlsr/Cache/Cache_Data/data_0", "cache");
    file(root, "Roaming/hlsr/Code Cache/js/index", "cache");
    file(root, "Roaming/hlsr/GPUCache/data_0", "cache");
    file(root, "Roaming/hlsr/Network/Cookies", "c");
    file(root, "Roaming/hlsr/version-state.json", "{}");
    file(root, "Roaming/hlsr/settings.json", "{}");
    file(
        root,
        "Roaming/hlsr/installer/keep.txt",
        "Windows: installer — не след 0.4.7",
    );
    // Данные 0.4.9, которые читает новая версия: не следы.
    file(root, "Roaming/hlsr/demo-tool.json", "{}");
    file(root, "Roaming/hlsr/launch-days.json", "{}");
    file(root, "Local/hlsr/demo-jobs/job-1/registry.json", "{}");
}

#[test]
fn windows_traces_go_user_data_stays() {
    let work = tempfile::tempdir().unwrap();
    let root = work.path();
    windows_tree(root);
    let dirs = LegacyDirs {
        chromium_profile: Some(root.join("Roaming").join("hlsr")),
        ..LegacyDirs::windows(
            root.join("Temp"),
            Some(root.join("Local").join("hlsr-updater")),
            Some(root.join("Roaming")),
        )
    };
    let found: Vec<String> = targets(&dirs)
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    assert_eq!(
        found,
        [
            "Temp/HLSR-Updater-lq0aaaaa-abcdefabcdef",
            "Temp/HLSR-Updater-mgx4z2a1-0123456789ab",
            "Local/hlsr-updater",
            "Roaming/HLSR Updater",
            "Roaming/hlsr/Cache",
            "Roaming/hlsr/Code Cache",
            "Roaming/hlsr/GPUCache",
        ]
    );

    let log = MemoryLogger::default();
    let cleaned = cleanup_legacy_traces(&dirs, &log);
    assert_eq!(cleaned.removed.len(), 7, "{cleaned:?}");
    assert!(cleaned.failed.is_empty(), "{cleaned:?}");
    let lines = log.take();
    assert_eq!(lines.len(), 7, "{lines:?}");
    assert!(
        lines.iter().all(|(level, line)| *level == "info"
            && line.starts_with("[update] следы 0.4.7–0.4.9: убран"))
    );

    assert_eq!(
        paths(&snapshot(root, None)),
        [
            "Local",
            "Local/hlsr",
            "Local/hlsr/demo-jobs",
            "Local/hlsr/demo-jobs/job-1",
            "Local/hlsr/demo-jobs/job-1/registry.json",
            "Local/hlsr/logs",
            "Local/hlsr/logs/main.log",
            "Local/hlsr/logs/updater.log",
            "Roaming",
            "Roaming/hlsr",
            "Roaming/hlsr/Cookies",
            "Roaming/hlsr/Local State",
            "Roaming/hlsr/Local Storage",
            "Roaming/hlsr/Local Storage/leveldb",
            "Roaming/hlsr/Local Storage/leveldb/000003.log",
            "Roaming/hlsr/Network",
            "Roaming/hlsr/Network/Cookies",
            "Roaming/hlsr/demo-tool.json",
            "Roaming/hlsr/installer",
            "Roaming/hlsr/installer/keep.txt",
            "Roaming/hlsr/launch-days.json",
            "Roaming/hlsr/online-session.json",
            "Roaming/hlsr/settings.json",
            "Roaming/hlsr/version-state.json",
            "Temp",
            "Temp/HLSR-Updater-ABC-0123456789ab",
            "Temp/HLSR-Updater-ABC-0123456789ab/x",
            "Temp/HLSR-Updater-latest",
            "Temp/HLSR-Updater-latest/readme.txt",
            "Temp/HLSR-Updater-mgx4z2a1-0123456789ab.log",
            "Temp/hlsr-diag-1",
            "Temp/hlsr-diag-1/report.txt",
        ]
    );

    // Второй запуск: убирать нечего, журнал молчит.
    let again = MemoryLogger::default();
    assert_eq!(cleanup_legacy_traces(&dirs, &again), Default::default());
    assert!(again.take().is_empty());
}

#[test]
fn linux_installer_profile_goes_versions_stay() {
    let work = tempfile::tempdir().unwrap();
    let root = work.path();
    file(root, ".config/hlsr/installer/Local State", "{}");
    file(root, ".config/hlsr/installer/Cache/data_0", "cache");
    file(root, ".config/hlsr/GPUCache/data_0", "cache");
    file(root, ".config/hlsr/DawnCache/data_0", "cache");
    file(root, ".config/hlsr/Local Storage/leveldb/000003.log", "ls");
    file(root, ".config/hlsr/Local State", "{}");
    file(root, ".config/hlsr/Cookies", "c");
    file(root, ".config/hlsr/online-session.json", "{}");
    file(root, ".config/hlsr/library.json", "{}");
    file(root, ".local/opt/hlsr/versions/0.4.7-1a2b3c/hlsr", "elf");
    file(root, ".local/opt/hlsr/versions/0.4.9-4d5e6f/hlsr", "elf");
    file(root, ".local/opt/hlsr/versions/0.5.0/hlsr", "elf");

    let dirs = LegacyDirs::linux(root.join(".config/hlsr"));
    let log = MemoryLogger::default();
    let cleaned = cleanup_legacy_traces(&dirs, &log);
    let data = root.join(".config/hlsr");
    assert_eq!(
        cleaned.removed,
        [
            data.join("installer"),
            data.join("DawnCache"),
            data.join("GPUCache")
        ]
    );

    assert_eq!(
        paths(&snapshot(root, None)),
        [
            ".config",
            ".config/hlsr",
            ".config/hlsr/Cookies",
            ".config/hlsr/Local State",
            ".config/hlsr/Local Storage",
            ".config/hlsr/Local Storage/leveldb",
            ".config/hlsr/Local Storage/leveldb/000003.log",
            ".config/hlsr/library.json",
            ".config/hlsr/online-session.json",
            ".local",
            ".local/opt",
            ".local/opt/hlsr",
            ".local/opt/hlsr/versions",
            ".local/opt/hlsr/versions/0.4.7-1a2b3c",
            ".local/opt/hlsr/versions/0.4.7-1a2b3c/hlsr",
            ".local/opt/hlsr/versions/0.4.9-4d5e6f",
            ".local/opt/hlsr/versions/0.4.9-4d5e6f/hlsr",
            ".local/opt/hlsr/versions/0.5.0",
            ".local/opt/hlsr/versions/0.5.0/hlsr",
        ]
    );
}

/// QA-сборка убирает свой кеш electron-updater (`hlsr-qa-updater`), кеш боевого лаунчера
/// (`hlsr-updater`, там может лежать его загрузка пути А) остаётся.
#[test]
fn qa_build_removes_only_its_updater_cache() {
    let work = tempfile::tempdir().unwrap();
    let root = work.path();
    file(root, "Local/hlsr-qa-updater/installer.exe", "installer");
    file(
        root,
        "Local/hlsr-updater/pending/HLSR-Setup-0.5.1.exe",
        "installer",
    );
    let dirs = LegacyDirs::windows(
        root.join("Temp"),
        Some(root.join("Local").join("hlsr-qa-updater")),
        None,
    );
    let cleaned = cleanup_legacy_traces(&dirs, &MemoryLogger::default());
    assert_eq!(
        cleaned.removed,
        [root.join("Local").join("hlsr-qa-updater")]
    );
    assert_eq!(
        paths(&snapshot(root, None)),
        [
            "Local",
            "Local/hlsr-updater",
            "Local/hlsr-updater/pending",
            "Local/hlsr-updater/pending/HLSR-Setup-0.5.1.exe",
        ]
    );
}

#[test]
fn missing_dirs_are_skipped() {
    let work = tempfile::tempdir().unwrap();
    let root = work.path();
    let dirs = LegacyDirs::windows(root.join("no-temp"), None, Some(root.join("no-roaming")));
    let log = MemoryLogger::default();
    assert_eq!(cleanup_legacy_traces(&dirs, &log), Default::default());
    assert!(log.take().is_empty());
}

/// Занятый каталог (ещё работающая копия апдейтера держит открытый файл) — строка журнала,
/// остальные следы всё равно убираются.
#[cfg(windows)]
#[test]
fn busy_stage_is_logged_and_others_are_removed() {
    use std::os::windows::fs::OpenOptionsExt;
    let work = tempfile::tempdir().unwrap();
    let root = work.path();
    file(
        root,
        "Temp/HLSR-Updater-mgx4z2a1-0123456789ab/HLSRUpdater.exe",
        "electron",
    );
    file(root, "Local/hlsr-updater/pending/x.exe", "installer");
    // Открыт без FILE_SHARE_DELETE — как исполняемый файл запущенного процесса.
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(root.join("Temp/HLSR-Updater-mgx4z2a1-0123456789ab/HLSRUpdater.exe"))
        .unwrap();
    let dirs = LegacyDirs::windows(
        root.join("Temp"),
        Some(root.join("Local").join("hlsr-updater")),
        None,
    );
    let log = MemoryLogger::default();
    let cleaned = cleanup_legacy_traces(&dirs, &log);
    drop(held);
    assert_eq!(cleaned.removed, [root.join("Local").join("hlsr-updater")]);
    assert_eq!(cleaned.failed.len(), 1, "{cleaned:?}");
    let lines = log.take();
    assert!(
        lines
            .iter()
            .any(|(level, line)| *level == "warn" && line.contains("не убран")),
        "{lines:?}"
    );
}

/// Запуск новой версии: без квитанции и метки следы не трогаются; по метке установщика — убираются,
/// а метка остаётся, пока занятый след не убран, и снимается, когда убрано всё.
#[cfg(windows)]
#[test]
fn launch_cleans_by_mark_and_keeps_it_until_everything_is_gone() {
    use std::os::windows::fs::OpenOptionsExt;
    let work = tempfile::tempdir().unwrap();
    let root = work.path();
    file(root, "hlsr/GPUCache/data_0", "cache");
    file(root, "hlsr/Cache/data_0", "cache");
    let mark = root.join("hlsr").join(SETUP_MARK);
    let dirs = LegacyDirs {
        chromium_profile: Some(root.join("hlsr")),
        ..LegacyDirs::default()
    };
    let log = MemoryLogger::default();
    assert_eq!(cleanup_on_launch(&dirs, false, &mark, &log), None);
    assert!(
        root.join("hlsr/GPUCache").is_dir(),
        "без квитанции и метки — не трогается"
    );

    fs::write(&mark, b"").unwrap();
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(root.join("hlsr/GPUCache/data_0"))
        .unwrap();
    let cleaned = cleanup_on_launch(&dirs, false, &mark, &log).expect("по метке");
    drop(held);
    assert_eq!(cleaned.removed, [root.join("hlsr/Cache")]);
    assert_eq!(cleaned.failed.len(), 1, "{cleaned:?}");
    assert!(mark.is_file(), "занятый след — метка остаётся");

    let cleaned = cleanup_on_launch(&dirs, false, &mark, &log).expect("по метке");
    assert_eq!(cleaned.removed, [root.join("hlsr/GPUCache")]);
    assert!(!mark.exists(), "убрано всё — метка снята");
    assert_eq!(cleanup_on_launch(&dirs, false, &mark, &log), None);
    assert!(
        cleanup_on_launch(&dirs, true, &mark, &log).is_some(),
        "по квитанции"
    );
}
