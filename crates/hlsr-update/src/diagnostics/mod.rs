//! Диагностика (`diagnostics/` 0.4.7): проверки по группам, выгрузка отчёта в ZIP, маскирование
//! личных данных. Модуль Rust на модуль JS:
//!
//! - [`runner`] — `DiagnosticsRunner.js` (`runId` и повторное использование прогона — [`service`],
//!   это `diagnostics-run` из `AppManager.js`);
//! - [`zip`] — `DiagnosticsZip.js` и обработчик `export-diagnostics-zip`;
//! - [`export`] — живая часть `DiagnosticsExport.js` (`buildManifest`, `collect`);
//! - [`log_policy`] — список файлов журнала (`log-policy.js`; ротацию делает `hlsr-core::logging`);
//! - маскирование (`redact.js`) — `hlsr_core::redact` (`redact_text_for`, `safe_env`).
//!
//! Данные других подпроектов (рендерер ПП1, загрузки и сведения о сборке ПП2, Steam и Proton
//! ПП3, профиль, статистика и Discord ПП4) приходят через [`context::DiagnosticsContext`]; сеть
//! и DNS — через [`net`]. Не переносится (решение 19 спецификации): `error-codes.js`,
//! `DiagnosticsExport.pack/exportDiagnostics` и канал `export-diagnostics` — они недостижимы в
//! 0.4.7.

pub mod context;
pub mod export;
pub mod log_policy;
pub mod net;
pub mod runner;
pub mod service;
pub mod zip;
