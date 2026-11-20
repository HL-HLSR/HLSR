//! Сборки Proton (`linux/ProtonManager.js`): закреплённая сборка Proton Wineland HLSR
//! (`wineland-11.0-20261003-hlsr3`, размер, SHA-256 и SHA-512 проверены вручную), её новые
//! релизы на GitHub (один раз за запуск, загрузка — только по кнопке), загрузка и распаковка
//! `tar` в `<данные>/protons`, поиск чужих сборок (`compatibilitytools.d` Steam и Heroic,
//! библиотеки Steam), выбор (`proton-selection.json`) и свои папки игрока.
//!
//! Модуль собирается на всех ОС (пути — по правилам `path` этой ОС, как у Node), но нужен только
//! на Linux. Сеть: [`GithubFetch`] — запросы как у `https` 0.4.7 (только HTTPS и хосты GitHub,
//! до 5 редиректов вручную, простой 30 с), метаданные при TLS-ошибке повторяются через
//! [`net_fallback`](crate::net_fallback).

use crate::linux::abort::{Abort, Shared};
use crate::linux::prefix_command::ProtonError;
use crate::linux::python_runtime::{io_error, make_temp_dir, open_private};
use crate::net_fallback::{self, FallbackGet};
use crate::spawn::{is_executable, process_env};
use crate::vdf;
use hlsr_core::js::collate::{Collator, Sensitivity, default_locale};
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform};
use hlsr_core::json_text::{js_number, json_parse, stringify};
use hlsr_delivery::ports::{Logger, TracingLogger};
use hlsr_delivery::providers::BoxFuture;
use regex::Regex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256, Sha512};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;
use tokio::io::AsyncWriteExt as _;

/// `RELEASE_TAG`: закреплённая сборка.
pub const RELEASE_TAG: &str = "wineland-11.0-20261003-hlsr3";
/// `RELEASE_API`.
pub const RELEASE_API: &str = "https://api.github.com/repos/kanoyo-git/proton-cachyos/releases/tags/wineland-11.0-20261003-hlsr3";
/// `RELEASES_API`: новые сборки выходят релизами того же репозитория. Список читается один раз
/// за запуск: у API GitHub без токена 60 запросов в час на адрес.
pub const RELEASES_API: &str =
    "https://api.github.com/repos/kanoyo-git/proton-cachyos/releases?per_page=20";
/// `MAX_ARCHIVE_BYTES`: 1 ГБ.
pub const MAX_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;
/// `ALLOWED_HOSTS`.
pub const ALLOWED_HOSTS: [&str; 4] = [
    "api.github.com",
    "github.com",
    "release-assets.githubusercontent.com",
    "objects.githubusercontent.com",
];
/// `User-Agent` запросов Proton и Python.
pub const USER_AGENT: &str = "HLSR-Linux";
/// `Accept` запросов Proton.
pub const ACCEPT: &str = "application/vnd.github+json";
/// Предел `readText` по умолчанию: 2 МБ.
pub const READ_TEXT_LIMIT: usize = 2 * 1024 * 1024;
/// Предел `.sha512sum`: 64 КБ.
pub const SUMS_LIMIT: usize = 64 * 1024;
/// Простой соединения (`timeout: 30000`).
pub const REQUEST_TIMEOUT: Duration = Duration::from_millis(30_000);
/// Редиректов не больше 5.
pub const MAX_REDIRECTS: u32 = 5;
/// Вывод `tar -t` не больше 16 МБ.
pub const LISTING_LIMIT: usize = 16 * 1024 * 1024;
/// Источник сборок HLSR в списке.
pub const SOURCE_WINELAND: &str = "HLSR / Proton Wineland";

/// Проверенный архив: имя, размер и хеши (`PINNED_ARCHIVE` и `expected` у `installArchive`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedArchive {
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub sha512: String,
}

impl PinnedArchive {
    pub fn to_json(&self) -> Value {
        json!({"name": self.name, "size": self.size, "sha256": self.sha256, "sha512": self.sha512})
    }
}

/// `PINNED_ARCHIVE`.
pub fn pinned_archive() -> PinnedArchive {
    PinnedArchive {
        name: "proton-wineland-11.0-20261003-hlsr3-x86_64.tar.xz".to_owned(),
        size: 415_958_952,
        sha256: "f93af495521e5201608f2b4cf83d2efe853b29897e34a7462e1272e3ccce7fd5".to_owned(),
        sha512: "abd5828f534f2d786c68a80f5cb1b204d4825e50c6b636edb81f8010fa2da23c09b425b0dbd312b09064df12950124257d9c308fb93d128a11c17ce67771a6db".to_owned(),
    }
}

// Шаблоны 0.4.7. `\d` в JS без флага `u` — только ASCII.
static WINELAND_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^wineland-([0-9]+\.[0-9]+)-([0-9]{8})-hlsr([0-9]+)$").expect("шаблон тега")
});
static WINELAND_DIRECTORY_VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^proton-wineland-([0-9]+\.[0-9]+)-([0-9]{8})-hlsr([0-9]+)-x86_64$")
        .expect("шаблон каталога")
});
static WINELAND_ARCHIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^proton-wineland-[0-9]+\.[0-9]+-[0-9]{8}-hlsr[0-9]+-x86_64\.tar\.xz$")
        .expect("шаблон архива")
});
static WINELAND_DIRECTORY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^proton-wineland-[0-9]+\.[0-9]+-[0-9]{8}-hlsr[0-9]+-x86_64$")
        .expect("шаблон каталога")
});
static UPSTREAM_WINELAND_DIRECTORY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^proton-wineland-[0-9]+\.[0-9]+-[0-9]{8}-x86_64$").expect("шаблон каталога")
});
static SHA512SUM_LINE: LazyLock<Regex> = LazyLock::new(|| {
    // `\s` JS — один класс на весь проект (`hlsr_core::js::WHITESPACE_CLASS`).
    let ws = hlsr_core::js::WHITESPACE_CLASS;
    Regex::new(&format!(r"^([0-9a-f]{{128}}) [ *]?([^{ws}/]+)[{ws}]*$"))
        .expect("шаблон строки .sha512sum")
});
static SHA256_DIGEST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^sha256:([0-9a-f]{64})$").expect("шаблон SHA-256"));

/// `WINELAND_ARCHIVE.test(name)`.
pub fn is_wineland_archive(name: &str) -> bool {
    WINELAND_ARCHIVE.is_match(name)
}

/// `WINELAND_DIRECTORY.test(name)`.
pub fn is_wineland_directory(name: &str) -> bool {
    WINELAND_DIRECTORY.is_match(name)
}

/// `LEGACY_CACHYOS_DIRECTORY.test(name)`: прежняя сборка CachyOS.
pub fn is_legacy_cachyos_directory(name: &str) -> bool {
    name.starts_with("proton-cachyos-")
}

/// `UPSTREAM_WINELAND_DIRECTORY.test(name)`: Wineland без правок HLSR.
pub fn is_upstream_wineland_directory(name: &str) -> bool {
    UPSTREAM_WINELAND_DIRECTORY.is_match(name)
}

/// Перенос строки `/\r?\n/` в `split`: `\r` срезается только перед `\n`.
pub(crate) fn split_lines(text: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = text.split('\n').collect();
    let last = parts.len() - 1;
    for part in &mut parts[..last] {
        *part = part.strip_suffix('\r').unwrap_or(part);
    }
    parts
}

/// `/[\x00-\x1f\x7f]/.test(text)`.
fn has_control(text: &str) -> bool {
    text.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
}

/// `validateArchiveListing(names, verbose, expectedRoot)` с шаблоном каталога сборок HLSR.
pub fn validate_archive_listing(
    names_text: &str,
    verbose_text: &str,
    expected_root: &str,
) -> Result<Vec<String>, ProtonError> {
    validate_archive_listing_with(
        names_text,
        verbose_text,
        expected_root,
        is_wineland_directory,
    )
}

/// `validateArchiveListing(namesText, verboseText, expectedRoot, rootPattern)`: вывод
/// `tar -t` и `tar -tv` описывает только файлы, каталоги и внутренние ссылки под
/// `expectedRoot`. Возвращает имена.
pub fn validate_archive_listing_with(
    names_text: &str,
    verbose_text: &str,
    expected_root: &str,
    root_pattern: impl Fn(&str) -> bool,
) -> Result<Vec<String>, ProtonError> {
    if !root_pattern(expected_root) {
        return Err(ProtonError::new(
            "Некорректный корневой каталог Proton-архива",
        ));
    }
    let names: Vec<&str> = split_lines(names_text)
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect();
    let verbose: Vec<&str> = split_lines(verbose_text)
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect();
    let root_prefix = format!("{expected_root}/");
    let posix = NodePath::new(Platform::Linux, "/");
    let bad_name = |name: &str| {
        posix.is_absolute(name)
            || name.contains('\\')
            || has_control(name)
            || name.contains(" -> ")
            || name.split('/').any(|part| part == "..")
            || (name != expected_root && !name.starts_with(&root_prefix))
    };
    if names.is_empty() || names.iter().any(|name| bad_name(name)) {
        return Err(ProtonError::new("Proton-архив содержит недопустимые пути"));
    }
    if verbose.len() != names.len() {
        return Err(ProtonError::new("Списки файлов Proton-архива не совпадают"));
    }
    for line in &verbose {
        let kind = line.chars().next().unwrap_or_default();
        if !matches!(kind, '-' | 'd' | 'l') {
            return Err(ProtonError::new(
                "Proton-архив содержит специальные файлы или жёсткие ссылки",
            ));
        }
        if kind != 'l' {
            continue;
        }
        let start = line.find(&root_prefix);
        let separator = line.rfind(" -> ");
        let (Some(start), Some(separator)) = (start, separator) else {
            return Err(ProtonError::new(
                "Символическая ссылка Proton-архива не читается",
            ));
        };
        if separator < start {
            return Err(ProtonError::new(
                "Символическая ссылка Proton-архива не читается",
            ));
        }
        let name = &line[start..separator];
        let target = &line[separator + 4..];
        if !names.contains(&name) {
            return Err(ProtonError::new(
                "Символическая ссылка отсутствует в списке Proton-архива",
            ));
        }
        let resolved = posix.normalize(&posix.join(&[&posix.dirname(name), target]));
        if target.is_empty()
            || posix.is_absolute(target)
            || target.contains('\\')
            || has_control(target)
            || (resolved != expected_root && !resolved.starts_with(&root_prefix))
        {
            return Err(ProtonError::new(
                "Символическая ссылка Proton-архива выходит за каталог установки",
            ));
        }
    }
    Ok(names.into_iter().map(str::to_owned).collect())
}

