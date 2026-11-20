//! Доверие к метаданным сборки (`build-metadata-trust.js`): при установке файл на диске должен
//! совпасть с описанием в подписанном манифесте (`descriptor`), записью в файловом манифесте и
//! версией лаунчера; при запуске — со снимком, который лежит в `installed.json`. Причины
//! недоверия — `BUILD_METADATA_MISSING`, `…_INVALID`, `…_MISMATCH`, `…_UNSUPPORTED`,
//! `BUILD_REQUIRES_NEWER_LAUNCHER`.
//!
//! Чего нет против 0.4.7: запись файлового манифеста не-объект (`null` в списке) не вызывает
//! `TypeError` — она не совпадает; манифест, разобранный и проверенный раньше, такой записи не содержит.

use crate::build_metadata::{
    BuildMetadata, Profile, RELATIVE_PATH, REQUIRED_FIELDS, SCHEMA_VERSION, normalize_built_at,
    now_ms,
};
use crate::build_metadata_reader::{self as reader, ReadError, ReadOptions, Reading};
use crate::js_value as jv;
use crate::semver;
use crate::timing::iso_from_ms;
use hlsr_core::js;
use serde_json::{Map, Value, json};
use std::path::Path;

/// Причина недоверия (`REASONS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// `BUILD_METADATA_MISSING`: обязательного файла нет.
    Missing,
    /// `BUILD_METADATA_INVALID`.
    Invalid,
    /// `BUILD_METADATA_MISMATCH`: файл не совпал с манифестом или снимком.
    Mismatch,
    /// `BUILD_METADATA_UNSUPPORTED`: версия схемы не та.
    Unsupported,
    /// `BUILD_REQUIRES_NEWER_LAUNCHER`.
    RequiresNewerLauncher,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Missing => "BUILD_METADATA_MISSING",
            Reason::Invalid => "BUILD_METADATA_INVALID",
            Reason::Mismatch => "BUILD_METADATA_MISMATCH",
            Reason::Unsupported => "BUILD_METADATA_UNSUPPORTED",
            Reason::RequiresNewerLauncher => "BUILD_REQUIRES_NEWER_LAUNCHER",
        }
    }
}

/// `ALL_REASONS` в порядке `REASONS`.
pub const ALL_REASONS: [Reason; 5] = [
    Reason::Missing,
    Reason::Invalid,
    Reason::Mismatch,
    Reason::Unsupported,
    Reason::RequiresNewerLauncher,
];

/// `BuildMetadataTrustError`: причина, текст для игрока и подробности.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct BuildMetadataTrustError {
    pub code: Reason,
    pub message: String,
    pub detail: Option<Value>,
}

impl BuildMetadataTrustError {
    /// `toPublic()`: `{code, message, detail}`.
    pub fn to_public(&self) -> Value {
        json!({
            "code": self.code.as_str(),
            "message": self.message,
            "detail": self.detail.clone().unwrap_or(Value::Null),
        })
    }
}

fn trust_error(
    code: Reason,
    message: impl Into<String>,
    detail: Option<Value>,
) -> BuildMetadataTrustError {
    let message = message.into();
    BuildMetadataTrustError {
        code,
        message: if message.is_empty() {
            code.as_str().to_owned()
        } else {
            message
        },
        detail: detail.filter(js::truthy),
    }
}

/// `mapReaderError(error, detail)`: версия схемы — `UNSUPPORTED`, всё прочее — `INVALID`.
fn map_reader_error(error: &ReadError, detail: Value) -> BuildMetadataTrustError {
    let code = if error.is_unsupported() {
        Reason::Unsupported
    } else {
        Reason::Invalid
    };
    let message = match error.message() {
        "" => "Метаданные сборки повреждены",
        message => message,
    };
    trust_error(code, message, Some(detail))
}

/// `prop` с отсутствием как `undefined`: `null` у JS — значение, а не отсутствие.
fn present<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    jv::prop(value, name)
}

