//! Загрузка с докачкой (`GameDownloadManager.js`): одна загрузка за раз; `.part` и
//! `.part.json` рядом с итоговым файлом; докачка по `Range`; sha256 потоком; до пяти попыток
//! с паузой 2ⁿ⁻¹ секунд ±20 % (не больше 30 с); пауза и отмена; запас места 256 МБ;
//! события `state` и `progress` не чаще раза в 300 мс.
//!
//! События 0.4.7 — `EventEmitter`: слушатель вызывается сразу, в той же точке кода, где
//! событие рождается (`GameDeliveryService` читает его состояние сразу после вызова
//! `start`). Поэтому слушатели здесь — функции ([`GameDownloadManager::on_event`]), а не
//! очередь. Отмена и пауза (`AbortController` и `response.destroy()` в 0.4.7) — флаги и
//! сигнал, по которому сбрасывается ожидание ответа, куска тела, ограничителя скорости и
//! паузы между попытками; запись куска на диск не прерывается, поэтому на любой точке
//! останова `.part` и счётчик байт согласованы.

use crate::atomic::fd_error;
use crate::download_rate_limiter::DownloadRateLimiter;
use crate::errors::{Code, DeliveryError, Failure, RawError, to_delivery_error};
use crate::file_ops::hex;
use crate::free_space::FreeSpaceCache;
use crate::http_client::{HttpClient, RequestOptions, Response, url_host};
use crate::mirror_runner::{Clock, system_clock};
use crate::node_fs;
use crate::paths::{assert_inside, remove_within_zone};
use crate::ports::{FreeSpace, Logger, SystemFreeSpace, TracingLogger};
use crate::providers::Provider;
use bytes::Bytes;
use hlsr_core::js;
use hlsr_core::js::fs::read_dir_names;
use hlsr_core::js::path::NodePath;
use hlsr_core::json_text::{js_number, json_parse, stringify};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::watch;

/// Версия формата `.part.json`.
pub const STATE_VERSION: u32 = 1;
pub const DEFAULT_MAX_ATTEMPTS: u32 = 5;
pub const DEFAULT_BACKOFF_MS: u64 = 1000;
pub const MAX_BACKOFF_MS: u64 = 30_000;
pub const PROGRESS_INTERVAL_MS: i64 = 300;
pub const SPEED_SMOOTHING: f64 = 0.3;
/// Запас места сверх размера архива.
pub const FREE_SPACE_RESERVE: u64 = 256 * 1024 * 1024;

/// Коды, после которых загрузка повторяется (`RETRYABLE_CODES`).
pub const RETRYABLE_CODES: &[Code] = &[
    Code::NetworkError,
    Code::NetworkTimeout,
    Code::ProviderLinkFailed,
    Code::ProviderForbidden,
    Code::ContentLengthMismatch,
    Code::RangeNotSupported,
];

/// Блок чтения при пересчёте хеша уже скачанной части.
const REHASH_BLOCK: usize = 4 * 1024 * 1024;

/// Случайное число из [0, 1) — `Math.random()`.
pub type Random = Arc<dyn Fn() -> f64 + Send + Sync>;

/// Слушатель событий загрузки.
pub type Listener = Arc<dyn Fn(&DownloadEvent) + Send + Sync>;

pub fn system_random() -> Random {
    Arc::new(|| {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).expect("системный генератор случайных чисел недоступен");
        (u64::from_le_bytes(bytes) >> 11) as f64 / (1u64 << 53) as f64
    })
}

/// Параметры менеджера (`options` конструктора).
#[derive(Clone, Default)]
pub struct DownloadOptions {
    pub http: Option<HttpClient>,
    pub logger: Option<Arc<dyn Logger>>,
    /// Простой соединения; `None` или 0 — 30 с.
    pub timeout: Option<Duration>,
    /// 0 и `None` — 5.
    pub max_attempts: Option<u32>,
    /// Основа паузы между попытками, мс; `None` — 1000.
    pub backoff_base_ms: Option<u64>,
    /// `None` — 256 МБ.
    pub free_space_reserve: Option<u64>,
    pub free_space: Option<Arc<dyn FreeSpace>>,
    pub rate_limiter: Option<Arc<DownloadRateLimiter>>,
    /// Часы (`Date.now()`); для тестов.
    pub clock: Option<Clock>,
    /// `Math.random()` для разброса паузы; для тестов.
    pub random: Option<Random>,
}

/// Запрос `start`.
#[derive(Clone)]
pub struct DownloadRequest {
    pub game_id: String,
    pub provider: Arc<dyn Provider>,
    pub filename: String,
    /// Нет или 0 — размер неизвестен (место и длина не проверяются).
    pub expected_size: Option<u64>,
    /// Нет или пустой — хеш не проверяется.
    pub expected_sha256: Option<String>,
}

/// Файлы загрузки (`partPaths`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartPaths {
    pub dir: PathBuf,
    pub part: PathBuf,
    pub state: PathBuf,
    pub target: PathBuf,
}

/// Чем кончилась загрузка (`start`): `{status, …}`.
#[derive(Debug, Clone, PartialEq)]
pub enum DownloadOutcome {
    Completed {
        file_path: PathBuf,
        sha256: String,
        size: u64,
        network_bytes: u64,
        duration_ms: u64,
    },
    Cancelled {
        network_bytes: u64,
    },
    Paused {
        bytes_downloaded: u64,
    },
}

