//! Лента «Главной» (`telemetry/news-client.js`): посты из админки сайта, мировые рекорды и свежие
//! подтверждённые забеги speedrun.com. Запрашивается по требованию (открыли вкладку, нажали «Обновить»), а не по таймеру:
//! новости меняются редко, сервер и так кеширует ответ. Ответ разбирается строго; при сбое остаётся
//! последняя удачная лента. Кеш — 2 минуты; свежие реакции (ответ на «Нравится») держатся поверх
//! кеша CDN те же 2 минуты.
//!
//! Одновременные `get` делят один запрос (`pending` 0.4.7). Ответ больше лимита (2 МиБ) здесь
//! завершается ошибкой ленты: в 0.4.7 транспорт не завершал запрос, и `pending` оставался
//! навсегда (спецификация, решение 6).

use super::config;
use super::transport::{RequestOptions, Transport};
use crate::js_value;
use crate::shared::news_contract::{self, Reactions};
use crate::shared::news_image_layout::{self, Crop, TileImage};
use crate::shared::{avatar_frames, nick_styles};
use hlsr_core::js;
use hlsr_core::js::iso::iso_from_ms;
use hlsr_core::json_text::{js_number, json_parse};
use hlsr_delivery::timing::Clock;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use url::Url;

/// `CACHE_MS`: 2 минуты.
pub const CACHE_MS: i64 = 2 * 60 * 1000;
/// `MAX_ITEMS`.
pub const MAX_ITEMS: usize = 40;
/// `MAX_RECORDS`: рекорды всех таблиц speedrun.com, по одному на сочетание сабкатегорий.
pub const MAX_RECORDS: usize = 400;
/// Посты длиннее счётчиков онлайна: до 40 записей по 8000 символов.
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

/// Страница забега — `/<игра>/runs/<id>`. Старый сервер присылал `/runs/<id>` без игры: такой
/// страницы нет, её адрес собирается из `gameKey` сразу, без запросов.
static RUN_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https://www\.speedrun\.com/(?:([A-Za-z0-9_-]{1,64})/)?runs?/([A-Za-z0-9]{8})$")
        .expect("шаблон RUN_URL")
});

/// Аватар Steam автора — только с CDN Steam, тот же список, что у сервера (`SteamDirectory.php`).
const AVATAR_HOSTS: [&str; 3] = [
    "avatars.steamstatic.com",
    "avatars.akamai.steamstatic.com",
    "steamcdn-a.akamaihd.net",
];

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// `text(value, max)`: непустая строка не длиннее `max` кодовых точек (как есть, без обрезки).
fn text(value: Option<&Value>, max: usize) -> Option<String> {
    let text = value?.as_str()?;
    (!js::trim(text).is_empty() && text.chars().count() <= max).then(|| text.to_owned())
}

/// `isoDate(value)`: строка, которую разбирает `Date.parse`, в виде `toISOString()`.
fn iso_date(value: Option<&Value>) -> Option<String> {
    js::date_parse(value?.as_str()?).map(iso_from_ms)
}

/// `Number.isInteger(value)`.
fn integer(value: Option<&Value>) -> Option<f64> {
    js_value::finite_number(value).filter(|n| n.fract() == 0.0)
}

fn avatar_url(value: Option<&Value>) -> String {
    let Some(text) = value.and_then(Value::as_str) else {
        return String::new();
    };
    if text.is_empty() || js::text::utf16_len(text) > 256 {
        return String::new();
    }
    let Ok(url) = Url::parse(text) else {
        return String::new();
    };
    let allowed = url.scheme() == "https"
        && AVATAR_HOSTS.contains(&url.host_str().unwrap_or(""))
        && url.username().is_empty()
        && url.password().is_none_or(str::is_empty);
    if allowed {
        url.as_str().to_owned()
    } else {
        String::new()
    }
}

/// Карточка без SteamID (`parseProfileCard`, окно «Спонсорство» v2): ник, аватар и оформление.
/// Проверки те же, что у профиля автора; пустой ник (скрытый) допустим.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileCard {
    pub persona_name: String,
    /// Только с CDN Steam, иначе пусто.
    pub avatar_url: String,
    /// `{style, css}` после проверки стиля.
    pub nick: Option<Value>,
    pub avatar_frame: Option<Value>,
    /// Подписка активна сейчас (значок у ника): включает её только `true` сервера.
    pub subscriber: bool,
}

