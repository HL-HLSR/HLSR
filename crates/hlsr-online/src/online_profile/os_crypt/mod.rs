//! Расшифровка `safeStorage` Electron как Chromium `os_crypt` (карта §3.5). Чистое ядро (разбор
//! `Local State`, выбор бэкенда Linux по окружению, расшифровка `v10`/`v11` при известном ключе или
//! пароле) — здесь и проверяется на всех ОС; работа с ОС (DPAPI Windows, связка ключей Linux по
//! D-Bus) — в [`windows`] и [`linux`] под `cfg`. Шифры — [`super::session_crypto`].
//!
//! Electron 44.5.1 = Chromium 152. Windows: ключ AES-256 из `Local State` под DPAPI, `v10` —
//! AES-256-GCM (nonce 12, тег 16). Linux: пароль из связки ключей, ключ PBKDF2(`saltysalt`, 1, 16),
//! `v11` — AES-128-CBC (IV 16×`0x20`), запасные ключи `peanuts` (`v10`) и пустой пароль (crbug
//! 1195256). `basic_text` — шифрования нет.

use super::session_crypto::{cbc_decrypt, gcm_decrypt, pbkdf2_sha1};
use hlsr_core::js::buffer_from_base64;
use serde_json::Value;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(windows)]
pub mod windows;

/// Префикс шифротекста Chromium: версия формата.
pub const PREFIX_V10: &[u8; 3] = b"v10";
pub const PREFIX_V11: &[u8; 3] = b"v11";
/// Соль PBKDF2 Chromium (Linux) и IV AES-CBC (16 пробелов).
pub const SALT: &[u8] = b"saltysalt";
pub const CBC_IV: [u8; 16] = [0x20; 16];
/// Запасной пароль `v10` Chromium (связки ключей нет): «peanuts».
pub const PASSWORD_V10: &[u8] = b"peanuts";

/// Бэкенд Linux, как его называет `safeStorage.getSelectedStorageBackend()` Chromium.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Kwallet6,
    Kwallet5,
    /// KDE 4 и старее: `getSelectedStorageBackend()` → `"kwallet"`.
    Kwallet4,
    GnomeLibsecret,
    /// Связки ключей нет: шифрования нет, 0.4.7 пишет `plain`.
    BasicText,
}

impl Backend {
    /// Строка, которую отдаёт `getSelectedStorageBackend()`.
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Kwallet6 => "kwallet6",
            Backend::Kwallet5 => "kwallet5",
            Backend::Kwallet4 => "kwallet",
            Backend::GnomeLibsecret => "gnome_libsecret",
            Backend::BasicText => "basic_text",
        }
    }
}

/// `encrypted_key` из `Local State` (JSON Chromium) без base64: `None`, если файла/поля нет или
/// это не строка. На Windows дальше снимается префикс `DPAPI` и `CryptUnprotectData`.
pub fn local_state_encrypted_key(local_state: &str) -> Option<Vec<u8>> {
    let json: Value = serde_json::from_str(local_state).ok()?;
    let encoded = json.get("os_crypt")?.get("encrypted_key")?.as_str()?;
    Some(buffer_from_base64(encoded))
}

/// Снять известный префикс версии (`v10`/`v11`); `None` — префикса нет.
fn strip_prefix(cipher: &[u8]) -> Option<(&[u8; 3], &[u8])> {
    let (head, body) = cipher.split_first_chunk::<3>()?;
    if head == PREFIX_V10 || head == PREFIX_V11 {
        Some((head, body))
    } else {
        None
    }
}

/// Расшифровать `v10` ключом AES-256 (Windows): nonce 12 байт после префикса, дальше шифротекст и
/// тег 16 байт. `None` — не тот префикс, короткий ввод или тег не сошёлся.
pub fn decrypt_v10_gcm(key: &[u8; 32], cipher: &[u8]) -> Option<Vec<u8>> {
    let (prefix, body) = strip_prefix(cipher)?;
    if prefix != PREFIX_V10 {
        return None;
    }
    let (nonce, rest) = body.split_first_chunk::<12>()?;
    gcm_decrypt(key, nonce, &[], rest).ok()
}

