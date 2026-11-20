//! Вкладка «Игроки» (`HomePlayers.vue` 0.4.9): «В сети» (обновляется раз в 30 с, пока вкладка
//! открыта) и «Рейтинг» (пять сортировок, фильтр игр, «ещё»; у «Новых» мест нет). Машина без
//! Slint: побочные действия возвращает списком, ответы возвращаются в неё методами по тегам.

use super::account::short_name;
use super::bridge::Tr;
use super::jsv::{field, num, prop, str_of, truthy};
use crate::intl::{self, DateStyle, Plural};
use crate::services::Reply;
use hlsr_core::language::Language;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;

/// Как часто обновляется «В сети».
pub const ONLINE_REFRESH: Duration = Duration::from_millis(30_000);
/// Размер страницы рейтинга.
const PAGE: usize = 50;

/// Игры, по которым считаются часы на сервере; у трёх сборок HL2 одно короткое имя в
/// `launcher-data`, поэтому подписи свои.
pub const GAMES: [(&str, &str); 6] = [
    ("", ""),
    ("hl1-won", "Half-Life"),
    ("hl2-2707", "HL2 2707"),
    ("hl2-4260", "HL2 4260"),
    ("hl2-5288", "HL2 5288"),
    ("ghosting-renewed", "Ghosting"),
];

/// Сортировки рейтинга: код, ключ подписи, значок.
pub const SORTS: [(&str, &str, &str); 5] = [
    ("hours", "#L_PLAYERS_SORT_HOURS", "fas fa-clock"),
    (
        "achievements",
        "#L_PLAYERS_SORT_ACHIEVEMENTS",
        "fas fa-trophy",
    ),
    ("launches", "#L_PLAYERS_SORT_LAUNCHES", "fas fa-play"),
    ("rating", "#L_PLAYERS_SORT_RATING", "fas fa-thumbs-up"),
    ("new", "#L_PLAYERS_SORT_NEW", "fas fa-user-plus"),
];

/// Побочное действие.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Invoke {
        tag: Tag,
        channel: &'static str,
        args: Vec<Value>,
    },
    /// Включить или выключить опрос «В сети» раз в 30 с.
    Poll(bool),
    /// `$emit("open-profile", id)`; пустой — вход.
    OpenProfile(String),
    /// Список прокручен к началу.
    ResetScroll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Online(u64),
    Top(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Online,
    Top,
}

