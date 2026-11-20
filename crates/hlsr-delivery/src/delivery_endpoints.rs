//! Адреса каналов и файлов на hlsr.pro и GitHub, список хостов загрузки
//! (`src/shared/delivery-endpoints.js`).

use hlsr_core::js::Platform;
use serde_json::{Value, json};
use url::Url;

pub const PRIMARY_HOST: &str = "hlsr.pro";
pub const FALLBACK_HOST: &str = "github.com";
pub const FALLBACK_ASSET_HOST: &str = "objects.githubusercontent.com";
/// Half-Life 2005 WON пока качается с сайта автора BXT; архив закреплён по
/// SHA-256 в манифесте, поэтому подмена на сервере не пройдёт проверку.
pub const BXT_HOST: &str = "bxt.rs";

pub const PRIMARY_BASE: &str = "https://hlsr.pro/downloads";

pub const LAUNCHER_RELEASES_REPO: &str = "HL-HLSR/HLSR-Releases";
pub const GAME_RELEASES_REPO: &str = "HL-HLSR/Ghosting-Renewed-Releases";
pub const RELEASES_REPO: &str = LAUNCHER_RELEASES_REPO;

pub const GITHUB_RELEASE_BASE: &str = "https://github.com/HL-HLSR/HLSR-Releases/releases";
pub const GAME_GITHUB_RELEASE_BASE: &str =
    "https://github.com/HL-HLSR/Ghosting-Renewed-Releases/releases";

/// Файл больше этого размера в релиз GitHub не кладётся.
pub const GITHUB_ASSET_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// `Number.isFinite(size) && size > 0 && size < 2 ГиБ`; размер не числа (`undefined`,
/// `null`, строка) — не конечное число, то есть `NaN`.
pub fn fits_github_asset(size: f64) -> bool {
    size.is_finite() && size > 0.0 && size < GITHUB_ASSET_MAX_BYTES as f64
}

/// Windows и Linux публикуются одинаково: подписанный `launcher-channel.json`,
/// установщик и `latest.yml` / `latest-linux.yml`. Каталоги и теги разные (`v*` и
/// `linux-v*`), поэтому каналы не пересекаются.
pub const LAUNCHER_CHUNKS_BASE: &str = "https://hlsr.pro/downloads/launcher/chunks";

const CHANNEL_FILE: &str = "launcher-channel.json";

/// Адреса обновления лаунчера одной ОС.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherEndpoints {
    pub platform: Platform,
    pub primary_base: &'static str,
    pub fallback_base: &'static str,
    pub channel_urls: [String; 2],
    pub latest_yml_urls: [String; 2],
    pub feed_urls: [String; 2],
    tag_prefix: &'static str,
}

/// `launcherFor(platform)`: любая ОС, кроме Linux, — Windows.
pub fn launcher_for(platform: Platform) -> LauncherEndpoints {
    let (primary_base, fallback_base, latest_yml, tag_prefix) = match platform {
        Platform::Linux => (
            "https://hlsr.pro/downloads/launcher/linux/stable",
            "https://github.com/HL-HLSR/HLSR-Releases/releases/download/linux-stable",
            "latest-linux.yml",
            "linux-v",
        ),
        Platform::Win32 => (
            "https://hlsr.pro/downloads/launcher/stable",
            "https://github.com/HL-HLSR/HLSR-Releases/releases/latest/download",
            "latest.yml",
            "v",
        ),
    };
    LauncherEndpoints {
        platform,
        primary_base,
        fallback_base,
        channel_urls: [
            format!("{primary_base}/{CHANNEL_FILE}"),
            format!("{fallback_base}/{CHANNEL_FILE}"),
        ],
        latest_yml_urls: [
            format!("{primary_base}/{latest_yml}"),
            format!("{fallback_base}/{latest_yml}"),
        ],
        feed_urls: [primary_base.to_owned(), fallback_base.to_owned()],
        tag_prefix,
    }
}

/// `launcher`: адреса для ОС, под которую собран лаунчер.
pub fn launcher() -> LauncherEndpoints {
    launcher_for(Platform::CURRENT)
}

