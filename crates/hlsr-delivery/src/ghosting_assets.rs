//! Картинки и шрифты Ghosting: импорт в мод под именем-хешем и предпросмотр в окне
//! (`ghosting-assets.js`).
//!
//! Импортируется только то, что игра прочитает: форматы проверяются по содержимому, а не по
//! расширению; копия кладётся в `ghostingmod/resource/hlsr-fonts` и
//! `ghostingmod/materials/vgui/logos/hlsr` под sha256 (повторный импорт не пишет файл заново),
//! ссылки на любом шаге — отказ. Предпросмотр читает файлы из папок установки по тем же путям
//! поиска, что у игры (`gameinfo.txt`), и отдаёт `data:`-адрес; TGA браузер не умеет, поэтому он
//! уходит в окно как простой RGBA PNG.

use crate::paths::is_inside_with;
use base64::prelude::{BASE64_STANDARD, Engine as _};
use flate2::Compression;
use flate2::write::ZlibEncoder;
use hlsr_core::js::collate::{Collator, Sensitivity, default_locale};
use hlsr_core::js::fs::{mtime_ms, read_dir};
use hlsr_core::js::path::NodePath;
use hlsr_core::js::text::hex;
use hlsr_core::js::{self, errno};
use sha2::{Digest, Sha256};
use std::fs::{File, Metadata};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

/// Предел размера исходного файла: 16 МБ.
pub const MAX_ASSET_BYTES: u64 = 16 * 1024 * 1024;

/// Ошибка с кодом `ASSET_*`: окно показывает текст по коду.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct AssetError {
    pub code: &'static str,
    pub message: &'static str,
}

const fn fail(code: &'static str, message: &'static str) -> AssetError {
    AssetError { code, message }
}

/// Что может пойти не так на шаге: ошибка с кодом или любая другая (её ловит вызывающий и
/// заменяет своим кодом).
enum Fault {
    Asset(AssetError),
    Other,
}

impl From<AssetError> for Fault {
    fn from(error: AssetError) -> Fault {
        Fault::Asset(error)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Font,
    Spray,
}

impl Kind {
    fn of(name: &str) -> Option<Kind> {
        match name {
            "font" => Some(Kind::Font),
            "spray" => Some(Kind::Spray),
            _ => None,
        }
    }

    fn extensions(self) -> &'static [&'static str] {
        match self {
            Kind::Font => &[".ttf", ".otf"],
            Kind::Spray => &[".png", ".bmp", ".tga"],
        }
    }

    fn directory(self) -> &'static [&'static str] {
        match self {
            Kind::Font => &["resource", "hlsr-fonts"],
            Kind::Spray => &["materials", "vgui", "logos", "hlsr"],
        }
    }
}

fn absolute_path(value: &str) -> Result<String, AssetError> {
    let path = NodePath::native();
    if value.is_empty() || value.contains('\0') || !path.is_absolute(value) {
        return Err(fail(
            "ASSET_INVALID_PATH",
            "An absolute file path is required",
        ));
    }
    Ok(path.resolve(&[value]))
}

fn read_failed() -> AssetError {
    fail("ASSET_READ_FAILED", "The selected asset could not be read")
}

/// Устройство и номер файла для проверки «тот же файл»: (dev, ino) в Linux, (серийный номер
/// тома, индекс файла) в Windows.
#[cfg(unix)]
fn identity(metadata: &Metadata, _file: Option<&File>) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn identity(_metadata: &Metadata, file: Option<&File>) -> Option<(u64, u64)> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let file = file?;
    // SAFETY: дескриптор принадлежит открытому `File`; структура — локальная.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
    if ok == 0 {
        return None;
    }
    Some((
        u64::from(info.dwVolumeSerialNumber),
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    ))
}

/// Идентификатор файла по пути без перехода по ссылке (`lstat`).
#[cfg(unix)]
fn path_identity(path: &Path, metadata: &Metadata) -> Option<(u64, u64)> {
    let _ = path;
    identity(metadata, None)
}

#[cfg(windows)]
fn path_identity(path: &Path, _metadata: &Metadata) -> Option<(u64, u64)> {
    use std::os::windows::fs::OpenOptionsExt as _;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .ok()?;
    identity(_metadata, Some(&file))
}

fn open_for_read(path: &Path) -> std::io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    options.open(path)
}

