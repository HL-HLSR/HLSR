//! Исправленные под Wine компоненты LiveSplit (`linux/LiveSplitFixes.js`).
//!
//! Файл заменяется, только если на диске ровно та сборка, из которой сделано исправление (по
//! sha256): компонент, который пользователь обновил сам, лаунчер не трогает. Исправления лежат в
//! `resources/livesplit-fixes`.

use crate::errors::RawError;
use crate::file_ops::hex;
use crate::node_fs;
use hlsr_core::js::path::NodePath;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// `FIXES_DIRECTORY`: папка исправлений в каталоге ресурсов.
pub const FIXES_DIRECTORY: &str = "livesplit-fixes";

/// Запись таблицы `LIVESPLIT_FIXES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fix<'a> {
    /// Путь внутри папки LiveSplit (через `/`).
    pub target: &'a str,
    /// Имя исправленного файла в `FIXES_DIRECTORY`.
    pub source: &'a str,
    /// sha256 исходной сборки.
    pub original: &'a str,
    /// sha256 исправленной сборки.
    pub fixed: &'a str,
}

/// `LIVESPLIT_FIXES`. SourceSplit 3.3.4.6: если LiveSplit запущен раньше игры, wine-mono
/// бросает `ArgumentOutOfRangeException` из `Process.MainModule`, пока у только что созданного
/// процесса пуст список модулей, и SourceSplit показывает окно ошибки (в Windows там
/// `Win32Exception`, её он пропускает). В исправлении этот вызов в `TryGetGameProcess` возвращает
/// `null`, и SourceSplit повторяет попытку. Патч: `scripts/sourcesplit-patch/patch.cs`.
pub const LIVESPLIT_FIXES: [Fix<'static>; 1] = [Fix {
    target: "Components/LiveSplit.SourceSplit.dll",
    source: "LiveSplit.SourceSplit.dll",
    original: "f8461b811571dbede634c9bd048d6fc05a64d5e949a52e2ec339bc27149871e8",
    fixed: "a02c1a2b545b0b17b889a795f7fd5ddc44807398d9ad4c683292aa2a558a20f1",
}];

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn sha256_of(file: &Path) -> Result<String, RawError> {
    Ok(hex(&Sha256::digest(node_fs::read_bytes(file)?)))
}

/// `applyLiveSplitFixes(root, {resourcesPath, fixes})`: пути заменённых файлов относительно
/// папки LiveSplit. Файла нет или он другой сборки — запись пропускается; исправление в
/// ресурсах не читается — ошибка чтения; не сходится с таблицей — «повреждено». Копия идёт
/// через временный файл `<цель>.<pid>.tmp`, при сбое переименования он остаётся (как в 0.4.7).
pub fn apply_live_split_fixes(
    root: &Path,
    resources_path: &Path,
    fixes: &[Fix<'_>],
) -> Result<Vec<String>, RawError> {
    let native = NodePath::native();
    let mut applied = Vec::new();
    for fix in fixes {
        let target = PathBuf::from(native.join(&[&text(root), fix.target]));
        let Ok(current) = sha256_of(&target) else {
            continue;
        };
        if current != fix.original {
            continue;
        }
        let source =
            PathBuf::from(native.join(&[&text(resources_path), FIXES_DIRECTORY, fix.source]));
        if sha256_of(&source)? != fix.fixed {
            return Err(RawError::new(format!(
                "Исправление LiveSplit повреждено: {}",
                fix.source
            )));
        }
        let temporary = PathBuf::from(format!("{}.{}.tmp", text(&target), std::process::id()));
        node_fs::copy_file(&source, &temporary)?;
        std::fs::rename(&temporary, &target)
            .map_err(|error| RawError::fs_with_dest(&error, "rename", &temporary, Some(&target)))?;
        applied.push(fix.target.to_owned());
    }
    Ok(applied)
}
