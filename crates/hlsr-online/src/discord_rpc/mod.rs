//! `discord-rpc/`: Discord Rich Presence. Модель (константы, настройка, присутствие, снимок для
//! окна), служба подключения, мост лаунчера и транспорт IPC Discord (`rpc/`: npm `discord-rpc`).

pub mod constants;
pub mod diagnostics_model;
pub mod discord_rich_presence_bridge;
pub mod discord_rich_presence_service;
pub mod preference;
pub mod presence_model;
pub mod rpc;
