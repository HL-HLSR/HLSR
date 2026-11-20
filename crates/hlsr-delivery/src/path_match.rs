//! Шаблоны путей (`path-match.js`): точный путь, префикс каталога или маска
//! с `*` и `**`, без учёта регистра.

use hlsr_core::js;
use std::collections::HashSet;

/// `normalize`: `\` → `/`, без одного начального `./` и без `/` в конце.
pub fn normalize(relative_path: &str) -> String {
    let path = relative_path.replace('\\', "/");
    let path = path.strip_prefix("./").unwrap_or(&path);
    path.trim_end_matches('/').to_owned()
}

/// Часть маски, как её переводит `toRegExp`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// Символ как есть (флаг `i` регулярного выражения).
    Char(char),
    /// `*` → `[^/]*`.
    Star,
    /// `**` → `.*` (без переводов строк, как `.` в JS).
    AnyDepth,
    /// `**/` → `(?:[^/]+/)*`.
    Folders,
}

/// Маска `toRegExp`: `^…$` с флагом `i` без `u`.
#[derive(Debug, Clone)]
struct Mask(Vec<Token>);

impl Mask {
    fn new(pattern: &str) -> Mask {
        let chars: Vec<char> = normalize(pattern).chars().collect();
        let mut tokens = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] != '*' {
                tokens.push(Token::Char(chars[i]));
                i += 1;
            } else if chars.get(i + 1) != Some(&'*') {
                tokens.push(Token::Star);
                i += 1;
            } else if chars.get(i + 2) == Some(&'/') {
                tokens.push(Token::Folders);
                i += 3;
            } else {
                tokens.push(Token::AnyDepth);
                i += 2;
            }
        }
        Mask(tokens)
    }

    fn test(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        matches(&self.0, &chars)
    }
}

/// `.` в регулярном выражении JS не берёт переводы строк.
fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn matches(tokens: &[Token], text: &[char]) -> bool {
    let Some((token, rest)) = tokens.split_first() else {
        return text.is_empty();
    };
    match token {
        Token::Char(expected) => text.first().is_some_and(|&c| {
            js::canonicalize_ignore_case(c) == js::canonicalize_ignore_case(*expected)
                && matches(rest, &text[1..])
        }),
        Token::Star => {
            let run = text.iter().take_while(|&&c| c != '/').count();
            (0..=run).any(|taken| matches(rest, &text[taken..]))
        }
        Token::AnyDepth => {
            let run = text.iter().take_while(|&&c| !is_line_terminator(c)).count();
            (0..=run).any(|taken| matches(rest, &text[taken..]))
        }
        Token::Folders => {
            if matches(rest, text) {
                return true;
            }
            // Ещё одна папка: один и больше символов без `/`, затем `/`.
            let name = text.iter().take_while(|&&c| c != '/').count();
            name > 0 && text.get(name) == Some(&'/') && matches(tokens, &text[name + 1..])
        }
    }
}

/// `createMatcher(patterns)`: путь подходит, если совпадает с шаблоном без `*`,
/// лежит внутри такого каталога или подходит под маску. Регистр не важен.
#[derive(Debug, Clone)]
pub struct Matcher {
    patterns: Vec<String>,
    exact: HashSet<String>,
    prefixes: Vec<String>,
    masks: Vec<Mask>,
}

impl Matcher {
    pub fn new<S: AsRef<str>>(patterns: &[S]) -> Matcher {
        let patterns: Vec<String> = patterns
            .iter()
            .map(|p| normalize(p.as_ref()))
            .filter(|p| !p.is_empty())
            .collect();
        let exact: HashSet<String> = patterns
            .iter()
            .filter(|p| !p.contains('*'))
            .map(|p| p.to_lowercase())
            .collect();
        let prefixes = exact.iter().map(|p| format!("{p}/")).collect();
        let masks = patterns
            .iter()
            .filter(|p| p.contains('*'))
            .map(|p| Mask::new(p))
            .collect();
        Matcher {
            patterns,
            exact,
            prefixes,
            masks,
        }
    }

    /// `match(relativePath)`.
    pub fn matches(&self, relative_path: &str) -> bool {
        let value = normalize(relative_path).to_lowercase();
        if value.is_empty() {
            return false;
        }
        self.exact.contains(&value)
            || self
                .prefixes
                .iter()
                .any(|prefix| value.starts_with(prefix.as_str()))
            || self.masks.iter().any(|mask| mask.test(&value))
    }

    /// `match.patterns`: нормализованные непустые шаблоны.
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_follow_to_reg_exp() {
        let m = Matcher::new(&["**/*.dem", "Half-Life/valve_WON/*.bmp", "a/**"]);
        assert!(m.matches("x.dem"));
        assert!(m.matches("A/B/C.DEM"));
        assert!(m.matches("half-life/valve_won/X.BMP"));
        assert!(!m.matches("Half-Life/valve_WON/a/x.bmp"));
        assert!(m.matches("a/x"));
        assert!(!m.matches("a/x\ny"), "`.` не берёт перевод строки");
    }
}
