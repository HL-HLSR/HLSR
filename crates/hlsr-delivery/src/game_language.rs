//! Язык игры (`game-language.js`). Игра Source берёт язык Steam и ищет `gameui_<язык>.txt`,
//! `hl2_<язык>.txt` и т. д. Чтобы игрок выбрал в лаунчере язык, отличный от языка Steam, лаунчер
//! кладёт файлы выбранного языка под именами языка движка в `custom/hlsr-language/resource`: `custom/*`
//! подключён в `gameinfo.txt` и важнее остального. Папка переписывается при каждом запуске и
//! удаляется, когда выбрано «как в Steam».

use crate::cfg_search::LanguageOption;
use crate::errors::DeliveryError;
use crate::file_ops::{resolve_inside, rm_sync};
use crate::node_fs::{copy_file, mkdir_recursive, write_text};
use crate::ports::{Registry, RegistryHive, SystemRegistry};
use hlsr_core::js::fs::{read_dir_names, read_file};
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform};
use regress::Regex;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

pub const AUTO: &str = "auto";
pub const ENGLISH: &str = "english";

pub const OVERLAY_DIR: &str = "hlsr-language";

const README: &str = "Этой папкой распоряжается лаунчер HLSR.\r\n\
\r\n\
Здесь лежат языковые файлы выбранного в лаунчере языка игры. Папка\r\n\
переписывается при каждом запуске и удаляется, когда язык переключён\r\n\
обратно на «как в Steam». Свои файлы сюда класть не нужно — положи их\r\n\
в соседнюю папку внутри custom/.\r\n";

/// Набор языковых файлов: имя без языка и папки, где искать файл выбранного языка
/// (`mod` — `resource` мода, `hl2` — `hl2/resource`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileSet {
    pub base: &'static str,
    pub sources: &'static [&'static str],
}

pub const FILE_SETS: [FileSet; 4] = [
    FileSet {
        base: "gameui",
        sources: &["mod", "hl2"],
    },
    FileSet {
        base: "hl2",
        sources: &["hl2"],
    },
    FileSet {
        base: "valve",
        sources: &["hl2"],
    },
    FileSet {
        base: "chat",
        sources: &["hl2"],
    },
];

/// Раздел реестра с языком движка: `HKCU\Software\Valve\Source`.
pub const REGISTRY_KEY: &str = "HKCU\\Software\\Valve\\Source";
const REGISTRY_PATH: &str = "Software\\Valve\\Source";
pub const REGISTRY_VALUE: &str = "Language";

/// `/REG_SZ\s+(\S+)/`.
static REG_SZ_VALUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"REG_SZ\s+(\S+)").expect("выражение 0.4.7 разбирается"));

/// `/^[^/\n]*\bgame\S*\s+\S*custom\/\*/im`.
static CUSTOM_MOUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::with_flags(r"^[^/\n]*\bgame\S*\s+\S*custom\/\*", "im")
        .expect("выражение 0.4.7 разбирается")
});

/// `isLanguage(value)`: `/^[a-z]{2,24}$/`.
pub fn is_language(value: &str) -> bool {
    (2..=24).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_lowercase())
}

/// `normalizeChoice(value)`: не язык (и не строка) — `auto`.
pub fn normalize_choice(value: Option<&str>) -> String {
    match value {
        Some(language) if is_language(language) && language != AUTO => language.to_owned(),
        _ => AUTO.to_owned(),
    }
}

/// Откуда взять язык движка и как.
pub struct EngineOptions<'a> {
    /// ОС (`process.platform`).
    pub platform: Platform,
    /// Язык по умолчанию под Proton, где реестра Steam нет.
    pub default_language: Option<&'a str>,
    /// Реестр Windows.
    pub registry: &'a dyn Registry,
}

impl Default for EngineOptions<'_> {
    fn default() -> Self {
        EngineOptions {
            platform: Platform::CURRENT,
            default_language: None,
            registry: &SystemRegistry,
        }
    }
}

/// `engineLanguage(options)`: язык, с которым запустится движок; `None` — узнать не удалось.
/// Под Proton языка Steam в реестре нет: движок берёт язык по умолчанию.
pub fn engine_language(options: &EngineOptions<'_>) -> Option<String> {
    match options.platform {
        Platform::Linux => Some(
            options
                .default_language
                .filter(|language| is_language(language))
                .unwrap_or(ENGLISH)
                .to_owned(),
        ),
        Platform::Win32 => {
            let output =
                options
                    .registry
                    .query(RegistryHive::CurrentUser, REGISTRY_PATH, REGISTRY_VALUE)?;
            let found = REG_SZ_VALUE.find(&output)?;
            let value = output[found.group(1)?].to_lowercase();
            is_language(&value).then_some(value)
        }
    }
}

/// Папка `resource` мода и `hl2`.
fn source_dir(install_path: &Path, game_directory: &str, key: &str) -> PathBuf {
    let path = NodePath::native();
    let install = install_path.to_string_lossy();
    PathBuf::from(match key {
        "mod" => path.join(&[&install, game_directory, "resource"]),
        _ => path.join(&[&install, "hl2", "resource"]),
    })
}

