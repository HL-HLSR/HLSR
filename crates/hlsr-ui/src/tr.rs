//! Строки интерфейса (`$t` 0.4.7): таблица из `locales/*.json`, текст как в 0.4.7 (`%0%`).

use hlsr_core::language::Language;

mod strings {
    include!(concat!(env!("OUT_DIR"), "/strings.rs"));
}

/// Строка `key` на языке `lang` с аргументами `%0%`, `%1%`… — как
/// `LocalizationHelper.get` 0.4.7 (`$t` рендерера): нет строки — английская,
/// нет и её — сам ключ (код 0.4.7 проверяет это, `DownloadBar.vue:393-397`).
///
/// Аргументы подставляются по очереди, как `_parse`: `str.replace("%i%",
/// args[i])` для i = 0, 1, … — только первое вхождение, в уже подставленном
/// тексте, со спецсимволами замены JS (`$$`, `$&`, `` $` ``, `$'`). Аргумента
/// нет — плейсхолдер остаётся.
pub fn tr(lang: Language, key: &str, args: &[&str]) -> String {
    let table = match lang {
        Language::En => strings::EN,
        Language::Ru => strings::RU,
    };
    match lookup(table, key).or_else(|| lookup(strings::EN, key)) {
        Some(text) => parse(text, args),
        None => key.to_owned(),
    }
}

fn lookup(table: &'static [(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table
        .binary_search_by(|(known, _)| (*known).cmp(key))
        .ok()
        .map(|index| table[index].1)
}

/// `LocalizationHelper._parse(str, vars)`.
fn parse(text: &str, args: &[&str]) -> String {
    let mut out = text.to_owned();
    for (index, arg) in args.iter().enumerate() {
        out = replace_first(&out, &format!("%{index}%"), arg);
    }
    out
}

/// `String.prototype.replace` со строкой-образцом: первое вхождение, замена по
/// GetSubstitution (без групп: `$1` и `$<` остаются как есть).
fn replace_first(haystack: &str, pattern: &str, replacement: &str) -> String {
    let Some(position) = haystack.find(pattern) else {
        return haystack.to_owned();
    };
    let after = position + pattern.len();
    let mut out = String::with_capacity(haystack.len() + replacement.len());
    out.push_str(&haystack[..position]);
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('$') => {
                chars.next();
                out.push('$');
            }
            Some('&') => {
                chars.next();
                out.push_str(pattern);
            }
            Some('`') => {
                chars.next();
                out.push_str(&haystack[..position]);
            }
            Some('\'') => {
                chars.next();
                out.push_str(&haystack[after..]);
            }
            _ => out.push('$'),
        }
    }
    out.push_str(&haystack[after..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_replaces_first_occurrence_in_order() {
        assert_eq!(parse("%0% и %0%", &["a"]), "a и %0%");
        assert_eq!(parse("%0% %1%", &["x"]), "x %1%");
        // Подстановка идёт по уже заменённому тексту.
        assert_eq!(parse("%0% %1%", &["%1%", "y"]), "y %1%");
        assert_eq!(parse("%1% %0%", &["a", "b"]), "b a");
        assert_eq!(parse("без", &["a"]), "без");
    }

    #[test]
    fn replacement_patterns_are_js() {
        assert_eq!(replace_first("a%0%b", "%0%", "$$"), "a$b");
        assert_eq!(replace_first("a%0%b", "%0%", "[$&]"), "a[%0%]b");
        assert_eq!(replace_first("a%0%b", "%0%", "$`"), "aab");
        assert_eq!(replace_first("a%0%b", "%0%", "$'"), "abb");
        assert_eq!(replace_first("a%0%b", "%0%", "$1$<x>$"), "a$1$<x>$b");
    }

    #[test]
    fn placeholders_and_braces_stay_as_in_0_4_7() {
        assert!(tr(Language::En, "#L_D_ERR_NO_SPACE", &[]).len() > 1);
        assert_eq!(tr(Language::Ru, "#НЕТ_КЛЮЧА", &["a"]), "#НЕТ_КЛЮЧА");
        let with_arg = strings::EN
            .iter()
            .find(|(_, text)| text.contains("%0%"))
            .expect("строка с %0%");
        assert!(!tr(Language::En, with_arg.0, &["Z"]).contains("%0%"));
        let with_brace = strings::EN.iter().find(|(_, text)| text.contains('{'));
        if let Some((key, text)) = with_brace {
            assert_eq!(tr(Language::En, key, &[]), *text, "скобки как в 0.4.7");
        }
    }

    #[test]
    fn tables_are_sorted_for_binary_search() {
        for table in [strings::EN, strings::RU] {
            assert!(table.windows(2).all(|pair| pair[0].0 < pair[1].0));
        }
    }
}
