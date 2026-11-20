//! Редактор конфигов на диске (`game-config-install.js`): чтение итоговых значений из cfg
//! игрока и запись изменений (правила — в [`crate::game_config`]).
//!
//! Файлы ищутся так же, как в игре: сначала языковая папка GoldSrc (valve_WON_russian), потом
//! папка мода (см. [`crate::cfg_search`]); `search` — один экземпляр `CfgSearch` на сервис,
//! в нём кеш языка Steam. Тексты читаются и пишутся в кодировке движка (GoldSrc — latin1 и
//! CRLF, Source — UTF-8 и LF), файлы больше 1 МиБ не читаются (для редактора их нет).
//!
//! `launchConfig.gameDirectory` из 0.4.7 здесь — просто `game_directory`; пустой — как
//! отсутствие `launchConfig`.
//!
//! 0.4.9: правки пишутся только в файл игрока ([`crate::player_config`]), со сверкой его отпечатка.

use crate::cfg_document;
use crate::cfg_search::{self, CfgSearch, Files, LanguageOption};
use crate::errors::Failure;
use crate::game_binds::{self, Encoding};
use crate::game_config::{self, Catalog, Hit};
use crate::node_fs::read_text;
use crate::player_config;
use hlsr_core::js::Platform;
use hlsr_core::js::object::JsObject;
use hlsr_core::js::path::NodePath;
use hlsr_core::json_text::js_key_order;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;

/// Файлы больше этого размера редактор не читает (`MAX_FILE`).
const MAX_FILE: u64 = 1024 * 1024;

/// Читатель cfg: файлы в папках поиска, кеш по имени без учёта регистра.
struct Reader {
    files: Files,
    encoding: Encoding,
    cache: HashMap<String, Option<String>>,
}

impl Reader {
    /// Текст файла или `None` (нет, не файл, больше 1 МиБ, не читается).
    fn read(&mut self, name: &str) -> Option<String> {
        let lower = name.to_lowercase();
        if let Some(text) = self.cache.get(&lower) {
            return text.clone();
        }
        let text = if self.files.exists(name) {
            let file = self.files.path_of(name);
            match std::fs::metadata(&file) {
                Ok(stat) if stat.is_file() && stat.len() <= MAX_FILE => {
                    read_text(&file, self.encoding).ok()
                }
                _ => None,
            }
        } else {
            None
        };
        self.cache.insert(lower, text.clone());
        text
    }
}

struct Context {
    catalog: Catalog,
    reader: Reader,
}

/// `contextOf`: `None` — игра не поддерживается (нет каталога для движка или папки игры).
fn context_of(
    search: &CfgSearch,
    install_path: &Path,
    game_directory: &str,
    game_id: &str,
    engine: &str,
) -> Result<Option<Context>, Failure> {
    let Some(catalog) = game_config::catalog_for(game_id, engine) else {
        return Ok(None);
    };
    if game_directory.is_empty() {
        return Ok(None);
    }
    // Языковая папка GoldSrc (valve_WON_russian) ищется первой, как в игре.
    let dirs = search.search_dirs(install_path, game_directory, engine, LanguageOption::Detect)?;
    let encoding = game_binds::engine_of(engine).encoding;
    let files = cfg_search::files(dirs)?;
    Ok(Some(Context {
        catalog,
        reader: Reader {
            files,
            encoding,
            cache: HashMap::new(),
        },
    }))
}

fn cvars_of(catalog: &Catalog) -> Vec<&'static str> {
    catalog.fields.iter().map(|field| field.cvar).collect()
}

/// Итог [`read_config`].
#[derive(Debug)]
pub struct ReadConfig {
    /// `{ cvar: строка из cfg }`.
    pub values: JsObject<String>,
    /// Файл, где каждое значение задано последним (имя как в exec).
    pub sources: JsObject<String>,
    /// Выполненные файлы цепочки.
    pub files: Vec<String>,
    /// Отпечаток файла игрока (`None` — файла нет).
    pub sha256: Option<String>,
    /// Имя файла игрока.
    pub player_file: String,
    /// Переменные, у которых в файле игрока больше одной команды: строка выше перекрыта нижней.
    pub shadowed: Vec<String>,
}

fn player_ctx<'a>(
    search: &'a CfgSearch,
    install_path: &'a Path,
    game_id: &'a str,
    game_directory: &'a str,
) -> player_config::Ctx<'a> {
    player_config::Ctx {
        search,
        install: install_path,
        game_id,
        game_directory,
        platform: Platform::CURRENT,
    }
}

/// `shadowedOf(text)`.
fn shadowed_of(text: &str) -> Vec<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for item in cfg_document::commands_of(&cfg_document::parse(text)) {
        let Some(key) = item.key.filter(|key| key.starts_with("cvar:")) else {
            continue;
        };
        match counts.iter_mut().find(|(known, _)| *known == key) {
            Some(entry) => entry.1 += 1,
            None => counts.push((key, 1)),
        }
    }
    counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(key, _)| key["cvar:".len()..].to_owned())
        .collect()
}

