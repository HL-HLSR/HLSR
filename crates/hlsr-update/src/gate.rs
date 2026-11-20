//! Шлюз обновления лаунчера (`launcher-update/LauncherUpdateGate.js`): канал по основному, затем
//! запасному адресу; подпись, версия совпадает с обещанной лентой, нет понижения, SHA-256
//! `latest.yml` совпадает с `latestYmlSha256` (сам файл качается заново); проверка скачанного
//! установщика по проверенным метаданным.

use crate::cancel::CancelToken;
use crate::channel::{self, CheckOptions, VerifyOptions, VerifyOutcome};
use crate::errors::LuCode;
use crate::ports::{GetError, GetOptions, HttpGet, Logger};
use hlsr_core::js::{self, Platform};
use hlsr_core::json_text::json_parse;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Журнал, который молчит: `logger` по умолчанию у шлюза 0.4.7.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullLogger;

impl Logger for NullLogger {
    fn info(&self, _message: &str) {}
    fn warn(&self, _message: &str) {}
    fn error(&self, _message: &str) {}
}

/// Почему не удалось получить канал или оценить его. Тексты — как у `Error` в 0.4.7.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GateError {
    /// Сбой `httpGet`: HTTP, сеть, allowlist, лимиты.
    #[error("{0}")]
    Get(#[from] GetError),
    /// `не JSON: <текст V8>` — текст ошибки разбора свой.
    #[error("не JSON: {0}")]
    NotJson(String),
    /// Установленная версия не SemVer — `compareSemver` бросает исключение.
    #[error("{0}")]
    NotSemver(#[from] channel::NotSemver),
    /// Каналов нет вовсе.
    #[error("каналы обновления лаунчера недоступны")]
    Unavailable,
}

impl GateError {
    /// Класс ошибки для сверки с 0.4.7 (тексты сетевых ошибок и V8 у Node свои).
    pub fn kind(&self) -> String {
        match self {
            GateError::Get(error) => error.kind(),
            GateError::NotJson(_) => "not-json".to_owned(),
            GateError::NotSemver(_) => "not-semver".to_owned(),
            GateError::Unavailable => "unavailable".to_owned(),
        }
    }
}

/// Результат оценки канала (`outcome` в `evaluateManifest`).
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub ok: bool,
    pub problems: Vec<String>,
    pub update_available: bool,
    pub downgrade: bool,
    pub unsupported: bool,
    pub code: Option<LuCode>,
    pub update_required: bool,
    /// Проверенный документ канала; у отклонённого — `None`.
    pub manifest: Option<Value>,
    pub source_url: String,
}

impl Outcome {
    /// Форма 0.4.7: поля в том же порядке.
    pub fn to_json(&self) -> Value {
        json!({
            "ok": self.ok,
            "problems": self.problems,
            "updateAvailable": self.update_available,
            "downgrade": self.downgrade,
            "unsupported": self.unsupported,
            "code": self.code.map(LuCode::as_str),
            "updateRequired": self.update_required,
            "manifest": self.manifest,
            "sourceUrl": self.source_url,
        })
    }
}

/// Документ канала и адрес, откуда он получен.
#[derive(Debug, Clone, PartialEq)]
pub struct FetchedChannel {
    pub manifest: Value,
    pub source_url: String,
}

/// Параметры шлюза (`options` конструктора).
#[derive(Clone)]
pub struct GateOptions {
    pub current_version: String,
    /// Адреса канала по порядку: основной, запасной.
    pub channel_urls: Vec<String>,
    /// Адреса `latest.yml` по порядку.
    pub latest_yml_urls: Vec<String>,
    /// Доверенные ключи (PEM).
    pub public_keys: Vec<String>,
    /// ОС канала: проверяется установщик именно её.
    pub platform: Platform,
    pub http_get: Arc<dyn HttpGet>,
    pub logger: Arc<dyn Logger>,
    /// `signal`: отмена запросов канала.
    pub cancel: Option<CancelToken>,
}

impl GateOptions {
    /// Боевые адреса и ключи для ОС сборки; журнал молчит.
    pub fn new(current_version: impl Into<String>, http_get: Arc<dyn HttpGet>) -> GateOptions {
        let endpoints = channel::launcher();
        GateOptions {
            current_version: current_version.into(),
            channel_urls: endpoints.channel_urls.to_vec(),
            latest_yml_urls: endpoints.latest_yml_urls.to_vec(),
            public_keys: channel::default_public_keys(),
            platform: endpoints.platform,
            http_get,
            logger: Arc::new(NullLogger),
            cancel: None,
        }
    }

