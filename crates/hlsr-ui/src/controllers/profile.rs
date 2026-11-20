//! Окно профиля (`ProfileModal.vue` 0.4.9): что показывают вход, чужой профиль, шапка с ником и
//! значками, рейтинг, меню «⋯», сводка, закреплённый забег, ссылки и достижения по ответам
//! `profile-get`, `runner-profile-view`, `runner-state`, `runner-login`, `runner-rate` и событию
//! `runner-profile-state`. Машина без UI: методы возвращают побочные действия ([`Effect`]), вид
//! (`hlsr-gpui::profile`) выполняет их — вызовы сервисов, буфер обмена, таймеры.
//!
//! Дельта 0.4.9: подписчик (`subscriber`, `supporterSince` — голубая рамка окна и значок у ника),
//! 100 % достижений (золотая рамка), значки-картинки `developer/bughunter/booster` и годовщины
//! 1–10 со свечением, меню «⋯» вместо строки SteamID, в сводке запуски, «N из M» и серия входов,
//! данные вкладки «Настройки» (`perks`, `stylePreset`, `telegram`, `discordLink`); у достижений —
//! редкость полученного (`tier`), замок секретного (`hidden`) и дата получения у своего (`at`).

use super::account::short_name;
use super::bridge::Tr;
use super::home::format_run;
use super::jsv::{field, num, or_zero, prop, str_of, truthy};
use super::wall_clock::WallClock;
use crate::intl::{self, DateStyle, Plural, SystemStyle};
use hlsr_core::js;
use hlsr_core::language::Language;
use hlsr_online::shared::{link_guard, profile_links, pronouns};
use serde_json::{Value, json};

/// Сколько висит подсказка под рейтингом и подтверждение входа.
pub const NOTE_MS: u64 = 4000;

/// Ошибки входа, у которых есть свой текст (`LOGIN_ERRORS`); остальные — «нет связи».
const LOGIN_ERRORS: [(&str, &str); 5] = [
    ("RUNNER_SESSION_EXPIRED", "#L_RUNNER_SESSION_EXPIRED"),
    ("RUNNER_CLIENT_MODIFIED", "#L_RUNNER_CLIENT_MODIFIED"),
    ("RUNNER_CLIENT_UNVERIFIED", "#L_RUNNER_CLIENT_UNVERIFIED"),
    ("RUNNER_CLIENT_OUTDATED", "#L_RUNNER_CLIENT_OUTDATED"),
    ("RUNNER_ATTEST_FAILED", "#L_RUNNER_ATTEST_FAILED"),
];

/// Что в окне.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Чужой профиль грузится или не загрузился.
    Pending,
    /// У пользователя нет профиля HLSR.
    Missing,
    /// Вход через Steam (гость, ожидание кода).
    SignIn,
    /// Шапка, вкладки, разделы.
    Main,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Pending => "pending",
            Kind::Missing => "missing",
            Kind::SignIn => "signin",
            Kind::Main => "main",
        }
    }
}

/// Чему ответ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    /// `runner-state` перед чужим профилем (`own`).
    Own,
    /// `profile-get` или `runner-profile-view`.
    Snapshot,
    /// `runner-login`.
    Login,
    /// `runner-login-cancel`.
    LoginCancel,
    /// `runner-rate`.
    Rate,
    /// Ответ не нужен (`.catch(() => {})`).
    Ignored,
}

/// Побочное действие.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Invoke {
        tag: Tag,
        channel: &'static str,
        args: Vec<Value>,
    },
    /// `$emit("server-hide-id", details.hideSteamId)`: значение с сервера (не булево — `None`).
    ServerHideId(Option<bool>),
    /// `clipboard.writeText(text)`.
    Copy(String),
    /// `$emit("notify", key)`.
    Notify(&'static str),
    /// Подсказка под рейтингом показана (или убрана): таймер на `NOTE_MS`.
    VoteNoteTimer(bool),
    /// Подтверждение входа показано (или убрано): таймер на `NOTE_MS`.
    LoginSuccessTimer(bool),
}

/// Значок под ником.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BadgeView {
    pub id: String,
    pub label: String,
    /// Классы FontAwesome встроенного значка-глифа (`verified`).
    pub icon: String,
    /// `data:`-адрес SVG своего значка.
    pub image: String,
    pub themed: bool,
    /// Картинка встроенного значка (`assets/launcher/badges/<asset>.png`): `developer`,
    /// `bughunter`, `booster`, `anniversary-N`.
    pub asset: &'static str,
    /// Цвет свечения `--glow` (rgb): у картинок разработчика, баг-хантера, бустера и годовщин со 2-й.
    pub glow: Option<[u8; 3]>,
    /// Годовщина: картинка 30 px вместо 24 (поле под нарисованное свечение).
    pub anniversary: bool,
}

/// Пункт меню «⋯» в шапке.
#[derive(Debug, Clone, PartialEq)]
pub struct MoreItem {
    /// `copy` или `steam`.
    pub id: &'static str,
    pub icon: &'static str,
    pub label: String,
}

/// Значки годовщины `anniversary-1…10` (`shared/anniversary-badges.js`).
pub const ANNIVERSARY_IDS: [&str; 10] = [
    "anniversary-1",
    "anniversary-2",
    "anniversary-3",
    "anniversary-4",
    "anniversary-5",
    "anniversary-6",
    "anniversary-7",
    "anniversary-8",
    "anniversary-9",
    "anniversary-10",
];

/// `GLOW` годовщин: преобладающий оттенок картинки; у первой свечения нет.
const ANNIVERSARY_GLOW: [Option<[u8; 3]>; 10] = [
    None,
    Some([48, 156, 255]),
    Some([46, 185, 255]),
    Some([47, 181, 255]),
    Some([33, 214, 255]),
    Some([66, 67, 255]),
    Some([123, 65, 255]),
    Some([180, 33, 255]),
    Some([206, 39, 255]),
    Some([255, 178, 85]),
];

/// Вкладка окна.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TabView {
    pub label: String,
    /// Классы FontAwesome значка вкладки.
    pub icon: &'static str,
    pub count: String,
}

/// Плитка сводки.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatView {
    pub icon: &'static str,
    pub label: String,
    pub value: String,
    pub wide: bool,
}

/// Закреплённый забег.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunView {
    pub game: String,
    pub category: String,
    pub time: String,
    pub url: String,
    /// Место (0 — нет).
    pub place: i64,
    /// «gold», «silver», «bronze» или пусто.
    pub medal: &'static str,
    /// Подпись места (`RunPlace.label`).
    pub place_label: String,
}

/// Ссылки профиля.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LinksView {
    pub discord: String,
    pub youtube: String,
    pub youtube_label: String,
    pub twitch: String,
    pub twitch_label: String,
}

