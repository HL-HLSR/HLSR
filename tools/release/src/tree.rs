//! Обход и копирование деревьев сборки. Ссылки и специальные файлы в выпуск не попадают:
//! 0.4.7 отказывает сборке со ссылкой (`file-list.js`, `listTree`), и строгий разбор архива
//! Linux их тоже не принимает.

use crate::error::{At, Result, fail};
use std::fs;
use std::path::{Path, PathBuf};

/// Запись дерева: путь от корня через `/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub rel: String,
    pub dir: bool,
    pub size: u64,
    pub executable: bool,
}

impl Entry {
    /// Путь файла на диске.
    pub fn path(&self, root: &Path) -> PathBuf {
        self.rel
            .split('/')
            .fold(root.to_owned(), |path, part| path.join(part))
    }
}

#[cfg(unix)]
fn is_executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_: &fs::Metadata) -> bool {
    false
}

fn name_of(entry: &fs::DirEntry) -> Result<String> {
    match entry.file_name().into_string() {
        Ok(name) => Ok(name),
        Err(name) => fail(format!("Имя файла не в Unicode: {name:?}")),
    }
}

fn walk_into(root: &Path, rel: &str, out: &mut Vec<Entry>) -> Result<()> {
    let dir = if rel.is_empty() {
        root.to_owned()
    } else {
        Entry {
            rel: rel.to_owned(),
            dir: true,
            size: 0,
            executable: false,
        }
        .path(root)
    };
    let mut names = Vec::new();
    for entry in fs::read_dir(&dir).at(&dir)? {
        let entry = entry.at(&dir)?;
        names.push((name_of(&entry)?, entry));
    }
    // Как `tar --sort=name`: по байтам имени.
    names.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    for (name, entry) in names {
        let child = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        let kind = entry.file_type().at(&entry.path())?;
        if kind.is_symlink() {
            return fail(format!("Ссылка в сборке: {child}"));
        }
        if kind.is_dir() {
            out.push(Entry {
                rel: child.clone(),
                dir: true,
                size: 0,
                executable: false,
            });
            walk_into(root, &child, out)?;
        } else if kind.is_file() {
            let meta = entry.metadata().at(&entry.path())?;
            out.push(Entry {
                rel: child,
                dir: false,
                size: meta.len(),
                executable: is_executable(&meta),
            });
        } else {
            return fail(format!("Неизвестный объект в сборке: {child}"));
        }
    }
    Ok(())
}

/// Записи дерева в глубину, имена по байтам; каталог идёт раньше своего содержимого.
pub fn walk_sorted(root: &Path) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    walk_into(root, "", &mut out)?;
    Ok(out)
}

/// Копия дерева `from` в `to` (создаётся). Ссылки — отказ.
pub fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).at(to)?;
    for entry in walk_sorted(from)? {
        let source = entry.path(from);
        let target = entry.path(to);
        if entry.dir {
            fs::create_dir_all(&target).at(&target)?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).at(parent)?;
            }
            fs::copy(&source, &target).at(&source)?;
        }
    }
    Ok(())
}

/// Каталог пуст или не существует.
pub fn is_empty_or_missing(dir: &Path) -> Result<bool> {
    match fs::read_dir(dir) {
        Ok(mut entries) => Ok(entries.next().is_none()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error).at(dir),
    }
}
