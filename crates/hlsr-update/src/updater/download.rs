//! Загрузка установщика (`updater/core/download.js`): запас места 400 МиБ, `.part` с докачкой по
//! `Range`, попытка `n` берёт адрес `urls[min(n, len-1)]` (0 — hlsr.pro, 1–4 — GitHub), до 5
//! попыток; `user-agent: HLSR-Updater`, простой сокета 30 с, не больше 5 редиректов и каждый —
//! внутри allowlist; скорость — за окно 3 с. Коды и тексты ошибок — как у 0.4.7: тайм-аут
//! запроса Node превращал в `UPD_DOWNLOAD_FAILED` с текстом «таймаут».

use crate::cancel::CancelToken;
use crate::errors::{UpdCode, UpdaterError};
use crate::ports::{Clock, SystemClock};
use hlsr_core::js;
use std::collections::VecDeque;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

/// `DEFAULT_TIMEOUT_MS`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(30_000);
/// `MAX_REDIRECTS`.
pub const MAX_REDIRECTS: u32 = 5;
/// `SPEED_WINDOW_MS`.
pub const SPEED_WINDOW_MS: i64 = 3000;
/// `FREE_SPACE_RESERVE`.
pub const FREE_SPACE_RESERVE: u64 = 400 * 1024 * 1024;
/// `attempts` по умолчанию: попытки с 0 по 4.
pub const DEFAULT_ATTEMPTS: u32 = 4;
pub const USER_AGENT: &str = "HLSR-Updater";

// ------------------------------------------------------------------- скорость

/// `SpeedMeter`: байты в секунду за последние 3 с (не меньше двух точек).
pub struct SpeedMeter {
    clock: Arc<dyn Clock>,
    points: VecDeque<(i64, u64)>,
}

impl SpeedMeter {
    pub fn new(clock: Arc<dyn Clock>) -> SpeedMeter {
        SpeedMeter {
            clock,
            points: VecDeque::new(),
        }
    }

    pub fn push(&mut self, total_bytes: u64) {
        let ts = self.clock.now_ms();
        self.points.push_back((ts, total_bytes));
        while self.points.len() > 2 && ts - self.points[0].0 > SPEED_WINDOW_MS {
            self.points.pop_front();
        }
    }

    pub fn bytes_per_second(&self) -> f64 {
        let (Some(first), Some(last)) = (self.points.front(), self.points.back()) else {
            return 0.0;
        };
        if self.points.len() < 2 {
            return 0.0;
        }
        let seconds = (last.0 - first.0) as f64 / 1000.0;
        if seconds <= 0.0 {
            return 0.0;
        }
        ((last.1 as f64 - first.1 as f64) / seconds).max(0.0)
    }
}

// ------------------------------------------------------------------- место на диске

/// Свободное место для каталога (`freeSpaceBytes`); `None` — неизвестно (проверка пропускается).
pub trait FreeSpace: Send + Sync {
    fn free_bytes(&self, dir: &Path) -> Option<u64>;
}

/// Место, доступное пользователю на томе (`GetDiskFreeSpaceExW`). Не Windows — неизвестно.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemFreeSpace;

impl FreeSpace for SystemFreeSpace {
    #[cfg(windows)]
    fn free_bytes(&self, dir: &Path) -> Option<u64> {
        use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        use windows::core::HSTRING;
        let mut available = 0u64;
        // SAFETY: строка пути живёт до конца вызова; указатель — на локальную переменную.
        unsafe { GetDiskFreeSpaceExW(&HSTRING::from(dir), Some(&mut available), None, None) }
            .ok()
            .map(|()| available)
    }

    #[cfg(not(windows))]
    fn free_bytes(&self, _dir: &Path) -> Option<u64> {
        None
    }
}

/// Подделка: заданное число байт (или `None`).
#[derive(Debug, Clone, Copy)]
pub struct FixedFreeSpace(pub Option<u64>);

impl FreeSpace for FixedFreeSpace {
    fn free_bytes(&self, _dir: &Path) -> Option<u64> {
        self.0
    }
}

// ------------------------------------------------------------------- запрос

/// Какие адреса редиректов разрешены (`isAllowedUrl`).
pub type AllowUrl = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Ход загрузки: `{done, total, bytesPerSecond}`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DownloadProgress {
    pub done: u64,
    pub total: u64,
    pub bytes_per_second: f64,
}

