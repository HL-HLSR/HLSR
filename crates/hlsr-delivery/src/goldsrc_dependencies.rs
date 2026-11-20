//! Зависимости HL1 перед запуском (`goldsrc-dependencies.js`). Лаунчер делает то же, что update.bat
//! из сборки bxt.rs: берёт последний релиз с GitHub (`/releases/latest`), сверяет его тег с записанной
//! версией и при расхождении подменяет файл. Сбой обновления запуск не останавливает: игра стартует со
//! старой версией, а интерфейс предупреждает, что ран с устаревшим BXT может не засчитаться.
//!
//! 0.4.9: файл ставится только с SHA-256 — закреплённым в лаунчере (`pin`: релиз по тегу) или от
//! GitHub; без хеша — `CHECKSUM_MISMATCH`.
//!
//! Адреса GitHub — константы; подменить их можно только в отладочной сборке и с `dev-hooks`
//! ([`UpdateOptions::api_root`], [`UpdateOptions::download_origin`]): тесты ходят на локальный сервер.

use crate::archive_service::{ArchiveService, ExtractOptions};
use crate::errors::{Code, DeliveryError, Failure, RawError, to_delivery_error};
use crate::file_ops::{hex, rm_sync};
use crate::http_client::{HttpClient, RequestOptions, parse_url, url_hostname};
use crate::js_value::{prop, template, to_number, truthy};
use crate::manifests::Dependency;
use crate::paths::assert_inside_with;
use crate::ports::{Logger, TracingLogger};
use hlsr_core::js;
use hlsr_core::js::path::NodePath;
use regress::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

/// Корень API репозиториев GitHub.
pub const API_ROOT: &str = "https://api.github.com/repos/";
/// Адрес, с которого GitHub отдаёт файлы релиза.
pub const GITHUB_ORIGIN: &str = "https://github.com";
/// Простой соединения при запросе релиза.
pub const CHECK_TIMEOUT: Duration = Duration::from_millis(10_000);
/// Простой соединения при скачивании файла релиза.
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_millis(90_000);
/// Файл релиза больше этого размера не качается.
pub const MAX_ASSET_BYTES: u64 = 32 * 1024 * 1024;

/// `/^[A-Za-z0-9-]+\/[A-Za-z0-9._-]+$/`.
static REPO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9-]+\/[A-Za-z0-9._-]+$").expect("выражение 0.4.7 разбирается")
});

/// `/^[0-9A-Za-z][0-9A-Za-z._-]{0,63}$/`.
static TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9A-Za-z][0-9A-Za-z._-]{0,63}$").expect("выражение 0.4.7 разбирается")
});

/// `/^sha256:([a-f0-9]{64})$/`.
static SHA256: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^sha256:([a-f0-9]{64})$").expect("выражение 0.4.7 разбирается"));

/// Адрес последнего релиза репозитория.
pub fn latest_release_url(api_root: &str, repo: &str) -> String {
    format!("{api_root}{repo}/releases/latest")
}

/// Что нашлось в релизе: тег, файл (имя, адрес, размер) и его SHA-256 (закреплённый или от GitHub).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

impl Release {
    /// Объект `{ tag, name, url, size, sha256 }`, как у 0.4.7.
    pub fn to_value(&self) -> Value {
        json!({
            "tag": self.tag,
            "name": self.name,
            "url": self.url,
            "size": self.size,
            "sha256": self.sha256,
        })
    }
}

/// Итог по одной зависимости.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    UpToDate,
    Updated,
    Failed,
}

impl Status {
    /// `up-to-date`, `updated` или `failed`.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::UpToDate => "up-to-date",
            Status::Updated => "updated",
            Status::Failed => "failed",
        }
    }
}

/// `{ id, title, from, to, status, error? }`: результат уходит в интерфейс.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyResult {
    pub id: String,
    pub title: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub status: Status,
    /// Код и текст ошибки у `Failed`.
    pub error: Option<(Code, String)>,
}

