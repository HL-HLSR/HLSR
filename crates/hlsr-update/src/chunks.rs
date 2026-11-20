//! Обновление лаунчера чанками (`launcher-update/chunks.js`).
//!
//! При выпуске каждый файл сборки режется на куски по содержимому (content-defined chunking:
//! граница ставится там, где скользящий gear-хеш даёт нужные нули, поэтому правка в середине
//! файла меняет только соседние куски). Кусок называется своим SHA-256, сжимается brotli и
//! кладётся в общее хранилище `chunks/<aa>/<sha256>`: одинаковые куски разных версий хранятся
//! один раз. Список файлов версии и их кусков — манифест чанков; его SHA-256 и адрес лежат в
//! подписанном `launcher-channel.json` (блок `delta`), так что манифест и через него каждый
//! кусок проверяются той же подписью, что и установщик.
//!
//! Клиент собирает новую версию рядом со старой: совпавшие целиком файлы копирует, куски
//! изменённых файлов, которые уже есть в старой версии, берёт с диска, а скачивает только
//! недостающие. Каждый кусок и каждый собранный файл сверяются по SHA-256.
//!
//! Модуль общий для лаунчера (Linux), апдейтера (Windows) и проверок выпуска. Выпуск режет
//! сборку в Node (скрипты выпуска, Р8); здесь [`build_store`] нужна тестам сборки версии: она
//! даёт те же границы и тот же манифест.

use crate::cancel::CancelToken;
use crate::channel::{
    field, is_allowed_url, is_integer, is_release_semver, is_sha256, regex_input,
};
use crate::ports::{BoxFuture, Clock, Logger, SystemClock};
use hlsr_core::js::text::hex;
use hlsr_core::js::{self, Platform};
use hlsr_core::json_text;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{self, Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinSet;
use url::Url;

pub const SCHEMA: u64 = 1;
pub const KIND: &str = "hlsr-launcher-chunks";
/// Параметры нарезки — часть формата: клиент режет старые файлы так же, как скрипт выпуска резал
/// новые. Менять только вместе с [`ALGORITHM`].
pub const ALGORITHM: &str = "gear-cdc-v1";
pub const MIN_CHUNK: usize = 16 * 1024;
pub const AVG_CHUNK: usize = 64 * 1024;
pub const MAX_CHUNK: usize = 256 * 1024;
const MASK_STRICT: u32 = (1 << 17) - 1;
const MASK_LOOSE: u32 = (1 << 15) - 1;
pub const COMPRESSION: &str = "br";

pub const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_FILES: usize = 20_000;
pub const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_COMPRESSED_CHUNK: u64 = MAX_CHUNK as u64 + 4096;

/// Потоков загрузки кусков.
pub const DOWNLOAD_CONCURRENCY: usize = 8;
/// Попыток на каждый адрес хранилища.
pub const DOWNLOAD_ATTEMPTS: usize = 3;
/// Простой соединения (`timeout` запроса).
pub const REQUEST_TIMEOUT: Duration = Duration::from_millis(30_000);
pub const MAX_REDIRECTS: u32 = 5;
pub const USER_AGENT: &str = "HLSR-Updater";

/// Пауза между событиями прогресса, пока загрузка не закончена.
const PROGRESS_INTERVAL_MS: i64 = 200;

// ------------------------------------------------------------------- ошибки

/// Ошибка чанков. Текст — как у `Error` в `chunks.js`: его показывает журнал; тексты внутренних
/// ошибок Node (сеть, fs, zlib, V8) не воспроизводятся.
#[derive(Debug, thiserror::Error)]
pub enum ChunkError {
    /// `AbortError` / `ABORTED`.
    #[error("Обновление отменено")]
    Aborted,
    #[error("Адрес вне allowlist: {0}")]
    NotAllowed(String),
    #[error("Слишком много перенаправлений")]
    TooManyRedirects,
    #[error("HTTP {status}: {url}")]
    Status { status: u16, url: String },
    #[error("Ответ больше {max} байт: {url}")]
    TooLarge { max: u64, url: String },
    #[error("Таймаут: {0}")]
    Timeout(String),
    /// Обрыв, отказ в соединении, ошибка TLS.
    #[error("{0}")]
    Network(String),
    #[error("Манифест чанков не совпал с подписанным каналом")]
    ManifestMismatch,
    /// `Манифест чанков: <первые пять причин через «; »>`.
    #[error("Манифест чанков: {0}")]
    ManifestInvalid(String),
    #[error("Манифест чанков недоступен")]
    ManifestUnavailable,
    /// Тело манифеста не JSON.
    #[error("{0}")]
    Json(String),
    #[error("Кусок {hash}: размер {got} != {expected}")]
    ChunkSize {
        hash: String,
        got: usize,
        expected: u64,
    },
    #[error("Кусок {0} повреждён")]
    ChunkCorrupt(String),
    #[error("Кусок {0} изменился на диске")]
    ChunkChanged(String),
    #[error("Кусок {0} после сжатия слишком велик")]
    PackedTooBig(String),
    #[error("Собранный файл не совпал: {0}")]
    FileMismatch(String),
    #[error("Файл {0} короче ожидаемого")]
    ShortFile(String),
    #[error("Ссылка в сборке: {0}")]
    Symlink(String),
    #[error("Неизвестный объект в сборке: {0}")]
    UnknownObject(String),
    /// brotli не распаковал кусок или вывод длиннее ожидаемого.
    #[error("{0}")]
    Decompress(String),
    #[error("Не заданы адреса хранилища кусков")]
    NoBases,
    #[error("{0}")]
    Io(#[from] io::Error),
}

impl ChunkError {
    /// Класс ошибки (`classify` скриптов сверки); текст сетевых ошибок и ошибок fs не сверяется.
    pub fn kind(&self) -> String {
        match self {
            ChunkError::Aborted => "aborted".to_owned(),
            ChunkError::NotAllowed(_) => "not-allowed".to_owned(),
            ChunkError::TooManyRedirects => "redirects".to_owned(),
            ChunkError::Status { status, .. } => format!("status:{status}"),
            ChunkError::TooLarge { .. } => "too-large".to_owned(),
            ChunkError::Timeout(_) => "timeout".to_owned(),
            ChunkError::Network(_) | ChunkError::NoBases => "network".to_owned(),
            ChunkError::ManifestMismatch => "manifest-mismatch".to_owned(),
            ChunkError::ManifestInvalid(_) => "manifest-invalid".to_owned(),
            ChunkError::ManifestUnavailable => "manifest-unavailable".to_owned(),
            ChunkError::Json(_) => "json".to_owned(),
            ChunkError::ChunkSize { .. } => "chunk-size".to_owned(),
            ChunkError::ChunkCorrupt(_) => "chunk-corrupt".to_owned(),
            ChunkError::ChunkChanged(_) => "chunk-changed".to_owned(),
            ChunkError::PackedTooBig(_) => "packed-too-big".to_owned(),
            ChunkError::FileMismatch(_) => "file-mismatch".to_owned(),
            ChunkError::ShortFile(_) => "short-file".to_owned(),
            ChunkError::Symlink(_) => "symlink".to_owned(),
            ChunkError::UnknownObject(_) => "unknown-object".to_owned(),
            ChunkError::Decompress(_) => "decode".to_owned(),
            ChunkError::Io(_) => "io".to_owned(),
        }
    }

    pub fn is_abort(&self) -> bool {
        matches!(self, ChunkError::Aborted)
    }
}

/// `throwIfAborted(signal)`.
fn throw_if_aborted(cancel: Option<&CancelToken>) -> Result<(), ChunkError> {
    if cancel.is_some_and(CancelToken::is_cancelled) {
        return Err(ChunkError::Aborted);
    }
    Ok(())
}

// ------------------------------------------------------------------- нарезка

/// Таблица gear: xorshift32 от `0x9e3779b9` (`GEAR`).
pub const GEAR: [u32; 256] = gear_table();

const fn gear_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut state: u32 = 0x9e37_79b9;
    let mut i = 0;
    while i < 256 {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        table[i] = state;
        i += 1;
    }
    table
}

/// Границы кусков буфера: `[начало, конец)`. Кусок — от 16 до 256 КиБ (кроме последнего), в
/// среднем 64 КиБ: до среднего размера граница ставится по строгой маске, дальше — по мягкой.
pub fn cut(buffer: &[u8]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let length = buffer.len();
    let mut start = 0;
    while start < length {
        let end = (start + MAX_CHUNK).min(length);
        let mut i = start + MIN_CHUNK;
        if i >= end {
            out.push(start..end);
            start = end;
            continue;
        }
        let mut boundary = end;
        let mut hash: u32 = 0;
        let middle = (start + AVG_CHUNK).min(end);
        while i < middle {
            hash = (hash << 1).wrapping_add(GEAR[usize::from(buffer[i])]);
            if hash & MASK_STRICT == 0 {
                boundary = i + 1;
                break;
            }
            i += 1;
        }
        if boundary == end {
            while i < end {
                hash = (hash << 1).wrapping_add(GEAR[usize::from(buffer[i])]);
                if hash & MASK_LOOSE == 0 {
                    boundary = i + 1;
                    break;
                }
                i += 1;
            }
        }
        out.push(start..boundary);
        start = boundary;
    }
    out
}

/// SHA-256 в шестнадцатеричной записи.
pub fn sha256(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// SHA-256 файла: читается кусками по 1 МиБ.
pub fn sha256_file(file: &Path) -> io::Result<String> {
    let mut hash = Sha256::new();
    let mut input = fs::File::open(file)?;
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hex(&hash.finalize()))
}

/// `chunkPath(hash)`: `<aa>/<sha256>` внутри хранилища.
pub fn chunk_path(hash: &str) -> String {
    format!("{}/{hash}", hash.get(..2).unwrap_or(hash))
}

/// `chunkUrl(base, hash)`: адрес куска; завершающие `/` базы отбрасываются.
pub fn chunk_url(base: &str, hash: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), chunk_path(hash))
}

