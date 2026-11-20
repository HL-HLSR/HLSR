//! Нагрузка установщика Windows (решение Р1): `HLSR-Setup-<версия>.exe` — это сам `HLSR.exe`, в
//! конец которого дописаны файлы сборки и хвост с сигнатурой. В 0.4.7 установщиком был
//! самораспаковывающийся exe electron-builder (NSIS portable): он распаковывал сборку во временный
//! каталог и запускал её с `PORTABLE_EXECUTABLE_FILE`. Здесь распаковки нет: установка читает файлы
//! прямо из своего exe.
//!
//! Формат файла:
//!
//! ```text
//! [HLSR.exe: N байт][нагрузка: L байт][хвост: 60 байт]
//! нагрузка = u64 LE длина списка, `install-files.json` (те же байты, что в сборке),
//!            файлы списка подряд в порядке списка, кроме `HLSR.exe` — его байты — первые N байт exe
//! хвост    = "HLSR-SETUP-1", u64 LE смещение нагрузки (N), u64 LE длина нагрузки (L), sha256 нагрузки
//! ```
//!
//! Загрузчик Windows не читает дописанное после образа, поэтому `HLSR-Setup.exe` запускается как
//! обычный `HLSR.exe`; установленный `HLSR.exe` — первые N байт, его размер и sha256 стоят в списке
//! файлов. Каждый файл при установке сверяется со списком (размер и sha256), как в 0.4.7; хеш всей
//! нагрузки проверяется при открытии, чтобы битый установщик отказал сразу.
//!
//! Путь В 0.4.7 (апдейтер собирает дерево из чанков и запускает `tree\HLSR.exe` с
//! `PORTABLE_EXECUTABLE_FILE`) тоже работает: источник — каталог exe с `resources\install-files.json`
//! ([`Source::Dir`]). Упаковку делает `tools/release` (задача 8).

use super::file_list::{self, FileItem, FileList, FileListError};
use super::layout;
use hlsr_core::json_text::parse_js_json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Сигнатура хвоста.
pub const SIGNATURE: &[u8; 12] = b"HLSR-SETUP-1";
/// Длина хвоста: сигнатура, смещение, длина, sha256.
pub const TRAILER_LEN: u64 = 12 + 8 + 8 + 32;

/// Хвост установщика.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trailer {
    /// Смещение нагрузки = длина `HLSR.exe`.
    pub offset: u64,
    /// Длина нагрузки.
    pub length: u64,
    /// sha256 нагрузки.
    pub sha256: [u8; 32],
}

impl Trailer {
    /// Байты хвоста.
    pub fn to_bytes(&self) -> [u8; TRAILER_LEN as usize] {
        let mut out = [0u8; TRAILER_LEN as usize];
        out[..12].copy_from_slice(SIGNATURE);
        out[12..20].copy_from_slice(&self.offset.to_le_bytes());
        out[20..28].copy_from_slice(&self.length.to_le_bytes());
        out[28..].copy_from_slice(&self.sha256);
        out
    }

    /// Разобрать хвост файла длиной `file_len`: сигнатура на месте, нагрузка кончается ровно перед
    /// хвостом, перед нагрузкой есть exe.
    pub fn parse(bytes: &[u8; TRAILER_LEN as usize], file_len: u64) -> Option<Trailer> {
        if &bytes[..12] != SIGNATURE {
            return None;
        }
        let number = |range: std::ops::Range<usize>| {
            let mut raw = [0u8; 8];
            raw.copy_from_slice(&bytes[range]);
            u64::from_le_bytes(raw)
        };
        let offset = number(12..20);
        let length = number(20..28);
        let end = offset.checked_add(length)?.checked_add(TRAILER_LEN)?;
        if offset == 0 || length < 8 || end != file_len {
            return None;
        }
        let mut sha256 = [0u8; 32];
        sha256.copy_from_slice(&bytes[28..]);
        Some(Trailer {
            offset,
            length,
            sha256,
        })
    }
}

/// Хвост установщика в конце `exe` (читаются только последние 60 байт); `None` — хвоста нет.
pub fn read_trailer(exe: &Path) -> io::Result<Option<Trailer>> {
    let mut file = File::open(exe)?;
    let len = file.metadata()?.len();
    if len < TRAILER_LEN + 8 {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(len - TRAILER_LEN))?;
    let mut bytes = [0u8; TRAILER_LEN as usize];
    file.read_exact(&mut bytes)?;
    Ok(Trailer::parse(&bytes, len))
}

/// Есть ли в конце `exe` нагрузка установщика: признак режима `Setup` (`hlsr_core::args::ModeEnv`).
pub fn has_trailer(exe: &Path) -> bool {
    matches!(read_trailer(exe), Ok(Some(_)))
}

