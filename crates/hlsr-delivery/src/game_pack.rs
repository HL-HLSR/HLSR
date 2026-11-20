//! Полная сборка в одном архиве `.hlsrpack` (7z, `game-pack.js`): папка игры целиком — файлы
//! сборки, сейвы, cfg, демо — и служебный `.hlsr-pack.json` в корне с версией и настройками
//! лаунчера. Архив переносят на флешку и ставят без загрузки.
//!
//! Архиву с флешки доверять нельзя: при импорте файлы сборки сверяются с подписанным списком
//! версии, изменённые заменяются оригиналами с сервера (контент откладывается, [`set_aside`]),
//! посторонние файлы отбрасываются, а исполняемые файлы, не подтверждённые списком, удаляются и
//! там, где проверка их не трогает (cfg, сейвы, Bunnymod XT).

use crate::errors::{Code, DeliveryError, Failure, RawError};
use crate::js_value as jv;
use crate::mutable_paths::ENGINE_CACHES;
use crate::path_match::Matcher;
use hlsr_core::js;
use hlsr_core::js::path::NodePath;
use hlsr_core::json_text::{js_number, json_parse};
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub const FORMAT: &str = "hlsr-pack";
pub const FORMAT_VERSION: u32 = 1;
pub const EXTENSION: &str = "hlsrpack";
pub const META_FILE: &str = ".hlsr-pack.json";
pub const MAX_META_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 300_000;
pub const MAX_UNCOMPRESSED_BYTES: u64 = 64 * 1024 * 1024 * 1024;

/// В архив не идут запись об установке (при импорте пишется заново), резервные копии лаунчера
/// (в том числе прежние конфиги игрока из импорта и перехода — другим игрокам они ни к чему) и
/// кэши движка, которые игра пересобирает сама.
static SKIPPED: LazyLock<Matcher> = LazyLock::new(|| {
    let mut patterns = vec![
        "installed.json",
        "installed.json.*",
        "*.hlsr-backup",
        "**/.hlsr-update-*",
        ".hlsr/backup",
        ".hlsr/cfg-import-backup",
        IMPORTED_BACKUP,
        META_FILE,
    ];
    patterns.extend_from_slice(ENGINE_CACHES);
    Matcher::new(&patterns)
});

/// Изменённые файлы сборки из архива ([`set_aside`]) у игр без слоя custom и вне его папок —
/// копией здесь, внутри папки игры: проверка файлов её не судит, в следующий экспорт она не едет.
pub const IMPORTED_BACKUP: &str = ".hlsr/imported-modified";

/// Слой custom Ghosting: контент из папок, которые игра ищет через `custom/*`, ложится в
/// `ghostingmod/custom/imported` — игра читает custom первым и выглядит как раньше.
/// (игра, папка слоя, корни через `/` в нижнем регистре).
const CUSTOM_LAYERS: [(&str, &str, [&str; 2]); 1] = [(
    "ghosting-renewed",
    "ghostingmod/custom/imported",
    ["ghostingmod/", "hl2/"],
)];

/// Изменённые программы и библиотеки не принимаются никогда, даже если заголовок испорчен: по
/// расширению тоже (Miles и движок грузят и `.asi`, `.flt`, `.mix`, `.m3d`).
const BINARY_EXTENSIONS: [&str; 11] = [
    ".dll", ".exe", ".so", ".dylib", ".asi", ".flt", ".mix", ".m3d", ".sys", ".drv", ".ocx",
];

/// Скрипты и прочее, что Windows или Wine запустит как программу.
const SCRIPT_EXTENSIONS: [&str; 21] = [
    ".bat", ".cmd", ".com", ".cpl", ".hta", ".jar", ".js", ".jse", ".lnk", ".msi", ".msp", ".pif",
    ".ps1", ".psm1", ".reg", ".scr", ".sh", ".vbe", ".vbs", ".wsf", ".wsh",
];

/// `PackError`: код (`TOO_LARGE`, `BAD_FORMAT`, `NEWER_FORMAT`, `UNKNOWN_GAME`) и текст.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct PackError {
    pub code: &'static str,
    pub message: String,
}

impl PackError {
    fn new(code: &'static str) -> PackError {
        PackError {
            code,
            message: code.to_owned(),
        }
    }