// ------------------------------------------------------------------- сжатие

/// `compress(data)`: brotli, качество 9, подсказка размера — длина данных.
pub fn compress(data: &[u8]) -> Vec<u8> {
    let params = brotli::enc::BrotliEncoderParams {
        quality: 9,
        size_hint: data.len(),
        ..Default::default()
    };
    let mut out = Vec::new();
    brotli::BrotliCompress(&mut &data[..], &mut out, &params)
        .expect("запись в Vec не бывает неудачной");
    out
}

/// `decompress(data, expectedSize)`: вывод не длиннее `max(expectedSize, 1)` — иначе ошибка, как
/// `maxOutputLength` у zlib Node.
pub fn decompress(data: &[u8], expected_size: u64) -> Result<Vec<u8>, ChunkError> {
    let limit = expected_size.max(1);
    let mut out = Vec::new();
    brotli::Decompressor::new(data, 4096)
        .take(limit + 1)
        .read_to_end(&mut out)
        .map_err(|error| ChunkError::Decompress(error.to_string()))?;
    if out.len() as u64 > limit {
        return Err(ChunkError::Decompress(format!(
            "вывод brotli длиннее {limit} байт"
        )));
    }
    Ok(out)
}

// ------------------------------------------------------------------- манифест

/// Кусок в списке файла: `[sha256, размер, размер после сжатия]`; пока кусок не записан в
/// хранилище, размера после сжатия нет и в манифесте два числа.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkInfo {
    pub hash: String,
    pub size: u64,
    pub csize: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFile {
    /// Путь от корня сборки через `/`.
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// Бит исполнения (`exec: true`, только у файлов Linux).
    pub exec: bool,
    pub chunks: Vec<ChunkInfo>,
}

/// Манифест чанков версии (`HLSR-<версия>-<платформа>.chunks.json`).
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub version: String,
    pub platform: Platform,
    pub files: Vec<ManifestFile>,
    /// Ключ аттестации сборки (`{v, publicKey}`), его кладёт скрипт выпуска; клиент поле не
    /// проверяет, но сохраняет.
    pub attest: Option<Value>,
}

impl Manifest {
    /// Документ в порядке полей `describeTree`: `schema`, `kind`, `version`, `platform`,
    /// `chunking`, `compression`, `files`, потом `attest`; у файла `exec` — после `chunks`.
    pub fn to_value(&self) -> Value {
        let files: Vec<Value> = self
            .files
            .iter()
            .map(|file| {
                let chunks: Vec<Value> = file
                    .chunks
                    .iter()
                    .map(|chunk| match chunk.csize {
                        Some(csize) => json!([chunk.hash, chunk.size, csize]),
                        None => json!([chunk.hash, chunk.size]),
                    })
                    .collect();
                let mut entry = Map::new();
                entry.insert("path".to_owned(), json!(file.path));
                entry.insert("size".to_owned(), json!(file.size));
                entry.insert("sha256".to_owned(), json!(file.sha256));
                entry.insert("chunks".to_owned(), Value::Array(chunks));
                if file.exec {
                    entry.insert("exec".to_owned(), json!(true));
                }
                Value::Object(entry)
            })
            .collect();
        let mut manifest = Map::new();
        manifest.insert("schema".to_owned(), json!(SCHEMA));
        manifest.insert("kind".to_owned(), json!(KIND));
        manifest.insert("version".to_owned(), json!(self.version));
        manifest.insert("platform".to_owned(), json!(self.platform.as_str()));
        manifest.insert(
            "chunking".to_owned(),
            json!({
                "algorithm": ALGORITHM,
                "min": MIN_CHUNK,
                "avg": AVG_CHUNK,
                "max": MAX_CHUNK,
            }),
        );
        manifest.insert("compression".to_owned(), json!(COMPRESSION));
        manifest.insert("files".to_owned(), Value::Array(files));
        if let Some(attest) = &self.attest {
            manifest.insert("attest".to_owned(), attest.clone());
        }
        Value::Object(manifest)
    }

    /// Текст файла манифеста: `JSON.stringify(manifest) + "\n"` — одна строка
    /// (`scripts/lib/launcher-chunks.js`).
    pub fn to_json_line(&self) -> String {
        format!("{}\n", json_text::stringify(&self.to_value(), ""))
    }

