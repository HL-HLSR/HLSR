//! Чтение метаданных сборки с диска (`build-metadata-reader.js`): `ghostingmod/hlsr-build.json`
//! внутри установки. Файл не должен быть ссылкой (symlink, junction), не больше 64 КиБ, и путь к
//! нему после раскрытия ссылок не должен выходить за установку (регистр букв не учитывается на
//! обеих ОС, как в 0.4.7). Любая ошибка `lstat` — «файла нет».

use crate::build_metadata::{
    self, BuildMetadata, BuildMetadataError, MAX_BYTES, MetadataCode, Profile, RELATIVE_PATH,
    ValidateOptions,
};
use crate::errors::{DeliveryError, RawError};
use crate::file_ops::hex;
use crate::paths::assert_inside_with;
use hlsr_core::js::fs::read_file;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::utf8_string;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::{MAIN_SEPARATOR_STR, Path, PathBuf};

/// Параметры `read` (`options`).
#[derive(Debug, Clone, Copy, Default)]
pub struct ReadOptions<'a> {
    /// `expected`: по умолчанию — [`Profile::product`].
    pub expected: Option<&'a Profile>,
    /// `now`: «сейчас» в миллисекундах.
    pub now: Option<f64>,
}

/// Прочитанный и проверенный файл.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub dto: BuildMetadata,
    /// Текст файла (`toString("utf8")`).
    pub raw: String,
    /// `stat.size`.
    pub size_bytes: u64,
    /// sha256 байтов файла, строчными шестнадцатеричными цифрами.
    pub sha256: String,
}

/// Результат `read`: `{ present: false }` или `{ present: true, dto, raw, sizeBytes, sha256 }`.
#[derive(Debug, Clone, PartialEq)]
pub enum Reading {
    Absent,
    Present(Box<Found>),
}

/// Исключение `read`: своя ошибка метаданных, `PATH_INVALID` от `assertInside` или сбой чтения.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ReadError {
    #[error(transparent)]
    Metadata(#[from] BuildMetadataError),
    #[error(transparent)]
    Path(#[from] DeliveryError),
    #[error(transparent)]
    Io(#[from] RawError),
}

impl ReadError {
    /// `error.code` в смысле `mapReaderError`: `BUILD_METADATA_UNSUPPORTED` отличает версию схемы.
    pub fn is_unsupported(&self) -> bool {
        matches!(self, ReadError::Metadata(error) if error.code == MetadataCode::Unsupported)
    }

    /// `error.message`.
    pub fn message(&self) -> &str {
        match self {
            ReadError::Metadata(error) => &error.message,
            ReadError::Path(error) => &error.message,
            ReadError::Io(error) => &error.message,
        }
    }
}

fn invalid(message: impl Into<String>, detail: serde_json::Value) -> ReadError {
    ReadError::Metadata(BuildMetadataError::new(
        MetadataCode::Invalid,
        message,
        Some(detail),
    ))
}

/// Путь так, как его печатает `fs.realpathSync`: без префикса `\\?\` у `canonicalize` Windows.
fn real_text(path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{rest}");
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return rest.to_owned();
        }
    }
    text
}

/// `assertRealpathContained`: настоящий путь файла лежит внутри настоящего пути установки.
/// Если какой-то из них не раскрывается, проверка молча пропускается.
fn assert_realpath_contained(install_root: &Path, target: &Path) -> Result<(), ReadError> {
    let (Ok(real_root), Ok(real_target)) = (
        std::fs::canonicalize(install_root),
        std::fs::canonicalize(target),
    ) else {
        return Ok(());
    };
    let (real_root, real_target) = (real_text(&real_root), real_text(&real_target));
    let prefix = if real_root.ends_with(MAIN_SEPARATOR_STR) {
        real_root.clone()
    } else {
        format!("{real_root}{MAIN_SEPARATOR_STR}")
    };
    let (lower_root, lower_target) = (real_root.to_lowercase(), real_target.to_lowercase());
    if lower_target != lower_root && !lower_target.starts_with(&prefix.to_lowercase()) {
        return Err(invalid(
            "Файл метаданных ведёт за пределы установки (symlink/junction)",
            json!({"target": target.to_string_lossy(), "realTarget": real_target}),
        ));
    }
    Ok(())
}

/// `read(installRoot, options)`.
pub fn read(install_root: &Path, options: &ReadOptions) -> Result<Reading, ReadError> {
    let path = NodePath::native();
    let root_text = install_root.to_string_lossy();
    let joined = path.join(&[&root_text, &RELATIVE_PATH.replace('/', path.sep())]);
    let target = PathBuf::from(assert_inside_with(
        &path,
        &root_text,
        &joined,
        Some("Файл метаданных сборки"),
    )?);
    let target_text = target.to_string_lossy().into_owned();

    let Ok(stat) = std::fs::symlink_metadata(&target) else {
        return Ok(Reading::Absent);
    };

    if stat.file_type().is_symlink() {
        return Err(invalid(
            "Файл метаданных сборки является symlink",
            json!({"target": target_text}),
        ));
    }

    if !stat.file_type().is_file() {
        return Err(invalid(
            "По пути метаданных сборки лежит не файл",
            json!({"target": target_text}),
        ));
    }

    if stat.len() > MAX_BYTES {
        return Err(invalid(
            format!(
                "Файл метаданных слишком большой: {} байт при пределе {MAX_BYTES}",
                stat.len()
            ),
            json!({"target": target_text, "size": stat.len(), "max": MAX_BYTES}),
        ));
    }

    assert_realpath_contained(install_root, &target)?;

    let bytes = read_file(&target).map_err(|error| RawError::fs(&error, "open", &target))?;
    let sha256 = hex(&Sha256::digest(&bytes));
    let raw = utf8_string(&bytes);

    let dto = build_metadata::parse(
        &raw,
        &ValidateOptions {
            expected: options.expected,
            now: options.now,
            source: Some(RELATIVE_PATH),
        },
    )?;

    Ok(Reading::Present(Box::new(Found {
        dto,
        raw,
        size_bytes: stat.len(),
        sha256,
    })))
}
