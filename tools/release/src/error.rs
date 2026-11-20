//! Ошибки упаковщика: текст для человека, который запускает сборку выпуска.

use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    #[error("{0}")]
    Message(String),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub type Result<T> = std::result::Result<T, ReleaseError>;

/// Отказ с текстом.
pub fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(ReleaseError::Message(message.into()))
}

/// Ошибка ввода-вывода с путём, на котором она случилась.
pub trait At<T> {
    fn at(self, path: &Path) -> Result<T>;
}

impl<T> At<T> for io::Result<T> {
    fn at(self, path: &Path) -> Result<T> {
        self.map_err(|source| ReleaseError::Io {
            path: path.to_owned(),
            source,
        })
    }
}