impl DependencyResult {
    pub fn to_value(&self) -> Value {
        let mut object = json!({
            "id": self.id,
            "title": self.title,
            "from": self.from,
            "to": self.to,
            "status": self.status.as_str(),
        });
        if let Some((code, message)) = &self.error {
            object["error"] = json!({"code": code.as_str(), "message": message});
        }
        object
    }
}

/// Зависимости обновления и подмены `updateDependencies` (`options` в 0.4.7).
#[derive(Default)]
pub struct UpdateOptions<'a> {
    /// HTTP-клиент; без него — настоящий.
    pub client: Option<&'a HttpClient>,
    /// 7-Zip для архивов (BXT); без него архив не распаковать (`EXTRACT_FAILED`).
    pub archive: Option<&'a ArchiveService>,
    pub logger: Option<&'a dyn Logger>,
    /// Другой корень API вместо `https://api.github.com/repos/`: локальный сервер тестов.
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    pub api_root: Option<&'a str>,
    /// Другой адрес вместо `https://github.com`, откуда качаются файлы: локальный сервер тестов.
    /// Проверка «файл лежит на github.com» идёт по адресу из ответа API, подменяется только запрос.
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    pub download_origin: Option<&'a str>,
}

impl UpdateOptions<'_> {
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    fn api_root(&self) -> &str {
        self.api_root.unwrap_or(API_ROOT)
    }

    #[cfg(not(any(debug_assertions, feature = "dev-hooks")))]
    fn api_root(&self) -> &str {
        API_ROOT
    }

    /// Адрес запроса файла релиза.
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    fn download_url(&self, url: &str) -> String {
        match (self.download_origin, url.strip_prefix(GITHUB_ORIGIN)) {
            (Some(origin), Some(rest)) => format!("{origin}{rest}"),
            _ => url.to_owned(),
        }
    }

    #[cfg(not(any(debug_assertions, feature = "dev-hooks")))]
    fn download_url(&self, url: &str) -> String {
        url.to_owned()
    }
}

/// `insideInstall(installPath, relative)`: путь внутри папки установки (`/` в относительном — разделитель
/// ОС), иначе `PATH_INVALID`.
fn inside_install(install_path: &Path, relative: &str) -> Result<PathBuf, DeliveryError> {
    let path = NodePath::native();
    let install = install_path.to_string_lossy();
    let joined = path.join(&[&install, &relative.replace('/', path.sep())]);
    assert_inside_with(&path, &install, &joined, Some("Зависимость")).map(PathBuf::from)
}

/// `readVersion(file)`: записанная версия; нет файла, нечего читать или пусто — `None`.
fn read_version(file: &Path) -> Option<String> {
    let bytes = hlsr_core::js::fs::read_file(file).ok()?;
    let text = js::utf8_string(&bytes);
    let text = js::trim(text.strip_prefix('\u{FEFF}').unwrap_or(&text));
    (!text.is_empty()).then(|| text.to_owned())
}

/// `latestRelease(dependency, client)`: последний (или закреплённый `pin`) релиз репозитория и нужный
/// файл в нём.
pub async fn latest_release(
    dependency: &Dependency,
    client: &HttpClient,
    api_root: &str,
) -> Result<Release, Failure> {
    require_sha256(find_release(dependency, client, api_root).await?)
}