/// Почему нагрузку не открыть: тексты идут в «Установщик повреждён: … Скачайте его заново.».
#[derive(Debug, thiserror::Error)]
pub enum PayloadError {
    #[error("в установщике нет файлов сборки")]
    Missing,
    #[error("файлы сборки в установщике повреждены")]
    Damaged,
    #[error(transparent)]
    List(#[from] FileListError),
    #[error("установщик не читается: {0}")]
    Io(#[from] io::Error),
}

/// Нагрузка, открытая из exe: список файлов и где лежит каждый файл.
#[derive(Debug, Clone)]
pub struct Payload {
    exe: PathBuf,
    trailer: Trailer,
    list: FileList,
    /// Путь файла списка → смещение в exe.
    offsets: HashMap<String, u64>,
}

impl Payload {
    /// Открыть нагрузку `exe`: хвост, sha256 всей нагрузки, список файлов (проверенный, как
    /// `fileList.read`), раскладка файлов ровно до конца нагрузки.
    pub fn open(exe: &Path) -> Result<Payload, PayloadError> {
        let trailer = read_trailer(exe)?.ok_or(PayloadError::Missing)?;
        let mut file = File::open(exe)?;
        file.seek(SeekFrom::Start(trailer.offset))?;
        let mut hasher = Sha256::new();
        let mut rest = trailer.length;
        let mut buffer = vec![0u8; 1024 * 1024];
        while rest > 0 {
            let want = rest.min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..want])?;
            hasher.update(&buffer[..want]);
            rest -= want as u64;
        }
        if hasher.finalize().as_slice() != trailer.sha256 {
            return Err(PayloadError::Damaged);
        }
        let (list, list_len) = read_list_at(&mut file, &trailer)?;

        let end = trailer.offset + trailer.length;
        let mut cursor = trailer.offset + 8 + list_len;
        let mut offsets = HashMap::new();
        for item in list.files.iter().filter(|item| item.path != layout::EXE) {
            offsets.insert(item.path.clone(), cursor);
            cursor = cursor
                .checked_add(item.size)
                .filter(|next| *next <= end)
                .ok_or(PayloadError::Damaged)?;
        }
        if cursor != end {
            return Err(PayloadError::Damaged);
        }
        Ok(Payload {
            exe: exe.to_owned(),
            trailer,
            list,
            offsets,
        })
    }

    pub fn exe(&self) -> &Path {
        &self.exe
    }

    pub fn trailer(&self) -> &Trailer {
        &self.trailer
    }

    pub fn list(&self) -> &FileList {
        &self.list
    }
}

/// Список файлов нагрузки `exe` без сверки её хеша (проверенный, как `fileList.read`): по нему
/// процесс узнаёт свою сборку до журнала ([`super::setup::app_name_of`]). Целиком нагрузку
/// проверяет [`Payload::open`], когда установщик открывает её.
pub fn read_list(exe: &Path) -> Result<FileList, PayloadError> {
    let trailer = read_trailer(exe)?.ok_or(PayloadError::Missing)?;
    let mut file = File::open(exe)?;
    read_list_at(&mut file, &trailer).map(|(list, _)| list)
}

/// Длина списка (u64 LE в начале нагрузки) и сам список.
fn read_list_at(file: &mut File, trailer: &Trailer) -> Result<(FileList, u64), PayloadError> {
    file.seek(SeekFrom::Start(trailer.offset))?;
    let mut raw = [0u8; 8];
    file.read_exact(&mut raw)?;
    let list_len = u64::from_le_bytes(raw);
    if list_len > trailer.length - 8 || list_len > 64 * 1024 * 1024 {
        return Err(PayloadError::Damaged);
    }
    let mut text = vec![0u8; list_len as usize];
    file.read_exact(&mut text)?;
    let data = parse_js_json(&hlsr_core::js::utf8_string(&text))
        .map_err(|error| FileListError::Unreadable(error.to_string()))?;
    Ok((file_list::from_value(data)?, list_len))
}

/// Откуда установка берёт файлы (`sourceDir` 0.4.7).
#[derive(Debug, Clone)]
pub enum Source {
    /// Распакованная сборка: каталог с `resources\install-files.json` (путь В 0.4.7 и тесты).
    Dir(PathBuf),
    /// Нагрузка в своём exe.
    Payload(Box<Payload>),
}

impl Source {
    /// Каталог, откуда запущен установщик: установка не ставит HLSR в него и не ставит его внутрь
    /// каталога установки. Для нагрузки — каталог exe.
    pub fn location(&self) -> PathBuf {
        match self {
            Source::Dir(dir) => dir.clone(),
            Source::Payload(payload) => payload
                .exe
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
        }
    }

    /// Что установка не может заменить или поглотить (проверка «Установщик запущен из каталога
    /// установки»): каталог распакованной сборки или сам exe с нагрузкой. В 0.4.7 источником был
    /// временный каталог распаковки NSIS, и каталог, где лежит `HLSR-Setup.exe`, в проверку не
    /// попадал; у нагрузки читается только exe, поэтому каталог установки внутри его каталога
    /// (`D:\HLSR-Setup.exe` → `D:\Games\HLSR\Launcher`) допустим.
    pub fn guard(&self) -> PathBuf {
        match self {
            Source::Dir(dir) => dir.clone(),
            Source::Payload(payload) => payload.exe.clone(),
        }
    }

    /// Список файлов сборки (`fileList.read(sourceDir)`).
    pub fn read_list(&self) -> Result<FileList, FileListError> {
        match self {
            Source::Dir(dir) => file_list::read(dir),
            Source::Payload(payload) => Ok(payload.list.clone()),
        }
    }

    /// Байты файла списка. Длину и sha256 сверяет установка.
    pub fn open(&self, item: &FileItem) -> io::Result<Box<dyn Read + Send>> {
        match self {
            Source::Dir(dir) => {
                let mut path = dir.clone();
                for part in item.path.split('/') {
                    path.push(part);
                }
                Ok(Box::new(File::open(path)?))
            }
            Source::Payload(payload) => {
                let mut file = File::open(&payload.exe)?;
                if item.path == layout::EXE {
                    return Ok(Box::new(file.take(payload.trailer.offset)));
                }
                let offset = payload.offsets.get(&item.path).copied().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "файла нет в установщике")
                })?;
                file.seek(SeekFrom::Start(offset))?;
                Ok(Box::new(file.take(item.size)))
            }
        }
    }
}
