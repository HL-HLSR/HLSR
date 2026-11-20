//! `security/attestation.js`: подтверждение сборки перед онлайн-функциями.
//!
//! Хеш файла в `X-HLSR-Build` подделывается одной строкой, поэтому к нему добавлен ответ на
//! вызов сервера: лаунчер просит вызов, сервер называет несколько участков файла сборки (из
//! манифеста выпуска) и одноразовое число; лаунчер читает участки с диска, считает их SHA-256 и
//! подписывает вместе с числом ключом своей сборки (`build_key.rs`). Верная подпись меняется на
//! билет — запись сервера «эта сборка проверена» на 15 минут, она уходит заголовком
//! `X-HLSR-Ticket`.
//!
//! Кеш и повторы — как в 0.4.7: билет обновляется за 120 с до конца срока; одновременные
//! запросы ждут одно рукопожатие; после сбоя — пауза 30 с (`force` — не чаще раза в 10 с);
//! 404 — тишина на час; `CLIENT_MODIFIED` пробрасывается вызывающему.

use super::build_identity::BuildIdentity;
use super::build_key::AttestKey;
use crate::telemetry::config;
use crate::telemetry::transport::{RequestOptions, Transport, TransportError};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hlsr_core::js;
use hlsr_core::json_text::{json_parse, stringify};
use hlsr_core::logger::Logger;
use hlsr_delivery::timing::{Clock, system_clock};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

pub const CHALLENGE_PATH: &str = "/v1/attest/challenge";
pub const PROVE_PATH: &str = "/v1/attest/prove";
pub const LABEL: &str = "hlsr-attest/1";
const MAX_RANGES: usize = 8;
const MAX_RANGE_BYTES: i64 = 256 * 1024;
const REFRESH_BEFORE_MS: i64 = 120 * 1000;
const FAILURE_BACKOFF_MS: i64 = 30 * 1000;
const FORCE_GAP_MS: i64 = 10 * 1000;
const UNSUPPORTED_MS: i64 = 60 * 60 * 1000;
const TIMEOUT_MS: u64 = 10_000;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// Сервер отверг подпись и признал сборку изменённой.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("CLIENT_MODIFIED")]
pub struct ClientModified;

/// Сбой рукопожатия: `CLIENT_MODIFIED` или любая другая ошибка (текст — `error.message`).
#[derive(Debug, Clone)]
enum Failure {
    ClientModified,
    Other(String),
}

impl Failure {
    fn coded(code: &str) -> Failure {
        Failure::Other(code.to_owned())
    }
}

impl From<TransportError> for Failure {
    fn from(error: TransportError) -> Failure {
        Failure::Other(error.to_string())
    }
}

type Outcome = Result<Option<String>, ClientModified>;

#[derive(Default)]
struct State {
    /// Билет и момент конца срока (мс).
    cached: Option<(String, i64)>,
    /// Идущее рукопожатие: его итог ждут все одновременные запросы.
    pending: Option<watch::Receiver<Option<Outcome>>>,
    retry_at: i64,
    last_attempt: i64,
    unsupported_until: i64,
}

/// Настройки `new BuildAttestation(opts)`.
pub struct AttestationOptions {
    pub identity: Option<BuildIdentity>,
    pub key: Option<Arc<dyn AttestKey>>,
    pub transport: Arc<dyn Transport>,
    /// `opts.apiBase || config.API_BASE`.
    pub api_base: String,
    /// `opts.launcherVersion || "0.0.0"`.
    pub launcher_version: String,
    pub clock: Clock,
    pub logger: Arc<dyn Logger>,
}

impl AttestationOptions {
    /// Настройки запущенного лаунчера: адрес API по умолчанию, часы ОС.
    pub fn new(
        identity: BuildIdentity,
        key: Arc<dyn AttestKey>,
        transport: Arc<dyn Transport>,
        launcher_version: impl Into<String>,
        logger: Arc<dyn Logger>,
    ) -> AttestationOptions {
        AttestationOptions {
            identity: Some(identity),
            key: Some(key),
            transport,
            api_base: config::API_BASE.to_owned(),
            launcher_version: launcher_version.into(),
            clock: system_clock(),
            logger,
        }
    }
}

struct Inner {
    identity: Option<BuildIdentity>,
    key: Option<Arc<dyn AttestKey>>,
    transport: Arc<dyn Transport>,
    base: String,
    version: String,
    clock: Clock,
    logger: Arc<dyn Logger>,
    state: Mutex<State>,
}

/// `BuildAttestation`; копии делят билет и состояние.
#[derive(Clone)]
pub struct BuildAttestation {
    inner: Arc<Inner>,
}

