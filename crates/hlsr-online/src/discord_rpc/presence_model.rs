//! `discord-rpc/presence-model.js`: что показывать в Discord. Состояние лаунчера и игры
//! (любое значение JSON, как объект 0.4.7) → нормализованная модель [`Presence`] → активность
//! [`Activity`] для `SET_ACTIVITY`.

use super::constants::{assets, button_urls, game_text};
use crate::game_presence::map_assets;
use crate::js_value::{field, to_number};
use hlsr_core::js;
use hlsr_core::json_text::js_number;
use serde_json::{Map, Value, json};

/// `KINDS`: что делает пользователь.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Launcher,
    Browsing,
    Ready,
    Installing,
    Updating,
    Repairing,
    Playing,
    GameMenu,
    GameMap,
    // Где игрок в лаунчере (раздел шапки или вкладка) и рендер демки — без игры.
    Home,
    Personalizing,
    Demos,
    DemoRender,
    Livesplit,
}

impl Kind {
    /// `KIND_LIST`: в порядке объекта `KINDS`.
    pub const ALL: [Kind; 14] = [
        Kind::Launcher,
        Kind::Browsing,
        Kind::Ready,
        Kind::Installing,
        Kind::Updating,
        Kind::Repairing,
        Kind::Playing,
        Kind::GameMenu,
        Kind::GameMap,
        Kind::Home,
        Kind::Personalizing,
        Kind::Demos,
        Kind::DemoRender,
        Kind::Livesplit,
    ];

    /// `LAUNCHER_KINDS`: состояния без игры — в Discord «HLSR Launcher» и само состояние.
    pub const LAUNCHER: [Kind; 6] = [
        Kind::Launcher,
        Kind::Home,
        Kind::Personalizing,
        Kind::Demos,
        Kind::DemoRender,
        Kind::Livesplit,
    ];

    /// `GAME_KINDS`: присутствие из моста игры.
    pub const GAME: [Kind; 2] = [Kind::GameMenu, Kind::GameMap];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Launcher => "launcher",
            Kind::Browsing => "browsing",
            Kind::Ready => "ready",
            Kind::Installing => "installing",
            Kind::Updating => "updating",
            Kind::Repairing => "repairing",
            Kind::Playing => "playing",
            Kind::GameMenu => "game_menu",
            Kind::GameMap => "game_map",
            Kind::Home => "home",
            Kind::Personalizing => "personalizing",
            Kind::Demos => "demos",
            Kind::DemoRender => "demo_render",
            Kind::Livesplit => "livesplit",
        }
    }

    pub fn parse(text: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.as_str() == text)
    }

    pub fn is_game(self) -> bool {
        Kind::GAME.contains(&self)
    }

    pub fn is_launcher(self) -> bool {
        Kind::LAUNCHER.contains(&self)
    }
}

/// `ACTIVITY_KINDS`: раздел лаунчера (`discord-rpc-activity` окна) → состояние.
pub const ACTIVITY_KINDS: [(&str, Kind); 4] = [
    ("home", Kind::Home),
    ("personalization", Kind::Personalizing),
    ("demo", Kind::Demos),
    ("livesplit", Kind::Livesplit),
];

/// Состояние раздела `activity` (ключ `ACTIVITY_KINDS` после `String(activity)`); иначе —
/// `None` («Игры»: обычные состояния выбора и запуска).
pub fn activity_kind(activity: Option<&Value>) -> Option<Kind> {
    let key = activity.map_or_else(|| "undefined".to_owned(), js::to_string);
    ACTIVITY_KINDS
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, kind)| *kind)
}

/// `EPISODE_TITLES`: эпизоды Ghosting запускаются как `ghosting-renewed`, в Discord — своё
/// название.
pub const EPISODE_TITLES: [(&str, &str); 2] = [
    ("ep1", "HL2 Ghosting: Episode One"),
    ("ep2", "HL2 Ghosting: Episode Two"),
];

fn episode_title(episode: Option<&str>) -> Option<&'static str> {
    let episode = episode?;
    EPISODE_TITLES
        .iter()
        .find(|(id, _)| *id == episode)
        .map(|(_, title)| *title)
}

/// Язык текстов лаунчера в присутствии: всё, кроме `"en"`, — русский.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    Ru,
    En,
}

impl Locale {
    pub const ALL: [Locale; 2] = [Locale::Ru, Locale::En];

    pub fn as_str(self) -> &'static str {
        match self {
            Locale::Ru => "ru",
            Locale::En => "en",
        }
    }

    fn of(value: Option<&Value>) -> Locale {
        if value.and_then(Value::as_str) == Some("en") {
            Locale::En
        } else {
            Locale::Ru
        }
    }
}