    /// `new DeliveryError("PACK_" + code, message)`.
    pub fn to_delivery(&self) -> DeliveryError {
        DeliveryError::new(Code::coerce(&format!("PACK_{}", self.code)), &self.message)
    }
}

/// Ошибка `collect`: слишком много файлов или ошибка `fs`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CollectError {
    #[error(transparent)]
    Pack(#[from] PackError),
    #[error(transparent)]
    Raw(#[from] RawError),
}

impl CollectError {
    /// Как исключение в `exportPack`: `PackError` → `DeliveryError("PACK_…")`.
    pub fn into_failure(self) -> Failure {
        match self {
            CollectError::Pack(error) => error.to_delivery().into(),
            CollectError::Raw(error) => error.into(),
        }
    }
}

fn native() -> NodePath {
    NodePath::native()
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `relative.split(path.sep).join("/")`.
fn to_posix(relative: &str) -> String {
    let sep = native().sep();
    if sep == "/" {
        relative.to_owned()
    } else {
        relative.split(sep).collect::<Vec<_>>().join("/")
    }
}

/// `relative.split("/").join(path.sep)`.
fn from_posix(relative: &str) -> String {
    relative.split('/').collect::<Vec<_>>().join(native().sep())
}

/// Итог `collect`: пути через `/` по порядку и их общий размер.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collected {
    pub files: Vec<String>,
    pub bytes: u64,
}

/// `collect(installPath)`: файлы папки игры для архива. Симлинки не идут: в архиве они бы
/// указывали на чужую машину.
pub fn collect(install_path: &Path) -> Result<Collected, CollectError> {
    let root = text(install_path);
    let path = native();
    let mut files = Vec::new();
    let mut bytes: u64 = 0;
    let mut stack = vec![String::new()];
    while let Some(dir) = stack.pop() {
        let full = path.join(&[&root, &dir]);
        let items = match js::fs::read_dir(Path::new(&full)) {
            Ok(items) => items,
            Err(error) if dir.is_empty() => {
                return Err(RawError::fs(&error, "scandir", Path::new(&full)).into());
            }
            Err(_) => continue,
        };
        for item in items {
            let relative = if dir.is_empty() {
                item.name.clone()
            } else {
                path.join(&[&dir, &item.name])
            };
            let posix = to_posix(&relative);
            if item.file_type.is_symlink() || SKIPPED.matches(&posix) {
                continue;
            }
            if item.file_type.is_dir() {
                stack.push(relative);
            } else if item.file_type.is_file() {
                files.push(posix);
                let file = path.join(&[&root, &relative]);
                let size = std::fs::metadata(&file)
                    .map_err(|error| RawError::fs(&error, "stat", Path::new(&file)))?
                    .len();
                bytes += size;
            }
            if files.len() > MAX_ENTRIES {
                return Err(PackError::new("TOO_LARGE").into());
            }
        }
    }
    files.sort_by(|a, b| js::cmp_utf16(a, b));
    Ok(Collected { files, bytes })
}

/// Входные данные `meta(input)`: то, что фасад передаёт экспорту (`extra`), и запись
/// `installed.json`.
#[derive(Debug, Clone, Default)]
pub struct MetaInput {
    pub launcher_id: Option<Value>,
    pub build: Option<Value>,
    pub launcher_version: Option<Value>,
    pub settings: Option<Value>,
    pub installed: Option<Value>,
    pub files: usize,
    pub bytes: u64,
    /// `exportedAt`, мс; нет — «сейчас».
    pub exported_at: Option<i64>,
}

/// `x || null`.
fn or_null(value: Option<&Value>) -> Value {
    value
        .filter(|value| js::truthy(value))
        .cloned()
        .unwrap_or(Value::Null)
}

/// `meta(input)`: сведения `.hlsr-pack.json`; `now` — «сейчас» в мс.
pub fn meta(input: &MetaInput, now: i64) -> Value {
    let empty = Value::Object(Map::new());
    let installed = input
        .installed
        .as_ref()
        .filter(|installed| js::truthy(installed))
        .unwrap_or(&empty);
    let field = |name: &str| jv::prop(installed, name);
    let exported_at = input.exported_at.filter(|ms| *ms != 0).unwrap_or(now);

    let mut game = Map::new();
    if let Some(launcher_id) = &input.launcher_id {
        game.insert("launcherId".to_owned(), launcher_id.clone());
    }
    if let Some(game_id) = field("gameId") {
        game.insert("gameId".to_owned(), game_id.clone());
    }
    game.insert("build".to_owned(), or_null(input.build.as_ref()));
    if let Some(version) = field("version") {
        game.insert("version".to_owned(), version.clone());
    }
    game.insert("publishedAt".to_owned(), or_null(field("publishedAt")));
    game.insert(
        "manifestSha256".to_owned(),
        or_null(field("manifestSha256")),
    );
    game.insert("archiveSha256".to_owned(), or_null(field("archiveSha256")));

    let mut out = Map::new();
    out.insert("format".to_owned(), json!(FORMAT));
    out.insert("version".to_owned(), json!(FORMAT_VERSION));
    out.insert(
        "exportedAt".to_owned(),
        json!(crate::timing::iso_from_ms(exported_at)),
    );
    out.insert(
        "launcherVersion".to_owned(),
        or_null(input.launcher_version.as_ref()),
    );
    out.insert("game".to_owned(), Value::Object(game));
    out.insert("files".to_owned(), json!(input.files));
    out.insert("bytes".to_owned(), json!(input.bytes));
    out.insert("settings".to_owned(), or_null(input.settings.as_ref()));
    Value::Object(out)
}

/// Сведения архива после `parseMeta` (и `installedVersion` из `readPackMeta`).
#[derive(Debug, Clone, PartialEq)]
pub struct PackMeta {
    pub game_id: String,
    pub version: String,
    pub launcher_id: Option<String>,
    pub build: Option<String>,
    pub published_at: Option<String>,
    pub manifest_sha256: Option<String>,
    pub archive_sha256: Option<String>,
    pub exported_at: Option<String>,
    pub files: Option<f64>,
    pub bytes: Option<f64>,
    pub settings: Option<Value>,
    /// `readPackMeta`: установленная версия той же игры (`null` — не установлена); у
    /// `parseMeta` поля нет.
    pub installed_version: Option<Option<String>>,
}

impl PackMeta {
    /// Объект 0.4.7: ключи и порядок как у `parseMeta`, затем `installedVersion`.
    pub fn to_json(&self) -> Value {
        let opt = |value: &Option<String>| value.as_ref().map_or(Value::Null, |v| json!(v));
        let num = |value: Option<f64>| value.map_or(Value::Null, js_number);
        let mut out = Map::new();
        out.insert("gameId".to_owned(), json!(self.game_id));
        out.insert("version".to_owned(), json!(self.version));
        out.insert("launcherId".to_owned(), opt(&self.launcher_id));
        out.insert("build".to_owned(), opt(&self.build));
        out.insert("publishedAt".to_owned(), opt(&self.published_at));
        out.insert("manifestSha256".to_owned(), opt(&self.manifest_sha256));
        out.insert("archiveSha256".to_owned(), opt(&self.archive_sha256));
        out.insert("exportedAt".to_owned(), opt(&self.exported_at));
        out.insert("files".to_owned(), num(self.files));
        out.insert("bytes".to_owned(), num(self.bytes));
        out.insert(
            "settings".to_owned(),
            self.settings.clone().unwrap_or(Value::Null),
        );
        if let Some(installed) = &self.installed_version {
            out.insert("installedVersion".to_owned(), opt(installed));
        }
        Value::Object(out)
    }
}

/// `/^[a-f0-9]{64}$/i`.
fn is_hex64(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `/^[A-Za-z0-9][A-Za-z0-9._-]{0,31}$/`.
fn is_pack_version(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 32
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// `/^[a-z0-9][a-z0-9-]{0,63}$/i`.
fn is_pack_game_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
}

fn optional_hex(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| is_hex64(value))
        .map(str::to_lowercase)
}

fn optional_date(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| value.encode_utf16().count() <= 40)
        .filter(|value| js::date_parse(value).is_some())
        .map(str::to_owned)
}

