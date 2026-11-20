//! Онлайн лаунчера без интерфейса: то, что делают `telemetry`, `online-profile` и их соседи
//! в 0.4.7 — транспорт API, телеметрия, профиль и вход, уведомления, публичные данные,
//! Discord, мосты чата. Модули повторяют модули 0.4.7 (имя файла JS → `snake_case.rs`,
//! подпапки как в `src/main/`), функции — те же имена в `snake_case`.

pub mod api;
pub mod chat_http;
pub mod discord_rpc;
mod fifo_map;
mod fs_error;
pub mod game_presence;
pub mod image_loader;
mod js_value;
pub mod notifications;
pub mod online_profile;
pub mod profile;
pub mod security;
pub mod seventv;
pub mod shared;
pub mod telemetry;
pub mod twitch_badges;
pub mod twitch_emotes;
pub mod youtube;
