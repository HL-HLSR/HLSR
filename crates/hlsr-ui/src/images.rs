//! Загрузчик картинок интерфейса (решение 5 спецификации ПП5; карта §0.16–0.17, §6 «Картинки
//! из сети»; риск 8).
//!
//! В 0.4.7 картинки грузил и держал Chromium: `<img>` и `background-image` с адресов `data:`,
//! файлов сборки и `https:` (CSP `img-src 'self' data: https: file:`, `referrerpolicy=no-referrer`).
//! Здесь:
//! - источники — `data:` (аватары из main), файлы сборки ([`register_bundle`], адрес
//!   `bundle:<имя>`), файлы на диске (`file:`), `https:` — через порт [`RemoteImages`]: в
//!   приложении это загрузчик ПП4 (`hlsr_online::image_loader`: без Referer, кеш на диске), в
//!   фикстурах — файлы ([`FixtureImages`]);
//! - декодирование в Rust (PNG, JPEG, WebP) в [`DECODERS`] потоках, в каждом по одной картинке
//!   за раз (кадры больше 4K — по одному на все потоки): полноразмерный кадр живёт только до
//!   уменьшения;
//! - уменьшение до области в физических пикселях (`Images.load` без размера — до размера
//!   окна), альфа предумножается до уменьшения, как у Skia;
//! - ленивость: картинка декодируется, только когда её спросили;
//! - кеш готовых картинок с пределом в байтах (вытесняются давно не нужные) и освобождение по
//!   области (`scope`: экран или игра — «games/218», «personalization»), когда экран уходит.

use crate::Rgba8Image;
use crate::env::Env;
use base64::Engine as _;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};

/// Предел стороны картинки — тот же, что у загрузчика ПП4 и картинок новостей.
pub const MAX_SIDE: u32 = hlsr_online::image_loader::MAX_SIDE;
/// Предел площади картинки.
pub const MAX_PIXELS: u64 = hlsr_online::image_loader::MAX_PIXELS;
/// Предел кеша готовых картинок по умолчанию (RGBA, 4 байта на пиксель).
pub const DEFAULT_BUDGET_BYTES: usize = 64 * 1024 * 1024;
/// Шаг размера области: соседние размеры (анимация ширины) берут одну картинку.
const BOX_STEP: u32 = 32;
/// Потоков декодирования: кадр и передний план фона игры (смена игры ждёт оба).
pub const DECODERS: usize = 2;
/// Больше стольких пикселей (4K) — по одному декодированию за раз, см. [`decode`].
const LARGE_PIXELS: u64 = 3840 * 2160;

/// Ошибка загрузки картинки; текст — код для журнала.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImageError {
    /// Адрес не разобран или источник неизвестен.
    #[error("IMAGE_INVALID_SOURCE")]
    InvalidSource,
    /// Файла сборки с таким именем нет.
    #[error("IMAGE_NO_BUNDLE")]
    NoBundle,
    /// Файл не прочитан.
    #[error("IMAGE_READ_FAILED")]
    Read,
    /// Сеть: код загрузчика ПП4 (`IMAGE_HTTP_404`, `IMAGE_TIMEOUT`…).
    #[error("{0}")]
    Remote(String),
    /// Не PNG, не JPEG и не WebP или файл повреждён.
    #[error("IMAGE_NOT_AN_IMAGE")]
    NotAnImage,
    /// Размеры вне пределов.
    #[error("IMAGE_DIMENSIONS")]
    Dimensions,
}

/// Откуда брать картинку.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ImageSource {
    /// `data:image/…;base64,…` или с процентным кодированием.
    DataUri(String),
    /// Файл сборки, зарегистрированный [`register_bundle`].
    Bundle(String),
    /// Файл на диске.
    File(PathBuf),
    /// `https:` (и что ещё примет порт): загрузчик ПП4.
    Remote(String),
}

