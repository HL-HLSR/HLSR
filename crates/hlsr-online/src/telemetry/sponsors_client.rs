//! Окно «Спонсорство» (`telemetry/sponsors-client.js` 0.4.9). Новый сервер отдаёт
//! `/v2/public/sponsors`: три раздела (активные подписчики, донаты, прошлые) и ссылки Tribute и
//! бота. Сервер на PHP знает только `/v1`: на 404 клиент откатывается на него, и окно показывает
//! прежний список (`version: 1`). Другие ошибки `/v2` (сеть, 5xx) на `/v1` не откатываются: это
//! сбой, а не старый сервер. Кеш `/v2` — минута, `/v1` — пять минут.
//!
//! Последний список `/v2` ([`SponsorsClient::last_list`]) — единственный источник адресов для
//! `sponsor-open-link` и `support-open`: окно присылает лишь раздел и номер записи.
//!
//! Одновременные `get` делят один запрос. Ответ больше лимита (512 КиБ) завершается ошибкой, а не
//! зависанием 0.4.7 (спецификация, решение 6).

use super::config;
use super::news_client::{AuthorProfile, parse_author_profile, parse_profile_card};
use super::transport::{RequestOptions, Transport};
use crate::js_value;
use crate::shared::news_contract::valid_post_id;
use crate::shared::sponsor_sections::{SECTIONS, SUPPORT_KEYS};
use hlsr_core::js;
use hlsr_core::json_text::{js_number, json_parse};
use hlsr_delivery::timing::Clock;
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};
use url::Url;

/// `CACHE_MS`: как `max-age` сервера для `/v2`.
pub const CACHE_MS: i64 = 60 * 1000;
/// `LEGACY_CACHE_MS`: `/v1`.
pub const LEGACY_CACHE_MS: i64 = 5 * 60 * 1000;
/// `MAX_ITEMS`: записей в разделе.
pub const MAX_ITEMS: usize = 200;
/// `AMOUNT_MAX`.
pub const AMOUNT_MAX: f64 = 100_000_000.0;
/// Хосты Telegram и Tribute, на которые могут вести ссылки `links` сервера.
const SUPPORT_HOSTS: [&str; 3] = ["t.me", "tribute.tg", "web.tribute.tg"];
/// Бот — только Telegram.
const BOT_HOST: &str = "t.me";
const ENTRY_URL_MAX: usize = 255;
const SUPPORT_URL_MAX: usize = 512;
const MAX_RESPONSE_BYTES: usize = 512 * 1024;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// `text(value, max)` спонсоров: строка не длиннее `max` кодовых точек, обрезанная по краям.
fn text(value: Option<&Value>, max: usize) -> Option<String> {
    let text = value?.as_str()?;
    (text.chars().count() <= max).then(|| js::trim(text).to_owned())
}

/// `raw.<key> === undefined ? "" : text(raw.<key>, max)`.
fn optional_text(raw: Option<&Value>, key: &str, max: usize) -> Option<String> {
    match js_value::field(raw, key) {
        None => Some(String::new()),
        other => text(other, max),
    }
}

/// `/^7656119[0-9]{10}$/`.
fn is_steam_id(id: &str) -> bool {
    id.len() == 17 && id.starts_with("7656119") && id.bytes().all(|b| b.is_ascii_digit())
}

/// `raw.subscriber === true`.
fn subscriber(raw: Option<&Value>) -> bool {
    js_value::field(raw, "subscriber") == Some(&Value::Bool(true))
}

/// Спонсор `/v1`.
#[derive(Debug, Clone, PartialEq)]
pub struct Sponsor {
    pub id: String,
    pub name: String,
    pub note: String,
    /// Целые рубли.
    pub amount: f64,
    pub profile: Option<AuthorProfile>,
    /// `profile.subscriber` 0.4.9: подписка активна (значок у ника).
    pub subscriber: bool,
}

impl Sponsor {
    pub fn to_json(&self) -> Value {
        let profile = self.profile.as_ref().map_or(Value::Null, |profile| {
            let mut profile = profile.to_json();
            profile["subscriber"] = Value::Bool(self.subscriber);
            profile
        });
        json!({
            "id": self.id,
            "name": self.name,
            "note": self.note,
            "amount": js_number(self.amount),
            "profile": profile,
        })
    }
}