impl DownloadOutcome {
    /// `status`: `completed`, `cancelled`, `paused`.
    pub fn status(&self) -> &'static str {
        match self {
            DownloadOutcome::Completed { .. } => "completed",
            DownloadOutcome::Cancelled { .. } => "cancelled",
            DownloadOutcome::Paused { .. } => "paused",
        }
    }

    pub fn to_json(&self) -> Value {
        match self {
            DownloadOutcome::Completed {
                file_path,
                sha256,
                size,
                network_bytes,
                duration_ms,
            } => json!({
                "status": "completed",
                "filePath": file_path.to_string_lossy(),
                "sha256": sha256,
                "size": size,
                "networkBytes": network_bytes,
                "durationMs": duration_ms,
            }),
            DownloadOutcome::Cancelled { network_bytes } => {
                json!({ "status": "cancelled", "networkBytes": network_bytes })
            }
            DownloadOutcome::Paused { bytes_downloaded } => {
                json!({ "status": "paused", "bytesDownloaded": bytes_downloaded })
            }
        }
    }
}

/// Загрузка, которую можно продолжить (`findResumable`); поля, которых нет в файле
/// состояния, отсутствуют (`undefined` в 0.4.7).
#[derive(Debug, Clone, PartialEq)]
pub struct Resumable {
    pub game_id: Option<Value>,
    pub filename: Option<Value>,
    pub expected_size: Option<Value>,
    pub bytes_on_disk: u64,
    pub bytes_downloaded: u64,
    pub updated_at: Option<Value>,
}

impl Resumable {
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        let mut put = |key: &str, value: &Option<Value>| {
            if let Some(value) = value {
                out.insert(key.to_owned(), value.clone());
            }
        };
        put("gameId", &self.game_id);
        put("filename", &self.filename);
        put("expectedSize", &self.expected_size);
        out.insert("bytesOnDisk".to_owned(), json!(self.bytes_on_disk));
        out.insert("bytesDownloaded".to_owned(), json!(self.bytes_downloaded));
        if let Some(updated) = &self.updated_at {
            out.insert("updatedAt".to_owned(), updated.clone());
        }
        Value::Object(out)
    }
}

/// Поля `.part.json` (`writeState`); отсутствующее поле в файл не попадает.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StateFile {
    pub game_id: Option<String>,
    pub filename: Option<String>,
    pub expected_size: Option<u64>,
    pub expected_sha256: Option<String>,
    pub provider_id: Option<String>,
    pub bytes_downloaded: Option<u64>,
}

/// Имя состояния в событии `state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateName {
    Preparing,
    Resuming,
    Downloading,
    Retrying,
    Paused,
    Cancelled,
    Downloaded,
    Error,
}

impl StateName {
    pub fn as_str(self) -> &'static str {
        match self {
            StateName::Preparing => "preparing",
            StateName::Resuming => "resuming",
            StateName::Downloading => "downloading",
            StateName::Retrying => "retrying",
            StateName::Paused => "paused",
            StateName::Cancelled => "cancelled",
            StateName::Downloaded => "downloaded",
            StateName::Error => "error",
        }
    }
}

/// Событие `progress` и основа события `state` (`progressOf`).
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadProgress {
    pub game_id: String,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub percent: f64,
    pub speed_bps: u64,
    pub eta_seconds: Option<u64>,
    pub provider_id: Option<String>,
    pub range_supported: bool,
}

impl DownloadProgress {
    fn fill(&self, out: &mut Map<String, Value>) {
        out.insert("gameId".to_owned(), json!(self.game_id));
        out.insert("bytesDownloaded".to_owned(), json!(self.bytes_downloaded));
        out.insert("totalBytes".to_owned(), json!(self.total_bytes));
        out.insert("percent".to_owned(), js_number(self.percent));
        out.insert("speedBps".to_owned(), json!(self.speed_bps));
        out.insert("etaSeconds".to_owned(), json!(self.eta_seconds));
        out.insert("providerId".to_owned(), json!(self.provider_id));
        out.insert("rangeSupported".to_owned(), json!(self.range_supported));
    }

    /// Объект события, как его видит слушатель 0.4.7.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        self.fill(&mut out);
        Value::Object(out)
    }
}

/// Дополнительные поля события `state`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StateExtra {
    pub attempt: Option<u32>,
    pub max_attempts: Option<u32>,
    pub retry_in_ms: Option<u64>,
    /// `toPublic()` ошибки.
    pub error: Option<Value>,
    pub file_path: Option<PathBuf>,
    pub sha256: Option<String>,
}

/// Событие `state`.
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadState {
    pub state: StateName,
    pub progress: DownloadProgress,
    pub extra: StateExtra,
}

impl DownloadState {
    /// Объект события: `{state, …progressOf, …extra}`.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("state".to_owned(), json!(self.state.as_str()));
        self.progress.fill(&mut out);
        let extra = &self.extra;
        if let Some(attempt) = extra.attempt {
            out.insert("attempt".to_owned(), json!(attempt));
        }
        if let Some(max) = extra.max_attempts {
            out.insert("maxAttempts".to_owned(), json!(max));
        }
        if let Some(ms) = extra.retry_in_ms {
            out.insert("retryInMs".to_owned(), json!(ms));
        }
        if let Some(error) = &extra.error {
            out.insert("error".to_owned(), error.clone());
        }
        if let Some(path) = &extra.file_path {
            out.insert("filePath".to_owned(), json!(path.to_string_lossy()));
        }
        if let Some(sha256) = &extra.sha256 {
            out.insert("sha256".to_owned(), json!(sha256));
        }
        Value::Object(out)
    }
}

/// Событие менеджера: `progress` или `state`.
#[derive(Debug, Clone, PartialEq)]
pub enum DownloadEvent {
    Progress(DownloadProgress),
    State(DownloadState),
}

/// Остановка, общая для `pause`, `cancel` и идущей загрузки.
struct Control {
    game_id: String,
    pause: AtomicBool,
    cancel: AtomicBool,
    abort: watch::Sender<bool>,
}

impl Control {
    fn new(game_id: &str) -> Control {
        Control {
            game_id: game_id.to_owned(),
            pause: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            abort: watch::channel(false).0,
        }
    }

    fn is_aborted(&self) -> bool {
        *self.abort.borrow()
    }

    fn abort(&self) {
        self.abort.send_replace(true);
    }

