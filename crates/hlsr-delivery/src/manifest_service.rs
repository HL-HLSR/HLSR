//! Манифест игры (`ManifestService.js`): чтение и проверка по схеме, список разрешённых
//! адресов `https-direct`, хеш манифеста, может ли сборка ставиться, порядок источников.

use crate::delivery_endpoints::is_allowed_download_url;
use crate::errors::{Code, DeliveryError, RawError};
use crate::schema_validator::{describe_errors, game_manifest_validator};
use crate::semver;
use hlsr_core::js;
use hlsr_core::json_text::{json_parse, stringify};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Единственная поддерживаемая версия схемы манифеста.
pub const SUPPORTED_SCHEMA_VERSION: i64 = 1;

/// Итог `validate`/`parse`/`loadFromFile`: `{manifest, sha256, source}`.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedManifest {
    pub manifest: Value,
    pub sha256: String,
    pub source: Option<String>,
}

/// Параметры `checkInstallable`: `launcherVersion` пустой — версия сервиса;
/// `allowedProviderTypes` — типы источников, которые сейчас разрешены.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InstallableOptions {
    pub launcher_version: Option<String>,
    pub allowed_provider_types: Vec<String>,
}

/// Причина отказа: `{code}` или `{code, details}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Reason {
    pub code: Code,
    pub details: Option<Value>,
}

impl Reason {
    /// Объект причины как у 0.4.7 (`details` только у `LAUNCHER_TOO_OLD`).
    pub fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("code".to_owned(), json!(self.code.as_str()));
        if let Some(details) = &self.details {
            object.insert("details".to_owned(), details.clone());
        }
        Value::Object(object)
    }
}

/// Итог `checkInstallable`: `{ok, reasons, providers}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Installable {
    pub ok: bool,
    pub reasons: Vec<Reason>,
    /// Источники базового архива с разрешёнными типами.
    pub providers: Vec<Value>,
}

/// `ManifestService`: версия лаунчера для проверки `minimumLauncherVersion`.
#[derive(Debug, Clone)]
pub struct ManifestService {
    launcher_version: String,
}

impl Default for ManifestService {
    fn default() -> Self {
        ManifestService::new(None)
    }
}

impl ManifestService {
    /// `new ManifestService({ launcherVersion })`: без версии — `0.0.0`.
    pub fn new(launcher_version: Option<&str>) -> ManifestService {
        ManifestService {
            launcher_version: launcher_version
                .filter(|version| !version.is_empty())
                .unwrap_or("0.0.0")
                .to_owned(),
        }
    }

    /// `loadFromFile(filePath)`: `utf8`, как `fs.readFileSync(…, "utf8")`.
    pub fn load_from_file(&self, file_path: &Path) -> Result<LoadedManifest, DeliveryError> {
        let raw = std::fs::read(file_path).map_err(|_| {
            DeliveryError::new(
                Code::ManifestNotFound,
                format!("Манифест не найден: {}", file_path.display()),
            )
        })?;
        self.parse(&String::from_utf8_lossy(&raw), file_path.to_str())
    }

    /// `parse(raw, source)`. Текст ошибки разбора — serde_json, не V8 (код тот же).
    pub fn parse(&self, raw: &str, source: Option<&str>) -> Result<LoadedManifest, DeliveryError> {
        let source = source.filter(|source| !source.is_empty());
        // `JSON.parse` не срезает BOM, в отличие от чтения файлов хранилищ.
        let data: Value = json_parse(raw).map_err(|error| {
            DeliveryError::new(
                Code::ManifestInvalid,
                format!("Манифест не является JSON: {error}"),
            )
            .with_details(json!({"source": source}))
        })?;
        self.validate(&data, source)
    }

    /// `validate(data, source)`.
    pub fn validate(
        &self,
        data: &Value,
        source: Option<&str>,
    ) -> Result<LoadedManifest, DeliveryError> {
        // `source || null`: пустая строка — как отсутствие.
        let source = source.filter(|source| !source.is_empty());
        let Some(object) = data.as_object() else {
            return Err(DeliveryError::new(
                Code::ManifestInvalid,
                "Манифест должен быть JSON-объектом",
            ));
        };

        let version_ok = object
            .get("schemaVersion")
            .and_then(Value::as_number)
            .is_some_and(|n| n.as_f64() == Some(SUPPORTED_SCHEMA_VERSION as f64));
        if !version_ok {
            return Err(DeliveryError::new(
                Code::ManifestInvalid,
                format!(
                    "Неподдерживаемая версия схемы манифеста: {}",
                    js::to_template_string(object.get("schemaVersion"))
                ),
            )
            .with_details(json!({"expected": SUPPORTED_SCHEMA_VERSION})));
        }

        let schema_errors = game_manifest_validator().validate(data);
        if !schema_errors.is_empty() {
            let errors = describe_errors(&schema_errors);
            return Err(DeliveryError::new(
                Code::ManifestInvalid,
                format!("Манифест не прошёл валидацию: {}", errors[0]),
            )
            .with_details(json!({
                "source": source,
                "errors": errors.iter().take(20).collect::<Vec<_>>(),
            })));
        }

        let mut bad_urls: Vec<String> = Vec::new();
        let mut inspect = |providers: Option<&Value>| {
            for provider in providers.and_then(Value::as_array).into_iter().flatten() {
                let Some(url) = provider.get("url").filter(|url| js::truthy(url)) else {
                    continue;
                };
                if provider.get("type").and_then(Value::as_str) != Some("https-direct") {
                    continue;
                }
                let url = url.as_str().unwrap_or_default();
                if !is_allowed_download_url(url) {
                    bad_urls.push(url.to_owned());
                }
            }
        };
        inspect(data.pointer("/baseArchive/providers"));
        inspect(data.pointer("/filesManifest/providers"));
        for patch in object
            .get("patches")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            inspect(patch.pointer("/archive/providers"));
            inspect(patch.get("providers"));
        }
        if let Some(first) = bad_urls.first() {
            return Err(DeliveryError::new(
                Code::ManifestInvalid,
                format!("Источник вне allowlist: {first}"),
            )
            .with_details(json!({
                "source": source,
                "hosts": bad_urls.iter().take(5).collect::<Vec<_>>(),
            })));
        }

