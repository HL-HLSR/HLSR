//! Заметки о выпуске (`release-notes.js`): текст заметок манифеста разбирается на пункты и кладётся в
//! `game-release-notes.json` рядом с данными лаунчера: `{ version: 1, games: { <игра>: { <версия>:
//! { items, raw, parser, ts } } } }`, не больше 10 версий на игру (самые старые по `ts` уходят),
//! не больше 40 пунктов по 300 знаков. Запись — `writeJsonAtomic` (два пробела и `\n`).
//!
//! Файл читается как есть: любой корень, кроме объекта с `version: 1`, — пустое хранилище; записи
//! с чужими типами не ломают чтение. Порядок ключей — как у объектов JS: ключи-индексы первыми
//! (`"9"` и `"10"` раньше `"1.0.0"`), он влияет и на файл, и на то, какие версии уйдут при равных `ts`.
//!
//! Чего нет против 0.4.7: массив на месте карты версий игры и массив `games` разобраны как у JS
//! только для чтения; запись в них (`capture`) не меняет файл, как и у 0.4.7 для ключей, не
//! являющихся индексами, а для ключей-индексов (`games: []` и игра `"0"`) 0.4.7 дописала бы
//! элемент массива — этот случай не повторяется.

use crate::atomic::write_json_atomic;
use crate::errors::RawError;
use crate::js_value as jv;
use crate::ports::{Logger, TracingLogger};
use crate::timing::{Clock, system_clock};
use hlsr_core::js::collate::{Collator, Sensitivity, default_locale};
use hlsr_core::js::sort::sort_by_comparator;
use hlsr_core::js::text::utf16_len;
use hlsr_core::js::{self, slice_utf16};
use hlsr_core::json_store::read_json_safe;
use hlsr_core::json_text::js_key_order;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::cmp::Ordering;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

const FILE_VERSION: u64 = 1;
pub const FILE_NAME: &str = "game-release-notes.json";

/// Версия разбора заметок: записи старее разбираются заново при чтении.
const PARSER_VERSION: f64 = 2.0;

pub const MAX_ITEMS: usize = 40;
pub const MAX_ITEM_LENGTH: usize = 300;
const MAX_RAW_LENGTH: usize = 2000;
const MAX_VERSIONS_PER_GAME: usize = 10;

/// Предложение короче этого сливается с предыдущим.
const MIN_SENTENCE: usize = 25;

// Регулярные выражения JS — на `regress` (`\s` JS шире и уже `\s` Unicode: U+FEFF есть, U+0085 нет).
static LINE_BREAK: LazyLock<regress::Regex> =
    LazyLock::new(|| regress::Regex::new(r"\r?\n").unwrap());
static TAG: LazyLock<regress::Regex> = LazyLock::new(|| regress::Regex::new(r"<[^>]*>").unwrap());
static LEADING_BULLET: LazyLock<regress::Regex> =
    LazyLock::new(|| regress::Regex::new(r"^\s*[-*•—]\s*").unwrap());
static SPACES: LazyLock<regress::Regex> = LazyLock::new(|| regress::Regex::new(r"\s+").unwrap());
static SENTENCE_BREAK: LazyLock<regress::Regex> =
    LazyLock::new(|| regress::Regex::new(r#"(?<=[.!?…])\s+(?=[«"A-ZА-ЯЁ0-9])"#).unwrap());
/// `/^[0-9A-Za-z.\-+]{1,32}$/`.
static VERSION_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9A-Za-z.+-]{1,32}$").unwrap());
/// `/^[a-z0-9]+([._-][a-z0-9]+)*$/`.
static GAME_ID_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9]+([._-][a-z0-9]+)*$").unwrap());

fn is_version(value: &str) -> bool {
    VERSION_PATTERN.is_match(value)
}

fn is_game_id(value: &str) -> bool {
    value.len() <= 64 && GAME_ID_PATTERN.is_match(value)
}

/// Индекс массива JS: каноническая запись целого от 0 до 2³² − 2.
fn array_index(key: &str) -> Option<usize> {
    if key.is_empty() || (key.len() > 1 && key.starts_with('0')) {
        return None;
    }
    if !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    key.parse::<u32>()
        .ok()
        .filter(|index| *index != u32::MAX)
        .map(|index| index as usize)
}

