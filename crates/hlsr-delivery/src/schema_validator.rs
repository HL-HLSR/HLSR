//! Проверка по схеме JSON (`schema-validator.js`): ajv 8 (`allErrors`, `strict: false`,
//! draft-07) и ajv-formats для двух схем доставки, `game-manifest` и `channel-manifest`.
//!
//! Крейт `jsonschema` не взят: без функций по умолчанию он добавляет к `hlsr` почти 3 МБ
//! (порог — 1 МБ), а `cargo deny` отвергает лицензию MIT-0 у его зависимости
//! `borrow-or-share`. Вместо него — ручная проверка, которая читает сами файлы схем
//! (`crates/hlsr-delivery/schemas/*.schema.json`; их же читают скрипты выпуска и JS 0.4.7) и повторяет ajv в том, что видно снаружи:
//! принят ли документ, какие ошибки и в каком порядке. Порядок — как у кода, который строит
//! ajv: сначала `$ref`, `const`, `enum`, `not`, `oneOf`, `allOf`, затем группы по типу данных
//! (число, строка, массив, объект); ошибка типа — до ключевых слов, если для типа в схеме
//! нет ключевых слов, и после них, если есть. Текст ошибки (`message`) совпадает с ajv.
//!
//! Поддержано то, что есть в двух схемах: `type`, `const`, `enum`, `not`, `oneOf`, `allOf`,
//! `$ref` на `#/definitions/…`, `minimum`, `maximum`, `minLength`, `maxLength`, `pattern`,
//! `format: "date-time"`, `minItems`, `maxItems`, `items` (одна схема), `required`,
//! `additionalProperties: false`, `properties`. Остальное при разборе схемы — ошибка, а не
//! молчаливый пропуск (ajv в `strict: false` его бы проигнорировал).
//!
//! Регулярные выражения схем (ajv компилирует их с флагом `u`) переведены в таблицу
//! [`PATTERNS`]: `\d` и `\s` JS — не `\d` и `\s` крейта `regex`, а `(?!…)` он не знает.
//! Сверка таблицы с `new RegExp(…, "u")` — на корпусе строк в фикстуре.

use hlsr_core::js;
use regex::Regex;
use serde_json::{Map, Number, Value};
use std::collections::HashMap;
use std::sync::OnceLock;

/// `schemas/game-manifest.schema.json`.
pub const GAME_MANIFEST_SCHEMA: &str = include_str!("../schemas/game-manifest.schema.json");
/// `schemas/channel-manifest.schema.json`.
pub const CHANNEL_MANIFEST_SCHEMA: &str = include_str!("../schemas/channel-manifest.schema.json");

