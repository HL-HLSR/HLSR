//! Выгрузка отчёта в ZIP против 0.4.7 (`DiagnosticsZip.buildZip`, фикстура `diagnostics.json`,
//! сценарии «С»): список записей архива и их несжатые байты — `7za x` обоих архивов, снимок
//! дерева одного формата. Плюс обработчик `export-diagnostics-zip` (диалог, отмена, сбой) и
//! отказы сборки.

mod support;

use hlsr_core::js::Platform;
use hlsr_core::paths::Resources;
use hlsr_update::diagnostics::export::ManifestInput;
use hlsr_update::diagnostics::runner::Check;
use hlsr_update::diagnostics::zip::{
    CrashInfo, ExportOutcome, SaveDialog, ZipData, ZipExporter, ZipRequest, build_zip,
};
use hlsr_update::ports::{BoxFuture, FakeClock, MemoryLogger};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime};
use support::tree::{replacing, snapshot};
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("diagnostics.json"));

/// 2026-10-16T09:30:05Z — часы сверки.
const NOW_MS: i64 = 1_792_143_005_000;

fn sevenzip() -> PathBuf {
    Resources::at(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources"))
        .sevenzip_for(Platform::CURRENT)
}

fn extract(zip: &Path, to: &Path) {
    let status = Command::new(sevenzip())
        .args(["x", "-y"])
        .arg(format!("-o{}", to.display()))
        .arg(zip)
        .output()
        .expect("7za");
    assert!(
        status.status.success(),
        "7za x не распаковал {}",
        zip.display()
    );
}

fn context(input: &Value) -> ManifestInput {
    let text = |key: &str| input[key].as_str().unwrap_or("").to_owned();
    ManifestInput {
        now: String::new(),
        app_name: text("appName"),
        app_version: text("appVersion"),
        build_channel: text("buildChannel"),
        electron_version: text("electronVersion"),
        chrome_version: text("chromeVersion"),
        node_version: text("nodeVersion"),
        arch: text("arch"),
        platform: text("platform"),
        os_release: text("osRelease"),
        library_path: text("libraryPath"),
        library_exists: input["libraryExists"].as_bool().unwrap_or(false),
        update_state: text("updateState"),
        update_error_code: None,
        env: input["env"]
            .as_object()
            .map(|map| {
                map.iter()
                    .map(|(k, v)| (k.clone(), str_of(v).to_owned()))
                    .collect()
            })
            .unwrap_or_default(),
        user_name: input["userName"].as_str().map(str::to_owned),
    }
}

fn check_of(row: &Value) -> Check {
    let mut check = Check::new(
        str_of(&row["id"]),
        str_of(&row["group"]),
        str_of(&row["status"]),
    )
    .code(row["code"].as_str())
    .detail(str_of(&row["detail"]));
    if let Some(key) = row["detailKey"].as_str() {
        check = check.detail_key(key);
    }
    if let Some(args) = row["detailArgs"].as_array() {
        check = check.detail_args(args.clone());
    }
    check
}

fn dump_time() -> SystemTime {
    let seconds = chrono::NaiveDate::from_ymd_opt(2026, 10, 15)
        .unwrap()
        .and_hms_opt(20, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp();
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds as u64)
}

/// Разложить файлы сценария и собрать `ZipRequest`.
fn request_of(spec: &Value, root: &Path) -> ZipRequest {
    let logs = root.join("logs");
    if let Some(files) = spec["logs"].as_object() {
        std::fs::create_dir_all(&logs).unwrap();
        for (name, text) in files {
            std::fs::write(logs.join(name), str_of(text)).unwrap();
        }
    }
    let notifications = root.join("notifications.json");
    if let Some(text) = spec["notifications"].as_str() {
        std::fs::write(&notifications, text).unwrap();
    }
    let installed = root.join("installed.json");
    if spec.get("installed").is_some() {
        std::fs::write(&installed, spec["installed"].to_string()).unwrap();
    } else if let Some(text) = spec["installedText"].as_str() {
        std::fs::write(&installed, text).unwrap();
    }
    let crash = spec.get("crash").map(|crash| {
        let dump = root.join("crash.dmp");
        if crash["missing"] != true {
            std::fs::write(&dump, str_of(&crash["dump"])).unwrap();
            let file = std::fs::OpenOptions::new().write(true).open(&dump).unwrap();
            file.set_modified(dump_time()).unwrap();
        }
        CrashInfo {
            dump_path: Some(dump),
            exit_code: crash["exitCode"].as_i64(),
            hint: crash["hint"].as_str().map(str::to_owned),
            dump_allowed: crash["allowed"] == true,
        }
    });
    let some_if = |present: bool, path: &Path| present.then(|| path.to_path_buf());
    let data = ZipData {
        logs_dir: if spec["logsMissing"] == true {
            Some(root.join("no-logs"))
        } else {
            some_if(spec.get("logs").is_some(), &logs)
        },
        checks: spec["checks"]
            .as_array()
            .map(|rows| rows.iter().map(check_of).collect())
            .unwrap_or_default(),
        stats_summary: spec.get("stats").cloned(),
        notifications_file: if spec["notificationsMissing"] == true {
            Some(root.join("no-notifications.json"))
        } else {
            some_if(spec.get("notifications").is_some(), &notifications)
        },
        installed_file: if spec["installedMissing"] == true {
            Some(root.join("no-installed.json"))
        } else {
            some_if(
                spec.get("installed").is_some() || spec.get("installedText").is_some(),
                &installed,
            )
        },
        crash_info: crash,
        discord_rpc: spec["discordNormalized"].clone(),
        context: context(&spec["context"]),
    };
    let mut request = ZipRequest::new(
        data,
        root.join("report.zip"),
        spec["includeDump"] == true,
        sevenzip(),
    );
    request.temp_dir = root.to_path_buf();
    request
}

#[test]
fn the_zip_has_the_same_entries_and_bytes_as_the_reference() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["zip"]) {
        let id = str_of(&case["id"]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let request = request_of(&case["spec"], root);
        let out_file = request.out_file.clone();
        let outcome = build_zip(&request, &FakeClock::new(NOW_MS));
        mismatches.check_eq(
            outcome.is_ok(),
            case["ok"].as_bool().unwrap(),
            format!("{id}: итог ({outcome:?})"),
        );
        if outcome.is_err() {
            continue;
        }
        let extracted = root.join("extracted");
        extract(&out_file, &extracted);
        let transform = replacing(&root.to_string_lossy(), "{root}");
        let entries = snapshot(&extracted, Some(&transform));
        mismatches.check(&entries, &case["entries"], format!("{id}: записи архива"));

        let leftovers: Vec<String> = std::fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("hlsr-diag") || name.ends_with(".tmp"))
            .collect();
        mismatches.check_eq(
            leftovers,
            Vec::<String>::new(),
            format!("{id}: следы сборки"),
        );
    }
    mismatches.finish("ZIP диагностики");
}

