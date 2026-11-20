//! Перебор источников (`mirror-runner.js`): по порядку, пока источник не ответит; коды
//! `SWITCH_CODES` ведут к следующему зеркалу, остальные останавливают операцию. На 429
//! хост встаёт на паузу (`Retry-After`, по умолчанию 15 мин, не больше часа) и
//! пропускается, пока она не кончится. Один артефакт не качается дважды одновременно.
//! Журнал попыток — для диагностики.

use crate::errors::{Code, DeliveryError, Failure, to_delivery_error};
use crate::ports::Logger;
use hlsr_core::js;
use hlsr_core::json_text::js_number;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use url::Url;

pub const SWITCH_CODES: &[Code] = &[
    Code::NetworkError,
    Code::NetworkTimeout,
    Code::HttpError,
    Code::TooManyRedirects,
    Code::InsecureRedirect,
    Code::ContentLengthMismatch,
    Code::SizeMismatch,
    Code::ChecksumMismatch,
    Code::ProviderLinkFailed,
    Code::ProviderForbidden,
    Code::ChannelUnavailable,
    Code::SignatureInvalid,
    Code::ManifestInvalid,
    Code::FilesManifestMissing,
];

pub const STOP_CODES: &[Code] = &[
    Code::Cancelled,
    Code::AlreadyRunning,
    Code::GameRunning,
    Code::FileLocked,
    Code::NoSpace,
    Code::PathNotWritable,
    Code::PathInvalid,
    Code::ArchiveListFailed,
    Code::ArchiveUnsafeEntry,
    Code::ArchiveLimitExceeded,
    Code::ExtractFailed,
    Code::RequiredFileMissing,
    Code::InstallFailed,
    Code::RollbackFailed,
    Code::RepairFailed,
    Code::VerifyFailed,
    Code::PatchFailed,
    Code::PatchChainMissing,
    Code::RemoteDisabled,
    Code::ProviderUnsupported,
    Code::ManifestDisabled,
    Code::ManifestNotFound,
    Code::ManifestVersionDraft,
    Code::LauncherTooOld,
    Code::ChannelDisabled,
    Code::UpdateRequired,
    Code::NotInstalled,
    Code::VersionMismatch,
];

/// Коды, после которых недокачанный файл выбрасывается (`onDiscardPartial`).
pub const DISCARD_PARTIAL_CODES: &[Code] = &[
    Code::SizeMismatch,
    Code::ChecksumMismatch,
    Code::ContentLengthMismatch,
];

pub const DEFAULT_COOLDOWN_MS: i64 = 15 * 60 * 1000;
pub const MAX_COOLDOWN_MS: i64 = 60 * 60 * 1000;

/// Часы: миллисекунды от 1970 года (`Date.now()`).
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Часы ОС.
pub fn system_clock() -> Clock {
    Arc::new(|| {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as i64)
    })
}

/// `new URL(String(descriptor.url))` — или `None`, если адреса нет или он неверный.
fn url_of(descriptor: &Value) -> Option<Url> {
    let raw = descriptor.get("url").filter(|url| js::truthy(url))?;
    Url::parse(&js::to_string(raw)).ok()
}

/// `hostOf(descriptor)`: `URL.hostname` адреса источника.
pub fn host_of(descriptor: &Value) -> Option<String> {
    url_of(descriptor).map(|url| crate::http_client::url_hostname(&url).to_owned())
}

/// `authorityOf(descriptor)`: `URL.host` (с портом, если он не стандартный).
pub fn authority_of(descriptor: &Value) -> Option<String> {
    url_of(descriptor).map(|url| crate::http_client::url_host(&url))
}

/// `parseRetryAfter(value, now)`: секунды (`/^\d+$/`) или дата HTTP; ответ — миллисекунды,
/// не больше часа. Дата в прошлом, мусор, отрицательное число — `None`.
pub fn parse_retry_after(value: Option<&str>, now: i64) -> Option<i64> {
    let raw = js::trim(value?);
    if raw.is_empty() {
        return None;
    }
    if raw.bytes().all(|b| b.is_ascii_digit()) {
        // Number.parseInt: длинное число — очень большое, больше f64 — Infinity (не годится).
        let seconds: f64 = raw.parse().unwrap_or(f64::INFINITY);
        if !seconds.is_finite() {
            return None;
        }
        return Some((seconds * 1000.0).min(MAX_COOLDOWN_MS as f64) as i64);
    }
    let at = js::date_parse(raw)?;
    let delta = at - now;
    (delta > 0).then(|| delta.min(MAX_COOLDOWN_MS))
}

