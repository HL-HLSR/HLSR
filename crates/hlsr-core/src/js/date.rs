//! `Date.parse` как в V8 (Node 24 main-процесса 0.4.7): формат даты ES (ISO 8601) и
//! «старые» даты, совместимые с Safari (`src/date/dateparser*.{h,cc}` V8 и
//! `ParseDateTimeString` в `builtins-date.cc`). Нужен `Retry-After` в виде даты HTTP.

/// `Date.parse(text)`: миллисекунды от 1970-01-01 UTC или `None` (в JS — `NaN`).
/// Дата без зоны считается местным временем ОС.
pub fn date_parse(text: &str) -> Option<i64> {
    date_parse_with(text, local_offset)
}

/// `Date.parse` с заданным смещением местного времени: `offset(местное_мс)` — сколько
/// миллисекунд местное время опережает UTC (для Москвы — 3 ч).
pub fn date_parse_with(text: &str, offset: impl Fn(i64) -> i64) -> Option<i64> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let parsed = parse(&units)?;
    let day = make_day(parsed.year, parsed.month, parsed.day)?;
    let time = i64::from(parsed.hour) * 3_600_000
        + i64::from(parsed.minute) * 60_000
        + i64::from(parsed.second) * 1000
        + i64::from(parsed.millisecond);
    let mut date = day * MS_PER_DAY + time;
    match parsed.utc_offset {
        None => {
            if !(-MAX_TIME_BEFORE_UTC_MS..=MAX_TIME_BEFORE_UTC_MS).contains(&date) {
                return None;
            }
            date -= offset(date);
        }
        Some(seconds) => {
            date -= i64::from(seconds) * 1000;
            if !(-MAX_TIME_MS..=MAX_TIME_MS).contains(&date) {
                return None;
            }
        }
    }
    // TimeClip.
    (-MAX_TIME_MS..=MAX_TIME_MS).contains(&date).then_some(date)
}

const MS_PER_DAY: i64 = 86_400_000;
const MAX_TIME_MS: i64 = 864 * 10_000_000_000_000;
const MAX_TIME_BEFORE_UTC_MS: i64 = MAX_TIME_MS + 864_000_000;
const MAX_YEAR: i64 = 1_000_000;

/// Смещение местного времени ОС в миллисекундах для местного момента `local_ms`.
fn local_offset(local_ms: i64) -> i64 {
    use chrono::{Local, LocalResult, Offset, TimeZone};
    let Some(naive) = chrono::DateTime::from_timestamp_millis(local_ms).map(|t| t.naive_utc())
    else {
        return 0;
    };
    let offset = match Local.offset_from_local_datetime(&naive) {
        LocalResult::Single(offset) | LocalResult::Ambiguous(offset, _) => offset,
        // Пропущенный час перехода на летнее время: смещение до перехода.
        LocalResult::None => Local.offset_from_utc_datetime(&naive),
    };
    i64::from(offset.fix().local_minus_utc()) * 1000
}

/// `MakeDay(year, month, date)`: номер дня от 1970-01-01, месяц с нуля, день может
/// выходить за длину месяца (31 февраля — 3 марта).
fn make_day(year: i32, month: i32, date: i32) -> Option<i64> {
    let year = i64::from(year);
    if !(-MAX_YEAR..=MAX_YEAR).contains(&year) {
        return None;
    }
    let month = i64::from(month);
    let y = year + month.div_euclid(12);
    let m = month.rem_euclid(12) + 1;
    Some(days_from_civil(y, m) + i64::from(date) - 1)
}