    async fn aborted(&self) {
        let mut open = self.abort.subscribe();
        let _ = open.wait_for(|aborted| *aborted).await;
    }

    fn stopped(&self) -> bool {
        self.pause.load(Ordering::SeqCst) || self.cancel.load(Ordering::SeqCst)
    }

    /// `signal.throwIfAborted()`.
    fn throw_if_aborted(&self) -> Result<(), Failure> {
        if self.is_aborted() {
            Err(stopped_error().into())
        } else {
            Ok(())
        }
    }

    /// Дождаться `future`, пока загрузку не остановили.
    async fn until_aborted<T>(&self, future: impl Future<Output = T>) -> Result<T, Failure> {
        self.throw_if_aborted()?;
        tokio::select! {
            biased;
            () = self.aborted() => Err(stopped_error().into()),
            out = future => Ok(out),
        }
    }
}

/// Ошибка остановленной загрузки; наружу не уходит: `run` смотрит на флаги.
fn stopped_error() -> DeliveryError {
    DeliveryError::new(Code::Cancelled, "Загрузка остановлена")
}

/// Загрузка, идущая сейчас; сбрасывает `active` и при сбросе будущего `start`.
struct ActiveGuard<'a> {
    active: &'a Mutex<Option<Arc<Control>>>,
    control: Arc<Control>,
}

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        let mut active = self.active.lock().unwrap_or_else(|p| p.into_inner());
        if active
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &self.control))
        {
            *active = None;
        }
    }
}

/// Состояние одной загрузки (`ctx`).
struct Ctx {
    game_id: String,
    provider: Arc<dyn Provider>,
    provider_id: Option<String>,
    filename: String,
    expected_size: Option<u64>,
    expected_sha256: Option<String>,
    files: PartPaths,
    bytes_downloaded: u64,
    network_bytes: u64,
    hash: Option<Sha256>,
    control: Arc<Control>,
    started_at: i64,
    speed_bps: f64,
    last_sample: Option<(i64, u64)>,
    last_emit: i64,
    range_supported: bool,
    attempt: u32,
}

impl Ctx {
    /// Ожидаемый размер, если он известен (`ctx.expectedSize` истинно).
    fn known_size(&self) -> Option<u64> {
        self.expected_size.filter(|&size| size != 0)
    }
}

pub struct GameDownloadManager {
    downloads: PathBuf,
    http: HttpClient,
    logger: Arc<dyn Logger>,
    timeout: Option<Duration>,
    max_attempts: u32,
    backoff_base_ms: u64,
    free_space_reserve: u64,
    free_space: Arc<dyn FreeSpace>,
    rate_limiter: Option<Arc<DownloadRateLimiter>>,
    clock: Clock,
    random: Random,
    listeners: Mutex<Vec<Listener>>,
    active: Mutex<Option<Arc<Control>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Число JS в строке: `${x}`.
fn js_num(number: f64) -> String {
    js::to_string(&js_number(number))
}

/// `.part` открыт на запись с `start` (`fs.createWriteStream(part, {flags, start})`):
/// с `start > 0` файл должен быть (`r+`), иначе создаётся или обрезается (`w`).
struct PartWriter {
    file: Option<std::fs::File>,
}

impl PartWriter {
    fn open(path: &Path, start: u64) -> Result<PartWriter, RawError> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true);
        if start == 0 {
            options.create(true).truncate(true);
        }
        let mut file = options
            .open(path)
            .map_err(|error| RawError::fs(&error, "open", path))?;
        if start > 0 {
            file.seek(SeekFrom::Start(start))
                .map_err(|error| fd_error(&error, "write"))?;
        }
        Ok(PartWriter { file: Some(file) })
    }

    /// Записать кусок; запись не прерывается остановкой загрузки.
    async fn write(&mut self, chunk: Bytes) -> Result<(), RawError> {
        let mut file = self.file.take().expect("файл .part на месте");
        let (file, result) = tokio::task::spawn_blocking(move || {
            let result = file.write_all(&chunk);
            (file, result)
        })
        .await
        .expect("запись в файл не паникует");
        self.file = Some(file);
        result.map_err(|error| fd_error(&error, "write"))
    }
}

impl GameDownloadManager {
    /// `new GameDownloadManager({paths: {downloads}, …})`.
    pub fn new(downloads: impl Into<PathBuf>, options: DownloadOptions) -> GameDownloadManager {
        let free_space = options.free_space.unwrap_or_else(|| {
            Arc::new(FreeSpaceCache::new(Arc::new(SystemFreeSpace))) as Arc<dyn FreeSpace>
        });
        GameDownloadManager {
            downloads: downloads.into(),
            http: options.http.unwrap_or_default(),
            logger: options.logger.unwrap_or_else(|| Arc::new(TracingLogger)),
            timeout: options.timeout,
            max_attempts: options
                .max_attempts
                .filter(|&n| n != 0)
                .unwrap_or(DEFAULT_MAX_ATTEMPTS),
            backoff_base_ms: options.backoff_base_ms.unwrap_or(DEFAULT_BACKOFF_MS),
            free_space_reserve: options.free_space_reserve.unwrap_or(FREE_SPACE_RESERVE),
            free_space,
            rate_limiter: options.rate_limiter,
            clock: options.clock.unwrap_or_else(system_clock),
            random: options.random.unwrap_or_else(system_random),
            listeners: Mutex::new(Vec::new()),
            active: Mutex::new(None),
        }
    }

    /// `on("progress" | "state", listener)`: слушатель вызывается сразу, при рождении события.
    pub fn on_event(&self, listener: impl Fn(&DownloadEvent) + Send + Sync + 'static) {
        lock(&self.listeners).push(Arc::new(listener));
    }

