//! Шифры сессии входа: AES-CBC с PKCS#7, AES-GCM, HMAC-SHA1 и PBKDF2-HMAC-SHA1 — то, чем
//! Chromium `os_crypt` шифрует `safeStorage` (Linux — AES-128-CBC, ключ PBKDF2 от пароля из связки
//! ключей; Windows — AES-256-GCM, ключ из `Local State`), и чем новое хранилище шифрует файл
//! сессии (AES-256-GCM). Режимы и PBKDF2 написаны здесь поверх блочного шифра `aes` и хеша `sha1`:
//! крейтов `cbc`, `aes-gcm` и `pbkdf2` нет в автономном кеше. Проверка — векторы NIST SP 800-38A,
//! спецификации GCM и RFC 2202/6070 (тесты ниже) и шифротексты настоящего Electron 0.4.7.

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use aes::{Aes128, Aes256};
use sha1::{Digest, Sha1};

/// Ошибка расшифровки: неверный ключ, дополнение или тег — для вызывающего это одно и то же.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("данные не расшифровываются этим ключом")]
pub struct DecryptError;

const BLOCK: usize = 16;

/// Блочный шифр AES-128 или AES-256 по длине ключа.
enum Cipher {
    Aes128(Box<Aes128>),
    Aes256(Box<Aes256>),
}

impl Cipher {
    fn new(key: &[u8]) -> Cipher {
        match key.len() {
            16 => Cipher::Aes128(Box::new(Aes128::new(GenericArray::from_slice(key)))),
            32 => Cipher::Aes256(Box::new(Aes256::new(GenericArray::from_slice(key)))),
            other => panic!("ключ AES: 16 или 32 байта, а не {other}"),
        }
    }

    fn encrypt(&self, block: &mut [u8; BLOCK]) {
        let block = GenericArray::from_mut_slice(block);
        match self {
            Cipher::Aes128(cipher) => cipher.encrypt_block(block),
            Cipher::Aes256(cipher) => cipher.encrypt_block(block),
        }
    }

    fn decrypt(&self, block: &mut [u8; BLOCK]) {
        let block = GenericArray::from_mut_slice(block);
        match self {
            Cipher::Aes128(cipher) => cipher.decrypt_block(block),
            Cipher::Aes256(cipher) => cipher.decrypt_block(block),
        }
    }
}

/// AES-CBC с дополнением PKCS#7; ключ 16 или 32 байта.
pub fn cbc_encrypt(key: &[u8], iv: &[u8; BLOCK], plain: &[u8]) -> Vec<u8> {
    let cipher = Cipher::new(key);
    let pad = BLOCK - plain.len() % BLOCK;
    let mut data = plain.to_vec();
    data.extend(std::iter::repeat_n(pad as u8, pad));
    let mut previous = *iv;
    let (blocks, rest) = data.as_chunks_mut::<BLOCK>();
    debug_assert!(rest.is_empty(), "дополнение кратно блоку");
    for block in blocks {
        for (byte, prev) in block.iter_mut().zip(previous) {
            *byte ^= prev;
        }
        cipher.encrypt(block);
        previous = *block;
    }
    data
}

/// AES-CBC с проверкой дополнения PKCS#7 (как `EVP_DecryptFinal` у Chromium).
pub fn cbc_decrypt(key: &[u8], iv: &[u8; BLOCK], data: &[u8]) -> Result<Vec<u8>, DecryptError> {
    let (blocks, rest) = data.as_chunks::<BLOCK>();
    if blocks.is_empty() || !rest.is_empty() {
        return Err(DecryptError);
    }
    let cipher = Cipher::new(key);
    let mut out = Vec::with_capacity(data.len());
    let mut previous = *iv;
    for chunk in blocks {
        let mut block = *chunk;
        cipher.decrypt(&mut block);
        for (byte, prev) in block.iter_mut().zip(previous) {
            *byte ^= prev;
        }
        out.extend_from_slice(&block);
        previous = *chunk;
    }
    let pad = usize::from(*out.last().ok_or(DecryptError)?);
    if pad == 0
        || pad > BLOCK
        || out[out.len() - pad..]
            .iter()
            .any(|&b| usize::from(b) != pad)
    {
        return Err(DecryptError);
    }
    out.truncate(out.len() - pad);
    Ok(out)
}

/// Умножение в GF(2¹²⁸) GCM (биты от старшего, многочлен x¹²⁸ + x⁷ + x² + x + 1).
fn gf_mul(x: u128, y: u128) -> u128 {
    const R: u128 = 0xE1 << 120;
    let mut z = 0u128;
    let mut v = y;
    for i in 0..128 {
        if (x >> (127 - i)) & 1 == 1 {
            z ^= v;
        }
        v = if v & 1 == 1 { (v >> 1) ^ R } else { v >> 1 };
    }
    z
}

