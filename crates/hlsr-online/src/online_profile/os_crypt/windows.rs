//! Windows: ключ AES-256 из `Local State` под DPAPI и расшифровка `safeStorage` (`os_crypt_win`
//! Chromium). `CryptProtectData`/`CryptUnprotectData` привязаны к текущему пользователю Windows:
//! чужой профиль или переустановка — не расшифровать (повторный вход). Те же DPAPI-примитивы
//! шифруют новое хранилище сессии ([`super::super::session_store`]).

use super::super::secure_session::SafeStorage;
use super::{Backend, PREFIX_V10, decrypt_v10_gcm, local_state_encrypted_key};
use std::ffi::c_void;
use std::path::Path;
use std::ptr;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};

/// Префикс ASCII `encrypted_key` на Windows: под ним — DPAPI-блоб ключа.
const DPAPI_PREFIX: &[u8] = b"DPAPI";

/// `CryptProtectData`/`CryptUnprotectData` для текущего пользователя, окна запрещены
/// (`CRYPTPROTECT_UI_FORBIDDEN`). `None` — вызов не удался (чужой профиль, испорченный блоб).
///
/// # Safety
/// `input` живёт на время вызова; выходной буфер освобождается `LocalFree`, как требует DPAPI.
unsafe fn dpapi(input: &[u8], protect: bool) -> Option<Vec<u8>> {
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    // SAFETY: указатели либо на живой срез `input`, либо нулевые; `out_blob` принимает результат.
    let ok = unsafe {
        if protect {
            CryptProtectData(
                &in_blob,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out_blob,
            )
        } else {
            CryptUnprotectData(
                &in_blob,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out_blob,
            )
        }
    };
    if ok == 0 {
        return None;
    }
    // SAFETY: при успехе DPAPI дал буфер `cbData` байт; копируем и отдаём память `LocalFree`.
    let data =
        unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) }.to_vec();
    // SAFETY: буфер выделен `LocalAlloc` внутри DPAPI.
    unsafe { LocalFree(out_blob.pbData as *mut c_void) };
    Some(data)
}

/// Зашифровать DPAPI (новое хранилище сессии).
pub fn dpapi_protect(data: &[u8]) -> Option<Vec<u8>> {
    // SAFETY: см. [`dpapi`].
    unsafe { dpapi(data, true) }
}

/// Расшифровать DPAPI.
pub fn dpapi_unprotect(blob: &[u8]) -> Option<Vec<u8>> {
    // SAFETY: см. [`dpapi`].
    unsafe { dpapi(blob, false) }
}

/// Ключ AES-256 из блоба `encrypted_key`: снять префикс `DPAPI` и `CryptUnprotectData`. Блоб без
/// префикса Chromium отвергает (`os_crypt_win.cc`), ключа тогда нет.
fn key_from_blob(blob: &[u8]) -> Option<[u8; 32]> {
    let stripped = blob.strip_prefix(DPAPI_PREFIX)?;
    dpapi_unprotect(stripped)?.try_into().ok()
}

/// `safeStorage` Windows: ключ из `Local State`, `v10` — AES-256-GCM.
pub struct WindowsSafeStorage {
    key: Option<[u8; 32]>,
}

impl WindowsSafeStorage {
    /// Ключ из текста `Local State`.
    pub fn from_local_state(local_state: &str) -> WindowsSafeStorage {
        let key = local_state_encrypted_key(local_state).and_then(|blob| key_from_blob(&blob));
        WindowsSafeStorage { key }
    }

    /// Ключ из `Local State` в каталоге данных (`%APPDATA%\hlsr`). Файла нет → ключа нет.
    pub fn from_data_dir(data_dir: &Path) -> WindowsSafeStorage {
        let text = std::fs::read_to_string(data_dir.join("Local State")).unwrap_or_default();
        WindowsSafeStorage::from_local_state(&text)
    }

    /// Ключ AES-256 (есть, если `Local State` прочитан и DPAPI снят).
    pub fn key(&self) -> Option<[u8; 32]> {
        self.key
    }
}