/// `value != null`.
fn not_nullish(value: Option<&Value>) -> Option<&Value> {
    value.filter(|value| !value.is_null())
}

/// `validateDescriptor(descriptor)`: объект с `path` и `schemaVersion` как у 0.4.7.
pub fn validate_descriptor(descriptor: &Value) -> Result<&Value, BuildMetadataTrustError> {
    if !descriptor.is_object() {
        return Err(trust_error(
            Reason::Invalid,
            "Некорректный descriptor buildMetadata",
            None,
        ));
    }

    if !jv::is_str(present(descriptor, "path"), RELATIVE_PATH) {
        let mut detail = Map::new();
        detail.insert("expected".to_owned(), json!(RELATIVE_PATH));
        if let Some(actual) = present(descriptor, "path") {
            detail.insert("actual".to_owned(), actual.clone());
        }
        return Err(trust_error(
            Reason::Invalid,
            format!(
                "descriptor.path должен быть строго \"{RELATIVE_PATH}\", получено \"{}\"",
                jv::template(present(descriptor, "path"))
            ),
            Some(Value::Object(detail)),
        ));
    }

    let schema_ok = present(descriptor, "schemaVersion")
        .and_then(jv::number)
        .is_some_and(|n| n == SCHEMA_VERSION as f64);
    if !schema_ok {
        return Err(trust_error(
            Reason::Unsupported,
            format!("descriptor.schemaVersion должен быть {SCHEMA_VERSION}"),
            None,
        ));
    }

    Ok(descriptor)
}

/// Параметры `verifyForInstall` (`options`).
#[derive(Debug, Clone, Copy)]
pub struct VerifyForInstall<'a> {
    /// `installRoot`.
    pub install_root: &'a Path,
    /// `descriptor` из подписанного манифеста игры (`buildMetadata`); `null` — как отсутствие.
    pub descriptor: &'a Value,
    /// `manifest`: манифест игры (`gameId`, `version`, `publishedAt`); ложное значение — `{}`.
    pub manifest: Option<&'a Value>,
    /// `filesManifest`: проверенный файловый манифест; без него запись о файле не сверяется.
    pub files_manifest: Option<&'a Value>,
    /// `launcherVersion`: версия лаунчера; без неё совместимость не проверяется.
    pub launcher_version: Option<&'a str>,
    /// `expected`: по умолчанию — [`Profile::product`].
    pub expected: Option<&'a Profile>,
    /// `now`: «сейчас» в миллисекундах.
    pub now: Option<f64>,
}

/// Результат `verifyForInstall` (`state: "trusted"`, `reason: null`).
#[derive(Debug, Clone, PartialEq)]
pub struct Verified {
    pub metadata: BuildMetadata,
    /// Запись файлового манифеста о файле метаданных (`filesEntry`).
    pub files_entry: Option<Value>,
    pub file_sha256: String,
    pub file_size: u64,
    /// `verifiedAt`: `toISOString()`.
    pub verified_at: String,
    /// `publishedAt` манифеста или `null`.
    pub published_at: Value,
}