/// Шаблон из схемы → то же выражение для крейта `regex`.
///
/// `\d` JS — только `0-9`; `\s` — `hlsr_core::js::WHITESPACE_CLASS` (пробелы Unicode и U+FEFF, без
/// U+0085); `^(?![A-Za-z]:)(?!/)[^\\:*?"<>|]+$`
/// без опережающих проверок: двоеточие в классе запрещено, поэтому «буква и двоеточие»
/// в начале отсеивается самим классом, остаётся запрет `/` в первом символе.
const PATTERNS: &[(&str, &str)] = &[
    (r"(^|/)\.\.(/|$)", r"(^|/)\.\.(/|$)"),
    (
        r#"^(?![A-Za-z]:)(?!/)[^\\:*?"<>|]+$"#,
        r#"^[^/\\:*?"<>|][^\\:*?"<>|]*$"#,
    ),
    (r"^[0-9a-f]{7,40}$", r"^[0-9a-f]{7,40}$"),
    (r"^[A-Za-z0-9+/=]{64,256}$", r"^[A-Za-z0-9+/=]{64,256}$"),
    (
        r#"^[^/\\:*?"<>|]+\.(7z|zip)$"#,
        r#"^[^/\\:*?"<>|]+\.(7z|zip)$"#,
    ),
    (r#"^[^/\\:*?"<>|]+\.json$"#, r#"^[^/\\:*?"<>|]+\.json$"#),
    (r"^[a-f0-9]{64}$", r"^[a-f0-9]{64}$"),
    (r"^[a-z0-9]+(-[a-z0-9]+)*$", r"^[a-z0-9]+(-[a-z0-9]+)*$"),
    (
        r"^[a-z0-9]+([._-][a-z0-9]+)*$",
        r"^[a-z0-9]+([._-][a-z0-9]+)*$",
    ),
    (r"^\d+\.\d+\.\d+$", r"^[0-9]+\.[0-9]+\.[0-9]+$"),
    (
        r"^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$",
        r"^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$",
    ),
    (
        r#"^http://(127\.0\.0\.1|localhost)(:\d+)?/[^\s"'<>]*$"#,
        "^http://(127\\.0\\.0\\.1|localhost)(:[0-9]+)?/[^SPACE\"'<>]*$",
    ),
    (
        r#"^https://[A-Za-z0-9.-]+(:\d+)?/[^\s"'<>]*$"#,
        "^https://[A-Za-z0-9.-]+(:[0-9]+)?/[^SPACE\"'<>]*$",
    ),
    (
        r"^https://disk\.yandex\.(ru|com)/d/[A-Za-z0-9_-]+$",
        r"^https://disk\.yandex\.(ru|com)/d/[A-Za-z0-9_-]+$",
    ),
];

/// Выражение из таблицы с подставленным пробелом JS.
fn rust_pattern(pattern: &str) -> Option<String> {
    PATTERNS
        .iter()
        .find(|(schema, _)| *schema == pattern)
        .map(|(_, rust)| rust.replace("SPACE", js::WHITESPACE_CLASS))
}

/// Подходит ли строка под шаблон схемы, как `new RegExp(pattern, "u").test(text)`.
/// `None` — шаблона нет в таблице.
pub fn pattern_matches(pattern: &str, text: &str) -> Option<bool> {
    let rust = rust_pattern(pattern)?;
    Some(Regex::new(&rust).expect("шаблон таблицы").is_match(text))
}

// ------------------------------------------------------------ date-time (ajv-formats)

/// Формат `date-time` из ajv-formats (режим `full`): дата `YYYY-MM-DD` и время с поясом,
/// разделённые одним `t`/`T` или пробельным символом; високосная секунда — по пересчёту в UTC.
pub fn is_date_time(text: &str) -> bool {
    // `str.split(/t|\s/i)`: ровно один разделитель.
    let is_separator = |c: char| js::canonicalize_ignore_case(c) == 'T' || js::is_whitespace(c);
    let mut parts = text.split(is_separator);
    let (Some(date), Some(time), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    is_date(date) && is_time(time)
}

fn digits(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// `date`: `^(\d\d\d\d)-(\d\d)-(\d\d)$`, месяц 1–12, день с учётом високосного года.
fn is_date(text: &str) -> bool {
    let b = text.as_bytes();
    if !text.is_ascii() || b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let (Some(year), Some(month), Some(day)) = (
        digits(&text[0..4]),
        digits(&text[5..7]),
        digits(&text[8..10]),
    ) else {
        return false;
    };
    const DAYS: [u32; 13] = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    (1..=12).contains(&month)
        && day >= 1
        && day
            <= if month == 2 && leap {
                29
            } else {
                DAYS[month as usize]
            }
}

/// `time` со строгим поясом: `^(\d\d):(\d\d):(\d\d(?:\.\d+)?)(z|([+-])(\d\d)(?::?(\d\d))?)?$`
/// (флаг `i`), пояс обязателен.
fn is_time(text: &str) -> bool {
    if !text.is_ascii() {
        return false;
    }
    let b = text.as_bytes();
    if b.len() < 8 || b[2] != b':' || b[5] != b':' {
        return false;
    }
    let (Some(hour), Some(minute)) = (digits(&text[0..2]), digits(&text[3..5])) else {
        return false;
    };
    if digits(&text[6..8]).is_none() {
        return false;
    }
    let mut end = 8;
    if b.get(end) == Some(&b'.') {
        let fraction = b[end + 1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        if fraction == 0 {
            return false;
        }
        end += 1 + fraction;
    }
    let second = js::to_number(&text[6..end]);
    let zone = &text[end..];
    let (zone_sign, zone_hour, zone_minute) = if zone.eq_ignore_ascii_case("z") {
        (1i64, 0i64, 0i64)
    } else {
        let mut chars = zone.chars();
        let sign = match chars.next() {
            Some('+') => 1,
            Some('-') => -1,
            // Пояса нет (или мусор): со строгим поясом это отказ.
            _ => return false,
        };
        let rest = chars.as_str();
        let Some(hours) = rest.get(0..2).and_then(digits) else {
            return false;
        };
        let rest = &rest[2..];
        let minutes = if rest.is_empty() {
            0
        } else {
            let rest = rest.strip_prefix(':').unwrap_or(rest);
            match (rest.len() == 2).then(|| digits(rest)).flatten() {
                Some(minutes) => i64::from(minutes),
                None => return false,
            }
        };
        (sign, i64::from(hours), minutes)
    };
    if zone_hour > 23 || zone_minute > 59 {
        return false;
    }
    let (hour, minute) = (i64::from(hour), i64::from(minute));
    if hour <= 23 && minute <= 59 && second < 60.0 {
        return true;
    }
    // Високосная секунда: по UTC это 23:59:60.
    let utc_minute = minute - zone_minute * zone_sign;
    let utc_hour = hour - zone_hour * zone_sign - i64::from(utc_minute < 0);
    (utc_hour == 23 || utc_hour == -1) && (utc_minute == 59 || utc_minute == -1) && second < 61.0
}

// ------------------------------------------------------------ ошибки

/// Ошибка ajv: путь к полю (`instancePath`), ключевое слово и текст.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaError {
    pub instance_path: String,
    pub keyword: &'static str,
    pub message: String,
}

impl SchemaError {
    /// `"<путь или «/»> <текст>"`, как `describeErrors`.
    pub fn describe(&self) -> String {
        let path = if self.instance_path.is_empty() {
            "/"
        } else {
            &self.instance_path
        };
        format!("{path} {}", self.message)
    }
}

/// `describeErrors(errors)`.
pub fn describe_errors(errors: &[SchemaError]) -> Vec<String> {
    errors.iter().map(SchemaError::describe).collect()
}

/// Схема не разобрана: незнакомое ключевое слово, шаблон вне таблицы, битая ссылка.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CompileError(pub String);

// ------------------------------------------------------------ разобранная схема

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonType {
    String,
    Number,
    Integer,
    Boolean,
    Null,
    Object,
    Array,
}

impl JsonType {
    fn parse(name: &str) -> Option<JsonType> {
        Some(match name {
            "string" => JsonType::String,
            "number" => JsonType::Number,
            "integer" => JsonType::Integer,
            "boolean" => JsonType::Boolean,
            "null" => JsonType::Null,
            "object" => JsonType::Object,
            "array" => JsonType::Array,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            JsonType::String => "string",
            JsonType::Number => "number",
            JsonType::Integer => "integer",
            JsonType::Boolean => "boolean",
            JsonType::Null => "null",
            JsonType::Object => "object",
            JsonType::Array => "array",
        }
    }

    fn matches(self, data: &Value) -> bool {
        match self {
            JsonType::String => data.is_string(),
            // `strict: false` отключает `strictNumbers`: бесконечность (`1e400`) — число и целое
            // (`Infinity % 1` — NaN, и `!(NaN)` истинно).
            JsonType::Number => data.is_number(),
            JsonType::Integer => {
                number_of(data).is_some_and(|n| n.is_infinite() || n.fract() == 0.0)
            }
            JsonType::Boolean => data.is_boolean(),
            JsonType::Null => data.is_null(),
            JsonType::Object => data.is_object(),
            JsonType::Array => data.is_array(),
        }
    }
}

/// Число JS: `Number` из JSON; за пределами `f64` (`1e400`) — бесконечность с тем же знаком.
fn number_value(n: &Number) -> f64 {
    n.as_f64().unwrap_or_else(|| {
        if n.to_string().starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        }
    })
}

fn number_of(data: &Value) -> Option<f64> {
    data.as_number().map(number_value)
}

/// `fast-deep-equal` (его использует ajv для `const` и `enum`).
fn deep_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => number_value(x) == number_value(y),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| deep_equal(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(key, value)| y.get(key).is_some_and(|other| deep_equal(value, other)))
        }
        _ => a == b,
    }
}

