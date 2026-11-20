//! Загрузчик картинок с кешем на диске (карта подпроекта 4, §0.8). В 0.4.7 `<img>` в окне
//! загружал Chromium и сам кешировал в `Cache/` (картинки новостей, аватары авторов, спонсоров,
//! профилей и друзей: CSP `img-src https:`). `Cache/` при переносе удаляется, а окна на Slint
//! картинок по адресу не грузит — это делает загрузчик.
//!
//! Правила:
//! - адрес — `https:` без учётных данных (`http:` — только на loopback и только в отладочной
//!   сборке или с `dev-hooks`, как у транспорта API); список разрешённых хостов задаёт
//!   приложение (по умолчанию любой, как `img-src https:`);
//! - ответ — только `200`, не больше [`MAX_IMAGE_BYTES`] (лишнее обрывает чтение), редиректы до 5;
//! - содержимое определяется по заголовку файла, а не по `Content-Type` и расширению: PNG, JPEG и
//!   WebP; размеры из заголовка не больше [`MAX_SIDE`] и [`MAX_PIXELS`] (те же пределы, что у
//!   картинок новостей); SVG, GIF и всё остальное отвергаются;
//! - кеш — файлы `<sha256 адреса>.<png|jpg|webp>` в одной папке, запись через временный файл
//!   `<sha256 адреса>.<pid>.<n>.tmp`; живёт [`CACHE_TTL_MS`] (по времени изменения файла), весь кеш
//!   не больше [`CACHE_MAX_BYTES`] (лишнее вытесняется от самых старых); повреждённый файл кеша
//!   загружается заново. Чужие файлы в папке (любое имя не такого вида) не читаются, не считаются
//!   и не вытесняются; недописанные `.tmp` прошлых запусков и просроченные файлы убираются при
//!   очередной записи; у адреса остаётся один файл — формат ответа мог смениться;
//! - одинаковые адреса, запрошенные одновременно, делят один запрос.
//!
//! Пиксели не декодируются: окно рисует картинку само (Slint), заголовок гарантирует формат и
//! размеры.

use hlsr_delivery::errors::{Code, Failure};
use hlsr_delivery::http_client::{
    HttpClient, Method, RequestOptions, is_loopback, read_body, url_hostname,
};
use hlsr_delivery::timing::{Clock, system_clock};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, UNIX_EPOCH};
use url::Url;

/// Предел размера файла картинки.
pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
/// Предел стороны (как у картинок новостей).
pub const MAX_SIDE: u32 = 8000;
/// Предел площади (как у картинок новостей).
pub const MAX_PIXELS: u64 = 32_000_000;
/// Простой соединения до ответа и между кусками тела.
pub const TIMEOUT_MS: u64 = 10_000;
/// Срок жизни файла в кеше.
pub const CACHE_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Предел размера всего кеша.
pub const CACHE_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Предел длины адреса (знаков).
pub const MAX_URL_LEN: usize = 2048;

/// Формат картинки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Webp,
}

impl ImageFormat {
    /// Расширение файла в кеше.
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Webp => "image/webp",
        }
    }

    const ALL: [ImageFormat; 3] = [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Webp];
}

/// Формат и размеры по заголовку файла.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageInfo {
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
}

fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn le16(bytes: &[u8]) -> u32 {
    u32::from(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn le24(bytes: &[u8]) -> u32 {
    u32::from(bytes[0]) | u32::from(bytes[1]) << 8 | u32::from(bytes[2]) << 16
}

fn sniff_png(bytes: &[u8]) -> Option<ImageInfo> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() < 24 || bytes[..8] != SIGNATURE || be32(&bytes[8..12]) != 13 {
        return None;
    }
    if &bytes[12..16] != b"IHDR" {
        return None;
    }
    let (width, height) = (be32(&bytes[16..20]), be32(&bytes[20..24]));
    (width != 0 && height != 0 && width <= i32::MAX as u32 && height <= i32::MAX as u32).then_some(
        ImageInfo {
            format: ImageFormat::Png,
            width,
            height,
        },
    )
}

/// JPEG: кадр (`SOFn`) ищется по сегментам после `SOI`.
fn sniff_jpeg(bytes: &[u8]) -> Option<ImageInfo> {
    if bytes.len() < 4 || bytes[..3] != [0xFF, 0xD8, 0xFF] {
        return None;
    }
    let mut at = 2;
    loop {
        // Знак маркера: 0xFF, возможно с набивкой 0xFF.
        if *bytes.get(at)? != 0xFF {
            return None;
        }
        while *bytes.get(at)? == 0xFF {
            at += 1;
        }
        let marker = *bytes.get(at)?;
        at += 1;
        match marker {
            // Без длины: TEM, RSTn, SOI.
            0x01 | 0xD0..=0xD8 => continue,
            // Конец изображения или начало данных до кадра.
            0xD9 | 0xDA => return None,
            _ => {}
        }
        let length = usize::from(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]));
        if length < 2 {
            return None;
        }
        // SOF0…SOF15 кроме DHT (C4), JPG (C8) и DAC (CC).
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let frame = bytes.get(at + 2..at + 7)?;
            let height = u32::from(u16::from_be_bytes([frame[1], frame[2]]));
            let width = u32::from(u16::from_be_bytes([frame[3], frame[4]]));
            return (width != 0 && height != 0).then_some(ImageInfo {
                format: ImageFormat::Jpeg,
                width,
                height,
            });
        }
        at += length;
    }
}

fn sniff_webp(bytes: &[u8]) -> Option<ImageInfo> {
    if bytes.len() < 30 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    let (width, height) = match &bytes[12..16] {
        // Сжатие с потерями: тег кадра (3 байта), код начала, 14 бит ширины и высоты.
        b"VP8 " => {
            if bytes[23..26] != [0x9D, 0x01, 0x2A] {
                return None;
            }
            (le16(&bytes[26..28]) & 0x3FFF, le16(&bytes[28..30]) & 0x3FFF)
        }
        // Без потерь: подпись 0x2F и два поля по 14 бит (размер минус один).
        b"VP8L" => {
            if bytes[20] != 0x2F {
                return None;
            }
            let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
            ((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1)
        }
        // Расширенный формат: холст 24+24 бита (размер минус один).
        b"VP8X" => (le24(&bytes[24..27]) + 1, le24(&bytes[27..30]) + 1),
        _ => return None,
    };
    (width != 0 && height != 0).then_some(ImageInfo {
        format: ImageFormat::Webp,
        width,
        height,
    })
}

/// Формат и размеры по заголовку файла; не PNG, JPEG и WebP или заголовок неверен — `None`.
pub fn sniff(bytes: &[u8]) -> Option<ImageInfo> {
    sniff_png(bytes)
        .or_else(|| sniff_jpeg(bytes))
        .or_else(|| sniff_webp(bytes))
}

/// Ошибка загрузки картинки.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImageError {
    #[error("IMAGE_INVALID_URL")]
    InvalidUrl,
    /// Схема не `https:` (и не `http:` на loopback в отладочной сборке).
    #[error("IMAGE_BAD_SCHEME")]
    BadScheme,
    /// В адресе имя или пароль.
    #[error("IMAGE_CREDENTIALS")]
    Credentials,
    #[error("IMAGE_HOST_NOT_ALLOWED")]
    HostNotAllowed,
    #[error("IMAGE_NETWORK")]
    Network,
    #[error("IMAGE_TIMEOUT")]
    Timeout,
    /// Ответ не `200`.
    #[error("IMAGE_HTTP_{0}")]
    Http(u16),
    /// Больше [`ImageLoaderOptions::max_bytes`].
    #[error("IMAGE_TOO_LARGE")]
    TooLarge,
    /// Не PNG, не JPEG и не WebP, или заголовок неверен.
    #[error("IMAGE_NOT_AN_IMAGE")]
    NotAnImage,
    /// Размеры вне пределов.
    #[error("IMAGE_DIMENSIONS")]
    Dimensions,
}

