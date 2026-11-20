//! Файловый манифест сборки, `*.files.json` (`files-manifest.js`): построение по каталогу,
//! чтение, проверка, хеш по каноническим полям.
//!
//! Стоковые cfg мода (0.4.9) лежат в папках игрока (`mutable-paths`): в `files` они не входят,
//! поэтому установка, обновления, патчи и удаления их не трогают, а `manifestSha256` от них не
//! зависит. «Проверить файлы» и ремонт сверяют их по отдельному списку `stockConfigs` того же
//! файла. `config.cfg` пишет игра, `autoexec.cfg` и `hlsr-*.cfg` — лаунчер: это не стоковые.

use crate::archive_entries::classify_path;
use crate::errors::{Code, DeliveryError, Failure, RawError};
use crate::file_ops::hex;
use crate::js_value::{self as jv, is_hex64, is_integer, prop, template, truthy};
use crate::mutable_paths::matches_mutable;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, errno};
use hlsr_core::json_text::{js_number, json_parse, stringify};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const SCHEMA_VERSION: i64 = 1;
/// Предел записей по умолчанию.
pub const MAX_ENTRIES: usize = 200_000;
const HASH_CHUNK: usize = 4 * 1024 * 1024;
/// `STOCK_CONFIG_DIRS`: папки cfg мода, где лежат стоковые cfg сборки.
pub const STOCK_CONFIG_DIRS: [&str; 4] = [
    "ghostingmod/cfg",
    "hl2/cfg",
    "lostcoast/cfg",
    "hl2_russian/cfg",
];
/// `MAX_STOCK_CONFIGS`: больше записей `stockConfigs` манифест не несёт.
pub const MAX_STOCK_CONFIGS: usize = 1000;

/// `NOT_STOCK`: `/^(config|autoexec)\.cfg$|^hlsr-.*\.cfg$/i`.
fn not_stock(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == "config.cfg"
        || lower == "autoexec.cfg"
        || (lower.starts_with("hlsr-") && lower.ends_with(".cfg"))
}

/// `isStockConfigPath(relative)`: `.cfg` прямо в одной из [`STOCK_CONFIG_DIRS`], кроме cfg игры
/// и лаунчера.
pub fn is_stock_config_path(relative: &str) -> bool {
    let Some(slash) = relative.rfind('/').filter(|&slash| slash > 0) else {
        return false;
    };
    let dir = relative[..slash].to_lowercase();
    let name = &relative[slash + 1..];
    name.to_lowercase().ends_with(".cfg")
        && !not_stock(name)
        && STOCK_CONFIG_DIRS.contains(&dir.as_str())
}

/// `collectStockConfigs(root)`: стоковые cfg каталога сборки — файлы (не ссылки) прямо в
/// [`STOCK_CONFIG_DIRS`], `{path, size, sha256}` по возрастанию пути (кодовые единицы UTF-16).
pub fn collect_stock_configs(root: &Path) -> Result<Vec<Value>, RawError> {
    let mut entries: Vec<(String, Value)> = Vec::new();
    for dir in STOCK_CONFIG_DIRS {
        let Ok(items) = js::fs::read_dir(&root.join(dir)) else {
            continue;
        };
        for item in items {
            let relative = format!("{dir}/{}", item.name);
            if !item.file_type.is_file() || !is_stock_config_path(&relative) {
                continue;
            }
            let full = root.join(&relative);
            let size = std::fs::metadata(&full)
                .map_err(|e| RawError::fs(&e, "stat", &full))?
                .len() as f64;
            let entry = json!({
                "path": relative,
                "size": js_number(size),
                "sha256": hash_file_sync(&full)?,
            });
            entries.push((relative, entry));
        }
    }
    entries.sort_by(|a, b| js::cmp_utf16(&a.0, &b.0));
    Ok(entries.into_iter().map(|(_, entry)| entry).collect())
}