/// `String(x || "")`.
fn string_or_empty(value: Option<&Value>) -> String {
    value
        .filter(|value| js::truthy(value))
        .map(js::to_string)
        .unwrap_or_default()
}

/// Проверки `parseMeta`: игра из реестра и не сборка из депо Steam.
pub struct ParseOptions<'a> {
    pub is_known_game: Option<&'a dyn Fn(&str) -> bool>,
    pub is_depot_build: Option<&'a dyn Fn(&str) -> bool>,
}

/// `parseMeta(text, options)`.
pub fn parse_meta(text: &str, options: &ParseOptions) -> Result<PackMeta, PackError> {
    if text.len() > MAX_META_BYTES {
        return Err(PackError::new("BAD_FORMAT"));
    }
    let raw = json_parse(text).map_err(|_| PackError::new("BAD_FORMAT"))?;
    let version = jv::prop(&raw, "version");
    if !js::truthy(&raw)
        || !jv::is_str(jv::prop(&raw, "format"), FORMAT)
        || !matches!(version, Some(Value::Number(_)))
    {
        return Err(PackError::new("BAD_FORMAT"));
    }
    if version
        .and_then(jv::number)
        .is_some_and(|v| v > FORMAT_VERSION as f64)
    {
        return Err(PackError::new("NEWER_FORMAT"));
    }

    let empty = Value::Object(Map::new());
    let game = jv::prop(&raw, "game")
        .filter(|game| js::truthy(game))
        .unwrap_or(&empty);
    let game_id = string_or_empty(jv::prop(game, "gameId"));
    if !is_pack_game_id(&game_id)
        || options
            .is_known_game
            .is_some_and(|is_known| !is_known(&game_id))
    {
        return Err(PackError::new("UNKNOWN_GAME"));
    }
    if options
        .is_depot_build
        .is_some_and(|is_depot| is_depot(&game_id))
    {
        return Err(PackError::new("UNKNOWN_GAME"));
    }
    let version = string_or_empty(jv::prop(game, "version"));
    if !is_pack_version(&version) {
        return Err(PackError::new("BAD_FORMAT"));
    }

    let settings = jv::prop(&raw, "settings")
        .filter(|settings| js::truthy(settings) && settings.is_object())
        .cloned();
    let launcher_id = match jv::prop(game, "launcherId") {
        Some(value @ (Value::String(_) | Value::Number(_))) => Some(js::to_string(value)),
        _ => None,
    };
    let build = jv::prop(game, "build")
        .and_then(Value::as_str)
        .map(|build| js::slice_utf16(build, 32));
    let integer = |value: Option<&Value>| -> Option<f64> {
        let number = value.filter(|v| v.is_number()).and_then(jv::number)?;
        (number.fract() == 0.0 && number.is_finite() && number >= 0.0).then_some(number)
    };
    let finite = |value: Option<&Value>| -> Option<f64> {
        let number = value.filter(|v| v.is_number()).and_then(jv::number)?;
        (number.is_finite() && number >= 0.0).then_some(number)
    };

    Ok(PackMeta {
        game_id,
        version,
        launcher_id,
        build,
        published_at: optional_date(jv::prop(game, "publishedAt")),
        manifest_sha256: optional_hex(jv::prop(game, "manifestSha256")),
        archive_sha256: optional_hex(jv::prop(game, "archiveSha256")),
        exported_at: optional_date(jv::prop(&raw, "exportedAt")),
        files: integer(jv::prop(&raw, "files")),
        bytes: finite(jv::prop(&raw, "bytes")),
        settings,
        installed_version: None,
    })
}