/// Релиз без требования хеша: чтобы понять, что ставить нечего, хватает тега. Пустой `sha256` —
/// GitHub его не сообщил.
async fn find_release(
    dependency: &Dependency,
    client: &HttpClient,
    api_root: &str,
) -> Result<Release, Failure> {
    if REPO.find(dependency.repo).is_none() {
        return Err(DeliveryError::new(
            Code::ManifestInvalid,
            format!("Недопустимый репозиторий {}", dependency.repo),
        )
        .into());
    }

    let options = RequestOptions {
        timeout: Some(CHECK_TIMEOUT),
        headers: vec![(
            "accept".to_owned(),
            "application/vnd.github+json".to_owned(),
        )],
        ..RequestOptions::default()
    };
    let pin = dependency.pin.as_ref();
    let url = match pin {
        Some(pin) => format!("{api_root}{}/releases/tags/{}", dependency.repo, pin.tag),
        None => latest_release_url(api_root, dependency.repo),
    };
    let data = client.get_json(&url, &options, None).await?;

    let tag = prop(&data, "tag_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if TAG.find(tag).is_none() || pin.is_some_and(|pin| tag != pin.tag) {
        return Err(DeliveryError::new(
            Code::ProviderLinkFailed,
            format!("GitHub вернул недопустимый тег релиза: {tag}"),
        )
        .into());
    }

    let pattern = Regex::with_flags(dependency.asset, "i").map_err(|error| {
        RawError::new(format!(
            "Invalid regular expression: /{}/i: {error}",
            dependency.asset
        ))
    })?;
    let assets = prop(&data, "assets")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let found = assets
        .iter()
        .find(|item| js::truthy(item) && pattern.find(&template(prop(item, "name"))).is_some());
    let Some(asset) = found else {
        return Err(not_found(tag, dependency).into());
    };
    let Some(download_url) = prop(asset, "browser_download_url").and_then(Value::as_str) else {
        return Err(not_found(tag, dependency).into());
    };

    // `Number(asset.size)`: у отсутствующего размера — NaN.
    let size = prop(asset, "size").map_or(f64::NAN, to_number);
    if !(size.is_finite() && size.fract() == 0.0) || size <= 0.0 || size > MAX_ASSET_BYTES as f64 {
        return Err(DeliveryError::new(
            Code::ProviderLinkFailed,
            format!(
                "Недопустимый размер {}: {}",
                template(prop(asset, "name")),
                template(prop(asset, "size"))
            ),
        )
        .into());
    }

    let url = parse_url(download_url)?;
    if url.scheme() != "https" || url_hostname(&url) != "github.com" {
        return Err(DeliveryError::new(
            Code::ProviderLinkFailed,
            format!("Файл релиза лежит не на github.com: {}", url_hostname(&url)),
        )
        .into());
    }

    // Хеш закреплённой версии лежит в лаунчере. Иначе — SHA-256, который GitHub отдаёт для файлов
    // релиза; у старых релизов его может не быть, и тогда файл не ставится.
    let digest = match prop(asset, "digest") {
        Some(value) if truthy(Some(value)) => template(Some(value)),
        _ => String::new(),
    };
    let name = template(prop(asset, "name"));
    let sha256 = match pin {
        Some(pin) => pin.sha256.to_owned(),
        None => SHA256
            .find(&digest)
            .and_then(|found| found.group(1))
            .map(|range| digest[range].to_owned())
            .unwrap_or_default(),
    };

    Ok(Release {
        tag: tag.to_owned(),
        name,
        url: url.as_str().to_owned(),
        size: size as u64,
        sha256,
    })
}

/// Без SHA-256 (закреплённого или от GitHub) файл не ставится.
fn require_sha256(release: Release) -> Result<Release, Failure> {
    if release.sha256.is_empty() {
        return Err(DeliveryError::new(
            Code::ChecksumMismatch,
            format!("GitHub не сообщил SHA-256 файла {}", release.name),
        )
        .into());
    }
    Ok(release)
}

fn not_found(tag: &str, dependency: &Dependency) -> DeliveryError {
    DeliveryError::new(
        Code::ProviderLinkFailed,
        format!("В релизе {tag} нет файла {}", dependency.asset),
    )
}

