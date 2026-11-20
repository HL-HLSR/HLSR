//! Раскладка библиотеки (`paths.js`): `Games`, `Tools`, `Downloads`, `Backups`,
//! `.hlsr/{staging,state,rollback}`; проверка «путь внутри каталога» и удаление
//! только внутри своей зоны.
//!
//! Функции с суффиксом `_with` считают пути по правилам `path` указанной ОС
//! (сверка ветки Windows на Linux); без суффикса — по правилам этой ОС.

use crate::errors::{Code, DeliveryError, Failure, RawError};
use crate::{file_ops, library_marker};
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime};

/// `LAYOUT`: папки библиотеки относительно корня.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub games: String,
    pub tools: String,
    pub downloads: String,
    pub backups: String,
    pub internal: String,
    pub staging: String,
    pub state: String,
    pub rollback: String,
}

impl Layout {
    /// `LAYOUT` с разделителем `path` этой ОС.
    pub fn of(path: &NodePath) -> Layout {
        Layout {
            games: "Games".to_owned(),
            tools: "Tools".to_owned(),
            downloads: "Downloads".to_owned(),
            backups: "Backups".to_owned(),
            internal: ".hlsr".to_owned(),
            staging: path.join(&[".hlsr", "staging"]),
            state: path.join(&[".hlsr", "state"]),
            rollback: path.join(&[".hlsr", "rollback"]),
        }
    }
}

/// Пути библиотеки (`resolveLibrary`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryPaths {
    pub root: PathBuf,
    pub games: PathBuf,
    pub tools: PathBuf,
    pub downloads: PathBuf,
    pub backups: PathBuf,
    pub internal: PathBuf,
    pub staging: PathBuf,
    pub state: PathBuf,
    pub rollback: PathBuf,
}