    /// Боевые адреса и ключи для заданной ОС.
    pub fn for_platform(mut self, platform: Platform) -> GateOptions {
        let endpoints = channel::launcher_for(platform);
        self.channel_urls = endpoints.channel_urls.to_vec();
        self.latest_yml_urls = endpoints.latest_yml_urls.to_vec();
        self.platform = platform;
        self
    }
}

/// Параметры [`LauncherUpdateGate::verify_installer`].
#[derive(Debug, Clone, Default)]
pub struct VerifyInstallerOptions {
    /// Метаданные вместо последних проверенных (`opts.manifest`).
    pub manifest: Option<Value>,
    /// Версия, которую обещала лента: если метаданные описывают другую — отказ.
    pub expected_version: Option<String>,
    pub verify: VerifyOptions,
}

/// `LauncherUpdateGate`.
pub struct LauncherUpdateGate {
    current_version: String,
    channel_urls: Vec<String>,
    latest_yml_urls: Vec<String>,
    public_keys: Vec<String>,
    platform: Platform,
    http_get: Arc<dyn HttpGet>,
    logger: Arc<dyn Logger>,
    cancel: Option<CancelToken>,
    last_result: Mutex<Option<Outcome>>,
}

impl LauncherUpdateGate {
    pub fn new(options: GateOptions) -> LauncherUpdateGate {
        LauncherUpdateGate {
            current_version: options.current_version,
            channel_urls: options.channel_urls,
            latest_yml_urls: options.latest_yml_urls,
            public_keys: options.public_keys,
            platform: options.platform,
            http_get: options.http_get,
            logger: options.logger,
            cancel: options.cancel,
            last_result: Mutex::new(None),
        }
    }