#[derive(Debug)]
enum Node {
    /// Булева схема: `true` — всё допустимо, `false` — ничего.
    Always(bool),
    Schema(Box<Compiled>),
}

#[derive(Debug, Default)]
struct Compiled {
    reference: Option<Node>,
    types: Vec<JsonType>,
    constant: Option<Value>,
    enumeration: Option<Vec<Value>>,
    not: Option<Node>,
    one_of: Option<Vec<Node>>,
    all_of: Option<Vec<Node>>,
    maximum: Option<Value>,
    minimum: Option<Value>,
    max_length: Option<Value>,
    min_length: Option<Value>,
    pattern: Option<(String, Regex)>,
    date_time: bool,
    max_items: Option<Value>,
    min_items: Option<Value>,
    items: Option<Node>,
    required: Vec<String>,
    no_additional: bool,
    properties: Vec<(String, Node)>,
}

impl Compiled {
    fn uses_number_group(&self) -> bool {
        self.maximum.is_some() || self.minimum.is_some() || self.date_time
    }

    fn uses_string_group(&self) -> bool {
        self.max_length.is_some()
            || self.min_length.is_some()
            || self.pattern.is_some()
            || self.date_time
    }

    fn uses_array_group(&self) -> bool {
        self.max_items.is_some() || self.min_items.is_some() || self.items.is_some()
    }