#[derive(Debug, Default)]
struct List {
    profiles: Vec<Value>,
    loading: bool,
    failed: bool,
    limited: bool,
    loaded: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Chip {
    pub id: String,
    pub label: String,
    pub icon: String,
    pub count: String,
    pub active: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Row {
    pub id: String,
    pub me: bool,
    /// Место в рейтинге; 0 — без места («В сети», сортировка «Новые»).
    pub rank: i32,
    /// Значок подписчика у ника (`profile.subscriber === true`).
    pub subscriber: bool,
    pub name: String,
    pub letter: String,
    pub avatar: String,
    pub nick_style: String,
    pub nick_css: String,
    pub frame_css: String,
    /// `playing`, `online`, `offline`.
    pub presence: &'static str,
    pub presence_text: String,
    pub value: String,
    pub unit: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayersView {
    pub modes: Vec<Chip>,
    pub sorts: Vec<Chip>,
    pub games: Vec<Chip>,
    pub summary: String,
    pub rows: Vec<Row>,
    /// Сообщение вместо списка (вход, загрузка, сбой, пусто).
    pub message: String,
    pub sign_in_button: bool,
    pub more: bool,
    /// Следующая страница грузится: «Показать ещё» недоступна.
    pub more_loading: bool,
    pub more_label: String,
    pub top: bool,
}

#[derive(Debug)]
pub struct PlayersState {
    mode: Mode,
    sort: String,
    game: String,
    online: List,
    online_total: i64,
    online_playing: i64,
    top: List,
    top_more: bool,
    broken: HashMap<String, bool>,
    request_online: u64,
    request_top: u64,
    active: bool,
    signed_in: bool,
    /// `document.hidden`: окно свёрнуто или спрятано в трей.
    hidden: bool,
    me: String,
}

fn number(value: Option<&Value>) -> f64 {
    num(value).unwrap_or(0.0)
}

impl Default for PlayersState {
    fn default() -> Self {
        PlayersState::new()
    }
}

impl PlayersState {
    pub fn new() -> PlayersState {
        PlayersState {
            mode: Mode::Online,
            sort: "hours".to_owned(),
            game: String::new(),
            online: List::default(),
            online_total: 0,
            online_playing: 0,
            top: List::default(),
            top_more: false,
            broken: HashMap::new(),
            request_online: 0,
            request_top: 0,
            active: false,
            signed_in: false,
            hidden: false,
            me: String::new(),
        }
    }

    fn per_game(&self) -> bool {
        self.sort == "hours" || self.sort == "launches"
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// `mounted`: если вкладка уже открыта — первый запрос; опрос по расписанию.
    pub fn mounted(&mut self) -> Vec<Effect> {
        let mut effects = vec![];
        if self.active {
            effects.extend(self.refresh(false));
        }
        effects.push(Effect::Poll(self.polling()));
        effects
    }

    /// `beforeUnmount`: ответы прежних запросов отбрасываются, опрос остановлен.
    pub fn unmounted(&mut self) -> Vec<Effect> {
        self.request_online += 1;
        self.request_top += 1;
        vec![Effect::Poll(false)]
    }

    fn polling(&self) -> bool {
        self.active && self.signed_in && self.mode == Mode::Online && !self.hidden
    }

    /// `visibilitychange` → `schedule()`: у скрытого окна опроса «В сети» нет, вернулось — опрос
    /// заново (без внеочередного запроса).
    pub fn set_hidden(&mut self, hidden: bool) -> Vec<Effect> {
        self.hidden = hidden;
        vec![Effect::Poll(self.polling())]
    }

    /// `watch active`.
    pub fn set_active(&mut self, active: bool) -> Vec<Effect> {
        if self.active == active {
            return vec![];
        }
        self.active = active;
        let mut effects = vec![];
        if active {
            effects.extend(self.refresh(false));
        }
        effects.push(Effect::Poll(self.polling()));
        effects
    }

    /// `watch signedIn` (и `me`): списки сброшены.
    pub fn set_signed_in(&mut self, signed_in: bool, me: &str) -> Vec<Effect> {
        self.me = me.to_owned();
        if self.signed_in == signed_in {
            return vec![];
        }
        self.signed_in = signed_in;
        self.online = List::default();
        self.online_total = 0;
        self.online_playing = 0;
        self.top = List::default();
        self.top_more = false;
        let mut effects = vec![];
        if self.active {
            effects.extend(self.refresh(false));
        }
        effects.push(Effect::Poll(self.polling()));
        effects
    }

    /// Кнопка «Обновить» шапки «Главной» и первое открытие вкладки.
    pub fn refresh(&mut self, force: bool) -> Vec<Effect> {
        if !self.signed_in {
            return vec![];
        }
        match self.mode {
            Mode::Online => self.load_online(),
            Mode::Top if force || !self.top.loaded => self.load_top(false),
            Mode::Top => vec![],
        }
    }

    /// Сработал опрос раз в 30 с.
    pub fn poll(&mut self) -> Vec<Effect> {
        self.load_online()
    }

    pub fn set_mode(&mut self, mode: Mode) -> Vec<Effect> {
        if mode == self.mode {
            return vec![];
        }
        self.mode = mode;
        let mut effects = vec![Effect::ResetScroll];
        effects.extend(self.refresh(false));
        effects.push(Effect::Poll(self.polling()));
        effects
    }

    pub fn set_sort(&mut self, sort: &str) -> Vec<Effect> {
        if sort == self.sort {
            return vec![];
        }
        self.sort = sort.to_owned();
        if !self.per_game() {
            self.game.clear();
        }
        self.load_top(false)
    }

    pub fn set_game(&mut self, game: &str) -> Vec<Effect> {
        if game == self.game {
            return vec![];
        }
        self.game = game.to_owned();
        self.load_top(false)
    }

    fn load_online(&mut self) -> Vec<Effect> {
        self.request_online += 1;
        self.online.loading = true;
        vec![Effect::Invoke {
            tag: Tag::Online(self.request_online),
            channel: "runner-players",
            args: vec![json!({"view": "online"})],
        }]
    }

    /// Ответ `runner-players` для «В сети».
    pub fn online_reply(&mut self, request: u64, reply: Reply) {
        if request != self.request_online {
            return;
        }
        self.online.loading = false;
        self.online.loaded = true;
        match reply {
            Ok(answer)
                if str_of(prop(&answer, "status")) == Some("ok")
                    && prop(&answer, "signedIn") != Some(&Value::Bool(false)) =>
            {
                self.online.profiles = prop(&answer, "profiles")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                self.online_total = number(prop(&answer, "total")) as i64;
                self.online_playing = number(prop(&answer, "playing")) as i64;
                self.online.failed = false;
                self.online.limited = false;
            }
            other => {
                // Прежний список остаётся: при сбое опроса он лишь немного устарел.
                self.online.failed = true;
                self.online.limited = other
                    .ok()
                    .is_some_and(|answer| str_of(prop(&answer, "code")) == Some("RUNNER_HTTP_429"));
            }
        }
    }

    /// `more` — следующая страница того же рейтинга; иначе рейтинг заново.
    pub fn load_top(&mut self, more: bool) -> Vec<Effect> {
        self.request_top += 1;
        let offset = if more { self.top.profiles.len() } else { 0 };
        let mut effects = vec![];
        if !more {
            self.top.profiles.clear();
            self.top_more = false;
            effects.push(Effect::ResetScroll);
        }
        self.top.loading = true;
        let game = if self.per_game() {
            self.game.clone()
        } else {
            String::new()
        };
        effects.push(Effect::Invoke {
            tag: Tag::Top(self.request_top),
            channel: "runner-players",
            args: vec![json!({
                "view": "top", "sort": self.sort, "game": game,
                "offset": (offset / PAGE) * PAGE
            })],
        });
        effects
    }

    /// Ответ `runner-players` для рейтинга.
    pub fn top_reply(&mut self, request: u64, reply: Reply) {
        if request != self.request_top {
            return;
        }
        self.top.loading = false;
        self.top.loaded = true;
        match reply {
            Ok(answer)
                if str_of(prop(&answer, "status")) == Some("ok")
                    && prop(&answer, "signedIn") != Some(&Value::Bool(false)) =>
            {
                let known: Vec<String> = self
                    .top
                    .profiles
                    .iter()
                    .map(|p| str_of(prop(p, "steamId64")).unwrap_or_default().to_owned())
                    .collect();
                if let Some(fresh) = prop(&answer, "profiles").and_then(Value::as_array) {
                    for profile in fresh {
                        let id = str_of(prop(profile, "steamId64")).unwrap_or_default();
                        if !known.iter().any(|known| known == id) {
                            self.top.profiles.push(profile.clone());
                        }
                    }
                }
                self.top_more = truthy(prop(&answer, "more"));
                self.top.failed = false;
                self.top.limited = false;
            }
            other => {
                self.top.failed = true;
                self.top.limited = other
                    .ok()
                    .is_some_and(|answer| str_of(prop(&answer, "code")) == Some("RUNNER_HTTP_429"));
            }
        }
    }

    /// Щелчок по строке, кнопка «Войти».
    pub fn open_profile(&self, id: &str) -> Vec<Effect> {
        vec![Effect::OpenProfile(id.to_owned())]
    }

    // ---- Вид -------------------------------------------------------------------------------

    fn current(&self) -> &List {
        match self.mode {
            Mode::Online => &self.online,
            Mode::Top => &self.top,
        }
    }

    fn sum_of(&self, profile: &Value, key: &str) -> f64 {
        let games = prop(profile, "games").and_then(Value::as_object);
        if self.mode == Mode::Top && self.per_game() && !self.game.is_empty() {
            return games
                .and_then(|games| games.get(&self.game))
                .map_or(0.0, |one| number(prop(one, key)));
        }
        games.map_or(0.0, |games| {
            games.values().map(|game| number(prop(game, key))).sum()
        })
    }

    fn count_word(lang: Language, tr: &Tr, key: &str, count: f64) -> String {
        let suffix = match intl::plural(lang, count) {
            Plural::One => "_ONE",
            Plural::Few => "_FEW",
            _ => "_MANY",
        };
        tr(&format!("{key}{suffix}"), &[])
    }

    fn value_of(&self, profile: &Value, lang: Language, tr: &Tr) -> String {
        match self.sort.as_str() {
            "hours" => {
                let hours = self.sum_of(profile, "totalPlaySeconds") / 3600.0;
                let shown = if hours >= 10.0 {
                    hours.floor()
                } else {
                    (hours * 10.0).floor() / 10.0
                };
                tr(
                    "#L_PLAYERS_HOURS",
                    &[intl::format_number(lang, shown).as_str()],
                )
            }
            "launches" => intl::format_number(lang, self.sum_of(profile, "launchCount")),
            "achievements" => super::jsv::number_text(number(prop(profile, "achievementCount"))),
            "rating" => {
                let score = number(field(prop(profile, "rating"), "score"));
                format!(
                    "{}{}",
                    if score > 0.0 { "+" } else { "" },
                    super::jsv::number_text(score)
                )
            }
            _ => match str_of(prop(profile, "registeredAt")).filter(|at| !at.is_empty()) {
                None => "—".to_owned(),
                Some(at) => {
                    let ms = chrono::DateTime::parse_from_rfc3339(at)
                        .map(|at| at.timestamp_millis() as f64)
                        .unwrap_or(f64::NAN);
                    intl::format_date(lang, DateStyle::DateShort, ms)
                }
            },
        }
    }

    fn unit_of(&self, profile: &Value, lang: Language, tr: &Tr) -> String {
        match self.sort.as_str() {
            "launches" => Self::count_word(
                lang,
                tr,
                "#L_PLAYERS_UNIT_LAUNCHES",
                self.sum_of(profile, "launchCount"),
            ),
            "achievements" => Self::count_word(
                lang,
                tr,
                "#L_PLAYERS_UNIT_ACHIEVEMENTS",
                number(prop(profile, "achievementCount")),
            ),
            "rating" => tr("#L_PLAYERS_UNIT_RATING", &[]),
            "new" => tr("#L_PLAYERS_UNIT_JOINED", &[]),
            _ => String::new(),
        }
    }

    fn row(&self, profile: &Value, lang: Language, tr: &Tr) -> Row {
        let id = str_of(prop(profile, "steamId64")).unwrap_or_default();
        let name = str_of(prop(profile, "personaName")).filter(|name| !name.is_empty());
        let presence = prop(profile, "presence");
        let (state, text) = {
            let playing = truthy(field(presence, "gameId"));
            let state = if playing {
                "playing"
            } else if truthy(field(presence, "online")) {
                "online"
            } else {
                "offline"
            };
            let game = str_of(field(presence, "launcherId")).and_then(short_name);
            let text = match game {
                Some(game) => tr("#L_PRESENCE_PLAYING", &[game]),
                None => tr(
                    if truthy(field(presence, "online")) {
                        "#L_PRESENCE_ONLINE"
                    } else {
                        "#L_PRESENCE_OFFLINE"
                    },
                    &[],
                ),
            };
            (state, text)
        };
        let top = self.mode == Mode::Top;
        Row {
            id: id.to_owned(),
            me: id == self.me && !self.me.is_empty(),
            rank: if top && self.sort != "new" {
                number(prop(profile, "rank")) as i32
            } else {
                0
            },
            subscriber: prop(profile, "subscriber") == Some(&Value::Bool(true)),
            name: name.unwrap_or(id).to_owned(),
            letter: name
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .and_then(|name| name.chars().next())
                .map_or_else(|| "?".to_owned(), |c| c.to_uppercase().collect()),
            avatar: if self.broken.get(id).copied().unwrap_or(false) {
                String::new()
            } else {
                str_of(field(prop(profile, "details"), "avatarUrl"))
                    .unwrap_or_default()
                    .to_owned()
            },
            nick_style: str_of(field(prop(profile, "nick"), "style"))
                .unwrap_or_default()
                .to_owned(),
            nick_css: str_of(field(prop(profile, "nick"), "css"))
                .unwrap_or_default()
                .to_owned(),
            frame_css: str_of(field(prop(profile, "avatarFrame"), "css"))
                .unwrap_or_default()
                .to_owned(),
            presence: state,
            presence_text: text,
            value: if top {
                self.value_of(profile, lang, tr)
            } else {
                String::new()
            },
            unit: if top {
                self.unit_of(profile, lang, tr)
            } else {
                String::new()
            },
        }
    }

    pub fn view(&self, lang: Language, tr: &Tr) -> PlayersView {
        let modes = vec![
            Chip {
                id: "online".into(),
                label: tr("#L_PLAYERS_ONLINE", &[]),
                count: if self.online.loaded {
                    self.online_total.to_string()
                } else {
                    String::new()
                },
                active: self.mode == Mode::Online,
                ..Chip::default()
            },
            Chip {
                id: "top".into(),
                label: tr("#L_PLAYERS_TOP", &[]),
                active: self.mode == Mode::Top,
                ..Chip::default()
            },
        ];
        let top = self.mode == Mode::Top;
        let sorts = if top {
            SORTS
                .iter()
                .map(|(id, key, icon)| Chip {
                    id: (*id).into(),
                    label: tr(key, &[]),
                    icon: (*icon).into(),
                    active: self.sort == *id,
                    ..Chip::default()
                })
                .collect()
        } else {
            vec![]
        };
        let games = if top && self.per_game() {
            GAMES
                .iter()
                .map(|(id, name)| Chip {
                    id: (*id).into(),
                    label: if id.is_empty() {
                        tr("#L_HOME_ALL_GAMES", &[])
                    } else {
                        (*name).into()
                    },
                    active: self.game == *id,
                    ..Chip::default()
                })
                .collect()
        } else {
            vec![]
        };
        let list = self.current();
        let (message, rows) = if !self.signed_in {
            (tr("#L_PLAYERS_SIGN_IN", &[]), vec![])
        } else if list.loading && list.profiles.is_empty() {
            (tr("#L_HOME_LOADING", &[]), vec![])
        } else if list.failed && list.profiles.is_empty() {
            (
                tr(
                    if list.limited {
                        "#L_PLAYERS_LIMIT"
                    } else {
                        "#L_PLAYERS_FAILED"
                    },
                    &[],
                ),
                vec![],
            )
        } else if list.profiles.is_empty() {
            (
                tr(
                    if top {
                        "#L_PLAYERS_EMPTY"
                    } else {
                        "#L_PLAYERS_NOBODY"
                    },
                    &[],
                ),
                vec![],
            )
        } else {
            (
                String::new(),
                list.profiles
                    .iter()
                    .map(|profile| self.row(profile, lang, tr))
                    .collect(),
            )
        };
        PlayersView {
            modes,
            sorts,
            games,
            summary: if !top && self.signed_in && self.online.loaded {
                tr(
                    "#L_PLAYERS_SUMMARY",
                    &[
                        self.online_total.to_string().as_str(),
                        self.online_playing.to_string().as_str(),
                    ],
                )
            } else {
                String::new()
            },
            rows,
            message,
            sign_in_button: !self.signed_in,
            more: top && self.top_more && self.signed_in,
            more_loading: self.top.loading,
            more_label: tr(
                if self.top.loading {
                    "#L_HOME_LOADING"
                } else {
                    "#L_PLAYERS_MORE"
                },
                &[],
            ),
            top,
        }
    }

    /// Профили текущего списка (тесты).
    pub fn ids(&self) -> Vec<String> {
        self.current()
            .profiles
            .iter()
            .map(|p| str_of(prop(p, "steamId64")).unwrap_or_default().to_owned())
            .collect()
    }

    pub fn sort(&self) -> &str {
        &self.sort
    }

    pub fn game(&self) -> &str {
        &self.game
    }
}

#[cfg(test)]
mod tests;
