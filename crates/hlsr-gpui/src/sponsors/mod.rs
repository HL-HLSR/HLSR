//! Спонсорство (S12): окно «Спонсорство» ([`modal`], открывает [`modal::open_sponsors`]) и
//! «Спасибо за поддержку» ([`thanks`]). Логика — `hlsr_ui::controllers::sponsors`.

pub mod modal;
#[cfg(test)]
mod tests;
pub mod thanks;