// ----------------------------------------------------------------------- свойства значений JSON

/// `Object.keys(value)` для объекта и массива (порядок JS); для прочих значений — пусто.
fn own_keys(value: &Value) -> Vec<String> {
    match value {
        Value::Object(map) => js_key_order(map)
            .into_iter()
            .map(|(key, _)| key.clone())
            .collect(),
        Value::Array(list) => (0..list.len()).map(|index| index.to_string()).collect(),
        _ => Vec::new(),
    }
}

/// `value[key]`, если свойство есть у самого значения (`hasOwnProperty`): поле объекта или
/// элемент массива; у остальных значений `None`.
fn own_get<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => map.get(key),
        Value::Array(list) => array_index(key).and_then(|index| list.get(index)),
        _ => None,
    }
}

// ----------------------------------------------------------------------- разбор заметок

/// `str.split(regex)` для регулярного выражения без захватов и пустых совпадений.
fn split<'a>(pattern: &regress::Regex, text: &'a str) -> Vec<&'a str> {
    let mut pieces = Vec::new();
    let mut last = 0;
    for found in pattern.find_iter(text) {
        pieces.push(&text[last..found.start()]);
        last = found.end();
    }
    pieces.push(&text[last..]);
    pieces
}

/// `str.replace(regex_g, with)`.
fn replace_all(pattern: &regress::Regex, text: &str, with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for found in pattern.find_iter(text) {
        out.push_str(&text[last..found.start()]);
        out.push_str(with);
        last = found.end();
    }
    out.push_str(&text[last..]);
    out
}

/// Одна строка заметки без разметки: теги — пробел, четыре сущности, маркер пункта в начале,
/// пробелы свёрнуты, края обрезаны.
fn clean_line(line: &str) -> String {
    let text = replace_all(&TAG, line, " ");
    let text = text
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    let text = match LEADING_BULLET.find(&text) {
        Some(found) => text[found.end()..].to_owned(),
        None => text,
    };
    js::trim(&replace_all(&SPACES, &text, " ")).to_owned()
}

/// Пункт длиннее 300 знаков обрезается по последнему пробелу (если он дальше середины) и получает `…`.
fn shorten(part: &str) -> String {
    let units: Vec<u16> = part.encode_utf16().collect();
    if units.len() <= MAX_ITEM_LENGTH {
        return part.to_owned();
    }
    let head = &units[..MAX_ITEM_LENGTH - 1];
    let cut = match head.iter().rposition(|&unit| unit == 0x20) {
        Some(space) if space > MAX_ITEM_LENGTH / 2 => &head[..space],
        _ => head,
    };
    format!("{}…", js::trim(&String::from_utf16_lossy(cut)))
}

/// `toBullets(line)`: строка делится на предложения, короткие предложения (до 25 знаков)
/// сливаются с предыдущим, пока пункт не длиннее 300 знаков.
fn to_bullets(line: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();

    for piece in split(&SENTENCE_BREAK, line) {
        if current.is_empty() {
            current = piece.to_owned();
            continue;
        }

        if utf16_len(piece) < MIN_SENTENCE
            && utf16_len(&current) + 1 + utf16_len(piece) <= MAX_ITEM_LENGTH
        {
            current.push(' ');
            current.push_str(piece);
            continue;
        }

        parts.push(std::mem::replace(&mut current, piece.to_owned()));
    }

    if !current.is_empty() {
        parts.push(current);
    }

    parts.iter().map(|part| shorten(part)).collect()
}

/// `toItems(raw)`: пункты заметки — не больше 40, каждый не длиннее 300 знаков. Берутся первые
/// 2000 знаков (единиц UTF-16). Не строка в 0.4.7 даёт пустой список — это проверяет `capture`.
pub fn to_items(raw: &str) -> Vec<String> {
    let text = slice_utf16(raw, MAX_RAW_LENGTH);

    split(&LINE_BREAK, &text)
        .into_iter()
        .map(clean_line)
        .filter(|line| !line.is_empty())
        .flat_map(|line| to_bullets(&line))
        .take(MAX_ITEMS)
        .collect()
}

