//! Архив лаунчера Linux: строгий USTAR `.tar.gz`, который распаковывает `launcher-update/archive.js`
//! 0.4.7 (и его перенос `hlsr_update::linux::archive`): только обычные файлы и каталоги, без
//! ссылок, PAX и длинных имён GNU. Вместо `build-launcher-archive.js`
//! (`tar --format=ustar --sort=name --owner=0 --group=0 --numeric-owner`) архив пишет крейт `tar`
//! по тем же правилам: корень `./`, записи в глубину по байтам имени, владелец 0/0 числом, права
//! 0755 у исполняемых и 0644 у остальных, время — `SOURCE_DATE_EPOCH` или 0 (одинаковые байты при
//! каждой сборке).
//!
//! Что обязан содержать архив, чтобы 0.4.7 его установила (`ArchiveInstall.verifyStage`):
//! `.hlsr-release.json`, ELF x86-64 `hlsr`, `resources/app.asar` (в 0.5 — заглушка: Rust-лаунчер её не
//! читает) и `resources/icons/hlsr.png`. `.hlsr-release.json` и заглушка пишутся в само дерево, как
//! 0.4.7 писала маркер в `linux-unpacked`: по этому же дереву `launcher-release-prepare-linux.js`
//! режет куски обновления, и собранное из них дерево должно пройти ту же проверку.

use crate::error::{At, ReleaseError, Result, fail};
use crate::hashes::{self, Digests};
use crate::tree::{self, Entry};
use crate::version;
use flate2::{Compression, GzBuilder};
use hlsr_core::json_formats::{compact, pretty2};
use hlsr_update::linux::archive::{ArchiveError, DEFAULT_MAX_BYTES, Sink, TarStream, gunzip_into};
use hlsr_update::linux::embedded_payload::{self, Describe};
use serde_json::json;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

/// Заглушка `resources/app.asar`: любой непустой файл, проверка 0.4.7 смотрит только на `isFile()`.
pub const APP_ASAR_STUB: &str = "HLSR 0.5: заглушка app.asar. Лаунчер на Rust этот файл не читает; он нужен проверке установки 0.4.7.\n";

/// Что упаковать.
#[derive(Debug, Clone)]
pub struct ArchiveOptions {
    /// Дерево сборки: `hlsr`, `resources/…`. Дополняется маркером и заглушкой.
    pub tree: PathBuf,
    pub version: String,
    pub out: PathBuf,
    /// Файлы дерева (через `/`), которым нужны права 0755 помимо `hlsr` и файлов с битом исполнения.
    pub executables: Vec<String>,
    /// Время записей; `None` — `SOURCE_DATE_EPOCH` или 0.
    pub mtime: Option<u64>,
}

/// Что получилось.
#[derive(Debug, Clone)]
pub struct ArchiveOutput {
    pub archive: PathBuf,
    /// `<архив>.json`: `{"version","unpackedSize"}`.
    pub info: PathBuf,
    /// `payload.json` рядом с архивом.
    pub descriptor: PathBuf,
    pub digests: Digests,
    pub unpacked_size: u64,
    pub entries: u64,
}

/// `HLSR-<версия>-linux-x86_64.tar.gz`.
pub fn archive_name(version: &str) -> String {
    embedded_payload::archive_name(version)
}

/// `.hlsr-release.json`: `JSON.stringify({version, platform, arch, executable}) + "\n"`.
pub fn release_marker(version: &str) -> String {
    compact(&json!({
        "version": version,
        "platform": "linux",
        "arch": "x64",
        "executable": "hlsr",
    })) + "\n"
}

/// ELF64 little-endian x86-64 (`ArchiveInstall.verifyStage`: `7f454c46`, класс 2, порядок 1, машина 62).
pub fn is_linux_x64_elf(header: &[u8]) -> bool {
    header.len() >= 20
        && header[..4] == [0x7f, b'E', b'L', b'F']
        && header[4] == 2
        && header[5] == 1
        && u16::from_le_bytes([header[18], header[19]]) == 62
}

const PNG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

fn read_head(path: &Path, len: usize) -> Result<Vec<u8>> {
    use std::io::Read as _;
    let mut head = Vec::new();
    File::open(path)
        .at(path)?
        .take(len as u64)
        .read_to_end(&mut head)
        .at(path)?;
    Ok(head)
}

/// Дерево годно для архива, который примет 0.4.7; недостающее служебное дописывается.
fn prepare_tree(options: &ArchiveOptions) -> Result<()> {
    let tree = &options.tree;
    if !tree.is_dir() {
        return fail(format!("Нет дерева сборки: {}", tree.display()));
    }
    let exe = tree.join("hlsr");
    if !exe.is_file() {
        return fail(format!("В сборке нет hlsr: {}", exe.display()));
    }
    if !is_linux_x64_elf(&read_head(&exe, 20)?) {
        return fail("hlsr — не ELF x86-64 для Linux: архив не пройдёт проверку установки 0.4.7");
    }
    let icon = tree.join("resources").join("icons").join("hlsr.png");
    if !icon.is_file() {
        return fail("В сборке нет resources/icons/hlsr.png: из него берётся значок меню");
    }
    if read_head(&icon, 8)? != PNG {
        return fail("resources/icons/hlsr.png — не PNG");
    }
    let asar = tree.join("resources").join("app.asar");
    if !asar.exists() {
        fs::write(&asar, APP_ASAR_STUB).at(&asar)?;
    }
    let marker = tree.join(".hlsr-release.json");
    fs::write(&marker, release_marker(&options.version)).at(&marker)?;
    Ok(())
}

fn mtime_of(options: &ArchiveOptions) -> u64 {
    options.mtime.unwrap_or_else(|| {
        std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0)
    })
}