/// Расшифровать шифротекст Linux паролем из связки ключей. `v11` — ключ от `password`, `v10` — от
/// `peanuts`; при неверном дополнении (crbug 1195256) — повтор с пустым паролем. `None` — нет
/// известного префикса или ни один ключ не подошёл.
pub fn decrypt_linux(password: &[u8], cipher: &[u8]) -> Option<Vec<u8>> {
    let (prefix, body) = strip_prefix(cipher)?;
    let primary: &[u8] = if prefix == PREFIX_V10 {
        PASSWORD_V10
    } else {
        password
    };
    cbc_with_password(primary, body).or_else(|| cbc_with_password(b"", body))
}

/// AES-128-CBC ключом PBKDF2(пароль, `saltysalt`, 1, 16) и IV из 16 пробелов.
fn cbc_with_password(password: &[u8], body: &[u8]) -> Option<Vec<u8>> {
    let key = pbkdf2_sha1(password, SALT, 1, 16);
    cbc_decrypt(&key, &CBC_IV, body).ok()
}

/// Окружение рабочего стола Linux, как его определяет `base::nix::GetDesktopEnvironment` Chromium.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DesktopEnvironment {
    Other,
    Cinnamon,
    Deepin,
    Gnome,
    Kde3,
    Kde4,
    Kde5,
    Kde6,
    Pantheon,
    Ukui,
    Unity,
    Xfce,
}

/// Версия KDE из `KDE_SESSION_VERSION` (6/5/4, иначе — KDE 3, как у Chromium).
fn kde_by_version(get: &impl Fn(&str) -> Option<String>) -> DesktopEnvironment {
    match get("KDE_SESSION_VERSION").as_deref() {
        Some("6") => DesktopEnvironment::Kde6,
        Some("5") => DesktopEnvironment::Kde5,
        Some("4") => DesktopEnvironment::Kde4,
        _ => DesktopEnvironment::Kde3,
    }
}

/// `GetDesktopEnvironment`: сначала токены `XDG_CURRENT_DESKTOP` (через `:`, с обрезкой пробелов),
/// затем `DESKTOP_SESSION`, затем устаревшие признаки GNOME/KDE.
fn desktop_environment(get: &impl Fn(&str) -> Option<String>) -> DesktopEnvironment {
    if let Some(current) = get("XDG_CURRENT_DESKTOP") {
        for token in current.split(':').map(str::trim).filter(|t| !t.is_empty()) {
            let found = match token {
                "Unity" => {
                    let session = get("DESKTOP_SESSION").unwrap_or_default();
                    if session.contains("gnome") && !session.contains("unity") {
                        Some(DesktopEnvironment::Gnome)
                    } else {
                        Some(DesktopEnvironment::Unity)
                    }
                }
                "GNOME" => Some(DesktopEnvironment::Gnome),
                "X-Cinnamon" => Some(DesktopEnvironment::Cinnamon),
                "KDE" => Some(kde_by_version(get)),
                "Pantheon" => Some(DesktopEnvironment::Pantheon),
                "XFCE" => Some(DesktopEnvironment::Xfce),
                "UKUI" => Some(DesktopEnvironment::Ukui),
                "Deepin" => Some(DesktopEnvironment::Deepin),
                _ => None,
            };
            if let Some(env) = found {
                return env;
            }
        }
    }
    if let Some(session) = get("DESKTOP_SESSION") {
        match session.as_str() {
            "deepin" => return DesktopEnvironment::Deepin,
            "gnome" | "mate" => return DesktopEnvironment::Gnome,
            "kde4" => return DesktopEnvironment::Kde4,
            "kde" => {
                return if get("KDE_SESSION_VERSION").is_some() {
                    kde_by_version(get)
                } else {
                    DesktopEnvironment::Kde3
                };
            }
            "ukui" => return DesktopEnvironment::Ukui,
            other if other == "xubuntu" || other.starts_with("xfce") => {
                return DesktopEnvironment::Xfce;
            }
            _ => {}
        }
    }
    if get("GNOME_DESKTOP_SESSION_ID").is_some() {
        return DesktopEnvironment::Gnome;
    }
    if get("KDE_FULL_SESSION").is_some() {
        return kde_by_version(get);
    }
    DesktopEnvironment::Other
}