/// Что делать после отказа источника.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Следующее зеркало.
    Switch,
    /// Смена зеркала не поможет.
    Stop,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Switch => "switch",
            Action::Stop => "stop",
        }
    }
}

/// `classify(error)`: `{action, code}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    pub action: Action,
    pub code: Code,
}

/// Ошибка источника как `DeliveryError`: чужая — `INSTALL_FAILED` (`toDeliveryError`).
pub fn to_delivery(failure: Failure) -> DeliveryError {
    to_delivery_error(failure, Some(Code::InstallFailed))
}

/// `classify(error)`: коды из `SWITCH_CODES` — к следующему зеркалу, прочие — стоп.
pub fn classify(failure: &Failure) -> Verdict {
    let code = to_delivery(failure.clone()).code;
    let action = if SWITCH_CODES.contains(&code) && !STOP_CODES.contains(&code) {
        Action::Switch
    } else {
        Action::Stop
    };
    Verdict { action, code }
}

/// `httpStatusOf(error)`: `details.status || details.statusCode`, если это конечное число.
fn http_status_of(error: &DeliveryError) -> Option<f64> {
    let details = error.details.as_ref()?;
    let status = details
        .get("status")
        .filter(|status| js::truthy(status))
        .or_else(|| details.get("statusCode"))?;
    status.as_f64().filter(|status| status.is_finite())
}

/// Итог попытки в журнале.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultCode {
    Ok,
    /// `isTerminalResult` сказал «остановиться на этом ответе».
    Terminal,
    /// Хост на паузе после 429.
    HostCooldown,
    Failed(Code),
}

impl ResultCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ResultCode::Ok => "OK",
            ResultCode::Terminal => "TERMINAL",
            ResultCode::HostCooldown => "HOST_COOLDOWN",
            ResultCode::Failed(code) => code.as_str(),
        }
    }
}

/// Запись журнала попыток.
#[derive(Debug, Clone, PartialEq)]
pub struct AttemptRecord {
    pub provider_id: Option<String>,
    pub host: Option<String>,
    /// С единицы.
    pub attempt: usize,
    pub result_code: ResultCode,
    /// Код HTTP из подробностей ошибки (число JS).
    pub http_status: Option<f64>,
    pub retry_after_ms: Option<i64>,
}

impl AttemptRecord {
    fn new(descriptor: &Value, index: usize, result_code: ResultCode) -> AttemptRecord {
        AttemptRecord {
            provider_id: provider_id(descriptor),
            host: host_of(descriptor),
            attempt: index + 1,
            result_code,
            http_status: None,
            retry_after_ms: None,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "providerId": self.provider_id,
            "host": self.host,
            "attempt": self.attempt,
            "resultCode": self.result_code.as_str(),
            "httpStatus": self.http_status.map_or(Value::Null, js_number),
            "retryAfterMs": self.retry_after_ms,
        })
    }
}

/// `descriptor.id || null`.
fn provider_id(descriptor: &Value) -> Option<String> {
    descriptor
        .get("id")
        .filter(|id| js::truthy(id))
        .map(js::to_string)
}

/// Паузы хостов (`HostCooldown`): хост → до какого момента.
pub struct HostCooldown {
    now: Clock,
    until: Mutex<Vec<(String, i64)>>,
}