/// Строка значения JSON, если это строка.
fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// `value[key]` истинно в смысле JS.
fn truthy_field(value: &Value, key: &str) -> bool {
    value.get(key).is_some_and(js::truthy)
}

/// `release.assets`, если это массив.
fn assets_of(release: &Value) -> &[Value] {
    release
        .get("assets")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// `safeReleaseAsset(release)`: закреплённый архив в релизе закреплённого тега (тот же размер,
/// а если GitHub сообщает SHA-256 — то же). Возвращает его описание из ответа GitHub.
pub fn safe_release_asset(release: &Value) -> Result<Value, ProtonError> {
    safe_release_asset_with(release, &pinned_archive())
}

/// [`safe_release_asset`] для другой закреплённой сборки (тесты).
pub fn safe_release_asset_with(
    release: &Value,
    pinned: &PinnedArchive,
) -> Result<Value, ProtonError> {
    if release.is_null() || str_field(release, "tag_name") != Some(RELEASE_TAG) {
        return Err(ProtonError::new(
            "GitHub вернул неожиданный релиз Proton Wineland",
        ));
    }
    let archive = assets_of(release)
        .iter()
        .find(|asset| str_field(asset, "name") == Some(&pinned.name));
    let size_matches = |asset: &Value| {
        asset
            .get("size")
            .and_then(Value::as_f64)
            .is_some_and(|size| size == pinned.size as f64)
    };
    let Some(archive) =
        archive.filter(|asset| truthy_field(asset, "browser_download_url") && size_matches(asset))
    else {
        return Err(ProtonError::new(
            "Закреплённый архив Proton Wineland отсутствует или заменён",
        ));
    };
    if truthy_field(archive, "digest") {
        let digest = archive
            .get("digest")
            .map(js::to_string)
            .unwrap_or_default()
            .to_lowercase();
        if digest != format!("sha256:{}", pinned.sha256) {
            return Err(ProtonError::new(
                "GitHub сообщил другой SHA-256 закреплённого Proton-архива",
            ));
        }
    }
    Ok(archive.clone())
}

/// Версия сборки Wineland из тега или имени каталога (`winelandVersion`).
#[derive(Debug, Clone, PartialEq)]
pub struct WinelandVersion {
    pub wine: String,
    pub date: String,
    /// `Number(match[3])`.
    pub build: f64,
    /// Тег, собранный из частей (номер сборки — как записан).
    pub tag: String,
}

impl WinelandVersion {
    pub fn to_json(&self) -> Value {
        json!({"wine": self.wine, "date": self.date, "build": js_number(self.build), "tag": self.tag})
    }
}

/// `winelandVersion(name)`.
pub fn wineland_version(name: &str) -> Option<WinelandVersion> {
    let captures = WINELAND_TAG
        .captures(name)
        .or_else(|| WINELAND_DIRECTORY_VERSION.captures(name))?;
    let wine = captures[1].to_owned();
    let date = captures[2].to_owned();
    let build = &captures[3];
    Some(WinelandVersion {
        tag: format!("wineland-{wine}-{date}-hlsr{build}"),
        build: js::to_number(build),
        wine,
        date,
    })
}

/// `localeCompare(b, undefined, {numeric: true})` для версии Wine `<число>.<число>`: числа
/// сравниваются по значению.
fn compare_numeric(a: &str, b: &str) -> Ordering {
    let parts = |text: &str| -> Vec<String> {
        text.split('.')
            .map(|part| {
                let trimmed = part.trim_start_matches('0');
                trimmed.to_owned()
            })
            .collect()
    };
    let (left, right) = (parts(a), parts(b));
    for (x, y) in left.iter().zip(&right) {
        let order = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
        if order != Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len())
}

/// `compareWineland(a, b)`: по дате сборки, номеру HLSR, версии Wine.
pub fn compare_wineland(a: &WinelandVersion, b: &WinelandVersion) -> Ordering {
    if a.date != b.date {
        return js::cmp_utf16(&a.date, &b.date);
    }
    if a.build != b.build {
        return a.build.partial_cmp(&b.build).unwrap_or(Ordering::Equal);
    }
    compare_numeric(&a.wine, &b.wine)
}

/// Новый релиз (`updateCandidate`): тег версии, архив с тем же именем, SHA-256 от GitHub и
/// `.sha512sum` рядом. Архив и `.sha512sum` — описания из ответа GitHub.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateCandidate {
    pub version: WinelandVersion,
    pub archive: Value,
    pub sums: Value,
    pub sha256: String,
}

impl UpdateCandidate {
    pub fn to_json(&self) -> Value {
        json!({
            "version": self.version.to_json(),
            "archive": self.archive,
            "sums": self.sums,
            "sha256": self.sha256,
        })
    }

    fn archive_name(&self) -> String {
        str_field(&self.archive, "name")
            .unwrap_or_default()
            .to_owned()
    }

    fn archive_size(&self) -> u64 {
        self.archive
            .get("size")
            .and_then(Value::as_f64)
            .map_or(0, |size| size as u64)
    }
}

/// `updateCandidate(release)`: релиз, который можно предложить, или `None`.
pub fn update_candidate(release: &Value) -> Option<UpdateCandidate> {
    if release.is_null() || truthy_field(release, "draft") || truthy_field(release, "prerelease") {
        return None;
    }
    let tag = str_field(release, "tag_name")?;
    let version = wineland_version(tag)?;
    if version.tag != tag {
        return None;
    }
    let name = format!(
        "proton-wineland-{}-{}-hlsr{}-x86_64.tar.xz",
        version.wine,
        version.date,
        js::to_string(&js_number(version.build))
    );
    let sums_name = format!(
        "{}.sha512sum",
        name.strip_suffix(".tar.xz").unwrap_or(&name)
    );
    let assets = assets_of(release);
    let find = |wanted: &str| {
        assets
            .iter()
            .find(|asset| js::truthy(asset) && str_field(asset, "name") == Some(wanted))
    };
    let archive = find(&name)?;
    let sums = find(&sums_name)?;
    if !truthy_field(archive, "browser_download_url") || !truthy_field(sums, "browser_download_url")
    {
        return None;
    }
    let size = archive.get("size").and_then(Value::as_f64)?;
    if !(size.fract() == 0.0 && size.abs() <= 9_007_199_254_740_991.0)
        || size <= 0.0
        || size > MAX_ARCHIVE_BYTES as f64
    {
        return None;
    }
    let digest = archive
        .get("digest")
        .filter(|digest| js::truthy(digest))
        .map(js::to_string)
        .unwrap_or_default()
        .to_lowercase();
    let sha256 = SHA256_DIGEST.captures(&digest)?[1].to_owned();
    Some(UpdateCandidate {
        version,
        archive: archive.clone(),
        sums: sums.clone(),
        sha256,
    })
}

/// `parseSha512sum(text, name)`: SHA-512 архива `name` из строки `<хеш>  <имя>`.
pub fn parse_sha512sum(text: &str, name: &str) -> Result<String, ProtonError> {
    for line in split_lines(text) {
        if let Some(captures) = SHA512SUM_LINE.captures(js::trim(line))
            && &captures[2] == name
        {
            return Ok(captures[1].to_owned());
        }
    }
    Err(ProtonError::new(
        "В .sha512sum нет строки для архива Proton Wineland",
    ))
}

/// `directoryInUse(dir)`: из каталога запущен хоть один процесс (игра, LiveSplit, wineserver) —
/// по `/proc/<pid>/exe` и `/proc/<pid>/cmdline`. Такой каталог удалять и менять нельзя.
pub fn directory_in_use(dir: &Path) -> bool {
    directory_in_use_at(Path::new("/proc"), dir)
}

/// [`directory_in_use`] над каталогом `proc` (в тестах — подделка).
pub fn directory_in_use_at(proc: &Path, dir: &Path) -> bool {
    let path = NodePath::native();
    let prefix = format!("{}{}", path.resolve(&[&dir.to_string_lossy()]), path.sep());
    let Ok(entries) = js::fs::read_dir_names(proc) else {
        return false;
    };
    for pid in entries {
        if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        // Чужой или завершившийся процесс — дальше.
        if let Ok(exe) = std::fs::read_link(proc.join(&pid).join("exe"))
            && exe.to_string_lossy().starts_with(&prefix)
        {
            return true;
        }
        if let Ok(bytes) = std::fs::read(proc.join(&pid).join("cmdline"))
            && js::utf8_string(&bytes)
                .split('\0')
                .any(|arg| arg.starts_with(&prefix))
        {
            return true;
        }
    }
    false
}