    /// Манифест из документа, который уже прошёл [`problems_of`]; `None` — документ другого устройства.
    pub fn from_value(value: &Value) -> Option<Manifest> {
        let object = value.as_object()?;
        let platform = match object.get("platform")?.as_str()? {
            "linux" => Platform::Linux,
            "win32" => Platform::Win32,
            _ => return None,
        };
        let files = object
            .get("files")?
            .as_array()?
            .iter()
            .map(file_from_value)
            .collect::<Option<Vec<_>>>()?;
        Some(Manifest {
            version: object.get("version")?.as_str()?.to_owned(),
            platform,
            files,
            attest: object.get("attest").cloned(),
        })
    }
}

fn number_u64(value: &Value) -> Option<u64> {
    let number = value.as_number()?.as_f64()?;
    (number >= 0.0 && number.fract() == 0.0 && number <= 9_007_199_254_740_991.0)
        .then_some(number as u64)
}

fn file_from_value(value: &Value) -> Option<ManifestFile> {
    let object = value.as_object()?;
    let chunks = object
        .get("chunks")?
        .as_array()?
        .iter()
        .map(|chunk| {
            let parts = chunk.as_array()?;
            Some(ChunkInfo {
                hash: parts.first()?.as_str()?.to_owned(),
                size: number_u64(parts.get(1)?)?,
                csize: match parts.get(2) {
                    Some(csize) => Some(number_u64(csize)?),
                    None => None,
                },
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(ManifestFile {
        path: object.get("path")?.as_str()?.to_owned(),
        size: number_u64(object.get("size")?)?,
        sha256: object.get("sha256")?.as_str()?.to_owned(),
        exec: object.get("exec") == Some(&Value::Bool(true)),
        chunks,
    })
}

// ------------------------------------------------------------------- проверка манифеста и блока delta

/// Что клиент ждёт от манифеста (`expected` в `problemsOf`); пустая строка — как `undefined`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Expected<'a> {
    pub version: Option<&'a str>,
    pub platform: Option<&'a str>,
}

/// `safePath(value)`: относительный путь без `..`, `.`, `\`, `:`, NUL и завершающего `/`.
fn safe_path(value: Option<&Value>) -> bool {
    let Some(Value::String(text)) = value else {
        return false;
    };
    let units = text.encode_utf16().count();
    units > 0
        && units < 512
        && !text.starts_with('/')
        && !text.split('/').any(|part| part == "." || part == "..")
        && !text.contains(['\\', ':', '\0'])
        && !text.ends_with('/')
}

/// Число JSON как `f64`.
fn number_of(value: Option<&Value>) -> Option<f64> {
    value?.as_number()?.as_f64()
}

/// `Number.isSafeInteger(x)`.
fn is_safe_integer(value: Option<&Value>) -> bool {
    number_of(value).is_some_and(|x| x.fract() == 0.0 && x.abs() <= 9_007_199_254_740_991.0)
}

/// `problemsOf(manifest, expected)`: причины, по которым документ не манифест чанков (пусто —
/// манифест годится). Документ разбирается как JSON из сети: поля могут быть любого типа.
pub fn problems_of(manifest: &Value, expected: &Expected<'_>) -> Vec<String> {
    let Some(object) = manifest.as_object() else {
        return vec!["манифест чанков не объект".to_owned()];
    };
    let mut problems = Vec::new();
    let get = |key: &str| object.get(key);
    let template = |key: &str| js::to_template_string(get(key));

    if number_of(get("schema")) != Some(SCHEMA as f64)
        || get("kind").and_then(Value::as_str) != Some(KIND)
    {
        problems.push("неизвестная схема манифеста чанков".to_owned());
    }
    if !is_release_semver(&regex_input(get("version"))) {
        problems.push(format!("некорректная версия: {}", template("version")));
    }
    if let Some(version) = expected.version.filter(|version| !version.is_empty())
        && get("version").and_then(Value::as_str) != Some(version)
    {
        problems.push(format!(
            "версия {}, ожидалась {version}",
            template("version")
        ));
    }
    let platform = get("platform").and_then(Value::as_str);
    if !matches!(platform, Some("linux" | "win32")) {
        problems.push(format!("неизвестная платформа: {}", template("platform")));
    }
    if let Some(wanted) = expected.platform.filter(|platform| !platform.is_empty())
        && platform != Some(wanted)
    {
        problems.push(format!(
            "платформа {}, ожидалась {wanted}",
            template("platform")
        ));
    }
    let chunking = get("chunking").and_then(Value::as_object);
    let chunking_field = |key: &str| chunking.and_then(|chunking| chunking.get(key));
    if chunking_field("algorithm").and_then(Value::as_str) != Some(ALGORITHM)
        || number_of(chunking_field("min")) != Some(MIN_CHUNK as f64)
        || number_of(chunking_field("avg")) != Some(AVG_CHUNK as f64)
        || number_of(chunking_field("max")) != Some(MAX_CHUNK as f64)
    {
        problems.push("неподдерживаемые параметры нарезки".to_owned());
    }
    if get("compression").and_then(Value::as_str) != Some(COMPRESSION) {
        problems.push(format!(
            "неподдерживаемое сжатие: {}",
            template("compression")
        ));
    }
    let files = get("files")
        .and_then(Value::as_array)
        .filter(|files| !files.is_empty() && files.len() <= MAX_FILES);
    let Some(files) = files else {
        problems.push("некорректный список файлов".to_owned());
        return problems;
    };

    let mut seen = HashSet::new();
    for file in files {
        // `file && file.path`: у ложного значения «путь» — оно само.
        let name = if js::truthy(file) {
            file.as_object().and_then(|file| file.get("path"))
        } else {
            Some(file)
        };
        let label = js::to_template_string(name);
        let (true, Some(Value::String(path)), Some(file)) =
            (safe_path(name), name, file.as_object())
        else {
            problems.push(format!("недопустимый путь: {label}"));
            continue;
        };
        if !seen.insert(path.to_lowercase()) {
            problems.push(format!("повтор: {path}"));
        }
        let size = file.get("size");
        if !is_safe_integer(size)
            || number_of(size).is_some_and(|size| size < 0.0 || size > MAX_FILE_BYTES as f64)
        {
            problems.push(format!("размер {label}"));
        }
        if !is_sha256(&regex_input(file.get("sha256"))) {
            problems.push(format!("SHA-256 {label}"));
        }
        if file
            .get("exec")
            .is_some_and(|exec| exec != &Value::Bool(true))
        {
            problems.push(format!("exec {label}"));
        }
        let Some(chunks) = file.get("chunks").and_then(Value::as_array) else {
            problems.push(format!("куски {label}"));
            continue;
        };
        let mut total = 0.0;
        for chunk in chunks {
            let valid = chunk.as_array().is_some_and(|parts| {
                parts.len() == 3
                    && is_sha256(&regex_input(parts.first()))
                    && is_integer(parts.get(1))
                        .is_some_and(|size| size > 0.0 && size <= MAX_CHUNK as f64)
                    && is_integer(parts.get(2))
                        .is_some_and(|csize| csize > 0.0 && csize <= MAX_COMPRESSED_CHUNK as f64)
            });
            if !valid {
                problems.push(format!("некорректный кусок в {label}"));
                break;
            }
            total += number_of(chunk.get(1)).unwrap_or(0.0);
        }
        if number_of(size) != Some(total) {
            problems.push(format!("куски {label} не складываются в размер файла"));
        }
        if problems.len() > 20 {
            break;
        }
    }
    problems
}

/// `^HLSR-\d+\.\d+\.\d+-(linux|win32)\.chunks\.json$`
fn is_chunk_manifest_name(text: &str) -> bool {
    let Some(middle) = text
        .strip_prefix("HLSR-")
        .and_then(|rest| rest.strip_suffix(".chunks.json"))
    else {
        return false;
    };
    let Some((version, platform)) = middle.rsplit_once('-') else {
        return false;
    };
    matches!(platform, "linux" | "win32") && is_release_semver(version)
}

/// `deltaProblems(delta, isAllowedUrl)`: блок `delta` подписанного канала — адрес и хеш манифеста
/// чанков и базы хранилища. Старые клиенты блок не знают и не смотрят на него. Канал вызывает эту
/// функцию из `delta_of`.
pub fn delta_problems(delta: &Value, allowed: impl Fn(&str) -> bool) -> Vec<String> {
    if !delta.is_object() {
        return vec!["нет блока delta".to_owned()];
    }
    let mut problems = Vec::new();
    if is_integer(field(delta, "schema")) != Some(SCHEMA as f64) {
        problems.push(format!(
            "delta.schema {}",
            js::to_template_string(field(delta, "schema"))
        ));
    }
    let manifest = field(delta, "manifest").filter(|value| js::truthy(value));
    let manifest_field = |key: &str| manifest.and_then(|manifest| field(manifest, key));
    if !is_chunk_manifest_name(&regex_input(manifest_field("filename"))) {
        problems.push("delta.manifest.filename".to_owned());
    }
    if is_integer(manifest_field("size"))
        .is_none_or(|size| size <= 0.0 || size > MAX_MANIFEST_BYTES as f64)
    {
        problems.push("delta.manifest.size".to_owned());
    }
    if !is_sha256(&regex_input(manifest_field("sha256"))) {
        problems.push("delta.manifest.sha256".to_owned());
    }
    for (label, list) in [
        ("delta.manifest.urls", manifest_field("urls")),
        ("delta.chunkBaseUrls", field(delta, "chunkBaseUrls")),
    ] {
        match list.and_then(Value::as_array) {
            Some(list) if !list.is_empty() => {
                for url in list {
                    if !allowed(&js::to_string(url)) {
                        problems.push(format!(
                            "{label} вне allowlist: {}",
                            js::to_template_string(Some(url))
                        ));
                    }
                }
            }
            _ => problems.push(format!("{label} пуст")),
        }
    }
    problems
}

/// Блок `delta` подписанного канала, прошедший [`delta_problems`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delta {
    pub filename: String,
    /// Размер манифеста из канала: больше манифест не принимается.
    pub size: u64,
    pub sha256: String,
    pub manifest_urls: Vec<String>,
    pub chunk_base_urls: Vec<String>,
}

impl Delta {
    /// Блок канала → типизированный вид; причины отказа — как у [`delta_problems`].
    pub fn from_value(delta: &Value, allowed: impl Fn(&str) -> bool) -> Result<Delta, Vec<String>> {
        let problems = delta_problems(delta, allowed);
        if !problems.is_empty() {
            return Err(problems);
        }
        let manifest = field(delta, "manifest");
        let text = |value: Option<&Value>| value.map(js::to_string).unwrap_or_default();
        let list = |value: Option<&Value>| -> Vec<String> {
            value
                .and_then(Value::as_array)
                .map(|list| list.iter().map(js::to_string).collect())
                .unwrap_or_default()
        };
        let manifest_field = |key: &str| manifest.and_then(|manifest| field(manifest, key));
        Ok(Delta {
            filename: text(manifest_field("filename")),
            size: is_integer(manifest_field("size")).map_or(0, |size| size as u64),
            sha256: text(manifest_field("sha256")),
            manifest_urls: list(manifest_field("urls")),
            chunk_base_urls: list(field(delta, "chunkBaseUrls")),
        })
    }
}

// ------------------------------------------------------------------- дерево сборки и хранилище (выпуск)

/// Как узнать, что файл исполняемый, когда платформа манифеста — Linux (`stat.mode & 0o111`).
pub type ExecCheck<'a> = &'a dyn Fn(&str) -> bool;

/// Параметры [`describe_tree`] и [`build_store`].
#[derive(Clone, Copy)]
pub struct DescribeOptions<'a> {
    pub version: &'a str,
    pub platform: Platform,
    /// Пути (от корня через `/`), которые в манифест не попадают.
    pub exclude: &'a [&'a str],
    /// Подмена признака исполняемого файла по относительному пути; `None` — биты прав файла (Unix).
    pub executable: Option<ExecCheck<'a>>,
}

