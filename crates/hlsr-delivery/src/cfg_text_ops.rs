//! Операции текстового редактора cfg (`shared/cfg-text-ops.js`): поиск и замена,
//! работа со строками. Каждая правка — одна замена участка текста
//! [`Edit`] (`start`, `end`, `insert`) и новое выделение (`sel_start`, `sel_end`):
//! редактор применяет её как единое действие, и «Отменить» откатывает правку целиком.
//!
//! Текст — `&str`, а все смещения (границы совпадений и правок, выделение) —
//! единицы UTF-16, как у `textarea` и `String.length` в JS. Регулярные выражения —
//! `regress` (синтаксис и семантика ECMAScript, флаги `u` и `i`), поэтому
//! просмотр вперёд и назад, `\p{…}`, именованные группы и неверные выражения
//! ведут себя как в V8.
//!
//! Отличие от 0.4.7, где он зависает: пустое совпадение регулярки перед
//! суррогатной парой зацикливает `findAll` 0.4.7 (`lastIndex += 1` попадает в
//! середину пары, V8 возвращается к её началу), здесь после пустого совпадения
//! идут на целый символ. Остальное, в том числе странное (при курсоре в позиции 0
//! текста с пустой первой строкой [`line_span`] даёт `start > end`, а
//! [`move_lines`] вверх — выделение `−1`), повторено как есть; см. [`Edit`].
//!
//! Известные расхождения движка выражений с V8, которые сверка нашла и которые не
//! исправлены (в строку поиска так не пишут):
//! - `[^\W]` при флагах `iu` в V8 совпадает с `ſ` (U+017F) и знаком Кельвина
//!   (U+212A), как `\w`; `regress` их не берёт;
//! - квантификатор после `\b` и `\B` V8 в режиме `u` отвергает, `regress` принимает —
//!   это исправлено проверкой в [`build_pattern`].

use hlsr_core::js;
use regress::{Match as RegexMatch, Regex};

/// Больше совпадений редактор не ищет.
pub const MAX_MATCHES: usize = 10_000;

/// Параметры поиска.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Запрос — регулярное выражение, а не текст.
    pub regex: bool,
    /// Слово целиком (`\w` по краям не допускается).
    pub whole_word: bool,
    pub case_sensitive: bool,
}

/// Совпадение: `[start, end)` в единицах UTF-16.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub start: usize,
    pub end: usize,
}

/// Одна правка текста и новое выделение.
///
/// `start` и `end` — заменяемый участок. У [`line_span`] 0.4.7 при курсоре в
/// позиции 0 текста с пустой первой строкой `start` оказывается больше `end`, и
/// правки строк наследуют это. Выделение знаковое, потому что [`move_lines`] вверх
/// в этом же случае возвращает `−1`, как 0.4.7. Что с таким числом делает
/// `textarea`: Chromium в `setSelectionRange(-1, -1)` приводит `−1` к 4294967295 и
/// обрезает по длине текста — курсор уходит в конец текста, а не в 0
/// (`setSelectionRange(-5, 2)` даёт `[2, 2]`). Как отобразить выделение на
/// редактор Slint, решает подпроект 5; здесь число отдаётся как есть.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub insert: String,
    pub sel_start: i64,
    pub sel_end: i64,
}

/// Ошибка выражения поиска.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternError {
    /// Пустой запрос.
    Empty,
    /// Неверное регулярное выражение; текст — от движка выражений.
    BadRegex(String),
}

/// Собранное выражение поиска.
pub struct Pattern {
    regex: Regex,
    flags: &'static str,
}

impl Pattern {
    /// Флаги выражения в записи JS: `giu` или `gu`.
    pub fn flags(&self) -> &str {
        self.flags
    }
}

/// `escapeRegExp`: `.*+?^${}()|[]\` с обратной косой чертой.
fn escape_regexp(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if ".*+?^${}()|[]\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// V8 в режиме `u` отвергает квантификатор после `\b` и `\B` (в классе `\b` —
/// возврат на шаг, это не граница); `regress` его принимает.
fn has_quantified_boundary(source: &str) -> bool {
    let chars: Vec<char> = source.chars().collect();
    let mut in_class = false;
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' => {
                let boundary = matches!(chars.get(i + 1), Some('b' | 'B'));
                if boundary && !in_class && matches!(chars.get(i + 2), Some('*' | '+' | '?' | '{'))
                {
                    return true;
                }
                i += 2;
                continue;
            }
            '[' => in_class = true,
            ']' => in_class = false,
            _ => {}
        }
        i += 1;
    }
    false
}

