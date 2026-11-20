//! Канал игры (`ChannelService.js`): подписанный удалённый манифест, ETag и
//! If-Modified-Since, три попытки, проверка схемы, игры, имени канала и подписи Ed25519.
//!
//! Ответ 304 берётся из кеша в памяти (по одному на игру). Кеш — запись `Arc<CacheEntry>`:
//! 0.4.7 меняет `checkedAt` в той же записи, и все, кто её держит, видят новое время.
//! Ошибки схемы и подписи, как и выключенный канал, не повторяются; прочие — до
//! `maxAttempts` раз с паузой `основа · 2ⁿ⁻¹` и разбросом.
//!
//! Свежесть (0.4.9, `acceptFresh`): подписанный манифест принимается, только если его `publishedAt`
//! не старше уже принятого по этому каналу. Принятые даты лежат в `game-channel-freshness.json`
//! (userData), испорченный файл считается пустым.

use crate::atomic::write_json_atomic;
use crate::channel_config::{self, ChannelConfig};
use crate::environment::Environment;
use crate::errors::{Code, DeliveryError, Failure, to_delivery_error};
use crate::http_client::{HttpClient, RequestOptions, read_body};
use crate::js_value::prop;
use crate::manifest_signing::{self, CheckOptions};
use crate::ports::{Logger, TracingLogger};
use crate::schema_validator::{channel_manifest_validator, describe_errors};
use crate::timing::{Clock, Random, iso_from_ms, retry_pause_ms, system_clock, system_random};
use hlsr_core::js;
use hlsr_core::json_store::read_json_safe;
use hlsr_core::json_text::json_parse;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const SUPPORTED_SCHEMA_VERSION: i64 = 1;
/// `publishedAt` дальше суток вперёд — ошибка часов при подписи: свежесть не сдвигает.
pub const FUTURE_SLACK_MS: i64 = 24 * 60 * 60 * 1000;
/// Канал больше 512 КБ не читается.
pub const MAX_BODY_BYTES: usize = 512 * 1024;
pub const DEFAULT_TIMEOUT_MS: u64 = 15_000;
pub const DEFAULT_ATTEMPTS: u32 = 3;
pub const DEFAULT_BACKOFF_MS: u64 = 500;

/// Параметры службы (`options` конструктора).
#[derive(Clone, Default)]
pub struct ChannelOptions {
    pub logger: Option<Arc<dyn Logger>>,
    /// Переменные и признак «production»; по умолчанию — окружение процесса.
    pub env: Option<Environment>,
    pub http: Option<HttpClient>,
    /// Простой соединения; `None` и 0 — 15 с.
    pub timeout: Option<Duration>,
    /// `None` и 0 — 3.
    pub max_attempts: Option<u32>,
    /// Основа паузы между попытками, мс; `None` — 500.
    pub backoff_base_ms: Option<u64>,
    /// Часы (`new Date()`); для тестов.
    pub clock: Option<Clock>,
    /// `Math.random()` для разброса паузы; для тестов.
    pub random: Option<Random>,
    /// `freshnessFile`: где помнить свежесть каналов; нет — только в памяти.
    pub freshness_file: Option<PathBuf>,
}

/// Свежесть каналов: `{"<gameId>/<channel> <url>": publishedAt}` и последняя дата из будущего,
/// о которой уже предупредили.
struct Freshness {
    file: Option<PathBuf>,
    seen: Map<String, Value>,
    future_warned: Option<String>,
}

impl Freshness {
    fn load(file: Option<PathBuf>) -> Freshness {
        let seen = match file.as_deref().map(read_json_safe) {
            Some(Value::Object(seen)) => seen,
            _ => Map::new(),
        };
        Freshness {
            file,
            seen,
            future_warned: None,
        }
    }
}

/// `Date.parse(value)`: не дата — `None` (NaN).
fn date_ms(value: Option<&Value>) -> Option<i64> {
    js::date_parse(&js::to_template_string(value))
}

/// Запись кеша канала одной игры.
pub struct CacheEntry {
    pub manifest: Value,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub fetched_at: String,
    checked_at: Mutex<String>,
}