impl HostCooldown {
    pub fn new(now: Clock) -> HostCooldown {
        HostCooldown {
            now,
            until: Mutex::new(Vec::new()),
        }
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, Vec<(String, i64)>> {
        self.until.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `blockedFor(host)`: сколько миллисекунд ещё ждать; кончившаяся пауза забывается.
    pub fn blocked_for(&self, host: Option<&str>) -> i64 {
        let Some(host) = host.filter(|h| !h.is_empty()) else {
            return 0;
        };
        let mut entries = self.entries();
        let Some(index) = entries.iter().position(|(h, _)| h == host) else {
            return 0;
        };
        let left = entries[index].1 - (self.now)();
        if left <= 0 {
            entries.remove(index);
            return 0;
        }
        left
    }

    /// `pause(host, ms)`: пауза не больше часа; более длинная прежняя остаётся.
    pub fn pause(&self, host: Option<&str>, ms: i64) {
        let Some(host) = host.filter(|h| !h.is_empty()) else {
            return;
        };
        if ms == 0 {
            return;
        }
        let until = (self.now)() + ms.min(MAX_COOLDOWN_MS);
        let mut entries = self.entries();
        match entries.iter_mut().find(|(h, _)| h == host) {
            Some(entry) => entry.1 = entry.1.max(until),
            None => entries.push((host.to_owned(), until)),
        }
    }

    /// `snapshot()`: хосты на паузе и сколько им осталось, в порядке постановки.
    pub fn snapshot(&self) -> Vec<(String, i64)> {
        let hosts: Vec<String> = self.entries().iter().map(|(h, _)| h.clone()).collect();
        hosts
            .into_iter()
            .filter_map(|host| {
                let left = self.blocked_for(Some(&host));
                (left > 0).then_some((host, left))
            })
            .collect()
    }
}

/// Попытка, которую получает операция: `{descriptor, index, isFallback}`.
#[derive(Debug, Clone)]
pub struct Attempt {
    pub descriptor: Value,
    pub index: usize,
    pub is_fallback: bool,
}

/// Итог `run`: `{value, providerId, usedFallback, attempts}`.
#[derive(Debug, Clone)]
pub struct MirrorOutcome<T> {
    pub value: T,
    pub provider_id: Option<String>,
    pub used_fallback: bool,
    pub attempts: Vec<AttemptRecord>,
}

/// `isTerminalResult(value)`: ответ, на котором перебор кончается без «OK» (пауза).
pub type TerminalResult<'a, T> = dyn Fn(&T) -> bool + Send + Sync + 'a;

/// `onDiscardPartial({descriptor, code})`: выбросить недокачанный файл.
pub type DiscardPartial<'a> = dyn Fn(&Value, Code) -> Result<(), Failure> + Send + Sync + 'a;

/// Запрос `run`.
pub struct MirrorRequest<'a, T> {
    /// Ключ артефакта: пока он качается, второй такой же запуск — `ALREADY_RUNNING`.
    /// Пустой — без проверки.
    pub artifact: &'a str,
    /// Источники по порядку (`orderProviders`).
    pub descriptors: &'a [Value],
    pub is_terminal_result: Option<&'a TerminalResult<'a, T>>,
    pub on_discard_partial: Option<&'a DiscardPartial<'a>>,
}

/// `MirrorRunner`.
pub struct MirrorRunner {
    logger: Arc<dyn Logger>,
    now: Clock,
    cooldown: HostCooldown,
    in_flight: Mutex<HashSet<String>>,
}

/// Артефакт в работе; снимается и при сбросе будущего `run`.
struct InFlight<'a> {
    set: &'a Mutex<HashSet<String>>,
    key: String,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.key);
    }
}

impl MirrorRunner {
    pub fn new(logger: Arc<dyn Logger>) -> MirrorRunner {
        MirrorRunner::with_clock(logger, system_clock())
    }

    pub fn with_clock(logger: Arc<dyn Logger>, now: Clock) -> MirrorRunner {
        MirrorRunner {
            logger,
            cooldown: HostCooldown::new(now.clone()),
            now,
            in_flight: Mutex::new(HashSet::new()),
        }
    }

    pub fn cooldown(&self) -> &HostCooldown {
        &self.cooldown
    }