impl<'a> DescribeOptions<'a> {
    pub fn new(version: &'a str, platform: Platform) -> DescribeOptions<'a> {
        DescribeOptions {
            version,
            platform,
            exclude: &[],
            executable: None,
        }
    }
}

/// `listTree(root)`: файлы дерева, относительные пути через `/`; ссылка или не файл — ошибка.
fn list_tree(root: &Path, relative: &str, out: &mut Vec<String>) -> Result<(), ChunkError> {
    let dir = if relative.is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR))
    };
    for entry in js::fs::read_dir(&dir)? {
        let child = if relative.is_empty() {
            entry.name.clone()
        } else {
            format!("{relative}/{}", entry.name)
        };
        if entry.file_type.is_symlink() {
            return Err(ChunkError::Symlink(child));
        }
        if entry.file_type.is_dir() {
            list_tree(root, &child, out)?;
        } else if entry.file_type.is_file() {
            out.push(child);
        } else {
            return Err(ChunkError::UnknownObject(child));
        }
    }
    Ok(())
}

/// Бит исполнения файла по правам (`stat.mode & 0o111`); на Windows прав нет.
fn mode_is_executable(full: &Path) -> io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(fs::metadata(full)?.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        fs::metadata(full)?;
        Ok(false)
    }
}

/// Куски, о которых узнал [`describe_tree`]: вызывается один раз на каждый новый кусок.
pub type OnChunk<'a> = &'a mut dyn FnMut(&str, &[u8]) -> Result<(), ChunkError>;