/// `parseSponsor(raw)`.
pub fn parse_sponsor(raw: &Value) -> Option<Sponsor> {
    let raw = Some(raw);
    if !js_value::is_object(raw) {
        return None;
    }
    let id = js_value::field(raw, "id")
        .and_then(Value::as_str)
        .filter(|id| valid_post_id(id))?;
    let name = text(js_value::field(raw, "name"), 64).filter(|name| !name.is_empty())?;
    let note = optional_text(raw, "note", 160)?;
    let amount = js_value::finite_number(js_value::field(raw, "amount"))
        .filter(|amount| amount.fract() == 0.0 && *amount >= 0.0 && *amount <= AMOUNT_MAX)?;
    let profile = js_value::field(raw, "profile");
    Some(Sponsor {
        id: id.to_owned(),
        name,
        note,
        amount,
        profile: parse_author_profile(profile),
        subscriber: subscriber(profile),
    })
}

/// `parseProfileCard(raw)` (`news-client.js` 0.4.9): карточка без SteamID — ник, аватар,
/// оформление, `subscriber`; пустой ник допустим.
fn profile_card(raw: Option<&Value>) -> Value {
    parse_profile_card(raw).map_or(Value::Null, |card| card.to_json())
}

/// `httpsUrl(raw, max)`: https-адрес с хостом, без учётных данных и порта, без пробелов и
/// управляющих знаков, не длиннее `max` единиц UTF-16.
fn https_url(raw: Option<&Value>, max: usize) -> Option<Url> {
    let raw = raw?.as_str()?;
    if raw.is_empty()
        || js::text::utf16_len(raw) > max
        || raw.chars().any(|c| (c as u32) <= 32 || c as u32 == 127)
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    let ok = url.scheme() == "https"
        && url.host_str().is_some_and(|host| !host.is_empty())
        && url.username().is_empty()
        && url.password().is_none_or(str::is_empty)
        && url.port().is_none();
    ok.then_some(url)
}

/// `parseLink(raw)`: `{mode: "none"}`, `{mode: "profile", steamId64}` или `{mode: "url", url}`;
/// всё непроверенное — `none`.
fn parse_link(raw: Option<&Value>) -> Value {
    let none = || json!({"mode": "none"});
    match js_value::field(raw, "mode").and_then(Value::as_str) {
        Some("profile") => match js_value::field(raw, "steamId64").and_then(Value::as_str) {
            Some(id) if is_steam_id(id) => json!({"mode": "profile", "steamId64": id}),
            _ => none(),
        },
        Some("url") => https_url(js_value::field(raw, "url"), ENTRY_URL_MAX)
            .map_or_else(none, |url| json!({"mode": "url", "url": url.as_str()})),
        _ => none(),
    }
}

/// `supportUrl(raw)`: https на один из хостов Telegram или Tribute.
fn support_url(raw: Option<&Value>) -> Option<String> {
    https_url(raw, SUPPORT_URL_MAX)
        .filter(|url| SUPPORT_HOSTS.contains(&url.host_str().unwrap_or_default()))
        .map(String::from)
}

/// `botUrl(raw, start)`: адрес бота (`https://t.me` без учётных данных и порта); `start` — с
/// какого сценария бот начинает (`subscribe`, `donate`, код привязки) или `None` — бот как есть.
pub fn bot_url(raw: Option<&Value>, start: Option<&str>) -> Option<String> {
    let mut url = https_url(raw, SUPPORT_URL_MAX).filter(|url| url.host_str() == Some(BOT_HOST))?;
    if let Some(start) = start.filter(|start| !start.is_empty()) {
        // `searchParams.set("start", …)`: первый `start` меняется, остальные убираются, запрос
        // пересобирается как application/x-www-form-urlencoded.
        let mut pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        let mut seen = false;
        pairs.retain_mut(|(key, value)| {
            if key != "start" {
                return true;
            }
            if seen {
                return false;
            }
            seen = true;
            start.clone_into(value);
            true
        });
        if !seen {
            pairs.push(("start".to_owned(), start.to_owned()));
        }
        url.query_pairs_mut().clear().extend_pairs(pairs);
    }
    Some(url.into())
}

/// `parseLinks(raw)`: бот — только с адресом, который откроет `support-open`.
fn parse_links(raw: Option<&Value>) -> Map<String, Value> {
    let mut links = Map::new();
    for key in SUPPORT_KEYS {
        let Some(value) = js_value::field(raw, key) else {
            continue;
        };
        let url = if key == "bot" {
            bot_url(Some(value), None)
        } else {
            support_url(Some(value))
        };
        if let Some(url) = url {
            links.insert(key.to_owned(), Value::String(url));
        }
    }
    links
}

