//! Формат патча (`patch-format.js`): манифест патча — полная замена изменённых файлов из
//! `payload/` и список удаляемых; запрещённые поля, SemVer, разница двух версий, проверка
//! элементов архива.

use crate::archive_entries::{ArchiveEntry, classify_path};
use crate::errors::{Code, DeliveryError, Failure, RawError};
use crate::file_ops::hex;
use crate::js_value::{self as jv, is_hex64, is_integer, prop, template, truthy};
use crate::path_match::Matcher;
use hlsr_core::js;
use hlsr_core::json_text::stringify;
use regex::Regex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::LazyLock;

pub const SCHEMA_VERSION: i64 = 1;
pub const PAYLOAD_ROOT: &str = "payload";

/// Поля, которыми удалённые данные могли бы задать выполнение.
pub const FORBIDDEN_FIELDS: [&str; 16] = [
    "command",
    "commands",
    "script",
    "scripts",
    "exec",
    "executable",
    "shell",
    "shellArgs",
    "args",
    "arguments",
    "env",
    "installPath",
    "absolutePath",
    "steamAppId",
    "preInstall",
    "postInstall",
];

/// `/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/` (`\d` JS — только `0-9`).
static SEMVER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$").expect("регулярное выражение")
});
/// `/^[a-z0-9]+(-[a-z0-9]+)*$/`.
static GAME_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9]+(-[a-z0-9]+)*$").expect("регулярное выражение"));

fn invalid(message: impl Into<String>) -> DeliveryError {
    DeliveryError::new(Code::ManifestInvalid, message)
}

fn unsafe_entry(message: impl Into<String>) -> DeliveryError {
    DeliveryError::new(Code::ArchiveUnsafeEntry, message)
}

/// `what || "Путь"`.
fn what_or_default(what: Option<&str>) -> &str {
    what.filter(|what| !what.is_empty()).unwrap_or("Путь")
}

/// `assertSafeRelativePath(value, what)` для строки: путь, нормализованный до `/`.
pub fn assert_safe_relative_path(value: &str, what: Option<&str>) -> Result<String, DeliveryError> {
    let what = what_or_default(what);
    let problems = classify_path(value);
    if !problems.is_empty() {
        return Err(unsafe_entry(format!(
            "{what} недопустим: {value} ({})",
            problems.join(", ")
        )));
    }
    let drive = value.as_bytes();
    if (drive.len() >= 2 && drive[0].is_ascii_alphabetic() && drive[1] == b':')
        || value.starts_with('/')
        || value.starts_with('\\')
    {
        return Err(unsafe_entry(format!(
            "{what} должен быть относительным: {value}"
        )));
    }
    Ok(value.replace('\\', "/"))
}

/// То же для произвольного значения JSON: `null` — пустой путь, не строка без замечаний
/// `classifyPath` — `TypeError` (`value.startsWith is not a function`), как в JS.
pub fn assert_safe_relative_path_value(
    value: &Value,
    what: Option<&str>,
) -> Result<String, Failure> {
    match value {
        Value::String(text) => Ok(assert_safe_relative_path(text, what)?),
        other => {
            // classifyPath(String(value == null ? "" : value))
            let text = if other.is_null() {
                String::new()
            } else {
                js::to_string(other)
            };
            let problems = classify_path(&text);
            if !problems.is_empty() {
                return Err(unsafe_entry(format!(
                    "{} недопустим: {} ({})",
                    what_or_default(what),
                    template(Some(other)),
                    problems.join(", ")
                ))
                .into());
            }
            Err(RawError::new("value.startsWith is not a function").into())
        }
    }
}

/// Результат `diff`.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffResult {
    pub added: Vec<Value>,
    pub changed: Vec<Value>,
    pub unchanged: Vec<Value>,
    pub deleted_files: Vec<String>,
    /// Добавленные и изменённые файлы по порядку UTF-16.
    pub payload: Vec<Value>,
    pub payload_size: f64,
}

impl DiffResult {
    /// Объект как у 0.4.7 (те же ключи и порядок).
    pub fn to_json(&self) -> Value {
        json!({
            "added": self.added,
            "changed": self.changed,
            "unchanged": self.unchanged,
            "deletedFiles": self.deleted_files,
            "payload": self.payload,
            "payloadSize": hlsr_core::json_text::js_number(self.payload_size),
        })
    }
}

