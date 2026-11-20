//! Экспорт и импорт сборки (`build-transfer.js`): настройки игры из лаунчера и cfg игрока в
//! одном файле `.hlsrbuild` (JSON с отступом в один пробел). Файлы сборки не переносятся: при
//! импорте лаунчер ставит или обновляет сборку сам, а cfg игрока переживают обновление
//! (`user_configs`).

use crate::atomic::write_file_atomic;
use crate::errors::RawError;
use crate::file_ops::hex;
use crate::paths::{assert_inside, assert_real_inside};
use crate::user_configs::{is_config_path, is_user_edited};
use crate::{cfg_files, js_value};
use base64::Engine as _;
use hlsr_core::js::fs::read_file;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, buffer_from_base64};
use hlsr_core::json_text::{json_parse, stringify};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Значение поля `format`.
pub const FORMAT: &str = "hlsr-build";
/// Версия формата, которую читает и пишет этот лаунчер.
pub const FORMAT_VERSION: u64 = 1;
/// Расширение файла.
pub const EXTENSION: &str = "hlsrbuild";
const MAX_FILES: usize = 400;
const MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 24 * 1024 * 1024;

/// Код отказа (`TransferError.code`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferCode {
    /// Слишком много или слишком большие файлы.
    TooLarge,
    /// Не файл сборки HLSR.
    BadFormat,
    /// Файл сделан более новым лаунчером.
    NewerFormat,
    /// Игры нет в реестре манифестов.
    UnknownGame,
    /// Контрольная сумма файла не сошлась.
    Corrupt,
    /// Путь выходит за папку игры.
    PathInvalid,
    /// cfg не прочитан: его держит другая программа или нет прав (`file` — какой).
    FileLocked,
}

impl TransferCode {
    pub fn as_str(self) -> &'static str {
        match self {
            TransferCode::TooLarge => "TOO_LARGE",
            TransferCode::BadFormat => "BAD_FORMAT",
            TransferCode::NewerFormat => "NEWER_FORMAT",
            TransferCode::UnknownGame => "UNKNOWN_GAME",
            TransferCode::Corrupt => "CORRUPT",
            TransferCode::PathInvalid => "PATH_INVALID",
            TransferCode::FileLocked => "FILE_LOCKED",
        }
    }
}

/// `TransferError`: отказ с кодом; текст — сам код, у `FILE_LOCKED` — «путь: причина».
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", .message.as_deref().unwrap_or(.code.as_str()))]
pub struct TransferError {
    pub code: TransferCode,
    /// `error.file`: cfg, который не прочитан (путь через `/`).
    pub file: Option<String>,
    pub message: Option<String>,
}

/// Чем кончился вызов, если не удался.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TransferFailure {
    /// Отказ по правилам переноса.
    #[error(transparent)]
    Transfer(#[from] TransferError),
    /// Ошибка файловой системы (в 0.4.7 исключение Node, фасад отвечает `IO_FAILED`).
    #[error(transparent)]
    Raw(#[from] RawError),
    /// Исключение JS на неожиданной форме документа (`TypeError`): фасад отвечает `IO_FAILED`.
    #[error("{0}")]
    Js(String),
}

fn refuse(code: TransferCode) -> TransferFailure {
    TransferError {
        code,
        file: None,
        message: None,
    }
    .into()
}

/// cfg игрока из файла сборки: путь от папки игры через `/` и байты.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickedFile {
    pub path: String,
    pub data: Vec<u8>,
}

fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// cfg игрока: изменённые или свои, которых нет в сборке (`collect`). Без `files.json`
/// установленной версии (сборка из Steam) переносятся все cfg. Порядок — порядок списка
/// редактора (язык по умолчанию процесса, см. [`cfg_files::default_locale`]).
pub fn collect(
    install_path: &Path,
    previous_files: Option<&Value>,
) -> Result<Vec<PickedFile>, TransferFailure> {
    collect_in(install_path, previous_files, &cfg_files::default_locale())
}

