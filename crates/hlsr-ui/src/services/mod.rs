//! Сервисы главного процесса со стороны окна (И2, решение 16): то, что окну 0.4.7 давал preload
//! (`window.hlsrElectron`: `ipcRenderer.invoke/send/sendSync/on`, `store.read/write/quarantine`).
//! Контроллеры экранов получают [`Services`] и о том, кто отвечает, не знают: в лаунчере — фасады
//! подпроектов 2–4 и 6 по каналам ([`ChannelService`]), в `hlsr-snap` и тестах —
//! [`crate::fixtures::FixtureServices`] на JSON сцен `tools/snap/screens`.
//!
//! Фасады отвечают на каналы по имени — метод `call` у `DeliveryApi`, `PlatformApi`, `OnlineApi`
//! и `UpdateApi` (`delivery.rs`, `platform.rs`, `online.rs`, `update.rs`).

pub mod delivery;
pub mod online;
pub mod platform;
pub mod router;
pub mod update;

pub use hlsr_delivery::providers::BoxFuture;
pub use hlsr_online::security::official::Standing;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Ответ `invoke`: значение или отказ (текст ошибки — `reject` обещания в 0.4.7).
pub type Reply = Result<Value, String>;

/// Событие главного процесса окну (`webContents.send(channel, payload)` →
/// `ipcRenderer.on(channel, …)`).
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceEvent {
    pub channel: String,
    pub payload: Value,
}

/// Чтение хранилища окна (`renderer-store-read`: `{found, valid, data}`).
#[derive(Debug, Clone, PartialEq)]
pub enum StoreRead {
    /// Файла нет (`found: false`).
    Missing,
    /// Файл есть, но не разобран (`found: true, valid: false`).
    Invalid,
    /// `data`.
    Found(Value),
}

/// Главный процесс со стороны окна — мост preload 0.4.7.
pub trait MainProcess: Send + Sync {
    /// `ipcRenderer.invoke(channel, ...args)`.
    fn invoke(&self, channel: &str, args: Vec<Value>) -> BoxFuture<'static, Reply>;
    /// `ipcRenderer.send(channel, ...args)`.
    fn send(&self, channel: &str, args: Vec<Value>);
    /// `ipcRenderer.sendSync(channel, ...args)`.
    fn send_sync(&self, channel: &str, args: Vec<Value>) -> Value;
    /// `store.read(configName)`.
    fn store_read(&self, name: &str) -> StoreRead;
    /// `store.write(configName, data)`: записано ли.
    fn store_write(&self, name: &str, data: Value) -> bool;
    /// `store.quarantine(configName)`: `{ok, error}`.
    fn store_quarantine(&self, name: &str) -> Value;
    /// События окну (`ipcRenderer.on`).
    fn subscribe(&self) -> broadcast::Receiver<ServiceEvent>;
    /// Официальна ли сборка сейчас (`OnlineApi::standing`; перемены — событие `build-official`).
    /// Не канал 0.4.9, в журнал вызовов не идёт. Фикстуры и тестовые двойники — официальная
    /// сборка, как 0.4.9; лаунчер отвечает сам ([`router::ChannelRouter::with_build`]).
    fn build(&self) -> Standing {
        Standing::Official
    }
}

/// Сервисы окна.
pub type Services = Arc<dyn MainProcess>;

/// Фасад подпроекта по каналам: `call(channel, args)` фасадов. `None` — канал не его;
/// `Some(Err(текст))` — отклонённый промис.
pub trait ChannelService: Send + Sync {
    fn call<'a>(&'a self, channel: &'a str, args: &'a [Value]) -> BoxFuture<'a, Option<Reply>>;
}