impl SafeStorage for WindowsSafeStorage {
    fn is_encryption_available(&self) -> bool {
        // На Windows после `ready` шифрование доступно всегда (Chromium `os_crypt_win`).
        true
    }

    fn selected_storage_backend(&self) -> Option<Backend> {
        // `safeStorage.getSelectedStorageBackend` на Windows не существует (проверено на 44.5.1).
        None
    }

    fn decrypt(&self, cipher: &[u8]) -> Option<Vec<u8>> {
        if cipher.starts_with(PREFIX_V10) {
            decrypt_v10_gcm(&self.key?, cipher)
        } else {
            // Устаревший формат: весь шифротекст — DPAPI-блоб (от 0.4.x не ожидается).
            dpapi_unprotect(cipher)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::session_crypto::gcm_encrypt;
    use super::super::PREFIX_V10;
    use super::*;
    use base64::Engine;
    use serde_json::json;

    fn seed_local_state(key: &[u8; 32]) -> String {
        let protected = dpapi_protect(key).expect("DPAPI защитил ключ");
        let mut blob = DPAPI_PREFIX.to_vec();
        blob.extend_from_slice(&protected);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&blob);
        json!({ "os_crypt": { "encrypted_key": encoded } }).to_string()
    }

    #[test]
    fn dpapi_round_trips_test_data() {
        let data = b"hlsr-test-only payload";
        let protected = dpapi_protect(data).expect("защита");
        assert_ne!(protected, data);
        assert_eq!(dpapi_unprotect(&protected).unwrap(), data);
        // Испорченный блоб не расшифровывается.
        let mut broken = protected.clone();
        broken[10] ^= 0xff;
        broken[20] ^= 0xff;
        assert_eq!(dpapi_unprotect(&broken), None);
    }

    #[test]
    fn windows_safe_storage_reads_key_and_decrypts_v10() {
        let key: [u8; 32] = std::array::from_fn(|i| i as u8);
        let storage = WindowsSafeStorage::from_local_state(&seed_local_state(&key));
        assert_eq!(storage.key(), Some(key));
        assert!(storage.is_encryption_available());
        assert_eq!(storage.selected_storage_backend(), None);

        let nonce = [9u8; 12];
        let mut cipher = PREFIX_V10.to_vec();
        cipher.extend_from_slice(&nonce);
        cipher.extend(gcm_encrypt(&key, &nonce, &[], b"{\"token\":\"ok\"}"));
        assert_eq!(storage.decrypt(&cipher).unwrap(), b"{\"token\":\"ok\"}");

        // Без ключа (нет Local State) — расшифровки нет, но шифрование «доступно».
        let empty = WindowsSafeStorage::from_local_state("{}");
        assert_eq!(empty.key(), None);
        assert!(empty.is_encryption_available());
        assert_eq!(empty.decrypt(&cipher), None);
    }

    /// `encrypted_key` без префикса `DPAPI` Chromium не принимает, даже если сам блоб — настоящий
    /// DPAPI-блоб ключа.
    #[test]
    fn key_blob_without_dpapi_prefix_is_rejected() {
        let key: [u8; 32] = std::array::from_fn(|i| 255 - i as u8);
        let protected = dpapi_protect(&key).expect("DPAPI защитил ключ");
        let encode = |blob: &[u8]| base64::engine::general_purpose::STANDARD.encode(blob);
        let local_state =
            |blob: &[u8]| json!({ "os_crypt": { "encrypted_key": encode(blob) } }).to_string();

        assert_eq!(
            WindowsSafeStorage::from_local_state(&local_state(&protected)).key(),
            None
        );
        let mut prefixed = DPAPI_PREFIX.to_vec();
        prefixed.extend_from_slice(&protected);
        assert_eq!(
            WindowsSafeStorage::from_local_state(&local_state(&prefixed)).key(),
            Some(key)
        );
    }
}
