//! Зеркало ремонта (`repair-mirror.js` 0.4.9): файлы эталона лежат по хешу — `<база>/<первые два
//! знака SHA-256>/<SHA-256>` (раскладка `sha256-2`). [`unavailable_reason`] — можно ли вообще
//! пользоваться зеркалом версии; [`plan`] решает, годится ли оно для списка файлов (вдобавок:
//! файлов не больше [`MAX_FILES`], каждый не больше `maxFileSize`); [`RepairMirrorClient`]
//! скачивает файлы в каталог эталона.
//!
//! Файл идёт потоком в `<цель>.part` с подсчётом SHA-256 и только после сверки размера и хеша
//! становится целью: в памяти целиком он не бывает. Прогресс по байтам — не чаще раза в
//! [`PROGRESS_EVERY_MS`] и после каждого файла. Отмена (`signal`) — `CANCELLED` «Восстановление
//! отменено», `.part` удаляется. Ошибка записи на диск — не сеть: ENOSPC/EDQUOT → `NO_SPACE`,
//! EACCES/EPERM/EBUSY/EROFS → `PATH_NOT_WRITABLE`, повтор с другого зеркала не делается.
//! Антивирус ненадолго держит только что записанный файл: переименование `.part` при EPERM/EBUSY
//! повторяется один раз через [`LOCK_RETRY_MS`]. Прочие сбои потока — `NETWORK_ERROR`.
//!
//! По одному файлу: провайдеры по порядку, на каждом до `max_attempts` попыток подряд (без пауз);
//! `REPAIR_SOURCE_MISSING` (ответ не 200) переводит к следующему провайдеру сразу, `CANCELLED`,
//! `NO_SPACE` и `PATH_NOT_WRITABLE` обрывают работу, любая другая ошибка — повтор. Все отказали —
//! ошибка последней попытки. Скачивание идёт через общий ограничитель скорости
//! (`DownloadRateLimiter`), как у игр и LiveSplit.
//!
//! Сброс будущего [`RepairMirrorClient::fetch_into`] посреди работы оставляет в каталоге уже
//! полученные файлы (и, может быть, `.part`); убрать каталог — дело вызывающего, как и после
//! ошибки.

use crate::archive_service::CancelHandle;
use crate::download_rate_limiter::DownloadRateLimiter;
use crate::errors::{Code, DeliveryError, Failure, RawError};
use crate::file_ops;
use crate::http_client::{self, HttpClient, RequestOptions, is_loopback, url_hostname};
use crate::js_value as jv;
use crate::node_fs;
use hlsr_core::js;
use hlsr_core::js::errno;
use hlsr_core::json_text::js_number;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt as _;

/// `SHA256_PATTERN`: `/^[a-f0-9]{64}$/`.
pub const SHA256_PATTERN: &str = "^[a-f0-9]{64}$";
/// `DEFAULT_TIMEOUT_MS`: простой запроса до ответа и между кусками тела.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
/// `DEFAULT_ATTEMPTS`: попыток на одном провайдере.
pub const DEFAULT_ATTEMPTS: u32 = 2;
/// `PROGRESS_EVERY_MS`: прогресс по байтам не чаще.
pub const PROGRESS_EVERY_MS: u64 = 200;
/// `LOCK_RETRY_MS`: пауза перед повторным переименованием `.part`.
pub const LOCK_RETRY_MS: u64 = 500;
/// `MAX_FILES`: больше файлов с зеркала не берётся.
pub const MAX_FILES: usize = 2000;
/// `LAYOUTS`: известные раскладки.
pub const LAYOUTS: [&str; 1] = ["sha256-2"];

/// `DISK_ERRORS`: код ошибки записи → код доставки.
fn disk_error(code: &str) -> Option<Code> {
    match code {
        "ENOSPC" | "EDQUOT" => Some(Code::NoSpace),
        "EACCES" | "EPERM" | "EBUSY" | "EROFS" => Some(Code::PathNotWritable),
        _ => None,
    }
}