/// `describeTree(root, options)`: манифест чанков каталога сборки (без размеров после сжатия).
/// `on_chunk(hash, data)` вызывается один раз на каждый новый кусок: выпуск пишет его в хранилище.
pub fn describe_tree(
    root: &Path,
    options: &DescribeOptions<'_>,
    on_chunk: OnChunk<'_>,
) -> Result<Manifest, ChunkError> {
    let exclude: HashSet<&str> = options.exclude.iter().copied().collect();
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    let mut names = Vec::new();
    list_tree(root, "", &mut names)?;
    names.sort_by(|a, b| js::cmp_utf16(a, b));
    for relative in names {
        if exclude.contains(relative.as_str()) {
            continue;
        }
        let full = root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        let data = js::fs::read_file(&full)?;
        let mut chunks = Vec::new();
        for range in cut(&data) {
            let piece = &data[range];
            let hash = sha256(piece);
            if seen.insert(hash.clone()) {
                on_chunk(&hash, piece)?;
            }
            chunks.push(ChunkInfo {
                hash,
                size: piece.len() as u64,
                csize: None,
            });
        }
        let exec = options.platform == Platform::Linux
            && match options.executable {
                Some(check) => check(&relative),
                None => mode_is_executable(&full)?,
            };
        files.push(ManifestFile {
            size: data.len() as u64,
            sha256: sha256(&data),
            path: relative,
            exec,
            chunks,
        });
    }
    Ok(Manifest {
        version: options.version.to_owned(),
        platform: options.platform,
        files,
        attest: None,
    })
}

/// Итог [`build_store`].
#[derive(Debug, Clone, PartialEq)]
pub struct BuildResult {
    pub manifest: Manifest,
    /// Записано новых кусков и их размер после сжатия.
    pub written: usize,
    pub written_bytes: u64,
    /// Кусков, которые уже лежали в хранилище.
    pub reused: usize,
    /// Всего разных кусков.
    pub chunks: usize,
}

/// `buildStore(root, store, options)`: нарезка сборки в хранилище — новые куски сжимаются и
/// пишутся в `store/<aa>/<sha256>`, уже лежащие там не трогаются.
pub fn build_store(
    root: &Path,
    store: &Path,
    options: &DescribeOptions<'_>,
) -> Result<BuildResult, ChunkError> {
    let (mut written, mut written_bytes, mut reused) = (0usize, 0u64, 0usize);
    let mut sizes: HashMap<String, u64> = HashMap::new();
    let mut manifest = describe_tree(root, options, &mut |hash, data| {
        let target = store.join(hash.get(..2).unwrap_or(hash)).join(hash);
        if target.exists() {
            reused += 1;
            sizes.insert(hash.to_owned(), fs::metadata(&target)?.len());
            return Ok(());
        }
        let packed = compress(data);
        if packed.len() as u64 > MAX_COMPRESSED_CHUNK {
            return Err(ChunkError::PackedTooBig(hash.to_owned()));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp = target.with_file_name(format!("{hash}.partial"));
        fs::write(&temp, &packed)?;
        fs::rename(&temp, &target)?;
        sizes.insert(hash.to_owned(), packed.len() as u64);
        written += 1;
        written_bytes += packed.len() as u64;
        Ok(())
    })?;
    for file in &mut manifest.files {
        for chunk in &mut file.chunks {
            chunk.csize = sizes.get(&chunk.hash).copied();
        }
    }
    Ok(BuildResult {
        manifest,
        written,
        written_bytes,
        reused,
        chunks: sizes.len(),
    })
}

// ------------------------------------------------------------------- сеть

/// Какие адреса разрешены (`isAllowedUrl`): в выпуске — [`is_allowed_url`] канала.
pub type AllowUrl = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Разрешает адреса из allowlist канала (`channel::is_allowed_url`).
pub fn production_allow() -> AllowUrl {
    Arc::new(is_allowed_url)
}

/// Параметры одного запроса (`options` `fetchBuffer`).
#[derive(Clone)]
pub struct FetchOptions {
    pub is_allowed: AllowUrl,
    /// Предел тела; больше — ошибка [`ChunkError::TooLarge`].
    pub max_bytes: u64,
    pub cancel: Option<CancelToken>,
}

/// `options.fetch` 0.4.7: тело ответа 200 по адресу. Тесты подставляют свою.
pub trait ChunkFetch: Send + Sync {
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        options: &'a FetchOptions,
    ) -> BoxFuture<'a, Result<Vec<u8>, ChunkError>>;
}

/// `fetchBuffer` на `reqwest`: GET с редиректами по allowlist (≤ 5, коды 3xx с `Location`),
/// `user-agent: HLSR-Updater`, `accept-encoding: identity`, простой соединения 30 с.
#[derive(Clone)]
pub struct HttpsFetch {
    client: reqwest::Client,
    idle: Duration,
}

impl HttpsFetch {
    pub fn new() -> HttpsFetch {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::ACCEPT_ENCODING,
            reqwest::header::HeaderValue::from_static("identity"),
        );
        HttpsFetch {
            client: hlsr_core::http::client_builder()
                .user_agent(USER_AGENT)
                .default_headers(headers)
                .build()
                .expect("клиент HTTP собирается из постоянных настроек"),
            idle: REQUEST_TIMEOUT,
        }
    }

    /// Другой простой соединения — для тестов (в 0.4.7 он постоянный, 30 с).
    pub fn with_idle_timeout(mut self, idle: Duration) -> HttpsFetch {
        self.idle = idle;
        self
    }

    async fn get(&self, url: &str, options: &FetchOptions) -> Result<Vec<u8>, ChunkError> {
        let mut current = url.to_owned();
        let mut redirects_left = MAX_REDIRECTS;
        loop {
            if !(options.is_allowed)(&current) {
                return Err(ChunkError::NotAllowed(current));
            }
            throw_if_aborted(options.cancel.as_ref())?;
            let target =
                Url::parse(&current).map_err(|_| ChunkError::Network("Invalid URL".to_owned()))?;
            let mut response =
                tokio::time::timeout(self.idle, self.client.get(target.clone()).send())
                    .await
                    .map_err(|_| ChunkError::Timeout(current.clone()))?
                    .map_err(|error| network_error(&error, &current))?;

            let status = response.status().as_u16();
            if (300..400).contains(&status)
                && let Some(location) = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .filter(|location| !location.is_empty())
                    .map(str::to_owned)
            {
                drop(response);
                if redirects_left == 0 {
                    return Err(ChunkError::TooManyRedirects);
                }
                let next = target
                    .join(&location)
                    .map_err(|_| ChunkError::Network("Invalid URL".to_owned()))?;
                current = next.to_string();
                redirects_left -= 1;
                continue;
            }
            if status != 200 {
                return Err(ChunkError::Status {
                    status,
                    url: current,
                });
            }

            let mut body = Vec::new();
            loop {
                let chunk = tokio::time::timeout(self.idle, response.chunk())
                    .await
                    .map_err(|_| ChunkError::Timeout(current.clone()))?
                    .map_err(|error| body_error(&error, &current))?;
                let Some(chunk) = chunk else {
                    return Ok(body);
                };
                if (body.len() + chunk.len()) as u64 > options.max_bytes {
                    return Err(ChunkError::TooLarge {
                        max: options.max_bytes,
                        url: current,
                    });
                }
                body.extend_from_slice(&chunk);
            }
        }
    }
}

impl Default for HttpsFetch {
    fn default() -> HttpsFetch {
        HttpsFetch::new()
    }
}