/// Достижение.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AchievementView {
    pub title: String,
    pub text: String,
    pub icon: String,
    pub unlocked: bool,
    /// Класс `secret` (`item.secret && !item.hidden`): открытое секретное, у замка его нет.
    pub secret: bool,
    /// Полоса прогресса у недостигнутого с целью.
    pub meter: bool,
    /// Доля полосы, 0…100 (`Math.round(progress * 100)`).
    pub percent: i32,
    pub state: String,
    /// Редкость полученного (`tier-*`): `gold`, `purple`, `rainbow`; иначе пусто.
    pub tier: &'static str,
    /// Когда получено (`formatAchievedAt`) — только у своего профиля; иначе пусто.
    pub at: String,
}

/// Рейтинг в строке под ником.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RatingView {
    pub can_vote: bool,
    pub score: String,
    /// `positive`, `negative` или пусто.
    pub tone: &'static str,
    pub voting: bool,
    /// Свой голос: 1, −1, 0.
    pub vote: i32,
    pub hint: String,
    pub like_label: String,
    pub like_title: String,
    pub dislike_label: String,
    pub dislike_title: String,
}

/// Блокировка.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BanView {
    pub title: String,
    pub reason: String,
}

/// Всё, что показывает окно профиля.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileView {
    pub kind: Kind,
    /// Узкое окно без вкладок (`is-guest`).
    pub narrow: bool,
    pub failed: bool,
    /// SteamID чужого профиля (заголовок «нет профиля»).
    pub target: String,
    pub guest_view: bool,
    pub login_pending: bool,
    pub login_code: String,
    pub login_error: String,
    pub avatar: String,
    /// `online`, `checking`, `offline`.
    pub dot: &'static str,
    pub presence_text: String,
    pub name: String,
    pub nick_style: String,
    pub nick_css: String,
    pub frame_css: String,
    pub developer: bool,
    pub badges_shown: bool,
    pub badges: Vec<BadgeView>,
    pub status: String,
    pub extras: String,
    /// Текст присутствия, когда игрок в игре; иначе пусто.
    pub playing: String,
    pub since: String,
    pub rating: Option<RatingView>,
    pub dubious: bool,
    /// Меню «⋯» (`moreItems`): пусто — кнопки нет.
    pub more: Vec<MoreItem>,
    /// Подписка активна (`subscriber`) и окно с профилем: голубая бегущая рамка.
    pub subscriber: bool,
    /// Все достижения, включая секретные (`complete`) и окно с профилем: золотая рамка.
    pub complete: bool,
    /// `supporterSince` (unix-секунды) для подсказки значка у ника.
    pub supporter_since: Option<i64>,
    pub tabs: Vec<TabView>,
    pub ban: Option<BanView>,
    pub stats: Vec<StatView>,
    pub run: Option<RunView>,
    pub record_empty_own: bool,
    pub links: Option<LinksView>,
    pub counter: String,
    pub percent: i32,
    pub achievements: Vec<AchievementView>,
}

/// Состояние окна (`data()` `ProfileModal.vue`).
#[derive(Debug, Clone)]
pub struct ProfileState {
    target: String,
    hide_steam_id: bool,
    account: Value,
    consent: bool,
    details: Value,
    badges: Vec<Value>,
    nick: Option<Value>,
    subscriber: bool,
    booster_since: Option<f64>,
    supporter_since: Option<f64>,
    custom_badges: Vec<Value>,
    avatar_frame: Option<Value>,
    ban: Option<Value>,
    mute: Option<Value>,
    perks: Value,
    style_preset: Option<Value>,
    telegram: Value,
    discord_link: Value,
    deletion_at: Option<String>,
    missing: bool,
    view_loading: bool,
    view_failed: bool,
    guest_view: bool,
    login_status: String,
    login_code: Option<String>,
    login_error: Option<String>,
    login_success: bool,
    achievements: Vec<Value>,
    summary: Value,
    stats: Value,
    /// Серия входов `{days, best}` — только у своего профиля.
    streak: Option<Value>,
    first_run_at: Option<String>,
    presence: Value,
    rating: Value,
    vote: Value,
    voting: bool,
    vote_note: VoteNote,
    place_refreshed: bool,
}

/// Подсказка под рейтингом (`voteNote`): текст строится на языке интерфейса при показе.
#[derive(Debug, Clone, PartialEq, Eq)]
enum VoteNote {
    None,
    Failed,
    /// «Голос можно поменять после …» (`vote.nextAt`).
    Next(String),
    /// «Лайкнуть можно не больше 5 игроков…» (`likesAt`).
    LikeLimit(String),
}

fn default_presence() -> Value {
    json!({"online": false, "gameId": null, "launcherId": null})
}

fn default_rating() -> Value {
    json!({"score": 0, "likes": 0, "dislikes": 0, "dubious": false})
}

/// `x || fallback` для значения ответа.
fn or_value(value: Option<&Value>, fallback: Value) -> Value {
    match value {
        Some(value) if truthy(Some(value)) => value.clone(),
        _ => fallback,
    }
}

fn list_of(value: Option<&Value>) -> Vec<Value> {
    value
        .filter(|value| truthy(Some(*value)))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn text_of(value: Option<&Value>) -> String {
    str_of(value).unwrap_or_default().to_owned()
}

fn default_perks() -> Value {
    json!({"styles": false, "subscriber": null})
}

fn default_telegram() -> Value {
    json!({"linked": false, "username": null})
}

fn default_discord_link() -> Value {
    json!({"linked": false, "username": null, "boosterSince": null})
}

/// `x || null` для числа ответа (unix-секунды).
fn seconds_of(value: Option<&Value>) -> Option<f64> {
    value
        .filter(|value| truthy(Some(*value)))
        .and_then(Value::as_f64)
}

/// Форма «один / несколько / много» по правилам языка (`Intl.PluralRules`), как в 0.4.9.
fn plural_key(
    lang: Language,
    count: f64,
    one: &'static str,
    few: &'static str,
    many: &'static str,
) -> &'static str {
    match intl::plural(lang, count) {
        Plural::One => one,
        Plural::Few => few,
        _ => many,
    }
}