/// `TEXT[locale].launcherDetails`.
pub const LAUNCHER_DETAILS: &str = "HLSR Launcher";

/// `TEXT[locale][kind]`: строка состояния лаунчера; у видов игры её нет.
pub fn kind_text(locale: Locale, kind: Kind) -> Option<&'static str> {
    let (ru, en) = match kind {
        Kind::Launcher => ("В лаунчере", "In the launcher"),
        Kind::Browsing => ("Выбирает сборку", "Selecting a build"),
        Kind::Ready => ("Готов к запуску", "Ready to play"),
        Kind::Installing => ("Установка сборки", "Installing build"),
        Kind::Updating => ("Обновление сборки", "Updating build"),
        Kind::Repairing => ("Проверка файлов", "Repairing files"),
        Kind::Playing => ("В игре", "In game"),
        Kind::Home => ("На главной", "On the home page"),
        Kind::Personalizing => ("Настраивает игру", "Customizing the game"),
        Kind::Demos => ("Смотрит демки", "Watching demos"),
        Kind::DemoRender => ("Рендерит демку", "Rendering a demo"),
        Kind::Livesplit => ("Настраивает LiveSplit", "Setting up LiveSplit"),
        Kind::GameMenu | Kind::GameMap => return None,
    };
    Some(match locale {
        Locale::Ru => ru,
        Locale::En => en,
    })
}

/// `TEXT[locale].buttons[key]`.
pub fn button_label(locale: Locale, key: &str) -> Option<&'static str> {
    match (key, locale) {
        ("site", Locale::Ru) => Some("Сайт HLSR"),
        ("site", Locale::En) => Some("HLSR website"),
        ("discord", _) => Some("Discord"),
        _ => None,
    }
}

/// `GAMES`: сборка → заголовок присутствия.
pub const GAMES: [(&str, &str); 5] = [
    ("ghosting-renewed", "Half-Life 2: Ghosting Mod: Renewed"),
    ("hl2-4260", "Half-Life 2 (4260)"),
    ("hl2-2707", "Half-Life 2 (2707)"),
    ("hl2-5288", "Half-Life 2 (5288)"),
    ("hl1-won", "Half-Life (WON)"),
];

/// `GAME_ALIASES`: Steam AppID → сборка.
pub const GAME_ALIASES: [(&str, &str); 3] = [
    ("70", "hl1-won"),
    ("218", "ghosting-renewed"),
    ("220", "hl2-4260"),
];

/// `FORBIDDEN_ACTIVITY_KEYS`: чего в активности быть не должно (личные данные, секреты).
pub const FORBIDDEN_ACTIVITY_KEYS: [&str; 18] = [
    "steamId",
    "steamID",
    "personaName",
    "userName",
    "installPath",
    "path",
    "args",
    "launchArgs",
    "installationId",
    "ip",
    "crashReason",
    "dump",
    "runFile",
    "config",
    "partyId",
    "matchSecret",
    "joinSecret",
    "spectateSecret",
];

/// `gameKeyOf(gameId)`: `String(gameId)` — ключ [`GAMES`] или AppID из [`GAME_ALIASES`].
fn game_key_of(game_id: Option<&Value>) -> Option<&'static str> {
    let key = game_id.map_or_else(String::new, js::to_string);
    GAMES
        .iter()
        .find(|(id, _)| *id == key)
        .map(|(id, _)| *id)
        .or_else(|| {
            GAME_ALIASES
                .iter()
                .find(|(alias, _)| *alias == key)
                .map(|(_, id)| *id)
        })
}

fn game_title(key: Option<&str>) -> Option<&'static str> {
    let key = key?;
    GAMES
        .iter()
        .find(|(id, _)| *id == key)
        .map(|(_, title)| *title)
}

/// Положительное конечное `Number(value)`, `Math.floor`.
fn positive_floor(value: Option<&Value>) -> Option<f64> {
    let x = to_number(value);
    (x.is_finite() && x > 0.0).then(|| x.floor())
}

/// `sameGame(a, b)`: одна и та же игра — id лаунчера (218) и id сборки (`ghosting-renewed`)
/// сравниваются через [`GAME_ALIASES`]; `null` — не та же.
pub fn same_game(a: Option<&Value>, b: Option<&Value>) -> bool {
    let (Some(a), Some(b)) = (a.filter(|a| !a.is_null()), b.filter(|b| !b.is_null())) else {
        return false;
    };
    match (game_key_of(Some(a)), game_key_of(Some(b))) {
        (Some(key_a), Some(key_b)) => key_a == key_b,
        _ => js::to_string(a) == js::to_string(b),
    }
}