/// `isProton(root)`: в каталоге есть исполняемый файл `proton`.
pub fn is_proton(root: &Path) -> bool {
    let executable = root.join("proton");
    is_executable(&executable) && std::fs::metadata(&executable).is_ok_and(|meta| meta.is_file())
}

/// `fs.realpathSync`: путь с развёрнутыми ссылками, в Windows без `\\?\`.
pub fn real_path(path: &Path) -> std::io::Result<PathBuf> {
    let real = std::fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        let text = real.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{rest}")));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(rest));
        }
    }
    Ok(real)
}

/// Программа `tar`. В Windows (только тесты) — системный `bsdtar`: `tar` MSYS из Git Bash не
/// понимает путей `C:\`.
pub fn tar_program() -> String {
    #[cfg(windows)]
    {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
        format!(r"{root}\System32\tar.exe")
    }
    #[cfg(not(windows))]
    {
        "tar".to_owned()
    }
}

// ---------------------------------------------------------------- сеть

/// Правила запроса (`request` в `ProtonManager.js` и `PythonRuntime.js`).
#[derive(Clone, Copy)]
pub struct FetchRules {
    /// Разрешённые хосты (только HTTPS).
    pub hosts: &'static [&'static str],
    /// Заголовок `Accept`.
    pub accept: Option<&'static str>,
    /// Текст отказа недоверенному адресу (и шестому редиректу).
    pub untrusted: &'static str,
    /// Текст тайм-аута.
    pub timeout: &'static str,
    /// Текст редиректа без `Location`; `None` — такой ответ идёт как неверный код.
    pub redirect_without_location: Option<&'static str>,
    /// Текст ответа не 200.
    pub status: fn(u16) -> String,
}

impl std::fmt::Debug for FetchRules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchRules")
            .field("hosts", &self.hosts)
            .finish_non_exhaustive()
    }
}

/// Правила Proton.
pub fn proton_rules() -> FetchRules {
    FetchRules {
        hosts: &ALLOWED_HOSTS,
        accept: Some(ACCEPT),
        untrusted: "Недоверенный адрес загрузки Proton",
        timeout: "Тайм-аут загрузки Proton",
        redirect_without_location: Some("GitHub вернул перенаправление без адреса"),
        status: |code| format!("GitHub ответил {code}"),
    }
}

/// Тело ответа кусками.
pub trait Body: Send {
    /// Следующий кусок; `None` — тело кончилось.
    fn chunk(&mut self) -> BoxFuture<'_, Result<Option<Vec<u8>>, ProtonError>>;
}

/// Тело из готовых кусков (подделки в тестах).
pub struct MemoryBody(std::collections::VecDeque<Vec<u8>>);

impl MemoryBody {
    pub fn new(chunks: impl IntoIterator<Item = Vec<u8>>) -> MemoryBody {
        MemoryBody(chunks.into_iter().collect())
    }
}

impl Body for MemoryBody {
    fn chunk(&mut self) -> BoxFuture<'_, Result<Option<Vec<u8>>, ProtonError>> {
        Box::pin(std::future::ready(Ok(self.0.pop_front())))
    }
}

/// Запрос ответа 200 (`request(url, 0, signal)`): тело читается потом.
pub trait Fetch: Send + Sync {
    fn request<'a>(
        &'a self,
        url: &'a str,
        abort: Option<&'a Abort>,
    ) -> BoxFuture<'a, Result<Box<dyn Body>, ProtonError>>;
}

/// Текст ответа (`readText(url, limit, signal)`); `limit` `None` — 2 МБ.
pub trait ReadText: Send + Sync {
    fn read_text<'a>(
        &'a self,
        url: &'a str,
        limit: Option<usize>,
        abort: Option<&'a Abort>,
    ) -> BoxFuture<'a, Result<String, ProtonError>>;
}

/// Куда на самом деле идёт запрос к проверенному адресу (тесты: локальный сервер по HTTP).
pub type Route = Arc<dyn Fn(&url::Url) -> url::Url + Send + Sync>;

/// Запросы GitHub как у `https.get` 0.4.7: только HTTPS и разрешённые хосты, редиректы вручную
/// (не больше 5, `Location` — от текущего адреса), `User-Agent: HLSR-Linux`, простой 30 с;
/// HTTP/1.1 и TLS — как у доставки (корни Mozilla, без системного прокси).
#[derive(Clone)]
pub struct GithubFetch {
    rules: FetchRules,
    client: reqwest::Client,
    route: Option<Route>,
    timeout: Duration,
}

impl std::fmt::Debug for GithubFetch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GithubFetch")
            .field("rules", &self.rules)
            .finish_non_exhaustive()
    }
}

impl GithubFetch {
    pub fn new(rules: FetchRules) -> GithubFetch {
        let client = hlsr_delivery::http_client::client_builder()
            .build()
            .expect("клиент HTTP собирается из постоянных настроек");
        GithubFetch {
            rules,
            client,
            route: None,
            timeout: REQUEST_TIMEOUT,
        }
    }

    /// Проверенные адреса отправлять на другой сервер (тесты): проверка хоста и редиректов
    /// идёт по исходному адресу.
    pub fn with_route(mut self, route: Route) -> GithubFetch {
        self.route = Some(route);
        self
    }

    /// Простой соединения вместо 30 с (тесты).
    pub fn with_timeout(mut self, timeout: Duration) -> GithubFetch {
        self.timeout = timeout;
        self
    }

    fn untrusted(&self) -> ProtonError {
        ProtonError::new(self.rules.untrusted)
    }

    fn timed_out(&self) -> ProtonError {
        ProtonError::new(self.rules.timeout)
    }

    /// `request(url, redirects, signal)`.
    pub async fn get(&self, url: &str) -> Result<GithubBody, ProtonError> {
        let mut current = url::Url::parse(url).map_err(|_| invalid_url())?;
        let mut redirects = 0;
        loop {
            if current.scheme() != "https"
                || !self.rules.hosts.contains(&current.host_str().unwrap_or(""))
                || redirects > MAX_REDIRECTS
            {
                return Err(self.untrusted());
            }
            let target = match &self.route {
                Some(route) => route(&current),
                None => current.clone(),
            };
            let mut request = self
                .client
                .get(target.clone())
                .header("user-agent", USER_AGENT);
            if let Some(accept) = self.rules.accept {
                request = request.header("accept", accept);
            }
            let response = match tokio::time::timeout(self.timeout, request.send()).await {
                Err(_) => return Err(self.timed_out()),
                Ok(Err(error)) if error.is_timeout() => return Err(self.timed_out()),
                Ok(Err(error)) => return Err(network_error(&error, target.as_str())),
                Ok(Ok(response)) => response,
            };
            let status = response.status().as_u16();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let location = response
                    .headers()
                    .get("location")
                    .map(|value| value.as_bytes().iter().map(|&b| char::from(b)).collect())
                    .filter(|location: &String| !location.is_empty());
                match (location, self.rules.redirect_without_location) {
                    (Some(location), _) => {
                        current = current.join(&location).map_err(|_| invalid_url())?;
                        redirects += 1;
                        continue;
                    }
                    (None, Some(message)) => return Err(ProtonError::new(message)),
                    (None, None) => {}
                }
            }
            if status != 200 {
                return Err(ProtonError::new((self.rules.status)(status)));
            }
            return Ok(GithubBody {
                response,
                timeout: self.timeout,
                timed_out: self.rules.timeout,
            });
        }
    }
}

/// `new URL(text)` не разобрался.
fn invalid_url() -> ProtonError {
    ProtonError::with_code("ERR_INVALID_URL", "Invalid URL")
}

/// Ошибка `reqwest` словами Node: TLS — код OpenSSL, как его отдаёт Node
/// (`UNABLE_TO_GET_ISSUER_CERT_LOCALLY`, `CERT_HAS_EXPIRED`…), ошибки ОС — код `errno`.
pub(crate) fn network_error(error: &reqwest::Error, url: &str) -> ProtonError {
    use std::error::Error as _;
    let mut source: Option<&(dyn std::error::Error + 'static)> = error.source();
    while let Some(current) = source {
        if let Some(tls) = current.downcast_ref::<rustls::Error>() {
            return tls_error(tls);
        }
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            if let Some(tls) = io.get_ref().and_then(|e| e.downcast_ref::<rustls::Error>()) {
                return tls_error(tls);
            }
            if let Some(code) = js::errno::code(io) {
                return ProtonError::with_code(&code, format!("{code}: {io}"));
            }
        }
        if current.to_string() == "dns error" {
            let host = url::Url::parse(url)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
                .unwrap_or_default();
            return ProtonError::with_code("ENOTFOUND", format!("getaddrinfo ENOTFOUND {host}"));
        }
        source = current.source();
    }
    ProtonError::new(error.to_string())
}

