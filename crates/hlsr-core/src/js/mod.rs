//! Мелочи семантики JS и Node, без которых файлы и тексты расходятся с 0.4.7.

pub mod collate;
mod date;
pub mod errno;
pub mod fs;
pub mod iso;
pub mod math;
pub mod number;
pub mod object;
pub mod path;
pub mod process;
pub mod sort;
pub mod text;
pub mod value;

pub use date::{date_parse, date_parse_with};

use crate::json_text::js_number;
use serde_json::Value;
use std::cmp::Ordering;
use std::time::SystemTime;

/// ОС в смысле `process.platform` 0.4.7: от неё зависят правила путей, регистра и ошибок.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// `"win32"`: `path.win32`, пути без учёта регистра.
    Win32,
    /// `"linux"`: `path.posix`.
    Linux,
}

impl Platform {
    /// ОС, под которую собран лаунчер.
    pub const CURRENT: Platform = if cfg!(windows) {
        Platform::Win32
    } else {
        Platform::Linux
    };

    /// Значение `process.platform`.
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Win32 => "win32",
            Platform::Linux => "linux",
        }
    }
}

/// `Number.prototype.toString(36)` для целого.
pub fn base36(mut n: u128) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).expect("цифры base36 — ASCII")
}

/// `String(value)` для элемента массива в `join`: `null` → пустая строка.
/// Для значения вне массива верно, когда оно не `null`.
pub fn to_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => match n.as_f64() {
            Some(x) if x.is_nan() => "NaN".to_owned(),
            Some(x) if x.is_infinite() => if x > 0.0 { "Infinity" } else { "-Infinity" }.to_owned(),
            Some(x) => js_number(x).to_string(),
            // Число JSON за пределами f64 (`1e999`) JS читает как бесконечность.
            None if n.to_string().starts_with('-') => "-Infinity".to_owned(),
            None => "Infinity".to_owned(),
        },
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(to_string).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

/// Значение в шаблонной строке `${value}`: отсутствующее (`undefined`) — «undefined»,
/// `null` — «null», остальное — `String(value)` ([`to_string`]).
pub fn to_template_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::Null) => "null".to_owned(),
        Some(value) => to_string(value),
    }
}

/// Истинно ли значение в JS: ложны `null`, `false`, `0` и `""`.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        // Число JSON за пределами f64 (`1e999`) — бесконечность, она истинна.
        Value::Number(n) => n.as_f64().is_none_or(|x| x != 0.0 && !x.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `\s` и `trim` в JS: пробелы Unicode, но без U+0085 и с U+FEFF.
pub fn is_whitespace(c: char) -> bool {
    c == '\u{FEFF}' || (c.is_whitespace() && c != '\u{85}')
}

/// Те же пробелы, что [`is_whitespace`], — содержимое класса `[…]` для шаблона `regex`
/// (`\s` JS: без U+0085, с U+FEFF).
pub const WHITESPACE_CLASS: &str = r"\t\n\x0B\x0C\r \u{A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}";

/// `String.prototype.trim`.
pub fn trim(text: &str) -> &str {
    text.trim_matches(is_whitespace)
}

/// `text.slice(0, units)`: первые `units` единиц UTF-16. Разрезанную
/// суррогатную пару JS оставил бы одиночным суррогатом; при разборе файла
/// он всё равно становится U+FFFD, поэтому сразу пишется U+FFFD.
pub fn slice_utf16(text: &str, units: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let width = c.len_utf16();
        if used + width > units {
            if used < units {
                out.push('\u{FFFD}');
            }
            break;
        }
        out.push(c);
        used += width;
    }
    out
}