    fn uses_object_group(&self) -> bool {
        !self.required.is_empty() || self.no_additional || !self.properties.is_empty()
    }

    /// Есть ли в схеме правила, кроме `$ref` (`schemaHasRulesButRef`).
    fn has_rules_besides_ref(&self) -> bool {
        !self.types.is_empty()
            || self.constant.is_some()
            || self.enumeration.is_some()
            || self.not.is_some()
            || self.one_of.is_some()
            || self.all_of.is_some()
            || self.uses_number_group()
            || self.uses_string_group()
            || self.uses_array_group()
            || self.uses_object_group()
    }

    /// `schemaHasRulesForType`: у `integer`, `boolean` и `null` групп нет.
    fn has_rules_for_type(&self, kind: JsonType) -> bool {
        match kind {
            JsonType::Number => self.uses_number_group(),
            JsonType::String => self.uses_string_group(),
            JsonType::Array => self.uses_array_group(),
            JsonType::Object => self.uses_object_group(),
            _ => false,
        }
    }
}

/// Ключевые слова, которые разбор схемы читает как пояснения.
const ANNOTATIONS: &[&str] = &["$schema", "$id", "title", "description", "definitions"];

fn compile_node(schema: &Value, root: &Value, depth: usize) -> Result<Node, CompileError> {
    if depth > 32 {
        return Err(CompileError("схема вложена слишком глубоко".to_owned()));
    }
    let object = match schema {
        Value::Bool(value) => return Ok(Node::Always(*value)),
        Value::Object(object) => object,
        other => return Err(CompileError(format!("схема не объект: {other}"))),
    };
    let mut compiled = Compiled::default();
    let sub = |value: &Value| compile_node(value, root, depth + 1);
    let sub_list = |value: &Value| -> Result<Vec<Node>, CompileError> {
        value
            .as_array()
            .ok_or_else(|| CompileError("ожидался массив схем".to_owned()))?
            .iter()
            .map(sub)
            .collect()
    };
    let size = |keyword: &str, value: &Value| -> Result<Value, CompileError> {
        if value.is_number() {
            Ok(value.clone())
        } else {
            Err(CompileError(format!("{keyword}: ожидалось число")))
        }
    };
    for (keyword, value) in object {
        match keyword.as_str() {
            key if ANNOTATIONS.contains(&key) => {}
            "$ref" => {
                let pointer = value
                    .as_str()
                    .and_then(|text| text.strip_prefix('#'))
                    .ok_or_else(|| CompileError(format!("$ref: {value}")))?;
                let target = root
                    .pointer(pointer)
                    .ok_or_else(|| CompileError(format!("$ref не найден: {pointer}")))?;
                compiled.reference = Some(sub(target)?);
            }
            "type" => {
                let names: Vec<&Value> = match value {
                    Value::Array(items) => items.iter().collect(),
                    single => vec![single],
                };
                for name in names {
                    let kind = name
                        .as_str()
                        .and_then(JsonType::parse)
                        .ok_or_else(|| CompileError(format!("type: {name}")))?;
                    compiled.types.push(kind);
                }
            }
            "const" => compiled.constant = Some(value.clone()),
            "enum" => {
                compiled.enumeration = Some(
                    value
                        .as_array()
                        .ok_or_else(|| CompileError("enum: ожидался массив".to_owned()))?
                        .clone(),
                );
            }
            "not" => compiled.not = Some(sub(value)?),
            "oneOf" => compiled.one_of = Some(sub_list(value)?),
            "allOf" => compiled.all_of = Some(sub_list(value)?),
            "maximum" => compiled.maximum = Some(size(keyword, value)?),
            "minimum" => compiled.minimum = Some(size(keyword, value)?),
            "maxLength" => compiled.max_length = Some(size(keyword, value)?),
            "minLength" => compiled.min_length = Some(size(keyword, value)?),
            "maxItems" => compiled.max_items = Some(size(keyword, value)?),
            "minItems" => compiled.min_items = Some(size(keyword, value)?),
            "pattern" => {
                let text = value
                    .as_str()
                    .ok_or_else(|| CompileError("pattern: ожидалась строка".to_owned()))?;
                let rust = rust_pattern(text).ok_or_else(|| {
                    CompileError(format!("шаблона нет в таблице PATTERNS: {text}"))
                })?;
                let regex = Regex::new(&rust).map_err(|e| CompileError(e.to_string()))?;
                compiled.pattern = Some((text.to_owned(), regex));
            }
            "format" => match value.as_str() {
                Some("date-time") => compiled.date_time = true,
                other => return Err(CompileError(format!("format: {other:?}"))),
            },
            "items" => compiled.items = Some(sub(value)?),
            "required" => {
                compiled.required = value
                    .as_array()
                    .ok_or_else(|| CompileError("required: ожидался массив".to_owned()))?
                    .iter()
                    .map(|name| {
                        name.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| CompileError("required: ожидались строки".to_owned()))
                    })
                    .collect::<Result<_, _>>()?;
            }
            "additionalProperties" => match value {
                Value::Bool(false) => compiled.no_additional = true,
                Value::Bool(true) => {}
                _ => {
                    return Err(CompileError(
                        "additionalProperties: поддержано только true и false".to_owned(),
                    ));
                }
            },
            "properties" => {
                let properties = value
                    .as_object()
                    .ok_or_else(|| CompileError("properties: ожидался объект".to_owned()))?;
                for (name, schema) in properties {
                    compiled.properties.push((name.clone(), sub(schema)?));
                }
            }
            other => {
                return Err(CompileError(format!(
                    "ключевое слово не поддержано: {other}"
                )));
            }
        }
    }
    Ok(Node::Schema(Box::new(compiled)))
}