/// TLS-ошибка с кодом, как у Node.
fn tls_error(error: &rustls::Error) -> ProtonError {
    use rustls::CertificateError as E;
    let (code, message) = match error {
        rustls::Error::InvalidCertificate(E::UnknownIssuer) => (
            "UNABLE_TO_GET_ISSUER_CERT_LOCALLY",
            "unable to get local issuer certificate".to_owned(),
        ),
        rustls::Error::InvalidCertificate(E::Expired | E::ExpiredContext { .. }) => {
            ("CERT_HAS_EXPIRED", "certificate has expired".to_owned())
        }
        rustls::Error::InvalidCertificate(E::NotValidYet | E::NotValidYetContext { .. }) => (
            "CERT_NOT_YET_VALID",
            "certificate is not yet valid".to_owned(),
        ),
        rustls::Error::InvalidCertificate(
            E::NotValidForName | E::NotValidForNameContext { .. },
        ) => (
            "ERR_TLS_CERT_ALTNAME_INVALID",
            "Hostname/IP does not match certificate's altnames".to_owned(),
        ),
        other => ("EPROTO", format!("TLS: {other}")),
    };
    ProtonError::with_code(code, message)
}

/// Тело ответа GitHub: простой дольше тайм-аута — ошибка.
pub struct GithubBody {
    response: reqwest::Response,
    timeout: Duration,
    timed_out: &'static str,
}

impl Body for GithubBody {
    fn chunk(&mut self) -> BoxFuture<'_, Result<Option<Vec<u8>>, ProtonError>> {
        Box::pin(async move {
            match tokio::time::timeout(self.timeout, self.response.chunk()).await {
                Err(_) => Err(ProtonError::new(self.timed_out)),
                Ok(Err(error)) => Err(ProtonError::with_code("ECONNRESET", error.to_string())),
                Ok(Ok(chunk)) => Ok(chunk.map(|bytes| bytes.to_vec())),
            }
        })
    }
}

impl Fetch for GithubFetch {
    fn request<'a>(
        &'a self,
        url: &'a str,
        abort: Option<&'a Abort>,
    ) -> BoxFuture<'a, Result<Box<dyn Body>, ProtonError>> {
        Box::pin(async move {
            let work = async { Ok(Box::new(self.get(url).await?) as Box<dyn Body>) };
            match abort {
                Some(abort) => tokio::select! {
                    result = work => result,
                    () = abort.aborted() => Err(ProtonError::abort("This operation was aborted")),
                },
                None => work.await,
            }
        })
    }
}

/// Тело целиком, но не больше `limit` (больше — `too_large`). Отмена прерывает чтение.
pub async fn read_limited(
    body: &mut dyn Body,
    limit: usize,
    too_large: &str,
    abort: Option<&Abort>,
) -> Result<Vec<u8>, ProtonError> {
    let work = async {
        let mut bytes = Vec::new();
        while let Some(chunk) = body.chunk().await? {
            if bytes.len() + chunk.len() > limit {
                return Err(ProtonError::new(too_large));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    };
    match abort {
        Some(abort) => tokio::select! {
            result = work => result,
            () = abort.aborted() => Err(ProtonError::abort("This operation was aborted")),
        },
        None => work.await,
    }
}

/// `readText(url, limit, signal)` 0.4.7: текст ответа; метаданные при TLS-ошибке повторяются
/// ([`net_fallback`]) — только для хостов GitHub, итоговый адрес тоже должен быть на них.
pub struct GithubText {
    fetch: Arc<dyn Fetch>,
    fallback: Arc<dyn FallbackGet>,
}

impl GithubText {
    pub fn new(fetch: Arc<dyn Fetch>, fallback: Arc<dyn FallbackGet>) -> GithubText {
        GithubText { fetch, fallback }
    }

    /// Запросы GitHub и повтор с системными корнями.
    pub fn system() -> GithubText {
        GithubText::new(
            Arc::new(GithubFetch::new(proton_rules())),
            net_fallback::system_fallback(),
        )
    }

    async fn read(
        &self,
        url: &str,
        limit: Option<usize>,
        abort: Option<&Abort>,
    ) -> Result<String, ProtonError> {
        let limit = limit.unwrap_or(READ_TEXT_LIMIT);
        let host_allowed = |text: &str| {
            url::Url::parse(text)
                .ok()
                .is_some_and(|url| ALLOWED_HOSTS.contains(&url.host_str().unwrap_or("")))
        };
        let mut body = match self.fetch.request(url, abort).await {
            Ok(body) => body,
            Err(error) => {
                // Метаданные релиза при TLS-ошибке перечитываются; архив сверяется по хешам.
                if error.is_abort()
                    || !net_fallback::is_tls_error(error.code.as_deref(), &error.message)
                    || !host_allowed(url)
                {
                    return Err(error);
                }
                let headers = [("User-Agent", USER_AGENT), ("Accept", ACCEPT)];
                let fallback = self.fallback.get(url, &headers, limit, abort).await?;
                if !host_allowed(&fallback.url) {
                    return Err(ProtonError::new("Недоверенный адрес загрузки Proton"));
                }
                if fallback.truncated {
                    return Err(ProtonError::new("Ответ GitHub слишком велик"));
                }
                if fallback.status != 200 {
                    return Err(ProtonError::new(format!(
                        "GitHub ответил {}",
                        fallback.status
                    )));
                }
                return Ok(js::utf8_string(&fallback.body.unwrap_or_default()));
            }
        };
        let bytes = read_limited(body.as_mut(), limit, "Ответ GitHub слишком велик", abort).await?;
        Ok(js::utf8_string(&bytes))
    }
}

impl ReadText for GithubText {
    fn read_text<'a>(
        &'a self,
        url: &'a str,
        limit: Option<usize>,
        abort: Option<&'a Abort>,
    ) -> BoxFuture<'a, Result<String, ProtonError>> {
        Box::pin(self.read(url, limit, abort))
    }
}

// ---------------------------------------------------------------- tar

/// `commandOutput(file, args, signal)`: вывод `tar <args> <file>`, не больше 16 МБ.
async fn command_output(file: &str, args: &[&str], abort: &Abort) -> Result<String, ProtonError> {
    let mut command = tokio::process::Command::new(tar_program());
    command
        .args(args)
        .arg(file)
        .env_clear()
        .envs(process_env())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let unreadable = || ProtonError::new("Список файлов Proton-архива не читается");
    let work = async {
        let mut child = command
            .spawn()
            .map_err(|error| ProtonError::from(crate::spawn::spawn_error("tar", &error)))?;
        let mut stdout = child.stdout.take().expect("канал вывода");
        let mut stderr = child.stderr.take().expect("канал ошибок");
        let drain = tokio::spawn(async move {
            let mut sink = Vec::new();
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut sink).await;
        });
        let mut output = Vec::new();
        let mut buffer = vec![0u8; 64 * 1024];
        let mut failed = false;
        loop {
            match tokio::io::AsyncReadExt::read(&mut stdout, &mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    output.extend_from_slice(&buffer[..read]);
                    if output.len() > LISTING_LIMIT {
                        failed = true;
                        let _ = child.start_kill();
                        break;
                    }
                }
            }
        }
        let status = child.wait().await;
        let _ = drain.await;
        match status {
            Ok(status) if status.success() && !failed => Ok(js::utf8_string(&output)),
            _ => Err(unreadable()),
        }
    };
    tokio::select! {
        result = work => result,
        () = abort.aborted() => Err(ProtonError::abort("This operation was aborted")),
    }
}

/// `listArchive(file, expectedRoot, signal)`: `tar -tJf` и `tar -tvJf` параллельно, затем
/// [`validate_archive_listing`].
pub async fn list_archive(
    file: &Path,
    expected_root: &str,
    abort: &Abort,
) -> Result<Vec<String>, ProtonError> {
    let file = file.to_string_lossy();
    let (names, verbose) = tokio::join!(
        command_output(&file, &["-tJf"], abort),
        command_output(&file, &["-tvJf"], abort),
    );
    validate_archive_listing(&names?, &verbose?, expected_root)
}

/// `run("tar", args, signal)`: код не 0 — `tar: <ошибки или код>`.
async fn run_tar(args: &[String], abort: &Abort) -> Result<(), ProtonError> {
    let mut command = tokio::process::Command::new(tar_program());
    command
        .args(args)
        .env_clear()
        .envs(process_env())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let work = async {
        let mut child = command
            .spawn()
            .map_err(|error| ProtonError::from(crate::spawn::spawn_error("tar", &error)))?;
        let mut stderr = child.stderr.take().expect("канал ошибок");
        let mut text = String::new();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            match tokio::io::AsyncReadExt::read(&mut stderr, &mut buffer).await {
                Ok(0) | Err(_) => break,
                // Каждый кусок — не больше 2048 знаков (`chunk.toString().slice(0, 2048)`).
                Ok(read) => {
                    text.push_str(&js::slice_utf16(&js::utf8_string(&buffer[..read]), 2048))
                }
            }
        }
        let status = child.wait().await.map_err(io_error)?;
        if status.success() {
            return Ok(());
        }
        let detail = js::trim(&text);
        let detail = if detail.is_empty() {
            status
                .code()
                .map_or_else(|| "null".to_owned(), |code| code.to_string())
        } else {
            detail.to_owned()
        };
        Err(ProtonError::new(format!("tar: {detail}")))
    };
    tokio::select! {
        result = work => result,
        () = abort.aborted() => Err(ProtonError::abort("This operation was aborted")),
    }
}

// ---------------------------------------------------------------- менеджер