/// `parseInt(text, 10)`: ведущие цифры, иначе `NaN`.
fn parse_int(text: Option<&str>) -> f64 {
    let digits: String = text
        .unwrap_or_default()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        f64::NAN
    } else {
        js::to_number(&digits)
    }
}

/// `compareVersions(a, b)`: по частям через `. - +`; части-числа — числом, остальные —
/// `localeCompare`. Знак результата, как у `Array.sort`.
fn compare_versions(a: &str, b: &str, collator: &Collator) -> f64 {
    let left: Vec<&str> = a.split(['.', '-', '+']).collect();
    let right: Vec<&str> = b.split(['.', '-', '+']).collect();

    for i in 0..left.len().max(right.len()) {
        let (l, r) = (
            parse_int(left.get(i).copied()),
            parse_int(right.get(i).copied()),
        );

        if l.is_nan() || r.is_nan() {
            match collator.compare(
                left.get(i).copied().unwrap_or_default(),
                right.get(i).copied().unwrap_or_default(),
            ) {
                Ordering::Less => return -1.0,
                Ordering::Greater => return 1.0,
                Ordering::Equal => continue,
            }
        }

        if l != r {
            return l - r;
        }
    }

    0.0
}

// ----------------------------------------------------------------------- хранилище

/// Параметры `new ReleaseNotesStore(options)`.
#[derive(Clone, Default)]
pub struct StoreOptions {
    /// `dir`: каталог данных лаунчера; пустой — ошибка.
    pub dir: PathBuf,
    /// `logger`: по умолчанию — журнал приложения.
    pub logger: Option<Arc<dyn Logger>>,
    /// Часы (`Date.now()`); для тестов.
    pub clock: Option<Clock>,
}

/// Заметки одной версии игры (`get`).
#[derive(Debug, Clone, PartialEq)]
pub struct Notes {
    pub game_id: String,
    pub version: String,
    pub items: Vec<String>,
}

/// Версия в `list`: пункты и метка времени записи (`ts` как в файле).
#[derive(Debug, Clone, PartialEq)]
pub struct VersionNotes {
    pub version: String,
    pub items: Vec<String>,
    pub ts: Value,
}

/// Игра в `list`: версии от новых к старым.
#[derive(Debug, Clone, PartialEq)]
pub struct GameNotes {
    pub game_id: String,
    pub versions: Vec<VersionNotes>,
}

pub struct ReleaseNotesStore {
    file: PathBuf,
    logger: Arc<dyn Logger>,
    clock: Clock,
}

/// `TypeError` V8 без кода: `error.code` у него нет.
fn type_error(message: String) -> RawError {
    RawError::new(message)
}

/// `(value.parser || 1)`, приведённое к числу.
fn parser_of(entry: &Value) -> f64 {
    match jv::prop(entry, "parser").filter(|parser| js::truthy(parser)) {
        Some(parser) => jv::to_number(parser),
        None => 1.0,
    }
}

/// `value.ts || fallback`.
fn ts_or(entry: &Value, fallback: Value) -> Value {
    jv::prop(entry, "ts")
        .filter(|ts| js::truthy(ts))
        .cloned()
        .unwrap_or(fallback)
}

impl ReleaseNotesStore {
    /// `new ReleaseNotesStore({ dir, logger })`.
    pub fn new(options: StoreOptions) -> Result<ReleaseNotesStore, RawError> {
        if options.dir.as_os_str().is_empty() {
            return Err(RawError::new("ReleaseNotesStore: не задан каталог"));
        }
        Ok(ReleaseNotesStore {
            file: options.dir.join(FILE_NAME),
            logger: options.logger.unwrap_or_else(|| Arc::new(TracingLogger)),
            clock: options.clock.unwrap_or_else(system_clock),
        })
    }

