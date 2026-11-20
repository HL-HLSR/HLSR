//! Местоимения профиля (`shared/pronouns.js`, сервер — `Profile/Customization.php`): только из
//! списка; на сервере хранится id, подпись — на языке того, кто смотрит.

/// `PRONOUNS`: id, подпись ru, подпись en.
pub const PRONOUNS: [(&str, &str, &str); 7] = [
    ("he", "он/его", "he/him"),
    ("she", "она/её", "she/her"),
    ("they", "они/их", "they/them"),
    ("he-they", "он/они", "he/they"),
    ("she-they", "она/они", "she/they"),
    ("any", "любые", "any pronouns"),
    ("ask", "спросите меня", "ask me"),
];

/// `normalizePronouns(value)`: id из списка или `""` («не указано»).
pub fn normalize_pronouns(value: Option<&str>) -> &'static str {
    value
        .and_then(|value| PRONOUNS.iter().find(|(id, _, _)| *id == value))
        .map_or("", |(id, _, _)| id)
}

fn label(locale: &str, ru: &'static str, en: &'static str) -> &'static str {
    if locale == "ru" { ru } else { en }
}

/// `pronounLabel(value, locale)`: подпись или `""`.
pub fn pronoun_label(value: Option<&str>, locale: &str) -> &'static str {
    value
        .and_then(|value| PRONOUNS.iter().find(|(id, _, _)| *id == value))
        .map_or("", |(_, ru, en)| label(locale, ru, en))
}

/// `pronounList(locale)`: `(id, подпись)` для выбора в настройках.
pub fn pronoun_list(locale: &str) -> Vec<(&'static str, &'static str)> {
    PRONOUNS
        .iter()
        .map(|(id, ru, en)| (*id, label(locale, ru, en)))
        .collect()
}
