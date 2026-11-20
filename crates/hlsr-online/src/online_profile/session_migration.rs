//! Шаг `onlineSession` в `migration-slint.json`: при первом запуске 0.5 перенести вход 0.4.7 из
//! `online-session.json` (расшифровав через [`SafeStorage`]) в новое хранилище
//! ([`super::session_store`]). Устройство — как у `hlsr_core::migration::local_storage`
//! (`MigrationLog`, `MigrationReport`). Файл 0.4.7 не читается на запись и не удаляется (бета:
//! 0.4.7 и 0.5 делят вход, карта §6.2). Связывание со стартом `hlsr-app` — задача 14.

use super::profile_state::normalize;
use super::secure_session::{SafeStorage, SessionRead, decode_session};
use super::session_store::{Session, SessionStore};
use hlsr_core::migration::{MigrationLog, MigrationReport};
use hlsr_core::paths::Paths;
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;
use std::time::SystemTime;

/// Шаг переноса входа.
pub const STEP_ONLINE_SESSION: &str = "onlineSession";
/// Файл сессии 0.4.7.
pub const LEGACY_SESSION_FILE: &str = "online-session.json";

/// Токен устройства 0.4.7 (`OnlineProfileService.js:18`): `hlsr_<16 hex>_<43 base64url>`.
static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^hlsr_[a-f0-9]{16}_[A-Za-z0-9_-]{43}$").expect("регулярка токена")
});

/// Токен проходит `TOKEN.test`.
pub fn token_valid(token: &str) -> bool {
    TOKEN.is_match(token)
}

/// Сессия из открытого текста конверта: токен по регулярке, профиль через `normalize`
/// (`OnlineProfileService.js:72-76`). `None` — токен или профиль отвергнуты (как у 0.4.7 — входа
/// нет).
pub fn session_from(saved: &Value) -> Option<Session> {
    let token = saved
        .get("token")
        .and_then(Value::as_str)
        .filter(|token| token_valid(token))?;
    let profile = normalize(saved.get("profile").unwrap_or(&Value::Null))?;
    Some(Session {
        token: token.to_owned(),
        profile,
    })
}

/// Перенести вход 0.4.7 в новое хранилище (один раз).
///
/// Шаг сделан → `skipped`. Новое хранилище уже с сессией → не трогаем (свежий вход 0.5). Иначе:
/// `online-session.json` нет → ничего; есть и расшифрован → токен и `normalize`, запись в новое
/// хранилище; не расшифровать/не прочитать → ошибка в шаг. Шаг отмечается всегда.
pub fn migrate_online_session(
    paths: &Paths,
    now: SystemTime,
    storage: &dyn SafeStorage,
    store: &SessionStore,
) -> MigrationReport {
    let mut log = MigrationLog::open(paths);
    if log.is_done(STEP_ONLINE_SESSION) {
        return MigrationReport {
            skipped: true,
            errors: Vec::new(),
        };
    }

    let mut errors = Vec::new();
    if store.read().is_none() {
        match decode_session(&paths.file(LEGACY_SESSION_FILE), storage) {
            SessionRead::Session(saved) => {
                // Токен или профиль не прошли — сессии нет, как у 0.4.7; это не ошибка переноса.
                if let Some(session) = session_from(&saved)
                    && let Err(error) = store.write(&session)
                {
                    errors.push(format!("новое хранилище сессии не записано: {error}"));
                }
            }
            SessionRead::Failed(message) => errors.push(message),
            SessionRead::Missing | SessionRead::NoSession => {}
        }
    }

    // Ошибки переноса остаются в `migration-slint.json` и в `MigrationReport` (их показывает
    // задача 14). Если шаг не отметился — при следующем запуске перенос повторится; это видно в
    // отчёте (как у `localStorage`), а не пропадает молча.
    if !log.record(STEP_ONLINE_SESSION, &errors, now) {
        errors.push(format!(
            "шаг не отмечен в {}, перенос повторится при следующем запуске",
            hlsr_core::migration::MIGRATION_FILE
        ));
    }
    MigrationReport {
        skipped: false,
        errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "hlsr_0123456789abcdef_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCdE";

    #[test]
    fn token_regex_matches_0_4_7() {
        assert!(token_valid(GOOD));
        // 15 hex вместо 16.
        assert!(!token_valid(
            "hlsr_0123456789abcde_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCdE"
        ));
        // Заглавная в hex-части.
        assert!(!token_valid(
            "hlsr_0123456789ABCDEF_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCdE"
        ));
        // Пустой и мусор.
        assert!(!token_valid(""));
        assert!(!token_valid("hlsr_"));
        // Перевод строки в конце не должен проходить якорь.
        assert!(!token_valid(&format!("{GOOD}\n")));
    }

    #[test]
    fn session_from_requires_token_and_profile() {
        use serde_json::json;
        let profile = json!({ "steamId64": "76561198000000111" });
        let saved = json!({ "token": GOOD, "profile": profile });
        assert!(session_from(&saved).is_some());
        // Чужой/битый токен.
        assert!(session_from(&json!({ "token": "nope", "profile": profile })).is_none());
        // Профиль, который normalize отвергает (нет steamId64).
        assert!(session_from(&json!({ "token": GOOD, "profile": { "x": 1 } })).is_none());
        // Нет профиля.
        assert!(session_from(&json!({ "token": GOOD })).is_none());
    }
}
