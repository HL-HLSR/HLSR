//! JSON-текст побайтно как у 0.4.7: `JSON.parse` и `JSON.stringify` из Node.

use serde_json::{Map, Number, Value};
use std::fmt::Write as _;

/// Как 0.4.7 записывает файл.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonFormat {
    /// `writeState` из `Store.js`: табуляция, без перевода строки в конце,
    /// временный файл `<файл>.tmp`, права 0600 (Unix).
    Store,
    /// `writeJsonAtomic` из `atomic.js`: два пробела и `\n` в конце,
    /// временный файл `.<имя>.<pid>.<время36>.tmp`, права по умолчанию.
    Atomic,
}

/// Текст, который записал бы `JSON.stringify` 0.4.7 для того же значения.
///
/// Числа выводятся так, как JS видит их после `JSON.parse`: через `f64`
/// (`1.50` → `1.5`, `1e400` → `null`). Ключи-индексы массива (`"50"`, `"130"`)
/// идут первыми по возрастанию, остальные — в порядке вставки, как в объекте JS.
pub fn to_js_json(value: &Value, format: JsonFormat) -> String {
    match format {
        JsonFormat::Store => stringify(value, "\t"),
        JsonFormat::Atomic => stringify(value, "  ") + "\n",
    }
}

/// `JSON.stringify(value, null, gap)`: `gap` — отступ уровня (`"  "`, `" "`, `"\t"`),
/// пустой — запись в одну строку без пробелов. Так пишутся остальные файлы 0.4.7:
/// `install.lock` и журналы установки (`"  "` без `\n`), `.hlsrbuild` (`" "`),
/// `presence-endpoint.json` (`""`). Числа и порядок ключей — как в [`to_js_json`].
pub fn stringify(value: &Value, gap: &str) -> String {
    let mut out = String::new();
    write_value(&mut out, value, gap, 0);
    out
}

/// Число в записи JS `Number.prototype.toString`; не конечное → `null`, `-0` → `0`.
pub fn js_number(n: f64) -> Value {
    match js_number_text(n) {
        Some(text) => Value::Number(text.parse::<Number>().expect("ryu-js выдаёт число JSON")),
        None => Value::Null,
    }
}

/// Разбор как `JSON.parse` 0.4.7 после среза BOM: одиночные суррогаты
/// `\uD800–\uDFFF` (JS их принимает, serde_json — нет) становятся `�`.
pub fn parse_js_json(text: &str) -> Result<Value, serde_json::Error> {
    json_parse(text.strip_prefix('\u{FEFF}').unwrap_or(text))
}

/// `JSON.parse(text)` без среза BOM (как `JSON.parse(fs.readFileSync(file, "utf8"))`: BOM —
/// ошибка разбора); одиночные суррогаты становятся `�`, как в [`parse_js_json`].
pub fn json_parse(text: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(&replace_lone_surrogates(text))
}

fn js_number_text(n: f64) -> Option<String> {
    if !n.is_finite() {
        return None;
    }
    if n == 0.0 {
        return Some("0".to_owned());
    }
    Some(ryu_js::Buffer::new().format_finite(n).to_owned())
}

fn write_value(out: &mut String, value: &Value, indent: &str, depth: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            let text = n.as_f64().and_then(js_number_text);
            out.push_str(text.as_deref().unwrap_or("null"));
        }
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                new_line(out, indent, depth + 1);
                write_value(out, item, indent, depth + 1);
            }
            new_line(out, indent, depth);
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (key, item)) in js_key_order(map).into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                new_line(out, indent, depth + 1);
                write_string(out, key);
                out.push_str(if indent.is_empty() { ":" } else { ": " });
                write_value(out, item, indent, depth + 1);
            }
            new_line(out, indent, depth);
            out.push('}');
        }
    }
}

fn new_line(out: &mut String, indent: &str, depth: usize) {
    if indent.is_empty() {
        return;
    }
    out.push('\n');
    for _ in 0..depth {
        out.push_str(indent);
    }
}

/// Порядок ключей объекта JS (`Object.keys`, `for…in`): индексы массива по возрастанию,
/// затем остальные в порядке вставки.
pub fn js_key_order(map: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut indices: Vec<(u32, (&String, &Value))> = Vec::new();
    let mut others = Vec::new();
    for entry in map {
        match array_index(entry.0) {
            Some(index) => indices.push((index, entry)),
            None => others.push(entry),
        }
    }
    indices.sort_by_key(|(index, _)| *index);
    indices
        .into_iter()
        .map(|(_, entry)| entry)
        .chain(others)
        .collect()
}

/// Индекс массива JS: каноническая запись целого от 0 до 2³² − 2.
pub fn array_index(key: &str) -> Option<u32> {
    if key.is_empty() || (key.len() > 1 && key.starts_with('0')) {
        return None;
    }
    if !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    key.parse::<u32>().ok().filter(|index| *index != u32::MAX)
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < '\u{20}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn replace_lone_surrogates(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains("\\u") {
        return text.into();
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            i += 1;
            continue;
        }
        // Экранированный символ пропускается целиком: `\\ud83d` — не суррогат.
        if bytes.get(i + 1) != Some(&b'u') {
            i += 2;
            continue;
        }
        let Some(unit) = hex_unit(bytes, i + 2) else {
            i += 2;
            continue;
        };
        let paired_low = (0xD800..0xDC00).contains(&unit)
            && bytes.get(i + 6) == Some(&b'\\')
            && bytes.get(i + 7) == Some(&b'u')
            && hex_unit(bytes, i + 8).is_some_and(|low| (0xDC00..0xE000).contains(&low));
        if paired_low {
            i += 12;
        } else if (0xD800..0xE000).contains(&unit) {
            out.push_str(&text[copied..i]);
            out.push_str("\\uFFFD");
            i += 6;
            copied = i;
        } else {
            i += 6;
        }
    }
    out.push_str(&text[copied..]);
    out.into()
}

fn hex_unit(bytes: &[u8], start: usize) -> Option<u32> {
    let digits = bytes.get(start..start + 4)?;
    let digits = std::str::from_utf8(digits).ok()?;
    u32::from_str_radix(digits, 16)
        .ok()
        .filter(|_| digits.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_parse_keeps_bom_an_error_unlike_parse_js_json() {
        // `JSON.parse("\uFEFF{}")` — SyntaxError; файлы хранилищ срезают BOM до разбора.
        assert!(json_parse("\u{FEFF}{}").is_err());
        assert!(parse_js_json("\u{FEFF}{}").is_ok());
    }

    #[test]
    fn json_parse_replaces_lone_surrogates() {
        let value = json_parse(r#"{"a":"\ud83d","b":"\ud83d\ude00"}"#).unwrap();
        assert_eq!(value["a"], "\u{FFFD}");
        assert_eq!(value["b"], "😀");
    }
}