#[test]
fn build_refuses_without_a_file_and_leaves_no_traces_when_the_archiver_fails() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut request = request_of(
        &json!({"context": FIXTURE["zip"][0]["spec"]["context"]}),
        root,
    );

    request.out_file = PathBuf::new();
    assert_eq!(
        build_zip(&request, &FakeClock::new(NOW_MS)).unwrap_err().0,
        "не выбран файл экспорта"
    );

    // Архиватора нет — отчёт не создан, временный каталог убран.
    request.out_file = root.join("report.zip");
    request.sevenzip = root.join("no-such-7za");
    assert_eq!(
        build_zip(&request, &FakeClock::new(NOW_MS)).unwrap_err().0,
        "не удалось создать архив диагностики"
    );
    assert!(!request.out_file.exists());
    let names: Vec<String> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.is_empty(), "следы: {names:?}");

    // Архиватор, не уложившийся в срок, убивается.
    request.sevenzip = sevenzip();
    request.sevenzip_timeout = Duration::ZERO;
    assert_eq!(
        build_zip(&request, &FakeClock::new(NOW_MS)).unwrap_err().0,
        "не удалось создать архив диагностики"
    );
    assert!(!request.out_file.exists());
}

// ------------------------------------------------------------------ export-diagnostics-zip

struct FakeDialog {
    answer: Mutex<Option<PathBuf>>,
    asked: Mutex<Vec<PathBuf>>,
}

