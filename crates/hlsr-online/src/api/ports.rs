//! Порты, которые онлайн отдаёт другим подпроектам и своим каналам: уведомления и события версий
//! для доставки (`Notifier`), телеметрия для событий обновления лаунчера (`TelemetrySink`),
//! профиль и окно для каналов ленты (`NewsProfile`, `NewsLinks`), регистрация протокола Discord.

use super::{BoxFuture, LinkDialog, OnlineWindow, current_locale};
use crate::discord_rpc::discord_rich_presence_bridge::ClientFactory;
use crate::discord_rpc::rpc::ipc::IpcConfig;
use crate::discord_rpc::rpc::register::{ProtocolClient, SystemProtocolClient};
use crate::notifications::version_events::{VersionEvents, VersionOptions};
use crate::online_profile::online_profile_service::{OnlineProfileService, OpenExternal};
use crate::shared::news_locales;
use crate::telemetry::launcher_update_events::TelemetrySink;
use crate::telemetry::news_ipc::{NewsError, NewsLinks, NewsProfile};
use crate::telemetry::telemetry_service::TelemetryService;
use hlsr_core::notifications::NotificationStore;
use hlsr_core::notifications::catalog::level_for_code;
use hlsr_delivery::ports::{Logger, Notifier, VersionEventInfo};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

/// Центр уведомлений для доставки: `notify(code, detail)` фасада 0.4.7
/// (`game-delivery/index.js:382-393`: уровень по каталогу, неизвестный код молча пропускается,
/// сбой записи — строка журнала) и события версий игр (`VersionEvents`).
pub struct OnlineNotifier {
    store: Arc<NotificationStore>,
    versions: Arc<VersionEvents>,
    logger: Arc<dyn Logger>,
}

impl OnlineNotifier {
    pub fn new(
        store: Arc<NotificationStore>,
        versions: Arc<VersionEvents>,
        logger: Arc<dyn Logger>,
    ) -> OnlineNotifier {
        OnlineNotifier {
            store,
            versions,
            logger,
        }
    }
}

fn version_options(info: &VersionEventInfo) -> VersionOptions {
    VersionOptions {
        has_release_notes: info.has_release_notes,
        launcher_id: info.launcher_id.clone(),
    }
}

impl Notifier for OnlineNotifier {
    fn notify(&self, code: &str, detail: &str) {
        let Some(level) = level_for_code(code) else {
            return;
        };
        if let Err(error) = self.store.add(level, code, Some(detail), None) {
            self.logger
                .warn(&format!("[game-delivery] уведомление не записано: {error}"));
        }
    }

    fn resolve_update_offers(
        &self,
        target: &str,
        installed_version: Option<&str>,
        game_id: &str,
    ) -> Result<(), String> {
        self.versions
            .resolve_update_offers(target, installed_version, Some(game_id));
        Ok(())
    }

    fn game_update_available(
        &self,
        version: &str,
        current_version: Option<&str>,
        game_id: &str,
        info: &VersionEventInfo,
    ) -> Result<(), String> {
        self.versions
            .game_update_available(
                Some(version),
                current_version,
                Some(game_id),
                &version_options(info),
            )
            .map(|_| ())
    }

    fn game_installed(
        &self,
        version: &str,
        game_id: &str,
        info: &VersionEventInfo,
    ) -> Result<(), String> {
        self.versions
            .game_installed(Some(version), Some(game_id), &version_options(info))
            .map(|_| ())
    }

    fn game_updated(
        &self,
        from_version: Option<&str>,
        to_version: &str,
        game_id: &str,
        info: &VersionEventInfo,
    ) -> Result<(), String> {
        self.versions
            .game_updated(
                from_version,
                Some(to_version),
                Some(game_id),
                &version_options(info),
            )
            .map(|_| ())
    }
}

/// `telemetry.enabled()` и `telemetry.emit(type, fields)` для `LauncherUpdateEvents`.
impl TelemetrySink for TelemetryService {
    fn enabled(&self) -> bool {
        TelemetryService::enabled(self)
    }

    fn emit(&self, event_type: &str, fields: Value) -> bool {
        TelemetryService::emit(self, event_type, &fields)
    }
}

/// Профиль для каналов ленты: `profile.action(path, body)` и `profile.achieve(id)`.
pub(super) struct NewsProfileLink(pub OnlineProfileService);

impl NewsProfile for NewsProfileLink {
    fn action<'a>(&'a self, path: &'a str, body: Value) -> BoxFuture<'a, Result<Value, NewsError>> {
        let action = self.0.action(path, Some(&body));
        Box::pin(async move {
            action.await.map_err(|error| match error.code {
                Some(code) => NewsError::with_code(code, error.message),
                None => NewsError::new(error.message),
            })
        })
    }

