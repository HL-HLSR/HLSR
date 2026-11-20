//! Строгая распаковка `.tar.gz` (`launcher-update/archive.js` 0.4.7): потоковый gunzip и разбор USTAR
//! без оболочки и системного `tar`.
//!
//! Принимаются только обычные файлы (`\0`, `'0'`) и каталоги (`'5'`) с контрольной суммой и `ustar`
//! в байтах 257–262; ссылки, устройства, PAX (`x`, `g`) и длинные имена GNU (`L`) — отказ. Путь без
//! `..`, `.`, начального `/`, обратной косой и управляющих символов, без повторов; не больше 50 000
//! записей; сумма размеров не больше предела и (если задано) ровно `expectedBytes`. Права: 0755, если
//! в заголовке есть хоть один бит исполнения, иначе 0644; файлы создаются только новыми (`wx`).
//!
//! Разбор не зависит от ОС ([`TarStream`] и [`Sink`]); запись на диск — [`extract`], права — только Unix.
//! Члены gzip — как у zlib Node: за концом члена ненулевой байт начинает следующий член (мусор — ошибка),
//! нули игнорируются. Отличие: Node отбрасывает распакованное в той же порции, где zlib нашёл ошибку,
//! Rust успевает разобрать его — итог (ошибка gzip) тот же, разница только в недораспакованных файлах
//! каталога `.staging-*`, который всё равно удаляется.

use crate::cancel::CancelToken;
use hlsr_core::js;
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

/// Предел распакованного по умолчанию (`options.maxBytes || 2 ГиБ`).
pub const DEFAULT_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Больше записей — отказ.
pub const MAX_ENTRIES: u64 = 50_000;
const BLOCK: usize = 512;
/// Порция вывода zlib Node (`chunkSize`).
const OUTPUT_CHUNK: usize = 16 * 1024;
/// Порция чтения `createReadStream`.
const INPUT_CHUNK: usize = 64 * 1024;

/// Отказ распаковки. Тексты — как у `Error` в `archive.js`.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("Недопустимое число в tar")]
    BadNumber,
    #[error("Слишком большое поле tar")]
    NumberTooBig,
    #[error("Небезопасный путь в tar")]
    UnsafePath,
    #[error("Выход за каталог установки в tar")]
    Traversal,
    #[error("Данные после конца tar")]
    DataAfterEnd,
    #[error("Повреждённый заголовок USTAR")]
    BadHeader,
    #[error("Ссылки и специальные файлы в tar запрещены")]
    Special,
    #[error("Некорректная запись tar")]
    BadEntry,
    #[error("Превышен размер распакованного архива")]
    TooBig,
    #[error("Повторяющийся путь в tar")]
    Duplicate,
    #[error("Архив tar обрезан")]
    Truncated,
    #[error("Размер распакованных файлов не совпал")]
    SizeMismatch,
    #[error("Распаковка отменена")]
    Aborted,
    /// Сбой gunzip (тексты zlib Node не воспроизводятся).
    #[error("{0}")]
    Gzip(String),
    /// Ошибка ФС с текстом Node (`EEXIST: file already exists, open '…'`).
    #[error("{0}")]
    Fs(String),
}

impl ArchiveError {
    /// Ошибка zlib (`Z_*` у Node).
    pub fn is_gzip(&self) -> bool {
        matches!(self, ArchiveError::Gzip(_))
    }
}

/// Итог распаковки: `{bytes, entries}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extracted {
    pub bytes: u64,
    pub entries: u64,
}

/// Куда идут записи архива. `extract` пишет на диск ([`FsSink`]); тесты могут только считать.
pub trait Sink {
    /// Каталог (`""` — сам каталог назначения).
    fn directory(&mut self, relative: &str) -> Result<(), ArchiveError>;
    /// Каталог-родитель файла (до разбора режима из заголовка, как в 0.4.7).
    fn parent(&mut self, relative: &str) -> Result<(), ArchiveError>;
    /// Новый файл (`wx`), `executable` — права 0755, иначе 0644.
    fn open(&mut self, relative: &str, executable: bool) -> Result<(), ArchiveError>;
    fn write(&mut self, data: &[u8]) -> Result<(), ArchiveError>;
    fn close(&mut self);
}