/// `cancelled()`.
fn cancelled() -> DeliveryError {
    DeliveryError::new(Code::Cancelled, "Восстановление отменено")
}

/// Готово, когда отменили; без сигнала — никогда.
async fn until_cancelled(signal: Option<&CancelHandle>) {
    match signal {
        Some(signal) => signal.cancelled().await,
        None => std::future::pending().await,
    }
}

fn is_cancelled(signal: Option<&CancelHandle>) -> bool {
    signal.is_some_and(CancelHandle::is_cancelled)
}

/// `assertDigest(sha256)`: 64 знака `[a-f0-9]`, иначе `PATH_INVALID`.
fn assert_digest(sha256: Option<&str>) -> Result<(), DeliveryError> {
    let value = sha256.map(|s| json!(s));
    if jv::is_hex64(value.as_ref()) {
        return Ok(());
    }
    Err(DeliveryError::new(
        Code::PathInvalid,
        format!(
            "Недопустимый SHA-256 для зеркала: {}",
            jv::template(value.as_ref())
        ),
    ))
}

/// `fileUrl(baseUrl, sha256, layout)`: адрес файла на зеркале. `baseUrl` без завершающих `/`.
pub fn file_url(
    base_url: &str,
    sha256: Option<&str>,
    layout: Option<&str>,
) -> Result<String, DeliveryError> {
    assert_digest(sha256)?;
    let digest = sha256.unwrap_or_default();
    if !layout.is_some_and(|layout| LAYOUTS.contains(&layout)) {
        return Err(DeliveryError::new(
            Code::ManifestInvalid,
            format!(
                "Неизвестная раскладка зеркала: {}",
                layout.map_or_else(|| "undefined".to_owned(), str::to_owned)
            ),
        ));
    }
    let base = base_url.trim_end_matches('/');
    Ok(format!("{base}/{}/{digest}", &digest[..2]))
}

/// `options` функций `plan` и `unavailableReason`.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlanOptions<'a> {
    /// Версия сборки: зеркало обязано быть для неё (`None` или пустая — не сверяется).
    pub version: Option<&'a str>,
    /// `allowedProviderTypes`: типы провайдеров, которыми можно пользоваться.
    pub allowed_provider_types: &'a [String],
}

/// Итог `plan`.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// `{usable: false, reason}`; `oversized` — первые пять слишком больших файлов.
    Unusable {
        reason: &'static str,
        oversized: Option<Vec<String>>,
    },
    /// `{usable: true, providers, totalBytes, files}`.
    Usable {
        providers: Vec<Value>,
        total_bytes: f64,
        files: usize,
    },
}

impl Plan {
    pub fn is_usable(&self) -> bool {
        matches!(self, Plan::Usable { .. })
    }

    pub fn to_json(&self) -> Value {
        match self {
            Plan::Unusable { reason, oversized } => {
                let mut out = Map::new();
                out.insert("usable".to_owned(), json!(false));
                out.insert("reason".to_owned(), json!(reason));
                if let Some(oversized) = oversized {
                    out.insert("oversized".to_owned(), json!(oversized));
                }
                Value::Object(out)
            }
            Plan::Usable {
                providers,
                total_bytes,
                files,
            } => json!({
                "usable": true,
                "providers": providers,
                "totalBytes": js_number(*total_bytes),
                "files": files,
            }),
        }
    }
}

fn unusable(reason: &'static str) -> Plan {
    Plan::Unusable {
        reason,
        oversized: None,
    }
}

