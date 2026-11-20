//! Разбор KeyValues (VDF) — `libraryfolders.vdf`, `loginusers.vdf`, `appmanifest_*.acf`,
//! `localconfig.vdf` (`vdf.js`). Разбор дословно как в 0.4.7: комментарии `//`, условные метки
//! `[$WIN32]` пропускаются, строки в кавычках с `\n \t \r \" \\`, слова без кавычек, вложенность
//! не глубже 32. Ошибка — `LAUNCH_CONFIG_INVALID` с номером строки.

use hlsr_core::js;
use hlsr_core::js::object::JsObject;
use hlsr_delivery::errors::{Code, DeliveryError};
use serde_json::{Map, Value};

/// `MAX_DEPTH`.
pub const MAX_DEPTH: usize = 32;

/// Значение: строка или вложенный раздел.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VdfValue {
    String(String),
    Object(VdfObject),
}

/// Раздел VDF — объект JS: ключи в порядке `Object.keys` (числовые по возрастанию, затем по
/// порядку вставки), повторный ключ перезаписывает значение на прежнем месте.
pub type VdfObject = JsObject<VdfValue>;

impl VdfValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            VdfValue::String(text) => Some(text),
            VdfValue::Object(_) => None,
        }
    }

    pub fn as_object(&self) -> Option<&VdfObject> {
        match self {
            VdfValue::Object(object) => Some(object),
            VdfValue::String(_) => None,
        }
    }

    /// `String(value)`: у раздела — `[object Object]`.
    pub fn js_string(&self) -> String {
        match self {
            VdfValue::String(text) => text.clone(),
            VdfValue::Object(_) => "[object Object]".to_owned(),
        }
    }

    /// Значение как JSON, ключи в порядке `Object.keys`.
    pub fn to_json(&self) -> Value {
        match self {
            VdfValue::String(text) => Value::String(text.clone()),
            VdfValue::Object(object) => object_to_json(object),
        }
    }
}

/// Раздел как объект JSON, ключи в порядке `Object.keys`.
pub fn object_to_json(object: &VdfObject) -> Value {
    let mut map = Map::new();
    for (key, value) in object.iter() {
        map.insert(key.to_owned(), value.to_json());
    }
    Value::Object(map)
}

enum Token {
    Open,
    Close,
    Text(String),
}

impl Token {
    /// `token.type` из 0.4.7.
    fn kind(&self) -> &'static str {
        match self {
            Token::Open => "{",
            Token::Close => "}",
            Token::Text(_) => "string",
        }
    }
}

fn error(line: impl std::fmt::Display, message: impl std::fmt::Display) -> DeliveryError {
    DeliveryError::new(
        Code::LaunchConfigInvalid,
        format!("VDF, строка {line}: {message}"),
    )
}

/// Разделитель слова без кавычек: `/[\s{}"]/`.
fn ends_bare_word(c: char) -> bool {
    js::is_whitespace(c) || matches!(c, '{' | '}' | '"')
}

fn escape_of(next: char) -> Option<char> {
    match next {
        '"' => Some('"'),
        '\\' => Some('\\'),
        'n' => Some('\n'),
        't' => Some('\t'),
        'r' => Some('\r'),
        _ => None,
    }
}

