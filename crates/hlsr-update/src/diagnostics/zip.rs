//! `diagnostics/DiagnosticsZip.js` и обработчик `export-diagnostics-zip` из `AppManager.js`:
//! отчёт для поддержки одним ZIP. Во временном каталоге `hlsr-diag-*` собираются `manifest.json`,
//! журналы, `diagnostics-summary.json`, `notifications.json`, `installed.json`,
//! `build-metadata.json`, `crash-dump.json` и (по флагу) сам дамп; всё, кроме журналов и
//! сведений о сборке, проходит маскирование личных данных. Архив делает `7za a -tzip` (ZIP-библиотеки
//! в проекте нет), затем файл ложится на место через `.tmp` и переименование.
//!
//! Отступление: временный каталог не проходит `realpathSync` (в Windows он дал бы путь вида
//! `\\?\…`, а архиватору нужен обычный).

use super::export::{self, CollectOptions, ManifestInput, read_text};
use super::runner::Check;
use crate::ports::{BoxFuture, Clock, Logger};
use hlsr_core::js::iso::iso_from_ms;
use hlsr_core::js::{self, iso_string};
use hlsr_core::json_text::{json_parse, stringify};
use hlsr_core::redact::redact_text_for;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Архиватор работает не дольше минуты (`timeout: 60000`).
pub const SEVENZIP_TIMEOUT: Duration = Duration::from_secs(60);

/// Ошибка выгрузки: текст попадает в журнал (`error.message`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ZipError(pub String);

impl From<std::io::Error> for ZipError {
    fn from(error: std::io::Error) -> ZipError {
        ZipError(error.to_string())
    }
}

/// Сведения о последнем падении игры (`gameDelivery.crashInfo(null)`, ПП3).
#[derive(Debug, Clone, Default)]
pub struct CrashInfo {
    /// Файл дампа; нет или не существует — `crash-dump.json` не пишется.
    pub dump_path: Option<PathBuf>,
    pub exit_code: Option<i64>,
    pub hint: Option<String>,
    /// Результат `crashDumps.isAllowedDumpPath(dumpPath, installPath)` (ПП3): дамп лежит в папке
    /// игры или в известной папке дампов. Без этого дамп в архив не кладётся.
    pub dump_allowed: bool,
}

/// Всё, из чего собирается отчёт (`opts` у `buildZip`, кроме `outFile` и `includeDump`).
#[derive(Debug, Clone, Default)]
pub struct ZipData {
    pub logs_dir: Option<PathBuf>,
    /// Строки последнего прогона проверок (`lastDiagnostics`); не было прогона — пусто.
    pub checks: Vec<Check>,
    /// `stats.summary("ghosting-renewed")` (ПП3); `None` — `null`.
    pub stats_summary: Option<Value>,
    pub notifications_file: Option<PathBuf>,
    pub installed_file: Option<PathBuf>,
    pub crash_info: Option<CrashInfo>,
    /// `discordDiagnostics.normalize(discordRpc.diagnostics())` (ПП4): уже в нормальной форме.
    pub discord_rpc: Value,
    /// Сведения об окружении, имя пользователя для маскирования.
    pub context: ManifestInput,
}

/// Куда и как собирать (`outFile`, `includeDump` и окружение сборки).
#[derive(Debug, Clone)]
pub struct ZipRequest {
    pub data: ZipData,
    pub out_file: PathBuf,
    pub include_dump: bool,
    /// `7za` (`Resources::sevenzip()`).
    pub sevenzip: PathBuf,
    /// Где создать `hlsr-diag-*`: `%TEMP%` (в отладочной сборке — `HLSR_TEMP_DIR`).
    pub temp_dir: PathBuf,
    pub sevenzip_timeout: Duration,
}

impl ZipRequest {
    pub fn new(
        data: ZipData,
        out_file: PathBuf,
        include_dump: bool,
        sevenzip: PathBuf,
    ) -> ZipRequest {
        ZipRequest {
            data,
            out_file,
            include_dump,
            sevenzip,
            temp_dir: std::env::temp_dir(),
            sevenzip_timeout: SEVENZIP_TIMEOUT,
        }
    }
}

/// `timestampName(date)` для заданного момента и смещения местного времени от UTC в секундах:
/// `20261016-093005`.
pub fn timestamp_name_at(ms: i64, offset_seconds: i32) -> String {
    let shifted = ms + i64::from(offset_seconds) * 1000;
    let iso = iso_from_ms(shifted);
    // 2026-10-16T09:30:05.000Z → 20261016-093005
    let digits = |range: std::ops::Range<usize>| iso.get(range).unwrap_or("");
    format!(
        "{}{}{}-{}{}{}",
        digits(0..4),
        digits(5..7),
        digits(8..10),
        digits(11..13),
        digits(14..16),
        digits(17..19)
    )
}

/// `timestampName(new Date(ms))`: по местному времени ОС.
pub fn timestamp_name(ms: i64) -> String {
    use chrono::{Local, Offset, TimeZone};
    let offset = Local
        .timestamp_millis_opt(ms)
        .single()
        .map_or(0, |at| at.offset().fix().local_minus_utc());
    timestamp_name_at(ms, offset)
}