/// Проверка документа по схеме: результат `compileSchema(schema)`.
#[derive(Debug)]
pub struct Validator {
    root: Node,
}

/// `compileSchema(schema)`.
pub fn compile_schema(schema: &Value) -> Result<Validator, CompileError> {
    Ok(Validator {
        root: compile_node(schema, schema, 0)?,
    })
}

impl Validator {
    /// Ошибки документа в порядке ajv; пусто — документ принят.
    pub fn validate(&self, data: &Value) -> Vec<SchemaError> {
        let mut errors = Vec::new();
        eval(&self.root, data, "", &mut errors);
        errors
    }

    /// Принят ли документ.
    pub fn is_valid(&self, data: &Value) -> bool {
        self.validate(data).is_empty()
    }
}

/// Проверка по схеме игры (`ManifestService`).
pub fn game_manifest_validator() -> &'static Validator {
    static VALIDATOR: OnceLock<Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| embedded_validator(GAME_MANIFEST_SCHEMA))
}

/// Проверка по схеме канала (`ChannelService`).
pub fn channel_manifest_validator() -> &'static Validator {
    static VALIDATOR: OnceLock<Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| embedded_validator(CHANNEL_MANIFEST_SCHEMA))
}

fn embedded_validator(text: &str) -> Validator {
    let schema = js_parse(text).expect("вшитая схема — JSON");
    compile_schema(&schema).expect("вшитая схема разбирается")
}

fn js_parse(text: &str) -> Result<Value, serde_json::Error> {
    hlsr_core::json_text::parse_js_json(text)
}