/// `tokenize(text)`: слова и скобки с номерами строк.
fn tokenize(text: &str) -> Result<Vec<(Token, usize)>, DeliveryError> {
    let source: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    let mut line = 1;

    while index < source.len() {
        let c = source[index];

        if c == '\n' {
            line += 1;
            index += 1;
            continue;
        }
        if matches!(c, ' ' | '\t' | '\r') {
            index += 1;
            continue;
        }
        if c == '/' && source.get(index + 1) == Some(&'/') {
            while index < source.len() && source[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if c == '{' || c == '}' {
            tokens.push((if c == '{' { Token::Open } else { Token::Close }, line));
            index += 1;
            continue;
        }
        if c == '[' {
            let Some(end) = source[index..].iter().position(|&x| x == ']') else {
                return Err(error(line, "незакрытая условная метка ["));
            };
            index += end + 1;
            continue;
        }
        if c == '"' {
            let mut value = String::new();
            index += 1;
            loop {
                let Some(&current) = source.get(index) else {
                    return Err(error(line, "незакрытая строка"));
                };
                if current == '"' {
                    index += 1;
                    break;
                }
                if current == '\\' {
                    if let Some(escaped) = source.get(index + 1).copied().and_then(escape_of) {
                        value.push(escaped);
                        index += 2;
                        continue;
                    }
                    value.push(current);
                    index += 1;
                    continue;
                }
                if current == '\n' {
                    line += 1;
                }
                value.push(current);
                index += 1;
            }
            tokens.push((Token::Text(value), line));
            continue;
        }

        let mut value = String::new();
        while let Some(&current) = source.get(index) {
            if ends_bare_word(current) {
                break;
            }
            if current == '/' && source.get(index + 1) == Some(&'/') {
                break;
            }
            value.push(current);
            index += 1;
        }
        if value.is_empty() {
            return Err(error(line, format!("неожиданный символ \"{c}\"")));
        }
        tokens.push((Token::Text(value), line));
    }

    Ok(tokens)
}

struct Parser {
    tokens: Vec<(Token, usize)>,
    position: usize,
}

impl Parser {
    /// Номер строки токена для текста ошибки; нет токена — `?`.
    fn line_of(&self, at: Option<usize>) -> String {
        at.and_then(|at| self.tokens.get(at))
            .map_or_else(|| "?".to_owned(), |(_, line)| line.to_string())
    }

    fn parse_object(&mut self, depth: usize) -> Result<VdfObject, DeliveryError> {
        if depth > MAX_DEPTH {
            return Err(error(
                self.line_of(Some(self.position)),
                "слишком глубокая вложенность",
            ));
        }

        let mut result = VdfObject::new();

        loop {
            let at = self.position;
            let Some((token, _)) = self.tokens.get(at) else {
                if depth == 0 {
                    return Ok(result);
                }
                return Err(error("?", "неожиданный конец файла: не хватает }"));
            };

            if matches!(token, Token::Close) {
                if depth == 0 {
                    return Err(error(self.line_of(Some(at)), "лишняя закрывающая скобка"));
                }
                self.position += 1;
                return Ok(result);
            }

            let Token::Text(key) = token else {
                return Err(error(
                    self.line_of(Some(at)),
                    format!("ожидался ключ, а не \"{}\"", token.kind()),
                ));
            };
            let key = key.clone();
            self.position += 1;

            let Some((next, _)) = self.tokens.get(self.position) else {
                return Err(error(
                    self.line_of(Some(at)),
                    format!("у ключа \"{key}\" нет значения"),
                ));
            };

            let value = match next {
                Token::Text(text) => {
                    let value = VdfValue::String(text.clone());
                    self.position += 1;
                    value
                }
                Token::Open => {
                    self.position += 1;
                    VdfValue::Object(self.parse_object(depth + 1)?)
                }
                Token::Close => {
                    return Err(error(
                        self.line_of(Some(self.position)),
                        format!("у ключа \"{key}\" вместо значения \"{}\"", next.kind()),
                    ));
                }
            };
            // `result["__proto__"] = …` в JS не создаёт собственного ключа.
            if key != "__proto__" {
                result.insert(key, value);
            }
        }
    }
}

/// `parse(text)`.
pub fn parse(text: &str) -> Result<VdfObject, DeliveryError> {
    let tokens = tokenize(text)?;
    Parser {
        tokens,
        position: 0,
    }
    .parse_object(0)
}

/// `getInsensitive(object, key)`: значение по ключу без учёта регистра (первый подошедший
/// в порядке `Object.keys`).
pub fn get_insensitive<'a>(object: &'a VdfObject, key: &str) -> Option<&'a VdfValue> {
    let lower = key.to_lowercase();
    object
        .iter()
        .find(|(candidate, _)| candidate.to_lowercase() == lower)
        .map(|(_, value)| value)
}

/// `getInsensitive(value, key)` для значения любого вида: у строки и у «ничего» — `None`
/// (`typeof object !== "object"`).
pub fn get_insensitive_in<'a>(value: Option<&'a VdfValue>, key: &str) -> Option<&'a VdfValue> {
    get_insensitive(value?.as_object()?, key)
}
