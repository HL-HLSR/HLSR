//! Сборки без нашего подписанного канала (`origin-updates.js`, HL1 с bxt.rs). Ставится только
//! архив, чей SHA-256 закреплён во встроенном манифесте. Лаунчер сверяет Last-Modified и ETag
//! архива в источнике с отпечатком закреплённой сборки: если там уже другая, обновление не
//! предлагается, в журнал пишется предупреждение — новая сборка ждёт выпуска HLSR с её SHA-256.
//!
//! `target` и `probe` принимают манифест игры как `Option<&Value>`: `None` — `null`, который
//! отдаёт `localManifestOf`, когда манифеста нет. Тексты `TypeError` V8 повторены дословно: по
//! ним ошибка уходит в `CHANNEL_UNAVAILABLE` проверки обновлений.

use crate::errors::{Code, DeliveryError, Failure, RawError};
use crate::http_client::{HttpClient, Method, RequestOptions};
use crate::js_value::{self as jv, prop, truthy};
use crate::ports::{Logger, TracingLogger};
use hlsr_core::js;
use hlsr_core::json_text::js_number;
use serde_json::{Map, Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// HEAD источника ждёт не дольше 10 с.
const PROBE_TIMEOUT_MS: u64 = 10_000;

/// Отпечаток архива, из которого собран встроенный манифест.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Baseline {
    pub version: String,
    pub last_modified: String,
    pub etag: String,
    pub size: u64,
}

impl Baseline {
    /// Как объект `baseline` 0.4.7.
    fn to_json(&self) -> Value {
        json!({
            "version": self.version,
            "lastModified": self.last_modified,
            "etag": self.etag,
            "size": self.size,
        })
    }
}

/// Источник игры: id источника в манифесте и отпечаток встроенного архива.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub provider_id: String,
    pub baseline: Baseline,
}

/// `ORIGINS`: игры, которые обновляются из источника.
pub fn default_origins() -> Vec<(String, Origin)> {
    vec![(
        "hl1-won".to_owned(),
        Origin {
            provider_id: "bxt-rs".to_owned(),
            baseline: Baseline {
                version: "1.0.0".to_owned(),
                last_modified: "Mon, 17 Jul 2023 11:23:57 GMT".to_owned(),
                etag: "\"64b524cd-9aee5c0\"".to_owned(),
                size: 162_457_024,
            },
        },
    )]
}

/// `a === b` для значений, прочитанных из JSON: числа, строки, `null` и логические
/// сравниваются по значению, отсутствующее (`undefined`) равно только отсутствующему,
/// объекты и массивы никогда (это разные объекты).
fn strict_equals(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Value::Null), Some(Value::Null)) => true,
        (Some(Value::Bool(a)), Some(Value::Bool(b))) => a == b,
        (Some(Value::String(a)), Some(Value::String(b))) => a == b,
        (Some(a @ Value::Number(_)), Some(b @ Value::Number(_))) => jv::number(a) == jv::number(b),
        _ => false,
    }
}

/// `sameStamp(a, b)`: одинаковый размер и либо равные ETag (если они есть у обоих), либо равные
/// Last-Modified.
pub fn same_stamp(a: &Value, b: &Value) -> bool {
    if !js::truthy(a) || !js::truthy(b) {
        return false;
    }
    if !strict_equals(prop(a, "size"), prop(b, "size")) {
        return false;
    }
    let (a_etag, b_etag) = (prop(a, "etag"), prop(b, "etag"));
    if truthy(a_etag) && truthy(b_etag) {
        return strict_equals(a_etag, b_etag);
    }
    truthy(prop(a, "lastModified"))
        && strict_equals(prop(a, "lastModified"), prop(b, "lastModified"))
}

/// Дата и время UTC из миллисекунд (алгоритм «гражданской даты» по числу дней).
struct Civil {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    millisecond: i64,
}

fn civil(ms: i64) -> Civil {
    let days = ms.div_euclid(86_400_000);
    let in_day = ms.rem_euclid(86_400_000);
    // Дни от 1970-01-01 → год, месяц, день (proleptic Gregorian).
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    Civil {
        year,
        month,
        day,
        hour: in_day / 3_600_000,
        minute: in_day / 60_000 % 60,
        second: in_day / 1000 % 60,
        millisecond: in_day % 1000,
    }
}

