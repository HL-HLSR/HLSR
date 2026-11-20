//! Настройки игр на диске (`GameSettingsStore.js`): `game-settings.json` в папке данных —
//! `{ "version": 1, "games": { "<игра>": { … } } }`, два пробела и `\n` в конце (`writeJsonAtomic`).
//! Игра записывается нормализованной (`normalizeSettings`), читается санитизированной
//! (`sanitizeSettings`); чужие записи и их порядок файл переживает, а лишние поля верхнего
//! уровня при записи теряются — как у 0.4.7.
//!
//! Файл любой формы читается как у 0.4.7: нет файла, не JSON и не объект — настройки по
//! умолчанию (повреждённый разбираемый файл ещё и в журнал). Если `games` — массив, а не
//! объект, записи ищутся по номерам, как у массива JS (имена свойств в файл не попадают).

use crate::atomic::write_json_atomic;
use crate::errors::RawError;
use crate::launch_args::{self, ArgsError, Settings};
use crate::ports::Logger;
use hlsr_core::js;
use hlsr_core::json_store::read_json_safe;
use hlsr_core::json_text::array_index;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const FILE_VERSION: u32 = 1;
pub const FILE_NAME: &str = "game-settings.json";

/// Предельный номер записи, если `games` — массив: дальше JS пишет файл в сотни мегабайт и
/// падает с `Invalid string length`.
const MAX_ARRAY_INDEX: u32 = 1_000_000;

/// Имена, которые есть у любого объекта JS (`Object.prototype`), подходящие под идентификатор игры.
const OBJECT_PROTOTYPE_NAMES: [&str; 7] = [
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
];

/// Методы `Array.prototype`: у массива `games` `all.games[имя]` — функция.
const ARRAY_PROTOTYPE_NAMES: [&str; 36] = [
    "at",
    "concat",
    "copyWithin",
    "entries",
    "every",
    "fill",
    "filter",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "flat",
    "flatMap",
    "forEach",
    "includes",
    "indexOf",
    "join",
    "keys",
    "lastIndexOf",
    "map",
    "pop",
    "push",
    "reduce",
    "reduceRight",
    "reverse",
    "shift",
    "slice",
    "some",
    "sort",
    "splice",
    "toReversed",
    "toSorted",
    "toSpliced",
    "unshift",
    "values",
    "with",
];

