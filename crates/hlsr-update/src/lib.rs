//! Установка и обновление лаунчера без интерфейса (подпроект 6): то, что делают `launcher-update/`,
//! `UpdateManager.js` обеих ОС, `src/updater`, `windows-install/`, `uninstall/`, `src/installer`,
//! `diagnostics/` и `linux/DesktopIntegration.js` в 0.4.7. Модуль Rust на модуль JS (`kebab-case.js`
//! и `CamelCase.js` → `snake_case.rs`), функции — те же имена в `snake_case`. Внешний мир (реестр,
//! ярлыки, процессы, диалоги) — за трейтами в [`ports`] с подделками для тестов.
//!
//! Главное ограничение: обновлялка 0.4.7 уже у пользователей и не меняется, поэтому каждый выпуск
//! 0.5.x обязан пройти её проверки как есть (спецификация, «Приёмка 0.4.7»).

pub mod api;
pub mod boot_gate;
pub mod cancel;
pub mod channel;
pub mod chunks;
pub mod diagnostics;
pub mod errors;
pub mod gate;
pub mod http;
pub mod installer_window;
pub mod legacy_traces;
pub mod linux;
#[cfg(any(debug_assertions, feature = "dev-hooks"))]
pub mod local_qa;
pub mod ports;
pub mod uninstall;
pub mod update_manager;
pub mod updater;
pub mod windows;