impl SaveDialog for FakeDialog {
    fn pick<'a>(&'a self, default_path: &'a Path) -> BoxFuture<'a, Option<PathBuf>> {
        self.asked.lock().unwrap().push(default_path.to_path_buf());
        let answer = self.answer.lock().unwrap().clone();
        Box::pin(async move { answer })
    }
}

fn exporter(
    root: &Path,
    answer: Option<PathBuf>,
) -> (ZipExporter, Arc<FakeDialog>, Arc<MemoryLogger>) {
    let dialog = Arc::new(FakeDialog {
        answer: Mutex::new(answer),
        asked: Mutex::default(),
    });
    let logger = Arc::new(MemoryLogger::default());
    let exporter = ZipExporter {
        dialog: dialog.clone(),
        logger: logger.clone(),
        clock: Arc::new(FakeClock::new(NOW_MS)),
        sevenzip: sevenzip(),
        temp_dir: root.to_path_buf(),
        documents_dir: root.join("Documents"),
    };
    (exporter, dialog, logger)
}

fn minimal_data() -> ZipData {
    ZipData {
        context: context(&FIXTURE["zip"][0]["spec"]["context"]),
        discord_rpc: json!(null),
        ..ZipData::default()
    }
}

#[tokio::test]
async fn export_saves_the_zip_where_the_person_chose() {
    let dir = tempfile::tempdir().unwrap();
    let chosen = dir.path().join("mine.zip");
    let (exporter, dialog, logger) = exporter(dir.path(), Some(chosen.clone()));
    let outcome = exporter.export(false, || Ok(minimal_data())).await;
    assert_eq!(outcome, ExportOutcome::Saved(chosen.clone()));
    assert_eq!(outcome.to_value(), json!({"ok": true}));
    assert!(chosen.exists());

    let asked = dialog.asked.lock().unwrap().clone();
    let name = asked[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(asked[0].starts_with(dir.path().join("Documents")));
    assert!(
        name.starts_with("HLSR-Diagnostics-") && name.ends_with(".zip"),
        "{name}"
    );
    // HLSR-Diagnostics-YYYYMMDD-HHMMSS.zip по местному времени.
    assert_eq!(name.len(), "HLSR-Diagnostics-20261016-093005.zip".len());
    assert_eq!(
        logger.take(),
        vec![(
            "info",
            "[diagnostics] ZIP экспортирован: mine.zip".to_owned()
        )]
    );
}

#[tokio::test]
async fn export_reports_cancellation_and_failures() {
    let dir = tempfile::tempdir().unwrap();

    let (exporter_cancelled, _, logger) = exporter(dir.path(), None);
    let cancelled = exporter_cancelled
        .export(false, || Ok(minimal_data()))
        .await;
    assert_eq!(cancelled, ExportOutcome::Cancelled);
    assert_eq!(
        cancelled.to_value(),
        json!({"ok": false, "cancelled": true})
    );
    assert!(logger.take().is_empty());

    let chosen = dir.path().join("broken.zip");
    let (exporter_failed, _, logger) = exporter(dir.path(), Some(chosen.clone()));
    let failed = exporter_failed
        .export(false, || Err("нет статистики".to_owned()))
        .await;
    assert_eq!(failed, ExportOutcome::Failed);
    assert_eq!(
        failed.to_value(),
        json!({"ok": false, "error": "EXPORT_FAILED"})
    );
    assert!(!chosen.exists());
    assert_eq!(
        logger.take(),
        vec![(
            "warn",
            "[diagnostics] экспорт ZIP не удался: нет статистики".to_owned()
        )]
    );

    let (mut exporter_no_archiver, _, logger) = exporter(dir.path(), Some(chosen.clone()));
    exporter_no_archiver.sevenzip = dir.path().join("no-such-7za");
    let failed = exporter_no_archiver
        .export(false, || Ok(minimal_data()))
        .await;
    assert_eq!(failed, ExportOutcome::Failed);
    assert_eq!(
        logger.take(),
        vec![(
            "warn",
            "[diagnostics] экспорт ZIP не удался: не удалось создать архив диагностики".to_owned()
        )]
    );
}