impl ImageSource {
    /// Разобрать адрес из разметки: `data:`, `https:`/`http:`, `bundle:<имя>`, `file:` или
    /// абсолютный путь. Пустой или незнакомый — `None`.
    pub fn parse(text: &str) -> Option<ImageSource> {
        let lower = text.get(..8).unwrap_or(text).to_ascii_lowercase();
        if lower.starts_with("data:") {
            return Some(ImageSource::DataUri(text.to_owned()));
        }
        if lower.starts_with("https:") || lower.starts_with("http:") {
            return Some(ImageSource::Remote(text.to_owned()));
        }
        if let Some(name) = text.strip_prefix("bundle:") {
            return (!name.is_empty()).then(|| ImageSource::Bundle(name.to_owned()));
        }
        if lower.starts_with("file:") {
            return file_url_path(text).map(ImageSource::File);
        }
        let path = Path::new(text);
        (!text.is_empty() && path.is_absolute()).then(|| ImageSource::File(path.to_path_buf()))
    }

    /// Ключ кеша источника.
    fn key(&self) -> Cow<'_, str> {
        match self {
            ImageSource::DataUri(text) | ImageSource::Remote(text) => Cow::Borrowed(text),
            ImageSource::Bundle(name) => Cow::Owned(format!("bundle:{name}")),
            ImageSource::File(path) => Cow::Owned(format!("file:{}", path.display())),
        }
    }
}

/// `file:///C:/a%20b.png` или `file:///home/a.png` → путь.
fn file_url_path(text: &str) -> Option<PathBuf> {
    let rest = text.get(5..)?;
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    // Хост пустой (file:///…) или localhost.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let decoded = percent_decode(rest.as_bytes())?;
    let decoded = String::from_utf8(decoded).ok()?;
    // /C:/… → C:/… на Windows.
    let bytes = decoded.as_bytes();
    let path = if cfg!(windows)
        && bytes.len() >= 3
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && bytes[2] == b':'
    {
        decoded[1..].to_owned()
    } else {
        decoded
    };
    let path = PathBuf::from(path);
    path.is_absolute().then_some(path)
}

fn percent_decode(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let text = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(text, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(out)
}

/// Байты картинки из `data:`-адреса (`;base64` или процентное кодирование).
pub fn data_uri_bytes(uri: &str) -> Result<Vec<u8>, ImageError> {
    let rest = uri
        .get(..5)
        .filter(|scheme| scheme.eq_ignore_ascii_case("data:"))
        .map(|_| &uri[5..])
        .ok_or(ImageError::InvalidSource)?;
    let (meta, payload) = rest.split_once(',').ok_or(ImageError::InvalidSource)?;
    let base64 = meta
        .rsplit(';')
        .next()
        .is_some_and(|last| last.eq_ignore_ascii_case("base64"));
    if base64 {
        // Пробелы и переводы строк внутри base64 браузер пропускает.
        let compact: Vec<u8> = payload
            .bytes()
            .filter(|b| !b.is_ascii_whitespace())
            .collect();
        let compact = percent_decode(&compact).ok_or(ImageError::InvalidSource)?;
        base64::engine::general_purpose::STANDARD
            .decode(&compact)
            .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(&compact))
            .map_err(|_| ImageError::InvalidSource)
    } else {
        percent_decode(payload.as_bytes()).ok_or(ImageError::InvalidSource)
    }
}

fn bundles() -> &'static Mutex<HashMap<String, &'static [u8]>> {
    static BUNDLES: OnceLock<Mutex<HashMap<String, &'static [u8]>>> = OnceLock::new();
    BUNDLES.get_or_init(Default::default)
}

/// Зарегистрировать файл сборки (`include_bytes!`) под именем `name`: адрес `bundle:<name>`.
pub fn register_bundle(name: &str, bytes: &'static [u8]) {
    bundles()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .insert(name.to_owned(), bytes);
}

fn bundle(name: &str) -> Option<&'static [u8]> {
    bundles()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .get(name)
        .copied()
}

/// Как вписывать картинку в область: нужен ли каждый пиксель области (`Cover`, а также
/// растяжение) или картинка целиком внутри (`Contain`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fit {
    Cover,
    Contain,
}