/// `verifyForInstall(options)`.
pub fn verify_for_install(options: &VerifyForInstall) -> Result<Verified, BuildMetadataTrustError> {
    let descriptor = validate_descriptor(options.descriptor)?;
    let empty = Value::Object(Map::new());
    let manifest = options
        .manifest
        .filter(|manifest| js::truthy(manifest))
        .unwrap_or(&empty);

    let found = match reader::read(
        options.install_root,
        &ReadOptions {
            expected: options.expected,
            now: options.now,
        },
    ) {
        Ok(Reading::Present(found)) => found,
        Ok(Reading::Absent) => {
            return Err(trust_error(
                Reason::Missing,
                format!("Обязательный файл метаданных {RELATIVE_PATH} отсутствует"),
                Some(json!({"relativePath": RELATIVE_PATH})),
            ));
        }
        Err(error) => {
            return Err(map_reader_error(&error, json!({"phase": "install-read"})));
        }
    };

    let dto = &found.dto;

    let mut files_entry = None;
    if let Some(files) = options
        .files_manifest
        .and_then(|files_manifest| jv::array(present(files_manifest, "files")))
    {
        let entry = files
            .iter()
            .find(|file| jv::is_str(present(file, "path"), RELATIVE_PATH))
            .ok_or_else(|| {
                trust_error(
                    Reason::Mismatch,
                    format!("Файл {RELATIVE_PATH} не входит в проверяемый files manifest"),
                    Some(json!({"relativePath": RELATIVE_PATH})),
                )
            })?;

        let size = present(entry, "size");
        let sha256 = present(entry, "sha256");
        let same = size.and_then(jv::number) == Some(found.size_bytes as f64)
            && sha256.and_then(Value::as_str) == Some(found.sha256.as_str());
        if !same {
            let mut expected = Map::new();
            for (name, value) in [("size", size), ("sha256", sha256)] {
                if let Some(value) = value {
                    expected.insert(name.to_owned(), value.clone());
                }
            }
            return Err(trust_error(
                Reason::Mismatch,
                "Метаданные на диске не совпали с записью в files manifest (size/sha256)",
                Some(json!({
                    "expected": expected,
                    "actual": {"size": found.size_bytes, "sha256": found.sha256},
                })),
            ));
        }
        files_entry = Some(entry.clone());
    }

    assert_consistent_with_manifest(dto, descriptor, manifest)?;

    assert_launcher_compatible(dto, options.launcher_version)?;

    let now = options.now.unwrap_or_else(now_ms);
    Ok(Verified {
        metadata: found.dto.clone(),
        files_entry,
        file_sha256: found.sha256.clone(),
        file_size: found.size_bytes,
        verified_at: iso_from_ms(now as i64),
        published_at: present(manifest, "publishedAt")
            .filter(|value| js::truthy(value))
            .cloned()
            .unwrap_or(Value::Null),
    })
}

/// `assertConsistentWithManifest(dto, descriptor, manifest)`: поля описания, `gameId` и `version`
/// манифеста — если заданы — совпадают с метаданными.
pub fn assert_consistent_with_manifest(
    dto: &BuildMetadata,
    descriptor: &Value,
    manifest: &Value,
) -> Result<(), BuildMetadataTrustError> {
    // (поле, значение в метаданных, ожидаемое из описания)
    let mut pairs: Vec<(&str, &str, Option<Value>)> = [
        ("buildId", dto.build_id.as_str()),
        ("buildName", dto.build_name.as_str()),
        ("gitCommit", dto.git_commit.as_str()),
        ("architecture", dto.architecture.as_str()),
    ]
    .into_iter()
    .map(|(field, actual)| (field, actual, present(descriptor, field).cloned()))
    .collect();

    if let Some(built_at) = not_nullish(present(descriptor, "builtAt")) {
        match normalize_built_at(built_at, None) {
            Ok(normalized) => {
                pairs.push(("builtAt", dto.built_at.as_str(), Some(json!(normalized))));
            }
            Err(_) => {
                return Err(trust_error(
                    Reason::Invalid,
                    format!(
                        "descriptor.builtAt некорректна: {}",
                        jv::template(Some(built_at))
                    ),
                    None,
                ));
            }
        }
    }

    for (field, actual, expected) in &pairs {
        let Some(expected) = not_nullish(expected.as_ref()) else {
            continue;
        };
        if expected.as_str() != Some(actual) {
            return Err(trust_error(
                Reason::Mismatch,
                format!(
                    "Поле {field} в метаданных ({actual}) не совпало с подписанным манифестом ({})",
                    jv::template(Some(expected))
                ),
                Some(json!({"field": field, "actual": actual, "expected": expected})),
            ));
        }
    }

    if let Some(game_id) = not_nullish(present(manifest, "gameId"))
        && game_id.as_str() != Some(dto.game_id.as_str())
    {
        return Err(trust_error(
            Reason::Mismatch,
            format!(
                "gameId метаданных ({}) не совпал с манифестом ({})",
                dto.game_id,
                jv::template(Some(game_id))
            ),
            Some(json!({"field": "gameId", "actual": dto.game_id, "expected": game_id})),
        ));
    }

    if let Some(version) = not_nullish(present(manifest, "version"))
        && version.as_str() != Some(dto.version.as_str())
    {
        return Err(trust_error(
            Reason::Mismatch,
            format!(
                "version метаданных ({}) не совпала с манифестом ({})",
                dto.version,
                jv::template(Some(version))
            ),
            Some(json!({"field": "version", "actual": dto.version, "expected": version})),
        ));
    }

    Ok(())
}