    /// `readAll()`: содержимое файла или пустое хранилище; `games` всегда объект или массив.
    pub fn read_all(&self) -> Value {
        let mut raw = read_json_safe(&self.file);
        let is_store = raw
            .as_object()
            .and_then(|object| object.get("version"))
            .and_then(jv::number)
            .is_some_and(|version| version == FILE_VERSION as f64);
        if !is_store {
            return json!({"version": FILE_VERSION, "games": {}});
        }
        if let Value::Object(object) = &mut raw {
            let games_ok = object
                .get("games")
                .is_some_and(|games| games.is_object() || games.is_array());
            if !games_ok {
                object.insert("games".to_owned(), json!({}));
            }
        }
        raw
    }

    /// `capture(gameId, version, rawNotes)`: разобрать и записать. `false` — нечего записывать или
    /// запись не удалась (в журнале предупреждение); `true` — записано или уже было записано
    /// разбором текущей версии. `Err` — `TypeError` 0.4.7 на испорченном файле.
    pub fn capture(
        &self,
        game_id: &str,
        version: &str,
        raw_notes: &Value,
    ) -> Result<bool, RawError> {
        if !is_game_id(game_id) || !is_version(version) {
            return Ok(false);
        }

        let Some(raw_notes) = raw_notes.as_str() else {
            return Ok(false);
        };
        let items = to_items(raw_notes);
        if items.is_empty() {
            return Ok(false);
        }

        let mut all = self.read_all();
        if let Some(Value::Object(games)) = all.get_mut("games") {
            let game = games.entry(game_id).or_insert_with(|| json!({}));
            if self.store_version(game, version, raw_notes, items)? {
                return Ok(true);
            }
        }

        if let Err(error) = write_json_atomic(&self.file, &all) {
            self.logger
                .warn(&format!("[release-notes] не сохранено: {}", error.message));
            return Ok(false);
        }

        Ok(true)
    }

    /// Положить версию в карту версий игры и убрать лишние. `Ok(true)` — запись уже была
    /// разобрана текущим разбором, писать файл не нужно.
    fn store_version(
        &self,
        game: &mut Value,
        version: &str,
        raw_notes: &str,
        items: Vec<String>,
    ) -> Result<bool, RawError> {
        let versions = match game {
            Value::Object(versions) => versions,
            Value::Null => {
                return Err(type_error(
                    "Cannot convert undefined or null to object".to_owned(),
                ));
            }
            // Массив на месте карты версий: свойство-не-индекс в файл не попадает (см. заголовок).
            Value::Array(_) => return Ok(false),
            primitive => return Err(assign_error(primitive, version)),
        };

        let existing = versions.get(version);
        if existing.is_some_and(|entry| js::truthy(entry) && parser_of(entry) >= PARSER_VERSION) {
            return Ok(true);
        }

        let ts = match existing {
            Some(entry) => ts_or(entry, json!((self.clock)())),
            None => json!((self.clock)()),
        };
        versions.insert(
            version.to_owned(),
            json!({
                "items": items,
                "raw": slice_utf16(raw_notes, MAX_RAW_LENGTH),
                "parser": PARSER_VERSION as u64,
                "ts": ts,
            }),
        );

        prune(versions)?;
        Ok(false)
    }

    /// `get(gameId, version)`.
    pub fn get(&self, game_id: &str, version: &str) -> Option<Notes> {
        if !is_game_id(game_id) || !is_version(version) {
            return None;
        }
        let all = self.read_all();
        notes_in(&all, game_id, version).map(|items| Notes {
            game_id: game_id.to_owned(),
            version: version.to_owned(),
            items,
        })
    }

    /// `has(gameId, version)`.
    pub fn has(&self, game_id: &str, version: &str) -> bool {
        self.get(game_id, version).is_some()
    }

    /// `list()` на языке по умолчанию процесса (`localeCompare` без языка).
    pub fn list(&self) -> Vec<GameNotes> {
        self.list_in(&default_locale())
    }