/// Размер после уменьшения под область `target` (физические пиксели); не больше исходного.
pub fn fitted_size(width: u32, height: u32, target: Option<(u32, u32)>, fit: Fit) -> (u32, u32) {
    let Some((tw, th)) = target.filter(|(tw, th)| *tw > 0 && *th > 0) else {
        return (width, height);
    };
    let sx = f64::from(tw) / f64::from(width);
    let sy = f64::from(th) / f64::from(height);
    let scale = match fit {
        Fit::Cover => sx.max(sy),
        Fit::Contain => sx.min(sy),
    };
    if scale >= 1.0 {
        return (width, height);
    }
    let scaled = |side: u32| ((f64::from(side) * scale).ceil() as u32).clamp(1, side);
    (scaled(width), scaled(height))
}

/// Декодированная картинка: RGBA с предумноженной альфой.
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    /// Исходный размер до уменьшения.
    pub natural: (u32, u32),
    pub pixels: Rgba8Image,
}

/// Декодировать PNG, JPEG или WebP и уменьшить под `target` (физические пиксели).
pub fn decode(bytes: &[u8], target: Option<(u32, u32)>, fit: Fit) -> Result<Decoded, ImageError> {
    let format = image::guess_format(bytes).map_err(|_| ImageError::NotAnImage)?;
    if !matches!(
        format,
        image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP
    ) {
        return Err(ImageError::NotAnImage);
    }
    let (width, height) = image::ImageReader::with_format(std::io::Cursor::new(bytes), format)
        .into_dimensions()
        .map_err(|_| ImageError::NotAnImage)?;
    if width == 0
        || height == 0
        || width > MAX_SIDE
        || height > MAX_SIDE
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err(ImageError::Dimensions);
    }
    // Кадры больше 4K декодируются по одному: пик памяти потоков [`DECODERS`] — один такой кадр
    // (до 128 МиБ RGBA) и один не больше 4K, а не два полноразмерных.
    static LARGE: Mutex<()> = Mutex::new(());
    let _large = (u64::from(width) * u64::from(height) > LARGE_PIXELS)
        .then(|| LARGE.lock().unwrap_or_else(|e| e.into_inner()));
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    reader.limits(limits);
    let mut rgba = reader
        .decode()
        .map_err(|_| ImageError::NotAnImage)?
        .into_rgba8();
    premultiply(&mut rgba);
    let (fw, fh) = fitted_size(width, height, target, fit);
    let rgba = if (fw, fh) == (width, height) {
        rgba
    } else {
        let resized = image::imageops::resize(&rgba, fw, fh, image::imageops::FilterType::Triangle);
        drop(rgba);
        resized
    };
    let pixels = Rgba8Image::from_bytes(fw, fh, rgba.as_raw()).premultiplied();
    Ok(Decoded {
        width: fw,
        height: fh,
        natural: (width, height),
        pixels,
    })
}

fn premultiply(rgba: &mut image::RgbaImage) {
    for pixel in rgba.pixels_mut() {
        let [r, g, b, a] = pixel.0;
        if a == 255 {
            continue;
        }
        let mul = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
        pixel.0 = [mul(r), mul(g), mul(b), a];
    }
}

/// Ответ порта сети: байты картинки или код ошибки загрузчика.
pub type RemoteResult = Result<Vec<u8>, String>;

/// Порт загрузки картинок из сети. В приложении — загрузчик ПП4 (`https:` без Referer, кеш на
/// диске, проверка формата и размеров); в фикстурах — файлы. `done` можно звать с любого потока.
pub trait RemoteImages {
    fn fetch(&self, url: &str, done: Box<dyn FnOnce(RemoteResult) + Send>);
}

/// Порт без сети: всякий адрес — ошибка (до подключения `OnlineApi`).
pub struct NoRemote;

impl RemoteImages for NoRemote {
    fn fetch(&self, _url: &str, done: Box<dyn FnOnce(RemoteResult) + Send>) {
        done(Err("IMAGE_NETWORK".to_owned()));
    }
}