/// Ошибка запроса словами Node (`connect ECONNREFUSED …`, `getaddrinfo ENOTFOUND …`, `socket hang
/// up`) — так её видит журнал; тайм-аут соединения — `request.on("timeout")` 0.4.7.
fn network_error(error: &reqwest::Error, url: &str) -> ChunkError {
    if error.is_timeout() {
        return ChunkError::Timeout(url.to_owned());
    }
    ChunkError::Network(crate::http::node_net_error(error, url).message)
}

/// Обрыв посреди тела: `aborted`, как у ответа Node.
fn body_error(error: &reqwest::Error, url: &str) -> ChunkError {
    if error.is_timeout() {
        return ChunkError::Timeout(url.to_owned());
    }
    ChunkError::Network(crate::http::node_body_error().message)
}

impl ChunkFetch for HttpsFetch {
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        options: &'a FetchOptions,
    ) -> BoxFuture<'a, Result<Vec<u8>, ChunkError>> {
        Box::pin(async move {
            match &options.cancel {
                Some(cancel) => tokio::select! {
                    biased;
                    () = cancel.cancelled() => Err(ChunkError::Aborted),
                    result = self.get(url, options) => result,
                },
                None => self.get(url, options).await,
            }
        })
    }
}

/// Параметры [`fetch_manifest`].
pub struct FetchManifestOptions<'a> {
    pub fetch: &'a dyn ChunkFetch,
    pub is_allowed: AllowUrl,
    /// Версия и платформа, которые должен описывать манифест.
    pub expected: Expected<'a>,
    pub cancel: Option<CancelToken>,
}

/// `fetchManifest(delta, options)`: манифест чанков версии — скачать по адресам из канала, сверить
/// размер и SHA-256 с подписанным каналом и проверить устройство. Адреса пробуются по порядку;
/// если не подошёл ни один, ошибка последнего.
pub async fn fetch_manifest(
    delta: &Delta,
    options: &FetchManifestOptions<'_>,
) -> Result<Manifest, ChunkError> {
    let mut last_error = None;
    for url in &delta.manifest_urls {
        throw_if_aborted(options.cancel.as_ref())?;
        let fetch_options = FetchOptions {
            is_allowed: options.is_allowed.clone(),
            max_bytes: delta.size,
            cancel: options.cancel.clone(),
        };
        match fetch_manifest_from(delta, url, options, &fetch_options).await {
            Ok(manifest) => return Ok(manifest),
            Err(error) if error.is_abort() => return Err(error),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or(ChunkError::ManifestUnavailable))
}

async fn fetch_manifest_from(
    delta: &Delta,
    url: &str,
    options: &FetchManifestOptions<'_>,
    fetch_options: &FetchOptions,
) -> Result<Manifest, ChunkError> {
    let body = options.fetch.fetch(url, fetch_options).await?;
    if body.len() as u64 != delta.size || sha256(&body) != delta.sha256 {
        return Err(ChunkError::ManifestMismatch);
    }
    let document = json_text::json_parse(&String::from_utf8_lossy(&body))
        .map_err(|error| ChunkError::Json(error.to_string()))?;
    let problems = problems_of(&document, &options.expected);
    if !problems.is_empty() {
        let shown: Vec<&str> = problems.iter().take(5).map(String::as_str).collect();
        return Err(ChunkError::ManifestInvalid(shown.join("; ")));
    }
    Manifest::from_value(&document)
        .ok_or_else(|| ChunkError::ManifestInvalid("некорректный список файлов".to_owned()))
}

// ------------------------------------------------------------------- сборка версии

/// Кусок старого файла, пригодный для новой версии.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPiece {
    pub file: PathBuf,
    pub offset: usize,
    pub size: usize,
}

/// Что из старой версии годится для новой (`planReuse`): целые совпавшие файлы и куски изменённых
/// файлов с тем же путём.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReusePlan {
    /// Путь в манифесте → файл старой версии, совпавший целиком.
    pub whole: HashMap<String, PathBuf>,
    /// SHA-256 куска → где он лежит в старой версии.
    pub local: HashMap<String, LocalPiece>,
}

/// `planReuse(manifest, sourceRoot, options)`. Файл, которого в старой версии нет или который не
/// обычный, пропускается; ошибка чтения существующего файла прерывает сборку.
pub fn plan_reuse(
    manifest: &Manifest,
    source_root: Option<&Path>,
    cancel: Option<&CancelToken>,
) -> Result<ReusePlan, ChunkError> {
    let mut plan = ReusePlan::default();
    let Some(source_root) = source_root else {
        return Ok(plan);
    };
    for file in &manifest.files {
        throw_if_aborted(cancel)?;
        let from = source_root.join(file.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        let Ok(stat) = fs::metadata(&from) else {
            continue;
        };
        if !stat.is_file() {
            continue;
        }
        if stat.len() == file.size && sha256_file(&from)? == file.sha256 {
            plan.whole.insert(file.path.clone(), from);
            continue;
        }
        if file.chunks.is_empty() || stat.len() > MAX_FILE_BYTES {
            continue;
        }
        let data = js::fs::read_file(&from)?;
        for range in cut(&data) {
            let hash = sha256(&data[range.clone()]);
            plan.local.entry(hash).or_insert_with(|| LocalPiece {
                file: from.clone(),
                offset: range.start,
                size: range.len(),
            });
        }
    }
    Ok(plan)
}

/// `readRange(file, offset, size)`: ровно `size` байт с `offset`.
fn read_range(file: &Path, offset: usize, size: usize) -> Result<Vec<u8>, ChunkError> {
    use std::io::{Seek, SeekFrom};
    let mut input = fs::File::open(file)?;
    input.seek(SeekFrom::Start(offset as u64))?;
    let mut buffer = vec![0u8; size];
    let mut done = 0;
    while done < size {
        let read = input.read(&mut buffer[done..])?;
        if read == 0 {
            return Err(ChunkError::ShortFile(file.display().to_string()));
        }
        done += read;
    }
    Ok(buffer)
}

/// Прогресс загрузки: `{done, total, bytesPerSecond}` по сжатым размерам кусков.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
    pub bytes_per_second: u64,
}

/// Обратный вызов прогресса.
pub type OnProgress = Arc<dyn Fn(Progress) + Send + Sync>;

/// `progressMeter(total, onProgress)`: не чаще раза в 200 мс, последнее событие — всегда.
struct ProgressMeter {
    total: u64,
    started_ms: i64,
    clock: Arc<dyn Clock>,
    on_progress: OnProgress,
    state: Mutex<(u64, i64)>,
}

impl ProgressMeter {
    fn new(total: u64, clock: Arc<dyn Clock>, on_progress: OnProgress) -> ProgressMeter {
        ProgressMeter {
            total,
            started_ms: clock.now_ms(),
            clock,
            on_progress,
            state: Mutex::new((0, 0)),
        }
    }

    fn add(&self, bytes: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.0 += bytes;
        let now = self.clock.now_ms();
        if now - state.1 < PROGRESS_INTERVAL_MS && state.0 < self.total {
            return;
        }
        state.1 = now;
        let seconds = (((now - self.started_ms) as f64) / 1000.0).max(0.001);
        let rate = js::math_round(state.0 as f64 / seconds);
        // Событие уходит под замком: порядок событий разных потоков сохраняется.
        (self.on_progress)(Progress {
            done: state.0,
            total: self.total,
            bytes_per_second: rate as u64,
        });
    }
}