        Ok(LoadedManifest {
            manifest: data.clone(),
            sha256: ManifestService::hash(data),
            source: source.map(str::to_owned),
        })
    }

    /// `ManifestService.hash(manifest)`: SHA-256 от `JSON.stringify(manifest)` — порядок
    /// ключей исходный, числа как в JS.
    pub fn hash(manifest: &Value) -> String {
        crate::file_ops::hex(&Sha256::digest(stringify(manifest, "").as_bytes()))
    }

    /// `checkInstallable(manifest, options)`. Ошибка — `TypeError` JS: у манифеста нет
    /// `baseArchive.providers` нужного вида.
    pub fn check_installable(
        &self,
        manifest: &Value,
        options: Option<&InstallableOptions>,
    ) -> Result<Installable, RawError> {
        if manifest.is_null() {
            return Err(crate::js_value::read_error(Some(manifest), "enabled"));
        }
        let no_options = InstallableOptions::default();
        let options = options.unwrap_or(&no_options);
        let get = |key: &str| manifest.get(key);
        let mut reasons = Vec::new();

        if !get("enabled").is_some_and(js::truthy) {
            reasons.push(Reason {
                code: Code::ManifestDisabled,
                details: None,
            });
        }
        if !get("version").is_some_and(js::truthy)
            || get("versionStatus").and_then(Value::as_str) == Some("draft")
        {
            reasons.push(Reason {
                code: Code::ManifestVersionDraft,
                details: None,
            });
        }
        if !get("expectedInstalledSize").is_some_and(js::truthy) {
            reasons.push(Reason {
                code: Code::ManifestInvalid,
                details: None,
            });
        }

        let launcher_version = options
            .launcher_version
            .as_deref()
            .filter(|version| !version.is_empty())
            .unwrap_or(&self.launcher_version);
        let minimum = get("minimumLauncherVersion");
        // `semver.parse(String(version || ""))`.
        let minimum_text = minimum
            .filter(|value| js::truthy(value))
            .map(js::to_string)
            .unwrap_or_default();
        if semver::lt(launcher_version, &minimum_text) {
            let mut details = Map::new();
            if let Some(required) = minimum {
                details.insert("required".to_owned(), required.clone());
            }
            details.insert("current".to_owned(), json!(launcher_version));
            reasons.push(Reason {
                code: Code::LauncherTooOld,
                details: Some(Value::Object(details)),
            });
        }

        let providers = match manifest.get("baseArchive") {
            None => {
                return Err(type_error(
                    "Cannot read properties of undefined (reading 'providers')",
                ));
            }
            Some(Value::Null) => {
                return Err(type_error(
                    "Cannot read properties of null (reading 'providers')",
                ));
            }
            Some(archive) => match archive.get("providers") {
                None => {
                    return Err(type_error(
                        "Cannot read properties of undefined (reading 'filter')",
                    ));
                }
                Some(Value::Null) => {
                    return Err(type_error(
                        "Cannot read properties of null (reading 'filter')",
                    ));
                }
                Some(Value::Array(list)) => list,
                Some(_) => {
                    return Err(type_error(
                        "manifest.baseArchive.providers.filter is not a function",
                    ));
                }
            },
        };
        let mut usable = Vec::new();
        for provider in providers {
            let kind = match provider {
                Value::Null => {
                    return Err(type_error(
                        "Cannot read properties of null (reading 'type')",
                    ));
                }
                other => other.get("type").and_then(Value::as_str),
            };
            if kind.is_some_and(|kind| options.allowed_provider_types.iter().any(|t| t == kind)) {
                usable.push(provider.clone());
            }
        }
        if usable.is_empty() {
            reasons.push(Reason {
                code: Code::RemoteDisabled,
                details: None,
            });
        }

        Ok(Installable {
            ok: reasons.is_empty(),
            reasons,
            providers: usable,
        })
    }

    /// `assertInstallable(manifest, options)`: первая причина отказа — ошибка.
    pub fn assert_installable(
        &self,
        manifest: &Value,
        options: Option<&InstallableOptions>,
    ) -> Result<Installable, crate::errors::Failure> {
        let check = self.check_installable(manifest, options)?;
        let Some(first) = check.reasons.first() else {
            return Ok(check);
        };
        Err(DeliveryError::new(
            first.code,
            format!("Установка сборки недоступна: {}", first.code),
        )
        .with_details(json!({
            "reasons": check.reasons.iter().map(Reason::to_json).collect::<Vec<_>>(),
        }))
        .into())
    }

    /// `ManifestService.orderProviders(providers)`: по `priority` (нет — 50), порядок равных сохраняется.
    pub fn order_providers(providers: &[Value]) -> Vec<Value> {
        let priority = |provider: &Value| -> f64 {
            match provider.get("priority") {
                None | Some(Value::Null) => 50.0,
                Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::INFINITY),
                Some(_) => f64::NAN,
            }
        };
        let mut ordered = providers.to_vec();
        ordered.sort_by(|a, b| {
            (priority(a) - priority(b))
                .partial_cmp(&0.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        ordered
    }
}

fn type_error(message: &str) -> RawError {
    RawError::new(message)
}
