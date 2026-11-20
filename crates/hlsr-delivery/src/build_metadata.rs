//! Метаданные сборки (`shared/build-metadata.js`): файл `ghostingmod/hlsr-build.json`, который
//! кладёт в сборку Ghosting сборщик. Разбор строгий: версия схемы 1, ровно десять полей, значения
//! по образцам (`gameId`, `buildName`, `architecture`, `requiredSteamAppId` — как у продукта),
//! дата сборки — UTC ISO-8601 не раньше 2020 года и не позже «сейчас» + 48 часов.
//!
//! Разбор — `JSON.parse` 0.4.7: текст его `SyntaxError` у V8 и у serde_json разный (после «не являются
//! JSON: »), код ошибки и префикс те же.

use crate::js_value as jv;
use crate::timing::iso_from_ms;
use hlsr_core::js;
use hlsr_core::js::text::utf16_len;
use hlsr_core::json_text::{js_key_order, json_parse};
use regex::Regex;
use serde_json::{Map, Value, json};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

pub const SCHEMA_VERSION: u64 = 1;

/// Файл метаданных больше 64 КиБ не читается.
pub const MAX_BYTES: u64 = 64 * 1024;

/// Путь файла в установке, через `/`.
pub const RELATIVE_PATH: &str = "ghostingmod/hlsr-build.json";

pub const REQUIRED_FIELDS: [&str; 10] = [
    "schemaVersion",
    "gameId",
    "version",
    "buildId",
    "buildName",
    "gitCommit",
    "builtAt",
    "architecture",
    "requiredSteamAppId",
    "minimumLauncherVersion",
];

const BUILD_ID_MAX: usize = 128;

/// `Date.UTC(2020, 0, 1)`.
const DATE_FLOOR_MS: i64 = 1_577_836_800_000;
const FUTURE_SKEW_MS: f64 = 48.0 * 60.0 * 60.0 * 1000.0;

/// `/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/` (`\d` JS — только `0-9`).
static SEMVER_FULL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$").unwrap());
/// `/^\d+\.\d+\.\d+$/`.
static SEMVER_STRICT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+$").unwrap());
/// `/^[a-z0-9]+([._-][a-z0-9]+)*$/`.
static BUILD_ID_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9]+([._-][a-z0-9]+)*$").unwrap());
/// `/^[0-9a-f]{7,40}$/`.
static GIT_COMMIT_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-f]{7,40}$").unwrap());
/// `/^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(\.\d{1,3})?Z$/`.
static BUILT_AT_SHAPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]{1,3})?Z$").unwrap()
});

/// Код ошибки метаданных (`CODES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataCode {
    /// `BUILD_METADATA_INVALID`.
    Invalid,
    /// `BUILD_METADATA_UNSUPPORTED`: версия схемы не та.
    Unsupported,
}

impl MetadataCode {
    pub fn as_str(self) -> &'static str {
        match self {
            MetadataCode::Invalid => "BUILD_METADATA_INVALID",
            MetadataCode::Unsupported => "BUILD_METADATA_UNSUPPORTED",
        }
    }
}

/// `BuildMetadataError`: код, текст для игрока и подробности.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct BuildMetadataError {
    pub code: MetadataCode,
    pub message: String,
    pub detail: Option<Value>,
}

impl BuildMetadataError {
    /// `new BuildMetadataError(code, message, detail)`: пустое сообщение — имя кода.
    pub fn new(code: MetadataCode, message: impl Into<String>, detail: Option<Value>) -> Self {
        let message = message.into();
        BuildMetadataError {
            code,
            message: if message.is_empty() {
                code.as_str().to_owned()
            } else {
                message
            },
            detail: detail.filter(js::truthy),
        }
    }

    /// `toPublic()`: `{code, message, detail}`.
    pub fn to_public(&self) -> Value {
        json!({
            "code": self.code.as_str(),
            "message": self.message,
            "detail": self.detail.clone().unwrap_or(Value::Null),
        })
    }
}

/// Что должно быть в метаданных (`PRODUCT_PROFILE` или своё `expected`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub game_id: String,
    pub build_name: String,
    pub architecture: String,
    pub required_steam_app_id: i64,
}

impl Profile {
    /// `PRODUCT_PROFILE`: Ghosting Renewed, сборка «HL1 Movement», x86, Steam-приложение Half-Life.
    pub fn product() -> Profile {
        Profile {
            game_id: "ghosting-renewed".to_owned(),
            build_name: "HL1 Movement".to_owned(),
            architecture: "x86".to_owned(),
            required_steam_app_id: 243_730,
        }
    }
}

/// Проверенные метаданные (то, что возвращает `validate`); `schemaVersion` всегда 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildMetadata {
    pub game_id: String,
    pub version: String,
    pub build_id: String,
    pub build_name: String,
    pub git_commit: String,
    /// Дата в записи `toISOString()`: `2026-10-14T09:30:15.250Z`.
    pub built_at: String,
    pub architecture: String,
    pub required_steam_app_id: i64,
    pub minimum_launcher_version: String,
}

