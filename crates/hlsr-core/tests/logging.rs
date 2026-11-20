use chrono::TimeZone;
use hlsr_core::logging::{EOL, LogFile, format_line, init, install_panic_hook, redact_user};
use hlsr_core::paths::Paths;
use hlsr_core::tasks::spawn_logged;
use tracing::Level;

#[test]
fn line_format_matches_electron_log() {
    let at = chrono::Local
        .with_ymd_and_hms(2026, 10, 14, 19, 43, 30)
        .unwrap()
        + chrono::Duration::milliseconds(671);
    assert_eq!(
        format_line(at, Level::INFO, Some("game-delivery"), "канал: HTTP 304"),
        format!("[2026-10-14 19:43:30.671] [info]  [game-delivery] канал: HTTP 304{EOL}")
    );
    assert_eq!(
        format_line(at, Level::ERROR, None, "x"),
        format!("[2026-10-14 19:43:30.671] [error] x{EOL}")
    );
    assert_eq!(
        format_line(at, Level::WARN, Some("launcher"), "y"),
        format!("[2026-10-14 19:43:30.671] [warn]  [launcher] y{EOL}")
    );
    assert_eq!(
        format_line(at, Level::DEBUG, None, "z"),
        format!("[2026-10-14 19:43:30.671] [debug] z{EOL}")
    );
    let early = chrono::Local.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
    assert_eq!(
        format_line(early, Level::INFO, None, ""),
        format!("[2026-01-02 03:04:05.000] [info]  {EOL}")
    );
}

#[test]
fn user_name_is_masked_in_both_separators() {
    assert_eq!(
        redact_user("C:\\Users\\Иван\\HLSR и /home/Иван/x", "Иван"),
        "C:\\Users\\***\\HLSR и /home/***/x"
    );
    assert_eq!(redact_user("/home/kanoyo2/x", "kanoyo"), "/home/kanoyo2/x");
    assert_eq!(redact_user("/a/b/", ""), "/a/b/");
    assert_eq!(redact_user("/home/a/a/a/", "a"), "/home/***/a/***/");
}

#[test]
fn rotation_keeps_three_archives() {
    let dir = tempfile::tempdir().unwrap();
    for (n, body) in [
        ("main.log", "0"),
        ("main.1.log", "1"),
        ("main.2.log", "2"),
        ("main.3.log", "3"),
    ] {
        std::fs::write(dir.path().join(n), body).unwrap();
    }
    std::fs::write(dir.path().join("main.log"), vec![b'x'; 2_097_153]).unwrap();
    let mut f = LogFile::open(dir.path(), "main.log").unwrap();
    f.write_line("new\n").unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("main.log")).unwrap(),
        "new\n"
    );
    assert_eq!(
        std::fs::metadata(dir.path().join("main.1.log"))
            .unwrap()
            .len(),
        2_097_153
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("main.2.log")).unwrap(),
        "1"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("main.3.log")).unwrap(),
        "2"
    );
}

#[test]
fn rotation_counts_written_bytes_like_electron_log() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.log"), vec![b'x'; 2_097_100]).unwrap();
    let mut f = LogFile::open(dir.path(), "main.log").unwrap();
    let line = format!("{}\n", "y".repeat(59));
    // 2 097 100 + 60 = 2 097 160 > 2 МБ: ротация перед следующей строкой, не этой.
    f.write_line(&line).unwrap();
    assert!(!dir.path().join("main.1.log").exists());
    f.write_line("z\n").unwrap();
    assert_eq!(
        std::fs::metadata(dir.path().join("main.1.log"))
            .unwrap()
            .len(),
        2_097_160
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("main.log")).unwrap(),
        "z\n"
    );
    // Ровно 2 МБ — ещё без ротации.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.log"), vec![b'x'; 2_097_152]).unwrap();
    let mut f = LogFile::open(dir.path(), "main.log").unwrap();
    f.write_line("a\n").unwrap();
    assert!(!dir.path().join("main.1.log").exists());
}

#[test]
fn missing_log_dir_is_created() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("state").join("logs");
    let mut f = LogFile::open(&logs, "main.log").unwrap();
    f.write_line("a\n").unwrap();
    assert_eq!(
        std::fs::read_to_string(logs.join("main.log")).unwrap(),
        "a\n"
    );
}

/// Глобальный подписчик ставится один раз на процесс, поэтому всё, что
/// требует `init`, проверяется в одном тесте.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn init_filters_targets_and_survives_panics() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let guard = init(&paths).unwrap();
    assert_eq!(guard.path(), paths.logs_dir.join("main.log"));
    install_panic_hook();

    tracing::info!(target: "game-delivery", "канал: HTTP 304");
    tracing::debug!(target: "launcher", "не пишется: debug");
    tracing::info!(target: "hlsr::window", "[ui-scale] масштаб 125 %");
    tracing::info!(target: "winit::window", "не пишется: info чужого крейта");
    tracing::warn!(target: "zbus", "чужое предупреждение");
    log::info!(target: "wgpu_core", "не пишется: info из log");
    log::warn!(target: "wgpu_core", "предупреждение из log");
    tracing::warn!(target: "launcher", count = 3, "поле");
    let home = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let user = std::env::var(home).unwrap();
    let user = std::path::Path::new(&user)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    tracing::info!(target: "launcher", "путь /home/{user}/HLSR");

    spawn_logged("проба", async { panic!("бум") })
        .await
        .unwrap();
    spawn_logged("после", async {}).await.unwrap();
    let handle = spawn_logged("отмена", std::future::pending());
    handle.abort();
    assert!(handle.await.unwrap_err().is_cancelled());

    let log = std::fs::read_to_string(paths.logs_dir.join("main.log")).unwrap();
    let lines: Vec<&str> = log
        .split(EOL)
        .filter(|line| line.starts_with('['))
        .map(|line| &line[26..])
        .collect();
    let expected_start = [
        "[info]  [game-delivery] канал: HTTP 304",
        "[info]  [ui-scale] масштаб 125 %",
        "[warn]  чужое предупреждение",
        "[warn]  предупреждение из log",
        "[warn]  [launcher] поле count=3",
        "[info]  [launcher] путь /home/***/HLSR",
    ];
    assert_eq!(&lines[..expected_start.len()], expected_start, "{log}");
    assert!(!log.contains("не пишется"), "{log}");
    assert!(
        log.contains("[error] [launcher] паника: бум в crates/hlsr-core/tests/logging.rs:")
            || log
                .contains("[error] [launcher] паника: бум в crates\\hlsr-core\\tests\\logging.rs:"),
        "{log}"
    );
    assert!(
        log.contains("[error] [launcher] фоновая задача проба упала: бум"),
        "{log}"
    );
    assert!(!log.contains("отмена"), "{log}");
}