/// Порт сети приложения: загрузчик ПП4 (`hlsr_online::image_loader`: `https:` без Referer,
/// кеш на диске, формат и размеры по заголовку). Загрузка — задача tokio; ответ — код ошибки
/// загрузчика (`IMAGE_HTTP_404`, `IMAGE_TIMEOUT`, `IMAGE_HOST_NOT_ALLOWED`…).
pub struct OnlineImages {
    loader: Arc<hlsr_online::image_loader::ImageLoader>,
    runtime: tokio::runtime::Handle,
}

impl OnlineImages {
    pub fn new(
        loader: Arc<hlsr_online::image_loader::ImageLoader>,
        runtime: tokio::runtime::Handle,
    ) -> OnlineImages {
        OnlineImages { loader, runtime }
    }
}

impl RemoteImages for OnlineImages {
    fn fetch(&self, url: &str, done: Box<dyn FnOnce(RemoteResult) + Send>) {
        let loader = self.loader.clone();
        let url = url.to_owned();
        self.runtime.spawn(async move {
            let result = loader
                .load(&url)
                .await
                .map(|image| image.bytes)
                .map_err(|error| error.to_string());
            done(result);
        });
    }
}

/// Порт из фикстур: адрес → файл; незнакомый адрес — `IMAGE_HTTP_404`.
pub struct FixtureImages {
    files: HashMap<String, PathBuf>,
}

impl FixtureImages {
    pub fn new(files: HashMap<String, PathBuf>) -> FixtureImages {
        FixtureImages { files }
    }
}

impl RemoteImages for FixtureImages {
    fn fetch(&self, url: &str, done: Box<dyn FnOnce(RemoteResult) + Send>) {
        let result = match self.files.get(url) {
            Some(path) => std::fs::read(path).map_err(|_| "IMAGE_NETWORK".to_owned()),
            None => Err("IMAGE_HTTP_404".to_owned()),
        };
        done(result);
    }
}

/// Запрос картинки.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageRequest {
    pub source: ImageSource,
    /// Область в физических пикселях; `None` — исходный размер (без уменьшения).
    pub target: Option<(u32, u32)>,
    pub fit: Fit,
    /// Кто держит картинку: [`ImageLoader::release_scope`] освобождает всё по области.
    pub scope: String,
    /// Область — точный размер, без шага [`BOX_STEP`]: картинку рисуют ровно в этот размер, и
    /// второго уменьшения на видеокарте нет (Chromium уменьшает исходник один раз). Для
    /// фиксированных картинок сборки (арт игр); у остальных соседние размеры делят одну копию.
    pub exact: bool,
}

impl ImageRequest {
    pub fn new(source: ImageSource, scope: &str) -> ImageRequest {
        ImageRequest {
            source,
            target: None,
            fit: Fit::Contain,
            scope: scope.to_owned(),
            exact: false,
        }
    }

    pub fn with_target(mut self, width: u32, height: u32, fit: Fit) -> ImageRequest {
        self.target = Some((width, height));
        self.fit = fit;
        self
    }

    /// Точная область (см. [`ImageRequest::exact`]).
    pub fn exact(mut self) -> ImageRequest {
        self.exact = true;
        self
    }

    /// Область запроса в кеше: с шагом или точная; пустая — `None`.
    fn area(&self) -> Option<(u32, u32)> {
        if self.exact {
            self.target.filter(|(w, h)| *w > 0 && *h > 0)
        } else {
            stepped(self.target)
        }
    }
}

/// Готовая картинка или ошибка.
pub type ImageResult = Result<Arc<Rgba8Image>, ImageError>;

/// Состояние кеша (для тестов и журнала).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CacheStats {
    pub entries: usize,
    pub bytes: usize,
    /// Декодирований с начала работы.
    pub decodes: usize,
    /// Ждут декодирования или сети.
    pub pending: usize,
}

/// Как вернуть результат из потока декодирования в поток интерфейса.
pub use crate::env::Dispatch;

/// Копия в кеше: источник, область, вписывание.
type CopyId<'a> = (&'a str, Option<(u32, u32)>, Fit);

