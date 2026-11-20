//! Кеш файловых манифестов (`FilesManifestStore.js`): `.hlsr/manifests/<игра>/<версия>.files.json`
//! (байты как скачаны) и `<версия>.trust.json` (размер, sha256, источник, хеш канала). Манифест
//! принимается из кеша, только если рядом есть запись доверия, совпадают игра, версия, размер и
//! sha256 и манифест проходит проверку; иначе оба файла убираются. Загрузка идёт по зеркалам
//! (`MirrorRunner`), на каждом — до трёх попыток.

use crate::atomic::{fd_error, write_file_atomic, write_json_atomic};
use crate::environment::Environment;
use crate::errors::{Code, DeliveryError, Failure, RawError, to_delivery_error};
use crate::feature_flag;
use crate::file_ops::{hex, rm_sync};
use crate::files_manifest::{self, MAX_ENTRIES, ValidateOptions};
use crate::http_client::{
    HttpClient, RequestOptions, is_loopback, parse_url, read_body, url_hostname,
};
use crate::js_value::{self as jv, prop};
use crate::manifest_service::ManifestService;
use crate::mirror_runner::{Attempt, AttemptRecord, MirrorRequest, MirrorRunner};
use crate::node_fs::mkdir_recursive;
use crate::paths::{LibraryPaths, assert_inside};
use crate::ports::{Logger, TracingLogger};
use crate::providers::{self, Provider, ProviderOptions};
use crate::timing::{Clock, Random, iso_from_ms, retry_pause_ms, system_clock, system_random};
use hlsr_core::js::{self, errno, fs::read_dir_names, fs::read_file};
use hlsr_core::json_store::read_json_safe;
use hlsr_core::json_text::json_parse;
use regex::Regex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

pub const META_VERSION: i64 = 1;
/// Манифест больше 64 МБ не принимается ни из кеша, ни из сети.
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub const DEFAULT_ATTEMPTS: u32 = 3;
pub const DEFAULT_BACKOFF_MS: u64 = 500;

