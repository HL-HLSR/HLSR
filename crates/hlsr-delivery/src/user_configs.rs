//! Конфиги игрока (`user-configs.js`): cfg, который игрок поменял сам или в
//! редакторе конфигов лаунчера, обновление, переустановка и проверка файлов не
//! возвращают к версии сборки. Изменённым считается cfg, чей SHA-256 не совпадает
//! с прежней версией сборки, или cfg, которого в ней не было. Недостающий cfg
//! по-прежнему восстанавливается: его игрок не менял.

use crate::file_ops::hex;
use hlsr_core::js::errno;
use hlsr_core::js::path::NodePath;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

/// `isConfigPath(relative)`: `/\.cfg$/i`.
pub fn is_config_path(relative: &str) -> bool {
    let bytes = relative.as_bytes();
    bytes.len() >= 4 && bytes[bytes.len() - 4..].eq_ignore_ascii_case(b".cfg")
}

/// `isUserEdited(installPath, relative, previousFiles, { newSha256 })`.
///
/// `previous_files` — `files.json` прежней версии (`None` — неизвестна: тогда
/// изменённым не считается ничего). `new_sha256` — хеш файла в новой версии:
/// совпавший с ним файл менять и так незачем.
pub fn is_user_edited(
    install_path: &Path,
    relative: &str,
    previous_files: Option<&Value>,
    new_sha256: Option<&str>,
) -> bool {
    let Some(files) = previous_files
        .and_then(|p| p.get("files"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    if !is_config_path(relative) {
        return false;
    }
    let path = NodePath::native();
    let full = path.join(&[
        &install_path.to_string_lossy(),
        &relative.split('/').collect::<Vec<_>>().join(path.sep()),
    ]);
    // Нечитаемый cfg считаем изменённым: его перенесут (или сбой переноса остановит обновление),
    // а не потеряют молча. Нет файла — не изменён.
    let unreadable = |error: std::io::Error| {
        !matches!(
            errno::code(&error).as_deref(),
            Some("ENOENT") | Some("ENOTDIR")
        )
    };
    let digest = match std::fs::metadata(&full) {
        Ok(meta) if meta.is_file() => match std::fs::read(&full) {
            Ok(bytes) => hex(&Sha256::digest(&bytes)),
            Err(error) => return unreadable(error),
        },
        Ok(_) => return false,
        Err(error) => return unreadable(error),
    };
    if new_sha256.is_some_and(|sha| !sha.is_empty() && sha == digest) {
        return false;
    }
    let previous = files
        .iter()
        .find(|file| file.get("path").and_then(Value::as_str) == Some(relative));
    previous.is_none_or(|file| file.get("sha256").and_then(Value::as_str) != Some(digest.as_str()))
}

/// `keptByPatch(installPath, patchManifest, previousFiles)`: пути патча
/// (заменяемые и удаляемые), которые остаются как у игрока, в порядке патча.
pub fn kept_by_patch(
    install_path: &Path,
    patch_manifest: &Value,
    previous_files: Option<&Value>,
) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    let mut keep = |relative: &str| {
        if !kept.iter().any(|k| k == relative) {
            kept.push(relative.to_owned());
        }
    };
    let list = |key: &str| {
        patch_manifest
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    for file in list("files") {
        let Some(relative) = file.get("path").and_then(Value::as_str) else {
            continue;
        };
        let new_sha256 = file.get("sha256").and_then(Value::as_str);
        if is_user_edited(install_path, relative, previous_files, new_sha256) {
            keep(relative);
        }
    }
    for relative in list("deletedFiles") {
        let Some(relative) = relative.as_str() else {
            continue;
        };
        if is_user_edited(install_path, relative, previous_files, None) {
            keep(relative);
        }
    }
    kept
}
