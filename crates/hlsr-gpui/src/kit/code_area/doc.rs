//! Текст редактора без GPUI: начала строк, табуляция (`tab-size: 4`), шаг по символам и словам.
//! Смещения — байты UTF-8; перевод строки — только `\n` (`textarea` хранит значение так же).

use std::borrow::Cow;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation as _;

/// `tab-size: 4` (`.cf__text`).
pub const TAB: usize = 4;

/// Строки текста: начало каждой и самая длинная в колонках.
#[derive(Clone, Debug)]
pub struct Lines {
    starts: Vec<usize>,
    len: usize,
    widest: usize,
}

impl Lines {
    pub fn new(text: &str) -> Lines {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
        let mut lines = Lines {
            starts,
            len: text.len(),
            widest: 0,
        };
        lines.widest = (0..lines.count())
            .map(|ix| columns(&text[lines.range(ix)]))
            .max()
            .unwrap_or(0);
        lines
    }

    pub fn count(&self) -> usize {
        self.starts.len()
    }

    /// Строка `ix` без перевода строки.
    pub fn range(&self, ix: usize) -> Range<usize> {
        let end = self.starts.get(ix + 1).map_or(self.len, |next| next - 1);
        self.starts[ix]..end
    }

    /// Строка, в которой стоит смещение.
    pub fn of(&self, offset: usize) -> usize {
        self.starts.partition_point(|&start| start <= offset) - 1
    }

    /// Самая длинная строка в колонках (табуляция — до следующей кратной 4).
    pub fn widest(&self) -> usize {
        self.widest
    }
}

/// Ширина строки в колонках.
pub fn columns(line: &str) -> usize {
    line.chars().fold(0, |col, c| col + width(c, col))
}

fn width(c: char, col: usize) -> usize {
    if c == '\t' { TAB - col % TAB } else { 1 }
}

/// Строка на экране: табуляция — пробелами до следующей кратной 4 колонки.
pub fn expand(line: &str) -> Cow<'_, str> {
    if !line.contains('\t') {
        return Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len() + TAB);
    let mut col = 0;
    for c in line.chars() {
        let n = width(c, col);
        if c == '\t' {
            out.extend(std::iter::repeat_n(' ', n));
        } else {
            out.push(c);
        }
        col += n;
    }
    Cow::Owned(out)
}

/// Смещение в строке → смещение в строке на экране.
pub fn to_display(line: &str, at: usize) -> usize {
    let mut col = 0;
    let mut shown = 0;
    for (byte, c) in line.char_indices() {
        if byte >= at {
            break;
        }
        let n = width(c, col);
        shown += if c == '\t' { n } else { c.len_utf8() };
        col += n;
    }
    shown
}

/// Смещение на экране → смещение в строке; внутри табуляции — к ближнему её краю.
pub fn from_display(line: &str, at: usize) -> usize {
    let mut col = 0;
    let mut shown = 0;
    for (byte, c) in line.char_indices() {
        let n = width(c, col);
        let size = if c == '\t' { n } else { c.len_utf8() };
        if at < shown + size {
            return if c == '\t' && (at - shown) * 2 > n {
                byte + 1
            } else {
                byte
            };
        }
        shown += size;
        col += n;
    }
    line.len()
}

/// Кусок со словом (буква или цифра), как у поля ввода F6.
fn is_word(part: &str) -> bool {
    part.chars().any(char::is_alphanumeric)
}

/// Граница символа (графемы) слева; в начале строки — перевод строки.
pub fn previous_boundary(text: &str, lines: &Lines, offset: usize) -> usize {
    if offset == 0 {
        return 0;
    }
    let start = lines.range(lines.of(offset)).start;
    if offset == start {
        return offset - 1;
    }
    text[start..offset]
        .grapheme_indices(true)
        .next_back()
        .map_or(start, |(at, _)| start + at)
}

