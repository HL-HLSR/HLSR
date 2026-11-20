//! Старая раскладка библиотеки (`library-migration.js`): игры лежали в `<библиотека>/<id>`, теперь —
//! в `<библиотека>/Games/<id>`. Поиск таких папок и перенос одной игры с правкой `installPath` в
//! её `installed.json`; не вышло записать `installed.json` — папка возвращается на место.

use crate::atomic::write_json_atomic;
use crate::errors::{Code, DeliveryError, Failure, RawError};
use crate::js_value as jv;
use crate::paths::is_game_id;
use crate::paths::{assert_inside_with, resolve_library};
use hlsr_core::js::path::NodePath;
use hlsr_core::json_store::read_json_safe;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub use crate::paths::is_inside;

/// `INSTALLED_FILE`.
pub const INSTALLED_FILE: &str = "installed.json";

/// Игра в старой раскладке (`detectLegacyLayout`): откуда и куда переносить.
#[derive(Debug, Clone, PartialEq)]
pub struct LegacyGame {
    pub game_id: String,
    pub from: PathBuf,
    pub to: PathBuf,
    /// `installed.version || null`.
    pub version: Value,
}

impl LegacyGame {
    /// `{gameId, from, to, version}`.
    pub fn to_json(&self) -> Value {
        json!({
            "gameId": self.game_id,
            "from": self.from.to_string_lossy(),
            "to": self.to.to_string_lossy(),
            "version": self.version,
        })
    }
}

/// Итог `migrateLegacyGame`: `{ok: true, gameId, from, to}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Migrated {
    pub game_id: String,
    pub from: PathBuf,
    pub to: PathBuf,
}

impl Migrated {
    pub fn to_json(&self) -> Value {
        json!({
            "ok": true,
            "gameId": self.game_id,
            "from": self.from.to_string_lossy(),
            "to": self.to.to_string_lossy(),
        })
    }
}

/// `installed` годится для переноса: объект с `gameId`, равным `game_id`.
fn belongs_to(installed: &Value, game_id: &str) -> bool {
    jv::truthy(Some(installed)) && jv::is_str(jv::prop(installed, "gameId"), game_id)
}

/// `detectLegacyLayout(libraryRoot, gameIds)`: игры с `installed.json` в `<библиотека>/<id>`,
/// у которых ещё нет `Games/<id>`.
pub fn detect_legacy_layout(
    library_root: &str,
    game_ids: &[&str],
) -> Result<Vec<LegacyGame>, DeliveryError> {
    let paths = resolve_library(library_root)?;
    let path = NodePath::native();
    let (root, games) = (paths.root.to_string_lossy(), paths.games.to_string_lossy());
    let mut found = Vec::new();
    for game_id in game_ids {
        if !is_game_id(game_id) {
            continue;
        }
        let legacy_dir = path.join(&[&root, game_id]);
        let target_dir = path.join(&[&games, game_id]);
        let installed = read_json_safe(&Path::new(&legacy_dir).join(INSTALLED_FILE));
        if !belongs_to(&installed, game_id) || Path::new(&target_dir).exists() {
            continue;
        }
        let version = jv::prop(&installed, "version")
            .filter(|v| jv::truthy(Some(v)))
            .cloned()
            .unwrap_or(Value::Null);
        found.push(LegacyGame {
            game_id: (*game_id).to_owned(),
            from: PathBuf::from(legacy_dir),
            to: PathBuf::from(target_dir),
            version,
        });
    }
    Ok(found)
}

fn path_invalid(message: String) -> Failure {
    DeliveryError::new(Code::PathInvalid, message).into()
}

fn rename(from: &Path, to: &Path) -> Result<(), RawError> {
    std::fs::rename(from, to).map_err(|e| RawError::fs_with_dest(&e, "rename", from, Some(to)))
}

/// `migrateLegacyGame(libraryRoot, gameId)`: перенести `<библиотека>/<id>` в `Games/<id>` и
/// записать в `installed.json` новый `installPath`.
pub fn migrate_legacy_game(library_root: &str, game_id: &str) -> Result<Migrated, Failure> {
    let paths = resolve_library(library_root)?;
    let path = NodePath::native();
    let text = |p: &Path| p.to_string_lossy().into_owned();
    let (root, games) = (text(&paths.root), text(&paths.games));
    let from = assert_inside_with(
        &path,
        &root,
        &path.join(&[&root, game_id]),
        Some("Старая папка игры"),
    )?;
    let to = assert_inside_with(
        &path,
        &games,
        &path.join(&[&games, game_id]),
        Some("Новая папка игры"),
    )?;
    for reserved in [
        &paths.games,
        &paths.downloads,
        &paths.backups,
        &paths.internal,
    ] {
        if from.to_lowercase() == text(reserved).to_lowercase() {
            return Err(path_invalid(format!(
                "Отказ мигрировать служебный каталог {from}"
            )));
        }
    }
    let (from, to) = (PathBuf::from(from), PathBuf::from(to));
    let installed = read_json_safe(&from.join(INSTALLED_FILE));
    if !belongs_to(&installed, game_id) {
        return Err(path_invalid(format!(
            "В {} нет installed.json игры {game_id} — мигрировать нечего",
            from.display()
        )));
    }
    if to.exists() {
        return Err(path_invalid(format!(
            "Целевой каталог уже существует: {} — миграция вручную",
            to.display()
        )));
    }
    crate::node_fs::mkdir_recursive(&paths.games)?;
    rename(&from, &to)?;

    let mut updated = installed;
    updated["installPath"] = json!(text(&to));
    if let Err(error) = write_json_atomic(&to.join(INSTALLED_FILE), &updated) {
        if let Err(back) = rename(&to, &from) {
            return Err(DeliveryError::new(
                Code::InstallFailed,
                format!(
                    "Перенос выполнен, но installed.json не обновлён и откат не удался: {}",
                    back.message
                ),
            )
            .into());
        }
        return Err(DeliveryError::new(
            Code::InstallFailed,
            format!(
                "Не удалось обновить installed.json: {} — перенос отменён",
                error.message
            ),
        )
        .into());
    }
    Ok(Migrated {
        game_id: game_id.to_owned(),
        from,
        to,
    })
}