impl ProfileState {
    /// `data()` окна: у чужого профиля — «грузится» (`viewLoading: !!targetSteamId`).
    pub fn new(target: &str, hide_steam_id: bool) -> ProfileState {
        ProfileState {
            target: target.to_owned(),
            hide_steam_id,
            account: json!({}),
            consent: false,
            details: json!({}),
            badges: Vec::new(),
            nick: None,
            subscriber: false,
            booster_since: None,
            supporter_since: None,
            custom_badges: Vec::new(),
            avatar_frame: None,
            ban: None,
            mute: None,
            perks: default_perks(),
            style_preset: None,
            telegram: default_telegram(),
            discord_link: default_discord_link(),
            deletion_at: None,
            missing: false,
            view_loading: !target.is_empty(),
            view_failed: false,
            guest_view: false,
            login_status: "signed-out".to_owned(),
            login_code: None,
            login_error: None,
            login_success: false,
            achievements: Vec::new(),
            summary: json!({"unlocked": 0, "total": 0, "percent": 0}),
            stats: json!({"totalSeconds": 0, "gamesPlayed": 0, "launches": 0, "best": null}),
            streak: None,
            first_run_at: None,
            presence: default_presence(),
            rating: default_rating(),
            vote: json!({"value": 0, "nextAt": null}),
            voting: false,
            vote_note: VoteNote::None,
            place_refreshed: false,
        }
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    /// Свой профиль (нет `targetSteamId`).
    pub fn is_own(&self) -> bool {
        self.target.is_empty()
    }

    /// Свойство `hideSteamId` оболочки.
    pub fn set_hide_steam_id(&mut self, hide: bool) {
        self.hide_steam_id = hide;
    }

    pub fn set_consent(&mut self, consent: bool) {
        self.consent = consent;
    }

    pub fn consent(&self) -> bool {
        self.consent
    }

    /// Аккаунт ответа (`account`): для настроек профиля.
    pub fn account(&self) -> &Value {
        &self.account
    }

    pub fn details(&self) -> &Value {
        &self.details
    }

    pub fn deletion_at(&self) -> Option<&str> {
        self.deletion_at.as_deref()
    }

    pub fn mute(&self) -> Option<&Value> {
        self.mute.as_ref()
    }

    /// Свойство `hideSteamId` оболочки (для вкладки «Настройки»).
    pub fn hide_steam_id(&self) -> bool {
        self.hide_steam_id
    }

    /// Подписка активна сейчас (`subscriber`).
    pub fn subscriber(&self) -> bool {
        self.subscriber
    }

    /// Права поддержавшего `{styles, subscriber}` (свой профиль).
    pub fn perks(&self) -> &Value {
        &self.perks
    }

    /// Выбранная заготовка подсветки (свой профиль).
    pub fn style_preset(&self) -> Option<&Value> {
        self.style_preset.as_ref()
    }

    /// Привязка Telegram `{linked, username}` (свой профиль).
    pub fn telegram(&self) -> &Value {
        &self.telegram
    }

    /// Привязка Discord `{linked, username, boosterSince}` (свой профиль).
    pub fn discord_link(&self) -> &Value {
        &self.discord_link
    }

    /// Достижения ответа как есть (вкладка «Достижения»).
    pub fn achievements(&self) -> &[Value] {
        &self.achievements
    }

    /// `summary` `{unlocked, total, percent}`.
    pub fn summary(&self) -> &Value {
        &self.summary
    }

    /// `complete`: все достижения, включая секретные.
    pub fn complete(&self) -> bool {
        let total = or_zero(prop(&self.summary, "total"));
        total > 0.0 && or_zero(prop(&self.summary, "unlocked")) == total
    }

    /// Окно без профиля (`guestWindow`): вход, загрузка, ошибка или профиля нет.
    pub fn guest_window(&self) -> bool {
        self.narrow()
    }

    fn signed_in(&self) -> bool {
        truthy(prop(&self.account, "signedIn"))
    }

    // ---- Загрузка и ответы -----------------------------------------------------------------

    /// `load()`: у чужого профиля сначала `runner-state` (вошёл ли сам), у своего — `profile-get`.
    pub fn load(&mut self) -> Vec<Effect> {
        if !self.target.is_empty() {
            self.view_loading = true;
            self.view_failed = false;
            return vec![Effect::Invoke {
                tag: Tag::Own,
                channel: "runner-state",
                args: Vec::new(),
            }];
        }
        vec![Effect::Invoke {
            tag: Tag::Snapshot,
            channel: "profile-get",
            // `invoke("profile-get", undefined)`: в журнале аргумент — `null`.
            args: vec![Value::Null],
        }]
    }

    /// Ответ на вызов `tag`; `Err` — отказ (`reject`).
    pub fn answer(&mut self, tag: Tag, reply: Result<Value, String>) -> Vec<Effect> {
        match tag {
            Tag::Own => self.own_reply(reply),
            Tag::Snapshot => self.snapshot_reply(reply),
            Tag::Login => match reply {
                Ok(result) => self.apply_snapshot(Some(&result)),
                // `await invoke(...)` без `try`: отказ обрывает действие.
                Err(_) => Vec::new(),
            },
            Tag::LoginCancel => match reply {
                Ok(result) => self.apply_snapshot(Some(&result)),
                Err(_) => Vec::new(),
            },
            Tag::Rate => self.rate_reply(reply),
            Tag::Ignored => Vec::new(),
        }
    }

    fn own_reply(&mut self, reply: Result<Value, String>) -> Vec<Effect> {
        match reply {
            Ok(own) => {
                self.guest_view = !truthy(field(prop(&own, "account"), "signedIn"));
                if self.guest_view {
                    let effects = self.apply_snapshot(Some(&own));
                    self.view_loading = false;
                    return effects;
                }
                vec![Effect::Invoke {
                    tag: Tag::Snapshot,
                    channel: "runner-profile-view",
                    args: vec![json!(self.target)],
                }]
            }
            Err(_) => {
                self.view_failed = true;
                self.view_loading = false;
                Vec::new()
            }
        }
    }

    fn snapshot_reply(&mut self, reply: Result<Value, String>) -> Vec<Effect> {
        let mut effects = Vec::new();
        match reply {
            Ok(data) => {
                effects.extend(self.apply_snapshot(Some(&data)));
                if self.target.is_empty() {
                    effects.push(Effect::Invoke {
                        tag: Tag::Ignored,
                        channel: "runner-sync",
                        args: Vec::new(),
                    });
                }
            }
            Err(_) => {
                if self.target.is_empty() {
                    self.login_error = Some("RUNNER_NETWORK".to_owned());
                } else {
                    self.view_failed = true;
                }
            }
        }
        self.view_loading = false;
        effects
    }

    /// `applySnapshot(data)`: снимок профиля (ответ или событие `runner-profile-state`).
    pub fn apply_snapshot(&mut self, data: Option<&Value>) -> Vec<Effect> {
        let data = data.filter(|data| truthy(Some(*data)));
        let failed = match data {
            None => true,
            Some(data) => str_of(prop(data, "status")) == Some("error"),
        };
        if failed {
            if !self.target.is_empty() && !self.guest_view {
                self.view_failed = true;
            } else {
                self.login_error = Some("RUNNER_NETWORK".to_owned());
            }
            return Vec::new();
        }
        let data = data.expect("снимок есть");
        let mut effects = Vec::new();
        self.missing = prop(data, "missing") == Some(&Value::Bool(true));
        self.account = or_value(prop(data, "account"), json!({}));
        self.achievements = list_of(prop(data, "achievements"));
        if let Some(summary) = prop(data, "summary").filter(|value| truthy(Some(*value))) {
            self.summary = summary.clone();
        }
        if let Some(stats) = prop(data, "stats").filter(|value| truthy(Some(*value))) {
            self.stats = stats.clone();
        }
        self.streak = prop(data, "streak")
            .filter(|value| self.target.is_empty() && truthy(Some(*value)))
            .cloned();
        self.first_run_at = prop(data, "firstRunAt")
            .filter(|value| truthy(Some(*value)))
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.details = or_value(prop(data, "details"), json!({}));
        self.presence = or_value(prop(data, "presence"), default_presence());
        self.rating = or_value(prop(data, "rating"), default_rating());
        self.vote = or_value(prop(data, "vote"), json!({"value": 0, "nextAt": null}));
        // Своё значение «Скрыть SteamID» на сервере: оболочка сверяет его с настройкой.
        if self.target.is_empty() && self.signed_in() {
            effects.push(Effect::ServerHideId(
                prop(&self.details, "hideSteamId").and_then(Value::as_bool),
            ));
        }
        self.badges = list_of(prop(data, "badges"));
        self.nick = prop(data, "nick")
            .filter(|value| truthy(Some(*value)))
            .cloned();
        self.subscriber = prop(data, "subscriber") == Some(&Value::Bool(true));
        self.booster_since = seconds_of(prop(data, "boosterSince"));
        self.supporter_since = seconds_of(prop(data, "supporterSince"));
        self.custom_badges = list_of(prop(data, "customBadges"));
        self.avatar_frame = prop(data, "avatarFrame")
            .filter(|value| truthy(Some(*value)))
            .cloned();
        self.deletion_at = prop(data, "deletionAt")
            .filter(|value| truthy(Some(*value)))
            .and_then(Value::as_str)
            .map(str::to_owned);
        if self.target.is_empty() {
            self.ban = prop(data, "ban")
                .filter(|value| truthy(Some(*value)))
                .cloned();
            self.mute = prop(data, "mute")
                .filter(|value| truthy(Some(*value)))
                .cloned();
            self.perks = or_value(prop(data, "perks"), default_perks());
            self.style_preset = prop(data, "stylePreset")
                .filter(|value| truthy(Some(*value)))
                .cloned();
            self.telegram = or_value(prop(data, "telegram"), default_telegram());
            self.discord_link = or_value(prop(data, "discordLink"), default_discord_link());
        }
        self.login_status = str_of(prop(data, "status"))
            .filter(|status| !status.is_empty())
            .unwrap_or("signed-out")
            .to_owned();
        self.login_code = prop(data, "userCode")
            .filter(|value| truthy(Some(*value)))
            .map(js::to_string);
        self.login_error = prop(data, "error")
            .filter(|value| truthy(Some(*value)))
            .map(js::to_string);
        effects.extend(self.refresh_pinned_place());
        effects
    }

    /// Закреплённые до появления места забеги его не знают: список рекордов заодно обновляет
    /// место и ссылку закреплённого.
    fn refresh_pinned_place(&mut self) -> Vec<Effect> {
        let run = prop(&self.details, "pinnedRun").filter(|run| truthy(Some(*run)));
        let Some(run) = run else { return Vec::new() };
        if !self.target.is_empty()
            || self.place_refreshed
            || truthy(prop(run, "place"))
            || !truthy(prop(&self.details, "speedrun"))
        {
            return Vec::new();
        }
        self.place_refreshed = true;
        vec![Effect::Invoke {
            tag: Tag::Ignored,
            channel: "runner-speedrun-bests",
            args: Vec::new(),
        }]
    }

    /// Событие `runner-profile-state`: чужой профиль (не гостевой вид) его не слушает; гость,
    /// вошедший из окна чужого профиля, загружает его заново.
    pub fn profile_pushed(&mut self, value: &Value) -> Vec<Effect> {
        if !self.target.is_empty() && !self.guest_view {
            return Vec::new();
        }
        let mut effects = self.apply_snapshot(Some(value));
        if self.guest_view && truthy(field(prop(value, "account"), "signedIn")) {
            self.guest_view = false;
            effects.extend(self.load());
        }
        effects
    }

    // ---- Вход ------------------------------------------------------------------------------

    /// `signIn()`: «Войти через Steam» (согласие принято).
    pub fn sign_in(&mut self) -> Vec<Effect> {
        if !self.consent {
            return Vec::new();
        }
        vec![Effect::Invoke {
            tag: Tag::Login,
            channel: "runner-login",
            args: vec![Value::Bool(true)],
        }]
    }

    /// `cancelLogin()`.
    pub fn cancel_login(&self) -> Vec<Effect> {
        vec![Effect::Invoke {
            tag: Tag::LoginCancel,
            channel: "runner-login-cancel",
            args: Vec::new(),
        }]
    }

    /// `onLoginSuccess()`.
    pub fn login_success(&mut self) -> Vec<Effect> {
        self.login_success = true;
        vec![Effect::LoginSuccessTimer(true)]
    }

    /// Таймер подтверждения истёк.
    pub fn login_success_done(&mut self) {
        self.login_success = false;
    }

    pub fn login_success_shown(&self) -> bool {
        self.login_success
    }

    /// `openPrivacy()` / `openTerms()`: `open-external-link` с ключом страницы.
    pub fn open_external(&self, key: &str) -> Vec<Effect> {
        vec![Effect::Invoke {
            tag: Tag::Ignored,
            channel: "open-external-link",
            args: vec![json!(key)],
        }]
    }

    /// `openLink(url)`.
    pub fn open_link(&self, url: &str) -> Vec<Effect> {
        vec![Effect::Invoke {
            tag: Tag::Ignored,
            channel: "runner-open-link",
            args: vec![json!(url)],
        }]
    }

    /// `openSteam()`: профиль Steam вошедшего.
    pub fn open_steam(&self) -> Vec<Effect> {
        match str_of(prop(&self.account, "steamId64")).filter(|id| !id.is_empty()) {
            Some(id) => self.open_link(&format!("https://steamcommunity.com/profiles/{id}")),
            None => Vec::new(),
        }
    }

    /// `copy(value)`: в буфер обмена и «Скопировано».
    pub fn copy(&self, value: &str) -> Vec<Effect> {
        if value.is_empty() {
            return Vec::new();
        }
        vec![
            Effect::Copy(value.to_owned()),
            Effect::Notify("#L_SB_COPIED"),
        ]
    }

    /// «Скопировать SteamID» (`copy(account.steamId2)`).
    pub fn copy_id(&self) -> Vec<Effect> {
        self.copy(str_of(prop(&self.account, "steamId2")).unwrap_or_default())
    }

    /// `moreItems`: SteamID скрыт — у чужого, если так решил владелец (меню пустое), у своего —
    /// по настройке на этом компьютере (нет только ссылки на Steam).
    pub fn more_items(&self, tr: &Tr) -> Vec<MoreItem> {
        let hidden = if self.target.is_empty() {
            self.hide_steam_id
        } else {
            prop(&self.details, "hideSteamId") == Some(&Value::Bool(true))
        };
        let steam_id2 = str_of(prop(&self.account, "steamId2")).unwrap_or_default();
        if steam_id2.is_empty() || (!self.target.is_empty() && hidden) {
            return Vec::new();
        }
        let mut items = vec![MoreItem {
            id: "copy",
            icon: "fas fa-copy",
            label: tr("#L_PROFILE_COPY_ID", &[]),
        }];
        if !hidden && str_of(prop(&self.account, "steamId64")).is_some_and(|id| !id.is_empty()) {
            items.push(MoreItem {
                id: "steam",
                icon: "fab fa-steam",
                label: tr("#L_PROFILE_MENU_STEAM", &[]),
            });
        }
        items
    }

    /// `chooseMore(item)`: пункт меню «⋯».
    pub fn choose_more(&self, id: &str) -> Vec<Effect> {
        match id {
            "copy" => self.copy_id(),
            "steam" => self.open_steam(),
            _ => Vec::new(),
        }
    }

    // ---- Голос -----------------------------------------------------------------------------

    /// `canVote`.
    pub fn can_vote(&self) -> bool {
        !self.target.is_empty() && !self.guest_view && !self.missing && self.signed_in()
    }

    fn vote_value(&self) -> i64 {
        or_zero(prop(&self.vote, "value")) as i64
    }

    /// `castVote(value)`: лайк (1), дизлайк (−1); повторное нажатие на свой голос снимает его.
    pub fn cast_vote(&mut self, value: i64) -> Vec<Effect> {
        if !self.can_vote() || self.voting {
            return Vec::new();
        }
        let next = if self.vote_value() == value { 0 } else { value };
        self.voting = true;
        self.vote_note = VoteNote::None;
        vec![
            Effect::VoteNoteTimer(false),
            Effect::Invoke {
                tag: Tag::Rate,
                channel: "runner-rate",
                args: vec![json!(self.target), json!(next)],
            },
        ]
    }

    fn rate_reply(&mut self, reply: Result<Value, String>) -> Vec<Effect> {
        let failed = match &reply {
            Ok(result) => !truthy(Some(result)) || str_of(prop(result, "status")) == Some("error"),
            Err(_) => true,
        };
        let mut note = None;
        match &reply {
            Ok(result) if !failed => {
                self.rating = prop(result, "rating").cloned().unwrap_or(Value::Null);
                self.vote = prop(result, "vote").cloned().unwrap_or(Value::Null);
                match str_of(prop(result, "status")) {
                    Some("cooldown") => {
                        note = Some(VoteNote::Next(text_of(field(
                            prop(result, "vote"),
                            "nextAt",
                        ))));
                    }
                    Some("like-limit") => {
                        note = Some(VoteNote::LikeLimit(text_of(prop(result, "likesAt"))));
                    }
                    _ => {}
                }
            }
            _ => note = Some(VoteNote::Failed),
        }
        // finally { voting = false } — после показа подсказки.
        self.voting = false;
        match note {
            Some(note) => {
                self.vote_note = note;
                vec![Effect::VoteNoteTimer(true)]
            }
            None => Vec::new(),
        }
    }

    /// Таймер подсказки истёк.
    pub fn vote_note_done(&mut self) {
        self.vote_note = VoteNote::None;
    }

    pub fn vote_note_shown(&self) -> bool {
        self.vote_note != VoteNote::None
    }

    // ---- Показ -----------------------------------------------------------------------------

    fn kind(&self) -> Kind {
        if !self.target.is_empty() && !self.guest_view && (self.view_loading || self.view_failed) {
            Kind::Pending
        } else if self.missing {
            Kind::Missing
        } else if !self.signed_in() && (self.target.is_empty() || self.guest_view) {
            Kind::SignIn
        } else {
            Kind::Main
        }
    }

    /// `profile__window.is-guest`.
    fn narrow(&self) -> bool {
        !self.signed_in() || self.missing || self.view_loading || self.view_failed
    }

    /// Подсказка под рейтингом на языке интерфейса (пусто — нет).
    pub fn vote_note_text(&self, lang: Language, wall: &WallClock, tr: &Tr) -> String {
        match &self.vote_note {
            VoteNote::None => String::new(),
            VoteNote::Failed => tr("#L_RATING_FAILED", &[]),
            VoteNote::Next(at) => tr(
                "#L_RATING_NEXT",
                &[format_vote_time(lang, wall, at).as_str()],
            ),
            VoteNote::LikeLimit(at) => tr(
                "#L_RATING_LIKE_LIMIT",
                &[format_vote_time(lang, wall, at).as_str()],
            ),
        }
    }

    /// Готовое к показу.
    pub fn view(&self, lang: Language, wall: &WallClock, tr: &Tr) -> ProfileView {
        let t = |key: &str| tr(key, &[]);
        let kind = self.kind();
        let locale = t("#L_RUNNER_LOCALE");
        let signed_in = self.signed_in();
        let sections_shown = signed_in && !self.missing;
        let connection = str_of(prop(&self.account, "connection")).unwrap_or_default();
        let checking = self.target.is_empty() && connection == "checking";
        let dot = if checking {
            "checking"
        } else if truthy(prop(&self.presence, "online")) {
            "online"
        } else {
            "offline"
        };
        let playing_game = str_of(prop(&self.presence, "launcherId"))
            .and_then(short_name)
            .unwrap_or_default();
        let presence_text = if dot == "checking" {
            t("#L_ST_CHECKING")
        } else if !playing_game.is_empty() {
            tr("#L_PRESENCE_PLAYING", &[playing_game])
        } else {
            t(if truthy(prop(&self.presence, "online")) {
                "#L_PRESENCE_ONLINE"
            } else {
                "#L_PRESENCE_OFFLINE"
            })
        };
        let display_name = match str_of(prop(&self.account, "personaName")) {
            Some(name) if !name.is_empty() => name.to_owned(),
            _ => t("#L_SB_STEAM_ACCOUNT"),
        };
        let status = {
            let text = str_of(prop(&self.details, "status")).unwrap_or_default();
            if link_guard::contains_link(&Value::String(text.to_owned())) {
                String::new()
            } else {
                text.to_owned()
            }
        };
        let pronoun = pronouns::pronoun_label(
            Some(pronouns::normalize_pronouns(str_of(prop(
                &self.details,
                "pronouns",
            )))),
            &locale,
        );
        let country = str_of(prop(&self.details, "country")).unwrap_or_default();
        let extras = [pronoun.to_owned(), intl::country_name(lang, country)]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let rating_shown = signed_in && !self.missing && !self.guest_view;
        let rating = rating_shown.then(|| self.rating_view(lang, wall, tr));
        let summary_unlocked = or_zero(prop(&self.summary, "unlocked"));
        let summary_total = or_zero(prop(&self.summary, "total"));
        let mut tabs = Vec::new();
        if sections_shown {
            tabs.push(TabView {
                label: t("#L_PROFILE_NAV_PROFILE"),
                icon: "fas fa-user",
                count: String::new(),
            });
            tabs.push(TabView {
                label: t("#L_PROFILE_ACHIEVEMENTS"),
                icon: "fas fa-trophy",
                count: format!(
                    "{}/{}",
                    js::number::number_to_string(summary_unlocked),
                    js::number::number_to_string(summary_total)
                ),
            });
            if self.target.is_empty() {
                tabs.push(TabView {
                    label: t("#L_PROFILE_NAV_SETTINGS"),
                    icon: "fas fa-sliders-h",
                    count: String::new(),
                });
            }
        }
        let ban = self
            .ban
            .as_ref()
            .filter(|_| self.target.is_empty())
            .map(|ban| {
                let until = str_of(prop(ban, "expiresAt")).filter(|at| !at.is_empty());
                let title = match until {
                    Some(at) => tr(
                        "#L_MOD_BANNED_UNTIL",
                        &[format_moderation_date(lang, wall, at).as_str()],
                    ),
                    None => t("#L_MOD_BANNED_FOREVER"),
                };
                let reason = str_of(prop(ban, "reason")).unwrap_or_default();
                BanView {
                    title,
                    reason: if reason.is_empty() {
                        String::new()
                    } else {
                        tr("#L_MOD_REASON", &[reason])
                    },
                }
            });
        let run = prop(&self.details, "pinnedRun")
            .filter(|run| truthy(Some(*run)))
            .map(|run| self.run_view(run, &locale, tr));
        let links = {
            let discord = text_of(prop(&self.details, "discord"));
            let youtube = text_of(prop(&self.details, "youtube"));
            let twitch = text_of(prop(&self.details, "twitch"));
            (!discord.is_empty() || !youtube.is_empty() || !twitch.is_empty()).then(|| LinksView {
                discord,
                youtube_label: if youtube.is_empty() {
                    String::new()
                } else {
                    self.channel_label("youtube")
                },
                youtube,
                twitch_label: if twitch.is_empty() {
                    String::new()
                } else {
                    self.channel_label("twitch")
                },
                twitch,
            })
        };
        let login_error = self
            .login_error
            .as_deref()
            .map(|code| {
                let key = LOGIN_ERRORS
                    .iter()
                    .find(|(known, _)| *known == code)
                    .map_or("#L_RUNNER_NETWORK", |(_, key)| *key);
                t(key)
            })
            .unwrap_or_default();
        let developer = self
            .badges
            .iter()
            .any(|badge| badge.as_str() == Some("developer"));
        ProfileView {
            kind,
            narrow: self.narrow(),
            failed: self.view_failed,
            target: self.target.clone(),
            guest_view: self.guest_view,
            login_pending: self.login_status == "pending",
            login_code: self.login_code.clone().unwrap_or_default(),
            login_error,
            avatar: text_of(prop(&self.account, "avatarDataUri")),
            dot,
            presence_text: presence_text.clone(),
            name: display_name,
            nick_style: text_of(field(self.nick.as_ref(), "style")),
            nick_css: text_of(field(self.nick.as_ref(), "css")),
            frame_css: text_of(field(self.avatar_frame.as_ref(), "css")),
            developer,
            badges_shown: !self.badges.is_empty(),
            badges: self.badge_list(&locale, lang, wall, tr),
            status,
            extras,
            playing: if playing_game.is_empty() {
                String::new()
            } else {
                presence_text
            },
            since: match self.first_run_at.as_deref() {
                None => t("#L_PROFILE_SINCE_UNKNOWN"),
                Some(iso) => tr("#L_PROFILE_SINCE", &[format_date(wall, iso).as_str()]),
            },
            rating,
            dubious: truthy(prop(&self.rating, "dubious")) && !self.missing,
            more: self.more_items(tr),
            subscriber: self.subscriber && !self.narrow(),
            complete: self.complete() && !self.narrow(),
            supporter_since: self.supporter_since.map(|since| since as i64),
            tabs,
            ban,
            stats: self.stat_rows(lang, tr),
            run,
            record_empty_own: self.target.is_empty(),
            links,
            counter: tr(
                "#L_PROFILE_COUNTER",
                &[format!(
                    "{}/{}",
                    js::number::number_to_string(summary_unlocked),
                    js::number::number_to_string(summary_total)
                )
                .as_str()],
            ),
            percent: or_zero(prop(&self.summary, "percent")).clamp(0.0, 100.0) as i32,
            achievements: self
                .achievements
                .iter()
                .map(|item| achievement_view(item, self.target.is_empty(), lang, wall, tr))
                .collect(),
        }
    }

    fn rating_view(&self, lang: Language, wall: &WallClock, tr: &Tr) -> RatingView {
        let can_vote = self.can_vote();
        let score = or_zero(prop(&self.rating, "score"));
        let likes = js::number::number_to_string(or_zero(prop(&self.rating, "likes")));
        let dislikes = js::number::number_to_string(or_zero(prop(&self.rating, "dislikes")));
        let vote = self.vote_value();
        let next_at = text_of(prop(&self.vote, "nextAt"));
        let title = |value: i64| {
            if !next_at.is_empty() && vote != 0 {
                return tr(
                    "#L_RATING_NEXT",
                    &[format_vote_time(lang, wall, &next_at).as_str()],
                );
            }
            tr(
                match (value, vote) {
                    (1, 1) => "#L_RATING_UNLIKE",
                    (1, _) => "#L_RATING_LIKE",
                    (_, -1) => "#L_RATING_UNDISLIKE",
                    _ => "#L_RATING_DISLIKE",
                },
                &[],
            )
        };
        RatingView {
            can_vote,
            score: if score > 0.0 {
                format!("+{}", js::number::number_to_string(score))
            } else {
                js::number::number_to_string(score)
            },
            tone: if score > 0.0 {
                "positive"
            } else if score < 0.0 {
                "negative"
            } else {
                ""
            },
            voting: self.voting,
            vote: vote as i32,
            hint: tr("#L_RATING_HINT", &[likes.as_str(), dislikes.as_str()]),
            like_label: tr(
                if vote == 1 {
                    "#L_RATING_UNLIKE"
                } else {
                    "#L_RATING_LIKE"
                },
                &[],
            ),
            like_title: title(1),
            dislike_label: tr(
                if vote == -1 {
                    "#L_RATING_UNDISLIKE"
                } else {
                    "#L_RATING_DISLIKE"
                },
                &[],
            ),
            dislike_title: title(-1),
        }
    }

    /// Значки: известные встроенные (картинки разработчика, баг-хантера, бустера и годовщин,
    /// глиф «подтверждён») и свои (подпись и пояснение на языке интерфейса).
    fn badge_list(
        &self,
        locale: &str,
        language: Language,
        wall: &WallClock,
        tr: &Tr,
    ) -> Vec<BadgeView> {
        let lang = if locale == "ru" { "ru" } else { "en" };
        let image = |id: &str, asset: &'static str, label: String, glow: [u8; 3]| BadgeView {
            id: id.to_owned(),
            label,
            asset,
            glow: Some(glow),
            ..BadgeView::default()
        };
        self.badges
            .iter()
            .filter_map(|id| {
                let id = id.as_str()?;
                match id {
                    // Свои значки из админки перекрывают встроенные с тем же id.
                    _ if self
                        .custom_badges
                        .iter()
                        .any(|badge| str_of(prop(badge, "id")) == Some(id)) =>
                    {
                        let badge = self
                            .custom_badges
                            .iter()
                            .find(|badge| str_of(prop(badge, "id")) == Some(id))?;
                        let note = {
                            let own = text_of(field(prop(badge, "note"), lang));
                            if own.is_empty() {
                                text_of(field(prop(badge, "note"), "ru"))
                            } else {
                                own
                            }
                        };
                        let label = text_of(field(prop(badge, "label"), lang));
                        Some(BadgeView {
                            id: id.to_owned(),
                            label: if note.is_empty() {
                                label
                            } else {
                                format!("{label} — {note}")
                            },
                            image: text_of(prop(badge, "icon")),
                            themed: prop(badge, "themed") == Some(&Value::Bool(true)),
                            ..BadgeView::default()
                        })
                    }
                    "developer" => Some(image(
                        id,
                        "developer",
                        tr("#L_RUNNER_DEVELOPER", &[]),
                        [40, 140, 255],
                    )),
                    "bughunter" => Some(image(
                        id,
                        "bughunter",
                        tr("#L_RUNNER_BUGHUNTER", &[]),
                        [250, 204, 21],
                    )),
                    "booster" => {
                        let since = self
                            .booster_since
                            .map(|since| format_since(wall, since))
                            .unwrap_or_default();
                        Some(image(
                            id,
                            "booster",
                            tr("#L_RUNNER_BOOSTER", &[since.as_str()]),
                            [255, 79, 216],
                        ))
                    }
                    "verified" => Some(BadgeView {
                        id: id.to_owned(),
                        label: tr("#L_RUNNER_VERIFIED", &[]),
                        icon: "fas fa-check-circle".to_owned(),
                        ..BadgeView::default()
                    }),
                    _ => {
                        let index = ANNIVERSARY_IDS.iter().position(|known| *known == id)?;
                        let count = (index + 1) as f64;
                        let key = plural_key(
                            language,
                            count,
                            "#L_RUNNER_ANNIVERSARY_ONE",
                            "#L_RUNNER_ANNIVERSARY_FEW",
                            "#L_RUNNER_ANNIVERSARY_MANY",
                        );
                        Some(BadgeView {
                            id: id.to_owned(),
                            label: tr(key, &[(index + 1).to_string().as_str()]),
                            asset: ANNIVERSARY_IDS[index],
                            glow: ANNIVERSARY_GLOW[index],
                            anniversary: true,
                            ..BadgeView::default()
                        })
                    }
                }
            })
            .collect()
    }