/// Готовая копия картинки: для своей области и вписывания.
struct Entry {
    image: Arc<Rgba8Image>,
    /// Область запроса (с шагом) и вписывание.
    target: Option<(u32, u32)>,
    fit: Fit,
    width: u32,
    height: u32,
    natural: (u32, u32),
    bytes: usize,
    scope: String,
    used: u64,
}

type Callback = Box<dyn FnOnce(ImageResult)>;

struct Pending {
    callbacks: Vec<Callback>,
    source: String,
    target: Option<(u32, u32)>,
    fit: Fit,
    scope: String,
    /// Поколение области при запросе: область освобождена — результат не кешируется.
    generation: u64,
}

struct Job {
    input: JobInput,
    target: Option<(u32, u32)>,
    fit: Fit,
    reply: Box<dyn FnOnce(Result<Decoded, ImageError>) + Send>,
}

enum JobInput {
    Bytes(Vec<u8>),
    Static(&'static [u8]),
    File(PathBuf),
}

struct Inner {
    id: u64,
    remote: Rc<dyn RemoteImages>,
    dispatch: Dispatch,
    worker: mpsc::Sender<Job>,
    budget: Cell<usize>,
    /// Источник → его готовые копии.
    entries: RefCell<HashMap<String, Vec<Entry>>>,
    /// Загрузки в пути: ключ запроса ([`pending_key`]) → кто ждёт.
    pending: RefCell<HashMap<String, Pending>>,
    /// Не загрузились: ключ запроса → (ошибка, область). Не повторяются, пока область не
    /// освобождена.
    failed: RefCell<HashMap<String, (ImageError, String)>>,
    generations: RefCell<HashMap<String, u64>>,
    bytes: Cell<usize>,
    clock: Cell<u64>,
    decodes: Cell<usize>,
    /// Готова новая картинка (адаптер `Images` поднимает `revision`).
    on_ready: RefCell<Option<Rc<dyn Fn()>>>,
}

thread_local! {
    /// Загрузчики потока интерфейса: результат из потока декодирования находит свой по номеру.
    static LOADERS: RefCell<Vec<(u64, Weak<Inner>)>> = const { RefCell::new(Vec::new()) };
}

fn next_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Область запроса с шагом [`BOX_STEP`] (соседние размеры — одна копия); пустая — `None`.
fn stepped(target: Option<(u32, u32)>) -> Option<(u32, u32)> {
    target.filter(|(w, h)| *w > 0 && *h > 0).map(|(w, h)| {
        (
            w.div_ceil(BOX_STEP) * BOX_STEP,
            h.div_ceil(BOX_STEP) * BOX_STEP,
        )
    })
}

/// Ключ загрузки: область, вписывание, источник.
fn pending_key(source: &str, target: Option<(u32, u32)>, fit: Fit) -> String {
    let size = target.map_or_else(|| "full".to_owned(), |(w, h)| format!("{w}x{h}"));
    format!("{size}|{}|{source}", fit_name(fit))
}

/// Загрузчик картинок. Живёт в потоке интерфейса; клон — тот же загрузчик.
#[derive(Clone)]
pub struct ImageLoader {
    inner: Rc<Inner>,
}

impl ImageLoader {
    /// Загрузчик с портом сети и возвратом в поток интерфейса через среду `env`.
    pub fn new(remote: Rc<dyn RemoteImages>, env: &dyn Env) -> ImageLoader {
        ImageLoader::with_dispatch(remote, crate::env::dispatch(env))
    }