/// `/^[0-9]{4}-[0-9]{2}-[0-9]{2}$/`.
fn is_date(text: &str) -> bool {
    text.len() == 10
        && text.bytes().enumerate().all(|(at, b)| {
            if at == 4 || at == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
}

/// `parseEntry(raw, section)`: запись раздела; сумма — только у донатов. Пустое имя — скрытый ник.
fn parse_entry(raw: &Value, section: &str) -> Option<Value> {
    let raw = Some(raw);
    if !js_value::is_object(raw) {
        return None;
    }
    let name = optional_text(raw, "name", 64)?;
    let note = optional_text(raw, "note", 160)?;
    let amount = if section == "donors" {
        js_value::finite_number(js_value::field(raw, "amountRub"))
            .filter(|amount| amount.fract() == 0.0 && *amount >= 0.0 && *amount <= AMOUNT_MAX)?
    } else {
        0.0
    };
    let since = js_value::field(raw, "since")
        .and_then(Value::as_str)
        .filter(|since| is_date(since))
        .unwrap_or_default();
    Some(json!({
        "profile": profile_card(js_value::field(raw, "card")),
        "name": name,
        "amount": js_number(amount),
        "note": note,
        "since": since,
        "link": parse_link(js_value::field(raw, "link")),
    }))
}

/// `parseList(raw)`: тело `/v2` → `{links, active, donors, past}` или `None`, если нет какого-то
/// из трёх разделов.
pub fn parse_list(raw: &Value) -> Option<Map<String, Value>> {
    let raw = Some(raw);
    let sections = SECTIONS
        .iter()
        .map(|section| js_value::field(raw, section).and_then(Value::as_array))
        .collect::<Option<Vec<_>>>()?;
    let mut list = Map::new();
    list.insert(
        "links".to_owned(),
        Value::Object(parse_links(js_value::field(raw, "links"))),
    );
    for (section, entries) in SECTIONS.iter().zip(sections) {
        let entries = entries
            .iter()
            .take(MAX_ITEMS)
            .filter_map(|entry| parse_entry(entry, section))
            .collect();
        list.insert((*section).to_owned(), Value::Array(entries));
    }
    Some(list)
}

/// `emptyList()`.
fn empty_list() -> Map<String, Value> {
    parse_list(&json!({"active": [], "donors": [], "past": []})).unwrap_or_default()
}

/// Параметры [`SponsorsClient::new`].
pub struct SponsorsClientOptions {
    /// По умолчанию `https://api.hlsr.pro/v2/public/sponsors`.
    pub endpoint: Option<String>,
    /// По умолчанию `https://api.hlsr.pro/v1/public/sponsors`.
    pub legacy_endpoint: Option<String>,
    /// По умолчанию `0.0.0`.
    pub launcher_version: Option<String>,
    pub clock: Clock,
    pub transport: Arc<dyn Transport>,
}

/// Последний удачный ответ: либо `/v2` (`list`), либо `/v1` (`legacy`), не оба.
#[derive(Default)]
struct State {
    list: Option<Arc<Map<String, Value>>>,
    list_at: i64,
    legacy: Option<Arc<Vec<Sponsor>>>,
    legacy_at: i64,
    completed: u64,
    last: Option<Value>,
}

impl State {
    /// `answer(version, status)`.
    fn answer(&self, version: u8, ok: bool) -> Value {
        let status = if ok { "ok" } else { "error" };
        if version == 1 {
            let sponsors: Vec<Value> = self
                .legacy
                .iter()
                .flat_map(|list| list.iter().map(Sponsor::to_json))
                .collect();
            return json!({"status": status, "version": 1, "sponsors": sponsors});
        }
        let mut answer = Map::new();
        answer.insert("status".to_owned(), json!(status));
        answer.insert("version".to_owned(), json!(2));
        answer.extend(self.list.as_deref().cloned().unwrap_or_else(empty_list));
        Value::Object(answer)
    }
}

pub struct SponsorsClient {
    endpoint: String,
    legacy_endpoint: String,
    launcher_version: String,
    clock: Clock,
    transport: Arc<dyn Transport>,
    state: Mutex<State>,
    fetching: tokio::sync::Mutex<()>,
}

impl SponsorsClient {
    pub fn new(options: SponsorsClientOptions) -> SponsorsClient {
        let or_default = |endpoint: Option<String>, path: &str| {
            endpoint
                .filter(|endpoint| !endpoint.is_empty())
                .unwrap_or_else(|| format!("{}{path}", config::API_BASE))
        };
        SponsorsClient {
            endpoint: or_default(options.endpoint, "/v2/public/sponsors"),
            legacy_endpoint: or_default(options.legacy_endpoint, "/v1/public/sponsors"),
            launcher_version: options
                .launcher_version
                .filter(|version| !version.is_empty())
                .unwrap_or_else(|| "0.0.0".to_owned()),
            clock: options.clock,
            transport: options.transport,
            state: Mutex::default(),
            fetching: tokio::sync::Mutex::new(()),
        }
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    /// `get({force})`: `{status: "ok", version: 2, links, active, donors, past}` или
    /// `{status: "ok", version: 1, sponsors}`; при ошибке `status: "error"` и последний удачный
    /// ответ той же версии (или пустой).
    pub async fn get(&self, force: bool) -> Value {
        let observed = {
            let state = lock(&self.state);
            if !force {
                let now = self.now();
                if state.list.is_some() && now - state.list_at < CACHE_MS {
                    return state.answer(2, true);
                }
                if state.legacy.is_some() && now - state.legacy_at < LEGACY_CACHE_MS {
                    return state.answer(1, true);
                }
            }
            state.completed
        };
        let _fetching = self.fetching.lock().await;
        {
            let state = lock(&self.state);
            if state.completed != observed
                && let Some(answer) = &state.last
            {
                return answer.clone();
            }
        }
        let answer = self.fetch().await;
        let mut state = lock(&self.state);
        state.completed += 1;
        state.last = Some(answer.clone());
        answer
    }

    /// `lastList()`: последний список `/v2`; `None`, если `/v2` не отвечал или сервер старый.
    pub fn last_list(&self) -> Option<Arc<Map<String, Value>>> {
        lock(&self.state).list.clone()
    }

    /// `getJson(endpoint)`: `Err(Some(код))` — ответ не 2xx, `Err(None)` — сеть или не JSON.
    async fn get_json(&self, endpoint: &str) -> Result<Value, Option<u16>> {
        let options = RequestOptions {
            method: Some("GET".to_owned()),
            timeout_ms: Some(config::online::TIMEOUT_MS),
            max_response_bytes: Some(MAX_RESPONSE_BYTES),
            launcher_version: Some(self.launcher_version.clone()),
            ..RequestOptions::default()
        };
        let response = self
            .transport
            .request(endpoint, &options)
            .await
            .map_err(|_| None)?;
        if !(200..300).contains(&response.status) {
            return Err(Some(response.status));
        }
        json_parse(&response.body).map_err(|_| None)
    }

    async fn fetch(&self) -> Value {
        let list = match self.get_json(&self.endpoint).await {
            Ok(body) => parse_list(&body),
            Err(Some(404)) => return self.fetch_legacy().await,
            Err(_) => None,
        };
        let mut state = lock(&self.state);
        let Some(list) = list else {
            return state.answer(2, false);
        };
        state.list = Some(Arc::new(list));
        state.list_at = self.now();
        state.legacy = None;
        state.answer(2, true)
    }

    async fn fetch_legacy(&self) -> Value {
        let data = self.get_json(&self.legacy_endpoint).await.ok();
        let mut state = lock(&self.state);
        let Some(list) = data
            .as_ref()
            .and_then(|data| data.get("sponsors"))
            .and_then(Value::as_array)
        else {
            return state.answer(1, false);
        };
        let mut seen: HashSet<String> = HashSet::new();
        let mut sponsors: Vec<Sponsor> = list
            .iter()
            .take(MAX_ITEMS)
            .filter_map(parse_sponsor)
            .filter(|sponsor| seen.insert(sponsor.id.clone()))
            .collect();
        // Устойчивая сортировка по сумме по убыванию (`sort` V8 устойчив).
        sponsors.sort_by(|a, b| {
            b.amount
                .partial_cmp(&a.amount)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        state.legacy = Some(Arc::new(sponsors));
        state.legacy_at = self.now();
        // Сервер откатился на старый: прежний список `/v2` больше не показывается, и его адреса
        // открывать нельзя.
        state.list = None;
        state.answer(1, true)
    }
}