impl BuildAttestation {
    pub fn new(options: AttestationOptions) -> BuildAttestation {
        let version = if options.launcher_version.is_empty() {
            "0.0.0".to_owned()
        } else {
            options.launcher_version
        };
        let base = if options.api_base.is_empty() {
            config::API_BASE.to_owned()
        } else {
            options.api_base
        };
        BuildAttestation {
            inner: Arc::new(Inner {
                identity: options.identity,
                key: options.key,
                transport: options.transport,
                base,
                version,
                clock: options.clock,
                logger: options.logger,
                state: Mutex::new(State::default()),
            }),
        }
    }

    fn now(&self) -> i64 {
        (self.inner.clock)()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// `enabled()`: подписать есть чем, и это сборка выпуска (у сборки разработки вместо хеша
    /// — `dev`).
    pub fn enabled(&self) -> bool {
        self.inner
            .identity
            .as_ref()
            .is_some_and(BuildIdentity::packaged)
            && self.inner.key.as_ref().is_some_and(|key| key.available())
    }

    /// `ticket(force)`: билет для `X-HLSR-Ticket` или `None`, если подтвердить сборку нечем или
    /// сервер этого не требует. `force` — получить новый билет, потому что сервер отказал по
    /// старому. Ошибка — только `CLIENT_MODIFIED`.
    pub async fn ticket(&self, force: bool) -> Outcome {
        if !self.enabled() {
            return Ok(None);
        }
        let mut receiver = {
            let mut state = self.state();
            let now = self.now();
            if now < state.unsupported_until {
                return Ok(None);
            }
            let fresh = state
                .cached
                .as_ref()
                .filter(|(_, expires_at)| *expires_at > now)
                .cloned();
            if let Some((ticket, expires_at)) = &fresh
                && !force
                && expires_at - now > REFRESH_BEFORE_MS
            {
                return Ok(Some(ticket.clone()));
            }
            let fresh = fresh.map(|(ticket, _)| ticket);
            if let Some(pending) = &state.pending {
                pending.clone()
            } else {
                let gap_ok = force && now - state.last_attempt >= FORCE_GAP_MS;
                if now < state.retry_at && !gap_ok {
                    return Ok(fresh);
                }
                state.last_attempt = now;
                let (sender, receiver) = watch::channel(None);
                state.pending = Some(receiver.clone());
                let this = self.clone();
                tokio::spawn(async move {
                    let outcome = this.settle(this.handshake().await, fresh);
                    let _ = sender.send(Some(outcome));
                });
                receiver
            }
        };
        let settled = receiver
            .wait_for(Option::is_some)
            .await
            .map(|value| value.clone());
        match settled {
            Ok(Some(outcome)) => outcome,
            // Задача рукопожатия не дожила до конца (среда выполнения останавливается): общий
            // запрос снимается, чтобы следующий вызов начал заново.
            _ => {
                let mut state = self.state();
                if state
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.same_channel(&receiver))
                {
                    state.pending = None;
                }
                Ok(None)
            }
        }
    }

    /// Итог рукопожатия → кеш, пауза, журнал; общий запрос снимается.
    fn settle(
        &self,
        result: Result<Option<(String, i64)>, Failure>,
        fresh: Option<String>,
    ) -> Outcome {
        let mut state = self.state();
        state.pending = None;
        match result {
            Ok(Some((ticket, ttl))) => {
                state.cached = Some((ticket.clone(), self.now() + ttl * 1000));
                Ok(Some(ticket))
            }
            Ok(None) => {
                state.retry_at = self.now() + FAILURE_BACKOFF_MS;
                Ok(fresh)
            }
            Err(Failure::ClientModified) => Err(ClientModified),
            Err(Failure::Other(message)) => {
                state.retry_at = self.now() + FAILURE_BACKOFF_MS;
                drop(state);
                self.inner.logger.warn(&format!(
                    "[attest] подтвердить сборку не удалось: {message}"
                ));
                Ok(fresh)
            }
        }
    }