    /// Загрузчик со своим способом вернуться в поток интерфейса (тесты).
    pub fn with_dispatch(remote: Rc<dyn RemoteImages>, dispatch: Dispatch) -> ImageLoader {
        let (worker, jobs) = mpsc::channel::<Job>();
        let jobs = Arc::new(Mutex::new(jobs));
        // Потоки декодирования: каждый по одной картинке, пока жив загрузчик (канал закрыт —
        // выход). Их несколько: кадр и передний план фона игры декодируются вместе.
        for _ in 0..DECODERS {
            let jobs = jobs.clone();
            let spawned = std::thread::Builder::new()
                .name("hlsr-images".into())
                .spawn(move || {
                    loop {
                        let next = jobs.lock().unwrap_or_else(|e| e.into_inner()).recv();
                        let Ok(job) = next else { break };
                        let bytes = match job.input {
                            JobInput::Bytes(bytes) => Ok(Cow::Owned(bytes)),
                            JobInput::Static(bytes) => Ok(Cow::Borrowed(bytes)),
                            JobInput::File(path) => std::fs::read(path)
                                .map(Cow::Owned)
                                .map_err(|_| ImageError::Read),
                        };
                        let result = bytes.and_then(|bytes| decode(&bytes, job.target, job.fit));
                        (job.reply)(result);
                    }
                });
            if let Err(error) = spawned {
                tracing::warn!("[images] поток декодирования не запущен: {error}");
            }
        }
        let inner = Rc::new(Inner {
            id: next_id(),
            remote,
            dispatch,
            worker,
            budget: Cell::new(DEFAULT_BUDGET_BYTES),
            entries: RefCell::default(),
            pending: RefCell::default(),
            failed: RefCell::default(),
            generations: RefCell::default(),
            bytes: Cell::new(0),
            clock: Cell::new(0),
            decodes: Cell::new(0),
            on_ready: RefCell::new(None),
        });
        LOADERS.with(|loaders| {
            let mut loaders = loaders.borrow_mut();
            loaders.retain(|(_, weak)| weak.strong_count() > 0);
            loaders.push((inner.id, Rc::downgrade(&inner)));
        });
        ImageLoader { inner }
    }

    /// Предел кеша в байтах; лишнее вытесняется сразу.
    pub fn set_budget(&self, bytes: usize) {
        self.inner.budget.set(bytes);
        self.inner.evict(None);
    }

    /// Звать при каждой готовой картинке (после колбэков запроса).
    pub fn on_ready(&self, notify: impl Fn() + 'static) {
        *self.inner.on_ready.borrow_mut() = Some(Rc::new(notify));
    }

    /// Готовая картинка из кеша: та же область или крупнее (тот же источник и вписывание).
    pub fn cached(&self, request: &ImageRequest) -> Option<Arc<Rgba8Image>> {
        self.inner
            .lookup(&request.source.key(), request.area(), request.fit, true)
    }

    /// То же, но без обновления LRU-метки: для чтения уже показанного слоя (подложки стекла),
    /// чтобы уходящий фон не становился «свежее» загружаемого нового и не вытеснял его.
    pub fn peek(&self, request: &ImageRequest) -> Option<Arc<Rgba8Image>> {
        self.inner
            .lookup(&request.source.key(), request.area(), request.fit, false)
    }

