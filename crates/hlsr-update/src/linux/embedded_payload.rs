//! Вшитый архив лаунчера (`launcher-update/embedded-payload.js` 0.4.7): установщик AppImage везёт
//! `resources/payload/` — `payload.json` (версия, размер, SHA-256, SHA-512, распакованный размер) и
//! `HLSR-<версия>-linux-x86_64.tar.gz`. Установщик ставит его без сети; служба обновления качает
//! AppImage по подписанному каналу, сверяет хеши и достаёт тот же архив рантаймом самого AppImage
//! (`--appimage-extract resources/payload`, без FUSE).

use super::{EnvVars, SyncRunner};
use crate::cancel::CancelToken;
use crate::channel::{field, is_sha256};
use crate::updater::processes::{RunExit, Spawn};
use hlsr_core::js;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Каталог вшитого архива в ресурсах (`DIRECTORY`).
pub const DIRECTORY: &str = "payload";
/// Описание (`DESCRIPTOR`).
pub const DESCRIPTOR: &str = "payload.json";
pub const SCHEMA: u64 = 1;
/// Предел распакованного размера (2 ГиБ).
pub const MAX_UNPACKED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Сколько ждать `--appimage-extract`.
pub const EXTRACT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Переменные AppImage, которые не уходят в рантайм скачанного установщика.
pub const EXTRACT_DROPPED: [&str; 5] = [
    "APPIMAGE",
    "APPDIR",
    "ARGV0",
    "OWD",
    "APPIMAGE_EXTRACT_AND_RUN",
];

/// `HLSR-<версия>-linux-x86_64.tar.gz`.
pub fn archive_name(version: &str) -> String {
    format!("HLSR-{version}-linux-x86_64.tar.gz")
}

/// `<resources>/payload`.
pub fn payload_dir(resources: &Path) -> PathBuf {
    resources.join(DIRECTORY)
}

/// Что описывает `payload.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Describe<'a> {
    pub version: &'a str,
    pub size: u64,
    pub sha256: &'a str,
    pub sha512: &'a str,
    pub unpacked_size: u64,
}

/// `describe(...)`: `{schema, version, installer: {format, filename, size, sha256, sha512, unpackedSize}}`.
pub fn describe(input: &Describe<'_>) -> Value {
    json!({
        "schema": SCHEMA,
        "version": input.version,
        "installer": {
            "format": "tar.gz",
            "filename": archive_name(input.version),
            "size": input.size,
            "sha256": input.sha256,
            "sha512": input.sha512,
            "unpackedSize": input.unpacked_size,
        },
    })
}

/// `Number.isSafeInteger(x) && x > 0`.
fn positive_safe_integer(value: Option<&Value>) -> Option<f64> {
    let number = value?.as_number()?.as_f64()?;
    (number.is_finite()
        && number.fract() == 0.0
        && number.abs() <= 9_007_199_254_740_991.0
        && number > 0.0)
        .then_some(number)
}

/// `x || ""` как строка для регулярного выражения.
fn regex_input(value: Option<&Value>) -> String {
    crate::channel::regex_input(value)
}

/// `^\d+\.\d+\.\d+$` (`\d` — только ASCII).
fn is_version(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

/// `^[A-Za-z0-9+/]{86}==$`
fn is_sha512_base64(text: &str) -> bool {
    text.len() == 88
        && text.ends_with("==")
        && text.as_bytes()[..86]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'+' || *byte == b'/')
}

/// `problemsOf(data)`: что не так с описанием; пусто — описание годно.
pub fn problems_of(data: &Value) -> Vec<String> {
    let schema = field(data, "schema")
        .and_then(Value::as_number)
        .and_then(serde_json::Number::as_f64);
    if !data.is_object() || schema != Some(SCHEMA as f64) {
        return vec!["неизвестная схема описания".to_owned()];
    }
    let version = field(data, "version");
    if !is_version(&regex_input(version)) {
        return vec![format!(
            "некорректная версия: {}",
            js::to_template_string(version)
        )];
    }
    let Some(installer) =
        field(data, "installer").filter(|value| value.is_object() || value.is_array())
    else {
        return vec!["нет описания архива".to_owned()];
    };
    let version = js::to_string(version.expect("версия проверена"));
    let installer_field = |key: &str| field(installer, key);
    let mut problems = Vec::new();
    if installer_field("format").and_then(Value::as_str) != Some("tar.gz") {
        problems.push("ожидался архив tar.gz".to_owned());
    }
    if installer_field("filename").and_then(Value::as_str) != Some(archive_name(&version).as_str())
    {
        problems.push(format!(
            "неожиданное имя архива: {}",
            js::to_template_string(installer_field("filename"))
        ));
    }
    if positive_safe_integer(installer_field("size")).is_none() {
        problems.push("некорректный размер архива".to_owned());
    }
    if !is_sha256(&regex_input(installer_field("sha256"))) {
        problems.push("некорректный SHA-256".to_owned());
    }
    if !is_sha512_base64(&regex_input(installer_field("sha512"))) {
        problems.push("некорректный SHA-512".to_owned());
    }
    if positive_safe_integer(installer_field("unpackedSize"))
        .is_none_or(|size| size > MAX_UNPACKED_BYTES as f64)
    {
        problems.push("некорректный распакованный размер".to_owned());
    }
    problems
}