impl BuildMetadata {
    /// Объект с ключами в порядке `REQUIRED_FIELDS`, как у 0.4.7 (в `installed.json` порядок виден).
    pub fn to_value(&self) -> Value {
        json!({
            "schemaVersion": SCHEMA_VERSION,
            "gameId": self.game_id,
            "version": self.version,
            "buildId": self.build_id,
            "buildName": self.build_name,
            "gitCommit": self.git_commit,
            "builtAt": self.built_at,
            "architecture": self.architecture,
            "requiredSteamAppId": self.required_steam_app_id,
            "minimumLauncherVersion": self.minimum_launcher_version,
        })
    }
}

/// Параметры `validate` и `parse` (`options`).
#[derive(Debug, Clone, Copy, Default)]
pub struct ValidateOptions<'a> {
    /// `expected`: по умолчанию — [`Profile::product`].
    pub expected: Option<&'a Profile>,
    /// `now`: «сейчас» в миллисекундах; по умолчанию — часы ОС.
    pub now: Option<f64>,
    /// `source`: откуда метаданные (в подробностях ошибки).
    pub source: Option<&'a str>,
}

/// `Date.now()`.
pub(crate) fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_millis() as f64)
}

fn invalid(message: impl Into<String>, detail: Value) -> BuildMetadataError {
    BuildMetadataError::new(MetadataCode::Invalid, message, Some(detail))
}

/// Год, месяц, день, час, минута, секунда UTC для момента `ms` (в том числе до 1970 года).
fn utc_parts(ms: i64) -> (i64, i64, i64, i64, i64, i64) {
    let days = ms.div_euclid(86_400_000);
    let rest = ms.rem_euclid(86_400_000);
    let (hour, minute, second) = (
        rest / 3_600_000,
        rest % 3_600_000 / 60_000,
        rest % 60_000 / 1000,
    );
    // Пролептический григорианский календарь (civil_from_days Говарда Хиннанта).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day, hour, minute, second)
}

/// `normalizeBuiltAt(value, nowMs)`: строка UTC ISO-8601 с настоящей датой не раньше 2020 года
/// и не позже «сейчас» + 48 часов → `toISOString()` (с миллисекундами).
pub fn normalize_built_at(value: &Value, now: Option<f64>) -> Result<String, BuildMetadataError> {
    let field = || json!({"field": "builtAt"});
    let Some(text) = value.as_str() else {
        return Err(invalid("builtAt должен быть строкой ISO-8601", field()));
    };
    if !BUILT_AT_SHAPE.is_match(text) {
        return Err(invalid(
            format!("builtAt не является UTC ISO-8601: {text}"),
            field(),
        ));
    }
    let Some(ms) = js::date_parse(text) else {
        return Err(invalid(
            format!("builtAt не разбирается как дата: {text}"),
            field(),
        ));
    };

    // 31 февраля `Date.parse` принимает и переносит на март: настоящая ли это дата, видно по обратной записи.
    let (year, month, day, hour, minute, second) = utc_parts(ms);
    let rebuilt = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z");
    // `value.replace(/\.\d{1,3}Z$/, "Z")`: форма проверена, точка в ней одна.
    let without_millis = match text.find('.') {
        Some(dot) => format!("{}Z", &text[..dot]),
        None => text.to_owned(),
    };
    if rebuilt != without_millis {
        return Err(invalid(
            format!("builtAt содержит несуществующую дату: {text}"),
            field(),
        ));
    }

    if ms < DATE_FLOOR_MS {
        return Err(invalid(
            format!("builtAt слишком старая, дата сборки не может быть раньше 2020: {text}"),
            field(),
        ));
    }

    let ceiling = now.unwrap_or_else(now_ms) + FUTURE_SKEW_MS;
    if ms as f64 > ceiling {
        return Err(invalid(
            format!("builtAt в будущем — дата сборки недостоверна: {text}"),
            field(),
        ));
    }

    Ok(iso_from_ms(ms))
}