impl ProfileCard {
    fn insert_into(&self, map: &mut Map<String, Value>) {
        map.insert("personaName".to_owned(), json!(self.persona_name));
        map.insert("avatarUrl".to_owned(), json!(self.avatar_url));
        map.insert("nick".to_owned(), self.nick.clone().unwrap_or(Value::Null));
        map.insert(
            "avatarFrame".to_owned(),
            self.avatar_frame.clone().unwrap_or(Value::Null),
        );
        map.insert("subscriber".to_owned(), json!(self.subscriber));
    }

    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        self.insert_into(&mut map);
        Value::Object(map)
    }
}

/// `parseProfileCard(raw)`: не объект — `None`.
pub fn parse_profile_card(raw: Option<&Value>) -> Option<ProfileCard> {
    if !js_value::is_object(raw) {
        return None;
    }
    Some(ProfileCard {
        persona_name: text(js_value::field(raw, "personaName"), 64).unwrap_or_default(),
        avatar_url: avatar_url(js_value::field(raw, "avatarUrl")),
        nick: nick_styles::normalize(js_value::field(raw, "nick").unwrap_or(&Value::Null)),
        avatar_frame: avatar_frames::normalize(
            js_value::field(raw, "avatarFrame").unwrap_or(&Value::Null),
        ),
        subscriber: js_value::field(raw, "subscriber") == Some(&Value::Bool(true)),
    })
}

/// Профиль HLSR автора поста (или спонсора): по нему лента рисует аватар и оформленный ник и
/// открывает профиль.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthorProfile {
    pub steam_id64: String,
    pub card: ProfileCard,
}

impl AuthorProfile {
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("steamId64".to_owned(), json!(self.steam_id64));
        self.card.insert_into(&mut map);
        Value::Object(map)
    }
}

/// `parseAuthorProfile(raw)`: нет профиля или он не прошёл проверку — `None`, и пост остаётся
/// с именем автора.
pub fn parse_author_profile(raw: Option<&Value>) -> Option<AuthorProfile> {
    if !js_value::is_object(raw) {
        return None;
    }
    let id = js_value::field(raw, "steamId64")?.as_str()?;
    let valid =
        id.len() == 17 && id.starts_with("7656119") && id.bytes().all(|b| b.is_ascii_digit());
    if !valid {
        return None;
    }
    Some(AuthorProfile {
        steam_id64: id.to_owned(),
        card: parse_profile_card(raw)?,
    })
}

/// Скриншот поста: адрес на сервере API, размеры и область показа.
#[derive(Debug, Clone, PartialEq)]
pub struct NewsImage {
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub crop: Option<Crop>,
}

impl NewsImage {
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("url".to_owned(), json!(self.url));
        map.insert("width".to_owned(), json!(self.width));
        map.insert("height".to_owned(), json!(self.height));
        if let Some(crop) = &self.crop {
            map.insert("crop".to_owned(), crop.to_json());
        }
        Value::Object(map)
    }

    /// Картинка для раскладки плиток.
    pub fn tile(&self) -> TileImage {
        TileImage {
            width: f64::from(self.width),
            height: f64::from(self.height),
            crop: self.crop,
        }
    }
}

/// `/^\/v1\/public\/news\/image\/[a-f0-9]{64}\.(png|jpg|webp)$/`.
fn is_news_image_path(path: &str) -> bool {
    let Some(name) = path.strip_prefix("/v1/public/news/image/") else {
        return false;
    };
    let Some((hash, extension)) = name.split_once('.') else {
        return false;
    };
    hash.len() == 64
        && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && matches!(extension, "png" | "jpg" | "webp")
}

fn parse_image(raw: &Value, api_base: &str) -> Option<NewsImage> {
    let raw = Some(raw);
    if !js_value::is_object(raw) {
        return None;
    }
    let target = js_value::field(raw, "url")?.as_str()?;
    if js::text::utf16_len(target) > 512 {
        return None;
    }
    let width = integer(js_value::field(raw, "width"))?;
    let height = integer(js_value::field(raw, "height"))?;
    if width < 1.0
        || height < 1.0
        || width > 8000.0
        || height > 8000.0
        || width * height > 32_000_000.0
    {
        return None;
    }
    let base = Url::parse(api_base).ok()?;
    let url = base.join(target).ok()?;
    let local = base.scheme() == "http" && base.host_str() == Some("127.0.0.1");
    let same_origin = url.origin().ascii_serialization() == base.origin().ascii_serialization();
    if !same_origin
        || (!local && url.scheme() != "https")
        || !url.username().is_empty()
        || url.password().is_some_and(|password| !password.is_empty())
        || url.query().is_some_and(|query| !query.is_empty())
        || url.fragment().is_some_and(|fragment| !fragment.is_empty())
        || !is_news_image_path(url.path())
    {
        return None;
    }
    Some(NewsImage {
        url: url.as_str().to_owned(),
        width: width as u32,
        height: height as u32,
        crop: news_image_layout::parse_crop(js_value::field(raw, "crop")),
    })
}