    /// `list()` на заданном языке: игры в порядке файла, версии — от новых к старым (сравнение
    /// версий, затем `ts` от новых к старым).
    pub fn list_in(&self, locale: &str) -> Vec<GameNotes> {
        let all = self.read_all();
        let collator = Collator::new(locale, Sensitivity::Variant);
        let games = &all["games"];
        let mut out = Vec::new();

        for game_id in own_keys(games) {
            if !is_game_id(&game_id) {
                continue;
            }

            let Some(stored) = own_get(games, &game_id) else {
                continue;
            };
            if !js::truthy(stored) || !(stored.is_object() || stored.is_array()) {
                continue;
            }

            let mut versions: Vec<VersionNotes> = own_keys(stored)
                .into_iter()
                .filter(|version| is_version(version))
                .filter_map(|version| {
                    let items = notes_in(&all, &game_id, &version)?;
                    let ts =
                        own_get(stored, &version).map_or(json!(0), |entry| ts_or(entry, json!(0)));
                    Some(VersionNotes { version, items, ts })
                })
                .collect();

            sort_by_comparator(&mut versions, |a, b| {
                let by_version = compare_versions(&b.version, &a.version, &collator);
                if by_version != 0.0 && !by_version.is_nan() {
                    by_version
                } else {
                    jv::to_number(&b.ts) - jv::to_number(&a.ts)
                }
            });

            if !versions.is_empty() {
                out.push(GameNotes { game_id, versions });
            }
        }

        out
    }
}

/// Присвоение свойства примитиву (`"use strict"`): `TypeError`.
fn assign_error(primitive: &Value, key: &str) -> RawError {
    let (kind, shown) = match primitive {
        Value::String(text) => ("string", text.clone()),
        Value::Bool(flag) => ("boolean", flag.to_string()),
        other => ("number", js::to_string(other)),
    };
    // Элементы строки и её `length` только для чтения; других свойств у неё не создать.
    let read_only = matches!(primitive, Value::String(text)
        if key == "length" || array_index(key).is_some_and(|index| index < utf16_len(text)));
    type_error(if read_only {
        format!("Cannot assign to read only property '{key}' of {kind} '{shown}'")
    } else {
        format!("Cannot create property '{key}' on {kind} '{shown}'")
    })
}

/// Больше 10 версий — самые старые по `ts` убираются (при равных — по порядку ключей).
fn prune(versions: &mut Map<String, Value>) -> Result<(), RawError> {
    let keys: Vec<String> = js_key_order(versions)
        .into_iter()
        .map(|(key, _)| key.clone())
        .collect();
    if keys.len() <= MAX_VERSIONS_PER_GAME {
        return Ok(());
    }

    // `game[a].ts || 0` для каждой версии; `null` вместо записи — `TypeError`, его бросит
    // первое же сравнение (в сортировке участвует каждый элемент).
    let mut stamps = Vec::with_capacity(keys.len());
    for key in &keys {
        let entry = &versions[key.as_str()];
        if entry.is_null() {
            return Err(type_error(
                "Cannot read properties of null (reading 'ts')".to_owned(),
            ));
        }
        stamps.push(ts_or(entry, json!(0)));
    }

    let mut order: Vec<usize> = (0..keys.len()).collect();
    sort_by_comparator(&mut order, |&a, &b| {
        jv::to_number(&stamps[a]) - jv::to_number(&stamps[b])
    });

    for &index in order.iter().take(keys.len() - MAX_VERSIONS_PER_GAME) {
        versions.shift_remove(&keys[index]);
    }
    Ok(())
}

/// Пункты версии из прочитанного файла, как их отдаёт `get`.
fn notes_in(all: &Value, game_id: &str, version: &str) -> Option<Vec<String>> {
    let game = own_get(&all["games"], game_id).filter(|game| js::truthy(game))?;
    let entry = own_get(game, version).filter(|entry| js::truthy(entry))?;
    let stored = jv::array(jv::prop(entry, "items"))?;

    // Запись прежнего разбора пересобирается из `raw`, если он есть.
    let legacy_raw = jv::prop(entry, "raw")
        .and_then(Value::as_str)
        .filter(|raw| !raw.is_empty() && parser_of(entry) < PARSER_VERSION);
    let source: Vec<String> = match legacy_raw {
        Some(raw) => to_items(raw),
        None => stored
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
    };

    let items: Vec<String> = source
        .iter()
        .filter(|item| !js::trim(item).is_empty())
        .take(MAX_ITEMS)
        .map(|item| slice_utf16(item, MAX_ITEM_LENGTH))
        .collect();

    (!items.is_empty()).then_some(items)
}