/// Прочитать исходный файл целиком: обычный файл (не ссылка), не пустой, не больше 16 МБ,
/// и не менявшийся, пока его открывали и читали.
fn read_source(file: &Path) -> Result<Vec<u8>, AssetError> {
    const CHANGED_OPENING: AssetError = fail(
        "ASSET_INVALID_FILE",
        "The selected asset changed while opening it",
    );
    const CHANGED_READING: AssetError = fail(
        "ASSET_INVALID_FILE",
        "The selected asset changed while reading it",
    );
    const TOO_LARGE: AssetError = fail("ASSET_TOO_LARGE", "The selected asset exceeds 16 MB");
    let before = std::fs::symlink_metadata(file).map_err(|_| read_failed())?;
    if !before.is_file() || before.file_type().is_symlink() {
        return Err(fail(
            "ASSET_INVALID_FILE",
            "The selected asset must be a regular file",
        ));
    }
    if before.len() > MAX_ASSET_BYTES {
        return Err(TOO_LARGE);
    }
    let mut handle = open_for_read(file).map_err(|_| read_failed())?;
    let stat = handle.metadata().map_err(|_| read_failed())?;
    if !stat.is_file() || identity(&stat, Some(&handle)) != path_identity(file, &before) {
        return Err(CHANGED_OPENING);
    }
    if stat.len() > MAX_ASSET_BYTES {
        return Err(TOO_LARGE);
    }
    if stat.len() == 0 {
        return Err(fail("ASSET_INVALID_FILE", "The selected asset is empty"));
    }
    let mut bytes = vec![0u8; stat.len() as usize];
    let mut read = 0;
    while read < bytes.len() {
        let count = handle.read(&mut bytes[read..]).map_err(|_| read_failed())?;
        if count == 0 {
            return Err(CHANGED_READING);
        }
        read += count;
    }
    let mut extra = [0u8; 1];
    if handle.read(&mut extra).map_err(|_| read_failed())? != 0 {
        return Err(CHANGED_READING);
    }
    let after = handle.metadata().map_err(|_| read_failed())?;
    let same_time = match (mtime_ms(&after), mtime_ms(&stat)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    };
    if after.len() != stat.len() || !same_time {
        return Err(CHANGED_READING);
    }
    Ok(bytes)
}

// ------------------------------------------------------------ проверка содержимого

fn be32(bytes: &[u8], at: usize) -> u64 {
    u64::from(u32::from_be_bytes([
        bytes[at],
        bytes[at + 1],
        bytes[at + 2],
        bytes[at + 3],
    ]))
}

fn le32(bytes: &[u8], at: usize) -> u64 {
    u64::from(u32::from_le_bytes([
        bytes[at],
        bytes[at + 1],
        bytes[at + 2],
        bytes[at + 3],
    ]))
}

fn le16(bytes: &[u8], at: usize) -> u64 {
    u64::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]))
}

/// `bytes.toString("ascii", from, to) === text`: Node при чтении ASCII сбрасывает старший бит
/// каждого байта, поэтому байты с битом 7 тоже дают те же буквы.
fn ascii_is(bytes: &[u8], from: usize, text: &str) -> bool {
    bytes
        .get(from..from + text.len())
        .is_some_and(|part| part.iter().zip(text.bytes()).all(|(&b, t)| b & 0x7f == t))
}

fn valid_font(bytes: &[u8], extension: &str) -> bool {
    if bytes.len() < 28 {
        return false;
    }
    let valid_signature = if extension == ".otf" {
        ascii_is(bytes, 0, "OTTO")
    } else {
        be32(bytes, 0) == 0x0001_0000 || ascii_is(bytes, 0, "true")
    };
    let tables = u64::from(u16::from_be_bytes([bytes[4], bytes[5]]));
    if !valid_signature || tables == 0 || tables > 256 || 12 + tables * 16 > bytes.len() as u64 {
        return false;
    }
    for index in 0..tables {
        let table = (12 + index * 16) as usize;
        let start = be32(bytes, table + 8);
        let length = be32(bytes, table + 12);
        if start < 12 + tables * 16 || start + length > bytes.len() as u64 {
            return false;
        }
    }
    true
}

