//! Живая часть `diagnostics/DiagnosticsExport.js`: `buildManifest` (сводка об окружении) и
//! `collect` (манифест и журналы с маскированием в каталог отчёта). `pack` и
//! `exportDiagnostics` не переносятся — ими пользовался только недостижимый канал
//! `export-diagnostics` (решение 19 спецификации).

use super::log_policy;
use hlsr_core::js::fs::read_file;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, utf8_string};
use hlsr_core::json_text::stringify;
use hlsr_core::redact::{redact_text_for, safe_env};
use serde_json::{Map, Value, json};
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

/// Журнал в отчёте — последний мегабайт (`MAX_LOG_BYTES`).
pub const MAX_LOG_BYTES: u64 = 1024 * 1024;

/// Сведения об окружении для `manifest.json` (поля `info` у `buildManifest`; пустая строка —
/// «не задано», как `||` в JS). В Rust-лаунчере нет Electron, Chromium и Node: блок `runtime`
/// оставлен ради формата отчёта, поля заполняет вызывающий (пустые остаются пустыми).
#[derive(Debug, Clone, Default)]
pub struct ManifestInput {
    /// `now` (ISO); пусто — `1970-01-01T00:00:00.000Z`.
    pub now: String,
    pub app_name: String,
    pub app_version: String,
    /// Пусто — `production`.
    pub build_channel: String,
    pub electron_version: String,
    pub chrome_version: String,
    pub node_version: String,
    pub arch: String,
    pub platform: String,
    pub os_release: String,
    pub library_path: String,
    pub library_exists: bool,
    /// Пусто — `idle`.
    pub update_state: String,
    pub update_error_code: Option<String>,
    /// Окружение процесса: в отчёт попадают только имена из `ALLOWED_ENV`.
    pub env: Vec<(String, String)>,
    /// Имя пользователя ОС: прячется в тексте (`<user>`).
    pub user_name: Option<String>,
}

fn or_default(text: &str, default: &str) -> String {
    if text.is_empty() { default } else { text }.to_owned()
}

/// `buildManifest(info)`.
pub fn build_manifest(info: &ManifestInput) -> Value {
    let user = info.user_name.as_deref();
    let env: Map<String, Value> = safe_env(&info.env, None, user)
        .into_iter()
        .map(|(name, value)| (name, Value::String(value)))
        .collect();
    json!({
        "schema": 1,
        "generatedAt": or_default(&info.now, "1970-01-01T00:00:00.000Z"),
        "app": {
            "name": info.app_name,
            "version": info.app_version,
            "buildChannel": or_default(&info.build_channel, "production"),
        },
        "runtime": {
            "electron": info.electron_version,
            "chrome": info.chrome_version,
            "node": info.node_version,
            "arch": info.arch,
        },
        "os": {
            "platform": info.platform,
            "release": info.os_release,
        },
        "library": {
            "path": redact_text_for(&info.library_path, user),
            "exists": info.library_exists,
        },
        "update": {
            "state": or_default(&info.update_state, "idle"),
            "lastErrorCode": info.update_error_code.as_deref().filter(|code| !code.is_empty()),
        },
        "env": env,
    })
}

/// Параметры [`collect`] (`options` у `collect`).
#[derive(Debug, Clone, Default)]
pub struct CollectOptions {
    pub manifest: ManifestInput,
    /// Каталог отчёта (создаётся).
    pub out_dir: PathBuf,
    /// Каталог журналов; нет или не существует — только манифест.
    pub logs_dir: Option<PathBuf>,
    /// Основной журнал; пусто — `main.log`.
    pub log_name: String,
    /// Предел хвоста журнала; `None` или 0 — [`MAX_LOG_BYTES`].
    pub max_log_bytes: Option<u64>,
    /// Архивов журнала; `None` или 0 — [`log_policy::MAX_ARCHIVES`].
    pub max_archives: Option<usize>,
}

/// Итог [`collect`].
#[derive(Debug, Clone)]
pub struct Collected {
    pub dir: PathBuf,
    /// Манифест, затем журналы в порядке `logFiles` + `gameLogFiles`.
    pub files: Vec<PathBuf>,
    pub manifest: Value,
}

/// `tailBytes(file, limit)`: последние `limit` байт файла как UTF-8 (разрезанный в начале символ
/// становится U+FFFD, как у `Buffer.toString`).
fn tail_bytes(file: &Path, limit: u64) -> io::Result<String> {
    let size = std::fs::metadata(file)?.len();
    let take = size.min(limit);
    let mut handle = std::fs::File::open(file)?;
    handle.seek(SeekFrom::Start(size - take))?;
    let mut buffer = vec![0u8; usize::try_from(take).unwrap_or(usize::MAX)];
    handle.read_exact(&mut buffer)?;
    Ok(utf8_string(&buffer))
}

/// `collect(options)`: `manifest.json` и хвосты журналов (маскированные) в `out_dir`.
pub fn collect(options: &CollectOptions) -> io::Result<Collected> {
    if options.out_dir.as_os_str().is_empty() {
        return Err(io::Error::other("не задан каталог отчёта"));
    }
    std::fs::create_dir_all(&options.out_dir)?;

    let manifest = build_manifest(&options.manifest);
    let mut files = Vec::new();

    let manifest_file = options.out_dir.join("manifest.json");
    std::fs::write(&manifest_file, stringify(&manifest, "  ") + "\n")?;
    files.push(manifest_file);

    let user = options.manifest.user_name.as_deref();
    let limit = options
        .max_log_bytes
        .filter(|max| *max != 0)
        .unwrap_or(MAX_LOG_BYTES);
    if let Some(logs_dir) = options.logs_dir.as_deref().filter(|dir| dir.exists()) {
        let logs_dir_text = logs_dir.to_string_lossy();
        let log_name = or_default(&options.log_name, "main.log");
        let logs = log_policy::log_files(&logs_dir_text, &log_name, options.max_archives)
            .into_iter()
            .chain(log_policy::game_log_files(&logs_dir_text));

        let path = NodePath::native();
        let root = path.resolve(&[&logs_dir_text]);
        for file in logs {
            let resolved = path.resolve(&[&file]);
            // Файл за пределами каталога журналов в отчёт не берётся.
            let sep = path.sep();
            if resolved != root
                && !js_lower(&resolved).starts_with(&format!("{}{sep}", js_lower(&root)))
            {
                continue;
            }
            let Ok(text) = tail_bytes(Path::new(&resolved), limit) else {
                continue;
            };
            let target = options.out_dir.join(path.basename(&resolved));
            std::fs::write(&target, redact_text_for(&text, user))?;
            files.push(target);
        }
    }

    Ok(Collected {
        dir: options.out_dir.clone(),
        files,
        manifest,
    })
}

/// `String.prototype.toLowerCase` для сравнения путей.
fn js_lower(text: &str) -> String {
    text.to_lowercase()
}

/// Строка файла как её читает `fs.readFileSync(file, "utf8")` (без среза BOM).
pub fn read_text(file: &Path) -> io::Result<String> {
    Ok(js::utf8_string(&read_file(file)?))
}