/// Пост из админки сайта.
#[derive(Debug, Clone, PartialEq)]
pub struct Post {
    pub id: String,
    pub title: String,
    pub body: String,
    pub author: String,
    pub author_profile: Option<AuthorProfile>,
    /// Не больше шести.
    pub images: Vec<NewsImage>,
    pub reactions: Reactions,
    /// `toISOString()`.
    pub date: String,
}

/// Мировой рекорд или свежий подтверждённый забег (`kind: "run"`) speedrun.com.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// `kind: "run"`: свежий забег, не рекорд.
    pub run: bool,
    /// Восемь знаков из адреса забега.
    pub id: String,
    pub game: String,
    pub game_key: String,
    pub category: String,
    pub subcategory: String,
    pub runners: Vec<String>,
    pub seconds: f64,
    pub date: String,
    pub url: String,
    /// Место в таблице (1–3) у свежего забега; у рекорда оно и так первое.
    pub place: Option<u8>,
    /// SteamID64 профиля HLSR бегуна (speedrun.com привязан) или `None`, в порядке `runners`;
    /// ни одного профиля — `None`.
    pub profiles: Option<Vec<Option<String>>>,
}

impl Record {
    fn kind(&self) -> &'static str {
        if self.run { "run" } else { "record" }
    }
}

/// Запись ленты.
#[derive(Debug, Clone, PartialEq)]
pub enum NewsItem {
    Post(Post),
    Record(Record),
}

impl NewsItem {
    pub fn id(&self) -> &str {
        match self {
            NewsItem::Post(post) => &post.id,
            NewsItem::Record(record) => &record.id,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            NewsItem::Post(_) => "post",
            NewsItem::Record(record) => record.kind(),
        }
    }

    /// Объект записи, как его отдаёт клиент 0.4.9 (порядок ключей тот же).
    pub fn to_json(&self) -> Value {
        match self {
            NewsItem::Post(post) => json!({
                "kind": "post",
                "id": post.id,
                "title": post.title,
                "body": post.body,
                "author": post.author,
                "authorProfile": post.author_profile.as_ref().map_or(Value::Null, AuthorProfile::to_json),
                "images": post.images.iter().map(NewsImage::to_json).collect::<Vec<_>>(),
                "reactions": post.reactions.to_json(),
                "date": post.date,
            }),
            NewsItem::Record(record) => json!({
                "kind": record.kind(),
                "id": record.id,
                "game": record.game,
                "gameKey": record.game_key,
                "category": record.category,
                "subcategory": record.subcategory,
                "runners": record.runners,
                "seconds": js_number(record.seconds),
                "date": record.date,
                "url": record.url,
                "place": record.place,
                "profiles": record.profiles,
            }),
        }
    }
}

