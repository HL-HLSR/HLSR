//! Канал обновления лаунчера (`launcher-update/launcher-channel.js` и часть про лаунчер
//! `shared/delivery-endpoints.js`): схема подписанного `launcher-channel.json`, allowlist адресов,
//! `compareSemver`, подпись, проверка файла установщика.
//!
//! Windows и Linux подписываются одним ключом и публикуют канал одного формата: установщик
//! (`HLSR-Setup-<версия>.exe` или `HLSR-Setup-<версия>-linux-x86_64.AppImage`), его хеши и
//! SHA-256 `latest.yml`. Клиент принимает только установщик своей ОС. Документ канала — `Value`:
//! незнакомые поля (блок `build`, будущие поля) сохраняются, и подпись покрывает их все.

use crate::cancel::CancelToken;
use crate::errors::LuCode;
use base64::prelude::{BASE64_STANDARD, Engine as _};
use hlsr_core::js::text::hex;
use hlsr_core::js::{self, Platform};
use hlsr_core::manifest_signing;
use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};
use std::cmp::Ordering;
use std::path::Path;
use tokio::io::{AsyncRead, AsyncReadExt};
use url::Url;

pub const SUPPORTED_SCHEMA_VERSION: u64 = 1;

/// Установщик больше этого не принимается (`MAX_INSTALLER_BYTES`).
pub const MAX_INSTALLER_BYTES: u64 = 512 * 1024 * 1024;

/// Хосты, с которых клиент принимает адреса установщика, чанков и канала.
pub const ALLOWED_HOSTS: [&str; 4] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "hlsr.pro",
];

/// `LAUNCHER_PUBLIC_KEYS`: открытые ключи канала (PEM). `keyId` боевого ключа — [`LAUNCHER_KEY_ID`].
pub const LAUNCHER_PUBLIC_KEYS: [&str; 1] = ["-----BEGIN PUBLIC KEY-----\n\
MCowBQYDK2VwAyEACpQQ9X1lGMQofXTJ2EuLFNU8WrYxtPR7TeyDp8bXZfY=\n\
-----END PUBLIC KEY-----\n"];

/// `keyId` боевого ключа: первые 16 шестнадцатеричных знаков sha256 от DER `SubjectPublicKeyInfo`.
pub const LAUNCHER_KEY_ID: &str = "def6be181724142e";

/// Ключи канала по умолчанию в виде, который принимают `check` и шлюз.
pub fn default_public_keys() -> Vec<String> {
    LAUNCHER_PUBLIC_KEYS
        .iter()
        .map(|key| (*key).to_owned())
        .collect()
}

const CHANNEL_FILE: &str = "launcher-channel.json";
const GITHUB_RELEASE_BASE: &str = "https://github.com/HL-HLSR/HLSR-Releases/releases";
/// Куски обновления чанками общие для платформ.
pub const LAUNCHER_CHUNKS_BASE: &str = "https://hlsr.pro/downloads/launcher/chunks";

// ------------------------------------------------------------------- адреса

/// Адреса обновления лаунчера одной ОС (`launcherFor(platform)`).
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

/// `launcherFor(platform)`.
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

    /// `releaseTag(version)`: `v0.5.0` или `linux-v0.5.0`.
    pub fn release_tag(&self, version: &str) -> String {
        format!("{}{version}", self.tag_prefix)
    }

    /// Манифест чанков версии лежит рядом с установщиком. Только на hlsr.pro: в релиз GitHub нельзя
    /// положить тысячи файлов, оттуда клиент берёт полный установщик.
    pub fn chunk_manifest_urls(&self, filename: &str) -> [String; 1] {
        [format!("{}/{filename}", self.primary_base)]
    }

    /// Куски лежат в общем для платформ хранилище.
    pub fn chunk_base_urls(&self) -> [&'static str; 1] {
        [LAUNCHER_CHUNKS_BASE]
    }
}

// ------------------------------------------------------------------- SemVer