impl LauncherEndpoints {
    /// `installerUrls(version, installerName)`: сайт и релиз GitHub.
    pub fn installer_urls(&self, version: &str, installer_name: &str) -> [String; 2] {
        [
            format!("{}/{installer_name}", self.primary_base),
            format!(
                "{GITHUB_RELEASE_BASE}/download/{}{version}/{installer_name}",
                self.tag_prefix
            ),
        ]
    }

    /// `releaseTag(version)`: `v0.4.7` или `linux-v0.4.7`.
    pub fn release_tag(&self, version: &str) -> String {
        format!("{}{version}", self.tag_prefix)
    }

    /// Обновление чанками: манифест версии лежит рядом с установщиком, куски — в общем
    /// для ОС хранилище. Только на hlsr.pro: в релиз GitHub нельзя положить тысячи файлов,
    /// оттуда клиент берёт полный установщик.
    pub fn chunk_manifest_urls(&self, filename: &str) -> [String; 1] {
        [format!("{}/{filename}", self.primary_base)]
    }

    pub fn chunk_base_urls(&self) -> [&'static str; 1] {
        [LAUNCHER_CHUNKS_BASE]
    }
}

/// Адреса файлов игры: канал, релиз, источники загрузки.
pub mod game {
    use super::*;

    fn primary_base(game_id: &str) -> String {
        format!("{PRIMARY_BASE}/{game_id}")
    }

    pub fn channel_url(game_id: &str) -> String {
        format!("{}/channel/stable.json", primary_base(game_id))
    }

    pub fn release_tag(game_id: &str, version: &str) -> String {
        format!("game-{game_id}-v{version}")
    }

    pub fn primary_file_url(game_id: &str, version: &str, filename: &str) -> String {
        format!("{}/{version}/{filename}", primary_base(game_id))
    }

    pub fn fallback_file_url(game_id: &str, version: &str, filename: &str) -> String {
        format!(
            "{GAME_GITHUB_RELEASE_BASE}/download/{}/{filename}",
            release_tag(game_id, version)
        )
    }

    pub fn release_repository() -> &'static str {
        GAME_RELEASES_REPO
    }

    /// `providersFor(gameId, version, filename, size)`: основной источник и, если файл
    /// помещается в релиз GitHub (или размер неизвестен — `None`), запасной.
    pub fn providers_for(
        game_id: &str,
        version: &str,
        filename: &str,
        size: Option<f64>,
    ) -> Vec<Value> {
        let mut providers = vec![json!({
            "id": "hlsr-primary",
            "type": "https-direct",
            "status": "permanent",
            "priority": 10,
            "url": primary_file_url(game_id, version, filename),
        })];
        if size.is_none_or(fits_github_asset) {
            providers.push(json!({
                "id": "github-fallback",
                "type": "https-direct",
                "status": "permanent",
                "priority": 20,
                "url": fallback_file_url(game_id, version, filename),
            }));
        }
        providers
    }
}

pub const ALLOWED_DOWNLOAD_HOSTS: &[&str] =
    &[PRIMARY_HOST, FALLBACK_HOST, FALLBACK_ASSET_HOST, BXT_HOST];

pub const FORBIDDEN_DOWNLOAD_HOSTS: &[&str] = &[
    "api.hlsr.pro",
    "disk.yandex.ru",
    "downloader.disk.yandex.ru",
    "storage.yandexcloud.net",
];

/// `isAllowedDownloadUrl(value)`: только `https` и только хосты из списка. Разбор адреса
/// — как у `new URL()` (WHATWG): `hlsr。pro` и `ｈｌｓｒ.pro` — тот же `hlsr.pro`.
/// Значение, которое JS привёл бы к строке (`String(value)`), вызывающий приводит сам.
pub fn is_allowed_download_url(value: &str) -> bool {
    let Ok(parsed) = Url::parse(value) else {
        return false;
    };
    if parsed.scheme() != "https" {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    !FORBIDDEN_DOWNLOAD_HOSTS.contains(&host) && ALLOWED_DOWNLOAD_HOSTS.contains(&host)
}