fn header(path: &str, dir: bool, size: u64, executable: bool, mtime: u64) -> Result<tar::Header> {
    let mut header = tar::Header::new_ustar();
    header.set_path(path).map_err(|error| {
        ReleaseError::Message(format!(
            "Путь не умещается в USTAR без расширений (имя 100 + префикс 155 байт): {path}: {error}"
        ))
    })?;
    header.set_entry_type(if dir {
        tar::EntryType::Directory
    } else {
        tar::EntryType::Regular
    });
    header.set_mode(if dir || executable { 0o755 } else { 0o644 });
    header.set_uid(0);
    header.set_gid(0);
    header.set_size(size);
    header.set_mtime(mtime);
    header.set_cksum();
    Ok(header)
}

/// Записать `.tar.gz` дерева: корень `./`, затем записи в глубину.
fn write_archive(options: &ArchiveOptions, entries: &[Entry], target: &Path) -> Result<()> {
    let mtime = mtime_of(options);
    let file = BufWriter::new(File::create(target).at(target)?);
    let gzip = GzBuilder::new()
        .operating_system(3)
        .write(file, Compression::new(9));
    let mut builder = tar::Builder::new(gzip);
    builder
        .append(&header("./", true, 0, true, mtime)?, io::empty())
        .at(target)?;
    for entry in entries {
        let extra = options.executables.contains(&entry.rel);
        let executable = entry.executable || extra || entry.rel == "hlsr";
        if entry.dir {
            builder
                .append(
                    &header(&format!("{}/", entry.rel), true, 0, true, mtime)?,
                    io::empty(),
                )
                .at(target)?;
        } else {
            let path = entry.path(&options.tree);
            let data = File::open(&path).at(&path)?;
            builder
                .append(
                    &header(&entry.rel, false, entry.size, executable, mtime)?,
                    data,
                )
                .at(&path)?;
        }
    }
    let gzip = builder.into_inner().at(target)?;
    let mut file = gzip.finish().at(target)?;
    file.flush().at(target)?;
    Ok(())
}

/// Счёт записей без записи на диск: проверка готового архива строгим разбором 0.4.7.
#[derive(Default)]
struct CountingSink {
    files: u64,
}

impl Sink for CountingSink {
    fn directory(&mut self, _: &str) -> std::result::Result<(), ArchiveError> {
        Ok(())
    }
    fn parent(&mut self, _: &str) -> std::result::Result<(), ArchiveError> {
        Ok(())
    }
    fn open(&mut self, _: &str, _: bool) -> std::result::Result<(), ArchiveError> {
        self.files += 1;
        Ok(())
    }
    fn write(&mut self, _: &[u8]) -> std::result::Result<(), ArchiveError> {
        Ok(())
    }
    fn close(&mut self) {}
}

/// Прогнать архив через разбор клиента: USTAR, пути, число записей, сумма размеров.
fn verify_archive(archive: &Path, unpacked_size: u64) -> Result<u64> {
    let file = File::open(archive).at(archive)?;
    let mut stream = TarStream::new(DEFAULT_MAX_BYTES);
    let mut sink = CountingSink::default();
    gunzip_into(file, |chunk| stream.feed(chunk, &mut sink, &mut None))
        .and_then(|()| stream.finish(Some(unpacked_size)))
        .map(|done| done.entries)
        .map_err(|error| {
            ReleaseError::Message(format!(
                "Готовый архив не проходит строгий разбор обновлялки: {error}"
            ))
        })
}

/// Собрать архив, `<архив>.json` и `payload.json`.
pub fn run(options: &ArchiveOptions) -> Result<ArchiveOutput> {
    version::require_release(&options.version)?;
    prepare_tree(options)?;
    let entries = tree::walk_sorted(&options.tree)?;
    let unpacked_size: u64 = entries.iter().map(|entry| entry.size).sum();
    if unpacked_size == 0 || unpacked_size > embedded_payload::MAX_UNPACKED_BYTES {
        return fail(format!(
            "Распакованный размер {unpacked_size} вне предела обновлялки (1…{} байт)",
            embedded_payload::MAX_UNPACKED_BYTES
        ));
    }

    fs::create_dir_all(&options.out).at(&options.out)?;
    let archive = options.out.join(archive_name(&options.version));
    let partial = options
        .out
        .join(format!("{}.partial", archive_name(&options.version)));
    if let Err(error) = write_archive(options, &entries, &partial) {
        let _ = fs::remove_file(&partial);
        return Err(error);
    }
    let _ = fs::remove_file(&archive);
    fs::rename(&partial, &archive).at(&archive)?;
    let parsed = verify_archive(&archive, unpacked_size)?;

    let digests = hashes::digests(&archive)?;
    let info = options
        .out
        .join(format!("{}.json", archive_name(&options.version)));
    fs::write(
        &info,
        compact(&json!({"version": options.version, "unpackedSize": unpacked_size})) + "\n",
    )
    .at(&info)?;

    let descriptor = embedded_payload::describe(&Describe {
        version: &options.version,
        size: digests.size,
        sha256: &digests.sha256,
        sha512: &digests.sha512,
        unpacked_size,
    });
    let problems = embedded_payload::problems_of(&descriptor);
    if !problems.is_empty() {
        return fail(format!(
            "Некорректное описание встроенного архива: {}",
            problems.join("; ")
        ));
    }
    let descriptor_path = options.out.join(embedded_payload::DESCRIPTOR);
    fs::write(&descriptor_path, pretty2(&descriptor) + "\n").at(&descriptor_path)?;

    Ok(ArchiveOutput {
        archive,
        info,
        descriptor: descriptor_path,
        digests,
        unpacked_size,
        entries: parsed,
    })
}