/// Бэкенд связки ключей по окружению (как `os_crypt::GetBackend` Chromium 152). 0.4.7 не задаёт
/// `--password-store`, поэтому выбор только по окружению. Полная сверка с настоящим Electron Linux —
/// по корпусу `backends` фикстуры `safe-storage.json` (снимается на Linux, отложено).
pub fn select_backend(get: impl Fn(&str) -> Option<String>) -> Backend {
    match desktop_environment(&get) {
        DesktopEnvironment::Kde6 => Backend::Kwallet6,
        DesktopEnvironment::Kde5 => Backend::Kwallet5,
        DesktopEnvironment::Kde4 | DesktopEnvironment::Kde3 => Backend::Kwallet4,
        DesktopEnvironment::Gnome
        | DesktopEnvironment::Cinnamon
        | DesktopEnvironment::Unity
        | DesktopEnvironment::Xfce
        | DesktopEnvironment::Pantheon
        | DesktopEnvironment::Ukui
        | DesktopEnvironment::Deepin => Backend::GnomeLibsecret,
        DesktopEnvironment::Other => Backend::BasicText,
    }
}

#[cfg(test)]
mod tests {
    use super::super::session_crypto::{cbc_encrypt, gcm_encrypt};
    use super::*;
    use serde_json::json;