/// Выражение для поиска или [`PatternError`].
pub fn build_pattern(query: &str, options: &Options) -> Result<Pattern, PatternError> {
    if query.is_empty() {
        return Err(PatternError::Empty);
    }
    let mut source = if options.regex {
        query.to_owned()
    } else {
        escape_regexp(query)
    };
    if options.whole_word {
        source = format!("(?<![\\w])(?:{source})(?![\\w])");
    }
    if has_quantified_boundary(&source) {
        return Err(PatternError::BadRegex("Nothing to repeat".to_owned()));
    }
    let flags = if options.case_sensitive { "gu" } else { "giu" };
    match Regex::with_flags(&source, flags) {
        Ok(regex) => Ok(Pattern { regex, flags }),
        Err(error) => Err(PatternError::BadRegex(error.to_string())),
    }
}

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn string(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

/// `text.slice(start, end)` для неотрицательных границ.
fn slice(units: &[u16], start: usize, end: usize) -> String {
    let end = end.min(units.len());
    if start >= end {
        return String::new();
    }
    string(&units[start..end])
}

fn len16(text: &str) -> usize {
    text.encode_utf16().count()
}

const NEWLINE: u16 = b'\n' as u16;

fn index_of(units: &[u16], needle: u16, from: usize) -> Option<usize> {
    units
        .get(from..)?
        .iter()
        .position(|&unit| unit == needle)
        .map(|at| at + from)
}

/// `text.lastIndexOf(needle, from)`: отрицательное `from` читается как 0.
fn last_index_of(units: &[u16], needle: u16, from: isize) -> Option<usize> {
    let last = units.len().checked_sub(1)?;
    let from = usize::try_from(from).unwrap_or(0).min(last);
    units[..=from].iter().rposition(|&unit| unit == needle)
}

/// Результат поиска.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindAll {
    pub matches: Vec<Match>,
    /// Выражение неверное (`BAD_REGEX`). Пустой запрос ошибкой не считается.
    pub bad_regex: bool,
    /// Найдено ровно [`MAX_MATCHES`]: возможно, дальше есть ещё.
    pub capped: bool,
}

/// Шаг после пустого совпадения: на целый символ, пара не рвётся.
fn step(units: &[u16], at: usize) -> usize {
    let lead = units.get(at).is_some_and(|u| (0xd800..0xdc00).contains(u));
    let trail = units
        .get(at + 1)
        .is_some_and(|u| (0xdc00..0xe000).contains(u));
    if lead && trail { 2 } else { 1 }
}

fn scan(pattern: &Pattern, units: &[u16]) -> Vec<Match> {
    let mut matches = Vec::new();
    let mut last = 0;
    while matches.len() < MAX_MATCHES && last <= units.len() {
        let Some(found) = pattern.regex.find_from_utf16(units, last).next() else {
            break;
        };
        let (start, end) = (found.start(), found.end());
        if start == end {
            // Пустые совпадения пропускаются.
            last = start + step(units, start);
            continue;
        }
        matches.push(Match { start, end });
        last = end;
    }
    matches
}

/// Все совпадения запроса; пустые совпадения регулярки пропускаются.
pub fn find_all(text: &str, query: &str, options: &Options) -> FindAll {
    match build_pattern(query, options) {
        Err(PatternError::Empty) => FindAll {
            matches: Vec::new(),
            bad_regex: false,
            capped: false,
        },
        Err(PatternError::BadRegex(_)) => FindAll {
            matches: Vec::new(),
            bad_regex: true,
            capped: false,
        },
        Ok(pattern) => {
            let matches = scan(&pattern, &utf16(text));
            let capped = matches.len() >= MAX_MATCHES;
            FindAll {
                matches,
                bad_regex: false,
                capped,
            }
        }
    }
}

