//! Вид лаунчера на GPUI (gpui-pre 0.3.13): основа (тема, масштаб, текст, значки, панели) и
//! экраны. Логика — в `hlsr-ui`; здесь только то, как это выглядит (docs/gpui/00-plan.md).
//!
//! Правила основы: размеры — CSS px макета 1600×900 через [`scale::u`] (rem = 16 × зум окна);
//! цвета и длины — токены [`theme::Theme`]; панели — заливка темы, матовое стекло над кадром —
//! у колонки значков ([`glass`]), как в 0.4.9; в теме Liquid Glass — у окон и панелей ([`liquid`]).

pub mod app;
pub mod assets;
pub mod bridge;
pub mod cache;
pub mod common;
pub mod configs;
pub mod demo;
pub mod dialogs;
pub mod env;
pub mod games;
pub mod glass;
pub mod header;
pub mod home;
pub mod icons;
pub mod image;
pub mod installer;
pub mod keys;
pub mod kit;
pub mod layer;
pub mod legacy;
pub mod liquid;
pub mod live;
pub mod media;
pub mod motion;
pub mod paint;
pub mod personal;
pub mod profile;
pub mod rows;
pub mod scale;
pub mod scroll;
pub mod settings;
pub mod shell;
#[cfg(feature = "dev-hooks")]
pub mod smoke;
pub mod snap;
pub mod sound;
pub mod sponsors;
pub mod text;
pub mod theme;
pub mod updater;
#[cfg(windows)]
pub mod win32;
pub mod window;

/// Шрифты и тема приложения — до первого окна. Корень окна зовёт [`theme::frame`] в начале
/// `render`.
pub fn init(cx: &mut gpui::App) {
    text::load_fonts(cx);
    theme::init(cx);
    kit::init(cx);
    layer::init(cx);
}