/// `parseSemver(version)`: числа — как `Number(...)` JS (дробные при переполнении), пре-релиз — строка.
#[derive(Debug, Clone, PartialEq)]
pub struct Semver {
    pub major: f64,
    pub minor: f64,
    pub patch: f64,
    pub prerelease: Option<String>,
}

fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// `[0-9A-Za-z.-]+`
fn is_prerelease(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
}

/// `^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$` (`\d` JS — только ASCII).
pub fn parse_semver(version: &str) -> Option<Semver> {
    let (core, prerelease) = match version.split_once('-') {
        Some((core, rest)) if is_prerelease(rest) => (core, Some(rest.to_owned())),
        Some(_) => return None,
        None => (version, None),
    };
    let mut parts = core.split('.');
    let (major, minor, patch) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || ![major, minor, patch].iter().all(|part| is_digits(part)) {
        return None;
    }
    let number = |text: &str| text.parse::<f64>().unwrap_or(f64::INFINITY);
    Some(Semver {
        major: number(major),
        minor: number(minor),
        patch: number(patch),
        prerelease,
    })
}

/// `Не SemVer: a / b` — `compareSemver` бросает исключение на любой версии вне формата.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Не SemVer: {0} / {1}")]
pub struct NotSemver(pub String, pub String);

/// `compareSemver(a, b)`: знак результата JS. Пре-релиз сравнивается строкой (не по правилам
/// SemVer): `0.5.0-beta.10 < 0.5.0-beta.2`; версия без пре-релиза старше версии с ним.
pub fn compare_semver(a: &str, b: &str) -> Result<Ordering, NotSemver> {
    let (Some(pa), Some(pb)) = (parse_semver(a), parse_semver(b)) else {
        return Err(NotSemver(a.to_owned(), b.to_owned()));
    };
    for (x, y) in [
        (pa.major, pb.major),
        (pa.minor, pb.minor),
        (pa.patch, pb.patch),
    ] {
        if x != y {
            return Ok(x.partial_cmp(&y).unwrap_or(Ordering::Equal));
        }
    }
    Ok(match (&pa.prerelease, &pb.prerelease) {
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        // Строки из ASCII: единицы UTF-16 и байты упорядочены одинаково.
        (Some(x), Some(y)) => x.cmp(y),
        (None, None) => Ordering::Equal,
    })
}

// ------------------------------------------------------------------- адреса: allowlist

/// `isAllowedUrl(url)`: только `https`, порт 443, без логина, хост из [`ALLOWED_HOSTS`]. Разбор
/// адреса — как у `new URL()` (WHATWG).
pub fn is_allowed_url(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed
            .password()
            .is_some_and(|password| !password.is_empty())
        || parsed.port().is_some_and(|port| port != 443)
    {
        return false;
    }
    parsed
        .host_str()
        .is_some_and(|host| ALLOWED_HOSTS.contains(&host))
}

// ------------------------------------------------------------------- проверка формы

/// Поле объекта JSON; у не-объекта (и массива) полей нет — `undefined`.
pub(crate) fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object().and_then(|object| object.get(key))
}

/// Строка, которую проверит регулярное выражение: `x || ""` приведённое к строке.
pub(crate) fn regex_input(value: Option<&Value>) -> String {
    match value {
        Some(value) if js::truthy(value) => js::to_string(value),
        _ => String::new(),
    }
}

/// `Number.isInteger(value)` для значения JSON.
pub(crate) fn is_integer(value: Option<&Value>) -> Option<f64> {
    let number = value?.as_number()?.as_f64()?;
    (number.is_finite() && number.fract() == 0.0).then_some(number)
}

/// `^[a-f0-9]{64}$`
pub(crate) fn is_sha256(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// `^[A-Za-z0-9+/]{86}==$`
fn is_sha512_base64(text: &str) -> bool {
    text.len() == 88
        && text.ends_with("==")
        && text.as_bytes()[..86]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'+' || *byte == b'/')
}

/// `^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$`
fn is_semver(text: &str) -> bool {
    parse_semver(text).is_some()
}