/// `isSafeGameId(value)`: `/^[a-z0-9][a-z0-9-]{0,63}$/i`.
pub fn is_safe_game_id(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && value.chars().count() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Отказ `save` и `reset`: проверка настроек или сбой записи (исключение Node).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StoreError {
    /// `{ ok: false, error }`: `INVALID_GAME_ID` или отказ проверки параметров.
    #[error(transparent)]
    Rejected(#[from] ArgsError),
    /// Исключение: файл не записан.
    #[error(transparent)]
    Io(#[from] RawError),
}

/// `all.games` — объект или массив.
enum Games {
    Object(Map<String, Value>),
    Array(Vec<Value>),
}

impl Games {
    /// `all.games[id]` как данные: собственное свойство или элемент массива; прочее
    /// (функции прототипа, `length`) для `typeof … === "object"` не годится и значения не даёт.
    fn data(&self, id: &str) -> Option<&Value> {
        match self {
            Games::Object(map) => map.get(id),
            Games::Array(list) => list.get(array_index(id)? as usize),
        }
    }

    /// `Boolean(all.games[id])`.
    fn truthy(&self, id: &str) -> bool {
        match self {
            Games::Object(map) => match map.get(id) {
                Some(value) => js::truthy(value),
                None => OBJECT_PROTOTYPE_NAMES.contains(&id),
            },
            Games::Array(list) => match array_index(id) {
                Some(index) => list.get(index as usize).is_some_and(js::truthy),
                None => {
                    id == "length" && !list.is_empty()
                        || OBJECT_PROTOTYPE_NAMES.contains(&id)
                        || ARRAY_PROTOTYPE_NAMES.contains(&id)
                }
            },
        }
    }

    /// `all.games[id] = value`.
    fn set(&mut self, id: &str, value: Value) -> Result<(), RawError> {
        match self {
            Games::Object(map) => {
                map.insert(id.to_owned(), value);
            }
            Games::Array(list) => {
                if let Some(index) = array_index(id) {
                    if index > MAX_ARRAY_INDEX {
                        return Err(RawError::new("Invalid string length"));
                    }
                    let index = index as usize;
                    if list.len() <= index {
                        // Пропуски массива JSON.stringify пишет как null.
                        list.resize(index + 1, Value::Null);
                    }
                    list[index] = value;
                } else if id == "length" {
                    // Длиной массива может быть только число.
                    return Err(RawError::new("Invalid array length"));
                }
                // Прочие имена — свойства массива, в JSON они не попадают.
            }
        }
        Ok(())
    }

    /// `delete all.games[id]`.
    fn delete(&mut self, id: &str) -> Result<(), RawError> {
        match self {
            Games::Object(map) => {
                map.shift_remove(id);
            }
            Games::Array(list) => {
                if let Some(index) = array_index(id) {
                    if let Some(slot) = list.get_mut(index as usize) {
                        // Дыра в массиве: JSON.stringify пишет null.
                        *slot = Value::Null;
                    }
                } else if id == "length" {
                    return Err(RawError::new(
                        "Cannot delete property 'length' of [object Array]",
                    ));
                }
            }
        }
        Ok(())
    }

    fn into_value(self) -> Value {
        match self {
            Games::Object(map) => Value::Object(map),
            Games::Array(list) => Value::Array(list),
        }
    }
}

/// Хранилище настроек игр. Каждая операция читает файл целиком и записывает его целиком, поэтому
/// `save` и `reset` нельзя вызывать одновременно (в 0.4.7 всё идёт в одном потоке): вызывающий
/// ставит их в очередь.
pub struct GameSettingsStore {
    file: PathBuf,
    logger: Arc<dyn Logger>,
}

impl GameSettingsStore {
    pub fn new(file: impl Into<PathBuf>, logger: Arc<dyn Logger>) -> GameSettingsStore {
        GameSettingsStore {
            file: file.into(),
            logger,
        }
    }

    /// `GameSettingsStore.defaultFile(userDataDir)`.
    pub fn default_file(user_data_dir: &Path) -> PathBuf {
        user_data_dir.join(FILE_NAME)
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    /// `readAll()`: записи игр; файл не объект с `games` — пусто (и в журнал, если файл разобран).
    fn read_games(&self) -> Games {
        let raw = read_json_safe(&self.file);
        match raw.get("games") {
            Some(Value::Object(games)) => return Games::Object(games.clone()),
            Some(Value::Array(games)) => return Games::Array(games.clone()),
            _ => {}
        }
        if !raw.is_null() {
            self.logger.warn(&format!(
                "[game-settings] {FILE_NAME} повреждён — использую значения по умолчанию"
            ));
        }
        Games::Object(Map::new())
    }

    fn write(&self, games: Games) -> Result<(), RawError> {
        let mut all = Map::new();
        all.insert("version".into(), Value::from(FILE_VERSION));
        all.insert("games".into(), games.into_value());
        write_json_atomic(&self.file, &Value::Object(all))
    }

    /// `get(gameId)`: настройки игры; небезопасный идентификатор или нет записи — по умолчанию.
    pub fn get(&self, game_id: &str) -> Settings {
        if !is_safe_game_id(game_id) {
            return launch_args::default_settings();
        }
        let games = self.read_games();
        let sanitized = launch_args::sanitize_settings(games.data(game_id).unwrap_or(&Value::Null));
        if let Some(dropped) = &sanitized.dropped {
            self.logger.warn(&format!(
                "[game-settings] параметры запуска {game_id} не прошли проверку \
                 ({}) — поле очищено, переключатели сохранены",
                dropped.code
            ));
        }
        sanitized.settings
    }

    /// `save(gameId, rawSettings)`: нормализованные настройки записываются в файл.
    pub fn save(&self, game_id: &str, raw_settings: &Value) -> Result<Settings, StoreError> {
        if !is_safe_game_id(game_id) {
            return Err(ArgsError::new("INVALID_GAME_ID").into());
        }
        let settings = launch_args::normalize_settings(raw_settings)?;
        let mut games = self.read_games();
        games.set(game_id, settings.to_value())?;
        self.write(games)?;
        let arguments = launch_args::build_user_arguments(&settings.to_value(), &[]).join(" ");
        self.logger.info(&format!(
            "[game-settings] сохранены настройки {game_id}: аргументы [{}]",
            if arguments.is_empty() {
                "нет"
            } else {
                &arguments
            }
        ));
        Ok(settings)
    }

    /// `saveImported(gameId, rawSettings)`: настройки из чужого файла (см.
    /// [`launch_args::imported_settings`]).
    pub fn save_imported(
        &self,
        game_id: &str,
        raw_settings: &Value,
    ) -> Result<Settings, StoreError> {
        if !is_safe_game_id(game_id) {
            return Err(ArgsError::new("INVALID_GAME_ID").into());
        }
        self.save(
            game_id,
            &launch_args::imported_settings(raw_settings, &self.get(game_id)),
        )
    }

    /// `reset(gameId)`: запись игры удаляется (файл переписывается, только если запись была).
    pub fn reset(&self, game_id: &str) -> Result<Settings, StoreError> {
        if !is_safe_game_id(game_id) {
            return Err(ArgsError::new("INVALID_GAME_ID").into());
        }
        let mut games = self.read_games();
        if games.truthy(game_id) {
            games.delete(game_id)?;
            self.write(games)?;
        }
        self.logger
            .info(&format!("[game-settings] настройки {game_id} сброшены"));
        Ok(launch_args::default_settings())
    }

    /// `launchArgumentsFor(gameId, trustedArgs)`: аргументы игрока для запуска.
    pub fn launch_arguments_for(&self, game_id: &str, trusted_args: &[String]) -> Vec<String> {
        let settings = self.get(game_id);
        launch_args::build_user_arguments(&settings.to_value(), trusted_args)
    }
}
