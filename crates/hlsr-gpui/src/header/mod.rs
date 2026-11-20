//! Блоки справа в шапке (заглушки S1; S4, S5).

pub mod account;
pub mod notifications;
pub mod search;
pub mod sponsor_button;
pub mod update_card;

// ---- S4: поиск, спонсоры, аккаунт, карточка обновления ----------------------------------------

#[cfg(test)]
mod tests;

use crate::shell::root::ShellContext;
use gpui::App;
use hlsr_core::language::Language;
use hlsr_ui::controllers::bridge::Tr;
use std::rc::Rc;

/// `z-index: 20` шапки: её всплывающие (меню аккаунта, список поиска) — под окнами-диалогами.
const Z_TOPBAR: usize = 20;
/// `z-index: 45` карточки обновления: над шапкой, под окнами (50).
const Z_UPDATE_CARD: usize = 45;
/// Картинки шапки (аватары аккаунта и поиска) в кеше загрузчика.
const SCOPE: &str = "header";

/// Перевод на языке оболочки (`$t`).
fn tr(cx: &App) -> Tr {
    let language = ShellContext::get(cx).map_or(Language::Ru, |shell| shell.shell.language());
    Rc::new(move |key: &str, args: &[&str]| hlsr_ui::tr(language, key, args))
}
