//! Скрипты гайда hl1-scripts для вкладки «Скрипты» Half-Life 1 (`shared/hl1-guide.js`).
//!
//! Данные собирает `scripts/import-hl1-guide.js` 0.4.7 из README гайда
//! (зафиксированный коммит) в `hl1-guide-data.json`: alias скриптов уже с
//! префиксами, чтобы скрипты разных авторов не перетирали друг друга. Файлы
//! лежат в крейте байт в байт, как в 0.4.7. Скрипт назначается так же, как любая
//! заготовка клавиатуры: на клавишу ставится одно из его действий, а alias
//! лаунчер пишет в hlsr-binds.cfg, только пока хотя бы одно действие скрипта
//! назначено.

use hlsr_core::js;
use hlsr_core::js::object::JsObject;
use hlsr_core::json_text::stringify;
use regress::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// В alias вида «sensitivity 2.5» гайд просит вписать свою чувствительность.
/// Импорт заменяет число на этот токен, а лаунчер перед запуском подставляет
/// значение sensitivity из config.cfg игрока.
pub const SENSITIVITY_TOKEN: &str = "{sensitivity}";
/// Значение по умолчанию в Half-Life.
pub const DEFAULT_SENSITIVITY: &str = "3";
/// То же для m_pitch: скрипты Xen обнуляют его на время трюка, а потом возвращают
/// значение игрока, со знаком инверсии мыши.
pub const M_PITCH_TOKEN: &str = "{m_pitch}";
pub const DEFAULT_M_PITCH: &str = "0.022";

/// Глобальный alias, который выполняет game.cfg при каждой загрузке карты или
/// сохранения. Скрипты с `needs_auto_alias` меняют его перед сменой уровня.
pub const AUTO_ALIAS: &str = "auto_alias";
pub const GAME_CFG: &str = "game.cfg";

/// Данные гайда, вшитые в крейт: те же файлы, что `src/shared/` 0.4.7.
pub const DATA_JSON: &str = include_str!("../data/hl1-guide-data.json");
/// Перевод описаний (`notes`, `usage`) на русский по id скрипта. У каждого
/// перевода хеш оригинала: после нового импорта гайда изменившийся текст
/// показывается без перевода, пока перевод не обновят.
pub const RU_JSON: &str = include_str!("../data/hl1-guide-ru.json");

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Source {
    pub repo: String,
    pub commit: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Chapter {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Video {
    pub id: String,
    pub start: u32,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Action {
    pub command: String,
    #[serde(rename = "guideKey")]
    pub guide_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Script {
    pub id: String,
    pub chapter: String,
    pub section: String,
    pub title: String,
    pub mark: Option<String>,
    pub credit: Option<String>,
    pub notes: Vec<String>,
    pub usage: String,
    pub video: Option<Video>,
    pub save: Option<String>,
    pub namespace: Option<String>,
    pub aliases: Vec<(String, String)>,
    pub commands: Vec<Value>,
    pub actions: Vec<Action>,
    #[serde(rename = "needsAutoAlias")]
    pub needs_auto_alias: bool,
}

/// Описание скрипта на другом языке.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Translation {
    pub notes: Vec<String>,
    pub usage: String,
}

#[derive(Deserialize)]
struct Data {
    source: Source,
    chapters: Vec<Chapter>,
    scripts: Vec<Script>,
}

#[derive(Deserialize)]
struct RuItem {
    hash: String,
    notes: Vec<String>,
    usage: String,
}

#[derive(Deserialize)]
struct RuTable {
    scripts: HashMap<String, RuItem>,
}

static DATA: LazyLock<Data> =
    LazyLock::new(|| serde_json::from_str(DATA_JSON).expect("hl1-guide-data.json разбирается"));
static RU: LazyLock<RuTable> =
    LazyLock::new(|| serde_json::from_str(RU_JSON).expect("hl1-guide-ru.json разбирается"));
static BY_ID: LazyLock<HashMap<&'static str, &'static Script>> = LazyLock::new(|| {
    DATA.scripts
        .iter()
        .map(|script| (script.id.as_str(), script))
        .collect()
});

pub fn source() -> &'static Source {
    &DATA.source
}

pub fn chapters() -> &'static [Chapter] {
    &DATA.chapters
}

pub fn scripts() -> &'static [Script] {
    &DATA.scripts
}

/// Скрипт по id; чужой id — `None`.
pub fn script_by_id(id: &str) -> Option<&'static Script> {
    BY_ID.get(id).copied()
}

fn js_regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("выражение 0.4.7 разбирается")
}