/// `parseItem(raw, apiBase)`: запись ленты или `None`, если она не прошла проверку. `api_base` —
/// адрес ленты: картинки поста принимаются только с его происхождения.
pub fn parse_item(raw: &Value, api_base: &str) -> Option<NewsItem> {
    let raw_opt = Some(raw);
    if !js_value::is_object(raw_opt) {
        return None;
    }
    let date = iso_date(js_value::field(raw_opt, "date"))?;
    let kind = js_value::field(raw_opt, "kind").and_then(Value::as_str);

    if kind == Some("post") {
        let id = js_value::field(raw_opt, "id")
            .and_then(Value::as_str)
            .filter(|id| news_contract::valid_post_id(id))
            .map(str::to_owned);
        let title = text(js_value::field(raw_opt, "title"), 160);
        let body = text(js_value::field(raw_opt, "body"), 8000);
        let author = match js_value::field(raw_opt, "author") {
            None => Some(String::new()),
            Some(Value::String(author)) if author.is_empty() => Some(String::new()),
            other => text(other, 80),
        };
        let images = js_value::field(raw_opt, "images");
        if images.is_some_and(|images| !images.is_array()) {
            return None;
        }
        let reactions = js_value::field(raw_opt, "reactions");
        if reactions.is_some_and(|reactions| !reactions.is_object()) {
            return None;
        }
        let author = author?;
        let images: Vec<NewsImage> = images
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(6)
            .filter_map(|image| parse_image(image, api_base))
            .collect();
        let author_profile = parse_author_profile(js_value::field(raw_opt, "authorProfile"));
        return Some(NewsItem::Post(Post {
            id: id?,
            title: title?,
            body: body?,
            author,
            author_profile,
            images,
            reactions: news_contract::counts(reactions),
            date,
        }));
    }

    let run = kind == Some("run");
    if kind == Some("record") || run {
        // Пары «имя — профиль» собираются до отбора имён: `profiles` идёт в порядке `runners`.
        let raw_runners: &[Value] = js_value::field(raw_opt, "runners")
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice);
        let raw_profiles = js_value::field(raw_opt, "profiles")
            .and_then(Value::as_array)
            .filter(|profiles| profiles.len() == raw_runners.len());
        let (runners, profiles): (Vec<String>, Vec<Option<String>>) = raw_runners
            .iter()
            .enumerate()
            .filter_map(|(i, name)| {
                let id = raw_profiles
                    .and_then(|profiles| profiles[i].as_str())
                    .filter(|id| id.len() == 17 && id.bytes().all(|b| b.is_ascii_digit()));
                Some((text(Some(name), 64)?, id.map(str::to_owned)))
            })
            .take(8)
            .unzip();
        let seconds = js_value::finite_number(js_value::field(raw_opt, "seconds"))
            .filter(|seconds| *seconds > 0.0 && *seconds <= 604_800.0);
        let game = text(js_value::field(raw_opt, "game"), 96);
        let url_text = js_value::field(raw_opt, "url").and_then(Value::as_str);
        let link = url_text.and_then(|url| RUN_URL.captures(url));
        let (Some(seconds), Some(game), Some(link), Some(url_text)) =
            (seconds, game, link, url_text)
        else {
            return None;
        };
        if runners.is_empty() {
            return None;
        }
        let game_key = js_value::field(raw_opt, "gameKey")
            .and_then(Value::as_str)
            .filter(|key| {
                (1..=16).contains(&key.len())
                    && key.bytes().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9'))
            })
            .unwrap_or("")
            .to_owned();
        let id = link[2].to_owned();
        let url = if link.get(1).is_some() || game_key.is_empty() {
            url_text.to_owned()
        } else {
            format!("https://www.speedrun.com/{game_key}/runs/{id}")
        };
        let place = integer(js_value::field(raw_opt, "place"))
            .filter(|place| run && (1.0..=3.0).contains(place))
            .map(|place| place as u8);
        return Some(NewsItem::Record(Record {
            run,
            id,
            game,
            game_key,
            category: text(js_value::field(raw_opt, "category"), 96).unwrap_or_default(),
            subcategory: text(js_value::field(raw_opt, "subcategory"), 160).unwrap_or_default(),
            runners,
            seconds,
            date,
            url,
            place,
            profiles: profiles.iter().any(Option::is_some).then_some(profiles),
        }));
    }

    None
}

/// Итог `get`: `ok` или `error` с последней удачной лентой.
#[derive(Debug, Clone, PartialEq)]
pub struct NewsAnswer {
    pub ok: bool,
    pub items: Arc<Vec<NewsItem>>,
}

impl NewsAnswer {
    /// `{status: "ok" | "error", items}`.
    pub fn to_json(&self) -> Value {
        json!({
            "status": if self.ok { "ok" } else { "error" },
            "items": self.items.iter().map(NewsItem::to_json).collect::<Vec<_>>(),
        })
    }
}

/// Параметры [`NewsClient::new`] (`options` конструктора 0.4.7).
pub struct NewsClientOptions {
    /// По умолчанию `https://api.hlsr.pro/v1/public/news`.
    pub endpoint: Option<String>,
    /// По умолчанию `0.0.0`.
    pub launcher_version: Option<String>,
    pub clock: Clock,
    pub transport: Arc<dyn Transport>,
}