impl CacheEntry {
    /// Когда канал в последний раз подтвердил этот ответ (200 или 304).
    pub fn checked_at(&self) -> String {
        self.checked_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Запись как объект 0.4.7: `{manifest, etag, lastModified, fetchedAt, checkedAt}`.
    pub fn to_json(&self) -> Value {
        json!({
            "manifest": self.manifest,
            "etag": self.etag,
            "lastModified": self.last_modified,
            "fetchedAt": self.fetched_at,
            "checkedAt": self.checked_at(),
        })
    }
}

/// Откуда взят манифест: `fresh` — скачан, `not-modified` — ответ 304, взят из кеша.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchStatus {
    Fresh,
    NotModified,
}

impl FetchStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            FetchStatus::Fresh => "fresh",
            FetchStatus::NotModified => "not-modified",
        }
    }
}

/// Итог `fetch`: `{status, manifest, config}`.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub status: FetchStatus,
    pub manifest: Value,
    pub config: ChannelConfig,
}

pub struct ChannelService {
    logger: Arc<dyn Logger>,
    env: Environment,
    http: HttpClient,
    timeout: Duration,
    max_attempts: u32,
    backoff_base_ms: u64,
    clock: Clock,
    random: Random,
    cache: Mutex<HashMap<String, Arc<CacheEntry>>>,
    freshness: Mutex<Freshness>,
}

fn disabled(game_id: &str) -> DeliveryError {
    DeliveryError::new(
        Code::ChannelDisabled,
        "Удалённый канал обновлений сборки пока не настроен",
    )
    .with_details(json!({ "gameId": game_id }))
}

impl ChannelService {
    /// `new ChannelService(options)`.
    pub fn new(options: ChannelOptions) -> ChannelService {
        ChannelService {
            logger: options.logger.unwrap_or_else(|| Arc::new(TracingLogger)),
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
            clock: options.clock.unwrap_or_else(system_clock),
            random: options.random.unwrap_or_else(system_random),
            cache: Mutex::new(HashMap::new()),
            freshness: Mutex::new(Freshness::load(options.freshness_file)),
        }
    }