/// Прочитанное описание: `manifest` — `{version, installer}` (в том виде, что в файле), `archive` — путь
/// к архиву рядом с описанием.
#[derive(Debug, Clone, PartialEq)]
pub struct Payload {
    pub manifest: Value,
    pub archive: PathBuf,
}

impl Payload {
    /// Версия из описания.
    pub fn version(&self) -> String {
        js::to_string(&self.manifest["version"])
    }
}

/// `read(directory)`: описание из `<каталог>/payload.json`. Ошибка — текст для окна установщика.
pub fn read(directory: &Path) -> Result<Payload, String> {
    let file = directory.join(DESCRIPTOR);
    let text = std::fs::read(&file).map_err(|error| {
        format!(
            "В установщике нет встроенного HLSR: {}",
            super::fs_error(&error, "open", &file)
        )
    })?;
    let data = hlsr_core::json_text::json_parse(&String::from_utf8_lossy(&text))
        .map_err(|error| format!("В установщике нет встроенного HLSR: {error}"))?;
    let problems = problems_of(&data);
    if !problems.is_empty() {
        return Err(format!(
            "Описание встроенного HLSR повреждено: {}",
            problems.join("; ")
        ));
    }
    let mut manifest = Map::new();
    manifest.insert("version".into(), data["version"].clone());
    manifest.insert("installer".into(), data["installer"].clone());
    let filename = js::to_string(&data["installer"]["filename"]);
    Ok(Payload {
        manifest: Value::Object(manifest),
        archive: directory.join(filename),
    })
}

/// `extractEnvironment(env)`: окружение без переменных AppImage, из которого запущен лаунчер.
pub fn extract_environment(source: &[(String, String)]) -> EnvVars {
    source
        .iter()
        .filter(|(name, _)| !EXTRACT_DROPPED.contains(&name.as_str()))
        .cloned()
        .collect()
}

/// `extractFrom(appImage, directory, {signal})`: `chmod 0700`, затем `<AppImage> --appimage-extract
/// resources/payload` с `cwd` в каталоге загрузки и чистым окружением, не дольше 10 мин; описание из
/// `<каталог>/squashfs-root/resources/payload`. Вызывать только для AppImage, сверенного с каналом.
/// `env` — окружение лаунчера (уже без `SLINT_*`/`WINIT_*`, см. `updater::processes::child_env`).
/// Отмена (`cancel`) снимает рантайм сразу, как `execFile(..., {signal})`: «Не удалось распаковать
/// установщик: The operation was aborted».
pub fn extract_from(
    app_image: &Path,
    directory: &Path,
    env: &[(String, String)],
    runner: &dyn SyncRunner,
    cancel: Option<&CancelToken>,
) -> Result<Payload, String> {
    super::chmod(app_image, 0o700).map_err(|error| super::fs_error(&error, "chmod", app_image))?;
    if cancel.is_some_and(CancelToken::is_cancelled) {
        return Err(format!(
            "Не удалось распаковать установщик: {}",
            super::RUN_ABORTED
        ));
    }
    let command = Spawn {
        file: app_image.to_owned(),
        args: vec![
            "--appimage-extract".to_owned(),
            format!("resources/{DIRECTORY}"),
        ],
        cwd: Some(directory.to_owned()),
        env: extract_environment(env),
        hidden: true,
        detached: false,
    };
    match runner.run(&command, EXTRACT_TIMEOUT, cancel) {
        Ok(RunExit::Exited(Some(0))) => {}
        Ok(RunExit::Exited(Some(code))) => {
            return Err(format!(
                "Не удалось распаковать установщик: Command failed: {} --appimage-extract resources/{DIRECTORY} (код {code})",
                app_image.display()
            ));
        }
        Ok(RunExit::Exited(None)) | Ok(RunExit::TimedOut) => {
            return Err(format!(
                "Не удалось распаковать установщик: Command failed: {} --appimage-extract resources/{DIRECTORY} (снят)",
                app_image.display()
            ));
        }
        Err(error) => return Err(format!("Не удалось распаковать установщик: {error}")),
    }
    read(
        &directory
            .join("squashfs-root")
            .join("resources")
            .join(DIRECTORY),
    )
}