/// `^\d+\.\d+\.\d+$`
pub(crate) fn is_release_semver(text: &str) -> bool {
    parse_semver(text).is_some_and(|semver| semver.prerelease.is_none())
}

/// `^HLSR-Setup-.+\.exe$`: `.` JS не совпадает с переводами строки.
fn is_windows_installer_name(text: &str) -> bool {
    text.strip_prefix("HLSR-Setup-")
        .and_then(|rest| rest.strip_suffix(".exe"))
        .is_some_and(|middle| {
            !middle.is_empty() && !middle.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
        })
}

/// `validateShape(data, platform)`: причины, по которым документ не канал лаунчера этой ОС.
pub fn validate_shape(data: &Value, platform: Platform) -> Vec<String> {
    let mut problems = Vec::new();
    let target = platform;
    let version_ok = |text: &str| {
        if target == Platform::Linux {
            is_release_semver(text)
        } else {
            is_semver(text)
        }
    };

    if !data.is_object() {
        return vec!["манифест не является JSON-объектом".to_owned()];
    }
    let template = |key: &str| js::to_template_string(field(data, key));

    if is_integer(field(data, "schemaVersion")) != Some(SUPPORTED_SCHEMA_VERSION as f64) {
        problems.push(format!(
            "неподдерживаемая schemaVersion: {}",
            template("schemaVersion")
        ));
    }

    if !version_ok(&regex_input(field(data, "version"))) {
        problems.push(format!("version не SemVer: {}", template("version")));
    }
    let published = field(data, "publishedAt").and_then(Value::as_str);
    if published.is_none_or(|text| js::date_parse(text).is_none()) {
        problems.push("publishedAt отсутствует или не дата".to_owned());
    }
    if !version_ok(&regex_input(field(data, "minimumSupportedVersion"))) {
        problems.push(format!(
            "minimumSupportedVersion не SemVer: {}",
            template("minimumSupportedVersion")
        ));
    }
    if !field(data, "updateRequired").is_some_and(Value::is_boolean) {
        problems.push("updateRequired должен быть boolean".to_owned());
    }

    let installer = field(data, "installer")
        .filter(|installer| matches!(installer, Value::Object(_) | Value::Array(_)));
    let Some(installer) = installer else {
        problems.push("нет блока installer".to_owned());
        return problems;
    };

    let filename = field(installer, "filename");
    let filename_ok = if target == Platform::Linux {
        let expected = format!(
            "HLSR-Setup-{}-linux-x86_64.AppImage",
            js::to_template_string(field(data, "version"))
        );
        filename.and_then(Value::as_str) == Some(expected.as_str())
    } else {
        filename
            .and_then(Value::as_str)
            .is_some_and(is_windows_installer_name)
    };
    if !filename_ok {
        problems.push(format!(
            "installer.filename неожиданный: {}",
            js::to_template_string(filename)
        ));
    }
    let size = is_integer(field(installer, "size"));
    if size.is_none_or(|size| {
        size <= 0.0 || (target == Platform::Linux && size > MAX_INSTALLER_BYTES as f64)
    }) {
        problems.push("installer.size должен быть положительным целым".to_owned());
    }
    if !is_sha256(&regex_input(field(installer, "sha256"))) {
        problems.push("installer.sha256 некорректен".to_owned());
    }
    if !is_sha512_base64(&regex_input(field(installer, "sha512"))) {
        problems.push("installer.sha512 некорректен (base64)".to_owned());
    }

    match field(installer, "urls").and_then(Value::as_array) {
        Some(urls) if !urls.is_empty() => {
            for url in urls {
                if !is_allowed_url(&js::to_string(url)) {
                    problems.push(format!(
                        "installer.urls вне allowlist: {}",
                        js::to_template_string(Some(url))
                    ));
                }
            }
        }
        _ => problems.push("installer.urls пуст".to_owned()),
    }

    if let Some(yml) = field(data, "latestYmlSha256").filter(|value| !value.is_null())
        && !is_sha256(&js::to_string(yml))
    {
        problems.push("latestYmlSha256 некорректен".to_owned());
    }

    problems
}

