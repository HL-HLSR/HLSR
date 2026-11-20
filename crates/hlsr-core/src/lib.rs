//! Основа лаунчера без интерфейса: пути, хранилище JSON, перенос, журнал, экземпляр.

pub mod args;
pub mod defaults;
pub mod error;
pub mod http;
pub mod instance;
pub mod js;
pub mod json_formats;
pub mod json_store;
pub mod json_text;
pub mod language;
pub mod launcher_prefs;
pub mod logger;
pub mod logging;
pub mod manifest_signing;
pub mod map_assets;
pub mod migration;
pub mod notifications;
pub mod paths;
pub mod redact;
pub mod settings;
pub mod store;
pub mod tasks;
#[cfg(windows)]
pub mod win32;
pub mod window_size;