/// Сравнение строк, как `<` и `Array.prototype.sort()` в JS: по единицам UTF-16.
/// От сравнения байтов UTF-8 отличается на символах выше U+FFFF.
pub fn cmp_utf16(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// `Canonicalize` регулярного выражения с флагом `i` без `u`: символ, к которому
/// сводится сравнение без учёта регистра. Не-ASCII символ не сводится к ASCII
/// (`ſ` не равно `s`, знак Кельвина — не `k`), символ с многосимвольной
/// заглавной формой (`ß`) остаётся собой, половинки суррогатной пары — тоже.
pub fn canonicalize_ignore_case(c: char) -> char {
    if c.len_utf16() != 1 {
        return c;
    }
    let mut upper = c.to_uppercase();
    let (Some(u), None) = (upper.next(), upper.next()) else {
        return c;
    };
    if u.len_utf16() != 1 || (u32::from(c) >= 128 && u32::from(u) < 128) {
        return c;
    }
    u
}

/// `Number(text)` для строки: пробелы по краям отбрасываются, пустая строка — `0`,
/// `0x`/`0o`/`0b`, `Infinity`; всё прочее, кроме десятичной записи, — `NaN`.
pub fn to_number(text: &str) -> f64 {
    let s = trim(text);
    if s.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = s.strip_prefix(prefix) {
            return parse_radix(digits, radix);
        }
    }
    match s {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    if is_decimal_literal(s) {
        s.parse().unwrap_or(f64::NAN)
    } else {
        f64::NAN
    }
}

/// `Buffer.from(text, "base64")`: понимает base64 и base64url, пропускает всё,
/// что не буква алфавита (пробелы, переводы строк, мусор), останавливается на
/// первом `=` и бросает неполный хвост. Единицу UTF-16 выше U+00FF Node читает
/// по младшему байту (`Ł` — это `A`, а `😀` начинается с `=`).
pub fn buffer_from_base64(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut bits = 0u32;
    let mut count = 0u32;
    for unit in text.encode_utf16() {
        let byte = (unit & 0xFF) as u8;
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => continue,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    out
}

/// `Buffer.from(текст, "latin1")`: младший байт каждой единицы UTF-16. Символ выше
/// U+00FF теряет старший байт (`Ł` — это `A`), символ вне BMP — две единицы, два байта.
pub fn latin1_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .map(|unit| (unit & 0xFF) as u8)
        .collect()
}

/// `buffer.toString("latin1")`: байт — символ U+00XX.
pub fn latin1_string(bytes: &[u8]) -> String {
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

/// `buffer.toString("utf8")`: недопустимые последовательности заменяются на U+FFFD по
/// максимальным частям, метка порядка байтов не срезается.
pub fn utf8_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// `Math.round`: до половины — вниз, ровно половина — вверх (к +бесконечности).
pub fn math_round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let floor = x.floor();
    if x - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// `encodeURIComponent(text)`: остаются `A-Z a-z 0-9 - _ . ! ~ * ' ( )`, остальное —
/// `%XX` байтов UTF-8 (заглавные цифры).
pub fn encode_uri_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `Number.prototype.toFixed(digits)`: точное десятичное значение числа (не кратчайшая
/// запись), округление к ближайшему, при равной близости — к большему по модулю
/// (`0.0078125.toFixed(6)` — `0.007813`; Rust округлил бы к чётному). Знак остаётся у
/// отрицательного, округлённого до нуля (`(-1e-7).toFixed(6)` — `-0.000000`), у `-0` его нет.
/// Не конечное число и `|x| ≥ 1e21` — как `String(x)`.
pub fn to_fixed(x: f64, digits: usize) -> String {
    if !x.is_finite() || x.abs() >= 1e21 {
        return number_text(x);
    }
    let sign = if x < 0.0 { "-" } else { "" };
    // У двоичного числа не больше 1074 знаков после запятой: с запасом запись точная.
    let exact = format!("{:.1100}", x.abs());
    let (whole, fraction) = exact.split_once('.').unwrap_or((&exact, ""));
    let fraction = fraction.as_bytes();
    let mut kept: Vec<u8> = whole.bytes().collect();
    kept.extend((0..digits).map(|at| fraction.get(at).copied().unwrap_or(b'0')));
    if fraction.get(digits).is_some_and(|&next| next >= b'5') {
        // Прибавить единицу в последнем разряде: 9 → 0 с переносом.
        let mut at = kept.len();
        loop {
            if at == 0 {
                kept.insert(0, b'1');
                break;
            }
            at -= 1;
            if kept[at] == b'9' {
                kept[at] = b'0';
            } else {
                kept[at] += 1;
                break;
            }
        }
    }
    let split = kept.len() - digits;
    let (whole, fraction) = kept.split_at(split);
    let whole = String::from_utf8_lossy(whole);
    if digits == 0 {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{}", String::from_utf8_lossy(fraction))
    }
}

/// `String(x)` для числа: `NaN`, `Infinity`, `-Infinity`, иначе кратчайшая запись JS.
fn number_text(x: f64) -> String {
    if x.is_nan() {
        "NaN".to_owned()
    } else if x.is_infinite() {
        if x > 0.0 { "Infinity" } else { "-Infinity" }.to_owned()
    } else {
        js_number(x).to_string()
    }
}

/// `Date.prototype.toISOString`: `2026-10-15T12:00:00.000Z`.
pub fn iso_string(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time)
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

fn parse_radix(digits: &str, radix: u32) -> f64 {
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return f64::NAN;
    }
    // До 128 бит — точно; дальше (не встречается) — с округлением на каждом шаге.
    match u128::from_str_radix(digits, radix) {
        Ok(n) => n as f64,
        Err(_) => digits.chars().fold(0.0, |acc, c| {
            acc * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0))
        }),
    }
}