/// [`collect`] с явным языком порядка списка.
pub fn collect_in(
    install_path: &Path,
    previous_files: Option<&Value>,
    locale: &str,
) -> Result<Vec<PickedFile>, TransferFailure> {
    let listing = cfg_files::list_in(install_path, locale);
    let known = previous_files
        .and_then(|previous| js_value::prop(previous, "files"))
        .is_some_and(Value::is_array);
    let path = NodePath::native();
    let mut picked: Vec<PickedFile> = Vec::new();
    let mut total = 0usize;
    for file in listing.files {
        if known && !is_user_edited(install_path, &file.path, previous_files, None) {
            continue;
        }
        let full = PathBuf::from(path.join(&[
            &install_path.to_string_lossy(),
            &file.path.split('/').collect::<Vec<_>>().join(path.sep()),
        ]));
        // Файл держит другая программа или нет прав: окно назовёт его.
        let data = read_file(&full).map_err(|e| {
            let raw = RawError::fs(&e, "open", &full);
            TransferError {
                code: TransferCode::FileLocked,
                message: Some(format!(
                    "{}: {}",
                    file.path,
                    raw.code.unwrap_or(raw.message)
                )),
                file: Some(file.path.clone()),
            }
        })?;
        total += data.len();
        // Число файлов в списке редактора не больше 400, так что первое условие недостижимо;
        // оно остаётся как в 0.4.7.
        if picked.len() >= MAX_FILES || total > MAX_TOTAL_BYTES {
            return Err(refuse(TransferCode::TooLarge));
        }
        picked.push(PickedFile {
            path: file.path,
            data,
        });
    }
    Ok(picked)
}

/// Что записывается в файл сборки.
#[derive(Debug, Clone)]
pub struct Build<'a> {
    pub launcher_id: &'a str,
    pub game_id: &'a str,
    /// Пустая строка — как `null`.
    pub build: Option<&'a str>,
    pub version: Option<&'a str>,
    pub launcher_version: Option<&'a str>,
    /// Настройки игры из лаунчера; ложное значение — `null`.
    pub settings: Option<&'a Value>,
    pub files: &'a [PickedFile],
    /// Время экспорта; не задано — сейчас.
    pub exported_at: Option<SystemTime>,
}

/// Текст файла `.hlsrbuild` (`serialize`).
pub fn serialize(build: &Build) -> String {
    serialize_at(build, SystemTime::now())
}

/// [`serialize`] при заданных часах: `now` идёт в `exportedAt`, когда время экспорта не задано.
pub fn serialize_at(build: &Build, now: SystemTime) -> String {
    let text = |value: Option<&str>| match value.filter(|text| !text.is_empty()) {
        Some(text) => json!(text),
        None => Value::Null,
    };
    let files: Vec<Value> = build
        .files
        .iter()
        .map(|file| {
            json!({
                "path": file.path,
                "sha256": sha256_hex(&file.data),
                "data": base64::engine::general_purpose::STANDARD.encode(&file.data),
            })
        })
        .collect();
    let settings = build
        .settings
        .filter(|settings| js::truthy(settings))
        .cloned()
        .unwrap_or(Value::Null);
    let document = json!({
        "format": FORMAT,
        "version": FORMAT_VERSION,
        "exportedAt": js::iso_string(build.exported_at.unwrap_or(now)),
        "launcherVersion": text(build.launcher_version),
        "game": {
            "launcherId": build.launcher_id,
            "gameId": build.game_id,
            "build": text(build.build),
            "version": text(build.version),
        },
        "settings": settings,
        "files": files,
    });
    stringify(&document, " ")
}

/// `validPath`: cfg внутри папки игры, без `..`, дисков, запрещённых в Windows знаков и
/// управляющих символов, не длиннее 260 единиц UTF-16.
fn valid_path(text: &str) -> bool {
    let drive = {
        let bytes = text.as_bytes();
        bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
    };
    text.encode_utf16().count() <= 260
        && is_config_path(text)
        && !text.contains('\\')
        // `path.isAbsolute`: без обратных косых это только начало с «/».
        && !text.starts_with('/')
        && !drive
        && text.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.chars().any(|c| "<>:\"|?*".contains(c))
                && !part.chars().any(|c| u32::from(c) < 32)
        })
}

/// `/^[a-z0-9][a-z0-9-]{0,63}$/i`.
fn is_game_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
}

/// Разобранный файл сборки.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub game_id: String,
    pub launcher_id: Option<String>,
    pub build: Option<String>,
    pub version: Option<String>,
    pub exported_at: Option<String>,
    /// Настройки игры; `None` — нет (или не объект).
    pub settings: Option<Value>,
    pub files: Vec<PickedFile>,
}