/// Номер совпадения для курсора: первое, что начинается не раньше `from`
/// (`backward` — последнее, что кончается не позже `from`). По кругу; `None`,
/// если совпадений нет.
pub fn nearest_match(matches: &[Match], from: usize, backward: bool) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    if backward {
        let found = matches
            .iter()
            .rposition(|m| m.end <= from && m.start < from);
        return Some(found.unwrap_or(matches.len() - 1));
    }
    Some(matches.iter().position(|m| m.start >= from).unwrap_or(0))
}

/// `$$`, `$&`, `$1`…`$99` и `$<имя>` в тексте замены. Не как `String.prototype.replace`:
/// номер вне групп остаётся как написан (`$12` при трёх группах не превращается в `$1` и «2»),
/// а `$<имя>` без такой группы даёт пустую строку.
fn expand_replacement(replacement: &str, units: &[u16], found: &RegexMatch) -> String {
    let chars: Vec<char> = replacement.chars().collect();
    let group = |range: Option<std::ops::Range<usize>>| {
        range.map_or_else(String::new, |r| string(&units[r]))
    };
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '$' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        match chars.get(i + 1) {
            Some('$') => {
                out.push('$');
                i += 2;
            }
            Some('&') => {
                out.push_str(&group(Some(found.range())));
                i += 2;
            }
            Some('<') if chars[i + 2..].contains(&'>') => {
                let close = chars[i + 2..]
                    .iter()
                    .position(|&c| c == '>')
                    .expect("проверено")
                    + i
                    + 2;
                let name: String = chars[i + 2..close].iter().collect();
                out.push_str(&group(found.named_group(&name)));
                i = close + 1;
            }
            Some(first) if first.is_ascii_digit() => {
                let two = chars.get(i + 2).is_some_and(char::is_ascii_digit);
                let width = if two { 2 } else { 1 };
                let digits: String = chars[i + 1..i + 1 + width].iter().collect();
                let index: usize = digits.parse().expect("цифры");
                if index > 0 && index <= found.captures.len() {
                    out.push_str(&group(found.group(index)));
                } else {
                    out.push('$');
                    out.push_str(&digits);
                }
                i += 1 + width;
            }
            _ => {
                out.push('$');
                i += 1;
            }
        }
    }
    out
}

/// Текст замены для одного совпадения: в режиме регулярки работают `$1`, `$&`.
fn replacement_for(
    units: &[u16],
    found: Match,
    pattern: Option<&Pattern>,
    replacement: &str,
    options: &Options,
) -> String {
    if !options.regex {
        return replacement.to_owned();
    }
    let Some(pattern) = pattern else {
        return replacement.to_owned();
    };
    // В 0.4.7 то же выражение выполняется с флагом `y` с позиции совпадения.
    match pattern.regex.find_from_utf16(units, found.start).next() {
        Some(at) if at.start() == found.start => expand_replacement(replacement, units, &at),
        _ => replacement.to_owned(),
    }
}

/// Смещение как знаковое число выделения.
fn signed(offset: usize) -> i64 {
    i64::try_from(offset).expect("смещение в тексте помещается в i64")
}

fn utf16_edit(start: usize, end: usize, insert: String, sel_start: i64, sel_end: i64) -> Edit {
    Edit {
        start,
        end,
        insert,
        sel_start,
        sel_end,
    }
}

/// Правка для одного совпадения: заменить его, выделить вставленное.
pub fn replace_one(
    text: &str,
    found: Match,
    query: &str,
    replacement: &str,
    options: &Options,
) -> Edit {
    let pattern = build_pattern(query, options).ok();
    let insert = replacement_for(&utf16(text), found, pattern.as_ref(), replacement, options);
    let length = len16(&insert);
    utf16_edit(
        found.start,
        found.end,
        insert,
        signed(found.start),
        signed(found.start + length),
    )
}

/// Итог замены всех совпадений.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceAll {
    pub count: usize,
    pub bad_regex: bool,
    /// Одна правка на весь текст; `None`, если заменять нечего.
    pub edit: Option<Edit>,
}