impl LibraryPaths {
    /// Каталоги в порядке, в котором их создаёт `ensureLayout`.
    pub fn all(&self) -> [&Path; 9] {
        [
            &self.root,
            &self.games,
            &self.tools,
            &self.downloads,
            &self.backups,
            &self.internal,
            &self.staging,
            &self.state,
            &self.rollback,
        ]
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn path_invalid(message: impl Into<String>) -> DeliveryError {
    DeliveryError::new(Code::PathInvalid, message)
}

/// Корень файловой системы после `resolve`: только у него в конце разделитель
/// (`/`, `C:\`, `\\сервер\ресурс\`) — то, для чего `path.dirname(p) === p`.
fn is_fs_root(path: &NodePath, resolved: &str) -> bool {
    resolved.ends_with(path.sep())
}

/// `resolveLibrary(libraryRoot)` на этой ОС.
pub fn resolve_library(library_root: &str) -> Result<LibraryPaths, DeliveryError> {
    resolve_library_with(&NodePath::native(), library_root)
}

/// `resolveLibrary`: в Windows библиотека обязана лежать на диске с буквой,
/// в Linux — по абсолютному пути; корень диска не годится.
pub fn resolve_library_with(
    path: &NodePath,
    library_root: &str,
) -> Result<LibraryPaths, DeliveryError> {
    let trimmed = js::trim(library_root);
    if trimmed.is_empty() {
        return Err(path_invalid("Путь библиотеки не задан"));
    }
    let root = path.resolve(&[trimmed]);
    let absolute = match path.platform() {
        Platform::Win32 => {
            let b = root.as_bytes();
            b.len() >= 3
                && b[0].is_ascii_alphabetic()
                && b[1] == b':'
                && matches!(b[2], b'\\' | b'/')
        }
        Platform::Linux => trimmed.starts_with('/'),
    };
    if !absolute {
        return Err(path_invalid(format!(
            "Недопустимый путь библиотеки: {library_root}"
        )));
    }
    if is_fs_root(path, &root) {
        return Err(path_invalid("Библиотекой не может быть корень диска"));
    }
    let layout = Layout::of(path);
    let join = |relative: &str| PathBuf::from(path.join(&[&root, relative]));
    Ok(LibraryPaths {
        games: join(&layout.games),
        tools: join(&layout.tools),
        downloads: join(&layout.downloads),
        backups: join(&layout.backups),
        internal: join(&layout.internal),
        staging: join(&layout.staging),
        state: join(&layout.state),
        rollback: join(&layout.rollback),
        root: PathBuf::from(root),
    })
}

/// `ensureLayout(paths)`: создать каталоги; маркер `.hlsr/library.json` — если
/// его нет или он чужой (ошибки маркера не мешают).
pub fn ensure_layout(paths: &LibraryPaths, now: SystemTime) -> Result<(), DeliveryError> {
    // Папки не было — её создаёт сам HLSR (в том числе подпапку HLSR из диалога выбора,
    // withHlsrFolder): полное удаление унесёт её целиком.
    let created = !paths.root.exists();
    for dir in paths.all() {
        std::fs::create_dir_all(dir).map_err(|error| {
            let raw = RawError::fs(&error, "mkdir", dir);
            DeliveryError::new(
                Code::PathNotWritable,
                format!("Не удалось создать {}: {}", dir.display(), raw.message),
            )
        })?;
    }
    if library_marker::read_marker(&paths.root).is_none() {
        let _ = library_marker::write_marker(&paths.root, now, created.then_some(CREATED_BY));
    }
    Ok(())
}

/// `createdBy` маркера папки, которую создал сам лаунчер.
const CREATED_BY: &str = "launcher";

/// Папки библиотеки, которые лаунчер создал сам при выборе пути (`validateInstallPath` создаёт
/// папку для проверки записи). При сохранении выбора (`set-library-path`) такая папка получает
/// отметку «создал лаунчер».
static CREATED_ROOTS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

fn root_key(root: &Path) -> String {
    let resolved = NodePath::native().resolve(&[&text(root)]);
    match Platform::CURRENT {
        Platform::Win32 => resolved.to_lowercase(),
        Platform::Linux => resolved,
    }
}

fn created_roots() -> std::sync::MutexGuard<'static, HashSet<String>> {
    CREATED_ROOTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `prepareLibraryRoot(target)`: создать папку библиотеки; если её не было — запомнить.
pub fn prepare_library_root(target: &Path) -> std::io::Result<()> {
    let existed = target.exists();
    std::fs::create_dir_all(target)?;
    if !existed {
        created_roots().insert(root_key(target));
    }
    Ok(())
}

/// `markCreatedLibrary(root)`: папке, созданной [`prepare_library_root`], — маркер
/// `createdBy: "launcher"` (если маркера ещё нет). `false` — папку создал не лаунчер или сбой.
pub fn mark_created_library(root: &Path, now: SystemTime) -> bool {
    if !created_roots().contains(&root_key(root)) {
        return false;
    }
    library_marker::read_marker(root).is_some()
        || library_marker::write_marker(root, now, Some(CREATED_BY)).is_ok()
}

/// `isInside(root, candidate)` на этой ОС.
pub fn is_inside(root: &Path, candidate: &Path) -> bool {
    is_inside_with(&NodePath::native(), &text(root), &text(candidate))
}

/// `isInside`: `candidate` совпадает с `root` или лежит внутри; в Windows — без учёта регистра.
pub fn is_inside_with(path: &NodePath, root: &str, candidate: &str) -> bool {
    let normalized_root = path.resolve(&[root]);
    let normalized = path.resolve(&[candidate]);
    if normalized == normalized_root {
        return true;
    }
    let prefix = if normalized_root.ends_with(path.sep()) {
        normalized_root
    } else {
        normalized_root + path.sep()
    };
    match path.platform() {
        Platform::Linux => normalized.starts_with(&prefix),
        Platform::Win32 => normalized
            .to_lowercase()
            .starts_with(&prefix.to_lowercase()),
    }
}

/// `assertInside(root, candidate, what)` на этой ОС: путь после `resolve`.
pub fn assert_inside(
    root: &Path,
    candidate: &Path,
    what: Option<&str>,
) -> Result<PathBuf, DeliveryError> {
    assert_inside_with(&NodePath::native(), &text(root), &text(candidate), what).map(PathBuf::from)
}

/// `assertInside`.
pub fn assert_inside_with(
    path: &NodePath,
    root: &str,
    candidate: &str,
    what: Option<&str>,
) -> Result<String, DeliveryError> {
    if !is_inside_with(path, root, candidate) {
        let what = what.filter(|w| !w.is_empty()).unwrap_or("Путь");
        return Err(path_invalid(format!(
            "{what} выходит за пределы разрешённого каталога: {candidate}"
        )));
    }
    Ok(path.resolve(&[candidate]))
}

/// `assertRealInside(root, full, what)`: ближайшая существующая папка на пути к файлу и сам файл
/// лежат в `root` и после разворота ссылок (junction, symlink), иначе запись ушла бы наружу.
/// Сам файл-ссылка не годится. Отказ — `PATH_INVALID`.
pub fn assert_real_inside(
    root: &Path,
    full: &Path,
    what: Option<&str>,
) -> Result<(), DeliveryError> {
    let path = NodePath::native();
    let (root_text, full_text) = (text(root), text(full));
    let what = what.filter(|w| !w.is_empty()).unwrap_or("Путь");
    let fail = || {
        path_invalid(format!(
            "{what} ведёт через ссылку за пределы {root_text}: {full_text}"
        ))
    };
    let exists = |target: &str| std::fs::symlink_metadata(target).is_ok();
    let real = |target: &str| {
        crate::cfg_files::real_path(Path::new(target))
            .map(|real| text(&real))
            .map_err(|_| fail())
    };

    let top = real(&root_text)?;
    let mut probe = path.dirname(&full_text);
    while !exists(&probe) {
        let parent = path.dirname(&probe);
        if parent == probe {
            break;
        }
        probe = parent;
    }
    if !is_inside_with(&path, &top, &real(&probe)?) {
        return Err(fail());
    }
    if let Ok(meta) = std::fs::symlink_metadata(&full_text)
        && (meta.file_type().is_symlink() || !is_inside_with(&path, &top, &real(&full_text)?))
    {
        return Err(fail());
    }
    Ok(())
}

/// Проверки `removeWithinZone` до удаления: что именно удалять (путь после `resolve`).
pub fn removal_target_with(
    path: &NodePath,
    zone_root: &str,
    target: &str,
) -> Result<String, DeliveryError> {
    let zone = path.resolve(&[zone_root]);
    let victim = path.resolve(&[target]);
    if victim == zone {
        return Err(path_invalid(format!("Отказ удалять саму зону {zone}")));
    }
    if !is_inside_with(path, &zone, &victim) {
        return Err(path_invalid(format!(
            "Отказ удалять {victim}: путь вне {zone}"
        )));
    }
    if is_fs_root(path, &victim) {
        return Err(path_invalid("Отказ удалять корень диска"));
    }
    Ok(victim)
}

/// `removeWithinZone(zoneRoot, target)`: удалить дерево внутри зоны, как
/// `fs.rmSync(…, { recursive, force, maxRetries: 5, retryDelay: 200 })`.
pub fn remove_within_zone(zone_root: &Path, target: &Path) -> Result<(), Failure> {
    let victim = removal_target_with(&NodePath::native(), &text(zone_root), &text(target))?;
    file_ops::rm_sync(Path::new(&victim), true, 5, Duration::from_millis(200))?;
    Ok(())
}

/// `/^[a-z0-9]+(-[a-z0-9]+)*$/` (`GAME_ID_RE` 0.4.7): идентификатор игры.
pub fn is_game_id(game_id: &str) -> bool {
    !game_id.is_empty()
        && game_id.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// `gameInstallPath(paths, gameId)` на этой ОС: `Games/<gameId>`.
pub fn game_install_path(paths: &LibraryPaths, game_id: &str) -> Result<PathBuf, DeliveryError> {
    game_install_path_with(&NodePath::native(), paths, game_id).map(PathBuf::from)
}

/// `gameInstallPath`.
pub fn game_install_path_with(
    path: &NodePath,
    paths: &LibraryPaths,
    game_id: &str,
) -> Result<String, DeliveryError> {
    if !is_game_id(game_id) {
        return Err(path_invalid(format!("Недопустимый gameId: {game_id}")));
    }
    let games = text(&paths.games);
    assert_inside_with(
        path,
        &games,
        &path.join(&[&games, game_id]),
        Some("Папка игры"),
    )
}