    fn channel_label(&self, name: &str) -> String {
        let value = prop(&self.details, name).cloned().unwrap_or(json!(""));
        profile_links::channel_name(name, &value)
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| {
                (if name == "youtube" {
                    "YouTube"
                } else {
                    "Twitch"
                })
                .to_owned()
            })
    }

    fn run_view(&self, run: &Value, locale: &str, tr: &Tr) -> RunView {
        let place = num(prop(run, "place"))
            .filter(|place| *place > 0.0)
            .map_or(0, |p| p as i64);
        RunView {
            game: text_of(prop(run, "game")),
            category: text_of(prop(run, "category")),
            time: format_run(or_zero(prop(run, "seconds"))),
            url: text_of(prop(run, "url")),
            place,
            medal: medal_of(place),
            place_label: if place > 0 {
                place_label(place, locale, tr)
            } else {
                String::new()
            },
        }
    }

    fn stat_rows(&self, lang: Language, tr: &Tr) -> Vec<StatView> {
        let t = |key: &str| tr(key, &[]);
        let total = or_zero(prop(&self.stats, "totalSeconds"));
        let number = |value: Option<&Value>| js::number::number_to_string(or_zero(value));
        let mut rows = vec![
            StatView {
                icon: "fas fa-clock",
                label: t("#L_PROFILE_STAT_TOTAL_TIME"),
                value: format_hours(total, tr),
                wide: false,
            },
            StatView {
                icon: "fas fa-play",
                label: t("#L_PROFILE_STAT_LAUNCHES"),
                value: number(prop(&self.stats, "launches")),
                wide: false,
            },
            StatView {
                icon: "fas fa-gamepad",
                label: t("#L_PROFILE_STAT_GAMES"),
                value: number(prop(&self.stats, "gamesPlayed")),
                wide: false,
            },
            StatView {
                icon: "fas fa-trophy",
                label: t("#L_PROFILE_STAT_DONE"),
                value: tr(
                    "#L_PROFILE_STAT_OF",
                    &[
                        number(prop(&self.summary, "unlocked")).as_str(),
                        number(prop(&self.summary, "total")).as_str(),
                    ],
                ),
                wide: false,
            },
        ];
        if let Some(streak) = &self.streak {
            let days = or_zero(prop(streak, "days"));
            let key = plural_key(
                lang,
                days,
                "#L_PROFILE_DAYS_ONE",
                "#L_PROFILE_DAYS_FEW",
                "#L_PROFILE_DAYS_MANY",
            );
            rows.push(StatView {
                icon: "fas fa-calendar-check",
                label: tr(
                    "#L_PROFILE_STAT_STREAK",
                    &[js::to_string(prop(streak, "best").unwrap_or(&Value::Null)).as_str()],
                ),
                value: tr(key, &[js::number::number_to_string(days).as_str()]),
                wide: true,
            });
        }
        if let Some(best) = prop(&self.stats, "best").filter(|best| truthy(Some(*best))) {
            let game_id = text_of(prop(best, "gameId"));
            let name = str_of(prop(best, "launcherId"))
                .and_then(short_name)
                .map_or(game_id, str::to_owned);
            rows.push(StatView {
                icon: "fas fa-fire",
                label: format!(
                    "{} · {}",
                    t("#L_PROFILE_STAT_BEST"),
                    format_hours(or_zero(prop(best, "seconds")), tr)
                ),
                value: name,
                wide: true,
            });
        }
        rows
    }
}

