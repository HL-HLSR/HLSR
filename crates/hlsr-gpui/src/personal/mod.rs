//! Внешний вид игры на странице «Персонализация» (`GamePersonalization.vue` и его части 0.4.9):
//! редактор (G8), превью (G9), оверлей LiveSplit и моды (G10). Место раздела `personal` и его
//! подразделы в списке слева — у каркаса страницы ([`crate::configs::sections`]).

mod code;
pub mod editor;
mod extras;
mod hud;
pub mod livesplit;
pub mod packs;
pub mod preview;
mod viewport;