    /// Картинка по запросу: из кеша сразу, иначе — после загрузки (`done` в потоке интерфейса).
    /// Одинаковые запросы, пока идёт загрузка, делят её.
    pub fn request(&self, request: ImageRequest, done: impl FnOnce(ImageResult) + 'static) {
        let inner = &self.inner;
        let source = request.source.key();
        let target = request.area();
        if let Some(image) = inner.lookup(&source, target, request.fit, true) {
            done(Ok(image));
            return;
        }
        let key = pending_key(&source, target, request.fit);
        let failed = inner
            .failed
            .borrow()
            .get(&key)
            .map(|(error, _)| error.clone());
        if let Some(error) = failed {
            done(Err(error));
            return;
        }
        if let Some(pending) = inner.pending.borrow_mut().get_mut(&key) {
            pending.callbacks.push(Box::new(done));
            return;
        }
        inner.pending.borrow_mut().insert(
            key.clone(),
            Pending {
                callbacks: vec![Box::new(done)],
                source: source.into_owned(),
                target,
                fit: request.fit,
                scope: request.scope.clone(),
                generation: inner.generation(&request.scope),
            },
        );
        let input = match &request.source {
            ImageSource::DataUri(uri) => data_uri_bytes(uri).map(JobInput::Bytes),
            ImageSource::Bundle(name) => bundle(name)
                .map(JobInput::Static)
                .ok_or(ImageError::NoBundle),
            ImageSource::File(path) => Ok(JobInput::File(path.clone())),
            ImageSource::Remote(url) => {
                let worker = inner.worker.clone();
                let dispatch = inner.dispatch.clone();
                let id = inner.id;
                let fit = request.fit;
                inner.remote.fetch(
                    url,
                    Box::new(move |fetched| match fetched {
                        Ok(bytes) => {
                            let reply = reply_to(id, key, dispatch);
                            let job = Job {
                                input: JobInput::Bytes(bytes),
                                target,
                                fit,
                                reply,
                            };
                            if let Err(mpsc::SendError(job)) = worker.send(job) {
                                (job.reply)(Err(ImageError::Remote("IMAGE_CLOSED".into())));
                            }
                        }
                        Err(code) => reply_to(id, key, dispatch)(Err(ImageError::Remote(code))),
                    }),
                );
                return;
            }
        };
        let reply = reply_to(inner.id, key, inner.dispatch.clone());
        match input {
            Ok(input) => {
                let job = Job {
                    input,
                    target,
                    fit: request.fit,
                    reply,
                };
                if let Err(mpsc::SendError(job)) = inner.worker.send(job) {
                    (job.reply)(Err(ImageError::Read));
                }
            }
            // Ответ — тем же путём, что из потока: колбэк никогда не зовётся внутри request.
            Err(error) => reply(Err(error)),
        }
    }

    /// Освободить картинки области: из кеша и у незаконченных загрузок (их результат не
    /// кешируется). Картинка, которую ещё держит разметка, живёт, пока её не заменят.
    pub fn release_scope(&self, scope: &str) {
        let inner = &self.inner;
        *inner
            .generations
            .borrow_mut()
            .entry(scope.to_owned())
            .or_default() += 1;
        let mut freed = 0;
        inner.entries.borrow_mut().retain(|_, copies| {
            copies.retain(|entry| {
                let keep = entry.scope != scope;
                if !keep {
                    freed += entry.bytes;
                }
                keep
            });
            !copies.is_empty()
        });
        inner.bytes.set(inner.bytes.get() - freed);
        inner
            .failed
            .borrow_mut()
            .retain(|_, (_, failed_scope)| failed_scope != scope);
    }

    /// Освободить весь кеш.
    pub fn clear(&self) {
        let scopes: Vec<String> = self
            .inner
            .entries
            .borrow()
            .values()
            .flatten()
            .map(|entry| entry.scope.clone())
            .collect();
        for scope in scopes {
            self.release_scope(&scope);
        }
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entries: self.inner.entries.borrow().values().map(Vec::len).sum(),
            bytes: self.inner.bytes.get(),
            decodes: self.inner.decodes.get(),
            pending: self.inner.pending.borrow().len(),
        }
    }
}

/// Колбэк потока декодирования: возвращается в поток интерфейса и отдаёт результат загрузчику.
fn reply_to(
    id: u64,
    key: String,
    dispatch: Dispatch,
) -> Box<dyn FnOnce(Result<Decoded, ImageError>) + Send> {
    Box::new(move |result| {
        dispatch(Box::new(move || {
            let inner = LOADERS.with(|loaders| {
                loaders
                    .borrow()
                    .iter()
                    .find(|(known, _)| *known == id)
                    .and_then(|(_, weak)| weak.upgrade())
            });
            if let Some(inner) = inner {
                inner.complete(&key, result);
            }
        }));
    })
}

impl Inner {
    fn generation(&self, scope: &str) -> u64 {
        self.generations.borrow().get(scope).copied().unwrap_or(0)
    }

    fn tick(&self) -> u64 {
        let now = self.clock.get() + 1;
        self.clock.set(now);
        now
    }