/// `deltaOf(data)`: блок обновления чанками из канала, если он есть и устроен правильно. Подпись
/// канала покрывает и его; без блока клиент качает установщик. Неправильный блок молча пропускается.
pub fn delta_of(data: &Value) -> Option<&Value> {
    let delta = field(data, "delta").filter(|delta| js::truthy(delta))?;
    crate::chunks::delta_problems(delta, is_allowed_url)
        .is_empty()
        .then_some(delta)
}

// ------------------------------------------------------------------- check

/// Параметры [`check`].
#[derive(Debug, Clone, Default)]
pub struct CheckOptions<'a> {
    /// Установленная версия; пустая или `None` — сравнений нет.
    pub current_version: Option<&'a str>,
    /// Доверенные ключи (PEM); `None` — [`LAUNCHER_PUBLIC_KEYS`].
    pub public_keys: Option<&'a [String]>,
    /// ОС канала; `None` — та, под которую собран лаунчер.
    pub platform: Option<Platform>,
}

/// Итог [`check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelCheck {
    pub ok: bool,
    pub problems: Vec<String>,
    pub update_available: bool,
    pub downgrade: bool,
    pub unsupported: bool,
    pub code: Option<LuCode>,
}

/// `check(data, options)`: форма, подпись, затем сравнение с установленной версией. Бросает
/// [`NotSemver`], если установленная версия не SemVer (как исключение `compareSemver`).
pub fn check(data: &Value, options: &CheckOptions<'_>) -> Result<ChannelCheck, NotSemver> {
    let defaults;
    let public_keys = match options.public_keys {
        Some(keys) => keys,
        None => {
            defaults = default_public_keys();
            &defaults
        }
    };
    let platform = options.platform.unwrap_or(Platform::CURRENT);

    let mut problems = validate_shape(data, platform);

    if problems.is_empty() {
        let signature = field(data, manifest_signing::SIGNATURE_FIELD).unwrap_or(&Value::Null);
        let signature_ok = public_keys
            .iter()
            .any(|pem| manifest_signing::verify(data, signature, pem));
        if !signature_ok {
            problems.push("подпись Ed25519 не прошла проверку".to_owned());
        }
    }

    let mut update_available = false;
    let mut downgrade = false;
    let mut unsupported = false;

    if let Some(current) = options
        .current_version
        .filter(|current| !current.is_empty())
        && problems.is_empty()
    {
        let version = regex_input(field(data, "version"));
        let minimum = regex_input(field(data, "minimumSupportedVersion"));
        let compared = compare_semver(&version, current)?;
        update_available = compared == Ordering::Greater;
        downgrade = compared == Ordering::Less;

        if compare_semver(current, &minimum)? == Ordering::Less {
            unsupported = true;
            problems.push(format!(
                "установленная версия {current} ниже минимально поддерживаемой \
                 {minimum} — требуется установка вручную"
            ));
        }
    }

    let code = if problems.is_empty() {
        None
    } else if unsupported {
        Some(LuCode::VersionUnsupported)
    } else {
        Some(LuCode::SchemaInvalid)
    };
    Ok(ChannelCheck {
        ok: problems.is_empty(),
        problems,
        update_available,
        downgrade,
        unsupported,
        code,
    })
}

// ------------------------------------------------------------------- проверка файла

/// Параметры [`verify_installer_stream`].
#[derive(Debug, Clone, Default)]
pub struct VerifyOptions {
    /// Предел размера; `None` или 0 — [`MAX_INSTALLER_BYTES`].
    pub max_bytes: Option<u64>,
    /// `signal`: отмена чтения.
    pub cancel: Option<CancelToken>,
}

/// Итог проверки установщика: `{ok, problems, code}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyOutcome {
    pub ok: bool,
    pub problems: Vec<String>,
    pub code: Option<LuCode>,
}

impl VerifyOutcome {
    pub(crate) fn failed(code: LuCode, problem: impl Into<String>) -> VerifyOutcome {
        VerifyOutcome {
            ok: false,
            problems: vec![problem.into()],
            code: Some(code),
        }
    }

