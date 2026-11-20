//! Локальный канал QA-сборки (`launcher-update/local-qa.js`): адрес ленты и ключ доверия берутся
//! из переменных окружения, но только у сборки с признаком `qa`. В выпуске модуля нет вовсе
//! (`cfg(any(debug_assertions, feature = "dev-hooks"))`, у 0.4.7 webpack заменял условие
//! константой `false`): переменные окружения пользователя не меняют адреса и ключи доверия
//! обновлений. Загрузка установщика с loopback (`download` в 0.4.7) идёт через [`feed_file_url`].
//!
//! ОС канала задаётся явно ([`configuration_for`]): у 0.4.7 файл жил только в Linux-ветке
//! (`latest-linux.yml`), здесь та же настройка годится и Windows (`latest.yml`); без указания
//! берётся ОС сборки ([`configuration`]).

use crate::channel;
use crate::gate::{GateOptions, LauncherUpdateGate};
use crate::http::QaHttpGet;
use crate::ports::Logger;
use hlsr_core::js::Platform;
use std::sync::Arc;
use url::Url;

/// `HLSR_BUILD_CHANNEL`: у QA-сборки — `qa`.
pub const BUILD_CHANNEL_VAR: &str = "HLSR_BUILD_CHANNEL";
/// `HLSR_UPDATE_FEED`: адрес локальной ленты.
pub const FEED_VAR: &str = "HLSR_UPDATE_FEED";
/// `HLSR_UPDATE_CHANNEL_KEY`: открытый ключ канала (PEM, переводы строк — `\n` буквами).
pub const CHANNEL_KEY_VAR: &str = "HLSR_UPDATE_CHANNEL_KEY";

/// `configuration(...)`: шлюз с локальным каналом и адрес ленты.
pub struct LocalQa {
    /// Адрес ленты без хвостовых `/`.
    pub feed: String,
    /// ОС канала: от неё имя `latest*.yml` на ленте и проверяемый установщик.
    pub platform: Platform,
    pub gate: LauncherUpdateGate,
}

/// Имя файла `latest*.yml` на ленте: то же, что в боевых адресах ОС (`launcherFor`:
/// `latest-linux.yml` или `latest.yml`).
pub fn latest_yml_name(platform: Platform) -> String {
    let endpoints = channel::launcher_for(platform);
    let url = &endpoints.latest_yml_urls[0];
    url.rsplit('/').next().unwrap_or_default().to_owned()
}

/// Адрес файла на ленте вместо боевого адреса того же имени (`transport.get` в 0.4.7): подписанные
/// адреса и проверка allowlist остаются боевыми, байты берутся на loopback.
pub fn feed_file_url(feed: &str, signed_url: &str) -> Option<String> {
    let parsed = Url::parse(signed_url).ok()?;
    // `path.posix.basename(pathname)`: хвостовые «/» не считаются.
    let name = parsed
        .path()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");
    let mut encoded = String::new();
    for byte in name.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => {
                encoded.push(byte as char);
            }
            // `encodeURIComponent` кодирует и «%» из уже закодированного пути.
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    Some(format!("{feed}/{encoded}"))
}

/// Конфигурация для ОС сборки ([`configuration_for`] с `Platform::CURRENT`).
pub fn configuration(
    current_version: &str,
    env: &dyn Fn(&str) -> Option<String>,
    logger: Arc<dyn Logger>,
) -> Option<LocalQa> {
    configuration_for(Platform::CURRENT, current_version, env, logger)
}

/// Конфигурация из переменных окружения (`env` — чтение переменной) для канала ОС `platform`.
/// `None` — обычная сборка или адрес ленты не подходит.
pub fn configuration_for(
    platform: Platform,
    current_version: &str,
    env: &dyn Fn(&str) -> Option<String>,
    logger: Arc<dyn Logger>,
) -> Option<LocalQa> {
    if env(BUILD_CHANNEL_VAR).as_deref() != Some("qa") {
        return None;
    }
    let feed_var = env(FEED_VAR).filter(|value| !value.is_empty())?;
    let base = Url::parse(&feed_var).ok()?;
    if base.scheme() != "http"
        || !matches!(base.host_str(), Some("127.0.0.1" | "localhost"))
        || !base.username().is_empty()
        || base.password().is_some_and(|password| !password.is_empty())
        || base.query().is_some_and(|query| !query.is_empty())
        || base.fragment().is_some_and(|fragment| !fragment.is_empty())
    {
        return None;
    }
    let feed = base.as_str().trim_end_matches('/').to_owned();
    let key = env(CHANNEL_KEY_VAR).filter(|value| !value.is_empty());
    logger.info(&format!("[updater] локальный QA-канал: {feed}"));

    let mut options = GateOptions::new(current_version, Arc::new(QaHttpGet::new()));
    options.channel_urls = vec![format!("{feed}/launcher-channel.json")];
    options.latest_yml_urls = vec![format!("{feed}/{}", latest_yml_name(platform))];
    options.platform = platform;
    options.public_keys = match key {
        // Ключ в окружении — одной строкой: переводы строк записаны буквами `\n`.
        Some(key) => vec![key.replace("\\n", "\n")],
        None => channel::default_public_keys(),
    };
    options.logger = logger;
    Some(LocalQa {
        feed,
        platform,
        gate: LauncherUpdateGate::new(options),
    })
}

/// [`configuration_for`] по окружению процесса.
pub fn configuration_from_process(
    platform: Platform,
    current_version: &str,
    logger: Arc<dyn Logger>,
) -> Option<LocalQa> {
    configuration_for(
        platform,
        current_version,
        &|name| std::env::var(name).ok(),
        logger,
    )
}
