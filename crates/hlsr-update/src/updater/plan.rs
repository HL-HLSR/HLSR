//! План внешнего апдейтера (`updater/core/plan.js`): `update-plan.json`, который лаунчер кладёт
//! рядом с копией апдейтера, проверяется целиком до того, как что-либо скачивается. Подпись канала,
//! версия, имя установщика ровно `HLSR-Setup-<версия>.exe` (и последний сегмент каждого адреса),
//! каталог установки с сегментом `HLSR*`, `HLSR.exe` в нём, `version-state.json` для квитанции.
//!
//! Пути — `path.win32` на любой ОС: план пишет только лаунчер Windows. Каталог установки без
//! сегмента `HLSR*` (`D:\Games\Launcher`) у 0.4.7 даёт `UPD_BAD_PLAN` (карта §0.5); здесь он
//! годится, если в нём маркер владельца установки HLSR ([`is_owned_launcher_dir`]).

use crate::channel::{self, CheckOptions};
use crate::errors::{LuCode, UpdCode, UpdaterCode, UpdaterError};
use crate::windows::layout::win_path;
use hlsr_core::js::{self, Platform};
use serde_json::{Map, Value, json};

/// `expectedInstallerName(version)`.
pub fn expected_installer_name(version: &str) -> String {
    format!("HLSR-Setup-{version}.exe")
}

/// `isPlausibleLauncherDir(dir)`: строка, абсолютный путь Windows, без `..`, не UNC, и какой-то
/// сегмент начинается с `HLSR` без учёта регистра ASCII (`/(^|[\\/])HLSR[^\\/]*([\\/]|$)/i`).
pub fn is_plausible_launcher_dir(dir: &Value) -> bool {
    let Some(dir) = dir.as_str() else {
        return false;
    };
    is_local_absolute(dir)
        && dir.split(['\\', '/']).any(|segment| {
            segment
                .get(..4)
                .is_some_and(|head| head.eq_ignore_ascii_case("HLSR"))
        })
}

/// Абсолютный путь Windows без `..` и не UNC.
fn is_local_absolute(dir: &str) -> bool {
    !dir.is_empty()
        && win_path().is_absolute(dir)
        && !dir.contains("..")
        && !dir.starts_with("\\\\")
}

/// Каталог установленного HLSR без сегмента `HLSR*` (`D:\Games\Launcher`, `E:\Игры\Лаунчер`):
/// в нём маркер владельца, который пишет только установщик HLSR. Отступление от 0.4.9: там такой
/// каталог давал `UPD_BAD_PLAN`, и лаунчер, поставленный не в `…\HLSR…`, не обновлялся.
pub fn is_owned_launcher_dir(dir: &Value) -> bool {
    dir.as_str().is_some_and(|dir| {
        is_local_absolute(dir)
            && std::path::Path::new(dir)
                .join(crate::windows::layout::OWNER_MARKER)
                .is_file()
    })
}

/// Параметры [`validate`] (`options`): ключи канала и адрес QA-ленты из `QA_TRUST`.
#[derive(Debug, Clone, Default)]
pub struct ValidateOptions {
    /// Доверенные ключи (PEM); `None` — боевые [`channel::LAUNCHER_PUBLIC_KEYS`].
    pub public_keys: Option<Vec<String>>,
    /// `downloadBase`: установщик качается с `<база>/HLSR-Setup-<версия>.exe` вместо адресов канала.
    pub download_base: Option<String>,
}

/// Установщик проверенного плана.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanInstaller {
    pub filename: String,
    pub size: Value,
    pub sha256: Value,
    pub sha512: Value,
    pub urls: Vec<String>,
}

/// Проверенный план (итог `validate`).
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Подписанный канал целиком.
    pub manifest: Value,
    /// `currentVersion` плана как есть (`None` — поля нет).
    pub current_version: Option<Value>,
    pub target_version: String,
    pub installer: PlanInstaller,
    pub install_dir: String,
    pub launcher_exe: String,
    pub receipt_file: String,
    /// `ru` или `en`.
    pub language: String,
}

impl Plan {
    /// Форма 0.4.7: поля в том же порядке, отсутствующее `currentVersion` не пишется.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("manifest".into(), self.manifest.clone());
        if let Some(current) = &self.current_version {
            out.insert("currentVersion".into(), current.clone());
        }
        out.insert("targetVersion".into(), json!(self.target_version));
        out.insert(
            "installer".into(),
            json!({
                "filename": self.installer.filename,
                "size": self.installer.size,
                "sha256": self.installer.sha256,
                "sha512": self.installer.sha512,
                "urls": self.installer.urls,
            }),
        );
        out.insert("installDir".into(), json!(self.install_dir));
        out.insert("launcherExe".into(), json!(self.launcher_exe));
        out.insert("receiptFile".into(), json!(self.receipt_file));
        out.insert("language".into(), json!(self.language));
        Value::Object(out)
    }

    /// Установленная версия строкой (`currentVersion` — строка в плане лаунчера).
    pub fn current_version_str(&self) -> Option<&str> {
        self.current_version.as_ref().and_then(Value::as_str)
    }
}

fn bad_plan(message: &str) -> UpdaterError {
    UpdaterError::new(UpdCode::BadPlan, message)
}