/// `formatSince(seconds)`: unix-секунды → `ДД.ММ.ГГГГ` по местному времени.
pub fn format_since(wall: &WallClock, seconds: f64) -> String {
    match wall.local((seconds * 1000.0) as i64) {
        Some(at) => format!("{:02}.{:02}.{}", at.day, at.month, at.year),
        None => String::new(),
    }
}

/// `medalClass`: первые три места.
pub fn medal_of(place: i64) -> &'static str {
    match place {
        1 => "gold",
        2 => "silver",
        3 => "bronze",
        _ => "",
    }
}

/// `RunPlace.label`: «N место»; по-английски — порядковое число («1st», «12th»).
pub fn place_label(place: i64, locale: &str, tr: &Tr) -> String {
    if locale != "en" {
        return tr("#L_RUN_PLACE", &[place.to_string().as_str()]);
    }
    let tens = place % 100;
    let suffix = if (11..=13).contains(&tens) {
        "th"
    } else {
        ["th", "st", "nd", "rd"]
            .get((place % 10) as usize)
            .copied()
            .unwrap_or("th")
    };
    tr("#L_RUN_PLACE", &[format!("{place}{suffix}").as_str()])
}

/// `formatHours(seconds)`.
pub fn format_hours(seconds: f64, tr: &Tr) -> String {
    let value = if seconds.is_nan() { 0.0 } else { seconds };
    let hours = (value / 3600.0).floor();
    let minutes = ((value % 3600.0) / 60.0).floor();
    if hours <= 0.0 {
        return tr(
            "#L_PROFILE_MINUTES",
            &[js::number::number_to_string(minutes).as_str()],
        );
    }
    tr(
        "#L_PROFILE_HOURS",
        &[
            js::number::number_to_string(hours).as_str(),
            js::number::number_to_string(minutes).as_str(),
        ],
    )
}

