//! `SystemRunner` — `defaultRun` (`execFile` без оболочки, с тайм-аутом): код выхода, вывод,
//! «не запустилось», убит по тайм-ауту, окружение без переменных окна лаунчера. Процессы настоящие,
//! но свои: `cmd.exe` (Windows) / `sh` (Linux) с безобидными командами.

use hlsr_platform::steam_depot::{CommandRunner, RunCode, SystemRunner};

fn shell(script: &str) -> (&'static str, Vec<String>) {
    if cfg!(windows) {
        ("cmd.exe", vec!["/c".to_owned(), script.to_owned()])
    } else {
        ("sh", vec!["-c".to_owned(), script.to_owned()])
    }
}

#[tokio::test]
async fn exit_code_and_output_are_reported() {
    let (file, args) = shell(if cfg!(windows) {
        "echo hi&& exit 3"
    } else {
        "echo hi; exit 3"
    });
    let result = SystemRunner.run(file, &args, 10_000).await;
    assert_eq!(result.status, 3);
    let error = result.error.expect("ненулевой код — ошибка");
    assert_eq!(error.code, Some(RunCode::Exit(3)));
    assert!(!error.killed);
    assert_eq!(error.signal, None);
    assert!(result.stdout.contains("hi"), "{:?}", result.stdout);
}

#[tokio::test]
async fn success_has_no_error() {
    let (file, args) = shell("exit 0");
    let result = SystemRunner.run(file, &args, 10_000).await;
    assert_eq!(result.status, 0);
    assert_eq!(result.error, None);
}

#[tokio::test]
async fn a_program_that_does_not_start_is_reported_by_name() {
    let result = SystemRunner
        .run("hlsr-no-such-program-4f2a", &[], 10_000)
        .await;
    let error = result.error.expect("не запустилась");
    assert_eq!(error.code, Some(RunCode::Name("ENOENT".to_owned())));
    assert_eq!(result.status, 1);
    assert!(!error.killed);
}

#[tokio::test]
async fn a_slow_program_is_killed_by_the_timeout() {
    let (file, args) = if cfg!(windows) {
        (
            "ping.exe",
            vec!["-n".to_owned(), "20".to_owned(), "127.0.0.1".to_owned()],
        )
    } else {
        ("sleep", vec!["20".to_owned()])
    };
    let started = std::time::Instant::now();
    let result = SystemRunner.run(file, &args, 300).await;
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    let error = result.error.expect("тайм-аут — ошибка");
    assert!(error.killed);
    assert_eq!(error.signal.as_deref(), Some("SIGTERM"));
    assert_eq!(result.status, 1);
}

/// Окружение этого процесса менять нельзя (тесты идут потоками, `set_var` небезопасен): сценарий
/// идёт в повторном запуске этого же тестового бинарника, которому нужные переменные заданы
/// через `Command::env`.
const INNER: &str = "HLSR_TEST_RERUN_INNER";

#[test]
fn the_launcher_window_variables_do_not_reach_the_child() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "the_launcher_window_variables_inner",
            "--test-threads=1",
        ])
        .env(INNER, "1")
        .env("SLINT_SCALE_FACTOR", "1.25")
        .env("WINIT_X11_SCALE_FACTOR", "1.25")
        .env("HLSR_TEST_KEEP", "kept")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "{stdout}
{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Сам сценарий: выполняется только в повторном запуске (в обычном прогоне пуст).
#[tokio::test]
async fn the_launcher_window_variables_inner() {
    if std::env::var_os(INNER).is_none() {
        return;
    }
    let (file, args) = shell(if cfg!(windows) { "set" } else { "env" });
    let result = SystemRunner.run(file, &args, 10_000).await;
    assert_eq!(result.status, 0);
    assert!(
        !result.stdout.contains("SLINT_SCALE_FACTOR"),
        "{}",
        result.stdout
    );
    assert!(!result.stdout.contains("WINIT_X11_SCALE_FACTOR"));
    assert!(result.stdout.contains("HLSR_TEST_KEEP=kept"));
}