/// Сборка Proton в списке (`discover()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtonEntry {
    pub id: String,
    /// Имя папки, как её нашёл поиск (не после разворота ссылок).
    pub name: String,
    /// Путь после разворота ссылок.
    pub path: String,
    pub source: String,
    /// Сборка HLSR в `<данные>/protons`.
    pub recommended: bool,
}

impl ProtonEntry {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "path": self.path,
            "source": self.source,
            "recommended": self.recommended,
        })
    }
}

/// Своя сборка в `<данные>/protons` (`managedWineland()`).
#[derive(Debug, Clone, PartialEq)]
pub struct ManagedWineland {
    pub name: String,
    pub path: String,
    pub version: WinelandVersion,
}

impl ManagedWineland {
    pub fn to_json(&self) -> Value {
        json!({"name": self.name, "path": self.path, "version": self.version.to_json()})
    }
}

/// `proton-selection.json` (`selectionData()`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionData {
    pub path: Option<String>,
    pub custom_paths: Vec<String>,
}

impl SelectionData {
    pub fn to_json(&self) -> Value {
        json!({"path": self.path, "customPaths": self.custom_paths})
    }
}

/// Состояние загрузки (`this.state`).
#[derive(Debug, Clone, PartialEq)]
pub struct ManagerState {
    /// `idle`, `checking`, `downloading`, `extracting`, `ready`, `error`.
    pub phase: String,
    pub percent: f64,
    pub error: Option<String>,
    /// `install` или `update`.
    pub operation: Option<String>,
}

impl Default for ManagerState {
    fn default() -> Self {
        ManagerState {
            phase: "idle".to_owned(),
            percent: 0.0,
            error: None,
            operation: None,
        }
    }
}

/// Проверка обновления (`this.update`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateState {
    pub checked: bool,
    pub available: bool,
    pub latest: Option<String>,
    pub updated_from: Option<String>,
    pub updated_to: Option<String>,
    pub error: Option<String>,
}

impl UpdateState {
    pub fn to_json(&self) -> Value {
        json!({
            "checked": self.checked,
            "available": self.available,
            "latest": self.latest,
            "updatedFrom": self.updated_from,
            "updatedTo": self.updated_to,
            "error": self.error,
        })
    }
}

/// Событие менеджера: `state` (полный `status()`), `install-started`, `installed`,
/// `update-available`, `updated`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProtonEvent {
    pub name: &'static str,
    pub payload: Value,
}

/// Подписчик событий.
pub type ProtonListener = Arc<dyn Fn(&ProtonEvent) + Send + Sync>;
/// Подписка: для отписки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenerId(u64);

/// Занят ли каталог сборки (`inUse`).
pub type InUse = Arc<dyn Fn(&Path) -> bool + Send + Sync>;

/// Параметры конструктора (`options`).
#[derive(Clone, Default)]
pub struct ProtonManagerOptions {
    /// `dataDir`: папка данных лаунчера.
    pub data_dir: PathBuf,
    /// `home`; по умолчанию — домашняя папка.
    pub home: Option<PathBuf>,
    pub logger: Option<Arc<dyn Logger>>,
    /// `readText`; по умолчанию — [`GithubText::system`].
    pub read_text: Option<Arc<dyn ReadText>>,
    /// `request` (загрузка архива); по умолчанию — [`GithubFetch`] с правилами Proton.
    pub request: Option<Arc<dyn Fetch>>,
    /// `inUse`; по умолчанию — [`directory_in_use`].
    pub in_use: Option<InUse>,
    /// Закреплённая сборка; по умолчанию — [`pinned_archive`] (тесты подставляют маленький архив).
    pub pinned: Option<PinnedArchive>,
}

/// Итог загрузки: установленная сборка или `None` (отменено, нечего ставить).
pub type InstallResult = Result<Option<ProtonEntry>, ProtonError>;

struct Inner {
    data_dir: String,
    home: String,
    root: String,
    selection_file: String,
    logger: Arc<dyn Logger>,
    read_text: Arc<dyn ReadText>,
    request: Arc<dyn Fetch>,
    in_use: InUse,
    pinned: PinnedArchive,
    path: NodePath,
    state: Mutex<ManagerState>,
    update: Mutex<UpdateState>,
    candidate: Mutex<Option<UpdateCandidate>>,
    pending: Mutex<Option<Shared<InstallResult>>>,
    controller: Mutex<Option<Abort>>,
    update_check: Mutex<Option<Shared<Option<UpdateCandidate>>>>,
    listeners: Mutex<Vec<(u64, ProtonListener)>>,
    next_listener: AtomicU64,
}

/// `ProtonManager`. Копии — тот же менеджер.
#[derive(Clone)]
pub struct ProtonManager {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ProtonManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtonManager")
            .field("root", &self.inner.root)
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Отмена на любой стадии: итог `None`, без ошибки.
enum Step<T> {
    Done(T),
    Cancelled,
}

impl ProtonManager {
    pub fn new(options: ProtonManagerOptions) -> ProtonManager {
        let path = NodePath::native();
        let data_dir = options.data_dir.to_string_lossy().into_owned();
        let home = options
            .home
            .or_else(std::env::home_dir)
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let root = path.join(&[&data_dir, "protons"]);
        let selection_file = path.join(&[&data_dir, "proton-selection.json"]);
        ProtonManager {
            inner: Arc::new(Inner {
                logger: options.logger.unwrap_or_else(|| Arc::new(TracingLogger)),
                read_text: options
                    .read_text
                    .unwrap_or_else(|| Arc::new(GithubText::system())),
                request: options
                    .request
                    .unwrap_or_else(|| Arc::new(GithubFetch::new(proton_rules()))),
                in_use: options.in_use.unwrap_or_else(|| Arc::new(directory_in_use)),
                pinned: options.pinned.unwrap_or_else(pinned_archive),
                data_dir,
                home,
                root,
                selection_file,
                path,
                state: Mutex::new(ManagerState::default()),
                update: Mutex::new(UpdateState::default()),
                candidate: Mutex::new(None),
                pending: Mutex::new(None),
                controller: Mutex::new(None),
                update_check: Mutex::new(None),
                listeners: Mutex::new(Vec::new()),
                next_listener: AtomicU64::new(1),
            }),
        }
    }

    /// `<данные>/protons`.
    pub fn root(&self) -> &str {
        &self.inner.root
    }

    /// `<данные>/proton-selection.json`.
    pub fn selection_file(&self) -> &str {
        &self.inner.selection_file
    }

    /// Состояние загрузки (`this.state`).
    pub fn state(&self) -> ManagerState {
        lock(&self.inner.state).clone()
    }

    /// Проверка обновления (`this.update`).
    pub fn update_state(&self) -> UpdateState {
        lock(&self.inner.update).clone()
    }

    /// Идёт загрузка или обновление (`this.pending !== null`).
    pub fn is_pending(&self) -> bool {
        lock(&self.inner.pending).is_some()
    }

    /// Есть отменяемая операция (`this.controller !== null`).
    pub fn has_controller(&self) -> bool {
        lock(&self.inner.controller).is_some()
    }

    /// `on(event, listener)` для всех событий.
    pub fn subscribe(&self, listener: ProtonListener) -> ListenerId {
        let id = self
            .inner
            .next_listener
            .fetch_add(1, AtomicOrdering::Relaxed);
        lock(&self.inner.listeners).push((id, listener));
        ListenerId(id)
    }

    pub fn unsubscribe(&self, id: ListenerId) {
        lock(&self.inner.listeners).retain(|(item, _)| *item != id.0);
    }

    fn emit(&self, name: &'static str, payload: Value) {
        let listeners: Vec<ProtonListener> = lock(&self.inner.listeners)
            .iter()
            .map(|(_, listener)| listener.clone())
            .collect();
        let event = ProtonEvent { name, payload };
        for listener in listeners {
            listener(&event);
        }
    }

    /// `setState(next)`: изменить состояние и сообщить `state`.
    fn set_state(&self, change: impl FnOnce(&mut ManagerState)) {
        change(&mut lock(&self.inner.state));
        self.emit("state", self.status());
    }

    fn join(&self, parts: &[&str]) -> String {
        self.inner.path.join(parts)
    }

    /// `path.resolve(this.root) + path.sep`.
    fn managed_prefix(&self) -> String {
        format!(
            "{}{}",
            self.inner.path.resolve(&[&self.inner.root]),
            self.inner.path.sep()
        )
    }

