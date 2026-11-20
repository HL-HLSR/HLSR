//! Версия выпуска (решение Р9): только `X.Y.Z`, беты тоже. Linux-канал, `install-files.json`,
//! `DisplayVersion` и указатель `current` пре-релиз не принимают, а лаунчер с `0.5.0-dev` в
//! `CARGO_PKG_VERSION` счёл бы выпуск `0.5.0` новее себя и предлагал бы обновиться на самого себя.

use crate::error::{Result, fail};
use hlsr_update::windows::layout::is_version;

/// `^\d+\.\d+\.\d+$` или отказ.
pub fn require_release(version: &str) -> Result<()> {
    if is_version(version) {
        return Ok(());
    }
    fail(format!(
        "Версия выпуска — только X.Y.Z (получено {version:?}): пре-релиз и «-dev» не публикуются"
    ))
}

/// `version = "…"` раздела `[workspace.package]` манифеста рабочего дерева.
pub fn workspace_version(manifest: &str) -> Option<String> {
    let mut in_section = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[workspace.package]";
            continue;
        }
        if !in_section {
            continue;
        }
        let Some(rest) = line.strip_prefix("version") else {
            continue;
        };
        let rest = rest.trim_start().strip_prefix('=')?.trim();
        let quoted = rest.strip_prefix('"')?;
        return quoted.split('"').next().map(str::to_owned);
    }
    None
}