/// `assertLauncherCompatible(dto, launcherVersion)`: лаунчер не старше `minimumLauncherVersion`.
/// Без версии лаунчера (пустая строка тоже) проверка пропускается.
pub fn assert_launcher_compatible(
    dto: &BuildMetadata,
    launcher_version: Option<&str>,
) -> Result<(), BuildMetadataTrustError> {
    let Some(launcher_version) = launcher_version.filter(|version| !version.is_empty()) else {
        return Ok(());
    };
    if semver::lt(launcher_version, &dto.minimum_launcher_version) {
        return Err(trust_error(
            Reason::RequiresNewerLauncher,
            format!(
                "Сборке нужен лаунчер не ниже {}, установлен {launcher_version}",
                dto.minimum_launcher_version
            ),
            Some(json!({"required": dto.minimum_launcher_version, "current": launcher_version})),
        ));
    }
    Ok(())
}

/// Параметры `verifyAgainstSnapshot` (`options`).
#[derive(Debug, Clone, Copy)]
pub struct VerifyAgainstSnapshot<'a> {
    /// `installRoot`.
    pub install_root: &'a Path,
    /// `snapshot`: запись `installed.json`; без `buildMetadata` сборка «прежняя» (`legacy`).
    pub snapshot: Option<&'a Value>,
    /// `launcherVersion`.
    pub launcher_version: Option<&'a str>,
    /// `expected`: по умолчанию — [`Profile::product`].
    pub expected: Option<&'a Profile>,
    /// `now`: «сейчас» в миллисекундах.
    pub now: Option<f64>,
}

/// `state` результата `verifyAgainstSnapshot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Trusted,
    Legacy,
    RepairRequired,
    LauncherUpdateRequired,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Trusted => "trusted",
            State::Legacy => "legacy",
            State::RepairRequired => "repair-required",
            State::LauncherUpdateRequired => "launcher-update-required",
        }
    }
}

/// Результат `verifyAgainstSnapshot`: `{ state, reason, repairable?, metadata, detail? }`.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotVerdict {
    pub state: State,
    pub reason: Option<Reason>,
    /// `repairable`: есть у `repair-required` (да) и `launcher-update-required` (нет).
    pub repairable: Option<bool>,
    pub metadata: Option<BuildMetadata>,
    pub detail: Option<Value>,
}

impl SnapshotVerdict {
    /// Объект с ключами в порядке 0.4.7: `state`, `reason`, `repairable`, `metadata`, `detail`
    /// (отсутствующие `repairable` и `detail` не пишутся).
    pub fn to_value(&self) -> Value {
        let mut out = Map::new();
        out.insert("state".to_owned(), json!(self.state.as_str()));
        out.insert(
            "reason".to_owned(),
            self.reason
                .map_or(Value::Null, |reason| json!(reason.as_str())),
        );
        if let Some(repairable) = self.repairable {
            out.insert("repairable".to_owned(), json!(repairable));
        }
        out.insert(
            "metadata".to_owned(),
            self.metadata
                .as_ref()
                .map_or(Value::Null, BuildMetadata::to_value),
        );
        if let Some(detail) = &self.detail {
            out.insert("detail".to_owned(), detail.clone());
        }
        Value::Object(out)
    }
}

