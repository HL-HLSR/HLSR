//! Логика интерфейса лаунчера без UI-фреймворка: контроллеры экранов, сервисы окна, движение,
//! слой всплывающего, CPU-растры, строки. Вид на GPUI — крейт `hlsr-gpui`; цикл событий он
//! передаёт логике через [`env::Env`].

pub mod clipboard;
pub mod controllers;
pub mod emoji;
pub mod env;
pub mod fa;
pub mod fields;
pub mod fixtures;
pub mod game_art;
pub mod images;
pub mod intl;
pub mod keyboard_layout;
pub mod keys;
pub mod markdown;
pub mod motion;
pub mod overlay;
pub mod parallax;
mod pixels;
pub mod render;
pub mod richtext;
pub mod safe_svg;
pub mod services;
mod tr;

pub use pixels::{Rgba, Rgba8Image};
pub use tr::tr;
