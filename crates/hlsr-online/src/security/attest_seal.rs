//! `security/attest-seal.js`: запись ключа подписи сборки. Seed Ed25519 (32 байта) хранится как
//! XOR с маской `sha256(соль ‖ «перец» ‖ "hlsr-seal/1")`: `1.<соль>.<перец>.<seed ⊕ маска>` в base64.
//! Не шифрование — способ, чтобы `strings` и поиск по hex в исполняемом файле не находили ключ.
//! Формат общий для сборки выпуска и лаунчера.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hlsr_core::js::buffer_from_base64;
use sha2::{Digest, Sha256};

pub const VERSION: &str = "1";
const LABEL: &[u8] = b"hlsr-seal/1";
pub const SEED_BYTES: usize = 32;
const SALT_BYTES: usize = 16;
const PEPPER_BYTES: usize = 32;

/// `seal(seed)` с seed не из 32 байт.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("seed ключа аттестации: нужно 32 байта")]
pub struct BadSeed;

/// `mask(salt, pepper)`.
pub fn mask(salt: &[u8], pepper: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(salt);
    hash.update(pepper);
    hash.update(LABEL);
    hash.finalize().into()
}

/// `xor(left, right)`: длина — как у `left`.
fn xor(left: &[u8], right: &[u8]) -> Vec<u8> {
    left.iter().zip(right).map(|(a, b)| a ^ b).collect()
}

/// `seal(seed)` с заданными солью и «перцем».
pub fn seal_with(
    seed: &[u8; SEED_BYTES],
    salt: &[u8; SALT_BYTES],
    pepper: &[u8; PEPPER_BYTES],
) -> String {
    [
        VERSION.to_owned(),
        STANDARD.encode(salt),
        STANDARD.encode(pepper),
        STANDARD.encode(xor(seed, &mask(salt, pepper))),
    ]
    .join(".")
}

/// `seal(seed)`: соль и «перец» случайны.
pub fn seal(seed: &[u8]) -> Result<String, BadSeed> {
    let seed: &[u8; SEED_BYTES] = seed.try_into().map_err(|_| BadSeed)?;
    let mut salt = [0u8; SALT_BYTES];
    let mut pepper = [0u8; PEPPER_BYTES];
    getrandom::fill(&mut salt).expect("ОС выдаёт случайные байты");
    getrandom::fill(&mut pepper).expect("ОС выдаёт случайные байты");
    Ok(seal_with(seed, &salt, &pepper))
}

/// `unseal(text)`: seed или `None`, если записи нет (разработка, QA) или она испорчена. base64
/// читается как `Buffer.from(…, "base64")` Node: лишние знаки пропускаются, `=` не обязателен.
pub fn unseal(text: Option<&str>) -> Option<[u8; SEED_BYTES]> {
    let text = text.filter(|text| !text.is_empty())?;
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() != 4 || parts[0] != VERSION {
        return None;
    }
    let salt = buffer_from_base64(parts[1]);
    let pepper = buffer_from_base64(parts[2]);
    let masked = buffer_from_base64(parts[3]);
    if salt.len() != SALT_BYTES || pepper.len() != PEPPER_BYTES || masked.len() != SEED_BYTES {
        return None;
    }
    xor(&masked, &mask(&salt, &pepper)).try_into().ok()
}