async fn download(
    release: &Release,
    file: &Path,
    client: &HttpClient,
    url: &str,
) -> Result<(), Failure> {
    let options = RequestOptions {
        timeout: Some(DOWNLOAD_TIMEOUT),
        ..RequestOptions::default()
    };
    let mut response = client.request(url, &options).await?;
    if response.status() != 200 {
        let status = response.status();
        drop(response);
        return Err(DeliveryError::new(
            Code::HttpError,
            format!("HTTP {status} для {}", release.name),
        )
        .into());
    }

    // Сбой потока (обрыв, диск) — NETWORK_ERROR; свои ошибки идут как есть.
    let streamed = stream_to_file(release, &mut response, file).await;
    let (size, digest) = streamed.map_err(|failure| {
        Failure::Delivery(to_delivery_error(failure, Some(Code::NetworkError)))
    })?;

    if size != release.size {
        return Err(DeliveryError::new(
            Code::SizeMismatch,
            format!("Получено {size} Б из {} для {}", release.size, release.name),
        )
        .into());
    }
    if digest != release.sha256 {
        return Err(DeliveryError::new(
            Code::ChecksumMismatch,
            format!("SHA-256 {} не совпал", release.name),
        )
        .with_details(json!({"expected": release.sha256, "actual": digest}))
        .into());
    }
    Ok(())
}

/// Тело ответа в файл: размер и SHA-256 полученного. Больше объявленного — `SIZE_MISMATCH` сразу.
async fn stream_to_file(
    release: &Release,
    response: &mut crate::http_client::Response,
    file: &Path,
) -> Result<(u64, String), Failure> {
    let mut out = fs::File::create(file).map_err(|e| RawError::fs(&e, "open", file))?;
    let mut hash = Sha256::new();
    let mut size: u64 = 0;
    while let Some(chunk) = response.chunk().await? {
        size += chunk.len() as u64;
        if size > release.size {
            return Err(DeliveryError::new(
                Code::SizeMismatch,
                format!("{} больше объявленных {} Б", release.name, release.size),
            )
            .into());
        }
        hash.update(&chunk);
        out.write_all(&chunk)
            .map_err(|e| RawError::fs(&e, "write", file))?;
    }
    Ok((size, hex(&hash.finalize())))
}

/// `assertWindowsBinary(file)`: файл начинается с «MZ».
fn assert_windows_binary(file: &Path) -> Result<(), Failure> {
    let mut head = [0u8; 2];
    let mut handle = fs::File::open(file).map_err(|e| RawError::fs(&e, "open", file))?;
    // `readSync` читает сколько есть: короткий файл оставляет нули.
    let mut filled = 0;
    while filled < head.len() {
        match handle.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            // Ошибка `readSync` по дескриптору — без пути.
            Err(error) => return Err(crate::atomic::fd_error(&error, "read").into()),
        }
    }
    if &head != b"MZ" {
        let name = NodePath::native().basename(&file.to_string_lossy());
        return Err(DeliveryError::new(
            Code::ExtractFailed,
            format!("{name} не похож на исполняемый файл Windows"),
        )
        .into());
    }
    Ok(())
}