/// `/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/` (`\d` JS — только `0-9`).
static VERSION_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$").unwrap());
/// `/^[a-z0-9]+(-[a-z0-9]+)*$/`.
static GAME_ID_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9]+(-[a-z0-9]+)*$").unwrap());
/// `/^[^:*?"<>|]+\.json$/`.
static FILENAME_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^[^:*?"<>|]+\.json$"#).unwrap());

/// Параметры хранилища (`options` конструктора, кроме `paths`).
#[derive(Clone, Default)]
pub struct StoreOptions {
    pub logger: Option<Arc<dyn Logger>>,
    /// Переменные и признак «production»; по умолчанию — окружение процесса.
    pub env: Option<Environment>,
    pub http: Option<HttpClient>,
    /// Простой соединения; `None` и 0 — 30 с.
    pub timeout: Option<Duration>,
    /// `None` и 0 — 3.
    pub max_attempts: Option<u32>,
    /// Основа паузы между попытками, мс; `None` — 500.
    pub backoff_base_ms: Option<u64>,
    /// Перебор зеркал; по умолчанию — свой.
    pub mirrors: Option<Arc<MirrorRunner>>,
    /// Часы (`new Date()`); для тестов.
    pub clock: Option<Clock>,
    /// `Math.random()` для разброса паузы; для тестов.
    pub random: Option<Random>,
}

/// Параметры `fetch` (`options`): хеш канала, из которого взято описание, и источник записи.
#[derive(Debug, Clone, Default)]
pub struct FetchOptions {
    /// `channelSha256`: пустая строка — как отсутствие.
    pub channel_sha256: Option<String>,
    /// `source`: по умолчанию — `{id, type}` источника; ложное значение — `"channel"`.
    pub source: Option<Value>,
}

pub struct FilesManifestStore {
    internal: PathBuf,
    logger: Arc<dyn Logger>,
    env: Environment,
    http: HttpClient,
    timeout: Duration,
    max_attempts: u32,
    backoff_base_ms: u64,
    mirrors: Arc<MirrorRunner>,
    clock: Clock,
    random: Random,
    last_mirror_attempts: Mutex<Vec<AttemptRecord>>,
}

fn path_invalid(message: String) -> DeliveryError {
    DeliveryError::new(Code::PathInvalid, message)
}

fn invalid(message: &str) -> DeliveryError {
    DeliveryError::new(Code::ManifestInvalid, message)
}

/// `String(value || "")`.
fn text_or_empty(value: Option<&Value>) -> String {
    value
        .filter(|value| js::truthy(value))
        .map(js::to_string)
        .unwrap_or_default()
}

impl FilesManifestStore {
    /// `new FilesManifestStore({paths, …})`: каталог кеша — `paths.internal/manifests`.
    pub fn new(paths: &LibraryPaths, options: StoreOptions) -> FilesManifestStore {
        let logger: Arc<dyn Logger> = options.logger.unwrap_or_else(|| Arc::new(TracingLogger));
        let clock = options.clock.unwrap_or_else(system_clock);
        FilesManifestStore {
            internal: paths.internal.clone(),
            env: options.env.unwrap_or_else(Environment::process),
            http: options.http.unwrap_or_default(),
            timeout: options
                .timeout
                .filter(|timeout| !timeout.is_zero())
                .unwrap_or(Duration::from_millis(DEFAULT_TIMEOUT_MS)),
            max_attempts: options
                .max_attempts
                .filter(|&n| n != 0)
                .unwrap_or(DEFAULT_ATTEMPTS),
            backoff_base_ms: options.backoff_base_ms.unwrap_or(DEFAULT_BACKOFF_MS),
            mirrors: options.mirrors.unwrap_or_else(|| {
                Arc::new(MirrorRunner::with_clock(logger.clone(), clock.clone()))
            }),
            random: options.random.unwrap_or_else(system_random),
            clock,
            logger,
            last_mirror_attempts: Mutex::new(Vec::new()),
        }
    }

    /// `rootDir()`.
    pub fn root_dir(&self) -> PathBuf {
        self.internal.join("manifests")
    }

    /// `gameDir(gameId)`.
    pub fn game_dir(&self, game_id: &str) -> Result<PathBuf, DeliveryError> {
        if !GAME_ID_PATTERN.is_match(game_id) {
            return Err(path_invalid(format!("Недопустимый gameId: {game_id}")));
        }
        let root = self.root_dir();
        assert_inside(&root, &root.join(game_id), Some("Папка манифестов"))
    }

    /// `manifestPath(gameId, version)`: версия проверяется раньше игры.
    pub fn manifest_path(&self, game_id: &str, version: &str) -> Result<PathBuf, DeliveryError> {
        if !VERSION_PATTERN.is_match(version) {
            return Err(path_invalid(format!("Недопустимая версия: {version}")));
        }
        let dir = self.game_dir(game_id)?;
        assert_inside(
            &dir,
            &dir.join(format!("{version}.files.json")),
            Some("Файловый манифест"),
        )
    }

    /// `metaPath(gameId, version)`: версия здесь не проверяется по шаблону.
    pub fn meta_path(&self, game_id: &str, version: &str) -> Result<PathBuf, DeliveryError> {
        let dir = self.game_dir(game_id)?;
        assert_inside(
            &dir,
            &dir.join(format!("{version}.trust.json")),
            Some("Metadata манифеста"),
        )
    }

    /// `has(gameId, version)`.
    pub fn has(&self, game_id: &str, version: &str) -> bool {
        self.manifest_path(game_id, version)
            .is_ok_and(|file| file.exists())
    }

    /// `list(gameId)`: версии в кеше, по порядку строк JS.
    pub fn list(&self, game_id: &str) -> Vec<String> {
        let Ok(dir) = self.game_dir(game_id) else {
            return Vec::new();
        };
        let Ok(names) = read_dir_names(&dir) else {
            return Vec::new();
        };
        let mut versions: Vec<String> = names
            .iter()
            .filter_map(|name| name.strip_suffix(".files.json"))
            .map(str::to_owned)
            .collect();
        versions.sort_by(|a, b| js::cmp_utf16(a, b));
        versions
    }

    /// `load(gameId, version)`: проверенный манифест из кеша; не прошёл проверку — `None`, и
    /// файлы убираются.
    pub fn load(&self, game_id: &str, version: &str) -> Option<Value> {
        let (file, meta_file) = match (
            self.manifest_path(game_id, version),
            self.meta_path(game_id, version),
        ) {
            (Ok(file), Ok(meta_file)) => (file, meta_file),
            _ => return None,
        };
        if !file.exists() {
            return None;
        }

        let meta = read_json_safe(&meta_file);
        let version_ok = prop(&meta, "metaVersion")
            .and_then(jv::number)
            .is_some_and(|n| n == META_VERSION as f64);
        if !version_ok {
            self.reject(game_id, version, "рядом нет metadata доверия");
            return None;
        }
        if prop(&meta, "gameId").and_then(Value::as_str) != Some(game_id)
            || prop(&meta, "version").and_then(Value::as_str) != Some(version)
        {
            self.reject(
                game_id,
                version,
                &format!(
                    "metadata описывает {} {}",
                    js::to_template_string(prop(&meta, "gameId")),
                    js::to_template_string(prop(&meta, "version"))
                ),
            );
            return None;
        }

        let raw = match read_file(&file) {
            Ok(raw) => raw,
            Err(error) => {
                // У каталога на месте файла чтение падает без пути в тексте (`EISDIR … read`).
                let message = if errno::code(&error).as_deref() == Some("EISDIR") {
                    fd_error(&error, "read").message
                } else {
                    RawError::fs(&error, "open", &file).message
                };
                self.reject(game_id, version, &format!("файл не читается: {message}"));
                return None;
            }
        };

        if raw.len() as u64 > MAX_MANIFEST_BYTES {
            self.reject(
                game_id,
                version,
                &format!("размер {} превышает предел", raw.len()),
            );
            return None;
        }
        // `meta.size != null && raw.length !== meta.size`.
        if let Some(size) = prop(&meta, "size").filter(|size| !size.is_null())
            && jv::number(size) != Some(raw.len() as f64)
        {
            self.reject(
                game_id,
                version,
                &format!(
                    "размер {} вместо {}",
                    raw.len(),
                    js::to_template_string(Some(size))
                ),
            );
            return None;
        }
        let digest = hex(&Sha256::digest(&raw));
        if prop(&meta, "sha256").and_then(Value::as_str) != Some(digest.as_str()) {
            self.reject(
                game_id,
                version,
                &format!(
                    "SHA-256 {digest} вместо {}",
                    js::to_template_string(prop(&meta, "sha256"))
                ),
            );
            return None;
        }

        let data = match json_parse(&String::from_utf8_lossy(&raw)) {
            Ok(data) => data,
            Err(error) => {
                self.reject(
                    game_id,
                    version,
                    &format!("не разбирается как JSON: {error}"),
                );
                return None;
            }
        };
        let options = ValidateOptions {
            game_id: Some(game_id.to_owned()),
            version: Some(version.to_owned()),
            max_entries: Some(MAX_ENTRIES as f64),
            require_manifest_sha256: true,
        };
        match files_manifest::validate(
            &data,
            Some(&format!("cache:{game_id}-{version}")),
            Some(&options),
        ) {
            Ok(_) => Some(data),
            Err(error) => {
                self.reject(game_id, version, &error.message);
                None
            }
        }
    }

    /// `reject(gameId, version, reason)`: запись в журнал и удаление манифеста и метаданных.
    fn reject(&self, game_id: &str, version: &str, reason: &str) {
        self.logger.warn(&format!(
            "[game-delivery] кешированный файловый манифест {game_id} {version} отклонён: {reason}"
        ));
        let files = [
            self.manifest_path(game_id, version),
            self.meta_path(game_id, version),
        ];
        for file in files.into_iter().flatten() {
            if file.exists()
                && let Err(error) = rm_sync(&file, false, 0, Duration::ZERO)
            {
                self.logger.warn(&format!(
                    "[game-delivery] не удалось убрать {}: {}",
                    file.display(),
                    error.message
                ));
            }
        }
    }

    /// `store(gameId, version, raw, meta)`: манифест — байтами, как скачан, рядом запись
    /// доверия `{metaVersion, gameId, version, size, storedAt, …meta}`. Каталог игры создаётся
    /// до проверки версии (как в 0.4.7).
    pub fn store(
        &self,
        game_id: &str,
        version: &str,
        raw: &[u8],
        meta: Option<&Map<String, Value>>,
    ) -> Result<PathBuf, Failure> {
        let dir = self.game_dir(game_id)?;
        mkdir_recursive(&dir)?;

        let file = self.manifest_path(game_id, version)?;
        write_file_atomic(&file, raw)?;

        let mut record = Map::new();
        record.insert("metaVersion".to_owned(), json!(META_VERSION));
        record.insert("gameId".to_owned(), json!(game_id));
        record.insert("version".to_owned(), json!(version));
        record.insert("size".to_owned(), json!(raw.len()));
        record.insert("storedAt".to_owned(), json!(iso_from_ms((self.clock)())));
        // `Object.assign`: ключ, который уже есть, меняет значение и остаётся на своём месте.
        for (key, value) in meta.into_iter().flatten() {
            record.insert(key.clone(), value.clone());
        }
        write_json_atomic(&self.meta_path(game_id, version)?, &Value::Object(record))?;
        Ok(file)
    }

    /// `fetch(gameId, descriptor, options)`: скачать манифест по описанию из канала, проверить
    /// и сохранить в кеш. Описание — `filesManifest` канала.
    pub async fn fetch(
        &self,
        game_id: &str,
        descriptor: &Value,
        options: &FetchOptions,
    ) -> Result<Value, DeliveryError> {
        self.assert_descriptor(game_id, descriptor)?;

        let allowed = feature_flag::allowed_provider_types(&self.env);
        // `(descriptor.providers || []).filter(...)`: не список считается пустым (в 0.4.7 это
        // TypeError; канал со схемой такого описания не пропускает).
        let usable: Vec<Value> = prop(descriptor, "providers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|provider| {
                prop(provider, "type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| allowed.contains(&kind))
            })
            .cloned()
            .collect();
        if usable.is_empty() {
            return Err(DeliveryError::new(
                Code::RemoteDisabled,
                "Источник файлового манифеста выключен feature flag'ом",
            )
            .with_details(json!({
                "gameId": game_id,
                "version": prop(descriptor, "version").cloned().unwrap_or(Value::Null),
            })));
        }

        let version = js::to_template_string(prop(descriptor, "version"));
        let artifact = format!("{game_id}:files-manifest:{version}");
        let ordered = ManifestService::order_providers(&usable);
        let provider_options = ProviderOptions::with_allowed_provider_types(
            self.http.clone(),
            allowed.iter().copied(),
        );

        let outcome = self
            .mirrors
            .run(
                MirrorRequest {
                    artifact: &artifact,
                    descriptors: &ordered,
                    is_terminal_result: None,
                    on_discard_partial: None,
                },
                |attempt: Attempt| {
                    let (provider_options, version) = (&provider_options, &version);
                    async move {
                        self.fetch_from(
                            game_id,
                            descriptor,
                            options,
                            version,
                            &attempt.descriptor,
                            provider_options,
                        )
                        .await
                    }
                },
            )
            .await?;

        *self
            .last_mirror_attempts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = outcome.attempts;
        Ok(outcome.value)
    }

    /// Операция `fetch` на одном источнике: до `max_attempts` попыток скачать и принять.
    async fn fetch_from(
        &self,
        game_id: &str,
        descriptor: &Value,
        options: &FetchOptions,
        version: &str,
        entry: &Value,
        provider_options: &ProviderOptions,
    ) -> Result<Value, Failure> {
        let provider = providers::create(entry, provider_options)?;

        // `{id: entry.id, type: entry.type}`: отсутствующее поле в объект не попадает.
        let mut source = Map::new();
        for key in ["id", "type"] {
            if let Some(value) = prop(entry, key) {
                source.insert(key.to_owned(), value.clone());
            }
        }
        let source = Value::Object(source);

        let mut last_error = None;
        for attempt in 1..=self.max_attempts {
            let result = async {
                let raw = self.download(provider.as_ref(), descriptor).await?;
                self.accept(game_id, descriptor, &raw, options, &source)
            }
            .await;
            match result {
                Ok(manifest) => return Ok(manifest),
                Err(failure) => {
                    let error = to_delivery_error(failure, Some(Code::ManifestNotFound));
                    if matches!(
                        error.code,
                        Code::ManifestInvalid
                            | Code::ChecksumMismatch
                            | Code::SizeMismatch
                            | Code::ArchiveUnsafeEntry
                    ) {
                        return Err(error.into());
                    }
                    if attempt == self.max_attempts {
                        last_error = Some(error);
                        break;
                    }
                    let pause = retry_pause_ms(self.backoff_base_ms, attempt, &self.random);
                    self.logger.warn(&format!(
                        "[game-delivery] файловый манифест {game_id} {version}: попытка \
                         {attempt} не удалась ({}), повтор через {pause} мс",
                        error.code
                    ));
                    last_error = Some(error);
                    tokio::time::sleep(Duration::from_millis(pause)).await;
                }
            }
        }
        Err(last_error
            .unwrap_or_else(|| {
                DeliveryError::new(Code::ManifestNotFound, "Файловый манифест недоступен")
            })
            .into())
    }

    /// `assertDescriptor(gameId, descriptor)`.
    fn assert_descriptor(&self, game_id: &str, descriptor: &Value) -> Result<(), DeliveryError> {
        if !js::truthy(descriptor) {
            return Err(DeliveryError::new(
                Code::ManifestNotFound,
                format!("Канал не объявляет файловый манифест для {game_id}"),
            ));
        }
        let version = prop(descriptor, "version");
        if !VERSION_PATTERN.is_match(&text_or_empty(version)) {
            return Err(invalid(&format!(
                "Недопустимая версия: {}",
                js::to_template_string(version)
            )));
        }
        if !jv::is_hex64(prop(descriptor, "sha256")) {
            return Err(invalid("В описании манифеста нет корректного SHA-256"));
        }
        let size = prop(descriptor, "size");
        if !jv::is_integer(size) || jv::as_number(size) <= 0.0 {
            return Err(invalid("В описании манифеста нет корректного размера"));
        }
        if jv::as_number(size) > MAX_MANIFEST_BYTES as f64 {
            return Err(invalid(&format!(
                "Заявленный размер манифеста {} превышает предел {MAX_MANIFEST_BYTES}",
                js::to_template_string(size)
            )));
        }
        let filename = text_or_empty(prop(descriptor, "filename"));
        if filename.is_empty() || filename.contains(['/', '\\']) || filename.contains("..") {
            return Err(path_invalid(format!(
                "Недопустимое имя файла манифеста: {filename}"
            )));
        }
        if !FILENAME_PATTERN.is_match(&filename) {
            return Err(path_invalid(format!(
                "Недопустимое имя файла манифеста: {filename}"
            )));
        }
        Ok(())
    }

    /// `download(provider, descriptor)`: файл рядом с архивом, не больше заявленного размера.
    async fn download(
        &self,
        provider: &dyn Provider,
        descriptor: &Value,
    ) -> Result<Vec<u8>, Failure> {
        let filename = text_or_empty(prop(descriptor, "filename"));
        let sibling = provider.resolve_sibling(&filename)?;

        let url = parse_url(&sibling.url)?;
        let https = url.scheme() == "https";
        if !https && !(url.scheme() == "http" && is_loopback(url_hostname(&url))) {
            return Err(DeliveryError::new(
                Code::InsecureRedirect,
                format!(
                    "Файловый манифест допускается только по HTTPS (или локальному HTTP): {}:",
                    url.scheme()
                ),
            )
            .into());
        }

        let options = RequestOptions {
            headers: sibling.headers.clone(),
            timeout: Some(self.timeout),
            ..RequestOptions::default()
        };
        let response = self.http.request(&sibling.url, &options).await?;
        let status = response.status();
        if status != 200 {
            drop(response);
            return Err(DeliveryError::new(
                Code::ManifestNotFound,
                format!("Файловый манифест недоступен: HTTP {status}"),
            )
            .with_details(json!({ "statusCode": status }))
            .into());
        }
        let limit = jv::as_number(prop(descriptor, "size")) as usize;
        Ok(read_body(response, Some(limit), None).await?)
    }

    /// `accept(gameId, descriptor, raw, options)`: размер, sha256, разбор, проверка, запись.
    fn accept(
        &self,
        game_id: &str,
        descriptor: &Value,
        raw: &[u8],
        options: &FetchOptions,
        provider_source: &Value,
    ) -> Result<Value, Failure> {
        let size = prop(descriptor, "size");
        if Some(raw.len() as f64) != size.and_then(jv::number) {
            return Err(DeliveryError::new(
                Code::SizeMismatch,
                format!(
                    "Размер файлового манифеста {}, ожидался {}",
                    raw.len(),
                    js::to_template_string(size)
                ),
            )
            .into());
        }
        let digest = hex(&Sha256::digest(raw));
        let expected = text_or_empty(prop(descriptor, "sha256"));
        if digest != expected {
            return Err(DeliveryError::new(
                Code::ChecksumMismatch,
                "SHA-256 файлового манифеста не совпал",
            )
            .with_details(json!({ "expected": expected, "actual": digest }))
            .into());
        }
        let data = json_parse(&String::from_utf8_lossy(raw)).map_err(|error| {
            DeliveryError::new(
                Code::ManifestInvalid,
                format!("Файловый манифест не является JSON: {error}"),
            )
        })?;

        let version = text_or_empty(prop(descriptor, "version"));
        let validate = ValidateOptions {
            game_id: Some(game_id.to_owned()),
            version: Some(version),
            max_entries: Some(MAX_ENTRIES as f64),
            require_manifest_sha256: true,
        };
        files_manifest::validate(&data, Some(&format!("channel:{game_id}")), Some(&validate))?;

        // `opts.source || "channel"`, где `opts = Object.assign({source}, options)`.
        let source = match &options.source {
            Some(value) if js::truthy(value) => value.clone(),
            Some(_) => json!("channel"),
            None => provider_source.clone(),
        };
        let mut meta = Map::new();
        meta.insert("sha256".to_owned(), json!(digest));
        meta.insert("source".to_owned(), source);
        meta.insert(
            "channelSha256".to_owned(),
            options
                .channel_sha256
                .as_ref()
                .filter(|sha| !sha.is_empty())
                .map_or(Value::Null, |sha| json!(sha)),
        );
        // `totalFiles: manifest.totalFiles` без значения в запись не попадает.
        for key in ["totalFiles", "manifestSha256"] {
            if let Some(value) = prop(&data, key) {
                meta.insert(key.to_owned(), value.clone());
            }
        }
        let file = self.store(game_id, &version_text(descriptor), raw, Some(&meta))?;

        self.logger.info(&format!(
            "[game-delivery] файловый манифест {game_id} {} проверен и сохранён: {} файлов, {} Б, {}",
            js::to_template_string(prop(descriptor, "version")),
            js::to_template_string(prop(&data, "totalFiles")),
            raw.len(),
            file.display()
        ));
        Ok(data)
    }

    /// `lastMirrorAttempts`: попытки последней удачной загрузки.
    pub fn last_mirror_attempts(&self) -> Vec<AttemptRecord> {
        self.last_mirror_attempts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

fn version_text(descriptor: &Value) -> String {
    text_or_empty(prop(descriptor, "version"))
}

impl std::fmt::Debug for FilesManifestStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilesManifestStore")
            .field("root", &self.root_dir())
            .finish_non_exhaustive()
    }
}