    /// `isBusy()`.
    pub fn is_busy(&self) -> bool {
        lock(&self.active).is_some()
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    fn emit(&self, event: &DownloadEvent) {
        let listeners: Vec<Listener> = lock(&self.listeners).clone();
        for listener in listeners {
            listener(event);
        }
    }

    // ------------------------------------------------------------------ пути и файлы

    fn download_dir(&self, game_id: &str) -> Result<PathBuf, DeliveryError> {
        let downloads = self.downloads.to_string_lossy();
        let joined = NodePath::native().join(&[&downloads, game_id]);
        assert_inside(&self.downloads, Path::new(&joined), Some("Папка загрузки"))
    }

    /// `partPaths(gameId, filename)`.
    pub fn part_paths(&self, game_id: &str, filename: &str) -> Result<PartPaths, DeliveryError> {
        let path = NodePath::native();
        let dir = self.download_dir(game_id)?;
        let dir_text = dir.to_string_lossy().into_owned();
        let base = path.basename(filename);
        let part = assert_inside(
            &dir,
            Path::new(&path.join(&[&dir_text, &format!("{base}.part")])),
            Some("Файл загрузки"),
        )?;
        let mut state = part.clone().into_os_string();
        state.push(".json");
        Ok(PartPaths {
            target: PathBuf::from(path.join(&[&dir_text, &base])),
            state: PathBuf::from(state),
            part,
            dir,
        })
    }

    /// `readState(statePath)`: файл состояния нашей версии или `None`.
    pub fn read_state(&self, state_path: &Path) -> Option<Value> {
        let text = node_fs::read_text(state_path, crate::game_binds::Encoding::Utf8).ok()?;
        let data = json_parse(&text).ok()?;
        let version = data.get("version")?.as_f64()?;
        (version == f64::from(STATE_VERSION)).then_some(data)
    }

    /// `writeState(statePath, state)`: JSON с отступом в два пробела, без перевода строки
    /// в конце, не атомарно (`fs.writeFileSync`). `updatedAt` — по часам менеджера.
    pub fn write_state(&self, state_path: &Path, state: &StateFile) -> Result<(), RawError> {
        let mut out = Map::new();
        out.insert("version".to_owned(), json!(STATE_VERSION));
        let mut put = |key: &str, value: Option<Value>| {
            if let Some(value) = value {
                out.insert(key.to_owned(), value);
            }
        };
        put("gameId", state.game_id.as_ref().map(|v| json!(v)));
        put("filename", state.filename.as_ref().map(|v| json!(v)));
        put("expectedSize", state.expected_size.map(|v| json!(v)));
        put(
            "expectedSha256",
            state.expected_sha256.as_ref().map(|v| json!(v)),
        );
        put("providerId", state.provider_id.as_ref().map(|v| json!(v)));
        put("bytesDownloaded", state.bytes_downloaded.map(|v| json!(v)));
        let at = SystemTime::UNIX_EPOCH + Duration::from_millis(self.now().max(0) as u64);
        out.insert("updatedAt".to_owned(), json!(js::iso_string(at)));
        node_fs::write_bytes(state_path, stringify(&Value::Object(out), "  ").as_bytes())
    }

    /// `findResumable(gameId, filename)`.
    pub fn find_resumable(
        &self,
        game_id: &str,
        filename: &str,
    ) -> Result<Option<Resumable>, DeliveryError> {
        let files = self.part_paths(game_id, filename)?;
        let Some(saved) = self.read_state(&files.state) else {
            return Ok(None);
        };
        let Ok(metadata) = std::fs::metadata(&files.part) else {
            return Ok(None);
        };
        let size = metadata.len();
        Ok(Some(Resumable {
            game_id: saved.get("gameId").cloned(),
            filename: saved.get("filename").cloned(),
            expected_size: saved.get("expectedSize").cloned(),
            bytes_on_disk: size,
            bytes_downloaded: saved_bytes(saved.get("bytesDownloaded"), size),
            updated_at: saved.get("updatedAt").cloned(),
        }))
    }

    /// `discard(gameId, filename)`: убрать `.part` и `.part.json`.
    pub fn discard(&self, game_id: &str, filename: &str) -> Result<(), Failure> {
        let files = self.part_paths(game_id, filename)?;
        for file in [&files.part, &files.state] {
            if file.exists() {
                remove_within_zone(&self.downloads, file)?;
            }
        }
        Ok(())
    }

    /// `discardPaused(gameId, keep)`: убрать все остановленные загрузки игры (по файлам
    /// состояния в её каталоге); `true`, если что-то убрано. `keep(filename)` — чужие загрузки в
    /// той же папке (эпизоды Ghosting), их отмена игры не трогает.
    pub fn discard_paused(
        &self,
        game_id: &str,
        keep: Option<&dyn Fn(&str) -> bool>,
    ) -> Result<bool, Failure> {
        let dir = self.download_dir(game_id)?;
        if !dir.exists() {
            return Ok(false);
        }
        let names = read_dir_names(&dir).map_err(|error| RawError::fs(&error, "scandir", &dir))?;
        let mut discarded = false;
        for name in names {
            if !name.ends_with(".part.json") {
                continue;
            }
            let Some(saved) = self.read_state(&dir.join(&name)) else {
                continue;
            };
            if saved.get("gameId").and_then(Value::as_str) != Some(game_id) {
                continue;
            }
            let Some(filename) = saved.get("filename").and_then(Value::as_str) else {
                continue;
            };
            if keep.is_some_and(|keep| keep(filename)) {
                continue;
            }
            self.discard(game_id, filename)?;
            discarded = true;
        }
        Ok(discarded)
    }

    // ------------------------------------------------------------------ пауза и отмена

    /// `pause()`: `true`, если загрузка идёт.
    pub fn pause(&self) -> bool {
        let active = lock(&self.active);
        let Some(control) = active.as_ref() else {
            return false;
        };
        control.pause.store(true, Ordering::SeqCst);
        control.abort();
        true
    }

    /// `cancel()`: `true`, если загрузка идёт.
    pub fn cancel(&self) -> bool {
        let active = lock(&self.active);
        let Some(control) = active.as_ref() else {
            return false;
        };
        control.cancel.store(true, Ordering::SeqCst);
        control.abort();
        true
    }

    // ------------------------------------------------------------------ загрузка

    /// `start(request)`. Ошибки до начала загрузки (`ALREADY_RUNNING`, `PATH_INVALID`,
    /// `NO_SPACE`, папка не создана) уходят как есть и без события; ошибка самой
    /// загрузки — после события `state` с `error`.
    pub async fn start(&self, request: DownloadRequest) -> Result<DownloadOutcome, Failure> {
        let DownloadRequest {
            game_id,
            provider,
            filename,
            expected_size,
            expected_sha256,
        } = request;

        let (control, files) = {
            let mut active = lock(&self.active);
            if let Some(current) = active.as_ref() {
                return Err(DeliveryError::new(
                    Code::AlreadyRunning,
                    "Уже идёт загрузка другой сборки",
                )
                .with_details(json!({ "gameId": current.game_id }))
                .into());
            }
            let files = self.part_paths(&game_id, &filename)?;
            let parent = NodePath::native().dirname(&files.dir.to_string_lossy());
            self.check_free_space(Path::new(&parent), expected_size)?;
            std::fs::create_dir_all(&files.dir)
                .map_err(|error| RawError::fs(&error, "mkdir", &files.dir))?;
            let control = Arc::new(Control::new(&game_id));
            *active = Some(control.clone());
            (control, files)
        };
        let _active = ActiveGuard {
            active: &self.active,
            control: control.clone(),
        };

        let mut ctx = Ctx {
            provider_id: provider.id().map(str::to_owned),
            filename: NodePath::native().basename(&filename),
            game_id,
            provider,
            expected_size,
            expected_sha256,
            files,
            bytes_downloaded: 0,
            network_bytes: 0,
            hash: None,
            control,
            started_at: self.now(),
            speed_bps: 0.0,
            last_sample: None,
            last_emit: 0,
            range_supported: true,
            attempt: 0,
        };

        let result = match self.restore(&mut ctx) {
            Ok(()) => self.run(&mut ctx).await,
            Err(error) => Err(error),
        };
        match result {
            Ok(outcome) => Ok(outcome),
            Err(failure) => {
                let delivery = to_delivery_error(failure, Some(Code::NetworkError));
                self.emit_state(
                    &ctx,
                    StateName::Error,
                    StateExtra {
                        error: Some(delivery.to_public()),
                        ..StateExtra::default()
                    },
                );
                Err(delivery.into())
            }
        }
    }

    /// `checkFreeSpace(dir, needBytes)`: свободно должно быть не меньше размера и запаса.
    fn check_free_space(&self, dir: &Path, need: Option<u64>) -> Result<(), DeliveryError> {
        let Some(need) = need.filter(|&need| need != 0) else {
            return Ok(());
        };
        let Some(free) = self.free_space.free_bytes(dir) else {
            return Ok(());
        };
        let required = need.saturating_add(self.free_space_reserve);
        if free < required {
            return Err(DeliveryError::new(
                Code::NoSpace,
                format!("Недостаточно места: нужно {required}, свободно {free}"),
            )
            .with_details(json!({ "requiredBytes": required, "freeBytes": free })));
        }
        Ok(())
    }

    /// `restore(ctx)`: продолжить с сохранённого места, если файл состояния тот же.
    fn restore(&self, ctx: &mut Ctx) -> Result<(), Failure> {
        let saved = self.read_state(&ctx.files.state);
        let on_disk = std::fs::metadata(&ctx.files.part).map_or(0, |m| m.len());

        let same = saved.as_ref().is_some_and(|saved| {
            json_text_equals(saved.get("expectedSha256"), ctx.expected_sha256.as_deref())
                && json_number_equals(saved.get("expectedSize"), ctx.expected_size)
        });
        if !same {
            if on_disk != 0 {
                self.discard(&ctx.game_id, &ctx.filename)?;
            }
            ctx.bytes_downloaded = 0;
            return Ok(());
        }
        let saved = saved.expect("состояние прочитано");
        ctx.bytes_downloaded = saved_bytes(saved.get("bytesDownloaded"), on_disk);
        if on_disk > ctx.bytes_downloaded {
            truncate(&ctx.files.part, ctx.bytes_downloaded)?;
        }
        Ok(())
    }

    async fn run(&self, ctx: &mut Ctx) -> Result<DownloadOutcome, Failure> {
        loop {
            ctx.attempt += 1;

            let attempt = match self.attempt(ctx).await {
                Ok(()) => self.complete(ctx),
                Err(error) => Err(error),
            };
            let error = match attempt {
                Ok(outcome) => return Ok(outcome),
                Err(error) => error,
            };

            if ctx.control.cancel.load(Ordering::SeqCst) {
                self.discard(&ctx.game_id, &ctx.filename)?;
                self.emit_state(ctx, StateName::Cancelled, StateExtra::default());
                return Ok(DownloadOutcome::Cancelled {
                    network_bytes: ctx.network_bytes,
                });
            }

            if ctx.control.pause.load(Ordering::SeqCst) {
                self.persist(ctx);
                self.emit_state(ctx, StateName::Paused, StateExtra::default());
                return Ok(DownloadOutcome::Paused {
                    bytes_downloaded: ctx.bytes_downloaded,
                });
            }

            let delivery = to_delivery_error(error, Some(Code::NetworkError));

            if ctx.files.part.exists() {
                self.persist(ctx);
            }

            if !RETRYABLE_CODES.contains(&delivery.code) || ctx.attempt >= self.max_attempts {
                return Err(delivery.into());
            }

            let wait = self.backoff(ctx.attempt);
            self.logger.warn(&format!(
                "[game-delivery] попытка {} не удалась ({}), повтор через {wait} мс",
                ctx.attempt, delivery.code
            ));
            self.emit_state(
                ctx,
                StateName::Retrying,
                StateExtra {
                    attempt: Some(ctx.attempt),
                    max_attempts: Some(self.max_attempts),
                    retry_in_ms: Some(wait),
                    error: Some(delivery.to_public()),
                    ..StateExtra::default()
                },
            );

            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(wait)) => {}
                () = ctx.control.aborted() => {}
            }
            ctx.hash = None;
        }
    }

    /// Пауза перед очередной попыткой: `min(30 с, round(основа × 2ⁿ⁻¹ × (0,8 + random × 0,4)))`.
    fn backoff(&self, attempt: u32) -> u64 {
        let jitter = 0.8 + (self.random)() * 0.4;
        let wait = self.backoff_base_ms as f64 * 2f64.powi(attempt as i32 - 1) * jitter;
        js::math_round(wait).min(MAX_BACKOFF_MS as f64) as u64
    }

    /// После удачной попытки: хеш, проверка, переименование, «downloaded».
    fn complete(&self, ctx: &mut Ctx) -> Result<DownloadOutcome, Failure> {
        let digest = hex(&ctx
            .hash
            .take()
            .expect("после попытки хеш посчитан")
            .finalize());
        self.finalize(ctx, &digest)?;
        Ok(DownloadOutcome::Completed {
            file_path: ctx.files.target.clone(),
            sha256: digest,
            size: ctx.bytes_downloaded,
            network_bytes: ctx.network_bytes,
            duration_ms: (self.now() - ctx.started_at).max(0) as u64,
        })
    }

    /// `syncWithDisk(ctx)`: счётчик не больше файла, лишнее в файле отрезается.
    fn sync_with_disk(&self, ctx: &mut Ctx) -> Result<(), Failure> {
        let on_disk = std::fs::metadata(&ctx.files.part).map_or(0, |m| m.len());
        if on_disk < ctx.bytes_downloaded {
            ctx.bytes_downloaded = on_disk;
        } else if on_disk > ctx.bytes_downloaded {
            truncate(&ctx.files.part, ctx.bytes_downloaded)?;
        }
        Ok(())
    }

    async fn attempt(&self, ctx: &mut Ctx) -> Result<(), Failure> {
        let control = ctx.control.clone();
        if ctx.hash.is_none() {
            self.sync_with_disk(ctx)?;
        }

        let resolved = control.until_aborted(ctx.provider.resolve()).await??;
        control.throw_if_aborted()?;
        let mut headers = resolved.headers.clone();
        if ctx.bytes_downloaded > 0 && ctx.provider.supports_range() {
            let range = format!("bytes={}-", ctx.bytes_downloaded);
            match headers.iter_mut().find(|(name, _)| name == "range") {
                Some(header) => header.1 = range,
                None => headers.push(("range".to_owned(), range)),
            }
        }

        self.emit_state(
            ctx,
            if ctx.bytes_downloaded > 0 {
                StateName::Resuming
            } else {
                StateName::Preparing
            },
            StateExtra::default(),
        );

        let options = RequestOptions {
            headers,
            timeout: self.timeout,
            ..RequestOptions::default()
        };
        let mut response = control
            .until_aborted(self.http.request(&resolved.url, &options))
            .await??;

        let status = response.status();
        let mut start = ctx.bytes_downloaded;
        let content_range = response.header("content-range");
        let content_length = response.header("content-length");

        self.logger.info(&format!(
            "[game-delivery] HTTP {status} от {}, запрошено с байта {start}{}, content-length: {}",
            url_host(response.url()),
            match content_range.as_deref().filter(|v| !v.is_empty()) {
                Some(range) => format!(", content-range: {range}"),
                None => String::new(),
            },
            content_length
                .as_deref()
                .filter(|v| !v.is_empty())
                .unwrap_or("?"),
        ));

        if status == 416 {
            drop(response);
            self.reset_progress(ctx);
            return Err(DeliveryError::new(
                Code::RangeNotSupported,
                "Сервер отклонил диапазон, файл начнётся заново",
            )
            .into());
        }

        if ctx.provider.should_refresh_on(status) {
            return Err(DeliveryError::new(
                Code::ProviderForbidden,
                format!("Источник ответил {status} — нужна новая ссылка"),
            )
            .into());
        }

        if status != 200 && status != 206 {
            let retryable = status >= 500 || status == 429 || status == 408;
            let retry_after = response.header("retry-after").filter(|v| !v.is_empty());
            return Err(DeliveryError::new(
                if retryable {
                    Code::NetworkError
                } else {
                    Code::HttpError
                },
                format!("Источник ответил HTTP {status}"),
            )
            .with_details(json!({ "status": status, "retryAfter": retry_after }))
            .into());
        }

        if start > 0 {
            if status == 206 {
                check_content_range(ctx, content_range.as_deref(), start)?;
            } else {
                self.logger
                    .warn("[game-delivery] сервер не поддержал Range, файл скачивается заново");
                ctx.range_supported = false;
                self.reset_progress(ctx);
                start = 0;
            }
        }

        check_content_length(ctx, content_length.as_deref(), start)?;

        if ctx.hash.is_none() {
            self.rebuild_hash(ctx, start).await?;
        }

        self.emit_state(ctx, StateName::Downloading, StateExtra::default());
        self.pump(ctx, &mut response, start).await?;

        if let Some(expected) = ctx.known_size()
            && ctx.bytes_downloaded != expected
        {
            return Err(DeliveryError::new(
                Code::ContentLengthMismatch,
                format!("Получено {} байт вместо {expected}", ctx.bytes_downloaded),
            )
            .into());
        }
        Ok(())
    }

    /// `resetProgress(ctx)`: начать файл заново; `.part` обнуляется.
    fn reset_progress(&self, ctx: &mut Ctx) {
        ctx.bytes_downloaded = 0;
        ctx.hash = None;
        // Файла может не быть — это нормально.
        let _ = std::fs::write(&ctx.files.part, "");
    }

    /// `rebuildHash(ctx, bytes)`: хеш уже скачанной части; сверка с остановкой между блоками.
    async fn rebuild_hash(&self, ctx: &mut Ctx, bytes: u64) -> Result<(), Failure> {
        let mut hash = Sha256::new();
        if bytes > 0 {
            let part = ctx.files.part.clone();
            let control = ctx.control.clone();
            hash = tokio::task::spawn_blocking(move || -> Result<Sha256, Failure> {
                let mut hash = Sha256::new();
                let mut file = std::fs::File::open(&part)
                    .map_err(|error| RawError::fs(&error, "open", &part))?;
                let mut buffer = vec![0u8; REHASH_BLOCK];
                let mut read = 0u64;
                while read < bytes {
                    control.throw_if_aborted()?;
                    let size = REHASH_BLOCK.min((bytes - read) as usize);
                    let got = file
                        .read(&mut buffer[..size])
                        .map_err(|error| RawError::fs(&error, "read", &part))?;
                    if got == 0 {
                        break;
                    }
                    hash.update(&buffer[..got]);
                    read += got as u64;
                }
                if read != bytes {
                    return Err(DeliveryError::new(
                        Code::NetworkError,
                        "Не удалось перечитать уже скачанную часть",
                    )
                    .into());
                }
                Ok(hash)
            })
            .await
            .expect("пересчёт хеша не паникует")?;
        }
        ctx.hash = Some(hash);
        Ok(())
    }

    /// Ошибка записи в `.part` для `run`: `PATH_NOT_WRITABLE`, кроме остановки загрузки.
    fn write_failure(ctx: &Ctx, error: RawError) -> Failure {
        if ctx.control.stopped() {
            return error.into();
        }
        DeliveryError::new(
            Code::PathNotWritable,
            format!("Ошибка записи: {}", error.message),
        )
        .into()
    }

    /// `pump(ctx, response, start)`: тело ответа → ограничитель скорости → счётчик, хеш,
    /// состояние → `.part`.
    async fn pump(
        &self,
        ctx: &mut Ctx,
        response: &mut Response,
        start: u64,
    ) -> Result<(), Failure> {
        let control = ctx.control.clone();
        let mut writer = PartWriter::open(&ctx.files.part, start)
            .map_err(|error| Self::write_failure(ctx, error))?;

        loop {
            let chunk = match control.until_aborted(response.chunk()).await? {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(error) => return Err(error.into()),
            };
            if let Some(limiter) = &self.rate_limiter {
                control.until_aborted(limiter.acquire(chunk.len())).await?;
            }

            ctx.hash
                .as_mut()
                .expect("хеш создан до чтения тела")
                .update(&chunk);
            ctx.bytes_downloaded += chunk.len() as u64;
            ctx.network_bytes += chunk.len() as u64;
            self.sample_progress(ctx);

            if let Err(error) = writer.write(chunk).await {
                return Err(Self::write_failure(ctx, error));
            }
        }
        // Файл закрывается до переименования.
        drop(writer);

        if control.stopped() {
            return Err(stopped_error().into());
        }
        Ok(())
    }

    /// `sampleProgress(ctx)`: скорость со сглаживанием 0,3, состояние на диск и событие
    /// `progress` — не чаще раза в 300 мс.
    fn sample_progress(&self, ctx: &mut Ctx) {
        let now = self.now();
        let Some((sampled_at, sampled_bytes)) = ctx.last_sample else {
            ctx.last_sample = Some((now, ctx.bytes_downloaded));
            return;
        };

        if now - ctx.last_emit < PROGRESS_INTERVAL_MS {
            return;
        }

        let elapsed = (now - sampled_at) as f64 / 1000.0;
        if elapsed > 0.0 {
            let instant = (ctx.bytes_downloaded as f64 - sampled_bytes as f64) / elapsed;
            ctx.speed_bps = if ctx.speed_bps != 0.0 && !ctx.speed_bps.is_nan() {
                ctx.speed_bps * (1.0 - SPEED_SMOOTHING) + instant * SPEED_SMOOTHING
            } else {
                instant
            };
        }

        ctx.last_sample = Some((now, ctx.bytes_downloaded));
        ctx.last_emit = now;

        self.persist(ctx);
        self.emit(&DownloadEvent::Progress(self.progress_of(ctx)));
    }

    /// `progressOf(ctx)`.
    fn progress_of(&self, ctx: &Ctx) -> DownloadProgress {
        let total = ctx.known_size().unwrap_or(0) as f64;
        let bytes = ctx.bytes_downloaded as f64;
        let remaining = (total - bytes).max(0.0);
        DownloadProgress {
            game_id: ctx.game_id.clone(),
            bytes_downloaded: ctx.bytes_downloaded,
            total_bytes: ctx.known_size().unwrap_or(0),
            percent: if total != 0.0 {
                (bytes / total * 100.0).min(100.0)
            } else {
                0.0
            },
            speed_bps: js::math_round(ctx.speed_bps).max(0.0) as u64,
            eta_seconds: (ctx.speed_bps > 1.0)
                .then(|| js::math_round(remaining / ctx.speed_bps) as u64),
            provider_id: ctx.provider_id.clone(),
            range_supported: ctx.range_supported,
        }
    }

    /// `persist(ctx)`: `.part.json`; сбой записи — в журнал, загрузку не останавливает.
    fn persist(&self, ctx: &Ctx) {
        let state = StateFile {
            game_id: Some(ctx.game_id.clone()),
            filename: Some(ctx.filename.clone()),
            expected_size: ctx.expected_size,
            expected_sha256: ctx.expected_sha256.clone(),
            provider_id: ctx.provider_id.clone(),
            bytes_downloaded: Some(ctx.bytes_downloaded),
        };
        if let Err(error) = self.write_state(&ctx.files.state, &state) {
            self.logger.warn(&format!(
                "[game-delivery] не удалось сохранить состояние загрузки: {}",
                error.message
            ));
        }
    }

    /// `finalize(ctx, digest)`: хеш сходится — `.part` становится итоговым файлом.
    fn finalize(&self, ctx: &Ctx, digest: &str) -> Result<(), Failure> {
        if let Some(expected) = ctx.expected_sha256.as_deref().filter(|s| !s.is_empty())
            && digest != expected
        {
            self.discard(&ctx.game_id, &ctx.filename)?;
            return Err(DeliveryError::new(
                Code::ChecksumMismatch,
                format!("SHA-256 не совпал: получено {digest}, ожидалось {expected}"),
            )
            .with_details(json!({ "expected": expected, "actual": digest }))
            .into());
        }

        std::fs::rename(&ctx.files.part, &ctx.files.target).map_err(|error| {
            RawError::fs_with_dest(&error, "rename", &ctx.files.part, Some(&ctx.files.target))
        })?;
        if ctx.files.state.exists() {
            remove_within_zone(&self.downloads, &ctx.files.state)?;
        }

        self.emit_state(
            ctx,
            StateName::Downloaded,
            StateExtra {
                file_path: Some(ctx.files.target.clone()),
                sha256: Some(digest.to_owned()),
                ..StateExtra::default()
            },
        );
        Ok(())
    }

    fn emit_state(&self, ctx: &Ctx, state: StateName, extra: StateExtra) {
        self.emit(&DownloadEvent::State(DownloadState {
            state,
            progress: self.progress_of(ctx),
            extra,
        }));
    }
}

