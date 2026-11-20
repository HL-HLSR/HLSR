//! Версии (`semver.js`): `X.Y.Z` с необязательным `-пререлизом`; пререлизы
//! сравниваются строкой (`<` JS), а не по правилам SemVer.

use hlsr_core::js;
use std::cmp::Ordering;

/// Разобранная версия. Числа — как `Number(…)` в JS.
#[derive(Debug, Clone, PartialEq)]
pub struct Version {
    pub major: f64,
    pub minor: f64,
    pub patch: f64,
    pub prerelease: Option<String>,
}

/// `parse(version)`: `/^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?/` — продолжение
/// после версии допускается (`1.0.0extra`, `1.0.0+build`).
pub fn parse(version: &str) -> Option<Version> {
    fn digits(text: &str) -> (&str, &str) {
        let end = text
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(text.len());
        text.split_at(end)
    }
    fn number(text: &str) -> Option<(f64, &str)> {
        let (digits, rest) = digits(text);
        (!digits.is_empty()).then(|| (js::to_number(digits), rest))
    }
    let (major, rest) = number(version)?;
    let (minor, rest) = number(rest.strip_prefix('.')?)?;
    let (patch, rest) = number(rest.strip_prefix('.')?)?;
    let prerelease = rest.strip_prefix('-').and_then(|tail| {
        let end = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'))
            .unwrap_or(tail.len());
        (end > 0).then(|| tail[..end].to_owned())
    });
    Some(Version {
        major,
        minor,
        patch,
        prerelease,
    })
}

/// `compare(a, b)`: неразбираемая версия меньше любой разбираемой.
pub fn compare(a: &str, b: &str) -> Ordering {
    let (left, right) = match (parse(a), parse(b)) {
        (None, None) => return Ordering::Equal,
        (None, Some(_)) => return Ordering::Less,
        (Some(_), None) => return Ordering::Greater,
        (Some(left), Some(right)) => (left, right),
    };
    for (l, r) in [
        (left.major, right.major),
        (left.minor, right.minor),
        (left.patch, right.patch),
    ] {
        if l != r {
            return if l < r {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
    }
    match (&left.prerelease, &right.prerelease) {
        (l, r) if l == r => Ordering::Equal,
        (None, _) => Ordering::Greater,
        (_, None) => Ordering::Less,
        (Some(l), Some(r)) => {
            if js::cmp_utf16(l, r).is_lt() {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        }
    }
}

/// `lt(a, b)`.
pub fn lt(a: &str, b: &str) -> bool {
    compare(a, b).is_lt()
}

/// `gte(a, b)`.
pub fn gte(a: &str, b: &str) -> bool {
    compare(a, b).is_ge()
}
