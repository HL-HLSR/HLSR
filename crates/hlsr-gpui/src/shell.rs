//! Оболочка лаунчера: корень ([`root`]: кадр окна, разделы, окна, тосты), шапка ([`topbar`]),
//! кнопки окна ([`window_controls`]), место колонки ([`rail`]).
//!
//! Фон корня — [`background::Background`] (S2); колонка значков — [`rail`], её стекло берёт кадр
//! у фона (`Background::backdrop`).
//!
//! Все размеры — CSS px макета 1600×900 (ступень высоты ≥ 860 из themes.json), в `rems`
//! через [`u`]: зум окна задаёт `rem`. Цвета — токены темы.

pub mod background;
pub mod boot;
pub mod eggs;
pub mod rail;
pub mod root;
#[cfg(test)]
mod tests;
pub mod topbar;
pub mod window_controls;

use gpui::Hsla;

pub(crate) fn alpha(color: Hsla, a: f32) -> Hsla {
    Hsla { a, ..color }
}
