//! Удаление игры из библиотеки (`game-removal.js`): папка игры, её загрузки, кеш манифестов и
//! служебные записи в `.hlsr` (журналы, staging, откаты). Симлинки не разыменовываются
//! (`removeTreeNoFollow` из `uninstall/library-cleanup.js`).

use crate::errors::{Code, DeliveryError};
/// `GAME_ID_RE`: `/^[a-z0-9]+(-[a-z0-9]+)*$/`.
pub use crate::paths::is_game_id;
use crate::paths::{LibraryPaths, assert_inside, game_install_path, is_inside};
use hlsr_core::js;
use hlsr_core::js::path::NodePath;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub const INSTALLED_FILE: &str = "installed.json";

fn native() -> NodePath {
    NodePath::native()
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn join(parts: &[&str]) -> PathBuf {
    PathBuf::from(native().join(parts))
}

/// `treeSize(target)`: размер дерева без перехода по симлинкам; ошибки — 0. Не общая с
/// `hlsr_update::linux::launcher_uninstall::tree_size` (`launcher-uninstall.js`): путь потомка
/// здесь — `path.join` Node (`NodePath`: разбор и нормализация текста пути, как `game-removal.js`),
/// там — `Path::join`; на путях с `..`, `.` и смешанными разделителями они расходятся.
pub fn tree_size(target: &Path) -> u64 {
    let Ok(stat) = std::fs::symlink_metadata(target) else {
        return 0;
    };
    if stat.file_type().is_symlink() {
        return 0;
    }
    if !stat.is_dir() {
        return stat.len();
    }
    let Ok(entries) = js::fs::read_dir_names(target) else {
        return 0;
    };
    entries
        .iter()
        .map(|entry| tree_size(&join(&[&text(target), entry])))
        .sum()
}

fn exists(target: &Path) -> bool {
    std::fs::symlink_metadata(target).is_ok()
}

/// `entryMatchers(gameId)`: `<id>.<что-то>.json`, `<id>-<время36>-<8 hex>`,
/// `repair-mirror-<id>-<время36>`.
fn is_own_entry(name: &str, game_id: &str) -> bool {
    let lower_alnum = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
    };
    if let Some(rest) = name.strip_prefix(game_id).and_then(|r| r.strip_prefix('.'))
        && let Some(middle) = rest.strip_suffix(".json")
        && !middle.is_empty()
        && !middle.contains('.')
    {
        return true;
    }
    if let Some(rest) = name.strip_prefix(game_id).and_then(|r| r.strip_prefix('-'))
        && let Some((stamp, hex)) = rest.split_once('-')
        && lower_alnum(stamp)
        && hex.len() == 8
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return true;
    }
    if let Some(rest) = name
        .strip_prefix("repair-mirror-")
        .and_then(|r| r.strip_prefix(game_id))
        .and_then(|r| r.strip_prefix('-'))
        && lower_alnum(rest)
    {
        return true;
    }
    false
}

/// `ownEntries(zone, gameId)`: записи игры в служебной папке.
pub fn own_entries(zone: &Path, game_id: &str) -> Vec<PathBuf> {
    let Ok(names) = js::fs::read_dir_names(zone) else {
        return Vec::new();
    };
    names
        .iter()
        .filter(|name| is_own_entry(name, game_id))
        .map(|name| join(&[&text(zone), name]))
        .collect()
}

/// Что удалить: вид (`game`, `downloads`, `manifests`, `state`, `staging`, `rollback`), зона и
/// путь.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub kind: &'static str,
    pub zone: PathBuf,
    pub path: PathBuf,
    pub bytes: u64,
}

