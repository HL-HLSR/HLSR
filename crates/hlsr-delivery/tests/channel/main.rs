//! Канал игры, кеш files.json, проверка обновлений и источник HL1 против 0.4.7: фикстура
//! `channel.json` из `tools/reference/delivery/channel.js` (`ChannelService.js`,
//! `FilesManifestStore.js`, `UpdateChecker.js`, `origin-updates.js` в Node 24 на тех же
//! входах). Чистые функции — на корпусе; сценарии — на локальном сервере того же формата
//! (`support/http.rs`).
//!
//! Время и случайность — поддельные, как у Node: часы сценария двигает шаг `clock`, разброс
//! повтора задан числом 0,5. Канал подписан тестовым ключом из зерна и отдаётся через
//! QA-переопределение (`HLSR_GAME_DELIVERY_CHANNEL_URL/KEY`), источник `local-http` — оба есть
//! только в сборке с отладочными возможностями.
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

#[path = "../support/mod.rs"]
mod support;

mod channel_service;
mod files_store;
mod origin;
mod scenario;
mod update_checker;
mod validate;