/// Замена всех совпадений одной правкой на весь текст.
pub fn replace_all(text: &str, query: &str, replacement: &str, options: &Options) -> ReplaceAll {
    let found = find_all(text, query, options);
    if found.bad_regex || found.matches.is_empty() {
        return ReplaceAll {
            count: 0,
            bad_regex: found.bad_regex,
            edit: None,
        };
    }
    let units = utf16(text);
    let pattern = build_pattern(query, options).ok();
    let mut out = String::new();
    let mut at = 0;
    for m in &found.matches {
        out.push_str(&slice(&units, at, m.start));
        out.push_str(&replacement_for(
            &units,
            *m,
            pattern.as_ref(),
            replacement,
            options,
        ));
        at = m.end;
    }
    out.push_str(&slice(&units, at, units.len()));
    ReplaceAll {
        count: found.matches.len(),
        bad_regex: false,
        edit: Some(utf16_edit(0, units.len(), out, 0, 0)),
    }
}

/// Границы строк, которых касается выделение.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineSpan {
    pub start: usize,
    pub end: usize,
}

/// Границы строк, которых касается выделение: `[начало первой, конец последней)`
/// без завершающего перевода строки. Выделение, кончающееся в начале строки, эту
/// строку не захватывает. У текста, который начинается с пустой строки, при курсоре
/// в позиции 0 начало оказывается за концом (`start > end`), как у 0.4.7:
/// `lastIndexOf("\n", -1)` ищет в позиции 0.
pub fn line_span(text: &str, sel_start: usize, sel_end: usize) -> LineSpan {
    span_in(&utf16(text), sel_start, sel_end)
}

fn span_in(units: &[u16], sel_start: usize, sel_end: usize) -> LineSpan {
    let mut end = sel_end;
    if end > sel_start && units.get(end - 1) == Some(&NEWLINE) {
        end -= 1;
    }
    let start = last_index_of(units, NEWLINE, sel_start as isize - 1).map_or(0, |at| at + 1);
    let stop = index_of(units, NEWLINE, end).unwrap_or(units.len());
    LineSpan { start, end: stop }
}

/// Строка и столбец (с единицы).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    pub line: usize,
    pub col: usize,
}

pub fn line_col(text: &str, pos: usize) -> LineCol {
    let units = utf16(text);
    let before = &units[..pos.min(units.len())];
    let line = before.iter().filter(|&&unit| unit == NEWLINE).count() + 1;
    let line_begin = last_index_of(before, NEWLINE, before.len() as isize).map_or(0, |at| at + 1);
    LineCol {
        line,
        col: pos + 1 - line_begin,
    }
}

/// Смещение начала строки `line` (с единицы); за последней строкой — длина текста.
pub fn line_start(text: &str, line: usize) -> usize {
    let units = utf16(text);
    let mut at = 0;
    for _ in 1..line {
        match index_of(&units, NEWLINE, at) {
            Some(next) => at = next + 1,
            None => return units.len(),
        }
    }
    at
}

pub fn line_count(text: &str) -> usize {
    text.encode_utf16().filter(|&unit| unit == NEWLINE).count() + 1
}

/// Число единиц UTF-16 в ведущих пробелах (`\s*`).
fn indent_of(line: &str) -> usize {
    line.chars()
        .take_while(|&c| js::is_whitespace(c))
        .map(char::len_utf16)
        .sum()
}

/// `^(\s*)\/\/ ?` : отступ и «//», за которыми не больше одного пробела.
fn comment_prefix(line: &str) -> Option<(usize, usize)> {
    let indent_bytes: usize = line
        .chars()
        .take_while(|&c| js::is_whitespace(c))
        .map(char::len_utf8)
        .sum();
    let rest = line[indent_bytes..].strip_prefix("//")?;
    let space = usize::from(rest.starts_with(' '));
    Some((indent_bytes, indent_bytes + 2 + space))
}

