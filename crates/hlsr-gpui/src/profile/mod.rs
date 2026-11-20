//! Профиль (S9): окно `ProfileModal.vue` 0.4.9 ([`modal`]) — шапка ([`header`], значки —
//! [`badges`]), вкладка «Профиль» ([`overview`]), бегущая рамка подписчика и 100 % ([`neon`]);
//! панель друзей ([`friends`]). Вкладки «Достижения» ([`achievements`], S10) и «Настройки»
//! ([`runner_settings`], [`style_presets`], S11) — места задач-владельцев: состояние окна им
//! даёт [`ProfileContext`].

pub mod achievements;
pub mod badges;
pub mod friends;
pub mod header;
pub mod modal;
pub mod neon;
pub mod overview;
pub mod runner_settings;
pub mod style_presets;
#[cfg(test)]
mod tests;

pub use modal::{ProfileContext, open_own_settings};
