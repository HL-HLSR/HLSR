//! Подпись манифестов и каналов Ed25519 (`manifest-signing.js`): каноническая форма,
//! подпись, проверка, `check` с причинами.
//!
//! Подпись — base64 64 байт в `signature.value`, `algorithm: "ed25519"`; `keyId` не
//! подписывается и при проверке не используется: перебираются все доверенные ключи.
//! Ключи — PEM (SPKI для открытого, PKCS#8 для закрытого), как у `crypto` в Node.

use crate::js::{self, buffer_from_base64};
use crate::json_text::stringify;
use base64::prelude::{BASE64_STANDARD, Engine as _};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde_json::{Map, Value, json};

/// Подпись не удалась или обязательная подпись не прошла: у 0.4.7 это `DeliveryError` с кодом
/// `MANIFEST_INVALID`. Код знает только доставка (`hlsr_delivery::errors::Code::ManifestInvalid`),
/// здесь только текст; `hlsr-delivery` превращает ошибку в свою, `hlsr-update` — в `LU_*`/`UPD_*`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SigningError(pub String);

impl SigningError {
    fn new(message: impl Into<String>) -> SigningError {
        SigningError(message.into())
    }
}

pub const ALGORITHM: &str = "ed25519";
pub const SIGNATURE_FIELD: &str = "signature";

/// Префикс DER `SubjectPublicKeyInfo` ключа Ed25519 (за ним 32 байта ключа).
const SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];
/// Префикс DER PKCS#8 закрытого ключа Ed25519 (за ним 32 байта зерна).
const PKCS8_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

// ------------------------------------------------------------ каноническая форма

/// Каноническая форма: примитивы как `JSON.stringify`, ключи объекта по порядку единиц
/// UTF-16, без пробелов, поле `signature` пропускается на любой глубине.
///
/// Одиночный суррогат в строке `Value` не хранится: `parse_js_json` заменяет его на
/// U+FFFD, а JS подписал бы `\udXXX`. Подписанный документ с такой строкой Rust не
/// проверит; подписывать такие документы нечем и незачем.
pub fn canonicalize(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(&mut out, value);
    out
}

fn write_canonical(out: &mut String, value: &Value) {
    match value {
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => write_canonical_object(out, map),
        primitive => out.push_str(&stringify(primitive, "")),
    }
}

fn write_canonical_object(out: &mut String, map: &Map<String, Value>) {
    let mut keys: Vec<&String> = map
        .keys()
        .filter(|key| key.as_str() != SIGNATURE_FIELD)
        .collect();
    keys.sort_by(|a, b| js::cmp_utf16(a, b));
    out.push('{');
    for (index, key) in keys.into_iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&stringify(&Value::String(key.clone()), ""));
        out.push(':');
        write_canonical(out, &map[key]);
    }
    out.push('}');
}

// ------------------------------------------------------------ PEM

/// Первый блок PEM текста: метка и содержимое. Строки читаются как у OpenSSL: пробелы и
/// `\r` в конце отбрасываются, перед блоком и после него может быть любой текст, метки
/// `BEGIN` и `END` совпадают.
fn pem_block(text: &str) -> Option<(&str, Vec<u8>)> {
    let mut lines = text
        .split('\n')
        .map(|line| line.trim_end_matches([' ', '\t', '\r']));
    let label = lines.find_map(|line| {
        line.strip_prefix("-----BEGIN ")
            .and_then(|rest| rest.strip_suffix("-----"))
    })?;
    let end = format!("-----END {label}-----");
    let mut body = String::new();
    for line in lines {
        if line == end {
            let body: String = body.chars().filter(|c| !c.is_ascii_whitespace()).collect();
            return BASE64_STANDARD.decode(body).ok().map(|der| (label, der));
        }
        if line.starts_with("-----END ") {
            return None;
        }
        body.push_str(line);
    }
    None
}

fn pem_text(label: &str, der: &[u8]) -> String {
    let body = BASE64_STANDARD.encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for line in body.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(line).expect("base64 — ASCII"));
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

fn signing_key_from_der(der: &[u8]) -> Option<SigningKey> {
    let seed: &[u8; 32] = der.strip_prefix(&PKCS8_PREFIX)?.try_into().ok()?;
    Some(SigningKey::from_bytes(seed))
}

/// `crypto.createPublicKey(pem)` для ключа Ed25519: открытый ключ (`PUBLIC KEY`) или
/// закрытый (`PRIVATE KEY`), из которого берётся открытый. Всё остальное — не Ed25519.
fn public_key_from_pem(pem: &str) -> Option<VerifyingKey> {
    let (label, der) = pem_block(pem)?;
    match label {
        "PUBLIC KEY" => {
            let key: &[u8; 32] = der.strip_prefix(&SPKI_PREFIX)?.try_into().ok()?;
            VerifyingKey::from_bytes(key).ok()
        }
        "PRIVATE KEY" => Some(signing_key_from_der(&der)?.verifying_key()),
        _ => None,
    }
}

// ------------------------------------------------------------ ключи

/// Пара ключей в PEM.
#[derive(Clone)]
pub struct KeyPair {
    pub public_key_pem: String,
    pub private_key_pem: String,
}

/// Ключи Ed25519 из зерна — как `crypto.createPrivateKey` с PKCS#8 из тех же 32 байт.
pub fn key_pair_from_seed(seed: &[u8; 32]) -> KeyPair {
    let signing = SigningKey::from_bytes(seed);
    let mut public = SPKI_PREFIX.to_vec();
    public.extend_from_slice(signing.verifying_key().as_bytes());
    let mut private = PKCS8_PREFIX.to_vec();
    private.extend_from_slice(seed);
    KeyPair {
        public_key_pem: pem_text("PUBLIC KEY", &public),
        private_key_pem: pem_text("PRIVATE KEY", &private),
    }
}

