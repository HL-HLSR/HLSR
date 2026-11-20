//! Ссылки профиля на YouTube- и Twitch-канал (`shared/profile-links.js`, сервер —
//! `Profile/Customization.php`): ввод «как удобно» → полный `https`-адрес канала; всё прочее
//! отклоняется ещё до сохранения.

use hlsr_core::js;
use serde_json::Value;
use url::Url;

pub const YOUTUBE_HOSTS: [&str; 3] = ["youtube.com", "www.youtube.com", "m.youtube.com"];
pub const TWITCH_HOSTS: [&str; 3] = ["twitch.tv", "www.twitch.tv", "m.twitch.tv"];

/// Служебные страницы Twitch, а не каналы.
pub const TWITCH_RESERVED: [&str; 18] = [
    "directory",
    "videos",
    "settings",
    "subscriptions",
    "inventory",
    "wallet",
    "drops",
    "search",
    "downloads",
    "jobs",
    "p",
    "store",
    "turbo",
    "prime",
    "login",
    "signup",
    "following",
    "friends",
];

fn chars_in(text: &str, min: usize, max: usize, extra: &[u8]) -> bool {
    (min..=max).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
}

/// `YOUTUBE_HANDLE`: 3–30 знаков: буквы, цифры, `_`, `-`, `.`.
fn is_youtube_handle(text: &str) -> bool {
    chars_in(text, 3, 30, b"_.-")
}

/// `YOUTUBE_CHANNEL_ID`: `UC` и 22 знака.
fn is_youtube_channel_id(text: &str) -> bool {
    text.strip_prefix("UC")
        .is_some_and(|rest| chars_in(rest, 22, 22, b"_-"))
}

/// `YOUTUBE_LEGACY`: 1–100 знаков.
fn is_youtube_legacy(text: &str) -> bool {
    chars_in(text, 1, 100, b"_.-")
}

/// `TWITCH_NAME`: до 25 знаков: буквы, цифры, `_`.
fn is_twitch_name(text: &str) -> bool {
    chars_in(text, 1, 25, b"_")
}

/// `^[a-z][a-z0-9+.-]*://` без учёта регистра.
fn has_scheme(text: &str) -> bool {
    let bytes = text.as_bytes();
    let Some(first) = bytes.first() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    let rest = bytes[1..]
        .iter()
        .position(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-')))
        .map_or(&bytes[bytes.len()..], |end| &bytes[1 + end..]);
    rest.starts_with(b"://")
}

/// `decodeURIComponent(part)`; ошибка разбора — `None`.
pub fn decode_uri_component(part: &str) -> Option<String> {
    let bytes = part.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let text = std::str::from_utf8(hex).ok()?;
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            out.push(u8::from_str_radix(text, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Части пути адреса `https` или `http` на одном из `hosts` (без имени, пароля и порта).
fn parse(value: &str, hosts: &[&str]) -> Option<Vec<String>> {
    let text = js::trim(value);
    if text.is_empty() {
        return None;
    }
    let with_scheme = if has_scheme(text) {
        text.to_owned()
    } else {
        format!("https://{text}")
    };
    let url = Url::parse(&with_scheme).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || !url.username().is_empty()
        || url.password().is_some_and(|p| !p.is_empty())
        || url.port().is_some()
    {
        return None;
    }
    let host = url.host_str().unwrap_or("").to_lowercase();
    if !hosts.contains(&host.as_str()) {
        return None;
    }
    Some(
        url.path()
            .split('/')
            .filter(|part| !part.is_empty())
            .map(|part| decode_uri_component(part).unwrap_or_else(|| part.to_owned()))
            .collect(),
    )
}

fn trimmed(value: &Value) -> &str {
    value.as_str().map_or("", js::trim)
}

/// `normalizeYoutube(value)`: адрес канала, `Some("")` для пустого поля или `None`, если это не
/// канал YouTube.
pub fn normalize_youtube(value: &Value) -> Option<String> {
    let text = trimmed(value);
    if text.is_empty() {
        return Some(String::new());
    }
    if text.starts_with('@')
        || (!text.contains('/') && !YOUTUBE_HOSTS.contains(&text.to_lowercase().as_str()))
    {
        let handle = text.strip_prefix('@').unwrap_or(text);
        return is_youtube_handle(handle).then(|| format!("https://www.youtube.com/@{handle}"));
    }
    let parts = parse(text, &YOUTUBE_HOSTS).filter(|parts| !parts.is_empty())?;
    let first = parts[0].as_str();
    let second = parts.get(1).map_or("", String::as_str);
    if let Some(handle) = first.strip_prefix('@') {
        return is_youtube_handle(handle).then(|| format!("https://www.youtube.com/@{handle}"));
    }
    if first == "channel" && is_youtube_channel_id(second) {
        return Some(format!("https://www.youtube.com/channel/{second}"));
    }
    if (first == "c" || first == "user") && is_youtube_legacy(second) {
        return Some(format!("https://www.youtube.com/{first}/{second}"));
    }
    None
}

fn twitch_reserved(name: &str) -> bool {
    TWITCH_RESERVED.contains(&name.to_lowercase().as_str())
}

/// `normalizeTwitch(value)`: адрес канала, `Some("")` или `None`.
pub fn normalize_twitch(value: &Value) -> Option<String> {
    let text = trimmed(value);
    if text.is_empty() {
        return Some(String::new());
    }
    let name = text.strip_prefix('@').unwrap_or(text);
    if is_twitch_name(name) {
        return (!twitch_reserved(name)).then(|| format!("https://www.twitch.tv/{name}"));
    }
    let parts = parse(text, &TWITCH_HOSTS)?;
    let first = parts.first().filter(|first| is_twitch_name(first))?;
    (!twitch_reserved(first)).then(|| format!("https://www.twitch.tv/{first}"))
}

/// `normalize(field, value)`: `youtube` или `twitch`; другое поле — `None`.
pub fn normalize(field: &str, value: &Value) -> Option<String> {
    match field {
        "youtube" => normalize_youtube(value),
        "twitch" => normalize_twitch(value),
        _ => None,
    }
}

/// `channelName(field, value)`: ник канала для подписи; у канала по ID (`UC…`) — `None`.
pub fn channel_name(field: &str, value: &Value) -> Option<String> {
    let url = normalize(field, value).filter(|url| !url.is_empty())?;
    let url = Url::parse(&url).ok()?;
    let parts: Vec<&str> = url.path().split('/').filter(|p| !p.is_empty()).collect();
    let first = *parts.first()?;
    if field == "twitch" || first.starts_with('@') {
        return Some(first.to_owned());
    }
    if first == "c" || first == "user" {
        return parts.get(1).map(|second| (*second).to_owned());
    }
    None
}