/// Дней от 1970-01-01 до первого числа месяца `m` (1–12) года `y` (пролептический григорианский).
fn days_from_civil(y: i64, m: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

// ---------------------------------------------------------------- разбор V8

const NONE: i32 = i32::MAX;
const MAX_SIGNIFICANT_DIGITS: usize = 9;

fn between(x: i32, lo: i32, hi: i32) -> bool {
    (x.wrapping_sub(lo) as u32) <= (hi.wrapping_sub(lo) as u32)
}

fn is_month(x: i32) -> bool {
    between(x, 1, 12)
}
fn is_day(x: i32) -> bool {
    between(x, 1, 31)
}
fn is_hour(x: i32) -> bool {
    between(x, 0, 23)
}
fn is_minute(x: i32) -> bool {
    between(x, 0, 59)
}
fn is_second(x: i32) -> bool {
    between(x, 0, 59)
}
fn is_hour12(x: i32) -> bool {
    between(x, 0, 12)
}
fn is_millisecond(x: i32) -> bool {
    between(x, 0, 999)
}

/// `IsWhiteSpace` V8: пробелы ECMAScript без переводов строки.
fn is_white_space(c: u32) -> bool {
    matches!(
        c,
        0x09 | 0x0B | 0x0C | 0x20 | 0xA0 | 0x1680 | 0x2000
            ..=0x200A | 0x202F | 0x205F | 0x3000 | 0xFEFF
    )
}

fn is_line_terminator(c: u32) -> bool {
    matches!(c, 0x0A | 0x0D | 0x2028 | 0x2029)
}

struct InputReader<'a> {
    index: usize,
    buffer: &'a [u16],
    ch: u32,
}

impl<'a> InputReader<'a> {
    fn new(buffer: &'a [u16]) -> Self {
        let mut reader = InputReader {
            index: 0,
            buffer,
            ch: 0,
        };
        reader.next();
        reader
    }

    fn position(&self) -> usize {
        self.index
    }

    fn next(&mut self) {
        self.ch = self
            .buffer
            .get(self.index)
            .map_or(0, |&unit| u32::from(unit));
        self.index += 1;
    }

    fn read_unsigned_numeral(&mut self) -> i32 {
        let mut n: i32 = 0;
        let mut i = 0;
        while self.ch == u32::from(b'0') {
            self.next();
        }
        while self.is_ascii_digit() {
            if i < MAX_SIGNIFICANT_DIGITS {
                n = n * 10 + (self.ch - u32::from(b'0')) as i32;
            }
            i += 1;
            self.next();
        }
        n
    }

    fn read_word(&mut self, prefix: &mut [u32; 3]) -> usize {
        let mut len = 0;
        while self.is_ascii_alpha_or_above() && !self.is_white_space_char() {
            if len < prefix.len() {
                prefix[len] = self.ch | 0x20;
            }
            self.next();
            len += 1;
        }
        for slot in prefix.iter_mut().skip(len) {
            *slot = 0;
        }
        len
    }

    fn skip(&mut self, c: u8) -> bool {
        if self.ch == u32::from(c) {
            self.next();
            true
        } else {
            false
        }
    }

    fn skip_white_space(&mut self) -> bool {
        if is_white_space(self.ch) || is_line_terminator(self.ch) {
            self.next();
            true
        } else {
            false
        }
    }

    fn skip_parentheses(&mut self) -> bool {
        if self.ch != u32::from(b'(') {
            return false;
        }
        let mut balance = 0;
        loop {
            if self.ch == u32::from(b')') {
                balance -= 1;
            } else if self.ch == u32::from(b'(') {
                balance += 1;
            }
            self.next();
            if !(balance > 0 && self.ch != 0) {
                break;
            }
        }
        true
    }