/// Параметры `diff`.
#[derive(Default)]
pub struct DiffOptions<'a> {
    /// Пути игрока, которые патч не трогает (шаблоны `path-match`).
    pub preserve_paths: &'a [String],
    /// Файл, который остаётся в сборке под другим путём или именем, — не удаляется.
    pub still_shipped: Option<&'a dyn Fn(&str) -> bool>,
}

/// Файлы манифеста по пути; повтор пути заменяет запись, оставляя первое место (`Map`).
fn files_by_path(manifest: &Value) -> Map<String, Value> {
    let mut by_path = Map::new();
    for file in jv::array(prop(manifest, "files")).into_iter().flatten() {
        if let Some(path) = prop(file, "path").and_then(Value::as_str) {
            by_path.insert(path.to_owned(), file.clone());
        }
    }
    by_path
}

fn size_of(file: &Value) -> f64 {
    jv::number(prop(file, "size").unwrap_or(&Value::Null)).unwrap_or(f64::NAN)
}

/// `diff(oldManifest, newManifest, options)`: что менять патчем между двумя версиями.
/// Записи манифестов — проверенные `files_manifest::validate` (путь — строка).
pub fn diff(
    old_manifest: &Value,
    new_manifest: &Value,
    options: &DiffOptions,
) -> Result<DiffResult, DeliveryError> {
    let preserved = Matcher::new(options.preserve_paths);
    let old_files = files_by_path(old_manifest);
    let new_files = files_by_path(new_manifest);

    let mut added = Vec::new();
    let mut changed = Vec::new();
    let mut deleted_files = Vec::new();
    let mut unchanged = Vec::new();

    for (relative_path, file) in &new_files {
        assert_safe_relative_path(relative_path, Some("Файл новой версии"))?;
        if preserved.matches(relative_path) {
            continue;
        }
        match old_files.get(relative_path) {
            None => added.push(file.clone()),
            Some(previous) if prop(previous, "sha256") != prop(file, "sha256") => {
                changed.push(file.clone());
            }
            Some(_) => unchanged.push(file.clone()),
        }
    }

    for relative_path in old_files.keys() {
        if new_files.contains_key(relative_path) || preserved.matches(relative_path) {
            continue;
        }
        if options
            .still_shipped
            .is_some_and(|still_shipped| still_shipped(relative_path))
        {
            continue;
        }
        deleted_files.push(assert_safe_relative_path(
            relative_path,
            Some("Удаляемый файл"),
        )?);
    }

    let path_of = |file: &Value| {
        prop(file, "path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let mut payload: Vec<Value> = added.iter().chain(&changed).cloned().collect();
    payload.sort_by(|a, b| js::cmp_utf16(&path_of(a), &path_of(b)));
    deleted_files.sort_by(|a, b| js::cmp_utf16(a, b));
    let payload_size = payload.iter().map(size_of).sum();

    Ok(DiffResult {
        added,
        changed,
        unchanged,
        deleted_files,
        payload,
        payload_size,
    })
}

/// `canonicalPayload(manifest)`: что подписывает `patchManifestSha256`. Ошибка — `TypeError`
/// JS (нет списка файлов или описания архива).
pub fn canonical_payload(manifest: &Value) -> Result<String, RawError> {
    if manifest.is_null() {
        return Err(jv::read_error(Some(manifest), "schemaVersion"));
    }
    let mut object = Map::new();
    let copy = |object: &mut Map<String, Value>, key: &str| {
        if let Some(value) = prop(manifest, key) {
            object.insert(key.to_owned(), value.clone());
        }
    };
    for key in [
        "schemaVersion",
        "gameId",
        "fromVersion",
        "toVersion",
        "payloadRoot",
    ] {
        copy(&mut object, key);
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
        let field = |name: &str| prop(file, name).cloned().unwrap_or(Value::Null);
        entries.push(Value::Array(vec![
            field("path"),
            field("size"),
            field("sha256"),
        ]));
    }
    object.insert("files".to_owned(), Value::Array(entries));
    copy(&mut object, "deletedFiles");

    let archive = match prop(manifest, "archive") {
        None => return Err(jv::read_error(None, "filename")),
        Some(Value::Null) => return Err(jv::read_error(Some(&Value::Null), "filename")),
        Some(archive) => archive,
    };
    let field = |name: &str| prop(archive, name).cloned().unwrap_or(Value::Null);
    object.insert(
        "archive".to_owned(),
        Value::Array(vec![field("filename"), field("size"), field("sha256")]),
    );
    for key in [
        "resultingFilesManifestSha256",
        "resultingTotalFiles",
        "resultingTotalSize",
    ] {
        copy(&mut object, key);
    }
    Ok(stringify(&Value::Object(object), ""))
}

/// `hashPatchManifest(manifest)`: SHA-256 от [`canonical_payload`].
pub fn hash_patch_manifest(manifest: &Value) -> Result<String, RawError> {
    Ok(hex(&Sha256::digest(
        canonical_payload(manifest)?.as_bytes(),
    )))
}

/// Параметры `validatePatchManifest`.
#[derive(Debug, Clone, Default)]
pub struct PatchValidateOptions {
    /// Установленная версия: патч обязан быть рассчитан на неё.
    pub expected_from: Option<String>,
}

/// `validatePatchManifest(data, options)`: данные возвращаются как есть. `Failure::Raw` —
/// `TypeError` JS (в списке удаляемых файлов не строка).
pub fn validate_patch_manifest<'a>(
    data: &'a Value,
    options: Option<&PatchValidateOptions>,
) -> Result<&'a Value, Failure> {
    if !data.is_object() {
        return Err(invalid("Манифест патча должен быть объектом").into());
    }
    let schema = prop(data, "schemaVersion");
    if !schema
        .and_then(jv::number)
        .is_some_and(|n| n == SCHEMA_VERSION as f64)
    {
        return Err(invalid(format!(
            "Неподдерживаемая версия манифеста патча: {}",
            template(schema)
        ))
        .into());
    }

    for field in FORBIDDEN_FIELDS {
        if data
            .as_object()
            .is_some_and(|object| object.contains_key(field))
        {
            return Err(invalid(format!(
                "Манифест патча не может содержать поле \"{field}\": удалённые данные не задают выполнение"
            ))
            .into());
        }
    }

    for field in ["gameId", "fromVersion", "toVersion", "payloadRoot"] {
        if prop(data, field)
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(invalid(format!("В манифесте патча нет поля {field}")).into());
        }
    }
    let text = |field: &str| {
        prop(data, field)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };

    if !GAME_ID.is_match(text("gameId")) {
        return Err(invalid(format!("Некорректный gameId: {}", text("gameId"))).into());
    }
    if !SEMVER.is_match(text("fromVersion")) || !SEMVER.is_match(text("toVersion")) {
        return Err(invalid("Версии патча должны быть SemVer").into());
    }
    if text("fromVersion") == text("toVersion") {
        return Err(invalid("fromVersion и toVersion совпадают").into());
    }
    if text("payloadRoot") != PAYLOAD_ROOT {
        return Err(invalid(format!("payloadRoot должен быть \"{PAYLOAD_ROOT}\"")).into());
    }

    let (Some(files), Some(deleted_files)) = (
        jv::array(prop(data, "files")),
        jv::array(prop(data, "deletedFiles")),
    ) else {
        return Err(invalid("В манифесте патча нет files/deletedFiles").into());
    };

    let mut seen: HashSet<&str> = HashSet::new();
    for file in files {
        let Some(path) = prop(file, "path")
            .filter(|_| js::truthy(file))
            .and_then(Value::as_str)
        else {
            return Err(invalid("Некорректная запись files").into());
        };
        assert_safe_relative_path(path, Some("Файл патча"))?;

        let size = prop(file, "size");
        if !is_integer(size) || jv::as_number(size) < 0.0 {
            return Err(invalid(format!("Некорректный размер {path}")).into());
        }
        if !is_hex64(prop(file, "sha256")) {
            return Err(invalid(format!("Некорректный sha256 {path}")).into());
        }
        if !seen.insert(path) {
            return Err(invalid(format!("Файл встречается дважды: {path}")).into());
        }
    }

    for relative_path in deleted_files {
        assert_safe_relative_path_value(relative_path, Some("Удаляемый файл"))?;
        if relative_path
            .as_str()
            .is_some_and(|path| seen.contains(path))
        {
            return Err(invalid(format!(
                "Файл одновременно обновляется и удаляется: {}",
                template(Some(relative_path))
            ))
            .into());
        }
    }

    let archive = prop(data, "archive").filter(|archive| js::truthy(archive));
    let Some(filename) = archive
        .and_then(|archive| prop(archive, "filename"))
        .and_then(Value::as_str)
    else {
        return Err(invalid("В манифесте патча нет описания архива").into());
    };
    assert_safe_relative_path(filename, Some("Имя архива патча"))?;
    if filename.contains('/') {
        return Err(invalid("Имя архива патча не может содержать путь").into());
    }
    let archive_size = archive.and_then(|archive| prop(archive, "size"));
    if !is_integer(archive_size) || jv::as_number(archive_size) <= 0.0 {
        return Err(invalid("Некорректный размер архива патча").into());
    }
    if !is_hex64(archive.and_then(|archive| prop(archive, "sha256"))) {
        return Err(invalid("Некорректный sha256 архива патча").into());
    }
    if !is_hex64(prop(data, "resultingFilesManifestSha256")) {
        return Err(invalid("Нет resultingFilesManifestSha256").into());
    }

    let declared = prop(data, "patchManifestSha256");
    if truthy(declared) {
        let expected = hash_patch_manifest(data)?;
        if !jv::is_str(declared, &expected) {
            return Err(invalid(format!(
                "patchManifestSha256 не сходится: {} != {expected}",
                template(declared)
            ))
            .into());
        }
    }

    if let Some(expected_from) = options
        .and_then(|options| options.expected_from.as_deref())
        .filter(|from| !from.is_empty())
        && text("fromVersion") != expected_from
    {
        return Err(DeliveryError::new(
            Code::ManifestInvalid,
            format!(
                "Патч рассчитан на версию {}, установлена {expected_from}",
                text("fromVersion")
            ),
        )
        .with_details(json!({"fromVersion": text("fromVersion"), "installed": expected_from}))
        .into());
    }

    Ok(data)
}

