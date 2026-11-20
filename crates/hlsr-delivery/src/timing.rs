//! Время и повторы доставки: часы (`Date.now()`), запись времени ISO (`new Date().toISOString()`),
//! случайность (`Math.random()`) и пауза между попытками `основа · 2ⁿ⁻¹` с разбросом.
//! Часы и случайность подменяются в тестах: так сверка с 0.4.7 идёт при поддельном времени.

pub use crate::game_download_manager::{Random, system_random};
pub use crate::mirror_runner::{Clock, system_clock};
/// `new Date(ms).toISOString()` — одна на все крейты, в `hlsr-core::js::iso` (годы вне 0000–9999 —
/// со знаком и шестью цифрами, как у V8).
pub use hlsr_core::js::iso::iso_from_ms;

/// Пауза перед следующей попыткой, мс: `base · 2^(attempt − 1)` плюс разброс
/// `floor(random · (base || 1))`. Так считают `ChannelService` и `FilesManifestStore`.
pub fn retry_pause_ms(base_ms: u64, attempt: u32, random: &Random) -> u64 {
    let delay = base_ms.saturating_mul(1u64 << (attempt - 1).min(32));
    let spread = base_ms.max(1) as f64;
    delay.saturating_add((random() * spread).floor() as u64)
}

#[cfg(test)]
mod tests {
    use super::iso_from_ms;

    #[test]
    fn iso_from_ms_like_date_to_iso_string() {
        assert_eq!(iso_from_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_from_ms(-1), "1969-12-31T23:59:59.999Z");
        assert_eq!(iso_from_ms(1_792_152_000_123), "2026-10-16T12:00:00.123Z");
    }
}