/// `new Date(ms).toISOString()`; год вне 0000–9999 — со знаком и шестью цифрами.
fn to_iso_string(ms: i64) -> String {
    let c = civil(ms);
    let year = if (0..=9999).contains(&c.year) {
        format!("{:04}", c.year)
    } else {
        format!("{}{:06}", if c.year < 0 { '-' } else { '+' }, c.year.abs())
    };
    format!(
        "{year}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        c.month, c.day, c.hour, c.minute, c.second, c.millisecond
    )
}

fn type_error(message: &str) -> Failure {
    RawError::new(message).into()
}

/// Параметры источника.
#[derive(Clone, Default)]
pub struct OriginOptions {
    pub logger: Option<Arc<dyn Logger>>,
    pub http: Option<HttpClient>,
    /// Игры и их отпечатки; по умолчанию — [`default_origins`].
    pub origins: Option<Vec<(String, Origin)>>,
}

pub struct OriginUpdates {
    logger: Arc<dyn Logger>,
    http: HttpClient,
    origins: Vec<(String, Origin)>,
    /// Отпечаток другой сборки, о которой уже предупредили (`warned`).
    warned: Mutex<Option<String>>,
}

impl OriginUpdates {
    /// `new OriginUpdates({logger, …})`.
    pub fn new(options: OriginOptions) -> OriginUpdates {
        OriginUpdates {
            logger: options.logger.unwrap_or_else(|| Arc::new(TracingLogger)),
            http: options.http.unwrap_or_default(),
            origins: options.origins.unwrap_or_else(default_origins),
            warned: Mutex::new(None),
        }
    }

    fn origin(&self, game_id: &str) -> Option<&Origin> {
        self.origins
            .iter()
            .find(|(id, _)| id == game_id)
            .map(|(_, origin)| origin)
    }

    /// `supports(gameId)`.
    pub fn supports(&self, game_id: &str) -> bool {
        self.origin(game_id).is_some()
    }

    /// `providerOf(gameId, manifest)`: источник базового архива с id из `ORIGINS`.
    fn provider_of(&self, game_id: &str, manifest: Option<&Value>) -> Result<Value, Failure> {
        let manifest = manifest
            .filter(|manifest| !manifest.is_null())
            .ok_or_else(|| type_error("Cannot read properties of null (reading 'baseArchive')"))?;
        let origin = self.origin(game_id);

        // `(manifest.baseArchive && manifest.baseArchive.providers) || []`.
        let providers = prop(manifest, "baseArchive")
            .filter(|archive| js::truthy(archive))
            .and_then(|archive| prop(archive, "providers"))
            .filter(|providers| js::truthy(providers));
        let list: &[Value] = match providers {
            None => &[],
            Some(Value::Array(list)) => list,
            Some(_) => return Err(type_error("providers.find is not a function")),
        };

        let no_origin = || type_error("Cannot read properties of undefined (reading 'providerId')");
        for item in list {
            if item.is_null() {
                return Err(type_error("Cannot read properties of null (reading 'id')"));
            }
            let origin = origin.ok_or_else(no_origin)?;
            if prop(item, "id").and_then(Value::as_str) == Some(origin.provider_id.as_str()) {
                return Ok(item.clone());
            }
        }
        let origin = origin.ok_or_else(no_origin)?;
        Err(DeliveryError::new(
            Code::ManifestInvalid,
            format!("В манифесте {game_id} нет источника {}", origin.provider_id),
        )
        .into())
    }

    /// `probe(gameId, manifest)`: HEAD архива — `{size, etag, lastModified}`.
    pub async fn probe(&self, game_id: &str, manifest: Option<&Value>) -> Result<Value, Failure> {
        let provider = self.provider_of(game_id, manifest)?;
        let url = js::to_template_string(prop(&provider, "url"));
        let options = RequestOptions {
            method: Method::Head,
            timeout: Some(Duration::from_millis(PROBE_TIMEOUT_MS)),
            ..RequestOptions::default()
        };
        let response = self.http.request(&url, &options).await?;
        let status = response.status();
        let length = response.header("content-length");
        let etag = response.header("etag").filter(|value| !value.is_empty());
        let last_modified = response
            .header("last-modified")
            .filter(|value| !value.is_empty());
        drop(response);

        if status != 200 {
            return Err(DeliveryError::new(
                Code::HttpError,
                format!("HTTP {status} при проверке {url}"),
            )
            .into());
        }
        // `Number(headers["content-length"])`: нет заголовка — `NaN`.
        let size = length.map_or(f64::NAN, |text| js::to_number(&text));
        if !(size.is_finite() && size.fract() == 0.0 && size > 0.0) {
            return Err(DeliveryError::new(
                Code::ProviderLinkFailed,
                "Источник не сообщил размер архива",
            )
            .into());
        }
        let mut stamp = Map::new();
        stamp.insert("size".to_owned(), js_number(size));
        stamp.insert("etag".to_owned(), json!(etag));
        stamp.insert("lastModified".to_owned(), json!(last_modified));
        Ok(Value::Object(stamp))
    }