/// `isExecutable(file)`: PE (Windows), ELF (Linux, Wine) и Mach-O — по первым байтам, а не по
/// расширению: движок и Miles грузят как библиотеки и `.asi`, `.flt`, `.mix`.
pub fn is_executable(file: &Path) -> bool {
    let extension = native().extname(&text(file)).to_lowercase();
    if SCRIPT_EXTENSIONS.contains(&extension.as_str()) {
        return true;
    }
    let mut head = [0u8; 4];
    let mut read = 0;
    let Ok(mut handle) = std::fs::File::open(file) else {
        return false;
    };
    while read < 4 {
        match handle.read(&mut head[read..]) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(_) => return false,
        }
    }
    if read >= 2 && head[0] == 0x4d && head[1] == 0x5a {
        return true;
    }
    let word = u32::from_be_bytes(head);
    if read >= 4 && word == 0x7f45_4c46 {
        return true;
    }
    read >= 4
        && [
            0xfeed_face,
            0xfeed_facf,
            0xcefa_edfe,
            0xcffa_edfe,
            0xcafe_babe,
        ]
        .contains(&word)
}

/// `walkFiles(root)`: файлы и всё, что не каталог (симлинки тоже), пути через `/`.
fn walk_files(root: &str) -> Vec<String> {
    let path = native();
    let mut files = Vec::new();
    let mut stack = vec![String::new()];
    while let Some(dir) = stack.pop() {
        let Ok(items) = js::fs::read_dir(Path::new(&path.join(&[root, &dir]))) else {
            continue;
        };
        for item in items {
            let relative = if dir.is_empty() {
                item.name.clone()
            } else {
                path.join(&[&dir, &item.name])
            };
            if item.file_type.is_dir() {
                stack.push(relative);
            } else {
                files.push(to_posix(&relative));
            }
        }
    }
    files
}

