//! Архив демо (`demo-archive.js` 0.4.9): список демо игры, выбор, имена в zip и упаковка.
//!
//! Демо HL1 — папки записей (`runsPath` мода и языковые папки), Ghosting — `demoDirs` без
//! вложенных. Выбранные файлы ложатся жёсткими ссылками (или копиями) в `.staging-*` папки
//! `<библиотека>/Demos`, оттуда 7za пакует zip `(hl1|ghosting)-demos-ГГГГ-ММ-ДД-ЧЧММ[-n].zip`.
//! Путь из окна в fs не попадает: упаковываются только демо из свежего списка.

use crate::archive_service::{ArchiveService, CreateOptions, PercentFn};
use crate::errors::{Failure, RawError};
use crate::file_ops::hex;
use crate::manifests::LocalConfig;
use crate::runs_scan::{self, ScanResult};
use chrono::{Datelike, NaiveDateTime, Timelike};
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{Platform, cmp_utf16, date_parse};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::Path;

/// `ARCHIVE_NAME`: `^(hl1|ghosting)-demos-[0-9-]+\.zip$`.
pub fn is_archive_name(name: &str) -> bool {
    ["hl1-demos-", "ghosting-demos-"].iter().any(|prefix| {
        name.strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(".zip"))
            .is_some_and(|stamp| {
                !stamp.is_empty() && stamp.bytes().all(|b| b.is_ascii_digit() || b == b'-')
            })
    })
}

/// Демо в списке: `{dir, name, size, mtimeMs}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Demo {
    /// Папка от папки игры, через `/`.
    pub dir: String,
    pub name: String,
    pub size: u64,
    pub mtime_ms: i64,
}

impl Demo {
    pub fn to_value(&self) -> Value {
        json!({"dir": self.dir, "name": self.name, "size": self.size, "mtimeMs": self.mtime_ms})
    }
}

/// `demoSources(config, installPath)`: папки демо игры и расширение.
pub fn demo_sources(config: &LocalConfig, install_path: &Path) -> (Vec<String>, &'static str) {
    if !config.demo_dirs.is_empty() {
        let dirs = config
            .demo_dirs
            .iter()
            .map(|dir| (*dir).to_owned())
            .collect();
        return (dirs, config.demo_extension.unwrap_or(".dem"));
    }
    if let Some(runs_path) = config.runs_path.filter(|path| !path.is_empty())
        && config.runs_extension == Some(".dem")
    {
        let dirs = if config.runs_language_dirs == Some(true) {
            runs_scan::run_dirs(install_path, runs_path)
        } else {
            vec![runs_path.to_owned()]
        };
        return (dirs, ".dem");
    }
    (Vec::new(), ".dem")
}

/// `listDemos(config, installPath)`: все демо игры, новые сверху. Одинаковые имена в разных
/// папках остаются оба.
pub fn list_demos(config: &LocalConfig, install_path: &Path) -> Vec<Demo> {
    let (dirs, extension) = demo_sources(config, install_path);
    let mut demos = Vec::new();
    for dir in dirs {
        let items =
            match runs_scan::scan_many(install_path, std::slice::from_ref(&dir), Some(extension)) {
                ScanResult::Ok { items, .. } => items,
                // Папки ещё нет — обычное дело; иначе папка не прочитана, список неполный.
                ScanResult::NoDirectory { .. } => continue,
                ScanResult::Error { code } => {
                    tracing::warn!("[demos] папка {dir} не прочитана: {code}");
                    continue;
                }
            };
        demos.extend(items.into_iter().map(|item| Demo {
            dir: dir.clone(),
            mtime_ms: date_parse(&item.modified_at).unwrap_or_default(),
            name: item.name,
            size: item.size,
        }));
    }
    demos.sort_by(|a, b| {
        b.mtime_ms
            .cmp(&a.mtime_ms)
            .then_with(|| cmp_utf16(&a.name, &b.name))
    });
    demos
}

/// `archiveName(slug, date, taken)`: местное время, занятое имя получает `-2`, `-3`…
pub fn archive_name(slug: &str, date: NaiveDateTime, taken: &HashSet<String>) -> String {
    let base = format!(
        "{slug}-demos-{}-{:02}-{:02}-{:02}{:02}",
        date.year(),
        date.month(),
        date.day(),
        date.hour(),
        date.minute()
    );
    let mut name = format!("{base}.zip");
    let mut n = 2;
    while taken.contains(&name) {
        name = format!("{base}-{n}.zip");
        n += 1;
    }
    name
}

/// `pickItems(requested, scanned)`: только то, что есть в свежем списке, — точное совпадение по
/// `dir` и `name`. Ошибка — `NO_ITEMS` или `DEMO_NOT_FOUND`.
pub fn pick_items(requested: &Value, scanned: &[Demo]) -> Result<Vec<Demo>, &'static str> {
    let Some(requested) = requested.as_array().filter(|list| !list.is_empty()) else {
        return Err("NO_ITEMS");
    };
    let mut picked: Vec<usize> = Vec::new();
    for wanted in requested {
        let field = |key: &str| wanted.get(key).and_then(Value::as_str);
        let found = scanned
            .iter()
            .position(|item| {
                field("dir") == Some(item.dir.as_str()) && field("name") == Some(item.name.as_str())
            })
            .ok_or("DEMO_NOT_FOUND")?;
        if !picked.contains(&found) {
            picked.push(found);
        }
    }
    Ok(picked
        .into_iter()
        .map(|index| scanned[index].clone())
        .collect())
}