    /// `run(request)`: операция по источникам по порядку.
    pub async fn run<T, F, Fut>(
        &self,
        request: MirrorRequest<'_, T>,
        mut operation: F,
    ) -> Result<MirrorOutcome<T>, DeliveryError>
    where
        F: FnMut(Attempt) -> Fut,
        Fut: Future<Output = Result<T, Failure>>,
    {
        let descriptors = request.descriptors;
        if descriptors.is_empty() {
            return Err(DeliveryError::new(
                Code::RemoteDisabled,
                "Нет ни одного доступного источника",
            ));
        }
        let key = request.artifact;
        let _in_flight = if key.is_empty() {
            None
        } else {
            let mut set = self.in_flight.lock().unwrap_or_else(|p| p.into_inner());
            if !set.insert(key.to_owned()) {
                return Err(DeliveryError::new(
                    Code::AlreadyRunning,
                    format!("Артефакт {key} уже загружается"),
                ));
            }
            Some(InFlight {
                set: &self.in_flight,
                key: key.to_owned(),
            })
        };

        let mut attempts = Vec::new();
        let mut last_error = None;
        for (index, descriptor) in descriptors.iter().enumerate() {
            let id = js::to_template_string(descriptor.get("id"));
            let authority = authority_of(descriptor);

            let blocked = self.cooldown.blocked_for(authority.as_deref());
            if blocked > 0 {
                let mut record = AttemptRecord::new(descriptor, index, ResultCode::HostCooldown);
                record.retry_after_ms = Some(blocked);
                attempts.push(record);
                self.logger.warn(&format!(
                    "[game-delivery] источник {id} пропущен: cooldown ещё {} с",
                    (blocked + 500).div_euclid(1000)
                ));
                continue;
            }

            let attempt = Attempt {
                descriptor: descriptor.clone(),
                index,
                is_fallback: index > 0,
            };
            match operation(attempt).await {
                Ok(value) => {
                    let terminal = request.is_terminal_result.is_some_and(|f| f(&value));
                    let code = if terminal {
                        ResultCode::Terminal
                    } else {
                        ResultCode::Ok
                    };
                    attempts.push(AttemptRecord::new(descriptor, index, code));
                    if !terminal && index > 0 {
                        self.logger.warn(&format!(
                            "[game-delivery] артефакт {} получен с резервного источника {id}",
                            if key.is_empty() { "?" } else { key }
                        ));
                    }
                    return Ok(MirrorOutcome {
                        value,
                        provider_id: provider_id(descriptor),
                        used_fallback: index > 0,
                        attempts,
                    });
                }
                Err(failure) => {
                    let verdict = classify(&failure);
                    let delivery = to_delivery(failure);
                    let status = http_status_of(&delivery);

                    let mut retry_after_ms = None;
                    if status == Some(429.0) {
                        let retry_after =
                            delivery.details.as_ref().and_then(|d| d.get("retryAfter"));
                        let retry_after = retry_after.filter(|v| !v.is_null()).map(js::to_string);
                        let ms = parse_retry_after(retry_after.as_deref(), (self.now)())
                            .filter(|&ms| ms != 0)
                            .unwrap_or(DEFAULT_COOLDOWN_MS);
                        self.cooldown.pause(authority.as_deref(), ms);
                        retry_after_ms = Some(ms);
                    }

                    let mut record =
                        AttemptRecord::new(descriptor, index, ResultCode::Failed(verdict.code));
                    record.http_status = status;
                    record.retry_after_ms = retry_after_ms;
                    attempts.push(record);

                    if verdict.action == Action::Stop {
                        self.logger.warn(&format!(
                            "[game-delivery] источник {id} отказал с {}: смена зеркала не поможет, \
                             операция остановлена",
                            verdict.code
                        ));
                        return Err(delivery);
                    }

                    if DISCARD_PARTIAL_CODES.contains(&verdict.code)
                        && let Some(discard) = request.on_discard_partial
                        && let Err(error) = discard(descriptor, verdict.code)
                    {
                        let message = match error {
                            Failure::Delivery(error) => error.message,
                            Failure::Raw(error) => error.message,
                        };
                        self.logger.warn(&format!(
                            "[game-delivery] не удалось очистить partial: {message}"
                        ));
                    }

                    self.logger.warn(&format!(
                        "[game-delivery] источник {id} отказал с {}{}",
                        verdict.code,
                        if index + 1 < descriptors.len() {
                            ", пробуем следующее зеркало"
                        } else {
                            ""
                        }
                    ));
                    last_error = Some(delivery);
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| DeliveryError::new(Code::NetworkError, "Все источники недоступны")))
    }
}