    /// `searchRoots()`: где искать сборки.
    pub fn search_roots(&self) -> Vec<String> {
        let home = self.inner.home.as_str();
        let mut roots = vec![
            self.inner.root.clone(),
            self.join(&[home, ".steam/root/compatibilitytools.d"]),
            self.join(&[home, ".steam/steam/compatibilitytools.d"]),
            self.join(&[home, ".local/share/Steam/compatibilitytools.d"]),
            self.join(&[
                home,
                ".var/app/com.valvesoftware.Steam/data/Steam/compatibilitytools.d",
            ]),
            self.join(&[home, ".local/share/Steam/steamapps/common"]),
            self.join(&[home, ".steam/root/steamapps/common"]),
            "/usr/share/steam/compatibilitytools.d".to_owned(),
            "/usr/local/share/steam/compatibilitytools.d".to_owned(),
            self.join(&[home, ".config/heroic/tools/proton"]),
            self.join(&[
                home,
                ".var/app/com.heroicgameslauncher.hgl/config/heroic/tools/proton",
            ]),
        ];
        for steam in [
            self.join(&[home, ".local/share/Steam"]),
            self.join(&[home, ".steam/root"]),
            self.join(&[home, ".var/app/com.valvesoftware.Steam/data/Steam"]),
        ] {
            // Списка библиотек может не быть.
            let file = self.join(&[&steam, "steamapps", "libraryfolders.vdf"]);
            let Ok(bytes) = js::fs::read_file(Path::new(&file)) else {
                continue;
            };
            let Ok(parsed) = vdf::parse(&js::utf8_string(&bytes)) else {
                continue;
            };
            let Some(folders) =
                vdf::get_insensitive(&parsed, "libraryfolders").and_then(|v| v.as_object())
            else {
                continue;
            };
            for (key, entry) in folders.iter() {
                if key.is_empty() || !key.bytes().all(|b| b.is_ascii_digit()) {
                    continue;
                }
                let value = match entry {
                    vdf::VdfValue::String(text) => Some(text.as_str()),
                    vdf::VdfValue::Object(object) => {
                        vdf::get_insensitive(object, "path").and_then(|v| v.as_str())
                    }
                };
                if let Some(value) = value
                    && self.inner.path.is_absolute(value)
                {
                    roots.push(self.join(&[value, "steamapps", "common"]));
                }
            }
        }
        roots
    }

    /// `discover()`: найденные сборки без повторов (по пути после разворота ссылок). Сборки HLSR
    /// идут первыми и от новой к старой: при запасном выборе берётся первая из них.
    pub fn discover(&self) -> Vec<ProtonEntry> {
        let path = &self.inner.path;
        let managed_prefix = self.managed_prefix();
        let mut found: Vec<ProtonEntry> = Vec::new();
        let mut add = |candidate: &str, source: &str| {
            let Ok(real) = real_path(Path::new(candidate)) else {
                return;
            };
            let real_text = real.to_string_lossy().into_owned();
            if !is_proton(&real) || found.iter().any(|entry| entry.path == real_text) {
                return;
            }
            let managed = real_text.starts_with(&managed_prefix);
            let wineland = managed && is_wineland_directory(&path.basename(&real_text));
            found.push(ProtonEntry {
                id: real_text.clone(),
                name: path.basename(candidate),
                path: real_text,
                source: if wineland { SOURCE_WINELAND } else { source }.to_owned(),
                recommended: wineland,
            });
        };
        for root in self.search_roots() {
            let Ok(entries) = js::fs::read_dir(Path::new(&root)) else {
                continue;
            };
            for entry in entries {
                if !entry.file_type.is_dir() && !entry.file_type.is_symlink() {
                    continue;
                }
                let source = if root == self.inner.root {
                    "HLSR"
                } else {
                    "Steam / system"
                };
                add(&path.join(&[&root, &entry.name]), source);
            }
        }
        for candidate in self.selection_data().custom_paths {
            add(&candidate, "Custom");
        }
        let collator = Collator::new(&default_locale(), Sensitivity::Variant);
        found.sort_by(|a, b| {
            b.recommended.cmp(&a.recommended).then_with(|| {
                if a.recommended {
                    collator.compare(&b.name, &a.name)
                } else {
                    collator.compare(&a.name, &b.name)
                }
            })
        });
        found
    }

    /// `selectedPath()`: путь из `proton-selection.json`.
    pub fn selected_path(&self) -> Option<String> {
        self.selection_data().path
    }

    /// `selectionData()`: битый или отсутствующий файл — пустой выбор.
    pub fn selection_data(&self) -> SelectionData {
        let Ok(bytes) = js::fs::read_file(Path::new(&self.inner.selection_file)) else {
            return SelectionData::default();
        };
        let Ok(value) = json_parse(&js::utf8_string(&bytes)) else {
            return SelectionData::default();
        };
        if value.is_null() {
            return SelectionData::default();
        }
        let path = value.get("path").and_then(Value::as_str).map(str::to_owned);
        let custom_paths = value
            .get("customPaths")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|item| self.inner.path.is_absolute(item))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        SelectionData { path, custom_paths }
    }

    /// `writeSelection(data)`: два пробела без перевода строки, через временный файл, 0600.
    pub fn write_selection(&self, data: &SelectionData) -> Result<(), ProtonError> {
        std::fs::create_dir_all(&self.inner.data_dir).map_err(io_error)?;
        let temp = format!("{}.{}.tmp", self.inner.selection_file, std::process::id());
        let text = stringify(&data.to_json(), "  ");
        write_private(Path::new(&temp), text.as_bytes())?;
        std::fs::rename(&temp, &self.inner.selection_file).map_err(io_error)
    }

    /// `selected()`: выбранная сборка; прежние CachyOS и Wineland без правок из `protons/`
    /// заменяются сборкой HLSR. Ничего не выбрано — первая сборка HLSR или первая найденная.
    pub fn selected(&self) -> Option<ProtonEntry> {
        let installed = self.discover();
        let choice = self.selected_path();
        if let Some(selected) = installed
            .iter()
            .find(|item| Some(&item.path) == choice.as_ref())
        {
            let name = self.inner.path.basename(&selected.path);
            let managed = selected.path.starts_with(&self.managed_prefix());
            if !managed
                || (!is_legacy_cachyos_directory(&name) && !is_upstream_wineland_directory(&name))
            {
                return Some(selected.clone());
            }
            return installed.into_iter().find(|item| item.recommended);
        }
        let recommended = installed.iter().position(|item| item.recommended);
        match recommended {
            Some(index) => Some(installed[index].clone()),
            None => installed.into_iter().next(),
        }
    }

    /// `select(path)`: выбрать найденную сборку.
    pub fn select(&self, raw_path: &str) -> Result<ProtonEntry, ProtonError> {
        let Some(item) = self
            .discover()
            .into_iter()
            .find(|entry| entry.path == raw_path)
        else {
            return Err(ProtonError::new(
                "Выбранный Proton не найден. Обновите список версий.",
            ));
        };
        let mut data = self.selection_data();
        data.path = Some(item.path.clone());
        self.write_selection(&data)?;
        self.emit("state", self.status());
        Ok(item)
    }

    /// `addCustom(path)`: своя папка с исполняемым `proton`.
    pub fn add_custom(&self, raw_path: &str) -> Result<Option<ProtonEntry>, ProtonError> {
        if !self.inner.path.is_absolute(raw_path) {
            return Err(ProtonError::new("Укажите абсолютный путь к папке Proton"));
        }
        let Ok(real) = real_path(Path::new(raw_path)) else {
            return Err(ProtonError::new("Папка Proton не найдена"));
        };
        if !is_proton(&real) {
            return Err(ProtonError::new("В папке нет исполняемого файла proton"));
        }
        let real = real.to_string_lossy().into_owned();
        let mut data = self.selection_data();
        if !data.custom_paths.contains(&real) {
            data.custom_paths.push(real.clone());
        }
        self.write_selection(&data)?;
        let item = self.discover().into_iter().find(|entry| entry.path == real);
        self.emit("state", self.status());
        Ok(item)
    }

    /// `status()`: найденные, выбранная, состояние загрузки и проверка обновления.
    pub fn status(&self) -> Value {
        let installed = self.discover();
        let selected = self.selected();
        let state = self.state();
        let mut object = Map::new();
        object.insert(
            "installed".to_owned(),
            Value::Array(installed.iter().map(ProtonEntry::to_json).collect()),
        );
        object.insert(
            "selected".to_owned(),
            selected.map_or(Value::Null, |item| Value::String(item.path)),
        );
        object.insert("phase".to_owned(), Value::String(state.phase));
        object.insert("percent".to_owned(), js_number(state.percent));
        object.insert("error".to_owned(), json!(state.error));
        object.insert("operation".to_owned(), json!(state.operation));
        object.insert("update".to_owned(), self.update_state().to_json());
        Value::Object(object)
    }

    /// `managedWineland()`: свои сборки HLSR от новой к старой.
    pub fn managed_wineland(&self) -> Vec<ManagedWineland> {
        let Ok(entries) = js::fs::read_dir(Path::new(&self.inner.root)) else {
            return Vec::new();
        };
        let mut list: Vec<ManagedWineland> = entries
            .into_iter()
            .filter(|entry| entry.file_type.is_dir() && is_wineland_directory(&entry.name))
            .filter_map(|entry| {
                let path = self.join(&[&self.inner.root, &entry.name]);
                let version = wineland_version(&entry.name)?;
                is_proton(Path::new(&path)).then_some(ManagedWineland {
                    name: entry.name,
                    path,
                    version,
                })
            })
            .collect();
        list.sort_by(|a, b| compare_wineland(&b.version, &a.version));
        list
    }

    /// `latestRelease(signal)`: самый новый подходящий релиз из списка GitHub.
    pub async fn latest_release(&self) -> Result<Option<UpdateCandidate>, ProtonError> {
        let text = self
            .inner
            .read_text
            .read_text(RELEASES_API, None, None)
            .await?;
        let list = json_parse(&text).map_err(|error| ProtonError::new(error.to_string()))?;
        let Value::Array(list) = list else {
            return Err(ProtonError::new(
                "GitHub вернул неожиданный список релизов Proton",
            ));
        };
        let mut candidates: Vec<UpdateCandidate> =
            list.iter().filter_map(update_candidate).collect();
        candidates.sort_by(|a, b| compare_wineland(&b.version, &a.version));
        Ok(candidates.into_iter().next())
    }