/// Нормализованная модель присутствия (результат `normalize`).
#[derive(Debug, Clone, PartialEq)]
pub struct Presence {
    pub kind: Kind,
    /// Ключ [`GAMES`]; у видов без игры ([`Kind::LAUNCHER`]) — `None`.
    pub game_id: Option<&'static str>,
    /// Ключ [`EPISODE_TITLES`]; только у `ghosting-renewed`.
    pub episode: Option<&'static str>,
    /// Начало сессии (мс), положительное целое число JS.
    pub session_started_at_ms: Option<f64>,
    pub locale: Locale,
    /// Только у [`Kind::GameMap`].
    pub map: Option<String>,
    /// Только у видов игры.
    pub fps: Option<i64>,
    pub running: bool,
    pub run_started_at_ms: Option<f64>,
    pub final_time: bool,
}

/// `normalize(state)`: неизвестный вид — лаунчер; вид со сборкой без известной сборки —
/// лаунчер; карта без верного имени — меню игры; забег — только на карте с началом > 0;
/// итоговое время — только на [`map_assets::FINAL_TIME_MAP`] без забега.
pub fn normalize(state: &Value) -> Presence {
    let source = Some(state).filter(|value| value.is_object() || value.is_array());
    let get = |key: &str| field(source, key);

    let kind = get("kind")
        .and_then(Value::as_str)
        .and_then(Kind::parse)
        .unwrap_or(Kind::Launcher);
    let game_key = game_key_of(get("gameId"));

    let mut effective = if !kind.is_launcher() && game_key.is_none() {
        Kind::Launcher
    } else {
        kind
    };

    let mut map = None;
    if effective == Kind::GameMap {
        map = map_assets::normalize_map_name(get("map").and_then(Value::as_str));
        if map.is_none() {
            effective = Kind::GameMenu;
        }
    }

    let session_started_at_ms = positive_floor(get("sessionStartedAtMs"));
    let fps = if effective.is_game() {
        map_assets::normalize_fps(get("fps").and_then(Value::as_f64))
    } else {
        None
    };

    let mut running = false;
    let mut run_started_at_ms = None;
    if effective == Kind::GameMap && get("running") == Some(&Value::Bool(true)) {
        run_started_at_ms = positive_floor(get("runStartedAtMs"));
        running = run_started_at_ms.is_some();
    }

    let final_time = effective == Kind::GameMap
        && get("finalTime") == Some(&Value::Bool(true))
        && !running
        && map.as_deref() == Some(map_assets::FINAL_TIME_MAP);

    let game_id = if effective.is_launcher() {
        None
    } else {
        game_key
    };
    // shortcut: эпизод — только строка; JS принял бы и `["ep1"]` (ключ через `String`), такого
    // сигнала у лаунчера нет.
    let episode = if game_id == Some("ghosting-renewed") {
        let episode = get("episode").and_then(Value::as_str);
        EPISODE_TITLES
            .iter()
            .find(|(id, _)| Some(*id) == episode)
            .map(|(id, _)| *id)
    } else {
        None
    };

    Presence {
        kind: effective,
        game_id,
        episode,
        session_started_at_ms,
        locale: Locale::of(get("locale")),
        map,
        fps,
        running,
        run_started_at_ms,
        final_time,
    }
}

/// `same(a, b)`: одинаковые модели (тогда присутствие не переотправляется).
pub fn same(a: Option<&Presence>, b: Option<&Presence>) -> bool {
    a == b
}

fn opt_number(value: Option<f64>) -> Value {
    value.map_or(Value::Null, js_number)
}

impl Presence {
    /// Модель как объект 0.4.7.
    pub fn to_json(&self) -> Value {
        json!({
            "kind": self.kind.as_str(),
            "gameId": self.game_id,
            "episode": self.episode,
            "sessionStartedAtMs": opt_number(self.session_started_at_ms),
            "locale": self.locale.as_str(),
            "map": self.map,
            "fps": self.fps,
            "running": self.running,
            "runStartedAtMs": opt_number(self.run_started_at_ms),
            "finalTime": self.final_time,
        })
    }