/// Настройки загрузчика.
#[derive(Clone)]
pub struct ImageLoaderOptions {
    pub max_bytes: usize,
    pub max_side: u32,
    pub max_pixels: u64,
    pub timeout: Duration,
    pub ttl_ms: i64,
    pub cache_max_bytes: u64,
    /// Разрешённые хосты (как `URL.hostname`, строчными); `None` — любой.
    pub hosts: Option<Vec<String>>,
    pub clock: Clock,
}

impl Default for ImageLoaderOptions {
    fn default() -> Self {
        ImageLoaderOptions {
            max_bytes: MAX_IMAGE_BYTES,
            max_side: MAX_SIDE,
            max_pixels: MAX_PIXELS,
            timeout: Duration::from_millis(TIMEOUT_MS),
            ttl_ms: CACHE_TTL_MS,
            cache_max_bytes: CACHE_MAX_BYTES,
            hosts: None,
            clock: system_clock(),
        }
    }
}

/// Загруженная картинка: проверенное содержимое и файл в кеше.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedImage {
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    /// Файл в кеше (для `Image::load_from_path`).
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    /// Взято из кеша без запроса.
    pub cached: bool,
}

/// Ключ кеша в имени файла: SHA-256 адреса, 64 строчные hex-цифры.
fn is_key(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Что за файл лежит в папке кеша.
enum Entry {
    /// `<ключ>.<png|jpg|webp>`.
    Image,
    /// `<ключ>.<pid>.<n>.tmp` недописанной записи процесса `pid`.
    Temp { pid: u32 },
    /// Всё остальное: файл не наш.
    Foreign,
}

impl Entry {
    fn of(name: &str) -> Entry {
        let Some((stem, extension)) = name.rsplit_once('.') else {
            return Entry::Foreign;
        };
        if is_key(stem) && ImageFormat::ALL.iter().any(|f| f.extension() == extension) {
            return Entry::Image;
        }
        if extension == "tmp"
            && let Some((rest, counter)) = stem.rsplit_once('.')
            && let Some((key, pid)) = rest.rsplit_once('.')
            && is_key(key)
            && let (Ok(pid), Ok(_)) = (pid.parse::<u32>(), counter.parse::<u64>())
        {
            return Entry::Temp { pid };
        }
        Entry::Foreign
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Размеры вмещаются в пределы (из настроек).
fn within_limits(options: &ImageLoaderOptions, info: &ImageInfo) -> bool {
    info.width <= options.max_side
        && info.height <= options.max_side
        && u64::from(info.width) * u64::from(info.height) <= options.max_pixels
}

/// Кеш на диске. Все вызовы блокирующие: загрузчик гоняет их в `spawn_blocking`.
#[derive(Clone)]
struct DiskCache {
    dir: PathBuf,
    options: ImageLoaderOptions,
    temp_counter: Arc<AtomicU64>,
}

impl DiskCache {
    fn now(&self) -> i64 {
        (self.options.clock)()
    }

    fn path(&self, key: &str, format: ImageFormat) -> PathBuf {
        self.dir.join(format!("{key}.{}", format.extension()))
    }

    /// Время изменения файла (мс от 1970 года); не прочиталось — «давно».
    fn modified_ms(meta: &std::fs::Metadata) -> i64 {
        meta.modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(i64::MIN, |age| age.as_millis() as i64)
    }

    fn is_fresh(&self, meta: &std::fs::Metadata) -> bool {
        self.now() - Self::modified_ms(meta) <= self.options.ttl_ms
    }

    /// Файл кеша, если он есть, свеж и цел; негодный (просроченный, повреждённый, с чужим
    /// форматом внутри) удаляется. Файлов одного адреса может быть несколько, если формат ответа
    /// сменился: годным считается любой из них.
    fn read(&self, key: &str) -> Option<LoadedImage> {
        for format in ImageFormat::ALL {
            let path = self.path(key, format);
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let image = self
                .is_fresh(&meta)
                .then(|| std::fs::read(&path).ok())
                .flatten()
                .and_then(|bytes| {
                    let info = sniff(&bytes)?;
                    (info.format == format && within_limits(&self.options, &info))
                        .then_some((info, bytes))
                });
            match image {
                Some((info, bytes)) => {
                    return Some(LoadedImage {
                        format,
                        width: info.width,
                        height: info.height,
                        path,
                        bytes,
                        cached: true,
                    });
                }
                None => {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        None
    }

    /// Запись в кеш через временный файл и уборка: файлы того же адреса в других форматах
    /// (прежний ответ), недописанные `.tmp` прошлых запусков, просроченное, лишнее по размеру.
    /// Сбой записи кеш не ломает: картинку отдаём из памяти.
    fn store(&self, key: &str, info: &ImageInfo, bytes: &[u8]) -> PathBuf {
        let path = self.path(key, info.format);
        if self.write_file(&path, bytes).is_ok() {
            for other in ImageFormat::ALL {
                if other != info.format {
                    let _ = std::fs::remove_file(self.path(key, other));
                }
            }
        }
        self.evict(&path);
        path
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let n = self.temp_counter.fetch_add(1, Ordering::Relaxed);
        let temp = path.with_extension(format!("{}.{n}.tmp", std::process::id()));
        std::fs::write(&temp, bytes)?;
        // Время изменения — по часам загрузчика: от него считается срок жизни.
        let stamped = std::fs::OpenOptions::new()
            .write(true)
            .open(&temp)
            .and_then(|file| {
                file.set_modified(UNIX_EPOCH + Duration::from_millis(self.now().max(0) as u64))
            });
        if let Err(error) = stamped.and_then(|()| std::fs::rename(&temp, path)) {
            let _ = std::fs::remove_file(&temp);
            return Err(error);
        }
        Ok(())
    }

    /// Убрать недописанное и просроченное, затем самые старые файлы, пока кеш больше предела;
    /// `keep` не трогается. Работает только с файлами кеша ([`Entry`]): чужие в папке остаются на
    /// месте и в размер не входят; `.tmp` своего процесса — запись идёт прямо сейчас.
    fn evict(&self, keep: &Path) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut files = Vec::new();
        let mut total = 0u64;
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let name = entry.file_name();
            match Entry::of(&name.to_string_lossy()) {
                Entry::Foreign => {}
                Entry::Temp { pid } => {
                    if pid != std::process::id() {
                        let _ = std::fs::remove_file(&path);
                    }
                }
                Entry::Image => {
                    if path != keep && !self.is_fresh(&meta) {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    total += meta.len();
                    files.push((meta.modified().ok(), path, meta.len()));
                }
            }
        }
        if total <= self.options.cache_max_bytes {
            return;
        }
        files.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        for (_, path, size) in files {
            if total <= self.options.cache_max_bytes {
                break;
            }
            if path == keep {
                continue;
            }
            if std::fs::remove_file(&path).is_ok() {
                total -= size;
            }
        }
    }
}

pub struct ImageLoader {
    cache: DiskCache,
    http: HttpClient,
    /// По замку на адрес: одинаковые запросы идут по очереди, второй находит файл в кеше.
    flights: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl ImageLoader {
    /// Загрузчик с кешем в папке `dir` (создаётся при первой записи).
    pub fn new(dir: impl Into<PathBuf>, options: ImageLoaderOptions) -> ImageLoader {
        ImageLoader {
            cache: DiskCache {
                dir: dir.into(),
                options,
                temp_counter: Arc::new(AtomicU64::new(0)),
            },
            http: HttpClient::new(),
            flights: Mutex::default(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.cache.dir
    }

    /// Проверка адреса; результат — разобранный адрес.
    fn check_url(&self, text: &str) -> Result<Url, ImageError> {
        if text.chars().count() > MAX_URL_LEN || text.chars().any(|c| c.is_control()) {
            return Err(ImageError::InvalidUrl);
        }
        let url = Url::parse(text).map_err(|_| ImageError::InvalidUrl)?;
        let host = url_hostname(&url);
        match url.scheme() {
            "https" => {}
            "http" => {
                let allowed =
                    cfg!(any(debug_assertions, feature = "dev-hooks")) && is_loopback(host);
                if !allowed {
                    return Err(ImageError::BadScheme);
                }
            }
            _ => return Err(ImageError::BadScheme),
        }
        if host.is_empty() {
            return Err(ImageError::InvalidUrl);
        }
        if !url.username().is_empty() || url.password().is_some_and(|p| !p.is_empty()) {
            return Err(ImageError::Credentials);
        }
        if let Some(hosts) = &self.cache.options.hosts
            && !hosts.iter().any(|allowed| allowed == host)
        {
            return Err(ImageError::HostNotAllowed);
        }
        Ok(url)
    }

    fn key(url: &Url) -> String {
        let digest = Sha256::digest(url.as_str().as_bytes());
        hlsr_core::js::text::hex(&digest)
    }

    /// Картинка по адресу: из кеша или с сервера.
    pub async fn load(&self, url: &str) -> Result<LoadedImage, ImageError> {
        let url = self.check_url(url)?;
        let key = Self::key(&url);
        let flight = lock(&self.flights).entry(key.clone()).or_default().clone();
        let _turn = flight.lock().await;
        let cache = self.cache.clone();
        let stored = {
            let key = key.clone();
            tokio::task::spawn_blocking(move || cache.read(&key))
                .await
                .ok()
                .flatten()
        };
        let result = match stored {
            Some(image) => Ok(image),
            None => self.download(&url, &key).await,
        };
        // Замок адреса больше не нужен, если на нём никто не ждёт.
        let mut flights = lock(&self.flights);
        if Arc::strong_count(&flight) <= 2 {
            flights.remove(&key);
        }
        result
    }

    async fn download(&self, url: &Url, key: &str) -> Result<LoadedImage, ImageError> {
        let options = &self.cache.options;
        let request = RequestOptions {
            method: Method::Get,
            headers: vec![(
                "accept".to_owned(),
                "image/png,image/jpeg,image/webp".to_owned(),
            )],
            timeout: Some(options.timeout),
        };
        let response = self
            .http
            .request(url.as_str(), &request)
            .await
            .map_err(map_failure)?;
        if response.status() != 200 {
            return Err(ImageError::Http(response.status()));
        }
        let declared = response
            .header("content-length")
            .and_then(|length| length.trim().parse::<u64>().ok());
        if declared.is_some_and(|length| length > options.max_bytes as u64) {
            return Err(ImageError::TooLarge);
        }
        let bytes = read_body(response, Some(options.max_bytes), None)
            .await
            .map_err(|error| match error.code {
                Code::HttpError => ImageError::TooLarge,
                Code::NetworkTimeout => ImageError::Timeout,
                _ => ImageError::Network,
            })?;
        let info = sniff(&bytes).ok_or(ImageError::NotAnImage)?;
        if !within_limits(options, &info) {
            return Err(ImageError::Dimensions);
        }
        let cache = self.cache.clone();
        let (key, for_disk) = (key.to_owned(), bytes.clone());
        let path = tokio::task::spawn_blocking(move || cache.store(&key, &info, &for_disk))
            .await
            .map_err(|_| ImageError::Network)?;
        Ok(LoadedImage {
            format: info.format,
            width: info.width,
            height: info.height,
            path,
            bytes,
            cached: false,
        })
    }
}

fn map_failure(failure: Failure) -> ImageError {
    match failure {
        Failure::Delivery(error) => match error.code {
            Code::NetworkTimeout => ImageError::Timeout,
            Code::InsecureRedirect => ImageError::BadScheme,
            _ => ImageError::Network,
        },
        Failure::Raw(_) => ImageError::Network,
    }
}