    /// `checkForUpdate()`: одна проверка метаданных за запуск; загрузка — только
    /// [`apply_update`](Self::apply_update). Итог — предложенный релиз или `None`.
    pub async fn check_for_update(&self) -> Option<UpdateCandidate> {
        let shared = {
            let mut check = lock(&self.inner.update_check);
            match check.as_ref() {
                Some(shared) => shared.clone(),
                None => {
                    let this = self.clone();
                    let shared = Shared::spawn(
                        async move {
                            match this.run_update_check().await {
                                Ok(candidate) => candidate,
                                Err(error) => {
                                    {
                                        let mut update = lock(&this.inner.update);
                                        update.checked = true;
                                        update.error = Some(error.message.clone());
                                    }
                                    this.set_state(|_| {});
                                    this.inner.logger.warn(&format!(
                                        "[proton] проверка обновления не удалась: {}",
                                        error.message
                                    ));
                                    None
                                }
                            }
                        },
                        || {},
                    );
                    *check = Some(shared.clone());
                    shared
                }
            }
        };
        shared.wait().await
    }

    /// `runUpdateCheck()`.
    async fn run_update_check(&self) -> Result<Option<UpdateCandidate>, ProtonError> {
        let pending = lock(&self.inner.pending).clone();
        if let Some(pending) = pending {
            let _ = pending.wait().await;
        }
        let selected = self.selected();
        let managed = self.managed_wineland();
        let current = selected
            .filter(|selected| selected.recommended)
            .and_then(|selected| managed.iter().find(|entry| entry.path == selected.path))
            .or_else(|| managed.first())
            .cloned();
        let Some(current) = current else {
            return Ok(None);
        };
        let latest = self.latest_release().await?;
        *lock(&self.inner.candidate) = latest.clone();
        let available = latest
            .as_ref()
            .is_some_and(|latest| compare_wineland(&latest.version, &current.version).is_gt());
        {
            let mut update = lock(&self.inner.update);
            update.checked = true;
            update.error = None;
            update.available = available;
            update.latest = Some(
                latest
                    .as_ref()
                    .map_or_else(|| current.version.tag.clone(), |l| l.version.tag.clone()),
            );
        }
        self.set_state(|_| {});
        if !available {
            return Ok(None);
        }
        let latest = latest.expect("доступное обновление есть");
        self.inner.logger.info(&format!(
            "[proton] доступна новая сборка {}",
            latest.version.tag
        ));
        self.emit(
            "update-available",
            json!({"from": current.version.tag, "to": latest.version.tag}),
        );
        Ok(Some(latest))
    }

    /// `assertSelectionIdle()`: выбрана сборка HLSR, из которой что-то запущено, —
    /// `PROTON_IN_USE`.
    pub fn assert_selection_idle(&self) -> Result<(), ProtonError> {
        if let Some(selected) = self.selected()
            && selected.recommended
            && (self.inner.in_use)(Path::new(&selected.path))
        {
            return Err(ProtonError::with_code(
                "PROTON_IN_USE",
                "Закройте игру и LiveSplit перед обновлением Proton.",
            ));
        }
        Ok(())
    }

    /// `applyUpdate()`: поставить предложенный релиз (проверенный по SHA-256 GitHub и
    /// `.sha512sum`) и перевести на него тех, кто сидел на сборке HLSR. Прежняя сборка остаётся:
    /// префиксы ссылаются на её файлы, и к ней можно вернуться вручную.
    pub async fn apply_update(&self) -> InstallResult {
        self.check_for_update().await;
        let shared = {
            let mut pending = lock(&self.inner.pending);
            match pending.as_ref() {
                Some(shared) => shared.clone(),
                None => {
                    let candidate = lock(&self.inner.candidate).clone();
                    let available = lock(&self.inner.update).available;
                    let Some(candidate) = candidate.filter(|_| available) else {
                        return Ok(None);
                    };
                    self.assert_selection_idle()?;
                    let selected = self.selected();
                    let current = match selected
                        .filter(|selected| selected.recommended)
                        .and_then(|selected| wineland_version(&selected.name))
                    {
                        Some(version) => version,
                        None => match self.managed_wineland().into_iter().next() {
                            Some(entry) => entry.version,
                            None => {
                                return Err(ProtonError::new(
                                    "Cannot read properties of undefined (reading 'version')",
                                ));
                            }
                        },
                    };
                    let this = self.clone();
                    let cleaner = self.clone();
                    let shared = Shared::spawn(
                        async move {
                            match this.update_body(&candidate, &current).await {
                                Ok(result) => Ok(result),
                                Err(error) => {
                                    *lock(&this.inner.controller) = None;
                                    let message = error.message.clone();
                                    this.set_state(|state| {
                                        state.phase = "error".to_owned();
                                        state.error = Some(message);
                                    });
                                    Err(error)
                                }
                            }
                        },
                        move || *lock(&cleaner.inner.pending) = None,
                    );
                    *pending = Some(shared.clone());
                    shared
                }
            }
        };
        shared.wait().await
    }

    async fn update_body(
        &self,
        candidate: &UpdateCandidate,
        current: &WinelandVersion,
    ) -> InstallResult {
        self.set_state(|state| {
            state.phase = "checking".to_owned();
            state.percent = 0.0;
            state.error = None;
            state.operation = Some("update".to_owned());
        });
        let existing = self.discover().into_iter().find(|entry| {
            entry.recommended
                && wineland_version(&entry.name)
                    .is_some_and(|version| version.tag == candidate.version.tag)
        });
        let installed = match existing {
            Some(installed) => {
                self.replace_managed_selection(&installed)?;
                self.set_state(|state| {
                    state.phase = "ready".to_owned();
                    state.percent = 100.0;
                    state.error = None;
                });
                installed
            }
            None => {
                let controller = Abort::new();
                let sums_url = candidate
                    .sums
                    .get("browser_download_url")
                    .map(js::to_string)
                    .unwrap_or_default();
                let read =
                    self.inner
                        .read_text
                        .read_text(&sums_url, Some(SUMS_LIMIT), Some(&controller));
                let sums = match self.read_metadata(&controller, read).await? {
                    Step::Done(sums) => sums,
                    Step::Cancelled => return Ok(None),
                };
                let name = candidate.archive_name();
                let expected = PinnedArchive {
                    sha512: parse_sha512sum(&sums, &name)?,
                    name,
                    size: candidate.archive_size(),
                    sha256: candidate.sha256.clone(),
                };
                match self
                    .install_archive(&candidate.archive, &expected, true, controller)
                    .await?
                {
                    Some(installed) => installed,
                    None => return Ok(None),
                }
            }
        };
        {
            let mut update = lock(&self.inner.update);
            update.available = false;
            update.updated_from = Some(current.tag.clone());
            update.updated_to = Some(candidate.version.tag.clone());
        }
        self.set_state(|_| {});
        self.emit(
            "updated",
            json!({"from": current.tag, "to": candidate.version.tag, "path": installed.path}),
        );
        Ok(Some(installed))
    }

    /// `ensureReference()`: есть сборка HLSR — выбрать её, если нужно; нет — скачать
    /// закреплённую (события `install-started` и `installed`).
    pub async fn ensure_reference(&self) -> InstallResult {
        let shared = {
            let mut pending = lock(&self.inner.pending);
            match pending.as_ref() {
                Some(shared) => shared.clone(),
                None => {
                    if let Some(existing) = self.discover().into_iter().find(|e| e.recommended) {
                        drop(pending);
                        self.select_reference_if_needed(&existing)?;
                        return Ok(Some(existing));
                    }
                    let this = self.clone();
                    let cleaner = self.clone();
                    let shared = Shared::spawn(
                        async move {
                            this.emit("install-started", json!({"version": RELEASE_TAG}));
                            let installed = this.download_reference().await?;
                            if let Some(installed) = &installed {
                                this.emit(
                                    "installed",
                                    json!({"version": RELEASE_TAG, "path": installed.path}),
                                );
                            }
                            Ok(installed)
                        },
                        move || *lock(&cleaner.inner.pending) = None,
                    );
                    *pending = Some(shared.clone());
                    shared
                }
            }
        };
        shared.wait().await
    }

    /// `selectReferenceIfNeeded(reference)`: выбрать сборку HLSR, если ничего не выбрано или
    /// выбрана прежняя сборка из `protons/`. Ручной выбор не трогается.
    pub fn select_reference_if_needed(&self, reference: &ProtonEntry) -> Result<bool, ProtonError> {
        let Some(selected) = self.selected_path() else {
            self.select(&reference.path)?;
            return Ok(true);
        };
        // Выбранная папка могла быть удалена.
        let real = real_path(Path::new(&selected))
            .map(|real| real.to_string_lossy().into_owned())
            .unwrap_or(selected);
        let managed = self
            .inner
            .path
            .resolve(&[&real])
            .starts_with(&self.managed_prefix());
        let name = self.inner.path.basename(&real);
        if managed && (is_legacy_cachyos_directory(&name) || is_upstream_wineland_directory(&name))
        {
            self.select(&reference.path)?;
            self.inner
                .logger
                .info("[proton] выбрана HLSR-сборка Proton Wineland");
            return Ok(true);
        }
        Ok(false)
    }