    fn passed() -> VerifyOutcome {
        VerifyOutcome {
            ok: true,
            problems: Vec::new(),
            code: None,
        }
    }
}

/// Размер из блока `installer` как число JS (`stat.size !== installer.size`).
fn expected_size(installer: &Value) -> Option<f64> {
    field(installer, "size")?.as_number()?.as_f64()
}

/// `verifyInstallerStream(filePath, installer, options)`: размер (не больше предела), затем sha256
/// и sha512 одним проходом по файлу. Файл читается кусками по 64 КиБ, как `createReadStream`.
pub async fn verify_installer_stream(
    path: &Path,
    installer: &Value,
    options: &VerifyOptions,
) -> VerifyOutcome {
    let display = path.display().to_string();
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return VerifyOutcome::failed(
                LuCode::InstallerUnreadable,
                format!(
                    "файл не читается: {}",
                    js::errno::fs_message(&error, "stat", &display, None)
                ),
            );
        }
    };
    let size = metadata.len();
    let max_bytes = options
        .max_bytes
        .filter(|max| *max != 0)
        .unwrap_or(MAX_INSTALLER_BYTES);
    if size > max_bytes {
        return VerifyOutcome::failed(
            LuCode::InstallerTooBig,
            format!("файл {size} байт больше допустимых {max_bytes}"),
        );
    }
    if expected_size(installer) != Some(size as f64) {
        return VerifyOutcome::failed(
            LuCode::InstallerSize,
            format!(
                "размер {size} != {}",
                js::to_template_string(field(installer, "size"))
            ),
        );
    }
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) => {
            return VerifyOutcome::failed(
                LuCode::InstallerUnreadable,
                format!(
                    "ошибка чтения: {}",
                    js::errno::fs_message(&error, "open", &display, None)
                ),
            );
        }
    };
    verify_installer_reader(file, size, installer, options.cancel.as_ref()).await
}

/// Чтение и сверка хешей для потока, про который известно, что в нём `size` байт (по `stat`).
/// Поток длиннее — «файл изменился во время проверки», короче — «прочитано N из M».
pub async fn verify_installer_reader<R: AsyncRead + Unpin>(
    mut reader: R,
    size: u64,
    installer: &Value,
    cancel: Option<&CancelToken>,
) -> VerifyOutcome {
    let mut sha256 = Sha256::new();
    let mut sha512 = Sha512::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut read: u64 = 0;
    loop {
        let count = match cancel {
            Some(cancel) => tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    return VerifyOutcome::failed(
                        LuCode::InstallerUnreadable,
                        "ошибка чтения: The operation was aborted",
                    );
                }
                result = reader.read(&mut buffer) => result,
            },
            None => reader.read(&mut buffer).await,
        };
        let count = match count {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) => {
                return VerifyOutcome::failed(
                    LuCode::InstallerUnreadable,
                    format!("ошибка чтения: {error}"),
                );
            }
        };
        read += count as u64;
        if read > size {
            return VerifyOutcome::failed(
                LuCode::InstallerSize,
                format!("файл изменился во время проверки ({read} > {size})"),
            );
        }
        sha256.update(&buffer[..count]);
        sha512.update(&buffer[..count]);
    }

    if expected_size(installer) != Some(read as f64) {
        return VerifyOutcome::failed(
            LuCode::InstallerSize,
            format!(
                "прочитано {read} из {}",
                js::to_template_string(field(installer, "size"))
            ),
        );
    }
    if field(installer, "sha256").and_then(Value::as_str) != Some(hex(&sha256.finalize()).as_str())
    {
        return VerifyOutcome::failed(LuCode::InstallerSha256, "SHA-256 не совпал");
    }
    if field(installer, "sha512").and_then(Value::as_str)
        != Some(BASE64_STANDARD.encode(sha512.finalize()).as_str())
    {
        return VerifyOutcome::failed(LuCode::InstallerSha512, "SHA-512 не совпал");
    }
    VerifyOutcome::passed()
}
