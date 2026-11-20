//! Хеши файлов выпуска: sha256 (hex) и sha512 (base64) потоком по мегабайту.

use crate::error::{At, Result};
use base64::prelude::{BASE64_STANDARD, Engine as _};
use hlsr_core::js::text::hex;
use sha2::{Digest, Sha256, Sha512};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Хеши одного файла.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digests {
    pub size: u64,
    /// Строчный hex, 64 символа.
    pub sha256: String,
    /// base64 с `==`, 88 символов.
    pub sha512: String,
}

pub fn sha256_bytes(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// sha256 и sha512 за один проход.
pub fn digests(path: &Path) -> Result<Digests> {
    let mut file = File::open(path).at(path)?;
    let mut sha256 = Sha256::new();
    let mut sha512 = Sha512::new();
    let mut size = 0u64;
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).at(path)?;
        if read == 0 {
            break;
        }
        sha256.update(&buffer[..read]);
        sha512.update(&buffer[..read]);
        size += read as u64;
    }
    Ok(Digests {
        size,
        sha256: hex(&sha256.finalize()),
        sha512: BASE64_STANDARD.encode(sha512.finalize()),
    })
}