    /// `post(endpoint, body, build)`: тело ответа 200 (объект или массив); `None` — 404.
    async fn post(
        &self,
        endpoint: &str,
        body: &Value,
        build: &str,
    ) -> Result<Option<Value>, Failure> {
        let options = RequestOptions {
            method: Some("POST".to_owned()),
            headers: vec![("x-hlsr-build".to_owned(), build.to_owned())],
            body: Some(stringify(body, "")),
            timeout_ms: Some(TIMEOUT_MS),
            max_response_bytes: Some(MAX_RESPONSE_BYTES),
            launcher_version: Some(self.inner.version.clone()),
        };
        let url = format!("{}{endpoint}", self.inner.base);
        let response = self.inner.transport.request(&url, &options).await?;
        let parsed = json_parse(&response.body).ok();
        if response.status == 200 {
            return match parsed {
                Some(data @ (Value::Object(_) | Value::Array(_))) => Ok(Some(data)),
                _ => Err(Failure::coded("ATTEST_BAD_RESPONSE")),
            };
        }
        // `(parse(response) || {}).error`: поле есть только у объекта.
        let error = parsed
            .as_ref()
            .and_then(|data| data.as_object())
            .and_then(|data| data.get("error"))
            .filter(|error| js::truthy(error));
        if response.status == 404 {
            // Сервер не знает аттестации или у этой версии нет ключа.
            self.state().unsupported_until = self.now() + UNSUPPORTED_MS;
            return Ok(None);
        }
        if response.status == 403 && error.and_then(Value::as_str) == Some("CLIENT_MODIFIED") {
            return Err(Failure::ClientModified);
        }
        let suffix = error.map_or_else(String::new, |error| format!("_{}", js::to_string(error)));
        Err(Failure::Other(format!(
            "ATTEST_HTTP_{}{suffix}",
            response.status
        )))
    }

    /// `handshake()`: вызов → участки файла → подпись → билет и срок жизни в секундах.
    async fn handshake(&self) -> Result<Option<(String, i64)>, Failure> {
        let (Some(identity), Some(key)) = (&self.inner.identity, &self.inner.key) else {
            return Ok(None);
        };
        let build = identity.header().await;
        if !valid_build_header(&build) {
            return Ok(None);
        }
        let Some(challenge) = self.post(CHALLENGE_PATH, &json!({}), &build).await? else {
            return Ok(None);
        };
        let token = challenge.get("challenge").and_then(Value::as_str);
        let ranges = challenge.get("ranges").and_then(Value::as_array);
        let (Some(token), Some(ranges)) = (token, ranges) else {
            return Err(Failure::coded("ATTEST_BAD_CHALLENGE"));
        };
        if js::text::utf16_len(token) > 1024 || ranges.is_empty() || ranges.len() > MAX_RANGES {
            return Err(Failure::coded("ATTEST_BAD_CHALLENGE"));
        }
        let mut hashes = Vec::with_capacity(ranges.len());
        for range in ranges {
            let pair = range.as_array().filter(|pair| pair.len() == 2);
            let bounds = pair.and_then(|pair| {
                let offset = crate::js_value::safe_integer(pair.first())?;
                let length = crate::js_value::safe_integer(pair.get(1))?;
                (offset >= 0 && length > 0 && length <= MAX_RANGE_BYTES).then_some(pair)
            });
            let Some(pair) = bounds else {
                return Err(Failure::coded("ATTEST_BAD_CHALLENGE"));
            };
            let bytes = identity
                .read_range(&pair[0], &pair[1])
                .await
                .map_err(|error| Failure::Other(error.message))?;
            hashes.push(js::text::hex(&Sha256::digest(&bytes)));
        }
        let message = [LABEL, &build, token, &hashes.join(",")].join("\n");
        let signature = key
            .sign(message.as_bytes())
            .map_err(|error| Failure::Other(error.to_string()))?;
        let body = json!({ "challenge": token, "signature": URL_SAFE_NO_PAD.encode(signature) });
        let Some(proved) = self.post(PROVE_PATH, &body, &build).await? else {
            return Ok(None);
        };
        let ticket = proved
            .get("ticket")
            .and_then(Value::as_str)
            .filter(|ticket| !ticket.is_empty() && js::text::utf16_len(ticket) <= 1024);
        // `Number.isInteger(ttl)`: целое число любой величины; границы 60–3600.
        let ttl = proved
            .get("ttl")
            .and_then(Value::as_f64)
            .filter(|ttl| ttl.fract() == 0.0 && (60.0..=3600.0).contains(ttl));
        match (ticket, ttl) {
            (Some(ticket), Some(ttl)) => Ok(Some((ticket.to_owned(), ttl as i64))),
            _ => Err(Failure::coded("ATTEST_BAD_RESPONSE")),
        }
    }
}

/// `BUILD_HEADER`: `^(win32|linux)/[0-9A-Za-z.-]+/[a-f0-9]{64}$`.
pub fn valid_build_header(header: &str) -> bool {
    let mut parts = header.split('/');
    let (Some(platform), Some(version), Some(sha), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    matches!(platform, "win32" | "linux")
        && !version.is_empty()
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
        && sha.len() == 64
        && sha
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}