/// Обратный вызов прогресса: `(transferred, total)` — распакованные байты и предел.
pub type OnProgress<'a> = &'a mut dyn FnMut(u64, u64);

/// `.replace(/\0.*$/, "")`: с самого левого NUL, после которого нет переводов строки, до конца.
fn strip_nul_tail(text: &str) -> &str {
    let start = text
        .char_indices()
        .rev()
        .find(|(_, c)| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
        .map_or(0, |(index, c)| index + c.len_utf8());
    match text[start..].find('\0') {
        Some(offset) => &text[..start + offset],
        None => text,
    }
}

/// `octal(buffer)`: поле `ascii` (старший бит снят), до NUL, без пробелов по краям; только `0–7`.
fn octal(field: &[u8]) -> Result<u64, ArchiveError> {
    let text: String = field.iter().map(|byte| char::from(byte & 0x7f)).collect();
    let text = js::trim(strip_nul_tail(&text));
    if !text.bytes().all(|byte| (b'0'..=b'7').contains(&byte)) {
        return Err(ArchiveError::BadNumber);
    }
    if text.is_empty() {
        return Ok(0);
    }
    let value = u64::from_str_radix(text, 8).map_err(|_| ArchiveError::NumberTooBig)?;
    if value > (1u64 << 53) - 1 {
        return Err(ArchiveError::NumberTooBig);
    }
    Ok(value)
}

/// `entryPath(header)`: `prefix/name`, проверки пути; `""` — корень (`./` или `.`).
fn entry_path(header: &[u8]) -> Result<String, ArchiveError> {
    let field = |start: usize, size: usize| -> String {
        strip_nul_tail(&String::from_utf8_lossy(&header[start..start + size])).to_owned()
    };
    let name = [field(345, 155), field(0, 100)]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if name.starts_with('/')
        || name
            .chars()
            .any(|c| c == '\\' || (c as u32) <= 0x1f || c == '\u{7f}')
    {
        return Err(ArchiveError::UnsafePath);
    }
    let stripped = name.strip_prefix("./").unwrap_or(&name);
    let parts: Vec<&str> = stripped
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if parts.iter().any(|part| *part == ".." || *part == ".") {
        if name == "./" || name == "." {
            return Ok(String::new());
        }
        return Err(ArchiveError::Traversal);
    }
    Ok(parts.join("/"))
}

/// Разбор потока tar порциями, как цикл `for await` в `archive.js`.
pub struct TarStream {
    limit: u64,
    pending: Vec<u8>,
    remaining: u64,
    padding: u64,
    total: u64,
    entries: u64,
    zeros: u64,
    files: HashSet<String>,
}

impl TarStream {
    /// `limit` — предел суммы размеров файлов.
    pub fn new(limit: u64) -> TarStream {
        TarStream {
            limit,
            pending: Vec::new(),
            remaining: 0,
            padding: 0,
            total: 0,
            entries: 0,
            zeros: 0,
            files: HashSet::new(),
        }
    }

    /// Очередная порция распакованного потока.
    pub fn feed(
        &mut self,
        chunk: &[u8],
        sink: &mut dyn Sink,
        on_progress: &mut Option<OnProgress<'_>>,
    ) -> Result<(), ArchiveError> {
        self.pending.extend_from_slice(chunk);
        let mut offset = 0;
        let result = self.consume(&mut offset, sink, on_progress);
        self.pending.drain(..offset);
        result
    }

    fn consume(
        &mut self,
        offset: &mut usize,
        sink: &mut dyn Sink,
        on_progress: &mut Option<OnProgress<'_>>,
    ) -> Result<(), ArchiveError> {
        while *offset < self.pending.len() {
            let available = (self.pending.len() - *offset) as u64;
            if self.remaining > 0 {
                let size = self.remaining.min(available) as usize;
                sink.write(&self.pending[*offset..*offset + size])?;
                self.remaining -= size as u64;
                *offset += size;
                if self.remaining == 0 {
                    sink.close();
                }
                continue;
            }
            if self.padding > 0 {
                let size = self.padding.min(available);
                self.padding -= size;
                *offset += size as usize;
                continue;
            }
            if available < BLOCK as u64 {
                break;
            }
            let header: [u8; BLOCK] = self.pending[*offset..*offset + BLOCK]
                .try_into()
                .expect("блок заголовка");
            *offset += BLOCK;
            if header.iter().all(|byte| *byte == 0) {
                self.zeros += 1;
                continue;
            }
            if self.zeros > 0 {
                return Err(ArchiveError::DataAfterEnd);
            }
            self.entry(&header, sink)?;
            if let Some(on_progress) = on_progress.as_mut() {
                on_progress(self.total, self.limit);
            }
        }
        Ok(())
    }

    fn entry(&mut self, header: &[u8; BLOCK], sink: &mut dyn Sink) -> Result<(), ArchiveError> {
        let checksum = octal(&header[148..156])?;
        let sum: u64 = header
            .iter()
            .enumerate()
            .map(|(index, byte)| {
                if (148..156).contains(&index) {
                    32
                } else {
                    u64::from(*byte)
                }
            })
            .sum();
        if sum != checksum || &header[257..262] != b"ustar" {
            return Err(ArchiveError::BadHeader);
        }
        let kind = header[156];
        if ![0, b'0', b'5'].contains(&kind) {
            return Err(ArchiveError::Special);
        }
        let relative = entry_path(header)?;
        let size = octal(&header[124..136])?;
        let directory = kind == b'5';
        if (relative.is_empty() && !directory) || (directory && size != 0) {
            return Err(ArchiveError::BadEntry);
        }
        self.entries += 1;
        if self.entries > MAX_ENTRIES {
            return Err(ArchiveError::TooBig);
        }
        self.total += size;
        if self.total > self.limit {
            return Err(ArchiveError::TooBig);
        }
        if !self.files.insert(relative.clone()) {
            return Err(ArchiveError::Duplicate);
        }
        if directory {
            sink.directory(&relative)?;
        } else {
            sink.parent(&relative)?;
            let executable = octal(&header[100..108])? & 0o111 != 0;
            sink.open(&relative, executable)?;
            self.remaining = size;
            self.padding = (BLOCK as u64 - size % BLOCK as u64) % BLOCK as u64;
            if self.remaining == 0 {
                sink.close();
            }
        }
        Ok(())
    }

    /// Конец потока: `{bytes, entries}` или «обрезан» / «размер не совпал».
    pub fn finish(&self, expected_bytes: Option<u64>) -> Result<Extracted, ArchiveError> {
        if self.remaining > 0 || self.padding > 0 || !self.pending.is_empty() || self.zeros < 2 {
            return Err(ArchiveError::Truncated);
        }
        if expected_bytes.is_some_and(|expected| expected != self.total) {
            return Err(ArchiveError::SizeMismatch);
        }
        Ok(Extracted {
            bytes: self.total,
            entries: self.entries,
        })
    }
}

/// Запись на диск в каталог назначения: каталоги 0755, файлы `wx` с 0755/0644.
pub struct FsSink {
    destination: PathBuf,
    file: Option<(File, PathBuf)>,
}

impl FsSink {
    pub fn new(destination: &Path) -> FsSink {
        FsSink {
            destination: destination.to_owned(),
            file: None,
        }
    }

    fn target(&self, relative: &str) -> PathBuf {
        if relative.is_empty() {
            self.destination.clone()
        } else {
            self.destination.join(relative)
        }
    }

    fn mkdir(dir: &Path) -> Result<(), ArchiveError> {
        super::make_dirs(dir, 0o755)
            .map_err(|error| ArchiveError::Fs(super::fs_error(&error, "mkdir", dir)))
    }
}

impl Sink for FsSink {
    fn directory(&mut self, relative: &str) -> Result<(), ArchiveError> {
        FsSink::mkdir(&self.target(relative))
    }

    fn parent(&mut self, relative: &str) -> Result<(), ArchiveError> {
        let target = self.target(relative);
        match target.parent() {
            Some(parent) => FsSink::mkdir(parent),
            None => Ok(()),
        }
    }

    fn open(&mut self, relative: &str, executable: bool) -> Result<(), ArchiveError> {
        let target = self.target(relative);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(if executable { 0o755 } else { 0o644 });
        }
        #[cfg(not(unix))]
        let _ = executable;
        let file = options
            .open(&target)
            .map_err(|error| ArchiveError::Fs(super::fs_error(&error, "open", &target)))?;
        self.file = Some((file, target));
        Ok(())
    }

    fn write(&mut self, data: &[u8]) -> Result<(), ArchiveError> {
        if let Some((file, _)) = self.file.as_mut() {
            file.write_all(data).map_err(|error| {
                ArchiveError::Fs(match js::errno::code(&error) {
                    Some(code) => format!("{code}: {}, write", js::errno::description(&code)),
                    None => error.to_string(),
                })
            })?;
        }
        Ok(())
    }

    fn close(&mut self) {
        self.file = None;
    }
}

