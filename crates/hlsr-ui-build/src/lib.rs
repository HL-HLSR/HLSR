//! Логика `build.rs` крейта `hlsr-ui`: строки → таблица Rust; разбор `themes.json` (тесты и
//! вид).

pub mod locales;
pub mod themes;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// ОС сборки: от неё зависит вариант строки (`{"windows": …, "linux": …}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOs {
    Windows,
    Linux,
}

impl TargetOs {
    /// Значение `CARGO_CFG_TARGET_OS`.
    pub fn from_cargo(value: &str) -> Option<TargetOs> {
        match value {
            "windows" => Some(TargetOs::Windows),
            "linux" => Some(TargetOs::Linux),
            _ => None,
        }
    }

    /// Ключ варианта в файлах строк.
    pub fn variant_key(self) -> &'static str {
        match self {
            TargetOs::Windows => "windows",
            TargetOs::Linux => "linux",
        }
    }
}

/// Ошибка сборки ресурсов интерфейса.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{}: не JSON: {source}", path.display())]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("{}: {message}", path.display())]
    Invalid { path: PathBuf, message: String },
}

/// Записать, только если содержимое изменилось: иначе cargo перезапускал бы
/// build.rs из-за новой даты файла.
pub fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, bytes)
}

pub(crate) fn invalid(path: &Path, message: impl Into<String>) -> BuildError {
    BuildError::Invalid {
        path: path.to_path_buf(),
        message: message.into(),
    }
}
