//! `security/build-key.js`: ключ подписи этой сборки (Ed25519). Запись маски вшивает `build.rs`
//! только в сборку выпуска без `dev-hooks` (переменная `HLSR_ATTEST_SEAL` при сборке); в отладке,
//! QA и тестах её нет, и ключ недоступен. Сервер знает только открытую половину — она уходит в
//! манифест выпуска.

use super::attest_seal;
use ed25519_dalek::{Signer, SigningKey};

/// Подписать нечем (`ATTEST_NO_KEY`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("ATTEST_NO_KEY")]
pub struct NoKey;

/// Ключ подписи для аттестации (`opts.key` 0.4.7: `{available(), sign(message)}`).
pub trait AttestKey: Send + Sync {
    fn available(&self) -> bool;
    /// Подпись Ed25519 (64 байта) над `message`.
    fn sign(&self, message: &[u8]) -> Result<[u8; 64], NoKey>;
}

/// Ключ из записи маски.
pub struct BuildKey {
    key: Option<SigningKey>,
}

impl BuildKey {
    /// `new BuildKey(sealed)`: испорченная или пустая запись — ключа нет.
    pub fn new(sealed: Option<&str>) -> BuildKey {
        BuildKey {
            key: attest_seal::unseal(sealed).map(|seed| SigningKey::from_bytes(&seed)),
        }
    }

    /// `BuildKey.embedded()`: запись, вшитая при сборке выпуска.
    pub fn embedded() -> BuildKey {
        BuildKey::new(Some(env!("HLSR_EMBEDDED_ATTEST_SEAL")))
    }

    /// Открытая половина (32 байта) — для манифеста выпуска и тестов.
    pub fn public_key(&self) -> Option<[u8; 32]> {
        self.key.as_ref().map(|key| key.verifying_key().to_bytes())
    }
}

impl AttestKey for BuildKey {
    fn available(&self) -> bool {
        self.key.is_some()
    }

    fn sign(&self, message: &[u8]) -> Result<[u8; 64], NoKey> {
        let key = self.key.as_ref().ok_or(NoKey)?;
        Ok(key.sign(message).to_bytes())
    }
}
