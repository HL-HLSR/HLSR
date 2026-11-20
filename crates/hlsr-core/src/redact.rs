//! `redact.js` 0.4.7: `redactText` прячет секреты в тексте уведомления (`NotificationStore.add`)
//! и в отчёте диагностики; с именем пользователя ОС (`userName`) — `redact_text_for`; `safeEnv`
//! и `isSecretEnvName` отбирают переменные окружения для отчёта.
//!
//! Шаблоны те же, но с семантикой регулярных выражений JS без флага `u`:
//! `\b` и `/i` — только ASCII, `\s` — пробелы JS, `\d` — `[0-9]`.
//! Обратная ссылка `\3` (у `regex` их нет) развёрнута в альтернативу.

use crate::js;
use regex::{Captures, NoExpand, Regex};
use std::sync::LazyLock;

const PLACEHOLDER: &str = "<redacted>";

/// В шаблонах `\s` JS записан как `%WS%` (содержимое класса — `crate::js::WHITESPACE_CLASS`).
fn regex(pattern: &str) -> Regex {
    Regex::new(&pattern.replace("%WS%", crate::js::WHITESPACE_CLASS)).expect("шаблон redact.js")
}

static PRIVATE_KEY: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"-----BEGIN [A-Z ]*PRIVATE KEY-----(?s:.)*?-----END [A-Z ]*PRIVATE KEY-----")
});
static HEADER: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"(?-u:\b)((?i-u:Authorization|Cookie|Set-Cookie|Proxy-Authorization))[%WS%]*:[%WS%]*[^\r\n]+",
    )
});
static BEARER: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?-u:\b)Bearer[%WS%]+[A-Za-z0-9._~+/-]+=*"));
// ("?)[^…]+\3 из redact.js: значение в кавычках с обеих сторон или без них.
static KEY_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r#"(?-u:\b)((?i-u:token|secret|password|passwd|pwd|api[_-]?key|access[_-]?token|refresh[_-]?token|credential))(?-u:\b)([%WS%]*[:=][%WS%]*)(?:"[^%WS%"'\r\n,;]+"|[^%WS%"'\r\n,;]+)"#,
    )
});
static GITHUB_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"(?-u:\b)(?:gh[pousr]_[A-Za-z0-9]{16,}|github_pat_[A-Za-z0-9_]{20,})(?-u:\b)")
});
static SSH_KEY: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?-u:\b)ssh-(?:rsa|ed25519)[%WS%]+[A-Za-z0-9+/=]{20,}"));
static URL: LazyLock<Regex> = LazyLock::new(|| regex(r#"(?i-u:https?)://[^%WS%"'<>)\]]+"#));
static SIGNED_QUERY: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"(?:^|[?&])(?i-u:sig|signature|token|access_token|id_token|refresh_token|key|apikey|api_key|password|auth|hash|expires|x-amz-[a-z-]+|se|sp|sv|sr|st)=",
    )
});
static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?-u:\b)[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}(?-u:\b)"));
static STEAM_ID: LazyLock<Regex> = LazyLock::new(|| regex(r"(?-u:\b)7656119[0-9]{10}(?-u:\b)"));
static WINDOWS_USER: LazyLock<Regex> =
    LazyLock::new(|| regex(r#"([A-Za-z]:\\(?i-u:Users)\\)[^\\/\r\n"']+"#));
static MAC_USER: LazyLock<Regex> = LazyLock::new(|| regex(r#"(/(?i-u:Users)/)[^\\/\r\n"']+"#));

/// Текст без ключей, токенов, подписанных ссылок, почты, SteamID и имён
/// пользователя в путях `C:\Users\…` и `/Users/…`.
pub fn redact_text(text: &str) -> String {
    redact_text_for(text, None)
}

/// `redactText(text, { userName })`: то же, а имя пользователя ОС (`userName`) в любом месте
/// текста — например в `/home/alice/…` — заменено на `<user>` (без учёта регистра, как
/// `new RegExp(escaped, "gi")`; пустое имя ничего не меняет). Имя заменяется раньше правил путей
/// `Users`, как в `redactUserPaths`.
pub fn redact_text_for(text: &str, user_name: Option<&str>) -> String {
    let out = redact_secrets(text);
    let out = redact_urls(&out);
    let out = EMAIL.replace_all(&out, NoExpand("<email>"));
    let out = STEAM_ID.replace_all(&out, NoExpand("<steamid>"));
    let out = match user_name.filter(|name| !name.is_empty()) {
        Some(name) => replace_user_name(&out, name),
        None => out.into_owned(),
    };
    let out = WINDOWS_USER.replace_all(&out, "${1}<user>");
    MAC_USER.replace_all(&out, "${1}<user>").into_owned()
}

/// Все вхождения `name` без учёта регистра → `<user>`: `/gi` без флага `u`, то есть сравнение
/// через `Canonicalize` ([`js::canonicalize_ignore_case`]), слева направо без пересечений.
fn replace_user_name(text: &str, name: &str) -> String {
    let needle: Vec<char> = name.chars().map(js::canonicalize_ignore_case).collect();
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let matches = i + needle.len() <= chars.len()
            && chars[i..i + needle.len()]
                .iter()
                .zip(&needle)
                .all(|(c, n)| js::canonicalize_ignore_case(*c) == *n);
        if matches {
            out.push_str("<user>");
            i += needle.len();
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// `ALLOWED_ENV`: переменные окружения, которые попадают в отчёт (остальные не читаются).
pub const ALLOWED_ENV: [&str; 9] = [
    "NUMBER_OF_PROCESSORS",
    "OS",
    "PROCESSOR_ARCHITECTURE",
    "PROCESSOR_IDENTIFIER",
    "SystemDrive",
    "COMPUTERNAME",
    // Linux: игры под Proton ведут себя по-разному в X11 и Wayland.
    // XDG_SESSION_TYPE отсекает SECRET_ENV_RE (SESSION), сессию видно по этим.
    "XDG_CURRENT_DESKTOP",
    "WAYLAND_DISPLAY",
    "DISPLAY",
];

/// `SECRET_ENV_RE`: имя похоже на секрет. `/i` без `u` — только ASCII, поэтому достаточно
/// перевода ASCII в верхний регистр.
const SECRET_ENV_PARTS: [&str; 15] = [
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "PWD",
    "CREDENTIAL",
    "APIKEY",
    "API_KEY",
    "_KEY",
    "KEY_",
    "AUTH",
    "COOKIE",
    "SESSION",
    "SIGNATURE",
    "PRIVATE",
];

/// `isSecretEnvName(name)`.
pub fn is_secret_env_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_ENV_PARTS.iter().any(|part| upper.contains(part))
}

/// `safeEnv(env, options)`: из окружения берутся только имена `allow` (по умолчанию
/// [`ALLOWED_ENV`]), которые в нём есть и не похожи на секрет; значения проходят
/// [`redact_text_for`]. Порядок — как в `allow`. `env` — пары «имя, значение».
pub fn safe_env(
    env: &[(String, String)],
    allow: Option<&[&str]>,
    user_name: Option<&str>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for name in allow.unwrap_or(&ALLOWED_ENV) {
        let Some((_, value)) = env.iter().find(|(key, _)| key == name) else {
            continue;
        };
        if is_secret_env_name(name) {
            continue;
        }
        out.push(((*name).to_owned(), redact_text_for(value, user_name)));
    }
    out
}

fn redact_secrets(text: &str) -> String {
    let out = PRIVATE_KEY.replace_all(
        text,
        NoExpand("-----BEGIN PRIVATE KEY----- <redacted> -----END PRIVATE KEY-----"),
    );
    let out = HEADER.replace_all(&out, |c: &Captures| format!("{}: {PLACEHOLDER}", &c[1]));
    let out = BEARER.replace_all(&out, NoExpand("Bearer <redacted>"));
    let out = KEY_VALUE.replace_all(&out, |c: &Captures| {
        format!("{}{}{PLACEHOLDER}", &c[1], &c[2])
    });
    let out = GITHUB_TOKEN.replace_all(&out, NoExpand(PLACEHOLDER));
    SSH_KEY
        .replace_all(&out, NoExpand("ssh-key <redacted>"))
        .into_owned()
}

/// Подписанная ссылка теряет всю строку запроса.
fn redact_urls(text: &str) -> String {
    URL.replace_all(text, |c: &Captures| {
        let url = &c[0];
        match url.find('?') {
            Some(index) if SIGNED_QUERY.is_match(&url[index..]) => {
                format!("{}?{PLACEHOLDER}", &url[..index])
            }
            _ => url.to_owned(),
        }
    })
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_space_class_is_js_whitespace() {
        let class = regex("^[%WS%]$");
        for c in (0..=0x10FFFF).filter_map(char::from_u32) {
            assert_eq!(
                class.is_match(c.encode_utf8(&mut [0; 4])),
                crate::js::is_whitespace(c),
                "U+{:04X}",
                u32::from(c)
            );
        }
    }
}