// ------------------------------------------------------------ выполнение

fn push(errors: &mut Vec<SchemaError>, path: &str, keyword: &'static str, message: String) {
    errors.push(SchemaError {
        instance_path: path.to_owned(),
        keyword,
        message,
    });
}

fn child_path(path: &str, key: &str) -> String {
    format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"))
}

fn eval(node: &Node, data: &Value, path: &str, errors: &mut Vec<SchemaError>) {
    match node {
        Node::Always(true) => {}
        Node::Always(false) => push(
            errors,
            path,
            "false schema",
            "boolean schema is false".into(),
        ),
        Node::Schema(schema) => eval_schema(schema, data, path, errors),
    }
}

fn valid(node: &Node, data: &Value, path: &str) -> bool {
    let mut errors = Vec::new();
    eval(node, data, path, &mut errors);
    errors.is_empty()
}

fn type_error(schema: &Compiled, path: &str, errors: &mut Vec<SchemaError>) {
    let names: Vec<&str> = schema.types.iter().map(|kind| kind.name()).collect();
    push(errors, path, "type", format!("must be {}", names.join(",")));
}

fn eval_schema(s: &Compiled, data: &Value, path: &str, errors: &mut Vec<SchemaError>) {
    // `$ref` без соседей — только ссылка.
    if let (Some(reference), false) = (&s.reference, s.has_rules_besides_ref()) {
        eval(reference, data, path, errors);
        return;
    }

    // Тип проверяется до ключевых слов, если для него в схеме нет ключевых слов.
    let check_types =
        !s.types.is_empty() && !(s.types.len() == 1 && s.has_rules_for_type(s.types[0]));
    if check_types && !s.types.iter().any(|kind| kind.matches(data)) {
        type_error(s, path, errors);
    }
    let type_errors = !check_types;
    let only_type = |kind: JsonType| s.types.len() == 1 && s.types[0] == kind && type_errors;

    // Ключевые слова без привязки к типу.
    if let Some(reference) = &s.reference {
        eval(reference, data, path, errors);
    }
    if let Some(constant) = &s.constant
        && !deep_equal(data, constant)
    {
        push(errors, path, "const", "must be equal to constant".into());
    }
    if let Some(allowed) = &s.enumeration
        && !allowed.iter().any(|value| deep_equal(data, value))
    {
        push(
            errors,
            path,
            "enum",
            "must be equal to one of the allowed values".into(),
        );
    }
    if let Some(not) = &s.not
        && valid(not, data, path)
    {
        push(errors, path, "not", "must NOT be valid".into());
    }
    if let Some(branches) = &s.one_of {
        let before = errors.len();
        let passing = branches
            .iter()
            .filter(|branch| {
                let count = errors.len();
                eval(branch, data, path, errors);
                errors.len() == count
            })
            .count();
        if passing == 1 {
            errors.truncate(before);
        } else {
            push(
                errors,
                path,
                "oneOf",
                "must match exactly one schema in oneOf".into(),
            );
        }
    }
    if let Some(branches) = &s.all_of {
        for branch in branches {
            eval(branch, data, path, errors);
        }
    }

    // Число.
    if s.uses_number_group() {
        if data.is_number() {
            let n = number_of(data).unwrap_or(f64::NAN);
            if let Some(maximum) = &s.maximum
                && n > number_of(maximum).unwrap_or(f64::NAN)
            {
                push(
                    errors,
                    path,
                    "maximum",
                    format!("must be <= {}", js::to_string(maximum)),
                );
            }
            if let Some(minimum) = &s.minimum
                && n < number_of(minimum).unwrap_or(f64::NAN)
            {
                push(
                    errors,
                    path,
                    "minimum",
                    format!("must be >= {}", js::to_string(minimum)),
                );
            }
        } else if only_type(JsonType::Number) {
            type_error(s, path, errors);
        }
    }

    // Строка.
    if s.uses_string_group() {
        if let Some(text) = data.as_str() {
            let length = text.chars().count() as f64;
            if let Some(maximum) = &s.max_length
                && length > number_of(maximum).unwrap_or(f64::NAN)
            {
                push(
                    errors,
                    path,
                    "maxLength",
                    format!(
                        "must NOT have more than {} characters",
                        js::to_string(maximum)
                    ),
                );
            }
            if let Some(minimum) = &s.min_length
                && length < number_of(minimum).unwrap_or(f64::NAN)
            {
                push(
                    errors,
                    path,
                    "minLength",
                    format!(
                        "must NOT have fewer than {} characters",
                        js::to_string(minimum)
                    ),
                );
            }
            if let Some((pattern, regex)) = &s.pattern
                && !regex.is_match(text)
            {
                push(
                    errors,
                    path,
                    "pattern",
                    format!("must match pattern \"{pattern}\""),
                );
            }
            if s.date_time && !is_date_time(text) {
                push(
                    errors,
                    path,
                    "format",
                    "must match format \"date-time\"".into(),
                );
            }
        } else if only_type(JsonType::String) {
            type_error(s, path, errors);
        }
    }

    // Массив.
    if s.uses_array_group() {
        if let Some(items) = data.as_array() {
            let length = items.len() as f64;
            if let Some(maximum) = &s.max_items
                && length > number_of(maximum).unwrap_or(f64::NAN)
            {
                push(
                    errors,
                    path,
                    "maxItems",
                    format!("must NOT have more than {} items", js::to_string(maximum)),
                );
            }
            if let Some(minimum) = &s.min_items
                && length < number_of(minimum).unwrap_or(f64::NAN)
            {
                push(
                    errors,
                    path,
                    "minItems",
                    format!("must NOT have fewer than {} items", js::to_string(minimum)),
                );
            }
            if let Some(item_schema) = &s.items {
                for (index, item) in items.iter().enumerate() {
                    eval(item_schema, item, &format!("{path}/{index}"), errors);
                }
            }
        } else if only_type(JsonType::Array) {
            type_error(s, path, errors);
        }
    }

    // Объект.
    if s.uses_object_group() {
        if let Some(object) = data.as_object() {
            eval_object(s, object, path, errors);
        } else if only_type(JsonType::Object) {
            type_error(s, path, errors);
        }
    }
}