/// `validatePayloadEntries(manifest, entries)`: все объявленные файлы лежат в архиве под
/// `payload/`, чужих файлов и ссылок нет; регистр не важен. `Failure::Raw` — `TypeError` JS.
pub fn validate_payload_entries(
    manifest: &Value,
    entries: &[ArchiveEntry],
) -> Result<bool, Failure> {
    let files = match prop(manifest, "files") {
        None => return Err(jv::read_error(None, "map").into()),
        Some(Value::Null) => return Err(jv::read_error(Some(&Value::Null), "map").into()),
        Some(Value::Array(files)) => files,
        Some(_) => return Err(RawError::new("manifest.files.map is not a function").into()),
    };
    let mut expected: Vec<String> = Vec::new();
    let mut expected_set: HashSet<String> = HashSet::new();
    for file in files {
        if file.is_null() {
            return Err(jv::read_error(Some(file), "path").into());
        }
        let key = format!("{PAYLOAD_ROOT}/{}", template(prop(file, "path"))).to_lowercase();
        if expected_set.insert(key.clone()) {
            expected.push(key);
        }
    }
    let mut seen: HashSet<String> = HashSet::new();

    for entry in entries {
        let normalized = entry.path.replace('\\', "/");

        if entry.is_symlink {
            return Err(unsafe_entry(format!("Патч содержит symlink: {normalized}")).into());
        }
        assert_safe_relative_path(&normalized, Some("Элемент архива патча"))?;
        if entry.is_directory {
            continue;
        }

        let lower = normalized.to_lowercase();
        if !lower.starts_with(&format!("{PAYLOAD_ROOT}/")) {
            return Err(unsafe_entry(format!(
                "Элемент патча вне каталога {PAYLOAD_ROOT}: {normalized}"
            ))
            .into());
        }
        if !expected_set.contains(&lower) {
            return Err(unsafe_entry(format!(
                "Файл есть в архиве, но не объявлен в манифесте патча: {normalized}"
            ))
            .into());
        }
        seen.insert(lower);
    }

    let missing: Vec<&String> = expected
        .iter()
        .filter(|item| !seen.contains(*item))
        .collect();
    if !missing.is_empty() {
        let shown: Vec<&str> = missing.iter().take(5).map(|item| item.as_str()).collect();
        return Err(DeliveryError::new(
            Code::ManifestInvalid,
            format!("В архиве патча не хватает файлов: {}", shown.join(", ")),
        )
        .with_details(json!({"missing": missing.iter().take(20).collect::<Vec<_>>()}))
        .into());
    }
    Ok(true)
}

/// `defaultPatchName(gameId, fromVersion, toVersion)`.
pub fn default_patch_name(game_id: &str, from_version: &str, to_version: &str) -> String {
    format!("{game_id}-{from_version}-to-{to_version}")
}