    fn achieve(&self, id: &str) {
        self.0.achieve(id);
    }
}

/// Окно и браузер для ссылок новостей (`confirmLink` и `openExternal` в `AppManager.js:294-302`).
pub(super) struct NewsLinksLink {
    pub window: Arc<dyn OnlineWindow>,
    pub open_external: OpenExternal,
    pub user_data_dir: PathBuf,
}

impl NewsLinksLink {
    /// Вопрос на языке окна (`newsLocales[currentLocale()]`).
    fn dialog(&self, host: &str, url: &str) -> LinkDialog {
        let locale = current_locale(&self.user_data_dir);
        let text = |key: &str| {
            news_locales::get(locale, key)
                .unwrap_or_default()
                .to_owned()
        };
        LinkDialog {
            title: text("#L_HOME_LINK_TITLE"),
            message: format!("{}\n{host}", text("#L_HOME_LINK_TEXT")),
            detail: url.to_owned(),
            cancel: text("#L_HOME_LINK_CANCEL"),
            open: text("#L_HOME_LINK_OPEN"),
        }
    }
}

impl NewsLinks for NewsLinksLink {
    fn confirm_link<'a>(&'a self, host: &'a str, url: &'a str) -> BoxFuture<'a, bool> {
        self.window.confirm_link(self.dialog(host, url))
    }

    fn open_external<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<(), NewsError>> {
        let opened = (self.open_external)(url.to_owned());
        Box::pin(async move { opened.await.map_err(NewsError::new) })
    }
}

/// Можно ли писать обработчик протокола `discord-<id>` в ОС (решение 8: в выпуске — как 0.4.7,
/// каждый вход в Discord). В отладочной сборке и `dev-hooks` — только при
/// `HLSR_DISCORD_REGISTER=1`, чтобы `cargo run` не писал настоящий реестр и `mimeapps.list`.
pub fn discord_register_allowed(dev: bool, variable: Option<&str>) -> bool {
    !dev || variable == Some("1")
}

/// Регистрация протокола Discord с проверкой [`discord_register_allowed`].
pub struct GuardedProtocolClient {
    inner: Arc<dyn ProtocolClient>,
    allowed: bool,
}

impl GuardedProtocolClient {
    pub fn new(inner: Arc<dyn ProtocolClient>, allowed: bool) -> GuardedProtocolClient {
        GuardedProtocolClient { inner, allowed }
    }

    /// Регистрация в ОС; разрешение — по сборке и `HLSR_DISCORD_REGISTER`.
    pub fn system() -> GuardedProtocolClient {
        let dev = cfg!(any(debug_assertions, feature = "dev-hooks"));
        let variable = std::env::var("HLSR_DISCORD_REGISTER").ok();
        GuardedProtocolClient::new(
            Arc::new(SystemProtocolClient::default()),
            discord_register_allowed(dev, variable.as_deref()),
        )
    }
}

impl ProtocolClient for GuardedProtocolClient {
    fn set_as_default_protocol_client(&self, protocol: &str) -> bool {
        self.allowed && self.inner.set_as_default_protocol_client(protocol)
    }
}

/// Настоящий Discord: IPC по адресам ОС и регистрация протокола через
/// [`GuardedProtocolClient::system`].
pub fn system_discord_client() -> ClientFactory {
    ClientFactory::System {
        protocol: Arc::new(GuardedProtocolClient::system()),
        ipc: IpcConfig::system(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord_rpc::rpc::register::FakeProtocolClient;

    #[test]
    fn discord_register_needs_variable_only_in_dev_builds() {
        assert!(discord_register_allowed(false, None));
        assert!(discord_register_allowed(false, Some("0")));
        assert!(!discord_register_allowed(true, None));
        assert!(!discord_register_allowed(true, Some("0")));
        assert!(!discord_register_allowed(true, Some("")));
        assert!(discord_register_allowed(true, Some("1")));
    }

    #[test]
    fn guarded_client_does_not_touch_the_os_when_not_allowed() {
        let fake = Arc::new(FakeProtocolClient::new(true));
        let closed = GuardedProtocolClient::new(fake.clone(), false);
        assert!(!closed.set_as_default_protocol_client("discord-1"));
        assert!(fake.calls().is_empty());
        let open = GuardedProtocolClient::new(fake.clone(), true);
        assert!(open.set_as_default_protocol_client("discord-1"));
        assert_eq!(fake.calls(), ["discord-1"]);
    }
}