fn ghash(h: u128, aad: &[u8], data: &[u8]) -> u128 {
    let mut y = 0u128;
    for part in [aad, data] {
        for chunk in part.chunks(BLOCK) {
            let mut block = [0u8; BLOCK];
            block[..chunk.len()].copy_from_slice(chunk);
            y = gf_mul(y ^ u128::from_be_bytes(block), h);
        }
    }
    let lengths = ((aad.len() as u128 * 8) << 64) | (data.len() as u128 * 8);
    gf_mul(y ^ lengths, h)
}

/// Счётчик GCM: шифрование `data` на месте, начиная с блока `counter`.
fn gcm_ctr(cipher: &Cipher, nonce: &[u8; 12], counter: u32, data: &mut [u8]) {
    for (index, chunk) in data.chunks_mut(BLOCK).enumerate() {
        let mut block = [0u8; BLOCK];
        block[..12].copy_from_slice(nonce);
        block[12..].copy_from_slice(&(counter.wrapping_add(index as u32)).to_be_bytes());
        cipher.encrypt(&mut block);
        for (byte, key) in chunk.iter_mut().zip(block) {
            *byte ^= key;
        }
    }
}

fn gcm_tag(cipher: &Cipher, nonce: &[u8; 12], aad: &[u8], data: &[u8]) -> [u8; BLOCK] {
    let mut h = [0u8; BLOCK];
    cipher.encrypt(&mut h);
    let s = ghash(u128::from_be_bytes(h), aad, data);
    let mut j0 = [0u8; BLOCK];
    j0[..12].copy_from_slice(nonce);
    j0[15] = 1;
    cipher.encrypt(&mut j0);
    (u128::from_be_bytes(j0) ^ s).to_be_bytes()
}

/// AES-GCM с 96-битным nonce: шифротекст и тег 16 байт в конце.
pub fn gcm_encrypt(key: &[u8], nonce: &[u8; 12], aad: &[u8], plain: &[u8]) -> Vec<u8> {
    let cipher = Cipher::new(key);
    let mut out = plain.to_vec();
    gcm_ctr(&cipher, nonce, 2, &mut out);
    let tag = gcm_tag(&cipher, nonce, aad, &out);
    out.extend_from_slice(&tag);
    out
}

/// AES-GCM: `data` — шифротекст и тег 16 байт. Тег сравнивается за постоянное время.
pub fn gcm_decrypt(
    key: &[u8],
    nonce: &[u8; 12],
    aad: &[u8],
    data: &[u8],
) -> Result<Vec<u8>, DecryptError> {
    if data.len() < BLOCK {
        return Err(DecryptError);
    }
    let (cipher_text, tag) = data.split_at(data.len() - BLOCK);
    let cipher = Cipher::new(key);
    let expected = gcm_tag(&cipher, nonce, aad, cipher_text);
    let diff = expected
        .iter()
        .zip(tag)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    if diff != 0 {
        return Err(DecryptError);
    }
    let mut out = cipher_text.to_vec();
    gcm_ctr(&cipher, nonce, 2, &mut out);
    Ok(out)
}