    fn is_end(&self) -> bool {
        self.ch == 0
    }
    fn is_ascii_digit(&self) -> bool {
        (u32::from(b'0')..=u32::from(b'9')).contains(&self.ch)
    }
    fn is_ascii_alpha_or_above(&self) -> bool {
        self.ch >= u32::from(b'A')
    }
    fn is_white_space_char(&self) -> bool {
        is_white_space(self.ch)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeywordType {
    Invalid,
    MonthName,
    TimeZoneName,
    TimeSeparator,
    AmPm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    Invalid,
    Unknown,
    WhiteSpace,
    Number,
    Symbol,
    EndOfInput,
    Keyword(KeywordType),
}

#[derive(Debug, Clone, Copy)]
struct DateToken {
    tag: Tag,
    length: usize,
    value: i32,
}

impl DateToken {
    fn is_invalid(&self) -> bool {
        self.tag == Tag::Invalid
    }
    fn is_number(&self) -> bool {
        self.tag == Tag::Number
    }
    fn is_white_space(&self) -> bool {
        self.tag == Tag::WhiteSpace
    }
    fn is_end_of_input(&self) -> bool {
        self.tag == Tag::EndOfInput
    }
    fn is_keyword(&self) -> bool {
        matches!(self.tag, Tag::Keyword(_))
    }
    fn is_symbol(&self, symbol: u8) -> bool {
        self.tag == Tag::Symbol && self.value == i32::from(symbol)
    }
    fn is_keyword_type(&self, kind: KeywordType) -> bool {
        self.tag == Tag::Keyword(kind)
    }
    fn is_fixed_length_number(&self, length: usize) -> bool {
        self.is_number() && self.length == length
    }
    fn is_ascii_sign(&self) -> bool {
        self.is_symbol(b'-') || self.is_symbol(b'+')
    }
    /// `+` → 1, `-` → -1.
    fn ascii_sign(&self) -> i32 {
        44 - self.value
    }
    fn is_keyword_z(&self) -> bool {
        self.is_keyword_type(KeywordType::TimeZoneName) && self.length == 1 && self.value == 0
    }

    fn number(value: i32, length: usize) -> Self {
        DateToken {
            tag: Tag::Number,
            length,
            value,
        }
    }
    fn symbol(symbol: u8) -> Self {
        DateToken {
            tag: Tag::Symbol,
            length: 1,
            value: i32::from(symbol),
        }
    }
    fn end_of_input() -> Self {
        DateToken {
            tag: Tag::EndOfInput,
            length: 0,
            value: -1,
        }
    }
    fn white_space(length: usize) -> Self {
        DateToken {
            tag: Tag::WhiteSpace,
            length,
            value: -1,
        }
    }
    fn unknown() -> Self {
        DateToken {
            tag: Tag::Unknown,
            length: 1,
            value: -1,
        }
    }
    fn invalid() -> Self {
        DateToken {
            tag: Tag::Invalid,
            length: 0,
            value: -1,
        }
    }
}

/// Слова: первые три буквы в нижнем регистре, тип, значение.
const KEYWORDS: &[([u8; 3], KeywordType, i32)] = &[
    (*b"jan", KeywordType::MonthName, 1),
    (*b"feb", KeywordType::MonthName, 2),
    (*b"mar", KeywordType::MonthName, 3),
    (*b"apr", KeywordType::MonthName, 4),
    (*b"may", KeywordType::MonthName, 5),
    (*b"jun", KeywordType::MonthName, 6),
    (*b"jul", KeywordType::MonthName, 7),
    (*b"aug", KeywordType::MonthName, 8),
    (*b"sep", KeywordType::MonthName, 9),
    (*b"oct", KeywordType::MonthName, 10),
    (*b"nov", KeywordType::MonthName, 11),
    (*b"dec", KeywordType::MonthName, 12),
    (*b"am\0", KeywordType::AmPm, 0),
    (*b"pm\0", KeywordType::AmPm, 12),
    (*b"ut\0", KeywordType::TimeZoneName, 0),
    (*b"utc", KeywordType::TimeZoneName, 0),
    (*b"z\0\0", KeywordType::TimeZoneName, 0),
    (*b"gmt", KeywordType::TimeZoneName, 0),
    (*b"cdt", KeywordType::TimeZoneName, -5),
    (*b"cst", KeywordType::TimeZoneName, -6),
    (*b"edt", KeywordType::TimeZoneName, -4),
    (*b"est", KeywordType::TimeZoneName, -5),
    (*b"mdt", KeywordType::TimeZoneName, -6),
    (*b"mst", KeywordType::TimeZoneName, -7),
    (*b"pdt", KeywordType::TimeZoneName, -7),
    (*b"pst", KeywordType::TimeZoneName, -8),
    (*b"t\0\0", KeywordType::TimeSeparator, 0),
];

/// `KeywordTable::Lookup`: слово длиннее трёх букв годится только для месяца.
fn lookup(prefix: &[u32; 3], len: usize) -> (KeywordType, i32) {
    for (word, kind, value) in KEYWORDS {
        let matched = prefix.iter().zip(word).all(|(&p, &w)| p == u32::from(w));
        if matched && (len <= 3 || *kind == KeywordType::MonthName) {
            return (*kind, *value);
        }
    }
    (KeywordType::Invalid, 0)
}

struct Tokenizer<'a> {
    input: InputReader<'a>,
    next: DateToken,
}

impl<'a> Tokenizer<'a> {
    fn new(mut input: InputReader<'a>) -> Self {
        let next = scan(&mut input);
        Tokenizer { input, next }
    }