    /// Файл свежести после создания службы (`channelFreshnessFile` фасада): читается сразу.
    pub fn set_freshness_file(&self, file: PathBuf) {
        *self
            .freshness
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Freshness::load(Some(file));
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<CacheEntry>>> {
        self.cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn iso_now(&self) -> String {
        iso_from_ms((self.clock)())
    }

    /// `configFor(gameId)`: настройки канала игры; нет такой игры — `None`.
    pub fn config_for(&self, game_id: &str) -> Option<ChannelConfig> {
        channel_config::resolve(game_id, &self.env)
    }

    /// `cachedFor(gameId)`.
    pub fn cached_for(&self, game_id: &str) -> Option<Arc<CacheEntry>> {
        self.cache().get(game_id).cloned()
    }

    /// `fetch(gameId, {ignoreCache})`. С `ignore_cache` условные заголовки не посылаются, но
    /// ответ 304 (пришёл сам) всё равно берётся из кеша.
    pub async fn fetch(&self, game_id: &str, ignore_cache: bool) -> Result<Fetched, DeliveryError> {
        let config = self
            .config_for(game_id)
            .filter(|config| config.url.is_some())
            .ok_or_else(|| disabled(game_id))?;

        let cached = self.cached_for(game_id);
        let mut headers: Vec<(String, String)> = Vec::new();
        if let Some(cached) = cached.as_ref().filter(|_| !ignore_cache) {
            if let Some(etag) = &cached.etag {
                headers.push(("if-none-match".to_owned(), etag.clone()));
            }
            if let Some(last_modified) = &cached.last_modified {
                headers.push(("if-modified-since".to_owned(), last_modified.clone()));
            }
        }

        let mut last_error = None;
        for attempt in 1..=self.max_attempts {
            match self
                .fetch_once(game_id, &config, &headers, cached.as_ref())
                .await
            {
                Ok(fetched) => return Ok(fetched),
                Err(failure) => {
                    let error = to_delivery_error(failure, Some(Code::ChannelUnavailable));
                    if matches!(
                        error.code,
                        Code::ManifestInvalid | Code::SignatureInvalid | Code::ChannelDisabled
                    ) {
                        return Err(error);
                    }
                    if attempt == self.max_attempts {
                        last_error = Some(error);
                        break;
                    }
                    let pause = retry_pause_ms(self.backoff_base_ms, attempt, &self.random);
                    self.logger.warn(&format!(
                        "[game-delivery] канал {game_id}: попытка {attempt} не удалась ({}), \
                         повтор через {pause} мс",
                        error.code
                    ));
                    last_error = Some(error);
                    tokio::time::sleep(Duration::from_millis(pause)).await;
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            DeliveryError::new(Code::ChannelUnavailable, "Канал обновлений недоступен")
        }))
    }

    async fn fetch_once(
        &self,
        game_id: &str,
        config: &ChannelConfig,
        headers: &[(String, String)],
        cached: Option<&Arc<CacheEntry>>,
    ) -> Result<Fetched, Failure> {
        let options = RequestOptions {
            headers: headers.to_vec(),
            timeout: Some(self.timeout),
            ..RequestOptions::default()
        };
        let url = config.url.as_deref().unwrap_or_default();
        let response = self.http.request(url, &options).await?;
        let status = response.status();

        if status == 304 {
            drop(response);
            let Some(cached) = cached else {
                return Err(DeliveryError::new(
                    Code::ChannelUnavailable,
                    "HTTP 304 без сохранённого ответа",
                )
                .into());
            };
            self.logger.info(&format!(
                "[game-delivery] канал {game_id}: HTTP 304, беру ответ из кеша"
            ));
            *cached
                .checked_at
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = self.iso_now();
            return Ok(Fetched {
                status: FetchStatus::NotModified,
                manifest: cached.manifest.clone(),
                config: config.clone(),
            });
        }

        if status != 200 {
            drop(response);
            return Err(DeliveryError::new(
                Code::ChannelUnavailable,
                format!("Канал обновлений ответил HTTP {status}"),
            )
            .with_details(json!({ "statusCode": status }))
            .into());
        }

        let etag = response.header("etag").filter(|value| !value.is_empty());
        let last_modified = response
            .header("last-modified")
            .filter(|value| !value.is_empty());
        let body = read_body(response, Some(MAX_BODY_BYTES), None).await?;

        let data = json_parse(&String::from_utf8_lossy(&body)).map_err(|error| {
            DeliveryError::new(
                Code::ManifestInvalid,
                format!("Channel manifest не является JSON: {error}"),
            )
        })?;
        let manifest = self.validate(&data, game_id, config)?.clone();
        self.accept_fresh(game_id, config, &manifest)?;

        let now = self.iso_now();
        let version = js::to_template_string(manifest.get("version"));
        self.cache().insert(
            game_id.to_owned(),
            Arc::new(CacheEntry {
                manifest: manifest.clone(),
                etag,
                last_modified,
                fetched_at: now.clone(),
                checked_at: Mutex::new(now),
            }),
        );
        self.logger.info(&format!(
            "[game-delivery] канал {game_id}: версия {version}, подпись {}",
            if config.require_signature {
                "проверена"
            } else {
                "не требуется"
            }
        ));
        Ok(Fetched {
            status: FetchStatus::Fresh,
            manifest,
            config: config.clone(),
        })
    }

    /// `validate(data, gameId, config)`: объект, версия схемы, схема, игра, имя канала, подпись —
    /// в таком порядке. Возвращает те же данные.
    pub fn validate<'a>(
        &self,
        data: &'a Value,
        game_id: &str,
        config: &ChannelConfig,
    ) -> Result<&'a Value, DeliveryError> {
        let invalid = |message: String| DeliveryError::new(Code::ManifestInvalid, message);

        if !data.is_object() {
            return Err(invalid(
                "Channel manifest должен быть JSON-объектом".to_owned(),
            ));
        }

        let schema_version = prop(data, "schemaVersion");
        let supported = schema_version
            .and_then(Value::as_number)
            .is_some_and(|n| n.as_f64() == Some(SUPPORTED_SCHEMA_VERSION as f64));
        if !supported {
            return Err(invalid(format!(
                "Неподдерживаемая версия схемы channel manifest: {}",
                js::to_template_string(schema_version)
            )));
        }

        let schema_errors = channel_manifest_validator().validate(data);
        if !schema_errors.is_empty() {
            let errors = describe_errors(&schema_errors);
            return Err(invalid(format!(
                "Channel manifest не прошёл валидацию: {}",
                errors[0]
            ))
            .with_details(json!({
                "errors": errors.iter().take(20).collect::<Vec<_>>(),
            })));
        }

        if prop(data, "gameId").and_then(Value::as_str) != Some(game_id) {
            return Err(invalid(format!(
                "Channel manifest описывает {}, запрошен {game_id}",
                js::to_template_string(prop(data, "gameId"))
            )));
        }

        if !config.channel.is_empty()
            && prop(data, "channel").and_then(Value::as_str) != Some(config.channel.as_str())
        {
            return Err(invalid(format!(
                "Channel manifest относится к каналу {}, ожидался {}",
                js::to_template_string(prop(data, "channel")),
                config.channel
            )));
        }

        self.verify_signature(data, config)?;
        Ok(data)
    }

