//! Запрет ссылок в описании профиля (`shared/link-guard.js`, сервер — `Profile/LinkGuard.php`):
//! `containsLink` ловит схемы, `www.`, IP, punycode, «site.com», «site . com», «site [dot] com»,
//! «site dot com», «site точка ком», полноширинные и «китайские» точки, буквы через пробел,
//! похожие кириллические и греческие буквы, невидимые символы и надстрочные знаки.
//!
//! Регулярки JS с флагом `u`. Просмотр вперёд в шаблонах проверки (`(?!…)` в конце) заменён
//! равносильной поглощающей частью: для `test` важно только, есть ли совпадение. Две замены с
//! просмотром вперёд (`dot` между пробелами и склейка букв через пробел) сделаны вручную.

use hlsr_core::js;
use icu_normalizer::DecomposingNormalizerBorrowed;
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::LazyLock;

/// Домены верхнего уровня, которые ловятся в записи без пробелов (site.com).
const TLDS: &str = "com net org edu gov info biz pro name mobi xyz top site online live link click club shop store app dev \
page blog tech space fun icu buzz win bid stream chat social group team zone world today news media email cloud \
host website web run games game wiki ink ly gg tv me io co cc fm am ai sh ms la vc gl gd ws pw tk ml ga cf gq to \
su ru ua by kz uz kg tj tm az am ge md lt lv ee pl cz sk hu ro bg rs hr si ba mk al gr tr il de at ch fr be nl lu \
uk ie es pt it dk no se fi is us ca mx br ar cl pe ve jp cn hk tw kr in id my sg th vn ph au nz za eg ng ke \
рф рус ру ком орг сайт онлайн москва дети";

/// Те, что ловятся и с пробелами или словами вместо точки (без коротких английских слов).
const STRONG: &str = "com net org info biz pro xyz top site online link club shop store app dev ly gg tv io cc fm ws pw tk \
ml ga cf gq su ru ua kz рф рус ру ком орг сайт онлайн";

/// Кириллица и греческие буквы, неотличимые от латинских.
const HOMOGLYPHS: [(char, char); 37] = [
    ('а', 'a'),
    ('в', 'b'),
    ('е', 'e'),
    ('ё', 'e'),
    ('к', 'k'),
    ('м', 'm'),
    ('н', 'h'),
    ('о', 'o'),
    ('р', 'p'),
    ('с', 'c'),
    ('т', 't'),
    ('у', 'y'),
    ('х', 'x'),
    ('і', 'i'),
    ('ї', 'i'),
    ('ј', 'j'),
    ('ѕ', 's'),
    ('ԁ', 'd'),
    ('ԛ', 'q'),
    ('ԝ', 'w'),
    ('ү', 'y'),
    ('һ', 'h'),
    ('ɡ', 'g'),
    ('ı', 'i'),
    ('α', 'a'),
    ('β', 'b'),
    ('ε', 'e'),
    ('η', 'n'),
    ('ι', 'i'),
    ('κ', 'k'),
    ('ν', 'v'),
    ('ο', 'o'),
    ('ρ', 'p'),
    ('τ', 't'),
    ('υ', 'u'),
    ('χ', 'x'),
    ('ω', 'w'),
];

fn regex(pattern: &str) -> Regex {
    Regex::new(&pattern.replace("%WS%", js::WHITESPACE_CLASS)).expect("шаблон link-guard")
}

fn alternation(list: &str) -> String {
    list.split(' ')
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join("|")
}

/// `END` = `(?![\p{L}\p{N}]|-[\p{L}\p{N}])` в поглощающем виде.
const END: &str = r"(?:$|[^\p{L}\p{N}\-]|-(?:$|[^\p{L}\p{N}]))";

static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let tlds = alternation(TLDS);
    let strong = alternation(STRONG);
    vec![
        // Схема: http://, hxxps://, steam:// и т. п.
        regex(r"(?:^|[^a-z0-9])[a-z][a-z0-9+.\-]{1,15}[%WS%]*:[%WS%]*/[%WS%]*/"),
        regex(r"(?:^|[^a-z0-9])www[%WS%]*\."),
        regex(r"xn--"),
        // `(?![0-9])` в конце.
        regex(r"(?:^|[^0-9])[0-9]{1,3}(?:[%WS%]*\.[%WS%]*[0-9]{1,3}){3}(?:$|[^0-9])"),
        // site.com, site.com/путь
        regex(&format!(r"[\p{{L}}\p{{N}}]\.(?:{tlds}){END}")),
        // site . com, site .com, site. com (последнее — только «сильные» зоны)
        regex(&format!(r"[\p{{L}}\p{{N}}][%WS%]+\.[%WS%]*(?:{tlds}){END}")),
        regex(&format!(r"[\p{{L}}\p{{N}}]\.[%WS%]+(?:{strong}){END}")),
        // Короткие адреса приглашений даже без точки: discord gg/xxx, t me/xxx.
        regex(
            r"(?:^|[^a-z0-9])(?:discord[%WS%]*\.?[%WS%]*gg|t[%WS%]*\.?[%WS%]*me|bit[%WS%]*\.?[%WS%]*ly|vk[%WS%]*\.?[%WS%]*cc|clck[%WS%]*\.?[%WS%]*ru)[%WS%]*/[%WS%]*[a-z0-9_]",
        ),
    ]
});

static INVISIBLE: LazyLock<Regex> =
    LazyLock::new(|| regex(r"[\p{M}\p{Cf}\x{115f}\x{1160}\x{3164}\x{ffa0}\x{2800}]"));