    /// `target(gameId, manifest)`: цель обновления в том же виде, что у канала — только
    /// закреплённая сборка. `None`: в источнике другой архив, проверка берёт встроенный манифест.
    pub async fn target(
        &self,
        game_id: &str,
        manifest: Option<&Value>,
    ) -> Result<Option<Value>, Failure> {
        let stamp = self.probe(game_id, manifest).await?;
        let origin = self.origin(game_id).ok_or_else(|| {
            type_error("Cannot read properties of undefined (reading 'baseline')")
        })?;
        // После удачного `probe` манифест — объект с источником базового архива.
        let archive = manifest
            .and_then(|manifest| prop(manifest, "baseArchive"))
            .cloned()
            .unwrap_or(Value::Null);

        if !same_stamp(&stamp, &origin.baseline.to_json()) {
            let first = [prop(&stamp, "etag"), prop(&stamp, "lastModified")]
                .into_iter()
                .find(|value| truthy(*value))
                .map_or_else(|| "?".to_owned(), js::to_template_string);
            let mark = format!(
                "{first}, {} Б",
                js::to_template_string(prop(&stamp, "size"))
            );
            let mut warned = self
                .warned
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // Проверка идёт каждые несколько минут — одной записи на отпечаток хватит.
            if warned.as_deref() != Some(mark.as_str()) {
                self.logger.warn(&format!(
                    "[game-delivery] {game_id}: в источнике другая сборка ({mark}), её SHA-256 не закреплён — \
                     обновление не предлагается до выпуска HLSR с этой сборкой"
                ));
            }
            *warned = Some(mark);
            return Ok(None);
        }

        let version = json!(origin.baseline.version);
        let sha256 = prop(&archive, "sha256").cloned();

        let last_modified = prop(&stamp, "lastModified").filter(|value| js::truthy(value));
        let published_at = match last_modified {
            None => Value::Null,
            Some(text) => {
                let at = js::date_parse(&js::to_string(text))
                    .ok_or_else(|| RawError::new("Invalid time value"))?;
                json!(to_iso_string(at))
            }
        };

        let mut full = Map::new();
        full.insert("version".to_owned(), version.clone());
        full.insert(
            "filename".to_owned(),
            json!(format!("{game_id}-{}.7z", js::to_string(&version))),
        );
        full.insert(
            "size".to_owned(),
            prop(&stamp, "size").cloned().unwrap_or(Value::Null),
        );
        if let Some(sha256) = sha256 {
            full.insert("sha256".to_owned(), sha256);
        }
        full.insert(
            "rootPrefix".to_owned(),
            prop(&archive, "rootPrefix")
                .filter(|prefix| js::truthy(prefix))
                .cloned()
                .unwrap_or(Value::Null),
        );
        full.insert(
            "providers".to_owned(),
            Value::Array(vec![self.provider_of(game_id, manifest)?]),
        );

        let mut target = Map::new();
        target.insert("source".to_owned(), json!("origin"));
        target.insert("version".to_owned(), version);
        target.insert("updateRequired".to_owned(), json!(false));
        target.insert("minimumLauncherVersion".to_owned(), Value::Null);
        target.insert("publishedAt".to_owned(), published_at);
        target.insert("expectedInstalledSize".to_owned(), Value::Null);
        target.insert("patches".to_owned(), json!([]));
        target.insert("fullArchive".to_owned(), Value::Object(full));
        for key in ["repairMirror", "filesManifest", "buildMetadata", "notes"] {
            target.insert(key.to_owned(), Value::Null);
        }
        target.insert("stamp".to_owned(), stamp);
        Ok(Some(Value::Object(target)))
    }
}

impl std::fmt::Debug for OriginUpdates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OriginUpdates")
            .field("origins", &self.origins)
            .finish_non_exhaustive()
    }
}