fn valid_png(bytes: &[u8]) -> bool {
    const SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 45 || bytes[..8] != SIGNATURE {
        return false;
    }
    if be32(bytes, 8) != 13 || !ascii_is(bytes, 12, "IHDR") {
        return false;
    }
    if be32(bytes, 16) == 0 || be32(bytes, 20) == 0 {
        return false;
    }
    let depths: &[u8] = match bytes[25] {
        0 => &[1, 2, 4, 8, 16],
        2 | 4 | 6 => &[8, 16],
        3 => &[1, 2, 4, 8],
        _ => return false,
    };
    if !depths.contains(&bytes[24]) || bytes[26] != 0 || bytes[27] != 0 || bytes[28] != 0 {
        return false;
    }
    let (mut offset, mut data) = (33u64, false);
    while offset + 12 <= bytes.len() as u64 {
        let at = offset as usize;
        let length = be32(bytes, at);
        if offset + length + 12 > bytes.len() as u64 {
            return false;
        }
        if ascii_is(bytes, at + 4, "IDAT") && length != 0 {
            data = true;
        }
        if ascii_is(bytes, at + 4, "IEND") {
            return length == 0 && data;
        }
        offset += length + 12;
    }
    false
}

fn valid_bmp(bytes: &[u8]) -> bool {
    if bytes.len() < 54 || !ascii_is(bytes, 0, "BM") {
        return false;
    }
    let header_size = le32(bytes, 14);
    let pixels = le32(bytes, 10);
    let width = i64::from(i32::from_le_bytes([
        bytes[18], bytes[19], bytes[20], bytes[21],
    ]));
    let height = i64::from(i32::from_le_bytes([
        bytes[22], bytes[23], bytes[24], bytes[25],
    ]))
    .abs();
    let depth = le16(bytes, 28);
    if ![40, 52, 56, 108, 124].contains(&header_size)
        || 14 + header_size > bytes.len() as u64
        || width <= 0
        || height == 0
        || le16(bytes, 26) != 1
        || ![8, 24, 32].contains(&depth)
        || le32(bytes, 30) != 0
    {
        return false;
    }
    let palette = if depth == 8 {
        let colors = le32(bytes, 46);
        (if colors == 0 { 256 } else { colors }) * 4
    } else {
        0
    };
    let rows = (width as u64 * depth).div_ceil(32) * 4 * height as u64;
    pixels >= 14 + header_size + palette && pixels + rows <= bytes.len() as u64
}

fn valid_tga(bytes: &[u8]) -> bool {
    if bytes.len() < 19 || le16(bytes, 12) == 0 || le16(bytes, 14) == 0 {
        return false;
    }
    let (kind, depth) = (bytes[2], bytes[16]);
    if ![2, 3, 10, 11].contains(&kind) || bytes[1] != 0 || ![8, 24, 32].contains(&depth) {
        return false;
    }
    let pixels = le16(bytes, 12) * le16(bytes, 14);
    let pixel_bytes = u64::from(depth / 8);
    let mut offset = 18 + u64::from(bytes[0]);
    if kind < 9 {
        return offset + pixels * pixel_bytes <= bytes.len() as u64;
    }
    let mut decoded = 0u64;
    while offset < bytes.len() as u64 && decoded < pixels {
        let packet = bytes[offset as usize];
        offset += 1;
        let count = u64::from(packet & 0x7f) + 1;
        decoded += count;
        offset += (if packet & 0x80 != 0 { 1 } else { count }) * pixel_bytes;
        if decoded > pixels || offset > bytes.len() as u64 {
            return false;
        }
    }
    decoded == pixels
}

fn validate_content(bytes: &[u8], extension: &str, kind: Kind) -> Result<(), AssetError> {
    let valid = match (kind, extension) {
        (Kind::Font, _) => valid_font(bytes, extension),
        (_, ".png") => valid_png(bytes),
        (_, ".bmp") => valid_bmp(bytes),
        _ => valid_tga(bytes),
    };
    if valid {
        Ok(())
    } else {
        Err(fail(
            "ASSET_INVALID_FILE",
            "The selected file does not contain a supported font or picture",
        ))
    }
}

// ------------------------------------------------------------------------ импорт

const UNSAFE_DIRECTORY: AssetError = fail(
    "ASSET_UNSAFE_PATH",
    "The asset destination contains a link or is not a directory",
);

