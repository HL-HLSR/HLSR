//! `steam_appid.txt` рядом с `HLSR.exe` (`src/main/index.js:213-240` 0.4.7): файл SteamVR
//! (250820) удаляется — и с BOM, и с нулями и пробелами в конце; другой AppID остаётся; ошибка —
//! строка журнала.

use hlsr_core::logger::MemoryLogger;
use hlsr_platform::legacy_app_id::remove_legacy_steam_app_id;

#[test]
fn steamvr_app_id_next_to_exe_is_removed() {
    for content in [
        &b"250820"[..],
        b"250820\n",
        b"\xEF\xBB\xBF250820\r\n",
        b"250820\0\0",
        b"  250820\t\0",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("steam_appid.txt");
        std::fs::write(&file, content).unwrap();
        let log = MemoryLogger::default();
        assert!(remove_legacy_steam_app_id(dir.path(), &log), "{content:?}");
        assert!(!file.exists());
        assert_eq!(
            log.take(),
            [(
                "info",
                "Removed legacy steam_appid.txt (SteamVR AppID 250820) next to HLSR.exe".to_owned()
            )]
        );
    }
}

#[test]
fn other_app_id_is_left_untouched() {
    for content in [&b"243730\n\0"[..], b"2508200", b"", b"250820 x"] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("steam_appid.txt");
        std::fs::write(&file, content).unwrap();
        let log = MemoryLogger::default();
        assert!(!remove_legacy_steam_app_id(dir.path(), &log));
        assert_eq!(std::fs::read(&file).unwrap(), content);
        assert_eq!(
            log.take(),
            [(
                "warn",
                "steam_appid.txt next to HLSR.exe has unexpected content, leaving it untouched"
                    .to_owned()
            )]
        );
    }
}

#[test]
fn missing_file_is_silent_and_unreadable_one_is_logged() {
    let dir = tempfile::tempdir().unwrap();
    let log = MemoryLogger::default();
    assert!(!remove_legacy_steam_app_id(dir.path(), &log));
    assert!(log.take().is_empty());

    // Каталог с этим именем: `existsSync` — да, чтение — ошибка.
    std::fs::create_dir(dir.path().join("steam_appid.txt")).unwrap();
    assert!(!remove_legacy_steam_app_id(dir.path(), &log));
    let lines = log.take();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].0, "warn");
    assert!(
        lines[0]
            .1
            .starts_with("Legacy steam_appid.txt migration failed: "),
        "{}",
        lines[0].1
    );
    assert!(dir.path().join("steam_appid.txt").is_dir());
}