    /// `toActivity(model, {version})` для уже нормализованной модели (`normalize` повторно
    /// ничего не меняет). `version` — версия лаунчера; пустая — без версии.
    pub fn activity(&self, version: Option<&str>) -> Activity {
        let locale = self.locale;
        let episode = episode_title(self.episode);
        let title = episode.unwrap_or(game_text::TITLE);
        let mut activity = Activity {
            details: episode
                .or_else(|| game_title(self.game_id))
                .unwrap_or(LAUNCHER_DETAILS)
                .to_owned(),
            state: kind_text(locale, self.kind).map(str::to_owned),
            large_image_key: assets::LARGE,
            large_image_text: match version.filter(|v| !v.is_empty()) {
                Some(version) => format!("HLSR v{version}"),
                None => "HLSR".to_owned(),
            },
            instance: false,
            small_image_key: None,
            small_image_text: None,
            start_timestamp: self.session_started_at_ms,
            buttons: Vec::new(),
        };

        let fps_suffix = self.fps.map_or_else(String::new, |fps| {
            format!(" | {}: {fps}", game_text::FPS_LABEL)
        });
        match self.kind {
            Kind::GameMenu => {
                activity.details = format!("{}: {title}", game_text::GAME_LABEL);
                activity.state = Some(format!("{}{fps_suffix}", game_text::MENU_STATE));
                activity.large_image_key = assets::MENU;
                activity.large_image_text = title.to_owned();
                activity.small_image_key = Some(assets::SMALL_IDLE);
                activity.small_image_text = Some(game_text::MENU_STATE);
            }
            Kind::GameMap => {
                let map = self.map.as_deref();
                activity.details = map.unwrap_or_default().to_owned();
                activity.state = Some(format!("{}{fps_suffix}", game_text::PLAYING_STATE));
                activity.large_image_key = map_assets::large_image_key_for(map, self.final_time);
                activity.large_image_text = if map_assets::is_known_map(map) {
                    map.unwrap_or_default().to_owned()
                } else {
                    title.to_owned()
                };
                if self.running {
                    activity.small_image_key = Some(assets::SMALL_RUNNING);
                    activity.small_image_text = Some(game_text::RUNNING_SMALL);
                } else {
                    activity.small_image_key = Some(assets::SMALL_IDLE);
                    activity.small_image_text = Some(game_text::PLAYING_STATE);
                }
            }
            _ => {}
        }

        activity.buttons = button_urls()
            .into_iter()
            .filter_map(|(key, url)| button_label(locale, key).map(|label| Button { label, url }))
            .take(2)
            .collect();
        activity
    }
}

/// Кнопка присутствия.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub label: &'static str,
    pub url: &'static str,
}

/// Активность для Discord (результат `toActivity`).
#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    pub details: String,
    /// У каждого вида есть строка; `None` не бывает, но тип повторяет `t[kind]` JS.
    pub state: Option<String>,
    pub large_image_key: &'static str,
    pub large_image_text: String,
    pub instance: bool,
    pub small_image_key: Option<&'static str>,
    pub small_image_text: Option<&'static str>,
    pub start_timestamp: Option<f64>,
    pub buttons: Vec<Button>,
}

impl Activity {
    /// Активность как объект 0.4.7: ключи в том же порядке, необязательные — только если есть.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("details".into(), json!(self.details));
        if let Some(state) = &self.state {
            out.insert("state".into(), json!(state));
        }
        out.insert("largeImageKey".into(), json!(self.large_image_key));
        out.insert("largeImageText".into(), json!(self.large_image_text));
        out.insert("instance".into(), json!(self.instance));
        if let Some(key) = self.small_image_key {
            out.insert("smallImageKey".into(), json!(key));
        }
        if let Some(text) = self.small_image_text {
            out.insert("smallImageText".into(), json!(text));
        }
        if let Some(start) = self.start_timestamp {
            out.insert("startTimestamp".into(), js_number(start));
        }
        if !self.buttons.is_empty() {
            let buttons: Vec<Value> = self
                .buttons
                .iter()
                .map(|b| json!({"label": b.label, "url": b.url}))
                .collect();
            out.insert("buttons".into(), Value::Array(buttons));
        }
        Value::Object(out)
    }
}

/// `toActivity(state, {version})`: нормализовать и собрать активность.
pub fn to_activity(state: &Value, version: Option<&str>) -> Activity {
    normalize(state).activity(version)
}

/// `kindFromDelivery(signal)`: вид по сигналу доставки (`{state, gameId}`).
pub fn kind_from_delivery(signal: &Value) -> Kind {
    let state = field(Some(signal), "state")
        .filter(|value| js::truthy(value))
        .map_or_else(String::new, js::to_string);
    match state.as_str() {
        "installing" | "downloading" | "extracting" => Kind::Installing,
        "updating" | "patching" => Kind::Updating,
        "repairing" | "verifying" => Kind::Repairing,
        "installed" | "ready" => Kind::Ready,
        _ if field(Some(signal), "gameId").is_some_and(|id| !id.is_null()) => Kind::Browsing,
        _ => Kind::Launcher,
    }
}