static SENSITIVITY: LazyLock<Regex> = LazyLock::new(|| js_regex(r"^\d{1,3}(\.\d{1,6})?$"));
static PITCH: LazyLock<Regex> = LazyLock::new(|| js_regex(r"^-?\d{1,3}(\.\d{1,6})?$"));

/// Число из config.cfg или значение по умолчанию: в alias попадает только
/// десятичное число, а не произвольный текст. Значения в cfg всегда строки,
/// всё остальное (в 0.4.7 — не строка и не число) — `None`.
pub fn clean_sensitivity(value: Option<&str>) -> String {
    let text = value.map_or("", js::trim);
    if SENSITIVITY.find(text).is_some() && js::to_number(text) > 0.0 {
        text.to_owned()
    } else {
        DEFAULT_SENSITIVITY.to_owned()
    }
}

pub fn clean_pitch(value: Option<&str>) -> String {
    let text = value.map_or("", js::trim);
    if PITCH.find(text).is_some() && js::to_number(text) != 0.0 {
        text.to_owned()
    } else {
        DEFAULT_M_PITCH.to_owned()
    }
}

/// Подставляет вместо токена чувствительность игрока.
pub fn with_sensitivity(value: &str, sensitivity: Option<&str>) -> String {
    value.replace(SENSITIVITY_TOKEN, &clean_sensitivity(sensitivity))
}

/// В alias есть значение, которое лаунчер подставит из cfg игрока.
pub fn has_player_token(value: &str) -> bool {
    value.contains(SENSITIVITY_TOKEN) || value.contains(M_PITCH_TOKEN)
}

/// Подставляет в alias значения игрока; `cvars` — переменные из его cfg.
pub fn with_player_cvars(value: &str, cvars: &JsObject<String>) -> String {
    with_sensitivity(value, cvars.get("sensitivity").map(String::as_str)).replace(
        M_PITCH_TOKEN,
        &clean_pitch(cvars.get("m_pitch").map(String::as_str)),
    )
}

/// Ссылки, которые вкладка «Скрипты» может открыть в браузере: страница гайда,
/// сохранения и ссылки из описаний, видео скриптов на YouTube.
struct Links {
    links: HashSet<String>,
    video_ids: HashSet<String>,
}

static LINKS: LazyLock<Links> = LazyLock::new(|| {
    let markdown_link = js_regex(r"\]\((https:\/\/[^)\s]+)\)");
    let mut links = HashSet::new();
    let mut video_ids = HashSet::new();
    links.insert(DATA.source.repo.clone());
    links.insert(format!("{}#readme", DATA.source.repo));
    for script in &DATA.scripts {
        if let Some(save) = script.save.as_ref().filter(|save| !save.is_empty()) {
            links.insert(save.clone());
        }
        if let Some(video) = &script.video {
            video_ids.insert(video.id.clone());
        }
        for text in std::iter::once(&script.usage).chain(&script.notes) {
            for found in markdown_link.find_iter(text) {
                if let Some(url) = found.group(1) {
                    links.insert(text[url].to_owned());
                }
            }
        }
    }
    Links { links, video_ids }
});

static WATCH_URL: LazyLock<Regex> = LazyLock::new(|| {
    js_regex(r"^https:\/\/www\.youtube\.com\/watch\?v=([\w-]{11})(?:&t=\d{1,6})?$")
});

/// Ссылка из гайда: страница, сохранение, ссылка из описания или видео скрипта
/// (с отметкой времени или без).
pub fn is_guide_link(url: &str) -> bool {
    if LINKS.links.contains(url) {
        return true;
    }
    WATCH_URL
        .find(url)
        .and_then(|found| found.group(1))
        .is_some_and(|id| LINKS.video_ids.contains(&url[id]))
}

/// FNV-1a 32 бита по `JSON.stringify([notes, usage])`: рендерер считает его без
/// `node:crypto`. Хеш идёт по единицам UTF-16.
pub fn text_hash(script: &Script) -> String {
    let text = stringify(&json!([script.notes, script.usage]), "");
    let mut hash: u32 = 0x811c_9dc5;
    for unit in text.encode_utf16() {
        hash ^= u32::from(unit);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{hash:08x}")
}

/// Описание скрипта на языке `lang` или `None`, если перевода нет или он сделан
/// для другой версии текста. Переводы есть только на русский.
pub fn translation_of(script: &Script, lang: &str) -> Option<Translation> {
    if lang != "ru" {
        return None;
    }
    let item = RU.scripts.get(&script.id)?;
    if item.hash != text_hash(script) || item.notes.len() != script.notes.len() {
        return None;
    }
    Some(Translation {
        notes: item.notes.clone(),
        usage: item.usage.clone(),
    })
}