/// Параметры [`extract`] (`options` 0.4.7).
#[derive(Default)]
pub struct ExtractOptions<'a> {
    /// Предел суммы размеров; `None` или 0 — 2 ГиБ.
    pub max_bytes: Option<u64>,
    /// Ровно столько байт должно получиться (`expectedBytes`).
    pub expected_bytes: Option<u64>,
    /// `signal`: проверяется на каждой порции распакованного потока.
    pub cancel: Option<CancelToken>,
    pub on_progress: Option<OnProgress<'a>>,
}

fn gzip_error(error: &io::Error) -> ArchiveError {
    ArchiveError::Gzip(error.to_string())
}

/// Распаковать поток gzip в разборщик: члены подряд, нули за концом — конец потока.
pub fn gunzip_into(
    reader: impl Read,
    mut each: impl FnMut(&[u8]) -> Result<(), ArchiveError>,
) -> Result<(), ArchiveError> {
    let mut reader = BufReader::with_capacity(INPUT_CHUNK, reader);
    if reader
        .fill_buf()
        .map_err(|error| gzip_error(&error))?
        .is_empty()
    {
        return Err(ArchiveError::Gzip("unexpected end of file".to_owned()));
    }
    let mut buffer = vec![0u8; OUTPUT_CHUNK];
    loop {
        let mut decoder = flate2::bufread::GzDecoder::new(reader);
        loop {
            let count = decoder
                .read(&mut buffer)
                .map_err(|error| gzip_error(&error))?;
            if count == 0 {
                break;
            }
            each(&buffer[..count])?;
        }
        reader = decoder.into_inner();
        let rest = reader.fill_buf().map_err(|error| gzip_error(&error))?;
        if rest.first().is_none_or(|byte| *byte == 0) {
            return Ok(());
        }
    }
}