    /// `acceptFresh(gameId, config, manifest)`: повтор старого подписанного манифеста не должен
    /// молча замораживать или откатывать обновления. `publishedAt` подписан и ставится в момент
    /// подписи, поэтому принимается только не старее уже принятого. Откат выпуска владельцем —
    /// новой подписью прежней версии.
    fn accept_fresh(
        &self,
        game_id: &str,
        config: &ChannelConfig,
        manifest: &Value,
    ) -> Result<(), DeliveryError> {
        if !config.require_signature {
            return Ok(());
        }
        let key = format!(
            "{game_id}/{} {}",
            config.channel,
            config.url.as_deref().unwrap_or_default()
        );
        let published = prop(manifest, "publishedAt");
        let mut freshness = self
            .freshness
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let seen = freshness.seen.get(&key).cloned();

        if let Some(seen) = seen.as_ref().filter(|seen| js::truthy(seen))
            && let (Some(at), Some(accepted)) = (date_ms(published), date_ms(Some(seen)))
            && at < accepted
        {
            return Err(DeliveryError::new(
                Code::ManifestInvalid,
                format!(
                    "Канал {game_id} отдал манифест от {}, а уже принят от {}: старый манифест не принимаю",
                    js::to_template_string(published),
                    js::to_template_string(Some(seen))
                ),
            )
            .with_details(json!({"publishedAt": published, "accepted": seen})));
        }
        let Some(published) = published.filter(|_| seen.as_ref() != published) else {
            return Ok(());
        };

        // Запомнить дату из будущего — значит не принимать честные выпуски до неё: такой манифест
        // принимается, но свежесть не сдвигает (и его повтор тоже).
        if date_ms(Some(published)).is_some_and(|at| at > (self.clock)() + FUTURE_SLACK_MS) {
            let mark = js::to_template_string(Some(published));
            // Проверка идёт каждые несколько минут — одной записи на дату хватит.
            if freshness.future_warned.as_deref() != Some(mark.as_str()) {
                self.logger.warn(&format!(
                    "[game-delivery] канал {game_id}: publishedAt {mark} из будущего — свежесть не запоминаю"
                ));
            }
            freshness.future_warned = Some(mark);
            return Ok(());
        }

        freshness.seen.insert(key, published.clone());
        let Some(file) = freshness.file.clone() else {
            return Ok(());
        };
        if let Err(error) = write_json_atomic(&file, &Value::Object(freshness.seen.clone())) {
            self.logger.warn(&format!(
                "[game-delivery] свежесть канала {game_id} не сохранена: {}",
                error.message
            ));
        }
        Ok(())
    }

    /// `verifySignature(data, config)`: при обязательной подписи — `check` с `required`.
    fn verify_signature(&self, data: &Value, config: &ChannelConfig) -> Result<(), DeliveryError> {
        if !config.require_signature {
            return Ok(());
        }
        manifest_signing::check(
            data,
            &CheckOptions {
                required: true,
                public_keys: &config.public_keys,
            },
        )
        .map(|_| ())
        .map_err(|error| {
            DeliveryError::new(
                Code::SignatureInvalid,
                format!("Подпись channel manifest не принята: {}", error.message),
            )
        })
    }
}

impl std::fmt::Debug for ChannelService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelService")
            .field("timeout", &self.timeout)
            .field("max_attempts", &self.max_attempts)
            .finish_non_exhaustive()
    }
}