/// Параметры [`assemble`].
pub struct AssembleOptions {
    /// Манифест версии из [`fetch_manifest`] (или [`build_store`]): у каждого куска есть размер
    /// после сжатия — по нему задаётся предел ответа.
    pub manifest: Manifest,
    /// Установленная версия, откуда берётся совпавшее; `None` — собрать целиком из сети.
    pub source_root: Option<PathBuf>,
    /// Пустой каталог, куда собирается версия.
    pub target_dir: PathBuf,
    /// Куски по `sha256` без сжатия; переживает неудачную сборку.
    pub cache_dir: PathBuf,
    /// Базы хранилища кусков из блока `delta` (`chunkBaseUrls`).
    pub chunk_base_urls: Vec<String>,
    pub is_allowed: AllowUrl,
    pub fetch: Arc<dyn ChunkFetch>,
    pub cancel: Option<CancelToken>,
    pub on_progress: Option<OnProgress>,
    pub log: Option<Arc<dyn Logger>>,
    /// Часы прогресса (по умолчанию — системные).
    pub clock: Arc<dyn Clock>,
}

impl AssembleOptions {
    pub fn new(
        manifest: Manifest,
        target_dir: PathBuf,
        cache_dir: PathBuf,
        chunk_base_urls: Vec<String>,
        fetch: Arc<dyn ChunkFetch>,
        is_allowed: AllowUrl,
    ) -> AssembleOptions {
        AssembleOptions {
            manifest,
            source_root: None,
            target_dir,
            cache_dir,
            chunk_base_urls,
            is_allowed,
            fetch,
            cancel: None,
            on_progress: None,
            log: None,
            clock: Arc::new(SystemClock),
        }
    }
}

/// Итог [`assemble`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assembled {
    /// Сжатый размер всех кусков, которые пришлось получить (скачать или взять из кеша).
    pub downloaded_bytes: u64,
    /// Байты, взятые из установленной версии.
    pub reused_bytes: u64,
    pub downloaded_chunks: usize,
}

#[derive(Debug, Clone)]
struct Needed {
    hash: String,
    size: u64,
    csize: u64,
}

struct Downloads {
    bases: Vec<String>,
    cache_dir: PathBuf,
    cancel: Option<CancelToken>,
    is_allowed: AllowUrl,
    fetch: Arc<dyn ChunkFetch>,
    meter: ProgressMeter,
    queue: Mutex<VecDeque<Needed>>,
    failure: Mutex<Option<ChunkError>>,
}

impl Downloads {
    fn failed(&self) -> bool {
        self.failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    fn fail(&self, error: ChunkError) {
        self.failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get_or_insert(error);
    }

    fn next(&self) -> Option<Needed> {
        if self.failed() {
            return None;
        }
        self.queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
    }

    /// Кусок уже лежит в кеше целым.
    async fn cached(&self, chunk: &Needed, target: &Path) -> bool {
        let Ok(stat) = tokio::fs::metadata(target).await else {
            return false;
        };
        if stat.len() != chunk.size {
            return false;
        }
        tokio::fs::read(target)
            .await
            .is_ok_and(|data| sha256(&data) == chunk.hash)
    }

    async fn one(&self, chunk: &Needed) -> Result<(), ChunkError> {
        let target = self.cache_dir.join(&chunk.hash);
        if self.cached(chunk, &target).await {
            self.meter.add(chunk.csize);
            return Ok(());
        }
        let mut last_error = None;
        for attempt in 0..DOWNLOAD_ATTEMPTS * self.bases.len() {
            throw_if_aborted(self.cancel.as_ref())?;
            let base = &self.bases[attempt % self.bases.len()];
            match self.download(chunk, base, &target).await {
                Ok(()) => {
                    self.meter.add(chunk.csize);
                    return Ok(());
                }
                Err(error) => {
                    if error.is_abort()
                        || self.cancel.as_ref().is_some_and(CancelToken::is_cancelled)
                    {
                        return Err(ChunkError::Aborted);
                    }
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or(ChunkError::NoBases))
    }

    async fn download(&self, chunk: &Needed, base: &str, target: &Path) -> Result<(), ChunkError> {
        let options = FetchOptions {
            is_allowed: self.is_allowed.clone(),
            max_bytes: chunk.csize,
            cancel: self.cancel.clone(),
        };
        let packed = self
            .fetch
            .fetch(&chunk_url(base, &chunk.hash), &options)
            .await?;
        if packed.len() as u64 != chunk.csize {
            return Err(ChunkError::ChunkSize {
                hash: chunk.hash.clone(),
                got: packed.len(),
                expected: chunk.csize,
            });
        }
        let data = decompress(&packed, chunk.size)?;
        if data.len() as u64 != chunk.size || sha256(&data) != chunk.hash {
            return Err(ChunkError::ChunkCorrupt(chunk.hash.clone()));
        }
        let partial = target.with_file_name(format!("{}.partial", chunk.hash));
        tokio::fs::write(&partial, &data).await?;
        tokio::fs::rename(&partial, target).await?;
        Ok(())
    }

    /// Поток загрузки: берёт куски из очереди, пока она не пуста и нет сбоя. `first` — кусок,
    /// взятый потоком при запуске.
    async fn worker(self: Arc<Self>, first: Option<Needed>) {
        let mut next = first;
        while let Some(chunk) = next.take().or_else(|| self.next()) {
            if let Err(error) = self.one(&chunk).await {
                self.fail(error);
            }
        }
    }
}

/// `downloadChunks(needed, options)`: до восьми потоков; по три попытки на каждую базу; первая
/// ошибка останавливает выдачу новых кусков. Возвращает сжатый размер всех кусков.
async fn download_chunks(
    needed: Vec<Needed>,
    downloads: Arc<Downloads>,
) -> Result<u64, ChunkError> {
    let total: u64 = needed.iter().map(|chunk| chunk.csize).sum();
    downloads.meter.add(0);
    let workers = DOWNLOAD_CONCURRENCY.min(needed.len().max(1));
    let mut set = JoinSet::new();
    for _ in 0..workers {
        // Как в 0.4.7: все потоки сразу берут по куску, прежде чем кто-то успеет упасть.
        let first = downloads
            .queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front();
        set.spawn(downloads.clone().worker(first));
    }
    while let Some(joined) = set.join_next().await {
        // Паника потока — не ошибка сети: пробрасывается как есть.
        if let Err(error) = joined
            && let Ok(panic) = error.try_into_panic()
        {
            std::panic::resume_unwind(panic);
        }
    }
    let failure = downloads
        .failure
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    match failure {
        Some(error) => Err(error),
        None => Ok(total),
    }
}

/// `copyWhole(from, to)`: файл копируется (`COPYFILE_EXCL`), существующий не затирается. Как в
/// libuv, источник открывается раньше цели — нет источника, нет и пустого файла; недокопированная
/// цель убирается, иначе сборка осталась бы с обрезанным файлом, который следующая попытка
/// принимала бы за существующий.
fn copy_whole(from: &Path, to: &Path) -> io::Result<()> {
    let mut source = fs::File::open(from)?;
    let mut target = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)?;
    let copied = io::copy(&mut source, &mut target)
        .and_then(|_| source.metadata())
        .and_then(|metadata| target.set_permissions(metadata.permissions()));
    if copied.is_err() {
        drop(target);
        let _ = fs::remove_file(to);
    }
    copied
}

/// Создать файл для сборки: `wx`, права 0644.
fn create_new_file(target: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o644);
    }
    options.open(target)
}