/// `saved.bytesDownloaded || 0`, не больше `size` (`Math.min`). Поле не число, отрицательное
/// или дробное — 0.4.7 дальше работал бы с `NaN` и дробной длиной; здесь это 0 или целая часть.
fn saved_bytes(value: Option<&Value>, size: u64) -> u64 {
    let number = match value {
        None => 0.0,
        Some(value) if !js::truthy(value) => 0.0,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(text)) => js::to_number(text),
        Some(Value::Bool(true)) => 1.0,
        Some(_) => f64::NAN,
    };
    if number.is_nan() || number < 0.0 {
        return 0;
    }
    (number.floor() as u64).min(size)
}

/// `saved.x === ctx.x` для строки: нет и нет, либо одна и та же строка.
fn json_text_equals(saved: Option<&Value>, expected: Option<&str>) -> bool {
    match (saved, expected) {
        (None, None) => true,
        (Some(Value::String(saved)), Some(expected)) => saved == expected,
        _ => false,
    }
}

/// `saved.x === ctx.x` для числа.
fn json_number_equals(saved: Option<&Value>, expected: Option<u64>) -> bool {
    match (saved, expected) {
        (None, None) => true,
        (Some(Value::Number(saved)), Some(expected)) => saved.as_f64() == Some(expected as f64),
        _ => false,
    }
}

