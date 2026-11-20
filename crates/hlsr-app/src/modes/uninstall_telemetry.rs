//! Событие «запущено удаление» (`reportUninstall` в `uninstall/cleanup-cli.js` 0.4.7) — служба
//! телеметрии ПП4 (`hlsr_online::telemetry::report_uninstall`) с токеном входа из хранилища сессии
//! 0.5 и с дожиданием отправки (исправление 0.4.7, решение 6 ПП4). Строки `telemetry: …` идут в
//! отчёт очистки.
//!
//! Отладочная сборка и `dev-hooks` в сеть не ходят, пока не попросили (`HLSR_ONLINE_REAL_API=1`,
//! как у онлайна лаунчера): событие ставится в очередь и не уходит.

use hlsr_online::online_profile::session_store::{SESSION_FILE, SessionStore, detect_vault};
use hlsr_online::telemetry::telemetry_service::{TelemetryOptions, report_uninstall};
use hlsr_online::telemetry::transport::{HttpTransport, Transport};
use hlsr_update::gate::NullLogger;
use hlsr_update::ports::UninstallTelemetry;
use std::path::PathBuf;
use std::sync::Arc;

/// Транспорт телеметрии этого запуска.
fn transport() -> Arc<dyn Transport> {
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    if std::env::var("HLSR_ONLINE_REAL_API").as_deref() != Ok("1") {
        return Arc::new(hlsr_online::telemetry::transport::OfflineTransport);
    }
    Arc::new(HttpTransport::new())
}

/// Телеметрия удаления через службу ПП4.
pub struct OnlineUninstallTelemetry;

impl UninstallTelemetry for OnlineUninstallTelemetry {
    fn report(&self, user_data: &str, launcher_version: &str, say: &mut dyn FnMut(&str)) {
        let dir = PathBuf::from(user_data);
        // Логгер молчит, как `{ info() {}, … }` в 0.4.7.
        let mut options =
            TelemetryOptions::new(&dir, launcher_version, Arc::new(NullLogger), transport());
        let token = SessionStore::at(dir.join(SESSION_FILE), detect_vault())
            .read()
            .map(|session| session.token);
        options.authorization = Some(Arc::new(move || token.clone()));
        options.install_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(PathBuf::from));
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(_) => {
                say("telemetry: пропущено");
                return;
            }
        };
        let mut lines: Vec<String> = Vec::new();
        runtime.block_on(report_uninstall(options, &mut |line: &str| {
            lines.push(line.to_owned())
        }));
        for line in &lines {
            say(line);
        }
    }
}