fn sha256_file(file: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(file)?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn write_json(file: &Path, value: &Value) -> std::io::Result<()> {
    std::fs::write(file, stringify(value, "  ") + "\n")
}

/// Запустить архиватор и дождаться: не успел за `timeout` — убит.
fn run_sevenzip(binary: &Path, args: &[&std::ffi::OsStr], timeout: Duration) -> bool {
    let mut command = Command::new(binary);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // `windowsHide: true`.
        command.creation_flags(0x0800_0000);
    }
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return false,
        }
    }
}

/// `buildZip(options)`: собирает отчёт и кладёт его в `request.out_file`.
pub fn build_zip(request: &ZipRequest, clock: &dyn Clock) -> Result<PathBuf, ZipError> {
    if request.out_file.as_os_str().is_empty() {
        return Err(ZipError("не выбран файл экспорта".to_owned()));
    }
    let data = &request.data;
    let temp = tempfile::Builder::new()
        .prefix("hlsr-diag-")
        .tempdir_in(&request.temp_dir)?;
    let root = temp.path();
    let user = data.context.user_name.as_deref();
    let redacted = |text: &str| redact_text_for(text, user);

    export::collect(&CollectOptions {
        manifest: ManifestInput {
            now: iso_from_ms(clock.now_ms()),
            ..data.context.clone()
        },
        out_dir: root.to_path_buf(),
        logs_dir: data.logs_dir.clone(),
        ..CollectOptions::default()
    })?;

    let summary = stringify(
        &json!({
            "schema": 1,
            "generatedAt": iso_from_ms(clock.now_ms()),
            "appVersion": data.context.app_version,
            "checks": data.checks.iter().map(Check::to_value).collect::<Vec<_>>(),
            "lastSession": data.stats_summary.as_ref().filter(|s| js::truthy(s)),
            "discordRpc": data.discord_rpc,
        }),
        "  ",
    ) + "\n";
    std::fs::write(root.join("diagnostics-summary.json"), redacted(&summary))?;

    for (name, file) in [
        ("notifications.json", &data.notifications_file),
        ("installed.json", &data.installed_file),
    ] {
        if let Some(file) = file.as_deref().filter(|file| file.exists()) {
            // Необязательный файл: сбой чтения или записи отчёт не ломает.
            if let Ok(text) = read_text(file) {
                let _ = std::fs::write(root.join(name), redacted(&text));
            }
        }
    }

    if let Some(file) = data.installed_file.as_deref().filter(|file| file.exists()) {
        let installed = read_text(file).ok().and_then(|text| json_parse(&text).ok());
        if let Some(installed) = installed {
            let _ = write_json(
                &root.join("build-metadata.json"),
                &build_metadata_summary(&installed),
            );
        }
    }

    if let Some(crash) = &data.crash_info
        && let Some(dump) = crash.dump_path.as_deref().filter(|dump| dump.exists())
    {
        let metadata = std::fs::metadata(dump)?;
        let name = dump
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut meta = Map::new();
        meta.insert("name".to_owned(), json!(name));
        meta.insert("size".to_owned(), json!(metadata.len()));
        meta.insert("mtime".to_owned(), json!(iso_string(metadata.modified()?)));
        meta.insert("sha256".to_owned(), json!(sha256_file(dump)?));
        meta.insert("exitCode".to_owned(), json!(crash.exit_code));
        meta.insert(
            "hint".to_owned(),
            json!(crash.hint.as_deref().filter(|hint| !hint.is_empty())),
        );
        meta.insert("included".to_owned(), json!(false));

        if request.include_dump && crash.dump_allowed {
            std::fs::copy(dump, root.join(&name))?;
            meta.insert("included".to_owned(), json!(true));
        }
        write_json(&root.join("crash-dump.json"), &Value::Object(meta))?;
    }

    let payload = root.join("payload");
    std::fs::create_dir_all(&payload)?;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_name() != "payload" {
            std::fs::rename(entry.path(), payload.join(entry.file_name()))?;
        }
    }

    let zip_temp = root.join("diagnostics.zip");
    let everything = payload.join("*");
    let made = run_sevenzip(
        &request.sevenzip,
        &[
            "a".as_ref(),
            "-tzip".as_ref(),
            "-y".as_ref(),
            zip_temp.as_os_str(),
            everything.as_os_str(),
        ],
        request.sevenzip_timeout,
    );
    if !made || !zip_temp.exists() {
        return Err(ZipError("не удалось создать архив диагностики".to_owned()));
    }

    let mut out_tmp = request.out_file.clone().into_os_string();
    out_tmp.push(".tmp");
    let out_tmp = PathBuf::from(out_tmp);
    std::fs::copy(&zip_temp, &out_tmp)?;
    std::fs::rename(&out_tmp, &request.out_file)?;

    Ok(request.out_file.clone())
}