/// `toRelative(root, full)`: путь от корня через `/`.
pub fn to_relative(root: &Path, full: &Path) -> String {
    let relative = full.strip_prefix(root).unwrap_or(full);
    relative.to_string_lossy().replace('\\', "/")
}

/// `hashFileSync(file)`: SHA-256 файла в hex, чтение блоками по 4 МиБ.
pub fn hash_file_sync(file: &Path) -> Result<String, RawError> {
    let mut handle = std::fs::File::open(file).map_err(|e| RawError::fs(&e, "open", file))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; HASH_CHUNK];
    loop {
        let read = match handle.read(&mut buffer) {
            Ok(read) => read,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(read_error(&e)),
        };
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hex(&hash.finalize()))
}

/// Ошибка `fs.readSync`: в тексте Node нет пути (`EISDIR: illegal operation on a directory, read`).
fn read_error(error: &std::io::Error) -> RawError {
    let Some(code) = errno::code(error) else {
        return RawError::new(error.to_string());
    };
    RawError {
        message: format!("{code}: {}, read", errno::description(&code)),
        code: Some(code.into_owned()),
        syscall: Some("read".to_owned()),
        path: None,
        stderr: None,
    }
}

/// `hashFile(file)` — то же, что [`hash_file_sync`].
pub fn hash_file(file: &Path) -> Result<String, RawError> {
    hash_file_sync(file)
}

/// Что обход пропустил: изменяемые игроком пути, ссылки, небезопасные имена.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Skipped {
    pub mutable: Vec<String>,
    pub links: Vec<String>,
    pub unsafe_entries: Vec<String>,
}

/// Файл для манифеста: путь от корня и полный путь.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedFile {
    pub relative: String,
    pub full: PathBuf,
}

/// Итог `collectFiles`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Collected {
    pub files: Vec<CollectedFile>,
    pub skipped: Skipped,
}

/// `collectFiles(root, { isMutable })`: обход без перехода по ссылкам, файлы по порядку UTF-16.
pub fn collect_files(
    root: &Path,
    is_mutable: Option<&dyn Fn(&str) -> bool>,
) -> Result<Collected, DeliveryError> {
    let mut files = Vec::new();
    let mut skipped = Skipped::default();
    let mut stack: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];

    while let Some((dir, prefix)) = stack.pop() {
        // `readdirSync`: в Linux имена по `strcmp` (libuv), в Windows — порядок NTFS.
        let items = js::fs::read_dir(&dir).map_err(|e| {
            DeliveryError::new(
                Code::PathNotWritable,
                format!(
                    "Не удалось прочитать {}: {}",
                    dir.display(),
                    RawError::fs(&e, "scandir", &dir).message
                ),
            )
        })?;
        for item in items {
            let name = item.name;
            let relative = if prefix.is_empty() {
                name.replace('\\', "/")
            } else {
                format!("{prefix}/{}", name.replace('\\', "/"))
            };
            let full = dir.join(&name);
            let kind = item.file_type;

            if kind.is_symlink() {
                skipped.links.push(relative);
                continue;
            }
            if !classify_path(&relative).is_empty() {
                skipped.unsafe_entries.push(relative);
                continue;
            }
            let mutable = match is_mutable {
                Some(is_mutable) => is_mutable(&relative),
                None => matches_mutable(&relative),
            };
            if mutable {
                skipped.mutable.push(relative);
                continue;
            }
            if kind.is_dir() {
                stack.push((full, relative));
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            files.push(CollectedFile { relative, full });
        }
    }

    files.sort_by(|a, b| js::cmp_utf16(&a.relative, &b.relative));
    Ok(Collected { files, skipped })
}

