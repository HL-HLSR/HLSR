//! «Главная» (`HomePanel.vue`, `NewsPost.vue`, `NewsImageViewer.vue`, `run-time.js` 0.4.9):
//! лента, рекорды и свежие забеги (во «Все» не больше [`RUNS_PER_BOARD`] на таблицу), места
//! 1–3, фильтр по игре, реакции и просмотр картинок. Машина без Slint: побочные действия
//! возвращает списком ([`Effect`]); ответы сервисов возвращаются в неё методами по тегам.
//! Игроков ведёт [`super::home_players`].

use super::bridge::Tr;
use super::jsv::{field, num, prop, str_of, truthy};
use super::wall_clock::WallClock;
use crate::intl::DateStyle;
use crate::services::Reply;
use hlsr_core::language::Language;
use hlsr_online::shared::news_contract::REACTIONS;
use hlsr_online::shared::news_image_layout::{
    TileImage, parse_crop, tile_aspect, tile_image_style,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// Порядок игр в фильтре рекордов.
const GAME_ORDER: [&str; 5] = ["hl1", "hl1ce", "hl2", "hl2ce", "hl2hl1"];

/// Короткие подписи фильтра (полное название — в подсказке).
fn game_short(key: &str) -> Option<&'static str> {
    Some(match key {
        "hl1" => "HL",
        "hl1ce" => "HL CE",
        "hl2" => "HL2",
        "hl2ce" => "HL2 CE",
        "hl2hl1" => "HL2 + HL1",
        _ => return None,
    })
}

fn game_rank(key: &str) -> usize {
    GAME_ORDER
        .iter()
        .position(|known| *known == key)
        .unwrap_or(GAME_ORDER.len())
}

/// Вкладки «Главной».
pub const TABS: [&str; 3] = ["feed", "records", "players"];

/// Во «Все» — не больше стольких свежих забегов одной таблицы (рекорды — все).
pub const RUNS_PER_BOARD: usize = 3;

/// Длительность оборота обновления, мс (`home-spin 0.8s`).
pub const SPIN_TURN_MS: f64 = 800.0;

/// Сколько ждать конца оборота стрелки, когда загрузка кончилась: иконка останавливается
/// только на целом обороте (`animationiteration`).
pub fn turn_remaining(elapsed_ms: f64) -> f64 {
    // Оборот ещё не начался (ответ в тот же кадр): первое `animationiteration` — через оборот.
    if elapsed_ms <= 0.0 {
        return SPIN_TURN_MS;
    }
    let rest = elapsed_ms.rem_euclid(SPIN_TURN_MS);
    if rest == 0.0 {
        0.0
    } else {
        SPIN_TURN_MS - rest
    }
}

/// `formatRun(seconds)`: `ч:мм:сс.ммм`, без часов — `м:сс.ммм`.
pub fn format_run(seconds: f64) -> String {
    let ms = super::jsv::round(seconds * 1000.0);
    let hours = (ms / 3_600_000.0).floor();
    let minutes = (ms / 60_000.0).floor() % 60.0;
    let rest = format!(
        "{:02}.{:03}",
        ((ms / 1000.0).floor() % 60.0) as i64,
        (ms % 1000.0) as i64
    );
    if hours > 0.0 {
        format!("{}:{:02}:{rest}", hours as i64, minutes as i64)
    } else {
        format!("{}:{rest}", minutes as i64)
    }
}

/// `direction(ids, from, to)`: «вперёд», если новая вкладка правее прежней или та же.
pub fn direction_next(ids: &[String], from: &str, to: &str) -> bool {
    let index = |id: &str| {
        ids.iter()
            .position(|known| known == id)
            .map_or(-1, |i| i as i64)
    };
    index(to) >= index(from)
}

/// Побочное действие.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Invoke {
        tag: Tag,
        channel: &'static str,
        args: Vec<Value>,
    },
    /// `$emit("open-profile", id)`: пустой идентификатор — вход или свой профиль.
    OpenProfile(String),
    /// `$emit("overlay", bool)`: открыт просмотр картинок.
    Overlay(bool),
}