/// `targetsFor(paths, gameId)`.
pub fn targets_for(paths: &LibraryPaths, game_id: &str) -> Vec<Target> {
    let manifests = join(&[&text(&paths.internal), "manifests"]);
    let mut targets = vec![
        Target {
            kind: "game",
            zone: paths.games.clone(),
            path: join(&[&text(&paths.games), game_id]),
            bytes: 0,
        },
        Target {
            kind: "downloads",
            zone: paths.downloads.clone(),
            path: join(&[&text(&paths.downloads), game_id]),
            bytes: 0,
        },
        Target {
            kind: "manifests",
            zone: manifests.clone(),
            path: join(&[&text(&manifests), game_id]),
            bytes: 0,
        },
        // Эпизоды, отложенные на время подмены сборки (`InstallService::park_episodes`).
        Target {
            kind: "state",
            zone: join(&[&text(&paths.state), "episodes-park"]),
            path: join(&[&text(&paths.state), "episodes-park", game_id]),
            bytes: 0,
        },
    ];
    for (kind, zone) in [
        ("state", &paths.state),
        ("staging", &paths.staging),
        ("rollback", &paths.rollback),
    ] {
        for entry in own_entries(zone, game_id) {
            targets.push(Target {
                kind,
                zone: zone.clone(),
                path: entry,
                bytes: 0,
            });
        }
    }
    targets
}

/// `pathChain(target)`: путь и все его родители от корня.
fn path_chain(target: &Path) -> Vec<String> {
    let path = native();
    let mut chain = Vec::new();
    let mut current = path.resolve(&[&text(target)]);
    loop {
        chain.insert(0, current.clone());
        let parent = path.dirname(&current);
        if parent == current {
            break;
        }
        current = parent;
    }
    chain
}

/// `findReparsePoint(target)`: первый симлинк (или junction) на пути; нечитаемая часть пути —
/// тоже она.
pub fn find_reparse_point(target: &Path) -> Option<String> {
    for part in path_chain(target) {
        match std::fs::symlink_metadata(&part) {
            Err(error) if js::errno::code(&error).as_deref() == Some("ENOENT") => continue,
            Err(_) => return Some(part),
            Ok(stat) if stat.file_type().is_symlink() => return Some(part),
            Ok(_) => {}
        }
    }
    None
}

/// `assertRemovable(paths, gameId)`: id допустим, игра установлена, на пути нет симлинков.
pub fn assert_removable(
    paths: &LibraryPaths,
    raw_game_id: &str,
) -> Result<(String, PathBuf), DeliveryError> {
    let game_id = raw_game_id.to_owned();
    if !is_game_id(&game_id) {
        return Err(DeliveryError::new(
            Code::PathInvalid,
            format!("Недопустимый gameId: {raw_game_id}"),
        ));
    }
    let install_path = game_install_path(paths, &game_id)?;
    if std::fs::metadata(join(&[&text(&install_path), INSTALLED_FILE])).is_err() {
        return Err(
            DeliveryError::new(Code::NotInstalled, "Игра не установлена")
                .with_details(json!({"gameId": game_id})),
        );
    }
    if let Some(reparse) = find_reparse_point(&install_path) {
        return Err(DeliveryError::new(
            Code::PathInvalid,
            format!("В пути установки есть символьная ссылка или junction: {reparse}"),
        )
        .with_details(json!({"gameId": game_id, "path": reparse})));
    }
    Ok((game_id, install_path))
}

/// Итог `planRemoval`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovalPlan {
    pub game_id: String,
    pub install_path: PathBuf,
    pub targets: Vec<Target>,
    pub total_bytes: u64,
}

/// `planRemoval(paths, gameId)`.
pub fn plan_removal(paths: &LibraryPaths, raw_game_id: &str) -> Result<RemovalPlan, DeliveryError> {
    let (game_id, install_path) = assert_removable(paths, raw_game_id)?;
    let targets: Vec<Target> = targets_for(paths, &game_id)
        .into_iter()
        .filter(|target| exists(&target.path))
        .map(|target| Target {
            bytes: tree_size(&target.path),
            ..target
        })
        .collect();
    let total_bytes = targets.iter().map(|target| target.bytes).sum();
    Ok(RemovalPlan {
        game_id,
        install_path,
        targets,
        total_bytes,
    })
}

/// Путь, который удалить не вышло, и код ошибки (`error.code || error.message`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    pub path: String,
    pub error: String,
}