/// `fs.truncateSync(file, len)`.
fn truncate(file: &Path, len: u64) -> Result<(), Failure> {
    let handle = std::fs::OpenOptions::new()
        .write(true)
        .open(file)
        .map_err(|error| RawError::fs(&error, "open", file))?;
    handle
        .set_len(len)
        .map_err(|error| RawError::fs(&error, "ftruncate", file))?;
    Ok(())
}

/// `/^bytes\s+(\d+)-(\d+)\/(\d+|\*)$/i` → (начало, конец, всего); числа остаются строками.
fn parse_content_range(text: &str) -> Option<(&str, &str, &str)> {
    let rest = text.get(..5).filter(|p| p.eq_ignore_ascii_case("bytes"))?;
    let rest = &text[rest.len()..];
    let trimmed = rest.trim_start_matches(js::is_whitespace);
    if trimmed.len() == rest.len() {
        return None;
    }
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let first = digits(trimmed);
    let (first_text, rest) = trimmed.split_at(first);
    let rest = rest.strip_prefix('-')?;
    let last = digits(rest);
    let (last_text, rest) = rest.split_at(last);
    let total = rest.strip_prefix('/')?;
    let valid_total =
        total == "*" || (!total.is_empty() && total.bytes().all(|b| b.is_ascii_digit()));
    (first > 0 && last > 0 && valid_total).then_some((first_text, last_text, total))
}