    const KEY: [u8; 32] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
        0x1e, 0x1f,
    ];

    fn make_v10(key: &[u8; 32], nonce: &[u8; 12], plain: &[u8]) -> Vec<u8> {
        let mut out = PREFIX_V10.to_vec();
        out.extend_from_slice(nonce);
        out.extend(gcm_encrypt(key, nonce, &[], plain));
        out
    }

    fn make_linux(prefix: &[u8; 3], password: &[u8], plain: &[u8]) -> Vec<u8> {
        let key = pbkdf2_sha1(password, SALT, 1, 16);
        let mut out = prefix.to_vec();
        out.extend(cbc_encrypt(&key, &CBC_IV, plain));
        out
    }

    #[test]
    fn v10_round_trips_and_rejects_tampering() {
        let nonce = [7u8; 12];
        let cipher = make_v10(&KEY, &nonce, b"{\"token\":\"x\"}");
        assert_eq!(
            decrypt_v10_gcm(&KEY, &cipher).unwrap(),
            b"{\"token\":\"x\"}"
        );
        // Порча тега.
        let mut tag = cipher.clone();
        *tag.last_mut().unwrap() ^= 1;
        assert_eq!(decrypt_v10_gcm(&KEY, &tag), None);
        // Порча nonce.
        let mut n = cipher.clone();
        n[3] ^= 1;
        assert_eq!(decrypt_v10_gcm(&KEY, &n), None);
        // Чужой ключ.
        let mut other = KEY;
        other[0] ^= 1;
        assert_eq!(decrypt_v10_gcm(&other, &cipher), None);
        // Не тот префикс и короткий ввод.
        assert_eq!(decrypt_v10_gcm(&KEY, b"v11abc"), None);
        assert_eq!(decrypt_v10_gcm(&KEY, b"v1"), None);
        assert_eq!(decrypt_v10_gcm(&KEY, b"v10"), None);
    }

    #[test]
    fn linux_v11_and_fallback_keys() {
        // v11 от пароля связки.
        let cipher = make_linux(PREFIX_V11, b"keyring-pw", b"payload");
        assert_eq!(decrypt_linux(b"keyring-pw", &cipher).unwrap(), b"payload");
        assert_eq!(decrypt_linux(b"wrong", &cipher), None);
        // v10 → peanuts, пароль связки игнорируется.
        let peanuts = make_linux(PREFIX_V10, PASSWORD_V10, b"legacy");
        assert_eq!(decrypt_linux(b"ignored", &peanuts).unwrap(), b"legacy");
        // Пустой пароль (crbug 1195256) как запасной для v11.
        let empty = make_linux(PREFIX_V11, b"", b"empty-pw");
        assert_eq!(decrypt_linux(b"keyring-pw", &empty).unwrap(), b"empty-pw");
        // Нет известного префикса.
        assert_eq!(decrypt_linux(b"pw", b"zzzpayload"), None);
    }

    #[test]
    fn local_state_key_parsing() {
        let blob = b"DPAPI\x01\x02\x03";
        let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, blob);
        let text = json!({ "os_crypt": { "encrypted_key": encoded } }).to_string();
        assert_eq!(local_state_encrypted_key(&text).unwrap(), blob);
        assert_eq!(local_state_encrypted_key("{}"), None);
        assert_eq!(local_state_encrypted_key("{\"os_crypt\":{}}"), None);
        assert_eq!(local_state_encrypted_key("не json"), None);
    }

    #[test]
    fn backend_names_match_chromium() {
        assert_eq!(Backend::Kwallet6.as_str(), "kwallet6");
        assert_eq!(Backend::Kwallet4.as_str(), "kwallet");
        assert_eq!(Backend::GnomeLibsecret.as_str(), "gnome_libsecret");
        assert_eq!(Backend::BasicText.as_str(), "basic_text");
    }

    #[test]
    fn backend_by_environment() {
        let pick = |pairs: &[(&str, &str)]| {
            let map: std::collections::HashMap<String, String> = pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            select_backend(move |name| map.get(name).cloned())
        };
        assert_eq!(
            pick(&[("XDG_CURRENT_DESKTOP", "GNOME")]),
            Backend::GnomeLibsecret
        );
        assert_eq!(
            pick(&[("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")]),
            Backend::GnomeLibsecret
        );
        assert_eq!(
            pick(&[("XDG_CURRENT_DESKTOP", " GNOME ")]),
            Backend::GnomeLibsecret
        );
        assert_eq!(
            pick(&[("XDG_CURRENT_DESKTOP", "X-Cinnamon")]),
            Backend::GnomeLibsecret
        );
        assert_eq!(
            pick(&[("XDG_CURRENT_DESKTOP", "XFCE")]),
            Backend::GnomeLibsecret
        );
        assert_eq!(
            pick(&[("XDG_CURRENT_DESKTOP", "KDE"), ("KDE_SESSION_VERSION", "6")]),
            Backend::Kwallet6
        );
        assert_eq!(
            pick(&[("XDG_CURRENT_DESKTOP", "KDE"), ("KDE_SESSION_VERSION", "5")]),
            Backend::Kwallet5
        );
        assert_eq!(
            pick(&[("XDG_CURRENT_DESKTOP", "KDE"), ("KDE_SESSION_VERSION", "4")]),
            Backend::Kwallet4
        );
        // Game Mode Steam Deck, Hyprland, sway, COSMIC — связки ключей нет.
        assert_eq!(
            pick(&[("XDG_CURRENT_DESKTOP", "Hyprland")]),
            Backend::BasicText
        );
        assert_eq!(
            pick(&[("XDG_CURRENT_DESKTOP", "gamescope")]),
            Backend::BasicText
        );
        assert_eq!(pick(&[]), Backend::BasicText);
        // Запасной путь DESKTOP_SESSION.
        assert_eq!(
            pick(&[("DESKTOP_SESSION", "gnome")]),
            Backend::GnomeLibsecret
        );
        assert_eq!(
            pick(&[("DESKTOP_SESSION", "xubuntu")]),
            Backend::GnomeLibsecret
        );
        assert_eq!(
            pick(&[("GNOME_DESKTOP_SESSION_ID", "this-is-deprecated")]),
            Backend::GnomeLibsecret
        );
    }
}