fn eval_object(
    s: &Compiled,
    object: &Map<String, Value>,
    path: &str,
    errors: &mut Vec<SchemaError>,
) {
    for name in &s.required {
        if !object.contains_key(name) {
            push(
                errors,
                path,
                "required",
                format!("must have required property '{name}'"),
            );
        }
    }
    if s.no_additional {
        let known: HashMap<&str, ()> = s
            .properties
            .iter()
            .map(|(name, _)| (name.as_str(), ()))
            .collect();
        for (key, _) in hlsr_core::json_text::js_key_order(object) {
            if !known.contains_key(key.as_str()) {
                push(
                    errors,
                    path,
                    "additionalProperties",
                    "must NOT have additional properties".into(),
                );
            }
        }
    }
    for (name, schema) in &s.properties {
        if let Some(value) = object.get(name) {
            eval(schema, value, &child_path(path, name), errors);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn embedded_schemas_compile() {
        assert!(!game_manifest_validator().is_valid(&json!(null)));
        assert!(!channel_manifest_validator().is_valid(&json!([])));
    }

    #[test]
    fn number_beyond_f64_is_infinity_and_still_a_number() {
        // `1e400` в JS — `Infinity`: ajv без `strictNumbers` считает его целым числом.
        let value = js_parse("1e400").unwrap();
        assert_eq!(number_of(&value), Some(f64::INFINITY));
        assert!(JsonType::Integer.matches(&value) && JsonType::Number.matches(&value));
        let negative = js_parse("-1e400").unwrap();
        assert_eq!(number_of(&negative), Some(f64::NEG_INFINITY));
        assert!(!JsonType::Integer.matches(&js_parse("1.5").unwrap()));
    }

    #[test]
    fn date_time_edges() {
        assert!(is_date_time("2026-10-12T16:14:25.349Z"));
        assert!(is_date_time("2026-10-12 16:14:25z"));
        assert!(!is_date_time("2026-10-12T16:14:25"));
        assert!(!is_date_time("2026-02-29T00:00:00Z"));
        assert!(is_date_time("2026-12-31T23:59:60Z"));
        assert!(!is_date_time("2026-12-31T22:59:60Z"));
    }
}