/// Ctrl+/: закомментировать строки выделения «// » по наименьшему отступу или
/// снять комментарий, если закомментированы все непустые.
pub fn toggle_comment(text: &str, sel_start: usize, sel_end: usize) -> Edit {
    let units = utf16(text);
    let span = span_in(&units, sel_start, sel_end);
    let block = slice(&units, span.start, span.end);
    let lines: Vec<&str> = block.split('\n').collect();
    let filled: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| !js::trim(line).is_empty())
        .collect();
    let uncomment = !filled.is_empty() && filled.iter().all(|line| comment_prefix(line).is_some());
    let indent = filled.iter().map(|line| indent_of(line)).min().unwrap_or(0);
    let out: Vec<String> = lines
        .iter()
        .map(|line| {
            if js::trim(line).is_empty() {
                return (*line).to_owned();
            }
            if uncomment {
                let (indent_end, prefix_end) = comment_prefix(line).expect("проверено");
                return format!("{}{}", &line[..indent_end], &line[prefix_end..]);
            }
            let cut: usize = line.chars().take(indent).map(char::len_utf8).sum();
            format!("{}// {}", &line[..cut], &line[cut..])
        })
        .collect();
    let insert = out.join("\n");
    let length = len16(&insert);
    utf16_edit(
        span.start,
        span.end,
        insert,
        signed(span.start),
        signed(span.start + length),
    )
}

/// Ctrl+D: копия строк выделения ниже, выделение переходит на копию.
pub fn duplicate_lines(text: &str, sel_start: usize, sel_end: usize) -> Edit {
    let units = utf16(text);
    let span = span_in(&units, sel_start, sel_end);
    let block = slice(&units, span.start, span.end);
    let shift = len16(&block) + 1;
    utf16_edit(
        span.end,
        span.end,
        format!("\n{block}"),
        signed(sel_start + shift),
        signed(sel_end + shift),
    )
}

/// Ctrl+Shift+K: удалить строки выделения.
pub fn delete_lines(text: &str, sel_start: usize, sel_end: usize) -> Edit {
    let units = utf16(text);
    let span = span_in(&units, sel_start, sel_end);
    if span.end < units.len() {
        return utf16_edit(
            span.start,
            span.end + 1,
            String::new(),
            signed(span.start),
            signed(span.start),
        );
    }
    let start = span.start.saturating_sub(1);
    let caret = signed(span_in(&units, start, start).start);
    utf16_edit(start, span.end, String::new(), caret, caret)
}

/// Alt+↑/↓: переставить строки выделения с соседней. `None` — двигать некуда.
pub fn move_lines(text: &str, sel_start: usize, sel_end: usize, direction: i32) -> Option<Edit> {
    let units = utf16(text);
    let span = span_in(&units, sel_start, sel_end);
    let block = slice(&units, span.start, span.end);
    if direction < 0 {
        if span.start == 0 {
            return None;
        }
        let prev = span_in(&units, span.start - 1, span.start - 1);
        let above = slice(&units, prev.start, prev.end);
        let delta = len16(&above) + 1;
        return Some(utf16_edit(
            prev.start,
            span.end,
            format!("{block}\n{above}"),
            signed(sel_start) - signed(delta),
            signed(sel_end) - signed(delta),
        ));
    }
    if span.end >= units.len() {
        return None;
    }
    let next = span_in(&units, span.end + 1, span.end + 1);
    let below = slice(&units, next.start, next.end);
    let delta = len16(&below) + 1;
    Some(utf16_edit(
        span.start,
        next.end,
        format!("{below}\n{block}"),
        signed(sel_start + delta),
        signed(sel_end + delta),
    ))
}

/// Tab / Shift+Tab по выделению в несколько строк: отступ табуляцией.
pub fn indent_lines(text: &str, sel_start: usize, sel_end: usize, outdent: bool) -> Edit {
    let units = utf16(text);
    let span = span_in(&units, sel_start, sel_end);
    let block = slice(&units, span.start, span.end);
    let out: Vec<String> = block
        .split('\n')
        .map(|line| {
            if !outdent {
                return if line.is_empty() {
                    String::new()
                } else {
                    format!("\t{line}")
                };
            }
            if let Some(rest) = line.strip_prefix('\t') {
                return rest.to_owned();
            }
            let spaces = line.chars().take_while(|&c| c == ' ').count().min(4);
            line[spaces..].to_owned()
        })
        .collect();
    let insert = out.join("\n");
    let length = len16(&insert);
    utf16_edit(
        span.start,
        span.end,
        insert,
        signed(span.start),
        signed(span.start + length),
    )
}
