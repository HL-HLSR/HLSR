//! Разделы окна «Спонсорство» и ключи ссылок (`shared/sponsor-sections.js` 0.4.9): один источник
//! для клиента ([`crate::telemetry::sponsors_client`]) и каналов
//! ([`crate::telemetry::sponsors_ipc`]). Остальные функции модуля (вид окна) — у окна,
//! `hlsr_ui::controllers::sponsors`.

/// `SECTIONS`: разделы ответа `/v2` по порядку окна.
pub const SECTIONS: [&str; 3] = ["active", "donors", "past"];
/// `SUPPORT_KEYS`: ключи `links` сервера и `support-open`.
pub const SUPPORT_KEYS: [&str; 3] = ["subscribe", "donate", "bot"];