/// `decodeURIComponent`: экранированные октеты должны сложиться в UTF-8, иначе `URI malformed`.
fn decode_uri_component(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Значение для `currentVersion` проверки канала: `if (currentVersion)` — ложные значения JS не
/// сравниваются; прочие — их строка (`Не SemVer: …` у не-версий).
fn current_for_check(value: Option<&Value>) -> Option<String> {
    value
        .filter(|value| js::truthy(value))
        .map(|value| js::to_template_string(Some(value)))
}

/// `validate(raw, options)`: проверенный план или ошибка `UPD_*`/`LU_*`. Исключение проверки
/// канала (установленная версия не SemVer) — `UPD_BAD_PLAN` с его текстом, как ловит его
/// `updater/main.js` (`toUpdaterError(error, BAD_PLAN)`).
pub fn validate(raw: &Value, options: &ValidateOptions) -> Result<Plan, UpdaterError> {
    let Some(object) = raw.as_object() else {
        return Err(bad_plan("план не является объектом"));
    };

    let manifest = object.get("manifest").unwrap_or(&Value::Null);
    if !js::truthy(manifest) || !(manifest.is_object() || manifest.is_array()) {
        return Err(bad_plan("в плане нет манифеста канала"));
    }
    let current_version = object.get("currentVersion");
    let target_version = object.get("targetVersion");

    let current = current_for_check(current_version);
    let defaults = channel::default_public_keys();
    let verdict = channel::check(
        manifest,
        &CheckOptions {
            current_version: current.as_deref(),
            public_keys: Some(options.public_keys.as_deref().unwrap_or(&defaults)),
            platform: Some(Platform::Win32),
        },
    )
    .map_err(|error| bad_plan(&error.to_string()))?;

    if !verdict.ok {
        let joined = verdict.problems.join("; ");
        let code: UpdaterCode = match verdict.code {
            Some(LuCode::SchemaInvalid) if joined.contains("подпись") => {
                UpdCode::SignatureInvalid.into()
            }
            Some(code) => code.into(),
            None => UpdCode::SignatureInvalid.into(),
        };
        return Err(UpdaterError::new(code, joined));
    }

    let version = manifest["version"].as_str().unwrap_or_default().to_owned();
    if target_version.is_none_or(|target| !js::truthy(target) || target.as_str() != Some(&version))
    {
        return Err(UpdaterError::new(
            UpdCode::VersionMismatch,
            format!(
                "план обещает {}, подписано {version}",
                js::to_template_string(target_version)
            ),
        ));
    }

    if !verdict.update_available {
        return Err(UpdaterError::new(
            UpdCode::VersionMismatch,
            format!(
                "{version} не новее установленной {}",
                js::to_template_string(current_version)
            ),
        ));
    }

    let want_name = expected_installer_name(&version);
    let installer = &manifest["installer"];
    let filename = installer["filename"].as_str().unwrap_or_default();
    if filename != want_name {
        return Err(UpdaterError::new(
            UpdCode::FilenameMismatch,
            format!("installer.filename={filename}, ожидалось {want_name}"),
        ));
    }

    let mut urls = Vec::new();
    for url in installer["urls"].as_array().into_iter().flatten() {
        let url = js::to_string(url);
        if !channel::is_allowed_url(&url) {
            return Err(UpdaterError::new(
                UpdCode::UrlNotAllowed,
                format!("URL вне allowlist: {url}"),
            ));
        }
        let Ok(parsed) = url::Url::parse(&url) else {
            return Err(UpdaterError::new(
                UpdCode::UrlNotAllowed,
                "неразбираемый URL",
            ));
        };
        let last = parsed.path().rsplit('/').next().unwrap_or("");
        let Some(last) = decode_uri_component(last) else {
            return Err(bad_plan("URI malformed"));
        };
        if last != want_name {
            return Err(UpdaterError::new(
                UpdCode::FilenameMismatch,
                format!("URL указывает на {last}, ожидалось {want_name}"),
            ));
        }
        urls.push(url);
    }
    if urls.is_empty() {
        return Err(bad_plan("в манифесте нет URL установщика"));
    }

    let install_dir = object.get("installDir").unwrap_or(&Value::Null);
    if !is_plausible_launcher_dir(install_dir) && !is_owned_launcher_dir(install_dir) {
        return Err(bad_plan("installDir не похож на каталог HLSR"));
    }
    let install_dir = install_dir.as_str().unwrap_or_default();

    let path = win_path();
    let launcher_exe = object.get("launcherExe").and_then(Value::as_str);
    let exe_ok = launcher_exe.is_some_and(|exe| {
        path.is_absolute(exe)
            && exe
                .get(exe.len().saturating_sub(4)..)
                .is_some_and(|ext| ext.eq_ignore_ascii_case(".exe"))
            && path.dirname(exe).to_lowercase() == install_dir.to_lowercase()
    });
    let Some(launcher_exe) = launcher_exe.filter(|_| exe_ok) else {
        return Err(bad_plan("launcherExe вне каталога установки"));
    };

    let receipt_file = object.get("receiptFile").and_then(Value::as_str);
    let Some(receipt_file) = receipt_file
        .filter(|file| path.is_absolute(file) && path.basename(file) == "version-state.json")
    else {
        return Err(bad_plan("receiptFile недопустим"));
    };

    let download_urls = match options
        .download_base
        .as_deref()
        .filter(|base| !base.is_empty())
    {
        Some(base) => vec![format!("{}/{want_name}", base.trim_end_matches('/'))],
        None => urls,
    };

    Ok(Plan {
        manifest: manifest.clone(),
        current_version: current_version.cloned(),
        target_version: version,
        installer: PlanInstaller {
            filename: want_name,
            size: installer["size"].clone(),
            sha256: installer["sha256"].clone(),
            sha512: installer["sha512"].clone(),
            urls: download_urls,
        },
        install_dir: install_dir.to_owned(),
        launcher_exe: launcher_exe.to_owned(),
        receipt_file: receipt_file.to_owned(),
        language: if object.get("language").and_then(Value::as_str) == Some("en") {
            "en"
        } else {
            "ru"
        }
        .to_owned(),
    })
}
