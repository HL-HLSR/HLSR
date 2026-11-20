//! Загрузка установщика AppImage по подписанному каналу (`launcher-update/ArchiveDownload.js` 0.4.7):
//! адреса `installer.urls` по очереди, `User-Agent: HLSR-Linux-Updater`, простой соединения 30 с, не
//! больше 5 перенаправлений (каждый адрес — в allowlist), файл создаётся только новым (`wx 0600`) и
//! сверяется с каналом (размер, SHA-256, SHA-512). Неудачная попытка удаляет файл и берёт следующий адрес.

use crate::cancel::CancelToken;
use crate::channel::{self, VerifyOptions};
use crate::chunks::AllowUrl;
use crate::ports::{Clock, SystemClock};
use hlsr_core::js;
use serde_json::{Value, json};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

pub const USER_AGENT: &str = "HLSR-Linux-Updater";
/// Простой соединения (`timeout: 30000`).
pub const IDLE_TIMEOUT: Duration = Duration::from_millis(30_000);
pub const MAX_REDIRECTS: u32 = 5;
const REDIRECTS: [u16; 5] = [301, 302, 303, 307, 308];

/// Ход загрузки: `{transferred, total, percent, bytesPerSecond}`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DownloadProgress {
    pub transferred: u64,
    pub total: u64,
    pub percent: f64,
    pub bytes_per_second: f64,
}

impl DownloadProgress {
    pub fn to_json(&self) -> Value {
        json!({
            "transferred": self.transferred,
            "total": self.total,
            "percent": hlsr_core::json_text::js_number(self.percent),
            "bytesPerSecond": hlsr_core::json_text::js_number(self.bytes_per_second),
        })
    }
}

pub type OnDownloadProgress = Arc<dyn Fn(DownloadProgress) + Send + Sync>;

/// Адрес запроса вместо подписанного (QA-сборка: тот же файл с loopback, `transport.get` в local-qa).
pub type Rewrite = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// `options` загрузки.
#[derive(Clone)]
pub struct DownloadOptions {
    /// `isAllowedUrl` (в выпуске — allowlist канала).
    pub is_allowed: AllowUrl,
    pub cancel: Option<CancelToken>,
    pub on_progress: Option<OnDownloadProgress>,
    pub rewrite: Option<Rewrite>,
    pub idle_timeout: Duration,
    pub clock: Arc<dyn Clock>,
}

impl Default for DownloadOptions {
    fn default() -> DownloadOptions {
        DownloadOptions {
            is_allowed: crate::chunks::production_allow(),
            cancel: None,
            on_progress: None,
            rewrite: None,
            idle_timeout: IDLE_TIMEOUT,
            clock: Arc::new(SystemClock),
        }
    }
}

/// `AbortError` Node.
const ABORTED: &str = "This operation was aborted";
/// `request.on("timeout")`: простой соединения (и тайм-аут соединения агента Node).
const TIMEOUT: &str = "Таймаут скачивания";

/// Клиент загрузки установщика.
#[derive(Clone)]
pub struct ArchiveDownload {
    client: reqwest::Client,
}

impl Default for ArchiveDownload {
    fn default() -> ArchiveDownload {
        ArchiveDownload::new()
    }
}

impl ArchiveDownload {
    pub fn new() -> ArchiveDownload {
        ArchiveDownload {
            client: hlsr_core::http::client_builder()
                .user_agent(USER_AGENT)
                .build()
                .expect("клиент HTTP собирается из постоянных настроек"),
        }
    }

    /// `responseFor(url)`: ответ 200 по адресу из allowlist (перенаправления — тоже из него).
    async fn response_for(
        &self,
        url: &str,
        options: &DownloadOptions,
    ) -> Result<reqwest::Response, String> {
        let mut current = url.to_owned();
        let mut redirects = MAX_REDIRECTS;
        loop {
            if !(options.is_allowed)(&current) {
                return Err("Недоверенный адрес установщика".to_owned());
            }
            let request_url = match &options.rewrite {
                Some(rewrite) => rewrite(&current),
                None => current.clone(),
            };
            let send = self.client.get(&request_url).send();
            let response = tokio::time::timeout(options.idle_timeout, send)
                .await
                .map_err(|_| TIMEOUT.to_owned())?
                .map_err(|error| request_error(&error, &request_url))?;
            let status = response.status().as_u16();
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            if let (true, Some(location)) = (REDIRECTS.contains(&status), location) {
                drop(response);
                if redirects == 0 {
                    return Err("Слишком много перенаправлений".to_owned());
                }
                let base = Url::parse(&current).map_err(|_| "Invalid URL".to_owned())?;
                current = base
                    .join(&location)
                    .map_err(|_| "Invalid URL".to_owned())?
                    .to_string();
                redirects -= 1;
                continue;
            }
            if status != 200 {
                return Err(format!("HTTP {status}"));
            }
            return Ok(response);
        }
    }