    /// `downloadReference()`: первая установка берёт закреплённую сборку, её хеши проверены
    /// вручную; более новая предлагается после проверки обновления.
    async fn download_reference(&self) -> InstallResult {
        let controller = Abort::new();
        self.set_state(|state| {
            state.phase = "checking".to_owned();
            state.percent = 0.0;
            state.error = None;
            state.operation = Some("install".to_owned());
        });
        let pinned = self.inner.pinned.clone();
        let read = async {
            let text = self
                .inner
                .read_text
                .read_text(RELEASE_API, None, Some(&controller))
                .await?;
            let release = json_parse(&text).map_err(|error| ProtonError::new(error.to_string()))?;
            safe_release_asset_with(&release, &pinned)
        };
        let archive = match self.read_metadata(&controller, read).await? {
            Step::Done(archive) => archive,
            Step::Cancelled => return Ok(None),
        };
        self.install_archive(&archive, &pinned, false, controller)
            .await
    }

    /// `readMetadata(controller, read)`: метаданные читаются под тем же контроллером, что и
    /// загрузка; отмена на любой стадии — `Cancelled`, состояние `idle`.
    async fn read_metadata<T>(
        &self,
        controller: &Abort,
        read: impl std::future::Future<Output = Result<T, ProtonError>>,
    ) -> Result<Step<T>, ProtonError> {
        *lock(&self.inner.controller) = Some(controller.clone());
        let result = tokio::select! {
            result = read => result,
            () = controller.aborted() => Err(ProtonError::abort("This operation was aborted")),
        };
        let result = result.and_then(|value| {
            if controller.is_aborted() {
                Err(ProtonError::abort("This operation was aborted"))
            } else {
                Ok(value)
            }
        });
        match result {
            Ok(value) => Ok(Step::Done(value)),
            Err(error) => {
                *lock(&self.inner.controller) = None;
                if controller.is_aborted() {
                    self.set_state(|state| {
                        state.phase = "idle".to_owned();
                        state.percent = 0.0;
                        state.error = None;
                    });
                    return Ok(Step::Cancelled);
                }
                let message = error.message.clone();
                self.set_state(|state| {
                    state.phase = "error".to_owned();
                    state.error = Some(message.clone());
                });
                self.inner
                    .logger
                    .error(&format!("[proton] установка не удалась: {message}"));
                Err(error)
            }
        }
    }

    /// `installArchive(archive, expected, options)`: загрузка во временную папку
    /// `protons/.download-*`, сверка имени, размера, SHA-256 и SHA-512, проверка списка архива,
    /// распаковка и перенос в `protons/<корень архива>`.
    pub async fn install_archive(
        &self,
        archive: &Value,
        expected: &PinnedArchive,
        replace_selection: bool,
        controller: Abort,
    ) -> InstallResult {
        *lock(&self.inner.controller) = Some(controller.clone());
        let mut staging: Option<PathBuf> = None;
        let result = self
            .install_into(
                archive,
                expected,
                replace_selection,
                &controller,
                &mut staging,
            )
            .await;
        let result = match result {
            Ok(entry) => Ok(Some(entry)),
            Err(_) if controller.is_aborted() => {
                self.set_state(|state| {
                    state.phase = "idle".to_owned();
                    state.percent = 0.0;
                    state.error = None;
                });
                Ok(None)
            }
            Err(error) => {
                let message = error.message.clone();
                self.set_state(|state| {
                    state.phase = "error".to_owned();
                    state.error = Some(message.clone());
                });
                self.inner
                    .logger
                    .error(&format!("[proton] установка не удалась: {message}"));
                Err(error)
            }
        };
        if let Some(staging) = staging {
            let _ = std::fs::remove_dir_all(staging);
        }
        *lock(&self.inner.controller) = None;
        result
    }

    async fn install_into(
        &self,
        archive: &Value,
        expected: &PinnedArchive,
        replace_selection: bool,
        controller: &Abort,
        staging_slot: &mut Option<PathBuf>,
    ) -> Result<ProtonEntry, ProtonError> {
        self.set_state(|state| {
            state.phase = "downloading".to_owned();
            state.percent = 0.0;
            state.error = None;
        });
        let root = PathBuf::from(&self.inner.root);
        std::fs::create_dir_all(&root).map_err(io_error)?;
        let staging = make_temp_dir(&root, ".download-")?;
        *staging_slot = Some(staging.clone());
        let name = str_field(archive, "name")
            .map(str::to_owned)
            .unwrap_or_else(|| archive.get("name").map(js::to_string).unwrap_or_default());
        let file = PathBuf::from(self.join(&[&staging.to_string_lossy(), &name]));
        let url = archive
            .get("browser_download_url")
            .map(js::to_string)
            .unwrap_or_default();
        let mut body = self.inner.request.request(&url, Some(controller)).await?;
        let mut writer = open_private(&file).await?;
        let mut sha256 = Sha256::new();
        let mut sha512 = Sha512::new();
        let mut bytes: u64 = 0;
        let download = async {
            while let Some(chunk) = body.chunk().await? {
                bytes += chunk.len() as u64;
                if bytes > MAX_ARCHIVE_BYTES {
                    return Err(ProtonError::new("Архив Proton слишком велик"));
                }
                sha256.update(&chunk);
                sha512.update(&chunk);
                let percent = (bytes as f64 / expected.size as f64 * 100.0).floor();
                self.set_state(|state| state.percent = percent.min(99.0));
                writer.write_all(&chunk).await.map_err(io_error)?;
            }
            writer.flush().await.map_err(io_error)
        };
        tokio::select! {
            result = download => result?,
            () = controller.aborted() => {
                return Err(ProtonError::abort("This operation was aborted"));
            }
        }
        drop(writer);
        drop(body);
        if controller.is_aborted() {
            return Err(ProtonError::abort("This operation was aborted"));
        }
        let hex = |digest: &[u8]| hlsr_delivery::file_ops::hex(digest);
        if name != expected.name
            || bytes != expected.size
            || hex(&sha256.finalize()) != expected.sha256
            || hex(&sha512.finalize()) != expected.sha512
        {
            return Err(ProtonError::new(
                "Архив Proton Wineland не совпал с проверенной сборкой HLSR",
            ));
        }
        self.set_state(|state| {
            state.phase = "extracting".to_owned();
            state.percent = 100.0;
        });
        let output = PathBuf::from(self.join(&[&staging.to_string_lossy(), "extract"]));
        std::fs::create_dir(&output).map_err(io_error)?;
        let expected_root = name.strip_suffix(".tar.xz").unwrap_or(&name).to_owned();
        list_archive(&file, &expected_root, controller).await?;
        run_tar(
            &[
                "-xJf".to_owned(),
                file.to_string_lossy().into_owned(),
                "-C".to_owned(),
                output.to_string_lossy().into_owned(),
                "--no-same-owner".to_owned(),
                "--no-same-permissions".to_owned(),
            ],
            controller,
        )
        .await?;
        if controller.is_aborted() {
            return Err(ProtonError::abort("This operation was aborted"));
        }
        let extracted = PathBuf::from(self.join(&[&output.to_string_lossy(), &expected_root]));
        let meta = std::fs::symlink_metadata(&extracted).map_err(io_error)?;
        if !meta.is_dir() || meta.file_type().is_symlink() || !is_proton(&extracted) {
            return Err(ProtonError::new(
                "Распакованный Proton не содержит корректный исполняемый файл",
            ));
        }
        let target = PathBuf::from(self.join(&[&self.inner.root, &expected_root]));
        if !target.exists() {
            std::fs::rename(&extracted, &target).map_err(io_error)?;
        }
        let real = real_path(&target)
            .map_err(io_error)?
            .to_string_lossy()
            .into_owned();
        let Some(chosen) = self.discover().into_iter().find(|entry| entry.path == real) else {
            return Err(ProtonError::new("Установленный Proton не обнаружен"));
        };
        if replace_selection {
            self.replace_managed_selection(&chosen)?;
        } else {
            self.select_reference_if_needed(&chosen)?;
        }
        self.set_state(|state| {
            state.phase = "ready".to_owned();
            state.error = None;
        });
        self.inner
            .logger
            .info(&format!("[proton] установлен {}", chosen.name));
        Ok(chosen)
    }

    /// `replaceManagedSelection(reference)`: обновление переводит на новую сборку только тех, кто
    /// сидел на сборке HLSR (или ни на чём); выбранный вручную системный Proton остаётся.
    pub fn replace_managed_selection(&self, reference: &ProtonEntry) -> Result<(), ProtonError> {
        self.assert_selection_idle()?;
        let selected = self.selected_path();
        let managed = match selected {
            None => true,
            Some(selected) => {
                // Выбранная папка могла быть удалена.
                let real = real_path(Path::new(&selected))
                    .map(|real| real.to_string_lossy().into_owned())
                    .unwrap_or(selected);
                real.is_empty()
                    || self
                        .inner
                        .path
                        .resolve(&[&real])
                        .starts_with(&self.managed_prefix())
            }
        };
        if managed {
            self.select(&reference.path)?;
        }
        Ok(())
    }

    /// `cancel()`: отменить загрузку; `false` — отменять нечего.
    pub fn cancel(&self) -> bool {
        let controller = lock(&self.inner.controller);
        match controller.as_ref() {
            Some(abort) if !abort.is_aborted() => {
                abort.abort();
                true
            }
            _ => false,
        }
    }
}

/// Файл с правами 0600 (`writeFileSync(file, text, {mode: 0o600})`).
pub(crate) fn write_private(file: &Path, bytes: &[u8]) -> Result<(), ProtonError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut handle = options.open(file).map_err(io_error)?;
    std::io::Write::write_all(&mut handle, bytes).map_err(io_error)
}
