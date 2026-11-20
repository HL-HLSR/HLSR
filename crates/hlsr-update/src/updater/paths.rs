//! Куда апдейтеру можно писать (`updater/core/paths.js`): не в пользовательские файлы и не в
//! каталоги `games`/`downloads` (кроме разрешённых корней), рабочий каталог — во временной зоне и
//! вне каталога установки. Пути сравниваются как `path.win32.resolve(…).toLowerCase()` без
//! хвостовых разделителей.

use crate::errors::{UpdCode, UpdaterError};
use crate::windows::layout::{resolve_lower, win_path};

/// `PROTECTED_SEGMENTS`: имена файлов и каталогов пользователя.
pub const PROTECTED_SEGMENTS: [&str; 6] = [
    "games",
    "downloads",
    "game-stats.json",
    "game-settings.json",
    "settings.json",
    "library.json",
];

/// `normalize(value)`: `path.resolve(String(value || "")).toLowerCase().replace(/[\\/]+$/, "")`.
pub fn normalize(value: &str) -> String {
    resolve_lower(&win_path(), value)
        .trim_end_matches(['\\', '/'])
        .to_owned()
}

/// `isInside(parent, child)`: тот же путь или внутри (`\` или `/` после родителя).
pub fn is_inside(parent: &str, child: &str) -> bool {
    let p = normalize(parent);
    let c = normalize(child);
    c == p || c.starts_with(&format!("{p}\\")) || c.starts_with(&format!("{p}/"))
}

/// `assertWritable(target, {allowedRoots, allowedFiles})`; пустые корни и файлы отбрасываются
/// (`filter(Boolean)`).
pub fn assert_writable(
    target: &str,
    allowed_roots: &[&str],
    allowed_files: &[&str],
) -> Result<(), UpdaterError> {
    let roots: Vec<&str> = allowed_roots
        .iter()
        .copied()
        .filter(|r| !r.is_empty())
        .collect();
    let normalized = normalize(target);

    if allowed_files
        .iter()
        .filter(|file| !file.is_empty())
        .any(|file| normalize(file) == normalized)
    {
        return Ok(());
    }

    let base = win_path().basename(&normalized);
    if PROTECTED_SEGMENTS.contains(&base.as_str()) {
        return Err(UpdaterError::new(
            UpdCode::BadPlan,
            format!("запись в пользовательский файл запрещена: {base}"),
        ));
    }

    let parts: Vec<&str> = normalized.split(['\\', '/']).collect();
    for guard in ["games", "downloads"] {
        if parts.contains(&guard) && !roots.iter().any(|root| is_inside(root, target)) {
            return Err(UpdaterError::new(
                UpdCode::BadPlan,
                format!("запись в каталог {guard} запрещена"),
            ));
        }
    }

    if roots.is_empty() || roots.iter().any(|root| is_inside(root, target)) {
        return Ok(());
    }
    Err(UpdaterError::new(
        UpdCode::BadPlan,
        "запись вне разрешённых каталогов",
    ))
}

/// `assertSafeWorkDir(workDir, {tempRoot, installDir})`.
pub fn assert_safe_work_dir(
    work_dir: &str,
    temp_root: &str,
    install_dir: Option<&str>,
) -> Result<(), UpdaterError> {
    if let Some(install_dir) = install_dir.filter(|dir| !dir.is_empty())
        && is_inside(install_dir, work_dir)
    {
        return Err(UpdaterError::new(
            UpdCode::StagingFailed,
            "рабочий каталог Updater не может лежать внутри каталога установки",
        ));
    }
    if !is_inside(temp_root, work_dir) {
        return Err(UpdaterError::new(
            UpdCode::StagingFailed,
            "рабочий каталог Updater вне временной зоны",
        ));
    }
    Ok(())
}