fn safe_directory(directory: &Path, create: bool) -> Result<(), Fault> {
    if create {
        match std::fs::create_dir(directory) {
            Ok(()) => {}
            Err(error) if errno::code(&error).as_deref() == Some("EEXIST") => {}
            Err(_) => return Err(Fault::Other),
        }
    }
    let stat = std::fs::symlink_metadata(directory).map_err(|_| Fault::Other)?;
    if !stat.is_dir() || stat.file_type().is_symlink() {
        return Err(UNSAFE_DIRECTORY.into());
    }
    Ok(())
}

/// Папка установки и каждая папка пути назначения — настоящие каталоги, не ссылки; с `create`
/// недостающие папки назначения создаются.
fn prepare_destination(
    install: &Path,
    directories: &[&str],
    create: bool,
) -> Result<PathBuf, Fault> {
    safe_directory(install, false)?;
    let mut directory = install.join("ghostingmod");
    safe_directory(&directory, false)?;
    for segment in directories {
        directory = directory.join(segment);
        safe_directory(&directory, create)?;
    }
    Ok(directory)
}

/// Результат импорта.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedAsset {
    /// Путь от папки мода через `/`: `materials/vgui/logos/hlsr/<sha256>.png`.
    pub value: String,
    /// Имя исходного файла.
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

/// `importAsset(installPath, sourcePath, kind)`: копия файла в мод под именем-хешем.
/// `kind` — `"font"` или `"spray"`.
pub fn import_asset(
    install_path: &str,
    source_path: &str,
    kind: &str,
) -> Result<ImportedAsset, AssetError> {
    let Some(kind) = Kind::of(kind) else {
        return Err(fail("ASSET_BAD_KIND", "Unsupported asset kind"));
    };
    let install = absolute_path(install_path)?;
    let source = absolute_path(source_path)?;
    let path = NodePath::native();
    let extension = path.extname(&source).to_lowercase();
    if !kind.extensions().contains(&extension.as_str()) {
        return Err(fail("ASSET_INVALID_TYPE", "Unsupported file extension"));
    }
    let install = Path::new(&install);
    match prepare_destination(install, &[], false) {
        Ok(_) => {}
        Err(Fault::Asset(error)) if error.code == "ASSET_UNSAFE_PATH" => return Err(error),
        Err(_) => {
            return Err(fail(
                "ASSET_NOT_INSTALLED",
                "The Ghosting Mod installation was not found",
            ));
        }
    }
    let bytes = read_source(Path::new(&source))?;
    validate_content(&bytes, &extension, kind)?;
    let sha256 = hex(&Sha256::digest(&bytes));
    let name = format!("{sha256}{extension}");
    let mut value = kind.directory().join("/");
    value.push('/');
    value.push_str(&name);
    let result = ImportedAsset {
        value,
        name: path.basename(&source),
        bytes: bytes.len() as u64,
        sha256,
    };
    let mut temporary: Option<PathBuf> = None;
    let outcome = write_destination(install, kind, &name, &bytes, &mut temporary);
    if let Some(temporary) = temporary {
        // Лучшее, что можно сделать, — убрать временный файл, если он остался.
        let _ = std::fs::remove_file(temporary);
    }
    match outcome {
        Ok(_) => Ok(result),
        Err(Fault::Asset(error)) => Err(error),
        Err(Fault::Other) => Err(fail(
            "ASSET_WRITE_FAILED",
            "The selected asset could not be copied into the mod",
        )),
    }
}

/// Положить `bytes` в папку назначения под `name`: через временный файл рядом и переименование.
/// `Ok(true)` — файл записан, `Ok(false)` — такой уже лежит (ничего не пишется).
fn write_destination(
    install: &Path,
    kind: Kind,
    name: &str,
    bytes: &[u8],
    temporary: &mut Option<PathBuf>,
) -> Result<bool, Fault> {
    let directory = prepare_destination(install, kind.directory(), true)?;
    let file = directory.join(name);
    match std::fs::symlink_metadata(&file) {
        Ok(stat) => {
            if !stat.is_file() || stat.file_type().is_symlink() {
                return Err(fail(
                    "ASSET_UNSAFE_PATH",
                    "The asset destination is not a regular file",
                )
                .into());
            }
            if stat.len() == bytes.len() as u64 && read_source(&file)? == bytes {
                return Ok(false);
            }
        }
        Err(error) if errno::code(&error).as_deref() == Some("ENOENT") => {}
        Err(_) => return Err(Fault::Other),
    }
    let mut random = [0u8; 12];
    getrandom::fill(&mut random).map_err(|_| Fault::Other)?;
    let temp_path = directory.join(format!(".{name}.{}.tmp", hex(&random)));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o644);
    }
    let mut handle = options.open(&temp_path).map_err(|_| Fault::Other)?;
    *temporary = Some(temp_path.clone());
    let written = handle.write_all(bytes).and_then(|()| handle.sync_all());
    drop(handle);
    written.map_err(|_| Fault::Other)?;
    prepare_destination(install, kind.directory(), false)?;
    std::fs::rename(&temp_path, &file).map_err(|_| Fault::Other)?;
    *temporary = None;
    Ok(true)
}