/// `validate(data, options)`: проверка разобранного документа.
pub fn validate(
    data: &Value,
    options: &ValidateOptions,
) -> Result<BuildMetadata, BuildMetadataError> {
    let product;
    let expected = match options.expected {
        Some(expected) => expected,
        None => {
            product = Profile::product();
            &product
        }
    };
    // `options.source || null`: пустая строка — тоже `null`.
    let source = options
        .source
        .filter(|source| !source.is_empty())
        .map_or(Value::Null, |source| json!(source));
    // `data.<поле>`: у объекта с этими полями значение есть (отсутствие проверено раньше).
    let Some(object) = data.as_object() else {
        return Err(invalid(
            "Метаданные сборки должны быть JSON-объектом",
            json!({"source": source}),
        ));
    };
    let field = |name: &str| -> &Value { object.get(name).unwrap_or(&Value::Null) };
    let shown = |name: &str| js::to_template_string(object.get(name));

    let schema_ok = object
        .get("schemaVersion")
        .and_then(jv::number)
        .is_some_and(|n| n == SCHEMA_VERSION as f64);
    if !schema_ok {
        let mut detail = Map::new();
        detail.insert("expected".to_owned(), json!(SCHEMA_VERSION));
        if let Some(actual) = object.get("schemaVersion") {
            detail.insert("actual".to_owned(), actual.clone());
        }
        detail.insert("source".to_owned(), source);
        return Err(BuildMetadataError::new(
            MetadataCode::Unsupported,
            format!(
                "Неподдерживаемая версия схемы метаданных: {}",
                shown("schemaVersion")
            ),
            Some(Value::Object(detail)),
        ));
    }

    for name in REQUIRED_FIELDS {
        if !object.contains_key(name) {
            return Err(invalid(
                format!("В метаданных сборки нет поля {name}"),
                json!({"field": name, "source": source}),
            ));
        }
    }

    for (key, _) in js_key_order(object) {
        if !REQUIRED_FIELDS.contains(&key.as_str()) {
            return Err(invalid(
                format!("Неизвестное поле в метаданных сборки: {key}"),
                json!({"field": key, "source": source}),
            ));
        }
    }

    let is = |name: &str, wanted: &str| field(name).as_str() == Some(wanted);

    if !is("gameId", &expected.game_id) {
        return Err(invalid(
            format!(
                "Метаданные описывают игру {}, ожидалась {}",
                shown("gameId"),
                expected.game_id
            ),
            json!({"field": "gameId", "expected": expected.game_id, "actual": field("gameId"), "source": source}),
        ));
    }

    let version_ok = field("version")
        .as_str()
        .is_some_and(|version| SEMVER_FULL.is_match(version) && utf16_len(version) <= 64);
    if !version_ok {
        return Err(invalid(
            format!("Некорректная версия сборки: {}", shown("version")),
            json!({"field": "version", "source": source}),
        ));
    }

    let build_id_ok = field("buildId").as_str().is_some_and(|build_id| {
        (3..=BUILD_ID_MAX).contains(&utf16_len(build_id)) && BUILD_ID_PATTERN.is_match(build_id)
    });
    if !build_id_ok {
        return Err(invalid(
            format!("Недопустимый buildId: {}", shown("buildId")),
            json!({"field": "buildId", "source": source}),
        ));
    }

    if !is("buildName", &expected.build_name) {
        return Err(invalid(
            format!(
                "Метаданные объявляют сборку \"{}\", ожидалась \"{}\"",
                shown("buildName"),
                expected.build_name
            ),
            json!({"field": "buildName", "expected": expected.build_name, "actual": field("buildName"), "source": source}),
        ));
    }

    if !field("gitCommit")
        .as_str()
        .is_some_and(|commit| GIT_COMMIT_PATTERN.is_match(commit))
    {
        return Err(invalid(
            format!("Некорректный gitCommit: {}", shown("gitCommit")),
            json!({"field": "gitCommit", "source": source}),
        ));
    }

    let built_at = normalize_built_at(field("builtAt"), options.now)?;

    if !is("architecture", &expected.architecture) {
        return Err(invalid(
            format!(
                "Архитектура {} не поддерживается, ожидалась {}",
                shown("architecture"),
                expected.architecture
            ),
            json!({"field": "architecture", "expected": expected.architecture, "actual": field("architecture"), "source": source}),
        ));
    }

    let steam_app_id = jv::number(field("requiredSteamAppId")).filter(|n| {
        n.is_finite() && n.fract() == 0.0 && *n == expected.required_steam_app_id as f64
    });
    if steam_app_id.is_none() {
        return Err(invalid(
            format!(
                "requiredSteamAppId {} не совпадает с ожидаемым {}",
                shown("requiredSteamAppId"),
                expected.required_steam_app_id
            ),
            json!({"field": "requiredSteamAppId", "expected": expected.required_steam_app_id, "actual": field("requiredSteamAppId"), "source": source}),
        ));
    }

    if !field("minimumLauncherVersion")
        .as_str()
        .is_some_and(|version| SEMVER_STRICT.is_match(version))
    {
        return Err(invalid(
            format!(
                "Некорректная minimumLauncherVersion: {}",
                shown("minimumLauncherVersion")
            ),
            json!({"field": "minimumLauncherVersion", "source": source}),
        ));
    }

    let text = |name: &str| field(name).as_str().unwrap_or_default().to_owned();
    Ok(BuildMetadata {
        game_id: text("gameId"),
        version: text("version"),
        build_id: text("buildId"),
        build_name: text("buildName"),
        git_commit: text("gitCommit"),
        built_at,
        architecture: text("architecture"),
        required_steam_app_id: expected.required_steam_app_id,
        minimum_launcher_version: text("minimumLauncherVersion"),
    })
}

/// `parse(raw, options)`: `JSON.parse` и `validate`.
pub fn parse(raw: &str, options: &ValidateOptions) -> Result<BuildMetadata, BuildMetadataError> {
    match json_parse(raw) {
        Ok(data) => validate(&data, options),
        Err(error) => Err(invalid(
            format!("Метаданные сборки не являются JSON: {error}"),
            json!({"source": options.source.filter(|source| !source.is_empty()).map_or(Value::Null, |source| json!(source))}),
        )),
    }
}