/// `build-metadata.json`: сводка из `installed.json` (схема 1).
fn build_metadata_summary(installed: &Value) -> Value {
    let truthy_field = |name: &str| installed.get(name).filter(|v| js::truthy(v));
    let meta = truthy_field("buildMetadata");
    let build_metadata = match meta {
        None => Value::Null,
        Some(meta) => {
            let mut out = Map::new();
            for key in [
                "schemaVersion",
                "gameId",
                "version",
                "buildId",
                "buildName",
                "gitCommit",
                "builtAt",
                "architecture",
                "minimumLauncherVersion",
            ] {
                if let Some(value) = meta.get(key) {
                    out.insert(key.to_owned(), value.clone());
                }
            }
            Value::Object(out)
        }
    };
    let or_null = |name: &str| truthy_field(name).cloned().unwrap_or(Value::Null);
    json!({
        "schema": 1,
        "state": if meta.is_some() { "present" } else { "legacy" },
        "buildMetadata": build_metadata,
        "verifiedAt": or_null("buildMetadataVerifiedAt"),
        "buildMetadataSha256": or_null("buildMetadataSha256"),
        "publishedAt": or_null("publishedAt"),
        "installedAt": or_null("installedAt"),
    })
}

// ------------------------------------------------------------------- обработчик выгрузки

/// `dialog.showSaveDialog`: выбор файла для отчёта.
pub trait SaveDialog: Send + Sync {
    /// Путь, который выбрал человек, или `None` — отмена.
    fn pick<'a>(&'a self, default_path: &'a Path) -> BoxFuture<'a, Option<PathBuf>>;
}

/// Ответ `export-diagnostics-zip`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportOutcome {
    Saved(PathBuf),
    Cancelled,
    Failed,
}

impl ExportOutcome {
    /// `{ok: true}` | `{ok: false, cancelled: true}` | `{ok: false, error: "EXPORT_FAILED"}`.
    pub fn to_value(&self) -> Value {
        match self {
            ExportOutcome::Saved(_) => json!({ "ok": true }),
            ExportOutcome::Cancelled => json!({ "ok": false, "cancelled": true }),
            ExportOutcome::Failed => json!({ "ok": false, "error": "EXPORT_FAILED" }),
        }
    }
}

/// Обработчик `export-diagnostics-zip`: диалог, сбор данных, сборка архива, журнал.
pub struct ZipExporter {
    pub dialog: Arc<dyn SaveDialog>,
    pub logger: Arc<dyn Logger>,
    pub clock: Arc<dyn Clock>,
    pub sevenzip: PathBuf,
    pub temp_dir: PathBuf,
    /// `app.getPath("documents")`: папка для имени по умолчанию.
    pub documents_dir: PathBuf,
}

impl ZipExporter {
    /// `include_dump` — человек разрешил приложить дамп. `gather` собирает данные отчёта
    /// (статистику, сведения о падении, Discord) уже после выбора файла; его ошибка — тоже
    /// `EXPORT_FAILED`.
    pub async fn export(
        &self,
        include_dump: bool,
        gather: impl FnOnce() -> Result<ZipData, String> + Send + 'static,
    ) -> ExportOutcome {
        let default_name = format!(
            "HLSR-Diagnostics-{}.zip",
            timestamp_name(self.clock.now_ms())
        );
        let default_path = self.documents_dir.join(default_name);
        let Some(picked) = self
            .dialog
            .pick(&default_path)
            .await
            .filter(|path| !path.as_os_str().is_empty())
        else {
            return ExportOutcome::Cancelled;
        };

        let sevenzip = self.sevenzip.clone();
        let temp_dir = self.temp_dir.clone();
        let clock = self.clock.clone();
        let out_file = picked.clone();
        let built = tokio::task::spawn_blocking(move || {
            let data = gather().map_err(ZipError)?;
            let mut request = ZipRequest::new(data, out_file, include_dump, sevenzip);
            request.temp_dir = temp_dir;
            build_zip(&request, clock.as_ref())
        })
        .await;
        match built {
            Ok(Ok(path)) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.logger
                    .info(&format!("[diagnostics] ZIP экспортирован: {name}"));
                ExportOutcome::Saved(path)
            }
            Ok(Err(error)) => {
                self.logger
                    .warn(&format!("[diagnostics] экспорт ZIP не удался: {error}"));
                ExportOutcome::Failed
            }
            Err(join) => {
                self.logger
                    .warn(&format!("[diagnostics] экспорт ZIP не удался: {join}"));
                ExportOutcome::Failed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_name_uses_the_given_offset() {
        // 2026-10-16T09:30:05Z
        let ms = 1_792_143_005_000;
        assert_eq!(iso_from_ms(ms), "2026-10-16T09:30:05.000Z");
        assert_eq!(timestamp_name_at(ms, 0), "20261016-093005");
        assert_eq!(timestamp_name_at(ms, 3 * 3600), "20261016-123005");
        // Переход через сутки и год: 2026-12-31T21:59:59Z.
        assert_eq!(
            timestamp_name_at(1_798_754_399_000, 7200),
            "20261231-235959"
        );
        assert_eq!(
            timestamp_name_at(1_798_754_399_000, 7201),
            "20270101-000000"
        );
        assert_eq!(timestamp_name_at(1_798_754_399_000, 0), "20261231-215959");
    }
}