/// Граница символа справа; в конце строки — за переводом строки.
pub fn next_boundary(text: &str, lines: &Lines, offset: usize) -> usize {
    let end = lines.range(lines.of(offset)).end;
    if offset >= end {
        return (offset + 1).min(text.len());
    }
    text[offset..end]
        .graphemes(true)
        .next()
        .map_or(end, |g| offset + g.len())
}

/// Начало слова слева; в начале строки — последнее слово прошлой строки (Ctrl+←).
pub fn previous_word(text: &str, lines: &Lines, offset: usize) -> usize {
    let line = lines.of(offset);
    let mut from = lines.range(line).start;
    if offset == from && line > 0 {
        from = lines.range(line - 1).start;
    }
    text[from..offset]
        .split_word_bound_indices()
        .rev()
        .find(|(_, part)| is_word(part))
        .map_or(from, |(at, _)| from + at)
}

/// Начало следующего слова (Windows: пробелы после слова пропускаются); в конце строки —
/// первое слово следующей (Ctrl+→).
pub fn next_word(text: &str, lines: &Lines, offset: usize) -> usize {
    let line = lines.of(offset);
    let mut to = lines.range(line).end;
    if offset == to && line + 1 < lines.count() {
        to = lines.range(line + 1).end;
    }
    text[offset..to]
        .split_word_bound_indices()
        .find(|(at, part)| *at > 0 && is_word(part))
        .map_or(to, |(at, _)| offset + at)
}

/// Слово под двойным щелчком: кусок между границами слов в строке (за концом — последний).
pub fn word_at(text: &str, lines: &Lines, offset: usize) -> Range<usize> {
    let line = lines.range(lines.of(offset));
    let mut last = offset..offset;
    for (at, part) in text[line.clone()].split_word_bound_indices() {
        last = line.start + at..line.start + at + part.len();
        if last.contains(&offset) {
            break;
        }
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_split_on_line_feeds() {
        let text = "ab\n\tc\n";
        let lines = Lines::new(text);
        assert_eq!(lines.count(), 3);
        assert_eq!(lines.range(0), 0..2);
        assert_eq!(lines.range(1), 3..5);
        assert_eq!(lines.range(2), 6..6);
        assert_eq!(
            (lines.of(0), lines.of(2), lines.of(3), lines.of(6)),
            (0, 0, 1, 2)
        );
        // Табуляция — 4 колонки: «\tc» — 5.
        assert_eq!(lines.widest(), 5);
        assert_eq!(Lines::new("").count(), 1);
    }

    #[test]
    fn tabs_stop_at_multiples_of_four() {
        let line = "a\tb\t\tя";
        assert_eq!(expand(line), "a   b       я");
        assert_eq!(columns(line), 13);
        // «b» после табуляции — 4-я колонка; «я» — 12-я, 2 байта.
        assert_eq!(to_display(line, 2), 4);
        assert_eq!(to_display(line, 5), 12);
        assert_eq!(from_display(line, 4), 2);
        // Внутри табуляции (колонки 1–3) — к ближнему краю.
        assert_eq!(from_display(line, 2), 1);
        assert_eq!(from_display(line, 3), 2);
        assert_eq!(from_display(line, 12), 5);
        assert_eq!(from_display(line, 99), line.len());
        assert!(matches!(expand("abc"), Cow::Borrowed(_)));
    }

    #[test]
    fn steps_cross_line_breaks_like_chromium() {
        let text = "bind w +forward\n\nalias x";
        let lines = Lines::new(text);
        assert_eq!(previous_boundary(text, &lines, 16), 15);
        assert_eq!(next_boundary(text, &lines, 15), 16);
        assert_eq!(next_boundary(text, &lines, text.len()), text.len());
        // Ctrl+← из начала третьей строки — через пустую к «forward».
        assert_eq!(previous_word(text, &lines, 17), 16);
        assert_eq!(previous_word(text, &lines, 16), 8);
        assert_eq!(next_word(text, &lines, 0), 5);
        assert_eq!(next_word(text, &lines, 15), 16);
        assert_eq!(next_word(text, &lines, 16), 17);
        assert_eq!(word_at(text, &lines, 10), 8..15);
    }
}