/// `canonicalPayload(manifest)`: `JSON.stringify` полей, которые подписывает `manifestSha256`.
/// Ошибка — `TypeError` JS (нет списка `files`, запись `null`).
pub fn canonical_payload(manifest: &Value) -> Result<String, RawError> {
    if manifest.is_null() {
        return Err(jv::read_error(Some(manifest), "schemaVersion"));
    }
    let mut object = Map::new();
    for key in [
        "schemaVersion",
        "gameId",
        "version",
        "totalFiles",
        "totalSize",
    ] {
        if let Some(value) = prop(manifest, key) {
            object.insert(key.to_owned(), value.clone());
        }
    }
    let files = match prop(manifest, "files") {
        None => return Err(jv::read_error(None, "map")),
        Some(Value::Null) => return Err(jv::read_error(Some(&Value::Null), "map")),
        Some(Value::Array(files)) => files,
        Some(_) => return Err(RawError::new("manifest.files.map is not a function")),
    };
    let mut entries = Vec::with_capacity(files.len());
    for file in files {
        if file.is_null() {
            return Err(jv::read_error(Some(file), "path"));
        }
        // `undefined` в массиве JSON.stringify пишет как `null`.
        let field = |name: &str| prop(file, name).cloned().unwrap_or(Value::Null);
        entries.push(Value::Array(vec![
            field("path"),
            field("size"),
            field("sha256"),
        ]));
    }
    object.insert("files".to_owned(), Value::Array(entries));
    Ok(stringify(&Value::Object(object), ""))
}

/// `hashManifest(manifest)`: SHA-256 от [`canonical_payload`].
pub fn hash_manifest(manifest: &Value) -> Result<String, RawError> {
    Ok(hex(&Sha256::digest(
        canonical_payload(manifest)?.as_bytes(),
    )))
}

/// Ход `build`: `{processed, total, path}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildProgress {
    pub processed: usize,
    pub total: usize,
    pub path: String,
}

/// Параметры `build`.
pub struct BuildOptions<'a> {
    pub root: &'a Path,
    pub game_id: Option<&'a str>,
    pub version: Option<&'a str>,
    /// Время для `generatedAt`; нет — сейчас.
    pub now: Option<SystemTime>,
    pub is_mutable: Option<&'a dyn Fn(&str) -> bool>,
    pub on_progress: Option<&'a mut dyn FnMut(&BuildProgress)>,
}

impl<'a> BuildOptions<'a> {
    /// Только каталог сборки.
    pub fn new(root: &'a Path) -> BuildOptions<'a> {
        BuildOptions {
            root,
            game_id: None,
            version: None,
            now: None,
            is_mutable: None,
            on_progress: None,
        }
    }
}

/// Итог `build`: манифест и пропущенное.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub manifest: Value,
    pub skipped: Skipped,
}

/// `build(options)`: манифест по каталогу сборки.
pub fn build(mut options: BuildOptions) -> Result<Built, Failure> {
    let cwd = std::env::current_dir()
        .map(|dir| dir.to_string_lossy().into_owned())
        .unwrap_or_default();
    let root_text = options.root.to_string_lossy();
    let root = PathBuf::from(NodePath::new(js::Platform::CURRENT, cwd).resolve(&[&root_text]));

    if !root.exists() {
        return Err(DeliveryError::new(
            Code::PathInvalid,
            format!("Каталог сборки не найден: {}", root.display()),
        )
        .into());
    }

    let Collected { files, skipped } = collect_files(&root, options.is_mutable)?;
    let mut entries = Vec::with_capacity(files.len());
    let mut total_size = 0.0f64;

    for (index, file) in files.iter().enumerate() {
        let size = std::fs::metadata(&file.full)
            .map_err(|e| RawError::fs(&e, "stat", &file.full))?
            .len() as f64;
        let sha256 = hash_file_sync(&file.full)?;
        entries.push(json!({
            "path": file.relative,
            "size": js_number(size),
            "sha256": sha256,
        }));
        total_size += size;

        if let Some(progress) = options.on_progress.as_mut()
            && (index % 50 == 0 || index == files.len() - 1)
        {
            progress(&BuildProgress {
                processed: index + 1,
                total: files.len(),
                path: file.relative.clone(),
            });
        }
    }

    let mut manifest = Map::new();
    manifest.insert("schemaVersion".to_owned(), json!(SCHEMA_VERSION));
    if let Some(game_id) = options.game_id {
        manifest.insert("gameId".to_owned(), json!(game_id));
    }
    if let Some(version) = options.version {
        manifest.insert("version".to_owned(), json!(version));
    }
    manifest.insert(
        "generatedAt".to_owned(),
        json!(js::iso_string(options.now.unwrap_or_else(SystemTime::now))),
    );
    manifest.insert("totalFiles".to_owned(), json!(entries.len()));
    manifest.insert("totalSize".to_owned(), js_number(total_size));
    manifest.insert("files".to_owned(), Value::Array(entries));
    let listed: HashSet<String> = files.iter().map(|f| f.relative.to_lowercase()).collect();
    let stock: Vec<Value> = collect_stock_configs(&root)?
        .into_iter()
        .filter(|entry| !listed.contains(&template(prop(entry, "path")).to_lowercase()))
        .collect();
    if !stock.is_empty() {
        manifest.insert("stockConfigs".to_owned(), Value::Array(stock));
    }

    let mut manifest = Value::Object(manifest);
    let hash = hash_manifest(&manifest)?;
    if let Value::Object(object) = &mut manifest {
        object.insert("manifestSha256".to_owned(), json!(hash));
    }
    Ok(Built { manifest, skipped })
}