/// `removeInside(root, relative)`: путь внутри `root` — `rmSync(force)`; иначе `false`.
fn remove_inside(root: &str, relative: &str) -> Result<bool, Failure> {
    let path = native();
    let full = path.join(&[root, &from_posix(relative)]);
    let inside = relative_path(&path, root, &full);
    if inside.is_empty() || inside.starts_with("..") || path.is_absolute(&inside) {
        return Ok(false);
    }
    crate::file_ops::rm_sync(
        Path::new(&full),
        false,
        0,
        std::time::Duration::from_millis(100),
    )?;
    Ok(true)
}

/// `path.relative(from, to)` для путей, из которых второй собран `path.join` от первого.
fn relative_path(path: &NodePath, from: &str, to: &str) -> String {
    let from = path.resolve(&[from]);
    let to = path.resolve(&[to]);
    let sep = path.sep();
    let fold = |s: &str| match path.platform() {
        js::Platform::Win32 => s.to_lowercase(),
        js::Platform::Linux => s.to_owned(),
    };
    let from_parts: Vec<&str> = from.split(sep).filter(|p| !p.is_empty()).collect();
    let to_parts: Vec<&str> = to.split(sep).filter(|p| !p.is_empty()).collect();
    let common = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| fold(a) == fold(b))
        .count();
    let mut parts: Vec<&str> = vec![".."; from_parts.len() - common];
    parts.extend_from_slice(&to_parts[common..]);
    parts.join(sep)
}

/// Зависимость сборки (`launchConfig.dependencies`): папка, без которой файл версии убирается.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyFiles {
    pub target: String,
    pub version_file: String,
}

/// То, что `sanitize` читает из отчёта проверки.
#[derive(Debug, Clone, Default)]
pub struct SanitizeCheck {
    pub extra: Vec<String>,
    pub valid: Vec<String>,
}

/// `sanitize(root, check, options)`: чистка распакованного архива после сверки со списком
/// версии. `extra` — посторонние файлы из проверки, `valid` — подтверждённые списком.
/// Исполняемые файлы вне `valid` удаляются; у зависимостей без папки убирается и файл версии,
/// чтобы лаунчер скачал их заново перед запуском. Итог — отброшенные пути по порядку.
pub fn sanitize(
    root: &Path,
    check: &SanitizeCheck,
    dependencies: &[DependencyFiles],
) -> Result<Vec<String>, Failure> {
    let root_text = text(root);
    let path = native();
    let mut dropped = Vec::new();
    for relative in &check.extra {
        if remove_inside(&root_text, relative)? {
            dropped.push(relative.clone());
        }
    }

    let verified: HashSet<String> = check.valid.iter().map(|item| item.to_lowercase()).collect();
    for relative in walk_files(&root_text) {
        if verified.contains(&relative.to_lowercase()) {
            continue;
        }
        let full = PathBuf::from(path.join(&[&root_text, &from_posix(&relative)]));
        if !is_executable(&full) {
            continue;
        }
        if remove_inside(&root_text, &relative)? {
            dropped.push(relative);
        }
    }

    for dependency in dependencies {
        if dependency.target.is_empty() || dependency.version_file.is_empty() {
            continue;
        }
        let target = path.join(&[&root_text, &from_posix(&dependency.target)]);
        if std::fs::metadata(&target).is_ok() {
            continue;
        }
        remove_inside(&root_text, &dependency.version_file)?;
    }

    let names =
        js::fs::read_dir_names(root).map_err(|error| RawError::fs(&error, "scandir", root))?;
    for name in names {
        if name == META_FILE || name == "installed.json" || name.starts_with("installed.json.") {
            crate::file_ops::rm_sync(
                Path::new(&path.join(&[&root_text, &name])),
                false,
                0,
                std::time::Duration::from_millis(100),
            )?;
        }
    }
    dropped.sort_by(|a, b| js::cmp_utf16(a, b));
    Ok(dropped)
}