/// Текущие значения настроек. `None` — игра не поддерживается (`readConfig`).
pub fn read_config(
    search: &CfgSearch,
    install_path: &Path,
    game_directory: &str,
    game_id: &str,
    engine: &str,
) -> Result<Option<ReadConfig>, player_config::Error> {
    let Some(mut context) = context_of(search, install_path, game_directory, game_id, engine)?
    else {
        return Ok(None);
    };
    let roots = game_config::engine_roots(engine).expect("движок известен каталогу");
    let executed = game_config::execute(
        &mut |name| context.reader.read(name),
        roots,
        &cvars_of(&context.catalog),
    );
    let mut values = JsObject::new();
    let mut sources = JsObject::new();
    for field in &context.catalog.fields {
        let Some(hit) = executed.found.get(&field.cvar.to_lowercase()) else {
            continue;
        };
        values.insert(field.cvar, hit.value.clone());
        sources.insert(field.cvar, hit.file.clone());
    }
    let current = player_config::read(&player_ctx(search, install_path, game_id, game_directory))?;
    Ok(Some(ReadConfig {
        values,
        sources,
        files: executed.files,
        sha256: current.sha256,
        player_file: NodePath::native().basename(&current.file.to_string_lossy()),
        shadowed: shadowed_of(&current.text),
    }))
}

/// Код отказа записи (`ConfigError.code`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigCode {
    /// Игра или движок не поддерживаются.
    Unsupported,
    /// Изменения — не объект.
    Invalid,
    /// Переменной нет в каталоге.
    UnknownSetting,
    /// Значение не годится.
    InvalidValue,
}

impl ConfigCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ConfigCode::Unsupported => "UNSUPPORTED",
            ConfigCode::Invalid => "INVALID",
            ConfigCode::UnknownSetting => "UNKNOWN_SETTING",
            ConfigCode::InvalidValue => "INVALID_VALUE",
        }
    }
}

/// `ConfigError`: код и переменная, на которой остановились.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub code: ConfigCode,
    /// Пустое имя — `None`, как `cvar || null`.
    pub cvar: Option<String>,
}

impl ConfigError {
    fn new(code: ConfigCode, cvar: &str) -> ConfigError {
        ConfigError {
            code,
            cvar: (!cvar.is_empty()).then(|| cvar.to_owned()),
        }
    }
}

/// Текст ошибки: `КОД` или `КОД: переменная`.
impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.cvar {
            Some(cvar) => write!(f, "{}: {cvar}", self.code.as_str()),
            None => f.write_str(self.code.as_str()),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Чем кончилась запись, если не удалась.
#[derive(Debug, thiserror::Error)]
pub enum WriteConfigError {
    /// Отказ по правилам редактора: ничего не записано.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// Ошибка файловой системы или путей.
    #[error(transparent)]
    Other(#[from] Failure),
    /// Ошибка файла игрока (`CONFIG_CHANGED` и другие).
    #[error(transparent)]
    Player(#[from] player_config::Error),
}

/// Итог [`write_config`].
#[derive(Debug)]
pub struct Written {
    /// Записанные файлы (имя файла игрока или пусто).
    pub written: Vec<String>,
    /// Отпечаток файла игрока после записи (`None` — файла нет).
    pub sha256: Option<String>,
}

/// Запись изменений (`writeConfig` 0.4.9). `changes` — объект `{ cvar: значение в единицах
/// окна }`. Всё пишется в файл игрока (раздел settings, на месте, если переменная там уже есть): он
/// выполняется последним и перекрывает config.cfg и файлы, которые лаунчер не правит. `expected` —
/// отпечаток файла из [`read_config`]; не совпал — `CONFIG_CHANGED`.
// shortcut: переменную, которую перебивает чужой exec после её строки, не отслеживаем (как 0.4.9).
pub fn write_config(
    search: &CfgSearch,
    install_path: &Path,
    game_directory: &str,
    game_id: &str,
    engine: &str,
    changes: &Value,
    expected: Option<Option<&str>>,
) -> Result<Written, WriteConfigError> {
    let Some(mut context) = context_of(search, install_path, game_directory, game_id, engine)?
    else {
        return Err(ConfigError::new(ConfigCode::Unsupported, "").into());
    };
    let Value::Object(changes) = changes else {
        return Err(ConfigError::new(ConfigCode::Invalid, "").into());
    };
    let roots = game_config::engine_roots(engine).expect("движок известен каталогу");
    let executed = game_config::execute(
        &mut |name| context.reader.read(name),
        roots,
        &cvars_of(&context.catalog),
    );
    let ctx = player_ctx(search, install_path, game_id, game_directory);
    let current = player_config::read(&ctx)?;
    let mut doc = cfg_document::parse(&current.text);
    for (cvar, value) in js_key_order(changes) {
        let Some(field) = context.catalog.by_cvar(&cvar.to_lowercase()) else {
            return Err(ConfigError::new(ConfigCode::UnknownSetting, cvar).into());
        };
        let hit: Option<&Hit> = executed.found.get(&field.cvar.to_lowercase());
        let Some(next) = game_config::to_cfg(field, value, hit.map(|h| h.value.as_str())) else {
            return Err(ConfigError::new(ConfigCode::InvalidValue, field.cvar).into());
        };
        if hit.is_some_and(|h| h.value == next) {
            continue;
        }
        doc = cfg_document::set(
            &doc,
            &format!("cvar:{}", field.cvar.to_lowercase()),
            &next,
            "settings",
        );
    }
    let text = cfg_document::serialize(&doc);
    if text == current.text {
        return Ok(Written {
            written: Vec::new(),
            sha256: current.sha256,
        });
    }
    let result = player_config::write(&ctx, &text, expected)?;
    Ok(Written {
        written: vec![NodePath::native().basename(&result.file.to_string_lossy())],
        sha256: Some(result.sha256),
    })
}