impl DownloadProgress {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "done": self.done,
            "total": self.total,
            "bytesPerSecond": hlsr_core::json_text::js_number(self.bytes_per_second),
        })
    }
}

pub type OnProgress = Arc<dyn Fn(DownloadProgress) + Send + Sync>;

/// HTTP-клиент апдейтера: без редиректов (их ведёт [`request`]), без сжатия и прокси.
pub fn client() -> reqwest::Client {
    hlsr_core::http::client_builder()
        .user_agent(USER_AGENT)
        .build()
        .expect("клиент HTTP собирается из постоянных настроек")
}

fn failed(message: impl Into<String>) -> UpdaterError {
    UpdaterError::new(UpdCode::DownloadFailed, message)
}

fn network(error: &reqwest::Error) -> UpdaterError {
    use std::error::Error as _;
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    failed(text)
}

/// `request(url, {headers, timeout, isAllowedUrl})`: ответ 200 или 206 после редиректов.
pub async fn request(
    client: &reqwest::Client,
    url: &str,
    range_from: Option<u64>,
    timeout: Duration,
    is_allowed: &AllowUrl,
) -> Result<reqwest::Response, UpdaterError> {
    let mut current = Url::parse(url)
        .map_err(|_| UpdaterError::new(UpdCode::UrlNotAllowed, "неразбираемый URL"))?;
    let mut redirects_left = MAX_REDIRECTS;
    loop {
        let mut builder = client.get(current.clone());
        if let Some(from) = range_from {
            builder = builder.header(reqwest::header::RANGE, format!("bytes={from}-"));
        }
        let response = tokio::time::timeout(timeout, builder.send())
            .await
            .map_err(|_| failed("таймаут"))?
            .map_err(|error| network(&error))?;
        let status = response.status().as_u16();
        if (301..=308).contains(&status)
            && let Some(location) = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .filter(|location| !location.is_empty())
                .map(str::to_owned)
        {
            drop(response);
            if redirects_left == 0 {
                return Err(UpdaterError::new(
                    UpdCode::DownloadHttp,
                    "слишком много редиректов",
                ));
            }
            let next = current.join(&location).map_err(|_| failed("Invalid URL"))?;
            if !is_allowed(next.as_str()) {
                return Err(UpdaterError::new(
                    UpdCode::UrlNotAllowed,
                    "редирект вне allowlist",
                ));
            }
            current = next;
            redirects_left -= 1;
            continue;
        }
        if status != 200 && status != 206 {
            return Err(UpdaterError::new(
                UpdCode::DownloadHttp,
                format!("HTTP {status}"),
            ));
        }
        return Ok(response);
    }
}

// ------------------------------------------------------------------- загрузка

/// Параметры [`download`] (`options` 0.4.7).
#[derive(Clone)]
pub struct DownloadOptions {
    pub urls: Vec<String>,
    pub dir: PathBuf,
    pub filename: String,
    pub size: u64,
    /// Номер последней попытки (`attempts`, по умолчанию 4 — всего пять).
    pub attempts: u32,
    pub timeout: Duration,
    /// Запас места сверх размера.
    pub reserve: u64,
    pub is_allowed_url: AllowUrl,
    /// `isCancelled`.
    pub cancel: CancelToken,
    pub on_progress: Option<OnProgress>,
    pub free_space: Arc<dyn FreeSpace>,
    pub clock: Arc<dyn Clock>,
    pub client: reqwest::Client,
}

impl DownloadOptions {
    pub fn new(
        urls: Vec<String>,
        dir: PathBuf,
        filename: impl Into<String>,
        size: u64,
        is_allowed_url: AllowUrl,
    ) -> DownloadOptions {
        DownloadOptions {
            urls,
            dir,
            filename: filename.into(),
            size,
            attempts: DEFAULT_ATTEMPTS,
            timeout: DEFAULT_TIMEOUT,
            reserve: FREE_SPACE_RESERVE,
            is_allowed_url,
            cancel: CancelToken::new(),
            on_progress: None,
            free_space: Arc::new(SystemFreeSpace),
            clock: Arc::new(SystemClock),
            client: client(),
        }
    }
}

fn fs_failure(error: &std::io::Error, syscall: &str, path: &Path) -> UpdaterError {
    UpdaterError::new(
        UpdCode::InstallFailed,
        js::errno::fs_message(error, syscall, &path.to_string_lossy(), None),
    )
}

