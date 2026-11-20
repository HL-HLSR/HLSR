//! `src/main/telemetry/` 0.4.7: транспорт API, согласие, личность установки, маркер сессии,
//! квитанция установки, схема событий, очередь и отправка, служба телеметрии, публичные данные
//! (онлайн, лента новостей, спонсоры).

pub mod config;
pub mod consent;
pub mod crash_marker;
pub mod event_schema;
pub mod identity;
pub mod install_receipt;
pub mod launcher_update_events;
pub mod news_client;
pub mod news_ipc;
pub mod online_client;
pub mod queue;
pub mod sender;
pub mod sponsors_client;
pub mod sponsors_ipc;
pub mod style_presets_client;
pub mod telemetry_service;
pub mod transport;

/// `telemetry/error-map.js`: перенесён с доставкой (её фасад пишет коды в события обновления).
pub use hlsr_delivery::error_map;