/// `usableProviders(mirror, opts)`: провайдеры разрешённых типов.
fn usable_providers(mirror: &Value, options: &PlanOptions<'_>) -> Vec<Value> {
    jv::array(jv::prop(mirror, "providers"))
        .map(|list| {
            list.iter()
                .filter(|provider| {
                    jv::prop(provider, "type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| {
                            options.allowed_provider_types.iter().any(|a| a == kind)
                        })
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// `unavailableReason(mirror, options)`: почему зеркалом этой версии пользоваться нельзя вообще
/// (`None` — можно).
pub fn unavailable_reason(
    mirror: Option<&Value>,
    options: &PlanOptions<'_>,
) -> Option<&'static str> {
    let Some(mirror) = mirror.filter(|m| js::truthy(m)) else {
        return Some("MIRROR_NOT_DECLARED");
    };
    let layout_known = jv::prop(mirror, "layout")
        .and_then(Value::as_str)
        .is_some_and(|layout| LAYOUTS.contains(&layout));
    if !layout_known {
        return Some("MIRROR_LAYOUT_UNKNOWN");
    }
    if let Some(version) = options.version.filter(|v| !v.is_empty())
        && !jv::is_str(jv::prop(mirror, "version"), version)
    {
        return Some("MIRROR_VERSION_MISMATCH");
    }
    if usable_providers(mirror, options).is_empty() {
        return Some("MIRROR_PROVIDER_DISABLED");
    }
    None
}

/// `plan(mirror, entries, options)`: годится ли зеркало `mirror` (`repairMirror` канала) для
/// файлов `entries` (записи `files.json`).
pub fn plan(mirror: Option<&Value>, entries: &[Value], options: &PlanOptions<'_>) -> Plan {
    if let Some(reason) = unavailable_reason(mirror, options) {
        return unusable(reason);
    }
    let mirror = mirror.unwrap_or(&Value::Null);
    if entries.is_empty() {
        return unusable("MIRROR_NOTHING_TO_FETCH");
    }
    if entries.len() > MAX_FILES {
        return unusable("MIRROR_TOO_MANY_FILES");
    }
    // `Number(mirror.maxFileSize) || 0`.
    let max_file_size = jv::prop(mirror, "maxFileSize").map_or(0.0, jv::to_number);
    let max_file_size = if max_file_size.is_nan() {
        0.0
    } else {
        max_file_size
    };
    let oversized: Vec<&Value> = entries
        .iter()
        .filter(|entry| jv::as_number(jv::prop(entry, "size")) > max_file_size)
        .collect();
    if !oversized.is_empty() {
        return Plan::Unusable {
            reason: "MIRROR_FILE_TOO_LARGE",
            oversized: Some(
                oversized
                    .iter()
                    .take(5)
                    .map(|entry| jv::template(jv::prop(entry, "path")))
                    .collect(),
            ),
        };
    }
    Plan::Usable {
        providers: usable_providers(mirror, options),
        total_bytes: entries
            .iter()
            .map(|entry| jv::as_number(jv::prop(entry, "size")))
            .sum(),
        files: entries.len(),
    }
}

/// Прогресс `fetchInto`.
#[derive(Debug, Clone, PartialEq)]
pub struct FetchProgress {
    pub fetched: usize,
    pub total: usize,
    pub bytes: u64,
    pub total_bytes: f64,
    pub percent: f64,
}

impl FetchProgress {
    pub fn to_json(&self) -> Value {
        json!({
            "fetched": self.fetched,
            "total": self.total,
            "bytes": self.bytes,
            "totalBytes": js_number(self.total_bytes),
            "percent": js_number(self.percent),
        })
    }
}

/// `onProgress`.
pub type ProgressFn = Arc<dyn Fn(&FetchProgress) + Send + Sync>;

/// `options` метода `fetchInto`.
#[derive(Clone, Default)]
pub struct FetchOptions {
    /// Провайдеры по порядку (`{type, url, …}`); адрес — `url`.
    pub providers: Vec<Value>,
    /// Раскладка (`mirror.layout`).
    pub layout: String,
    /// `signal`: отмена загрузки.
    pub signal: Option<CancelHandle>,
    pub on_progress: Option<ProgressFn>,
}

/// Итог `fetchInto`: `{files, bytes}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Fetched {
    /// Пути файлов (`entry.path`) в порядке записей.
    pub files: Vec<String>,
    pub bytes: u64,
}

/// Параметры конструктора (`options`).
#[derive(Clone, Default)]
pub struct RepairMirrorOptions {
    /// `timeout`: `None` — [`DEFAULT_TIMEOUT_MS`].
    pub timeout: Option<Duration>,
    /// `maxAttempts`: `None` или 0 — [`DEFAULT_ATTEMPTS`].
    pub max_attempts: Option<u32>,
    pub http: Option<HttpClient>,
    /// Общий ограничитель скорости загрузок.
    pub rate_limiter: Option<Arc<DownloadRateLimiter>>,
}

/// `RepairMirrorClient`.
pub struct RepairMirrorClient {
    timeout: Duration,
    max_attempts: u32,
    http: HttpClient,
    rate_limiter: Option<Arc<DownloadRateLimiter>>,
}

/// Ошибка `fs.write` Node: без пути (`ENOSPC: no space left on device, write`).
fn write_error(error: &std::io::Error) -> RawError {
    let Some(code) = errno::code(error) else {
        return RawError::new(error.to_string());
    };
    RawError {
        message: format!("{code}: {}, write", errno::description(&code)),
        code: Some(code.into_owned()),
        syscall: Some("write".to_owned()),
        path: None,
        stderr: None,
    }
}

/// `renameSettled(from, to, signal)`: при EPERM/EBUSY одна повторная попытка через
/// [`LOCK_RETRY_MS`] (отмена в паузе — `CANCELLED`).
async fn rename_settled(from: &Path, to: &Path) -> Result<(), Failure> {
    let rename = || {
        std::fs::rename(from, to).map_err(|e| RawError::fs_with_dest(&e, "rename", from, Some(to)))
    };
    match rename() {
        Err(error) if matches!(error.code.as_deref(), Some("EPERM" | "EBUSY")) => {
            tokio::time::sleep(Duration::from_millis(LOCK_RETRY_MS)).await;
            Ok(rename()?)
        }
        other => Ok(other?),
    }
}

impl RepairMirrorClient {
    /// `new RepairMirrorClient(options)`.
    pub fn new(options: RepairMirrorOptions) -> RepairMirrorClient {
        RepairMirrorClient {
            timeout: options
                .timeout
                .filter(|t| !t.is_zero())
                .unwrap_or(Duration::from_millis(DEFAULT_TIMEOUT_MS)),
            max_attempts: options
                .max_attempts
                .filter(|&n| n != 0)
                .unwrap_or(DEFAULT_ATTEMPTS),
            http: options.http.unwrap_or_default(),
            rate_limiter: options.rate_limiter,
        }
    }

    /// `fetchOne(baseUrl, entry, layout, target, { signal, onBytes })`: файл `entry`
    /// (`{path, size, sha256}`) с зеркала `base_url` в `target` через `<target>.part`; размер и
    /// SHA-256 сверены. Только HTTPS (HTTP — на loopback). Итог — число байт.
    pub async fn fetch_one(
        &self,
        base_url: &str,
        entry: &Value,
        layout: &str,
        target: &Path,
        signal: Option<&CancelHandle>,
        on_bytes: &mut (dyn FnMut(u64) + Send),
    ) -> Result<u64, Failure> {
        let sha256 = jv::prop(entry, "sha256").and_then(Value::as_str);
        let url = http_client::parse_url(&file_url(base_url, sha256, Some(layout))?)?;
        let secure =
            url.scheme() == "https" || (url.scheme() == "http" && is_loopback(url_hostname(&url)));
        if !secure {
            return Err(DeliveryError::new(
                Code::InsecureRedirect,
                format!(
                    "Зеркало ремонта допускается только по HTTPS: {}:",
                    url.scheme()
                ),
            )
            .into());
        }

        let path = jv::template(jv::prop(entry, "path"));
        let part = PathBuf::from(format!("{}.part", target.to_string_lossy()));
        let result = tokio::select! {
            result = self.download(url.as_str(), entry, &path, &part, target, on_bytes) => result,
            () = until_cancelled(signal) => Err(cancelled().into()),
        };
        let Err(error) = result else {
            return result;
        };

        let _ = std::fs::remove_file(&part);
        if is_cancelled(signal) {
            return Err(cancelled().into());
        }
        let error = match error {
            Failure::Delivery(error) => return Err(error.into()),
            Failure::Raw(error) => error,
        };
        let details = json!({ "path": path });
        // Ошибка записи на диск — не сеть: повтор с другого зеркала не поможет.
        if let Some(code) = error.code.as_deref().and_then(disk_error) {
            return Err(DeliveryError::new(
                code,
                format!("Не удалось записать {path}: {}", error.message),
            )
            .with_details(details)
            .into());
        }
        Err(DeliveryError::new(
            Code::NetworkError,
            format!("Обрыв загрузки {path}: {}", error.message),
        )
        .with_details(details)
        .into())
    }

    /// Тело `try` у `fetchOne`: ответ потоком в `.part`, сверка, переименование.
    async fn download(
        &self,
        url: &str,
        entry: &Value,
        path: &str,
        part: &Path,
        target: &Path,
        on_bytes: &mut (dyn FnMut(u64) + Send),
    ) -> Result<u64, Failure> {
        let options = RequestOptions {
            timeout: Some(self.timeout),
            ..RequestOptions::default()
        };
        let mut response = self.http.request(url, &options).await?;
        if response.status() != 200 {
            let status = response.status();
            drop(response);
            return Err(DeliveryError::new(
                Code::RepairSourceMissing,
                format!("Файл {path} недоступен в зеркале ремонта: HTTP {status}"),
            )
            .with_details(json!({"statusCode": status, "path": path}))
            .into());
        }

        let size = jv::prop(entry, "size");
        let expected = size.and_then(jv::number).unwrap_or(f64::NAN);
        let mut file = tokio::fs::File::create(part)
            .await
            .map_err(|e| RawError::fs(&e, "open", part))?;
        let mut hash = Sha256::new();
        let mut received: u64 = 0;
        while let Some(chunk) = response.chunk().await? {
            if let Some(limiter) = &self.rate_limiter {
                limiter.acquire(chunk.len()).await;
            }
            received += chunk.len() as u64;
            if received as f64 > expected {
                return Err(DeliveryError::new(
                    Code::SizeMismatch,
                    format!("Файл {path} из зеркала длиннее манифеста"),
                )
                .with_details(json!({ "path": path }))
                .into());
            }
            hash.update(&chunk);
            on_bytes(chunk.len() as u64);
            file.write_all(&chunk).await.map_err(|e| write_error(&e))?;
        }
        // tokio дописывает в фоне: ошибка последней записи всплывает только здесь.
        file.flush().await.map_err(|e| write_error(&e))?;
        drop(file);

        if received as f64 != expected {
            return Err(DeliveryError::new(
                Code::SizeMismatch,
                format!(
                    "Размер файла {path} из зеркала {received}, ожидался {}",
                    jv::template(size)
                ),
            )
            .with_details(json!({ "path": path }))
            .into());
        }
        let digest = file_ops::hex(&hash.finalize());
        if !jv::is_str(jv::prop(entry, "sha256"), &digest) {
            let mut details = Map::new();
            details.insert("path".to_owned(), json!(path));
            if let Some(expected) = jv::prop(entry, "sha256") {
                details.insert("expected".to_owned(), expected.clone());
            }
            details.insert("actual".to_owned(), json!(digest));
            return Err(DeliveryError::new(
                Code::ChecksumMismatch,
                format!("Файл {path} из зеркала не совпал с манифестом"),
            )
            .with_details(Value::Object(details))
            .into());
        }

        rename_settled(part, target).await?;
        Ok(received)
    }

    /// `fetchInto(targetRoot, entries, options)`: все файлы `entries` — в `target_root` по их
    /// путям. Первая же неудача (все провайдеры и попытки) обрывает работу.
    pub async fn fetch_into(
        &self,
        target_root: &Path,
        entries: &[Value],
        options: &FetchOptions,
    ) -> Result<Fetched, Failure> {
        let signal = options.signal.as_ref();
        let mut progress = Progress {
            on_progress: options.on_progress.clone(),
            total: entries.len(),
            total_bytes: entries
                .iter()
                .map(|entry| jv::as_number(jv::prop(entry, "size")))
                .sum(),
            bytes: 0,
            reported_at: None,
        };

        node_fs::mkdir_recursive(target_root)?;
        let mut fetched = Vec::new();

        for entry in entries {
            let path = jv::template(jv::prop(entry, "path"));
            let target =
                file_ops::resolve_inside(target_root, &path, Some(&format!("Эталон \"{path}\"")))?;
            if let Some(dir) = target.parent() {
                node_fs::mkdir_recursive(dir)?;
            }

            let mut done = false;
            let mut last_error: Option<Failure> = None;
            'providers: for provider in &options.providers {
                let base_url = jv::template(jv::prop(provider, "url"));
                for _attempt in 0..self.max_attempts {
                    if is_cancelled(signal) {
                        return Err(cancelled().into());
                    }
                    let mut partial: u64 = 0;
                    let count = fetched.len();
                    let outcome = self
                        .fetch_one(
                            &base_url,
                            entry,
                            &options.layout,
                            &target,
                            signal,
                            &mut |bytes| {
                                partial += bytes;
                                progress.bytes += bytes;
                                progress.report(count, false);
                            },
                        )
                        .await;
                    match outcome {
                        Ok(_) => {
                            done = true;
                            break 'providers;
                        }
                        Err(error) => {
                            progress.bytes -= partial;
                            let code = match &error {
                                Failure::Delivery(e) => Some(e.code),
                                Failure::Raw(_) => None,
                            };
                            if matches!(
                                code,
                                Some(Code::Cancelled | Code::NoSpace | Code::PathNotWritable)
                            ) {
                                return Err(error);
                            }
                            last_error = Some(error);
                            if code == Some(Code::RepairSourceMissing) {
                                break;
                            }
                        }
                    }
                }
            }

            if !done {
                return Err(last_error.unwrap_or_else(|| {
                    DeliveryError::new(
                        Code::RepairSourceMissing,
                        format!("Файл {path} недоступен в зеркале"),
                    )
                    .with_details(json!({ "path": path }))
                    .into()
                }));
            }

            fetched.push(path);
            progress.report(fetched.len(), true);
        }
        Ok(Fetched {
            files: fetched,
            bytes: progress.bytes,
        })
    }
}

/// Счёт байт и `report(force)` у `fetchInto`.
struct Progress {
    on_progress: Option<ProgressFn>,
    total: usize,
    total_bytes: f64,
    bytes: u64,
    reported_at: Option<Instant>,
}

impl Progress {
    fn report(&mut self, fetched: usize, force: bool) {
        let Some(on_progress) = &self.on_progress else {
            return;
        };
        let now = Instant::now();
        if !force
            && self
                .reported_at
                .is_some_and(|at| now - at < Duration::from_millis(PROGRESS_EVERY_MS))
        {
            return;
        }
        self.reported_at = Some(now);
        let percent = if self.total_bytes != 0.0 {
            (self.bytes as f64 / self.total_bytes) * 100.0
        } else {
            (fetched as f64 / self.total as f64) * 100.0
        };
        on_progress(&FetchProgress {
            fetched,
            total: self.total,
            bytes: self.bytes,
            total_bytes: self.total_bytes,
            percent,
        });
    }
}
