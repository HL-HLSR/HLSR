//! `diagnostics/log-policy.js`: какие файлы журнала попадают в отчёт. Предел размера и три
//! архива `main.1.log`…`main.3.log` — те же, что у ротации в `hlsr_core::logging` (она не
//! переносится сюда: `rotate`, `applyPolicy`, категории и `tag` живут там).

use hlsr_core::js::fs::read_dir_names;
use hlsr_core::js::path::NodePath;
use std::path::Path;

/// Файл журнала растёт до 2 МиБ (`MAX_FILE_BYTES`).
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Архивов журнала — три (`MAX_ARCHIVES`).
pub const MAX_ARCHIVES: usize = 3;

/// `archiveName(file, index)`: `…/main.log` → `…/main.1.log`.
pub fn archive_name(file: &str, index: usize) -> String {
    let path = NodePath::native();
    let dir = path.dirname(file);
    let ext = path.extname(file);
    let base = path.basename(file);
    let base = base.strip_suffix(ext.as_str()).unwrap_or(&base);
    path.join(&[&dir, &format!("{base}.{index}{ext}")])
}

/// `logFiles(dir, baseName, { maxArchives })`: основной журнал, затем существующие архивы по номеру.
/// `max_archives` — `None` или 0 дают [`MAX_ARCHIVES`] (`options.maxArchives || MAX_ARCHIVES`).
pub fn log_files(dir: &str, base_name: &str, max_archives: Option<usize>) -> Vec<String> {
    let max_archives = max_archives.filter(|max| *max != 0).unwrap_or(MAX_ARCHIVES);
    let main = NodePath::native().join(&[dir, base_name]);
    let mut files = Vec::new();
    if Path::new(&main).exists() {
        files.push(main.clone());
    }
    for index in 1..=max_archives {
        let archive = archive_name(&main, index);
        if Path::new(&archive).exists() {
            files.push(archive);
        }
    }
    files
}

/// `GAME_LOG_RE`: `^proton-[a-z0-9]+(?:-[a-z0-9]+)*\.log$`.
fn is_game_log_name(name: &str) -> bool {
    let Some(rest) = name
        .strip_prefix("proton-")
        .and_then(|rest| rest.strip_suffix(".log"))
    else {
        return false;
    };
    rest.split('-').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

/// `gameLogFiles(dir)`: вывод umu и Proton последнего запуска каждой игры (Linux,
/// `ProtonRunner`), по имени. Нет каталога — пусто.
pub fn game_log_files(dir: &str) -> Vec<String> {
    let Ok(mut names) = read_dir_names(Path::new(dir)) else {
        return Vec::new();
    };
    names.retain(|name| is_game_log_name(name));
    names.sort_by(|a, b| hlsr_core::js::cmp_utf16(a, b));
    names
        .iter()
        .map(|name| NodePath::native().join(&[dir, name]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_log_names() {
        for good in [
            "proton-ghosting-renewed.log",
            "proton-a.log",
            "proton-a1-b2-c3.log",
            "proton-0.log",
        ] {
            assert!(is_game_log_name(good), "{good}");
        }
        for bad in [
            "proton-Bad_Name.log",
            "proton-.log",
            "proton--a.log",
            "proton-a-.log",
            "proton-a.log.1",
            "Proton-a.log",
            "xproton-a.log",
            "proton-a.LOG",
            "proton-a_b.log",
            "main.log",
        ] {
            assert!(!is_game_log_name(bad), "{bad}");
        }
    }

    #[test]
    fn archive_names() {
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            archive_name(&format!("logs{sep}main.log"), 2),
            format!("logs{sep}main.2.log")
        );
        assert_eq!(archive_name("main", 1), "main.1");
    }
}