    /// Копия для той же области — сразу; иначе самая лёгкая копия с тем же вписыванием, у
    /// которой пикселей не меньше, чем нужно области (или уже исходный размер).
    fn lookup(
        &self,
        source: &str,
        target: Option<(u32, u32)>,
        fit: Fit,
        touch: bool,
    ) -> Option<Arc<Rgba8Image>> {
        let mut entries = self.entries.borrow_mut();
        let copies = entries.get_mut(source)?;
        let found = match copies
            .iter()
            .position(|entry| entry.target == target && entry.fit == fit)
        {
            Some(exact) => &mut copies[exact],
            None => copies
                .iter_mut()
                .filter(|entry| entry.fit == fit)
                .filter(|entry| {
                    let (natural_w, natural_h) = entry.natural;
                    let (need_w, need_h) = fitted_size(natural_w, natural_h, target, fit);
                    entry.width >= need_w && entry.height >= need_h
                })
                .min_by_key(|entry| entry.bytes)?,
        };
        if touch {
            found.used = self.tick();
        }
        Some(found.image.clone())
    }

    fn complete(&self, key: &str, result: Result<Decoded, ImageError>) {
        let Some(pending) = self.pending.borrow_mut().remove(key) else {
            return;
        };
        let current = pending.generation == self.generation(&pending.scope);
        let result = result.map(|decoded| {
            self.decodes.set(self.decodes.get() + 1);
            let bytes = decoded.pixels.byte_len();
            let (width, height, natural) = (decoded.width, decoded.height, decoded.natural);
            let image = Arc::new(decoded.pixels);
            if current {
                self.store(
                    &pending.source,
                    Entry {
                        image: image.clone(),
                        target: pending.target,
                        fit: pending.fit,
                        width,
                        height,
                        natural,
                        bytes,
                        scope: pending.scope.clone(),
                        used: self.tick(),
                    },
                );
            }
            image
        });
        if let Err(error) = &result
            && current
        {
            self.failed
                .borrow_mut()
                .insert(key.to_owned(), (error.clone(), pending.scope.clone()));
        }
        for callback in pending.callbacks {
            callback(result.clone());
        }
        let notify = self.on_ready.borrow().clone();
        if let Some(notify) = notify {
            notify();
        }
    }

    /// Положить копию (заменив копию той же области) и вытеснить лишнее.
    fn store(&self, source: &str, entry: Entry) {
        let (target, fit, bytes) = (entry.target, entry.fit, entry.bytes);
        let replaced = {
            let mut entries = self.entries.borrow_mut();
            let copies = entries.entry(source.to_owned()).or_default();
            match copies
                .iter()
                .position(|known| known.target == target && known.fit == fit)
            {
                Some(at) => std::mem::replace(&mut copies[at], entry).bytes,
                None => {
                    copies.push(entry);
                    0
                }
            }
        };
        self.bytes.set(self.bytes.get() + bytes - replaced);
        self.evict(Some((source, target, fit)));
    }

    /// Вытеснить давно не нужные копии, пока кеш больше предела; `keep` — только что
    /// положенная (её не трогать, даже если она одна больше предела).
    fn evict(&self, keep: Option<CopyId<'_>>) {
        let budget = self.budget.get();
        let mut entries = self.entries.borrow_mut();
        while self.bytes.get() > budget {
            let oldest = entries
                .iter()
                .flat_map(|(source, copies)| {
                    copies
                        .iter()
                        .enumerate()
                        .map(move |(at, entry)| (source, at, entry))
                })
                .filter(|(source, _, entry)| {
                    keep != Some((source.as_str(), entry.target, entry.fit))
                })
                .min_by_key(|(_, _, entry)| entry.used)
                .map(|(source, at, _)| (source.clone(), at));
            let Some((source, at)) = oldest else { break };
            if let Some(copies) = entries.get_mut(&source) {
                let entry = copies.remove(at);
                self.bytes.set(self.bytes.get() - entry.bytes);
                if copies.is_empty() {
                    entries.remove(&source);
                }
            }
        }
    }
}

fn fit_name(fit: Fit) -> &'static str {
    match fit {
        Fit::Cover => "cover",
        Fit::Contain => "contain",
    }
}