/// Бит исполнения файла после сборки (`fs.chmodSync(target, 0o755)`); на Windows ничего не меняет.
fn make_executable(target: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(target, fs::Permissions::from_mode(0o755))
    }
    #[cfg(not(unix))]
    {
        let _ = target;
        Ok(())
    }
}

/// Запись файлов версии из кусков (после загрузки): совпавшие целиком копируются, остальные
/// складываются из кусков — с диска старой версии или из кеша.
fn write_files(
    manifest: &Manifest,
    reuse: &ReusePlan,
    target_dir: &Path,
    cache_dir: &Path,
    cancel: Option<&CancelToken>,
) -> Result<u64, ChunkError> {
    let mut reused_bytes = 0u64;
    for file in &manifest.files {
        throw_if_aborted(cancel)?;
        let target = target_dir.join(file.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Some(whole) = reuse.whole.get(&file.path) {
            copy_whole(whole, &target)?;
            reused_bytes += file.size;
        } else {
            let mut hash = Sha256::new();
            let mut output = create_new_file(&target)?;
            for chunk in &file.chunks {
                let source = reuse.local.get(&chunk.hash);
                let data = match source {
                    Some(source) => read_range(&source.file, source.offset, source.size)?,
                    None => js::fs::read_file(&cache_dir.join(&chunk.hash))?,
                };
                if data.len() as u64 != chunk.size || sha256(&data) != chunk.hash {
                    return Err(ChunkError::ChunkChanged(chunk.hash.clone()));
                }
                if source.is_some() {
                    reused_bytes += chunk.size;
                }
                hash.update(&data);
                output.write_all(&data)?;
            }
            drop(output);
            if hex(&hash.finalize()) != file.sha256 {
                return Err(ChunkError::FileMismatch(file.path.clone()));
            }
        }
        if file.exec {
            make_executable(&target)?;
        }
    }
    Ok(reused_bytes)
}

/// `assemble(options)`: собирает версию из манифеста в пустой `target_dir`. Из установленной
/// версии берётся всё совпавшее, недостающие куски скачиваются (8 потоков), каждый кусок и каждый
/// файл сверяются по SHA-256.
pub async fn assemble(options: AssembleOptions) -> Result<Assembled, ChunkError> {
    let AssembleOptions {
        manifest,
        source_root,
        target_dir,
        cache_dir,
        chunk_base_urls,
        is_allowed,
        fetch,
        cancel,
        on_progress,
        log,
        clock,
    } = options;
    let manifest = Arc::new(manifest);

    let reuse = {
        let (manifest, cancel) = (manifest.clone(), cancel.clone());
        tokio::task::spawn_blocking(move || {
            plan_reuse(&manifest, source_root.as_deref(), cancel.as_ref())
        })
        .await
        .map_err(join_error)??
    };
    let mut needed: Vec<Needed> = Vec::new();
    let mut seen = HashSet::new();
    for file in &manifest.files {
        if reuse.whole.contains_key(&file.path) {
            continue;
        }
        for chunk in &file.chunks {
            if !reuse.local.contains_key(&chunk.hash) && seen.insert(chunk.hash.clone()) {
                needed.push(Needed {
                    hash: chunk.hash.clone(),
                    size: chunk.size,
                    csize: chunk.csize.unwrap_or(0),
                });
            }
        }
    }
    if let Some(log) = &log {
        log.info(&format!(
            "[chunks] {}: файлов целиком {}, кусков с диска {}, скачать {}",
            manifest.version,
            reuse.whole.len(),
            reuse.local.len(),
            needed.len()
        ));
    }

    tokio::fs::create_dir_all(&cache_dir).await?;
    let total: u64 = needed.iter().map(|chunk| chunk.csize).sum();
    let downloaded_chunks = needed.len();
    let downloads = Arc::new(Downloads {
        bases: chunk_base_urls,
        cache_dir: cache_dir.clone(),
        cancel: cancel.clone(),
        is_allowed,
        fetch,
        meter: ProgressMeter::new(
            total,
            clock,
            on_progress.unwrap_or_else(|| Arc::new(|_| {})),
        ),
        queue: Mutex::new(needed.iter().cloned().collect()),
        failure: Mutex::new(None),
    });
    let downloaded_bytes = download_chunks(needed, downloads).await?;

    let reused_bytes = {
        let manifest = manifest.clone();
        tokio::task::spawn_blocking(move || {
            write_files(&manifest, &reuse, &target_dir, &cache_dir, cancel.as_ref())
        })
        .await
        .map_err(join_error)??
    };
    Ok(Assembled {
        downloaded_bytes,
        reused_bytes,
        downloaded_chunks,
    })
}

fn join_error(error: tokio::task::JoinError) -> ChunkError {
    match error.try_into_panic() {
        Ok(panic) => std::panic::resume_unwind(panic),
        Err(error) => ChunkError::Io(io::Error::other(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::copy_whole;
    use std::fs;
    use std::io::ErrorKind;

    #[test]
    fn copy_whole_copies_the_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("from"), dir.path().join("to"));
        fs::write(&from, b"content\0\xff").unwrap();
        copy_whole(&from, &to).unwrap();
        assert_eq!(fs::read(&to).unwrap(), b"content\0\xff");
        fs::write(&from, b"").unwrap();
        copy_whole(&from, &dir.path().join("empty-copy")).unwrap();
        assert_eq!(fs::read(dir.path().join("empty-copy")).unwrap(), b"");
    }

    /// Нет источника: ошибка и никакого пустого файла на месте цели (libuv открывает источник первым).
    #[test]
    fn copy_whole_leaves_no_empty_file_without_a_source() {
        let dir = tempfile::tempdir().unwrap();
        let to = dir.path().join("to");
        let error = copy_whole(&dir.path().join("missing"), &to).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::NotFound);
        assert!(!to.exists(), "пустой файл остался");
    }

    /// Источник — каталог: в Unix он открывается, а чтение падает уже после создания цели — цель
    /// убирается (в Windows отказ приходит при открытии, цель не создаётся).
    #[test]
    fn copy_whole_removes_a_half_made_target() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("from"), dir.path().join("to"));
        fs::create_dir(&from).unwrap();
        assert!(copy_whole(&from, &to).is_err());
        assert!(!to.exists(), "недокопированный файл остался");
    }

    /// Существующий файл не затирается: `EEXIST`, содержимое прежнее.
    #[test]
    fn copy_whole_does_not_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("from"), dir.path().join("to"));
        fs::write(&from, b"new").unwrap();
        fs::write(&to, b"old").unwrap();
        let error = copy_whole(&from, &to).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&to).unwrap(), b"old");
    }
}