// ------------------------------------------------------------------- предпросмотр

/// Системные шрифты Windows, которые есть в префиксе Proton: название семейства → файл.
/// Предпросмотр читает только файлы внутри установленного мода или известный шрифт из
/// префикса игры; имя, введённое в окне, произвольным чтением не становится.
const FONT_FAMILIES: &[(&str, &str)] = &[
    ("arial", "arial.ttf"),
    ("arial black", "ariblk.ttf"),
    ("impact", "impact.ttf"),
    ("times new roman", "times.ttf"),
    ("courier new", "cour.ttf"),
    ("verdana", "verdana.ttf"),
    ("georgia", "georgia.ttf"),
    ("trebuchet ms", "trebuc.ttf"),
    ("comic sans ms", "comic.ttf"),
    ("andale mono", "andalemo.ttf"),
    ("calibri", "carlito.ttf"),
    ("carlito", "carlito.ttf"),
    ("segoe ui", "carlito.ttf"),
];

/// Папки каталога: имена подпапок (без ссылок) по `localeCompare` полного пути в нижнем
/// регистре, язык порядка — язык `collator` (`localeCompare` без языка: `Variant`).
fn subdirs(directory: &Path, collator: &Collator) -> Vec<PathBuf> {
    let Ok(entries) = read_dir(directory) else {
        return Vec::new();
    };
    let mut list: Vec<PathBuf> = entries
        .into_iter()
        .filter(|entry| entry.file_type.is_dir())
        .map(|entry| directory.join(entry.name))
        .collect();
    list.sort_by(|a, b| {
        collator.compare(
            &a.to_string_lossy().to_lowercase(),
            &b.to_string_lossy().to_lowercase(),
        )
    });
    list
}

/// Пути поиска игры из `gameinfo.txt` Ghosting, в том же порядке: сначала `custom/*` мода,
/// потом сам мод, его `content/*`, затем `custom/*` и папка `hl2`. Файлы в VPK предпросмотр
/// не читает.
fn search_roots(install: &Path, collator: &Collator) -> Vec<PathBuf> {
    let mod_dir = install.join("ghostingmod");
    let mut roots = subdirs(&mod_dir.join("custom"), collator);
    roots.push(mod_dir.clone());
    roots.extend(subdirs(&mod_dir.join("content"), collator));
    roots.extend(subdirs(&install.join("hl2").join("custom"), collator));
    roots.push(install.join("hl2"));
    roots
}

/// Движок под Windows и Proton не различает регистр, поэтому и путь ищется без учёта
/// регистра: точное имя, если есть, иначе первое совпадение в папке.
fn find_insensitive(root: &Path, segments: &[&str]) -> Option<PathBuf> {
    let mut current = root.to_path_buf();
    for segment in segments {
        let exact = NodePath::native().join(&[&current.to_string_lossy(), segment]);
        let exact = PathBuf::from(exact);
        if exact.exists() {
            current = exact;
            continue;
        }
        let entries = read_dir(&current).ok()?;
        let lower = segment.to_lowercase();
        let found = entries
            .into_iter()
            .find(|entry| entry.name.to_lowercase() == lower)?;
        current =
            PathBuf::from(NodePath::native().join(&[&current.to_string_lossy(), &found.name]));
    }
    Some(current)
}

/// `fs.realpathSync`: настоящий путь. В Windows `canonicalize` возвращает путь с `\\?\`,
/// а Node отдаёт обычный (`C:\…`); приставка снимается.
fn real_path(path: &Path) -> std::io::Result<PathBuf> {
    let real = std::fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        let text = real.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{rest}")));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(rest));
        }
    }
    Ok(real)
}