/// Итог [`set_aside`]: куда легли изменённые файлы (пути через `/` от корня).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetAside {
    pub custom: Vec<String>,
    pub backup: Vec<String>,
    pub rejected: Vec<String>,
}

/// `customLayerOf(gameId)`: папка слоя custom игры через `/`; нет слоя — `None`.
pub fn custom_layer_of(game_id: &str) -> Option<&'static str> {
    CUSTOM_LAYERS
        .iter()
        .find(|(game, ..)| *game == game_id)
        .map(|(_, dir, _)| *dir)
}

/// `setAside(root, gameId, changed)`: изменённые файлы сборки (`changed` — пути из подписанного
/// списка) уходят с мест сборки, чтобы туда легли оригиналы: контент — в слой custom или копией в
/// [`IMPORTED_BACKUP`], исполняемые удаляются.
pub fn set_aside(root: &Path, game_id: &str, changed: &[String]) -> Result<SetAside, Failure> {
    let path = native();
    let root_text = text(root);
    let layer = CUSTOM_LAYERS.iter().find(|(game, ..)| *game == game_id);
    // Корни — ASCII: совпадение без учёта регистра = `relative.toLowerCase().startsWith`.
    let root_of = |relative: &str| {
        layer.and_then(|(_, _, roots)| {
            roots.iter().position(|root| {
                relative
                    .get(..root.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(root))
            })
        })
    };
    // `hl2/x` и `ghostingmod/x` ложатся в один файл слоя: его занимает корень, который игра
    // читает раньше (порядок `roots`). Отступление от 0.4.9: там второй тихо затирал первый.
    let mut order: Vec<&String> = changed.iter().collect();
    order.sort_by_key(|relative| root_of(relative).unwrap_or(usize::MAX));
    let mut result = SetAside::default();
    for relative in order {
        let full = PathBuf::from(path.join(&[&root_text, &from_posix(relative)]));
        if !full.exists() {
            continue;
        }
        let extension = path.extname(relative).to_lowercase();
        if BINARY_EXTENSIONS.contains(&extension.as_str()) || is_executable(&full) {
            crate::file_ops::rm_sync(&full, false, 0, std::time::Duration::from_millis(100))?;
            result.rejected.push(relative.clone());
            continue;
        }
        let custom = match (layer, root_of(relative)) {
            (Some((_, dir, roots)), Some(index)) => {
                Some(format!("{dir}/{}", &relative[roots[index].len()..]))
            }
            _ => None,
        }
        // Файл слоя уже занят (лежал в архиве или его занял корень выше): этот — копией.
        .filter(|target| {
            std::fs::symlink_metadata(path.join(&[&root_text, &from_posix(target)])).is_err()
        });
        let in_layer = custom.is_some();
        let target = custom.unwrap_or_else(|| format!("{IMPORTED_BACKUP}/{relative}"));
        let destination = PathBuf::from(path.join(&[&root_text, &from_posix(&target)]));
        if let Some(dir) = destination.parent() {
            crate::node_fs::mkdir_recursive(dir)?;
        }
        std::fs::rename(&full, &destination).map_err(|error| {
            RawError::fs_with_dest(&error, "rename", &full, Some(destination.as_path()))
        })?;
        if in_layer {
            result.custom.push(target);
        } else {
            result.backup.push(target);
        }
    }
    Ok(result)
}