    /// Последний принятый (или, если принятых не было, первый отклонённый) итог `evaluate`.
    pub fn last_result(&self) -> Option<Outcome> {
        self.last_result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn remember(&self, outcome: &Outcome) {
        *self
            .last_result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(outcome.clone());
    }

    fn get_options(&self) -> GetOptions {
        GetOptions {
            cancel: self.cancel.clone(),
            ..GetOptions::default()
        }
    }

    /// `fetchChannelFrom(url)`: тело ответа как JSON (`Buffer.toString("utf8")` + `JSON.parse`).
    pub async fn fetch_channel_from(&self, url: &str) -> Result<Value, GateError> {
        let body = self.http_get.get(url, &self.get_options()).await?;
        json_parse(&String::from_utf8_lossy(&body))
            .map_err(|error| GateError::NotJson(error.to_string()))
    }

    /// `fetchChannel()`: первый канал, который ответил JSON; ошибка — последняя.
    pub async fn fetch_channel(&self) -> Result<FetchedChannel, GateError> {
        let mut last_error = None;
        for url in &self.channel_urls {
            match self.fetch_channel_from(url).await {
                Ok(manifest) => {
                    return Ok(FetchedChannel {
                        manifest,
                        source_url: url.clone(),
                    });
                }
                Err(error) => {
                    self.logger.warn(&format!(
                        "[launcher-update] канал недоступен ({url}): {error}"
                    ));
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or(GateError::Unavailable))
    }

    /// `probeLatest()`: справка по основному каналу без записи итога; любой сбой — `None`.
    pub async fn probe_latest(&self) -> Option<Outcome> {
        let url = self.channel_urls.first()?;
        let manifest = match self.fetch_channel_from(url).await {
            Ok(manifest) => manifest,
            Err(error) => {
                self.logger.warn(&format!(
                    "[launcher-update] канал не ответил на справку ({url}): {error}"
                ));
                return None;
            }
        };
        match self.evaluate_manifest(&manifest, url, None).await {
            Ok(outcome) => Some(outcome),
            Err(error) => {
                self.logger.warn(&format!(
                    "[launcher-update] справка по каналу не удалась: {error}"
                ));
                None
            }
        }
    }

    /// `evaluate(expectedVersion)`: каналы по порядку; первый принятый возвращается сразу, иначе
    /// первый отклонённый. Если ни один канал не ответил — последняя ошибка.
    pub async fn evaluate(&self, expected_version: Option<&str>) -> Result<Outcome, GateError> {
        let mut first_outcome: Option<Outcome> = None;
        let mut last_error = None;

        for url in &self.channel_urls {
            let manifest = match self.fetch_channel_from(url).await {
                Ok(manifest) => manifest,
                Err(error) => {
                    self.logger.warn(&format!(
                        "[launcher-update] канал недоступен ({url}): {error}"
                    ));
                    last_error = Some(error);
                    continue;
                }
            };

            let outcome = self
                .evaluate_manifest(&manifest, url, expected_version)
                .await?;

            if outcome.ok {
                if first_outcome.is_some() {
                    self.logger.info(&format!(
                        "[launcher-update] основной канал не подошёл, взят резервный: {url}"
                    ));
                }
                self.remember(&outcome);
                return Ok(outcome);
            }
            if first_outcome.is_none() {
                first_outcome = Some(outcome);
            }
        }

        if let Some(outcome) = first_outcome {
            self.remember(&outcome);
            return Ok(outcome);
        }
        Err(last_error.unwrap_or(GateError::Unavailable))
    }

    /// `evaluateManifest(manifest, sourceUrl, expectedVersion)`.
    pub async fn evaluate_manifest(
        &self,
        manifest: &Value,
        source_url: &str,
        expected_version: Option<&str>,
    ) -> Result<Outcome, GateError> {
        let result = channel::check(
            manifest,
            &CheckOptions {
                current_version: Some(&self.current_version),
                public_keys: Some(&self.public_keys),
                platform: Some(self.platform),
            },
        )?;

        let mut outcome = Outcome {
            ok: result.ok,
            problems: result.problems.clone(),
            update_available: result.update_available,
            downgrade: result.downgrade,
            unsupported: result.unsupported,
            code: result.code,
            // Обязательно только обновление на версию строго новее: подписанный канал той же
            // или старой версии (повтор с зеркала) не должен блокировать запуск (0.4.9, A-8).
            update_required: result.ok
                && result.update_available
                && manifest.get("updateRequired") == Some(&Value::Bool(true)),
            manifest: result.ok.then(|| manifest.clone()),
            source_url: source_url.to_owned(),
        };
        let version = js::to_template_string(manifest.get("version"));

        if let Some(expected) = expected_version.filter(|expected| !expected.is_empty())
            && result.ok
            && version != expected
        {
            outcome.ok = false;
            outcome.problems.push(format!(
                "feed предлагает {expected}, подписанные метаданные — {version}"
            ));
        }

        if result.ok && result.downgrade {
            outcome.ok = false;
            outcome.problems.push(format!(
                "версия {version} старше установленной {} (downgrade отклонён)",
                self.current_version
            ));
        }

        let yml_sha = manifest
            .get("latestYmlSha256")
            .filter(|value| js::truthy(value))
            .map(js::to_string);
        if outcome.ok
            && let Some(expected_sha) = yml_sha
        {
            let mut yml_ok = false;
            let mut last_error = "недоступен".to_owned();

            for url in &self.latest_yml_urls {
                match self.http_get.get(url, &GetOptions::default()).await {
                    Ok(yml) => {
                        let sha = Sha256::digest(&yml)
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<String>();
                        if sha == expected_sha {
                            yml_ok = true;
                            break;
                        }
                        last_error = format!("SHA-256 latest.yml не совпал ({url})");
                    }
                    Err(error) => last_error = error.to_string(),
                }
            }

            if !yml_ok {
                outcome.ok = false;
                outcome
                    .problems
                    .push(format!("latest.yml не подтверждён: {last_error}"));
            }
        }

        Ok(outcome)
    }

    /// `verifyInstaller(filePath, options)`: скачанный файл против проверенных метаданных.
    pub async fn verify_installer(
        &self,
        path: &Path,
        options: &VerifyInstallerOptions,
    ) -> VerifyOutcome {
        let source = options
            .manifest
            .clone()
            .or_else(|| self.last_result().and_then(|outcome| outcome.manifest));
        let Some(source) = source else {
            return VerifyOutcome::failed(LuCode::NoMetadata, "нет проверенных метаданных");
        };

        if let Some(expected) = options
            .expected_version
            .as_deref()
            .filter(|e| !e.is_empty())
            && source.get("version").and_then(Value::as_str) != Some(expected)
        {
            return VerifyOutcome::failed(
                LuCode::VersionMismatch,
                format!(
                    "проверенные метаданные описывают {}, а скачана {expected}",
                    js::to_template_string(source.get("version"))
                ),
            );
        }

        let installer = source
            .get("installer")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        channel::verify_installer_stream(path, &installer, &options.verify).await
    }
}
