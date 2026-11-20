//! Чтение файла и каталога, как `fs.readFileSync` и `fs.readdirSync` Node: в Windows libuv
//! открывает и каталог (`FILE_FLAG_BACKUP_SEMANTICS`), и ошибка приходит при чтении —
//! `EISDIR`, а не `EPERM` при открытии, как у `std::fs::read`. Список каталога в Linux
//! отсортирован по байтам имён (`scandir` libuv), в Windows идёт в порядке тома.

use std::fs::{FileType, Metadata};
use std::io::{self, Read as _};
use std::path::Path;
use std::time::UNIX_EPOCH;

/// Байты файла.
pub fn read_file(path: &Path) -> io::Result<Vec<u8>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    }
    let mut bytes = Vec::new();
    options.open(path)?.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Элемент каталога: `fs.Dirent` из `readdirSync(dir, { withFileTypes: true })`.
/// `file_type` не раскрывает ссылки: ссылка на каталог — не каталог.
#[derive(Debug)]
pub struct DirEntry {
    pub name: String,
    pub file_type: FileType,
}

/// `fs.readdirSync(dir, { withFileTypes: true })`. Имя с недопустимым UTF-8 читается
/// с заменой на U+FFFD, как у Node.
pub fn read_dir(path: &Path) -> io::Result<Vec<DirEntry>> {
    let mut entries: Vec<(std::ffi::OsString, FileType)> = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        entries.push((entry.file_name(), entry.file_type()?));
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    }
    Ok(entries
        .into_iter()
        .map(|(name, file_type)| DirEntry {
            name: name.to_string_lossy().into_owned(),
            file_type,
        })
        .collect())
}

/// `fs.readdirSync(dir)`: имена элементов каталога.
pub fn read_dir_names(path: &Path) -> io::Result<Vec<String>> {
    Ok(read_dir(path)?.into_iter().map(|e| e.name).collect())
}

/// `stats.mtimeMs`: `секунды × 1000 + наносекунды / 1e6` в числах с плавающей точкой,
/// как считает JS из полей libuv. Время до эпохи: секунды отрицательные, наносекунды —
/// положительная доля.
pub fn mtime_ms(metadata: &Metadata) -> io::Result<f64> {
    let modified = metadata.modified()?;
    let (seconds, nanos) = match modified.duration_since(UNIX_EPOCH) {
        Ok(after) => (after.as_secs() as i64, after.subsec_nanos()),
        Err(error) => {
            let before = error.duration();
            match before.subsec_nanos() {
                0 => (-(before.as_secs() as i64), 0),
                fraction => (-(before.as_secs() as i64) - 1, 1_000_000_000 - fraction),
            }
        }
    };
    Ok(seconds as f64 * 1000.0 + f64::from(nanos) / 1e6)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Порядок, который `fs.readdirSync` Node 24 отдал в Linux для тех же имён (снят вручную:
    /// `scandir` libuv сортирует по байтам имён).
    #[cfg(unix)]
    #[test]
    fn read_dir_lists_names_by_bytes_like_node_on_linux() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["b", "B", "a", "é", "Z", "z", "10", "9", "_", "-", "ы"] {
            std::fs::write(dir.path().join(name), "x").unwrap();
        }
        assert_eq!(
            read_dir_names(dir.path()).unwrap(),
            ["-", "10", "9", "B", "Z", "_", "a", "b", "z", "é", "ы"]
        );
        std::fs::create_dir(dir.path().join("d")).unwrap();
        let kinds: Vec<(String, bool)> = read_dir(dir.path())
            .unwrap()
            .into_iter()
            .filter(|e| e.name == "a" || e.name == "d")
            .map(|e| (e.name, e.file_type.is_dir()))
            .collect();
        assert_eq!(kinds, [("a".to_owned(), false), ("d".to_owned(), true)]);
    }

    #[test]
    fn read_dir_reports_missing_directory_and_plain_file_like_node() {
        let dir = tempfile::tempdir().unwrap();
        let missing = read_dir(&dir.path().join("none")).unwrap_err();
        assert_eq!(crate::js::errno::code(&missing).as_deref(), Some("ENOENT"));
        let file = dir.path().join("file");
        std::fs::write(&file, "x").unwrap();
        let not_dir = read_dir(&file).unwrap_err();
        let code = crate::js::errno::code(&not_dir);
        // В Windows путь к файлу libuv сводит к ENOENT (ERROR_PATH_NOT_FOUND), в Linux — ENOTDIR.
        assert!(
            matches!(code.as_deref(), Some("ENOTDIR" | "ENOENT")),
            "{code:?}"
        );
    }

    /// `mtimeMs` — `секунды × 1000 + наносекунды / 1e6`; `Date` из него округляется
    /// (`fs.utimesSync(…, 1700000000.5)` дал в Node `1700000000500`).
    #[test]
    fn mtime_ms_is_seconds_times_1000_plus_nanos_over_1e6() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, "x").unwrap();
        let handle = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
        let time = UNIX_EPOCH + Duration::new(1_700_000_000, 500_000_000);
        handle
            .set_times(std::fs::FileTimes::new().set_modified(time))
            .unwrap();
        drop(handle);
        let ms = mtime_ms(&std::fs::metadata(&file).unwrap()).unwrap();
        assert_eq!(ms, 1_700_000_000_500.0);
        assert_eq!(crate::js::math_round(ms), 1_700_000_000_500.0);
    }
}