fn repair(reason: Reason, detail: Value) -> SnapshotVerdict {
    SnapshotVerdict {
        state: State::RepairRequired,
        reason: Some(reason),
        repairable: Some(true),
        metadata: None,
        detail: Some(detail),
    }
}

/// `a === b` для значения метаданных (строка или число) и значения снимка.
fn strictly_equal(actual: &Value, expected: Option<&Value>) -> bool {
    match (actual, expected) {
        (Value::String(a), Some(Value::String(b))) => a == b,
        (Value::Number(_), Some(b @ Value::Number(_))) => jv::number(actual) == jv::number(b),
        _ => false,
    }
}

/// `verifyAgainstSnapshot(options)`.
pub fn verify_against_snapshot(options: &VerifyAgainstSnapshot) -> SnapshotVerdict {
    let expected = options
        .snapshot
        .and_then(|snapshot| present(snapshot, "buildMetadata"))
        .filter(|metadata| js::truthy(metadata));
    let Some(expected) = expected else {
        return SnapshotVerdict {
            state: State::Legacy,
            reason: None,
            repairable: None,
            metadata: None,
            detail: None,
        };
    };

    let found = match reader::read(
        options.install_root,
        &ReadOptions {
            expected: options.expected,
            now: options.now,
        },
    ) {
        Ok(Reading::Present(found)) => found,
        Ok(Reading::Absent) => {
            return repair(Reason::Missing, json!({"relativePath": RELATIVE_PATH}));
        }
        Err(error) => {
            let mapped = map_reader_error(&error, json!({"phase": "runtime-read"}));
            return repair(mapped.code, mapped.detail.unwrap_or(Value::Null));
        }
    };

    let dto = &found.dto;
    let actual = dto.to_value();

    for field in REQUIRED_FIELDS {
        let expected_field = present(expected, field);
        if !strictly_equal(&actual[field], expected_field) {
            let mut detail = Map::new();
            detail.insert("field".to_owned(), json!(field));
            detail.insert("actual".to_owned(), actual[field].clone());
            if let Some(value) = expected_field {
                detail.insert("expected".to_owned(), value.clone());
            }
            return repair(Reason::Mismatch, Value::Object(detail));
        }
    }

    if let Some(launcher) = options
        .launcher_version
        .filter(|version| !version.is_empty())
        && semver::lt(launcher, &dto.minimum_launcher_version)
    {
        return SnapshotVerdict {
            state: State::LauncherUpdateRequired,
            reason: Some(Reason::RequiresNewerLauncher),
            repairable: Some(false),
            metadata: Some(dto.clone()),
            detail: Some(json!({"required": dto.minimum_launcher_version, "current": launcher})),
        };
    }

    SnapshotVerdict {
        state: State::Trusted,
        reason: None,
        repairable: None,
        metadata: Some(dto.clone()),
        detail: None,
    }
}

/// Результат `snapshotFromVerified`: то, что `installed.json` хранит о метаданных сборки.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildSnapshot {
    pub build_metadata: BuildMetadata,
    pub verified_at: String,
    pub sha256: String,
    /// `publishedAt` или `null`.
    pub published_at: Value,
}

impl BuildSnapshot {
    /// `{ buildMetadata, buildMetadataVerifiedAt, buildMetadataSha256, publishedAt }`.
    pub fn to_value(&self) -> Value {
        json!({
            "buildMetadata": self.build_metadata.to_value(),
            "buildMetadataVerifiedAt": self.verified_at,
            "buildMetadataSha256": self.sha256,
            "publishedAt": self.published_at,
        })
    }
}

/// `snapshotFromVerified(verified)`.
pub fn snapshot_from_verified(verified: &Verified) -> BuildSnapshot {
    BuildSnapshot {
        build_metadata: verified.metadata.clone(),
        verified_at: verified.verified_at.clone(),
        sha256: verified.file_sha256.clone(),
        published_at: if js::truthy(&verified.published_at) {
            verified.published_at.clone()
        } else {
            Value::Null
        },
    }
}