/// `entryNames(items)`: имя в архиве — имя файла; повтор получает префикс папки, а если и оно
/// занято — `-2`, `-3` перед расширением. Сравнение без учёта регистра.
pub fn entry_names(items: &[Demo]) -> Vec<String> {
    let used: HashSet<String> = items.iter().map(|item| item.name.to_lowercase()).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let (posix, path) = (NodePath::new(Platform::Linux, ""), NodePath::native());
    items
        .iter()
        .map(|item| {
            let mut entry = item.name.clone();
            if seen.contains(&entry.to_lowercase()) {
                let base = format!("{}_{}", posix.basename(&item.dir), item.name);
                let ext = path.extname(&base);
                let stem = &base[..base.len() - ext.len()];
                entry = base.clone();
                let mut n = 2;
                while used.contains(&entry.to_lowercase()) || seen.contains(&entry.to_lowercase()) {
                    entry = format!("{stem}-{n}{ext}");
                    n += 1;
                }
            }
            seen.insert(entry.to_lowercase());
            entry
        })
        .collect()
}

/// Что упаковать (`packDemos({ installPath, items, archivesDir, slug, now, archive, onProgress })`).
pub struct PackRequest<'a> {
    pub install_path: &'a Path,
    pub items: &'a [Demo],
    pub archives_dir: &'a Path,
    /// `hl1` или `ghosting`.
    pub slug: &'a str,
    /// Местное время для имени архива.
    pub now: NaiveDateTime,
    pub archive: &'a ArchiveService,
    pub on_progress: Option<PercentFn>,
}

/// Готовый архив: `{name, size}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packed {
    pub name: String,
    pub size: u64,
}

impl Packed {
    pub fn to_value(&self) -> Value {
        json!({"name": self.name, "size": self.size})
    }
}

/// Демо пропало после списка: ошибка с кодом `DEMO_NOT_FOUND`.
pub fn is_demo_not_found(failure: &Failure) -> bool {
    matches!(failure, Failure::Raw(error) if error.code.as_deref() == Some("DEMO_NOT_FOUND"))
}

/// `packDemos(…)`: zip из выбранных демо. При ошибке не остаётся ни zip, ни `.staging-*`.
pub async fn pack_demos(request: PackRequest<'_>) -> Result<Packed, Failure> {
    let dir = request.archives_dir;
    std::fs::create_dir_all(dir).map_err(|error| RawError::fs(&error, "mkdir", dir))?;
    // Остатки прерванной упаковки; занятая папка не должна ломать новую упаковку.
    for old in names_in(dir)? {
        if old.starts_with(".staging-") {
            let old = dir.join(old);
            let _ = std::fs::remove_dir_all(&old).or_else(|_| std::fs::remove_file(&old));
        }
    }
    let name = archive_name(request.slug, request.now, &names_in(dir)?);
    let zip_path = dir.join(&name);
    let mut random = [0u8; 6];
    getrandom::fill(&mut random).map_err(|error| {
        RawError::new(format!(
            "системный генератор случайных чисел недоступен: {error}"
        ))
    })?;
    let staging = dir.join(format!(".staging-{}", hex(&random)));

    let result = async {
        std::fs::create_dir(&staging).map_err(|error| RawError::fs(&error, "mkdir", &staging))?;
        let entries = entry_names(request.items);
        for (item, entry) in request.items.iter().zip(&entries) {
            let mut source = request.install_path.to_path_buf();
            source.extend(item.dir.split('/'));
            source.push(&item.name);
            stage(&source, &staging.join(entry))?;
        }
        // Список файлов для 7za лежит в staging: после сбоя его уберёт чистка .staging-*.
        request
            .archive
            .create_zip(
                &zip_path,
                CreateOptions {
                    cwd: Some(staging.clone()),
                    list_dir: Some(staging.clone()),
                    files: entries,
                    on_progress: request.on_progress.clone(),
                    ..CreateOptions::default()
                },
            )
            .await?;
        let size = std::fs::metadata(&zip_path)
            .map_err(|error| RawError::fs(&error, "stat", &zip_path))?
            .len();
        Ok(Packed {
            name: name.clone(),
            size,
        })
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_file(&zip_path);
    }
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// Имена в папке.
fn names_in(dir: &Path) -> Result<HashSet<String>, RawError> {
    let entries = std::fs::read_dir(dir).map_err(|error| RawError::fs(&error, "scandir", dir))?;
    Ok(entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect())
}

/// Жёсткая ссылка, иначе копия; исходника нет — `DEMO_NOT_FOUND`.
fn stage(source: &Path, target: &Path) -> Result<(), RawError> {
    if std::fs::hard_link(source, target).is_ok() {
        return Ok(());
    }
    std::fs::copy(source, target).map(|_| ()).map_err(|error| {
        let mut raw = RawError::fs_with_dest(&error, "copyfile", source, Some(target));
        if error.kind() == std::io::ErrorKind::NotFound {
            raw.code = Some("DEMO_NOT_FOUND".to_owned());
        }
        raw
    })
}