async fn update_one(
    install_path: &Path,
    dependency: &Dependency,
    client: &HttpClient,
    options: &UpdateOptions<'_>,
    logger: &dyn Logger,
) -> Result<DependencyResult, Failure> {
    let target = inside_install(install_path, dependency.target)?;
    let version_file = inside_install(install_path, dependency.version_file)?;
    let current = read_version(&version_file);
    let mut result = DependencyResult {
        id: dependency.id.to_owned(),
        title: dependency.title.to_owned(),
        from: current.clone(),
        to: current.clone(),
        status: Status::UpToDate,
        error: None,
    };

    if dependency
        .pin
        .as_ref()
        .is_some_and(|pin| current.as_deref() == Some(pin.tag))
        && target.exists()
    {
        return Ok(result);
    }

    // Тег сверяется до хеша: стоящий текущий релиз без SHA-256 от GitHub — не ошибка, качать нечего.
    let release = find_release(dependency, client, options.api_root()).await?;
    if current.as_deref() == Some(release.tag.as_str()) && target.exists() {
        return Ok(result);
    }
    let release = require_sha256(release)?;

    let dir = target.parent().unwrap_or(Path::new("."));
    let temp = dir.join(format!(
        ".hlsr-update-{}-{}",
        dependency.id,
        std::process::id()
    ));
    let mut download_name = temp.clone().into_os_string();
    download_name.push(".download");
    let download_file = PathBuf::from(download_name);

    let installed = install(
        dependency,
        &release,
        client,
        options,
        &temp,
        &download_file,
        &target,
        &version_file,
    )
    .await;
    // `finally`: уборка идёт при любом исходе, и её сбой заменяет исходную ошибку.
    let cleanup = rm_sync(&temp, true, 0, Duration::ZERO)
        .and_then(|()| rm_sync(&download_file, false, 0, Duration::ZERO));
    cleanup?;
    installed?;

    logger.info(&format!(
        "[launch] {} обновлён: {} -> {}",
        dependency.title,
        current.as_deref().unwrap_or("неизвестная версия"),
        release.tag
    ));
    result.to = Some(release.tag);
    result.status = Status::Updated;
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
async fn install(
    dependency: &Dependency,
    release: &Release,
    client: &HttpClient,
    options: &UpdateOptions<'_>,
    temp: &Path,
    download_file: &Path,
    target: &Path,
    version_file: &Path,
) -> Result<(), Failure> {
    rm_sync(temp, true, 0, Duration::ZERO)?;
    download(
        release,
        download_file,
        client,
        &options.download_url(&release.url),
    )
    .await?;

    let mut fresh = download_file.to_owned();
    if let Some(extract) = dependency.extract {
        let Some(archive) = options.archive else {
            return Err(DeliveryError::new(
                Code::ExtractFailed,
                "Нет 7-Zip для распаковки обновления",
            )
            .into());
        };
        archive
            .extract(download_file, temp, ExtractOptions::default())
            .await?;
        fresh = inside_install(temp, extract)?;
        if !fresh.exists() {
            return Err(DeliveryError::new(
                Code::ExtractFailed,
                format!("В {} нет {extract}", release.name),
            )
            .into());
        }
    }

    assert_windows_binary(&fresh)?;
    fs::rename(&fresh, target)
        .map_err(|e| RawError::fs_with_dest(&e, "rename", &fresh, Some(target)))?;
    fs::write(version_file, &release.tag).map_err(|e| RawError::fs(&e, "open", version_file))?;
    Ok(())
}

/// `updateDependencies(installPath, config, options)`: обновляет зависимости по очереди. Сбой каждой уходит
/// в её результат, а запуск продолжается; исключением выходит только путь файла версии за папку установки.
pub async fn update_dependencies(
    install_path: &Path,
    dependencies: &[Dependency],
    options: &UpdateOptions<'_>,
) -> Result<Vec<DependencyResult>, DeliveryError> {
    let tracing = TracingLogger;
    let logger: &dyn Logger = options.logger.unwrap_or(&tracing);
    let own_client = (options.client.is_none() && !dependencies.is_empty()).then(HttpClient::new);
    let mut results = Vec::new();
    for dependency in dependencies {
        let client = options
            .client
            .or(own_client.as_ref())
            .expect("клиент есть, когда есть зависимости");
        match update_one(install_path, dependency, client, options, logger).await {
            Ok(result) => results.push(result),
            Err(failure) => {
                let delivery = to_delivery_error(failure, Some(Code::NetworkError));
                logger.warn(&format!(
                    "[launch] {} не обновлён: {} {}",
                    dependency.title,
                    delivery.code.as_str(),
                    delivery.message
                ));
                results.push(DependencyResult {
                    id: dependency.id.to_owned(),
                    title: dependency.title.to_owned(),
                    from: read_version(&inside_install(install_path, dependency.version_file)?),
                    to: None,
                    status: Status::Failed,
                    error: Some((delivery.code, delivery.message)),
                });
            }
        }
    }
    Ok(results)
}