/// `path.relative(base, real)` — не пусто и вниз от `base`: `real` строго внутри `base`.
fn strictly_inside(base: &Path, real: &Path) -> bool {
    let path = NodePath::native();
    let (base, real) = (base.to_string_lossy(), real.to_string_lossy());
    path.resolve(&[&base]) != path.resolve(&[&real]) && is_inside_with(&path, &base, &real)
}

/// Файл берётся только из папок установки: ссылка наружу не читается.
fn find_in_search_paths(
    install: &str,
    segments: &[&str],
    locale: &str,
) -> Result<Option<PathBuf>, AssetError> {
    let install = Path::new(install);
    let install_real = real_path(install)
        .map_err(|_| fail("ASSET_READ_FAILED", "The preview asset could not be read"))?;
    let mut unsafe_found = false;
    for root in search_roots(install, &Collator::new(locale, Sensitivity::Variant)) {
        let Some(candidate) = find_insensitive(&root, segments) else {
            continue;
        };
        let (Ok(real), Ok(root_real)) = (real_path(&candidate), real_path(&root)) else {
            continue;
        };
        if !strictly_inside(&root_real, &real) || !strictly_inside(&install_real, &real) {
            unsafe_found = true;
            continue;
        }
        if !std::fs::metadata(&real).is_ok_and(|stat| stat.is_file()) {
            continue;
        }
        return Ok(Some(real));
    }
    if unsafe_found {
        return Err(fail("ASSET_UNSAFE_PATH", "Asset is outside the mod"));
    }
    Ok(None)
}

/// Предпросмотр: адрес `data:` с содержимым.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewAsset {
    pub data_url: String,
}

/// `readPreviewAsset(installPath, kind, value, prefix)`: `value` — путь от папки мода (или
/// название системного шрифта, если задан префикс Proton). Порядок папок `custom/*` и
/// `content/*` — по `localeCompare` без языка, то есть по языку по умолчанию процесса
/// ([`default_locale`]).
pub fn read_preview_asset(
    install_path: &str,
    kind: &str,
    value: &str,
    prefix: Option<&Path>,
) -> Result<PreviewAsset, AssetError> {
    read_preview_asset_in(install_path, kind, value, prefix, &default_locale())
}

/// [`read_preview_asset`] с языком порядка папок, заданным явно (BCP 47, `en-US`).
pub fn read_preview_asset_in(
    install_path: &str,
    kind: &str,
    value: &str,
    prefix: Option<&Path>,
    locale: &str,
) -> Result<PreviewAsset, AssetError> {
    let invalid = || fail("ASSET_INVALID_PATH", "Invalid preview asset");
    let kind = Kind::of(kind).ok_or_else(invalid)?;
    if value.encode_utf16().count() > 512 {
        return Err(invalid());
    }
    let prefix = prefix.filter(|prefix| !prefix.as_os_str().is_empty());
    let system_font = match (kind, prefix) {
        (Kind::Font, Some(_)) => {
            let family = js::trim(value).to_lowercase();
            FONT_FAMILIES
                .iter()
                .find(|(name, _)| *name == family)
                .map(|(_, file)| *file)
        }
        _ => None,
    };
    let file: PathBuf = if let (Some(system_font), Some(prefix)) = (system_font, prefix) {
        let directory = prefix.join("drive_c").join("windows").join("Fonts");
        // Префикс, который ещё не подготовлен, папки Fonts не имеет.
        let found = read_dir(&directory).ok().and_then(|entries| {
            entries
                .into_iter()
                .find(|entry| entry.name.to_lowercase() == system_font)
        });
        let Some(found) = found else {
            return Err(fail("ASSET_INVALID_FILE", "Font is not available"));
        };
        directory.join(found.name)
    } else {
        let relative = value.replace('\\', "/");
        if relative.is_empty()
            || relative.starts_with('/')
            || relative.contains(':')
            || relative.split('/').any(|part| part == "..")
        {
            return Err(fail(
                "ASSET_INVALID_PATH",
                "A mod-relative asset path is required",
            ));
        }
        let install = absolute_path(install_path)?;
        let segments: Vec<&str> = relative
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();
        match find_in_search_paths(&install, &segments, locale)? {
            Some(found) => found,
            None => {
                return Err(fail(
                    "ASSET_READ_FAILED",
                    "The preview asset could not be read",
                ));
            }
        }
    };
    let extension = NodePath::native()
        .extname(&file.to_string_lossy())
        .to_lowercase();
    if !kind.extensions().contains(&extension.as_str()) {
        return Err(fail("ASSET_INVALID_TYPE", "Unsupported preview asset"));
    }
    let bytes = read_source(&file)?;
    validate_content(&bytes, &extension, kind)?;
    let output = if extension == ".tga" {
        tga_to_png(&bytes)?
    } else {
        bytes
    };
    let mime = match (kind, extension.as_str()) {
        (Kind::Font, ".otf") => "font/otf",
        (Kind::Font, _) => "font/ttf",
        (_, ".bmp") => "image/bmp",
        _ => "image/png",
    };
    Ok(PreviewAsset {
        data_url: format!("data:{mime};base64,{}", BASE64_STANDARD.encode(output)),
    })
}