/// `extract(archive, destination, options)`: распаковать `.tar.gz` в существующий каталог.
pub fn extract(
    archive: &Path,
    destination: &Path,
    mut options: ExtractOptions<'_>,
) -> Result<Extracted, ArchiveError> {
    let limit = options
        .max_bytes
        .filter(|max| *max != 0)
        .unwrap_or(DEFAULT_MAX_BYTES);
    let file = File::open(archive)
        .map_err(|error| ArchiveError::Fs(super::fs_error(&error, "open", archive)))?;
    let mut stream = TarStream::new(limit);
    let mut sink = FsSink::new(destination);
    let cancel = options.cancel.clone();
    let mut on_progress = options.on_progress.take();
    gunzip_into(file, |chunk| {
        if cancel.as_ref().is_some_and(CancelToken::is_cancelled) {
            return Err(ArchiveError::Aborted);
        }
        stream.feed(chunk, &mut sink, &mut on_progress)
    })?;
    sink.close();
    stream.finish(options.expected_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_tail_is_cut_only_after_the_last_line_break() {
        assert_eq!(strip_nul_tail("abc\0def"), "abc");
        assert_eq!(strip_nul_tail("abc\0x\ny"), "abc\0x\ny");
        assert_eq!(strip_nul_tail("a\0b\nc\0d"), "a\0b\nc");
        assert_eq!(strip_nul_tail("plain"), "plain");
    }

    #[test]
    fn octal_fields_like_node() {
        assert_eq!(octal(b"0000755\0").unwrap(), 0o755);
        assert_eq!(octal(b"   0000014 \0").unwrap(), 12);
        assert_eq!(octal(b"\0\0\0\0").unwrap(), 0);
        assert_eq!(octal(&[b'0', 0xb7, 0xb5, 0xb5, 0]).unwrap(), 0o755);
        assert!(matches!(octal(b"12x4567\0"), Err(ArchiveError::BadNumber)));
    }
}
