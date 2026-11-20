//! Подпись манифестов и каналов Ed25519 (`manifest-signing.js`): каноническая форма,
//! подпись, проверка, `check` с причинами.
//!
//! Подпись — base64 64 байт в `signature.value`, `algorithm: "ed25519"`; `keyId` не
//! подписывается и при проверке не используется: перебираются все доверенные ключи.
//! Ключи — PEM (SPKI для открытого, PKCS#8 для закрытого), как у `crypto` в Node.
//!
//! Р7 подпроекта 6: сама подпись переехала в [`hlsr_core::manifest_signing`], чтобы `hlsr-update`
//! не зависел от доставки. Здесь — реэкспорт и три функции с прежней ошибкой доставки
//! ([`DeliveryError`] с кодом `MANIFEST_INVALID`): ядро отдаёт только текст ([`SigningError`]).

use crate::errors::{Code, DeliveryError};
use hlsr_core::manifest_signing as core;
use serde_json::{Map, Value};

pub use hlsr_core::manifest_signing::{
    ALGORITHM, Check, CheckOptions, KeyPair, Reason, SIGNATURE_FIELD, SigningError, canonicalize,
    generate_key_pair, key_pair_from_seed, verify,
};

impl From<SigningError> for DeliveryError {
    fn from(error: SigningError) -> DeliveryError {
        DeliveryError::new(Code::ManifestInvalid, error.0)
    }
}

/// `sign(manifest, privateKeyPem)`: `{algorithm, value}`. Ключ, не являющийся закрытым
/// ключом Ed25519, — `MANIFEST_INVALID` (текст причины, в отличие от OpenSSL, свой).
pub fn sign(manifest: &Value, private_key_pem: &str) -> Result<Value, DeliveryError> {
    Ok(core::sign(manifest, private_key_pem)?)
}

/// `check(manifest, options)`. Подписью считается любое «истинное» значение поля
/// `signature`; документ не-объект (массив, строка, число) — без подписи.
pub fn check(manifest: &Value, options: &CheckOptions) -> Result<Check, DeliveryError> {
    Ok(core::check(manifest, options)?)
}

/// `attach(manifest, privateKeyPem)`: копия с подписью в конце (прежняя подпись снимается,
/// порядок остальных полей сохраняется).
pub fn attach(
    manifest: &Map<String, Value>,
    private_key_pem: &str,
) -> Result<Map<String, Value>, DeliveryError> {
    Ok(core::attach(manifest, private_key_pem)?)
}