fn resource_file_name(base: &str, language: &str) -> String {
    format!("{base}_{language}.txt")
}

/// Файл выбранного языка набора: первый существующий из его папок.
fn source_of_chosen_language(
    install_path: &Path,
    game_directory: &str,
    set: &FileSet,
    chosen_language: &str,
) -> Option<PathBuf> {
    set.sources.iter().find_map(|key| {
        let file = source_dir(install_path, game_directory, key)
            .join(resource_file_name(set.base, chosen_language));
        file.exists().then_some(file)
    })
}

/// Подключён ли `custom/*` в `gameinfo.txt` мода.
fn mounts_custom(install_path: &Path, game_directory: &str) -> bool {
    let path = NodePath::native();
    ["gameinfo.txt", "GameInfo.txt"].iter().any(|name| {
        let file = path.join(&[&install_path.to_string_lossy(), game_directory, name]);
        read_file(Path::new(&file))
            .is_ok_and(|bytes| CUSTOM_MOUNT.find(&js::utf8_string(&bytes)).is_some())
    })
}

/// `/^gameui_([a-z]{2,24})\.txt$/i`: язык из имени файла в нижнем регистре. Флаг `i` без `u` сводит
/// регистр только у ASCII.
fn language_of_file(name: &str) -> Option<String> {
    if !name.is_ascii() {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_prefix("gameui_")?.strip_suffix(".txt")?;
    is_language(stem).then(|| stem.to_owned())
}

/// `availableLanguages(installPath, gameDirectory)`: языки, для которых у мода есть `gameui_<язык>.txt`
/// (английский первым, остальные по алфавиту); пусто, если `custom/*` не подключён.
pub fn available_languages(install_path: &Path, game_directory: &str) -> Vec<String> {
    if !mounts_custom(install_path, game_directory) {
        return Vec::new();
    }
    let dir = source_dir(install_path, game_directory, "mod");
    let Ok(names) = read_dir_names(&dir) else {
        return Vec::new();
    };
    let mut found: Vec<String> = Vec::new();
    for name in names {
        if let Some(language) = language_of_file(&name)
            && !found.contains(&language)
        {
            found.push(language);
        }
    }
    found.sort();
    let (english, rest): (Vec<String>, Vec<String>) =
        found.into_iter().partition(|language| language == ENGLISH);
    english.into_iter().chain(rest).collect()
}

/// `overlayRoot(installPath, gameDirectory)`: `<мод>/custom/hlsr-language` внутри папки установки.
pub fn overlay_root(install_path: &Path, game_directory: &str) -> Result<PathBuf, DeliveryError> {
    let path = NodePath::native();
    resolve_inside(
        install_path,
        &path.join(&[game_directory, "custom", OVERLAY_DIR]),
        Some("Папка подмены языка"),
    )
}

/// Итог [`clear`]: `{ ok, removed, message? }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cleared {
    pub ok: bool,
    pub removed: bool,
    pub message: Option<String>,
}

impl Cleared {
    pub fn to_value(&self) -> Value {
        let mut object = json!({"ok": self.ok, "removed": self.removed});
        if let Some(message) = &self.message {
            object["message"] = json!(message);
        }
        object
    }
}

/// `clear(installPath, gameDirectory)`: убрать папку подмены. Не вышло — `ok: false` и текст ошибки;
/// исключением выходит только путь за папку установки.
pub fn clear(install_path: &Path, game_directory: &str) -> Result<Cleared, DeliveryError> {
    let root = overlay_root(install_path, game_directory)?;
    Ok(match rm_sync(&root, true, 3, Duration::from_millis(60)) {
        Ok(()) => Cleared {
            ok: true,
            removed: true,
            message: None,
        },
        Err(error) => Cleared {
            ok: false,
            removed: false,
            message: Some(error.message),
        },
    })
}

/// Почему подмены нет (`reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Выбрано «как в Steam».
    Auto,
    /// Язык движка не определить.
    EngineUnknown,
    /// Выбранный язык совпал с языком движка.
    SameAsEngine,
    /// Нет файлов выбранного языка.
    NoSource,
    /// Не удалось убрать прежнюю подмену.
    RemoveFailed,
    /// Не удалось записать файлы, подмена убрана.
    WriteFailed,
    /// Не удалось записать файлы и убрать недописанное.
    WriteFailedDirty,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Auto => "AUTO",
            Reason::EngineUnknown => "ENGINE_UNKNOWN",
            Reason::SameAsEngine => "SAME_AS_ENGINE",
            Reason::NoSource => "NO_SOURCE",
            Reason::RemoveFailed => "REMOVE_FAILED",
            Reason::WriteFailed => "WRITE_FAILED",
            Reason::WriteFailedDirty => "WRITE_FAILED_DIRTY",
        }
    }
}