static BRACKET_DOT: LazyLock<Regex> =
    LazyLock::new(|| regex(r"[%WS%]*[\[({<][%WS%]*(?:dot|точка|\.)[%WS%]*[\])}>][%WS%]*"));
static WORD_DOT: LazyLock<Regex> = LazyLock::new(|| regex(r"(^|[%WS%])(?:dot|точка|тчк)"));

/// `plainLetter(cp)`: стилизованные латинские буквы и цифры → обычные.
fn plain_letter(c: char) -> Option<char> {
    let cp = c as u32;
    let az = |n: u32| char::from_u32(97 + n);
    match cp {
        0xff01..=0xff5e => char::from_u32(cp - 0xfee0),
        0x24b6..=0x24cf => az(cp - 0x24b6),
        0x24d0..=0x24e9 => az(cp - 0x24d0),
        0x1d400..=0x1d6a3 => az((cp - 0x1d400) % 52 % 26),
        0x1d7ce..=0x1d7ff => char::from_digit((cp - 0x1d7ce) % 10, 10),
        0x1f130..=0x1f189 => az((cp - 0x1f130) % 26),
        0x1f1e6..=0x1f1ff => az(cp - 0x1f1e6),
        _ => None,
    }
}

/// `.replace(/(^|\s)(?:dot|точка|тчк)(?=\s)/gu, "$1.")`.
fn replace_word_dots(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut pos = 0;
    while pos <= text.len() {
        let Some(found) = WORD_DOT.captures_at(text, pos) else {
            break;
        };
        let whole = found.get(0).expect("совпадение");
        if text[whole.end()..]
            .chars()
            .next()
            .is_some_and(js::is_whitespace)
        {
            out.push_str(&text[last..whole.start()]);
            out.push_str(&found[1]);
            out.push('.');
            last = whole.end();
            pos = whole.end();
        } else {
            // Совпадение без пробела после — следующая попытка со следующего символа.
            pos = whole.start()
                + text[whole.start()..]
                    .chars()
                    .next()
                    .map_or(1, char::len_utf8);
        }
    }
    out.push_str(&text[last..]);
    out
}

/// `fold(text)`: стилизованные буквы, NFKD, без невидимых и надстрочных знаков, нижний регистр,
/// точки-заменители → «.», «дробная» косая → «/».
pub fn fold(text: &str) -> String {
    let plain: String = text.chars().map(|c| plain_letter(c).unwrap_or(c)).collect();
    let decomposed = DecomposingNormalizerBorrowed::new_nfkd().normalize(&plain);
    let visible = INVISIBLE.replace_all(&decomposed, "");
    let lower: String = visible
        .to_lowercase()
        .chars()
        .map(|c| match c {
            '\u{3002}' | '\u{ff0e}' | '\u{ff61}' | '\u{2024}' | '\u{2027}' | '\u{00b7}'
            | '\u{2022}' | '\u{2219}' | '\u{22c5}' | '\u{30fb}' => '.',
            other => other,
        })
        .collect();
    let bracketed = BRACKET_DOT.replace_all(&lower, ".");
    replace_word_dots(&bracketed)
        .chars()
        .map(|c| match c {
            '\u{2044}' | '\u{2215}' | '\u{29f8}' => '/',
            other => other,
        })
        .collect()
}

static WORD: LazyLock<Regex> = LazyLock::new(|| regex(r"^[\p{L}\p{N}]$"));

/// `[\p{L}\p{N}]`.
fn is_word(c: char) -> bool {
    let mut buf = [0u8; 4];
    WORD.is_match(c.encode_utf8(&mut buf))
}

/// `squeeze(text)`: «s i t e . c o m» → «site . com» — одиночные буквы через пробел склеиваются
/// (`/(?:[\p{L}\p{N}] ){2,}[\p{L}\p{N}](?![\p{L}\p{N}])/gu`).
pub fn squeeze(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < n {
        // Пары «буква пробел» подряд от i.
        let mut pairs = 0;
        while i + 2 * pairs + 1 < n
            && is_word(chars[i + 2 * pairs])
            && chars[i + 2 * pairs + 1] == ' '
        {
            pairs += 1;
        }
        // Жадно: больше пар, затем меньше — пока за последней буквой не окажется не-буква.
        let end = (2..=pairs).rev().find_map(|count| {
            let last = i + 2 * count;
            (last < n && is_word(chars[last]) && chars.get(last + 1).is_none_or(|c| !is_word(*c)))
                .then_some(last)
        });
        match end {
            Some(last) => {
                out.extend(chars[i..=last].iter().filter(|c| **c != ' '));
                i = last + 1;
            }
            None => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

/// `latin(text)`: похожие кириллические и греческие буквы → латинские.
pub fn latin(text: &str) -> String {
    text.chars()
        .map(|c| {
            let cp = c as u32;
            let in_range =
                (0x0250..=0x02af).contains(&cp) || cp == 0x0131 || (0x0370..=0x052f).contains(&cp);
            if in_range {
                HOMOGLYPHS
                    .iter()
                    .find(|(from, _)| *from == c)
                    .map_or(c, |(_, to)| *to)
            } else {
                c
            }
        })
        .collect()
}

/// `containsLink(value)`: есть ли в тексте ссылка (в любом из видов разбора).
pub fn contains_link(value: &Value) -> bool {
    let Some(text) = value.as_str().filter(|text| !js::trim(text).is_empty()) else {
        return false;
    };
    let folded = fold(text);
    let squeezed = squeeze(&folded);
    let variants: HashSet<String> = [latin(&folded), latin(&squeezed), folded, squeezed]
        .into_iter()
        .collect();
    variants
        .iter()
        .any(|text| PATTERNS.iter().any(|pattern| pattern.is_match(text)))
}