/// `checkContentRange(ctx, header, start)`.
fn check_content_range(ctx: &Ctx, header: Option<&str>, start: u64) -> Result<(), DeliveryError> {
    let Some((first, _, total)) = parse_content_range(header.unwrap_or("")) else {
        return Err(DeliveryError::new(
            Code::RangeNotSupported,
            format!(
                "Некорректный Content-Range: {}",
                header.unwrap_or("undefined")
            ),
        ));
    };
    let number = |text: &str| text.parse::<f64>().unwrap_or(f64::NAN);
    if number(first) != start as f64 {
        return Err(DeliveryError::new(
            Code::RangeNotSupported,
            format!("Сервер продолжил с {first} вместо {start}"),
        ));
    }
    if let Some(expected) = ctx.known_size()
        && total != "*"
        && number(total) != expected as f64
    {
        return Err(DeliveryError::new(
            Code::SizeMismatch,
            format!("Размер файла на источнике {total} не совпадает с манифестом {expected}"),
        ));
    }
    Ok(())
}

/// `checkContentLength(ctx, header, start)`: длина тела должна быть `размер − start`.
fn check_content_length(ctx: &Ctx, header: Option<&str>, start: u64) -> Result<(), DeliveryError> {
    let (Some(header), Some(expected_size)) = (header, ctx.known_size()) else {
        return Ok(());
    };
    let length = js::to_number(header);
    if !length.is_finite() {
        return Ok(());
    }
    let expected = expected_size as f64 - start as f64;
    if length != expected {
        return Err(DeliveryError::new(
            Code::ContentLengthMismatch,
            format!(
                "Content-Length {} не совпадает с ожидаемым {}",
                js_num(length),
                js_num(expected)
            ),
        ));
    }
    Ok(())
}