/// Браузер не читает TGA, поэтому предпросмотр переводит его в простой RGBA PNG.
fn tga_to_png(input: &[u8]) -> Result<Vec<u8>, AssetError> {
    let invalid = |message: &'static str| fail("ASSET_INVALID_FILE", message);
    let width = le16(input, 12) as usize;
    let height = le16(input, 14) as usize;
    let depth = usize::from(input[16] / 8);
    if width * height > 4 * 1024 * 1024 {
        return Err(fail(
            "ASSET_TOO_LARGE",
            "Preview image dimensions are too large",
        ));
    }
    let total = width * height;
    let mut position = 18 + usize::from(input[0]);
    let mut pixel = 0usize;
    let mut pixels = vec![0u8; total * 4];
    let rle = input[2] >= 9;
    let descriptor = input[17];

    let read = |position: &mut usize| -> Result<[u8; 4], AssetError> {
        if *position + depth > input.len() {
            return Err(invalid("Truncated TGA image"));
        }
        let at = *position;
        let rgba = if depth == 1 {
            [input[at], input[at], input[at], 255]
        } else {
            [
                input[at + 2],
                input[at + 1],
                input[at],
                if depth == 4 { input[at + 3] } else { 255 },
            ]
        };
        *position += depth;
        Ok(rgba)
    };
    let mut write_pixel = |pixel: &mut usize, rgba: [u8; 4]| -> Result<(), AssetError> {
        if *pixel >= total {
            return Err(invalid("Invalid TGA run"));
        }
        let (mut x, mut y) = (*pixel % width, *pixel / width);
        if descriptor & 32 == 0 {
            y = height - y - 1;
        }
        if descriptor & 16 != 0 {
            x = width - x - 1;
        }
        let offset = (y * width + x) * 4;
        pixels[offset..offset + 4].copy_from_slice(&rgba);
        *pixel += 1;
        Ok(())
    };

    while pixel < total {
        if !rle {
            let rgba = read(&mut position)?;
            write_pixel(&mut pixel, rgba)?;
            continue;
        }
        if position >= input.len() {
            return Err(invalid("Truncated TGA run"));
        }
        let packet = input[position];
        position += 1;
        let count = usize::from(packet & 127) + 1;
        if packet & 128 != 0 {
            let rgba = read(&mut position)?;
            for _ in 0..count {
                write_pixel(&mut pixel, rgba)?;
            }
        } else {
            for _ in 0..count {
                let rgba = read(&mut position)?;
                write_pixel(&mut pixel, rgba)?;
            }
        }
    }

    let mut rows = Vec::with_capacity(height * (width * 4 + 1));
    for y in 0..height {
        rows.push(0);
        rows.extend_from_slice(&pixels[y * width * 4..(y + 1) * width * 4]);
    }
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&(width as u32).to_be_bytes());
    header.extend_from_slice(&(height as u32).to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&rows)
        .map_err(|_| invalid("Truncated TGA image"))?;
    let compressed = encoder
        .finish()
        .map_err(|_| invalid("Truncated TGA image"))?;

    let mut png = vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    for (name, data) in [
        (b"IHDR", header.as_slice()),
        (b"IDAT", compressed.as_slice()),
        (b"IEND", &[][..]),
    ] {
        let mut body = name.to_vec();
        body.extend_from_slice(data);
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        png.extend_from_slice(&body);
        png.extend_from_slice(&crc32fast::hash(&body).to_be_bytes());
    }
    Ok(png)
}