/// `formatDate(iso)`: `дд.мм.гггг` в местном поясе; не дата — пусто.
pub fn format_date(wall: &WallClock, iso: &str) -> String {
    let Some(ms) = js::date_parse(iso) else {
        return String::new();
    };
    match wall.local(ms) {
        Some(at) => format!("{:02}.{:02}.{}", at.day, at.month, at.year),
        None => String::new(),
    }
}

/// `formatVoteTime(iso)`: `toLocaleTimeString([], {hour, minute: "2-digit"})`; не дата — пусто.
pub fn format_vote_time(lang: Language, wall: &WallClock, iso: &str) -> String {
    match js::date_parse(iso) {
        Some(ms) => wall.format_system(lang, SystemStyle::Time, ms as f64),
        None => String::new(),
    }
}

/// `formatModerationDate(iso)`.
pub fn format_moderation_date(lang: Language, wall: &WallClock, iso: &str) -> String {
    let ms = js::date_parse(iso).map_or(f64::NAN, |ms| ms as f64);
    wall.format(lang, DateStyle::DateTimeLong, ms)
}

/// `formatAchievedAt(iso)`: «8 ноября 2026, 02:41» / «Nov 8, 2026, 2:41 AM» (en-US `dateStyle:
/// medium, timeStyle: short`) по часам игрока. Не дата — пусто (0.4.9 пишет «Invalid Date»).
pub fn format_achieved_at(lang: Language, wall: &WallClock, iso: &str) -> String {
    const RU: [&str; 12] = [
        "января",
        "февраля",
        "марта",
        "апреля",
        "мая",
        "июня",
        "июля",
        "августа",
        "сентября",
        "октября",
        "ноября",
        "декабря",
    ];
    const EN: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let Some(at) = js::date_parse(iso).and_then(|ms| wall.local(ms)) else {
        return String::new();
    };
    let month = (at.month as usize).saturating_sub(1).min(11);
    match lang {
        Language::Ru => format!(
            "{} {} {}, {:02}:{:02}",
            at.day, RU[month], at.year, at.hour, at.minute
        ),
        Language::En => format!(
            "{} {}, {}, {}:{:02} {}",
            EN[month],
            at.day,
            at.year,
            match at.hour % 12 {
                0 => 12,
                hour => hour,
            },
            at.minute,
            if at.hour >= 12 { "PM" } else { "AM" }
        ),
    }
}