/// `generateKeyPair()`: новая пара ключей, зерно — от генератора ОС.
pub fn generate_key_pair() -> KeyPair {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).expect("системный генератор случайных чисел недоступен");
    key_pair_from_seed(&seed)
}

// ------------------------------------------------------------ подпись и проверка

fn invalid_private_key(reason: &str) -> SigningError {
    SigningError::new(format!("Некорректный приватный ключ: {reason}"))
}

fn signing_key_from_pem(pem: &str) -> Result<SigningKey, SigningError> {
    let (label, der) = pem_block(pem).ok_or_else(|| invalid_private_key("это не PEM"))?;
    if label != "PRIVATE KEY" {
        return Err(invalid_private_key(&format!("метка PEM «{label}»")));
    }
    signing_key_from_der(&der)
        .ok_or_else(|| invalid_private_key("это не закрытый ключ Ed25519 в PKCS#8"))
}

fn sign_payload(payload: &str, private_key_pem: &str) -> Result<Value, SigningError> {
    let key = signing_key_from_pem(private_key_pem)?;
    let signature = key.sign(payload.as_bytes());
    Ok(json!({
        "algorithm": ALGORITHM,
        "value": BASE64_STANDARD.encode(signature.to_bytes()),
    }))
}

/// `sign(manifest, privateKeyPem)`: `{algorithm, value}`. Ключ, не являющийся закрытым
/// ключом Ed25519, — `MANIFEST_INVALID` (текст причины, в отличие от OpenSSL, свой).
pub fn sign(manifest: &Value, private_key_pem: &str) -> Result<Value, SigningError> {
    sign_payload(&canonicalize(manifest), private_key_pem)
}

/// `verify(manifest, signature, publicKeyPem)`: `false` при любой неполадке — чужой
/// алгоритм, не строка в `value`, ключ не Ed25519, подпись не той длины. Значение
/// `value` декодируется как `Buffer.from(…, "base64")`.
pub fn verify(manifest: &Value, signature: &Value, public_key_pem: &str) -> bool {
    if signature.get("algorithm").and_then(Value::as_str) != Some(ALGORITHM) {
        return false;
    }
    let Some(value) = signature.get("value").and_then(Value::as_str) else {
        return false;
    };
    let Some(key) = public_key_from_pem(public_key_pem) else {
        return false;
    };
    let Ok(bytes): Result<[u8; 64], _> = buffer_from_base64(value).try_into() else {
        return false;
    };
    // verify_strict, а не verify: как OpenSSL и BoringSSL, отвергает ключ или R малого
    // порядка (подпись «для любого документа»), а S не из [0, L) отвергают обе функции.
    key.verify_strict(
        canonicalize(manifest).as_bytes(),
        &Signature::from_bytes(&bytes),
    )
    .is_ok()
}

/// Параметры `check`.
#[derive(Debug, Clone, Copy, Default)]
pub struct CheckOptions<'a> {
    /// Документ без подписи, без ключей или с неверной подписью — ошибка, а не итог.
    pub required: bool,
    /// Доверенные ключи (PEM). Пустой список — как пустой `TRUSTED_PUBLIC_KEYS` в 0.4.7.
    pub public_keys: &'a [String],
}

/// Причина, по которой подпись не засчитана.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Unsigned,
    NoTrustedKeys,
    BadSignature,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Unsigned => "unsigned",
            Reason::NoTrustedKeys => "no-trusted-keys",
            Reason::BadSignature => "bad-signature",
        }
    }
}

/// Итог `check`: `{signed, valid, reason}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Check {
    pub signed: bool,
    pub valid: bool,
    pub reason: Option<Reason>,
}

/// `check(manifest, options)`. Подписью считается любое «истинное» значение поля
/// `signature`; документ не-объект (массив, строка, число) — без подписи.
pub fn check(manifest: &Value, options: &CheckOptions) -> Result<Check, SigningError> {
    let Some(signature) = manifest.get(SIGNATURE_FIELD).filter(|s| js::truthy(s)) else {
        if options.required {
            return Err(SigningError::new(
                "Манифест без подписи отклонён: включена обязательная проверка подписи",
            ));
        }
        return Ok(Check {
            signed: false,
            valid: false,
            reason: Some(Reason::Unsigned),
        });
    };

    if options.public_keys.is_empty() {
        if options.required {
            return Err(SigningError::new(
                "Нет доверенных публичных ключей — подпись проверить нечем",
            ));
        }
        return Ok(Check {
            signed: true,
            valid: false,
            reason: Some(Reason::NoTrustedKeys),
        });
    }

    if options
        .public_keys
        .iter()
        .any(|pem| verify(manifest, signature, pem))
    {
        return Ok(Check {
            signed: true,
            valid: true,
            reason: None,
        });
    }

    if options.required {
        return Err(SigningError::new("Подпись манифеста не прошла проверку"));
    }
    Ok(Check {
        signed: true,
        valid: false,
        reason: Some(Reason::BadSignature),
    })
}

/// `attach(manifest, privateKeyPem)`: копия с подписью в конце (прежняя подпись снимается,
/// порядок остальных полей сохраняется).
pub fn attach(
    manifest: &Map<String, Value>,
    private_key_pem: &str,
) -> Result<Map<String, Value>, SigningError> {
    let mut copy = manifest.clone();
    copy.shift_remove(SIGNATURE_FIELD);
    let mut payload = String::new();
    write_canonical_object(&mut payload, &copy);
    copy.insert(
        SIGNATURE_FIELD.to_owned(),
        sign_payload(&payload, private_key_pem)?,
    );
    Ok(copy)
}