#[derive(Default)]
struct State {
    cache: Option<Arc<Vec<NewsItem>>>,
    cached_at: i64,
    reaction_updates: HashMap<String, (i64, Reactions)>,
    /// Сколько запросов завершилось: ожидающий `get` видит, что за время ожидания ответ уже есть.
    completed: u64,
    last: Option<NewsAnswer>,
}

pub struct NewsClient {
    endpoint: String,
    launcher_version: String,
    clock: Clock,
    transport: Arc<dyn Transport>,
    state: Mutex<State>,
    /// `pending`: один запрос за раз; остальные ждут его итога.
    fetching: tokio::sync::Mutex<()>,
}

impl NewsClient {
    pub fn new(options: NewsClientOptions) -> NewsClient {
        NewsClient {
            endpoint: options
                .endpoint
                .filter(|endpoint| !endpoint.is_empty())
                .unwrap_or_else(|| format!("{}/v1/public/news", config::API_BASE)),
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

    /// `updateReactions(id, raw)`: свежие счётчики поста поверх кеша на 2 минуты.
    pub fn update_reactions(&self, id: &str, raw: &Value) {
        let reactions = news_contract::counts(Some(raw));
        let at = self.now();
        let mut state = lock(&self.state);
        state
            .reaction_updates
            .insert(id.to_owned(), (at, reactions));
        if let Some(cache) = &state.cache {
            let updated: Vec<NewsItem> = cache
                .iter()
                .map(|item| match item {
                    NewsItem::Post(post) if post.id == id => NewsItem::Post(Post {
                        reactions,
                        ..post.clone()
                    }),
                    other => other.clone(),
                })
                .collect();
            state.cache = Some(Arc::new(updated));
        }
    }

    /// `get({force})`: лента из кеша (пока ему меньше 2 минут и нет `force`) или с сервера.
    pub async fn get(&self, force: bool) -> NewsAnswer {
        let observed = {
            let state = lock(&self.state);
            if !force
                && let Some(cache) = &state.cache
                && self.now() - state.cached_at < CACHE_MS
            {
                return NewsAnswer {
                    ok: true,
                    items: cache.clone(),
                };
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

    async fn fetch(&self) -> NewsAnswer {
        match self.load().await {
            Some(items) => NewsAnswer { ok: true, items },
            None => NewsAnswer {
                ok: false,
                items: lock(&self.state).cache.clone().unwrap_or_default(),
            },
        }
    }

    /// Запрос и разбор; `None` — любой сбой (сеть, код, JSON, форма).
    async fn load(&self) -> Option<Arc<Vec<NewsItem>>> {
        let options = RequestOptions {
            method: Some("GET".to_owned()),
            timeout_ms: Some(config::online::TIMEOUT_MS),
            max_response_bytes: Some(MAX_RESPONSE_BYTES),
            launcher_version: Some(self.launcher_version.clone()),
            ..RequestOptions::default()
        };
        let response = self
            .transport
            .request(&self.endpoint, &options)
            .await
            .ok()?;
        if !(200..300).contains(&response.status) {
            return None;
        }
        let data = json_parse(&response.body).ok()?;
        let all = data.get("items")?.as_array()?;
        // Полный список рекордов сервер отдаёт отдельно (`records`); в `items` их доля для старых
        // лаунчеров.
        let records = data.get("records").and_then(Value::as_array);
        let mut raw: Vec<&Value> = Vec::new();
        for item in all.iter().take(MAX_ITEMS) {
            let is_record = item.get("kind").and_then(Value::as_str) == Some("record");
            if records.is_none() || !is_record {
                raw.push(item);
            }
        }
        raw.extend(records.into_iter().flatten().take(MAX_RECORDS));

        let mut seen: HashSet<(&'static str, String)> = HashSet::new();
        let mut items: Vec<NewsItem> = raw
            .into_iter()
            .filter_map(|item| parse_item(item, &self.endpoint))
            .filter(|item| seen.insert((item.kind(), item.id().to_owned())))
            .collect();

        let mut state = lock(&self.state);
        let now = self.now();
        for item in &mut items {
            if let NewsItem::Post(post) = item
                && let Some((at, reactions)) = state.reaction_updates.get(&post.id)
                && now - at < CACHE_MS
            {
                post.reactions = *reactions;
            }
        }
        state
            .reaction_updates
            .retain(|_, (at, _)| now - *at < CACHE_MS);
        let items = Arc::new(items);
        state.cache = Some(items.clone());
        state.cached_at = self.now();
        Some(items)
    }
}