/// Итог [`apply`]: `{ ok, applied, language, engineLanguage, reason?, message?, files }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub ok: bool,
    pub applied: bool,
    pub language: String,
    pub engine_language: Option<String>,
    pub reason: Option<Reason>,
    pub message: Option<String>,
    /// Имена файлов в папке подмены.
    pub files: Vec<String>,
}

impl Applied {
    /// Объект результата с теми же ключами и порядком, что у 0.4.7 (нет `reason` и `message` — нет ключей).
    pub fn to_value(&self) -> Value {
        let mut object = serde_json::Map::new();
        object.insert("ok".into(), json!(self.ok));
        object.insert("applied".into(), json!(self.applied));
        object.insert("language".into(), json!(self.language));
        object.insert("engineLanguage".into(), json!(self.engine_language));
        if let Some(reason) = self.reason {
            object.insert("reason".into(), json!(reason.as_str()));
        }
        if let Some(message) = &self.message {
            object.insert("message".into(), json!(message));
        }
        object.insert("files".into(), json!(self.files));
        Value::Object(object)
    }
}

/// Откуда берётся язык движка и каким реестром, плюс то, что нужно [`engine_language`].
pub struct ApplyOptions<'a> {
    /// Язык движка задан явно (`Given(None)` — «не определён»); `Detect` — по ОС и реестру.
    pub engine_language: LanguageOption<'a>,
    pub engine: EngineOptions<'a>,
}

impl Default for ApplyOptions<'_> {
    fn default() -> Self {
        ApplyOptions {
            engine_language: LanguageOption::Detect,
            engine: EngineOptions::default(),
        }
    }
}

fn no_overlay(language: &str, engine: Option<&str>, reason: Reason, cleared: &Cleared) -> Applied {
    Applied {
        ok: cleared.ok,
        applied: false,
        language: language.to_owned(),
        engine_language: engine.map(str::to_owned),
        reason: Some(if cleared.ok {
            reason
        } else {
            Reason::RemoveFailed
        }),
        message: cleared.message.clone(),
        files: Vec::new(),
    }
}

/// `apply(installPath, gameDirectory, choice, options)`: привести подмену в соответствие выбору.
/// Сбой диска — не ошибка, а результат (`ok: false`); исключением выходит только путь за папку установки.
pub fn apply(
    install_path: &Path,
    game_directory: &str,
    choice: Option<&str>,
    options: &ApplyOptions<'_>,
) -> Result<Applied, DeliveryError> {
    let language = normalize_choice(choice);
    if language == AUTO {
        let cleared = clear(install_path, game_directory)?;
        return Ok(no_overlay(&language, None, Reason::Auto, &cleared));
    }

    let detected;
    let engine: Option<&str> = match options.engine_language {
        LanguageOption::Given(given) => given,
        LanguageOption::Detect => {
            detected = engine_language(&options.engine);
            detected.as_deref()
        }
    };
    let Some(engine) = engine.filter(|engine| is_language(engine)) else {
        let cleared = clear(install_path, game_directory)?;
        return Ok(no_overlay(&language, None, Reason::EngineUnknown, &cleared));
    };
    if engine == language {
        let cleared = clear(install_path, game_directory)?;
        return Ok(no_overlay(
            &language,
            Some(engine),
            Reason::SameAsEngine,
            &cleared,
        ));
    }

    // Файл выбранного языка → имя, под которым его будет искать движок.
    let planned: Vec<(PathBuf, String)> = FILE_SETS
        .iter()
        .filter_map(|set| {
            source_of_chosen_language(install_path, game_directory, set, &language)
                .map(|file| (file, resource_file_name(set.base, engine)))
        })
        .collect();
    if planned.is_empty() {
        let cleared = clear(install_path, game_directory)?;
        return Ok(no_overlay(
            &language,
            Some(engine),
            Reason::NoSource,
            &cleared,
        ));
    }

    let cleared = clear(install_path, game_directory)?;
    if !cleared.ok {
        return Ok(no_overlay(
            &language,
            Some(engine),
            Reason::RemoveFailed,
            &cleared,
        ));
    }

    let root = overlay_root(install_path, game_directory)?;
    let resource_dir = root.join("resource");
    let written = (|| -> Result<(), crate::errors::RawError> {
        mkdir_recursive(&resource_dir)?;
        write_text(
            &root.join("README.txt"),
            README,
            crate::game_binds::Encoding::Utf8,
        )?;
        for (source, name) in &planned {
            copy_file(source, &resource_dir.join(name))?;
        }
        Ok(())
    })();
    if let Err(error) = written {
        let cleaned = clear(install_path, game_directory)?;
        return Ok(Applied {
            ok: false,
            applied: false,
            language,
            engine_language: Some(engine.to_owned()),
            reason: Some(if cleaned.ok {
                Reason::WriteFailed
            } else {
                Reason::WriteFailedDirty
            }),
            message: Some(error.message),
            files: Vec::new(),
        });
    }

    Ok(Applied {
        ok: true,
        applied: true,
        language,
        engine_language: Some(engine.to_owned()),
        reason: None,
        message: None,
        files: planned.into_iter().map(|(_, name)| name).collect(),
    })
}