/// Разбор файла сборки (`parse`). `is_known_game` — проверка идентификатора игры по реестру
/// манифестов лаунчера; без неё годится любой верный идентификатор.
pub fn parse(
    text: &str,
    is_known_game: Option<&dyn Fn(&str) -> bool>,
) -> Result<Parsed, TransferFailure> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(refuse(TransferCode::TooLarge));
    }
    let raw = json_parse(text).map_err(|_| refuse(TransferCode::BadFormat))?;
    let prop = |value: &Value, name: &str| js_value::prop(value, name).cloned();
    if !js::truthy(&raw) || js_value::prop(&raw, "format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(refuse(TransferCode::BadFormat));
    }
    let Some(version) = js_value::prop(&raw, "version").and_then(js_value::number) else {
        return Err(refuse(TransferCode::BadFormat));
    };
    if version > FORMAT_VERSION as f64 {
        return Err(refuse(TransferCode::NewerFormat));
    }
    let game = prop(&raw, "game")
        .filter(js::truthy)
        .unwrap_or_else(|| Value::Object(Map::new()));
    let game_id = match prop(&game, "gameId").filter(js::truthy) {
        Some(id) => js::to_string(&id),
        None => String::new(),
    };
    if !is_game_id(&game_id) || is_known_game.is_some_and(|known| !known(&game_id)) {
        return Err(refuse(TransferCode::UnknownGame));
    }
    let Some(list) = js_value::prop(&raw, "files").and_then(Value::as_array) else {
        return Err(refuse(TransferCode::BadFormat));
    };
    if list.len() > MAX_FILES {
        return Err(refuse(TransferCode::BadFormat));
    }
    let mut files: Vec<PickedFile> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut total = 0usize;
    for file in list {
        if !js::truthy(file) {
            return Err(refuse(TransferCode::BadFormat));
        }
        let path_value = js_value::prop(file, "path");
        let path_text = match path_value.filter(|v| js::truthy(v)) {
            Some(value) => js::to_string(value),
            None => String::new(),
        };
        let data = js_value::prop(file, "data").and_then(Value::as_str);
        let (true, Some(data)) = (valid_path(&path_text), data) else {
            return Err(refuse(TransferCode::BadFormat));
        };
        // Путь, который `validPath` пропустил как строку, но сам не строка (`["a.cfg"]`):
        // в 0.4.7 дальше падает `file.path.toLowerCase` — TypeError.
        let Some(path) = path_value.and_then(Value::as_str) else {
            return Err(TransferFailure::Js(
                "TypeError: file.path.toLowerCase is not a function".to_owned(),
            ));
        };
        if !seen.insert(path.to_lowercase()) {
            return Err(refuse(TransferCode::BadFormat));
        }
        let data = buffer_from_base64(data);
        if let Some(expected) = js_value::prop(file, "sha256").filter(|v| js::truthy(v))
            && expected.as_str() != Some(sha256_hex(&data).as_str())
        {
            return Err(refuse(TransferCode::Corrupt));
        }
        total += data.len();
        if data.len() as u64 > cfg_files::MAX_FILE_BYTES || total > MAX_TOTAL_BYTES {
            return Err(refuse(TransferCode::TooLarge));
        }
        files.push(PickedFile {
            path: path.to_owned(),
            data,
        });
    }
    let string_of = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_owned);
    Ok(Parsed {
        game_id,
        launcher_id: string_of(js_value::prop(&game, "launcherId")),
        build: string_of(js_value::prop(&game, "build")),
        version: string_of(js_value::prop(&game, "version")),
        exported_at: string_of(js_value::prop(&raw, "exportedAt")),
        settings: js_value::prop(&raw, "settings")
            .filter(|settings| settings.is_object())
            .cloned(),
        files,
    })
}

/// Запись не должна уйти через ссылку за пределы папки игры (`paths::assert_real_inside`).
fn assert_writable(install_path: &Path, full: &Path) -> Result<(), TransferFailure> {
    assert_real_inside(install_path, full, Some("Конфиг"))
        .map_err(|_| refuse(TransferCode::PathInvalid))
}

/// Запись cfg из файла сборки в папку игры (`applyFiles`): сначала проверяются все пути (ни
/// один файл не пишется, если хоть один выходит за папку игры), потом файлы пишутся по одному
/// и атомарно — сбой записи оставляет уже записанные. Возвращает пути в порядке записи.
pub fn apply_files(
    install_path: &Path,
    files: &[PickedFile],
) -> Result<Vec<String>, TransferFailure> {
    let path = NodePath::native();
    let mut fulls: Vec<PathBuf> = Vec::new();
    for file in files {
        let joined = path.join(&[
            &install_path.to_string_lossy(),
            &file.path.split('/').collect::<Vec<_>>().join(path.sep()),
        ]);
        let full = assert_inside(install_path, Path::new(&joined), Some("Конфиг"))
            .map_err(|_| refuse(TransferCode::PathInvalid))?;
        assert_writable(install_path, &full)?;
        fulls.push(full);
    }
    for (file, full) in files.iter().zip(&fulls) {
        write_file_atomic(full, &file.data)?;
    }
    Ok(files.iter().map(|file| file.path.clone()).collect())
}