fn file_size(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|metadata| metadata.len())
}

/// Одна попытка: ответ пишется в `.part` (дописывается при 206 на запрос с `Range`).
async fn attempt(
    options: &DownloadOptions,
    url: &str,
    partial: &Path,
    done: &mut u64,
    meter: &mut SpeedMeter,
) -> Result<(), UpdaterError> {
    let total = options.size;
    let range_from = (*done > 0).then_some(*done);
    let response = request(
        &options.client,
        url,
        range_from,
        options.timeout,
        &options.is_allowed_url,
    )
    .await?;
    let resuming = *done > 0 && response.status().as_u16() == 206;
    if *done > 0 && !resuming {
        let _ = std::fs::remove_file(partial);
        *done = 0;
    }
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resuming)
        .truncate(!resuming)
        .open(partial)
        .map_err(|error| failed(error.to_string()))?;

    let mut response = response;
    loop {
        let chunk = tokio::select! {
            biased;
            () = options.cancel.cancelled() => {
                return Err(UpdaterError::new(UpdCode::Cancelled, "отменено"));
            }
            chunk = tokio::time::timeout(options.timeout, response.chunk()) => chunk,
        };
        let chunk = chunk
            .map_err(|_| failed("таймаут"))?
            .map_err(|error| network(&error))?;
        let Some(chunk) = chunk else {
            return Ok(());
        };
        if options.cancel.is_cancelled() {
            return Err(UpdaterError::new(UpdCode::Cancelled, "отменено"));
        }
        *done += chunk.len() as u64;
        if *done > total {
            return Err(UpdaterError::new(
                UpdCode::DownloadSize,
                "ответ длиннее размера",
            ));
        }
        out.write_all(&chunk)
            .map_err(|error| failed(error.to_string()))?;
        meter.push(*done);
        if let Some(on_progress) = &options.on_progress {
            on_progress(DownloadProgress {
                done: *done,
                total,
                bytes_per_second: meter.bytes_per_second(),
            });
        }
    }
}

/// `download(options)`: путь к скачанному `<dir>/<filename>`.
pub async fn download(options: DownloadOptions) -> Result<PathBuf, UpdaterError> {
    let target = options.dir.join(&options.filename);
    let mut partial = target.as_os_str().to_owned();
    partial.push(".part");
    let partial = PathBuf::from(partial);
    let total = options.size;

    std::fs::create_dir_all(&options.dir)
        .map_err(|error| fs_failure(&error, "mkdir", &options.dir))?;

    if let Some(free) = options.free_space.free_bytes(&options.dir)
        && (free as u128) < u128::from(total) + u128::from(options.reserve)
    {
        return Err(UpdaterError::new(
            UpdCode::NoSpace,
            format!("свободно {free} байт, нужно {total} + запас"),
        ));
    }

    if file_size(&target) == Some(total) {
        return Ok(target);
    }

    let mut meter = SpeedMeter::new(options.clock.clone());
    let mut last_error: Option<UpdaterError> = None;

    for attempt_no in 0..=options.attempts {
        if options.cancel.is_cancelled() {
            return Err(UpdaterError::new(
                UpdCode::Cancelled,
                "отменено пользователем",
            ));
        }
        let mut done = file_size(&partial).unwrap_or(0);
        if done > total {
            std::fs::remove_file(&partial)
                .map_err(|error| fs_failure(&error, "unlink", &partial))?;
            done = 0;
        }
        if done == total {
            break;
        }
        let Some(url) = options
            .urls
            .get((attempt_no as usize).min(options.urls.len().saturating_sub(1)))
        else {
            break;
        };
        match attempt(&options, url, &partial, &mut done, &mut meter).await {
            Ok(()) => {
                if done == total {
                    break;
                }
                last_error = Some(failed(format!("получено {done} из {total}")));
            }
            Err(error) if error.code == UpdCode::Cancelled => return Err(error),
            Err(error) => last_error = Some(error),
        }
    }

    let final_size = file_size(&partial).unwrap_or(0);
    if final_size != total {
        return Err(last_error.unwrap_or_else(|| {
            UpdaterError::new(
                UpdCode::DownloadSize,
                format!("получено {final_size} из {total}"),
            )
        }));
    }
    std::fs::rename(&partial, &target).map_err(|error| fs_failure(&error, "rename", &partial))?;
    Ok(target)
}
