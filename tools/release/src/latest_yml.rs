//! `latest.yml` и `latest-linux.yml`: текст, который пишет `build-windows-setup.js:92-113` и
//! `launcher-release-prepare-linux.js:17-20` (карта §2.6). Файл читает electron-updater 0.4.7:
//! `version`, `files[0].url/sha512/size`, `path`, `sha512`, `releaseDate`. YAML собирается
//! строками, а не библиотекой: порядок ключей и кавычки — часть формата, и по хешу этого файла
//! (`latestYmlSha256`) подпись канала ловит подмену.

use crate::error::{At, Result, fail};
use crate::hashes::{self, Digests};
use hlsr_update::channel::MAX_INSTALLER_BYTES;
use std::path::Path;
use std::time::SystemTime;

/// `yamlQuote`: одинарные кавычки, внутренние удваиваются.
pub fn yaml_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Что описывает файл.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Latest<'a> {
    pub version: &'a str,
    /// Имя установщика (`HLSR-Setup-<v>.exe` или `HLSR-Setup-<v>-linux-x86_64.AppImage`).
    pub filename: &'a str,
    pub sha512: &'a str,
    pub size: u64,
    /// ISO-8601 с миллисекундами и `Z` (`new Date().toISOString()`).
    pub release_date: &'a str,
}

/// Текст файла, заканчивается переводом строки.
pub fn render(latest: &Latest<'_>) -> String {
    let Latest {
        version,
        filename,
        sha512,
        size,
        release_date,
    } = latest;
    format!(
        "version: {version}\nfiles:\n  - url: {filename}\n    sha512: {sha512}\n    size: {size}\n\
         path: {filename}\nsha512: {sha512}\nreleaseDate: {}\n",
        yaml_quote(release_date)
    )
}

/// Имя установщика Windows.
pub fn windows_installer_name(version: &str) -> String {
    format!("HLSR-Setup-{version}.exe")
}

/// Имя установщика Linux.
pub fn linux_installer_name(version: &str) -> String {
    format!("HLSR-Setup-{version}-linux-x86_64.AppImage")
}

/// `latest-linux.yml` для готового AppImage: хеши и размер берутся из самого файла. Установщик
/// больше 512 МиБ канал не примет (`MAX_INSTALLER_BYTES`), его не выпускаем.
pub fn write_for_image(
    image: &Path,
    version: &str,
    release_date: Option<&str>,
    out: &Path,
) -> Result<(Digests, String)> {
    let filename = linux_installer_name(version);
    if image.file_name().and_then(|name| name.to_str()) != Some(filename.as_str()) {
        return fail(format!(
            "AppImage называется {}, ожидалось {filename}",
            image.display()
        ));
    }
    let digests = hashes::digests(image)?;
    if digests.size > MAX_INSTALLER_BYTES {
        return fail(format!(
            "AppImage {} байт больше предела {MAX_INSTALLER_BYTES}: канал его не примет",
            digests.size
        ));
    }
    let now;
    let date = match release_date {
        Some(date) => date,
        None => {
            now = hlsr_core::js::iso_string(SystemTime::now());
            &now
        }
    };
    let text = render(&Latest {
        version,
        filename: &filename,
        sha512: &digests.sha512,
        size: digests.size,
        release_date: date,
    });
    std::fs::write(out, &text).at(out)?;
    Ok((digests, hashes::sha256_bytes(text.as_bytes())))
}