/// Чему ответ.
#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    Profile,
    News,
    /// Реакции игрока: ревизия входа и номер запроса.
    Mine(u64, u64),
    /// Ответ на реакцию: пост, ключ, ревизия входа.
    React(String, String, u64),
    /// Ссылка поста.
    Link(String),
    Ignored,
}

#[derive(Debug, Clone)]
struct Pending {
    counts: Value,
    mine: Vec<String>,
}

/// Просмотр картинок поста.
#[derive(Debug, Clone, PartialEq)]
pub struct Viewer {
    pub images: Vec<String>,
    pub title: String,
    pub index: usize,
}

/// Плитка скриншота: раскладка по `news-image-layout`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tile {
    pub index: i32,
    pub url: String,
    pub aspect: f32,
    pub wide: bool,
    /// Положение картинки в плитке — доли плитки (`width`, `height`, `left`, `top`).
    pub width: f32,
    pub height: f32,
    pub left: f32,
    pub top: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReactionView {
    pub key: String,
    /// Смайлик вида (`REACTIONS`).
    pub emoji: String,
    pub count: i32,
    pub selected: bool,
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PostView {
    pub id: String,
    /// SteamID автора с профилем HLSR (пусто — имя без ссылки).
    pub author_id: String,
    pub name: String,
    pub letter: String,
    pub avatar: String,
    pub nick_style: String,
    pub nick_css: String,
    pub frame_css: String,
    /// Значок подписчика у ника автора (`profile.subscriber === true`, 0.4.9).
    pub subscriber: bool,
    pub date: String,
    pub stamp: String,
    pub title: String,
    pub body: String,
    pub tiles: Vec<Tile>,
    pub single_image: bool,
    pub reactions: Vec<ReactionView>,
    pub pending: bool,
    pub notice: String,
    /// «Войти» рядом с замечанием.
    pub notice_sign_in: bool,
}

/// Бегун забега: `profile` — SteamID64 профиля HLSR (speedrun.com привязан), иначе пусто.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Runner {
    pub name: String,
    pub profile: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordView {
    pub id: String,
    pub url: String,
    /// «Игра · Категория» или «Категория».
    pub game: String,
    pub date: String,
    pub iso: String,
    /// Место в таблице: рекорд — 1, свежий забег — 1–3 с сервера, иначе 0.
    pub place: u8,
    /// «Мировой рекорд» / «N место»; без места — пусто.
    pub place_label: String,
    pub subs: Vec<String>,
    pub time: String,
    /// Бегуны через запятую.
    pub runner: String,
    pub runners: Vec<Runner>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Chip {
    pub key: String,
    pub label: String,
    pub title: String,
    pub active: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HomeView {
    pub loading: bool,
    pub spinning: bool,
    /// Устаревшая или не загруженная лента (`⚠ …`).
    pub note: String,
    pub posts: Vec<PostView>,
    pub records: Vec<RecordView>,
    pub chips: Vec<Chip>,
    pub empty_posts: String,
    pub empty_records: String,
    pub tab: i32,
    pub slide_next: bool,
}

#[derive(Debug)]
pub struct HomeState {
    wall: WallClock,
    items: Vec<Value>,
    loading: bool,
    loaded: bool,
    failed: bool,
    spinning: bool,
    record_filter: String,
    filter_next: bool,
    active_tab: String,
    slide_next: bool,
    mine: HashMap<String, Vec<String>>,
    pending: HashMap<String, Pending>,
    notices: HashMap<String, String>,
    viewer: Option<Viewer>,
    profile: Value,
    auth_revision: u64,
    mine_request: u64,
    gone: bool,
}

fn utf16_index(text: &str) -> usize {
    text.encode_utf16().count()
}

impl HomeState {
    pub fn new(wall: WallClock) -> HomeState {
        HomeState {
            wall,
            items: Vec::new(),
            loading: false,
            loaded: false,
            failed: false,
            spinning: false,
            record_filter: "all".to_owned(),
            filter_next: true,
            active_tab: "feed".to_owned(),
            slide_next: true,
            mine: HashMap::new(),
            pending: HashMap::new(),
            notices: HashMap::new(),
            viewer: None,
            profile: json!({"status": "signed-out"}),
            auth_revision: 0,
            mine_request: 0,
            gone: false,
        }
    }

    // ---- Жизненный цикл -------------------------------------------------------------------

    /// `mounted`: состояние входа, затем лента.
    pub fn mounted(&mut self) -> Vec<Effect> {
        let mut effects = vec![Effect::Invoke {
            tag: Tag::Profile,
            channel: "runner-state",
            args: vec![],
        }];
        effects.extend(self.load(false));
        effects
    }

    /// `beforeUnmount`.
    pub fn unmounted(&mut self) -> Vec<Effect> {
        self.gone = true;
        self.mine_request += 1;
        self.viewer = None;
        vec![Effect::Overlay(false)]
    }

    pub fn load(&mut self, force: bool) -> Vec<Effect> {
        self.loading = true;
        self.spinning = true;
        vec![Effect::Invoke {
            tag: Tag::News,
            channel: "public-news-get",
            args: vec![json!({"force": force})],
        }]
    }

    /// Ответ `public-news-get`.
    pub fn news_reply(&mut self, reply: Reply) -> Vec<Effect> {
        let mut effects = vec![];
        match reply {
            Ok(answer) => {
                self.items = prop(&answer, "items")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                self.failed = str_of(prop(&answer, "status")) != Some("ok");
                if self.record_filter != "all"
                    && !self
                        .record_games()
                        .iter()
                        .any(|(key, _)| *key == self.record_filter)
                {
                    self.record_filter = "all".to_owned();
                }
                effects.extend(self.load_mine());
            }
            Err(_) => self.failed = true,
        }
        self.loading = false;
        self.loaded = true;
        effects
    }

    /// Конец загрузки: стрелка крутится до целого оборота (адаптер) или стоит сразу, когда
    /// анимаций нет.
    pub fn stop_spinning(&mut self) {
        if !self.loading {
            self.spinning = false;
        }
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn is_spinning(&self) -> bool {
        self.spinning
    }

    // ---- Вход ------------------------------------------------------------------------------

    /// Ответ `runner-state` и событие `runner-profile-state`.
    pub fn profile_state(&mut self, state: &Value) -> Vec<Effect> {
        if self.gone || !truthy(Some(state)) || !truthy(prop(state, "status")) {
            return vec![];
        }
        let identity = |value: &Value| {
            format!(
                "{}:{}",
                str_of(prop(value, "status")).unwrap_or_default(),
                str_of(field(prop(value, "account"), "steamId64")).unwrap_or_default()
            )
        };
        let changed = identity(state) != identity(&self.profile);
        self.profile = state.clone();
        if !changed {
            return vec![];
        }
        self.auth_revision += 1;
        self.mine_request += 1;
        let pending: Vec<(String, Pending)> = self.pending.drain().collect();
        for (id, before) in pending {
            self.set_counts(&id, before.counts);
        }
        self.mine.clear();
        self.notices.clear();
        self.load_mine()
    }

    fn signed_in(&self) -> bool {
        str_of(prop(&self.profile, "status")) == Some("signed-in")
    }

    /// SteamID вошедшего.
    pub fn me(&self) -> String {
        str_of(field(prop(&self.profile, "account"), "steamId64"))
            .unwrap_or_default()
            .to_owned()
    }

    pub fn is_signed_in(&self) -> bool {
        self.signed_in()
    }

    fn posts(&self) -> Vec<&Value> {
        self.items
            .iter()
            .filter(|item| str_of(prop(item, "kind")) == Some("post"))
            .collect()
    }

    /// Рекорды и свежие подтверждённые забеги — одним списком по дате.
    fn records(&self) -> Vec<&Value> {
        self.items
            .iter()
            .filter(|item| matches!(str_of(prop(item, "kind")), Some("record" | "run")))
            .collect()
    }

    fn game_key(record: &Value) -> Option<&str> {
        str_of(prop(record, "gameKey"))
            .filter(|key| !key.is_empty())
            .or_else(|| str_of(prop(record, "game")))
    }

    /// `visibleRecords`: игра фильтра; во «Все» — забегов одной таблицы (игра, категория,
    /// подкатегория) не больше [`RUNS_PER_BOARD`], рекорды — все.
    fn visible_records(&self) -> Vec<&Value> {
        let records = self.records();
        if self.record_filter != "all" {
            return records
                .into_iter()
                .filter(|record| Self::game_key(record) == Some(self.record_filter.as_str()))
                .collect();
        }
        let mut seen: HashMap<String, usize> = HashMap::new();
        records
            .into_iter()
            .filter(|record| {
                if str_of(prop(record, "kind")) != Some("run") {
                    return true;
                }
                let board = [
                    Self::game_key(record),
                    str_of(prop(record, "category")),
                    str_of(prop(record, "subcategory")),
                ]
                .map(Option::unwrap_or_default)
                .join("\n");
                let count = seen.entry(board).or_default();
                *count += 1;
                *count <= RUNS_PER_BOARD
            })
            .collect()
    }

    /// `placeOf`: рекорд — 1, свежий забег — `place` с сервера (1–3), иначе 0.
    fn place_of(record: &Value) -> u8 {
        if str_of(prop(record, "kind")) == Some("record") {
            return 1;
        }
        num(prop(record, "place"))
            .filter(|place| [1., 2., 3.].contains(place))
            .map_or(0, |place| place as u8)
    }

    fn post_id(post: &Value) -> String {
        match prop(post, "id") {
            Some(Value::String(id)) => id.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        }
    }

    fn load_mine(&mut self) -> Vec<Effect> {
        let ids: Vec<Value> = self
            .posts()
            .iter()
            .map(|p| json!(Self::post_id(p)))
            .collect();
        if !self.signed_in() || ids.is_empty() {
            return vec![];
        }
        self.mine_request += 1;
        vec![Effect::Invoke {
            tag: Tag::Mine(self.auth_revision, self.mine_request),
            channel: "public-news-mine",
            args: vec![Value::Array(ids)],
        }]
    }

    /// Ответ `public-news-mine`.
    pub fn mine_reply(&mut self, revision: u64, request: u64, reply: Reply) {
        if self.gone || revision != self.auth_revision || request != self.mine_request {
            return;
        }
        let Ok(reply) = reply else { return };
        if str_of(prop(&reply, "status")) != Some("ok") {
            return;
        }
        if let Some(Value::Object(mine)) = prop(&reply, "mine") {
            for (id, keys) in mine {
                if !self.pending.contains_key(id) {
                    self.mine.insert(id.clone(), string_list(keys));
                }
            }
        }
    }

    fn set_counts(&mut self, id: &str, reactions: Value) {
        for item in &mut self.items {
            if str_of(prop(item, "kind")) == Some("post") && Self::post_id(item) == id {
                item["reactions"] = reactions.clone();
            }
        }
    }

    // ---- Реакции и ссылки ------------------------------------------------------------------

    /// Кнопка реакции.
    pub fn react(&mut self, id: &str, key: &str) -> Vec<Effect> {
        if self.pending.contains_key(id) {
            return vec![];
        }
        if !self.signed_in() {
            self.notices
                .insert(id.to_owned(), "#L_HOME_REACT_SIGN_IN".to_owned());
            return vec![];
        }
        let Some(post) = self
            .posts()
            .into_iter()
            .find(|post| Self::post_id(post) == id)
            .cloned()
        else {
            return vec![];
        };
        self.mine_request += 1;
        let counts = prop(&post, "reactions")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let before = Pending {
            counts: counts.clone(),
            mine: self.mine.get(id).cloned().unwrap_or_default(),
        };
        let revision = self.auth_revision;
        self.notices.insert(id.to_owned(), String::new());
        let selected = before.mine.iter().any(|known| known == key);
        let next_mine: Vec<String> = if selected {
            before
                .mine
                .iter()
                .filter(|known| *known != key)
                .cloned()
                .collect()
        } else {
            let mut keys = before.mine.clone();
            keys.push(key.to_owned());
            keys
        };
        self.mine.insert(id.to_owned(), next_mine);
        let count = num(prop(&counts, key)).unwrap_or(0.0);
        let mut changed = counts.as_object().cloned().unwrap_or_default();
        changed.insert(
            key.to_owned(),
            whole((count + if selected { -1.0 } else { 1.0 }).max(0.0)),
        );
        self.pending.insert(id.to_owned(), before);
        self.set_counts(id, Value::Object(changed));
        vec![Effect::Invoke {
            tag: Tag::React(id.to_owned(), key.to_owned(), revision),
            channel: "public-news-react",
            args: vec![json!(id), json!(key)],
        }]
    }

    /// Ответ `public-news-react`.
    pub fn react_reply(&mut self, id: &str, _key: &str, revision: u64, reply: Reply) {
        if self.gone || revision != self.auth_revision {
            return;
        }
        self.mine_request += 1;
        let Some(before) = self.pending.get(id).cloned() else {
            return;
        };
        match reply {
            Ok(answer) if str_of(prop(&answer, "status")) == Some("ok") => {
                if let Some(reactions) = prop(&answer, "reactions") {
                    self.set_counts(id, reactions.clone());
                }
                self.mine.insert(
                    id.to_owned(),
                    prop(&answer, "mine").map(string_list).unwrap_or_default(),
                );
            }
            other => {
                self.set_counts(id, before.counts.clone());
                self.mine.insert(id.to_owned(), before.mine.clone());
                let limited = other
                    .ok()
                    .is_some_and(|answer| str_of(prop(&answer, "code")) == Some("RUNNER_HTTP_429"));
                self.notices.insert(
                    id.to_owned(),
                    if limited {
                        "#L_HOME_REACT_LIMIT"
                    } else {
                        "#L_HOME_REACT_FAILED"
                    }
                    .to_owned(),
                );
            }
        }
        self.pending.remove(id);
    }

    /// Ссылка внутри поста: `public-news-open-link`.
    pub fn open_post_link(&self, url: &str, post_id: &str) -> Vec<Effect> {
        vec![Effect::Invoke {
            tag: Tag::Link(post_id.to_owned()),
            channel: "public-news-open-link",
            args: vec![json!(url)],
        }]
    }

    pub fn link_reply(&mut self, post_id: &str, reply: Reply) {
        let failed = match reply {
            Ok(answer) => {
                !truthy(Some(&answer)) || str_of(prop(&answer, "status")) == Some("error")
            }
            Err(_) => true,
        };
        if failed {
            self.notices
                .insert(post_id.to_owned(), "#L_HOME_LINK_FAILED".to_owned());
        }
    }

    /// Запись рекорда: `runner-open-link`.
    pub fn open_record(&self, url: &str) -> Vec<Effect> {
        vec![Effect::Invoke {
            tag: Tag::Ignored,
            channel: "runner-open-link",
            args: vec![json!(url)],
        }]
    }

    // ---- Вкладки и фильтр ------------------------------------------------------------------

    pub fn tab(&self) -> &str {
        &self.active_tab
    }

    pub fn select_tab(&mut self, tab: &str) {
        let ids: Vec<String> = TABS.iter().map(|id| id.to_string()).collect();
        self.slide_next = direction_next(&ids, &self.active_tab, tab);
        self.active_tab = tab.to_owned();
    }

    pub fn set_filter(&mut self, key: &str) {
        if key == self.record_filter {
            return;
        }
        let mut keys = vec!["all".to_owned()];
        keys.extend(self.record_games().iter().map(|(key, _)| key.to_string()));
        self.filter_next = direction_next(&keys, &self.record_filter, key);
        self.record_filter = key.to_owned();
    }

    pub fn filter(&self) -> &str {
        &self.record_filter
    }

    pub fn filter_next(&self) -> bool {
        self.filter_next
    }

    /// Игры рекордов по порядку фильтра: ключ и полное название.
    fn record_games(&self) -> Vec<(String, String)> {
        let mut games: Vec<(String, String)> = vec![];
        for record in self.records() {
            let title = str_of(prop(record, "game")).unwrap_or_default().to_owned();
            let key = str_of(prop(record, "gameKey"))
                .filter(|key| !key.is_empty())
                .map_or_else(|| title.clone(), str::to_owned);
            if !games.iter().any(|(known, _)| *known == key) {
                games.push((key, title));
            }
        }
        games.sort_by_key(|(key, _)| game_rank(key));
        games
    }

    // ---- Просмотр картинок -----------------------------------------------------------------

    pub fn show_image(&mut self, post_id: &str, index: usize) -> Vec<Effect> {
        let Some(post) = self
            .posts()
            .into_iter()
            .find(|post| Self::post_id(post) == post_id)
            .cloned()
        else {
            return vec![];
        };
        let images: Vec<String> = prop(&post, "images")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .map(|image| str_of(prop(image, "url")).unwrap_or_default().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        self.viewer = Some(Viewer {
            images,
            title: str_of(prop(&post, "title")).unwrap_or_default().to_owned(),
            index,
        });
        vec![Effect::Overlay(true)]
    }

    pub fn close_image(&mut self) -> Vec<Effect> {
        if self.viewer.take().is_some() {
            vec![Effect::Overlay(false)]
        } else {
            vec![]
        }
    }

    /// ←/→ и кнопки «Назад»/«Вперёд» окна просмотра.
    pub fn move_image(&mut self, delta: i32) {
        if let Some(viewer) = &mut self.viewer {
            let last = viewer.images.len().saturating_sub(1) as i32;
            viewer.index = (viewer.index as i32 + delta).clamp(0, last) as usize;
        }
    }

    pub fn viewer(&self) -> Option<&Viewer> {
        self.viewer.as_ref()
    }

    // ---- Вид -------------------------------------------------------------------------------

    fn date_ms(iso: &str) -> f64 {
        chrono::DateTime::parse_from_rfc3339(iso)
            .map(|at| at.timestamp_millis() as f64)
            .unwrap_or(f64::NAN)
    }

    fn post_view(&self, post: &Value, lang: Language, tr: &Tr) -> PostView {
        let id = Self::post_id(post);
        let profile = prop(post, "authorProfile").filter(|profile| truthy(Some(*profile)));
        let name = str_of(field(profile, "personaName"))
            .filter(|name| !name.is_empty())
            .or_else(|| str_of(prop(post, "author")).filter(|name| !name.is_empty()))
            .map_or_else(|| tr("#L_HOME_AUTHOR_FALLBACK", &[]), str::to_owned);
        let iso = str_of(prop(post, "date")).unwrap_or_default();
        let ms = Self::date_ms(iso);
        let day = self.wall.format(lang, DateStyle::DateLong, ms);
        let time = self.wall.format(lang, DateStyle::Time, ms);
        let stamp = tr("#L_HOME_POSTED_AT", &[])
            .replacen("{date}", &day, 1)
            .replacen("{time}", &time, 1);
        let images = prop(post, "images")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let count = images.len();
        let tiles = images
            .iter()
            .enumerate()
            .map(|(index, image)| {
                let tile = TileImage {
                    width: num(prop(image, "width")).unwrap_or(1.0),
                    height: num(prop(image, "height")).unwrap_or(1.0),
                    crop: parse_crop(prop(image, "crop")),
                };
                let aspect = tile_aspect(&tile, index, count);
                let style = tile_image_style(&tile, aspect);
                let fraction = |text: &str| -> f32 {
                    text.trim_end_matches('%').parse::<f64>().unwrap_or(0.0) as f32 / 100.0
                };
                Tile {
                    index: index as i32,
                    url: str_of(prop(image, "url")).unwrap_or_default().to_owned(),
                    aspect: aspect as f32,
                    wide: count > 1 && count % 2 == 1 && index == 0,
                    width: fraction(&style.width),
                    height: fraction(&style.height),
                    left: fraction(&style.left),
                    top: fraction(&style.top),
                }
            })
            .collect();
        let counts = prop(post, "reactions")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let mine = self.mine.get(&id).cloned().unwrap_or_default();
        let reactions = REACTIONS
            .iter()
            .map(|(key, emoji)| {
                let count = num(prop(&counts, key)).unwrap_or(0.0);
                ReactionView {
                    key: (*key).to_owned(),
                    emoji: (*emoji).to_owned(),
                    count: count as i32,
                    selected: mine.iter().any(|known| known == key),
                    label: format!(
                        "{} · {}",
                        tr(&format!("#L_HOME_REACTION_{key}"), &[]),
                        super::jsv::number_text(count)
                    ),
                }
            })
            .collect();
        let notice_key = self.notices.get(&id).cloned().unwrap_or_default();
        let avatar_broken = false;
        let avatar = str_of(field(profile, "avatarUrl"))
            .filter(|url| !url.is_empty() && !avatar_broken)
            .unwrap_or_default()
            .to_owned();
        let nick = field(profile, "nick");
        PostView {
            author_id: str_of(field(profile, "steamId64"))
                .unwrap_or_default()
                .to_owned(),
            letter: name
                .trim()
                .chars()
                .next()
                .map_or_else(|| "?".to_owned(), |c| c.to_string()),
            name,
            avatar,
            nick_style: str_of(field(nick, "style")).unwrap_or_default().to_owned(),
            nick_css: str_of(field(nick, "css")).unwrap_or_default().to_owned(),
            frame_css: str_of(field(field(profile, "avatarFrame"), "css"))
                .unwrap_or_default()
                .to_owned(),
            subscriber: field(profile, "subscriber") == Some(&Value::Bool(true)),
            date: day,
            stamp,
            title: str_of(prop(post, "title")).unwrap_or_default().to_owned(),
            body: str_of(prop(post, "body")).unwrap_or_default().to_owned(),
            single_image: count == 1,
            tiles,
            reactions,
            pending: self.pending.contains_key(&id),
            notice: if notice_key.is_empty() {
                String::new()
            } else {
                tr(&notice_key, &[])
            },
            notice_sign_in: notice_key == "#L_HOME_REACT_SIGN_IN",
            id,
        }
    }

    fn record_view(&self, record: &Value, lang: Language, tr: &Tr) -> RecordView {
        let iso = str_of(prop(record, "date")).unwrap_or_default();
        let category = str_of(prop(record, "category")).unwrap_or_default();
        let game = str_of(prop(record, "game")).unwrap_or_default();
        let profiles = prop(record, "profiles").and_then(Value::as_array);
        let runners: Vec<Runner> = prop(record, "runners")
            .and_then(Value::as_array)
            .map(|list| {
                // `profileOf(record, i)`: профиль по исходной позиции бегуна в `runners`.
                list.iter()
                    .enumerate()
                    .filter_map(|(i, name)| Some((i, name.as_str()?)))
                    .map(|(i, name)| Runner {
                        name: name.to_owned(),
                        profile: str_of(profiles.and_then(|list| list.get(i)))
                            .unwrap_or_default()
                            .to_owned(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let sub = str_of(prop(record, "subcategory")).unwrap_or_default();
        let place = Self::place_of(record);
        RecordView {
            id: Self::post_id(record),
            url: str_of(prop(record, "url")).unwrap_or_default().to_owned(),
            game: if self.record_filter == "all" {
                format!("{game} · {category}")
            } else {
                category.to_owned()
            },
            date: self
                .wall
                .format(lang, DateStyle::DateLong, Self::date_ms(iso)),
            iso: iso.to_owned(),
            place,
            place_label: match place {
                0 => String::new(),
                1 => tr("#L_HOME_WR", &[]),
                n => tr("#L_HOME_PLACE", &[n.to_string().as_str()]),
            },
            subs: sub
                .split(" · ")
                .filter(|part| !part.is_empty())
                .map(str::to_owned)
                .collect(),
            time: format_run(num(prop(record, "seconds")).unwrap_or(f64::NAN)),
            runner: runners
                .iter()
                .map(|runner| runner.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            runners,
        }
    }

    pub fn view(&self, lang: Language, tr: &Tr) -> HomeView {
        let posts = self.posts();
        let records = self.records();
        let games = self.record_games();
        let mut chips = vec![Chip {
            key: "all".into(),
            label: tr("#L_HOME_ALL_GAMES", &[]),
            title: tr("#L_HOME_ALL_GAMES", &[]),
            active: self.record_filter == "all",
        }];
        if games.len() > 1 {
            chips.extend(games.iter().map(|(key, title)| Chip {
                key: key.clone(),
                label: game_short(key).map_or_else(|| title.clone(), str::to_owned),
                title: title.clone(),
                active: *key == self.record_filter,
            }));
        } else {
            chips.clear();
        }
        let visible = self.visible_records();
        let waiting = self.loading && !self.loaded;
        HomeView {
            loading: self.loading,
            spinning: self.spinning,
            note: if self.failed && self.active_tab != "players" {
                tr(
                    if self.items.is_empty() {
                        "#L_HOME_FAILED"
                    } else {
                        "#L_HOME_STALE"
                    },
                    &[],
                )
            } else {
                String::new()
            },
            posts: posts
                .iter()
                .map(|post| self.post_view(post, lang, tr))
                .collect(),
            records: visible
                .iter()
                .map(|record| self.record_view(record, lang, tr))
                .collect(),
            chips,
            empty_posts: if posts.is_empty() {
                tr(
                    if waiting {
                        "#L_HOME_LOADING"
                    } else {
                        "#L_HOME_NO_POSTS"
                    },
                    &[],
                )
            } else {
                String::new()
            },
            empty_records: if records.is_empty() {
                tr(
                    if waiting {
                        "#L_HOME_LOADING"
                    } else {
                        "#L_HOME_NO_RECORDS"
                    },
                    &[],
                )
            } else {
                String::new()
            },
            tab: TABS
                .iter()
                .position(|id| *id == self.active_tab)
                .unwrap_or(0) as i32,
            slide_next: self.slide_next,
        }
    }

    /// Записи при текущем фильтре (тесты).
    pub fn visible_record_ids(&self) -> Vec<String> {
        self.visible_records()
            .iter()
            .map(|record| Self::post_id(record))
            .collect()
    }

    /// Счётчики реакций поста (тесты).
    pub fn counts(&self, id: &str) -> Value {
        self.posts()
            .into_iter()
            .find(|post| Self::post_id(post) == id)
            .and_then(|post| prop(post, "reactions").cloned())
            .unwrap_or(Value::Null)
    }

    /// Мои реакции на пост (тесты).
    pub fn mine_of(&self, id: &str) -> Vec<String> {
        self.mine.get(id).cloned().unwrap_or_default()
    }

    /// Замечание поста (тесты).
    pub fn notice(&self, id: &str) -> String {
        self.notices.get(id).cloned().unwrap_or_default()
    }
}

/// Целое число JSON, когда значение целое (счётчики приходят из main целыми).
fn whole(value: f64) -> Value {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
        json!(value as i64)
    } else {
        json!(value)
    }
}

fn string_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Пустой ответ ленты для сцен и тестов.
pub fn empty_feed() -> Value {
    json!({"status": "ok", "items": Vec::<Value>::new(), "mine": Map::new()})
}

/// Длина текста в кодовых единицах UTF-16 (счёт `String#length`).
pub fn js_length(text: &str) -> usize {
    utf16_index(text)
}

#[cfg(test)]
mod tests;