impl Failed {
    pub fn to_json(&self) -> Value {
        json!({"path": self.path, "error": self.error})
    }
}

/// Счёт `removeTreeNoFollow`: удалено элементов и что не удалилось.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Removal {
    pub removed: u64,
    pub failed: Vec<Failed>,
}

fn error_code(error: &std::io::Error, syscall: &str, path: &Path) -> String {
    js::errno::code(error)
        .map(|code| code.into_owned())
        .unwrap_or_else(|| crate::errors::RawError::fs(error, syscall, path).message)
}

/// `removeTreeNoFollow(target, result)`: дерево удаляется без перехода по симлинкам (сами
/// ссылки удаляются).
pub fn remove_tree_no_follow(target: &Path, out: &mut Removal) {
    let fail = |out: &mut Removal, error: &std::io::Error, syscall: &str| {
        out.failed.push(Failed {
            path: text(target),
            error: error_code(error, syscall, target),
        });
    };
    let stat = match std::fs::symlink_metadata(target) {
        Ok(stat) => stat,
        Err(error) if js::errno::code(&error).as_deref() == Some("ENOENT") => return,
        Err(error) => return fail(out, &error, "lstat"),
    };

    if stat.file_type().is_symlink() {
        match std::fs::remove_file(target) {
            Ok(()) => out.removed += 1,
            Err(_) => match std::fs::remove_dir(target) {
                Ok(()) => out.removed += 1,
                Err(inner) => fail(out, &inner, "rmdir"),
            },
        }
        return;
    }

    if stat.is_dir() {
        let entries = match js::fs::read_dir_names(target) {
            Ok(entries) => entries,
            Err(error) => return fail(out, &error, "scandir"),
        };
        for entry in entries {
            remove_tree_no_follow(&join(&[&text(target), &entry]), out);
        }
        match std::fs::remove_dir(target) {
            Ok(()) => out.removed += 1,
            Err(error) => fail(out, &error, "rmdir"),
        }
        return;
    }

    match std::fs::remove_file(target) {
        Ok(()) => out.removed += 1,
        Err(error) => fail(out, &error, "unlink"),
    }
}

/// Итог `removeGame`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    pub removed: u64,
    pub failed: Vec<Failed>,
    pub partial: bool,
    pub freed_bytes: u64,
}

/// `removeGame(paths, gameId)`: план и удаление по нему; `installed.json` — первым.
pub fn remove_game(paths: &LibraryPaths, raw_game_id: &str) -> Result<Removed, DeliveryError> {
    let plan = plan_removal(paths, raw_game_id)?;
    remove_planned(&plan)
}

/// Удаление по готовому плану (`removeGame` без повторного `planRemoval`).
pub fn remove_planned(plan: &RemovalPlan) -> Result<Removed, DeliveryError> {
    let mut result = Removal::default();
    let installed_file = join(&[&text(&plan.install_path), INSTALLED_FILE]);
    match std::fs::remove_file(&installed_file) {
        Ok(()) => result.removed += 1,
        Err(error) if js::errno::code(&error).as_deref() == Some("ENOENT") => {}
        Err(error) => result.failed.push(Failed {
            path: text(&installed_file),
            error: error_code(&error, "unlink", &installed_file),
        }),
    }

    let path = native();
    for target in &plan.targets {
        let zone = PathBuf::from(path.resolve(&[&text(&target.zone)]));
        let victim = PathBuf::from(path.resolve(&[&text(&target.path)]));
        if victim == zone || !is_inside(&zone, &victim) {
            result.failed.push(Failed {
                path: text(&victim),
                error: "OUT_OF_ZONE".to_owned(),
            });
            continue;
        }
        assert_inside(
            &zone,
            &victim,
            Some(&format!("Удаляемый путь ({})", target.kind)),
        )?;
        remove_tree_no_follow(&victim, &mut result);
    }

    let partial = !result.failed.is_empty();
    Ok(Removed {
        removed: result.removed,
        failed: result.failed,
        partial,
        freed_bytes: plan.total_bytes,
    })
}