    async fn attempt(
        &self,
        url: &str,
        installer: &Value,
        target: &Path,
        options: &DownloadOptions,
    ) -> Result<(), String> {
        let mut response = match &options.cancel {
            Some(cancel) => tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(ABORTED.to_owned()),
                response = self.response_for(url, options) => response?,
            },
            None => self.response_for(url, options).await?,
        };
        let size = installer["size"].as_f64().unwrap_or(0.0);
        let mut file = open_new(target).map_err(|error| super::fs_error(&error, "open", target))?;
        let started = options.clock.now_ms();
        let mut transferred: u64 = 0;
        loop {
            let next = tokio::time::timeout(options.idle_timeout, response.chunk());
            let chunk = match &options.cancel {
                Some(cancel) => tokio::select! {
                    biased;
                    () = cancel.cancelled() => return Err(ABORTED.to_owned()),
                    chunk = next => chunk,
                },
                None => next.await,
            };
            let Some(chunk) = chunk
                .map_err(|_| TIMEOUT.to_owned())?
                .map_err(|error| body_error(&error))?
            else {
                break;
            };
            transferred += chunk.len() as u64;
            if transferred as f64 > size {
                return Err("Установщик больше подписанного размера".to_owned());
            }
            if let Some(on_progress) = &options.on_progress {
                let elapsed = (options.clock.now_ms() - started).max(1) as f64;
                on_progress(DownloadProgress {
                    transferred,
                    total: size as u64,
                    percent: transferred as f64 / size * 100.0,
                    bytes_per_second: transferred as f64 * 1000.0 / elapsed,
                });
            }
            file.write_all(&chunk)
                .map_err(|error| super::fs_error(&error, "write", target))?;
        }
        drop(file);
        let verdict = channel::verify_installer_stream(
            target,
            installer,
            &VerifyOptions {
                max_bytes: None,
                cancel: options.cancel.clone(),
            },
        )
        .await;
        if options
            .cancel
            .as_ref()
            .is_some_and(CancelToken::is_cancelled)
        {
            return Err(ABORTED.to_owned());
        }
        if !verdict.ok {
            return Err(verdict.problems.join("; "));
        }
        Ok(())
    }

    /// `download(manifest, directory)`: установщик канала в `<каталог>/<installer.filename>`.
    pub async fn download(
        &self,
        manifest: &Value,
        directory: &Path,
        options: &DownloadOptions,
    ) -> Result<PathBuf, String> {
        super::make_dirs(directory, 0o777)
            .map_err(|error| super::fs_error(&error, "mkdir", directory))?;
        let installer = &manifest["installer"];
        let filename = js::to_string(&installer["filename"]);
        let urls: Vec<String> = installer["urls"]
            .as_array()
            .map(|urls| urls.iter().map(js::to_string).collect())
            .unwrap_or_default();
        let mut last_error = None;
        for url in urls {
            if options
                .cancel
                .as_ref()
                .is_some_and(CancelToken::is_cancelled)
            {
                return Err("Скачивание отменено".to_owned());
            }
            let target = directory.join(&filename);
            match self.attempt(&url, installer, &target, options).await {
                Ok(()) => return Ok(target),
                Err(error) => {
                    last_error = Some(error);
                    let _ = std::fs::remove_file(&target);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| "Нет доступных зеркал установщика".to_owned()))
    }
}

/// `createWriteStream(target, {flags: "wx", mode: 0o600})`.
fn open_new(target: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(target)
}

/// Ошибка запроса словами Node (`getaddrinfo ENOTFOUND <имя>`, `connect ECONNREFUSED <узел>:<порт>`,
/// `socket hang up`): по тексту `errorCode` службы выбирает тот же класс, что 0.4.7 (`ENOTFOUND`,
/// `ECONN…` — `NETWORK`). Тайм-аут соединения (`connect_timeout` клиента) — `request.on("timeout")`.
fn request_error(error: &reqwest::Error, url: &str) -> String {
    if error.is_timeout() {
        return TIMEOUT.to_owned();
    }
    crate::http::node_net_error(error, url).message
}

/// Обрыв посреди тела: `aborted` (`pipeline` 0.4.7 получает его от ответа).
fn body_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return TIMEOUT.to_owned();
    }
    crate::http::node_body_error().message
}
