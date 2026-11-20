//! Перенос входа 0.4.7 при запуске (шаг `onlineSession` в `migration-slint.json`, задача 8): до
//! окна и до службы профиля. `safeStorage` этой ОС открывается, только если переносить есть что
//! (`online-session.json` 0.4.7 есть, шаг не сделан, нового входа нет): на Linux чтение пароля
//! связки ключей может показать окно разблокировки KWallet.

use crate::online_profile::os_crypt::Backend;
use crate::online_profile::secure_session::SafeStorage;
use crate::online_profile::session_migration::{
    LEGACY_SESSION_FILE, STEP_ONLINE_SESSION, migrate_online_session,
};
use crate::online_profile::session_store::SessionStore;
use hlsr_core::migration::{MigrationLog, MigrationReport};
use hlsr_core::paths::Paths;
use hlsr_delivery::ports::Logger;
use std::time::SystemTime;

/// `safeStorage` без шифрования (связки нет или переносить нечего): `cipher` не прочитать,
/// `plain` читается.
struct NoSafeStorage;

impl SafeStorage for NoSafeStorage {
    fn is_encryption_available(&self) -> bool {
        false
    }

    fn selected_storage_backend(&self) -> Option<Backend> {
        None
    }

    fn decrypt(&self, _cipher: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

/// Windows: ключ из `Local State` под DPAPI.
#[cfg(windows)]
fn system_safe_storage(paths: &Paths) -> Box<dyn SafeStorage> {
    Box::new(
        crate::online_profile::os_crypt::windows::WindowsSafeStorage::from_data_dir(
            &paths.data_dir,
        ),
    )
}

/// Linux: бэкенд по окружению и пароль связки по шине сеанса (свой поток и среда tokio: перенос
/// идёт до окна из любого контекста). Шины нет — без шифрования.
#[cfg(target_os = "linux")]
fn system_safe_storage(_paths: &Paths) -> Box<dyn SafeStorage> {
    use crate::online_profile::os_crypt::linux::LinuxSafeStorage;
    let detected = std::thread::spawn(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        runtime.block_on(async {
            let connection = zbus::Connection::session().await.ok()?;
            Some(LinuxSafeStorage::detect(&connection).await)
        })
    })
    .join()
    .ok()
    .flatten();
    match detected {
        Some(storage) => Box::new(storage),
        None => Box::new(NoSafeStorage),
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn system_safe_storage(_paths: &Paths) -> Box<dyn SafeStorage> {
    Box::new(NoSafeStorage)
}

/// Перенести вход 0.4.7 в `store` (один раз); ошибки — в журнал (`[migration] onlineSession: …`)
/// и в отчёт.
pub fn migrate_session(
    paths: &Paths,
    store: &SessionStore,
    logger: &dyn Logger,
    now: SystemTime,
) -> MigrationReport {
    if MigrationLog::open(paths).is_done(STEP_ONLINE_SESSION) {
        return MigrationReport {
            skipped: true,
            errors: Vec::new(),
        };
    }
    let legacy = paths.file(LEGACY_SESSION_FILE).exists();
    let storage = if legacy && store.read().is_none() {
        system_safe_storage(paths)
    } else {
        Box::new(NoSafeStorage)
    };
    let report = migrate_online_session(paths, now, storage.as_ref(), store);
    for error in &report.errors {
        logger.warn(&format!("[migration] {STEP_ONLINE_SESSION}: {error}"));
    }
    report
}