    fn next(&mut self) -> DateToken {
        let result = self.next;
        self.next = scan(&mut self.input);
        result
    }

    fn peek(&self) -> DateToken {
        self.next
    }

    fn skip_symbol(&mut self, symbol: u8) -> bool {
        if self.next.is_symbol(symbol) {
            self.next = scan(&mut self.input);
            true
        } else {
            false
        }
    }
}

fn scan(input: &mut InputReader<'_>) -> DateToken {
    let pre_pos = input.position();
    if input.is_end() {
        return DateToken::end_of_input();
    }
    if input.is_ascii_digit() {
        let n = input.read_unsigned_numeral();
        return DateToken::number(n, input.position() - pre_pos);
    }
    for symbol in *b":-+.)" {
        if input.skip(symbol) {
            return DateToken::symbol(symbol);
        }
    }
    if input.is_ascii_alpha_or_above() && !input.is_white_space_char() {
        let mut prefix = [0u32; 3];
        let length = input.read_word(&mut prefix);
        let (kind, value) = lookup(&prefix, length);
        return DateToken {
            tag: Tag::Keyword(kind),
            length,
            value,
        };
    }
    if input.skip_white_space() {
        return DateToken::white_space(input.position() - pre_pos);
    }
    if input.skip_parentheses() {
        return DateToken::unknown();
    }
    input.next();
    DateToken::unknown()
}

/// `ReadMilliseconds`: первые три значащие цифры числа.
fn read_milliseconds(token: DateToken) -> i32 {
    let mut number = token.value;
    let mut length = token.length;
    if length < 3 {
        if length == 1 {
            number *= 100;
        } else if length == 2 {
            number *= 10;
        }
    } else if length > 3 {
        length = length.min(MAX_SIGNIFICANT_DIGITS);
        let mut factor = 1;
        loop {
            factor *= 10;
            length -= 1;
            if length <= 3 {
                break;
            }
        }
        number /= factor;
    }
    number
}

struct TimeZoneComposer {
    sign: i32,
    hour: i32,
    minute: i32,
}

impl TimeZoneComposer {
    fn set(&mut self, offset_in_hours: i32) {
        self.sign = if offset_in_hours < 0 { -1 } else { 1 };
        self.hour = offset_in_hours * self.sign;
        self.minute = 0;
    }
    fn set_sign(&mut self, sign: i32) {
        self.sign = if sign < 0 { -1 } else { 1 };
    }
    fn is_expecting(&self, n: i32) -> bool {
        self.hour != NONE && self.minute == NONE && is_minute(n)
    }
    fn is_utc(&self) -> bool {
        self.hour == 0 && self.minute == 0
    }
    fn is_empty(&self) -> bool {
        self.hour == NONE
    }
    /// Смещение в секундах; `Some(None)` — зоны нет; `None` — ошибка.
    fn write(&mut self) -> Option<Option<i32>> {
        if self.sign == NONE {
            return Some(None);
        }
        if self.hour == NONE {
            self.hour = 0;
        }
        if self.minute == NONE {
            self.minute = 0;
        }
        // Как в V8: беззнаковая 32-битная арифметика.
        let total = (self.hour as u32)
            .wrapping_mul(3600)
            .wrapping_add((self.minute as u32).wrapping_mul(60));
        if total > (1u32 << 30) - 1 {
            return None;
        }
        let total = total as i32;
        Some(Some(if self.sign < 0 { -total } else { total }))
    }
}

struct TimeComposer {
    comp: [i32; 4],
    index: usize,
    hour_offset: i32,
}

impl TimeComposer {
    fn is_empty(&self) -> bool {
        self.index == 0
    }
    fn is_expecting(&self, n: i32) -> bool {
        (self.index == 1 && is_minute(n))
            || (self.index == 2 && is_second(n))
            || (self.index == 3 && is_millisecond(n))
    }
    fn add(&mut self, n: i32) -> bool {
        if self.index < 4 {
            self.comp[self.index] = n;
            self.index += 1;
            true
        } else {
            false
        }
    }
    fn add_final(&mut self, n: i32) -> bool {
        if !self.add(n) {
            return false;
        }
        while self.index < 4 {
            self.comp[self.index] = 0;
            self.index += 1;
        }
        true
    }
    fn write(&mut self) -> Option<[i32; 4]> {
        while self.index < 4 {
            self.comp[self.index] = 0;
            self.index += 1;
        }
        let [mut hour, minute, second, millisecond] = self.comp;
        if self.hour_offset != NONE {
            if !is_hour12(hour) {
                return None;
            }
            hour %= 12;
            hour += self.hour_offset;
        }
        if !is_hour(hour)
            || !is_minute(minute)
            || !is_second(second)
            || !is_millisecond(millisecond)
        {
            // 24:00:00.000 допустимо.
            if hour != 24 || minute != 0 || second != 0 || millisecond != 0 {
                return None;
            }
        }
        Some([hour, minute, second, millisecond])
    }
}

struct DayComposer {
    comp: [i32; 3],
    index: usize,
    named_month: i32,
    is_iso_date: bool,
}

impl DayComposer {
    fn is_empty(&self) -> bool {
        self.index == 0
    }
    fn add(&mut self, n: i32) -> bool {
        if self.index < 3 {
            self.comp[self.index] = n;
            self.index += 1;
            true
        } else {
            false
        }
    }
    /// (год, месяц с нуля, день).
    fn write(&mut self) -> Option<(i32, i32, i32)> {
        if self.index < 1 {
            return None;
        }
        while self.index < 3 {
            self.comp[self.index] = 1;
            self.index += 1;
        }
        let mut year = 0;
        let month;
        let day;
        if self.named_month == NONE {
            if self.is_iso_date || (self.index == 3 && !is_day(self.comp[0])) {
                year = self.comp[0];
                month = self.comp[1];
                day = self.comp[2];
            } else {
                month = self.comp[0];
                day = self.comp[1];
                if self.index == 3 {
                    year = self.comp[2];
                }
            }
        } else {
            month = self.named_month;
            if self.index == 1 {
                day = self.comp[0];
            } else if !is_day(self.comp[0]) {
                year = self.comp[0];
                day = self.comp[1];
            } else {
                day = self.comp[0];
                year = self.comp[1];
            }
        }
        if !self.is_iso_date {
            if between(year, 0, 49) {
                year += 2000;
            } else if between(year, 50, 99) {
                year += 1900;
            }
        }
        if !is_month(month) || !is_day(day) {
            return None;
        }
        Some((year, month - 1, day))
    }
}

struct Parsed {
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: i32,
    millisecond: i32,
    utc_offset: Option<i32>,
}

fn parse(units: &[u16]) -> Option<Parsed> {
    let mut scanner = Tokenizer::new(InputReader::new(units));
    let mut tz = TimeZoneComposer {
        sign: NONE,
        hour: NONE,
        minute: NONE,
    };
    let mut time = TimeComposer {
        comp: [0; 4],
        index: 0,
        hour_offset: NONE,
    };
    let mut day = DayComposer {
        comp: [0; 3],
        index: 0,
        named_month: NONE,
        is_iso_date: false,
    };

    let next_unhandled = parse_es5_date_time(&mut scanner, &mut day, &mut time, &mut tz);
    if next_unhandled.is_invalid() {
        return None;
    }
    let mut has_read_number = !day.is_empty();
    let mut token = next_unhandled;
    while !token.is_end_of_input() {
        if token.is_number() {
            has_read_number = true;
            let n = token.value;
            if scanner.skip_symbol(b':') {
                if scanner.skip_symbol(b':') {
                    if !time.is_empty() {
                        return None;
                    }
                    time.add(n);
                    time.add(0);
                } else {
                    if !time.add(n) {
                        return None;
                    }
                    if scanner.peek().is_symbol(b'.') {
                        scanner.next();
                    }
                }
            } else if scanner.skip_symbol(b'.') && time.is_expecting(n) {
                time.add(n);
                if !scanner.peek().is_number() {
                    return None;
                }
                let ms = read_milliseconds(scanner.next());
                if ms < 0 {
                    return None;
                }
                time.add_final(ms);
            } else if tz.is_expecting(n) {
                tz.minute = n;
            } else if time.is_expecting(n) {
                time.add_final(n);
                let peek = scanner.peek();
                if !peek.is_end_of_input()
                    && !peek.is_white_space()
                    && !peek.is_keyword_z()
                    && !peek.is_ascii_sign()
                {
                    return None;
                }
            } else {
                if !day.add(n) {
                    return None;
                }
                scanner.skip_symbol(b'-');
            }
        } else if token.is_keyword() {
            let Tag::Keyword(kind) = token.tag else {
                unreachable!("is_keyword");
            };
            let value = token.value;
            if kind == KeywordType::AmPm && !time.is_empty() {
                time.hour_offset = value;
            } else if kind == KeywordType::MonthName {
                day.named_month = value;
                scanner.skip_symbol(b'-');
            } else if kind == KeywordType::TimeZoneName && has_read_number {
                tz.set(value);
            } else {
                if has_read_number {
                    return None;
                }
                if scanner.peek().is_number() {
                    return None;
                }
            }
        } else if token.is_ascii_sign() && (tz.is_utc() || !time.is_empty()) {
            tz.set_sign(token.ascii_sign());
            let mut n = 0;
            let mut length = 0;
            if scanner.peek().is_number() {
                let next = scanner.next();
                length = next.length;
                n = next.value;
            }
            has_read_number = true;
            if scanner.peek().is_symbol(b':') {
                tz.hour = n;
                tz.minute = NONE;
            } else if length == 1 || length == 2 {
                tz.hour = n;
                tz.minute = 0;
            } else if length == 3 || length == 4 {
                tz.hour = n / 100;
                tz.minute = n % 100;
            } else {
                return None;
            }
        } else if (token.is_ascii_sign() || token.is_symbol(b')')) && has_read_number {
            return None;
        }
        token = scanner.next();
    }

    let (year, month, day) = day.write()?;
    let [hour, minute, second, millisecond] = time.write()?;
    let utc_offset = tz.write()?;
    Some(Parsed {
        year,
        month,
        day,
        hour,
        minute,
        second,
        millisecond,
        utc_offset,
    })
}

fn parse_es5_date_time(
    scanner: &mut Tokenizer<'_>,
    day: &mut DayComposer,
    time: &mut TimeComposer,
    tz: &mut TimeZoneComposer,
) -> DateToken {
    // Дата: [('-'|'+')yy]yyyy['-'MM['-'DD]].
    if scanner.peek().is_ascii_sign() {
        let sign_token = scanner.next();
        if !scanner.peek().is_fixed_length_number(6) {
            return sign_token;
        }
        let sign = sign_token.ascii_sign();
        let year = scanner.next().value;
        if sign < 0 && year == 0 {
            return sign_token;
        }
        day.add(sign * year);
    } else if scanner.peek().is_fixed_length_number(4) {
        day.add(scanner.next().value);
    } else {
        return scanner.next();
    }
    if scanner.skip_symbol(b'-') {
        if !scanner.peek().is_fixed_length_number(2) || !is_month(scanner.peek().value) {
            return scanner.next();
        }
        day.add(scanner.next().value);
        if scanner.skip_symbol(b'-') {
            if !scanner.peek().is_fixed_length_number(2) || !is_day(scanner.peek().value) {
                return scanner.next();
            }
            day.add(scanner.next().value);
        }
    }
    // Время: 'T'HH':'mm[':'ss['.'sss]][Z|(+|-)hh:mm].
    if !scanner.peek().is_keyword_type(KeywordType::TimeSeparator) {
        if !scanner.peek().is_end_of_input() {
            return scanner.next();
        }
    } else {
        scanner.next();
        if !scanner.peek().is_fixed_length_number(2) || !between(scanner.peek().value, 0, 24) {
            return DateToken::invalid();
        }
        let hour_is_24 = scanner.peek().value == 24;
        time.add(scanner.next().value);
        if !scanner.skip_symbol(b':') {
            return DateToken::invalid();
        }
        if !scanner.peek().is_fixed_length_number(2)
            || !is_minute(scanner.peek().value)
            || (hour_is_24 && scanner.peek().value > 0)
        {
            return DateToken::invalid();
        }
        time.add(scanner.next().value);
        if scanner.skip_symbol(b':') {
            if !scanner.peek().is_fixed_length_number(2)
                || !is_second(scanner.peek().value)
                || (hour_is_24 && scanner.peek().value > 0)
            {
                return DateToken::invalid();
            }
            time.add(scanner.next().value);
            if scanner.skip_symbol(b'.') {
                if !scanner.peek().is_number() || (hour_is_24 && scanner.peek().value > 0) {
                    return DateToken::invalid();
                }
                time.add(read_milliseconds(scanner.next()));
            }
        }
        if scanner.peek().is_keyword_z() {
            scanner.next();
            tz.set(0);
        } else if scanner.peek().is_symbol(b'+') || scanner.peek().is_symbol(b'-') {
            tz.set_sign(if scanner.next().is_symbol(b'+') {
                1
            } else {
                -1
            });
            if scanner.peek().is_fixed_length_number(4) {
                let hourmin = scanner.next().value;
                let hour = hourmin / 100;
                let min = hourmin % 100;
                if !is_hour(hour) || !is_minute(min) {
                    return DateToken::invalid();
                }
                tz.hour = hour;
                tz.minute = min;
            } else {
                if !scanner.peek().is_fixed_length_number(2) || !is_hour(scanner.peek().value) {
                    return DateToken::invalid();
                }
                tz.hour = scanner.next().value;
                if !scanner.skip_symbol(b':') {
                    return DateToken::invalid();
                }
                if !scanner.peek().is_fixed_length_number(2) || !is_minute(scanner.peek().value) {
                    return DateToken::invalid();
                }
                tz.minute = scanner.next().value;
            }
        }
        if !scanner.peek().is_end_of_input() {
            return DateToken::invalid();
        }
    }
    // Без зоны: дата без времени — UTC, дата со временем — местное время.
    if tz.is_empty() && time.is_empty() {
        tz.set(0);
    }
    day.is_iso_date = true;
    DateToken::end_of_input()
}