/// StrDecimalLiteral без `Infinity`: `[+-]? (цифры [. цифры?] | . цифры) ([eE][+-]? цифры)?`.
/// Rust сам разобрал бы и `inf`, и `NaN` — их JS не принимает.
fn is_decimal_literal(s: &str) -> bool {
    let s = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (mantissa, exponent) = match s.find(['e', 'E']) {
        Some(at) => (&s[..at], Some(&s[at + 1..])),
        None => (s, None),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    let mantissa_ok = digits(int) && digits(frac) && !(int.is_empty() && frac.is_empty());
    let exponent_ok = exponent.is_none_or(|e| {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        !e.is_empty() && digits(e)
    });
    mantissa_ok && exponent_ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn base36_matches_js_to_string_36() {
        assert_eq!(base36(0), "0");
        assert_eq!(base36(35), "z");
        assert_eq!(base36(1_760_000_000_000), "mgj6k3cw");
    }

    #[test]
    fn to_string_and_truthy() {
        assert_eq!(to_string(&json!([1.5, null, [true, "a"]])), "1.5,,true,a");
        assert_eq!(to_string(&json!({})), "[object Object]");
        for falsy in [json!(null), json!(false), json!(0), json!(-0.0), json!("")] {
            assert!(!truthy(&falsy), "{falsy}");
        }
        for value in [json!([]), json!({}), json!("0"), json!(-1)] {
            assert!(truthy(&value), "{value}");
        }
    }

    #[test]
    fn to_string_of_a_json_number_out_of_f64_range_is_infinity() {
        // `String(JSON.parse("1e999"))`, `String(JSON.parse("-1e999"))`.
        let parse = |text: &str| serde_json::from_str::<Value>(text).unwrap();
        assert_eq!(to_string(&parse("1e999")), "Infinity");
        assert_eq!(to_string(&parse("-1e999")), "-Infinity");
        assert_eq!(to_string(&parse("1e300")), "1e+300");
        assert!(truthy(&parse("1e999")));
        assert!(truthy(&parse("-1e999")));
        assert!(!truthy(&parse("0.0")));
    }

    #[test]
    fn template_string_of_missing_and_null_values() {
        // `${undefined}`, `${null}`, `${[1, null]}`, `${{}}`, `${1e21}`.
        assert_eq!(to_template_string(None), "undefined");
        assert_eq!(to_template_string(Some(&json!(null))), "null");
        assert_eq!(to_template_string(Some(&json!([1, null]))), "1,");
        assert_eq!(to_template_string(Some(&json!({}))), "[object Object]");
        assert_eq!(to_template_string(Some(&json!("текст"))), "текст");
        assert_eq!(to_template_string(Some(&json!(1e21))), "1e+21");
    }

    #[test]
    fn slice_counts_utf16_units() {
        assert_eq!(slice_utf16("яя", 1), "я");
        assert_eq!(slice_utf16("a😀b", 3), "a😀");
        assert_eq!(slice_utf16("a😀b", 2), "a\u{FFFD}");
        assert_eq!(slice_utf16("ab", 10), "ab");
    }
}