/// HMAC-SHA1 (RFC 2104).
pub fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..20].copy_from_slice(&Sha1::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha1::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(message);
    let mut outer = Sha1::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// PBKDF2-HMAC-SHA1 (RFC 8018).
pub fn pbkdf2_sha1(password: &[u8], salt: &[u8], iterations: u32, length: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(length);
    let mut index = 1u32;
    while out.len() < length {
        let mut message = salt.to_vec();
        message.extend_from_slice(&index.to_be_bytes());
        let mut u = hmac_sha1(password, &message);
        let mut t = u;
        for _ in 1..iterations {
            u = hmac_sha1(password, &u);
            for (acc, byte) in t.iter_mut().zip(u) {
                *acc ^= byte;
            }
        }
        out.extend_from_slice(&t);
        index += 1;
    }
    out.truncate(length);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn cbc_matches_nist_sp800_38a() {
        // F.2.1 / F.2.2 (AES-128) и F.2.5 (AES-256), первые два блока; PKCS#7 добавляет блок.
        let iv: [u8; 16] = hex("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let plain = hex("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51");
        let cases = [
            (
                "2b7e151628aed2a6abf7158809cf4f3c",
                "7649abac8119b246cee98e9b12e9197d5086cb9b507219ee95db113a917678b2",
            ),
            (
                "603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4",
                "f58c4c04d6e5f1ba779eabfb5f7bfbd69cfc4e967edb808d679f777bc6702c7d",
            ),
        ];
        for (key, cipher) in cases {
            let key = hex(key);
            let encrypted = cbc_encrypt(&key, &iv, &plain);
            assert_eq!(&encrypted[..32], hex(cipher).as_slice());
            assert_eq!(encrypted.len(), 48);
            assert_eq!(cbc_decrypt(&key, &iv, &encrypted).unwrap(), plain);
        }
        let key = hex("2b7e151628aed2a6abf7158809cf4f3c");
        assert_eq!(cbc_decrypt(&key, &iv, &[]), Err(DecryptError));
        assert_eq!(cbc_decrypt(&key, &iv, &[0u8; 15]), Err(DecryptError));
        let mut broken = cbc_encrypt(&key, &iv, b"abc");
        broken[15] ^= 1;
        assert_eq!(cbc_decrypt(&key, &iv, &broken), Err(DecryptError));
        for length in 0..40 {
            let text: Vec<u8> = (0..length as u8).collect();
            assert_eq!(
                cbc_decrypt(&key, &iv, &cbc_encrypt(&key, &iv, &text)).unwrap(),
                text
            );
        }
    }

    #[test]
    fn gcm_matches_specification_vectors() {
        // McGrew–Viega, «The Galois/Counter Mode of Operation», тесты 1–4 (AES-128) и 13–16 (AES-256).
        let p4 = "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39";
        let aad = "feedfacedeadbeeffeedfacedeadbeefabaddad2";
        let cases: [(&str, &str, &str, &str, &str, &str); 6] = [
            (
                "00000000000000000000000000000000",
                "000000000000000000000000",
                "",
                "",
                "",
                "58e2fccefa7e3061367f1d57a4e7455a",
            ),
            (
                "00000000000000000000000000000000",
                "000000000000000000000000",
                "00000000000000000000000000000000",
                "",
                "0388dace60b6a392f328c2b971b2fe78",
                "ab6e47d42cec13bdf53a67b21257bddf",
            ),
            (
                "feffe9928665731c6d6a8f9467308308",
                "cafebabefacedbaddecaf888",
                "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b391aafd255",
                "",
                "42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e091473f5985",
                "4d5c2af327cd64a62cf35abd2ba6fab4",
            ),
            (
                "feffe9928665731c6d6a8f9467308308",
                "cafebabefacedbaddecaf888",
                p4,
                aad,
                "42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e091",
                "5bc94fbc3221a5db94fae95ae7121a47",
            ),
            (
                "0000000000000000000000000000000000000000000000000000000000000000",
                "000000000000000000000000",
                "",
                "",
                "",
                "530f8afbc74536b9a963b4f1c4cb738b",
            ),
            (
                "feffe9928665731c6d6a8f9467308308feffe9928665731c6d6a8f9467308308",
                "cafebabefacedbaddecaf888",
                p4,
                aad,
                "522dc1f099567d07f47f37a32a84427d643a8cdcbfe5c0c97598a2bd2555d1aa8cb08e48590dbb3da7b08b1056828838c5f61e6393ba7a0abcc9f662",
                "76fc6ece0f4e1768cddf8853bb2d551b",
            ),
        ];
        for (key, nonce, plain, aad, cipher, tag) in cases {
            let (key, nonce, plain, aad) = (hex(key), hex(nonce), hex(plain), hex(aad));
            let nonce: [u8; 12] = nonce.try_into().unwrap();
            let mut expected = hex(cipher);
            expected.extend(hex(tag));
            assert_eq!(
                gcm_encrypt(&key, &nonce, &aad, &plain),
                expected,
                "ключ {}",
                key.len()
            );
            assert_eq!(gcm_decrypt(&key, &nonce, &aad, &expected).unwrap(), plain);
            let mut broken = expected.clone();
            *broken.last_mut().unwrap() ^= 1;
            assert_eq!(gcm_decrypt(&key, &nonce, &aad, &broken), Err(DecryptError));
        }
        assert_eq!(
            gcm_decrypt(&[0u8; 32], &[0u8; 12], &[], &[0u8; 15]),
            Err(DecryptError)
        );
    }

    #[test]
    fn hmac_and_pbkdf2_match_rfc_vectors() {
        assert_eq!(
            hmac_sha1(&[0x0b; 20], b"Hi There").to_vec(),
            hex("b617318655057264e28bc0b6fb378c8ef146be00")
        );
        assert_eq!(
            hmac_sha1(b"Jefe", b"what do ya want for nothing?").to_vec(),
            hex("effcdf6ae5eb2fa2d27416d5f184df9c259a7c79")
        );
        assert_eq!(
            hmac_sha1(
                &[0xaa; 80],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )
            .to_vec(),
            hex("aa4ae5e15272d00e95705637ce8a3b55ed402112")
        );
        // RFC 6070.
        assert_eq!(
            pbkdf2_sha1(b"password", b"salt", 1, 20),
            hex("0c60c80f961f0e71f3a9b524af6012062fe037a6")
        );
        assert_eq!(
            pbkdf2_sha1(b"password", b"salt", 2, 20),
            hex("ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957")
        );
        assert_eq!(
            pbkdf2_sha1(b"password", b"salt", 4096, 20),
            hex("4b007901b765489abead49d926f721d065a429c1")
        );
        assert_eq!(
            pbkdf2_sha1(
                b"passwordPASSWORDpassword",
                b"saltSALTsaltSALTsaltSALTsaltSALTsalt",
                4096,
                25
            ),
            hex("3d2eec4fe41c849b80c8d83662c0e44a8b291a964cf2f07038")
        );
    }
}