fn achievement_view(
    item: &Value,
    own: bool,
    lang: Language,
    wall: &WallClock,
    tr: &Tr,
) -> AchievementView {
    let unlocked = truthy(prop(item, "unlocked"));
    let goal = num(prop(item, "goal")).filter(|goal| *goal != 0.0 && !goal.is_nan());
    let title_key = str_of(prop(item, "title")).unwrap_or_default();
    let description_key = str_of(prop(item, "description")).unwrap_or_default();
    let text = match goal {
        Some(goal) => tr(
            description_key,
            &[js::number::number_to_string(goal).as_str()],
        ),
        None => tr(description_key, &[]),
    };
    let state = if unlocked {
        tr("#L_PROFILE_UNLOCKED", &[])
    } else {
        match goal {
            None => tr("#L_PROFILE_LOCKED", &[]),
            Some(goal) => format!(
                "{}/{}",
                js::number::number_to_string(or_zero(prop(item, "value"))),
                js::number::number_to_string(goal)
            ),
        }
    };
    AchievementView {
        title: tr(title_key, &[]),
        text,
        icon: text_of(prop(item, "icon")),
        unlocked,
        secret: truthy(prop(item, "secret")) && !truthy(prop(item, "hidden")),
        meter: !unlocked && goal.is_some(),
        percent: super::jsv::round(or_zero(prop(item, "progress")) * 100.0) as i32,
        state,
        tier: match str_of(prop(item, "tier")) {
            Some("gold") if unlocked => "gold",
            Some("purple") if unlocked => "purple",
            Some("rainbow") if unlocked => "rainbow",
            _ => "",
        },
        at: match str_of(prop(item, "at")) {
            Some(iso) if own && !iso.is_empty() => format_achieved_at(lang, wall, iso),
            _ => String::new(),
        },
    }
}

#[cfg(test)]
mod tests;