/// `load(file)`: прочитать и проверить.
pub fn load(file: &Path) -> Result<Value, DeliveryError> {
    let raw = std::fs::read(file).map_err(|_| {
        DeliveryError::new(
            Code::ManifestNotFound,
            format!("Файловый манифест не найден: {}", file.display()),
        )
    })?;
    let data = json_parse(&String::from_utf8_lossy(&raw)).map_err(|e| {
        DeliveryError::new(
            Code::ManifestInvalid,
            format!("Файловый манифест не является JSON: {e}"),
        )
    })?;
    validate(&data, file.to_str(), None)?;
    Ok(data)
}

/// Параметры `validate`.
#[derive(Debug, Clone, Default)]
pub struct ValidateOptions {
    /// Ожидаемая игра (пустая строка — не проверять).
    pub game_id: Option<String>,
    /// Ожидаемая версия.
    pub version: Option<String>,
    /// Предел записей; ложное значение JS (`0`, `NaN`) — [`MAX_ENTRIES`].
    pub max_entries: Option<f64>,
    pub require_manifest_sha256: bool,
}

fn invalid(message: impl Into<String>) -> DeliveryError {
    DeliveryError::new(Code::ManifestInvalid, message)
}

/// `validate(data, source, options)`: данные возвращаются как есть.
pub fn validate<'a>(
    data: &'a Value,
    source: Option<&str>,
    options: Option<&ValidateOptions>,
) -> Result<&'a Value, DeliveryError> {
    let default_options = ValidateOptions::default();
    let options = options.unwrap_or(&default_options);
    let source = source.filter(|source| !source.is_empty());

    if !data.is_object() {
        return Err(invalid("Файловый манифест должен быть JSON-объектом"));
    }
    if !prop(data, "schemaVersion")
        .and_then(jv::number)
        .is_some_and(|n| n == SCHEMA_VERSION as f64)
    {
        return Err(invalid("Неподдерживаемая версия файлового манифеста"));
    }
    let Some(files) = prop(data, "files").and_then(Value::as_array) else {
        return Err(invalid("В файловом манифесте нет списка файлов"));
    };

    let max_entries = options
        .max_entries
        .filter(|max| *max != 0.0 && !max.is_nan())
        .unwrap_or(MAX_ENTRIES as f64);
    if files.len() as f64 > max_entries {
        return Err(invalid(format!(
            "Слишком много записей в файловом манифесте: {} при пределе {}",
            files.len(),
            template(Some(&js_number(max_entries)))
        )));
    }

    if let Some(game_id) = options.game_id.as_deref().filter(|id| !id.is_empty())
        && !jv::is_str(prop(data, "gameId"), game_id)
    {
        return Err(invalid(format!(
            "Файловый манифест описывает игру {}, ожидалась {game_id}",
            template(prop(data, "gameId"))
        )));
    }
    if let Some(version) = options.version.as_deref().filter(|v| !v.is_empty())
        && !jv::is_str(prop(data, "version"), version)
    {
        return Err(invalid(format!(
            "Файловый манифест описывает версию {}, ожидалась {version}",
            template(prop(data, "version"))
        )));
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut total_size = 0.0f64;
    for file in files {
        let path = prop(file, "path").and_then(Value::as_str);
        if !js::truthy(file) || path.is_none() || !is_hex64(prop(file, "sha256")) {
            let shown = if js::truthy(file) {
                template(prop(file, "path"))
            } else {
                template(Some(file))
            };
            return Err(invalid(format!("Некорректная запись манифеста: {shown}")));
        }
        let path = path.unwrap_or_default();
        let size = prop(file, "size");
        if !is_integer(size) || jv::as_number(size) < 0.0 {
            return Err(invalid(format!(
                "Некорректный размер файла {path}: {}",
                template(size)
            )));
        }

        let problems = classify_path(path);
        if !problems.is_empty() {
            return Err(DeliveryError::new(
                Code::ArchiveUnsafeEntry,
                format!(
                    "Опасный путь в файловом манифесте: {path} ({})",
                    problems.join(", ")
                ),
            )
            .with_details(json!({"source": source})));
        }

        if !seen.insert(path.to_lowercase()) {
            return Err(invalid(format!(
                "Путь встречается в файловом манифесте дважды: {path}"
            )));
        }
        total_size += jv::as_number(size);
    }

    if let Some(stock) = prop(data, "stockConfigs") {
        let Some(stock) = stock.as_array().filter(|s| s.len() <= MAX_STOCK_CONFIGS) else {
            return Err(invalid(
                "Некорректный список стоковых cfg в файловом манифесте",
            ));
        };
        for file in stock {
            let path = prop(file, "path").and_then(Value::as_str);
            let size = prop(file, "size");
            let valid = js::truthy(file)
                && path.is_some_and(|path| {
                    is_stock_config_path(path)
                        && classify_path(path).is_empty()
                        && !seen.contains(&path.to_lowercase())
                })
                && is_hex64(prop(file, "sha256"))
                && is_integer(size)
                && jv::as_number(size) >= 0.0;
            if !valid {
                let shown = if js::truthy(file) {
                    template(prop(file, "path"))
                } else {
                    template(Some(file))
                };
                return Err(invalid(format!(
                    "Некорректный стоковый cfg в файловом манифесте: {shown}"
                )));
            }
            seen.insert(path.unwrap_or_default().to_lowercase());
        }
    }

    if let Some(declared) = prop(data, "totalFiles").filter(|value| !value.is_null())
        && jv::number(declared) != Some(files.len() as f64)
    {
        return Err(invalid(format!(
            "totalFiles не сходится: заявлено {}, записей {}",
            template(Some(declared)),
            files.len()
        )));
    }
    if let Some(declared) = prop(data, "totalSize").filter(|value| !value.is_null())
        && jv::number(declared) != Some(total_size)
    {
        return Err(invalid(format!(
            "totalSize не сходится: заявлено {}, посчитано {}",
            template(Some(declared)),
            template(Some(&js_number(total_size)))
        )));
    }

    let expected = hash_manifest(data).map_err(|e| invalid(e.message))?;
    let declared = prop(data, "manifestSha256");
    if truthy(declared) && !jv::is_str(declared, &expected) {
        return Err(invalid(format!(
            "manifestSha256 не сходится: в файле {}, посчитано {expected}",
            template(declared)
        )));
    }
    if options.require_manifest_sha256 && !truthy(declared) {
        return Err(invalid("В файловом манифесте нет manifestSha256"));
    }

    Ok(data)
}

/// `serialize(manifest)`: `JSON.stringify(manifest, null, 2) + "\n"`.
pub fn serialize(manifest: &Value) -> String {
    stringify(manifest, "  ") + "\n"
}
